"""Production RPC discovery survives session transitions (#142).

Uses the shared HTTP mock, but launches the actual RPC/RPC-UI entrypoint.
No fabricated SessionHome can accidentally repair the constructor under test.
"""

import json
import os
from pathlib import Path
import queue
import signal
import subprocess
import sys
import tempfile
import threading
import time
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from harness import CONFIG_TOML, MODELS_TOML, OMP_BINARY, MockModel  # noqa: E402

COVERS: dict[str, list[str]] = {"py": [], "rpc": []}


class RpcPromptFacts(unittest.TestCase):
	def exercise(self, mode, flags=(), populated=True):
		with tempfile.TemporaryDirectory(prefix="omp-rpc-facts-") as directory:
			root = Path(directory)
			home, project, data, config, sessions = [root / name for name in
				("home", "home/project", "data", "config", "sessions")]
			for path in (home, project, data, config, sessions):
				path.mkdir(parents=True, exist_ok=True)
			markers = {"context": "RPC_CONTEXT_142", "rule": "RPC_RULE_142", "skill": "RPC_SKILL_142"}
			if populated:
				(project / ".omp/rules").mkdir(parents=True)
				(project / ".omp/skills/probe").mkdir(parents=True)
				(project / "AGENTS.md").write_text("RPC_SHADOWED_142\n")
				(project / ".omp/AGENTS.md").write_text(markers["context"] + "\n")
				(project / ".omp/rules/probe.md").write_text("---\nalwaysApply: true\n---\n" + markers["rule"])
				(project / ".omp/skills/probe/SKILL.md").write_text(
					"---\nname: probe\ndescription: " + markers["skill"] + "\n---\nProbe body.\n")
			mock = MockModel("ack", loop=True)
			self.addCleanup(mock.close)
			(data / "models.toml").write_text(MODELS_TOML.format(port=mock.port))
			(data / "config.toml").write_text(CONFIG_TOML)
			frames = queue.Queue()
			with (root / "stderr.log").open("w+") as errors:
				process = subprocess.Popen(
					[str(OMP_BINARY), mode, "--yolo", "--no-tools", "--model", "mock",
					 "--project", str(project), "--session-dir", str(sessions), *flags],
					cwd=project, env={**os.environ, "HOME": str(home), "OMP_CONFIG_DIR": str(config),
					 "OMP_DATA_DIR": str(data)}, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
					stderr=errors, text=True, start_new_session=True,
				)
				def read_frames():
					try:
						for line in process.stdout:
							frames.put(json.loads(line))
					except Exception as error:
						frames.put(error)
					finally:
						frames.put(None)
				reader = threading.Thread(target=read_frames, daemon=True)
				reader.start()
				def receive(predicate):
					deadline = time.monotonic() + 45
					while True:
						frame = frames.get(timeout=max(0.01, deadline - time.monotonic()))
						if frame is None or isinstance(frame, Exception):
							errors.flush()
							self.fail(f"RPC closed before expected frame: {frame}; {(root / 'stderr.log').read_text()}")
						if predicate(frame):
							return frame
				def request(kind, **params):
					process.stdin.write(json.dumps({"id": kind, "type": kind, **params}) + "\n")
					process.stdin.flush()
					response = receive(lambda frame: frame.get("type") == "response" and frame.get("id") == kind)
					self.assertTrue(response["success"], response)
					return response.get("data", {})
				def prompt(stage):
					before = len(mock.state()["captures"])
					request("prompt", message="Reply ack.")
					receive(lambda frame: frame.get("type") == "agent_end")
					captures = mock.state()["captures"][before:]
					self.assertTrue(captures, stage)
					for capture in captures:
						wire = json.dumps(capture["messages"])
						for name, marker in markers.items():
							expected = populated and not (name == "context" and "--no-context-files" in flags) and not (name == "rule" and "--no-rules" in flags)
							self.assertEqual(marker in wire, expected, (mode, flags, stage, name, wire))
						self.assertNotIn("RPC_SHADOWED_142", wire, "native context must retain discovery precedence")
				try:
					initial = request("get_state")["sessionFile"]
					prompt("launch")
					original = Path(initial).read_bytes()
					request("new_session")
					created = request("get_state")["sessionFile"]
					self.assertNotEqual(initial, created)
					prompt("new_session")
					request("switch_session", sessionPath=initial)
					prompt("switch_session")
					request("quit")
					process.wait(timeout=15)
					self.assertEqual(process.returncode, 0)
					self.assertTrue(Path(initial).read_bytes().startswith(original), "switch appends; earlier journal bytes stay intact")
					for path in (initial, created):
						journal = Path(path).read_text()
						self.assertIn("prompt.facts", journal)
						for name, marker in markers.items():
							expected = populated and not (name == "context" and "--no-context-files" in flags) and not (name == "rule" and "--no-rules" in flags)
							self.assertEqual(marker in journal, expected, (path, name))
				finally:
					if process.poll() is None:
						os.killpg(process.pid, signal.SIGKILL)
						process.wait(timeout=15)
					process.stdin.close()
					process.stdout.close()
					reader.join(timeout=5)
			mock.close()

	def test_discovery_survives_rpc_and_rpc_ui_transitions(self):
		for mode in ("rpc", "rpc-ui"):
			for flags in ((), ("--no-context-files",), ("--no-rules",)):
				with self.subTest(mode=mode, flags=flags):
					self.exercise(mode, flags)

	def test_unrelated_empty_project_does_not_inherit_prior_facts(self):
		self.exercise("rpc")
		self.exercise("rpc", populated=False)


if __name__ == "__main__":
	unittest.main()
