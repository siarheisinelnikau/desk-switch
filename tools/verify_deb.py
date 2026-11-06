#!/usr/bin/env python3
"""Check Debian metadata and the installed launcher in an isolated profile."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import tempfile
import tomllib

ROOT = Path(__file__).resolve().parents[1]


def check(package):
    version = tomllib.loads((ROOT / "Cargo.toml").read_text())["package"]["version"]
    for field, expected in [("Package", "desk-switch"), ("Version", version + "-1"), ("Architecture", "amd64")]:
        actual = subprocess.check_output(["dpkg-deb", "--field", str(package), field], text=True).strip()
        assert actual == expected, (field, actual)
    with tempfile.TemporaryDirectory(prefix="desk-switch-deb-check-") as temporary:
        work = Path(temporary)
        extracted = work / "installed"
        subprocess.run(["dpkg-deb", "--extract", str(package), str(extracted)], check=True)
        subprocess.run(["desktop-file-validate", str(extracted / "usr/share/applications/com.deskswitch.Desktop.desktop")], check=True)
        environment = dict(os.environ, XDG_DATA_HOME=str(work / "user data"), XDG_CONFIG_HOME=str(work / "config"))
        launcher = extracted / "usr/bin/desk-switch"
        result = subprocess.check_output([str(launcher), "--build-info"], env=environment, text=True).strip()
        stamp = json.loads(result.removeprefix("DESK-SWITCH-BUILD:"))
        assert stamp["app"] == version
        copied = work / "user data/desk-switch/application/desk-switch"
        bundled = extracted / "usr/lib/desk-switch/desk-switch"
        assert copied.read_bytes() == bundled.read_bytes()
        assert os.access(copied, os.W_OK | os.X_OK)
        original = copied.stat().st_mtime_ns
        subprocess.run([str(launcher), "--version"], env=environment, check=True)
        assert copied.stat().st_mtime_ns == original, "repeated launch recopied the binary"
        copied.write_text('#!/bin/sh\n# DESK-SWITCH-BUILD:{"app":"99.0.0","protocol":2}\nprintf "newer-user-update\\n"\n')
        copied.chmod(0o700)
        assert subprocess.check_output([str(launcher), "--version"], env=environment, text=True).strip() == "newer-user-update"
        copied.write_text('#!/bin/sh\n# DESK-SWITCH-BUILD:{"app":"0.1.0","protocol":1}\nexit 9\n')
        subprocess.run([str(launcher), "--version"], env=environment, check=True)
        assert copied.read_bytes() == bundled.read_bytes()
        print("PASS: Debian metadata, desktop entry, real launcher, writable updates, no downgrade, package upgrade")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("package", type=Path)
    check(parser.parse_args().package.resolve())
