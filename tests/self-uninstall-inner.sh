#!/bin/bash
# Runs inside a privileged Ubuntu container; see tests/self-uninstall.sh.
# $1: how nami is uninstalled once the refusals are checked:
#   typed     sudo nami self-uninstall, `uninstall` typed
#   yes       sudo nami self-uninstall --yes
#   web       curl .../uninstall | sh, `uninstall` typed on the terminal
#   web-yes   curl .../uninstall | sh -s -- --yes
set -u
mode=$1
fails=0
ok() { echo "  ok   $1"; }
bad() { echo "  FAIL $1"; fails=$((fails + 1)); }
check() { if eval "$2" >/tmp/out 2>&1; then ok "$1"; else bad "$1"; sed 's/^/       /' /tmp/out | tail -14; fi; }
# `in_tty <typed> <command>`: the command on a terminal of its own, what's typed on it.
in_tty() { printf '%s\n' "$1" | script -qec "$2" /dev/null; }

apt-get update -qq >/dev/null && apt-get install -y -qq curl ca-certificates python3 openssl >/dev/null 2>&1
mkdir -p /etc/systemd/system
printf '#!/bin/sh\necho "$@" >> /tmp/systemctl.log\n' > /usr/local/sbin/systemctl && chmod +x /usr/local/sbin/systemctl
# Something in the store that isn't nami's (as Nix's would be): it stays.
mkdir -p /nix/store/00000000000000000000000000000000-not-namis

# The release site, as packaging/publish.sh lays it out (as tests/no-app-inner.sh does).
k() { NAMI_ROOT=/keys /new/nami "$@"; }
mkdir -p /keys/nix/var/nami
key=$(k keygen test-1 2>/dev/null | grep '^test-1:')
S=/site rev=0123456789ab B=http://localhost:8000 v=9.9.8
mkdir -p $S/menu/$rev $S/releases/$v /tmp/nami-$v
cp /i.xz $S/menu/$rev/index-x86_64-linux.json.xz
k catalog sign $S/menu/$rev/index-x86_64-linux.json.xz --key test-1 >/dev/null
echo $rev > $S/menu/nixos-unstable
cp /new/nami /tmp/nami-$v/nami; cp /src/install.sh /src/uninstall.sh /src/packaging/setup.sh /tmp/nami-$v/
tar -C /tmp -czf $S/releases/$v/nami-$v-x86_64-linux.tar.gz nami-$v
(cd $S/releases/$v && sha256sum nami-$v-x86_64-linux.tar.gz > SHA256SUMS)
k catalog sign $S/releases/$v/SHA256SUMS --key test-1 >/dev/null
echo $v > $S/releases/latest
sed -e "s|@BASE@|$B|g" -e "s|@CHANNEL@|nixos-unstable|g" -e "s|@KEYS@|$key|g" -e "s|@KEY@|$key|g" /src/packaging/install-web.sh > $S/install
sed -e "s|@BASE@|$B|g" /src/packaging/uninstall-web.sh > $S/uninstall
(cd $S && python3 -m http.server 8000 >/tmp/http.log 2>&1 &)
sleep 2

echo "== the web uninstaller, before nami's here"
check "it says nami isn't installed, and does nothing" "curl -fsSL $B/uninstall | sh -s -- --yes | tee /dev/stderr | grep -q \"nami isn't installed on this computer\""

echo "== installing ($mode)"
curl -fsSL $B/install | sh -s -- --no-app > /tmp/install.log 2>&1
rc=$?
sed 's/^/       /' /tmp/install.log | tail -4
check "the installer finished" "[ $rc = 0 ] && grep -q 'nami $v is installed' /tmp/install.log"
check "and says how to uninstall nami: sudo nami self-uninstall" "grep -q 'Uninstall nami again with: sudo nami self-uninstall' /tmp/install.log && ! grep -q 'uninstall.sh' /tmp/install.log"
. /etc/profile.d/nami.sh
nami install hello >/tmp/hello.log 2>&1 || cat /tmp/hello.log
# Someone's files: their nami files and their home.
useradd -m ada
mkdir -p /home/ada/.config/nami /home/ada/Documents
printf 'packages:\n  - hello\n' > /home/ada/user.nami
echo 'export EDITOR=nano' > /home/ada/.config/nami/env.sh
echo "Notes" > /home/ada/Documents/notes.txt
chown -R ada: /home/ada
cp /etc/nami/system.nami /tmp/system.nami.before

# Everything of nami's that's here now, to be checked again.
there() {
  for f in /nami/bin/nami /nami/uninstall.sh /usr/local/bin/nami /etc/profile.d/nami.sh /etc/tmpfiles.d/nami.conf; do
    [ -e "$f" ] || { echo "missing: $f"; return 1; }
  done
  nami list --json | grep -q '"hello-' || { echo "hello isn't installed: $(nami list --json | head -c 300)"; return 1; }
}
untouched() { there && { cmp /etc/nami/system.nami /tmp/system.nami.before || { diff /tmp/system.nami.before /etc/nami/system.nami; false; }; }; }
check "nami and hello are installed" "there"

echo "== help and Tab completion"
check "nami help lists self-uninstall" "nami help | grep -q 'self-uninstall'"
check "nami self-uninstall --help says it's nami itself" "nami self-uninstall --help | grep -q 'Uninstall nami itself from this computer'"
check "Tab completes it" "nami __complete bash -- self-un | grep -q '^self-uninstall'"

echo "== nami uninstall is nami remove"
for c in uninstall remove; do
  check "nami $c alone: which apps, and how nami itself goes (exit 2)" "nami $c > /tmp/u.log 2>&1; rc=\$?; cat /tmp/u.log; [ \$rc = 2 ] && grep -q 'which apps' /tmp/u.log && grep -q 'To uninstall nami itself: sudo nami self-uninstall' /tmp/u.log"
