// Settings with defaults, TOML persistence and per-area reset.
//
// (c) Copyright 2026 Liminal HQ, Scott Morris
// SPDX-License-Identifier: Apache-2.0 OR MIT

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::token::Token;

/// Application identifier, used for directory names.
pub const APP_ID: &str = "ca.liminalhq.mailslot";

/// Where configuration and data live.
#[derive(Clone, Debug)]
pub struct Paths {
	/// Holds `config.toml` and the `token` file.
	pub config_dir: PathBuf,
	/// Holds `history.jsonl` and `tls/`.
	pub data_dir: PathBuf,
}

impl Paths {
	/// The XDG locations for the current user.
	pub fn xdg() -> Self {
		let base = directories::BaseDirs::new();
		let config = base
			.as_ref()
			.map(|b| b.config_dir().to_path_buf())
			.unwrap_or_else(|| PathBuf::from("."));
		let data = base
			.as_ref()
			.map(|b| b.data_dir().to_path_buf())
			.unwrap_or_else(|| PathBuf::from("."));
		Self {
			config_dir: config.join(APP_ID),
			data_dir: data.join(APP_ID),
		}
	}

	/// Keeps everything under one directory, which suits tests and embedding.
	pub fn in_dir(base: impl AsRef<Path>) -> Self {
		let base = base.as_ref();
		Self {
			config_dir: base.join("config"),
			data_dir: base.join("data"),
		}
	}

	/// Path of `config.toml`.
	pub fn config_file(&self) -> PathBuf {
		self.config_dir.join("config.toml")
	}
	/// Path of the token file.
	pub fn token_file(&self) -> PathBuf {
		self.config_dir.join("token")
	}
	/// Path of the upload history.
	pub fn history_file(&self) -> PathBuf {
		self.data_dir.join("history.jsonl")
	}
	/// Directory for the self-signed certificate.
	pub fn tls_dir(&self) -> PathBuf {
		self.data_dir.join("tls")
	}
}

/// How received files are grouped inside the destination.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubfolderScheme {
	/// Everything in the destination folder itself.
	#[default]
	None,
	/// One folder per UTC day, named `YYYY-MM-DD`.
	ByDate,
	/// One folder per sender label (`X-Mailslot-Sender`).
	BySender,
}

/// What to do when the target name already exists.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Collision {
	/// Add `-1`, `-2` and so on. Nothing is ever overwritten.
	#[default]
	Suffix,
	/// Refuse the upload with `409`.
	Reject,
}

/// Sharing settings.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Sharing {
	/// Listening port; `0` picks a free one.
	pub port: u16,
	/// Address to bind (`0.0.0.0` for all interfaces).
	pub bind: String,
	/// Start sharing when the app launches.
	pub start_on_launch: bool,
	/// Minutes until the link expires; `0` means never.
	pub expiry_minutes: u32,
	/// Stop after this many files; `0` means no limit (`1` is one-shot mode).
	pub stop_after_files: u32,
	/// Stop sharing after this many minutes; `0` means no timer.
	pub auto_stop_minutes: u32,
	/// Names this link. It travels in `mailslot://` links, and senders using it are
	/// filed under this name when subfolders are by sender label.
	pub link_label: String,
}

impl Default for Sharing {
	fn default() -> Self {
		Self {
			port: crate::proto::DEFAULT_PORT,
			bind: "0.0.0.0".into(),
			start_on_launch: false,
			expiry_minutes: 60,
			stop_after_files: 0,
			auto_stop_minutes: 0,
			link_label: String::new(),
		}
	}
}

/// Storage settings.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Storage {
	/// Destination folder.
	pub dest_dir: PathBuf,
	/// Subfolder scheme.
	pub subfolder: SubfolderScheme,
	/// Collision behaviour.
	pub collision: Collision,
	/// Write a `<name>.sha256` file beside each received file.
	pub sidecars: bool,
	/// Re-read each file from disk after writing and compare its hash.
	pub verify_on_receive: bool,
}

