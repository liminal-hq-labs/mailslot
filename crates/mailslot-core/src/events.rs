// Typed events and state snapshots exposed to frontends.
//
// (c) Copyright 2026 Liminal HQ, Scott Morris
// SPDX-License-Identifier: Apache-2.0 OR MIT

use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

/// Identifier of one received upload, unique within an engine.
pub type UploadId = u64;
/// Identifier of one outgoing transfer, unique within an engine.
pub type SendId = u64;

/// Lifecycle of the share.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Status {
	/// Not listening.
	Stopped,
	/// Binding and, if requested, mapping the port.
	Starting,
	/// Accepting uploads.
	Sharing,
	/// Shutting down.
	Stopping,
}

/// Why sharing ended.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum StopReason {
	/// The user stopped it.
	Requested,
	/// The link expired.
	Expired,
	/// The auto-stop timer fired.
	Timer,
	/// The file-count limit was reached.
	FileLimit,
	/// The server failed.
	Error(String),
}

/// Progress of one received file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum UploadState {
	/// Bytes are arriving.
	Receiving,
	/// Saved and verified.
	Done,
	/// Aborted, rejected or failed.
	Failed,
}

/// A received (or receiving) file as the UI shows it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UploadSnapshot {
	/// Identifier.
	pub id: UploadId,
	/// Name the sender asked for, sanitized.
	pub name: String,
	/// Name as saved, once finished.
	pub saved_as: Option<String>,
	/// Final path, once finished.
	pub path: Option<PathBuf>,
	/// Bytes received so far.
	pub received: u64,
	/// Declared size, when known.
	pub expected: Option<u64>,
	/// State.
	pub state: UploadState,
	/// SHA-256 as lowercase hex, once finished.
	pub sha256: Option<String>,
	/// Sender address.
	pub peer: String,
	/// Failure reason.
	pub reason: Option<String>,
}

impl UploadSnapshot {
	/// Fraction complete between 0 and 1 (1 when done, 0 when the size is unknown).
	pub fn fraction(&self) -> f64 {
		match (self.state, self.expected) {
			(UploadState::Done, _) => 1.0,
			(_, Some(0)) => 1.0,
			(_, Some(e)) => (self.received as f64 / e as f64).clamp(0.0, 1.0),
			_ => 0.0,
		}
	}
}

/// Events about received files, produced by the server.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum UploadEvent {
	/// A body started arriving.
	Started {
		/// Identifier.
		id: UploadId,
		/// Sanitized requested name.
		name: String,
		/// Declared size.
		expected: Option<u64>,
		/// Sender address.
		peer: String,
	},
	/// More bytes arrived (throttled to about 10 per second per upload).
	Progress {
		/// Identifier.
		id: UploadId,
		/// Bytes so far.
		received: u64,
	},
	/// The file was saved.
	Finished {
		/// Identifier.
		id: UploadId,
		/// Final path.
		path: PathBuf,
		/// Final file name.
		saved_as: String,
		/// Size in bytes.
		size: u64,
		/// SHA-256, lowercase hex.
		sha256: String,
		/// Sender address.
		peer: String,
	},
	/// The upload ended without a file.
	Failed {
		/// Identifier.
		id: UploadId,
		/// Human-readable reason.
		reason: String,
		/// Whether a partial file was kept for resuming.
		resumable: bool,
	},
	/// A request was turned away before an upload began.
	Rejected {
		/// Sender address.
		peer: String,
		/// HTTP status returned.
		status: u16,
		/// Reason.
		reason: String,
	},
}

/// Classification of a port-mapping problem.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DiagnosticKind {
	/// No gateway answered.
	NoGateway,
	/// A gateway answered but refused mappings (UPnP likely switched off).
	MappingRefused,
	/// The gateway's external address is private: double NAT.
	DoubleNat,
	/// The gateway's external address is in 100.64.0.0/10: carrier-grade NAT.
	Cgnat,
	/// The external port is taken by something else.
	PortConflict,
	/// The gateway's external address differs from the public address seen from outside.
	AddressMismatch,
	/// Anything else.
	Other,
}

/// A human-readable diagnosis.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diagnostic {
	/// Class of problem.
	pub kind: DiagnosticKind,
	/// Message fit for display.
	pub message: String,
	/// What was known about the router and the address used when this was produced.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub details: Option<Box<InternetDetails>>,
}

