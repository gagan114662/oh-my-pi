#!/usr/bin/env python3
"""Filesystem discovery proof; does not claim an agent followed scoped rules."""
import difflib
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess

ROOT = Path(__file__).resolve().parents[2]
OUT = ROOT / 'target/discovery-proof'
EXPECTED = {'claude', 'cursor', 'windsurf', 'gemini', 'codex', 'cline', 'github', 'vscode'}


def run(label, arguments):
    command = ['just', '--command', 'cargo', *arguments]
    with (OUT / f'{label}.log').open('wb') as log:
        result = subprocess.run(command, cwd=ROOT, stdout=log, stderr=subprocess.STDOUT)
    (OUT / f'{label}.json').write_text(json.dumps({'command': command, 'exit': result.returncode}) + '\n')
    return result.returncode


def inventory(label):
    return run(label, ['nextest', 'run', '--locked', '--profile', 'ci', '--no-fail-fast',
        '--no-tests', 'fail', '--success-output', 'immediate', '-p', 'omp-driver', '--lib',
        '-E', 'test(discovery::github::tests::documented_foreign_provider_filesystem_inventory)'])


def main():
    OUT.mkdir(parents=True, exist_ok=True)
    source = ROOT / 'crates/driver/src/discovery/rules.rs'
    fixture = ROOT / 'crates/driver/src/discovery/github.rs'
    original = source.read_bytes()
    fixture_hash = hashlib.sha256(fixture.read_bytes()).hexdigest()
    results = {}
    for package in ('omp-driver', 'omp-envd'):
        results[f'{package}-tests'] = run(f'{package}-tests', ['nextest', 'run', '--locked',
            '--profile', 'ci', '--all-targets', '--no-fail-fast', '--success-output', 'immediate', '-p', package])
        results[f'{package}-docs'] = run(f'{package}-docs', ['test', '--locked', '--doc', '--no-fail-fast', '-p', package])
    rows = {}
    for package in ('omp-driver', 'omp-envd'):
        log = re.sub(r'\x1b\[[0-9;]*m', '', (OUT / f'{package}-tests.log').read_text(errors='replace'))
        for line in log.splitlines():
            match = re.match(r'\s*\|\s*(\w+)\s*\|.*\| discovered \|', line)
            if match and match[1] in EXPECTED:
                rows[match[1]] = line.strip()
    before = inventory('inventory-before')
    needle = b'".github/instructions",'
    assert original.count(needle) == 1, 'Mutation must target only the production source path'
    changed = original.replace(needle, b'".github/disabled-instructions-negative-control",')
    (OUT / 'mutation.diff').write_text(''.join(difflib.unified_diff(original.decode().splitlines(True), changed.decode().splitlines(True), fromfile=str(source.relative_to(ROOT)), tofile='negative-control')))
    negative = None
    try:
        source.write_bytes(changed)
        negative = inventory('inventory-disabled-provider')
    finally:
        source.write_bytes(original)
    assert hashlib.sha256(fixture.read_bytes()).hexdigest() == fixture_hash, 'Fixture changed'
    assert source.read_bytes() == original, 'Production source not restored'
    restored = inventory('inventory-restored')
    negative_text = (OUT / 'inventory-disabled-provider.log').read_text(errors='replace')
    detected = negative == 100 and 'github no longer discovers .github/instructions/github.instructions.md' in negative_text
    passed = all(code == 0 for code in results.values()) and rows.keys() == EXPECTED and before == 0 and restored == 0 and detected
    report = {'full_suites': results, 'inventory_before': before, 'negative_raw_exit': negative,
        'negative_assertion_observed': detected, 'inventory_restored': restored,
        'fixture_sha256': fixture_hash, 'source_sha256': hashlib.sha256(original).hexdigest(),
        'providers': sorted(rows), 'passed': passed}
    (OUT / 'result.json').write_text(json.dumps(report, indent=2) + '\n')
    summary = '\n'.join(['## Foreign configuration discovery', '',
        'Actual filesystem fixtures; model-directed matching/nonmatching behavior remains a separate proof.', '',
        '| Provider | Capability | Evidence |', '|---|---|---|',
        *[rows.get(name, f'| {name} | unknown | MISSING |') for name in sorted(EXPECTED)], '',
        f'Disabled GitHub provider caught by unchanged test: {detected}; raw exit: {negative}.',
        f'Normal/restored inventory exits: {before}/{restored}. Overall pass: {passed}.', '',
        'Full package/doctest exits: `' + json.dumps(results) + '`', ''])
    (OUT / 'summary.md').write_text(summary)
    if os.environ.get('GITHUB_STEP_SUMMARY'):
        with open(os.environ['GITHUB_STEP_SUMMARY'], 'a') as stream:
            stream.write(summary)
    if not passed:
        raise SystemExit(1)


if __name__ == '__main__':
    main()
