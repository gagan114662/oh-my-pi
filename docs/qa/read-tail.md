# Tail selector acceptance evidence (#31)

`read-tail.yml` runs the same production-binary checker against the parent of
this change (`b59b952172`) and the selected head. Each side builds its own source,
uses its own shared HTTP mock harness, and records source/checker/binary digests.
The checker invokes real `read` and `eval` tools and inspects the tool results
sent back to the provider. Synthetic assistant responses cannot satisfy the
checks without a corresponding tool result.

The QA table covers 200,000-line local text, short/empty text, raw terminal
newlines, a directory listing, invalid zero/overflow counts, and Python artifact
reads including 200,000 lines. The artifact probe asserts exact raw strings.
Captured provider messages, process output, failures and expected/actual tables
are retained per case. The before build must expose an observed semantic
mismatch and retain a failing QA exit; missing execution is not a baseline reproduction. The after build
must pass every case. Complete tools/envd/driver/Python package tests and
doctests run independently of the QA outcome.

Run an already built binary with:

```sh
OMP_BINARY=/absolute/path/to/omp \
OMP_READ_TAIL_EVIDENCE_DIR=target/read-tail/qa \
python3 scripts/qa/cases/test_read_tail.py -v
```

## Linux test-discovery failure

Check run `34247669855` compiled the affected targets, then failed before test
execution: the `omp-tools` libtest binary received SIGSEGV while executing
`--list --format terse`. Nextest returned 104, with empty binary stdout/stderr.
This is failed execution evidence, not a passing test result. No root cause or
baseline classification has been established from that log.

The leaf workflow's Linux before/after jobs build exactly the same four affected
package targets with the pinned toolchain, unmodified source Cargo settings,
and source-provided embedded Python setup. Each directly runs the tools libtest
listing with a 120-second deadline, preserving its exit/signal code, binary
hash, output, and source/toolchain/configuration provenance. A failed listing
also receives a bounded GDB run that captures all-thread backtraces, registers,
and shared libraries. Failures remain failures on both revisions. A passing
macOS result does not resolve this Linux failure; compare the actual Linux
artifacts before classifying it.
