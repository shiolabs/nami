"""Every version each package has had, for `firefox: 150` in a nami file.

The catalog is one nixpkgs snapshot: one version of everything. This adds
every release before it: nixos-unstable's (every few days, since 2016) and
every stable channel's (16.03 on, with their backports), about 5000.

Evaluating each in full would take weeks, so:
  1. each release's packages.json (every package's version, no evaluation)
     is read: every package's version (also ones since removed);
  2. going from the newest release to the oldest, a version not seen in a
     newer one is new there: its newest build (the newest libraries);
  3. each release with new versions is evaluated for just those packages
     (their output paths), several at a time. Releases from before
     packages.json existed are evaluated in full.
Every step is kept in <history-dir>, so a run picks up where one stopped,
and a new catalog only adds its own release. A release today's Nix can't
evaluate is noted and skipped.

Release lists (packages.json, store-paths) alone aren't enough: they don't
say which build is which package's, and two builds can share a name. The
builds are in cache.nixos.org, which keeps every build.

usage: python3 index/history.py <history-dir> [workers]
  writes <history-dir>/versions-x86_64-linux.json:
    attr -> [[version, date, nixpkgs revision, {output: path}], ...], newest first
"""
import concurrent.futures
import json
import lzma
import os
import re
import resource
import shutil
import subprocess
import sys
import tempfile
import threading
import time
import urllib.error
import urllib.request
import xml.etree.ElementTree as ET
from datetime import datetime, timezone

BUCKET = "https://nix-releases.s3.amazonaws.com"
RELEASES = "https://releases.nixos.org"
NS = {"s3": "http://s3.amazonaws.com/doc/2006-03-01/"}

# Just the asked-for packages (attribute paths, from attrs.json), at a
# release unpacked in `src`. recurseIntoAttrs moved between pkgs and lib.
SOME = """{ src }:
let pkgs = import src { system = "x86_64-linux"; config.allowUnfree = true; };
    lib = pkgs.lib;
    get = a: let r = builtins.tryEval (let d = lib.attrByPath (lib.splitString "." a) null pkgs;
                                       in if d != null && d ? outPath then builtins.seq d.outPath d else null);
             in if r.success then r.value else null;
    picked = map (a: { name = a; value = get a; }) (builtins.fromJSON (builtins.readFile ./attrs.json));
in builtins.listToAttrs (builtins.filter (x: x.value != null) picked)
"""
# All of a release's packages (the catalog's set), for releases without packages.json.
ALL = """{ src }:
let pkgs = import src { system = "x86_64-linux"; config.allowUnfree = true; };
    recurse = pkgs.recurseIntoAttrs or pkgs.lib.recurseIntoAttrs;
in pkgs // { python3Packages = recurse pkgs.python3Packages; perlPackages = recurse pkgs.perlPackages; }
"""


def get(url, timeout=120):
    # The network can drop for a while (DNS, Wi-Fi): wait it out, up to
    # about 20 minutes, rather than lose a run of hours. Not found is final.
    for wait in [5, 15, 30, 60, 120, 300, 600, 0]:
        try:
            with urllib.request.urlopen(url, timeout=timeout) as r:
                return r.read()
        except urllib.error.HTTPError as e:
            if e.code < 500 or not wait:
                raise
        except (urllib.error.URLError, OSError):
            if not wait:
                raise
        time.sleep(wait)


def listing(prefix):
    """The release directories under `prefix` (nixos/unstable/), by name."""
    out, marker = [], ""
    while True:
        url = f"{BUCKET}/?prefix={prefix}&delimiter=/" + (f"&marker={marker}" if marker else "")
        root = ET.fromstring(get(url, 60))
        prefixes = [p.text for p in root.findall("s3:CommonPrefixes/s3:Prefix", NS)]
        out += [p.rstrip("/").rsplit("/", 1)[-1] for p in prefixes]
        if root.findtext("s3:IsTruncated", namespaces=NS) != "true" or not prefixes:
            return out
        marker = prefixes[-1]


