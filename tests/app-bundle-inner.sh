#!/bin/sh
# Runs inside a clean container; see tests/app-bundle.sh. The bundles are in /b.
# usage: sh /run.sh cli|cli-user|gui|nixos|update|delta|delta-slow|install|projects|projects-user
set -u
fails=0
ok() { echo "  ok   $1"; }
bad() { echo "  FAIL $1"; fails=$((fails + 1)); }
check() { if eval "$2" >/tmp/out 2>&1; then ok "$1"; else bad "$1"; sed 's/^/       /' /tmp/out | tail -15; fi; }
. /etc/os-release 2>/dev/null
echo "  ($PRETTY_NAME; the distro's glibc: $(ldd --version 2>/dev/null | head -1 | sed 's/.* //'))"

cli() {
  touch /tmp/start
  check "no /nix here" "test ! -e /nix"
  check "jq runs" "echo '{\"a\":[1,2,3]}' | /b/jq-x86_64 -c '.a | add' | grep -qx 6"
  check "on the bundle's glibc, not the distro's" "LD_DEBUG=libs /b/jq-x86_64 --version 2>&1 | grep 'calling init' | grep -q '/tmp/.....//*[a-z0-9]*-glibc-2'"
  check "the distro's libc isn't loaded" "! LD_DEBUG=libs /b/jq-x86_64 --version 2>&1 | grep 'calling init' | grep -Eq ' /(usr/)?lib(64|/x86_64-linux-gnu)?/libc.so'"
  check "started again, it's already unpacked (under 50 ms)" "s=\$(date +%s%N); /b/jq-x86_64 --version >/dev/null; e=\$(date +%s%N); [ \$(( (e - s) / 1000000 )) -lt 50 ]"
  check "as a link named jq, it's jq" "ln -sf /b/jq-x86_64 /tmp/jq && /tmp/jq --version | grep -q '^jq-'"
  check "curl runs (OpenSSL, nghttp2, zstd, …)" "/b/curl-x86_64 --version | grep -q '^curl .*OpenSSL'"
  if [ -e /etc/ssl/certs/ca-certificates.crt ] || [ -e /etc/pki/tls/certs/ca-bundle.crt ] || [ -e /etc/ssl/ca-bundle.pem ]; then
    check "curl https with the distro's certificates" "/b/curl-x86_64 -fsS https://cache.nixos.org/nix-cache-info | grep -q StoreDir"
  fi
  if [ -e /b/bash-from-ubuntu-x86_64 ]; then
    check "Ubuntu's bash, bundled, runs here" "/b/bash-from-ubuntu-x86_64 -c 'echo \$BASH_VERSION' | grep -q '^5'"
  fi
  check "it says what it is" "/b/jq-x86_64 --nami-bundle-info | grep -q '^hash: '"
  check "it checks out against its hash" "/b/jq-x86_64 --nami-bundle-verify"
  # (A byte changed: X, or Y where it is an X already.)
  c=X; [ "$(dd if=/b/jq-x86_64 bs=1 skip=2000000 count=1 2>/dev/null)" = X ] && c=Y
  cp /b/jq-x86_64 /tmp/broken && printf $c | dd of=/tmp/broken bs=1 seek=2000000 conv=notrunc 2>/dev/null
  check "a damaged copy is caught" "! /tmp/broken --nami-bundle-verify"
  check "and won't run what's damaged" "rm -rf \$HOME/.cache/tray-bundle /tmp/nami-bundle-*; ! /tmp/broken --version"
  shared
  if [ -e /b/jq-thin-x86_64 ]; then
    check "a thin jq ($(du -k /b/jq-thin-x86_64 | cut -f1) KiB) fetches what it lacks, checked, and runs" "rm -rf ~/.cache/tray-bundle; echo '[3,4]' | /b/jq-thin-x86_64 -c add | grep -qx 7"
  fi
  check "nothing was put in /usr, /lib or /etc" "! find /usr /lib* /etc -newer /tmp/start -not -type d 2>/dev/null | grep -v '^/etc/hostname' | grep -q ."
}

