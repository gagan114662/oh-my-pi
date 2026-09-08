"""Real PTY backpressure and bounded cleanup tests; no production UI claim."""
import os
import pty
import select
import socket
import subprocess
import sys
import termios
import threading
import time
import unittest
from unittest.mock import patch

from pty_debug import read_reply, kill_and_reap, launch, termios_mode


class PtyDebugTests(unittest.TestCase):
    def test_reply_wait_drains_repaint_that_blocks_host_before_reply(self):
        master, slave = pty.openpty()
        client, server = socket.socketpair()
        written = threading.Event()
        errors = []
        payload = b'x' * (512 * 1024)
        def host():
            try:
                view = memoryview(payload)
                while view:
                    count = os.write(slave, view)
                    view = view[count:]
                written.set()
                server.sendall(b'{"ok":true,"rows":32,"cols":92}\n')
            except Exception as error:
                errors.append(error)
        thread = threading.Thread(target=host, daemon=True)
        thread.start()
        received = bytearray()
        try:
            # The old socket-only wait cannot finish: actual PTY writes block
            # before this host can send its response. No mock readiness flag.
            client.settimeout(0.05)
            with self.assertRaises(TimeoutError):
                client.recv(1)
            self.assertFalse(written.is_set())
            client.setblocking(False)
            response = read_reply(client, master, received.extend, time.monotonic() + 2)
            self.assertEqual(response, {'ok': True, 'rows': 32, 'cols': 92})
            self.assertTrue(written.wait(1))
            thread.join(timeout=1)
            self.assertFalse(thread.is_alive())
            self.assertFalse(errors)
            self.assertGreater(len(received), 400000)
        finally:
            client.close()
            server.close()
            os.close(master)
            os.close(slave)
            thread.join(timeout=1)

    def test_missing_reply_keeps_original_bound(self):
        master, slave = pty.openpty()
        client, server = socket.socketpair()
        try:
            started = time.monotonic()
            with self.assertRaises(TimeoutError):
                read_reply(client, master, lambda chunk: None, started + 0.05)
            self.assertLess(time.monotonic() - started, 1)
        finally:
            client.close()
            server.close()
            os.close(master)
            os.close(slave)

    def test_reap_timeout_is_returned_without_masking_original_failure(self):
        class Survives:
            pid = 123
            returncode = None
            def poll(self):
                return None
        with patch('pty_debug.os.killpg') as kill:
            record = kill_and_reap(Survives(), None, lambda chunk: None, timeout=0)
        kill.assert_called_once()
        self.assertFalse(record['reaped'])
        self.assertIsNone(record['exit_code'])
        self.assertIn('TimeoutError', record['error'])

    def test_real_child_exit_is_observed_and_reaped(self):
        child = subprocess.Popen(['sleep', '60'], start_new_session=True)
        record = kill_and_reap(child, None, lambda chunk: None)
        self.assertTrue(record['reaped'])
        self.assertLess(record['exit_code'], 0)

    RAW_CHILD = (
        'import os, sys, termios, tty\n'
        'fd = os.open(sys.argv[1], os.O_RDWR)\n'
        'original = termios.tcgetattr(fd)\n'
        'tty.setraw(fd)\n'
        'if sys.argv[2] == "restore":\n'
        '    termios.tcsetattr(fd, termios.TCSANOW, original)\n'
        'os.write(fd, b"done\\r\\n")\n'
        'os._exit(0)\n'
    )

    def _run_raw_child(self, restore):
        master, slave = pty.openpty()
        received = bytearray()
        try:
            before = termios.tcgetattr(slave)
            child = launch([sys.executable, '-c', self.RAW_CHILD, os.ttyname(slave), 'restore' if restore else 'leave'],
                           stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
            record = None
            deadline = time.monotonic() + 10
            while child.poll() is None and time.monotonic() < deadline:
                if select.select([master], [], [], 0.05)[0]:
                    try:
                        received.extend(os.read(master, 65536))
                    except OSError:
                        break
            if child.poll() is None:
                record = kill_and_reap(child, master, received.extend)
            self.assertEqual(child.returncode, 0, (record, child.stderr.read()))
            # The driver's own slave descriptor must still answer after the
            # child exits: a revoked slave (session leader + controlling tty
            # on macOS) would raise ENOTTY here and hide the real answer.
            after = termios.tcgetattr(slave)
            return termios_mode(before), termios_mode(after), bytes(received)
        finally:
            os.close(master)
            os.close(slave)

    def test_child_that_leaves_raw_mode_is_detected_after_exit(self):
        before, after, received = self._run_raw_child(restore=False)
        self.assertIn(b'done', received)
        self.assertTrue(before['ICANON'] and before['ECHO'] and before['OPOST'])
        self.assertNotEqual(before, after)
        self.assertFalse(after['ICANON'])

    def test_child_that_restores_termios_matches_before_exit(self):
        before, after, received = self._run_raw_child(restore=True)
        self.assertIn(b'done', received)
        self.assertEqual(before, after)

    def test_launch_creates_a_signalable_process_group(self):
        child = launch(['sleep', '60'])
        try:
            self.assertEqual(os.getpgid(child.pid), child.pid)
            self.assertNotEqual(os.getsid(child.pid), child.pid)
        finally:
            record = kill_and_reap(child, None, lambda chunk: None)
        self.assertTrue(record['reaped'])
        self.assertLess(record['exit_code'], 0)


if __name__ == '__main__':
    unittest.main()
