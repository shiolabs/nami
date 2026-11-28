"""Tests for extra-apps.py: python3 -I packaging/extra-apps-test.py"""
import importlib.util
import io
import os
import shutil
import tempfile
import unittest

spec = importlib.util.spec_from_file_location("extra_apps", os.path.join(os.path.dirname(os.path.abspath(__file__)), "extra-apps.py"))
xa = importlib.util.module_from_spec(spec)
spec.loader.exec_module(xa)

DESKTOP = """[Desktop Entry]
Type=Application
Name=WordCraft
Name[de]=WortHandwerk
Comment=Write documents
Exec=wordcraft %F
Icon=wordcraft
Categories=Office;WordProcessor;
MimeType=application/vnd.oasis.opendocument.text;
"""

METAINFO = """<?xml version="1.0" encoding="UTF-8"?>
<component type="desktop-application">
  <id>com.storytold.WordCraft</id>
  <name>WordCraft</name>
  <summary>A fast word processor</summary>
  <summary xml:lang="de">Ein schnelles Textverarbeitungsprogramm</summary>
  <project_license>Apache-2.0</project_license>
  <developer id="com.storytold"><name>Storytold</name></developer>
  <url type="homepage">https://github.com/storytold/wordcraft</url>
  <description><p>Long text.</p></description>
</component>
"""

STORE_PATH = "/nix/store/" + "a" * 32 + "-wordcraft-0.5.0"


def png(size):
    from PIL import Image

    buf = io.BytesIO()
    Image.new("RGBA", (size, size), (200, 40, 40, 255)).save(buf, "PNG")
    return buf.getvalue()


