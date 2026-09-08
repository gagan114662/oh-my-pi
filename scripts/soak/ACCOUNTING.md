# Completed-turn evidence

Issue #105 requires at least **500 completed turns** in one journal. The
`turn.receipt@1` event records one inference, not one user turn: tool
continuations and retries can produce multiple receipts, including before a
later turn failure. Receipt counts remain diagnostic information only.

The production kernel now appends `turn.outcome@1` at its terminal boundary.
Its `by` field identifies the durable `turn.start@1`, and its typed `status` is
`completed`, `incomplete`, `failed`, `cancelled`, or `steered`. Only `completed` counts toward
500. Budget exhaustion, uncompacted overflow, and pause handoff are incomplete,
even though the legacy control stop may be `Completed`. The append follows Director decisions, reply settlement, final session
state flush, and the `agent_end` notification enqueue. A provider stop or a
process exit is not this boundary. Detached notification execution is not a
new awaited turn obligation.

The outcome is synced using the journal's ordinary durable append. It does
not invoke Component reducers after commitment. A crash before a complete
outcome frame leaves no success; recovery ignores a torn tail. A crash after
the outcome may precede caller acknowledgement, but the durable turn is still
complete. Repeating an identical outcome is idempotent; a conflicting outcome
requires rewinding before the old outcome. Old journals acquire no synthetic
successful outcomes on open.

`journal_accounting.py` reads complete OMS frames and follows `prior` ancestry
from the selected durable head (the final complete entry by default). It
counts distinct completed turn identities, excluding abandoned branches and
non-success outcomes. Report `--head` selects an explicit historical head.
An in-memory rewind without a subsequent durable branch append is not a
persisted selection and cannot change an offline report. Invalid ancestry,
conflicting outcomes, or ambiguous journal selection produce unknown / FAIL.
The report uses the driver's `session.txt`, falling back only to a sole
journal, and publishes selected head, successful identities, and journal hash
in `turn-accounting.json`. The sampler publishes completed turns separately
from receipt counts.

The driver waits for the requested number of successful process exits instead
of stopping after that many attempts. This remains an execution budget only;
process exits cannot satisfy the journal criterion. The duration and maximum
run limits and every acceptance threshold remain unchanged.

Validation includes production kernel success, cancellation, failure, and
multi-inference assertions; session reopen, duplicate, rewind, stale-turn and
torn-tail tests; and offline branch-parser regressions. Passing these checks
is not evidence that the full hosted soak or its deliberate failing companion
has run. Those executions remain required.

Offline script regression checks:

```sh
python3 -m unittest discover -s scripts/soak -p 'test_*.py'
```

The separate `turn-accounting.yml` leaf builds the selected real app and runs
two print/resume turns with tool continuations, then runs every affected
journal/session/agent/driver/app target and its doctests. An independent job
builds the frozen original parent with the identical Python fixture; the
expected negative requires actual successful process execution and inference
receipts but zero terminal markers. Build or transport errors do not qualify.
The raw negative checker exit and its expected-control result are separate.
This two-turn smoke proof does not replace the >=500-turn, >=60-minute soak.
The existing lifecycle hook field `summary.committed_turns` retains its prior
inference-request meaning; the new accounting does not interpret that field.

The original parent job in run `34257276585` built source
`29fe965b37ba9a0a5ced3ddf7a73bb751db0ab7d`, but its provider did not publish
readiness within the unchanged 10-second limit. Its observation contained no
app processes or journals, so that run is a fixture startup failure, not a
semantic negative for completed-turn accounting. The empty provider log does
not establish which startup operation stalled.

The provider now builds responses without constructing a throwaway mock HTTP
listener. Both it and the shared QA mock bind numeric loopback without the
standard library HTTP server's reverse-DNS lookup. An offline subprocess
regression makes reverse DNS unavailable, starts the actual provider within
the same 10-second limit, and checks JSON and streaming responses. This
reproduces and removes an unnecessary DNS dependency; it does not prove DNS
caused the historical hosted failure. Startup logs identify imports and bind
phases, and a stack dump after nine seconds diagnoses stalls before the
existing readiness gate. A fresh hosted parent execution must still run the
real turns before it can qualify as the semantic negative.

Run `34263807696` exercised the same fixture on head
`05a13d28e52bf4782bfbc11f3cca4d115406da1b` and the frozen parent above.
Both executed two successful app processes, four provider requests, four
receipts, and two tool results. The head recorded two distinct completed-turn
markers; the parent recorded zero and the fixture exited 1 with
`failure_kind=completed_turn_count`. Thus this run supplies a semantic negative,
unlike the earlier startup failure. The retained journals were independently
recounted after download, with SHA-256 matching each report.

The head's overall job still failed: its app target suite had 314 passes and
one failure in
`envd_contract::worker_cancel_forwards_effects_unknown_once_and_respawn_serves_next_request`
after replacement-host startup hit its event deadline. The journal, session,
agent, and driver target suites and all five paired doctest commands exited 0.
This is evidence for the two-turn accounting behavior, not a passing combined
acceptance gate or the required long soak.
