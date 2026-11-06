# CLAUDE.md

A summary of [AGENTS.md](AGENTS.md), which is canonical. Keep the two in sync.

## What this is

Mailslot is a private file-drop server and client: GTK4 and libadwaita desktop app (`mailslot`), CLI (`mailslot-cli`) and an embeddable Rust library (`mailslot-core`). App ID `ca.liminalhq.mailslot`. Behaviour and the HTTP contract live in `SPEC.md`.

## Rules to remember

- **Canadian English.** Real em dashes. No hard-wrapped markdown prose. Ship the result, not the journey: no "this PR" or reviewer names in code or docs.
- **Conventional Commits.** Body in markdown with bold labels and no headings, hard-wrapped, written to a file and committed with `git commit -F`. End with a `Co-Authored-By` trailer.
- **Git.** Topic branches, merge commits only. Never push, tag, publish or dispatch workflows unless asked. Never `git add -A` or `git commit -a`. `PLAN.md` stays out of git.
- **Headers.** SPDX header on every source, script, YAML, SVG and `.ui` file: summary line, blank, "(c) Copyright 2026 Liminal HQ, Scott Morris", `SPDX-License-Identifier: Apache-2.0 OR MIT`.
- **Core first.** `mailslot-core` has no GTK dependency and exposes an `Engine` with an event stream. GTK and the CLI are thin shells.
- **GTK.** GObjects only on the main thread. Never block the main loop. Strings through `gettext`. No colour literals outside `style.css`, which uses only libadwaita named colours.
- **Async.** All I/O is async on tokio. Clippy bans blocking `std::fs`, `std::thread::sleep` and `reqwest::blocking`. Never hold a lock across `.await`.
- **Security.** Never log the token. Identical `404` outside the token. Never serve files back. Never open received files.
- **Tests.** Display tests are exactly the `#[ignore = "needs display"]` ones. `make validate` is the single gate. Report exactly what ran.
- **Docker.** Plain OCI, no published ports, `--network host` only where documented.
- **Docs.** Update `SPEC.md`, `docs/decisions.md` and `docs/architecture.md` in the same change as the behaviour.

## Commands

```sh
make image      # build the dev image
make validate   # full gate: lint, test, UI tests, smoke, UPnP, data, deny
make test       # nextest, no display
make run        # GUI on the host Wayland session via Docker
```
