#!/bin/bash
# Runs inside a privileged Ubuntu container; see tests/users.sh.
set -u
fails=0
ok() { echo "  ok   $1"; }
bad() { echo "  FAIL $1"; fails=$((fails + 1)); }
check() { if eval "$2" >/tmp/out 2>&1; then ok "$1"; else bad "$1"; sed 's/^/       /' /tmp/out | tail -8; fi; }
check_fails() { if eval "$2" >/tmp/out 2>&1; then bad "$1 (succeeded!)"; else ok "$1: $(grep -m1 error /tmp/out)"; fi; }

/src/install.sh /new/nami >/dev/null
nami update --from /i.xz --unsigned --drvs /drvs >/dev/null
nami daemon 2>/tmp/daemon.log &
sleep 1

echo "== root: generations, diff, rollback"
nami install hello >/dev/null 2>&1
nami install jq >/dev/null 2>&1
nami remove hello >/dev/null 2>&1
nami generations
check "diff shows hello removed" "nami diff | grep -- '− hello'"
check "diff 1 2 shows jq added" "nami diff 1 2 | grep -- '+ jq'"
check "rollback prints what changed" "nami rollback | grep -- '+ hello'"

echo "== root: caches belong to generations"
mkdir -p /tmp/emptycache
cat > /tmp/sys.yaml <<EOF
caches:
  - url: file:///tmp/emptycache
    key: test-1:6NCHdD59X431o0gWypbMrAURkbJ16ZPMQFGspcDShjY=
packages: [hello, jq]
EOF
nami apply /tmp/sys.yaml >/dev/null 2>&1
check "caches.json has the cache" "grep -q emptycache /nix/var/nami/caches.json"
check "generations show the cache" "nami generations | tail -1 | grep '1 caches'"
check "diff shows + cache" "nami diff | grep '+ cache file:///tmp/emptycache'"
nami rollback >/dev/null 2>&1
check "rollback restores the old cache list" "! grep -q emptycache /nix/var/nami/caches.json"
nami rollback 5 >/dev/null 2>&1

echo "== root: pruning"
for p in ripgrep fd; do nami install $p >/dev/null 2>&1; done
n=$(nami generations | wc -l)
check "--older-than 1h keeps everything recent" "nami gc --older-than 1h | grep -v 'removed generations'"
check "--keep 2 leaves 2 generations" "nami gc --keep 2 >/dev/null && [ \$(nami generations | wc -l) -eq 2 ]"
echo "     ($n generations before, $(nami generations | wc -l) after)"
printf 'packages: [hello]\nhistory: 1\n' > /tmp/keep.yaml
check "history in your nami file prunes on apply" "nami apply /tmp/keep.yaml && [ \$(nami generations | wc -l) -eq 1 ]"

echo "== alice: own profile, no sudo"
useradd -m alice
as_alice() { su - alice -c "$1"; }
check "install from nixpkgs" "as_alice 'nami install ripgrep cowsay'"
check "on her PATH via ~/.local/state/nami/profile" "as_alice 'rg --version && cowsay hej' | grep -q ripgrep"
check "not in root's profile" "! nami list | grep -q ripgrep"
check "builds a recipe (the daemon runs it)" "as_alice 'cp -r /ex /home/alice/ex && nami install ./ex/zcat-lite'"
check "runs it" "as_alice 'zcat-lite --version' | grep 'zcat-lite for nami'"
check "extends: works too" "as_alice 'nami build ./ex/jq-patched'"
check "her generations" "as_alice 'nami generations' | grep -c packages | grep -q 2"
check "her rollback" "as_alice 'nami rollback' && ! as_alice 'command -v zcat-lite'"

echo "== bob: his very first install, undone"
useradd -m bob
as_bob() { su - bob -c "$1"; }
as_bob 'nami install cowsay' >/dev/null 2>&1
check "nami undo takes his first install back" "as_bob 'nami undo' && ! as_bob 'command -v cowsay'"
check "and out of his user.nami" "! grep -q cowsay /home/bob/user.nami"
check "nami undo 1 brings it back" "as_bob 'nami undo 1' && as_bob 'cowsay hej' && grep -q cowsay /home/bob/user.nami"
check "past his first change: nothing to undo" "as_bob 'nami undo' && ! as_bob 'nami undo' && as_bob 'nami undo' 2>&1 | grep -q 'before your first change'"

