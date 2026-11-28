#!/bin/bash
# Runs inside a privileged Ubuntu container; see tests/self-update.sh.
set -u
fails=0
ok() { echo "  ok   $1"; }
bad() { echo "  FAIL $1"; fails=$((fails + 1)); }
check() { if eval "$2" >/tmp/out 2>&1; then ok "$1"; else bad "$1"; sed 's/^/       /' /tmp/out | tail -8; fi; }

# systemd's place, and a systemctl that only says what it was asked (no systemd runs here).
mkdir -p /etc/systemd/system
printf '#!/bin/sh\necho "$@" >> /tmp/systemctl.log\n' > /usr/local/sbin/systemctl && chmod +x /usr/local/sbin/systemctl
U=/etc/systemd/system
/src/install.sh /new/nami >/dev/null
nami update --from /i.xz --unsigned >/dev/null 2>&1
nami daemon 2>/tmp/daemon.log &
sleep 1
nami install python3 >/dev/null 2>&1   # a web server for the release site

# The release site, laid out as packaging/publish.sh does, signed with a key of our own.
key=$(nami keygen test-1 2>/dev/null)
S=/site rev=0123456789ab
mkdir -p $S/menu/$rev $S/releases/9.9.9
cp /i.xz $S/menu/$rev/index-x86_64-linux.json.xz
nami catalog sign $S/menu/$rev/index-x86_64-linux.json.xz --key test-1 >/dev/null
echo $rev > $S/menu/nixos-unstable
echo 9.9.9 > $S/releases/latest
r=/tmp/nami-9.9.9; mkdir -p $r
cp /new/nami $r/nami; cp /src/install.sh /src/uninstall.sh /src/packaging/setup.sh $r/
echo 'touch /updated-to-9.9.9' >> $r/setup.sh   # so we can tell it ran
tar -C /tmp -czf $S/releases/9.9.9/nami-9.9.9-x86_64-linux.tar.gz nami-9.9.9
printf '## Updates\n- Everything in one place.\n' > $S/releases/9.9.9/notes.md
(cd $S/releases/9.9.9 && sha256sum nami-9.9.9-x86_64-linux.tar.gz notes.md > SHA256SUMS)
nami catalog sign $S/releases/9.9.9/SHA256SUMS --key test-1 >/dev/null
(cd $S && /nix/var/nami/profiles/default/bin/python3 -m http.server 8000 >/tmp/http.log 2>&1 &)
sleep 2
nami update --from http://localhost:8000/menu/nixos-unstable --trust "$key" 2>&1 | tail -2
useradd -m -s /bin/bash -G sudo bob

echo "== updating by itself"
check "install.sh set up the daily update" "grep -q 'ExecStart=/nami/bin/nami os-update' $U/nami-update.service && grep -q OnCalendar=daily $U/nami-update.timer && [ \"\$(readlink $U/timers.target.wants/nami-update.timer)\" = $U/nami-update.timer ]"
mkdir -p /etc/nami && printf 'packages: []\nupdates: manual\n' > /etc/nami/system.nami
check "updates: manual takes it away" "nami apply /etc/nami/system.nami && [ ! -e $U/nami-update.timer ] && [ ! -L $U/timers.target.wants/nami-update.timer ]"
check "nami check knows the key" "nami check /etc/nami/system.nami"
printf 'packages: []\nupdates: sometimes\n' > /etc/nami/system.nami
check "and says what it takes" "! nami check /etc/nami/system.nami && nami check /etc/nami/system.nami 2>&1 | grep -q 'updates: auto or manual'"
printf 'packages: []\n' > /etc/nami/system.nami
check "without it, it's back" "nami apply /etc/nami/system.nami && [ -e $U/nami-update.timer ] && [ -L $U/timers.target.wants/nami-update.timer ]"
# A computer with an older nami, which had no timer: it comes with this one's install.
rm -f $U/nami-update.timer $U/nami-update.service $U/timers.target.wants/nami-update.timer

