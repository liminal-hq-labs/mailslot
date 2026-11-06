// Preferences dialog: one page per settings area, each with defaults and a reset.
//
// (c) Copyright 2026 Liminal HQ, Scott Morris
// SPDX-License-Identifier: Apache-2.0 OR MIT

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gio, glib};
use mailslot_core::Engine;
use mailslot_core::config::{Collision, Config, Section, SubfolderScheme};

use crate::i18n::gettext;

type Refresher = Box<dyn Fn(&Config)>;

#[derive(Clone)]
struct Ctx {
	engine: Engine,
	cfg: Rc<RefCell<Config>>,
	refreshers: Rc<RefCell<Vec<Refresher>>>,
	updating: Rc<Cell<bool>>,
	/// A save is in flight; later changes are folded into one follow-up save.
	saving: Rc<Cell<bool>>,
	/// The working copy changed while a save was in flight.
	dirty: Rc<Cell<bool>>,
	dialog: adw::PreferencesDialog,
}

impl Ctx {
	/// Saves the working copy and applies it to the engine.
	fn apply(&self) {
		if self.updating.get() {
			return;
		}
		// Saves must reach the engine in order. Spawning one task per change lets an older
		// snapshot finish last and overwrite a newer one, so at most one save runs at a time
		// and changes made meanwhile trigger exactly one more save of the latest copy.
		if self.saving.replace(true) {
			self.dirty.set(true);
			return;
		}
		let ctx = self.clone();
		glib::spawn_future_local(async move {
			loop {
				ctx.dirty.set(false);
				let cfg = ctx.cfg.borrow().clone();
				if let Err(e) = ctx.engine.update_settings(cfg).await {
					ctx.dialog.add_toast(adw::Toast::new(&e.to_string()));
				}
				if !ctx.dirty.get() {
					break;
				}
			}
			ctx.saving.set(false);
		});
	}

	fn refresh_all(&self) {
		self.updating.set(true);
		let cfg = self.cfg.borrow().clone();
		for r in self.refreshers.borrow().iter() {
			r(&cfg);
		}
		self.updating.set(false);
	}

	fn switch(
		&self,
		group: &adw::PreferencesGroup,
		title: &str,
		subtitle: &str,
		get: fn(&Config) -> bool,
		set: fn(&mut Config, bool),
	) {
		let row = adw::SwitchRow::builder()
			.title(title)
			.subtitle(subtitle)
			.active(get(&self.cfg.borrow()))
			.build();
		let ctx = self.clone();
		row.connect_active_notify(move |r| {
			if ctx.updating.get() {
				return;
			}
			set(&mut ctx.cfg.borrow_mut(), r.is_active());
			ctx.apply();
		});
		let row2 = row.clone();
		self.refreshers
			.borrow_mut()
			.push(Box::new(move |c| row2.set_active(get(c))));
		group.add(&row);
	}

	fn spin(
		&self,
		group: &adw::PreferencesGroup,
		title: &str,
		subtitle: &str,
		range: (f64, f64),
		get: fn(&Config) -> f64,
		set: fn(&mut Config, f64),
	) {
		let row = adw::SpinRow::with_range(range.0, range.1, 1.0);
		row.set_title(title);
		row.set_subtitle(subtitle);
		row.set_numeric(true);
		row.set_value(get(&self.cfg.borrow()));
		let ctx = self.clone();
		row.connect_value_notify(move |r| {
			if ctx.updating.get() {
				return;
			}
			set(&mut ctx.cfg.borrow_mut(), r.value());
			ctx.apply();
		});
		let row2 = row.clone();
		self.refreshers
			.borrow_mut()
			.push(Box::new(move |c| row2.set_value(get(c))));
		group.add(&row);
	}

	fn entry(
		&self,
		group: &adw::PreferencesGroup,
		title: &str,
		get: fn(&Config) -> String,
		set: fn(&mut Config, String),
	) {
		let row = adw::EntryRow::builder()
			.title(title)
			.text(get(&self.cfg.borrow()))
			.show_apply_button(true)
			.build();
		let ctx = self.clone();
		row.connect_apply(move |r| {
			set(&mut ctx.cfg.borrow_mut(), r.text().to_string());
			ctx.apply();
		});
		let row2 = row.clone();
		self.refreshers
			.borrow_mut()
			.push(Box::new(move |c| row2.set_text(&get(c))));
		group.add(&row);
	}

