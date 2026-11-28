"""Add an extra app (one built from packaging/apps/<name>, not from nixpkgs)
to a published catalog: its entry in the index, its icon in icons-128.tar.

Everything about the app comes from the built package itself, the way a
desktop would see it:
  - share/applications/*.desktop: that it's an app, its Icon= and Exec=
  - share/metainfo/*.xml (AppStream): summary, homepage, license, developer
  - share/icons (or pixmaps): the icon, 128px PNG if it ships one, else the
    largest PNG scaled down, else its SVG as it is (like publish-icons.py)
packaging/apps/<name>/app.nami, if there is one, overrides any of the
entry's fields (description, homepage, license, publisher, mainProgram,
unfree).

The entry has the fields nixpkgs entries have, plus where its store path is
served from ("cache": url + key) and who makes it ("publisher"). The
catalog is signed after this, so the signature covers both.

usage: python3 -I packaging/extra-apps.py <index.json.xz> <icons-128.tar>
         <recipe-dir> <store-path> <path-on-disk> <cache-url> <cache-key>
       python3 -I packaging/extra-apps.py --check <index.json(.xz)>
         <recipe-dir> <store-path> <path-on-disk>
The second form (packaging/check-app.sh) changes nothing: it prints, as
Markdown, what Nami would show for the app and where it's downloaded from,
and fails the way the first form would (not an app, a nixpkgs name).
"""
import configparser
import hashlib
import io
import json
import lzma
import os
import sys
import tarfile
import xml.etree.ElementTree as ET

OVERRIDABLE = {"description", "homepage", "license", "publisher", "mainProgram", "unfree"}


def read_desktop(text):
    """The [Desktop Entry] keys of a .desktop file (untranslated)."""
    p = configparser.ConfigParser(interpolation=None, strict=False)
    p.optionxform = str
    p.read_string(text)
    return dict(p["Desktop Entry"]) if p.has_section("Desktop Entry") else {}


def read_metainfo(text):
    """summary, homepage, license and developer of an AppStream file."""
    root = ET.fromstring(text)

    def plain(tag):  # the untranslated element
        for e in root.findall(tag):
            if not any(k.endswith("lang") for k in e.attrib):
                return " ".join((e.text or "").split()) or None
        return None

    out = {
        "summary": plain("summary"),
        "name": plain("name"),
        "license": plain("project_license"),
        "publisher": plain("developer/name") or plain("developer_name"),
    }
    for u in root.findall("url"):
        if u.get("type") == "homepage" and u.text:
            out["homepage"] = u.text.strip()
    return {k: v for k, v in out.items() if v}


def is_image(data):
    return data[:8] == b"\x89PNG\r\n\x1a\n" or b"<svg" in data[:2048]


def pick_icon(share, name):
    """(data, ext) of the icon named `name` under share/, best first."""
    if name and os.path.isabs(name):
        if os.path.isfile(name):
            return open(name, "rb").read(), name.rsplit(".", 1)[-1]
        name = os.path.basename(name).rsplit(".", 1)[0]
    found = []  # (score, path)
    for top in ("icons", "pixmaps"):
        for d, _, files in os.walk(os.path.join(share, top)):
            for f in files:
                stem, _, ext = f.rpartition(".")
                if stem != name or ext not in ("png", "svg"):
                    continue
                size = next((int(x.split("x")[0]) for x in d.split(os.sep) if "x" in x and x.split("x")[0].isdigit()), 0)
                # 128px PNG, then bigger PNGs (scaled down), then SVG, then smaller PNGs
                score = (3, 0) if ext == "png" and size == 128 else (2, -size) if ext == "png" and size > 128 else (1, 0) if ext == "svg" else (0, size)
                found.append((score, os.path.join(d, f)))
    for _, path in sorted(found, reverse=True):
        data = open(path, "rb").read()
        if is_image(data):
            return data, path.rsplit(".", 1)[-1]
    return None, None


def shrink(data):
    """A PNG at most 128px (publish-icons.py does the same)."""
    from PIL import Image

    img = Image.open(io.BytesIO(data))
    if max(img.size) <= 128:
        return data
    img.thumbnail((128, 128), Image.LANCZOS)
    buf = io.BytesIO()
    img.save(buf, "PNG", optimize=True)
    return buf.getvalue()


def describe(out_dir, store_path, overrides=None, scale=True):
    """(index entry, (icon data, ext) or None) for a built package at
    `out_dir` (its files) whose store path is `store_path`."""
    base = os.path.basename(store_path.rstrip("/"))
    name = base.split("-", 1)[1] if len(base) > 33 and base[32] == "-" else base
    share = os.path.join(out_dir, "share")
    apps_dir = os.path.join(share, "applications")
    desktops = sorted(f for f in os.listdir(apps_dir) if f.endswith(".desktop")) if os.path.isdir(apps_dir) else []
    if not desktops:
        raise ValueError(f"{store_path} has no share/applications/*.desktop: not an app")
    desk = read_desktop(open(os.path.join(apps_dir, desktops[0]), encoding="utf-8").read())
    meta = {}
    for d in ("metainfo", "appdata"):
        md = os.path.join(share, d)
        for f in sorted(os.listdir(md)) if os.path.isdir(md) else []:
            if f.endswith(".xml"):
                meta = read_metainfo(open(os.path.join(md, f), encoding="utf-8").read())
                break
        if meta:
            break

    entry = {"name": name, "outputs": {"out": store_path}, "apps": desktops}
    description = meta.get("summary") or desk.get("Comment") or desk.get("GenericName")
    if description:
        entry["description"] = description
    for k in ("homepage", "license", "publisher"):
        if meta.get(k):
            entry[k] = meta[k]
    exe = os.path.basename((desk.get("Exec") or "").split(" ")[0].strip('"'))
    if exe and os.path.exists(os.path.join(out_dir, "bin", exe)):
        entry["mainProgram"] = exe
    for k, v in (overrides or {}).items():
        if k not in OVERRIDABLE:
            raise ValueError(f"app.nami: unknown key {k!r} (valid: {', '.join(sorted(OVERRIDABLE))})")
        entry[k] = v

    data, ext = pick_icon(share, desk.get("Icon") or "")
    if data and ext == "png" and scale:
        data = shrink(data)
    return entry, (data, ext) if data else None


