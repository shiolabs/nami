#!/usr/bin/env sh
# The unfree-app path end to end:
#   1. Ubuntu (privileged): build examples/packages/vscode from Microsoft's tarball, push it
#   2. Fedora (unprivileged, can't build): install it from that cache with
#      Xvfb + ImageMagick from nixpkgs, open a file, screenshot the window
# usage: tests/vscode.sh <index.json.xz> <out-dir>
set -eu
cd "$(dirname "$0")/.."
PATH="$PWD/tests/lib:$PATH"   # docker, or rootless podman where there is none
index=$(realpath "${1:?index}"); out=$(realpath -m "${2:?out dir}")
mkdir -p "$out/cache"
docker run --rm --privileged -e HOST_UID="$(id -u)" -v "$PWD/target/release/nami":/usr/local/bin/nami:ro \
  -v "$PWD/examples/packages":/ex:ro -v "$index":/i.xz:ro -v "$out/cache":/cache -v "$PWD/tests/vscode-build-inner.sh":/run.sh:ro \
  ubuntu:24.04 bash /run.sh
docker run --rm -v "$PWD/target/release/nami":/new/nami:ro -v "$PWD":/src:ro -v "$PWD/examples/packages":/ex:ro \
  -v "$index":/i.xz:ro -v "$out/cache":/cache:ro -v "$out":/out -v "$PWD/tests/vscode-gui-inner.sh":/run.sh:ro \
  fedora:latest sh /run.sh
echo "screenshot: $out/vscode.png"
