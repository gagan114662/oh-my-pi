#!/usr/bin/env python3
"""Require an explicit, nonempty Gap in every Partial ADR status section."""
import argparse
import json
import os
from pathlib import Path
import re

ROOT = Path(__file__).resolve().parents[1]


def inspect(root: Path) -> tuple[list[dict], list[dict]]:
    records, failures = [], []
    for path in sorted((root / "docs/adr").glob("[0-9]*.md")):
        text = path.read_text()
        headings = list(re.finditer(r"^## Status in omp\s*$", text, re.MULTILINE))
        relative = path.relative_to(root).as_posix()
        if len(headings) != 1:
            failures.append(dict(path=relative, line=1, reason="expected exactly one Status in omp section"))
            continue
        start = headings[0].end()
        line = text.count("\n", 0, headings[0].start()) + 1
        section = re.split(r"^## ", text[start:], maxsplit=1, flags=re.MULTILINE)[0]
        # Status is the first word, allowing ordinary Markdown emphasis.
        match = re.match(r"\s*(?:\*\*)?([A-Za-z]+)\b", section)
        status = match[1] if match else ""
        gap = re.search(r"\bGap:\s*(\S[^\n]*)", section)
        record = dict(path=relative, line=line, status=status, gap=gap[1].strip() if gap else None)
        records.append(record)
        if status not in ("Implemented", "Partial"):
            failures.append(dict(path=relative, line=line, reason="status must begin Implemented or Partial"))
        elif status == "Partial" and not gap:
            failures.append(dict(path=relative, line=line, reason="Partial status must name a nonempty Gap:"))
    if not records:
        failures.append(dict(path="docs/adr", line=1, reason="no numbered ADR status records found"))
    return records, failures


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=ROOT)
    parser.add_argument("--report", type=Path)
    args = parser.parse_args()
    records, failures = inspect(args.root)
    for failure in failures:
        print(f"{failure['path']}:{failure['line']}: {failure['reason']}")
        if os.environ.get("GITHUB_ACTIONS") == "true":
            path = failure['path'].replace('%', '%25').replace('\n', '%0A').replace('\r', '%0D').replace(',', '%2C')
            print(f"::error file={path},line={failure['line']}::{failure['reason']}")
    print(f"Checked {len(records)} ADR status records; {len(failures)} violations")
    if args.report:
        args.report.parent.mkdir(parents=True, exist_ok=True)
        args.report.write_text(json.dumps(dict(records=records, failures=failures), indent=2) + "\n")
    return int(bool(failures))


if __name__ == "__main__":
    raise SystemExit(main())
