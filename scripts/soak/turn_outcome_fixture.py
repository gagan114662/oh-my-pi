#!/usr/bin/env python3
"""Two real print/resume turns; identical journal criterion for parent and head."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import time
from types import SimpleNamespace

from driver import run_turn
from journal_accounting import completed_turns, select_journal


def cleanup_daemons(binary, project):
	prefix = f'{binary} envd --root {project} --state-dir '
	listing = subprocess.check_output(['ps', '-axo', 'pid=,command='], text=True)
	for line in listing.splitlines():
		parts = line.strip().split(None, 1)
		if len(parts) != 2 or not parts[1].startswith(prefix):
			continue
		pid = int(parts[0])
		current = subprocess.run(['ps', '-p', str(pid), '-o', 'command='], capture_output=True, text=True, timeout=5).stdout.strip()
		if current == parts[1]:
			try:
				assert os.getpgid(pid) == pid, 'fixture daemon must own its process group'
				os.killpg(pid, signal.SIGKILL)
			except ProcessLookupError:
				pass


def main():
	parser = argparse.ArgumentParser()
	parser.add_argument('--binary', type=Path, required=True)
	parser.add_argument('--out', type=Path, required=True)
	args = parser.parse_args()
	binary, out = args.binary.resolve(), args.out.resolve()
	out.mkdir(parents=True, exist_ok=True)
	project, sessions, data = (out / name for name in ('project', 'sessions', 'data'))
	for directory in (project, sessions, data):
		directory.mkdir()
	report = {'status': 'incomplete', 'processes': [], 'binary_sha256': hashlib.sha256(binary.read_bytes()).hexdigest()}
	provider = None
	try:
		with (out / 'provider.log').open('wb') as log:
			provider = subprocess.Popen([sys.executable, str(Path(__file__).with_name('provider.py')), '--log', str(out / 'provider.jsonl'), '--ready-file', str(out / 'port')], stdout=log, stderr=subprocess.STDOUT)
		deadline = time.monotonic() + 10
		while not (out / 'port').exists():
			assert provider.poll() is None and time.monotonic() < deadline, 'provider failed to start'
			time.sleep(0.05)
		port = int((out / 'port').read_text())
		(data / 'models.toml').write_text(f'''[providers.mock]
baseUrl = "http://127.0.0.1:{port}/v1"
auth = "none"
[providers.mock.models.mock]
name = "Turn accounting mock"
api = "openai-completions"
contextWindow = 128000
maxTokens = 8192
supportsTools = true
supportsStreaming = true
''')
		options = SimpleNamespace(omp=str(binary), project=project, session_dir=sessions, model='mock', max_time='2m', turn_timeout=150, sentinel='TURN-ACCOUNTING')
		session_id = None
		for number in (1, 2):
			code, returned, killed = run_turn(options, number, session_id, out, dict(os.environ, OMP_DATA_DIR=str(data)))
			report['processes'].append({'turn': number, 'exit': code, 'session': returned, 'killed': killed})
			assert code == 0 and not killed and returned, 'actual print turn did not complete normally'
			assert session_id is None or returned == session_id, 'resume changed journal identity'
			session_id = returned
		path = select_journal(sessions, session_id)
		accounting = completed_turns(path)
		report['accounting'] = accounting
		journal = path.read_text()
		report['receipts'] = journal.count('event: turn.receipt@1\n')
		report['tool_results'] = journal.count('event: tool.result@1\n')
		report['provider_requests'] = sum(json.loads(line).get('kind') == 'request' for line in (out / 'provider.jsonl').read_text().splitlines())
		assert report['provider_requests'] >= 4 and report['receipts'] >= 4 and report['tool_results'] >= 2, 'required real tool continuations/inference evidence missing'
		if accounting['count'] != 2:
			report.update(status='failed', failure_kind='completed_turn_count', expected=2, actual=accounting['count'])
			return 1
		report['status'] = 'passed'
		return 0
	except BaseException as error:
		report.update(status='failed', failure_kind='execution', error=str(error))
		raise
	finally:
		if provider:
			provider.terminate()
			try:
				provider.wait(timeout=5)
			except subprocess.TimeoutExpired:
				provider.kill()
				provider.wait()
		try:
			cleanup_daemons(binary, project)
		except BaseException as error:
			report.update(status='failed', failure_kind='cleanup', cleanup_error=str(error))
			raise
		finally:
			(out / 'observation.json').write_text(json.dumps(report, indent=2) + '\n')
			print(json.dumps(report), flush=True)


if __name__ == '__main__':
	sys.exit(main())
