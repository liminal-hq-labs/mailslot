// The engine: one object a frontend drives with commands and observes through events.
//
// (c) Copyright 2026 Liminal HQ, Scott Morris
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The [`Engine`] ties configuration, the server, port mapping, history and the
//! client together behind a small command set and a typed event stream.
//!
//! The engine owns no global state. It runs on a [`tokio::runtime::Handle`] you give
//! it (or the current one), or on a private runtime if you ask for one. Every command is
//! an `async fn` that executes on that runtime, so you can await it from any executor,
//! including the GLib main loop.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
#[cfg(feature = "client")]
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::atomic::AtomicU64;
#[cfg(feature = "client")]
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::{Duration, SystemTime};

use futures_util::Stream;
use tokio::runtime::{Handle, Runtime};
use tokio::sync::{broadcast, mpsc};
use tokio_stream::StreamExt as _;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::wrappers::errors::BroadcastStreamRecvError;
use tokio_util::sync::CancellationToken;

use crate::config::{Config, Paths};
use crate::error::{Error, Result};
use crate::events::*;
use crate::history::{Direction, History, HistoryEntry};
use crate::netinfo;
use crate::proto::MailslotLink;
use crate::server::{
	FreeSpace, HashIndex, Server, ServerConfig, ServerEvent, ServerHandle, ServerSettings,
	SystemFreeSpace,
};
use crate::snippets::SnippetContext;
use crate::token::Token;

const EVENT_CAPACITY: usize = 2048;

/// A stream of engine events. Dropping it unsubscribes.
pub struct EventStream(Pin<Box<dyn Stream<Item = Event> + Send>>);

impl Stream for EventStream {
	type Item = Event;
	fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Event>> {
		self.0.as_mut().poll_next(cx)
	}
}

/// What to send and where.
#[cfg(feature = "client")]
#[derive(Clone, Debug)]
pub struct SendRequest {
	/// The destination share.
	pub link: MailslotLink,
	/// Files and folders; folders are expanded to the files beneath them.
	pub paths: Vec<PathBuf>,
	/// Client behaviour.
	pub options: crate::client::ClientOptions,
}

struct Running {
	server: ServerHandle,
	cancel: CancellationToken,
	portmap: Option<(CancellationToken, tokio::task::JoinHandle<()>)>,
	#[cfg(feature = "discovery")]
	advertiser: Option<crate::discovery::Advertiser>,
}

struct Inner {
	handle: Handle,
	runtime: Mutex<Option<Runtime>>,
	paths: Paths,
	persist: bool,
	config: Mutex<Config>,
	token: Mutex<Token>,
	state: Mutex<EngineState>,
	events: broadcast::Sender<Event>,
	running: tokio::sync::Mutex<Option<Running>>,
	history: History,
	hashes: Arc<HashIndex>,
	free_space: Arc<dyn FreeSpace>,
	#[cfg_attr(not(feature = "client"), allow(dead_code))]
	next_send: AtomicU64,
	sends: Mutex<HashMap<SendId, tokio::task::AbortHandle>>,
	#[cfg(feature = "portmap")]
	portmap_overrides: Option<crate::portmap::PortMapOptions>,
}

impl Drop for Inner {
	fn drop(&mut self) {
		if let Some(rt) = self.runtime.lock().ok().and_then(|mut r| r.take()) {
			rt.shutdown_background();
		}
	}
}

/// Builds an [`Engine`].
pub struct EngineBuilder {
	paths: Paths,
	config: Option<Config>,
	token: Option<Token>,
	handle: Option<Handle>,
	own_runtime: bool,
	persist: bool,
	free_space: Arc<dyn FreeSpace>,
	#[cfg(feature = "portmap")]
	portmap_overrides: Option<crate::portmap::PortMapOptions>,
}

impl Default for EngineBuilder {
	fn default() -> Self {
		Self::new(Paths::xdg())
	}
}

impl EngineBuilder {
	/// Starts a builder that stores settings and data under `paths`.
	pub fn new(paths: Paths) -> Self {
		Self {
			paths,
			config: None,
			token: None,
			handle: None,
			own_runtime: false,
			persist: true,
			free_space: Arc::new(SystemFreeSpace),
			#[cfg(feature = "portmap")]
			portmap_overrides: None,
		}
	}

	/// Uses this configuration instead of reading `config.toml`.
	pub fn config(mut self, config: Config) -> Self {
		self.config = Some(config);
		self
	}

