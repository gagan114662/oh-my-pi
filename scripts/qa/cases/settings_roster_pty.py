"""Actual production settings walkthrough through OMP_TTY and debug resize hooks.

Raw terminal bytes, VT cell JSON and browser-readable HTML are observations of
omp, not reconstructed settings UI. No provider request is permitted.
"""
from __future__ import annotations

import fcntl
import html
import hashlib
import json
import os
from pathlib import Path
import pty
import select
import signal
import socket
import struct
import subprocess
import sys
import tempfile
import termios
import time

import pyte

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from harness import MODELS_TOML, OMP_BINARY, MockModel


def process_command(pid):
    result = subprocess.run(['ps', '-p', str(pid), '-o', 'command='], capture_output=True, text=True, timeout=5)
    if result.returncode not in (0, 1):
        raise RuntimeError(f'process inspection failed for {pid}: exit {result.returncode}')
    return result.stdout.strip() or None


def wait_identity_gone(pid, command, seconds):
    deadline = time.monotonic() + seconds
    while process_command(pid) == command:
        if time.monotonic() >= deadline:
            return False
        time.sleep(0.05)
    return True


def cleanup_daemons(binary, project, records):
    prefix = f'{binary} envd --root {project} --state-dir '
    listing = subprocess.check_output(['ps', '-axo', 'pid=,command='], text=True, timeout=5)
    for line in listing.splitlines():
        parts = line.strip().split(None, 1)
        if len(parts) != 2 or not parts[1].startswith(prefix):
            continue
        pid, command = int(parts[0]), parts[1]
        record = {'pid': pid, 'command': command, 'signals_sent': [], 'identity_disappeared': False, 'exit_code': None}
        records.append(record)
        for sig in (signal.SIGTERM, signal.SIGKILL):
            # Revalidate both identity and group ownership immediately before each
            # signal, including escalation; never signal a reused unrelated PID.
            if process_command(pid) != command:
                record['identity_disappeared'] = True
                break
            try:
                assert os.getpgid(pid) == pid, 'fixture daemon must own its process group'
                os.killpg(pid, sig)
                record['signals_sent'].append(sig.name)
            except ProcessLookupError:
                pass
            if wait_identity_gone(pid, command, 5):
                record['identity_disappeared'] = True
                break
        if not record['identity_disappeared']:
            raise RuntimeError(f'fixture daemon identity {pid} survived bounded TERM/KILL cleanup')
        # Detached daemons are not our child: disappearance is verified, but an
        # exit code is not available and is deliberately not fabricated.


    remaining = subprocess.check_output(['ps', '-axo', 'pid=,command='], text=True, timeout=5)
    assert not any(len(parts := line.strip().split(None, 1)) == 2 and parts[1].startswith(prefix)
                   for line in remaining.splitlines()), 'fixture daemon still present after cleanup'


