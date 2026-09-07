#!/usr/bin/env python3
"""Exercise missing-tool diagnostics against an isolated executable PATH."""
import importlib.util
from pathlib import Path
import tempfile
import time
import unittest

spec = importlib.util.spec_from_file_location('doctor', Path(__file__).with_name('build-doctor.py'))
doctor = importlib.util.module_from_spec(spec)
spec.loader.exec_module(doctor)


class DoctorTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.bin = self.root / 'bin'
        self.bin.mkdir()
        (self.root / '.cargo').mkdir()
        (self.root / '.cargo/config.toml').write_text('[env]\nCMAKE_POLICY_VERSION_MINIMUM = "3.5"\n')
        (self.root / 'rust-toolchain.toml').write_text('[toolchain]\nchannel = "nightly-2026-08-08"\ncomponents = ["clippy", "rustc-codegen-cranelift-preview"]\n')
        requirements = self.root / 'crates/py/requirements.txt'
        requirements.parent.mkdir(parents=True)
        requirements.write_text('cloudpickle==3.1.2\n')
        vendor = self.root / 'vendor/python'
        (vendor / 'bundled').mkdir(parents=True)
        for name in ['pyo3-config.txt', 'PYTHON.json', 'stdlib.bin']:
            (vendor / name).touch()
        (vendor / 'bundled/.requirements.stamp').write_bytes(requirements.read_bytes())

    def command(self, name, text=''):
        p = self.bin / name
        p.write_text('#!/bin/sh\nprintf "%s\\n" "' + text + '"\n')
        p.chmod(0o755)

    def provision(self):
        for name in ['cargo', 'rustup', 'just', 'cc', 'c++', 'cmake', 'ninja', 'uv', 'cargo-nextest', 'pkg-config']:
            self.command(name)
        self.command('cmake', 'cmake version 4.1.0')
        self.command('rustup', 'nightly-2026-08-08-aarch64-apple-darwin (default)\nclippy-aarch64-apple-darwin\nrustc-codegen-cranelift-aarch64-apple-darwin')

    def results(self, host='x86_64-linux', **kwargs):
        return {name: (ok, remedy) for name, ok, remedy in doctor.inspect(
            self.root, host, str(self.bin), environ={}, **kwargs)}

    def test_empty_path_reports_all_missing_tools_without_build(self):
        start = time.monotonic()
        result = self.results('arm64-darwin', executable=lambda _: False)
        for name in ['cmake', 'ninja', 'configured Apple Silicon linker', 'cargo-nextest']:
            self.assertFalse(result[name][0])
            self.assertIn('install', result[name][1].lower())
        self.assertLess(time.monotonic() - start, 3)

    def test_complete_profile_and_each_missing_native_tool(self):
        self.provision()
        self.assertTrue(all(ok for ok, _ in self.results().values()))
        for name in ['cmake', 'ninja', 'cc', 'c++', 'pkg-config', 'cargo-nextest']:
            with self.subTest(name=name):
                p = self.bin / name
                saved = p.read_text()
                p.unlink()
                self.assertFalse(self.results()[name][0])
                p.write_text(saved)
                p.chmod(0o755)

    def test_apple_linker_uses_configured_absolute_path(self):
        self.provision()
        seen = []
        result = self.results('arm64-darwin', executable=lambda p: seen.append(p) or False)
        self.assertEqual(seen, ['/opt/homebrew/bin/ld64.lld'])
        self.assertFalse(result['configured Apple Silicon linker'][0])
        self.assertNotIn('configured Apple Silicon linker', self.results())

    def test_old_cmake_and_wrong_toolchain(self):
        self.provision()
        self.command('cmake', 'cmake version 3.4.0')
        self.command('rustup', 'stable-x86_64-unknown-linux-gnu')
        result = self.results()
        self.assertFalse(result['CMake version'][0])
        self.assertFalse(result['pinned toolchain nightly-2026-08-08'][0])
        self.assertFalse(result['pinned toolchain components'][0])

    def test_missing_bundle_and_stale_requirements(self):
        self.provision()
        (self.root / 'vendor/python/stdlib.bin').unlink()
        (self.root / 'vendor/python/bundled/.requirements.stamp').write_text('stale')
        result = self.results()
        self.assertFalse(result['embedded Python build inputs'][0])
        self.assertFalse(result['bundled Python requirements'][0])
        self.assertIn('just setup-python', result['embedded Python build inputs'][1])

    def test_environment_cannot_silently_lower_policy(self):
        self.provision()
        result = {name: ok for name, ok, _ in doctor.inspect(
            self.root, 'x86_64-linux', str(self.bin),
            environ={'CMAKE_POLICY_VERSION_MINIMUM': '3.4'})}
        self.assertFalse(result['vendored Opus CMake policy'])

    def test_missing_policy_fails_even_with_cmake_four(self):
        self.provision()
        (self.root / '.cargo/config.toml').write_text('[env]\n')
        self.assertFalse(self.results()['vendored Opus CMake policy'][0])


if __name__ == '__main__':
    unittest.main()
