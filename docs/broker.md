# MQTT broker

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

Then put the password in the node's configuration (each [node's page](README.md#nodes) says where) and keep that
file readable only by its owner (`chmod 600`); `install-as-service.sh` does it for the Linux
monitor's installed copy.

`max_packet_size` drops messages over 64 KiB at the broker (the largest today is a Linux monitor's
`Hello`, about 5.5 KB). The hub accepts up to 128 KiB, so an oversized message never reaches it;
rumqttc's default of 10 KiB would close the hub's connection, and a node could keep it down that
way. Mosquitto logs
warnings about the permissions of `acl` and about the `oxihome/broadcast/down` pattern without
`%u`; both are expected. Without TLS the passwords cross the network in plain text (layer 3 of the
"Segurança" note in Trilium).

## Raw MQTT messages

To see the JSON messages nodes and the hub exchange, subscribe inside the broker container as
`observer`, which reads everything and cannot publish. Its password is in
`infra/mosquitto/observer-password` (out of git); the script passes it through an options file, not
the command line, where `ps` would show it:

```sh
infra/mosquitto/watch.sh                      # everything
infra/mosquitto/watch.sh 'oxihome/+/up'       # one topic filter
```
