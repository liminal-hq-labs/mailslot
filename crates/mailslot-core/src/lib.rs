// Mailslot core: an async engine for private file-drop sharing, with no UI dependency.
//
// (c) Copyright 2026 Liminal HQ, Scott Morris
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Mailslot core: a private file-drop endpoint as a library.
//!
//! Anyone holding the secret link can push files into a folder; nothing is ever served
//! back. The crate has no UI dependency. A frontend drives an [`Engine`] with commands
//! (`start`, `stop`, `rotate_token`, `send`, `update_settings`) and watches a typed
//! [`events::Event`] stream plus [`Engine::state`] snapshots.
//!
//! The engine keeps no global state and builds no hidden runtime: give it a
//! [`tokio::runtime::Handle`] (or run inside a runtime, or ask for a private one with
//! [`EngineBuilder::own_runtime`]). Commands are `async fn`s that run on that runtime, so
//! they can be awaited from any executor, including the GLib main loop.
//!
//! # Features
//!
//! | Feature | Adds |
//! |---|---|
//! | `server` (default) | the HTTP server and the [`Engine`] |
//! | `client` (default) | the resumable [`client`] and `Engine::send` |
//! | `portmap` (default) | UPnP-IGD, PCP and NAT-PMP port mapping |
//! | `tls` (default) | self-signed HTTPS with public-key pinning |
//! | `watch` (default) | debounced folder watching and the persisted send queue |
//! | `discovery` (default) | opt-in mDNS advertising and browsing |
//!
//! # Example
//!
//! Share a folder on the loopback interface, upload a file with the built-in client and
//! shut down:
//!
//! ```
//! # #[tokio::main(flavor = "current_thread")]
//! # async fn main() -> mailslot_core::Result<()> {
//! use mailslot_core::client::{Client, ClientOptions};
//! use mailslot_core::config::Config;
//! use mailslot_core::proto::MailslotLink;
//! use mailslot_core::{Engine, events::Status};
//!
//! let dest = std::env::temp_dir().join(format!("mailslot-doc-{}", std::process::id()));
//! let mut config = Config::default();
//! config.sharing.bind = "127.0.0.1".into();
//! config.sharing.port = 0; // any free port
//! config.storage.dest_dir = dest.join("received");
//! config.limits.min_free_mb = 0;
//!
//! mailslot_core::tls::ensure_default_provider();
//! let engine = Engine::builder().config(config).persist(false).build().await?;
//! let state = engine.start().await?;
//! assert_eq!(state.status, Status::Sharing);
//!
//! let file = dest.join("hello.txt");
//! tokio::fs::create_dir_all(&dest).await?;
//! tokio::fs::write(&file, b"hello").await?;
//! let link = MailslotLink::new("127.0.0.1", state.addr.unwrap().port(), engine.token());
//! let sent = Client::new(link, ClientOptions::default())?.send_file(&file, None).await?;
//! assert_eq!(sent.saved_as, "hello.txt");
//!
//! engine.stop().await?;
//! tokio::fs::remove_dir_all(&dest).await?;
//! # Ok(())
//! # }
//! ```
//!
//! # Embedding
//!
//! `examples/embed.rs` shows the same flow as a plain program. In a Tauri app, wrap the
//! engine in managed state, expose `start`, `stop` and `rotate_token` as commands, and
//! forward [`Engine::subscribe`] events to the frontend with `app.emit`.

#[cfg(feature = "client")]
pub mod client;
pub mod config;
pub mod diagnostics;
#[cfg(feature = "discovery")]
pub mod discovery;
#[cfg(feature = "server")]
mod engine;
pub mod error;
pub mod events;
pub mod history;
pub mod naming;
pub mod netinfo;
#[cfg(feature = "client")]
pub mod outbox;
#[cfg(feature = "portmap")]
pub mod portmap;
pub mod proto;
pub mod redact;
#[cfg(feature = "server")]
pub mod server;
pub mod snippets;
#[cfg(feature = "tls")]
pub mod tls;
pub mod token;
#[cfg(feature = "watch")]
pub mod watch;

pub use error::{Error, Result};

#[cfg(all(feature = "server", feature = "client"))]
pub use engine::SendRequest;
#[cfg(feature = "server")]
pub use engine::{Engine, EngineBuilder, EventStream};
