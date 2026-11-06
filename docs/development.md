# Development

## Requirements

- Docker (or Podman, since the image is plain OCI) and `make`. Everything else is in the image.
- Native builds also work with GTK 4.20 and libadwaita 1.8 or newer and the pinned Rust toolchain (`rust-toolchain.toml`).

## The dev image

`docker/Dockerfile` has a family of stages. `make image` builds `dev-gtk`.

| Stage | Base | Purpose |
|---|---|---|
| `ci-gtk` | Fedora 45 | Root image with GTK 4.24, libadwaita 1.10, Rust 1.98.0, nextest, cargo-deny, weston, Xvfb, blueprint, AppStream tools, shellcheck, miniupnpc. Used by CI |
| `dev-gtk` | `ci-gtk` | Same tools as a non-root user whose ids match yours |
| `upnp-test` | Fedora 45 | A miniupnpd gateway for `make upnp-test` |
| `pkg-musl` | `ci-gtk` | Static CLI build |
| `pkg-rpm` | `ci-gtk` | cargo-generate-rpm and rpmlint |
| `pkg-deb` | Ubuntu 26.04 | cargo-deb and lintian |
| `pkg-appimage` | Ubuntu 26.04 | linuxdeploy with the GTK plugin |

Stage order is cheapest first, because builders without BuildKit build every stage ahead of the target. The image never publishes ports. `docker/smoke-check.sh` (installed as `mailslot-smoke-check`) checks the toolchain versions.

Flatpak builds need bubblewrap privileges, so they run in the `flatpak-github-actions` image with `--privileged` (`make flatpak`).

## Make targets

Run `make help` for the list. The ones you will use most:

| Target | Does |
|---|---|
| `make image` | Build the dev image |
| `make shell` | Shell in the image with the repository at `/workspace` |
| `make fmt` / `make lint` | Format, or check formatting, clippy, headers, style, shell, Dockerfile, workflows and docs |
| `make test` | `cargo nextest run --workspace`, no display needed |
| `make test-ui` | Display tests under headless weston (`UI_BACKEND=x11` for Xvfb) |
| `make smoke` | `curl -T` and `mailslot-cli` round trips against a local headless server |
| `make upnp-test` | Port mapping against miniupnpd on isolated Docker networks |
| `make data-check` | Desktop file, AppStream, XML, icons, banner, the systemd unit and the architecture diagrams |
| `make pot` | Regenerate the translation template and merge it into the catalogues |
| `make validate` | Everything, the same as CI |
| `make run` | The GUI from the image on your Wayland session |
| `make deb`, `rpm`, `appimage`, `cli-tarball`, `packages`, `flatpak` | Packages into `dist/` |

Cargo caches live in the named volumes `mailslot-cargo-registry` and `mailslot-target`. The container's `target` is separate from the host's because the glibc versions differ. `make clean-volumes` removes them.

## Formatting

Rust is formatted with rustfmt using hard tabs (see `rustfmt.toml`). That overrides the house note about not having a `rustfmt.toml`. `make fmt` formats and `make fmt-check` checks. JavaScript, CSS and HTML for the web upload page, and TOML, Markdown, `.ui` and SVG files also use tabs. YAML uses spaces. `.editorconfig` is authoritative.

## Lint rules that need explaining

- `clippy.toml` bans blocking `std::fs` calls, `std::thread::sleep` and `reqwest::blocking`, and `clippy::await_holding_lock` is denied through `-D warnings`. Use `tokio::fs` and `tokio::time`. If a blocking call is genuinely needed (for example in `main` before the runtime exists), allow it at that call site with a comment saying why.
- `scripts/check-style.sh` fails on colour literals outside `crates/mailslot-gtk/resources/style.css`, and on anything in `style.css` other than semantic libadwaita named colours.
- `scripts/check-headers.sh` requires the SPDX header on every source, script, YAML, SVG, `.ui` and Dockerfile.

## Running the GUI

`make run` starts the app from the image on the host Wayland session (`make run-x11` for X11, which needs `xhost +SI:localuser:$USER` once).

- The container uses the host network, so a phone on your LAN can reach the dev server and LAN address detection sees the host interfaces. That exposes the dev server on the LAN, which is the point. `make run-isolated` uses a bridge network with nothing published, for UI-only work.
- `GDK_DEBUG=no-portals` is set, so GTK uses its built-in file chooser. The host's portal would return paths that don't exist inside the container. Test portal flows natively or in the Flatpak.
- Notifications in a container may be dropped, because `gio::Notification` on GNOME needs a matching installed desktop file. Check them with `make run-native` after `make install-dev-desktop`.
- If GPU rendering misbehaves, run with `GSK_RENDERER=cairo`.
- Received files go to `.dev-uploads/` in the repository, which is ignored by git.

`make run-native` runs `cargo run -p mailslot-gtk` with the host toolchain.

## Testing

- `make test` needs no display. It runs the HTTP tests, naming property tests, the fake IGD tests and the engine tests.
- Display tests are exactly the `#[ignore = "needs display"]` ones under `crates/mailslot-gtk/tests/ui_*.rs`. `scripts/test-ui.sh` starts `weston --backend=headless` inside `dbus-run-session` and runs `cargo nextest run -p mailslot-gtk --run-ignored only`.
- `scripts/smoke.sh` runs the core `serve` example on `127.0.0.1` with an ephemeral port, uploads with `curl` and `mailslot-cli`, and checks checksums, resume, dedupe, `404`, `413` and `422`.
- The nextest `ci` profile retries twice and writes JUnit XML to `target/nextest/ci/junit.xml`, which CI posts to the pull request.

