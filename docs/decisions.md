# Decisions

Each decision is numbered and stays here even when superseded, with a note on what replaced it.

| # | Decision | Choice | Why |
|---|---|---|---|
| D1 | Name and app ID | **Mailslot**, `ca.liminalhq.mailslot`, GUI binary `mailslot`, CLI binary `mailslot-cli` | Anyone with the address can push something in, and nothing comes back out. It fits the house-and-threshold naming (Foyer, Mudroom, Jar) |
| D2 | Toolchain | Rust pinned at `1.98.0` with rustfmt and clippy, edition 2024, `rust-version = "1.92"` | Same pin as Folio and Fizz Level Studio. Edition 2024 matches Fizz 2 |
| D3 | GUI toolkit | `gtk4` 0.11 (`v4_20`), `libadwaita` 0.9 (`v1_8`), `glib` and `gio` 0.22 | The floor is GNOME 49. Nothing needs newer APIs. The image and Flatpak target GTK 4.24 and libadwaita 1.10 |
| D4 | UI definition | XML composite templates with a gresource, and small dynamic rows in Rust | The gtk-rs book path, no extra build tool, and easy to debug. Blueprint can follow if the XML grows heavy |
| D5 | HTTP server | axum 0.8 on tokio | Streams bodies with a running byte cap, graceful shutdown, and TLS through `axum-server` |
| D6 | Core-first architecture | `mailslot-core` has no GTK dependency and exposes an `Engine` with commands, an event stream and a state snapshot. The GUI and CLI are thin shells | Any frontend (Tauri, Qt, WinUI, a CLI) can sit on the same engine. CI guards the boundary with a `core-embed` job |
| D7 | Async everywhere | All I/O in core is async on tokio. No blocking calls in async paths, enforced by clippy `disallowed-methods` and `await_holding_lock` | The server must stay responsive under load and the GTK main loop must never block |
| D8 | Thread bridge | The GTK side consumes the engine's event stream with `glib::spawn_future_local`. Commands go back through `Engine` methods | GObjects never leave the main thread |
| D9 | Settings storage | TOML with `serde` at the XDG config directory, mode `0600` | GSettings needs schema installation, which plain `cargo run` skips. TOML works under Flatpak and is shared with the CLI |
| D10 | Notifications | `gio::Notification` through `Application::send_notification` | Uses the notification portal under Flatpak automatically |
| D11 | QR code | `qrcode` rendered to a `gdk::MemoryTexture` shown in a `gtk::Picture`, nearest-neighbour with a white quiet zone | No SVG loader dependency and it stays crisp |
| D12 | Token | 32 random bytes as base64url without padding (43 characters), compared in constant time | 256 bits of entropy |
| D13 | Hashing | Streaming SHA-256 while writing | The workflow is to verify a reported checksum |
| D14 | Network information | Interface and gateway discovery locally. Public address lookup is opt-in and off by default | No automatic outbound calls |
| D15 | HTTPS | Self-signed certificate through `rcgen` and `rustls` with the `ring` backend. The public-key pin is shown and put in links | Avoids OpenSSL and cmake. A pin is safer than `curl -k` |
| D16 | Protocol extensions | Resume (`HEAD`, `Content-Range`) and dedupe (`have?sha256=`) are additive to the `PUT` contract | `curl -T` keeps working unchanged |
| D17 | Client library | The client lives in `mailslot-core` behind the `client` feature. The CLI and the Send page share it | One implementation of resume, retry and backoff |
| D18 | UPnP | `igd-next` with NAT-PMP and PCP fallback. Lease-based, opt-in per session, removed on stop | Convenience without a silent permanent exposure |
| D19 | Settings scope | Expiry, quota, lockout and HTTPS are in the first release, so the preferences are real from day one | The settings UI should not ship with placeholder pages |
| D20 | Native look | Pure libadwaita with system accent colours. No brand colours in the UI. `scripts/check-style.sh` enforces it | Mailslot should look like part of the desktop |
| D21 | Brand | Primary red, white and accent blue in the icon, banner, About artwork and metainfo only | A distinct identity without fighting the system theme |
| D22 | Internationalization | Every user-facing string goes through `gettext`. English ships first | House rule. A `po/` catalogue follows |
| D23 | Dev container | A Fedora 45 image in this repository with `ci-gtk` and `dev-gtk` stages, plain OCI, no published ports | Fedora 45 ships GTK 4.24 and libadwaita 1.10, matching development hosts. The image is specific to this project, so it stays here |
| D24 | Dev commands | `Makefile`, with `make validate` as the single gate that mirrors CI | `make` is everywhere. The house equivalent of `bun run validate` |
| D25 | CI | One job per check, a final `required` aggregate job, a shared CI image and sticky test-result comments | Each check shows as its own status |
| D26 | Packaging | Flatpak, AppImage, deb, rpm and an Arch PKGBUILD for the GUI. deb, rpm, a static musl tarball and the AppImage for the CLI. `mailslot-core` is publish-ready for crates.io | Every common way to install, and the CLI has no GTK dependency |
| D27 | deb floor | Built on Ubuntu 26.04. Ubuntu 24.04 LTS is not supported | The shortcuts dialog needs libadwaita 1.8 and system accent colours need 1.6. The Flatpak covers older distributions |
| D28 | Web page | Plain JavaScript and CSS, no framework, no inline script, strict content security policy | Small, auditable and works on phones |
| D29 | Licence | Apache-2.0 OR MIT, with the holder "Liminal HQ, Scott Morris" and SPDX headers | House standard |
| D30 | Git | Conventional Commits, topic branches and merge commits. Planning files stay out of git | House standard |
