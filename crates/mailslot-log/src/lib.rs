// Frontend logging: environment filter, private rotating log file and secret redaction.
//
// (c) Copyright 2026 Liminal HQ, Scott Morris
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Installs the `tracing` subscriber for a Mailslot frontend.
//!
//! `mailslot-core` only emits events; each frontend (the GTK app, `mailslot-cli` and
//! `mailslotd`) calls [`init`] once to decide where they go. Events are written to
//! `$XDG_STATE_HOME/ca.liminalhq.mailslot/mailslot.log` through a non-blocking appender, so
//! logging never blocks a main loop, and optionally to standard error. Every line passes
//! through [`mailslot_core::redact::redact`] first.
//!
//! The filter comes from `MAILSLOT_LOG`, then `RUST_LOG`, then defaults to `info`. For
//! example, `MAILSLOT_LOG=mailslot_core::portmap=debug` shows the UPnP conversation.

#![allow(clippy::disallowed_methods)] // The log file is written from a dedicated worker thread, outside any async runtime.

mod rotate;
mod writer;

use std::path::{Path, PathBuf};

use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

pub use rotate::RotatingFile;
pub use writer::Redacting;

/// The application folder name under the state directory.
pub const APP_ID: &str = mailslot_core::config::APP_ID;
/// File name of the active log.
pub const LOG_FILE: &str = "mailslot.log";
/// Size at which the log rotates: 1 MiB.
pub const MAX_BYTES: u64 = 1024 * 1024;
/// Files kept: the active log and two rotated ones.
pub const KEEP_FILES: usize = 3;

/// Where a frontend sends its logs.
#[derive(Clone, Copy, Debug, Default)]
pub struct Options {
	/// Also write to standard error.
	pub stderr: bool,
}

/// Keeps the background writer alive. Drop it at exit to flush the last lines.
#[must_use = "dropping the guard stops logging and flushes the file"]
pub struct Guard {
	_worker: Option<WorkerGuard>,
	path: Option<PathBuf>,
}

impl Guard {
	/// The log file in use, or `None` when it could not be opened.
	pub fn path(&self) -> Option<&Path> {
		self.path.as_deref()
	}
}

/// The folder that holds the log: `$XDG_STATE_HOME/ca.liminalhq.mailslot`, by default
/// `~/.local/state/ca.liminalhq.mailslot`.
pub fn log_dir() -> PathBuf {
	let base = std::env::var_os("XDG_STATE_HOME")
		.map(PathBuf::from)
		.filter(|p| p.is_absolute())
		.or_else(|| {
			directories::BaseDirs::new().map(|b| {
				b.state_dir()
					.map_or_else(|| b.home_dir().join(".local/state"), Path::to_path_buf)
			})
		})
		.unwrap_or_else(|| PathBuf::from("."));
	base.join(APP_ID)
}

/// Path of the active log file.
pub fn log_path() -> PathBuf {
	log_dir().join(LOG_FILE)
}

/// The filter text: `MAILSLOT_LOG`, then `RUST_LOG`, then `info`.
pub fn filter_text() -> String {
	["MAILSLOT_LOG", "RUST_LOG"]
		.iter()
		.find_map(|k| std::env::var(k).ok().filter(|v| !v.trim().is_empty()))
		.unwrap_or_else(|| "info".to_owned())
}

/// Whether the log level was chosen explicitly through the environment.
pub fn filter_is_set() -> bool {
	std::env::var("MAILSLOT_LOG").is_ok_and(|v| !v.trim().is_empty())
}

/// Installs the global subscriber. A second call, or a subscriber installed by an embedder,
/// is left alone. A log file that cannot be opened falls back to standard error with a note.
pub fn init(opts: Options) -> Guard {
	let filter = EnvFilter::try_new(filter_text()).unwrap_or_else(|_| EnvFilter::new("info"));
	let path = log_path();
	let (file, worker, open_error) = match RotatingFile::open(&path, MAX_BYTES, KEEP_FILES) {
		Ok(f) => {
			let (w, g) = tracing_appender::non_blocking(f);
			(Some(w), Some(g), None)
		}
		Err(e) => (None, None, Some(e)),
	};
	let opened = file.is_some();
	let stderr = opts.stderr || file.is_none();
	let file_layer = file.map(|w| {
		tracing_subscriber::fmt::layer()
			.with_ansi(false)
			.with_writer(Redacting::new(w))
	});
	let stderr_layer = stderr.then(|| {
		tracing_subscriber::fmt::layer()
			.with_ansi(false)
			.with_writer(Redacting::new(std::io::stderr))
	});
	let _ = tracing_subscriber::registry()
		.with(filter)
		.with(file_layer)
		.with(stderr_layer)
		.try_init();
	if let Some(e) = open_error {
		tracing::warn!(path = %path.display(), error = %e, "could not open the log file; logging to standard error");
	}
	Guard {
		_worker: worker,
		path: opened.then_some(path),
	}
}

/// The newest `count` lines of the log, oldest first, with secrets removed. Reads the
/// rotated file too when the active one is short. Blocking: call it off the main loop.
pub fn read_tail(count: usize) -> Vec<String> {
	read_tail_from(&log_path(), count)
}

/// [`read_tail`] for an explicit log path.
pub fn read_tail_from(path: &Path, count: usize) -> Vec<String> {
	let mut lines: Vec<String> = Vec::new();
	for candidate in [rotate::rotated_name(path, 1), path.to_path_buf()] {
		if let Ok(text) = std::fs::read_to_string(&candidate) {
			lines.extend(text.lines().map(str::to_owned));
		}
	}
	let start = lines.len().saturating_sub(count);
	lines[start..]
		.iter()
		.map(|l| mailslot_core::redact::redact(l))
		.collect()
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn tail_joins_rotated_and_active_and_redacts() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join(LOG_FILE);
		std::fs::write(rotate::rotated_name(&path, 1), "old 1\nold 2\n").unwrap();
		std::fs::write(
			&path,
			"new 1\nGET /AbCdEfGhIjKlMnOpQrStUvWxYz0123456789-_AbCd/x\n",
		)
		.unwrap();
		let t = read_tail_from(&path, 3);
		assert_eq!(t.len(), 3);
		assert_eq!(t[0], "old 2");
		assert!(!t[2].contains("AbCdEf"), "{}", t[2]);
	}
}