	/// Uses this token instead of the persisted one.
	pub fn token(mut self, token: Token) -> Self {
		self.token = Some(token);
		self
	}

	/// Runs on this runtime.
	pub fn runtime_handle(mut self, handle: Handle) -> Self {
		self.handle = Some(handle);
		self
	}

	/// Builds a private two-thread runtime when no handle is given and none is current.
	pub fn own_runtime(mut self, yes: bool) -> Self {
		self.own_runtime = yes;
		self
	}

	/// Whether settings and the token are written to disk (default `true`).
	pub fn persist(mut self, yes: bool) -> Self {
		self.persist = yes;
		self
	}

	/// Replaces the free-space probe (useful in tests).
	pub fn free_space(mut self, fs: Arc<dyn FreeSpace>) -> Self {
		self.free_space = fs;
		self
	}

	/// Overrides port-mapping options (SSDP address and so on), for tests.
	#[cfg(feature = "portmap")]
	pub fn portmap_options(mut self, opts: crate::portmap::PortMapOptions) -> Self {
		self.portmap_overrides = Some(opts);
		self
	}

	/// Builds the engine.
	pub async fn build(self) -> Result<Engine> {
		let (handle, runtime) =
			match self.handle {
				Some(h) => (h, None),
				None => match Handle::try_current() {
					Ok(h) => (h, None),
					Err(_) if self.own_runtime => {
						let rt = tokio::runtime::Builder::new_multi_thread()
							.worker_threads(2)
							.enable_all()
							.thread_name("mailslot")
							.build()?;
						(rt.handle().clone(), Some(rt))
					}
					Err(_) => return Err(Error::State(
						"no tokio runtime is available; pass a handle or call own_runtime(true)"
							.into(),
					)),
				},
			};
		let paths = self.paths.clone();
		let persist = self.persist;
		let (config, token) = {
			let cfg_in = self.config;
			let tok_in = self.token;
			handle
				.spawn(async move {
					let config = match cfg_in {
						Some(c) => c,
						None => Config::load(&paths).await?,
					};
					let token = match tok_in {
						Some(t) => t,
						None if persist => crate::config::load_or_create_token(&paths).await?,
						None => Token::generate()?,
					};
					Ok::<_, Error>((config, token))
				})
				.await
				.map_err(|e| Error::State(e.to_string()))??
		};
		config.validate()?;
		let (tx, _) = broadcast::channel(EVENT_CAPACITY);
		let history = History::new(self.paths.history_file());
		Ok(Engine {
			inner: Arc::new(Inner {
				handle,
				runtime: Mutex::new(runtime),
				paths: self.paths,
				persist,
				config: Mutex::new(config),
				token: Mutex::new(token),
				state: Mutex::new(EngineState::default()),
				events: tx,
				running: tokio::sync::Mutex::new(None),
				history,
				hashes: Arc::new(HashIndex::default()),
				free_space: self.free_space,
				next_send: AtomicU64::new(1),
				sends: Mutex::new(HashMap::new()),
				#[cfg(feature = "portmap")]
				portmap_overrides: self.portmap_overrides,
			}),
		})
	}
}

/// The Mailslot engine. Cheap to clone; clones share state.
#[derive(Clone)]
pub struct Engine {
	inner: Arc<Inner>,
}

impl std::fmt::Debug for Engine {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		f.debug_struct("Engine")
			.field("status", &self.state().status)
			.finish_non_exhaustive()
	}
}

impl Inner {
	fn emit(&self, e: Event) {
		let _ = self.events.send(e);
	}

	fn state(&self) -> EngineState {
		self.state.lock().map(|s| s.clone()).unwrap_or_default()
	}

	fn mutate(&self, f: impl FnOnce(&mut EngineState)) {
		if let Ok(mut s) = self.state.lock() {
			f(&mut s);
		}
	}

	fn emit_state(&self) {
		self.emit(Event::StateChanged(Box::new(self.state())));
	}

	fn config(&self) -> Config {
		self.config.lock().map(|c| c.clone()).unwrap_or_default()
	}

	fn token(&self) -> Token {
		self.token.lock().expect("token lock").clone()
	}
}

fn bind_addr(c: &Config) -> Result<SocketAddr> {
	let ip: IpAddr =
		c.sharing.bind.parse().map_err(|_| {
			Error::InvalidConfig(format!("`{}` is not an IP address", c.sharing.bind))
		})?;
	Ok(SocketAddr::new(ip, c.sharing.port))
}

