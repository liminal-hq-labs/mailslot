#!/usr/bin/env bash
# Validates the architecture diagrams: self-contained, small, accessible, motion-aware, text fits.
#
# (c) Copyright 2026 Liminal HQ, Scott Morris
# SPDX-License-Identifier: Apache-2.0 OR MIT

set -euo pipefail

# Usage: check-diagrams.sh [--static | --render] [FILE.svg...]
#
# The static pass runs xmllint and greps every docs/diagrams/*.svg for forbidden constructs, the
# size limit, the licence header, <title>, <desc>, role="img", viewBox, data-loop and the
# prefers-color-scheme and prefers-reduced-motion rules. The render pass loads each diagram in
# headless Chromium (scripts/diagram-fit.py) and checks text fit through the whole loop. Without a
# browser the render pass is skipped with a notice, unless DIAGRAMS_REQUIRE_RENDER=1.

cd "$(dirname "${BASH_SOURCE[0]}")/.."

mode=all
files=()
for arg in "$@"; do
	case "$arg" in
	--static) mode=static ;;
	--render) mode=render ;;
	*) files+=("$arg") ;;
	esac
done
if [ "${#files[@]}" -eq 0 ]; then
	shopt -s nullglob
	files=(docs/diagrams/*.svg)
	shopt -u nullglob
fi
if [ "${#files[@]}" -eq 0 ]; then
	echo "check-diagrams: no diagrams found" >&2
	exit 1
fi

max_bytes=61440
fail=0

err() {
	echo "check-diagrams: FAIL: $*" >&2
	fail=1
}

static_checks() {
	local f="$1" size
	xmllint --noout "$f" || err "$f is not well-formed XML"
	if grep -nE '<script|foreignObject|<iframe|<image|<use[^>]+href="[^#]|href="https?:|@import|@font-face|url\(\s*["'"'"']?(https?:|data:|//)' "$f"; then
		err "$f has a forbidden construct (script, foreignObject, image, external reference, @import or web font)"
	fi
	if grep -nE '\son[a-z]+="' "$f"; then
		err "$f has an event handler attribute"
	fi
	size="$(wc -c <"$f")"
	[ "$size" -le "$max_bytes" ] || err "$f is ${size} bytes, over ${max_bytes}"
	head -n 10 "$f" | grep -q 'SPDX-License-Identifier: Apache-2.0 OR MIT' || err "$f is missing the SPDX licence line"
	head -n 10 "$f" | grep -q '(c) Copyright 2026 Liminal HQ, Scott Morris' || err "$f is missing the copyright line"
	grep -q '<title' "$f" || err "$f is missing <title>"
	grep -q '<desc' "$f" || err "$f is missing <desc>"
	grep -q 'role="img"' "$f" || err "$f is missing role=\"img\""
	grep -qE '<svg[^>]+viewBox="' "$f" || err "$f is missing a viewBox"
	grep -qE '<svg[^>]+data-loop="[0-9.]+"' "$f" || err "$f is missing data-loop on the root element"
	grep -q 'prefers-reduced-motion: reduce' "$f" || err "$f does not handle prefers-reduced-motion"
	grep -q 'prefers-color-scheme: dark' "$f" || err "$f does not handle prefers-color-scheme"
	if grep -nE '^ +[^ ]' "$f"; then
		err "$f indents with spaces (house style is tabs)"
	fi
	# Every font stack ends in a generic family, so a missing font never falls back to serif.
	if grep -noE 'font-family[:=] *"?[^;"}]*' "$f" | grep -vE '(sans-serif|monospace) *$'; then
		err "$f has a font stack that does not end in sans-serif or monospace"
	fi
}

if [ "$mode" != render ]; then
	for f in "${files[@]}"; do
		before="$fail"
		static_checks "$f"
		[ "$fail" != "$before" ] || echo "check-diagrams: ok: $f ($(wc -c <"$f") bytes)"
	done
fi

if [ "$mode" != static ]; then
	browser_found=0
	for b in "${CHROMIUM:-}" chromium chromium-browser google-chrome google-chrome-stable chrome; do
		if [ -n "$b" ] && command -v "$b" >/dev/null 2>&1; then
			browser_found=1
			break
		fi
	done
	if [ "$browser_found" -eq 1 ] && command -v python3 >/dev/null 2>&1; then
		python3 scripts/diagram-fit.py "${files[@]}" || fail=1
	elif [ "${DIAGRAMS_REQUIRE_RENDER:-0}" = 1 ]; then
		err "the text-fit pass needs python3 and Chromium or Chrome (set CHROMIUM)"
	else
		echo "check-diagrams: skipped the text-fit pass: no python3 and Chromium here (make data-check runs it on the host, CI in the diagrams job)"
	fi
fi

exit "$fail"
