"""Which catalog packages are apps, and a picture for every app.

Run on the index build machine after in-container.sh (build-index.sh does).

Apps: a package is an app when its installed output has
share/applications/*.desktop. Hydra publishes a file listing (<hash>.ls)
next to every package in cache.nixos.org; wrapped apps (chromium,
thunderbird, ...) link share/ into another store path, so links are
followed into that path's listing.

Icons, best source first:
  1. Flathub's 128px icon, matched by desktop file ID or name
  2. Debian's AppStream icons (extracted from Debian's own packages),
     matched by desktop file ID
  3. The icon inside the package itself: hicolor/pixmaps files from its
     listing, pulled out of the package's NAR as it streams by (just until
     the file goes by, however big the package)
  4. Papirus, an icon theme with drawings of thousands of Linux apps, by
     the app's name
  5. What the desktop file's Icon= names (for a package Hydra hasn't
     built, the desktop entry its build closure makes): a file in another
     store path, the file by that name anywhere in the package, the Flathub
     app of that ID, or Papirus' icon of that name (helpers that show a
     theme icon: dialog-password, applications-system...); then the
     package's icon in another format (XPM, ICO...) or theme folder (intl/)
  6. The icon of the same app in another package: a variant sharing its
     desktop file or package name (ibus-mozc and ibus-engines.mozc), or
     the app its Icon= names (androidStudioPackages.dev: android-studio)
  7. Flathub or Papirus by a looser name: without a channel (-dev,
     -canary, ...) or version (charles-proxy5) on the end, ignoring case
     and dashes (vcv-rack: VCVRack); never one common word (Heaven)
  8. A picture in the package named like the app: icon.png or logo.svg in
     its own data folder (share/<app>/), or a part of its name
     (share/lomiri-clock-app/clock-app.svg)
XPM, ICO, GIF, BMP and JPEG icons are made PNGs (with Pillow, or
ImageMagick's `magick`; without either they're passed over), SVGZ
unpacked. An app whose desktop file names no icon, and that none of these
has, has none.

A package set's member is named by its attr's last part only when the
package's name says it too: androidStudioPackages.stable isn't Flathub's
io.gitlab.daikhan.stable.

Packages Hydra hasn't built have no listing. They count as apps when their
build closure (drvs/<attr>.closure.json, for unfree packages) makes a
desktop entry, or when Flathub has them under exactly their name (not a
package set's member: rPackages.bruno is a library, not Flathub's Bruno).

Each icon's sha256 goes into the index ("icon"), so the catalog signature
covers it, and all of them are packed into <out>/icons.tar (attr.png or
attr.svg), which `nami update` fetches from next to the index. Listings
and extracted icons are cached in <out>/.cache, so the next build only
downloads what changed.

usage: python3 -I index/apps.py <index.json> <out-dir>
"""
import gzip
import hashlib
import io
import json
import os
import re
import subprocess
import sys
import tarfile
import threading
from concurrent.futures import ThreadPoolExecutor

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import nar  # noqa: E402
from nar import CACHE, NarReader, children, get, listing, lookup, nar_extract, walk_files  # noqa: E402

PNG = b"\x89PNG\r\n\x1a\n"
# Picture files an Icon= can name, and those made PNGs (or unpacked) first.
IMAGES = (".png", ".svg", ".svgz", ".xpm", ".ico", ".gif", ".bmp", ".jpg", ".jpeg")
# A release channel or packaging on the end of a name: the same app.
CHANNEL = re.compile(r"[-_.](dev|canary|beta|stable|nightly|preview|insiders|unstable|unwrapped|wrapped|bin|fhs|symbolic)$", re.I)
# Freedesktop icon names start with their context; a desktop shows
# dialog-password-symbolic's parent dialog-password when the theme hasn't the
# longer one (the spec's fallback, here for those names alone).
CONTEXTS = {"accessories", "applications", "audio", "call", "dialog", "document", "edit", "emblem", "find", "folder", "format", "go",
            "help", "input", "mail", "mark", "media", "microphone", "network", "notifications", "preferences", "printer", "process",
            "security", "system", "text", "tools", "user", "utilities", "video", "view", "weather", "window"}


def norm(s):
    """A name to compare: lowercase letters and digits alone."""
    return re.sub(r"[^a-z0-9]", "", s.lower())