def releases(cache):
    """Every release: [channel, name, date], newest first (kept in releases.json,
    so only new ones are looked up again)."""
    path = os.path.join(cache, "releases.json")
    known = {r[1]: r for r in json.load(open(path))} if os.path.exists(path) else {}
    channels = [c.rstrip("/").rsplit("/", 1)[-1] for c in listing("nixos/")]
    channels = [c for c in channels if c == "unstable" or re.fullmatch(r"\d\d\.\d\d", c)]
    todo = []
    for c in channels:
        for n in listing(f"nixos/{c}/"):
            if re.fullmatch(r"nixos-\d\d\.\d\d(pre)?[\d.]+[0-9a-f]{7,40}", n) and n not in known:
                todo.append((c, n))

    def dated(cn):
        c, n = cn
        req = urllib.request.Request(f"{RELEASES}/nixos/{c}/{n}/nixexprs.tar.xz", method="HEAD")
        try:
            with urllib.request.urlopen(req, timeout=60) as r:
                when = datetime.strptime(r.headers["Last-Modified"], "%a, %d %b %Y %H:%M:%S %Z").replace(tzinfo=timezone.utc)
            return [c, n, when.strftime("%Y-%m-%d %H:%M")]
        except Exception:
            return None

    with concurrent.futures.ThreadPoolExecutor(16) as pool:
        for r in pool.map(dated, todo):
            if r:
                known[r[1]] = r
    out = sorted(known.values(), key=lambda r: r[2], reverse=True)
    with open(path + ".part", "w") as f:
        json.dump(out, f)
    os.replace(path + ".part", path)
    return out


def rev_of(name):
    return name.rsplit(".", 1)[-1]


def stored(path):
    if os.path.exists(path):
        with lzma.open(path, "rt") as f:
            return json.load(f)
    return None


def store(path, value):
    with lzma.open(path + ".part", "wt") as f:
        json.dump(value, f, separators=(",", ":"))
    os.replace(path + ".part", path)


def versions_of(rel, cache):
    """Every package of a release and its version, from its packages.json,
    kept in versions-all/; returns whether it has one (older releases don't)."""
    c, n, _ = rel
    path = os.path.join(cache, "versions-all", f"{n}.json.xz")
    if os.path.exists(path):
        return stored(path).get("packages") is not None
    try:
        raw = get(f"{RELEASES}/nixos/{c}/{n}/packages.json.br")
        data = json.loads(subprocess.run(["brotli", "-dc"], input=raw, stdout=subprocess.PIPE, check=True).stdout)
        pkgs = {a: p["version"] for a, p in data["packages"].items() if p.get("version")}
    except urllib.error.HTTPError as e:
        if e.code != 404:
            raise
        pkgs = None
    store(path, {"packages": pkgs})
    return pkgs is not None


def release_versions(rel, cache):
    return stored(os.path.join(cache, "versions-all", f"{rel[1]}.json.xz")).get("packages")


_missing = None


def _init_missing(cache, missing):
    global _missing, _cache
    _missing, _cache = missing, cache


def _missing_in(rel):
    """Which of the missing versions `rel` has."""
    versions = release_versions(rel, _cache) if os.path.exists(os.path.join(_cache, "versions-all", f"{rel[1]}.json.xz")) else None
    return [(a, v) for a, v in (versions or {}).items() if (a, v) in _missing]


# Evaluating a whole release takes a few GB more: ten of those at a time.
_whole = threading.Semaphore(10)


def _versions_of(args):
    return versions_of(*args)


# Unpacked releases in /tmp at once: a nixpkgs is up to 90,000 files, and
# a RAM folder holds only so many (a million, here).
_in_ram = threading.BoundedSemaphore(6)


def _tmp(cache):
    """Where to unpack a release: /tmp if it's RAM with room (one of six
    places there), else the cache folder on disk. Returns (folder, in_ram)."""
    if _in_ram.acquire(blocking=False):
        try:
            st = os.statvfs("/tmp")
            if st.f_bavail * st.f_frsize > 4 * 2**30 and st.f_favail > 300_000:
                os.makedirs("/tmp/nami-history", exist_ok=True)
                return "/tmp/nami-history", True
        except OSError:
            pass
        _in_ram.release()
    return os.path.join(cache, "tmp"), False


def deep_stack():
    hard = resource.getrlimit(resource.RLIMIT_STACK)[1]
    resource.setrlimit(resource.RLIMIT_STACK, (hard, hard))


