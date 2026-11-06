// The main window.
//
// (c) Copyright 2026 Liminal HQ, Scott Morris
// SPDX-License-Identifier: Apache-2.0 OR MIT

use std::cell::{Cell, OnceCell, RefCell};
use std::rc::Rc;
use std::time::SystemTime;

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::glib::subclass::InitializingObject;
use gtk::{CompositeTemplate, TemplateChild, gdk, gio, glib};
use mailslot_core::events::{Event, InternetState, SendEvent, Status, UploadEvent};
use mailslot_core::proto::MailslotLink;
use mailslot_core::{Error as CoreError, SendRequest, snippets};

use crate::bridge::Bridge;
use crate::i18n::{fill, gettext, ngettext};
use crate::model::UploadItem;
use crate::widgets::UploadRow;

mod imp {
	use super::*;

	#[derive(CompositeTemplate, Default)]
	#[template(resource = "/ca/liminalhq/mailslot/ui/window.ui")]
	pub struct MailslotWindow {
		#[template_child]
		pub header: TemplateChild<adw::HeaderBar>,
		#[template_child]
		pub start_button: TemplateChild<gtk::Button>,
		#[template_child]
		pub exposed_badge: TemplateChild<gtk::Button>,
		#[template_child]
		pub banner: TemplateChild<adw::Banner>,
		#[template_child]
		pub toasts: TemplateChild<adw::ToastOverlay>,
		#[template_child]
		pub stack: TemplateChild<adw::ViewStack>,
		#[template_child]
		pub share_stack: TemplateChild<gtk::Stack>,
		#[template_child]
		pub link_row: TemplateChild<adw::ActionRow>,
		#[template_child]
		pub interface_row: TemplateChild<adw::ComboRow>,
		#[template_child]
		pub token_row: TemplateChild<adw::ActionRow>,
		#[template_child]
		pub reveal_button: TemplateChild<gtk::ToggleButton>,
		#[template_child]
		pub expiry_row: TemplateChild<adw::ActionRow>,
		#[template_child]
		pub qr_picture: TemplateChild<gtk::Picture>,
		#[template_child]
		pub internet_row: TemplateChild<adw::SwitchRow>,
		#[template_child]
		pub external_row: TemplateChild<adw::ActionRow>,
		#[template_child]
		pub manual_row: TemplateChild<adw::ButtonRow>,
		#[template_child]
		pub incoming_list: TemplateChild<gtk::ListBox>,
		#[template_child]
		pub send_link_entry: TemplateChild<adw::EntryRow>,
		#[template_child]
		pub paste_link_button: TemplateChild<gtk::Button>,
		#[template_child]
		pub send_status_row: TemplateChild<adw::ActionRow>,
		#[template_child]
		pub send_files_row: TemplateChild<adw::ButtonRow>,
		#[template_child]
		pub send_folder_row: TemplateChild<adw::ButtonRow>,
		#[template_child]
		pub send_list: TemplateChild<gtk::ListBox>,
		#[template_child]
		pub history_stack: TemplateChild<gtk::Stack>,
		#[template_child]
		pub history_list: TemplateChild<gtk::ListBox>,

		pub bridge: OnceCell<Bridge>,
		pub urls: RefCell<Vec<mailslot_core::events::ShareUrl>>,
		pub banner_action: RefCell<Option<Rc<dyn Fn()>>>,
		pub updating: Cell<bool>,
		pub force_close: Cell<bool>,
		pub send_check: Cell<u32>,
		pub unseen: Cell<u32>,
		pub last_batch_name: RefCell<String>,
	}

	#[glib::object_subclass]
	impl ObjectSubclass for MailslotWindow {
		const NAME: &'static str = "MailslotWindow";
		type Type = super::MailslotWindow;
		type ParentType = adw::ApplicationWindow;

		fn class_init(klass: &mut Self::Class) {
			klass.bind_template();
		}

		fn instance_init(obj: &InitializingObject<Self>) {
			obj.init_template();
		}
	}

