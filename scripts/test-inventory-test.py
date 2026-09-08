#!/usr/bin/env python3
"""Negative nextest JSON/JUnit fixtures at the binary/package evidence boundary."""
from contextlib import redirect_stdout
import io
import sys
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
from types import SimpleNamespace

spec = importlib.util.spec_from_file_location('inventory', Path(__file__).with_name('test-inventory.py'))
inventory = importlib.util.module_from_spec(spec)
spec.loader.exec_module(inventory)


class EvidenceTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.metadata = {'workspace_members': ['a@1', 'b@1'], 'packages': [
            {'id': 'a@1', 'name': 'omp-a'}, {'id': 'b@1', 'name': 'omp-b'}]}

    def phase(self, label, package, tests, xml=None, discovery=0, run=0):
        directory = self.root / label
        directory.mkdir()
        binary = package.split('@')[0] + '::integration'
        data = {'test-count': len(tests), 'rust-suites': {binary: {
            'package-id': package, 'binary-id': binary, 'status': 'listed',
            'testcases': {name: {'ignored': False, 'filter-match': {'status': 'matches'}} for name in tests}}}}
        (directory / 'list.json').write_text(json.dumps(data))
        if xml is not None:
            (directory / 'junit.xml').write_text(xml)
        return ({'phase': label, 'packages': [package], 'discovery_exit': discovery, 'run_exit': run}, directory)

    def xml(self, package, cases):
        binary = package + '::integration'
        return f'<testsuites tests="{len(cases)}"><testsuite name="{binary}" tests="{len(cases)}">' + ''.join(
            f'<testcase name="{name}" classname="{binary}">{result}</testcase>' for name, result in cases) + '</testsuite></testsuites>'

    def rows(self, runs):
        report = inventory.summarize(self.metadata, runs)
        return report, {r['package']: r for r in report['packages']}

    def target_metadata(self):
        for package in self.metadata['packages']:
            package['targets'] = [
                {'name': 'integration', 'kind': ['test'], 'test': True},
                {'name': 'optional', 'kind': ['test'], 'test': True,
                 'required-features': ['extra']},
                {'name': 'build-script-build', 'kind': ['custom-build'], 'test': False}]
        return self.metadata

    def target_phase(self, label, package, tests, xml=None, **kwargs):
        run, folder = self.phase(label, package, tests, xml, **kwargs)
        data = json.loads((folder / 'list.json').read_text())
        for suite in data['rust-suites'].values():
            suite.update(kind='test', **{'binary-name': 'integration'})
        (folder / 'list.json').write_text(json.dumps(data))
        return run, folder

    def test_target_table_retains_unobserved_and_empty_targets(self):
        runs = [self.target_phase('ok', 'a@1', ['x'], self.xml('a', [('x', '')])),
                self.target_phase('empty', 'b@1', [], self.xml('b', []))]
        rows = inventory.target_inventory(self.target_metadata(), runs)
        self.assertEqual(len(rows), 4)
        self.assertEqual(rows[0]['run'], 1)
        self.assertEqual(rows[0]['status'], 'passed')
        self.assertIsNone(rows[1]['run'])
        self.assertEqual(rows[1]['required_features'], ['extra'])
        self.assertEqual(rows[2]['status'], 'zero registered tests')
        self.assertEqual(rows[2]['run'], 0)

    def test_target_table_matches_proc_macro_binary(self):
        metadata = self.target_metadata()
        metadata['packages'][0]['targets'][0]['kind'] = ['proc-macro']
        run, folder = self.target_phase('macro', 'a@1', ['x'], self.xml('a', [('x', '')]))
        data = json.loads((folder / 'list.json').read_text())
        data['rust-suites']['a::integration']['kind'] = 'proc-macro'
        (folder / 'list.json').write_text(json.dumps(data))
        row = inventory.target_inventory(metadata, [(run, folder)])[0]
        self.assertEqual(row['status'], 'passed')
        self.assertEqual(row['kind'], 'lib')

    def test_target_table_preserves_failure_across_repeat(self):
        runs = [self.target_phase('bad', 'a@1', ['x'], self.xml('a', [('x', '<failure/>')]), run=1),
                self.target_phase('good', 'a@1', ['x'], self.xml('a', [('x', '')]))]
        row = inventory.target_inventory(self.target_metadata(), runs)[0]
        self.assertEqual((row['run'], row['passed'], row['failed']), (1, 0, 1))

    def test_target_table_build_failure_is_unknown_not_zero_pass(self):
        runs = [self.target_phase('build', 'a@1', [], discovery=100, run=None)]
        row = inventory.target_inventory(self.target_metadata(), runs)[0]
        self.assertIsNone(row['run'])
        self.assertIn('not observed', row['status'])
        self.assertIn('discovery/build incomplete', row['problems'][0])

    def test_target_table_missing_and_corrupt_results_are_not_passes(self):
        for index, xml in enumerate([None, '<broken>', self.xml('a', [])]):
            run = self.target_phase(str(index), 'a@1', ['x'], xml)
            row = inventory.target_inventory(self.target_metadata(), [run])[0]
            self.assertNotEqual(row['status'], 'passed')
            self.assertEqual(row['registered'], 1)

    def test_both_crates_pass_with_equal_test_names_different_binaries(self):
        phases = [self.phase(x, x+'@1', ['same'], self.xml(x, [('same', '')])) for x in ['a', 'b']]
        report, rows = self.rows(phases)
        self.assertEqual(report['status'], 'passed')
        self.assertEqual([r['run'] for r in rows.values()], [1, 1])

    def test_zero_registered_is_not_missing_compilation(self):
        zero = self.phase('zero', 'a@1', [], self.xml('a', []), run=4)
        failed = self.phase('compile', 'b@1', [], discovery=100, run=None)
        report, rows = self.rows([zero, failed])
        self.assertEqual(rows['omp-a']['status'], 'zero registered tests')
        self.assertEqual(rows['omp-a']['registered'], 0)
        self.assertEqual(rows['omp-b']['status'], 'unknown: discovery unavailable')
        self.assertIsNone(rows['omp-b']['run'])
        self.assertEqual(report['status'], 'failed')

    def test_absent_junit_is_unknown_not_pass(self):
        _, rows = self.rows([self.phase('missing', 'a@1', ['x'])])
        self.assertEqual(rows['omp-a']['status'], 'unknown: execution unavailable')
        self.assertIsNone(rows['omp-a']['passed'])

    def test_listed_but_zero_run_is_distinct(self):
        _, rows = self.rows([self.phase('empty', 'a@1', ['x'], self.xml('a', []))])
        self.assertEqual(rows['omp-a']['status'], 'zero executed tests')
        self.assertEqual(rows['omp-a']['missing'], 1)

    def test_failure_and_flaky_pass_are_not_erased_by_repeated_success(self):
        bad = self.phase('first', 'a@1', ['x'], self.xml('a', [('x', '<flakyFailure/>')]))
        good = self.phase('second', 'a@1', ['x'], self.xml('a', [('x', '')]))
        _, rows = self.rows([bad, good])
        self.assertEqual(rows['omp-a']['failed'], 1)
        self.assertEqual(rows['omp-a']['passed'], 0)
        self.assertEqual(rows['omp-a']['run'], 1)

    def test_duplicate_or_unknown_test_cannot_inflate_coverage(self):
        for cases in [[('x', ''), ('x', '')], [('invented', '')]]:
            with self.subTest(cases=cases):
                label=str(len(list(self.root.iterdir())))
                report, rows = self.rows([self.phase(label, 'a@1', ['x'], self.xml('a', cases))])
                self.assertEqual(report['status'], 'failed')
                self.assertEqual(len(rows), 2)
                self.assertTrue(any('invalid evidence' in e for e in report['errors']))

    def test_truncated_xml_retains_all_crate_rows(self):
        report, rows = self.rows([self.phase('truncated', 'a@1', ['x'], '<testsuites>')])
        self.assertEqual(report['status'], 'failed')
        self.assertEqual(len(rows), 2)

    def test_missing_one_result_and_skipped_selected_test_fail(self):
        run = self.phase('partial', 'a@1', ['x', 'y', 'z'], self.xml('a', [('x', ''), ('y', '<skipped/>')]))
        report, rows = self.rows([run])
        self.assertEqual(rows['omp-a']['run'], 1)
        self.assertEqual(rows['omp-a']['skipped'], 1)
        self.assertEqual(rows['omp-a']['missing'], 1)
        self.assertEqual(report['status'], 'failed')

    def test_discovery_streams_diagnostics_without_polluting_json_or_losing_failure(self):
        log = self.root / 'compile.log'
        output = self.root / 'list.json'
        console = io.StringIO()
        script = 'import sys,json; print(json.dumps({"test-count": 0})); print("compiling fixture", file=sys.stderr); sys.exit(7)'
        with redirect_stdout(console):
            code = inventory.invoke([sys.executable, '-c', script], log, output)
        self.assertEqual(code, 7)
        self.assertEqual(json.loads(output.read_text()), {'test-count': 0})
        self.assertEqual(log.read_text(), 'compiling fixture\n')
        self.assertEqual(console.getvalue(), 'compiling fixture\n')

    def test_capture_preserves_failed_exit_and_discards_stale_junit(self):
        inventory.save(self.root / 'metadata.json', self.metadata)
        inventory.save(self.root / 'manifest.json', {'revision': 'fixed', 'metadata_sha256': inventory.digest(self.root / 'metadata.json')})
        junit = self.root / 'old-junit.xml'
        junit.write_text(self.xml('a', [('stale', '')]))
        observed = []
        def invoke(command, log, stdout=None):
            observed.append(command)
            log.write_text('error: no tests to run')
            if stdout is not None:
                stdout.write_text('{"test-count":0,"rust-suites":{}}')
                return 0
            self.assertFalse(junit.exists(), 'stale report survived into the next invocation')
            return 4
        args = SimpleNamespace(output=self.root, phase='capture', selection=['--', '--workspace', '--exclude', 'omp-b', '--locked'], junit=junit)
        with patch.object(inventory.subprocess, 'check_output', return_value='fixed'), patch.object(inventory, 'invoke', side_effect=invoke):
            self.assertEqual(inventory.capture(args), 4)
        self.assertEqual(observed[1], ['cargo', 'nextest', 'run', '--profile', 'ci', '--workspace', '--exclude', 'omp-b', '--locked'])
        saved=json.loads((self.root / 'capture/run.json').read_text())
        self.assertEqual(saved['run_exit'], 4)
        self.assertFalse((self.root / 'capture/junit.xml').exists())

    def test_always_report_without_initialization_fails_and_writes_artifact(self):
        output = self.root / 'not-initialized'
        summary = self.root / 'step-summary.md'
        with patch.dict(inventory.os.environ, {'GITHUB_STEP_SUMMARY': str(summary)}), patch('builtins.print'):
            self.assertEqual(inventory.report(SimpleNamespace(output=output)), 1)
        report = json.loads((output / 'summary.json').read_text())
        self.assertEqual(report['status'], 'failed')
        self.assertTrue(report['errors'])
        self.assertEqual(report['packages'], [])
        self.assertIn('**failed**', summary.read_text())

    def test_workspace_scope_comes_from_metadata_not_fixed_count(self):
        self.metadata['workspace_members'].append('c@1')
        self.metadata['packages'].append({'id': 'c@1', 'name': 'omp-new'})
        report, rows = self.rows([])
        self.assertEqual(len(rows), 3)
        self.assertEqual(rows['omp-new']['status'], 'unknown: discovery unavailable')
        names=inventory.members(self.metadata)
        self.assertEqual(inventory.selected_packages(names,['--workspace','--exclude','omp-b']), ['a@1','c@1'])


if __name__ == '__main__':
    unittest.main()
