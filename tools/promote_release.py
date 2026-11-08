#!/usr/bin/env python3
"""Verify a locally signed manifest and complete its already-built GitHub release."""
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile
import time
import tomllib
import urllib.request
import urllib.error

from prepare_release import checked_binary, checked_macos_archive
from release_assets import asset_names

ROOT = Path(__file__).resolve().parents[1]
REPOSITORY = "siarheisinelnikau/desk-switch"
SIGNED_START = "<!-- desk-switch-update:start -->"
SIGNED_END = "<!-- desk-switch-update:end -->"


def download_asset(artifact, asset, destination):
    """Read the public file, with a bounded official-API fallback for outages."""
    request = urllib.request.Request(artifact["url"], headers={
        "User-Agent": "Desk-Switch-release-verifier",
        "Accept": "application/octet-stream",
    })
    for attempt in range(2):
        try:
            with urllib.request.urlopen(request, timeout=60) as response:
                return response.read(artifact["size"] + 1)
        except urllib.error.HTTPError as error:
            if error.code not in {408, 425, 429, 500, 502, 503, 504}:
                raise
            error.close()
            print(f"Public download returned HTTP {error.code}: {asset['name']}", flush=True)
        except (urllib.error.URLError, TimeoutError):
            print(f"Public download temporarily unavailable: {asset['name']}", flush=True)
        if attempt == 0:
            time.sleep(1)
    # gh follows the official asset endpoint's redirect. The token stays in the
    # established CI environment, and the exact signed bytes are checked below.
    assert type(asset["id"]) is int and asset["id"] > 0, "Invalid release asset identity"
    print(f"Verifying through the official GitHub asset API: {asset['name']}", flush=True)
    with destination.open("wb") as output:
        subprocess.run([
            "gh", "api", "-H", "Accept: application/octet-stream",
            f"repos/{REPOSITORY}/releases/assets/{asset['id']}",
        ], stdout=output, check=True, timeout=90)
    with destination.open("rb") as downloaded:
        return downloaded.read(artifact["size"] + 1)


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
        targets = asset_names(version)
        assert set(release["targets"]) == set(targets)
        info = json.loads(subprocess.check_output(["gh", "api", f"repos/{REPOSITORY}/releases/tags/{tag}"], text=True))
        assert info["tag_name"] == tag and not info["draft"], "Unexpected release candidate"
        expected_source = os.environ.get("DESK_SWITCH_RELEASE_SOURCE_SHA") or os.environ.get("GITHUB_SHA")
        if expected_source:
            assert re.fullmatch(r"[0-9a-f]{40}", expected_source), "Invalid release source"
            ref = json.loads(subprocess.check_output(["gh", "api", f"repos/{REPOSITORY}/git/ref/tags/{tag}"], text=True))
            assert ref["object"]["type"] == "commit" and ref["object"]["sha"] == expected_source, "Release tag belongs to another source"
        assets = {asset["name"]: asset for asset in info["assets"]}
        assert set(assets) == set(targets.values()), "Release artifacts do not match the signed platform/architecture matrix"
        for platform, filename in targets.items():
            artifact = release["targets"][platform]
            assert artifact["url"] == f"https://github.com/{REPOSITORY}/releases/download/{tag}/{filename}"
            assert 0 < artifact["size"] <= 128 * 1024 * 1024
            asset = assets[filename]
            assert asset["browser_download_url"] == artifact["url"] and asset["state"] == "uploaded", "Unexpected release download"
            assert asset["size"] == artifact["size"] and asset["digest"] == "sha256:" + artifact["sha256"], "Release asset metadata differs from its signature"
            path = work / filename
            data = download_asset(artifact, asset, path)
            assert len(data) == artifact["size"] and hashlib.sha256(data).hexdigest() == artifact["sha256"]
            path.write_bytes(data)
            if platform == "windows-x86_64":
                checked_binary(path, platform, version, protocol)
            elif platform.startswith("macos-"):
                checked_macos_archive(path, platform, version, protocol)
            elif platform == "linux-x86_64-deb":
                subprocess.run(["dpkg-deb", "--extract", str(path), str(work / "installed")], check=True)
                checked_binary(work / "installed/usr/lib/desk-switch/desk-switch", "linux-x86_64", version, protocol)
                assert subprocess.check_output(["dpkg-deb", "--field", str(path), "Package"], text=True).strip() == "desk-switch"
                assert subprocess.check_output(["dpkg-deb", "--field", str(path), "Version"], text=True).strip() == version
                assert subprocess.check_output(["dpkg-deb", "--field", str(path), "Architecture"], text=True).strip() == "amd64"
        notes = work / "notes.md"
        # A maintainer may have cleared Prerelease manually before this job.
        # The signature, native binaries and package metadata were still verified
        # above. Repair the missing envelope without changing the release files.
        expected_notes = signed_notes(info["body"], manifest.read_text(), (ROOT / f"release/notes/{version}.md").read_text())
        notes.write_text(expected_notes)
        subprocess.run(["gh", "release", "edit", tag, "--repo", REPOSITORY, "--notes-file", str(notes), "--prerelease=false", "--latest"], check=True)
        published = json.loads(subprocess.check_output(["gh", "release", "view", tag, "--repo", REPOSITORY, "--json", "isPrerelease,body"], text=True))
        assert not published["isPrerelease"]
        assert signed_notes(published["body"], manifest.read_text(), "") == published["body"], "Signed metadata was not saved"
        latest = json.loads(subprocess.check_output(["gh", "api", f"repos/{REPOSITORY}/releases/latest"], text=True))
        assert latest["tag_name"] == tag, "The signed release is not the update channel's latest release"
        print(f"Published signed stable {tag}: {len(targets)} verified native artifacts")



if __name__ == "__main__":
    main()