# One store for every bundle: what two share is there once; what none
# needs any more goes.
shared() {
  S=~/.cache/tray-bundle/store
  rm -rf ~/.cache/tray-bundle
  cp /b/curl-x86_64 /tmp/curl-x86_64
  /b/jq-x86_64 --version >/dev/null && /tmp/curl-x86_64 --version >/dev/null
  check "jq and curl share one glibc" "test \$(ls $S | grep -c -- '-glibc-2') -eq 1 && test -d $S/\$(ls $S | grep -- '-glibc-2')"
  rm /tmp/curl-x86_64
  check "curl deleted, its parts go (and jq's stay)" "/b/jq-x86_64 --nami-bundle-clean | grep -q 'removed [1-9]' && ! ls $S | grep -q -- '-curl-' && ls $S | grep -q -- '-jq-' && /b/jq-x86_64 --version >/dev/null"
}

cli_user() {
  check "someone without a home: jq runs" "echo '[1,2]' | /b/jq-x86_64 -c 'length' | grep -qx 2"
  check "unpacked under /tmp, theirs" "test -O /tmp/nami-bundle-\$(id -u)"
  check "curl runs" "/b/curl-x86_64 --version >/dev/null"
}

# Started in the background: is it still up after a while, with a window?
gui_app() { # name, window-title pattern, command...
  name=$1 title=$2; shift 2
  start=$(date +%s%N)
  # In a process group of its own: an AppImage's runtime starts the app as its child.
  setsid "$@" >/tmp/$name.log 2>&1 &
  pid=$!
  for _ in $(seq 120); do
    sleep 0.25
    xwininfo -root -tree 2>/dev/null | grep -qi "$title" && break
  done
  if kill -0 $pid 2>/dev/null && xwininfo -root -tree | grep -qi "$title"; then
    ok "$name: a window, on X11, $(( ($(date +%s%N) - start) / 1000000 )) ms after starting it"
    sleep 2
    import -window root "/b/shots/$DISTRO-$name.png" 2>/dev/null && ok "$name: screenshot shots/$DISTRO-$name.png"
  else
    bad "$name: a window, on X11"; tail -15 /tmp/$name.log | sed 's/^/       /'
  fi
  kill -- -$pid 2>/dev/null; wait $pid 2>/dev/null; sleep 1
}

