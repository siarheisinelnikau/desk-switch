#!/usr/bin/env python3
"""Generate or check the complete third-party package notice inventory."""

from __future__ import annotations

import argparse
from collections import Counter
import json
from pathlib import Path
import re
import subprocess
import sys
import tomllib
from typing import Any, Iterable, Sequence

ROOT = Path(__file__).resolve().parents[1]
LOCK_PATH = ROOT / "Cargo.lock"
NOTICES_PATH = ROOT / "THIRD-PARTY-NOTICES.md"
ROOT_PACKAGE = "desk-switch"
PACKAGE_MARKER = re.compile(r"^<!-- cargo-package: ([^ ]+) ([^ ]+) -->$", re.MULTILINE)

# These files contain package-specific copyright notices or attribution and are
# therefore reproduced in addition to the common MIT and Apache-2.0 terms.
VENDORED_TEXTS = {
    "assets/third-party/file-transfer/cap-std-LICENSE-MIT": "cap-std and cap-fs-ext (MIT option)",
    "assets/third-party/file-transfer/fuser-LICENSE-MIT": "fuser (MIT option)",
    "assets/third-party/file-transfer/nix-LICENSE-MIT": "nix (MIT option)",
    "assets/third-party/file-transfer/page-size-LICENSE-MIT": "page_size (MIT option)",
    "assets/third-party/file-transfer/ref-cast-LICENSE-MIT": "ref-cast and ref-cast-impl (MIT option)",
    "assets/third-party/file-transfer/rfd-LICENSE": "rfd",
    "assets/third-party/file-transfer/rfd-window-identifier-LICENSE": "rfd XDG portal window identifier support",
    "assets/third-party/vb-cable/NOTICE.txt": "VB-CABLE Virtual Audio Device",
    "vendor/egui_software_backend/LICENSE-MIT": "egui_software_backend (MIT option)",
    "vendor/m1ddc/LICENSE": "m1ddc",
    "vendor/rust-auto-updator/LICENSE": "rust-auto-updator",
}

PACKAGE_TEXTS = {
    "cap-std": ("assets/third-party/file-transfer/cap-std-LICENSE-MIT",),
    "cap-fs-ext": ("assets/third-party/file-transfer/cap-std-LICENSE-MIT",),
    "egui_software_backend": (
        "vendor/egui_software_backend/LICENSE-MIT",
        "vendor/egui_software_backend/LICENSE-APACHE",
    ),
    "fuser": ("assets/third-party/file-transfer/fuser-LICENSE-MIT",),
    "nix": ("assets/third-party/file-transfer/nix-LICENSE-MIT",),
    "page_size": ("assets/third-party/file-transfer/page-size-LICENSE-MIT",),
    "ref-cast": ("assets/third-party/file-transfer/ref-cast-LICENSE-MIT",),
    "ref-cast-impl": ("assets/third-party/file-transfer/ref-cast-LICENSE-MIT",),
    "rfd": (
        "assets/third-party/file-transfer/rfd-LICENSE",
        "assets/third-party/file-transfer/rfd-window-identifier-LICENSE",
    ),
    "rust-auto-updator": ("vendor/rust-auto-updator/LICENSE",),
}

MIT_TERMS = """MIT License

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
"""


class NoticesOutdatedError(Exception):
    """The committed notice inventory differs from Cargo.lock."""


def identity(package: dict[str, Any]) -> tuple[str, str]:
    return str(package["name"]), str(package["version"])


def package_sort_key(package: dict[str, Any]) -> tuple[str, str, str]:
    name, version = identity(package)
    return name.lower(), version, name


def identity_sort_key(item: tuple[str, str]) -> tuple[str, str, str]:
    name, version = item
    return name.lower(), version, name


def locked_packages(lock_path: Path = LOCK_PATH) -> set[tuple[str, str]]:
    """Read package identities without Cargo, network access, or registry state."""
    with lock_path.open("rb") as stream:
        lock = tomllib.load(stream)
    return {
        (package["name"], package["version"])
        for package in lock.get("package", [])
        if package["name"] != ROOT_PACKAGE
    }


def documented_packages(notices_text: str) -> list[tuple[str, str]]:
    return [(match.group(1), match.group(2)) for match in PACKAGE_MARKER.finditer(notices_text)]


