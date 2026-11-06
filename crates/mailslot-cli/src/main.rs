// mailslot-cli entry point and command-line definition.
//
// (c) Copyright 2026 Liminal HQ, Scott Morris
// SPDX-License-Identifier: Apache-2.0 OR MIT

mod misc;
mod send;
mod serve;
mod watch;

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, CommandFactory, Parser, Subcommand};

/// Serve a private file drop, or send files to one.
#[derive(Parser, Debug)]
#[command(name = "mailslot-cli", version, about, long_about = None, propagate_version = true)]
pub struct Cli {
	#[command(subcommand)]
	pub command: Command,
}

/// Options shared by commands that talk to a link.
#[derive(Args, Debug, Clone)]
pub struct ClientArgs {
	/// Files uploaded at once.
	#[arg(long, default_value_t = 2)]
	pub parallel: usize,
	/// Retries after the first attempt.
	#[arg(long, default_value_t = 5)]
	pub retries: u32,
	/// Cap total bandwidth, for example `500k` or `2m` (bytes per second).
	#[arg(long, value_parser = parse_rate)]
	pub limit: Option<u64>,
	/// Send even if the server already has the content.
	#[arg(long)]
	pub no_dedupe: bool,
	/// Label for the per-sender subfolder scheme.
	#[arg(long)]
	pub label: Option<String>,
	/// Accept any certificate on an https link that has no key pin.
	#[arg(long)]
	pub insecure: bool,
	/// Print one JSON object per line instead of human-readable progress.
	#[arg(long)]
	pub json: bool,
}

#[derive(Subcommand, Debug)]
pub enum Command {
	/// Run a share with no window. Settings come from the config file; flags override them.
	Serve(ServeArgs),
	/// Send files (and standard input with --stdin) to a link: `send FILE... LINK`.
	Send(SendArgs),
	/// Watch a folder and push new and changed files to a link: `watch DIR LINK`.
	Watch(WatchArgs),
	/// Show this machine's link with a QR code, or check a link you were given.
	Pair(PairArgs),
	/// Ask a link's server for its limits and whether it is accepting files.
	Status(StatusArgs),
	/// Print a shell completion script.
	Completions {
		/// Shell to generate for.
		shell: clap_complete::Shell,
	},
	/// Print the man page.
	Man,
}

#[derive(Args, Debug)]
pub struct ServeArgs {
	/// Destination folder.
	#[arg(long)]
	pub dir: Option<PathBuf>,
	/// Address to bind, for example `0.0.0.0` or `127.0.0.1`.
	#[arg(long)]
	pub bind: Option<String>,
	/// Port (0 picks a free one).
	#[arg(long)]
	pub port: Option<u16>,
	/// Largest file in megabytes.
	#[arg(long)]
	pub max_mb: Option<u64>,
	/// Minutes until the link expires (0 for never).
	#[arg(long)]
	pub expiry_minutes: Option<u32>,
	/// Stop after this many files (1 is one-shot mode).
	#[arg(long)]
	pub stop_after: Option<u32>,
	/// Name this link; senders using it are filed under the name with `--subfolders sender`.
	#[arg(long)]
	pub label: Option<String>,
	/// How to group received files: `none`, `date` or `sender`.
	#[arg(long, value_parser = ["none", "date", "sender"])]
	pub subfolders: Option<String>,
	/// Announce this share on the local network with mDNS (never includes the secret).
	#[arg(long)]
	pub mdns: bool,
	/// Serve HTTPS with a self-signed certificate and print its key pin.
	#[arg(long)]
	pub https: bool,
	/// Open the port on the router with UPnP while sharing.
	#[arg(long)]
	pub upnp: bool,
	/// Create a new token before starting.
	#[arg(long)]
	pub new_token: bool,
	/// Keep settings, token and history under this folder instead of the XDG locations.
	#[arg(long)]
	pub home: Option<PathBuf>,
	/// Do not read or write any settings, token or history files.
	#[arg(long)]
	pub ephemeral: bool,
	/// Print the first line as JSON for scripts.
	#[arg(long)]
	pub json: bool,
}

