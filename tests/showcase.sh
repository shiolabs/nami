#!/usr/bin/env sh
# The showcase (docs/guide/ship-your-app.md, "Real apps, bundled"): real open-source apps whose Linux
# downloads are a source tarball, one distro's .deb or a Flatpak, bundled
# with `nami bundle`, then run on six distributions in clean containers.
# What it prints is the page's table; the screenshots are in $OUT/shots.
#
#   make   in an Ubuntu 24.04 container: from the catalog Open Surge (a game,
#          Allegro), Strawberry (Qt 6), Rnote and Amberol (GTK 4), Element
#          (Electron) and
#          aria2 (a command); from upstream's own source, with `nami bundle .`,
#          simp (Rust, egui and wgpu) and tmux (autotools: project.nami says
#          how, tests/fixtures/showcase). Each also --thin.
#   run    each bundle on Ubuntu 20.04 and 24.04, Debian 12, Fedora, Arch,
#          openSUSE Tumbleweed and Linux Mint 22.3 (Mint's own image, linuxmintd:
#          Ubuntu 24.04 with Mint's packages): started with an empty cache (the first start,
#          timed) under X11 (Xvfb), a window and a screenshot; sound into a
#          null sink (PulseAudio's); on Wayland (Weston) where the distro has
#          it; the command-line ones doing their job
#
# usage: tests/showcase.sh [make|run ...]   (default: make run)
#   NAMI=target/release/nami (static, ./build.sh)   INDEX=/nix/var/nami/index.json
#   OUT=~/.cache/nami-showcase (bundles, sources, screenshots)
#   IMAGES: the distros   APPS: which apps (default all)
set -u
cd "$(dirname "$0")/.."
PATH="$PWD/tests/lib:$PATH"
nami=$(realpath "${NAMI:-target/release/nami}")
index=$(realpath "${INDEX:-/nix/var/nami/index.json}")
out=${OUT:-$HOME/.cache/nami-showcase}
mkdir -p "$out/src" "$out/b" "$out/shots" "$out/logs"
out=$(realpath "$out")
stages=${*:-make run}
images=${IMAGES:-"ubuntu:20.04 ubuntu:24.04 debian:12 fedora:latest archlinux:latest opensuse/tumbleweed linuxmintd/mint22.3-amd64"}
apps=${APPS:-"opensurge simp strawberry rnote amberol element tmux aria2"}
fail=0
log() { printf '%s\n' "$*"; }

# Upstream's sources, each in a folder of its own, checked.
fetch() { # name url sha256
  d="$out/src/$1" f="$out/src/$1/$(basename "$2")"
  mkdir -p "$d"
  [ -s "$f" ] || curl -sSfL -o "$f" "$2" || return 1
  echo "$3  $f" | sha256sum -c --quiet || { rm -f "$f"; return 1; }
  rm -rf "$d/tree" && mkdir -p "$d/tree" && tar -xf "$f" -C "$d/tree" --strip-components=1
  [ ! -d "tests/fixtures/showcase/$1" ] || cp "tests/fixtures/showcase/$1/"* "$d/tree/"
}

make_bundles() {
  log "== make (in ubuntu:24.04)"
  fetch simp https://codeload.github.com/Kl4rry/simp/tar.gz/refs/tags/v3.11.0 79f93854d6ca23a0b67d8f0067d48331ea5f4a9da101e9769a73866d045406d9 || { log "  FAIL fetching simp"; fail=$((fail + 1)); }
  fetch tmux https://github.com/tmux/tmux/releases/download/3.8/tmux-3.8.tar.gz e79c699c7e949dccd0a4a125e17b8d1261e311b16979dbc8eb34542f3966d82e || { log "  FAIL fetching tmux"; fail=$((fail + 1)); }
  docker volume create nami-showcase-nix >/dev/null
  docker run --rm --privileged -e APPS="$apps" -v nami-showcase-nix:/nix -v "$nami":/usr/local/bin/nami:ro -v "$index":/index.json:ro \
    -v "$out/src":/src:ro -v "$out/b":/out -v "$out/logs":/logs ubuntu:24.04 bash -c '
    set -u
    fails=0
    nami update --from /index.json --unsigned >/dev/null
    apt-get update -qq >/dev/null && DEBIAN_FRONTEND=noninteractive apt-get install -y -qq --no-install-recommends ca-certificates >/dev/null
    # t <app> <dir> <nami bundle arguments>: the bundle, and its thin one.
    t() {
      app=$1 dir=$2; shift 2
      case " $APPS " in *" $app "*) ;; *) return 0 ;; esac
      s=$(date +%s)
      if (cd "$dir" && nami bundle "$@" --desktop -o /out/$app-x86_64) >/logs/make-$app.out 2>/logs/make-$app.log; then
        echo "  ok   $app: $(du -h /out/$app-x86_64 | cut -f1), in $(( $(date +%s) - s ))s"
      else
        echo "  FAIL $app"; tail -30 /logs/make-$app.log | sed "s/^/       /"; fails=$((fails + 1)); return
      fi
      grep -E "^(project|  [a-z]+ +|  note)" /logs/make-$app.log | sed "s/^/       /"
      (cd "$dir" && nami bundle "$@" --thin -o /out/$app-thin-x86_64) >/dev/null 2>/logs/make-$app-thin.log \
        && echo "  ok   $app --thin: $(du -h /out/$app-thin-x86_64 | cut -f1)" || { echo "  FAIL $app --thin"; tail -10 /logs/make-$app-thin.log; fails=$((fails + 1)); }
    }
    t opensurge /tmp opensurge
    t strawberry /tmp strawberry
    t rnote /tmp rnote
    t amberol /tmp amberol
    t element /tmp element-desktop
    t aria2 /tmp aria2
    # From upstream: the unpacked source, as a developer has it.
    rm -rf /tmp/simp /tmp/tmux && cp -a /src/simp/tree /tmp/simp && cp -a /src/tmux/tree /tmp/tmux
    t simp /tmp/simp .
    t tmux /tmp/tmux .
    [ "$fails" -eq 0 ]' || fail=$((fail + 1))
}

