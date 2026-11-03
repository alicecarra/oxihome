# Boards

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