	impl ObjectImpl for MailslotWindow {}
	impl WidgetImpl for MailslotWindow {}
	impl WindowImpl for MailslotWindow {
		fn close_request(&self) -> glib::Propagation {
			let win = self.obj();
			if self.force_close.get() {
				return self.parent_close_request();
			}
			let sharing = self
				.bridge
				.get()
				.is_some_and(|b| b.state().status == Status::Sharing);
			if !sharing {
				return self.parent_close_request();
			}
			let win = win.clone();
			glib::spawn_future_local(async move {
				let dialog = adw::AlertDialog::new(
					Some(&gettext("Stop sharing and quit?")),
					Some(&gettext(
						"Files can no longer be sent to your link once Mailslot closes.",
					)),
				);
				dialog.add_responses(&[
					("cancel", &gettext("_Keep Sharing")),
					("quit", &gettext("_Stop and Quit")),
				]);
				dialog.set_response_appearance("quit", adw::ResponseAppearance::Destructive);
				dialog.set_default_response(Some("cancel"));
				dialog.set_close_response("cancel");
				if dialog.choose_future(Some(&win)).await == "quit" {
					if let Some(b) = win.imp().bridge.get() {
						let _ = b.engine().stop().await;
					}
					win.imp().force_close.set(true);
					win.close();
				}
			});
			glib::Propagation::Stop
		}
	}
	impl ApplicationWindowImpl for MailslotWindow {}
	impl AdwApplicationWindowImpl for MailslotWindow {}
}

glib::wrapper! {
	/// The Mailslot window: header bar, Share, Send and History pages.
	pub struct MailslotWindow(ObjectSubclass<imp::MailslotWindow>)
		@extends adw::ApplicationWindow, gtk::ApplicationWindow, gtk::Window, gtk::Widget,
		@implements gio::ActionGroup, gio::ActionMap, gtk::Accessible, gtk::Buildable,
		gtk::ConstraintTarget, gtk::Native, gtk::Root, gtk::ShortcutManager;
}

fn relative_expiry(t: SystemTime) -> String {
	let secs = t
		.duration_since(SystemTime::now())
		.map(|d| d.as_secs())
		.unwrap_or(0);
	let at = t
		.duration_since(SystemTime::UNIX_EPOCH)
		.ok()
		.and_then(|d| glib::DateTime::from_unix_local(d.as_secs() as i64).ok())
		.and_then(|d| d.format("%H:%M").ok())
		.map(|s| s.to_string())
		.unwrap_or_default();
	if secs < 90 {
		fill(&gettext("In under a minute (at {})"), &[&at])
	} else {
		let mins = secs.div_ceil(60);
		fill(
			&gettext("In {} (at {})"),
			&[
				&fill(
					&ngettext("{} minute", "{} minutes", mins as u32),
					&[&mins.to_string()],
				),
				&at,
			],
		)
	}
}

impl MailslotWindow {
	/// Builds the window around a bridge. Call `present()` to show it.
	pub fn new(app: &adw::Application, bridge: Bridge) -> Self {
		let win: Self = glib::Object::builder().property("application", app).build();
		let imp = win.imp();
		let _ = imp.bridge.set(bridge.clone());
		win.setup_models(&bridge);
		win.setup_actions();
		win.setup_widgets();
		win.setup_drop_target();
		let weak = win.downgrade();
		bridge.connect(move |ev| {
			if let Some(win) = weak.upgrade() {
				win.on_event(ev);
			}
		});
		win.refresh(&bridge.state());
		glib::timeout_add_seconds_local(
			30,
			glib::clone!(
				#[weak]
				win,
				#[upgrade_or]
				glib::ControlFlow::Break,
				move || {
					if let Some(b) = win.imp().bridge.get() {
						win.refresh_expiry(&b.state());
					}
					glib::ControlFlow::Continue
				}
			),
		);
		win
	}

	/// The bridge this window shows.
	pub fn bridge(&self) -> &Bridge {
		self.imp().bridge.get().expect("bridge is set in new()")
	}

	/// Shows a toast message.
	pub fn toast(&self, text: &str) {
		self.imp().toasts.add_toast(adw::Toast::new(text));
	}

