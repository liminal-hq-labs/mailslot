// An in-process fake UPnP Internet Gateway Device: SSDP responder plus SOAP control.
//
// (c) Copyright 2026 Liminal HQ, Scott Morris
// SPDX-License-Identifier: Apache-2.0 OR MIT

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, UdpSocket};
use tokio_util::sync::CancellationToken;

const SERVICE: &str = "urn:schemas-upnp-org:service:WANIPConnection:1";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mapping {
	pub external_port: u16,
	pub internal_client: String,
	pub internal_port: u16,
	pub description: String,
	pub lease: u32,
}

pub struct FakeIgd {
	pub ssdp_addr: SocketAddr,
	pub mappings: Arc<Mutex<BTreeMap<u16, Mapping>>>,
	pub external_ip: Arc<Mutex<String>>,
	pub refuse_adds: Arc<Mutex<bool>>,
	/// Behave like MiniUPnPd: refuse (606) a mapping whose internal client is not the
	/// address the request came from.
	pub require_own_address: Arc<Mutex<bool>>,
	/// Internal clients seen in `AddPortMapping` requests, refused or not.
	pub requested_clients: Arc<Mutex<Vec<String>>>,
	cancel: CancellationToken,
}

impl Drop for FakeIgd {
	fn drop(&mut self) {
		self.cancel.cancel();
	}
}

fn root_desc() -> String {
	format!(
		r#"<?xml version="1.0"?>
<root xmlns="urn:schemas-upnp-org:device-1-0"><specVersion><major>1</major><minor>0</minor></specVersion>
<device><deviceType>urn:schemas-upnp-org:device:InternetGatewayDevice:1</deviceType>
<friendlyName>Fake IGD</friendlyName>
<deviceList><device><deviceType>urn:schemas-upnp-org:device:WANDevice:1</deviceType>
<deviceList><device><deviceType>urn:schemas-upnp-org:device:WANConnectionDevice:1</deviceType>
<serviceList><service><serviceType>{SERVICE}</serviceType><serviceId>urn:upnp-org:serviceId:WANIPConn1</serviceId>
<SCPDURL>/scpd.xml</SCPDURL><controlURL>/control</controlURL><eventSubURL>/event</eventSubURL></service></serviceList>
</device></deviceList></device></deviceList></device></root>"#
	)
}

fn action(name: &str, args: &[&str]) -> String {
	let list: String = args
		.iter()
		.map(|a| format!("<argument><name>{a}</name><direction>in</direction></argument>"))
		.collect();
	format!("<action><name>{name}</name><argumentList>{list}</argumentList></action>")
}

fn scpd() -> String {
	let add = [
		"NewRemoteHost",
		"NewExternalPort",
		"NewProtocol",
		"NewInternalPort",
		"NewInternalClient",
		"NewEnabled",
		"NewPortMappingDescription",
		"NewLeaseDuration",
	];
	format!(
		r#"<?xml version="1.0"?><scpd xmlns="urn:schemas-upnp-org:service-1-0"><actionList>{}{}{}{}</actionList></scpd>"#,
		action("GetExternalIPAddress", &[]),
		action("AddPortMapping", &add),
		action(
			"DeletePortMapping",
			&["NewRemoteHost", "NewExternalPort", "NewProtocol"]
		),
		action("GetGenericPortMappingEntry", &["NewPortMappingIndex"]),
	)
}

fn envelope(body: &str) -> String {
	format!(
		r#"<?xml version="1.0"?><s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/" s:encodingStyle="http://schemas.xmlsoap.org/soap/encoding/"><s:Body>{body}</s:Body></s:Envelope>"#
	)
}

fn fault(code: u16, desc: &str) -> String {
	envelope(&format!(
		r#"<s:Fault><faultcode>s:Client</faultcode><faultstring>UPnPError</faultstring><detail><UPnPError xmlns="urn:schemas-upnp-org:control-1-0"><errorCode>{code}</errorCode><errorDescription>{desc}</errorDescription></UPnPError></detail></s:Fault>"#
	))
}

