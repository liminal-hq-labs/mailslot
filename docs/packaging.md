# Packaging

Mailslot ships as two packages from one workspace: **`mailslot`** (the GTK app, the CLI, desktop file, icons, AppStream metadata) and **`mailslot-cli`** (the CLI alone, with no GTK dependency, for servers and agents).

| Format | Surface | Built by | Notes |
|---|---|---|---|
| Flatpak | GUI and CLI | `make flatpak` | `org.gnome.Platform//51`, in the flatpak-github-actions image with `--privileged`. Covers older distributions |
| AppImage | GUI and CLI | `make appimage` | linuxdeploy with the GTK plugin, bundles GTK 4 and libadwaita. `Mailslot.AppImage mailslot-cli …` runs the CLI. Embeds `gh-releases-zsync` update information |
| deb | `mailslot`, `mailslot-cli` | `make deb` | Built on Ubuntu 26.04 (GTK 4.22, libadwaita 1.9). Ubuntu 24.04 is not supported |
| rpm | `mailslot`, `mailslot-cli` | `make rpm` | Built on Fedora |
| Arch | `mailslot`, `mailslot-cli` | `packaging/arch/PKGBUILD` | Split package |
| CLI tarball | CLI | `make cli-tarball` | Static musl build with man page and completions |
| crates.io | `mailslot-core` | by hand | Publish-ready. Published only when the maintainer decides |

`make packages` builds deb, rpm, AppImage and the tarball into `dist/` with a `SHA256SUMS` file. The release workflow attaches everything to the GitHub release.

The `mailslot` package ships both binaries and declares `Provides`, `Conflicts` and `Replaces` for `mailslot-cli`, so the two never fight over `/usr/bin/mailslot-cli`.

## Verification

- `lintian` on each deb and `rpmlint` on each rpm, run by the package scripts.
- `scripts/test-package.sh deb|rpm` installs the package in a clean container of the target distribution and runs `mailslot --version` and `mailslot-cli --version`.
- `scripts/test-package.sh appimage` runs `--appimage-extract-and-run mailslot --version`, launches the app under headless weston, and validates the bundled metainfo.
- `scripts/test-package.sh cli-tarball` unpacks the tarball and runs `mailslot-cli --version` on a distribution without GTK.

## Cargo metadata

The package scripts read `[package.metadata.deb]` and `[package.metadata.generate-rpm]` from the crates' manifests. Paths are relative to each crate directory, and `target/release/…` resolves to the real target directory. Both crates also need the usual package fields (`description`, `license`, `repository`, `homepage`, `readme`, `keywords`, `categories`).

### `crates/mailslot-gtk/Cargo.toml`

