#!/usr/bin/env python3
"""Validate backticked current crate paths in ADR status/reference sections."""
from pathlib import Path
import re
import sys

ROOT = Path(__file__).resolve().parents[1]


def expand_braces(value: str) -> list[str]:
    match = re.search(r"\{([^{}]+)\}", value)
    if not match:
        return [value]
    return [expanded for part in match[1].split(",")
            for expanded in expand_braces(value[:match.start()] + part + value[match.end():])]


def main() -> int:
    failures = []
    checked = 0
    for document in sorted((ROOT / "docs/adr").glob("*.md")):
        current = False
        for number, line in enumerate(document.read_text().splitlines(), 1):
            if line.startswith("## "):
                current = line in ("## Status in omp", "## References")
            if not current:
                continue
            for reference in re.findall(r"`(crates/[^`\s]+)`", line):
                for path in expand_braces(reference):
                    path = re.sub(r":\d+(?:[-–]\d+)?$", "", path.split("::", 1)[0])
                    checked += 1
                    if not any(ROOT.glob(path)):
                        failures.append(f"{document.relative_to(ROOT)}:{number}: missing {path}")
    if not checked:
        failures.append("no current ADR crate references found")
    for failure in failures:
        print(failure, file=sys.stderr)
    print(f"Checked {checked} current ADR crate references; {len(failures)} unresolved")
    return int(bool(failures))


if __name__ == "__main__":
    raise SystemExit(main())
