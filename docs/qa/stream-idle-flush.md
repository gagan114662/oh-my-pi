# Idle provider streams must flush durable prefixes

Baseline `b59b952172` failed P6 in run `34245635651`: the provider sent its
prefix, but the real CLI journal never contained it within the unchanged
three-second bound. This is a production timer defect, not evidence that the
bound is too small. `ContentStreams::append` checked the 250ms window only when
another delta arrived. With less than 4096 bytes buffered and no later event,
there was no task wake-up that could flush the prefix.

The inference loop now waits on the pending buffer's original deadline alongside
provider events, cancellation and mailbox control. Expiry flushes the existing
buffer and updates live components; it does not synthesize provider output or
completion. Cancellation retains priority, and no timer runs with an empty
buffer. Byte-budget and event-order flush behavior is unchanged.

The regression checker stalls a live kernel stream after one small delta and
waits for the actual journal prefix before allowing any additional provider
event. It exercises both release/completion and cancellation, checks provider
stream disposal, and verifies append-only preservation of the observed prefix.
Existing coalescing tests still constrain burst entry counts and large deltas.

`stream-idle-flush.yml` uses a fresh source checkout without restored build
artifacts. It records source/checker/P6/binary hashes, builds that source's
production application host, runs both unchanged P6 tests normally and under
actual bounded disk pressure through the existing `scripts/p6-latency.py`
helper, and runs complete agent/driver/app targets plus doctests. The unchanged
helper requires both runs to complete with at least 2x margin inside the original
3s journal and 30s resume bounds. Missing measurements fail the always-run report;
normal/contended JSON, disk-load counters and raw logs are retained.
Dispatch `source_ref=b59b952172` separately for a red baseline with the same
new regression checker; failing exits are never converted into passes.

The baseline's leaked-handle report is not independently explained by a stack
trace. Its panic occurs before the explicit process reap and gateway release.
The environment daemon owns a separate process group, so killing the CLI group
alone cannot prove daemon cleanup. The unchanged P6 run remains necessary to
establish successful lifecycle behavior after fixing durability; the timer fix
alone does not claim that every panic cleanup path is repaired.

Run `34258775970` at source `2359f7ac023ed163eaaa10962cd05b36ebfb4ecc`
passed both normal P6 tests and all affected package tests/doctests. Normal
journal visibility took 385.870ms (7.77x margin); frame resume took 727.660ms
(41.23x margin). Under disk pressure, replay and resumed-frame assertions also
passed, but the resumed process did not exit within the original 30 seconds
after `ctrl+c ctrl+c`. The test failed and reported leaked handles. The load
completed 18,436 syncs without a reported load error. No contended timing file
survived, so this run establishes neither contended timing margins nor clean
shutdown. The historical artifact contains no resumed process stack or terminal
tail; the cause of that exit timeout remains unknown.

P6 now publishes each timing phase before teardown, retaining `completed: false`
until the entire proof succeeds. Quit duration is a separate diagnostic field;
the existing timing checker and its acceptance limits are unchanged. After a
failed quit wait, the fixture captures the owned resumed PID's `ps` state
(without command-line arguments), a debug-frame response, the last 64KiB of PTY
output, and the synthetic journal in `normal.shutdown.json` or
`contended.shutdown.json` beside the timing file. Process inspection has a
separate two-second diagnostic timeout, and the debug socket retains its existing
I/O limits. These observations happen after failure and cannot make the failed
30-second wait pass. The original key sequence and clean-exit assertion remain.
The extra diagnostics have been statically checked; a fresh production P6 run
is still required to identify or verify a shutdown fix.
