// The watch command: push new and changed files from a folder, surviving restarts.
//
// (c) Copyright 2026 Liminal HQ, Scott Morris
// SPDX-License-Identifier: Apache-2.0 OR MIT

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use anyhow::{Context, Result};
use mailslot_core::config::Paths;
use mailslot_core::outbox::Outbox;
use mailslot_core::proto::MailslotLink;
use mailslot_core::watch::DirWatcher;
use sha2::{Digest, Sha256};
use tokio::signal::unix::{SignalKind, signal};

use crate::WatchArgs;
use crate::send::{print_error, print_outcome, reporting_client};

fn state_file(a: &WatchArgs, dir: &Path, link: &MailslotLink) -> PathBuf {
	if let Some(f) = &a.state {
		return f.clone();
	}
	let paths = match &a.home {
		Some(h) => Paths::in_dir(h),
		None => Paths::xdg(),
	};
	let key = format!("{}\n{}:{}", dir.display(), link.host, link.port);
	let id = hex::encode(&Sha256::digest(key.as_bytes())[..8]);
	paths.data_dir.join("outbox").join(format!("{id}.json"))
}

fn hidden(p: &Path) -> bool {
	p.file_name()
		.is_some_and(|n| n.to_string_lossy().starts_with('.'))
}

/// Watches the folder with inotify and sends a file once it has stopped changing.
///
/// The queue is saved after every change, so files waiting when the process stops (or
/// the network drops) are sent on the next run. Content hashing on the server still
/// skips anything it already has, and interrupted uploads resume.
pub async fn run(a: WatchArgs) -> Result<ExitCode> {
	mailslot_core::tls::ensure_default_provider();
	let link = MailslotLink::parse(&a.link).context("the link is not valid")?;
	let client = reporting_client(link.clone(), &a.client)?;
	let json = a.client.json;
	let dir = tokio::fs::canonicalize(&a.dir)
		.await
		.with_context(|| format!("cannot open {}", a.dir.display()))?;
	let file = state_file(&a, &dir, &link);
	let first_run = !tokio::fs::try_exists(&file).await.unwrap_or(false);
	let mut outbox = Outbox::open(&file).await?;

	let mut watcher = DirWatcher::start(&dir, Duration::from_millis(a.debounce_ms.max(100)))?;
	let existing: Vec<PathBuf> = mailslot_core::client::expand_paths(std::slice::from_ref(&dir))
		.await?
		.into_iter()
		.filter(|p| !hidden(p))
		.collect();
	if a.skip_existing && first_run {
		outbox.mark_done(&existing).await?;
	} else {
		for p in &existing {
			outbox.enqueue_if_changed(p).await?;
		}
	}
	eprintln!(
		"Watching {} ({} waiting; Ctrl+C to stop)",
		dir.display(),
		outbox.pending_len()
	);

	let mut term = signal(SignalKind::terminate())?;
	let mut int = signal(SignalKind::interrupt())?;
	loop {
		let mut retry_wait: Option<Duration> = None;
		while let Some(p) = outbox.next() {
			if !tokio::fs::try_exists(&p).await.unwrap_or(false) {
				outbox.forget(&p).await?;
				continue;
			}
			match client.send_file(&p, None).await {
				Ok(o) => {
					print_outcome(&o, json);
					outbox.complete(&p).await?;
				}
				Err(e) => {
					print_error(&p.display().to_string(), &e, json);
					outbox.defer(&p).await?;
					let n = outbox.attempts(&p).min(5);
					retry_wait = Some(
						Duration::from_secs(a.interval.max(1))
							.saturating_mul(1 << n)
							.min(Duration::from_secs(120)),
					);
					break;
				}
			}
		}
		let sleep = async {
			match retry_wait {
				Some(d) => tokio::time::sleep(d).await,
				None => std::future::pending::<()>().await,
			}
		};
		tokio::select! {
			_ = term.recv() => break,
			_ = int.recv() => break,
			ev = watcher.next() => match ev {
				Some(p) => {
					outbox.enqueue_if_changed(&p).await?;
				}
				None => break,
			},
			() = sleep => {}
		}
	}
	Ok(ExitCode::SUCCESS)
}
