# Network devices node

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
