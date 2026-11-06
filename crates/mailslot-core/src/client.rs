// Resumable upload client: dedupe, retries with backoff, parallel files and bandwidth limit.
//
// (c) Copyright 2026 Liminal HQ, Scott Morris
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A client for Mailslot servers.
//!
//! [`Client::send_file`] hashes the file, asks the server whether it already has it,
//! then uploads from wherever a previous attempt stopped. The upload identifier is
//! derived from the content hash and name, so even a restarted process resumes.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use reqwest::StatusCode;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio::sync::{Mutex, Semaphore};

use crate::error::{Error, Result};
use crate::proto::{self, MailslotLink};

const CHUNK: usize = 64 * 1024;

/// Client behaviour.
#[derive(Clone, Debug)]
pub struct ClientOptions {
	/// Files uploaded at once by [`Client::send_many`].
	pub parallel: usize,
	/// Retries after the first attempt.
	pub retries: u32,
	/// First backoff delay; doubles each retry.
	pub backoff_base: Duration,
	/// Longest backoff delay.
	pub backoff_max: Duration,
	/// Total bandwidth cap in bytes per second across all files.
	pub bandwidth_limit: Option<u64>,
	/// Ask the server whether it already has the file.
	pub dedupe: bool,
	/// Label the server can use for per-sender folders.
	pub sender_label: Option<String>,
	/// Accept any certificate on an HTTPS link without a pin.
	pub insecure: bool,
	/// Overall request timeout per attempt (`None` for no limit).
	pub request_timeout: Option<Duration>,
}

impl Default for ClientOptions {
	fn default() -> Self {
		Self {
			parallel: 2,
			retries: 5,
			backoff_base: Duration::from_millis(500),
			backoff_max: Duration::from_secs(15),
			bandwidth_limit: None,
			dedupe: true,
			sender_label: None,
			insecure: false,
			request_timeout: None,
		}
	}
}

/// What the server reports about itself.
#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize)]
pub struct Health {
	/// Always `true` when reachable.
	pub ok: bool,
	/// Largest accepted file.
	pub max_bytes: u64,
	/// Link expiry as RFC 3339.
	pub expires_at: Option<String>,
	/// Whether uploads are currently accepted.
	pub accepting: bool,
	/// Features such as `resume` and `dedupe`.
	#[serde(default)]
	pub capabilities: Vec<String>,
}

/// A progress report.
#[derive(Clone, Debug, PartialEq)]
pub struct Progress {
	/// File name.
	pub name: String,
	/// Bytes the server holds, including earlier attempts.
	pub sent: u64,
	/// File size.
	pub total: u64,
	/// Recent throughput.
	pub bytes_per_sec: f64,
}

/// Something that happened to a transfer that is not progress.
#[derive(Clone, Debug, PartialEq)]
pub enum Notice {
	/// Waiting before the given attempt number.
	Retrying {
		/// Attempt about to start.
		attempt: u32,
		/// Delay in seconds.
		delay_secs: f64,
		/// Why the last attempt failed.
		reason: String,
	},
	/// The upload resumed from this offset.
	Resumed {
		/// Bytes the server already held.
		offset: u64,
	},
}

/// The result of sending one file.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct SendOutcome {
	/// Local file name used in the request.
	pub name: String,
	/// Name the server saved it as (empty when skipped).
	pub saved_as: String,
	/// SHA-256 of the file.
	pub sha256: String,
	/// Size in bytes.
	pub size: u64,
	/// `true` when the server already had the file and nothing was sent.
	pub skipped: bool,
}

type ProgressFn = Arc<dyn Fn(Progress) + Send + Sync>;
type NoticeFn = Arc<dyn Fn(Notice) + Send + Sync>;

struct Limiter {
	rate: u64,
	next: Mutex<Instant>,
}

impl Limiter {
	async fn take(&self, n: usize) {
		let mut next = self.next.lock().await;
		let now = Instant::now();
		if *next > now {
			tokio::time::sleep(*next - now).await;
		}
		let base = (*next).max(now);
		*next = base + Duration::from_secs_f64(n as f64 / self.rate as f64);
	}
}

