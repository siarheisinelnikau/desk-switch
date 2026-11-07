#!/usr/bin/env python3
"""Verify native release artifacts and sign their updater metadata."""
import argparse
import hashlib
import json
import plistlib
import re
import shutil
import subprocess
import stat
import tempfile
import tomllib
import unicodedata
import zipfile
from pathlib import Path, PurePosixPath

ROOT = Path(__file__).resolve().parents[1]
REPOSITORY = "siarheisinelnikau/desk-switch"
from release_assets import asset_names

MAX_ARTIFACT_BYTES = 128 * 1024 * 1024
MAX_BUNDLE_BYTES = 256 * 1024 * 1024


def checked_binary(path: Path, platform: str, version: str, protocol: int) -> bytes:
    data = path.read_bytes()
    checked_binary_data(data, platform, version, protocol)
    return data


def checked_binary_data(data: bytes, platform: str, version: str, protocol: int):
    if not 0 < len(data) <= MAX_ARTIFACT_BYTES:
        raise ValueError(f"Invalid {platform} binary size")
    if platform == "linux-x86_64":
        if data[:6] != b"\x7fELF\x02\x01" or int.from_bytes(data[18:20], "little") != 62:
            raise ValueError("The Ubuntu release must be an x86-64 ELF executable")
    elif platform == "windows-x86_64":
        offset = int.from_bytes(data[60:64], "little")
        if data[:2] != b"MZ" or data[offset:offset+6] != b"PE\0\0\x64\x86":
            raise ValueError("The Windows release must be an x86-64 PE executable")
    elif platform in {"macos-arm64", "macos-x86_64"}:
        checked_macho(data, platform)
    else:
        raise ValueError(f"Unsupported release binary platform: {platform}")
    stamps = []
    for raw in re.findall(rb'DESK-SWITCH-BUILD:(\{[^}]{1,127}\})', data):
        try:
            stamps.append(json.loads(raw))
        except (json.JSONDecodeError, UnicodeDecodeError):
            continue
    if not stamps or any(stamp != {"app":version,"protocol":protocol} for stamp in stamps):
        raise ValueError(f"The {platform} binary version/protocol does not match this release")


def checked_macho(data: bytes, platform: str):
    architecture = {"macos-arm64": 0x0100000c, "macos-x86_64": 0x01000007}[platform]
    if (len(data) < 32 or data[:4] != b"\xcf\xfa\xed\xfe"
            or int.from_bytes(data[4:8], "little") != architecture
            or int.from_bytes(data[12:16], "little") != 2):
        raise ValueError(f"The {platform} release must be a thin 64-bit Mach-O executable")


