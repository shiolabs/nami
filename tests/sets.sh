#!/usr/bin/env sh
# The package sets nixpkgs doesn't walk (index/catalog.nix: rPackages,
# emacsPackages, ...), from a published catalog: its build definitions in
# shards (defs/), the closures in parts. Installs one of each kind: one the
# binary cache has, one built here from a closure in parts, one unfree.
# The catalog needs the rest of nixpkgs too (nami builds with bubblewrap,
# R wants mesa): a whole one (index/build-index.sh, packaging/publish.sh
# CATALOG_ONLY=1), or ONLY="rPackages ..."'s merged into the live one's index.
# usage: tests/sets.sh <published catalog dir: menu/<rev> with index-x86_64-linux.json.xz(.sig), defs/, drvs/> [<its key, name:base64>]
set -eu
cd "$(dirname "$0")/.."
PATH="$PWD/tests/lib:$PATH"   # docker, or rootless podman where there is none
cat=$(realpath "${1:?catalog dir}")
docker run --rm --privileged -e TRUST="${2:-}" -v "$PWD/target/release/nami":/new/nami:ro -v "$PWD":/src:ro -v "$cat":/cat:ro \
  -v "$PWD/tests/sets-inner.sh":/run.sh:ro ubuntu:24.04 bash /run.sh
