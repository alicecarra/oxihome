# Linux monitor

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
