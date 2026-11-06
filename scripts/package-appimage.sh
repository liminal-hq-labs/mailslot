#!/usr/bin/env bash
# Builds the AppImage with linuxdeploy and its GTK plugin, bundling GTK 4 and libadwaita.
#
# (c) Copyright 2026 Liminal HQ, Scott Morris
# SPDX-License-Identifier: Apache-2.0 OR MIT

# `version` and `ipc_pkg` come from package-common.sh.
# shellcheck disable=SC2154
set -euo pipefail

# shellcheck source=/dev/null
. "$(dirname "${BASH_SOURCE[0]}")/package-common.sh"

# Runs inside the pkg-appimage image, which is built on the oldest distribution that still provides
# GTK 4.20 and libadwaita 1.8, because the build host sets the glibc floor. FUSE is not needed:
# APPIMAGE_EXTRACT_AND_RUN is set in the image. Embeds `gh-releases-zsync` update information.

cargo build --release --locked -p mailslot-gtk -p mailslot-cli "${ipc_pkg[@]}"
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

extra=()
[ -x "$bin/mailslotd" ] && extra+=(--executable "$bin/mailslotd")

PATH="/usr/local/bin:$PATH" linuxdeploy \
	--appdir "$appdir" \
	--plugin gtk \
	--executable "$bin/mailslot" \
	--executable "$bin/mailslot-cli" \
	"${extra[@]}" \
	--desktop-file data/ca.liminalhq.mailslot.desktop \
	--icon-file data/icons/hicolor/scalable/apps/ca.liminalhq.mailslot.svg \
	--icon-filename ca.liminalhq.mailslot \
	--custom-apprun packaging/appimage/AppRun

# The GTK plugin's hook forces GTK_THEME (which would override the system accent colour and dark
# mode that libadwaita follows) and GDK_BACKEND=x11. Remove both before packing.
hook="$appdir/apprun-hooks/linuxdeploy-plugin-gtk.sh"
sed -i '/^export GTK_THEME=/d;/^export GDK_BACKEND=/d' "$hook"
if grep -qE '^export (GTK_THEME|GDK_BACKEND)=' "$hook"; then
	echo "package-appimage: the GTK hook still forces the theme or backend" >&2
	exit 1
fi

PATH="/usr/local/bin:$PATH" linuxdeploy --appdir "$appdir" --output appimage

# appimagetool writes the zsync file next to where it ran.
mv -f ./Mailslot-*.AppImage.zsync dist/

ls -l dist/Mailslot-*.AppImage*
