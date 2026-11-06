// mailslot entry point.
//
// (c) Copyright 2026 Liminal HQ, Scott Morris
// SPDX-License-Identifier: Apache-2.0 OR MIT

fn main() -> gtk::glib::ExitCode {
	if std::env::args()
		.skip(1)
		.any(|a| a == "--version" || a == "-V")
	{
		println!("mailslot {}", env!("CARGO_PKG_VERSION"));
		return gtk::glib::ExitCode::SUCCESS;
	}
	// Logs go to the log file; they also go to standard error when MAILSLOT_LOG is set.
	let _log = mailslot_log::init(mailslot_log::Options {
		stderr: mailslot_log::filter_is_set(),
	});
	tracing::info!(version = env!("CARGO_PKG_VERSION"), "Mailslot starting");
	mailslot_gtk::i18n::init();
	mailslot_gtk::run()
}
