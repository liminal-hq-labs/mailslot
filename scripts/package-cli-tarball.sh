#!/usr/bin/env bash
# Builds the static musl mailslot-cli tarball and checks that it really is static.
#
# (c) Copyright 2026 Liminal HQ, Scott Morris
# SPDX-License-Identifier: Apache-2.0 OR MIT

# `version` and `ipc_pkg` come from package-common.sh.
# shellcheck disable=SC2154
set -euo pipefail

# shellcheck source=/dev/null
. "$(dirname "${BASH_SOURCE[0]}")/package-common.sh"

target=x86_64-unknown-linux-musl
name="mailslot-cli-${version}-x86_64-linux-musl"
stage="dist/${name}"

cargo build --release --locked --target "$target" -p mailslot-cli "${ipc_pkg[@]}" --no-default-features

release="${CARGO_TARGET_DIR:-target}/${target}/release"
built="$release/mailslot-cli"
binaries=("$built")
[ -x "$release/mailslotd" ] && binaries+=("$release/mailslotd")
for bin in "${binaries[@]}"; do
	if ldd "$bin" 2>&1 | grep -qv 'not a dynamic executable\|statically linked'; then
		echo "package-cli-tarball: $bin is not statically linked" >&2
		ldd "$bin" >&2 || true
		exit 1
	fi
done

rm -rf "$stage"
mkdir -p "$stage/completions" "$stage/man"
install -m 0755 "$built" "$stage/mailslot-cli"
if [ -x "$release/mailslotd" ]; then
	install -m 0755 "$release/mailslotd" "$stage/mailslotd"
	install -m 0644 data/mailslotd.service "$stage/mailslotd.service"
fi
install -m 0644 LICENSE-APACHE LICENSE-MIT "$stage/"
install -m 0644 data/man/mailslot-cli.1 "$stage/man/"
install -m 0644 data/completions/mailslot-cli.bash data/completions/mailslot-cli.fish data/completions/_mailslot-cli "$stage/completions/"
cat >"$stage/README.txt" <<TXT
mailslot-cli ${version}

A static build of the Mailslot command-line client and headless receiver, with the mailslotd
daemon and its systemd user unit when present. Copy the binaries to a directory on your PATH, and
mailslotd.service to ~/.config/systemd/user/. Documentation: https://github.com/liminal-hq-labs/mailslot
TXT

tar -C dist -czf "dist/${name}.tar.gz" "$name"
rm -rf "$stage"
echo "package-cli-tarball: dist/${name}.tar.gz"
