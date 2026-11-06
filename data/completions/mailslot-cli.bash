# Bash completion for mailslot-cli.
#
# (c) Copyright 2026 Liminal HQ, Scott Morris
# SPDX-License-Identifier: Apache-2.0 OR MIT

_mailslot_cli() {
	local cur prev cmd
	cur="${COMP_WORDS[COMP_CWORD]}"
	prev="${COMP_WORDS[COMP_CWORD - 1]}"
	cmd="${COMP_WORDS[1]}"
	local commands="send serve watch pair status help"

	if [ "$COMP_CWORD" -eq 1 ]; then
		COMPREPLY=($(compgen -W "$commands --help --version" -- "$cur"))
		return
	fi

	case "$prev" in
	--dir | --name) COMPREPLY=($(compgen -d -- "$cur")); return ;;
	esac

	case "$cmd" in
	send)
		COMPREPLY=($(compgen -W "--name --jobs --limit --json --help" -- "$cur") $(compgen -f -- "$cur"))
		;;
	serve)
		COMPREPLY=($(compgen -W "--bind --dir --port --max-mb --help" -- "$cur"))
		;;
	watch)
		COMPREPLY=($(compgen -W "--help" -- "$cur") $(compgen -d -- "$cur"))
		;;
	pair | status)
		COMPREPLY=($(compgen -W "--help" -- "$cur"))
		;;
	esac
}
complete -F _mailslot_cli mailslot-cli
