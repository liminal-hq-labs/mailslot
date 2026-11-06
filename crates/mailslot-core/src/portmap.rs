// Router port mapping: UPnP-IGD first, then PCP and NAT-PMP, always with a lease.
//
// (c) Copyright 2026 Liminal HQ, Scott Morris
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Opens a TCP port on the router so a share is reachable from the internet.
//!
//! Mappings always carry a lease (one hour by default, never permanent). Callers renew
//! them while sharing and call [`ActiveMapping::remove`] on stop. Opening is opt-in per
//! session; see the threat model.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::num::NonZeroU16;
use std::time::Duration;

use igd_next::PortMappingProtocol;
use igd_next::aio::Gateway;
use igd_next::aio::tokio::Tokio;

use crate::events::{Diagnostic, DiagnosticKind};
use crate::netinfo;

/// Prefix of the description Mailslot writes on the router, used to find stale mappings.
pub const MAPPING_PREFIX: &str = "Mailslot";
const DEFAULT_LEASE: Duration = Duration::from_secs(3600);
const MIN_LEASE: Duration = Duration::from_secs(120);

/// Options for opening a mapping.
#[derive(Clone, Debug)]
pub struct PortMapOptions {
	/// The local TCP port to expose.
	pub local_port: u16,
	/// The LAN address to forward to; detected when `None`.
	pub local_ip: Option<IpAddr>,
	/// Requested lease (clamped to at least two minutes). One hour by default.
	pub lease: Duration,
	/// The public address seen by an outside service (from the opt-in lookup), used to
	/// detect NAT in front of the router.
	pub public_ip: Option<IpAddr>,
	/// Where to send the SSDP search. `None` uses the standard multicast address.
	pub ssdp_addr: Option<SocketAddr>,
	/// Local address to bind the SSDP socket to.
	pub ssdp_bind: Option<SocketAddr>,
	/// Fall back to PCP and NAT-PMP when UPnP finds nothing.
	pub fallback_pmp: bool,
	/// How long to wait for a UPnP gateway.
	pub search_timeout: Duration,
}

impl Default for PortMapOptions {
	fn default() -> Self {
		Self {
			local_port: 0,
			local_ip: None,
			lease: DEFAULT_LEASE,
			public_ip: None,
			ssdp_addr: None,
			ssdp_bind: None,
			fallback_pmp: true,
			search_timeout: Duration::from_secs(4),
		}
	}
}

/// Which protocol created the mapping.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Protocol {
	/// UPnP Internet Gateway Device.
	Upnp,
	/// Port Control Protocol.
	Pcp,
	/// NAT Port Mapping Protocol.
	NatPmp,
}

enum Backend {
	Upnp {
		gateway: Box<Gateway<Tokio>>,
		local: SocketAddr,
		external_port: u16,
	},
	Crab(Box<crab_nat::PortMapping>),
}

/// An open mapping.
pub struct ActiveMapping {
	backend: Backend,
	external: SocketAddr,
	protocol: Protocol,
	lease: Duration,
	warnings: Vec<Diagnostic>,
}

impl std::fmt::Debug for ActiveMapping {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		f.debug_struct("ActiveMapping")
			.field("external", &self.external)
			.field("protocol", &self.protocol)
			.finish()
	}
}

fn diag(kind: DiagnosticKind, message: impl Into<String>) -> Diagnostic {
	Diagnostic {
		kind,
		message: message.into(),
	}
}

fn clamp_lease(l: Duration) -> Duration {
	l.max(MIN_LEASE)
}

fn description() -> String {
	format!("{MAPPING_PREFIX} ({})", crate::config::APP_ID)
}

impl ActiveMapping {
	/// The address a sender on the internet would use.
	pub fn external(&self) -> SocketAddr {
		self.external
	}

	/// Which protocol made the mapping.
	pub fn protocol(&self) -> Protocol {
		self.protocol
	}

	/// The lease granted.
	pub fn lease(&self) -> Duration {
		self.lease
	}

	/// Problems that did not stop the mapping but mean it may not work (double NAT, CGNAT).
	pub fn warnings(&self) -> &[Diagnostic] {
		&self.warnings
	}

	/// Renews the lease. Call about halfway through it.
	pub async fn renew(&mut self) -> Result<(), Diagnostic> {
		let lease = self.lease;
		match &mut self.backend {
			Backend::Upnp {
				gateway,
				local,
				external_port,
			} => gateway
				.add_port(
					PortMappingProtocol::TCP,
					*external_port,
					*local,
					lease.as_secs() as u32,
					&description(),
				)
				.await
				.map_err(|e| {
					diag(
						DiagnosticKind::Other,
						format!("The router would not renew the mapping: {e}"),
					)
				}),
			Backend::Crab(m) => m.renew().await.map_err(|e| {
				diag(
					DiagnosticKind::Other,
					format!("The router would not renew the mapping: {e}"),
				)
			}),
		}
	}

