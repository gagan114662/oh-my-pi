"""Exercise the status gate through its real CLI, including negative cases."""
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

CHECKER = Path(__file__).with_name("check-adr-status.py")


class StatusGate(unittest.TestCase):
    def run_gate(self, documents):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            adrs = root / "docs/adr"
            adrs.mkdir(parents=True)
            for name, text in documents.items():
                (adrs / name).write_text(text)
            report = root / "report.json"
            result = subprocess.run([sys.executable, str(CHECKER), "--root", str(root),
                                     "--report", str(report)], capture_output=True, text=True)
            return result, json.loads(report.read_text())

    def test_all_records_are_inspected(self):
        result, report = self.run_gate({
            "0001-good.md": "## Status in omp\n**Implemented.** Ready.\n",
            "0002-good.md": "## Status in omp\n**Partial.** Works. Gap: restart proof missing.\n",
            "0003-bad.md": "## Status in omp\n**Partial.** Works.\n",
        })
        self.assertEqual(result.returncode, 1)
        self.assertEqual(len(report["records"]), 3)
        self.assertEqual(report["failures"][0]["path"], "docs/adr/0003-bad.md")

    def test_gap_elsewhere_does_not_hide_missing_status_gap(self):
        result, _ = self.run_gate({"0001.md":
            "# Gap: background\n## Status in omp\n**Partial.**\n## References\nGap: elsewhere.\n"})
        self.assertEqual(result.returncode, 1)

    def test_empty_gap_is_rejected(self):
        result, _ = self.run_gate({"0001.md": "## Status in omp\n**Partial.** Gap: \n"})
        self.assertEqual(result.returncode, 1)

    def test_explicit_gap_and_implemented_pass(self):
        result, report = self.run_gate({
            "0001.md": "## Status in omp\n**Partial.**\nGap: video input is absent.\n",
            "0002.md": "## Status in omp\n**Implemented.**\n",
        })
        self.assertEqual(result.returncode, 0, result.stdout)
        self.assertEqual(report["failures"], [])

    def test_missing_or_duplicate_status_fails(self):
        for text in ["# Missing\n", "## Status in omp\nImplemented\n## Status in omp\nImplemented\n"]:
            with self.subTest(text=text):
                result, _ = self.run_gate({"0001.md": text})
                self.assertEqual(result.returncode, 1)

    def test_empty_inventory_fails(self):
        result, _ = self.run_gate({})
        self.assertEqual(result.returncode, 1)


if __name__ == "__main__":
    unittest.main()
