#!/bin/bash
# Runs inside a privileged Ubuntu container; see tests/rename-install.sh.
# A test: namios.dev counts its update checks and downloads apart (NAMI_TEST, site/worker.js).
export NAMI_TEST=1
set -u
fails=0
ok() { echo "  ok   $1"; }
bad() { echo "  FAIL $1"; fails=$((fails + 1)); }
check() { if eval "$2" >/tmp/out 2>&1; then ok "$1"; else bad "$1"; sed 's/^/       /' /tmp/out | tail -8; fi; }
apt-get update -qq >/dev/null 2>&1; apt-get install -y -qq ca-certificates >/dev/null 2>&1
cat=https://namios.dev/menu/nixos-unstable
key=$(grep '^tray-2:' /src/packaging/keys)

echo "== tray, as it was"
sh /old-install.sh /tray >/dev/null
export PATH="/nix/var/tray/bin:$PATH"
tray update --from $cat --trust "$key" >/dev/null 2>&1 || tray update --from $cat --trust "$(grep '^tray-3:' /src/packaging/keys)" >/dev/null 2>&1
tray daemon 2>/tmp/daemon.log &
sleep 1
useradd -m -s /bin/bash alice
check "root's app, in the computer's file" "tray install jq >/dev/null 2>&1; grep -q jq /etc/tray/computer.tray"
check "alice's own app" "su - alice -c 'tray install hello >/dev/null 2>&1; hello'"
check "her file is user.tray" "ls /home/alice/user.tray /home/alice/.config/tray /home/alice/.local/state/tray/profile >/dev/null"
cp /nix/var/tray/bin/tray /old-tray
kill %1 2>/dev/null; sleep 1

echo "== install.sh with nami"
check "install.sh runs" "sh /src/install.sh /new/nami"
check "/nix/var/tray is nami's, the old name a link" "[ \"\$(readlink /nix/var/tray)\" = nami ] && [ -d /nix/var/nami/profiles ]"
check "/etc/tray is nami's" "[ \"\$(readlink /etc/tray)\" = nami ] && [ -f /etc/nami/computer.tray ]"
check "nami and tray, the same program" "nami --version | grep -q '^nami' && tray --version | grep -q '^nami' && [ \"\$(readlink /nix/var/nami/bin/tray)\" = nami ]"
check "profile.d and fish: nami's alone" "[ -f /etc/profile.d/nami.sh ] && [ ! -e /etc/profile.d/tray.sh ]"
check "bash.bashrc: one line, nami's" "grep -c 'env.sh' /etc/bash.bashrc | grep -qx 1 && grep -q 'nami/env.sh' /etc/bash.bashrc"
check "polkit: nami's action and tray's" "[ ! -d /usr/share/polkit-1/actions ] || { [ -f /usr/share/polkit-1/actions/org.namios.change-computer.policy ] && [ -f /usr/share/polkit-1/actions/org.trayos.change-computer.policy ]; }"
nami daemon 2>/tmp/daemon2.log &
sleep 1
check "root's apps: from computer.tray" "nami list 2>&1 | grep -q jq"
check "alice's app, and her folders moved as she runs nami" "su - alice -c 'nami list | grep -q hello && hello' && [ \"\$(readlink /home/alice/.local/state/tray)\" = nami ] && [ \"\$(readlink /home/alice/.config/tray)\" = nami ]"
check "her login shell finds her apps (the new profile.d)" "su - alice -c 'command -v hello' | grep -q 'state/nami/profile'"
check "eval \$(tray hook bash) still works" "su - alice -c 'eval \"\$(tray hook bash)\" && echo HOOKED' | grep -q HOOKED"
check "a change renames computer.tray (not in git)" "nami remove jq 2>&1 | grep -q 'renamed' && [ -f /etc/nami/system.nami ] && [ \"\$(readlink /etc/nami/computer.tray)\" = system.nami ]"
check "the older tray still finds it all (its state through the links)" "/old-tray list 2>&1 | head -5; su - alice -c '/old-tray list' | grep -q hello"
kill %1 2>/dev/null

echo
[ $fails -eq 0 ] && echo "all passed" || echo "$fails failed"
[ $fails -eq 0 ]
