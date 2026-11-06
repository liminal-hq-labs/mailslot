#!/usr/bin/env bash
# Checks that every source file carries the SPDX licence header AGENTS.md requires.
#
# (c) Copyright 2026 Liminal HQ, Scott Morris
# SPDX-License-Identifier: Apache-2.0 OR MIT

set -euo pipefail

# The header is a one-line summary, a blank comment line, then the copyright block. Markdown, JSON,
# lockfiles and Cargo manifests are exempt. Exits 1 and lists offenders.

cd "$(git rev-parse --show-toplevel 2>/dev/null || dirname "$(dirname "${BASH_SOURCE[0]}")")"

missing=()

is_candidate() {
	case "$1" in
	*/Cargo.toml | Cargo.toml | *Cargo.lock | *.json | *.jsonc | *.md | *.png | *.zsync) return 1 ;;
	*.rs | *.sh | *.py | *.yml | *.yaml | *.js | *.css | *.ui | *.svg | *.toml | *.desktop | *.xml | *.1 | *.html) return 0 ;;
	*/Dockerfile | Dockerfile | Makefile | */PKGBUILD | PKGBUILD) return 0 ;;
	*) return 1 ;;
	esac
}

if git rev-parse --git-dir >/dev/null 2>&1; then
	list="$(git ls-files --cached --others --exclude-standard)"
else
	list="$(find . -type f -not -path './target/*' -not -path './.git/*' | sed 's|^\./||')"
fi

while IFS= read -r f; do
	[ -f "$f" ] || continue
	is_candidate "$f" || continue
	if ! head -n 10 "$f" | grep -q 'SPDX-License-Identifier: Apache-2.0 OR MIT'; then
		missing+=("$f")
	elif ! head -n 10 "$f" | grep -q '(c) Copyright 2026 Liminal HQ, Scott Morris'; then
		missing+=("$f (copyright line)")
	fi
done <<<"$list"

if [ "${#missing[@]}" -gt 0 ]; then
	echo "Missing or wrong licence header (see AGENTS.md, 'Licence headers'):"
	printf '  %s\n' "${missing[@]}"
	exit 1
fi
echo "check-headers: ok"
