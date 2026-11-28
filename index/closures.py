"""Build closures for packages the binary cache doesn't have (unfree ones:
Hydra doesn't build them), so nami can build them on the user's machine.

Runs inside the nixos/nix container (in-container.sh), where every
derivation is instantiated. For each attr it writes /out/drvs/<attr>.closure.json:

  {"top": "<drv>",
   "drvs":   {"<drv>": <derivation JSON>, ...},  every derivation that has to be built
   "cached": {"<drv>": {"out": "<path>"}, ...}, inputs to download instead
   "srcs":   {"<path>": "<base64 NAR>", ...}}    plain files (patches, scripts) not in the cache

A derivation goes in "drvs" when some output it has to provide isn't on
cache.nixos.org; the walk stops at inputs that are.

With --parts, a package in a set (rPackages.tidyverse) gets its closure in
parts instead: tens of thousands of them share most of what they build, so each
part is kept once, by its sha256, in <out dir>/closure-parts.jsonl
(["<sha256>", "<text>"] a line; index/defs.py puts them in the catalog's
defs/), and the closure names them:

  {"top": "<drv>", "cached": {...},
   "drvs_by_sha256": {"<drv>": "<sha256 of its derivation JSON>", ...},
   "srcs_by_sha256": {"<path>": "<sha256 of its base64 NAR>", ...}}

usage: closures.py [--parts] <attrs.tsv: attr TAB drvPath> <out dir>
"""
import base64
import hashlib
import json
import os
import subprocess
import sys
import threading
import urllib.request
from concurrent.futures import ThreadPoolExecutor

args = sys.argv[1:]
in_parts_too = args[:1] == ["--parts"]
attrs_file, out = (args[in_parts_too:] + [None, None])[:2]
cached_memo = {}


def in_cache(path):
    """Is this store path (basename) on cache.nixos.org?"""
    h = path.split("-", 1)[0]
    if h not in cached_memo:
        try:
            req = urllib.request.Request(f"https://cache.nixos.org/{h}.narinfo", method="HEAD")
            cached_memo[h] = urllib.request.urlopen(req, timeout=30).status == 200
        except Exception:
            cached_memo[h] = False
    return cached_memo[h]


drv_memo = {}


def show(drvs):
    """Derivation JSON for each drv basename (batched `nix derivation show`)."""
    need = [d for d in drvs if d not in drv_memo]
    for i in range(0, len(need), 200):
        chunk = need[i:i + 200]
        res = subprocess.run(["nix", "derivation", "show", *[f"/nix/store/{d}" for d in chunk]], capture_output=True, text=True)
        if res.returncode == 0:
            data = json.loads(res.stdout)
            data = data.get("derivations", data)
            for k, v in data.items():
                drv_memo[k.removeprefix("/nix/store/")] = v
    return {d: drv_memo.get(d) for d in drvs}


def outputs_of(drv):
    """Output name -> path. Fixed-output (download) steps don't list their
    path under outputs; it's in their environment."""
    env = drv.get("env") or {}
    paths = {o: v.get("path") or env.get(o, "") for o, v in drv["outputs"].items()}
    return {o: p.removeprefix("/nix/store/") for o, p in paths.items() if p}


def closure(top):
    todo, drvs, cached, srcs = [top], {}, {}, {}
    while todo:
        infos = show(todo)
        todo = []
        for d, drv in infos.items():
            if drv is None:
                raise RuntimeError(f"can't read {d}")
            drvs[d] = drv
            for s in drv["inputs"]["srcs"]:
                if s not in srcs and not in_cache(s):
                    srcs[s] = None
            deps = show(list(drv["inputs"]["drvs"]))
            for dep, use in drv["inputs"]["drvs"].items():
                if dep in drvs or dep in cached or dep in todo:
                    continue
                outs = outputs_of(deps[dep])
                wanted = [outs[o] for o in use["outputs"] if o in outs]
                if all(in_cache(p) for p in wanted):
                    cached[dep] = outs
                else:
                    todo.append(dep)
    for s in srcs:
        # Through the store, not the file system: a user's own store
        # (build-index.sh with nami) keeps its /nix/store paths elsewhere.
        nar = subprocess.run(["nix", "store", "dump-path", f"/nix/store/{s}"], capture_output=True, check=True).stdout
        srcs[s] = base64.b64encode(nar).decode()
    return {"top": top, "drvs": drvs, "cached": cached, "srcs": srcs}


def one(line):
    attr, drv = line.split("\t")
    try:
        top = drv.removeprefix("/nix/store/")
        outs = outputs_of(show([top])[top])
        wanted = outs.get("bin") or outs.get("out") or next(iter(outs.values()), "")
        if wanted and in_cache(wanted):
            return attr, "in the binary cache"
        c = closure(top)
    except Exception as e:
        return attr, f"skipped: {e}"
    what = f"{len(c['drvs'])} to build, {len(c['srcs'])} sources"
    if in_parts_too and "." in attr:
        c = in_parts(c)
    with open(os.path.join(out, "drvs", f"{attr}.closure.json"), "w") as f:
        json.dump(c, f, separators=(",", ":"), sort_keys=True)
    return attr, what


parts_lock = threading.Lock()
parts_seen = set()
parts = None


def in_parts(c):
    """A closure that names its parts by sha256; the parts go (once) into
    closure-parts.jsonl."""
    def part(text):
        sha = hashlib.sha256(text.encode()).hexdigest()
        with parts_lock:
            if sha not in parts_seen:
                parts_seen.add(sha)
                parts.write(json.dumps([sha, text]) + "\n")
        return sha
    return {"top": c["top"], "cached": c["cached"],
            "drvs_by_sha256": {d: part(json.dumps(v, separators=(",", ":"), sort_keys=True)) for d, v in c["drvs"].items()},
            "srcs_by_sha256": {p: part(b) for p, b in c["srcs"].items()}}


if __name__ == "__main__":
    lines = [l.rstrip("\n") for l in open(attrs_file) if l.strip()]
    # (Added to: closures.py runs once for unfree packages, once for uncached ones.)
    parts_file = os.path.join(out, "closure-parts.jsonl")
    if os.path.exists(parts_file):
        parts_seen.update(json.loads(l)[0] for l in open(parts_file) if l.strip())
    with open(parts_file, "a") as parts, ThreadPoolExecutor(8) as pool:
        for attr, what in pool.map(one, lines):
            print(f"{attr}: {what}", flush=True)
