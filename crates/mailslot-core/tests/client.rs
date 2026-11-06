// Client and server interoperating: resume, dedupe, retry, parallel, bandwidth, stdin and TLS pin.
//
// (c) Copyright 2026 Liminal HQ, Scott Morris
// SPDX-License-Identifier: Apache-2.0 OR MIT

#![allow(clippy::disallowed_methods)] // Tests may use blocking std calls for set-up and assertions.

mod support;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use mailslot_core::client::{Client, ClientOptions, Notice};
use mailslot_core::proto::{self, MailslotLink};
use sha2::{Digest, Sha256};
use support::{Fixture, pattern, sha256_hex};
use tokio::io::AsyncWriteExt;

fn link(f: &Fixture) -> MailslotLink {
	MailslotLink::new("127.0.0.1", f.addr.port(), f.token.clone())
}

fn write(f: &Fixture, name: &str, data: &[u8]) -> std::path::PathBuf {
	let p = f.dir.path().join(name);
	std::fs::write(&p, data).unwrap();
	p
}

#[tokio::test]
async fn sends_a_file_and_hashes_match() {
	let f = Fixture::start().await;
	let data = pattern(1_500_000);
	let src = write(&f, "doc.bin", &data);
	let progress = Arc::new(Mutex::new(Vec::new()));
	let p2 = progress.clone();
	let client = Client::new(link(&f), ClientOptions::default())
		.unwrap()
		.on_progress(move |p| p2.lock().unwrap().push(p.sent));
	let out = client.send_file(&src, None).await.unwrap();
	assert_eq!(out.sha256, sha256_hex(&data));
	assert_eq!(out.saved_as, "doc.bin");
	assert!(!out.skipped);
	assert_eq!(std::fs::read(f.dest().join("doc.bin")).unwrap(), data);
	let seen = progress.lock().unwrap().clone();
	assert_eq!(*seen.last().unwrap(), data.len() as u64);
	assert!(seen.windows(2).all(|w| w[0] <= w[1]));
}

#[tokio::test]
async fn health_and_empty_files() {
	let f = Fixture::start().await;
	let client = Client::new(link(&f), ClientOptions::default()).unwrap();
	let h = client.health().await.unwrap();
	assert!(h.ok && h.accepting && h.capabilities.iter().any(|c| c == "resume"));
	let empty = write(&f, "empty.txt", b"");
	let out = client.send_file(&empty, None).await.unwrap();
	assert_eq!(out.size, 0);
	assert_eq!(std::fs::read(f.dest().join("empty.txt")).unwrap().len(), 0);
}

#[tokio::test]
async fn dedupe_skips_a_file_the_server_already_has() {
	let f = Fixture::start().await;
	let src = write(&f, "same.bin", &pattern(50_000));
	let client = Client::new(link(&f), ClientOptions::default()).unwrap();
	assert!(!client.send_file(&src, None).await.unwrap().skipped);
	let again = client.send_file(&src, None).await.unwrap();
	assert!(again.skipped);
	assert_eq!(f.files(), vec!["same.bin".to_owned()]);
	// Renamed copies of the same content are skipped too.
	let copy = write(&f, "copy.bin", &pattern(50_000));
	assert!(client.send_file(&copy, None).await.unwrap().skipped);
	// With dedupe off the file is sent again and kept under a new name.
	let opts = ClientOptions {
		dedupe: false,
		..Default::default()
	};
	let out = Client::new(link(&f), opts)
		.unwrap()
		.send_file(&src, None)
		.await
		.unwrap();
	assert_eq!(out.saved_as, "same-1.bin");
}

#[tokio::test]
async fn resumes_from_the_offset_the_server_holds() {
	let f = Fixture::start().await;
	let data = pattern(400_000);
	let name = "resume.bin";
	let src = write(&f, name, &data);
	// Seed a partial upload with the id the client derives.
	let sha = sha256_hex(&data);
	let id = hex::encode(&Sha256::digest(format!("{sha}:{name}").as_bytes())[..16]);
	let half = 150_000usize;
	let r = f
		.http
		.put(f.url(name))
		.header(proto::HDR_UPLOAD_ID, &id)
		.header(
			"content-range",
			format!("bytes 0-{}/{}", half - 1, data.len()),
		)
		.body(data[..half].to_vec())
		.send()
		.await
		.unwrap();
	assert_eq!(r.status(), reqwest::StatusCode::ACCEPTED);
	let notices = Arc::new(Mutex::new(Vec::new()));
	let n2 = notices.clone();
	let client = Client::new(link(&f), ClientOptions::default())
		.unwrap()
		.on_notice(move |n| n2.lock().unwrap().push(n));
	let out = client.send_file(&src, None).await.unwrap();
	assert_eq!(out.sha256, sha);
	assert_eq!(std::fs::read(f.dest().join(name)).unwrap(), data);
	assert!(notices.lock().unwrap().contains(&Notice::Resumed {
		offset: half as u64
	}));
}

#[tokio::test]
async fn retries_with_backoff_when_the_server_is_busy() {
	let f = Fixture::with(|c| c.settings.max_concurrent = 1).await;
	// Occupy the only slot for a moment.
	let mut s = tokio::net::TcpStream::connect(f.addr).await.unwrap();
	let head = format!(
		"PUT /{}/hog.bin HTTP/1.1\r\nHost: x\r\nContent-Length: 100\r\n\r\nabc",
		f.token.expose()
	);
	s.write_all(head.as_bytes()).await.unwrap();
	tokio::time::sleep(Duration::from_millis(150)).await;
	let src = write(&f, "late.txt", b"retry me");
	let notices = Arc::new(Mutex::new(Vec::new()));
	let n2 = notices.clone();
	let opts = ClientOptions {
		backoff_base: Duration::from_millis(50),
		..Default::default()
	};
	let client = Client::new(link(&f), opts)
		.unwrap()
		.on_notice(move |n| n2.lock().unwrap().push(n));
	let release = tokio::spawn(async move {
		tokio::time::sleep(Duration::from_millis(400)).await;
		drop(s);
	});
	let out = client.send_file(&src, None).await.unwrap();
	release.await.unwrap();
	assert_eq!(out.saved_as, "late.txt");
	assert!(
		notices
			.lock()
			.unwrap()
			.iter()
			.any(|n| matches!(n, Notice::Retrying { .. }))
	);
}

