# Agent loop

The production turn owner is `Kernel<C>` in `crates/agent/src/loop.rs`.
`crates/driver/src/headless/kernel.rs::compose_kernel` assembles inference,
environment access, tools, approvals, and session state. App code adapts that
composition to CLI, terminal, native, and remote presentation. Process placement
is described separately in [processes.md](processes.md).

## Composition and durable state

The kernel owns turn execution, its dispatcher, cancellation tree, Director
registry, lifecycle hooks, and live-component bridges. It receives a mutable
`omp_session::Session` when executing a turn. Session state does not live in a
second kernel-owned transcript writer: `crates/session/src/session.rs` and
`crates/session/src/fold.rs` append to `omp-journal` and fold the same entries into
`omp-dom`. Reopening a session replays durable entries through the fold.

```text
app presentation adapter
  → driver compose_kernel
  → agent Kernel + Dispatcher
  → session Session
      → journal append
      → DOM fold
      → Session::subscribe() consumers, including chat
```

The old `AgentState`, `Journal`, `ControlMailbox`, and `Arbiter` descriptions
referred to a previous implementation. There are no current `agent/src/state.rs`,
`journal.rs`, `mailbox.rs`, or `regime.rs` modules. The durable session and the
Director interfaces are the current ownership boundaries.

## Inference and tools

`crates/ai/src/call.rs` defines typed inference requests;
`crates/ai/src/event.rs` defines canonical `ChatEvent` stream values. Provider
codecs and Tower layers in `omp-ai` normalize provider behavior before the kernel
consumes it. Catalog compatibility facts belong to `omp-catalog`.

The kernel projects session history, prepares the request through Directors and
hooks, and consumes the inference stream. `crates/agent/src/dispatch.rs` owns the
tool dispatch policy and settled results. Versioned tool identities, argument
feeds, typed outcomes, diagnostics, and visibility receipts belong to `omp-tool`.
`crates/tool/src/incoming.rs` defines `IncomingParams`; "ArgFeed" in design prose
is shorthand rather than the name of a Rust type.

Resource effects cross `omp-env` to `omp-envd`. The environment host owns
filesystem, process, document, and worker resources, so app presentation code
must not bypass its authorities. Durable tool results and inference receipts
are recorded through the session; model and presentation projections derive
from that state. The central result-bound policy and its remaining notice
projection gap are recorded in [ADR 0009](../adr/0009-bound-output-once.md).

## Directors, cancellation, and jobs

`crates/agent/src/director.rs` defines the Director interface and stack;
`crates/agent/src/directors/` contains concrete policies. Python Directors are
registered by `omp.extensions.director` in
`crates/py/python/omp/extensions.py` and bridged by `PyDirector` in
`crates/envd/src/exthost/extensions.rs`. The proposed `omp.regimes` API is not
exported by the frozen Python package; it must not be inferred from the presence
of Director support.

`crates/agent/src/cancel.rs` and `crates/agent/src/dispatch.rs` own scoped
cancellation and dispatch settlement. `crates/agent/src/jobs.rs` owns detached-job
coordination. Driver subagent composition lives in `crates/driver/src/subagent/`.
The environment host supervises the resources that must actually stop; a
cancelled UI future alone is not a process kill boundary.

## Lifecycle hooks and the gate

`HookGate` in `crates/agent/src/hooks.rs` owns the subscription bitmap, the
dispatch queue, and the pending-reply table; `Kernel::with_hook_gate` installs
it (`crates/agent/src/loop.rs`). An unsubscribed event costs one relaxed atomic
load, a bit test, and a branch: no payload is built. Subscribed decisions run the
`HookPhase` order PRECHECK, TRANSFORM, REVIEW, APPROVAL, then OBSERVE; a deny
short-circuits the later phases. Subscriptions marked fail-closed synthesize a
deny when the extension host is gone instead of letting the effect through.
`HookGate::delegated_channel` hands one complete dispatch to envd's
`HookControlFactory` (`crates/envd/src/tools.rs`), which selects sealed
subscriptions, orders them, calls the exact extension generation, and answers
with one final decision. `LifecycleHooks` carries the non-gating seams.

## Presentation and verification

`omp-chat` is an actor over `Session::subscribe()`. It consumes snapshots and
ordered updates and sends user actions to the host. It never owns a parallel
canonical session or provider policy. Terminal and native renderers consume the
presentation state described in [crate architecture](crates.md).

Tests should target the owner of the changed behavior: agent dispatch/Directors,
session replay, environment effects, or driver composition. Joined-system tests
in `crates/e2e/tests/` include P1–P10 and tool-source coverage. Existence of a test
is not a passing result; use the matching `just` recipe to execute it.
