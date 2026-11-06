// Headless smoke server: prints one URL line, then serves until SIGINT or SIGTERM.
//
// (c) Copyright 2026 Liminal HQ, Scott Morris
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Usage: `cargo run -p mailslot-core --example serve -- --bind 127.0.0.1:0 --dir DIR [--max-mb N]`
//!
//! Prints exactly one line, `URL http://127.0.0.1:PORT/TOKEN/`, to standard output once
//! the server is listening. Exits non-zero if it cannot start.

use std::net::SocketAddr;
use std::path::PathBuf;

use mailslot_core::proto::MailslotLink;
use mailslot_core::server::{Server, ServerConfig, ServerEvent};
use mailslot_core::token::Token;
use tokio::signal::unix::{SignalKind, signal};

struct Args {
	bind: SocketAddr,
	dir: PathBuf,
	max_mb: u64,
	min_free_mb: u64,
	token: Option<String>,
	expiry_secs: Option<u64>,
}

fn parse() -> Result<Args, String> {
	let mut a = Args {
		bind: "127.0.0.1:0".parse().unwrap(),
		dir: PathBuf::from("uploads"),
		max_mb: 200,
		min_free_mb: 0,
		token: None,
		expiry_secs: None,
	};
	let mut it = std::env::args().skip(1);
	while let Some(flag) = it.next() {
		let mut value = || it.next().ok_or_else(|| format!("{flag} needs a value"));
		match flag.as_str() {
			"--bind" => a.bind = value()?.parse().map_err(|e| format!("--bind: {e}"))?,
			"--dir" => a.dir = PathBuf::from(value()?),
			"--max-mb" => a.max_mb = value()?.parse().map_err(|e| format!("--max-mb: {e}"))?,
			"--min-free-mb" => a.min_free_mb = value()?.parse().map_err(|e| format!("--min-free-mb: {e}"))?,
			"--token" => a.token = Some(value()?),
			"--expiry-secs" => a.expiry_secs = Some(value()?.parse().map_err(|e| format!("--expiry-secs: {e}"))?),
			"-h" | "--help" => return Err("usage: serve [--bind ADDR] [--dir DIR] [--max-mb N] [--min-free-mb N] [--token TOKEN] [--expiry-secs N]".into()),
			other => return Err(format!("unknown flag {other}")),
		}
	}
	Ok(a)
}

#[tokio::main]
async fn main() {
	let args = match parse() {
		Ok(a) => a,
		Err(e) => {
			eprintln!("{e}");
			std::process::exit(2);
		}
	};
	let token = match args
		.token
		.as_deref()
		.map(Token::parse)
		.unwrap_or_else(Token::generate)
	{
		Ok(t) => t,
		Err(e) => {
			eprintln!("{e}");
			std::process::exit(2);
		}
	};
	let mut cfg = ServerConfig::new(args.bind, args.dir, token.clone());
	cfg.settings.max_file_bytes = args.max_mb * 1024 * 1024;
	cfg.settings.min_free_bytes = args.min_free_mb * 1024 * 1024;
	cfg.settings.expires_at = args
		.expiry_secs
		.map(|s| std::time::SystemTime::now() + std::time::Duration::from_secs(s));
	let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
	let handle = match Server::start(cfg, tx).await {
		Ok(h) => h,
		Err(e) => {
			eprintln!("cannot start: {e}");
			std::process::exit(1);
		}
	};
	let addr = handle.local_addr();
	let link = MailslotLink::new(addr.ip().to_string(), addr.port(), token);
	println!("URL {}", link.drop_url());

	// Drain events so they do not pile up; log finished files to standard error without the token.
	tokio::spawn(async move {
		while let Some(ev) = rx.recv().await {
			if let ServerEvent::Upload(mailslot_core::events::UploadEvent::Finished {
				saved_as,
				size,
				sha256,
				..
			}) = ev
			{
				eprintln!("saved {saved_as} ({size} bytes) sha256={sha256}");
			}
		}
	});

	let mut term = signal(SignalKind::terminate()).expect("sigterm handler");
	let mut int = signal(SignalKind::interrupt()).expect("sigint handler");
	tokio::select! {
		_ = term.recv() => {}
		_ = int.recv() => {}
	}
	handle.shutdown().await;
}
