#!/usr/bin/env python3
"""Capture nextest evidence and fail closed on zero/missing workspace coverage.

Sources: https://nexte.st/docs/machine-readable/list/ and
https://nexte.st/docs/machine-readable/junit/ . Counts are execution records,
never Rust source-marker counts. Doctests remain a separate CI obligation.
"""
import argparse
import hashlib
import json
import os
import re
from pathlib import Path
import subprocess
import sys
import xml.etree.ElementTree as ET


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def save(path, value):
    path.write_text(json.dumps(value, indent=2) + '\n')


def members(metadata):
    packages = {p['id']: p for p in metadata['packages']}
    ids = metadata['workspace_members']
    if len(set(ids)) != len(ids) or any(i not in packages for i in ids):
        raise ValueError('Invalid Cargo workspace membership')
    return {i: packages[i]['name'] for i in ids}


def selected_packages(names, args):
    # Deliberately require explicit scope; never guess Cargo's default members.
    selected, excluded = set(), set()
    for index, arg in enumerate(args):
        if arg == '--workspace':
            selected.update(names)
        elif arg in ('-p', '--package', '--exclude'):
            if index + 1 == len(args):
                raise ValueError('Missing package selector value')
            matches = {pid for pid, name in names.items() if name == args[index + 1] or pid == args[index + 1]}
            if not matches:
                raise ValueError('Package selector is not an exact workspace member: ' + args[index + 1])
            (excluded if arg == '--exclude' else selected).update(matches)
    if not selected:
        raise ValueError('Capture requires --workspace or exact -p/--package selectors')
    return sorted(selected - excluded)


def listed_tests(data, names, scope):
    binaries, expected, registered, ignored = {}, set(), set(), set()
    suites = data['rust-suites']
    for key, suite in suites.items():
        pid, binary = suite['package-id'], suite['binary-id']
        if pid not in names or pid not in scope or binary != key or binary in binaries:
            raise ValueError('Unknown, duplicate or out-of-scope test binary: ' + key)
        binaries[binary] = pid
        if suite.get('status') != 'listed':
            raise ValueError('Test binary was not successfully listed: ' + binary)
        for name, test in suite['testcases'].items():
            identity = (binary, name)
            registered.add(identity)
            if test['ignored']:
                ignored.add(identity)
            elif test['filter-match']['status'] == 'matches':
                expected.add(identity)
    if data['test-count'] != len(registered):
        raise ValueError('nextest test-count disagrees with testcase inventory')
    return binaries, registered, expected, ignored


def junit_results(path, binaries, registered):
    root = ET.parse(path).getroot()
    if root.tag != 'testsuites':
        raise ValueError('Expected nextest testsuites XML root')
    results = {}
    for suite in root.findall('testsuite'):
        binary = suite.attrib['name']
        if binary not in binaries:
            raise ValueError('JUnit binary missing from nextest list: ' + binary)
        cases = suite.findall('testcase')
        if int(suite.attrib['tests']) != len(cases):
            raise ValueError('JUnit suite count does not match testcase elements')
        for case in cases:
            key = (binary, case.attrib['name'])
            if key not in registered or key in results:
                raise ValueError('Unknown or duplicate executed test: ' + repr(key))
            if case.attrib.get('classname', binary) != binary:
                raise ValueError('JUnit classname disagrees with binary identity')
            if case.find('skipped') is not None:
                result = 'skipped'
            elif any(case.find(tag) is not None for tag in ('failure', 'error', 'flakyFailure', 'flakyError', 'rerunFailure', 'rerunError')):
                # CI promises no retries. Never hide a flaky first failure.
                result = 'failed'
            else:
                result = 'passed'
            results[key] = result
    if 'tests' in root.attrib and int(root.attrib['tests']) != len(results):
        raise ValueError('JUnit total count disagrees with testcases')
    return results


