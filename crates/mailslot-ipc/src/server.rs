// The JSON-RPC server that drives an engine.
//
// (c) Copyright 2026 Liminal HQ, Scott Morris
// SPDX-License-Identifier: Apache-2.0 OR MIT

use std::path::Path;
use std::time::Duration;

use mailslot_core::config::{Config, Section};
use mailslot_core::proto::MailslotLink;
use mailslot_core::{Engine, SendRequest};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;
use tokio_stream::StreamExt;
use tokio_util::sync::CancellationToken;

use crate::protocol::{DiscoverParams, PROTOCOL_VERSION, RpcError, SendParams, codes};

/// Longest accepted request line.
const MAX_LINE: usize = 4 * 1024 * 1024;

/// Serves the JSON-RPC API for an engine.
#[derive(Clone)]
pub struct Daemon {
	engine: Engine,
	shutdown: CancellationToken,
}

#[cfg(unix)]
mod unix {
	use super::*;
	use std::os::unix::fs::PermissionsExt;
	use tokio::net::{UnixListener, UnixStream};

	/// Binds the socket at `path`: creates its folder with mode `0700`, removes a stale
	/// socket file left by a crashed daemon, and sets the socket to `0600`. Fails with
	/// `AddrInUse` if another daemon is listening.
	pub async fn bind(path: &Path) -> std::io::Result<UnixListener> {
		if let Some(dir) = path.parent() {
			let mut b = tokio::fs::DirBuilder::new();
			b.recursive(true);
			b.mode(0o700);
			b.create(dir).await?;
			tokio::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).await?;
		}
		if tokio::fs::symlink_metadata(path).await.is_ok() {
			if UnixStream::connect(path).await.is_ok() {
				return Err(std::io::Error::new(
					std::io::ErrorKind::AddrInUse,
					"another mailslotd is already listening",
				));
			}
			tokio::fs::remove_file(path).await?;
		}
		let listener = UnixListener::bind(path)?;
		tokio::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).await?;
		Ok(listener)
	}

	impl Daemon {
		/// Accepts connections until [`Daemon::shutdown_token`] is cancelled (by the
		/// `shutdown` method or by the caller).
		pub async fn serve(&self, listener: UnixListener) {
			loop {
				tokio::select! {
					_ = self.shutdown.cancelled() => break,
					r = listener.accept() => match r {
						Ok((stream, _)) => {
							let this = self.clone();
							tokio::spawn(async move { this.connection(stream).await });
						}
						Err(e) => {
							tracing::warn!("accept failed: {e}");
							tokio::time::sleep(Duration::from_millis(50)).await;
						}
					},
				}
			}
		}

		async fn connection(&self, stream: UnixStream) {
			let (read, mut write) = stream.into_split();
			let (out_tx, mut out_rx) = mpsc::unbounded_channel::<String>();
			let writer = tokio::spawn(async move {
				while let Some(line) = out_rx.recv().await {
					if write.write_all(line.as_bytes()).await.is_err()
						|| write.write_all(b"\n").await.is_err()
					{
						break;
					}
				}
			});
			let mut subscription: Option<tokio::task::JoinHandle<()>> = None;
			let mut lines = BufReader::new(read).lines();
			loop {
				let line = tokio::select! {
					_ = self.shutdown.cancelled() => break,
					l = lines.next_line() => match l {
						Ok(Some(l)) => l,
						_ => break,
					},
				};
				if line.len() > MAX_LINE {
					let _ = out_tx.send(error_response(
						Value::Null,
						RpcError::new(codes::INVALID_REQUEST, "request too large"),
					));
					break;
				}
				if line.trim().is_empty() {
					continue;
				}
				if let Some(reply) = self.handle_line(&line, &out_tx, &mut subscription).await {
					let _ = out_tx.send(reply);
				}
			}
			if let Some(s) = subscription {
				s.abort();
			}
			drop(out_tx);
			let _ = tokio::time::timeout(Duration::from_secs(1), writer).await;
		}
	}
}

