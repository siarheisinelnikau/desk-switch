"""Canonical installer names shared by release packaging and signing."""
import re


def asset_names(version):
    if not re.fullmatch(r"(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)", version):
        raise ValueError("Expected a stable semantic release version")
    names = {
        "windows-x86_64": f"desk-switch-v{version}-windows-x86-64.exe",
        "linux-x86_64-deb": f"desk-switch-v{version}-linux-x86-64.deb",
    }
    # Only 0.5.0 published experimental macOS archives. Preserve that signed
    # historical matrix; current releases ship Windows and Ubuntu only.
    if version == "0.5.0":
        names.update({
            "macos-arm64-app": f"desk-switch-v{version}-macos-arm64.zip",
            "macos-x86_64-app": f"desk-switch-v{version}-macos-x86-64.zip",
        })
    return names