gui() {
  mkdir -p /b/shots
  Xvfb :99 -screen 0 1280x800x24 >/tmp/xvfb.log 2>&1 &
  export DISPLAY=:99
  for _ in $(seq 20); do xwininfo -root >/dev/null 2>&1 && break; sleep 0.3; done
  check "glxinfo: OpenGL from the distro's Mesa" "/b/glxinfo-x86_64 -B | tee /tmp/glx | grep -Eq 'OpenGL renderer string: (llvmpipe|softpipe)'"
  grep -E 'renderer|OpenGL version' /tmp/glx | sed 's/^/       /'
  check "it said so: the host's libGLX_mesa" "/b/glxinfo-x86_64 --nami-bundle-gl | grep -q 'host libGLX_mesa.so.0'"
  check "the SDL demo draws with OpenGL" "/b/sdl-demo-x86_64 90 | tee /tmp/sdl | grep -q 'drew 90 frames'"
  sed 's/^/       /' /tmp/sdl
  gui_app sdl-demo "SDL demo" /b/sdl-demo-x86_64 0
  check "the SDL demo without zenity (and GTK) draws too" "/b/sdl-demo-slim-x86_64 30 | grep -q 'drew 30 frames'"
  gui_app gnome-text-editor "Text Editor\|Untitled\|New Document" env GSK_DEBUG=renderer /b/gnome-text-editor-x86_64
  grep -i "renderer" /tmp/gnome-text-editor.log | head -2 | sed 's/^/       /'
  # Its parts are there now: the slim one (a subset) starts at once.
  check "the slim gnome-text-editor's first start finds all it needs there (under 1 s)" "s=\$(date +%s%N); /b/gnome-text-editor-slim-x86_64 --nami-bundle-extract >/dev/null; [ \$(( (\$(date +%s%N) - s) / 1000000 )) -lt 1000 ]"
  gui_app gnome-text-editor-slim "Text Editor\|Untitled\|New Document" env GSK_DEBUG=renderer /b/gnome-text-editor-slim-x86_64
  if [ -e /b/gnome-text-editor-x86_64.AppImage ]; then
    gui_app gnome-text-editor-appimage "Text Editor\|Untitled\|New Document" env APPIMAGE_EXTRACT_AND_RUN=1 /b/gnome-text-editor-x86_64.AppImage
  fi
  # The SDL demo (GTK 4 too) and gnome-text-editor have run: one copy of
  # GTK 4, and the disk used once.
  S=~/.cache/tray-bundle/store
  check "two bundles with GTK 4 keep one copy of it" "test \$(ls $S | grep -c -- '-gtk4-[0-9]') -eq 1"
  du -sm $S | sed "s/^/       the shared store: /; s/\t.*/ MiB, for gnome-text-editor (964 MiB unpacked), its slim one (732), the SDL demos (955 and 329)/"
  check "and together less than gnome-text-editor and the SDL demo each alone (1919 MiB)" "test \$(du -sm $S | cut -f1) -lt 1500"
  # A Qt 6 app.
  gui_app featherpad "FeatherPad" /b/featherpad-x86_64
  check "it puts itself in the app menu" "/b/gnome-text-editor-x86_64 --nami-bundle-install && grep -q '^Exec=/b/gnome-text-editor-x86_64' ~/.local/share/applications/nami-bundle-org.gnome.TextEditor.desktop && ls ~/.local/share/icons/hicolor/*/apps/nami-bundle-org.gnome.TextEditor.*"
  check "and takes itself out" "/b/gnome-text-editor-x86_64 --nami-bundle-uninstall && ! test -e ~/.local/share/applications/nami-bundle-org.gnome.TextEditor.desktop"
  # The entry namiOS's install makes has the same name (src/bundle_menu.rs): it's left be.
  check "an entry namiOS made for it is left be" "e=~/.local/share/applications/nami-bundle-org.gnome.TextEditor.desktop; printf '[Desktop Entry]\nExec=/x/apps/gte/gte\nX-Tray-App=gte\n' > \$e && /b/gnome-text-editor-x86_64 --nami-bundle-install | grep -q 'installed already' && ! /b/gnome-text-editor-x86_64 --nami-bundle-uninstall 2>/dev/null && grep -q X-Tray-App=gte \$e && rm \$e"
  if command -v weston >/dev/null; then
    export XDG_RUNTIME_DIR=/tmp/xdg && mkdir -p $XDG_RUNTIME_DIR && chmod 700 $XDG_RUNTIME_DIR
    for backend in headless headless-backend.so; do
      weston --backend=$backend --socket=wayland-9 --idle-time=0 >/tmp/weston.log 2>&1 &
      for _ in $(seq 20); do [ -S $XDG_RUNTIME_DIR/wayland-9 ] && break; sleep 0.3; done
      [ -S $XDG_RUNTIME_DIR/wayland-9 ] && break
    done
    WAYLAND_DISPLAY=wayland-9 GDK_BACKEND=wayland WAYLAND_DEBUG=client DISPLAY= /b/gnome-text-editor-x86_64 >/tmp/wl.log 2>&1 &
    pid=$!
    sleep 8
    check "gnome-text-editor on Wayland: up, with a window" "kill -0 $pid && grep -q 'xdg_toplevel.*set_title' /tmp/wl.log && grep -q 'wl_surface.*attach' /tmp/wl.log"
    grep -o 'xdg_toplevel[^ ]*set_title("[^"]*")' /tmp/wl.log | head -1 | sed 's/^/       /'
    kill $pid 2>/dev/null
  fi
}