def summarize(metadata, runs):
    names = members(metadata)
    rows = {pid: {'package': name, 'registered': set(), 'expected': set(), 'passed': set(),
                  'failed': set(), 'skipped': set(), 'ignored': set(), 'missing': set(),
                  'discovered': False, 'execution_evidence': False, 'problems': []} for pid, name in names.items()}
    errors = []
    for run, folder in runs:
        scope = list(rows)
        try:
            scope = run['packages']
            if any(pid not in rows for pid in scope):
                raise ValueError('Run names a package outside the metadata inventory')
            if run.get('discovery_exit') != 0:
                for pid in scope:
                    rows[pid]['problems'].append('discovery/build failed or incomplete; execution unknown')
                errors.append(run['phase'] + ': discovery did not complete')
                continue
            binaries, registered, expected, ignored = listed_tests(json.loads((folder / 'list.json').read_text()), names, scope)
            for pid in scope:
                rows[pid]['discovered'] = True
            for field, values in [('registered', registered), ('expected', expected), ('ignored', ignored)]:
                for key in values:
                    rows[binaries[key[0]]][field].add(key)
            if run.get('run_exit') != 0:
                errors.append(run['phase'] + ': nextest run failed or did not finish')
                for pid in scope:
                    rows[pid]['problems'].append('execution phase failed or incomplete; see phase exit/log')
            report = folder / 'junit.xml'
            if not report.exists():
                errors.append(run['phase'] + ': JUnit evidence missing')
                for pid in scope:
                    rows[pid]['problems'].append('JUnit missing; execution unknown')
                actual = {}
            else:
                actual = junit_results(report, binaries, registered)
                for pid in scope:
                    rows[pid]['execution_evidence'] = True
            for key in expected:
                if actual.get(key) == 'skipped':
                    rows[binaries[key[0]]]['problems'].append('Selected test was skipped instead of executed')
            for key, outcome in actual.items():
                rows[binaries[key[0]]][outcome].add(key)
            for key in expected - actual.keys():
                rows[binaries[key[0]]]['missing'].add(key)
                errors.append(run['phase'] + ': expected result missing: ' + repr(key))
        except (OSError, ValueError, KeyError, TypeError, ET.ParseError) as error:
            errors.append(str(run.get('phase', 'unknown')) + ': invalid evidence: ' + str(error))
            for pid in scope:
                if pid in rows:
                    rows[pid]['problems'].append('invalid phase evidence: ' + str(error))
    output = []
    for row in rows.values():
        # A later passing repeat never erases an earlier failure.
        row['passed'] -= row['failed']
        executed = row['passed'] | row['failed']
        if not row['discovered']:
            status = 'unknown: discovery unavailable'
        elif not row['registered']:
            status = 'zero registered tests'
        elif not row['execution_evidence']:
            status = 'unknown: execution unavailable'
        elif not executed:
            status = 'zero executed tests'
        elif row['failed']:
            status = 'failed tests'
        elif row['missing'] or row['problems']:
            status = 'incomplete evidence'
        else:
            status = 'passed'
        if status != 'passed':
            errors.append(row['package'] + ': ' + status)
        output.append({'package': row['package'], 'status': status, 'run': len(executed),
                       **{k: len(row[k]) for k in ('registered', 'expected', 'passed', 'failed', 'skipped', 'ignored', 'missing')},
                       'problems': sorted(set(row['problems']))})
        if not row['discovered']:
            for key in ('registered', 'expected', 'ignored'):
                output[-1][key] = None
        if not row['execution_evidence'] and (not row['discovered'] or row['registered']):
            for key in ('run', 'passed', 'failed', 'skipped', 'missing'):
                output[-1][key] = None
    return {'status': 'failed' if errors else 'passed', 'packages': sorted(output, key=lambda r: r['package']),
            'errors': sorted(set(errors)), 'count_semantics': 'unique binary-id/test-name outcomes; no doctests included'}


