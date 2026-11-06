// Mailslot IPC: a local JSON-RPC 2.0 API over a Unix socket.
//
// (c) Copyright 2026 Liminal HQ, Scott Morris
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A local JSON-RPC 2.0 API for the Mailslot [`Engine`](mailslot_core::Engine).
//!
//! Frontends written in other languages (Qt, WinUI, a script) drive the same engine
//! through a socket instead of linking to Rust. [`Daemon`] serves the API for an engine
//! and `mailslotd` is a ready-made headless process around it. [`Client`] is the Rust
//! side of the same protocol.
//!
//! # Transport
//!
//! Newline-delimited JSON over a Unix domain socket: each request, response and
//! notification is one JSON object on one line. The socket is
//! `$XDG_RUNTIME_DIR/ca.liminalhq.mailslot/mailslotd.sock`; the directory is created
//! with mode `0700` and the socket is `0600`, so only the owning user can connect. See
//! [`default_socket_path`]. Batches are not supported.
//!
//! On Windows the intended transport is a named pipe. That is not implemented yet:
//! TODO(windows): serve the same protocol over `\\.\pipe\ca.liminalhq.mailslot`. The
//! protocol types and [`Client`] logic are transport-agnostic apart from connecting.
//!
//! # Methods
//!
//! | Method | Params | Result |
//! |---|---|---|
//! | `ping` | none | `"pong"` |
//! | `version` | none | `{"daemon", "protocol"}` |
//! | `state` | none | the engine state snapshot |
//! | `start` | none | the engine state |
//! | `stop` | none | `null` |
//! | `pause`, `resume` | none | `null` |
//! | `rotate_token` | none | the engine state with the new links |
//! | `settings.get` | none | the full settings |
//! | `settings.update` | the full settings | `null` |
//! | `settings.reset` | `{"section"?}` (`sharing`, `storage`, `limits`, `security`, `network`, `notifications`) | the settings after the reset |
//! | `internet.set` | `{"enabled": bool}` | `null` |
//! | `send` | `{"link", "paths", "options"?}` | `{"ids": [..]}` |
//! | `send.cancel` | `{"id"}` | `null` |
//! | `check_link` | `{"link"}` | the server's health |
//! | `discover` | `{"timeout_secs"?}` | nearby shares (mDNS) |
//! | `history.get` | none | history entries |
//! | `history.clear` | none | `null` |
//! | `subscribe`, `unsubscribe` | none | `true` |
//! | `shutdown` | none | `null`, then the daemon exits |
//!
//! After `subscribe`, every engine event is pushed to the connection as a notification
//! (`{"jsonrpc":"2.0","method":"event","params":<event>}`). Events use serde's external
//! tagging, for example `{"Upload":{"Finished":{...}}}`.
//!
//! Errors use JSON-RPC codes: `-32700` parse error, `-32600` invalid request, `-32601`
//! unknown method, `-32602` invalid params, and `-32000` for an engine error (the
//! message is human-readable). The state snapshot contains the share links, which hold
//! the secret; the socket permissions are what protect it.
//!
//! # Example
//!
//! ```no_run
//! # async fn demo() -> Result<(), Box<dyn std::error::Error>> {
//! use mailslot_ipc::{Client, default_socket_path};
//!
//! let client = Client::connect(default_socket_path()).await?;
//! let state = client.start().await?;
//! println!("sharing at {}", state.urls[0].url);
//! # Ok(())
//! # }
//! ```

mod client;
mod protocol;
mod server;

pub use client::{Client, ClientError};
pub use protocol::{
	PROTOCOL_VERSION, RpcError, SendOptions, SendParams, codes, default_socket_path,
};
pub use server::{Daemon, bind};
