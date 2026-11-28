#!/bin/sh
# In the container (tests/claude-desktop-bundle.sh): the bundle at $1.
set -u
b=$1
fails=0
ok() { echo "  ok   $1"; }
bad() { echo "  FAIL $1"; fails=$((fails + 1)); }
. /etc/os-release 2>/dev/null
echo "  ($PRETTY_NAME; the distro's glibc: $(ldd --version 2>/dev/null | head -1 | sed 's/.* //'))"
mkdir -p /b/shots

"$b" --nami-bundle-extract >/tmp/extract.log 2>&1 && ok "unpacked ($(du -sm ~/.cache/tray-bundle 2>/dev/null | cut -f1) MiB)" || { bad "unpacked"; tail /tmp/extract.log; }

# X11: a window named Claude, and what's in it after it has loaded.
Xvfb :99 -screen 0 1400x900x24 >/tmp/xvfb.log 2>&1 &
export DISPLAY=:99
for _ in $(seq 20); do xwininfo -root >/dev/null 2>&1 && break; sleep 0.3; done
start=$(date +%s)
setsid "$b" --ozone-platform=x11 >/tmp/x11.log 2>&1 &
pid=$!
for _ in $(seq 120); do
  sleep 0.5
  xwininfo -root -tree 2>/dev/null | grep -q '"Claude"' && break
done
if kill -0 $pid 2>/dev/null && xwininfo -root -tree | grep -q '"Claude"'; then
  ok "a window named Claude, on X11, $(( $(date +%s) - start )) s after starting it"
  sleep 25
  import -window root "/b/shots/$DISTRO-x11.png" 2>/dev/null && ok "screenshot shots/$DISTRO-x11.png"
else
  bad "a window named Claude, on X11"
fi
grep -iE 'error|fatal|sandbox' /tmp/x11.log | grep -v 'dbus\|Gtk-WARNING\|viz_main\|gpu_init\|command_buffer' | head -8 | sed 's/^/       /'
cp /tmp/x11.log /b/shots/$DISTRO-x11.log 2>/dev/null
kill -- -$pid 2>/dev/null; wait $pid 2>/dev/null; sleep 2
pkill -f 'lib/claude-desktop/claude-desktop' 2>/dev/null; sleep 1

# Wayland (Weston, headless): Electron picks Wayland by itself in a Wayland session (XDG_SESSION_TYPE).
if command -v weston >/dev/null; then
  export XDG_RUNTIME_DIR=/tmp/xdg && mkdir -p $XDG_RUNTIME_DIR && chmod 700 $XDG_RUNTIME_DIR
  weston --backend=headless --socket=wayland-9 --idle-time=0 >/tmp/weston.log 2>&1 &
  for _ in $(seq 20); do [ -S $XDG_RUNTIME_DIR/wayland-9 ] && break; sleep 0.3; done
  XDG_SESSION_TYPE=wayland WAYLAND_DISPLAY=wayland-9 WAYLAND_DEBUG=1 DISPLAY= setsid "$b" >/tmp/wl.log 2>&1 &
  pid=$!
  for _ in $(seq 60); do sleep 0.5; grep -q 'xdg_toplevel.*set_app_id' /tmp/wl.log && break; done
  sleep 3
  if kill -0 $pid 2>/dev/null && grep -q 'xdg_toplevel.*set_app_id("com.anthropic.Claude")' /tmp/wl.log && grep -q 'wl_surface.*attach' /tmp/wl.log; then
    ok "on Wayland: a window, app_id com.anthropic.Claude"
  else
    bad "on Wayland: a window, app_id com.anthropic.Claude"; grep -v '^\[' /tmp/wl.log | tail -8 | sed 's/^/       /'
  fi
  kill -- -$pid 2>/dev/null
fi
cp /tmp/wl.log /b/shots/$DISTRO-wayland.log 2>/dev/null
[ $fails = 0 ]
