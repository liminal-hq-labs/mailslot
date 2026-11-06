// Display test: the window builds and starts in the stopped state.
//
// (c) Copyright 2026 Liminal HQ, Scott Morris
// SPDX-License-Identifier: Apache-2.0 OR MIT

#![allow(clippy::disallowed_methods)] // Tests may block and use std::fs for set-up.

mod common;

use adw::prelude::*;
use common::*;
use mailslot_core::events::Status;

#[test]
#[ignore = "needs display"]
fn window_builds_and_starts_stopped() {
	init();
	let dir = tempfile::tempdir().unwrap();
	let (_app, win, bridge) = make_window(dir.path());
	win.present();
	settle();
	assert_eq!(bridge.state().status, Status::Stopped);
	assert_eq!(win.title().as_deref(), Some("Mailslot"));
	let start = find_by_id(&win, "start_button")
		.and_downcast::<gtk::Button>()
		.expect("start button");
	assert_eq!(start.label().as_deref(), Some("Start sharing"));
	assert!(start.has_css_class("suggested-action"));
	let stack = find_by_id(&win, "share_stack")
		.and_downcast::<gtk::Stack>()
		.unwrap();
	assert_eq!(stack.visible_child_name().as_deref(), Some("stopped"));
	let pages = find_by_id(&win, "stack")
		.and_downcast::<adw::ViewStack>()
		.unwrap();
	let names: Vec<String> = (0..3)
		.filter_map(|i| pages.pages().item(i))
		.filter_map(|p| p.downcast::<adw::ViewStackPage>().ok())
		.filter_map(|p| p.name().map(|n| n.to_string()))
		.collect();
	assert_eq!(names, vec!["share", "send", "history"]);
	// The internet badge only appears while exposed.
	assert!(!find_by_id(&win, "exposed_badge").unwrap().is_visible());
	assert!(
		find_by_id(&win, "banner")
			.and_downcast::<adw::Banner>()
			.is_some_and(|b| !b.is_revealed())
	);
}
