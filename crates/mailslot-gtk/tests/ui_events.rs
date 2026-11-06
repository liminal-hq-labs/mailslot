// Display test: synthetic engine events become list rows with the right fraction and state.
//
// (c) Copyright 2026 Liminal HQ, Scott Morris
// SPDX-License-Identifier: Apache-2.0 OR MIT

#![allow(clippy::disallowed_methods)] // Tests may block and use std::fs for set-up.

mod common;

use adw::prelude::*;
use common::*;
use mailslot_core::events::{Event, UploadEvent};
use mailslot_gtk::model::UploadItem;

fn item(bridge: &mailslot_gtk::Bridge, pos: u32) -> UploadItem {
	bridge
		.uploads()
		.item(pos)
		.and_downcast::<UploadItem>()
		.expect("item")
}

#[test]
#[ignore = "needs display"]
fn synthetic_events_produce_rows() {
	init();
	let dir = tempfile::tempdir().unwrap();
	let (_app, win, bridge) = make_window(dir.path());
	win.present();
	bridge.apply(&Event::Upload(UploadEvent::Started {
		id: 1,
		name: "report.zip".into(),
		expected: Some(1000),
		peer: "10.0.0.5".into(),
	}));
	bridge.apply(&Event::Upload(UploadEvent::Started {
		id: 2,
		name: "setup.exe".into(),
		expected: None,
		peer: "10.0.0.6".into(),
	}));
	settle();
	assert_eq!(bridge.uploads().n_items(), 2);
	// Newest first.
	let (second, first) = (item(&bridge, 0), item(&bridge, 1));
	assert_eq!(
		(
			first.name().as_str(),
			first.state().as_str(),
			first.fraction()
		),
		("report.zip", "receiving", 0.0)
	);
	assert_eq!(second.fraction(), 0.0, "unknown size has no fraction yet");
	assert!(second.risky(), "executables are flagged");
	assert!(!first.risky());

	bridge.apply(&Event::Upload(UploadEvent::Progress {
		id: 1,
		received: 250,
	}));
	assert!((first.fraction() - 0.25).abs() < 1e-9);
	assert_eq!(first.received(), 250);
	settle();
	let list = find_by_id(&win, "incoming_list")
		.and_downcast::<gtk::ListBox>()
		.unwrap();
	let rows = all_of::<mailslot_gtk::widgets::UploadRow>(&list);
	assert_eq!(rows.len(), 2);
	let row = rows.iter().find(|r| r.title() == "report.zip").unwrap();
	assert!(
		row.subtitle().unwrap().contains("25%"),
		"{:?}",
		row.subtitle()
	);

	bridge.apply(&Event::Upload(UploadEvent::Finished {
		id: 1,
		path: "/tmp/received/report.zip".into(),
		saved_as: "report-1.zip".into(),
		size: 1000,
		sha256: "ab".repeat(32),
		peer: "10.0.0.5".into(),
	}));
	assert_eq!((first.state().as_str(), first.fraction()), ("done", 1.0));
	assert_eq!(first.saved_as(), "report-1.zip");
	assert_eq!(first.sha256().len(), 64);
	settle();
	assert!(row.subtitle().unwrap().contains("report-1.zip"));

	bridge.apply(&Event::Upload(UploadEvent::Failed {
		id: 2,
		reason: "connection closed".into(),
		resumable: false,
	}));
	assert_eq!(second.state(), "failed");
	assert_eq!(second.reason(), "connection closed");
	settle();

	// Clearing removes finished rows only.
	bridge.apply(&Event::Upload(UploadEvent::Started {
		id: 3,
		name: "live.bin".into(),
		expected: Some(10),
		peer: "x".into(),
	}));
	bridge.clear_finished();
	assert_eq!(bridge.uploads().n_items(), 1);
	assert_eq!(item(&bridge, 0).name(), "live.bin");
}
