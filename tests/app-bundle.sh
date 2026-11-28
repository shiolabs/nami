#!/usr/bin/env sh
# `nami bundle`: programs made into one file each, then run on other Linuxes
# in clean containers, where there is no Nix, no nami and no /nix.
# (tests/bundle.sh is something else: the one-file installer of nami itself.)
#
#   make  in an Ubuntu container: bundle jq, curl, gnome-text-editor (whole,
#         with GStreamer left out, and as an AppImage), FeatherPad (Qt 6),
#         Mesa's glxinfo, an SDL + OpenGL program from a recipe
#         (tests/fixtures/bundle-sdl-demo), Ubuntu's own bash (a program
#         made for another Linux), and a program that updates itself, 1.0
#         and 2.0 published (tests/fixtures/bundle-update-demo), with
#         busybox to serve it
#   cli   jq and curl on Ubuntu 20.04 and 24.04, Debian, Fedora, Arch and
#         openSUSE, as root and as someone else: their own glibc, not the
#         distro's; once unpacked they start at once; damage is caught
#         (and every bundle shares one store: what two carry is there once;
#         a bundle's parts go with it); a thin jq fetches its parts
#   nixos a computer with its own /nix/store: its identical parts are used
#   update a bundle made with --update-url, 1.0, updates itself to 2.0 from
#         a server in the container (only the part that changed comes);
#         someone else's feed, a changed feed and changed parts are refused;
#         offline, read-only and turned off it runs as before; on Ubuntu
#         20.04 and 24.04 and Fedora
#   delta  updates that download only what changed: ripgrep 15.1.0 to
#         15.2.0 and FeatherPad 1.6.3 to 1.6.4 (from cache.nixos.org),
#         published with patches (the release before found in the publish
#         folder, and at the --update-url); each 1.x copy updates on Ubuntu
#         24.04, Fedora and Ubuntu 20.04, byte for byte, the bytes sent
#         added up against whole parts; then, for an update someone waits
#         for, whole parts on a fast link and patches on a slow one
#   install  the install line (nami-install.sh from --publish-dir) as root on
#         Ubuntu 20.04 and 24.04, Fedora and Arch: installed and checked,
#         updated by running it again, a download signed with another key
#         and a damaged one refused; sandboxed bundles without bubblewrap
#         (they run, and say so once) and with the distro's (no home, no
#         network, Documents when asked for, a home of its own)
#   gui   the GUI bundles under X11 (Xvfb) on Ubuntu 20.04, 24.04, Fedora
#         and Arch, drawn by the distro's own Mesa (OpenGL through the
#         host's driver), and gnome-text-editor on Wayland (Weston) on
#         Ubuntu 24.04 and Arch; screenshots in $OUT/shots
#   projects  `nami bundle .` with no recipe, in an Ubuntu container: the
#         small projects in tests/fixtures/bundle-projects (Rust, Go, CMake,
#         Meson, a Makefile that project.nami says how to build, a Rust
#         program that opens libX11 at run time as winit does, and a
#         release tarball made with Ubuntu's gcc, unpacked and not); then
#         each bundle on Ubuntu 20.04 and 24.04, Fedora and Arch
#   langs  `nami bundle .` for the other kinds, in an Ubuntu container:
#         Python (PyPI's idna, sqlite), Node.js (npm's ms), Electron (a
#         window), Zig, Flutter (a window) and .NET (ICU); then each with
#         `nami bundle --test` (rootless podman, from here): started, and
#         the two apps' windows open and drawn, on all its distributions
#
# usage: tests/app-bundle.sh [make|cli|nixos|update|delta|install|gui|projects|langs|all ...]
#   (default: all but langs)
#   NAMI=target/release/nami (static, ./build.sh: it carries the runtime)
#   INDEX=/nix/var/nami/index.json   OUT=target/app-bundle   NIX_VOLUME=nami-bundle-nix
#   CLI_IMAGES / GUI_IMAGES / UPDATE_IMAGES / PROJECT_IMAGES: the distros
set -u
cd "$(dirname "$0")/.."
PATH="$PWD/tests/lib:$PATH"   # docker, or rootless podman where there is none
nami=$(realpath "${NAMI:-target/release/nami}")
index=$(realpath "${INDEX:-/nix/var/nami/index.json}")
mkdir -p "${OUT:-target/app-bundle}"
out=$(realpath "${OUT:-target/app-bundle}")
stages=${*:-make cli nixos update delta install gui projects}
cli_images=${CLI_IMAGES:-"ubuntu:20.04 ubuntu:24.04 debian:12 fedora:latest archlinux:latest opensuse/tumbleweed"}
gui_images=${GUI_IMAGES:-"ubuntu:20.04 ubuntu:24.04 fedora:latest archlinux:latest"}
update_images=${UPDATE_IMAGES:-"ubuntu:24.04 fedora:latest ubuntu:20.04"}
install_images=${INSTALL_IMAGES:-"ubuntu:20.04 ubuntu:24.04 fedora:latest archlinux:latest"}
project_images=${PROJECT_IMAGES:-"ubuntu:20.04 ubuntu:24.04 fedora:latest archlinux:latest"}
# The make and projects stages' store, a volume (one of its own per run side by side).
vol=${NIX_VOLUME:-nami-bundle-nix}
fail=0
log() { printf '%s\n' "$*"; }

