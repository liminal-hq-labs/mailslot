// The engine's log events carry no token, even with every event enabled.
//
// (c) Copyright 2026 Liminal HQ, Scott Morris
// SPDX-License-Identifier: Apache-2.0 OR MIT

#![allow(clippy::disallowed_methods)] // Tests may use blocking std calls for set-up and assertions.

use std::io::Write;
use std::sync::{Arc, Mutex};

use mailslot_core::EngineBuilder;
use mailslot_core::client::{Client, ClientOptions};
use mailslot_core::config::{Config, Paths};
use mailslot_core::proto::MailslotLink;

#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl Write for Captured {
	fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
		self.0.lock().unwrap().extend_from_slice(b);
		Ok(b.len())
	}
	fn flush(&mut self) -> std::io::Result<()> {
		Ok(())
	}
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Captured {
	type Writer = Captured;
	fn make_writer(&'a self) -> Captured {
		self.clone()
	}
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn engine_events_never_contain_the_token() {
	let sink = Captured::default();
	tracing_subscriber::fmt()
		.with_max_level(tracing::Level::TRACE)
		.with_ansi(false)
		.with_writer(sink.clone())
		.init();

	let dir = tempfile::tempdir().unwrap();
	let mut cfg = Config::default();
	cfg.sharing.bind = "127.0.0.1".into();
	cfg.sharing.port = 0;
	cfg.storage.dest_dir = dir.path().join("received");
	cfg.limits.min_free_mb = 0;
	mailslot_core::tls::ensure_default_provider();
	let engine = EngineBuilder::new(Paths::in_dir(dir.path()))
		.config(cfg)
		.persist(false)
		.build()
		.await
		.unwrap();
	let state = engine.start().await.unwrap();
	let port = state.addr.unwrap().port();
	let token = engine.token();

	// A good upload, a request with the wrong token and a rejected one.
	let src = dir.path().join("a.txt");
	std::fs::write(&src, b"hello").unwrap();
	let good = MailslotLink::new("127.0.0.1", port, token.clone());
	Client::new(good, ClientOptions::default())
		.unwrap()
		.send_file(&src, None)
		.await
		.unwrap();
	let wrong = MailslotLink::new(
		"127.0.0.1",
		port,
		mailslot_core::token::Token::parse("WrongTokenWrongTokenWrongToken123").unwrap(),
	);
	let _ = Client::new(wrong, ClientOptions::default())
		.unwrap()
		.health()
		.await;
	engine.pause().await.unwrap();
	engine.rotate_token().await.unwrap();
	engine.stop().await.unwrap();

	let out = String::from_utf8(sink.0.lock().unwrap().clone()).unwrap();
	assert!(out.contains("upload finished"), "{out}");
	assert!(out.contains("sharing"), "{out}");
	assert!(!out.contains(token.expose()), "the token leaked:\n{out}");
}