	/// Removes the mapping from the router.
	pub async fn remove(self) -> Result<(), Diagnostic> {
		match self.backend {
			Backend::Upnp {
				gateway,
				external_port,
				..
			} => gateway
				.remove_port(PortMappingProtocol::TCP, external_port)
				.await
				.map_err(|e| {
					diag(
						DiagnosticKind::Other,
						format!("Could not remove the router mapping: {e}"),
					)
				}),
			Backend::Crab(m) => m.try_drop().await.map_err(|(e, _)| {
				diag(
					DiagnosticKind::Other,
					format!("Could not remove the router mapping: {e}"),
				)
			}),
		}
	}
}

fn search_options(opts: &PortMapOptions) -> igd_next::SearchOptions {
	let mut s = igd_next::SearchOptions::default();
	s.timeout = Some(opts.search_timeout);
	s.single_search_timeout = Some(opts.search_timeout);
	if let Some(a) = opts.ssdp_addr {
		s.broadcast_address = a;
		s.bind_addr = opts
			.ssdp_bind
			.unwrap_or_else(|| SocketAddr::new(Ipv4Addr::LOCALHOST.into(), 0));
		s.allowed_gateway_ips = vec![a.ip()];
	} else if let Some(b) = opts.ssdp_bind {
		s.bind_addr = b;
	}
	s
}

async fn lan_ip(opts: &PortMapOptions) -> Option<IpAddr> {
	if let Some(ip) = opts.local_ip {
		return Some(ip);
	}
	netinfo::lan_addrs()
		.await
		.into_iter()
		.find(|a| a.ip.is_ipv4())
		.map(|a| a.ip)
}

/// Removes mappings an earlier Mailslot run left on the router (matched by description
/// and by forwarding to this machine). Returns the external ports that were freed.
pub async fn cleanup_stale(opts: &PortMapOptions) -> Vec<u16> {
	let Some(local_ip) = lan_ip(opts).await else {
		return Vec::new();
	};
	let Ok(gateway) = igd_next::aio::tokio::search_gateway(search_options(opts)).await else {
		return Vec::new();
	};
	let mut stale = Vec::new();
	for index in 0..256u32 {
		match gateway.get_generic_port_mapping_entry(index).await {
			Ok(e) => {
				if e.port_mapping_description.starts_with(MAPPING_PREFIX)
					&& e.internal_client == local_ip.to_string()
					&& e.protocol == PortMappingProtocol::TCP
				{
					stale.push(e.external_port);
				}
			}
			Err(_) => break,
		}
	}
	let mut removed = Vec::new();
	for port in stale {
		if gateway
			.remove_port(PortMappingProtocol::TCP, port)
			.await
			.is_ok()
		{
			removed.push(port);
		}
	}
	removed
}

fn nat_warnings(external_ip: IpAddr, public_ip: Option<IpAddr>) -> Vec<Diagnostic> {
	let mut w = Vec::new();
	match external_ip {
		IpAddr::V4(v4) if netinfo::is_cgnat(&v4) => w.push(diag(
			DiagnosticKind::Cgnat,
			"Your internet provider shares one public address between customers (carrier-grade NAT), so the port cannot be reached from outside. Ask your provider for a public address, or use a tunnel.",
		)),
		ip if netinfo::is_private_or_cgnat(&ip) => w.push(diag(
			DiagnosticKind::DoubleNat,
			"The router's internet address is a private one, which suggests another router sits in front of it (double NAT). The port will not be reachable from outside until that router also forwards it.",
		)),
		_ => {}
	}
	if let Some(public) = public_ip
		&& public != external_ip
		&& w.is_empty()
	{
		w.push(diag(
				DiagnosticKind::AddressMismatch,
				"The address your router reports differs from the one the internet sees, so another layer of NAT is likely in the way.",
			));
	}
	w
}

/// Opens a mapping. On failure the diagnostic explains why in plain words.
pub async fn open(opts: &PortMapOptions) -> Result<ActiveMapping, Diagnostic> {
	if opts.local_port == 0 {
		return Err(diag(DiagnosticKind::Other, "There is no port to open yet."));
	}
	let Some(local_ip) = lan_ip(opts).await else {
		return Err(diag(
			DiagnosticKind::NoGateway,
			"No local network address was found, so the router cannot be asked to forward a port.",
		));
	};
	let lease = clamp_lease(opts.lease);
	let local = SocketAddr::new(local_ip, opts.local_port);

	let upnp_error = match igd_next::aio::tokio::search_gateway(search_options(opts)).await {
		Ok(gateway) => match open_upnp(gateway, local, lease, opts).await {
			Ok(m) => return Ok(m),
			Err(d) => d,
		},
		Err(_) => diag(
			DiagnosticKind::NoGateway,
			"No router answered. UPnP may be switched off on the router, or this network has no router that supports it. Use the manual port-forward instructions instead.",
		),
	};

	if opts.fallback_pmp
		&& opts.ssdp_addr.is_none()
		&& let Some(m) = open_pmp(local, lease, opts).await
	{
		return Ok(m);
	}
	Err(upnp_error)
}

