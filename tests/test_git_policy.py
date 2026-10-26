# SPDX-License-Identifier: GPL-3.0-or-later

from pathlib import Path
import sys
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "scripts"))
from check_git_policy import check_event, message_error, route_error


class GitPolicyTests(unittest.TestCase):
    def test_valid_routes_include_community_forks(self):
        self.assertIsNone(route_error("develop", "feature/hardware-discovery", True))
        self.assertIsNone(route_error("develop", "patch-1", False))
        self.assertIsNone(route_error("staging", "develop", True))
        self.assertIsNone(route_error("main", "staging", True))

    def test_skipped_stages_and_forked_promotions_are_rejected(self):
        for base, head, same_repo in [
            ("main", "feature/a", True), ("main", "develop", True),
            ("staging", "feature/a", True), ("develop", "main", True),
            ("main", "staging", False), ("staging", "develop", False),
        ]:
            with self.subTest(base=base, head=head):
                self.assertIsNotNone(route_error(base, head, same_repo))

    def test_trailers_are_case_insensitive(self):
        self.assertIsNone(message_error("feat: detect hardware\n\nExplain the change."))
        self.assertIsNotNone(message_error("feat: detect hardware\n\nCo-Authored-By: someone"))

    @patch("check_git_policy.subprocess.check_output")
    def test_pr_checks_all_commit_messages(self, output):
        output.return_value = "feat: detect CPU\n\nCo-authored-by: someone\n"
        event = {"pull_request": {
            "base": {"ref": "develop", "sha": "a" * 40, "repo": {"full_name": "dexterhere/kvlm"}},
            "head": {"ref": "feature/a", "sha": "b" * 40, "repo": {"full_name": "dexterhere/kvlm"}},
        }}
        self.assertEqual(check_event(event), ["Co-authored-by trailers are not permitted."])
        self.assertEqual(output.call_args.args[0][-1], "a" * 40 + ".." + "b" * 40)

    @patch("check_git_policy.subprocess.check_output", return_value="docs: bootstrap\n")
    def test_new_branch_checks_history_and_deletion_is_rejected(self, output):
        self.assertEqual(check_event({"before": "0" * 40, "after": "a" * 40}), [])
        self.assertEqual(output.call_args.args[0][-1], "a" * 40)
        self.assertTrue(check_event({"before": "a" * 40, "after": "0" * 40}))
