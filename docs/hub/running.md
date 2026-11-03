# Running the hub

Connects to the broker configured in `hub/hub.toml` with the password in `hub/secrets.toml` (out
of git; `[mqtt] secrets` points to it, and without that line the hub connects anonymously):

```sh
cp hub/secrets.toml.example hub/secrets.toml   # then set the password of oxihome-hub
chmod 600 hub/secrets.toml
```

It asks every node for its `Hello` and `State`,
tracks which nodes are online (offline after `[presence] missed_heartbeats`, default 3, times the
node's heartbeat interval without messages; changing it needs a hub restart) and serves a web interface
(Topcoat) on the `[http]` address, by default <http://127.0.0.1:8080>:

```sh
cargo run -p oxihome-hub -- hub/hub.toml
```

Requests must name the hub in `Host` (`localhost`, `127.0.0.1`, `[::1]`, the `[http] host` or one
of `[http] allowed_hosts`), otherwise 403. A hub listening on `0.0.0.0` lists the names it is
reached by, e.g. `allowed_hosts = ["docker-host", "192.168.15.100"]`; the notebook's
`hub/hub.toml` lists its Wi-Fi and Tailscale addresses, so a tablet reaches a dev hub at
<http://192.168.15.10:8080>. firewalld blocks 8080 from the network; open it until the next reboot
or firewalld reload with:

```sh
sudo firewall-cmd --add-port=8080/tcp
```

Without the check, a site
pointing its own domain at 127.0.0.1 (DNS rebinding) would pass as same-origin. Topcoat already
rejects POSTs from other sites (`Sec-Fetch-Site`/`Origin`).

## Login

Since hub 0.5.0 the web interface asks for a password, whose argon2 hash goes in the secrets file
as `[http] password_hash`. The hub refuses to start without it unless `[http] host` is localhost.
Make the hash (the password is read from the first line of standard input):

```sh
cargo run -q -p oxihome-hub -- hash-password
```

Sessions last 30 days, extended while in use, and are kept in the database (table `sessions`), so
a restart or deploy keeps everyone signed in. "Sign out" in the menu ends the current one. After 10
wrong passwords in a minute the login refuses every attempt for the rest of that minute.

The session cookie is `Secure`, so the browser keeps it only over HTTPS or on localhost: a dev hub
at <http://localhost:8080> works, but at <http://192.168.15.10:8080> the login seems to do nothing
(the login page says so).

Only one hub can run per broker: both use the MQTT client id `oxihome-hub` and keep disconnecting
each other.

`GET /health` needs no login and tells nothing about the home: `200 ok` while the hub is connected
to the broker, `503 broker disconnected` otherwise. `oxihome-hub health hub/hub.toml` asks it on the
port of that config (on loopback when `[http] host` is `0.0.0.0`) and exits with 1 unless the
answer is 200; the production compose file uses it as the hub's Docker healthcheck, every 30 s, so
`docker ps` shows the hub as healthy or unhealthy.

Ctrl+C or `SIGTERM` stops the hub at once: it logs `shutting down`, stops handling messages and
rules, closes the pages' event streams and gives other requests up to 3 s to finish. (Before
2026-10-12 it took 30 s with a page open, waiting for the event streams, which never end on their
own.) Before exiting it waits for the pending database writes.
