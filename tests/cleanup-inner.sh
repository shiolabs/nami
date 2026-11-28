#!/bin/bash
# Runs inside a privileged Ubuntu container; see tests/cleanup.sh.
set -u
fails=0
ok() { echo "  ok   $1"; }
bad() { echo "  FAIL $1"; fails=$((fails + 1)); }
check() { if eval "$2" >/tmp/out 2>&1; then ok "$1"; else bad "$1"; sed 's/^/       /' /tmp/out | tail -8; fi; }

/src/install.sh /new/nami >/dev/null
nami update --from "$INDEX" --unsigned >/dev/null 2>&1
nami daemon 2>/tmp/daemon.log &
sleep 1
P=/nix/var/nami/profiles
later() { echo $(($(date +%s) + $1 * 86400)); }
current() { readlink "$1/default" | sed 's/default-//'; }
# A package's store path in a change (of the computer's, or a person's profile folder).
path_of() { jq -r --arg a "$3" '.packages[] | select(.attr == $a) | .paths[0]' "$1/default-$2.json"; }
as() { su - "$1" -c "$2"; }

echo "== removing says what the change before still holds"
nami install hello >/dev/null 2>&1
G1=$(current $P)
nami install ripgrep >/dev/null 2>&1
G2=$(current $P)
RG=$(path_of $P "$G2" ripgrep)
# ripgrep left running from it, as an app still open after it's removed.
sleep 100000 | "$RG/bin/rg" never-there >/dev/null 2>&1 &
nami remove ripgrep >/tmp/remove 2>&1
G3=$(current $P)
check "it says how much can be freed, and that it goes by itself" "grep -E '^  [0-9.]+ MB can be freed now with nami gc --old · or it goes by itself after 30 days$' /tmp/remove"
size=$(grep -oE '^  [0-9.]+ MB' /tmp/remove | tr -dc 0-9.)
check "a sensible size: ripgrep's few MB (${size:-none})" "awk 'BEGIN { exit !(\"$size\" + 0 >= 1 && \"$size\" + 0 < 60) }'"
nami install jq >/dev/null 2>&1
nami install tree >/dev/null 2>&1
G5=$(current $P)
# A project's tools, pinned by its nami.lock: kept while the project is there.
mkdir -p /proj && printf 'packages:\n  - cowsay\n' > /proj/project.nami
COW=$(cd /proj && nami run -- sh -c 'readlink -f "$(command -v cowsay)"' 2>/dev/null | sed -E 's|^(/nix/store/[^/]+).*|\1|')
check "the project's cowsay is in the store ($COW)" "[ -n '$COW' ] && [ -e '$COW' ]"

echo "== a person's own: as her file says (keep: 1)"
useradd -m -s /bin/bash alice
A=$P/per-user/alice
as alice 'printf "cleanup:\n  keep: 1\n" > ~/user.nami'
as alice 'nami install htop' >/dev/null 2>&1
HTOP=$(path_of $A "$(current $A)" htop)
as alice 'nami remove htop' >/tmp/aremove 2>&1
check "her hint: it's freed by itself, or sooner" "grep -E '^  [0-9.]+ MB is freed by itself after 30 days · or sooner with nami gc --old$' /tmp/aremove"

echo "== a day on: nothing is old enough"
check "the daily cleanup says nothing" "[ -z \"\$(nami gc --auto 2>&1)\" ]"
check "everything's there" "[ -e $P/default-$G1.json ] && [ -e '$RG' ] && [ -e '$HTOP' ]"

