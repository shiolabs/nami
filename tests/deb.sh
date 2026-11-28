#!/bin/sh
# Apps that are only on apt, or only a .deb, for real: `nami recipe from-deb`
# writes their recipes (an Ubuntu package by name, a .deb by URL and as a
# file), nami builds them (the .deb unpacked, its programs pointed at
# nixpkgs' libraries) and they run. Then the same as a computer's own
# packages, folders in its config repo, listed by path.
#
#   dctrl-tools   Ubuntu only (grep-dctrl); links only libc
#   jq            Ubuntu's build; libonig found by its file in the cache
#   git-delta     a .deb from GitHub releases; libgcc_s and zlib
#   sublime-text  a .deb with its app in /opt, a launcher script, a menu
#                 entry and icons; GTK loaded at runtime
#   zenity        and zenity-common (same source package, no nixpkgs one);
#                 a GTK 4 app: it opens its dialog in a Wayland compositor
#                 without a screen (cage), and grim takes a screenshot
#   distro-info   and distro-info-data, which its programs read at
#                 /usr/share/distro-info: pointed at the package's
#   trurl         links Ubuntu's libcurl-gnutls, which nixpkgs doesn't have:
#                 bundled from Ubuntu
#   curl          libcurl.so.4 with Ubuntu's symbol versions (CURL_OPENSSL_4):
#                 nixpkgs' curl has them
#   jpegoptim     libjpeg.so.8: nixpkgs' libjpeg8
#
# No root, Docker or Nix needed: nami runs as root of a user namespace with
# a scratch store mounted at /nix/store, through a copy of its loader.
# usage: tests/deb.sh   (work files: ${NAMI_DEB_DIR:-~/.cache/nami-deb-test};
# what it downloads from the binary cache is kept, what it builds goes)
set -eu
# A test: namios.dev counts its update checks and downloads apart (NAMI_TEST, site/worker.js).
export NAMI_TEST=1
cd "$(dirname "$0")/.."
W=${NAMI_DEB_DIR:-$HOME/.cache/nami-deb-test}
CHANNEL=${NAMI_CHANNEL:-https://namios.dev/menu/nixos-unstable}
KEY=${NAMI_KEY:-tray-2:uHz+KI06voKMb/xTAfd9nd4d8y008M5x5xWFy/pbpIs=}
DELTA=https://github.com/dandavison/delta/releases/download/0.18.2/git-delta_0.18.2_amd64.deb
SUBLIME=https://download.sublimetext.com/sublime-text_build-4200_amd64.deb

if [ "${1:-}" != inner ]; then
  cargo build -q
  mkdir -p "$W/root/nix/store" "$W/lib"
  for lib in $(ldd target/debug/nami | grep -o '/nix/store/[^ ]*'); do cp -Lf "$lib" "$W/lib/"; done
  exec unshare -rm sh "$0" inner
fi

PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin
R=$W/root
mount --bind "$R/nix/store" /nix/store
T=$PWD/target/debug/nami
nami() { NAMI_ROOT=$R "$W/lib/ld-linux-x86-64.so.2" --library-path "$W/lib" "$T" "$@"; }
fails=0
ok() { echo "  ok   $1"; }
bad() { echo "  FAIL $1"; fails=$((fails + 1)); }
check() { if (eval "$2") >"$W/out" 2>&1; then ok "$1"; else bad "$1"; sed 's/^/       /' "$W/out" | tail -20; fi; }

[ -f "$R/nix/var/nami/index.json" ] || nami update --from "$CHANNEL" --trust "$KEY" >/dev/null
rm -rf "${W:?}/work"
mkdir -p "$W/work"
cd "$W/work"

echo "== recipes from Debian packages"
check "from-deb dctrl-tools (Ubuntu's archive)" 'nami recipe from-deb dctrl-tools && grep -q "^name: dctrl-tools" dctrl-tools/package.nami && grep -q "url: https://archive.ubuntu.com/ubuntu/pool/.*dctrl-tools_.*_amd64.deb" dctrl-tools/package.nami && grep -q "^  sha256: [0-9a-f]\{64\}" dctrl-tools/package.nami'
check "from-deb jq: its libraries as nixpkgs packages" 'nami recipe from-deb jq && grep -q "^  - jq .*libjq.so.1" jq/package.nami && grep -q "^  - oniguruma.*libonig.so.5" jq/package.nami && ! grep -q "NOT FOUND" jq/package.nami'
check "from-deb a URL (git-delta)" 'nami recipe from-deb $DELTA && grep -q "^  - zlib .*libz.so.1" git-delta/package.nami && grep -q "^  - libgcc.lib .*libgcc_s.so.1" git-delta/package.nami'
check "from-deb a .deb file" 'curl -sfLo delta.deb $DELTA && nami recipe from-deb ./delta.deb --out delta-file && grep -q "^  path: ../delta.deb" delta-file/package.nami'
check "from-deb an app in /opt (sublime-text)" 'nami recipe from-deb $SUBLIME && grep -q "^runtime-deps:" sublime-text/package.nami && grep -q "^  - gtk3 .*libgtk-3.so.0" sublime-text/package.nami && grep -q "# App: share/applications/sublime_text.desktop, icon sublime-text" sublime-text/package.nami'
check "an unknown package is an error" '! nami recipe from-deb no-such-package-nami 2>&1 | tee /dev/stderr | grep -q wrote'
check "a recipe there already isn't overwritten" '! nami recipe from-deb jq 2>&1 | grep -q wrote'
cat jq/package.nami sublime-text/package.nami | sed 's/^/       /'

echo "== built and run"
run() { out=$(nami build "./$1" 2>"$W/$1.log") || { tail -30 "$W/$1.log"; return 1; }; echo "$out" > "$W/$1.out"; }
check "dctrl-tools builds and runs" 'run dctrl-tools && $(cat $W/dctrl-tools.out)/bin/grep-dctrl --version | grep -q "grep-dctrl (dctrl-tools) 2"'
check "jq builds and runs" 'run jq && echo "{\"a\": [1, 2]}" | $(cat $W/jq.out)/bin/jq -c ".a" | grep -qx "\[1,2\]" && $(cat $W/jq.out)/bin/jq --version'
check "git-delta builds and runs" 'run git-delta && $(cat $W/git-delta.out)/bin/delta --version | grep -qx "delta 0.18.2"'
check "the .deb file builds the same program" 'run delta-file && $(cat $W/delta-file.out)/bin/delta --version | grep -qx "delta 0.18.2"'
check "sublime-text builds and runs" 'run sublime-text && $(cat $W/sublime-text.out)/bin/subl --version | grep -q "Sublime Text Build 4200"'
S=$(cat "$W/sublime-text.out" 2>/dev/null || echo /nonexistent)
check "its /opt program is linked into bin" '[ "$(readlink $S/bin/sublime_text)" = ../opt/sublime_text/sublime_text ]'
check "its launcher points into the package" 'grep -q "exec $S/opt/sublime_text/sublime_text" $S/bin/subl'
check "its menu entry starts it by name" 'grep -qx "Exec=sublime_text %F" $S/share/applications/sublime_text.desktop && grep -qx "Icon=sublime-text" $S/share/applications/sublime_text.desktop && [ -f $S/share/icons/hicolor/128x128/apps/sublime-text.png ]'
check "its programs use nixpkgs' libraries" 'ldd $S/opt/sublime_text/sublime_text | grep -q "libX11.so.6 => /nix/store/" && ! ldd $S/opt/sublime_text/sublime_text | grep -q "not found"'
check "a rebuild is a no-op" 'nami build ./dctrl-tools 2>&1 | grep -q "already built"'

echo "== an app's own packages, built-in paths, bundled libraries"
check "from-deb zenity takes zenity-common (same source) along" 'nami recipe from-deb zenity && grep -q "^  - url: .*/zenity_.*_amd64.deb" zenity/package.nami && grep -q "^  - url: .*/zenity-common_.*_all.deb" zenity/package.nami && grep -q "^  - gtk4 " zenity/package.nami && ! grep -q "NOT FOUND" zenity/package.nami'
check "from-deb distro-info takes its data, which it reads at /usr/share/distro-info" 'nami recipe from-deb distro-info && grep -q "^  - url: .*/distro-info-data_.*_all.deb" distro-info/package.nami && grep -q "built in.*: /usr/share/distro-info" distro-info/package.nami'
check "from-deb trurl bundles Ubuntu's libcurl-gnutls (nixpkgs has none)" 'nami recipe from-deb trurl && grep -A3 "^bundle:" trurl/package.nami | grep -q "url: .*/libcurl3t64-gnutls_.*_amd64.deb" && grep -q "libcurl-gnutls.so.4: nixpkgs has no such library" trurl/package.nami && ! grep -q "NOT FOUND" trurl/package.nami'
check "from-deb curl: libcurl.so.4's CURL_OPENSSL_4 is in nixpkgs' curl" 'nami recipe from-deb curl && grep -q "^  - curl .*libcurl.so.4 (CURL_OPENSSL_4)" curl/package.nami && ! grep -q "^bundle:" curl/package.nami'
check "from-deb jpegoptim: libjpeg.so.8 is nixpkgs' libjpeg8" 'nami recipe from-deb jpegoptim && grep -q "^  - libjpeg8 .*libjpeg.so.8" jpegoptim/package.nami'
cat zenity/package.nami distro-info/package.nami trurl/package.nami | sed 's/^/       /'
check "distro-info builds, its data in share" 'run distro-info && [ -f $(cat $W/distro-info.out)/share/distro-info/ubuntu.csv ]'
D=$(cat "$W/distro-info.out" 2>/dev/null || echo /nonexistent)
# This computer's own /usr/share/distro-info hidden: the package's is used.
[ ! -d /usr/share/distro-info ] || mount -t tmpfs none /usr/share/distro-info
check "it reads its data from the package, not /usr" '$D/bin/distro-info --series=noble -r | grep -qx "24.04 LTS" && ! NIX_REDIRECTS= $D/bin/.ubuntu-distro-info-wrapped --series=noble -r'
check "its programs link libredirect; its wrappers say what moves" 'readelf -d $D/bin/.ubuntu-distro-info-wrapped | grep -q "NEEDED.*libredirect.so" && grep -q "NIX_REDIRECTS=./usr/share/distro-info=$D/share/distro-info" $D/bin/distro-info'
check "trurl builds and runs with the bundled libcurl-gnutls" 'run trurl && T=$(cat $W/trurl.out) && [ -f $T/lib/libcurl-gnutls.so.4 ] && $T/bin/trurl "https://example.com/a/b?x=1" --get "{host} {path}" | grep -qx "example.com /a/b" && ! ldd $T/bin/trurl | grep -q "not found"'
check "jpegoptim builds and runs with libjpeg8" 'run jpegoptim && $(cat $W/jpegoptim.out)/bin/jpegoptim --version | grep -q "^jpegoptim"'

echo "== a GUI app from a .deb opens a window"
# zenity, in cage (a Wayland compositor) without a screen; grim takes a
# screenshot of it.
mkdir -p gui && printf 'packages: [cage, grim]\n' > gui/project.nami
check "zenity builds" 'run zenity'
Z=$(cat "$W/zenity.out" 2>/dev/null || echo /nonexistent)
gui() {
  eval "$(nami env --project ./gui)" || return 1
  export XDG_RUNTIME_DIR=$W/run WLR_BACKENDS=headless WLR_RENDERER=pixman WLR_LIBINPUT_NO_DEVICES=1 NO_AT_BRIDGE=1
  rm -rf "$XDG_RUNTIME_DIR" "$W/shot.png" && mkdir -m 700 "$XDG_RUNTIME_DIR"
  unset WAYLAND_DISPLAY DISPLAY DBUS_SESSION_BUS_ADDRESS
  cage -- "$Z/bin/zenity" --info --text "Hello from a .deb" >"$W/zenity-gui.log" 2>&1 &
  cage=$!
  # A blank screen is a tiny PNG; the dialog makes it bigger.
  timeout 30 sh -c "until [ -S $XDG_RUNTIME_DIR/wayland-0 ] && WAYLAND_DISPLAY=wayland-0 grim $W/shot.png 2>/dev/null && [ \$(wc -c < $W/shot.png) -gt 6000 ]; do sleep 0.5; done"
  ok=$?
  kill "$cage"; wait "$cage"
  cat "$W/zenity-gui.log"
  [ "$ok" = 0 ] && ! grep -Eq "error while loading shared libraries|cannot open shared object|undefined symbol" "$W/zenity-gui.log"
}
check "zenity opens its dialog (screenshot: $W/shot.png)" gui

echo "== your own packages"
mkdir -p conf/packages
mv dctrl-tools git-delta jq conf/packages/
cat > conf/system.nami <<'EOF'
packages:
  - ./packages/dctrl-tools
  - ./packages/git-delta
EOF
P=$R/nix/var/nami/profiles/default/bin
check "nami check knows them" 'nami check conf/system.nami'
check "packages: [./packages/name] builds them" 'nami apply conf/system.nami && $P/grep-dctrl --version && $P/delta --version'
check "the lock names the package" 'grep -q "./packages/dctrl-tools" conf/nami.lock'
check "nami install ./path to your package" 'cd conf && nami install ./packages/jq && echo "[3]" | $P/jq -c ".[0]" | grep -qx 3 && grep -q "^  - ./packages/jq" system.nami'
check "nami remove ./path" 'cd conf && nami remove ./packages/git-delta && [ ! -e $P/delta ]'

# What this run built, unpacked and installed goes; what it downloaded
# from the binary cache stays, for the next run.
for e in "$R"/nix/var/nami/db/*.json; do
  if grep -q '"built":true' "$e"; then rm -rf "$R/nix/store/$(basename "$e" .json)" "$e"; fi
done
rm -rf "$R/nix/var/nami/profiles" "$R/etc/nami"

echo
[ "$fails" -eq 0 ] && echo "all passed" || { echo "$fails failed"; exit 1; }
