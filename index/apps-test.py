"""Tests for apps.py's icon finding (no downloads): python3 -I index/apps-test.py"""
import gzip
import importlib.util
import io
import os
import sys
import unittest

here = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, here)
spec = importlib.util.spec_from_file_location("apps", os.path.join(here, "apps.py"))
apps = importlib.util.module_from_spec(spec)
spec.loader.exec_module(apps)
import nar  # noqa: E402

SVG = b'<svg xmlns="http://www.w3.org/2000/svg" width="8" height="8"/>'
XPM = b"""/* XPM */
static char * x[] = {
"2 2 2 1",
"  c None",
". c #FF0000",
". ",
" ."};
"""


def nar_file(data):
    """A NAR whose root is one regular file."""
    def s(b):
        return len(b).to_bytes(8, "little") + b + b"\0" * ((8 - len(b) % 8) % 8)

    return b"".join(s(x) for x in (b"nix-archive-1", b"(", b"type", b"regular", b"contents")) + s(data) + s(b")")


class Names(unittest.TestCase):
    def test_image_stem_keeps_dotted_ids(self):
        self.assertEqual(apps.image_stem("org.xfce.ScreenSaver"), "org.xfce.screensaver")
        self.assertEqual(apps.image_stem("jpilot.xpm"), "jpilot")
        self.assertEqual(apps.image_stem("/opt/jaero/jaero.ico"), "jaero")
        self.assertEqual(apps.image_stem("share/icons/x.svgz"), "x")

    def test_looser(self):
        self.assertEqual(apps.looser("android-studio-dev"), ["android-studio"])
        self.assertEqual(apps.looser("android-studio-for-platform-canary"), ["android-studio-for-platform"])
        self.assertEqual(apps.looser("charles-proxy5"), ["charles-proxy"])
        self.assertEqual(apps.looser("pdfstudio2021"), ["pdfstudio"])
        self.assertEqual(apps.looser("ut1999"), [])  # "ut" alone says nothing
        self.assertEqual(apps.looser("gimp"), [])

    def test_theme_fallbacks(self):
        self.assertEqual(apps.theme_fallbacks("preferences-desktop-display-randr"), ["preferences-desktop-display", "preferences-desktop"])
        self.assertEqual(apps.theme_fallbacks("input-keyboard-virtual"), ["input-keyboard"])
        self.assertEqual(apps.theme_fallbacks("network-wireless-symbolic"), [])
        self.assertEqual(apps.theme_fallbacks("starship-sf64"), [])  # an app's name, not the theme's

    def test_own_name(self):
        self.assertEqual(apps.own_name("kdePackages.kate", "kate-24.12"), "kate")
        self.assertEqual(apps.own_name("androidStudioPackages.stable", "android-studio-2026.2"), "")
        self.assertEqual(apps.own_name("zathuraPkgs.zathura_cb", "zathura-cb-0.2"), "zathura_cb")
        self.assertEqual(apps.own_name("gimp", "gimp-3.0"), "gimp")

    def test_words(self):
        self.assertEqual(apps.words("Heaven"), 1)
        self.assertEqual(apps.words("vcv-rack"), 2)
        self.assertEqual(apps.words("VCVRack"), 2)
        self.assertEqual(apps.words("pdfstudio2021"), 2)

    def test_desktop_icons(self):
        text = "[Desktop Action x]\nIcon=no\n[Desktop Entry]\nName=A\nIcon=org.example.A\n"
        self.assertEqual(apps.desktop_icons(text), "org.example.A")
        self.assertEqual(apps.desktop_icons("[Desktop Entry]\nName=A\n"), "")

    def test_own_picture(self):
        for n in ("icon", "logo", "icon_color", "icon512", "product_logo_128", "app-icon"):
            self.assertTrue(apps.OWN_PICTURE.fullmatch(n), n)
        for n in ("icon_star_empty@20", "discordicon", "achhidden", "github_logo"):
            self.assertFalse(apps.OWN_PICTURE.fullmatch(n), n)

    def test_best_file(self):
        files = [("/s/share/icons/hicolor/16x16/apps/a.png", 9), ("/s/share/icons/hicolor/128x128/apps/a.png", 9),
                 ("/s/share/icons/hicolor/scalable/apps/a.svg", 9), ("/s/lib/a.png", 99)]
        self.assertEqual(apps.best_file(files), "/s/share/icons/hicolor/128x128/apps/a.png")
        self.assertEqual(apps.best_file(files[:1] + files[2:3]), "/s/share/icons/hicolor/scalable/apps/a.svg")
        self.assertIsNone(apps.best_file([]))


