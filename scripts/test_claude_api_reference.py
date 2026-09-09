#!/usr/bin/env python3
"""Offline integrity checks for the actual vendored Anthropic reference."""
import importlib.util
from pathlib import Path
import shutil
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1] / '.omp/skills/claude-api'
SPEC = importlib.util.spec_from_file_location('claude_reference_verify', ROOT / 'verify.py')
VERIFY = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(VERIFY)


class ReferenceBundleTests(unittest.TestCase):
    def test_complete_pinned_bundle(self):
        report = VERIFY.verify(ROOT)
        self.assertEqual(report['status'], 'passed', report)
        self.assertEqual(report['files_checked'], 70)
        self.assertEqual(report['revision'], VERIFY.PIN)

    def test_missing_reference_is_actionable_and_changed_reference_fails(self):
        with tempfile.TemporaryDirectory(prefix='omp-reference-') as directory:
            root = Path(directory) / 'claude-api'
            shutil.copytree(ROOT, root)
            reference = root / 'upstream/shared/token-counting.md'
            original = reference.read_bytes()
            reference.unlink()
            missing = VERIFY.verify(root)
            self.assertEqual(missing['status'], 'failed')
            self.assertTrue(any('token-counting.md' in text and 'restore' in text
                                for text in missing['problems']), missing)
            reference.write_bytes(original + b'\nmodified reference\n')
            changed = VERIFY.verify(root)
            self.assertEqual(changed['status'], 'failed')
            self.assertTrue(any('content differs' in text for text in changed['problems']), changed)
            reference.write_bytes(original)
            self.assertEqual(VERIFY.verify(root)['status'], 'passed')


if __name__ == '__main__':
    unittest.main()
