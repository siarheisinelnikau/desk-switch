"""Canonical installer names shared by release packaging and signing."""
import re


def asset_names(version):
    if not re.fullmatch(r"(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)", version):
        raise ValueError("Expected a stable semantic release version")
    version_parts = tuple(map(int, version.split(".")))
    # Preserve the names embedded in earlier signed manifests.
    windows_suffix = "-windows-x86-64" if version_parts < (0, 8, 1) else ""
    names = {
        "windows-x86_64": f"desk-switch-v{version}{windows_suffix}.exe",
        "linux-x86_64-deb": f"desk-switch-v{version}-linux-x86-64.deb",
    }
    # Keep historical signed matrices intact. Releases from 0.10.0 ship only
    # Windows and Ubuntu until macOS can be verified on physical hardware.
    if version == "0.5.0" or (0, 6, 2) <= version_parts < (0, 10, 0):
        names.update({
            "macos-arm64-app": f"desk-switch-v{version}-macos-arm64.zip",
            "macos-x86_64-app": f"desk-switch-v{version}-macos-x86-64.zip",
        })
    return names
