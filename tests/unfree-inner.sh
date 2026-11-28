#!/bin/bash
# Runs inside a privileged Ubuntu container; see tests/unfree.sh.
set -u
fails=0
ok() { echo "  ok   $1"; }
bad() { echo "  FAIL $1"; fails=$((fails + 1)); }
check() { if eval "$2" >/tmp/out 2>&1; then ok "$1"; else bad "$1"; sed 's/^/       /' /tmp/out | tail -12; fi; }

/src/install.sh /new/nami >/dev/null
nami update --from /cat/index-x86_64-linux.json.xz --drvs /cat/drvs --unsigned >/dev/null 2>&1
nami daemon 2>/tmp/daemon.log &
sleep 1
useradd -m -s /bin/bash alice
as_alice() { su - alice -c "$1"; }

echo "== unfree apps, built from the catalog's closures"
check "spotify: built here from its snap" "as_alice 'nami install spotify' && test -x /home/alice/.local/state/nami/profile/bin/spotify"
check "its libraries resolve" "! ldd \$(readlink -f /home/alice/.local/state/nami/profile/share/spotify/spotify) | grep -q 'not found'"
check "vscode: downloaded from Microsoft and built here" "as_alice 'nami install vscode' && test -x /home/alice/.local/state/nami/profile/bin/code && grep -q 'building VSCode_' /tmp/out"
check "discord: a chain of builds (FHS env, desktop item, ...)" "as_alice 'nami install discord' && test -x /home/alice/.local/state/nami/profile/bin/discord"
check "they show up in the app menu" "ls /home/alice/.local/share/applications | grep -qi discord"
check "a second user gets them from the store, no rebuild" "useradd -m bob && su - bob -c 'nami install discord' > /tmp/bob 2>&1 && ! grep -q 'building' /tmp/bob"

echo "== opting out"
useradd -m carol
su - carol -c 'printf "unfree: false\npackages:\n  - hello\n" > ~/user.nami'
check "unfree: false refuses them" "su - carol -c 'nami install spotify' 2>&1 | grep -q \"isn't free software\""
check "free software still installs" "su - carol -c 'nami install hello'"
check "nami check knows the option" "su - carol -c 'nami check ~/user.nami'"

echo "== the catalog's closures are signed"
check "a tampered closure is refused" "cp -r /cat/drvs /tmp/drvs && sed -i 's/Valve/Valv3/' /tmp/drvs/steam.closure.json && rm -rf /var/cache/nami /nix/var/nami/cache 2>/dev/null; nami update --from /cat/index-x86_64-linux.json.xz --drvs /tmp/drvs --unsigned >/dev/null 2>&1; nami gc >/dev/null 2>&1; useradd -m dave && su - dave -c 'nami install steam' 2>&1 | grep -q \"doesn't match the catalog\""

echo
[ "$fails" -eq 0 ] && echo PASS || { echo "$fails FAILED"; tail -5 /tmp/daemon.log; exit 1; }
