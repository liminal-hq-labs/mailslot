#!/usr/bin/env bash
# Builds the AppImage with linuxdeploy and its GTK plugin, bundling GTK 4 and libadwaita.
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

# Runs inside the pkg-appimage image, which is built on the oldest distribution that still provides
# GTK 4.20 and libadwaita 1.8, because the build host sets the glibc floor. FUSE is not needed:
# APPIMAGE_EXTRACT_AND_RUN is set in the image. Embeds `gh-releases-zsync` update information.

cargo build --release --locked -p mailslot-gtk -p mailslot-cli
bin="${CARGO_TARGET_DIR:-target}/release"

appdir="$PWD/dist/AppDir"
rm -rf "$appdir"
mkdir -p "$appdir/usr/share/metainfo" "$appdir/usr/share/man/man1"
install -m 0644 data/ca.liminalhq.mailslot.metainfo.xml "$appdir/usr/share/metainfo/"
install -m 0644 data/man/mailslot-cli.1 "$appdir/usr/share/man/man1/"

export DEPLOY_GTK_VERSION=4
export LINUXDEPLOY_OUTPUT_VERSION="$version"
export LDAI_VERSION="$version"
export LDAI_UPDATE_INFORMATION="gh-releases-zsync|liminal-hq|mailslot|latest|Mailslot-*x86_64.AppImage.zsync"
export LDAI_OUTPUT="dist/Mailslot-${version}-x86_64.AppImage"

PATH="/usr/local/bin:$PATH" linuxdeploy \
	--appdir "$appdir" \
	--plugin gtk \
	--executable "$bin/mailslot" \
	--executable "$bin/mailslot-cli" \
	--desktop-file data/ca.liminalhq.mailslot.desktop \
	--icon-file data/icons/hicolor/scalable/apps/ca.liminalhq.mailslot.svg \
	--icon-filename ca.liminalhq.mailslot \
	--custom-apprun packaging/appimage/AppRun \
	--output appimage

ls -l dist/Mailslot-*.AppImage*
