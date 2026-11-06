// Mailslot core: an async engine for private file-drop sharing, with no UI dependency.
//
// (c) Copyright 2026 Liminal HQ, Scott Morris
// SPDX-License-Identifier: Apache-2.0 OR MIT

#[cfg(feature = "client")]
pub mod client;
pub mod config;
#[cfg(feature = "server")]
mod engine;
pub mod error;
pub mod events;
pub mod history;
pub mod naming;
pub mod netinfo;
#[cfg(feature = "portmap")]
pub mod portmap;
pub mod proto;
#[cfg(feature = "server")]
pub mod server;
pub mod snippets;
#[cfg(feature = "tls")]
pub mod tls;
pub mod token;

pub use error::{Error, Result};

#[cfg(feature = "server")]
pub use engine::{Engine, EngineBuilder, EventStream, SendRequest};