class Pictures(unittest.TestCase):
    def test_png_and_svg_as_they_are(self):
        png = apps.PNG + b"rest"
        self.assertEqual(apps.as_icon(png), (png, "png"))
        self.assertEqual(apps.as_icon(SVG, "a.svg"), (SVG, "svg"))

    def test_svgz_unpacked(self):
        self.assertEqual(apps.as_icon(gzip.compress(SVG), "a.svgz"), (SVG, "svg"))

    def test_not_a_picture(self):
        self.assertIsNone(apps.as_icon(b"#!/bin/sh\n", "a.png"))
        self.assertIsNone(apps.as_icon(None))

    @unittest.skipUnless(importlib.util.find_spec("PIL"), "needs Pillow")
    def test_xpm_and_ico_made_png(self):
        from PIL import Image

        data, ext = apps.as_icon(XPM, "a.xpm")
        self.assertEqual((ext, data[:8]), ("png", apps.PNG))
        self.assertEqual(Image.open(io.BytesIO(data)).size, (2, 2))
        buf = io.BytesIO()
        Image.new("RGBA", (64, 64), "red").save(buf, "ICO", sizes=[(16, 16), (64, 64)])
        data, ext = apps.as_icon(buf.getvalue(), "a.ico")
        self.assertEqual(Image.open(io.BytesIO(data)).size, (64, 64))  # the largest


class Variants(unittest.TestCase):
    def setUp(self):
        apps.index = {
            "android-studio": {"name": "android-studio-2026.2"},
            "androidStudioPackages.dev": {"name": "android-studio-dev-2026.2"},
            "ibus-engines.mozc": {"name": "ibus-mozc-3.35"},
            "ibus-mozc": {"name": "ibus-mozc-3.35"},
            "evolution-data-server": {"name": "evolution-data-server-3.62"},
            "evolution-data-server-gtk4": {"name": "evolution-data-server-3.62"},
            "heaven": {"name": "heaven-1"},
            "unigine-heaven": {"name": "unigine-heaven-4"},
        }
        apps.found.clear()
        apps.found.update({
            "android-studio": (["android-studio.desktop"], []),
            "androidStudioPackages.dev": (["android-studio-dev.desktop"], []),
            "ibus-engines.mozc": (["ibus-setup-mozc-jp.desktop"], []),
            "ibus-mozc": (["ibus-setup-mozc-jp.desktop"], []),
            "evolution-data-server": (["org.gnome.Evolution-alarm-notify.desktop", "org.gnome.evolution-data-server.OAuth2-handler.desktop"], []),
            "evolution-data-server-gtk4": (["org.gnome.evolution-data-server.OAuth2-handler.desktop"], []),
            "heaven": (["heaven.desktop"], []),
            "unigine-heaven": (["unigine-heaven.desktop"], []),
        })
        apps.closure_icons.clear()  # Icon= as a build closure says it (no downloads)
        apps.closure_icons.update({"androidStudioPackages.dev": ["android-studio-dev"], "ibus-mozc": [], "evolution-data-server-gtk4": [],
                                   "unigine-heaven": ["Heaven"]})

    def test_same_app_in_another_package(self):
        have = {a: (a, a.encode(), "png", "x") for a in ("android-studio", "ibus-engines.mozc", "evolution-data-server", "heaven")}
        got = apps.variants(["androidStudioPackages.dev", "ibus-mozc", "evolution-data-server-gtk4", "unigine-heaven"], have)
        self.assertEqual(got["androidStudioPackages.dev"][1], b"android-studio")  # its Icon= without -dev
        self.assertEqual(got["ibus-mozc"][1], b"ibus-engines.mozc")  # the same desktop file
        self.assertEqual(got["evolution-data-server-gtk4"][1], b"evolution-data-server")
        # Icon=Heaven is one common word: another app called heaven may be another app.
        self.assertNotIn("unigine-heaven", got)


class Nar(unittest.TestCase):
    def test_a_store_path_that_is_one_file(self):
        self.assertEqual(nar.NarReader(io.BytesIO(nar_file(b"picture"))).find(""), b"picture")

    def test_link_above_the_store_path(self):
        self.assertEqual(nar.resolve("/nix/store/x-a", ["share"], "../../../etc/x"), "/nix/store/x-a/etc/x")


if __name__ == "__main__":
    unittest.main()
