# SPDX-License-Identifier: GPL-3.0-or-later
import importlib.util
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location(
    "compare_runtimes", Path(__file__).resolve().parents[1] / "scripts" / "compare_runtimes.py"
)
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


class CompareRuntimesTests(unittest.TestCase):
    def test_summarize_skips_missing_values(self):
        runs = [{"v": 1.0}, {"v": None}, {"v": 3.0}]
        self.assertEqual(module.summarize(runs, "v"), {"median": 2.0, "min": 1.0, "max": 3.0})
        self.assertIsNone(module.summarize([{"v": None}], "v"))

    def test_private_identifier_blocks_output(self):
        with self.assertRaises(SystemExit):
            module.refuse_if_private(str(Path.home()))
        module.refuse_if_private("a clean line with no identifiers")


if __name__ == "__main__":
    unittest.main()