#[cfg(unix)]
pub use unix::bind;

impl Daemon {
	/// Wraps an engine.
	pub fn new(engine: Engine) -> Self {
		Self {
			engine,
			shutdown: CancellationToken::new(),
		}
	}

	/// Cancel this to stop serving; the `shutdown` method cancels it too.
	pub fn shutdown_token(&self) -> CancellationToken {
		self.shutdown.clone()
	}

	/// The engine being served.
	pub fn engine(&self) -> &Engine {
		&self.engine
	}

	/// Parses one request line and returns the response line, if one is due
	/// (notifications get none).
	async fn handle_line(
		&self,
		line: &str,
		out: &mpsc::UnboundedSender<String>,
		subscription: &mut Option<tokio::task::JoinHandle<()>>,
	) -> Option<String> {
		let value: Value = match serde_json::from_str(line) {
			Ok(v) => v,
			Err(e) => {
				return Some(error_response(
					Value::Null,
					RpcError::new(codes::PARSE_ERROR, format!("parse error: {e}")),
				));
			}
		};
		let Some(obj) = value.as_object() else {
			return Some(error_response(
				Value::Null,
				RpcError::new(
					codes::INVALID_REQUEST,
					"batches and non-objects are not supported",
				),
			));
		};
		let id = obj.get("id").cloned();
		let valid_id = id
			.as_ref()
			.is_none_or(|i| i.is_string() || i.is_number() || i.is_null());
		let method = obj.get("method").and_then(Value::as_str);
		if obj.get("jsonrpc").and_then(Value::as_str) != Some("2.0")
			|| method.is_none()
			|| !valid_id
		{
			return Some(error_response(
				id.unwrap_or(Value::Null),
				RpcError::new(codes::INVALID_REQUEST, "not a JSON-RPC 2.0 request"),
			));
		}
		let method = method.unwrap_or_default();
		let params = obj.get("params").cloned().unwrap_or(Value::Null);
		let result = self.call(method, params, out, subscription).await;
		let id = id?; // A request without an id is a notification: run it, reply to nothing.
		Some(match result {
			Ok(v) => json!({ "jsonrpc": "2.0", "id": id, "result": v }).to_string(),
			Err(e) => error_response(id, e),
		})
	}

