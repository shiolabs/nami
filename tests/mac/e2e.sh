#!/bin/sh
# nami on a Mac, end to end, for real (on GitHub's macOS runners: .github/workflows/macos.yml;
# never on a Mac someone uses: it makes the /nix volume and takes it away again).
#
# A site like namios.dev, served here: a release with this nami's tarball for this Mac,
# signed, and the catalog in tests/mac (a handful of packages, all ready-built on
# cache.nixos.org), signed with a test key. Then what someone does:
#   curl -fsSL <site>/install | sh          the volume, the daemon, the shells, the catalog
#   nami install ripgrep jq                 as themselves, through the daemon
#   nami install alacritty itsycal          apps with a window, copied into ~/Applications/nami Apps
#                                           (sudo ... --system: /Applications/nami Apps), signed as
#                                           in the store, known to Launch Services, opened, removed,
#                                           undone; and the uninstaller takes them away
#   nami install cask:rectangle             an app from Homebrew's casks (the live API): found by
#                                           search and info, downloaded, checked, in nami Apps
#                                           quarantined and signed, removed and undone offline;
#                                           cask:google-chrome (no checksum: notarized only); a
#                                           .pkg cask, a version and no such cask refused; one
#                                           Gatekeeper doesn't accept said; --system; user.nami
#                                           with cask:maccy and a plain `rectangle`
#   a project: nami shell -c 'node --version', nami run <task>, nami env
#   nami undo, user.nami: apply / history / diff / undo
#   the daily update                        launchd's dev.namios.update: sudo nami os-update, the
#                                           job kickstarted, `updates: manual` takes it away; casks
#                                           served here: one at its newer version, as the person,
#                                           not one that updates itself, nor one open till it's quit
#   sudo nami self-uninstall --yes         and nothing of it left (after what mustn't uninstall:
#                                           an app's name, no sudo, an answer but `uninstall`);
#                                           then installed again, and curl .../uninstall | sh -s -- --yes
# With nami.app (gui/mac/make-app.sh) in the release too: the installer puts it in
# /Applications, it opens as Finder opens it and finds nami, and the uninstaller takes it away.
# usage: tests/mac/e2e.sh <nami binary> [nami.app]
set -eu
here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../.." && pwd)
bin=$(cd "$(dirname "$1")" && pwd)/$(basename "$1")
app=${2:+$(cd "$(dirname "${2:-.}")" && pwd)/$(basename "${2:-.}")}
[ "$(uname -s)" = Darwin ] || { echo "this is for a Mac"; exit 1; }
case $(uname -m) in arm64) system=aarch64-darwin ;; x86_64) system=x86_64-darwin ;; esac
version=$(sed -n 's/^version = "\(.*\)"/\1/p' "$repo/Cargo.toml" | head -1)
work=$(mktemp -d /tmp/nami-e2e.XXXXXX)
site=$work/site port=8765 base=http://127.0.0.1:8765
step() { printf '\n=== %s\n' "$*"; }
fail() { printf '\nFAIL: %s\n' "$*"; exit 1; }
# A program nami installed is found first (not Homebrew's, which GitHub's Macs have plenty of).
ours() { hash -r; p=$(command -v "$1") || fail "no $1"; case $p in */.local/state/nami/*|/nix/*) echo "$1: $p" ;; *) fail "$1 is $p, not nami's" ;; esac; }
gone() { hash -r; p=$(command -v "$1" 2>/dev/null) || return 0; case $p in */.local/state/nami/*|/nix/*) fail "$1 still there ($2): $p" ;; esac; }

step "a test key, and the site"
# (Signing as root of a scratch store: nami keeps keys in its store's var folder.)
keys=$work/keys
pub=$(sudo env NAMI_ROOT="$keys" NAMI_NO_MIGRATE=1 "$bin" keygen test-1 2>/dev/null | tail -1)
case $pub in test-1:*) ;; *) fail "no test key: $pub" ;; esac
sign() { sudo env NAMI_ROOT="$keys" NAMI_NO_MIGRATE=1 "$bin" catalog sign "$1" --key test-1 >/dev/null; sudo chown "$(id -u)" "$1.sig"; }
rel=$site/releases/$version
mkdir -p "$rel" "$site/menu/test" "$work/stage/nami-$version"
cp "$bin" "$repo/install.sh" "$repo/uninstall.sh" "$repo/install-mac.sh" "$repo/uninstall-mac.sh" "$repo/packaging/setup.sh" "$work/stage/nami-$version/"
[ -z "$app" ] || ditto "$app" "$work/stage/nami-$version/nami.app"
COPYFILE_DISABLE=1 tar -C "$work/stage" -czf "$rel/nami-$version-$system.tar.gz" "nami-$version"
(cd "$rel" && shasum -a 256 "nami-$version-$system.tar.gz" > SHA256SUMS)
sign "$rel/SHA256SUMS"
echo "$version" > "$site/releases/latest"
cp "$here/index-$system.json" "$site/menu/test/index-$system.json"
xz -k "$site/menu/test/index-$system.json"
sign "$site/menu/test/index-$system.json.xz"
sign "$site/menu/test/index-$system.json"
rm "$site/menu/test/index-$system.json"
# Linux's channel names a release with no Mac catalog in it; the Mac's own channel file names the one
# that has it, as packaging/publish.sh makes them (SYSTEM=...): a Mac reads its own first.
echo linuxonly > "$site/menu/nixos-unstable"
echo test > "$site/menu/nixos-unstable.$system"
sed -e "s|@BASE@|$base|g" -e "s|@CHANNEL@|nixos-unstable|g" -e "s|@KEYS@|$pub|g" -e "s|@KEY@|$pub|g" "$repo/packaging/install-web.sh" > "$site/install"
sed -e "s|@BASE@|$base|g" "$repo/packaging/uninstall-web.sh" > "$site/uninstall"
(cd "$site" && exec python3 -m http.server "$port" --bind 127.0.0.1 >"$work/http.log" 2>&1) &
server=$!
# (What the script ends with stays its exit code: an error set -e or set -u stops it at isn't a pass.)
trap 'st=$?; kill $server 2>/dev/null || true; exit $st' EXIT
i=0; until curl -fsS "$base/releases/latest" >/dev/null 2>&1; do i=$((i + 1)); [ $i -lt 50 ] || fail "the test site didn't start"; sleep 0.2; done

step "curl -fsSL $base/install | sh"
curl -fsSL "$base/install" | sh
mount | grep ' on /nix (' || fail "/nix isn't a volume"
diskutil info /nix | grep -E 'Volume Name|File System Personality|Owners|Encrypted|FileVault' || true
grep '/nix apfs' /etc/fstab || fail "no /nix line in /etc/fstab"
grep -x nix /etc/synthetic.conf || fail "no nix in /etc/synthetic.conf"
sudo launchctl print system/dev.namios.daemon | grep -E 'state|pid' | head -3 || fail "the daemon isn't loaded"
[ -S /nix/var/nami/daemon.sock ] || fail "no daemon socket"
grep -q '>>> nami' /etc/zshrc || fail "no nami block in /etc/zshrc"
[ -f /usr/local/share/zsh/site-functions/_nami ] || fail "no zsh completion"
# The daily update: launchd's job, root's, a plist launchd can read, and loaded.
upd=/Library/LaunchDaemons/dev.namios.update.plist
job() { sudo launchctl print system/dev.namios.update > "$work/job.txt" 2>&1; }
[ -f "$upd" ] || fail "no $upd"
[ "$(stat -f '%Su:%Sg %Lp' "$upd")" = "root:wheel 644" ] || fail "$upd is $(stat -f '%Su:%Sg %Lp' "$upd"), not root:wheel 644"
plutil -lint "$upd" || fail "$upd isn't a plist launchd can read"
plutil -p "$upd"
job || { cat "$work/job.txt"; fail "launchd doesn't know dev.namios.update"; }
grep -E 'state|path|program|last exit' "$work/job.txt" | head -8

step "a new terminal finds nami (zsh, as macOS starts it)"
zsh -lic 'command -v nami && nami --version' || fail "a new zsh doesn't find nami"
bash -lc 'command -v nami' || fail "a login bash doesn't find nami"
# From here, as a new terminal has it.
. /nix/var/nami/nami.sh
command -v nami
nami --version

step "nami search, info, versions, list"
nami search ripgrep | head -5
nami info jq
nami versions jq || echo "(no histories in the test catalog)"
nami list

step "nami install ripgrep jq (as $(id -un), through the daemon)"
nami install ripgrep jq
hash -r
ours rg
ours jq
rg --version
jq --version
echo '{"a":[1,2]}' | jq -c '.a | length' | grep -qx 2 || fail "jq didn't work"
nami list | grep -E 'ripgrep|jq'

if [ -n "$app" ]; then
  step "nami's app: in Applications, and it opens as Finder opens it"
  [ "$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' /Applications/nami.app/Contents/Info.plist)" = dev.namios.nami ] || fail "no nami.app in /Applications"
  codesign --verify --strict --verbose=2 /Applications/nami.app || fail "nami.app's signature doesn't hold"
  ! xattr /Applications/nami.app | grep -q quarantine || fail "nami.app is quarantined"
  # (LaunchServices, with launchd's PATH: nami's found all the same. perl's alarm: no timeout.)
  perl -e 'alarm 120; exec @ARGV' open -W -n --stdout "$work/app.log" --stderr "$work/app.log" /Applications/nami.app --args --smoke-test || { cat "$work/app.log"; fail "nami.app didn't open"; }
  cat "$work/app.log"
  grep -q "nami answered: true" "$work/app.log" || fail "nami.app didn't find nami"
fi

step "apps with a window, in Applications: alacritty and itsycal in ~/Applications/nami Apps"
# alacritty is built by nixpkgs (signed ad hoc, as the linker signs it; its bundle's MacOS is a
# link out of it, to the package's bin/); itsycal is its developer's own build, signed and sealed.
apps="$HOME/Applications/nami Apps"
LSREGISTER=/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister
# (Its source in the store: what .nami-apps says it was copied from.)
source_of() { awk -F'\t' -v n="$1.app" '$1 == n { print $2 }' "$2/.nami-apps"; }
# A copy as it should be: a bundle of its own, nothing leading into /nix, not quarantined, and
# signed as well as the store's is.
whole() {
  copy=$2/$1.app src=$(source_of "$1" "$2")
  [ -n "$src" ] || fail "$2/.nami-apps doesn't have $1"
  [ -f "$copy/Contents/Info.plist" ] || fail "no $1.app in $2"
  ! find "$copy" -type l -lname '/nix/*' | grep -q . || fail "$1.app has links into /nix"
  ! xattr -r "$copy" 2>/dev/null | grep -q com.apple.quarantine || fail "$1.app is quarantined"
  name=$(/usr/libexec/PlistBuddy -c 'Print :CFBundleExecutable' "$copy/Contents/Info.plist")
  exe=$copy/Contents/MacOS/$name
  [ -f "$exe" ] && [ -x "$exe" ] || fail "$1.app's program isn't in it: $exe"
  cmp "$exe" "$src/Contents/MacOS/$name" || fail "$1's program isn't the store's, byte for byte"
  if codesign --verify --deep --strict "$src" 2>/dev/null; then
    codesign --verify --deep --strict --verbose=2 "$copy" || fail "$1.app's signature holds in the store, not in the copy"
  else
    # (nixpkgs' own builds: the bundle isn't sealed. On Apple silicon the linker signs the
    # program, and that signature, as the program alone, holds; Intel's are unsigned, as
    # Intel Macs allow.)
    echo "($1.app in the store isn't sealed as a bundle: $(codesign --verify --deep --strict "$src" 2>&1 | grep -v '^In architecture' | tail -1))"
    cp "$exe" "$work/exe"
    if [ "$(uname -m)" = arm64 ]; then
      codesign --verify --verbose=2 "$work/exe" || fail "$1's program isn't signed in the copy"
    else
      codesign --verify --verbose=2 "$work/exe" 2>&1 | head -1 || true
    fi
    rm -f "$work/exe"
  fi
  codesign -dvv "$copy" 2>&1 | grep -E '^(Identifier|Signature|Authority|TeamIdentifier)=' || true
}
# Applications as the person's packages are: each app there whole while its package is
# installed, and not there once it isn't.
in_step() {
  for pa in alacritty:Alacritty itsycal:Itsycal; do
    if nami list 2>/dev/null | grep -qw "${pa%%:*}"; then
      whole "${pa#*:}" "$apps"
    else
      [ ! -e "$apps/${pa#*:}.app" ] || fail "${pa#*:}.app is in $apps, ${pa%%:*} isn't installed"
    fi
  done
}
/usr/bin/time -p nami install alacritty itsycal 2>&1 | tee "$work/apps.log"
grep -qx "Added Alacritty to Applications." "$work/apps.log" || fail "nami didn't say it added Alacritty"
grep -qx "Added Itsycal to Applications." "$work/apps.log" || fail "nami didn't say it added Itsycal"
ls -la "$apps"
cat "$apps/.nami-apps"
du -sh "$apps"/*.app
whole Alacritty "$apps"
whole Itsycal "$apps"
[ -d "$apps/Alacritty.app/Contents/MacOS" ] && [ ! -L "$apps/Alacritty.app/Contents/MacOS" ] || fail "Alacritty.app's MacOS isn't a folder of its own"
# Launch Services knows them (Spotlight, Open With, the Dock): nami told it.
"$LSREGISTER" -dump > "$work/ls.txt" 2>/dev/null || true
grep -F "$apps/Alacritty.app" "$work/ls.txt" | head -2
grep -qF "$apps/Alacritty.app" "$work/ls.txt" || fail "Launch Services doesn't know Alacritty.app"
grep -qF "$apps/Itsycal.app" "$work/ls.txt" || fail "Launch Services doesn't know Itsycal.app"
# Spotlight (GitHub's Macs may have it off: said, not checked).
mdutil -s / 2>&1 | tail -1 || true
echo "Spotlight finds: $(mdfind -onlyin "$apps" 'kMDItemContentType == "com.apple.application-bundle"' 2>/dev/null | tr '\n' ' ')"
# Opened as Finder and the Dock open it: the copy runs.
open -n "$apps/Alacritty.app" || fail "Alacritty.app didn't open"
pid=
i=0; while [ $i -lt 30 ]; do
  pid=$(ps -axo pid=,comm= | awk -v p="$apps/Alacritty.app/" 'index($0, p) { print $1; exit }')
  [ -z "$pid" ] || break
  i=$((i + 1)); sleep 0.5
done
[ -n "$pid" ] && echo "Alacritty runs from the copy (pid $pid)" || echo "(Alacritty opened, but no window stays on this Mac: no process from the copy)"
# Removed while it's open: gone from Applications; then undo brings it back.
nami remove alacritty 2>&1 | tee "$work/apps.log"
grep -qx "Removed Alacritty from Applications." "$work/apps.log" || fail "nami didn't say it removed Alacritty"
[ ! -e "$apps/Alacritty.app" ] || fail "Alacritty.app is still in $apps"
[ -d "$apps/Itsycal.app" ] || fail "Itsycal.app went with Alacritty"
[ -z "$pid" ] || kill "$pid" 2>/dev/null || true
"$LSREGISTER" -dump 2>/dev/null | grep -qF "$apps/Alacritty.app" && echo "(Launch Services still lists Alacritty.app)"
nami undo
whole Alacritty "$apps"
# Something of the person's own in the folder stays, whatever nami does.
echo mine > "$apps/my notes.txt"
# Taken out by hand: it comes back at the next change, or with nothing to change.
rm -rf "$apps/Itsycal.app"
nami install itsycal
whole Itsycal "$apps"

step "everyone's apps: sudo nami install --system, in /Applications/nami Apps"
sudo /nix/var/nami/bin/nami install --system itsycal 2>&1 | tee "$work/apps.log"
grep -qx "Added Itsycal to Applications." "$work/apps.log" || fail "nami didn't say it added Itsycal for everyone"
ls -la "/Applications/nami Apps"
whole Itsycal "/Applications/nami Apps"
[ "$(stat -f %Su "/Applications/nami Apps/Itsycal.app")" = root ] || fail "everyone's Itsycal isn't root's"
sudo /nix/var/nami/bin/nami remove --system itsycal
[ ! -e "/Applications/nami Apps" ] || fail "/Applications/nami Apps is still there with nothing in it"
sudo /nix/var/nami/bin/nami undo
whole Itsycal "/Applications/nami Apps"

step "apps from Homebrew's casks: nami search and info find them"
# (The live list, as people get it: formulae.brew.sh. No version is checked: only what casks are.)
echo "Gatekeeper: $(spctl --status 2>&1)"
nami search rectangle | tee "$work/casks.log"
grep -q 'cask:rectangle' "$work/casks.log" || fail "nami search doesn't find Homebrew's Rectangle"
grep -q "Homebrew's casks" "$work/casks.log" || fail "nami search doesn't say where Rectangle is from"
nami info cask:rectangle | tee "$work/casks.log"
grep -q "from Homebrew's casks" "$work/casks.log" || fail "nami info doesn't say Rectangle is from Homebrew's casks"
nami search --json chrome | grep -q '"attr":"cask:google-chrome"' || fail "nami search --json doesn't find Homebrew's Google Chrome"

step "a cask's app: nami install cask:rectangle, in ~/Applications/nami Apps, quarantined, signed"
# A copy of a cask's app as it should be: nami's (.nami-apps says which cask), a bundle of its
# own, its program the store's byte for byte, its developer's signature whole, marked as
# downloaded (macOS asks the first time, as for any download), and Gatekeeper's verdict said.
cask_whole() {
  copy=$2/$1.app src=$(source_of "$1" "$2")
  [ -n "$src" ] || fail "$2/.nami-apps doesn't have $1"
  awk -F'\t' -v n="$1.app" -v c="cask:$3" '$1 == n && $3 == c { found = 1 } END { exit !found }' "$2/.nami-apps" || fail "$2/.nami-apps doesn't say $1 is cask:$3"
  case $src in /nix/store/*-cask-"$3"-*/Applications/"$1".app) ;; *) fail "$1's store path isn't a cask's: $src" ;; esac
  # (The store path links to the app as nami keeps it, root's, on the Mac's own volume.)
  kept=$(readlink "$src")
  case $kept in "/Library/Application Support/nami/casks/$3-"*/"$1.app") ;; *) fail "$1's store path doesn't lead to nami's casks folder: $kept" ;; esac
  [ "$(stat -f %Su "$kept")" = root ] || fail "$kept isn't root's"
  [ "$(stat -f %d "$kept")" = "$(stat -f %d "$2")" ] && echo "$1.app: kept on the same volume as $2 (a clone)" || echo "$1.app: kept on another volume than $2 (a copy)"
  [ -f "$copy/Contents/Info.plist" ] || fail "no $1.app in $2"
  ! find "$copy" -type l -lname '/nix/*' | grep -q . || fail "$1.app has links into /nix"
  q=$(xattr -p com.apple.quarantine "$copy" 2>/dev/null) || fail "$1.app isn't quarantined"
  case $q in [0-9a-f][0-9a-f][0-9a-f][0-9a-f]\;*\;nami\;*) echo "$1.app: com.apple.quarantine $q" ;; *) fail "$1.app's quarantine isn't nami's: $q" ;; esac
  ! xattr "$kept" | grep -q com.apple.quarantine || fail "the kept $1.app is quarantined (only its copy in nami Apps is)"
  echo "$1.app's extended attributes: kept: $(xattr "$kept" | tr '\n' ' '); in $2: $(xattr "$copy" | tr '\n' ' ')"
  name=$(/usr/libexec/PlistBuddy -c 'Print :CFBundleExecutable' "$copy/Contents/Info.plist")
  cmp "$copy/Contents/MacOS/$name" "$kept/Contents/MacOS/$name" || fail "$1's program isn't the kept one's, byte for byte"
  codesign --verify --deep --strict --verbose=2 "$kept" || fail "$1.app's signature doesn't hold as nami keeps it"
  codesign --verify --deep --strict --verbose=2 "$copy" || fail "$1.app's signature doesn't hold in its copy"
  codesign -dvv "$copy" 2>&1 | grep -E '^(Identifier|Authority|TeamIdentifier|Notarization Ticket)=' || true
  spctl --assess --type execute -vv "$copy" || fail "Gatekeeper doesn't accept $1.app"
}
/usr/bin/time -p nami install cask:rectangle 2>&1 | tee "$work/casks.log"
grep -q "Downloading Rectangle " "$work/casks.log" || fail "nami didn't say it downloaded Rectangle"
grep -qx "Added Rectangle to Applications." "$work/casks.log" || fail "nami didn't say it added Rectangle"
# (The person's file: ~/user.nami, or ~/.config/nami/user.nami.)
uf=$HOME/user.nami; [ -f "$uf" ] || uf=$HOME/.config/nami/user.nami
grep -q "cask:rectangle" "$uf" || fail "$uf doesn't list cask:rectangle"
ls -la "$apps"
cat "$apps/.nami-apps"
du -sh "$apps/Rectangle.app"
cask_whole Rectangle "$apps" rectangle
# (As a download Finder moved into Applications: 0x0100 too, so macOS doesn't translocate it.)
xattr -p com.apple.quarantine "$apps/Rectangle.app" | grep -q '^0181;' || fail "Rectangle.app's quarantine flags aren't 0181"
"$LSREGISTER" -dump 2>/dev/null | grep -qF "$apps/Rectangle.app" || fail "Launch Services doesn't know Rectangle.app"
nami list | tee "$work/casks.log"
grep -q 'cask:rectangle' "$work/casks.log" || fail "nami list doesn't have cask:rectangle"
nami history | head -8 | tee "$work/casks.log"
grep -q 'rectangle' "$work/casks.log" || fail "nami history doesn't have Rectangle"
# nami's window with Rectangle installed (Discover: Popular Apps from Homebrew's casks), as
# pictures for the job's artifacts (NAMI_SHOTS), before anything else is on the screen (macOS's
# question below stays: nobody answers it here). Said, not checked.
if [ -n "$app" ] && [ -n "${NAMI_SHOTS:-}" ]; then
  mkdir -p "$NAMI_SHOTS"
  open -n /Applications/nami.app || true
  sleep 20
  screencapture -x "$NAMI_SHOTS/discover-20s-$system.png" 2>&1 || echo "(no screenshot)"
  sleep 25
  screencapture -x "$NAMI_SHOTS/discover-45s-$system.png" 2>&1 || echo "(no screenshot)"
  gui=$(ps -axo pid=,comm= | awk '/nami.app\/Contents\/MacOS\/nami-gui/ { print $1; exit }')
  [ -z "$gui" ] || kill "$gui" 2>/dev/null || true
  # nami's app asking first, Uninstall nami?, light and dark, as Uninstall nami… opens it
  # (`--page uninstall-nami`: nothing's pressed, nothing changes); and a cask's page, as
  # Discover opens it (Google Chrome, not installed yet).
  shot() {
    NAMI_THEME=$2 /Applications/nami.app/Contents/MacOS/nami-gui --page "$1" > "$work/shot.log" 2>&1 &
    pid=$!
    sleep 15
    screencapture -x "$NAMI_SHOTS/$3.png" || echo "(no picture: screencapture failed)"
    kill "$pid" 2>/dev/null || true
    wait "$pid" 2>/dev/null || true
    cat "$work/shot.log"
  }
  shot uninstall-nami light "uninstall-nami-$system-light"
  shot uninstall-nami dark "uninstall-nami-$system-dark"
  shot app:cask:google-chrome light "cask-page-$system"
  # Installed's Storage, with older changes to clean up, and its Clean Up asking first
  # (`--page storage-clean-up`: nothing's pressed, nothing changes).
  shot storage light "storage-$system-light"
  shot storage dark "storage-$system-dark"
  shot storage-clean-up light "storage-clean-up-$system-light"
  shot storage-clean-up dark "storage-clean-up-$system-dark"
  ls -la "$NAMI_SHOTS"
