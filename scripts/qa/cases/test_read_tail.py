#!/usr/bin/env python3
"""Exercise #31 through the actual binary and mock-provider tool-result messages.

The identical checker runs on the recorded pre-fix revision and selected head.
Captures, stdout, stderr, and expected/actual rows are retained even on failure.
"""
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import time
import traceback
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from harness import MODELS_TOML, OMP_BINARY, MockModel, call

OMP_BINARY = Path(os.environ.get("OMP_BINARY", str(OMP_BINARY))).resolve()

COVERS = {"py": [], "rpc": []}

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



class ReadTail(unittest.TestCase):
    def test_production_tail_sources(self):
        output = Path(os.environ.get("OMP_READ_TAIL_EVIDENCE_DIR", "target/read-tail/qa"))
        output.mkdir(parents=True, exist_ok=True)
        rows = []
        with tempfile.TemporaryDirectory(prefix="omp-read-tail-") as temporary:
            root = Path(temporary)
            project = root / "home/project"
            project.mkdir(parents=True)
            (project / "large.txt").write_text("".join(f"TAIL_LINE_{n:06d}\n" for n in range(1, 200001)))
            (project / "short.txt").write_text("OMIT_FIRST\nKEEP_LAST\n")
            (project / "empty.txt").write_text("")
            (project / "directory").mkdir()
            for name in ("aaa.txt", "mmm.txt", "zzz.txt"):
                (project / "directory" / name).write_text(name)
            artifact_code = r"""import omp
ref = await omp.artifacts.put(b'one\ntwo\n', media_type='text/plain')
assert await omp.artifacts.read(ref, 'raw:-2') == 'two\n'
assert await omp.artifacts.read(ref, 'raw:-1') == ''
large = await omp.artifacts.put(''.join(f'ART_{n:06d}\n' for n in range(1, 200001)).encode(), media_type='text/plain')
assert await omp.artifacts.read(large, 'raw:-2') == 'ART_200000\n'
print('ARTIFACT_TAIL_PARITY_OK')
"""
            cases = [
                ("local-large", call("read", path="large.txt:-2"), ["TAIL_LINE_199999", "TAIL_LINE_200000"], ["TAIL_LINE_199998"], False),
                ("local-raw-newline", call("read", path="short.txt:raw:-2"), ["KEEP_LAST"], ["OMIT_FIRST"], False),
                ("local-raw-terminal-empty", call("read", path="short.txt:raw:-1"), [], ["KEEP_LAST", "OMIT_FIRST"], False),
                ("local-short", call("read", path="short.txt:-60"), ["OMIT_FIRST", "KEEP_LAST"], [], False),
                ("local-empty", call("read", path="empty.txt:-60"), [], ["KEEP_LAST"], False),
                ("directory", call("read", path="directory:-1"), ["zzz.txt"], ["aaa.txt", "mmm.txt"], False),
                ("invalid-zero", call("read", path="short.txt:-0"), [], ["KEEP_LAST"], True),
                ("invalid-overflow", call("read", path="short.txt:-18446744073709551616"), [], ["KEEP_LAST"], True),
                ("artifact-python-parity", call("eval", language="py", code=artifact_code), ["ARTIFACT_TAIL_PARITY_OK"], ["AssertionError"], False),
            ]
            for name, reply, required, forbidden, error_expected in cases:
                with self.subTest(source=name):
                    directory = output / name
                    directory.mkdir(parents=True, exist_ok=True)
                    row = {"case": name, "required": required, "forbidden": forbidden,
                           "error_expected": error_expected, "actual": None, "status": "failed"}
                    rows.append(row)
                    process = None
                    stdout = stderr = ""
                    try:
                        data, config, home = root / name / "data", root / name / "config", root / "home"
                        data.mkdir(parents=True)
                        config.mkdir(parents=True)
                        (config / "config.cfg").write_text("ai_retry_max_retries 1\nai_retry_base_delay_ms 50\nai_retry_max_delay_ms 200\n")
                        environment = dict(os.environ)
                        for key in ("OMP_PROFILE", "OMP_CONFIG_FILES", "OMP_CODING_AGENT_SESSION_DIR"):
                            environment.pop(key, None)
                        environment.update(HOME=str(home), OMP_DATA_DIR=str(data), OMP_CONFIG_DIR=str(config),
                                           OMP_CACHE_DIR=str(root / name / "cache"), OMP_STATE_DIR=str(root / name / "state"))
                        with MockModel(reply, "ack") as mock:
                            (data / "models.toml").write_text(MODELS_TOML.format(port=mock.port))
                            process = subprocess.Popen([str(OMP_BINARY), "print", "--mode", "json", "--yolo", "--model", "mock",
                                "--project", str(project), "--session-dir", str(root / name / "sessions"), "--max-time", "90s", "Run the requested tool."],
                                cwd=project, env=environment, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                                stderr=subprocess.PIPE, text=True, start_new_session=True)
                            try:
                                stdout, stderr = process.communicate(timeout=110)
                            except subprocess.TimeoutExpired:
                                os.killpg(process.pid, signal.SIGKILL)
                                stdout, stderr = process.communicate(timeout=10)
                                raise
                            finally:
                                (directory / "captures.json").write_text(json.dumps(mock.state(), indent=2))
                                if stdout or stderr:
                                    (directory / "stdout.jsonl").write_text(stdout)
                                    (directory / "stderr.log").write_text(stderr)
                            self.assertEqual(process.returncode, 0, stderr)
                            captures = mock.state()["captures"]
                            self.assertEqual(len(captures), 2, "tool result must reach the next provider request")
                            results = [message for message in captures[1]["messages"] if message.get("role") == "tool"]
                            self.assertTrue(results, "provider must receive actual tool output")
                            actual = json.dumps(results)
                            row["actual"] = actual
                            events = [json.loads(line) for line in stdout.splitlines() if line.startswith('{')]
                            terminal = [event for event in events if event.get("type") == "agent_end"]
                            self.assertEqual(len(terminal), 1)
                            self.assertIs(terminal[0].get("isTerminal"), True)
                            assistant = [message for message in terminal[0]["messages"] if message.get("role") == "assistant"]
                            self.assertTrue(assistant)
                            self.assertEqual(assistant[-1].get("stopReason"), "stop", assistant[-1])
                            self.assertNotIn("errorMessage", assistant[-1])
                            self.assertEqual("".join(part.get("text", "") for part in assistant[-1]["content"] if part.get("type") == "text"), "ack")
                            tool_results = [tool for event in events if event.get("type") == "turn_end" for tool in event.get("toolResults", [])]
                            self.assertEqual(len(tool_results), 1)
                            row["terminal_validated"] = True
                            row["actual_error"] = bool(tool_results[0].get("isError"))
                            row["semantic_mismatch"] = (row["actual_error"] != error_expected or
                                any(marker not in actual for marker in required) or any(marker in actual for marker in forbidden))
                            self.assertEqual(row["actual_error"], error_expected, tool_results)
                            for marker in required:
                                self.assertIn(marker, actual)
                            for marker in forbidden:
                                self.assertNotIn(marker, actual)
                            row["status"] = "passed"
                    except BaseException:
                        row["failure"] = traceback.format_exc()
                        (directory / "failure.txt").write_text(row["failure"])
                        raise
                    finally:
                        if process is not None:
                            try:
                                os.killpg(process.pid, signal.SIGKILL)
                            except ProcessLookupError:
                                pass
                            process.wait(timeout=10)
                        row["daemon_cleanup"] = stop_fixture_daemons(project)
                        (output / "manifest.json").write_text(json.dumps({"runs": rows}, indent=2) + "\n")
                        lines = ["# Production tail reads", "", "Identical checker expectations apply to before and after builds; failures remain failures.", "",
                                 "| Source | Required | Forbidden | Expected tool error | Actual provider tool result | Result |",
                                 "| --- | --- | --- | --- | --- | --- |"]
                        for item in rows:
                            cells = [item["case"], str(item["required"]), str(item["forbidden"]), str(item["error_expected"]), item["actual"] or "not observed", item["status"]]
                            lines.append("| " + " | ".join(cell.replace("|", "\\|").replace("\n", "<br>") for cell in cells) + " |")
                        (output / "summary.md").write_text("\n".join(lines) + "\n")

if __name__ == "__main__":
    unittest.main()
