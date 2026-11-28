#!/bin/bash
# Runs inside a privileged Ubuntu container; see tests/catalog.sh.
set -u
fails=0
ok() { echo "  ok   $1"; }
bad() { echo "  FAIL $1"; fails=$((fails + 1)); }
check() { if eval "$2" >/tmp/out 2>&1; then ok "$1"; else bad "$1"; sed 's/^/       /' /tmp/out | tail -6; fi; }

/src/install.sh /new/nami >/dev/null
mkdir -p /cat/drvs && cp /i.xz /cat/index-x86_64-linux.json.xz
cp /drvs/jq.json /drvs/hello.json /cat/drvs/

echo "== signing"
PUB=$(nami keygen catalog-1 2>/dev/null)
check "nami catalog sign writes a .sig" "nami catalog sign /cat && grep -q '^catalog-1:' /cat/index-x86_64-linux.json.xz.sig"
check "unsigned catalogs are refused by default" "! nami update --from /i.xz && grep -q \"isn't signed\" /tmp/out"
check "a signature from an untrusted key is refused" "! nami update --from /cat/index-x86_64-linux.json.xz && grep -q \"doesn't trust\" /tmp/out"
check "trusting the key accepts it" "nami update --from /cat/index-x86_64-linux.json.xz --trust '$PUB' | grep -q 'Catalog updated'"
check "--refresh keeps checking it" "nami update --refresh | grep -q 'Catalog refreshed'"
cp /cat/index-x86_64-linux.json.xz /tmp/good.xz
xz -dc /tmp/good.xz | sed 's/"name":"jq-1.8.2"/"name":"jq-6.6.6"/' | xz > /cat/index-x86_64-linux.json.xz
check "a changed catalog fails its signature" "! nami update --refresh && grep -q 'changed after signing' /tmp/out"
cp /tmp/good.xz /cat/index-x86_64-linux.json.xz

echo "== build definitions are covered by the signature"
nami update --from /cat/index-x86_64-linux.json.xz --drvs /cat/drvs >/dev/null
mkdir -p /r/jq2 && printf 'extends: jq\nname: jq2\n' > /r/jq2/package.nami
check "the index carries the definition's hash" "xz -dc /cat/index-x86_64-linux.json.xz | jq -e '.jq.drv | length == 64'"
sed -i 's/"pname":"jq"/"pname":"evil"/' /cat/drvs/jq.json
check "a tampered definition is refused" "! nami build /r/jq2 && grep -q \"doesn't match the catalog\" /tmp/out"

echo
[ "$fails" -eq 0 ] && echo PASS || { echo "$fails FAILED"; exit 1; }
