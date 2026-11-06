# Developer commands for Mailslot. `make validate` is the single gate and mirrors CI.
#
# (c) Copyright 2026 Liminal HQ, Scott Morris
# SPDX-License-Identifier: Apache-2.0 OR MIT

SHELL := /bin/bash
.DEFAULT_GOAL := help

UID := $(shell id -u)
GID := $(shell id -g)
IMAGE ?= mailslot-dev:local
TTY := $(shell [ -t 0 ] && echo -it)

# Pinned third-party images used for linting. Override to try a newer one.
HADOLINT_IMAGE ?= hadolint/hadolint:latest
ACTIONLINT_IMAGE ?= rhysd/actionlint:latest
SHELLCHECK_IMAGE ?= koalaman/shellcheck:stable
MARKDOWNLINT_IMAGE ?= davidanson/markdownlint-cli2:v0.17.2
FLATPAK_IMAGE ?= ghcr.io/flathub-infra/flatpak-github-actions:gnome-51

# Runs a command in the dev image with the repository mounted. Caches live in named volumes, and
# the container's cargo target directory is separate from the host's `target/` (different glibc).
# No port is published.
IN_IMAGE = docker run --rm $(TTY) --user $(UID):$(GID) \
	-v "$(CURDIR):/workspace" -w /workspace \
	-v mailslot-cargo-registry:/home/dev/.cargo/registry \
	-v mailslot-target:/target -e CARGO_TARGET_DIR=/target \
	$(IMAGE)

# Package images run as the caller so files in dist/ stay owned by the caller.
PKG_RUN = docker run --rm --user $(UID):$(GID) -e HOME=/tmp -e CARGO_HOME=/tmp/cargo \
	-e RUSTUP_TOOLCHAIN=1.98.0 -e CARGO_TARGET_DIR=/tmp/target \
	-v "$(CURDIR):/workspace" -w /workspace

