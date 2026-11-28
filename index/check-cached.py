"""Whether a catalog's packages can be installed from cache.nixos.org: each
package's whole closure (its narinfos' References, followed) is there.
For a Mac's catalog and ARM Linux's, where nothing of the catalog's is built on the
computer. Each store path is asked about once, for every package that has it.

  check-cached.py <index-<system>.json(.xz)> [attr ...]
      Prints each package's outcome; exits 1 if any (not marked uncached)
      has a store path the cache lacks.
"""
import concurrent.futures
import http.client
import json
import lzma
import sys
import threading
import time

local = threading.local()
# What the cache said of each store path asked about: its references, or None when it hasn't got it.
known = {}


def narinfo(h):
    """The cache's answer for a store path: 200 and its narinfo, or 404. Anything else (a
    dropped connection, a 5xx, a 429) is asked again, waiting longer each time: a blip
    once marked a few packages missing and failed a four-hour check."""
    for attempt in range(8):
        try:
            c = getattr(local, "c", None)
            if c is None:
                c = local.c = http.client.HTTPSConnection("cache.nixos.org", timeout=30)
            c.request("GET", f"/{h}.narinfo")
            r = c.getresponse()
            body = r.read().decode()
            if r.status in (200, 404):
                return r.status, body
        except Exception:
            pass
        local.c = None
        time.sleep(min(60, 2 ** attempt))
    raise RuntimeError(f"cache.nixos.org didn't answer for {h}")


def references(base):
    status, body = narinfo(base.split("-", 1)[0])
    if status == 404:
        return None
    for line in body.splitlines():
        if line.startswith("References:"):
            return line.split()[1:]
    return []


def closure(paths, ex):
    seen, missing = set(), []
    frontier = {p.rsplit("/", 1)[-1] for p in paths}
    while frontier:
        seen |= frontier
        todo = sorted(b for b in frontier if b not in known)
        for base, refs in zip(todo, ex.map(references, todo)):
            known[base] = refs
        next_ = set()
        for base in sorted(frontier):
            refs = known[base]
            if refs is None:
                missing.append(base)
            else:
                next_ |= {r for r in refs if r not in seen}
        frontier = next_
    return seen, missing


def main():
    if len(sys.argv) < 2:
        sys.exit(__doc__)
    f = sys.argv[1]
    data = lzma.open(f).read() if f.endswith(".xz") else open(f, "rb").read()
    index = json.loads(data)
    attrs = sys.argv[2:] or sorted(index)
    bad = 0
    ex = concurrent.futures.ThreadPoolExecutor(32)
    for a in attrs:
        p = index[a]
        outs = [v for v in p["outputs"].values() if v]
        seen, missing = closure(outs, ex)
        if missing:
            mark = "(marked uncached)" if p.get("uncached") else "MISSING"
            bad += 0 if p.get("uncached") else 1
            print(f"{a}: {mark}: {len(missing)} of {len(seen)} paths not in the cache, e.g. {missing[0]}")
        else:
            print(f"{a}: ok, {len(seen)} paths")
    sys.exit(1 if bad else 0)


if __name__ == "__main__":
    main()
