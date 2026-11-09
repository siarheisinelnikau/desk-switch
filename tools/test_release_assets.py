#!/usr/bin/env python3
"""Native release validation with small synthetic binaries; no signing keys or installs."""
import copy
import io
import json
import plistlib
import shutil
import stat
import unittest
import uuid
import zipfile
from pathlib import Path
from unittest.mock import patch

import prepare_release
from prepare_release import checked_binary_data, checked_macos_archive
from release_assets import asset_names
from normalize_release_assets import candidate_updates

VERSION = "0.5.0"
PROTOCOL = 7
PLIST = "Desk Switch.app/Contents/Info.plist"
EXECUTABLE = "Desk Switch.app/Contents/MacOS/desk-switch"
NATIVE = "Desk Switch.app/Contents/Helpers/desk-switch-native"
DDC = "Desk Switch.app/Contents/Helpers/desk-switch-ddc"


def macho(architecture="arm64", extra=b""):
    header = bytearray(32)
    header[:4] = b"\xcf\xfa\xed\xfe"
    header[4:8] = {"arm64": 0x0100000c, "x86_64": 0x01000007}[architecture].to_bytes(4, "little")
    header[12:16] = (2).to_bytes(4, "little")
    return bytes(header) + extra


def bundle(architecture="arm64", version=VERSION, protocol=PROTOCOL):
    native = macho(architecture, b"synthetic-native-helper")
    ddc = macho(architecture, b"synthetic-ddc-helper")
    stamp = b"DESK-SWITCH-BUILD:" + json.dumps({"app": version, "protocol": protocol}, separators=(",", ":")).encode()
    return {
        PLIST: (plistlib.dumps({
            "CFBundleExecutable": "desk-switch", "CFBundleIdentifier": "com.deskswitch.Desktop",
            "CFBundlePackageType": "APPL", "CFBundleShortVersionString": version, "CFBundleVersion": version,
        }), stat.S_IFREG | 0o644),
        EXECUTABLE: (macho(architecture, stamp + native + ddc), stat.S_IFREG | 0o755),
        NATIVE: (native, stat.S_IFREG | 0o755), DDC: (ddc, stat.S_IFREG | 0o755),
    }


def archive(files):
    output = io.BytesIO()
    with zipfile.ZipFile(output, "w", compression=zipfile.ZIP_DEFLATED) as writer:
        for name, (data, mode) in files.items():
            entry = zipfile.ZipInfo(name)
            entry.filename = name  # Preserve intentionally invalid raw names on Windows too.
            entry.create_system = 3
            entry.compress_type = zipfile.ZIP_DEFLATED
            entry.external_attr = mode << 16
            writer.writestr(entry, data)
    return output.getvalue()


