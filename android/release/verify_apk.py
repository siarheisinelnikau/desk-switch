#!/usr/bin/env python3
"""Verify the identity and native contents of an Android release candidate."""

import argparse
import os
from pathlib import Path
import re
import subprocess
import zipfile


PACKAGE = "io.github.deskswitch.remote.experimental"
LAUNCHER = "io.github.deskswitch.remote.ui.RemoteActivity"
ABIS = {"arm64-v8a", "armeabi-v7a", "x86", "x86_64"}
FINGERPRINT_PATH = Path(__file__).resolve().parents[1] / "release-signing.sha256"


def verify_metadata(apk, badging, version, version_code):
    package_line = next((line for line in badging.splitlines() if line.startswith("package: ")), "")
    attributes = dict(re.findall(r"(\w+)='([^']*)'", package_line))
    expected = {"name": PACKAGE, "versionName": f"{version}-experimental", "versionCode": version_code}
    if any(attributes.get(key) != value for key, value in expected.items()):
        raise SystemExit(f"Unexpected APK package or version: {package_line}")

    if "application-debuggable" in badging.splitlines():
        raise SystemExit("Release APK is debuggable")
    if not re.search(rf"^launchable-activity: name='{re.escape(LAUNCHER)}'", badging, re.MULTILINE):
        raise SystemExit("Release APK is missing the Desk Switch Remote launcher activity")

    with zipfile.ZipFile(apk) as archive:
        native_entries = [name for name in archive.namelist() if name.startswith("lib/") and name.endswith(".so")]
        native_abis = {name.split("/")[1] for name in native_entries}
        if native_abis != ABIS or any(
            native_entries.count(f"lib/{abi}/libdesk_switch_noise.so") != 1 for abi in ABIS
        ):
            raise SystemExit("Release APK must contain one Desk Switch Noise library for each supported ABI")


def verify_certificate(output, expected):
    if not re.fullmatch(r"[0-9a-f]{64}", expected):
        raise SystemExit("Expected a single lowercase SHA-256 digest in release-signing.sha256")
    signers = re.findall(r"^Signer #[0-9]+ certificate SHA-256 digest: ([0-9a-f]{64})$", output, re.MULTILINE)
    if signers != [expected]:
        raise SystemExit("Release APK signer does not match release-signing.sha256")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("apk", type=Path)
    parser.add_argument("version")
    parser.add_argument("version_code")
    parser.add_argument("--verify-signature", action="store_true")
    args = parser.parse_args()
    if not re.fullmatch(r"[1-9][0-9]*", args.version_code):
        parser.error("version_code must be a positive integer")

    build_tools = Path(os.environ["ANDROID_HOME"]) / "build-tools" / "35.0.0"
    aapt2 = build_tools / ("aapt2.exe" if os.name == "nt" else "aapt2")
    badging = subprocess.check_output([aapt2, "dump", "badging", args.apk], encoding="utf-8")
    verify_metadata(args.apk, badging, args.version, args.version_code)

    if args.verify_signature:
        java = "java.exe" if os.name == "nt" else "java"
        if java_home := os.environ.get("JAVA_HOME"):
            java = Path(java_home) / "bin" / java
        signature = subprocess.check_output(
            [java, "-jar", build_tools / "lib" / "apksigner.jar",
             "verify", "--verbose", "--print-certs", args.apk],
            encoding="utf-8",
        )
        verify_certificate(signature, FINGERPRINT_PATH.read_text(encoding="ascii").strip())
    print(f"Verified {args.apk.name}")


if __name__ == "__main__":
    main()
