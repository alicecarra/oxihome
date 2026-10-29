#!/usr/bin/env bash
# Flashes over USB with our bootloader (it does the OTA rollback) and the partition table for the
# board's flash (`flash` in cfg.toml, 4mb when absent). Erasing otadata makes the board boot this
# image from ota_0 even after an OTA update went to ota_1.
# Usage: flash.sh [espflash options] <elf>   (also the cargo runner)
set -euo pipefail

image="${@: -1}"
options=("${@:1:$#-1}")
firmware_dir="$(dirname "$(realpath "$0")")"
bootloader="$(ls -t "$(dirname "$image")"/build/esp-idf-sys-*/out/build/bootloader/bootloader.bin | head -n 1)"

flash="$(sed -n 's/^flash = "\(.*\)"/\1/p' "$firmware_dir/cfg.toml")"
flash="${flash:-4mb}"
table="$firmware_dir/partitions-$flash.csv"
[ -f "$table" ] || { echo "no partition table for flash \"$flash\"" >&2; exit 1; }

port=()
for ((i = 0; i < ${#options[@]}; i++)); do
    case "${options[i]}" in
        -p | --port) port=(--port "${options[i + 1]}") ;;
        --port=*) port=("${options[i]}") ;;
    esac
done
# A table larger than the chip puts ota_1 past the end of the flash: the board would not boot.
detected="$(espflash board-info "${port[@]}" | sed -n 's/^Flash size: *\([0-9]*\) *MB.*/\1/p')"
if [ -z "$detected" ] || [ "$detected" -lt "${flash%mb}" ]; then
    echo "cfg.toml says flash = \"$flash\", the board has ${detected:-unknown} MB" >&2
    exit 1
fi

exec espflash flash \
    --bootloader "$bootloader" \
    --partition-table "$table" \
    --flash-size "$flash" \
    --erase-parts otadata \
    "${options[@]}" "$image"
