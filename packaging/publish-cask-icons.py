"""The icons of Homebrew's casks, and the Mac App Store's screenshots of the apps it sells, into
a Mac's catalog before it's signed (packaging/publish.sh, SYSTEM=aarch64-darwin or x86_64-darwin
with CASK_ICONS=<folder>).

<folder> has what the public repo's cask-icons.yml made on GitHub's runners
(packaging/cask-icons.py; packaging/catalog-update.sh fetches its newest with `gh`):
cask-icons.tar (index.json and <token>.png) and cask-screenshots.tar (screenshots/<sha256>.jpg).
Each is checked here (a token's name; an icon a PNG of 16 to 512 pixels under 1 MB, a
screenshot a JPEG of 64 to 2560 pixels under 2 MB; the sha256 the index says), and the catalog
gets an entry naming them, `nami:cask-icons` (src/casks.rs, ICONS_ENTRY): {"cask_icons":
{<token>: <sha256>} for the 600 most installed (AHEAD), which come with `nami update`,
"cask_icons_later": the same for the rest, each fetched as its app is first shown,
"cask_screenshots": {<token>: [<sha256>, ...]}}. The signature over the catalog covers them
so; a Mac keeps only what it names. Beside it:
  <menu>/<rev>/icons-<system>.tar          the 600's icons, as cask:<token>.png (a Mac's first fetch)
  <menu>/objects/<sha256>                  each icon and screenshot, by content (the 600's one at a
                                           time after; the rest and the screenshots as they're shown)
  <menu>/<rev>/files-<system>.json         {"icons/cask:<token>.png": <sha256>,
                                           "screenshots/<sha256>.jpg": <sha256>}: what
                                           packaging/r2-prune.sh keeps of the objects

usage: publish-cask-icons.py <index-<system>.json.xz> <folder> <system>
  (the index is rewritten in place; its folder is <menu>/<rev>)
"""
import hashlib
import io
import json
import lzma
import os
import re
import struct
import sys
import tarfile

ENTRY = "nami:cask-icons"
TOKEN = re.compile(r"^[a-z0-9][a-z0-9+._@-]{0,127}$")
SHA = re.compile(r"^[0-9a-f]{64}$")
PNG = b"\x89PNG\r\n\x1a\n"
MAX_SIDE, MAX_BYTES = 512, 1 << 20
MAX_SHOT_SIDE, MAX_SHOT_BYTES = 2560, 2 << 20
# How many of the most installed come with `nami update` (CASK_ICONS_AHEAD: another number, to try).
AHEAD = int(os.environ.get("CASK_ICONS_AHEAD", "600"))


def png_size(data):
    """A PNG's width and height (its IHDR), or None if it isn't one."""
    if not data.startswith(PNG) or len(data) < 33 or data[12:16] != b"IHDR":
        return None
    return struct.unpack(">II", data[16:24])


def jpeg_size(data):
    """A JPEG's width and height (its frame header), or None if it isn't one."""
    if not data.startswith(b"\xff\xd8"):
        return None
    i = 2
    while i + 9 < len(data):
        if data[i] != 0xFF:
            return None
        marker = data[i + 1]
        if marker in (0xD8, 0x01, 0xFF) or 0xD0 <= marker <= 0xD7:
            i += 1 if marker == 0xFF else 2
            continue
        length = struct.unpack(">H", data[i + 2:i + 4])[0]
        if 0xC0 <= marker <= 0xCF and marker not in (0xC4, 0xC8, 0xCC):
            h, w = struct.unpack(">HH", data[i + 5:i + 9])
            return w, h
        i += 2 + length
    return None


def put(objects, sha, data):
    obj = os.path.join(objects, sha)
    if not os.path.exists(obj):
        with open(obj + ".part", "wb") as o:
            o.write(data)
        os.replace(obj + ".part", obj)


def icons(tar_path):
    """The index, and {token: (sha256, bytes)}: the icons it names that are there and check out."""
    files = {}
    with tarfile.open(tar_path) as t:
        members = {m.name: m for m in t.getmembers() if m.isfile()}
        if "index.json" not in members:
            sys.exit(f"{tar_path} has no index.json")
        index = json.load(t.extractfile(members["index.json"]))
        for token, about in sorted(index.get("icons", {}).items()):
            m = members.get(f"{token}.png")
            if not TOKEN.match(token) or m is None or m.size > MAX_BYTES:
                print(f"  {token}: left out (no such icon, or not a cask's name)")
                continue
            data = t.extractfile(m).read()
            size = png_size(data)
            if hashlib.sha256(data).hexdigest() != about.get("sha256") or size is None or max(size) > MAX_SIDE or min(size) < 16:
                print(f"  {token}: left out (not the PNG its index names, or not an icon's size)")
                continue
            files[token] = (about["sha256"], data)
    return index, files