async fn build_urls(
	inner: &Inner,
	addr: SocketAddr,
	token: &Token,
	tls_pin: &Option<String>,
	cfg: &Config,
) -> Vec<ShareUrl> {
	let mut hosts: Vec<(String, IpAddr)> = Vec::new();
	if addr.ip().is_unspecified() {
		for a in netinfo::lan_addrs().await {
			hosts.push((a.interface, a.ip));
		}
	} else if !addr.ip().is_loopback() {
		hosts.push(("Selected address".into(), addr.ip()));
	}
	let pref = cfg.network.interface.trim().to_owned();
	if !pref.is_empty() {
		hosts.sort_by_key(|(name, ip)| !(name == &pref || ip.to_string() == pref));
	}
	hosts.push(("This computer".into(), IpAddr::V4(Ipv4Addr::LOCALHOST)));
	let _ = inner;
	hosts
		.into_iter()
		.map(|(label, ip)| {
			let host = ip.to_string();
			let mut link = MailslotLink::new(host.clone(), addr.port(), token.clone())
				.with_label(cfg.sharing.link_label.trim());
			if cfg.security.https {
				link = link.with_tls(tls_pin.clone());
			}
			ShareUrl {
				label,
				host,
				url: link.drop_url(),
				link: link.to_uri(),
			}
		})
		.collect()
}

impl Engine {
	/// Starts a builder with XDG paths.
	pub fn builder() -> EngineBuilder {
		EngineBuilder::default()
	}

	/// Runs a future on the engine's runtime and awaits its result from any executor.
	async fn run<T, F>(&self, fut: F) -> Result<T>
	where
		F: std::future::Future<Output = Result<T>> + Send + 'static,
		T: Send + 'static,
	{
		self.inner
			.handle
			.spawn(fut)
			.await
			.map_err(|e| Error::State(format!("engine task failed: {e}")))?
	}

	/// Subscribes to events. Receivers that fall behind get [`Event::Lagged`].
	pub fn subscribe(&self) -> EventStream {
		let rx = self.inner.events.subscribe();
		EventStream(Box::pin(BroadcastStream::new(rx).map(|r| match r {
			Ok(e) => e,
			Err(BroadcastStreamRecvError::Lagged(n)) => Event::Lagged(n),
		})))
	}

	/// A snapshot of the current state.
	pub fn state(&self) -> EngineState {
		self.inner.state()
	}

	/// A copy of the current settings.
	pub fn config(&self) -> Config {
		self.inner.config()
	}

	/// The current token. Treat it as a secret.
	pub fn token(&self) -> Token {
		self.inner.token()
	}

	/// The runtime handle the engine uses.
	pub fn runtime_handle(&self) -> Handle {
		self.inner.handle.clone()
	}

	/// The paths in use.
	pub fn paths(&self) -> &Paths {
		&self.inner.paths
	}

	/// The history file.
	pub fn history(&self) -> History {
		self.inner.history.clone()
	}

	/// Loads the history, oldest first. Runs on the engine's runtime, so it can be awaited
	/// from any executor.
	pub async fn load_history(&self) -> Result<Vec<HistoryEntry>> {
		let history = self.inner.history.clone();
		self.run(async move { history.load().await }).await
	}

	/// Deletes the history file.
	pub async fn clear_history(&self) -> Result<()> {
		let history = self.inner.history.clone();
		self.run(async move { history.clear().await }).await
	}

	/// Drops history entries older than the configured number of days.
	pub async fn prune_history(&self) -> Result<usize> {
		let history = self.inner.history.clone();
		let days = self.config().notifications.history_days;
		self.run(async move { history.prune(days).await }).await
	}

	/// Inputs for the copy-snippet menu, from the first URL in the current state.
	pub fn snippet_context(&self) -> Option<SnippetContext> {
		let st = self.state();
		let url = st.urls.first()?;
		let cfg = self.config();
		let expires = match st.expires_at {
			Some(t) => {
				let secs = t
					.duration_since(SystemTime::now())
					.map(|d| d.as_secs())
					.unwrap_or(0);
				format!("in about {} minutes", secs.div_ceil(60))
			}
			None => "never".into(),
		};
		Some(SnippetContext {
			drop_url: url.url.clone(),
			link: url.link.clone(),
			max_bytes: cfg.limits.max_file_mb * 1024 * 1024,
			expires,
			tls_pin: st.tls_pin.clone(),
		})
	}

