#!/bin/sh
# Runs inside a clean container; see tests/bundle-rename.sh. The bundles are
# in /b: t/ made by tray before the rename (its runtime knows only tray's
# names), c/ made by this nami.
# usage: sh /run.sh run|host|oldhost
set -u
fails=0
ok() { echo "  ok   $1"; }
bad() { echo "  FAIL $1"; fails=$((fails + 1)); }
check() { if eval "$2" >/tmp/out 2>&1; then ok "$1"; else bad "$1"; sed 's/^/       /' /tmp/out | tail -15; fi; }
. /etc/os-release 2>/dev/null
echo "  ($PRETTY_NAME)"
mkdir -p "$HOME"
C=$HOME/.cache/tray-bundle
parts() { ls "${1:-$C}/store" 2>/dev/null | grep -c "$2"; }
# The store's parts, and when each was made: unchanged means nothing was unpacked again.
snapshot() { ls --full-time "$C/store" | grep -v '^total' | sort; }
# (Fedora has no diff, nor cmp.)
same() { [ "$(sha256sum <"$1")" = "$(sha256sum <"$2")" ]; }
serve() { # serve <name> <published folder>: at http://127.0.0.1:8473/<name>
  [ -f /tmp/httpd.pid ] && kill "$(cat /tmp/httpd.pid)" 2>/dev/null && sleep 0.3
  mkdir -p /tmp/srv && rm -rf "/tmp/srv/$1" && cp -r "$2" "/tmp/srv/$1"
  /b/busybox-x86_64 httpd -f -p 127.0.0.1:8473 -h /tmp/srv 2>>/tmp/httpd.log &
  echo $! >/tmp/httpd.pid
  sleep 0.5
}
info() { "$1" "--$2-bundle-info" </dev/null 2>&1; }

