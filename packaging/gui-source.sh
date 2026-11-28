#!/bin/sh
# Points examples/packages/nami-gui at a release's nami-gui for one kind of computer: the
# version (Cargo.toml's), and that kind's address and checksum under `source:` (added when
# the recipe has none for it yet). The other kinds' stay as they are. packaging/gui-release.sh
# runs it for the nami-gui it makes; dev/publish and the public repo's arm-linux.yml for the
# one made on the other kind of computer, so x86_64's nami and ARM's carry the same recipe.
# usage: [BASE_URL=https://namios.dev] packaging/gui-source.sh <x86_64-linux|aarch64-linux> <sha256>
set -eu
cd "$(dirname "$0")/.."
BASE_URL=${BASE_URL:-https://namios.dev}
system=${1:?x86_64-linux or aarch64-linux}
sum=${2:?the sha256 of its tarball}
case $system in x86_64-linux|aarch64-linux) ;; *) echo "nami-gui is made for x86_64-linux and aarch64-linux, not $system"; exit 1 ;; esac
echo "$sum" | grep -Eq '^[0-9a-f]{64}$' || { echo "\"$sum\" isn't a sha256"; exit 1; }
version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
recipe=examples/packages/nami-gui/package.nami
url=$BASE_URL/releases/$version/nami-gui-$version-$system.tar.gz
awk -v sys="$system" -v url="$url" -v sum="$sum" -v ver="$version" '
  function add() { print "  " sys ":"; print "    url: " url; print "    sha256: " sum; done = 1 }
  /^version: / { print "version: " ver; next }
  /^source:/ { src = 1; print; next }
  # (The end of source:, the next key: this kind of computer goes last, when it had none.)
  src && /^[^ #]/ { if (!done) add(); src = 0; mine = 0 }
  # (One source for every computer, not one under each kind: left as it is, and refused.)
  src && /^  (url|sha256|path|name|type):/ { one = 1 }
  src && $0 == "  " sys ":" { mine = 1; found = 1; done = 1; print; next }
  src && /^  [^ ]/ { mine = 0 }
  mine && /^    url: / { print "    url: " url; n++; next }
  mine && /^    sha256: / { print "    sha256: " sum; n++; next }
  { print }
  END { if (src && !done) add()
        if (one || !done || (found && n != 2)) exit 1 }
' "$recipe" > "$recipe.new" || { rm -f "$recipe.new"; echo "$recipe: its source: isn't by kind of computer (x86_64-linux:, aarch64-linux:)"; exit 1; }
mv "$recipe.new" "$recipe"
echo "$recipe: $system's is $url ($sum)"
