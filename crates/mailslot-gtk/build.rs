// Compiles the UI definitions and stylesheet into a gresource bundle.
//
// (c) Copyright 2026 Liminal HQ, Scott Morris
// SPDX-License-Identifier: Apache-2.0 OR MIT

fn main() {
	glib_build_tools::compile_resources(
		&["resources"],
		"resources/mailslot.gresource.xml",
		"mailslot.gresource",
	);
}
