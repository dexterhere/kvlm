# SPDX-License-Identifier: GPL-3.0-or-later

from html.parser import HTMLParser
from pathlib import Path
import re
import tomllib
import unittest
import xml.etree.ElementTree as ElementTree

ROOT = Path(__file__).resolve().parents[1]
SITE = ROOT / "site"
PAGE = SITE / "index.html"
CANONICAL = "https://dexterhere.github.io/kvlm/"
MAX_SCREENSHOT_BYTES = 300 * 1024


class PageParser(HTMLParser):
    def __init__(self):
        super().__init__(convert_charrefs=True)
        self.tags = []
        self.ids = set()
        self.h1_count = 0
        self.meta = {}
        self.links = []
        self.references = []
        self.title_parts = []
        self.badge_parts = []
        self._in_title = False
        self._badge_depth = 0

    def handle_starttag(self, tag, attrs):
        attributes = dict(attrs)
        self.tags.append(tag)
        if "id" in attributes:
            self.ids.add(attributes["id"])
        if tag == "h1":
            self.h1_count += 1
        if tag == "meta":
            key = attributes.get("property") or attributes.get("name")
            if key:
                self.meta[key] = attributes.get("content", "")
        if tag == "title":
            self._in_title = True
        if tag in {"a", "link"} and "href" in attributes:
            self.links.append(attributes["href"])
        for attribute in ("href", "src"):
            if attribute in attributes:
                self.references.append((tag, attribute, attributes[attribute]))
        if "badge" in attributes.get("class", "").split():
            self._badge_depth = 1
        elif self._badge_depth:
            self._badge_depth += 1

    def handle_endtag(self, tag):
        if tag == "title":
            self._in_title = False
        if self._badge_depth:
            self._badge_depth -= 1

    def handle_data(self, data):
        if self._in_title:
            self.title_parts.append(data)
        if self._badge_depth:
            self.badge_parts.append(data)

    @property
    def title(self):
        return "".join(self.title_parts).strip()

    @property
    def badge(self):
        return "".join(self.badge_parts).strip()


def parse_page():
    parser = PageParser()
    parser.feed(PAGE.read_text(encoding="utf-8"))
    parser.close()
    return parser


class SiteStructureTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.page = parse_page()

    def test_page_has_one_heading_and_language(self):
        html = PAGE.read_text(encoding="utf-8")
        self.assertEqual(self.page.h1_count, 1)
        self.assertIn('<html lang="en">', html)
        self.assertTrue(self.page.title)

    def test_metadata_is_present(self):
        self.assertTrue(self.page.meta["description"])
        self.assertEqual(self.page.meta["og:type"], "website")
        self.assertTrue(self.page.meta["og:title"])
        self.assertTrue(self.page.meta["og:description"])
        self.assertEqual(self.page.meta["og:url"], CANONICAL)
        self.assertEqual(self.page.meta["og:image"], CANONICAL + "assets/og.png")
        self.assertEqual(self.page.meta["twitter:card"], "summary_large_image")
        self.assertIn(CANONICAL, self.page.links)

    def test_no_script_or_iframe(self):
        self.assertNotIn("script", self.page.tags)
        self.assertNotIn("iframe", self.page.tags)

    def test_only_the_page_itself_is_loaded(self):
        for tag, attribute, value in self.page.references:
            if value.startswith(("https://", "http://", "mailto:")):
                continue
            with self.subTest(tag=tag, attribute=attribute, value=value):
                self.assertFalse(value.startswith("/"), value)
                self.assertNotIn("..", Path(value).parts)
                if value.startswith("#"):
                    self.assertIn(value[1:], self.page.ids)
                    continue
                path = (SITE / value).resolve()
                self.assertTrue(path.is_relative_to(SITE.resolve()))
                self.assertTrue(path.is_file(), value)
                self.assertGreater(path.stat().st_size, 0, value)

    def test_css_is_self_contained(self):
        css = (SITE / "styles.css").read_text(encoding="utf-8")
        self.assertNotIn("http://", css)
        self.assertNotIn("https://", css)
        self.assertNotIn("@import", css)
        for raw in re.findall(r"url\(([^)]+)\)", css):
            value = raw.strip().strip("\"'")
            with self.subTest(url=value):
                self.assertFalse(value.startswith("//"), value)
                path = (SITE / value).resolve()
                self.assertTrue(path.is_relative_to(SITE.resolve()))
                self.assertTrue(path.is_file(), value)
                self.assertGreater(path.stat().st_size, 0, value)

    def test_version_matches_cargo(self):
        with (ROOT / "Cargo.toml").open("rb") as manifest:
            version = tomllib.load(manifest)["workspace"]["package"]["version"]
        self.assertEqual(self.page.badge, f"{version} beta")

    def test_sitemap_matches_canonical_url(self):
        tree = ElementTree.parse(SITE / "sitemap.xml")
        namespace = {"sitemap": "http://www.sitemaps.org/schemas/sitemap/0.9"}
        locations = [node.text for node in tree.findall(".//sitemap:loc", namespace)]
        self.assertEqual(locations, [CANONICAL])
        robots = (SITE / "robots.txt").read_text(encoding="utf-8")
        self.assertIn(f"Sitemap: {CANONICAL}sitemap.xml", robots)


class SiteAssetTests(unittest.TestCase):
    def test_svg_copies_match_their_sources(self):
        pairs = [
            (SITE / "assets/logo.svg", ROOT / "docs/assets/logo.svg"),
            (
                SITE / "assets/favicon.svg",
                ROOT / "data/icons/hicolor/scalable/apps/io.github.dexterhere.Kvlm.svg",
            ),
        ]
        for copy, source in pairs:
            with self.subTest(copy=copy.name):
                self.assertEqual(copy.read_bytes(), source.read_bytes())

    def test_social_and_icon_images_exist(self):
        for name in (
            "og.png",
            "favicon-32.png",
            "apple-touch-icon.png",
        ):
            path = SITE / "assets" / name
            with self.subTest(name=name):
                self.assertTrue(path.is_file())
                self.assertGreater(path.stat().st_size, 0)

    def test_font_is_self_hosted_and_licensed(self):
        font = SITE / "assets/fonts/figtree-latin.woff2"
        self.assertEqual(font.read_bytes()[:4], b"wOF2")
        self.assertTrue((SITE / "assets/fonts/Figtree-OFL.txt").is_file())
        css = (SITE / "styles.css").read_text(encoding="utf-8")
        self.assertIn('font-family: "Figtree"', css)

    def test_screenshots_are_present_and_small(self):
        shots = sorted((SITE / "screenshots").glob("*.webp"))
        self.assertEqual(
            {shot.name for shot in shots},
            {"my-computer.webp", "models.webp", "runtimes.webp"},
        )
        for shot in shots:
            with self.subTest(shot=shot.name):
                self.assertLessEqual(shot.stat().st_size, MAX_SCREENSHOT_BYTES)


if __name__ == "__main__":
    unittest.main()
