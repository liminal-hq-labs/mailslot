// The send command.
//
// (c) Copyright 2026 Liminal HQ, Scott Morris
// SPDX-License-Identifier: Apache-2.0 OR MIT

use std::io::{IsTerminal, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use mailslot_core::client::{Client, ClientOptions, Notice, SendOutcome};
use mailslot_core::proto::MailslotLink;

use crate::{ClientArgs, SendArgs};

pub fn client_options(a: &ClientArgs) -> ClientOptions {
	ClientOptions {
		parallel: a.parallel.max(1),
		retries: a.retries,
		bandwidth_limit: a.limit,
		dedupe: !a.no_dedupe,
		sender_label: a.label.clone(),
		insecure: a.insecure,
		..Default::default()
	}
}

pub fn human_bytes(n: f64) -> String {
	let units = ["B", "KB", "MB", "GB", "TB"];
	let mut v = n;
	let mut i = 0;
	while v >= 1024.0 && i < units.len() - 1 {
		v /= 1024.0;
		i += 1;
	}
	if i == 0 {
		format!("{v:.0} {}", units[i])
	} else {
		format!("{v:.1} {}", units[i])
	}
}

/// Builds a client that reports progress as JSON lines or on a single updating line.
pub fn reporting_client(link: MailslotLink, args: &ClientArgs) -> Result<Client> {
	let json = args.json;
	let tty = std::io::stderr().is_terminal();
	let last_print = Arc::new(Mutex::new(
		std::time::Instant::now() - Duration::from_secs(1),
	));
	let client = Client::new(link, client_options(args))?
		.on_progress(move |p| {
			if json {
				println!("{}", serde_json::json!({ "event": "progress", "name": p.name, "sent": p.sent, "total": p.total, "bytes_per_sec": p.bytes_per_sec }));
			} else if tty {
				let Ok(mut last) = last_print.lock() else { return };
				if last.elapsed() < Duration::from_millis(150) && p.sent < p.total {
					return;
				}
				*last = std::time::Instant::now();
				let pct = (p.sent * 100).checked_div(p.total).map_or_else(|| "?".to_owned(), |v| v.to_string());
				eprint!("\r\x1b[2K{}  {pct}%  {}/s", p.name, human_bytes(p.bytes_per_sec));
				let _ = std::io::stderr().flush();
			}
		})
		.on_notice(move |n| {
			let (event, detail) = match n {
				Notice::Retrying { attempt, delay_secs, reason } => ("retrying", format!("attempt {attempt} in {delay_secs:.1}s ({reason})")),
				Notice::Resumed { offset } => ("resumed", format!("from byte {offset}")),
			};
			if json {
				println!("{}", serde_json::json!({ "event": event, "detail": detail }));
			} else {
				eprintln!("\r\x1b[2K{event}: {detail}");
			}
		});
	Ok(client)
}

pub fn print_outcome(o: &SendOutcome, json: bool) {
	if json {
		println!(
			"{}",
			serde_json::json!({ "event": "done", "name": o.name, "saved_as": o.saved_as, "sha256": o.sha256, "size": o.size, "skipped": o.skipped })
		);
	} else if o.skipped {
		eprint!("\r\x1b[2K");
		println!("{}  {}  (already on the server)", o.sha256, o.name);
	} else {
		eprint!("\r\x1b[2K");
		println!("{}  {}  -> {}", o.sha256, o.name, o.saved_as);
	}
}

pub fn print_error(name: &str, e: &dyn std::fmt::Display, json: bool) {
	if json {
		println!(
			"{}",
			serde_json::json!({ "event": "error", "name": name, "message": e.to_string() })
		);
	} else {
		eprintln!("\r\x1b[2K{name}: {e}");
	}
}

pub async fn run(a: SendArgs) -> Result<ExitCode> {
	mailslot_core::tls::ensure_default_provider();
	let (link_text, files) = a.args.split_last().context("expected FILE... LINK")?;
	let link = MailslotLink::parse(link_text)
		.context("the last argument must be a mailslot:// or http(s):// link")?;
	if files.is_empty() && !a.stdin {
		bail!("give at least one file before the link, or use --stdin");
	}
	let use_stdin = a.stdin;
	let paths: Vec<PathBuf> = files.iter().map(PathBuf::from).collect();
	for p in &paths {
		if !tokio::fs::try_exists(p).await.unwrap_or(false) {
			bail!("{} does not exist", p.display());
		}
	}
	let expanded = mailslot_core::client::expand_paths(&paths).await?;
	let client = reporting_client(link, &a.client)?;
	let json = a.client.json;
	let mut failed = false;

	if use_stdin {
		match client.send_stream(tokio::io::stdin(), &a.name).await {
			Ok(o) => print_outcome(&o, json),
			Err(e) => {
				failed = true;
				print_error(&a.name, &e, json);
			}
		}
	}
	let results = client.send_many(expanded.clone()).await;
	for (path, r) in expanded.iter().zip(results) {
		match r {
			Ok(o) => print_outcome(&o, json),
			Err(e) => {
				failed = true;
				print_error(&path.display().to_string(), &e, json);
			}
		}
	}
	Ok(if failed {
		ExitCode::from(1)
	} else {
		ExitCode::SUCCESS
	})
}