make_bundles() {
  log "== make (in ubuntu:24.04)"
  docker volume create $vol >/dev/null
  docker run --rm --privileged -e ONLY="${ONLY:-}" -v $vol:/nix -v "$nami":/usr/local/bin/nami:ro -v "$index":/index.json:ro \
    -v "$PWD/tests/fixtures/bundle-sdl-demo":/src/sdl-demo:ro -v "$PWD/tests/fixtures/bundle-update-demo":/src/update-demo:ro -v "$out":/out ubuntu:24.04 bash -c '
    set -eu
    nami update --from /index.json --unsigned >/dev/null
    cd /out
    # t <name> <nami bundle arguments>: ONLY=<names> makes just those.
    t() {
      case " ${ONLY:-$1} " in *" $1 "*) ;; *) return 0 ;; esac
      shift; s=$(date +%s)
      nami bundle "$@" >/dev/null 2>/tmp/log || { tail -20 /tmp/log; exit 1; }
      { grep -E "MiB| MB|left out" /tmp/log || true; }; echo "   ($(( $(date +%s) - s ))s)"
    }
    t jq jq
    t curl curl
    t gte gnome-text-editor --desktop --appimage
    # What it never loads left out; leaving out what it links to is refused.
    t gte-slim gnome-text-editor --exclude python3 --exclude freepats --exclude flite -o gnome-text-editor-slim-x86_64
    if [ -z "${ONLY:-}" ]; then
      if nami bundle gnome-text-editor --exclude gst- -o /tmp/broken >/tmp/ex 2>&1; then echo "--exclude gst- was let through"; exit 1; fi
      grep -q "needs libgst" /tmp/ex || { cat /tmp/ex; exit 1; }
      echo "--exclude of what it links to: refused"
    fi
    t glxinfo mesa-demos --program glxinfo
    rm -rf /tmp/sdl-demo && cp -r /src/sdl-demo /tmp/sdl-demo
    t sdl /tmp/sdl-demo --desktop
    # Its icon is a theme'"'"'s, by its name (Icon=applications-games): the file, from a theme it carries, where it does.
    case " ${ONLY:-sdl} " in *" sdl "*)
      if ls sdl-demo-x86_64.svg sdl-demo-x86_64.png >/dev/null 2>&1; then echo "   its icon (applications-games), from what it carries: $(grep ^Icon= sdl-demo-x86_64.desktop)"; else echo "   (no theme it carries has applications-games: Settings finds it in the computer'"'"'s)"; fi ;;
    esac
    # A Qt 6 app.
    t qt featherpad
    t sdl-slim /tmp/sdl-demo --exclude zenity --exclude gtk+3 --exclude gst- --exclude gstreamer --exclude ffmpeg -o sdl-demo-slim-x86_64
    # A program made for Ubuntu (from this container): glibc and libraries from nixpkgs instead.
    t foreign /usr/bin/bash -o bash-from-ubuntu-x86_64
    # Sandboxed (bubblewrap where the distro has it): without the network, with it, with Documents.
    t sandbox jq --sandbox -o jq-sandboxed-x86_64
    t sandbox-curl curl --sandbox -o curl-nonet-x86_64
    t sandbox-curl-net curl --sandbox network -o curl-net-x86_64
    t sandbox-curl-docs curl --sandbox documents -o curl-docs-x86_64
    t sandbox-gdbus glib --program gdbus --sandbox -o gdbus-sandboxed-x86_64
    # Thin: what cache.nixos.org has is fetched on first start.
    t jq-thin jq --thin -o jq-thin-x86_64
    t gte-thin gnome-text-editor --thin -o gnome-text-editor-thin-x86_64
    # Updating itself: version 1.0, then 2.0 published (and a server to serve it).
    case " ${ONLY:-update} " in *" update "*)
      u=/out/update; rm -rf $u /tmp/ud && mkdir -p $u/v1 $u/v2 $u/v3 && cp -r /src/update-demo /tmp/ud
      url=http://127.0.0.1:8471/feed
      ONLY= t busybox busybox -o busybox-x86_64
      ONLY= t update-1 /tmp/ud --update-url $url --publish-dir $u/feed-v1 --developer "Ada Lovelace" -o $u/v1/update-demo-x86_64
      sed -i "s/1\.0/2.0/" /tmp/ud/src/demo.c /tmp/ud/package.nami
      ONLY= t update-2 /tmp/ud --update-url $url --publish-dir $u/feed --developer "Ada Lovelace" -o $u/v2/update-demo-x86_64
      # The same, signed by someone else (a key of their own).
      XDG_CONFIG_HOME=/tmp/other ONLY= t update-other /tmp/ud --update-url $url --publish-dir $u/evil -o /tmp/evil-x86_64
      echo "   the developer: $(nami bundle --show-signing-key 2>/dev/null); someone else: $(XDG_CONFIG_HOME=/tmp/other nami bundle --show-signing-key 2>/dev/null)"
      # On CI, without the key given: refused, not a new key.
      if CI=1 XDG_CONFIG_HOME=/tmp/ci nami bundle /tmp/ud --update-url $url -o /tmp/ci-x86_64 >/tmp/ci.log 2>&1; then echo "CI made a key of its own"; exit 1; fi
      grep -q NAMI_BUNDLE_SIGNING_KEY /tmp/ci.log || { cat /tmp/ci.log; exit 1; }
      echo "   on CI without NAMI_BUNDLE_SIGNING_KEY: refused"
      # The developer hands their key over to a new one (signed by the old): 3.0 is signed with it.
      nami bundle --rotate-key >/tmp/rot.log 2>&1 || { cat /tmp/rot.log; exit 1; }
      echo "   --rotate-key: $(head -1 /tmp/rot.log | cut -d: -f1), the old one kept: $(ls /root/.config/nami)"
      sed -i "s/2\.0/3.0/" /tmp/ud/src/demo.c /tmp/ud/package.nami
      ONLY= t update-3 /tmp/ud --update-url $url --publish-dir $u/feed-v3 --developer "Ada Lovelace" -o $u/v3/update-demo-x86_64
      ls $u/feed | sed "s/^/   /" ;;
    esac
    nami bundle --verify jq-x86_64
    ls -l /out' || fail=$((fail + 1))
}

