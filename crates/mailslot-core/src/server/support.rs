// Server support types: lockout, rate limiting, free-space probe and the hash index.
//
// (c) Copyright 2026 Liminal HQ, Scott Morris
// SPDX-License-Identifier: Apache-2.0 OR MIT

use std::collections::HashMap;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Reports free disk space. Replace it in tests to simulate a full disk.
pub trait FreeSpace: Send + Sync + 'static {
	/// Bytes available to an unprivileged user on the filesystem holding `dir`.
	fn available(&self, dir: &Path) -> std::io::Result<u64>;
}

/// Asks the operating system with `statvfs`.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemFreeSpace;

impl FreeSpace for SystemFreeSpace {
	fn available(&self, dir: &Path) -> std::io::Result<u64> {
		let s = rustix::fs::statvfs(dir)?;
		Ok(s.f_bavail.saturating_mul(s.f_frsize))
	}
}

/// A free-space provider that always reports the same number, for tests.
#[derive(Clone, Copy, Debug)]
pub struct FixedFreeSpace(pub u64);

impl FreeSpace for FixedFreeSpace {
	fn available(&self, _dir: &Path) -> std::io::Result<u64> {
		Ok(self.0)
	}
}

/// Remembers which SHA-256 values have been received, for `GET have?sha256=`.
#[derive(Debug, Default)]
pub struct HashIndex {
	map: Mutex<HashMap<String, PathBuf>>,
}

impl HashIndex {
	/// Records a received file.
	pub fn insert(&self, sha256: String, path: PathBuf) {
		if let Ok(mut m) = self.map.lock() {
			m.insert(sha256, path);
		}
	}

	/// Looks up where a hash was saved.
	pub fn get(&self, sha256: &str) -> Option<PathBuf> {
		self.map.lock().ok()?.get(sha256).cloned()
	}
}

#[derive(Default)]
struct BadEntry {
	count: u32,
	last: Option<Instant>,
	locked_until: Option<Instant>,
}

/// Counts bad-token requests per address and locks offenders out.
#[derive(Default)]
pub struct Lockout {
	map: Mutex<HashMap<IpAddr, BadEntry>>,
}

impl Lockout {
	/// Whether the address is currently locked out.
	pub fn is_locked(&self, ip: IpAddr) -> bool {
		let Ok(mut m) = self.map.lock() else {
			return false;
		};
		match m.get_mut(&ip) {
			Some(e) => match e.locked_until {
				Some(t) if t > Instant::now() => true,
				Some(_) => {
					m.remove(&ip);
					false
				}
				None => false,
			},
			None => false,
		}
	}

	/// Records a bad-token request. Returns `true` if the address is now locked.
	pub fn note_bad(&self, ip: IpAddr, threshold: u32, lock_for: Duration) -> bool {
		if threshold == 0 {
			return false;
		}
		let Ok(mut m) = self.map.lock() else {
			return false;
		};
		if m.len() > 4096 {
			let now = Instant::now();
			m.retain(|_, e| {
				e.locked_until.is_some_and(|t| t > now)
					|| e.last.is_some_and(|l| now.duration_since(l) < lock_for)
			});
		}
		let e = m.entry(ip).or_default();
		let now = Instant::now();
		if e.last.is_some_and(|l| now.duration_since(l) > lock_for) {
			e.count = 0;
		}
		e.last = Some(now);
		e.count += 1;
		if e.count >= threshold {
			e.locked_until = Some(now + lock_for);
			true
		} else {
			false
		}
	}
}

/// A fixed-window request counter per address.
#[derive(Default)]
pub struct RateLimiter {
	map: Mutex<HashMap<IpAddr, (Instant, u32)>>,
}

impl RateLimiter {
	/// Counts a request. `Err(secs)` is the `Retry-After` value when over the limit.
	pub fn check(&self, ip: IpAddr, per_minute: u32) -> Result<(), u64> {
		if per_minute == 0 {
			return Ok(());
		}
		let Ok(mut m) = self.map.lock() else {
			return Ok(());
		};
		let now = Instant::now();
		if m.len() > 4096 {
			m.retain(|_, (start, _)| now.duration_since(*start) < Duration::from_secs(60));
		}
		let e = m.entry(ip).or_insert((now, 0));
		if now.duration_since(e.0) >= Duration::from_secs(60) {
			*e = (now, 0);
		}
		if e.1 >= per_minute {
			let wait = 60u64
				.saturating_sub(now.duration_since(e.0).as_secs())
				.max(1);
			return Err(wait);
		}
		e.1 += 1;
		Ok(())
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn lockout_triggers_at_threshold() {
		let l = Lockout::default();
		let ip: IpAddr = "10.0.0.1".parse().unwrap();
		for _ in 0..2 {
			assert!(!l.note_bad(ip, 3, Duration::from_secs(60)));
		}
		assert!(!l.is_locked(ip));
		assert!(l.note_bad(ip, 3, Duration::from_secs(60)));
		assert!(l.is_locked(ip));
		assert!(!l.is_locked("10.0.0.2".parse().unwrap()));
	}

	#[test]
	fn rate_limiter_counts_per_address() {
		let r = RateLimiter::default();
		let ip: IpAddr = "10.0.0.1".parse().unwrap();
		assert!(r.check(ip, 2).is_ok());
		assert!(r.check(ip, 2).is_ok());
		assert!(r.check(ip, 2).is_err());
		assert!(r.check("10.0.0.9".parse().unwrap(), 2).is_ok());
	}
}