fi
# Opened as Finder opens it: quarantined, macOS asks first (on a Mac with nobody at it, the
# question waits, and `open` with it: in the background, ended after). What happens is said,
# and pictured (NAMI_SHOTS), not checked.
open -g -j "$apps/Rectangle.app" > "$work/open.log" 2>&1 &
opener=$!
sleep 10
pid=$(ps -axo pid=,comm= | awk '/Rectangle.app\/Contents\/MacOS\/Rectangle/ { print $1; exit }')
if kill -0 "$opener" 2>/dev/null; then echo "(open is still waiting after 10 s: macOS is asking first)"; fi
[ -z "${NAMI_SHOTS:-}" ] || { mkdir -p "$NAMI_SHOTS"; screencapture -x "$NAMI_SHOTS/first-open-$system.png" 2>&1 || true; }
if [ -n "$pid" ]; then
  echo "Rectangle's process: $(ps -o stat=,comm= -p "$pid")"
  if ps -o comm= -p "$pid" | grep -q AppTranslocation; then echo "(macOS runs it translocated, from a read-only copy elsewhere)"; fi
  kill "$pid" 2>/dev/null || true
else
  echo "(Rectangle isn't running)"
fi
kill "$opener" 2>/dev/null || true
cat "$work/open.log"
ps -axo comm= | grep -E 'CoreServicesUIAgent|UserNotificationCenter' | head -2 || true
xattr -p com.apple.quarantine "$apps/Rectangle.app" || true
# Removed, and back with undo: from the store, without downloading it again.
nami remove cask:rectangle 2>&1 | tee "$work/casks.log"
grep -qx "Removed Rectangle from Applications." "$work/casks.log" || fail "nami didn't say it removed Rectangle"
[ ! -e "$apps/Rectangle.app" ] || fail "Rectangle.app is still in $apps"
nami undo 2>&1 | tee "$work/casks.log"
! grep -q "Downloading" "$work/casks.log" || fail "undo downloaded Rectangle again"
cask_whole Rectangle "$apps" rectangle