	fn setup_models(&self, bridge: &Bridge) {
		let imp = self.imp();
		let weak = self.downgrade();
		let make = move |obj: &glib::Object| -> gtk::Widget {
			let item = obj.downcast_ref::<UploadItem>().expect("UploadItem");
			let win = weak.upgrade();
			UploadRow::new(item, win.as_ref().map(|w| w.upcast_ref::<gtk::Window>())).upcast()
		};
		imp.incoming_list
			.bind_model(Some(bridge.uploads()), make.clone());
		imp.send_list.bind_model(Some(bridge.sends()), make);
		let placeholder = |text: String| {
			let label = gtk::Label::new(Some(&text));
			label.add_css_class("dim-label");
			label.set_margin_top(18);
			label.set_margin_bottom(18);
			label
		};
		imp.incoming_list
			.set_placeholder(Some(&placeholder(gettext("Waiting for files…"))));
		imp.send_list
			.set_placeholder(Some(&placeholder(gettext("Nothing sent yet"))));
	}

	fn setup_actions(&self) {
		let toggle = gio::ActionEntry::builder("toggle-sharing")
			.activate(|win: &Self, _, _| win.toggle_sharing())
			.build();
		let copy_link = gio::ActionEntry::builder("copy-link")
			.activate(|win: &Self, _, _| {
				if let Some(u) = win.selected_url() {
					win.clipboard().set_text(&u.url);
					win.toast(&gettext("Link copied"));
				}
			})
			.build();
		let copy_external = gio::ActionEntry::builder("copy-external")
			.activate(|win: &Self, _, _| {
				if let Some(b) = win.imp().bridge.get()
					&& let InternetState::Open { url, .. } = b.state().internet
				{
					win.clipboard().set_text(&url);
					win.toast(&gettext("Internet link copied"));
				}
			})
			.build();
		let copy_snippet = gio::ActionEntry::builder("copy-snippet")
			.parameter_type(Some(glib::VariantTy::STRING))
			.activate(|win: &Self, _, param| {
				let kind: String = param.and_then(|p| p.get()).unwrap_or_default();
				win.copy_snippet(&kind);
			})
			.build();
		let new_link = gio::ActionEntry::builder("new-link")
			.activate(|win: &Self, _, _| win.confirm_new_link())
			.build();
		let clear = gio::ActionEntry::builder("clear-finished")
			.activate(|win: &Self, _, _| {
				win.bridge().clear_finished();
			})
			.build();
		let clear_history = gio::ActionEntry::builder("clear-history")
			.activate(|win: &Self, _, _| {
				let win = win.clone();
				glib::spawn_future_local(async move {
					let _ = win.bridge().engine().clear_history().await;
					win.reload_history();
				});
			})
			.build();
		let close_internet = gio::ActionEntry::builder("close-internet")
			.activate(|win: &Self, _, _| {
				let win = win.clone();
				glib::spawn_future_local(async move {
					let _ = win.bridge().engine().set_internet_exposed(false).await;
				});
			})
			.build();
		let toast = gio::ActionEntry::builder("toast")
			.parameter_type(Some(glib::VariantTy::STRING))
			.activate(|win: &Self, _, param| {
				if let Some(t) = param.and_then(|p| p.get::<String>()) {
					win.toast(&t);
				}
			})
			.build();
		self.add_action_entries([
			toggle,
			copy_link,
			copy_external,
			copy_snippet,
			new_link,
			clear,
			clear_history,
			close_internet,
			toast,
		]);
	}

	fn setup_widgets(&self) {
		let imp = self.imp();
		imp.reveal_button.connect_toggled(glib::clone!(
			#[weak(rename_to = win)]
			self,
			move |_| win.refresh_token_row()
		));
		imp.interface_row.connect_selected_notify(glib::clone!(
			#[weak(rename_to = win)]
			self,
			move |_| {
				if !win.imp().updating.get() {
					win.refresh_link();
				}
			}
		));
		imp.internet_row.connect_active_notify(glib::clone!(
			#[weak(rename_to = win)]
			self,
			move |row| {
				if win.imp().updating.get() {
					return;
				}
				win.on_internet_toggled(row.is_active());
			}
		));
		imp.manual_row.connect_activated(glib::clone!(
			#[weak(rename_to = win)]
			self,
			move |_| win.show_manual_instructions()
		));
		imp.banner.connect_button_clicked(glib::clone!(
			#[weak(rename_to = win)]
			self,
			move |_| {
				let action = win.imp().banner_action.borrow().clone();
				if let Some(a) = action {
					a();
				}
			}
		));
		imp.stack.connect_visible_child_name_notify(glib::clone!(
			#[weak(rename_to = win)]
			self,
			move |s| {
				if s.visible_child_name().as_deref() == Some("history") {
					win.reload_history();
				}
			}
		));
		// Send page.
		imp.send_link_entry.connect_changed(glib::clone!(
			#[weak(rename_to = win)]
			self,
			move |_| win.check_send_link()
		));
		imp.paste_link_button.connect_clicked(glib::clone!(
			#[weak(rename_to = win)]
			self,
			move |_| {
				let win = win.clone();
				glib::spawn_future_local(async move {
					if let Ok(Some(text)) = win.clipboard().read_text_future().await {
						win.imp().send_link_entry.set_text(text.trim());
					}
				});
			}
		));
		imp.send_files_row.connect_activated(glib::clone!(
			#[weak(rename_to = win)]
			self,
			move |_| win.choose_to_send(false)
		));
		imp.send_folder_row.connect_activated(glib::clone!(
			#[weak(rename_to = win)]
			self,
			move |_| win.choose_to_send(true)
		));
		self.connect_is_active_notify(|win| {
			if win.is_active() {
				win.imp().unseen.set(0);
				if let Some(app) = win.application() {
					app.withdraw_notification("received");
				}
			}
		});
	}

