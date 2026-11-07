"""Canonical installer names shared by release packaging and signing."""
import re


def asset_names(version):
    if not re.fullmatch(r"(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)", version):
        raise ValueError("Expected a stable semantic release version")
    return {
        "windows-x86_64": f"desk-switch-v{version}-windows-x86-64.exe",
        "linux-x86_64-deb": f"desk-switch-v{version}-linux-x86-64.deb",
    }
