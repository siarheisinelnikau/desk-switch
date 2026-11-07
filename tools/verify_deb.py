#!/usr/bin/env python3
"""Check Debian metadata and the installed launcher in an isolated profile."""
import argparse
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile
import tomllib

ROOT = Path(__file__).resolve().parents[1]


def check(package):
    version = tomllib.loads((ROOT / "Cargo.toml").read_text())["package"]["version"]
    for field, expected in [("Package", "desk-switch"), ("Version", version), ("Architecture", "amd64")]:
        actual = subprocess.check_output(["dpkg-deb", "--field", str(package), field], text=True).strip()
        assert actual == expected, (field, actual)
    dependencies = subprocess.check_output(["dpkg-deb", "--field", str(package), "Depends"], text=True)
    for required in ["ddcutil", "pipewire-bin", "pipewire-pulse", "pulseaudio-utils", "acl", "udev", "python3"]:
        assert any(re.fullmatch(r"\s*" + re.escape(required) + r"(?:\s*\([^)]*\))?\s*", dependency)
                   for dependency in dependencies.split(",")), f"{required} must be a required runtime dependency"
    with tempfile.TemporaryDirectory(prefix="desk-switch-deb-check-") as temporary:
        work = Path(temporary)
        extracted = work / "installed"
        subprocess.run(["dpkg-deb", "--extract", str(package), str(extracted)], check=True)
        subprocess.run(["desktop-file-validate", str(extracted / "usr/share/applications/com.deskswitch.Desktop.desktop")], check=True)
        environment = dict(os.environ, XDG_DATA_HOME=str(work / "user data"), XDG_CONFIG_HOME=str(work / "config"))
        helper = extracted / "usr/lib/desk-switch/input-access"
        assert helper.is_file() and os.access(helper, os.X_OK)
        rule = extracted / "usr/lib/udev/rules.d/99-desk-switch-input-access.rules"
        assert rule.is_file() and 'input-access --restore %k' in rule.read_text()
        control = work / "control"
        subprocess.run(["dpkg-deb", "--control", str(package), str(control)], check=True)
        postinst = control / "postinst"
        assert os.access(postinst, os.X_OK) and "--reload-rules" in postinst.read_text()
        assert "trigger" not in postinst.read_text(), "installation must not trigger real input devices"
        subprocess.run(["sh", "-n", str(postinst)], check=True)
        assert (extracted / "usr/share/polkit-1/actions/com.deskswitch.input-access.policy").is_file()
        launcher = extracted / "usr/bin/desk-switch"
        result = subprocess.check_output([str(launcher), "--build-info"], env=environment, text=True).strip()
        stamp = json.loads(result.removeprefix("DESK-SWITCH-BUILD:"))
        assert stamp["app"] == version
        bundled = extracted / "usr/lib/desk-switch/desk-switch"
        original = bundled.read_bytes()
        subprocess.run([str(launcher), "--version"], env=environment, check=True)
        assert bundled.read_bytes() == original, "launcher must not mutate the package-owned executable"
        assert not (work / "user data/desk-switch/application/desk-switch").exists()
        assert (extracted / "usr/share/doc/desk-switch/rust-auto-updator-LICENSE").is_file()
        print("PASS: Debian metadata, required monitor/audio utilities, menu icon, launcher, package-owned binary and updater license")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("package", type=Path)
    check(parser.parse_args().package.resolve())
