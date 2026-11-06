# mailslot-core

The engine behind Mailslot, a private file-drop endpoint: anyone holding a secret link can push files into a folder, and nothing is ever served back.

This crate has no UI dependency. It provides an async HTTP server, a resumable upload client, router port mapping (UPnP-IGD, PCP, NAT-PMP), self-signed HTTPS with key pinning, settings, history, and an `Engine` that ties them together behind a small command set and a typed event stream.

```toml
[dependencies]
mailslot-core = "0.1"
```

See the [crate documentation](https://docs.rs/mailslot-core) and `examples/embed.rs` for embedding the engine in any Rust program.

Licensed under either of the Apache License, Version 2.0 or the MIT licence, at your option.