SCRIPTS := $(wildcard scripts/*.sh docker/*.sh)

.PHONY: help
help: ## List the targets
	@grep -hE '^[a-zA-Z_-]+:.*## ' $(MAKEFILE_LIST) | sort | awk -F':.*## ' '{printf "  %-18s %s\n", $$1, $$2}'

# Image ------------------------------------------------------------------------------------------

.PHONY: image
image: ## Build the dev image (Fedora 45, GTK 4, libadwaita)
	docker build --target dev-gtk --build-arg USER_UID=$(UID) --build-arg USER_GID=$(GID) -t $(IMAGE) docker/

.PHONY: shell
shell: ## Interactive shell in the dev image
	$(IN_IMAGE) bash

# Build and test ---------------------------------------------------------------------------------

.PHONY: build
build: ## cargo build --workspace
	$(IN_IMAGE) cargo build --workspace

.PHONY: check
check: ## cargo check --workspace --all-targets
	$(IN_IMAGE) cargo check --workspace --all-targets

.PHONY: fmt
fmt: ## Format Rust with rustfmt (hard tabs, see rustfmt.toml)
	$(IN_IMAGE) cargo fmt --all

.PHONY: fmt-check
fmt-check: ## cargo fmt --all -- --check
	$(IN_IMAGE) cargo fmt --all -- --check

.PHONY: clippy
clippy: ## cargo clippy -D warnings (includes the async-safety lints in clippy.toml)
	$(IN_IMAGE) cargo clippy --workspace --all-targets -- -D warnings

.PHONY: headers
headers: ## Check SPDX headers
	bash scripts/check-headers.sh

.PHONY: shellcheck
shellcheck: ## shellcheck on every script
	docker run --rm -v "$(CURDIR):/mnt" -w /mnt $(SHELLCHECK_IMAGE) $(SCRIPTS)

.PHONY: hadolint
hadolint: ## hadolint on docker/Dockerfile
	docker run --rm -i -v "$(CURDIR)/.hadolint.yaml:/.hadolint.yaml:ro" $(HADOLINT_IMAGE) hadolint --config /.hadolint.yaml - < docker/Dockerfile

.PHONY: actionlint
actionlint: ## actionlint on the workflows
	docker run --rm -v "$(CURDIR):/repo" -w /repo $(ACTIONLINT_IMAGE) -color

.PHONY: docs-lint
docs-lint: ## markdownlint on every tracked or new Markdown file (git-ignored files are skipped)
	files="$$(git ls-files --cached --others --exclude-standard '*.md')"; \
	docker run --rm -v "$(CURDIR):/workspace" -w /workspace $(MARKDOWNLINT_IMAGE) $$files

.PHONY: lint
lint: fmt-check clippy headers style shellcheck hadolint actionlint docs-lint ## Everything that checks style

.PHONY: test
test: ## cargo nextest run --workspace (no display needed)
	$(IN_IMAGE) cargo nextest run --workspace

.PHONY: test-ci
test-ci: ## nextest with the ci profile (writes target/nextest/ci/junit.xml)
	$(IN_IMAGE) cargo nextest run --workspace --profile ci

.PHONY: test-ui
test-ui: ## Display tests under headless weston (UI_BACKEND=x11 for Xvfb)
	$(IN_IMAGE) env UI_BACKEND=$${UI_BACKEND:-wayland} bash scripts/test-ui.sh

.PHONY: screenshots
screenshots: ## Capture docs/screenshots/*.png from the UI test under headless weston
	mkdir -p docs/screenshots
	$(IN_IMAGE) env MAILSLOT_SCREENSHOT_DIR=/workspace/docs/screenshots bash scripts/test-ui.sh -E 'test(screenshots)'

.PHONY: smoke
smoke: ## curl and CLI round trips against a local headless server
	$(IN_IMAGE) bash scripts/smoke.sh

.PHONY: upnp-test
upnp-test: image upnp-image ## Port mapping against miniupnpd on an isolated Docker network
	bash scripts/upnp-test.sh

.PHONY: upnp-image
upnp-image: ## Build the miniupnpd gateway image
	docker build --target upnp-test -t mailslot-upnp-test:local docker/

.PHONY: pot
pot: ## Regenerate po/mailslot.pot and merge it into the catalogues
	$(IN_IMAGE) bash scripts/make-pot.sh

.PHONY: deny
deny: ## cargo deny check
	$(IN_IMAGE) cargo deny check

.PHONY: style
style: ## No colour literals outside style.css, named colours only
	bash scripts/check-style.sh

.PHONY: data-check
data-check: ## Desktop file, AppStream, XML and SVG validity, banner and icons
	$(IN_IMAGE) bash scripts/check-data.sh

.PHONY: validate
validate: lint test test-ui smoke upnp-test data-check deny ## The single gate, same as CI
	@echo "make validate: all gates passed"

# Running ----------------------------------------------------------------------------------------

.PHONY: run
run: ## GUI from the dev image on the host Wayland session (uses the host network)
	docker/run-gui.sh wayland

.PHONY: run-x11
run-x11: ## GUI from the dev image on the host X11 session
	docker/run-gui.sh x11

.PHONY: run-isolated
run-isolated: ## GUI on a bridge network with no host networking (UI-only work)
	ISOLATED=1 docker/run-gui.sh wayland

.PHONY: run-native
run-native: ## GUI with the host toolchain
	cargo run -p mailslot-gtk

.PHONY: install-dev-desktop
install-dev-desktop: ## Install the desktop file and icons into ~/.local/share (for notifications)
	install -Dm644 data/ca.liminalhq.mailslot.desktop $(HOME)/.local/share/applications/ca.liminalhq.mailslot.desktop
	install -Dm644 data/icons/hicolor/scalable/apps/ca.liminalhq.mailslot.svg $(HOME)/.local/share/icons/hicolor/scalable/apps/ca.liminalhq.mailslot.svg
	install -Dm644 data/icons/hicolor/symbolic/apps/ca.liminalhq.mailslot-symbolic.svg $(HOME)/.local/share/icons/hicolor/symbolic/apps/ca.liminalhq.mailslot-symbolic.svg

# Packaging --------------------------------------------------------------------------------------

.PHONY: flatpak
flatpak: ## Build the Flatpak (needs --privileged for bubblewrap), output in build-aux/flatpak/repo
	docker run --rm --privileged -v "$(CURDIR):/workspace" -w /workspace $(FLATPAK_IMAGE) bash scripts/package-flatpak.sh

.PHONY: deb
deb: ## Build mailslot and mailslot-cli .deb packages on Ubuntu 26.04 into dist/
	docker build --target pkg-deb -t mailslot-pkg-deb:local docker/
	$(PKG_RUN) mailslot-pkg-deb:local bash scripts/package-deb.sh

.PHONY: rpm
rpm: ## Build mailslot and mailslot-cli .rpm packages on Fedora into dist/
	docker build --target pkg-rpm -t mailslot-pkg-rpm:local docker/
	$(PKG_RUN) mailslot-pkg-rpm:local bash scripts/package-rpm.sh

.PHONY: appimage
appimage: ## Build the AppImage on Ubuntu 26.04 into dist/
	docker build --target pkg-appimage -t mailslot-pkg-appimage:local docker/
	$(PKG_RUN) mailslot-pkg-appimage:local bash scripts/package-appimage.sh

.PHONY: cli-tarball
cli-tarball: ## Build the static musl mailslot-cli tarball into dist/
	docker build --target pkg-musl -t mailslot-pkg-musl:local docker/
	$(PKG_RUN) mailslot-pkg-musl:local bash scripts/package-cli-tarball.sh

.PHONY: packages
packages: deb rpm appimage cli-tarball ## Every package except the Flatpak
	cd dist && find . -maxdepth 1 -type f ! -name SHA256SUMS -printf '%P\n' | sort | xargs sha256sum > SHA256SUMS

.PHONY: clean-volumes
clean-volumes: ## Remove the cargo and target volumes
	-docker volume rm mailslot-cargo-registry mailslot-target
