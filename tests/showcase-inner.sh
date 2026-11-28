#!/bin/sh
# Runs inside a clean container, as someone who isn't root; see
# tests/showcase.sh. The bundles are in /b, screenshots go to /shots.
# Each app starts with an empty cache (its first start, timed), then again.
# It prints a line per check, then one `RESULT` line per app for the table.
set -u
fails=0
ok() { echo "  ok   $1"; }
bad() { echo "  FAIL $1"; fails=$((fails + 1)); }
ms() { echo $(( ($(date +%s%N) - $1) / 1000000 )); }
. /etc/os-release 2>/dev/null
# Linux Mint's image says it's Ubuntu (its os-release); its apt sources say which Mint.
grep -rqs "packages.linuxmint.com zena" /etc/apt/ && PRETTY_NAME="Linux Mint 22.3"
echo "  ($PRETTY_NAME; glibc $(ldd --version 2>/dev/null | head -1 | sed 's/.* //'); $(uname -r))"
C=$HOME/.cache/tray-bundle
fresh() { rm -rf "$C" /tmp/nami-bundle-*; }
unpacked() { du -sm "$C/store" 2>/dev/null | cut -f1; }

# The desktop: X11, sound into a null sink, a keyring, and Wayland where there's Weston.
Xvfb :99 -screen 0 1280x800x24 >/tmp/xvfb.log 2>&1 &
export DISPLAY=:99
for _ in $(seq 40); do xwininfo -root >/dev/null 2>&1 && break; sleep 0.25; done
export XDG_RUNTIME_DIR=/tmp/xdg && mkdir -p $XDG_RUNTIME_DIR && chmod 700 $XDG_RUNTIME_DIR
pulseaudio --daemonize=yes --exit-idle-time=-1 --log-target=file:/tmp/pulse.log -n \
  --load=module-native-protocol-unix --load="module-null-sink sink_name=null" --load=module-always-sink >/dev/null 2>&1
for _ in $(seq 20); do pactl info >/dev/null 2>&1 && break; sleep 0.25; done
pactl info >/dev/null 2>&1 || echo "  (no PulseAudio here: sound isn't checked)"
# A keyring, as a desktop has (Element keeps its keys there): the login one,
# made and unlocked (a login does that).
mkdir -p ~/.local/share/keyrings && printf login > ~/.local/share/keyrings/default
printf showcase | gnome-keyring-daemon --unlock --components=secrets >/dev/null 2>&1
wl=
if command -v weston >/dev/null; then
  for backend in headless headless-backend.so; do
    weston --backend=$backend --socket=wayland-9 --idle-time=0 >/tmp/weston.log 2>&1 &
    for _ in $(seq 20); do [ -S $XDG_RUNTIME_DIR/wayland-9 ] && break; sleep 0.3; done
    [ -S $XDG_RUNTIME_DIR/wayland-9 ] && { wl=wayland-9; break; }
  done
fi

# Something heard: a stream from the app into the null sink, and not silence.
sound() { # app
  pactl info >/dev/null 2>&1 || { echo "-"; return; }
  if pactl list sink-inputs 2>/dev/null | grep -q 'Sink Input'; then
    timeout 2 parec -d null.monitor --raw --format=s16le 2>/dev/null | od -An -v -tx1 | tr -d ' \n0' | grep -q . && { echo ok; return; }
    echo silent; return
  fi
  echo none
}

window_up() { xwininfo -root -tree 2>/dev/null | grep -i -- "$1" | grep -qv 'has no name'; }

# gui <app> <window title> <sound: yes|no> <command...>
gui() {
  app=$1 title=$2 want_sound=$3; shift 3
  case " ${APPS:-$app} " in *" $app "*) ;; *) return 0 ;; esac
  [ -e "/b/$app-x86_64" ] || return 0
  fresh
  start=$(date +%s%N)
  setsid "$@" >/tmp/$app.log 2>&1 &
  pid=$!
  first=
  for _ in $(seq 480); do window_up "$title" && { first=$(ms $start); break; }; sleep 0.25; done
  draws=no snd=- wayland=- second=-
  if [ -n "$first" ] && kill -0 $pid 2>/dev/null; then
    ok "$app: a window on X11, $first ms after its first start"
    sleep ${SHOT_WAIT:-6}
    [ "$want_sound" = yes ] && snd=$(sound "$app")
    import -window root /tmp/shot.png 2>/dev/null
    colours=$(identify -format %k /tmp/shot.png 2>/dev/null || echo 0)
    if [ "${colours:-0}" -gt 8 ]; then
      draws=ok; cp /tmp/shot.png "/shots/$DISTRO-$app.png" && ok "$app: draws ($colours colours), screenshot shots/$DISTRO-$app.png"
    else
      bad "$app: draws (the screen has $colours colours)"; cp /tmp/shot.png "/shots/$DISTRO-$app-blank.png"
    fi
    case $snd in ok) ok "$app: sound, into the null sink" ;; -) ;; *) bad "$app: sound ($snd)" ;; esac
  else
    bad "$app: a window on X11"; tail -20 /tmp/$app.log | sed 's/^/       /'
  fi
  kill -- -$pid 2>/dev/null; sleep 1; kill -9 -- -$pid 2>/dev/null; wait $pid 2>/dev/null
  size=$(unpacked)
  # Again: what's there is used.
  if [ -n "$first" ]; then
    start=$(date +%s%N)
    setsid "$@" >/tmp/$app-2.log 2>&1 &
    pid=$!
    for _ in $(seq 240); do window_up "$title" && { second=$(ms $start); break; }; sleep 0.1; done
    kill -- -$pid 2>/dev/null; sleep 1; kill -9 -- -$pid 2>/dev/null; wait $pid 2>/dev/null
  fi
  # Wayland: up, with a toplevel that has a title, and something attached.
  if [ -n "$wl" ] && [ -n "$first" ] && [ "$WLENV" != skip ]; then
    WAYLAND_DISPLAY=$wl WAYLAND_DEBUG=client DISPLAY= setsid env $WLENV "$@" $WLARGS >/tmp/$app-wl.log 2>&1 &
    pid=$!
    for _ in $(seq 120); do grep -q 'xdg_toplevel.*set_title' /tmp/$app-wl.log 2>/dev/null && break; sleep 0.5; done
    sleep 3
    if kill -0 $pid 2>/dev/null && grep -q 'xdg_toplevel.*set_title' /tmp/$app-wl.log && grep -q 'wl_surface.*attach' /tmp/$app-wl.log; then
      wayland=ok; ok "$app: on Wayland, $(grep -o 'xdg_toplevel[^ ]*set_title("[^"]*")' /tmp/$app-wl.log | head -1)"
    else
      wayland=no; bad "$app: on Wayland"; grep -v '^\[' /tmp/$app-wl.log | tail -8 | sed 's/^/       /'
    fi
    kill -- -$pid 2>/dev/null; sleep 1; kill -9 -- -$pid 2>/dev/null; wait $pid 2>/dev/null
  fi
  echo "RESULT $DISTRO $app start=$([ -n "$first" ] && echo ok || echo no) draws=$draws sound=$snd wayland=$wayland first_ms=${first:--} second_ms=$second unpacked_mib=${size:--}"
}

