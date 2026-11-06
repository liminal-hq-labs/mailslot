#!/usr/bin/env bash
# Builds the mailslot and mailslot-cli .rpm packages with cargo-generate-rpm, then lints them.
#
# (c) Copyright 2026 Liminal HQ, Scott Morris
# SPDX-License-Identifier: Apache-2.0 OR MIT

# `version` and `ipc_pkg` come from package-common.sh.
# shellcheck disable=SC2154
set -euo pipefail

# shellcheck source=/dev/null
. "$(dirname "${BASH_SOURCE[0]}")/package-common.sh"

# Runs inside the pkg-rpm image (Fedora). The metadata is `[package.metadata.generate-rpm]` in each
# crate's Cargo.toml; see docs/packaging.md.

cargo build --release --locked -p mailslot-gtk -p mailslot-cli "${ipc_pkg[@]}"
cargo generate-rpm -p crates/mailslot-gtk -o dist/
cargo generate-rpm -p crates/mailslot-cli -o dist/

ls -l dist/*.rpm
for rpm in dist/*.rpm; do
	rpm -qpi "$rpm" | sed -n '1,20p'
	rpmlint "$rpm" || [ "${RPMLINT_STRICT:-0}" != 1 ]
done
