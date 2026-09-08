#!/usr/bin/env python3
"""Source-bound production watchdog proof; preserves raw negative-control exit."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import time

PARENT = 'b59b952172f1f331c2a79a6bc22f6ea25a3e2135'
FILES = ('scripts/soak/watchdog_fixture.py', 'scripts/soak/watchdog_provider.py', 'scripts/soak/turn_outcome_fixture.py', 'scripts/soak/journal_accounting.py', 'scripts/soak/driver.py', 'scripts/qa/harness.py', 'scripts/soak/check_watchdog.py', '.github/workflows/watchdog-proof.yml')


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def parent_control(report, raw_exit):
    phases = report.get('phases', [])
    if len(phases) != 1:
        return False
    phase = phases[0]
    return raw_exit == 1 and phase.get('phase') == 'repeat' and phase.get('status') == 'failed' and phase.get('failure_kind') == 'acceptance' and phase.get('raw_exit') == 0 and phase.get('matching_call_count') == 100 and phase.get('tool_results', 0) >= 100 and phase.get('provider_requests') == 101 and not phase.get('loop_notice_ids') and not phase.get('externally_stopped') and not phase.get('cleanup_errors')


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('mode', choices=('head', 'parent'))
    parser.add_argument('--proof-root', required=True, type=Path)
    parser.add_argument('--out', required=True, type=Path)
    args = parser.parse_args()
    proof, out = args.proof_root.resolve(), args.out.resolve()
    out.mkdir(parents=True, exist_ok=True)
    report = {'status': 'incomplete', 'mode': args.mode, 'commands': []}
    def save():
        (out / 'report.json').write_text(json.dumps(report, indent=2) + '\n')
    def run(name, command):
        row = {'name': name, 'argv': command, 'raw_exit': None, 'started_wall': time.time()}
        report['commands'].append(row)
        save()
        with (out / (name + '.log')).open('wb') as log:
            row['raw_exit'] = subprocess.run(command, stdout=log, stderr=subprocess.STDOUT).returncode
        row['finished_wall'] = time.time()
        save()
        return row['raw_exit']
    try:
        report.update(source_revision=subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip(), checker_revision=subprocess.check_output(['git', '-C', str(proof), 'rev-parse', 'HEAD'], text=True).strip(), source_before=subprocess.check_output(['git', 'status', '--porcelain=v1'], text=True), fixture_hashes={name: sha(proof / name) for name in FILES}, source_hashes={name: sha(Path(name)) for name in ('Cargo.lock', '.cargo/config.toml', 'rust-toolchain.toml')})
        assert not report['source_before'], 'source checkout must be clean'
        assert not subprocess.check_output(['git', '-C', str(proof), 'status', '--porcelain=v1'], text=True), 'checker checkout must be clean'
        if args.mode == 'parent':
            assert report['source_revision'] == PARENT
        assert run('offline-checker-tests', [sys.executable, '-m', 'unittest', 'discover', '-s', str(proof / 'scripts/soak'), '-p', 'test_watchdog*.py']) == 0
        assert run('build', ['just', 'build']) == 0, 'source build failed; no semantic evidence'
        binary = Path('target/debug/omp').resolve()
        report['binary_sha256'] = sha(binary)
        command = [sys.executable, str(proof / 'scripts/soak/watchdog_fixture.py'), '--binary', str(binary), '--out', str(out / 'fixture')]
        if args.mode == 'parent':
            command += ['--phase', 'repeat', '--parent']
        raw_exit = run('production-phases', command)
        observation = json.loads((out / 'fixture/report.json').read_text())
        report['observation'] = observation
        if args.mode == 'parent':
            report['expected_control_status'] = 'passed' if parent_control(observation, raw_exit) else 'failed'
            report['status'] = report['expected_control_status']
        else:
            report['status'] = 'passed' if raw_exit == 0 and observation['status'] == 'passed' else 'failed'
        assert report['fixture_hashes'] == {name: sha(proof / name) for name in FILES}, 'checker changed during proof'
        report['source_after'] = subprocess.check_output(['git', 'status', '--porcelain=v1'], text=True)
        assert report['source_before'] == report['source_after'], 'production source changed during proof'
    except BaseException as error:
        report.update(status='failed', error=str(error))
    finally:
        save()
        lines = [f'## Watchdog correctness ({args.mode})', '', f"Source: `{report.get('source_revision', 'unknown')}`. Status: **{report['status']}**.", '', '| Measurement | Observed | Frozen requirement |', '| --- | --- | --- |']
        for phase in report.get('observation', {}).get('phases', []):
            if phase['phase'] == 'repeat':
                lines.append(f"| identical tool calls before stop | {phase.get('matching_call_count', 'unknown')} | <=16 (2 x8) |")
            else:
                lines.append(f"| minutes to idle notice | {phase.get('minutes_to_idle_notice', 'unknown')} | <=31 |")
                lines.append(f"| real observation seconds | {phase.get('observed_seconds', 'unknown')} | >=1860 |")
                lines.append(f"| non-stream journal gap seconds | {phase.get('maximum_nonstream_gap_seconds', 'unknown')} | >=1800 |")
                lines.append(f"| held sleep gone at CLI exit | {phase.get('sleep_gone_at_cli_exit', 'unknown')} | true |")
        lines += ['', 'Raw process and checker exits remain in report.json. A parent expected-control PASS describes an observed100-call failure, never a passing parent criterion.', '', 'This is a deterministic transport correctness proof, not Anthropic performance evidence or the >=500-turn/>=60-minute full soak.']
        summary = '\n'.join(lines) + '\n'
        (out / 'summary.md').write_text(summary)
        if os.environ.get('GITHUB_STEP_SUMMARY'):
            with open(os.environ['GITHUB_STEP_SUMMARY'], 'a') as handle:
                handle.write(summary)
    return 0 if report['status'] == 'passed' else 1


if __name__ == '__main__':
    sys.exit(main())
