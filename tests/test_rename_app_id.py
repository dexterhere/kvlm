# SPDX-License-Identifier: GPL-3.0-or-later

from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "scripts"))
from rename_app_id import apply, current_id, plan, tracked, validate

OLD = "org.gnome.Example"
NEW = "io.github.owner.Example"


def make_repo(root: Path) -> None:
    (root / "crates/workstation-core/src").mkdir(parents=True)
    (root / "crates/workstation-core/src/lib.rs").write_text(
        f'pub const APP_ID: &str = "{OLD}";\n'
    )
    (root / "data").mkdir()
    (root / f"data/{OLD}.desktop.in").write_text(f"Icon={OLD}\n")
    (root / "data/blob.bin").write_bytes(b"\xff\xfe" + OLD.encode())
    (root / "README.md").write_text("nothing here\n")
    for command in (
        ["init", "-q"],
        ["add", "."],
        ["-c", "user.name=t", "-c", "user.email=t@example.org", "commit", "-qm", "init"],
    ):
        subprocess.run(["git", "-C", str(root), *command], check=True)


class RenameTests(unittest.TestCase):
    def test_ids_that_imply_another_project_or_are_malformed_are_refused(self):
        validate(NEW)
        for bad in ("org.gnome.Other", "org.kde.X.Y", "nodots", "a.b", "1abc.def.Ghi", "a b.c.d"):
            with self.subTest(bad=bad), self.assertRaises(ValueError):
                validate(bad)

    def test_plan_is_read_only_and_apply_renames_and_rewrites_text_only(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            make_repo(root)
            self.assertEqual(current_id(root), OLD)
            changes = plan(root, OLD, NEW, tracked(root))
            self.assertEqual(
                changes["renames"],
                [(f"data/{OLD}.desktop.in", f"data/{NEW}.desktop.in")],
            )
            self.assertEqual(
                sorted(changes["rewrites"]),
                sorted([f"data/{OLD}.desktop.in", "crates/workstation-core/src/lib.rs"]),
            )
            self.assertTrue((root / f"data/{OLD}.desktop.in").exists(), "a plan changes nothing")
            apply(root, OLD, NEW, changes)
            self.assertEqual(current_id(root), NEW)
            self.assertEqual((root / f"data/{NEW}.desktop.in").read_text(), f"Icon={NEW}\n")
            self.assertFalse((root / f"data/{OLD}.desktop.in").exists())
            self.assertEqual((root / "data/blob.bin").read_bytes()[:2], b"\xff\xfe")


if __name__ == "__main__":
    unittest.main()
