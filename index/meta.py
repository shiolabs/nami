"""What people see of an app when they look around the catalog: its name,
what kind of app it is, screenshots, a few lines about it, and how many
people use it, for the window's categories and app pages and for ranking
search results.

Run after index/apps.py (build-index.sh does): only apps get these. The
apps neither source has get theirs from their own desktop files after
(index/desktop.py).

Sources, matched by desktop file ID (or Flathub ID, as apps.py does):
  1. Flathub's AppStream (one file): name, summary, about, categories,
     keywords, screenshots; its daily stats: downloads, for popularity
  2. Debian's AppStream (dep11), for apps Flathub hasn't; and Debian's
     popularity contest (installs), for apps in Debian

Fields added to an app's entry (all optional):
  title        "GIMP" (the app's own name; the attr stays what people type)
  summary      one line, the app's own
  about        a few sentences
  categories   freedesktop main categories: ["Graphics", "Photography"]
  keywords     ["photo", "paint", ...]
  screenshots  thumbnail URLs (Flathub's or Debian's), the default first
  popularity   0-1000: the app's rank among the catalog's apps, by new
               installs from Flathub (or, counting less, Debian's installs)

usage: python3 -I index/meta.py <index.json> [<cache-dir>]
  rewrites <index.json> in place; downloads go in <cache-dir>
  (default ~/.cache/nami-meta), kept a day.
"""
import datetime
import gzip
import json
import os
import re
import sys
import time
import urllib.request
import xml.etree.ElementTree as ET

index_file = sys.argv[1]
cache = sys.argv[2] if len(sys.argv) > 2 else os.path.expanduser("~/.cache/nami-meta")
os.makedirs(cache, exist_ok=True)
here = os.path.dirname(os.path.abspath(__file__))
KNOWN = json.load(open(os.path.join(here, "flathub-ids.json")))
sys.path.insert(0, here)
from apps import own_name  # noqa: E402  (androidStudioPackages.stable isn't Flathub's *.stable)

# The freedesktop main categories, the ones the window groups by.
MAIN = {"AudioVideo", "Audio", "Video", "Development", "Education", "Game", "Graphics", "Network", "Office", "Science", "Settings", "System", "Utility"}
# A few additional ones that say more than their main one.
EXTRA = {"Photography", "WebBrowser", "Email", "Chat", "InstantMessaging", "VideoConference", "TextEditor", "IDE", "WordProcessor", "Spreadsheet", "Presentation", "Music", "Player", "AudioVideoEditing", "3DGraphics", "RasterGraphics", "VectorGraphics", "Emulator", "Math", "Astronomy", "Finance", "FileManager", "TerminalEmulator", "Security"}


def get(url, name):
    path = os.path.join(cache, name)
    if not os.path.exists(path) or time.time() - os.path.getmtime(path) > 86400:
        for attempt in range(3):
            try:
                with urllib.request.urlopen(url, timeout=300) as r:
                    data = r.read()
                with open(path + ".part", "wb") as f:
                    f.write(data)
                os.replace(path + ".part", path)
                break
            except Exception as e:
                print(f"  {url}: {e}", flush=True)
                if attempt == 2:
                    return None
    return open(path, "rb").read()


def plain(el):
    return re.sub(r"\s+", " ", "".join(el.itertext())).strip() if el is not None else ""


def untranslated(parent, tag):
    return next((e for e in parent.findall(tag) if "{http://www.w3.org/XML/1998/namespace}lang" not in e.attrib), None)


def about_of(desc):
    """The description's first paragraphs, as plain text, up to ~500 chars."""
    if desc is None:
        return ""
    out = ""
    for p in desc:
        if p.tag != "p":
            continue
        t = plain(p)
        if out and len(out) + len(t) > 500:
            break
        out = f"{out} {t}".strip()
    return out[:600]


def tidy(cats):
    seen = []
    for c in cats:
        if (c in MAIN or c in EXTRA) and c not in seen:
            seen.append(c)
    return seen


