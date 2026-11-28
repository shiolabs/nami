#!/usr/bin/env sh
# nami import end to end: a home-manager setup (zsh with oh-my-zsh and
# plugins, starship, zoxide, eza, tmux, variables) imported and applied by a
# user on a fresh Ubuntu, then a real interactive zsh checked for each part.
# usage: tests/import.sh <index.json.xz>
set -eu
cd "$(dirname "$0")/.."
PATH="$PWD/tests/lib:$PATH"   # docker, or rootless podman where there is none
index=$(realpath "${1:?index}")
docker run --rm --privileged -v "$PWD/target/release/nami":/new/nami:ro -v "$PWD":/src:ro \
  -v "$index":/i.xz:ro ubuntu:24.04 bash /src/tests/import-inner.sh
