// Local network discovery: LAN addresses, default gateway and an opt-in public-IP lookup.
//
// (c) Copyright 2026 Liminal HQ, Scott Morris
// SPDX-License-Identifier: Apache-2.0 OR MIT

use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use serde::Serialize;

use crate::error::{Error, Result};

/// One local address.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct LanAddr {
	/// Interface name such as `wlan0`.
	pub interface: String,
	/// The address.
	pub ip: IpAddr,
	/// The interface belongs to a container, virtual machine or VPN, so other devices on the
	/// local network usually cannot reach this address.
	pub is_virtual: bool,
	/// The operating system sends traffic for the internet from this address.
	pub is_default_route: bool,
}

impl LanAddr {
	/// A short name for the kind of virtual interface, or `None` for a physical one.
	pub fn virtual_kind(&self) -> Option<&'static str> {
		virtual_kind(&self.interface)
	}
}

/// Interface-name prefixes of virtual interfaces, with a plain-words kind for display.
const VIRTUAL_PREFIXES: &[(&str, &str)] = &[
	("docker", "Docker"),
	("br-", "Docker"),
	("veth", "container"),
	("virbr", "virtual machine"),
	("vmnet", "virtual machine"),
	("vboxnet", "virtual machine"),
	("lxc", "container"),
	("lxd", "container"),
	("cni", "container"),
	("flannel", "container"),
	("cali", "container"),
	("podman", "container"),
	("tun", "VPN"),
	("tap", "VPN"),
	("wg", "VPN"),
	("tailscale", "VPN"),
	("zt", "VPN"),
];

/// Classifies an interface name, returning the kind of virtual interface it looks like.
pub fn virtual_kind(interface: &str) -> Option<&'static str> {
	let name = interface.to_ascii_lowercase();
	VIRTUAL_PREFIXES
		.iter()
		.find(|(prefix, _)| name.starts_with(prefix))
		.map(|(_, kind)| *kind)
}

/// Ranks candidate addresses, best first.
///
/// The address on the default route comes first, then private addresses on physical
/// interfaces, then other physical addresses, and virtual interfaces last. IPv4 precedes
/// IPv6, and ties break by interface name so the order is stable. `default_ip` is the
/// address the system uses to reach the internet, when known.
pub fn rank_addrs(candidates: Vec<(String, IpAddr)>, default_ip: Option<IpAddr>) -> Vec<LanAddr> {
	let mut v: Vec<LanAddr> = candidates
		.into_iter()
		.filter(|(_, ip)| !ip.is_loopback() && !ip.is_unspecified() && !is_link_local(ip))
		.map(|(interface, ip)| LanAddr {
			is_virtual: virtual_kind(&interface).is_some(),
			is_default_route: default_ip == Some(ip),
			interface,
			ip,
		})
		.collect();
	v.sort_by_key(|a| {
		let tier = if a.is_default_route {
			0
		} else if a.is_virtual {
			3
		} else if is_private_or_cgnat(&a.ip) {
			1
		} else {
			2
		};
		(a.ip.is_ipv6(), tier, a.interface.clone())
	});
	v
}

/// The local address the operating system would use to reach `target`.
///
/// Connecting a UDP socket only selects a route and sends nothing, so this is safe to call
/// for any address, including the router's.
pub async fn local_addr_towards(target: SocketAddr) -> Option<IpAddr> {
	let bind: SocketAddr = if target.is_ipv4() {
		(Ipv4Addr::UNSPECIFIED, 0).into()
	} else {
		(std::net::Ipv6Addr::UNSPECIFIED, 0).into()
	};
	let socket = tokio::net::UdpSocket::bind(bind).await.ok()?;
	socket.connect(target).await.ok()?;
	let ip = socket.local_addr().ok()?.ip();
	(!ip.is_unspecified()).then_some(ip)
}

