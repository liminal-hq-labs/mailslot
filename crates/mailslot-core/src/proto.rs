// Wire protocol constants and the mailslot:// link format.
//
// (c) Copyright 2026 Liminal HQ, Scott Morris
// SPDX-License-Identifier: Apache-2.0 OR MIT

use url::Url;

use crate::error::{Error, Result};
use crate::token::Token;

/// Default listening port.
pub const DEFAULT_PORT: u16 = 8931;
/// Protocol revision reported by the health route.
pub const PROTOCOL_VERSION: u32 = 1;

/// Response header naming the file as saved on the server.
pub const HDR_SAVED_AS: &str = "x-mailslot-saved-as";
/// Response header carrying the SHA-256 the server computed, as lowercase hex.
pub const HDR_SHA256: &str = "x-mailslot-sha256";
/// Request header: the SHA-256 the sender expects; a mismatch yields `422`.
pub const HDR_EXPECTED_SHA256: &str = "x-mailslot-expected-sha256";
/// Request header: a client-chosen identifier that makes an upload resumable.
pub const HDR_UPLOAD_ID: &str = "x-mailslot-upload-id";
/// Response header: the number of bytes the server holds for an upload.
pub const HDR_OFFSET: &str = "x-mailslot-offset";
/// Request header: a label for the sender, used by the per-sender subfolder scheme.
pub const HDR_SENDER: &str = "x-mailslot-sender";

/// Route names reserved under the token prefix for `GET`.
pub const ROUTE_HEALTH: &str = "health";
/// Dedupe route name.
pub const ROUTE_HAVE: &str = "have";

/// Capabilities advertised by the health route.
pub const CAPABILITIES: &[&str] = &[
	"resume",
	"content-range",
	"dedupe",
	"chunked",
	"sha256",
	"expected-sha256",
];

/// The strict Content-Security-Policy sent on every response.
pub const CSP: &str = "default-src 'none'; script-src 'self'; style-src 'self'; connect-src 'self'; img-src 'self' data:; form-action 'none'; frame-ancestors 'none'; base-uri 'none'";

/// Everything a sender needs to reach a share.
///
/// The text form is `mailslot://HOST:PORT/TOKEN` with optional `?tls=1&pin=BASE64`
/// where `pin` is the base64 SHA-256 of the server's public key (the same value curl's
/// `--pinnedpubkey sha256//…` takes). Plain `http://` and `https://` drop URLs parse too.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MailslotLink {
	/// Host name or address (IPv6 without brackets).
	pub host: String,
	/// TCP port.
	pub port: u16,
	/// The share token.
	pub token: Token,
	/// Whether the server speaks HTTPS.
	pub tls: bool,
	/// Public-key pin (base64 SHA-256 of the SPKI) for self-signed HTTPS.
	pub pin: Option<String>,
	/// Name of the link; senders use it as their sender label.
	pub label: Option<String>,
}

impl MailslotLink {
	/// Builds a link from parts.
	pub fn new(host: impl Into<String>, port: u16, token: Token) -> Self {
		Self {
			host: host.into(),
			port,
			token,
			tls: false,
			pin: None,
			label: None,
		}
	}

	/// Names the link.
	pub fn with_label(mut self, label: impl Into<String>) -> Self {
		let l = label.into();
		self.label = (!l.trim().is_empty()).then_some(l);
		self
	}

	/// Marks the link as HTTPS with an optional key pin.
	pub fn with_tls(mut self, pin: Option<String>) -> Self {
		self.tls = true;
		self.pin = pin;
		self
	}

	/// Parses `mailslot://`, `http://` or `https://` text.
	pub fn parse(text: &str) -> Result<Self> {
		let text = text.trim();
		let url = Url::parse(text).map_err(|e| Error::InvalidLink(e.to_string()))?;
		let scheme = url.scheme();
		if !matches!(scheme, "mailslot" | "http" | "https") {
			return Err(Error::InvalidLink(format!("unsupported scheme `{scheme}`")));
		}
		let host = url
			.host_str()
			.ok_or_else(|| Error::InvalidLink("missing host".into()))?
			.trim_start_matches('[')
			.trim_end_matches(']')
			.to_owned();
		let default_port = if scheme == "https" {
			443
		} else if scheme == "http" {
			80
		} else {
			DEFAULT_PORT
		};
		let port = url.port().unwrap_or(default_port);
		let token_text = url
			.path_segments()
			.and_then(|mut s| s.next())
			.filter(|s| !s.is_empty())
			.ok_or_else(|| Error::InvalidLink("missing token".into()))?;
		let token =
			Token::parse(token_text).map_err(|_| Error::InvalidLink("malformed token".into()))?;
		let mut tls = scheme == "https";
		let mut pin = None;
		let mut label = None;
		for (k, v) in url.query_pairs() {
			match k.as_ref() {
				"tls" => tls = v == "1" || v == "true",
				"pin" => pin = Some(v.into_owned()),
				"label" => label = Some(v.into_owned()).filter(|l| !l.trim().is_empty()),
				_ => {}
			}
		}
		Ok(Self {
			host,
			port,
			token,
			tls,
			pin,
			label,
		})
	}

