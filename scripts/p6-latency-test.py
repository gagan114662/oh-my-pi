#!/usr/bin/env python3
"""Negative controls for the P6 timing evidence gate."""

import copy
import importlib.util
from pathlib import Path
import tempfile
import unittest

spec = importlib.util.spec_from_file_location("p6_latency", Path(__file__).with_name("p6-latency.py"))
proof = importlib.util.module_from_spec(spec)
spec.loader.exec_module(proof)


class TimingGateTests(unittest.TestCase):
    def setUp(self):
        row = {"completed": True,
               "journal": {"completed": True, "elapsed_ms": 100, "bound_ms": 3000},
               "resume": {"completed": True, "elapsed_ms": 1000, "bound_ms": 30000}}
        self.rows = {"normal": copy.deepcopy(row), "contended": copy.deepcopy(row)}
        self.load = {"error": None, "test_exit_code": 0, "syncs": 3, "syncs_before_test": 1}

    def test_measured_margin_passes(self):
        self.assertEqual(proof.validate(self.rows, self.load), [])

    def test_slow_contended_journal_fails(self):
        self.rows["contended"]["journal"]["elapsed_ms"] = 1501
        self.assertIn("contended/journal: less than 2x timing margin", proof.validate(self.rows, self.load))

    def test_inflated_bound_cannot_hide_slow_result(self):
        self.rows["contended"]["journal"]["bound_ms"] = 300000
        self.assertTrue(proof.validate(self.rows, self.load))

    def test_unfinished_and_invalid_measurements_fail(self):
        for value in (None, 0, -1, float("nan"), float("inf"), True, "10"):
            with self.subTest(value=value):
                self.rows["normal"]["resume"]["elapsed_ms"] = value
                self.assertTrue(proof.validate(self.rows, self.load))
        self.rows["normal"]["resume"] = None
        self.assertTrue(proof.validate(self.rows, self.load))

    def test_failed_full_proof_cannot_pass_on_partial_timings(self):
        self.rows["normal"]["completed"] = False
        self.assertTrue(proof.validate(self.rows, self.load))

    def test_missing_real_pressure_fails(self):
        self.load["syncs"] = self.load["syncs_before_test"]
        self.assertTrue(proof.validate(self.rows, self.load))

    def test_missing_artifacts_produce_failed_report(self):
        with tempfile.TemporaryDirectory() as temporary:
            output = Path(temporary)
            self.assertEqual(proof.report(output), 1)
            self.assertTrue((output / "p6-latency.json").is_file())


if __name__ == "__main__":
    unittest.main()
