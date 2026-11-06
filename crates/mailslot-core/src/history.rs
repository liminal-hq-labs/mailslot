// JSONL history of received and sent files.
//
// (c) Copyright 2026 Liminal HQ, Scott Morris
// SPDX-License-Identifier: Apache-2.0 OR MIT

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use tokio::io::AsyncWriteExt;

use crate::error::Result;

/// Direction of a history entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
	/// A file this machine received.
	Received,
	/// A file this machine sent.
	Sent,
}

/// One line of `history.jsonl`. The token is never recorded.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryEntry {
	/// RFC 3339 time.
	pub time: String,
	/// Direction.
	pub direction: Direction,
	/// File name.
	pub name: String,
	/// Size in bytes.
	pub size: u64,
	/// SHA-256, lowercase hex.
	pub sha256: String,
	/// Peer address (sender for received files, host for sent files).
	#[serde(default)]
	pub peer: Option<String>,
	/// Where the file was saved (received only).
	#[serde(default)]
	pub path: Option<PathBuf>,
}

impl HistoryEntry {
	/// Builds an entry stamped with the current time.
	pub fn now(
		direction: Direction,
		name: String,
		size: u64,
		sha256: String,
		peer: Option<String>,
		path: Option<PathBuf>,
	) -> Self {
		let time = OffsetDateTime::now_utc()
			.format(&Rfc3339)
			.unwrap_or_default();
		Self {
			time,
			direction,
			name,
			size,
			sha256,
			peer,
			path,
		}
	}
}

/// A history file.
#[derive(Clone, Debug)]
pub struct History {
	path: PathBuf,
}

impl History {
	/// Points at a history file (created on first write).
	pub fn new(path: impl Into<PathBuf>) -> Self {
		Self { path: path.into() }
	}

	/// The file path.
	pub fn path(&self) -> &Path {
		&self.path
	}

	/// Appends one entry.
	pub async fn append(&self, entry: &HistoryEntry) -> Result<()> {
		if let Some(dir) = self.path.parent() {
			tokio::fs::create_dir_all(dir).await?;
		}
		let mut line = serde_json::to_string(entry).unwrap_or_default();
		line.push('\n');
		let mut f = tokio::fs::OpenOptions::new()
			.create(true)
			.append(true)
			.open(&self.path)
			.await?;
		f.write_all(line.as_bytes()).await?;
		f.flush().await?;
		Ok(())
	}

	/// Loads every readable entry, oldest first; malformed lines are skipped.
	pub async fn load(&self) -> Result<Vec<HistoryEntry>> {
		let text = match tokio::fs::read_to_string(&self.path).await {
			Ok(t) => t,
			Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
			Err(e) => return Err(e.into()),
		};
		Ok(text
			.lines()
			.filter_map(|l| serde_json::from_str(l).ok())
			.collect())
	}

	/// Drops entries older than `days` and rewrites the file. Returns how many were removed.
	pub async fn prune(&self, days: u32) -> Result<usize> {
		let entries = self.load().await?;
		let cutoff = OffsetDateTime::now_utc() - time::Duration::days(i64::from(days));
		let keep: Vec<_> = entries
			.iter()
			.filter(|e| {
				OffsetDateTime::parse(&e.time, &Rfc3339)
					.map(|t| t >= cutoff)
					.unwrap_or(true)
			})
			.collect();
		let removed = entries.len() - keep.len();
		if removed > 0 {
			let mut text = String::new();
			for e in keep {
				text.push_str(&serde_json::to_string(e).unwrap_or_default());
				text.push('\n');
			}
			let tmp = self.path.with_extension("jsonl.tmp");
			tokio::fs::write(&tmp, text).await?;
			tokio::fs::rename(&tmp, &self.path).await?;
		}
		Ok(removed)
	}

	/// Deletes the history file.
	pub async fn clear(&self) -> Result<()> {
		match tokio::fs::remove_file(&self.path).await {
			Ok(()) => Ok(()),
			Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
			Err(e) => Err(e.into()),
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[tokio::test]
	async fn append_load_prune_roundtrip() {
		let dir = tempfile::tempdir().unwrap();
		let h = History::new(dir.path().join("sub/history.jsonl"));
		assert!(h.load().await.unwrap().is_empty());
		h.append(&HistoryEntry::now(
			Direction::Received,
			"a.txt".into(),
			3,
			"ab".into(),
			Some("1.2.3.4".into()),
			None,
		))
		.await
		.unwrap();
		let mut old =
			HistoryEntry::now(Direction::Sent, "b.txt".into(), 4, "cd".into(), None, None);
		old.time = "2020-01-01T00:00:00Z".into();
		h.append(&old).await.unwrap();
		assert_eq!(h.load().await.unwrap().len(), 2);
		assert_eq!(h.prune(90).await.unwrap(), 1);
		let left = h.load().await.unwrap();
		assert_eq!(left.len(), 1);
		assert_eq!(left[0].name, "a.txt");
		h.clear().await.unwrap();
		assert!(h.load().await.unwrap().is_empty());
	}
}
