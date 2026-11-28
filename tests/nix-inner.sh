#!/bin/bash
# Runs inside a privileged Ubuntu container with Nix; see tests/nix.sh.
# READONLY=1: /nix/store mounted read-only, as on NixOS.
# /paths: the catalog's store paths for ripgrep, hello and tree.
set -u
. /paths
fails=0
ok() { echo "  ok   $1"; }
bad() { echo "  FAIL $1"; fails=$((fails + 1)); }
check() { if eval "$2" >/tmp/out 2>&1; then ok "$1"; else bad "$1"; sed 's/^/       /' /tmp/out | tail -12; fi; }

# The machine's Nix (single-user, root), as copied from nixos/nix.
cp -a /seed/nix / && cp -a /seed/etc/nix /etc/
printf 'build-users-group =\nsandbox = false\nssl-cert-file = %s\n' "$(ls /nix/store/*-nss-cacert-*/etc/ssl/certs/ca-bundle.crt | head -1)" >> /etc/nix/nix.conf
N=/nix/var/nix/profiles/default/bin
if [ "${READONLY:-}" = 1 ]; then
  # As on NixOS: the store is a read-only bind mount; only Nix writes to it.
  mount --bind /nix/store /nix/store && mount -o remount,ro,bind /nix/store
  check "the store is read-only" "! touch /nix/store/x"
fi
valid() { $N/nix-store --check-validity "$1" 2>/dev/null; }

echo "== Nix's own ripgrep"
check "Nix fetches ripgrep and roots it" "$N/nix-store --realise $RG --add-root /nix/var/nix/gcroots/users-rg"
before=$(stat -c %i $RG/bin/rg)

/src/install.sh /nami-bin >/dev/null
. /etc/profile.d/nami.sh
nami update --from /i.xz --unsigned >/dev/null 2>&1
check "nami sees Nix" "nami doctor | grep -q 'shared with Nix'"

echo "== installing"
check "install ripgrep, hello" "nami install ripgrep hello"
check "ripgrep came from Nix's copy, not downloaded again" "! grep -q 'fetching.*paths' /tmp/out || grep 'fetching 1 paths' /tmp/out"
check "and Nix's copy was left as it was" "[ \$(stat -c %i $RG/bin/rg) = $before ]"
check "hello is in Nix's database" "valid $HELLO"
check "hello runs" "hello --version | head -1 | grep -q hello"
check "nami roots what it uses" "test -L /nix/var/nix/gcroots/tray/\$(basename $HELLO)"

echo "== a recipe built by nami"
mkdir -p /tmp/ex && cp -r /src/examples/packages/hello /tmp/ex/
check "nami build ./hello" "cd /tmp/ex && nami build ./hello > /tmp/built"
B=$(tail -1 /tmp/built)
check "its output is registered in Nix" "valid $B"
check "Nix verifies its contents" "$N/nix-store --verify-path $B"
check "it runs" "$B/bin/hello"

echo "== a project"
mkdir -p /tmp/app && printf 'packages:\n  - tree\n' > /tmp/app/project.nami
check "nami run in a project" "cd /tmp/app && nami run -- tree --version"

echo "== a user without root, while Nix's daemon runs too"
$N/nix-daemon >/tmp/nix-daemon.log 2>&1 &
nami daemon >/tmp/nami-daemon.log 2>&1 &
sleep 1
useradd -m -s /bin/bash bob
check "bob installs jq through nami's daemon" "su - bob -c 'nami install jq && jq --version'"
check "jq is in Nix's database" "valid \$(readlink -f /home/bob/.local/state/nami/profile/bin/jq | cut -d/ -f1-4)"
check "Nix users still work through Nix's daemon" "NIX_REMOTE=daemon $N/nix-store --check-validity $RG"

echo "== Nix's garbage collector"
check "nix-collect-garbage" "$N/nix-collect-garbage"
check "keeps nami's apps" "valid $HELLO && hello --version >/dev/null"
check "keeps nami's build" "valid $B"
check "keeps the project's tools" "valid $TREE && cd /tmp/app && nami run -- tree --version"

echo "== nami's garbage collector"
check "nami remove ripgrep hello" "nami remove ripgrep hello"
rm -rf /tmp/app
check "nami gc" "nami gc --old"
check "deletes hello (only nami used it)" "! valid $HELLO && ! test -e $HELLO"
check "but not ripgrep (Nix uses it)" "valid $RG && $RG/bin/rg --version >/dev/null"
check "and Nix's own tools still work" "$N/nix-store --version && $N/nix-store --verify --check-contents >/dev/null"

echo
[ "$fails" -eq 0 ] && echo PASS || { echo "$fails FAILED"; exit 1; }
