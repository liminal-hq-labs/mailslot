// The HTTP server: accept loop, shared state and lifecycle.
//
// (c) Copyright 2026 Liminal HQ, Scott Morris
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The upload server.
//!
//! [`Server::start`] binds a listener and serves the routes described in the protocol
//! reference. Most callers use [`crate::Engine`] instead, which wraps this module with
//! configuration, events and port mapping.

mod handler;
mod support;
mod upload;

use std::collections::HashSet;
use std::convert::Infallible;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, SystemTime};

use hyper::body::Incoming;
use hyper_util::rt::{TokioExecutor, TokioIo, TokioTimer};
use hyper_util::server::conn::auto;
use tokio::net::TcpListener;
use tokio::sync::{Semaphore, mpsc};
use tokio_util::sync::CancellationToken;

pub use support::{FixedFreeSpace, FreeSpace, HashIndex, Lockout, RateLimiter, SystemFreeSpace};

use crate::config::{Collision, Config, SubfolderScheme};
use crate::error::{Error, Result};
use crate::events::UploadEvent;
use crate::token::Token;

const MIB: u64 = 1024 * 1024;

/// Settings that can change while the server runs.
#[derive(Clone, Debug)]
pub struct ServerSettings {
	/// Largest accepted file in bytes.
	pub max_file_bytes: u64,
	/// Total bytes accepted per session; `0` for no quota.
	pub quota_bytes: u64,
	/// Free-space floor in bytes.
	pub min_free_bytes: u64,
	/// Uploads handled at once.
	pub max_concurrent: u32,
	/// Requests per minute per address; `0` disables.
	pub rate_limit_per_min: u32,
	/// How long a body may stall.
	pub idle_timeout: Duration,
	/// When the link expires.
	pub expires_at: Option<SystemTime>,
	/// Stop after this many files; `0` for no limit.
	pub stop_after_files: u32,
	/// Allowed extensions; empty allows all.
	pub allowed_extensions: Vec<String>,
	/// Name collision behaviour.
	pub collision: Collision,
	/// Subfolder scheme.
	pub subfolder: SubfolderScheme,
	/// Write `.sha256` sidecars.
	pub sidecars: bool,
	/// Re-hash from disk after writing.
	pub verify_on_receive: bool,
	/// Bad-token requests before lockout; `0` disables.
	pub lockout_threshold: u32,
	/// How long a lockout lasts.
	pub lockout_duration: Duration,
}

impl Default for ServerSettings {
	fn default() -> Self {
		Self::from_config(&Config::default(), None)
	}
}

impl ServerSettings {
	/// Derives settings from a [`Config`]; `expires_at` is computed by the caller.
	pub fn from_config(c: &Config, expires_at: Option<SystemTime>) -> Self {
		Self {
			max_file_bytes: c.limits.max_file_mb.saturating_mul(MIB),
			quota_bytes: c.limits.session_quota_mb.saturating_mul(MIB),
			min_free_bytes: c.limits.min_free_mb.saturating_mul(MIB),
			max_concurrent: c.limits.max_concurrent,
			rate_limit_per_min: c.limits.rate_limit_per_min,
			idle_timeout: Duration::from_secs(c.limits.idle_timeout_secs.max(1)),
			expires_at,
			stop_after_files: c.sharing.stop_after_files,
			allowed_extensions: c
				.security
				.allowed_extensions
				.iter()
				.map(|e| e.trim_start_matches('.').to_ascii_lowercase())
				.collect(),
			collision: c.storage.collision,
			subfolder: c.storage.subfolder,
			sidecars: c.storage.sidecars,
			verify_on_receive: c.storage.verify_on_receive,
			lockout_threshold: c.security.bad_token_lockout,
			lockout_duration: Duration::from_secs(600),
		}
	}
}

/// Everything needed to start a server.
pub struct ServerConfig {
	/// Address to bind; port `0` picks a free one.
	pub bind: SocketAddr,
	/// Destination folder (created if missing).
	pub dest_dir: PathBuf,
	/// The share token.
	pub token: Token,
	/// Live-adjustable settings.
	pub settings: ServerSettings,
	/// TLS material; `None` serves plain HTTP.
	#[cfg(feature = "tls")]
	pub tls: Option<crate::tls::TlsMaterial>,
	/// Free-space probe.
	pub free_space: Arc<dyn FreeSpace>,
	/// Index of received hashes (shared with the caller so it can be pre-seeded).
	pub hashes: Arc<HashIndex>,
}

