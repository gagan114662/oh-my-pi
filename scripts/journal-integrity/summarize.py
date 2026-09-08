#!/usr/bin/env python3
"""Read actual CLI fixture artifacts; missing or inconsistent proof is a failure."""
import csv
import hashlib
import json
import os
import math
import xml.etree.ElementTree as ET
from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parents[2]
OUT = ROOT / 'target/journal-integrity'
FIXTURE = OUT / 'fixture'


# Exact production tests whose assertions include physical verification and
# operation-specific state/identity checks. Package success is insufficient.
OPERATIONS = [
    ('rewind', 'omp-session', 'omp-session::rewind', 'subscription_survives_rewind_and_marks_branch_prior'),
    ('branch pruning', 'omp-session', 'omp-session::rewind', 'subscription_survives_rewind_and_marks_branch_prior'),
    ('blob GC', 'omp-session', 'omp-session::replay_law', 'receipt_and_compaction_facts_materialize_and_survive_reopen'),
    ('compaction', 'omp-session', 'omp-session::replay_law', 'receipt_and_compaction_facts_materialize_and_survive_reopen'),
    ('import', 'omp-app', 'omp-app', 'session_import::tests::imported_session_records_source_selection_metadata'),
    ('lift', 'omp-e2e', 'omp-e2e::p10_lift_idempotence', 'p10_edit_lift_is_idempotent_and_dispatches_at_the_live_revision'),
]


def operation_rows(out):
    rows, failures, inventory = [], [], {}
    for operation, package, classname, name in OPERATIONS:
        path = out / f'{package}.junit.xml'
        try:
            root = ET.parse(path).getroot()
            cases = list(root.iter('testcase'))
            inventory[package] = {
                'testcases': len(cases),
                'skipped': sum(case.find('skipped') is not None for case in cases),
                'sha256': hashlib.sha256(path.read_bytes()).hexdigest(),
            }
            matching = [case for case in cases if case.get('classname') == classname and case.get('name') == name]
            if len(matching) != 1:
                raise ValueError(f'expected one exact testcase; found {len(matching)}')
            case = matching[0]
            elapsed = float(case.attrib['time'])
            bad = [child.tag for child in case if child.tag in {'failure', 'error', 'skipped', 'rerunFailure', 'rerunError', 'flakyFailure', 'flakyError'}]
            passed = not bad and math.isfinite(elapsed) and elapsed >= 0
            observed = f'{classname} / {name}: {"PASS" if passed else bad or "invalid elapsed"}; {elapsed}s ({path.name})'
        except (OSError, ET.ParseError, ValueError, KeyError) as error:
            passed = False
            observed = f'{path.name}: {error}'
        rows.append((operation, 'exact operation test passed including sealed-chain assertions', observed, passed))
        if not passed:
            failures.append(f'operation: {operation}')
    return rows, failures, inventory


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
        exact_edit = len(original) == len(tampered) and len(changed) == 1 and changed[0] == oracle['changed_byte_offset'] and oracle['offset'] <= changed[0] < oracle['frame_end']
        frame = original[oracle['offset']:oracle['frame_end']]
        actual_kind = (oracle.get('kind') == 'tool.result@1'
                       and b'event: tool.result@1\n' in frame
                       and f"by: {oracle.get('call_id')}\n".encode() in frame
                       and 0 < oracle['offset'] < oracle['frame_end'] < len(original))
        rows.append(('edited entry', 'middle tool.result@1 caused by recorded call', oracle.get('kind', 'missing'), actual_kind))
        if not actual_kind:
            failures.append('edited entry is not the declared middle tool result')
        rows.append(('byte edit', 'one byte inside divergent frame', changed, exact_edit))
        if not exact_edit:
            failures.append('byte edit is not the declared mutation')
    except (OSError, KeyError) as error:
        failures.append(f'byte mutation proof: {type(error).__name__}')
    exit_file = OUT / 'exits.csv'
    observed = {row[0]: row[1:] for row in csv.reader(exit_file.open())} if exit_file.exists() else {}
    for check in ['proof'] + [f'{package}-{mode}' for package in ['omp-app', 'omp-journal', 'omp-session', 'omp-agent', 'omp-e2e'] for mode in ['tests', 'docs', 'junit']]:
        passed = observed.get(check) == ['0', '0']
        rows.append((check, 'command and log exit 0', observed.get(check, 'missing'), passed))
        if not passed:
            failures.append(check)
    operations, operation_failures, inventory = operation_rows(OUT)
    rows.extend(operations)
    failures.extend(operation_failures)
    provenance = {'revision': subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip(),
                  'source_status': subprocess.check_output(['git', 'status', '--porcelain=v1'], cwd=ROOT, text=True),
                  'hashes': {str(p.relative_to(ROOT)): hashlib.sha256(p.read_bytes()).hexdigest() for p in
                             [Path(__file__), ROOT / 'crates/app/tests/session_verify.rs', ROOT / '.github/workflows/journal-integrity.yml', ROOT / 'crates/session/tests/rewind.rs', ROOT / 'crates/session/tests/replay_law.rs', ROOT / 'crates/app/src/session_import.rs', ROOT / 'crates/e2e/tests/p10_lift_idempotence.rs']}}
    binary = ROOT / 'target/debug/omp'
    if binary.is_file():
        provenance['binary_sha256'] = hashlib.sha256(binary.read_bytes()).hexdigest()
    (OUT / 'provenance.json').write_text(json.dumps(provenance, indent=2) + '\n')
    (OUT / 'results.json').write_text(json.dumps({'rows': rows, 'failures': failures, 'oracle': oracle, 'test_inventory': inventory}, indent=2) + '\n')
    text = ['## Journal integrity proof', '', '| Check | Expected | Observed | Pass |', '|---|---|---|---|']
    text += ['| ' + ' | '.join(str(value).replace('|', '&#124;').replace('\n', ' ') for value in row) + ' |' for row in rows]
    text += ['', f'Expected first divergent entry: `{oracle.get("id", "missing")}`, frame byte offset: `{oracle.get("offset", "missing")}`.',
             '', 'Artifacts contain original and edited journals plus exact CLI stdout/stderr. These are a deterministic production Session/CLI fixture, not real-model evidence.',
             '', 'Operation rows require exact non-skipped JUnit results, with test source hashes and raw reports retained. Rewind/pruning and compaction/blob-GC share fixtures that explicitly assert both operations. Import covers conversion, not picker selection. P10 includes actual Session admission, replay and lift, plus its original proto fixture. Explicit legacy migration remains pending.',
             '', f'Observed operation-report inventory (including skipped counts): `{json.dumps(inventory, sort_keys=True)}`.',
             '', *[f'FAIL: {failure}' for failure in failures]]
    summary = '\n'.join(text) + '\n'
    (OUT / 'summary.md').write_text(summary)
    if os.environ.get('GITHUB_STEP_SUMMARY'):
        with open(os.environ['GITHUB_STEP_SUMMARY'], 'a') as output:
            output.write(summary)
    return bool(failures)


if __name__ == '__main__':
    raise SystemExit(main())