#[derive(Args, Debug)]
pub struct SendArgs {
	/// Files, then the link as the last argument.
	#[arg(required = true, num_args = 1.., value_name = "FILE... LINK")]
	pub args: Vec<String>,
	/// Also send standard input, as a file called `--name`.
	#[arg(long)]
	pub stdin: bool,
	/// Name for data read from standard input.
	#[arg(long, default_value = "stdin.bin")]
	pub name: String,
	#[command(flatten)]
	pub client: ClientArgs,
}

#[derive(Args, Debug)]
pub struct WatchArgs {
	/// Folder to watch.
	pub dir: PathBuf,
	/// Link to push to.
	pub link: String,
	/// Seconds to wait before retrying a file that failed to send.
	#[arg(long, default_value_t = 5)]
	pub interval: u64,
	/// Do not send files that already exist; push only new and changed ones. Applies when
	/// there is no saved queue yet.
	#[arg(long)]
	pub skip_existing: bool,
	/// Quiet time in milliseconds before a changed file is considered finished.
	#[arg(long, default_value_t = 800)]
	pub debounce_ms: u64,
	/// Where to keep the persisted send queue (default: the data folder).
	#[arg(long)]
	pub state: Option<PathBuf>,
	/// Keep settings and data under this folder instead of the XDG locations.
	#[arg(long)]
	pub home: Option<PathBuf>,
	#[command(flatten)]
	pub client: ClientArgs,
}

#[derive(Args, Debug)]
pub struct PairArgs {
	/// A link to check instead of showing this machine's.
	pub link: Option<String>,
	/// Keep settings under this folder instead of the XDG locations.
	#[arg(long)]
	pub home: Option<PathBuf>,
	/// Look for shares announcing themselves on the local network instead. Announcements
	/// never include the secret, so you still need the link from its owner.
	#[arg(long)]
	pub discover: bool,
	/// Seconds to listen when discovering.
	#[arg(long, default_value_t = 3)]
	pub timeout: u64,
	/// Print JSON instead of text and a QR code.
	#[arg(long)]
	pub json: bool,
}

#[derive(Args, Debug)]
pub struct StatusArgs {
	/// The link to ask.
	pub link: String,
	/// Accept any certificate on an https link that has no key pin.
	#[arg(long)]
	pub insecure: bool,
	/// Print JSON.
	#[arg(long)]
	pub json: bool,
}

fn parse_rate(s: &str) -> Result<u64, String> {
	let s = s.trim().to_ascii_lowercase();
	let (num, mult) = match s.chars().last() {
		Some('k') => (&s[..s.len() - 1], 1024u64),
		Some('m') => (&s[..s.len() - 1], 1024 * 1024),
		Some('g') => (&s[..s.len() - 1], 1024 * 1024 * 1024),
		_ => (s.as_str(), 1),
	};
	num.parse::<f64>()
		.map(|n| (n * mult as f64) as u64)
		.map_err(|_| format!("`{s}` is not a rate such as 500k or 2m"))
}

#[tokio::main]
async fn main() -> ExitCode {
	let cli = Cli::parse();
	// Logs go to the log file and standard error; completions and the man page print only
	// their own output.
	let _log = (!matches!(cli.command, Command::Completions { .. } | Command::Man))
		.then(|| mailslot_log::init(mailslot_log::Options { stderr: true }));
	let result = match cli.command {
		Command::Serve(a) => serve::run(a).await,
		Command::Send(a) => send::run(a).await,
		Command::Watch(a) => watch::run(a).await,
		Command::Pair(a) => misc::pair(a).await,
		Command::Status(a) => misc::status(a).await,
		Command::Completions { shell } => {
			clap_complete::generate(
				shell,
				&mut Cli::command(),
				"mailslot-cli",
				&mut std::io::stdout(),
			);
			Ok(ExitCode::SUCCESS)
		}
		Command::Man => misc::man(),
	};
	match result {
		Ok(code) => code,
		Err(e) => {
			eprintln!("mailslot-cli: {e:#}");
			ExitCode::from(1)
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn rates_parse() {
		assert_eq!(parse_rate("500k").unwrap(), 512_000);
		assert_eq!(parse_rate("2m").unwrap(), 2 * 1024 * 1024);
		assert_eq!(parse_rate("1000").unwrap(), 1000);
		assert!(parse_rate("fast").is_err());
	}

	#[test]
	fn cli_definition_is_valid() {
		Cli::command().debug_assert();
	}
}
