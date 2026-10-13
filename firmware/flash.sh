#!/usr/bin/env bash
# Flashes over USB with our bootloader (it does the OTA rollback) and partition table. Erasing
# otadata makes the board boot this image from ota_0 even after an OTA update went to ota_1.
# Usage: flash.sh [espflash options] <elf>   (also the cargo runner)
set -euo pipefail

image="${@: -1}"
options=("${@:1:$#-1}")
firmware_dir="$(dirname "$(realpath "$0")")"
bootloader="$(ls -t "$(dirname "$image")"/build/esp-idf-sys-*/out/build/bootloader/bootloader.bin | head -n 1)"

exec espflash flash \
    --bootloader "$bootloader" \
    --partition-table "$firmware_dir/partitions.csv" \
    --erase-parts otadata \
    "${options[@]}" "$image"
