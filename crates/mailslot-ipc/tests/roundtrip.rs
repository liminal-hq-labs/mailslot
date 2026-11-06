// Client and daemon round trip over a real Unix socket.
//
// (c) Copyright 2026 Liminal HQ, Scott Morris
// SPDX-License-Identifier: Apache-2.0 OR MIT

#![allow(clippy::disallowed_methods)] // Tests read file modes synchronously.

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::time::Duration;

use mailslot_core::config::{Config, Paths};
use mailslot_core::events::{Event, SendEvent, Status, UploadEvent};
use mailslot_core::{Engine, EngineBuilder};
use mailslot_ipc::{Client, ClientError, Daemon, SendParams, bind, codes};
use tempfile::TempDir;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

struct Rig {
	dir: TempDir,
	engine: Engine,
	socket: PathBuf,
	daemon: Daemon,
}

async fn rig() -> Rig {
	mailslot_core::tls::ensure_default_provider();
	let dir = tempfile::tempdir().unwrap();
	let mut cfg = Config::default();
	cfg.sharing.bind = "127.0.0.1".into();
	cfg.sharing.port = 0;
	cfg.storage.dest_dir = dir.path().join("received");
	cfg.limits.min_free_mb = 0;
	let engine = EngineBuilder::new(Paths::in_dir(dir.path()))
		.config(cfg)
		.persist(false)
		.build()
		.await
		.unwrap();
	let socket = dir.path().join("run/ca.liminalhq.mailslot/mailslotd.sock");
	let listener = bind(&socket).await.unwrap();
	let daemon = Daemon::new(engine.clone());
	let d = daemon.clone();
	tokio::spawn(async move { d.serve(listener).await });
	Rig {
		dir,
		engine,
		socket,
		daemon,
	}
}

async fn next_event(
	rx: &mut tokio::sync::broadcast::Receiver<Event>,
	mut pred: impl FnMut(&Event) -> bool,
) -> Event {
	tokio::time::timeout(Duration::from_secs(10), async {
		loop {
			let e = rx.recv().await.expect("events stay open");
			if pred(&e) {
				return e;
			}
		}
	})
	.await
	.expect("timed out waiting for an event")
}

#[tokio::test]
async fn socket_and_folder_are_private() {
	let r = rig().await;
	let sock = std::fs::metadata(&r.socket).unwrap().permissions().mode() & 0o777;
	let dir = std::fs::metadata(r.socket.parent().unwrap())
		.unwrap()
		.permissions()
		.mode()
		& 0o777;
	assert_eq!((sock, dir), (0o600, 0o700));
}

#[tokio::test]
async fn a_second_daemon_cannot_take_a_live_socket_but_a_stale_one_is_replaced() {
	let r = rig().await;
	let err = bind(&r.socket).await.unwrap_err();
	assert_eq!(err.kind(), std::io::ErrorKind::AddrInUse);
	// A leftover socket file with nobody listening is cleaned up.
	let stale = r.dir.path().join("stale.sock");
	drop(std::os::unix::net::UnixListener::bind(&stale).unwrap());
	assert!(stale.exists());
	bind(&stale).await.expect("stale socket replaced");
}

#[tokio::test]
async fn drives_the_engine_end_to_end() {
	let r = rig().await;
	let c = Client::connect(&r.socket).await.unwrap();
	c.ping().await.unwrap();
	let v: serde_json::Value = c.call("version", serde_json::Value::Null).await.unwrap();
	assert_eq!(v["protocol"], 1);
	assert_eq!(c.state().await.unwrap().status, Status::Stopped);

	c.subscribe().await.unwrap();
	let mut events = c.events();
	let st = c.start().await.unwrap();
	assert_eq!(st.status, Status::Sharing);
	next_event(
		&mut events,
		|e| matches!(e, Event::StateChanged(s) if s.status == Status::Sharing),
	)
	.await;
	let url = st.urls[0].url.clone();

	// An upload through the share arrives as a notification.
	let put = reqwest::Client::new()
		.put(format!("{url}hello.txt"))
		.body("hello ipc")
		.send()
		.await
		.unwrap();
	assert_eq!(put.status(), 201);
	let fin = next_event(&mut events, |e| {
		matches!(e, Event::Upload(UploadEvent::Finished { .. }))
	})
	.await;
	let Event::Upload(UploadEvent::Finished { saved_as, size, .. }) = fin else {
		unreachable!()
	};
	assert_eq!((saved_as.as_str(), size), ("hello.txt", 9));
	assert_eq!(c.state().await.unwrap().received_files, 1);

	// Pause and resume.
	c.pause().await.unwrap();
	assert!(c.state().await.unwrap().paused);
	assert_eq!(
		reqwest::Client::new()
			.put(format!("{url}b.txt"))
			.body("x")
			.send()
			.await
			.unwrap()
			.status(),
		503
	);
	c.resume().await.unwrap();
	assert!(!c.state().await.unwrap().paused);

	// Settings round trip and validation.
	let mut cfg = c.settings().await.unwrap();
	assert_eq!(cfg.limits.max_file_mb, 200);
	cfg.limits.max_file_mb = 7;
	c.update_settings(&cfg).await.unwrap();
	assert_eq!(r.engine.config().limits.max_file_mb, 7);
	cfg.sharing.bind = "not an address".into();
	let ClientError::Rpc(e) = c.update_settings(&cfg).await.unwrap_err() else {
		panic!("expected an rpc error")
	};
	assert_eq!(e.code, codes::ENGINE_ERROR);
	let reset: Config = c
		.call("settings.reset", serde_json::json!({ "section": "limits" }))
		.await
		.unwrap();
	assert_eq!(reset.limits.max_file_mb, 200);
	assert!(
		matches!(c.call_raw("settings.reset", serde_json::json!({ "section": "nope" })).await, Err(ClientError::Rpc(e)) if e.code == codes::INVALID_PARAMS)
	);

	// Send a file to our own share through the API and watch the progress events.
	let src = r.dir.path().join("sent.bin");
	std::fs::write(&src, vec![7u8; 300_000]).unwrap();
	let ids = c
		.send(&SendParams {
			link: st.urls[0].link.clone(),
			paths: vec![src],
			options: Default::default(),
		})
		.await
		.unwrap();
	assert_eq!(ids.len(), 1);
	let done = next_event(&mut events, |e| {
		matches!(
			e,
			Event::Send(SendEvent::Finished { .. } | SendEvent::Failed { .. })
		)
	})
	.await;
	assert!(
		matches!(done, Event::Send(SendEvent::Finished { .. })),
		"{done:?}"
	);
	let health = c.check_link(&st.urls[0].link).await.unwrap();
	assert!(health.accepting);
	assert!(
		c.history()
			.await
			.unwrap()
			.iter()
			.any(|h| h.name == "hello.txt")
	);

	// A new link replaces the old one.
	let rotated = c.rotate_token().await.unwrap();
	assert_ne!(rotated.urls[0].url, url);
	assert_eq!(
		reqwest::Client::new()
			.get(format!("{url}health"))
			.send()
			.await
			.unwrap()
			.status(),
		404
	);

	c.stop().await.unwrap();
	assert_eq!(c.state().await.unwrap().status, Status::Stopped);
	assert!(matches!(c.pause().await, Err(ClientError::Rpc(e)) if e.code == codes::ENGINE_ERROR));
	c.shutdown().await.unwrap();
	tokio::time::timeout(
		Duration::from_secs(5),
		r.daemon.shutdown_token().cancelled(),
	)
	.await
	.expect("shutdown request cancels the daemon");
}

