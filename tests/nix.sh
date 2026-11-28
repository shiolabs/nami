#!/usr/bin/env sh
# nami next to an installed Nix: Nix's store is shared, never overwritten,
# and each side's garbage collector keeps what the other uses.
# Ubuntu with Nix copied in from the nixos/nix image (store, db, config),
# once with a writable store and once read-only, as NixOS mounts it.
# usage: tests/nix.sh <index.json.xz>
set -eu
cd "$(dirname "$0")/.."
PATH="$PWD/tests/lib:$PATH"   # docker, or rootless podman where there is none
index=$(realpath "${1:?index}")
docker volume create nami-nix-seed >/dev/null
docker run --rm -v nami-nix-seed:/seed nixos/nix:latest sh -c '
  [ -e /seed/nix/var/nix/db/db.sqlite ] && exit 0
  cp -a /nix /seed/ && mkdir -p /seed/etc && cp -aL /etc/nix /seed/etc/
  ln -sfn "$(dirname "$(dirname "$(readlink -f "$(command -v nix-store)")")")" /seed/nix/var/nix/profiles/default'
paths=$(python3 -I -c 'import json,lzma,sys; d=json.load(lzma.open(sys.argv[1])); print(" ".join(d[a]["outputs"]["out"] for a in ("ripgrep","hello","tree")))' "$index")
# In a file, not the environment: Nix counts paths in a process's
# environment as in use, so PID 1 would keep them alive.
pathfile=$(mktemp); set -- $paths; printf 'RG=%s\nHELLO=%s\nTREE=%s\n' "$1" "$2" "$3" > "$pathfile"
for ro in 0 1; do
  echo "== /nix/store $( [ $ro = 1 ] && echo 'read-only (as on NixOS)' || echo writable)"
  docker run --rm --privileged -e READONLY=$ro -v "$pathfile":/paths:ro -v "$PWD/target/release/nami":/nami-bin:ro -v "$PWD":/src:ro -v "$index":/i.xz:ro \
  -v nami-nix-seed:/seed:ro -v "$PWD/tests/nix-inner.sh":/run.sh:ro ubuntu:24.04 bash /run.sh
done
rm -f "$pathfile"
