# Completed-turn evidence gap

Issue #105 requires at least **500 completed turns** in one journal. The
`turn.receipt@1` event records one inference, not one user turn: tool
continuations and retries can produce multiple receipts, including before a
later turn failure. Receipt counts remain diagnostic information only.

The current kernel does not journal an authoritative successful turn end.
`crates/agent/src/loop.rs::finish_turn` journals failure/interruption and
publishes `KernelEvent::TurnEnded` in memory. A provider `stop` message precedes
Director review and final settlement hooks, so it cannot establish successful
completion either. The report therefore marks completed distinct turns as
**unknown / FAIL**, preserving the >=500 threshold. Exit-zero process counts
are displayed separately and do not satisfy that journal requirement.

The driver waits for the requested number of successful process exits instead
of stopping after that many attempts. This is an execution budget only; failures
and kills cannot prematurely exhaust it. The existing duration and maximum-run
limits still apply, and this does not establish completed-turn acceptance.

Required follow-up: add an authoritative durable terminal outcome at the
production lifecycle boundary, then count successful distinct turn identities
from the selected journal branch. Verify failures, cancellation, crash windows,
retries, and rewinds cannot create or duplicate successful outcomes. Update the
sampler to publish that count separately from inference receipts, and run the
full hosted soak plus its deliberate failing companion. This script repair
does not provide that production event or claim the original soak passed.

Offline script regression checks:

```sh
python3 -m unittest discover -s scripts/soak -p 'test_*.py'
```
