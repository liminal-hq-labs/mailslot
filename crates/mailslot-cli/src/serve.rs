// The serve command: a headless share.
//
// (c) Copyright 2026 Liminal HQ, Scott Morris
// SPDX-License-Identifier: Apache-2.0 OR MIT

use std::process::ExitCode;

use anyhow::{Context, Result};
use mailslot_core::config::{Config, Paths};
use mailslot_core::events::{Event, InternetEvent, InternetState, StopReason, UploadEvent};
use mailslot_core::{Engine, EngineBuilder};
use tokio::signal::unix::{SignalKind, signal};
use tokio_stream::StreamExt;

use crate::ServeArgs;

pub async fn run(a: ServeArgs) -> Result<ExitCode> {
	mailslot_core::tls::ensure_default_provider();
	let paths = match &a.home {
		Some(h) => Paths::in_dir(h),
		None => Paths::xdg(),
	};
	let mut config = if a.ephemeral {
		Config::default()
	} else {
		Config::load(&paths)
			.await
			.context("reading the settings file")?
	};
	if let Some(d) = &a.dir {
		config.storage.dest_dir = d.clone();
	}
	if let Some(b) = &a.bind {
		config.sharing.bind = b.clone();
	}
	if let Some(p) = a.port {
		config.sharing.port = p;
	}
	if let Some(m) = a.max_mb {
		config.limits.max_file_mb = m;
	}
	if let Some(m) = a.expiry_minutes {
		config.sharing.expiry_minutes = m;
	}
	if let Some(n) = a.stop_after {
		config.sharing.stop_after_files = n;
	}
	if let Some(l) = &a.label {
		config.sharing.link_label = l.clone();
	}
	if let Some(m) = &a.subfolders {
		use mailslot_core::config::SubfolderScheme::*;
		config.storage.subfolder = match m.as_str() {
			"date" => ByDate,
			"sender" => BySender,
			_ => None,
		};
	}
	if a.mdns {
		config.network.mdns = true;
	}
	if a.https {
		config.security.https = true;
	}
	if a.upnp {
		config.network.upnp = true;
	}

	let builder = EngineBuilder::new(paths.clone())
		.config(config)
		.persist(!a.ephemeral);
	let engine = builder.build().await.context("preparing the engine")?;
	if a.new_token {
		engine.rotate_token().await?;
	}
	let mut events = engine.subscribe();
	let state = engine.start().await.context("starting the share")?;
	let first = state.urls.first().context("no address to share")?;
	if a.json {
		println!(
			"{}",
			serde_json::json!({ "url": first.url, "link": first.link, "port": state.addr.map(|s| s.port()), "tls_pin": state.tls_pin, "urls": state.urls.iter().map(|u| &u.url).collect::<Vec<_>>() })
		);
	} else {
		println!("URL {}", first.url);
		eprintln!("Link for mailslot-cli: {}", first.link);
		for u in state.urls.iter().skip(1) {
			eprintln!("Also reachable at ({}): {}", u.label, u.url);
		}
		eprintln!("Saving to {}", engine.config().storage.dest_dir.display());
		if let Some(pin) = &state.tls_pin {
			eprintln!("HTTPS key pin: sha256//{pin}");
		}
	}

	let mut term = signal(SignalKind::terminate())?;
	let mut int = signal(SignalKind::interrupt())?;
	let code = loop {
		tokio::select! {
			_ = term.recv() => break shutdown(&engine).await,
			_ = int.recv() => break shutdown(&engine).await,
			ev = events.next() => match ev {
				Some(Event::Upload(UploadEvent::Finished { saved_as, size, sha256, .. })) => eprintln!("saved {saved_as} ({size} bytes) sha256={sha256}"),
				Some(Event::Upload(UploadEvent::Failed { reason, .. })) => eprintln!("upload failed: {reason}"),
				Some(Event::Internet(InternetEvent::State(InternetState::Open { url, warnings, .. }))) => {
					eprintln!("Open to the internet at {url}");
					for w in warnings { eprintln!("warning: {}", w.message); }
				}
				Some(Event::Internet(InternetEvent::State(InternetState::Failed(d)))) => eprintln!("could not open to the internet: {}", d.message),
				Some(Event::Stopped(reason)) => {
					match reason {
						StopReason::Expired => eprintln!("The link expired; stopped."),
						StopReason::FileLimit => eprintln!("File limit reached; stopped."),
						StopReason::Timer => eprintln!("Auto-stop timer reached; stopped."),
						StopReason::Requested => {}
						StopReason::Error(e) => eprintln!("stopped: {e}"),
					}
					break ExitCode::SUCCESS;
				}
				Some(_) => {}
				None => break ExitCode::SUCCESS,
			}
		}
	};
	Ok(code)
}

async fn shutdown(engine: &Engine) -> ExitCode {
	// Stopping removes any router mapping before the process exits.
	engine.shutdown().await;
	ExitCode::SUCCESS
}
