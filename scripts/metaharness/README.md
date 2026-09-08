# OMP2 production-process edit adapter

This opt-in Bun adapter launches an explicitly configured OMP binary through its existing `--mode json` interface, copies each input fixture into a fresh workspace, and checks the resulting files in the adapter process. It never accepts the agent's claim of success as a score. The installed `omp bench arm` command embeds this adapter; ordinary `omp bench MODEL` still runs the inference-throughput benchmark. Bun and Git are explicit host prerequisites. No runtime is installed automatically and no checkout scripts are needed by the installed command.

The adapter is **not yet completion evidence for issue #19**. The checked-in tests execute a clearly labeled fixture executable; they verify adapter plumbing, not OMP production behavior or harness improvement. An actual OMP A/A + A/B run, compiled CLI validation and CI artifact publication remain required. The protected-evaluator authority in #50 remains required before running adversarial candidate code.

## Commands

Run the build wrapper **around a real build**, with binary and provenance paths outside source trees where possible:

```sh
omp bench build /checkout/omp2 /checkout/omp2/target/debug/omp /artifacts/base-build.json -- just build
omp bench arm --manifest /artifacts/experiment.json --same-commit
# Compare prepared arms; baseline must resolve to HEAD~1 in candidate.source:
omp bench arm --manifest /artifacts/comparison.json --base HEAD~1
# Direct development runner uses the same embedded source:
bun scripts/metaharness-omp2.ts run /artifacts/experiment.json
bun test scripts/metaharness/adapter.test.ts
```

`--same-commit` requires identical source and binary hashes for both prepared arms. `--base REF` verifies the prepared baseline commit against the ref in the candidate checkout; it does not guess build commands or create a worktree. Both switches require a manifest and are mutually exclusive. Every experiment runs an A/A phase before A/B. Use `--bun /absolute/path/to/bun` on either subcommand when Bun is not on PATH.

The wrapper records commit, working-source SHA-256, binary SHA-256 and build argv. It rejects failed builds, source changes during the build, and binaries not freshly emitted by the command. An incremental no-op build deliberately fails freshness validation; rebuild the executable rather than stamping an old one. Every trial checks hashes and source/binary modification ordering again, including after execution. There is no CLI to stamp an existing binary. This is an external build receipt, not a signed compiler attestation.

`git ls-files --cached --others --exclude-standard` defines the source inventory. Tracked working bytes, deletions, untracked nonignored files, and commit identity contribute; ignored build outputs do not. Source inputs must be regular files. For a Git tree with deliberate symlinks, define and review a source-inventory extension before running it.

## Experiment manifest

All paths should be absolute. Output must be an empty directory outside the source and fixture trees. The same exact model identifier is passed to all arms. `latest` aliases are rejected; the operator must select an immutable provider snapshot where the provider supports one.

```json
{
  "version": 1,
  "model": "provider/exact-model-snapshot-id",
  "baseline": {
    "id": "baseline",
    "source": "/checkout/baseline",
    "binary": "/checkout/baseline/target/debug/omp",
    "provenance": "/artifacts/baseline-build.json",
    "args": ["--no-ext"]
  },
  "candidate": {
    "id": "candidate",
    "source": "/checkout/candidate",
    "binary": "/checkout/candidate/target/debug/omp",
    "provenance": "/artifacts/candidate-build.json",
    "args": ["--no-ext"]
  },
  "tasks": [{
    "id": "task-001",
    "name": "Repair a selected edit fixture",
    "input": "/fixtures/task-001/input",
    "expected": "/fixtures/task-001/expected",
    "prompt": "The frozen task prompt from the selected benchmark corpus."
  }],
  "repetitions": 2,
  "timeoutMs": 120000,
  "output": "/artifacts/experiment-001",
  "verifier": {"id": "exact-bytes-v1", "mode": "exact"}
}
```

Each trial receives fresh HOME/USERPROFILE, OMP config/data/state/cache, all XDG
roots (including an owner-only runtime directory), temporary directories and
session storage outside the scored project. Ambient OMP profiles, user context,
Python/Node/shell injection variables and undeclared credentials are not passed.
PATH, LANG, LC_ALL and TZ remain host execution prerequisites.

Declare provider environment **names**, never secret values, in the manifest:

```json
"provider": {
  "env": ["OMP_ANTHROPIC_API_KEY"],
  "models": {"path": "/artifacts/models.toml", "sha256": "SHA256_OF_NON_SECRET_CATALOG"}
}
```

