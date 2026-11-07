"""Sign tested CI installers with the required pinned project identity."""
import base64
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import tomllib

from release_assets import asset_names

ROOT = Path(__file__).resolve().parents[1]


def output(ready):
    with open(os.environ["GITHUB_OUTPUT"], "a", encoding="utf-8") as stream:
        print(f"ready={str(ready).lower()}", file=stream)


def main():
    key = os.environ.pop("DESK_SWITCH_UPDATE_SIGNING_KEY", "").strip()
    if not key:
        raise SystemExit("Configure DESK_SWITCH_UPDATE_SIGNING_KEY in the release environment before publishing.")
    data = key.encode() if key.startswith("-----BEGIN") else base64.b64decode(key, validate=True)
    version = tomllib.loads((ROOT / "Cargo.toml").read_text())["package"]["version"]
    with tempfile.TemporaryDirectory(prefix="desk-switch-ci-sign-", dir=os.environ.get("RUNNER_TEMP")) as directory:
        path = Path(directory) / "signing-key.pem"
        path.write_bytes(data)
        path.chmod(0o600)
        subprocess.run([
            sys.executable, str(ROOT / "tools/prepare_release.py"),
            "--windows", str(ROOT / "artifacts/desk-switch.exe"),
            "--deb", str(ROOT / "artifacts" / asset_names(version)["linux-x86_64-deb"]),
            "--tag", f"v{version}", "--key", str(path),
            "--out", str(ROOT / "dist/signed"),
        ], check=True)
    manifest = ROOT / f"release/manifests/{version}.json"
    manifest.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(ROOT / "dist/signed/update.json", manifest)
    output(True)
    print(f"Prepared pinned-key update metadata for v{version}.")


if __name__ == "__main__":
    main()
