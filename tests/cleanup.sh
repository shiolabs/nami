#!/usr/bin/env sh
# Old changes clean up by themselves (`cleanup:`, src/cleanup.rs): install,
# remove (the hint says what it can free), then, the clock moved on, the
# daily cleanup (`nami gc --auto`, as root) takes the old changes, the
# computer's and a person's (as her), and what only they used; it keeps the
# current change and the newest, a project's pinned tools, and what a running
# program uses. Undo past it says so; low space cleans up early. On Ubuntu,
# in a container (namiOS's own systems: the unit tests and the VM's).
# usage: tests/cleanup.sh [index.json(.xz)] (default: this computer's catalog)
set -eu
cd "$(dirname "$0")/.."
PATH="$PWD/tests/lib:$PATH"   # docker, or rootless podman where there is none
index=$(realpath "${1:-/nix/var/nami/index.json}")
case $index in *.xz) at=/i.json.xz ;; *) at=/i.json ;; esac
docker run --rm --privileged -v "$PWD/target/release/nami":/new/nami:ro -v "$PWD":/src:ro -v "$index":$at:ro -e INDEX=$at \
  -v "$PWD/tests/cleanup-inner.sh":/run.sh:ro ubuntu:24.04 bash -c 'apt-get -qq update >/dev/null && apt-get -qq install -y jq xz-utils >/dev/null && bash /run.sh'
