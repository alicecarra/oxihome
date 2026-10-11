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
tracks which nodes are online (offline after `[presence] missed_heartbeats`, default 3, times the
node's heartbeat interval without messages; changing it needs a hub restart) and serves a web interface
(Topcoat) on the `[http]` address, by default <http://127.0.0.1:8080>:

```sh
cargo run -p oxihome-hub -- hub/hub.toml
```

Only one hub can run per broker: both use the MQTT client id `oxihome-hub` and keep disconnecting
each other.

The home's configuration lives in TOML files in the directory set in `[config] dir` (default
`hub/config/`, versioned in git). The hub reads them at start. They can be edited on `/settings`,
where each file has a text box and a Save button: the whole configuration is validated with the
new text before the file is written, and an error (file, line, column) leaves the file untouched.
Saving is refused if the file changed on disk since the page was loaded. After editing a file in an
editor instead, press **Reload config from disk**; a file with an error is reported there and the
previous configuration stays in use.

- `names.toml`: friendly names for nodes, entities and channels.

  ```toml
  [nodes."esp32-08d1f9ca41e4"]
  name = "Test board"
  entities.1 = { name = "LED", channels.0 = "Power" }
  ```

- `rules.toml`: a rule reacts to an event of a channel and sets or toggles boolean channels, or
  shows a notification as a toast on the web interface (`notify = { title = "...", message = "..." }`).
  E.g. the simulator's living room button toggles the bedroom lamp:

  ```toml
  [[rules]]
  name = "Living room button toggles bedroom lamp"
  when = { device = "sim-living-room", entity = 2, channel = 0, event = "Press" }
  then = [{ toggle = { device = "sim-bedroom", entity = 1, channel = 0 } }]
  ```

- `nodes.toml`: desired operation parameters of each node (seconds). The hub sends a `Set` when a
  node reports another value: when it connects, comes back online, or after a reload. A parameter
  left out keeps the node's value. Ranges: `heartbeat_interval` 5–300, `wifi_check_interval` 1–60.
  `/settings` shows the node's value next to the desired one.

  ```toml
  [nodes."esp32-08d1f9ca41e4"]
  heartbeat_interval = 30
  wifi_check_interval = 5
  ```

The page uses htmx: the node list refreshes every 2 s and the switches post to `/set` without
reloading the page. `/set` answers with an empty body on success or the error message; a missing `value` means
`false` (an unchecked switch sends nothing). The same
request from the command line:

```sh
curl -X POST -d "device_id=esp32-08d1f9ca41e4&entity=1&channel=0&value=true" localhost:8080/set
```

Styles use Tailwind CSS 4. `hub/build.rs` runs Topcoat's Tailwind integration: on the first build it
downloads the standalone Tailwind CLI (pinned by Topcoat, cached in `target/topcoat/cache/tailwind/`,
needs internet once), scans `hub/src/**/*.rs` for class names (input: `hub/styles.css`) and writes
the CSS that the hub serves at `/static/app.css`. No Node and no `topcoat asset bundle` are needed.
Class names must appear literally in the code; names assembled at runtime are not generated.

UI components come from Topcoat UI (shadcn/ui style): their Rust source is copied into
`hub/src/components/` and becomes ours to edit; the theme (design tokens, light and dark) is
`hub/styles.css` and the install state is `hub/components.toml`. The Topcoat CLI is only needed to
add or update components, not to build or run the hub:

```sh
cargo install topcoat-cli --version 0.10.0 --locked
cd hub
topcoat ui list                  # available and installed components
topcoat ui add badge button card # copy components into src/components
```

Avoid `dialog`, `alert_dialog`, `sheet`, `sidebar` and `tabs`: they need Topcoat's JavaScript
runtime, which the hub does not use.

htmx is vendored in `hub/static/htmx.min.js` and served by the hub, so the page works without
internet access. To update it, change the version and download again:

```sh
curl -sSfL -o hub/static/htmx.min.js https://cdn.jsdelivr.net/npm/htmx.org@2.0.10/dist/htmx.min.js
```

## Simulator

Simulated nodes that talk to the broker like real boards, to work on the hub without hardware.
Each node in `simulator/simulator.toml` gets its own MQTT connection and a list of entity profiles:
`lamp` (switchable `Bool`), `button` (events) and `climate_sensor` (temperature and humidity).

```sh
cargo run -p oxihome-simulator -- simulator/simulator.toml
```

Type `help` for the commands: `press <node> <entity>`, `offline <node>`, `online <node>`,
`reboot <node>`, `list`, `quit`.

## Raw MQTT messages

To see the JSON messages nodes and the hub exchange, subscribe inside the broker container:

```sh
docker exec oxihome-mosquitto mosquitto_sub -t 'oxihome/#' -v
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
