#!/usr/bin/env python3
"""Tests for third-party notice generation and its offline freshness check."""

from __future__ import annotations

from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

import gen_third_party_notices as notices

ROOT = Path(__file__).resolve().parents[1]


class ThirdPartyNoticeTests(unittest.TestCase):
    def test_real_repository_check_is_offline_and_current(self):
        result = subprocess.run(
            [sys.executable, str(ROOT / "tools/gen_third_party_notices.py"), "--check"],
            cwd=ROOT,
            text=True,
            encoding="utf-8",
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            timeout=30,
            check=False,
        )
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("covers all", result.stdout)
        self.assertIn("no missing, stale, or duplicate entries", result.stdout)

    def test_synthetic_stale_file_lists_missing_stale_and_duplicates(self):
        with tempfile.TemporaryDirectory(prefix="desk-switch-notices-test-") as temporary:
            work = Path(temporary)
            lock = work / "Cargo.lock"
            document = work / "THIRD-PARTY-NOTICES.md"
            lock.write_text(
                """version = 4

[[package]]
name = "desk-switch"
version = "0.8.0"

[[package]]
name = "alpha"
version = "1.2.3"

[[package]]
name = "beta"
version = "4.5.6"
""",
                encoding="utf-8",
            )
            document.write_text(
                """# Synthetic notices
<!-- cargo-package: alpha 1.2.2 -->
<!-- cargo-package: alpha 1.2.2 -->
""",
                encoding="utf-8",
            )
            with self.assertRaises(notices.NoticesOutdatedError) as caught:
                notices.check_notices(lock, document)
            message = str(caught.exception)
            self.assertIn("Missing entries (2):", message)
            self.assertIn("alpha 1.2.3", message)
            self.assertIn("beta 4.5.6", message)
            self.assertIn("Stale entries (1):", message)
            self.assertIn("alpha 1.2.2", message)
            self.assertIn("Duplicate entries (1):", message)
            self.assertNotIn("desk-switch 0.8.0\n", message)

    def test_regeneration_is_deterministic_and_sorted(self):
        packages = [
            {
                "name": "zulu",
                "version": "1.0.0",
                "license": "MIT",
                "repository": "https://example.invalid/zulu",
                "authors": ["Zulu Author"],
            },
            {
                "name": "alpha",
                "version": "2.0.0",
                "license": "Apache-2.0",
                "homepage": "https://example.invalid/alpha",
            },
            {
                "name": "Beta",
                "version": "1.0.0",
                "license": "MIT OR Apache-2.0",
            },
            {
                "name": "alpha",
                "version": "10.0.0",
                "license": "MIT",
            },
            {
                "name": "desk-switch",
                "version": "0.8.0",
                "license": "MIT",
            },
            {
                "name": "alpha",
                "version": "1.0.0",
                "license": "MIT",
            },
        ]
        first = notices.render_notices(packages, ROOT)
        second = notices.render_notices(reversed(packages), ROOT)
        self.assertEqual(first, second)
        markers = notices.documented_packages(first)
        self.assertEqual(
            markers,
            [
                ("alpha", "1.0.0"),
                ("alpha", "10.0.0"),
                ("alpha", "2.0.0"),
                ("Beta", "1.0.0"),
                ("zulu", "1.0.0"),
            ],
        )
        self.assertNotIn("cargo-package: desk-switch", first)
        self.assertIn("Zulu Author", first)
        self.assertIn("https://example.invalid/alpha", first)
        self.assertLess(first.index("## Appendix A: MIT License"), first.index("## Appendix B:"))


if __name__ == "__main__":
    unittest.main(verbosity=2)