	fn setup_drop_target(&self) {
		let target = gtk::DropTarget::new(gdk::FileList::static_type(), gdk::DragAction::COPY);
		target.connect_drop(glib::clone!(
			#[weak(rename_to = win)]
			self,
			#[upgrade_or]
			false,
			move |_, value, _, _| {
				let Ok(list) = value.get::<gdk::FileList>() else {
					return false;
				};
				let paths: Vec<std::path::PathBuf> =
					list.files().iter().filter_map(|f| f.path()).collect();
				win.handle_dropped(paths)
			}
		));
		self.add_controller(target);
	}

	/// A dropped folder sets the destination when sharing; dropped files are sent when
	/// the Send page is showing and a link is ready.
	fn handle_dropped(&self, paths: Vec<std::path::PathBuf>) -> bool {
		let imp = self.imp();
		if imp.stack.visible_child_name().as_deref() == Some("send") {
			self.send_paths(paths);
			return true;
		}
		if let [dir] = paths.as_slice()
			&& dir.is_dir()
		{
			let mut cfg = self.bridge().engine().config();
			cfg.storage.dest_dir = dir.clone();
			let win = self.clone();
			let shown = dir.display().to_string();
			glib::spawn_future_local(async move {
				match win.bridge().engine().update_settings(cfg).await {
					Ok(()) => win.toast(&fill(&gettext("Saving to {}"), &[&shown])),
					Err(e) => win.toast(&e.to_string()),
				}
			});
			return true;
		}
		false
	}

	fn clipboard(&self) -> gdk::Clipboard {
		WidgetExt::display(self).clipboard()
	}

	// ----- state ------------------------------------------------------------------

	fn on_event(&self, ev: &Event) {
		match ev {
			Event::StateChanged(s) => self.refresh(s),
			Event::Stopped(reason) => {
				use mailslot_core::events::StopReason::*;
				match reason {
					Expired => self.toast(&gettext("The link expired, so sharing stopped")),
					Timer => self.toast(&gettext("Auto-stop timer reached, so sharing stopped")),
					FileLimit => self.toast(&gettext("File limit reached, so sharing stopped")),
					Error(e) => self.toast(&e.clone()),
					Requested => {}
				}
			}
			Event::TokenRotated => {
				self.toast(&gettext("New link ready. The old one no longer works."))
			}
			Event::Upload(UploadEvent::Finished {
				saved_as,
				size,
				path,
				..
			}) => {
				self.notify_received(saved_as, *size, path);
				self.reload_history_if_visible();
			}
			Event::Upload(UploadEvent::Rejected { .. }) => {}
			Event::Send(SendEvent::Finished { .. } | SendEvent::Skipped { .. }) => {
				self.reload_history_if_visible()
			}
			Event::Internet(_) => self.refresh(&self.bridge().state()),
			Event::SettingsChanged => self.refresh(&self.bridge().engine().state()),
			_ => {}
		}
	}

