#!/bin/sh
# Sets a broker user's password (generating one when missing) and reloads the broker.
# Prints the password: put it in the node's configuration.
set -eu
[ $# -ge 1 ] || { echo "usage: $0 <user> [password]" >&2; exit 1; }
user=$1
password=${2:-$(openssl rand -hex 16)}
directory=$(cd "$(dirname "$0")" && pwd)
[ -e "$directory/passwd" ] || touch "$directory/passwd"
docker run --rm -v "$directory:/w" eclipse-mosquitto:2 sh -c \
    'mosquitto_passwd -b /w/passwd "$1" "$2" && chown mosquitto:mosquitto /w/passwd && chmod 600 /w/passwd' \
    sh "$user" "$password" >/dev/null 2>&1
docker kill -s HUP oxihome-mosquitto >/dev/null 2>&1 || true
echo "$password"
