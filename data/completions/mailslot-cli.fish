# Fish completion for mailslot-cli.
#
# (c) Copyright 2026 Liminal HQ, Scott Morris
# SPDX-License-Identifier: Apache-2.0 OR MIT

complete -c mailslot-cli -f
complete -c mailslot-cli -n __fish_use_subcommand -a send -d 'Upload files, folders or stdin to a link'
complete -c mailslot-cli -n __fish_use_subcommand -a serve -d 'Run a headless receiver'
complete -c mailslot-cli -n __fish_use_subcommand -a watch -d 'Push a folder to a link'
complete -c mailslot-cli -n __fish_use_subcommand -a pair -d 'Print the link and a QR code'
complete -c mailslot-cli -n __fish_use_subcommand -a status -d 'Show receiver state'
complete -c mailslot-cli -s h -l help -d 'Print help'
complete -c mailslot-cli -s V -l version -d 'Print the version'

complete -c mailslot-cli -n '__fish_seen_subcommand_from send' -F
complete -c mailslot-cli -n '__fish_seen_subcommand_from send' -l name -r -d 'Name for data read from stdin'
complete -c mailslot-cli -n '__fish_seen_subcommand_from send' -l jobs -r -d 'Files in parallel'
complete -c mailslot-cli -n '__fish_seen_subcommand_from send' -l limit -r -d 'Bandwidth limit, for example 2M'
complete -c mailslot-cli -n '__fish_seen_subcommand_from send' -l json -d 'Print one JSON object per event'

complete -c mailslot-cli -n '__fish_seen_subcommand_from serve' -l bind -r -d 'Address to listen on'
complete -c mailslot-cli -n '__fish_seen_subcommand_from serve' -l dir -r -F -d 'Destination folder'
complete -c mailslot-cli -n '__fish_seen_subcommand_from serve' -l port -r -d 'Port'
complete -c mailslot-cli -n '__fish_seen_subcommand_from serve' -l max-mb -r -d 'Per-file size cap in MB'
complete -c mailslot-cli -n '__fish_seen_subcommand_from watch' -F
