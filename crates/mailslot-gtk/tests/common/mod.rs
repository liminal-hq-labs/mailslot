// Helpers shared by the display tests.
//
// (c) Copyright 2026 Liminal HQ, Scott Morris
// SPDX-License-Identifier: Apache-2.0 OR MIT

#![allow(dead_code, clippy::disallowed_methods)] // Tests may block; not every test uses every helper.

use std::time::{Duration, Instant};

use adw::prelude::*;
use gtk::glib;
use mailslot_core::config::{Config, Paths};
use mailslot_core::{Engine, EngineBuilder};
use mailslot_gtk::{Bridge, MailslotWindow};

/// Initializes GTK on this thread. Each display test file has exactly one test, so the
/// thread that calls this is the GTK main thread for the whole process.
pub fn init() {
	mailslot_gtk::init_for_tests().expect("a display is needed: run under weston or Xvfb");
}

/// Runs the main loop until `cond` is true or the timeout passes.
pub fn spin_until(timeout: Duration, mut cond: impl FnMut() -> bool) -> bool {
	let ctx = glib::MainContext::default();
	let end = Instant::now() + timeout;
	while Instant::now() < end {
		while ctx.iteration(false) {}
		if cond() {
			return true;
		}
		std::thread::sleep(Duration::from_millis(10));
	}
	cond()
}

/// Lets pending events run.
pub fn settle() {
	let ctx = glib::MainContext::default();
	let end = Instant::now() + Duration::from_millis(150);
	while Instant::now() < end {
		while ctx.iteration(false) {}
		std::thread::sleep(Duration::from_millis(10));
	}
}

pub fn test_config(dir: &std::path::Path) -> Config {
	let mut c = Config::default();
	c.sharing.bind = "127.0.0.1".into();
	c.sharing.port = 0;
	c.storage.dest_dir = dir.join("received");
	c.limits.min_free_mb = 0;
	c
}

pub fn make_engine(dir: &std::path::Path) -> Engine {
	mailslot_core::tls::ensure_default_provider();
	let builder = EngineBuilder::new(Paths::in_dir(dir))
		.config(test_config(dir))
		.persist(false)
		.own_runtime(true);
	glib::MainContext::default()
		.block_on(builder.build())
		.expect("engine builds")
}

pub fn make_window(dir: &std::path::Path) -> (adw::Application, MailslotWindow, Bridge) {
	let app = adw::Application::builder()
		.application_id("ca.liminalhq.mailslot.test")
		.flags(gtk::gio::ApplicationFlags::NON_UNIQUE)
		.build();
	app.register(gtk::gio::Cancellable::NONE)
		.expect("application registers");
	let bridge = Bridge::new(make_engine(dir));
	bridge.run();
	let win = MailslotWindow::new(&app, bridge.clone());
	(app, win, bridge)
}

/// Finds a descendant by its Buildable id (the `id` in the `.ui` file).
pub fn find_by_id(root: &impl IsA<gtk::Widget>, id: &str) -> Option<gtk::Widget> {
	fn walk(w: &gtk::Widget, id: &str) -> Option<gtk::Widget> {
		if gtk::prelude::BuildableExt::buildable_id(w.upcast_ref::<gtk::Buildable>()).as_deref()
			== Some(id)
		{
			return Some(w.clone());
		}
		let mut child = w.first_child();
		while let Some(c) = child {
			if let Some(found) = walk(&c, id) {
				return Some(found);
			}
			child = c.next_sibling();
		}
		None
	}
	walk(root.upcast_ref(), id)
}

/// Collects every descendant of type `T`.
pub fn all_of<T: IsA<gtk::Widget>>(root: &impl IsA<gtk::Widget>) -> Vec<T> {
	fn walk<T: IsA<gtk::Widget>>(w: &gtk::Widget, out: &mut Vec<T>) {
		if let Some(t) = w.downcast_ref::<T>() {
			out.push(t.clone());
		}
		let mut child = w.first_child();
		while let Some(c) = child {
			walk(&c, out);
			child = c.next_sibling();
		}
	}
	let mut out = Vec::new();
	walk(root.upcast_ref(), &mut out);
	out
}
