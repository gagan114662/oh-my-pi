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
production application host, runs both unchanged P6 tests, and runs complete
agent/driver/app targets plus doctests. P6 timing JSON and raw logs are retained.
Dispatch `source_ref=b59b952172` separately for a red baseline with the same
new regression checker; failing exits are never converted into passes.

The baseline's leaked-handle report is not independently explained by a stack
trace. Its panic occurs before the explicit process reap and gateway release.
The environment daemon owns a separate process group, so killing the CLI group
alone cannot prove daemon cleanup. The unchanged P6 run remains necessary to
establish successful lifecycle behavior after fixing durability; the timer fix
alone does not claim that every panic cleanup path is repaired.
