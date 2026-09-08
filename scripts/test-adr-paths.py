"""Exercise the real path-check CLI, including browser annotations and summaries."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

CHECKER = Path(__file__).with_name("check-adr-paths.py")


class PathGate(unittest.TestCase):
    def run_gate(self, documents, files=()):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "docs/adr").mkdir(parents=True)
            for name, body in documents.items():
                (root / "docs/adr" / name).write_text(body)
            for name in files:
                path = root / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text("fixture")
            report = root / "report.json"
            summary = root / "summary.md"
            summary.write_text("Earlier step evidence\n")
            result = subprocess.run(
                [sys.executable, str(CHECKER), "--root", str(root), "--report", str(report)],
                env={**os.environ, "GITHUB_STEP_SUMMARY": str(summary)},
                capture_output=True, text=True, timeout=10)
            return result, json.loads(report.read_text()), summary.read_text()

    def test_resolved_paths_braces_globs_symbols_and_line_ranges(self):
        result, report, summary = self.run_gate({"0001.md":
            "## Status in omp\n`crates/ai/src/{title,stt}.rs`\n"
            "## References\n`crates/ai/src/*.rs` `crates/ai/src/title.rs:2-4` "
            "`crates/ai/src/stt.rs::Engine`\n"},
            ["crates/ai/src/title.rs", "crates/ai/src/stt.rs"])
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(len(report["paths"]), 5)
        self.assertEqual(report["failures"], [])
        self.assertEqual(summary.count("| resolved |"), 5)
        self.assertTrue(summary.startswith("Earlier step evidence\n"))

    def test_injected_deleted_crate_fails_at_exact_document_line(self):
        result, report, summary = self.run_gate({"0001.md":
            "# ADR\n## Status in omp\n`crates/ai/src/lib.rs`\n`crates/inference/src/lib.rs`\n"},
            ["crates/ai/src/lib.rs"])
        self.assertEqual(result.returncode, 1)
        self.assertIn("::error file=docs/adr/0001.md,line=4::missing crates/inference/src/lib.rs", result.stdout)
        self.assertEqual(len(report["paths"]), 2)
        self.assertEqual(len(report["failures"]), 1)
        self.assertIn("| crates/inference/src/lib.rs | **missing** |", summary)
        self.assertIn("| crates/ai/src/lib.rs | resolved |", summary)

    def test_every_missing_path_is_annotated(self):
        result, report, summary = self.run_gate({
            "0001.md": "## Status in omp\n`crates/inference`\n",
            "0002.md": "## References\n`crates/shell-engine`\n"})
        self.assertEqual(result.returncode, 1)
        self.assertEqual(result.stdout.count("::error file="), 2)
        self.assertEqual(len(report["failures"]), 2)
        self.assertEqual(summary.count("**missing**"), 2)

    def test_historical_context_does_not_become_current_claim(self):
        result, report, _ = self.run_gate({"0001.md":
            "## Context\n`crates/old`\n## Status in omp\n`crates/ai/src/lib.rs`\n"
            "## Consequences\n`crates/old`\n"}, ["crates/ai/src/lib.rs"])
        self.assertEqual(result.returncode, 0)
        self.assertEqual(len(report["paths"]), 1)

    def test_empty_inventory_fails_with_visible_summary(self):
        result, report, summary = self.run_gate({})
        self.assertEqual(result.returncode, 1)
        self.assertIn("::error file=docs/adr/README.md,line=1::", result.stdout)
        self.assertIn("no current ADR crate references found", summary)
        self.assertFalse(report["paths"])


if __name__ == "__main__":
    unittest.main()
