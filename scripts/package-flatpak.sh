#!/usr/bin/env bash
# Builds the Flatpak inside the flatpak-github-actions image (run with --privileged).
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

# The cargo sources are generated from Cargo.lock so the build itself runs offline. Output goes to
# build-aux/flatpak/repo, with a single-file bundle in dist/.

generator_ref=41c20aa10819cdb2a4f3ca171758a96d1955c018
python3 -m pip install --quiet --break-system-packages aiohttp tomlkit PyYAML 2>/dev/null || true
curl --proto '=https' --tlsv1.2 -fsSL -o /tmp/flatpak-cargo-generator.py \
	"https://raw.githubusercontent.com/flatpak/flatpak-builder-tools/${generator_ref}/cargo/flatpak-cargo-generator.py"
python3 /tmp/flatpak-cargo-generator.py Cargo.lock -o build-aux/flatpak/cargo-sources.json

flatpak-builder --force-clean --disable-rofiles-fuse \
	--install-deps-from=flathub --user \
	--repo=build-aux/flatpak/repo \
	build-aux/flatpak/build build-aux/flatpak/ca.liminalhq.mailslot.json

flatpak build-bundle build-aux/flatpak/repo "dist/mailslot-${version}.flatpak" ca.liminalhq.mailslot
echo "package-flatpak: dist/mailslot-${version}.flatpak"