	/// Starts sharing with the current settings.
	pub async fn start(&self) -> Result<EngineState> {
		let inner = self.inner.clone();
		self.run(async move { start_inner(inner).await }).await
	}

	/// Stops sharing and removes any router mapping.
	pub async fn stop(&self) -> Result<()> {
		let inner = self.inner.clone();
		self.run(async move {
			stop_inner(inner, StopReason::Requested).await;
			Ok(())
		})
		.await
	}

	/// Refuses new uploads with `503` (and reports `accepting: false` in `health`) until
	/// [`Engine::resume`]. Uploads already in progress finish.
	pub async fn pause(&self) -> Result<()> {
		self.set_paused(true).await
	}

	/// Accepts uploads again after [`Engine::pause`].
	pub async fn resume(&self) -> Result<()> {
		self.set_paused(false).await
	}

	async fn set_paused(&self, paused: bool) -> Result<()> {
		let inner = self.inner.clone();
		self.run(async move {
			let running = inner.running.lock().await;
			let Some(r) = running.as_ref() else {
				return Err(Error::State("not sharing".into()));
			};
			r.server.set_accepting(!paused);
			drop(running);
			inner.mutate(|s| s.paused = paused);
			inner.emit_state();
			Ok(())
		})
		.await
	}

	/// Browses the local network for other Mailslot shares (mDNS) for `timeout`.
	/// Announcements never contain the secret, so a found share still needs its link.
	#[cfg(feature = "discovery")]
	pub async fn discover_peers(&self, timeout: Duration) -> Result<Vec<crate::discovery::Peer>> {
		self.run(async move { crate::discovery::browse(timeout).await })
			.await
	}

	/// Replaces the token. The old link stops working immediately.
	pub async fn rotate_token(&self) -> Result<Token> {
		let inner = self.inner.clone();
		self.run(async move {
			let new = Token::generate()?;
			if inner.persist {
				crate::config::save_token(&inner.paths, &new).await?;
			}
			if let Ok(mut t) = inner.token.lock() {
				*t = new.clone();
			}
			let running = inner.running.lock().await;
			if let Some(r) = running.as_ref() {
				r.server.rotate_token(new.clone());
				let cfg = inner.config();
				let (addr, pin) = {
					let s = inner.state();
					(s.addr, s.tls_pin)
				};
				if let Some(addr) = addr {
					let urls = build_urls(&inner, addr, &new, &pin, &cfg).await;
					inner.mutate(|s| s.urls = urls);
				}
				refresh_internet_url(&inner, &new);
			}
			drop(running);
			inner.emit(Event::TokenRotated);
			inner.emit_state();
			Ok(new)
		})
		.await
	}

	/// Saves new settings. Limits and rules apply live; port, address, HTTPS and
	/// destination changes set `restart_required` while sharing.
	pub async fn update_settings(&self, new: Config) -> Result<()> {
		new.validate()?;
		let inner = self.inner.clone();
		self.run(async move {
			let old = inner.config();
			if inner.persist {
				new.save(&inner.paths).await?;
			}
			if let Ok(mut c) = inner.config.lock() {
				*c = new.clone();
			}
			let mut running = inner.running.lock().await;
			if let Some(r) = running.as_mut() {
				let expires = inner.state().expires_at;
				r.server
					.update_settings(ServerSettings::from_config(&new, expires));
				let restart = old.sharing.port != new.sharing.port
					|| old.sharing.bind != new.sharing.bind
					|| old.security.https != new.security.https
					|| old.storage.dest_dir != new.storage.dest_dir;
				if restart {
					inner.mutate(|s| s.restart_required = true);
				}
				let (addr, pin) = {
					let s = inner.state();
					(s.addr, s.tls_pin)
				};
				if let Some(addr) = addr {
					let urls = build_urls(&inner, addr, &inner.token(), &pin, &new).await;
					inner.mutate(|s| s.urls = urls);
				}
				#[cfg(feature = "discovery")]
				if old.network.mdns != new.network.mdns
					|| old.sharing.link_label != new.sharing.link_label
				{
					r.advertiser = None;
					r.advertiser = start_advertising(&inner, &new, r.server.local_addr().port());
				}
			}
			drop(running);
			inner.emit(Event::SettingsChanged);
			inner.emit_state();
			if old.network.upnp != new.network.upnp && inner.state().status == Status::Sharing {
				set_internet_inner(inner.clone(), new.network.upnp).await;
			}
			Ok(())
		})
		.await
	}