Both fields are optional. Built-in Anthropic routing needs no custom catalog.
Declared credentials must be nonempty in the invoking environment; supported
names end in `_API_KEY`, `_ACCESS_TOKEN` or `_BASE_URL`, plus explicitly declared
HTTP(S)/ALL/NO_PROXY inputs. Values are passed directly to the child environment
and are not added to manifests or reports. The optional non-secret models.toml
is hash-checked, retained in memory and copied to each trial's fresh data root.
Do not put credentials in that catalog: trial files are evidence artifacts.
Other provider setup must be added explicitly, not recovered from the caller's
home. Provider-side prompt caching is not reset by filesystem isolation; its
order effects remain part of the interleaved A/A measurement.
 Extra argv must not override adapter-owned model, project, session or output-mode flags. Do not use `--no-tools` for an edit benchmark. Unix process groups are owned and killed on timeout, overflow and completion; Windows is rejected until equivalent job-object ownership exists. Stdout/stderr are bounded to 64 MiB combined.

The complete A/A phase runs before A/B. Within each task/repetition, the two arms alternate; which arm is first also alternates across tasks/repetitions. A/A uses the exact baseline binary and argv for both arms. It measures the observed run-window noise floor, not a formal confidence interval. Every comparison row includes the absolute A/A delta. Effects at or below that floor are unresolved; success-rate effects below 5 percentage points on 20 or fewer tasks are also unresolved. No promotion occurs automatically.

## Oracle and compatibility

Exact mode checks the entire file inventory and file bytes, including unrelated files. Missing files, added files, symlinks, special files, and changed unrelated content fail verification. Expected trees and source/build receipts are rechecked after every trial. This detects tampering; it does **not** prevent an unsandboxed candidate process from changing its evaluator or artifacts. Do not equate these integrity checks with #50.

The v1 oracle in `origin/main:packages/typescript-edit-benchmark/src/verify.ts` compares formatted files. To use that oracle, set `mode: "formatted"` and supply an absolute formatter executable, its SHA-256, and argv. The formatter receives file bytes on stdin; `{path}` in argv expands to the full file name. Stdout is the normalized content. A formatter error fails the run rather than silently downgrading the verifier.

Pin the formatter's complete dependency closure externally; hashing only the interpreter executable does not pin a JS module, config file or shared library. Copying the v1 Prettier wrapper without its pinned package/lockfile is insufficient. The exact-byte oracle is intentionally a different, stricter oracle: do not compare its scores to v1 formatted-equivalence baselines. A same-window comparison must use the same verifier id, executable and protected dependencies.

The normalized `result.json` consumed by v1 `packages/metaharness/src/benchmarks.ts` is emitted for each arm. Its `tasks[].runs[]` includes `runIndex`, success/error, duration, tokens and toolCalls; summary includes totalRuns, successfulRuns, taskSuccessRate, editSuccessRate and totalTokens. The root report additionally carries the requested legacy metric vector and all-run accounting.

Compatibility reference: `origin/main` was inspected at commit `72b2d32e5f83f65d25b5fbe24fc98039c4dd3bc6`. Selection matches that v1 edit runner: successful runs first, non-ghost runs next, lower total tokens next, earlier repetition last. Primary compatibility metrics use one selected run per task. Ghost/timeout/retry counts also appear separately; all-run totals include every attempt, so selected-run reporting cannot hide failed work. V1's exhausted-transport classification requires an explicit `Timeout exhausted` retry-end diagnostic; the adapter does not infer it from an arbitrary failed edit. `allRuns.timedOut` always counts every adapter timeout, including ghosts.

`cacheRead`, `cacheWrite`, `cacheReadTokens`, and `cacheWriteTokens` are token counts; `spend` is total USD. Cache hit ratio uses consistently averaged quantities: cache-read tokens / (cache-read tokens + uncached input tokens). Null means no denominator, not zero cache effectiveness. OMP print output reports total cost but not reliable per-cache monetary allocation; no allocation is invented. Canonical serialized tool-argument character counts are UTF-16 length, consistent with JS string counting; they are not raw provider wire bytes. Model observations are checked against the pinned requested presentation identity, but provider-side immutable-snapshot attestation is not exposed by this interface.

## Evidence artifacts

- `report.json`: manifest, build identities, fixture hashes, verifier, summaries, A/A deltas and limitations.
- `summary.md`: browser-readable comparison table, including every cache metric and staleness result.
- `runs.json`: continuously saved all-run outcomes, including failures and timeouts.
- `trial-N/trace.jsonl`, `stderr.txt`, `project/`, `sessions/`: replay/review material. Review artifacts for credentials and sensitive source before publication.
- `{aa-a,aa-b,baseline,candidate}/result.json`: normalized metaharness input.

Production qualification must include deliberate stale-build, corrupted-oracle, wrong-model and failed-task runs, actual OMP traces, and uploaded CI artifacts. Fixture-command unit tests alone do not satisfy those requirements.

Malformed telemetry preserves already observed usage, marks the row incomplete, and forces every comparison unresolved. Usage totals are lower bounds when any row is incomplete; unknown usage is never reported as proven zero cost.

## Optional official RULER answer scoring

