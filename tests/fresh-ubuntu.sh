#!/bin/sh
# A new computer, end to end, without Docker: a freshly unpacked Ubuntu
# (user + mount + pid namespaces, no root needed), nami installed from
# target/release/nami and the live catalog, then a new user sets her computer
# up from a config repo with `nami apply <repo>` and opens a terminal.
# usage: tests/fresh-ubuntu.sh <config repo dir> [ubuntu release, e.g. 24.04]
# The repo is any directory under git with a computer's folder in it
# (<hostname>/user.nami: yours, or one `nami import` wrote; with several,
# the container's hostname picks). Work files: ${NAMI_FRESH_DIR:-~/.cache/nami-fresh}.
set -eu
cd "$(dirname "$0")/.."
cfg=$(realpath "${1:?config repo dir}")
release=${2:-24.04}
[ -d "$cfg/.git" ] || { echo "$cfg isn't a git repo (git init && git add -A && git commit)"; exit 1; }
[ -x target/release/nami ] || { echo "build nami first (./build.sh)"; exit 1; }
W=${NAMI_FRESH_DIR:-$HOME/.cache/nami-fresh}
mkdir -p "$W"
url=https://cdimage.ubuntu.com/ubuntu-base/releases/$release/release
tarball=$(curl -fsSL "$url/" | grep -o "ubuntu-base-$release[.0-9]*-base-amd64.tar.gz" | sort -uV | tail -1)
if [ ! -f "$W/$tarball" ]; then
  curl -fsSL -o "$W/$tarball.part" "$url/$tarball"
  (cd "$W" && curl -fsSL "$url/SHA256SUMS" | grep " \*\?$tarball\$" | sed "s| \*\?$tarball\$|  $tarball.part|" | sha256sum -c --quiet -)
  mv "$W/$tarball.part" "$W/$tarball"
fi
# A fresh root every run (the store and her home are left from the last one otherwise).
if [ -d "$W/root" ]; then
  chmod -R u+w "$W/root"
  rm -rf "${W:?}/root"
fi
mkdir -p "$W/root"
unshare -r tar -xzf "$W/$tarball" -C "$W/root" --exclude='dev/*' 2>/dev/null || true
echo "$tarball"
exec unshare -r -m -p -f --kill-child sh -s "$W/root" "$PWD" "$cfg" <<'OUTER'
set -eu
R=$1 src=$2 cfg=$3
mount --bind "$R" "$R"   # a mount of its own, for pivot_root
mount --rbind /dev "$R/dev"
mount -t proc proc "$R/proc"
mount --rbind /sys "$R/sys"
mount -t tmpfs tmpfs "$R/tmp"
cat /etc/resolv.conf > "$R/etc/resolv.conf"
mkdir -p "$R/src" "$R/srv/cfg" "$R/.old"
mount --bind "$src" "$R/src"
mount --bind "$cfg" "$R/srv/cfg"
# pivot_root, not chroot: inside a chroot no user namespace (her uid) can be made.
cd "$R"
./usr/sbin/pivot_root . .old
exec /bin/sh -c 'umount -l /.old && rmdir /.old; cd / && exec /bin/bash /src/tests/fresh-ubuntu-inner.sh'
OUTER
