#!/bin/sh
# nami.app, on a Mac: nami's window (nami-gui, built without namiOS: --no-default-features),
# its Info.plist and icon, signed ad hoc. CI makes one for each Mac (.github/workflows/macos.yml)
# and puts it in that Mac's nami tarball; install-mac.sh copies it to /Applications.
#   gui/mac/make-app.sh <nami-gui> <version> <out-dir>    -> <out-dir>/nami.app
#
# Signed ad hoc (`codesign --sign -`): Apple silicon runs nothing unsigned, and an ad-hoc
# signature is enough for that. It names no developer, so it isn't notarized: a nami.app
# downloaded with a browser is stopped by Gatekeeper the first time (docs/macos.md says what
# a person sees and does). One copied by nami's installer (curl, not a browser) carries no
# quarantine, and opens. A Developer ID signature and notarization are for later.
set -eu
[ $# -eq 3 ] || { echo "usage: $0 <nami-gui> <version> <out-dir>"; exit 2; }
bin=$1 version=$2 out=$3
here=$(cd "$(dirname "$0")" && pwd)
app=$out/nami.app
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$bin" "$app/Contents/MacOS/nami-gui"
chmod 755 "$app/Contents/MacOS/nami-gui"
sed "s/@VERSION@/$version/g" "$here/Info.plist" > "$app/Contents/Info.plist"
printf 'APPL????' > "$app/Contents/PkgInfo"
cp "$here/nami.icns" "$app/Contents/Resources/nami.icns"
plutil -lint "$app/Contents/Info.plist"
# (Nothing extra in its folders: a signature covers every file, and extended attributes break it.)
xattr -cr "$app"
codesign --force --sign - --identifier dev.namios.nami "$app"
codesign --verify --strict --verbose=2 "$app"
echo "made $app ($version)"
