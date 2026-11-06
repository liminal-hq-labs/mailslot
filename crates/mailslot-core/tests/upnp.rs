// UPnP port mapping against an in-process fake gateway; nothing touches a real router.
//
// (c) Copyright 2026 Liminal HQ, Scott Morris
// SPDX-License-Identifier: Apache-2.0 OR MIT

#![allow(clippy::disallowed_methods)] // Tests may use blocking std calls for set-up and assertions.

mod support;

use std::time::Duration;

use mailslot_core::events::{DiagnosticKind, InternetState};
use mailslot_core::portmap::{self, PortMapOptions, Protocol};
use support::fake_igd::{FakeIgd, Mapping};

fn opts(igd: &FakeIgd) -> PortMapOptions {
	PortMapOptions {
		local_port: 8931,
		local_ip: Some("127.0.0.1".parse().unwrap()),
		ssdp_addr: Some(igd.ssdp_addr),
		search_timeout: Duration::from_secs(3),
		..Default::default()
	}
}

#[tokio::test]
async fn maps_renews_and_removes_with_a_lease() {
	let igd = FakeIgd::start("203.0.113.7").await;
	let mut m = portmap::open(&opts(&igd)).await.expect("mapping");
	assert_eq!(m.protocol(), Protocol::Upnp);
	assert_eq!(m.external(), "203.0.113.7:8931".parse().unwrap());
	assert!(m.warnings().is_empty());
	{
		let map = igd.mappings.lock().unwrap();
		let e = map.get(&8931).expect("entry on the router");
		assert!(e.description.starts_with("Mailslot"));
		assert_eq!(e.internal_client, "127.0.0.1");
		assert!(e.lease > 0 && e.lease <= 3600, "lease {}", e.lease);
	}
	m.renew().await.unwrap();
	assert_eq!(igd.mapping_count(), 1);
	m.remove().await.unwrap();
	assert_eq!(igd.mapping_count(), 0);
}

#[tokio::test]
async fn removes_stale_mailslot_mappings_only() {
	let igd = FakeIgd::start("203.0.113.7").await;
	let stale = |port: u16, desc: &str, client: &str| Mapping {
		external_port: port,
		internal_client: client.into(),
		internal_port: port,
		description: desc.into(),
		lease: 100,
	};
	igd.preload(stale(9001, "Mailslot (ca.liminalhq.mailslot)", "127.0.0.1"));
	igd.preload(stale(9002, "Other app", "127.0.0.1"));
	igd.preload(stale(
		9003,
		"Mailslot (ca.liminalhq.mailslot)",
		"192.168.1.50",
	));
	let removed = portmap::cleanup_stale(&opts(&igd)).await;
	assert_eq!(removed, vec![9001]);
	let left: Vec<u16> = igd.mappings.lock().unwrap().keys().copied().collect();
	assert_eq!(left, vec![9002, 9003]);
}

#[tokio::test]
async fn refused_mappings_are_explained() {
	let igd = FakeIgd::start("203.0.113.7").await;
	*igd.refuse_adds.lock().unwrap() = true;
	let d = portmap::open(&opts(&igd)).await.unwrap_err();
	assert_eq!(d.kind, DiagnosticKind::MappingRefused);
	assert!(d.message.contains("UPnP"));
	assert!(d.message.contains("UPnP error 606: Action not authorized"));
}

#[tokio::test]
async fn internal_client_is_the_address_used_to_reach_the_router() {
	// No override: the address comes from the route to the router, which on loopback is
	// loopback itself, and a MiniUPnPd-style router accepts it.
	let igd = FakeIgd::start("203.0.113.7").await;
	*igd.require_own_address.lock().unwrap() = true;
	let mut o = opts(&igd);
	o.local_ip = None;
	let m = portmap::open(&o).await.expect("mapping");
	assert_eq!(*igd.requested_clients.lock().unwrap(), vec!["127.0.0.1"]);
	assert_eq!(
		m.details().internal_client,
		Some("127.0.0.1".parse().unwrap())
	);
	assert_eq!(m.details().gateway, Some("127.0.0.1".parse().unwrap()));
	m.remove().await.unwrap();
}

#[tokio::test]
async fn error_606_names_the_internal_client_and_the_network_problem() {
	let igd = FakeIgd::start("203.0.113.7").await;
	*igd.require_own_address.lock().unwrap() = true;
	let mut o = opts(&igd);
	// An address that is not this machine's and not on the router's network.
	o.local_ip = Some("172.19.0.1".parse().unwrap());
	let d = portmap::open(&o).await.unwrap_err();
	assert_eq!(d.kind, DiagnosticKind::MappingRefused);
	assert!(
		d.message.contains("UPnP error 606: Action not authorized"),
		"{}",
		d.message
	);
	assert!(d.message.contains("172.19.0.1"), "{}", d.message);
	assert!(
		d.message.contains("not on the router's network"),
		"{}",
		d.message
	);
	assert!(d.message.contains("UPnP is enabled for this network"));
	let details = d.details.expect("details");
	assert_eq!(details.error_code, Some(606));
	assert_eq!(details.internal_client, Some("172.19.0.1".parse().unwrap()));
	assert_eq!(*igd.requested_clients.lock().unwrap(), vec!["172.19.0.1"]);
	assert_eq!(igd.mapping_count(), 0);
}

#[tokio::test]
async fn port_conflict_falls_back_to_another_external_port() {
	let igd = FakeIgd::start("203.0.113.7").await;
	igd.preload(Mapping {
		external_port: 8931,
		internal_client: "192.168.1.9".into(),
		internal_port: 8931,
		description: "Other".into(),
		lease: 0,
	});
	let m = portmap::open(&opts(&igd)).await.expect("any port");
	assert_ne!(m.external().port(), 8931);
}

#[tokio::test]
async fn cgnat_and_double_nat_are_flagged() {
	let igd = FakeIgd::start("100.72.1.2").await;
	let m = portmap::open(&opts(&igd)).await.unwrap();
	assert_eq!(m.warnings()[0].kind, DiagnosticKind::Cgnat);
	m.remove().await.unwrap();
	*igd.external_ip.lock().unwrap() = "192.168.0.2".into();
	let m = portmap::open(&opts(&igd)).await.unwrap();
	assert_eq!(m.warnings()[0].kind, DiagnosticKind::DoubleNat);
	m.remove().await.unwrap();
	// A mismatch with the opt-in public lookup is reported too.
	*igd.external_ip.lock().unwrap() = "203.0.113.7".into();
	let mut o = opts(&igd);
	o.public_ip = Some("198.51.100.4".parse().unwrap());
	let m = portmap::open(&o).await.unwrap();
	assert_eq!(m.warnings()[0].kind, DiagnosticKind::AddressMismatch);
}

#[tokio::test]
async fn no_gateway_is_reported() {
	// A bound UDP socket nobody answers on.
	let silent = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
	let o = PortMapOptions {
		local_port: 8931,
		local_ip: Some("127.0.0.1".parse().unwrap()),
		ssdp_addr: Some(silent.local_addr().unwrap()),
		search_timeout: Duration::from_millis(400),
		..Default::default()
	};
	let d = portmap::open(&o).await.unwrap_err();
	assert_eq!(d.kind, DiagnosticKind::NoGateway);
	let _ = InternetState::Off;
}
