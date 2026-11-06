// Mailslot GTK frontend: a thin libadwaita shell over mailslot-core.
//
// (c) Copyright 2026 Liminal HQ, Scott Morris
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The GTK 4 and libadwaita frontend. All logic lives in `mailslot-core`; this crate
//! shows its state and forwards user commands. Everything here runs on the GLib main
//! thread, and the engine's event stream is consumed with `glib::spawn_future_local`.

pub mod application;
pub mod bridge;
pub mod i18n;
pub mod model;
pub mod preferences;
pub mod qr;
pub mod widgets;
pub mod window;

pub use application::{APP_ID, build_engine, init_resources, load_css, run};
pub use bridge::Bridge;
pub use window::MailslotWindow;

/// Initializes GTK and libadwaita, registers resources and loads the stylesheet.
/// Tests call this once on their only thread; the app does it through GApplication.
pub fn init_for_tests() -> Result<(), gtk::glib::BoolError> {
	gtk::init()?;
	adw::init()?;
	init_resources();
	load_css();
	Ok(())
}
