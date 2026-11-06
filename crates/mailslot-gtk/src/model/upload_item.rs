// GObject holding one received or sent file for list models.
//
// (c) Copyright 2026 Liminal HQ, Scott Morris
// SPDX-License-Identifier: Apache-2.0 OR MIT

use std::cell::{Cell, RefCell};

use gtk::glib;
use gtk::glib::prelude::*;
use gtk::glib::subclass::prelude::*;

/// Direction of a transfer shown in a list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
	/// Received from a sender.
	Incoming,
	/// Sent to another share.
	Outgoing,
}

mod imp {
	use super::*;

	#[derive(Debug, Default, glib::Properties)]
	#[properties(wrapper_type = super::UploadItem)]
	pub struct UploadItem {
		#[property(get, set)]
		pub id: Cell<u64>,
		#[property(get, set)]
		pub name: RefCell<String>,
		#[property(get, set)]
		pub saved_as: RefCell<String>,
		#[property(get, set)]
		pub received: Cell<u64>,
		/// Declared size; `0` while unknown.
		#[property(get, set)]
		pub expected: Cell<u64>,
		/// `0.0` to `1.0`.
		#[property(get, set)]
		pub fraction: Cell<f64>,
		/// One of `receiving`, `done`, `failed`, `skipped`.
		#[property(get, set)]
		pub state: RefCell<String>,
		#[property(get, set)]
		pub sha256: RefCell<String>,
		#[property(get, set)]
		pub path: RefCell<String>,
		/// Bytes per second.
		#[property(get, set)]
		pub speed: Cell<f64>,
		#[property(get, set)]
		pub peer: RefCell<String>,
		#[property(get, set)]
		pub reason: RefCell<String>,
		/// Executable or script: opening may run code.
		#[property(get, set)]
		pub risky: Cell<bool>,
		#[property(get, set)]
		pub outgoing: Cell<bool>,
	}

	#[glib::object_subclass]
	impl ObjectSubclass for UploadItem {
		const NAME: &'static str = "MailslotUploadItem";
		type Type = super::UploadItem;
	}

	#[glib::derived_properties]
	impl ObjectImpl for UploadItem {}
}

glib::wrapper! {
	/// One row of the Incoming or Sending list. Create and use on the main thread only.
	pub struct UploadItem(ObjectSubclass<imp::UploadItem>);
}

impl UploadItem {
	/// Creates an item in the `receiving` state.
	pub fn new(
		id: u64,
		name: &str,
		expected: Option<u64>,
		peer: &str,
		direction: Direction,
	) -> Self {
		glib::Object::builder()
			.property("id", id)
			.property("name", name)
			.property("expected", expected.unwrap_or(0))
			.property("peer", peer)
			.property("state", "receiving")
			.property("outgoing", direction == Direction::Outgoing)
			.build()
	}
}
