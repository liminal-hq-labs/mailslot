#!/usr/bin/env bash
# Runs the Mailslot GUI from the dev image on the host's Wayland or X11 session.
#
# (c) Copyright 2026 Liminal HQ, Scott Morris
# SPDX-License-Identifier: Apache-2.0 OR MIT

set -euo pipefail

# The container uses the host network so the dev server is reachable from a phone on the LAN and
# LAN address detection sees the host's interfaces. That exposes the dev server on the LAN, which
# is the point. Pass ISOLATED=1 for a bridge network with no published ports (UI-only work).
#
# GDK_DEBUG=no-portals stops GTK asking the host's file chooser portal, which would return paths
# that do not exist inside the container. Test portal flows natively or in the Flatpak.
#
# Usage: docker/run-gui.sh [wayland|x11] [command...]

mode="${1:-wayland}"
shift || true
image="${MAILSLOT_IMAGE:-mailslot-dev:local}"

if [ "$#" -eq 0 ]; then
	set -- cargo run -p mailslot-gtk
fi

args=(
	--rm -it
	--user "$(id -u):$(id -g)"
	-v "$PWD:/workspace"
	-v mailslot-cargo-registry:/home/dev/.cargo/registry
	-v mailslot-target:/target
	-e CARGO_TARGET_DIR=/target
	-e GDK_DEBUG=no-portals
	-e XDG_RUNTIME_DIR=/tmp/xdg
	--device /dev/dri
)

mkdir -p .dev-uploads
args+=(-v "$PWD/.dev-uploads:/home/dev/Downloads/Mailslot")

if [ "${ISOLATED:-0}" = 1 ]; then
	args+=(--network bridge)
else
	args+=(--network host)
fi

case "$mode" in
wayland)
	: "${WAYLAND_DISPLAY:?WAYLAND_DISPLAY is not set; is a Wayland session running?}"
	: "${XDG_RUNTIME_DIR:?XDG_RUNTIME_DIR is not set}"
	args+=(
		-e WAYLAND_DISPLAY="$WAYLAND_DISPLAY"
		-e GDK_BACKEND=wayland
		-v "$XDG_RUNTIME_DIR/$WAYLAND_DISPLAY:/tmp/xdg/$WAYLAND_DISPLAY:ro"
	)
	if [ -S "$XDG_RUNTIME_DIR/bus" ]; then
		args+=(
			-e DBUS_SESSION_BUS_ADDRESS=unix:path=/tmp/xdg/bus
			-v "$XDG_RUNTIME_DIR/bus:/tmp/xdg/bus"
		)
	fi
	;;
x11)
	: "${DISPLAY:?DISPLAY is not set}"
	# Needs `xhost +SI:localuser:$USER` once on the host.
	args+=(
		-e DISPLAY="$DISPLAY"
		-e GDK_BACKEND=x11
		-v /tmp/.X11-unix:/tmp/.X11-unix:ro
	)
	;;
*)
	echo "usage: $0 [wayland|x11] [command...]" >&2
	exit 2
	;;
esac

# If GPU rendering misbehaves, set GSK_RENDERER=cairo.
[ -n "${GSK_RENDERER:-}" ] && args+=(-e GSK_RENDERER="$GSK_RENDERER")

exec docker run "${args[@]}" "$image" "$@"
