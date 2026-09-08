#!/usr/bin/env python3
"""Transplant the exact new fixture; only the expected runtime assertion is a negative proof."""
import argparse
import csv
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import time
import xml.etree.ElementTree as ET

HERE = Path(__file__).resolve().parent
MANIFEST = json.loads((HERE / 'fixture.json').read_text())
RELATIVE = 'crates/tools/src/eval/kernel.rs'
ANCHOR = b'\t#[tokio::test]\n\tasync fn timeout_and_dropped_run_leave_the_worker_available_for_the_next_cell()'


def sha(data):
    return hashlib.sha256(data).hexdigest()


def git(root, *args):
    return subprocess.check_output(['git', *args], cwd=root, text=True).strip()


def prepare(root, output, arm):
    output.mkdir(parents=True, exist_ok=False)
    fixture = (HERE / 'fixture.rs').read_bytes()
    assert sha(fixture) == MANIFEST['fixture_sha256'], 'frozen fixture changed'
    revision = git(root, 'rev-parse', 'HEAD')
    assert revision == MANIFEST[arm], 'source revision does not match fixed proof arm'
    assert not git(root, 'status', '--porcelain=v1'), 'source must begin clean'
    source = root / RELATIVE
    original = source.read_bytes()
    if arm == 'head':
        assert original.count(fixture) == 1, 'head must contain the identical existing fixture'
        changed = original
    else:
        assert b'struct DeadlineHold ' not in original and original.count(ANCHOR) == 1
        changed = original.replace(ANCHOR, fixture + ANCHOR, 1)
        source.write_bytes(changed)
    (output / 'source.diff').write_text(git(root, 'diff', '--', RELATIVE) + '\n')
    report = {'arm': arm, 'source_revision': revision,
              'checker_revision': git(HERE, 'rev-parse', 'HEAD'),
              'checker_sha256': sha(Path(__file__).read_bytes()),
              'fixture_sha256': sha(fixture), 'original_source_sha256': sha(original),
              'tested_source_sha256': sha(changed), 'test': MANIFEST['test'],
              'scope': 'parent differs from its committed source only by the exact head fixture insertion; head source unchanged'}
    (output / 'provenance.json').write_text(json.dumps(report, indent=2) + '\n')


def execute(root, output, arm):
    report = json.loads((output / 'provenance.json').read_text())
    assert sha((root / RELATIVE).read_bytes()) == report['tested_source_sha256']
    changed_paths = git(root, 'diff', '--name-only').splitlines()
    assert changed_paths == ([RELATIVE] if arm == 'parent' else []), 'unexpected tracked source change'
    junit = root / 'target/nextest/ci/junit.xml'
    junit.unlink(missing_ok=True)
    command = ['just', '--command', 'cargo', 'nextest', 'run', '--locked', '--profile', 'ci',
               '--no-fail-fast', '-p', 'omp-tools', '--lib', '-E', f'test(={MANIFEST["test"]})']
    started = time.monotonic()
    with (output / 'test.log').open('w') as log:
        result = subprocess.run(command, cwd=root, stdout=log, stderr=subprocess.STDOUT)
    report.update(command=command, command_exit=result.returncode, command_seconds=time.monotonic()-started)
    if junit.exists():
        shutil.copyfile(junit, output / 'junit.xml')
    # Preserve raw result even if XML parsing or a semantic assertion below fails.
    (output / 'execution.json').write_text(json.dumps(report, indent=2) + '\n')
    return 0


def summarize(root, output, arm):
    output.mkdir(parents=True, exist_ok=True)
    failures = []
    report_file = output / 'execution.json'
    report = json.loads(report_file.read_text()) if report_file.exists() else {}
    cases = []
    try:
        cases = [case for case in ET.parse(output / 'junit.xml').iter('testcase')
                 if MANIFEST['test'] in case.get('name', '')]
    except (OSError, ET.ParseError):
        failures.append('Fresh executable-test JUnit evidence missing; build failure is not negative proof')
    case = cases[0] if len(cases) == 1 else None
    elapsed = float(case.get('time', '0')) if case is not None else 0
    failure_nodes = list(case.findall('failure')) if case is not None else []
    error_nodes = list(case.findall('error')) if case is not None else []
    skipped = list(case.findall('skipped')) if case is not None else []
    log_path = output / 'test.log'
    log = log_path.read_text() if log_path.exists() else ''
    expected_assertion = bool(re.search(r'left:\s*Timeout\s+right:\s*Cancelled', log))
    if arm == 'head':
        valid = report.get('command_exit') == 0 and case is not None and not (failure_nodes or error_nodes or skipped) and elapsed >= 4
        expectation = 'both real two-second orders preserve first reason; test passes'
    else:
        valid = report.get('command_exit', 0) != 0 and case is not None and bool(failure_nodes) and not (error_nodes or skipped) and expected_assertion and elapsed >= 2
        expectation = 'real watchdog relabels prior cancellation as Timeout; exact Cancelled assertion fails'
    if not valid:
        failures.append('Required semantic result or real elapsed lower bound not observed')
    rows = [('terminal-order fixture', expectation, f'exit={report.get("command_exit", "missing")}; test_seconds={elapsed}; exact_assertion={expected_assertion}', valid)]
    if arm == 'head':
        exits_file = output / 'exits.csv'
        exits = {row[0]: row[1:] for row in csv.reader(exits_file.open())} if exits_file.exists() else {}
        for package in ['omp-tools', 'omp-con']:
            for mode in ['tests', 'docs']:
                key = f'{package}-{mode}'
                passed = exits.get(key) == ['0', '0']
                rows.append((key, 'command and log exit zero', exits.get(key, 'missing'), passed))
                if not passed:
                    failures.append(key)
    provenance = output / 'provenance.json'
    details = json.loads(provenance.read_text()) if provenance.exists() else {}
    result = {'arm': arm, 'rows': rows, 'failures': failures, 'provenance': details}
    (output / 'result.json').write_text(json.dumps(result, indent=2) + '\n')
    lines = [f'## Eval terminal precedence: {arm}', '', f'Source: `{details.get("source_revision", "missing")}`; checker: `{details.get("checker_revision", "missing")}`.',
             f'Frozen fixture SHA-256: `{MANIFEST["fixture_sha256"]}`.', '', '| Check | Expected | Observed | Pass |', '|---|---|---|---|']
    lines += ['| ' + ' | '.join(str(value).replace('|', '&#124;').replace('\n', ' ') for value in row) + ' |' for row in rows]
    lines += ['', 'Test duration comes from fresh nextest JUnit, excluding compilation. Parent timeout/hang/build failure is not accepted. No retries or model calls. Full-suite logs preserve ignored/skipped counts.', '', *[f'FAIL: {failure}' for failure in failures]]
    text = '\n'.join(lines) + '\n'
    (output / 'summary.md').write_text(text)
    if os.environ.get('GITHUB_STEP_SUMMARY'):
        with open(os.environ['GITHUB_STEP_SUMMARY'], 'a') as summary:
            summary.write(text)
    return bool(failures)


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('phase', choices=['prepare', 'execute', 'summarize'])
    parser.add_argument('--root', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--arm', choices=['head', 'parent'], required=True)
    args = parser.parse_args()
    operation = {'prepare': prepare, 'execute': execute, 'summarize': summarize}[args.phase]
    raise SystemExit(operation(args.root.resolve(), args.output.resolve(), args.arm))
