# AGENTS.md

## Table of contents

- [Project status](#project-status)
- [Spelling and writing](#spelling-and-writing)
- [Markdown formatting](#markdown-formatting)
- [Authoring voice](#authoring-voice)
- [Commit messages](#commit-messages)
- [Pull requests](#pull-requests)
- [Git workflow](#git-workflow)
- [Local tooling](#local-tooling)
- [Licence headers](#licence-headers)
- [Architecture rules](#architecture-rules)
- [GTK rules](#gtk-rules)
- [Async rules](#async-rules)
- [Security rules](#security-rules)
- [Testing](#testing)
- [Documentation](#documentation)
- [Repository layout](#repository-layout)

## Project status

Mailslot is a private file-drop server and client for Linux: a GTK4 and libadwaita desktop app, a command-line client and server, and an embeddable Rust library, all built on one engine. It is in **early development** and pre-1.0. `SPEC.md` is the source of truth for behaviour and the HTTP contract, and `docs/decisions.md` records why.

## Spelling and writing

**REQUIREMENT:** Use **Canadian English** in UI strings, comments, commit messages, pull request text and documentation: `colour`, `centre`, `behaviour`, `cancelled`, `licence` (noun) and `license` (verb), and `-ize` endings such as `customize`. Keep American spellings where an API, crate, CSS property or protocol requires them (`color`, `background-color`).

- Use a real em dash (`—`) in prose, never `--`. A CLI flag such as `--check` is not prose.
- Sentence case in the UI. Title Case for window titles.
- Write tight. No padding, no restating the heading.

## Markdown formatting

**REQUIREMENT:** Do not hard-wrap markdown prose. Write each paragraph or bullet as one line and let the renderer reflow it. This applies to `README.md`, `SPEC.md`, `docs/` and pull request descriptions. Commit message bodies are the one exception: hard-wrap them at roughly 72 to 100 characters. Code comments wrap normally.

## Authoring voice

**REQUIREMENT:** Ship the result, not how it was arrived at. Write comments, identifiers, docs and pull request text as the author of the artifact, for the person who meets it later.

- No "this PR", reviewer names or commit hashes in comments or docs. State the fact or the reasoning directly.
- When a comment has been edited more than once, rewrite it as one clean explanation.
- Commit messages are the exception: they are the right place to record why a change happened.

## Commit messages

**Format:** Conventional Commits (`feat`, `fix`, `docs`, `test`, `ci`, `build`, `chore`, `refactor`, `perf`, `style`), with an optional scope.

- The body explains what and why, not how. Use markdown with **bold labels** and no headings. Hard-wrap.
- Backtick every code-level reference (identifiers, paths, config keys, flags, crate names). User-facing strings use quotes.
- End with a `Co-Authored-By:` trailer for the model that wrote the change.
- Write the message to a file with a single-quoted heredoc and commit with `git commit -F <file>`, so backticks and `$()` are not expanded. Check the result with `git log -1 --pretty=fuller`.

## Pull requests

- **Title:** capitalized, imperative, about 70 characters, no Conventional Commit prefix, outcome-focused.
- **Body:** `## Summary` with flat bullets that open with a **bold lead-in**, then `## Test plan` with `- [x]` or `- [ ]` items naming the commands run. Finish with what was not verified.
- **Labels:** one of `enhancement`, `bug`, `documentation`, `testing`, `ci`, `build`, `chore`, plus scope labels where useful.
- Open pull requests ready for review unless there is a stated blocker.
- Fix a bug with a test that fails before the fix, and state exactly what was and was not run.

## Git workflow

- `main` is the default branch. Work on topic branches (`feat/`, `fix/issue-<n>-…`, `docs/`, `chore/`, `ci/`, `build/`). Never commit to `main`.
- **Merge commits only.** No squash and no rebase merges.
- **Never** push, tag, publish or dispatch workflows unless asked.
- **Never** run `git add -A` or `git commit -a`, because checkouts may be shared. Stage explicit paths.
- Prefer the `gh` CLI for GitHub work.
- Planning files such as `PLAN.md` stay out of git.

## Local tooling

- **Toolchain:** pinned in `rust-toolchain.toml` (Rust 1.98.0, edition 2024).
- **Container:** the Fedora 45 image in `docker/` is the canonical build environment, with GTK 4.24, libadwaita 1.10, weston, blueprint, AppStream tools and nextest. `make image` builds it. The host also builds natively if GTK 4.20 and libadwaita 1.8 are installed.
- **Gate:** `make validate` is the single local gate and mirrors CI. It must pass before a pull request is opened or updated.
- **Editor settings:** `.editorconfig` is authoritative: tabs for Rust, TOML, Markdown, `.ui`, SVG, CSS, JS and HTML, spaces for YAML, tabs in the Makefile.
- Rust is formatted with rustfmt using hard tabs (see `rustfmt.toml`, which overrides the house "no rustfmt.toml" note for this project). Web UI assets (JS, CSS, HTML) also use tabs. Strictness comes from CI: `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings` and `cargo nextest run`.
- The Docker image stays plain OCI and never publishes ports. `--network host` is used only where documented (SSDP multicast and LAN testing).
- Do not run anything that opens a port to the internet. Test UPnP against the fake IGD and the Docker `miniupnpd` only.

## Licence headers

Every source, script, YAML, Dockerfile, SVG and `.ui` file starts with a one-line summary, a blank comment line and the copyright block:

```rust
// One-line summary of the file.
//
// (c) Copyright 2026 Liminal HQ, Scott Morris
// SPDX-License-Identifier: Apache-2.0 OR MIT
```

Use `#` comments for shell, YAML, TOML and Dockerfiles, and an XML comment for `.ui` and SVG. Markdown, JSON and lockfiles don't need one. `scripts/check-headers.sh` enforces it. The project is licensed `Apache-2.0 OR MIT`.

## Architecture rules

- **Core first.** `mailslot-core` has no GTK or UI dependency. It exposes an `Engine` (builder, explicit config, `start`, `stop`, `rotate_token`, `send`), a typed event stream and a state snapshot. That is the whole contract a frontend needs.
- **No hidden global state, no hidden runtime.** The engine takes a `tokio::runtime::Handle` and builds its own runtime only if asked.
- **Feature flags** `server`, `client`, `portmap` and `tls` let embedders pull in only what they use.
- **Frontends are thin.** `mailslot` (GTK) and `mailslot-cli` call the engine. Business logic does not live in them.
- **SPEC first.** A change to the HTTP contract, settings keys or defaults updates `SPEC.md` in the same change.
- `docs/architecture.md` has the layers and the guide to writing a new frontend. CI's `core-embed` job guards the boundary.
- Errors: `thiserror` enums in core, `anyhow` at the edges of binaries.

## GTK rules

- **GObjects stay on the main thread.** Never send a `glib::Object` across threads. Pass plain data to the main loop, as the engine's event stream does.
- **Never block the main loop.** No `block_on`, no sleeping, no synchronous I/O in a signal handler. Use `glib::spawn_future_local` to consume the engine's event stream.
- **All user-facing strings go through `gettext`.**
- **No colour literals outside `crates/mailslot-gtk/resources/style.css`**, and `style.css` uses only libadwaita named colours (`@accent_bg_color` and similar). `scripts/check-style.sh` fails otherwise. Brand colours belong to the icon, banner, About artwork and metainfo only.
- Prefer libadwaita widgets: `ToolbarView`, `PreferencesDialog`, `StatusPage`, `Banner`, `Toast`, `AlertDialog`, `ShortcutsDialog`. Symbolic icons come from the icon theme.
- Icon-only buttons need an accessible label.
- Composite templates: the resource path in `#[template(resource = "…")]` must match the gresource prefix, `#[template_child]` names must equal the XML ids, and custom widgets used in templates need `ensure_type()` first.
- Follow the system light, dark and high-contrast settings. Do not force a colour scheme.

## Async rules

- All I/O in core, server, client and portmap is `async` on tokio: `tokio::fs`, `tokio::net`, streaming bodies. The public API is async or returns streams.
- Clippy `disallowed-methods` in `clippy.toml` bans `std::fs::*`, `std::thread::sleep` and `reqwest::blocking`, and `await_holding_lock` is denied. Fix the code rather than adding an `allow`. A justified exception needs a comment saying why.
- Never hold a lock across an `.await`.
- CPU-heavy work such as hashing large buffers goes through `spawn_blocking` or is chunked so tasks stay responsive.
- Load tests prove that 50 concurrent uploads don't stall the server.

## Security rules

- Never log the token. Redact it in debug output and in URLs shown in logs.
- Identical `404` for everything outside the token. Do not add routes that behave differently without the token.
- Never serve files back from the destination.
- Never open, execute or extract received files.
- Compare secrets in constant time.
- Docs and UI describe UPnP exposure plainly.

## Testing

- Tests that need a display are exactly the `#[ignore = "needs display"]` ones under `crates/mailslot-gtk/tests/ui_*.rs`. `make test-ui` runs them under headless weston. Nothing else is ignored.
- `cargo nextest run --workspace` needs no display.
- `naming` has property tests. HTTP behaviour is covered by `tests/http.rs`. UPnP has a fake IGD in the core test suite.
- State exactly what was and was not run when reporting results.

## Documentation

- Update `SPEC.md` when behaviour changes, add or amend a `docs/decisions.md` entry when a decision is made, and update `docs/architecture.md` when structure changes. A change that alters behaviour without touching the doc that describes it is incomplete.
- `README.md` is for people who want to use Mailslot: install, first run, receiving, sending, security in plain words. Keep architecture and history out of it.
- Resolve an item in `docs/open-questions.md` by moving the answer into `docs/decisions.md` and deleting the question.
- `CLAUDE.md` is a summary of this file. Keep them in sync.

## Repository layout

```text
README.md SPEC.md AGENTS.md CLAUDE.md LICENSE-APACHE LICENSE-MIT
Cargo.toml  rust-toolchain.toml  deny.toml  clippy.toml  Makefile
assets/        hero banner
data/          desktop file, AppStream metainfo, icons, man page, completions
build-aux/     Flatpak manifest
packaging/     Arch PKGBUILD, AppImage recipe, deb and rpm metadata, CLI tarball recipe
docker/        Dockerfile, smoke-check.sh, run-gui.sh
scripts/       check and test scripts that Makefile and CI call
docs/          decisions, security, architecture, development, gtk-primer, packaging, open questions
.github/       workflows, templates, release configuration, Dependabot
crates/
  mailslot-core/  engine, protocol, server, client, config, portmap, netinfo
  mailslot-cli/   mailslot-cli binary
  mailslot-gtk/   GTK app, binary `mailslot`
```
