#!/bin/sh
# nami-gui for a release, for this kind of computer (x86_64 or ARM Linux): built, packed with
# its .desktop file into dist/nami-gui-<v>-<system>.tar.gz, and examples/packages/nami-gui's
# source for this kind pointed at it (packaging/gui-source.sh: the version, the address, the
# checksum). Run before nami's own release build: nami carries its examples, so it carries
# this checksum too; then packaging/publish.sh puts the tarball in the release.
# x86_64's is made by nami's release itself (with namiOS's desktop, nami-desktop, beside it); ARM
# Linux's on GitHub's ARM runner by nami's public repo (its arm-linux.yml), from nami's window
# alone.
# Needs gpui's libraries: a
# distribution's (.github/workflows/ci.yml lists Ubuntu's).
# usage: [BASE_URL=https://namios.dev] packaging/gui-release.sh
set -eu
cd "$(dirname "$0")/.."
case "$(uname -s)-$(uname -m)" in
  Linux-x86_64) system=x86_64-linux ;;
  Linux-aarch64) system=aarch64-linux ;;
  *) echo "nami-gui's release is made on x86_64 or ARM Linux, not $(uname -s) $(uname -m)"; exit 1 ;;
esac
version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
# (NAMI_VERSION: what `nami-gui --version` says.) Nothing of the computer it's built on goes in
# what's shipped: the crates' sources are /cargo/..., nami's /nami/... in its messages, and no
# library search path (a dev shell's points in a home folder: the recipe's autoPatchelfHook
# gives the installed program nami's own). A Nix dev shell's linker adds one for each library
# folder unless told not to (NIX_DONT_SET_RPATH*), and its NIX_LDFLAGS has -rpath in it.
cargo_home=$(cd "${CARGO_HOME:-$HOME/.cargo}" 2>/dev/null && pwd -P || echo "${CARGO_HOME:-$HOME/.cargo}")
NIX_LDFLAGS=$(printf '%s' "${NIX_LDFLAGS:-}" | sed -E 's/(^| )-rpath +[^ ]+//g')
export NIX_LDFLAGS NIX_DONT_SET_RPATH=1 "NIX_DONT_SET_RPATH_$(uname -m)_unknown_linux_gnu=1"
NAMI_VERSION=$version RUSTFLAGS="${RUSTFLAGS:-} --remap-path-prefix=$cargo_home=/cargo --remap-path-prefix=$(pwd -P)=/nami" \
  cargo build --release --manifest-path gui/Cargo.toml
for b in gui/target/release/nami-gui gui/target/release/nami-desktop; do
  [ -e "$b" ] || continue
  ! readelf -d "$b" | grep -q 'R\(UN\)\?PATH' || { echo "$b has a library search path ($(readelf -d "$b" | grep -o 'path: \[.*'))"; exit 1; }
  ! grep -aq "$HOME" "$b" || { echo "$b still has $HOME in it: not shipping it"; exit 1; }
done
stage=$(mktemp -d); trap 'rm -rf "$stage"' EXIT
mkdir -p "$stage/nami-gui-$version" dist
cp gui/target/release/nami-gui gui/nami-gui.desktop gui/nami.desktop gui/nami-gui.svg gui/nami.svg gui/nami-monitor.svg gui/nami-settings.svg "$stage/nami-gui-$version/"
# namiOS's desktop, where this nami-gui is built with namiOS's part (x86_64's, here).
[ ! -e gui/target/release/nami-desktop ] || cp gui/target/release/nami-desktop "$stage/nami-gui-$version/"
tar=dist/nami-gui-$version-$system.tar.gz
# The same bytes for the same build: no dates or owners in it.
tar -C "$stage" --sort=name --mtime=@0 --owner=0 --group=0 --numeric-owner -cf - "nami-gui-$version" | gzip -n > "$tar"
sum=$(sha256sum "$tar" | cut -d' ' -f1)
BASE_URL=${BASE_URL:-https://namios.dev} sh packaging/gui-source.sh "$system" "$sum"
echo "$tar ($(du -h "$tar" | cut -f1)): examples/packages/nami-gui points at it"