def check_notices(
    lock_path: Path = LOCK_PATH, notices_path: Path = NOTICES_PATH
) -> int:
    """Raise a descriptive error for missing, stale, or duplicate entries."""
    expected = locked_packages(lock_path)
    documented = documented_packages(notices_path.read_text(encoding="utf-8"))
    actual = set(documented)
    missing = sorted(expected - actual, key=identity_sort_key)
    stale = sorted(actual - expected, key=identity_sort_key)
    duplicates = sorted(
        (item for item, count in Counter(documented).items() if count > 1),
        key=identity_sort_key,
    )
    if missing or stale or duplicates:
        lines = [f"{notices_path.name} is out of date with {lock_path.name}."]
        for label, items in (
            ("Missing entries", missing),
            ("Stale entries", stale),
            ("Duplicate entries", duplicates),
        ):
            if items:
                lines.append(f"{label} ({len(items)}):")
                lines.extend(f"  - {name} {version}" for name, version in items)
        lines.append("Regenerate with: python tools/gen_third_party_notices.py")
        raise NoticesOutdatedError("\n".join(lines))
    return len(expected)


def cargo_metadata() -> dict[str, Any]:
    command = ["cargo", "metadata", "--format-version", "1", "--locked"]
    try:
        result = subprocess.run(
            command,
            cwd=ROOT,
            check=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            encoding="utf-8",
        )
    except FileNotFoundError as error:
        raise RuntimeError("cargo is required to regenerate third-party notices") from error
    except subprocess.CalledProcessError as error:
        detail = error.stderr.strip() or error.stdout.strip()
        raise RuntimeError(f"cargo metadata failed:\n{detail}") from error
    return json.loads(result.stdout)


def clean_inline(value: Any) -> str:
    return " ".join(str(value).split())


def fenced(text: str, language: str = "text") -> str:
    """Use a fence longer than any backtick run in the upstream text."""
    longest = max((len(run) for run in re.findall(r"`+", text)), default=0)
    fence = "`" * max(3, longest + 1)
    normalized = text.replace("\r\n", "\n").replace("\r", "\n")
    return f"{fence}{language}\n{normalized.rstrip()}\n{fence}"


def relative_manifest_path(package: dict[str, Any], root: Path) -> str | None:
    manifest = package.get("manifest_path")
    if not manifest:
        return None
    try:
        return (Path(manifest).resolve().parent.relative_to(root.resolve())).as_posix()
    except ValueError:
        return None


def upstream_notices(package: dict[str, Any]) -> list[tuple[str, str]]:
    """Return root-level NOTICE/NOTICES files shipped by the resolved crate."""
    manifest = package.get("manifest_path")
    if not manifest:
        return []
    package_root = Path(manifest).parent
    if not package_root.is_dir():
        return []
    files = sorted(
        (
            path
            for path in package_root.iterdir()
            if path.is_file() and path.name.upper().startswith("NOTICE")
        ),
        key=lambda path: path.name.lower(),
    )
    return [(path.name, path.read_text(encoding="utf-8")) for path in files]


def license_references(license_expression: str) -> list[str]:
    references = []
    if re.search(r"(?<![\w.-])MIT(?![\w.-])", license_expression):
        references.append("[MIT license terms](#appendix-a-mit-license)")
    if re.search(r"(?<![\w.-])Apache-2\.0(?![\w.-])", license_expression):
        references.append("[Apache-2.0 license terms](#appendix-b-apache-20-license)")
    return references


def render_package(package: dict[str, Any], root: Path) -> str:
    name, version = identity(package)
    lines = [
        f"### {clean_inline(name)} {clean_inline(version)}",
        "",
        f"<!-- cargo-package: {name} {version} -->",
        "",
    ]
    license_expression = clean_inline(package.get("license") or "Not specified in Cargo metadata")
    license_line = f"- **License (Cargo metadata):** `{license_expression}`"
    references = license_references(license_expression)
    if references:
        license_line += "; see " + " and ".join(references)
    lines.extend([license_line])

    for field, label in (("repository", "Repository"), ("homepage", "Homepage")):
        value = package.get(field)
        lines.append(
            f"- **{label}:** {clean_inline(value) if value else 'Not specified in Cargo metadata'}"
        )
    authors = [clean_inline(author) for author in package.get("authors") or [] if clean_inline(author)]
    if authors:
        lines.append("- **Authors:** " + "; ".join(authors))

    vendored_path = relative_manifest_path(package, root)
    if package.get("source") is None and vendored_path:
        lines.append(f"- **Source:** Vendored path dependency at `{vendored_path}`")

    texts = PACKAGE_TEXTS.get(name, ())
    if texts:
        lines.append("- **Vendored license texts:**")
        for path in texts:
            if path.endswith("LICENSE-APACHE"):
                lines.append(
                    f"  - `{path}`: reproduced as [Appendix B](#appendix-b-apache-20-license)"
                )
            else:
                lines.append(f"  - `{path}`: reproduced in [Appendix C](#appendix-c-vendored-license-and-notice-texts)")

    notices = upstream_notices(package)
    if notices:
        lines.append("- **Upstream NOTICE files:**")
        for filename, _ in notices:
            lines.append(f"  - `{filename}` (reproduced below)")
    lines.append("")
    for filename, text in notices:
        lines.extend([f"#### Upstream `{filename}`", "", fenced(text), ""])
    return "\n".join(lines).rstrip() + "\n"


