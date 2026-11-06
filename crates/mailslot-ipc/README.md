# mailslot-ipc

A local JSON-RPC 2.0 API for the Mailslot engine over a Unix domain socket, and `mailslotd`, a headless daemon that serves it. Any program (a Qt or C# frontend, a script, a status bar widget) can start and stop sharing, change settings, send files and watch events without linking to Rust.

Messages are one JSON object per line. The socket lives in `$XDG_RUNTIME_DIR/ca.liminalhq.mailslot/mailslotd.sock` with mode `0600`. See the crate documentation for the method list.

Licensed under either of the Apache License, Version 2.0 or the MIT licence, at your option.
