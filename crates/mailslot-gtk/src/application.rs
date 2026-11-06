// The application object: actions, accelerators, styling and window lifecycle.
//
// (c) Copyright 2026 Liminal HQ, Scott Morris
// SPDX-License-Identifier: Apache-2.0 OR MIT

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gdk, gio, glib};
use mailslot_core::config::Paths;
use mailslot_core::{Engine, EngineBuilder};

use crate::bridge::Bridge;
use crate::i18n::gettext;
use crate::window::MailslotWindow;

/// The application identifier.
pub const APP_ID: &str = "ca.liminalhq.mailslot";

/// Registers the compiled resources once.
pub fn init_resources() {
	static ONCE: std::sync::Once = std::sync::Once::new();
	ONCE.call_once(|| {
		gio::resources_register_include!("mailslot.gresource")
			.expect("the gresource bundle is built in");
	});
}

/// Installs the application stylesheet for the default display.
pub fn load_css() {
	let Some(display) = gdk::Display::default() else {
		return;
	};
	let provider = gtk::CssProvider::new();
	provider.load_from_resource("/ca/liminalhq/mailslot/style.css");
	gtk::style_context_add_provider_for_display(
		&display,
		&provider,
		gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
	);
}

/// Builds an engine that runs on its own tokio runtime (GLib has no tokio reactor).
/// `MAILSLOT_HOME` keeps settings and data under one folder, for development and tests.
pub async fn build_engine() -> Result<Engine, mailslot_core::Error> {
	mailslot_core::tls::ensure_default_provider();
	let paths = match std::env::var_os("MAILSLOT_HOME") {
		Some(h) => Paths::in_dir(PathBuf::from(h)),
		None => Paths::xdg(),
	};
	EngineBuilder::new(paths).own_runtime(true).build().await
}

fn shortcuts_dialog() -> adw::ShortcutsDialog {
	let dialog = adw::ShortcutsDialog::new();
	let general = adw::ShortcutsSection::new(Some(&gettext("General")));
	for (title, accel) in [
		(gettext("Start or stop sharing"), "<Control><Shift>s"),
		(gettext("Copy link"), "<Control><Shift>c"),
		(gettext("Make a new link"), "<Control><Shift>n"),
		(gettext("Preferences"), "<Control>comma"),
		(gettext("Keyboard shortcuts"), "<Control>question"),
		(gettext("Quit"), "<Control>q"),
	] {
		general.add(adw::ShortcutsItem::new(&title, accel));
	}
	dialog.add(general);
	dialog
}

fn about_dialog() -> adw::AboutDialog {
	adw::AboutDialog::builder()
		.application_name(gettext("Mailslot"))
		.application_icon(APP_ID)
		.developer_name("Liminal HQ")
		.developers(vec!["Scott Morris".to_owned()])
		.version(env!("CARGO_PKG_VERSION"))
		.comments(gettext(
			"A private file drop. Share a secret link and receive files; nothing is ever served back.",
		))
		.website("https://github.com/liminal-hq/mailslot")
		.issue_url("https://github.com/liminal-hq/mailslot/issues")
		.copyright("© 2026 Liminal HQ, Scott Morris")
		.license_type(gtk::License::Custom)
		.license(
			"Licensed under either of the Apache License, Version 2.0 or the MIT licence, at your option.",
		)
		.build()
}

/// Runs the application and returns the process exit code.
pub fn run() -> glib::ExitCode {
	let app = adw::Application::builder().application_id(APP_ID).build();
	let window: Rc<RefCell<Option<MailslotWindow>>> = Rc::default();

	app.connect_startup(|app| {
		init_resources();
		load_css();
		gtk::Window::set_default_icon_name(APP_ID);
		for (action, accels) in [
			("app.quit", &["<Control>q"][..]),
			("app.preferences", &["<Control>comma"]),
			(
				"app.shortcuts",
				&["<Control>question", "<Control><Shift>slash"],
			),
			("win.toggle-sharing", &["<Control><Shift>s"]),
			("win.copy-link", &["<Control><Shift>c"]),
			("win.new-link", &["<Control><Shift>n"]),
		] {
			app.set_accels_for_action(action, accels);
		}
	});

	{
		let window = window.clone();
		app.connect_activate(move |app| {
			if let Some(w) = window.borrow().as_ref() {
				w.present();
				return;
			}
			let (app, window) = (app.clone(), window.clone());
			glib::spawn_future_local(async move {
				let engine = match build_engine().await {
					Ok(e) => e,
					Err(e) => {
						let dialog = adw::AlertDialog::new(
							Some(&gettext("Mailslot could not start")),
							Some(&e.to_string()),
						);
						dialog.add_response("ok", &gettext("_Close"));
						dialog.present(None::<&gtk::Widget>);
						return;
					}
				};
				let bridge = Bridge::new(engine.clone());
				bridge.run();
				let win = MailslotWindow::new(&app, bridge);
				let _ = engine.prune_history().await;
				win.present();
				if engine.config().sharing.start_on_launch {
					gio::prelude::ActionGroupExt::activate_action(&win, "toggle-sharing", None);
				}
				*window.borrow_mut() = Some(win);
			});
		});
	}

	let add = |app: &adw::Application, name: &str, f: Box<dyn Fn(&adw::Application)>| {
		let action = gio::SimpleAction::new(name, None);
		let app2 = app.clone();
		action.connect_activate(move |_, _| f(&app2));
		app.add_action(&action);
	};
	{
		let window = window.clone();
		add(
			&app,
			"quit",
			Box::new(move |app| {
				// Closing the window runs the "stop sharing?" confirmation.
				match window.borrow().as_ref() {
					Some(w) => w.close(),
					None => app.quit(),
				}
			}),
		);
	}
	{
		let window = window.clone();
		add(
			&app,
			"present",
			Box::new(move |_| {
				if let Some(w) = window.borrow().as_ref() {
					w.present();
				}
			}),
		);
	}
	{
		let window = window.clone();
		add(
			&app,
			"preferences",
			Box::new(move |_| {
				if let Some(w) = window.borrow().as_ref() {
					crate::preferences::build(w.bridge().engine(), w.upcast_ref()).present(Some(w));
				}
			}),
		);
	}
	{
		let window = window.clone();
		add(
			&app,
			"shortcuts",
			Box::new(move |_| {
				if let Some(w) = window.borrow().as_ref() {
					shortcuts_dialog().present(Some(w));
				}
			}),
		);
	}
	{
		let window = window.clone();
		add(
			&app,
			"about",
			Box::new(move |_| {
				if let Some(w) = window.borrow().as_ref() {
					about_dialog().present(Some(w));
				}
			}),
		);
	}
	let show_file = gio::SimpleAction::new("show-file", Some(glib::VariantTy::STRING));
	show_file.connect_activate(|_, param| {
		if let Some(path) = param.and_then(|p| p.get::<String>()) {
			let file = gio::File::for_path(path);
			gtk::FileLauncher::new(Some(&file)).open_containing_folder(
				None::<&gtk::Window>,
				gio::Cancellable::NONE,
				|_| {},
			);
		}
	});
	app.add_action(&show_file);

	app.run_with_args::<&str>(&[])
}