```toml
[package.metadata.deb]
name = "mailslot"
maintainer = "Scott Morris <scott.moris@gmail.com>"
copyright = "2026, Liminal HQ, Scott Morris"
license-file = ["../../LICENSE-MIT", "0"]
section = "net"
priority = "optional"
depends = "$auto, libgtk-4-1 (>= 4.20), libadwaita-1-0 (>= 1.8)"
recommends = "xdg-desktop-portal"
provides = "mailslot-cli"
conflicts = "mailslot-cli"
replaces = "mailslot-cli"
extended-description = """\
Mailslot receives files through a private link. Start sharing, hand out the \
link, and files sent to it land in a folder with a SHA-256 checksum. Nothing \
is ever served back.
 .
It also sends: the Send page and the mailslot-cli command push files to \
another Mailslot, resume interrupted transfers and skip files the other side \
already has. Includes the mailslot-cli command."""
assets = [
	["target/release/mailslot", "usr/bin/", "755"],
	["target/release/mailslot-cli", "usr/bin/", "755"],
	["../../data/ca.liminalhq.mailslot.desktop", "usr/share/applications/", "644"],
	["../../data/ca.liminalhq.mailslot.metainfo.xml", "usr/share/metainfo/", "644"],
	["../../data/icons/hicolor/scalable/apps/ca.liminalhq.mailslot.svg", "usr/share/icons/hicolor/scalable/apps/", "644"],
	["../../data/icons/hicolor/symbolic/apps/ca.liminalhq.mailslot-symbolic.svg", "usr/share/icons/hicolor/symbolic/apps/", "644"],
	["../../data/man/mailslot-cli.1", "usr/share/man/man1/", "644"],
	["../../data/completions/mailslot-cli.bash", "usr/share/bash-completion/completions/mailslot-cli", "644"],
	["../../data/completions/mailslot-cli.fish", "usr/share/fish/vendor_completions.d/mailslot-cli.fish", "644"],
	["../../data/completions/_mailslot-cli", "usr/share/zsh/vendor-completions/_mailslot-cli", "644"],
	["../../LICENSE-APACHE", "usr/share/doc/mailslot/", "644"],
	["../../LICENSE-MIT", "usr/share/doc/mailslot/", "644"],
	["../../README.md", "usr/share/doc/mailslot/", "644"],
]

[package.metadata.generate-rpm]
name = "mailslot"
summary = "A private drop slot for files sent your way"
description = """\
Mailslot receives files through a private link. Start sharing, hand out the link, and files sent to it land in a folder with a SHA-256 checksum. Nothing is ever served back. It also sends: the Send page and the mailslot-cli command push files to another Mailslot, resume interrupted transfers and skip files the other side already has. Includes the mailslot-cli command."""
license = "Apache-2.0 OR MIT"
url = "https://github.com/liminal-hq/mailslot"
vendor = "Liminal HQ"
release = "1"
auto-req = "auto"
requires = { gtk4 = ">= 4.20", libadwaita = ">= 1.8" }
recommends = { xdg-desktop-portal = "*" }
conflicts = { mailslot-cli = "*" }
obsoletes = { mailslot-cli = "< 0.1.0" }
assets = [
	{ source = "target/release/mailslot", dest = "/usr/bin/mailslot", mode = "755" },
	{ source = "target/release/mailslot-cli", dest = "/usr/bin/mailslot-cli", mode = "755" },
	{ source = "../../data/ca.liminalhq.mailslot.desktop", dest = "/usr/share/applications/ca.liminalhq.mailslot.desktop", mode = "644" },
	{ source = "../../data/ca.liminalhq.mailslot.metainfo.xml", dest = "/usr/share/metainfo/ca.liminalhq.mailslot.metainfo.xml", mode = "644" },
	{ source = "../../data/icons/hicolor/scalable/apps/ca.liminalhq.mailslot.svg", dest = "/usr/share/icons/hicolor/scalable/apps/ca.liminalhq.mailslot.svg", mode = "644" },
	{ source = "../../data/icons/hicolor/symbolic/apps/ca.liminalhq.mailslot-symbolic.svg", dest = "/usr/share/icons/hicolor/symbolic/apps/ca.liminalhq.mailslot-symbolic.svg", mode = "644" },
	{ source = "../../data/man/mailslot-cli.1", dest = "/usr/share/man/man1/mailslot-cli.1", mode = "644", doc = true },
	{ source = "../../data/completions/mailslot-cli.bash", dest = "/usr/share/bash-completion/completions/mailslot-cli", mode = "644" },
	{ source = "../../data/completions/mailslot-cli.fish", dest = "/usr/share/fish/vendor_completions.d/mailslot-cli.fish", mode = "644" },
	{ source = "../../data/completions/_mailslot-cli", dest = "/usr/share/zsh/site-functions/_mailslot-cli", mode = "644" },
	{ source = "../../LICENSE-APACHE", dest = "/usr/share/licenses/mailslot/LICENSE-APACHE", mode = "644", doc = true },
	{ source = "../../LICENSE-MIT", dest = "/usr/share/licenses/mailslot/LICENSE-MIT", mode = "644", doc = true },
	{ source = "../../README.md", dest = "/usr/share/doc/mailslot/README.md", mode = "644", doc = true },
]
```

