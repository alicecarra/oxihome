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

## Test board wiring

ESP32 DevKit V1 (30 pins), `esp32-08d1f9ca41e4`. Entities the firmware declares:

| Entity | What | Pin |
|---|---|---|
| 0 | Operation parameters (heartbeat and Wi-Fi check intervals) | - |
| 1 | Onboard LED, `Bool`; the BOOT button toggles it locally | GPIO2 |
| 2 | BOOT button, events `Press`, `LongPress` | GPIO0 |
| 3 | DHT11 module: channel 0 temperature (°C), channel 1 humidity (%), read every 30 s | GPIO4 (D4) |
| 4 | External LED on the breadboard, `Bool` | GPIO18 (D18) |
| 5 | Active buzzer, `Trigger` with the patterns `Short`, `Long` and `Alarm` | GPIO19 (D19) |
| 6 | VS1838B IR receiver module, event `Received` with the code as `Text` | GPIO23 (D23) |

DHT11 module: `+` to 3V3, `-` to GND, `S` to D4 (the module has its pull-up). External LED: D18 →
220–330 Ω resistor → LED long leg; short leg (flat side) to GND. Active buzzer (sealed black bottom;
the passive one, with a green board underneath, stays silent on a steady voltage): `+` to D19, the
other leg to GND. For a louder beep, drive it from 5 V through the kit's PN2222: D19 → 1 kΩ → base,
emitter to GND, collector to the buzzer's other leg, `+` to VIN. IR receiver module: `G` to GND, `R`
to 3V3 (not 5 V: the output follows the supply), `Y` to D23. Free pins to use next: 25, 26, 27, 32,
33 (21 and 22 are kept for I2C); avoid 0, 2, 12 and 15 (boot) and 6–11 (flash).

The IR receiver is read by the RMT peripheral. NEC frames (most cheap remotes, including the kit's)
arrive as `NEC 00 45` (8-bit address and command in hex) or `NEC 1234 45` (extended 16-bit
address); a held key sends `NEC 00 45 repeat` about every 110 ms. Sony SIRC frames (12, 15 or 20
bits) arrive as `SONY15 30 15` (length, address and command in hex); Sony remotes send each frame
at least three times, so the same code again within 150 ms arrives as `SONY15 30 15 repeat`.
Anything else arrives as
`raw 9000 4500 560 …`, the mark and space durations in µs (at most 128). The code is a `Text` value
(up to 1024 bytes); the hub shows it on the node card and in its log. The decoder tolerates a remote
held at an angle: pulses under 250 µs are merged with their neighbours, the 4.5 ms header space may
come after noise, and each bit's space counts as 0 below 1.1 ms and 1 up to 2.4 ms (the inverted
command byte still rules out wrong readings).

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

- `nodes.toml`: each node by the id from its `Hello`, with its friendly names (node, entities,
  channels) and its desired operation parameters in seconds. The hub sends a `Set` when a node
  reports another value: when it connects, comes back online, or after a reload. A parameter left
  out keeps the node's value. Ranges: `heartbeat_interval` 5–300, `wifi_check_interval` 1–60.
  `/settings` shows the node's value next to the desired one. A misspelled key is an error. (Until
  2026-10-12 the names were in a separate `names.toml`.)

  ```toml
  [nodes."esp32-08d1f9ca41e4"]
  name = "Test board"
  entities.1 = { name = "LED", channels.0 = "Power" }
  parameters = { heartbeat_interval = 30, wifi_check_interval = 5 }
  ```

