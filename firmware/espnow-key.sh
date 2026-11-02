#!/usr/bin/env bash
# Prints the `espnow_key` line of an ESP-NOW leaf: the first 16 bytes of
# HMAC-SHA256(the gateway's espnow_secret, the leaf's device id), as the gateway computes it.
# The gateway is the `espnow_gateway` of boards/<leaf>.toml; its secret is in boards/<gateway>.toml.
# Usage: espnow-key.sh <leaf device id>
set -euo pipefail

firmware_dir="$(dirname "$(realpath "$0")")"
[ $# -eq 1 ] || { echo "usage: $0 <leaf device id>" >&2; exit 1; }
leaf=$1
value() { sed -n "s/^$1 = \"\(.*\)\"/\1/p" "$2"; }

gateway="$(value espnow_gateway "$firmware_dir/boards/$leaf.toml")"
[ -n "$gateway" ] || { echo "boards/$leaf.toml has no espnow_gateway" >&2; exit 1; }
secret="$(value espnow_secret "$firmware_dir/boards/$gateway.toml")"
[ -n "$secret" ] || { echo "boards/$gateway.toml has no espnow_secret" >&2; exit 1; }

key="$(printf '%s' "$leaf" | openssl dgst -sha256 -mac HMAC -macopt "hexkey:$secret" | sed 's/.*= //')"
echo "espnow_key = \"${key:0:32}\""
