// Filename sanitizing and collision-free candidate names.
//
// (c) Copyright 2026 Liminal HQ, Scott Morris
// SPDX-License-Identifier: Apache-2.0 OR MIT

use percent_encoding::percent_decode_str;

/// Longest permitted name, in characters.
pub const MAX_CHARS: usize = 150;
/// Longest permitted name, in bytes.
pub const MAX_BYTES: usize = 240;
/// Name used when nothing usable remains.
pub const FALLBACK: &str = "upload";

fn is_bidi_override(c: char) -> bool {
	matches!(c, '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{200E}' | '\u{200F}' | '\u{061C}')
}

fn allowed(c: char) -> bool {
	c.is_alphanumeric() || matches!(c, '.' | '_' | '-' | ' ' | '(' | ')')
}

/// Turns an untrusted request path segment into a safe file name.
///
/// The input is percent-decoded once, backslashes count as separators, only the last
/// path component is kept, control and bidi override characters are dropped, anything
/// outside letters, numbers and `._- ()` becomes `_`, leading and trailing dots and
/// spaces are trimmed, and the result is limited to 150 characters and 240 bytes.
/// An empty result becomes `upload`.
pub fn sanitize(raw: &str) -> String {
	let decoded = percent_decode_str(raw)
		.decode_utf8_lossy()
		.replace('\\', "/");
	let base = decoded.rsplit('/').next().unwrap_or("");
	let cleaned: String = base
		.chars()
		.filter(|c| !c.is_control() && !is_bidi_override(*c))
		.map(|c| if allowed(c) { c } else { '_' })
		.collect();
	let trimmed = cleaned.trim_matches(|c| c == '.' || c == ' ');
	let limited = limit(trimmed);
	if limited.is_empty() {
		FALLBACK.to_owned()
	} else {
		limited
	}
}

fn limit(name: &str) -> String {
	if name.chars().count() <= MAX_CHARS && name.len() <= MAX_BYTES {
		return name.to_owned();
	}
	let (stem, ext) = split_extension(name);
	// Keep a short extension so the file type survives truncation.
	let ext = if ext.chars().count() <= 20 { ext } else { "" };
	let budget_chars = MAX_CHARS.saturating_sub(ext.chars().count());
	let budget_bytes = MAX_BYTES.saturating_sub(ext.len());
	let mut out = String::new();
	for c in stem.chars() {
		if out.chars().count() + 1 > budget_chars || out.len() + c.len_utf8() > budget_bytes {
			break;
		}
		out.push(c);
	}
	let mut result = format!("{out}{ext}");
	result = result.trim_matches(|c| c == '.' || c == ' ').to_owned();
	if result.chars().count() > MAX_CHARS || result.len() > MAX_BYTES {
		result = result.chars().take(MAX_CHARS.min(MAX_BYTES / 4)).collect();
	}
	result
}

const COMPOUND: &[&str] = &[
	".tar.gz",
	".tar.bz2",
	".tar.xz",
	".tar.zst",
	".tar.lz",
	".tar.lzma",
];

/// Splits a name into stem and extension (with the dot), keeping `.tar.gz` together.
pub fn split_extension(name: &str) -> (&str, &str) {
	let lower = name.to_ascii_lowercase();
	for ext in COMPOUND {
		if lower.ends_with(ext) && name.len() > ext.len() {
			let at = name.len() - ext.len();
			return (&name[..at], &name[at..]);
		}
	}
	match name.rfind('.') {
		Some(i) if i > 0 => (&name[..i], &name[i..]),
		_ => (name, ""),
	}
}

/// Returns the lowercase extension without its dot, if any.
pub fn extension_lower(name: &str) -> Option<String> {
	let (_, ext) = split_extension(name);
	let last = ext.rsplit('.').next().unwrap_or("");
	if last.is_empty() {
		None
	} else {
		Some(last.to_ascii_lowercase())
	}
}

