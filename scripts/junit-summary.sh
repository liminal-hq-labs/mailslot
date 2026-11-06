#!/usr/bin/env bash
# Prints a Markdown summary of JUnit XML files, for the GitHub job summary.
#
# (c) Copyright 2026 Liminal HQ, Scott Morris
# SPDX-License-Identifier: Apache-2.0 OR MIT

set -euo pipefail

# Usage: junit-summary.sh FILE.xml...

# shellcheck disable=SC2016
printf '| Suite | Tests | Failures | Errors | Skipped |\n|---|---|---|---|---|\n'
for f in "$@"; do
	if [ ! -f "$f" ]; then
		# shellcheck disable=SC2016
		printf '| `%s` | missing | | | |\n' "$f"
		continue
	fi
	line="$(grep -m1 -E '<testsuites ' "$f" || true)"
	attr() { printf '%s' "$line" | grep -oE "$1=\"[0-9]+\"" | head -n1 | grep -oE '[0-9]+' || echo 0; }
	# shellcheck disable=SC2016
	printf '| `%s` | %s | %s | %s | %s |\n' "$(basename "$f")" "$(attr tests)" "$(attr failures)" "$(attr errors)" "$(attr skipped)"
done
