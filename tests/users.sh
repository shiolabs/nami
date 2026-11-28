#!/usr/bin/env sh
# Generations (diff, rollback, caches, pruning) and per-user profiles through
# the daemon, including what a user must NOT be able to do.
# usage: tests/users.sh <index.json.xz> <drvs-dir>
set -eu
cd "$(dirname "$0")/.."
PATH="$PWD/tests/lib:$PATH"   # docker, or rootless podman where there is none
index=$(realpath "${1:?index}"); drvs=$(realpath "${2:?drvs dir}")
dri=""; [ -e /dev/dri ] && dri="--device /dev/dri"
docker run --rm --privileged $dri -v "$PWD/target/release/nami":/new/nami:ro -v "$PWD":/src:ro \
  -v "$PWD/examples/packages":/ex:ro -v "$index":/i.xz:ro -v "$drvs":/drvs:ro \
  -v "$PWD/tests/users-inner.sh":/run.sh:ro ubuntu:24.04 bash /run.sh
