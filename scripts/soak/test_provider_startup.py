"""Offline provider startup: numeric loopback must not depend on reverse DNS."""
import json
from pathlib import Path
import socket
import subprocess
import sys
import tempfile
import time
import unittest
from unittest.mock import patch
from urllib.request import Request, urlopen

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'qa'))
from harness import LoopbackHTTPServer, MockModel, Reply, call


class ProviderStartupTests(unittest.TestCase):
    def test_wire_builders_do_not_construct_a_listener(self):
        with patch.object(MockModel, '__init__', side_effect=AssertionError('unexpected listener')):
            completion = MockModel._completion(call('bash', command='echo fixture'), 1, {'model': 'mock'})
            self.assertEqual(completion['choices'][0]['message']['tool_calls'][0]['function']['name'], 'bash')
            events = MockModel._sse_events(Reply(text='fixture response'), 2, {'model': 'mock'})
            self.assertEqual(events[-1], b'[DONE]')
            self.assertIn(b'fixture response', b''.join(events))

    def test_inherited_builders_preserve_subclass_wire_dispatch(self):
        class CustomizedModel(MockModel):
            @staticmethod
            def _tool_call_wire(tool_call, ordinal, index):
                wire = MockModel._tool_call_wire(tool_call, ordinal, index)
                wire['id'] = 'customized'
                return wire

        with CustomizedModel(call('bash', command='echo fixture')) as mock:
            reply = call('bash', command='echo fixture')
            completion = mock._completion(reply, 1, {'model': 'mock'})
            self.assertEqual(completion['choices'][0]['message']['tool_calls'][0]['id'], 'customized')
            self.assertIn(b'customized', b''.join(mock._sse_events(reply, 1, {'model': 'mock'})))

    def test_shared_mock_binds_without_reverse_dns_and_remains_loopback_only(self):
        with patch.object(socket, 'getfqdn', side_effect=AssertionError('unexpected reverse DNS')):
            with MockModel('fixture response') as mock:
                with urlopen(f'http://127.0.0.1:{mock.port}/state', timeout=2) as response:
                    self.assertEqual(json.load(response)['served'], 0)
            with self.assertRaisesRegex(ValueError, 'numeric loopback'):
                LoopbackHTTPServer(('0.0.0.0', 0), object)

    def test_real_provider_is_ready_and_serves_both_wire_modes_without_reverse_dns(self):
        provider = Path(__file__).with_name('provider.py')
        wrapper = '''import runpy,socket,sys
def unavailable_reverse_dns(host):
 raise AssertionError("reverse DNS is unavailable")
socket.getfqdn=unavailable_reverse_dns
sys.argv=sys.argv[1:]
runpy.run_path(sys.argv[0],run_name="__main__")
'''
        with tempfile.TemporaryDirectory(prefix='omp-soak-provider-') as directory:
            root = Path(directory)
            ready, requests = root / 'port', root / 'requests.jsonl'
            with (root / 'provider.log').open('wb') as log:
                process = subprocess.Popen([sys.executable, '-c', wrapper, str(provider),
                    '--log', str(requests), '--ready-file', str(ready)], stdout=log, stderr=subprocess.STDOUT)
            try:
                deadline = time.monotonic() + 10
                while not ready.exists():
                    self.assertIsNone(process.poll(), (root / 'provider.log').read_text())
                    self.assertLess(time.monotonic(), deadline, 'unchanged provider readiness deadline')
                    time.sleep(0.01)
                base = f'http://127.0.0.1:{int(ready.read_text())}'
                def post(path, body):
                    request = Request(base + path, data=json.dumps(body).encode(),
                                      headers={'Content-Type': 'application/json'})
                    with urlopen(request, timeout=2) as response:
                        return response.read()
                post('/control', {'deltas_per_second': 10000})
                body = {'model': 'mock', 'messages': [{'role': 'user', 'content': 'fixture'}], 'stream': False}
                first = json.loads(post('/v1/chat/completions', body))
                self.assertEqual(first['choices'][0]['message']['tool_calls'][0]['function']['name'], 'bash')
                body['stream'] = True
                second = post('/v1/chat/completions', body)
                self.assertIn(b'[DONE]', second)
                self.assertIn(b'Soak turn complete', second)
                with urlopen(base + '/state', timeout=2) as response:
                    self.assertEqual(json.load(response)['served'], 2)
                self.assertGreaterEqual(len(requests.read_text().splitlines()), 3)
            finally:
                process.terminate()
                try:
                    process.wait(timeout=2)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait(timeout=2)


if __name__ == '__main__':
    unittest.main()
