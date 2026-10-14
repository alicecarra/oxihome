#!/usr/bin/env bash
# Takes the hub's place: stops the production hub on docker-host, brings its configuration
# (into hub/config, so git shows what changed there) and database (hub/oxihome.sqlite) here and
# runs the hub from the source, connected to the production broker. Run again to restart the hub
# without fetching anything; deploy.sh hub sends the state back and ends the session.
set -euo pipefail
cd "$(dirname "$0")/../.."
server=${OXIHOME_SERVER:-docker-host}
remote=oxihome

if ! ssh "$server" test -e "$remote/hub/dev-session"; then
    if [ -n "$(git status --porcelain -- hub/config)" ]; then
        echo "hub/config has uncommitted changes, which the server's configuration would replace" >&2
        exit 1
    fi
    ssh "$server" "cd $remote && docker compose stop hub && touch hub/dev-session"
    rsync -a --delete "$server:$remote/hub/config/" hub/config/
    rm -f hub/oxihome.sqlite*
    rsync -a "$server:$remote/hub/oxihome.sqlite*" hub/
    echo "dev session started; deploy.sh hub ends it"
fi

exec cargo run -p oxihome-hub -- hub/hub.toml
