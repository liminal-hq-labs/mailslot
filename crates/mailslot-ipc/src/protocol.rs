// Protocol types, error codes and the socket path.
//
// (c) Copyright 2026 Liminal HQ, Scott Morris
// SPDX-License-Identifier: Apache-2.0 OR MIT

use std::path::PathBuf;
use std::time::Duration;

use mailslot_core::client::ClientOptions;
use serde::{Deserialize, Serialize};

/// Revision of the IPC protocol, reported by `version`.
pub const PROTOCOL_VERSION: u32 = 1;

/// JSON-RPC error codes.
pub mod codes {
	/// Invalid JSON was received.
	pub const PARSE_ERROR: i64 = -32700;
	/// The JSON is not a valid request object.
	pub const INVALID_REQUEST: i64 = -32600;
	/// The method does not exist.
	pub const METHOD_NOT_FOUND: i64 = -32601;
	/// The parameters are not valid for the method.
	pub const INVALID_PARAMS: i64 = -32602;
	/// The engine reported an error.
	pub const ENGINE_ERROR: i64 = -32000;
}

/// A JSON-RPC error object.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[error("{message} (code {code})")]
pub struct RpcError {
	/// Error code, see [`codes`].
	pub code: i64,
	/// Human-readable message.
	pub message: String,
	/// Optional extra data.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub data: Option<serde_json::Value>,
}

impl RpcError {
	pub(crate) fn new(code: i64, message: impl Into<String>) -> Self {
		Self {
			code,
			message: message.into(),
			data: None,
		}
	}
}

/// Client behaviour for `send`; every field is optional.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SendOptions {
	/// Files uploaded at once.
	pub parallel: Option<usize>,
	/// Retries after the first attempt.
	pub retries: Option<u32>,
	/// Bandwidth cap in bytes per second.
	pub limit_bytes_per_sec: Option<u64>,
	/// Ask the server whether it already has the file (default `true`).
	pub dedupe: Option<bool>,
	/// Sender label for per-sender folders.
	pub label: Option<String>,
	/// Accept any certificate on an https link without a pin.
	pub insecure: Option<bool>,
}

impl SendOptions {
	pub(crate) fn into_client_options(self) -> ClientOptions {
		let d = ClientOptions::default();
		ClientOptions {
			parallel: self.parallel.unwrap_or(d.parallel),
			retries: self.retries.unwrap_or(d.retries),
			bandwidth_limit: self.limit_bytes_per_sec,
			dedupe: self.dedupe.unwrap_or(d.dedupe),
			sender_label: self.label,
			insecure: self.insecure.unwrap_or(false),
			..d
		}
	}
}

/// Parameters of `send`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SendParams {
	/// A `mailslot://` or `http(s)://` link.
	pub link: String,
	/// Files and folders to send.
	pub paths: Vec<PathBuf>,
	/// Options.
	#[serde(default)]
	pub options: SendOptions,
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub(crate) struct DiscoverParams {
	pub timeout_secs: Option<u64>,
}

impl DiscoverParams {
	pub(crate) fn timeout(&self) -> Duration {
		Duration::from_secs(self.timeout_secs.unwrap_or(3).clamp(1, 30))
	}
}

/// Where the daemon listens: `$XDG_RUNTIME_DIR/ca.liminalhq.mailslot/mailslotd.sock`,
/// falling back to a per-user folder in the temporary directory.
pub fn default_socket_path() -> PathBuf {
	let base = std::env::var_os("XDG_RUNTIME_DIR")
		.map(PathBuf::from)
		.filter(|p| p.is_absolute())
		.unwrap_or_else(|| {
			#[cfg(unix)]
			let uid = rustix_uid();
			#[cfg(not(unix))]
			let uid = 0;
			std::env::temp_dir().join(format!("mailslot-{uid}"))
		});
	base.join(mailslot_core::config::APP_ID)
		.join("mailslotd.sock")
}

#[cfg(unix)]
fn rustix_uid() -> u32 {
	rustix::process::getuid().as_raw()
}
