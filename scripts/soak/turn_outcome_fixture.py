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


def fixture_environment(out, data):
	environment = dict(os.environ)
	for name in ('OMP_PROFILE', 'OMP_CONFIG_FILES', 'OMP_CODING_AGENT_SESSION_DIR', 'OMP_SESSION_DIR', 'BASH_ENV', 'ENV', 'ZDOTDIR'):
		environment.pop(name, None)
	paths = {
		'HOME': out / 'home',
		'OMP_CONFIG_DIR': out / 'config', 'OMP_DATA_DIR': data,
		'OMP_CACHE_DIR': out / 'cache', 'OMP_STATE_DIR': out / 'state',
		'XDG_CONFIG_HOME': out / 'xdg/config', 'XDG_DATA_HOME': out / 'xdg/data',
		'XDG_CACHE_HOME': out / 'xdg/cache', 'XDG_STATE_HOME': out / 'xdg/state',
	}
	for name, path in paths.items():
		path.mkdir(parents=True, exist_ok=True)
		environment[name] = str(path)
	return environment, {name: str(path) for name, path in paths.items()}


def process_command(pid):
	result = subprocess.run(['ps', '-p', str(pid), '-o', 'command='], capture_output=True, text=True, timeout=5)
	if result.returncode not in (0, 1):
		raise RuntimeError(f'process inspection failed for {pid}: exit {result.returncode}')
	return result.stdout.strip() or None


def wait_identity_gone(pid, command, seconds):
	deadline = time.monotonic() + seconds
	while process_command(pid) == command:
		if time.monotonic() >= deadline:
			return False
		time.sleep(0.05)
	return True


def cleanup_daemons(binary, project, records):
	prefix = f'{binary} envd --root {project} --state-dir '
	listing = subprocess.check_output(['ps', '-axo', 'pid=,command='], text=True, timeout=5)
	for line in listing.splitlines():
		parts = line.strip().split(None, 1)
		if len(parts) != 2 or not parts[1].startswith(prefix):
			continue
		pid, command = int(parts[0]), parts[1]
		record = {'pid': pid, 'command': command, 'signals_sent': [], 'identity_disappeared': False, 'exit_code': None}
		records.append(record)
		for sig in (signal.SIGTERM, signal.SIGKILL):
			# Revalidate both identity and group ownership immediately before each
			# signal, including escalation; never signal a reused unrelated PID.
			if process_command(pid) != command:
				record['identity_disappeared'] = True
				break
			try:
				assert os.getpgid(pid) == pid, 'fixture daemon must own its process group'
				os.killpg(pid, sig)
				record['signals_sent'].append(sig.name)
			except ProcessLookupError:
				pass
			if wait_identity_gone(pid, command, 5):
				record['identity_disappeared'] = True
				break
		if not record['identity_disappeared']:
			raise RuntimeError(f'fixture daemon identity {pid} survived bounded TERM/KILL cleanup')
		# Detached daemons are not our child: disappearance is verified, but an
		# exit code is not available and is deliberately not fabricated.


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
	environment, isolated_paths = fixture_environment(out, data)
	report = {'status': 'incomplete', 'processes': [], 'daemon_cleanup': [], 'isolated_paths': isolated_paths, 'binary_sha256': hashlib.sha256(binary.read_bytes()).hexdigest()}
	provider = None
	try:
		with (out / 'provider.log').open('wb') as log:
			provider = subprocess.Popen([sys.executable, str(Path(__file__).with_name('provider.py')), '--log', str(out / 'provider.jsonl'), '--ready-file', str(out / 'port')], stdout=log, stderr=subprocess.STDOUT, cwd=project, env=environment)
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
			code, returned, killed = run_turn(options, number, session_id, out, environment)
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
		cleanup_errors = []
		if provider:
			provider_cleanup = {'pid': provider.pid, 'signals_sent': [], 'exit_code': None}
			report['provider_cleanup'] = provider_cleanup
			try:
				if provider.poll() is None:
					provider.terminate()
					provider_cleanup['signals_sent'].append('SIGTERM')
				try:
					provider_cleanup['exit_code'] = provider.wait(timeout=5)
				except subprocess.TimeoutExpired:
					provider.kill()
					provider_cleanup['signals_sent'].append('SIGKILL')
					provider_cleanup['exit_code'] = provider.wait(timeout=5)
			except Exception as error:
				cleanup_errors.append(str(error))
		try:
			cleanup_daemons(binary, project, report['daemon_cleanup'])
		except Exception as error:
			cleanup_errors.append(str(error))
		if cleanup_errors:
			report.update(status='failed', failure_kind='cleanup', cleanup_errors=cleanup_errors)
		(out / 'observation.json').write_text(json.dumps(report, indent=2) + '\n')
		print(json.dumps(report), flush=True)
		if cleanup_errors:
			raise RuntimeError('fixture cleanup could not verify termination')



if __name__ == '__main__':
	sys.exit(main())