def merge(index, members, attr, entry, icon, cache):
    """Add `attr` to the index (a dict) and its icon to `members` ({tar
    name: bytes}). An attr nixpkgs already has is never replaced."""
    if attr in index and "cache" not in index[attr]:
        raise ValueError(f"the catalog already has {attr} from nixpkgs; rename the extra app")
    entry = dict(entry, cache=cache)
    for ext in ("png", "svg"):
        members.pop(f"{attr}.{ext}", None)
    if icon:
        data, ext = icon
        members[f"{attr}.{ext}"] = data
        entry["icon"] = hashlib.sha256(data).hexdigest()
    index[attr] = entry
    return index, members


def load_overrides(recipe):
    if not os.path.exists(os.path.join(recipe, "app.nami")):
        return {}
    import yaml

    return yaml.safe_load(open(os.path.join(recipe, "app.nami"))) or {}


def source_urls(recipe):
    """The url: lines of a recipe (no YAML parser needed): what a reviewer
    has to trust, since the build downloads them."""
    urls = []
    for line in open(os.path.join(recipe, "package.nami"), encoding="utf-8"):
        k, _, v = line.strip().lstrip("-{ ").partition(":")
        if k.strip() == "url":
            urls.append(v.strip().strip("}\"' ").split(",")[0].strip("\"' "))
    return urls


def check(index_file, recipe, store_path, out_dir):
    attr = os.path.basename(os.path.normpath(recipe))
    opener = lzma.open if index_file.endswith(".xz") else open
    with opener(index_file, "rt") as f:
        index = json.load(f)
    entry, icon = describe(out_dir, store_path, load_overrides(recipe), scale=False)
    if attr in index and "cache" not in index[attr]:
        raise ValueError(f"the catalog already has {attr} from nixpkgs; rename the recipe's directory (or use nixpkgs' {attr})")
    bins = sorted(os.listdir(os.path.join(out_dir, "bin"))) if os.path.isdir(os.path.join(out_dir, "bin")) else []
    rows = [
        ("Name", f"`{attr}` (`nami install {attr}`)"),
        ("Description", entry.get("description") or "**missing**: add a summary to the AppStream metainfo"),
        ("Publisher", entry.get("publisher") or "**missing**: add developer/name to the AppStream metainfo"),
        ("License", entry.get("license") or "**missing**: add project_license to the AppStream metainfo"),
        ("Homepage", entry.get("homepage") or "**missing**"),
        ("Icon", f"{icon[1]}, {max(1, len(icon[0]) // 1024)} KiB" if icon else "**missing**: ship one in share/icons, named like the .desktop file's Icon="),
        ("Opens", f"`{entry['mainProgram']}`" if entry.get("mainProgram") else "**no bin/ program matches the .desktop Exec=**"),
        ("Programs", ", ".join(f"`{b}`" for b in bins) or "none"),
        ("Downloads", ", ".join(source_urls(recipe)) or "nothing (source: path)"),
        ("Store path", f"`{store_path}`"),
    ]
    print(f"### {attr}\n\n| | |\n|---|---|")
    for k, v in rows:
        print(f"| {k} | {v} |")
    if attr in index:
        print(f"\nReplaces `{index[attr].get('name', attr)}`, in the catalog now.")
    print()
    return not any("**missing" in v or "**no bin" in v for _, v in rows)


def main():
    if sys.argv[1:2] == ["--check"]:
        try:
            complete = check(*sys.argv[2:6])
        except (ValueError, OSError) as e:
            print(f"### {os.path.basename(os.path.normpath(sys.argv[3]))}\n\n**Not added:** {e}\n")
            sys.exit(1)
        sys.exit(0 if complete else 2)
    index_file, tar_file, recipe, store_path, out_dir, cache_url, cache_key = sys.argv[1:8]
    attr = os.path.basename(os.path.normpath(recipe))
    entry, icon = describe(out_dir, store_path, load_overrides(recipe))
    index = json.load(lzma.open(index_file))
    members = {}
    if os.path.exists(tar_file):
        with tarfile.open(tar_file) as t:
            members = {m.name: t.extractfile(m).read() for m in t.getmembers() if m.isfile()}
    merge(index, members, attr, entry, icon, {"url": cache_url, "key": cache_key})
    with tarfile.open(tar_file + ".tmp", "w", format=tarfile.USTAR_FORMAT) as t:
        for n in sorted(members):
            info = tarfile.TarInfo(n)
            info.size, info.mtime, info.mode = len(members[n]), 0, 0o644
            t.addfile(info, io.BytesIO(members[n]))
    with lzma.open(index_file + ".tmp", "wt", preset=9) as f:
        json.dump(index, f, separators=(",", ":"), sort_keys=True)
    os.replace(tar_file + ".tmp", tar_file)
    os.replace(index_file + ".tmp", index_file)
    print(f"{attr}: {entry['name']}, {entry.get('description', 'no description')}" + ("" if icon else " (no icon)"))


if __name__ == "__main__":
    main()
