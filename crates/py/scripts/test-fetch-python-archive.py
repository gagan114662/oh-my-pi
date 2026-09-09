#!/usr/bin/env python3
"""Exercise the production archive fetch stages with real curl/zstd/tar."""
import functools
import http.server
import io
import os
from pathlib import Path
import subprocess
import tarfile
import tempfile
import threading
import unittest

SCRIPT = Path(__file__).with_name('fetch-python-archive.sh')


class QuietHandler(http.server.SimpleHTTPRequestHandler):
    def log_message(self, *_args):
        pass


class FetchArchiveTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='omp-fetch-python-test-')
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.vendor = self.root / 'vendor' / 'python'
        self.vendor.mkdir(parents=True)
        (self.vendor / 'previous').write_text('working tree')
        self.server = http.server.ThreadingHTTPServer(
            ('127.0.0.1', 0), functools.partial(QuietHandler, directory=self.root))
        self.thread = threading.Thread(target=self.server.serve_forever)
        self.thread.start()
        self.addCleanup(self.stop_server)
        self.base = f'http://127.0.0.1:{self.server.server_port}'

    def stop_server(self):
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(timeout=5)
        self.assertFalse(self.thread.is_alive())

    def archive(self, data=None):
        if data is None:
            buf = io.BytesIO()
            with tarfile.open(fileobj=buf, mode='w') as tar:
                content = b'real fixture\n'
                info = tarfile.TarInfo('python/install/fixture')
                info.size = len(content)
                tar.addfile(info, io.BytesIO(content))
            data = buf.getvalue()
        result = subprocess.run(['zstd', '-q', '-c'], input=data,
                                capture_output=True, check=True, timeout=10)
        path = self.root / 'fixture.tar.zst'
        path.write_bytes(result.stdout)
        return path

    def fetch(self, name='fixture.tar.zst', env=None):
        result = subprocess.run(
            ['bash', str(SCRIPT), f'{self.base}/{name}', str(self.vendor), 'fixture-v1'],
            capture_output=True, text=True, timeout=30, env=env)
        self.assertEqual(list(self.vendor.parent.glob('.fetch-py.*')), [], result.stderr)
        return result

    def failed(self, result, stage):
        self.assertNotEqual(result.returncode, 0, result.stderr)
        self.assertIn(f'error: python archive {stage}: exit {result.returncode}', result.stderr)
        self.assertEqual((self.vendor / 'previous').read_text(), 'working tree')
        self.assertFalse((self.vendor / '.archive.stamp').exists())

    def test_valid_archive_replaces_only_after_complete_validation(self):
        self.archive()
        result = self.fetch()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual((self.vendor / 'install/fixture').read_text(), 'real fixture\n')
        self.assertEqual((self.vendor / '.archive.stamp').read_text(), 'fixture-v1\n')
        self.assertFalse((self.vendor / 'previous').exists())
        for stage in ('download', 'decompress', 'extract'):
            self.assertIn(f'python archive {stage}: exit 0', result.stderr)

    def test_truncated_compressed_archive(self):
        path = self.archive()
        path.write_bytes(path.read_bytes()[:-5])
        self.failed(self.fetch(), 'decompress')

    def test_corrupt_compressed_archive(self):
        self.archive().write_bytes(b'not a zstandard archive')
        self.failed(self.fetch(), 'decompress')

    def test_invalid_tar(self):
        self.archive(b'not a tar archive')
        self.failed(self.fetch(), 'extract')

    def test_truncated_tar(self):
        buf = io.BytesIO()
        with tarfile.open(fileobj=buf, mode='w') as tar:
            info = tarfile.TarInfo('python/install/fixture')
            info.size = 8192
            tar.addfile(info, io.BytesIO(b'x' * info.size))
        self.archive(buf.getvalue()[:1024])
        self.failed(self.fetch(), 'extract')

    def test_http_failure(self):
        result = self.fetch('missing.tar.zst')
        self.failed(result, 'download')
        self.assertEqual(result.returncode, 22)

    def test_signal_status_is_never_accepted_or_retried(self):
        # Inject the exact observed status at each stage; real-format failure
        # cases above independently exercise the actual tools.
        self.archive()
        for stage, executable in [('download', 'curl'), ('decompress', 'zstd'), ('extract', 'tar')]:
            with self.subTest(stage=stage):
                bindir = self.root / stage
                bindir.mkdir()
                stub = bindir / executable
                stub.write_text('#!/bin/sh\nexit 141\n')
                stub.chmod(0o755)
                env = dict(os.environ, PATH=str(bindir) + os.pathsep + os.environ['PATH'])
                result = self.fetch(env=env)
                self.failed(result, stage)
                self.assertEqual(result.returncode, 141)


if __name__ == '__main__':
    unittest.main()