def image_stem(path):
    """An Icon= or file name without its folder and picture extension:
    org.xfce.ScreenSaver stays whole, jpilot.xpm is jpilot."""
    name = path.rsplit("/", 1)[-1]
    for ext in IMAGES:
        if name.lower().endswith(ext):
            return name[: -len(ext)].lower()
    return name.lower()


def looser(name):
    """The names an app may go by besides its own: without a channel or
    version on the end (android-studio-dev, charles-proxy5)."""
    out = []
    n = name
    while True:
        m = CHANNEL.search(n)
        if not m:
            break
        n = n[: m.start()]
        out.append(n)
    bare = re.sub(r"[-_.]?\d+$", "", n)
    if bare != n and len(norm(bare)) >= 5:
        out.append(bare)
    return [x for x in out if x]


def theme_fallbacks(name):
    """The spec's fallback names for a freedesktop theme icon:
    preferences-desktop-display-randr, then preferences-desktop-display,
    then preferences-desktop (never a bare context)."""
    parts = name.lower().removesuffix("-symbolic").split("-")
    if parts[0] not in CONTEXTS:
        return []
    return ["-".join(parts[:i]) for i in range(len(parts) - 1, 1, -1)]


def own_name(attr, pname):
    """The attr's last part, when it names the app: a package set's member
    (kdePackages.kate) yes, a channel of one (androidStudioPackages.stable)
    no, since the package's name doesn't say it."""
    last = attr.rsplit(".", 1)[-1]
    if "." in attr and norm(last) not in norm(pname or ""):
        return ""
    return last


def desktop_icons(text):
    """The Icon= of a desktop file's [Desktop Entry] ('' when none)."""
    inside = False
    for line in (text or "").splitlines():
        line = line.strip()
        if line.startswith("["):
            inside = line == "[Desktop Entry]"
        elif inside and "=" in line and line.split("=", 1)[0].strip() == "Icon":
            return line.split("=", 1)[1].strip()  # (dte writes "Icon = text-editor")
    return ""


def to_png(data, kind=""):
    """XPM, ICO, GIF, BMP or JPEG as a PNG (an ICO's largest picture), or
    None: with Pillow, else (or for what it can't read) ImageMagick."""
    try:
        from PIL import Image

        img = Image.open(io.BytesIO(data))
        if getattr(img, "ico", None):
            img = img.ico.getimage(max(img.ico.sizes(), key=lambda s: s[0] * s[1]))
        img.load()
        if img.mode not in ("RGBA", "RGB", "LA", "L"):
            img = img.convert("RGBA")
        buf = io.BytesIO()
        img.save(buf, "PNG")
        return buf.getvalue()
    except Exception:
        pass  # no Pillow, or an XPM it can't read (comments in it)
    try:
        src = f"{kind}:-" if kind in ("xpm", "ico", "gif", "bmp", "jpg", "jpeg") else "-"
        out = subprocess.run(["magick", src + "[0]", "png:-"], input=data, capture_output=True, timeout=60)
    except (OSError, subprocess.TimeoutExpired):
        return None
    return out.stdout if out.returncode == 0 and out.stdout[:8] == PNG else None


def as_icon(data, path=""):
    """(data, "png" or "svg") for a picture file's contents, or None."""
    if not data:
        return None
    if data[:8] == PNG:
        return data, "png"
    if data[:2] == b"\x1f\x8b":  # SVGZ
        try:
            data = gzip.decompress(data)
        except Exception:
            return None
    if b"<svg" in data[:4096]:
        return data, "svg"
    if path.lower().endswith(IMAGES[3:]) or data[:9] == b"/* XPM */" or data[:4] in (b"\0\0\1\0", b"GIF8") or data[:2] in (b"BM", b"\xff\xd8"):
        png = to_png(data, path.rsplit(".", 1)[-1].lower())
        return (png, "png") if png else None
    return None


def pixel_size(path):
    """The size a hicolor-style path says (48 for .../48x48/apps/x.png), else 0."""
    return next((int(x.split("x")[0].split("@")[0]) for x in path.split("/") if re.fullmatch(r"\d+x\d+(@\d+x?)?", x)), 0)


