# Build and flash

## Firmware project

`firmware/` was generated from the official `esp-idf` template (already done, kept here for reference):

```sh
cargo generate esp-rs/esp-idf-template cargo --name firmware --vcs none -d mcu=esp32 -d advanced=false --silent
```

`cargo-generate` adds the new project to the root workspace; it was moved to `exclude` in the root
`Cargo.toml` because the firmware builds with the `esp` toolchain and the `xtensa-esp32-espidf` target.

Build (the first build downloads and compiles ESP-IDF into `firmware/.embuild`; it takes a while and
needs a few GB):

```sh
. ~/export-esp.sh
cd firmware
cargo build
```

## Flash and monitor

With the board on USB, from `firmware/`. The CH9102 adapter shows up as `/dev/ttyACM0` or `/dev/ttyACM1`
depending on the order devices were plugged in; the `/dev/serial/by-id/` path does not change:

```sh
./flash.sh --port /dev/serial/by-id/usb-1a86_* target/xtensa-esp32-espidf/release/firmware
espflash monitor --port /dev/serial/by-id/usb-1a86_*
```

Or build, flash and monitor in one step (uses the runner in `.cargo/config.toml`, which is
`flash.sh`):

```sh
cargo run --release
```

Always flash through `flash.sh`, not plain `espflash flash`. The flash has two app slots for
over-the-air updates, with one partition table per flash size, picked by `flash` in `cfg.toml`
(`4mb` without it): `partitions-4mb.csv` has `nvs`, `otadata`, `phy_init`, `ota_0` and `ota_1` of
1984 KiB each; `partitions-16mb.csv` (NanoESP32-C6) keeps the same first three, has slots of
4 MiB and a FAT `storage` partition of the remaining ~7.9 MiB, mounted by the firmware at
`/storage` (formatted on first boot). Before writing, the script reads the board's flash size
with `espflash board-info` and stops if the table needs more. The table and the bootloader only
change over the cable: an update over the air writes just the app. The script writes the table
and the bootloader of our build, which rolls back
a new image that does not mark itself valid (`CONFIG_BOOTLOADER_APP_ROLLBACK_ENABLE` in
`sdkconfig.defaults`); `espflash` alone writes its own bootloader and its single-app table. The
script also erases `otadata`, so the board boots the flashed image from `ota_0` even if an update
had moved it to `ota_1`. Images go over the air in release; debug ones may not fit a 4 MB
board's slot.

## Firmware configuration

The firmware reads the Wi-Fi credentials and the broker address at compile time from
`firmware/cfg.toml`, which is ignored by git:

```sh
cp firmware/cfg.toml.example firmware/cfg.toml
chmod 600 firmware/cfg.toml
```

`mqtt_url` must use the LAN address of the machine running the broker: `mqtt://192.168.15.100:1883` (docker-host).
`mqtt_password` is the board's own password; its user name is its device id (`esp32-<mac>`, printed
by `espflash` as the MAC address), so each board needs its own build. Empty connects anonymously.
`peripherals` lists what is wired to the board besides the onboard LED and BOOT button (`climate`,
`external_led`, `buzzer`, `ir`, `potentiometer`); the firmware declares and drives only those, with
the entity numbers of the [test board's table](boards.md#test-board-wiring). Without the line, all of them; `""` for none. An unknown
name is an error on the serial log at boot, and the board restarts 10 s later (as with any startup
error).

Without a network the board keeps running: the Wi-Fi reconnects in the background (every
`WifiCheckInterval`, without blocking the loop, so the button still toggles its output), messages
that cannot be queued are dropped and logged, and the hub asks for the state again when the board
is back. A task watchdog restarts the board if its loop stops for 120 s.
