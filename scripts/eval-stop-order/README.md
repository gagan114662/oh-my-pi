# First stop reason: fixed parent/head proof

This leaf compares production `6aca9cbb8ead79a0ed04b6fdc0f6b26d2f147d9d` with
actual `fc17d7cd4282a043fcbe733aeac68b98b705ce95`. Head includes the preceding
`2ce6f17e8c` Python-attachment lock correction and dependency feature change.
The checkout running the checker is separate and has its own recorded SHA.
No API, account, or model request is involved.

`fixture.rs` is the exact new `DeadlineHold`, `DeadlineInstaller`, and
`first_stop_reason_survives_later_watchdog_or_cancel_during_native_wait` block
from head, including its original assertions and two-second real timeout.
Its SHA-256 is frozen in `fixture.json`. Head already contains those bytes;
parent receives only that insertion into its existing tests module. The
provenance and source.diff artifacts identify both the original and tested
source bytes. No production code, manifest, older assertion, timeout, or
fixture helper is rewritten on parent.

The fixture uses interfaces already present at parent, so no compatibility
adjustment is currently needed. It pauses initialization out of the timeout
window, holds a real embedded-Python native call, and waits for the watchdog's
host signal after the unchanged two-second interval. Head must pass both
orderings; parent must execute and fail specifically with actual `Timeout`
versus expected `Cancelled`. A compile error, deadlock, generic test failure,
missing result, or killed test is not an accepted negative result.

Fresh nextest JUnit identifies the exact test and its elapsed duration excluding
compilation. The head test must take at least four seconds (two orderings), and
the parent at least two (the first ordering). Both logs and XML are retained.
Complete `omp-tools` and `omp-con` targets and doctests run on head even after
a proof failure, with every command/log status retained. The browser summary
shows each required result; this is not a claim that any run has passed yet.
