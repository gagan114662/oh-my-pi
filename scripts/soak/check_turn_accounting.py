#!/usr/bin/env python3
"""Run source-bound terminal accounting evidence without transplanting production."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys

PARENT = '29fe965b37ba9a0a5ced3ddf7a73bb751db0ab7d'
PACKAGES = ('omp-journal', 'omp-session', 'omp-agent', 'omp-driver', 'omp-app')
FIXTURES = ('scripts/soak/turn_outcome_fixture.py', 'scripts/soak/driver.py', 'scripts/soak/provider.py', 'scripts/soak/journal_accounting.py', 'scripts/qa/harness.py')


def sha(path):
	return hashlib.sha256(path.read_bytes()).hexdigest()


def main():
	parser = argparse.ArgumentParser()
	parser.add_argument('mode', choices=('head', 'parent'))
	parser.add_argument('--proof-root', type=Path, required=True)
	parser.add_argument('--out', type=Path, required=True)
	args = parser.parse_args()
	proof, out = args.proof_root.resolve(), args.out.resolve()
	out.mkdir(parents=True, exist_ok=True)
	commands = []
	report = {'status': 'incomplete', 'mode': args.mode, 'commands': commands}

	def save():
		(out / 'report.json').write_text(json.dumps(report, indent=2) + '\n')

	def run(name, argv):
		record = {'name': name, 'argv': argv, 'raw_exit': None}
		commands.append(record)
		save()
		with (out / (name + '.log')).open('wb') as log:
			record['raw_exit'] = subprocess.run(argv, stdout=log, stderr=subprocess.STDOUT).returncode
		save()
		print(json.dumps(record), flush=True)
		return record['raw_exit']

	try:
		report.update(source_revision=subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip(), checker_revision=subprocess.check_output(['git', '-C', str(proof), 'rev-parse', 'HEAD'], text=True).strip(), checker_sha256=sha(Path(__file__)), workflow_sha256=sha(proof / '.github/workflows/turn-accounting.yml'), fixture_sha256={p: sha(proof / p) for p in FIXTURES}, source_status_before=subprocess.check_output(['git', 'status', '--porcelain=v1'], text=True))
		if args.mode == 'parent':
			assert report['source_revision'] == PARENT, 'comparison must use the frozen original source'
		if args.mode == 'head':
			run('accounting-tests', [sys.executable, '-m', 'unittest', 'discover', '-s', str(proof / 'scripts/soak'), '-p', 'test_*.py'])
		build = run('build', ['just', 'build'])
		try:
			if build == 0:
				fixture_exit = run('production-fixture', [sys.executable, str(proof / 'scripts/soak/turn_outcome_fixture.py'), '--binary', str(Path('target/debug/omp').resolve()), '--out', str(out / 'fixture')])
				observation = json.loads((out / 'fixture/observation.json').read_text())
				report['observation'] = observation
				if args.mode == 'parent':
					# Preserve raw fixture failure. A build, transport, tool, or cleanup
					# failure is not a successful negative control.
					expected = fixture_exit == 1 and observation['status'] == 'failed' and observation.get('failure_kind') == 'completed_turn_count' and observation.get('actual') == 0 and observation.get('expected') == 2
					expected = expected and len(observation['processes']) == 2 and all(p['exit'] == 0 and not p['killed'] for p in observation['processes'])
					expected = expected and observation['receipts'] >= 4 and observation['tool_results'] >= 2 and observation['provider_requests'] >= 4 and not observation['accounting']['terminal_outcomes']
					report['expected_parent_control'] = 'passed' if expected else 'failed'
					report['status'] = report['expected_parent_control']
				else:
					report['fixture_passed'] = fixture_exit == 0 and observation['status'] == 'passed' and observation['accounting']['count'] == 2
		except Exception as error:
			report['fixture_error'] = str(error)

		if args.mode == 'head':
			# All affected suites and docs run even after a preceding package fails.
			for package in PACKAGES:
				run(package + '-targets', ['just', '--command', 'cargo', 'nextest', 'run', '--profile', 'ci', '--locked', '-p', package, '--all-targets', '--no-fail-fast', '--no-tests', 'fail'])
				run(package + '-doctests', ['just', '--command', 'cargo', 'test', '--doc', '--locked', '-p', package, '--no-fail-fast'])
			report['status'] = 'passed' if len(commands) == 13 and report.get('fixture_passed') and all(command['raw_exit'] == 0 for command in commands) else 'failed'
		assert report['fixture_sha256'] == {p: sha(proof / p) for p in FIXTURES}, 'fixture changed during execution'
		report['source_status_after'] = subprocess.check_output(['git', 'status', '--porcelain=v1'], text=True)
		assert report['source_status_after'] == report['source_status_before'], 'production source changed during proof'
	except BaseException as error:
		report.update(status='failed', error=str(error))
		raise
	finally:
		save()
		if os.environ.get('GITHUB_STEP_SUMMARY'):
			with open(os.environ['GITHUB_STEP_SUMMARY'], 'a') as summary:
				summary.write('## Terminal turn accounting ' + args.mode + '\n\n```json\n' + json.dumps(report, indent=2) + '\n```\n')
	return 0 if report['status'] == 'passed' else 1


if __name__ == '__main__':
	sys.exit(main())
