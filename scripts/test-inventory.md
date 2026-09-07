# Executed test inventory

The inventory joins Cargo workspace package IDs, nextest binary IDs and actual
JUnit testcase records. It never counts `#[test]` text or assumes there are 46
crates. Tests with the same name in different binaries/packages remain distinct.
Repeated outcomes are deduplicated by binary ID/test name; a later success does
not erase an earlier failure. No minimum baseline count is invented.

## CI wiring

The nextest CI profile emits JUnit without changing its test policy:

```toml
[profile.ci.junit]
path = "junit.xml"
```

The macOS workspace job initializes before clippy so blocked test stages still
have a workspace inventory. Its output directory is unique to the run and attempt,
so restoring the Cargo target cache cannot reuse an old manifest:

```sh
export TEST_INVENTORY_DIR="target/test-inventory/${GITHUB_RUN_ID}-${GITHUB_RUN_ATTEMPT}"
python3 scripts/test-inventory.py --output "$TEST_INVENTORY_DIR" init
```

Each existing macOS `cargo nextest run --profile ci` invocation uses a capture
call with the same package/target/filter/build selectors. Example:

```sh
python3 scripts/test-inventory.py --output "$TEST_INVENTORY_DIR" capture \
  --phase workspace -- --workspace --exclude omp-e2e --locked
python3 scripts/test-inventory.py --output "$TEST_INVENTORY_DIR" capture \
  --phase p7 -- -p omp-e2e --test p7_tui --locked
```

Use unique phase names for the existing P1–P6, P9–P10/tool-sources and P8 invocations.
The wrapper first lists those exact selected targets, then runs the original
nextest command. Listing may build tests but does not repeat their test bodies;
the run reuses Cargo outputs. List/run raw exit codes and commands are retained;
no-tests remains a failure. Run-only flags, custom profiles and altered no-test
policies are rejected rather than silently reinterpreted. The configured JUnit
file is removed before each run to reject stale results and copied into the
phase directory afterward. `--junit PATH` supports a different target directory.

Keep every existing `cargo test --doc` invocation. Doctests are not nextest
cases and are deliberately not included in this table; their independent gate
must remain visible. Do not remove source tests, assertions or ignored-state
policy to satisfy the inventory.

In an **always** step after the existing test stages:

```sh
python3 scripts/test-inventory.py --output "$TEST_INVENTORY_DIR" report
```

The always-run artifact step uploads exactly `$TEST_INVENTORY_DIR` using the existing pinned artifact
action with `if-no-files-found: error`. The report appends its per-crate table
and the recorded nextest summary lines to `$GITHUB_STEP_SUMMARY`.
A report failure must remain a failing CI step. It catches unrepresented newly
added crates automatically; there is no zero-test exception list.

The repository currently separates e2e target selections. The final aggregate
therefore needs **all existing e2e capture phases**, not just the workspace
phase that excludes `omp-e2e`. This inventory proves the registered/executed
selection and per-crate nonzero coverage, not that every unselected test in the
repository ran. Expanding the selection is a separate reviewable change.

## Status semantics

- **Zero registered tests:** successful nextest discovery produced no cases for
  a selected crate. A manifest with a library target alone does not prove tests.
- **Zero executed tests:** discovery listed cases, and a valid execution report
  exists, but none executed. Filter/skip-only coverage does not pass the gate.
- **Unknown discovery/build:** the phase never completed listing, or no phase
  selected the crate. Counts are `null`; the shared build failure does not prove
  that each individual crate failed to compile.
- **Unknown execution:** list exists but JUnit is absent. Never read an absent
  report as zero failures.
- **Failed/incomplete:** failed/error/flaky cases, missing selected outcomes,
  skipped selected tests, unsuccessful phase exit, malformed or mismatched
  evidence. A later repeated success cannot hide the prior failed outcome.
- **Passed:** nonzero executed cases, complete selected results, valid identities,
  and successful contributing phases. This is test-execution evidence, not proof
  that the tests adequately exercise each crate's behavioral contract.

`python3 scripts/test-inventory-test.py` runs small negative parser/capture
fixtures without compiling Rust. Actual complete workspace execution and hosted
red-before/green-after proof remain necessary for #45; the HTTP behavior tests
are owned separately.

Format references: [nextest list](https://nexte.st/docs/machine-readable/list/),
[nextest JUnit](https://nexte.st/docs/machine-readable/junit/).