def screenshots(path, said):
    """{token: [(sha256, bytes)]}: the screenshots the index names that are there and check out."""
    if not os.path.exists(path):
        return {}
    out = {}
    with tarfile.open(path) as t:
        members = {m.name: m for m in t.getmembers() if m.isfile()}
        for token, about in sorted(said.items()):
            kept = []
            for sha in about.get("files", [])[:8]:
                m = members.get(f"screenshots/{sha}.jpg") if isinstance(sha, str) and SHA.match(sha) else None
                if not TOKEN.match(token) or m is None or m.size > MAX_SHOT_BYTES:
                    continue
                data = t.extractfile(m).read()
                size = jpeg_size(data)
                if hashlib.sha256(data).hexdigest() == sha and size and max(size) <= MAX_SHOT_SIDE and min(size) >= 64:
                    kept.append((sha, data))
                else:
                    print(f"  {token}: a screenshot left out (not the JPEG its index names, or not a screenshot's size)")
            if kept:
                out[token] = kept
    return out


def main():
    if len(sys.argv) != 4:
        sys.exit(__doc__)
    index_path, folder, system = sys.argv[1:4]
    if not re.match(r"^(aarch64|x86_64)-darwin$", system):
        sys.exit(f"{system}: the icons of Homebrew's casks are for a Mac's catalog")
    menu_rev = os.path.dirname(os.path.abspath(index_path))
    objects = os.path.join(os.path.dirname(menu_rev), "objects")
    tar_path = os.path.join(folder, "cask-icons.tar")
    said, files = icons(tar_path)
    if not files:
        sys.exit(f"{tar_path}: no icons to publish")
    shots = screenshots(os.path.join(folder, "cask-screenshots.tar"), said.get("screenshots", {}))
    # The most installed come with `nami update` (the tar); the rest as each app is shown.
    by_installs = sorted(files, key=lambda t: (-int(said["icons"][t].get("installs") or 0), t))
    ahead = set(by_installs[:AHEAD])
    # The catalog, with the entry (as publish-icons.py writes Linux's: compact, sorted).
    index = json.load(lzma.open(index_path))
    if ENTRY in index and "cask_icons" not in index[ENTRY]:
        sys.exit(f"{index_path} has a package named {ENTRY}")
    index[ENTRY] = {
        "name": "nami-cask-icons",
        "outputs": {},
        "uncached": True,
        "description": "The icons of Homebrew's casks nami shows on a Mac (not a package)",
        "cask_icons": {t: sha for t, (sha, _) in files.items() if t in ahead},
        "cask_icons_later": {t: sha for t, (sha, _) in files.items() if t not in ahead},
        "cask_screenshots": {t: [sha for sha, _ in s] for t, s in shots.items()},
    }
    with lzma.open(index_path + ".part", "wt", preset=9) as f:
        json.dump(index, f, separators=(",", ":"), sort_keys=True)
    os.replace(index_path + ".part", index_path)
    # Beside it: the icons' tar, the objects and what names them.
    os.makedirs(objects, exist_ok=True)
    tar_out = os.path.join(menu_rev, f"icons-{system}.tar")
    with tarfile.open(tar_out + ".part", "w", format=tarfile.USTAR_FORMAT) as t:
        for token, (sha, data) in files.items():
            put(objects, sha, data)
            if token not in ahead:
                continue
            info = tarfile.TarInfo(f"cask:{token}.png")
            info.size, info.mtime, info.mode = len(data), 0, 0o644
            t.addfile(info, io.BytesIO(data))
    os.replace(tar_out + ".part", tar_out)
    for s in shots.values():
        for sha, data in s:
            put(objects, sha, data)
    named = {f"icons/cask:{t}.png": sha for t, (sha, _) in files.items()}
    named.update({f"screenshots/{sha}.jpg": sha for s in shots.values() for sha, _ in s})
    with open(os.path.join(menu_rev, f"files-{system}.json"), "w") as f:
        json.dump(named, f, separators=(",", ":"), sort_keys=True)
    total = sum(len(d) for _, d in files.values())
    tar_bytes = sum(len(d) for t, (_, d) in files.items() if t in ahead)
    shot_bytes = sum(len(d) for s in shots.values() for _, d in s)
    entry = len(json.dumps(index[ENTRY], separators=(",", ":"), sort_keys=True))
    print(f"{len(files)} icons of Homebrew's casks in {os.path.basename(index_path)} ({total // 1024} KB; of {len(said.get('icons', {}))} in {os.path.basename(tar_path)}): {len(ahead)} in {os.path.basename(tar_out)} ({tar_bytes // 1024} KB), the rest as each app is shown")
    print(f"the catalog's entry {ENTRY}: {entry // 1024} KB of its JSON")
    print(f"{len(shots)} apps' screenshots from the Mac App Store ({sum(len(s) for s in shots.values())} of them, {shot_bytes // 1024} KB, fetched as each app's page opens)")


if __name__ == "__main__":
    main()
