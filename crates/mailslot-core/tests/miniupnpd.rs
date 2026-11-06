// UPnP port mapping against a real miniupnpd gateway, run by scripts/upnp-test.sh.
//
// (c) Copyright 2026 Liminal HQ, Scott Morris
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! These tests need an isolated Docker network with miniupnpd as the gateway. They read
//! `MAILSLOT_UPNP_GATEWAY` (the gateway's address) and `MAILSLOT_UPNP_LAN_IP` (this
//! machine's address on that network) and never run unless asked with
//! `--run-ignored only`. Nothing here talks to a real router.

#![allow(clippy::disallowed_methods)] // Tests read the environment synchronously.

use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use igd_next::PortMappingProtocol;
use mailslot_core::portmap::{self, PortMapOptions, Protocol};

const TEST_PORT: u16 = 18931;

fn env_ip(name: &str) -> IpAddr {
	std::env::var(name)
		.unwrap_or_else(|_| panic!("{name} must be set (scripts/upnp-test.sh does this)"))
		.parse()
		.unwrap_or_else(|_| panic!("{name} is not an IP address"))
}

fn options() -> PortMapOptions {
	let lan = env_ip("MAILSLOT_UPNP_LAN_IP");
	PortMapOptions {
		local_port: TEST_PORT,
		local_ip: Some(lan),
		// Real multicast discovery, bound to the LAN address.
		ssdp_bind: Some(SocketAddr::new(lan, 0)),
		fallback_pmp: false,
		search_timeout: Duration::from_secs(8),
		lease: Duration::from_secs(300),
		..Default::default()
	}
}

/// Lists what the gateway currently maps, as `(external_port, internal_client, description)`.
async fn gateway_entries() -> Vec<(u16, String, String)> {
	let gateway_ip = env_ip("MAILSLOT_UPNP_GATEWAY");
	let mut search = igd_next::SearchOptions::default();
	search.bind_addr = SocketAddr::new(env_ip("MAILSLOT_UPNP_LAN_IP"), 0);
	search.timeout = Some(Duration::from_secs(8));
	search.allowed_gateway_ips = vec![gateway_ip];
	let gw = igd_next::aio::tokio::search_gateway(search)
		.await
		.expect("gateway found");
	let mut out = Vec::new();
	for i in 0..64 {
		match gw.get_generic_port_mapping_entry(i).await {
			Ok(e) => out.push((
				e.external_port,
				e.internal_client,
				e.port_mapping_description,
			)),
			Err(_) => break,
		}
	}
	out
}

/// Waits up to a few seconds for the gateway's listing to satisfy `cond`, since a gateway may
/// take a moment to show a mapping it has just accepted.
async fn listed_eventually(
	cond: impl Fn(&[(u16, String, String)]) -> bool,
) -> Vec<(u16, String, String)> {
	let mut entries = gateway_entries().await;
	for _ in 0..5 {
		if cond(&entries) {
			break;
		}
		tokio::time::sleep(Duration::from_secs(1)).await;
		entries = gateway_entries().await;
	}
	entries
}

#[tokio::test]
#[ignore = "needs gateway"]
async fn miniupnpd_map_renew_unmap() {
	let lan = env_ip("MAILSLOT_UPNP_LAN_IP").to_string();
	let mut m = portmap::open(&options()).await.expect("mapping opens");
	assert_eq!(m.protocol(), Protocol::Upnp);
	assert!(
		m.lease() <= Duration::from_secs(300) && m.lease() > Duration::ZERO,
		"mappings are never permanent"
	);
	let want = m.external().port();
	let entries = listed_eventually(|e| {
		e.iter()
			.any(|(p, c, d)| *p == want && *c == lan && d.starts_with("Mailslot"))
	})
	.await;
	assert!(
		entries
			.iter()
			.any(|(p, c, d)| *p == want && *c == lan && d.starts_with("Mailslot")),
		"mapped external port {want} not listed: {entries:?}"
	);
	m.renew().await.expect("renews");
	assert!(
		gateway_entries()
			.await
			.iter()
			.any(|(p, ..)| *p == m.external().port())
	);
	let port = m.external().port();
	m.remove().await.expect("removes");
	assert!(
		!gateway_entries().await.iter().any(|(p, ..)| *p == port),
		"mapping is gone after remove"
	);
	let _ = PortMappingProtocol::TCP;
}

#[tokio::test]
#[ignore = "needs gateway"]
async fn miniupnpd_cleanup_removes_stale_mappings() {
	let lan = env_ip("MAILSLOT_UPNP_LAN_IP").to_string();
	// Leave a mapping behind as a crashed run would, then clean it up.
	let m = portmap::open(&options()).await.expect("mapping opens");
	let port = m.external().port();
	std::mem::forget(m);
	assert!(
		gateway_entries()
			.await
			.iter()
			.any(|(p, c, _)| *p == port && *c == lan)
	);
	let removed = portmap::cleanup_stale(&options()).await;
	assert!(removed.contains(&port), "{removed:?}");
	assert!(!gateway_entries().await.iter().any(|(p, ..)| *p == port));
}