### `crates/mailslot-cli/Cargo.toml`

```toml
[package.metadata.deb]
name = "mailslot-cli"
maintainer = "Scott Morris <scott.moris@gmail.com>"
copyright = "2026, Liminal HQ, Scott Morris"
license-file = ["../../LICENSE-MIT", "0"]
section = "net"
priority = "optional"
depends = "$auto"
extended-description = """\
mailslot-cli sends files to a Mailslot and runs a headless receiver, with no \
GTK dependency. Uploads resume after a dropped connection, retry with backoff \
and skip files the receiver already has."""
assets = [
	["target/release/mailslot-cli", "usr/bin/", "755"],
	["../../data/man/mailslot-cli.1", "usr/share/man/man1/", "644"],
	["../../data/completions/mailslot-cli.bash", "usr/share/bash-completion/completions/mailslot-cli", "644"],
	["../../data/completions/mailslot-cli.fish", "usr/share/fish/vendor_completions.d/mailslot-cli.fish", "644"],
	["../../data/completions/_mailslot-cli", "usr/share/zsh/vendor-completions/_mailslot-cli", "644"],
	["../../LICENSE-APACHE", "usr/share/doc/mailslot-cli/", "644"],
	["../../LICENSE-MIT", "usr/share/doc/mailslot-cli/", "644"],
	["../../README.md", "usr/share/doc/mailslot-cli/", "644"],
]

[package.metadata.generate-rpm]
name = "mailslot-cli"
summary = "Send files to a Mailslot and receive them headless"
description = """\
mailslot-cli sends files to a Mailslot and runs a headless receiver, with no GTK dependency. Uploads resume after a dropped connection, retry with backoff and skip files the receiver already has."""
license = "Apache-2.0 OR MIT"
url = "https://github.com/liminal-hq/mailslot"
vendor = "Liminal HQ"
release = "1"
auto-req = "auto"
assets = [
	{ source = "target/release/mailslot-cli", dest = "/usr/bin/mailslot-cli", mode = "755" },
	{ source = "../../data/man/mailslot-cli.1", dest = "/usr/share/man/man1/mailslot-cli.1", mode = "644", doc = true },
	{ source = "../../data/completions/mailslot-cli.bash", dest = "/usr/share/bash-completion/completions/mailslot-cli", mode = "644" },
	{ source = "../../data/completions/mailslot-cli.fish", dest = "/usr/share/fish/vendor_completions.d/mailslot-cli.fish", mode = "644" },
	{ source = "../../data/completions/_mailslot-cli", dest = "/usr/share/zsh/site-functions/_mailslot-cli", mode = "644" },
	{ source = "../../LICENSE-APACHE", dest = "/usr/share/licenses/mailslot-cli/LICENSE-APACHE", mode = "644", doc = true },
	{ source = "../../LICENSE-MIT", dest = "/usr/share/licenses/mailslot-cli/LICENSE-MIT", mode = "644", doc = true },
	{ source = "../../README.md", dest = "/usr/share/doc/mailslot-cli/README.md", mode = "644", doc = true },
]
```

### `mailslot-core` for crates.io

```toml
[package]
description = "Engine, protocol, server, client and UPnP port mapping for Mailslot, a private file-drop slot"
readme = "README.md"
keywords = ["file-transfer", "upload", "upnp", "server", "client"]
categories = ["network-programming", "web-programming::http-server"]

[package.metadata.docs.rs]
all-features = true
rustdoc-args = ["--cfg", "docsrs"]
```

## Release notes

Release notes are generated from pull request labels (`.github/release.yml`), and `docs/releases/X.Y.Z.md` carries the hand-written summary that the AppStream `<release>` entry mirrors.
