# Setup

Oxihome development environment. Tested on Arch Linux.

## ESP32 toolchain

Rust firmware on `esp-idf` (std) for the ESP32-WROOM-32 (Xtensa).

Build, flash and project generation tools:

```sh
cargo install espup espflash ldproxy cargo-generate
```

- `espup`: installs and updates the Rust toolchain for Xtensa.
- `espflash`: flashes the firmware and opens the serial monitor.
- `ldproxy`: linker wrapper used by the `esp-idf` build.
- `cargo-generate`: creates projects from templates.

Xtensa toolchain (generates `~/export-esp.sh`):

```sh
espup install
```

Load the toolchain environment in every shell that builds the firmware:

```sh
. ~/export-esp.sh
```

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
espflash flash --port /dev/serial/by-id/usb-1a86_* target/xtensa-esp32-espidf/debug/firmware
espflash monitor --port /dev/serial/by-id/usb-1a86_*
```

Or build, flash and monitor in one step (uses the runner in `.cargo/config.toml`):

```sh
cargo run
```

## Firmware configuration

The firmware reads the Wi-Fi credentials and the broker address at compile time from
`firmware/cfg.toml`, which is ignored by git:

```sh
cp firmware/cfg.toml.example firmware/cfg.toml
```

`mqtt_url` must use the LAN address of the machine running the broker, e.g. `mqtt://192.168.15.10:1883`.

## MQTT broker

Mosquitto 2 in Docker, anonymous access on port 1883 (see `infra/`):

```sh
docker compose -f infra/compose.yaml up -d
```

## Hub

Connects to the broker configured in `hub/hub.toml`, asks every node for its `Hello` and `State`,
tracks which nodes are online (offline after 90 s without messages) and serves a web interface
(Topcoat) on the `[http]` address, by default <http://127.0.0.1:8080>:

```sh
cargo run -p oxihome-hub -- hub/hub.toml
```

Only one hub can run per broker: both use the MQTT client id `oxihome-hub` and keep disconnecting
each other.

The page's buttons post to `/set`; the same request from the command line:

```sh
curl -X POST -d "device_id=esp32-08d1f9ca41e4&entity=1&channel=0&value=true" localhost:8080/set
```

## Client

Talks to nodes over MQTT or the USB serial port. Type `help` for the commands, e.g. `set 1 0 true`
turns the LED on.

```sh
cargo run -p oxihome-client -- mqtt localhost esp32-08d1f9ca41e4
cargo run -p oxihome-client -- serial /dev/serial/by-id/usb-1a86_*
```

The current firmware only speaks MQTT; serial mode needs a firmware that uses the UART transport.
Close `espflash monitor` before using serial mode, only one program can hold the port.

## Editor

`.vscode/settings.json` links both the root workspace and `firmware/` in rust-analyzer. The firmware
check needs the ESP toolchain environment, so start the editor from a shell that loaded it:

```sh
. ~/export-esp.sh
code .
```

## Serial port access

On Arch, serial ports (`/dev/ttyUSB0`, `/dev/ttyACM0`) belong to the `uucp` group:

```sh
sudo usermod -aG uucp $USER
```

Log out and back in for the group change to take effect.
