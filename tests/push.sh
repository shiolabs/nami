#!/usr/bin/env sh
# Sharing builds through a signed cache, across distros:
#   A (Ubuntu, may build)     builds two recipes, pushes them to a cache dir
#   B (Alpine, can't build:   installs them from that cache over HTTP; without
#      no --privileged)       a sandbox, it would fail if it tried to build
#   C (Alpine)                gets a tampered copy of the cache and must refuse
#
# usage: tests/push.sh <index.json.xz> <drvs-dir>
set -eu
cd "$(dirname "$0")/.."
PATH="$PWD/tests/lib:$PATH"   # docker, or rootless podman where there is none
index=$(realpath "${1:?index}"); drvs=$(realpath "${2:?drvs dir}")
work=$(mktemp -d); trap 'docker run --rm -v "$work":/w alpine rm -rf /w/cache /w/bad; rm -rf "$work"' EXIT
mkdir -p "$work/cache"
mounts="-v $PWD/target/release/nami:/usr/local/bin/nami:ro -v $PWD/examples/packages:/ex:ro -v $index:/i.xz:ro -v $drvs:/drvs:ro"

echo "== A: build and push"
docker run --rm --privileged $mounts -v "$work":/w ubuntu:24.04 sh -euc '
  nami update --from /i.xz --unsigned --drvs /drvs >/dev/null
  nami keygen alice > /w/pubkey 2>/dev/null
  nami push /w/cache /ex/hello /ex/jq-patched 2>&1 | grep -E "^pushed|built "
  ls /w/cache | head -3'

key=$(cat "$work/pubkey")
cat > "$work/user.nami" <<EOF
caches:
  - url: http://127.0.0.1:8080
    key: $key
packages:
  - /ex/hello
  - /ex/jq-patched
EOF

echo "== B: install from the cache, no build sandbox available"
docker run --rm $mounts -v "$work/cache":/srv:ro -v "$work/user.nami":/user.nami:ro alpine:latest sh -euc '
  apk add -q busybox-extras >/dev/null && busybox-extras httpd -p 127.0.0.1:8080 -h /srv
  nami update --from /i.xz --unsigned --drvs /drvs >/dev/null
  nami apply /user.nami 2>&1 | grep -E "downloading|^\+|generation|error"
  P=/nix/var/nami/profiles/default/bin
  $P/hello --version | head -1
  $P/jq --help | sed -n 3p'

echo "== C: tampered cache is refused"
cp -r "$work/cache" "$work/bad"
f=$(grep -l "hello-2.12.1$" "$work"/bad/*.narinfo)
sed -i 's/^NarSize: .*/NarSize: 1/' "$f"
if docker run --rm $mounts -v "$work/bad":/srv:ro -v "$work/user.nami":/user.nami:ro alpine:latest sh -euc '
  apk add -q busybox-extras >/dev/null && busybox-extras httpd -p 127.0.0.1:8080 -h /srv
  nami update --from /i.xz --unsigned --drvs /drvs >/dev/null
  nami apply /user.nami' > "$work/c.log" 2>&1; then
  echo "FAIL: tampered cache was accepted"; exit 1
fi
grep -E "^error" "$work/c.log"
echo "PASS"