echo "== forty days on"
NAMI_NOW=$(later 40) nami gc --auto >/tmp/auto 2>&1
check "it says what went, in a line" "grep -qE \"^Cleaned up: changes [0-9, –]+, alice's change 1, [0-9.]+ [MGk]B freed\$\" /tmp/auto && [ \$(wc -l </tmp/auto) = 1 ]"
check "the old changes are gone; the current one and the newest three stay" "[ ! -e $P/default-$G1.json ] && [ ! -e $P/default-$G2 ] && [ -e $P/default-$G3.json ] && [ -e $P/default-$G5.json ] && [ \$(current $P) = $G5 ]"
check "her old change is gone (keep: 1), her current one stays" "[ ! -e $A/default-1.json ] && [ -e $A/default-\$(current $A).json ]"
check "what only her old change used is freed" "[ ! -e '$HTOP' ]"
check "hello, still in use, stays" "[ -e \"\$(path_of $P $G5 hello)\" ]"
check "the project's pinned tools stay" "[ -e '$COW' ]"
check "ripgrep stays while it runs" "[ -e '$RG' ] && '$RG/bin/rg' --version"
pkill -f never-there
sleep 1
NAMI_NOW=$(later 40) nami gc --auto >/tmp/auto2 2>&1
check "closed, it goes too" "[ ! -e '$RG' ] && grep -qE '^Cleaned up: [0-9.]+ [MGk]B freed$' /tmp/auto2"
check "then there's nothing to say" "[ -z \"\$(NAMI_NOW=$(later 40) nami gc --auto 2>&1)\" ]"
rm -rf /proj
NAMI_NOW=$(later 40) nami gc --auto >/dev/null 2>&1
check "the project gone, its tools go" "[ ! -e '$COW' ]"

echo "== undo, past what was cleaned up"
check "it says the change was cleaned up" "! nami undo $G2 && nami undo $G2 2>&1 | grep -q 'change $G2 was cleaned up'"
check "undo to one that's kept works" "nami undo $G3 && [ \$(current $P) = $G3 ]"
nami undo "$G5" >/dev/null 2>&1

echo "== space low: early"
printf 'cleanup:\n  keep: 1\n' >> /etc/nami/system.nami
NAMI_SPACE_LOW=1 nami install sl >/tmp/low 2>&1
G6=$(current $P)
check "nami install says it once" "[ \$(grep -c 'Space is low, so old changes were cleaned up early: changes' /tmp/low) = 1 ]"
check "whatever their age; the current one stays, and what it made" "[ ! -e $P/default-$G3.json ] && [ -e $P/default-$G5.json ] && [ -e $P/default-$G6.json ] && [ -e $P/default/bin/sl ] || { ls $P; cat /tmp/low; false; }"

echo "== Clean Up Now (Settings): by the file's rules, without waiting"
nami remove sl >/dev/null 2>&1
check "the dry run says what goes, and how many stay" "nami gc --now --dry-run --json | jq -e '.keep == 1 and (.changes | length) == 2 and .bytes > 0'"
nami gc --now >/tmp/now 2>&1
check "it says each as it goes (Storage's bar)" "grep -qE '^removing [0-9]+ paths' /tmp/now && grep -qE '^  [0-9a-z]{32}-sl-' /tmp/now"
check "only the current change is left" "[ \$(ls $P | grep -cE '^default-[0-9]+[.]json\$') = 1 ]"

echo "== Undo goes back to what a change was made from"
# Alice: + jq, + tree, undo (tree goes), + fd (made from the change before tree), undo:
# fd goes, and tree stays gone (before, undo went forward to the change with tree).
as alice 'nami install jq' >/dev/null 2>&1
as alice 'nami install tree' >/dev/null 2>&1
as alice 'nami undo' >/dev/null 2>&1
as alice 'nami install fd' >/dev/null 2>&1
as alice 'nami undo' >/tmp/undo2 2>&1
as alice 'nami list' 2>&1 | sed -n '/Yours/,$p' >/tmp/alist  # her own (the system's are listed too)
check "fd went" "! grep -qw fd /tmp/alist || { cat /tmp/undo2 /tmp/alist; false; }"
check "tree didn't come back" "! grep -qw tree /tmp/alist || { cat /tmp/undo2 /tmp/alist; false; }"
check "jq stayed" "grep -qw jq /tmp/alist || { cat /tmp/alist; false; }"

echo
[ "$fails" -eq 0 ] && echo PASS || { echo "$fails FAILED"; tail -5 /tmp/daemon.log; exit 1; }
