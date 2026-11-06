# Mailslot specification

## Table of contents

- [Purpose](#purpose)
- [Users and scenarios](#users-and-scenarios)
- [Non-goals](#non-goals)
- [Components](#components)
- [HTTP contract](#http-contract)
- [Resumable and deduplicated uploads](#resumable-and-deduplicated-uploads)
- [Web upload page](#web-upload-page)
- [Mailslot links](#mailslot-links)
- [Command line](#command-line)
- [Desktop app behaviour](#desktop-app-behaviour)
- [Settings](#settings)
- [Limits](#limits)
- [UPnP port mapping](#upnp-port-mapping)
- [Files, paths and names](#files-paths-and-names)
- [Sync roadmap](#sync-roadmap)
- [Acceptance criteria](#acceptance-criteria)

## Purpose

Mailslot is a private drop slot for files. It runs an upload endpoint behind a secret link, so a person, a phone or an agent can send a file to your machine with a browser, `curl -T` or the Mailslot client. Each file lands in a folder you choose with its SHA-256 checksum, and nothing is ever served back.

It is both a server and a client. The same engine receives files in the desktop app or a headless service, and sends them from the desktop app or the command line.

## Users and scenarios

- **Receiving from an agent.** Start sharing, paste the generated instructions into an agent session, and verify the checksum it reports.
- **Receiving from a phone.** Scan the QR code on the link card, pick photos in the browser, and watch them arrive.
- **Sending to another machine.** Run `mailslot-cli send FILE LINK` or use the Send page. Interrupted uploads resume and files the receiver already holds are skipped.
- **Headless receiver.** Run `mailslot-cli serve` on a server with no display.
- **Embedding.** A Rust application links `mailslot-core` and drives the same engine behind its own interface.

## Non-goals

- Serving files back, browsing the destination folder or any download feature. The one exception is the opt-in read token described in the [sync roadmap](#sync-roadmap), which is off by default and separate from the upload token.
- Accounts, a cloud relay or a hosted service.
- Opening, executing or extracting anything that is received.
- Replacing a full sync tool. Sync here means push from a watched folder first.

## Components

| Component | Role |
|---|---|
| `mailslot-core` | GTK-free library: `Engine`, protocol types, server, client, config, UPnP port mapping, network information |
| `mailslot-cli` | Command-line binary: `send`, `serve`, `watch`, `pair`, `status` |
| `mailslot` (crate `mailslot-gtk`) | GTK4 and libadwaita desktop app: Share page, Send page, Preferences, history |

Identifiers are fixed across the project:

- Application ID `ca.liminalhq.mailslot`, GUI binary `mailslot`, CLI binary `mailslot-cli`.
- Resource prefix `/ca/liminalhq/mailslot/`, gettext domain `mailslot`.
- App icon name `ca.liminalhq.mailslot` and `ca.liminalhq.mailslot-symbolic`.

## HTTP contract

Every route lives under `/<token>/`. Anything else returns a bare `404 not found`. The token is compared in constant time, and a wrong token is indistinguishable from an unknown path.

| Method and path | Result |
|---|---|
| `GET /<token>` | `308` to `/<token>/` |
| `GET /<token>/` | Web upload page (no inline script) |
| `GET /<token>/drop.js`, `/drop.css` | Page assets, embedded in the binary |
| `GET /<token>/health` | `200` JSON, see below |
| `GET /<token>/have?sha256=<hex>` | Duplicate check, see [Resumable and deduplicated uploads](#resumable-and-deduplicated-uploads) |
| `HEAD /<token>/<name>` | Resume state for a file, see the same section |
| `PUT /<token>/<name>` (and `POST`, for script compatibility) | Raw body upload |

The health body is:

```json
{"ok":true,"version":"0.1.0","max_bytes":209715200,"expires_at":null,"accepting":true,"capabilities":["chunked","sha256","resume","dedupe"]}
```

`expires_at` is `null` or an RFC 3339 timestamp. `accepting` is false while sharing is paused or stopping.

### Upload

`PUT /<token>/<name>` takes the raw body. A complete upload returns `201 Created`, the saved file name as the body and these headers:

- `X-Mailslot-Saved-As: <name>`, the name after sanitizing and collision handling.
- `X-Mailslot-Sha256: <hex>`, the digest the server computed while writing.

With `Accept: application/json` the body is `{"name":"…","size":N,"sha256":"…"}` instead. Chunked bodies (`curl -T -`) are supported.

`curl -T FILE URL/` keeps working unchanged. Resume and dedupe are additive.

### Integrity

A sender may include `X-Mailslot-Expected-Sha256: <hex>` or the RFC 9530 `Content-Digest: sha-256=:<base64>:`. If the finished upload does not match, the server deletes it and returns `422`.

### Status codes

| Status | Meaning |
|---|---|
| `201` | Complete, saved |
| `200` | Duplicate: the server already holds this content and kept nothing new |
| `202` | Partial range accepted, more expected |
| `308` | Redirect from the token root |
| `400` | Empty name after sanitizing, or a malformed header |
| `404` | Bad token or unknown path |
| `409` | `Content-Range` start does not match the offset the server holds |
| `410` | Link expired, or a one-shot link has been used |
| `413` | Over the size cap (`Content-Length` is checked up front, and chunked bodies are counted as they arrive) |
| `415` | Extension not allowed |
| `416` | `Content-Range` is not satisfiable |
| `422` | Checksum mismatch |
| `429` | Concurrency or rate limit, with `Retry-After` |
| `503` | Paused or stopping |
| `507` | Free space is below the floor |

### Headers on every response

- `X-Content-Type-Options: nosniff`
- `Cache-Control: no-store`
- `Referrer-Policy: no-referrer`
- `X-Frame-Options: DENY`
- `Server: mailslot`, with no version.

The page carries `Content-Security-Policy: default-src 'none'; script-src 'self'; style-src 'self'; connect-src 'self'; img-src 'self' data:; form-action 'none'; frame-ancestors 'none'; base-uri 'none'`.

## Resumable and deduplicated uploads

Uploads that carry `X-Mailslot-Expected-Sha256` are resumable. The server keeps an interrupted upload as a partial file keyed by the file name and the expected digest, and removes it after one hour without activity. An upload without an expected digest is deleted when it aborts, as before.

**`HEAD /<token>/<name>`** reports what the server holds, keyed by `X-Mailslot-Expected-Sha256` when the request carries it:

- `200` with `X-Mailslot-Offset: <bytes>` (`0` when there is no partial) and `X-Mailslot-Exists: true|false`.
- `Exists` is true when a finished file with that digest has already been received.

**Resuming.** The client sends `PUT` with `Content-Range: bytes <start>-<end>/<total>` where `<start>` equals the offset the server reported.

- A matching range appends to the partial. The server answers `202` with `X-Mailslot-Offset` until the last byte arrives, then finalizes and answers `201` with the same headers as an unbroken upload. The SHA-256 covers the whole file, including the part received earlier.
- A start that does not match returns `409` with the current `X-Mailslot-Offset`, so the client can realign.
- A range past the declared total returns `416`.

**`GET /<token>/have?sha256=<hex>`** returns `200` with `{"have":true,"name":"…","size":N}` or `{"have":false}`. The index covers files received in the current history window, not a scan of the destination folder. A `PUT` whose expected digest is already held returns `200` with `X-Mailslot-Duplicate: true` and writes nothing.

Dedupe never reveals files outside the token. The answer is only about a digest the caller already knows.

## Web upload page

The server serves a small upload page from embedded assets.

- Drag and drop or browse, for multiple files and folders.
- Per-file progress, speed and ETA, pause and resume, retry, and a copy button for the SHA-256 the server returns.
- A clear success or error state per file, and it works on phones.
- It uses the same resumable protocol as the client: `HEAD` for the offset, `Content-Range`, and `have` for dedupe.
- Plain JavaScript and CSS with no framework, no CDN and no inline script, so the content security policy stays strict.
- It lists only uploads from the current browser session. It never lists or serves anything from the destination folder.
- Browsers disable WebCrypto on plain HTTP outside localhost, so the page pre-hashes only when `crypto.subtle` exists (HTTPS) and otherwise skips dedupe. The server's SHA-256 is authoritative.
- Light and dark follow `prefers-color-scheme`, with system fonts. The brand red and blue appear only in the logo and links.

## Mailslot links

A link carries everything a client needs: `mailslot://HOST:PORT/TOKEN[?tls=1&pin=<base64>&label=<text>]`.

- `tls=1` means HTTPS. Without it the link means plain HTTP.
- `pin` is the base64 SHA-256 of the server's public key (the value `curl --pinnedpubkey sha256//…` takes). Clients must verify it when it is present.
- `label` is a display name for history and per-sender folders.

The Share page shows both the `mailslot://` link and the plain `http(s)://HOST:PORT/TOKEN/` URL for browsers and `curl`. The QR code encodes the URL form so a phone camera opens the web page.

## Command line

`mailslot-cli` has no GTK dependency.

| Command | Purpose |
|---|---|
| `send [FILE…\|-] LINK` | Upload files, folders (recursively) or stdin (`--name`). Resumes, retries with backoff, sends the SHA-256 up front, skips duplicates, runs files in parallel (`--jobs`), and limits bandwidth (`--limit`). `--json` prints one JSON object per event. Exit code is non-zero if any file failed |
| `serve` | Run a headless receiver. Prints one line `URL <http://HOST:PORT/TOKEN/>` to stdout once listening, then serves until SIGTERM or SIGINT |
| `watch DIR LINK` | Push new and changed files in a folder (roadmap v1, uses the same send queue) |
| `pair` | Print a link and a QR code in the terminal for a running or configured receiver |
| `status` | Show the configured receiver's state, link and recent uploads |

Flags override the TOML configuration file. The smoke entry point is `cargo run -p mailslot-core --example serve -- --bind 127.0.0.1:0 --dir DIR [--max-mb N]`, with the same one-line `URL` contract as `serve`.

`mailslot --version` and `mailslot-cli --version` print `<binary> X.Y.Z`.

## Desktop app behaviour

The app is a single `adw::ApplicationWindow` using `adw::ToolbarView`, with a view switcher for Share, Send and History.

- **Share page.** A Start and Stop toggle in the header bar. When stopped, an `adw::StatusPage`. When sharing, the link card: URL row with copy, QR code, interface picker, a masked-token toggle, the snippet menu (curl, PowerShell, wget, agent prompt) and the live Incoming list. Each row shows icon, name, size, progress, speed, status and SHA-256 with copy, plus Open and Show in folder.
- **Link.** The token is persisted. "New link" asks for confirmation, generates a new token and invalidates the old URL immediately, aborting uploads in flight on the old token.
- **Open to the internet.** A switch on the link card, off by default. When on, an "Internet-exposed" indicator stays in the header and the external URL appears with a copy button. See [UPnP port mapping](#upnp-port-mapping).
- **Send page.** Pick files or folders, or paste or scan a link, and see progress per file. The queue survives restarts.
- **History.** Sent and received transfers, kept for the configured period.
- **Destination.** Chosen with `gtk::FileDialog`. Dropping a folder on the window also sets it.
- **Notifications.** One per completed upload, coalesced when many arrive together.
- **Close.** Closing the window while sharing asks "Stop sharing and quit?".
- **Errors.** Start failures show an `adw::Banner` with an action (for example "Use port 8932"), transient problems use `adw::Toast`, and destructive actions use `adw::AlertDialog`.
- **Shortcuts.** `Ctrl+Q` quit, `Ctrl+,` preferences, `Ctrl+Shift+C` copy link, `Ctrl+Shift+S` start or stop, `Ctrl+?` shortcuts dialog.
- **Look.** Pure libadwaita with symbolic icons and system accent colours. There are no colour literals outside `style.css`, and `style.css` uses only libadwaita named colours. Brand colours appear only in the app icon, About dialog artwork and metainfo branding.

## Settings

Settings live in `$XDG_CONFIG_HOME/ca.liminalhq.mailslot/config.toml`. The desktop app edits them in an `adw::PreferencesDialog` with one page per section and a "Reset to defaults" button on each. The CLI reads the same file and flags override it. Unknown keys are preserved. The file mode is `0600` because it holds the token.

### Sharing

| Key | Default | Meaning |
|---|---|---|
| `sharing.port` | `8931` | Listening port. `0` picks a free port |
| `sharing.bind` | `"0.0.0.0"` | Bind address |
| `sharing.start_on_launch` | `false` | Start sharing when the app opens |
| `sharing.link_expiry_minutes` | `60` | Link lifetime. `0` means never |
| `sharing.stop_after_files` | `0` | Stop after this many files. `0` means off |
| `sharing.auto_stop_minutes` | `0` | Stop after this long. `0` means off |

### Storage

| Key | Default | Meaning |
|---|---|---|
| `storage.dest_dir` | `~/Downloads/Mailslot` | Destination folder, created on first start |
| `storage.subfolders` | `"none"` | `none`, `date` (`YYYY-MM-DD`) or `sender` (the link label) |
| `storage.collision` | `"suffix"` | Names are never overwritten. `suffix` adds `-1`, `-2`… |
| `storage.sha256_sidecars` | `false` | Write `<name>.sha256` next to each file |
| `storage.verify_on_receive` | `true` | Compare against an expected digest when one is sent |

### Limits

| Key | Default | Meaning |
|---|---|---|
| `limits.max_file_mb` | `200` | Per-file cap |
| `limits.session_quota_mb` | `0` | Total per sharing session. `0` means unlimited |
| `limits.min_free_mb` | `1024` | Refuse uploads below this free space |
| `limits.max_concurrent` | `4` | Simultaneous uploads |
| `limits.rate_per_ip_per_minute` | `120` | Requests per peer address. `0` means off |
| `limits.idle_timeout_seconds` | `60` | Abort an upload with no data for this long |

### Security

| Key | Default | Meaning |
|---|---|---|
| `security.token` | generated | 256-bit token, base64url without padding |
| `security.lockout_attempts` | `10` | Bad-token requests before a peer is dropped. `0` means off |
| `security.lockout_minutes` | `10` | How long a locked-out peer is dropped |
| `security.allowed_extensions` | `[]` | Allow list. Empty means any |
| `security.https` | `false` | Serve HTTPS with a self-signed certificate and show the key pin |
| `security.warn_on_executables` | `true` | Flag executable and script extensions in the UI |

### Network

| Key | Default | Meaning |
|---|---|---|
| `network.upnp` | `false` | Remember the "Open to the internet" choice |
| `network.upnp_lease_minutes` | `60` | Mapping lease, capped by the link expiry |
| `network.public_ip_lookup` | `false` | Allow the one-request public address lookup |
| `network.interface` | `"auto"` | Interface used for the displayed link |

### Notifications and history

| Key | Default | Meaning |
|---|---|---|
| `notifications.notify` | `true` | Desktop notification per completed upload |
| `history.keep` | `true` | Keep a history of sent and received transfers |
| `history.days` | `90` | Retention in days |

History is stored as JSON Lines in `$XDG_DATA_HOME/ca.liminalhq.mailslot/history.jsonl` with time, direction, name, size, SHA-256 and peer. The token is never stored there. The TLS certificate lives in `tls/` in the same directory.

## Limits

- Filenames are sanitized as described in [Files, paths and names](#files-paths-and-names).
- Progress events are throttled to 10 per second per upload. Terminal events are never dropped.
- Header read timeout is 10 seconds and the body idle timeout is `limits.idle_timeout_seconds`.
- An aborted upload deletes its `.part` file unless it is resumable. Stale partials older than one hour are swept from the destination on startup and while running.
- The server holds no file in memory. Bodies stream to disk with a running byte count.

## UPnP port mapping

Mailslot can ask the router to forward the sharing port so the link works from the internet. It is explicit, short-lived and visible.

- **Opt-in per session.** The "Open to the internet" switch is off by default. `network.upnp` only remembers the last choice.
- **Lease-based.** The mapping is a TCP mapping with a lease (`network.upnp_lease_minutes`, default one hour, never infinite). It is renewed while sharing and never outlives the link expiry.
- **Removed on exit.** The mapping is removed on Stop, on quit and on SIGTERM or SIGINT. On startup, Mailslot removes stale mappings whose description is `Mailslot` and which it can identify.
- **Protocols.** UPnP-IGD through `igd-next`, with NAT-PMP and PCP fallback.
- **Visible.** While a mapping is active the header shows "Internet-exposed" and the link card shows the external URL with a copy button.
- **Honest diagnostics.** The app reports, in plain language: no gateway found, UPnP disabled on the router, a port already mapped by something else, and double NAT or carrier-grade NAT. Double NAT is detected when the router's external address is private or in `100.64.0.0/10`, or differs from the address returned by the opt-in public-IP lookup.
- **Fallback.** When mapping fails, the Network panel shows manual port-forward instructions and a reminder to remove the forward afterwards.
- **Testing.** The core test suite runs a fake IGD (SSDP and SOAP). `make upnp-test` runs `miniupnpd` in a Docker network. Checking a real router is manual.

## Files, paths and names

- Default destination `~/Downloads/Mailslot`, created on first start.
- Config `$XDG_CONFIG_HOME/ca.liminalhq.mailslot/config.toml`. Data `$XDG_DATA_HOME/ca.liminalhq.mailslot/`.
- Names are percent-decoded, and `\` becomes `/`, then only the last path component is kept.
- Control characters and bidirectional overrides (U+202A to U+202E, U+2066 to U+2069) are removed.
- Anything outside letters, digits and ``._- ()`` becomes `_`.
- Leading dots and spaces and trailing dots and spaces are trimmed, so there are no hidden files.
- Names are limited to 150 characters and 240 bytes. If nothing is left the name is `upload`.
- Temporary files are created exclusively in the destination folder (`O_CREAT|O_EXCL|O_NOFOLLOW`) with mode `0600`, then claimed with a hard link under the final name (which fails if it exists, so Mailslot tries `-1`, `-2` and so on, keeping double extensions like `name-1.tar.gz`) and the temporary file is removed. Final files have mode `0644` with no execute bit.

## Sync roadmap

The first release is manual send with dedupe and resume. Sync grows from the same protocol.

### Version 1

- `mailslot-cli watch DIR LINK` and a GUI Outbox folder: push sync from a watched folder using inotify, content-hash dedupe, resume after network drops and a send queue that survives restarts.
- Pairing by QR code or `mailslot://` link, plus opt-in mDNS discovery on the LAN so a phone or laptop finds the app without typing.
- A per-sender subfolder option on the server, keyed by link label, and a history view of both sent and received transfers.
- `mailslot-ipc`: a local JSON-RPC API (Unix socket or named pipe) and a headless `mailslotd`, so non-Rust interfaces drive the same engine.

### Later

- A separate opt-in read-only token for `pull` (list and fetch), enabling one-way pull sync and then two-way sync with conflict rules.
- Shared clipboard and text snippets, and "send to my other machine".
- Multiple named links with per-link rules and quotas.
- End-to-end encryption of payloads (age), so plain-HTTP and untrusted-relay transfers stay private.
- Relay and tunnel hints.
- UniFFI or C ABI bindings for in-process use from C++ and C#.

## Acceptance criteria

1. `curl -T FILE URL/FILE` saves the file and returns a SHA-256 equal to `sha256sum FILE`.
2. `mailslot-cli send` does the same, resumes after a killed connection with the final SHA-256 correct, and skips a file the server already holds.
3. A phone on the LAN uploads two files through the web page and both appear in the Incoming list with correct checksums.
4. "New link" makes the old URL return `404`.
5. Every response outside the token is an identical `404`, and traversal names (`../x`, `%2e%2e%2fx`, `a\b`, bidirectional characters, `.hidden`) are saved safely inside the destination.
6. Twenty concurrent uploads of the same name produce twenty distinct files.
7. Limits are enforced: `413` over the size cap, `507` below the free-space floor, `429` above the concurrency limit, `410` after expiry.
8. A fake IGD test and `make upnp-test` show the mapping created, renewed and removed on Stop, and the app shows "Internet-exposed" only while it exists.
9. Fifty concurrent uploads do not stall the server, and the GTK main loop is never blocked.
10. Following the system accent colour setting changes the app's accent. There are no colour literals outside `style.css`.
11. `mailslot-core` builds without GTK and its embed example runs.
12. `make validate` passes.
