# Executable elastic-slot model checks

`just tlc` runs pinned TLC 1.8.0 over the complete tracked model/configuration
inventory. It requires Python 3 and Java 17+. The first run downloads the
4.5 MB official `tla2tools.jar`; every run verifies SHA-256 before executing it:

```
b658b4e504fdf0b721caf7066320f6b6fe5805f4dd2f717d0e47baba4097205e
```

Provenance: [TLA+ v1.8.0 release](https://github.com/tlaplus/tlaplus/releases/tag/v1.8.0),
asset ID `544648411`. The pinned hash matches the release asset's API digest.
Use `--jar /path/to/tla2tools.jar` for an already downloaded copy. A checksum
mismatch fails; the runner never silently uses a different installed TLC.

The runner covers these variants without dropping assertions:

| Variant | Module | Configuration | Invariants / properties |
|---|---|---|---|
| elastic | elastic/proof/ElasticSlots.tla | ElasticSlots.cfg | 10 / 10 |
| adr | docs/adr/0034/ElasticSlots.tla | ElasticSlots.cfg | 10 / 10 |
| bridges | docs/adr/0034/ElasticSlots.tla | ElasticSlotsBridges.cfg | 7 / 6 |
| pluscal | docs/adr/0034/ElasticSlotsPlusCal.tla | ElasticSlotsPlusCal.cfg | 10 / 10 |
| pluscal-bridges | docs/adr/0034/ElasticSlotsPlusCal.tla | ElasticSlotsBridges.cfg | 7 / 6 |

The two original model/config copies must stay byte-identical. An added or
removed model or neighboring configuration fails inventory validation until
explicitly covered. The new PlusCal configuration preserves the main model's
constants, ten invariants and ten temporal properties. Before checking PlusCal,
the runner regenerates its translation in scratch and compares the generated
TLA+ body with the tracked translation. It does not edit the checkout.

Each TLC invocation uses one worker, a 1 GiB heap, a 512 MiB scratch-file limit,
and a ten-minute deadline by default. `--timeout`, `--heap-mb` and `--disk-mb`
can increase those budgets on a provisioned host. A timeout or resource limit
is **incomplete**, never a passing model check. Temporary state stores are
removed after each invocation. Logs, exact executed models/configurations,
source hashes, tool identity and results remain under `target/tlc`. A selected
`--variant` is explicitly reported as partial coverage.

## Deliberate counterexample

Run:

```sh
python3 scripts/check-tla.py --download --output target/tlc-mutation \
  --variant bridges --mutate-ech
```

This command must exit nonzero. In a scratch model it drops the semantic rows
from a successful retirement while retaining the frontier commitment. The
unchanged `ExactCommittedHistory` invariant must fail. TLC's actual trace,
including the initial state and subsequent transitions, is included in the
summary and retained log. This is not an always-false assertion or invented
trace. If the transition changes, the mutation selector fails loudly and must
be reviewed. Do not use this mode to claim normal model-check success.

`just tlc-test` covers jar tampering, duplicate drift, undiscovered variants,
timeout cleanup and mutation targeting. It does not replace actual TLC runs.

## CI integration and remaining acceptance

The checked-in `tlc` job uses Ubuntu 24.04 and `actions/setup-java` v6.0.0
pinned to commit `dd06d9cba3e5552c54d9f8ea23572deb30010f7c`, with Temurin
Java 17. It runs all five variants with a 600-second per-invocation deadline,
4 GiB heap and 4 GiB scratch limit. The job budget is 35 minutes and the
model-check step budget is 32 minutes, leaving time for evidence upload.
Interrupted, resource-limited and incomplete runs fail; budgets never imply
success. Main-branch changes under `elastic/**` also trigger the workflow.

The job runs:

```sh
python3 scripts/check-tla.py --download --output target/tlc \
  --timeout 600 --heap-mb 4096 --disk-mb 4096
```

The workflow always uploads `target/tlc/` as artifact `tlc-output`, including
on failure. The runner writes its readable summary to `$GITHUB_STEP_SUMMARY`;
`results.json` must say `passed` with `coverage: all` before accepting the
full gate. The workflow writes an explicit incomplete notice if interruption
prevents the final summary. All five logs, configuration text, executed
sources and hashes are retained. Give the job enough resources for the
unchanged larger configurations; do not reduce their constants or temporal
obligations merely to get green. Hosted execution is still required.

A companion deliberate-mutation workflow run must be **red**, retain the ECH
counterexample, and be linked beside the unmutated green run. The local
counterexample confirms the runner has teeth; it does not replace that hosted
demonstration.

TLC proves properties of these finite models. It does not execute Rust or
establish refinement of `crates/tui/src/slots.rs`. Issue #14's separate
spec/implementation-divergence requirement still needs a production Rust
behavior test or a trace/refinement bridge and its deliberate failure proof.
A hash of Rust source or a parsed cfg would not establish that correspondence.

The first real TLC run exposed undeclared `ℕ` in both original source copies;
replacing those four identifiers with `Naturals`' exported `Nat` fixes semantic
analysis without changing any transition, invariant or property. The first
small Bridges runs then completed; full larger-model and hosted acceptance
must be recorded independently.