# ---------- Flathub ----------
flathub = {}  # lowercase id or desktop id -> meta
raw = get("https://dl.flathub.org/repo/appstream/x86_64/appstream.xml.gz", "flathub-appstream.xml.gz")
if raw:
    for c in ET.fromstring(gzip.decompress(raw)).iter("component"):
        if c.get("type") not in ("desktop-application", "desktop", "console-application"):
            continue
        fid = (c.findtext("id") or "").removesuffix(".desktop")
        shots = []
        for s in c.iter("screenshot"):
            thumbs = [i for i in s.findall("image") if i.get("type") == "thumbnail" and i.text]
            best = min(thumbs, key=lambda i: abs(int(i.get("width") or 0) - 752), default=None)
            if best is not None:
                (shots.insert(0, best.text) if s.get("type") == "default" else shots.append(best.text))
        m = {
            "flathub": fid,
            "title": plain(untranslated(c, "name")),
            "summary": plain(untranslated(c, "summary")),
            "about": about_of(untranslated(c, "description")),
            "categories": tidy(e.text for e in c.iter("category") if e.text),
            "keywords": [e.text.strip() for e in c.iter("keyword") if e.text and "{http://www.w3.org/XML/1998/namespace}lang" not in e.attrib][:12],
            "screenshots": shots[:4],
        }
        keys = {fid.lower()}
        keys.update(e.text.lower().removesuffix(".desktop") for e in c.findall("launchable") if e.text)
        for k in keys:
            flathub.setdefault(k, m)
by_last = {}
for k, m in flathub.items():
    by_last.setdefault(k.rsplit(".", 1)[-1], set()).add(m["flathub"])

# Installs over the last four weeks (one file a day), by Flathub ID.
downloads = {}
today = datetime.date.today()
for d in range(1, 29):
    day = today - datetime.timedelta(days=d)
    raw = get(f"https://flathub.org/stats/{day:%Y/%m/%d}.json", f"flathub-{day}.json")
    if not raw:
        continue
    for ref, arches in (json.loads(raw).get("refs") or {}).items():
        # [downloads, of which updates]: new installs are what people pick.
        downloads[ref] = downloads.get(ref, 0) + sum(v[0] - v[1] for v in arches.values())

# ---------- Debian ----------
debian = {}  # lowercase desktop id -> meta
DEB = "https://deb.debian.org/debian/dists/sid"
SHOTS = "https://screenshots.debian.net"
try:
    import yaml

    loader = getattr(yaml, "CSafeLoader", yaml.SafeLoader)
    for comp in ("main", "contrib", "non-free"):
        raw = get(f"{DEB}/{comp}/dep11/Components-amd64.yml.gz", f"debian-{comp}.yml.gz")
        if not raw:
            continue
        for d in yaml.load_all(gzip.decompress(raw), Loader=loader):
            if not d or "ID" not in d or d.get("Type") not in ("desktop-application", "console-application"):
                continue
            shots = []
            for s in d.get("Screenshots") or []:
                t = min(s.get("thumbnails") or [], key=lambda i: abs(int(i.get("width") or 0) - 752), default=None)
                url = (t or {}).get("url")
                if url:
                    url = url if url.startswith("http") else f"{(s.get('source-image') or {}).get('url', '').split('/screenshots/')[0] or SHOTS}/{url}"
                    (shots.insert(0, url) if s.get("default") else shots.append(url))
            desc = (d.get("Description") or {}).get("C") or ""
            m = {
                "debian": d.get("Package"),
                "title": (d.get("Name") or {}).get("C", ""),
                "summary": (d.get("Summary") or {}).get("C", ""),
                "about": about_of(ET.fromstring(f"<d>{desc}</d>")) if desc else "",
                "categories": tidy(d.get("Categories") or []),
                "keywords": ((d.get("Keywords") or {}).get("C") or [])[:12],
                "screenshots": shots[:4],
            }
            keys = {d["ID"].lower().removesuffix(".desktop")}
            keys.update(x.lower().removesuffix(".desktop") for x in (d.get("Launchable") or {}).get("desktop-id", []))
            for k in keys:
                debian.setdefault(k, m)