step "a cask with no checksum: cask:google-chrome, only as notarized by Apple; one app's space for both"
# The disk before and after: what each volume uses (the Mac's own: the kept app and its clone in
# nami Apps; /nix: the store path, links), and the free space they share (APFS: one container).
# The download and what it unpacked to are gone by then.
used() { df -k "$1" | awk 'NR == 2 { print $3 }'; }
free_kb() { df -k /System/Volumes/Data | awk 'NR == 2 { print $4 }'; }
data0=$(used /System/Volumes/Data) nix0=$(used /nix) free0=$(free_kb)
/usr/bin/time -p nami install cask:google-chrome 2>&1 | tee "$work/casks.log" || fail "nami didn't install Google Chrome"
sync
data1=$(used /System/Volumes/Data) nix1=$(used /nix) free1=$(free_kb)
grep -qx "Added Google Chrome to Applications." "$work/casks.log" || fail "nami didn't say it added Google Chrome"
cask_whole "Google Chrome" "$apps" google-chrome
spctl --assess --type execute -vv "$apps/Google Chrome.app" 2>&1 | grep -E 'source=' || true
app_kb=$(du -sk "$kept" | cut -f1)
echo "Google Chrome.app: $((app_kb / 1024)) MB as du counts it (kept, and as much again in nami Apps)"
echo "disk: the Mac's own volume uses $(((data1 - data0) / 1024)) MB more, /nix $(((nix1 - nix0) / 1024)) MB more; free space $(((free0 - free1) / 1024)) MB less"
[ $((free0 - free1)) -lt $((app_kb * 3 / 2)) ] || fail "Google Chrome took more than one app's space: $(((free0 - free1) / 1024)) MB less free for a $((app_kb / 1024)) MB app"
[ $((nix1 - nix0)) -lt $((app_kb / 4)) ] || fail "Google Chrome took $(((nix1 - nix0) / 1024)) MB on /nix"
du -sh "$apps/Google Chrome.app"
nami remove cask:google-chrome
[ ! -e "$apps/Google Chrome.app" ] || fail "Google Chrome.app is still in $apps"