def main():
    def expired(signum, frame):
        raise TimeoutError('settings PTY proof exceeded 180 seconds')
    signal.signal(signal.SIGALRM, expired)
    signal.alarm(180)
    output = Path(os.environ['OMP_SETTINGS_ROSTER_PROOF_DIR']) / 'live-pty'
    output.mkdir(parents=True, exist_ok=False)
    rows, failures, raw = [], [], bytearray()
    screen = pyte.Screen(140, 45)
    stream = pyte.ByteStream(screen)
    provenance = {'revision': subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip(),
                  'source_status': subprocess.check_output(['git', 'status', '--porcelain=v1'], text=True),
                  'hashes': {str(path): hashlib.sha256(path.read_bytes()).hexdigest() for path in
                             (Path(__file__), Path(__file__).parents[1] / 'harness.py', OMP_BINARY)}}
    (output / 'provenance.json').write_text(json.dumps(provenance, indent=2))
    process = None
    master = slave = None
    mock = MockModel('Unexpected model invocation: /settings must be a local command.', loop=True)
    with tempfile.TemporaryDirectory(prefix='omp-settings-') as temporary:
        root = Path(temporary)
        for name in ('home', 'data', 'config', 'project', 'cache', 'state'):
            (root / name).mkdir()
        (root / 'data/models.toml').write_text(MODELS_TOML.format(port=mock.port))
        (root / 'config/config.cfg').write_text('cl_startup_check_update false\n')
        debug = root / 'debug.sock'
        env = {k: v for k, v in os.environ.items() if not k.startswith('OMP_') and k != 'NO_COLOR'}
        env.update(HOME=str(root / 'home'), OMP_DATA_DIR=str(root / 'data'),
                   OMP_CONFIG_DIR=str(root / 'config'), XDG_CACHE_HOME=str(root / 'cache'),
                   XDG_STATE_HOME=str(root / 'state'), XDG_DATA_HOME=str(root / 'data'),
                   XDG_CONFIG_HOME=str(root / 'config'), TERM='xterm-256color', COLORTERM='truecolor',
                   OMP_TUI_DEBUG=str(debug))

        def drain(seconds=0.2):
            deadline = time.monotonic() + seconds
            while time.monotonic() < deadline:
                if select.select([master], [], [], min(0.05, max(0, deadline-time.monotonic())))[0]:
                    try:
                        chunk = os.read(master, 65536)
                    except OSError:
                        break
                    if not chunk:
                        break
                    raw.extend(chunk)
                    stream.feed(chunk)

        def request(op):
            with socket.socket(socket.AF_UNIX) as connection:
                connection.settimeout(2)
                connection.connect(str(debug))
                connection.sendall(json.dumps({'op': op}).encode() + b'\n')
                with connection.makefile('rb') as reader:
                    response = json.loads(reader.readline())
            if response.get('ok') is not True:
                raise AssertionError(response)
            return response

        def capture(name):
            drain()
            text = '\n'.join(screen.display)
            cells = [[{'text': c.data, 'fg': c.fg, 'bg': c.bg} for c in
                      [screen.buffer[y][x] for x in range(screen.columns)]] for y in range(screen.lines)]
            (output / f'{name}.json').write_text(json.dumps({'cells': cells, 'text': text}, indent=2))
            def color(value):
                return '#' + value if len(value) == 6 and all(c in '0123456789abcdefABCDEF' for c in value) else value
            body = []
            for line in cells:
                body.append(''.join('<span style="' +
                    (f'color:{color(c["fg"])};' if c['fg'] != 'default' else '') +
                    (f'background:{color(c["bg"])};' if c['bg'] != 'default' else '') +
                    '">' + html.escape(c['text']) + '</span>' for c in line))
            (output / f'{name}.html').write_text('<meta charset="utf-8"><pre>' + '\n'.join(body) + '</pre>')
            (output / f'{name}.ansi').write_bytes(raw)
            return text, cells

        try:
            master, slave = pty.openpty()
            before = termios.tcgetattr(slave)
            fcntl.ioctl(master, termios.TIOCSWINSZ, struct.pack('HHHH', 45, 140, 0, 0))
            env['OMP_TTY'] = os.ttyname(slave)
            with (output / 'stdout.log').open('wb') as stdout, (output / 'stderr.log').open('wb') as stderr:
                process = subprocess.Popen([str(OMP_BINARY), 'chat', '--model', 'mock', '--project',
                    str(root / 'project'), '--envd-idle-timeout', '2'], cwd=root / 'project', env=env,
                    stdin=subprocess.DEVNULL, stdout=stdout, stderr=stderr, start_new_session=True)
                deadline = time.monotonic() + 40
                while True:
                    drain()
                    try:
                        request('info')
                        break
                    except (OSError, ValueError, AssertionError):
                        if process.poll() is not None or time.monotonic() >= deadline:
                            raise AssertionError('production chat debug hook never became ready')
                capture('ready')
                os.write(master, b'/settings\r')
                # Independently fixed expected human labels from the product's
                # first group in each tab. Tab-strip names alone cannot pass.
                tabs = [('Appearance', 'Dark Theme'), ('Model', 'Show Thinking Blocks'),
                        ('Interaction', 'Autocomplete Items'), ('Context', 'Auto-Promote Context'),
                        ('Memory', 'Memory Backend'), ('Files', 'Edit Mode'),
                        ('Shell', 'Bash Auto-Background'), ('Tools', 'Inspect Image'),
                        ('Tasks', 'Goal Mode'), ('Providers', 'Image Provider Order')]
                def expect(name, marker, occurrences=1):
                    deadline = time.monotonic() + 8
                    while True:
                        text, _ = capture(name)
                        if text.count(marker) >= occurrences:
                            rows.append((name, marker, True))
                            return text
                        if process.poll() is not None or time.monotonic() >= deadline:
                            raise AssertionError(f'{name}: expected visible row {marker!r}')
                for index, (tab, label) in enumerate(tabs):
                    if index:
                        os.write(master, b'\x1b[C')
                    expect(f'tab-{index:02}-{tab.lower()}', label)
                os.write(master, b'\x1b[C')
                expect('tab-wrap', 'Dark Theme')
                os.write(master, b'Show Thinking Blocks')
                expect('keyboard-search', 'Show Thinking Blocks', occurrences=2)
                os.write(master, b'zz_unmatched_fixture')
                expect('keyboard-no-match', 'No settings match.')
                os.write(master, b'\x1b')
                expect('keyboard-clear', 'Show Thinking Blocks')
                if mock.captures:
                    failures.append('/settings caused a model request')
        except Exception as error:
            failures.append(str(error))
        finally:
            signal.alarm(0)
            if process is not None and process.poll() is None:
                try:
                    fcntl.ioctl(master, termios.TIOCSWINSZ, struct.pack('HHHH', 32, 92, 0, 0))
                    screen.resize(32, 92)
                    request('resize')
                    drain(0.5)
                    info = request('info')
                    resized_text, _ = capture('resized')
                    resized = 'Show Thinking Blocks' in resized_text and info.get('rows') == 32 and info.get('cols') == 92
                    rows.append(('resize', json.dumps(info), resized))
                    if not resized:
                        failures.append('resize dimensions did not propagate')
                    os.write(master, b'\x1b')
                    drain()
                    os.write(master, b'\x03\x03')
                    deadline = time.monotonic() + 10
                    while process.poll() is None and time.monotonic() < deadline:
                        drain()
                    restored = process.poll() == 0 and termios.tcgetattr(slave) == before
                    rows.append(('quit', 'zero exit and original termios restored', restored))
                    if not restored:
                        failures.append('clean quit or termios restoration failed')
                except Exception as error:
                    failures.append(f'cleanup proof: {error}')
                finally:
                    if process.poll() is None:
                        try:
                            os.killpg(process.pid, signal.SIGKILL)
                            process.wait(timeout=5)
                        except (OSError, subprocess.TimeoutExpired) as error:
                            failures.append(f'process reap failed: {error}')
            daemon_records = []
            try:
                cleanup_daemons(OMP_BINARY, root / 'project', daemon_records)
                rows.append(('daemon cleanup', 'fixture identities verified absent', True))
            except Exception as error:
                failures.append(f'daemon cleanup: {error}')
            if mock.captures:
                failures.append('unexpected provider request')
            if not any(stage == 'quit' and passed for stage, _, passed in rows):
                failures.append('clean quit was not verified')
            (output / 'daemon-cleanup.json').write_text(json.dumps({'project': str(root / 'project'), 'binary': str(OMP_BINARY), 'identities': daemon_records}, indent=2))
            (output / 'terminal.ansi').write_bytes(raw)
            (output / 'requests.json').write_text(json.dumps(mock.captures, indent=2))
            (output / 'result.json').write_text(json.dumps({'rows': rows, 'failures': failures}, indent=2))
            (output / 'summary.md').write_text('## Real PTY settings walkthrough\n\n| Stage | Observed | Pass |\n|---|---|---|\n' +
                ''.join(f'| {stage} | {html.escape(detail).replace("|", "&#124;")} | {passed} |\n' for stage, detail, passed in rows) +
                '\n' + '\n'.join('FAIL: ' + f for f in failures) + '\n\nHTML and JSON contain VT-emulated terminal colors; raw ANSI is retained.\n')
            for fd in (master, slave):
                if fd is not None:
                    os.close(fd)
            mock.close()
    if failures:
        raise SystemExit('\n'.join(failures))


if __name__ == '__main__':
    main()
