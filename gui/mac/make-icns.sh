#!/bin/sh
# nami.icns, nami.app's icon, from icon.svg: each size macOS asks for, drawn with rsvg-convert
# and packed as macOS's iconutil packs an iconset (PNGs, one per size). Run after icon.svg
# changes; nami.icns is kept in the repo, so building nami.app needs neither tool.
#   gui/mac/make-icns.sh
set -eu
here=$(cd "$(dirname "$0")" && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
for size in 16 32 64 128 256 512 1024; do
  rsvg-convert -w "$size" -h "$size" "$here/icon.svg" -o "$work/$size.png"
done
python3 -I - "$work" "$here/nami.icns" <<'EOF'
import struct, sys
work, out = sys.argv[1], sys.argv[2]
# (type, pixels): as iconutil writes them, each a PNG.
kinds = [("icp4", 16), ("ic11", 32), ("icp5", 32), ("ic12", 64), ("ic07", 128), ("ic13", 256), ("ic08", 256), ("ic14", 512), ("ic09", 512), ("ic10", 1024)]
chunks = b""
for kind, size in kinds:
    data = open(f"{work}/{size}.png", "rb").read()
    chunks += kind.encode() + struct.pack(">I", len(data) + 8) + data
open(out, "wb").write(b"icns" + struct.pack(">I", len(chunks) + 8) + chunks)
EOF
echo "made $here/nami.icns ($(wc -c < "$here/nami.icns") bytes)"
