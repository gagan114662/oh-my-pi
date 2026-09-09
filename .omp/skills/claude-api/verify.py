#!/usr/bin/env python3
"""Check the complete pinned reference bundle without credentials or network."""
import hashlib
import json
from pathlib import Path
import sys

PIN = '41bbe19d1a1a7eaab5e7bb9050a417e5c6cffc8f'


def verify(root):
    manifest = json.loads((root / 'UPSTREAM.json').read_text())
    problems = []
    if manifest.get('revision') != PIN:
        problems.append('UPSTREAM.json: unexpected revision; restore the pinned bundle')
    expected = set()
    for entry in manifest['files']:
        relative = Path(entry['path'])
        if relative.is_absolute() or '..' in relative.parts:
            problems.append(f'UPSTREAM.json: unsafe relative path {relative}')
            continue
        expected.add(relative.as_posix())
        path = root / 'upstream' / relative
        try:
            data = path.read_bytes()
        except OSError as error:
            problems.append(f'{path}: {error}; restore this file from the complete pinned bundle')
            continue
        git_blob = hashlib.sha1(b'blob ' + str(len(data)).encode() + b'\0' + data).hexdigest()
        if (len(data) != entry['bytes'] or hashlib.sha256(data).hexdigest() != entry['sha256']
                or git_blob != entry['git_blob']):
            problems.append(f'{path}: content differs from pinned upstream; restore the complete bundle')
    actual = {path.relative_to(root / 'upstream').as_posix()
              for path in (root / 'upstream').rglob('*') if path.is_file()}
    for extra in sorted(actual - expected):
        problems.append(f'upstream/{extra}: not part of the pinned bundle')
    return {'revision': manifest.get('revision'), 'files_checked': len(expected),
            'status': 'failed' if problems else 'passed', 'problems': problems}


if __name__ == '__main__':
    try:
        report = verify(Path(__file__).resolve().parent)
    except (OSError, ValueError, KeyError, TypeError) as error:
        report = {'status': 'failed', 'problems': [f'Invalid or missing manifest: {error}; restore UPSTREAM.json']}
    print(json.dumps(report, indent=2))
    sys.exit(report['status'] != 'passed')
