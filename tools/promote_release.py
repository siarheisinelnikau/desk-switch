#!/usr/bin/env python3
"""Verify a locally signed manifest and complete its already-built GitHub release."""
import hashlib
import json
from pathlib import Path
import re
import subprocess
import tempfile
import tomllib
import urllib.request

from prepare_release import checked_binary

ROOT = Path(__file__).resolve().parents[1]
REPOSITORY = "siarheisinelnikau/desk-switch"


def main():
    version = tomllib.loads((ROOT / "Cargo.toml").read_text())["package"]["version"]
    tag = f"v{version}"
    protocol = int(re.search(r"pub const PROTOCOL: u16 = (\d+);", (ROOT / "src/compatibility.rs").read_text())[1])
    manifest = ROOT / f"release/manifests/{version}.json"
    envelope = json.loads(manifest.read_text())
    public = bytes.fromhex((ROOT / "release/update-public-key.hex").read_text().strip())
    with tempfile.TemporaryDirectory(prefix="desk-switch-promote-") as directory:
        work = Path(directory)
        (work / "key.der").write_bytes(bytes.fromhex("302a300506032b6570032100") + public)
        (work / "signature").write_bytes(bytes.fromhex(envelope["signature"]))
        (work / "payload").write_bytes(envelope["payload"].encode())
        subprocess.run(["openssl", "pkeyutl", "-verify", "-rawin", "-pubin", "-keyform", "DER", "-inkey", str(work / "key.der"), "-sigfile", str(work / "signature"), "-in", str(work / "payload")], check=True)
        release = json.loads(envelope["payload"])
        assert release["version"] == version and release["protocol"] == protocol
        targets = {"linux-x86_64": "desk-switch-linux-x86_64", "windows-x86_64": "desk-switch-windows-x86_64.exe"}
        assert set(release["targets"]) == set(targets) | {"linux-x86_64-deb"}
        info = json.loads(subprocess.check_output(["gh", "release", "view", tag, "--repo", REPOSITORY, "--json", "isPrerelease,assets"], text=True))
        assets = {asset["name"]: asset for asset in info["assets"]}
        assert set(targets.values()) | {"desk-switch-linux-x86_64.deb"} <= set(assets)
        for platform, filename in targets.items():
            artifact = release["targets"][platform]
            assert artifact["url"] == f"https://github.com/{REPOSITORY}/releases/download/{tag}/{filename}"
            assert 0 < artifact["size"] <= 128 * 1024 * 1024
            path = work / filename
            with urllib.request.urlopen(artifact["url"], timeout=60) as response:
                data = response.read(artifact["size"] + 1)
            assert len(data) == artifact["size"] and hashlib.sha256(data).hexdigest() == artifact["sha256"]
            path.write_bytes(data)
            checked_binary(path, platform, version, protocol)
        # The installer must contain the exact Linux binary covered by the signature.
        package = work / "package.deb"
        with urllib.request.urlopen(f"https://github.com/{REPOSITORY}/releases/download/{tag}/desk-switch-linux-x86_64.deb", timeout=60) as response:
            package.write_bytes(response.read(128 * 1024 * 1024 + 1))
        assert package.stat().st_size <= 128 * 1024 * 1024
        installer = release["targets"]["linux-x86_64-deb"]
        assert installer["sha256"] == hashlib.sha256(package.read_bytes()).hexdigest()
        assert installer["size"] == package.stat().st_size
        subprocess.run(["dpkg-deb", "--extract", str(package), str(work / "installed")], check=True)
        bundled = work / "installed/usr/lib/desk-switch/desk-switch"
        assert hashlib.sha256(bundled.read_bytes()).hexdigest() == release["targets"]["linux-x86_64"]["sha256"]
        assert subprocess.check_output(["dpkg-deb", "--field", str(package), "Version"], text=True).strip() == version
        update = work / "update.json"
        update.write_bytes(manifest.read_bytes())
        if not info["isPrerelease"]:
            with urllib.request.urlopen(f"https://github.com/{REPOSITORY}/releases/download/{tag}/update.json", timeout=30) as response:
                assert response.read(65537) == update.read_bytes()
            print("Already published with this signed manifest")
            return
        subprocess.run(["gh", "release", "upload", tag, str(update), "--repo", REPOSITORY, "--clobber"], check=True)
        subprocess.run(["gh", "release", "edit", tag, "--repo", REPOSITORY, "--prerelease=false", "--latest"], check=True)
        print(f"Published signed stable {tag}, including the Debian installer")


if __name__ == "__main__":
    main()
