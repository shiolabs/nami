#!/bin/bash
# Inside the fresh Ubuntu, as root; see tests/fresh-ubuntu.sh.
# A test: namios.dev counts its update checks and downloads apart (NAMI_TEST, site/worker.js).
export NAMI_TEST=1
set -u
fails=0
ok() { echo "  ok   $1"; }
bad() { echo "  FAIL $1"; fails=$((fails + 1)); }
check() { if eval "$2" >/tmp/out 2>&1; then ok "$1"; else bad "$1"; sed 's/^/       /' /tmp/out | tail -12; fi; }

echo "== install"
. /etc/os-release; echo "  $PRETTY_NAME"
/src/install.sh /src/target/release/nami >/tmp/install.log 2>&1 || { cat /tmp/install.log; exit 1; }
nami update --from https://namios.dev/menu/nixos-unstable --trust 'tray-2:uHz+KI06voKMb/xTAfd9nd4d8y008M5x5xWFy/pbpIs=' 2>&1 | tail -1
nami daemon 2>/tmp/daemon.log &
sleep 1

# A new user, as `adduser` makes one: uid 1000, ~ from /etc/skel.
echo 'alice:x:1000:1000:Alice:/home/alice:/bin/bash' >> /etc/passwd
echo 'alice:x:1000:' >> /etc/group
echo 'alice:*:20000:0:99999:7:::' >> /etc/shadow
cp -r /etc/skel /home/alice
# Her own user namespace, uid 1000 in it (no setuid su needed).
as_alice() { unshare --user --map-user=1000 --map-group=1000 env -i HOME=/home/alice USER=alice LOGNAME=alice SHELL=/bin/bash TERM=xterm-256color LANG=C.UTF-8 bash -lc "$1"; }
# A terminal: a login bash on a tty, with commands typed into it.
terminal() { printf '%s\nexit\n' "$1" | as_alice 'script -qec "bash -il" /dev/null' 2>&1 | tr -d '\r'; }

echo "== set up from the repo"
check "she isn't root, and has no nami file yet" "as_alice 'test \$(id -u) = 1000 && ! test -e ~/user.nami'"
start=$(date +%s)
as_alice 'nami apply file:///srv/cfg' >/tmp/apply.log 2>&1; rc=$?
echo "  ($(( $(date +%s) - start ))s)"
grep -E '^(  [+~−]|leaving out|everything else|error)' /tmp/apply.log | sed 's/^/    | /'
check "nami apply <repo> worked" "test $rc = 0"
# The user.nami it took (this computer's folder's).
U=$(readlink -f /home/alice/user.nami)
check "~/.config/nami is the repo's checkout" "as_alice 'git -C ~/.config/nami status' || test -d /home/alice/.config/nami/.git"
check "the secrets key stays out of git" "grep -qx /age.key /home/alice/.config/nami/.git/info/exclude"
P=/home/alice/.local/state/nami/profile
for pkg in $(sed -n '/^packages:/,/^[^ ]/s/^  - \([A-Za-z0-9_+-][A-Za-z0-9_.+-]*\)\([ :].*\)*$/\1/p' $U); do
  grep -q "leaving out $pkg:" /tmp/apply.log && continue
  check "package $pkg" "as_alice 'nami list' | grep -q -- '$pkg'"
done

# (`- zsh:` with `login-shell: true` under it)
login=$(awk '/^  - [a-z]+:/ { p = $2; sub(":", "", p) } /^      login-shell: *true/ { print p }' $U)
if [ -n "$login" ]; then
  echo "== a terminal ($login)"
  terminal 'echo "SHELL_IS=${ZSH_VERSION:+zsh}${FISH_VERSION:+fish}"' > /tmp/term.out
  check "it opens $login" "grep -q 'SHELL_IS=$login' /tmp/term.out"
  check "no errors while starting" "! grep -iE 'command not found|no such file|error' /tmp/term.out"
fi

echo
[ $fails -eq 0 ] && echo "all passed" || { echo "$fails failed"; exit 1; }