# A bundle that updates itself (made with --update-url): version 1.0, and
# 2.0 published to a server here (busybox's httpd, bundled, on 127.0.0.1).
update() {
  U=/b/update A=/tmp/app/update-demo-x86_64 STAGED=/tmp/app/.update-demo-x86_64.nami-update
  mkdir -p /tmp/srv /tmp/app
  serve() { # serve <published folder>: as the feed
    [ -f /tmp/httpd.pid ] && kill "$(cat /tmp/httpd.pid)" 2>/dev/null && sleep 0.3
    rm -rf /tmp/srv/feed && cp -r "$1" /tmp/srv/feed
    /b/busybox-x86_64 httpd -f -vv -p 127.0.0.1:8471 -h /tmp/srv 2>>/tmp/httpd.log &
    echo $! >/tmp/httpd.pid
    sleep 0.5
  }
  asked() { wc -l </tmp/httpd.log; }
  fresh() { rm -rf /tmp/app/.update-demo-x86_64* ~/.cache/tray-bundle/updates; cp $U/v1/update-demo-x86_64 $A; }
  # Without looking for updates (the checks below say when to look).
  v() { NAMI_BUNDLE_UPDATE=0 "$@"; }
  # Byte for byte (Fedora has no cmp).
  same() { [ "$(sha256sum <"$1")" = "$(sha256sum <"$2")" ]; }
  : >/tmp/httpd.log
  fresh
  check "version 1.0 runs" "v $A | grep -q 'update-demo 1.0'"
  check "it says where it updates from, and whose key signs" "$A --nami-bundle-info | grep -q 'updates:  from http://127.0.0.1:8471/feed (signed by bundle-'"
  # Nothing serving the feed (offline): it says so, and starts as ever.
  check "offline: --nami-bundle-update says it couldn't reach the feed" "! $A --nami-bundle-update >/tmp/o 2>&1 && grep -q 'fetching http://127.0.0.1:8471/feed/update.json' /tmp/o"
  sed 's/^/       /' /tmp/o
  check "offline: starting, looking each time, takes no longer (under 100 ms)" "$A >/dev/null; s=\$(date +%s%N); NAMI_BUNDLE_UPDATE_INTERVAL=0 $A >/dev/null; [ \$(( (\$(date +%s%N) - s) / 1000000 )) -lt 100 ]"
  check "offline: still 1.0, and what happened is noted" "v $A | grep -q 'update-demo 1.0' && sleep 1 && $A --nami-bundle-info | grep -q \"couldn't\""
  # Someone else's feed, and the developer's changed.
  serve $U/evil
  check "a feed signed by someone else is refused" "! $A --nami-bundle-update >/tmp/o 2>&1 && grep -q 'not by this bundle' /tmp/o"
  serve $U/feed
  sed -i 's/"serial": \([0-9]*\)/"serial": 9\1/' /tmp/srv/feed/update.json
  check "a feed changed after signing is refused" "! $A --nami-bundle-update >/tmp/o 2>&1 && grep -q \"signature doesn't match\" /tmp/o"
  # Every part on the server damaged.
  serve $U/feed
  for f in /tmp/srv/feed/*.nar.zst; do printf 'X' | dd of="$f" bs=1 seek=100 conv=notrunc 2>/dev/null; done
  check "a changed part is refused, and nothing is left half-done" "! $A --nami-bundle-update >/tmp/o 2>&1 && grep -q \"doesn't match the signed description\" /tmp/o && v $A | grep -q 'update-demo 1.0' && ! ls -a /tmp/app | grep -q nami-update"
  sed 's/^/       /' /tmp/o
  # The real thing.
  serve $U/feed
  : >/tmp/httpd.log
  check "updated: only the part that changed is downloaded" "$A --nami-bundle-update | tee /tmp/o | grep -q 'downloaded 1 of its'"
  sed 's/^/       /' /tmp/o
  check "the server was asked for the feed, the description and that part only" "test \$(grep -c 'url:.*nar.zst' /tmp/httpd.log) -eq 1 && grep -q 'url:.*update.json' /tmp/httpd.log && grep -q 'url:.*meta.json' /tmp/httpd.log && ! grep -q runtime- /tmp/httpd.log"
  grep -o 'url:.*' /tmp/httpd.log | sed 's/^/       /'
  check "it runs 2.0 now" "v $A | grep -q 'update-demo 2.0'"
  check "byte for byte the developer's 2.0, and whole" "same $A $U/v2/update-demo-x86_64 && $A --nami-bundle-verify"
  check "2.0 is the newest" "$A --nami-bundle-update | grep -q 'is the newest'"
  # 1.0's own feed, signed and all, offered to 2.0: not taken back.
  serve $U/feed-v1
  check "an older version offered isn't taken" "$A --nami-bundle-update | grep -q 'is the newest' && v $A | grep -q 'update-demo 2.0'"
  # As people get it: started, it looks in the background; the next start is the new one.
  serve $U/feed
  fresh
  check "in the background: 1.0 starts at once" "$A | grep -q 'update-demo 1.0'"
  check "and gets 2.0 ready beside itself" "for i in \$(seq 60); do [ -e $STAGED ] && break; sleep 0.25; done; [ -e $STAGED ]"
  check "the next start is 2.0" "$A | grep -q 'update-demo 2.0' && ! [ -e $STAGED ] && same $A $U/v2/update-demo-x86_64"
  check "and it doesn't look again the same day" "n=\$(asked); $A >/dev/null; sleep 2; [ \$(asked) -eq \$n ]"
  # Turned off: for a run, and until turned on.
  fresh
  check "NAMI_BUNDLE_UPDATE=0: it doesn't look" "n=\$(asked); NAMI_BUNDLE_UPDATE=0 $A >/dev/null; sleep 2; [ \$(asked) -eq \$n ] && ! [ -e $STAGED ]"
  check "--nami-bundle-updates off: it doesn't look" "$A --nami-bundle-updates off >/dev/null && n=\$(asked) && $A >/dev/null && sleep 2 && [ \$(asked) -eq \$n ] && ! [ -e $STAGED ] && $A --nami-bundle-info | grep -q 'turned off'"
  check "--nami-bundle-updates on: it does again" "$A --nami-bundle-updates on >/dev/null && $A >/dev/null && for i in \$(seq 60); do [ -e $STAGED ] && break; sleep 0.25; done; [ -e $STAGED ]"
  # Where it can't replace itself (/b is read-only): it says so, and runs.
  R=$U/v1/update-demo-x86_64
  check "read-only: --nami-bundle-update says it can't, and how to get it" "$R --nami-bundle-update | tee /tmp/o | grep -q \"can't update itself: $U/v1 is on a read-only file system. Download it from http://127.0.0.1:8471/feed/update-demo-x86_64\""
  sed 's/^/       /' /tmp/o
  rm -rf ~/.cache/tray-bundle/updates
  check "read-only: started, it says so once, at the next start" "$R >/dev/null 2>&1; sleep 2; $R 2>&1 >/dev/null | grep -q \"can't update itself\" && ! $R 2>&1 >/dev/null | grep -q \"can't update itself\" && v $R | grep -q 'update-demo 1.0'"
  kill "$(cat /tmp/httpd.pid)" 2>/dev/null
}

# A computer with its own /nix/store (NixOS, namiOS): what it has, the same,
# is linked to, not unpacked.
nixos() {
  rm -rf ~/.cache/tray-bundle
  check "jq runs" "NAMI_BUNDLE_HOST_STORE=1 /b/jq-x86_64 --version | grep -q '^jq-'"
  S=~/.cache/tray-bundle/store
  check "its parts are the host's (links into /nix/store)" "test \$(find $S -maxdepth 1 -type l -lname '/nix/store/*' | wc -l) -ge 5"
  check "but glibc is its own (its ld.so.cache is the bundle's)" "test -d $S/\$(ls $S | grep -- '-glibc-2') -a ! -L $S/\$(ls $S | grep -- '-glibc-2')"
}

# `nami bundle .`'s bundles (tests/app-bundle.sh projects): each project's
# program, its libraries and data where it was built to find them.
projects() {
  check "no /nix here" "test ! -e /nix"
  check "Rust: rust-hello runs" "/b/rust-hello-x86_64 nami | grep -qx 'rust-hello 0.3.1: hello, nami'"
  check "it knows its name and version" "/b/rust-hello-x86_64 --nami-bundle-info | head -1 | grep -qx 'rust-hello 0.3.1'"
  # A Rust program that opens libX11 at run time, as winit does: the distro has none.
  check "Rust: rust-x11 opens libX11, which it doesn't link (from the bundle: the distro has none)" "! ls /usr/lib*/libX11.so.6 /usr/lib/x86_64-linux-gnu/libX11.so.6 2>/dev/null | grep -q . && /b/rust-x11-x86_64 | grep -qx 'rust-x11: libX11 opened'"
  check "Go: go-hello runs (cgo, so glibc)" "/b/go-hello-x86_64 | grep -q '^go-hello: hello, '"
  check "CMake: cmake-hello finds its data folder and its own library" "/b/cmake-hello-x86_64 | grep -q '^cmake-hello 1.4.0: hello from the data folder, from libgreet, zlib 1\.'"
  check "Meson: hello-meson runs" "/b/hello-meson-x86_64 | grep -q '^hello-meson 0.9.2: hello, zlib 1\.'"
  check "a Makefile project.nami builds: make-hello runs" "/b/make-hello-x86_64 | grep -qx 'make-hello: hello, from a Makefile'"
  check "a release tarball made on Ubuntu: prebuilt-hello runs" "/b/prebuilt-hello-x86_64 | grep -qx 'prebuilt-hello: hello, from a library beside it'"
  check "and its unpacked folder too" "/b/prebuilt-dir-x86_64 | grep -qx 'prebuilt-hello: hello, from a library beside it'"
  check "hello-meson puts itself in the app menu, with its icon" "HOME=/tmp/menu /b/hello-meson-x86_64 --nami-bundle-install && grep -q '^Exec=/b/hello-meson-x86_64' /tmp/menu/.local/share/applications/nami-bundle-hello-meson.desktop && ls /tmp/menu/.local/share/icons/hicolor/*/apps/nami-bundle-hello-meson.*"
}