/// A connection to one share.
#[derive(Clone)]
pub struct Client {
	http: reqwest::Client,
	link: MailslotLink,
	opts: ClientOptions,
	limiter: Option<Arc<Limiter>>,
	on_progress: Option<ProgressFn>,
	on_notice: Option<NoticeFn>,
}

impl std::fmt::Debug for Client {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		f.debug_struct("Client")
			.field("host", &self.link.host)
			.field("port", &self.link.port)
			.finish_non_exhaustive()
	}
}

enum Attempt {
	Done {
		saved_as: String,
		sha256: String,
	},
	Retry {
		reason: String,
		retry_after: Option<Duration>,
		offset: Option<u64>,
	},
	Fatal(String),
}

impl Client {
	/// Builds a client for a link.
	pub fn new(link: MailslotLink, opts: ClientOptions) -> Result<Self> {
		let mut b = reqwest::Client::builder()
			.connect_timeout(Duration::from_secs(10))
			.pool_max_idle_per_host(2);
		if let Some(t) = opts.request_timeout {
			b = b.timeout(t);
		}
		if link.tls {
			#[cfg(feature = "tls")]
			{
				b = b.tls_backend_preconfigured(crate::tls::client_config(
					link.pin.clone(),
					opts.insecure,
				));
			}
			#[cfg(not(feature = "tls"))]
			return Err(Error::FeatureDisabled("tls"));
		} else {
			#[cfg(feature = "tls")]
			{
				// Build without touching the system verifier, which needs a default provider.
				b = b.tls_backend_preconfigured(crate::tls::client_config(None, true));
			}
		}
		let http = b.build().map_err(|e| Error::Send(e.to_string()))?;
		let limiter = opts.bandwidth_limit.filter(|r| *r > 0).map(|rate| {
			Arc::new(Limiter {
				rate,
				next: Mutex::new(Instant::now()),
			})
		});
		Ok(Self {
			http,
			link,
			opts,
			limiter,
			on_progress: None,
			on_notice: None,
		})
	}

	/// Sets the progress callback.
	pub fn on_progress(mut self, f: impl Fn(Progress) + Send + Sync + 'static) -> Self {
		self.on_progress = Some(Arc::new(f));
		self
	}

	/// Sets the callback for retries and resumes.
	pub fn on_notice(mut self, f: impl Fn(Notice) + Send + Sync + 'static) -> Self {
		self.on_notice = Some(Arc::new(f));
		self
	}

	/// The link this client targets.
	pub fn link(&self) -> &MailslotLink {
		&self.link
	}

	/// Asks the server for its capabilities.
	pub async fn health(&self) -> Result<Health> {
		let url = format!("{}{}", self.link.drop_url(), proto::ROUTE_HEALTH);
		let r = self
			.http
			.get(url)
			.send()
			.await
			.map_err(|e| Error::Send(e.to_string()))?;
		if !r.status().is_success() {
			return Err(Error::Send(format!("server answered {}", r.status())));
		}
		r.json().await.map_err(|e| Error::Send(e.to_string()))
	}

	/// Asks the server whether it already holds a file with this SHA-256.
	pub async fn have(&self, sha256: &str) -> Result<bool> {
		let url = format!(
			"{}{}?sha256={sha256}",
			self.link.drop_url(),
			proto::ROUTE_HAVE
		);
		let r = self
			.http
			.get(url)
			.send()
			.await
			.map_err(|e| Error::Send(e.to_string()))?;
		if !r.status().is_success() {
			return Ok(false);
		}
		let v: serde_json::Value = r.json().await.map_err(|e| Error::Send(e.to_string()))?;
		Ok(v.get("have")
			.and_then(serde_json::Value::as_bool)
			.unwrap_or(false))
	}

	fn report(&self, name: &str, sent: u64, total: u64, bps: f64) {
		if let Some(f) = &self.on_progress {
			f(Progress {
				name: name.to_owned(),
				sent,
				total,
				bytes_per_sec: bps,
			});
		}
	}

	fn notice(&self, n: Notice) {
		if let Some(f) = &self.on_notice {
			f(n);
		}
	}