	/// Turns router exposure on or off while sharing, and remembers the choice.
	pub async fn set_internet_exposed(&self, on: bool) -> Result<()> {
		let mut cfg = self.config();
		cfg.network.upnp = on;
		let inner = self.inner.clone();
		self.run(async move {
			if inner.persist {
				cfg.save(&inner.paths).await?;
			}
			if let Ok(mut c) = inner.config.lock() {
				c.network.upnp = on;
			}
			if inner.state().status == Status::Sharing {
				set_internet_inner(inner.clone(), on).await;
			}
			inner.emit(Event::SettingsChanged);
			Ok(())
		})
		.await
	}

	/// Sends files to another share. Returns the transfer ids; progress arrives as
	/// [`Event::Send`].
	#[cfg(feature = "client")]
	pub async fn send(&self, req: SendRequest) -> Result<Vec<SendId>> {
		let inner = self.inner.clone();
		self.run(async move { send_inner(inner, req).await }).await
	}

	/// Asks a link's server for its limits and whether it is accepting files.
	#[cfg(feature = "client")]
	pub async fn check_link(&self, link: MailslotLink) -> Result<crate::client::Health> {
		self.run(async move {
			crate::client::Client::new(
				link,
				crate::client::ClientOptions {
					retries: 0,
					..Default::default()
				},
			)?
			.health()
			.await
		})
		.await
	}

	/// Cancels a running transfer.
	pub fn cancel_send(&self, id: SendId) {
		if let Some(h) = self.inner.sends.lock().ok().and_then(|mut m| m.remove(&id)) {
			h.abort();
			self.inner.mutate(|s| {
				s.apply_send(&SendEvent::Failed {
					id,
					reason: "cancelled".into(),
				});
			});
			self.inner.emit(Event::Send(SendEvent::Failed {
				id,
				reason: "cancelled".into(),
			}));
		}
	}

	/// Clears finished entries from the incoming and outgoing lists.
	pub fn clear_finished(&self) {
		self.inner.mutate(|s| {
			s.uploads.retain(|u| u.state == UploadState::Receiving);
			s.sends.retain(|t| !t.finished);
		});
		self.inner.emit_state();
	}

	/// Looks for a free port at or after `from` (for the "use the next port" suggestion).
	pub async fn find_free_port(&self, from: u16) -> Option<u16> {
		let bind = self.config().sharing.bind;
		let ip: IpAddr = bind.parse().ok()?;
		self.run(async move {
			for p in from..from.saturating_add(100) {
				if tokio::net::TcpListener::bind(SocketAddr::new(ip, p))
					.await
					.is_ok()
				{
					return Ok(Some(p));
				}
			}
			Ok(None)
		})
		.await
		.ok()
		.flatten()
	}

	/// Looks up the public address with one request, if the setting allows it.
	pub async fn lookup_public_ip(&self) -> Result<IpAddr> {
		if !self.config().network.public_ip_lookup {
			return Err(Error::State(
				"the public address lookup is turned off in Network settings".into(),
			));
		}
		self.run(async { netinfo::lookup_public_ip().await }).await
	}

	/// Stops everything. Call before dropping when the engine owns its runtime.
	pub async fn shutdown(&self) {
		let _ = self.stop().await;
		let ids: Vec<_> = self
			.inner
			.sends
			.lock()
			.map(|m| m.keys().copied().collect())
			.unwrap_or_default();
		for id in ids {
			self.cancel_send(id);
		}
	}
}

/// Starts the mDNS announcement when the preference is on. The secret is never included.
#[cfg(feature = "discovery")]
fn start_advertising(
	inner: &Inner,
	cfg: &Config,
	port: u16,
) -> Option<crate::discovery::Advertiser> {
	if !cfg.network.mdns {
		inner.mutate(|s| s.advertising = false);
		return None;
	}
	let label = cfg.sharing.link_label.trim();
	let result = crate::discovery::Advertiser::start(
		&format!("Mailslot on {}", crate::discovery::hostname()),
		port,
		cfg.security.https,
		(!label.is_empty()).then_some(label),
	);
	match result {
		Ok(a) => {
			inner.mutate(|s| s.advertising = true);
			Some(a)
		}
		Err(e) => {
			tracing::warn!("mDNS advertising failed: {e}");
			inner.mutate(|s| s.advertising = false);
			None
		}
	}
}

