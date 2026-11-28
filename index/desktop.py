"""What their own desktop files say about the apps index/meta.py found
nothing for (neither Flathub nor Debian has them): the app's name, its
line, its freedesktop categories and its search words, so they're named,
have a kind and can be found in the window like the rest.

The desktop file is read out of the package as it streams from
cache.nixos.org (index/nar.py; cached in <out>/.cache): the one named like
the package or its program, else the first the app menu shows. Packages
whose download is over 100 MB are left as they are.

Run after index/meta.py (build-index.sh does).

usage: python3 -I index/desktop.py <index.json> <out-dir>
"""
import json
import os
import sys
from concurrent.futures import ThreadPoolExecutor

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import nar  # noqa: E402

LIMIT = 100 << 20

index_path, out = sys.argv[1], sys.argv[2]
index = json.load(open(index_path))
nar.setup(os.path.join(out, ".cache"))


def entry(text):
    """A desktop file's [Desktop Entry] keys (untranslated)."""
    keys, inside = {}, False
    for line in text.splitlines():
        line = line.strip()
        if line.startswith("["):
            inside = line == "[Desktop Entry]"
        elif inside and "=" in line and "[" not in line.split("=", 1)[0]:
            k, v = line.split("=", 1)
            keys.setdefault(k.strip(), v.strip())
    return keys


def shown(keys):
    return keys.get("Type", "Application") == "Application" and keys.get("NoDisplay", "").lower() != "true" and keys.get("Hidden", "").lower() != "true"


def describe(attr):
    e = index[attr]
    outs = e.get("outputs") or {}
    out_path = outs.get("bin") or outs.get("out")
    if not out_path:
        return attr, None
    names = {attr.rsplit(".", 1)[-1].lower(), (e.get("mainProgram") or "").lower()} - {""}
    desks = sorted(e.get("apps") or [], key=lambda d: (not any(n == d.lower().removesuffix(".desktop").rsplit(".", 1)[-1] for n in names), d))
    for d in desks:
        hit = nar.lookup(f"{out_path}/share/applications/{d}")
        if not hit:
            continue
        data = nar.nar_extract(f"{hit[0]}/{hit[1]}", LIMIT)
        keys = entry((data or b"").decode(errors="replace"))
        if keys.get("Name") and shown(keys):
            return attr, keys
    return attr, None


todo = [a for a, e in index.items() if e.get("apps") and "title" not in e]
named = 0
with ThreadPoolExecutor(16) as pool:
    for attr, keys in pool.map(describe, todo):
        if not keys:
            continue
        e = index[attr]
        e["title"] = keys["Name"]
        line = keys.get("Comment") or keys.get("GenericName")
        if line and line.lower() != keys["Name"].lower():
            e["summary"] = line
        cats = [c for c in keys.get("Categories", "").split(";") if c.isalpha()][:6]
        if cats:
            e["categories"] = cats
        words = [w.strip() for w in keys.get("Keywords", "").split(";") if w.strip()][:20]
        if words:
            e["keywords"] = words
        named += 1

print(f"{named} of {len(todo)} apps Flathub and Debian haven't named by their desktop files", flush=True)
json.dump(index, open(index_path + ".tmp", "w"), separators=(",", ":"))
os.replace(index_path + ".tmp", index_path)