### UPnP

`make upnp-test` creates two Docker bridges (with no masquerading and no published ports) and runs miniupnpd on them as a stand-in router. It first proves the gateway with `upnpc` (discover, add a mapping, list it, delete it), then runs the core tests named `miniupnpd_*` and marked `#[ignore = "needs gateway"]`. Those read the gateway address from `MAILSLOT_UPNP_GATEWAY` and the client address from `MAILSLOT_UPNP_LAN_IP`.

SSDP multicast works between containers on a user-defined bridge, so this test does not need `--network host`. Running the real app against SSDP on your own LAN does. Testing against a real router is manual and done with the maintainer present.

## Translations

Every user-facing string goes through `gettext`, and `.ui` strings are marked `translatable`. `po/POTFILES.in` lists the files that carry strings, and `po/LINGUAS` lists the languages with a catalogue. `make pot` runs `scripts/make-pot.sh`, which regenerates `po/mailslot.pot` with `xgettext`, merges it into each `po/<lang>.po` (creating stubs for new languages) and fails if a file with translatable strings is missing from `POTFILES.in`. `en_CA` and `fr` start as stubs.

## The daemon

`mailslotd` is the headless daemon from `mailslot-ipc`. Run it with `cargo run -p mailslot-ipc --bin mailslotd` and connect to its socket in `$XDG_RUNTIME_DIR/ca.liminalhq.mailslot/` (see the [specification](../SPEC.md#local-ipc-and-mailslotd)). To run it as a service, install `data/mailslotd.service` into `~/.config/systemd/user/` and run `systemctl --user enable --now mailslotd`. The deb, rpm and tarball packages ship the unit, and the Flatpak and AppImage do not.

## Checking the data files

`make data-check` validates the desktop file with `desktop-file-validate`, the AppStream metadata with `appstreamcli validate --no-net`, every XML, `.ui` and SVG file with `xmllint`, renders both icons, validates the banner and the architecture diagrams, and checks the shell completions.

The banner is `assets/hero.svg`. It must stay under 60 KB, contain no scripts or external references, and handle `prefers-color-scheme` and `prefers-reduced-motion`. The wordmark is outlined text between the `hero-text` markers.

### Architecture diagrams

The diagrams in `docs/diagrams/` are animated SVG with the same rules as the banner, and `scripts/check-diagrams.sh` checks them in two passes.

- **Static** (inside the dev image, from `check-data.sh`): `xmllint`, no scripts, `foreignObject`, images, external references or web fonts, at most 60 KB, the licence header, `<title>`, `<desc>`, `role="img"`, `viewBox`, `data-loop`, both media queries, tabs, and font stacks that end in `sans-serif` or `monospace`.
- **Text fit** (`scripts/diagram-fit.py` with `diagram-fit.js`): loads each diagram in headless Chromium, steps through one loop every quarter second in light, dark and reduced motion, measures every label with the native fonts and again with DejaVu Sans and Liberation Sans, and fails when a label comes within 4 px of its box, crosses another box, overlaps another label or sits on a line, when contrast drops below 4.5, when an animation is still visible under reduced motion, or when one jumps at the loop point. The dev image has no browser, so `make data-check` runs this pass on the host and skips it with a notice when Chromium is missing. CI runs it in the `diagrams` job with the runner's Chrome.

The checker relies on a few conventions:

- The root `<svg>` has `data-loop="<seconds>"`. Every animation repeats indefinitely, starts at 0 and lasts a whole fraction of the loop.
- A shape that holds text has an `id` and `data-box`, and each `<text>` inside it has `data-for="<id>"`. Connectors have `data-line`. The background has `data-bg`.
- Everything that moves sits in a `.motion` group that `prefers-reduced-motion: reduce` hides, and a `.static` group shows the finished frame instead.

To look at frames while drawing, write screenshots somewhere outside the repository:

```sh
python3 scripts/diagram-fit.py --shots /tmp/frames --times 0,2.5,5,7.5 docs/diagrams/upload-lifecycle.svg
```

## Continuous integration

`.github/workflows/ci.yml` runs one job per check and a final `required` job that branch protection should require. Jobs that need the toolchain run in `ghcr.io/liminal-hq/mailslot-ci`, which the `image` job builds and pushes from `main`. The workflows assume the repository `liminal-hq/mailslot`, and the first run on a new repository needs one `workflow_dispatch` of CI on `main` to publish the image.

`publish-test-results.yml` posts a single sticky comment on the pull request with counts, failing test names and each check's status. Lint the workflows locally with `make actionlint`.

## Troubleshooting

- **`make validate` fails on `smoke` or `upnp-test` before Rust code exists.** Both skip the Rust-dependent parts when the packages are absent and fail only when the binaries exist but misbehave.
- **Files in `target/` are owned by root.** The image runs as your user. Run `make image` again so the user ids match.
- **`xmllint` or `appstreamcli` not found.** Run the checks through `make`, which uses the image.
- **A phone can't reach the dev server.** Use `make run` (host network), check the interface shown on the link card and any host firewall.
