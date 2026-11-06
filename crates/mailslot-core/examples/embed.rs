// Embeds the Mailslot engine in a plain Rust program with no GUI.
//
// (c) Copyright 2026 Liminal HQ, Scott Morris
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Starts a share in a temporary folder, uploads one file to it with the built-in
//! client, prints the events it sees and shuts down. Everything stays on 127.0.0.1.
//!
//! Run with `cargo run -p mailslot-core --example embed`.

use futures_util::StreamExt;
use mailslot_core::client::{Client, ClientOptions};
use mailslot_core::config::{Config, Paths};
use mailslot_core::events::Event;
use mailslot_core::proto::MailslotLink;
use mailslot_core::{Engine, Result};

#[tokio::main]
async fn main() -> Result<()> {
	let scratch = std::env::temp_dir().join(format!("mailslot-embed-{}", std::process::id()));
	let mut config = Config::default();
	config.sharing.bind = "127.0.0.1".into();
	config.sharing.port = 0;
	config.storage.dest_dir = scratch.join("received");
	config.limits.min_free_mb = 0;

	// No GTK, no global state: the engine runs on the runtime we are already in.
	let engine = Engine::builder()
		.config(config)
		.persist(false)
		.build()
		.await?;
	// `Engine::builder()` reads XDG paths; `EngineBuilder::new(Paths::in_dir(..))` keeps files elsewhere.
	let _ = Paths::in_dir(&scratch);

	let mut events = engine.subscribe();
	let printer = tokio::spawn(async move {
		while let Some(ev) = events.next().await {
			match ev {
				Event::Upload(u) => println!("upload event: {u:?}"),
				Event::StateChanged(s) => println!("state: {:?}", s.status),
				_ => {}
			}
		}
	});

	let state = engine.start().await?;
	let addr = state.addr.expect("listening");
	println!("sharing on {addr}");

	// Send a file to ourselves with the client.
	let file = scratch.join("hello.txt");
	tokio::fs::create_dir_all(&scratch).await?;
	tokio::fs::write(&file, b"hello from the embed example\n").await?;
	let link = MailslotLink::new("127.0.0.1", addr.port(), engine.token());
	let outcome = Client::new(link, ClientOptions::default())?
		.send_file(&file, None)
		.await?;
	println!("sent {} (sha256 {})", outcome.saved_as, outcome.sha256);

	engine.stop().await?;
	printer.abort();
	tokio::fs::remove_dir_all(&scratch).await?;
	Ok(())
}
