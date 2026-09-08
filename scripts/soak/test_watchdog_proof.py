"""Parser and gate teeth only; these tests do not simulate elapsed proof time."""
import copy
import json
from pathlib import Path
import tempfile
import unittest

from check_watchdog import parent_control
from watchdog_fixture import acceptance, calls, frames, notices
from watchdog_provider import REPEAT


class GateTests(unittest.TestCase):
    def test_exact_payload_and_stream_reconstruction(self):
        encoded = json.dumps(REPEAT)
        entries = [
            {'id': 'call1', 'event': 'tool.call@1', 'payload': {'name': 'bash', 'sid': 1}},
            {'id': 'call2', 'event': 'tool.call@1', 'payload': {'name': 'bash', 'args': {'command': REPEAT['command'] + '; true'}}},
            {'id': 's1', 'event': 'stream@1', 'payload': {'sid': 1, 'op': 'append', 'text': encoded[:20]}},
            {'id': 's2', 'event': 'stream@1', 'payload': {'sid': 1, 'op': 'append', 'text': encoded[20:]}},
        ]
        self.assertEqual(calls(entries, REPEAT), ['call1'])
        entries[-1]['payload']['text'] = 'broken'
        with self.assertRaises(ValueError):
            calls(entries, REPEAT)

    def test_notice_must_be_kernel_patch_not_authored_text(self):
        # Actual canonical shape from hosted run34275220932; no Txn.label.
        node = {'tag': 'notice', 'props': [['kind', 'warn'], ['custom:name', 'loop-guard']], 'content': 'stopped'}
        entry = {'event': 'patch@1', 'payload': {'ops': [['ins', 18, 83, node]]}}
        self.assertEqual(notices(entry, 'loop-guard'), [node])
        for kind in ('warn', 'error'):
            candidate = copy.deepcopy(entry)
            candidate['payload']['ops'][0][3]['props'][0][1] = kind
            self.assertEqual(len(notices(candidate, 'loop-guard')), 1)
        for tag in ('user', 'assistant', 'text'):
            candidate = copy.deepcopy(entry)
            candidate['payload']['ops'][0][3]['tag'] = tag
            self.assertEqual(notices(candidate, 'loop-guard'), [])
        for props in ([['name', 'loop-guard'], ['kind', 'warn']],
                      [['custom:name', 'other'], ['kind', 'warn']],
                      [['custom:name', 'loop-guard'], ['kind', 'hook']],
                      [['custom:name', 'loop-guard']],
                      [['custom:name', 'loop-guard'], ['kind', 'warn'], ['kind', 'error']]):
            candidate = copy.deepcopy(entry)
            candidate['payload']['ops'][0][3]['props'] = props
            self.assertEqual(notices(candidate, 'loop-guard'), [])
        for event in ('user.message@1', 'tool.result@1', 'stream@1'):
            self.assertEqual(notices({**entry, 'event': event}, 'loop-guard'), [])
        self.assertEqual(notices({'event': 'patch@1', 'payload': {'ops': [['ins', 1, None, 'loop-guard']]}}, 'loop-guard'), [])

    def test_repeat_gate_rejects17_or_success_or_no_notice(self):
        good = {'phase': 'repeat', 'matching_call_count': 16, 'loop_notice_ids': ['notice'], 'provider_requests': 16, 'terminal_status': 'incomplete'}
        self.assertTrue(acceptance(good))
        for key, value in [('matching_call_count', 17), ('matching_call_count', 0), ('provider_requests', 17), ('terminal_status', 'completed'), ('loop_notice_ids', [])]:
            with self.subTest(key=key, value=value):
                self.assertFalse(acceptance({**good, key: value}))

    def test_idle_gate_preserves_real_duration_and_cleanup(self):
        good = {'phase': 'idle', 'matching_call_count': 1, 'idle_notice_ids': ['notice'], 'terminal_status': 'cancelled', 'observed_seconds': 1860, 'maximum_nonstream_gap_seconds': 1800, 'observed_gap_seconds': 1800, 'minutes_to_idle_notice': 30.1, 'sleep_gone_at_cli_exit': True, 'held_sleep_observed_until_deadline_window': True}
        self.assertTrue(acceptance(good))
        for key, value in [('observed_seconds', 1859), ('maximum_nonstream_gap_seconds', 1799.9), ('minutes_to_idle_notice', 31.01), ('minutes_to_idle_notice', None), ('sleep_gone_at_cli_exit', False), ('held_sleep_observed_until_deadline_window', False), ('idle_notice_ids', []), ('terminal_status', 'completed')]:
            with self.subTest(key=key):
                self.assertFalse(acceptance({**good, key: value}))

    def test_parent_control_requires_actual100_not_build_transport_or_timeout_red(self):
        phase = {'phase': 'repeat', 'status': 'failed', 'failure_kind': 'acceptance', 'raw_exit': 0, 'matching_call_count': 100, 'tool_results': 100, 'provider_requests': 101, 'loop_notice_ids': []}
        report = {'phases': [phase]}
        self.assertTrue(parent_control(report, 1))
        self.assertFalse(parent_control(report, 0))
        for key, value in [('failure_kind', 'execution'), ('matching_call_count', 99), ('raw_exit', 1), ('externally_stopped', True), ('cleanup_errors', ['survivor']), ('tool_results', 0)]:
            mutated = copy.deepcopy(report)
            mutated['phases'][0][key] = value
            self.assertFalse(parent_control(mutated, 1), key)

    def test_torn_frame_ignored_but_duplicate_payload_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'journal.oms'
            path.write_text('id: 1\nevent: tool.call@1\ndata: {"name":"bash"}\n\nid: torn')
            self.assertEqual(len(frames(path)), 1)
            path.write_text('id: 1\nevent: tool.call@1\ndata: {"name":"bash","name":"eval"}\n\n')
            with self.assertRaises(ValueError):
                frames(path)


if __name__ == '__main__':
    unittest.main()