except ImportError:
    print("PyYAML missing: skipping Debian's AppStream", flush=True)
except ET.ParseError:
    pass

installs = {}
raw = get("https://popcon.debian.org/by_inst.gz", "popcon.gz")
for line in gzip.decompress(raw).decode(errors="replace").splitlines() if raw else []:
    f = line.split()
    if len(f) > 2 and f[0].isdigit():
        installs[f[1]] = int(f[2])

# ---------- matching ----------
index = json.load(open(index_file))


def stems(attr, e):
    desk = [d.removesuffix(".desktop").lower() for d in e.get("apps") or []]
    return desk, [*desk, own_name(attr, e.get("name")).lower(), (e.get("mainProgram") or "").lower()]


def match(attr, e):
    desk, names = stems(attr, e)
    fh = None
    if attr in KNOWN:
        fh = flathub.get(KNOWN[attr].lower())
    for s in desk:
        fh = fh or flathub.get(s)
    for n in names:
        hits = by_last.get(n, set())
        if not fh and n and len(hits) == 1:
            fh = flathub.get(next(iter(hits)).lower())
    deb = next((debian[s] for s in desk + names if s in debian), None)
    return fh, deb


def percentiles(counts):
    """{attr: count} -> {attr: 0-1000}, by rank among the nonzero ones."""
    ranked = sorted((c, a) for a, c in counts.items() if c > 0)
    n = len(ranked)
    return {a: round(1000 * (i + 1) / n) for i, (_, a) in enumerate(ranked)}


fl_count, deb_count = {}, {}
matched = 0
for attr, e in index.items():
    for k in ("title", "summary", "about", "categories", "keywords", "screenshots", "popularity"):
        e.pop(k, None)
    if "apps" not in e:
        continue
    fh, deb = match(attr, e)
    if not (fh or deb):
        continue
    matched += 1
    for k in ("title", "summary", "about", "categories", "keywords", "screenshots"):
        v = (fh or {}).get(k) or (deb or {}).get(k)
        if v:
            e[k] = v
    # A suite's other desktop files (LibreOffice's Writer, Calc...) bring
    # their own search words: "word" finds LibreOffice.
    desk, _ = stems(attr, e)
    last = attr.rsplit(".", 1)[-1].lower()
    parts = [m for d in desk for m in (flathub.get(d), debian.get(d), debian.get(f"{last}-{d}")) if m and m is not fh and m is not deb]
    words = list(e.get("keywords") or [])
    for m in parts:
        words += [w for w in [m.get("title"), m.get("summary"), *(m.get("keywords") or [])] if w and w not in words]
    if words:
        e["keywords"] = words[:80]
    if fh:
        fl_count[attr] = downloads.get(fh["flathub"], 0)
    if deb and deb.get("debian"):
        deb_count[attr] = installs.get(deb["debian"], 0)

# Flathub's downloads are what people pick; Debian's installs count what
# every Debian comes with too, so they count for less, where Flathub hasn't it.
fl_p, deb_p = percentiles(fl_count), percentiles(deb_count)
for attr in set(fl_p) | set(deb_p):
    index[attr]["popularity"] = fl_p[attr] if attr in fl_p else deb_p[attr] * 3 // 4

apps = sum(1 for e in index.values() if "apps" in e)
print(f"{matched} of {apps} apps described ({len(flathub)} Flathub, {len(debian)} Debian entries); "
      f"{sum(1 for e in index.values() if e.get('screenshots'))} with screenshots, "
      f"{sum(1 for e in index.values() if 'popularity' in e)} ranked", flush=True)
json.dump(index, open(index_file + ".tmp", "w"), separators=(",", ":"))
os.replace(index_file + ".tmp", index_file)