	/// Updates every widget from a state snapshot.
	pub fn refresh(&self, st: &mailslot_core::events::EngineState) {
		let imp = self.imp();
		imp.updating.set(true);
		let sharing = st.status == Status::Sharing;
		imp.share_stack
			.set_visible_child_name(if sharing { "running" } else { "stopped" });
		let busy = matches!(st.status, Status::Starting | Status::Stopping);
		imp.start_button.set_sensitive(!busy);
		imp.start_button.set_label(&if sharing {
			gettext("Stop sharing")
		} else {
			gettext("Start sharing")
		});
		imp.start_button.remove_css_class("suggested-action");
		imp.start_button.remove_css_class("destructive-action");
		imp.start_button.add_css_class(if sharing {
			"destructive-action"
		} else {
			"suggested-action"
		});

		// Addresses.
		let changed = *imp.urls.borrow() != st.urls;
		if changed {
			let prev = imp.interface_row.selected();
			let labels: Vec<String> = st
				.urls
				.iter()
				.map(|u| {
					if u.host == "127.0.0.1" {
						gettext("This computer (127.0.0.1)")
					} else {
						format!("{} ({})", u.label, u.host)
					}
				})
				.collect();
			let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
			imp.interface_row
				.set_model(Some(&gtk::StringList::new(&refs)));
			if (prev as usize) < labels.len() {
				imp.interface_row.set_selected(prev);
			}
			*imp.urls.borrow_mut() = st.urls.clone();
		}
		imp.updating.set(false);
		self.refresh_link();
		self.refresh_expiry(st);

		// Internet exposure.
		imp.updating.set(true);
		let cfg = self.bridge().engine().config();
		let (open, external_url) = match &st.internet {
			InternetState::Open { url, .. } => (true, Some(url.clone())),
			_ => (false, None),
		};
		imp.internet_row
			.set_active(cfg.network.upnp && st.internet != InternetState::Off || open);
		imp.internet_row.set_sensitive(sharing);
		imp.internet_row.set_subtitle(&match &st.internet {
			InternetState::Mapping => gettext("Asking your router…"),
			InternetState::Open { .. } => {
				gettext("The port is open on your router for this session")
			}
			InternetState::Failed(_) => gettext("Could not open the port"),
			InternetState::Off => gettext(
				"Asks your router to forward the port with UPnP while you share. Off by default.",
			),
		});
		imp.external_row.set_visible(open);
		if let Some(url) = &external_url {
			imp.external_row.set_subtitle(url);
		}
		imp.exposed_badge.set_visible(open);
		imp.updating.set(false);

		// Banner: most urgent message wins.
		match &st.internet {
			InternetState::Failed(d) => self.show_banner(
				&d.message,
				Some(&gettext("Manual Steps")),
				Some(Rc::new(glib::clone!(
					#[weak(rename_to = win)]
					self,
					move || win.show_manual_instructions()
				))),
			),
			InternetState::Open { warnings, .. } if !warnings.is_empty() => self.show_banner(
				&warnings[0].message,
				Some(&gettext("Manual Steps")),
				Some(Rc::new(glib::clone!(
					#[weak(rename_to = win)]
					self,
					move || win.show_manual_instructions()
				))),
			),
			_ if st.restart_required && sharing => self.show_banner(
				&gettext("Settings changed. Restart sharing to apply them."),
				Some(&gettext("Restart")),
				Some(Rc::new(glib::clone!(
					#[weak(rename_to = win)]
					self,
					move || win.restart_sharing()
				))),
			),
			_ => {
				if sharing {
					imp.banner.set_revealed(false);
					*imp.banner_action.borrow_mut() = None;
				}
			}
		}
	}

	fn show_banner(&self, text: &str, button: Option<&str>, action: Option<Rc<dyn Fn()>>) {
		let imp = self.imp();
		imp.banner.set_title(text);
		imp.banner.set_button_label(button);
		*imp.banner_action.borrow_mut() = action;
		imp.banner.set_revealed(true);
	}

	fn selected_url(&self) -> Option<mailslot_core::events::ShareUrl> {
		let imp = self.imp();
		let urls = imp.urls.borrow();
		urls.get(imp.interface_row.selected() as usize)
			.or_else(|| urls.first())
			.cloned()
	}

	fn refresh_link(&self) {
		let imp = self.imp();
		match self.selected_url() {
			Some(u) => {
				imp.link_row.set_subtitle(&u.url);
				match crate::qr::texture_for(&u.url) {
					Some(t) => imp.qr_picture.set_paintable(Some(&t)),
					None => imp.qr_picture.set_paintable(gdk::Paintable::NONE),
				}
			}
			None => {
				imp.link_row.set_subtitle("");
				imp.qr_picture.set_paintable(gdk::Paintable::NONE);
			}
		}
		self.refresh_token_row();
	}

