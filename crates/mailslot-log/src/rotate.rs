// A size-capped log file with simple numbered rotation, created private.
//
// (c) Copyright 2026 Liminal HQ, Scott Morris
// SPDX-License-Identifier: Apache-2.0 OR MIT

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

/// The name of rotated file `n` (`mailslot.log.1` is the newest).
pub(crate) fn rotated_name(path: &Path, n: usize) -> PathBuf {
	let mut s = path.as_os_str().to_owned();
	s.push(format!(".{n}"));
	PathBuf::from(s)
}

/// A file that rotates at `max_bytes`, keeping `keep` files in total: the active file plus
/// `keep - 1` rotated ones. Files are created with mode 0600.
pub struct RotatingFile {
	path: PathBuf,
	file: File,
	size: u64,
	max_bytes: u64,
	keep: usize,
}

fn open_private(path: &Path) -> io::Result<File> {
	OpenOptions::new()
		.create(true)
		.append(true)
		.mode(0o600)
		.open(path)
}

impl RotatingFile {
	/// Opens (creating the folder if needed) the active log for appending.
	pub fn open(path: &Path, max_bytes: u64, keep: usize) -> io::Result<Self> {
		if let Some(dir) = path.parent() {
			std::fs::create_dir_all(dir)?;
		}
		let file = open_private(path)?;
		let size = file.metadata()?.len();
		Ok(Self {
			path: path.to_path_buf(),
			file,
			size,
			max_bytes,
			keep: keep.max(1),
		})
	}

	fn rotate(&mut self) -> io::Result<()> {
		self.file.flush()?;
		if self.keep > 1 {
			let oldest = rotated_name(&self.path, self.keep - 1);
			let _ = std::fs::remove_file(&oldest);
			for n in (1..self.keep - 1).rev() {
				let _ =
					std::fs::rename(rotated_name(&self.path, n), rotated_name(&self.path, n + 1));
			}
			std::fs::rename(&self.path, rotated_name(&self.path, 1))?;
		} else {
			std::fs::remove_file(&self.path)?;
		}
		self.file = open_private(&self.path)?;
		self.size = 0;
		Ok(())
	}
}

impl Write for RotatingFile {
	fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
		if self.size > 0 && self.size + buf.len() as u64 > self.max_bytes {
			self.rotate()?;
		}
		let n = self.file.write(buf)?;
		self.size += n as u64;
		Ok(n)
	}

	fn flush(&mut self) -> io::Result<()> {
		self.file.flush()
	}
}

#[cfg(test)]
mod tests {
	use std::os::unix::fs::PermissionsExt;

	use super::*;

	#[test]
	fn rotates_at_the_cap_and_keeps_three_files() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("mailslot.log");
		let mut f = RotatingFile::open(&path, 100, 3).unwrap();
		for i in 0..40 {
			// One write per line, as the log writer does, so a line never splits across files.
			f.write_all(format!("line {i:03} {}\n", "x".repeat(20)).as_bytes())
				.unwrap();
		}
		f.flush().unwrap();
		let names: Vec<_> = std::fs::read_dir(dir.path())
			.unwrap()
			.map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
			.collect();
		assert_eq!(names.len(), 3, "{names:?}");
		for name in ["mailslot.log", "mailslot.log.1", "mailslot.log.2"] {
			assert!(names.iter().any(|n| n == name), "{name} missing: {names:?}");
		}
		for name in &names {
			let meta = std::fs::metadata(dir.path().join(name)).unwrap();
			assert!(meta.len() <= 100, "{name} is {} bytes", meta.len());
			assert_eq!(meta.permissions().mode() & 0o777, 0o600, "{name}");
		}
		// The newest line is in the active file and the oldest lines are gone.
		let active = std::fs::read_to_string(&path).unwrap();
		assert!(active.contains("line 039"));
		let all: String = names
			.iter()
			.map(|n| std::fs::read_to_string(dir.path().join(n)).unwrap())
			.collect();
		assert!(!all.contains("line 000"));
	}

	#[test]
	fn appends_to_an_existing_file() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("mailslot.log");
		std::fs::write(&path, "before\n").unwrap();
		let mut f = RotatingFile::open(&path, 1024, 3).unwrap();
		writeln!(f, "after").unwrap();
		f.flush().unwrap();
		assert_eq!(std::fs::read_to_string(&path).unwrap(), "before\nafter\n");
	}
}
