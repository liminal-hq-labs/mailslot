#!/usr/bin/env bash
# Shared setup for the package-*.sh scripts: the workspace version, dist/, and the optional daemon.
#
# (c) Copyright 2026 Liminal HQ, Scott Morris
# SPDX-License-Identifier: Apache-2.0 OR MIT

# shellcheck disable=SC2034,SC2164
# Sourced, not run. Sets `version`, creates `dist/`, and sets `ipc_pkg` to `(-p mailslot-ipc)` when
# the workspace has the daemon crate, so every package ships `mailslotd` once it exists.

cd "$(dirname "${BASH_SOURCE[0]}")/.."

version="$(sed -n 's/^version *= *"\(.*\)"/\1/p' Cargo.toml | head -n1)"
[ -n "$version" ] || {
	echo "could not read the workspace version from Cargo.toml" >&2
	exit 1
}
mkdir -p dist

ipc_pkg=()
if cargo metadata --no-deps --format-version 1 2>/dev/null | grep -q '"name":"mailslot-ipc"'; then
	ipc_pkg=(-p mailslot-ipc)
fi
