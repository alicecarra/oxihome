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
./flash.sh --port /dev/serial/by-id/usb-1a86_* target/xtensa-esp32-espidf/release/firmware
espflash monitor --port /dev/serial/by-id/usb-1a86_*
```

Or build, flash and monitor in one step (uses the runner in `.cargo/config.toml`, which is
`flash.sh`):

```sh
cargo run --release
```

Always flash through `flash.sh`, not plain `espflash flash`. The flash has two app slots for
over-the-air updates (`partitions.csv`: `nvs`, `otadata`, `phy_init`, `ota_0` and `ota_1` of
1984 KiB each), and the script writes that table and the bootloader of our build, which rolls back
a new image that does not mark itself valid (`CONFIG_BOOTLOADER_APP_ROLLBACK_ENABLE` in
`sdkconfig.defaults`); `espflash` alone writes its own bootloader and its single-app table. The
script also erases `otadata`, so the board boots the flashed image from `ota_0` even if an update
had moved it to `ota_1`. Images go over the air in release (about 70% of a slot); debug ones fill
over 85%.

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
the entity numbers of the table below. Without the line, all of them; `""` for none. An unknown
name is an error on the serial log at boot, and the board restarts 10 s later (as with any startup
error).

Without a network the board keeps running: the Wi-Fi reconnects in the background (every
`WifiCheckInterval`, without blocking the loop, so the button still toggles its output), messages
that cannot be queued are dropped and logged, and the hub asks for the state again when the board
is back. A task watchdog restarts the board if its loop stops for 120 s.

## Over-the-air updates

Each board has its own configuration in `firmware/boards/<device id>.toml` (ignored by git), with
the same keys as `cfg.toml`. After changing the firmware and its version in `firmware/Cargo.toml`:

```sh
firmware/build-images.sh                      # every board
firmware/build-images.sh esp32-14335c2e7270   # or some of them
```

A board with another chip has a `chip` line in its file (`chip = "esp32c6"`); without it the
script builds for the classic ESP32. The firmware ignores the line.

The script builds a release image per board into `hub/firmware/<device id>/<version>.bin` (ignored
by git and readable only by the owner: the images hold the passwords) and puts `cfg.toml` back as
it was. The hub serves them when `hub.toml` has an `[ota]` section:

```toml
[ota]
listen = "0.0.0.0:8081"                     # reachable by the boards
public_url = "http://192.168.15.10:8081"    # how the boards reach it
images = "hub/firmware"
```

With firewalld (the laptop has it, `wlan0` in the `public` zone), the boards cannot reach port
8081 until it is open to the home network. The MQTT port works without this because Docker opens
its published ports itself. Until the next reboot:

```sh
sudo firewall-cmd --zone=public --add-rich-rule='rule family="ipv4" source address="192.168.15.0/24" port port="8081" protocol="tcp" accept'
```

Add `--permanent` and run `sudo firewall-cmd --reload` to keep it. Without the rule, the board
reports `UpdateDownloadFailed` (`ESP_ERR_HTTP_CONNECT`) after about 18 s and stays on its version.

The production hub on docker-host serves the images itself (`public_url` on 192.168.15.100) and
needs none of this: Docker publishes 8081 there. Without sudo on the notebook, a relay in Docker
gets past firewalld the same way (`docker run --rm -d --name ota-relay --add-host
host.docker.internal:host-gateway -p 8082:8082 alpine/socat TCP-LISTEN:8082,fork,reuseaddr
TCP:host.docker.internal:8081`, with `public_url` on port 8082).

That server is separate from the web interface, which stays private (`127.0.0.1` on the notebook;
in production `0.0.0.0` inside the container, published only on docker-host's localhost for
Caddy), and answers only `GET /ota/<token>`: a random link that works once and for 5 minutes.
When an online node that declares the `Ota` channel has an image newer than its firmware, its page
shows **Update to x.y.z**.
The hub sends the link in an `Ota` command over MQTT; the board answers, downloads the image into
its other app slot and reboots into it. The new image is on trial until it reaches the broker and
sends its `Hello`; if it does not within 60 s, the board restarts and the bootloader goes back to
the previous one. The hub notifies the outcome: updated (the `Hello` has the new version), rolled
back (it has the old one), failed or no answer within 5 minutes. A failure comes as an `Error` of
its own after the command's `Ack`: `UpdateDownloadFailed` (8) when the image could not be
downloaded (no connection, HTTP status, transfer cut), `UpdateRejected` (9) when it failed the
final check (corrupt, or for another chip), `Internal` for a flash write error. Firmware before
0.19.0 sends `Internal` with an "OTA:" detail, which the hub also takes as a failed update.

Each finished update is also stored in the `updates` table, kept until the node is forgotten (see
below): the node page's
**History** section (between the fields and the rules, refreshed live) lists the last 10
(versions, outcome, how long it took), next to the last 10
notifications about the node (offline, back online, rebooted, updates; the reboot of an update is
not notified), which go away with the
rest of the history after `retention_days`.

An offline node's page has a **Forget** button: the hub drops the node from memory and its rows
from `nodes`, `channel_values` and `updates`; its readings, events and commands age out with the
rest of the history, and its names in `nodes.toml` stay. A node that speaks again comes back.

## Test board wiring

ESP32 DevKit V1 (30 pins), `esp32-08d1f9ca41e4`, with every peripheral but the IR transmitter,
which is on the second board (see [Second board wiring](#second-board-wiring)). Entities the
firmware declares:

| Entity | What | Pin |
|---|---|---|
| 0 | Operation parameters (heartbeat and Wi-Fi check intervals) | - |
| 1 | Onboard LED, `Bool`; the BOOT button toggles it locally | GPIO2 |
| 2 | BOOT button, events `Press`, `LongPress` | GPIO0 |
| 3 | DHT11 module: channel 0 temperature (°C), channel 1 humidity (%), read every 30 s | GPIO4 (D4) |
| 4 | External LED on the breadboard, `Bool` | GPIO18 (D18) |
| 5 | Active buzzer, `Trigger` with the patterns `Short`, `Long` and `Alarm` | GPIO19 (D19) |
| 6 | VS1838B IR receiver module, event `Received` with the code as `Text` | GPIO23 (D23) |
| 7 | Potentiometer, `Int` 0–100 %, sent when it moves 2 points or more | GPIO34 (D34) |

DHT11 module: `+` to 3V3, `-` to GND, `S` to D4 (the module has its pull-up). External LED: D18 →
220–330 Ω resistor → LED long leg; short leg (flat side) to GND. Active buzzer (sealed black bottom;
the passive one, with a green board underneath, stays silent on a steady voltage): `+` to D19, the
other leg to GND. For a louder beep, drive it from 5 V through the kit's PN2222: D19 → 1 kΩ → base,
emitter to GND, collector to the buzzer's other leg, `+` to VIN. IR receiver module: `G` to GND, `R`
to 3V3 (not 5 V: the output follows the supply), `Y` to D23. Potentiometer: outer legs to 3V3 and
GND (swapping them only reverses the direction), middle leg to D34; never 5 V, and only ADC1 pins
(32–39) work while Wi-Fi is on. Free pins to use next: 25, 26, 27, 32, 33 (21 and 22 are kept for
I2C); avoid 0, 2, 12 and 15 (boot) and 6–11 (flash).

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

## Second board wiring

`esp32-14335c2e7270` ("Outra placa"), a third-party module (FCC ID 2BB77-ES32-32X, same ESP32 chip,
CH340 USB serial: `/dev/serial/by-id/usb-1a86_USB_Serial-if00-port0`). Its `cfg.toml` has
`peripherals = ""`; the hardware comes from the hub's `nodes.toml`:

| Entity | What | Pin |
|---|---|---|
| 1 | Onboard LED, `Bool`; the BOOT button toggles it locally | GPIO2 |
| 2 | BOOT button, events `Press`, `LongPress` | GPIO0 |
| 3 | IR transmitter (bare IR LED), `Trigger` with the code as `Text` | GPIO18 (D18) |

The IR LED takes more current than a pin gives, so a 2N2222 NPN switches it from 5 V: D18 → 1 kΩ →
base; emitter to GND; VIN → 47 Ω (or two 100 Ω in parallel) → LED long leg (anode); short leg
(cathode, flat side) → collector. About 75 mA in short pulses, a few metres of range. Generic
"2N2222 TO-92" parts come with either pinout, E-B-C or C-B-E (the base is always in the middle):
ours needed turning 180° from E-B-C. To check, move the 1 kΩ end from D18 to 3V3: the LED must
light up steadily (a phone camera shows infrared as purple); if it barely does, turn the transistor
around. When idle the LED must be off, or the receivers it points at are blinded.

The transmitter sends the codes the receiver reports, through the RMT peripheral:
`NEC 00 45` / `NEC 1234 45` (38 kHz), `SONY12|15|20 <address> <command>` (40 kHz, sent three
times 45 ms apart, as Sony remotes do), `RC5 00 0C` and `RC6 00 0C` (36 kHz; the toggle bit flips on
each send, so the device sees a new press), and `raw 9000 4500 560 …` (38 kHz, marks and spaces in
µs, up to 512). A trailing `repeat` is ignored. A code it does not know is refused with
`InvalidValue`. Sending blocks the board's main loop for the frame (under 200 ms).

## ESP32-C6 board

The NanoESP32-C6 v1.0 (`esp32-e8f60af8b410`, 16 MB of flash) is a RISC-V chip. The same `esp`
toolchain builds it; only the target and `MCU` change (the default in `.cargo/config.toml` stays the
classic ESP32). It has two USB-C ports: the native one (`303a:1001`, USB Serial/JTAG) shows up as
`/dev/ttyACM0` and is enough to flash and monitor. From `firmware/`:

```sh
MCU=esp32c6 cargo build --release --target riscv32imac-esp-espidf
./flash.sh --port /dev/ttyACM0 target/riscv32imac-esp-espidf/release/firmware
espflash monitor --port /dev/ttyACM0
```

`build-images.sh esp32-e8f60af8b410` builds its image (its board file has `chip = "esp32c6"`).
Differences from the classic board in the factory hardware: BOOT is GPIO9 and toggles nothing,
there is no onboard LED entity (the LED is a WS2812 on GPIO8, not a plain output), and the
potentiometer goes on GPIO2 (the ADC1 pins are GPIO0 to GPIO6). The other peripherals keep their
pins. Reserved: 12 and 13 (USB), 16 and 17 (UART0), 24 to 30 (flash). The release image fills
85% of an app slot (the classic one, about 70%).

The device id comes from the Wi-Fi station MAC (the one `espflash` prints): the C6 factory MAC in
eFuse is 8 bytes (EUI-64), so it would give another id.

Wired to it, from the hub's `nodes.toml`:

| Entity | What | Pin |
|---|---|---|
| 2 | BOOT button, events `Press`, `LongPress` | GPIO9 |
| 3 | AHT20 + BMP280 module (I2C), temperature (°C), humidity (%) and pressure (hPa) | SDA GPIO6, SCL GPIO7 |

The module goes on 3V3 and has its own pull-ups. Its driver, `aht20_bmp280`, is the only one with
two pins: `pin` is SDA and `clock_pin` is SCL, both checked like any pin; it reads every
`read_interval_seconds` (30 by default, 2 to 3600) and takes about 80 ms. The bus runs at 100 kHz
(esp-idf-hal's default is 1 MHz, too fast for the AHT20). When a chip does not answer, the error
lists the addresses that do (`nothing answers on the bus` is the wiring). Temperature and humidity
come from the AHT20 (0x38) and pressure from the BMP280 (0x77, or 0x76); the BMP280's own
temperature is not reported, because the chip warms itself. A board has one I2C bus for it, so a
second such entity fails to mount.

## MQTT broker

Mosquitto 2 in Docker on port 1883 (see `infra/`), with a password per client and no anonymous
access:

```sh
docker compose -f infra/compose.yaml up -d
```

Each node logs in with its device id as the user name (`esp32-<mac>`, `weather`, `network-devices`, `telegram`, `rss`, `youtube`, `printer`, `reminders`, `notebook`...);
the hub is `oxihome-hub` and `observer` only reads. The ACL in `infra/mosquitto/acl` (in git: it has
no secrets) lets a node publish only on `oxihome/<its id>/up` and read only its own `down` and the
broadcast; only the hub publishes on `down`. The client id is forced to the user name, so a login
cannot kick out another node by taking its client id. The hub also drops a message whose `src` does
not match the topic it came on.

The hashed passwords are in `infra/mosquitto/passwd`, out of git and owned by the container's
`mosquitto` user (uid 1883, mode 0600: Mosquitto refuses to start if it cannot read it). Create or
change a user with the script, which generates a password when none is given, prints it and reloads
the broker:

```sh
infra/mosquitto/set-password.sh <device id> [password]
```

Then put the password in the node's configuration (each section below says where) and keep that
file readable only by its owner (`chmod 600`); `install-as-service.sh` does it for the Linux
monitor's installed copy.

`max_packet_size` drops messages over 64 KiB at the broker (the largest today is a Linux monitor's
`Hello`, about 5.5 KB). The hub accepts up to 128 KiB, so an oversized message never reaches it;
rumqttc's default of 10 KiB would close the hub's connection, and a node could keep it down that
way. Mosquitto logs
warnings about the permissions of `acl` and about the `oxihome/broadcast/down` pattern without
`%u`; both are expected. Without TLS the passwords cross the network in plain text (layer 3 of the
"Segurança" note in Trilium).

## Hub

Connects to the broker configured in `hub/hub.toml` with the password in `hub/secrets.toml` (out
of git; `[mqtt] secrets` points to it, and without that line the hub connects anonymously):

```sh
cp hub/secrets.toml.example hub/secrets.toml   # then set the password of oxihome-hub
chmod 600 hub/secrets.toml
```

It asks every node for its `Hello` and `State`,
tracks which nodes are online (offline after `[presence] missed_heartbeats`, default 3, times the
node's heartbeat interval without messages; changing it needs a hub restart) and serves a web interface
(Topcoat) on the `[http]` address, by default <http://127.0.0.1:8080>:

```sh
cargo run -p oxihome-hub -- hub/hub.toml
```

Requests must name the hub in `Host` (`localhost`, `127.0.0.1`, `[::1]`, the `[http] host` or one
of `[http] allowed_hosts`), otherwise 403. A hub listening on `0.0.0.0` lists the names it is
reached by, e.g. `allowed_hosts = ["docker-host", "192.168.15.100"]`; the notebook's
`hub/hub.toml` lists its Wi-Fi and Tailscale addresses, so a tablet reaches a dev hub at
<http://192.168.15.10:8080>. firewalld blocks 8080 from the network; open it until the next reboot
or firewalld reload with:

```sh
sudo firewall-cmd --add-port=8080/tcp
```

Without the check, a site
pointing its own domain at 127.0.0.1 (DNS rebinding) would pass as same-origin. Topcoat already
rejects POSTs from other sites (`Sec-Fetch-Site`/`Origin`).

### Login

Since hub 0.5.0 the web interface asks for a password, whose argon2 hash goes in the secrets file
as `[http] password_hash`. The hub refuses to start without it unless `[http] host` is localhost.
Make the hash (the password is read from the first line of standard input):

```sh
cargo run -q -p oxihome-hub -- hash-password
```

Sessions last 30 days, extended while in use, and are kept in the database (table `sessions`), so
a restart or deploy keeps everyone signed in. "Sign out" in the menu ends the current one. After 10
wrong passwords in a minute the login refuses every attempt for the rest of that minute.

The session cookie is `Secure`, so the browser keeps it only over HTTPS or on localhost: a dev hub
at <http://localhost:8080> works, but at <http://192.168.15.10:8080> the login seems to do nothing
(the login page says so).

Only one hub can run per broker: both use the MQTT client id `oxihome-hub` and keep disconnecting
each other.

Ctrl+C or `SIGTERM` stops the hub at once: it logs `shutting down`, stops handling messages and
rules, closes the pages' event streams and gives other requests up to 3 s to finish. (Before
2026-10-12 it took 30 s with a page open, waiting for the event streams, which never end on their
own.) Before exiting it waits for the pending database writes.

### Database

The hub keeps its state and history in SQLite, in the file set in `[storage] path` (default
config: `hub/oxihome.sqlite`, out of git, with its `-wal` and `-shm` files). The file and its tables are created
at start when missing; the schema is in `hub/migrations/` and is applied by `sqlx` on start. SQLite
is built into the binary (`sqlite-bundled`), so no system package is needed. Dependencies added
on 2026-10-12:

```sh
cargo add -p oxihome-hub sqlx@0.9 --no-default-features -F runtime-tokio,sqlite-bundled,migrate,macros
cargo add -p oxihome-hub serde_json@1
```

What it keeps:

- `nodes`: the last `Hello` of each node (firmware, entities), when it was last seen, uptime, RSSI
  and heartbeat interval. After a restart the nodes show as offline with all their channels
  instead of "not seen", until they talk again; a lower uptime in the first heartbeat still shows
  as a reboot.
- `channel_values`: the last value of each channel, with its confidence, shown right after a
  restart; the `Query State` sent on connecting updates it.
- `readings`: every value change (history), with a `number` column for `Bool` (0 or 1), `Int` and
  `Float`, for charts.
- `events`: every event (button, IR code). The last event of each channel and the recent codes
  of the Learn section on `/remotes` are read back from it at start.
- `rule_runs`: the last run of each rule, shown on `/rules`. Runs of rules no longer in
  `rules.toml` (removed or renamed) are dropped at start and whenever the configuration is applied.
- `notifications`: the toasts (node offline, rule failed, `notify` actions), for the dashboard's
  `events` block.

`readings`, `events` and `notifications` keep `[storage] retention_days` days (default 7), pruned at start and
then every hour. Writes go to a task of their own, in one transaction per batch, so the MQTT loop
never waits for the disk. Times are milliseconds since the Unix epoch. To look inside (with the
`sqlite` package):

```sh
sqlite3 hub/oxihome.sqlite "SELECT device_id, entity, channel, datetime(at / 1000, 'unixepoch', 'localtime'), value FROM readings ORDER BY at DESC LIMIT 20"
```

Deleting the file starts from scratch: the nodes rebuild the state as before, from their `Hello`
and `State`.

The home's configuration lives in TOML files in the directory set in `[config] dir` (default
`hub/config/`, versioned in git). The hub reads them at start. They can be edited on `/settings`,
where each file has a text box and a Save button: the whole configuration is validated with the
new text before the file is written, and an error (file, line, column) leaves the file untouched.
Saving is refused if the file changed on disk since the page was loaded. After editing a file in an
editor instead, press **Reload config from disk**; a file with an error is reported there and the
previous configuration stays in use.

- `nodes.toml`: each node by the id from its `Hello`, with its friendly names (node, entities,
  channels) and its desired operation parameters in seconds. A node may name its entities and
  channels in its `Hello`; a name here wins over the node's, so only names the node does not give,
  or that the house calls something else, need to be here. The hub sends a `Set` when a node
  reports another value: when it connects, comes back online, or after a reload. A parameter left
  out keeps the node's value. Ranges: `heartbeat_interval` 5–300, `wifi_check_interval` 1–60.
  The node's page shows the node's value next to the desired one. A misspelled key is an error. (Until
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
  board buzzer (`invoke = { device = "...", entity = 5, channel = 0, value = "Alarm" }`) or an IR
  transmitter (`value = "SONY15 30 15"`, the code to send). The node's
  `Hello` decides what `value` is: one of the options of an `Enum` channel, or any text up to the
  `max_length` of a `Text` channel. While the node is online the hub refuses other values without
  sending them, and `/rules` shows the problem under the action. In the title and message of
  `notify` and the `value` of `invoke`, placeholders take what the trigger is about:

  | Trigger | Placeholders |
  |---|---|
  | channel event | `{node}`, `{channel}`; `{value}` too for `Received` (the event's text, e.g. `offline`) |
  | channel limit (`above`/`below`) | `{node}`, `{channel}`, `{value}` (the new value with its unit, e.g. `52.3 km/h`) |
  | `offline` | `{node}` |
  | `stopped`, `noisy` | `{node}`, `{channel}`, `{detail}` |
  | `unanswered`, `reported` | `{node}`, `{detail}` (the report's text, or e.g. "Used to report every 30 s") |
  | `at`, `cron`, `button`, `remote` | none |

  Names come from `nodes.toml`, then from what the node declares. A placeholder the trigger does
  not fill, or a misspelled one, stops the file from loading; `{{` and `}}` are literal braces.
  An `invoke` on an `Enum` channel cannot use placeholders, since the value must be one of the
  options. "Run now" on `/rules` fills them with stand-ins ("any node", "a channel", "run by
  hand", the limit for `{value}`), or the node's name when the trigger names one; `/rules` checks
  an `invoke`'s text against the channel with those stand-ins.
  `{ wait = "2s" }` between actions pauses the rule (`"500ms"`, `"1m 30s"`; more than zero, at
  most 30 minutes), e.g. to give a receiver time to turn on before choosing its input; a hub
  restart during the wait drops the rest. While a rule's actions run, the same rule firing again
  about the same node and channel is ignored ("still running" in the log), so a double press
  does not send the IR codes twice; another node, as with `offline = "*"`, still runs. "Run now"
  on a rule with a wait answers right away and its card shows "running" until it ends.
  `enabled = false` turns a rule off. Triggers:
  - an event of a channel: `when = { device = "...", entity = 2, channel = 0, event = "Press" }`;
    `data = "NEC 00 52"` also requires the event's text to be exactly that (a held key's
    `NEC 00 52 repeat` does not match);
  - a button, pressed on a `buttons` block of the dashboard or with "Press" on `/rules`:
    `when = { button = "Turn on" }` (the text is the button's label; two rules with the same button
    are an error when the configuration loads);
  - a key of a remote named in `remotes.toml`, from any IR receiver:
    `when = { remote = "Kit Lafvin", key = "0" }` (repeats do not match; an unknown key is an
    error when the configuration loads);
  - a local time, every day or on some days: `when = { at = "07:00", days = ["monday", "friday"] }`;
  - a numeric channel crossing a limit: `when = { device = "...", entity = 3, channel = 0, above = 28 }`
    (or `below = 27`). It runs when a value confirmed by the node crosses the limit, not again while
    the value stays past it; the first value the hub learns counts as a crossing (a value kept in the
    database from before a restart is not a first value, so a restart does not run the rule again). A pair of rules
    (on above 28, off below 27) gives hysteresis. `for = "5m"` (at most 24h) runs it only if the
    value stays past the limit that long after crossing: a confirmed value back within the limit or
    the node going offline ends the wait, and so does a hub restart;
  - a cron expression, standard five fields (minute, hour, day of month, month, day of week):
    `when = { cron = "*/30 8-22 * * mon-fri" }`. When both the day of month and the day of week
    are restricted, either one matching is enough, as in classic cron;
  - a node going offline: `when = { offline = "sim-bedroom" }`, or `offline = "*"` for any node.
    It runs when the hub marks the node offline (the same moment as the toast);
  - an alert of the traffic map on `/traffic`, checked every 10 s: `when = { stopped = "..." }` (a
    channel that sent `State` regularly went quiet while the node keeps sending other messages; a
    node that goes silent altogether is the `offline` trigger's), `noisy = "..."` (a channel sends
    far more than usual for at least 3 of the last 5 minutes, so holding a remote key is not noise)
    or `unanswered = "..."` (commands to the node went without an answer in the last hour), with a
    node id or `"*"` for any node. It runs when the alert goes up, not again while it stays up nor
    within 30 minutes of the last time; alerts about a node that is offline or installing an
    update are skipped, also after it comes back. The hub also shows a toast. The counts live in
    memory, so they start over when the hub restarts;
  - a node reporting a problem by itself: `when = { reported = "..." }` (node id or `"*"`), for an
    `Error` that answers no command, such as the firmware's "DHT11 not responding". It runs for
    every report; the hub also shows a toast and lists the report on the node's page.

  `cooldown = "1h"` (optional, at most 168h; not on button rules) keeps a rule from starting again
  that soon about the same node and channel, so `offline = "*"` still reports other nodes; the
  hub logs `rule "...": skipped, cooling down`. "Run now" ignores it and does not start it, and a
  hub restart forgets it.

  Conditions (`if`, optional) are checked when the trigger fires; the actions only run when every
  condition holds, otherwise the hub logs `rule "...": skipped, ... does not hold`. Each condition is
  one of:
  - the current value of a channel: `{ device = "...", entity = 3, channel = 0, above = 25 }`, or
    `below`, or `equals` (`true`/`false`, a number, or the text of a `Text` or `Enum` value). It does
    not hold while the node is offline or the value is unknown;
  - a local time range: `{ from = "22:00", to = "06:00" }`, from included, to excluded; when `from`
    is later than `to` the range crosses midnight;
  - days of the week: `{ days = ["saturday", "sunday"] }`;
  - a node online or offline: `{ online = "sim-bedroom" }`, `{ offline = "sim-bedroom" }`.

  ```toml
  [[rules]]
  name = "Button at night lights the lamp only when it is off"
  when = { device = "sim-living-room", entity = 2, channel = 0, event = "Press" }
  if = [
      { from = "22:00", to = "06:00" },
      { device = "sim-bedroom", entity = 1, channel = 0, equals = false },
  ]
  then = [{ set = { device = "sim-bedroom", entity = 1, channel = 0, value = true } }]
  ```

  Times are local, in the zone set in `[time] zone` of `hub/hub.toml` (an IANA name such as
  `America/Sao_Paulo`; required; the hub reads the machine's tzdata and falls back to a copy built
  into it, so the machine's own zone does not matter). Scheduled rules are checked once per minute:
  the minute the hub starts does not run, minutes skipped by a short gap (up to 5 minutes, e.g. a
  clock adjustment) run right after, and longer gaps (hub stopped or suspended) are not run late.
  `/rules` lists the rules with their trigger, conditions, next run and last result. Examples:

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

- `remotes.toml`: infrared remotes and the codes of their keys. The `/remotes` page lists them and
  has a "Learn" card with the last codes received (not repeats nor `raw` signals): name a remote and
  a key and the hub adds it to the file, keeping its comments. The remote field starts with the remote
  of the last learned key. A code on two keys is an error. Node
  cards show a named code as `Kit Lafvin · 0`. Every key, and every code in "Learn", has a "Send"
  button that sends it through the IR transmitter picked at the top of the page (the transmitters
  are the `ir_transmitter` entries of `nodes.toml`; with none, the buttons are disabled). A
  "Codes" switch there shows or hides the code column of the remotes (hidden by default; the choice
  stays in the browser).

  ```toml
  [remotes."Kit Lafvin".keys]
  0 = "NEC 00 52"
  1 = "NEC 00 16"
  ```

- `dashboard.toml`: the home page `/`, a list of blocks in file order on a grid of three columns
  (one on phones), refreshed live; the cards of a row share its height. Each block has a `type`, an optional `title` and a `width` of
  1 to 3 columns (default 1). Types: `nodes` (nodes by presence), `channels` (chosen channels with
  value and controls), `buttons` (buttons of rules by name, `buttons = ["Turn on", "Turn off"]`; a button of a disabled rule is greyed out), `value` (one channel's value, large, dimmed
  while its node is offline), `history` (the node page's chart for one channel, with 15 min, 1 h, 6 h and 24 h
  buttons; `range` of `15m`, `1h`, `6h` or `24h` sets the default, `24h` when missing; the picked range stays
  in the address, `?range4=6h` for the fifth block, through live updates and reloads), `rules` (the next 5 scheduled runs and the rules whose last run failed,
  with the error) and `events` (the latest node events and notifications, newest first, `limit` 1 to
  50, default 10; IR codes show the key name from `remotes.toml`) and `feeds` (the items of a node
  laid out like the RSS node, `device = "rss"`: every feed in one list, newest first, the title
  linking to the post in a new tab, the feed's name and age below; only unread items, with "Show
  read (N)" bringing the read ones back, grey, and the choice kept in the address, `?read5=1` for
  the sixth block; "Mark all read" invokes the node's entity 1; `limit` 1 to 50, default 10). The
  node keeps an unread count per feed, not which items, so a feed's newest N items are the unread
  ones. Notifications are stored in the database too (table `notifications`, same retention as
  events), so they survive a restart.

  Each `[[dashboards]]` is a tab, in file order: the first one at `/` (where the installed app
  opens), every one at `/d/<name>`; the name cannot be empty or have `/`. With a single dashboard
  there are no tabs. The tab row stays under the header while scrolling; a tab swaps the page in
  place and back works. A dot on another tab says it needs a look: red when a `job` block's state
  is `failed` or `error` or a `rules` block has a rule whose last run failed, amber when a node a
  `channels`, `value`, `history`, `feeds` or `job` block shows is offline. `attention = false` on a
  block leaves it out.

  ```toml
  [[dashboards]]
  name = "Home"

  [[dashboards.blocks]]
  type = "value"
  channel = { device = "esp32-08d1f9ca41e4", entity = 3, channel = 0 }

  [[dashboards.blocks]]
  type = "history"
  width = 2
  channel = { device = "esp32-08d1f9ca41e4", entity = 3, channel = 0 }
  range = "6h"

  [[dashboards.blocks]]
  type = "events"
  limit = 5

  [[dashboards.blocks]]
  type = "feeds"
  device = "rss"
  width = 2
  ```

Clicking a node's name on the home page opens its page, `/nodes/<id>`, with two sections that
update live. **Node overview**: name, presence, device id, firmware, uptime, signal and the
parameters (the node's value in red when it differs from the desired one). **Node fields**: a grid
of cards, one per entity, each channel with what it is (type, range and unit, options, access), its
value or last event and its controls. A number the node measures (a read only `Int` or `Float`,
such as temperature or humidity) also gets a chart from the database (see "Database"). Buttons next to **Edit names** pick the last
15 min, 1 h (default), 6 h or 24 h for every chart of the page; the choice stays in the address
(`?range=6h`), so live updates, edits and reloads keep it. A chart has 144 steps (about 6 s, 25 s, 2.5 min
or 10 min), the line at each step's average and a light band from its minimum
to its maximum, with the overall minimum and maximum below. Values are stored only when they change,
so a step without readings keeps the previous value; the chart starts at the first value known.
It is inline SVG drawn by the hub (`hub/src/web/chart.rs`), without JavaScript. Each section has its own edit button (**Edit** for the node
name and parameters, **Edit names** for entity and channel names), which turns only that section
into fields; it stops updating while edited. Saving writes `nodes.toml` with `toml_edit`, keeping
comments and the `entities.1 = { ... }` lines: an empty field removes the name or parameter, a field
left out stays as it is (an entity or channel name field holds only the hub's own name, with the
node's name as placeholder, so emptying it goes back to the node's name), the whole configuration is validated first (an error shows at the top and
keeps what was typed), and changed parameters go to the node right away. A node that never sent
a `Hello` (none in the database either) shows as unknown. At the bottom, **Rules** lists, one line per rule, the rules
that run on something from the node (an event or value of one of its channels, the node going
offline, `offline = "*"` included), the channels or presence their conditions check and the channels they act on; each name links to the rule's card
on `/rules`, which is highlighted. A channel used by rules shows a "N rules" mark (their names on
hover) that jumps there. Remote keys, buttons and schedules are not tied to a node, so those
rules only show through their actions. The pages set the mobile viewport, so they fit a
phone screen.

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

These `curl` lines, and the `/events` one above, work as they are only on a hub without
`[http] password_hash`; with the login on, add the session cookie copied from the browser (`-b`).

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
topcoat ui add select checkbox   # for the rule editor
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
and connect again; `Handle::is_connected` says whether what `act` returns gets published now. The simulator is built this way. The firmware uses only the core: its `Board`
implements `Device` (entities, LED and parameter state, LED, parameter and buzzer commands) and its
own MQTT client carries the messages.

Entities and channels may carry a `name` in the `Hello` (`EntityDescriptor::named` and
`Channel::named`; at most `NAME_MAX_LENGTH`, 128 bytes, and left out of the JSON when missing). The
hub shows it unless `nodes.toml` names the same thing, and ignores an empty or longer one. The
software nodes name everything they declare (feeds, devices, disks and containers by their name in
the node's configuration); the firmware does not name anything yet.

Publishing never waits: outgoing messages go into a queue of `Broker::queue_size` messages
(`mqtt::DEFAULT_QUEUE_SIZE`, 4096; `queue_size` under `[mqtt]` in each node's configuration) and
new ones are dropped, with a log line, while it is full. While disconnected the node drops its
reports and answers instead of queuing them, since the announcement after reconnecting carries the
current state. (Until 2026-10-13 a full queue of 32 blocked the loop for good: a node that lost
the broker for a few minutes stopped reconnecting, though its service still showed as running.)

`NodeCore::new` takes the node's `Build`, sent in its `Hello`: a short `name` that says what the
program is (`firmware`, `weather`, `linux-monitor`, `network-devices`, `telegram`, `rss`, `printer`, `reminders`, `simulator`), its `version`
(`major.minor.patch`, from its `Cargo.toml`: raise it when the node changes) and its `target`
(`esp32` for the firmware, from `MCU`; `Build::native` gives `<os>-<architecture>`, e.g.
`linux-x86_64`, for the software nodes). The hub shows it as **Software**, e.g.
`firmware 0.13.0 (esp32)`.

Every message carries `version`, the protocol version (`PROTOCOL_VERSION` in `oxihome-protocol`),
raised with every change of the wire format that the other side cannot read; it is 2 since the
`Build`. A node speaks only its own version; the hub reads the current one and the previous one
(`oxihome_protocol::legacy::read`, which turns the older messages into the current types, e.g. a
version 1 `Hello` with `fw_version` into a `Build` with target `unknown`). A node on the previous
version keeps working, can be updated over the air, and shows a **protocol 1** badge on its page,
in `/nodes` and in the dashboard's nodes block. Older or newer versions are ignored, logged and
notified once per node ("speaks another protocol"). So a node may lag one version, never two: when
`PROTOCOL_VERSION` goes up, `legacy` moves up with it, and the hub can be restarted first and the
nodes updated afterwards.

A node tells people something with a `Notice` (`NodeCore::notice`): a `level` (`Info`, `Warning`,
`Critical`), a `title` (up to 128 bytes), an optional `message` (up to 1024) and an optional `key`,
so a newer notice with the same key replaces the node's earlier one instead of piling up. Only the
hub reads it, as a toast, a notification and the `notice` rule trigger; the hub ignores `level` for
now. It came without a new protocol version (2026-10-24): only the hub reads messages from nodes
and the hub is always updated first, so no node has to be updated for it; a hub older than 0.17.0
logs a `Notice` as a message it cannot read.

```sh
cargo test -p oxihome-node --features mqtt
cargo clippy -p oxihome-node --features mqtt
```

## Weather node

`weather/` (`oxihome-weather`) is a software node with the weather outside, from
[Open-Meteo](https://open-meteo.com) (free, no account or key). It fetches every 10 minutes and
sends a heartbeat every 30 s; a failed fetch keeps the last readings.

```sh
cp weather/weather.toml.example weather/weather.toml   # then set the location
chmod 600 weather/weather.toml
cargo run -p oxihome-weather -- weather/weather.toml
```

`weather/weather.toml` stays out of git because it holds the location and the broker password
(`[mqtt] password`, user `weather` or the `[node] id`); a public place near home is
precise enough. Entities: 1 now (temperature °C, humidity %, precipitation mm over the last 15
minutes, wind km/h, raining `Bool`), 2 today (maximum and minimum °C, chance of rain %), 3 the local
time of the last reading as `Text`. All read only; rules use them like any sensor, with
`above`/`below` limits (e.g. precipitation above 0 to say it started raining). The protocol gained
the `Precipitation` and `WindSpeed` quantities and the `Millimeter` and `KilometerPerHour` units
for it.

## Linux monitor

`linux-monitor/` (`oxihome-linux-monitor`) is a software node with the state of the Linux machine
it runs on; install it on each computer and each one becomes a node. It reads `sysinfo` and `/sys`
and reports every 30 s with the heartbeat (the battery charge also goes in the heartbeat).

```sh
cp linux-monitor/linux-monitor.toml.example linux-monitor/linux-monitor.toml   # then pick disks and interfaces
chmod 600 linux-monitor/linux-monitor.toml
cargo run -p oxihome-linux-monitor -- linux-monitor/linux-monitor.toml
```

`linux-monitor/linux-monitor.toml` stays out of git because it is per machine; `[mqtt] password`
is the password of the node's id on the broker (create it with `set-password.sh` first). Without
`[node] id` the node takes the hostname. The program prints the mount points and interfaces it
finds when it starts.

To keep it running on a machine, as a systemd user service in `~/oxihome-linux-monitor` (binary,
config and log in the journal), here or on another machine over ssh:

```sh
linux-monitor/install-as-service.sh                        # this machine
linux-monitor/install-as-service.sh alice@192.168.15.100   # another one
sudo loginctl enable-linger $USER   # on servers: start at boot, without logging in
```

The script builds in release, replaces the binary, installs `oxihome-linux-monitor.service` and
restarts the service; run it again after changing the code. It never overwrites an installed
`linux-monitor.toml`: the first time it copies `linux-monitor/linux-monitor.toml` here and starts,
or copies the example on another machine and waits for it to be edited. The service reads that copy, not
the one in the repository. It also stops when the other machine has an older glibc than the binary
needs (x86_64 only; a Raspberry Pi needs its own build). Extra ssh options go in `SSH_OPTIONS`,
e.g. to reuse a master connection: `SSH_OPTIONS='-o ControlPath=~/.ssh/cm-%r@%h:%p'`.

Entities, all read only: 1 CPU (usage %, load over 1, 5 and 15 minutes, average frequency MHz,
temperature °C when a known sensor exists), 2 memory (used %, used, available and total GB, swap
used %), 3 battery (charge %, charging, plugged in; only on machines with their own battery, not
counting mice and keyboards), 4 system (uptime s, hostname, kernel, distribution). Disks take ids 10
to 29 and interfaces 30 to 49, in the order of the configuration, so add new ones at the end: disks
report used %, free and total GB; interfaces report download and upload kB/s, connected and the
address. Containers take ids 50 to 69, from `containers` in the configuration (names as
`docker ps` shows them, read through the Docker API on `docker_socket`, `/var/run/docker.sock` by
default; the user needs to be in the `docker` group): running, health (`healthy`, `unhealthy`,
`starting`, `none` without a healthcheck), CPU % of one whole CPU like `docker stats`, memory MB,
restarts, seconds up, and an event channel with `stopped`, `started`, `unhealthy` or `healthy` when
that changes between two reports (a rule with `data = "stopped"` catches a container going down; a
container that falls and comes back within 30 s only shows in restarts). A stopped container
reports zeros. CPU usage and rates start on the second report. GB are 10^9 bytes, so `df -h` (GiB) shows
smaller numbers. The protocol gained the `DataSize`, `DataRate` and `Frequency` quantities and the
`Gigabyte`, `Megabyte`, `KilobytePerSecond` and `Megahertz` units for it. Plans for more (GPU, sensors, session
events, actions) are in the Trilium note "Monitor Linux".

## Network devices node

`network-devices/` (`oxihome-network-devices`) is a software node that tells whether devices are
on the home network (phones, TV, console...), by MAC address. Every 30 s it sends one UDP datagram
to each address of the subnet, waits 10 s for the kernel to resolve them, and reads
`/proc/net/arp`: a device counts when its MAC has a complete entry. No root and no extra tools,
but Linux only, and the machine must be on that network (a laptop that leaves home sees nothing).

```sh
cp network-devices/network-devices.toml.example network-devices/network-devices.toml   # then list the devices
chmod 600 network-devices/network-devices.toml
infra/mosquitto/set-password.sh network-devices   # the password goes in [mqtt] password
cargo run -p oxihome-network-devices -- network-devices/network-devices.toml
```

`network-devices/network-devices.toml` stays out of git because it lists the household's devices
and holds the broker password. Each device has its entity id fixed in the file (0 is reserved), so
rules survive adding and removing devices. A device goes online as soon as it answers and offline
only after `offline_after` seconds of silence (`[network]`, overridable per device, at least 60):
phones turn Wi-Fi off while asleep, so 600 s suits them. Until a device answers, or stays silent
for that long after the start, its entity has no state. Phones need a fixed MAC on the home
network (turn off the rotating private address of iOS and Android).

Entities, one per device, all read only: channel 0 online (`Bool`), channel 1 an event with
`online` or `offline` when that changes (not on the first value), channel 2 the last IPv4 address,
channel 3 since when (local time, `2026-10-13 09:15`): the moment it came online, or the last time
it answered once offline. A rule with `event = "Received"` and `data = "offline"` on a phone's
entity catches it leaving. Entity 100 is the whole network: channel 0 how many devices answered the
last scan, channel 1 how many of them are not in `[[devices]]`. Those counts have no waiting time,
so sleeping phones make them go up and down; no MAC of an unknown device leaves the node. Channel 2
is "People home": how many devices with `person = true` (phones and watches, not the TV) are
online, with their `offline_after` wait, and no value until each of them is known. Nobody home
for 20 minutes is `when = { device = "network-devices", entity = 100, channel = 2, below = 1,
for = "20m" }`.

## Telegram bot

`telegram/` (`oxihome-telegram`) is a software node that links the house to a Telegram chat: rules
send messages to it, and the commands typed there become events. It uses `teloxide-core` (the Bot
API client only, with `rustls`) and long polling, so nothing is open to the internet. The plan and
its decisions are in the Trilium note "Bot do Telegram".

First, in Telegram, with [@BotFather](https://t.me/BotFather): `/newbot` (a name and a user name
ending in `bot`) gives the token; `/setjoingroups`, pick the bot, `Disable`, so nobody can add it
to a group. Your numeric user id comes from [@userinfobot](https://t.me/userinfobot) (send it any
message). Send the new bot a message too: a bot cannot start a private chat. Only one program may
poll a bot (a second `getUpdates` gets error 409), so working on the notebook while production
runs needs a second bot with its own token.

```sh
cp telegram/telegram.toml.example telegram/telegram.toml   # then the token, users, chat and commands
chmod 600 telegram/telegram.toml
infra/mosquitto/set-password.sh telegram   # the password goes in [mqtt] password
cargo run -p oxihome-telegram -- telegram/telegram.toml
```

`telegram/telegram.toml` stays out of git because it holds the bot token and the broker password.
`users` are the Telegram ids allowed to command; messages from anyone else are ignored without an
answer and logged. `chat` is where the rules' messages go (your user id for the private chat). Each
`[[commands]]` becomes an entry of the bot's menu (sent with `setMyCommands` at start) and an option
of the command event; names are 1 to 32 lowercase letters, digits or `_`.

Entities: 1 message, two `Trigger` channels of `Text` (up to 4096 bytes): 0 sends to `chat` with
sound, 1 silently. 2 command, an `Event` channel of `Enum` with the declared commands: `/receiver`
becomes `Received` with `receiver`. 3 connection: 0 whether Telegram answers `getUpdates` (`Bool`),
1 the last API error (`Text`). 4 reaction, an `Event` channel of `Text`: when an allowed user adds a
reaction (any emoji) in `chat` to one of the last 100 messages the bot sent, `Received` with that
message's text. The bot keeps those messages in memory only, so after a restart a reaction to an
older one does nothing; removing a reaction does nothing either. Example rules:

```toml
[[rules]]
name = "Receiver from Telegram"
when = { device = "telegram", entity = 2, channel = 0, event = "Received", data = "receiver" }
then = [{ invoke = { device = "esp32-14335c2e7270", entity = 3, channel = 0, value = "SONY15 30 15" } }]

