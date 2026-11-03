# Over-the-air updates

Each board has its own configuration in `firmware/boards/<device id>.toml` (ignored by git), with
the same keys as `cfg.toml`. After changing the firmware and its version in `firmware/Cargo.toml`:

```sh
firmware/build-images.sh                      # every board
firmware/build-images.sh esp32-14335c2e7270   # or some of them
```

A board with another chip has a `chip` line in its file (`chip = "esp32c6"`); without it the
script builds for the classic ESP32. A board with 16 MB of flash also has `flash = "16mb"`, which
picks its partition table; the script stops if an image does not fit that table's slot and warns
above 90%. The firmware ignores both lines.

The script builds a release image per board into `hub/firmware/<device id>/<version>.bin` (ignored
by git and readable only by the owner: the images hold the passwords) and puts `cfg.toml` back as
it was. The hub serves them when `hub.toml` has an `[ota]` section:

```toml
[ota]
listen = "0.0.0.0:8081"                     # reachable by the boards
public_url = "http://192.168.15.10:8081"    # how the boards reach it
images = "hub/firmware"
```

With firewalld (the laptop has it, `wlan0` in the `public` zone), the boards cannot reach port
8081 until it is open to the home network. The MQTT port works without this because Docker opens
its published ports itself. Until the next reboot:

```sh
sudo firewall-cmd --zone=public --add-rich-rule='rule family="ipv4" source address="192.168.15.0/24" port port="8081" protocol="tcp" accept'
```

Add `--permanent` and run `sudo firewall-cmd --reload` to keep it. Without the rule, the board
reports `UpdateDownloadFailed` (`ESP_ERR_HTTP_CONNECT`) after about 18 s and stays on its version.

The production hub on docker-host serves the images itself (`public_url` on 192.168.15.100) and
needs none of this: Docker publishes 8081 there. Without sudo on the notebook, a relay in Docker
gets past firewalld the same way (`docker run --rm -d --name ota-relay --add-host
host.docker.internal:host-gateway -p 8082:8082 alpine/socat TCP-LISTEN:8082,fork,reuseaddr
TCP:host.docker.internal:8081`, with `public_url` on port 8082).

That server is separate from the web interface, which stays private (`127.0.0.1` on the notebook;
in production `0.0.0.0` inside the container, published only on docker-host's localhost for
Caddy), and answers only `GET /ota/<token>`: a random link that works once and for 5 minutes.
When an online node that declares the `Ota` channel has an image newer than its firmware, its page
shows **Update to x.y.z**.
The hub sends the link in an `Ota` command over MQTT; the board answers, downloads the image into
its other app slot and reboots into it. The new image is on trial until it reaches the broker and
sends its `Hello`; if it does not within 60 s, the board restarts and the bootloader goes back to
the previous one. The hub notifies the outcome: updated (the `Hello` has the new version), rolled
back (it has the old one), failed or no answer within 5 minutes. A failure comes as an `Error` of
its own after the command's `Ack`: `UpdateDownloadFailed` (8) when the image could not be
downloaded (no connection, HTTP status, transfer cut), `UpdateRejected` (9) when it failed the
final check (corrupt, or for another chip), `Internal` for a flash write error. Firmware before
0.19.0 sends `Internal` with an "OTA:" detail, which the hub also takes as a failed update.

Each finished update is also stored in the `updates` table, kept until the node is forgotten (see
below): the node page's
**History** section (between the fields and the rules, refreshed live) lists the last 10
(versions, outcome, how long it took), next to the last 10
notifications about the node (offline, back online, rebooted, updates; the reboot of an update is
not notified), which go away with the
rest of the history after `retention_days`.

An offline node's page has a **Forget** button: the hub drops the node from memory and its rows
from `nodes`, `channel_values` and `updates`; its readings, events and commands age out with the
rest of the history, and its names in `nodes.toml` stay. A node that speaks again comes back.