def best_file(files):
    """The best of (path, size) picture files: hicolor's apps folder (or
    pixmaps) first, then a PNG of 64-512px, an SVG, another PNG, the rest;
    the largest of those (an SVG's folder size says nothing) and the least
    deep (assets/app.png, not a theme's assets/themes/app/App.png); of
    equals, the first."""
    def score(c):
        p = c[0]
        png, svg = p.lower().endswith(".png"), p.lower().endswith((".svg", ".svgz"))
        px = 0 if svg else pixel_size(p)
        tier = 3 if png and 64 <= px <= 512 else 2 if svg else 1 if png else 0
        return ("/apps/" in p or "/pixmaps/" in p, tier, px if px <= 512 else -px, -p.count("/"))

    return max(files, key=score)[0] if files else None


# The app's own picture in its data folder: icon.png, logo.svg,
# icon_color.svg, icon512.png, product_logo_128.png.
OWN_PICTURE = re.compile(r"(app[-_]?)?(icon|logo)([-_]?(colou?r|\d+(x\d+)?))?|product[-_]logo([-_]\d+)?")


# Folders of pictures that aren't the app's icon.
NOT_ICONS = {"doc", "docs", "help", "examples", "tests", "test", "testability", "screenshots", "backgrounds", "wallpapers", "themes"}


# ---------- the build ----------

index = out = cache = icons = None
found = {}  # attr -> (desktop file names, icon candidates)
closure_icons = {}  # attr -> Icon= of the desktop entries its build closure makes
KNOWN = {}
ids, by_id, by_last, by_norm = [], {}, {}, {}
debian, deb_tars, theme, theme_norm = {}, {}, {}, {}
deb_lock = threading.Lock()


def setup(index_path, out_dir):
    global index, out, cache, icons
    index = json.load(open(index_path))
    out = out_dir
    cache = os.path.join(out, ".cache")
    icons = os.path.join(out, "icons")
    nar.setup(cache)
    os.makedirs(icons, exist_ok=True)


def app_info(attr):
    """(desktop file names, icon candidates) for one package."""
    e = index[attr]
    out_path = (e.get("outputs") or {}).get("bin") or (e.get("outputs") or {}).get("out")
    if not out_path:
        return attr, [], []
    if listing(out_path.split("/")[3][:32]) is None:
        return attr, None, []  # not in the binary cache
    desktops = sorted(n for n in children(out_path + "/share/applications") if n.endswith(".desktop"))
    if not desktops:
        return attr, [], []
    files = list(walk_files(out_path + "/share/icons")) + list(walk_files(out_path + "/share/pixmaps"))
    cands = [(p, s) for p, s in files if p.lower().endswith(IMAGES)]
    return attr, desktops, cands


def closure_desktops(attr):
    """Desktop entries an unfree package's build closure makes: {name: Icon=}."""
    f = os.path.join(out, "drvs", f"{attr}.closure.json")
    if not os.path.exists(f):
        return {}
    c = json.load(open(f))
    # (A closure in parts names its derivations the same way: closures.py --parts.)
    drvs = c.get("drvs") or c.get("drvs_by_sha256") or {}
    res = {}
    for d, drv in drvs.items():
        if d.endswith(".desktop.drv"):
            text = ((drv or {}).get("structuredAttrs") or (drv or {}).get("env") or {}).get("text") if isinstance(drv, dict) else None
            res[d.split("-", 1)[1][: -len(".drv")]] = desktop_icons(text)
    return res


def scan():
    """Which packages are apps (found), and their "apps" in the index."""
    attrs = [a for a, e in index.items() if e.get("outputs")]
    print(f"scanning {len(attrs)} packages for apps", flush=True)
    with ThreadPoolExecutor(64) as pool:
        scanned = list(pool.map(app_info, attrs))
    found.clear()
    found.update({a: (d, c) for a, d, c in scanned if d})
    uncached = [a for a, d, _ in scanned if d is None]
    for a in index:
        index[a].pop("apps", None)
        index[a].pop("icon", None)
    print(f"{len(found)} apps; {len(uncached)} packages aren't in the binary cache", flush=True)
    load_flathub()
    # Not built by Hydra, so no listing: an app if its build closure makes a
    # desktop entry, or if Flathub has exactly this name.
    for a in uncached:
        desk = closure_desktops(a)
        if desk:
            found[a] = (sorted(desk), [])
            closure_icons[a] = [desk[d] for d in sorted(desk) if desk[d]]
            continue
        # (A package set's member is a library or plugin of that name: rPackages.bruno
        # isn't Flathub's Bruno, emacsPackages.slack isn't Slack.)
        name = "" if "." in a else a.lower()
        fid = KNOWN.get(a) or (name and by_id.get(name)) or (by_last[name][0] if name and len(by_last.get(name, [])) == 1 else None)
        if fid:
            found[a] = ([], [])
    for a, (d, _) in found.items():
        index[a]["apps"] = d
    print(f"{len(found)} apps in all", flush=True)


