#!/usr/bin/env sh
# Settings apps change (mutable: true): see what changed, keep it in the
# spec, undo it, and merge it with spec changes setting by setting.
# usage: tests/drift.sh <index.json.xz>
set -eu
cd "$(dirname "$0")/.."
PATH="$PWD/tests/lib:$PATH"   # docker, or rootless podman where there is none
index=$(realpath "${1:?index}")
docker run --rm --privileged -v "$PWD/target/release/nami":/new/nami:ro -v "$PWD":/src:ro -v "$index":/i.xz:ro \
  -v "$PWD/tests/drift-inner.sh":/run.sh:ro ubuntu:24.04 bash /run.sh
