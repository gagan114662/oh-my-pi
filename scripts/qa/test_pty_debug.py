"""Real PTY backpressure and bounded cleanup tests; no production UI claim."""
import os
import pty
import socket
import subprocess
import threading
import time
import unittest
from unittest.mock import patch

from pty_debug import read_reply, kill_and_reap


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


if __name__ == '__main__':
    unittest.main()
