#!/usr/bin/env bash
# Builds the mailslot and mailslot-cli .deb packages with cargo-deb, then lints them.
#
# (c) Copyright 2026 Liminal HQ, Scott Morris
# SPDX-License-Identifier: Apache-2.0 OR MIT

# `version` and `ipc_pkg` come from package-common.sh.
# shellcheck disable=SC2154
set -euo pipefail

# shellcheck source=/dev/null
. "$(dirname "${BASH_SOURCE[0]}")/package-common.sh"

# Runs inside the pkg-deb image (Ubuntu 26.04). The metadata is `[package.metadata.deb]` in each
# crate's Cargo.toml; see docs/packaging.md.

cargo build --release --locked -p mailslot-gtk -p mailslot-cli "${ipc_pkg[@]}"
cargo deb --no-build -p mailslot-gtk -o dist/
cargo deb --no-build -p mailslot-cli -o dist/

ls -l dist/*.deb
failed=0
for deb in dist/*.deb; do
	dpkg-deb --info "$deb" | sed -n '1,25p'
	lintian --fail-on error --tag-display-limit 0 "$deb" || failed=1
done
exit "$failed"