class Package(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.mkdtemp()
        share = os.path.join(self.dir, "share")
        for d in ("applications", "metainfo", "icons/hicolor/256x256/apps", "icons/hicolor/scalable/apps"):
            os.makedirs(os.path.join(share, d))
        os.makedirs(os.path.join(self.dir, "bin"))
        open(os.path.join(self.dir, "bin/wordcraft"), "w").close()
        open(os.path.join(share, "applications/wordcraft.desktop"), "w").write(DESKTOP)
        open(os.path.join(share, "metainfo/com.storytold.WordCraft.metainfo.xml"), "w").write(METAINFO)
        open(os.path.join(share, "icons/hicolor/256x256/apps/wordcraft.png"), "wb").write(png(256))
        open(os.path.join(share, "icons/hicolor/scalable/apps/wordcraft.svg"), "w").write("<svg xmlns='http://www.w3.org/2000/svg'/>")

    def tearDown(self):
        shutil.rmtree(self.dir)

    def test_desktop_and_metainfo(self):
        d = xa.read_desktop(DESKTOP)
        self.assertEqual((d["Name"], d["Icon"], d["Categories"]), ("WordCraft", "wordcraft", "Office;WordProcessor;"))
        m = xa.read_metainfo(METAINFO)
        self.assertEqual(m, {"summary": "A fast word processor", "name": "WordCraft", "license": "Apache-2.0",
                             "publisher": "Storytold", "homepage": "https://github.com/storytold/wordcraft"})
        self.assertEqual(xa.read_metainfo("<component><developer_name>Old Style</developer_name></component>")["publisher"], "Old Style")

    def test_describe(self):
        entry, icon = xa.describe(self.dir, STORE_PATH)
        self.assertEqual(entry, {
            "name": "wordcraft-0.5.0", "outputs": {"out": STORE_PATH}, "apps": ["wordcraft.desktop"],
            "description": "A fast word processor", "homepage": "https://github.com/storytold/wordcraft",
            "license": "Apache-2.0", "publisher": "Storytold", "mainProgram": "wordcraft"})
        # The 256px PNG beats the SVG, scaled down to 128px.
        from PIL import Image

        self.assertEqual(icon[1], "png")
        self.assertEqual(Image.open(io.BytesIO(icon[0])).size, (128, 128))

    def test_overrides_and_fallbacks(self):
        os.remove(os.path.join(self.dir, "share/metainfo/com.storytold.WordCraft.metainfo.xml"))
        os.remove(os.path.join(self.dir, "share/icons/hicolor/256x256/apps/wordcraft.png"))
        entry, icon = xa.describe(self.dir, STORE_PATH, {"publisher": "Storytold Ltd"})
        self.assertEqual(entry["description"], "Write documents")  # the desktop file's Comment
        self.assertEqual(entry["publisher"], "Storytold Ltd")
        self.assertNotIn("license", entry)
        self.assertEqual(icon[1], "svg")
        with self.assertRaises(ValueError):
            xa.describe(self.dir, STORE_PATH, {"shelf": "Work"})

    def test_not_an_app(self):
        shutil.rmtree(os.path.join(self.dir, "share/applications"))
        with self.assertRaises(ValueError):
            xa.describe(self.dir, STORE_PATH)

    def test_check(self):
        import contextlib
        import json

        recipe = os.path.join(self.dir, "recipe", "wordcraft")
        os.makedirs(recipe)
        open(os.path.join(recipe, "package.nami"), "w").write(
            "name: wordcraft\nsource:\n  url: https://example.org/wc.tar.gz\n  sha256: abc\n")
        index = os.path.join(self.dir, "index.json")
        json.dump({"hello": {"name": "hello-2.12", "outputs": {}}}, open(index, "w"))
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            self.assertTrue(xa.check(index, recipe, STORE_PATH, self.dir))
        self.assertIn("| Publisher | Storytold |", out.getvalue())
        self.assertIn("| Downloads | https://example.org/wc.tar.gz |", out.getvalue())
        # Missing what people see: still builds, but reported.
        os.remove(os.path.join(self.dir, "share/metainfo/com.storytold.WordCraft.metainfo.xml"))
        with contextlib.redirect_stdout(io.StringIO()):
            self.assertFalse(xa.check(index, recipe, STORE_PATH, self.dir))
        # A nixpkgs name is refused.
        hello = os.path.join(self.dir, "recipe", "hello")
        shutil.copytree(recipe, hello)
        with self.assertRaises(ValueError):
            xa.check(index, hello, STORE_PATH, self.dir)


class Merge(unittest.TestCase):
    CACHE = {"url": "https://namios.dev/cache", "key": "tray-apps-1:" + "A" * 43 + "="}

    def test_merge(self):
        index = {"hello": {"name": "hello-2.12", "outputs": {"out": "/nix/store/x-hello-2.12"}, "icon": "abc"}}
        members = {"hello.png": b"h", "wordcraft.svg": b"old"}
        entry = {"name": "wordcraft-0.5.0", "outputs": {"out": STORE_PATH}, "apps": ["wordcraft.desktop"]}
        xa.merge(index, members, "wordcraft", entry, (b"\x89PNG\r\n\x1a\nnew", "png"), self.CACHE)
        self.assertEqual(index["wordcraft"]["cache"], self.CACHE)
        self.assertEqual(index["wordcraft"]["icon"], xa.hashlib.sha256(b"\x89PNG\r\n\x1a\nnew").hexdigest())
        self.assertEqual(sorted(members), ["hello.png", "wordcraft.png"])  # the old icon went
        self.assertEqual(index["hello"]["icon"], "abc")
        # Published again (a new version): replaced.
        xa.merge(index, members, "wordcraft", dict(entry, name="wordcraft-0.6.0"), None, self.CACHE)
        self.assertEqual(index["wordcraft"]["name"], "wordcraft-0.6.0")
        self.assertNotIn("icon", index["wordcraft"])
        self.assertNotIn("wordcraft.png", members)

    def test_never_replaces_nixpkgs(self):
        index = {"hello": {"name": "hello-2.12", "outputs": {}}}
        with self.assertRaises(ValueError):
            xa.merge(index, {}, "hello", {"name": "hello-9"}, None, self.CACHE)


if __name__ == "__main__":
    unittest.main()