`answerVerifier` adds answer scoring to the same process runner. Filesystem verification remains active: for read-only answer tasks, use matching empty input/expected fixture directories. This integration supports the official RULER synthetic metric families; it is **not a claim of completing RULER, reproducing its inference protocol, or supporting OOLONG**.

Obtain the two official source files `scripts/eval/evaluate.py` and `scripts/eval/synthetic/constants.py` from NVIDIA/RULER revision `c3f5e3b4f87f97e048793bb510a3a6b19a46bf3a`, retaining their paths under `source`. The adapter checks their hardcoded SHA-256 digests and executes the upstream metric function and preprocessing function unchanged. It extracts the preprocessing function from the upstream AST to avoid importing the heavyweight upstream CLI, which imports NeMo/pandas and may download NLTK data. Only the Python standard library is needed; nothing is downloaded by this adapter. Python executable bytes are pinned, but its standard-library installation is not independently attested.

Add this optional object to the manifest:

```json
{
  "answerVerifier": {
    "kind": "ruler",
    "source": "/benchmarks/RULER",
    "revision": "c3f5e3b4f87f97e048793bb510a3a6b19a46bf3a",
    "python": "/absolute/path/to/python3",
    "pythonSha256": "SHA256_OF_PYTHON_EXECUTABLE",
    "family": "niah",
    "dataset": {
      "path": "/benchmarks/generated/niah_single_1/validation.jsonl",
      "sha256": "SHA256_OF_DATASET_BYTES",
      "origin": "IMMUTABLE_DATASET_OR_GENERATOR_SOURCE_URL",
      "generationCommand": ["EXACT", "GENERATION", "COMMAND"]
    }
  }
}
```

Each task additionally specifies `datasetIndex`, matching an integer `index` in the pinned JSONL dataset. Its `prompt` must exactly equal that record's `input` plus its optional string `answer_prefix`, matching upstream `scripts/pred/call_api.py`. Missing prefixes contribute an empty string; present non-string prefixes are rejected. Preserve spacing and newlines exactly. Records require nonempty string `outputs` references. References are never appended to the prompt or copied into the task workspace. Dataset source and generation command are recorded operator declarations; hashes prove which bytes were scored, not that those declarations are authentic. Preserve official generation parameters (including tokenizer, length, seed and task configuration) in the command/provenance, and identify any task subset in published results.

Scoring extracts only text blocks from the last settled assistant `message_end` before terminal `agent_end`; thinking, tool output, earlier replies and duplicated terminal messages are excluded. Truncated, aborted, erroneous and unfinished tool-call replies are rejected. Each run retains `answer` and `answerScore`; 100 is required for binary complete-success. `report.json.answerEvaluation` separately reports the **official batch score across all attempts**, including empty predictions for failed runs, rather than selecting each task's best retry. A score of 50 remains 50; it is not rounded into a pass. These scores describe an OMP agent-harness protocol using RULER records, not automatically a leaderboard-comparable RULER run.

The deterministic tests use clearly labeled local contract records, not standardized evaluation data:

```sh
OMP_RULER_SOURCE=/benchmarks/RULER OMP_RULER_PYTHON=/absolute/path/to/python3 \
  bun test scripts/metaharness/adapter.test.ts scripts/metaharness/ruler.test.ts
```

Without `OMP_RULER_SOURCE`, the two official-source execution tests are explicitly skipped; final-answer extraction and the existing adapter tests still run. A real standardized result requires the immutable official/generated dataset, reviewed generation provenance, actual production OMP binaries, provider access, and published run artifacts. The manifest does not currently enforce an explicit maximum generated-answer token count; generation settings must be established through supported, verified arm arguments/configuration, and differences from the upstream inference protocol must be disclosed. Evaluator isolation and descendant cost accounting remain separate limitations.


## Production state-isolation regression

After a real build wrapped by `omp bench build`, run:

```sh
python3 scripts/qa/cases/benchmark_isolation.py \
  --source /checkout/omp2 --binary /checkout/omp2/target/debug/omp \
  --provenance /artifacts/build.json --output /artifacts/state-isolation
```

The output directory must not exist. This uses the existing adapter and shared
loopback MockModel against the actual OMP binary, with no paid provider calls.
A positive control must expose a synthetic inherited home-context marker to the
provider. The four real A/A+A/B trials must exclude it; the mock also plants a
second marker in trial zero's home before its response finishes, and later
trials must exclude that marker. Every trial must complete successfully. Captures,
traces, provenance, per-trial results and summary.md remain readable on failure.
The checker explicitly stops only detached envd processes whose executable and
project root match its owned fixtures.

This regression is authored but has not yet run against a compiled production
binary. Bun adapter tests exercise environment isolation and catalog copying;
they do not replace that runtime proof. Filesystem state isolation is not an
access-control sandbox (#50), complete descendant accounting (#62), or protected
candidate promotion (#67); all three remain open.
