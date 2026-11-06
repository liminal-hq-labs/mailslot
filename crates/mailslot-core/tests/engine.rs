// Engine commands, state and events.
//
// (c) Copyright 2026 Liminal HQ, Scott Morris
// SPDX-License-Identifier: Apache-2.0 OR MIT

#![allow(clippy::disallowed_methods)] // Tests may use blocking std calls for set-up and assertions.

mod support;

use std::time::Duration;

use futures_util::StreamExt;
use mailslot_core::client::{Client, ClientOptions};
use mailslot_core::config::{Config, Paths};
use mailslot_core::events::{
	Event, InternetState, SendEvent, Status, StopReason, UploadEvent, UploadState,
};
use mailslot_core::proto::MailslotLink;
use mailslot_core::{Engine, EngineBuilder, SendRequest};
use support::{Fixture, pattern, sha256_hex};

fn test_config(dir: &std::path::Path) -> Config {
	let mut c = Config::default();
	c.sharing.bind = "127.0.0.1".into();
	c.sharing.port = 0;
	c.storage.dest_dir = dir.join("received");
	c.limits.min_free_mb = 0;
	c.security.bad_token_lockout = 0;
	c.limits.rate_limit_per_min = 0;
	c
}

async fn engine(dir: &std::path::Path) -> Engine {
	mailslot_core::tls::ensure_default_provider();
	EngineBuilder::new(Paths::in_dir(dir))
		.config(test_config(dir))
		.persist(false)
		.build()
		.await
		.unwrap()
}

fn link_of(e: &Engine) -> MailslotLink {
	let port = e.state().addr.unwrap().port();
	MailslotLink::new("127.0.0.1", port, e.token())
}

async fn next_matching(
	events: &mut mailslot_core::EventStream,
	mut pred: impl FnMut(&Event) -> bool,
) -> Event {
	tokio::time::timeout(Duration::from_secs(10), async {
		while let Some(e) = events.next().await {
			if pred(&e) {
				return e;
			}
		}
		panic!("event stream ended");
	})
	.await
	.expect("timed out waiting for an event")
}

#[tokio::test]
async fn start_reaches_sharing_with_port_zero_and_stop_returns_to_stopped() {
	let dir = tempfile::tempdir().unwrap();
	let e = engine(dir.path()).await;
	assert_eq!(e.state().status, Status::Stopped);
	let mut ev = e.subscribe();
	let st = e.start().await.unwrap();
	assert_eq!(st.status, Status::Sharing);
	let addr = st.addr.unwrap();
	assert_ne!(addr.port(), 0);
	assert!(!st.urls.is_empty());
	assert!(
		st.urls
			.iter()
			.any(|u| u.host == "127.0.0.1" && u.url.contains(e.token().expose()))
	);
	assert!(st.expires_at.is_some(), "default expiry is one hour");
	next_matching(
		&mut ev,
		|e| matches!(e, Event::StateChanged(s) if s.status == Status::Sharing),
	)
	.await;
	assert!(e.start().await.is_err(), "starting twice is an error");
	e.stop().await.unwrap();
	let st = e.state();
	assert_eq!(st.status, Status::Stopped);
	assert!(st.addr.is_none() && st.urls.is_empty());
	next_matching(&mut ev, |e| {
		matches!(e, Event::Stopped(StopReason::Requested))
	})
	.await;
	// The port is free again.
	assert!(tokio::net::TcpListener::bind(addr).await.is_ok());
}