step "what nami doesn't install yet is refused, and nothing changes"
cp "$uf" "$work/user.before"
! nami install cask:zoom > "$work/casks.log" 2>&1 || fail "nami installed Zoom (a .pkg)"
cat "$work/casks.log"
grep -q "Apple's installer (a .pkg)" "$work/casks.log" || fail "no plain message for Zoom's .pkg"
grep -q "nami installs Homebrew's casks that are an app and nothing more" "$work/casks.log" || fail "the message doesn't say what nami installs"
! nami install cask:rectangle@0.80 > "$work/casks.log" 2>&1 || fail "nami installed a version of a cask"
cat "$work/casks.log"
grep -q "only the newest version" "$work/casks.log" || fail "no plain message for a cask's version"
! nami install cask:no-such-cask-here > "$work/casks.log" 2>&1 || fail "nami installed a cask that isn't there"
cat "$work/casks.log"
grep -q "isn't one of Homebrew's casks" "$work/casks.log" || fail "no plain message for a cask that isn't there"
cmp "$uf" "$work/user.before" || fail "a refused cask changed user.nami"

step "a downloaded app Gatekeeper doesn't accept, from a cask with a checksum: installed, and said"
if [ -n "$app" ]; then
  # (nami.app is signed ad hoc, not notarized: a cask of it, served here.)
  mkdir -p "$site/api/cask" "$work/fakecache"
  (cd "$(dirname "$app")" && ditto -c -k --keepParent "$(basename "$app")" "$site/nami-test.zip")
  sum=$(shasum -a 256 "$site/nami-test.zip" | cut -d' ' -f1)
  printf '{"token":"nami-test","name":["nami Test"],"desc":"A test","homepage":"%s","url":"%s/nami-test.zip","version":"1.0","sha256":"%s","artifacts":[{"app":["nami.app",{"target":"nami Test.app"}]}],"depends_on":{},"auto_updates":false}\n' "$base" "$base" "$sum" > "$site/api/cask/nami-test.json"
  { printf '['; cat "$site/api/cask/nami-test.json"; printf ']'; } > "$site/api/cask.json"
  XDG_CACHE_HOME=$work/fakecache NAMI_CASKS_API=$base/api nami install cask:nami-test 2>&1 | tee "$work/casks.log" || fail "nami didn't install the test cask"
  grep -q "Gatekeeper doesn't accept nami Test" "$work/casks.log" || fail "nami didn't say Gatekeeper doesn't accept it"
  xattr -p com.apple.quarantine "$apps/nami Test.app" || fail "nami Test.app isn't quarantined"
  ! spctl --assess --type execute "$apps/nami Test.app" 2>/dev/null || echo "(Gatekeeper accepts it here: assessments are off)"
  XDG_CACHE_HOME=$work/fakecache NAMI_CASKS_API=$base/api nami remove cask:nami-test
  [ ! -e "$apps/nami Test.app" ] || fail "nami Test.app is still in $apps"