/// Facts about a router mapping attempt, for diagnostics. Never contains the token.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct InternetDetails {
	/// The router's address.
	pub gateway: Option<IpAddr>,
	/// Manufacturer and model from the router's device description, when it gave one.
	pub router: Option<String>,
	/// The local address the router was asked to forward to.
	pub internal_client: Option<IpAddr>,
	/// The interface that address belongs to.
	pub interface: Option<String>,
	/// The router's raw error code, when it refused a request.
	pub error_code: Option<u16>,
	/// The router's raw error text.
	pub error_text: Option<String>,
}

/// State of internet exposure through the router.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum InternetState {
	/// Not requested.
	#[default]
	Off,
	/// Asking the router.
	Mapping,
	/// Reachable from outside.
	Open {
		/// External address and port as seen from the internet.
		external: SocketAddr,
		/// Ready-to-share URL.
		url: String,
		/// Warnings that did not stop the mapping (for instance a suspected double NAT).
		warnings: Vec<Diagnostic>,
	},
	/// The attempt failed.
	Failed(Diagnostic),
}

/// Events about the router mapping.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum InternetEvent {
	/// The state changed.
	State(InternetState),
	/// A stale mapping from an earlier run was removed.
	StaleRemoved {
		/// External port that was freed.
		external_port: u16,
	},
}

/// Events about outgoing transfers.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum SendEvent {
	/// A transfer began.
	Started {
		/// Identifier.
		id: SendId,
		/// File name.
		name: String,
		/// Size in bytes.
		total: u64,
	},
	/// Progress.
	Progress {
		/// Identifier.
		id: SendId,
		/// Bytes sent (including bytes the server already held).
		sent: u64,
		/// Size in bytes.
		total: u64,
		/// Recent throughput.
		bytes_per_sec: f64,
	},
	/// The server already had the file.
	Skipped {
		/// Identifier.
		id: SendId,
	},
	/// Waiting before another attempt.
	Retrying {
		/// Identifier.
		id: SendId,
		/// Attempt number about to start (2 or more).
		attempt: u32,
		/// Seconds of delay.
		delay_secs: f64,
	},
	/// The transfer completed.
	Finished {
		/// Identifier.
		id: SendId,
		/// SHA-256 confirmed by the server.
		sha256: String,
		/// Name as saved by the server.
		saved_as: String,
	},
	/// The transfer failed.
	Failed {
		/// Identifier.
		id: SendId,
		/// Reason.
		reason: String,
	},
}

/// An outgoing transfer as the UI shows it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SendSnapshot {
	/// Identifier.
	pub id: SendId,
	/// File name.
	pub name: String,
	/// Size in bytes.
	pub total: u64,
	/// Bytes sent.
	pub sent: u64,
	/// `true` once the transfer ended.
	pub finished: bool,
	/// `true` if the server already had the file.
	pub skipped: bool,
	/// Failure reason.
	pub error: Option<String>,
	/// SHA-256 from the server.
	pub sha256: Option<String>,
}

/// One URL a sender could use, for one local address.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShareUrl {
	/// Label such as `Wi-Fi (wlan0)` or `This computer`.
	pub label: String,
	/// Host or address in the URL.
	pub host: String,
	/// Full drop URL, including the token.
	pub url: String,
	/// Full `mailslot://` link.
	pub link: String,
	/// Kind of virtual interface (`Docker`, `VPN`, …) this address sits on, which other
	/// devices usually cannot reach. `None` for a physical interface.
	#[serde(default)]
	pub virtual_kind: Option<String>,
	/// The system sends internet traffic from this address.
	#[serde(default)]
	pub is_default_route: bool,
}

impl std::fmt::Debug for ShareUrl {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		f.debug_struct("ShareUrl")
			.field("label", &self.label)
			.field("host", &self.host)
			.finish_non_exhaustive()
	}
}