# The install line (nami-install.sh, from --publish-dir) and the sandbox. As root
# in a fresh container; curl is the bundled one where the distro has none.
install() {
  U=/b/update D=$HOME/.local/bin/update-demo
  mkdir -p /tmp/srv /usr/local/bin
  serve() {
    [ -f /tmp/httpd.pid ] && kill "$(cat /tmp/httpd.pid)" 2>/dev/null && sleep 0.3
    rm -rf /tmp/srv/feed && cp -r "$1" /tmp/srv/feed
    /b/busybox-x86_64 httpd -f -p 127.0.0.1:8471 -h /tmp/srv 2>/dev/null &
    echo $! >/tmp/httpd.pid
    sleep 0.5
  }
  command -v curl >/dev/null 2>&1 || ln -sf /b/curl-x86_64 /usr/local/bin/curl
  line() { curl -fsSL http://127.0.0.1:8471/feed/nami-install.sh | sh; }
  serve $U/feed-v1
  check "the install line installs it, checked, in ~/.local/bin" "line >/tmp/o 2>&1 && grep -q 'is installed' /tmp/o && test -x $D && NAMI_BUNDLE_UPDATE=0 $D | grep -q 'update-demo 1.0'"
  sed 's/^/       /' /tmp/o
  check "it says who made it, and the key" "grep -q 'by Ada Lovelace, its updates signed with the key' /tmp/o"
  check "--nami-bundle-info: its developer, and the key trusted for it" "$D --nami-bundle-info >/tmp/i && grep -q '^developer: Ada Lovelace' /tmp/i && grep -q 'the key you trust for it' /tmp/i && grep -q '^sandbox:  Not sandboxed' /tmp/i"
  serve $U/feed
  check "the line again: the copy there updates itself" "line >/tmp/o 2>&1 && grep -q 'from 1.0 to 2.0' /tmp/o && NAMI_BUNDLE_UPDATE=0 $D | grep -q 'update-demo 2.0'"
  sed 's/^/       /' /tmp/o
  # The developer's key handed over to a new one (--rotate-key): followed, silently.
  old=$($D --nami-bundle-info | grep '^signed:')
  serve $U/feed-v3
  check "an update signed with the key the trusted one handed over to: taken" "line >/tmp/o 2>&1 && grep -q 'from 2.0 to 3.0' /tmp/o && NAMI_BUNDLE_UPDATE=0 $D | grep -q 'update-demo 3.0'"
  sed 's/^/       /' /tmp/o
  check "and the new key is the one trusted now" "$D --nami-bundle-info | grep -q 'the key you trust for it' && [ \"\$($D --nami-bundle-info | grep '^signed:')\" != \"\$old\" ]"
  # A new download signed with the new key, where the old one is trusted: taken too.
  check "a new download with the handed-over key, the old one trusted: installed" "rm -rf /tmp/h2 && serve $U/feed-v1 && HOME=/tmp/h2 sh -c 'curl -fsSL http://127.0.0.1:8471/feed/nami-install.sh | sh' >/dev/null 2>&1 && rm /tmp/h2/.local/bin/update-demo && serve $U/feed-v3 && HOME=/tmp/h2 sh -c 'curl -fsSL http://127.0.0.1:8471/feed/nami-install.sh | sh' >/tmp/o 2>&1 && HOME=/tmp/h2 /tmp/h2/.local/bin/update-demo --nami-bundle-info | grep -q 'the key you trust for it'"
  sed 's/^/       /' /tmp/o
  serve $U/feed
  # The same app signed with another key: as a new download, refused.
  rm -f $D
  serve $U/evil
  check "a download signed with another key than the one trusted is refused" "! line >/tmp/o 2>&1 && grep -q 'signed with another key' /tmp/o && ! test -e $D"
  sed 's/^/       /' /tmp/o
  check "and says how to trust the new key: with --trust, it's installed" "grep -q 'sh -s -- --trust' /tmp/o && curl -fsSL http://127.0.0.1:8471/feed/nami-install.sh | sh -s -- --trust >/dev/null 2>&1 && test -x $D && $D --nami-bundle-info | grep -q 'the key you trust for it'"
  rm -f $D
  check "a damaged download: nothing installed" "serve $U/feed && printf X | dd of=/tmp/srv/feed/update-demo-x86_64 bs=1 seek=5000 conv=notrunc 2>/dev/null && ! line >/tmp/o 2>&1 && grep -q \"doesn't match what the developer published\" /tmp/o && ! test -e $D"

  # The sandbox: first without bubblewrap.
  check "without bubblewrap: a sandboxed bundle runs, and says so" "/b/jq-sandboxed-x86_64 -n 1 2>/tmp/o | grep -qx 1 && grep -q 'runs without its sandbox' /tmp/o"
  check "once" "! /b/jq-sandboxed-x86_64 -n 1 2>&1 >/dev/null | grep -q sandbox"
  check "NAMI_BUNDLE_SANDBOX=require: refused without it" "! NAMI_BUNDLE_SANDBOX=require /b/jq-sandboxed-x86_64 -n 1 >/dev/null 2>/tmp/o && grep -q bubblewrap /tmp/o"
  case $ID in
    ubuntu|debian) (apt-get update -qq && DEBIAN_FRONTEND=noninteractive apt-get install -y -qq bubblewrap ca-certificates) >/dev/null 2>&1 ;;
    fedora) dnf install -y -q bubblewrap >/dev/null 2>&1 ;;
    arch) pacman -Sy --noconfirm bubblewrap >/dev/null 2>&1 ;;
  esac
  if ! command -v bwrap >/dev/null; then bad "installing bubblewrap"; return; fi
  echo "       ($(bwrap --version))"
  check "with bubblewrap: it says what it may use" "/b/curl-docs-x86_64 --nami-bundle-info | grep -q '^sandbox:  Runs in a sandbox: it can use your Documents folder.'"
  echo secret >~/secret1.txt; mkdir -p ~/Documents; echo doc >~/Documents/a1.txt
  check "unsandboxed, curl reads a file in the home" "/b/curl-x86_64 -s \"file://\$HOME/secret{1}.txt\" | grep -q secret"
  check "sandboxed, it can't" "! /b/curl-nonet-x86_64 -s \"file://\$HOME/secret{1}.txt\" 2>/dev/null | grep -q secret"
  check "with documents: Documents, and not the rest" "/b/curl-docs-x86_64 -s \"file://\$HOME/Documents/a{1}.txt\" | grep -q doc && ! /b/curl-docs-x86_64 -s \"file://\$HOME/secret{1}.txt\" 2>/dev/null | grep -q secret"
  check "a file it's opened with, it reads" "/b/curl-nonet-x86_64 -s file://\$HOME/secret1.txt | grep -q secret"
  check "without the network: no connection" "! /b/curl-nonet-x86_64 -fsS -m 10 https://cache.nixos.org/nix-cache-info >/dev/null 2>&1"
  check "with the network: it connects" "/b/curl-net-x86_64 -fsS -m 30 https://cache.nixos.org/nix-cache-info | grep -q StoreDir"
  check "what it writes goes in a home of its own" "cd /tmp && /b/curl-nonet-x86_64 -s file:///etc/os-release -o \$HOME/out1.txt && ! test -e \$HOME/out1.txt && test -s \$HOME/.local/share/nami-bundle/sandbox/curl/out1.txt"
}

