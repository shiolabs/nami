#!/usr/bin/env sh
# A Python with its packages (`- python3: { version: 3.13, with: [numpy, requests] }`,
# python_env.rs): in a project (nami run, the lock, made again from it), in a person's
# profile (apply, undo), and a package its set doesn't have.
# usage: tests/python.sh <index.json.xz>
set -eu
cd "$(dirname "$0")/.."
PATH="$PWD/tests/lib:$PATH"   # docker, or rootless podman where there is none
index=$(realpath "${1:?index}")
docker run --rm --privileged -v "$PWD/target/release/nami":/new/nami:ro -v "$PWD":/src:ro -v "$index":/i.xz:ro \
  -v "$PWD/tests/python-inner.sh":/run.sh:ro ubuntu:24.04 bash /run.sh
