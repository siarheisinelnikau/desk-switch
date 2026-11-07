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
SIGNED_START = "<!-- desk-switch-update:start -->"
SIGNED_END = "<!-- desk-switch-update:end -->"


def signed_notes(body, manifest, fallback):
    """Preserve release notes, attach missing metadata, reject a conflicting block."""
    expected = json.loads(manifest)
    if SIGNED_START in body or SIGNED_END in body:
        assert body.count(SIGNED_START) == body.count(SIGNED_END) == 1, "Malformed signed update block"
        before, rest = body.split(SIGNED_START, 1)
        signed, after = rest.split(SIGNED_END, 1)
        assert SIGNED_END not in before and SIGNED_START not in after, "Malformed signed update block"
        assert json.loads(signed.strip()) == expected, "A different signed update is already attached"
        return body
    block = SIGNED_START + "\n" + manifest.strip() + "\n" + SIGNED_END
    return (body.strip() or fallback.strip()) + "\n\n" + block + "\n"


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
        targets = {"windows-x86_64": "desk-switch-windows-x86_64.exe", "linux-x86_64-deb": "desk-switch-linux-x86_64.deb"}
        assert set(release["targets"]) == set(targets)
        info = json.loads(subprocess.check_output(["gh", "release", "view", tag, "--repo", REPOSITORY, "--json", "isPrerelease,assets,body"], text=True))
        assets = {asset["name"]: asset for asset in info["assets"]}
        assert set(assets) == set(targets.values()), "Only the .deb and .exe belong in this release"
        for platform, filename in targets.items():
            artifact = release["targets"][platform]
            assert artifact["url"] == f"https://github.com/{REPOSITORY}/releases/download/{tag}/{filename}"
            assert 0 < artifact["size"] <= 128 * 1024 * 1024
            path = work / filename
            with urllib.request.urlopen(artifact["url"], timeout=60) as response:
                data = response.read(artifact["size"] + 1)
            assert len(data) == artifact["size"] and hashlib.sha256(data).hexdigest() == artifact["sha256"]
            path.write_bytes(data)
            if platform == "windows-x86_64":
                checked_binary(path, platform, version, protocol)
            else:
                subprocess.run(["dpkg-deb", "--extract", str(path), str(work / "installed")], check=True)
                checked_binary(work / "installed/usr/lib/desk-switch/desk-switch", "linux-x86_64", version, protocol)
                assert subprocess.check_output(["dpkg-deb", "--field", str(path), "Package"], text=True).strip() == "desk-switch"
                assert subprocess.check_output(["dpkg-deb", "--field", str(path), "Version"], text=True).strip() == version
                assert subprocess.check_output(["dpkg-deb", "--field", str(path), "Architecture"], text=True).strip() == "amd64"
        notes = work / "notes.md"
        # A maintainer may have cleared Prerelease manually before this job.
        # The signature, both binaries and package metadata were still verified
        # above. Repair the missing envelope without changing the release files.
        expected_notes = signed_notes(info["body"], manifest.read_text(), (ROOT / f"release/notes/{version}.md").read_text())
        notes.write_text(expected_notes)
        subprocess.run(["gh", "release", "edit", tag, "--repo", REPOSITORY, "--notes-file", str(notes), "--prerelease=false", "--latest"], check=True)
        published = json.loads(subprocess.check_output(["gh", "release", "view", tag, "--repo", REPOSITORY, "--json", "isPrerelease,body"], text=True))
        assert not published["isPrerelease"]
        assert signed_notes(published["body"], manifest.read_text(), "") == published["body"], "Signed metadata was not saved"
        latest = json.loads(subprocess.check_output(["gh", "api", f"repos/{REPOSITORY}/releases/latest"], text=True))
        assert latest["tag_name"] == tag, "The signed release is not the update channel's latest release"
        print(f"Published signed stable {tag}: .deb + .exe only")



if __name__ == "__main__":
    main()