fn tag(body: &str, name: &str) -> String {
	let open = format!("<{name}>");
	let close = format!("</{name}>");
	body.find(&open)
		.and_then(|s| {
			body[s + open.len()..]
				.find(&close)
				.map(|e| body[s + open.len()..s + open.len() + e].to_owned())
		})
		.unwrap_or_default()
}

impl FakeIgd {
	pub async fn start(external_ip: &str) -> Self {
		let http = TcpListener::bind("127.0.0.1:0").await.unwrap();
		let http_addr = http.local_addr().unwrap();
		let udp = UdpSocket::bind("127.0.0.1:0").await.unwrap();
		let ssdp_addr = udp.local_addr().unwrap();
		let mappings = Arc::new(Mutex::new(BTreeMap::new()));
		let external_ip = Arc::new(Mutex::new(external_ip.to_owned()));
		let refuse_adds = Arc::new(Mutex::new(false));
		let require_own_address = Arc::new(Mutex::new(false));
		let requested_clients = Arc::new(Mutex::new(Vec::new()));
		let cancel = CancellationToken::new();

		let c = cancel.clone();
		tokio::spawn(async move {
			let mut buf = [0u8; 2048];
			loop {
				tokio::select! {
					_ = c.cancelled() => break,
					r = udp.recv_from(&mut buf) => {
						let Ok((n, from)) = r else { continue };
						if !String::from_utf8_lossy(&buf[..n]).starts_with("M-SEARCH") { continue; }
						let reply = format!("HTTP/1.1 200 OK\r\nCACHE-CONTROL: max-age=120\r\nST: urn:schemas-upnp-org:device:InternetGatewayDevice:1\r\nUSN: uuid:fake::urn:schemas-upnp-org:device:InternetGatewayDevice:1\r\nEXT:\r\nSERVER: fake/1.0 UPnP/1.1 fake/1.0\r\nLOCATION: http://{http_addr}/rootDesc.xml\r\n\r\n");
						let _ = udp.send_to(reply.as_bytes(), from).await;
					}
				}
			}
		});

		let (m, ip, refuse, c) = (
			mappings.clone(),
			external_ip.clone(),
			refuse_adds.clone(),
			cancel.clone(),
		);
		let (strict, seen) = (require_own_address.clone(), requested_clients.clone());
		tokio::spawn(async move {
			loop {
				let (mut s, peer) = tokio::select! { _ = c.cancelled() => break, r = http.accept() => match r { Ok(v) => v, Err(_) => continue } };
				let (m, ip, refuse) = (m.clone(), ip.clone(), refuse.clone());
				let (strict, seen) = (strict.clone(), seen.clone());
				tokio::spawn(async move {
					let mut data = Vec::new();
					let mut buf = [0u8; 4096];
					let (head_end, content_len) = loop {
						let n = s.read(&mut buf).await.unwrap_or(0);
						if n == 0 {
							return;
						}
						data.extend_from_slice(&buf[..n]);
						if let Some(p) = data.windows(4).position(|w| w == b"\r\n\r\n") {
							let head = String::from_utf8_lossy(&data[..p]).to_lowercase();
							let cl = head
								.lines()
								.find_map(|l| l.strip_prefix("content-length:"))
								.and_then(|v| v.trim().parse::<usize>().ok())
								.unwrap_or(0);
							break (p + 4, cl);
						}
					};
					while data.len() < head_end + content_len {
						let n = s.read(&mut buf).await.unwrap_or(0);
						if n == 0 {
							break;
						}
						data.extend_from_slice(&buf[..n]);
					}
					let text = String::from_utf8_lossy(&data).into_owned();
					let first = text.lines().next().unwrap_or_default().to_owned();
					let body = &text[head_end.min(text.len())..];
					let (status, ctype, out) = if first.starts_with("GET /rootDesc.xml") {
						("200 OK", "text/xml", root_desc())
					} else if first.starts_with("GET /scpd.xml") {
						("200 OK", "text/xml", scpd())
					} else if first.starts_with("POST /control") {
						let soap = soap_response(
							body,
							&m,
							&ip,
							&refuse,
							peer.ip().to_string(),
							&strict,
							&seen,
						);
						("200 OK", "text/xml", soap)
					} else {
						("404 Not Found", "text/plain", String::new())
					};
					let resp = format!(
						"HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{out}",
						out.len()
					);
					let _ = s.write_all(resp.as_bytes()).await;
					let _ = s.shutdown().await;
				});
			}
		});
		Self {
			ssdp_addr,
			mappings,
			external_ip,
			refuse_adds,
			require_own_address,
			requested_clients,
			cancel,
		}
	}