/// Returns the `n`th collision candidate: `name`, then `stem-1.ext`, `stem-2.ext`, and so on.
pub fn candidate(name: &str, n: u32) -> String {
	if n == 0 {
		return name.to_owned();
	}
	let (stem, ext) = split_extension(name);
	format!("{stem}-{n}{ext}")
}

/// Whether the extension is one the UI should warn about before opening.
pub fn is_executable_like(name: &str) -> bool {
	const RISKY: &[&str] = &[
		"exe", "msi", "bat", "cmd", "com", "scr", "ps1", "vbs", "js", "jar", "sh", "bash", "run",
		"appimage", "desktop", "py", "pl", "rb", "lnk", "dll", "so", "deb", "rpm", "apk", "dmg",
		"app", "command",
	];
	extension_lower(name).is_some_and(|e| RISKY.contains(&e.as_str()))
}

#[cfg(test)]
mod tests {
	use super::*;
	use proptest::prelude::*;

	#[test]
	fn traversal_names_collapse_to_a_basename() {
		assert_eq!(sanitize("../x"), "x");
		assert_eq!(sanitize("%2e%2e%2fx"), "x");
		assert_eq!(sanitize("a\\b"), "b");
		assert_eq!(sanitize("..\\..\\win.ini"), "win.ini");
		assert_eq!(sanitize(".hidden"), "hidden");
		assert_eq!(sanitize(".."), FALLBACK);
		assert_eq!(sanitize(""), FALLBACK);
		assert_eq!(sanitize("a/"), FALLBACK);
	}

	#[test]
	fn bidi_and_control_characters_are_removed() {
		assert_eq!(sanitize("evil\u{202E}gpj.exe"), "evilgpj.exe");
		assert_eq!(sanitize("a\u{0}b\nc"), "abc");
	}

	#[test]
	fn unusual_characters_become_underscores() {
		assert_eq!(sanitize("a:b*c?.txt"), "a_b_c_.txt");
		assert_eq!(sanitize("naïve café.txt"), "naïve café.txt");
	}

	#[test]
	fn candidates_keep_double_extensions() {
		assert_eq!(candidate("a.tar.gz", 0), "a.tar.gz");
		assert_eq!(candidate("a.tar.gz", 2), "a-2.tar.gz");
		assert_eq!(candidate("notes.txt", 1), "notes-1.txt");
		assert_eq!(candidate("README", 3), "README-3");
	}

	#[test]
	fn long_names_keep_their_extension() {
		let name = format!("{}.zip", "a".repeat(400));
		let out = sanitize(&name);
		assert!(out.len() <= MAX_BYTES && out.chars().count() <= MAX_CHARS);
		assert!(out.ends_with(".zip"));
	}

	proptest! {
		#[test]
		fn output_is_always_safe(raw in "\\PC{0,400}") {
			let out = sanitize(&raw);
			prop_assert!(!out.is_empty());
			prop_assert!(!out.contains('/') && !out.contains('\\') && !out.contains('\0'));
			prop_assert!(!out.chars().any(|c| c.is_control() || is_bidi_override(c)));
			prop_assert!(!out.starts_with('.') && !out.starts_with(' '));
			prop_assert!(!out.ends_with('.') && !out.ends_with(' '));
			prop_assert!(out.len() <= MAX_BYTES);
			prop_assert!(out.chars().count() <= MAX_CHARS);
			// Sanitizing is stable: a safe name stays as it is.
			prop_assert_eq!(sanitize(&out), out);
		}

		#[test]
		fn arbitrary_bytes_in_percent_form_are_safe(bytes in proptest::collection::vec(any::<u8>(), 0..64)) {
			let raw: String = bytes.iter().map(|b| format!("%{b:02x}")).collect();
			let out = sanitize(&raw);
			prop_assert!(!out.contains('/') && !out.chars().any(|c| c.is_control()));
			prop_assert!(!out.starts_with('.'));
		}
	}
}
