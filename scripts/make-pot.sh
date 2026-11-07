#!/usr/bin/env bash
# Generates po/mailslot.pot with xgettext and merges it into each catalogue listed in po/LINGUAS.
#
# (c) Copyright 2026 Liminal HQ, Scott Morris
# SPDX-License-Identifier: Apache-2.0 OR MIT

set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."

# Every file that uses gettext or marks UI strings translatable must be in POTFILES.in.
missing=0
while IFS= read -r f; do
	if ! grep -qxF "$f" po/POTFILES.in; then
		echo "make-pot: $f has translatable strings but is not in po/POTFILES.in" >&2
		missing=1
	fi
done < <(grep -rlE 'gettext\(|ngettext\(|translatable="yes"' crates --include='*.rs' --include='*.ui' | grep -v '/i18n.rs$' | sort)
[ "$missing" -eq 0 ] || exit 1

mapfile -t sources < <(grep -vE '^\s*(#|$)' po/POTFILES.in)
rs=() ui=() other=()
for f in "${sources[@]}"; do
	case "$f" in
	*.rs) rs+=("$f") ;;
	*.ui) ui+=("$f") ;;
	*) other+=("$f") ;;
	esac
done

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
common=(--from-code=UTF-8 --add-comments=TRANSLATORS --package-name=mailslot --msgid-bugs-address=https://github.com/liminal-hq-labs/mailslot/issues --no-location)

xgettext "${common[@]}" --language=Rust --keyword=gettext --keyword=ngettext:1,2 --keyword=pgettext:1c,2 -o "$tmp/rs.pot" "${rs[@]}"
xgettext "${common[@]}" --language=Glade -o "$tmp/ui.pot" "${ui[@]}"
xgettext "${common[@]}" --language=Desktop --keyword=Name --keyword=GenericName --keyword=Comment --keyword=Keywords -o "$tmp/desktop.pot" data/ca.liminalhq.mailslot.desktop
msgcat --use-first --sort-output -o po/mailslot.pot "$tmp/rs.pot" "$tmp/ui.pot" "$tmp/desktop.pot"
echo "make-pot: po/mailslot.pot ($(grep -c '^msgid ' po/mailslot.pot) entries)"

while IFS= read -r lang; do
	case "$lang" in '' | '#'*) continue ;; esac
	if [ -f "po/${lang}.po" ]; then
		msgmerge --update --backup=none --quiet "po/${lang}.po" po/mailslot.pot
	else
		msginit --no-translator --locale="$lang" --input=po/mailslot.pot --output-file="po/${lang}.po" >/dev/null
	fi
	echo "make-pot: po/${lang}.po"
done <po/LINGUAS