/// A snapshot of everything a frontend needs to draw itself.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EngineState {
	/// Lifecycle.
	pub status: Status,
	/// Bound address while sharing.
	pub addr: Option<SocketAddr>,
	/// Candidate URLs, best first.
	pub urls: Vec<ShareUrl>,
	/// When the link expires.
	pub expires_at: Option<SystemTime>,
	/// Key pin when HTTPS is on.
	pub tls_pin: Option<String>,
	/// Router exposure.
	pub internet: InternetState,
	/// What is known about the router and the address used for the latest attempt.
	#[serde(default)]
	pub internet_details: Option<InternetDetails>,
	/// Received files this session.
	pub uploads: Vec<UploadSnapshot>,
	/// Outgoing transfers this session.
	pub sends: Vec<SendSnapshot>,
	/// Files received this session.
	pub received_files: u64,
	/// Bytes received this session.
	pub received_bytes: u64,
	/// Settings changed that only apply after a restart of sharing.
	pub restart_required: bool,
	/// Uploads are being refused with `503` until resumed.
	pub paused: bool,
	/// Advertising on the local network with mDNS.
	pub advertising: bool,
}

impl Default for EngineState {
	fn default() -> Self {
		Self {
			status: Status::Stopped,
			addr: None,
			urls: Vec::new(),
			expires_at: None,
			tls_pin: None,
			internet: InternetState::Off,
			internet_details: None,
			uploads: Vec::new(),
			sends: Vec::new(),
			received_files: 0,
			received_bytes: 0,
			restart_required: false,
			paused: false,
			advertising: false,
		}
	}
}

/// Anything the engine reports.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Event {
	/// Lifecycle or URLs changed; the snapshot is current.
	StateChanged(Box<EngineState>),
	/// Sharing ended.
	Stopped(StopReason),
	/// The token was replaced; old links no longer work.
	TokenRotated,
	/// Settings were saved.
	SettingsChanged,
	/// A received-file event.
	Upload(UploadEvent),
	/// A router-mapping event.
	Internet(InternetEvent),
	/// An outgoing-transfer event.
	Send(SendEvent),
	/// The receiver fell behind and missed events; call `state()` to resynchronize.
	Lagged(u64),
}

#[cfg(feature = "server")]
impl EngineState {
	pub(crate) fn apply_upload(&mut self, ev: &UploadEvent) {
		match ev {
			UploadEvent::Started {
				id,
				name,
				expected,
				peer,
			} => {
				self.uploads.push(UploadSnapshot {
					id: *id,
					name: name.clone(),
					saved_as: None,
					path: None,
					received: 0,
					expected: *expected,
					state: UploadState::Receiving,
					sha256: None,
					peer: peer.clone(),
					reason: None,
				});
			}
			UploadEvent::Progress { id, received } => {
				if let Some(u) = self.uploads.iter_mut().find(|u| u.id == *id) {
					u.received = *received;
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
				if let Some(u) = self.uploads.iter_mut().find(|u| u.id == *id) {
					u.state = UploadState::Done;
					u.received = *size;
					u.expected = Some(*size);
					u.saved_as = Some(saved_as.clone());
					u.path = Some(path.clone());
					u.sha256 = Some(sha256.clone());
				}
				self.received_files += 1;
				self.received_bytes += size;
			}
			UploadEvent::Failed { id, reason, .. } => {
				if let Some(u) = self.uploads.iter_mut().find(|u| u.id == *id) {
					u.state = UploadState::Failed;
					u.reason = Some(reason.clone());
				}
			}
			UploadEvent::Rejected { .. } => {}
		}
	}

	pub(crate) fn apply_send(&mut self, ev: &SendEvent) {
		let find = |s: &mut EngineState, id: SendId| s.sends.iter().position(|t| t.id == id);
		match ev {
			SendEvent::Started { id, name, total } => self.sends.push(SendSnapshot {
				id: *id,
				name: name.clone(),
				total: *total,
				sent: 0,
				finished: false,
				skipped: false,
				error: None,
				sha256: None,
			}),
			SendEvent::Progress { id, sent, .. } => {
				if let Some(i) = find(self, *id) {
					self.sends[i].sent = *sent;
				}
			}
			SendEvent::Skipped { id } => {
				if let Some(i) = find(self, *id) {
					let t = &mut self.sends[i];
					t.skipped = true;
					t.finished = true;
					t.sent = t.total;
				}
			}
			SendEvent::Finished { id, sha256, .. } => {
				if let Some(i) = find(self, *id) {
					let t = &mut self.sends[i];
					t.finished = true;
					t.sent = t.total;
					t.sha256 = Some(sha256.clone());
				}
			}
			SendEvent::Failed { id, reason } => {
				if let Some(i) = find(self, *id) {
					let t = &mut self.sends[i];
					t.finished = true;
					t.error = Some(reason.clone());
				}
			}
			SendEvent::Retrying { .. } => {}
		}
	}
}
