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
	mailslot_gtk::i18n::init();
	mailslot_gtk::run()
}
