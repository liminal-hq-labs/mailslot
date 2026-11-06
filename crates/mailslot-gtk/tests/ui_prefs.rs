// Display test: preferences apply immediately and reset to defaults.
//
// (c) Copyright 2026 Liminal HQ, Scott Morris
// SPDX-License-Identifier: Apache-2.0 OR MIT

#![allow(clippy::disallowed_methods)] // Tests may block and use std::fs for set-up.

mod common;

use std::time::Duration;

use adw::prelude::*;
use common::*;
use mailslot_core::config::Config;

#[test]
#[ignore = "needs display"]
fn preferences_apply_and_reset() {
	init();
	let dir = tempfile::tempdir().unwrap();
	let (_app, win, bridge) = make_window(dir.path());
	win.present();
	let dialog = mailslot_gtk::preferences::build(bridge.engine(), win.upcast_ref());
	dialog.present(Some(&win));
	settle();
	let rows = all_of::<adw::SwitchRow>(&dialog);
	let https = rows
		.iter()
		.find(|r| r.title() == "Use HTTPS")
		.expect("HTTPS row");
	assert!(!https.is_active());
	https.set_active(true);
	assert!(spin_until(Duration::from_secs(5), || bridge
		.engine()
		.config()
		.security
		.https));

	// Every area has a reset button, and resetting restores the defaults.
	let resets: Vec<_> = all_of::<adw::ButtonRow>(&dialog)
		.into_iter()
		.filter(|r| r.title() == "Reset to Defaults")
		.collect();
	assert_eq!(resets.len(), 6, "one reset per page");
	let spins = all_of::<adw::SpinRow>(&dialog);
	let max_mb = spins
		.iter()
		.find(|r| r.title() == "Largest file (MB)")
		.unwrap();
	max_mb.set_value(5.0);
	assert!(spin_until(Duration::from_secs(5), || bridge
		.engine()
		.config()
		.limits
		.max_file_mb
		== 5));
	for r in &resets {
		r.emit_by_name::<()>("activated", &[]);
	}
	assert!(spin_until(Duration::from_secs(5), || {
		let c = bridge.engine().config();
		!c.security.https && c.limits.max_file_mb == 200
	}));
	// Defaults match the documented ones, except the values the test config pins.
	let d = Config::default();
	assert_eq!(bridge.engine().config().limits, d.limits);
	assert_eq!(bridge.engine().config().security, d.security);
}
