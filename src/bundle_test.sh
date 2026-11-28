#!/bin/sh
# `nami bundle --test` (src/bundle_test.rs): runs inside a clean container,
# as someone who isn't root, the bundle at /b/app. As tests/showcase-inner.sh
# does for the site's showcase: an empty cache (its first start, timed);
# for an app with a window, X11 (Xvfb), D-Bus and a window looked for, and
# a screenshot's colours (that it draws); for a command, it run with
# $NAMI_TEST_ARGS. The last line is one `RESULT key=value ...` for nami.
set -u
. /etc/os-release 2>/dev/null
# Linux Mint's own image (linuxmintd) is Ubuntu's with Mint's packages:
# its os-release is Ubuntu's, its apt sources say which Mint.
mint=$(sed -n 's/^deb .*packages\.linuxmint\.com \([a-z]*\) .*/\1/p' /etc/apt/sources.list /etc/apt/sources.list.d/*.list 2>/dev/null | head -1)
case $mint in
  alfa) PRETTY_NAME="Linux Mint 23" ;; zena) PRETTY_NAME="Linux Mint 22.3" ;; zara) PRETTY_NAME="Linux Mint 22.2" ;; xia) PRETTY_NAME="Linux Mint 22.1" ;; wilma) PRETTY_NAME="Linux Mint 22" ;;
  virginia) PRETTY_NAME="Linux Mint 21.3" ;; ?*) PRETTY_NAME="Linux Mint ($mint)" ;;
esac
PRETTY_NAME=$(echo "${PRETTY_NAME:-}" | sed 's/ (Container Image)//')
glibc=$(ldd --version 2>/dev/null | head -1 | sed 's/.* //')
ms() { echo $(( ($(date +%s%N) - $1) / 1000000 )); }
result() { echo "RESULT distro=$(echo "${PRETTY_NAME:-?}" | tr ' ' '_') glibc=${glibc:-?} $*"; exit 0; }
# What a program that didn't start says, from the loader or the kernel.
broken() { grep -Eqi 'error while loading shared libraries|cannot open shared object|version .GLIBC_|exec format error|no such file or directory.*ld-linux|segmentation fault|illegal instruction|bus error' "$1"; }
rm -rf "$HOME/.cache/nami-bundle" "$HOME/.cache/tray-bundle" /tmp/nami-bundle-* 2>/dev/null

if [ "${NAMI_TEST_KIND:-cli}" = cli ]; then
  start=$(date +%s%N)
  # shellcheck disable=SC2086
  timeout 60 /b/app $NAMI_TEST_ARGS </dev/null >/tmp/out 2>&1
  code=$? first=$(ms "$start")
  sed 's/^/  | /' /tmp/out | tail -n 12
  started=yes
  case $code in 124) started=yes ;; 126|127) started=no ;; esac
  [ "$code" -gt 128 ] && [ "$code" != 124 ] && started=no
  broken /tmp/out && started=no
  result "started=$started exit=$code first_ms=$first"
fi

# An app with a window: X11 (Xvfb), D-Bus, a runtime folder.
export XDG_RUNTIME_DIR=/tmp/xdg && mkdir -p $XDG_RUNTIME_DIR && chmod 700 $XDG_RUNTIME_DIR
Xvfb :99 -screen 0 1280x800x24 >/tmp/xvfb.log 2>&1 &
export DISPLAY=:99
for _ in $(seq 40); do xwininfo -root >/dev/null 2>&1 && break; sleep 0.25; done
# A window: a child of the root that has a name and is on the screen.
window_up() {
  for id in $(xwininfo -root -children 2>/dev/null | sed -n 's/^ *\(0x[0-9a-f]*\) "[^"]\+".*/\1/p'); do
    xwininfo -id "$id" 2>/dev/null | grep -q 'IsViewable' && { xwininfo -id "$id" | sed -n 's/^xwininfo: Window id: [^ ]* "\(.*\)"/\1/p' | head -1; return 0; }
  done
  return 1
}
start=$(date +%s%N)
# shellcheck disable=SC2086
setsid /b/app $NAMI_TEST_ARGS </dev/null >/tmp/out 2>&1 &
pid=$!
first= title=
for _ in $(seq 480); do
  title=$(window_up) && { first=$(ms "$start"); break; }
  kill -0 $pid 2>/dev/null || break
  sleep 0.25
done
started=yes window=no draws=no colours=0
if [ -n "$first" ]; then
  window=yes
  sleep "${NAMI_TEST_WAIT:-4}"
  if command -v import >/dev/null && import -window root /tmp/shot.png 2>/dev/null; then
    colours=$(identify -format %k /tmp/shot.png 2>/dev/null || echo 0)
    [ "${colours:-0}" -gt 8 ] && draws=yes
    cp /tmp/shot.png /shots/shot.png 2>/dev/null
  else
    draws=-
  fi
else
  kill -0 $pid 2>/dev/null || started=no
  broken /tmp/out && started=no
fi
sed 's/^/  | /' /tmp/out | grep -v '^  | *$' | tail -n 12
kill -- -$pid 2>/dev/null; sleep 1; kill -9 -- -$pid 2>/dev/null
result "started=$started window=$window draws=$draws colours=$colours first_ms=${first:--} title=$(echo "$title" | tr ' ' '_')"
