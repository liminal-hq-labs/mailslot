// A Rust client for the Mailslot IPC protocol.
//
// (c) Copyright 2026 Liminal HQ, Scott Morris
// SPDX-License-Identifier: Apache-2.0 OR MIT

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use mailslot_core::client::Health;
use mailslot_core::config::Config;
use mailslot_core::discovery::Peer;
use mailslot_core::events::{EngineState, Event, SendId};
use mailslot_core::history::HistoryEntry;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{broadcast, oneshot};

use crate::protocol::{RpcError, SendParams};

/// Why a call failed.
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
	/// The socket could not be used.
	#[error("socket error: {0}")]
	Io(#[from] std::io::Error),
	/// The daemon answered with an error.
	#[error(transparent)]
	Rpc(#[from] RpcError),
	/// The connection closed before an answer arrived.
	#[error("the connection to the daemon closed")]
	Closed,
	/// The answer was not in the expected shape.
	#[error("unexpected answer: {0}")]
	Decode(String),
	/// This platform has no transport yet.
	#[error("unix sockets are not available on this platform")]
	Unsupported,
}

type Pending = Mutex<HashMap<u64, oneshot::Sender<Result<Value, RpcError>>>>;

struct Inner {
	#[cfg(unix)]
	write: tokio::sync::Mutex<tokio::net::unix::OwnedWriteHalf>,
	pending: Pending,
	next: AtomicU64,
	events: broadcast::Sender<Event>,
}

/// A connection to a running daemon. Cheap to clone.
#[derive(Clone)]
pub struct Client {
	inner: Arc<Inner>,
}

impl Client {
	/// Connects to the daemon's socket.
	#[cfg(unix)]
	pub async fn connect(path: impl AsRef<Path>) -> Result<Self, ClientError> {
		let stream = tokio::net::UnixStream::connect(path.as_ref()).await?;
		let (read, write) = stream.into_split();
		let (events, _) = broadcast::channel(1024);
		let inner = Arc::new(Inner {
			write: tokio::sync::Mutex::new(write),
			pending: Mutex::new(HashMap::new()),
			next: AtomicU64::new(1),
			events,
		});
		let reader = inner.clone();
		tokio::spawn(async move {
			let mut lines = BufReader::new(read).lines();
			while let Ok(Some(line)) = lines.next_line().await {
				let Ok(v) = serde_json::from_str::<Value>(&line) else {
					continue;
				};
				if let Some(id) = v.get("id").and_then(Value::as_u64) {
					let result = match v.get("error") {
						Some(e) => Err(serde_json::from_value::<RpcError>(e.clone())
							.unwrap_or_else(|e| RpcError {
								code: -1,
								message: e.to_string(),
								data: None,
							})),
						None => Ok(v.get("result").cloned().unwrap_or(Value::Null)),
					};
					let tx = reader.pending.lock().ok().and_then(|mut p| p.remove(&id));
					if let Some(tx) = tx {
						let _ = tx.send(result);
					}
				} else if v.get("method").and_then(Value::as_str) == Some("event")
					&& let Some(ev) = v
						.get("params")
						.and_then(|p| serde_json::from_value::<Event>(p.clone()).ok())
				{
					let _ = reader.events.send(ev);
				}
			}
			// Connection closed: fail everything still waiting.
			if let Ok(mut p) = reader.pending.lock() {
				p.clear();
			}
		});
		Ok(Self { inner })
	}

	/// Connects to the daemon's socket.
	#[cfg(not(unix))]
	pub async fn connect(_path: impl AsRef<Path>) -> Result<Self, ClientError> {
		// TODO(windows): connect to the named pipe.
		Err(ClientError::Unsupported)
	}

	/// Calls a method and returns the raw result.
	pub async fn call_raw(&self, method: &str, params: Value) -> Result<Value, ClientError> {
		let id = self.inner.next.fetch_add(1, Ordering::SeqCst);
		let (tx, rx) = oneshot::channel();
		self.inner
			.pending
			.lock()
			.map_err(|_| ClientError::Closed)?
			.insert(id, tx);
		let line =
			json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }).to_string();
		#[cfg(unix)]
		{
			let mut w = self.inner.write.lock().await;
			w.write_all(line.as_bytes()).await?;
			w.write_all(b"\n").await?;
		}
		#[cfg(not(unix))]
		let _ = line;
		match rx.await {
			Ok(r) => r.map_err(ClientError::Rpc),
			Err(_) => Err(ClientError::Closed),
		}
	}

	/// Calls a method and decodes the result.
	pub async fn call<T: DeserializeOwned>(
		&self,
		method: &str,
		params: impl Serialize,
	) -> Result<T, ClientError> {
		let params =
			serde_json::to_value(params).map_err(|e| ClientError::Decode(e.to_string()))?;
		let v = self.call_raw(method, params).await?;
		serde_json::from_value(v).map_err(|e| ClientError::Decode(e.to_string()))
	}

	/// A receiver of pushed events. Call [`Client::subscribe`] to start the flow.
	pub fn events(&self) -> broadcast::Receiver<Event> {
		self.inner.events.subscribe()
	}

	/// Starts event notifications on this connection.
	pub async fn subscribe(&self) -> Result<(), ClientError> {
		self.call::<bool>("subscribe", Value::Null)
			.await
			.map(|_| ())
	}

	/// Checks the daemon is alive.
	pub async fn ping(&self) -> Result<(), ClientError> {
		self.call::<String>("ping", Value::Null).await.map(|_| ())
	}

	/// The engine state.
	pub async fn state(&self) -> Result<EngineState, ClientError> {
		self.call("state", Value::Null).await
	}

	/// Starts sharing.
	pub async fn start(&self) -> Result<EngineState, ClientError> {
		self.call("start", Value::Null).await
	}

	/// Stops sharing.
	pub async fn stop(&self) -> Result<(), ClientError> {
		self.call::<Value>("stop", Value::Null).await.map(|_| ())
	}

	/// Pauses accepting files.
	pub async fn pause(&self) -> Result<(), ClientError> {
		self.call::<Value>("pause", Value::Null).await.map(|_| ())
	}

	/// Resumes accepting files.
	pub async fn resume(&self) -> Result<(), ClientError> {
		self.call::<Value>("resume", Value::Null).await.map(|_| ())
	}

	/// Makes a new link.
	pub async fn rotate_token(&self) -> Result<EngineState, ClientError> {
		self.call("rotate_token", Value::Null).await
	}

	/// The current settings.
	pub async fn settings(&self) -> Result<Config, ClientError> {
		self.call("settings.get", Value::Null).await
	}

	/// Saves new settings.
	pub async fn update_settings(&self, config: &Config) -> Result<(), ClientError> {
		self.call::<Value>("settings.update", config)
			.await
			.map(|_| ())
	}

	/// Sends files to a link and returns the transfer ids.
	pub async fn send(&self, params: &SendParams) -> Result<Vec<SendId>, ClientError> {
		#[derive(serde::Deserialize)]
		struct R {
			ids: Vec<SendId>,
		}
		Ok(self.call::<R>("send", params).await?.ids)
	}

	/// Asks a link's server for its limits.
	pub async fn check_link(&self, link: &str) -> Result<Health, ClientError> {
		self.call("check_link", json!({ "link": link })).await
	}

	/// Looks for nearby shares.
	pub async fn discover(&self, timeout_secs: u64) -> Result<Vec<Peer>, ClientError> {
		self.call("discover", json!({ "timeout_secs": timeout_secs }))
			.await
	}

	/// The history.
	pub async fn history(&self) -> Result<Vec<HistoryEntry>, ClientError> {
		self.call("history.get", Value::Null).await
	}

	/// Asks the daemon to exit.
	pub async fn shutdown(&self) -> Result<(), ClientError> {
		self.call::<Value>("shutdown", Value::Null)
			.await
			.map(|_| ())
	}
}