[[rules]]
name = "Warn on Telegram when a node goes offline"
when = { offline = "*" }
then = [{ invoke = { device = "telegram", entity = 1, channel = 0, value = "{node} went offline" } }]

[[rules]]
name = "Warn on Telegram when a channel stops reporting"
when = { stopped = "*" }
then = [{ invoke = { device = "telegram", entity = 1, channel = 0, value = "{node} › {channel} stopped reporting: {detail}" } }]
```

A command is accepted only when it is declared, at most 10 per minute, and younger than `max_age`
(60 s by default: after an outage Telegram delivers what it kept, up to 24 h). An accepted command
gets a 👍 reaction, which means it reached the broker, not that a rule ran; with the broker down the
bot answers that the command was not delivered. Anything else from an allowed user gets the list of
commands. Messages go as plain text, one at a time; one that cannot go out (no internet) is tried
again for 5 minutes and then dropped, and Telegram's flood limit (429) is respected.

## RSS node

`rss/` (`oxihome-rss`) is a software node that follows RSS, Atom and JSON feeds (read with
`feed-rs`): an event for each new item and an unread count per feed, kept across restarts. The plan
and its decisions are in the Trilium note "Nó de RSS".

```sh
cp rss/rss.toml.example rss/rss.toml   # then the broker password and the feeds
chmod 600 rss/rss.toml
infra/mosquitto/set-password.sh rss    # the password goes in [mqtt] password
cargo run -p oxihome-rss -- rss/rss.toml
```

`rss/rss.toml` stays out of git because it holds the broker password. Each `[[feeds]]` has a fixed
`entity` from 2 up (the hub keeps history, rules and its own names by number, so removing a feed
must not renumber the others), a `name` (the entity's name on the hub, and in the log), the `url`, an `interval` in minutes (30 when missing, at least 15),
an optional `filter`, a regular expression on the title that ignores case, and an optional
`exclude`, one on the title and the link whose matching items are skipped. GitHub releases are
at `<repository>/releases.atom`, crates.io versions at
`https://static.crates.io/rss/crates/<crate>.xml`, a YouTube channel at
`https://www.youtube.com/feeds/videos.xml?channel_id=<id>`, a subreddit at
`https://www.reddit.com/r/<name>/.rss`. The `url` may also be a page that links its feed with
`<link rel="alternate">` (RSS, Atom or JSON Feed), such as a YouTube channel
(`https://www.youtube.com/@<handle>`) or a blog's home: when it is not a feed, the node follows the
first feed it links, looked up again at each start.