cli() {
  for image in $cli_images; do
    log "== cli on $image"
    docker run --rm -v "$out":/b:ro -v "$PWD/tests/app-bundle-inner.sh":/run.sh:ro "$image" sh /run.sh cli || fail=$((fail + 1))
    # Someone without a home of their own: the cache falls back to /tmp.
    docker run --rm -u 1234:1234 -e HOME=/ -v "$out":/b:ro -v "$PWD/tests/app-bundle-inner.sh":/run.sh:ro "$image" sh /run.sh cli-user || fail=$((fail + 1))
  done
}

# Updating itself: version 1.0 to 2.0, from a server in the container.
update() {
  for image in $update_images; do
    log "== update on $image"
    docker run --rm -v "$out":/b:ro -v "$PWD/tests/app-bundle-inner.sh":/run.sh:ro "$image" sh /run.sh update || fail=$((fail + 1))
  done
}

# The install line (nami-install.sh from --publish-dir), and the sandbox, with
# the distro's bubblewrap and without: as root, the install line's curl the
# bundled one where the distro has none.
install() {
  for image in $install_images; do
    log "== install line and sandbox on $image"
    docker run --rm --privileged -v "$out":/b:ro -v "$PWD/tests/app-bundle-inner.sh":/run.sh:ro "$image" sh /run.sh install || fail=$((fail + 1))
  done
}

