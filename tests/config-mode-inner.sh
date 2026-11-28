#!/bin/bash
# Runs inside a privileged Ubuntu container; see tests/config-mode.sh.
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
# Inside a desktop session (dbus + dconf), like a terminal in GNOME.
as() { su - alice -c "dbus-run-session -- bash -lc \"$1\"" 2>&1; }
C=/home/alice/user.nami
mkdir -p /home/alice/.config/nami && cat > $C <<'YAML'
# Alice's laptop. Edited by hand AND by nami.
packages:
  - jq   # JSON on the command line
  - hello
YAML
chown -R alice: /home/alice/.config $C

echo "== the nami file is the source of truth"
check "apply" "as 'nami apply'"
check "install edits nami file (comments kept) and applies" "as 'nami install tree' && grep -q '^  - tree' $C && grep -q '# JSON on the command line' $C && as 'nami list --json' | grep -q '\"attr\":\"tree\"'"
check "the lock follows" "grep -q '\"tree\"' /home/alice/.config/nami/nami.lock"
check "remove edits it too" "as 'nami remove hello' && ! grep -q hello $C && ! as 'nami list --json' | grep -q '\"attr\":\"hello\"'"
cp $C /tmp/before-bad
check "a failing install leaves the nami file as it was" "! as 'nami install no-such-package-xyz' && cmp -s $C /tmp/before-bad"
check "rollback restores the nami file and nami.lock with the profile" "as 'nami rollback' && grep -q hello $C && as 'nami list --json' | grep -q '\"attr\":\"hello\"'"
check "nami config set keeps comments" "as 'nami config set env.EDITOR nvim' && grep -q 'EDITOR: nvim' $C && grep -q '# JSON on the command line' $C"
check "nami config add/remove packages" "as \"nami config add packages ripgrep && nami config remove packages tree\" && grep -q '^  - ripgrep' $C && ! grep -q '^  - tree' $C"
check "nami config get reads a file from stdin" "printf 'packages:\\n  - gimp\\n' | nami config get --json --file - | grep -qx '{\"packages\":\\[\"gimp\"\\]}'"
check "apply picks it up (in a new shell)" "as 'nami apply' && as 'echo \\\$EDITOR' | grep -qx nvim"

echo "== desktop settings"
cat >> $C <<'YAML'
settings:
  org.gnome.desktop.interface:
    color-scheme: prefer-dark
    clock-format: 12h
    text-scaling-factor: 1.25
YAML
check "check: no problems" "as 'nami check' | grep -q 'No problems'"
check "apply sets them in dconf" "as 'nami apply && gsettings get org.gnome.desktop.interface color-scheme' | grep -qx \"'prefer-dark'\""
check "numbers too" "as 'gsettings get org.gnome.desktop.interface text-scaling-factor' | grep -qx '1.25'"
cp $C /tmp/good.yaml
sed -i 's/color-scheme: prefer-dark/colour-scheme: prefer-dark/' $C
check "a misspelled key: error with did-you-mean" "as 'nami check' | grep -q \"no key colour-scheme (did you mean .color-scheme.?)\""
sed -i 's/colour-scheme: prefer-dark/color-scheme: darkish/' $C
check "a wrong enum value: error listing the allowed ones" "as 'nami check' | grep -q 'one of: default, prefer-dark, prefer-light'"
cp /tmp/good.yaml $C && sed -i '/clock-format/d' $C && chown alice: $C
check "removing a key resets it" "as 'nami apply && gsettings get org.gnome.desktop.interface clock-format' | grep -qx \"'24h'\""
check "rollback restores the earlier value" "as 'nami rollback && gsettings get org.gnome.desktop.interface clock-format' | grep -qx \"'12h'\""

echo
[ "$fails" -eq 0 ] && echo PASS || { echo "$fails FAILED"; exit 1; }