def target_inventory(metadata, runs):
    """Retain every declared test target, even when discovery never reaches it."""
    names = members(metadata)
    rows = {}
    for package in metadata['packages']:
        if package['id'] not in names:
            continue
        for target in package.get('targets', []):
            if not target.get('test'):
                continue
            kind = target['kind'][0]
            if kind in ('lib', 'rlib', 'dylib', 'cdylib', 'staticlib', 'proc-macro'):
                kind = 'lib'
            key = (package['id'], kind, target['name'])
            if key in rows:
                raise ValueError('Duplicate Cargo test target: ' + repr(key))
            rows[key] = {'package': package['name'], 'target': target['name'], 'kind': kind,
                         'required_features': target.get('required-features', []),
                         'registered': set(), 'expected': set(), 'passed': set(),
                         'failed': set(), 'skipped': set(), 'missing': set(),
                         'discovered': False, 'execution_evidence': False, 'problems': []}
    for run, folder in runs:
        touched = []
        try:
            if run.get('discovery_exit') != 0:
                for key, row in rows.items():
                    if key[0] in run['packages']:
                        row['problems'].append(run['phase'] + ': discovery/build incomplete')
                continue
            data = json.loads((folder / 'list.json').read_text())
            binaries, registered, expected, _ = listed_tests(data, names, run['packages'])
            target_keys = {}
            for binary, suite in data['rust-suites'].items():
                kind = suite['kind']
                if kind in ('proc-macro', 'rlib', 'dylib', 'cdylib', 'staticlib'):
                    kind = 'lib'
                key = (suite['package-id'], kind, suite['binary-name'])
                if key not in rows:
                    raise ValueError('Listed binary has no Cargo test target: ' + binary)
                target_keys[binary] = key
                touched.append(key)
            for key in touched:
                rows[key]['discovered'] = True
                if run.get('run_exit') != 0:
                    rows[key]['problems'].append(run['phase'] + ': execution phase failed or incomplete')
            for field, values in [('registered', registered), ('expected', expected)]:
                for identity in values:
                    rows[target_keys[identity[0]]][field].add(identity)
            report = folder / 'junit.xml'
            if not report.exists():
                for key in touched:
                    rows[key]['problems'].append(run['phase'] + ': JUnit unavailable')
                continue
            actual = junit_results(report, binaries, registered)
            for key in touched:
                rows[key]['execution_evidence'] = True
            for identity, outcome in actual.items():
                rows[target_keys[identity[0]]][outcome].add(identity)
            for identity in expected - actual.keys():
                rows[target_keys[identity[0]]]['missing'].add(identity)
            for identity in expected:
                if actual.get(identity) == 'skipped':
                    rows[target_keys[identity[0]]]['problems'].append('Selected test skipped')
        except (OSError, ValueError, KeyError, TypeError, ET.ParseError) as error:
            # Never erase manifest rows when a listing or result is corrupt.
            for key, row in rows.items():
                if key[0] in run.get('packages', []):
                    row['problems'].append(run.get('phase', 'unknown') + ': invalid evidence: ' + str(error))
    output = []
    for row in rows.values():
        row['passed'] -= row['failed']
        executed = row['passed'] | row['failed']
        if not row['discovered']:
            status = 'not observed: selection/build unknown'
        elif not row['registered']:
            status = 'zero registered tests'
        elif not row['execution_evidence']:
            status = 'unknown: execution unavailable'
        elif row['failed']:
            status = 'failed tests'
        elif not executed:
            status = 'zero executed tests'
        elif row['missing'] or row['problems']:
            status = 'incomplete evidence'
        else:
            status = 'passed'
        result = {key: row[key] for key in ('package', 'target', 'kind', 'required_features')}
        result.update(status=status, problems=sorted(set(row['problems'])))
        result['registered'] = len(row['registered']) if row['discovered'] else None
        for field in ('passed', 'failed', 'skipped', 'missing'):
            result[field] = len(row[field]) if row['execution_evidence'] else None
        result['run'] = len(executed) if row['execution_evidence'] else None
        output.append(result)
    return sorted(output, key=lambda row: (row['package'], row['kind'], row['target']))


