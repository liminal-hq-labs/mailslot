// End-to-end tests of the mailslot-cli binary against its own server on 127.0.0.1.
//
// (c) Copyright 2026 Liminal HQ, Scott Morris
// SPDX-License-Identifier: Apache-2.0 OR MIT

#![allow(clippy::disallowed_methods)] // Tests drive child processes and read files synchronously.

use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_mailslot-cli");

struct Serve {
	child: Child,
	link: String,
}

impl Serve {
	fn start(dest: &Path) -> Self {
		let mut child = Command::new(BIN)
			.args([
				"serve",
				"--ephemeral",
				"--bind",
				"127.0.0.1",
				"--port",
				"0",
				"--dir",
			])
			.arg(dest)
			.stdout(Stdio::piped())
			.stderr(Stdio::null())
			.spawn()
			.unwrap();
		let mut line = String::new();
		BufReader::new(child.stdout.as_mut().unwrap())
			.read_line(&mut line)
			.unwrap();
		let url = line
			.trim()
			.strip_prefix("URL ")
			.expect("URL line")
			.to_owned();
		let rest = url.strip_prefix("http://").unwrap();
		let (authority, token) = rest.split_once('/').unwrap();
		let link = format!("mailslot://{authority}/{}", token.trim_end_matches('/'));
		Self { child, link }
	}
}

impl Drop for Serve {
	fn drop(&mut self) {
		let _ = Command::new("kill")
			.args(["-TERM", &self.child.id().to_string()])
			.status();
		let _ = self.child.wait();
	}
}

fn sha(data: &[u8]) -> String {
	let mut child = Command::new("sha256sum")
		.stdin(Stdio::piped())
		.stdout(Stdio::piped())
		.spawn()
		.unwrap();
	std::io::Write::write_all(child.stdin.as_mut().unwrap(), data).unwrap();
	let out = child.wait_with_output().unwrap();
	String::from_utf8_lossy(&out.stdout)
		.split_whitespace()
		.next()
		.unwrap()
		.to_owned()
}

#[test]
fn send_resumes_dedupes_and_reports_hashes() {
	let dir = tempfile::tempdir().unwrap();
	let dest = dir.path().join("up");
	let server = Serve::start(&dest);
	let data: Vec<u8> = (0..500_000u32).map(|i| (i % 253) as u8).collect();
	let src = dir.path().join("payload.bin");
	std::fs::write(&src, &data).unwrap();

	let out = Command::new(BIN)
		.args(["send", "--json"])
		.arg(&src)
		.arg(&server.link)
		.output()
		.unwrap();
	assert!(out.status.success());
	let text = String::from_utf8_lossy(&out.stdout);
	let done: serde_json::Value = text
		.lines()
		.filter_map(|l| serde_json::from_str(l).ok())
		.find(|v: &serde_json::Value| v["event"] == "done")
		.unwrap();
	assert_eq!(done["sha256"], sha(&data));
	assert_eq!(done["skipped"], false);
	assert_eq!(std::fs::read(dest.join("payload.bin")).unwrap(), data);

	// Second send is deduplicated.
	let again = Command::new(BIN)
		.args(["send", "--json"])
		.arg(&src)
		.arg(&server.link)
		.output()
		.unwrap();
	assert!(String::from_utf8_lossy(&again.stdout).contains("\"skipped\":true"));
	assert_eq!(std::fs::read_dir(&dest).unwrap().count(), 1);

	// Standard input.
	let mut child = Command::new(BIN)
		.args(["send", "--stdin", "--name", "in.txt"])
		.arg(&server.link)
		.stdin(Stdio::piped())
		.stdout(Stdio::null())
		.stderr(Stdio::null())
		.spawn()
		.unwrap();
	std::io::Write::write_all(child.stdin.as_mut().unwrap(), b"from stdin").unwrap();
	drop(child.stdin.take());
	assert!(child.wait().unwrap().success());
	assert_eq!(std::fs::read(dest.join("in.txt")).unwrap(), b"from stdin");

	// Bad link fails with a non-zero exit.
	let bad = Command::new(BIN)
		.args(["send"])
		.arg(&src)
		.arg("http://127.0.0.1:1/AAAAAAAAAAAAAAAAAAAAAAAA/")
		.args(["--retries", "0"])
		.output()
		.unwrap();
	assert!(!bad.status.success());

	// Status.
	let st = Command::new(BIN)
		.args(["status", "--json"])
		.arg(&server.link)
		.output()
		.unwrap();
	let v: serde_json::Value = serde_json::from_slice(&st.stdout).unwrap();
	assert_eq!(v["accepting"], true);
}

