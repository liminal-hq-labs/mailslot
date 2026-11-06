// Display test: writes screenshots when MAILSLOT_SCREENSHOT_DIR is set.
//
// (c) Copyright 2026 Liminal HQ, Scott Morris
// SPDX-License-Identifier: Apache-2.0 OR MIT

#![allow(clippy::disallowed_methods)] // Tests may block and use std::fs for set-up.

mod common;

use std::path::Path;
use std::time::Duration;

use adw::prelude::*;
use common::*;
use gtk::graphene;
use mailslot_core::events::{Event, Status, UploadEvent};

/// Renders the window's current contents to a PNG.
fn save_png(win: &mailslot_gtk::MailslotWindow, path: &Path) {
	let paintable = gtk::WidgetPaintable::new(Some(win));
	let (w, h) = (win.width(), win.height());
	let snapshot = gtk::Snapshot::new();
	paintable.snapshot(&snapshot, f64::from(w), f64::from(h));
	let node = snapshot.to_node().expect("render node");
	let renderer = win.native().and_then(|n| n.renderer()).expect("renderer");
	let texture = renderer.render_texture(
		&node,
		Some(&graphene::Rect::new(0.0, 0.0, w as f32, h as f32)),
	);
	texture.save_to_png(path).expect("png is written");
}

#[test]
#[ignore = "needs display"]
fn screenshots() {
	let Some(dir) = std::env::var_os("MAILSLOT_SCREENSHOT_DIR") else {
		eprintln!("MAILSLOT_SCREENSHOT_DIR is not set; nothing to capture");
		return;
	};
	let dir = std::path::PathBuf::from(dir);
	std::fs::create_dir_all(&dir).unwrap();
	init();
	// Make the repository's app icon visible to the icon theme so the screenshots show it.
	let icons = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/icons");
	if let Some(display) = gtk::gdk::Display::default() {
		gtk::IconTheme::for_display(&display).add_search_path(&icons);
	}
	let tmp = tempfile::tempdir().unwrap();
	let (_app, win, bridge) = make_window(tmp.path());
	win.set_default_size(760, 1040);
	win.present();
	settle();
	save_png(&win, &dir.join("main-idle.png"));

	gtk::gio::prelude::ActionGroupExt::activate_action(&win, "toggle-sharing", None);
	assert!(spin_until(Duration::from_secs(10), || bridge
		.state()
		.status
		== Status::Sharing));
	let uploads = [
		(
			"holiday-photos.zip",
			48_000_000u64,
			31_000_000u64,
			"10.0.0.14",
		),
		("invoice-2026-03.pdf", 220_000, 220_000, "10.0.0.14"),
		("build-logs.tar.gz", 9_400_000, 2_100_000, "203.0.113.20"),
	];
	for (i, (name, size, got, peer)) in uploads.iter().enumerate() {
		let id = i as u64 + 1;
		bridge.apply(&Event::Upload(UploadEvent::Started {
			id,
			name: (*name).into(),
			expected: Some(*size),
			peer: (*peer).into(),
		}));
		if got == size {
			bridge.apply(&Event::Upload(UploadEvent::Finished {
				id,
				path: format!("/tmp/{name}").into(),
				saved_as: (*name).into(),
				size: *size,
				sha256: "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08".into(),
				peer: (*peer).into(),
			}));
		} else {
			bridge.apply(&Event::Upload(UploadEvent::Progress { id, received: *got }));
		}
	}
	settle();
	settle();
	settle();
	save_png(&win, &dir.join("main-receiving.png"));
	gtk::gio::prelude::ActionGroupExt::activate_action(&win, "toggle-sharing", None);
	spin_until(Duration::from_secs(10), || {
		bridge.state().status == Status::Stopped
	});
}