def checked_macos_archive(path: Path, target: str, version: str, protocol: int) -> bytes:
    """Check bundle identity, architecture and ZIP paths without a macOS host."""
    if target not in {"macos-arm64-app", "macos-x86_64-app"}:
        raise ValueError("Unsupported macOS application target")
    data = path.read_bytes()
    if not 0 < len(data) <= MAX_ARTIFACT_BYTES:
        raise ValueError("Invalid macOS archive size")
    executable = "Desk Switch.app/Contents/MacOS/desk-switch"
    helpers = {"Desk Switch.app/Contents/Helpers/desk-switch-native", "Desk Switch.app/Contents/Helpers/desk-switch-ddc"}
    required = {"Desk Switch.app/Contents/Info.plist", executable, *helpers}
    with zipfile.ZipFile(path) as archive:
        entries = archive.infolist()
        if not entries or len(entries) > 4096:
            raise ValueError("Invalid macOS bundle entry count")
        seen, contents = set(), {}
        total = 0
        for entry in entries:
            name = entry.filename.removesuffix("/")
            path_parts = name.split("/")
            folded = unicodedata.normalize("NFC", name).casefold()
            mode = (entry.external_attr >> 16) & 0xffff
            if (not name or "\\" in entry.orig_filename or "\0" in entry.orig_filename
                    or PurePosixPath(name).is_absolute()
                    or any(part in {"", ".", ".."} for part in path_parts)
                    or path_parts[0] != "Desk Switch.app" or folded in seen
                    or entry.flag_bits & 1
                    or stat.S_IFMT(mode) not in {0, stat.S_IFREG, stat.S_IFDIR}
                    or (stat.S_IFMT(mode) == stat.S_IFDIR and not entry.is_dir())
                    or entry.compress_type not in {zipfile.ZIP_STORED, zipfile.ZIP_DEFLATED}):
                raise ValueError("Unsafe or duplicate macOS bundle entry")
            seen.add(folded)
            total += entry.file_size
            if not 0 <= entry.file_size <= MAX_ARTIFACT_BYTES or total > MAX_BUNDLE_BYTES:
                raise ValueError("Expanded macOS bundle is too large")
            with archive.open(entry) as source:
                item = source.read(entry.file_size + 1)
            if len(item) != entry.file_size:
                raise ValueError("Invalid macOS bundle file size")
            if name in required:
                contents[name] = item
                if (name == executable or name in helpers) and mode & 0o111 == 0:
                    raise ValueError("The bundled macOS executable is not executable")
        if set(contents) != required:
            raise ValueError("The macOS bundle is missing its executable or Info.plist")
        if len(contents["Desk Switch.app/Contents/Info.plist"]) > 1024 * 1024:
            raise ValueError("The macOS property list is too large")
        info = plistlib.loads(contents["Desk Switch.app/Contents/Info.plist"])
        expected = {"CFBundleExecutable": "desk-switch", "CFBundleIdentifier": "com.deskswitch.Desktop",
                    "CFBundlePackageType": "APPL", "CFBundleShortVersionString": version,
                    "CFBundleVersion": version}
        if not isinstance(info, dict) or any(info.get(key) != value for key, value in expected.items()):
            raise ValueError("The macOS bundle identity/version does not match this release")
        platform = target.removesuffix("-app")
        checked_binary_data(contents[executable], platform, version, protocol)
        for helper in helpers:
            checked_macho(contents[helper], platform)
            if contents[helper] not in contents[executable]:
                raise ValueError("A macOS helper differs from the executable's embedded copy")
    return data


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--windows", required=True, type=Path)
    parser.add_argument("--tag", required=True)
    parser.add_argument("--deb", required=True, type=Path)
    parser.add_argument("--macos-arm64", type=Path)
    parser.add_argument("--macos-x86-64", type=Path)
    parser.add_argument("--key", type=Path, default=ROOT/".release-private/update-signing-key.pem")
    parser.add_argument("--out", type=Path, default=ROOT/"dist")
    args = parser.parse_args()
    version = tomllib.loads((ROOT/"Cargo.toml").read_text())["package"]["version"]
    if args.tag != f"v{version}" or not re.fullmatch(r"(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)", version):
        raise ValueError("Release tag must exactly match the stable version in Cargo.toml")
    protocol = int(re.search(r"pub const PROTOCOL: u16 = (\d+);", (ROOT/"src/compatibility.rs").read_text())[1])
    targets = {}
    names = asset_names(version)
    sources = {"windows-x86_64": args.windows, "linux-x86_64-deb": args.deb}
    for target, source in [("macos-arm64-app", args.macos_arm64), ("macos-x86_64-app", args.macos_x86_64)]:
        if target in names:
            if source is None:
                raise ValueError(f"Missing required {target} application archive")
            checked_macos_archive(source, target, version, protocol)
            sources[target] = source
        elif source is not None:
            raise ValueError("This historical release does not include macOS applications")
    artifacts = [(sources[target], filename) for target, filename in names.items()]
    checked_binary(args.windows,"windows-x86_64",version,protocol)
    fields = subprocess.check_output(["dpkg-deb", "--field", str(args.deb), "Package", "Version", "Architecture"], text=True)
    if fields.splitlines() != ["Package: desk-switch", f"Version: {version}", "Architecture: amd64"]:
        raise ValueError("Debian installer metadata does not match the release")
    with tempfile.TemporaryDirectory(prefix="desk-switch-verify-deb-") as directory:
        subprocess.run(["dpkg-deb","--extract",str(args.deb),directory],check=True)
        checked_binary(Path(directory)/"usr/lib/desk-switch/desk-switch","linux-x86_64",version,protocol)
    for platform,(source,filename) in zip(names,artifacts):
        data=source.read_bytes()
        if not 0 < len(data) <= MAX_ARTIFACT_BYTES:
            raise ValueError("Invalid installer size")
        targets[platform]={"url":f"https://github.com/{REPOSITORY}/releases/download/{args.tag}/{filename}","sha256":hashlib.sha256(data).hexdigest(),"size":len(data)}
    public = subprocess.check_output(["openssl", "pkey", "-in", str(args.key), "-pubout", "-outform", "DER"])
    pinned = bytes.fromhex((ROOT/"release/update-public-key.hex").read_text().strip())
    if public != bytes.fromhex("302a300506032b6570032100") + pinned:
        raise ValueError("The signing key does not match the public key pinned in the clients")
    payload = json.dumps({"version": version, "protocol": protocol, "targets": targets}, sort_keys=True, separators=(",", ":"))
    with tempfile.TemporaryDirectory(prefix="desk-switch-sign-") as directory:
        message = Path(directory)/"payload.json"
        signature = Path(directory)/"signature.bin"
        message.write_bytes(payload.encode())
        subprocess.run(["openssl", "pkeyutl", "-sign", "-rawin", "-inkey", str(args.key), "-in", str(message), "-out", str(signature)], check=True)
        signed = signature.read_bytes()
    args.out.mkdir(parents=True, exist_ok=True)
    for source, filename in artifacts:
        if source.resolve() != (args.out/filename).resolve():
            shutil.copy2(source, args.out/filename)
    (args.out/"update.json").write_text(json.dumps({"payload": payload, "signature": signed.hex()}, indent=2)+"\n")
    print(f"Prepared signed {args.tag}, protocol {protocol}, {len(targets)} platform/architecture targets, in {args.out}")


if __name__ == "__main__":
    main()
