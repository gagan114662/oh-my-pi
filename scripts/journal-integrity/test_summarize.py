#!/usr/bin/env python3
"""Negative tests for evidence parsing; these XML fixtures are not run evidence."""
import importlib.util
from pathlib import Path
import tempfile
import unittest
import xml.etree.ElementTree as ET

spec = importlib.util.spec_from_file_location('summarize', Path(__file__).with_name('summarize.py'))
proof = importlib.util.module_from_spec(spec)
spec.loader.exec_module(proof)


class OperationEvidenceTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.out = Path(self.directory.name)

    def reports(self, defect=None):
        packages = {}
        seen = set()
        for _, package, classname, name in proof.OPERATIONS:
            if (package, classname, name) in seen:
                continue
            seen.add((package, classname, name))
            root = packages.setdefault(package, ET.Element('testsuites'))
            case = ET.SubElement(root, 'testcase', classname=classname, name=name, time='0.1')
            if defect:
                defect(case)
        for package, root in packages.items():
            ET.ElementTree(root).write(self.out / f'{package}.junit.xml')

    def test_exact_cases_supply_every_required_row(self):
        self.reports()
        rows, failures, _ = proof.operation_rows(self.out)
        self.assertEqual(len(rows), 6)
        self.assertFalse(failures)
        self.assertTrue(all(row[-1] for row in rows))

    def test_missing_reports_fail_every_operation(self):
        rows, failures, _ = proof.operation_rows(self.out)
        self.assertEqual(len(failures), 6)
        self.assertFalse(any(row[-1] for row in rows))

    def test_failed_skipped_flaky_or_invalid_time_cannot_pass(self):
        for tag in ['failure', 'error', 'skipped', 'flakyFailure', 'rerunFailure']:
            with self.subTest(tag=tag):
                self.reports(lambda case: ET.SubElement(case, tag))
                rows, failures, _ = proof.operation_rows(self.out)
                self.assertEqual(len(failures), 6)
                self.assertFalse(any(row[-1] for row in rows))
        self.reports(lambda case: case.set('time', 'nan'))
        self.assertEqual(len(proof.operation_rows(self.out)[1]), 6)

    def test_wrong_name_and_duplicate_are_not_accepted(self):
        self.reports(lambda case: case.set('name', 'different_test'))
        self.assertEqual(len(proof.operation_rows(self.out)[1]), 6)
        self.reports()
        path = self.out / 'omp-e2e.junit.xml'
        root = ET.parse(path).getroot()
        root.append(ET.fromstring(ET.tostring(root[0])))
        ET.ElementTree(root).write(path)
        self.assertEqual(proof.operation_rows(self.out)[1], ['operation: lift'])

    def test_malformed_xml_is_a_failed_row_not_a_crash(self):
        self.reports()
        (self.out / 'omp-app.junit.xml').write_text('<truncated')
        self.assertEqual(proof.operation_rows(self.out)[1], ['operation: import'])


if __name__ == '__main__':
    unittest.main()
