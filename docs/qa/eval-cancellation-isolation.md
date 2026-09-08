# Eval cancellation-isolation fixture setup

Linux Check 34256567013 at f73dbe0def ran 1,792 affected-package tests:
1,791 passed, one failed, two skipped. The failing test was
`eval::kernel::tests::cancelling_one_worker_does_not_interrupt_another_session`:
the first completion was `Timeout`, where the unchanged assertion requires
`Cancelled`.

Source inspection shows that `open_session` returns after spawning a worker;
Python bootstrap and namespace installation precede that worker's `Started`
event. The original fixture submitted two cells with two-second execution
budgets, then waited for both workers to start before cancelling the first.
A slow second startup can therefore consume the first cell's budget before its
cancellation request. This is a source-derived hypothesis for the observed run,
not a runtime-proven explanation: the failed log contains no cancellation timing.

The fixture now completes a bounded, successful `None` cell in each session
before submitting either infinite cell. Both infinite cells retain their original
two-second budgets, the noninterference observation remains 30 milliseconds,
and both terminal assertions still require `Cancelled`. No retries or skips are
introduced. Existing helper warmups also have two-second cell budgets.

Diagnostics report elapsed time since each submission, its cancellation flag,
worker liveness, and whether that exact cancellation target is active immediately
before cancellation. Elapsed time and the flag are also printed when cancellation
returns. These timings include queue/setup time and are not represented as precise
cell activation times. The run handle does not expose the watchdog's timed-out
flag; it has not been added solely for this test. The unchanged terminal assertion
reports an observed `Timeout` if the problem persists.

Production cancellation reason precedence is unchanged. In particular, the
watchdog can set `timed_out` after a cancellation request if interruption is slow;
this patch neither claims that behavior caused the failure nor proves it correct.
Rust formatting and whitespace checks are the only local validation so far. The
original test must run with this setup, followed by complete affected tests and
doctests when build resources are available.
