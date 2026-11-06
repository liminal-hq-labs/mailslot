# Dev, CI and packaging images for Mailslot: GTK 4, libadwaita and a pinned Rust toolchain.
#
# Stages: ci-gtk (root, tools in /usr/local) and dev-gtk (non-root user) on Fedora 45, plus
# pkg-deb (Ubuntu 26.04), pkg-rpm (Fedora), pkg-appimage (Ubuntu 26.04), pkg-musl (static CLI)
# and upnp-test (a miniupnpd gateway). The image is plain OCI, so it works under Podman too.
# No stage publishes a port. Stage order is cheapest first, because builders without BuildKit build
# every stage ahead of the target.
#
# (c) Copyright 2026 Liminal HQ, Scott Morris
# SPDX-License-Identifier: Apache-2.0 OR MIT

ARG FEDORA_VERSION=45
ARG FEDORA_DIGEST=sha256:5ad386251aa5d0111754dff91a930acc1a928afe7bafc46762024e096ab9bbb0
ARG UBUNTU_VERSION=26.04
ARG UBUNTU_DIGEST=sha256:6a06111c3b0a8600feadb270aec771dc239bf407b25625f93f44ab00e11f4d3c
ARG RUST_TOOLCHAIN=1.98.0
ARG CARGO_DENY_VERSION=0.20.2
ARG CARGO_DEB_VERSION=3.8.0
ARG CARGO_GENERATE_RPM_VERSION=0.21.1
ARG LINUXDEPLOY_VERSION=1-alpha-20250213-2
ARG LINUXDEPLOY_GTK_REF=7a3fbc31a9e5075073ff8790f26effbac5f84453

# CI image: everything `make validate` needs, installed system-wide as root.
FROM registry.fedoraproject.org/fedora:${FEDORA_VERSION}@${FEDORA_DIGEST} AS ci-gtk

SHELL ["/bin/bash", "-o", "pipefail", "-c"]

ARG RUST_TOOLCHAIN
ARG CARGO_DENY_VERSION

RUN dnf -y install --setopt=install_weak_deps=False \
        gcc gcc-c++ pkgconf-pkg-config git make which curl ca-certificates tar xz gzip findutils \
        gtk4-devel libadwaita-devel glib2-devel blueprint-compiler gettext \
        desktop-file-utils appstream libxml2 librsvg2-tools ShellCheck \
        weston xorg-x11-server-Xvfb dbus-daemon dbus-tools mesa-dri-drivers \
        adwaita-icon-theme google-noto-sans-fonts google-noto-sans-mono-fonts \
        procps-ng iproute miniupnpc \
    && dnf clean all

ENV RUSTUP_HOME=/usr/local/rustup \
    CARGO_HOME=/usr/local/cargo \
    PATH=/usr/local/cargo/bin:${PATH}

RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
        | sh -s -- -y --no-modify-path --profile minimal \
            --default-toolchain "${RUST_TOOLCHAIN}" --component clippy,rustfmt \
    && curl --proto '=https' --tlsv1.2 -LsSf https://get.nexte.st/latest/linux \
        | tar zxf - -C /usr/local/cargo/bin \
    && curl --proto '=https' --tlsv1.2 -LsSf \
        "https://github.com/EmbarkStudios/cargo-deny/releases/download/${CARGO_DENY_VERSION}/cargo-deny-${CARGO_DENY_VERSION}-x86_64-unknown-linux-musl.tar.gz" \
        | tar zxf - --strip-components=1 -C /usr/local/cargo/bin \
            "cargo-deny-${CARGO_DENY_VERSION}-x86_64-unknown-linux-musl/cargo-deny" \
    && chmod -R a+rX /usr/local/rustup /usr/local/cargo

COPY smoke-check.sh /usr/local/bin/mailslot-smoke-check
RUN chmod 0755 /usr/local/bin/mailslot-smoke-check

# Dev image: the same tools, run as a non-root user whose ids match the host's.
FROM ci-gtk AS dev-gtk

ARG USER_UID=1000
ARG USER_GID=1000

RUN groupadd -g "${USER_GID}" dev \
    && useradd -l -m -u "${USER_UID}" -g "${USER_GID}" dev \
    && chown -R dev:dev /usr/local/cargo /usr/local/rustup \
    && install -d -o dev -g dev /target /home/dev/.cargo/registry

USER dev
ENV CARGO_TARGET_DIR=/target
WORKDIR /workspace

# UPnP test gateway: miniupnpd on a Docker network, standing in for a router. It needs
# NET_ADMIN to manage its own iptables chains, and the test network is an internal bridge, so
# nothing is reachable from outside the host. scripts/upnp-test.sh drives it.
FROM registry.fedoraproject.org/fedora:${FEDORA_VERSION}@${FEDORA_DIGEST} AS upnp-test

SHELL ["/bin/bash", "-o", "pipefail", "-c"]