#[tokio::test]
async fn uploads_flow_into_events_state_and_history() {
	let dir = tempfile::tempdir().unwrap();
	let e = engine(dir.path()).await;
	let mut ev = e.subscribe();
	e.start().await.unwrap();
	let src = dir.path().join("in.bin");
	let data = pattern(700_000);
	std::fs::write(&src, &data).unwrap();
	Client::new(link_of(&e), ClientOptions::default())
		.unwrap()
		.send_file(&src, None)
		.await
		.unwrap();
	let fin = next_matching(&mut ev, |e| {
		matches!(e, Event::Upload(UploadEvent::Finished { .. }))
	})
	.await;
	let Event::Upload(UploadEvent::Finished {
		sha256, saved_as, ..
	}) = fin
	else {
		unreachable!()
	};
	assert_eq!(
		(sha256.as_str(), saved_as.as_str()),
		(sha256_hex(&data).as_str(), "in.bin")
	);
	let st = e.state();
	assert_eq!(st.received_files, 1);
	assert_eq!(st.received_bytes, data.len() as u64);
	let up = &st.uploads[0];
	assert_eq!((up.state, up.fraction()), (UploadState::Done, 1.0));
	assert_eq!(up.sha256.as_deref(), Some(sha256_hex(&data).as_str()));
	// History landed on disk (the pump writes it just before the event).
	let hist = e.history().load().await.unwrap();
	assert_eq!(hist.len(), 1);
	assert_eq!(hist[0].name, "in.bin");
	e.clear_finished();
	assert!(e.state().uploads.is_empty());
	e.stop().await.unwrap();
}

#[tokio::test]
async fn rotating_the_token_invalidates_the_old_link() {
	let dir = tempfile::tempdir().unwrap();
	let e = engine(dir.path()).await;
	e.start().await.unwrap();
	let old = link_of(&e);
	let http = reqwest::Client::new();
	assert_eq!(
		http.get(format!("{}health", old.drop_url()))
			.send()
			.await
			.unwrap()
			.status(),
		200
	);
	let mut ev = e.subscribe();
	let new = e.rotate_token().await.unwrap();
	assert_ne!(new.expose(), old.token.expose());
	assert_eq!(
		http.get(format!("{}health", old.drop_url()))
			.send()
			.await
			.unwrap()
			.status(),
		404
	);
	assert_eq!(
		http.get(format!("{}health", link_of(&e).drop_url()))
			.send()
			.await
			.unwrap()
			.status(),
		200
	);
	assert!(e.state().urls[0].url.contains(new.expose()));
	next_matching(&mut ev, |e| matches!(e, Event::TokenRotated)).await;
	e.stop().await.unwrap();
}

#[tokio::test]
async fn settings_apply_live_and_flag_restarts() {
	let dir = tempfile::tempdir().unwrap();
	let e = engine(dir.path()).await;
	e.start().await.unwrap();
	let http = reqwest::Client::new();
	let url = format!("{}tiny.bin", link_of(&e).drop_url());
	assert_eq!(
		http.put(&url)
			.body(vec![0u8; 2 * 1024 * 1024])
			.send()
			.await
			.unwrap()
			.status(),
		201
	);
	let mut c = e.config();
	c.limits.max_file_mb = 1;
	e.update_settings(c.clone()).await.unwrap();
	assert_eq!(
		http.put(&url)
			.body(vec![0u8; 2 * 1024 * 1024])
			.send()
			.await
			.unwrap()
			.status(),
		413
	);
	assert!(!e.state().restart_required);
	c.sharing.port = 40123;
	e.update_settings(c).await.unwrap();
	assert!(e.state().restart_required);
	let mut bad = e.config();
	bad.sharing.bind = "not an ip".into();
	assert!(e.update_settings(bad).await.is_err());
	e.stop().await.unwrap();
}

#[tokio::test]
async fn one_shot_mode_stops_the_share() {
	let dir = tempfile::tempdir().unwrap();
	let mut cfg = test_config(dir.path());
	cfg.sharing.stop_after_files = 1;
	mailslot_core::tls::ensure_default_provider();
	let e = EngineBuilder::new(Paths::in_dir(dir.path()))
		.config(cfg)
		.persist(false)
		.build()
		.await
		.unwrap();
	let mut ev = e.subscribe();
	e.start().await.unwrap();
	let url = format!("{}once.txt", link_of(&e).drop_url());
	assert_eq!(
		reqwest::Client::new()
			.put(url)
			.body("x")
			.send()
			.await
			.unwrap()
			.status(),
		201
	);
	next_matching(&mut ev, |e| {
		matches!(e, Event::Stopped(StopReason::FileLimit))
	})
	.await;
	assert_eq!(e.state().status, Status::Stopped);
}

