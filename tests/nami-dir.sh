#!/usr/bin/env sh
# nami's folder is /nami (0.20). A new computer gets it there, on Ubuntu, Fedora and
# Arch, from install.sh: /nix/var/nami a link to it, nami's daemon at /nami, root's
# and a person's apps; the uninstaller takes it all. A computer nami 0.19.0 set up
# (its own installer, apps for root and someone, its daemon running) updates itself
# to this nami from a local release server, as computers do: the folder moves to
# /nami once, the old daemon serves on through the move, the apps and their history
# are as they were, an older nami still works there, and the uninstaller takes it.
# usage: tests/nami-dir.sh <index.json.xz> [image...]
#        (`nami run static` first: target/release/nami. 0.19.0's release comes from
#        NAMI_OLD_RELEASE, a folder with its unpacked files, or namios.dev.)
set -u
cd "$(dirname "$0")/.."
PATH="$PWD/tests/lib:$PATH"   # docker, or rootless podman where there is none
index=$(realpath "${1:?index.json.xz}"); shift
[ -x target/release/nami ] || { echo "build nami first: nami run static"; exit 1; }
old=${NAMI_OLD_RELEASE:-$HOME/.cache/nami-0.19.0/nami-0.19.0}
if [ ! -x "$old/nami" ]; then
  mkdir -p "$HOME/.cache/nami-0.19.0"
  curl -fsSL -o "$HOME/.cache/nami-0.19.0/n.tgz" "https://namios.dev/releases/0.19.0/nami-0.19.0-x86_64-linux.tar.gz?via=test"
  tar -xzf "$HOME/.cache/nami-0.19.0/n.tgz" -C "$HOME/.cache/nami-0.19.0"
  old=$HOME/.cache/nami-0.19.0/nami-0.19.0
fi
old=$(realpath "$old")
fails=0
run() {
  docker run --rm --privileged -v "$PWD/target/release/nami":/new/nami:ro -v "$old":/old:ro -v "$PWD":/src:ro \
    -v "$index":/i.xz:ro "$1" bash /src/tests/nami-dir-inner.sh "$2" || fails=$((fails + 1))
}
for image in ${*:-ubuntu:24.04 fedora:latest archlinux:latest}; do
  echo "== a new computer: $image"
  run "$image" fresh
done
echo "== updated from 0.19.0 (Ubuntu)"
run ubuntu:24.04 update
echo
[ "$fails" = 0 ] && echo "all passed" || { echo "$fails failed"; exit 1; }
