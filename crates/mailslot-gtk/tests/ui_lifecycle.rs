// Display test: the application stays running after activate and quits on request.
//
// (c) Copyright 2026 Liminal HQ, Scott Morris
// SPDX-License-Identifier: Apache-2.0 OR MIT

#![allow(clippy::disallowed_methods)] // Tests may block and use std::fs for set-up.

mod common;

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use gtk::{gio, glib};

#[test]
#[ignore = "needs display"]
fn application_stays_running_after_activate() {
	common::init();
	let dir = tempfile::tempdir().unwrap();
	// SAFETY: this is the only test in the process and no other thread reads the environment yet.
	unsafe { std::env::set_var("MAILSLOT_HOME", dir.path()) };
	mailslot_gtk::init_resources();
	let app = mailslot_gtk::build_application();
	app.set_flags(gio::ApplicationFlags::NON_UNIQUE);
	let alive = Rc::new(Cell::new(false));
	{
		let (app, alive) = (app.clone(), alive.clone());
		// If the application exited by itself, `run` returns before this timer fires.
		glib::timeout_add_local_once(Duration::from_secs(3), move || {
			alive.set(!app.windows().is_empty());
			app.quit();
		});
	}
	app.run_with_args::<&str>(&[]);
	assert!(
		alive.get(),
		"the application exited before it showed a window"
	);
}
