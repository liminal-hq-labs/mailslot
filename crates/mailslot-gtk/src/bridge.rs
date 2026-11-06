// Bridge between the engine's event stream and GTK list models.
//
// (c) Copyright 2026 Liminal HQ, Scott Morris
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The bridge owns the [`Engine`], consumes its event stream on the GLib main loop and
//! keeps `gio::ListStore<UploadItem>` models current. All GObjects stay on the main
//! thread; the engine's runtime only ever sees plain data.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Instant;

use futures_util::StreamExt;
use gtk::gio;
use gtk::glib;
use gtk::prelude::*;
use mailslot_core::Engine;
use mailslot_core::events::{
	EngineState, Event, SendEvent, Status, UploadEvent, UploadSnapshot, UploadState,
};
use mailslot_core::naming;

use crate::model::{Direction, UploadItem};

type Observer = Rc<dyn Fn(&Event)>;

struct Inner {
	engine: Engine,
	uploads: gio::ListStore,
	sends: gio::ListStore,
	incoming_index: RefCell<HashMap<u64, UploadItem>>,
	outgoing_index: RefCell<HashMap<u64, UploadItem>>,
	last_sample: RefCell<HashMap<(bool, u64), (Instant, u64)>>,
	state: RefCell<EngineState>,
	observers: RefCell<Vec<Observer>>,
}

/// Cheap to clone; clones share state.
#[derive(Clone)]
pub struct Bridge(Rc<Inner>);

impl Bridge {
	/// Wraps an engine. Call [`Bridge::run`] to start consuming its events.
	pub fn new(engine: Engine) -> Self {
		let state = engine.state();
		Self(Rc::new(Inner {
			engine,
			uploads: gio::ListStore::new::<UploadItem>(),
			sends: gio::ListStore::new::<UploadItem>(),
			incoming_index: RefCell::default(),
			outgoing_index: RefCell::default(),
			last_sample: RefCell::default(),
			state: RefCell::new(state),
			observers: RefCell::default(),
		}))
	}

	/// The engine.
	pub fn engine(&self) -> &Engine {
		&self.0.engine
	}

	/// Received files.
	pub fn uploads(&self) -> &gio::ListStore {
		&self.0.uploads
	}

	/// Outgoing transfers.
	pub fn sends(&self) -> &gio::ListStore {
		&self.0.sends
	}

	/// The latest state snapshot seen.
	pub fn state(&self) -> EngineState {
		self.0.state.borrow().clone()
	}

	/// Registers a callback that runs after each event has updated the models.
	pub fn connect(&self, f: impl Fn(&Event) + 'static) {
		self.0.observers.borrow_mut().push(Rc::new(f));
	}

	/// Consumes engine events on the main loop until the stream ends.
	pub fn run(&self) {
		let this = self.clone();
		glib::spawn_future_local(async move {
			let mut events = this.0.engine.subscribe();
			while let Some(ev) = events.next().await {
				this.apply(&ev);
			}
		});
	}

	fn risky(&self, name: &str) -> bool {
		self.0.engine.config().security.warn_on_executables && naming::is_executable_like(name)
	}

	fn fraction(received: u64, expected: u64) -> f64 {
		if expected == 0 {
			0.0
		} else {
			(received as f64 / expected as f64).clamp(0.0, 1.0)
		}
	}

	fn sample_speed(&self, outgoing: bool, id: u64, bytes: u64, item: &UploadItem) {
		let mut samples = self.0.last_sample.borrow_mut();
		let now = Instant::now();
		match samples.get(&(outgoing, id)) {
			Some((at, prev)) => {
				let dt = now.duration_since(*at).as_secs_f64();
				if dt >= 0.4 && bytes >= *prev {
					let inst = (bytes - prev) as f64 / dt;
					let smooth = if item.speed() > 0.0 {
						item.speed() * 0.6 + inst * 0.4
					} else {
						inst
					};
					item.set_speed(smooth);
					samples.insert((outgoing, id), (now, bytes));
				}
			}
			None => {
				samples.insert((outgoing, id), (now, bytes));
			}
		}
	}

	/// Applies one event to the models and notifies observers. Public so tests can feed
	/// synthetic events.
	pub fn apply(&self, ev: &Event) {
		match ev {
			Event::StateChanged(s) => {
				*self.0.state.borrow_mut() = (**s).clone();
				if s.status == Status::Stopped {
					for i in 0..self.0.uploads.n_items() {
						if let Some(item) = self.0.uploads.item(i).and_downcast::<UploadItem>()
							&& item.state() == "receiving"
						{
							item.set_state("failed");
							item.set_reason(crate::i18n::gettext("Sharing stopped"));
							item.set_speed(0.0);
						}
					}
				}
			}
			Event::Upload(u) => self.apply_upload(u),
			Event::Send(s) => self.apply_send(s),
			Event::Lagged(_) => self.resync(),
			_ => {}
		}
		let observers: Vec<Observer> = self.0.observers.borrow().clone();
		for o in observers {
			o(ev);
		}
	}

