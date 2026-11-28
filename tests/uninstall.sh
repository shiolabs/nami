#!/usr/bin/env sh
# install.sh + nami install + uninstall.sh leaves the machine as it was,
# including a store path that nami didn't create (e.g. from a Nix install)
# and a file in /etc nami wrote over (back as it was); /etc/nami, the
# person's own files, stays.
# usage: tests/uninstall.sh <index.json.xz>
set -eu
cd "$(dirname "$0")/.."
PATH="$PWD/tests/lib:$PATH"   # docker, or rootless podman where there is none
index=$(realpath "${1:?index}")
docker run --rm -v "$PWD":/src:ro -v "$PWD/target/release/nami":/new/nami:ro -v "$index":/i.xz:ro ubuntu:24.04 sh -euc '
  mkdir -p /nix/store/00000000000000000000000000000000-not-namis
  /src/install.sh /new/nami >/dev/null
  nami update --from /i.xz --unsigned >/dev/null
  nami install hello >/dev/null 2>&1
  # A file nami writes over one that was there, and one of its own.
  echo original > /etc/demo.conf
  printf "files:\n  /etc/demo.conf: from nami\n  /etc/nami-only.conf: mine\n" > /etc/nami/system.nami
  nami apply >/dev/null 2>&1
  [ "$(cat /etc/demo.conf)" = "from nami" ] || { echo "FAIL: nami apply did not write /etc/demo.conf"; exit 1; }
  echo "before: $(ls /nix/store | wc -l) store entries, $(ls /etc/profile.d/nami.sh /etc/tmpfiles.d/nami.conf | wc -l) config files"
  /src/uninstall.sh
  left=$(ls /nix/store)
  [ "$left" = 00000000000000000000000000000000-not-namis ] || { echo "FAIL: store has: $left"; exit 1; }
  for f in /nami /nix/var/nami /etc/profile.d/nami.sh /etc/environment.d/50-nami.conf /etc/systemd/user-environment-generators/50-nami /etc/tmpfiles.d/nami.conf /usr/local/bin/nami /run/opengl-driver /usr/local/share/bash-completion/completions/nami; do
    if [ -e "$f" ] || [ -L "$f" ]; then echo "FAIL: $f still exists"; exit 1; fi
  done
  [ "$(cat /etc/demo.conf)" = original ] && [ ! -e /etc/demo.conf.nami-backup ] || { echo "FAIL: /etc/demo.conf is not back as it was"; exit 1; }
  [ ! -e /etc/nami-only.conf ] || { echo "FAIL: /etc/nami-only.conf is still there"; exit 1; }
  [ -f /etc/nami/system.nami ] || { echo "FAIL: /etc/nami/system.nami (theirs) went"; exit 1; }
  echo PASS'