# ---------- icon sources ----------


def load_flathub():
    global KNOWN, ids
    if ids:
        return
    # Apps whose Flathub ID neither their desktop file nor their name gives away.
    KNOWN = json.load(open(os.path.join(os.path.dirname(os.path.abspath(__file__)), "flathub-ids.json")))
    ids = json.loads(get("https://flathub.org/api/v2/appstream", 120) or b"[]")
    for i in ids:
        by_id[i.lower()] = i
        by_last.setdefault(i.rsplit(".", 1)[-1].lower(), []).append(i)
        by_norm.setdefault(norm(i.rsplit(".", 1)[-1]), []).append(i)


def stems(attr):
    d = [x[: -len(".desktop")] for x in found[attr][0]]
    return d, [*d, own_name(attr, index[attr].get("name")), index[attr].get("mainProgram") or ""]


def flathub_url(fid):
    return f"https://dl.flathub.org/media/icons/128x128/{fid}.png"


def flathub(attr):
    if KNOWN.get(attr, "").lower() in by_id:
        return flathub_url(KNOWN[attr])
    desk, names = stems(attr)
    for s in desk:
        if s.lower() in by_id:
            return flathub_url(by_id[s.lower()])
    for n in names:
        hits = by_last.get(n.lower().split(".")[-1], [])
        if n and len(hits) == 1:
            return flathub_url(hits[0])
    return None


def flathub_named(name, loose=False):
    """Flathub's app of this ID (an Icon= like com.example.App); loose:
    the one app whose ID ends in this name, ignoring case and dashes."""
    if name.lower() in by_id:
        return flathub_url(by_id[name.lower()])
    hits = by_norm.get(norm(name), []) if loose and len(norm(name)) >= 5 else []
    return flathub_url(hits[0]) if len(hits) == 1 else None


def load_debian():
    DEB = "https://deb.debian.org/debian/dists/sid"
    try:
        import yaml  # PyYAML: optional; without it, this source is skipped
    except ImportError:
        print("PyYAML missing: skipping Debian's icons", flush=True)
        return
    for comp in ("main", "contrib", "non-free"):
        raw = get(f"{DEB}/{comp}/dep11/Components-amd64.yml.gz", 300)
        if not raw:
            continue
        tars = {}
        for size in (128, 64):
            tar = get(f"{DEB}/{comp}/dep11/icons-{size}x{size}.tar.gz", 300)
            if tar:
                deb_tars[(comp, size)] = tarfile.open(fileobj=io.BytesIO(tar))
                tars[size] = set(deb_tars[(comp, size)].getnames())
        loader = getattr(yaml, "CSafeLoader", yaml.SafeLoader)
        for d in yaml.load_all(gzip.decompress(raw), Loader=loader):
            if not d or "ID" not in d:
                continue
            hit = None
            for size in (128, 64):
                for c in (d.get("Icon") or {}).get("cached") or []:
                    if c.get("width") == size and size in tars:
                        m = next((m for m in (c["name"], f"{size}x{size}/" + c["name"]) if m in tars[size]), None)
                        hit = hit or (m and ((comp, size), m))
            if not hit:
                continue
            keys = {d["ID"].lower().removesuffix(".desktop")}
            keys.update(x.lower().removesuffix(".desktop") for x in (d.get("Launchable") or {}).get("desktop-id", []))
            for k in keys:
                debian.setdefault(k, hit)


def from_debian(attr):
    desk, names = stems(attr)
    for s in desk or names:
        hit = debian.get(s.lower())
        if hit:
            with deb_lock:  # tarfile isn't thread-safe
                return deb_tars[hit[0]].extractfile(hit[1]).read()
    return None