impl Default for Storage {
	fn default() -> Self {
		let base = directories::UserDirs::new()
			.and_then(|d| d.download_dir().map(Path::to_path_buf))
			.or_else(|| directories::BaseDirs::new().map(|b| b.home_dir().join("Downloads")))
			.unwrap_or_else(|| PathBuf::from("Downloads"));
		Self {
			dest_dir: base.join("Mailslot"),
			subfolder: SubfolderScheme::None,
			collision: Collision::Suffix,
			sidecars: false,
			verify_on_receive: false,
		}
	}
}

/// Limit settings.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Limits {
	/// Largest accepted file, in megabytes.
	pub max_file_mb: u64,
	/// Total accepted per session, in megabytes; `0` means no quota.
	pub session_quota_mb: u64,
	/// Refuse uploads that would leave less than this much free space, in megabytes.
	pub min_free_mb: u64,
	/// Uploads handled at once.
	pub max_concurrent: u32,
	/// Requests per minute per client address; `0` disables the limit.
	pub rate_limit_per_min: u32,
	/// Seconds a body may stall before the upload is aborted.
	pub idle_timeout_secs: u64,
}

impl Default for Limits {
	fn default() -> Self {
		Self {
			max_file_mb: 200,
			session_quota_mb: 0,
			min_free_mb: 1024,
			max_concurrent: 4,
			rate_limit_per_min: 600,
			idle_timeout_secs: 60,
		}
	}
}

/// Security settings.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Security {
	/// Bad-token requests from one address before it is locked out; `0` disables.
	pub bad_token_lockout: u32,
	/// Allowed file extensions (lowercase, no dot); empty allows everything.
	pub allowed_extensions: Vec<String>,
	/// Serve over HTTPS with a self-signed certificate.
	pub https: bool,
	/// Flag executables and scripts in the UI.
	pub warn_on_executables: bool,
}

impl Default for Security {
	fn default() -> Self {
		Self {
			bad_token_lockout: 20,
			allowed_extensions: Vec::new(),
			https: false,
			warn_on_executables: true,
		}
	}
}

/// The shortest router mapping lease, in minutes.
pub const UPNP_LEASE_MIN_MINUTES: u32 = 5;
/// The longest router mapping lease, in minutes. Mappings are never permanent.
pub const UPNP_LEASE_MAX_MINUTES: u32 = 60;

/// Network settings.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Network {
	/// Open the port on the router with UPnP while sharing (remembered between sessions).
	pub upnp: bool,
	/// Router mapping lease in minutes, from 5 to 60. It is also capped by the link expiry.
	pub upnp_lease_minutes: u32,
	/// Allow the public-IP lookup button to make one request.
	pub public_ip_lookup: bool,
	/// Interface (name or address) to advertise in the link; empty means automatic.
	pub interface: String,
	/// Announce this share on the local network with mDNS while sharing. The secret is
	/// never part of the announcement.
	pub mdns: bool,
}

impl Default for Network {
	fn default() -> Self {
		Self {
			upnp: false,
			upnp_lease_minutes: UPNP_LEASE_MAX_MINUTES,
			public_ip_lookup: false,
			interface: String::new(),
			mdns: false,
		}
	}
}

impl Network {
	/// The mapping lease, with the configured minutes held inside the allowed range.
	pub fn upnp_lease(&self) -> std::time::Duration {
		let m = self
			.upnp_lease_minutes
			.clamp(UPNP_LEASE_MIN_MINUTES, UPNP_LEASE_MAX_MINUTES);
		std::time::Duration::from_secs(u64::from(m) * 60)
	}
}

/// Notification and history settings.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Notifications {
	/// Notify when an upload completes.
	pub notify: bool,
	/// Keep a history of received and sent files.
	pub keep_history: bool,
	/// Days to keep history entries.
	pub history_days: u32,
}

impl Default for Notifications {
	fn default() -> Self {
		Self {
			notify: true,
			keep_history: true,
			history_days: 90,
		}
	}
}

