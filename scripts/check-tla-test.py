#!/usr/bin/env python3
"""Failure-path tests for TLC evidence tooling; actual proofs use check-tla.py."""
import importlib.util
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

spec = importlib.util.spec_from_file_location('check_tla', Path(__file__).with_name('check-tla.py'))
checker = importlib.util.module_from_spec(spec)
spec.loader.exec_module(checker)


class Checks(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        subprocess.run(['git', 'init', '-q', str(self.root)], check=True)
        for pair in checker.PAIRS.values():
            for name in pair:
                target = self.root / name
                target.parent.mkdir(parents=True, exist_ok=True)
                target.write_bytes((checker.ROOT / name).read_bytes())

    def test_inventory_has_every_pair(self):
        hashes = checker.inventory(self.root)
        self.assertEqual(set(hashes), {p for pair in checker.PAIRS.values() for p in pair})

    def test_duplicate_drift_fails(self):
        with (self.root / 'elastic/proof/ElasticSlots.tla').open('a') as out:
            out.write('\n\\* drift\n')
        with self.assertRaisesRegex(ValueError, 'Duplicate drift'):
            checker.inventory(self.root)

    def test_added_model_or_config_requires_coverage(self):
        for suffix in ['.tla', '.cfg']:
            with self.subTest(suffix=suffix):
                path = self.root / (checker.ADR + 'Uncovered' + suffix)
                path.write_text('uncovered')
                with self.assertRaisesRegex(ValueError, 'Unreviewed model/config'):
                    checker.inventory(self.root)
                path.unlink()

    def test_modified_jar_cannot_execute(self):
        jar = self.root / 'tool.jar'
        jar.write_bytes(b'not the pinned release')
        with self.assertRaisesRegex(ValueError, 'SHA-256 mismatch'):
            checker.acquire(jar, False)

    def test_timeout_is_incomplete_and_kills_checker(self):
        result = checker.execute([sys.executable, '-c', 'import time; time.sleep(30)'],
                                 self.root, self.root / 'timeout.log', 0.1, 10)
        self.assertEqual(result['incomplete'], 'timeout')
        self.assertNotEqual(result['exit_code'], 0)
        self.assertLess(result['seconds'], 3)

    def test_mutation_preserves_invariants_and_changes_one_transition(self):
        path = self.root / 'elastic/proof/ElasticSlots.tla'
        original = path.read_text()
        checker.mutate(path)
        changed = path.read_text()
        self.assertIn('ExactCommittedHistory ≜ history = CommittedRows(c, final) ∘ PartialHeadRows', changed)
        self.assertNotEqual(original, changed)
        with self.assertRaisesRegex(ValueError, 'mutation target changed'):
            checker.mutate(path)


if __name__ == '__main__':
    unittest.main()
