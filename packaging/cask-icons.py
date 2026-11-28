"""The icons of Homebrew's casks, as nami shows them on a Mac (src/casks.rs).

Made each week on GitHub's runners (.github/workflows/cask-icons.yml). nami chooses the casks
(`nami catalog cask-icons`: every one nami installs, the most installed first) and says where
each one's app is in its developer's download; this takes the app's icon from that download:
the app's Info.plist names it (CFBundleIconFile, else CFBundleIconName, else AppIcon.icns, as
casks.rs's app_icon_file does), it's read from Contents/Resources, and it becomes a PNG of at
most 256 pixels.

A zip is read where it is, with HTTP range requests: only its list of files and the files
needed come down. Anything else (a disk image, a tar) is downloaded whole, checked against
the cask's sha256 when it has one, only those files are taken out of it (7-Zip's own 7zz
reads disk images, HFS+ and APFS alike), and it's deleted. A download still coming after 20
minutes is left for next time.

On Linux the work is shared (--shard K/N: a matrix's job each) and put together after
(--merge, which also fetches the screenshots). An icon only in Assets.car (no .icns) is taken
on a Mac (--mac: AppKit draws it, as Finder does), for the casks merge lists in mac.json.

What a run before made is used again (--previous, its folder): only the casks new to the set,
those whose icon is missing, those that couldn't be made (again at a new version, after four
weeks, or with new RULES; a failure of the network's or a server's moment, next time; on a Mac,
again at a new version), and a few whose version changed (--refresh, the oldest first) are made.

Screenshots: the Mac App Store's, for the apps it sells, and nothing else. Each app's
Info.plist has its bundle's identifier (CFBundleIdentifier); Apple's public lookup
(itunes.apple.com/lookup, entity=macSoftware) says which of them are Mac apps on the App
Store, its version there, and its screenshots: up to 4, each a JPEG at most 1280 pixels wide.
They're fetched again only when the App Store's version changes.

What it makes, in <out>:
  <token>.png     each icon
  screenshots/<sha256>.jpg
  index.json      {"made": <date>, "rules": RULES,
                   "icons": {<token>: {"sha256", "version", "made", "installs", "bundle", "by"}},
                   "failed": {<token>: {"version", "why", "made", "assets_car", "mac_tried"}},
                   "screenshots": {<token>: {"bundle", "store_version", "files": [<sha256>], "made"}}}
  cask-icons.tar  index.json and the icons; cask-screenshots.tar, the screenshots: as nami's
                  catalog takes them (each checked, and named in its signature)
  mac.json        (--merge) the casks whose icon only a Mac can take

usage: cask-icons.py <casks.json> <out> [--previous <dir>] [--refresh N] [--jobs N]
                     [--only <token>,...] [--work <dir>] [--no-screenshots]
                     [--shard K/N | --merge <shard out>... | --mac]
  <casks.json>: what `nami catalog cask-icons --casks ... --installs ...` printed (--mac:
  merge's mac.json, and <out> the set it made, its two tars unpacked).
Needs Python 3 with Pillow (and its JPEG 2000 support, for some older icons) and 7-Zip's 7zz
(github.com/ip7z/7zip's releases); on a Mac (--mac), its own tools and swiftc.
"""
import argparse
import concurrent.futures
import datetime
import hashlib
import io
import json
import os
import plistlib
import re
import shutil
import subprocess
import sys
import tarfile
import tempfile
import threading
import time
import unicodedata
import urllib.error
import urllib.request
import zipfile

# (Pillow: on Linux; the Mac's part, --mac, has sips instead.)
try:
    from PIL import Image
except ImportError:
    Image = None

SIZE = 256
# How icons are taken out: a set made by other rules has its failures tried again.
RULES = 3
# 7-Zip's own build (7zz, 26.05 on GitHub's runners): Ubuntu's 7zip (23.01) lists a disk image's
# partitions, not their files, and doesn't read LZMA ones (Chrome's).
SEVEN = shutil.which("7zz") or "7zz"
TOKEN = re.compile(r"^[a-z0-9][a-z0-9+._@-]{0,127}$")
BUNDLE = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._-]{0,254}$")
# The largest download taken (an app's whole disk image), and the most of a zip read.
MAX_DOWNLOAD = 4 << 30
MAX_FILE = 64 << 20
MAX_SECONDS = 20 * 60
# As nami asks for a cask's download: its own name, unless the cask says otherwise.
AGENT = "nami (cask icons)"
TIMEOUT = 60
PNG = b"\x89PNG\r\n\x1a\n"
said = threading.Lock()


def say(*a):
    with said:
        print(*a, flush=True)


class Failed(Exception):
    """Why a cask has no icon made. `lasting`: as long as its version is the same (not the
    network's or a server's moment: then it's tried again next time)."""

    def __init__(self, why, lasting=True):
        super().__init__(why)
        self.lasting = lasting


def key(name):
    """A name as a Mac's disk compares it: Unicode's composed form, and either case (a cask
    says `RawTherapee_MacOS...` of a file named `..._macOS...`)."""
    return unicodedata.normalize("NFC", name).casefold()


# ---------- Downloads ----------


