#!/bin/bash
# Runs inside a privileged Ubuntu container; see tests/lock.sh.
set -u
fails=0
ok() { echo "  ok   $1"; }
bad() { echo "  FAIL $1"; fails=$((fails + 1)); }
check() { if eval "$2" >/tmp/out 2>&1; then ok "$1"; else bad "$1"; sed 's/^/       /' /tmp/out | tail -8; fi; }

/src/install.sh /new/nami >/dev/null
nami update --from /i.xz --unsigned >/dev/null
nami daemon 2>/tmp/daemon.log &
sleep 1
useradd -m -s /bin/bash alice
useradd -m -s /bin/bash carol
as() { su - "$1" -c "$2"; }
paths() { as "$1" 'nami list --json' | jq -cS '[.[] | {attr, name}] | sort_by(.attr)'; }
profile_paths() { jq -cS '[.packages[] | .paths] | sort' "$(ls -d /nix/var/nami/profiles/per-user/$1/default-*.json | sort -V | tail -1)"; }

echo "== first apply writes nami.lock"
as alice 'mkdir -p ~/dots/recipes && cp -r /src/examples/packages/zcat-lite ~/dots/recipes/ && printf "packages:\n  - hello\n  - jq\n  - ./recipes/zcat-lite\n" > ~/dots/user.nami'
check "apply" "as alice 'cd ~/dots && nami apply ./user.nami'"
check "nami.lock written next to the nami file" "jq -e '.packages.hello.paths[0] | startswith(\"/nix/store/\")' /home/alice/dots/nami.lock"
check "locks the recipe too" "jq -e '.packages[\"./recipes/zcat-lite\"].name == \"zcat-lite-0.1.0\"' /home/alice/dots/nami.lock"
check "records which catalog" "jq -e '.catalog.sha256 | length == 64' /home/alice/dots/nami.lock"
HELLO=$(jq -r '.packages.hello.paths[0]' /home/alice/dots/nami.lock)
cp /home/alice/dots/nami.lock /tmp/original.lock
profile_paths alice > /tmp/alice-paths

echo "== the catalog moves on"
# Make the catalog say hello is something else (cowsay's build stands in for a newer hello).
xz -dc /i.xz | jq -c '.hello.outputs.out = .cowsay.outputs.out | .hello.name = "hello-9.9"' > /tmp/newer.json
nami update --from /tmp/newer.json --unsigned >/dev/null
check "plain apply keeps the locked hello" "as alice 'cd ~/dots && nami apply ./user.nami' && [ \"\$(jq -r '.packages.hello.paths[0]' /home/alice/dots/nami.lock)\" = $HELLO ] && as alice 'nami list --json' | grep -q '"hello-2.12.3"'"
check "a new package is resolved and added to the lock" "as alice \"cd ~/dots && sed -i 's/  - jq/  - jq\n  - ripgrep/' user.nami && nami apply ./user.nami\" && jq -e '.packages.ripgrep' /home/alice/dots/nami.lock"
check "--upgrade moves to the new catalog" "as alice 'cd ~/dots && nami apply ./user.nami --upgrade' && jq -e '.packages.hello.name == \"hello-9.9\"' /home/alice/dots/nami.lock"

echo "== reproduce exactly elsewhere"
as alice "cd ~/dots && sed -i '/ripgrep/d' user.nami"
cp /tmp/original.lock /home/alice/dots/nami.lock
cp -r /home/alice/dots /home/carol/dots && chown -R carol: /home/carol/dots
check "--locked on another account installs exactly the locked set" "as carol 'cd ~/dots && nami apply ./user.nami --locked'"
check "same store paths as alice's first apply" "[ \"\$(profile_paths carol)\" = \"\$(cat /tmp/alice-paths)\" ]"
check "even though the catalog says otherwise now" "as carol 'nami list --json' | grep -q '"hello-2.12.3"'"
check "--locked refuses packages missing from the lock" "as carol \"cd ~/dots && printf '  - tree\n' >> user.nami\" && ! as carol 'cd ~/dots && nami apply ./user.nami --locked' && grep -q \"tree isn't in nami.lock\" /tmp/out"

echo
[ "$fails" -eq 0 ] && echo PASS || { echo "$fails FAILED"; exit 1; }
