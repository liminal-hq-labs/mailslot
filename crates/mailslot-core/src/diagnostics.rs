// A plain-text diagnostics report for support requests.
//
// (c) Copyright 2026 Liminal HQ, Scott Morris
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Builds the text the "Copy diagnostics" action puts on the clipboard: what Mailslot knows
//! about the router and the chosen address, the latest error and the tail of the log.
//!
//! The report is passed through [`redact`](crate::redact::redact) as a whole, so the token
//! cannot appear in it whatever the inputs hold.

use crate::config::Config;
use crate::events::{EngineState, InternetState};
use crate::redact::redact;

/// Builds the report. `log_tail` holds the newest log lines, oldest first.
pub fn report(
	state: &EngineState,
	config: &Config,
	selected_host: Option<&str>,
	version: &str,
	log_tail: &[String],
) -> String {
	let mut s = String::new();
	let mut line = |k: &str, v: String| s.push_str(&format!("{k}: {v}\n"));
	line("Mailslot", version.to_owned());
	line("Status", format!("{:?}", state.status));
	line(
		"Listening on",
		state
			.addr
			.map_or_else(|| "not sharing".into(), |a| a.to_string()),
	);
	line(
		"Selected address",
		selected_host.unwrap_or("none").to_owned(),
	);
	let preferred = config.network.interface.trim();
	if !preferred.is_empty() {
		line("Preferred interface", preferred.to_owned());
	}
	line("UPnP requested", config.network.upnp.to_string());
	let internet = match &state.internet {
		InternetState::Off => "off".to_owned(),
		InternetState::Mapping => "asking the router".to_owned(),
		InternetState::Open { external, .. } => format!("open at {external}"),
		InternetState::Failed(d) => format!("failed: {}", d.message),
	};
	line("Internet", internet);
	if let Some(d) = &state.internet_details {
		let show = |o: Option<String>| o.unwrap_or_else(|| "unknown".into());
		line("Router", show(d.router.clone()));
		line("Gateway", show(d.gateway.map(|g| g.to_string())));
		line(
			"Internal client",
			show(d.internal_client.map(|g| g.to_string())),
		);
		line("Interface", show(d.interface.clone()));
		if let Some(c) = d.error_code {
			line(
				"Router error",
				format!("{c} {}", d.error_text.as_deref().unwrap_or("")),
			);
		} else if let Some(t) = &d.error_text {
			line("Router error", t.clone());
		}
	}
	s.push_str(&format!("\nLog (last {} lines)\n", log_tail.len()));
	for l in log_tail {
		s.push_str(l);
		s.push('\n');
	}
	redact(&s)
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::events::{Diagnostic, DiagnosticKind, InternetDetails};

	#[test]
	fn report_names_the_facts_and_never_the_token() {
		let token = "AbCdEfGhIjKlMnOpQrStUvWxYz0123456789-_AbCd";
		let state = EngineState {
			internet: InternetState::Failed(Diagnostic {
				kind: DiagnosticKind::MappingRefused,
				message: "UPnP error 606: Action not authorized".into(),
				details: None,
			}),
			internet_details: Some(InternetDetails {
				gateway: Some("192.168.1.1".parse().unwrap()),
				router: Some("Ubiquiti UDM-SE".into()),
				internal_client: Some("192.168.1.100".parse().unwrap()),
				interface: Some("enp16s0u4".into()),
				error_code: Some(606),
				error_text: Some("Action not authorized".into()),
			}),
			..Default::default()
		};
		let tail = vec![format!("INFO request GET /{token}/x status=404")];
		let r = report(
			&state,
			&Config::default(),
			Some("192.168.1.100"),
			"0.1.0",
			&tail,
		);
		assert!(!r.contains(token), "{r}");
		for needle in [
			"UDM-SE",
			"192.168.1.1",
			"enp16s0u4",
			"606 Action not authorized",
			"0.1.0",
		] {
			assert!(r.contains(needle), "{needle} missing in {r}");
		}
	}
}
