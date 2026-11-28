#!/bin/sh
# Claude Desktop, bundled (`nami bundle ./examples/packages/claude-desktop`),
# started on other Linuxes in containers: under Xvfb (a window, and a
# screenshot of it) and under Weston (a Wayland window), as someone who
# isn't root (Electron won't run as root with its sandbox).
#
# The bundle is Anthropic's app: it stays on this computer (default
# ~/.cache/nami-claude-desktop/bundle), never in the repo or uploaded.
#
# usage: tests/claude-desktop-bundle.sh [bundle] [image ...]
#   (default images: Ubuntu 24.04 and Arch; screenshots go next to the bundle, in shots/)
set -u
cd "$(dirname "$0")/.."
bundle=${1:-$HOME/.cache/nami-claude-desktop/bundle/claude-desktop-x86_64}
[ $# -gt 0 ] && shift
images=${*:-docker.io/library/ubuntu:24.04 docker.io/library/archlinux:latest}
# docker, or rootless podman (tests/lib/docker, first on PATH, sets it up;
# podman from the store when it isn't on PATH).
PATH=$PWD/tests/lib:$PATH
command -v podman >/dev/null || PATH=$PATH:$(ls -d /nix/store/*-podman-5*/bin 2>/dev/null | head -1)
dir=$(cd "$(dirname "$bundle")" && pwd)
fail=0

image() {
  name=nami-claude-gui-$(echo "${1##*/}" | tr ':/.' '---')
  docker image inspect "$name" >/dev/null 2>&1 && { echo "$name"; return; }
  case $1 in
    *ubuntu:*|*debian:*) pkgs="RUN apt-get update && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends xvfb x11-utils imagemagick libgl1-mesa-dri libegl1 libglx-mesa0 libgl1 fonts-dejavu-core procps dbus weston ca-certificates" ;;
    *archlinux:*) pkgs="RUN pacman -Syu --noconfirm xorg-server-xvfb xorg-xwininfo imagemagick mesa ttf-dejavu procps-ng dbus weston" ;;
  esac
  printf 'FROM %s\n%s\nRUN userdel -r ubuntu 2>/dev/null; useradd -m -u 1000 u\n' "$1" "$pkgs" | docker build -q -t "$name" - >/dev/null || return 1
  echo "$name"
}

for i in $images; do
  echo "== Claude Desktop on $i"
  img=$(image "$i") || { echo "  FAIL building the image"; fail=$((fail + 1)); continue; }
  # Chromium's sandbox makes user namespaces: the container's default
  # seccomp filter refuses them (a desktop doesn't).
  # Rootless podman: the container's user 1000 is you, who can write the
  # screenshots next to the bundle.
  keep=; docker --version 2>/dev/null | grep -qi podman && keep=--userns=keep-id:uid=1000,gid=1000
  docker run --rm $keep --security-opt seccomp=unconfined --shm-size=1g -u 1000:1000 -e HOME=/home/u -e DISTRO="$(echo "${i##*/}" | tr ':/' '--')" \
    -v "$dir":/b -v "$PWD/tests/claude-desktop-bundle-inner.sh":/run.sh:ro "$img" \
    dbus-run-session -- sh /run.sh "/b/$(basename "$bundle")" || fail=$((fail + 1))
done
[ $fail = 0 ] && echo "all passed" || { echo "$fail failed"; exit 1; }
