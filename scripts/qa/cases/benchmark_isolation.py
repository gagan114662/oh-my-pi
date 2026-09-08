#!/usr/bin/env python3
"""Real OMP context-isolation proof through the existing benchmark adapter.

Requires an actual build receipt; never stamps an existing binary. Uses only
loopback synthetic provider replies. No model-quality or sandbox claim.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import time
import traceback

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from harness import MockModel, MODELS_TOML

AMBIENT = "BENCH_AMBIENT_HOME_MUST_NOT_BLEED_684f"
EARLIER = "BENCH_PREVIOUS_TRIAL_MUST_NOT_BLEED_109c"
ADAPTER = Path(__file__).resolve().parents[2] / "metaharness/adapter.ts"


def stop_owned_daemons(binary, projects):
    """envd detaches; stop only exact executable/root invocations from this proof."""
    prefixes = tuple(f"{binary} envd --root {project} --state-dir " for project in projects)
    listing = subprocess.run(["ps", "-axo", "pid=,command="], capture_output=True,
                             text=True, check=True, timeout=5).stdout
    for line in listing.splitlines():
        parts = line.strip().split(None, 1)
        if len(parts) != 2 or not parts[1].startswith(prefixes):
            continue
        pid, command = int(parts[0]), parts[1]
        current = subprocess.run(["ps", "-p", str(pid), "-o", "command="],
                                 capture_output=True, text=True, timeout=5).stdout.strip()
        if current != command:
            continue
        try:
            if os.getpgid(pid) != pid:
                raise AssertionError("owned envd did not create its own process group")
            os.killpg(pid, signal.SIGKILL)
            deadline = time.monotonic() + 5
            while True:
                remaining = subprocess.run(["ps", "-p", str(pid), "-o", "command="],
                                           capture_output=True, text=True, timeout=5).stdout.strip()
                if remaining != command:
                    break
                if time.monotonic() >= deadline:
                    raise AssertionError("owned envd survived cleanup")
                time.sleep(0.05)
        except ProcessLookupError:
            pass


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("source", "binary", "provenance", "output"):
        parser.add_argument("--" + name, type=Path, required=True)
    parser.add_argument("--bun", default="bun")
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    binary = args.binary.resolve()
    experiment = output / "experiment"
    projects = [output / "control/project"] + [experiment / f"trial-{i}/project" for i in range(4)]
    rows = []
    captures = []

    class MutatingMock(MockModel):
        def _sse_events(self, reply, ordinal, request):
            # Change the first trial's home before its provider reply completes.
            # The next trial must discover from a different home, even though
            # this mutation is outside the project inventory being scored.
            if ordinal == 2:  # request 1 is the positive control
                marker = experiment / "trial-0/home/.claude/CLAUDE.md"
                marker.parent.mkdir(parents=True, exist_ok=True)
                marker.write_text(EARLIER)
            return super()._sse_events(reply, ordinal, request)

    try:
        with MutatingMock("done", loop=True) as mock:
            models = output / "models.toml"
            models.write_text(MODELS_TOML.format(port=mock.port))
            ambient = output / "ambient-home"
            (ambient / ".claude").mkdir(parents=True)
            (ambient / ".claude/CLAUDE.md").write_text(AMBIENT)
            (output / "input").mkdir()
            (output / "expected").mkdir()
            arm = {"id": "production", "source": str(args.source.resolve()), "binary": str(binary),
                   "provenance": str(args.provenance.resolve()), "args": ["--no-ext"]}
            manifest = {"version": 1, "model": "mock/mock", "baseline": arm, "candidate": arm,
                        "tasks": [{"id": "context", "name": "Home context isolation",
                                   "input": str(output / "input"), "expected": str(output / "expected"),
                                   "prompt": "Reply with done."}],
                        "repetitions": 1, "timeoutMs": 30000, "output": str(experiment),
                        "verifier": {"id": "exact-bytes-v1", "mode": "exact"},
                        "provider": {"models": {"path": str(models),
                                     "sha256": hashlib.sha256(models.read_bytes()).hexdigest()}}}
            spec = output / "manifest.json"
            spec.write_text(json.dumps(manifest))
            script = output / "run.ts"
            script.write_text("import {execute,runExperiment,trialEnvironment,verifyArm} from " + json.dumps(str(ADAPTER)) + ";\n" + r'''
import {mkdir,readFile,writeFile} from "node:fs/promises";
import {join,dirname} from "node:path";
const spec = JSON.parse(await readFile(process.argv[2],"utf8"));
const root=dirname(process.argv[2]);
await verifyArm(spec.baseline);
const control=join(root,"control");
const env=await trialEnvironment(control);
env.HOME=join(root,"ambient-home");
env.USERPROFILE=env.HOME;
await writeFile(join(env.OMP_DATA_DIR!,"models.toml"),await readFile(spec.provider.models.path));
await mkdir(join(control,"project"));
const result=await execute([spec.baseline.binary,"--mode","json","--model",spec.model,
 "--project",join(control,"project"),"--session-dir",join(control,"sessions"),
 "--no-ext","--no-tools","Reply with done."],join(control,"project"),30000,undefined,env);
await writeFile(join(root,"control.json"),JSON.stringify(result));
if(result.exitCode!==0 || result.timedOut || result.error) throw new Error("Production positive control failed");
await runExperiment(spec);
''')
            env = dict(os.environ, HOME=str(ambient), OMP_PROFILE="ambient-profile-must-not-propagate")
            process = subprocess.Popen([args.bun, str(script), str(spec)], env=env,
                                       stdout=subprocess.PIPE, stderr=subprocess.PIPE, start_new_session=True)
            try:
                stdout, stderr = process.communicate(timeout=180)
            except subprocess.TimeoutExpired as error:
                (output / "stdout.log").write_bytes(error.stdout or b"")
                (output / "stderr.log").write_bytes(error.stderr or b"")
                raise
            finally:
                try:
                    os.killpg(process.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
                process.wait(timeout=5)
                captures = mock.state()["captures"]
            (output / "stdout.log").write_bytes(stdout)
            (output / "stderr.log").write_bytes(stderr)
            assert process.returncode == 0, f"production runner exit {process.returncode}; see stderr.log"
            assert len(captures) == 5, f"expected control plus four trials, got {len(captures)}"
            visible = [json.dumps(capture.get("messages")) for capture in captures]
            assert AMBIENT in visible[0], "positive control did not load home context"
            rows.append(("positive control sees ambient context", True))
            for index, text in enumerate(visible[1:]):
                assert AMBIENT not in text, f"ambient context leaked into trial {index}"
                assert EARLIER not in text, f"previous home leaked into trial {index}"
                rows.append((f"trial {index} excludes ambient and previous home", True))
            assert (experiment / "trial-0/home/.claude/CLAUDE.md").read_text() == EARLIER
            runs = json.loads((experiment / "runs.json").read_text())
            assert len(runs) == 4 and all(run["success"] for run in runs), "real production trials failed"
    except BaseException:
        (output / "failure.txt").write_text(traceback.format_exc())
        raise
    finally:
        (output / "captures.json").write_text(json.dumps(captures, indent=2))
        (output / "summary.md").write_text("# Production benchmark state isolation\n\n"
            "Synthetic loopback provider; no quality, full-tree accounting or sandbox claim.\n\n"
            "| Check | Passed |\n|---|---|\n" + "".join(f"| {name} | {passed} |\n" for name, passed in rows)
            + "\nA failure.txt file means this run failed; partial rows are not success.\n")
        try:
            stop_owned_daemons(binary, projects)
        except BaseException:
            (output / "cleanup-failure.txt").write_text(traceback.format_exc())
            raise


if __name__ == "__main__":
    main()
