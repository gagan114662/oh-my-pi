#!/usr/bin/env python3
"""Soak report (#105): renders the shell's measurements and grades the frozen rows.

Inputs (all produced by the workflow shell, the provider log, or the journal):
  <out>/samples.csv      sample.sh rows
  <out>/phases.jsonl     phase.sh markers
  <out>/driver.jsonl     driver turn records
  <out>/provider.jsonl   provider request log
  <out>/last-request.json
  <session-dir>/*.oms    the journal

Every row names the issue whose demo it is. ``--strict`` exits 1 when any
required row fails; the rig is expected to be red until those issues land.
"""

from __future__ import annotations

import argparse
import csv
import json
import os
import statistics
import sys
from pathlib import Path


def jsonl(path: Path) -> list[dict]:
	if not path.exists():
		return []
	out = []
	for line in path.read_text().splitlines():
		line = line.strip()
		if line:
			try:
				out.append(json.loads(line))
			except json.JSONDecodeError:
				pass
	return out


def phase_times(phases: list[dict], name: str) -> list[float]:
	return [float(p["ts"]) for p in phases if p.get("phase") == name]


def first_ok_turn_after(driver: list[dict], ts: float) -> float | None:
	ends = [d for d in driver if d.get("kind") == "turn_end" and d.get("exit") == 0 and float(d["ts"]) >= ts]
	return min(float(d["ts"]) for d in ends) if ends else None


