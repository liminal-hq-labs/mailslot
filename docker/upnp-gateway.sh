#!/usr/bin/env bash
# Entrypoint of the upnp-test image: runs miniupnpd as a fake router between two networks.
#
# (c) Copyright 2026 Liminal HQ, Scott Morris
# SPDX-License-Identifier: Apache-2.0 OR MIT

set -euo pipefail

# LAN_PREFIX and WAN_PREFIX are the first three octets of the two Docker networks the gateway is
# attached to (for example 10.77.1). miniupnpd listens on the LAN interface and maps ports on the
# WAN interface. Both networks are internal Docker bridges, so nothing leaves the host.

lan_prefix="${LAN_PREFIX:?LAN_PREFIX is required}"
wan_prefix="${WAN_PREFIX:?WAN_PREFIX is required}"

iface_for() {
	ip -o -4 addr show | awk -v p="$1." 'index($4, p) == 1 { print $2; exit }'
}

lan_if="$(iface_for "$lan_prefix")"
wan_if="$(iface_for "$wan_prefix")"
wan_ip="$(ip -o -4 addr show dev "$wan_if" | awk '{ sub("/.*", "", $4); print $4; exit }')"
lan_ip="$(ip -o -4 addr show dev "$lan_if" | awk '{ sub("/.*", "", $4); print $4; exit }')"
echo "gateway: lan ${lan_if} ${lan_ip}, wan ${wan_if} ${wan_ip}"

# Both interfaces are private, so tell miniupnpd not to refuse because the "external" address
# is not public. Mailslot's double NAT diagnosis is expected to report exactly that.
cat > /tmp/miniupnpd.conf <<CONF
ext_ifname=${wan_if}
ext_ip=${wan_ip}
ext_allow_private_ipv4=yes
listening_ip=${lan_if}
http_port=5000
enable_natpmp=yes
enable_upnp=yes
secure_mode=no
system_uptime=yes
uuid=11111111-2222-3333-4444-555555555555
allow 1024-65535 ${lan_prefix}.0/24 1024-65535
deny 0-65535 0.0.0.0/0 0-65535
CONF

cd /etc/miniupnpd
export IPV6=no
./nft_init.sh "$wan_if" "$wan_ip" 2>/dev/null || ./nft_init.sh || true

exec miniupnpd -d -f /tmp/miniupnpd.conf
