#!/usr/bin/env python3
"""Bootstrap mechanics only; the container observation proves real prerequisites."""
from pathlib import Path
import subprocess
import tempfile
import unittest

SCRIPT = Path(__file__).with_name('build-doctor.sh').resolve()


class BootstrapTests(unittest.TestCase):
    def test_without_python_reports_repairs_without_external_commands(self):
        with tempfile.TemporaryDirectory() as empty:
            result = subprocess.run(['/bin/sh', str(SCRIPT)], env={'PATH': empty},
                                    capture_output=True, text=True, timeout=3)
        self.assertEqual(result.returncode, 1)
        for tool in ('python3', 'just', 'cc', 'c++', 'cmake', 'ninja', 'uv', 'cargo-nextest'):
            self.assertIn(f'FAIL {tool}: Install ', result.stdout)
        self.assertIn('CMake 3.15', result.stdout)
        self.assertIn('/opt/homebrew/opt/lld@22/bin/ld64.lld', result.stdout)
        self.assertEqual(result.stderr, '')

    def test_python_delegation_preserves_arguments_and_exit(self):
        with tempfile.TemporaryDirectory(prefix='doctor path ') as directory:
            executable = Path(directory) / 'python3'
            executable.write_text('#!/bin/sh\nprintf "%s\\n" "$@"\nexit 17\n')
            executable.chmod(0o700)
            result = subprocess.run(['/bin/sh', str(SCRIPT), 'argument with spaces', '--help'],
                                    env={'PATH': directory}, capture_output=True, text=True, timeout=3)
        self.assertEqual(result.returncode, 17)
        self.assertEqual(result.stdout.splitlines(),
                         [str(SCRIPT.with_name('build-doctor.py')), 'argument with spaces', '--help'])
        self.assertEqual(result.stderr, '')


if __name__ == '__main__':
    unittest.main()