RUN dnf -y install --setopt=install_weak_deps=False \
        miniupnpd nftables which iproute procps-ng curl \
    && dnf clean all

COPY upnp-gateway.sh /usr/local/bin/upnp-gateway
RUN chmod 0755 /usr/local/bin/upnp-gateway

ENTRYPOINT ["/usr/local/bin/upnp-gateway"]

# Static CLI image: the musl target, for the `mailslot-cli` release tarball.
FROM ci-gtk AS pkg-musl

RUN dnf -y install --setopt=install_weak_deps=False musl-gcc musl-libc-static zip \
    && dnf clean all \
    && rustup target add x86_64-unknown-linux-musl

ENV CC_x86_64_unknown_linux_musl=musl-gcc \
    CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER=musl-gcc

WORKDIR /workspace

# rpm image: builds the rpm packages on Fedora and checks them with rpmlint.
FROM ci-gtk AS pkg-rpm

ARG CARGO_GENERATE_RPM_VERSION

RUN dnf -y install --setopt=install_weak_deps=False rpm-build rpmlint \
    && dnf clean all \
    && cargo install cargo-generate-rpm --version "${CARGO_GENERATE_RPM_VERSION}" --locked \
    && rm -rf /usr/local/cargo/registry /usr/local/cargo/git

WORKDIR /workspace

# deb image: Ubuntu 26.04 provides GTK 4.22 and libadwaita 1.9, so the package can depend on them.
FROM ubuntu:${UBUNTU_VERSION}@${UBUNTU_DIGEST} AS pkg-deb

SHELL ["/bin/bash", "-o", "pipefail", "-c"]

ARG DEBIAN_FRONTEND=noninteractive
ARG RUST_TOOLCHAIN
ARG CARGO_DEB_VERSION

RUN apt-get update && apt-get install -y --no-install-recommends \
        build-essential pkg-config curl ca-certificates git gettext xz-utils \
        libgtk-4-dev libadwaita-1-dev libglib2.0-dev dpkg-dev lintian \
    && rm -rf /var/lib/apt/lists/*

ENV RUSTUP_HOME=/usr/local/rustup \
    CARGO_HOME=/usr/local/cargo \
    PATH=/usr/local/cargo/bin:${PATH}

RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
        | sh -s -- -y --no-modify-path --profile minimal --default-toolchain "${RUST_TOOLCHAIN}" \
    && cargo install cargo-deb --version "${CARGO_DEB_VERSION}" --locked \
    && rm -rf /usr/local/cargo/registry /usr/local/cargo/git \
    && chmod -R a+rX /usr/local/rustup /usr/local/cargo

WORKDIR /workspace

# AppImage image: built on the oldest distribution that still provides GTK 4.20 and libadwaita 1.8,
# because the build host sets the glibc floor of the result. Runs linuxdeploy without FUSE.
FROM ubuntu:${UBUNTU_VERSION}@${UBUNTU_DIGEST} AS pkg-appimage

SHELL ["/bin/bash", "-o", "pipefail", "-c"]

ARG DEBIAN_FRONTEND=noninteractive
ARG RUST_TOOLCHAIN
ARG LINUXDEPLOY_VERSION
ARG LINUXDEPLOY_GTK_REF

RUN apt-get update && apt-get install -y --no-install-recommends \
        build-essential pkg-config curl ca-certificates git gettext file patchelf \
        desktop-file-utils appstream libxml2-utils squashfs-tools zsync \
        libgtk-4-dev libadwaita-1-dev libglib2.0-dev librsvg2-dev librsvg2-common \
        adwaita-icon-theme shared-mime-info gsettings-desktop-schemas \
        weston dbus mesa-libgallium \
    && rm -rf /var/lib/apt/lists/*

ENV RUSTUP_HOME=/usr/local/rustup \
    CARGO_HOME=/usr/local/cargo \
    PATH=/usr/local/cargo/bin:${PATH} \
    APPIMAGE_EXTRACT_AND_RUN=1 \
    NO_STRIP=1

RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
        | sh -s -- -y --no-modify-path --profile minimal --default-toolchain "${RUST_TOOLCHAIN}" \
    && curl --proto '=https' --tlsv1.2 -fsSL -o /usr/local/bin/linuxdeploy \
        "https://github.com/linuxdeploy/linuxdeploy/releases/download/${LINUXDEPLOY_VERSION}/linuxdeploy-x86_64.AppImage" \
    && curl --proto '=https' --tlsv1.2 -fsSL -o /usr/local/bin/linuxdeploy-plugin-gtk.sh \
        "https://raw.githubusercontent.com/linuxdeploy/linuxdeploy-plugin-gtk/${LINUXDEPLOY_GTK_REF}/linuxdeploy-plugin-gtk.sh" \
    && chmod 0755 /usr/local/bin/linuxdeploy /usr/local/bin/linuxdeploy-plugin-gtk.sh \
    && chmod -R a+rX /usr/local/rustup /usr/local/cargo

WORKDIR /workspace
