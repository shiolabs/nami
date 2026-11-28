#!/usr/bin/env sh
# The one-file installer (packaging/bundle.sh) on a fresh Ubuntu, on Ubuntu
# with Nix, and on a stand-in for NixOS (Nix + /etc/NIXOS, so the module
# path runs; NixOS itself isn't a container image).
# usage: [DOCKER_ARGS=...] tests/bundle.sh dist/nami-<version>-x86_64-linux.run
set -eu
cd "$(dirname "$0")/.."
PATH="$PWD/tests/lib:$PATH"   # docker, or rootless podman where there is none
run=$(realpath "${1:?installer}")
docker volume create nami-nix-seed >/dev/null
docker run --rm -v nami-nix-seed:/seed nixos/nix:latest sh -c '
  [ -e /seed/nix/var/nix/db/db.sqlite ] && exit 0
  cp -a /nix /seed/ && mkdir -p /seed/etc && cp -aL /etc/nix /seed/etc/
  ln -sfn "$(dirname "$(dirname "$(readlink -f "$(command -v nix-store)")")")" /seed/nix/var/nix/profiles/default'
fail=0
for mode in plain nix nixos; do
  echo "== $mode"
  docker run --rm --privileged ${DOCKER_ARGS:-} -e MODE=$mode -v "$run":/nami.run:ro -v nami-nix-seed:/seed:ro \
    -v "$PWD/tests/bundle-inner.sh":/run.sh:ro ubuntu:24.04 bash /run.sh || fail=$((fail + 1))
done
[ "$fail" -eq 0 ]