def nix_env(tmp, expr, src, attrs):
    """nix-env's packages for `attrs` (all, if None): (True, {attr: info}), or
    (False, why). One broken package (an error tryEval can't catch) fails
    them all, so a failed list is tried again in halves, leaving out just
    the broken ones."""
    if attrs is not None:
        with open(os.path.join(tmp, "attrs.json"), "w") as f:
            json.dump(attrs, f)
    r = subprocess.run(
        ["nix-env", "-f", expr, "--arg", "src", src, "-qa", "--json", "--out-path"],
        stdout=subprocess.PIPE, stderr=subprocess.PIPE, preexec_fn=deep_stack,
        # Older nixpkgs goes deeper than today's Nix allows by default.
        env={**os.environ, "NIX_CONFIG": "experimental-features = nix-command flakes\nmax-call-depth = 1000000"},
    )
    if r.returncode == 0:
        return True, json.loads(r.stdout)
    errors = [l for l in r.stderr.decode(errors="replace").splitlines() if "error" in l.lower()]
    why = errors[-1].strip() if errors else "no message"
    if attrs is None or len(attrs) == 1:
        return False, why
    half = len(attrs) // 2
    found = {}
    for part in (attrs[:half], attrs[half:]):
        ok, got = nix_env(tmp, expr, src, part)
        if ok:
            found.update(got)
    return True, found


def evaluate(rel, attrs, cache):
    """Output paths of `attrs` (all, if None) at release `rel`: attr -> [version, {output: path}]."""
    c, n, _ = rel
    # What was evaluated there before is kept (builds/), with the packages
    # asked for (asked/): only ones not asked before are evaluated now.
    path = os.path.join(cache, "builds", f"{n}.json.xz")
    asked_path = os.path.join(cache, "asked", f"{n}.json.xz")
    have = stored(path)
    if have is not None:
        if attrs is None:
            return have
        asked = set(stored(asked_path) or have)
        attrs = [a for a in attrs if a not in asked]
        if not attrs:
            return have
    if os.path.exists(os.path.join(cache, "failed", n)):
        return have or {}
    # Unpacked in memory (/tmp, if it's RAM), as it's thousands of small files;
    # fetched, unpacked and removed by curl, tar and rm, which (unlike this
    # program's threads) work in parallel.
    where, in_ram = _tmp(cache)
    tmp = tempfile.mkdtemp(dir=where)
    try:
        got = subprocess.run(["curl", "-fsS", "--http1.1", "--retry", "5", "--retry-all-errors",
                              "-o", os.path.join(tmp, "nixexprs.tar.xz"), f"{RELEASES}/nixos/{c}/{n}/nixexprs.tar.xz"])
        if got.returncode != 0:
            # Not noted as failed: the next run tries again.
            print(f"skipping {n} for now: it didn't download", file=sys.stderr, flush=True)
            return have or {}
        # tar, not tarfile: Python unpacks one at a time, tar in parallel.
        os.makedirs(os.path.join(tmp, "src"))
        unpacked = subprocess.run(["tar", "-xJf", os.path.join(tmp, "nixexprs.tar.xz"), "-C", os.path.join(tmp, "src"),
                                   "--no-same-owner", "--no-same-permissions"], stderr=subprocess.PIPE)
        if unpacked.returncode != 0:
            why = unpacked.stderr.decode(errors="replace").strip().splitlines()[-1:]
            print(f"skipping {n} for now: it didn't unpack ({' '.join(why)})", file=sys.stderr, flush=True)
            return have or {}
        src = next(os.scandir(os.path.join(tmp, "src"))).path
        expr = os.path.join(tmp, "release.nix")
        with open(expr, "w") as f:
            f.write(ALL if attrs is None else SOME)
        ok, found = nix_env(tmp, expr, src, sorted(attrs) if attrs is not None else None)
    finally:
        subprocess.run(["rm", "-rf", tmp])
        if in_ram:
            _in_ram.release()
    if not ok:
        with open(os.path.join(cache, "failed", n), "w") as f:
            f.write(found + "\n")
        print(f"skipping {n}: it doesn't evaluate ({found})", file=sys.stderr, flush=True)
        return {}
    builds = dict(have or {})
    for attr, p in found.items():
        outs = {o: v for o, v in (p.get("outputs") or {}).items() if v}
        version = p.get("version") or ""
        if not version:
            m = re.search(r"-(\d[^-]*(?:-.*)?)$", p.get("name", ""))
            version = m.group(1) if m else ""
        if version and outs:
            builds[attr] = [version, outs]
    store(path, builds)
    if attrs is not None:
        store(asked_path, sorted(set(attrs) | set(stored(asked_path) or have or {})))
    return builds


def _shard_names():
    return [f"{i:03x}" for i in range(4096)]