# Difference-only updates: ripgrep 15.1.0 and FeatherPad 1.6.3, each updated
# to the next version from /b/delta (made by tests/app-bundle.sh's delta
# stage), served here. What the server sent is added up from its log.
delta() {
  mkdir -p /tmp/d
  /b/busybox-x86_64 httpd -f -vv -p 127.0.0.1:8472 -h /b/delta 2>/tmp/dhttpd.log &
  sleep 0.5
  same() { [ "$(sha256sum <"$1")" = "$(sha256sum <"$2")" ]; }
  for app in ripgrep:rg:15.2.0 featherpad:fp:1.6.4; do
    name=${app%%:*} f=${app#*:}; f=${f%%:*} v=${app##*:}
    A=/tmp/d/$name-x86_64
    # A home of its own: no other bundle's parts (busybox's glibc is 15.2.0's) to take from.
    rm -rf /tmp/home-$f /tmp/d/.$name-x86_64*; cp /b/delta/$f-v1/$name-x86_64 $A
    : >/tmp/dhttpd.log
    check "$name: updates to $v, only what changed in each part" "HOME=/tmp/home-$f NAMI_BUNDLE_DELTA=always $A --nami-bundle-update >/tmp/d/$f.out 2>&1; cat /tmp/d/$f.out; grep -q 'to $v' /tmp/d/$f.out && grep -q 'only what changed' /tmp/d/$f.out"
    check "$name: byte for byte the developer's $v" "same $A /b/delta/$f-v2/$name-x86_64 && $A --nami-bundle-verify"
    # Bytes sent, and what whole parts would have been (an older runtime's).
    sent=0 whole=0
    for u in $(sed -n 's/.*url:\/[a-z]*-feed\///p' /tmp/dhttpd.log); do
      s=$(wc -c </b/delta/$f-feed/$u)
      sent=$((sent + s))
      case $u in
        *.patch.zst) whole=$((whole + $(wc -c </b/delta/$f-feed/${u%%.from-*}.nar.zst))) ;;
        *.nar.zst|runtime-*) whole=$((whole + s)) ;;
      esac
    done
    parts=$(grep -c 'url:.*\(nar\|patch\)\.zst' /tmp/dhttpd.log)
    sed 's/^/       /' /tmp/d/$f.out
    echo "       $name: the server sent $((sent / 1024)) KiB for $parts parts, in place of $((whole / 1024)) KiB whole ($((100 - sent * 100 / whole))% less)"
    check "$name: less than three quarters of the whole parts" "[ $((sent * 4)) -lt $((whole * 3)) ]"
  done
  kill $! 2>/dev/null
}