- `rules.toml`: a rule runs its actions when its trigger (`when`) fires. Actions set or toggle
  boolean channels, or show a notification as a toast on the web interface
  (`notify = { title = "...", message = "..." }`), or fire a `Trigger` channel such as the test
  board buzzer (`invoke = { device = "...", entity = 5, channel = 0, value = "Alarm" }`). The node's
  `Hello` decides what `value` is: one of the options of an `Enum` channel, or any text up to the
  `max_length` of a `Text` channel. While the node is online the hub refuses other values without
  sending them, and `/rules` shows the problem under the action.
  `enabled = false` turns a rule off. Triggers:
  - an event of a channel: `when = { device = "...", entity = 2, channel = 0, event = "Press" }`;
    `data = "NEC 00 52"` also requires the event's text to be exactly that (a held key's
    `NEC 00 52 repeat` does not match);
  - a button of a panel in `panels.toml`, pressed on the `/panel` page:
    `when = { panel = "Test board", button = "Turn on" }` (a button can run several rules; an unknown
    panel or button is an error when the configuration loads);
  - a key of a remote named in `remotes.toml`, from any IR receiver:
    `when = { remote = "Kit Lafvin", key = "0" }` (repeats do not match; an unknown key is an
    error when the configuration loads);
  - a local time, every day or on some days: `when = { at = "07:00", days = ["monday", "friday"] }`;
  - a numeric channel crossing a limit: `when = { device = "...", entity = 3, channel = 0, above = 28 }`
    (or `below = 27`). It runs when a value confirmed by the node crosses the limit, not again while
    the value stays past it; the first value the hub learns counts as a crossing. A pair of rules
    (on above 28, off below 27) gives hysteresis;
  - a cron expression, standard five fields (minute, hour, day of month, month, day of week):
    `when = { cron = "*/30 8-22 * * mon-fri" }`. When both the day of month and the day of week
    are restricted, either one matching is enough, as in classic cron;
  - a node going offline: `when = { offline = "sim-bedroom" }`, or `offline = "*"` for any node.
    It runs when the hub marks the node offline (the same moment as the toast).

  Times are local, in the zone set in `[time] zone` of `hub/hub.toml` (an IANA name such as
  `America/Sao_Paulo`; the system's zone when missing). Scheduled rules are checked once per minute:
  the minute the hub starts does not run, minutes skipped by a short gap (up to 5 minutes, e.g. a
  clock adjustment) run right after, and longer gaps (hub stopped or suspended) are not run late.
  `/rules` lists the rules with their trigger, next run and last result. Examples:

  ```toml
  [[rules]]
  name = "Living room button toggles bedroom lamp"
  when = { device = "sim-living-room", entity = 2, channel = 0, event = "Press" }
  then = [{ toggle = { device = "sim-bedroom", entity = 1, channel = 0 } }]

  [[rules]]
  name = "Test board LED on for waking up"
  when = { at = "07:00", days = ["monday", "tuesday", "wednesday", "thursday", "friday"] }
  then = [{ set = { device = "esp32-08d1f9ca41e4", entity = 1, channel = 0, value = true } }]

  [[rules]]
  name = "Test board LED blinks: on at even minutes"
  when = { cron = "*/2 * * * *" }
  then = [{ set = { device = "esp32-08d1f9ca41e4", entity = 1, channel = 0, value = true } }]
  ```

- `panels.toml`: panels of buttons for the `/panel` page, in file order. Each button shows the
  rules it runs ("no rule yet" and disabled when none is enabled). Panel names, and button names
  within a panel, are unique.

  ```toml
  [[panels]]
  name = "Test board"
  buttons = ["Turn on", "Turn off"]
  ```

- `remotes.toml`: infrared remotes and the codes of their keys. The `/remotes` page lists them and
  has a "Learn" card with the last codes received (not repeats nor `raw` signals): name a remote and
  a key and the hub adds it to the file, keeping its comments. The remote field starts with the remote
  of the last learned key. A code on two keys is an error. Node
  cards show a named code as `Kit Lafvin · 0`.

  ```toml
  [remotes."Kit Lafvin".keys]
  0 = "NEC 00 52"
  1 = "NEC 00 16"
  ```

The page uses htmx: the switches post to `/set` and the controls of `Trigger` channels (a button per
option, such as the buzzer patterns, or a text box for a `Text` channel) post to `/invoke`, without
reloading the page, and the hub announces
changes over server-sent events at `/events` (`nodes` when a value, event or presence changes,
`notifications` for a new toast). The page fetches the node list and the toasts again when it hears
them, and every 30 s as well, which also keeps the "3 s ago" labels moving. To watch the events:
`curl -N localhost:8080/events`.

`/set` answers with an empty body on success or the error message; a missing `value` means `false`
(an unchecked switch sends nothing). The same
request from the command line:

```sh
curl -X POST -d "device_id=esp32-08d1f9ca41e4&entity=1&channel=0&value=true" localhost:8080/set
curl -X POST -d "device_id=esp32-08d1f9ca41e4&entity=5&channel=0&value=Short" localhost:8080/invoke
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

htmx and its SSE extension are vendored in `hub/static/` and served by the hub, so the page works
without internet access. To update them, change the version and download again:

```sh
curl -sSfL -o hub/static/htmx.min.js https://cdn.jsdelivr.net/npm/htmx.org@2.0.10/dist/htmx.min.js
curl -sSfL -o hub/static/htmx-ext-sse.min.js https://cdn.jsdelivr.net/npm/htmx-ext-sse@2.2.4/dist/sse.min.js
```

## Node library

`node/` (`oxihome-node`) is what every node does, whatever it runs on: it answers the hub's queries,
numbers messages, answers a repeated command with the same `Ack` without running it again, and
checks each command against the entities the node declares (unknown entity or channel, `Access`,
value type, `Int`/`Float` range, `Enum` option, `Text` length) before the node sees it. A node
implements the `Device` trait (its entities, the state of an entity, running a checked command) and
`NodeCore` turns messages from the hub into answers. No network and no clock, so it builds for the
boards too.

The `mqtt` feature (Tokio and `rumqttc`, for computers only) runs a software node: implement
`SoftwareNode` (device id, answers to a message, announcement, periodic report) and
`mqtt::spawn` connects it with its own MQTT client, subscribes to its topics and the broadcast,
announces on every connection, publishes the report every `report_interval`, reconnects by itself,
and returns a `Handle` to act on the node from elsewhere (a console, a chat bot) or to disconnect it
and connect again. The simulator is built this way. The firmware uses only the core: its `Board`
implements `Device` (entities, LED and parameter state, LED, parameter and buzzer commands) and its
own MQTT client carries the messages.

```sh
cargo test -p oxihome-node
cargo clippy -p oxihome-node --features mqtt
```

## Simulator

Simulated nodes that talk to the broker like real boards, to work on the hub without hardware.
Each node in `simulator/simulator.toml` gets its own MQTT connection and a list of entity profiles:
`lamp` (switchable `Bool`), `button` (events) and `climate_sensor` (temperature and humidity).

```sh
cargo run -p oxihome-simulator -- simulator/simulator.toml
```

Type `help` for the commands: `press <node> <entity>`, `offline <node>`, `online <node>`,
`reboot <node>`, `climate <node> <temperature> [humidity]`, `list`, `quit`.

Climate sensors follow a slow wave (22 ± 2 °C, 50 ± 5 %). `climate sim-living-room 30 80` fixes
the readings and sends them right away, to test rules with limits; a value left out keeps following
the wave, and `climate sim-living-room auto` goes back to the wave for both.

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

## Known issue: hub link crash

Since 2026-10-11, `rust-lld` (rustc 1.99.0, b940084d7) crashes with a segmentation fault when linking
`oxihome-hub`; the simulator links fine. `rustup update` found no newer stable. Linking with the GNU
linker works:

```sh
RUSTFLAGS="-C link-arg=-fuse-ld=bfd" cargo build -p oxihome-hub
```

Changing `RUSTFLAGS` rebuilds every dependency once.
