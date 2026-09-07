# Runtime specification and live-owner evidence

[Hosted run 34155217508](https://github.com/gagan114662/oh-my-pi/actions/runs/34155217508), at `41ee243633`, compiled and ran the checker successfully before the checker exited 1 with **49 diagnostics**: 12 DATA literals and 37 Python CONTROL occurrences (36 distinct keys). The earlier duplicate-row and decorator-ABI diagnostics are absent from this real run.

## Prepared repair

- Added 48 canonical symbol rows from existing Python signatures, native client/protobuf signatures, owner authorization code, and declared documentation contracts. Eleven rows cover valid missing DATA operations, 36 cover distinct CONTROL keys, and one covers `omp.env.mcp.live-header`, which the old literal scanner silently missed.
- Missing MCP operations return `InvalidArgument` before authorization; the synthetic `omp.env.mcp.invalid` name is no longer presented as an operation requiring metadata.
- The checker includes hyphenated dispatch keys and accepts one optional `--` before its output path, rejecting extra arguments.
- Added runtime-spec admission/alias-uniqueness tests, all-eight-MCP-operation lookup tests, and a production-domain-router test that distinguishes declared context metadata from missing runtime routing.

These changes await hosted validation. Static independent lookup/literal checks find no missing current DATA or CONTROL keys and no duplicate lookup keys. Formatting and diff checks pass. No local Rust build was started.

## Contract sources and proof limits

| Operation group | Phase/authority evidence |
| --- | --- |
| Native `omp.env.*` DATA | `crates/envd/src/server.rs::authorize_data_operation` requires Environment authority and EffectsAuthorized; protobuf requests and `crates/env/src/client.rs` provide exact signatures. |
| Devices and hook dispatch | `crates/envd/src/tools.rs::require_active_invocation`, called by both owners, requires active lifecycle and EffectsAuthorized. |
| Convar requests | `crates/envd/src/exthost/control.rs::ConvarControlAuthority::authorize` admits owned operations without an effect-phase requirement. |
| UI dynamic commands | `crates/envd/src/exthost/presentation.rs::UiControlAuthority::authorize` explicitly uses Open plus `ui.commands`. |
| Telemetry requests | `TelemetryControlAuthority::authorize` uses Open; the class exists, but production binding remains a separate gap below. |
| MCP CONTROL | `ProductionMcpControlAuthority` checks connection identity and delegates to the Environment MCP manager. Mount/list/unmount are Open Environment metadata; invocation carries the declared DATA effect requirement. Metadata alone does not add a new phase guard to this CONTROL owner. |
| Context durable mutations | `docs/py/08-context.md` explicitly defines durable CONTROL before DATA authorization. `DECLARED_CONTEXT_CONTROL` is documented as a declared contract, **not evidence of an implemented owner**. |
| Provider request | `docs/py/13-inference.md` defines paid inference as a durable EffectsAuthorized Core request. Its live owner is not wired by the production UI-only binding. |

## Live wiring gaps still requiring implementation

Static metadata completeness is not runtime completeness. Do not close these gaps based on a green spec gate.

1. **All nine context keys have no domain route.** `ControlDomain::handles` in `crates/envd/src/exthost/control.rs` has no `omp.context.*` branch. `CompositeControlAuthority::owner` returns `unhandled_operation` when no route handles a key. `crates/envd/tests/domain_control_router.rs` exercises the real `HostControlAuthorityFactory` with all owners supplied and now verifies actual requests to `omp.context.view`, `.pin`, and `.compact` still return that error despite existing metadata. The remaining keys are `.usage`, `.epoch`, `.unpin`, `.message.parts`, `.message.verdict`, and `.message.raw_args`.
2. **Provider, prompts, and telemetry require external owners absent from the production binding inspected.** `production_control_bindings` in `crates/envd/src/server.rs` installs manifest-gated late-bound slots. `LateBoundControlAuthority::owner` rejects an absent lease/factory. The production call in `crates/app/src/chat_cmd.rs` supplies only `ui` in `ExternalDomainControlFactories` and defaults the other fields. Thus six provider keys, `omp.prompts.invalidate`, and seven telemetry keys have declarations but no demonstrated usable production binding in this composition. Provider/prompt concrete handlers were not found; telemetry has an existing owner implementation to wire.

Next work must implement and exercise these owners through production composition and replace the no-route proof with a real successful/denied-request contract. Broadening the router or adding metadata without owners is not a fix.

## Exact observed diagnostics before this repair

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
