#!/usr/bin/env sh
# A person's ~/user.nami (env, aliases, files: ~/...) as a user without root: rendering to real formats
# (checked with git, jq, python's tomllib), env/aliases in shells, backups of
# existing files, changes, rollback, and unmanaging.
# usage: tests/home.sh <index.json.xz>
set -eu
cd "$(dirname "$0")/.."
PATH="$PWD/tests/lib:$PATH"   # docker, or rootless podman where there is none
index=$(realpath "${1:?index}")
docker run --rm --privileged -v "$PWD/target/release/nami":/new/nami:ro -v "$PWD":/src:ro \
  -v "$index":/i.xz:ro -v "$PWD/tests/home-inner.sh":/run.sh:ro ubuntu:24.04 bash /run.sh
