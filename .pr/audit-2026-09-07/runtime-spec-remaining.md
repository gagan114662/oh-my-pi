# Runtime spec remaining drift

Static literal scan after path repair (not a successful gate execution).

Environment literals without rows: 12. The checker also currently classifies internal/sentinel names (e.g. mcp.invalid) as operations, so this list needs semantic triage.

- `omp.env.Process.info`
- `omp.env.fs.privileged_mutation`
- `omp.env.mcp.config`
- `omp.env.mcp.invalid`
- `omp.env.mcp.invoke`
- `omp.env.mcp.prompt`
- `omp.env.mcp.reset`
- `omp.env.mcp.resource`
- `omp.env.mcp.status`
- `omp.env.mcp.subscribe`
- `omp.env.workspace.list`
- `omp.env.worktree`

Python CONTROL call literals without rows: 36 (existing 4-item baseline excluded).

- `omp.context.compact` — `crates/py/python/omp/context.py`
- `omp.context.epoch` — `crates/py/python/omp/context.py`
- `omp.context.message.parts` — `crates/py/python/omp/context.py`
- `omp.context.message.raw_args` — `crates/py/python/omp/context.py`
- `omp.context.message.verdict` — `crates/py/python/omp/context.py`
- `omp.context.pin` — `crates/py/python/omp/context.py`
- `omp.context.unpin` — `crates/py/python/omp/context.py`
- `omp.context.usage` — `crates/py/python/omp/context.py`
- `omp.context.view` — `crates/py/python/omp/context.py`
- `omp.convars.declare` — `crates/py/python/omp/convars.py`
- `omp.convars.get` — `crates/py/python/omp/convars.py`
- `omp.convars.observe` — `crates/py/python/omp/convars.py`
- `omp.devices.dynamic_mount` — `crates/py/python/omp/devices.py`
- `omp.devices.invoke` — `crates/py/python/omp/devices.py`
- `omp.devices.refresh` — `crates/py/python/omp/devices.py`
- `omp.devices.set_availability` — `crates/py/python/omp/devices.py`
- `omp.hooks.dispatch` — `crates/py/python/omp/hooks.py`
- `omp.mcp.invoke` — `crates/py/python/omp/mcp.py`
- `omp.mcp.mount` — `crates/py/python/omp/mcp.py`
- `omp.mcp.servers` — `crates/py/python/omp/mcp.py`
- `omp.mcp.unmount` — `crates/py/python/omp/mcp.py`
- `omp.prompts.invalidate` — `crates/py/python/omp/prompts.py`
- `omp.provider.is_authenticated` — `crates/py/python/omp/provider.py`
- `omp.provider.models` — `crates/py/python/omp/provider.py`
- `omp.provider.replace` — `crates/py/python/omp/provider.py`
- `omp.provider.request` — `crates/py/python/omp/provider.py`
- `omp.provider.retract` — `crates/py/python/omp/provider.py`
- `omp.provider.watch_models` — `crates/py/python/omp/provider.py`
- `omp.telemetry.export.stats` — `crates/py/python/omp/telemetry.py`
- `omp.telemetry.export.stop` — `crates/py/python/omp/telemetry.py`
- `omp.telemetry.flush` — `crates/py/python/omp/telemetry.py`
- `omp.telemetry.query` — `crates/py/python/omp/telemetry.py`
- `omp.telemetry.rev_metrics` — `crates/py/python/omp/telemetry.py`
- `omp.telemetry.span.close` — `crates/py/python/omp/telemetry.py`
- `omp.telemetry.span.open` — `crates/py/python/omp/telemetry.py`
- `omp.ui.dynamic_mount` — `crates/py/python/omp/ui/__init__.py`