	pub fn mapping_count(&self) -> usize {
		self.mappings.lock().unwrap().len()
	}

	pub fn preload(&self, m: Mapping) {
		self.mappings.lock().unwrap().insert(m.external_port, m);
	}
}

fn soap_response(
	body: &str,
	m: &Arc<Mutex<BTreeMap<u16, Mapping>>>,
	ip: &Arc<Mutex<String>>,
	refuse: &Arc<Mutex<bool>>,
	peer: String,
	strict: &Arc<Mutex<bool>>,
	seen: &Arc<Mutex<Vec<String>>>,
) -> String {
	if body.contains("GetExternalIPAddress") {
		let ip = ip.lock().unwrap().clone();
		return envelope(&format!(
			r#"<u:GetExternalIPAddressResponse xmlns:u="{SERVICE}"><NewExternalIPAddress>{ip}</NewExternalIPAddress></u:GetExternalIPAddressResponse>"#
		));
	}
	if body.contains("AddPortMapping") {
		seen.lock().unwrap().push(tag(body, "NewInternalClient"));
		if *refuse.lock().unwrap()
			|| (*strict.lock().unwrap() && tag(body, "NewInternalClient") != peer)
		{
			return fault(606, "Action not authorized");
		}
		let ext: u16 = tag(body, "NewExternalPort").parse().unwrap_or(0);
		let mut map = m.lock().unwrap();
		let client = tag(body, "NewInternalClient");
		if let Some(existing) = map.get(&ext)
			&& existing.internal_client != client
		{
			return fault(718, "ConflictInMappingEntry");
		}
		map.insert(
			ext,
			Mapping {
				external_port: ext,
				internal_client: client,
				internal_port: tag(body, "NewInternalPort").parse().unwrap_or(0),
				description: tag(body, "NewPortMappingDescription"),
				lease: tag(body, "NewLeaseDuration").parse().unwrap_or(0),
			},
		);
		return envelope(&format!(
			r#"<u:AddPortMappingResponse xmlns:u="{SERVICE}"/>"#
		));
	}
	if body.contains("DeletePortMapping") {
		let ext: u16 = tag(body, "NewExternalPort").parse().unwrap_or(0);
		return if m.lock().unwrap().remove(&ext).is_some() {
			envelope(&format!(
				r#"<u:DeletePortMappingResponse xmlns:u="{SERVICE}"/>"#
			))
		} else {
			fault(714, "NoSuchEntryInArray")
		};
	}
	if body.contains("GetGenericPortMappingEntry") {
		let idx: usize = tag(body, "NewPortMappingIndex").parse().unwrap_or(0);
		let map = m.lock().unwrap();
		return match map.values().nth(idx) {
			Some(e) => envelope(&format!(
				r#"<u:GetGenericPortMappingEntryResponse xmlns:u="{SERVICE}"><NewRemoteHost></NewRemoteHost><NewExternalPort>{}</NewExternalPort><NewProtocol>TCP</NewProtocol><NewInternalPort>{}</NewInternalPort><NewInternalClient>{}</NewInternalClient><NewEnabled>1</NewEnabled><NewPortMappingDescription>{}</NewPortMappingDescription><NewLeaseDuration>{}</NewLeaseDuration></u:GetGenericPortMappingEntryResponse>"#,
				e.external_port, e.internal_port, e.internal_client, e.description, e.lease
			)),
			None => fault(713, "SpecifiedArrayIndexInvalid"),
		};
	}
	fault(401, "Invalid Action")
}