	fn authority(&self) -> String {
		if self.host.contains(':') {
			format!("[{}]:{}", self.host, self.port)
		} else {
			format!("{}:{}", self.host, self.port)
		}
	}

	/// The `http(s)://host:port/token/` drop URL.
	pub fn drop_url(&self) -> String {
		let scheme = if self.tls { "https" } else { "http" };
		format!("{scheme}://{}/{}/", self.authority(), self.token.expose())
	}

	/// The URL for a given file name under the drop URL.
	pub fn file_url(&self, name: &str) -> String {
		let encoded = percent_encoding::utf8_percent_encode(name, NAME_SET);
		format!("{}{encoded}", self.drop_url())
	}

	/// The shareable `mailslot://` text.
	pub fn to_uri(&self) -> String {
		let enc = |v: &str| {
			percent_encoding::utf8_percent_encode(v, percent_encoding::NON_ALPHANUMERIC).to_string()
		};
		let mut query = Vec::new();
		if self.tls {
			query.push("tls=1".to_owned());
		}
		if let Some(pin) = &self.pin {
			query.push(format!("pin={}", enc(pin)));
		}
		if let Some(label) = &self.label {
			query.push(format!("label={}", enc(label)));
		}
		let mut s = format!("mailslot://{}/{}", self.authority(), self.token.expose());
		if !query.is_empty() {
			s.push('?');
			s.push_str(&query.join("&"));
		}
		s
	}
}

/// Characters escaped in a file name placed in a URL path.
pub const NAME_SET: &percent_encoding::AsciiSet = &percent_encoding::NON_ALPHANUMERIC
	.remove(b'-')
	.remove(b'_')
	.remove(b'.')
	.remove(b'~');

#[cfg(test)]
mod tests {
	use super::*;

	const TOK: &str = "abcdefghijklmnopqrstuvwxyz0123456789ABCDEFG";

	#[test]
	fn parses_http_and_mailslot_forms() {
		let a = MailslotLink::parse(&format!("http://192.168.1.5:8931/{TOK}/")).unwrap();
		assert_eq!(
			(a.host.as_str(), a.port, a.tls),
			("192.168.1.5", 8931, false)
		);
		let b = MailslotLink::parse(&format!(
			"mailslot://example.org:9000/{TOK}?tls=1&pin=AbC%2B%2F%3D"
		))
		.unwrap();
		assert!(b.tls);
		assert_eq!(b.pin.as_deref(), Some("AbC+/="));
		assert_eq!(MailslotLink::parse(&b.to_uri()).unwrap(), b);
		assert!(b.drop_url().starts_with("https://example.org:9000/"));
	}

	#[test]
	fn labels_roundtrip() {
		let l = MailslotLink::new("10.0.0.2", 8931, Token::parse(TOK).unwrap())
			.with_label("Jules (CI)");
		assert!(l.to_uri().ends_with("?label=Jules%20%28CI%29"));
		assert_eq!(
			MailslotLink::parse(&l.to_uri()).unwrap().label.as_deref(),
			Some("Jules (CI)")
		);
		assert!(
			MailslotLink::new("h", 1, Token::parse(TOK).unwrap())
				.with_label("  ")
				.label
				.is_none()
		);
	}

	#[test]
	fn rejects_garbage() {
		assert!(MailslotLink::parse("ftp://x/").is_err());
		assert!(MailslotLink::parse("http://host:1/").is_err());
		assert!(MailslotLink::parse("not a url").is_err());
	}

	#[test]
	fn file_urls_are_encoded() {
		let l = MailslotLink::new("::1", 1, Token::parse(TOK).unwrap());
		assert!(l.file_url("a b/ü.txt").ends_with("/a%20b%2F%C3%BC.txt"));
		assert!(l.drop_url().starts_with("http://[::1]:1/"));
	}
}
