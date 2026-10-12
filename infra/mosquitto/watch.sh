#!/bin/sh
# Prints the messages on the broker as the read-only observer: `infra/mosquitto/watch.sh ['oxihome/#']`.
# The password goes through an options file, never the command line, where `ps` would show it.
set -eu
directory=$(cd "$(dirname "$0")" && pwd)
printf -- '-u observer\n-P %s\n' "$(cat "$directory/observer-password")" |
    docker exec -i oxihome-mosquitto sh -c 'mkdir -p -m 700 /tmp/observer && cat > /tmp/observer/mosquitto_sub'
# With a terminal, -it makes Ctrl+C stop mosquitto_sub inside the container too.
terminal=
[ -t 0 ] && terminal=-it
exec docker exec $terminal -e XDG_CONFIG_HOME=/tmp/observer oxihome-mosquitto mosquitto_sub -t "${1:-oxihome/#}" -v
