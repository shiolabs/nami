#!/usr/bin/env sh
# install.sh over a tray already installed (docs/rename-migration.md), on
# Ubuntu: tray's installer and tray (static, of the release before nami's
# name), a person's app and the computer's, then this repo's install.sh with
# nami. tray's places are nami's, with the old names pointing there, both
# commands work, and the older tray still finds everything.
# usage: tests/rename-install.sh <tray's static build> [its release's commit (default 321fd30a)]
#        (`nami run static` first: target/release/nami)
set -eu
cd "$(dirname "$0")/.."
PATH="$PWD/tests/lib:$PATH"   # docker, or rootless podman where there is none
old=$(realpath "${1:?a static tray}")
rev=${2:-321fd30a}
W=$(mktemp -d)
trap 'rm -rf "$W"' EXIT
git archive "$rev" install.sh > "$W/old.tar" && tar xf "$W/old.tar" -C "$W" && mv "$W/install.sh" "$W/old-install.sh"
docker run --rm --privileged -v "$PWD/target/release/nami":/new/nami:ro -v "$old":/tray:ro -v "$W/old-install.sh":/old-install.sh:ro -v "$PWD":/src:ro \
  ubuntu:24.04 bash /src/tests/rename-install-inner.sh
