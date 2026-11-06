// pair, status and documentation generators.
//
// (c) Copyright 2026 Liminal HQ, Scott Morris
// SPDX-License-Identifier: Apache-2.0 OR MIT

use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::CommandFactory;
use mailslot_core::client::{Client, ClientOptions};
use mailslot_core::config::{Config, Paths};
use mailslot_core::proto::MailslotLink;

use crate::{Cli, PairArgs, StatusArgs};

/// Renders text as a QR code using half-block characters.
fn qr_text(data: &str) -> Option<String> {
	use qrcode::{Color, QrCode};
	let code = QrCode::new(data.as_bytes()).ok()?;
	let w = code.width();
	let quiet = 2usize;
	let at = |x: i64, y: i64| -> bool {
		if x < 0 || y < 0 || x >= w as i64 || y >= w as i64 {
			return false;
		}
		code[(x as usize, y as usize)] == Color::Dark
	};
	let mut out = String::new();
	let total = w as i64 + quiet as i64 * 2;
	let mut y = 0;
	while y < total {
		for x in 0..total {
			let (px, py) = (x - quiet as i64, y - quiet as i64);
			let (top, bottom) = (at(px, py), at(px, py + 1));
			// Light modules are drawn as filled blocks so the code scans on dark terminals.
			out.push(match (top, bottom) {
				(false, false) => '█',
				(true, false) => '▄',
				(false, true) => '▀',
				(true, true) => ' ',
			});
		}
		out.push('\n');
		y += 2;
	}
	Some(out)
}

pub async fn pair(a: PairArgs) -> Result<ExitCode> {
	mailslot_core::tls::ensure_default_provider();
	if let Some(text) = &a.link {
		let link = MailslotLink::parse(text).context("the link is not valid")?;
		let client = Client::new(
			link.clone(),
			ClientOptions {
				insecure: false,
				..Default::default()
			},
		)?;
		let health = client.health().await.context("could not reach that link")?;
		if a.json {
			println!(
				"{}",
				serde_json::json!({ "reachable": true, "host": link.host, "port": link.port, "tls": link.tls, "max_bytes": health.max_bytes, "accepting": health.accepting })
			);
		} else {
			println!(
				"Reachable: {}:{}{}",
				link.host,
				link.port,
				if link.tls { " (HTTPS, pinned)" } else { "" }
			);
			println!(
				"Accepting files: {}",
				if health.accepting { "yes" } else { "no" }
			);
			println!(
				"Largest file: {}",
				crate::send::human_bytes(health.max_bytes as f64)
			);
		}
		return Ok(ExitCode::SUCCESS);
	}
	let paths = match &a.home {
		Some(h) => Paths::in_dir(h),
		None => Paths::xdg(),
	};
	let config = Config::load(&paths).await?;
	let token = mailslot_core::config::load_or_create_token(&paths).await?;
	let host = mailslot_core::netinfo::lan_addrs()
		.await
		.into_iter()
		.next()
		.map(|a| a.ip.to_string())
		.unwrap_or_else(|| "127.0.0.1".into());
	let mut link = MailslotLink::new(host, config.sharing.port, token);
	if config.security.https {
		let pin = mailslot_core::tls::load_or_generate(&paths.tls_dir(), &[link.host.clone()])
			.await?
			.pin;
		link = link.with_tls(Some(pin));
	}
	if a.json {
		println!(
			"{}",
			serde_json::json!({ "link": link.to_uri(), "url": link.drop_url() })
		);
	} else {
		println!("{}", link.to_uri());
		println!("{}", link.drop_url());
		if let Some(qr) = qr_text(&link.to_uri()) {
			println!("\n{qr}");
		}
		eprintln!("The share must be running (`mailslot-cli serve`) for the link to work.");
	}
	Ok(ExitCode::SUCCESS)
}

pub async fn status(a: StatusArgs) -> Result<ExitCode> {
	mailslot_core::tls::ensure_default_provider();
	let link = MailslotLink::parse(&a.link).context("the link is not valid")?;
	let client = Client::new(
		link,
		ClientOptions {
			insecure: a.insecure,
			retries: 0,
			..Default::default()
		},
	)?;
	match client.health().await {
		Ok(h) => {
			if a.json {
				println!(
					"{}",
					serde_json::json!({ "ok": h.ok, "accepting": h.accepting, "max_bytes": h.max_bytes, "expires_at": h.expires_at, "capabilities": h.capabilities })
				);
			} else {
				println!("accepting: {}", if h.accepting { "yes" } else { "no" });
				println!(
					"largest file: {}",
					crate::send::human_bytes(h.max_bytes as f64)
				);
				println!("expires: {}", h.expires_at.as_deref().unwrap_or("never"));
				println!("features: {}", h.capabilities.join(", "));
			}
			Ok(if h.accepting {
				ExitCode::SUCCESS
			} else {
				ExitCode::from(2)
			})
		}
		Err(e) => {
			if a.json {
				println!(
					"{}",
					serde_json::json!({ "ok": false, "error": e.to_string() })
				);
			} else {
				eprintln!("unreachable: {e}");
			}
			Ok(ExitCode::from(1))
		}
	}
}

pub fn man() -> Result<ExitCode> {
	let cmd = Cli::command();
	let mut buf = Vec::new();
	clap_mangen::Man::new(cmd.clone()).render(&mut buf)?;
	for sub in cmd.get_subcommands() {
		let name = format!("mailslot-cli-{}", sub.get_name());
		let sub = sub.clone().name(&*Box::leak(name.into_boxed_str()));
		clap_mangen::Man::new(sub).render(&mut buf)?;
	}
	use std::io::Write;
	std::io::stdout().write_all(&buf)?;
	Ok(ExitCode::SUCCESS)
}