done
check "nami uninstall hello removes hello" "nami uninstall hello && ! nami list --json | grep -q '\"hello-'"
check "and nami is still here" "[ -x /nami/bin/nami ] && [ -L /usr/local/bin/nami ]"
nami install hello >/dev/null 2>&1
check "hello installed again" "untouched"

echo "== what doesn't uninstall"
check "nami self-uninstall firefox: removes nami itself, try nami remove firefox (exit 2)" \
  "nami self-uninstall firefox > /tmp/s.log 2>&1; rc=\$?; cat /tmp/s.log; [ \$rc = 2 ] && grep -q 'nami self-uninstall removes nami itself. To remove firefox, run: nami remove firefox' /tmp/s.log"
check "  nothing was removed" "untouched"
check "without sudo: one line, sudo nami self-uninstall (exit 77)" \
  "runuser -u ada -- /nami/bin/nami self-uninstall > /tmp/s.log 2>&1; rc=\$?; cat /tmp/s.log; [ \$rc = 77 ] && [ \$(wc -l < /tmp/s.log) = 1 ] && grep -q 'sudo nami self-uninstall' /tmp/s.log"
check "  nothing was removed" "untouched"
check "the warning: nami itself, not a package, what goes, what stays" \
  "printf 'no\n' | nami self-uninstall > /tmp/s.log 2>&1; cat /tmp/s.log
   grep -q 'This uninstalls nami itself from this computer, not a package: nami, every app and tool installed with it, and its store (/nix/store).' /tmp/s.log &&
   grep -q 'your nami files: ~/user.nami, ~/.config/nami, and the system.s in /etc/nami' /tmp/s.log &&
   grep -q 'your home folder and everything in it' /tmp/s.log && grep -q 'To remove one app instead, use nami remove <name>.' /tmp/s.log &&
   grep -q 'Type uninstall to uninstall nami' /tmp/s.log"
for word in no y yes "" "uninstall nami"; do
  check "typed '$word': Nothing was removed. (exit 1)" "printf '%s\n' '$word' | nami self-uninstall > /tmp/s.log 2>&1; rc=\$?; tail -1 /tmp/s.log; [ \$rc = 1 ] && tail -1 /tmp/s.log | grep -q 'Nothing was removed.'"
  check "  everything's still here" "untouched"
done
check "no answer at all (no terminal, no --yes): Nothing was removed." "nami self-uninstall < /dev/null > /tmp/s.log 2>&1; [ \$? = 1 ] && grep -q 'Nothing was removed.' /tmp/s.log && untouched"
check "the web uninstaller: firefox isn't nami (exit 2)" "curl -fsSL $B/uninstall | sh -s -- firefox > /tmp/w.log 2>&1; rc=\$?; cat /tmp/w.log; [ \$rc = 2 ] && grep -q 'To remove firefox, run: nami remove firefox' /tmp/w.log && untouched"
check "the web uninstaller asks on the terminal: 'no' removes nothing" \
  "in_tty no 'curl -fsSL $B/uninstall | sh' > /tmp/w.log 2>&1; cat /tmp/w.log; grep -q 'not a package' /tmp/w.log && grep -q 'Nothing was removed.' /tmp/w.log && untouched"
check "the web uninstaller with no terminal and no --yes removes nothing" \
  "curl -fsSL $B/uninstall | setsid sh > /tmp/w.log 2>&1; cat /tmp/w.log; grep -q 'run it with --yes' /tmp/w.log && grep -q 'Nothing was removed.' /tmp/w.log && untouched"

echo "== uninstalling ($mode)"
case $mode in
  typed) printf 'uninstall\n' | nami self-uninstall > /tmp/un.log 2>&1 ;;
  yes) nami self-uninstall --yes > /tmp/un.log 2>&1 ;;
  web) in_tty uninstall "curl -fsSL $B/uninstall | sh" > /tmp/un.log 2>&1 ;;
  web-yes) curl -fsSL $B/uninstall | sh -s -- --yes > /tmp/un.log 2>&1 ;;
esac
rc=$?
sed 's/^/       /' /tmp/un.log | tail -8
check "it finished" "[ $rc = 0 ] && grep -q 'nami removed' /tmp/un.log"
hash -r
check "nami's command is gone" "! command -v nami"
for f in /nami /nix/var/nami /etc/profile.d/nami.sh /etc/environment.d/50-nami.conf /etc/systemd/user-environment-generators/50-nami \
         /etc/tmpfiles.d/nami.conf /usr/local/bin/nami /run/opengl-driver /usr/local/share/bash-completion/completions/nami \
         /etc/systemd/system/nami-daemon.service /etc/systemd/system/nami-update.timer /usr/share/polkit-1/actions/org.namios.change-computer.policy; do
  check "gone: $f" "[ ! -e $f ] && [ ! -L $f ]"
done
check "the store has only what wasn't nami's" "[ \"\$(ls /nix/store)\" = 00000000000000000000000000000000-not-namis ]"
check "stays: /etc/nami/system.nami, as it was" "cmp /etc/nami/system.nami /tmp/system.nami.before"
check "stays: ada's user.nami, ~/.config/nami and her files" "[ -f /home/ada/user.nami ] && [ -f /home/ada/.config/nami/env.sh ] && [ \"\$(cat /home/ada/Documents/notes.txt)\" = Notes ]"
check "the web uninstaller, again: nami isn't installed" "curl -fsSL $B/uninstall | sh | grep -q \"nami isn't installed on this computer\""

echo
[ "$fails" = 0 ] && echo "all passed" || { echo "$fails failed"; exit 1; }
