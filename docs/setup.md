# Setup

Oxihome development environment. Tested on Arch Linux. Everything else is in the
[index](README.md).

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

## Known issue: hub link crash

Since 2026-10-11, `rust-lld` (rustc 1.99.0, b940084d7) crashes with a segmentation fault when linking
`oxihome-hub`; the simulator links fine. `rustup update` found no newer stable. Linking with the GNU
linker works:

```sh
RUSTFLAGS="-C link-arg=-fuse-ld=bfd" cargo build -p oxihome-hub
```

Changing `RUSTFLAGS` rebuilds every dependency once.
