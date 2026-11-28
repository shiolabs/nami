#!/usr/bin/env sh
# GUI checks in clean containers, using only what install.sh sets up.
#  1. GPU: OpenGL (EGL) and Vulkan through nixpkgs mesa. Uses the host GPU via
#     /dev/dri when available, otherwise software rendering (llvmpipe/lavapipe).
#  2. Firefox renders a page with several scripts and font families; the
#     screenshots land in <out-dir> for a human to look at.
#
# usage: tests/gui.sh <index.json.xz> <out-dir> [image...]
set -u
cd "$(dirname "$0")/.."
PATH="$PWD/tests/lib:$PATH"   # docker, or rootless podman where there is none

index=$(realpath "${1:?index}"); out=$(realpath -m "${2:?out dir}"); shift 2
mkdir -p "$out"
images=${*:-"ubuntu:24.04 fedora:latest alpine:latest archlinux:latest"}
dri=""; [ -e /dev/dri ] && dri="--device /dev/dri"

cat > "$out/page.html" <<'EOF'
<html><body style="font-family:sans-serif;padding:32px;background:#f6f3ee">
<h1>nami 🎉</h1><p>Firefox from nixpkgs, installed by nami. No Nix on this machine.</p>
<p style="font-family:serif">Serif: Æble, Øl, Ål. Ελληνικά, Кириллица</p>
<p style="font-family:monospace">monospace: nami install firefox</p>
<canvas id=c width=300 height=60></canvas><p id=gl></p>
<script>
let g=document.getElementById('c').getContext('2d');g.fillStyle='#c33';g.fillRect(0,0,300,60);
g.fillStyle='#fff';g.font='26px sans-serif';g.fillText('canvas OK',80,40);
document.getElementById('gl').textContent='WebGL: '+(document.createElement('canvas').getContext('webgl')?'available':'unavailable');
</script></body></html>
EOF

scenario='
set -eu
/src/install.sh /new/nami >/dev/null
. /etc/profile.d/nami.sh
nami update --from /index.json.xz --unsigned >/dev/null
nami install mesa-demos vulkan-tools firefox 2>&1 | grep -E "^(  [+~−]|✓|error)"
nami doctor | grep -E "Graphics|apps with windows"
echo "GL:     $(eglinfo -B -p surfaceless 2>/dev/null | grep -m1 "core profile renderer" | cut -d: -f2)"
echo "Vulkan: $(vulkaninfo --summary 2>/dev/null | grep -m1 deviceName | cut -d= -f2)"
HOME=/tmp timeout 120 firefox --headless --window-size=800,420 --screenshot /out/$SHOT file:///out/page.html >/dev/null 2>&1
test -s /out/$SHOT && echo "screenshot: $SHOT"
'

fail=0
for image in $images; do
  shot="firefox-$(echo "$image" | tr ':/' '--').png"
  echo "== $image"
  if ! docker run --rm $dri -e SHOT="$shot" \
      -v "$PWD/target/release/nami":/new/nami:ro -v "$PWD":/src:ro \
      -v "$index":/index.json.xz:ro -v "$out":/out "$image" sh -c "$scenario"; then
    echo "FAIL $image"; fail=$((fail + 1))
  fi
done
[ "$fail" -eq 0 ]
