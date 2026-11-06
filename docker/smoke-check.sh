#!/usr/bin/env bash
# Checks that the Mailslot dev and CI image has every tool the project needs.
#
# (c) Copyright 2026 Liminal HQ, Scott Morris
# SPDX-License-Identifier: Apache-2.0 OR MIT

set -euo pipefail

want_rust="${RUST_TOOLCHAIN:-1.98.0}"
fail=0

check() {
	local label="$1"
	shift
	local out
	if out="$("$@" 2>&1 | head -n1)"; then
		printf '%-18s %s\n' "$label" "$out"
	else
		printf '%-18s MISSING\n' "$label" >&2
		fail=1
	fi
}

pkg_atleast() {
	if pkg-config --atleast-version="$2" "$1"; then
		printf '%-18s %s (>= %s)\n' "$1" "$(pkg-config --modversion "$1")" "$2"
	else
		printf '%-18s too old or missing (need >= %s)\n' "$1" "$2" >&2
		fail=1
	fi
}

pkg_atleast gtk4 4.20
pkg_atleast libadwaita-1 1.8

rust_now="$(rustc --version | awk '{print $2}')"
if [ "$rust_now" = "$want_rust" ]; then
	printf '%-18s %s\n' rustc "$rust_now"
else
	printf '%-18s %s (expected %s)\n' rustc "$rust_now" "$want_rust" >&2
	fail=1
fi

check cargo cargo --version
check clippy cargo clippy --version
check rustfmt rustfmt --version
check nextest cargo nextest --version
check cargo-deny cargo deny --version
check weston weston --version
check xvfb-run sh -c 'command -v xvfb-run'
check appstreamcli appstreamcli --version
check blueprint blueprint-compiler --version
check desktop-validate sh -c 'command -v desktop-file-validate'
check xmllint sh -c 'command -v xmllint'
check rsvg-convert rsvg-convert --version
check shellcheck shellcheck --version
check upnpc sh -c 'command -v upnpc'

exit "$fail"
