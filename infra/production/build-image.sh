#!/usr/bin/env bash
# Builds the image of a workspace binary, tagged oxihome-<binary>:<version>-<commit>, and prints the tag.
# Usage: build-image.sh <binary>   (hub, weather...)
set -euo pipefail
[ $# -eq 1 ] || { echo "usage: $0 <binary>" >&2; exit 1; }
binary=$1
cd "$(dirname "$0")/../.."

version="$(sed -n 's/^version = "\(.*\)"/\1/p' "$binary/Cargo.toml" | head -n 1)"
commit="$(git rev-parse --short HEAD)"
if [ -n "$(git status --porcelain -- "$binary" protocol node Cargo.toml Cargo.lock)" ]; then
    echo "warning: uncommitted changes in $binary or its dependencies go into the image" >&2
    commit="$commit-dirty"
fi
tag="oxihome-$binary:$version-$commit"

docker build -f infra/production/Dockerfile --build-arg "BINARY=$binary" -t "$tag" . >&2
echo "$tag"
