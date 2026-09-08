"""Bounded debug replies while continuing to consume a live terminal's output."""
import errno
import json
import os
import select
import signal
import socket
import time


def consume_pty(master, consume):
    try:
        chunk = os.read(master, 65536)
    except OSError as error:
        if error.errno == errno.EIO:  # Unix PTY EOF after the last slave closes.
            return False
        raise
    if chunk:
        consume(chunk)
    return bool(chunk)


def read_reply(connection, master, consume, deadline):
    pending = bytearray()
    live_pty = True
    while True:
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            raise TimeoutError('debug reply deadline expired while pumping terminal output')
        readable, _, _ = select.select([connection, master] if live_pty else [connection], [], [], remaining)
        if master in readable:
            live_pty = consume_pty(master, consume)
        if connection in readable:
            chunk = connection.recv(65536)
            if not chunk:
                raise EOFError('debug connection closed before a complete reply')
            pending.extend(chunk)
            if b'\n' in pending:
                return json.loads(pending.split(b'\n', 1)[0])


def request(debug, op, master, consume, timeout=2):
    deadline = time.monotonic() + timeout
    with socket.socket(socket.AF_UNIX) as connection:
        connection.settimeout(timeout)
        connection.connect(str(debug))
        connection.settimeout(max(0, deadline - time.monotonic()))
        connection.sendall(json.dumps({'op': op}).encode() + b'\n')
        connection.setblocking(False)
        response = read_reply(connection, master, consume, deadline)
    if response.get('ok') is not True:
        raise AssertionError(response)
    return response


def kill_and_reap(process, master, consume, timeout=5):
    """Cleanup evidence only: SIGKILL can never count as a clean quit."""
    record = {'pid': process.pid, 'signal': None, 'exit_code': process.poll(), 'reaped': False}
    try:
        if process.poll() is None:
            os.killpg(process.pid, signal.SIGKILL)
            record['signal'] = 'SIGKILL'
        deadline = time.monotonic() + timeout
        live_pty = master is not None
        while process.poll() is None:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise TimeoutError('process was not reaped within the five-second cleanup bound')
            if live_pty:
                if select.select([master], [], [], min(0.05, remaining))[0]:
                    live_pty = consume_pty(master, consume)
            else:
                time.sleep(min(0.05, remaining))
        record.update(exit_code=process.returncode, reaped=True)
    except Exception as error:
        record['error'] = f'{type(error).__name__}: {error}'
    return record