def _read(source, path):
    """A published catalog's file: from a folder, or a URL (None if it isn't there)."""
    if "://" not in source:
        p = os.path.join(source, path)
        return open(p, "rb").read() if os.path.exists(p) else None
    try:
        # (The catalog's host turns away Python's own User-Agent.)
        return get(urllib.request.Request(f"{source.rstrip('/')}/{path}", headers={"User-Agent": "nami-index"}))
    except urllib.error.HTTPError as e:
        if e.code == 404:
            return None
        raise


def seed(cache, source):
    """Starts <history-dir> from a published catalog's histories (its
    versions/ shards, `source` a menu/<rev> folder or URL), so a new
    catalog evaluates only the releases after that one: minutes, not the
    day or two of a first run. Every release up to and including the
    catalog's own counts as looked at.

    The shards leave out each package's own version (index/versions.py):
    it comes back from the catalog's index, at its release, for the
    packages that release's packages.json lists (as a full run has them).
    Kept in seed.json and seed-versions.json.xz; main() adds the newer
    releases to them. Returns False when the catalog has no histories."""
    rev = source.rstrip("/").rsplit("/", 1)[-1]
    with concurrent.futures.ThreadPoolExecutor(32) as pool:
        raw = list(pool.map(lambda s: _read(source, f"versions/{s}.json"), _shard_names()))
    if not any(raw):
        print(f"{source} has no version histories", file=sys.stderr)
        return False
    versions = {}
    for r in raw:
        if r:
            versions.update(json.loads(r))
    del raw
    index = _read(source, "index-x86_64-linux.json.xz")
    index = json.loads(lzma.decompress(index)) if index else json.loads(_read(source, "index-x86_64-linux.json"))
    for d in ("versions-all", "builds", "asked", "failed", "tmp"):
        os.makedirs(os.path.join(cache, d), exist_ok=True)
    rel = next((r for r in releases(cache) if r[0] == "unstable" and rev_of(r[1]) == rev), None)
    if rel is None:
        sys.exit(f"no nixos-unstable release {rev}")
    versions_of(rel, cache)
    listed = release_versions(rel, cache) or {}
    try:
        sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
        from versions import version_of
    finally:
        sys.path.pop(0)
    back = 0
    for attr, version in listed.items():
        p = index.get(attr)
        if not p or not p.get("outputs") or version != version_of(p.get("name", "")):
            continue
        history = versions.setdefault(attr, [])
        if any(e[0] == version for e in history):
            continue
        history.append([version, rel[2][:10], rev, p["outputs"]])
        history.sort(key=lambda e: e[1], reverse=True)
        back += 1
    store(os.path.join(cache, "seed-versions.json.xz"), versions)
    with open(os.path.join(cache, "seed.json"), "w") as f:
        json.dump({"source": source, "release": rel[1], "date": rel[2]}, f)
    write_result(cache, versions)
    print(f"seeded from {source}: {len(versions)} packages, {back} of their own versions back; "
          f"releases up to {rel[1]} count as looked at", file=sys.stderr)
    return True


def write_result(cache, result):
    with open(os.path.join(cache, "versions-x86_64-linux.json.part"), "w") as f:
        json.dump(result, f, separators=(",", ":"), sort_keys=True)
    os.replace(os.path.join(cache, "versions-x86_64-linux.json.part"), os.path.join(cache, "versions-x86_64-linux.json"))


