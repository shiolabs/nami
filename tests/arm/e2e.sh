#!/bin/sh
# nami on ARM Linux (aarch64-linux), end to end, for real (on GitHub's ubuntu-24.04-arm runners:
# .github/workflows/arm-linux.yml; never on a computer someone uses: it installs nami in /nix
# and /nami and takes it away again).
#
# A site like namios.dev, served here: a release with this nami's tarball for ARM Linux (and
# nami's app's, when it's given), signed, and the catalog in tests/arm (a handful of packages,
# all ready-built on cache.nixos.org), signed with a test key, named by ARM Linux's own channel
# file. Then what someone does:
#   curl -fsSL <site>/install | sh          /nix, /nami, the daemon, the shells, the catalog
#                                           (with --no-app, when the release has the app)
#   nami install ripgrep jq                 as themselves, through the daemon
#   a project: nami shell -c, nami run <task>, nami env; one that builds with stdenv's gcc
#   nami undo, user.nami: apply / history / diff / undo
#   nami bundle jq                          a bundle for ARM computers, made and run here
#   curl -fsSL <site>/install | sh          again, for nami's app (nami-gui): built from its
#                                           recipe, opened on screens of its own (Xvfb, cage)
#   sudo nami remove nami-gui               taken out
#   sudo nami install nami-gui              and back
#   sudo sh /nami/uninstall.sh              and nothing of it left
# usage: tests/arm/e2e.sh <the release's tarball, nami-<v>-aarch64-linux.tar.gz>
#                         [its app's, nami-gui-<v>-aarch64-linux.tar.gz]
# (That nami carries the app's checksum: packaging/gui-source.sh wrote it before nami was built.)
set -eu
here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../.." && pwd)
tarball=$(cd "$(dirname "$1")" && pwd)/$(basename "$1")
gui=; [ -z "${2:-}" ] || gui=$(cd "$(dirname "$2")" && pwd)/$(basename "$2")
[ "$(uname -s)-$(uname -m)" = Linux-aarch64 ] || { echo "this is for ARM Linux"; exit 1; }
system=aarch64-linux
version=$(sed -n 's/^version = "\(.*\)"/\1/p' "$repo/Cargo.toml" | head -1)
[ "$(basename "$tarball")" = "nami-$version-$system.tar.gz" ] || { echo "$tarball isn't nami-$version-$system.tar.gz"; exit 1; }
[ -z "$gui" ] || [ "$(basename "$gui")" = "nami-gui-$version-$system.tar.gz" ] || { echo "$gui isn't nami-gui-$version-$system.tar.gz"; exit 1; }
work=$(mktemp -d /tmp/nami-e2e.XXXXXX)
chmod 755 "$work"
site=$work/site port=8765 base=http://127.0.0.1:8765
step() { printf '\n=== %s\n' "$*"; }
fail() { printf '\nFAIL: %s\n' "$*"; exit 1; }
# A program nami installed is found first (not Ubuntu's).
ours() { hash -r; p=$(command -v "$1") || fail "no $1"; case $p in */.local/state/nami/*|/nix/*|/nami/*) echo "$1: $p" ;; *) fail "$1 is $p, not nami's" ;; esac; }
gone() { hash -r; p=$(command -v "$1" 2>/dev/null) || return 0; case $p in */.local/state/nami/*|/nix/*|/nami/*) fail "$1 still there ($2): $p" ;; esac; }

step "the release's nami"
mkdir -p "$work/unpacked"
tar -xzf "$tarball" -C "$work/unpacked"
bin=$work/unpacked/nami-$version/nami
file "$bin"
file "$bin" | grep -q 'ARM aarch64' || fail "nami isn't an aarch64 program"
file "$bin" | grep -q 'statically linked' || fail "nami isn't static"
"$bin" --version

step "a test key, and the site"
# (Signing as root of a scratch store: nami keeps keys in its store's var folder.)
keys=$work/keys
pub=$(sudo env NAMI_ROOT="$keys" NAMI_NO_MIGRATE=1 "$bin" keygen test-1 2>/dev/null | tail -1)
case $pub in test-1:*) ;; *) fail "no test key: $pub" ;; esac
sign() { sudo env NAMI_ROOT="$keys" NAMI_NO_MIGRATE=1 "$bin" catalog sign "$1" --key test-1 >/dev/null; sudo chown "$(id -u)" "$1.sig"; }
rel=$site/releases/$version
mkdir -p "$rel" "$site/menu/test"
cp "$tarball" "$rel/"
# (nami's app beside it, in SHA256SUMS, as packaging/publish.sh puts it: the installer offers
# it on ARM Linux when the release has it.)
[ -z "$gui" ] || cp "$gui" "$rel/"
(cd "$rel" && sha256sum nami-*"-$system.tar.gz" > SHA256SUMS)
cat "$rel/SHA256SUMS"
sign "$rel/SHA256SUMS"
echo "$version" > "$site/releases/latest"
cp "$here/index-$system.json" "$site/menu/test/index-$system.json"
xz -k "$site/menu/test/index-$system.json"
sign "$site/menu/test/index-$system.json.xz"
sign "$site/menu/test/index-$system.json"
rm "$site/menu/test/index-$system.json"
# x86_64's channel names a release with no ARM catalog in it; ARM Linux's own channel file names
# the one that has it, as packaging/publish.sh makes them (SYSTEM=aarch64-linux): it's read first.
echo x86only > "$site/menu/nixos-unstable"
echo test > "$site/menu/nixos-unstable.$system"
sed -e "s|@BASE@|$base|g" -e "s|@CHANNEL@|nixos-unstable|g" -e "s|@KEYS@|$pub|g" -e "s|@KEY@|$pub|g" "$repo/packaging/install-web.sh" > "$site/install"
(cd "$site" && exec python3 -m http.server "$port" --bind 127.0.0.1 >"$work/http.log" 2>&1) &
server=$!
trap 'kill $server 2>/dev/null || true' EXIT
i=0; until curl -fsS "$base/releases/latest" >/dev/null 2>&1; do i=$((i + 1)); [ $i -lt 50 ] || fail "the test site didn't start"; sleep 0.2; done

# A desktop to log in to, as the installer looks for one before it installs nami's app.
sudo mkdir -p /usr/share/wayland-sessions
[ -e /usr/share/wayland-sessions/nami-e2e.desktop ] || { printf '[Desktop Entry]\nName=Desktop\nExec=true\n' | sudo tee /usr/share/wayland-sessions/nami-e2e.desktop >/dev/null; desktop_file=1; }
trap 'kill $server 2>/dev/null || true; [ -z "${desktop_file:-}" ] || sudo rm -f /usr/share/wayland-sessions/nami-e2e.desktop' EXIT
gui_listed() { sudo grep -qE '^[[:space:]]*-[[:space:]]*(/etc/nami/packages/)?nami-gui[[:space:]]*$' /etc/nami/system.nami 2>/dev/null; }

if [ -n "$gui" ]; then
  step "curl -fsSL $base/install | sh -s -- --no-app"
  curl -fsSL "$base/install" | NAMI_TEST=1 sh -s -- --no-app 2>&1 | tee "$work/install.log"
  grep -q "Add it with: sudo nami install nami-gui" "$work/install.log" || fail "the installer didn't say how to add nami's app"
else
  step "curl -fsSL $base/install | sh (a release without nami's app for ARM Linux)"
  curl -fsSL "$base/install" | NAMI_TEST=1 sh 2>&1 | tee "$work/install.log"
  ! grep -q 'nami-gui' "$work/install.log" || fail "the installer offered nami's app (nami-gui), which this release hasn't got for ARM Linux"
fi
grep -q "nami $version is installed" "$work/install.log" || fail "the installer didn't finish"
! grep -q "Installing nami's app" "$work/install.log" || fail "the installer installed nami's app"
[ ! -e /etc/nami/packages/nami-gui ] && ! gui_listed || fail "nami's app is there"
V=$(cd /nix/var/nami && pwd -P)
echo "nami's folder: $V"
[ -x "$V/bin/nami" ] || fail "no $V/bin/nami"
systemctl is-active nami-daemon.service || fail "the daemon isn't running"
[ -S "$V/daemon.sock" ] || ls -la "$V"
grep -q "$system" "$V/index-resolved" || fail "the catalog isn't ARM Linux's: $(cat "$V/index-resolved")"
cat "$V/index-resolved"

step "a new terminal finds nami"
bash -lc 'command -v nami && nami --version' || fail "a login bash doesn't find nami"
# From here, as a new terminal has it.
for f in /etc/profile.d/nami.sh; do [ -e "$f" ] && . "$f"; done
export PATH="$HOME/.local/state/nami/profile/bin:$V/bin:$PATH"
command -v nami
nami --version

step "nami search, info, list"
nami search ripgrep | head -5
nami info jq
nami list

step "nami install ripgrep jq (as $(id -un), through the daemon)"
nami install ripgrep jq
ours rg
ours jq
rg --version
file -L "$(command -v rg)" | grep -q 'ARM aarch64' || fail "rg isn't an aarch64 program"
echo '{"a":[1,2]}' | jq -c '.a | length' | grep -qx 2 || fail "jq didn't work"
nami list | grep -E 'ripgrep|jq'

step "nami update --refresh, nami upgrade"
nami update --refresh
nami upgrade --dry-run
nami upgrade
rg --version >/dev/null || fail "ripgrep went in the upgrade"

step "history, diff, undo, remove"
nami install fd
ours fd
fd --version
nami history
nami diff
nami undo
gone fd "after undo"
nami remove jq
gone jq "after remove"
rg --version >/dev/null || fail "ripgrep went with jq"

step "a project: nami shell -c, run, env, hook"
proj=$work/myapp
mkdir -p "$proj"
cat > "$proj/project.nami" <<'EOF'
packages:
  - nodejs
env:
  GREETING: hello from nami
tasks:
  hi: node -e 'console.log(process.env.GREETING)'
EOF
cd "$proj"
nami shell -c 'node --version' | grep -q '^v' || fail "no node in the project's shell"
file -L "$(nami shell -c 'command -v node' | tail -1)" | grep -q 'ARM aarch64' || fail "node isn't an aarch64 program"
nami run hi | grep -qx 'hello from nami' || fail "the task didn't run"
nami run -- node -e 'console.log(1+1)' | grep -qx 2 || fail "nami run -- node failed"
nami env | grep -q 'GREETING' || fail "nami env has no GREETING"
[ -f nami.lock ] && grep -q "\"system\": \"$system\"" nami.lock || fail "no lock for $system"
nami hook bash | grep -q . || fail "no bash hook"
cd "$work"

step "a project to build in: go turns on nixpkgs' build environment (stdenv, its gcc)"
gproj=$work/gotool
mkdir -p "$gproj/c"
printf 'packages:\n  - go\n' > "$gproj/project.nami"
cat > "$gproj/main.go" <<'EOF'
package main

import "fmt"

func main() { fmt.Println("built with nami") }
EOF
printf 'module example.com/hello\n\ngo 1.22\n' > "$gproj/go.mod"
printf '#include <stdio.h>\nint main(void) { puts("built with nami'"'"'s gcc"); return 0; }\n' > "$gproj/c/hi.c"
cd "$gproj"
nami shell -c 'go version && echo "stdenv: $stdenv" && command -v cc'
nami shell -c 'echo $stdenv' | grep -q stdenv-linux || fail "no stdenv in the build environment"
nami run -- go build -o hello . || fail "go build failed"
./hello | grep -qx 'built with nami' || fail "the program go built doesn't run"
nami run -- cc -o hi c/hi.c || fail "cc failed"
./hi | grep -q "built with nami's gcc" || fail "the program cc built doesn't run"
file hi
file hi | grep -q 'ARM aarch64' || fail "cc didn't build for aarch64"
cd "$work"

step "user.nami: apply, history, diff, undo"
cat > "$HOME/user.nami" <<'EOF'
packages:
  - htop
  - ripgrep
env: { NAMI_E2E: "yes" }
aliases: { ll: ls -la }
EOF
nami check "$HOME/user.nami" || true
nami apply 2>&1 | tee "$work/apply.log"
ours htop
htop --version
grep -q NAMI_E2E "$HOME/.config/nami/env.sh" || fail "env: isn't in ~/.config/nami/env.sh"
bash -lc 'echo "$NAMI_E2E"' | grep -qx yes || fail "a new login bash doesn't have env:"
nami history
nami diff
nami undo
gone htop "after undo"
rm -f "$HOME/user.nami"

step "nami bundle jq: one file for ARM computers, made here"
nami install jq
nami bundle jq -o "$work/jq-aarch64" 2>&1 | tee "$work/bundle.log"
[ -x "$work/jq-aarch64" ] || fail "no bundle"
tail -c 4096 "$work/jq-aarch64" | grep -q '"arch":"aarch64"' || fail "the bundle doesn't say it's for aarch64"
"$work/jq-aarch64" --nami-bundle-verify || fail "the bundle doesn't check out against its hash"
echo '[3,4]' | "$work/jq-aarch64" -c add | grep -qx 7 || fail "the bundle didn't run"
# (On its own glibc, from a folder nami isn't in.)
env -i PATH=/usr/bin:/bin HOME="$work" "$work/jq-aarch64" --version || fail "the bundle didn't run without nami"

step "a package that isn't in the catalog says so"
nami install no-such-package-here 2>&1 | tee "$work/missing.log" || true
grep -qi "catalog\|no package" "$work/missing.log" || fail "no clear error for a missing package"

step "nami self-update says it's the newest"
nami self-update --check

if [ -n "$gui" ]; then
  step "nami's app: its download"
  # The recipe this nami carries says where the app is on namios.dev, where this release isn't
  # yet. nami keeps a download by its sha256 and its file's name, so fetching the same file from
  # this site once, for a recipe of its own, is what the app's recipe finds (and checks) then.
  sum=$(sha256sum "$gui" | cut -d' ' -f1)
  grep -q "sha256: $sum" "$repo/examples/packages/nami-gui/package.nami" || fail "the app's recipe doesn't have this nami-gui's checksum ($sum)"
  mkdir -p "$work/fetch-gui"
  printf 'name: fetch-nami-gui\nversion: "1"\nsource:\n  url: %s\n  sha256: %s\ninstall: mkdir -p $out && cp nami-gui.desktop $out/\n' \
    "$base/releases/$version/nami-gui-$version-$system.tar.gz" "$sum" > "$work/fetch-gui/package.nami"
  sudo "$V/bin/nami" build "$work/fetch-gui"

  step "curl -fsSL $base/install | sh: again, without --no-app"
  curl -fsSL "$base/install" | NAMI_TEST=1 sh 2>&1 | tee "$work/app.log"
  grep -q "nami's app is in your app menu" "$work/app.log" || fail "the installer didn't install nami's app"
  grep -q "Remove it with: sudo nami remove nami-gui" "$work/app.log" || fail "the installer didn't say how to remove the app"
  gui_listed || fail "system.nami doesn't list nami-gui"
  sudo cmp "$repo/examples/packages/nami-gui/package.nami" /etc/nami/packages/nami-gui/package.nami || fail "/etc/nami/packages/nami-gui isn't this nami's recipe"
  sudo sed -n '/^source:/,/^build-deps:/p' /etc/nami/packages/nami-gui/package.nami
  ours nami-gui
  prog=$(readlink -f "$(command -v nami-gui)")
  file "$prog"
  file "$prog" | grep -q 'ARM aarch64' || fail "nami-gui isn't an aarch64 program"
  readelf -l "$prog" | grep -q 'interpreter: /nix/store/.*-glibc-.*/lib/ld-linux-aarch64.so.1' || fail "nami-gui doesn't start with nixpkgs' glibc"
  [ "$(nami-gui --version)" = "nami-gui $version" ] || fail "nami-gui --version says $(nami-gui --version)"
  [ -e "${prog%/bin/nami-gui}/share/applications/nami-gui.desktop" ] || fail "nami-gui has no .desktop file"
  [ ! -e "${prog%/bin/nami-gui}/bin/nami-desktop" ] || fail "ARM's nami-gui has namiOS's desktop in it"
  nami doctor 2>&1 | grep -iE 'graphics|vulkan|apps with windows' || true
  ls /run/opengl-driver/share/vulkan/icd.d/

  # <what> <command...>: nami-gui --smoke-test, as the command starts it: its window opens,
  # nami's first look is taken, a frame is drawn after it, and it ends saying so
  # (gui/src/main.rs). It changes nothing.
  opens() {
    what=$1; shift
    timeout 150 "$@" >"$work/smoke.log" 2>&1 || true
    sed 's/^/  /' "$work/smoke.log"
    grep -q 'the window opened and drew' "$work/smoke.log" && echo "nami-gui opened on $what"
  }
  step "nami-gui opens: on X11 (Xvfb), drawn with mesa's Vulkan (lavapipe)"
  Xvfb :99 -screen 0 1280x800x24 -nolisten tcp >"$work/xvfb.log" 2>&1 &
  xvfb=$!
  i=0; until [ -S /tmp/.X11-unix/X99 ]; do i=$((i + 1)); [ $i -lt 50 ] || fail "Xvfb didn't start: $(cat "$work/xvfb.log")"; sleep 0.2; done
  ok_x11=; opens "X11 (Xvfb)" env -u WAYLAND_DISPLAY DISPLAY=:99 nami-gui --smoke-test && ok_x11=1
  kill "$xvfb" 2>/dev/null || true
  # (cage, wlroots' kiosk: headless, with a seat, as gpui asks for one; it ends with the app.)
  step "nami-gui opens: on Wayland (cage, headless), drawn with lavapipe"
  run=$work/run; mkdir -p "$run"; chmod 700 "$run"
  ok_wayland=; opens "Wayland (cage)" env -u DISPLAY XDG_RUNTIME_DIR="$run" WLR_BACKENDS=headless WLR_LIBINPUT_NO_DEVICES=1 WLR_RENDERER=pixman \
    cage -- sh -c 'echo "on Wayland: $WAYLAND_DISPLAY"; unset DISPLAY; exec nami-gui --smoke-test' && ok_wayland=1
  [ -n "$ok_x11" ] || fail "nami-gui didn't open on X11"
  [ -n "$ok_wayland" ] || fail "nami-gui didn't open on Wayland"

  step "sudo nami remove nami-gui"
  sudo "$V/bin/nami" remove nami-gui
  ! gui_listed || fail "system.nami still lists nami-gui"
  gone nami-gui "after sudo nami remove nami-gui"

  step "sudo nami install nami-gui"
  sudo "$V/bin/nami" install nami-gui
  gui_listed || fail "system.nami doesn't list nami-gui again"
  [ "$(sudo grep -c nami-gui /etc/nami/system.nami)" = 1 ] || fail "system.nami lists nami-gui more than once"
  ours nami-gui
  [ "$(nami-gui --version)" = "nami-gui $version" ] || fail "nami-gui --version says $(nami-gui --version)"
  sudo "$V/bin/nami" install nami-gui 2>&1 | grep -q 'already installed' || fail "installing it again isn't \"already installed\""
fi

step "uninstall"
sudo sh "$V/uninstall.sh"
[ ! -e /nix/var/nami ] || fail "/nix/var/nami is still there"
[ ! -e "$V" ] || fail "$V is still there"
[ ! -e /usr/local/bin/nami ] || fail "/usr/local/bin/nami is still there"
! systemctl is-active nami-daemon.service >/dev/null 2>&1 || fail "the daemon is still running"
gone rg "after uninstalling"

printf '\nPASS: nami on %s (%s, %s KB pages)\n' "$system" "$(. /etc/os-release && echo "$PRETTY_NAME")" "$(( $(getconf PAGESIZE) / 1024 ))"
