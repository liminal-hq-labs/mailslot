// Error types shared across the crate.
//
// (c) Copyright 2026 Liminal HQ, Scott Morris
// SPDX-License-Identifier: Apache-2.0 OR MIT

use std::path::PathBuf;

/// Errors returned by the engine and its modules.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
	/// The address is already in use by another program.
	#[error("the address is already in use")]
	AddrInUse,
	/// The operating system refused to bind the address.
	#[error("permission denied while binding the address")]
	PermissionDenied,
	/// The destination folder cannot be created or written.
	#[error("the destination folder {0} is not writable")]
	DestNotWritable(PathBuf),
	/// A token was malformed.
	#[error("the token is not valid")]
	InvalidToken,
	/// A link could not be parsed.
	#[error("the link is not valid: {0}")]
	InvalidLink(String),
	/// The configuration holds an unusable value.
	#[error("invalid setting: {0}")]
	InvalidConfig(String),
	/// The operating system could not supply random bytes.
	#[error("random source failed: {0}")]
	Random(String),
	/// The engine is not in a state where the command makes sense.
	#[error("{0}")]
	State(String),
	/// A feature was not compiled in.
	#[error("the `{0}` feature is not enabled")]
	FeatureDisabled(&'static str),
	/// TLS material could not be created or loaded.
	#[error("tls: {0}")]
	Tls(String),
	/// Sending a file failed.
	#[error("send failed: {0}")]
	Send(String),
	/// Any other input or output failure.
	#[error(transparent)]
	Io(#[from] std::io::Error),
}

impl Error {
	pub(crate) fn from_bind(e: std::io::Error) -> Self {
		match e.kind() {
			std::io::ErrorKind::AddrInUse => Error::AddrInUse,
			std::io::ErrorKind::PermissionDenied => Error::PermissionDenied,
			_ => Error::Io(e),
		}
	}
}

/// Convenience result alias.
pub type Result<T, E = Error> = std::result::Result<T, E>;