async fn start_inner(inner: Arc<Inner>) -> Result<EngineState> {
	let mut running = inner.running.lock().await;
	if running.is_some() {
		return Err(Error::State("already sharing".into()));
	}
	let cfg = inner.config();
	cfg.validate()?;
	inner.mutate(|s| {
		s.status = Status::Starting;
		s.restart_required = false;
		s.internet = InternetState::Off;
	});
	inner.emit_state();

	match start_server(&inner, &cfg).await {
		Ok(r) => {
			*running = Some(r);
		}
		Err(e) => {
			inner.mutate(|s| s.status = Status::Stopped);
			inner.emit_state();
			return Err(e);
		}
	}
	drop(running);
	if cfg.network.upnp {
		set_internet_inner(inner.clone(), true).await;
	}
	Ok(inner.state())
}

async fn start_server(inner: &Arc<Inner>, cfg: &Config) -> Result<Running> {
	let token = inner.token();
	#[cfg(feature = "tls")]
	let tls = if cfg.security.https {
		let hosts: Vec<String> = netinfo::lan_addrs()
			.await
			.into_iter()
			.map(|a| a.ip.to_string())
			.collect();
		Some(crate::tls::load_or_generate(&inner.paths.tls_dir(), &hosts).await?)
	} else {
		None
	};
	#[cfg(not(feature = "tls"))]
	if cfg.security.https {
		return Err(Error::FeatureDisabled("tls"));
	}
	let expires_at = (cfg.sharing.expiry_minutes > 0).then(|| {
		SystemTime::now() + Duration::from_secs(u64::from(cfg.sharing.expiry_minutes) * 60)
	});

	// Seed the dedupe index from earlier sessions.
	if let Ok(entries) = inner.history.load().await {
		for e in entries
			.into_iter()
			.filter(|e| e.direction == Direction::Received)
		{
			if let Some(p) = e.path {
				inner.hashes.insert(e.sha256, p);
			}
		}
	}

	let (tx, mut rx) = mpsc::unbounded_channel();
	let mut sc = ServerConfig::new(bind_addr(cfg)?, cfg.storage.dest_dir.clone(), token.clone());
	sc.settings = ServerSettings::from_config(cfg, expires_at);
	sc.free_space = inner.free_space.clone();
	sc.hashes = inner.hashes.clone();
	#[cfg(feature = "tls")]
	{
		sc.tls = tls.clone();
	}
	let server = Server::start(sc, tx).await?;
	let addr = server.local_addr();
	let cancel = CancellationToken::new();

	// Event pump.
	{
		let inner = inner.clone();
		let cancel = cancel.clone();
		inner.handle.clone().spawn(async move {
			while let Some(ev) = rx.recv().await {
				match ev {
					ServerEvent::Upload(u) => {
						inner.mutate(|s| s.apply_upload(&u));
						if let UploadEvent::Finished {
							saved_as,
							size,
							sha256,
							path,
							peer,
							..
						} = &u && inner.config().notifications.keep_history
						{
							let entry = HistoryEntry::now(
								Direction::Received,
								saved_as.clone(),
								*size,
								sha256.clone(),
								Some(peer.clone()),
								Some(path.clone()),
							);
							let _ = inner.history.append(&entry).await;
						}
						inner.emit(Event::Upload(u));
					}
					ServerEvent::FileLimitReached => {
						let inner = inner.clone();
						let h = inner.handle.clone();
						h.spawn(async move { stop_inner(inner, StopReason::FileLimit).await });
					}
				}
			}
			let _ = cancel;
		});
	}

	// Timers.
	let timers = [
		(
			expires_at.map(|t| t.duration_since(SystemTime::now()).unwrap_or_default()),
			StopReason::Expired,
		),
		(
			(cfg.sharing.auto_stop_minutes > 0)
				.then(|| Duration::from_secs(u64::from(cfg.sharing.auto_stop_minutes) * 60)),
			StopReason::Timer,
		),
	];
	for (dur, reason) in timers {
		if let Some(d) = dur {
			let inner = inner.clone();
			let cancel = cancel.clone();
			inner.handle.clone().spawn(async move {
				tokio::select! {
					_ = cancel.cancelled() => {}
					_ = tokio::time::sleep(d) => stop_inner(inner, reason).await,
				}
			});
		}
	}

	let pin = {
		#[cfg(feature = "tls")]
		{
			tls.as_ref().map(|m| m.pin.clone())
		}
		#[cfg(not(feature = "tls"))]
		{
			None::<String>
		}
	};
	let urls = build_urls(inner, addr, &token, &pin, cfg).await;
	inner.mutate(|s| {
		s.status = Status::Sharing;
		s.addr = Some(addr);
		s.urls = urls;
		s.expires_at = expires_at;
		s.tls_pin = pin;
	});
	inner.emit_state();
	#[cfg(feature = "discovery")]
	let advertiser = start_advertising(inner, cfg, addr.port());
	Ok(Running {
		server,
		cancel,
		portmap: None,
		#[cfg(feature = "discovery")]
		advertiser,
	})
}