	fn refresh_token_row(&self) {
		let imp = self.imp();
		let Some(b) = imp.bridge.get() else { return };
		let token = b.engine().token();
		let text = if imp.reveal_button.is_active() {
			token.expose().to_owned()
		} else {
			token.masked()
		};
		imp.token_row.set_subtitle(&text);
		imp.token_row.add_css_class("link-url");
		imp.reveal_button
			.set_icon_name(if imp.reveal_button.is_active() {
				"view-conceal-symbolic"
			} else {
				"view-reveal-symbolic"
			});
	}

	fn refresh_expiry(&self, st: &mailslot_core::events::EngineState) {
		let imp = self.imp();
		imp.expiry_row.set_subtitle(&match st.expires_at {
			Some(t) => relative_expiry(t),
			None => gettext("Never"),
		});
	}

	// ----- commands ---------------------------------------------------------------

	fn toggle_sharing(&self) {
		let win = self.clone();
		glib::spawn_future_local(async move {
			let engine = win.bridge().engine().clone();
			match engine.state().status {
				Status::Sharing => {
					let _ = engine.stop().await;
				}
				Status::Stopped => {
					win.imp().banner.set_revealed(false);
					if let Err(e) = engine.start().await {
						win.report_start_error(e).await;
					}
				}
				_ => {}
			}
		});
	}

	async fn report_start_error(&self, e: CoreError) {
		let engine = self.bridge().engine().clone();
		match e {
			CoreError::AddrInUse => {
				let port = engine.config().sharing.port;
				let next = engine.find_free_port(port.saturating_add(1)).await;
				let text = fill(
					&gettext("Port {} is already in use by another program."),
					&[&port.to_string()],
				);
				match next {
					Some(p) => {
						let win = self.clone();
						self.show_banner(
							&text,
							Some(&fill(&gettext("Use Port {}"), &[&p.to_string()])),
							Some(Rc::new(move || {
								let win = win.clone();
								glib::spawn_future_local(async move {
									let mut cfg = win.bridge().engine().config();
									cfg.sharing.port = p;
									if win.bridge().engine().update_settings(cfg).await.is_ok() {
										win.toggle_sharing();
									}
								});
							})),
						);
					}
					None => self.show_banner(&text, None, None),
				}
			}
			CoreError::PermissionDenied => self.show_banner(
				&gettext(
					"Mailslot is not allowed to listen on that port. Choose a port above 1024 in Preferences.",
				),
				None,
				None,
			),
			CoreError::DestNotWritable(p) => self.show_banner(
				&fill(
					&gettext("Cannot write to {}. Choose another folder in Preferences."),
					&[&p.display().to_string()],
				),
				None,
				None,
			),
			other => self.show_banner(&other.to_string(), None, None),
		}
	}

	fn restart_sharing(&self) {
		let win = self.clone();
		glib::spawn_future_local(async move {
			let engine = win.bridge().engine().clone();
			let _ = engine.stop().await;
			if let Err(e) = engine.start().await {
				win.report_start_error(e).await;
			}
		});
	}

	fn confirm_new_link(&self) {
		let win = self.clone();
		glib::spawn_future_local(async move {
			let sharing = win.bridge().state().status == Status::Sharing;
			if sharing {
				let dialog = adw::AlertDialog::new(
					Some(&gettext("Make a new link?")),
					Some(&gettext(
						"The current link stops working immediately and uploads in progress are cancelled.",
					)),
				);
				dialog.add_responses(&[
					("cancel", &gettext("_Cancel")),
					("new", &gettext("_New Link")),
				]);
				dialog.set_response_appearance("new", adw::ResponseAppearance::Destructive);
				dialog.set_close_response("cancel");
				if dialog.choose_future(Some(&win)).await != "new" {
					return;
				}
			}
			if let Err(e) = win.bridge().engine().rotate_token().await {
				win.toast(&e.to_string());
			}
		});
	}