# An update someone waits for takes the faster way: ripgrep 15.1.0 to
# 15.2.0 from busybox's httpd (fast), then from tests/lib/slow-httpd.py at
# 300 KiB/s, by itself and with NAMI_BUNDLE_DELTA=never and always.
delta_slow() {
  mkdir -p /tmp/d
  same() { [ "$(sha256sum <"$1")" = "$(sha256sum <"$2")" ]; }
  up() { # <NAMI_BUNDLE_DELTA>: a fresh 15.1.0 copy updated, and how long it took
    rm -rf /tmp/home-s /tmp/d/.ripgrep-x86_64*; cp /b/delta/rg-v1/ripgrep-x86_64 /tmp/d/
    s=$(date +%s)
    HOME=/tmp/home-s NAMI_BUNDLE_DEBUG=1 NAMI_BUNDLE_DELTA=$1 /tmp/d/ripgrep-x86_64 --nami-bundle-update >/tmp/d/s.out 2>&1 || return 1
    echo "$(( $(date +%s) - s )) s: $(tail -1 /tmp/d/s.out) $(grep -o "Some((.*)) s (patches, whole)" /tmp/d/s.out)" >/tmp/d/took
    same /tmp/d/ripgrep-x86_64 /b/delta/rg-v2/ripgrep-x86_64
  }
  patched() { grep -q 'only what changed' /tmp/d/s.out; }
  took() { sed 's/^/       /' /tmp/d/took; }
  /b/busybox-x86_64 httpd -f -p 127.0.0.1:8472 -h /b/delta & h=$!; sleep 0.5
  check "a fast link, someone waiting: whole parts" "up '' && ! patched"; took
  kill $h; sleep 0.3
  /b/python3-x86_64 /slow.py /b/delta 8472 307200 & h=$!; sleep 2
  check "a slow link, someone waiting: the patches" "up '' && patched"; took
  check "a slow link, NAMI_BUNDLE_DELTA=never: whole parts" "up never && ! patched"; took
  check "a slow link, NAMI_BUNDLE_DELTA=always: the patches" "up always && patched"; took
  kill $h
}

projects_user() {
  check "someone else, without a home: the Rust, CMake and tarball bundles run" "/b/rust-hello-x86_64 | grep -q 'hello, world' && /b/cmake-hello-x86_64 | grep -q 'data folder' && /b/prebuilt-hello-x86_64 | grep -q 'hello'"
}

case $1 in
  projects) projects ;;
  projects-user) projects_user ;;
  nixos) nixos ;;
  update) update ;;
  delta) delta ;;
  delta-slow) delta_slow ;;
  install) install ;;
  cli) cli ;;
  cli-user) cli_user ;;
  gui) gui ;;
esac
[ "$fails" -eq 0 ] && echo "  PASS" || { echo "  $fails FAILED"; exit 1; }