def render_notices(packages: Iterable[dict[str, Any]], root: Path = ROOT) -> str:
    """Render a deterministic document from cargo metadata package records."""
    ordered = sorted(
        (package for package in packages if package.get("name") != ROOT_PACKAGE),
        key=package_sort_key,
    )
    project_license = (root / "LICENSE").read_text(encoding="utf-8")
    apache_license = (root / "vendor/egui_software_backend/LICENSE-APACHE").read_text(encoding="utf-8")
    sections = [
        "# Third-Party Notices",
        "",
        "Desk Switch includes or links to the third-party packages and components listed below. "
        "This file is generated from `cargo metadata --format-version 1 --locked`; the offline "
        "freshness check compares its package markers with `Cargo.lock`. SPDX license expressions, "
        "repository/homepage URLs, and authors are reproduced from Cargo metadata. An SPDX "
        "expression that offers alternatives describes the upstream licensing options; it does not "
        "assert that every option is used. Package-specific copyright and NOTICE texts remain part "
        "of their respective notices.",
        "",
        "To regenerate from the repository root, run:",
        "",
        "```console",
        "python tools/gen_third_party_notices.py",
        "```",
        "",
        "To check the committed inventory without Cargo or network access, run "
        "`python tools/gen_third_party_notices.py --check`.",
        "",
        f"The resolved graph contains **{len(ordered)} third-party Cargo packages**. "
        "The root package `desk-switch` is excluded from that count. Additional non-Cargo bundled "
        "components are listed separately.",
        "",
        "## Desk Switch License",
        "",
        fenced(project_license),
        "",
        "## Additional Bundled Components",
        "",
        "### m1ddc",
        "",
        "- **Version:** Not versioned in the vendored source snapshot",
        "- **License:** MIT; the exact vendored text is reproduced in Appendix C",
        "- **Repository:** https://github.com/waydabber/m1ddc",
        "- **Source:** `vendor/m1ddc`; used by the macOS Apple Silicon DDC transport",
        "",
        "### VB-CABLE Virtual Audio Device",
        "",
        "The original VB-Audio installer archives are separate donationware, not part of Desk "
        "Switch's MIT-licensed code. The vendored redistribution notice and archive hashes are "
        "reproduced verbatim in Appendix C.",
        "",
        "- **Website:** https://www.vb-cable.com/",
        "- **Source:** `assets/third-party/vb-cable`",
        "",
        "## Resolved Cargo Packages",
        "",
    ]
    sections.extend(render_package(package, root) for package in ordered)
    sections.extend(
        [
            "",
            "## Appendix A: MIT License",
            "",
            "The common MIT terms are reproduced here once. Package-specific vendored copies in "
            "Appendix C preserve their original copyright lines and formatting.",
            "",
            fenced(MIT_TERMS),
            "",
            "## Appendix B: Apache-2.0 License",
            "",
            "This is the exact `vendor/egui_software_backend/LICENSE-APACHE` text and the standard "
            "Apache License, Version 2.0 terms referenced by packages whose metadata includes "
            "Apache-2.0.",
            "",
            fenced(apache_license),
            "",
            "## Appendix C: Vendored License and NOTICE Texts",
            "",
            "The following texts are copied verbatim from the repository. Each file appears once; "
            "packages and bundled components above refer to these copies where applicable.",
            "",
        ]
    )
    for path, description in sorted(VENDORED_TEXTS.items()):
        text = (root / path).read_text(encoding="utf-8")
        sections.extend([f"### `{path}`", "", f"**Applies to:** {description}", "", fenced(text), ""])
    return "\n".join(sections).rstrip() + "\n"


def generate() -> int:
    metadata = cargo_metadata()
    packages = metadata.get("packages")
    if not isinstance(packages, list):
        raise RuntimeError("cargo metadata did not return a packages array")
    document = render_notices(packages)
    NOTICES_PATH.write_text(document, encoding="utf-8", newline="\n")
    # Validate both inventories immediately, so a partially stale lockfile or a
    # renderer mistake cannot produce a document that fails the offline check.
    count = check_notices()
    print(f"Wrote {NOTICES_PATH.name}: {count} third-party Cargo packages plus bundled components.")
    return 0


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--check",
        action="store_true",
        help="check Cargo.lock coverage using only Python (no Cargo or network)",
    )
    args = parser.parse_args(argv)
    try:
        if args.check:
            count = check_notices()
            print(
                f"{NOTICES_PATH.name} covers all {count} third-party Cargo.lock packages; "
                "no missing, stale, or duplicate entries."
            )
            return 0
        return generate()
    except (NoticesOutdatedError, OSError, RuntimeError, ValueError, tomllib.TOMLDecodeError) as error:
        print(f"error: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
