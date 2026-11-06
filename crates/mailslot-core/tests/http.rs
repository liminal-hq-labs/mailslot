// HTTP contract tests: every route, rule and limit of the server.
//
// (c) Copyright 2026 Liminal HQ, Scott Morris
// SPDX-License-Identifier: Apache-2.0 OR MIT

#![allow(clippy::disallowed_methods)] // Tests may use blocking std calls for set-up and assertions.

mod support;

use std::sync::Arc;
use std::time::Duration;

use mailslot_core::config::{Collision, SubfolderScheme};
use mailslot_core::events::UploadEvent;
use mailslot_core::proto;
use mailslot_core::server::{FixedFreeSpace, ServerEvent};
use reqwest::StatusCode;
use support::{Fixture, pattern, sha256_hex};
use tokio::io::AsyncWriteExt;

#[tokio::test]
async fn put_returns_201_with_matching_sha_header() {
	let f = Fixture::start().await;
	let data = pattern(100_000);
	let r = f
		.http
		.put(f.url("report.bin"))
		.body(data.clone())
		.send()
		.await
		.unwrap();
	assert_eq!(r.status(), StatusCode::CREATED);
	assert_eq!(r.headers()[proto::HDR_SAVED_AS], "report.bin");
	assert_eq!(
		r.headers()[proto::HDR_SHA256].to_str().unwrap(),
		sha256_hex(&data)
	);
	assert_eq!(r.text().await.unwrap(), "report.bin");
	assert_eq!(std::fs::read(f.dest().join("report.bin")).unwrap(), data);
	assert!(f.parts().is_empty());
}

#[tokio::test]
async fn json_reply_on_request() {
	let f = Fixture::start().await;
	let r = f
		.http
		.put(f.url("a.txt"))
		.header("accept", "application/json")
		.body("abc")
		.send()
		.await
		.unwrap();
	let v: serde_json::Value = r.json().await.unwrap();
	assert_eq!(v["name"], "a.txt");
	assert_eq!(v["size"], 3);
	assert_eq!(v["sha256"], sha256_hex(b"abc"));
}

#[tokio::test]
async fn post_is_an_alias_for_put() {
	let f = Fixture::start().await;
	let r = f
		.http
		.post(f.url("p.txt"))
		.body("post body")
		.send()
		.await
		.unwrap();
	assert_eq!(r.status(), StatusCode::CREATED);
	assert_eq!(
		std::fs::read_to_string(f.dest().join("p.txt")).unwrap(),
		"post body"
	);
}

#[tokio::test]
async fn chunked_bodies_are_accepted() {
	let f = Fixture::start().await;
	let parts: Vec<Result<Vec<u8>, std::io::Error>> = vec![
		Ok(b"hello ".to_vec()),
		Ok(b"chunked ".to_vec()),
		Ok(b"world".to_vec()),
	];
	let body = reqwest::Body::wrap_stream(futures_util::stream::iter(parts));
	let r = f.http.put(f.url("c.txt")).body(body).send().await.unwrap();
	assert_eq!(r.status(), StatusCode::CREATED);
	assert_eq!(
		r.headers()[proto::HDR_SHA256].to_str().unwrap(),
		sha256_hex(b"hello chunked world")
	);
}

#[tokio::test]
async fn too_large_by_content_length_is_413() {
	let f = Fixture::with(|c| c.settings.max_file_bytes = 1000).await;
	let r = f
		.http
		.put(f.url("big"))
		.body(vec![0u8; 2000])
		.send()
		.await
		.unwrap();
	assert_eq!(r.status(), StatusCode::PAYLOAD_TOO_LARGE);
	assert!(f.files().is_empty());
}

#[tokio::test]
async fn too_large_by_streamed_count_is_413_and_cleans_up() {
	let f = Fixture::with(|c| c.settings.max_file_bytes = 1000).await;
	let chunks: Vec<Result<Vec<u8>, std::io::Error>> =
		(0..10).map(|_| Ok(vec![1u8; 500])).collect();
	let body = reqwest::Body::wrap_stream(futures_util::stream::iter(chunks));
	let r = f.http.put(f.url("big")).body(body).send().await;
	if let Ok(r) = r {
		assert_eq!(r.status(), StatusCode::PAYLOAD_TOO_LARGE);
	}
	assert!(f.wait_no_parts().await);
	assert!(f.files().is_empty());
}

