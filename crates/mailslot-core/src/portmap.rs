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
use std::time::{Duration, Instant};

use igd_next::PortMappingProtocol;
use igd_next::aio::Gateway;
use igd_next::aio::tokio::Tokio;

use crate::events::{Diagnostic, DiagnosticKind, InternetDetails};
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
	/// The LAN address to forward to. When `None`, the address the system uses to reach the
	/// router is used.
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
	details: InternetDetails,
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
		details: None,
	}
}

fn diag_with(
	kind: DiagnosticKind,
	message: impl Into<String>,
	details: &InternetDetails,
) -> Diagnostic {
	Diagnostic {
		details: Some(Box::new(details.clone())),
		..diag(kind, message)
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

	/// The router and address facts behind this mapping.
	pub fn details(&self) -> &InternetDetails {
		&self.details
	}

	/// Renews the lease. Call about halfway through it.
	pub async fn renew(&mut self) -> Result<(), Diagnostic> {
		let lease = self.lease;
		let started = Instant::now();
		let result = match &mut self.backend {
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
		};
		match &result {
			Ok(()) => tracing::info!(
				protocol = ?self.protocol,
				lease_secs = lease.as_secs(),
				elapsed_ms = started.elapsed().as_millis() as u64,
				"renewed router port mapping"
			),
			Err(d) => tracing::warn!(error = %d.message, "router port mapping renewal failed"),
		}
		result
	}

	/// Removes the mapping from the router.
	pub async fn remove(self) -> Result<(), Diagnostic> {
		let (protocol, external) = (self.protocol, self.external);
		let result = match self.backend {
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
		};
		match &result {
			Ok(()) => tracing::info!(
				?protocol,
				port = external.port(),
				"removed router port mapping"
			),
			Err(d) => {
				tracing::warn!(?protocol, error = %d.message, "could not remove router port mapping")
			}
		}
		result
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

/// The address the router should forward to: the override when given, otherwise the address
/// the operating system uses to reach the router, never a guess from a list of interfaces.
async fn local_ip_for(opts: &PortMapOptions, gateway: IpAddr) -> Option<IpAddr> {
	if let Some(ip) = opts.local_ip {
		tracing::debug!(%ip, "using the configured local address for the mapping");
		return Some(ip);
	}
	let ip = netinfo::local_addr_towards(SocketAddr::new(gateway, 1900)).await;
	tracing::debug!(%gateway, local = ?ip, "address the system uses to reach the router");
	ip
}

/// Whether `local` shares a subnet with `gateway`, when that can be told. The prefix comes
/// from the interface that owns `local`, falling back to /24 for an unknown address.
async fn on_router_network(local: IpAddr, gateway: IpAddr) -> Option<bool> {
	let (IpAddr::V4(local), IpAddr::V4(gateway)) = (local, gateway) else {
		return None;
	};
	let prefix = tokio::task::spawn_blocking(move || {
		netdev::get_interfaces()
			.into_iter()
			.flat_map(|i| i.ipv4)
			.find(|n| n.addr() == local)
			.map(|n| n.prefix_len())
	})
	.await
	.ok()
	.flatten()
	.unwrap_or(24)
	.min(32);
	let mask = u32::MAX.checked_shl(32 - u32::from(prefix)).unwrap_or(0);
	Some(u32::from(local) & mask == u32::from(gateway) & mask)
}

async fn interface_of(ip: IpAddr) -> Option<String> {
	netinfo::lan_addrs()
		.await
		.into_iter()
		.find(|a| a.ip == ip)
		.map(|a| a.interface)
}

/// Reads the manufacturer and model from the router's device description. Best effort, with a
/// short timeout; the answer only improves diagnostics.
async fn describe_router(gateway: &Gateway<Tokio>) -> Option<String> {
	use tokio::io::{AsyncReadExt, AsyncWriteExt};
	let path = if gateway.root_url.starts_with('/') {
		gateway.root_url.clone()
	} else if let Ok(u) = url::Url::parse(&gateway.root_url) {
		u.path().to_owned()
	} else {
		return None;
	};
	let fetch = async {
		let mut stream = tokio::net::TcpStream::connect(gateway.addr).await.ok()?;
		let request = format!(
			"GET {path} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\nAccept: text/xml\r\n\r\n",
			gateway.addr
		);
		stream.write_all(request.as_bytes()).await.ok()?;
		let mut body = Vec::new();
		stream.take(64 * 1024).read_to_end(&mut body).await.ok()?;
		Some(String::from_utf8_lossy(&body).into_owned())
	};
	let text = tokio::time::timeout(Duration::from_secs(2), fetch)
		.await
		.ok()??;
	let make = xml_tag(&text, "manufacturer");
	let model = xml_tag(&text, "modelName").or_else(|| xml_tag(&text, "friendlyName"));
	let number = xml_tag(&text, "modelNumber");
	let parts: Vec<String> = [make, model, number].into_iter().flatten().collect();
	(!parts.is_empty()).then(|| parts.join(" "))
}

fn xml_tag(text: &str, name: &str) -> Option<String> {
	let open = format!("<{name}>");
	let start = text.find(&open)? + open.len();
	let end = text[start..].find(&format!("</{name}>"))?;
	let v = text[start..start + end].trim();
	(!v.is_empty()).then(|| v.chars().take(80).collect())
}

/// The raw UPnP error code and text behind an `AddPortMapping` failure, when there is one.
fn add_error_code(e: &igd_next::AddPortError) -> (Option<u16>, String) {
	use igd_next::AddPortError as E;
	match e {
		E::ActionNotAuthorized => (Some(606), "Action not authorized".into()),
		E::DescriptionTooLong => (Some(605), "String argument too long".into()),
		E::PortInUse => (Some(718), "ConflictInMappingEntry".into()),
		E::SamePortValuesRequired => (Some(724), "SamePortValuesRequired".into()),
		E::OnlyPermanentLeasesSupported => (Some(725), "OnlyPermanentLeasesSupported".into()),
		E::RequestError(igd_next::RequestError::ErrorCode(c, t)) => (Some(*c), t.clone()),
		other => (None, other.to_string()),
	}
}

fn add_any_error_code(e: &igd_next::AddAnyPortError) -> (Option<u16>, String) {
	use igd_next::AddAnyPortError as E;
	match e {
		E::ActionNotAuthorized => (Some(606), "Action not authorized".into()),
		E::NoPortsAvailable => (Some(728), "NoPortsAvailable".into()),
		E::ExternalPortInUse => (Some(718), "ConflictInMappingEntry".into()),
		E::RequestError(igd_next::RequestError::ErrorCode(c, t)) => (Some(*c), t.clone()),
		other => (None, other.to_string()),
	}
}

fn format_upnp_error(code: Option<u16>, text: &str) -> String {
	match code {
		Some(c) => format!("UPnP error {c}: {text}"),
		None => format!("UPnP error: {text}"),
	}
}

/// The explanation for a refused `AddPortMapping`, naming the address that was asked for.
fn refusal_message(
	code: Option<u16>,
	text: &str,
	local: SocketAddr,
	gateway: IpAddr,
	on_network: Option<bool>,
) -> String {
	let head = format_upnp_error(code, text);
	if code == Some(606) {
		let mut m = format!(
			"{head}. The router at {gateway} would not forward port {} to {}. Routers running MiniUPnPd, including UniFi, only forward to the address that makes the request, so check that UPnP is enabled for this network and that the request came from this machine's own address.",
			local.port(),
			local.ip()
		);
		if on_network == Some(false) {
			m.push_str(&format!(
				" {} is not on the router's network, so the router will refuse to forward to it.",
				local.ip()
			));
		}
		m
	} else {
		format!(
			"{head}. The router refused the mapping; UPnP port mapping may be disabled in its settings."
		)
	}
}

/// Removes mappings an earlier Mailslot run left on the router (matched by description
/// and by forwarding to this machine). Returns the external ports that were freed.
pub async fn cleanup_stale(opts: &PortMapOptions) -> Vec<u16> {
	let Ok(gateway) = igd_next::aio::tokio::search_gateway(search_options(opts)).await else {
		tracing::debug!("stale mapping cleanup skipped: no UPnP gateway answered");
		return Vec::new();
	};
	let Some(local_ip) = local_ip_for(opts, gateway.addr.ip()).await else {
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
	tracing::info!(client = %local_ip, ?removed, "stale mapping cleanup finished");
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
	for d in &w {
		tracing::warn!(kind = ?d.kind, %external_ip, "{}", d.message);
	}
	w
}

/// Opens a mapping. On failure the diagnostic explains why in plain words.
pub async fn open(opts: &PortMapOptions) -> Result<ActiveMapping, Diagnostic> {
	let started = Instant::now();
	if opts.local_port == 0 {
		return Err(diag(DiagnosticKind::Other, "There is no port to open yet."));
	}
	let lease = clamp_lease(opts.lease);
	tracing::info!(
		port = opts.local_port,
		lease_secs = lease.as_secs(),
		"asking the router to open a port"
	);

	let searched = Instant::now();
	let upnp_error = match igd_next::aio::tokio::search_gateway(search_options(opts)).await {
		Ok(gateway) => {
			let router = describe_router(&gateway).await;
			tracing::info!(
				gateway = %gateway.addr,
				location = %gateway.root_url,
				router = router.as_deref().unwrap_or("unknown"),
				elapsed_ms = searched.elapsed().as_millis() as u64,
				"found a UPnP gateway"
			);
			let gateway_ip = gateway.addr.ip();
			let mut details = InternetDetails {
				gateway: Some(gateway_ip),
				router,
				..Default::default()
			};
			match local_ip_for(opts, gateway_ip).await {
				Some(local_ip) => {
					details.internal_client = Some(local_ip);
					details.interface = interface_of(local_ip).await;
					let local = SocketAddr::new(local_ip, opts.local_port);
					match open_upnp(gateway, local, lease, opts, details).await {
						Ok(m) => {
							tracing::info!(
								external = %m.external(),
								elapsed_ms = started.elapsed().as_millis() as u64,
								"router port mapping is open"
							);
							return Ok(m);
						}
						Err(d) => d,
					}
				}
				None => diag_with(
					DiagnosticKind::NoGateway,
					"No local network address could be used to reach the router, so it cannot be asked to forward a port.",
					&details,
				),
			}
		}
		Err(e) => {
			tracing::warn!(
				error = %e,
				elapsed_ms = searched.elapsed().as_millis() as u64,
				"no UPnP gateway answered"
			);
			diag(
				DiagnosticKind::NoGateway,
				"No router answered. UPnP may be switched off on the router, or this network has no router that supports it. Use the manual port-forward instructions instead.",
			)
		}
	};
	tracing::warn!(kind = ?upnp_error.kind, error = %upnp_error.message, "UPnP mapping failed");

	if opts.fallback_pmp && opts.ssdp_addr.is_none() {
		let gw = netinfo::default_gateway().await;
		let local_ip = match (opts.local_ip, gw) {
			(Some(ip), _) => Some(ip),
			(None, Some(gw)) => local_ip_for(opts, gw.into()).await,
			(None, None) => netinfo::default_route_ip().await,
		};
		if let Some(local_ip) = local_ip {
			let local = SocketAddr::new(local_ip, opts.local_port);
			if let Some(m) = open_pmp(local, lease, opts).await {
				tracing::info!(protocol = ?m.protocol(), external = %m.external(), "mapped through PCP or NAT-PMP");
				return Ok(m);
			}
			tracing::debug!("PCP and NAT-PMP did not answer");
		}
	}
	Err(upnp_error)
}

async fn open_upnp(
	gateway: Gateway<Tokio>,
	local: SocketAddr,
	lease: Duration,
	opts: &PortMapOptions,
	mut details: InternetDetails,
) -> Result<ActiveMapping, Diagnostic> {
	let gateway_ip = gateway.addr.ip();
	let on_network = on_router_network(local.ip(), gateway_ip).await;
	if on_network == Some(false) {
		tracing::warn!(
			local = %local.ip(),
			gateway = %gateway_ip,
			"the local address is not on the router's network"
		);
	}
	let asked = Instant::now();
	let external_ip = match gateway.get_external_ip().await {
		Ok(ip) => ip,
		Err(e) => {
			let (code, text) = match &e {
				igd_next::GetExternalIpError::ActionNotAuthorized => {
					(Some(606), "Action not authorized".to_owned())
				}
				igd_next::GetExternalIpError::RequestError(igd_next::RequestError::ErrorCode(
					c,
					t,
				)) => (Some(*c), t.clone()),
				other => (None, other.to_string()),
			};
			tracing::warn!(?code, %text, "GetExternalIPAddress failed");
			details.error_code = code;
			details.error_text = Some(text.clone());
			return Err(diag_with(
				DiagnosticKind::MappingRefused,
				format!(
					"{}. The router answered but would not tell us its internet address.",
					format_upnp_error(code, &text)
				),
				&details,
			));
		}
	};
	tracing::info!(
		%external_ip,
		elapsed_ms = asked.elapsed().as_millis() as u64,
		"GetExternalIPAddress answered"
	);
	let secs = lease.as_secs() as u32;
	let desc = description();
	tracing::info!(
		internal_client = %local.ip(),
		internal_port = local.port(),
		external_port = local.port(),
		lease_secs = secs,
		"AddPortMapping request"
	);
	let added = Instant::now();
	let refused = |details: &mut InternetDetails, code: Option<u16>, text: String| {
		tracing::warn!(?code, %text, internal_client = %local.ip(), "AddPortMapping refused");
		details.error_code = code;
		details.error_text = Some(text.clone());
		diag_with(
			DiagnosticKind::MappingRefused,
			refusal_message(code, &text, local, gateway_ip, on_network),
			details,
		)
	};
	let external_port = match gateway
		.add_port(PortMappingProtocol::TCP, local.port(), local, secs, &desc)
		.await
	{
		Ok(()) => local.port(),
		Err(igd_next::AddPortError::PortInUse) => {
			tracing::info!(
				port = local.port(),
				"external port is in use; asking for another"
			);
			match gateway
				.add_any_port(PortMappingProtocol::TCP, local, secs, &desc)
				.await
			{
				Ok(p) => p,
				Err(e) => {
					let (code, text) = add_any_error_code(&e);
					if code == Some(606) {
						return Err(refused(&mut details, code, text));
					}
					details.error_code = code;
					details.error_text = Some(text.clone());
					return Err(diag_with(
						DiagnosticKind::PortConflict,
						format!(
							"{}. Port {} is already forwarded to another device and no other port could be used.",
							format_upnp_error(code, &text),
							local.port()
						),
						&details,
					));
				}
			}
		}
		Err(igd_next::AddPortError::OnlyPermanentLeasesSupported) => {
			let mut d = diag_with(
				DiagnosticKind::MappingRefused,
				"UPnP error 725: OnlyPermanentLeasesSupported. The router only supports permanent mappings. Mailslot never creates those; use the manual port-forward instructions.",
				&details,
			);
			if let Some(x) = d.details.as_mut() {
				x.error_code = Some(725);
				x.error_text = Some("OnlyPermanentLeasesSupported".into());
			}
			tracing::warn!("router only supports permanent mappings");
			return Err(d);
		}
		Err(igd_next::AddPortError::SamePortValuesRequired) => {
			tracing::warn!("router needs identical internal and external ports");
			return Err(diag_with(
				DiagnosticKind::PortConflict,
				"UPnP error 724: SamePortValuesRequired. The router needs the same internal and external port, and that port is not free.",
				&details,
			));
		}
		Err(e) => {
			let (code, text) = add_error_code(&e);
			return Err(refused(&mut details, code, text));
		}
	};
	tracing::info!(
		external_port,
		elapsed_ms = added.elapsed().as_millis() as u64,
		"AddPortMapping accepted"
	);
	Ok(ActiveMapping {
		external: SocketAddr::new(external_ip, external_port),
		protocol: Protocol::Upnp,
		lease,
		warnings: nat_warnings(external_ip, opts.public_ip),
		details,
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
	tracing::debug!(%gw, internal_client = %local.ip(), "trying PCP and NAT-PMP");
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
		details: InternetDetails {
			gateway: Some(gw.into()),
			internal_client: Some(local.ip()),
			..Default::default()
		},
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