/// The settings areas, one per Preferences page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Section {
	/// [`Sharing`].
	Sharing,
	/// [`Storage`].
	Storage,
	/// [`Limits`].
	Limits,
	/// [`Security`].
	Security,
	/// [`Network`].
	Network,
	/// [`Notifications`].
	Notifications,
}

/// All settings. Every key has a default, so a partial file is fine.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
	/// Sharing.
	pub sharing: Sharing,
	/// Storage.
	pub storage: Storage,
	/// Limits.
	pub limits: Limits,
	/// Security.
	pub security: Security,
	/// Network.
	pub network: Network,
	/// Notifications and history.
	pub notifications: Notifications,
}

impl Config {
	/// Restores one area to its defaults.
	pub fn reset_section(&mut self, section: Section) {
		match section {
			Section::Sharing => self.sharing = Sharing::default(),
			Section::Storage => self.storage = Storage::default(),
			Section::Limits => self.limits = Limits::default(),
			Section::Security => self.security = Security::default(),
			Section::Network => self.network = Network::default(),
			Section::Notifications => self.notifications = Notifications::default(),
		}
	}

	/// Restores everything to defaults.
	pub fn reset(&mut self) {
		*self = Self::default();
	}

	/// Checks values that would stop the server from working.
	pub fn validate(&self) -> Result<()> {
		if self.sharing.bind.parse::<std::net::IpAddr>().is_err() {
			return Err(Error::InvalidConfig(format!(
				"`{}` is not an IP address",
				self.sharing.bind
			)));
		}
		if self.limits.max_concurrent == 0 {
			return Err(Error::InvalidConfig(
				"concurrent uploads must be at least 1".into(),
			));
		}
		if self.limits.max_file_mb == 0 {
			return Err(Error::InvalidConfig(
				"maximum file size must be at least 1 MB".into(),
			));
		}
		if !(UPNP_LEASE_MIN_MINUTES..=UPNP_LEASE_MAX_MINUTES)
			.contains(&self.network.upnp_lease_minutes)
		{
			return Err(Error::InvalidConfig(format!(
				"the UPnP lease must be between {UPNP_LEASE_MIN_MINUTES} and {UPNP_LEASE_MAX_MINUTES} minutes"
			)));
		}
		Ok(())
	}

	/// Parses TOML text.
	pub fn from_toml(text: &str) -> Result<Self> {
		toml::from_str(text).map_err(|e| Error::InvalidConfig(e.to_string()))
	}

	/// Renders TOML text.
	pub fn to_toml(&self) -> String {
		toml::to_string_pretty(self).unwrap_or_default()
	}

	/// Loads `config.toml`, returning defaults when the file does not exist.
	pub async fn load(paths: &Paths) -> Result<Self> {
		match tokio::fs::read_to_string(paths.config_file()).await {
			Ok(text) => Self::from_toml(&text),
			Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
			Err(e) => Err(e.into()),
		}
	}

	/// Writes `config.toml`, creating the directory as needed.
	pub async fn save(&self, paths: &Paths) -> Result<()> {
		tokio::fs::create_dir_all(&paths.config_dir).await?;
		let tmp = paths.config_dir.join("config.toml.tmp");
		tokio::fs::write(&tmp, self.to_toml()).await?;
		tokio::fs::rename(&tmp, paths.config_file()).await?;
		Ok(())
	}
}

/// Loads the persisted token, or creates one with file mode `0600`.
pub async fn load_or_create_token(paths: &Paths) -> Result<Token> {
	match tokio::fs::read_to_string(paths.token_file()).await {
		Ok(text) => {
			if let Ok(t) = Token::parse(&text) {
				return Ok(t);
			}
		}
		Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.into()),
		Err(_) => {}
	}
	let token = Token::generate()?;
	save_token(paths, &token).await?;
	Ok(token)
}

