"""Fixture isolation and cleanup mechanics; these do not run production turns."""
import os
from pathlib import Path
import signal
import tempfile
import unittest
from unittest.mock import patch

import turn_outcome_fixture as fixture


class FixtureIsolationTests(unittest.TestCase):
	def test_owner_configuration_is_not_inherited(self):
		with tempfile.TemporaryDirectory() as directory:
			root = Path(directory)
			with patch.dict(os.environ, {'HOME': '/unrelated', 'OMP_PROFILE': 'owner', 'OMP_CONFIG_FILES': '/unrelated/override', 'OMP_CODING_AGENT_SESSION_DIR': '/unrelated/sessions', 'XDG_CONFIG_HOME': '/unrelated/xdg', 'BASH_ENV': '/unrelated/bash'}):
				environment, paths = fixture.fixture_environment(root, root / 'data')
				for name in ('OMP_PROFILE', 'OMP_CONFIG_FILES', 'OMP_CODING_AGENT_SESSION_DIR', 'BASH_ENV'):
					self.assertNotIn(name, environment)
				for name, path in paths.items():
					self.assertEqual(environment[name], path)
					self.assertTrue(Path(path).is_relative_to(root))
					self.assertTrue(Path(path).is_dir())
				self.assertEqual(os.environ['HOME'], '/unrelated', 'fixture must not mutate owner process environment')

	def test_graceful_cleanup_records_disappearance_without_inventing_exit_code(self):
		command = '/fixture/omp envd --root /fixture/project --state-dir /fixture/state'
		records = []
		with patch.object(fixture.subprocess, 'check_output', return_value='123 ' + command), patch.object(fixture, 'process_command', return_value=command), patch.object(fixture.os, 'getpgid', return_value=123), patch.object(fixture.os, 'killpg') as kill, patch.object(fixture, 'wait_identity_gone', return_value=True):
			fixture.cleanup_daemons('/fixture/omp', '/fixture/project', records)
		kill.assert_called_once_with(123, signal.SIGTERM)
		self.assertTrue(records[0]['identity_disappeared'])
		self.assertIsNone(records[0]['exit_code'])

	def test_pid_reuse_before_escalation_is_not_signalled(self):
		command = '/fixture/omp envd --root /fixture/project --state-dir /fixture/state'
		records = []
		with patch.object(fixture.subprocess, 'check_output', return_value='123 ' + command), patch.object(fixture, 'process_command', side_effect=[command, '/unrelated/process']), patch.object(fixture.os, 'getpgid', return_value=123), patch.object(fixture.os, 'killpg') as kill, patch.object(fixture, 'wait_identity_gone', return_value=False):
			fixture.cleanup_daemons('/fixture/omp', '/fixture/project', records)
		kill.assert_called_once_with(123, signal.SIGTERM)
		self.assertTrue(records[0]['identity_disappeared'])

	def test_surviving_daemon_cannot_be_reported_as_cleaned(self):
		command = '/fixture/omp envd --root /fixture/project --state-dir /fixture/state'
		records = []
		with patch.object(fixture.subprocess, 'check_output', return_value='123 ' + command), patch.object(fixture, 'process_command', return_value=command), patch.object(fixture.os, 'getpgid', return_value=123), patch.object(fixture.os, 'killpg'), patch.object(fixture, 'wait_identity_gone', return_value=False), self.assertRaises(RuntimeError):
			fixture.cleanup_daemons('/fixture/omp', '/fixture/project', records)
		self.assertFalse(records[0]['identity_disappeared'])
		self.assertEqual(records[0]['signals_sent'], ['SIGTERM', 'SIGKILL'])
