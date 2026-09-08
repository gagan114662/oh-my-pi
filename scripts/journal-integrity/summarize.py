#!/usr/bin/env python3
"""Read actual CLI fixture artifacts; missing or inconsistent proof is a failure."""
import csv
import hashlib
import json
import os
from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parents[2]
OUT = ROOT / 'target/journal-integrity'
FIXTURE = OUT / 'fixture'


def main():
    OUT.mkdir(parents=True, exist_ok=True)
    rows, failures = [], []
    def load(path):
        try:
            return json.loads(path.read_text())
        except (OSError, ValueError) as error:
            failures.append(f'{path.relative_to(OUT)}: {type(error).__name__}')
            return {}
    oracle = load(FIXTURE / 'expected/oracle.json')
    for stage, expected in [('valid', 'verified'), ('tampered', 'invalid'),
                            ('legacy_unsealed', 'legacy_unsealed'), ('torn_tail', 'torn_tail'),
                            ('empty', 'empty'), ('expected-tip-mismatch', 'invalid')]:
        result = load(FIXTURE / stage / 'stdout.txt')
        exit_result = load(FIXTURE / stage / 'exit.json')
        passed = (result.get('status') == expected and exit_result.get('reaped') is True
                  and exit_result.get('success') is (stage == 'valid')
                  and exit_result.get('code') == (0 if stage == 'valid' else 1))
        if stage == 'tampered':
            passed &= bool(oracle) and result.get('first_bad_entry_id') == oracle.get('id') and result.get('first_bad_byte_offset') == oracle.get('offset')
        if stage == 'expected-tip-mismatch':
            passed &= 'externally expected tip mismatch' in (result.get('diagnostic') or '')
        if stage == 'torn_tail':
            passed &= result.get('verification', {}).get('torn_tail_bytes', 0) > 0
        rows.append((stage, expected, result.get('status', 'missing'), passed))
        if not passed:
            failures.append(stage)
    refusal = load(FIXTURE / 'tampered/session-open.json')
    rows.append(('Session::open', 'refuse before fold', refusal.get('refused'), refusal.get('refused') is True))
    if refusal.get('refused') is not True:
        failures.append('Session::open refusal')
    try:
        original = (FIXTURE / 'valid/journal.oms').read_bytes()
        tampered = (FIXTURE / 'tampered/journal.oms').read_bytes()
        changed = [index for index, (a, b) in enumerate(zip(original, tampered)) if a != b]
        exact_edit = len(original) == len(tampered) and len(changed) == 1 and changed[0] >= oracle['offset']
        rows.append(('byte edit', 'one byte inside divergent frame', changed, exact_edit))
        if not exact_edit:
            failures.append('byte edit is not the declared mutation')
    except (OSError, KeyError) as error:
        failures.append(f'byte mutation proof: {type(error).__name__}')
    exit_file = OUT / 'exits.csv'
    observed = {row[0]: row[1:] for row in csv.reader(exit_file.open())} if exit_file.exists() else {}
    for check in ['proof'] + [f'{package}-{mode}' for package in ['omp-app', 'omp-journal', 'omp-session'] for mode in ['tests', 'docs']]:
        passed = observed.get(check) == ['0', '0']
        rows.append((check, 'command and log exit 0', observed.get(check, 'missing'), passed))
        if not passed:
            failures.append(check)
    provenance = {'revision': subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip(),
                  'source_status': subprocess.check_output(['git', 'status', '--porcelain=v1'], cwd=ROOT, text=True),
                  'hashes': {str(p.relative_to(ROOT)): hashlib.sha256(p.read_bytes()).hexdigest() for p in
                             [Path(__file__), ROOT / 'crates/app/tests/session_verify.rs', ROOT / '.github/workflows/journal-integrity.yml']}}
    binary = ROOT / 'target/debug/omp'
    if binary.is_file():
        provenance['binary_sha256'] = hashlib.sha256(binary.read_bytes()).hexdigest()
    (OUT / 'provenance.json').write_text(json.dumps(provenance, indent=2) + '\n')
    (OUT / 'results.json').write_text(json.dumps({'rows': rows, 'failures': failures, 'oracle': oracle}, indent=2) + '\n')
    text = ['## Journal integrity proof', '', '| Check | Expected | Observed | Pass |', '|---|---|---|---|']
    text += ['| ' + ' | '.join(str(value).replace('|', '&#124;').replace('\n', ' ') for value in row) + ' |' for row in rows]
    text += ['', f'Expected first divergent entry: `{oracle.get("id", "missing")}`, frame byte offset: `{oracle.get("offset", "missing")}`.',
             '', 'Artifacts contain original and edited journals plus exact CLI stdout/stderr. These are a deterministic production Session/CLI fixture, not real-model evidence.',
             '', 'Compaction, lift and import operation-specific browser proofs and explicit legacy migration remain pending. The full suite logs retain skipped-test counts; a successful helper test is not an independent model run.',
             '', *[f'FAIL: {failure}' for failure in failures]]
    summary = '\n'.join(text) + '\n'
    (OUT / 'summary.md').write_text(summary)
    if os.environ.get('GITHUB_STEP_SUMMARY'):
        with open(os.environ['GITHUB_STEP_SUMMARY'], 'a') as output:
            output.write(summary)
    return bool(failures)


if __name__ == '__main__':
    raise SystemExit(main())
