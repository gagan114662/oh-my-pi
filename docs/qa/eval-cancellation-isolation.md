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

The initial warmup change left cancellation precedence unchanged. Subsequent
run `34259539503` still observed `Timeout` after an earlier explicit cancellation.
The production fix in `6aca9cbb8ead79a0ed04b6fdc0f6b26d2f147d9d` records the
first stop reason atomically: later deadline escalation cannot rename an earlier
cancellation, and a later cancellation cannot rename an earlier timeout.

Run `34263239637` used the same frozen fixture on that fix and parent
`fc17d7cd4282a043fcbe733aeac68b98b705ce95`. The parent source differed only by
insertion of the fixture. It failed with exactly `left: Timeout, right: Cancelled`
after 2.413 seconds. The head passed both real two-second deadline orderings in
4.531 seconds. Durations come from the individual JUnit cases, excluding builds;
neither result was a build failure or external timeout. The frozen fixture hash
was `0a359e185a7d14f13ba584318dce497457e257f819b7580f1c481f3665ba5790`.

The head job nevertheless failed its full tools suite: 737 tests passed and the
new local-tail test failed on an outdated expected line count. The con suite
passed 46 tests, and both doctest commands passed. The tail expectation is fixed
separately in `0d1ecce1c78f485cb24001e0c4d12d113a6f8753`; that later change was
not part of the frozen head tested here. This proves the focused stop-order
behavior, not a green combined gate or every cancellation lifecycle path.
