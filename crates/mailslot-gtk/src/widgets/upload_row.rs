// Row widget for one received or sent file.
//
// (c) Copyright 2026 Liminal HQ, Scott Morris
// SPDX-License-Identifier: Apache-2.0 OR MIT

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::glib;
use gtk::glib::subclass::InitializingObject;
use gtk::{CompositeTemplate, TemplateChild, gio};

use crate::i18n::{fill, gettext};
use crate::model::UploadItem;

mod imp {
	use super::*;

	#[derive(Debug, Default, CompositeTemplate)]
	#[template(resource = "/ca/liminalhq/mailslot/ui/upload_row.ui")]
	pub struct UploadRow {
		#[template_child]
		pub state_icon: TemplateChild<gtk::Image>,
		#[template_child]
		pub risky_icon: TemplateChild<gtk::Image>,
		#[template_child]
		pub progress: TemplateChild<gtk::ProgressBar>,
		#[template_child]
		pub copy_hash_button: TemplateChild<gtk::Button>,
		#[template_child]
		pub open_button: TemplateChild<gtk::Button>,
		#[template_child]
		pub folder_button: TemplateChild<gtk::Button>,
	}

	#[glib::object_subclass]
	impl ObjectSubclass for UploadRow {
		const NAME: &'static str = "MailslotUploadRow";
		type Type = super::UploadRow;
		type ParentType = adw::ActionRow;

		fn class_init(klass: &mut Self::Class) {
			klass.bind_template();
		}

		fn instance_init(obj: &InitializingObject<Self>) {
			obj.init_template();
		}
	}

	impl ObjectImpl for UploadRow {}
	impl WidgetImpl for UploadRow {}
	impl ListBoxRowImpl for UploadRow {}
	impl PreferencesRowImpl for UploadRow {}
	impl ActionRowImpl for UploadRow {}
}

glib::wrapper! {
	/// An `AdwActionRow` showing a transfer: name, status line, progress and actions.
	pub struct UploadRow(ObjectSubclass<imp::UploadRow>)
		@extends adw::ActionRow, adw::PreferencesRow, gtk::ListBoxRow, gtk::Widget,
		@implements gtk::Accessible, gtk::Actionable, gtk::Buildable, gtk::ConstraintTarget;
}

/// Human-readable size such as `3.2 MB`.
pub fn format_size(bytes: u64) -> String {
	glib::format_size(bytes).to_string()
}

/// The status line shown under the file name.
pub fn status_text(item: &UploadItem) -> String {
	let size = format_size(if item.expected() > 0 {
		item.expected()
	} else {
		item.received()
	});
	match item.state().as_str() {
		"done" => {
			let saved = item.saved_as();
			if item.outgoing() {
				fill(&gettext("Sent · {}"), &[&size])
			} else if !saved.is_empty() && saved != item.name() {
				fill(&gettext("Saved as {} · {}"), &[&saved, &size])
			} else {
				fill(&gettext("Received · {}"), &[&size])
			}
		}
		"skipped" => gettext("Already on the other side, nothing sent"),
		"failed" => {
			let r = item.reason();
			if r.is_empty() { gettext("Failed") } else { r }
		}
		_ => {
			let pct = format!("{}%", (item.fraction() * 100.0).round() as u32);
			let mut s = if item.expected() > 0 {
				fill(
					&gettext("{} of {} · {}"),
					&[&format_size(item.received()), &size, &pct],
				)
			} else {
				fill(&gettext("{} received"), &[&format_size(item.received())])
			};
			if item.speed() > 1.0 {
				s.push_str(" · ");
				s.push_str(&fill(
					&gettext("{}/s"),
					&[&format_size(item.speed() as u64)],
				));
				if item.expected() > item.received() {
					let secs =
						((item.expected() - item.received()) as f64 / item.speed()).ceil() as u64;
					s.push_str(" · ");
					s.push_str(&fill(&gettext("{} left"), &[&format_eta(secs)]));
				}
			}
			if !item.outgoing() && !item.peer().is_empty() {
				s.push_str(" · ");
				s.push_str(&item.peer());
			}
			s
		}
	}
}

/// `45 s`, `3 min`, `1 h 5 min`.
pub fn format_eta(secs: u64) -> String {
	if secs < 60 {
		fill(&gettext("{} s"), &[&secs.to_string()])
	} else if secs < 3600 {
		fill(&gettext("{} min"), &[&(secs.div_ceil(60)).to_string()])
	} else {
		fill(
			&gettext("{} h {} min"),
			&[
				&(secs / 3600).to_string(),
				&((secs % 3600).div_ceil(60)).to_string(),
			],
		)
	}
}

impl UploadRow {
	/// Creates a row bound to `item`. `window` is used as the parent of launchers.
	pub fn new(item: &UploadItem, window: Option<&gtk::Window>) -> Self {
		let row: Self = glib::Object::new();
		let imp = row.imp();
		row.set_title(&item.name());

		let refresh = {
			let row = row.downgrade();
			let item = item.clone();
			move || {
				let Some(row) = row.upgrade() else { return };
				let imp = row.imp();
				let state = item.state();
				row.set_subtitle(&status_text(&item));
				imp.progress.set_fraction(item.fraction());
				imp.progress
					.set_visible(state == "receiving" || state == "skipped");
				let (icon, done) = match state.as_str() {
					"done" => ("emblem-ok-symbolic", true),
					"skipped" => ("emblem-ok-symbolic", true),
					"failed" => ("dialog-error-symbolic", false),
					_ => (
						if item.outgoing() {
							"document-send-symbolic"
						} else {
							"folder-download-symbolic"
						},
						false,
					),
				};
				imp.state_icon.set_icon_name(Some(icon));
				imp.copy_hash_button
					.set_visible(done && !item.sha256().is_empty());
				let can_open = state == "done" && !item.outgoing() && !item.path().is_empty();
				imp.open_button.set_visible(can_open);
				imp.folder_button.set_visible(can_open);
				imp.risky_icon.set_visible(item.risky() && !item.outgoing());
				row.remove_css_class("upload-done");
				row.remove_css_class("upload-failed");
				match state.as_str() {
					"done" | "skipped" => row.add_css_class("upload-done"),
					"failed" => row.add_css_class("upload-failed"),
					_ => {}
				}
			}
		};
		refresh();
		for prop in [
			"state", "fraction", "received", "speed", "saved-as", "sha256", "path", "reason",
			"risky", "expected",
		] {
			let refresh = refresh.clone();
			item.connect_notify_local(Some(prop), move |_, _| refresh());
		}
		item.bind_property("name", &row, "title")
			.sync_create()
			.build();

		let window = window.cloned();
		imp.copy_hash_button.connect_clicked({
			let item = item.clone();
			move |b| {
				b.clipboard().set_text(&item.sha256());
				b.activate_action("win.toast", Some(&gettext("SHA-256 copied").to_variant()))
					.ok();
			}
		});
		imp.open_button.connect_clicked({
			let (item, window) = (item.clone(), window.clone());
			move |_| {
				let file = gio::File::for_path(item.path());
				gtk::FileLauncher::new(Some(&file)).launch(
					window.as_ref(),
					gio::Cancellable::NONE,
					|_| {},
				);
			}
		});
		imp.folder_button.connect_clicked({
			let item = item.clone();
			move |_| {
				let file = gio::File::for_path(item.path());
				gtk::FileLauncher::new(Some(&file)).open_containing_folder(
					window.as_ref(),
					gio::Cancellable::NONE,
					|_| {},
				);
			}
		});
		row
	}
}
