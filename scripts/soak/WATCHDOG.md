# Real-duration watchdog proof (#124)

This leaf prepares the external correctness phases required by issue #124.
The hosted evidence below records which phases ran; this does not replace the full #105 soak. The
scripted OpenAI wire transport makes no Anthropic quality, cost, or performance
claim (#41 A7). Production app, driver, kernel, shell host, and journal run from
the selected source without fixture-specific production changes.

`watchdog-proof.yml` builds two fresh source checkouts with the same separately
checked-out, hashed checker. The head runs two phases serially, resuming one
journal. The original parent `b59b952172f1f331c2a79a6bc22f6ea25a3e2135` runs only
the identical-call phase. Each builds its own actual app; no production source
is transplanted, no cached app binary is used, and the source/checker/lockfile/
Cargo-config/toolchain/binary hashes and raw exits are retained.

## Frozen measurements

- **Identical tool calls before stop:** script exactly 100 responses with the
  same `bash` arguments (`printf 'WATCHDOG-REPEAT\n'; exit 1`), then a final
  answer. Count matching `tool.call@1` identities in the actual journal,
  reconstructing streamed arguments by `sid`. Require 1–16 calls, no more than
  16 provider requests, a kernel `loop-guard` notice, and an incomplete or
  cancelled terminal outcome. Merely mentioning a notice in tool/model text
  does not count. The original parent control requires all 100 matching calls,
  at least 100 actual tool results, the 101st final-answer request, a natural
  exit 0, no loop-guard notice, and the same checker failing its acceptance row.
  Build/import/transport/timeout/cleanup failure never qualifies as semantic red.
- **Minutes to idle notice:** invoke a shell script that records its PID and
  `exec`s GNU `sleep infinity`, with tool `timeout: 0`. Owner configuration
  disables `sv_shell_auto_background_enabled` so the default 60-second background
  transition cannot manufacture progress or finish the foreground tool. The
  convars remain at frozen defaults 30 minutes,500 requests,6 hours, and 8 loop
  exchanges; actual `omp config get` output and full config dump are saved.
  The CLI also gets `--max-time 6h`, which cannot win the 31-minute observation.
- Require a real non-stream journal gap of at least 1800 seconds ending no later
  than the idle notice, that notice within 31 minutes from gap start, a durable
  cancelled outcome, natural print exit 1 (the production cancellation contract),
  and observation lasting at least 1860 seconds. The runner continues observing
  after cancellation until 31 minutes; it does **not** require a working watchdog
  to leave a tool hanging for 31 minutes. No injectable clock or shorter limit is
  available in this fixture. The outer 1890-second kill is always a failed proof.
- The held external sleep PID must remain observable through the deadline
  sampling window and already be gone at natural CLI exit, before harness
  cleanup. Journal ULID timestamps enforce the full 1800 seconds; independent
  monotonic observations poll every 0.5 seconds, with at most 1-second uncertainty
  on a difference of two observations. The 1-second observation tolerance does
  not lower the durable timestamp or total 1860-second thresholds. A wall versus
  monotonic clock change of 2 seconds fails execution.

All observations, the complete raw journal, selected head/turn identities,
provider request bodies, configured script, process stdout/stderr, and process
cleanup results are uploaded. HOME, all XDG roots, and OMP config/data/cache/state
roots are private. Profile/config/session overrides and shell startup injection
variables are cleared. Provider and app receive the same isolated environment.
Daemon cleanup revalidates identity before bounded TERM then KILL; detached
process disappearance is reported without inventing an unavailable exit code.
The fixture separately detects a leaked held sleep and checks remaining local
Unix sockets cannot accept connections. Cleanup cannot turn a failed run green.

## Running and remaining evidence

With a freshly built app and GNU `gsleep` on macOS (or GNU `sleep` on Linux):

```sh
python3 scripts/soak/watchdog_fixture.py --binary target/debug/omp --out target/watchdog-manual
```

This takes at least 31 real minutes. It is not a unit-test shortcut. The leaf
runs on native macOS15 with supported prerequisites and GNU coreutils. Its
parent expected-control status is separate from the raw failing fixture exit.
The existing full agent/session/driver/app suites and doctests remain required;
this leaf does not narrow or replace those checks.

Six offline gate/parser tests reject 17 calls, a missing or spoofed notice,
completed-as-cancelled accounting, shortened elapsed time, late notice, leaked
sleep, malformed arguments, and a fake parent red. They prove checker mechanics,
not the runtime watchdog. Hosted head and parent artifacts remain pending.
A separate deliberate implementation-disabled idle control is also still pending;
no production bypass switch has been added to manufacture that control.

## Integrating the phases into the full soak

The current generic soak driver has a 720-second outer turn timeout, and it is
not suitable for the 30-minute held-tool phase. Do not lower the watchdog or
reinterpret the driver's timeout as enforcement evidence.

The next integration run must first finish the genuine >=60-minute, >=500
completed-distinct-turn soak and stop its driver. Then, with no concurrent
journal writer, reuse that exact project, config/data roots, session directory,
and driver-selected session identity for these two phases in order. Carry the
provider URL update through the same isolated owner configuration, retain the
initial and final heads, and append the two measurement rows to the soak report.
At least one additional 31-minute observation follows the baseline hour, plus
build/setup and the short identical-call phase. The cancelled/incomplete phase
outcomes add **zero** to the 500-success count. An outer workflow budget must
allow this real duration; the existing 240-minute soak job cap is a budget to
measure against, not permission to compress either threshold.

The standalone fixture currently creates a fresh private session. Wiring these
phases into the existing successful 500-turn journal and actually running that
combined soak is intentionally still pending, as are all other #105 fault rows.
Neither the two-phase leaf nor a static review satisfies that acceptance item.

## Provider startup evidence

Run `34259443561` at head `81abe06ff6f94e6b393604051724fa689fbb4eb3`
failed both phases before the provider published a port. Each phase wrote its
script descriptor, but its provider log was empty, `initial_head` was null,
and no watchdog turn ran. Those failures are not behavioral evidence for the
loop guard or the real-duration idle cutoff.

The watchdog provider uses one actual `MockModel` listener; it does not create
the soak provider's former throwaway listener. It did share the standard HTTP
listener's unnecessary reverse-DNS lookup. This branch applies the numeric
loopback binding correction from `05a13d28e5` to the shared harness, without
changing its response builders. A subprocess regression with reverse DNS
unavailable failed for both scripts before the correction and passed after it:
each real provider published a port within the original ten-second limit,
served JSON and streaming HTTP responses, and retained both request captures.
It also checks the unchanged 100-call repeat script and idle shell arguments.
This demonstrates the DNS dependency, not the historical hosted stall's exact
cause. Import/bind phase logs and a nine-second startup stack dump now preserve
that distinction on future failures.

The production fixture, its ten-second readiness deadline, watchdog assertions,
and `31 * 60` real observation duration are unchanged. The 25 offline soak tests
passed locally; no new production watchdog phase has run. The existing leaf's
`test_watchdog*.py` discovery includes the new startup regression automatically.

## Hosted source 4379a98a: observed failures and follow-up

Run [34275220932](https://github.com/gagan114662/oh-my-pi/actions/runs/34275220932)
used head `4379a98a2782bc75bcd57b0796ac6e31cafdd846` and original parent
`b59b952172f1f331c2a79a6bc22f6ea25a3e2135`. Both providers reached LISTENING.
The parent naturally executed 100 matching calls / 100 results / 101 requests,
exited0, and failed semantic acceptance as required. Head repeat naturally
exited0 with an incomplete outcome,16 calls/results/requests, and **eight**
actual kernel loop-guard patches. The checker incorrectly required a serialized
Txn.label and `name` property; canonical patch@1 has no label and uses
`custom:name`. The corrected parser requires an inserted notice with warn/error
kind and the exact producer name, rejects authored text/hook kinds, and finds
all eight notices in this retained journal. This retrospective parsing is not
a new production run.

Head idle observed1860.136s, but its external sleep survived only about29.255s.
At30s the generic dispatcher detached the call and its foreground budget, also
sent as the environment execution deadline, interrupted native bash. The CLI
then exhausted the one-response provider script and exited1 at326.414s. No idle
notice or1800s held execution was proved. The CLI log at20:44:59.757904Z records
`environment verdict omitted output projection facts`; the resulting
`effects_unknown` is **not evidence of a malformed detached JSON outcome**.
The native timeout/fallback abort publisher omitted both retained outcome and
projection metadata required by the driver.

The follow-up separates these contracts:

- Driver opts only the exact registered native core bash identity into
  executor-owned foreground policy; replacement/worker tools retain generic
  limits. Bash's configured auto-background switch/threshold and explicit
  shell timeout govern its execution. Turn idle/wall/request bounds and
  cancellation remain active.
- Only an omitted (`deadline_ms=0`) execution deadline for that registered
  native shell defers to shell policy. Any nonzero deadline wins; other tools
  retain the default environment deadline. Admission remains bounded even
  when the shell's execution deadline is omitted.
- Native timeout/cancellation fallback aborts now retain canonical bytes and
  publish matching projection facts. Worker/pre-admission abort publishing is
  a separate existing path and is not claimed fixed by this change.
- A separate source-level defect is corrected: native auto-background emits
  `ToolTerminal::Detached`, which the old driver rejected as a non-CallOutcome.
  The driver decodes that exact typed variant after artifact verification and
  passes its real job reference through the existing durable detached lowering.
  It does not present detached work as completed success.

All25 offline soak tests pass, including canonical notice negatives. Rust
regressions cover exact core identity, explicit/default execution deadlines,
retained abort bytes/projection, detached decoding, and a paused-clock dispatcher
call surviving its generic budget and still cancelling. Existing shell tests
preserve timeout0 and positive timeout behavior; generic detachment tests remain.
These Rust tests are **uncompiled/unrun locally**. The paused-clock test is a
scheduler regression, never the31-minute acceptance proof. Fresh affected
agent/tools/envd/driver suites and doctests, full CI, and a fresh actual
>=1800s held-tool / >=1860s observation with verified cleanup remain required.