echo "== carol, an admin: sudo decides (plain apply and undo are hers, --system the computer's)"
useradd -m -G sudo carol
as_carol() { su - carol -c "$1"; }
# (The computer keeping its changes again: above, it kept one.)
printf 'packages:\n  - hello\n' > /etc/nami/system.nami && nami apply >/dev/null 2>&1
M=/home/carol/.local/state/nami/profile/bin
as_carol 'nami install cowsay' >/dev/null 2>&1
check "for everyone: says sudo nami undo" "as_carol 'nami install --system figlet' 2>&1 | grep -q 'Undo with sudo nami undo'"
check "nami undo: hers (cowsay), the computer's left as it is" "as_carol 'nami undo' && [ ! -e $M/cowsay ] && as_carol 'command -v figlet'"
check "nami undo again: nothing of hers, and it says where the computer's are" "! as_carol 'nami undo' && as_carol 'nami undo' 2>&1 | grep -q 'sudo nami undo'"
check "nami undo --user is hers too" "as_carol 'nami undo --user 1' && [ -e $M/cowsay ]"
check "nami history is hers; --system the computer's" "as_carol 'nami history' | grep -q cowsay && as_carol 'nami history --system' | grep -q figlet && ! as_carol 'nami history' | grep -q figlet"
check "nami undo --system: the computer's (figlet)" "as_carol 'nami undo --system' && ! as_carol 'command -v figlet' && [ -e $M/cowsay ]"
printf 'packages:\n  - ripgrep\n' > /home/carol/user.nami && chown carol /home/carol/user.nami
printf 'packages:\n  - hello\n  - jq\n' > /etc/nami/system.nami
check "nami apply: her file, and it says so" "as_carol 'nami apply' 2>&1 | grep -q 'Applied ~/user.nami' && [ -e $M/rg ] && ! test -e /nix/var/nami/profiles/default/bin/jq"
check "nami apply --system: the system's, and it says so" "as_carol 'nami apply --system' 2>&1 | grep -q \"Applied system.nami\" && test -e /nix/var/nami/profiles/default/bin/jq"
check "root's nami apply: the system's" "printf 'packages:\n  - hello\n' > /etc/nami/system.nami && nami apply 2>&1 | grep -q \"system.nami\" && ! test -e /nix/var/nami/profiles/default/bin/jq && [ -e $M/rg ]"

echo "== alice: boundaries"
check_fails "can't write the store directly" "as_alice 'touch /nix/store/evil'"
check_fails "can't update the index" "as_alice 'nami update --from /i.xz --unsigned'"
check_fails "can't use a cache root doesn't trust" "as_alice 'printf \"caches:\\n  - url: https://evil.example\\n    key: evil-1:6NCHdD59X431o0gWypbMrAURkbJ16ZPMQFGspcDShjY=\\npackages: []\\n\" > y.yaml && nami apply y.yaml'"
mkdir -p /tmp/leak && printf 'name: leak\nversion: "1"\nsource: { path: /root/secret }\ninstall: cp -r . $out\n' > /tmp/leak/package.nami
echo "root-only" > /root/secret; chmod 700 /root
check_fails "can't copy root-only files into the store" "as_alice 'nami build /tmp/leak'"

echo "== alice: GUI app gets GPU drivers from the system profile"
# On a desktop, logind gives the logged-in user the GPU; here, the device's group does.
if [ -e /dev/dri/renderD128 ]; then
  gid=$(stat -c %g /dev/dri/renderD128)
  getent group "$gid" >/dev/null || groupadd -g "$gid" gpu
  usermod -aG "$(getent group "$gid" | cut -d: -f1)" alice
fi
check "install mesa-demos" "as_alice 'nami install mesa-demos'"
check "system profile now has mesa" "nami doctor | grep 'For apps with windows' | grep -q mesa"
check "her eglinfo renders" "as_alice 'eglinfo -B -p surfaceless' | grep 'core profile renderer'"
grep 'core profile renderer' /tmp/out | head -1 | sed 's/^/       /'
[ -e /dev/dri/renderD128 ] && check "on the GPU, not in software" "! grep -q llvmpipe /tmp/out"

echo "== root GC keeps users' packages"
nami gc --old >/dev/null 2>&1
check "alice's rg survives root's gc" "as_alice 'rg --version'"
check "store paths are still all accounted for" "nami gc | grep 'Nothing to free'"

echo
[ "$fails" -eq 0 ] && echo PASS || { echo "$fails FAILED"; echo "--- daemon log"; tail -5 /tmp/daemon.log; exit 1; }
