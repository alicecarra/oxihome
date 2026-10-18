#!/usr/bin/env bash
# Deploys a service of the production stack on docker-host: builds its image here, loads it there
# and restarts the service with it.
# Usage: deploy.sh <service> [--no-build] [--discard]
#   --no-build  keeps the image running there (with the hub: just ends a dev session)
#   --discard   hub only: drops the dev session's configuration and database instead of sending them
#
# The hub's configuration (hub/config) and database (hub/oxihome.sqlite) go to the server when a
# dev session is open (dev-hub.sh); the database there is backed up first either way.
set -euo pipefail
cd "$(dirname "$0")/../.."
server=${OXIHOME_SERVER:-docker-host}
remote=oxihome

service=
build=1
discard=
for argument in "$@"; do
    case $argument in
        --no-build) build= ;;
        --discard) discard=1 ;;
        -*) echo "unknown option $argument" >&2; exit 1 ;;
        *) service=$argument ;;
    esac
done
[ -n "$service" ] || { echo "usage: $0 <service> [--no-build] [--discard]" >&2; exit 1; }
# The youtube service is a second rss node.
case $service in
    youtube) binary=rss ;;
    *) binary=$service ;;
esac

if [ "$service" = hub ] && pgrep -x oxihome-hub >/dev/null; then
    echo "a hub is running here: stop it first" >&2
    exit 1
fi

if [ -n "$build" ]; then
    tag="$(infra/production/build-image.sh "$binary")"
    echo "sending $tag"
    docker save "$tag" | ssh "$server" docker load
    ssh "$server" docker tag "$tag" "oxihome-$binary:current"
fi
scp -q infra/production/compose.yaml "$server:$remote/compose.yaml"

if [ "$service" = hub ]; then
    ssh "$server" "set -e
        cd $remote
        docker compose stop hub
        backup=hub/backups/\$(date +%Y%m%d-%H%M%S)
        mkdir -p \$backup
        cp -p hub/oxihome.sqlite* \$backup/ 2>/dev/null || true
        ls -1d hub/backups/* | head -n -10 | xargs -r rm -r"
    rsync -a --delete hub/firmware/ "$server:$remote/hub/firmware/"
    if ssh "$server" test -e "$remote/hub/dev-session"; then
        if [ -z "$discard" ]; then
            echo "sending the dev session's configuration and database"
            rsync -a --delete hub/config/ "$server:$remote/hub/config/"
            ssh "$server" "rm -f $remote/hub/oxihome.sqlite*"
            rsync -a hub/oxihome.sqlite* "$server:$remote/hub/"
        fi
        ssh "$server" rm "$remote/hub/dev-session"
    fi
fi

ssh "$server" "cd $remote && docker compose up -d $service"