	fn on_internet_toggled(&self, on: bool) {
		let win = self.clone();
		glib::spawn_future_local(async move {
			let engine = win.bridge().engine().clone();
			if on {
				let dialog = adw::AlertDialog::new(
					Some(&gettext("Open to the internet?")),
					Some(&gettext(
						"Your router will forward this port to this computer for the current session, and anyone on the internet who has the link can send you files. The mapping is time-limited and removed when you stop sharing.",
					)),
				);
				dialog.add_responses(&[
					("cancel", &gettext("_Cancel")),
					("open", &gettext("_Open Port")),
				]);
				dialog.set_response_appearance("open", adw::ResponseAppearance::Suggested);
				dialog.set_close_response("cancel");
				if dialog.choose_future(Some(&win)).await != "open" {
					win.imp().updating.set(true);
					win.imp().internet_row.set_active(false);
					win.imp().updating.set(false);
					return;
				}
			}
			if let Err(e) = engine.set_internet_exposed(on).await {
				win.toast(&e.to_string());
			}
		});
	}

	fn show_manual_instructions(&self) {
		let b = self.bridge();
		let port = b
			.state()
			.addr
			.map(|a| a.port())
			.unwrap_or(b.engine().config().sharing.port);
		let dialog = adw::AlertDialog::new(Some(&gettext("Forward the port yourself")), None);
		let body = fill(
			&gettext(
				"1. Open your router's admin page (often the gateway address, such as 192.168.1.1).\n2. Find Port Forwarding (sometimes called Virtual Servers or NAT).\n3. Forward TCP port {} to this computer's local address.\n4. Share the internet link, using your public address in place of the local one.\n5. Remove the forward when you are done.",
			),
			&[&port.to_string()],
		);
		dialog.set_body(&body);
		dialog.add_response("ok", &gettext("_Close"));
		dialog.present(Some(self));
	}

	fn copy_snippet(&self, kind: &str) {
		let Some(ctx) = self.bridge().engine().snippet_context() else {
			self.toast(&gettext("Start sharing first"));
			return;
		};
		let text = match kind {
			"curl" => snippets::curl(&ctx, "FILE"),
			"powershell" => snippets::powershell(&ctx, "FILE"),
			"wget" => snippets::wget(&ctx, "FILE"),
			"cli" => snippets::cli(&ctx, "FILE"),
			"agent" => snippets::agent_prompt(&ctx),
			_ => return,
		};
		self.clipboard().set_text(&text);
		self.toast(&gettext("Copied"));
	}

	fn notify_received(&self, name: &str, size: u64, path: &std::path::Path) {
		let b = self.bridge();
		if !b.engine().config().notifications.notify || self.is_active() {
			return;
		}
		let Some(app) = self.application() else {
			return;
		};
		let n = self.imp().unseen.get() + 1;
		self.imp().unseen.set(n);
		let note = if n == 1 {
			let note = gio::Notification::new(&gettext("File received"));
			note.set_body(Some(&format!("{} · {}", name, glib::format_size(size))));
			note.add_button_with_target_value(
				&gettext("Show in Folder"),
				"app.show-file",
				Some(&path.to_string_lossy().to_string().to_variant()),
			);
			note
		} else {
			let note = gio::Notification::new(&fill(
				&ngettext("{} file received", "{} files received", n),
				&[&n.to_string()],
			));
			note.set_body(Some(&gettext("Open Mailslot to see them")));
			note
		};
		note.set_default_action("app.present");
		app.send_notification(Some("received"), &note);
		*self.imp().last_batch_name.borrow_mut() = name.to_owned();
	}

	// ----- history ----------------------------------------------------------------

	fn reload_history_if_visible(&self) {
		if self.imp().stack.visible_child_name().as_deref() == Some("history") {
			self.reload_history();
		}
	}

