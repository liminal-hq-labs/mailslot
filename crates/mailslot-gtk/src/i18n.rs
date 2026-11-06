// Translation helpers.
//
// (c) Copyright 2026 Liminal HQ, Scott Morris
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! gettext wiring. Every user-facing string goes through [`gettext`] or [`ngettext`]
//! (or carries `translatable="yes"` in a `.ui` file).

pub use gettextrs::{gettext, ngettext};

/// The gettext domain.
pub const DOMAIN: &str = "mailslot";

/// Binds the text domain. Missing catalogues fall back to the English source strings.
///
/// Call this first thing in `main`, before any other thread exists: `setlocale` reads
/// and writes process-wide state.
pub fn init() {
	// SAFETY: `main` calls this before starting threads (GTK and tokio are not initialized yet).
	let _ = unsafe { gettextrs::setlocale(gettextrs::LocaleCategory::LcAll, "") };
	let dir = std::env::var_os("MAILSLOT_LOCALEDIR")
		.map(std::path::PathBuf::from)
		.unwrap_or_else(|| std::path::PathBuf::from("/usr/share/locale"));
	let _ = gettextrs::bindtextdomain(DOMAIN, dir);
	let _ = gettextrs::bind_textdomain_codeset(DOMAIN, "UTF-8");
	let _ = gettextrs::textdomain(DOMAIN);
}

/// Formats `{}` placeholders after translating, since gettext strings cannot use `format!`.
pub fn fill(template: &str, args: &[&str]) -> String {
	let mut out = String::with_capacity(template.len() + 16);
	let mut parts = template.split("{}");
	if let Some(first) = parts.next() {
		out.push_str(first);
	}
	for (i, part) in parts.enumerate() {
		out.push_str(args.get(i).copied().unwrap_or(""));
		out.push_str(part);
	}
	out
}

#[cfg(test)]
mod tests {
	use super::fill;

	#[test]
	fn fills_placeholders_in_order() {
		assert_eq!(fill("{} of {}", &["1", "2"]), "1 of 2");
		assert_eq!(fill("none", &[]), "none");
	}
}
