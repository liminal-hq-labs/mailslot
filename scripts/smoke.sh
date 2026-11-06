#!/usr/bin/env bash
# Round-trip smoke test: curl and mailslot-cli against a local headless server.
#
# (c) Copyright 2026 Liminal HQ, Scott Morris
# SPDX-License-Identifier: Apache-2.0 OR MIT

set -euo pipefail

# Starts the core `serve` example on 127.0.0.1 with an ephemeral port (nothing is published), uploads
# files with curl and mailslot-cli, and checks the checksum, resume, dedupe and error paths.

cd "$(dirname "${BASH_SOURCE[0]}")/.."

target="${CARGO_TARGET_DIR:-target}"
work="$(mktemp -d)"
dest="$work/dest"
mkdir -p "$dest"
server_pid=""

cleanup() {
	[ -n "$server_pid" ] && kill "$server_pid" 2>/dev/null || true
	rm -rf "$work"
}
trap cleanup EXIT

fail() {
	echo "smoke: FAIL: $*" >&2
	exit 1
}
pass() { echo "smoke: ok: $*"; }

cargo build -q -p mailslot-core --example serve
serve_bin="$target/debug/examples/serve"
[ -x "$serve_bin" ] || fail "serve example not found at $serve_bin"

"$serve_bin" --bind 127.0.0.1:0 --dir "$dest" --max-mb 1 >"$work/serve.out" 2>"$work/serve.err" &
server_pid=$!

url=""
for _ in $(seq 1 100); do
	url="$(sed -n 's/^URL \(.*\)$/\1/p' "$work/serve.out" | head -n1)"
	[ -n "$url" ] && break
	kill -0 "$server_pid" 2>/dev/null || fail "server exited early: $(cat "$work/serve.err")"
	sleep 0.1
done
[ -n "$url" ] || fail "server never printed its URL"
base="${url%/}"
root="${base%/*}"
token="${base##*/}"
pass "server listening at ${root}/<token>/"

head -c 300000 /dev/urandom >"$work/a.bin"
want="$(sha256sum "$work/a.bin" | cut -d' ' -f1)"

# 1. PUT returns 201 and the checksum curl can compare with sha256sum.
hdrs="$(curl -sS -D - -o /dev/null -T "$work/a.bin" "$base/a.bin")"
echo "$hdrs" | head -n1 | grep -q ' 201' || fail "PUT did not return 201: $hdrs"
got="$(echo "$hdrs" | tr -d '\r' | sed -n 's/^[Xx]-[Mm]ailslot-[Ss]ha256: //p')"
[ "$got" = "$want" ] || fail "checksum header $got differs from sha256sum $want"
[ "$(sha256sum "$dest/a.bin" | cut -d' ' -f1)" = "$want" ] || fail "saved file differs"
pass "curl -T saved a.bin with a matching SHA-256"

# 2. POST alias.
code="$(curl -sS -o /dev/null -w '%{http_code}' -X POST --data-binary @"$work/a.bin" "$base/b.bin")"
[ "$code" = 201 ] || fail "POST returned $code"
pass "POST alias"

# 3. Same name again is never overwritten.
curl -sS -o /dev/null -T "$work/a.bin" "$base/a.bin"
[ "$(find "$dest" -maxdepth 1 -name 'a*.bin' | wc -l)" -ge 2 ] || fail "second upload overwrote the first"
pass "name collision keeps both files"

# 4. Wrong token and unknown paths are identical 404s.
for path in "/nope/a.bin" "/" "/$token-x/"; do
	code="$(curl -sS -o /dev/null -w '%{http_code}' -T "$work/a.bin" "${root}${path}")"
	[ "$code" = 404 ] || fail "${path} returned $code, want 404"
done
pass "bad token and unknown paths return 404"

# 5. Over the size cap.
head -c 2000000 /dev/zero >"$work/big.bin"
code="$(curl -sS -o /dev/null -w '%{http_code}' -T "$work/big.bin" "$base/big.bin")"
[ "$code" = 413 ] || fail "oversize upload returned $code, want 413"
[ ! -e "$dest/big.bin" ] || fail "oversize upload left a file"
pass "413 over the size cap"