	fn reload_history(&self) {
		let win = self.clone();
		glib::spawn_future_local(async move {
			let entries = win
				.bridge()
				.engine()
				.load_history()
				.await
				.unwrap_or_default();
			let imp = win.imp();
			imp.history_list.remove_all();
			for e in entries.iter().rev() {
				let row = adw::ActionRow::new();
				row.set_title(&glib::markup_escape_text(&e.name));
				let when = glib::DateTime::from_iso8601(&e.time, None)
					.ok()
					.and_then(|d| d.to_local().ok())
					.and_then(|d| d.format("%x %H:%M").ok())
					.map(|s| s.to_string())
					.unwrap_or_else(|| e.time.clone());
				let dir = match e.direction {
					mailslot_core::history::Direction::Received => gettext("Received"),
					mailslot_core::history::Direction::Sent => gettext("Sent"),
				};
				row.set_subtitle(&glib::markup_escape_text(&format!(
					"{dir} · {} · {when}",
					glib::format_size(e.size)
				)));
				row.add_prefix(&gtk::Image::from_icon_name(match e.direction {
					mailslot_core::history::Direction::Received => "folder-download-symbolic",
					mailslot_core::history::Direction::Sent => "document-send-symbolic",
				}));
				let copy = gtk::Button::from_icon_name("edit-copy-symbolic");
				copy.set_valign(gtk::Align::Center);
				copy.add_css_class("flat");
				copy.set_tooltip_text(Some(&gettext("Copy SHA-256")));
				let sha = e.sha256.clone();
				copy.connect_clicked(move |b| b.clipboard().set_text(&sha));
				row.add_suffix(&copy);
				imp.history_list.append(&row);
			}
			imp.history_stack
				.set_visible_child_name(if entries.is_empty() { "empty" } else { "list" });
		});
	}

	// ----- send page --------------------------------------------------------------

	fn current_link(&self) -> Option<MailslotLink> {
		MailslotLink::parse(self.imp().send_link_entry.text().trim()).ok()
	}

	fn check_send_link(&self) {
		let imp = self.imp();
		let text = imp.send_link_entry.text().to_string();
		let ticket = imp.send_check.get().wrapping_add(1);
		imp.send_check.set(ticket);
		if text.trim().is_empty() {
			imp.send_status_row
				.set_subtitle(&gettext("Paste a link to begin"));
			imp.send_files_row.set_sensitive(false);
			imp.send_folder_row.set_sensitive(false);
			return;
		}
		let Ok(link) = MailslotLink::parse(text.trim()) else {
			imp.send_status_row
				.set_subtitle(&gettext("That is not a Mailslot link"));
			imp.send_files_row.set_sensitive(false);
			imp.send_folder_row.set_sensitive(false);
			return;
		};
		imp.send_files_row.set_sensitive(true);
		imp.send_folder_row.set_sensitive(true);
		imp.send_status_row.set_subtitle(&fill(
			&gettext("Checking {}…"),
			&[&format!("{}:{}", link.host, link.port)],
		));
		let win = self.clone();
		glib::spawn_future_local(async move {
			let engine = win.bridge().engine().clone();
			let result = engine.check_link(link.clone()).await;
			if win.imp().send_check.get() != ticket {
				return;
			}
			let text = match result {
				Ok(h) if h.accepting => fill(
					&gettext("{} is ready · up to {} per file"),
					&[
						&format!("{}:{}", link.host, link.port),
						glib::format_size(h.max_bytes).as_ref(),
					],
				),
				Ok(_) => gettext("The other side is not accepting files right now"),
				Err(_) => gettext("Could not reach that link"),
			};
			win.imp().send_status_row.set_subtitle(&text);
		});
	}

	fn choose_to_send(&self, folder: bool) {
		let win = self.clone();
		glib::spawn_future_local(async move {
			let dialog = gtk::FileDialog::new();
			let paths: Vec<std::path::PathBuf> = if folder {
				dialog.set_title(&gettext("Choose a folder to send"));
				match dialog.select_folder_future(Some(&win)).await {
					Ok(f) => f.path().into_iter().collect(),
					Err(_) => return,
				}
			} else {
				dialog.set_title(&gettext("Choose files to send"));
				match dialog.open_multiple_future(Some(&win)).await {
					Ok(list) => list
						.iter::<gio::File>()
						.flatten()
						.filter_map(|f| f.path())
						.collect(),
					Err(_) => return,
				}
			};
			win.send_paths(paths);
		});
	}

	/// Sends files or folders to the link in the entry.
	pub fn send_paths(&self, paths: Vec<std::path::PathBuf>) {
		let Some(link) = self.current_link() else {
			self.toast(&gettext("Paste a link first"));
			return;
		};
		let win = self.clone();
		glib::spawn_future_local(async move {
			let req = SendRequest {
				link,
				paths,
				options: Default::default(),
			};
			match win.bridge().engine().send(req).await {
				Ok(ids) if ids.is_empty() => win.toast(&gettext("Nothing to send")),
				Ok(_) => {}
				Err(e) => win.toast(&e.to_string()),
			}
		});
	}
}