fi

step "everyone's cask: sudo nami install --system cask:maccy, in /Applications/nami Apps"
sudo /nix/var/nami/bin/nami install --system cask:maccy 2>&1 | tee "$work/casks.log"
grep -qx "Added Maccy to Applications." "$work/casks.log" || fail "nami didn't say it added Maccy for everyone"
cask_whole Maccy "/Applications/nami Apps" maccy
[ "$(stat -f %Su "/Applications/nami Apps/Maccy.app")" = root ] || fail "everyone's Maccy isn't root's"
sudo /nix/var/nami/bin/nami remove --system cask:maccy
[ ! -e "/Applications/nami Apps/Maccy.app" ] || fail "Maccy.app is still in /Applications/nami Apps"

step "nami update --refresh, nami upgrade (as $(id -un))"
nami update --refresh 2>&1 | tee "$work/update.log"
grep -q "Homebrew's casks refreshed" "$work/update.log" || fail "nami update --refresh didn't refresh Homebrew's casks"
nami upgrade --dry-run
nami upgrade 2>&1 | tee "$work/upgrade.log"
rg --version >/dev/null || fail "ripgrep went in the upgrade"
cask_whole Rectangle "$apps" rectangle

step "tab completion"
nami __complete install rip | head -5 || true
zsh -ic 'autoload -Uz compinit && compinit -u && whence -v _nami' || fail "zsh doesn't find _nami"

step "history, diff, undo, remove"
nami install fd
ours fd
fd --version
nami history
nami diff
nami undo
hash -r
gone fd "after undo"
nami history
nami remove jq
hash -r
gone jq "after remove"
in_step
rg --version >/dev/null || fail "ripgrep went with jq"
# Undone, the remove made after an undo goes back to what it was made from: jq back, and fd,
# undone before it, not (undo isn't "the change numbered before"). Then jq goes again.
nami undo
hash -r
ours jq
gone fd "after undoing the remove made after fd's undo"
in_step
nami remove jq
gone jq "after remove"

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
nami shell -c 'node --version'
nami shell -c 'node --version' | grep -q '^v' || fail "no node in the project's shell"
nami run hi | grep -qx 'hello from nami' || fail "the task didn't run"
nami run -- node -e 'console.log(1+1)' | grep -qx 2 || fail "nami run -- node failed"
nami env | grep -q 'GREETING' || fail "nami env has no GREETING"
[ -f nami.lock ] && grep -q "\"system\": \"$system\"" nami.lock || fail "no lock for $system"
nami hook zsh | grep -q add-zsh-hook || fail "no zsh hook"
command -v node >/dev/null 2>&1 && [ "$(command -v node)" != "${proj}" ] && echo "(node outside the project: $(command -v node))"
cd "$work"

step "a project to build in: go turns on nixpkgs' build environment (stdenv, its clang, nixpkgs' bash)"
gproj=$work/gotool
mkdir -p "$gproj"
printf 'packages:\n  - go\n' > "$gproj/project.nami"
cat > "$gproj/main.go" <<'EOF'
package main

import "fmt"

func main() { fmt.Println("built with nami") }
EOF
printf 'module example.com/hello\n\ngo 1.22\n' > "$gproj/go.mod"
# (In a folder of its own: go refuses C files beside a package's Go files without cgo.)
mkdir -p "$gproj/c"
printf '#include <stdio.h>\nint main(void) { puts("built with nami'"'"'s clang"); return 0; }\n' > "$gproj/c/hi.c"
cd "$gproj"
nami shell -c 'go version && echo "stdenv: $stdenv" && command -v cc && bash --version | head -1'
nami shell -c 'echo $stdenv' | grep -q stdenv-darwin || fail "no stdenv in the build environment"
nami run -- go build -o hello . || fail "go build failed"
./hello | grep -qx 'built with nami' || fail "the program go built doesn't run"
# C, with nixpkgs' clang and macOS's SDK from the cache.
nami run -- cc -o hi c/hi.c || fail "cc failed"

./hi | grep -q "built with nami's clang" || fail "the program cc built doesn't run"
otool -L hi
nami env | grep -q '^export stdenv=' || fail "nami env has no stdenv"
cd "$work"