run() {
  echo "-- one store for old and new bundles"
  check "tray's ripgrep runs" "/b/t/rg-chain --version | grep -q '^ripgrep 15.1.0'"
  check "nami's, of the same parts, runs" "/b/c/rg-chain-2 --version | grep -q '^ripgrep 15.1.0'"
  check "both in ~/.cache/tray-bundle, none in a nami-bundle" "test -d $C/store && test ! -e $HOME/.cache/nami-bundle"
  check "their parts there once, both recorded" "[ \$(parts $C ripgrep-15.1.0) = 1 ] && [ \$(ls $C/bundles | wc -l) = 2 ]"
  check "a new bundle answers to tray's words (--tray-bundle-info)" "info /b/c/rg-chain-2 tray | head -1 | grep -q '^ripgrep 15.1.0' && info /b/c/rg-chain-2 tray | grep -q '^made by:  nami'"
  check "an old one, to its own" "info /b/t/rg-chain tray | grep -q '^made by:  tray'"
  check "TRAY_BUNDLE_CACHE is heeded by a new bundle" "TRAY_BUNDLE_CACHE=/tmp/tc /b/c/rg-chain-2 --version >/dev/null && [ \$(parts /tmp/tc ripgrep-15.1.0) = 1 ]"
  check "TRAY_BUNDLE_DEBUG too" "TRAY_BUNDLE_CACHE=/tmp/tc TRAY_BUNDLE_DEBUG=1 /b/c/rg-chain-2 --version 2>&1 | grep -qE '^(nami|tray)-bundle: starting'"
  check "its program is told where it is as tray told it: TRAY_BUNDLE (and NAMI_BUNDLE)" "/b/busybox-x86_64 env | grep -qx TRAY_BUNDLE=/b/busybox-x86_64 && /b/busybox-x86_64 env | grep -qx NAMI_BUNDLE=/b/busybox-x86_64"
  echo "-- with XDG_CACHE_HOME"
  check "old then new: one store, \$XDG_CACHE_HOME/tray-bundle" "XDG_CACHE_HOME=/tmp/x1 /b/t/rg-chain --version >/dev/null && XDG_CACHE_HOME=/tmp/x1 /b/c/rg-chain-2 --version >/dev/null && test ! -e /tmp/x1/nami-bundle && [ \$(parts /tmp/x1/tray-bundle ripgrep-15.1.0) = 1 ]"
  # nami 0.18.6 to 0.18.8 unpacked into $XDG_CACHE_HOME/nami-bundle: as one of theirs left it.
  NAMI_BUNDLE_CACHE=/tmp/x2/nami-bundle /b/c/rg-chain-2 --version >/dev/null
  check "a store 0.18.6-0.18.8 made in \$XDG_CACHE_HOME/nami-bundle is used, not started again" "XDG_CACHE_HOME=/tmp/x2 /b/c/rg-chain-3 --version | grep -q 15.2.0 && test ! -e /tmp/x2/tray-bundle && [ \$(parts /tmp/x2/nami-bundle ripgrep-15) = 2 ]"

  echo "-- cleaning up takes only what no bundle uses, whichever runtime cleans"
  mkdir -p ~/apps && cp /b/t/rg-chain ~/apps/rg-old && cp /b/c/rg-chain-3 ~/apps/rg-new && cp /b/t/htop ~/apps/htop-old && cp /b/c/htop ~/apps/htop-new
  for a in rg-old rg-new htop-old htop-new; do ~/apps/$a --version >/dev/null 2>&1; done
  check "four bundles, old and new, in one store" "[ \$(parts $C ripgrep-15) = 2 ] && [ \$(parts $C htop) -ge 1 ]"
  rm -f ~/apps/htop-new
  check "the new runtime's clean leaves the old htop's parts (it's still there)" "~/apps/rg-new --nami-bundle-clean && [ \$(parts $C htop) -ge 1 ] && ~/apps/htop-old --version | grep -q htop"
  rm -f ~/apps/htop-old
  check "and takes htop's once neither is there; the old ripgrep's stay" "~/apps/rg-new --nami-bundle-clean && [ \$(parts $C htop) = 0 ] && [ \$(parts $C ripgrep-15.1.0) = 1 ]"
  snapshot >/tmp/before
  check "the old ripgrep starts, nothing unpacked again" "~/apps/rg-old --version | grep -q 15.1.0 && snapshot >/tmp/after && same /tmp/after /tmp/before"
  rm -f ~/apps/rg-new
  check "the old runtime's clean (--tray-bundle-clean) takes 15.2.0's, keeps 15.1.0's" "~/apps/rg-old --tray-bundle-clean && [ \$(parts $C ripgrep-15.2.0) = 0 ] && [ \$(parts $C ripgrep-15.1.0) = 1 ] && ~/apps/rg-old --version | grep -q 15.1.0"
  # Both folders with XDG_CACHE_HOME: tray's (old bundles), and 0.18.6-0.18.8's, where a bundle that's gone left its parts.
  cp /b/c/rg-chain-3 /tmp/gone && NAMI_BUNDLE_CACHE=/tmp/x1/nami-bundle /tmp/gone --version >/dev/null && rm /tmp/gone
  check "the new runtime's clean cleans the other folder too, and only what's unused" "XDG_CACHE_HOME=/tmp/x1 /b/c/rg-chain-2 --nami-bundle-clean && [ \$(parts /tmp/x1/nami-bundle ripgrep-15.2.0) = 0 ] && [ \$(parts /tmp/x1/tray-bundle ripgrep-15.1.0) = 1 ]"

  echo "-- updates: old to new (whole parts), then new to newer (patches)"
  mkdir -p ~/bin
  cp /b/t/rg-chain ~/bin/rg
  ~/bin/rg --version >/dev/null
  check "the old one turns its updates off (~/.config/tray-bundle)" "~/bin/rg --tray-bundle-updates off && ls ~/.config/tray-bundle/updates-off | grep -q ."
  serve chain /b/chain@2
  snapshot >/tmp/before
  check "the old runtime updates itself to the one nami made (--tray-bundle-update)" "~/bin/rg --tray-bundle-update | tee /tmp/o | grep -q 'updated' && info ~/bin/rg nami | grep -q '^made by:  nami'"
  sed 's/^/       /' /tmp/o
  check "which runs, on the parts the old one unpacked (none again)" "~/bin/rg --version | grep -q 15.1.0 && snapshot >/tmp/after && same /tmp/after /tmp/before"
  check "updates still off: the new runtime heeds what the old one set" "info ~/bin/rg nami | grep -q 'turned off'"
  check "on again: both marks gone" "~/bin/rg --nami-bundle-updates on && [ -z \"\$(find ~/.config/tray-bundle/updates-off ~/.config/nami-bundle/updates-off -type f 2>/dev/null)\" ]"
  serve chain /b/chain
  # (Someone waiting on a fast link gets whole parts; a background check, patches: as it would.)
  check "then to 15.2.0 with patches, made against what the old runtime unpacked" "NAMI_BUNDLE_DELTA=always ~/bin/rg --nami-bundle-update | tee /tmp/o | grep -q 'only what changed' && ~/bin/rg --version | grep -q '^ripgrep 15.2.0' && ~/bin/rg --nami-bundle-verify"
  sed 's/^/       /' /tmp/o
  echo "-- updates: old straight to new, where the feed has patches the old one doesn't know"
  cp /b/t/rg-direct ~/bin/rgd
  serve direct /b/direct
  check "whole parts, and it runs" "~/bin/rgd --tray-bundle-update | tee /tmp/o | grep -q updated && ~/bin/rgd --version | grep -q '^ripgrep 15.2.0' && info ~/bin/rgd nami | grep -q '^made by:  nami'"
  sed 's/^/       /' /tmp/o
  echo "-- an old one's own update as it starts: got ready in the background, then started"
  cp /b/t/rg-direct ~/bin/rgb
  TRAY_BUNDLE_UPDATE_INTERVAL=0 ~/bin/rgb --version >/dev/null
  n=0; while [ ! -e ~/bin/.rgb.tray-update ] && [ $n -lt 60 ]; do sleep 0.5; n=$((n + 1)); done
  check "the old runtime got the new one ready (.rgb.tray-update)" "test -e ~/bin/.rgb.tray-update"
  check "started, it's the new one, which doesn't look again at once" "~/bin/rgb --version | grep -q '^ripgrep 15.2.0' && info ~/bin/rgb nami | grep -q '^made by:  nami' && ! ls -a ~/bin | grep -q 'rgb.*-update'"
  echo "-- updates: new to old (a release tray made after nami)"
  cp /b/c/rg-back ~/bin/rgr
  serve back /b/back
  check "the new runtime takes it" "~/bin/rgr --nami-bundle-update | tee /tmp/o | grep -q updated && ~/bin/rgr --version | grep -q '^ripgrep 15.2.0' && info ~/bin/rgr tray | grep -q '^made by:  tray'"
  sed 's/^/       /' /tmp/o

  echo "-- the install line, over a copy made before the rename"
  cmd=$(sed -n 's/^command=//p' /b/chain/nami-install.sh | tr -d "'\"")
  mkdir -p ~/.local/bin && cp /b/t/rg-chain ~/.local/bin/$cmd
  serve chain /b/chain
  check "it's asked in its own words, and updates (to 15.2.0)" "sh /b/chain/nami-install.sh </dev/null 2>&1 | tee /tmp/o | grep -q 'installed already' && ~/.local/bin/$cmd --version | grep -q '^ripgrep 15.2.0'"
  sed 's/^/       /' /tmp/o

  echo "-- the app menu: the entry an old runtime made is the app's"
  A=$HOME/.local/share/applications
  check "the old htop puts itself in the menu: tray-bundle-htop.desktop" "/b/t/htop --tray-bundle-install && test -f $A/tray-bundle-htop.desktop"
  cp /b/c/htop ~/apps/htop
  check "the new one, installed, is that entry (no second one)" "~/apps/htop --tray-bundle-install && test ! -e $A/nami-bundle-htop.desktop && grep -q \"Exec=.*$HOME/apps/htop\" $A/tray-bundle-htop.desktop"
  check "and taken out, it's gone, with its icon" "~/apps/htop --nami-bundle-uninstall && test ! -e $A/tray-bundle-htop.desktop && ! ls -R ~/.local/share/icons 2>/dev/null | grep -q bundle-htop"
  check "new, it's nami-bundle-htop.desktop" "~/apps/htop --nami-bundle-install && test -f $A/nami-bundle-htop.desktop && ~/apps/htop --nami-bundle-uninstall"

  echo "-- an AppImage nami made: its parts under .tray-bundle, and nami 0.18.6-0.18.8's .nami-bundle read too"
  rm -rf /tmp/ai && mkdir -p /tmp/ai && cd /tmp/ai
  check "unpacked, it has .tray-bundle" "/b/c/jq.AppImage --appimage-extract >/dev/null && test -f squashfs-root/.tray-bundle/meta.json"
  check "and runs" "echo '[1,2]' | APPDIR=/tmp/ai/squashfs-root squashfs-root/AppRun -c add | grep -qx 3"
  mv squashfs-root/.tray-bundle squashfs-root/.nami-bundle
  check "with .nami-bundle, it runs too" "rm -rf $C; echo '[1,2]' | APPDIR=/tmp/ai/squashfs-root squashfs-root/AppRun -c add | grep -qx 3"
  cd /
}