#[tokio::test]
async fn start_failure_reports_and_returns_to_stopped() {
	let dir = tempfile::tempdir().unwrap();
	let hog = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let mut cfg = test_config(dir.path());
	cfg.sharing.port = hog.local_addr().unwrap().port();
	mailslot_core::tls::ensure_default_provider();
	let e = EngineBuilder::new(Paths::in_dir(dir.path()))
		.config(cfg)
		.persist(false)
		.build()
		.await
		.unwrap();
	let err = e.start().await.unwrap_err();
	assert!(matches!(err, mailslot_core::Error::AddrInUse), "{err:?}");
	assert_eq!(e.state().status, Status::Stopped);
	let free = e
		.find_free_port(hog.local_addr().unwrap().port())
		.await
		.unwrap();
	assert_ne!(free, hog.local_addr().unwrap().port());
}

#[tokio::test]
async fn send_command_reports_progress_and_dedupes_via_history() {
	let target = Fixture::start().await;
	let dir = tempfile::tempdir().unwrap();
	let e = engine(dir.path()).await;
	let mut ev = e.subscribe();
	let src = dir.path().join("out.bin");
	let data = pattern(900_000);
	std::fs::write(&src, &data).unwrap();
	let link = MailslotLink::new("127.0.0.1", target.addr.port(), target.token.clone());
	let ids = e
		.send(SendRequest {
			link: link.clone(),
			paths: vec![src.clone()],
			options: ClientOptions::default(),
		})
		.await
		.unwrap();
	assert_eq!(ids.len(), 1);
	let fin = next_matching(&mut ev, |e| {
		matches!(
			e,
			Event::Send(SendEvent::Finished { .. } | SendEvent::Failed { .. })
		)
	})
	.await;
	let Event::Send(SendEvent::Finished { sha256, .. }) = fin else {
		panic!("{fin:?}")
	};
	assert_eq!(sha256, sha256_hex(&data));
	let s = &e.state().sends[0];
	assert!(s.finished && s.sent == data.len() as u64 && s.error.is_none());
	assert_eq!(e.history().load().await.unwrap()[0].name, "out.bin");
	// Sending the same content again is skipped by the server's dedupe.
	e.send(SendRequest {
		link,
		paths: vec![src],
		options: ClientOptions::default(),
	})
	.await
	.unwrap();
	next_matching(&mut ev, |e| {
		matches!(e, Event::Send(SendEvent::Skipped { .. }))
	})
	.await;
}

#[tokio::test]
async fn dedupe_index_survives_a_restart_through_history() {
	let dir = tempfile::tempdir().unwrap();
	let src = dir.path().join("keep.bin");
	let data = pattern(10_000);
	std::fs::write(&src, &data).unwrap();
	let first = engine(dir.path()).await;
	let mut ev = first.subscribe();
	first.start().await.unwrap();
	Client::new(link_of(&first), ClientOptions::default())
		.unwrap()
		.send_file(&src, None)
		.await
		.unwrap();
	next_matching(&mut ev, |e| {
		matches!(e, Event::Upload(UploadEvent::Finished { .. }))
	})
	.await;
	first.stop().await.unwrap();
	let second = engine(dir.path()).await;
	second.start().await.unwrap();
	let c = Client::new(link_of(&second), ClientOptions::default()).unwrap();
	assert!(c.have(&sha256_hex(&data)).await.unwrap());
	second.stop().await.unwrap();
}