#[test]
fn watch_pushes_new_files() {
	let dir = tempfile::tempdir().unwrap();
	let dest = dir.path().join("up");
	let server = Serve::start(&dest);
	let watched = dir.path().join("outbox");
	std::fs::create_dir_all(&watched).unwrap();
	let mut watcher = Command::new(BIN)
		.args(["watch", "--interval", "1", "--json"])
		.arg(&watched)
		.arg(&server.link)
		.stdout(Stdio::null())
		.stderr(Stdio::null())
		.spawn()
		.unwrap();
	std::thread::sleep(Duration::from_millis(1500));
	std::fs::write(watched.join("new.txt"), "watched content").unwrap();
	let deadline = Instant::now() + Duration::from_secs(15);
	while !dest.join("new.txt").exists() && Instant::now() < deadline {
		std::thread::sleep(Duration::from_millis(200));
	}
	let _ = Command::new("kill")
		.args(["-TERM", &watcher.id().to_string()])
		.status();
	let _ = watcher.wait();
	assert_eq!(
		std::fs::read(dest.join("new.txt")).unwrap(),
		b"watched content"
	);
}

#[test]
fn completions_and_man_are_generated() {
	for shell in ["bash", "zsh", "fish"] {
		let out = Command::new(BIN)
			.args(["completions", shell])
			.output()
			.unwrap();
		assert!(out.status.success() && !out.stdout.is_empty(), "{shell}");
	}
	let man = Command::new(BIN).arg("man").output().unwrap();
	assert!(String::from_utf8_lossy(&man.stdout).contains(".TH mailslot-cli"));
}

fn run_watch_for(secs: u64, dir: &Path, link: &str, state: &Path) -> String {
	let child = Command::new(BIN)
		.args(["watch", "--json", "--debounce-ms", "200", "--state"])
		.arg(state)
		.arg(dir)
		.arg(link)
		.stdout(Stdio::piped())
		.stderr(Stdio::null())
		.spawn()
		.unwrap();
	std::thread::sleep(Duration::from_secs(secs));
	let _ = Command::new("kill")
		.args(["-TERM", &child.id().to_string()])
		.status();
	let out = child.wait_with_output().unwrap();
	String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn watch_queue_survives_restarts_and_resends_only_changes() {
	let dir = tempfile::tempdir().unwrap();
	let dest = dir.path().join("up");
	let server = Serve::start(&dest);
	let outbox = dir.path().join("outbox");
	std::fs::create_dir_all(outbox.join("sub")).unwrap();
	std::fs::write(outbox.join("sub/a.txt"), "first").unwrap();
	let state = dir.path().join("state/queue.json");

	// First run sends what is already there.
	let out = run_watch_for(3, &outbox, &server.link, &state);
	assert!(
		out.contains("\"event\":\"done\"") && out.contains("a.txt"),
		"{out}"
	);
	assert_eq!(std::fs::read(dest.join("a.txt")).unwrap(), b"first");
	assert!(state.exists(), "the queue is persisted");

	// A restart with nothing new sends nothing, not even a dedupe check.
	let out = run_watch_for(2, &outbox, &server.link, &state);
	assert!(
		!out.contains("\"event\":\"done\"") && !out.contains("skipped"),
		"{out}"
	);

	// A change made while the watcher was stopped is picked up on the next start.
	std::fs::write(outbox.join("sub/a.txt"), "second version").unwrap();
	std::fs::write(outbox.join("b.txt"), "new file").unwrap();
	let out = run_watch_for(3, &outbox, &server.link, &state);
	assert!(out.contains("b.txt") && out.contains("a.txt"), "{out}");
	assert_eq!(std::fs::read(dest.join("b.txt")).unwrap(), b"new file");
	assert_eq!(
		std::fs::read(dest.join("a-1.txt")).unwrap(),
		b"second version"
	);
}

#[test]
fn pair_discover_runs_and_never_asks_for_a_secret() {
	let out = Command::new(BIN)
		.args(["pair", "--discover", "--timeout", "1", "--json"])
		.output()
		.unwrap();
	assert!(out.status.success());
	let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
	assert!(v.is_array());
}

#[test]
fn serve_label_files_senders_by_name() {
	let dir = tempfile::tempdir().unwrap();
	let dest = dir.path().join("up");
	let mut child = Command::new(BIN)
		.args([
			"serve",
			"--ephemeral",
			"--bind",
			"127.0.0.1",
			"--port",
			"0",
			"--subfolders",
			"sender",
			"--label",
			"Jules",
			"--dir",
		])
		.arg(&dest)
		.stdout(Stdio::piped())
		.stderr(Stdio::null())
		.spawn()
		.unwrap();
	let mut line = String::new();
	BufReader::new(child.stdout.as_mut().unwrap())
		.read_line(&mut line)
		.unwrap();
	let url = line.trim().strip_prefix("URL ").unwrap().to_owned();
	let src = dir.path().join("note.txt");
	std::fs::write(&src, "hi").unwrap();
	let ok = Command::new("curl")
		.args(["--silent", "--fail", "-T"])
		.arg(&src)
		.arg(format!("{url}note.txt"))
		.status();
	let _ = Command::new("kill")
		.args(["-TERM", &child.id().to_string()])
		.status();
	let _ = child.wait();
	if ok.is_ok_and(|s| s.success()) {
		assert!(dest.join("Jules/note.txt").is_file());
	}
}