	fn combo(
		&self,
		group: &adw::PreferencesGroup,
		title: &str,
		options: &[String],
		get: fn(&Config) -> u32,
		set: fn(&mut Config, u32),
	) {
		let refs: Vec<&str> = options.iter().map(String::as_str).collect();
		let row = adw::ComboRow::builder()
			.title(title)
			.model(&gtk::StringList::new(&refs))
			.selected(get(&self.cfg.borrow()))
			.build();
		let ctx = self.clone();
		row.connect_selected_notify(move |r| {
			if ctx.updating.get() {
				return;
			}
			set(&mut ctx.cfg.borrow_mut(), r.selected());
			ctx.apply();
		});
		let row2 = row.clone();
		self.refreshers
			.borrow_mut()
			.push(Box::new(move |c| row2.set_selected(get(c))));
		group.add(&row);
	}

	fn reset_group(&self, section: Section) -> adw::PreferencesGroup {
		let group = adw::PreferencesGroup::new();
		let row = adw::ButtonRow::builder()
			.title(gettext("Reset to Defaults"))
			.build();
		row.add_css_class("destructive-action");
		let ctx = self.clone();
		row.connect_activated(move |_| {
			ctx.cfg.borrow_mut().reset_section(section);
			ctx.refresh_all();
			// Refreshing suppresses change handlers, so save explicitly.
			ctx.apply();
		});
		group.add(&row);
		group
	}
}

fn page(title: &str, icon: &str) -> adw::PreferencesPage {
	adw::PreferencesPage::builder()
		.title(title)
		.icon_name(icon)
		.build()
}

fn group(title: &str, description: &str) -> adw::PreferencesGroup {
	adw::PreferencesGroup::builder()
		.title(title)
		.description(description)
		.build()
}

fn subfolder_index(c: &Config) -> u32 {
	match c.storage.subfolder {
		SubfolderScheme::None => 0,
		SubfolderScheme::ByDate => 1,
		SubfolderScheme::BySender => 2,
	}
}