# A distro with what a desktop has: X11 (Xvfb), Wayland (Weston), Mesa with
# OpenGL and Vulkan (llvmpipe, lavapipe), fonts, D-Bus, PulseAudio, GNOME
# Keyring.
gui_image() {
  name=nami-showcase2-$(echo "$1" | tr ':/.' '---')
  docker image inspect "$name" >/dev/null 2>&1 && { echo "$name"; return; }
  case $1 in
    ubuntu:20.04) pkgs="RUN apt-get update && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends xvfb x11-utils imagemagick libgl1-mesa-dri libegl1 libglx-mesa0 libgl1 mesa-vulkan-drivers fonts-dejavu-core adwaita-icon-theme procps dbus pulseaudio pulseaudio-utils gnome-keyring ca-certificates" ;;
    ubuntu:*|debian:*|*linuxmintd/*) pkgs="RUN apt-get update && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends xvfb x11-utils imagemagick libgl1-mesa-dri libegl1 libglx-mesa0 libgl1 mesa-vulkan-drivers fonts-dejavu-core adwaita-icon-theme procps dbus weston pulseaudio pulseaudio-utils gnome-keyring ca-certificates" ;;
    fedora:*) pkgs="RUN dnf install -y xorg-x11-server-Xvfb xwininfo ImageMagick mesa-dri-drivers mesa-libEGL mesa-libGL mesa-vulkan-drivers dejavu-sans-fonts adwaita-icon-theme procps-ng dbus-daemon weston pulseaudio pulseaudio-utils gnome-keyring && dnf clean all" ;;
    archlinux:*) pkgs="RUN pacman -Syu --noconfirm xorg-server-xvfb xorg-xwininfo imagemagick mesa vulkan-swrast ttf-dejavu adwaita-icon-theme procps-ng dbus weston pulseaudio gnome-keyring" ;;
    opensuse/*) pkgs="RUN zypper -n install --no-recommends xorg-x11-server-Xvfb xwininfo ImageMagick Mesa-dri Mesa-libEGL1 Mesa-libGL1 libvulkan_lvp dejavu-fonts adwaita-icon-theme procps dbus-1-daemon weston pulseaudio pulseaudio-utils gnome-keyring gzip && zypper clean -a" ;;
  esac
  printf 'FROM %s\n%s\nRUN userdel -r ubuntu 2>/dev/null; useradd -m -u 1000 u\n' "$1" "$pkgs" | docker build -q -t "$name" - >/dev/null || return 1
  echo "$name"
}

run() {
  # Something to look at and something to hear: simp's own icon, and 20
  # seconds of 440 Hz for Amberol.
  cp "$out/src/simp/tree/icon.png" "$out/b/showcase.png" 2>/dev/null
  [ -s "$out/b/tone.wav" ] || python3 -I -c 'import math,struct,sys,wave
w=wave.open(sys.argv[1],"wb"); w.setnchannels(1); w.setsampwidth(2); w.setframerate(44100)
w.writeframes(b"".join(struct.pack("<h",int(12000*math.sin(2*math.pi*440*i/44100))) for i in range(44100*20)))' "$out/b/tone.wav"
  for image in $images; do
    log "== run on $image"
    img=$(gui_image "$image") || { log "  FAIL building the image"; fail=$((fail + 1)); continue; }
    # As someone, not root (Electron won't run as root with its sandbox);
    # Chromium's sandbox makes user namespaces, which the container's
    # default seccomp filter refuses (a desktop doesn't). Rootless podman:
    # the container's user 1000 is you, who can write the screenshots.
    keep=; docker --version 2>/dev/null | grep -qi podman && keep=--userns=keep-id:uid=1000,gid=1000
    docker run --rm $keep --security-opt seccomp=unconfined --shm-size=1g -u 1000:1000 -e HOME=/home/u \
      -e APPS="$apps" -e DISTRO="$(echo "$image" | tr ':/' '--')" -v "$out/b":/b:ro -v "$out/shots":/shots \
      -v "$PWD/tests/showcase-inner.sh":/run.sh:ro "$img" dbus-run-session -- sh /run.sh || fail=$((fail + 1))
  done
}

for s in $stages; do
  case $s in
    make) make_bundles ;;
    run) run ;;
    *) echo "unknown stage $s"; exit 2 ;;
  esac
done
[ "$fail" -eq 0 ] && echo "PASS" || { echo "$fail FAILED"; exit 1; }