# cli <app> <check...>: run in a shell, with an empty cache, timed.
cli() {
  app=$1; shift
  case " ${APPS:-$app} " in *" $app "*) ;; *) return 0 ;; esac
  [ -e "/b/$app-x86_64" ] || return 0
  fresh
  start=$(date +%s%N)
  "/b/$app-x86_64" ${VFLAG:---version} >/tmp/$app.v 2>&1 && first=$(ms $start) || first=
  start=$(date +%s%N)
  "/b/$app-x86_64" ${VFLAG:---version} >/dev/null 2>&1 && second=$(ms $start) || second=-
  works=no
  if [ -n "$first" ] && sh -c "$*" >/tmp/$app.log 2>&1; then works=ok; ok "$app: $(head -1 /tmp/$app.v), works, first start $first ms"
  else bad "$app: works"; tail -n 15 /tmp/$app.v /tmp/$app.log | sed 's/^/       /'; fi
  echo "RESULT $DISTRO $app start=$([ -n "$first" ] && echo ok || echo no) works=$works first_ms=${first:--} second_ms=$second unpacked_mib=$(unpacked)"
}

# thin <app>: the small file fetches the rest, checked, from cache.nixos.org.
thin() {
  app=$1; shift
  case " ${APPS:-$app} " in *" $app "*) ;; *) return 0 ;; esac
  [ -e "/b/$app-thin-x86_64" ] || return 0
  fresh
  start=$(date +%s%N)
  if "/b/$app-thin-x86_64" --nami-bundle-extract >/tmp/$app-thin.log 2>&1; then
    ok "$app --thin ($(du -k /b/$app-thin-x86_64 | cut -f1) KiB): fetched what it lacks in $(ms $start) ms"
    echo "RESULT $DISTRO $app-thin fetched_ms=$(ms $start) unpacked_mib=$(unpacked)"
  else
    bad "$app --thin"; tail -10 /tmp/$app-thin.log | sed 's/^/       /'
  fi
}

# The apps.
WLARGS= WLENV=skip SHOT_WAIT=14   # Allegro 5 speaks X11 only (XWayland, on a Wayland desktop)
gui opensurge "surge" yes /b/opensurge-x86_64
WLENV= SHOT_WAIT=6
gui simp "simp" no /b/simp-x86_64 /b/showcase.png
WLENV="QT_QPA_PLATFORM=wayland"
gui strawberry "strawberry" no /b/strawberry-x86_64
WLENV="GDK_BACKEND=wayland"
gui rnote "rnote" no /b/rnote-x86_64
gui amberol "amberol" yes /b/amberol-x86_64 /b/tone.wav
WLENV= WLARGS=--ozone-platform=wayland SHOT_WAIT=15
# (Electron finds the keyring by the desktop's name, XDG_CURRENT_DESKTOP: this has none.)
gui element "element" no /b/element-x86_64 --password-store=gnome-libsecret
VFLAG=-V cli tmux 'S=/tmp/t.sock; /b/tmux-x86_64 -S $S -f /dev/null new-session -d -x 80 -y 24 "echo hello from \$(uname -s); sleep 30" && sleep 1 && /b/tmux-x86_64 -S $S capture-pane -p | grep -q "hello from Linux" && TERM=xterm-256color /b/tmux-x86_64 -S $S list-sessions && /b/tmux-x86_64 -S $S kill-server'
VFLAG=--version cli aria2 'cd /tmp && /b/aria2-x86_64 -q -o nix-cache-info https://cache.nixos.org/nix-cache-info && grep -q StoreDir /tmp/nix-cache-info'
for a in opensurge simp strawberry rnote amberol element tmux aria2; do thin $a; done
mkdir -p /shots/logs && for f in /tmp/*.log; do cp "$f" "/shots/logs/$DISTRO-$(basename "$f")"; done
[ "$fails" -eq 0 ]
