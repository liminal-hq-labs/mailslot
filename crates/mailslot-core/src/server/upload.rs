// Streaming uploads: temp file, hashing, limits, resume and atomic claim of the final name.
//
// (c) Copyright 2026 Liminal HQ, Scott Morris
// SPDX-License-Identifier: Apache-2.0 OR MIT

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::http::request::Parts;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::Response;
use base64::Engine as _;
use futures_util::StreamExt;
use sha2::{Digest, Sha256};
use time::OffsetDateTime;
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};

use super::handler::{plain, reject};
use super::{ServerEvent, ServerSettings, Shared};
use crate::config::{Collision, SubfolderScheme};
use crate::events::UploadEvent;
use crate::{naming, proto};

const WRITE_BUFFER: usize = 256 * 1024;
const PROGRESS_EVERY: Duration = Duration::from_millis(100);
const SPACE_RECHECK_BYTES: u64 = 8 * 1024 * 1024;

struct ByteRange {
	start: u64,
	end: u64,
	total: u64,
}

fn header_str<'a>(h: &'a HeaderMap, name: &str) -> Option<&'a str> {
	h.get(name).and_then(|v| v.to_str().ok())
}

fn parse_range(v: &str) -> Option<ByteRange> {
	let rest = v.trim().strip_prefix("bytes ")?;
	let (span, total) = rest.split_once('/')?;
	let (start, end) = span.split_once('-')?;
	let r = ByteRange {
		start: start.parse().ok()?,
		end: end.parse().ok()?,
		total: total.parse().ok()?,
	};
	(r.start <= r.end && r.end < r.total).then_some(r)
}

fn valid_upload_id(id: &str) -> bool {
	(8..=64).contains(&id.len())
		&& id
			.bytes()
			.all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn expected_sha(h: &HeaderMap) -> Result<Option<String>, ()> {
	if let Some(v) = header_str(h, proto::HDR_EXPECTED_SHA256) {
		let v = v.trim().to_ascii_lowercase();
		return if v.len() == 64 && v.bytes().all(|b| b.is_ascii_hexdigit()) {
			Ok(Some(v))
		} else {
			Err(())
		};
	}
	if let Some(v) = header_str(h, "content-digest") {
		for item in v.split(',') {
			if let Some(b64) = item
				.trim()
				.strip_prefix("sha-256=:")
				.and_then(|s| s.strip_suffix(':'))
			{
				let bytes = base64::engine::general_purpose::STANDARD
					.decode(b64)
					.map_err(|_| ())?;
				return if bytes.len() == 32 {
					Ok(Some(hex::encode(bytes)))
				} else {
					Err(())
				};
			}
		}
	}
	Ok(None)
}

fn part_path(dest: &Path, key: &str) -> PathBuf {
	dest.join(format!(".mailslot-{key}.part"))
}

fn resumable_key(upload_id: &str) -> String {
	hex::encode(&Sha256::digest(upload_id.as_bytes())[..12])
}

fn offset_response(status: StatusCode, offset: u64, body: &'static str) -> Response {
	let mut r = plain(status, body);
	r.headers_mut()
		.insert(proto::HDR_OFFSET, HeaderValue::from(offset));
	r
}

/// `HEAD /<token>/<name>`: how many bytes the server holds for an upload id.
pub(super) async fn head(shared: &Shared, headers: &HeaderMap) -> Response {
	let mut offset = 0;
	if let Some(id) = header_str(headers, proto::HDR_UPLOAD_ID).filter(|i| valid_upload_id(i))
		&& let Ok(m) = tokio::fs::metadata(part_path(&shared.dest, &resumable_key(id))).await
	{
		offset = m.len();
	}
	let mut r = offset_response(StatusCode::OK, offset, "");
	r.headers_mut()
		.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
	r
}

struct ActiveGuard<'a>(&'a Shared);
impl Drop for ActiveGuard<'_> {
	fn drop(&mut self) {
		self.0.active.fetch_sub(1, Ordering::SeqCst);
	}
}

/// Tracks one upload so every exit path reports and cleans up.
struct Attempt {
	shared: Arc<Shared>,
	id: u64,
	part: PathBuf,
	resumable: bool,
	keep_part: bool,
	upload_id_lock: Option<String>,
	reserved_file: bool,
	finished: bool,
}

impl Attempt {
	/// Ends the attempt with an error response.
	fn release_reservation(&mut self) {
		if self.reserved_file {
			self.reserved_file = false;
			self.shared.files_reserved.fetch_sub(1, Ordering::SeqCst);
		}
	}