#[tokio::test]
async fn upnp_exposure_uses_the_gateway_and_cleans_up() {
	use mailslot_core::portmap::PortMapOptions;
	let igd = support::fake_igd::FakeIgd::start("203.0.113.9").await;
	let dir = tempfile::tempdir().unwrap();
	let mut cfg = test_config(dir.path());
	cfg.network.upnp = true;
	mailslot_core::tls::ensure_default_provider();
	let opts = PortMapOptions {
		local_ip: Some("127.0.0.1".parse().unwrap()),
		ssdp_addr: Some(igd.ssdp_addr),
		search_timeout: Duration::from_secs(3),
		..Default::default()
	};
	let e = EngineBuilder::new(Paths::in_dir(dir.path()))
		.config(cfg)
		.persist(false)
		.portmap_options(opts)
		.build()
		.await
		.unwrap();
	let mut ev = e.subscribe();
	e.start().await.unwrap();
	let open = next_matching(&mut ev, |e| {
		matches!(
			e,
			Event::Internet(mailslot_core::events::InternetEvent::State(
				InternetState::Open { .. }
			))
		)
	})
	.await;
	let Event::Internet(mailslot_core::events::InternetEvent::State(InternetState::Open {
		external,
		url,
		..
	})) = open
	else {
		unreachable!()
	};
	assert_eq!(external.ip().to_string(), "203.0.113.9");
	assert!(url.starts_with("http://203.0.113.9:") && url.contains(e.token().expose()));
	assert_eq!(igd.mapping_count(), 1);
	assert!(matches!(e.state().internet, InternetState::Open { .. }));
	// Turning it off removes the mapping without stopping the share.
	e.set_internet_exposed(false).await.unwrap();
	assert_eq!(igd.mapping_count(), 0);
	assert_eq!(e.state().status, Status::Sharing);
	e.set_internet_exposed(true).await.unwrap();
	next_matching(&mut ev, |e| {
		matches!(
			e,
			Event::Internet(mailslot_core::events::InternetEvent::State(
				InternetState::Open { .. }
			))
		)
	})
	.await;
	assert_eq!(igd.mapping_count(), 1);
	// Stopping removes it.
	e.stop().await.unwrap();
	assert_eq!(igd.mapping_count(), 0);
	assert_eq!(e.state().internet, InternetState::Off);
}

/// Polls a future on the calling thread with no runtime at all, standing in for any
/// non-tokio executor such as the GLib main loop.
fn block_on<F: std::future::Future>(fut: F) -> F::Output {
	use std::sync::Arc;
	use std::task::{Context, Poll, Wake, Waker};
	struct Parker(std::thread::Thread);
	impl Wake for Parker {
		fn wake(self: Arc<Self>) {
			self.0.unpark();
		}
	}
	let waker = Waker::from(Arc::new(Parker(std::thread::current())));
	let mut cx = Context::from_waker(&waker);
	let mut fut = std::pin::pin!(fut);
	loop {
		if let Poll::Ready(v) = fut.as_mut().poll(&mut cx) {
			return v;
		}
		std::thread::park_timeout(Duration::from_millis(50));
	}
}

#[test]
fn own_runtime_engine_is_driven_by_a_foreign_executor() {
	let dir = tempfile::tempdir().unwrap();
	let path = dir.path().to_path_buf();
	mailslot_core::tls::ensure_default_provider();
	let status = block_on(async {
		let e = EngineBuilder::new(Paths::in_dir(&path))
			.config(test_config(&path))
			.persist(false)
			.own_runtime(true)
			.build()
			.await
			.unwrap();
		let st = e.start().await.unwrap();
		let addr = st.addr.unwrap();
		assert!(std::net::TcpStream::connect(addr).is_ok());
		e.stop().await.unwrap();
		st.status
	});
	assert_eq!(status, Status::Sharing);
}

