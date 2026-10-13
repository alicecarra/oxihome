#!/usr/bin/env bash
# Builds a release image per board for over-the-air updates: boards/<device id>.toml (the board's
# cfg.toml) becomes <images>/<device id>/<version>.bin, served by the hub ([ota] images).
# Usage: build-images.sh [device id...]   (all boards when none given)
set -euo pipefail
umask 077

firmware_dir="$(dirname "$(realpath "$0")")"
images="${OXIHOME_IMAGES:-$firmware_dir/../hub/firmware}"
cd "$firmware_dir"

version="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)"
if [ $# -eq 0 ]; then
    set -- $(ls boards/*.toml | xargs -n 1 basename | sed 's/\.toml$//')
fi

backup="$(mktemp)"
cp cfg.toml "$backup"
trap 'cp "$backup" cfg.toml; rm -f "$backup"' EXIT

for device_id in "$@"; do
    cp "boards/$device_id.toml" cfg.toml
    touch src/config.rs
    cargo build --release
    mkdir -p "$images/$device_id"
    espflash save-image --chip esp32 target/xtensa-esp32-espidf/release/firmware \
        "$images/$device_id/$version.bin"
    echo "$device_id: $images/$device_id/$version.bin"
done
