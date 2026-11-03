# Node library

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
