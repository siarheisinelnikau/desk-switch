#!/usr/bin/env python3
"""Package a native macOS 13+ build as a verified, ad-hoc-signed .app ZIP."""
import argparse
import os
from pathlib import Path
import plistlib
import re
import shutil
import stat
import subprocess
import sys
import tempfile
import tomllib
import zipfile

from prepare_release import checked_binary_data, checked_macos_archive

ROOT = Path(__file__).resolve().parents[1]


def run(*arguments, **kwargs):
    return subprocess.run(arguments, check=True, timeout=120, **kwargs)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--out", type=Path, default=ROOT / "dist")
    args = parser.parse_args()
    if sys.platform != "darwin":
        raise RuntimeError("macOS packaging requires the Apple SDK, iconutil and codesign")
    version = tomllib.loads((ROOT / "Cargo.toml").read_text())["package"]["version"]
    protocol = int(re.search(r"pub const PROTOCOL: u16 = (\d+);", (ROOT / "src/compatibility.rs").read_text())[1])
    binary = args.binary.resolve(strict=True)
    data = binary.read_bytes()
    cpu = int.from_bytes(data[4:8], "little")
    architecture = {0x0100000C: "arm64", 0x01000007: "x86-64"}.get(cpu)
    if architecture is None:
        raise ValueError("Expected an arm64 or x86-64 Mach-O executable")
    key = "macos-arm64-app" if architecture == "arm64" else "macos-x86_64-app"
    checked_binary_data(data, key.removesuffix("-app"), version, protocol)
    run(str(binary), "--build-info", stdout=subprocess.DEVNULL)
    args.out.mkdir(parents=True, exist_ok=True)
    output = args.out.resolve() / f"desk-switch-v{version}-macos-{architecture}.zip"
    with tempfile.TemporaryDirectory(prefix="desk-switch-macos-") as temporary:
        work = Path(temporary)
        app = work / "Desk Switch.app"
        contents = app / "Contents"
        for name in ("MacOS", "Helpers", "Resources"):
            (contents / name).mkdir(parents=True)
        main_binary = contents / "MacOS/desk-switch"
        shutil.copyfile(binary, main_binary)
        main_binary.chmod(0o755)
        for name in ("desk-switch-native", "desk-switch-ddc"):
            helper = contents / "Helpers" / name
            run(str(binary), "macos-export-helper", name, str(helper))
            helper.chmod(0o755)
            run(str(helper), "--version", stdout=subprocess.DEVNULL)
            # build.rs signs helpers before embedding them. Re-signing here
            # would change the bytes checked by the running application.
            run("/usr/bin/codesign", "--verify", "--strict", str(helper))
        info = {
            "CFBundleIdentifier": "com.deskswitch.Desktop",
            "CFBundleName": "Desk Switch",
            "CFBundleDisplayName": "Desk Switch",
            "CFBundleExecutable": "desk-switch",
            "CFBundlePackageType": "APPL",
            "CFBundleShortVersionString": version,
            "CFBundleVersion": version,
            "CFBundleIconFile": "DeskSwitch.icns",
            "LSMinimumSystemVersion": "13.0",
            "NSHighResolutionCapable": True,
            "NSMicrophoneUsageDescription": "Share the selected microphone with computers you paired in Desk Switch.",
            "NSAudioCaptureUsageDescription": "Share system audio with computers you paired in Desk Switch.",
            "NSHumanReadableCopyright": "Copyright Desk Switch contributors. MIT license.",
        }
        (contents / "Info.plist").write_bytes(plistlib.dumps(info, fmt=plistlib.FMT_XML, sort_keys=True))
        iconset = work / "DeskSwitch.iconset"
        iconset.mkdir()
        for points in (16, 32, 128, 256, 512):
            for scale in (1, 2):
                pixels = points * scale
                source = ROOT / f"assets/icons/desk-switch-{min(pixels, 256)}.png"
                destination = iconset / f"icon_{points}x{points}{'@2x' if scale == 2 else ''}.png"
                if pixels <= 256:
                    shutil.copyfile(source, destination)
                else:
                    run("/usr/bin/sips", "-z", str(pixels), str(pixels), str(source), "--out", str(destination), stdout=subprocess.DEVNULL)
        run("/usr/bin/iconutil", "-c", "icns", str(iconset), "-o", str(contents / "Resources/DeskSwitch.icns"))
        notices = {
            "LICENSE.txt": ROOT / "LICENSE",
            "m1ddc-LICENSE.txt": ROOT / "vendor/m1ddc/LICENSE",
            "VB-CABLE-NOTICE.txt": ROOT / "assets/third-party/vb-cable/NOTICE.txt",
        }
        for name, source in notices.items():
            shutil.copyfile(source, contents / "Resources" / name)
        # No --deep: preserve the already embedded nested helper signatures.
        run("/usr/bin/codesign", "--force", "--sign", "-", "--timestamp=none", str(app))
        run("/usr/bin/codesign", "--verify", "--deep", "--strict", str(app))
        with zipfile.ZipFile(output, "w", compression=zipfile.ZIP_DEFLATED, compresslevel=9) as archive:
            for file in sorted(app.rglob("*")):
                if file.is_symlink():
                    raise ValueError("The macOS bundle must not contain symbolic links")
                if file.is_file():
                    entry = zipfile.ZipInfo(file.relative_to(work).as_posix(), (2020, 1, 1, 0, 0, 0))
                    entry.create_system = 3
                    entry.compress_type = zipfile.ZIP_DEFLATED
                    entry.external_attr = (stat.S_IFREG | (0o755 if file in (main_binary, contents / "Helpers/desk-switch-native", contents / "Helpers/desk-switch-ddc") else 0o644)) << 16
                    archive.writestr(entry, file.read_bytes())
        checked_macos_archive(output, key, version, protocol)
    print(f"Prepared {output} (macOS 13+, {architecture}; ad-hoc signed, not notarized)")


if __name__ == "__main__":
    main()
