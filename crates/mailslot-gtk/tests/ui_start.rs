// Display test: Start reaches the Sharing state on port 0 and shows a link.
//
// (c) Copyright 2026 Liminal HQ, Scott Morris
// SPDX-License-Identifier: Apache-2.0 OR MIT

#![allow(clippy::disallowed_methods)] // Tests may block and use std::fs for set-up.

mod common;

use std::time::Duration;

use adw::prelude::*;
use common::*;
use mailslot_core::events::Status;

#[test]
#[ignore = "needs display"]
fn start_reaches_sharing_and_stop_returns() {
	init();
	let dir = tempfile::tempdir().unwrap();
	let (_app, win, bridge) = make_window(dir.path());
	win.present();
	settle();
	gtk::gio::prelude::ActionGroupExt::activate_action(&win, "toggle-sharing", None);
	assert!(
		spin_until(Duration::from_secs(10), || bridge.state().status
			== Status::Sharing),
		"never reached Sharing"
	);
	settle();
	let st = bridge.state();
	assert_ne!(st.addr.unwrap().port(), 0, "port 0 resolves to a real port");
	let stack = find_by_id(&win, "share_stack")
		.and_downcast::<gtk::Stack>()
		.unwrap();
	assert_eq!(stack.visible_child_name().as_deref(), Some("running"));
	let link = find_by_id(&win, "link_row")
		.and_downcast::<adw::ActionRow>()
		.unwrap();
	let url = link.subtitle().unwrap().to_string();
	assert!(
		url.starts_with("http://") && url.contains(bridge.engine().token().expose()),
		"{url}"
	);
	let qr = find_by_id(&win, "qr_picture")
		.and_downcast::<gtk::Picture>()
		.unwrap();
	assert!(qr.paintable().is_some(), "QR code is drawn");
	let start = find_by_id(&win, "start_button")
		.and_downcast::<gtk::Button>()
		.unwrap();
	assert_eq!(start.label().as_deref(), Some("Stop sharing"));
	// The token is masked until revealed.
	let token_row = find_by_id(&win, "token_row")
		.and_downcast::<adw::ActionRow>()
		.unwrap();
	assert!(
		!token_row
			.subtitle()
			.unwrap()
			.contains(bridge.engine().token().expose())
	);
	find_by_id(&win, "reveal_button")
		.and_downcast::<gtk::ToggleButton>()
		.unwrap()
		.set_active(true);
	assert!(
		token_row
			.subtitle()
			.unwrap()
			.contains(bridge.engine().token().expose())
	);

	gtk::gio::prelude::ActionGroupExt::activate_action(&win, "toggle-sharing", None);
	assert!(spin_until(Duration::from_secs(10), || bridge
		.state()
		.status
		== Status::Stopped));
	settle();
	assert_eq!(stack.visible_child_name().as_deref(), Some("stopped"));
}
