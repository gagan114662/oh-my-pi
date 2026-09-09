#!/usr/bin/env python3
"""Observe actual just doctor in a fresh container; no PATH or host mocking."""
import hashlib
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import time

OUTPUT = Path('/evidence')
SOURCE = Path('/source')
OUTPUT.mkdir(parents=True, exist_ok=True)
missing = ('cc', 'c++', 'cmake', 'ninja', 'uv', 'cargo-nextest', 'pkg-config')
commands = {name: shutil.which(name) for name in (*missing, 'cargo', 'rustup', 'rustc', 'just', 'python3')}
(OUTPUT / 'executables.json').write_text(json.dumps(commands, indent=2))
(OUTPUT / 'environment.json').write_text(json.dumps(dict(os.environ), indent=2))
(OUTPUT / 'platform.json').write_text(json.dumps({'system': platform.system(), 'machine': platform.machine()}, indent=2))
for name, command in [('packages', ['dpkg-query', '-W']), ('rustup', ['rustup', 'show']),
                      ('rustc', ['rustc', '--version', '--verbose']), ('just', ['just', '--version'])]:
    result = subprocess.run(command, capture_output=True, text=True, timeout=30)
    (OUTPUT / f'{name}.txt').write_text(result.stdout + result.stderr)
    if result.returncode:
        raise SystemExit(f'inventory failed: {command}: {result.returncode}')
for filename in ('justfile', 'scripts/build-doctor.py', 'rust-toolchain.toml', '.cargo/config.toml'):
    with (OUTPUT / 'source-files.sha256').open('a') as hashes:
        hashes.write(hashlib.sha256((SOURCE / filename).read_bytes()).hexdigest() + '  ' + filename + '\n')
violations = [f'container unexpectedly contains {name}: {commands[name]}' for name in missing if commands[name]]
violations += [f'launcher prerequisite absent: {name}' for name in ('cargo', 'rustup', 'rustc', 'just', 'python3') if not commands[name]]
start = time.monotonic()
try:
    result = subprocess.run(['just', 'doctor'], cwd=SOURCE, capture_output=True, text=True, timeout=3)
    code, stdout, stderr = result.returncode, result.stdout, result.stderr
except subprocess.TimeoutExpired as error:
    code = None
    stdout = (error.stdout or b'').decode(errors='replace')
    stderr = (error.stderr or b'').decode(errors='replace')
    violations.append('just doctor exceeded the three-second wall-clock limit')
elapsed = time.monotonic() - start
(OUTPUT / 'doctor.stdout').write_text(stdout)
(OUTPUT / 'doctor.stderr').write_text(stderr)
if code != 1:
    violations.append(f'expected diagnostic failure exit 1, observed {code}')
if elapsed >= 3:
    violations.append(f'diagnostic took {elapsed:.6f}s, required <3s')
for name in ('cmake', 'ninja'):
    line = next((line for line in stdout.splitlines() if line.startswith('FAIL ' + name + ':')), '')
    if not line or 'install' not in line.lower():
        violations.append(f'missing actionable real {name} diagnostic')
if not any(line.startswith('OK   pinned toolchain nightly-') for line in stdout.splitlines()):
    violations.append('pinned toolchain was not recognized by doctor')
if 'OK   pinned toolchain components' not in stdout:
    violations.append('pinned toolchain components were not recognized by doctor')
report = {'elapsed_seconds': elapsed, 'exit_code': code, 'violations': violations,
          'scope': 'Fresh Linux container with pinned Rust plus just/Python diagnostic launchers. Normal PATH; no native build prerequisites. Not pristine macOS or literally Rust-only.'}
(OUTPUT / 'result.json').write_text(json.dumps(report, indent=2))
(OUTPUT / 'summary.md').write_text('## Minimal-container prerequisite diagnosis\n\n' + report['scope'] +
    '\n\n| Observation | Result |\n|---|---|\n' + f'| Actual `just doctor` exit | {code} |\n| Elapsed wall time | {elapsed:.6f}s |\n' +
    f'| Proof validation | {"FAIL" if violations else "PASS"} |\n\n```text\n' + stdout + stderr + '\n```\n' +
    '\n'.join('FAIL: ' + failure for failure in violations) + '\n\nLinux does not use the configured Apple Silicon linker; missing macOS lld is not proved by this container.\n')
if violations:
    raise SystemExit('\n'.join(violations))