	async fn call(
		&self,
		method: &str,
		params: Value,
		out: &mpsc::UnboundedSender<String>,
		subscription: &mut Option<tokio::task::JoinHandle<()>>,
	) -> Result<Value, RpcError> {
		let e = &self.engine;
		match method {
			"ping" => Ok(json!("pong")),
			"version" => {
				Ok(json!({ "daemon": env!("CARGO_PKG_VERSION"), "protocol": PROTOCOL_VERSION }))
			}
			"state" => to_value(e.state()),
			"start" => to_value(e.start().await.map_err(engine_error)?),
			"stop" => unit(e.stop().await),
			"pause" => unit(e.pause().await),
			"resume" => unit(e.resume().await),
			"rotate_token" => {
				e.rotate_token().await.map_err(engine_error)?;
				to_value(e.state())
			}
			"settings.get" => to_value(e.config()),
			"settings.update" => {
				let cfg: Config = parse(params)?;
				unit(e.update_settings(cfg).await)
			}
			"settings.reset" => {
				#[derive(serde::Deserialize, Default)]
				#[serde(default)]
				struct P {
					section: Option<String>,
				}
				let p: P = parse_or_default(params)?;
				let mut cfg = e.config();
				match p.section.as_deref() {
					None => cfg.reset(),
					Some(name) => cfg.reset_section(section(name)?),
				}
				e.update_settings(cfg).await.map_err(engine_error)?;
				to_value(e.config())
			}
			"internet.set" => {
				#[derive(serde::Deserialize)]
				struct P {
					enabled: bool,
				}
				let p: P = parse(params)?;
				unit(e.set_internet_exposed(p.enabled).await)
			}
			"send" => {
				let p: SendParams = parse(params)?;
				let link = MailslotLink::parse(&p.link)
					.map_err(|e| RpcError::new(codes::INVALID_PARAMS, e.to_string()))?;
				let ids = e
					.send(SendRequest {
						link,
						paths: p.paths,
						options: p.options.into_client_options(),
					})
					.await
					.map_err(engine_error)?;
				Ok(json!({ "ids": ids }))
			}
			"send.cancel" => {
				#[derive(serde::Deserialize)]
				struct P {
					id: u64,
				}
				let p: P = parse(params)?;
				e.cancel_send(p.id);
				Ok(Value::Null)
			}
			"check_link" => {
				#[derive(serde::Deserialize)]
				struct P {
					link: String,
				}
				let p: P = parse(params)?;
				let link = MailslotLink::parse(&p.link)
					.map_err(|e| RpcError::new(codes::INVALID_PARAMS, e.to_string()))?;
				let health = e.check_link(link).await.map_err(engine_error)?;
				Ok(
					json!({ "ok": health.ok, "accepting": health.accepting, "max_bytes": health.max_bytes, "expires_at": health.expires_at, "capabilities": health.capabilities }),
				)
			}
			"discover" => {
				let p: DiscoverParams = parse_or_default(params)?;
				to_value(e.discover_peers(p.timeout()).await.map_err(engine_error)?)
			}
			"history.get" => to_value(e.load_history().await.map_err(engine_error)?),
			"history.clear" => unit(e.clear_history().await),
			"subscribe" => {
				if subscription.is_none() {
					let mut events = e.subscribe();
					let out = out.clone();
					*subscription = Some(tokio::spawn(async move {
						while let Some(ev) = events.next().await {
							let line = json!({ "jsonrpc": "2.0", "method": "event", "params": ev })
								.to_string();
							if out.send(line).is_err() {
								break;
							}
						}
					}));
				}
				Ok(json!(true))
			}
			"unsubscribe" => {
				if let Some(s) = subscription.take() {
					s.abort();
				}
				Ok(json!(true))
			}
			"shutdown" => {
				let token = self.shutdown.clone();
				// Reply first; the connection loop sees the cancellation afterwards.
				tokio::spawn(async move {
					tokio::time::sleep(Duration::from_millis(100)).await;
					token.cancel();
				});
				Ok(Value::Null)
			}
			other => Err(RpcError::new(
				codes::METHOD_NOT_FOUND,
				format!("unknown method `{other}`"),
			)),
		}
	}
}

fn section(name: &str) -> Result<Section, RpcError> {
	Ok(match name {
		"sharing" => Section::Sharing,
		"storage" => Section::Storage,
		"limits" => Section::Limits,
		"security" => Section::Security,
		"network" => Section::Network,
		"notifications" => Section::Notifications,
		other => {
			return Err(RpcError::new(
				codes::INVALID_PARAMS,
				format!("unknown section `{other}`"),
			));
		}
	})
}

fn parse<T: DeserializeOwned>(params: Value) -> Result<T, RpcError> {
	serde_json::from_value(params)
		.map_err(|e| RpcError::new(codes::INVALID_PARAMS, format!("invalid params: {e}")))
}

fn parse_or_default<T: DeserializeOwned + Default>(params: Value) -> Result<T, RpcError> {
	if params.is_null() {
		Ok(T::default())
	} else {
		parse(params)
	}
}

fn to_value<T: serde::Serialize>(v: T) -> Result<Value, RpcError> {
	serde_json::to_value(v).map_err(|e| RpcError::new(codes::ENGINE_ERROR, e.to_string()))
}

fn unit(r: mailslot_core::Result<()>) -> Result<Value, RpcError> {
	r.map(|()| Value::Null).map_err(engine_error)
}

fn engine_error(e: mailslot_core::Error) -> RpcError {
	RpcError::new(codes::ENGINE_ERROR, e.to_string())
}

fn error_response(id: Value, e: RpcError) -> String {
	json!({ "jsonrpc": "2.0", "id": id, "error": e }).to_string()
}
