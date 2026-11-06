#!/usr/bin/env python3
"""Build an amd64 Debian package with desktop integration and package-managed signed updates."""
import argparse
import hashlib
import math
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import tomllib

from prepare_release import checked_binary

ROOT = Path(__file__).resolve().parents[1]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--out", type=Path, default=ROOT / "dist")
    args = parser.parse_args()
    version = tomllib.loads((ROOT / "Cargo.toml").read_text())["package"]["version"]
    import re
    protocol = int(re.search(r"pub const PROTOCOL: u16 = (\d+);", (ROOT / "src/compatibility.rs").read_text())[1])
    binary = args.binary.resolve()
    checked_binary(binary, "linux-x86_64", version, protocol)
    epoch = int(os.environ.get("SOURCE_DATE_EPOCH") or subprocess.check_output(
        ["git", "log", "-1", "--format=%ct"], cwd=ROOT))
    with tempfile.TemporaryDirectory(prefix="desk-switch-deb-") as temporary:
        work = Path(temporary)
        stage = work / "package"
        files = {
            "usr/lib/desk-switch/desk-switch": binary,
            "usr/bin/desk-switch": ROOT / "packaging/linux/desk-switch",
            "usr/share/applications/com.deskswitch.Desktop.desktop": ROOT / "packaging/linux/com.deskswitch.Desktop.desktop",
            "usr/share/icons/hicolor/scalable/apps/com.deskswitch.Desktop.svg": ROOT / "packaging/linux/com.deskswitch.Desktop.svg",
            "usr/share/doc/desk-switch/copyright": ROOT / "LICENSE",
            "usr/share/doc/desk-switch/rust-auto-updator-LICENSE": ROOT / "vendor/rust-auto-updator/LICENSE",
        }
        for name, source in files.items():
            target = stage / name
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(source, target)
            target.chmod(0o755 if name in {"usr/bin/desk-switch", "usr/lib/desk-switch/desk-switch"} else 0o644)
        subprocess.run(["desktop-file-validate", str(stage / "usr/share/applications/com.deskswitch.Desktop.desktop")], check=True)
        (work / "debian").mkdir()
        (work / "debian/control").write_text("Source: desk-switch\n\nPackage: desk-switch\nArchitecture: amd64\nDescription: Share input and audio between PCs\n")
        shlibs = subprocess.check_output(["dpkg-shlibdeps", "-O", "-e", str(binary)], cwd=work, text=True)
        native = next(line.split("=", 1)[1] for line in shlibs.splitlines() if line.startswith("shlibs:Depends="))
        dependencies = native + ", pkexec, ca-certificates, ddcutil, pipewire-bin, pipewire-pulse, pulseaudio-utils, libx11-6, libx11-xcb1, libxcursor1, libxrandr2, libxi6, libxinerama1, libxkbcommon0, libxkbcommon-x11-0, libwayland-client0, libegl1, libgl1"
        control = stage / "DEBIAN"
        control.mkdir()
        size = math.ceil(sum(p.stat().st_size for p in stage.rglob("*") if p.is_file()) / 1024)
        (control / "control").write_text(
            f"Package: desk-switch\nVersion: {version}\nArchitecture: amd64\n"
            "Maintainer: Desk Switch contributors <9218241+siarheisinelnikau@users.noreply.github.com>\n"
            "Section: utils\nPriority: optional\n"
            f"Installed-Size: {size}\nDepends: {dependencies}\n"
            "Recommends: wireplumber, xdg-desktop-portal\n"
            "Homepage: https://github.com/siarheisinelnikau/desk-switch\n"
            "Description: Share keyboard, mouse and system audio between PCs\n"
            " Native desktop window and tray for Windows and Ubuntu pairs.\n"
            " Includes authenticated pairing, signed updates and optional monitor switching.\n")
        (control / "md5sums").write_text("".join(
            f"{hashlib.md5((stage / name).read_bytes()).hexdigest()}  {name}\n" for name in sorted(files)))
        for path in stage.rglob("*"):
            os.utime(path, (epoch, epoch))
        os.utime(stage, (epoch, epoch))
        args.out.mkdir(parents=True, exist_ok=True)
        output = args.out.resolve() / "desk-switch-linux-x86_64.deb"
        subprocess.run(["dpkg-deb", "--root-owner-group", "-Zxz", "--build", str(stage), str(output)],
                       check=True, env=dict(os.environ, SOURCE_DATE_EPOCH=str(epoch)))
        subprocess.run(["dpkg-deb", "--info", str(output)], check=True)
        print(output)


if __name__ == "__main__":
    main()
