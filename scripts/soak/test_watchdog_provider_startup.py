"""Real local HTTP startup for both watchdog scripts with DNS unavailable."""
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import time
import unittest
from urllib.request import Request, urlopen


class WatchdogProviderStartupTests(unittest.TestCase):
    def test_both_scripts_start_without_reverse_dns_and_capture_actual_requests(self):
        provider = Path(__file__).with_name('watchdog_provider.py')
        wrapper = '''import runpy,socket,sys
def unavailable_reverse_dns(host):
 raise AssertionError("reverse DNS unavailable")
socket.getfqdn=unavailable_reverse_dns
sys.argv=sys.argv[1:]
runpy.run_path(sys.argv[0],run_name="__main__")
'''
        for phase, count in [('repeat', 100), ('idle', 1)]:
            with self.subTest(phase=phase), tempfile.TemporaryDirectory(prefix='omp-watchdog-http-') as directory:
                out = Path(directory)
                with (out / 'provider.log').open('wb') as log:
                    process = subprocess.Popen([sys.executable, '-c', wrapper, str(provider), phase,
                                                '--out', str(out)], stdout=log, stderr=subprocess.STDOUT)
                try:
                    deadline = time.monotonic() + 10
                    while not (out / 'port').exists():
                        self.assertIsNone(process.poll(), (out / 'provider.log').read_text())
                        self.assertLess(time.monotonic(), deadline, 'unchanged ten-second readiness gate')
                        time.sleep(0.01)
                    base = f'http://127.0.0.1:{int((out / "port").read_text())}'
                    script = json.loads((out / 'script.json').read_text())
                    self.assertEqual(script['phase'], phase)
                    self.assertEqual(script['scripted_tool_responses'], count)
                    if phase == 'idle':
                        self.assertEqual(script['arguments'], {'command': '/bin/bash stall.sh', 'timeout': 0})
                    body = {'model': 'mock', 'messages': [{'role': 'user', 'content': 'fixture'}], 'stream': False}
                    def post():
                        request = Request(base + '/v1/chat/completions', data=json.dumps(body).encode(),
                                          headers={'Content-Type': 'application/json'})
                        with urlopen(request, timeout=2) as response:
                            return response.read()
                    first = json.loads(post())
                    tool = first['choices'][0]['message']['tool_calls'][0]['function']
                    self.assertEqual(tool['name'], 'bash')
                    self.assertEqual(json.loads(tool['arguments']), script['arguments'])
                    body['stream'] = True
                    second = post()
                    self.assertIn(b'[DONE]', second)
                    events = [json.loads(line[6:]) for line in second.splitlines()
                              if line.startswith(b'data: ') and line != b'data: [DONE]']
                    deltas = [event['choices'][0]['delta'] for event in events]
                    if phase == 'idle':
                        self.assertEqual(''.join(delta.get('content', '') for delta in deltas),
                                         'Adversarial script exhausted.')
                    else:
                        arguments = ''.join(tool['function'].get('arguments', '')
                                            for delta in deltas for tool in delta.get('tool_calls', []))
                        self.assertEqual(json.loads(arguments), script['arguments'])
                    deadline = time.monotonic() + 2
                    captures = []
                    while len(captures) < 2:
                        path = out / 'provider.jsonl'
                        captures = [json.loads(line) for line in path.read_text().splitlines()] if path.exists() else []
                        self.assertLess(time.monotonic(), deadline, 'provider must retain both requests')
                        time.sleep(0.01)
                    self.assertEqual([row['ordinal'] for row in captures], [1, 2])
                    self.assertEqual([row['body']['stream'] for row in captures], [False, True])
                finally:
                    process.terminate()
                    try:
                        process.wait(timeout=2)
                    except subprocess.TimeoutExpired:
                        process.kill()
                        process.wait(timeout=2)


if __name__ == '__main__':
    unittest.main()
