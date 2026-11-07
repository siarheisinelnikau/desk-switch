#!/usr/bin/env python3
"""Rename an unsigned release candidate in place without replacing its installers."""
import json
from pathlib import Path
import re
import subprocess
import tomllib

from promote_release import REPOSITORY, SIGNED_START, SIGNED_END
from release_assets import asset_names

ROOT = Path(__file__).resolve().parents[1]


def candidate_updates(release, version):
    """Preflight the entire candidate before making any remote changes."""
    assert release["tag_name"] == f"v{version}", "Unexpected release version"
    assert release["prerelease"] and not release["draft"], "Only published prerelease candidates can be renamed"
    assert not release.get("immutable"), "Immutable releases cannot be renamed"
    body = release.get("body") or ""
    assert SIGNED_START not in body and SIGNED_END not in body, "Signed artifact URLs cannot be renamed"
    names = asset_names(version)
    previous = {
        "windows-x86_64": f"desk-switch-v{version}.exe",
        "linux-x86_64-deb": "desk-switch-linux-x86_64.deb",
    }
    assets = release["assets"]
    assert len(assets) == len(names), "Expected the complete native platform/architecture matrix"
    assert len({asset["id"] for asset in assets}) == len(assets), "Duplicate asset identifiers"
    updates = []
    for platform, canonical in names.items():
        matches = [asset for asset in assets if asset["name"] in {previous.get(platform, canonical), canonical}]
        assert len(matches) == 1, f"Unexpected or duplicate installer for {platform}"
        asset = matches[0]
        assert asset["state"] == "uploaded" and asset["size"] > 0, "Installer upload is incomplete"
        assert re.fullmatch(r"sha256:[0-9a-f]{64}", asset.get("digest") or ""), "Missing installer digest"
        if asset["name"] != canonical:
            updates.append({"id": asset["id"], "name": canonical})
    return updates


def verify_result(before, after, version, notes):
    assert before["id"] == after["id"], "Release identity changed"
    assert not candidate_updates(after, version), "Installer names were not saved"
    assert after["body"] == notes, "Release notes were not saved"
    snapshot = lambda release: {
        asset["id"]: (asset["size"], asset["digest"]) for asset in release["assets"]
    }
    assert snapshot(before) == snapshot(after), "An installer was replaced or its bytes changed"


def api(endpoint, payload=None, missing_ok=False):
    command = ["gh", "api", endpoint]
    if payload is not None:
        command += ["--method", "PATCH", "--input", "-"]
    result = subprocess.run(command, input=json.dumps(payload) if payload is not None else None,
                            capture_output=True, text=True)
    if missing_ok and result.returncode and "HTTP 404" in result.stderr:
        return None
    if result.returncode:
        raise RuntimeError(f"GitHub API failed: {result.stderr.strip()}")
    return json.loads(result.stdout)


def main():
    version = tomllib.loads((ROOT / "Cargo.toml").read_text())["package"]["version"]
    endpoint = f"repos/{REPOSITORY}/releases"
    release = api(f"{endpoint}/tags/v{version}", missing_ok=True)
    if release is None or not release["prerelease"]:
        print(f"No unsigned prerelease candidate to rename for v{version}; stable releases are unchanged.")
        return
    updates = candidate_updates(release, version)
    notes = (ROOT / f"release/notes/{version}.md").read_text(encoding="utf-8")
    assert SIGNED_START not in notes and SIGNED_END not in notes
    for update in updates:
        api(f"{endpoint}/assets/{update['id']}", {"name": update["name"]})
        print(f"Renamed installer to {update['name']}")
    if release["body"] != notes:
        api(f"{endpoint}/{release['id']}", {"body": notes})
    verify_result(release, api(f"{endpoint}/{release['id']}"), version, notes)
    print(f"Verified canonical filenames for v{version}; installer IDs, sizes and SHA-256 digests are unchanged.")


if __name__ == "__main__":
    main()
