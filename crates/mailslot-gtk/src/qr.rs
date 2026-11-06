// QR code rendering into a texture.
//
// (c) Copyright 2026 Liminal HQ, Scott Morris
// SPDX-License-Identifier: Apache-2.0 OR MIT

use gtk::gdk;
use gtk::glib;
use gtk::glib::prelude::*;
use gtk::prelude::TextureExt;
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

/// Reads a QR code from 8-bit luminance pixels, returning its text.
pub fn decode_luma(width: usize, height: usize, luma: &[u8]) -> Option<String> {
	if luma.len() < width * height || width == 0 || height == 0 {
		return None;
	}
	let mut img =
		rqrr::PreparedImage::prepare_from_greyscale(width, height, |x, y| luma[y * width + x]);
	img.detect_grids()
		.into_iter()
		.find_map(|g| g.decode().ok().map(|(_, text)| text))
}

/// Luminance pixels of a texture, composited over white, with their size.
pub fn texture_luma(texture: &gdk::Texture) -> Option<(usize, usize, Vec<u8>)> {
	let (w, h) = (
		usize::try_from(texture.width()).ok()?,
		usize::try_from(texture.height()).ok()?,
	);
	let mut downloader = gdk::TextureDownloader::new(texture);
	downloader.set_format(gdk::MemoryFormat::R8g8b8a8);
	let (bytes, stride) = downloader.download_bytes();
	let mut luma = Vec::with_capacity(w * h);
	for y in 0..h {
		for x in 0..w {
			let i = y * stride + x * 4;
			let px = bytes.get(i..i + 4)?;
			let a = u32::from(px[3]);
			let l =
				(u32::from(px[0]) * 299 + u32::from(px[1]) * 587 + u32::from(px[2]) * 114) / 1000;
			luma.push(((l * a + 255 * (255 - a)) / 255) as u8);
		}
	}
	Some((w, h, luma))
}

/// Reads a QR code from an image texture (a file, a screenshot or the clipboard).
pub fn decode_texture(texture: &gdk::Texture) -> Option<String> {
	let (w, h, luma) = texture_luma(texture)?;
	decode_luma(w, h, &luma)
}

#[cfg(test)]
mod tests {
	use super::{decode_luma, render};

	#[test]
	fn a_rendered_code_reads_back() {
		let link =
			"mailslot://192.168.1.2:8931/abcdefghijklmnopqrstuvwxyz0123456789ABCDEFG?label=Desk";
		let (px, side) = render(link).unwrap();
		assert_eq!(decode_luma(side, side, &px).as_deref(), Some(link));
		assert_eq!(decode_luma(side, side, &vec![255; side * side]), None);
	}

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
