#!/usr/bin/env python3
"""Production #124 phases. Real 30-minute idle limit; >=31-minute observation."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shlex
import shutil
import signal
import socket
import subprocess
import sys
import time

from journal_accounting import completed_turns, select_journal, unique_object
from turn_outcome_fixture import fixture_environment, cleanup_daemons, process_command, wait_identity_gone
from watchdog_provider import REPEAT, STALL

IDLE_SECONDS = 30 * 60
OBSERVATION_SECONDS = 31 * 60
DEFAULTS = {'sv_turn_idle_minutes': '30', 'sv_turn_max_requests': '500', 'sv_turn_max_wall_hours': '6', 'sv_tools_loop_guard_limit': '8'}
ALPHABET = '0123456789ABCDEFGHJKMNPQRSTVWXYZ'


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def frames(path):
    """Read complete frames only; final validation also checks selected ancestry."""
    result = []
    for block in path.read_bytes().split(b'\n\n')[:-1]:
        fields = {}
        for line in block.decode().splitlines():
            if line.startswith(':'):
                continue
            key, separator, value = line.partition(':')
            if not separator or key in fields:
                raise ValueError('invalid journal frame')
            fields[key] = value.removeprefix(' ')
        fields['payload'] = json.loads(fields['data'], object_pairs_hook=unique_object)
        result.append(fields)
    return result


def timestamp(entry):
    value = 0
    for char in entry['id'][:10]:
        value = value * 32 + ALPHABET.index(char)
    return value / 1000


def notices(entry, name):
    """Match canonical kernel notice nodes, not authored text naming a guard.

    Txn.label is diagnostic-only and is not serialized in patch@1. Producer
    names serialize as custom:name. Authored user/assistant/hook content does
    not become a warn/error notice merely by mentioning the watchdog.
    """
    if entry.get('event') != 'patch@1':
        return []
    found = []
    for op in entry.get('payload', {}).get('ops', []):
        if not isinstance(op, list) or len(op) != 4 or op[0] != 'ins':
            continue
        node = op[3]
        if not isinstance(node, dict) or node.get('tag') != 'notice':
            continue
        pairs = node.get('props', [])
        if not isinstance(pairs, list) or any(not isinstance(pair, list) or len(pair) != 2 or not isinstance(pair[0], str) for pair in pairs):
            continue
        props = dict(pairs)
        if len(props) != len(pairs):
            continue
        if props.get('custom:name') == name and props.get('kind') in ('warn', 'error'):
            found.append(node)
    return found


def calls(entries, payload):
    streams = {}
    for entry in entries:
        data = entry['payload']
        if entry['event'] == 'stream@1' and data['op'] == 'append':
            streams[data['sid']] = streams.get(data['sid'], '') + data.get('text', '')
    matches = []
    for entry in entries:
        data = entry['payload']
        if entry['event'] != 'tool.call@1' or data['name'] != 'bash':
            continue
        args = data.get('args')
        if args is None and data.get('sid') in streams:
            args = json.loads(streams[data['sid']], object_pairs_hook=unique_object)
        if args == payload:
            matches.append(entry['id'])
    return matches


def acceptance(record):
    if record['phase'] == 'repeat':
        return (
            1 <= record['matching_call_count'] <= 16
            and bool(record['loop_notice_ids'])
            and record['provider_requests'] <= 16
            and record['terminal_status'] in ('incomplete', 'cancelled')
        )
    return (
        record['matching_call_count'] == 1
        and bool(record['idle_notice_ids'])
        and record['terminal_status'] == 'cancelled'
        and record['observed_seconds'] >= OBSERVATION_SECONDS
        and record['maximum_nonstream_gap_seconds'] >= IDLE_SECONDS
        # Independent observer polls every0.5s; this is measurement uncertainty,
        # never a shorter watchdog. The durable timestamp gap above stays1800s.
        and record['observed_gap_seconds'] >= IDLE_SECONDS - 1
        and record['minutes_to_idle_notice'] is not None
        and record['minutes_to_idle_notice'] <= 31
        and record['sleep_gone_at_cli_exit']
        and record['held_sleep_observed_until_deadline_window']
    )


def stop_child(child):
    record = {'pid': child.pid, 'signals_sent': [], 'exit_code': child.poll()}
    if child.poll() is None:
        child.terminate()
        record['signals_sent'].append('SIGTERM')
        try:
            child.wait(timeout=5)
        except subprocess.TimeoutExpired:
            child.kill()
            record['signals_sent'].append('SIGKILL')
            child.wait(timeout=5)
    record['exit_code'] = child.returncode
    return record


def config(binary, project, env, out, parent):
    records = []
    def invoke(arguments):
        result = subprocess.run([str(binary), 'config', *arguments], cwd=project, env=env, capture_output=True, text=True, timeout=30)
        record = {'argv': arguments, 'raw_exit': result.returncode, 'stdout': result.stdout, 'stderr': result.stderr}
        records.append(record)
        (out / 'configuration.json').write_text(json.dumps(records, indent=2))
        return result
    # Owner setup only: hold the tool in the foreground, never change turn limits.
    result = invoke(['set', 'sv_shell_auto_background_enabled', 'false'])
    assert result.returncode == 0, 'cannot disable foreground detachment for held-tool proof'
    for key, expected in {**DEFAULTS, 'sv_shell_auto_background_enabled': 'false'}.items():
        result = invoke(['get', key])
        if parent and key != 'sv_tools_loop_guard_limit' and key != 'sv_shell_auto_background_enabled' and result.returncode != 0:
            continue  # Original parent lacks new convars; preserve raw unknown-setting output.
        assert result.returncode == 0 and result.stdout.strip() == expected, f'unexpected effective {key}'
    result = invoke(['dump'])
    assert result.returncode == 0
    return records


def run_phase(binary, out, project, sessions, data, env, phase, resume, parent):
    out.mkdir()
    record = {'phase': phase, 'status': 'incomplete', 'cleanup': [], 'daemon_cleanup': [], 'sleep_samples': []}
    provider = child = None
    observed = {}
    started = time.monotonic()
    start_wall = time.time()
    journal = None
    old_ids = set()
    if resume:
        journal = select_journal(sessions, resume)
        old_ids = {entry['id'] for entry in frames(journal)}
    record['initial_head'] = next(reversed(frames(journal)))['id'] if journal else None
    expected_sleep = None
    sleep_pid = None
    normal_exit_at = None
    try:
        record['configuration'] = config(binary, project, env, out, parent)
        if phase == 'idle':
            sleep_binary = shutil.which('gsleep') or shutil.which('sleep')
            assert sleep_binary, 'GNU sleep is required for external sleep infinity'
            # The script execs the real sleep: the recorded shell PID becomes the
            # held sleep PID, allowing independent liveness/cancellation evidence.
            (project / 'stall.sh').write_text('printf "%s" "$$" > sleep.pid\nexec ' + shlex.quote(sleep_binary) + ' infinity\n')
            record['stall_script_sha256'] = sha(project / 'stall.sh')
            record['sleep_binary'] = sleep_binary
            record['sleep_binary_sha256'] = sha(Path(sleep_binary).resolve())
            expected_sleep = sleep_binary + ' infinity'
        with (out / 'provider.log').open('wb') as log:
            provider = subprocess.Popen([sys.executable, str(Path(__file__).with_name('watchdog_provider.py')), phase, '--out', str(out)], cwd=project, env=env, stdout=log, stderr=subprocess.STDOUT)
        deadline = time.monotonic() + 10
        while not (out / 'port').exists():
            assert provider.poll() is None and time.monotonic() < deadline, 'provider did not start'
            time.sleep(0.05)
        port = int((out / 'port').read_text())
        (data / 'models.toml').write_text(f'''[providers.mock]
baseUrl = "http://127.0.0.1:{port}/v1"
auth = "none"
[providers.mock.models.mock]
name = "Watchdog deterministic transport"
api = "openai-completions"
contextWindow = 128000
maxTokens = 8192
supportsTools = true
supportsStreaming = true
''')
        command = [str(binary), 'print', '--mode', 'json', '--yolo', '--model', 'mock', '--project', str(project), '--session-dir', str(sessions), '--max-time', '6h']
        if resume:
            command += ['--resume', resume]
        command += ['Run the requested tool. Keep trying the same operation until the harness settles this turn.']
        record['argv'] = command
        started, start_wall = time.monotonic(), time.time()
        record['started_wall'] = start_wall
        record['started_monotonic'] = started
        with (out / 'stdout.jsonl').open('wb') as stdout, (out / 'stderr.log').open('wb') as stderr:
            child = subprocess.Popen(command, cwd=project, env=env, stdout=stdout, stderr=stderr, stdin=subprocess.DEVNULL, start_new_session=True)
        hard_seconds = OBSERVATION_SECONDS + 30 if phase == 'idle' else 300
        while True:
            now = time.monotonic()
            if journal is None:
                candidates = list(sessions.glob('*.oms'))
                if len(candidates) == 1:
                    journal = candidates[0]
            if journal:
                entries = frames(journal)
                with (out / 'entry-observations.jsonl').open('a') as log:
                    for entry in entries:
                        if entry['id'] not in old_ids and entry['id'] not in observed:
                            observed[entry['id']] = now
                            log.write(json.dumps({'id': entry['id'], 'event': entry['event'], 'monotonic': now, 'wall': time.time()}) + '\n')
            if phase == 'idle' and (project / 'sleep.pid').exists():
                sleep_pid = int((project / 'sleep.pid').read_text())
                actual = process_command(sleep_pid)
                if actual == expected_sleep or actual is None:
                    record['sleep_samples'].append({'pid': sleep_pid, 'monotonic': now, 'command': actual})
                else:
                    raise AssertionError(f'held sleep identity mismatch: {actual!r}')
            code = child.poll()
            if code is not None and normal_exit_at is None:
                normal_exit_at = now
                record['raw_exit'] = code
                record['exit_after_seconds'] = now - started
                record['sleep_gone_at_cli_exit'] = phase != 'idle' or (sleep_pid is not None and process_command(sleep_pid) is None)
            elapsed = now - started
            assert abs((time.time() - start_wall) - elapsed) < 2, 'wall clock changed during monotonic proof'
            # Idle keeps observing after natural settlement until a full31min has
            # elapsed. The outer deadline cannot become an accepted turn notice.
            if code is not None and (phase != 'idle' or elapsed >= OBSERVATION_SECONDS):
                break
            if elapsed >= hard_seconds:
                record['externally_stopped'] = True
                break
            assert provider.poll() is None, 'provider died before proof ended'
            time.sleep(0.5)
        record['observed_seconds'] = time.monotonic() - started
        assert journal is not None, 'no actual journal'
        accounting = completed_turns(journal)
        record['accounting'] = accounting
        all_entries = frames(journal)
        # One serial appended turn, no rewind: ancestry is validated above, and
        # explicit non-linear parents reject this phase rather than mixing paths.
        for before, after in zip(all_entries, all_entries[1:]):
            assert after.get('prior', before['id']) == before['id'], 'phase changed journal branch'
        entries = [entry for entry in all_entries if entry['id'] not in old_ids]
        turn_ids = [entry['id'] for entry in entries if entry['event'] == 'turn.start@1']
        assert len(turn_ids) == 1, 'phase must contain exactly one actual turn'
        record['turn_id'] = turn_ids[0]
        record['terminal_status'] = accounting['terminal_outcomes'].get(turn_ids[0])
        record['matching_call_ids'] = calls(entries, REPEAT if phase == 'repeat' else STALL)
        record['matching_call_count'] = len(record['matching_call_ids'])
        record['tool_results'] = sum(entry['event'] == 'tool.result@1' for entry in entries)
        record['loop_notice_ids'] = [entry['id'] for entry in entries if notices(entry, 'loop-guard')]
        idle_notices = [entry for entry in entries if any('idle watchdog' in node.get('content', '') for node in notices(entry, 'turn-limit'))]
        record['idle_notice_ids'] = [entry['id'] for entry in idle_notices]
        nonstream = [entry for entry in entries if entry['event'] != 'stream@1']
        gaps = [(timestamp(right) - timestamp(left), left, right) for left, right in zip(nonstream, nonstream[1:]) if not idle_notices or timestamp(right) <= timestamp(idle_notices[0])]
        gap, left, right = max(gaps, key=lambda item: item[0]) if gaps else (0, None, None)
        record['maximum_nonstream_gap_seconds'] = gap
        record['gap_entry_ids'] = [left['id'], right['id']] if left else []
        record['observed_gap_seconds'] = observed[right['id']] - observed[left['id']] if left else None
        record['minutes_to_idle_notice'] = (observed[idle_notices[0]['id']] - observed[left['id']]) / 60 if idle_notices and left else None
        time.sleep(0.2)  # Let the independent provider log its final request.
        captures = [json.loads(line) for line in (out / 'provider.jsonl').read_text().splitlines()]
        record['provider_requests'] = len(captures)
        assert captures and any(tool.get('function', {}).get('name') == 'bash' for tool in captures[0]['body'].get('tools', [])), 'actual catalog omitted bash'
        record['journal_sha256'] = sha(journal)
        shutil.copyfile(journal, out / 'journal.oms')
        assert not record.get('externally_stopped'), 'outer deadline is not watchdog success'
        assert record['raw_exit'] == (1 if phase == 'idle' else 0), 'unexpected production process exit (cancelled print exits1)'
        record['held_sleep_observed_until_deadline_window'] = bool(left) and any(sample['command'] == expected_sleep and sample['monotonic'] - observed[left['id']] >= IDLE_SECONDS - 1 for sample in record['sleep_samples'])
        passed = acceptance(record)
        record.update(status='passed' if passed else 'failed', failure_kind=None if passed else 'acceptance')
        return journal.stem, record
    except BaseException as error:
        record.update(status='failed', failure_kind='execution', error=str(error))
        return resume, record
    finally:
        errors = []
        for owned in (child, provider):
            if owned:
                try:
                    record['cleanup'].append(stop_child(owned))
                except Exception as error:
                    errors.append(str(error))
        try:
            cleanup_daemons(binary, project, record['daemon_cleanup'])
            # Independent held-child cleanup is separate from proof success.
            # A leaked sleep makes the phase fail even if cleanup can kill it.
            if sleep_pid and process_command(sleep_pid) == expected_sleep:
                record['held_sleep_leaked'] = True
                for sig in (signal.SIGTERM, signal.SIGKILL):
                    if process_command(sleep_pid) != expected_sleep:
                        break
                    os.kill(sleep_pid, sig)
                    if wait_identity_gone(sleep_pid, expected_sleep, 5):
                        break
                assert process_command(sleep_pid) != expected_sleep, 'held sleep survived cleanup'
                errors.append('held sleep survived production cancellation')
            sockets = []
            # Production project_state uses SHA256(canonical state directory)
            # for owner-local /tmp sockets. Scope probes to this private state,
            # including sockets outside the fixture directory on macOS.
            candidates = {path for path in out.parent.rglob('*') if path.is_socket()}
            record['socket_state_prefixes'] = []
            for state in (data / 'projects').glob('*'):
                if state.is_dir():
                    digest = hashlib.sha256(os.fsencode(state.resolve())).hexdigest()[:32]
                    prefix = f'omp-{os.geteuid()}-{digest}-'
                    record['socket_state_prefixes'].append(prefix)
                    candidates.update(Path('/tmp').glob(prefix + '*.sock'))
            for path in candidates:
                if path.is_socket():
                    probe = socket.socket(socket.AF_UNIX)
                    probe.settimeout(1)
                    try:
                        result = probe.connect_ex(str(path))
                    finally:
                        probe.close()
                    sockets.append({'path': str(path), 'connect_errno': result})
                    assert result != 0, 'fixture socket still accepts connections'
            record['socket_cleanup'] = sockets
        except Exception as error:
            errors.append(str(error))
        if errors:
            record.update(status='failed', failure_kind='cleanup', cleanup_errors=errors)
        (out / 'observation.json').write_text(json.dumps(record, indent=2) + '\n')


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--out', type=Path, required=True)
    parser.add_argument('--phase', choices=('repeat', 'all'), default='all')
    parser.add_argument('--parent', action='store_true')
    args = parser.parse_args()
    assert not args.parent or args.phase == 'repeat', 'parent arm is the100-call semantic control only'
    out, binary = args.out.resolve(), args.binary.resolve()
    out.mkdir(parents=True)
    project, sessions, data = (out / name for name in ('project', 'sessions', 'data'))
    for path in (project, sessions, data):
        path.mkdir()
    env, isolated = fixture_environment(out, data)
    report = {'status': 'incomplete', 'binary_sha256': sha(binary), 'isolated_paths': isolated, 'phases': [], 'model_claim': 'deterministic transport correctness only; no Anthropic performance measurement'}
    session = None
    for phase in ('repeat', 'idle') if args.phase == 'all' else ('repeat',):
        session, record = run_phase(binary, out / phase, project, sessions, data, env, phase, session, args.parent)
        report['phases'].append(record)
        (out / 'report.json').write_text(json.dumps(report, indent=2))
    report['status'] = 'passed' if all(phase['status'] == 'passed' for phase in report['phases']) else 'failed'
    (out / 'report.json').write_text(json.dumps(report, indent=2) + '\n')
    return 0 if report['status'] == 'passed' else 1


if __name__ == '__main__':
    sys.exit(main())
