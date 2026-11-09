#!/usr/bin/env python3
"""Report unsafe blocks, functions, and implementations without SAFETY comments."""

from __future__ import annotations

import re
from dataclasses import dataclass
from pathlib import Path


UNSAFE_RE = re.compile(r"(?<![\w#])unsafe\b")
SAFETY_RE = re.compile(r"^\s*//\s*SAFETY:")
RAW_STRING_RE = re.compile(r"(?:br|cr|r)(?P<hashes>#{0,255})\"")


@dataclass(frozen=True)
class FileAudit:
    path: Path
    documented: int
    undocumented: int

    @property
    def total(self) -> int:
        return self.documented + self.undocumented


def mask_rust(source: str) -> str:
    """Replace comments and literals with spaces, preserving offsets and lines."""
    masked = list(source)
    length = len(source)
    index = 0
    block_depth = 0

    def blank(start: int, end: int) -> None:
        for position in range(start, end):
            if masked[position] != "\n":
                masked[position] = " "

    while index < length:
        if block_depth:
            if source.startswith("/*", index):
                block_depth += 1
                blank(index, index + 2)
                index += 2
            elif source.startswith("*/", index):
                block_depth -= 1
                blank(index, index + 2)
                index += 2
            else:
                blank(index, index + 1)
                index += 1
            continue

        if source.startswith("//", index):
            end = source.find("\n", index)
            end = length if end == -1 else end
            blank(index, end)
            index = end
            continue
        if source.startswith("/*", index):
            block_depth = 1
            blank(index, index + 2)
            index += 2
            continue

        raw_match = RAW_STRING_RE.match(source, index)
        if raw_match:
            terminator = '"' + raw_match.group("hashes")
            end = source.find(terminator, raw_match.end())
            end = length if end == -1 else end + len(terminator)
            blank(index, end)
            index = end
            continue

        if source[index] == '"':
            end = index + 1
            while end < length:
                if source[end] == "\\":
                    end += 2
                elif source[end] == '"':
                    end += 1
                    break
                else:
                    end += 1
            end = min(end, length)
            blank(index, end)
            index = end
            continue

        if source[index] == "'":
            # Lifetimes have no closing quote. Character literals either contain
            # one Unicode scalar or an escape sequence.
            end = -1
            if index + 2 < length and source[index + 1] == "\\":
                cursor = index + 2
                while cursor < length and source[cursor] != "\n":
                    if source[cursor] == "\\":
                        cursor += 2
                    elif source[cursor] == "'":
                        end = cursor + 1
                        break
                    else:
                        cursor += 1
            elif index + 2 < length and source[index + 2] == "'":
                end = index + 3
            if end != -1:
                blank(index, end)
                index = end
                continue

        index += 1

    return "".join(masked)


def has_safety_comment(lines: list[str], line_index: int) -> bool:
    """Check the contiguous comment/attribute region above an unsafe item."""
    for previous in range(line_index - 1, -1, -1):
        stripped = lines[previous].strip()
        if SAFETY_RE.match(stripped):
            return True
        if not stripped or stripped.startswith("//"):
            continue
        if stripped.startswith("#[") or stripped.startswith("#!["):
            continue
        return False
    return False


def audit_file(path: Path) -> FileAudit:
    source = path.read_text(encoding="utf-8")
    code = mask_rust(source)
    lines = source.splitlines()
    documented = 0
    undocumented = 0

    for match in UNSAFE_RE.finditer(code):
        remainder = code[match.end() :]
        if not re.match(r"\s*(?:\{|(?:\bextern\b\s*)?\bfn\b|\bimpl\b)", remainder):
            continue
        line_index = code.count("\n", 0, match.start())
        if has_safety_comment(lines, line_index):
            documented += 1
        else:
            undocumented += 1

    return FileAudit(path, documented, undocumented)


def main() -> int:
    repository = Path(__file__).resolve().parent.parent
    source_root = repository / "src"
    paths = sorted(
        path
        for path in source_root.rglob("*.rs")
        if not {"vendor", "target"}.intersection(path.relative_to(repository).parts)
    )

    audits = [audit_file(path) for path in paths]
    audits = [audit for audit in audits if audit.total]
    audits.sort(key=lambda audit: (-audit.undocumented, str(audit.path)))

    rows = [
        (
            audit.path.relative_to(repository).as_posix(),
            str(audit.documented),
            str(audit.undocumented),
            str(audit.total),
        )
        for audit in audits
    ]
    totals = (
        "TOTAL",
        str(sum(audit.documented for audit in audits)),
        str(sum(audit.undocumented for audit in audits)),
        str(sum(audit.total for audit in audits)),
    )
    rows.append(totals)
    headers = ("File", "Documented", "Undocumented", "Total")
    widths = [
        max(len(headers[column]), *(len(row[column]) for row in rows))
        for column in range(4)
    ]

    def print_row(row: tuple[str, str, str, str]) -> None:
        print(
            f"{row[0]:<{widths[0]}}  {row[1]:>{widths[1]}}  "
            f"{row[2]:>{widths[2]}}  {row[3]:>{widths[3]}}"
        )

    print_row(headers)
    print_row(tuple("-" * width for width in widths))
    for row in rows[:-1]:
        print_row(row)
    print_row(tuple("-" * width for width in widths))
    print_row(totals)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
