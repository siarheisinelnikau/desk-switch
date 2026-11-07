#!/usr/bin/env python3
"""Check candidate rename guards and resumability without GitHub credentials."""
import copy
import unittest

from normalize_release_assets import candidate_updates, verify_result
from promote_release import SIGNED_START


def candidate():
    return {
        "id": 100, "tag_name": "v0.4.8", "prerelease": True, "draft": False, "body": "Notes",
        "assets": [
            {"id": 2, "name": "desk-switch-linux-x86_64.deb", "state": "uploaded", "size": 200, "digest": "sha256:" + "22" * 32},
            {"id": 1, "name": "desk-switch-v0.4.8.exe", "state": "uploaded", "size": 100, "digest": "sha256:" + "11" * 32},
        ],
    }


class CandidateRenameTests(unittest.TestCase):
    def test_partial_rename_can_resume_without_replacing_assets(self):
        before = candidate()
        partial = copy.deepcopy(before)
        partial["assets"][0]["name"] = "desk-switch-v0.4.8-linux-x86-64.deb"
        self.assertEqual(candidate_updates(partial, "0.4.8"), [{"id": 1, "name": "desk-switch-v0.4.8-windows-x86-64.exe"}])
        partial["assets"][1]["name"] = "desk-switch-v0.4.8-windows-x86-64.exe"
        self.assertEqual(candidate_updates(partial, "0.4.8"), [])
        verify_result(before, partial, "0.4.8", "Notes")

    def test_stable_signed_draft_immutable_or_other_version_cannot_be_renamed(self):
        for key, value in [("prerelease", False), ("draft", True), ("immutable", True),
                           ("tag_name", "v0.4.7"), ("body", "Notes\n" + SIGNED_START)]:
            with self.subTest(key=key), self.assertRaises(AssertionError):
                release = candidate()
                release[key] = value
                candidate_updates(release, "0.4.8")

    def test_unexpected_duplicate_or_incomplete_installers_fail_preflight(self):
        for change in [lambda assets: assets.append(copy.deepcopy(assets[0])),
                       lambda assets: assets[0].update(name="another-product.deb"),
                       lambda assets: assets[0].update(name="desk-switch-v0.4.8.exe"),
                       lambda assets: assets[0].update(state="new"),
                       lambda assets: assets[0].update(digest=None)]:
            with self.subTest(change=change), self.assertRaises(AssertionError):
                release = candidate()
                change(release["assets"])
                candidate_updates(release, "0.4.8")

    def test_postflight_detects_replacement_or_changed_bytes(self):
        before = candidate()
        after = copy.deepcopy(before)
        for update in candidate_updates(after, "0.4.8"):
            next(asset for asset in after["assets"] if asset["id"] == update["id"])["name"] = update["name"]
        for key, value in [("id", 3), ("size", 101), ("digest", "sha256:" + "33" * 32)]:
            changed = copy.deepcopy(after)
            changed["assets"][0][key] = value
            with self.subTest(key=key), self.assertRaises(AssertionError):
                verify_result(before, changed, "0.4.8", "Notes")


if __name__ == "__main__":
    unittest.main()
