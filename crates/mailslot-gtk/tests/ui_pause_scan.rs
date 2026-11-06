// Display test: pausing refuses uploads, and a QR image fills the Send page link.
//
// (c) Copyright 2026 Liminal HQ, Scott Morris
// SPDX-License-Identifier: Apache-2.0 OR MIT

#![allow(clippy::disallowed_methods)] // Tests may block on a local socket.

mod common;

use std::io::{Read, Write};
use std::time::Duration;

use adw::prelude::*;
use common::*;
use mailslot_core::events::Status;

fn http_get(addr: std::net::SocketAddr, path: &str) -> String {
	let mut s = std::net::TcpStream::connect(addr).unwrap();
	write!(
		s,
		"GET {path} HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n"
	)
	.unwrap();
	let mut out = String::new();
	s.read_to_string(&mut out).unwrap();
	out
}

#[test]
#[ignore = "needs display"]
fn pause_and_scan() {
	init();
	let dir = tempfile::tempdir().unwrap();
	let (_app, win, bridge) = make_window(dir.path());
	win.present();
	gtk::gio::prelude::ActionGroupExt::activate_action(&win, "toggle-sharing", None);
	assert!(spin_until(Duration::from_secs(10), || bridge
		.state()
		.status
		== Status::Sharing));
	let pause = find_by_id(&win, "pause_button")
		.and_downcast::<gtk::Button>()
		.unwrap();
	assert!(pause.is_visible());
	assert_eq!(
		pause.icon_name().as_deref(),
		Some("media-playback-pause-symbolic")
	);

	let addr = bridge.state().addr.unwrap();
	let token = bridge.engine().token();
	let health = format!("/{}/health", token.expose());
	assert!(http_get(addr, &health).contains("\"accepting\":true"));

	gtk::gio::prelude::ActionGroupExt::activate_action(&win, "toggle-pause", None);
	assert!(spin_until(Duration::from_secs(10), || bridge
		.state()
		.paused));
	settle();
	assert_eq!(
		pause.icon_name().as_deref(),
		Some("media-playback-start-symbolic")
	);
	let banner = find_by_id(&win, "banner")
		.and_downcast::<adw::Banner>()
		.unwrap();
	assert!(banner.is_revealed() && banner.title().contains("Paused"));
	assert!(http_get(addr, &health).contains("\"accepting\":false"));

	gtk::gio::prelude::ActionGroupExt::activate_action(&win, "toggle-pause", None);
	assert!(spin_until(Duration::from_secs(10), || !bridge
		.state()
		.paused));
	assert!(http_get(addr, &health).contains("\"accepting\":true"));

	// A QR code in an image (here rendered by the app) fills the Send page.
	let link =
		"mailslot://192.168.1.20:8931/abcdefghijklmnopqrstuvwxyz0123456789ABCDEFG?label=Desk";
	let texture = mailslot_gtk::qr::texture_for(link).unwrap();
	win.scan_texture(texture);
	let entry = find_by_id(&win, "send_link_entry")
		.and_downcast::<adw::EntryRow>()
		.unwrap();
	assert!(
		spin_until(Duration::from_secs(10), || entry.text() == link),
		"scanned text: {:?}",
		entry.text()
	);

	gtk::gio::prelude::ActionGroupExt::activate_action(&win, "toggle-sharing", None);
	assert!(spin_until(Duration::from_secs(10), || bridge
		.state()
		.status
		== Status::Stopped));
}