host() {
  echo "-- nami installs a bundle tray made: unpacked in its own words, not started"
  mkdir -p ~/Downloads && cp /b/t/htop /b/t/rg-direct ~/Downloads/
  A=$HOME/.local/share/applications
  check "installed: ready (unpacked), not started with an option it doesn't know" "nami install ~/Downloads/htop </dev/null 2>&1 | tee /tmp/o | grep -q 'is installed' && ! grep -q \"isn't unpacked yet\" /tmp/o && [ \$(parts $C htop) -ge 1 ]"
  sed 's/^/       /' /tmp/o
  check "its menu entry" "grep -q 'X-Tray-App=' $A/nami-bundle-htop.desktop"
  check "removed: its parts cleaned by its own runtime (--tray-bundle-clean)" "nami remove htop </dev/null >/tmp/o 2>&1 && [ \$(parts $C htop) = 0 ] && test ! -e $A/nami-bundle-htop.desktop"
  sed 's/^/       /' /tmp/o
  check "an old runtime's menu entry there: nami's install takes its place, one entry" "/b/t/htop --tray-bundle-install && nami install ~/Downloads/htop </dev/null >/tmp/o 2>&1 && grep -q 'X-Tray-App=' $A/tray-bundle-htop.desktop && test ! -e $A/nami-bundle-htop.desktop"
  check "removed, it's gone" "nami remove htop </dev/null >/tmp/o 2>&1 && test ! -e $A/tray-bundle-htop.desktop"
  echo "-- nami updates an installed bundle tray made"
  serve direct /b/direct
  check "installed" "nami install ~/Downloads/rg-direct </dev/null >/tmp/o 2>&1"
  check "nami file-apps --update: the old runtime asked in its words, updated to 15.2.0" "nami file-apps --update </dev/null 2>&1 | tee /tmp/o | grep -q 'Updated to' && ls ~/.local/share/nami/apps/*/rg-direct* | head -1 | xargs -I{} sh -c '{} --version' | grep -q '^ripgrep 15.2.0'"
  sed 's/^/       /' /tmp/o
}

oldhost() {
  echo "-- tray (from before the rename) installs a bundle nami made"
  mkdir -p ~/Downloads && cp /b/c/htop ~/Downloads/
  check "installed and unpacked (the new runtime knows --tray-bundle-extract)" "tray install ~/Downloads/htop </dev/null 2>&1 | tee /tmp/o | grep -q 'is installed' && ! grep -q \"isn't unpacked yet\" /tmp/o && [ \$(parts $C htop) -ge 1 ]"
  sed 's/^/       /' /tmp/o
  check "removed: its parts cleaned (--tray-bundle-clean)" "tray remove htop </dev/null >/tmp/o 2>&1 && [ \$(parts $C htop) = 0 ]"
  sed 's/^/       /' /tmp/o
}

case ${1:-} in
  run) run ;;
  host) host ;;
  oldhost) oldhost ;;
  *) echo "usage: $0 run|host|oldhost"; exit 2 ;;
esac
[ -f /tmp/httpd.pid ] && kill "$(cat /tmp/httpd.pid)" 2>/dev/null
echo "  $fails failed"
[ $fails = 0 ]
