#!/bin/bash
# Runs inside a privileged Ubuntu container; see tests/drift.sh.
set -u
fails=0
ok() { echo "  ok   $1"; }
bad() { echo "  FAIL $1"; fails=$((fails + 1)); }
check() { if eval "$2" >/tmp/out 2>&1; then ok "$1"; else bad "$1"; sed 's/^/       /' /tmp/out | tail -12; fi; }

/src/install.sh /new/nami >/dev/null
nami update --from /i.xz --unsigned >/dev/null 2>&1
nami daemon 2>/tmp/daemon.log &
sleep 1
useradd -m -s /bin/bash alice
as_alice() { su - alice -c "$1"; }
S=/home/alice/.config/Code/User/settings.json
as_alice 'cat > ~/user.nami' <<'YAML'
# Alice's laptop
packages:
  - hello
files:
  ~/.config/Code/User/settings.json:
    mutable: true   # VS Code may change it too
    content:
      editor.fontSize: 14
      workbench.colorTheme: Default Dark Modern
YAML
as_alice 'nami apply ~/user.nami' >/dev/null 2>&1

echo "== an app changes its settings"
# What VS Code writes: comments, its own layout, a trailing comma.
as_alice "cat > $S" <<'JSON'
{
    // changed in VS Code
    "editor.fontSize": 16,
    "workbench.colorTheme": "Default Dark Modern",
    "files.autoSave": "afterDelay",
}
JSON
check "nami home diff shows each changed setting" "as_alice 'nami home diff' | grep -q 'editor.fontSize: 14 -> 16' && as_alice 'nami home diff' | grep -q 'files.autoSave: (not set) -> afterDelay'"
check "and as JSON for the GUI" "as_alice 'nami home diff --json' | grep -q '\"path\":\\[\"editor.fontSize\"\\]'"
check "applying the unchanged spec leaves the app's changes alone" "as_alice 'nami apply ~/user.nami' && grep -q '\"editor.fontSize\": 16' $S"

echo "== keep"
check "nami home keep writes them into the spec" "as_alice 'nami home keep' && grep -q 'editor.fontSize: 16' /home/alice/user.nami && grep -q 'files.autoSave: afterDelay' /home/alice/user.nami"
check "comments in the spec survive" "grep -q \"# Alice's laptop\" /home/alice/user.nami && grep -q '# VS Code may change it too' /home/alice/user.nami"
check "nothing differs any more" "as_alice 'nami home diff' | grep -q 'says what your spec says'"

echo "== merge: the app and the spec both change it"
as_alice "sed -i 's/\"editor.fontSize\": 16/\"editor.fontSize\": 18/' $S"
as_alice "sed -i 's/workbench.colorTheme: Default Dark Modern/workbench.colorTheme: Solarized Light/' ~/user.nami"
check "apply merges setting by setting" "as_alice 'nami apply ~/user.nami' 2>&1 | grep -q 'merged ~/.config/Code/User/settings.json'"
check "the app's change stayed" "grep -q '\"editor.fontSize\": 18' $S"
check "the spec's change arrived" "grep -q 'Solarized Light' $S"
as_alice "sed -i 's/\"editor.fontSize\": 18/\"editor.fontSize\": 20/' $S"
as_alice "sed -i 's/editor.fontSize: 16/editor.fontSize: 12/' ~/user.nami"
check "the same setting changed in both: the spec wins, and says so" "as_alice 'nami apply ~/user.nami' 2>&1 | grep -q 'editor.fontSize changed in both' && grep -q '\"editor.fontSize\": 12' $S"
check "the app's version is kept in a backup" "ls /home/alice/.config/Code/User/ | grep -q 'settings.json.*backup\\|settings.json.nami'"

echo "== undo"
as_alice "sed -i 's/\"editor.fontSize\": 12/\"editor.fontSize\": 30/' $S"
check "nami home undo puts the spec's version back" "as_alice 'nami home undo ~/.config/Code/User/settings.json' && grep -q '\"editor.fontSize\": 12' $S && as_alice 'nami home diff' | grep -q 'says what your spec says'"
check "and the file stays editable" "test -w $S && ! test -L $S"

echo
[ "$fails" -eq 0 ] && echo PASS || { echo "$fails FAILED"; exit 1; }