def main():
    if sys.argv[1] == "--seed":
        sys.exit(0 if seed(sys.argv[3], sys.argv[2]) else 3)
    cache = sys.argv[1]
    mem_gb = os.sysconf("SC_PAGE_SIZE") * os.sysconf("SC_PHYS_PAGES") // 2**30
    # An evaluation of just some packages takes a few GB; the oldest, in full, more.
    workers = int(sys.argv[2]) if len(sys.argv) > 2 else max(1, min(os.cpu_count() * 3 // 4, mem_gb // 4))
    for d in ("versions-all", "builds", "asked", "failed", "tmp"):
        os.makedirs(os.path.join(cache, d), exist_ok=True)
    rels = releases(cache)
    # Seeded (seed()): only the releases after the seed's are looked at;
    # what they find goes over the seed's histories.
    seeded = None
    if os.path.exists(os.path.join(cache, "seed.json")):
        s = json.load(open(os.path.join(cache, "seed.json")))
        seeded = stored(os.path.join(cache, "seed-versions.json.xz"))
        rels = [r for r in rels if r[2] > s["date"]]
        print(f"seeded up to {s['release']}: {len(rels)} newer releases", file=sys.stderr, flush=True)
        if not rels:
            write_result(cache, seeded)
            return
    print(f"{len(rels)} releases, {rels[-1][1]} .. {rels[0][1]}", file=sys.stderr, flush=True)

    # 1. Versions, from each release's packages.json: in processes (reading
    # the JSON is the slow part, and threads would take turns), `workers` at
    # a time, as each takes a couple of GB while it reads.
    with concurrent.futures.ProcessPoolExecutor(workers) as pool:
        listed = list(pool.map(_versions_of, [(r, cache) for r in rels], chunksize=4))

    # 2. Newest to oldest: the versions new in each release (every package
    # there ever was, also ones since removed: nodejs_20, python38). One
    # release at a time, so only what's been seen is kept in memory.
    seen, jobs = set(), []
    for rel, has in zip(rels, listed):
        if not has:
            continue
        new = []
        for a, ver in release_versions(rel, cache).items():
            if (a, ver) not in seen:
                seen.add((a, ver))
                new.append((a, ver))
        if new:
            jobs.append((rel, new))
    # Releases without packages.json: evaluated in full (the oldest ones).
    jobs += [(rel, None) for rel, has in zip(rels, listed) if not has]
    del seen
    print(f"{len(jobs)} releases to evaluate, {workers} at a time", file=sys.stderr, flush=True)

    # 3. Their output paths.
    done = 0

    def one(job):
        rel, pairs = job
        if pairs is None:
            with _whole:
                return evaluate(rel, None, cache)
        return evaluate(rel, [a for a, _ in pairs], cache)

    found, missing = {}, set()

    def keep(rel, builds):
        for attr, (version, outs) in builds.items():
            found.setdefault(attr, {}).setdefault(version, []).append([version, rel[2][:10], rev_of(rel[1]), outs])

    with concurrent.futures.ThreadPoolExecutor(workers) as pool:
        for (rel, pairs), builds in zip(jobs, pool.map(one, jobs)):
            done += 1
            if done % 25 == 0:
                print(f"  {done}/{len(jobs)}", file=sys.stderr, flush=True)
            keep(rel, builds)
            # (Top-level packages: in package sets, a missing one is mostly
            # one never built there, at any release.)
            missing.update(p for p in pairs or [] if "." not in p[0] and p[0] not in builds)

    # 4. Versions that didn't evaluate in the newest release that has them
    # (nixpkgs had marked them insecure or broken by then, as it does before
    # removing a package: nodejs_16): from the oldest one that has them,
    # from before that (and built then).
    print(f"{len(missing)} versions again, from their first release", file=sys.stderr, flush=True)
    with concurrent.futures.ProcessPoolExecutor(min(workers, 6), initializer=_init_missing, initargs=(cache, missing)) as pool:
        present = list(pool.map(_missing_in, rels, chunksize=4))
    oldest = {}
    for rel, pairs in zip(rels, present):
        for p in pairs:
            oldest[p] = rel
    retry = {}
    for (a, ver), rel in oldest.items():
        retry.setdefault(rel[1], (rel, []))[1].append(a)
    retry = list(retry.values())
    with concurrent.futures.ThreadPoolExecutor(workers) as pool:
        for (rel, attrs), builds in zip(retry, pool.map(lambda j: evaluate(j[0], j[1], cache), retry)):
            keep(rel, {a: b for a, b in builds.items() if a in set(attrs)})
    shutil.rmtree(os.path.join(cache, "tmp"), ignore_errors=True)
    shutil.rmtree("/tmp/nami-history", ignore_errors=True)

    # Each version once: its newest build.
    result = {}
    for attr, by_version in found.items():
        entries = [max(builds, key=lambda e: e[1]) for builds in by_version.values()]
        result[attr] = sorted(entries, key=lambda e: e[1], reverse=True)
    # Over the seed: a version a newer release has is its newer build.
    if seeded is not None:
        for attr, entries in result.items():
            newer = {e[0] for e in entries}
            seeded[attr] = sorted(entries + [e for e in seeded.get(attr, []) if e[0] not in newer],
                                  key=lambda e: e[1], reverse=True)
        result = seeded
    write_result(cache, result)
    n = sum(len(v) for v in result.values())
    print(f"{len(result)} packages, {n} versions", file=sys.stderr)


if __name__ == "__main__":
    main()