echo "== a package of its own, changed"
# A recipe in /etc/nami/packages that system.nami lists: a new version of it is built
# and switched to, by `nami install` of it and by `nami apply`, and said as an update.
P=/etc/nami/packages/tiny
recipe() { mkdir -p $1; printf 'name: %s\nversion: %s\ninstall: |\n  mkdir -p $out/bin; printf "#!/bin/sh\\necho %s %s\\n" > $out/bin/%s; chmod +x $out/bin/%s\n' "$2" "$3" "$2" "$3" "$2" "$2" > $1/package.nami; }
version_in_profile() { nami list --json | grep -o "\"$1-[0-9.]*\"" | head -1; }
recipe $P tiny 1.0
check "installed from its folder" "nami install $P && [ \"\$(version_in_profile tiny)\" = '\"tiny-1.0\"' ] && grep -q './packages/tiny' /etc/nami/system.nami"
recipe $P tiny 2.0
check "installed again, changed: updated, and said so" "nami install $P >/tmp/said 2>&1; cat /tmp/said; [ \"\$(version_in_profile tiny)\" = '\"tiny-2.0\"' ] && grep -q '✓ Updated tiny 1.0 → 2.0 · in system.nami' /tmp/said && ! grep -q 'already installed' /tmp/said"
check "as it was: already installed, by its name" "nami install $P 2>&1 | grep -q '✓ tiny is already installed · in system.nami'"
recipe $P tiny 3.0
check "nami apply builds the changed recipe too" "nami apply >/tmp/said 2>&1; cat /tmp/said; [ \"\$(version_in_profile tiny)\" = '\"tiny-3.0\"' ] && grep -q '~ tiny 2.0 → 3.0' /tmp/said"
check "and then has nothing to change" "nami apply 2>&1 | grep -q 'Nothing to change'"
nami remove $P >/dev/null 2>&1
# nami's window, from an older release (a stand-in, built here): the update brings this one's.
recipe /etc/nami/packages/nami-gui nami-gui 0.0.1
nami install /etc/nami/packages/nami-gui >/dev/null 2>&1
check "an older nami-gui is installed" "[ \"\$(version_in_profile nami-gui)\" = '\"nami-gui-0.0.1\"' ]"

echo "== finding out"
check "a newer nami is out" "nami self-update --check --json | grep -q '\"latest\":\"9.9.9\",\"newer\":true'"
check "anyone can ask" "su - bob -c 'nami self-update --check' | grep -q 9.9.9"
check "with what's new in it, and its size" "nami self-update --check --json | grep -q '\"notes\":\"## Updates\\\\n- Everything in one place.\",\"size\":[0-9]'"
cp $S/releases/9.9.9/notes.md /tmp/notes.md && echo '- Something else.' >> $S/releases/9.9.9/notes.md
check "notes changed after signing aren't shown" "nami self-update --check --json | grep -q '\"notes\":null'"
cp /tmp/notes.md $S/releases/9.9.9/notes.md

echo "== refusing what can't be checked"
mv $S/releases/9.9.9/SHA256SUMS.sig /tmp/sig
check "an unsigned release is refused" "! nami self-update 2>&1 | tee /dev/stderr | grep -qE 'installed|Updated nami'; nami self-update 2>&1 | grep -q \"isn't signed\""
mv /tmp/sig $S/releases/9.9.9/SHA256SUMS.sig
cp $S/releases/9.9.9/nami-9.9.9-x86_64-linux.tar.gz /tmp/good.tgz
echo x >> $S/releases/9.9.9/nami-9.9.9-x86_64-linux.tar.gz
check "an altered one is refused" "nami self-update 2>&1 | grep -q \"doesn't match its checksum\" && [ ! -e /updated-to-9.9.9 ]"
cp /tmp/good.tgz $S/releases/9.9.9/nami-9.9.9-x86_64-linux.tar.gz

echo "== installing"
check "an admin updates nami without sudo" "su - bob -c 'cd / && nami self-update' && [ -e /updated-to-9.9.9 ]"
check "nami still works after" "nami status --json | grep -q catalog"
check "the update set up the daily update" "[ -e $U/nami-update.timer ] && [ -L $U/timers.target.wants/nami-update.timer ]"
GUI=$(sed -n 's/^version: //p' /src/examples/packages/nami-gui/package.nami)
check "the update brought nami's window as this nami has it ($GUI)" "grep -q \"^version: $GUI\$\" /etc/nami/packages/nami-gui/package.nami && [ \"\$(version_in_profile nami-gui)\" = '\"nami-gui-$GUI\"' ]"
# (Listed by its path, as installers before this one wrote it: removed by its name, for good.)
check "sudo nami remove nami-gui takes out ./packages/nami-gui" "grep -q './packages/nami-gui' /etc/nami/system.nami && nami remove nami-gui && ! grep -q nami-gui /etc/nami/system.nami && [ -z \"\$(version_in_profile nami-gui)\" ]"

echo
[ "$fails" = 0 ] && echo "all passed" || { echo "$fails failed"; exit 1; }
