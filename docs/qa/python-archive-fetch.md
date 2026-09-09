# Python archive fetch stage evidence (#39)

Run 34251479511 failed `just setup-python` with exit 141 immediately after the
20260807 aarch64 debug archive fetch began. Its log contains no individual stage
statuses. The old `curl | zstd -d | tar -x` pipeline with `pipefail` therefore does
not establish which process failed, nor why. SIGPIPE from an early downstream
close is a possible mechanism, not a demonstrated cause of this hosted failure.

Local investigation on 2026-09-08 used the exact pinned archive:
`cpython-3.14.7+20260807-aarch64-apple-darwin-freethreaded+debug-full.tar.zst`.
SHA-256: `b13c271a52016e58640313ef9a6c544cc2eb8f502be96e796cdc272079ed6f2e`.
The downloaded archive expands to a 235704320-byte tar. This digest records the
observed fixture; it is not an independently authenticated integrity guarantee.
Local tools: zstd 1.5.7 and bsdtar 3.5.3 / libarchive 3.7.4.

| Execution | Observed result |
| --- | --- |
| Original pipeline, actual GitHub URL | curl 0, zstd 0, tar 0 |
| Downloaded pinned archive through zstd / tar pipe | zstd 0, tar 0 |
| New staged helper, downloaded pinned archive | download 0, decompress 0, extract 0 |
| New staged helper, actual GitHub URL | download 0, decompress 0, extract 0 |
| Bounded regression suite | 7 tests passed |

The new helper separates download, full decompression, and extraction with files.
This eliminates upstream broken-pipe dependence between those stages and reports
the exact exit status of any failing stage. It does not reinterpret 141 as success
or retry failed operations. Temporary staging is cleaned on normal exit/failure
and handled INT/TERM; SIGKILL cannot run shell cleanup. Installation starts only
after extraction succeeds and a regular `python` directory exists. The old tree
is retained when any fetch stage fails. This is not a concurrent installer or a
transaction covering interruption during final tree replacement.

Regression tests use real curl against an isolated loopback HTTP server, real
zstd, and real tar. Cases cover successful replacement, truncated/corrupt zstd,
invalid/truncated tar, HTTP 404, and injected exit 141 at each stage. Failures must
retain the prior tree, omit a new stamp, report the failing status, and leave no
owned scratch directories. Missing tools fail tests instead of being skipped.

Run from the checkout:

```sh
python3 crates/py/scripts/test-fetch-python-archive.py
bash -n crates/py/scripts/fetch-python.sh crates/py/scripts/fetch-python-archive.sh
just setup-python
```

The local original pipeline did not reproduce the hosted failure. A fresh hosted
setup run is still required; this evidence does not claim that the historical
141 cause is known, or that full Python artifact generation/Rust linking passed.