async fn stop_inner(inner: Arc<Inner>, reason: StopReason) {
	let mut guard = inner.running.lock().await;
	let Some(mut r) = guard.take() else { return };
	inner.mutate(|s| s.status = Status::Stopping);
	inner.emit_state();
	r.cancel.cancel();
	if let Some((cancel, task)) = r.portmap.take() {
		cancel.cancel();
		let _ = tokio::time::timeout(Duration::from_secs(8), task).await;
	}
	r.server.shutdown().await;
	#[cfg(feature = "discovery")]
	drop(r.advertiser.take());
	inner.mutate(|s| {
		s.paused = false;
		s.advertising = false;
		s.status = Status::Stopped;
		s.addr = None;
		s.urls.clear();
		s.expires_at = None;
		s.tls_pin = None;
		s.internet = InternetState::Off;
		s.restart_required = false;
		for u in s
			.uploads
			.iter_mut()
			.filter(|u| u.state == UploadState::Receiving)
		{
			u.state = UploadState::Failed;
			u.reason = Some("sharing stopped".into());
		}
	});
	inner.emit(Event::Internet(InternetEvent::State(InternetState::Off)));
	inner.emit(Event::Stopped(reason));
	inner.emit_state();
}

fn refresh_internet_url(inner: &Inner, token: &Token) {
	let cfg = inner.config();
	inner.mutate(|s| {
		if let InternetState::Open { external, url, .. } = &mut s.internet {
			let mut link =
				MailslotLink::new(external.ip().to_string(), external.port(), token.clone());
			if cfg.security.https {
				link = link.with_tls(s.tls_pin.clone());
			}
			*url = link.drop_url();
		}
	});
}

async fn set_internet_inner(inner: Arc<Inner>, on: bool) {
	#[cfg(feature = "portmap")]
	{
		let mut guard = inner.running.lock().await;
		let Some(r) = guard.as_mut() else { return };
		if let Some((cancel, task)) = r.portmap.take() {
			cancel.cancel();
			let _ = tokio::time::timeout(Duration::from_secs(8), task).await;
			set_internet_state(&inner, InternetState::Off);
		}
		if !on {
			return;
		}
		let cancel = CancellationToken::new();
		let port = r.server.local_addr().port();
		let bind_ip = r.server.local_addr().ip();
		let task = inner
			.handle
			.spawn(portmap_task(inner.clone(), cancel.clone(), port, bind_ip));
		r.portmap = Some((cancel, task));
	}
	#[cfg(not(feature = "portmap"))]
	{
		let _ = (inner, on);
	}
}

#[cfg(feature = "portmap")]
fn set_internet_state(inner: &Inner, st: InternetState) {
	inner.mutate(|s| s.internet = st.clone());
	inner.emit(Event::Internet(InternetEvent::State(st)));
	inner.emit_state();
}

