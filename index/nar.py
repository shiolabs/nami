"""Reading cache.nixos.org without downloading whole packages: a store
path's file listing (Hydra's <hash>.ls), links followed into other store
paths, and one file pulled out of a package's NAR as it streams by. Used by
index/apps.py (icons) and index/desktop.py (desktop files).

Listings and pulled-out files are cached under `setup(<cache dir>)`.
"""
import hashlib
import json
import os
import subprocess
import urllib.request
import zlib
from functools import lru_cache

CACHE = "https://cache.nixos.org"
cache = None


def setup(cache_dir):
    global cache
    cache = cache_dir
    for d in (cache, os.path.join(cache, "ls"), os.path.join(cache, "nar-icons")):
        os.makedirs(d, exist_ok=True)


def get(url, timeout=60):
    try:
        return urllib.request.urlopen(url, timeout=timeout).read()
    except Exception:
        return None


@lru_cache(maxsize=4096)
def listing(h):
    """The file listing of store path hash `h`, or None (cached on disk, deflated)."""
    f = os.path.join(cache, "ls", h + ".z")
    if os.path.exists(f):
        data = open(f, "rb").read()
        return json.loads(zlib.decompress(data)) if data else None
    raw = get(f"{CACHE}/{h}.ls")
    if raw is not None:
        try:
            raw = subprocess.run(["zstd", "-dcq"], input=raw, capture_output=True).stdout or raw
            json.loads(raw)
        except Exception:
            raw = None
    open(f, "wb").write(zlib.compress(raw, 6) if raw else b"")
    return json.loads(raw) if raw else None


def lookup(path, depth=0):
    """The listing node at an absolute store path, following symlinks
    into other store paths. Returns (store_path, rel, node) or None."""
    if depth > 8 or not path.startswith("/nix/store/"):
        return None
    parts = path.split("/")
    sp = "/".join(parts[:4])
    ls = listing(parts[3][:32])
    if not ls:
        return None
    node, rel = ls["root"], []
    for i, name in enumerate(parts[4:]):
        if node.get("type") == "symlink":
            return lookup(resolve(sp, rel, node["target"]) + "/" + "/".join(parts[4 + i:]), depth + 1)
        node = (node.get("entries") or {}).get(name)
        if node is None:
            return None
        rel.append(name)
    if node.get("type") == "symlink":
        return lookup(resolve(sp, rel[:-1], node["target"]), depth + 1)
    return sp, "/".join(rel), node


def resolve(sp, reldir, target):
    if target.startswith("/"):
        return target
    parts = [sp] + reldir
    for t in target.split("/"):
        if t == "..":
            if len(parts) > 1:  # never above the store path (a link out of /nix/store)
                parts.pop()
        elif t not in ("", "."):
            parts.append(t)
    return "/".join(parts)


def children(path):
    """{name: absolute path} of a directory, links followed."""
    hit = lookup(path)
    if not hit or hit[2].get("type") != "directory":
        return {}
    sp, rel, node = hit
    return {k: f"{sp}/{rel}/{k}" if rel else f"{sp}/{k}" for k in (node.get("entries") or {})}


def walk_files(path, depth=0, deepest=6):
    """Every regular file under a directory: (absolute path, size)."""
    if depth > deepest:
        return
    for name, p in children(path).items():
        hit = lookup(p)
        if not hit:
            continue
        kind = hit[2].get("type")
        if kind == "directory":
            yield from walk_files(p, depth + 1, deepest)
        elif kind == "regular":
            yield (f"{hit[0]}/{hit[1]}", hit[2].get("size", 0))


