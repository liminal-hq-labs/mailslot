#!/usr/bin/env bash
# Installs a built package in a clean container of its target distribution and runs it.
#
# (c) Copyright 2026 Liminal HQ, Scott Morris
# SPDX-License-Identifier: Apache-2.0 OR MIT

# Usage: test-package.sh deb|rpm|appimage|cli-tarball
# The containers use no published ports. Packages are read from dist/.

set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."

kind="${1:?usage: $0 deb|rpm|appimage|cli-tarball}"
ubuntu_image="${UBUNTU_IMAGE:-ubuntu:26.04}"
fedora_image="${FEDORA_IMAGE:-registry.fedoraproject.org/fedora:45}"

case "$kind" in
deb)
	docker run --rm -v "$PWD/dist:/dist:ro" "$ubuntu_image" bash -euo pipefail -c '
		apt-get update -qq
		apt-get install -y -qq /dist/mailslot_*.deb
		mailslot --version
		mailslot-cli --version
		if command -v mailslotd >/dev/null; then
			mailslotd --version
			test -f /usr/lib/systemd/user/mailslotd.service
		fi
		apt-get purge -y -qq mailslot
		apt-get install -y -qq /dist/mailslot-cli_*.deb
		mailslot-cli --version
		hash -r
		! command -v mailslot
	'
	;;
rpm)
	docker run --rm -v "$PWD/dist:/dist:ro" "$fedora_image" bash -euo pipefail -c '
		dnf -y -q install /dist/mailslot-[0-9]*.rpm
		mailslot --version
		mailslot-cli --version
		if command -v mailslotd >/dev/null; then
			mailslotd --version
			test -f /usr/lib/systemd/user/mailslotd.service
		fi
		dnf -y -q remove mailslot
		dnf -y -q install /dist/mailslot-cli-*.rpm
		mailslot-cli --version
		hash -r
		! command -v mailslot
	'
	;;
appimage)
	image="${MAILSLOT_IMAGE:-mailslot-pkg-appimage:local}"
	docker run --rm --user "$(id -u):$(id -g)" -e HOME=/tmp -e APPIMAGE_EXTRACT_AND_RUN=1 \
		-v "$PWD/dist:/dist" "$image" bash -euo pipefail -c '
		app="$(ls /dist/Mailslot-*.AppImage | head -n1)"
		chmod +x "$app"
		"$app" mailslot --version
		"$app" mailslot-cli --version
		cd /tmp && "$app" --appimage-extract >/dev/null
		appstreamcli validate --no-net squashfs-root/usr/share/metainfo/ca.liminalhq.mailslot.metainfo.xml
		export XDG_RUNTIME_DIR=/tmp/xdg; mkdir -p "$XDG_RUNTIME_DIR"; chmod 700 "$XDG_RUNTIME_DIR"
		weston --backend=headless --socket=wayland-pkg --width=1024 --height=768 >/tmp/weston.log 2>&1 &
		for _ in $(seq 1 50); do [ -S "$XDG_RUNTIME_DIR/wayland-pkg" ] && break; sleep 0.1; done
		WAYLAND_DISPLAY=wayland-pkg GDK_BACKEND=wayland GSK_RENDERER=cairo timeout 8 dbus-run-session -- "$app" &
		pid=$!
		sleep 5
		kill -0 "$pid" 2>/dev/null || { echo "the AppImage exited early" >&2; exit 1; }
		kill "$pid" || true
	'
	;;
cli-tarball)
	tarball="$(find dist -name "mailslot-cli-*-x86_64-linux-musl.tar.gz" | head -n1)"
	docker run --rm -v "$PWD/dist:/dist:ro" "$ubuntu_image" bash -euo pipefail -c '
		tar -C /tmp -xzf /dist/'"$(basename "$tarball")"'
		/tmp/mailslot-cli-*/mailslot-cli --version
	'
	;;
*)
	echo "usage: $0 deb|rpm|appimage|cli-tarball" >&2
	exit 2
	;;
esac
echo "test-package: $kind ok"
