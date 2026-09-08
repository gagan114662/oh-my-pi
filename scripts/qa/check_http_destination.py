#!/usr/bin/env python3
"""Source-bound HTTP authority proof; expected mutation failure is a separate result."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import signal
import subprocess
import sys
import difflib

FIXTURE = 'crates/envd/src/http_egress_tests.rs'
FIXTURE_HASH = 'f6897c3ef1e7adf91d0dc00bd223091f45f1ba3eec4334880199d7a5cbc711d6'
IMPLEMENTATION = 'crates/envd/src/http_egress.rs'
PACKAGES = ('omp-envd', 'omp-env', 'omp-http', 'omp-driver')
CASES = (
    'native_http_denies_effectful_get_and_redirect_before_destination_effects',
    'native_http_allows_relative_and_explicit_cross_origin_redirects',
    'native_http_revocation_interrupts_held_redirect_and_preserves_destination_counter',
    'native_http_cancel_is_processed_while_redirect_response_is_pending',
    'native_http_explicit_broad_owner_scope_preserves_existing_local_access',
    'native_http_https_downgrade_never_reaches_plaintext_destination',
    'native_http_session_host_intersects_authenticated_project_baseline',
    'native_http_rejects_malformed_redirect_without_leaking_url_secrets',
)

def digest(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def git(*args, cwd=None):
    return subprocess.check_output(['git', *args], cwd=cwd, text=True).strip()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('mode', choices=('normal', 'mutation'))
    parser.add_argument('--proof-root', required=True, type=Path)
    parser.add_argument('--output', required=True, type=Path)
    args = parser.parse_args()
    proof = args.proof_root.resolve()
    out = args.output.resolve()
    out.mkdir(parents=True, exist_ok=True)
    def interrupted(signum, frame):
        raise KeyboardInterrupt(f'interrupted by signal {signum}')

    signal.signal(signal.SIGTERM, interrupted)
    records = []
    report = {'mode': args.mode, 'status': 'incomplete', 'commands': records}

    def save():
        (out / 'report.json').write_text(json.dumps(report, indent=2) + '\n')

    def run(name, argv, cases=()):
        record = {'name': name, 'argv': argv, 'raw_exit': None, 'status': 'incomplete'}
        records.append(record)
        save()
        # Write subprocess output directly: no pipeline can hide the raw test exit.
        with (out / (name + '.log')).open('wb') as log:
            result = subprocess.run(argv, stdout=log, stderr=subprocess.STDOUT)
        record['raw_exit'] = result.returncode
        log_text = (out / (name + '.log')).read_text(errors='replace')
        record['summaries'] = re.findall(r'Summary[^\n]*', log_text)
        record['missing_pass_cases'] = [case for case in cases if not re.search(r'PASS[^\n]*\b' + re.escape(case) + r'\b', log_text)]
        record['status'] = 'passed' if result.returncode == 0 and not record['missing_pass_cases'] else 'failed'
        save()
        print(name, json.dumps(record), flush=True)
        return record, log_text

    def focused(name, cases=CASES):
        return run(name, ['just', '--command', 'cargo', 'nextest', 'run', '--profile', 'ci', '--locked', '-p', 'omp-envd', '--lib', '--no-fail-fast', '--no-tests', 'fail', '-E', ' | '.join('test(' + case + ')' for case in cases)], cases)

    try:
        report.update(source_revision=git('rev-parse', 'HEAD'), checker_revision=git('rev-parse', 'HEAD', cwd=proof), checker_sha256=digest(__file__), workflow_sha256=digest(proof / '.github/workflows/http-destination.yml'), source_status_before=git('status', '--porcelain=v1'))
        assert digest(proof / FIXTURE) == FIXTURE_HASH, 'Proof fixture differs from frozen hash'
        Path(FIXTURE).write_bytes((proof / FIXTURE).read_bytes())
        report['source_status_after_fixture_copy'] = git('status', '--porcelain=v1')
        tracked = git('ls-files').splitlines()
        # Freeze every tracked test/source/config file for mutation restoration, not just the oracle.
        hashes = {p: digest(p) for p in tracked if Path(p).is_file()}
        report['source_file_sha256'] = hashes
        save()
        if args.mode == 'normal':
            focused('http-fixtures')
            for package, case in [('omp-driver', 'child_configuration_cannot_replace_captured_native_http_floor'), ('omp-env', 'native_http_and_its_cancel_follow_session_authority')]:
                run(package + '-focused', ['just', '--command', 'cargo', 'nextest', 'run', '--profile', 'ci', '--locked', '-p', package, '--no-fail-fast', '--no-tests', 'fail', '-E', 'test(' + case + ')'], (case,))
            # Continue through every package and doctest target after failures.
            for package in PACKAGES:
                run(package + '-targets', ['just', '--command', 'cargo', 'nextest', 'run', '--profile', 'ci', '--locked', '-p', package, '--all-targets', '--no-fail-fast', '--no-tests', 'fail'])
                run(package + '-doctests', ['just', '--command', 'cargo', 'test', '--doc', '--locked', '-p', package, '--no-fail-fast'])
            report['status'] = 'passed' if len(records) == 11 and all(r['status'] == 'passed' for r in records) else 'failed'
        else:
            preflight, _ = focused('preflight', CASES[:1])
            assert preflight['status'] == 'passed', 'Normal counter fixture must pass before mutation'
            original = Path(IMPLEMENTATION).read_bytes()
            needle = b'.any(|policy| !policy.allows_destination(&host, port))'
            assert original.count(needle) == 1, 'Expected exactly one destination admission predicate'
            mutated = original.replace(needle, b'.any(|_policy| false)')
            report['mutation'] = {'original_sha256': hashlib.sha256(original).hexdigest(), 'mutated_sha256': hashlib.sha256(mutated).hexdigest(), 'fixture_sha256': digest(FIXTURE), 'expected_control_status': 'incomplete'}
            (out / 'mutation.diff').write_text(''.join(difflib.unified_diff(original.decode().splitlines(True), mutated.decode().splitlines(True), fromfile=IMPLEMENTATION, tofile=IMPLEMENTATION)))
            save()
            try:
                Path(IMPLEMENTATION).write_bytes(mutated)
                mutant, log = focused('mutation-raw', CASES[:1])
                report['mutation']['raw_test_exit'] = mutant['raw_exit']
                oracle = ('denied GET reached independent destination' in log and re.search(r'left:\s*1\s*\n\s*right:\s*0', log) is not None)
                expected = mutant['raw_exit'] == 100 and oracle and CASES[0] in log
                report['mutation']['independent_counter_failure_observed'] = oracle
                report['mutation']['expected_control_status'] = 'passed' if expected else 'failed'
            finally:
                Path(IMPLEMENTATION).write_bytes(original)
                report['changed_files_after_restore'] = [p for p, sha in hashes.items() if not Path(p).is_file() or digest(p) != sha]
                save()
            restored, _ = focused('restored-normal')
            report['status'] = 'passed' if report['mutation']['expected_control_status'] == 'passed' and not report['changed_files_after_restore'] and restored['status'] == 'passed' else 'failed'
    except BaseException as error:
        report['status'] = 'failed'
        report['error'] = str(error)
        raise
    finally:
        save()
        if os.environ.get('GITHUB_STEP_SUMMARY'):
            with open(os.environ['GITHUB_STEP_SUMMARY'], 'a') as summary:
                summary.write('## Native HTTP ' + args.mode + '\n\n```json\n' + json.dumps({k: v for k, v in report.items() if k != 'source_file_sha256'}, indent=2) + '\n```\n')
    return 0 if report['status'] == 'passed' else 1


if __name__ == '__main__':
    sys.exit(main())
