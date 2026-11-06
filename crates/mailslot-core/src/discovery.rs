// Opt-in mDNS advertising and browsing for Mailslot shares on the local network.
//
// (c) Copyright 2026 Liminal HQ, Scott Morris
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Local-network discovery.
//!
//! A share advertises `_mailslot._tcp` with its host name and port only while it is
//! sharing and only when the user turned the preference on. The announcement carries a
//! protocol version, whether HTTPS is on and the optional link label. **It never carries
//! the secret**: discovery tells a sender where a share is, and the link still has to be
//! given to them.

use std::net::IpAddr;
use std::time::{Duration, Instant};

use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// The mDNS service type.
pub const SERVICE_TYPE: &str = "_mailslot._tcp.local.";

fn mdns_err(e: mdns_sd::Error) -> Error {
	Error::State(format!("mDNS: {e}"))
}

/// This machine's host name.
pub fn hostname() -> String {
	let name = rustix::system::uname()
		.nodename()
		.to_string_lossy()
		.into_owned();
	let name = name.split('.').next().unwrap_or("").trim().to_owned();
	if name.is_empty() {
		"mailslot".into()
	} else {
		name
	}
}

/// A running announcement. Dropping it stops advertising.
pub struct Advertiser {
	daemon: ServiceDaemon,
	fullname: String,
}

impl std::fmt::Debug for Advertiser {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		f.debug_struct("Advertiser")
			.field("fullname", &self.fullname)
			.finish()
	}
}

impl Advertiser {
	/// Starts announcing a share. `instance` is the display name.
	pub fn start(instance: &str, port: u16, tls: bool, label: Option<&str>) -> Result<Self> {
		let daemon = ServiceDaemon::new().map_err(mdns_err)?;
		let host = format!("{}.local.", hostname());
		let mut props = vec![
			("v".to_owned(), "1".to_owned()),
			("tls".to_owned(), if tls { "1" } else { "0" }.to_owned()),
		];
		if let Some(l) = label.filter(|l| !l.is_empty()) {
			props.push(("label".to_owned(), l.chars().take(64).collect()));
		}
		let info = ServiceInfo::new(SERVICE_TYPE, instance, &host, "", port, &props[..])
			.map_err(mdns_err)?
			.enable_addr_auto();
		let fullname = info.get_fullname().to_owned();
		daemon.register(info).map_err(mdns_err)?;
		Ok(Self { daemon, fullname })
	}

	/// The instance's full mDNS name.
	pub fn fullname(&self) -> &str {
		&self.fullname
	}
}

impl Drop for Advertiser {
	fn drop(&mut self) {
		// Both calls only queue a command for the daemon thread.
		let _ = self.daemon.unregister(&self.fullname);
		let _ = self.daemon.shutdown();
	}
}

/// A share found on the network.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Peer {
	/// Display name, usually the host name.
	pub instance: String,
	/// mDNS host name.
	pub host: String,
	/// IPv4 addresses.
	pub addresses: Vec<IpAddr>,
	/// TCP port.
	pub port: u16,
	/// Whether the share uses HTTPS.
	pub tls: bool,
	/// The link label, if the owner set one.
	pub label: Option<String>,
}

impl Peer {
	/// A `mailslot://` address without the secret, to prefill a Send form.
	pub fn partial_link(&self) -> String {
		let host = self
			.addresses
			.first()
			.map(ToString::to_string)
			.unwrap_or_else(|| self.host.trim_end_matches('.').to_owned());
		format!("mailslot://{host}:{}/", self.port)
	}
}

/// Browses for shares for `timeout` and returns what answered.
pub async fn browse(timeout: Duration) -> Result<Vec<Peer>> {
	let daemon = ServiceDaemon::new().map_err(mdns_err)?;
	let rx = daemon.browse(SERVICE_TYPE).map_err(mdns_err)?;
	let end = Instant::now() + timeout;
	let mut peers: Vec<Peer> = Vec::new();
	while let Some(left) = end.checked_duration_since(Instant::now()) {
		let Ok(Ok(event)) = tokio::time::timeout(left, rx.recv_async()).await else {
			break;
		};
		if let ServiceEvent::ServiceResolved(info) = event {
			let fullname = info.get_fullname().to_owned();
			let instance = fullname
				.strip_suffix(SERVICE_TYPE)
				.map(|s| s.trim_end_matches('.').to_owned())
				.unwrap_or(fullname);
			let peer = Peer {
				instance,
				host: info.get_hostname().to_owned(),
				addresses: info
					.get_addresses_v4()
					.into_iter()
					.map(IpAddr::V4)
					.collect(),
				port: info.get_port(),
				tls: info.get_property_val_str("tls") == Some("1"),
				label: info
					.get_property_val_str("label")
					.map(str::to_owned)
					.filter(|l| !l.is_empty()),
			};
			if !peers
				.iter()
				.any(|p| p.instance == peer.instance && p.port == peer.port)
			{
				peers.push(peer);
			}
		}
	}
	let _ = daemon.shutdown();
	Ok(peers)
}
