// Shared helpers for the integration tests.
//
// (c) Copyright 2026 Liminal HQ, Scott Morris
// SPDX-License-Identifier: Apache-2.0 OR MIT

#![allow(dead_code)]
#![allow(clippy::disallowed_methods)] // Tests may use blocking std calls for set-up and assertions.

pub mod fake_igd;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use mailslot_core::server::{
	FixedFreeSpace, Server, ServerConfig, ServerEvent, ServerHandle, ServerSettings,
};
use mailslot_core::token::Token;
use sha2::{Digest, Sha256};
use tempfile::TempDir;
use tokio::sync::mpsc::UnboundedReceiver;

pub struct Fixture {
	pub dir: TempDir,
	pub handle: ServerHandle,
	pub addr: SocketAddr,
	pub token: Token,
	pub events: UnboundedReceiver<ServerEvent>,
	pub http: reqwest::Client,
}

impl Fixture {
	pub async fn start() -> Self {
		Self::with(|_| {}).await
	}

	pub async fn with(tweak: impl FnOnce(&mut ServerConfig)) -> Self {
		mailslot_core::tls::ensure_default_provider();
		let dir = tempfile::tempdir().unwrap();
		let token = Token::generate().unwrap();
		let mut cfg = ServerConfig::new(
			"127.0.0.1:0".parse().unwrap(),
			dir.path().join("dest"),
			token.clone(),
		);
		cfg.settings = ServerSettings {
			min_free_bytes: 0,
			lockout_threshold: 0,
			rate_limit_per_min: 0,
			..ServerSettings::default()
		};
		cfg.free_space = Arc::new(FixedFreeSpace(u64::MAX / 2));
		tweak(&mut cfg);
		let (tx, events) = tokio::sync::mpsc::unbounded_channel();
		let handle = Server::start(cfg, tx).await.unwrap();
		let addr = handle.local_addr();
		let http = reqwest::Client::builder()
			.redirect(reqwest::redirect::Policy::none())
			.build()
			.unwrap();
		Self {
			dir,
			handle,
			addr,
			token,
			events,
			http,
		}
	}

	pub fn dest(&self) -> PathBuf {
		self.dir.path().join("dest")
	}

	pub fn url(&self, rest: &str) -> String {
		format!("http://{}/{}/{}", self.addr, self.token.expose(), rest)
	}

	pub fn raw_url(&self, path: &str) -> String {
		format!("http://{}/{}", self.addr, path)
	}

	/// Names of regular files in the destination (excluding directories).
	pub fn files(&self) -> Vec<String> {
		let mut v: Vec<String> = std::fs::read_dir(self.dest())
			.map(|rd| {
				rd.filter_map(|e| e.ok())
					.filter(|e| e.path().is_file())
					.map(|e| e.file_name().to_string_lossy().into_owned())
					.collect()
			})
			.unwrap_or_default();
		v.sort();
		v
	}

	pub fn parts(&self) -> Vec<String> {
		self.files()
			.into_iter()
			.filter(|n| n.ends_with(".part"))
			.collect()
	}

	/// Waits until no `.part` files remain (cleanup after an abort is asynchronous).
	pub async fn wait_no_parts(&self) -> bool {
		for _ in 0..100 {
			if self.parts().is_empty() {
				return true;
			}
			tokio::time::sleep(std::time::Duration::from_millis(30)).await;
		}
		false
	}
}

pub fn sha256_hex(data: &[u8]) -> String {
	hex::encode(Sha256::digest(data))
}

pub fn pattern(len: usize) -> Vec<u8> {
	(0..len).map(|i| (i.wrapping_mul(31) % 251) as u8).collect()
}
