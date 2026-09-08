"""Offline regression proofs for honest soak turn accounting (no omp binary)."""

import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

import driver

HERE = Path(__file__).resolve().parent


class AccountingTests(unittest.TestCase):
	def report(self, receipts, exits):
		with tempfile.TemporaryDirectory() as directory:
			root = Path(directory)
			(root / "session.oms").write_text(
				'event: turn.start@1\ndata: {}\n\n'
				+ 'event: turn.receipt@1\ndata: {}\n\n' * receipts
			)
			records = []
			for index, code in enumerate(exits):
				records.extend([
					{"kind": "turn_start", "turn": index, "ts": index},
					{"kind": "turn_end", "turn": index, "exit": code, "ts": index},
				])
			records.append({"kind": "end", "minutes": 60})
			(root / "driver.jsonl").write_text("\n".join(map(json.dumps, records)))
			result = subprocess.run(
				[sys.executable, str(HERE / "report.py"), "--out", directory,
				 "--session-dir", directory, "--strict"],
				text=True, capture_output=True,
				env={key: value for key, value in os.environ.items() if key != "GITHUB_STEP_SUMMARY"},
			)
			self.assertEqual(result.returncode, 1, result.stderr)
			self.assertIn("| completed distinct turns (journal) | unknown:", result.stdout)
			self.assertIn("| >= 500 | **FAIL** |", result.stdout)
			self.assertIn(f"| inference receipts (turn.receipt@1) | {receipts} | info; not completed turns | info |", result.stdout)
			return result.stdout

	def test_multiple_inferences_in_one_turn_do_not_satisfy_gate(self):
		self.report(1000, [0])

	def test_receipts_before_failed_or_killed_turn_do_not_satisfy_gate(self):
		self.report(1000, [1, -9])

	def test_failed_attempt_followed_by_success_is_not_two_completions(self):
		self.report(1000, [1, 0])

	def test_successful_process_exits_do_not_replace_journal_evidence(self):
		self.report(1000, [0] * 500)

	def test_driver_does_not_stop_after_failed_attempts(self):
		with tempfile.TemporaryDirectory() as directory:
			root = Path(directory)
			argv = ["driver.py", "--omp", "unused", "--out", directory,
				"--project", str(root / "project"), "--session-dir", str(root / "sessions"),
				"--data-dir", str(root / "data"), "--duration-min", "0", "--min-turns", "2"]
			with patch.object(sys, "argv", argv), patch.object(driver.signal, "signal"), \
				patch.object(driver.time, "sleep"), patch.object(driver, "run_turn", side_effect=[
					(1, "session", False), (-9, "session", True),
					(0, "session", False), (0, "session", False),
				]) as run:
				driver.main()
			self.assertEqual(run.call_count, 4)
			end = json.loads((root / "driver.jsonl").read_text().splitlines()[-1])
			self.assertEqual(end["turns"], 4)
			self.assertEqual(end["successful_exits"], 2)


if __name__ == "__main__":
	unittest.main()
