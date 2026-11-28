#!/bin/sh
# The icons of Homebrew's casks, and the Mac App Store's screenshots of the apps it sells, on a
# Mac for real (GitHub's macOS runners; never a Mac someone uses: it runs nami as root of a
# scratch root, and installs a cask there).
#
# A site like namios.dev, served here: tests/mac's catalog with the set put in it as a Mac's
# catalog gets it (packaging/publish-cask-icons.py, from <set>: the folder of cask-icons.yml's
# artifact), signed with a test key. Then, in a scratch root (NAMI_ROOT):
#   nami update --from <site>        the catalog, and the icons that come with it (each its sha256)
#   nami info/search --json          casks with their icons, and screenshots where they have them;
#                                    an icon that comes as its app is shown, fetched and checked
#   nami info --screenshots          a cask's screenshots downloaded, each the catalog's sha256
#   nami install --system cask:<x>   a cask the set hasn't got (taken out of it here), for
#                                    everyone: its app's own icon (sips), root's and the person's
#   nami's app                       pictured into <shots>: Discover, and apps' pages
# usage: tests/mac/cask-icons.sh <nami> <nami.app> <set> <shots>
set -eu
here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../.." && pwd)
bin=$(cd "$(dirname "$1")" && pwd)/$(basename "$1")
app=$(cd "$(dirname "$2")" && pwd)/$(basename "$2")
set_dir=$(cd "$3" && pwd)
shots=$4
[ "$(uname -s)" = Darwin ] || { echo "this is for a Mac"; exit 1; }
case $(uname -m) in arm64) system=aarch64-darwin ;; x86_64) system=x86_64-darwin ;; esac
work=$(mktemp -d /tmp/nami-cask-icons.XXXXXX)
site=$work/site port=8766 base=http://127.0.0.1:8766 root=$work/root
mkdir -p "$shots" "$root" "$work/root-home"
step() { printf '\n=== %s\n' "$*"; }
fail() { printf '\nFAIL: %s\n' "$*"; exit 1; }
# nami as root of the scratch root (as `sudo nami` is), and as the person here.
n() { sudo env NAMI_ROOT="$root" NAMI_NO_MIGRATE=1 HOME="$work/root-home" "$bin" "$@"; }
mine() { env NAMI_ROOT="$root" NAMI_NO_MIGRATE=1 "$bin" "$@"; }
py() { python3 -I -c "$@"; }

step "a test key, and the site with the set in its catalog"
keys=$work/keys
pub=$(sudo env NAMI_ROOT="$keys" NAMI_NO_MIGRATE=1 "$bin" keygen test-1 2>/dev/null | tail -1)
case $pub in test-1:*) ;; *) fail "no test key: $pub" ;; esac
sign() { sudo env NAMI_ROOT="$keys" NAMI_NO_MIGRATE=1 "$bin" catalog sign "$1" --key test-1 >/dev/null; sudo chown "$(id -u)" "$1.sig"; }
menu=$site/menu/test
mkdir -p "$menu"
xz -c "$here/index-$system.json" > "$menu/index-$system.json.xz"
# The set less one cask ($outside), to install it and see its app's own icon.
outside=clocker
mkdir -p "$work/set"
cp "$set_dir/cask-screenshots.tar" "$work/set/" 2>/dev/null || true
py "
import io, json, sys, tarfile
src, dst, gone = sys.argv[1:4]
with tarfile.open(src) as t, tarfile.open(dst, 'w', format=tarfile.USTAR_FORMAT) as out:
    for m in t.getmembers():
        data = t.extractfile(m).read()
        if m.name == gone + '.png':
            continue
        if m.name == 'index.json':
            index = json.loads(data)
            index['icons'].pop(gone, None)
            data = json.dumps(index).encode()
        info = tarfile.TarInfo(m.name)
        info.size = len(data)
        out.addfile(info, io.BytesIO(data))
