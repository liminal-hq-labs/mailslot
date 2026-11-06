// Debounced folder watching on top of inotify and the platform equivalents.
//
// (c) Copyright 2026 Liminal HQ, Scott Morris
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! [`DirWatcher`] reports files that were created or changed under a folder once they
//! have stopped changing. Events are debounced, and a file is only reported after two
//! size checks a debounce interval apart agree, so half-written files are not sent.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use notify::{EventKind, RecursiveMode, Watcher};
use tokio::sync::mpsc;

use crate::error::{Error, Result};

/// Watches a folder tree.
pub struct DirWatcher {
	rx: mpsc::UnboundedReceiver<PathBuf>,
	_watcher: notify::RecommendedWatcher,
}

fn ignored(path: &Path) -> bool {
	let name = path
		.file_name()
		.map(|n| n.to_string_lossy().into_owned())
		.unwrap_or_default();
	name.starts_with('.')
		|| name.ends_with('~')
		|| [".part", ".crdownload", ".tmp", ".swp", ".partial"]
			.iter()
			.any(|s| name.ends_with(s))
}

impl DirWatcher {
	/// Starts watching `dir` recursively. Call from inside a tokio runtime.
	pub fn start(dir: &Path, debounce: Duration) -> Result<Self> {
		let (raw_tx, mut raw_rx) = mpsc::unbounded_channel::<PathBuf>();
		let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
			// notify calls this on its own thread; sending never blocks.
			if let Ok(ev) = res
				&& matches!(ev.kind, EventKind::Create(_) | EventKind::Modify(_))
			{
				for p in ev.paths {
					let _ = raw_tx.send(p);
				}
			}
		})
		.map_err(|e| Error::State(format!("watch: {e}")))?;
		watcher
			.watch(dir, RecursiveMode::Recursive)
			.map_err(|e| Error::State(format!("watch: {e}")))?;

		let (out_tx, rx) = mpsc::unbounded_channel();
		tokio::spawn(async move {
			// path -> (last event, size seen at the previous check)
			let mut seen: HashMap<PathBuf, (Instant, Option<u64>)> = HashMap::new();
			let mut tick = tokio::time::interval(Duration::from_millis(100).min(debounce));
			loop {
				tokio::select! {
					ev = raw_rx.recv() => match ev {
						Some(p) if !ignored(&p) => {
							seen.insert(p, (Instant::now(), None));
						}
						Some(_) => {}
						None => break,
					},
					_ = tick.tick() => {
						let due: Vec<PathBuf> = seen.iter().filter(|(_, (t, _))| t.elapsed() >= debounce).map(|(p, _)| p.clone()).collect();
						for p in due {
							let Ok(meta) = tokio::fs::metadata(&p).await else {
								seen.remove(&p);
								continue;
							};
							if !meta.is_file() {
								seen.remove(&p);
								continue;
							}
							let prev = seen.get(&p).and_then(|(_, s)| *s);
							if prev == Some(meta.len()) {
								seen.remove(&p);
								if out_tx.send(p).is_err() {
									return;
								}
							} else {
								seen.insert(p, (Instant::now(), Some(meta.len())));
							}
						}
					}
				}
			}
		});
		Ok(Self {
			rx,
			_watcher: watcher,
		})
	}

	/// The next settled file, or `None` when the watcher has stopped.
	pub async fn next(&mut self) -> Option<PathBuf> {
		self.rx.recv().await
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[tokio::test]
	async fn reports_new_and_changed_files_once_they_settle() {
		let dir = tempfile::tempdir().unwrap();
		let mut w = DirWatcher::start(dir.path(), Duration::from_millis(200)).unwrap();
		tokio::fs::write(dir.path().join("a.txt"), "hello")
			.await
			.unwrap();
		tokio::fs::write(dir.path().join(".hidden"), "x")
			.await
			.unwrap();
		tokio::fs::write(dir.path().join("b.part"), "x")
			.await
			.unwrap();
		let got = tokio::time::timeout(Duration::from_secs(5), w.next())
			.await
			.expect("event")
			.unwrap();
		assert_eq!(got.file_name().unwrap(), "a.txt");
		// A file written in several steps is reported once, after the last write.
		tokio::fs::create_dir(dir.path().join("sub")).await.unwrap();
		let big = dir.path().join("sub/big.bin");
		for _ in 0..3 {
			let mut f = tokio::fs::OpenOptions::new()
				.create(true)
				.append(true)
				.open(&big)
				.await
				.unwrap();
			tokio::io::AsyncWriteExt::write_all(&mut f, &[1u8; 1000])
				.await
				.unwrap();
			tokio::time::sleep(Duration::from_millis(60)).await;
		}
		let got = tokio::time::timeout(Duration::from_secs(5), w.next())
			.await
			.expect("event")
			.unwrap();
		assert!(got.ends_with("big.bin"));
		assert_eq!(tokio::fs::metadata(&got).await.unwrap().len(), 3000);
		assert!(
			tokio::time::timeout(Duration::from_millis(600), w.next())
				.await
				.is_err(),
			"nothing else is reported"
		);
	}
}
