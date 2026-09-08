#!/usr/bin/env python3
"""Verify cleanup preserves evidence and refuses paths outside its build area."""
import hashlib
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

spec = importlib.util.spec_from_file_location(
    'release', Path(__file__).with_name('release-tested-binaries.py'))
release = importlib.util.module_from_spec(spec)
spec.loader.exec_module(release)


class ReleaseTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.target = self.root / 'target'
        self.target.mkdir()
        self.phase = self.target / 'evidence'
        self.phase.mkdir()
        self.binary = self.target / 'test-executable'
        self.binary.write_bytes(b'completed test')
        self.app = self.target / 'omp'
        self.app.write_bytes(b'application')
        self.library = self.target / 'libomp.rlib'
        self.library.write_bytes(b'library')

    def inventory(self, extra=None, run_exit=0):
        suites = {'unit': {'binary-path': str(self.binary), 'package-name': 'omp-chat'}}
        if extra:
            suites['extra'] = {'binary-path': str(extra), 'package-name': 'omp-chat'}
        data = {'rust-build-meta': {'target-directory': str(self.target),
                'non-test-binaries': {'app': [{'path': 'omp'}]}}, 'rust-suites': suites}
        raw = json.dumps(data).encode()
        (self.phase / 'list.json').write_bytes(raw)
        (self.phase / 'run.json').write_text(json.dumps({
            'phase': 'workspace', 'run_exit': run_exit, 'discovery_exit': 0,
            'hashes': {'list.json': hashlib.sha256(raw).hexdigest()}}))

    def test_removes_only_completed_test_executables(self):
        self.inventory()
        result = release.release(self.phase, self.target)
        self.assertEqual(result['removed_test_executables'], 1)
        self.assertFalse(self.binary.exists())
        for path in [self.app, self.library, self.phase / 'list.json', self.phase / 'run.json']:
            self.assertTrue(path.exists())

    def test_validates_every_path_before_any_deletion(self):
        outside = self.root / 'source'
        outside.write_text('keep')
        self.inventory(extra=outside)
        with self.assertRaises(ValueError):
            release.release(self.phase, self.target)
        self.assertTrue(self.binary.exists())
        self.assertEqual(outside.read_text(), 'keep')

    def test_preserves_non_test_application(self):
        self.inventory(extra=self.app)
        with self.assertRaises(ValueError):
            release.release(self.phase, self.target)
        self.assertTrue(self.binary.exists())
        self.assertTrue(self.app.exists())

    def test_refuses_failed_phase_or_changed_inventory(self):
        self.inventory(run_exit=100)
        with self.assertRaises(ValueError):
            release.release(self.phase, self.target)
        self.inventory()
        with (self.phase / 'list.json').open('a') as output:
            output.write(' ')
        with self.assertRaises(ValueError):
            release.release(self.phase, self.target)
        self.assertTrue(self.binary.exists())


if __name__ == '__main__':
    unittest.main()
