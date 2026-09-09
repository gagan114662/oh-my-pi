#!/usr/bin/env python3
"""Run the unchanged product invariant against real and deliberately broken rosters."""
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[2]
OUT = ROOT / 'target/settings-roster'
SOURCE = ROOT / 'crates/chat/src/overlays/settings.rs'
MARKER = 'Deliberately Unbound Hosted Group'


def sha(data):
    return hashlib.sha256(data).hexdigest()


def git(*args):
    return subprocess.check_output(['git', *args], cwd=ROOT, text=True).strip()


def replace_source(data):
    # Preserve the original on a short write or ENOSPC.
    with tempfile.NamedTemporaryFile(dir=SOURCE.parent, delete=False) as temporary:
        path = Path(temporary.name)
        try:
            temporary.write(data)
            temporary.flush()
            os.fsync(temporary.fileno())
            os.replace(path, SOURCE)
        finally:
            path.unlink(missing_ok=True)


def execute(arm):
    output = OUT / arm
    output.mkdir(parents=True, exist_ok=False)
    env = dict(os.environ, OMP_SETTINGS_ROSTER_EVIDENCE_DIR=str(output))
    command = ['just', '--command', 'cargo', 'nextest', 'run', '--locked', '--profile', 'ci', '--no-fail-fast',
               '-p', 'omp-app', '--test', 'settings_roster']
    with (output / 'run.log').open('w') as log:
        result = subprocess.run(command, cwd=ROOT, env=env, stdout=log, stderr=subprocess.STDOUT)
    report = output / 'settings-roster.json'
    rows = json.loads(report.read_text()) if report.is_file() else None
    return {'command': command, 'exit': result.returncode, 'rows': rows,
            'source_sha256': sha(SOURCE.read_bytes()), 'status': git('status', '--porcelain=v1')}


def main():
    OUT.mkdir(parents=True, exist_ok=True)
    original = SOURCE.read_bytes()
    text = original.decode()
    anchor = 'groups: &["Theme",'
    if text.count(anchor) != 1 or MARKER in text:
        raise ValueError('Cannot identify the actual Appearance TabSpec safely')
    mutated = text.replace(anchor, f'groups: &["{MARKER}", "Theme",', 1).encode()
    report = {
        'source_revision': git('rev-parse', 'HEAD'),
        'source_status_before': git('status', '--porcelain=v1'),
        'checker_sha256': sha(Path(__file__).read_bytes()),
        'test_sha256': sha((ROOT / 'crates/app/tests/settings_roster.rs').read_bytes()),
        'workflow_sha256': sha((ROOT / '.github/workflows/settings-roster.yml').read_bytes()),
        'roster_before_sha256': sha(original), 'roster_mutated_sha256': sha(mutated),
        'scope': 'Identical product invariant; only the actual SETTING_TABS source gains one unbound group. No UI or real-PTY execution claim.',
        'arms': {},
    }
    passed = False
    try:
        report['arms']['normal'] = execute('normal')
        replace_source(mutated)
        (OUT / 'mutation.diff').write_text(git('diff', '--', str(SOURCE.relative_to(ROOT))) + '\n')
        report['arms']['mutated'] = execute('mutated')
        normal, broken = report['arms']['normal'], report['arms']['mutated']
        normal_complete = bool(normal['rows']) and all(
            row['advertised'] and row['convars'] and not row['unrenderable'] for row in normal['rows'])
        named = [row for row in broken['rows'] or [] if row['group'] == MARKER]
        log = (OUT / 'mutated/run.log').read_text()
        passed = (normal['exit'] == 0 and normal_complete and broken['exit'] != 0
                  and len(named) == 1 and named[0]['advertised']
                  and not named[0]['convars'] and not named[0]['unrenderable']
                  and 'settings declarations diverge from the actual UI roster' in log
                  and MARKER in log)
    finally:
        replace_source(original)
        report['restored_sha256'] = sha(SOURCE.read_bytes())
        report['source_status_after_restore'] = git('status', '--porcelain=v1')
        report['passed'] = passed
        (OUT / 'proof.json').write_text(json.dumps(report, indent=2) + '\n')
        lines = ['## Settings roster structural proof', '', report['scope'], '',
                 '| Arm | Test exit | Enumeration present |', '|---|---|---|']
        for arm in ('normal', 'mutated'):
            item = report['arms'].get(arm)
            lines.append(f"| {arm} | {item['exit'] if item else 'missing'} | {bool(item and item['rows'])} |")
            table = OUT / arm / 'settings-roster.md'
            if table.exists():
                lines.extend(['', f'### {arm}', '', table.read_text()])
        lines += ['', f'Invariant proof passed: {passed}', '',
                  'Counts are tab/group pairs; General occurs under both Context and Memory. Before cleanup: 57 pairs / 56 unique names. After cleanup: 44 pairs / 43 unique names (13 removed, Commands & Skills bound).',
                  'Real PTY input, resize and clean-quit evidence remains required for full issue acceptance.']
        (OUT / 'summary.md').write_text('\n'.join(lines) + '\n')
    return 0 if passed else 1


if __name__ == '__main__':
    sys.exit(main())
