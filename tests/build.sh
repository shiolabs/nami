#!/usr/bin/env sh
# Builds the example recipes in clean containers and runs the results.
# Builds need a sandbox (user + mount namespaces), hence --privileged.
#
# usage: tests/build.sh <index.json.xz> [image...]
set -u
cd "$(dirname "$0")/.."
PATH="$PWD/tests/lib:$PATH"   # docker, or rootless podman where there is none
index=$(realpath "${1:?index}"); shift
images=${*:-"ubuntu:24.04 alpine:latest"}

scenario='
set -eu
P=/nix/var/nami/profiles/default/bin
nami update --from /index.json.xz --unsigned >/dev/null
cd /src/examples
for r in hello zcat-lite greeting bat-bin; do echo "$r: $(nami build ./$r 2>/tmp/$r.log || { tail -30 /tmp/$r.log; exit 1; })"; done
printf "packages:\n  - ./hello\n  - ./greeting\n  - ./bat-bin\n  - jq\n" > user.nami
nami apply user.nami 2>&1 | grep -E "^\+|generation"
$P/hello --version | head -1
$P/greeting
$P/bat --version
echo "{\"built\": \"by nami\"}" | $P/jq -c .
echo "rebuild is a no-op: $(nami build ./hello 2>&1 | head -1)"
'

fail=0
for image in $images; do
  echo "== $image"
  docker run --rm --privileged -v "$PWD/target/release/nami":/usr/local/bin/nami:ro \
    -v "$PWD/examples/packages":/src/examples-ro:ro -v "$index":/index.json.xz:ro "$image" \
    sh -c "mkdir -p /src && cp -r /src/examples-ro /src/examples && $scenario" || { echo "FAIL $image"; fail=$((fail+1)); }
done
[ "$fail" -eq 0 ]