def best_candidate(attr, more=False):
    """The package's own icon in hicolor's apps folder or pixmaps: named
    like its own name (mate-panel, not mate-system-log), else like any of
    its desktop files, else the one icon there; a PNG or SVG. more: in any
    of the theme's folders (an input method's is in intl/), and in other
    formats (XPM, ICO...)."""
    cands = [c for c in found[attr][1] if more or c[0].lower().endswith((".png", ".svg", ".svgz"))]
    if not cands:
        return None
    desk, names = stems(attr)
    main = {n.lower() for n in names[len(desk):] if n}
    wanted = {n.lower() for n in names if n}
    apps_dir = [(p, s) for p, s in cands if "/apps/" in p or "/pixmaps/" in p]
    named = [c for c in apps_dir if image_stem(c[0]) in main] or [c for c in apps_dir if image_stem(c[0]) in wanted]
    if not named and len({image_stem(p) for p, _ in apps_dir}) == 1:
        named = apps_dir
    if more and not named:
        named = [c for c in cands if image_stem(c[0]) in wanted]
    return best_file(named)


# Papirus' sizes and kinds, best first: an app's drawing, then the theme
# icons helpers name (actions at 24px, status at 32 or 48px); all SVGs.
PAPIRUS_SIZES = ("64x64", "48x48", "32x32", "24x24")


def load_papirus():
    """Papirus' icons by name ({name: svg path}), from its package."""
    e = index.get("papirus-icon-theme")
    if not e:
        return
    sp = e["outputs"]["out"]
    d = os.path.join(cache, "papirus3-" + sp.split("/")[3][:32])
    if not os.path.isdir(d):
        info = get(f"{CACHE}/{sp.split('/')[3][:32]}.narinfo")
        if not info:
            return
        fields = dict(l.split(": ", 1) for l in info.decode().splitlines() if ": " in l)
        tool = {"xz": ["xz", "-dc"], "zstd": ["zstd", "-dcq"]}.get(fields.get("Compression", "xz"), ["cat"])
        dl = subprocess.Popen(["curl", "-sfL", f"{CACHE}/{fields['URL']}"], stdout=subprocess.PIPE)
        un = subprocess.Popen(tool, stdin=dl.stdout, stdout=subprocess.PIPE)
        dl.stdout.close()
        files = NarReader(un.stdout).collect("share/icons/Papirus/")
        un.wait()
        dl.wait()
        os.makedirs(d + ".tmp", exist_ok=True)
        ranks = {}
        for name, data in files.items():
            for _ in range(4):  # aliases: name -> another icon (relative link)
                if not isinstance(data, tuple):
                    break
                base = name.rsplit("/", 1)[0].split("/") if "/" in name else []
                for t in data[1].split("/"):
                    base = base[:-1] if t == ".." else base + [t] if t not in ("", ".") else base
                data = files.get("/".join(base))
            if not isinstance(data, bytes) or name.count("/") != 2:
                continue
            size, kind, file = name.split("/")
            if size not in PAPIRUS_SIZES or kind == "panel" or not file.endswith(".svg"):  # (panel: a tray's light glyphs)
                continue
            # apps win over other kinds with the same name, then the larger size
            rank = (kind == "apps", -PAPIRUS_SIZES.index(size))
            if rank > ranks.get(file, (False, -9)):
                ranks[file] = rank
                open(os.path.join(d + ".tmp", file), "wb").write(data)
        os.replace(d + ".tmp", d)
    for n in os.listdir(d):
        if n.endswith(".svg"):
            theme[n[:-4].lower()] = os.path.join(d, n)
    for n in sorted(theme):
        theme_norm.setdefault(norm(n), theme[n])


def papirus(name, loose=False):
    """Papirus' icon by name (or the name with -symbolic off), as bytes;
    loose: ignoring case and dashes too (vcv-rack: VCVRack)."""
    n = image_stem(name)
    for k in (n, n.removesuffix("-symbolic")):
        if k in theme:
            return open(theme[k], "rb").read()
    if loose and len(norm(n)) >= 4 and norm(n) in theme_norm:
        return open(theme_norm[norm(n)], "rb").read()
    return None


def icon_for(attr):
    """(attr, data, extension, source); one odd package never stops the run."""
    try:
        return find_icon(attr)
    except Exception as e:
        print(f"warning: {attr}: {e!r}", flush=True)
        return attr, None, None, None


def extracted(path):
    """A picture file out of a package as (data, ext), or None."""
    return as_icon(nar_extract(path), path)


