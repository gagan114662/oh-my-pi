#!/usr/bin/env python3
"""Run P6 under bounded disk pressure and publish honest timing margins."""

import argparse
import json
import math
import os
from pathlib import Path
import subprocess
import tempfile
import threading
import time


def contended(command, output):
    output.mkdir(parents=True, exist_ok=True)
    stop = threading.Event()
    ready = threading.Event()
    state = {"bytes_written": 0, "syncs": 0, "error": None}

    def load():
        try:
            # Fixed 8 MiB footprint on the same volume as the proof artifacts.
            # Repeated fsync prevents this from being only a memory-copy load.
            with tempfile.TemporaryFile(dir=output) as disk:
                block = bytes(range(256)) * 4096
                while not stop.is_set():
                    disk.seek(0)
                    for _ in range(8):
                        disk.write(block)
                        state["bytes_written"] += len(block)
                    disk.flush()
                    os.fsync(disk.fileno())
                    state["syncs"] += 1
                    ready.set()
        except Exception as error:
            state["error"] = str(error)
            ready.set()

    worker = threading.Thread(target=load, daemon=True)
    started = time.monotonic()
    worker.start()
    code = 1
    try:
        if not ready.wait(30) or state["error"]:
            raise RuntimeError(f"disk pressure failed to start: {state}")
        state["syncs_before_test"] = state["syncs"]
        env = dict(os.environ, OMP_P6_LATENCY_PATH=str(output / "contended.json"))
        code = subprocess.run(command, env=env, check=False).returncode
    finally:
        stop.set()
        worker.join(timeout=30)
        if worker.is_alive():
            state["error"] = "disk pressure worker did not stop within 30 seconds"
        state["elapsed_ms"] = (time.monotonic() - started) * 1000
        state["test_exit_code"] = code
        (output / "load.json").write_text(json.dumps(state, indent=2) + "\n")
    return code if code else int(bool(state["error"]))


def validate(rows, load):
    problems = []
    if load.get("error") or load.get("test_exit_code") != 0:
        problems.append("contended run or disk pressure failed")
    if load.get("syncs", 0) <= load.get("syncs_before_test", 0):
        problems.append("no disk sync completed during the contended test")
    for mode in ("normal", "contended"):
        row = rows.get(mode, {})
        if row.get("completed") is not True:
            problems.append(f"{mode}: full crash/resume proof did not complete")
        for phase, bound in (("journal", 3000), ("resume", 30000)):
            timing = row.get(phase) or {}
            elapsed = timing.get("elapsed_ms")
            if (timing.get("completed") is not True
                    or timing.get("bound_ms") != bound
                    or not isinstance(elapsed, (int, float))
                    or isinstance(elapsed, bool)
                    or not math.isfinite(elapsed) or elapsed <= 0):
                problems.append(f"{mode}/{phase}: missing or invalid measurement")
            elif elapsed * 2 > bound:
                problems.append(f"{mode}/{phase}: less than 2x timing margin")
    return problems


def report(output):
    problems = []

    def read(name):
        try:
            value = json.loads((output / name).read_text())
            if not isinstance(value, dict):
                raise ValueError("expected an object")
            return value
        except (OSError, ValueError) as error:
            problems.append(f"{name}: {error}")
            return {}

    rows = {mode: read(f"{mode}.json") for mode in ("normal", "contended")}
    load = read("load.json")
    problems.extend(validate(rows, load))
    output.mkdir(parents=True, exist_ok=True)
    (output / "p6-latency.json").write_text(json.dumps(
        {"runs": rows, "disk_pressure": load, "problems": problems}, indent=2) + "\n")
    lines = ["## Crash/restart timing", "",
             "Each measured wait must leave at least 2x room inside its unchanged limit.",
             "This is evidence from these two runs, not a population percentile.", "",
             "| Run | Wait | Observed ms | Limit ms | Margin |",
             "| --- | --- | ---: | ---: | ---: |"]
    for mode, row in rows.items():
        for phase in ("journal", "resume"):
            timing = row.get(phase) or {}
            elapsed, bound = timing.get("elapsed_ms"), timing.get("bound_ms")
            margin = f"{bound / elapsed:.2f}x" if (
                isinstance(elapsed, (int, float)) and elapsed > 0
                and isinstance(bound, (int, float))) else "unavailable"
            lines.append(f"| {mode} | {phase} | {elapsed} | {bound} | {margin} |")
    lines += ["", "Disk pressure:", "```json", json.dumps(load, indent=2), "```", ""]
    lines += [f"- FAIL: {problem}" for problem in problems]
    text = "\n".join(lines) + "\n"
    (output / "summary.md").write_text(text)
    if os.environ.get("GITHUB_STEP_SUMMARY"):
        with open(os.environ["GITHUB_STEP_SUMMARY"], "a") as summary:
            summary.write(text)
    print(text)
    return int(bool(problems))


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mode", choices=("contended", "report"))
    parser.add_argument("--output", type=Path, required=True)
    args, command = parser.parse_known_args()
    if command[:1] == ["--"]:
        command = command[1:]
    if args.mode == "contended" and not command:
        parser.error("contended requires a command after --")
    raise SystemExit(contended(command, args.output) if args.mode == "contended"
                     else report(args.output))
