#!/usr/bin/env bash
# Runs the display-needing GTK tests under a headless Wayland compositor (or Xvfb).
#
# (c) Copyright 2026 Liminal HQ, Scott Morris
# SPDX-License-Identifier: Apache-2.0 OR MIT

set -euo pipefail

# UI_BACKEND=x11 uses xvfb-run and GDK_BACKEND=x11 instead of weston. The tests are exactly the
# `#[ignore = "needs display"]` ones, so `--run-ignored only` selects them.

cd "$(dirname "${BASH_SOURCE[0]}")/.."

run_tests() {
	cargo nextest run -p mailslot-gtk --run-ignored only "$@"
}

if [ "${UI_BACKEND:-wayland}" = x11 ]; then
	export GDK_BACKEND=x11
	exec dbus-run-session -- xvfb-run -a -s '-screen 0 1280x1024x24' \
		cargo nextest run -p mailslot-gtk --run-ignored only "$@"
fi

export XDG_RUNTIME_DIR="${XDG_RUNTIME_DIR:-$(mktemp -d)}"
chmod 0700 "$XDG_RUNTIME_DIR"
socket="wayland-test-$$"

weston --backend=headless --socket="$socket" --width=1280 --height=1024 >"${XDG_RUNTIME_DIR}/weston.log" 2>&1 &
weston_pid=$!
trap 'kill "$weston_pid" 2>/dev/null || true' EXIT

for _ in $(seq 1 50); do
	[ -S "$XDG_RUNTIME_DIR/$socket" ] && break
	sleep 0.1
done
if [ ! -S "$XDG_RUNTIME_DIR/$socket" ]; then
	echo "test-ui: weston did not start" >&2
	cat "${XDG_RUNTIME_DIR}/weston.log" >&2 || true
	exit 1
fi

export WAYLAND_DISPLAY="$socket" GDK_BACKEND=wayland GSK_RENDERER=cairo
export -f run_tests
dbus-run-session -- bash -c 'run_tests "$@"' _ "$@"