def request(job, url, extra=None):
    headers = {"User-Agent": AGENT}
    for k, v in job.get("headers") or []:
        headers[k] = v
    headers.update(extra or {})
    return urllib.request.Request(url, headers=headers)


def opened(job, extra=None):
    try:
        r = urllib.request.urlopen(request(job, job["url"], extra), timeout=TIMEOUT)
    except urllib.error.HTTPError as e:
        raise Failed(f"its download answered {e.code}", lasting=e.code < 500 and e.code != 429)
    except (urllib.error.URLError, OSError) as e:
        raise Failed(f"couldn't download it ({e})", lasting=False)
    # No checksum: then nothing on the way may be in the clear (as nami's own download).
    if not job.get("sha256") and not r.geturl().startswith("https://"):
        r.close()
        raise Failed("its download went to an address that isn't https, and the cask has no checksum")
    return r


class Remote:
    """A file on a server that answers range requests, read as a local one (for zipfile):
    in blocks, each fetched once."""

    BLOCK = 256 << 10

    def __init__(self, job, url, size):
        self.job, self.url, self.size, self.pos = job, url, size, 0
        self.blocks = {}
        self.fetched = 0

    def seekable(self):
        return True

    def readable(self):
        return True

    def tell(self):
        return self.pos

    def seek(self, off, whence=0):
        self.pos = off if whence == 0 else self.pos + off if whence == 1 else self.size + off
        return self.pos

    def block(self, n):
        if n not in self.blocks:
            start = n * self.BLOCK
            end = min(self.size, start + self.BLOCK) - 1
            r = urllib.request.urlopen(request(self.job, self.url, {"Range": f"bytes={start}-{end}"}), timeout=TIMEOUT)
            data = r.read()
            if r.status != 206 or len(data) != end - start + 1:
                raise Failed("its server stopped answering range requests")
            self.fetched += len(data)
            if self.fetched > MAX_FILE * 4:
                raise Failed("too much of its zip is needed")
            if len(self.blocks) > 64:
                self.blocks.pop(next(iter(self.blocks)))
            self.blocks[n] = data
        return self.blocks[n]

    def read(self, n=-1):
        if n is None or n < 0:
            n = self.size - self.pos
        n = max(0, min(n, self.size - self.pos))
        out = bytearray()
        while len(out) < n:
            b = self.block(self.pos // self.BLOCK)
            at = self.pos % self.BLOCK
            take = b[at:at + n - len(out)]
            out += take
            self.pos += len(take)
        return bytes(out)

    def close(self):
        pass


def fetch(job, work, whole=False):
    """The download, as something to read the app from: a zip where it is, else (or `whole`) a
    local file."""
    r = opened(job, {"Range": "bytes=0-4095"})
    head = r.read(4096) if r.status == 206 else b""
    span = r.headers.get("Content-Range", "")
    m = re.match(r"bytes 0-\d+/(\d+)$", span)
    if r.status == 206 and m and head.startswith(b"PK\x03\x04") and not job.get("nested") and not whole:
        size = int(m.group(1))
        r.close()
        return zipfile.ZipFile(Remote(job, r.geturl(), size))
    if r.status == 206:
        r.close()
        r = opened(job)
    # The whole of it (a server without range requests answered 200: that's it already).
    path = os.path.join(work, "download")
    h = hashlib.sha256()
    total = 0
    start = time.monotonic()
    with open(path, "wb") as f:
        while True:
            block = r.read(1 << 20)
            if not block:
                break
            total += len(block)
            if total > MAX_DOWNLOAD:
                raise Failed("its download is over 4 GB")
            # (A server that slow is tried again next time, not waited on for hours.)
            if time.monotonic() - start > MAX_SECONDS:
                raise Failed(f"its download was still coming after {MAX_SECONDS // 60} minutes", lasting=False)
            h.update(block)
            f.write(block)
    r.close()
    if job.get("sha256") and h.hexdigest() != job["sha256"]:
        raise Failed("its download isn't what Homebrew's cask says it is (another sha256)")
    return path


# ---------- What's in a download ----------


class Zip:
    def __init__(self, z):
        self.z = z
        # (A Mac's zip has its names in UTF-8 without saying so: read as such, not as CP437.)
        self.infos = {}
        for i in z.infolist():
            name = i.filename
            if not i.flag_bits & 0x800:
                try:
                    name = name.encode("cp437").decode("utf-8")
                except UnicodeError:
                    pass
            if not name.startswith("__MACOSX/"):
                self.infos[name.rstrip("/")] = i

    def names(self):
        return self.infos.keys()

    def link(self, name):
        i = self.infos.get(name)
        return i is not None and (i.external_attr >> 16) & 0o170000 == 0o120000

    def target(self, name):
        return self.z.read(self.infos[name]).decode(errors="replace")

    def read(self, name):
        i = self.infos[name]
        if i.file_size > MAX_FILE:
            raise Failed(f"{name} is too big for an icon")
        return self.z.read(i)

    def take(self, name, dest):
        with self.z.open(self.infos[name]) as src, open(dest, "wb") as out:
            shutil.copyfileobj(src, out)


class Tar:
    def __init__(self, path):
        self.t = tarfile.open(path, "r:*")
        self.members = {m.name.lstrip("./").rstrip("/"): m for m in self.t.getmembers()}

    def names(self):
        return self.members.keys()

    def link(self, name):
        m = self.members.get(name)
        return m is not None and m.issym()

    def target(self, name):
        return self.members[name].linkname

    def read(self, name):
        m = self.members[name]
        if m.size > MAX_FILE:
            raise Failed(f"{name} is too big for an icon")
        return self.t.extractfile(m).read()

    def take(self, name, dest):
        with self.t.extractfile(self.members[name]) as src, open(dest, "wb") as out:
            shutil.copyfileobj(src, out)


class Image7z:
    """A disk image, by 7-Zip: listed once, and only the files asked for taken out."""

    def __init__(self, path, work):
        self.path, self.work = path, work
        out = subprocess.run([SEVEN, "l", "-slt", "-ba", path], capture_output=True, timeout=600)
        if out.returncode != 0 and not out.stdout:
            raise Failed("7-Zip couldn't read its disk image: " + out.stderr.decode(errors="replace").strip()[-200:])
        self.entries = {}
        for block in out.stdout.decode(errors="replace").split("\n\n"):
            f = dict(l.split(" = ", 1) for l in block.splitlines() if " = " in l)
            if "Path" in f:
                self.entries[f["Path"]] = f
        if not self.entries:
            raise Failed("its disk image has nothing 7-Zip reads")

    def names(self):
        return self.entries.keys()

    def link(self, name):
        f = self.entries.get(name, {})
        return f.get("Symbolic Link", "") != "" or "l" in f.get("Attributes", "")[:1].lower()

    def target(self, name):
        t = self.entries.get(name, {}).get("Symbolic Link", "")
        if not t:
            # (HFS+ keeps a link's target as its contents.)
            dest = os.path.join(self.work, "link")
            self.take(name, dest)
            t = open(dest, "rb").read().decode(errors="replace")
            os.remove(dest)
        return t


    def take(self, name, dest):
        d = tempfile.mkdtemp(dir=self.work)
        out = subprocess.run([SEVEN, "x", "-y", "-spd", f"-o{d}", self.path, name], capture_output=True, timeout=1200)
        got = os.path.join(d, name)
        if not os.path.isfile(got):
            raise Failed(f"7-Zip couldn't take {os.path.basename(name)} out of its disk image: " + out.stderr.decode(errors="replace").strip()[-200:])
        shutil.move(got, dest)
        shutil.rmtree(d, ignore_errors=True)

    def read(self, name):
        if int(self.entries.get(name, {}).get("Size", "0") or 0) > MAX_FILE:
            raise Failed(f"{name} is too big for an icon")
        dest = os.path.join(self.work, "taken")
        self.take(name, dest)
        with open(dest, "rb") as f:
            data = f.read()
        os.remove(dest)
        return data

def resolve(a, name):
    """`name`, its links followed inside the download (a few, and never out of it)."""
    for _ in range(4):
        if not a.link(name):
            return name
        t = a.target(name).strip()
        to = os.path.normpath(os.path.join(os.path.dirname(name), t))
        if not t or t.startswith("/") or to.startswith(".."):
            raise Failed(f"{os.path.basename(name)} is a link out of the app")
        name = to
    raise Failed(f"{os.path.basename(name)} is a link to a link to a link")



def kind(path):
    """What a downloaded file is, by its bytes (as casks.rs's kind_of)."""
    with open(path, "rb") as f:
        head = f.read(512)
        f.seek(0, 2)
        size = f.tell()
        tail = b""
        if size >= 512:
            f.seek(size - 512)
            tail = f.read(4)
    if head.startswith((b"PK\x03\x04", b"PK\x05\x06")):
        return "zip"
    if head.startswith(b"xar!"):
        return "pkg"
    if tail == b"koly":
        return "dmg"
    if head.startswith((b"\x1f\x8b", b"BZh", b"\xfd7zXZ\x00")) or head[257:262] == b"ustar":
        return "tar"
    return "other"


def archive(path, work):
    k = kind(path)
    if k == "zip":
        try:
            return Zip(zipfile.ZipFile(path))
        except zipfile.BadZipFile:
            # (One Python can't read, as some tools write them: 7-Zip can.)
            return Image7z(path, work)
    if k == "tar":
        return Tar(path)
    if k == "dmg":
        return Image7z(path, work)
    if k == "pkg":
        raise Failed("its download is an installer package (.pkg)")
    raise Failed("its download isn't a zip, a tar or a disk image")


# ---------- The app's icon ----------


def icon_file(icon_file_, icon_name, resources):
    """Which of an app's Resources is its icon (as casks.rs's app_icon_file)."""
    lower = {r.lower(): r for r in resources}

    def named(n):
        if not isinstance(n, str):
            return None
        n = n.strip().rsplit("/", 1)[-1]
        if not n or n.startswith("."):
            return None
        hit = lower.get(n.lower())
        if hit and hit.lower().endswith((".icns", ".png", ".tiff", ".tif")):
            return hit
        return lower.get(n.lower() + ".icns")

    return named(icon_file_) or named(icon_name) or lower.get("appicon.icns")


def find_app(a, source):
    """Where the app `source` (as the cask says: `Foo.app`, `Foo 1.2/Foo.app`) is in `a`: as it
    says, or in a folder (a disk image's volume, a zip's top folder)."""
    want = key(source.strip("/") + "/Contents/Info.plist")
    hits = [n for n in a.names() if key(n) == want or key(n).endswith("/" + want)]
    if not hits:
        # (Its name alone, wherever it is: a cask's path can be its folder's in another version.)
        tail = key("/" + os.path.basename(source.strip("/")) + "/Contents/Info.plist")
        hits = [n for n in a.names() if key("/" + n).endswith(tail)]
    if not hits:
        return None
    return min(hits, key=lambda n: (n.count("/"), n))[: -len("/Contents/Info.plist")]


def icon_of(a, app, about):
    """The app's .icns, as bytes; and in `about`, its bundle's identifier (`bundle`), when it
    has one, for the Mac App Store."""
    if a.link(app + "/Contents/Info.plist"):
        raise Failed("its Info.plist is a link")
    try:
        data = a.read(app + "/Contents/Info.plist")
        # (An XML one may start at its DOCTYPE, or after spaces or a byte-order mark, which
        # plistlib doesn't take for XML by itself.)
        info = plistlib.loads(data) if data.startswith(b"bplist") else plistlib.loads(data.lstrip(b"\xef\xbb\xbf \t\r\n"), fmt=plistlib.FMT_XML)
    except Failed:
        raise
    except Exception as e:
        raise Failed(f"its app's Info.plist doesn't read ({e})")
    bundle = info.get("CFBundleIdentifier") if isinstance(info, dict) else None
    if isinstance(bundle, str) and BUNDLE.match(bundle):
        about["bundle"] = bundle
    res = app + "/Contents/Resources/"
    resources = [n[len(res):] for n in a.names() if n.startswith(res) and "/" not in n[len(res):]]
    f = icon_file(info.get("CFBundleIconFile"), info.get("CFBundleIconName"), resources)
    if not f:
        if any(r.lower() == "assets.car" for r in resources):
            # (Its picture is the Mac job's to take: --mac.)
            about["assets_car"] = True
            raise Failed("its app's icon is only in Assets.car, which only a Mac reads")
        raise Failed("its app has no icon file")
    return a.read(resolve(a, res + f))


def to_png(icns):
    """An .icns (or the PNG or TIFF an app names as its icon) as a PNG of at most SIZE pixels,
    from its smallest picture that size or larger."""
    try:
        im = Image.open(io.BytesIO(icns))
        if not icns.startswith(b"icns"):
            im.load()
    except Exception as e:
        raise Failed(f"its icon doesn't read ({e})")
    sizes = sorted(im.info.get("sizes") or [], key=lambda s: (s[0] * s[2], -s[2])) if icns.startswith(b"icns") else []
    if icns.startswith(b"icns") and not sizes:
        raise Failed("its icon has no pictures")
    big = [s for s in sizes if s[0] * s[2] >= SIZE]
    tries = big + [s for s in reversed(sizes) if s not in big]
    last = None
    for s in tries:
        try:
            im = Image.open(io.BytesIO(icns))
            im.size = s
            im.load()
            break
        except Exception as e:
            last = e
    else:
        if sizes:
            raise Failed(f"none of its icon's pictures read ({last})")
    profile = im.info.get("icc_profile")
    im = im.convert("RGBA")
    if profile:
        try:
            from PIL import ImageCms
            src = ImageCms.ImageCmsProfile(io.BytesIO(profile))
            im = ImageCms.profileToProfile(im, src, ImageCms.createProfile("sRGB"), outputMode="RGBA")
        except Exception:
            pass
    if max(im.size) > SIZE:
        im.thumbnail((SIZE, SIZE), Image.LANCZOS)
    if im.getchannel("A").getbbox() is None:
        raise Failed("its icon is empty")
    out = io.BytesIO()
    im.save(out, "PNG", optimize=True)
    data = out.getvalue()
    if shutil.which("oxipng"):
        r = subprocess.run(["oxipng", "-o", "4", "--strip", "safe", "-q", "--stdout", "-"], input=data, capture_output=True, timeout=120)
        if r.returncode == 0 and r.stdout.startswith(PNG) and len(r.stdout) < len(data):
            data = r.stdout
    return data, im.size


def make(job, work, about):
    """One cask's icon: its PNG (and in `about`, what icon_of says)."""
    d = tempfile.mkdtemp(dir=work, prefix=job["token"] + ".")
    try:
        try:
            got = fetch(job, d)
        except zipfile.BadZipFile:
            # (A zip whose list of files isn't where range requests find it: the whole of it.)
            got = fetch(job, d, whole=True)
        a = Zip(got) if isinstance(got, zipfile.ZipFile) else archive(got, d)
        if job.get("nested"):
            n = job["nested"].strip("/")
            hits = [x for x in a.names() if key(x) == key(n) or key(x).endswith("/" + key(n))]
            if not hits:
                raise Failed(f"its download has no {n}")
            inner = os.path.join(d, "nested")
            a.take(min(hits, key=len), inner)
            a = archive(inner, d)
        for _ in range(2):
            for source in job["apps"]:
                app = find_app(a, source)
                if app is not None:
                    return to_png(icon_of(a, app, about))
            # A download that's one archive in another (a disk image in a zip), as Homebrew
            # unpacks it without the cask saying so.
            inner = [x for x in a.names() if x.lower().endswith((".dmg", ".zip", ".tar.gz", ".tgz", ".tar.xz", ".tar.bz2")) and x.count("/") <= 1 and not x.startswith("__MACOSX/")]
            if len(inner) != 1:
                break
            a.take(inner[0], os.path.join(d, "inner"))
            a = archive(os.path.join(d, "inner"), d)
        raise Failed(f"its download has no {job['apps'][0]}")
    finally:
        shutil.rmtree(d, ignore_errors=True)


# ---------- Screenshots, from the Mac App Store ----------

# Apple's public lookup (no key): apps by their bundles' identifiers, many at once.
LOOKUP = "https://itunes.apple.com/lookup?entity=macSoftware&bundleId="
SHOTS = 4
SHOT_WIDTH = 1280
MAX_SHOT = 2 << 20


def look_up(bundles):
    """The Mac App Store's apps of these bundle identifiers: {bundle: (its version there, its
    screenshots' addresses)}. Only Mac apps (an iPhone app of the same identifier isn't one),
    and only those with screenshots. None if the App Store can't be asked."""
    found = {}
    bundles = sorted(set(bundles))
    for i in range(0, len(bundles), 40):
        part = bundles[i:i + 40]
        last = None
        for attempt in range(4):
            try:
                r = urllib.request.urlopen(urllib.request.Request(LOOKUP + ",".join(part), headers={"User-Agent": AGENT}), timeout=TIMEOUT)
                results = json.load(r).get("results", [])
                break
            except Exception as e:
                last = e
                time.sleep(15 * (attempt + 1))
        else:
            say(f"the App Store can't be asked ({last})")
            return None
        for x in results:
            b = x.get("bundleId")
            urls = [u for u in x.get("screenshotUrls") or [] if isinstance(u, str) and u.startswith("https://") and ".mzstatic.com/" in u]
            if x.get("kind") == "mac-software" and b in part and urls:
                found[b] = (str(x.get("version") or ""), urls[:SHOTS])
        # (Apple asks for few calls a minute.)
        time.sleep(3)
    return found


def bigger(url):
    """A screenshot's address at SHOT_WIDTH: the App Store's are thumbnails of any size asked
    for (`.../800x500bb.jpg`)."""
    return re.sub(r"/\d+x\d+[a-z]*\.(jpg|png|webp)$", f"/{SHOT_WIDTH}x{SHOT_WIDTH}bb.jpg", url)


def shot(url):
    """One screenshot, a JPEG at most SHOT_WIDTH pixels wide: as the App Store gives it when it
    is one, else made one (Pillow)."""
    r = urllib.request.urlopen(urllib.request.Request(bigger(url), headers={"User-Agent": AGENT}), timeout=TIMEOUT)
    data = r.read(MAX_SHOT * 4 + 1)
    im = Image.open(io.BytesIO(data))
    im.load()
    if im.format == "JPEG" and im.width <= SHOT_WIDTH and len(data) <= MAX_SHOT:
        return data
    im = im.convert("RGB")
    if im.width > SHOT_WIDTH:
        im.thumbnail((SHOT_WIDTH, SHOT_WIDTH * 4), Image.LANCZOS)
    out = io.BytesIO()
    im.save(out, "JPEG", quality=82, optimize=True, progressive=True)
    return out.getvalue()


def screenshots(apps, prev, previous_dir, out):
    """Each app's App Store screenshots (`apps`: {token: bundle}), into out/screenshots/: as a
    run before had them while the App Store's version is the same, else fetched again.
    {token: {"bundle", "store_version", "files": [sha256], "made"}}."""
    os.makedirs(os.path.join(out, "screenshots"), exist_ok=True)
    store = look_up(apps.values())
    kept = {}
    if store is None:
        # (Without the App Store this time: what was there stays.)
        store = {}
        for t, was in prev.items():
            if t in apps and was.get("bundle") == apps[t]:
                store[apps[t]] = (was.get("store_version", ""), None)
    today = datetime.date.today().isoformat()
    for t, bundle in sorted(apps.items()):
        if bundle not in store:
            continue
        version, urls = store[bundle]
        was = prev.get(t, {})
        old = [os.path.join(previous_dir or "", "screenshots", f"{s}.jpg") for s in was.get("files", [])]
        if was.get("bundle") == bundle and (urls is None or was.get("store_version") == version) and old and all(os.path.exists(o) for o in old):
            for o in old:
                shutil.copyfile(o, os.path.join(out, "screenshots", os.path.basename(o)))
            kept[t] = was
            continue
        files = []
        for u in urls or []:
            try:
                data = shot(u)
            except Exception as e:
                say(f"  {t}: a screenshot didn't come ({e})")
                continue
            sha = hashlib.sha256(data).hexdigest()
            with open(os.path.join(out, "screenshots", f"{sha}.jpg"), "wb") as f:
                f.write(data)
            if sha not in files:
                files.append(sha)
        if files:
            kept[t] = {"bundle": bundle, "store_version": version, "files": files, "made": today}
            kb = sum(os.path.getsize(os.path.join(out, "screenshots", f"{s}.jpg")) for s in files) // 1024
            say(f"  {t}: {len(files)} screenshots from the Mac App Store ({kb} KB)")
    return kept


# ---------- On a Mac: the icons only in Assets.car ----------

# AppKit draws the app's icon as Finder does (its Assets.car too): at most 512 pixels, as a PNG.
ICON_SWIFT = """
import AppKit
let icon = NSWorkspace.shared.icon(forFile: CommandLine.arguments[1])
var rect = NSRect(x: 0, y: 0, width: 512, height: 512)
guard let cg = icon.cgImage(forProposedRect: &rect, context: nil, hints: nil),
      let png = NSBitmapImageRep(cgImage: cg).representation(using: .png, properties: [:]) else { exit(1) }
try! png.write(to: URL(fileURLWithPath: CommandLine.arguments[2]))
"""


def mac_icon(job, work, tool, generic):
    """One cask's icon, on a Mac: its download whole, its app as macOS opens it (ditto, hdiutil,
    tar), its icon drawn by AppKit, made 256 pixels by sips. Not one that's macOS's own icon for
    any app (`generic`): that's an icon AppKit couldn't read."""
    d = tempfile.mkdtemp(dir=work, prefix=job["token"] + ".")
    mounted = []
    try:
        path = fetch(job, d, whole=True)
        k = kind(path)
        top = os.path.join(d, "unpacked")
        os.makedirs(top)
        if k == "zip":
            subprocess.run(["/usr/bin/ditto", "-x", "-k", path, top], check=True, capture_output=True, timeout=1200)
        elif k == "tar":
            subprocess.run(["/usr/bin/tar", "-xf", path, "-C", top], check=True, capture_output=True, timeout=1200)
        elif k == "dmg":
            mnt = os.path.join(d, "mnt")
            r = subprocess.run(["/usr/bin/hdiutil", "attach", "-nobrowse", "-readonly", "-noautoopen", "-noverify", "-mountpoint", mnt, path], input=b"qn\n", capture_output=True, timeout=600)
            if r.returncode != 0:
                raise Failed("hdiutil couldn't open its disk image")
            mounted.append(mnt)
            top = mnt
        else:
            raise Failed("its download isn't a zip, a tar or a disk image")
        names = []
        for root, dirs, _ in os.walk(top):
            rel = os.path.relpath(root, top)
            depth = 0 if rel == "." else rel.count("/") + 1
            for x in list(dirs):
                if x.endswith(".app"):
                    names.append(os.path.normpath(os.path.join(rel, x, "Contents", "Info.plist")))
                    dirs.remove(x)
            if depth >= 3:
                dirs[:] = []
        app = None
        for source in job["apps"]:
            want = key(source.strip("/"))
            hits = [n[: -len("/Contents/Info.plist")] for n in names if key(n[: -len("/Contents/Info.plist")]) == want or key(n[: -len("/Contents/Info.plist")]).endswith("/" + want)]
            hits = hits or [n[: -len("/Contents/Info.plist")] for n in names if key(os.path.basename(n[: -len("/Contents/Info.plist")])) == key(os.path.basename(source))]
            if hits:
                app = os.path.join(top, min(hits, key=len))
                break
        if app is None:
            raise Failed(f"its download has no {job['apps'][0]}")
        big = os.path.join(d, "big.png")
        out = os.path.join(d, "icon.png")
        subprocess.run([tool, app, big], check=True, capture_output=True, timeout=120)
        subprocess.run(["/usr/bin/sips", "-Z", str(SIZE), big, "--out", out], check=True, capture_output=True, timeout=120)
        data = open(out, "rb").read()
        if not data.startswith(PNG):
            raise Failed("AppKit drew no icon")
        if data == generic:
            raise Failed("AppKit drew macOS's icon for any app, not its own")
        return data
    finally:
        for m in mounted:
            subprocess.run(["/usr/bin/hdiutil", "detach", m, "-force", "-quiet"], capture_output=True, timeout=120)
        shutil.rmtree(d, ignore_errors=True)


def mac(todo_path, out, work):
    """The icons only in Assets.car (`todo`: the casks merge found), into the set in `out`."""
    todo = json.load(open(todo_path))
    index = json.load(open(os.path.join(out, "index.json")))
    tool = os.path.join(work, "icon")
    os.makedirs(work, exist_ok=True)
    with open(tool + ".swift", "w") as f:
        f.write(ICON_SWIFT)
    subprocess.run(["swiftc", "-O", "-o", tool, tool + ".swift"], check=True)
    # macOS's icon for an app without its own: what isn't one.
    blank = os.path.join(work, "Blank.app")
    os.makedirs(os.path.join(blank, "Contents"))
    with open(os.path.join(blank, "Contents", "Info.plist"), "wb") as f:
        plistlib.dump({"CFBundleIdentifier": "dev.namios.blank", "CFBundlePackageType": "APPL"}, f)
    subprocess.run([tool, blank, os.path.join(work, "blank.png")], check=True)
    subprocess.run(["/usr/bin/sips", "-Z", str(SIZE), os.path.join(work, "blank.png"), "--out", os.path.join(work, "blank-256.png")], check=True, capture_output=True)
    generic = open(os.path.join(work, "blank-256.png"), "rb").read()
    today = datetime.date.today().isoformat()
    made = 0
    for j in todo:
        t = j["token"]
        try:
            data = mac_icon(j, work, tool, generic)
        except Exception as e:
            why = str(e) if isinstance(e, Failed) else f"{type(e).__name__}: {e}"
            say(f"  {t}: no icon on the Mac either: {why}")
            # (Not tried on a Mac again until its version changes.)
            if t in index["failed"] and (not isinstance(e, Failed) or e.lasting):
                index["failed"][t]["mac_tried"] = j["version"]
            continue
        with open(os.path.join(out, f"{t}.png"), "wb") as f:
            f.write(data)
        was = index["failed"].pop(t, {})
        index["icons"][t] = {"sha256": hashlib.sha256(data).hexdigest(), "version": j["version"], "made": today, "installs": j.get("installs", 0), "by": "mac", **({"bundle": was["bundle"]} if was.get("bundle") else {})}
        made += 1
        say(f"  {t}: {len(data) // 1024} KB, on the Mac")
    finish(out, index)
    say(f"{made} of {len(todo)} icons only in Assets.car made on the Mac")


# ---------- The set ----------


def png_ok(data):
    return data.startswith(PNG) and len(data) < (1 << 20)


def write_tar(path, names, top):
    """A tar of `names` (in `top`), the same bytes every time: no dates or owners."""
    with tarfile.open(path + ".part", "w", format=tarfile.USTAR_FORMAT) as t:
        for n in names:
            info = t.gettarinfo(os.path.join(top, n), arcname=n)
            info.mtime, info.uid, info.gid, info.uname, info.gname, info.mode = 0, 0, 0, "", "", 0o644
            with open(os.path.join(top, n), "rb") as f:
                t.addfile(info, f)
    os.replace(path + ".part", path)


def finish(out, index):
    """index.json and the two tars, from what's in `out`."""
    index["icons"] = dict(sorted(index["icons"].items()))
    index["failed"] = dict(sorted(index["failed"].items()))
    index["screenshots"] = dict(sorted(index.get("screenshots", {}).items()))
    index["rules"] = RULES
    with open(os.path.join(out, "index.json"), "w") as f:
        json.dump(index, f, indent=1, sort_keys=True)
    write_tar(os.path.join(out, "cask-icons.tar"), ["index.json"] + [f"{k}.png" for k in index["icons"]], out)
    shots = sorted({f"screenshots/{s}.jpg" for w in index["screenshots"].values() for s in w["files"]})
    write_tar(os.path.join(out, "cask-screenshots.tar"), shots, out)
    size = sum(os.path.getsize(os.path.join(out, f"{k}.png")) for k in index["icons"])
    shot_size = sum(os.path.getsize(os.path.join(out, n)) for n in shots)
    say(f"{len(index['icons'])} icons ({size // 1024} KB in all), {len(index['failed'])} casks without one")
    say(f"{len(index['screenshots'])} apps with screenshots from the Mac App Store ({len(shots)} screenshots, {shot_size // 1024} KB)")


def make_icons(jobs, prev, previous_dir, out, work, refresh, workers):
    """Each job's icon into `out` (as a run before made it, where that still holds): the icons
    and the failures, as index.json has them."""
    today = datetime.date.today().isoformat()
    icons, failed, todo = {}, {}, []
    for j in jobs:
        t = j["token"]
        had = prev["icons"].get(t)
        png = os.path.join(previous_dir or "", f"{t}.png")
        data = open(png, "rb").read() if had and previous_dir and os.path.exists(png) else None
        if data and png_ok(data) and hashlib.sha256(data).hexdigest() == had.get("sha256"):
            icons[t] = dict(had, installs=j.get("installs", 0))
            with open(os.path.join(out, f"{t}.png"), "wb") as f:
                f.write(data)
            continue
        was = prev["failed"].get(t)
        # (Tried again when its version changes, after four weeks, or when the way icons are
        # taken out changed: RULES.)
        fresh_enough = was and was.get("made", "") >= (datetime.date.today() - datetime.timedelta(days=28)).isoformat()
        if was and was.get("version") == j["version"] and fresh_enough and prev.get("rules") == RULES:
            failed[t] = was
            continue
        todo.append(j)
    # A few whose version changed since, the oldest first: an app's icon changes now and then.
    # (Not those the Mac made: Linux can't.)
    stale = sorted((icons[j["token"]].get("made", ""), j["token"]) for j in jobs if j["token"] in icons and icons[j["token"]].get("version") != j["version"] and icons[j["token"]].get("by") != "mac")
    again = {t for _, t in stale[: max(0, refresh)]}
    todo += [j for j in jobs if j["token"] in again]
    say(f"{len(jobs)} casks: {len(icons)} icons from before ({len(again)} made again: their version changed), {len(failed)} that couldn't be made at this version, {len(todo)} to make")

    def one(j):
        about = {}
        try:
            data, size = make(j, work, about)
            return j, data, size, None, about
        except Failed as e:
            about["lasting"] = e.lasting
            return j, None, None, str(e), about
        except Exception as e:  # anything else of one cask's is that cask's alone, this time
            about["lasting"] = False
            return j, None, None, f"{type(e).__name__}: {e}", about

    made = 0
    with concurrent.futures.ThreadPoolExecutor(max_workers=max(1, workers)) as pool:
        for j, data, size, why, about in pool.map(one, todo):
            t = j["token"]
            if data is None:
                lasting = about.pop("lasting", True)
                say(f"  {t}: no icon: {why}" + ("" if lasting else " (tried again next time)"))
                if t not in icons and lasting:
                    failed[t] = {"version": j["version"], "why": why, "made": today, **about}
                elif t in icons and about.get("bundle"):
                    icons[t]["bundle"] = about["bundle"]
                continue
            with open(os.path.join(out, f"{t}.png"), "wb") as f:
                f.write(data)
            icons[t] = {"sha256": hashlib.sha256(data).hexdigest(), "version": j["version"], "made": today, "installs": j.get("installs", 0), **about}
            failed.pop(t, None)
            made += 1
            say(f"  {t}: {size[0]}x{size[1]}, {len(data) // 1024} KB")
    return icons, failed, made, len(todo)


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("casks", help="what `nami catalog cask-icons` printed")
    ap.add_argument("out")
    ap.add_argument("--previous", help="the folder of a run before (its index.json, PNGs and screenshots/)")
    ap.add_argument("--refresh", type=int, default=25, help="how many icons whose cask's version changed are made again")
    ap.add_argument("--jobs", type=int, default=4)
    ap.add_argument("--only", help="these tokens alone (comma-separated), to try")
    ap.add_argument("--work", help="where downloads go (default: beside <out>)")
    ap.add_argument("--no-screenshots", action="store_true", help="icons alone")
    ap.add_argument("--shard", help="K/N: the casks whose place in the list is K modulo N, alone, and no screenshots (a matrix's job; --merge puts them together)")
    ap.add_argument("--merge", nargs="+", metavar="SHARD", help="the shards' <out> folders, put together in <out>, with the screenshots; the casks whose icon only a Mac can take in <out>/mac.json")
    ap.add_argument("--mac", action="store_true", help="on a Mac: the icons of <casks> (merge's mac.json) into the set in <out>")
    a = ap.parse_args()
    work = a.work or tempfile.mkdtemp(prefix="cask-icons-work.", dir=os.path.dirname(os.path.abspath(a.out)))
    if a.mac:
        return mac(a.casks, a.out, work)
    if Image is None:
        sys.exit("Pillow is needed (python3-pil)")
    jobs = [j for j in json.load(open(a.casks)) if TOKEN.match(j.get("token", "")) and j.get("apps")]
    if a.only:
        only = set(a.only.split(","))
        jobs = [j for j in jobs if j["token"] in only]
    prev = {"icons": {}, "failed": {}, "screenshots": {}}
    if a.previous and os.path.exists(os.path.join(a.previous, "index.json")):
        prev.update(json.load(open(os.path.join(a.previous, "index.json"))))
    os.makedirs(a.out, exist_ok=True)
    os.makedirs(work, exist_ok=True)
    wanted = {j["token"] for j in jobs}

    if a.merge:
        icons, failed = {}, {}
        for d in a.merge:
            part = json.load(open(os.path.join(d, "index.json")))
            for t, w in part["icons"].items():
                if t in wanted:
                    shutil.copyfile(os.path.join(d, f"{t}.png"), os.path.join(a.out, f"{t}.png"))
                    icons[t] = w
            failed.update({t: w for t, w in part["failed"].items() if t in wanted})
        made, attempted = None, None
    else:
        if a.shard:
            k, n = (int(x) for x in a.shard.split("/"))
            jobs = [j for i, j in enumerate(jobs) if i % n == k]
            a.no_screenshots = True
        icons, failed, made, attempted = make_icons(jobs, prev, a.previous, a.out, work, a.refresh, a.jobs)
    shutil.rmtree(work, ignore_errors=True)
    for n in os.listdir(a.out):
        if n.endswith(".png") and n[:-4] not in icons:
            os.remove(os.path.join(a.out, n))
    # The Mac App Store's screenshots of the apps it sells, by the identifier each app's
    # Info.plist has (an app that failed only for its icon has one too).
    shots = {}
    shutil.rmtree(os.path.join(a.out, "screenshots"), ignore_errors=True)
    if not a.no_screenshots:
        bundles = {t: w["bundle"] for t, w in list(icons.items()) + list(failed.items()) if t in wanted and w.get("bundle")}
        shots = screenshots(bundles, prev.get("screenshots", {}), a.previous, a.out)
    index = {"made": datetime.date.today().isoformat(), "icons": icons, "failed": {t: w for t, w in failed.items() if t in wanted}, "screenshots": shots}
    finish(a.out, index)
    if a.merge:
        # For the Mac: those only a Mac reads (Assets.car), as their jobs.
        by = {j["token"]: j for j in jobs}
        todo = [by[t] for t, w in sorted(index["failed"].items()) if w.get("assets_car") and w.get("mac_tried") != w.get("version") and t in by]
        with open(os.path.join(a.out, "mac.json"), "w") as f:
            json.dump(todo, f)
        say(f"{len(todo)} icons only in Assets.car, for the Mac")
    elif attempted and attempted >= 20 and made < attempted // 3:
        sys.exit("too few icons made: something's wrong with how they're made")


if __name__ == "__main__":
    main()
