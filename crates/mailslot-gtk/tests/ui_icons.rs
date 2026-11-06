// Display test: the bundled application icons resolve after startup without an install.
//
// (c) Copyright 2026 Liminal HQ, Scott Morris
// SPDX-License-Identifier: Apache-2.0 OR MIT

#![allow(clippy::disallowed_methods)] // Tests may block.

use adw::prelude::*;

#[test]
#[ignore = "needs display"]
fn app_icons_resolve_after_startup() {
	gtk::init().expect("a display is needed: run under weston or Xvfb");
	adw::init().unwrap();
	let display = gtk::gdk::Display::default().expect("display");
	let theme = gtk::IconTheme::for_display(&display);

	// Registering runs the application's startup handler, as `run` does.
	let app = mailslot_gtk::build_application();
	app.register(gtk::gio::Cancellable::NONE)
		.expect("application registers");

	assert!(theme.has_icon(mailslot_gtk::APP_ID), "app icon missing");
	assert!(
		theme.has_icon("ca.liminalhq.mailslot-symbolic"),
		"symbolic icon missing"
	);
}