def find_icon(attr):
    url = flathub(attr)
    if url:
        data = get(url, 30)
        if data and data[:8] == PNG:
            return attr, data, "png", "flathub"
    data = from_debian(attr)
    if data:
        return attr, data, "png", "debian"
    path = best_candidate(attr)
    if path:
        got = extracted(path)
        if got:
            return attr, *got, "package"
    desk, names = stems(attr)
    for n in desk + names:
        data = n and papirus(n)
        if data:
            return attr, data, "svg", "papirus"
    for name in icon_names(attr):
        got = by_icon_name(attr, name)
        if got:
            return (attr, *got)
    path = best_candidate(attr, more=True)
    got = path and extracted(path)
    if got:
        return attr, *got, "package"
    return attr, None, None, None


def by_icon_name(attr, name):
    """(data, ext, source) for what an Icon= names, or None."""
    if name.startswith("/nix/store/"):
        got = extracted(name)
        if got:
            return (*got, "package")
    out_path = index[attr]["outputs"].get("bin") or index[attr]["outputs"].get("out")
    stem = image_stem(name)
    if attr not in closure_icons and listing(out_path.split("/")[3][:32]):
        # Kept anywhere in the package (/usr/share/app/x.svg, /opt/app/x.ico
        # as upstream wrote it, or lib/<app>/...): any picture by that name.
        hits = [(p, n) for p, n in walk_files(out_path, 0, 10) if p.lower().endswith(IMAGES) and image_stem(p) == stem]
        path = best_file(hits)
        got = path and extracted(path)
        if got:
            return (*got, "package")
    if name.startswith("/"):
        return None
    url = flathub_named(name)
    data = url and get(url, 30)
    if data and data[:8] == PNG:
        return data, "png", "flathub"
    data = papirus(name)
    if data:
        return data, "svg", "papirus"
    for fallback in theme_fallbacks(name):
        data = papirus(fallback)
        if data:
            return data, "svg", "papirus"
    return None


def icon_names(attr):
    """The Icon= of the app's desktop files (read out of its NAR), the
    one named like the app first; for a package Hydra hasn't built, its
    build closure's."""
    if attr in closure_icons:
        return closure_icons[attr]
    e = index[attr]["outputs"]
    out_path = e.get("bin") or e.get("out")
    wanted = {norm(n) for n in (own_name(attr, index[attr].get("name")), index[attr].get("mainProgram")) if n}
    res = []
    for d in sorted(found[attr][0], key=lambda d: norm(d.removesuffix(".desktop").rsplit(".", 1)[-1]) not in wanted):
        hit = lookup(f"{out_path}/share/applications/{d}")
        if not hit:
            continue
        name = desktop_icons((nar_extract(f"{hit[0]}/{hit[1]}") or b"").decode(errors="replace"))
        if name and name not in res:
            res.append(name)
    return res


def variants(missing, have):
    """Icons for apps that have none from another package of the same app:
    one sharing a desktop file or package name, or the app an Icon= names
    (or names without its channel: android-studio-dev). {attr: result}."""
    by_desk, by_pname, by_name = {}, {}, {}
    for a, r in have.items():
        if a not in found:
            continue
        for d in found[a][0]:
            by_desk.setdefault(d, r)
            by_name.setdefault(norm(d.removesuffix(".desktop")), r)
        if index[a].get("name"):
            by_pname.setdefault(index[a]["name"], r)
        by_name.setdefault(norm(a), r)
        own = own_name(a, index[a].get("name"))
        if own:
            by_name.setdefault(norm(own), r)
    res = {}
    for a in missing:
        r = next((by_desk[d] for d in found[a][0] if d in by_desk), None) or by_pname.get(index[a].get("name") or "")
        if not r:
            for n in icon_names(a):
                # One common word (Heaven, Logic) only when the app's own name starts
                # with it (linyaps-web-store-installer: linyaps).
                if n.startswith("/") or words(n) < 2 and "." not in n and not norm(a.rsplit(".", 1)[-1]).startswith(norm(n)):
                    continue
                for k in [n, *looser(n)]:
                    r = r or by_name.get(norm(image_stem(k)))
        if r:
            res[a] = (a, r[1], r[2], "variant")
    return res


def words(name):
    """How many words a name has (vcv-rack, VCVRack and pdfstudio2021: 2)."""
    return len(re.findall(r"[A-Z]+(?![a-z])|[A-Z]?[a-z]+|\d+", name))