# 6. Traversal names stay inside the destination.
curl -sS -o /dev/null -T "$work/a.bin" "$base/..%2f..%2fescape.bin" || true
[ ! -e "$work/escape.bin" ] && [ ! -e "$dest/../escape.bin" ] || fail "traversal escaped the destination"
pass "traversal name contained"

# 7. Health reports capabilities.
health="$(curl -sS "$base/health")"
echo "$health" | grep -q '"ok":true' || fail "health: $health"
echo "$health" | grep -q 'resume' || fail "health does not advertise resume: $health"
pass "health reports capabilities"

# 8. Expected digest mismatch is a 422 and keeps nothing.
code="$(curl -sS -o /dev/null -w '%{http_code}' -H 'X-Mailslot-Expected-Sha256: 0000000000000000000000000000000000000000000000000000000000000000' -T "$work/a.bin" "$base/mismatch.bin")"
[ "$code" = 422 ] || fail "digest mismatch returned $code, want 422"
[ ! -e "$dest/mismatch.bin" ] || fail "mismatched upload left a file"
pass "422 on digest mismatch"

# 9. Resume: send the first part with an expected digest, ask for the offset, send the rest.
total="$(stat -c %s "$work/a.bin")"
head -c 100000 "$work/a.bin" >"$work/part1"
tail -c +100001 "$work/a.bin" >"$work/part2"
code="$(curl -sS -o /dev/null -w '%{http_code}' -X PUT -H "X-Mailslot-Expected-Sha256: $want" -H "X-Mailslot-Upload-Id: smoke-resume-1" -H "Content-Range: bytes 0-99999/$total" --data-binary @"$work/part1" "$base/resumed.bin")"
[ "$code" = 202 ] || fail "first range returned $code, want 202"
offset="$(curl -sS -I -H "X-Mailslot-Upload-Id: smoke-resume-1" "$base/resumed.bin" | tr -d '\r' | sed -n 's/^[Xx]-[Mm]ailslot-[Oo]ffset: //p')"
[ "$offset" = 100000 ] || fail "HEAD offset was '$offset', want 100000"
hdrs="$(curl -sS -D - -o /dev/null -X PUT -H "X-Mailslot-Expected-Sha256: $want" -H "X-Mailslot-Upload-Id: smoke-resume-1" -H "Content-Range: bytes 100000-$((total - 1))/$total" --data-binary @"$work/part2" "$base/resumed.bin")"
echo "$hdrs" | head -n1 | grep -q ' 201' || fail "final range did not return 201: $hdrs"
got="$(echo "$hdrs" | tr -d '\r' | sed -n 's/^[Xx]-[Mm]ailslot-[Ss]ha256: //p')"
[ "$got" = "$want" ] || fail "resumed checksum $got differs from $want"
pass "resume: 202, HEAD offset, 201 with the whole-file SHA-256"

# 10. Dedupe: the server now holds this digest.
have="$(curl -sS "$base/have?sha256=$want")"
echo "$have" | grep -q '"have":true' || fail "have: $have"
pass "have?sha256 reports the file"

# 11. mailslot-cli, when the package exists.
if cargo metadata --no-deps --format-version 1 2>/dev/null | grep -q '"name":"mailslot-cli"'; then
	cargo build -q -p mailslot-cli
	cli="$target/debug/mailslot-cli"
	head -c 150000 /dev/urandom >"$work/c.bin"
	cwant="$(sha256sum "$work/c.bin" | cut -d' ' -f1)"
	"$cli" send "$work/c.bin" "mailslot://${base#http://}" >"$work/cli1.out" 2>&1 || fail "mailslot-cli send failed: $(cat "$work/cli1.out")"
	[ "$(find "$dest" -maxdepth 1 -name 'c*.bin' -exec sha256sum {} + | grep -c "$cwant")" -eq 1 ] || fail "CLI upload missing or wrong"
	"$cli" send "$work/c.bin" "mailslot://${base#http://}" >"$work/cli2.out" 2>&1 || fail "second mailslot-cli send failed"
	[ "$(find "$dest" -maxdepth 1 -name 'c*.bin' | wc -l)" -eq 1 ] || fail "CLI did not skip the duplicate"
	pass "mailslot-cli send, and dedupe on the second send"
else
	echo "smoke: skipped mailslot-cli checks (package not present)"
fi

echo "smoke: all checks passed"
