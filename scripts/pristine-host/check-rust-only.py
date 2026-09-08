#!/usr/bin/env python3
"""Host-side observation; Python and just never enter the measured image."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import time
import uuid


def observe(image, source, out):
    out.mkdir(parents=True, exist_ok=True)
    name = 'omp-rust-only-' + uuid.uuid4().hex
    command = ['docker', 'run', '--rm', '--network', 'none', '--name', name,
               '--mount', f'type=bind,src={source},dst=/source,readonly', image]
    start = time.monotonic()
    violations = []
    try:
        result = subprocess.run(command, capture_output=True, timeout=3)
        code, stdout, stderr = result.returncode, result.stdout, result.stderr
    except subprocess.TimeoutExpired as error:
        code, stdout, stderr = None, error.stdout or b'', error.stderr or b''
        violations.append('container invocation exceeded three seconds')
    elapsed = time.monotonic() - start
    # A timed-out Docker client can leave its container behind. Remove only the
    # uniquely named container created by this observation, never other jobs.
    cleanup = subprocess.run(['docker', 'rm', '-f', name], capture_output=True, timeout=15)
    remaining = subprocess.run(['docker', 'container', 'inspect', name], capture_output=True, timeout=15)
    if remaining.returncode == 0:
        violations.append('owned container survived cleanup')
    (out / 'doctor.stdout').write_bytes(stdout)
    (out / 'doctor.stderr').write_bytes(stderr)
    text = stdout.decode(errors='replace')
    if code != 1:
        violations.append(f'expected diagnostic exit 1, observed {code}')
    if elapsed >= 3:
        violations.append(f'invocation took {elapsed:.6f}s, required <3s')
    for tool in ('python3', 'just', 'cmake', 'ninja'):
        line = next((line for line in text.splitlines() if line.startswith(f'FAIL {tool}:')), '')
        if 'install' not in line.lower():
            violations.append(f'missing actionable {tool} diagnostic')
    report = {'command': command, 'elapsed_seconds': elapsed, 'raw_exit': code,
              'violations': violations, 'cleanup_exit': cleanup.returncode,
              'container_absent': remaining.returncode != 0}
    (out / 'result.json').write_text(json.dumps(report, indent=2) + '\n')
    return report


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--image', required=True)
    parser.add_argument('--source', required=True, type=Path)
    parser.add_argument('--out', required=True, type=Path)
    args = parser.parse_args()
    source, out = args.source.resolve(), args.out.resolve()
    out.mkdir(parents=True, exist_ok=True)
    assert not (source / '.git').exists() and not (source / 'target').exists(), 'Use an archived clean source tree'
    probe = '''set -eu
dpkg-query -W
for tool in python3 just cc c++ cmake ninja uv cargo-nextest; do
  if command -v "$tool"; then echo "unexpected prerequisite: $tool"; exit 1; fi
done
rustc --version --verbose
rustup show
'''
    inventory = subprocess.run(['docker', 'run', '--rm', '--network', 'none',
        '--mount', f'type=bind,src={source},dst=/source,readonly',
        '--entrypoint', '/bin/sh', args.image, '-c', probe], capture_output=True, timeout=60)
    (out / 'inventory.log').write_bytes(inventory.stdout + inventory.stderr)
    assert inventory.returncode == 0, 'Rust-only image inventory failed'
    with (out / 'image.json').open('wb') as image_record:
        subprocess.run(['docker', 'image', 'inspect', args.image], stdout=image_record, check=True, timeout=30)
    files = ('scripts/build-doctor.sh', 'scripts/build-doctor.py', 'rust-toolchain.toml', 'justfile')
    (out / 'source-files.json').write_text(json.dumps({name: hashlib.sha256((source / name).read_bytes()).hexdigest() for name in files}, indent=2))
    normal = observe(args.image, source, out / 'normal')
    with tempfile.TemporaryDirectory(prefix='omp-doctor-negative-') as temporary:
        mutant = Path(temporary) / 'source'
        shutil.copytree(source, mutant)
        (mutant / 'scripts/build-doctor.sh').write_text('#!/bin/sh\nexit 0\n')
        negative = observe(args.image, mutant, out / 'disabled-doctor')
    negative_ok = negative['raw_exit'] == 0 and bool(negative['violations']) and negative['container_absent']
    passed = not normal['violations'] and negative_ok
    summary = ('## Rust-only Linux bootstrap diagnosis\n\n'
        'Fresh Debian base plus the pinned Rust toolchain only; no Python, just, C/C++ compiler, CMake, or Ninja. '
        'Python observes from the CI host, outside the measured container. This is not a pristine macOS VM.\n\n'
        '| Observation | Result |\n|---|---|\n'
        f'| Actual shell doctor exit | {normal["raw_exit"]} |\n'
        f'| Wall time including container startup | {normal["elapsed_seconds"]:.6f}s |\n'
        f'| Disabled doctor rejected | {negative_ok} |\n'
        f'| Proof | {"PASS" if passed else "FAIL"} |\n\n'
        '```text\n' + (out / 'normal/doctor.stdout').read_text() + '\n```\n')
    (out / 'summary.md').write_text(summary)
    if os.environ.get('GITHUB_STEP_SUMMARY'):
        with open(os.environ['GITHUB_STEP_SUMMARY'], 'a') as handle:
            handle.write(summary)
    assert passed, 'Rust-only bootstrap proof failed; see retained reports'


if __name__ == '__main__':
    main()