	/// Sends a file, resuming and retrying as needed.
	pub async fn send_file(&self, path: &Path, name: Option<&str>) -> Result<SendOutcome> {
		let name = name.map(str::to_owned).unwrap_or_else(|| {
			path.file_name()
				.map(|n| n.to_string_lossy().into_owned())
				.unwrap_or_else(|| "upload".into())
		});
		let size = tokio::fs::metadata(path).await?.len();
		let sha = hash_file(path).await?;
		self.report(&name, 0, size, 0.0);

		if self.opts.dedupe && self.have(&sha).await.unwrap_or(false) {
			self.report(&name, size, size, 0.0);
			return Ok(SendOutcome {
				name,
				saved_as: String::new(),
				sha256: sha,
				size,
				skipped: true,
			});
		}

		let upload_id = hex::encode(&Sha256::digest(format!("{sha}:{name}").as_bytes())[..16]);
		let url = self.link.file_url(&name);
		let mut attempt_no = 0u32;
		let mut offset: Option<u64> = None;
		loop {
			attempt_no += 1;
			let off = match offset.take() {
				Some(o) => o,
				None => self.query_offset(&url, &upload_id).await.unwrap_or(0),
			};
			let off = if off > size { 0 } else { off };
			if off > 0 {
				self.notice(Notice::Resumed { offset: off });
			}
			match self
				.try_once(path, &name, &url, &upload_id, size, off, &sha)
				.await
			{
				Attempt::Done { saved_as, sha256 } => {
					if sha256 != sha {
						return Err(Error::Send(format!(
							"server reported a different SHA-256 ({sha256}) than the local file ({sha})"
						)));
					}
					self.report(&name, size, size, 0.0);
					return Ok(SendOutcome {
						name,
						saved_as,
						sha256: sha,
						size,
						skipped: false,
					});
				}
				Attempt::Fatal(reason) => return Err(Error::Send(reason)),
				Attempt::Retry {
					reason,
					retry_after,
					offset: hint,
				} => {
					if attempt_no > self.opts.retries {
						return Err(Error::Send(format!(
							"{reason} (gave up after {attempt_no} attempts)"
						)));
					}
					offset = hint;
					let exp = self
						.opts
						.backoff_base
						.saturating_mul(1u32 << (attempt_no - 1).min(10));
					let delay = retry_after.unwrap_or_else(|| exp.min(self.opts.backoff_max));
					self.notice(Notice::Retrying {
						attempt: attempt_no + 1,
						delay_secs: delay.as_secs_f64(),
						reason,
					});
					if hint.is_none() {
						tokio::time::sleep(delay).await;
					}
				}
			}
		}
	}

	async fn query_offset(&self, url: &str, upload_id: &str) -> Option<u64> {
		let r = self
			.http
			.head(url)
			.header(proto::HDR_UPLOAD_ID, upload_id)
			.send()
			.await
			.ok()?;
		if !r.status().is_success() {
			return None;
		}
		r.headers()
			.get(proto::HDR_OFFSET)?
			.to_str()
			.ok()?
			.parse()
			.ok()
	}