#[tokio::test]
async fn aborted_upload_leaves_no_part_file() {
	let f = Fixture::start().await;
	let mut s = tokio::net::TcpStream::connect(f.addr).await.unwrap();
	let head = format!(
		"PUT /{}/abort.bin HTTP/1.1\r\nHost: x\r\nContent-Length: 1000000\r\n\r\n",
		f.token.expose()
	);
	s.write_all(head.as_bytes()).await.unwrap();
	s.write_all(&vec![7u8; 50_000]).await.unwrap();
	s.flush().await.unwrap();
	// Wait until the server has created the temporary file, then hang up.
	for _ in 0..100 {
		if !f.parts().is_empty() {
			break;
		}
		tokio::time::sleep(Duration::from_millis(20)).await;
	}
	drop(s);
	assert!(
		f.wait_no_parts().await,
		"part file was left behind: {:?}",
		f.files()
	);
	assert!(f.files().is_empty());
}

#[tokio::test]
async fn bad_token_and_unknown_paths_are_indistinguishable() {
	let f = Fixture::start().await;
	let mut seen = Vec::new();
	for path in [
		"",
		"nothing",
		"wrongtoken/",
		"wrongtoken/file",
		"x/y/z",
		&format!("{}x/", f.token.expose()),
		&f.token.expose()[..10],
	] {
		let r = f.http.get(f.raw_url(path)).send().await.unwrap();
		let status = r.status();
		let mut headers: Vec<_> = r
			.headers()
			.iter()
			.map(|(k, v)| (k.to_string(), v.to_str().unwrap().to_owned()))
			.filter(|(k, _)| k != "date")
			.collect();
		headers.sort();
		let body = r.text().await.unwrap();
		seen.push((status, headers, body));
	}
	for s in &seen {
		assert_eq!(s.0, StatusCode::NOT_FOUND);
		assert_eq!(s, &seen[0]);
	}
	let put = f
		.http
		.put(f.raw_url("wrongtoken/file"))
		.body("x")
		.send()
		.await
		.unwrap();
	assert_eq!(put.status(), StatusCode::NOT_FOUND);
	assert!(f.files().is_empty());
}

#[tokio::test]
async fn traversal_and_odd_names_stay_inside_the_destination() {
	let f = Fixture::start().await;
	let cases: &[(&str, &str)] = &[
		("..%2Fx", "x"),
		("%2e%2e%2fx", "x"),
		("a%5Cb", "b"),
		("..%5C..%5Cwin.ini", "win.ini"),
		("%2Ehidden", "hidden"),
		("evil%E2%80%AEgpj.exe", "evilgpj.exe"),
		("a%00b", "ab"),
	];
	for (raw, expected) in cases {
		let r = f
			.http
			.put(f.raw_url(&format!("{}/{raw}", f.token.expose())))
			.body("x")
			.send()
			.await
			.unwrap();
		assert_eq!(r.status(), StatusCode::CREATED, "{raw}");
		let saved = r.headers()[proto::HDR_SAVED_AS]
			.to_str()
			.unwrap()
			.to_owned();
		assert!(
			saved.starts_with(expected.trim_end_matches(".exe")) || saved.starts_with(expected),
			"{raw} saved as {saved}"
		);
		assert!(f.dest().join(&saved).is_file());
	}
	// Nothing escaped the destination.
	let outside: Vec<_> = std::fs::read_dir(f.dir.path())
		.unwrap()
		.filter_map(|e| e.ok())
		.map(|e| e.file_name().to_string_lossy().into_owned())
		.collect();
	assert_eq!(outside, vec!["dest".to_owned()]);
	for name in f.files() {
		assert!(!name.starts_with('.') && !name.contains('/') && !name.contains('\\'));
	}
}

#[tokio::test]
async fn twenty_concurrent_same_name_uploads_make_twenty_files() {
	let f = Fixture::with(|c| c.settings.max_concurrent = 64).await;
	let mut set = tokio::task::JoinSet::new();
	for i in 0..20u8 {
		let (http, url) = (f.http.clone(), f.url("same.txt"));
		set.spawn(async move {
			http.put(url)
				.body(vec![i; 1000])
				.send()
				.await
				.unwrap()
				.status()
		});
	}
	while let Some(s) = set.join_next().await {
		assert_eq!(s.unwrap(), StatusCode::CREATED);
	}
	let files = f.files();
	assert_eq!(files.len(), 20, "{files:?}");
	assert!(files.contains(&"same.txt".to_owned()) && files.contains(&"same-19.txt".to_owned()));
	let mut contents: Vec<u8> = files
		.iter()
		.map(|n| std::fs::read(f.dest().join(n)).unwrap()[0])
		.collect();
	contents.sort();
	assert_eq!(contents, (0..20u8).collect::<Vec<_>>());
}

