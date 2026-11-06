// QR code rendering into a texture.
//
// (c) Copyright 2026 Liminal HQ, Scott Morris
// SPDX-License-Identifier: Apache-2.0 OR MIT

use gtk::gdk;
use gtk::glib;
use gtk::glib::prelude::*;
use qrcode::{Color, EcLevel, QrCode};

/// Target side of the texture in pixels; modules are scaled by a whole number to fit.
const TARGET: usize = 240;
/// Light border around the code, in modules, required for reliable scanning.
const QUIET: usize = 4;

/// Renders `text` as a dark-on-light QR code texture, or `None` if it does not fit.
pub fn texture_for(text: &str) -> Option<gdk::Texture> {
	let (pixels, side) = render(text)?;
	let bytes = glib::Bytes::from_owned(pixels);
	Some(
		gdk::MemoryTexture::new(
			side as i32,
			side as i32,
			gdk::MemoryFormat::G8,
			&bytes,
			side,
		)
		.upcast(),
	)
}

/// One byte per pixel (0 dark, 255 light) and the side length in pixels.
pub fn render(text: &str) -> Option<(Vec<u8>, usize)> {
	let code = QrCode::with_error_correction_level(text.as_bytes(), EcLevel::M).ok()?;
	let modules = code.width();
	let scale = (TARGET / (modules + QUIET * 2)).max(2);
	let side = (modules + QUIET * 2) * scale;
	let mut pixels = vec![255u8; side * side];
	for my in 0..modules {
		for mx in 0..modules {
			if code[(mx, my)] == Color::Dark {
				for dy in 0..scale {
					let row = ((my + QUIET) * scale + dy) * side + (mx + QUIET) * scale;
					pixels[row..row + scale].fill(0);
				}
			}
		}
	}
	Some((pixels, side))
}

#[cfg(test)]
mod tests {
	use super::render;

	#[test]
	fn has_a_light_quiet_zone_and_dark_modules() {
		let (px, side) = render("mailslot://192.168.1.2:8931/abc").unwrap();
		assert_eq!(px.len(), side * side);
		assert!(
			px[..side * 8].iter().all(|p| *p == 255),
			"top border is light"
		);
		assert!(px.contains(&0));
	}
}
