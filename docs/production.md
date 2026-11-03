# Production

The production stack runs in Docker on `docker-host` (192.168.15.100, reserved in the router's
DHCP; `ssh docker-host` over Tailscale), from `~/oxihome` there. `infra/production/compose.yaml` is
its compose file; secrets and the home's configuration live only on the server. The plan and its
decisions are in the Trilium note "Produção".

## Broker

The broker's configuration is the same as the development one (`infra/mosquitto`). First install,
from the repository root on the notebook:

```sh
ssh docker-host 'mkdir -p ~/oxihome/mosquitto && chmod 700 ~/oxihome'
scp infra/production/compose.yaml docker-host:oxihome/compose.yaml
scp -p infra/mosquitto/{mosquitto.conf,acl,set-password.sh,watch.sh,observer-password} docker-host:oxihome/mosquitto/
# passwd belongs to the container's mosquitto user (uid 1883): read and write it through the image.
docker run --rm -v "$PWD/infra/mosquitto:/w:ro" eclipse-mosquitto:2 cat /w/passwd \
  | ssh docker-host 'docker run --rm -i -v ~/oxihome/mosquitto:/w eclipse-mosquitto:2 sh -c "cat > /w/passwd && chown mosquitto:mosquitto /w/passwd && chmod 600 /w/passwd"'
ssh docker-host 'cd ~/oxihome && docker compose up -d'
```

The container keeps the name `oxihome-mosquitto`, so the scripts work on the server as they do
locally: `ssh docker-host oxihome/mosquitto/set-password.sh <device id>` and
`ssh -t docker-host oxihome/mosquitto/watch.sh`. Users created there exist only on the server's broker.

The broker of `infra/compose.yaml` on the notebook is no longer used day to day (every board and
node points to docker-host); it stays for working offline.

## Images

`infra/production/Dockerfile` builds one image per workspace binary (`--build-arg BINARY=hub`
builds `oxihome-hub`), on Debian trixie (without `tzdata`: the hub carries its own time zone database). Images are built
on the notebook (the server is slower) and sent over ssh; there is no registry:

```sh
infra/production/build-image.sh hub    # prints the tag: oxihome-hub:<version>-<commit>[-dirty]
```

On the server, `oxihome-<binary>:current` is the image the compose file runs; the versioned tags
stay there, so going back is `docker tag oxihome-hub:<old tag> oxihome-hub:current` and
`docker compose up -d hub`.

## Deploy

```sh
infra/production/deploy.sh weather           # build, send, restart
infra/production/deploy.sh hub
infra/production/deploy.sh hub --no-build    # restart with the image already there
```

For the hub it also backs up the database into `~/oxihome/hub/backups` (the last 10) and replaces
`~/oxihome/hub/firmware` with `hub/firmware` (the over-the-air images). It refuses to run while a
hub runs on the notebook. The scripts copy with rsync, needed on both machines (on the server:
`sudo apt install rsync`).

