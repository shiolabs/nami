"""The catalog's version histories, for `firefox: 150` in a nami file:
looked up when someone asks for a version, one small piece at a time, so
the catalog itself doesn't grow.

The histories are split into 4096 shards by the first three hex digits of
sha256(attr): versions/<shard>.json is {attr: [[version, date, rev,
{output: path}], ...]}, newest first, for the attrs in that shard. Each is
signed with the catalog's key at publishing (nami catalog sign <dir>/versions),
so the server needn't be trusted. A package's own version (the catalog's)
isn't repeated. Packages since removed from nixpkgs (nodejs_20) are here
too, with only their history.

Run after index/history.py (build-index.sh does).

usage: python3 index/versions.py <out-dir> <history-dir>
  reads  <out-dir>/index-x86_64-linux.json, <history-dir>/versions-x86_64-linux.json
  writes <out-dir>/versions/<shard>.json
"""
import hashlib
import json
import os
import shutil
import sys


def version_of(name):
    """firefox-157.0.1 -> 157.0.1 (as nami's query::version_of)."""
    for i, c in enumerate(name):
        if c == "-" and name[i + 1:i + 2].isdigit():
            v = name[i + 1:]
            for s in ("-wrapped", "-unwrapped"):
                if v.endswith(s):
                    v = v[: -len(s)]
            return v
    return ""


def shard(attr):
    return hashlib.sha256(attr.encode()).hexdigest()[:3]


def main():
    out, history = sys.argv[1], sys.argv[2]
    with open(os.path.join(out, "index-x86_64-linux.json")) as f:
        index = json.load(f)
    with open(os.path.join(history, "versions-x86_64-linux.json")) as f:
        versions = json.load(f)
    # Every package there has been: also ones since removed (nodejs_20),
    # which have only their history.
    shards = {}
    for attr, history in versions.items():
        own = version_of(index[attr].get("name", "")) if attr in index else None
        earlier = [v for v in history if v[0] != own]
        if earlier:
            shards.setdefault(shard(attr), {})[attr] = earlier
    vdir = os.path.join(out, "versions")
    shutil.rmtree(vdir, ignore_errors=True)
    os.makedirs(vdir)
    for s, attrs in shards.items():
        with open(os.path.join(vdir, f"{s}.json"), "w") as f:
            json.dump(attrs, f, separators=(",", ":"), sort_keys=True)
    n = sum(len(a) for a in shards.values())
    print(f"version histories for {n} packages, in {len(shards)} shards", file=sys.stderr)


if __name__ == "__main__":
    main()
