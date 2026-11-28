#!/usr/bin/env sh
# `nami bundle --arch aarch64`: bundles for ARM computers, made on this
# x86_64 one from the catalog's aarch64 packages, then run on arm64
# Ubuntu 24.04, Debian 12 and Fedora, emulated, with no root and nothing
# on this computer changed:
#
#   index  the catalog's aarch64 index, for a few packages (index/build-index.sh
#          with SYSTEM=aarch64-linux, in a nixos/nix container)
#   make   in an Ubuntu container: jq (whole and --thin) and tmux for
#          aarch64; what --arch refuses (a project, --update-url)
#   run    each distro's arm64 files (podman pulls and unpacks them; none
#          runs), and in a user namespace of its own with a binfmt_misc of
#          its own (Linux 6.7 and later give each one that mounts it its
#          own), qemu-aarch64 for aarch64 programs: chrooted into each, the
#          bundles run on their own aarch64 glibc, check out against their
#          hashes, and the thin one fetches its parts
#
# usage: tests/bundle-arm.sh [index|make|run ...]   (default: all three)
#   NAMI=target/release/nami (ARM=1 ./build.sh: it carries the aarch64 runtimes)
#   INDEX=/nix/var/nami/index.json   OUT=~/.cache/nami-arm   NIX_VOLUME=nami-bundle-nix
#   RELEASE: the nixos-unstable release the catalog is from (its index-source)
#   QEMU: a static qemu-aarch64 (default: Ubuntu's qemu-user-static, fetched once)
set -u
cd "$(dirname "$0")/.."
PATH="$PWD/tests/lib:$PATH"
nami=$(realpath "${NAMI:-target/release/nami}")
index=$(realpath "${INDEX:-/nix/var/nami/index.json}")
out=${OUT:-$HOME/.cache/nami-arm}
mkdir -p "$out/index" "$out/b" "$out/rootfs"
out=$(realpath "$out")
vol=${NIX_VOLUME:-nami-bundle-nix}
stages=${*:-index make run}
fail=0

arm_index() {
  [ -s "$out/index/index-aarch64-linux.json" ] && return
  release=${RELEASE:?the nixos-unstable release the catalog is from, e.g. https://releases.nixos.org/nixos/unstable/nixos-26.11pre1092090.55bfb4d7437b}
  SYSTEM=aarch64-linux ONLY="jq tmux" WORKERS=2 index/build-index.sh "$release" "$out/index" || fail=$((fail + 1))
}

make_bundles() {
  echo "== make (in ubuntu:24.04, x86_64)"
  docker volume create $vol >/dev/null
  docker run --rm --platform linux/amd64 -v $vol:/nix -v "$nami":/usr/local/bin/nami:ro -v "$index":/index.json:ro \
    -v "$out/index":/arm:ro -v "$out/b":/out ubuntu:24.04 bash -c '
    fails=0
    ok() { echo "  ok   $1"; }
    bad() { echo "  FAIL $1"; fails=$((fails + 1)); }
    nami update --from /index.json --unsigned >/dev/null
    export NAMI_BUNDLE_INDEX_AARCH64=/arm/index-aarch64-linux.json
    cd /tmp
    for a in "jq" "jq --thin" "tmux"; do
      set -- $a; o=/out/$1$([ -n "${2:-}" ] && echo -thin)-aarch64
      if nami bundle $a --arch aarch64 -o $o >/tmp/log 2>&1; then ok "$a --arch aarch64: $(grep -o "[0-9.]* [KMG]B unpacked" /tmp/log)"; else bad "$a --arch aarch64"; tail -5 /tmp/log; fi
    done
    tail -c 4096 /out/jq-aarch64 | grep -q "\"arch\":\"aarch64\"" && ok "it says it is for aarch64" || bad "it says it is for aarch64"
    nami bundle --verify /out/jq-aarch64 >/dev/null && ok "nami (x86_64) checks it against its hash" || bad "nami checks it against its hash"
    mkdir -p /tmp/proj && echo "[package]" > /tmp/proj/Cargo.toml
    nami bundle /tmp/proj --arch aarch64 2>&1 | grep -q "ARM runner" && ok "a project is refused, saying where to build it" || bad "a project is refused"
    nami bundle jq --arch aarch64 --update-url https://example.com/x 2>&1 | grep -q "update.json" && ok "--update-url is refused, saying why" || bad "--update-url is refused"
    [ "$fails" -eq 0 ]' || fail=$((fail + 1))
}

# An arm64 distro's files: pulled for arm64 and unpacked, its x86_64 image put back.
rootfs() { # image dir
  [ -e "$2/etc/os-release" ] && return
  docker pull -q --platform linux/arm64 "$1" >/dev/null && id=$(docker create --platform linux/arm64 "$1" /bin/true) || return 1
  mkdir -p "$2" && docker export "$id" | tar -x -C "$2" 2>/dev/null
  docker rm "$id" >/dev/null
  docker pull -q --platform linux/amd64 "$1" >/dev/null
}

run() {
  q=${QEMU:-$out/qemu-aarch64-static}
  if [ ! -x "$q" ]; then
    docker run --rm --platform linux/amd64 -v "$out":/out ubuntu:24.04 bash -c 'apt-get update -qq >/dev/null && cd /tmp && apt-get download -qq qemu-user-static >/dev/null 2>&1 && dpkg-deb -x qemu-user-static_*.deb q && cp q/usr/bin/qemu-aarch64-static /out/' || { echo "  FAIL getting qemu-aarch64-static"; fail=$((fail + 1)); return; }
  fi
  for d in ubuntu:24.04 debian:12 fedora:latest; do
    dir="$out/rootfs/$(echo "$d" | tr ':/' '--')"
    rootfs "docker.io/library/$d" "$dir" || { echo "  FAIL unpacking $d for arm64"; fail=$((fail + 1)); continue; }
    REG=':qemu-aarch64:M::\x7fELF\x02\x01\x01\x00\x00\x00\x00\x00\x00\x00\x00\x00\x02\x00\xb7\x00:\xff\xff\xff\xff\xff\xff\xff\x00\xff\xff\xff\xff\xff\xff\xff\xff\xfe\xff\xff\xff:'"$q"':FOC'
    REG=$REG ROOT=$dir B=$out/b unshare --user --map-root-user --mount --pid --fork sh -c '
      mount -t binfmt_misc binfmt_misc /proc/sys/fs/binfmt_misc && echo "$REG" > /proc/sys/fs/binfmt_misc/register || { echo "  FAIL this kernel gives a user namespace no binfmt_misc of its own (Linux 6.7 and later do)"; exit 1; }
      mkdir -p "$ROOT/b" "$ROOT/proc" "$ROOT/tmp" "$ROOT/dev" "$ROOT/run/t"
      mount --bind "$B" "$ROOT/b" && mount -t proc proc "$ROOT/proc" && mount -t tmpfs tmpfs "$ROOT/tmp" && mount --rbind /dev "$ROOT/dev"
      rm -f "$ROOT/etc/resolv.conf"; cp -L /etc/resolv.conf "$ROOT/etc/resolv.conf"
      cp tests/bundle-arm-inner.sh "$ROOT/run/t/run.sh"
      exec chroot "$ROOT" /bin/sh /run/t/run.sh' || fail=$((fail + 1))
  done
}

for s in $stages; do
  case $s in
    index) arm_index ;;
    make) make_bundles ;;
    run) run ;;
    *) echo "unknown stage $s"; exit 2 ;;
  esac
done
[ "$fail" -eq 0 ] && echo "PASS" || { echo "$fail FAILED"; exit 1; }
