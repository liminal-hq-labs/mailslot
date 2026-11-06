# Security

Mailslot is designed to be reachable from the internet while it is sharing. This page describes what it protects, what it defends against and what it does not.

## Assets

- The disk: capacity and existing files.
- The host: no code execution through received files.
- The token: whoever has it can upload.
- Upload confidentiality and integrity.

## Model

The link is the credential. Anyone holding it can upload within the configured limits, and nobody else can learn anything: every path outside the token returns the same `404`. Nothing is ever served back, so a leaked link cannot expose existing files. A leaked link can only be used to send files, and "New link" ends that immediately.

## Threats and mitigations

| Threat | Mitigation |
|---|---|
| Internet scanners | Identical `404` for everything outside the token, a generic `Server` header, no directory listing, and a 256-bit token that makes guessing infeasible. Bad-token lockout drops a peer after repeated misses |
| Token leak through shell history, chat logs, `Referer` or proxy logs | `Referrer-Policy: no-referrer`, the token is never logged, "New link" is one click, and expiry, one-shot mode and quotas limit what a leaked token can do |
| Path traversal, overwrite and symlink tricks | Names are reduced to one sanitized component. Temporary files use `O_EXCL` and `O_NOFOLLOW`. Final names are claimed with a hard link that fails if the name exists |
| Disk filling | Per-file cap, session quota, free-space floor (`507`) and a concurrency limit |
| Slow connections and exhaustion | Header read timeout of 10 seconds, body idle timeout, a connection limit and a per-peer rate limit |
| Malicious content | Files are never opened, executed or extracted. They are written with mode `0644` and no execute bit. The UI flags executable and script extensions, and "Open" uses the default handler only on an explicit click |
| Tampering on plain HTTP | The expected-digest header detects modification when the digest travels out of band. HTTPS with a pinned key prevents it |
| Forgotten exposure | A prominent sharing state, link expiry, an auto-stop timer and an "Internet-exposed" indicator whenever a port mapping exists |
| Request smuggling and parser bugs | hyper 1.x with no custom HTTP parsing, and `cargo-deny` advisories in CI |
| Local privilege | Runs as the user. The Flatpak is limited to the network and the chosen folder |
| Resume abuse | Partials are keyed by name and expected digest, limited by the quota and the per-file cap, and swept after an hour |
| Dedupe probing | `have` answers only for a digest the caller supplies, and only for files received within the history window |

## UPnP

A UPnP mapping exposes the machine to the whole internet, not just the people you told. For that reason:

- It is opt-in each session, through a switch that is off by default.
- It is time-limited: a lease of one hour by default, renewed only while sharing and never longer than the link expiry.
- It is visible: "Internet-exposed" stays in the header while the mapping exists, and the external URL is shown.
- It is removed on Stop, on quit and on SIGTERM or SIGINT, and stale Mailslot mappings are removed on startup.
- It never sets a permanent lease.

Routers that ignore leases can leave a mapping behind after a crash. Check the router if Mailslot was killed with SIGKILL or the machine lost power while exposed.

## Logs

The log never contains the token. The engine does not log it or a full link, and the log writer removes token-shaped path segments and `token=` or `pin=` values from every line as a second guard. The log file is created with mode `0600` in `$XDG_STATE_HOME/ca.liminalhq.mailslot` and is capped at three files of 1 MiB. It does record file names, sender addresses and the router's address, so look it over before attaching it to a public issue. "Copy diagnostics" redacts the same way.

## Transport

Plain HTTP is the default because it keeps `curl` usage simple. On a trusted LAN that is reasonable. Over the internet, enable HTTPS in the Security preferences. Mailslot generates a self-signed certificate and shows its public-key pin, and `mailslot://` links carry the pin so Mailslot clients verify it. For `curl`, use `--pinnedpubkey 'sha256//…'` instead of `-k`.

Payload encryption independent of transport (age) is on the roadmap.

## Filename sanitizing

The full rules are in the [specification](../SPEC.md#files-paths-and-names): control and bidirectional characters removed, a conservative character set, no leading dot, length limits and a fallback name.

## Not covered

- Malware scanning. A scan hook is a future option.
- A hostile local user on the same machine. The config file is `0600`, but a local attacker running as the same user can read it.
- Anyone who holds the link while it is valid. Treat it like a password.

## Reporting a vulnerability

Report privately through GitHub's private vulnerability reporting on the repository's Security tab. Please don't open a public issue for a vulnerability.