def main() -> None:
	parser = argparse.ArgumentParser()
	parser.add_argument("--out", required=True)
	parser.add_argument("--session-dir", required=True)
	parser.add_argument("--min-turns", type=int, default=500)
	parser.add_argument("--min-minutes", type=float, default=60)
	parser.add_argument("--sentinel", default="")
	parser.add_argument("--strict", action="store_true")
	options = parser.parse_args()
	out = Path(options.out)
	samples = list(csv.DictReader((out / "samples.csv").open())) if (out / "samples.csv").exists() else []
	phases = jsonl(out / "phases.jsonl")
	driver = jsonl(out / "driver.jsonl")
	provider = jsonl(out / "provider.jsonl")
	requests = [r for r in provider if r.get("kind") == "request"]
	journals = sorted(Path(options.session_dir).glob("*.oms"), key=lambda p: p.stat().st_mtime)
	journal = journals[-1].read_text(errors="replace") if journals else ""

	def count(prefix: str) -> int:
		return journal.count(f"\nevent: {prefix}") + (1 if journal.startswith(f"event: {prefix}") else 0)

	rows: list[tuple[str, str, str, str, bool | None]] = []

	def row(issue: str, name: str, value, threshold: str, ok: bool | None):
		rows.append((issue, name, str(value), threshold, ok))

	# --- #105 rig rows -------------------------------------------------------
	receipts = count("turn.receipt@1")
	minutes = 0.0
	for d in driver:
		if d.get("kind") == "end":
			minutes = float(d.get("minutes", 0))
	turns_started = sum(1 for d in driver if d.get("kind") == "turn_start")
	turns_ok = sum(1 for d in driver if d.get("kind") == "turn_end" and d.get("exit") == 0)
	# Receipts account for inference requests, not explicit user turns. A
	# tool continuation/retry can emit several receipts; a later failure can
	# leave them all durable. The current journal has no successful turn-end
	# event after the Director/hook yield checks, so this criterion is unknown.
	# Fail closed even when every driver process exited successfully.
	row("#105", "completed distinct turns (journal)", "unknown: no durable successful turn-end event", f">= {options.min_turns}", False)
	row("#105", "inference receipts (turn.receipt@1)", receipts, "info; not completed turns", None)
	row("#105", "driver minutes", f"{minutes:.1f}", f">= {options.min_minutes}", minutes >= options.min_minutes)
	row("#105", "driver attempts started / processes exiting 0", f"{turns_started} / {turns_ok}", "info", None)
	kills = [float(p["ts"]) for p in phases if p.get("phase") == "kill" and p.get("pid") not in (None, "", "none")]
	resumed = 0
	for ts in kills:
		if first_ok_turn_after(driver, ts) is not None:
			resumed += 1
	row("#105", "kill -9 of a live omp (pid recorded) / resumed with a clean later turn", f"{len(kills)} / {resumed}", "resumed == kills >= 5", len(kills) >= 5 and resumed == len(kills))

	def sample_at(minute: int) -> dict | None:
		for s in samples:
			if int(s["minute"]) >= minute:
				return s
		return None

	s5, s_last = sample_at(5), (samples[-1] if samples else None)
	if s5 and s_last:
		r5, rl = int(s5["rss_envd_kb"] or 0), int(s_last["rss_envd_kb"] or 0)
		ok = r5 > 0 and rl <= 1.5 * r5
		row("#105/#129", "envd RSS kB at min 5 / last sample", f"{r5} / {rl}", "last <= 1.5 x min5", ok)
		row("#105", "omp print RSS kB at min 5 / last sample", f"{s5['rss_omp_kb']} / {s_last['rss_omp_kb']}", "info (short-lived process)", None)
		row("#105", "journal bytes at min 5 / last sample", f"{s5['journal_bytes']} / {s_last['journal_bytes']}", "info", None)
		row("#109", "data dir bytes (du -sb) at min 5 / last sample", f"{s5['blob_bytes']} / {s_last['blob_bytes']}", "info until a cap exists", None)
	final = [p for p in phases if p.get("phase") == "final"]
	if final:
		f = final[-1]
		orphans = int(f.get("omp_print_procs") or -1)
		row("#105/#125", "omp print processes after driver exit (pgrep)", orphans, "== 0", orphans == 0)
		row("#105", "envd processes after driver exit (pgrep)", f.get("envd_procs", "?"), "info (900 s idle timeout by design)", None)

	# --- #107 ENOSPC -----------------------------------------------------------
	fills, frees = phase_times(phases, "enospc-fill"), phase_times(phases, "enospc-free")
	if fills and frees:
		alive = any(d.get("kind") == "turn_end" and float(d["ts"]) > fills[0] and float(d["ts"]) < frees[0] and d.get("exit") == 0 for d in driver)
		after = first_ok_turn_after(driver, frees[0])
		row("#107", "clean turn while disk full", alive, "true (degraded, not dead)", alive)
		row("#107", "seconds to clean turn after space freed", "none" if after is None else f"{after - frees[0]:.0f}", "<= 120", after is not None and after - frees[0] <= 120)
		row("#107", "storage-degraded notices in journal", journal.count("storage-degraded"), ">= 1", journal.count("storage-degraded") >= 1)

	# --- #118 stall ------------------------------------------------------------
	stalled = [r for r in requests if r.get("mode") == "stall"]
	if stalled:
		st = float(stalled[0]["ts"])
		later = [float(r["ts"]) for r in requests if float(r["ts"]) > st]
		gap = (min(later) - st) if later else None
		row("#105", "stall: seconds to next provider request", "none" if gap is None else f"{gap:.0f}", "< 300", gap is not None and gap < 300)
		row("#118", "stall: seconds to next provider request", "none" if gap is None else f"{gap:.0f}", "<= 90", gap is not None and gap <= 90)

	# --- #119 529 window -------------------------------------------------------
	e_start, e_end = phase_times(phases, "error-start"), phase_times(phases, "error-end")
	if e_start and e_end:
		inside = [r for r in requests if e_start[0] <= float(r["ts"]) <= e_end[0]]
		errs = sum(1 for r in inside if r.get("status") == 529)
		row("#105/#119", "provider requests during the 15-minute 529 window", f"{len(inside)} ({errs} answered 529)", "<= 20", len(inside) <= 20)
		after = first_ok_turn_after(driver, e_end[0])
		row("#119", "seconds to clean turn after the window", "none" if after is None else f"{after - e_end[0]:.0f}", "<= 180", after is not None and after - e_end[0] <= 180)

	# --- #112 overflow ---------------------------------------------------------
	overflows = [r for r in requests if r.get("status") == 400]
	if phase_times(phases, "overflow-start"):
		row("#112", "context_length_exceeded responses", len(overflows), "info", None)
		if overflows:
			after = first_ok_turn_after(driver, float(overflows[0]["ts"]))
			row("#112", "clean turn after the first overflow", after is not None, "true", after is not None)
			row("#112", "compaction@1 entries", count("compaction@1"), ">= 1 after overflow", count("compaction@1") >= 1)

	# --- #106 stream granularity ---------------------------------------------
	streams, starts = count("stream@1"), count("msg.assistant.start@1")
	if starts:
		ratio = streams / starts
		row("#106", "stream@1 entries per assistant message", f"{ratio:.1f} ({streams}/{starts})", "<= 8 at 20 deltas/s", ratio <= 8)

	# --- #114 sentinel ---------------------------------------------------------
	last = out / "last-request.json"
	if options.sentinel and last.exists():
		present = options.sentinel in last.read_text(errors="replace")
		row("#114", f"sentinel {options.sentinel} in the last provider request", present, "true", present)

	# --- #115 prefix stability --------------------------------------------------
	tail = [r for r in requests if r.get("messages_bytes")][-100:]
	if len(tail) >= 10:
		ratios = [r["shared_prefix_bytes"] / r["messages_bytes"] for r in tail if r["messages_bytes"]]
		med = statistics.median(ratios)
		row("#115", "median shared-prefix ratio, last 100 requests", f"{med:.2f}", ">= 0.85", med >= 0.85)

	# --- #122 clock step / #125 SIGHUP -----------------------------------------
	for name, issue, label in (("clock-back", "#122", "clean turn within 180 s after clock stepped back 1 h"), ("sighup", "#125", "clean turn within 180 s after SIGHUP")):
		ts = phase_times(phases, name)
		if ts:
			after = first_ok_turn_after(driver, ts[0])
			ok = after is not None and after - ts[0] <= 180
			row(issue, label, "none" if after is None else f"{after - ts[0]:.0f}s", "<= 180", ok)
	if phase_times(phases, "sighup"):
		hup = [p for p in phases if p.get("phase") == "sighup"]
		pid = hup[-1].get("pid")
		ends = [d for d in driver if d.get("kind") == "turn_end" and str(d.get("pid")) == str(pid)]
		row("#125", f"exit code of the omp print process that received SIGHUP (pid {pid})", ends[-1].get("exit") if ends else "?", "== 0 (continue headless)", bool(ends) and ends[-1].get("exit") == 0)

	# --- render ---------------------------------------------------------------
	lines = ["## Soak report (#105)", "", f"Journal: `{journals[-1].name if journals else 'none'}` · samples: {len(samples)} · provider requests: {len(requests)}", "", "| issue | row | value | threshold | result |", "|---|---|---|---|---|"]
	failed = 0
	for issue, name, value, threshold, ok in rows:
		mark = "info" if ok is None else ("PASS" if ok else "**FAIL**")
		failed += 0 if ok in (None, True) else 1
		lines.append(f"| {issue} | {name} | {value} | {threshold} | {mark} |")
	lines += ["", "### Samples (every interval, taken by the shell)", "", "| min | phase | journal B | data dir B | rss omp kB | rss envd kB | omp | envd | receipts | compactions | stream | asst | notices |", "|---|---|---|---|---|---|---|---|---|---|---|---|---|"]
	for s in samples:
		lines.append("| " + " | ".join(s[k] for k in ("minute", "phase", "journal_bytes", "blob_bytes", "rss_omp_kb", "rss_envd_kb", "omp_procs", "envd_procs", "receipts", "compactions", "stream_entries", "assistant_starts", "notices")) + " |")
	text = "\n".join(lines) + "\n"
	print(text)
	summary = os.environ.get("GITHUB_STEP_SUMMARY")
	if summary:
		with open(summary, "a") as handle:
			handle.write(text)
	(out / "report.md").write_text(text)
	print(f"{failed} required row(s) failed", file=sys.stderr)
	if options.strict and failed:
		sys.exit(1)


if __name__ == "__main__":
	main()
