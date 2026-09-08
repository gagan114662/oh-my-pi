"""Production theme demo via P7's OMP_TTY/debug hooks; never mocks the UI.

Run with uv run --with pyte python scripts/qa/cases/theme_live_pty.py.
HTML captures render the actual VT-emulated cell colors. Retained frame PNG
proof remains in theme_defaults.rs; its monochrome raster is not color proof.
"""
from __future__ import annotations

import fcntl
import html
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


def main():
    output = Path(os.environ['OMP_THEME_PROOF_DIR']) / 'live-pty'
    output.mkdir(parents=True, exist_ok=True)
    rows, failures, raw = [], [], bytearray()
    screen = pyte.Screen(100, 30)
    stream = pyte.ByteStream(screen)
    process = None
    master = slave = None
    mock = MockModel('Unexpected model invocation: /theme must be a local command.', loop=True)
    with tempfile.TemporaryDirectory(prefix='omp-theme-') as temporary:
        root = Path(temporary)
        for name in ('home', 'data', 'config', 'project', 'cache', 'state'):
            (root / name).mkdir()
        (root / 'data/models.toml').write_text(MODELS_TOML.format(port=mock.port))
        debug = root / 'debug.sock'
        env = {k: v for k, v in os.environ.items() if not k.startswith('OMP_') and k != 'NO_COLOR'}
        env.update(HOME=str(root / 'home'), OMP_DATA_DIR=str(root / 'data'),
                   OMP_CONFIG_DIR=str(root / 'config'), XDG_CACHE_HOME=str(root / 'cache'),
                   XDG_STATE_HOME=str(root / 'state'), TERM='xterm-256color', COLORTERM='truecolor',
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
            fcntl.ioctl(master, termios.TIOCSWINSZ, struct.pack('HHHH', 30, 100, 0, 0))
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
                os.write(master, b'/theme\r')
                deadline = time.monotonic() + 8
                while True:
                    text, cells = capture('theme-selector')
                    if all(name in text.lower() for name in ('default', 'dark', 'light')):
                        break
                    if time.monotonic() >= deadline:
                        raise AssertionError('/theme did not expose every shipped palette: default, dark, light')
                original_selector = cells
                rows.append(('selector', 'all shipped names visible', True))
                baseline_config = {str(p.relative_to(root / 'config')): p.read_bytes() for p in (root / 'config').rglob('*') if p.is_file()}
                os.write(master, b'\x1b[A\x1b[A\x1b[A')
                drain(0.5)
                palettes = []
                for index in range(3):
                    if index:
                        os.write(master, b'\x1b[B')
                    drain(0.5)
                    text, cells = capture(f'arrow-{index}')
                    palettes.append(cells)
                # Compare the same glyph at the same location, excluding every
                # palette-name row. Cursor movement/text changes cannot satisfy this.
                changed_rows = set()
                first, last = palettes[0], palettes[-1]
                for y, (left, right) in enumerate(zip(first, last)):
                    line = ''.join(c['text'] for c in left + right).lower()
                    if any(n in line for n in ('default', 'dark', 'light')):
                        continue
                    for a, b in zip(left, right):
                        if a['text'].strip() and a['text'] == b['text'] and (a['fg'], a['bg']) != (b['fg'], b['bg']):
                            changed_rows.add(y)
                recolored = len(changed_rows) >= 2
                rows.append(('arrows', f'unchanged glyphs recolored on surrounding rows {sorted(changed_rows)}', recolored))
                if not recolored:
                    failures.append('arrow navigation did not recolor surrounding UI on at least two rows')
                preview_config = {str(p.relative_to(root / 'config')): p.read_bytes() for p in (root / 'config').rglob('*') if p.is_file()}
                if preview_config != baseline_config:
                    failures.append('preview wrote configuration before Enter')
                os.write(master, b'\x1b')
                drain(0.5)
                capture('cancelled')
                cancelled_config = {str(p.relative_to(root / 'config')): p.read_bytes() for p in (root / 'config').rglob('*') if p.is_file()}
                rows.append(('cancel', 'configuration unchanged', cancelled_config == baseline_config))
                if cancelled_config != baseline_config:
                    failures.append('cancel did not preserve configuration')
                # Escape leaves the submenu for Settings; re-enter its selected
                # theme row, choose the last shipped palette, then persist.
                os.write(master, b'\r')
                drain(0.5)
                _, restored_cells = capture('cancel-reopened')
                comparable = [(a, b) for left, right in zip(original_selector, restored_cells)
                              for a, b in zip(left, right) if a['text'].strip() and a['text'] == b['text']]
                restored_colors = bool(comparable) and all((a['fg'], a['bg']) == (b['fg'], b['bg']) for a, b in comparable)
                rows.append(('cancel colors', 'original selector colors restored', restored_colors))
                if not restored_colors:
                    failures.append('Escape did not restore original UI colors')
                os.write(master, b'\x1b[B\x1b[B\r')
                drain(1)
                capture('committed')
                configs = list((root / 'config').rglob('*.cfg'))
                config_text = '\n'.join(p.read_text() for p in configs)
                (output / 'persisted-config.cfg').write_text(config_text)
                persisted = any('cl_theme_' in line and 'light' in line for line in config_text.splitlines())
                rows.append(('enter', 'selected light palette persisted in cfg', persisted))
                if not persisted:
                    failures.append('Enter did not persist the selected light palette')
                if mock.captures:
                    failures.append('/theme was sent to the model instead of handled locally')
        except Exception as error:
            failures.append(str(error))
        finally:
            if process is not None and process.poll() is None:
                try:
                    fcntl.ioctl(master, termios.TIOCSWINSZ, struct.pack('HHHH', 32, 92, 0, 0))
                    screen.resize(32, 92)
                    request('resize')
                    drain(0.5)
                    info = request('info')
                    capture('resized')
                    resized = info.get('rows') == 32 and info.get('cols') == 92
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
                        os.killpg(process.pid, signal.SIGKILL)
                        process.wait(timeout=5)
            (output / 'terminal.ansi').write_bytes(raw)
            (output / 'requests.json').write_text(json.dumps(mock.captures, indent=2))
            (output / 'result.json').write_text(json.dumps({'rows': rows, 'failures': failures}, indent=2))
            (output / 'summary.md').write_text('## Real PTY theme demo\n\n| Stage | Observed | Pass |\n|---|---|---|\n' +
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
