#!/usr/bin/env bash
# Builds a release image per board for over-the-air updates: boards/<device id>.toml (the board's
# cfg.toml) becomes <images>/<device id>/<version>.bin, served by the hub ([ota] images).
# A `chip = "esp32c6"` line in the board's file picks the chip; esp32 without it. A
# `flash = "16mb"` line picks partitions-16mb.csv; partitions-4mb.csv without it.
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
    chip="$(sed -n 's/^chip = "\(.*\)"/\1/p' "boards/$device_id.toml")"
    chip="${chip:-esp32}"
    case "$chip" in
        esp32) target=xtensa-esp32-espidf ;;
        esp32c6) target=riscv32imac-esp-espidf ;;
        *) echo "$device_id: unknown chip \"$chip\"" >&2; exit 1 ;;
    esac
    flash="$(sed -n 's/^flash = "\(.*\)"/\1/p' "boards/$device_id.toml")"
    flash="${flash:-4mb}"
    table="partitions-$flash.csv"
    [ -f "$table" ] || { echo "$device_id: no partition table for flash \"$flash\"" >&2; exit 1; }
    slot=$(($(awk -F', *' '$1 == "ota_0" { print $5 }' "$table")))

    cp "boards/$device_id.toml" cfg.toml
    touch src/config.rs
    MCU="$chip" cargo build --release --target "$target"
    mkdir -p "$images/$device_id"
    output="$images/$device_id/$version.bin.partial"
    espflash save-image --chip "$chip" --flash-size "$flash" \
        "target/$target/release/firmware" "$output"
    size=$(stat -c %s "$output")
    if [ "$size" -gt "$slot" ]; then
        rm -f "$output"
        echo "$device_id: image of $size bytes does not fit the $slot-byte slot of $table" >&2
        exit 1
    fi
    if [ $((size * 10)) -gt $((slot * 9)) ]; then
        echo "$device_id: warning: the image fills $((size * 100 / slot))% of its slot" >&2
    fi
    mv "$output" "$images/$device_id/$version.bin"
    echo "$device_id: $images/$device_id/$version.bin ($((size * 100 / slot))% of the slot)"
done
