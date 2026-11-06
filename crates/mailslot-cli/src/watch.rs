// The watch command: push new and changed files from a folder.
//
// (c) Copyright 2026 Liminal HQ, Scott Morris
// SPDX-License-Identifier: Apache-2.0 OR MIT

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result};
use mailslot_core::proto::MailslotLink;
use tokio::signal::unix::{SignalKind, signal};

use crate::WatchArgs;
use crate::send::{print_error, print_outcome, reporting_client};

#[derive(Clone, Copy, PartialEq, Eq)]
struct Stamp {
	len: u64,
	modified: Option<SystemTime>,
}

async fn scan(dir: &PathBuf) -> Result<HashMap<PathBuf, Stamp>> {
	let mut out = HashMap::new();
	for p in mailslot_core::client::expand_paths(std::slice::from_ref(dir)).await? {
		if let Ok(m) = tokio::fs::metadata(&p).await {
			out.insert(
				p,
				Stamp {
					len: m.len(),
					modified: m.modified().ok(),
				},
			);
		}
	}
	Ok(out)
}

/// Polls the folder and sends a file once its size and modification time have stopped
/// changing between two scans. Content hashing makes repeats cheap: the server skips
/// anything it already has, and interrupted uploads resume.
pub async fn run(a: WatchArgs) -> Result<ExitCode> {
	mailslot_core::tls::ensure_default_provider();
	let link = MailslotLink::parse(&a.link).context("the link is not valid")?;
	let client = reporting_client(link, &a.client)?;
	let json = a.client.json;
	let mut sent: HashMap<PathBuf, Stamp> = HashMap::new();
	let mut pending: HashMap<PathBuf, Stamp> = HashMap::new();
	if a.skip_existing {
		sent = scan(&a.dir).await?;
	}
	eprintln!("Watching {} (Ctrl+C to stop)", a.dir.display());
	let mut term = signal(SignalKind::terminate())?;
	let mut int = signal(SignalKind::interrupt())?;
	let mut tick = tokio::time::interval(Duration::from_secs(a.interval.max(1)));
	loop {
		tokio::select! {
			_ = term.recv() => break,
			_ = int.recv() => break,
			_ = tick.tick() => {}
		}
		let now = match scan(&a.dir).await {
			Ok(n) => n,
			Err(e) => {
				eprintln!("scan failed: {e}");
				continue;
			}
		};
		let mut ready = Vec::new();
		for (p, st) in &now {
			if sent.get(p) == Some(st) {
				continue;
			}
			if pending.get(p) == Some(st) {
				ready.push(p.clone());
			} else {
				pending.insert(p.clone(), *st);
			}
		}
		ready.sort();
		for p in ready {
			let st = now[&p];
			match client.send_file(&p, None).await {
				Ok(o) => {
					print_outcome(&o, json);
					sent.insert(p.clone(), st);
					pending.remove(&p);
				}
				Err(e) => print_error(&p.display().to_string(), &e, json),
			}
		}
	}
	Ok(ExitCode::SUCCESS)
}
