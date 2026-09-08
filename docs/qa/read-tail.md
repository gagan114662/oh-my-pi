# Tail selector acceptance evidence (#31)

`read-tail.yml` runs the selected head on push. Dispatch it separately with
`source_ref=b59b952172` to run the same production-binary checker against the
parent of this change, retaining its red result in a separate run. Each side builds its own source,
uses its own shared HTTP mock harness, and records source/checker/binary digests.
The checker invokes real `read` and `eval` tools and inspects the tool results
sent back to the provider. Synthetic assistant responses cannot satisfy the
checks without a corresponding tool result.

The QA table covers 200,000-line local text, short/empty text, raw terminal
newlines, a directory listing, invalid zero/overflow counts, and Python artifact
reads including 200,000 lines. The artifact probe asserts exact raw strings.
Video cases generate actual MP4/MOV fixtures and cover preview grids, frame and
time parity, the exact `1h5m42s` selection, corrupt input and out-of-range seeks.
The captured provider request must contain a PNG, which is decoded into the
case evidence directory; metadata text alone cannot satisfy the video cases.
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

The leaf workflow's Linux job builds the selected source. Its separately
dispatched before/after runs build exactly the same four affected
package targets with the pinned toolchain, unmodified source Cargo settings,
and source-provided embedded Python setup. Each directly runs the tools libtest
listing with a 120-second deadline, preserving its exit/signal code, binary
hash, output, and source/toolchain/configuration provenance. A failed listing
also receives a bounded GDB run that captures all-thread backtraces, registers,
and shared libraries. Failures remain failures on both revisions. A passing
macOS result does not resolve this Linux failure; compare the actual Linux
artifacts before classifying it.

## Video QA prerequisites

The production video checker requires both `ffmpeg` and `ffprobe` on PATH, with
the seekable `fd` protocol and an encoder supporting `libx264` for generated
fixtures. Check `ffmpeg -h protocol=fd` and `ffprobe -h protocol=fd` when selecting
a distribution; unsupported utilities fail rather than reopen a pathname. On macOS use
`brew install ffmpeg`; on Ubuntu use `sudo apt-get install ffmpeg`. The read-tail
leaf installs this explicitly. Ordinary Rust selector and bounded-reader tests
do not need external media utilities; no test is newly ignored. Existing CI
runner availability is not assumed, and a missing utility makes production QA
fail with its process diagnostic. Runtime video reads also return a typed
missing-binary fault instead of claiming an extraction succeeded.

Media confinement tests disguise a local concat playlist as MP4 and an HTTP
playlist as MKV. Both must fail without an image; a live HTTP fixture records
any attempted network reference and must receive zero requests. Frame/time
results also equal an independently decoded frame-2 oracle, verified distinct
from frame 0. Normal Unix Rust tests cover a held file surviving replacement,
symlink rejection at final/ancestor components, output bounds and bounded child
reaping after deadline/output-limit failure. No new ignore annotations are used.

## First production QA failure and corrections

[Head run 34250672559](https://github.com/gagan114662/oh-my-pi/actions/runs/34250672559)
(source eca75da104) reached real tool results. `large.txt:-2` returned lines
199998–200000: tail resolution had reused absolute-range formatting, which adds
one leading context line. Tail formatting now preserves the requested count;
absolute ranges retain their established context behavior. A 200,000-line
formatter regression asserts both outputs, and the production fixture still
forbids line 199998 for `:-2`.

The artifact eval case in both that run and
[parent run 34250899450](https://github.com/gagan114662/oh-my-pi/actions/runs/34250899450)
never invoked eval: captured catalogs advertised only bash/edit/glob/grep/hub/task/read.
The fixture omitted the explicit `--py-eval` CLI opt-in. QA now requests that
capability and asserts the requested tool appears in the actual provider catalog
before asserting its output. This does not change the artifact parity assertions.
Parent additionally failed the unsupported tail reads, as expected. Both old
runs predate video support; neither supplies video execution evidence. These
corrections still require fresh production execution, not just static checks.