#[cfg(feature = "portmap")]
async fn portmap_task(inner: Arc<Inner>, cancel: CancellationToken, port: u16, bind_ip: IpAddr) {
	use crate::portmap;
	let cfg = inner.config();
	set_internet_state(&inner, InternetState::Mapping);
	let public_ip = if cfg.network.public_ip_lookup {
		netinfo::lookup_public_ip().await.ok()
	} else {
		None
	};
	let mut base = inner.portmap_overrides.clone().unwrap_or_default();
	base.local_port = port;
	if !bind_ip.is_unspecified() && base.local_ip.is_none() {
		base.local_ip = Some(bind_ip);
	}
	base.public_ip = public_ip;
	let expires = inner.state().expires_at;
	let lease_for = |now: SystemTime| -> Duration {
		let cap = Duration::from_secs(3600);
		match expires {
			Some(t) => t
				.duration_since(now)
				.unwrap_or_default()
				.clamp(Duration::from_secs(120), cap),
			None => cap,
		}
	};

	for p in portmap::cleanup_stale(&base).await {
		inner.emit(Event::Internet(InternetEvent::StaleRemoved {
			external_port: p,
		}));
	}
	base.lease = lease_for(SystemTime::now());
	let mut mapping = match portmap::open(&base).await {
		Ok(m) => m,
		Err(d) => {
			set_internet_state(&inner, InternetState::Failed(d));
			cancel.cancelled().await;
			return;
		}
	};
	let announce = |inner: &Inner, m: &portmap::ActiveMapping| {
		let cfg = inner.config();
		let mut link = MailslotLink::new(
			m.external().ip().to_string(),
			m.external().port(),
			inner.token(),
		);
		if cfg.security.https {
			link = link.with_tls(inner.state().tls_pin);
		}
		set_internet_state(
			inner,
			InternetState::Open {
				external: m.external(),
				url: link.drop_url(),
				warnings: m.warnings().to_vec(),
			},
		);
	};
	announce(&inner, &mapping);
	loop {
		let wait = mapping.lease() / 2;
		tokio::select! {
			_ = cancel.cancelled() => break,
			_ = tokio::time::sleep(wait) => {
				if let Err(d) = mapping.renew().await {
					set_internet_state(&inner, InternetState::Failed(d));
					cancel.cancelled().await;
					return;
				}
			}
		}
	}
	let _ = mapping.remove().await;
}

#[cfg(feature = "client")]
async fn send_inner(inner: Arc<Inner>, req: SendRequest) -> Result<Vec<SendId>> {
	use crate::client::{Client, Notice};
	let files = crate::client::expand_paths(&req.paths).await?;
	let client_base = Client::new(req.link.clone(), req.options.clone())?;
	let sem = Arc::new(tokio::sync::Semaphore::new(req.options.parallel.max(1)));
	let mut ids = Vec::new();
	for path in files {
		let id = inner.next_send.fetch_add(1, Ordering::SeqCst);
		let name = path
			.file_name()
			.map(|n| n.to_string_lossy().into_owned())
			.unwrap_or_else(|| "upload".into());
		let total = tokio::fs::metadata(&path).await?.len();
		let ev = SendEvent::Started {
			id,
			name: name.clone(),
			total,
		};
		inner.mutate(|s| s.apply_send(&ev));
		inner.emit(Event::Send(ev));
		ids.push(id);

		let (i1, i2) = (inner.clone(), inner.clone());
		let client = client_base
			.clone()
			.on_progress(move |p| {
				let ev = SendEvent::Progress {
					id,
					sent: p.sent,
					total: p.total,
					bytes_per_sec: p.bytes_per_sec,
				};
				i1.mutate(|s| s.apply_send(&ev));
				i1.emit(Event::Send(ev));
			})
			.on_notice(move |n| {
				if let Notice::Retrying {
					attempt,
					delay_secs,
					..
				} = n
				{
					i2.emit(Event::Send(SendEvent::Retrying {
						id,
						attempt,
						delay_secs,
					}));
				}
			});
		let (sem, inner2, host) = (sem.clone(), inner.clone(), req.link.host.clone());
		let task = inner.handle.spawn(async move {
			let _permit = sem.acquire_owned().await;
			let result = client.send_file(&path, None).await;
			let ev = match result {
				Ok(o) if o.skipped => SendEvent::Skipped { id },
				Ok(o) => SendEvent::Finished {
					id,
					sha256: o.sha256,
					saved_as: o.saved_as,
				},
				Err(e) => SendEvent::Failed {
					id,
					reason: e.to_string(),
				},
			};
			if let SendEvent::Finished { sha256, .. } = &ev
				&& inner2.config().notifications.keep_history
			{
				let entry = HistoryEntry::now(
					Direction::Sent,
					name.clone(),
					total,
					sha256.clone(),
					Some(host),
					None,
				);
				let _ = inner2.history.append(&entry).await;
			}
			inner2.mutate(|s| s.apply_send(&ev));
			inner2.emit(Event::Send(ev));
			if let Ok(mut m) = inner2.sends.lock() {
				m.remove(&id);
			}
		});
		if let Ok(mut m) = inner.sends.lock() {
			m.insert(id, task.abort_handle());
		}
	}
	Ok(ids)
}