" "$set_dir/cask-icons.tar" "$work/set/cask-icons.tar" "$outside"
python3 -I "$repo/packaging/publish-cask-icons.py" "$menu/index-$system.json.xz" "$work/set" "$system"
sign "$menu/index-$system.json.xz"
xz -dc "$menu/index-$system.json.xz" > "$menu/index-$system.json"
sign "$menu/index-$system.json"
rm "$menu/index-$system.json"
echo linuxonly > "$site/menu/nixos-unstable"
echo test > "$site/menu/nixos-unstable.$system"
ls -la "$menu" && echo "$(ls "$site/menu/objects" | wc -l | tr -d ' ') objects"
(cd "$site" && exec python3 -m http.server "$port" --bind 127.0.0.1 >"$work/http.log" 2>&1) &
server=$!
trap 'st=$?; kill $server 2>/dev/null || true; exit $st' EXIT
i=0; until curl -fsS "$base/menu/nixos-unstable" >/dev/null 2>&1; do i=$((i + 1)); [ $i -lt 50 ] || fail "the test site didn't start"; sleep 0.2; done
# What the signed catalog names: token -> sha256, and the screenshots.
xz -dc "$menu/index-$system.json.xz" > "$work/catalog.json"
named() { py "import json, sys; print(json.load(open(sys.argv[1]))['nami:cask-icons'][sys.argv[2]].get(sys.argv[3], ''))" "$work/catalog.json" "$@" 2>/dev/null || true; }

step "nami update: the catalog, and the icons it names"
start=$(date +%s)
n update --from "$base/menu/nixos-unstable" --trust "$pub" 2>&1 | tee "$work/update.log"
echo "($(($(date +%s) - start)) s)"
var=$root/nami; [ -d "$var/icons" ] || var=$root/nix/var/nami
[ -d "$var/icons" ] || fail "no icons folder in $root"
have=$(ls "$var/icons" | grep -c '^cask:.*\.png$' || true)
want=$(py "import json, sys; print(len(json.load(open(sys.argv[1]))['nami:cask-icons']['cask_icons']))" "$work/catalog.json")
echo "$have of the $want icons the catalog names, $(du -sh "$var/icons" | cut -f1)"
[ "$have" -eq "$want" ] || fail "nami update kept $have icons, the catalog names $want"
# Each one is the sha256 the signed catalog says.
py "
import hashlib, json, os, sys
named = json.load(open(sys.argv[1]))['nami:cask-icons']['cask_icons']
bad = [t for t, sha in named.items() if hashlib.sha256(open(os.path.join(sys.argv[2], f'cask:{t}.png'), 'rb').read()).hexdigest() != sha]
sys.exit(f'not the catalog\'s: {bad}' if bad else 0)" "$work/catalog.json" "$var/icons" || fail "an icon isn't the catalog's"
# Again: nothing to fetch.
n update --refresh 2>&1 | tee "$work/update.log"

step "nami info and search: casks with their icons, and screenshots"
mine info --json cask:google-chrome cask:visual-studio-code cask:rectangle > "$work/info.json"
py "
import json, sys
for f in json.load(open(sys.argv[1])):
    print(f['attr'], f.get('icon'), len(f.get('screenshots') or []))
    assert f.get('icon') and f['icon'].endswith(f['attr'] + '.png'), f
