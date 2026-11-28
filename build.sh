#!/usr/bin/env sh
# Builds a fully static nami binary (musl) in a container: no host toolchain needed.
# Without Docker, builds it with the flake (nix build .#nami) instead.
# usage: ./build.sh [cargo args...]   e.g. ./build.sh test
set -eu
cd "$(dirname "$0")"
cmd=${1:-build}; [ $# -gt 0 ] && shift
if ! command -v docker >/dev/null 2>&1 && command -v nix >/dev/null 2>&1 && [ "$cmd" = build ]; then
  # No Docker, but Nix (NixOS): the flake builds the same static binary.
  out=$(nix build --no-link --print-out-paths .#nami)
  mkdir -p target/release && install -m755 "$out/bin/nami" target/release/nami
  for r in nami-bundle-runtime nami-bundle-runtime-net; do [ ! -e "$out/bin/$r" ] || install -m755 "$out/bin/$r" target/release/$r; done
  echo "target/release/nami (from the flake)"; exit 0
fi
docker image inspect nami-builder >/dev/null 2>&1 || docker build -q -t nami-builder -f Dockerfile.build . >/dev/null
docker volume create nami-cargo >/dev/null
# A new volume belongs to root; the build runs as you.
docker run --rm -v nami-cargo:/cargo nami-builder chown "$(id -u):$(id -g)" /cargo
docker run --rm -u "$(id -u):$(id -g)" -e CARGO_HOME=/cargo -e HOME=/tmp \
  -v nami-cargo:/cargo -v "$PWD":/src -w /src nami-builder \
  cargo "$cmd" --release "$@"
# The bundle runtime (`nami bundle`'s start of every bundle) rides along at
# the end of nami, so the one file has both.
# ARM=1: the aarch64 runtimes too, for `nami bundle --arch aarch64` (Rust's
# aarch64 musl target, Zig as the C compiler: Dockerfile.build-aarch64).
# On an ARM computer (GitHub's ubuntu-24.04-arm, .github/workflows/arm-linux.yml)
# everything built is aarch64's, nami and its runtimes: ARM=1 has nothing to add.
arm=
if [ "$cmd" = build ] && [ "${ARM:-}" = 1 ] && [ "$(uname -m)" != aarch64 ]; then
  docker image inspect nami-builder-aarch64 >/dev/null 2>&1 || docker build -q -t nami-builder-aarch64 - < Dockerfile.build-aarch64 >/dev/null
  docker run --rm -u "$(id -u):$(id -g)" -e CARGO_HOME=/cargo -e HOME=/tmp \
    -v nami-cargo:/cargo -v "$PWD":/src -w /src nami-builder-aarch64 \
    cargo build --release --target aarch64-unknown-linux-musl --bin nami-bundle-runtime --bin nami-bundle-runtime-net
  a=target/aarch64-unknown-linux-musl/release
  arm="$a/nami-bundle-runtime $a/nami-bundle-runtime-net"
fi
if [ "$cmd" = build ] && [ -x target/release/nami-bundle-runtime ] && [ -x target/release/nami-bundle-runtime-net ]; then
  # shellcheck disable=SC2086
  target/release/nami attach-bundle-runtime target/release/nami target/release/nami-bundle-runtime target/release/nami-bundle-runtime-net $arm
fi
