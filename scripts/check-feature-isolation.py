#!/usr/bin/env python3
"""Record package-scoped Cargo graphs and reject unrelated audio dependencies.

This checks dependency resolution only. It does not claim compilation, runtime
correctness, or the clean-host acceptance demonstration from issue #39.
"""

import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess


PACKAGES = (
    "omp-core", "omp-dom", "omp-journal", "omp-session", "omp-con",
    "omp-secrets", "omp-catalog",
)
FORBIDDEN = frozenset(("audiopus_sys", "opus", "webrtc"))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=Path(__file__).resolve().parents[1])
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    if "CARGO_RESOLVER_FEATURE_UNIFICATION" in os.environ:
        parser.error("remove CARGO_RESOLVER_FEATURE_UNIFICATION; prove checked-in configuration")
    revision = subprocess.check_output(
        ["git", "rev-parse", "HEAD"], cwd=args.root, text=True,
    ).strip()
    dirty = bool(subprocess.check_output(
        ["git", "status", "--porcelain"], cwd=args.root, text=True,
    ).strip())
    config_hash = hashlib.sha256((args.root / ".cargo/config.toml").read_bytes()).hexdigest()
    checker_hash = hashlib.sha256(Path(__file__).read_bytes()).hexdigest()
    checker_revision = subprocess.check_output(
        ["git", "rev-parse", "HEAD"], cwd=Path(__file__).resolve().parent, text=True,
    ).strip()
    rows = []
    for package in (*PACKAGES, "omp-app"):
        command = ["cargo", "tree", "--locked", "-p", package,
                   "--edges", "normal,build", "--prefix", "none"]
        result = subprocess.run(command, cwd=args.root, text=True, capture_output=True)
        (args.output / f"{package}.tree.txt").write_text(result.stdout)
        (args.output / f"{package}.stderr.txt").write_text(result.stderr)
        names = {line.split()[0] for line in result.stdout.splitlines() if line.strip()}
        audio = sorted(names & FORBIDDEN)
        # The application's production voice surface must remain available.
        expected = FORBIDDEN if package == "omp-app" else frozenset()
        passed = result.returncode == 0 and package in names and set(audio) == expected
        rows.append({"package": package, "command": command, "exit": result.returncode,
                     "audio_dependencies": audio, "passed": passed})
    passed = all(row["passed"] for row in rows)
    report = {"revision": revision, "dirty": dirty, "config_sha256": config_hash,
              "checker_revision": checker_revision, "checker_sha256": checker_hash,
              "scope": "dependency resolution only",
              "passed": passed, "packages": rows}
    (args.output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    summary = [f"Revision: `{revision}` ({'modified working tree' if dirty else 'clean checkout'})",
               f"Cargo configuration SHA-256: `{config_hash}`", "",
               f"Checker revision: `{checker_revision}`; SHA-256: `{checker_hash}`", "",
               "Dependency resolution only; tests are separate.",
               "", "| Package | Audio/RTC dependencies | Result |", "| --- | --- | --- |"]
    for row in rows:
        summary.append(f"| {row['package']} | {', '.join(row['audio_dependencies']) or 'none'}"
                       f" | {'PASS' if row['passed'] else 'FAIL'} |")
    summary = "\n".join(summary) + "\n"
    (args.output / "summary.md").write_text(summary)
    print(summary)
    if output := os.environ.get("GITHUB_STEP_SUMMARY"):
        with open(output, "a") as stream:
            stream.write(summary)
    return 0 if passed else 1


if __name__ == "__main__":
    raise SystemExit(main())
