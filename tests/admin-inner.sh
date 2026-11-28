#!/bin/bash
# Runs inside a privileged Ubuntu container; see tests/admin.sh.
set -u
fails=0
ok() { echo "  ok   $1"; }
bad() { echo "  FAIL $1"; fails=$((fails + 1)); }
check() { if eval "$2" >/tmp/out 2>&1; then ok "$1"; else bad "$1"; sed 's/^/       /' /tmp/out | tail -8; fi; }

/src/install.sh /new/nami >/dev/null
nami update --from /i.xz --unsigned >/dev/null 2>&1
nami daemon 2>/tmp/daemon.log &
sleep 1
useradd -m -s /bin/bash -G sudo bob     # an admin
useradd -m -s /bin/bash alice           # not one
as() { su - "$1" -c "$2"; }
system_has() { nami list --json | grep -q "\"attr\":\"$1\""; }
# (Their own profile's change in use: an admin's `nami list --json` is the computer's.)
mine_has() { d=/nix/var/nami/profiles/per-user/$1; grep -q "\"attr\": *\"$2\"" "$d/$(readlink "$d/default").json"; }

echo "== who is who"
check "bob is an admin" "as bob 'nami status --json' | grep -q '\"admin\":true'"
check "alice isn't" "as alice 'nami status --json' | grep -q '\"admin\":false'"
check "bob's nami file is his own, the computer's with --system" "[ \"\$(as bob 'cd / && nami config path')\" = /home/bob/user.nami ] && [ \"\$(as bob 'cd / && nami config path --system')\" = /etc/nami/system.nami ]"
check "alice's is her own" "[ \"\$(as alice 'cd / && nami config path')\" = /home/alice/user.nami ]"

echo "== bob's apps are his own; for everyone when he says so, or when they set up the computer"
as bob 'printf "packages:\n  - jq\nenv:\n  EDITOR: nano\n" > ~/user.nami && nami apply ~/user.nami' >/dev/null 2>&1
check "jq is bob's own" "mine_has bob jq || as bob 'ls ~/.local/state/nami/profile/bin/jq'"
check "bob installs ripgrep for himself" "as bob 'cd / && nami install ripgrep' && mine_has bob ripgrep && grep -q ripgrep /home/bob/user.nami"
check "it isn't the computer's" "! system_has ripgrep"
check "his apps stay in his file, not moved to the computer's" "grep -q jq /home/bob/user.nami && grep -q EDITOR /home/bob/user.nami && ! system_has jq"
check "bob installs tree for everyone, without sudo" "as bob 'cd / && nami install --system tree' && system_has tree && grep -q tree /etc/nami/system.nami && ! grep -q tree /home/bob/user.nami"
check "everyone gets tree" "as alice 'command -v tree'"
check "nami list shows both, each said whose" "as bob 'nami list' | grep -q 'System · system.nami' && as bob 'nami list' | grep -q 'Yours · user.nami' && as alice 'nami list' | grep -q 'System · system.nami'"
check "nami status: the computer's and his own" "as bob 'nami status' | grep -q 'System · for everyone' && as bob 'nami status' | grep -q 'bob · only you'"
check "nami upgrade is his own; --system the computer's (through the daemon)" "as bob 'nami upgrade' && as bob 'nami upgrade --system' && grep -q 'bob: nami upgrade --system' /tmp/daemon.log && ! grep -q 'bob: nami upgrade\$' /tmp/daemon.log"
check "alice can't install for everyone" "as alice 'nami install --system hello; echo rc=\$?' 2>&1 | grep -q 'requires an administrator' && ! system_has hello"
check "nor read the computer's file through the daemon (no polkit in here to ask for an admin's password)" "! as alice 'nami config get --json --file /etc/nami/system.nami'"
check "/etc/nami belongs to the admins: the computer's file theirs to read, root's to write" "[ \"\$(stat -L -c %G /etc/nami)\" = sudo ] && [ \"\$(stat -c %a:%U:%G /etc/nami/system.nami)\" = 640:root:sudo ]"
check "bob reads it, and changes it through nami (no polkit in here to ask for his password)" "as bob 'test -r /etc/nami/system.nami && ! test -w /etc/nami/system.nami && nami config set --system timezone Europe/Copenhagen && grep -q Copenhagen /etc/nami/system.nami && nami config unset --system timezone' && ! grep -q Copenhagen /etc/nami/system.nami && [ \"\$(stat -c %a /etc/nami/system.nami)\" = 640 ]"

echo "== removing, from where it is; undo, as an admin"
check "bob removes ripgrep: his own" "as bob 'cd / && nami remove ripgrep' && ! mine_has bob ripgrep && ! grep -q ripgrep /home/bob/user.nami"
check "bob removes tree: everyone's" "as bob 'cd / && nami remove tree' && ! system_has tree && ! grep -q tree /etc/nami/system.nami"
check "bob's plain undo is his own: ripgrep back, tree still gone" "as bob 'cd / && nami undo' && mine_has bob ripgrep && ! system_has tree"
check "a change for everyone says sudo nami undo takes it back" "as bob 'cd / && nami install --system hello' 2>&1 | grep -q 'Undo with sudo nami undo'"
check "bob's undo --system brings tree back, as sudo nami undo would" "as bob 'cd / && nami undo --system' && ! system_has hello && as bob 'cd / && nami undo --system' && system_has tree && grep -q tree /etc/nami/system.nami"

echo "== the computer's file is only apps"
check "a person's keys there do nothing, and it says so" "printf 'env:\n  X: y\n' >> /etc/nami/system.nami && as bob 'cd / && nami apply --system' 2>&1 | grep -q 'env: only works on namiOS' && sed -i '/^env:/,\$d' /etc/nami/system.nami"

echo "== alice"
check "alice installs her own app" "as alice 'cd / && nami install hello' && mine_has alice hello"
check "it isn't the computer's" "! system_has hello && ! grep -q hello /etc/nami/system.nami"
d=/nix/var/nami/profiles/per-user/alice
check "her file as her steps had it: beside them, hers alone; the steps everyone reads list her apps alone" "ls $d/default-*.config.json >/dev/null && for c in $d/default-*.config.json; do [ \"\$(stat -c %a:%U \$c)\" = 600:alice ] || exit 1; done && ! grep -l '\"text\"' \$(ls $d/default-*.json | grep -v '[.]config[.]json') &&as bob 'nami gc --dry-run' && as alice 'nami history && nami rollback --help >/dev/null'"
check "alice can't read the computer's file, nor change it" "! as alice 'test -r /etc/nami/system.nami' && ! as alice 'test -w /etc/nami/system.nami' && ! as alice 'nami config set timezone Europe/Oslo --file /etc/nami/system.nami' && ! grep -q Oslo /etc/nami/system.nami"
gens=$(nami generations --json | grep -o '"generation"' | wc -l)
as alice 'cd / && nami apply /etc/nami/system.nami' >/dev/null 2>&1
check "her applying the computer's file changes nothing without an admin's name and password" "[ \"\$(nami generations --json | grep -o '\"generation\"' | wc -l)\" = $gens ] && ! grep -q 'alice: nami' /tmp/daemon.log"

echo
[ "$fails" = 0 ] && echo "all passed" || { echo "$fails failed"; exit 1; }
