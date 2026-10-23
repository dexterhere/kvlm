# SPDX-License-Identifier: GPL-3.0-or-later

from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "scripts"))
from build_app import build
from check_repository import markdown_errors


class MarkdownTests(unittest.TestCase):
    def test_relative_links_fragments_and_external_links(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "docs").mkdir()
            (root / "docs/guide.md").write_text("# Guide\n")
            (root / "README.md").write_text(
                "[guide](docs/guide.md#guide) [web](https://example.org) "
                "[email](mailto:community@example.org) [here](#local)\n"
            )
            self.assertEqual(markdown_errors(root), [])
            (root / "docs/guide.md").write_text("[home](../README.md) [bad](absent.md)\n")
            self.assertEqual(markdown_errors(root), ["docs/guide.md: missing link absent.md"])

    def test_encoded_filename_is_resolved(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "a b.md").write_text("# Title\n")
            (root / "README.md").write_text("[encoded](a%20b.md)\n")
            self.assertEqual(markdown_errors(root), [])


class BuildTests(unittest.TestCase):
    def test_copies_successful_build_for_each_profile(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for profile in ("debug", "release"):
                with self.subTest(profile=profile):
                    binary = root / "target" / profile / "local-ai-workstation"
                    binary.parent.mkdir(parents=True, exist_ok=True)
                    binary.write_bytes(b"test executable")
                    binary.chmod(0o755)
                    output = root / "installed-app"
                    with patch("build_app.subprocess.run") as run:
                        build("cargo", root, root / "target", output, profile)
                    self.assertEqual(output.read_bytes(), binary.read_bytes())
                    self.assertTrue(output.stat().st_mode & 0o111)
                    self.assertEqual("--release" in run.call_args.args[0], profile == "release")

    def test_failed_build_does_not_replace_existing_output(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            output = root / "installed-app"
            output.write_bytes(b"previous executable")
            with patch("build_app.subprocess.run", side_effect=subprocess.CalledProcessError(1, "cargo")):
                with self.assertRaises(subprocess.CalledProcessError):
                    build("cargo", root, root / "target", output, "debug")
            self.assertEqual(output.read_bytes(), b"previous executable")


if __name__ == "__main__":
    unittest.main()
