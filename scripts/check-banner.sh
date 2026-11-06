#!/usr/bin/env bash
# Validates the hero banner: well-formed, self-contained, small and motion-aware.
#
# (c) Copyright 2026 Liminal HQ, Scott Morris
# SPDX-License-Identifier: Apache-2.0 OR MIT

set -euo pipefail

banner="${1:-assets/hero.svg}"
max_bytes=61440
fail=0

err() {
	echo "check-banner: $*" >&2
	fail=1
}

[ -f "$banner" ] || {
	echo "check-banner: $banner not found" >&2
	exit 1
}

xmllint --noout "$banner" || err "not well-formed XML"

if grep -nE '<script|foreignObject|href="https?:|xlink:href="https?:|@import|url\(\s*["'"'"']?https?:' "$banner"; then
	err "forbidden construct (script, foreignObject, external reference or @import)"
fi

size="$(wc -c <"$banner")"
[ "$size" -le "$max_bytes" ] || err "size ${size} bytes is over ${max_bytes}"

grep -q 'prefers-reduced-motion' "$banner" || err "missing prefers-reduced-motion handling"
grep -q 'prefers-color-scheme' "$banner" || err "missing prefers-color-scheme handling"
grep -q '<title' "$banner" || err "missing <title>"
grep -q '<desc' "$banner" || err "missing <desc>"
grep -q 'role="img"' "$banner" || err 'missing role="img"'
grep -q 'SPDX-License-Identifier' "$banner" || err "missing licence header"

tmp="$(mktemp --suffix=.png)"
trap 'rm -f "$tmp"' EXIT
rsvg-convert "$banner" -o "$tmp" || err "rsvg-convert could not render the banner"

if [ "$fail" -eq 0 ]; then
	echo "check-banner: ok (${size} bytes)"
fi
exit "$fail"