step "what a copy costs: /nix is a volume of its own, so APFS can't clone into Applications"
# (go's store path, a few hundred MB: as a big app would be. Said, not checked.)
big=$(ls -d /nix/store/*-go-1.* 2>/dev/null | grep -v '\.drv$' | head -1)
if [ -n "$big" ]; then
  echo "$big: $(du -sh "$big" | cut -f1), $(find "$big" -type f | wc -l | tr -d ' ') files"
  # (Copies of the store are read-only: writable first, to remove them.)
  drop() { ${2:-} chmod -R u+w "$1" 2>/dev/null || true; ${2:-} rm -rf "$1"; }
  echo "copied (ditto):"
  /usr/bin/time -p ditto "$big" "$HOME/Applications/.nami-measure" 2>&1 | tail -3
  drop "$HOME/Applications/.nami-measure"
  echo "cloned (cp -c):"
  /usr/bin/time -p cp -c -R "$big" "$HOME/Applications/.nami-measure" 2>&1 | tail -4 || true
  drop "$HOME/Applications/.nami-measure"
  echo "cloned within /nix (cp -c):"
  /usr/bin/time -p sudo cp -c -R "$big" /nix/var/nami-measure 2>&1 | tail -3 || true
  drop /nix/var/nami-measure sudo
fi

step "user.nami: apply, history, diff, undo"
cat > "$HOME/user.nami" <<'EOF'
packages:
  - htop
  - ripgrep
  - cask:maccy
  - rectangle
env: { NAMI_E2E: "yes" }
aliases: { ll: ls -la }
settings:
  org.gnome.desktop.interface:
    color-scheme: prefer-dark
EOF
nami check "$HOME/user.nami" 2>&1 | tee "$work/check.log" || true
grep -q "rectangle: Rectangle from Homebrew's casks" "$work/check.log" || fail "nami check doesn't say rectangle is Homebrew's cask"
nami apply 2>&1 | tee "$work/apply.log"
grep -q "Linux's, not on a Mac" "$work/apply.log" || fail "settings: wasn't said to be left out"
# (A plain name the Mac's catalog hasn't got: the cask of that name, as nami check said; nami.lock
# has it already, as cask:rectangle, so it isn't downloaded again. Maccy, new for this person, is.)
! grep -q "Downloading Rectangle" "$work/apply.log" || fail "apply downloaded Rectangle again (nami.lock has it, as cask:rectangle)"
grep -q "Downloading Maccy" "$work/apply.log" || fail "apply didn't say it downloaded Maccy"
# (Its packages: htop, ripgrep and two casks' apps; the others are taken out of Applications, the
# person's own file stays.)
[ ! -e "$apps/Alacritty.app" ] && [ ! -e "$apps/Itsycal.app" ] || fail "apply left apps in $apps"
[ -f "$apps/my notes.txt" ] || fail "apply took the person's own file from $apps"
cask_whole Maccy "$apps" maccy
cask_whole Rectangle "$apps" rectangle
ours htop
htop --version
grep -q NAMI_E2E "$HOME/.config/nami/env.sh" || fail "env: isn't in ~/.config/nami/env.sh"
zsh -ic 'echo "$NAMI_E2E"' | grep -qx yes || fail "a new zsh doesn't have env:"
nami history
nami diff
# Undone: back to what apply was made from, the packages before it, and Applications with
# them: Alacritty, Itsycal and Rectangle back as they were, Maccy gone.
nami undo
hash -r
gone htop "after undo"
gone jq "after undo"
in_step
whole Alacritty "$apps"
whole Itsycal "$apps"
cask_whole Rectangle "$apps" rectangle
[ ! -e "$apps/Maccy.app" ] || fail "Maccy.app is still in $apps after undo"

step "nami storage: the disk (the APFS container), what nami takes, what cleaning up frees"
nami storage
t0=$(python3 -c 'import time; print(time.time())')
nami storage --json > "$work/storage.json"
echo "nami storage --json took $(python3 -c "import time; print(round(time.time() - $t0, 2))") s ($(ls /nix/var/nami/db | wc -l | tr -d ' ') store paths)"
python3 - "$work/storage.json" <<'EOF' || fail "nami storage --json isn't as it should be"
import json, sys
r = json.load(open(sys.argv[1]))
print(json.dumps(r["disk"]), json.dumps(r["cleanup"]))
for p in r["parts"]:
    print(f'  {p["part"]:9} {p["bytes"]:>12} {p["paths"]:>6}')
for a in r["largest"]:
    print(f'  {a["attr"]:24} {a["adds"]:>12} {a["whose"]}')
assert r["disk"]["container"] and 0 < r["disk"]["free"] < r["disk"]["size"], r["disk"]
assert sum(p["bytes"] for p in r["parts"]) == r["total"] > 0
assert any(p["part"] == "casks" and p["bytes"] > 0 for p in r["parts"]), "no casks' apps"
EOF
df -k /nix
diskutil info /nix | grep -iE 'container|volume used|free space' || true
nami gc --now --dry-run --json

step "nami gc: a cask's kept app goes once no change has it, and stays while one does"
casks="/Library/Application Support/nami/casks"
ls -la "$casks"
chrome=$(ls -d "$casks/google-chrome-"* 2>/dev/null | head -1)
[ -n "$chrome" ] || fail "Google Chrome isn't kept before gc (earlier changes have it)"
rect=$(readlink "$(source_of Rectangle "$apps")")
# (Only the newest change of this person's: history: 1. A kept app new in the last hour is
# spared, as an install may not have its store path yet: Chrome's made older by hand.)
printf 'history: 1\npackages:\n  - cask:rectangle\n' > "$HOME/user.nami"
nami apply 2>&1 | tail -4
sudo touch -t 202601010000 "$chrome"
# (What nothing uses goes from the store as an administrator says: sudo nami gc.)
sudo /nix/var/nami/bin/nami gc 2>&1 | tail -6
[ ! -e "$chrome" ] || fail "gc left Google Chrome's kept app, which no change has: $chrome"
[ -d "$rect" ] || fail "gc took Rectangle's kept app, which the change in use has: $rect"
cask_whole Rectangle "$apps" rectangle
rm -f "$HOME/user.nami"

step "Clean Up, as nami's app does it: nami gc --now --user, then as root nami gc --now"
nami install fd
nami remove fd
nami storage --json > "$work/before.json"
nami gc --now --user
sudo /nix/var/nami/bin/nami gc --now 2>&1 | tail -3
nami storage --json > "$work/after.json"
python3 - "$work/before.json" "$work/after.json" <<'EOF' || fail "Clean Up didn't free what nami storage said"
import json, sys
b, a = (json.load(open(f)) for f in sys.argv[1:])
print("said it frees", b["cleanup"]["bytes"], "· nami took", b["total"], "then", a["total"], "· now it frees", a["cleanup"]["bytes"])
assert a["total"] <= b["total"] - b["cleanup"]["bytes"] + 4096 * 1024, "less went than was said"
assert a["cleanup"]["bytes"] == 0 and not a["cleanup"]["changes"], a["cleanup"]
EOF
[ -z "$app" ] || [ -z "${NAMI_SHOTS:-}" ] || shot storage light "storage-cleaned-$system-light"

step "a package that isn't ready-built for Mac says so"
nami install no-such-package-here 2>&1 | tee "$work/missing.log" || true
grep -qi "catalog\|no package" "$work/missing.log" || fail "no clear error for a missing package"

step "nami self-update says it's the newest"
nami self-update --check

step "the daily update: sudo nami os-update, as root"
# (Nothing newer on the test site: nami stays, the catalog's refreshed, everyone's apps checked.)
rc=0; sudo /nix/var/nami/bin/nami os-update > "$work/os-update.log" 2>&1 || rc=$?
cat "$work/os-update.log"
[ "$rc" = 0 ] || fail "sudo nami os-update ended with $rc"
grep -q "is the newest version" "$work/os-update.log" || fail "os-update didn't look for a newer nami"
grep -q "^catalog: " "$work/os-update.log" || fail "os-update didn't refresh the catalog"
[ -f /nix/var/nami/updated ] || fail "os-update went well, and didn't note it (/nix/var/nami/updated)"

step "the daily update: launchd's job itself, kickstarted"
sudo rm -f /var/log/nami-update.log
sudo launchctl kickstart system/dev.namios.update || fail "launchctl kickstart system/dev.namios.update"
# (Until it's run and ended: ten minutes at most.)
i=0; while :; do
  job || fail "launchd doesn't know dev.namios.update"
  if ! grep -q 'state = running' "$work/job.txt" && ! grep -q 'last exit code = (never exited)' "$work/job.txt"; then break; fi
  i=$((i + 1)); [ $i -lt 600 ] || { cat "$work/job.txt"; fail "the daily update's job didn't end in ten minutes"; }
  sleep 1
done
grep -E 'state|runs|last exit' "$work/job.txt"
sudo cat /var/log/nami-update.log || fail "the job wrote no /var/log/nami-update.log"
grep -q 'last exit code = 0' "$work/job.txt" || fail "the daily update's job didn't go well"
sudo grep -q "is the newest version" /var/log/nami-update.log || fail "the job didn't run nami os-update"

step "the daily update: a cask's app at its newer version, as the person; not one that updates itself, nor one that's open"
# Three apps of casks served here (NAMI_CASKS_API), each at 1.0 then 2.0: Steady doesn't update
# itself, Selfish does (Homebrew's auto_updates), and Busy doesn't but is open.
capi=$base/api
mkdir -p "$site/api/cask" "$work/capps"
testapp() {
  d=$work/capps/$1-$2/$1.app
  mkdir -p "$d/Contents/MacOS"
  printf '#include <unistd.h>\nint main(void) { for (;;) pause(); }\n' | cc -x c -o "$d/Contents/MacOS/$1" -
  printf '<?xml version="1.0" encoding="UTF-8"?>\n<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">\n<plist version="1.0"><dict><key>CFBundleExecutable</key><string>%s</string><key>CFBundleIdentifier</key><string>dev.namios.test.%s</string><key>CFBundleName</key><string>%s</string><key>CFBundlePackageType</key><string>APPL</string><key>CFBundleShortVersionString</key><string>%s</string><key>CFBundleVersion</key><string>%s</string></dict></plist>\n' "$1" "$1" "$1" "$2" "$2" > "$d/Contents/Info.plist"
  codesign -s - --force "$d"
  (cd "$work/capps/$1-$2" && ditto -c -k --keepParent "$1.app" "$site/$1-$2.zip")
}
for v in 1.0 2.0; do for a in NamiSteady NamiSelfish NamiBusy; do testapp "$a" "$v"; done; done
# The casks at version $1, and Homebrew's list of them.
casks_at() {
  for a in NamiSteady NamiSelfish NamiBusy; do
    t=$(echo "$a" | tr '[:upper:]' '[:lower:]' | sed 's/^nami/nami-/')
    self=false; [ "$a" = NamiSelfish ] && self=true
    sum=$(shasum -a 256 "$site/$a-$1.zip" | cut -d' ' -f1)
    printf '{"token":"%s","name":["%s"],"desc":"A test","homepage":"%s","url":"%s/%s-%s.zip","version":"%s","sha256":"%s","artifacts":[{"app":["%s.app"]}],"depends_on":{},"auto_updates":%s}\n' "$t" "$a" "$base" "$base" "$a" "$1" "$1" "$sum" "$a" "$self" > "$site/api/cask/$t.json"
  done
  { printf '['; cat "$site/api/cask/nami-steady.json"; printf ','; cat "$site/api/cask/nami-selfish.json"; printf ','; cat "$site/api/cask/nami-busy.json"; printf ']'; } > "$site/api/cask.json"
}
version_of() { /usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$apps/$1.app/Contents/Info.plist"; }
at() { [ "$(version_of "$1")" = "$2" ] || fail "$1 is at $(version_of "$1"), not $2 ($3)"; }
casks_at 1.0
printf 'packages:\n  - cask:nami-steady\n  - cask:nami-selfish\n  - cask:nami-busy\n' > "$HOME/user.nami"
NAMI_CASKS_API=$capi nami update --refresh
NAMI_CASKS_API=$capi nami apply 2>&1 | tee "$work/casks.log"
for a in NamiSteady NamiSelfish NamiBusy; do at "$a" 1.0 "as installed"; done
"$apps/NamiBusy.app/Contents/MacOS/NamiBusy" & busy=$!
sleep 1
kill -0 "$busy" || fail "NamiBusy didn't stay open"
# Homebrew's casks have 2.0: the person's Updates has the two that don't update themselves.
casks_at 2.0
NAMI_CASKS_API=$capi nami update --refresh
NAMI_CASKS_API=$capi nami upgrade --dry-run --json | tee "$work/upd.json"
python3 -I -c '
import json, sys
ups = sorted((u["attr"], u["from"], u["to"]) for u in json.load(open(sys.argv[1])))
assert ups == [("cask:nami-busy", "1.0", "2.0"), ("cask:nami-steady", "1.0", "2.0")], ups' "$work/upd.json" || fail "nami upgrade --dry-run doesn't have the casks' updates"
# The daily update, as root: the person's own casks by nami run as them.
rc=0; sudo env NAMI_CASKS_API="$capi" /nix/var/nami/bin/nami os-update > "$work/os-update.log" 2>&1 || rc=$?
cat "$work/os-update.log"
[ "$rc" = 0 ] || fail "sudo nami os-update ended with $rc"
grep -q "$(id -un)'s apps from Homebrew's casks" "$work/os-update.log" || fail "os-update didn't see to the person's casks"
grep -q "NamiBusy is open, so it stays at 1.0 for now" "$work/os-update.log" || fail "os-update didn't say NamiBusy stays while it's open"
at NamiSteady 2.0 "after the daily update"
at NamiSelfish 1.0 "it updates itself"
at NamiBusy 1.0 "it's open"
kill -0 "$busy" || fail "the daily update quit NamiBusy"
grep -q '"cask:nami-steady-2.0"' "$HOME/.config/nami/nami.lock" || fail "nami.lock doesn't have NamiSteady 2.0"
[ "$(stat -f %Su "$apps/NamiSteady.app")" = "$(id -un)" ] || fail "NamiSteady's copy isn't the person's"
nami history | head -6
# Quit, the next day's update has it.
kill "$busy"; wait "$busy" 2>/dev/null || true
rc=0; sudo env NAMI_CASKS_API="$capi" /nix/var/nami/bin/nami os-update > "$work/os-update.log" 2>&1 || rc=$?
cat "$work/os-update.log"
[ "$rc" = 0 ] || fail "sudo nami os-update ended with $rc"
at NamiBusy 2.0 "once it was quit"
at NamiSelfish 1.0 "it updates itself"
NAMI_CASKS_API=$capi nami upgrade --dry-run | grep -q "Everything is up to date" || fail "updates left after the daily update"
# Again: nothing to do, and the person's nami isn't run.
sudo env NAMI_CASKS_API="$capi" /nix/var/nami/bin/nami os-update > "$work/os-update.log" 2>&1 || fail "the third os-update failed"
! grep -q "$(id -un)'s apps from Homebrew's casks" "$work/os-update.log" || fail "os-update ran the person's update with nothing newer"
# As they were: the apps gone, and Homebrew's own list again, the person's and root's.
NAMI_CASKS_API=$capi nami remove cask:nami-steady cask:nami-selfish cask:nami-busy
rm -f "$HOME/user.nami"
nami update --refresh
sudo /nix/var/nami/bin/nami update --refresh

step "updates: manual in system.nami, and sudo nami apply: no daily update"
# (system.nami as the Mac has it: written from everyone's apps when there's none yet.)
sudo /nix/var/nami/bin/nami apply
sudo cp /etc/nami/system.nami "$work/system.nami.was"
printf 'updates: manual\n' | sudo tee -a /etc/nami/system.nami >/dev/null
sudo /nix/var/nami/bin/nami apply
[ ! -e "$upd" ] || fail "updates: manual left $upd"
! job || { cat "$work/job.txt"; fail "updates: manual left dev.namios.update loaded"; }
sudo /nix/var/nami/bin/nami os-update --set-up | grep -q "off (updates: manual)" || fail "--set-up doesn't say updates are off"
[ ! -e "$upd" ] || fail "--set-up put $upd back with updates: manual"
# And on again: the line out, applied.
sudo cp "$work/system.nami.was" /etc/nami/system.nami
sudo /nix/var/nami/bin/nami apply
[ -f "$upd" ] || fail "without updates: manual, sudo nami apply didn't put $upd back"
job || { cat "$work/job.txt"; fail "dev.namios.update isn't loaded again"; }

step "what doesn't uninstall nami: an app's name, no sudo, any answer but the word"
# (Each said, then checked: `rc` is what it ended with.)
said() { rc=0; "$@" > "$work/said.log" 2>&1 < /dev/null || rc=$?; cat "$work/said.log"; }
still() { mount | grep -q ' on /nix (' && [ -x /nix/var/nami/bin/nami ] && [ -L /usr/local/bin/nami ] || fail "nami went: $1"; }
said nami self-uninstall firefox
[ "$rc" = 2 ] && grep -q 'nami self-uninstall removes nami itself. To remove firefox, run: nami remove firefox' "$work/said.log" || fail "nami self-uninstall firefox didn't point at nami remove (exit $rc)"
still "nami self-uninstall firefox"
said nami self-uninstall
[ "$rc" = 77 ] && [ "$(wc -l < "$work/said.log" | tr -d ' ')" = 1 ] && grep -q 'sudo nami self-uninstall' "$work/said.log" || fail "without sudo it didn't say sudo nami self-uninstall, in a line (exit $rc)"
still "without sudo"
rc=0; printf 'yes\n' | sudo nami self-uninstall > "$work/said.log" 2>&1 || rc=$?
cat "$work/said.log"
[ "$rc" = 1 ] && tail -1 "$work/said.log" | grep -q 'Nothing was removed.' || fail "'yes' typed didn't cancel (exit $rc)"
grep -q 'This uninstalls nami itself from this Mac, not a package' "$work/said.log" && grep -q 'the Nix Store volume, /nix' "$work/said.log" && grep -q 'your nami files: ~/user.nami and ~/.config/nami' "$work/said.log" || fail "the warning didn't say what goes and what stays"
still "'yes' typed"
said sh -c "curl -fsSL '$base/uninstall' | sh -s -- firefox"
[ "$rc" = 2 ] && grep -q 'To remove firefox, run: nami remove firefox' "$work/said.log" || fail "the web uninstaller took firefox for nami (exit $rc)"
# (No terminal here to type on, and no --yes: nothing.)
said sh -c "curl -fsSL '$base/uninstall' | sh"
grep -q 'not a package' "$work/said.log" && grep -q 'Nothing was removed.' "$work/said.log" || fail "the web uninstaller without a terminal or --yes"
still "the web uninstaller, unanswered"

step "sudo nami self-uninstall --yes"
sudo nami self-uninstall --yes
! mount | grep -q ' on /nix (' || fail "/nix is still mounted"
! diskutil info "Nix Store" >/dev/null 2>&1 || fail "the Nix Store volume is still there"
! grep -q '/nix apfs' /etc/fstab 2>/dev/null || fail "fstab still has /nix"
! grep -qx nix /etc/synthetic.conf 2>/dev/null || fail "synthetic.conf still has nix"
! grep -q '>>> nami' /etc/zshrc || fail "/etc/zshrc still has nami's block"
[ ! -e /Library/LaunchDaemons/dev.namios.daemon.plist ] || fail "the daemon's plist is still there"
[ ! -e /Library/LaunchDaemons/dev.namios.store.plist ] || fail "the store's plist is still there"
[ ! -e /usr/local/bin/nami ] || fail "/usr/local/bin/nami is still there"
[ ! -e /Applications/nami.app ] || fail "/Applications/nami.app is still there"
# The apps nami copied, everyone's and the person's; what's the person's own stays.
[ ! -e "/Applications/nami Apps" ] || fail "/Applications/nami Apps is still there"
[ ! -e "/Library/Application Support/nami" ] || fail "the casks' kept apps are still there: $(ls "/Library/Application Support/nami/casks" 2>/dev/null | tr '\n' ' ')"
[ "$(ls -A "$apps")" = "my notes.txt" ] || fail "$apps has more than the person's own file: $(ls -A "$apps" | tr '\n' ' ')"
! sudo launchctl print system/dev.namios.daemon >/dev/null 2>&1 || fail "the daemon is still loaded"
[ ! -e "$upd" ] && [ ! -e /var/log/nami-update.log ] || fail "the daily update's plist or log is still there"
! job || fail "the daily update is still loaded"
hash -r
! command -v nami >/dev/null 2>&1 || fail "nami is still on PATH: $(command -v nami)"

step "installed again, and the web uninstaller: curl -fsSL $base/uninstall | sh -s -- --yes"
curl -fsSL "$base/install" | sh -s -- --no-app
mount | grep -q ' on /nix (' || fail "installed again: /nix isn't a volume"
[ -f "$upd" ] && job || fail "installed again: no daily update"
curl -fsSL "$base/uninstall" | sh -s -- --yes
[ ! -e "$upd" ] && ! job || fail "the web uninstaller left the daily update"
! mount | grep -q ' on /nix (' || fail "/nix is still mounted after the web uninstaller"
! diskutil info "Nix Store" >/dev/null 2>&1 || fail "the Nix Store volume is still there after the web uninstaller"
[ ! -e /Library/LaunchDaemons/dev.namios.daemon.plist ] && [ ! -e /usr/local/bin/nami ] || fail "the web uninstaller left nami's daemon or command"
curl -fsSL "$base/uninstall" | sh | grep -q "nami isn't installed on this Mac" || fail "the web uninstaller, with nami gone, didn't say so"

printf '\nPASS: nami on %s (%s)\n' "$system" "$(sw_vers -productVersion)"
