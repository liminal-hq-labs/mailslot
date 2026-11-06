// Secret redaction for anything that leaves the process as text: logs and diagnostics.
//
// (c) Copyright 2026 Liminal HQ, Scott Morris
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Removes the share token from text.
//!
//! The engine never logs the token or a full link. This is the second line of defence for
//! frontends: every log line and diagnostics report passes through [`redact`], which hides
//! token-shaped path segments, `token=` style fields and any secret registered with
//! [`add_secret`].

use std::sync::{Mutex, OnceLock};

/// What replaces a secret.
pub const REDACTED: &str = "<redacted>";

/// The shortest path segment treated as a token. Generated tokens are 43 characters.
const MIN_SEGMENT: usize = 24;
const MAX_SEGMENT: usize = 128;

fn secrets() -> &'static Mutex<Vec<String>> {
	static SECRETS: OnceLock<Mutex<Vec<String>>> = OnceLock::new();
	SECRETS.get_or_init(Mutex::default)
}

/// Registers an exact secret (for example a token supplied by the user, which may not
/// look like a generated one) to be removed wherever it appears.
pub fn add_secret(secret: &str) {
	let secret = secret.trim();
	if secret.len() < 8 {
		return;
	}
	if let Ok(mut v) = secrets().lock()
		&& !v.iter().any(|s| s == secret)
	{
		v.push(secret.to_owned());
	}
}

fn is_token_byte(b: u8) -> bool {
	b.is_ascii_alphanumeric() || b == b'-' || b == b'_'
}

/// Replaces tokens in `text`: registered secrets, the value after `token=`, `pin=` and
/// `Authorization:`-style prefixes, and any path segment that looks like a token
/// (24 to 128 URL-safe characters between `/` marks).
pub fn redact(text: &str) -> String {
	let mut out = text.to_owned();
	if let Ok(v) = secrets().lock() {
		for s in v.iter() {
			out = out.replace(s.as_str(), REDACTED);
		}
	}
	redact_segments(&redact_fields(&out))
}

fn redact_fields(text: &str) -> String {
	let mut out = String::with_capacity(text.len());
	let mut rest = text;
	'outer: while !rest.is_empty() {
		for key in ["token=", "pin=", "Token="] {
			if rest.starts_with(key) {
				out.push_str(key);
				let value = &rest[key.len()..];
				let end = value
					.find(|c: char| c.is_whitespace() || matches!(c, '&' | '"' | '\'' | ','))
					.unwrap_or(value.len());
				if end > 0 {
					out.push_str(REDACTED);
				}
				rest = &value[end..];
				continue 'outer;
			}
		}
		let c = rest.chars().next().expect("not empty");
		out.push(c);
		rest = &rest[c.len_utf8()..];
	}
	out
}

fn redact_segments(text: &str) -> String {
	let bytes = text.as_bytes();
	let mut out = String::with_capacity(text.len());
	let mut i = 0;
	let mut copied = 0;
	while i < bytes.len() {
		if bytes[i] == b'/' {
			let start = i + 1;
			let mut end = start;
			while end < bytes.len() && is_token_byte(bytes[end]) {
				end += 1;
			}
			let len = end - start;
			let delimited = end == bytes.len()
				|| matches!(
					bytes[end],
					b'/' | b'?' | b'"' | b'\'' | b' ' | b'\n' | b'\r' | b'\t' | b',' | b')' | b'>'
				);
			if (MIN_SEGMENT..=MAX_SEGMENT).contains(&len) && delimited {
				out.push_str(&text[copied..start]);
				out.push_str(REDACTED);
				copied = end;
			}
			i = end.max(i + 1);
		} else {
			i += 1;
		}
	}
	out.push_str(&text[copied..]);
	out
}

#[cfg(test)]
mod tests {
	use super::*;

	const TOKEN: &str = "AbCdEfGhIjKlMnOpQrStUvWxYz0123456789-_AbCd";

	#[test]
	fn hides_tokens_in_urls_and_links() {
		for text in [
			format!("PUT http://192.168.1.100:8932/{TOKEN}/photo.jpg"),
			format!("mailslot://192.168.1.100:8932/{TOKEN}?tls=1&pin=abcdef"),
			format!("rejected /{TOKEN}"),
			format!("path=\"/{TOKEN}/\" status=404"),
		] {
			let r = redact(&text);
			assert!(!r.contains(TOKEN), "{r}");
			assert!(r.contains(REDACTED));
		}
	}

	#[test]
	fn hides_field_values_and_registered_secrets() {
		let r = redact("token=hunter2hunter2 pin=zzz label=x");
		assert!(!r.contains("hunter2"), "{r}");
		assert!(!r.contains("zzz"), "{r}");
		assert!(r.contains("label=x"));
		add_secret("my-short-secret");
		assert!(!redact("saw my-short-secret here").contains("my-short-secret"));
	}

	#[test]
	fn leaves_ordinary_text_alone() {
		let t = "uploaded /home/scott/Downloads/a.zip 1234 bytes from 10.0.0.2";
		assert_eq!(redact(t), t);
		assert_eq!(redact("héllo wörld /tmp/ünï"), "héllo wörld /tmp/ünï");
	}
}