impl ServerConfig {
	/// A plain-HTTP configuration with the system free-space probe.
	pub fn new(bind: SocketAddr, dest_dir: impl Into<PathBuf>, token: Token) -> Self {
		Self {
			bind,
			dest_dir: dest_dir.into(),
			token,
			settings: ServerSettings::default(),
			#[cfg(feature = "tls")]
			tls: None,
			free_space: Arc::new(SystemFreeSpace),
			hashes: Arc::new(HashIndex::default()),
		}
	}
}

/// Events the server emits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ServerEvent {
	/// A received-file event.
	Upload(UploadEvent),
	/// The configured file count was reached; the owner should stop sharing.
	FileLimitReached,
}

pub(crate) struct Shared {
	token: RwLock<(Token, CancellationToken)>,
	settings: RwLock<Arc<ServerSettings>>,
	events: mpsc::UnboundedSender<ServerEvent>,
	dest: PathBuf,
	next_id: AtomicU64,
	active: AtomicU32,
	received_bytes: AtomicU64,
	files_reserved: AtomicU32,
	files_done: AtomicU32,
	in_flight_ids: Mutex<HashSet<String>>,
	lockout: Lockout,
	rate: RateLimiter,
	free_space: Arc<dyn FreeSpace>,
	hashes: Arc<HashIndex>,
	accepting: AtomicBool,
	shutdown: CancellationToken,
}

impl Shared {
	pub(crate) fn settings(&self) -> Arc<ServerSettings> {
		self.settings.read().map(|s| s.clone()).unwrap_or_default()
	}

	pub(crate) fn token_matches(&self, candidate: &str) -> bool {
		self.token
			.read()
			.map(|t| t.0.matches(candidate))
			.unwrap_or(false)
	}

	pub(crate) fn token_cancel(&self) -> CancellationToken {
		self.token.read().map(|t| t.1.clone()).unwrap_or_default()
	}

	pub(crate) fn emit(&self, ev: UploadEvent) {
		let _ = self.events.send(ServerEvent::Upload(ev));
	}
}

/// A running server.
pub struct ServerHandle {
	addr: SocketAddr,
	shared: Arc<Shared>,
	task: tokio::task::JoinHandle<()>,
	conns: Arc<Semaphore>,
}

impl std::fmt::Debug for ServerHandle {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		f.debug_struct("ServerHandle")
			.field("addr", &self.addr)
			.finish_non_exhaustive()
	}
}

impl ServerHandle {
	/// The bound address (with the real port when `0` was requested).
	pub fn local_addr(&self) -> SocketAddr {
		self.addr
	}

	/// Replaces the live settings. Uploads in progress keep the old limits.
	pub fn update_settings(&self, settings: ServerSettings) {
		if let Ok(mut s) = self.shared.settings.write() {
			*s = Arc::new(settings);
		}
	}

	/// Swaps the token. The old token stops working at once and uploads in flight
	/// under it are aborted.
	pub fn rotate_token(&self, token: Token) {
		if let Ok(mut t) = self.shared.token.write() {
			t.1.cancel();
			*t = (token, CancellationToken::new());
		}
	}

	/// Pauses or resumes accepting uploads; a paused server answers `503`.
	pub fn set_accepting(&self, accepting: bool) {
		self.shared.accepting.store(accepting, Ordering::SeqCst);
	}

	/// Bytes accepted so far this session.
	pub fn received_bytes(&self) -> u64 {
		self.shared.received_bytes.load(Ordering::SeqCst)
	}

	/// Stops listening, aborts uploads in flight and waits for the accept loop to end.
	pub async fn shutdown(self) {
		self.shared.shutdown.cancel();
		let _ = self.task.await;
		// Connections drain as their tasks see the shutdown signal.
		let _ = tokio::time::timeout(
			Duration::from_secs(2),
			self.conns.acquire_many(MAX_CONNECTIONS as u32),
		)
		.await;
	}
}

const MAX_CONNECTIONS: usize = 512;
const HEADER_READ_TIMEOUT: Duration = Duration::from_secs(10);
const PARTIAL_TTL: Duration = Duration::from_secs(3600);

/// Entry point for the server.
pub struct Server;