	fn apply_upload(&self, ev: &UploadEvent) {
		let mut index = self.0.incoming_index.borrow_mut();
		match ev {
			UploadEvent::Started {
				id,
				name,
				expected,
				peer,
			} => {
				let item = UploadItem::new(*id, name, *expected, peer, Direction::Incoming);
				item.set_risky(self.risky(name));
				index.insert(*id, item.clone());
				self.0.uploads.insert(0, &item);
			}
			UploadEvent::Progress { id, received } => {
				if let Some(item) = index.get(id) {
					item.set_received(*received);
					item.set_fraction(Self::fraction(*received, item.expected()));
					self.sample_speed(false, *id, *received, item);
				}
			}
			UploadEvent::Finished {
				id,
				path,
				saved_as,
				size,
				sha256,
				..
			} => {
				if let Some(item) = index.get(id) {
					item.set_received(*size);
					item.set_expected(*size);
					item.set_fraction(1.0);
					item.set_speed(0.0);
					item.set_saved_as(saved_as.as_str());
					item.set_path(path.to_string_lossy().as_ref());
					item.set_sha256(sha256.as_str());
					item.set_risky(self.risky(saved_as));
					item.set_state("done");
				}
			}
			UploadEvent::Failed { id, reason, .. } => {
				if let Some(item) = index.get(id) {
					item.set_reason(reason.as_str());
					item.set_speed(0.0);
					item.set_state("failed");
				}
			}
			UploadEvent::Rejected { .. } => {}
		}
	}

	fn apply_send(&self, ev: &SendEvent) {
		let mut index = self.0.outgoing_index.borrow_mut();
		match ev {
			SendEvent::Started { id, name, total } => {
				let item = UploadItem::new(*id, name, Some(*total), "", Direction::Outgoing);
				index.insert(*id, item.clone());
				self.0.sends.insert(0, &item);
			}
			SendEvent::Progress {
				id,
				sent,
				total,
				bytes_per_sec,
			} => {
				if let Some(item) = index.get(id) {
					item.set_received(*sent);
					item.set_fraction(Self::fraction(*sent, *total));
					item.set_speed(*bytes_per_sec);
				}
			}
			SendEvent::Skipped { id } => {
				if let Some(item) = index.get(id) {
					item.set_fraction(1.0);
					item.set_speed(0.0);
					item.set_state("skipped");
				}
			}
			SendEvent::Retrying { id, attempt, .. } => {
				if let Some(item) = index.get(id) {
					item.set_reason(crate::i18n::fill(
						&crate::i18n::gettext("Retrying (attempt {})"),
						&[&attempt.to_string()],
					));
				}
			}
			SendEvent::Finished {
				id,
				sha256,
				saved_as,
			} => {
				if let Some(item) = index.get(id) {
					item.set_fraction(1.0);
					item.set_speed(0.0);
					item.set_sha256(sha256.as_str());
					item.set_saved_as(saved_as.as_str());
					item.set_state("done");
				}
			}
			SendEvent::Failed { id, reason } => {
				if let Some(item) = index.get(id) {
					item.set_reason(reason.as_str());
					item.set_speed(0.0);
					item.set_state("failed");
				}
			}
		}
	}

	/// Rebuilds the models from the engine's snapshot after missed events.
	pub fn resync(&self) {
		let st = self.0.engine.state();
		*self.0.state.borrow_mut() = st.clone();
		self.0.uploads.remove_all();
		self.0.incoming_index.borrow_mut().clear();
		for u in st.uploads.iter().rev() {
			let item = Self::item_from_snapshot(u);
			item.set_risky(self.risky(&u.name));
			self.0
				.incoming_index
				.borrow_mut()
				.insert(u.id, item.clone());
			self.0.uploads.insert(0, &item);
		}
	}

	fn item_from_snapshot(u: &UploadSnapshot) -> UploadItem {
		let item = UploadItem::new(u.id, &u.name, u.expected, &u.peer, Direction::Incoming);
		item.set_received(u.received);
		item.set_fraction(u.fraction());
		item.set_state(match u.state {
			UploadState::Receiving => "receiving",
			UploadState::Done => "done",
			UploadState::Failed => "failed",
		});
		if let Some(s) = &u.sha256 {
			item.set_sha256(s.as_str());
		}
		if let Some(p) = &u.path {
			item.set_path(p.to_string_lossy().as_ref());
		}
		if let Some(r) = &u.reason {
			item.set_reason(r.as_str());
		}
		if let Some(n) = &u.saved_as {
			item.set_saved_as(n.as_str());
		}
		item
	}

	/// Removes finished rows from both lists and from the engine's snapshot.
	pub fn clear_finished(&self) {
		for (store, index) in [
			(&self.0.uploads, &self.0.incoming_index),
			(&self.0.sends, &self.0.outgoing_index),
		] {
			let mut i = 0;
			while i < store.n_items() {
				let finished = store
					.item(i)
					.and_downcast::<UploadItem>()
					.is_some_and(|it| it.state() != "receiving");
				if finished {
					if let Some(it) = store.item(i).and_downcast::<UploadItem>() {
						index.borrow_mut().remove(&it.id());
					}
					store.remove(i);
				} else {
					i += 1;
				}
			}
		}
		self.0.engine.clear_finished();
	}
}
