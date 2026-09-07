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

Credentials and gateway settings are inherited from the invoking environment. Per-trial config and session storage are separate from the scored project. Extra argv must not override adapter-owned model, project, session or output-mode flags. Do not use `--no-tools` for an edit benchmark. Unix process groups are owned and killed on timeout, overflow and completion; Windows is rejected until equivalent job-object ownership exists. Stdout/stderr are bounded to 64 MiB combined.

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
