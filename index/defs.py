"""The catalog's build definitions, as published: a few thousand shards, not
a file per package.

in-container.sh writes one file per package, drvs/<attr>.json (its build
definition, for recipes that `extends:` it), and drvs/<attr>.closure.json
for those built on the computer; closures.py --parts writes the parts that
the closures of packages in sets (rPackages.tidyverse) share, once each, to
closure-parts.jsonl. A file each would be some 200,000 files per catalog,
each a write to the host (R2 allows a million a month), so they go into
4096 shards by what they hold:

  defs/<first 3 hex digits of its sha256>.json.xz   {"<sha256>": "<text>", ...}

The signed index lists each package's definition by its sha256 (`drv`),
and its closure (`closure`); a closure in parts lists its parts the same
way. nami fetches the one shard it needs, takes the text out and checks its
sha256, as it did a file's. The same text is there once (python3Packages.x
is python313Packages.x).

A closure in one piece (a package of its own: steam, or unfree: CUDA's)
can be megabytes; those stay files of their own, in drvs/, where every nami
version looks for them.

usage: python3 -I index/defs.py <out dir> [<drvs dir to publish>]
  reads  <out>/drvs/, <out>/closure-parts.jsonl
  writes <out>/defs/; and links the closures in one piece into <drvs dir to publish>
"""
import hashlib
import json
import lzma
import os
import shutil
import sys

SHARD_DIGITS = 3


def sha256(text):
    return hashlib.sha256(text.encode()).hexdigest()


def whole_closure(text):
    """A closure in one piece (not one in parts: closures.py --parts)."""
    return '"drvs_by_sha256"' not in text


def places(out):
    """Where each text the shards hold is: (sha256, file, offset of its
    line in closure-parts.jsonl or None). Definitions, closures in parts,
    parts. (Only where: all of them at once would be a gigabyte.)"""
    drvs = os.path.join(out, "drvs")
    for name in sorted(os.listdir(drvs)):
        if not name.endswith(".json"):
            continue
        path = os.path.join(drvs, name)
        with open(path, "rb") as f:
            text = f.read().decode()
        if name.endswith(".closure.json") and whole_closure(text):
            continue
        yield sha256(text), path, None
    parts = os.path.join(out, "closure-parts.jsonl")
    if os.path.exists(parts):
        with open(parts, "rb") as f:
            while True:
                at = f.tell()
                line = f.readline()
                if not line:
                    break
                if line.strip():
                    yield json.loads(line)[0], parts, at


def read(path, at):
    with open(path, "rb") as f:
        if at is None:
            return f.read().decode()
        f.seek(at)
        return json.loads(f.readline())[1]


def write(out):
    shards = {}
    for h, path, at in places(out):
        shards.setdefault(h[:SHARD_DIGITS], {})[h] = (path, at)
    dest = os.path.join(out, "defs")
    shutil.rmtree(dest, ignore_errors=True)
    os.makedirs(dest)
    for s, held in shards.items():
        texts = {h: read(*where) for h, where in held.items()}
        data = json.dumps(texts, separators=(",", ":"), sort_keys=True, ensure_ascii=False).encode()
        with open(os.path.join(dest, f"{s}.json.xz"), "wb") as f:
            f.write(lzma.compress(data, preset=6))
    return sum(len(h) for h in shards.values()), len(shards)


def link_whole_closures(out, dest):
    """The closures in one piece, into the published drvs/ (hard links where it can)."""
    drvs = os.path.join(out, "drvs")
    os.makedirs(dest, exist_ok=True)
    n = 0
    for name in sorted(os.listdir(drvs)):
        if not name.endswith(".closure.json"):
            continue
        src = os.path.join(drvs, name)
        with open(src, "rb") as f:
            if not whole_closure(f.read().decode()):
                continue
        try:
            os.link(src, os.path.join(dest, name))
        except OSError:
            shutil.copyfile(src, os.path.join(dest, name))
        n += 1
    return n


if __name__ == "__main__":
    if len(sys.argv) not in (2, 3):
        sys.exit(__doc__)
    n, files = write(sys.argv[1])
    print(f"{n} build definitions and closure parts, in {files} shards (defs/)", file=sys.stderr)
    if len(sys.argv) == 3:
        print(f"{link_whole_closures(sys.argv[1], sys.argv[2])} closures in one piece (drvs/)", file=sys.stderr)