	#[allow(clippy::too_many_arguments)]
	async fn try_once(
		&self,
		path: &Path,
		name: &str,
		url: &str,
		upload_id: &str,
		size: u64,
		off: u64,
		sha: &str,
	) -> Attempt {
		let mut file = match tokio::fs::File::open(path).await {
			Ok(f) => f,
			Err(e) => return Attempt::Fatal(format!("cannot open {}: {e}", path.display())),
		};
		if off > 0 && file.seek(std::io::SeekFrom::Start(off)).await.is_err() {
			return Attempt::Fatal("cannot seek in the file".into());
		}
		let remaining = size - off;
		let limiter = self.limiter.clone();
		let progress = self.on_progress.clone();
		let pname = name.to_owned();
		let started = Instant::now();
		let state = (file, off, Instant::now(), 0u64);
		let stream =
			futures_util::stream::unfold(state, move |(mut file, sent, mut last, mut since)| {
				let limiter = limiter.clone();
				let progress = progress.clone();
				let pname = pname.clone();
				async move {
					if sent >= size {
						return None;
					}
					let want = CHUNK.min((size - sent) as usize);
					let mut buf = vec![0u8; want];
					match file.read_exact(&mut buf).await {
						Ok(_) => {}
						Err(e) => return Some((Err(e), (file, size, last, since))),
					}
					if let Some(l) = &limiter {
						l.take(want).await;
					}
					let sent = sent + want as u64;
					since += want as u64;
					if last.elapsed() >= Duration::from_millis(100) || sent == size {
						if let Some(p) = &progress {
							let secs = started.elapsed().as_secs_f64().max(0.001);
							p(Progress {
								name: pname,
								sent,
								total: size,
								bytes_per_sec: (sent - off) as f64 / secs,
							});
						}
						last = Instant::now();
						since = 0;
					}
					Some((Ok(bytes::Bytes::from(buf)), (file, sent, last, since)))
				}
			});
		let mut req = self
			.http
			.put(url)
			.header(proto::HDR_UPLOAD_ID, upload_id)
			.header(proto::HDR_EXPECTED_SHA256, sha)
			.header(reqwest::header::ACCEPT, "application/json")
			.header(reqwest::header::CONTENT_LENGTH, remaining);
		if size > 0 {
			req = req.header(
				reqwest::header::CONTENT_RANGE,
				format!("bytes {off}-{}/{size}", size - 1),
			);
		}
		if let Some(label) = &self.opts.sender_label {
			req = req.header(proto::HDR_SENDER, label.as_str());
		}
		let resp = match req.body(reqwest::Body::wrap_stream(stream)).send().await {
			Ok(r) => r,
			Err(e) => {
				return Attempt::Retry {
					reason: e.to_string(),
					retry_after: None,
					offset: None,
				};
			}
		};
		let status = resp.status();
		let retry_after = resp
			.headers()
			.get(reqwest::header::RETRY_AFTER)
			.and_then(|v| v.to_str().ok())
			.and_then(|v| v.parse::<u64>().ok())
			.map(Duration::from_secs);
		let hint = resp
			.headers()
			.get(proto::HDR_OFFSET)
			.and_then(|v| v.to_str().ok())
			.and_then(|v| v.parse::<u64>().ok());
		match status {
			StatusCode::CREATED => {
				let v: serde_json::Value = resp.json().await.unwrap_or_default();
				Attempt::Done {
					saved_as: v
						.get("name")
						.and_then(|v| v.as_str())
						.unwrap_or_default()
						.to_owned(),
					sha256: v
						.get("sha256")
						.and_then(|v| v.as_str())
						.unwrap_or_default()
						.to_owned(),
				}
			}
			StatusCode::CONFLICT if hint.is_some() => Attempt::Retry {
				reason: "server offset differs".into(),
				retry_after: None,
				offset: hint,
			},
			StatusCode::TOO_MANY_REQUESTS
			| StatusCode::SERVICE_UNAVAILABLE
			| StatusCode::REQUEST_TIMEOUT => Attempt::Retry {
				reason: format!("server answered {status}"),
				retry_after,
				offset: None,
			},
			s if s.is_server_error() && s != StatusCode::INSUFFICIENT_STORAGE => Attempt::Retry {
				reason: format!("server answered {s}"),
				retry_after,
				offset: None,
			},
			s => {
				let body = resp.text().await.unwrap_or_default();
				Attempt::Fatal(format!("server answered {} {}", s.as_u16(), body.trim()))
			}
		}
	}

	/// Sends several files, `parallel` at a time. Results keep the input order.
	pub async fn send_many(&self, paths: Vec<PathBuf>) -> Vec<Result<SendOutcome>> {
		let sem = Arc::new(Semaphore::new(self.opts.parallel.max(1)));
		let mut tasks = Vec::new();
		for p in paths {
			let this = self.clone();
			let sem = sem.clone();
			tasks.push(tokio::spawn(async move {
				let _permit = sem.acquire_owned().await;
				this.send_file(&p, None).await
			}));
		}
		let mut out = Vec::new();
		for t in tasks {
			out.push(t.await.unwrap_or_else(|e| Err(Error::Send(e.to_string()))));
		}
		out
	}

