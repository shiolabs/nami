#!/usr/bin/env sh
# The computer's apps and admins: an admin (bob, in the sudo group) changes
# /etc/nami/system.nami without sudo, through the daemon; someone who isn't
# (alice) still installs her own apps, and can't change the computer's.
# usage: tests/admin.sh <index.json.xz>
set -eu
cd "$(dirname "$0")/.."
PATH="$PWD/tests/lib:$PATH"   # docker, or rootless podman where there is none
index=$(realpath "${1:?index}")
docker run --rm --privileged -v "$PWD/target/release/nami":/new/nami:ro -v "$PWD":/src:ro -v "$index":/i.xz:ro \
  ubuntu:24.04 bash /src/tests/admin-inner.sh
