#!/usr/bin/env python3
"""Validate backticked current crate paths in ADR status/reference sections."""
import argparse
import json
import os
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


def inspect(root: Path) -> dict:
    paths = []
    failures = []
    documents = sorted((root / "docs/adr").glob("*.md"))
    for document in documents:
        current = False
        for number, line in enumerate(document.read_text().splitlines(), 1):
            if line.startswith("## "):
                current = line in ("## Status in omp", "## References")
            if not current:
                continue
            for reference in re.findall(r"`(crates/[^`\s]+)`", line):
                for path in expand_braces(reference):
                    path = re.sub(r":\d+(?:[-–]\d+)?$", "", path.split("::", 1)[0])
                    resolved = ".." not in Path(path).parts and any(root.glob(path))
                    row = {"document": document.relative_to(root).as_posix(),
                           "line": number, "path": path, "resolved": resolved}
                    paths.append(row)
                    if not resolved:
                        failures.append({**row, "reason": f"missing {path}"})
    if not paths:
        failures.append({"document": "docs/adr/README.md", "line": 1,
                         "reason": "no current ADR crate references found"})
    return {"scope": "Backticked crates/ paths in Status in omp and References sections; historical prose is excluded.",
            "documents": len(documents), "paths": paths, "failures": failures}


def escape_command(value: str) -> str:
    return value.replace("%", "%25").replace("\r", "%0D").replace("\n", "%0A")


def cell(value: str) -> str:
    return value.replace("&", "&amp;").replace("<", "&lt;").replace("|", "&#124;").replace("`", "&#96;")


def summary(report: dict) -> str:
    lines = ["## Current ADR path resolution", "", report["scope"], "",
             f"Checked {len(report['paths'])} references across {report['documents']} Markdown documents; "
             f"{len(report['failures'])} unresolved.", "",
             "| ADR | Line | Referenced path | Status |", "| --- | ---: | --- | --- |"]
    for row in report["paths"]:
        lines.append(f"| {cell(row['document'])} | {row['line']} | {cell(row['path'])} | "
                     f"{'resolved' if row['resolved'] else '**missing**'} |")
    if not report["paths"]:
        lines.extend(["", "**Failure: no current ADR crate references found.**"])
    return "\n".join(lines) + "\n"


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=ROOT)
    parser.add_argument("--report", type=Path)
    args = parser.parse_args()
    report = inspect(args.root.resolve())
    if args.report:
        args.report.parent.mkdir(parents=True, exist_ok=True)
        args.report.write_text(json.dumps(report, indent=2) + "\n")
    for failure in report["failures"]:
        filename = escape_command(failure["document"]).replace(",", "%2C").replace(":", "%3A")
        print(f"::error file={filename},line={failure['line']}::{escape_command(failure['reason'])}")
        print(f"{failure['document']}:{failure['line']}: {failure['reason']}", file=sys.stderr)
    rendered = summary(report)
    print(rendered, end="")
    if destination := os.environ.get("GITHUB_STEP_SUMMARY"):
        with Path(destination).open("a") as stream:
            stream.write(rendered)
    print(f"Checked {len(report['paths'])} current ADR crate references; {len(report['failures'])} unresolved")
    return int(bool(report["failures"]))


if __name__ == "__main__":
    raise SystemExit(main())
