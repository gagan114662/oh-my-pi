# Real-duration watchdog proof (#124)

This leaf prepares the external correctness phases required by issue #124.
It does not claim either phase has run, or replace the full #105 soak. The
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
  at least 100 actual tool results, the 101 st final-answer request, a natural
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
python 3 scripts/soak/watchdog_fixture.py --binary target/debug/omp --out target/watchdog-manual
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