class ReleaseAssetsTests(unittest.TestCase):
    def setUp(self):
        # Normal inherited workspace permissions also work in Windows app sandboxes.
        parent = Path(__file__).resolve().parents[1] / "target"
        parent.mkdir(exist_ok=True)
        self.directory = parent / f"release-test-{uuid.uuid4().hex}"
        self.directory.mkdir(mode=0o755)
        assert self.directory.resolve().parent == parent.resolve()
        self.addCleanup(shutil.rmtree, self.directory)
        self.path = self.directory / "application.zip"

    def check(self, files, architecture="arm64"):
        data = archive(files)
        self.path.write_bytes(data)
        self.assertEqual(checked_macos_archive(self.path, f"macos-{architecture}-app", VERSION, PROTOCOL), data)

    def reject(self, files, architecture="arm64"):
        self.path.write_bytes(archive(files))
        with self.assertRaises(ValueError):
            checked_macos_archive(self.path, f"macos-{architecture}-app", VERSION, PROTOCOL)

    def test_four_targets_include_version_platform_and_correct_architecture(self):
        self.assertEqual(asset_names(VERSION), {
            "windows-x86_64": "desk-switch-v0.5.0-windows-x86-64.exe",
            "linux-x86_64-deb": "desk-switch-v0.5.0-linux-x86-64.deb",
            "macos-arm64-app": "desk-switch-v0.5.0-macos-arm64.zip",
            "macos-x86_64-app": "desk-switch-v0.5.0-macos-x86-64.zip",
        })
        self.assertEqual(len(asset_names("0.4.11")), 2)
        for value in ["v0.5.0", "0.5.0-beta", "../0.5.0", "00.5.0", "0.5"]:
            with self.subTest(value=value), self.assertRaises(ValueError):
                asset_names(value)

    def test_historical_two_platform_releases_preserve_their_signed_matrix(self):
        for version in ["0.5.1", "0.5.2", "0.5.11", "0.6.0", "0.6.1"]:
            with self.subTest(version=version):
                names = asset_names(version)
                self.assertEqual(names, {
                    "windows-x86_64": f"desk-switch-v{version}-windows-x86-64.exe",
                    "linux-x86_64-deb": f"desk-switch-v{version}-linux-x86-64.deb",
                })
                candidate = {"tag_name": f"v{version}", "prerelease": True, "draft": False,
                             "body": "Notes", "assets": [{"id": index, "name": name, "state": "uploaded", "size": 10, "digest": "sha256:" + "11" * 32} for index, name in enumerate(names.values(), 1)]}
                self.assertEqual(candidate_updates(candidate, version), [])
                candidate["assets"].append({"id": 3, "name": f"desk-switch-v{version}-macos-arm64.zip", "state": "uploaded", "size": 10, "digest": "sha256:" + "22" * 32})
                with self.assertRaises(AssertionError):
                    candidate_updates(candidate, version)

    def test_macos_returns_at_062_with_numeric_version_ordering(self):
        for version in ["0.6.2", "0.6.10", "0.7.0", "0.8.0"]:
            with self.subTest(version=version):
                self.assertEqual(asset_names(version), {
                    "windows-x86_64": f"desk-switch-v{version}-windows-x86-64.exe",
                    "linux-x86_64-deb": f"desk-switch-v{version}-linux-x86-64.deb",
                    "macos-arm64-app": f"desk-switch-v{version}-macos-arm64.zip",
                    "macos-x86_64-app": f"desk-switch-v{version}-macos-x86-64.zip",
                })

    def test_single_windows_executable_uses_the_short_versioned_name(self):
        for version in ["0.8.1", "0.10.0", "1.0.0"]:
            with self.subTest(version=version):
                self.assertEqual(asset_names(version), {
                    "windows-x86_64": f"desk-switch-v{version}.exe",
                    "linux-x86_64-deb": f"desk-switch-v{version}-linux-x86-64.deb",
                    "macos-arm64-app": f"desk-switch-v{version}-macos-arm64.zip",
                    "macos-x86_64-app": f"desk-switch-v{version}-macos-x86-64.zip",
                })

    def test_historical_two_platform_releases_reject_mac_archives_before_signing(self):
        root = self.directory
        (root / "src").mkdir()
        (root / "src/compatibility.rs").write_text("pub const PROTOCOL: u16 = 7;")
        for version in ["0.5.1", "0.6.0", "0.6.1"]:
            (root / "Cargo.toml").write_text(f'[package]\nversion = "{version}"\n')
            with self.subTest(version=version), patch.object(prepare_release, "ROOT", root), patch("sys.argv", ["prepare_release.py", "--windows", "missing.exe", "--deb", "missing.deb", "--tag", f"v{version}", "--macos-arm64", "unexpected.zip"]), patch.object(prepare_release.subprocess, "check_output", side_effect=AssertionError("Signing key must not be accessed")), self.assertRaisesRegex(ValueError, "This release does not include macOS"):
                prepare_release.main()

    def test_both_mac_architectures_pass_without_executing_the_bundle(self):
        for architecture in ["arm64", "x86_64"]:
            with self.subTest(architecture=architecture):
                self.check(bundle(architecture), architecture)

    def test_restored_mac_archives_require_the_current_version_and_protocol(self):
        for architecture in ["arm64", "x86_64"]:
            with self.subTest(architecture=architecture):
                current = archive(bundle(architecture, version="0.6.2", protocol=8))
                self.path.write_bytes(current)
                self.assertEqual(checked_macos_archive(self.path, f"macos-{architecture}-app", "0.6.2", 8), current)
                self.path.write_bytes(archive(bundle(architecture)))
                with self.assertRaises(ValueError):
                    checked_macos_archive(self.path, f"macos-{architecture}-app", "0.6.2", 8)

    def test_wrong_architecture_and_non_executable_macho_are_rejected(self):
        self.reject(bundle("x86_64"), "arm64")
        files = bundle()
        data = bytearray(files[EXECUTABLE][0]); data[12:16] = (6).to_bytes(4, "little")
        files[EXECUTABLE] = (bytes(data), stat.S_IFREG | 0o755)
        self.reject(files)

    def test_wrong_or_conflicting_build_stamps_are_rejected(self):
        for value in [b'"app":"0.5.0"', b'"protocol":7']:
            files = bundle()
            data = files[EXECUTABLE][0].replace(value, value.replace(b"0.5.0", b"0.4.11").replace(b":7", b":6"))
            files[EXECUTABLE] = (data, stat.S_IFREG | 0o755)
            self.reject(files)
        files = bundle()
        files[EXECUTABLE] = (files[EXECUTABLE][0] + b'DESK-SWITCH-BUILD:{"app":"0.4.11","protocol":7}', stat.S_IFREG | 0o755)
        self.reject(files)

    def test_bundle_identity_and_both_version_fields_must_match(self):
        for key in ["CFBundleIdentifier", "CFBundleExecutable", "CFBundlePackageType", "CFBundleVersion", "CFBundleShortVersionString"]:
            files = bundle()
            info = plistlib.loads(files[PLIST][0]); info[key] = "different"
            files[PLIST] = (plistlib.dumps(info), stat.S_IFREG | 0o644)
            with self.subTest(key=key):
                self.reject(files)

    def test_required_helpers_and_executable_modes_are_checked(self):
        for name in [EXECUTABLE, PLIST, NATIVE, DDC]:
            files = bundle(); del files[name]
            with self.subTest(missing=name):
                self.reject(files)
        for name in [EXECUTABLE, NATIVE, DDC]:
            files = bundle(); files[name] = (files[name][0], stat.S_IFREG | 0o644)
            with self.subTest(non_executable=name):
                self.reject(files)

    def test_helper_bytes_must_match_the_main_executable(self):
        files = bundle(); files[NATIVE] = (files[NATIVE][0] + b"tampered", stat.S_IFREG | 0o755)
        self.reject(files)
        files = bundle(); files[DDC] = (macho("x86_64", b"wrong-architecture"), stat.S_IFREG | 0o755)
        self.reject(files)

    def test_links_traversal_aliases_and_extra_bundle_roots_are_rejected(self):
        for name, mode in [("../outside", stat.S_IFREG), ("/absolute", stat.S_IFREG),
                           ("Other.app/Contents/file", stat.S_IFREG),
                           ("Desk Switch.app/Contents/../outside", stat.S_IFREG),
                           ("Desk Switch.app/Contents\\outside", stat.S_IFREG),
                           ("Desk Switch.app/Contents/bad\0name", stat.S_IFREG),
                           ("Desk Switch.app/Contents//alias", stat.S_IFREG),
                           ("Desk Switch.app/Contents/./alias", stat.S_IFREG),
                           ("Desk Switch.app/Contents/link", stat.S_IFLNK),
                           ("Desk Switch.app/Contents/pipe", stat.S_IFIFO)]:
            files = bundle(); files[name] = (b"outside", mode | 0o644)
            with self.subTest(name=name):
                self.reject(files)

    def test_windows_and_linux_binary_checks_remain_architecture_and_stamp_specific(self):
        stamp = b'DESK-SWITCH-BUILD:{"app":"0.5.0","protocol":7}'
        windows = bytearray(128)
        windows[:2] = b"MZ"; windows[60:64] = (64).to_bytes(4, "little")
        windows[64:70] = b"PE\0\0\x64\x86"
        linux = bytearray(64)
        linux[:6] = b"\x7fELF\x02\x01"; linux[18:20] = (62).to_bytes(2, "little")
        for platform, binary, wrong in [("windows-x86_64", bytes(windows), bytes(linux)),
                                         ("linux-x86_64", bytes(linux), bytes(windows))]:
            with self.subTest(platform=platform):
                checked_binary_data(binary + stamp, platform, VERSION, PROTOCOL)
                with self.assertRaises(ValueError):
                    checked_binary_data(wrong + stamp, platform, VERSION, PROTOCOL)
                with self.assertRaises(ValueError):
                    checked_binary_data(binary, platform, VERSION, PROTOCOL)

    def test_case_and_unicode_colliding_paths_are_rejected(self):
        for names in [("Icon.icns", "icon.icns"), ("caf\u00e9", "cafe\u0301")]:
            files = bundle()
            for name in names:
                files["Desk Switch.app/Contents/Resources/" + name] = (b"resource", stat.S_IFREG | 0o644)
            self.reject(files)

    def test_expanded_and_compressed_size_limits_are_enforced_with_small_fixtures(self):
        files = bundle()
        with patch.object(prepare_release, "MAX_BUNDLE_BYTES", 100):
            self.reject(files)
        self.path.write_bytes(archive(files))
        with patch.object(prepare_release, "MAX_ARTIFACT_BYTES", 100), self.assertRaises(ValueError):
            checked_macos_archive(self.path, "macos-arm64-app", VERSION, PROTOCOL)

    def test_missing_mac_target_fails_before_accessing_the_signing_key(self):
        root = self.directory
        (root / "src").mkdir()
        (root / "src/compatibility.rs").write_text("pub const PROTOCOL: u16 = 7;")
        for version in ["0.5.0", "0.6.2", "1.0.0"]:
            (root / "Cargo.toml").write_text(f'[package]\nversion = "{version}"\n')
            with self.subTest(version=version), patch.object(prepare_release, "ROOT", root), patch("sys.argv", ["prepare_release.py", "--windows", "missing.exe", "--deb", "missing.deb", "--tag", f"v{version}"]), patch.object(prepare_release.subprocess, "check_output", side_effect=AssertionError("Signing key must not be accessed")), self.assertRaisesRegex(ValueError, "Missing required macos-arm64-app"):
                prepare_release.main()

    def test_restored_release_requires_intel_archive_before_accessing_the_signing_key(self):
        root = self.directory
        (root / "src").mkdir()
        (root / "Cargo.toml").write_text('[package]\nversion = "0.6.2"\n')
        (root / "src/compatibility.rs").write_text("pub const PROTOCOL: u16 = 8;")
        self.path.write_bytes(archive(bundle(version="0.6.2", protocol=8)))
        with patch.object(prepare_release, "ROOT", root), patch("sys.argv", ["prepare_release.py", "--windows", "missing.exe", "--deb", "missing.deb", "--tag", "v0.6.2", "--macos-arm64", str(self.path)]), patch.object(prepare_release.subprocess, "check_output", side_effect=AssertionError("Signing key must not be accessed")), self.assertRaisesRegex(ValueError, "Missing required macos-x86_64-app"):
            prepare_release.main()

    def test_new_candidates_require_all_four_assets_without_renaming_mac_archives(self):
        for version in ["0.5.0", "0.6.2", "1.0.0"]:
            names = asset_names(version)
            candidate = {"tag_name": f"v{version}", "prerelease": True, "draft": False,
                         "body": "Notes", "assets": [{"id": index, "name": name, "state": "uploaded", "size": 10, "digest": "sha256:" + "11" * 32} for index, name in enumerate(names.values(), 1)]}
            with self.subTest(version=version):
                self.assertEqual(candidate_updates(candidate, version), [])
                for missing in range(len(names)):
                    incomplete = copy.deepcopy(candidate)
                    incomplete["assets"].pop(missing)
                    with self.assertRaises(AssertionError):
                        candidate_updates(incomplete, version)


if __name__ == "__main__":
    unittest.main()