#[tokio::test]
async fn expected_sha_mismatch_is_422_and_file_removed() {
	let f = Fixture::start().await;
	let r = f
		.http
		.put(f.url("m.bin"))
		.header(proto::HDR_EXPECTED_SHA256, sha256_hex(b"other"))
		.body("data")
		.send()
		.await
		.unwrap();
	assert_eq!(r.status(), StatusCode::UNPROCESSABLE_ENTITY);
	assert!(f.wait_no_parts().await);
	assert!(f.files().is_empty());
	let ok = f
		.http
		.put(f.url("m.bin"))
		.header(proto::HDR_EXPECTED_SHA256, sha256_hex(b"data"))
		.body("data")
		.send()
		.await
		.unwrap();
	assert_eq!(ok.status(), StatusCode::CREATED);
}

#[tokio::test]
async fn content_digest_header_is_honoured() {
	use base64::Engine as _;
	let f = Fixture::start().await;
	let digest =
		base64::engine::general_purpose::STANDARD.encode(hex::decode(sha256_hex(b"data")).unwrap());
	let ok = f
		.http
		.put(f.url("d.bin"))
		.header("content-digest", format!("sha-256=:{digest}:"))
		.body("data")
		.send()
		.await
		.unwrap();
	assert_eq!(ok.status(), StatusCode::CREATED);
	let bad = f
		.http
		.put(f.url("e.bin"))
		.header("content-digest", format!("sha-256=:{digest}:"))
		.body("different")
		.send()
		.await
		.unwrap();
	assert_eq!(bad.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn low_free_space_is_507() {
	let f = Fixture::with(|c| {
		c.settings.min_free_bytes = 1000;
		c.free_space = Arc::new(FixedFreeSpace(500));
	})
	.await;
	let r = f.http.put(f.url("x")).body("hello").send().await.unwrap();
	assert_eq!(r.status(), StatusCode::INSUFFICIENT_STORAGE);
	assert!(f.files().is_empty());
}

#[tokio::test]
async fn concurrency_above_the_limit_is_429() {
	let f = Fixture::with(|c| c.settings.max_concurrent = 1).await;
	let mut s = tokio::net::TcpStream::connect(f.addr).await.unwrap();
	let head = format!(
		"PUT /{}/slow.bin HTTP/1.1\r\nHost: x\r\nContent-Length: 100\r\n\r\nabc",
		f.token.expose()
	);
	s.write_all(head.as_bytes()).await.unwrap();
	for _ in 0..100 {
		if !f.parts().is_empty() {
			break;
		}
		tokio::time::sleep(Duration::from_millis(20)).await;
	}
	let r = f.http.put(f.url("second")).body("x").send().await.unwrap();
	assert_eq!(r.status(), StatusCode::TOO_MANY_REQUESTS);
	assert!(r.headers().contains_key("retry-after"));
	drop(s);
}

#[tokio::test]
async fn security_headers_are_on_every_reply() {
	let f = Fixture::start().await;
	f.http.put(f.url("h.txt")).body("x").send().await.unwrap();
	for r in [
		f.http.get(f.url("")).send().await.unwrap(),
		f.http.get(f.raw_url("nope")).send().await.unwrap(),
		f.http.put(f.url("h2.txt")).body("x").send().await.unwrap(),
		f.http.get(f.url("health")).send().await.unwrap(),
	] {
		let h = r.headers();
		assert_eq!(h["x-content-type-options"], "nosniff");
		assert_eq!(h["cache-control"], "no-store");
		assert_eq!(h["referrer-policy"], "no-referrer");
		assert_eq!(h["x-frame-options"], "DENY");
		assert_eq!(h["server"], "mailslot");
		assert!(
			h["content-security-policy"]
				.to_str()
				.unwrap()
				.starts_with("default-src 'none'")
		);
	}
}

#[tokio::test]
async fn token_without_slash_redirects_with_308() {
	let f = Fixture::start().await;
	let r = f
		.http
		.get(f.raw_url(f.token.expose()))
		.send()
		.await
		.unwrap();
	assert_eq!(r.status(), StatusCode::PERMANENT_REDIRECT);
	assert_eq!(r.headers()["location"], format!("/{}/", f.token.expose()));
}

#[tokio::test]
async fn drop_page_is_served_with_strict_csp_and_no_inline_script() {
	let f = Fixture::start().await;
	let r = f.http.get(f.url("")).send().await.unwrap();
	assert_eq!(r.status(), StatusCode::OK);
	assert_eq!(r.headers()["content-type"], "text/html; charset=utf-8");
	let csp = r.headers()["content-security-policy"]
		.to_str()
		.unwrap()
		.to_owned();
	assert_eq!(csp, proto::CSP);
	assert!(csp.contains("script-src 'self'") && !csp.contains("unsafe-inline"));
	let html = r.text().await.unwrap();
	assert!(html.contains("drop.js"));
	for asset in ["drop.js", "drop.css"] {
		let a = f.http.get(f.url(asset)).send().await.unwrap();
		assert_eq!(a.status(), StatusCode::OK, "{asset}");
	}
	// Uploads are never served back.
	f.http
		.put(f.url("secret.txt"))
		.body("x")
		.send()
		.await
		.unwrap();
	assert_eq!(
		f.http
			.get(f.url("secret.txt"))
			.send()
			.await
			.unwrap()
			.status(),
		StatusCode::METHOD_NOT_ALLOWED
	);
}

#[tokio::test]
async fn health_reports_capabilities() {
	let f = Fixture::with(|c| c.settings.max_file_bytes = 12345).await;
	let v: serde_json::Value = f
		.http
		.get(f.url("health"))
		.send()
		.await
		.unwrap()
		.json()
		.await
		.unwrap();
	assert_eq!(v["ok"], true);
	assert_eq!(v["max_bytes"], 12345);
	assert_eq!(v["accepting"], true);
	assert!(v["expires_at"].is_null());
	let caps: Vec<&str> = v["capabilities"]
		.as_array()
		.unwrap()
		.iter()
		.filter_map(|c| c.as_str())
		.collect();
	assert!(caps.contains(&"resume") && caps.contains(&"dedupe"));
}

#[tokio::test]
async fn empty_name_is_400() {
	let f = Fixture::start().await;
	assert_eq!(
		f.http
			.put(f.url(""))
			.body("x")
			.send()
			.await
			.unwrap()
			.status(),
		StatusCode::BAD_REQUEST
	);
	assert_eq!(
		f.http
			.put(f.raw_url(f.token.expose()))
			.body("x")
			.send()
			.await
			.unwrap()
			.status(),
		StatusCode::BAD_REQUEST
	);
}

#[tokio::test]
async fn expired_link_is_410() {
	let f = Fixture::with(|c| {
		c.settings.expires_at = Some(std::time::SystemTime::now() - Duration::from_secs(1))
	})
	.await;
	assert_eq!(
		f.http
			.put(f.url("late"))
			.body("x")
			.send()
			.await
			.unwrap()
			.status(),
		StatusCode::GONE
	);
	assert_eq!(
		f.http.get(f.url("")).send().await.unwrap().status(),
		StatusCode::GONE
	);
	let v: serde_json::Value = f
		.http
		.get(f.url("health"))
		.send()
		.await
		.unwrap()
		.json()
		.await
		.unwrap();
	assert_eq!(v["accepting"], false);
	assert!(v["expires_at"].is_string());
}

#[tokio::test]
async fn one_shot_mode_accepts_one_file_then_410_and_signals() {
	let mut f = Fixture::with(|c| c.settings.stop_after_files = 1).await;
	assert_eq!(
		f.http
			.put(f.url("one"))
			.body("x")
			.send()
			.await
			.unwrap()
			.status(),
		StatusCode::CREATED
	);
	assert_eq!(
		f.http
			.put(f.url("two"))
			.body("x")
			.send()
			.await
			.unwrap()
			.status(),
		StatusCode::GONE
	);
	let mut saw_limit = false;
	while let Ok(Some(ev)) = tokio::time::timeout(Duration::from_millis(300), f.events.recv()).await
	{
		saw_limit |= ev == ServerEvent::FileLimitReached;
	}
	assert!(saw_limit);
}

#[tokio::test]
async fn session_quota_is_enforced() {
	let f = Fixture::with(|c| c.settings.quota_bytes = 1500).await;
	assert_eq!(
		f.http
			.put(f.url("a"))
			.body(vec![0u8; 1000])
			.send()
			.await
			.unwrap()
			.status(),
		StatusCode::CREATED
	);
	assert_eq!(
		f.http
			.put(f.url("b"))
			.body(vec![0u8; 1000])
			.send()
			.await
			.unwrap()
			.status(),
		StatusCode::INSUFFICIENT_STORAGE
	);
}

#[tokio::test]
async fn disallowed_extension_is_415() {
	let f =
		Fixture::with(|c| c.settings.allowed_extensions = vec!["zip".into(), "txt".into()]).await;
	assert_eq!(
		f.http
			.put(f.url("a.exe"))
			.body("x")
			.send()
			.await
			.unwrap()
			.status(),
		StatusCode::UNSUPPORTED_MEDIA_TYPE
	);
	assert_eq!(
		f.http
			.put(f.url("noext"))
			.body("x")
			.send()
			.await
			.unwrap()
			.status(),
		StatusCode::UNSUPPORTED_MEDIA_TYPE
	);
	assert_eq!(
		f.http
			.put(f.url("a.ZIP"))
			.body("x")
			.send()
			.await
			.unwrap()
			.status(),
		StatusCode::CREATED
	);
}

#[tokio::test]
async fn bad_token_lockout_drops_the_address() {
	let f = Fixture::with(|c| c.settings.lockout_threshold = 3).await;
	for _ in 0..3 {
		let _ = f.http.get(f.raw_url("badbadbad/")).send().await;
	}
	// Even the right token is refused now, and new connections are dropped.
	let fresh = reqwest::Client::builder()
		.pool_max_idle_per_host(0)
		.build()
		.unwrap();
	let r = fresh.get(f.url("health")).send().await;
	assert!(r.is_err() || r.unwrap().status() == StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn rate_limit_returns_429_with_retry_after() {
	let f = Fixture::with(|c| c.settings.rate_limit_per_min = 3).await;
	for _ in 0..3 {
		assert_eq!(
			f.http.get(f.url("health")).send().await.unwrap().status(),
			StatusCode::OK
		);
	}
	let r = f.http.get(f.url("health")).send().await.unwrap();
	assert_eq!(r.status(), StatusCode::TOO_MANY_REQUESTS);
	assert!(r.headers().contains_key("retry-after"));
}

#[tokio::test]
async fn collision_reject_returns_409_and_suffix_never_overwrites() {
	let f = Fixture::start().await;
	f.http.put(f.url("n.txt")).body("one").send().await.unwrap();
	let r = f.http.put(f.url("n.txt")).body("two").send().await.unwrap();
	assert_eq!(r.headers()[proto::HDR_SAVED_AS], "n-1.txt");
	assert_eq!(
		std::fs::read_to_string(f.dest().join("n.txt")).unwrap(),
		"one"
	);
	let g = Fixture::with(|c| c.settings.collision = Collision::Reject).await;
	g.http.put(g.url("n.txt")).body("one").send().await.unwrap();
	assert_eq!(
		g.http
			.put(g.url("n.txt"))
			.body("two")
			.send()
			.await
			.unwrap()
			.status(),
		StatusCode::CONFLICT
	);
	assert_eq!(g.files(), vec!["n.txt".to_owned()]);
}

#[tokio::test]
async fn sidecar_and_subfolder_schemes() {
	let f = Fixture::with(|c| {
		c.settings.sidecars = true;
		c.settings.subfolder = SubfolderScheme::BySender;
	})
	.await;
	let r = f
		.http
		.put(f.url("s.txt"))
		.header(proto::HDR_SENDER, "Jules")
		.body("abc")
		.send()
		.await
		.unwrap();
	assert_eq!(r.status(), StatusCode::CREATED);
	let side = std::fs::read_to_string(f.dest().join("Jules/s.txt.sha256")).unwrap();
	assert_eq!(side, format!("{}  s.txt\n", sha256_hex(b"abc")));
	let g = Fixture::with(|c| c.settings.subfolder = SubfolderScheme::ByDate).await;
	g.http.put(g.url("d.txt")).body("x").send().await.unwrap();
	let day: Vec<_> = std::fs::read_dir(g.dest())
		.unwrap()
		.filter_map(|e| e.ok())
		.filter(|e| e.path().is_dir())
		.collect();
	assert_eq!(day.len(), 1);
	assert!(day[0].path().join("d.txt").is_file());
}

#[tokio::test]
async fn verify_on_receive_accepts_good_files() {
	let f = Fixture::with(|c| c.settings.verify_on_receive = true).await;
	assert_eq!(
		f.http
			.put(f.url("v"))
			.body(pattern(300_000))
			.send()
			.await
			.unwrap()
			.status(),
		StatusCode::CREATED
	);
}

#[tokio::test]
async fn rotating_the_token_kills_the_old_url_and_aborts_inflight_uploads() {
	let f = Fixture::start().await;
	let mut s = tokio::net::TcpStream::connect(f.addr).await.unwrap();
	let head = format!(
		"PUT /{}/inflight.bin HTTP/1.1\r\nHost: x\r\nContent-Length: 100000\r\n\r\nabc",
		f.token.expose()
	);
	s.write_all(head.as_bytes()).await.unwrap();
	for _ in 0..100 {
		if !f.parts().is_empty() {
			break;
		}
		tokio::time::sleep(Duration::from_millis(20)).await;
	}
	let new = mailslot_core::token::Token::generate().unwrap();
	f.handle.rotate_token(new.clone());
	assert_eq!(
		f.http.get(f.url("health")).send().await.unwrap().status(),
		StatusCode::NOT_FOUND
	);
	let ok = f
		.http
		.get(format!("http://{}/{}/health", f.addr, new.expose()))
		.send()
		.await
		.unwrap();
	assert_eq!(ok.status(), StatusCode::OK);
	assert!(f.wait_no_parts().await);
	assert!(f.files().is_empty());
	drop(s);
}

#[tokio::test]
async fn resume_with_head_offset_and_content_range() {
	let f = Fixture::start().await;
	let data = pattern(10_000);
	let total = data.len();
	let id = "resume-test-0001";
	// First half as a partial range.
	let r = f
		.http
		.put(f.url("big.bin"))
		.header(proto::HDR_UPLOAD_ID, id)
		.header("content-range", format!("bytes 0-4999/{total}"))
		.body(data[..5000].to_vec())
		.send()
		.await
		.unwrap();
	assert_eq!(r.status(), StatusCode::ACCEPTED);
	assert_eq!(r.headers()[proto::HDR_OFFSET], "5000");
	assert!(f.files().is_empty() || f.files().iter().all(|n| n.ends_with(".part")));
	// HEAD reports the offset.
	let h = f
		.http
		.head(f.url("big.bin"))
		.header(proto::HDR_UPLOAD_ID, id)
		.send()
		.await
		.unwrap();
	assert_eq!(h.headers()[proto::HDR_OFFSET], "5000");
	// A wrong offset is refused with the true one.
	let wrong = f
		.http
		.put(f.url("big.bin"))
		.header(proto::HDR_UPLOAD_ID, id)
		.header("content-range", format!("bytes 100-{}/{total}", total - 1))
		.body(data[100..].to_vec())
		.send()
		.await
		.unwrap();
	assert_eq!(wrong.status(), StatusCode::CONFLICT);
	assert_eq!(wrong.headers()[proto::HDR_OFFSET], "5000");
	// Finish.
	let done = f
		.http
		.put(f.url("big.bin"))
		.header(proto::HDR_UPLOAD_ID, id)
		.header(proto::HDR_EXPECTED_SHA256, sha256_hex(&data))
		.header("content-range", format!("bytes 5000-{}/{total}", total - 1))
		.body(data[5000..].to_vec())
		.send()
		.await
		.unwrap();
	assert_eq!(done.status(), StatusCode::CREATED);
	assert_eq!(
		done.headers()[proto::HDR_SHA256].to_str().unwrap(),
		sha256_hex(&data)
	);
	assert_eq!(std::fs::read(f.dest().join("big.bin")).unwrap(), data);
	assert!(f.parts().is_empty());
	// After completion the offset is back to zero.
	let h = f
		.http
		.head(f.url("big.bin"))
		.header(proto::HDR_UPLOAD_ID, id)
		.send()
		.await
		.unwrap();
	assert_eq!(h.headers()[proto::HDR_OFFSET], "0");
}

#[tokio::test]
async fn interrupted_resumable_upload_keeps_its_partial() {
	let f = Fixture::start().await;
	let data = pattern(200_000);
	let id = "resume-test-0002";
	let mut s = tokio::net::TcpStream::connect(f.addr).await.unwrap();
	let head = format!(
		"PUT /{}/cut.bin HTTP/1.1\r\nHost: x\r\nX-Mailslot-Upload-Id: {id}\r\nContent-Range: bytes 0-{}/{}\r\nContent-Length: {}\r\n\r\n",
		f.token.expose(),
		data.len() - 1,
		data.len(),
		data.len()
	);
	s.write_all(head.as_bytes()).await.unwrap();
	s.write_all(&data[..64 * 1024]).await.unwrap();
	s.flush().await.unwrap();
	tokio::time::sleep(Duration::from_millis(200)).await;
	drop(s);
	let mut offset = 0u64;
	for _ in 0..100 {
		let h = f
			.http
			.head(f.url("cut.bin"))
			.header(proto::HDR_UPLOAD_ID, id)
			.send()
			.await
			.unwrap();
		offset = h.headers()[proto::HDR_OFFSET]
			.to_str()
			.unwrap()
			.parse()
			.unwrap();
		if offset > 0 {
			break;
		}
		tokio::time::sleep(Duration::from_millis(30)).await;
	}
	assert!(offset > 0 && offset <= 64 * 1024, "offset {offset}");
	let off = offset as usize;
	let done = f
		.http
		.put(f.url("cut.bin"))
		.header(proto::HDR_UPLOAD_ID, id)
		.header(
			"content-range",
			format!("bytes {off}-{}/{}", data.len() - 1, data.len()),
		)
		.body(data[off..].to_vec())
		.send()
		.await
		.unwrap();
	assert_eq!(done.status(), StatusCode::CREATED);
	assert_eq!(std::fs::read(f.dest().join("cut.bin")).unwrap(), data);
}

#[tokio::test]
async fn have_endpoint_dedupes_by_hash() {
	let f = Fixture::start().await;
	let sha = sha256_hex(b"known content");
	let q = |sha: &str| f.url(&format!("have?sha256={sha}"));
	let before: serde_json::Value = f
		.http
		.get(q(&sha))
		.send()
		.await
		.unwrap()
		.json()
		.await
		.unwrap();
	assert_eq!(before["have"], false);
	f.http
		.put(f.url("k.txt"))
		.body("known content")
		.send()
		.await
		.unwrap();
	let after: serde_json::Value = f
		.http
		.get(q(&sha))
		.send()
		.await
		.unwrap()
		.json()
		.await
		.unwrap();
	assert_eq!(after["have"], true);
	// Deleting the file means the server no longer claims to have it.
	std::fs::remove_file(f.dest().join("k.txt")).unwrap();
	let gone: serde_json::Value = f
		.http
		.get(q(&sha))
		.send()
		.await
		.unwrap()
		.json()
		.await
		.unwrap();
	assert_eq!(gone["have"], false);
	assert_eq!(
		f.http
			.get(f.url("have?sha256=zz"))
			.send()
			.await
			.unwrap()
			.status(),
		StatusCode::BAD_REQUEST
	);
}

#[tokio::test]
async fn fifty_concurrent_uploads_do_not_stall() {
	let f = Fixture::with(|c| c.settings.max_concurrent = 100).await;
	let started = std::time::Instant::now();
	let mut set = tokio::task::JoinSet::new();
	for i in 0..50usize {
		let (http, url) = (f.http.clone(), f.url(&format!("load-{i}.bin")));
		set.spawn(async move {
			let data = pattern(512 * 1024 + i);
			let r = http.put(url).body(data.clone()).send().await.unwrap();
			(
				r.status(),
				r.headers()[proto::HDR_SHA256].to_str().unwrap().to_owned(),
				sha256_hex(&data),
			)
		});
	}
	// While the load runs, health must keep answering quickly.
	let mut worst = Duration::ZERO;
	let health = async {
		for _ in 0..10 {
			let t = std::time::Instant::now();
			f.http.get(f.url("health")).send().await.unwrap();
			worst = worst.max(t.elapsed());
			tokio::time::sleep(Duration::from_millis(20)).await;
		}
	};
	let results = async {
		let mut out = Vec::new();
		while let Some(r) = set.join_next().await {
			out.push(r.unwrap());
		}
		out
	};
	let (results, ()) = tokio::join!(results, health);
	assert_eq!(results.len(), 50);
	for (status, got, want) in results {
		assert_eq!(status, StatusCode::CREATED);
		assert_eq!(got, want);
	}
	assert_eq!(f.files().len(), 50);
	assert!(
		worst < Duration::from_secs(2),
		"health stalled for {worst:?}"
	);
	assert!(started.elapsed() < Duration::from_secs(30));
}

#[tokio::test]
async fn events_report_progress_and_completion() {
	let mut f = Fixture::start().await;
	f.http
		.put(f.url("e.bin"))
		.body(pattern(2_000_000))
		.send()
		.await
		.unwrap();
	let mut started = false;
	let mut finished = false;
	while let Ok(Some(ev)) = tokio::time::timeout(Duration::from_millis(200), f.events.recv()).await
	{
		match ev {
			ServerEvent::Upload(UploadEvent::Started { name, expected, .. }) => {
				assert_eq!(name, "e.bin");
				assert_eq!(expected, Some(2_000_000));
				started = true;
			}
			ServerEvent::Upload(UploadEvent::Finished {
				size,
				sha256,
				saved_as,
				..
			}) => {
				assert_eq!((size, saved_as.as_str()), (2_000_000, "e.bin"));
				assert_eq!(sha256, sha256_hex(&pattern(2_000_000)));
				finished = true;
			}
			_ => {}
		}
	}
	assert!(started && finished);
}

#[tokio::test]
async fn curl_upload_interop() {
	if std::process::Command::new("curl")
		.arg("--version")
		.output()
		.is_err()
	{
		eprintln!("curl not installed; skipping");
		return;
	}
	let f = Fixture::start().await;
	let src = f.dir.path().join("curl-src.bin");
	std::fs::write(&src, pattern(300_000)).unwrap();
	let out = tokio::process::Command::new("curl")
		.args(["--silent", "--fail", "-i", "-T"])
		.arg(&src)
		.arg(f.url("curl-src.bin"))
		.output()
		.await
		.unwrap();
	assert!(
		out.status.success(),
		"{}",
		String::from_utf8_lossy(&out.stderr)
	);
	let text = String::from_utf8_lossy(&out.stdout).to_lowercase();
	assert!(text.contains(&format!(
		"x-mailslot-sha256: {}",
		sha256_hex(&pattern(300_000))
	)));
	// Chunked from stdin.
	let mut child = tokio::process::Command::new("curl")
		.args(["--silent", "--fail", "-T", "-"])
		.arg(f.url("stdin.bin"))
		.stdin(std::process::Stdio::piped())
		.stdout(std::process::Stdio::null())
		.spawn()
		.unwrap();
	child
		.stdin
		.take()
		.unwrap()
		.write_all(b"from stdin")
		.await
		.unwrap();
	assert!(child.wait().await.unwrap().success());
	assert_eq!(
		std::fs::read(f.dest().join("stdin.bin")).unwrap(),
		b"from stdin"
	);
}

#[cfg(unix)]
#[tokio::test]
async fn part_file_is_private_and_final_file_is_not_executable() {
	use std::os::unix::fs::PermissionsExt;
	use tokio::io::AsyncReadExt;

	let f = Fixture::start().await;
	let mut s = tokio::net::TcpStream::connect(f.addr).await.unwrap();
	let head = format!(
		"PUT /{}/mode.bin HTTP/1.1\r\nHost: x\r\nContent-Length: 2000\r\nConnection: close\r\n\r\n",
		f.token.expose()
	);
	s.write_all(head.as_bytes()).await.unwrap();
	s.write_all(&[1u8; 1000]).await.unwrap();
	s.flush().await.unwrap();
	// Mid-stream: the temporary file exists and is readable only by its owner.
	for _ in 0..100 {
		if !f.parts().is_empty() {
			break;
		}
		tokio::time::sleep(Duration::from_millis(20)).await;
	}
	let parts = f.parts();
	assert_eq!(parts.len(), 1, "{parts:?}");
	let mode = std::fs::metadata(f.dest().join(&parts[0]))
		.unwrap()
		.permissions()
		.mode();
	assert_eq!(mode & 0o777, 0o600, "part mode is {mode:o}");

	s.write_all(&[1u8; 1000]).await.unwrap();
	let mut resp = String::new();
	s.read_to_string(&mut resp).await.unwrap();
	assert!(resp.starts_with("HTTP/1.1 201"), "{resp}");
	let mode = std::fs::metadata(f.dest().join("mode.bin"))
		.unwrap()
		.permissions()
		.mode();
	assert_eq!(mode & 0o777, 0o644, "final mode is {mode:o}");
	assert_eq!(mode & 0o111, 0, "no execute bits");
}
