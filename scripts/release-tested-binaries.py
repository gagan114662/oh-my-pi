#!/usr/bin/env python3
"""Release completed workspace test executables before linking acceptance tests."""
import argparse
import hashlib
import json
from pathlib import Path
import shutil


def release(phase, target):
    target = target.resolve(strict=True)
    record = json.loads((phase / 'run.json').read_text())
    if (record.get('phase') != 'workspace' or record.get('run_exit') != 0
            or record.get('discovery_exit') != 0):
        raise ValueError('Only a successfully completed workspace phase can be released')
    raw = (phase / 'list.json').read_bytes()
    if hashlib.sha256(raw).hexdigest() != record.get('hashes', {}).get('list.json'):
        raise ValueError('Test inventory digest mismatch')
    inventory = json.loads(raw)
    meta = inventory['rust-build-meta']
    if Path(meta['target-directory']).resolve() != target:
        raise ValueError('Inventory belongs to a different Cargo target directory')
    protected = {
        (target / item['path']).resolve()
        for items in meta.get('non-test-binaries', {}).values() for item in items
    }
    paths = set()
    # Validate the whole plan before deleting anything. Retain the evidence,
    # libraries, build scripts, application binaries, and acceptance executables.
    for suite in inventory['rust-suites'].values():
        path = Path(suite['binary-path']).resolve(strict=True)
        if (not path.is_relative_to(target) or path == target or not path.is_file()
                or path in protected or suite.get('package-name') == 'omp-e2e'):
            raise ValueError(f'Refusing unexpected test executable: {path}')
        paths.add(path)
    before = shutil.disk_usage(target).free
    total = sum(path.stat().st_size for path in paths)
    for path in paths:
        path.unlink()
    return {'removed_test_executables': len(paths), 'removed_logical_bytes': total,
            'free_bytes_before': before, 'free_bytes_after': shutil.disk_usage(target).free}


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--phase', type=Path, required=True)
    parser.add_argument('--target', type=Path, required=True)
    args = parser.parse_args()
    print(json.dumps(release(args.phase, args.target), indent=2))
