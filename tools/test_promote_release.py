#!/usr/bin/env python3
"""Release-body regression checks; no network, credentials or package installation."""
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import MagicMock, patch
import urllib.error
from promote_release import SIGNED_START, SIGNED_END, download_asset, signed_notes


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


class AssetDownloadTests(unittest.TestCase):
    artifact = {"url": "https://github.com/siarheisinelnikau/desk-switch/releases/download/v0.6.1/desk-switch-v0.6.1-windows-x86-64.exe", "size": 4}
    asset = {"id": 123, "name": "desk-switch-v0.6.1-windows-x86-64.exe"}

    def response(self, data):
        response = MagicMock()
        response.__enter__.return_value.read.return_value = data
        return response

    def transient(self):
        return urllib.error.HTTPError(self.artifact["url"], 504, "Gateway Time-out", {}, None)

    def test_public_success_does_not_use_the_authenticated_api(self):
        with tempfile.TemporaryDirectory() as directory, patch("promote_release.urllib.request.urlopen", return_value=self.response(b"data")) as request, patch("promote_release.subprocess.run") as api:
            self.assertEqual(download_asset(self.artifact, self.asset, Path(directory) / "asset"), b"data")
            self.assertEqual(request.call_args.args[0].full_url, self.artifact["url"])
            api.assert_not_called()

    def test_one_transient_public_failure_retries_without_using_the_api(self):
        with tempfile.TemporaryDirectory() as directory, patch("promote_release.urllib.request.urlopen", side_effect=[self.transient(), self.response(b"data")]) as request, patch("promote_release.time.sleep"), patch("promote_release.subprocess.run") as api:
            self.assertEqual(download_asset(self.artifact, self.asset, Path(directory) / "asset"), b"data")
            self.assertEqual(request.call_count, 2)
            api.assert_not_called()

    def test_repeated_504_uses_the_exact_official_asset_id(self):
        def api(args, **kwargs):
            self.assertEqual(args, ["gh", "api", "-H", "Accept: application/octet-stream", "repos/siarheisinelnikau/desk-switch/releases/assets/123"])
            self.assertEqual(kwargs["timeout"], 90)
            self.assertTrue(kwargs["check"])
            kwargs["stdout"].write(b"data")
        with tempfile.TemporaryDirectory() as directory, patch("promote_release.urllib.request.urlopen", side_effect=self.transient()), patch("promote_release.time.sleep"), patch("promote_release.subprocess.run", side_effect=api) as fallback:
            self.assertEqual(download_asset(self.artifact, self.asset, Path(directory) / "asset"), b"data")
            self.assertEqual(fallback.call_count, 1)

    def test_nontransient_public_errors_fail_without_an_authenticated_fallback(self):
        for code in [401, 403, 404]:
            with self.subTest(code=code), tempfile.TemporaryDirectory() as directory, patch("promote_release.urllib.request.urlopen", side_effect=urllib.error.HTTPError(self.artifact["url"], code, "Unavailable", {}, None)), patch("promote_release.subprocess.run") as api:
                with self.assertRaises(urllib.error.HTTPError):
                    download_asset(self.artifact, self.asset, Path(directory) / "asset")
                api.assert_not_called()

    def test_fallback_cannot_route_to_an_arbitrary_asset_path(self):
        with tempfile.TemporaryDirectory() as directory, patch("promote_release.urllib.request.urlopen", side_effect=self.transient()), patch("promote_release.time.sleep"), patch("promote_release.subprocess.run") as api:
            with self.assertRaises(AssertionError):
                download_asset(self.artifact, {"id": "../other", "name": "asset"}, Path(directory) / "asset")
            api.assert_not_called()

    def test_oversized_api_data_remains_detectable_before_native_validation(self):
        def api(args, **kwargs):
            kwargs["stdout"].write(b"data-too-large")
        with tempfile.TemporaryDirectory() as directory, patch("promote_release.urllib.request.urlopen", side_effect=self.transient()), patch("promote_release.time.sleep"), patch("promote_release.subprocess.run", side_effect=api):
            data = download_asset(self.artifact, self.asset, Path(directory) / "asset")
            self.assertEqual(len(data), self.artifact["size"] + 1)
            self.assertNotEqual(len(data), self.artifact["size"])


if __name__ == "__main__":
    unittest.main()