#[tokio::test]
async fn fatal_errors_do_not_retry() {
	let f = Fixture::with(|c| c.settings.max_file_bytes = 10).await;
	let src = write(&f, "toobig.bin", &pattern(1000));
	let client = Client::new(
		link(&f),
		ClientOptions {
			retries: 5,
			backoff_base: Duration::from_secs(30),
			..Default::default()
		},
	)
	.unwrap();
	let started = Instant::now();
	let err = client.send_file(&src, None).await.unwrap_err().to_string();
	assert!(err.contains("413"), "{err}");
	assert!(started.elapsed() < Duration::from_secs(5));
	let wrong = MailslotLink::new(
		"127.0.0.1",
		f.addr.port(),
		mailslot_core::token::Token::generate().unwrap(),
	);
	let err = Client::new(wrong, ClientOptions::default())
		.unwrap()
		.send_file(&src, None)
		.await
		.unwrap_err()
		.to_string();
	assert!(err.contains("404"), "{err}");
}

#[tokio::test]
async fn sends_many_files_in_parallel_in_order() {
	let f = Fixture::with(|c| c.settings.max_concurrent = 16).await;
	let paths: Vec<_> = (0..8)
		.map(|i| write(&f, &format!("p{i}.bin"), &pattern(100_000 + i)))
		.collect();
	let client = Client::new(
		link(&f),
		ClientOptions {
			parallel: 4,
			..Default::default()
		},
	)
	.unwrap();
	let results = client.send_many(paths).await;
	assert_eq!(results.len(), 8);
	for (i, r) in results.into_iter().enumerate() {
		let o = r.unwrap();
		assert_eq!(o.name, format!("p{i}.bin"));
		assert_eq!(o.sha256, sha256_hex(&pattern(100_000 + i)));
	}
}

#[tokio::test]
async fn bandwidth_limit_slows_the_transfer() {
	let f = Fixture::start().await;
	let src = write(&f, "slow.bin", &pattern(400_000));
	let opts = ClientOptions {
		bandwidth_limit: Some(200_000),
		dedupe: false,
		..Default::default()
	};
	let started = Instant::now();
	Client::new(link(&f), opts)
		.unwrap()
		.send_file(&src, None)
		.await
		.unwrap();
	assert!(
		started.elapsed() >= Duration::from_millis(1500),
		"took {:?}",
		started.elapsed()
	);
}

#[tokio::test]
async fn streams_unknown_length_input_like_stdin() {
	let f = Fixture::start().await;
	let (mut tx, rx) = tokio::io::duplex(64 * 1024);
	let data = pattern(250_000);
	let d2 = data.clone();
	tokio::spawn(async move {
		tx.write_all(&d2).await.unwrap();
		tx.shutdown().await.unwrap();
	});
	let out = Client::new(link(&f), ClientOptions::default())
		.unwrap()
		.send_stream(rx, "piped.bin")
		.await
		.unwrap();
	assert_eq!(out.sha256, sha256_hex(&data));
	assert_eq!(out.size, data.len() as u64);
	assert_eq!(std::fs::read(f.dest().join("piped.bin")).unwrap(), data);
}

#[tokio::test]
async fn expands_folders() {
	let f = Fixture::start().await;
	let root = f.dir.path().join("tree");
	std::fs::create_dir_all(root.join("a/b")).unwrap();
	std::fs::write(root.join("x.txt"), "x").unwrap();
	std::fs::write(root.join("a/y.txt"), "y").unwrap();
	std::fs::write(root.join("a/b/z.txt"), "z").unwrap();
	let files = mailslot_core::client::expand_paths(&[root]).await.unwrap();
	assert_eq!(files.len(), 3);
}

#[tokio::test]
async fn https_with_a_key_pin_works_and_a_wrong_pin_is_refused() {
	let dir = tempfile::tempdir().unwrap();
	let material = mailslot_core::tls::load_or_generate(dir.path(), &[])
		.await
		.unwrap();
	let m2 = material.clone();
	let f = Fixture::with(move |c| c.tls = Some(m2)).await;
	let src = write(&f, "tls.txt", b"over tls");
	let good = MailslotLink::new("127.0.0.1", f.addr.port(), f.token.clone())
		.with_tls(Some(material.pin.clone()));
	let out = Client::new(good, ClientOptions::default())
		.unwrap()
		.send_file(&src, None)
		.await
		.unwrap();
	assert_eq!(out.saved_as, "tls.txt");
	assert_eq!(
		std::fs::read(f.dest().join("tls.txt")).unwrap(),
		b"over tls"
	);
	let bad = MailslotLink::new("127.0.0.1", f.addr.port(), f.token.clone())
		.with_tls(Some("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=".into()));
	let opts = ClientOptions {
		retries: 0,
		..Default::default()
	};
	assert!(
		Client::new(bad, opts.clone())
			.unwrap()
			.send_file(&src, None)
			.await
			.is_err()
	);
	let unpinned = MailslotLink::new("127.0.0.1", f.addr.port(), f.token.clone()).with_tls(None);
	assert!(
		Client::new(unpinned, opts)
			.unwrap()
			.send_file(&src, None)
			.await
			.is_err()
	);
}
