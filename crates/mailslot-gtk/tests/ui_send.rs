// Display test: the Send page sends a file to another share and reports progress.
//
// (c) Copyright 2026 Liminal HQ, Scott Morris
// SPDX-License-Identifier: Apache-2.0 OR MIT

#![allow(clippy::disallowed_methods)] // Tests may block and use std::fs for set-up.

mod common;

use std::time::Duration;

use adw::prelude::*;
use common::*;
use gtk::glib;
use mailslot_core::proto::MailslotLink;
use mailslot_gtk::model::UploadItem;

#[test]
#[ignore = "needs display"]
fn send_page_sends_a_file() {
	init();
	let dir = tempfile::tempdir().unwrap();
	let (_app, win, bridge) = make_window(&dir.path().join("sender"));

	// A second engine in the same process plays the receiving side on 127.0.0.1.
	let receiver = make_engine(&dir.path().join("receiver"));
	let state = glib::MainContext::default()
		.block_on(receiver.start())
		.expect("receiver starts");
	let link = MailslotLink::new("127.0.0.1", state.addr.unwrap().port(), receiver.token());

	win.present();
	let entry = find_by_id(&win, "send_link_entry")
		.and_downcast::<adw::EntryRow>()
		.unwrap();
	entry.set_text(&link.to_uri());
	assert!(
		spin_until(Duration::from_secs(10), || find_by_id(
			&win,
			"send_status_row"
		)
		.and_downcast::<adw::ActionRow>()
		.and_then(|r| r.subtitle())
		.is_some_and(|s| s.contains("ready"))),
		"link check never succeeded"
	);

	let src = dir.path().join("hello.txt");
	std::fs::write(&src, b"sent from the GTK test").unwrap();
	win.send_paths(vec![src]);
	assert!(
		spin_until(Duration::from_secs(10), || bridge
			.sends()
			.item(0)
			.and_downcast::<UploadItem>()
			.is_some_and(|i| i.state() == "done")),
		"send never finished"
	);
	let item = bridge.sends().item(0).and_downcast::<UploadItem>().unwrap();
	assert_eq!((item.fraction(), item.name().as_str()), (1.0, "hello.txt"));
	assert_eq!(item.sha256().len(), 64);
	let landed = receiver.config().storage.dest_dir.join("hello.txt");
	assert_eq!(std::fs::read(landed).unwrap(), b"sent from the GTK test");
	glib::MainContext::default()
		.block_on(receiver.stop())
		.unwrap();
}