	async fn fail(mut self, status: StatusCode, reason: &'static str) -> Response {
		self.release_reservation();
		self.finished = true;
		let resumable = self.resumable && self.keep_part;
		if !resumable {
			let _ = tokio::fs::remove_file(&self.part).await;
		}
		self.shared.emit(UploadEvent::Failed {
			id: self.id,
			reason: reason.to_owned(),
			resumable,
		});
		plain(status, reason)
	}

	async fn fail_with_offset(
		mut self,
		status: StatusCode,
		reason: &'static str,
		offset: u64,
	) -> Response {
		self.keep_part = true;
		self.finished = true;
		self.shared.emit(UploadEvent::Failed {
			id: self.id,
			reason: reason.to_owned(),
			resumable: self.resumable,
		});
		offset_response(status, offset, reason)
	}
}

impl Drop for Attempt {
	fn drop(&mut self) {
		if let Some(id) = &self.upload_id_lock
			&& let Ok(mut set) = self.shared.in_flight_ids.lock()
		{
			set.remove(id);
		}
		if self.finished {
			return;
		}
		if self.reserved_file {
			self.shared.files_reserved.fetch_sub(1, Ordering::SeqCst);
		}
		let keep = self.resumable && self.keep_part;
		self.shared.emit(UploadEvent::Failed {
			id: self.id,
			reason: "connection closed".into(),
			resumable: keep,
		});
		if !keep {
			let path = self.part.clone();
			if let Ok(h) = tokio::runtime::Handle::try_current() {
				h.spawn(async move {
					let _ = tokio::fs::remove_file(path).await;
				});
			}
		}
	}
}

fn target_dir(shared: &Shared, s: &ServerSettings, sender: Option<&str>) -> PathBuf {
	match s.subfolder {
		SubfolderScheme::None => shared.dest.clone(),
		SubfolderScheme::ByDate => {
			let d = OffsetDateTime::now_utc().date();
			shared.dest.join(format!(
				"{:04}-{:02}-{:02}",
				d.year(),
				u8::from(d.month()),
				d.day()
			))
		}
		SubfolderScheme::BySender => shared
			.dest
			.join(naming::sanitize(sender.unwrap_or("unknown"))),
	}
}

async fn open_part(
	path: &Path,
	create_new: bool,
	truncate: bool,
) -> std::io::Result<tokio::fs::File> {
	let mut o = tokio::fs::OpenOptions::new();
	o.write(true);
	if create_new {
		o.create_new(true);
	} else {
		o.create(true).truncate(truncate);
	}
	#[cfg(unix)]
	{
		o.mode(0o644);
		o.custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32);
	}
	o.open(path).await
}

async fn hash_prefix(path: &Path, len: u64, hasher: &mut Sha256) -> std::io::Result<()> {
	let mut f = tokio::fs::File::open(path).await?.take(len);
	let mut buf = vec![0u8; 64 * 1024];
	loop {
		let n = f.read(&mut buf).await?;
		if n == 0 {
			break;
		}
		hasher.update(&buf[..n]);
	}
	Ok(())
}