# Parts a host's own /nix/store has: the make stage's store stands in for a
# NixOS one.
nixos() {
  log "== on a computer with its own /nix/store"
  docker run --rm -v $vol:/nix:ro -v "$out":/b:ro -v "$PWD/tests/app-bundle-inner.sh":/run.sh:ro ubuntu:24.04 sh /run.sh nixos || fail=$((fail + 1))
}

# A distro with a desktop's parts: Xvfb, Weston, Mesa, fonts, and tools to look.
gui_image() {
  name=nami-bundle-gui2-$(echo "$1" | tr ':/.' '---')
  docker image inspect "$name" >/dev/null 2>&1 && { echo "$name"; return; }
  case $1 in
    ubuntu:20.04) pkgs="RUN apt-get update && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends xvfb x11-utils imagemagick libgl1-mesa-dri libegl1 libglx-mesa0 libgl1 fonts-dejavu-core adwaita-icon-theme procps dbus" ;;
    ubuntu:*|debian:*) pkgs="RUN apt-get update && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends xvfb x11-utils imagemagick libgl1-mesa-dri libegl1 libglx-mesa0 libgl1 fonts-dejavu-core adwaita-icon-theme procps dbus weston" ;;
    fedora:*) pkgs="RUN dnf install -y xorg-x11-server-Xvfb xwininfo ImageMagick mesa-dri-drivers mesa-libEGL mesa-libGL dejavu-sans-fonts adwaita-icon-theme procps-ng dbus-daemon && dnf clean all" ;;
    archlinux:*) pkgs="RUN pacman -Syu --noconfirm xorg-server-xvfb xorg-xwininfo imagemagick mesa ttf-dejavu adwaita-icon-theme procps-ng dbus weston" ;;
  esac
  printf 'FROM %s\n%s\n' "$1" "$pkgs" | docker build -q -t "$name" - >/dev/null || return 1
  echo "$name"
}

gui() {
  mkdir -p "$out/shots"
  for image in $gui_images; do
    log "== gui on $image"
    img=$(gui_image "$image") || { log "  FAIL building the image"; fail=$((fail + 1)); continue; }
    docker run --rm -e DISTRO="$(echo "$image" | tr ':/' '--')" -v "$out":/b -v "$PWD/tests/app-bundle-inner.sh":/run.sh:ro "$img" dbus-run-session -- sh /run.sh gui || fail=$((fail + 1))
  done
}

