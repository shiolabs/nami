#!/bin/bash
# Runs inside a privileged container; see tests/nami-dir.sh. $1: fresh, update or namios.
# /new/nami is this nami, /old 0.19.0's release (its nami, install.sh, setup.sh, uninstall.sh).
set -u
fails=0
ok() { echo "  ok   $1"; }
bad() { echo "  FAIL $1"; fails=$((fails + 1)); }
check() { if eval "$2" >/tmp/out 2>&1; then ok "$1"; else bad "$1"; sed 's/^/       /' /tmp/out | tail -8; fi; }
# (cmp isn't in every image.)
same() { [ "$(sha256sum < "$1")" = "$(sha256sum < "$2")" ]; }
done_() { echo; [ "$fails" = 0 ] && { echo "all passed"; exit 0; }; echo "$fails failed"; exit 1; }
moved() {
  check "/nami is nami's folder" '[ -d /nami ] && [ ! -L /nami ] && same /nami/bin/nami /new/nami'
  check "/nix/var/nami is a relative link to it" '[ "$(readlink /nix/var/nami)" = ../../nami ] && same /nix/var/nami/bin/nami /new/nami'
}
# A person (where the image has useradd and su: Fedora's has no su), an admin.
person() { command -v useradd >/dev/null && command -v su >/dev/null && useradd -m -s /bin/bash "$@" 2>/dev/null; }
as() { su - "$1" -c "$2"; }
gone() {
  check "uninstalled: /nami, its link and nami's store paths are gone" '[ ! -e /nami ] && [ ! -L /nami ] && [ ! -e /nix/var/nami ] && [ ! -L /nix/var/nami ] && [ -z "$(ls /nix/store)" ] && [ ! -e /usr/local/bin/nami ]'
}

case $1 in
fresh)
  /src/install.sh /new/nami >/tmp/install.log 2>&1 || { cat /tmp/install.log; exit 1; }
  moved
  check "nami on PATH is /nami's, and only /nami is named" '[ "$(readlink /usr/local/bin/nami)" = /nami/bin/nami ] && grep -q "/nami/bin:" /etc/profile.d/nami.sh && ! grep -q /nix/var/nami /etc/profile.d/nami.sh'
  nami update --from /i.xz --unsigned >/dev/null 2>&1
  nami daemon 2>/tmp/daemon.log &
  daemon=$!
  sleep 1
  check "the daemon listens in /nami" '[ -S /nami/daemon.sock ]'
  check "root's apps, in /nami" 'nami install hello >/dev/null 2>&1 && /nami/profiles/default/bin/hello'
  if person ada; then
    check "a person's own apps, through the daemon" "as ada 'nami install jq' >/dev/null 2>&1 && as ada 'jq --version'"
  fi
  check "running nami again moves nothing" '! nami list 2>&1 | grep -q "/nami now" && [ -L /nix/var/nami ]'
  kill "$daemon"; wait "$daemon" 2>/dev/null
  # (install.sh alone, as here: the uninstaller is the repo's; a release puts it in /nami.)
  sh /src/uninstall.sh >/tmp/uninstall.log 2>&1 || cat /tmp/uninstall.log
  gone
  done_
  ;;
update|namios) ;;
*) echo "usage: $0 fresh|update|namios"; exit 2 ;;
esac

# 0.19.0, with its own installer; systemd's place, and a systemctl that only says
# what it was asked (no systemd runs here).
# (/run/systemd/system: as if it did, so the installers write the daemon's unit.)
mkdir -p /etc/systemd/system /run/systemd/system
printf '#!/bin/sh\necho "$@" >> /tmp/systemctl.log\n' > /usr/local/sbin/systemctl && chmod +x /usr/local/sbin/systemctl
/old/install.sh /old/nami >/dev/null 2>&1
check "0.19.0 keeps its folder in /nix/var/nami" '[ -d /nix/var/nami ] && [ ! -L /nix/var/nami ] && [ ! -e /nami ] && /nix/var/nami/bin/nami --version | grep -q 0.19.0'
nami update --from /i.xz --unsigned >/dev/null 2>&1
nami install hello python3 >/dev/null 2>&1
nami install ripgrep >/dev/null 2>&1 && nami remove ripgrep >/dev/null 2>&1   # (some history)
if [ "$1" = namios ]; then
  # What a computer nami made has: its systems in nami's folder, the one in use
  # (/run/current-system), /etc/static through it. 0.19.0 updates itself there by
  # copying the new nami into its place; the daily update then runs it.
  mkdir -p /nix/var/nami/os/system-1/etc /run/booted-system
  echo "namiOS 1" > /nix/var/nami/os/system-1/etc/issue
  ln -sfn /nix/var/nami/os/system-1 /run/current-system
  ln -sfn /run/current-system/etc /etc/static
  printf '#!/bin/sh\n' > /run/booted-system/activate && chmod +x /run/booted-system/activate
fi
groupadd -f sudo
person bob -G sudo
/nix/var/nami/bin/nami daemon 2>/tmp/daemon.log &
old_daemon=$!
sleep 1
as bob 'nami install jq' >/dev/null 2>&1
check "before: bob's apps, through 0.19.0's daemon" "as bob 'jq --version'"
ls /nix/var/nami/profiles | sort > /tmp/profiles.before

