#!/usr/bin/env python3
"""Prepare the signed release metadata for exactly one .exe and one .deb."""
import argparse
import hashlib
import json
import re
import shutil
import subprocess
import tempfile
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
REPOSITORY = "siarheisinelnikau/desk-switch"
from release_assets import asset_names


def checked_binary(path: Path, platform: str, version: str, protocol: int) -> bytes:
    data = path.read_bytes()
    if not 0 < len(data) <= 128 * 1024 * 1024:
        raise ValueError(f"Invalid {platform} binary size")
    if platform == "linux-x86_64":
        if data[:6] != b"\x7fELF\x02\x01" or int.from_bytes(data[18:20], "little") != 62:
            raise ValueError("The Ubuntu release must be an x86-64 ELF executable")
    else:
        offset = int.from_bytes(data[60:64], "little")
        if data[:2] != b"MZ" or data[offset:offset+6] != b"PE\0\0\x64\x86":
            raise ValueError("The Windows release must be an x86-64 PE executable")
    stamps = []
    for raw in re.findall(rb'DESK-SWITCH-BUILD:(\{[^}]{1,127}\})', data):
        try:
            stamps.append(json.loads(raw))
        except (json.JSONDecodeError, UnicodeDecodeError):
            continue
    if not stamps or any(stamp != {"app":version,"protocol":protocol} for stamp in stamps):
        raise ValueError(f"The {platform} binary version/protocol does not match this release")
    return data


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--windows", required=True, type=Path)
    parser.add_argument("--tag", required=True)
    parser.add_argument("--deb", required=True, type=Path)
    parser.add_argument("--key", type=Path, default=ROOT/".release-private/update-signing-key.pem")
    parser.add_argument("--out", type=Path, default=ROOT/"dist")
    args = parser.parse_args()
    version = tomllib.loads((ROOT/"Cargo.toml").read_text())["package"]["version"]
    if args.tag != f"v{version}" or not re.fullmatch(r"(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)", version):
        raise ValueError("Release tag must exactly match the stable version in Cargo.toml")
    protocol = int(re.search(r"pub const PROTOCOL: u16 = (\d+);", (ROOT/"src/compatibility.rs").read_text())[1])
    public = subprocess.check_output(["openssl", "pkey", "-in", str(args.key), "-pubout", "-outform", "DER"])
    pinned = bytes.fromhex((ROOT/"release/update-public-key.hex").read_text().strip())
    if public != bytes.fromhex("302a300506032b6570032100") + pinned:
        raise ValueError("The signing key does not match the public key pinned in the clients")
    targets = {}
    names = asset_names(version)
    artifacts = [(args.windows,names["windows-x86_64"]),(args.deb,names["linux-x86_64-deb"])]
    checked_binary(args.windows,"windows-x86_64",version,protocol)
    fields = subprocess.check_output(["dpkg-deb", "--field", str(args.deb), "Package", "Version", "Architecture"], text=True)
    if fields.splitlines() != ["Package: desk-switch", f"Version: {version}", "Architecture: amd64"]:
        raise ValueError("Debian installer metadata does not match the release")
    with tempfile.TemporaryDirectory(prefix="desk-switch-verify-deb-") as directory:
        subprocess.run(["dpkg-deb","--extract",str(args.deb),directory],check=True)
        checked_binary(Path(directory)/"usr/lib/desk-switch/desk-switch","linux-x86_64",version,protocol)
    for platform,(source,filename) in zip(["windows-x86_64","linux-x86_64-deb"],artifacts):
        data=source.read_bytes()
        if not 0 < len(data) <= 128*1024*1024:
            raise ValueError("Invalid installer size")
        targets[platform]={"url":f"https://github.com/{REPOSITORY}/releases/download/{args.tag}/{filename}","sha256":hashlib.sha256(data).hexdigest(),"size":len(data)}
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
    print(f"Prepared signed {args.tag}, protocol {protocol}, both platforms, in {args.out}")


if __name__ == "__main__":
    main()
