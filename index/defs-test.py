"""Tests for defs.py: python3 -I index/defs-test.py"""
import hashlib
import importlib.util
import json
import lzma
import os
import tempfile
import unittest

spec = importlib.util.spec_from_file_location("defs", os.path.join(os.path.dirname(os.path.abspath(__file__)), "defs.py"))
defs = importlib.util.module_from_spec(spec)
spec.loader.exec_module(defs)

sha = lambda text: hashlib.sha256(text.encode()).hexdigest()


def shards(out):
    held = {}
    for name in os.listdir(os.path.join(out, "defs")):
        with open(os.path.join(out, "defs", name), "rb") as f:
            held.update(json.loads(lzma.decompress(f.read())))
    return held


class Defs(unittest.TestCase):
    def test_every_text_once_by_its_sha256(self):
        with tempfile.TemporaryDirectory() as out:
            drvs = os.path.join(out, "drvs")
            os.makedirs(drvs)
            same = '{"/nix/store/x-python3.13-requests.drv":{"env":{"d":"Grüße \\"q\\"\\n"}}}\n'
            files = {
                "hello.json": '{"/nix/store/x-hello.drv":{"env":{"name":"hello"}}}\n',
                "python3Packages.requests.json": same,
                "python313Packages.requests.json": same,
                'pkg."dbus-1".json': '{"a":1}\n',
                # A closure in one piece: a file of its own, not in a shard.
                "steam.closure.json": '{"top":"s.drv","drvs":{"s.drv":{}},"cached":{},"srcs":{}}',
                # One in parts: in a shard, and its parts.
                "rPackages.tidyverse.closure.json": '{"cached":{},"drvs_by_sha256":{"t.drv":"%s"},"srcs_by_sha256":{},"top":"t.drv"}' % sha('{"env":{}}'),
            }
            for name, text in files.items():
                with open(os.path.join(drvs, name), "w", encoding="utf-8") as f:
                    f.write(text)
            with open(os.path.join(out, "closure-parts.jsonl"), "w") as f:
                f.write(json.dumps([sha('{"env":{}}'), '{"env":{}}']) + "\n")
            n, files_written = defs.write(out)
            held = shards(out)
            self.assertEqual(n, 5, "the shared definition once")
            self.assertEqual(len(os.listdir(os.path.join(out, "defs"))), files_written)
            for name, text in files.items():
                self.assertEqual(sha(text) in held, name != "steam.closure.json", name)
            for h, text in held.items():
                self.assertEqual(sha(text), h)
                self.assertTrue(any(f.startswith(h[:3]) for f in os.listdir(os.path.join(out, "defs"))))
            self.assertEqual(held[sha('{"env":{}}')], '{"env":{}}')

            published = os.path.join(out, "site-drvs")
            self.assertEqual(defs.link_whole_closures(out, published), 1)
            self.assertEqual(os.listdir(published), ["steam.closure.json"])


if __name__ == "__main__":
    unittest.main()