/// Builds the dialog. Changes are saved and applied as they are made.
pub fn build(engine: &Engine, parent: &gtk::Window) -> adw::PreferencesDialog {
	let dialog = adw::PreferencesDialog::new();
	dialog.set_title(&gettext("Preferences"));
	dialog.set_search_enabled(true);
	let ctx = Ctx {
		engine: engine.clone(),
		cfg: Rc::new(RefCell::new(engine.config())),
		refreshers: Rc::default(),
		updating: Rc::default(),
		saving: Rc::default(),
		dirty: Rc::default(),
		dialog: dialog.clone(),
	};

	// Sharing.
	let p = page(&gettext("Sharing"), "emblem-shared-symbolic");
	let g = group(
		&gettext("Link"),
		&gettext("Changes to the port or address take effect the next time you start sharing."),
	);
	ctx.spin(
		&g,
		&gettext("Port"),
		&gettext("0 picks a free port"),
		(0.0, 65535.0),
		|c| f64::from(c.sharing.port),
		|c, v| c.sharing.port = v as u16,
	);
	ctx.entry(
		&g,
		&gettext("Bind address"),
		|c| c.sharing.bind.clone(),
		|c, v| {
			if v.parse::<std::net::IpAddr>().is_ok() {
				c.sharing.bind = v;
			}
		},
	);
	ctx.switch(
		&g,
		&gettext("Start sharing on launch"),
		"",
		|c| c.sharing.start_on_launch,
		|c, v| c.sharing.start_on_launch = v,
	);
	p.add(&g);
	let g = group(
		&gettext("Session rules"),
		&gettext("Limit how long and how much a link can be used."),
	);
	ctx.spin(
		&g,
		&gettext("Link expires after (minutes)"),
		&gettext("0 means never"),
		(0.0, 10080.0),
		|c| f64::from(c.sharing.expiry_minutes),
		|c, v| c.sharing.expiry_minutes = v as u32,
	);
	ctx.spin(
		&g,
		&gettext("Stop after this many files"),
		&gettext("0 means no limit; 1 is one-shot mode"),
		(0.0, 100000.0),
		|c| f64::from(c.sharing.stop_after_files),
		|c, v| c.sharing.stop_after_files = v as u32,
	);
	ctx.spin(
		&g,
		&gettext("Auto-stop after (minutes)"),
		&gettext("0 means no timer"),
		(0.0, 10080.0),
		|c| f64::from(c.sharing.auto_stop_minutes),
		|c, v| c.sharing.auto_stop_minutes = v as u32,
	);
	p.add(&g);
	p.add(&ctx.reset_group(Section::Sharing));
	dialog.add(&p);

	// Storage.
	let p = page(&gettext("Storage"), "folder-symbolic");
	let g = group(&gettext("Received files"), "");
	let dest_row = adw::ActionRow::builder()
		.title(gettext("Destination folder"))
		.subtitle(ctx.cfg.borrow().storage.dest_dir.display().to_string())
		.build();
	let choose = gtk::Button::from_icon_name("folder-open-symbolic");
	choose.set_valign(gtk::Align::Center);
	choose.add_css_class("flat");
	choose.set_tooltip_text(Some(&gettext("Choose a folder")));
	dest_row.add_suffix(&choose);
	{
		let (ctx2, dest_row2, parent2) = (ctx.clone(), dest_row.clone(), parent.clone());
		choose.connect_clicked(move |_| {
			let (ctx, dest_row, parent) = (ctx2.clone(), dest_row2.clone(), parent2.clone());
			glib::spawn_future_local(async move {
				let dialog = gtk::FileDialog::new();
				dialog.set_title(&gettext("Choose the destination folder"));
				if let Ok(f) = dialog.select_folder_future(Some(&parent)).await
					&& let Some(path) = f.path()
				{
					ctx.cfg.borrow_mut().storage.dest_dir = path.clone();
					dest_row.set_subtitle(&path.display().to_string());
					ctx.apply();
				}
			});
		});
		let dest_row3 = dest_row.clone();
		ctx.refreshers.borrow_mut().push(Box::new(move |c| {
			dest_row3.set_subtitle(&c.storage.dest_dir.display().to_string())
		}));
	}
	g.add(&dest_row);
	ctx.combo(
		&g,
		&gettext("Subfolders"),
		&[
			gettext("None"),
			gettext("By date"),
			gettext("By sender label"),
		],
		subfolder_index,
		|c, v| {
			c.storage.subfolder = [
				SubfolderScheme::None,
				SubfolderScheme::ByDate,
				SubfolderScheme::BySender,
			][v.min(2) as usize];
		},
	);
	ctx.combo(
		&g,
		&gettext("If a file name exists"),
		&[gettext("Add a number"), gettext("Refuse the upload")],
		|c| u32::from(c.storage.collision == Collision::Reject),
		|c, v| {
			c.storage.collision = if v == 1 {
				Collision::Reject
			} else {
				Collision::Suffix
			};
		},
	);
	ctx.switch(
		&g,
		&gettext("Write .sha256 files"),
		&gettext("Save a checksum file beside each received file"),
		|c| c.storage.sidecars,
		|c, v| c.storage.sidecars = v,
	);
	ctx.switch(
		&g,
		&gettext("Verify after saving"),
		&gettext("Read each file back from disk and check its hash"),
		|c| c.storage.verify_on_receive,
		|c, v| c.storage.verify_on_receive = v,
	);
	p.add(&g);
	p.add(&ctx.reset_group(Section::Storage));
	dialog.add(&p);

	// Limits.
	let p = page(&gettext("Limits"), "emblem-system-symbolic");
	let g = group(&gettext("Uploads"), "");
	ctx.spin(
		&g,
		&gettext("Largest file (MB)"),
		"",
		(1.0, 1_000_000.0),
		|c| c.limits.max_file_mb as f64,
		|c, v| c.limits.max_file_mb = v as u64,
	);
	ctx.spin(
		&g,
		&gettext("Session quota (MB)"),
		&gettext("0 means no quota"),
		(0.0, 10_000_000.0),
		|c| c.limits.session_quota_mb as f64,
		|c, v| c.limits.session_quota_mb = v as u64,
	);
	ctx.spin(
		&g,
		&gettext("Keep this much disk free (MB)"),
		&gettext("Uploads that would go below this are refused"),
		(0.0, 10_000_000.0),
		|c| c.limits.min_free_mb as f64,
		|c, v| c.limits.min_free_mb = v as u64,
	);
	ctx.spin(
		&g,
		&gettext("Uploads at once"),
		"",
		(1.0, 64.0),
		|c| f64::from(c.limits.max_concurrent),
		|c, v| c.limits.max_concurrent = v as u32,
	);
	ctx.spin(
		&g,
		&gettext("Requests per minute per sender"),
		&gettext("0 means no limit"),
		(0.0, 100000.0),
		|c| f64::from(c.limits.rate_limit_per_min),
		|c, v| c.limits.rate_limit_per_min = v as u32,
	);
	ctx.spin(
		&g,
		&gettext("Idle timeout (seconds)"),
		&gettext("A stalled upload is dropped after this long"),
		(5.0, 3600.0),
		|c| c.limits.idle_timeout_secs as f64,
		|c, v| c.limits.idle_timeout_secs = v as u64,
	);
	p.add(&g);
	p.add(&ctx.reset_group(Section::Limits));
	dialog.add(&p);

	// Security.
	let p = page(&gettext("Security"), "security-high-symbolic");
	let g = group(&gettext("Link"), "");
	let new_link = adw::ButtonRow::builder()
		.title(gettext("Make a New Link…"))
		.start_icon_name("view-refresh-symbolic")
		.build();
	new_link.connect_activated(glib::clone!(
		#[weak]
		parent,
		move |_| {
			let _ = parent.activate_action("win.new-link", None);
		}
	));
	g.add(&new_link);
	ctx.spin(
		&g,
		&gettext("Lock out after bad links"),
		&gettext(
			"Wrong-link requests from one address before it is blocked for ten minutes; 0 turns this off",
		),
		(0.0, 1000.0),
		|c| f64::from(c.security.bad_token_lockout),
		|c, v| c.security.bad_token_lockout = v as u32,
	);
	ctx.entry(
		&g,
		&gettext("Allowed file types"),
		|c| c.security.allowed_extensions.join(", "),
		|c, v| {
			c.security.allowed_extensions = v
				.split([',', ' '])
				.map(|s| s.trim().trim_start_matches('.').to_ascii_lowercase())
				.filter(|s| !s.is_empty())
				.collect();
		},
	);
	ctx.switch(
		&g,
		&gettext("Use HTTPS"),
		&gettext("A self-signed certificate; senders verify it with a key pin shown in the link"),
		|c| c.security.https,
		|c, v| c.security.https = v,
	);
	ctx.switch(
		&g,
		&gettext("Warn about executables"),
		&gettext("Mark programs and scripts as risky to open"),
		|c| c.security.warn_on_executables,
		|c, v| c.security.warn_on_executables = v,
	);
	p.add(&g);
	p.add(&ctx.reset_group(Section::Security));
	dialog.add(&p);

	// Network.
	let p = page(&gettext("Network"), "network-wired-symbolic");
	let g = group(&gettext("Reachability"), "");
	ctx.switch(
		&g,
		&gettext("Open to the internet with UPnP"),
		&gettext(
			"Asks your router to forward the port while you share. You are asked to confirm each session.",
		),
		|c| c.network.upnp,
		|c, v| c.network.upnp = v,
	);
	ctx.switch(
		&g,
		&gettext("Allow public address lookup"),
		&gettext("Makes one request to a public service to check for double NAT"),
		|c| c.network.public_ip_lookup,
		|c, v| c.network.public_ip_lookup = v,
	);
	ctx.entry(
		&g,
		&gettext("Preferred interface or address"),
		|c| c.network.interface.clone(),
		|c, v| c.network.interface = v.trim().to_owned(),
	);
	p.add(&g);
	p.add(&ctx.reset_group(Section::Network));
	dialog.add(&p);

	// Notifications and history.
	let p = page(
		&gettext("Notifications"),
		"preferences-system-notifications-symbolic",
	);
	let g = group(&gettext("Notifications"), "");
	ctx.switch(
		&g,
		&gettext("Notify when a file arrives"),
		"",
		|c| c.notifications.notify,
		|c, v| c.notifications.notify = v,
	);
	p.add(&g);
	let g = group(&gettext("History"), "");
	ctx.switch(
		&g,
		&gettext("Keep a history"),
		&gettext("Names, sizes and checksums of files sent and received; never the link"),
		|c| c.notifications.keep_history,
		|c, v| c.notifications.keep_history = v,
	);
	ctx.spin(
		&g,
		&gettext("Keep history for (days)"),
		"",
		(1.0, 3650.0),
		|c| f64::from(c.notifications.history_days),
		|c, v| c.notifications.history_days = v as u32,
	);
	let clear = adw::ButtonRow::builder()
		.title(gettext("Clear History"))
		.build();
	clear.add_css_class("destructive-action");
	clear.connect_activated(glib::clone!(
		#[weak]
		parent,
		move |_| {
			let _ = parent.activate_action("win.clear-history", None);
		}
	));
	g.add(&clear);
	p.add(&g);
	p.add(&ctx.reset_group(Section::Notifications));
	dialog.add(&p);

	let _ = (gio::Cancellable::NONE, PathBuf::new());
	dialog
}