# `nami bundle .`: each project built with the tools nami gives it, as a
# developer would, and what it found said; then run elsewhere.
projects() {
  log "== projects: nami bundle . (in ubuntu:24.04)"
  mkdir -p "$out/projects"
  docker run --rm -v $vol:/nix -v "$nami":/usr/local/bin/nami:ro -v "$index":/index.json:ro \
    -v "$PWD/tests/fixtures/bundle-projects":/src:ro -v "$out/projects":/out ubuntu:24.04 bash -c '
    set -u
    fails=0
    ok() { echo "  ok   $1"; }
    bad() { echo "  FAIL $1"; fails=$((fails + 1)); }
    # b <name> <folder> <nami bundle arguments>: made, and what it said kept in /out/<name>.log
    b() {
      name=$1 dir=$2; shift 2
      s=$(date +%s)
      if (cd "$dir" && nami bundle "$@") >/out/$name.out 2>/out/$name.log; then
        ok "$name: made ($(( $(date +%s) - s ))s, $(du -h /out/$name-x86_64 2>/dev/null | cut -f1))"
      else
        bad "$name: nami bundle"; tail -25 /out/$name.log | sed "s/^/       /"
      fi
    }
    said() { if grep -Eq "$2" "/out/$1.log"; then ok "$1: said $3"; else bad "$1: said $3"; grep -E "^(project|  )" "/out/$1.log" | sed "s/^/       /"; fi; }
    nami update --from /index.json --unsigned >/dev/null
    apt-get update -qq >/dev/null && DEBIAN_FRONTEND=noninteractive apt-get install -y -qq --no-install-recommends gcc libc6-dev ca-certificates git >/dev/null || bad "apt-get install gcc"
    rm -rf /tmp/p && cp -r /src /tmp/p
    b rust-hello /tmp/p/rust-hello . --desktop -o /out/rust-hello-x86_64
    said rust-hello "project: Rust" "it is Rust"
    said rust-hello "version +0\.3\.1 +\(Cargo.toml\)" "the version, from Cargo.toml"
    said rust-hello "desktop +assets/rust-hello.desktop +\(in the repo\)" "the .desktop file in the repo"
    said rust-hello "icon .*rust-hello.svg +\(Cargo.toml\)" "the icon, from [package.metadata.nami]"
    test -s /out/rust-hello-x86_64.desktop && test -s /out/rust-hello-x86_64.svg && ok "rust-hello: --desktop wrote its menu entry and icon" || bad "rust-hello: --desktop wrote its menu entry and icon"
    # What its crates open at run time (x11-dl, as winit), from the Cargo.lock its build makes.
    b rust-x11 /tmp/p/rust-x11 . -o /out/rust-x11-x86_64
    said rust-x11 "opens +libx11, .*\(Cargo.lock: x11-dl" "what it opens at run time, from Cargo.lock"
    # Go: no version anywhere, said so, then given.
    b go-hello-noversion /tmp/p/go-hello . -o /tmp/go-noversion
    said go-hello-noversion "no version found.*--app-version" "it found no version, and how to say one"
    said go-hello-noversion "no .desktop file found.*--desktop-file" "there is no .desktop file, and how to give one"
    b go-hello /tmp/p/go-hello . --app-version 2.0.1 -o /out/go-hello-x86_64
    said go-hello "name +go-hello +\(go.mod" "the name, from go.mod"
    said go-hello "version +2\.0\.1 +\(--app-version\)" "the version given"
    b cmake-hello /tmp/p/cmake-hello . -o /out/cmake-hello-x86_64
    said cmake-hello "version +1\.4\.0 +\(CMakeLists.txt" "the version, from project()"
    said cmake-hello "needs +zlib" "it needs zlib (find_package)"
    b hello-meson /tmp/p/hello-meson . --desktop -o /out/hello-meson-x86_64
    said hello-meson "version +0\.9\.2" "the version, from project()"
    said hello-meson "desktop .*installed by its build" "the .desktop file its build installs"
    test -s /out/hello-meson-x86_64.svg && ok "hello-meson: its icon, from its build" || bad "hello-meson: its icon, from its build"
    b make-hello /tmp/p/make-hello . -o /out/make-hello-x86_64
    said make-hello "version +5\.0\.0 +\(project.nami\)" "the version, from project.nami"
    # A release tarball, made with Ubuntu'"'"'s gcc: bundled as it is, and unpacked as a folder.
    sh /tmp/p/prebuilt-hello/build-on-ubuntu.sh /tmp
    b prebuilt-hello /tmp /tmp/prebuilt-hello-1.2.3-linux-x86_64.tar.gz -o /out/prebuilt-hello-x86_64
    said prebuilt-hello "version +1\.2\.3 +\(the file" "the version, from the file name"
    said prebuilt-hello "is built for other distributions" "it was built for other distributions"
    mkdir -p /tmp/unpacked && tar -xzf /tmp/prebuilt-hello-1.2.3-linux-x86_64.tar.gz -C /tmp/unpacked
    b prebuilt-dir /tmp/unpacked/prebuilt-hello-1.2.3 . -o /out/prebuilt-dir-x86_64
    said prebuilt-dir "project: programs already built" "they are programs already built"
    said prebuilt-dir "version +1\\.2\\.3 +\\(the folder.s name\\)" "the version, from the folder name"
    # A folder nami can'"'"'t tell how to build: what to write, said.
    mkdir -p /tmp/nothing && echo hi > /tmp/nothing/README
    if (cd /tmp/nothing && nami bundle .) >/dev/null 2>/tmp/nothing.log; then bad "an unknown folder is refused"
    elif grep -q "bundle:" /tmp/nothing.log && grep -q "build:" /tmp/nothing.log; then ok "an unknown folder: refused, saying how to tell it (bundle: build: in project.nami)"
    else bad "an unknown folder: refused, saying how"; cat /tmp/nothing.log; fi
    # Built again, unchanged: the same store path (it is named after what was built).
    b rust-again /tmp/p/rust-hello . -o /tmp/rust-again-x86_64
    a=$(/out/rust-hello-x86_64 --nami-bundle-info | grep -- "-rust-hello-0.3.1") c=$(/tmp/rust-again-x86_64 --nami-bundle-info | grep -- "-rust-hello-0.3.1")
    [ -n "$a" ] && [ "$a" = "$c" ] && ok "built again: the same store path ($(echo $a))" || bad "built again: the same store path ($a, $c)"
    ls -l /out | grep -- -x86_64
    [ "$fails" -eq 0 ]' || fail=$((fail + 1))
  for image in $project_images; do
    log "== projects on $image"
    docker run --rm -v "$out/projects":/b:ro -v "$PWD/tests/app-bundle-inner.sh":/run.sh:ro "$image" sh /run.sh projects || fail=$((fail + 1))
    docker run --rm -u 1234:1234 -e HOME=/tmp/home -v "$out/projects":/b:ro -v "$PWD/tests/app-bundle-inner.sh":/run.sh:ro "$image" sh /run.sh projects-user || fail=$((fail + 1))
  done
}

