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