#[tokio::test]
async fn pause_refuses_uploads_until_resumed() {
	let dir = tempfile::tempdir().unwrap();
	let e = engine(dir.path()).await;
	assert!(
		e.pause().await.is_err(),
		"pausing while stopped is an error"
	);
	e.start().await.unwrap();
	let base = link_of(&e).drop_url();
	let http = reqwest::Client::new();
	e.pause().await.unwrap();
	assert!(e.state().paused);
	assert_eq!(
		http.put(format!("{base}a.txt"))
			.body("x")
			.send()
			.await
			.unwrap()
			.status(),
		503
	);
	let health: serde_json::Value = http
		.get(format!("{base}health"))
		.send()
		.await
		.unwrap()
		.json()
		.await
		.unwrap();
	assert_eq!(health["accepting"], false);
	e.resume().await.unwrap();
	assert!(!e.state().paused);
	assert_eq!(
		http.put(format!("{base}a.txt"))
			.body("x")
			.send()
			.await
			.unwrap()
			.status(),
		201
	);
	e.pause().await.unwrap();
	e.stop().await.unwrap();
	assert!(!e.state().paused, "stopping clears the pause");
}

#[tokio::test]
async fn link_label_names_the_sender_folder_and_travels_in_links() {
	use mailslot_core::config::SubfolderScheme;
	let dir = tempfile::tempdir().unwrap();
	let mut cfg = test_config(dir.path());
	cfg.storage.subfolder = SubfolderScheme::BySender;
	cfg.sharing.link_label = "Jules".into();
	mailslot_core::tls::ensure_default_provider();
	let e = EngineBuilder::new(Paths::in_dir(dir.path()))
		.config(cfg)
		.persist(false)
		.build()
		.await
		.unwrap();
	let st = e.start().await.unwrap();
	assert!(
		st.urls[0].link.contains("label=Jules"),
		"{}",
		st.urls[0].link
	);
	// A sender using the label-carrying link is filed under it, with no extra options.
	let link = MailslotLink::parse(&st.urls[0].link).unwrap();
	assert_eq!(link.label.as_deref(), Some("Jules"));
	let src = dir.path().join("f.txt");
	std::fs::write(&src, "x").unwrap();
	Client::new(link, ClientOptions::default())
		.unwrap()
		.send_file(&src, None)
		.await
		.unwrap();
	assert!(e.config().storage.dest_dir.join("Jules/f.txt").is_file());
	// A plain PUT without any header lands there too.
	let url = format!("{}g.txt", link_of(&e).drop_url());
	reqwest::Client::new()
		.put(url)
		.body("y")
		.send()
		.await
		.unwrap();
	assert!(e.config().storage.dest_dir.join("Jules/g.txt").is_file());
	e.stop().await.unwrap();
}

#[tokio::test]
async fn mdns_advertises_only_when_enabled_and_never_the_secret() {
	let dir = tempfile::tempdir().unwrap();
	// Off by default: nothing is announced.
	let e = engine(dir.path()).await;
	e.start().await.unwrap();
	assert!(!e.state().advertising);
	e.stop().await.unwrap();

	let mut cfg = test_config(dir.path());
	cfg.network.mdns = true;
	cfg.sharing.link_label = "Desk".into();
	mailslot_core::tls::ensure_default_provider();
	let e = EngineBuilder::new(Paths::in_dir(dir.path()))
		.config(cfg)
		.persist(false)
		.build()
		.await
		.unwrap();
	let st = e.start().await.unwrap();
	assert!(st.advertising);
	let port = st.addr.unwrap().port();
	let mut found = None;
	for _ in 0..3 {
		let peers = e.discover_peers(Duration::from_secs(2)).await.unwrap();
		if let Some(p) = peers.into_iter().find(|p| p.port == port) {
			found = Some(p);
			break;
		}
	}
	let Some(peer) = found else {
		// Some sandboxes and CI containers have no multicast; the advertising state above
		// still proved the preference is honoured.
		assert!(
			std::env::var_os("MAILSLOT_REQUIRE_MDNS").is_none(),
			"mDNS multicast is required here but nothing was found"
		);
		eprintln!("no mDNS answers (multicast unavailable); skipping the browse checks");
		e.stop().await.unwrap();
		return;
	};
	assert_eq!(peer.label.as_deref(), Some("Desk"));
	assert!(
		!format!("{peer:?}").contains(e.token().expose()),
		"the secret is never announced"
	);
	assert!(peer.partial_link().ends_with(&format!(":{port}/")));
	e.stop().await.unwrap();
	assert!(!e.state().advertising);
}
