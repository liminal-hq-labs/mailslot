#!/usr/bin/env bash
# Builds the mailslot and mailslot-cli .rpm packages with cargo-generate-rpm, then lints them.
#
# (c) Copyright 2026 Liminal HQ, Scott Morris
# SPDX-License-Identifier: Apache-2.0 OR MIT

set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

version="$(sed -n "s/^version *= *\"\\(.*\\)\"/\\1/p" Cargo.toml | head -n1)"
[ -n "$version" ] || {
	echo "could not read the workspace version from Cargo.toml" >&2
	exit 1
}
mkdir -p dist

# Runs inside the pkg-rpm image (Fedora). The metadata is `[package.metadata.generate-rpm]` in each
# crate's Cargo.toml; see docs/packaging.md.

cargo build --release --locked -p mailslot-gtk -p mailslot-cli
cargo generate-rpm -p crates/mailslot-gtk -o dist/
cargo generate-rpm -p crates/mailslot-cli -o dist/

ls -l dist/*.rpm
for rpm in dist/*.rpm; do
	rpm -qpi "$rpm" | sed -n '1,20p'
	rpmlint "$rpm" || [ "${RPMLINT_STRICT:-0}" != 1 ]
done
