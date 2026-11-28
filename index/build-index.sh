#!/usr/bin/env sh
# Builds the nami package index for one nixpkgs channel release.
#
# This is the ONLY place Nix runs: on index-build infrastructure, inside a
# throwaway container (or directly, on a machine that has Nix). End users
# download the resulting files and never need Nix installed.
#
# Output:
#   index-x86_64-linux.json(.xz)   attr -> {name, outputs}
#   drvs/<attr>.json               build definition, for `extends:` recipes
#   drvs/<attr>.closure.json       how to build one the binary cache hasn't
#   defs/<shard>.json.xz           all of drvs/ but the closures in one piece, in
#                                  4096 shards by sha256, as published (index/defs.py)
#   icons.tar                      app icons (attr.png / attr.svg)
#   versions/<shard>.json          the versions a package had before, for `firefox: 150`
#
# What's in it: index/catalog.nix.
# usage: [WORKERS=n] [ONLY="rPackages ..."] [SYSTEM=aarch64-linux] index/build-index.sh <release-url> <out-dir>
#   SYSTEM: just another platform's index-<system>.json (in-container.sh), next to x86_64's.
#   ARM Linux's (aarch64-linux, from nixos-unstable as x86_64's) and a Mac's (aarch64-darwin,
#   x86_64-darwin): its own catalog, with descriptions, licenses and which packages aren't
#   ready-built (no icons, definitions or histories yet). A Mac's is made from a nixpkgs with
#   Mac builds (packaging/catalog-update.sh says which); index/check-cached.py checks each
#   package installs from cache.nixos.org.
#   e.g. index/build-index.sh https://releases.nixos.org/nixos/unstable/nixos-26.11pre1086391.151fa4e8ddfd out/
# HISTORY=<dir> keeps what version histories are made from (default
# ~/.cache/nami-history; each nixpkgs release is looked at once, ever);
# HISTORY=no leaves them out. A first run starts from the live catalog's
# histories and adds only the releases since (minutes); SEED=<a menu/<rev>
# folder or URL> starts from another's, SEED=no from scratch (a day or two).
#
# Without Docker or Nix (namiOS), nami brings Nix and the rest (index/project.nami,
# index/nami-env.sh: with a store of its own, never the system's), and it
# all runs at the lowest CPU and disk priority, on half the cores
# (WORKERS), so the computer stays usable through the hours it takes.
set -eu

release=${1:?release url}
out=$(realpath -m "${2:?out dir}")
here=$(cd "$(dirname "$0")" && pwd)
mkdir -p "$out"

if ! command -v docker >/dev/null 2>&1 && ! command -v nix >/dev/null 2>&1 && command -v nami >/dev/null 2>&1; then
  WORKERS=${WORKERS:-$(( $(nproc) / 2 > 0 ? $(nproc) / 2 : 1 ))}
  export WORKERS
  exec nice -n 19 ionice -c3 nami run --project "$here" -- sh "$here/nami-env.sh" sh "$here/build-index.sh" "$release" "$out"
fi

if command -v docker >/dev/null 2>&1; then
  docker run --rm ${WORKERS:+-e WORKERS} ${ONLY:+-e ONLY} ${SYSTEM:+-e SYSTEM} -v "$out":/out -v "$here/in-container.sh":/in-container.sh:ro -v "$here/catalog.nix":/catalog.nix:ro -v "$here/closures.py":/closures.py:ro -v "$here/uncached.py":/uncached.py:ro nixos/nix:latest \
    sh -c "sh /in-container.sh '$release' && chown -R $(id -u):$(id -g) /out"
elif command -v nix >/dev/null 2>&1; then
  # No Docker but Nix is here (NixOS, or nix develop): evaluate directly.
  # The derivations land in the local store; nix-collect-garbage removes them.
  OUT=$out CATALOG=$here/catalog.nix CLOSURES=$here/closures.py UNCACHED=$here/uncached.py sh "$here/in-container.sh" "$release"
else
  echo "needs docker or nix"; exit 1
fi
# Another platform's index is only that (its apps, icons and histories are x86_64's).
if [ "${SYSTEM:-x86_64-linux}" != x86_64-linux ]; then
  xz -T0 -kf "$out/index-$SYSTEM.json"
  ls -la "$out/index-$SYSTEM.json"*
  exit 0
fi
# Which packages are apps, and an icon for each (host-side; needs python3,
# curl, xz and zstd; PyYAML adds Debian's icons).
python3 -I "$here/apps.py" "$out/index-x86_64-linux.json" "$out"
# Their names, kinds, screenshots and how many use them (Flathub, Debian).
python3 -I "$here/meta.py" "$out/index-x86_64-linux.json"
# The rest by what their own desktop files say.
python3 -I "$here/desktop.py" "$out/index-x86_64-linux.json" "$out"
# Every version each package has had (needs Nix: older releases are evaluated).
history=${HISTORY:-$HOME/.cache/nami-history}
if [ "$history" != no ]; then
  # Made from scratch, the histories take a day or two: a first run starts
  # from the published catalog's instead (history.py --seed), and looks at
  # just the releases after it.
  # Only a catalog without histories means from scratch; anything else
  # stops here rather than spend a day or two by mistake.
  if [ ! -e "$history/versions-x86_64-linux.json" ] && [ "${SEED:-}" != no ]; then
    base=${BASE_URL:-https://namios.dev}
    seed=${SEED:-$base/menu/$(curl -fsS "$base/menu/nixos-unstable")}
    seeded=0; python3 -I "$here/history.py" --seed "$seed" "$history" || seeded=$?
    case $seeded in
      0) ;;
      3) echo "no histories to start from: made from scratch (a day or two)" ;;
      *) echo "couldn't start the histories from $seed (SEED=no: from scratch)"; exit 1 ;;
    esac
  fi
  python3 -I "$here/history.py" "$history" ${WORKERS:-}
  python3 -I "$here/versions.py" "$out" "$history"
fi
# The build definitions and closure parts in shards, as published (defs/).
python3 -I "$here/defs.py" "$out"
xz -T0 -kf "$out/index-x86_64-linux.json"
echo "next: nami catalog sign $out --key <name>, then serve $out anywhere"
ls -la "$out"
