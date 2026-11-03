# Sensors

## ADXL345 accelerometer

Driver `adxl345`, on either chip, for vibration and tilt. Board `esp32-e8f60afbdf9c` (NanoESP32-C6)
has it as entity 9 through `peripherals = "accelerometer"` in its board file, which picks SDA on
GPIO6 and SCL on GPIO7 (GPIO21 and GPIO22 on the classic ESP32).

| Module | Board |
|---|---|
| VCC | 3V3 |
| GND | GND |
| SDA | GPIO6 |
| SCL | GPIO7 |
| CS | 3V3 (selects I2C) |
| SDO | GND (address 0x53; high is 0x1D) |
| INT1, INT2 | not connected |

In `nodes.toml` the same is `{ id = 9, pin = 6, driver = "adxl345", clock_pin = 7 }`, with
`read_interval_seconds` (30 by default, 2 to 3600) and `vibration_threshold_milli_g` (250 by
default, 10 to 16000). It uses the board's one I2C bus, so it cannot share a board with
`aht20_bmp280` yet: the second of them fails to mount.

Channels: `Vibrating` (0), acceleration `X`, `Y` and `Z` in g (1 to 3), `Pitch` and `Roll` in
degrees (4 and 5). `Vibrating` is reported as soon as it changes, the others every read interval
as the average of the latest 100 ms. The board reads the chip's FIFO every 100 ms; a swing above the
threshold on any axis inside one read is vibration, and it ends after 5 quiet seconds. The chip's
own activity detection is not used: with link mode and ac coupling it went on and off every 5 s on
a board at rest (firmware 0.27.0 during development). Tune the threshold with the sensor fixed to
the real appliance.

Creating the board's broker user:

```sh
ssh docker-host oxihome/mosquitto/set-password.sh esp32-e8f60afbdf9c   # goes in mqtt_password
```

## Reed switch module

Magnetic reed switch on a 3-pin board with a comparator, driver `contact`, on either chip. Board
`esp32-e8f60afbdf9c` has it as entity 10 through `contact` in `peripherals` of its board file, which
picks GPIO20 (GPIO25 on the classic ESP32).

| Module | Board |
|---|---|
| `+` | 3V3 (never 5 V: the output rises to the supply voltage) |
| `-` | GND |
| `DO` | GPIO20 |

In `nodes.toml` the same is `{ id = 10, pin = 20, driver = "contact" }`, with `pull_up` (on by
default, not available on the classic ESP32's input-only pins) and `inverted` (off by default).
The driver also takes any other contact wired between the pin and GND.

Channel 0, `Closed` (`Bool`), is true while the pin is low (high when `inverted`), debounced for
50 ms and reported as soon as it changes. A rule fires on it with `equals`, and `for` covers "open
for 10 minutes":

```toml
when = { device = "esp32-e8f60afbdf9c", entity = 10, channel = 0, equals = false, for = "10m" }
```

## GPS module

GY-NEO6MV2 (u-blox NEO-6M), driver `gps`, on either chip. It sends NMEA at 9600 baud once a
second and needs no command, so only its TX is wired: module `VCC` to 3V3 (or 5 V, the module has
its own regulator), `GND` to GND, `TX` to the board's RX pin (`pin` in `nodes.toml`); its `RX`
stays loose. The logic is 3.3 V, no divider. Suggested pins: GPIO16 on the classic ESP32 (an
input-only pin, 34 to 39, works too) and GPIO5 or GPIO20 on the NanoESP32-C6. It uses UART1, so
a board takes one `gps`.

```toml
{ id = 8, pin = 16, driver = "gps" }   # report_interval_seconds = 30 by default, 2 to 3600
```

Channels: 0 fix (`Bool`), 1 satellites, 2 latitude and 3 longitude (°), 4 altitude (m above sea
level), 5 speed (km/h), 6 UTC time (unix time). Positions, altitude, speed and time are left out
without a fix. The fix coming or going is reported at once; the rest every
`report_interval_seconds`, when something changed. Indoors it needs a window: a cold start takes
from 30 s to a few minutes, and the blue LED on the module blinks once it has a fix.
