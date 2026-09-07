# Runtime spec remaining drift

The runtime-spec checker compiled and ran on Ubuntu in [workflow run 34154484336, job 101843358051](https://github.com/gagan114662/oh-my-pi/actions/runs/34154484336/job/101843358051), against commit `d8807995e126e5f4594ddeb69e97559d0c2f2561`. It exited 1 at 2026-09-07 19:11:39 UTC. This replaces the earlier static estimate with the actual job output.

The job emitted **56 diagnostic lines**: 2 duplicate-symbol diagnostics, 5 callback-ABI diagnostics, 12 DATA-operation diagnostics, and 37 Python CONTROL diagnostics covering 36 distinct operations (`omp.provider.models` occurs twice).

## Mechanical corrections prepared after this run

- Removed one identical duplicate `omp.env.workspace.snapshot` metadata row. The original row, signature, authority, and durability remain.
- Corrected callback checking for the five decorator factories below. The previous check compared their registration arguments against the callback ABI; it now checks their callback examples for exactly two arguments, with `ctx` second. Direct callback signatures still require `(payload, ctx)`. Added regression tests for valid factories and reversed/missing/extra callback arguments.

These changes have not yet been rerun in CI. They address seven observed diagnostic lines; the DATA and Python CONTROL failures below remain unresolved. No phase, authority, durability, or operation rows were invented to make the gate pass.

Observed mechanical diagnostics:

```text
duplicate public symbol omp.env.workspace.snapshot (owners docs/py/12-agents.md and docs/py/12-agents.md)
duplicate operation lookup key omp.env.workspace.snapshot (omp.env.workspace.snapshot and omp.env.workspace.snapshot)
omp.ui.message_renderer violates the (payload, ctx) callback ABI
omp.ui.completion violates the (payload, ctx) callback ABI
omp.ui.shortcut violates the (payload, ctx) callback ABI
omp.ui.command violates the (payload, ctx) callback ABI
omp.renderer violates the (payload, ctx) callback ABI
```

## Actual missing DATA-operation diagnostics

```text
DATA dispatch operation omp.env.fs.privileged_mutation is missing from the runtime spec
DATA dispatch operation omp.env.workspace.list is missing from the runtime spec
DATA dispatch operation omp.env.worktree is missing from the runtime spec
DATA dispatch operation omp.env.mcp.status is missing from the runtime spec
DATA dispatch operation omp.env.mcp.subscribe is missing from the runtime spec
DATA dispatch operation omp.env.mcp.reset is missing from the runtime spec
DATA dispatch operation omp.env.mcp.resource is missing from the runtime spec
DATA dispatch operation omp.env.mcp.prompt is missing from the runtime spec
DATA dispatch operation omp.env.mcp.invoke is missing from the runtime spec
DATA dispatch operation omp.env.mcp.config is missing from the runtime spec
DATA dispatch operation omp.env.mcp.invalid is missing from the runtime spec
DATA dispatch operation omp.env.Process.info is missing from the runtime spec
```

These are the checker's literal findings, not approved public API additions. For example, `omp.env.mcp.invalid` is the dispatcher's missing-operation sentinel and needs semantic review rather than an invented public API row.

## Actual missing Python CONTROL diagnostics

The existing four-operation debt baseline remains unchanged. Duplicate occurrences are retained here to match the job log exactly.

```text
Python CONTROL operation omp.devices.dynamic_mount in crates/py/python/omp/devices.py has no generated spec row
Python CONTROL operation omp.devices.set_availability in crates/py/python/omp/devices.py has no generated spec row
Python CONTROL operation omp.devices.refresh in crates/py/python/omp/devices.py has no generated spec row
Python CONTROL operation omp.devices.invoke in crates/py/python/omp/devices.py has no generated spec row
Python CONTROL operation omp.hooks.dispatch in crates/py/python/omp/hooks.py has no generated spec row
Python CONTROL operation omp.mcp.invoke in crates/py/python/omp/mcp.py has no generated spec row
Python CONTROL operation omp.mcp.mount in crates/py/python/omp/mcp.py has no generated spec row
Python CONTROL operation omp.mcp.unmount in crates/py/python/omp/mcp.py has no generated spec row
Python CONTROL operation omp.mcp.servers in crates/py/python/omp/mcp.py has no generated spec row
Python CONTROL operation omp.prompts.invalidate in crates/py/python/omp/prompts.py has no generated spec row
Python CONTROL operation omp.convars.declare in crates/py/python/omp/convars.py has no generated spec row
Python CONTROL operation omp.convars.get in crates/py/python/omp/convars.py has no generated spec row
Python CONTROL operation omp.convars.observe in crates/py/python/omp/convars.py has no generated spec row
Python CONTROL operation omp.context.message.parts in crates/py/python/omp/context.py has no generated spec row
Python CONTROL operation omp.context.message.verdict in crates/py/python/omp/context.py has no generated spec row
Python CONTROL operation omp.context.message.raw_args in crates/py/python/omp/context.py has no generated spec row
Python CONTROL operation omp.context.view in crates/py/python/omp/context.py has no generated spec row
Python CONTROL operation omp.context.usage in crates/py/python/omp/context.py has no generated spec row
Python CONTROL operation omp.context.pin in crates/py/python/omp/context.py has no generated spec row
Python CONTROL operation omp.context.unpin in crates/py/python/omp/context.py has no generated spec row
Python CONTROL operation omp.context.compact in crates/py/python/omp/context.py has no generated spec row
Python CONTROL operation omp.context.epoch in crates/py/python/omp/context.py has no generated spec row
Python CONTROL operation omp.telemetry.export.stop in crates/py/python/omp/telemetry.py has no generated spec row
Python CONTROL operation omp.telemetry.export.stats in crates/py/python/omp/telemetry.py has no generated spec row
Python CONTROL operation omp.telemetry.flush in crates/py/python/omp/telemetry.py has no generated spec row
Python CONTROL operation omp.telemetry.query in crates/py/python/omp/telemetry.py has no generated spec row
Python CONTROL operation omp.telemetry.rev_metrics in crates/py/python/omp/telemetry.py has no generated spec row
Python CONTROL operation omp.telemetry.span.open in crates/py/python/omp/telemetry.py has no generated spec row
Python CONTROL operation omp.telemetry.span.close in crates/py/python/omp/telemetry.py has no generated spec row
Python CONTROL operation omp.provider.retract in crates/py/python/omp/provider.py has no generated spec row
Python CONTROL operation omp.provider.replace in crates/py/python/omp/provider.py has no generated spec row
Python CONTROL operation omp.provider.models in crates/py/python/omp/provider.py has no generated spec row
Python CONTROL operation omp.provider.is_authenticated in crates/py/python/omp/provider.py has no generated spec row
Python CONTROL operation omp.provider.request in crates/py/python/omp/provider.py has no generated spec row
Python CONTROL operation omp.provider.models in crates/py/python/omp/provider.py has no generated spec row
Python CONTROL operation omp.provider.watch_models in crates/py/python/omp/provider.py has no generated spec row
Python CONTROL operation omp.ui.dynamic_mount in crates/py/python/omp/ui/__init__.py has no generated spec row
```

## Validation status

The observed run reached all checker validations and reported no stale-owner-path, setting/default, telemetry, phase-matrix, or dependency-policy failures. The checker remains a failing gate until the unresolved contracts are modeled or their dispatch semantics are corrected. The callback validation function and its two unit tests were extracted unchanged into a standalone Rust test binary; both passed. Formatting and diff checks passed. The complete checker and workspace have not been rebuilt locally after these changes.
