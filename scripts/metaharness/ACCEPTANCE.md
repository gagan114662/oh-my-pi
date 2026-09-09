# Issue 19 consolidation audit

Reviewed against the current [issue 19](https://github.com/gagan114662/oh-my-pi/issues/19)
body and acceptance comments on 2026-09-08. This is an implementation/evidence
inventory, not a replacement for the issue's acceptance requirements.

The complete adapter stack must be integrated: the trial-isolation patch depends
on the preceding CLI, build-provenance, external verification, official scorer,
and scorer/reference snapshot changes. Shared `ci.yml` is restored byte-for-byte
to `b59b952172f1f331c2a79a6bc22f6ea25a3e2135`; benchmark-specific checks live in
`benchmark-contracts.yml`. The leaf retains the 10-minute adapter job, the
120-minute native job ceiling, the exact installed CLI parsing test selection,
and the independent app doctest command. It uploads source identity and raw logs
and distinguishes contract checks from benchmark results in the run summary.
The restored shared CI also contains baseline checks absent from this older
branch's source; consolidate the full stack into the current integration source
before interpreting a hosted shared-CI result.

| Requirement | Code present | Remaining evidence or implementation |
|---|---|---|
| Real production binary, freshness refusal | `build`, `sourceIdentity`, `verifyArm`, per-trial rechecks | Fresh compiled CLI check; real OMP A/A and A/B with published source/binary receipts; deliberate stale-source/binary rejection on the hosted source |
| Normalized edit metric vector, ghosts, timeouts and cache usage | `summarize` includes the vector, cache token fields, total spend and hit ratio | Real v1-compatible comparison using contemporaneous matched trials; per-category cache monetary cost is unavailable and must stay labeled |
| A/A noise floor before A/B | `schedule` runs A/A first and counterbalances arm order per task; `compare` reports A/A deltas | Actual model runs, uncertainty/repetition plan, meaningful-effect declaration; deterministic order is not randomized order |
| Small effects unresolved | At most 20 tasks and less than five percentage points remain unresolved; effects no larger than A/A remain unresolved | Genuine taskset trials and statistical analysis; exceeding the observed floor is explicitly not significance |
| Independent outcome verification | Expected files are snapshotted before execution; verified scorer bytes are executed without reopening source | Protected evaluator authority (#50), protected promotion (#67), adversarial isolation proof; snapshots detect changes but do not deny writes |
| Independent task state | Fresh HOME, OMP/XDG/TMP/session roots; explicit provider environment/catalog inputs | `benchmark_isolation.py` against a freshly built OMP binary, including its positive control and cross-trial marker proof |
| All attempts and full costs | Per-attempt records and all-run totals are retained; incomplete telemetry is labeled | Descendant/proposer/evaluator accounting (#62) and multiple-budget curves |
| Official evaluation | Pinned RULER scorer and dataset/prompt provenance support | Actual selected-cohort execution and published results; no full RULER or OOLONG run is established |
| Broader research cohorts | Not implemented by this adapter patch | Versioned availability/digest/task/limit matrix for the named context, interactive, systems and persistence cohorts; unresolved released configurations remain missing |
| Held-out competitive comparisons | Not implemented by this adapter patch | Source-family splits/leakage audit, comparator manifest, matched versus best-configuration tracks, ablations, randomized paired trials, predeclared winning rules and honest losses/inconclusive results |
| Browser-reviewable acceptance | Leaf contract artifacts/summary and adapter comparison report support | Actual two-arm full-vector table, staleness assertion, A/A delta rows and raw production run artifacts; hosted parent/negative-control evidence |

Local lightweight validation on this consolidation: all 19 Bun tests passed,
zero failed, 115 assertions. The two pinned official RULER files were fetched at
`c3f5e3b4f87f97e048793bb510a3a6b19a46bf3a` and verified by the scorer tests.
These tests use local contract records and fixture executables; they are not a
standardized dataset result, a production OMP smoke run, or competitive evidence.
No heavy Rust build or real model call was run during this consolidation.