	/// Streams data of unknown length (for example standard input) in one request.
	/// There is no resume; the request uses chunked encoding.
	pub async fn send_stream<R>(&self, reader: R, name: &str) -> Result<SendOutcome>
	where
		R: tokio::io::AsyncRead + Send + Unpin + 'static,
	{
		let hasher = Arc::new(std::sync::Mutex::new(Sha256::new()));
		let count = Arc::new(std::sync::atomic::AtomicU64::new(0));
		let (h2, c2) = (hasher.clone(), count.clone());
		let progress = self.on_progress.clone();
		let pname = name.to_owned();
		let stream = tokio_util::io::ReaderStream::with_capacity(reader, CHUNK).inspect(move |c| {
			if let Ok(b) = c {
				if let Ok(mut h) = h2.lock() {
					h.update(b);
				}
				let n = c2.fetch_add(b.len() as u64, std::sync::atomic::Ordering::SeqCst)
					+ b.len() as u64;
				if let Some(p) = &progress {
					p(Progress {
						name: pname.clone(),
						sent: n,
						total: 0,
						bytes_per_sec: 0.0,
					});
				}
			}
		});
		let mut req = self
			.http
			.put(self.link.file_url(name))
			.header(reqwest::header::ACCEPT, "application/json");
		if let Some(label) = &self.opts.sender_label {
			req = req.header(proto::HDR_SENDER, label.as_str());
		}
		let resp = req
			.body(reqwest::Body::wrap_stream(stream))
			.send()
			.await
			.map_err(|e| Error::Send(e.to_string()))?;
		if resp.status() != StatusCode::CREATED {
			let s = resp.status();
			return Err(Error::Send(format!(
				"server answered {} {}",
				s.as_u16(),
				resp.text().await.unwrap_or_default().trim()
			)));
		}
		let v: serde_json::Value = resp.json().await.map_err(|e| Error::Send(e.to_string()))?;
		let local = hex::encode(
			hasher
				.lock()
				.map(|h| h.clone().finalize())
				.map_err(|_| Error::Send("hash lock poisoned".into()))?,
		);
		let remote = v
			.get("sha256")
			.and_then(|v| v.as_str())
			.unwrap_or_default()
			.to_owned();
		if local != remote {
			return Err(Error::Send(format!(
				"server reported a different SHA-256 ({remote}) than the data sent ({local})"
			)));
		}
		Ok(SendOutcome {
			name: name.to_owned(),
			saved_as: v
				.get("name")
				.and_then(|v| v.as_str())
				.unwrap_or_default()
				.to_owned(),
			sha256: local,
			size: count.load(std::sync::atomic::Ordering::SeqCst),
			skipped: false,
		})
	}
}

/// SHA-256 of a file, streamed.
pub async fn hash_file(path: &Path) -> Result<String> {
	let mut f = tokio::fs::File::open(path).await?;
	let mut h = Sha256::new();
	let mut buf = vec![0u8; 256 * 1024];
	loop {
		let n = f.read(&mut buf).await?;
		if n == 0 {
			break;
		}
		h.update(&buf[..n]);
	}
	Ok(hex::encode(h.finalize()))
}

/// Expands directories into the files beneath them (sorted, hidden entries included).
pub async fn expand_paths(paths: &[PathBuf]) -> Result<Vec<PathBuf>> {
	let mut out = Vec::new();
	let mut stack: Vec<PathBuf> = paths.iter().rev().cloned().collect();
	while let Some(p) = stack.pop() {
		let meta = tokio::fs::metadata(&p).await?;
		if meta.is_dir() {
			let mut rd = tokio::fs::read_dir(&p).await?;
			let mut children = Vec::new();
			while let Some(e) = rd.next_entry().await? {
				children.push(e.path());
			}
			children.sort();
			stack.extend(children.into_iter().rev());
		} else if meta.is_file() {
			out.push(p);
		}
	}
	Ok(out)
}