def loosely(attr):
    """Flathub or Papirus by a looser name, then a picture in the app's own
    data folder: (attr, data, ext, source). A one-word name (Heaven,
    Logic) is too common to go by loosely."""
    desk, names = stems(attr)
    given = [n for n in icon_names(attr) if n and not n.startswith("/")]
    for n in [*given, *names]:
        if not n or words(n) < 2:
            continue
        for k in [n, *looser(n)]:
            url = flathub_named(k, loose=True)
            data = url and get(url, 30)
            if data and data[:8] == PNG:
                return attr, data, "png", "flathub"
            data = papirus(k, loose=True)
            if data:
                return attr, data, "svg", "papirus"
    out_path = index[attr]["outputs"].get("bin") or index[attr]["outputs"].get("out")
    if attr in closure_icons or not listing(out_path.split("/")[3][:32]):
        return attr, None, None, None
    # Folders named like the app (not like a theme icon: settings/), and the
    # app's name in parts (lomiri-clock-app's share/lomiri-clock-app/clock-app.svg).
    own = {norm(n) for n in [*names, *(g for g in given if not papirus(g))]} - {""}
    folder, part = [], []
    for p, n in walk_files(out_path, 0, 10):
        parts = p.split("/")[4:]
        if not p.lower().endswith(IMAGES) or NOT_ICONS & {x.lower() for x in parts[:-1]}:
            continue
        stem = norm(image_stem(p))
        if OWN_PICTURE.fullmatch(image_stem(p)) and any(norm(x) in own for x in parts[:-1]):
            folder.append((p, n))
        elif len(stem) >= 8 and any(o.startswith(stem) or o.endswith(stem) for o in own):
            part.append((p, n))
    for hits in (folder, part):
        path = best_file(hits)
        got = path and extracted(path)
        if got:
            return attr, *got, "package"
    return attr, None, None, None


def find_all(attrs, have=None):
    """Icons for these apps: {attr: (attr, data, ext, source)}. `have`: the
    icons other apps already have, for variants."""
    with ThreadPoolExecutor(16) as pool:
        results = {r[0]: r for r in pool.map(icon_for, attrs)}
    known = {**(have or {}), **{a: r for a, r in results.items() if r[1]}}
    while True:
        missing = [a for a in attrs if not results[a][1]]
        more = variants(missing, known)
        if not more:
            break
        results.update(more)
        known.update(more)
    missing = [a for a in attrs if not results[a][1]]
    with ThreadPoolExecutor(16) as pool:
        for r in pool.map(lambda a: _safe(loosely, a), missing):
            if r[1]:
                results[r[0]] = r
    return results


def _safe(fn, attr):
    try:
        return fn(attr)
    except Exception as e:
        print(f"warning: {attr}: {e!r}", flush=True)
        return attr, None, None, None


def write(results, index_path):
    counts = {}
    for name in os.listdir(icons):
        os.remove(os.path.join(icons, name))
    for attr in sorted(results):
        _, data, ext, source = results[attr]
        counts[source] = counts.get(source, 0) + 1
        index[attr].pop("icon", None)
        if data and len(attr) + 4 <= 100:
            open(os.path.join(icons, f"{attr}.{ext}"), "wb").write(data)
            index[attr]["icon"] = hashlib.sha256(data).hexdigest()
    with tarfile.open(os.path.join(out, "icons.tar"), "w", format=tarfile.USTAR_FORMAT) as tar:
        for name in sorted(os.listdir(icons)):
            tar.add(os.path.join(icons, name), arcname=name)
    with open(index_path + ".tmp", "w") as f:
        json.dump(index, f, separators=(",", ":"))
    os.replace(index_path + ".tmp", index_path)
    have = sum(1 for a in found if index[a].get("icon"))
    print(f"{have} of {len(found)} apps have an icon: " + ", ".join(f"{n} from {s}" for s, n in sorted(counts.items(), key=lambda x: str(x[0])) if s))


def main(index_path, out_dir):
    setup(index_path, out_dir)
    scan()
    load_debian()
    load_papirus()
    print(f"{len(theme)} Papirus icons", flush=True)
    print(f"finding icons ({len(debian)} Debian entries, {len(ids)} Flathub apps)", flush=True)
    write(find_all(sorted(found)), index_path)


if __name__ == "__main__":
    main(sys.argv[1], sys.argv[2])
