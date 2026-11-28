#!/usr/bin/env sh
# Unfree apps: not in the binary cache, so nami builds them from the
# catalog's build closures (index/closures.py) - for users via the daemon.
# usage: tests/unfree.sh <catalog dir with index-x86_64-linux.json.xz and drvs/>
set -eu
cd "$(dirname "$0")/.."
PATH="$PWD/tests/lib:$PATH"   # docker, or rootless podman where there is none
cat=$(realpath "${1:?catalog dir}")
docker run --rm --privileged -v "$PWD/target/release/nami":/new/nami:ro -v "$PWD":/src:ro -v "$cat":/cat:ro \
  -v "$PWD/tests/unfree-inner.sh":/run.sh:ro ubuntu:24.04 bash /run.sh
