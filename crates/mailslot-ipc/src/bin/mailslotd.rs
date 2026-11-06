// mailslotd: the headless Mailslot daemon.
//
// (c) Copyright 2026 Liminal HQ, Scott Morris
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! `mailslotd` runs the Mailslot engine with no window and serves the JSON-RPC API
//! described in the `mailslot-ipc` crate on a Unix socket.
//!
//! It prints one line, `SOCKET <path>`, once it is listening, and exits on SIGINT,
//! SIGTERM or the `shutdown` method, removing the socket and any router mapping.

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::Parser;
use mailslot_core::EngineBuilder;
use mailslot_core::config::Paths;
use mailslot_ipc::{Daemon, bind, default_socket_path};

/// Headless Mailslot with a local JSON-RPC API.
#[derive(Parser, Debug)]
#[command(name = "mailslotd", version, about)]
struct Args {
	/// Socket path (default: `$XDG_RUNTIME_DIR/ca.liminalhq.mailslot/mailslotd.sock`).
	#[arg(long)]
	socket: Option<PathBuf>,
	/// Keep settings, token and history under this folder instead of the XDG locations.
	#[arg(long)]
	home: Option<PathBuf>,
	/// Do not read or write settings, token or history files.
	#[arg(long)]
	ephemeral: bool,
	/// Start sharing immediately.
	#[arg(long)]
	start: bool,
}

#[cfg(unix)]
#[tokio::main]
async fn main() -> ExitCode {
	match run(Args::parse()).await {
		Ok(()) => ExitCode::SUCCESS,
		Err(e) => {
			eprintln!("mailslotd: {e:#}");
			ExitCode::from(1)
		}
	}
}

#[cfg(not(unix))]
fn main() -> ExitCode {
	// TODO(windows): serve the API on a named pipe.
	eprintln!(
		"mailslotd needs Unix domain sockets; a Windows named-pipe transport is not implemented yet"
	);
	ExitCode::from(1)
}

#[cfg(unix)]
async fn run(a: Args) -> Result<()> {
	use tokio::signal::unix::{SignalKind, signal};

	let _log = mailslot_log::init(mailslot_log::Options { stderr: true });
	mailslot_core::tls::ensure_default_provider();
	let paths = match &a.home {
		Some(h) => Paths::in_dir(h),
		None => Paths::xdg(),
	};
	let engine = EngineBuilder::new(paths)
		.persist(!a.ephemeral)
		.build()
		.await
		.context("preparing the engine")?;
	tracing::info!(version = env!("CARGO_PKG_VERSION"), "mailslotd starting");
	let path = a.socket.unwrap_or_else(default_socket_path);
	let listener = bind(&path)
		.await
		.with_context(|| format!("binding {}", path.display()))?;
	let daemon = Daemon::new(engine.clone());
	println!("SOCKET {}", path.display());
	if a.start {
		engine.start().await.context("starting the share")?;
	}

	let shutdown = daemon.shutdown_token();
	let serving = {
		let daemon = daemon.clone();
		tokio::spawn(async move { daemon.serve(listener).await })
	};
	let mut term = signal(SignalKind::terminate())?;
	let mut int = signal(SignalKind::interrupt())?;
	tokio::select! {
		_ = term.recv() => {}
		_ = int.recv() => {}
		_ = shutdown.cancelled() => {}
	}
	shutdown.cancel();
	let _ = serving.await;
	// Stopping removes any router mapping before the process exits.
	engine.shutdown().await;
	let _ = tokio::fs::remove_file(&path).await;
	Ok(())
}
