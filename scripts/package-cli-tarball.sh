#!/usr/bin/env bash
# Builds the static musl mailslot-cli tarball and checks that it really is static.
#
# (c) Copyright 2026 Liminal HQ, Scott Morris
# SPDX-License-Identifier: Apache-2.0 OR MIT

set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

version="$(sed -n "s/^version *= *\"\\(.*\\)\"/\\1/p" Cargo.toml | head -n1)"
[ -n "$version" ] || {
	echo "could not read the workspace version from Cargo.toml" >&2
	exit 1
}
mkdir -p dist

target=x86_64-unknown-linux-musl
name="mailslot-cli-${version}-x86_64-linux-musl"
stage="dist/${name}"

cargo build --release --locked --target "$target" -p mailslot-cli --no-default-features

built="${CARGO_TARGET_DIR:-target}/${target}/release/mailslot-cli"
if ldd "$built" 2>&1 | grep -qv 'not a dynamic executable\|statically linked'; then
	echo "package-cli-tarball: $built is not statically linked" >&2
	ldd "$built" >&2 || true
	exit 1
fi

rm -rf "$stage"
mkdir -p "$stage/completions" "$stage/man"
install -m 0755 "$built" "$stage/mailslot-cli"
install -m 0644 LICENSE-APACHE LICENSE-MIT "$stage/"
install -m 0644 data/man/mailslot-cli.1 "$stage/man/"
install -m 0644 data/completions/mailslot-cli.bash data/completions/mailslot-cli.fish data/completions/_mailslot-cli "$stage/completions/"
cat >"$stage/README.txt" <<TXT
mailslot-cli ${version}

A static build of the Mailslot command-line client and headless receiver. Copy mailslot-cli
to a directory on your PATH. Documentation: https://github.com/liminal-hq/mailslot
TXT

tar -C dist -czf "dist/${name}.tar.gz" "$name"
rm -rf "$stage"
echo "package-cli-tarball: dist/${name}.tar.gz"
