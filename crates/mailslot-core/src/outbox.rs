// A persisted queue of files to send, so a watched folder survives restarts.
//
// (c) Copyright 2026 Liminal HQ, Scott Morris
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The outbox remembers which files were already sent (by size and modification time)
//! and which are waiting, in a small JSON file. Files are still deduplicated by content
//! hash on the server, so a stale or missing outbox only costs a re-hash.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::Result;

/// A file's size and modification time, used to notice changes cheaply.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stamp {
	/// Size in bytes.
	pub len: u64,
	/// Modification time in nanoseconds since the Unix epoch.
	pub modified_ns: u128,
}

impl Stamp {
	/// Reads the stamp of a file.
	pub async fn of(path: &Path) -> std::io::Result<Self> {
		let m = tokio::fs::metadata(path).await?;
		let modified_ns = m
			.modified()
			.ok()
			.and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
			.map_or(0, |d| d.as_nanos());
		Ok(Self {
			len: m.len(),
			modified_ns,
		})
	}
}

#[derive(Default, Serialize, Deserialize)]
struct State {
	done: BTreeMap<PathBuf, Stamp>,
	pending: Vec<PathBuf>,
	#[serde(default)]
	attempts: BTreeMap<PathBuf, u32>,
}

/// The queue, backed by a file.
pub struct Outbox {
	file: PathBuf,
	state: State,
}

impl Outbox {
	/// Opens (or starts) the queue stored in `file`. An unreadable file starts empty.
	pub async fn open(file: impl Into<PathBuf>) -> Result<Self> {
		let file = file.into();
		let state = match tokio::fs::read_to_string(&file).await {
			Ok(text) => serde_json::from_str(&text).unwrap_or_default(),
			Err(e) if e.kind() == std::io::ErrorKind::NotFound => State::default(),
			Err(e) => return Err(e.into()),
		};
		Ok(Self { file, state })
	}

	/// The queue file.
	pub fn file(&self) -> &Path {
		&self.file
	}

	async fn save(&self) -> Result<()> {
		if let Some(dir) = self.file.parent() {
			tokio::fs::create_dir_all(dir).await?;
		}
		let tmp = self.file.with_extension("json.tmp");
		tokio::fs::write(
			&tmp,
			serde_json::to_vec_pretty(&self.state).unwrap_or_default(),
		)
		.await?;
		tokio::fs::rename(&tmp, &self.file).await?;
		Ok(())
	}

	/// Queues the file unless it was already sent unchanged. Returns whether it was queued.
	pub async fn enqueue_if_changed(&mut self, path: &Path) -> Result<bool> {
		let Ok(stamp) = Stamp::of(path).await else {
			return Ok(false);
		};
		if self.state.done.get(path) == Some(&stamp) || self.state.pending.iter().any(|p| p == path)
		{
			return Ok(false);
		}
		self.state.pending.push(path.to_path_buf());
		self.save().await?;
		Ok(true)
	}

	/// Records files as already handled without sending them.
	pub async fn mark_done(&mut self, paths: &[PathBuf]) -> Result<()> {
		for p in paths {
			if let Ok(stamp) = Stamp::of(p).await {
				self.state.done.insert(p.clone(), stamp);
			}
		}
		self.save().await
	}

	/// The next file to send.
	pub fn next(&self) -> Option<PathBuf> {
		self.state.pending.first().cloned()
	}

	/// How many files are waiting.
	pub fn pending_len(&self) -> usize {
		self.state.pending.len()
	}

	/// Failed attempts so far for a queued file.
	pub fn attempts(&self, path: &Path) -> u32 {
		self.state.attempts.get(path).copied().unwrap_or(0)
	}

	/// Marks the file sent and removes it from the queue.
	pub async fn complete(&mut self, path: &Path) -> Result<()> {
		self.state.pending.retain(|p| p != path);
		self.state.attempts.remove(path);
		if let Ok(stamp) = Stamp::of(path).await {
			self.state.done.insert(path.to_path_buf(), stamp);
		}
		self.save().await
	}

	/// Moves a file that failed to the back of the queue and counts the attempt.
	pub async fn defer(&mut self, path: &Path) -> Result<()> {
		self.state.pending.retain(|p| p != path);
		self.state.pending.push(path.to_path_buf());
		*self.state.attempts.entry(path.to_path_buf()).or_insert(0) += 1;
		self.save().await
	}

	/// Drops a queued file that no longer exists.
	pub async fn forget(&mut self, path: &Path) -> Result<()> {
		self.state.pending.retain(|p| p != path);
		self.state.attempts.remove(path);
		self.save().await
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[tokio::test]
	async fn queue_survives_a_restart_and_skips_unchanged_files() {
		let dir = tempfile::tempdir().unwrap();
		let (a, b) = (dir.path().join("a.txt"), dir.path().join("b.txt"));
		tokio::fs::write(&a, "one").await.unwrap();
		tokio::fs::write(&b, "two").await.unwrap();
		let file = dir.path().join("state/outbox.json");
		let mut q = Outbox::open(&file).await.unwrap();
		assert!(q.enqueue_if_changed(&a).await.unwrap());
		assert!(q.enqueue_if_changed(&b).await.unwrap());
		assert!(!q.enqueue_if_changed(&a).await.unwrap(), "already queued");
		q.complete(&a).await.unwrap();
		drop(q);

		let mut q = Outbox::open(&file).await.unwrap();
		assert_eq!(q.pending_len(), 1);
		assert_eq!(q.next().as_deref(), Some(b.as_path()));
		assert!(!q.enqueue_if_changed(&a).await.unwrap(), "sent unchanged");
		tokio::fs::write(&a, "changed content").await.unwrap();
		assert!(
			q.enqueue_if_changed(&a).await.unwrap(),
			"changed files are queued again"
		);
		q.defer(&b).await.unwrap();
		assert_eq!(q.attempts(&b), 1);
		assert_eq!(q.next().as_deref(), Some(a.as_path()));
		q.forget(&a).await.unwrap();
		assert_eq!(q.pending_len(), 1);
	}

	#[tokio::test]
	async fn a_corrupt_file_starts_empty() {
		let dir = tempfile::tempdir().unwrap();
		let file = dir.path().join("o.json");
		tokio::fs::write(&file, "{ not json").await.unwrap();
		assert_eq!(Outbox::open(&file).await.unwrap().pending_len(), 0);
	}
}
