#!/usr/bin/env bash
# UPnP test: Mailslot's port mapping against miniupnpd on an isolated Docker network.
#
# (c) Copyright 2026 Liminal HQ, Scott Morris
# SPDX-License-Identifier: Apache-2.0 OR MIT

set -euo pipefail

# Two isolated Docker bridges (no masquerading, no published ports, so nothing reaches the
# internet) stand in for a home network and a router uplink. A miniupnpd container is the gateway. A client container with
# the workspace mounted first proves the gateway with `upnpc`, then runs the miniupnpd-backed
# tests of mailslot-core when they exist. SSDP multicast works between containers on a user-defined
# bridge, so `--network host` is not needed here.
#
# Test contract with mailslot-core: tests named `miniupnpd_*`, marked `#[ignore = "needs gateway"]`,
# read the gateway's address from MAILSLOT_UPNP_GATEWAY and the LAN address from MAILSLOT_UPNP_LAN_IP.

cd "$(dirname "${BASH_SOURCE[0]}")/.."

suffix="$$"
lan="mailslot-upnp-lan-$suffix"
wan="mailslot-upnp-wan-$suffix"
gw="mailslot-upnp-gw-$suffix"
dev_image="${MAILSLOT_IMAGE:-mailslot-dev:local}"
gw_image="${MAILSLOT_UPNP_IMAGE:-mailslot-upnp-test:local}"
lan_prefix="${LAN_PREFIX:-10.77.1}"
wan_prefix="${WAN_PREFIX:-10.77.2}"
client_ip="${lan_prefix}.10"
gateway_ip="${lan_prefix}.2"

cleanup() {
	docker rm -f "$gw" >/dev/null 2>&1 || true
	docker network rm "$lan" "$wan" >/dev/null 2>&1 || true
}
trap cleanup EXIT

fail() {
	echo "upnp-test: FAIL: $*" >&2
	docker logs "$gw" 2>&1 | tail -n 30 >&2 || true
	exit 1
}

# The LAN bridge is not `--internal` because multicast needs a default route, but masquerading is
# off, so nothing from it is translated onto the host's uplink.
docker network create --subnet "${lan_prefix}.0/24" \
	-o com.docker.network.bridge.enable_ip_masquerade=false "$lan" >/dev/null
docker network create --internal --subnet "${wan_prefix}.0/24" "$wan" >/dev/null

docker create --name "$gw" --cap-add NET_ADMIN \
	--sysctl net.ipv4.ip_forward=1 \
	--network "$lan" --ip "$gateway_ip" \
	-e LAN_PREFIX="$lan_prefix" -e WAN_PREFIX="$wan_prefix" \
	"$gw_image" >/dev/null
docker network connect --ip "${wan_prefix}.2" "$wan" "$gw"
docker start "$gw" >/dev/null

for _ in $(seq 1 50); do
	docker logs "$gw" 2>&1 | grep -q 'HTTP listening on port' && break
	[ "$(docker inspect -f '{{.State.Running}}' "$gw")" = true ] || fail "gateway container exited"
	sleep 0.2
done
docker logs "$gw" 2>&1 | grep -q 'HTTP listening on port' || fail "miniupnpd did not start"
echo "upnp-test: gateway up at $gateway_ip"

client=(docker run --rm --user "$(id -u):$(id -g)" --network "$lan" --ip "$client_ip"
	-v "$PWD:/workspace" -v mailslot-cargo-registry:/home/dev/.cargo/registry -v mailslot-target:/target
	-e CARGO_TARGET_DIR=/target -e MAILSLOT_UPNP_GATEWAY="$gateway_ip" -e MAILSLOT_UPNP_LAN_IP="$client_ip"
	"$dev_image")

# 1. The gateway itself: discover it, add a mapping, see it, delete it. The script runs inside
# the container, so its variables stay literal.
# shellcheck disable=SC2016
"${client[@]}" bash -euo pipefail -c '
	upnpc -s | tee /tmp/status
	grep -q "ExternalIPAddress = " /tmp/status
	upnpc -a "$MAILSLOT_UPNP_LAN_IP" 8931 8931 TCP 60 | tee /tmp/add
	grep -q "is redirected to internal" /tmp/add
	upnpc -l | grep -q "8931->$MAILSLOT_UPNP_LAN_IP:8931"
	upnpc -d 8931 TCP
	! upnpc -l | grep -q "8931->"
' || fail "the gateway did not map and unmap a port"
echo "upnp-test: ok: miniupnpd maps and unmaps a TCP port"

# 2. Mailslot's own port mapping, when the core tests exist. The test network has no route to the
# internet, so the tests are built first on the default network (which can fetch crates) and then
# run offline on the isolated one.
if cargo metadata --no-deps --format-version 1 2>/dev/null | grep -q '"name":"mailslot-core"'; then
	docker run --rm --user "$(id -u):$(id -g)" -v "$PWD:/workspace" -w /workspace \
		-v mailslot-cargo-registry:/home/dev/.cargo/registry -v mailslot-target:/target \
		-e CARGO_TARGET_DIR=/target "$dev_image" \
		cargo nextest run -p mailslot-core --all-features --no-run || fail "could not build the core tests"
	"${client[@]}" env CARGO_NET_OFFLINE=true cargo nextest run -p mailslot-core --all-features --run-ignored only \
		-E 'test(/miniupnpd_/)' --no-tests=warn || fail "mailslot-core miniupnpd tests failed"
	echo "upnp-test: ok: mailslot-core tests against miniupnpd"
else
	echo "upnp-test: skipped mailslot-core tests (package not present)"
fi

echo "upnp-test: all checks passed"
