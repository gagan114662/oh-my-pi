"""Offline tests of the proof judge; synthetic files here are never runtime evidence."""
import contextlib
import importlib.util
import io
import json
from pathlib import Path
import tempfile
import unittest

spec = importlib.util.spec_from_file_location('proof', Path(__file__).with_name('prove.py'))
proof = importlib.util.module_from_spec(spec)
spec.loader.exec_module(proof)


class ProofJudge(unittest.TestCase):
    def judge(self, exit_code, failure, log, elapsed, with_xml=True):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'execution.json').write_text(json.dumps({'command_exit': exit_code}))
            (root / 'test.log').write_text(log)
            if with_xml:
                (root / 'junit.xml').write_text(f'<testsuites><testsuite><testcase name="{proof.MANIFEST["test"]}" time="{elapsed}">{failure}</testcase></testsuite></testsuites>')
            with contextlib.redirect_stdout(io.StringIO()):
                return proof.summarize(root, root, 'parent')

    def test_compile_failure_is_not_negative_proof(self):
        self.assertTrue(self.judge(101, '', 'error: could not compile', 0, with_xml=False))

    def test_generic_failure_or_timeout_is_not_negative_proof(self):
        self.assertTrue(self.judge(100, '<failure>other assertion</failure>', 'other assertion', 2.1))
        self.assertTrue(self.judge(100, '<error>timeout</error>', 'left: Timeout\nright: Cancelled', 240))

    def test_fast_synthetic_failure_cannot_claim_two_second_watchdog(self):
        self.assertTrue(self.judge(100, '<failure/>', 'left: Timeout\nright: Cancelled', 0.01))

    def test_exact_observed_semantic_failure_is_accepted(self):
        self.assertFalse(self.judge(100, '<failure/>', 'left: Timeout\nright: Cancelled', 2.01))


if __name__ == '__main__':
    unittest.main()