def nar_extract(path, limit=None):
    """One file out of a store path's NAR, streamed (cached by path). A
    failed download is retried, and never remembered as "no such file".
    With `limit`, NARs bigger than that (compressed bytes) aren't read."""
    key = hashlib.sha256(path.encode()).hexdigest()
    f = os.path.join(cache, "nar-icons", key)
    if os.path.exists(f):
        data = open(f, "rb").read()
        return data or None
    if not path.startswith("/nix/store/") or path.count("/") < 3:
        return None
    # A file inside a store path, or a store path that is one file itself
    # (an icon fetched on its own: /nix/store/<hash>-app.png).
    sp, rel = "/".join(path.split("/")[:4]), (path.split("/", 4) + [""])[4]
    info = get(f"{CACHE}/{sp.split('/')[3][:32]}.narinfo")
    if not info:
        return None
    fields = dict(l.split(": ", 1) for l in info.decode().splitlines() if ": " in l)
    tool = {"xz": ["xz", "-dc"], "zstd": ["zstd", "-dcq"], "bzip2": ["bzip2", "-dc"], "none": ["cat"]}.get(fields.get("Compression", "xz"))
    if not tool or limit and int(fields.get("FileSize") or 0) > limit:
        return None
    for attempt in range(2):
        dl = subprocess.Popen(["curl", "-sfL", "--retry", "3", f"{CACHE}/{fields['URL']}"], stdout=subprocess.PIPE)
        un = subprocess.Popen(tool, stdin=dl.stdout, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
        dl.stdout.close()
        try:
            data, complete = NarReader(un.stdout).find(rel), True
        except Exception:
            data, complete = None, False
        un.kill()
        dl.kill()
        un.wait()
        dl.wait()
        if complete:
            open(f, "wb").write(data or b"")  # found it, or the NAR really hasn't got it
            return data
    return None


class NarReader:
    """Just enough of the NAR format to pull one file out of a stream."""

    def __init__(self, stream):
        self.s = stream

    def read(self, n):
        buf = b""
        while len(buf) < n:
            chunk = self.s.read(min(n - len(buf), 1 << 20))
            if not chunk:
                raise EOFError
            buf += chunk
        return buf

    def skip(self, n):
        while n:
            n -= len(self.read(min(n, 1 << 20)))

    def num(self):
        return int.from_bytes(self.read(8), "little")

    def string(self, limit=1 << 16):
        n = self.num()
        if n > limit:
            raise ValueError("unexpected NAR string")
        data = self.read(n)
        self.skip((8 - n % 8) % 8)
        return data

    def expect(self, s):
        if self.string() != s:
            raise ValueError(f"bad NAR: expected {s!r}")

    def find(self, rel):
        self.expect(b"nix-archive-1")
        return self.node("", rel)

    def collect(self, prefix):
        """Every file under `prefix`: {rel: bytes or ("link", target)}."""
        self.expect(b"nix-archive-1")
        self.found = {}
        self.node("", "\0", prefix)
        return self.found

    def node(self, path, want, prefix=None):
        self.expect(b"(")
        self.expect(b"type")
        kind = self.string()
        result = None
        keep = prefix is not None and path.startswith(prefix)
        if kind == b"regular":
            tag = self.string()
            if tag == b"executable":
                self.string()
                tag = self.string()
            n = self.num()
            if path == want or keep:
                result = self.read(n)
                if keep:
                    self.found[path[len(prefix):]] = result
                    result = None
            else:
                self.skip(n)
            self.skip((8 - n % 8) % 8)
            self.expect(b")")
        elif kind == b"symlink":
            self.expect(b"target")
            target = self.string()
            if keep:
                self.found[path[len(prefix):]] = ("link", target.decode())
            self.expect(b")")
        else:
            while True:
                tag = self.string()
                if tag == b")":
                    break
                self.expect(b"(")
                self.expect(b"name")
                name = self.string().decode()
                self.expect(b"node")
                child = f"{path}/{name}" if path else name
                if prefix is not None:
                    self.node(child, want, prefix if (child + "/").startswith(prefix) or prefix.startswith(child + "/") else None)
                elif want == child or want.startswith(child + "/"):
                    result = self.node(child, want)
                    if result is not None:
                        return result  # stop reading: the rest isn't needed
                else:
                    self.node(child, "\0")
                self.expect(b")")
        return result
