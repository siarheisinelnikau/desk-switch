#!/usr/bin/env python3
"""Release-body regression checks; no network, credentials or package installation."""
import json
import unittest
from promote_release import SIGNED_START, SIGNED_END, signed_notes


class SignedNotesTests(unittest.TestCase):
    manifest = json.dumps({"payload": '{"version":"0.4.7"}', "signature": "test-signature"})

    def test_manually_published_release_keeps_notes_and_gains_the_envelope(self):
        notes = signed_notes("Maintainer's existing release notes", self.manifest, "Fallback")
        self.assertTrue(notes.startswith("Maintainer's existing release notes\n\n"))
        self.assertEqual(json.loads(notes.split(SIGNED_START)[1].split(SIGNED_END)[0]), json.loads(self.manifest))

    def test_existing_matching_envelope_is_idempotent_including_json_whitespace(self):
        notes = "Notes\n" + SIGNED_START + "\n" + json.dumps(json.loads(self.manifest), indent=4) + "\n" + SIGNED_END
        self.assertEqual(signed_notes(notes, self.manifest, "Fallback"), notes)

    def test_a_different_signed_release_is_never_overwritten(self):
        notes = signed_notes("Notes", self.manifest, "")
        other = json.dumps({"payload": '{"version":"0.4.8"}', "signature": "other"})
        with self.assertRaises(AssertionError):
            signed_notes(notes, other, "")

    def test_partial_duplicate_and_reversed_markers_are_rejected(self):
        for body in [SIGNED_START, SIGNED_END, SIGNED_END + SIGNED_START,
                     SIGNED_START * 2 + SIGNED_END, SIGNED_START + SIGNED_END * 2]:
            with self.subTest(body=body), self.assertRaises((AssertionError, ValueError)):
                signed_notes(body, self.manifest, "")


if __name__ == "__main__":
    unittest.main()
