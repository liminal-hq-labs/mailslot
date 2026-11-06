#!/usr/bin/env bash
# Fails on colour literals outside style.css, and on non-semantic colours inside it.
#
# (c) Copyright 2026 Liminal HQ, Scott Morris
# SPDX-License-Identifier: Apache-2.0 OR MIT

set -euo pipefail

# The GTK app takes its colours from the system through libadwaita named colours (accent colour
# included). Brand colours belong to the icon, banner, About artwork and metainfo only.

cd "$(git rev-parse --show-toplevel 2>/dev/null || dirname "$(dirname "${BASH_SOURCE[0]}")")"

dir="${1:-crates/mailslot-gtk}"
if [ ! -d "$dir" ]; then
	echo "check-style: $dir does not exist yet, nothing to check"
	exit 0
fi

fail=0
css="$dir/resources/style.css"

# A colour literal: #rgb, #rrggbb, #rrggbbaa, rgb(), rgba(), hsl(), hsla(), or a GdkRGBA constructor.
literal='(["'"'"'(,: =]|^)#[0-9a-fA-F]{3,8}\b|\brgba?\(|\bhsla?\(|RGBA::(new|parse)|gdk::RGBA'

# Rust, UI definitions and any other stylesheet: no literals at all.
while IFS= read -r f; do
	[ "$f" = "$css" ] && continue
	if grep -nE "$literal" "$f" >/dev/null; then
		grep -nHE "$literal" "$f" | sed 's/^/check-style: literal colour: /' >&2
		fail=1
	fi
done < <(find "$dir" -type f \( -name '*.rs' -o -name '*.ui' -o -name '*.css' -o -name '*.blp' \) -not -path '*/target/*')

# style.css: named colours only, from the semantic libadwaita set.
if [ -f "$css" ]; then
	if grep -nE "$literal" "$css" >/dev/null; then
		grep -nHE "$literal" "$css" | sed 's/^/check-style: literal colour: /' >&2
		fail=1
	fi
	allowed='^(accent|destructive|success|warning|error|window|view|headerbar|card|dialog|popover|sidebar|secondary_sidebar|thumbnail)_(color|bg_color|fg_color|border_color|backdrop_color|shade_color)$|^(shade_color|scrollbar_outline_color)$'
	while IFS= read -r name; do
		if ! [[ "$name" =~ $allowed ]]; then
			echo "check-style: $css uses @$name, which is not a semantic libadwaita named colour" >&2
			fail=1
		fi
	done < <(grep -oE '@[a-z_0-9]+' "$css" | sed 's/^@//' | grep -vE '^(define-color|import|keyframes|media|charset|namespace)$' | sort -u)
fi

if [ "$fail" -eq 0 ]; then
	echo "check-style: ok"
fi
exit "$fail"
