"""Offline branch/accounting tests; production lifecycle tests live in Rust."""
import json
import os
import subprocess
import sys
from pathlib import Path
import tempfile
import unittest

from journal_accounting import completed_turns, select_journal


def identity(index):
	return f'{index:026d}'


def frame(index, event, by=None, prior=None, status=None):
	rows = [f'event: {event}@1', f'id: {identity(index)}']
	if by is not None:
		rows.append(f'by: {identity(by)}')
	if prior is not None:
		rows.append(f'prior: {identity(prior)}')
	rows.append('data: ' + json.dumps({} if status is None else {'status': status}))
	return '\n'.join(rows) + '\n\n'


class JournalAccountingTests(unittest.TestCase):
	def count(self, content, head=None):
		with tempfile.TemporaryDirectory() as root:
			path = Path(root) / 'one.oms'
			path.write_text(content)
			return completed_turns(path, head)

	def test_receipts_and_all_nonsuccess_terminal_statuses_do_not_count(self):
		content = frame(1, 'journal')
		for index, status in enumerate(('incomplete', 'failed', 'cancelled', 'steered'), 1):
			start = index * 3
			content += frame(start, 'turn.start', 1)
			content += frame(start + 1, 'turn.receipt', start)
			content += frame(start + 2, 'turn.outcome', start, status=status)
		self.assertEqual(self.count(content)['count'], 0)

	def test_distinct_turn_identity_not_receipt_or_duplicate_marker_count(self):
		content = frame(1, 'journal') + frame(2, 'turn.start', 1)
		content += frame(3, 'turn.receipt', 2) + frame(4, 'turn.receipt', 2)
		content += frame(5, 'turn.outcome', 2, status='completed')
		content += frame(6, 'turn.outcome', 2, status='completed')
		self.assertEqual(self.count(content)['turn_ids'], [identity(2)])

	def test_rewind_abandons_success_and_explicit_head_recovers_old_branch(self):
		content = frame(1, 'journal') + frame(2, 'turn.start', 1)
		content += frame(3, 'turn.outcome', 2, status='completed')
		content += frame(4, 'turn.outcome', 2, prior=2, status='cancelled')
		self.assertEqual(self.count(content)['count'], 0)
		self.assertEqual(self.count(content, identity(3))['count'], 1)

	def test_retry_after_rewind_counts_one_original_turn(self):
		content = frame(1, 'journal') + frame(2, 'turn.start', 1)
		content += frame(3, 'turn.outcome', 2, status='failed')
		content += frame(4, 'turn.receipt', 2, prior=2)
		content += frame(5, 'turn.outcome', 2, status='completed')
		self.assertEqual(self.count(content)['turn_ids'], [identity(2)])

	def test_torn_tail_is_not_a_completion(self):
		content = frame(1, 'journal') + frame(2, 'turn.start', 1)
		content += frame(3, 'turn.outcome', 2, status='completed').rstrip('\n')
		result = self.count(content)
		self.assertEqual(result['count'], 0)
		self.assertGreater(result['ignored_torn_tail_bytes'], 0)

	def test_conflicting_outcomes_missing_parent_and_wrong_cause_fail_closed(self):
		base = frame(1, 'journal') + frame(2, 'turn.start', 1)
		for tail in (
			frame(3, 'turn.outcome', 2, status='completed') + frame(4, 'turn.outcome', 2, status='failed'),
			frame(3, 'turn.outcome', 2, prior=9, status='completed'),
			frame(3, 'turn.outcome', 1, status='completed'),
		):
			with self.subTest(tail=tail), self.assertRaises(ValueError):
				self.count(base + tail)

	def test_never_selects_unrelated_newer_journal(self):
		with tempfile.TemporaryDirectory() as root:
			directory = Path(root)
			for index in (1, 2):
				(directory / (identity(index) + '.oms')).touch()
			with self.assertRaises(ValueError):
				select_journal(directory)
			self.assertEqual(select_journal(directory, identity(1)).name, identity(1) + '.oms')

	def test_report_preserves_500_threshold_and_uses_durable_count(self):
		with tempfile.TemporaryDirectory() as root:
			directory = Path(root)
			content = frame(1, 'journal')
			for number in range(500):
				start = 2 + number * 2
				content += frame(start, 'turn.start', 1)
				content += frame(start + 1, 'turn.outcome', start, status='completed')
			(directory / 'one.oms').write_text(content)
			result = subprocess.run([sys.executable, str(Path(__file__).with_name('report.py')), '--out', root, '--session-dir', root], capture_output=True, text=True, env={k: v for k, v in os.environ.items() if k != 'GITHUB_STEP_SUMMARY'})
			self.assertEqual(result.returncode, 0, result.stderr)
			self.assertIn('| completed distinct turns (journal) | 500 | >= 500 | PASS |', result.stdout)
			report = json.loads((directory / 'turn-accounting.json').read_text())
			self.assertEqual(len(report['turn_ids']), 500)
			self.assertEqual(report['head'], identity(1001))

	def test_duplicate_terminal_status_keys_fail_in_either_order(self):
		base = frame(1, 'journal') + frame(2, 'turn.start', 1)
		for first, second in (('failed', 'completed'), ('completed', 'failed')):
			terminal = frame(3, 'turn.outcome', 2, status='completed')
			terminal = terminal.replace('{"status": "completed"}', '{"status":"' + first + '","status":"' + second + '"}')
			with self.subTest(first=first), self.assertRaises(ValueError):
				self.count(base + terminal)