pub(super) async fn put(
	shared: &Arc<Shared>,
	peer: SocketAddr,
	parts: &Parts,
	body: Body,
	raw_name: &str,
) -> Response {
	let settings = shared.settings();
	let h = &parts.headers;
	if !shared.accepting.load(Ordering::SeqCst) {
		return reject(shared, peer, StatusCode::SERVICE_UNAVAILABLE, "paused");
	}
	let name = naming::sanitize(raw_name);
	if !settings.allowed_extensions.is_empty() {
		let ok = naming::extension_lower(&name)
			.is_some_and(|e| settings.allowed_extensions.contains(&e));
		if !ok {
			return reject(
				shared,
				peer,
				StatusCode::UNSUPPORTED_MEDIA_TYPE,
				"file type not allowed",
			);
		}
	}
	let expected = match expected_sha(h) {
		Ok(v) => v,
		Err(()) => return reject(shared, peer, StatusCode::BAD_REQUEST, "bad checksum header"),
	};
	let upload_id = match header_str(h, proto::HDR_UPLOAD_ID) {
		Some(id) if valid_upload_id(id) => Some(id.to_owned()),
		Some(_) => return reject(shared, peer, StatusCode::BAD_REQUEST, "bad upload id"),
		None => None,
	};
	let range = match header_str(h, header::CONTENT_RANGE.as_str()) {
		Some(v) => match parse_range(v) {
			Some(r) if upload_id.is_some() => Some(r),
			_ => return reject(shared, peer, StatusCode::BAD_REQUEST, "bad content range"),
		},
		None => None,
	};
	let content_length: Option<u64> =
		header_str(h, header::CONTENT_LENGTH.as_str()).and_then(|v| v.parse().ok());
	if let (Some(r), Some(cl)) = (&range, content_length)
		&& cl != r.end - r.start + 1
	{
		return reject(
			shared,
			peer,
			StatusCode::BAD_REQUEST,
			"content range does not match length",
		);
	}
	let declared_total = range.as_ref().map(|r| r.total).or(content_length);
	if declared_total.is_some_and(|t| t > settings.max_file_bytes) {
		return reject(
			shared,
			peer,
			StatusCode::PAYLOAD_TOO_LARGE,
			"file too large",
		);
	}
	let incoming = range
		.as_ref()
		.map(|r| r.end - r.start + 1)
		.or(content_length)
		.unwrap_or(0);
	if settings.quota_bytes > 0
		&& shared
			.received_bytes
			.load(Ordering::SeqCst)
			.saturating_add(incoming)
			> settings.quota_bytes
	{
		return reject(
			shared,
			peer,
			StatusCode::INSUFFICIENT_STORAGE,
			"session quota reached",
		);
	}

	// Concurrency.
	let now_active = shared.active.fetch_add(1, Ordering::SeqCst) + 1;
	let _active = ActiveGuard(shared);
	if now_active > settings.max_concurrent {
		let mut r = reject(
			shared,
			peer,
			StatusCode::TOO_MANY_REQUESTS,
			"too many uploads in progress",
		);
		r.headers_mut()
			.insert(header::RETRY_AFTER, HeaderValue::from_static("1"));
		return r;
	}

	// Free space.
	{
		let fs = shared.free_space.clone();
		let dir = shared.dest.clone();
		let avail = tokio::task::spawn_blocking(move || fs.available(&dir))
			.await
			.ok()
			.and_then(Result::ok);
		if let Some(avail) = avail
			&& avail < settings.min_free_bytes.saturating_add(incoming)
		{
			return reject(
				shared,
				peer,
				StatusCode::INSUFFICIENT_STORAGE,
				"not enough free space",
			);
		}
	}

	// File-count rule.
	let mut reserved_file = false;
	if settings.stop_after_files > 0 {
		let n = shared.files_reserved.fetch_add(1, Ordering::SeqCst);
		if n >= settings.stop_after_files {
			shared.files_reserved.fetch_sub(1, Ordering::SeqCst);
			return reject(shared, peer, StatusCode::GONE, "file limit reached");
		}
		reserved_file = true;
	}

	let id = shared.next_id.fetch_add(1, Ordering::SeqCst);
	let resumable = upload_id.is_some();
	let key = match &upload_id {
		Some(u) => resumable_key(u),
		None => {
			let mut b = [0u8; 8];
			let _ = getrandom::fill(&mut b);
			format!("{}-{}", hex::encode(b), id)
		}
	};
	let part = part_path(&shared.dest, &key);
	let mut attempt = Attempt {
		shared: shared.clone(),
		id,
		part: part.clone(),
		resumable,
		keep_part: resumable,
		upload_id_lock: None,
		reserved_file,
		finished: false,
	};

	if let Some(uid) = &upload_id {
		let fresh = shared
			.in_flight_ids
			.lock()
			.map(|mut s| s.insert(uid.clone()))
			.unwrap_or(false);
		if !fresh {
			// Another request owns this upload id. Report without touching its file.
			attempt.finished = true;
			attempt.reserved_file = false;
			if reserved_file {
				shared.files_reserved.fetch_sub(1, Ordering::SeqCst);
			}
			return reject(
				shared,
				peer,
				StatusCode::CONFLICT,
				"upload already in progress",
			);
		}
		attempt.upload_id_lock = Some(uid.clone());
	}

	shared.emit(UploadEvent::Started {
		id,
		name: name.clone(),
		expected: declared_total,
		peer: peer.ip().to_string(),
	});

	// Open (or resume) the partial file.
	let start = range.as_ref().map(|r| r.start).unwrap_or(0);
	let mut hasher = Sha256::new();
	let file = if start > 0 {
		let have = tokio::fs::metadata(&part)
			.await
			.map(|m| m.len())
			.unwrap_or(0);
		if have != start {
			return attempt
				.fail_with_offset(StatusCode::CONFLICT, "offset mismatch", have)
				.await;
		}
		if hash_prefix(&part, start, &mut hasher).await.is_err() {
			return attempt
				.fail(
					StatusCode::INTERNAL_SERVER_ERROR,
					"could not read partial file",
				)
				.await;
		}
		match open_part(&part, false, false).await {
			Ok(mut f) => {
				if f.seek(std::io::SeekFrom::Start(start)).await.is_err() {
					return attempt
						.fail(StatusCode::INTERNAL_SERVER_ERROR, "could not resume")
						.await;
				}
				f
			}
			Err(_) => {
				return attempt
					.fail(
						StatusCode::INTERNAL_SERVER_ERROR,
						"could not open partial file",
					)
					.await;
			}
		}
	} else {
		match open_part(&part, !resumable, true).await {
			Ok(f) => f,
			Err(_) => {
				return attempt
					.fail(
						StatusCode::INTERNAL_SERVER_ERROR,
						"could not create temporary file",
					)
					.await;
			}
		}
	};
	let mut writer = tokio::io::BufWriter::with_capacity(WRITE_BUFFER, file);

	// Stream the body.
	let cancel = shared.token_cancel();
	let mut stream = body.into_data_stream();
	let mut counted: u64 = 0;
	let mut last_emit = Instant::now();
	let mut last_space_check: u64 = 0;
	loop {
		let next = tokio::select! {
			_ = cancel.cancelled() => {
				attempt.keep_part = false;
				return attempt.fail(StatusCode::GONE, "link replaced").await;
			}
			_ = shared.shutdown.cancelled() => {
				attempt.keep_part = true;
				return attempt.fail(StatusCode::SERVICE_UNAVAILABLE, "stopping").await;
			}
			r = tokio::time::timeout(settings.idle_timeout, stream.next()) => r,
		};
		let chunk = match next {
			Err(_) => {
				return attempt
					.fail(StatusCode::REQUEST_TIMEOUT, "upload stalled")
					.await;
			}
			Ok(None) => break,
			Ok(Some(Err(_))) => {
				return attempt
					.fail(StatusCode::BAD_REQUEST, "connection error")
					.await;
			}
			Ok(Some(Ok(c))) => c,
		};
		counted += chunk.len() as u64;
		if start + counted > settings.max_file_bytes {
			attempt.keep_part = false;
			return attempt
				.fail(StatusCode::PAYLOAD_TOO_LARGE, "file too large")
				.await;
		}
		if settings.quota_bytes > 0
			&& shared.received_bytes.load(Ordering::SeqCst) + counted > settings.quota_bytes
		{
			attempt.keep_part = false;
			return attempt
				.fail(StatusCode::INSUFFICIENT_STORAGE, "session quota reached")
				.await;
		}
		if counted - last_space_check >= SPACE_RECHECK_BYTES {
			last_space_check = counted;
			let fs = shared.free_space.clone();
			let dir = shared.dest.clone();
			let avail = tokio::task::spawn_blocking(move || fs.available(&dir))
				.await
				.ok()
				.and_then(Result::ok);
			if avail.is_some_and(|a| a < settings.min_free_bytes) {
				attempt.keep_part = false;
				return attempt
					.fail(StatusCode::INSUFFICIENT_STORAGE, "not enough free space")
					.await;
			}
		}
		hasher.update(&chunk);
		if writer.write_all(&chunk).await.is_err() {
			return attempt
				.fail(StatusCode::INSUFFICIENT_STORAGE, "write failed")
				.await;
		}
		if last_emit.elapsed() >= PROGRESS_EVERY {
			last_emit = Instant::now();
			shared.emit(UploadEvent::Progress {
				id,
				received: start + counted,
			});
		}
	}
	if writer.flush().await.is_err() {
		return attempt
			.fail(StatusCode::INSUFFICIENT_STORAGE, "write failed")
			.await;
	}
	drop(writer);
	let size = start + counted;

	// A partial range that did not reach the end of the file: keep it and report the offset.
	if let Some(r) = &range
		&& size < r.total
	{
		shared.received_bytes.fetch_add(counted, Ordering::SeqCst);
		shared.emit(UploadEvent::Progress { id, received: size });
		return attempt.partial_ok(size);
	}
	if let Some(cl) = content_length
		&& range.is_none()
		&& cl != counted
	{
		return attempt
			.fail(StatusCode::BAD_REQUEST, "body shorter than declared")
			.await;
	}

	let sha = hex::encode(hasher.finalize());
	if let Some(exp) = &expected
		&& *exp != sha
	{
		attempt.keep_part = false;
		return attempt
			.fail(StatusCode::UNPROCESSABLE_ENTITY, "checksum mismatch")
			.await;
	}
	if settings.verify_on_receive {
		let mut again = Sha256::new();
		let ok = hash_prefix(&part, size, &mut again).await.is_ok()
			&& hex::encode(again.finalize()) == sha;
		if !ok {
			attempt.keep_part = false;
			return attempt
				.fail(StatusCode::INTERNAL_SERVER_ERROR, "verification failed")
				.await;
		}
	}

	// Claim the final name without ever overwriting.
	let sender = header_str(h, proto::HDR_SENDER)
		.map(str::to_owned)
		.or_else(|| {
			parts.uri.query().and_then(|q| {
				q.split('&')
					.find_map(|kv| kv.strip_prefix("from="))
					.map(|v| {
						percent_encoding::percent_decode_str(v)
							.decode_utf8_lossy()
							.into_owned()
					})
			})
		});
	let dir = target_dir(shared, &settings, sender.as_deref());
	if dir != shared.dest && tokio::fs::create_dir_all(&dir).await.is_err() {
		return attempt
			.fail(StatusCode::INTERNAL_SERVER_ERROR, "could not create folder")
			.await;
	}
	let mut saved: Option<(PathBuf, String)> = None;
	for n in 0..10_000u32 {
		let cand = naming::candidate(&name, n);
		let target = dir.join(&cand);
		match tokio::fs::hard_link(&part, &target).await {
			Ok(()) => {
				saved = Some((target, cand));
				break;
			}
			Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
				if settings.collision == Collision::Reject {
					attempt.keep_part = false;
					return attempt
						.fail(StatusCode::CONFLICT, "a file with that name exists")
						.await;
				}
			}
			Err(_) => break,
		}
	}
	let Some((target, saved_as)) = saved else {
		return attempt
			.fail(StatusCode::INTERNAL_SERVER_ERROR, "could not save file")
			.await;
	};
	let _ = tokio::fs::remove_file(&part).await;
	if settings.sidecars {
		let sidecar = dir.join(format!("{saved_as}.sha256"));
		if let Ok(mut f) = tokio::fs::OpenOptions::new()
			.write(true)
			.create_new(true)
			.open(&sidecar)
			.await
		{
			let _ = f.write_all(format!("{sha}  {saved_as}\n").as_bytes()).await;
		}
	}
	attempt.finished = true;
	shared.received_bytes.fetch_add(counted, Ordering::SeqCst);
	shared.hashes.insert(sha.clone(), target.clone());
	shared.emit(UploadEvent::Finished {
		id,
		path: target,
		saved_as: saved_as.clone(),
		size,
		sha256: sha.clone(),
		peer: peer.ip().to_string(),
	});
	if settings.stop_after_files > 0 {
		let done = shared.files_done.fetch_add(1, Ordering::SeqCst) + 1;
		if done >= settings.stop_after_files {
			let _ = shared.events.send(ServerEvent::FileLimitReached);
		}
	}
	created(
		&saved_as,
		&sha,
		size,
		header_str(h, header::ACCEPT.as_str()).is_some_and(|a| a.contains("application/json")),
	)
}

