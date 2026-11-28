#!/usr/bin/env sh
# Runs the same nami scenario on many distros, each in a clean container.
# Nothing in the container is prepared: no Nix, no certificates, no packages.
#
# usage: tests/distros.sh <index.json.xz> [image...]
set -u
cd "$(dirname "$0")/.."
PATH="$PWD/tests/lib:$PATH"   # docker, or rootless podman where there is none

index=$(realpath "${1:?path to index-x86_64-linux.json.xz}"); shift
bin=target/release/nami
[ -x "$bin" ] || { echo "build first: ./build.sh"; exit 1; }

images=${*:-"ubuntu:24.04 debian:12 fedora:latest archlinux:latest opensuse/tumbleweed alpine:latest ghcr.io/void-linux/void-musl"}

scenario='
set -eux
P=/nix/var/nami/profiles/default/bin
nami update --from /index.json.xz --unsigned
nami install hello ripgrep
$P/hello
$P/rg --version
printf "packages:\n  - hello\n  - jq\n" > /user.nami
nami apply /user.nami
$P/jq --version
test ! -e $P/rg
nami generations
nami rollback
$P/rg --version
nami gc --old
nami list
'

pass=0; fail=0
for image in $images; do
  log=$(mktemp)
  if docker run --rm -v "$PWD/$bin":/usr/local/bin/nami:ro -v "$index":/index.json.xz:ro \
       "$image" sh -c "$scenario" >"$log" 2>&1; then
    echo "PASS  $image"; pass=$((pass + 1))
  else
    echo "FAIL  $image"; tail -15 "$log" | sed 's/^/      /'; fail=$((fail + 1))
  fi
  rm -f "$log"
done
echo "$pass passed, $fail failed"
[ "$fail" -eq 0 ]
