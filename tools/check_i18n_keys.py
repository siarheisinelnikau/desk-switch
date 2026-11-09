#!/usr/bin/env python3
"""Verify that every translated string literal exists in all six catalogs.

The localization catalogs in ``locales/`` map the source string exactly as it
is written in Rust to the display string of that language. A literal that is
missing from a catalog is shown verbatim, so a new user-facing string would
silently stay in one language.

The scan is intentionally conservative: when the argument of a translation
helper is not a plain string literal the call is skipped, because the value
then comes from a variable, a ``format!`` result or a state label.
"""

from __future__ import annotations

import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
LOCALES = ROOT / "locales"
SOURCE = ROOT / "src"

LANGUAGES = ("ru", "en", "de", "fr", "es", "it")

# Helpers whose first argument is the translatable source string.
FIRST_ARGUMENT_HELPERS = (
    "tr",
    "ui_text",
    "item",
    "title",
    "subheading",
    "note",
    "primary",
    "badge",
)

# Helpers whose translatable source string is the second argument.
SECOND_ARGUMENT_HELPERS = ("tr_language",)

# Literals that are deliberately not catalog entries.
ALLOWLIST = {
    # Language identifiers, not user-visible text.
    "ru",
    "en",
    "de",
    "fr",
    "es",
    "it",
    # Internal identifiers used by widget lookups.
    "managed-theme",
    "managed-language",
    # Format and protocol tokens.
    "{}",
    "{}: {}",
    "%",
}

HELPER = re.compile(
    r"\b(" + "|".join(FIRST_ARGUMENT_HELPERS + SECOND_ARGUMENT_HELPERS) + r")\s*\("
)


def read_literal(text: str, start: int) -> tuple[str, int] | None:
    """Return the literal starting at ``start`` and the index after it."""
    if text.startswith('r"', start) or text.startswith('r#', start):
        raw = text[start:].startswith('r#"')
        if raw:
            end = text.find('"#', start + 3)
            if end < 0:
                return None
            return text[start + 3 : end], end + 2
        if text.startswith('r"', start):
            end = text.find('"', start + 2)
            if end < 0:
                return None
            return text[start + 2 : end], end + 1
        return None
    if text[start] != '"':
        return None
    index = start + 1
    value: list[str] = []
    while index < len(text):
        character = text[index]
        if character == "\\":
            if index + 1 >= len(text):
                return None
            escape = text[index + 1]
            value.append(
                {
                    "n": "\n",
                    "t": "\t",
                    "r": "\r",
                    "0": "\0",
                    "\\": "\\",
                    '"': '"',
                    "'": "'",
                }.get(escape, escape)
            )
            index += 2
            continue
        if character == '"':
            return "".join(value), index + 1
        value.append(character)
        index += 1
    return None


def arguments(text: str, open_paren: int) -> list[str | None]:
    """Split the call arguments, returning the literal text of each one."""
    depth = 1
    index = open_paren + 1
    current: list[str] = []
    parts: list[str | None] = []
    while index < len(text):
        character = text[index]
        if character in "([{":
            depth += 1
        elif character in ")]}":
            depth -= 1
            if depth == 0:
                parts.append("".join(current))
                return parts
        if depth == 1 and character == ",":
            parts.append("".join(current))
            current = []
            index += 1
            continue
        current.append(character)
        index += 1
    return parts


def literal_argument(raw: str | None) -> str | None:
    if raw is None:
        return None
    stripped = raw.strip()
    if not stripped.startswith('"'):
        return None
    literal = read_literal(stripped, 0)
    if literal is None:
        return None
    value, end = literal
    if stripped[end:].strip():
        return None
    return value


def scan_file(path: Path) -> list[tuple[int, str]]:
    text = path.read_text(encoding="utf-8")
    found: list[tuple[int, str]] = []
    for match in HELPER.finditer(text):
        helper = match.group(1)
        parts = arguments(text, match.end() - 1)
        raw = None
        if helper in SECOND_ARGUMENT_HELPERS:
            if len(parts) < 2:
                continue
            raw = parts[1]
        else:
            if not parts:
                continue
            raw = parts[0]
        value = literal_argument(raw)
        if value is None or value in ALLOWLIST or not value.strip():
            continue
        line = text.count("\n", 0, match.start()) + 1
        found.append((line, value))
    return found


def main() -> int:
    catalogs: dict[str, dict[str, str]] = {}
    for language in LANGUAGES:
        path = LOCALES / f"{language}.json"
        try:
            catalogs[language] = json.loads(path.read_text(encoding="utf-8"))
        except FileNotFoundError:
            print(f"missing catalog: {path}", file=sys.stderr)
            return 1
        except json.JSONDecodeError as error:
            print(f"invalid catalog {path}: {error}", file=sys.stderr)
            return 1

    missing: dict[tuple[str, str], list[str]] = {}
    checked = 0
    for path in sorted(SOURCE.rglob("*.rs")):
        relative = path.relative_to(ROOT).as_posix()
        if relative.startswith("vendor/"):
            continue
        for line, value in scan_file(path):
            checked += 1
            for language in LANGUAGES:
                if value not in catalogs[language]:
                    missing.setdefault((relative, value), []).append(language)

    if missing:
        print(f"{len(missing)} translated literal(s) are missing from a catalog:")
        for (relative, value), languages in sorted(missing.items()):
            print(f"  {relative}: {value!r} missing in {', '.join(languages)}")
        return 1
    print(
        f"Checked {checked} translated literal(s); every one exists in all "
        f"{len(LANGUAGES)} catalogs."
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