/// The address the system uses to reach the internet. No packet is sent.
pub async fn default_route_ip() -> Option<IpAddr> {
	// A documentation-range address: routed like any public one, never contacted.
	local_addr_towards((Ipv4Addr::new(192, 0, 2, 1), 9).into()).await
}

/// Lists usable non-loopback addresses ranked best first (see [`rank_addrs`]); link-local
/// IPv6 is omitted.
pub async fn lan_addrs() -> Vec<LanAddr> {
	let default_ip = default_route_ip().await;
	let found =
		tokio::task::spawn_blocking(|| local_ip_address::list_afinet_netifas().unwrap_or_default())
			.await
			.unwrap_or_default();
	let ranked = rank_addrs(found, default_ip);
	tracing::debug!(
		default_route = ?default_ip,
		candidates = ?ranked.iter().map(|a| format!("{} {}", a.interface, a.ip)).collect::<Vec<_>>(),
		"ranked local addresses"
	);
	ranked
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

	fn owner() -> Vec<(String, IpAddr)> {
		[
			("br-ee4fb9c67e7c", "172.20.0.1"),
			("br-7570d573c24f", "172.18.0.1"),
			("br-1d5f17f039c0", "172.19.0.1"),
			("docker0", "172.17.0.1"),
			("virbr1", "192.168.106.1"),
			("enp16s0u4", "192.168.1.100"),
			("lo", "127.0.0.1"),
		]
		.into_iter()
		.map(|(n, ip)| (n.to_owned(), ip.parse().unwrap()))
		.collect()
	}

	#[test]
	fn default_route_address_ranks_first_and_virtual_last() {
		let r = rank_addrs(owner(), Some("192.168.1.100".parse().unwrap()));
		assert_eq!(r[0].interface, "enp16s0u4");
		assert!(r[0].is_default_route && !r[0].is_virtual);
		assert!(r[1..].iter().all(|a| a.is_virtual && !a.is_default_route));
		assert_eq!(r.len(), 6, "loopback is dropped");
	}

	#[test]
	fn physical_beats_virtual_without_a_default_route() {
		let r = rank_addrs(owner(), None);
		assert_eq!(r[0].interface, "enp16s0u4");
		assert!(!r[0].is_default_route);
		assert_eq!(r[1].interface, "br-1d5f17f039c0");
	}

	#[test]
	fn default_route_wins_even_when_the_name_looks_virtual() {
		let r = rank_addrs(
			vec![
				("wlan0".into(), "192.168.1.5".parse().unwrap()),
				("wg0".into(), "10.8.0.2".parse().unwrap()),
			],
			Some("10.8.0.2".parse().unwrap()),
		);
		assert_eq!(r[0].interface, "wg0");
		assert!(r[0].is_virtual && r[0].is_default_route);
	}

	#[test]
	fn ipv4_precedes_ipv6_and_link_local_is_dropped() {
		let r = rank_addrs(
			vec![
				("eth0".into(), "2001:db8::2".parse().unwrap()),
				("eth0".into(), "fe80::1".parse().unwrap()),
				("eth0".into(), "192.168.1.2".parse().unwrap()),
			],
			None,
		);
		assert_eq!(r.len(), 2);
		assert!(r[0].ip.is_ipv4());
	}

	#[test]
	fn names_the_kind_of_virtual_interface() {
		assert_eq!(virtual_kind("br-1d5f17f039c0"), Some("Docker"));
		assert_eq!(virtual_kind("virbr0"), Some("virtual machine"));
		assert_eq!(virtual_kind("tailscale0"), Some("VPN"));
		assert_eq!(virtual_kind("enp16s0u4"), None);
		assert_eq!(virtual_kind("wlan0"), None);
	}

	#[tokio::test]
	async fn route_lookup_towards_loopback_is_loopback() {
		let ip = local_addr_towards("127.0.0.1:1900".parse().unwrap()).await;
		assert_eq!(ip, Some(IpAddr::V4(Ipv4Addr::LOCALHOST)));
	}
}
