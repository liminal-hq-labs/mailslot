// Token generation, parsing and constant-time comparison.
//
// (c) Copyright 2026 Liminal HQ, Scott Morris
// SPDX-License-Identifier: Apache-2.0 OR MIT

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

use crate::error::Error;

/// Number of random bytes in a freshly generated token (256 bits).
pub const TOKEN_BYTES: usize = 32;

/// The secret that prefixes every route of a share.
///
/// The value is never printed by `Debug` or `Display`; call [`Token::expose`] when
/// the text is genuinely needed (building a link, writing the token file).
#[derive(Clone)]
pub struct Token(String);

impl Token {
	/// Generates a new 256-bit token from the operating system's random source.
	pub fn generate() -> Result<Self, Error> {
		let mut bytes = [0u8; TOKEN_BYTES];
		getrandom::fill(&mut bytes).map_err(|e| Error::Random(e.to_string()))?;
		Ok(Self(URL_SAFE_NO_PAD.encode(bytes)))
	}

	/// Parses a token from text, accepting URL-safe characters only.
	pub fn parse(text: &str) -> Result<Self, Error> {
		let text = text.trim();
		let ok = (16..=128).contains(&text.len())
			&& text
				.bytes()
				.all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
		if ok {
			Ok(Self(text.to_owned()))
		} else {
			Err(Error::InvalidToken)
		}
	}

	/// Returns the token text.
	pub fn expose(&self) -> &str {
		&self.0
	}

	/// Compares a candidate against this token in constant time.
	///
	/// Both sides are hashed first so the comparison takes the same time whatever the
	/// candidate's length.
	pub fn matches(&self, candidate: &str) -> bool {
		let a = Sha256::digest(self.0.as_bytes());
		let b = Sha256::digest(candidate.as_bytes());
		a.ct_eq(&b).into()
	}

	/// Returns a masked rendition such as `AbCd…wXyZ` for display.
	pub fn masked(&self) -> String {
		let n = self.0.chars().count();
		if n <= 8 {
			return "•".repeat(n);
		}
		let head: String = self.0.chars().take(4).collect();
		let tail: String = self.0.chars().skip(n - 4).collect();
		format!("{head}…{tail}")
	}
}

impl std::fmt::Debug for Token {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		f.write_str("Token(<redacted>)")
	}
}

impl PartialEq for Token {
	fn eq(&self, other: &Self) -> bool {
		self.matches(&other.0)
	}
}

impl Eq for Token {}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn generated_tokens_are_long_and_distinct() {
		let a = Token::generate().unwrap();
		let b = Token::generate().unwrap();
		assert_eq!(a.expose().len(), 43);
		assert_ne!(a, b);
		assert!(a.matches(a.expose()));
		assert!(!a.matches("nope"));
		assert!(!a.matches(""));
	}

	#[test]
	fn debug_never_shows_the_secret() {
		let t = Token::generate().unwrap();
		assert!(!format!("{t:?}").contains(t.expose()));
	}

	#[test]
	fn parse_rejects_bad_characters() {
		assert!(Token::parse("short").is_err());
		assert!(Token::parse("has/slash/has/slash/has/slash").is_err());
		assert!(Token::parse("abcdefghijklmnopqrstuvwxyz0123456789-_").is_ok());
	}
}