async fn ask(
	write: &mut tokio::net::unix::OwnedWriteHalf,
	lines: &mut tokio::io::Lines<BufReader<tokio::net::unix::OwnedReadHalf>>,
	text: &str,
) -> serde_json::Value {
	write.write_all(text.as_bytes()).await.unwrap();
	write.write_all(b"\n").await.unwrap();
	serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap()
}

#[tokio::test]
async fn malformed_traffic_gets_proper_json_rpc_errors() {
	let r = rig().await;
	let stream = tokio::net::UnixStream::connect(&r.socket).await.unwrap();
	let (read, write) = stream.into_split();
	let lines = BufReader::new(read).lines();
	let (mut write, mut lines) = (write, lines);
	let v = ask(&mut write, &mut lines, "{ not json").await;
	assert_eq!(
		(v["error"]["code"].as_i64(), v["id"].is_null()),
		(Some(codes::PARSE_ERROR), true)
	);
	let v = ask(
		&mut write,
		&mut lines,
		r#"[{"jsonrpc":"2.0","id":1,"method":"ping"}]"#,
	)
	.await;
	assert_eq!(v["error"]["code"], codes::INVALID_REQUEST);
	let v = ask(&mut write, &mut lines, r#"{"id":2,"method":"ping"}"#).await;
	assert_eq!(
		(v["error"]["code"].as_i64(), v["id"].as_i64()),
		(Some(codes::INVALID_REQUEST), Some(2))
	);
	let v = ask(
		&mut write,
		&mut lines,
		r#"{"jsonrpc":"2.0","id":3,"method":"nope"}"#,
	)
	.await;
	assert_eq!(
		(v["error"]["code"].as_i64(), v["id"].as_i64()),
		(Some(codes::METHOD_NOT_FOUND), Some(3))
	);
	let v = ask(
		&mut write,
		&mut lines,
		r#"{"jsonrpc":"2.0","id":"abc","method":"send","params":{"link":5}}"#,
	)
	.await;
	assert_eq!(
		(v["error"]["code"].as_i64(), v["id"].as_str()),
		(Some(codes::INVALID_PARAMS), Some("abc"))
	);
	let v = ask(
		&mut write,
		&mut lines,
		r#"{"jsonrpc":"2.0","id":4,"method":"send","params":{"link":"not a link","paths":[]}}"#,
	)
	.await;
	assert_eq!(v["error"]["code"], codes::INVALID_PARAMS);
	// A notification (no id) is executed but gets no reply; the next answer is for the next request.
	write
		.write_all(b"{\"jsonrpc\":\"2.0\",\"method\":\"ping\"}\n")
		.await
		.unwrap();
	let v = ask(
		&mut write,
		&mut lines,
		r#"{"jsonrpc":"2.0","id":5,"method":"ping"}"#,
	)
	.await;
	assert_eq!(
		(v["id"].as_i64(), v["result"].as_str()),
		(Some(5), Some("pong"))
	);
}

#[tokio::test]
async fn unsubscribed_connections_get_no_events() {
	let r = rig().await;
	let quiet = Client::connect(&r.socket).await.unwrap();
	let loud = Client::connect(&r.socket).await.unwrap();
	loud.subscribe().await.unwrap();
	let mut loud_events = loud.events();
	let mut quiet_events = quiet.events();
	r.engine.start().await.unwrap();
	next_event(&mut loud_events, |e| matches!(e, Event::StateChanged(_))).await;
	assert!(
		tokio::time::timeout(Duration::from_millis(300), quiet_events.recv())
			.await
			.is_err()
	);
	r.engine.stop().await.unwrap();
}
