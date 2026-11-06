// mailslotd binary: starts, serves the API and exits cleanly.
//
// (c) Copyright 2026 Liminal HQ, Scott Morris
// SPDX-License-Identifier: Apache-2.0 OR MIT

#![allow(clippy::disallowed_methods)] // Tests drive a child process and read the file system synchronously.

use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};

use mailslot_core::events::Status;
use mailslot_ipc::Client;

#[tokio::test]
async fn mailslotd_serves_and_cleans_up_on_sigterm() {
	let dir = tempfile::tempdir().unwrap();
	let socket = dir.path().join("sock/mailslotd.sock");
	let mut child = Command::new(env!("CARGO_BIN_EXE_mailslotd"))
		.args(["--ephemeral", "--socket"])
		.arg(&socket)
		.stdout(Stdio::piped())
		.stderr(Stdio::null())
		.spawn()
		.unwrap();
	let mut line = String::new();
	BufReader::new(child.stdout.as_mut().unwrap())
		.read_line(&mut line)
		.unwrap();
	assert_eq!(line.trim(), format!("SOCKET {}", socket.display()));

	let c = Client::connect(&socket).await.unwrap();
	c.ping().await.unwrap();
	assert_eq!(c.state().await.unwrap().status, Status::Stopped);
	let mut cfg = c.settings().await.unwrap();
	cfg.sharing.bind = "127.0.0.1".into();
	cfg.sharing.port = 0;
	cfg.storage.dest_dir = dir.path().join("in");
	cfg.limits.min_free_mb = 0;
	c.update_settings(&cfg).await.unwrap();
	let st = c.start().await.unwrap();
	assert!(st.addr.is_some());
	drop(c);

	let _ = Command::new("kill")
		.args(["-TERM", &child.id().to_string()])
		.status();
	let status = tokio::task::spawn_blocking(move || child.wait().unwrap())
		.await
		.unwrap();
	assert!(status.success());
	assert!(!socket.exists(), "the socket file is removed on exit");
}

#[tokio::test]
async fn the_shutdown_method_stops_the_daemon() {
	let dir = tempfile::tempdir().unwrap();
	let socket = dir.path().join("s.sock");
	let mut child = Command::new(env!("CARGO_BIN_EXE_mailslotd"))
		.args(["--ephemeral", "--socket"])
		.arg(&socket)
		.stdout(Stdio::piped())
		.stderr(Stdio::null())
		.spawn()
		.unwrap();
	let mut line = String::new();
	BufReader::new(child.stdout.as_mut().unwrap())
		.read_line(&mut line)
		.unwrap();
	Client::connect(&socket)
		.await
		.unwrap()
		.shutdown()
		.await
		.unwrap();
	let status = tokio::task::spawn_blocking(move || child.wait().unwrap())
		.await
		.unwrap();
	assert!(status.success());
}