Entities: 1 all feeds, 0 "mark all as read" (`Trigger` without value), 1 the total unread
(`Int`). Each feed: 0 the newest item's title (`Text`), 1 its link (`Text`), 2 unread (`Int`), 3
"mark as read" (`Trigger`), 4 whether the feed answers (`Bool`, false after 3 failures in a row), 5
new item (`Event` of `Text`, `title — link`, `Received`). An example rule:

```toml
[[rules]]
name = "Beep on a new Mosquitto release"
when = { device = "rss", entity = 4, channel = 5, event = "Received" }
then = [{ invoke = { device = "esp32-08d1f9ca41e4", entity = 5, channel = 0, value = "Short" } }]
```

On the hub, the dashboard's `feeds` block (see "Hub", `dashboard.toml`) reads the node: unread
items from every feed in one list, with "Mark all read".

The first fetch of a feed (and of a feed whose URL changed) only marks its items as seen, so
nothing floods in. Afterwards, new items that pass the filters count as unread and become events,
oldest first, at most 10 per fetch (the rest only count). Opening a link does not mark anything as
read; only the two `Trigger`s do. `<name>-state.json`, next to `<name>.toml`, keeps the seen
ids (the last fetch's plus up to 500 older ones), the unread counts and the newest title and link;
it is written through a temporary file, so the directory must be writable. Fetches use `ETag` and
`If-Modified-Since` (kept in memory only), a 30 s timeout and a 5 MB limit; the feeds start 10 s
apart.

### YouTube

The YouTube channels run in a second instance of the same program, node `youtube`, so they have
their own unread count, "Mark all read" and `feeds` block (`device = "youtube"`), apart from the
blogs. Its configuration is `rss/youtube.toml`, also out of git, and its state
`rss/youtube-state.json`:

```sh
cp rss/youtube.toml.example rss/youtube.toml   # then the broker password and the channels
chmod 600 rss/youtube.toml
infra/mosquitto/set-password.sh youtube        # the password goes in [mqtt] password
cargo run -p oxihome-rss -- rss/youtube.toml
```

A channel's feed has its 15 newest uploads: videos, Shorts and live streams. Shorts have links
with `/shorts/`, so `exclude = "/shorts/"` skips them. A live stream enters the feed when it is
scheduled, not when it starts. The server sends no `ETag` or `Last-Modified` and caches the feed
for 15 minutes.

## Reminders node

`reminders/` (`oxihome-reminders`) is a software node with reminders that fall due at fixed times
(`cron`) or some time after they were last done (`every`, e.g. water the plants 3 days later). A
pending reminder is sent again every `repeat` until it is done, never inside the quiet hours (what
falls there goes out when they end). Done comes from the hub (a button, a rule) or from a reaction
to its Telegram message. The plan and its decisions are in the Trilium note "Nó de lembretes".

```sh
cp reminders/reminders.toml.example reminders/reminders.toml   # then the broker password and the reminders
chmod 600 reminders/reminders.toml
infra/mosquitto/set-password.sh reminders   # the password goes in [mqtt] password
cargo run -p oxihome-reminders -- reminders/reminders.toml
```

`reminders/reminders.toml` stays out of git because it holds the broker password; the node keeps
`reminders-state.json` next to it (last done, pending, last sent), so a restart neither forgets a
pending reminder nor sends it again early. A new reminder starts counting at the node's start,
without a warning. When the node was stopped while a reminder fell due, it sends one warning, not
one per missed time. Done before a fixed time counts for it when that time is less than 2 hours
away (the 8:30 medicine taken at 8:10 is not sent at 8:30).

Entities: 1 all reminders: 0 how many are pending (`Int`), 1 the text to send (`Event` of `Text`:
the name, and when repeated the name with "(pending since 08:30)"), 2 list pending (`Trigger`: sends
"Pending: A, B" on channel 1), 3 done by text (`Trigger` with `Text`: marks done the one reminder
whose name is in the text; a text with no name or with more than one is ignored, so no reminder name
may contain another). Each reminder, from entity 2: 0 pending (`Bool`), 1 done (`Trigger`), 2 last
done and 3 next due (`UnixTime`), 4 due (`Event`, `Press` when it falls due). The rules that link
it to Telegram, one for all reminders each:

```toml
[[rules]]
name = "Reminders: Telegram"
when = { device = "reminders", entity = 1, channel = 1, event = "Received" }
then = [{ invoke = { device = "telegram", entity = 1, channel = 0, value = "⏰ {value}" } }]

[[rules]]
name = "Reminders: done by a Telegram reaction"
when = { device = "telegram", entity = 4, channel = 0, event = "Received" }
then = [{ invoke = { device = "reminders", entity = 1, channel = 3, value = "{value}" } }]
```

## Printer node

`printer/` (`oxihome-printer`) is a software node with the state of a Creality K1 family 3D printer
(the K1C at `192.168.15.70`), on its stock firmware: no root, no Moonraker. It follows the websocket
the printer's own web page uses (port 9999, no password), which sends the whole status when it
connects and then only the fields that change, a few times a second while printing. Like that page,
the node sends `{"ModeCode": "heart_beat"}` every 5 s; 30 s without a message counts as the
printer gone, and it tries again every 10 s (the failure is logged once, not every attempt).

```sh
cp printer/printer.toml.example printer/printer.toml   # then set the broker password
chmod 600 printer/printer.toml
cargo run -p oxihome-printer -- printer/printer.toml
```

`printer/printer.toml` stays out of git because it holds the broker password (user `printer` or the
`[node] id`). Entities, all read only: 1 temperatures (nozzle, nozzle target, bed, bed target,
chamber, °C), 2 job (state, progress %, layer, layers, elapsed and left s, the file name without
folder and `.gcode`, and a change event), 3 printer (light on, error code; 0 is no error), 4
connection (reachable). While disconnected the readings keep their last values.

The job state follows the printer's `state` field, read from its web page: 0 `idle`, 1 `printing`,
2 `finished`, 3 `failed`, 4 `cancelled`, 5 `paused`. Each change publishes the new name as an
event on the job's change channel (not on the first status after connecting), so a rule with
`data = "finished"` catches the end of a print. To keep the traffic down, each channel goes out
alone, and only when it moved far enough from the last value sent: more than 1 °C for the
temperatures (the nozzle swings about 0.7 °C around its target and the chamber flips between two
whole degrees), 5 minutes for the elapsed and left times, any change for the rest. Small drifts
add up until they pass the step. While printing that is about one message a minute per layer
change plus a few temperatures. The camera (`http://192.168.15.70:8080/?action=snapshot`) is not used
yet.

## Simulator

Simulated nodes that talk to the broker like real boards, to work on the hub without hardware. Each
node in `simulator/simulator.toml` gets its own MQTT connection and a list of entities: either
ready-made profiles under `entities` (`lamp`, a switchable `Bool`; `button`, events;
`climate_sensor`, the firmware's DHT11), numbered from 1, or `clone`, a file with the entities
another node declared.

```sh
cp simulator/secrets.toml.example simulator/secrets.toml   # then set one password per node
chmod 600 simulator/secrets.toml
ssh docker-host oxihome/mosquitto/set-password.sh sim-remote   # one broker user per node
cargo run -p oxihome-simulator -- simulator/simulator.toml
```

The passwords are in `simulator/secrets.toml` (out of git), pointed to by `[mqtt] secrets`. The
nodes connect to the production broker and show up in the house's hub like any node (the "Any node
offline" rule also sends a Telegram message when one goes offline); the **Forget** button on a
node's page removes one that is no longer simulated.

The behaviour comes from each channel's access, not from the profile:

| Access | Behaviour |
| --- | --- |
| `Read` | `Int` and `Float` follow a slow wave around the middle of their range (5 % of the range each way, on the step), each channel with its own period; other types hold a default. `set` fixes a value until `auto`. |
| `ReadWrite` | Keeps the last `Set`, from the hub or from `set`; starts in the middle of the range (`false`, the first option, empty text), the management entity's parameters at their documented default. |
| `Write` | Keeps the last `Set`, left out of the state. |
| `Trigger` | Answers `Ack` and prints the channel and the value on the console. |
| `Event` | Sent by `event`: `Press` for channels without a value type, `Received` with the value for the others (an IR code). |

Type `help` for the commands. Channels are written `<entity>.<channel>`, values as their type
declares (`true`/`false`/`on`/`off`, numbers in range, an option of the list, text up to the end of
the line):

```text
list                                    every node's channels, with names, access and type
set sim-living-room 3.0 31              a fixed temperature, sent right away
set sim-living-room 1.0 on              the lamp turned on by hand, as the hub would see it
auto sim-living-room [3.0]              readings back to their wave
event sim-remote 2                      a press on the button (`press sim-remote 2` too)
event <node> 6.0 NEC 00 45              an IR code received, on a clone of the test board
offline | online | reboot <node>
```

To simulate a node the hub knows, download what it declared in its last `Hello`: **Download
entities** on the node's page in the hub (`/nodes/<device id>/entities.json`) gives
`<device id>.json`. Save it in `simulator/clones/` and point a node at it:

```toml
[[nodes]]
id = "sim-remote"
clone = "simulator/clones/esp32-14335c2e7270.json"
```

The clone keeps its own id from `simulator.toml` and the simulator's build, so the hub does not
offer it firmware updates. Boards send their entities without names (those live in the hub's
`nodes.toml`), so a cloned board shows "entity 3" until it gets names there; software nodes name
everything themselves.

### Controls in the hub

With a `[control]` section (`id = "simulator"`), the simulator is also a software node whose
entities do what the console does, so the hub's node page, dashboard and rules can drive the
simulated nodes. The hub only switches `Bool` channels and fires triggers, so every control is one
of those. One entity per simulated node, numbered from 1 in the order of `simulator.toml` and named
after the node:

| Channel | Kind | Does |
| --- | --- | --- |
| 0 Online | `Bool`, switch | `online` and `offline`; also follows the console |
| 1 Reboot | trigger | `reboot` |
| 2 Automatic readings | trigger | `auto` for the whole node |
| 3 and up, `Fix <entity>.<channel> …` | trigger with text, or with options for `Enum` and `Bool` readings | `set` on a read only channel; the text is read as that channel's type, out of range is refused |
| then `Event <entity>.<channel> …` | trigger, with the event's value type | `event` |

The control channels follow the order of the simulated channels, so changing a clone file or the
order of the nodes renumbers them; `list` shows them. Its broker user is created like the others
(`set-password.sh simulator`).

A rule is enough for a scenario, e.g. a dashboard button that warms the living room, drops the node
and brings it back:

```toml
[[rules]]
name = "Simulate a hot living room"
when = { button = "Hot living room" }
then = [
    { invoke = { device = "simulator", entity = 1, channel = 4, value = "31" } },
    { wait = "1m" },
    { set = { device = "simulator", entity = 1, channel = 0, value = false } },
    { wait = "30s" },
    { set = { device = "simulator", entity = 1, channel = 0, value = true } },
]
```

## Raw MQTT messages

To see the JSON messages nodes and the hub exchange, subscribe inside the broker container as
`observer`, which reads everything and cannot publish. Its password is in
`infra/mosquitto/observer-password` (out of git); the script passes it through an options file, not
the command line, where `ps` would show it:

```sh
infra/mosquitto/watch.sh                      # everything
infra/mosquitto/watch.sh 'oxihome/+/up'       # one topic filter
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

## Production

The production stack runs in Docker on `docker-host` (192.168.15.100, reserved in the router's
DHCP; `ssh docker-host` over Tailscale), from `~/oxihome` there. `infra/production/compose.yaml` is
its compose file; secrets and the home's configuration live only on the server. The plan and its
decisions are in the Trilium note "Produção".

### Broker

The broker's configuration is the same as the development one (`infra/mosquitto`). First install,
from the repository root on the notebook:

```sh
ssh docker-host 'mkdir -p ~/oxihome/mosquitto && chmod 700 ~/oxihome'
scp infra/production/compose.yaml docker-host:oxihome/compose.yaml
scp -p infra/mosquitto/{mosquitto.conf,acl,set-password.sh,watch.sh,observer-password} docker-host:oxihome/mosquitto/
# passwd belongs to the container's mosquitto user (uid 1883): read and write it through the image.
docker run --rm -v "$PWD/infra/mosquitto:/w:ro" eclipse-mosquitto:2 cat /w/passwd \
  | ssh docker-host 'docker run --rm -i -v ~/oxihome/mosquitto:/w eclipse-mosquitto:2 sh -c "cat > /w/passwd && chown mosquitto:mosquitto /w/passwd && chmod 600 /w/passwd"'
ssh docker-host 'cd ~/oxihome && docker compose up -d'
```

The container keeps the name `oxihome-mosquitto`, so the scripts work on the server as they do
locally: `ssh docker-host oxihome/mosquitto/set-password.sh <device id>` and
`ssh -t docker-host oxihome/mosquitto/watch.sh`. Users created there exist only on the server's broker.

The broker of `infra/compose.yaml` on the notebook is no longer used day to day (every board and
node points to docker-host); it stays for working offline.

### Images

`infra/production/Dockerfile` builds one image per workspace binary (`--build-arg BINARY=hub`
builds `oxihome-hub`), on Debian trixie (without `tzdata`: the hub carries its own time zone database). Images are built
on the notebook (the server is slower) and sent over ssh; there is no registry:

```sh
infra/production/build-image.sh hub    # prints the tag: oxihome-hub:<version>-<commit>[-dirty]
```

On the server, `oxihome-<binary>:current` is the image the compose file runs; the versioned tags
stay there, so going back is `docker tag oxihome-hub:<old tag> oxihome-hub:current` and
`docker compose up -d hub`.

### Deploy

```sh
infra/production/deploy.sh weather           # build, send, restart
infra/production/deploy.sh hub
infra/production/deploy.sh hub --no-build    # restart with the image already there
```

For the hub it also backs up the database into `~/oxihome/hub/backups` (the last 10) and replaces
`~/oxihome/hub/firmware` with `hub/firmware` (the over-the-air images). It refuses to run while a
hub runs on the notebook. The scripts copy with rsync, needed on both machines (on the server:
`sudo apt install rsync`).

Files on the server, each made once by hand: `~/oxihome/hub/hub.toml` (a copy of
`infra/production/hub.toml`: broker `mosquitto`, web interface on port 8090 since the Trilium has
8080, images on 8081), `~/oxihome/hub/secrets.toml` (a copy of `hub/secrets.toml`, with the
production `[http] password_hash`, see "Login") and
`~/oxihome/weather/weather.toml` (the local one with `host = "mosquitto"`) and
`~/oxihome/network-devices/network-devices.toml` (the local one with `host = "localhost"`: it runs
on the host's network to read its ARP table) and `~/oxihome/telegram/telegram.toml` (the production
bot's token, `host = "mosquitto"`) and `~/oxihome/rss/rss.toml` (`host = "mosquitto"`; the node
writes `rss-state.json` there, so that volume is not read only) and `~/oxihome/youtube/youtube.toml`
(the same for the YouTube instance) and `~/oxihome/printer/printer.toml` (`host = "mosquitto"` under `[mqtt]` only; the printer keeps its address) and `~/oxihome/reminders/reminders.toml` (`host = "mosquitto"`; the node writes `reminders-state.json` there, so that volume is not read only either). The web interface is
at <https://oxihome.alicecarra.com>, through Caddy (see "HTTPS"); the hub publishes 8090 only on
the server's localhost. The containers run as uid 1000, the owner of `~/oxihome`.

```sh
ssh docker-host 'mkdir -p ~/oxihome/hub/config ~/oxihome/hub/firmware ~/oxihome/weather && chmod 700 ~/oxihome/hub ~/oxihome/weather'
scp infra/production/hub.toml docker-host:oxihome/hub/hub.toml
scp -p hub/secrets.toml docker-host:oxihome/hub/secrets.toml
sed 's/^host = "192.168.15.100"/host = "mosquitto"/' weather/weather.toml | ssh docker-host 'umask 077; cat > ~/oxihome/weather/weather.toml'
ssh docker-host 'mkdir -p ~/oxihome/network-devices && chmod 700 ~/oxihome/network-devices'
sed 's/^host = "192.168.15.100"/host = "localhost"/' network-devices/network-devices.toml | ssh docker-host 'umask 077; cat > ~/oxihome/network-devices/network-devices.toml'
ssh docker-host 'mkdir -p ~/oxihome/telegram && chmod 700 ~/oxihome/telegram'
ssh docker-host oxihome/mosquitto/set-password.sh telegram   # the password for [mqtt] password
sed 's/^host = .*/host = "mosquitto"/' telegram/telegram.toml | ssh docker-host 'umask 077; cat > ~/oxihome/telegram/telegram.toml'
infra/production/deploy.sh telegram
ssh docker-host 'mkdir -p ~/oxihome/rss && chmod 700 ~/oxihome/rss'
ssh docker-host oxihome/mosquitto/set-password.sh rss   # the password for [mqtt] password
sed 's/^host = .*/host = "mosquitto"/' rss/rss.toml | ssh docker-host 'umask 077; cat > ~/oxihome/rss/rss.toml'
infra/production/deploy.sh rss
ssh docker-host 'mkdir -p ~/oxihome/youtube && chmod 700 ~/oxihome/youtube'
ssh docker-host oxihome/mosquitto/set-password.sh youtube   # the password for [mqtt] password
sed 's/^host = .*/host = "mosquitto"/' rss/youtube.toml | ssh docker-host 'umask 077; cat > ~/oxihome/youtube/youtube.toml'
infra/production/deploy.sh youtube   # the rss image; deploy.sh rss does not restart this one
ssh docker-host 'mkdir -p ~/oxihome/printer && chmod 700 ~/oxihome/printer'
ssh docker-host oxihome/mosquitto/set-password.sh printer   # the password for [mqtt] password
sed '0,/^host = /s/^host = .*/host = "mosquitto"/' printer/printer.toml | ssh docker-host 'umask 077; cat > ~/oxihome/printer/printer.toml'
infra/production/deploy.sh printer
ssh docker-host 'mkdir -p ~/oxihome/reminders && chmod 700 ~/oxihome/reminders'
ssh docker-host oxihome/mosquitto/set-password.sh reminders   # the password for [mqtt] password
sed 's/^host = .*/host = "mosquitto"/' reminders/reminders.toml | ssh docker-host 'umask 077; cat > ~/oxihome/reminders/reminders.toml'
infra/production/deploy.sh reminders
```

The first hub deploy took the notebook's configuration and database by faking a dev session
(`ssh docker-host touch oxihome/hub/dev-session` before `deploy.sh hub`).

### HTTPS

Caddy, a stack of its own in `~/caddy` on docker-host (not part of this repository), serves the web
interface at <https://oxihome.alicecarra.com> and passes it to the hub at `localhost:8090`. It
runs on the host's network, so it can reach any service published on localhost without joining
its network. Files: `docker-compose.yml`, a `Dockerfile` that builds the official image with the
Cloudflare DNS plugin (`xcaddy build --with github.com/caddy-dns/cloudflare`), `conf/Caddyfile`
(one block per site) and `.env` (mode 600) with `CLOUDFLARE_API_TOKEN`. The certificates come from
Let's Encrypt through the DNS challenge, so nothing is open to the internet.

At Cloudflare: an API token from the "Edit zone DNS" template plus Zone / Zone / Read, limited to
the `alicecarra.com` zone, and an A record `oxihome` → `192.168.15.100`, DNS only (grey cloud). Caddy
took ports 80 and 443 from the pihole, which is stopped for now (`~/pihole`).

```sh
ssh docker-host 'cd ~/pihole && docker compose stop'
ssh docker-host 'cd ~/caddy && docker compose up -d --build'
ssh docker-host 'cd ~/caddy && docker compose logs -f'       # wait for the certificate
ssh docker-host 'cd ~/caddy && docker compose exec caddy caddy reload --config /etc/caddy/Caddyfile'   # after editing the Caddyfile
```

Away from home the same address works over Tailscale: docker-host advertises a route to its own LAN
address, approved in the admin console (Machines → docker-host → Edit route settings). Phones, macOS
and Windows accept routes by default (Android: "Use Tailscale subnets"); Linux needs
`--accept-routes`.

```sh
ssh -t docker-host 'sudo tailscale set --advertise-routes=192.168.15.100/32'
sudo tailscale set --accept-routes    # on a Linux client used away from home
```

### Working on the hub

Only one hub may run per broker, so the notebook's hub takes the production one's place:

```sh
infra/production/dev-hub.sh
```

It stops the hub on the server, brings its configuration into `hub/config` (so `git diff` shows what
changed in production; it refuses to run with uncommitted changes there) and its database into
`hub/oxihome.sqlite`, and runs `cargo run -p oxihome-hub -- hub/hub.toml`, whose broker is
docker-host's. Stop it with Ctrl+C and run it again as often as needed: only the first run fetches.
`deploy.sh hub` ends the session: it sends the configuration and the database back and starts the
production hub with the new image. `deploy.sh hub --no-build` returns without a new image, and
`--discard` drops the session's configuration and database instead of sending them. While the
session is open the house depends on the notebook's hub.

The database goes back already migrated by the session's hub, so end a session that ran a newer
version with a deploy of that version, not `--no-build`.