async fn open_upnp(
	gateway: Gateway<Tokio>,
	local: SocketAddr,
	lease: Duration,
	opts: &PortMapOptions,
) -> Result<ActiveMapping, Diagnostic> {
	let external_ip = gateway.get_external_ip().await.map_err(|e| {
		diag(
			DiagnosticKind::MappingRefused,
			format!("The router answered but would not tell us its internet address: {e}"),
		)
	})?;
	let secs = lease.as_secs() as u32;
	let desc = description();
	let external_port = match gateway
		.add_port(PortMappingProtocol::TCP, local.port(), local, secs, &desc)
		.await
	{
		Ok(()) => local.port(),
		Err(igd_next::AddPortError::PortInUse) => gateway
			.add_any_port(PortMappingProtocol::TCP, local, secs, &desc)
			.await
			.map_err(|e| {
				diag(
					DiagnosticKind::PortConflict,
					format!(
						"Port {} is already forwarded to another device and no other port could be used: {e}",
						local.port()
					),
				)
			})?,
		Err(igd_next::AddPortError::OnlyPermanentLeasesSupported) => {
			return Err(diag(
				DiagnosticKind::MappingRefused,
				"The router only supports permanent mappings. Mailslot never creates those; use the manual port-forward instructions.",
			));
		}
		Err(igd_next::AddPortError::SamePortValuesRequired) => {
			return Err(diag(
				DiagnosticKind::PortConflict,
				"The router needs the same internal and external port, and that port is not free.",
			));
		}
		Err(e) => {
			return Err(diag(
				DiagnosticKind::MappingRefused,
				format!(
					"The router refused the mapping ({e}). UPnP port mapping may be disabled in its settings."
				),
			));
		}
	};
	Ok(ActiveMapping {
		external: SocketAddr::new(external_ip, external_port),
		protocol: Protocol::Upnp,
		lease,
		warnings: nat_warnings(external_ip, opts.public_ip),
		backend: Backend::Upnp {
			gateway: Box::new(gateway),
			local,
			external_port,
		},
	})
}

async fn open_pmp(
	local: SocketAddr,
	lease: Duration,
	opts: &PortMapOptions,
) -> Option<ActiveMapping> {
	let gw = netinfo::default_gateway().await?;
	let internal = NonZeroU16::new(local.port())?;
	let options = crab_nat::PortMappingOptions {
		external_port: Some(internal),
		lifetime_seconds: Some(lease.as_secs() as u32),
		timeout_config: None,
	};
	let mapping = crab_nat::PortMapping::new(
		gw.into(),
		local.ip(),
		crab_nat::InternetProtocol::Tcp,
		internal,
		options,
	)
	.await
	.ok()?;
	// The external address is reported by PCP; NAT-PMP needs a separate query that
	// this library does not expose, so the opt-in public address stands in.
	let (protocol, external_ip) = match mapping.mapping_type() {
		crab_nat::PortMappingType::Pcp { external_ip, .. } => (Protocol::Pcp, Some(external_ip)),
		crab_nat::PortMappingType::NatPmp => (Protocol::NatPmp, opts.public_ip),
	};
	let ip = external_ip.unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED));
	let warnings = if ip.is_unspecified() {
		Vec::new()
	} else {
		nat_warnings(ip, opts.public_ip)
	};
	Some(ActiveMapping {
		external: SocketAddr::new(ip, mapping.external_port().get()),
		protocol,
		lease: Duration::from_secs(u64::from(mapping.lifetime())),
		warnings,
		backend: Backend::Crab(Box::new(mapping)),
	})
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn warnings_classify_private_and_cgnat() {
		assert_eq!(
			nat_warnings("100.64.0.9".parse().unwrap(), None)[0].kind,
			DiagnosticKind::Cgnat
		);
		assert_eq!(
			nat_warnings("192.168.1.2".parse().unwrap(), None)[0].kind,
			DiagnosticKind::DoubleNat
		);
		assert!(
			nat_warnings(
				"203.0.113.5".parse().unwrap(),
				Some("203.0.113.5".parse().unwrap())
			)
			.is_empty()
		);
		assert_eq!(
			nat_warnings(
				"203.0.113.5".parse().unwrap(),
				Some("198.51.100.1".parse().unwrap())
			)[0]
			.kind,
			DiagnosticKind::AddressMismatch
		);
	}

	#[test]
	fn lease_is_never_permanent() {
		assert!(clamp_lease(Duration::ZERO) >= MIN_LEASE);
	}
}
