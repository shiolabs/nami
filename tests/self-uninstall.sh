#!/usr/bin/env sh
# Uninstalling nami itself, as people do it: installed with the web installer
# (packaging/install-web.sh) from a local release site, on Ubuntu, then
# `sudo nami self-uninstall` (the word `uninstall` typed, or --yes) or the web
# uninstaller (packaging/uninstall-web.sh: `curl .../uninstall | sh`, typed on
# the terminal, or `sh -s -- --yes`). First what must not uninstall: `nami
# uninstall` alone and `nami uninstall <app>` (an app's removal, as `nami
# remove`), `nami self-uninstall firefox`, without sudo, any answer but the
# word, no terminal and no --yes. Then everything nami added is gone, and what
# stays stays: /etc/nami, someone's user.nami, ~/.config/nami and home.
# usage: tests/self-uninstall.sh <index.json.xz> [typed|yes|web|web-yes...]
#        (`nami run static` first: target/release/nami.)
set -u
cd "$(dirname "$0")/.."
PATH="$PWD/tests/lib:$PATH"   # docker, or rootless podman where there is none
index=$(realpath "${1:?index.json.xz}"); shift
bin=${NAMI_BIN:-$PWD/target/release/nami}
[ -x "$bin" ] || { echo "build nami first: nami run static"; exit 1; }
fails=0
for mode in ${*:-typed yes web web-yes}; do
  echo "== $mode"
  docker run --rm --privileged -v "$bin":/new/nami:ro -v "$PWD":/src:ro -v "$index":/i.xz:ro \
    ubuntu:24.04 bash /src/tests/self-uninstall-inner.sh "$mode" || fails=$((fails + 1))
done
echo
[ "$fails" = 0 ] && echo "all passed" || { echo "$fails failed"; exit 1; }
