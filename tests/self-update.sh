#!/usr/bin/env sh
# nami self-update end to end: a local release server with a newer, signed
# release. An unsigned or altered one is refused; the real one installs,
# for root and for an admin (through the daemon). Its notes (what's new) come
# with the check only as the signed sums have them. The daily update's timer
# (no systemd here: its files) comes with the install, goes and comes back
# with system.nami's `updates:`, and comes to a computer that had none as it
# updates. A package of the computer's own (/etc/nami/packages), changed, is
# built anew by `nami install` of it and by `nami apply`; and the update brings
# nami's window (nami-gui) at the version that goes with it (from namios.dev).
# usage: tests/self-update.sh <index.json.xz>
set -eu
cd "$(dirname "$0")/.."
PATH="$PWD/tests/lib:$PATH"   # docker, or rootless podman where there is none
index=$(realpath "${1:?index}")
docker run --rm --privileged -v "$PWD/target/release/nami":/new/nami:ro -v "$PWD":/src:ro -v "$index":/i.xz:ro \
  ubuntu:24.04 bash /src/tests/self-update-inner.sh
