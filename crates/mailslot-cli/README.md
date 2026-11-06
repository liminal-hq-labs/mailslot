# mailslot-cli

Command-line server and client for Mailslot, a private file drop: anyone holding a secret link can push files into a folder, and nothing is ever served back.

```sh
mailslot-cli serve --dir ~/Drops            # prints a link to share
mailslot-cli send report.zip 'mailslot://host:8931/TOKEN'
mailslot-cli watch ~/Outbox 'mailslot://host:8931/TOKEN'
mailslot-cli pair                           # link and QR code for this machine
mailslot-cli status 'mailslot://host:8931/TOKEN'
```

Run `mailslot-cli --help` for every option. Shell completions: `mailslot-cli completions bash|zsh|fish`. Man page: `mailslot-cli man`.

Licensed under either of the Apache License, Version 2.0 or the MIT licence, at your option.
