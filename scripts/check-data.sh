#!/usr/bin/env bash
# Validates the desktop file, AppStream metadata, XML, SVG icons, banner, diagrams and completions.
#
# (c) Copyright 2026 Liminal HQ, Scott Morris
# SPDX-License-Identifier: Apache-2.0 OR MIT

set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

fail=0
step() {
	local label="$1"
	shift
	if "$@" >/dev/null 2>"$tmp/err"; then
		echo "check-data: ok: $label"
	else
		echo "check-data: FAIL: $label" >&2
		cat "$tmp/err" >&2
		fail=1
	fi
}

step "desktop file" desktop-file-validate data/ca.liminalhq.mailslot.desktop
step "AppStream metainfo" appstreamcli validate --no-net --explain data/ca.liminalhq.mailslot.metainfo.xml
step "metainfo is well-formed" xmllint --noout data/ca.liminalhq.mailslot.metainfo.xml

while IFS= read -r f; do
	step "xmllint $f" xmllint --noout "$f"
done < <(find assets data build-aux crates -type f \( -name '*.svg' -o -name '*.ui' -o -name '*.xml' \) -not -path '*/target/*' 2>/dev/null | sort)

for icon in data/icons/hicolor/scalable/apps/ca.liminalhq.mailslot.svg data/icons/hicolor/symbolic/apps/ca.liminalhq.mailslot-symbolic.svg; do
	step "renders $icon" rsvg-convert -w 64 "$icon" -o "$tmp/icon.png"
done

step "mailslotd.service names the installed binary" grep -qx 'ExecStart=/usr/bin/mailslotd' data/mailslotd.service
step "banner" bash scripts/check-banner.sh
step "architecture diagrams" bash scripts/check-diagrams.sh --static
step "bash completion syntax" bash -n data/completions/mailslot-cli.bash
if command -v zsh >/dev/null 2>&1; then
	step "zsh completion syntax" zsh -n data/completions/_mailslot-cli
fi

exit "$fail"
