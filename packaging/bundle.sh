#!/bin/sh
# Packs everything into one installer: dist/nami-<version>-x86_64-linux.run
# (nami, the catalog in out/, the NixOS module). On the
# target machine: sudo sh nami-<version>-x86_64-linux.run
# Needs target/release/nami (./build.sh) and out/ (index/build-index.sh).
# ICONS_URL=<url of icons.tar>: leave the app icons out; nami downloads them
# from there on install (checked against the catalog, so any host will do).
# usage: [ICONS_URL=...] packaging/bundle.sh
set -eu
cd "$(dirname "$0")/.."
version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
for f in target/release/nami out/index-x86_64-linux.json.xz out/drvs; do
  [ -e "$f" ] || { echo "missing $f (see the usage notes at the top of $0)"; exit 1; }
done
stage=$(mktemp -d); trap 'rm -rf "$stage"' EXIT
b=$stage/nami-$version
mkdir -p "$b/catalog" "$b/nixos"
cp target/release/nami install.sh uninstall.sh "$b/"
cp packaging/setup.sh "$b/" && chmod +x "$b/setup.sh"
cp packaging/nixos/nami.nix "$b/nixos/"
cp out/index-x86_64-linux.json.xz "$b/catalog/" && cp -r out/drvs "$b/catalog/"
if [ -n "${ICONS_URL:-}" ]; then
  echo "$ICONS_URL" > "$b/catalog/icons-url"
else
  cp out/icons.tar "$b/catalog/"
fi
[ -e out/git-revision ] && cp out/git-revision "$b/catalog/"

mkdir -p dist
run=dist/nami-$version-x86_64-linux.run
header() {
cat <<HEAD
#!/bin/sh
# Nami $version installer: sudo sh \$0
# Unpacks to a temporary directory and runs setup.sh from it.
set -eu
[ "\$(id -u)" -eq 0 ] || { echo "run as root: sudo sh \$0"; exit 1; }
tmp=\$(mktemp -d "\${TMPDIR:-/var/tmp}/nami-install.XXXXXX"); trap 'rm -rf "\$tmp"' EXIT
echo "unpacking Nami $version"
tail -n +$1 "\$0" | tar -xz -C "\$tmp"
sh "\$tmp/nami-$version/setup.sh"
exit 0
HEAD
}
# The payload starts on the line after the header.
lines=$(header 0 | wc -l)
header $((lines + 1)) > "$run"
tar -C "$stage" -cz "nami-$version" >> "$run"
chmod +x "$run"
echo "$run ($(du -h "$run" | cut -f1))"
