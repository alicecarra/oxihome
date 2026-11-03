# Printer node

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
