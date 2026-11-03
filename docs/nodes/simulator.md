# Simulator

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

## Controls in the hub

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
