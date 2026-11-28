#!/usr/bin/env sh
# Per-project environments (a project's project.nami): tools only inside the
# project, pinned by nami.lock, safe from gc while the project exists. Also:
# dotfiles repos' user.nami, and `nami apply`.
# usage: tests/project.sh <index.json.xz>
set -eu
cd "$(dirname "$0")/.."
PATH="$PWD/tests/lib:$PATH"   # docker, or rootless podman where there is none
index=$(realpath "${1:?index}")
# A newer catalog for the pinning checks: the same catalog without cowsay.
newer=$(mktemp -d)
python3 -I -c 'import json,lzma,sys; d=json.loads(lzma.open(sys.argv[1]).read()); d.pop("cowsay",None); lzma.open(sys.argv[2],"wt").write(json.dumps(d))' "$index" "$newer/newer.json.xz"
docker run --rm --privileged -v "$PWD/target/release/nami":/new/nami:ro -v "$PWD":/src:ro -v "$index":/i.xz:ro -v "$newer/newer.json.xz":/newer.xz:ro \
  -v "$PWD/tests/project-inner.sh":/run.sh:ro ubuntu:24.04 bash /run.sh