# Difference-only updates, with real apps from cache.nixos.org: ripgrep
# 15.1.0 then 15.2.0 (the release before found in the publish folder), and
# FeatherPad 1.6.3 then 1.6.4 (found at the --update-url, served here);
# then a 1.6.3 copy and a 15.1.0 copy update on each distribution.
delta() {
  log "== delta: two releases each, published (in ubuntu:24.04)"
  docker volume create $vol >/dev/null
  docker run --rm -v $vol:/nix -v "$nami":/usr/local/bin/nami:ro -v "$index":/index.json:ro -v "$out":/out ubuntu:24.04 bash -c '
    set -eu
    nami update --from /index.json --unsigned >/dev/null
    [ -e /out/busybox-x86_64 ] || nami bundle busybox -o /out/busybox-x86_64 >/dev/null 2>&1
    # (Python, bundled, for a slow server: tests/lib/slow-httpd.py.)
    [ -e /out/python3-x86_64 ] || nami bundle python3 -o /out/python3-x86_64 >/dev/null 2>&1
    d=/out/delta url=http://127.0.0.1:8472
    rm -rf $d && mkdir -p $d/rg-v1 $d/rg-v2 $d/fp-v1 $d/fp-v2
    b() { # <dir> <store path> <nami bundle options...>
      local o=$1 p=$2; shift 2; s=$(date +%s)
      nami bundle /nix/store/$p -o $d/$o "$@" >/dev/null 2>$d/$(dirname $o).log || { tail -20 $d/$(dirname $o).log; exit 1; }
      echo "   $o: $(du -h $d/$o | cut -f1) ($(( $(date +%s) - s ))s)"; grep -E "^(since|no patch)" $d/$(dirname $o).log | sed "s/^/     /" || true
    }
    b rg-v1/ripgrep-x86_64 jmxrpp9mj5l669s0y4f4rfdh10ln51xj-ripgrep-15.1.0 --update-url $url/rg-feed --publish-dir $d/rg-feed
    b rg-v2/ripgrep-x86_64 vli6118fdbngwy46j1w741ansmrzfszr-ripgrep-15.2.0 --update-url $url/rg-feed --publish-dir $d/rg-feed
    grep -q "^since 15.1.0: .* as patches" $d/rg-v2.log || { echo "ripgrep 15.2.0: no patches from the publish folder"; exit 1; }
    b fp-v1/featherpad-x86_64 50mla5r01hk610wvb10lgsaxcq0vg3p0-featherpad-1.6.3 --update-url $url/fp-feed --publish-dir $d/fp-feed
    # 1.6.4 into a new folder: the release before comes from the URL.
    /out/busybox-x86_64 httpd -f -p 127.0.0.1:8472 -h $d & h=$!; sleep 0.5
    b fp-v2/featherpad-x86_64 pydwqx25s83psfp5l02yys1pn7x8p9r3-featherpad-1.6.4 --update-url $url/fp-feed --publish-dir $d/fp-new
    kill $h
    grep -q "^since 1.6.3: .* as patches" $d/fp-v2.log || { echo "FeatherPad 1.6.4: no patches from $url/fp-feed"; exit 1; }
    rm -rf $d/fp-feed && mv $d/fp-new $d/fp-feed
    du -ch $d/*-feed/*.patch.zst | tail -1 | sed "s/^/   patches: /"' || fail=$((fail + 1))
  for image in $update_images; do
    log "== delta: updates on $image"
    docker run --rm -v "$out":/b:ro -v "$PWD/tests/app-bundle-inner.sh":/run.sh:ro "$image" sh /run.sh delta || fail=$((fail + 1))
  done
  # Someone waiting, on a slow link (300 KiB/s): the patches; on a fast one, whole parts.
  log "== delta: updates someone waits for, on a slow link (ubuntu:24.04)"
  docker run --rm -v "$out":/b:ro -v "$PWD/tests/app-bundle-inner.sh":/run.sh:ro -v "$PWD/tests/lib/slow-httpd.py":/slow.py:ro ubuntu:24.04 sh /run.sh delta-slow || fail=$((fail + 1))
}

# `nami bundle .` for Python, Node.js, Electron, Zig, Flutter and .NET: made
# with the catalog's tools, what was found said; then `nami bundle --test`.
langs() {
  log "== langs: nami bundle . (in ubuntu:24.04)"
  mkdir -p "$out/langs"
  docker volume create $vol >/dev/null
  docker run --rm -v $vol:/nix -v "$nami":/usr/local/bin/nami:ro -v "$index":/index.json:ro \
    -v "$PWD/tests/fixtures/bundle-projects":/src:ro -v "$out/langs":/out ubuntu:24.04 bash -c '
    set -u
    fails=0
    ok() { echo "  ok   $1"; }
    bad() { echo "  FAIL $1"; fails=$((fails + 1)); }
    b() {
      name=$1; shift
      s=$(date +%s)
      if (cd /tmp/p/$name && nami bundle . "$@" -o /out/$name-x86_64) >/out/$name.out 2>/out/$name.log; then
        ok "$name: made ($(( $(date +%s) - s ))s, $(du -h /out/$name-x86_64 | cut -f1))"
      else
        bad "$name: nami bundle"; tail -25 /out/$name.log | sed "s/^/       /"
      fi
    }
    said() { if grep -Eq "$2" "/out/$1.log"; then ok "$1: said $3"; else bad "$1: said $3"; grep -E "^(project|  )" "/out/$1.log" | sed "s/^/       /"; fi; }
    runs() { if out=$(/out/$1-x86_64 ada 2>&1) && echo "$out" | grep -Eq "$2"; then ok "$1 runs: $out"; else bad "$1 runs"; echo "$out" | tail -5 | sed "s/^/       /"; fi; }
    nami update --from /index.json --unsigned >/dev/null
    apt-get update -qq >/dev/null && DEBIAN_FRONTEND=noninteractive apt-get install -y -qq --no-install-recommends ca-certificates git >/dev/null || bad "apt-get install"
    rm -rf /tmp/p && cp -r /src /tmp/p
    b py-hello
    said py-hello "project: Python \(pyproject.toml\)" "it is Python"
    said py-hello "runs +py-hello = py_hello.cli:main +\(\[project.scripts\]\)" "its entry point"
    said py-hello "python +3\.[0-9]+ +\(requires-python >=3\.11\)" "the Python requires-python takes"
    runs py-hello "^py-hello 1\.2\.0: hello, ada \(Python 3\.[0-9]+, xn--bcher-kva\.example, sqlite 3\."
    b node-hello
    said node-hello "name +node-hello +\(package.json\)" "its name, less its npm scope"
    said node-hello "runs +node-hello = cli.js" "its bin"
    runs node-hello "^node-hello 1\.0\.0: hello, ada \(Node [0-9]+, a minute is 1m\)"
    b electron-hello
    said electron-hello "electron +43 +\(package.json: electron \^43" "the Electron it asks for"
    said electron-hello "desktop +electron-hello.desktop" "a menu entry, for its window"
    said electron-hello "icon +build/icon.svg +\(package.json\)" "electron-builder'"'"'s icon"
    b zig-hello
    said zig-hello "zig +0\.15 +\(build.zig.zon: minimum_zig_version" "the Zig build.zig.zon asks for"
    runs zig-hello "^zig-hello 0\.2\.0: hello, from Zig 0\.15"
    s=$(du -m /out/zig-hello-x86_64 | cut -f1); [ "$s" -lt 20 ] && ok "zig-hello: not the compiler along with it ($s MiB)" || bad "zig-hello: $s MiB, the compiler along with it"
    # nami run in a Zig project: zig is both a tool and an input to the build
    # environment, and its setup hook (readonly flags) is sourced once.
    printf "packages: [zig@0.15]\ndevelop: true\n" >/tmp/p/zig-hello/project.nami
    if out=$(cd /tmp/p/zig-hello && nami run -- zig version 2>&1) && echo "$out" | grep -q "^0\.15"; then ok "zig-hello: nami run -- zig version: $out"; else bad "zig-hello: nami run"; echo "$out" | tail -5 | sed "s/^/       /"; fi
    b flutter-hello
    said flutter-hello "runner +flutter_hello" "its runner"
    said flutter-hello "desktop +flutter_hello.desktop +\(made for its window" "a menu entry, for its window"
    b dotnet-hello
    said dotnet-hello "runtime +\.NET 8\.[0-9.]+ +\(TargetFramework net8\.0\)" "the .NET its TargetFramework says"
    runs dotnet-hello "^dotnet-hello 3\.0\.1: hello, ada \(\.NET 8, 1\.234,5\)"
    [ "$fails" -eq 0 ]' || fail=$((fail + 1))
  # Each on every distribution `nami bundle --test` knows, from here: its own podman, rootless.
  log "== langs: nami bundle --test"
  podman=$(command -v podman) || podman=$(nix build --no-link --print-out-paths 'nixpkgs#podman^out' 2>/dev/null)/bin/podman
  base=${XDG_CONFIG_HOME:-$HOME/.config}/nami-tests
  old=${XDG_CONFIG_HOME:-$HOME/.config}/tr''ay-tests
  [ -e "$base" ] || [ ! -d "$old" ] || base=$old
  for app in py-hello node-hello electron-hello zig-hello flutter-hello dotnet-hello; do
    [ -e "$out/langs/$app-x86_64" ] || continue
    if env PATH="$(dirname "$podman"):/run/wrappers/bin:$PATH" HOME="$base/home" XDG_CONFIG_HOME="$base/home/.config" XDG_DATA_HOME="$base/home/.local/share" \
        "$nami" bundle --test "$out/langs/$app-x86_64" --test-args ada >"$out/langs/$app.test" 2>&1; then
      log "  ok   $app: nami bundle --test"
    else
      log "  FAIL $app: nami bundle --test"; fail=$((fail + 1))
    fi
    sed 's/^/       /' "$out/langs/$app.test" | grep -v "making its image\|: done$"
  done
}

for s in $stages; do
  case $s in
    make) make_bundles ;;
    cli) cli ;;
    nixos) nixos ;;
    update) update ;;
    delta) delta ;;
    install) install ;;
    gui) gui ;;
    projects) projects ;;
    langs) langs ;;
    all) make_bundles; cli; nixos; update; delta; install; gui; projects; langs ;;
    *) echo "unknown stage $s"; exit 2 ;;
  esac
done
[ "$fail" -eq 0 ] && echo "PASS" || { echo "$fail FAILED"; exit 1; }