def invoke(command, log, stdout=None):
    if stdout is not None:
        with stdout.open('w') as out, log.open('w') as err:
            # Keep machine-readable stdout isolated, but expose compilation
            # progress immediately instead of hiding it until discovery fails.
            with subprocess.Popen(command, stdout=out, stderr=subprocess.PIPE,
                                  text=True, errors='replace') as process:
                for line in process.stderr:
                    print(line, end='', flush=True)
                    err.write(line)
                    err.flush()
                return process.wait()
    with log.open('w') as output:
        process = subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, errors='replace')
        for line in process.stdout:
            print(line, end='', flush=True)
            output.write(line)
        return process.wait()


def init(args):
    args.output.mkdir(parents=True, exist_ok=True)
    manifest = args.output / 'manifest.json'
    if manifest.exists():
        raise ValueError('Evidence directory already initialized; choose a fresh run directory')
    metadata = args.output / 'metadata.json'
    command = ['cargo', 'metadata', '--no-deps', '--format-version', '1', '--locked']
    code = invoke(command, args.output / 'metadata.log', metadata)
    if code != 0:
        raise ValueError('Cargo metadata failed; no authoritative package inventory is available')
    members(json.loads(metadata.read_text()))
    save(manifest, {'revision': subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip(),
                    'metadata_sha256': digest(metadata), 'metadata_command': command})
    return 0


def capture(args):
    manifest = json.loads((args.output / 'manifest.json').read_text())
    if digest(args.output / 'metadata.json') != manifest['metadata_sha256']:
        raise ValueError('Cargo metadata changed after inventory initialization')
    revision = subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip()
    if revision != manifest['revision']:
        raise ValueError('Source revision changed during evidence collection')
    selection = args.selection[1:] if args.selection[:1] == ['--'] else args.selection
    for prohibited in ['--no-run', '--message-format', '--profile', '--no-tests', '--no-tests=pass', '--run-ignored']:
        if any(arg == prohibited or arg.startswith(prohibited + '=') for arg in selection):
            raise ValueError('Capture expects selection/build flags only; disallowed flag: ' + prohibited)
    names = members(json.loads((args.output / 'metadata.json').read_text()))
    folder = args.output / args.phase
    folder.mkdir()  # refuse stale or duplicate phase evidence
    listing = ['cargo', 'nextest', 'list', '--message-format', 'json', *selection]
    command = ['cargo', 'nextest', 'run', '--profile', 'ci', *selection]
    run = {'phase': args.phase, 'revision': revision, 'packages': selected_packages(names, selection),
           'list_command': listing, 'run_command': command, 'discovery_exit': None, 'run_exit': None}
    save(folder / 'run.json', run)
    run['discovery_exit'] = invoke(listing, folder / 'list.log', folder / 'list.json')
    save(folder / 'run.json', run)
    if run['discovery_exit'] != 0:
        return run['discovery_exit']
    # A prior phase's report must never be mistaken for this run's execution.
    args.junit.unlink(missing_ok=True)
    run['run_exit'] = invoke(command, folder / 'run.log')
    if args.junit.exists():
        (folder / 'junit.xml').write_bytes(args.junit.read_bytes())
    run['hashes'] = {p.name: digest(p) for p in folder.iterdir() if p.is_file() and p.name != 'run.json'}
    save(folder / 'run.json', run)
    return run['run_exit']


