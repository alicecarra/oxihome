# ESP-NOW leaves and gateway

A leaf board stays off the Wi-Fi network and the broker: it talks over ESP-NOW (frames straight
between ESP32 radios) to a gateway board, which is on the broker and publishes the leaf's messages
on `oxihome/<leaf>/up` and sends it what comes on `oxihome/<leaf>/down` and on the broadcast. The
hub sees the leaf as any other node. Today the gateway is the second NanoESP32-C6
(`esp32-e8f60afb3ddc`) and its leaf is the C6 board (`esp32-e8f60af8b410`).

The gateway's board file gets its leaves and a secret:

```sh
openssl rand -hex 16   # the gateway's espnow_secret
```

```toml
espnow_leaves = "esp32-e8f60af8b410"
espnow_secret = "<32 hex digits>"
```

The leaf's board file gets its gateway and its key, which `espnow-key.sh` derives from the
gateway's secret (the gateway computes the same one):

```sh
firmware/espnow-key.sh esp32-e8f60af8b410   # prints the espnow_key line
```

```toml
espnow_gateway = "esp32-e8f60afb3ddc"
espnow_key = "<printed by espnow-key.sh>"
```

The leaf keeps `wifi_ssid` and `wifi_password`: it joins the Wi-Fi only to download an update, so
an update fails when the leaf is out of the Wi-Fi's reach (flash it by cable then). It does not use
`mqtt_url` or `mqtt_password`.

The broker must let the gateway publish and read for its leaves: a block per gateway at the end of
`infra/mosquitto/acl`, one pair of lines per leaf, copied to the server like the rest of the broker
files (then `docker kill -s HUP oxihome-mosquitto` there):

```text
user esp32-e8f60afb3ddc
topic write oxihome/esp32-e8f60af8b410/up
topic read oxihome/esp32-e8f60af8b410/down
```

At start the leaf looks for the gateway on channels 1 to 13 (the gateway is on the router's
channel) and asks it whether it is on the broker; when the gateway connects to the broker it tells
its leaves, and each one sends its `Hello` again. Frames are encrypted with a key per leaf, and a
gateway takes only the leaves in its list (ESP-IDF allows 7 encrypted peers by default). Messages
go in parts of 240 bytes, up to 4 KB. Not done yet: battery leaves that sleep between messages,
the leaf's signal in its heartbeat, and the list of leaves coming from the hub.