impl Server {
	/// Binds and starts serving. Events go to `events`.
	pub async fn start(
		cfg: ServerConfig,
		events: mpsc::UnboundedSender<ServerEvent>,
	) -> Result<ServerHandle> {
		tokio::fs::create_dir_all(&cfg.dest_dir)
			.await
			.map_err(|_| Error::DestNotWritable(cfg.dest_dir.clone()))?;
		check_writable(&cfg.dest_dir).await?;
		let listener = TcpListener::bind(cfg.bind)
			.await
			.map_err(Error::from_bind)?;
		let addr = listener.local_addr()?;
		let shutdown = CancellationToken::new();
		let shared = Arc::new(Shared {
			token: RwLock::new((cfg.token, CancellationToken::new())),
			settings: RwLock::new(Arc::new(cfg.settings)),
			events,
			dest: cfg.dest_dir.clone(),
			next_id: AtomicU64::new(1),
			active: AtomicU32::new(0),
			received_bytes: AtomicU64::new(0),
			files_reserved: AtomicU32::new(0),
			files_done: AtomicU32::new(0),
			in_flight_ids: Mutex::new(HashSet::new()),
			lockout: Lockout::default(),
			rate: RateLimiter::default(),
			free_space: cfg.free_space,
			hashes: cfg.hashes,
			accepting: AtomicBool::new(true),
			shutdown: shutdown.clone(),
		});
		#[cfg(feature = "tls")]
		let acceptor = match &cfg.tls {
			Some(m) => Some(tokio_rustls::TlsAcceptor::from(crate::tls::server_config(
				m,
			)?)),
			None => None,
		};
		tokio::spawn(sweep_stale_parts(cfg.dest_dir.clone()));
		let conns = Arc::new(Semaphore::new(MAX_CONNECTIONS));
		let task = tokio::spawn(accept_loop(
			listener,
			shared.clone(),
			conns.clone(),
			#[cfg(feature = "tls")]
			acceptor,
		));
		Ok(ServerHandle {
			addr,
			shared,
			task,
			conns,
		})
	}
}

async fn check_writable(dir: &Path) -> Result<()> {
	let probe = dir.join(format!(".mailslot-probe-{}", std::process::id()));
	tokio::fs::write(&probe, b"")
		.await
		.map_err(|_| Error::DestNotWritable(dir.to_path_buf()))?;
	let _ = tokio::fs::remove_file(&probe).await;
	Ok(())
}

/// Removes abandoned partial files older than an hour.
async fn sweep_stale_parts(dir: PathBuf) {
	let Ok(mut rd) = tokio::fs::read_dir(&dir).await else {
		return;
	};
	while let Ok(Some(entry)) = rd.next_entry().await {
		let name = entry.file_name();
		let name = name.to_string_lossy();
		if !(name.starts_with(".mailslot-") && name.ends_with(".part")) {
			continue;
		}
		let Ok(meta) = entry.metadata().await else {
			continue;
		};
		let age = meta
			.modified()
			.ok()
			.and_then(|m| m.elapsed().ok())
			.unwrap_or_default();
		if age > PARTIAL_TTL {
			let _ = tokio::fs::remove_file(entry.path()).await;
		}
	}
}

async fn accept_loop(
	listener: TcpListener,
	shared: Arc<Shared>,
	conns: Arc<Semaphore>,
	#[cfg(feature = "tls")] acceptor: Option<tokio_rustls::TlsAcceptor>,
) {
	loop {
		let (stream, peer) = tokio::select! {
			_ = shared.shutdown.cancelled() => break,
			r = listener.accept() => match r {
				Ok(v) => v,
				Err(_) => {
					tokio::time::sleep(Duration::from_millis(50)).await;
					continue;
				}
			},
		};
		if shared.lockout.is_locked(peer.ip()) {
			drop(stream);
			continue;
		}
		let Ok(permit) = conns.clone().try_acquire_owned() else {
			drop(stream);
			continue;
		};
		let shared = shared.clone();
		#[cfg(feature = "tls")]
		let acceptor = acceptor.clone();
		tokio::spawn(async move {
			let _permit = permit;
			#[cfg(feature = "tls")]
			if let Some(acc) = acceptor {
				if let Ok(Ok(tls)) =
					tokio::time::timeout(HEADER_READ_TIMEOUT, acc.accept(stream)).await
				{
					serve_connection(tls, peer, shared).await
				}
				return;
			}
			serve_connection(stream, peer, shared).await;
		});
	}
}

async fn serve_connection<S>(io: S, peer: SocketAddr, shared: Arc<Shared>)
where
	S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
	let shutdown = shared.shutdown.clone();
	let svc = hyper::service::service_fn(move |req: hyper::Request<Incoming>| {
		let shared = shared.clone();
		async move {
			Ok::<_, Infallible>(handler::handle(shared, peer, req.map(axum::body::Body::new)).await)
		}
	});
	let mut builder = auto::Builder::new(TokioExecutor::new());
	builder
		.http1()
		.timer(TokioTimer::new())
		.header_read_timeout(HEADER_READ_TIMEOUT)
		.keep_alive(true);
	let conn = builder.serve_connection(TokioIo::new(io), svc);
	tokio::select! {
		_ = conn => {},
		_ = shutdown.cancelled() => {},
	}
}
