// Local network discovery: LAN addresses, default gateway and an opt-in public-IP lookup.
//
// (c) Copyright 2026 Liminal HQ, Scott Morris
// SPDX-License-Identifier: Apache-2.0 OR MIT

use std::net::{IpAddr, Ipv4Addr};

use serde::Serialize;

use crate::error::{Error, Result};

/// One local address.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct LanAddr {
	/// Interface name such as `wlan0`.
	pub interface: String,
	/// The address.
	pub ip: IpAddr,
}

/// Lists usable non-loopback addresses (IPv4 first, link-local IPv6 omitted).
pub async fn lan_addrs() -> Vec<LanAddr> {
	tokio::task::spawn_blocking(|| {
		let mut v: Vec<LanAddr> = local_ip_address::list_afinet_netifas()
			.unwrap_or_default()
			.into_iter()
			.filter(|(_, ip)| !ip.is_loopback() && !ip.is_unspecified() && !is_link_local(ip))
			.map(|(interface, ip)| LanAddr { interface, ip })
			.collect();
		v.sort_by_key(|a| (a.ip.is_ipv6(), a.interface.clone()));
		v
	})
	.await
	.unwrap_or_default()
}

fn is_link_local(ip: &IpAddr) -> bool {
	match ip {
		IpAddr::V4(v) => v.is_link_local(),
		IpAddr::V6(v) => (v.segments()[0] & 0xffc0) == 0xfe80,
	}
}

/// The IPv4 default gateway, when one can be found.
#[cfg(feature = "portmap")]
pub async fn default_gateway() -> Option<Ipv4Addr> {
	tokio::task::spawn_blocking(|| {
		let gw = netdev::get_default_gateway().ok()?;
		gw.ipv4.into_iter().next()
	})
	.await
	.ok()
	.flatten()
}

/// The IPv4 default gateway, when one can be found.
#[cfg(not(feature = "portmap"))]
pub async fn default_gateway() -> Option<Ipv4Addr> {
	None
}

/// Whether the address is private, loopback, link-local or carrier-grade NAT space.
pub fn is_private_or_cgnat(ip: &IpAddr) -> bool {
	match ip {
		IpAddr::V4(v) => v.is_private() || v.is_loopback() || v.is_link_local() || is_cgnat(v),
		IpAddr::V6(v) => v.is_loopback() || (v.segments()[0] & 0xfe00) == 0xfc00,
	}
}

/// Whether the address is in 100.64.0.0/10.
pub fn is_cgnat(ip: &Ipv4Addr) -> bool {
	let o = ip.octets();
	o[0] == 100 && (o[1] & 0xc0) == 0x40
}

/// Asks a public service for this machine's internet-facing address. One request, only
/// when the caller explicitly invokes it.
#[cfg(feature = "client")]
pub async fn lookup_public_ip() -> Result<IpAddr> {
	#[cfg(feature = "tls")]
	crate::tls::ensure_default_provider();
	let client = reqwest::Client::builder()
		.timeout(std::time::Duration::from_secs(8))
		.build()
		.map_err(|e| Error::Send(e.to_string()))?;
	let text = client
		.get("https://api.ipify.org")
		.send()
		.await
		.map_err(|e| Error::Send(e.to_string()))?
		.text()
		.await
		.map_err(|e| Error::Send(e.to_string()))?;
	text.trim()
		.parse()
		.map_err(|_| Error::Send("unexpected reply from the lookup service".into()))
}

/// Asks a public service for this machine's internet-facing address.
#[cfg(not(feature = "client"))]
pub async fn lookup_public_ip() -> Result<IpAddr> {
	Err(Error::FeatureDisabled("client"))
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn classifies_ranges() {
		assert!(is_private_or_cgnat(&"192.168.0.1".parse().unwrap()));
		assert!(is_private_or_cgnat(&"100.64.1.1".parse().unwrap()));
		assert!(is_private_or_cgnat(&"100.127.255.255".parse().unwrap()));
		assert!(!is_private_or_cgnat(&"100.128.0.1".parse().unwrap()));
		assert!(!is_private_or_cgnat(&"8.8.8.8".parse().unwrap()));
	}
}
