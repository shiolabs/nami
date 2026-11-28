#!/usr/bin/env sh
# Signed catalogs: sign, trust, refuse unsigned/untrusted/changed catalogs,
# and build definitions checked against the signed index.
# usage: tests/catalog.sh <index.json.xz with drv hashes> <drvs-dir>
set -eu
cd "$(dirname "$0")/.."
PATH="$PWD/tests/lib:$PATH"   # docker, or rootless podman where there is none
index=$(realpath "${1:?index}"); drvs=$(realpath "${2:?drvs}")
docker run --rm --privileged -v "$PWD/target/release/nami":/new/nami:ro -v "$PWD":/src:ro -v "$index":/i.xz:ro -v "$drvs":/drvs:ro \
  -v "$PWD/tests/catalog-inner.sh":/run.sh:ro ubuntu:24.04 bash -c 'apt-get -qq update >/dev/null && apt-get -qq install -y jq xz-utils >/dev/null && bash /run.sh'