# The release site, laid out as packaging/publish.sh does, signed with a key of our own:
# release 9.9.9 is this nami.
key=$(nami keygen test-1 2>/dev/null)
S=/site rev=0123456789ab
mkdir -p $S/menu/$rev $S/releases/9.9.9
cp /i.xz $S/menu/$rev/index-x86_64-linux.json.xz
nami catalog sign $S/menu/$rev/index-x86_64-linux.json.xz --key test-1 >/dev/null
echo $rev > $S/menu/nixos-unstable
echo 9.9.9 > $S/releases/latest
r=/tmp/nami-9.9.9; mkdir -p $r
cp /new/nami $r/nami; cp /src/install.sh /src/uninstall.sh /src/packaging/setup.sh $r/
tar -C /tmp -czf $S/releases/9.9.9/nami-9.9.9-x86_64-linux.tar.gz nami-9.9.9
printf '## Updates\n- nami lives in /nami.\n' > $S/releases/9.9.9/notes.md
(cd $S/releases/9.9.9 && sha256sum nami-9.9.9-x86_64-linux.tar.gz notes.md > SHA256SUMS)
nami catalog sign $S/releases/9.9.9/SHA256SUMS --key test-1 >/dev/null
/nix/var/nami/profiles/default/bin/python3 -m http.server -d $S 8000 >/tmp/http.log 2>&1 &
http=$!
sleep 2
nami update --from http://localhost:8000/menu/nixos-unstable --trust "$key" >/dev/null 2>&1

echo "== 0.19.0 updates itself"
check "0.19.0's self-update installs this nami" 'nami self-update >/tmp/update.log 2>&1; cat /tmp/update.log; grep -q "Updated nami to 9.9.9" /tmp/update.log'
if [ "$1" = namios ]; then
  # 0.19.0 copied it into /nix/var/nami/bin and moved nothing; the daily update's next
  # step is the new nami, as root (`os-update`, started in 0.19.0's place).
  check "0.19.0 put this nami in its place; nothing moved yet" 'same /nix/var/nami/bin/nami /new/nami && [ ! -L /nix/var/nami ] && [ ! -e /nami ]'
  check "0.19.0's daemon is running" "kill -0 $old_daemon"
  /nix/var/nami/bin/nami list >/tmp/first.log 2>&1
  check "the new nami's first run as root moves it, and says so" 'grep -q "its folder is /nami now" /tmp/first.log'
else
  check "the update moved it, and said so" 'grep -q "its folder is /nami now" /tmp/update.log'
fi
moved
check "tray's old name isn't made" '[ ! -e /nix/var/tray ]'
check "the profiles and their history are as they were" 'ls /nami/profiles | sort | diff /tmp/profiles.before -'
check "root's apps" '/nami/profiles/default/bin/hello && nami list 2>/dev/null | grep -q hello'
check "bob's apps" "as bob 'jq --version'"
check "0.19.0's daemon is still running" "kill -0 $old_daemon"
check "and serves: bob installs through it" "as bob 'nami install ripgrep' >/dev/null 2>&1 && as bob 'rg --version' && [ -S /nami/daemon.sock ]"
check "running nami again moves nothing" '! nami list 2>&1 | grep -q "/nami now"'
check "an older nami (0.19.0) still works on it" '/old/nami list 2>/dev/null | grep -q hello'
if [ "$1" = namios ]; then
  check "the system in use is reached as before" '[ "$(cat /run/current-system/etc/issue)" = "namiOS 1" ] && [ "$(cat /etc/static/issue)" = "namiOS 1" ] && [ -d /nami/os/system-1 ]'
  # An older system's boot entry names /nix/var/nami/os/system-1; its initrd looks for it
  # under /newroot: the relative link reaches it there.
  mkdir -p /newroot && mount --rbind / /newroot
  check "from an initrd's /newroot, the old name reaches the system" '[ -d /newroot/nix/var/nami/os/system-1/etc ] && [ "$(readlink -f /newroot/nix/var/nami/os/system-1)" = /newroot/nami/os/system-1 ]'
  umount -R /newroot 2>/dev/null || umount -l /newroot
else
  U=/etc/systemd/system
  check "the units name /nami" "grep -q 'ExecStart=/nami/bin/nami daemon' $U/nami-daemon.service && grep -q 'ExecStart=/nami/bin/nami os-update' $U/nami-update.service"
  check "the daemon is restarted onto this nami last, once all is in place" "tail -1 /tmp/systemctl.log | grep -qx 'restart nami-daemon.service'"
  check "PATH names /nami" "grep -q '/nami/bin:' /etc/profile.d/nami.sh && [ \"\$(readlink /usr/local/bin/nami)\" = /nami/bin/nami ]"
  check "the uninstaller is in /nami" '[ -x /nami/uninstall.sh ]'
  check "the daily update runs with this nami" 'nami os-update >/tmp/os-update.log 2>&1 || { cat /tmp/os-update.log; false; }'
fi

echo "== this nami's daemon"
kill "$old_daemon"; wait "$old_daemon" 2>/dev/null
nami daemon 2>/tmp/daemon2.log &
daemon=$!
sleep 1
check "listens in /nami, and serves" "[ -S /nami/daemon.sock ] && as bob 'nami remove ripgrep' >/dev/null 2>&1 && ! as bob 'command -v rg'"
kill "$daemon"; wait "$daemon" 2>/dev/null
kill "$http" 2>/dev/null
if [ "$1" = update ]; then
  echo "== uninstalling"
  sh /nami/uninstall.sh >/tmp/uninstall.log 2>&1 || cat /tmp/uninstall.log
  gone
fi
done_
