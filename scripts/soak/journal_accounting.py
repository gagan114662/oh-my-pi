#!/usr/bin/env python3
"""Count durable terminal outcomes on one OMS branch, never inference receipts."""

import argparse
import hashlib
import json
from pathlib import Path
import re

IDENTITY = re.compile(r'[0-7][0-9A-HJKMNP-TV-Z]{25}\Z')


def completed_turns(path: Path, head: str | None = None) -> dict:
	content = path.read_bytes()
	frames = content.split(b'\n\n')
	trailing_bytes = len(frames.pop())
	entries = {}
	previous = None
	for frame in frames:
		fields = {}
		for line in frame.decode('utf-8').splitlines():
			if line.startswith(':'):
				continue
			key, sep, value = line.partition(':')
			if not sep or key in fields:
				raise ValueError('invalid or duplicate journal field')
			fields[key] = value.removeprefix(' ')
		entry_id = fields.get('id', '')
		if not IDENTITY.fullmatch(entry_id) or entry_id in entries:
			raise ValueError('missing, invalid, or duplicate journal identity')
		if not fields.get('event') or 'data' not in fields:
			raise ValueError('incomplete committed journal frame')
		fields['payload'] = json.loads(fields['data'])
		parent = fields.get('prior', previous)
		if previous is None:
			if fields['event'] != 'journal@1' or parent is not None or 'by' in fields:
				raise ValueError('invalid journal genesis')
		elif parent not in entries or fields.get('by') not in entries:
			raise ValueError('missing journal parent or cause')
		fields['parent'] = parent
		entries[entry_id] = fields
		previous = entry_id
	selected = previous if head is None else head
	if selected not in entries:
		raise ValueError('missing selected journal head')
	chain = []
	cursor = selected
	while cursor is not None:
		chain.append(cursor)
		cursor = entries[cursor]['parent']
	outcomes = {}
	current_turn = None
	for entry_id in reversed(chain):
		entry = entries[entry_id]
		if entry['event'] == 'turn.start@1':
			current_turn = entry_id
		elif entry['event'] == 'turn.outcome@1':
			if current_turn is None or entry.get('by') != current_turn:
				raise ValueError('terminal outcome does not identify the selected current turn')
			payload = entry['payload']
			if not isinstance(payload, dict) or set(payload) != {'status'} or payload['status'] not in ('completed', 'incomplete', 'failed', 'cancelled', 'steered'):
				raise ValueError('invalid terminal outcome payload')
			status = payload['status']
			if current_turn in outcomes and outcomes[current_turn] != status:
				raise ValueError('conflicting terminal outcomes on selected branch')
			outcomes[current_turn] = status
	ids = [turn for turn, status in outcomes.items() if status == 'completed']
	return {'journal': str(path), 'journal_sha256': hashlib.sha256(content).hexdigest(), 'count': len(ids), 'turn_ids': ids, 'head': selected, 'terminal_outcomes': outcomes, 'ignored_torn_tail_bytes': trailing_bytes}


def select_journal(directory: Path, session_id: str | None = None) -> Path:
	if session_id:
		if not IDENTITY.fullmatch(session_id):
			raise ValueError('invalid driver session identity')
		path = directory / (session_id + '.oms')
		if not path.is_file():
			raise ValueError('driver-selected journal missing')
		return path
	paths = list(directory.glob('*.oms'))
	if len(paths) != 1:
		raise ValueError('journal selection ambiguous or missing')
	return paths[0]


if __name__ == '__main__':
	parser = argparse.ArgumentParser()
	parser.add_argument('journal', type=Path)
	parser.add_argument('--head')
	args = parser.parse_args()
	try:
		print(completed_turns(args.journal, args.head)['count'])
	except (OSError, ValueError) as error:
		print('unknown')
		raise SystemExit(1) from error
