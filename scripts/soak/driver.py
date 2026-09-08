#!/usr/bin/env python3
"""Soak driver (#105): one session, one journal, many real ``omp print`` turns.

Runs ``omp print --mode json`` repeatedly against the same session file.
The first turn creates the session and reads its id from the ``session``
header line; every later turn passes ``--resume <id>`` so each restart goes
through the production resume path. Faults are NOT injected here: the
workflow kills the child from its pidfile, fills the disk, and steers the
provider. This loop only keeps issuing turns and records what happened.

Stdlib only. Writes:
  <out>/driver.jsonl   one record per turn (start, end, exit, killed, session)
  <out>/omp.pid        pid of the live ``omp print`` child (its own session/pgid)
  <out>/session.txt    session id once known
"""

from __future__ import annotations

import argparse
import json
import os
import signal
import subprocess
import sys
import threading
import time
from pathlib import Path


def log(path: Path, record: dict) -> None:
	record["ts"] = time.time()
	with path.open("a") as handle:
		handle.write(json.dumps(record) + "\n")


def run_turn(options, turn: int, session_id: str | None, out: Path, env: dict) -> tuple[int | None, str | None, bool]:
	prompt = (
		f"Soak turn {turn}. Run the tool and then summarise. Remember the code word {options.sentinel}."
		if turn == 1
		else f"Soak turn {turn}. Run the tool and then summarise in one sentence."
	)
	command = [
		options.omp,
		"print",
		"--mode",
		"json",
		"--yolo",
		"--model",
		options.model,
		"--project",
		str(options.project),
		"--session-dir",
		str(options.session_dir),
		"--max-time",
		options.max_time,
	]
	if session_id:
		command += ["--resume", session_id]
	command.append(prompt)
	started = time.time()
	child = subprocess.Popen(
		command,
		cwd=options.project,
		env=env,
		stdout=subprocess.PIPE,
		stderr=open(out / "omp-stderr.log", "ab"),
		stdin=subprocess.DEVNULL,
		text=True,
		start_new_session=True,
	)
	(out / "omp.pid").write_text(str(child.pid))
	log(out / "driver.jsonl", {"kind": "turn_start", "turn": turn, "pid": child.pid, "resume": session_id})
	found: list[str] = []
	lines = 0

	def pump():
		nonlocal lines
		assert child.stdout is not None
		for line in child.stdout:
			lines += 1
			line = line.strip()
			if not found and line.startswith("{"):
				try:
					event = json.loads(line)
				except json.JSONDecodeError:
					continue
				if event.get("type") == "session" and event.get("id"):
					found.append(str(event["id"]))
					(out / "session.txt").write_text(found[0])

	reader = threading.Thread(target=pump, daemon=True)
	reader.start()
	killed = False
	try:
		child.wait(timeout=options.turn_timeout)
	except subprocess.TimeoutExpired:
		killed = True
		try:
			os.killpg(child.pid, signal.SIGKILL)
		except ProcessLookupError:
			pass
		child.wait()
	reader.join(timeout=5)
	try:
		(out / "omp.pid").unlink()
	except FileNotFoundError:
		pass
	log(
		out / "driver.jsonl",
		{
			"kind": "turn_end",
			"turn": turn,
			"pid": child.pid,
			"exit": child.returncode,
			"seconds": round(time.time() - started, 3),
			"stdout_lines": lines,
			"timed_out": killed,
			"session": found[0] if found else session_id,
		},
	)
	return child.returncode, (found[0] if found else session_id), killed


def main() -> None:
	parser = argparse.ArgumentParser()
	parser.add_argument("--omp", required=True)
	parser.add_argument("--out", required=True)
	parser.add_argument("--project", required=True)
	parser.add_argument("--session-dir", required=True)
	parser.add_argument("--data-dir", required=True)
	parser.add_argument("--model", default="mock")
	parser.add_argument("--duration-min", type=float, default=60)
	parser.add_argument("--min-turns", type=int, default=500)
	parser.add_argument("--max-min", type=float, default=120)
	parser.add_argument("--turn-timeout", type=float, default=720)
	parser.add_argument("--max-time", default="10m")
	parser.add_argument("--sentinel", default="SENTINEL-unset")
	parser.add_argument("--pause", type=float, default=0.2, help="seconds between turns")
	parser.add_argument("--child-env", action="append", default=[], metavar="NAME=VALUE", help="environment set only for the omp child (e.g. libfaketime); the driver itself must not be preloaded")
	options = parser.parse_args()
	options.project = Path(options.project)
	options.session_dir = Path(options.session_dir)
	out = Path(options.out)
	out.mkdir(parents=True, exist_ok=True)
	options.project.mkdir(parents=True, exist_ok=True)
	options.session_dir.mkdir(parents=True, exist_ok=True)
	env = {**os.environ, "OMP_DATA_DIR": options.data_dir}
	for item in options.child_env:
		name, _, value = item.partition("=")
		env[name] = value
	started = time.time()
	log(out / "driver.jsonl", {"kind": "start", "pid": os.getpid(), "argv": sys.argv})
	session_id = None
	turn = 0
	successful_exits = 0
	stop = {"flag": False}

	def on_term(signum, frame):
		stop["flag"] = True

	signal.signal(signal.SIGTERM, on_term)
	while not stop["flag"]:
		elapsed_min = (time.time() - started) / 60
		if elapsed_min >= options.max_min:
			break
		if elapsed_min >= options.duration_min and successful_exits >= options.min_turns:
			break
		turn += 1
		code, session_id, _ = run_turn(options, turn, session_id, out, env)
		# This is only an execution budget: the report independently fails the
		# completed-turn gate until the journal carries authoritative outcomes.
		successful_exits += int(code == 0)
		time.sleep(options.pause)
	log(out / "driver.jsonl", {"kind": "end", "turns": turn, "successful_exits": successful_exits, "minutes": round((time.time() - started) / 60, 2), "session": session_id})


if __name__ == "__main__":
	main()