def report(args):
    result = {'status': 'failed', 'packages': [], 'errors': []}
    try:
        manifest = json.loads((args.output / 'manifest.json').read_text())
        metadata = args.output / 'metadata.json'
        if digest(metadata) != manifest['metadata_sha256']:
            raise ValueError('Metadata evidence digest mismatch')
        parsed_metadata = json.loads(metadata.read_text())
        # Preserve the authoritative crate rows if a later phase artifact is corrupt.
        result = summarize(parsed_metadata, [])
        result['targets'] = target_inventory(parsed_metadata, [])
        runs = []
        for path in sorted(args.output.glob('*/run.json')):
            run = json.loads(path.read_text())
            if run['revision'] != manifest['revision']:
                raise ValueError('Mixed source revisions in evidence')
            for name, value in run.get('hashes', {}).items():
                if digest(path.parent / name) != value:
                    raise ValueError('Evidence changed: ' + str(path.parent / name))
            runs.append((run, path.parent))
        result = summarize(parsed_metadata, runs)
        result['targets'] = target_inventory(parsed_metadata, runs)
        target_errors = {problem for row in result['targets'] for problem in row['problems']
                         if ': invalid evidence:' in problem}
        if target_errors:
            result['status'] = 'failed'
            result['errors'].extend(sorted(target_errors))
        result['revision'] = manifest['revision']
        result['nextest_summaries'] = []
        for run, folder in runs:
            log = folder / 'run.log'
            if log.exists():
                lines = [re.sub(r'\x1b\[[0-?]*[ -/]*[@-~]', '', line) for line in log.read_text(errors='replace').splitlines()]
                result['nextest_summaries'].append({'phase': run['phase'], 'lines': [line for line in lines if line.lstrip().startswith('Summary')]})
    except (OSError, ValueError, KeyError, TypeError, ET.ParseError) as error:
        result['errors'].append(str(error))
    args.output.mkdir(parents=True, exist_ok=True)
    save(args.output / 'summary.json', result)
    text = ['# Executed workspace test inventory', '', 'Status: **' + result['status'] + '**', '',
            '| Crate | Registered | Run | Passed | Failed | Skipped | Missing | Status |',
            '|---|---:|---:|---:|---:|---:|---:|---|']
    for row in result['packages']:
        text.append('| ' + ' | '.join(('unknown' if row[k] is None else str(row[k])) for k in ('package','registered','run','passed','failed','skipped','missing','status')) + ' |')
    text.extend(['', '## Declared test targets', '',
                 'Unobserved targets remain visible; optional features and target selection can prevent discovery. Zero-test targets are reported explicitly. This table does not change the existing per-crate gate.', '',
                 '| Crate | Target | Kind | Run | Passed | Failed | Status | Required features |',
                 '|---|---|---|---:|---:|---:|---|---|'])
    for row in result.get('targets', []):
        values = [row[key] for key in ('package', 'target', 'kind', 'run', 'passed', 'failed', 'status')]
        values.append(', '.join(row['required_features']))
        text.append('| ' + ' | '.join('unknown' if value is None else str(value).replace('|', '&#124;') for value in values) + ' |')
    text.extend(['', 'Counts come from nextest list/JUnit, not source markers. Doctests remain separate.', ''])
    text.extend('- ' + error for error in result['errors'])
    for phase in result.get('nextest_summaries', []):
        text.extend(['', 'Recorded nextest summary: ' + phase['phase'], '```text', '\n'.join(phase['lines']) or '(no summary line recorded)', '```'])
    summary = '\n'.join(text) + '\n'
    (args.output / 'summary.md').write_text(summary)
    print(summary)
    if os.environ.get('GITHUB_STEP_SUMMARY'):
        with open(os.environ['GITHUB_STEP_SUMMARY'], 'a') as output:
            output.write(summary)
    return 0 if result['status'] == 'passed' else 1


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, required=True)
    commands = parser.add_subparsers(dest='command', required=True)
    commands.add_parser('init')
    commands.add_parser('report')
    run = commands.add_parser('capture')
    run.add_argument('--phase', required=True)
    run.add_argument('--junit', type=Path, default=Path('target/nextest/ci/junit.xml'))
    run.add_argument('selection', nargs=argparse.REMAINDER)
    args = parser.parse_args()
    if getattr(args, 'phase', '').startswith('.') or '/' in getattr(args, 'phase', ''):
        parser.error('phase must be a simple directory name')
    try:
        return {'init': init, 'capture': capture, 'report': report}[args.command](args)
    except (OSError, ValueError, KeyError) as error:
        print('Test inventory failed: ' + str(error), file=sys.stderr)
        return 1


if __name__ == '__main__':
    raise SystemExit(main())
