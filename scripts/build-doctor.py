#!/usr/bin/env python3
"""Offline build prerequisite diagnostics; no downloads or compilation."""
import argparse
import os
from pathlib import Path
import platform
import re
import shutil
import subprocess
import sys
import time
try:
    import tomllib
except ImportError:
    sys.exit("Install Python 3.11+ to run the build doctor (tomllib is required).")


def inspect(root, host, path, environ=None, executable=None):
    """Inspect a host; PATH/host/filesystem seam allows isolated missing-tool tests."""
    environ = os.environ if environ is None else environ
    executable = executable or (lambda p: os.path.isfile(p) and os.access(p, os.X_OK))
    findings = []
    deadline = time.monotonic() + 2

    def check(name, ok, remedy):
        findings.append((name, bool(ok), remedy))

    commands = {
        'cargo': 'Install the pinned Rust toolchain with rustup.',
        'rustup': 'Install rustup, then run rustup show from this checkout.',
        'just': 'Install just (brew install just; or cargo install just --locked).',
        'cc': 'Install Xcode Command Line Tools (xcode-select --install) or a C compiler.',
        'c++': 'Install Xcode Command Line Tools or a C++ compiler.',
        'cmake': 'Install CMake 3.15 or newer (brew install cmake; Linux: cmake package).',
        'ninja': 'Install Ninja (brew install ninja; Linux: ninja-build package).',
        'uv': 'Install uv, required by just setup-python.',
        'curl': 'Install curl, required to download the embedded Python archive.',
        'zstd': 'Install zstd (brew install zstd; Linux: zstd package) for embedded Python archives.',
        'tar': 'Install tar, required to extract the embedded Python archive.',
        'cargo-nextest': 'Install cargo-nextest (cargo install cargo-nextest --locked).',
    }
    if host.endswith('linux'):
        commands['pkg-config'] = 'Install pkg-config and native development packages; see docs/building.md.'
    available = {name: shutil.which(name, path=path) for name in commands}
    for name, remedy in commands.items():
        check(name, available[name], remedy)
    if host == 'arm64-darwin':
        linker = '/opt/homebrew/opt/lld@22/bin/ld64.lld'
        check('configured Apple Silicon linker', executable(linker),
              'Run brew install lld@22; .cargo/config.toml requires ' + linker + '.')

    def probe(name, args):
        if not available[name]:
            return None
        try:
            remaining = max(0.01, deadline - time.monotonic())
            result = subprocess.run([available[name], *args], capture_output=True, text=True,
                                    timeout=remaining, env={**environ, 'PATH': path})
            return result.stdout if result.returncode == 0 else ''
        except (OSError, subprocess.TimeoutExpired):
            return ''

    version = probe('cmake', ['--version'])
    if version is not None:
        match = re.search(r'cmake version (\d+)\.(\d+)', version)
        check('CMake version', match and tuple(map(int, match.groups())) >= (3, 15),
              commands['cmake'] + ' Version probe must finish within the doctor deadline.')
    toolchain = tomllib.loads((root / 'rust-toolchain.toml').read_text())['toolchain']
    installed = probe('rustup', ['toolchain', 'list'])
    if installed is not None:
        check('pinned toolchain ' + toolchain['channel'],
              any(line.split()[0].startswith(toolchain['channel'] + '-')
                  for line in installed.splitlines() if line.split()),
              'Run rustup toolchain install ' + toolchain['channel'] +
              ' --component ' + ','.join(toolchain['components']) + '.')
    components = probe('rustup', ['component', 'list', '--installed', '--toolchain', toolchain['channel']])
    if components is not None:
        names = [line.split()[0] for line in components.splitlines() if line.split()]
        # rustup accepts the manifest's -preview alias but lists the canonical name.
        missing = [component for component in toolchain['components']
                   if not any(name == component.removesuffix('-preview') or
                              name.startswith(component.removesuffix('-preview') + '-') for name in names)]
        check('pinned toolchain components', not missing,
              'Run rustup component add --toolchain ' + toolchain['channel'] + ' ' + ' '.join(missing) + '.')
    config = tomllib.loads((root / '.cargo/config.toml').read_text())
    policy = environ.get('CMAKE_POLICY_VERSION_MINIMUM', config['env'].get('CMAKE_POLICY_VERSION_MINIMUM'))
    if isinstance(policy, dict):
        policy = policy['value']
    try:
        policy_ok = tuple(map(int, str(policy).split('.'))) >= (3, 5)
    except ValueError:
        policy_ok = False
    check('vendored Opus CMake policy', policy_ok,
          'Restore CMAKE_POLICY_VERSION_MINIMUM = "3.5" in .cargo/config.toml; remove a conflicting shell override.')
    configured_python = environ.get('PYO3_CONFIG_FILE')
    pyconfig = Path(configured_python) if configured_python else root / 'vendor/python/pyo3-config.txt'
    if not pyconfig.is_absolute():
        pyconfig = root / pyconfig
    vendor = pyconfig.parent
    required = [pyconfig, vendor / 'PYTHON.json', vendor / 'stdlib.bin']
    check('embedded Python build inputs', all(p.is_file() for p in required),
          'Run just setup-python; missing: ' + ', '.join(str(p) for p in required if not p.is_file()))
    stamp = vendor / 'bundled/.requirements.stamp'
    requirements = root / 'crates/py/requirements.txt'
    check('bundled Python requirements', stamp.is_file() and stamp.read_bytes() == requirements.read_bytes(),
          'Run just setup-python to prepare packages matching crates/py/requirements.txt.')
    return findings


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--host', default=platform.machine().lower() + '-' + sys.platform,
                        choices=['arm64-darwin', 'x86_64-darwin', 'aarch64-linux', 'x86_64-linux'])
    args = parser.parse_args()
    root = Path(__file__).resolve().parent.parent
    findings = inspect(root, args.host, os.environ.get('PATH', ''))
    print('Build prerequisites for ' + args.host + ' (offline; no compilation)')
    for name, ok, remedy in findings:
        print(('OK   ' if ok else 'FAIL ') + name + ('' if ok else ': ' + remedy))
    failed = sum(not ok for _, ok, _ in findings)
    print(f'{failed} prerequisite check(s) failed. ' +
          ('Resolve these before building.' if failed else 'Prerequisites detected; compilation and clean-host CI proof still required.'))
    return 1 if failed else 0


if __name__ == '__main__':
    sys.exit(main())