impl Attempt {
	fn partial_ok(mut self, offset: u64) -> Response {
		self.release_reservation();
		self.keep_part = true;
		self.finished = true;
		self.shared.emit(UploadEvent::Failed {
			id: self.id,
			reason: "partial upload kept".into(),
			resumable: true,
		});
		offset_response(StatusCode::ACCEPTED, offset, "partial")
	}
}

fn created(saved_as: &str, sha: &str, size: u64, json: bool) -> Response {
	let mut r = if json {
		let mut r = Response::new(Body::from(
			serde_json::json!({ "name": saved_as, "size": size, "sha256": sha }).to_string(),
		));
		r.headers_mut().insert(
			header::CONTENT_TYPE,
			HeaderValue::from_static("application/json"),
		);
		r
	} else {
		let mut r = Response::new(Body::from(saved_as.to_owned()));
		r.headers_mut().insert(
			header::CONTENT_TYPE,
			HeaderValue::from_static("text/plain; charset=utf-8"),
		);
		r
	};
	*r.status_mut() = StatusCode::CREATED;
	if let Ok(v) = HeaderValue::from_str(saved_as) {
		r.headers_mut().insert(proto::HDR_SAVED_AS, v);
	}
	if let Ok(v) = HeaderValue::from_str(sha) {
		r.headers_mut().insert(proto::HDR_SHA256, v);
	}
	r
}
