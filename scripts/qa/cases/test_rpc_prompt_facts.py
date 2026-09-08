"""Production RPC discovery survives session transitions (#142).

Uses the shared HTTP mock, but launches the actual RPC/RPC-UI entrypoint.
No fabricated SessionHome can accidentally repair the constructor under test.
"""

from contextlib import contextmanager
import json
import os
from pathlib import Path
import queue
import signal
import selectors
import subprocess
import sys
import tempfile
import threading
import time
import unittest
import traceback

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from harness import MODELS_TOML, OMP_BINARY, MockModel  # noqa: E402

COVERS: dict[str, list[str]] = {"py": [], "rpc": []}


def stop_fixture_daemons(project):
	"""Stop only detached envd groups whose executable and root match this fixture."""
	prefixes = tuple(f"{binary} envd --root {root} --state-dir "
		for binary in {str(OMP_BINARY), str(OMP_BINARY.resolve())}
		for root in {str(project), str(project.resolve())})
	listing = subprocess.run(["ps", "-axo", "pid=,command="], check=True,
		capture_output=True, text=True, timeout=5).stdout
	stopped = []
	for line in listing.splitlines():
		parts = line.strip().split(None, 1)
		if len(parts) != 2 or not parts[1].startswith(prefixes):
			continue
		pid, command = int(parts[0]), parts[1]
		# Revalidate the exact process immediately before signalling its owned group.
		current = subprocess.run(["ps", "-p", str(pid), "-o", "command="],
			capture_output=True, text=True, timeout=5).stdout.strip()
		if current != command:
			continue
		try:
			if os.getpgid(pid) != pid:
				raise AssertionError(f"fixture envd {pid} does not own its process group")
			os.killpg(pid, signal.SIGKILL)
		except ProcessLookupError:
			continue
		stopped.append({"pid": pid, "command": command})
		deadline = time.monotonic() + 5
		while True:
			remaining = subprocess.run(["ps", "-p", str(pid), "-o", "command="],
				capture_output=True, text=True, timeout=5).stdout.strip()
			if remaining != command:
				break
			if time.monotonic() >= deadline:
				raise AssertionError(f"fixture envd {pid} survived group cleanup")
			time.sleep(0.05)
	return stopped


@contextmanager
def evidence_run(root, mode, flags, populated):
	"""Export only this synthetic fixture's captures and journals, including failures."""
	evidence = {"id": f"{mode}-{root.name}", "mode": mode, "flags": list(flags),
		"populated": populated, "status": "passed", "failure": None, "rows": [], "files": [],
		"shutdown_contract": "quit acknowledged, then client stdin EOF, then successful process exit"}
	try:
		yield evidence
	except BaseException:
		evidence["status"] = "failed"
		evidence["failure"] = traceback.format_exc()
		raise
	finally:
		if destination := os.environ.get("OMP_RPC_FACTS_EVIDENCE_DIR"):
			output = Path(destination)
			run = output / evidence["id"]
			run.mkdir(parents=True, exist_ok=True)
			mock = evidence.pop("mock", None)
			def save(relative, body):
				path = run / relative
				path.parent.mkdir(parents=True, exist_ok=True)
				path.write_bytes(body)
				evidence["files"].append(str(path.relative_to(output)))
			save("captures.json", json.dumps(mock.state()["captures"] if mock else [], indent=2).encode())
			# Never follow a session path supplied by RPC outside the temporary fixture.
			for journal in sorted((root / "sessions").glob("*.oms")):
				if not journal.is_symlink():
					save(f"journals/{journal.name}", journal.read_bytes())
			stderr = root / "stderr.log"
			save("stderr.log", stderr.read_bytes() if stderr.exists() else b"")
			if evidence["failure"]:
				save("failure.txt", evidence["failure"].encode())
			evidence["files"].append(f"{evidence['id']}/result.json")
			(run / "result.json").write_text(json.dumps(evidence, indent=2) + "\n")
			manifest_path = output / "manifest.json"
			manifest = json.loads(manifest_path.read_text()) if manifest_path.exists() else {"runs": []}
			manifest["runs"].append(evidence)
			manifest_path.write_text(json.dumps(manifest, indent=2) + "\n")
			summary = ["# RPC prompt-facts acceptance evidence", "",
				"Captures come only from the local mock. Journals come only from temporary fixture sessions.", "",
				"Shutdown proof requires the client to close stdin after the quit acknowledgement. It does not prove exit while stdin remains open.", ""]
			for result in manifest["runs"]:
				summary.extend([f"## {result['id']}: {result['status']}", "",
					f"Mode: `{result['mode']}`; flags: `{' '.join(result['flags']) or '(none)'}`; populated project: `{result['populated']}`.", "",
					"| Transition / source | Capture | Marker | Expected present | Actual present | Result |",
					"| --- | --- | --- | --- | --- | --- |"])
				for row in result["rows"]:
					actual = row["actual"]
					outcome = "not observed" if actual is None else "pass" if actual == row["expected"] else "FAIL"
					summary.append(f"| {row['stage']} | {row.get('capture', '—')} | {row['marker']} | {row['expected']} | {actual if actual is not None else 'not observed'} | {outcome} |")
				if result["failure"]:
					summary.extend(["", "Failure diagnostic:", "", "```text", result["failure"].replace("```", "---"), "```"])
				summary.extend(["", "Files: " + ", ".join(f"`{path}`" for path in result["files"]), ""])
			(output / "summary.md").write_text("\n".join(summary) + "\n")


