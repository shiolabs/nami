"""Tests for closures.py's closures in parts, and uncached.py's sets Hydra
doesn't build: python3 -I index/closures-test.py"""
import hashlib
import importlib.util
import io
import json
import os
import sys
import tempfile
import unittest

here = os.path.dirname(os.path.abspath(__file__))


def load(name, file):
    spec = importlib.util.spec_from_file_location(name, os.path.join(here, file))
    m = importlib.util.module_from_spec(spec)
    argv, sys.argv = sys.argv, [file]
    try:
        spec.loader.exec_module(m)
    finally:
        sys.argv = argv
    return m


closures = load("closures", "closures.py")
uncached = load("uncached", "uncached.py")
sha = lambda text: hashlib.sha256(text.encode()).hexdigest()


class InParts(unittest.TestCase):
    def test_each_part_once_by_its_sha256(self):
        closures.parts = io.StringIO()
        closures.parts_seen.clear()
        drv = {"env": {"name": "ggplot2"}, "outputs": {"out": {"path": "x"}}}
        whole = {"top": "a.drv", "cached": {"c.drv": {"out": "c"}}, "drvs": {"a.drv": drv, "b.drv": {"env": {}}}, "srcs": {"/nix/store/p": "UEFUQ0g="}}
        one = closures.in_parts(whole)
        again = closures.in_parts({**whole, "top": "b.drv"})
        lines = [json.loads(l) for l in closures.parts.getvalue().splitlines()]
        self.assertEqual(len(lines), 3, "a part two closures share is kept once")
        held = dict(lines)
        for h, text in held.items():
            self.assertEqual(sha(text), h)
        self.assertEqual(json.loads(held[one["drvs_by_sha256"]["a.drv"]]), drv)
        self.assertEqual(held[one["srcs_by_sha256"]["/nix/store/p"]], "UEFUQ0g=")
        self.assertEqual((one["top"], one["cached"], again["top"]), ("a.drv", whole["cached"], "b.drv"))
        self.assertNotIn("drvs", one)


class Sets(unittest.TestCase):
    def test_a_set_hydra_doesnt_build_keeps_its_closures(self):
        sets = {"rPackages"}
        self.assertTrue(uncached.in_sets("rPackages.tidyverse", sets))
        self.assertFalse(uncached.in_sets("rPackages", sets))
        self.assertFalse(uncached.in_sets("haskellPackages.foo", sets))
        with tempfile.TemporaryDirectory() as out:
            os.makedirs(os.path.join(out, "drvs"))
            jobs = os.path.join(out, "jobs.jsonl")
            with open(jobs, "w") as f:
                for attr in ("rPackages.tidyverse", "haskellPackages.failed", "lutris"):
                    f.write(json.dumps({"attr": attr, "outputs": {"out": f"/nix/store/x-{attr}"}, "meta": {}}) + "\n")
            parts = {"top": "t.drv", "cached": {}, "drvs_by_sha256": {}, "srcs_by_sha256": {}}
            whole = {"top": "f.drv", "cached": {}, "drvs": {"f.drv": {"outputs": {"out": {"path": "/nix/store/y-f"}}}}, "srcs": {}}
            for attr, c in (("rPackages.tidyverse", parts), ("haskellPackages.failed", whole)):
                with open(os.path.join(out, "drvs", f"{attr}.closure.json"), "w") as f:
                    json.dump(c, f)
            unfree = os.path.join(out, "unfree.tsv")
            open(unfree, "w").close()
            uncached.keep(jobs, out, unfree, sets)
            self.assertEqual(sorted(os.listdir(os.path.join(out, "drvs"))), ["rPackages.tidyverse.closure.json"])


if __name__ == "__main__":
    unittest.main()