" "$work/info.json" || fail "nami info doesn't give casks their icons"
mine search --json chrome | py "
import json, sys
hit = [f for f in json.load(sys.stdin) if f['attr'] == 'cask:google-chrome']
assert hit and hit[0].get('icon'), hit
print('search:', hit[0]['icon'])" || fail "nami search doesn't give Chrome its icon"
# One of the icons that come as the app is shown (not with `nami update`): fetched by `nami
# info`, kept in the cache by its sha256, the one the catalog names.
later=$(py "
import json, sys
later = json.load(open(sys.argv[1]))['nami:cask-icons']['cask_icons_later']
print(len(later), next((t for t in ['keka', 'itsycal', 'hiddenbar', 'menubarx', 'pearcleaner'] if t in later), sorted(later)[0]))" "$work/catalog.json")
echo "icons that come as they're shown: ${later% *}"
later=${later#* }
[ ! -e "$var/icons/cask:$later.png" ] || fail "cask:$later's icon came with nami update"
mine info --json "cask:$later" > "$work/later.json"
py "
import hashlib, json, os, sys
f = json.load(open(sys.argv[1]))[0]
sha = json.load(open(sys.argv[2]))['nami:cask-icons']['cask_icons_later'][sys.argv[3]]
print(f['attr'], f.get('icon'))
assert f.get('icon') and os.path.basename(f['icon']) == sha + '.png', f
assert hashlib.sha256(open(f['icon'], 'rb').read()).hexdigest() == sha
" "$work/later.json" "$work/catalog.json" "$later" || fail "cask:$later's icon didn't come as it was shown"
# The Popular Apps (gui/src/nami.rs) with screenshots from the Mac App Store, and one to look at.
popular=$(grep -o '"cask:[a-z0-9@._+-]*"' "$repo/gui/src/nami.rs" | tr -d '"' | sed 's/^cask://' | tr '\n' ' ')
with=
for t in $popular; do [ -z "$(named cask_screenshots "$t")" ] || with="$with $t"; done
echo "Popular Apps with screenshots:${with:- none}"
shot_app=$(echo $with | cut -d' ' -f1)
if [ -z "$shot_app" ]; then
  shot_app=$(py "import json, sys; print(next(iter(json.load(open(sys.argv[1]))['nami:cask-icons']['cask_screenshots']), ''))" "$work/catalog.json")
fi
[ -n "$shot_app" ] || fail "the catalog names no screenshots"
mine info --json --screenshots "cask:$shot_app" > "$work/shots.json"
py "
import hashlib, json, os, sys
f = json.load(open(sys.argv[1]))[0]
files = f['screenshots']
print(f['attr'], files)
assert files, 'no screenshots'
for p in files:
    assert os.path.isfile(p), p
    assert hashlib.sha256(open(p, 'rb').read()).hexdigest() == os.path.basename(p).split('.')[0], p
" "$work/shots.json" || fail "cask:$shot_app's screenshots didn't come as the catalog names them"

step "a cask the set hasn't got, installed for everyone: its app's own icon"
[ -z "$(named cask_icons "$outside")$(named cask_icons_later "$outside")" ] || fail "cask:$outside is in the set"
n install --system "cask:$outside" 2>&1 | tail -5
n list --json > "$work/list.json"
py "
import json, os, sys
hit = [f for f in json.load(open(sys.argv[1])) if f['attr'] == 'cask:' + sys.argv[2]]
assert hit and hit[0].get('icon') and os.path.isfile(hit[0]['icon']), hit
print(hit[0]['attr'], hit[0]['icon'])
" "$work/list.json" "$outside" || fail "cask:$outside has no icon of its own app"
icon=$(py "import json, sys; print([f for f in json.load(open(sys.argv[1])) if f['attr'] == 'cask:' + sys.argv[2]][0]['icon'])" "$work/list.json" "$outside")
sips -g pixelWidth -g pixelHeight "$icon"
cp "$icon" "$shots/own-icon-$outside.png"
# And as the person here sees it (an app everyone has): its own icon too.
mine info --json "cask:$outside" | py "
import json, os, sys
f = json.load(sys.stdin)[0]
print('as $(id -un):', f['attr'], f.get('icon'))
assert f.get('icon') and os.path.isfile(f['icon']), f" || fail "cask:$outside, everyone's, has no icon for the person here"

step "nami's app: Discover, and apps' pages"
# (As the person here, with the scratch root's catalog: NAMI is the nami it runs.)
picture() {
  NAMI="$bin" NAMI_ROOT="$root" NAMI_NO_MIGRATE=1 NAMI_THEME=$2 "$app/Contents/MacOS/nami-gui" ${1:+--page "$1"} > "$work/gui.log" 2>&1 &
  pid=$!
  sleep "$4"
  screencapture -x "$shots/$3.png" || echo "(no picture: screencapture failed)"
  kill "$pid" 2>/dev/null || true
  wait "$pid" 2>/dev/null || true
  tail -5 "$work/gui.log"
}
picture "" light discover-light 40
picture "" dark discover-dark 30
picture "app:cask:$shot_app" light "app-$shot_app" 30
picture app:cask:google-chrome light app-google-chrome 25
picture "app:cask:$outside" light "app-$outside" 20
picture "app:cask:$later" light "app-$later" 20
ls -la "$shots"
echo "screenshots: cask:$shot_app; own icon: cask:$outside; as it's shown: cask:$later"
echo PASS