def marker_rows(stage, markers, flags, populated, content=None, capture=None):
	rows = []
	for name, marker in {**markers, "shadowed": "RPC_SHADOWED_142"}.items():
		expected = populated and name != "shadowed" and not (name == "context" and "--no-context-files" in flags) and not (name == "rule" and "--no-rules" in flags)
		row = {"stage": stage, "marker": marker, "expected": expected,
			"actual": None if content is None else marker in content}
		if capture is not None:
			row["capture"] = capture
		rows.append(row)
	return rows


class RpcPromptFacts(unittest.TestCase):
	def exercise(self, mode, flags=(), populated=True):
		with tempfile.TemporaryDirectory(prefix="omp-rpc-facts-") as directory:
			root = Path(directory)
			with evidence_run(root, mode, flags, populated) as evidence:
				home, project, data, config, sessions = [root / name for name in
					("home", "home/project", "data", "config", "sessions")]
				for path in (home, project, data, config, sessions):
					path.mkdir(parents=True, exist_ok=True)
				markers = {"context": "RPC_CONTEXT_142", "rule": "RPC_RULE_142", "skill": "RPC_SKILL_142"}
				for stage in ("launch", "new_session", "switch_session", "journal_initial", "journal_created"):
					evidence["rows"].extend(marker_rows(stage, markers, flags, populated))
				if populated:
					(project / ".omp/rules").mkdir(parents=True)
					(project / ".omp/skills/probe").mkdir(parents=True)
					(project / "AGENTS.md").write_text("RPC_SHADOWED_142\n")
					(project / ".omp/AGENTS.md").write_text(markers["context"] + "\n")
					(project / ".omp/rules/probe.md").write_text("---\nalwaysApply: true\n---\n" + markers["rule"])
					(project / ".omp/skills/probe/SKILL.md").write_text(
						"---\nname: probe\ndescription: " + markers["skill"] + "\n---\nProbe body.\n")
				mock = MockModel("ack", loop=True)
				evidence["mock"] = mock
				self.addCleanup(mock.close)
				(data / "models.toml").write_text(MODELS_TOML.format(port=mock.port))
				(config / "config.cfg").write_text(
					"ai_retry_max_retries 1\nai_retry_base_delay_ms 50\nai_retry_max_delay_ms 200\n")
				environment = dict(os.environ)
				for name in ("OMP_PROFILE", "OMP_CONFIG_FILES", "OMP_CODING_AGENT_SESSION_DIR"):
					environment.pop(name, None)
				environment.update({
					"HOME": str(home), "OMP_CONFIG_DIR": str(config), "OMP_DATA_DIR": str(data),
					"OMP_CACHE_DIR": str(root / "cache"), "OMP_STATE_DIR": str(root / "state"),
				})
				frames = queue.Queue()
				with (root / "stderr.log").open("w+") as errors:
					rpc_deadline = time.monotonic() + 180
					process = subprocess.Popen(
						[str(OMP_BINARY), mode, "--yolo", "--no-tools", "--model", "mock",
						 "--project", str(project), "--session-dir", str(sessions), "--max-time", "180s", *flags],
						cwd=project, env=environment, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
						stderr=errors, text=True, start_new_session=True,
					)
					stop_reader = threading.Event()
					def read_frames():
						pending = bytearray()
						try:
							with selectors.DefaultSelector() as readable:
								readable.register(process.stdout, selectors.EVENT_READ)
								while not stop_reader.is_set():
									if not readable.select(timeout=0.2):
										continue
									chunk = os.read(process.stdout.fileno(), 64 * 1024)
									if not chunk:
										if pending:
											frames.put(json.loads(pending))
										break
									pending.extend(chunk)
									while b"\n" in pending:
										line, _, remainder = pending.partition(b"\n")
										pending = bytearray(remainder)
										frames.put(json.loads(line))
						except Exception as error:
							frames.put(error)
						finally:
							frames.put(None)
					reader = threading.Thread(target=read_frames, daemon=True)
					reader.start()
					def receive(predicate):
						deadline = min(time.monotonic() + 45, rpc_deadline)
						while True:
							remaining = deadline - time.monotonic()
							if remaining <= 0:
								self.fail(f"RPC timed out waiting for a frame; {(root / 'stderr.log').read_text()}")
							try:
								frame = frames.get(timeout=remaining)
							except queue.Empty:
								self.fail(f"RPC timed out waiting for a frame; {(root / 'stderr.log').read_text()}")
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
						terminal = receive(lambda frame: frame.get("type") == "agent_end")
						self.assertNotIn("error", terminal, (stage, terminal))
						self.assertIs(terminal.get("isTerminal"), True, (stage, terminal))
						self.assertIs(terminal.get("cancelled"), False, (stage, terminal))
						self.assertEqual(terminal.get("text"), "ack", (stage, terminal))
						captures = mock.state()["captures"][before:]
						self.assertTrue(captures, stage)
						evidence["rows"] = [row for row in evidence["rows"] if row["stage"] != stage]
						for index, capture in enumerate(captures, start=before):
							wire = json.dumps(capture["messages"])
							evidence["rows"].extend(marker_rows(stage, markers, flags, populated, wire, index))
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
						process.stdin.close()
						process.wait(timeout=15)
						self.assertEqual(process.returncode, 0)
						self.assertTrue(Path(initial).read_bytes().startswith(original), "switch appends; earlier journal bytes stay intact")
						for stage, path in (("journal_initial", initial), ("journal_created", created)):
							journal = Path(path).read_text()
							evidence["rows"] = [row for row in evidence["rows"] if row["stage"] != stage]
							evidence["rows"].extend(marker_rows(stage, markers, flags, populated, journal))
							self.assertIn("prompt.facts", journal)
							for name, marker in markers.items():
								expected = populated and not (name == "context" and "--no-context-files" in flags) and not (name == "rule" and "--no-rules" in flags)
								self.assertEqual(marker in journal, expected, (path, name))
					finally:
						# Reap the invocation group even if its leader already exited.
						try:
							os.killpg(process.pid, signal.SIGKILL)
						except ProcessLookupError:
							pass
						try:
							process.wait(timeout=15)
						finally:
							stop_reader.set()
							reader.join(timeout=5)
							try:
								process.stdin.close()
							except BrokenPipeError:
								pass
							process.stdout.close()
							evidence["daemon_cleanup"] = stop_fixture_daemons(project)
							self.assertFalse(reader.is_alive(), "RPC stdout reader did not stop")
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
