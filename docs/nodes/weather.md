# Weather node

`weather/` (`oxihome-weather`) is a software node with the weather outside, from
[Open-Meteo](https://open-meteo.com) (free, no account or key). It fetches every 15 minutes on the
clock (a minute after each quarter-hour, when Open-Meteo has new current values; `[fetch]
interval`, 5 to 60 minutes) and sends a heartbeat every 30 s. A failed fetch keeps the last
readings and is tried again after 1, 2, 4, 8... minutes, up to the interval; Open-Meteo counts as
unreachable once the data is 30 minutes old. It only publishes the entities whose values changed.

```sh
cp weather/weather.toml.example weather/weather.toml   # then set the location
chmod 600 weather/weather.toml
cargo run -p oxihome-weather -- weather/weather.toml
```

`weather/weather.toml` stays out of git because it holds the location and the broker password
(`[mqtt] password`, user `weather` or the `[node] id`); a public place near home is
precise enough. The node refuses to start with a latitude or longitude out of range.

Entities, all read only:

| Entity | Channels |
|---|---|
| 1 Now | 0 temperature °C, 1 humidity %, 2 precipitation mm over the last 15 minutes, 3 wind km/h, 4 raining `Bool` (drizzle, rain, showers or thunderstorm in the WMO weather code), 5 feels like °C, 6 gusts km/h, 7 condition `Text` ("Partly cloudy"), 8 daytime `Bool` (between sunrise and sunset; flips on time, between fetches) |
| 2 Today | 0 maximum °C, 1 minimum °C, 2 chance of rain %, 3 condition `Text`, 4 UV index, 5 sunrise and 6 sunset (`UnixTime`) |
| 4 Connection | 0 reachable `Bool`, 1 data age (s), 2 updated at: when Open-Meteo observed the current values (`UnixTime`) |
| 5 Next hour | 0 precipitation mm forecast, 1 chance of rain %, 2 rain expected `Bool` (at least 0.1 mm) |
| 6 Tomorrow | as Today |

Entity 3 (`Updated`, the local time of the last reading as `Text`) went away in 0.4.2: its time is
now `Updated at` in Connection. Its id stays unused.

Rules use them like any sensor, with `above`/`below` limits (e.g. precipitation above 0 to say it
started raining) or `equals` on the flags (daytime equals false to turn a light on at sunset, rain
expected equals true for a warning). The protocol gained the `Precipitation` and `WindSpeed`
quantities and the `Millimeter` and `KilometerPerHour` units for it.
