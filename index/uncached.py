"""Free packages the binary cache doesn't have, and which of them nami can
build on the user's machine.

Hydra builds most of nixpkgs, but not what needs software that isn't free
(steam-run, lutris), nor what's marked not to (hydraPlatforms = []: the
thunderbird wrapper, google-fonts), nor the package sets nixpkgs doesn't
walk (rPackages, emacsPackages: index/catalog.nix adds them; Hydra builds
a few hundred of their most used), and some of its builds fail. All but the
failed ones build fine here, from a closure (index/closures.py); a failed
one would likely fail here too, so it's only marked, and `nami check` says
it will be left out.

  uncached.py find <jobs.jsonl> <out dir> [set ...]
      HEAD-checks every free package's out path on cache.nixos.org. Writes
      <out>/uncached.txt (every attr the cache doesn't have) and prints
      "attr TAB drvPath" for the top-level ones and those in the sets
      named (the ones Hydra doesn't build), for closures.py.
  uncached.py find-all <jobs.jsonl> <out dir>
      The same for every package, free or not, and nothing printed: a Mac's
      catalog (nami installs only what's ready-built there) marks each one
      the cache doesn't have.
  uncached.py keep <jobs.jsonl> <out dir> <unfree.tsv> [set ...]
      After closures.py: keeps the closure of each package that needs
      software that isn't free, or that Hydra doesn't build (or is in one
      of the sets named); removes the closures of the ones Hydra failed to
      build (any not in unfree.tsv).
"""
import concurrent.futures
import glob
import http.client
import json
import os
import sys
import threading
import time

local = threading.local()


def status(h):
    """200 or 404 for a store path. Anything else (a dropped connection, a 5xx, a 429) is
    asked again, waiting longer each time: a blip once left 31 of an Intel Mac's packages the
    cache hasn't got unmarked."""
    for attempt in range(8):
        try:
            c = getattr(local, "c", None)
            if c is None:
                c = local.c = http.client.HTTPSConnection("cache.nixos.org", timeout=30)
            c.request("HEAD", f"/{h}.narinfo")
            r = c.getresponse()
            r.read()
            if r.status in (200, 404):
                return r.status
        except Exception:
            pass
        local.c = None
        time.sleep(min(60, 2 ** attempt))
    raise RuntimeError(f"cache.nixos.org didn't answer for {h}")


def jobs(path):
    for line in open(path):
        j = json.loads(line)
        if j.get("attr") and j.get("outputs"):
            yield j


def in_sets(attr, sets):
    return attr.split(".", 1)[0] in sets and "." in attr


def find(jobs_file, out, sets=(), every=False):
    todo = []
    for j in jobs(jobs_file):
        m = j.get("meta") or {}
        if m.get("unfree") and not every:
            continue
        p = j["outputs"].get("out") or next(iter(j["outputs"].values()), None)
        if p:
            todo.append((j["attr"], j.get("drvPath"), p.rsplit("/", 1)[-1].split("-", 1)[0]))
    with concurrent.futures.ThreadPoolExecutor(64) as ex:
        found = list(ex.map(lambda t: status(t[2]), todo))
    missing = [(a, d) for (a, d, _), st in zip(todo, found) if st == 404]
    with open(os.path.join(out, "uncached.txt"), "w") as f:
        f.writelines(f"{a}\n" for a, _ in missing)
    for a, d in missing:
        if every:
            break
        if d and ("." not in a or in_sets(a, sets)):
            print(f"{a}\t{d}")
    print(f"{len(todo)} {'' if every else 'free '}packages, {len(missing)} not in the binary cache", file=sys.stderr)


def keep(jobs_file, out, unfree_tsv, sets=()):
    unfree_attrs = {line.split("\t", 1)[0] for line in open(unfree_tsv) if line.strip()}
    unfree_outs, not_for_hydra = set(), set()
    for j in jobs(jobs_file):
        m = j.get("meta") or {}
        if m.get("unfree"):
            unfree_outs.update(p.rsplit("/", 1)[-1] for p in j["outputs"].values() if p)
        if m.get("hydraPlatforms") == []:
            not_for_hydra.add(j["attr"])
    kept = dropped = 0
    for f in glob.glob(os.path.join(out, "drvs", "*.closure.json")):
        attr = os.path.basename(f)[: -len(".closure.json")]
        if attr in unfree_attrs:
            continue
        if in_sets(attr, sets):
            kept += 1
            continue
        c = json.load(open(f))
        outs = {o["path"].rsplit("/", 1)[-1] for v in c["drvs"].values() for o in (v.get("outputs") or {}).values() if isinstance(o, dict) and o.get("path")}
        if outs & unfree_outs or attr in not_for_hydra:
            kept += 1
        else:
            os.remove(f)
            dropped += 1
    print(f"{kept} free packages nami can build here; {dropped} that Hydra failed to build, left out", file=sys.stderr)


if __name__ == "__main__":
    cmd = sys.argv[1] if len(sys.argv) > 1 else ""
    if cmd == "find-all" and len(sys.argv) == 4:
        find(sys.argv[2], sys.argv[3], every=True)
    elif cmd == "find" and len(sys.argv) >= 4:
        find(sys.argv[2], sys.argv[3], set(sys.argv[4:]))
    elif cmd == "keep" and len(sys.argv) >= 5:
        keep(sys.argv[2], sys.argv[3], sys.argv[4], set(sys.argv[5:]))
    else:
        sys.exit(__doc__)
