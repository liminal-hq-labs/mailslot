// Request routing for the token-prefixed routes.
//
// (c) Copyright 2026 Liminal HQ, Scott Morris
// SPDX-License-Identifier: Apache-2.0 OR MIT

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::SystemTime;

use axum::body::Body;
use axum::http::{HeaderValue, Method, Request, StatusCode, header};
use axum::response::Response;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use super::{Shared, upload};
use crate::events::UploadEvent;
use crate::proto;

const INDEX_HTML: &str = include_str!("../../assets/web/index.html");
const DROP_JS: &str = include_str!("../../assets/web/drop.js");
const DROP_CSS: &str = include_str!("../../assets/web/drop.css");
const LOGO_SVG: &str = include_str!("../../assets/web/logo.svg");

pub(super) fn plain(status: StatusCode, body: &'static str) -> Response {
	let mut r = Response::new(Body::from(body));
	*r.status_mut() = status;
	r.headers_mut().insert(
		header::CONTENT_TYPE,
		HeaderValue::from_static("text/plain; charset=utf-8"),
	);
	r
}

pub(super) fn not_found() -> Response {
	plain(StatusCode::NOT_FOUND, "not found")
}

fn asset(body: &'static str, content_type: &'static str) -> Response {
	let mut r = Response::new(Body::from(body));
	r.headers_mut()
		.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
	r
}

/// Adds the headers every reply carries.
fn harden(mut r: Response, close: bool) -> Response {
	let h = r.headers_mut();
	h.insert(
		header::X_CONTENT_TYPE_OPTIONS,
		HeaderValue::from_static("nosniff"),
	);
	h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
	h.insert(
		header::REFERRER_POLICY,
		HeaderValue::from_static("no-referrer"),
	);
	h.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
	h.insert(header::SERVER, HeaderValue::from_static("mailslot"));
	h.insert(
		header::CONTENT_SECURITY_POLICY,
		HeaderValue::from_static(proto::CSP),
	);
	if close {
		h.insert(header::CONNECTION, HeaderValue::from_static("close"));
	}
	r
}

pub(super) async fn handle(shared: Arc<Shared>, peer: SocketAddr, req: Request<Body>) -> Response {
	let locked = shared.lockout.is_locked(peer.ip());
	let resp = if locked {
		not_found()
	} else {
		route(&shared, peer, req).await
	};
	harden(resp, locked)
}

async fn route(shared: &Arc<Shared>, peer: SocketAddr, req: Request<Body>) -> Response {
	let (parts, body) = req.into_parts();
	let path = parts.uri.path().to_owned();
	let Some(trimmed) = path.strip_prefix('/') else {
		return not_found();
	};
	let (tok, rest) = match trimmed.split_once('/') {
		Some((t, r)) => (t, Some(r)),
		None => (trimmed, None),
	};
	let settings = shared.settings();
	if !shared.token_matches(tok) {
		shared.lockout.note_bad(
			peer.ip(),
			settings.lockout_threshold,
			settings.lockout_duration,
		);
		return not_found();
	}
	let Some(rest) = rest else {
		return match parts.method {
			Method::GET | Method::HEAD => {
				let mut r = Response::new(Body::empty());
				*r.status_mut() = StatusCode::PERMANENT_REDIRECT;
				if let Ok(v) = HeaderValue::from_str(&format!("/{tok}/")) {
					r.headers_mut().insert(header::LOCATION, v);
				}
				r
			}
			Method::PUT | Method::POST => {
				reject(shared, peer, StatusCode::BAD_REQUEST, "missing file name")
			}
			_ => not_found(),
		};
	};
	if let Err(wait) = shared.rate.check(peer.ip(), settings.rate_limit_per_min) {
		let mut r = reject(shared, peer, StatusCode::TOO_MANY_REQUESTS, "rate limit");
		if let Ok(v) = HeaderValue::from_str(&wait.to_string()) {
			r.headers_mut().insert(header::RETRY_AFTER, v);
		}
		return r;
	}
	let expired = settings.expires_at.is_some_and(|t| SystemTime::now() >= t);
	let accepting = shared.accepting.load(std::sync::atomic::Ordering::SeqCst);

	match (&parts.method, rest) {
		(&Method::GET, proto::ROUTE_HEALTH) => health(&settings, accepting && !expired),
		(_, _) if expired => reject(shared, peer, StatusCode::GONE, "link expired"),
		(&Method::GET, "") => asset(INDEX_HTML, "text/html; charset=utf-8"),
		(&Method::GET, "drop.js") => asset(DROP_JS, "text/javascript; charset=utf-8"),
		(&Method::GET, "drop.css") => asset(DROP_CSS, "text/css; charset=utf-8"),
		(&Method::GET, "logo.svg") => asset(LOGO_SVG, "image/svg+xml"),
		(&Method::GET, proto::ROUTE_HAVE) => have(shared, parts.uri.query()).await,
		(&Method::HEAD, name) if !name.is_empty() => upload::head(shared, &parts.headers).await,
		(&Method::PUT | &Method::POST, name) if !name.is_empty() => {
			upload::put(shared, peer, &parts, body, name).await
		}
		(&Method::PUT | &Method::POST, _) => {
			reject(shared, peer, StatusCode::BAD_REQUEST, "missing file name")
		}
		_ => plain(StatusCode::METHOD_NOT_ALLOWED, "method not allowed"),
	}
}

pub(super) fn reject(
	shared: &Shared,
	peer: SocketAddr,
	status: StatusCode,
	reason: &'static str,
) -> Response {
	shared.emit(UploadEvent::Rejected {
		peer: peer.ip().to_string(),
		status: status.as_u16(),
		reason: reason.to_owned(),
	});
	plain(status, reason)
}

fn json(value: serde_json::Value) -> Response {
	let mut r = Response::new(Body::from(value.to_string()));
	r.headers_mut().insert(
		header::CONTENT_TYPE,
		HeaderValue::from_static("application/json"),
	);
	r
}

fn health(settings: &super::ServerSettings, accepting: bool) -> Response {
	let expires = settings
		.expires_at
		.and_then(|t| OffsetDateTime::from(t).format(&Rfc3339).ok());
	json(serde_json::json!({
		"ok": true,
		"protocol": proto::PROTOCOL_VERSION,
		"max_bytes": settings.max_file_bytes,
		"expires_at": expires,
		"accepting": accepting,
		"capabilities": proto::CAPABILITIES,
	}))
}

async fn have(shared: &Shared, query: Option<&str>) -> Response {
	let wanted = query
		.into_iter()
		.flat_map(|q| q.split('&'))
		.find_map(|kv| kv.strip_prefix("sha256="))
		.map(str::to_ascii_lowercase)
		.filter(|h| h.len() == 64 && h.bytes().all(|b| b.is_ascii_hexdigit()));
	let Some(hash) = wanted else {
		return plain(StatusCode::BAD_REQUEST, "expected sha256=<64 hex digits>");
	};
	let have = match shared.hashes.get(&hash) {
		Some(path) => tokio::fs::metadata(&path)
			.await
			.map(|m| m.is_file())
			.unwrap_or(false),
		None => false,
	};
	json(serde_json::json!({ "have": have }))
}