/// Persists a token with file mode `0600`.
pub async fn save_token(paths: &Paths, token: &Token) -> Result<()> {
	use tokio::io::AsyncWriteExt;
	tokio::fs::create_dir_all(&paths.config_dir).await?;
	let tmp = paths.config_dir.join("token.tmp");
	let mut opts = tokio::fs::OpenOptions::new();
	opts.write(true).create(true).truncate(true);
	#[cfg(unix)]
	opts.mode(0o600);
	let mut f = opts.open(&tmp).await?;
	f.write_all(token.expose().as_bytes()).await?;
	f.flush().await?;
	drop(f);
	tokio::fs::rename(&tmp, paths.token_file()).await?;
	Ok(())
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn defaults_follow_the_plan() {
		let c = Config::default();
		assert_eq!(c.sharing.port, 8931);
		assert_eq!(c.sharing.expiry_minutes, 60);
		assert!(!c.sharing.start_on_launch);
		assert_eq!(c.limits.max_file_mb, 200);
		assert_eq!(c.limits.min_free_mb, 1024);
		assert_eq!(c.limits.max_concurrent, 4);
		assert!(c.storage.dest_dir.ends_with("Downloads/Mailslot"));
		assert!(c.notifications.keep_history);
		assert_eq!(c.notifications.history_days, 90);
		assert!(!c.network.upnp && !c.network.public_ip_lookup);
		assert_eq!(c.network.upnp_lease_minutes, 60);
	}

	#[test]
	fn upnp_lease_is_validated_clamped_and_reset() {
		let mut c = Config::default();
		assert_eq!(c.network.upnp_lease(), std::time::Duration::from_secs(3600));
		for bad in [0, 4, 61, 1000] {
			c.network.upnp_lease_minutes = bad;
			assert!(c.validate().is_err(), "{bad} should be refused");
		}
		for ok in [5, 30, 60] {
			c.network.upnp_lease_minutes = ok;
			assert!(c.validate().is_ok(), "{ok} should be accepted");
		}
		// A hand-edited file outside the range is held inside it when used.
		let c = Config::from_toml("[network]\nupnp_lease_minutes = 600\n").unwrap();
		assert_eq!(c.network.upnp_lease(), std::time::Duration::from_secs(3600));
		let c = Config::from_toml("[network]\nupnp_lease_minutes = 1\n").unwrap();
		assert_eq!(c.network.upnp_lease(), std::time::Duration::from_secs(300));
		// It round-trips through TOML and resets with its section.
		let mut c = Config::default();
		c.network.upnp_lease_minutes = 15;
		let back = Config::from_toml(&c.to_toml()).unwrap();
		assert_eq!(back.network.upnp_lease_minutes, 15);
		c.reset_section(Section::Network);
		assert_eq!(c.network.upnp_lease_minutes, 60);
	}

	#[test]
	fn partial_toml_fills_in_defaults() {
		let c = Config::from_toml("[sharing]\nport = 9000\n").unwrap();
		assert_eq!(c.sharing.port, 9000);
		assert_eq!(c.limits, Limits::default());
		assert_eq!(Config::from_toml(&c.to_toml()).unwrap(), c);
	}

	#[test]
	fn reset_restores_sections() {
		let mut c = Config::default();
		c.limits.max_file_mb = 5;
		c.sharing.port = 1;
		c.reset_section(Section::Limits);
		assert_eq!(c.limits, Limits::default());
		assert_eq!(c.sharing.port, 1);
		c.reset();
		assert_eq!(c, Config::default());
	}

	#[tokio::test]
	async fn token_file_is_private_and_stable() {
		let dir = tempfile::tempdir().unwrap();
		let paths = Paths::in_dir(dir.path());
		let a = load_or_create_token(&paths).await.unwrap();
		let b = load_or_create_token(&paths).await.unwrap();
		assert_eq!(a, b);
		#[cfg(unix)]
		{
			use std::os::unix::fs::PermissionsExt;
			let mode = tokio::fs::metadata(paths.token_file())
				.await
				.unwrap()
				.permissions()
				.mode();
			assert_eq!(mode & 0o777, 0o600);
		}
	}
}