Files on the server, each made once by hand: `~/oxihome/hub/hub.toml` (a copy of
`infra/production/hub.toml`: broker `mosquitto`, web interface on port 8090 since the Trilium has
8080, images on 8081), `~/oxihome/hub/secrets.toml` (a copy of `hub/secrets.toml`, with the
production `[http] password_hash`, see [Login](hub/running.md#login)) and
`~/oxihome/weather/weather.toml` (the local one with `host = "mosquitto"`) and
`~/oxihome/network-devices/network-devices.toml` (the local one with `host = "localhost"`: it runs
on the host's network to read its ARP table) and `~/oxihome/telegram/telegram.toml` (the production
bot's token, `host = "mosquitto"`) and `~/oxihome/rss/rss.toml` (`host = "mosquitto"`; the node
writes `rss-state.json` there, so that volume is not read only) and `~/oxihome/youtube/youtube.toml`
(the same for the YouTube instance) and `~/oxihome/printer/printer.toml` (`host = "mosquitto"` under `[mqtt]` only; the printer keeps its address) and `~/oxihome/reminders/reminders.toml` (`host = "mosquitto"`; the node writes `reminders-state.json` there, so that volume is not read only either). The web interface is
at <https://oxihome.alicecarra.com>, through Caddy (see [HTTPS](#https)); the hub publishes 8090 only on
the server's localhost. The containers run as uid 1000, the owner of `~/oxihome`.

```sh
ssh docker-host 'mkdir -p ~/oxihome/hub/config ~/oxihome/hub/firmware ~/oxihome/weather && chmod 700 ~/oxihome/hub ~/oxihome/weather'
scp infra/production/hub.toml docker-host:oxihome/hub/hub.toml
scp -p hub/secrets.toml docker-host:oxihome/hub/secrets.toml
sed 's/^host = "192.168.15.100"/host = "mosquitto"/' weather/weather.toml | ssh docker-host 'umask 077; cat > ~/oxihome/weather/weather.toml'
ssh docker-host 'mkdir -p ~/oxihome/network-devices && chmod 700 ~/oxihome/network-devices'
sed 's/^host = "192.168.15.100"/host = "localhost"/' network-devices/network-devices.toml | ssh docker-host 'umask 077; cat > ~/oxihome/network-devices/network-devices.toml'
ssh docker-host 'mkdir -p ~/oxihome/telegram && chmod 700 ~/oxihome/telegram'
ssh docker-host oxihome/mosquitto/set-password.sh telegram   # the password for [mqtt] password
sed 's/^host = .*/host = "mosquitto"/' telegram/telegram.toml | ssh docker-host 'umask 077; cat > ~/oxihome/telegram/telegram.toml'
infra/production/deploy.sh telegram
ssh docker-host 'mkdir -p ~/oxihome/rss && chmod 700 ~/oxihome/rss'
ssh docker-host oxihome/mosquitto/set-password.sh rss   # the password for [mqtt] password
sed 's/^host = .*/host = "mosquitto"/' rss/rss.toml | ssh docker-host 'umask 077; cat > ~/oxihome/rss/rss.toml'
infra/production/deploy.sh rss
ssh docker-host 'mkdir -p ~/oxihome/youtube && chmod 700 ~/oxihome/youtube'
ssh docker-host oxihome/mosquitto/set-password.sh youtube   # the password for [mqtt] password
sed 's/^host = .*/host = "mosquitto"/' rss/youtube.toml | ssh docker-host 'umask 077; cat > ~/oxihome/youtube/youtube.toml'
infra/production/deploy.sh youtube   # the rss image; deploy.sh rss does not restart this one
ssh docker-host 'mkdir -p ~/oxihome/printer && chmod 700 ~/oxihome/printer'
ssh docker-host oxihome/mosquitto/set-password.sh printer   # the password for [mqtt] password
sed '0,/^host = /s/^host = .*/host = "mosquitto"/' printer/printer.toml | ssh docker-host 'umask 077; cat > ~/oxihome/printer/printer.toml'
infra/production/deploy.sh printer
ssh docker-host 'mkdir -p ~/oxihome/reminders && chmod 700 ~/oxihome/reminders'
ssh docker-host oxihome/mosquitto/set-password.sh reminders   # the password for [mqtt] password
sed 's/^host = .*/host = "mosquitto"/' reminders/reminders.toml | ssh docker-host 'umask 077; cat > ~/oxihome/reminders/reminders.toml'
infra/production/deploy.sh reminders
```

The first hub deploy took the notebook's configuration and database by faking a dev session
(`ssh docker-host touch oxihome/hub/dev-session` before `deploy.sh hub`).

## HTTPS

Caddy, a stack of its own in `~/caddy` on docker-host (not part of this repository), serves the web
interface at <https://oxihome.alicecarra.com> and passes it to the hub at `localhost:8090`. It
runs on the host's network, so it can reach any service published on localhost without joining
its network. Files: `docker-compose.yml`, a `Dockerfile` that builds the official image with the
Cloudflare DNS plugin (`xcaddy build --with github.com/caddy-dns/cloudflare`), `conf/Caddyfile`
(one block per site) and `.env` (mode 600) with `CLOUDFLARE_API_TOKEN`. The certificates come from
Let's Encrypt through the DNS challenge, so nothing is open to the internet.

At Cloudflare: an API token from the "Edit zone DNS" template plus Zone / Zone / Read, limited to
the `alicecarra.com` zone, and an A record `oxihome` → `192.168.15.100`, DNS only (grey cloud). Caddy
took ports 80 and 443 from the pihole, which is stopped for now (`~/pihole`).

```sh
ssh docker-host 'cd ~/pihole && docker compose stop'
ssh docker-host 'cd ~/caddy && docker compose up -d --build'
ssh docker-host 'cd ~/caddy && docker compose logs -f'       # wait for the certificate
ssh docker-host 'cd ~/caddy && docker compose exec caddy caddy reload --config /etc/caddy/Caddyfile'   # after editing the Caddyfile
```

Away from home the same address works over Tailscale: docker-host advertises a route to its own LAN
address, approved in the admin console (Machines → docker-host → Edit route settings). Phones, macOS
and Windows accept routes by default (Android: "Use Tailscale subnets"); Linux needs
`--accept-routes`.

```sh
ssh -t docker-host 'sudo tailscale set --advertise-routes=192.168.15.100/32'
sudo tailscale set --accept-routes    # on a Linux client used away from home
```

## Working on the hub

Only one hub may run per broker, so the notebook's hub takes the production one's place:

```sh
infra/production/dev-hub.sh
```

It stops the hub on the server, brings its configuration into `hub/config` (so `git diff` shows what
changed in production; it refuses to run with uncommitted changes there) and its database into
`hub/oxihome.sqlite`, and runs `cargo run -p oxihome-hub -- hub/hub.toml`, whose broker is
docker-host's. Stop it with Ctrl+C and run it again as often as needed: only the first run fetches.
`deploy.sh hub` ends the session: it sends the configuration and the database back and starts the
production hub with the new image. `deploy.sh hub --no-build` returns without a new image, and
`--discard` drops the session's configuration and database instead of sending them. While the
session is open the house depends on the notebook's hub.

The database goes back already migrated by the session's hub, so end a session that ran a newer
version with a deploy of that version, not `--no-build`.
