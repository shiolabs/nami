#!/bin/bash
# Runs inside a privileged Ubuntu container; see tests/sets.sh.
set -u
fails=0
ok() { echo "  ok   $1"; }
bad() { echo "  FAIL $1"; fails=$((fails + 1)); }
check() { : >/tmp/log; if eval "$2" >/tmp/out 2>&1; then ok "$1"; else bad "$1"; cat /tmp/log /tmp/out | sed 's/^/       /' | tail -15; fi; }
has() { jq -e --arg a "$1" 'has($a)' /tmp/index.json >/dev/null; }
apt-get -qq update >/dev/null && apt-get -qq install -y jq xz-utils >/dev/null

/src/install.sh /new/nami >/dev/null
xz -dc /cat/index-x86_64-linux.json.xz > /tmp/index.json
if [ -n "$TRUST" ]; then trust="--trust $TRUST"; else trust=--unsigned; fi
nami update --from /cat/index-x86_64-linux.json.xz --drvs /cat/drvs $trust >/tmp/update 2>&1 || { cat /tmp/update; exit 1; }
P=/nix/var/nami/profiles/default/bin

echo "== the published layout"
check "build definitions in shards, not a file each" "test \$(ls /cat/defs | wc -l) -le 4096 && ! ls /cat/drvs | grep -v '\.closure\.json\$'"
check "a closure in parts isn't a file of its own" "! test -e /cat/drvs/emacsPackages.magit.closure.json"

echo "== from the binary cache"
check "rPackages.ggplot2" "nami install rPackages.ggplot2 >/tmp/log 2>&1 && ! grep -q building /tmp/log && nami list | grep -q rPackages.ggplot2"
check "perlPackages.ack (the name people know)" "nami install perlPackages.ack && test -x $P/ack"

echo "== built here, from a closure in parts"
for a in emacsPackages.magit coqPackages_8_20.mathcomp-ssreflect cmdpack.ecm gerbilPackages-unstable.gerbil-utils; do
  has "$a" || { bad "$a isn't in the catalog"; continue; }
  check "$a" "nami install $a >/tmp/log 2>&1 && grep -q building /tmp/log && nami list | grep -qF $a"
done

echo "== unfree, built here from its closure"
check "cudaPackages.cuda_cupti" "nami install cudaPackages.cuda_cupti && nami list | grep -qF cudaPackages.cuda_cupti"

echo
[ "$fails" -eq 0 ] && echo PASS || { echo "$fails FAILED"; exit 1; }
