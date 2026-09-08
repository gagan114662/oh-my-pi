# omp-envd

`omp-envd` is OMP's live project-environment host. It assembles and serves the
environment daemon and owns project-scoped filesystem and document access,
process execution, workspace search, blob storage, tool dispatch, policy, and
extension runtime resources exposed through the environment protocol.

This is the crate to change for host behavior. `omp-env` is only the typed
client and framing boundary; it does not contain an alternate host.

## Structure

- `server` owns environment-service dispatch, project state, client
  connections, and the `EnvServer`/`EnvdError` server boundary.
- `workspace`, `docs`, `document_cache`, `search_backend`, and `tool_search`
  provide workspace, search, and document operations.
- `exec`, `process_store`, `process_log`, and `direnv` manage
  commands, named processes, logs, and shell environment setup.
- `tools` and the `tool_*` modules implement daemon-backed tool operations.
- `exthost` owns extension manifests, lifecycle, CONTROL routing, quotas,
  service routing, cancellation, and the sole Python extension child role,
  `__omp-ext-host`.
- `eval` owns the lazy, killable `__omp-eval-child` machinery. The built-in
  `py_eval` is an Environment-routed tool backed by a fresh disposable eval
  namespace for each call.
- `worker_pool` owns named-worker placement and generation-fenced DATA
  transport. Named workers are distinct from extension hosts and eval
  children; they are not a legacy Python tool-child route.
- `policy`, `admission`, `http_egress`, `vault`, and `recovery` enforce access
  decisions and manage durable runtime state.
- `run` starts the platform transport. `ProjectEnvironment::attach` joins the
  build-keyed detached daemon and composes session-only tools locally.

## Document authority (docserver module)

The `docserver` module is the project-scoped authority over document state,
portable filesystem values, revision-aware edits, transactions, file watching,
and language-server sessions. Connection-specific behavior remains isolated in
sessions, while bounded protocol framing and adapters keep LSP and edit formats
from becoming independent sources of state.

The `omp` executable recognizes the hidden `__omp-eval-child` and
`__omp-ext-host` arguments because those children re-enter the same binary.
Their entry functions and runtime implementations remain owned by `omp-envd`;
`omp-app` only performs process-level dispatch. Parent processes do not
preflight-boot CPython: each Python child initializes its own interpreter.

## Philosophy

Each project and executable generation has one detached environment daemon.
Environment-locus tools — including opt-in `py_eval` — and filesystem,
process, document, browser, debugger, and memory effects execute there.
Session-locus tools, client-layer extension hosts, MCP, presenters, and agent
controls stay in the attaching process behind the same partitioned
`EnvClient`. Named-worker placement remains a separate execution facility. An
embedded full host is used only as a loud spawn fallback or by explicitly
isolated compositions.

The document socket is build-stable while environment sockets are build-keyed.
`DocumentHost` reconnects after a server restart, and a surviving current-build
environment may rehost the document authority without invalidating its clones.
A stale-build daemon drains without rehosting and releases authority as soon as
its last client disconnects.

The crate is deliberately below the headless driver and application layers.
Capabilities that require regime state, inference composition,
application-authored content, host RPC resources, or telemetry delivery enter
through `RegistryBridges`. `omp-driver` constructs those bridges and the
session composition; `omp-envd` does not import app presentation policy.

## Development

Run `just setup-python` once before commands that link embedded Python. Then
use the workspace recipes:

- `just check-pkg omp-envd`
- `just test-pkg omp-envd`

Run joined behavior separately with `just e2e` or the exact narrower E2E
recipe shown by `just --list`.

## Native HTTP destination authority

`sv_native_http_policy` uses the existing `SandboxNetworkPolicy` JSON schema.
Its default is `{"mode":"open"}`, preserving authorized broad native HTTP.
Command sandbox enablement and `sv_sandbox_network_mode` remain independent.
A restricted example is:

```json
{"mode":"proxy","allow_domains":[{"domain":"api.example.com","ports":[443]}],"allow_ports":[443],"dns":"proxy_only"}
```

The host captures this policy before tool execution. Later console writes do
not widen that running Environment. Delegated contexts also retain the captured
parent restrictions in `sv_native_http_inherited_policies`, which scripts cannot
write. Every policy in the inherited list must allow the destination. Domain
ports remain paired with their domain; deny rules win. Invocation effects still
require `env.net`, exact tokens, connection ownership, and current generations.

Native HTTP runs on the session EnvServer because that host owns the invocation
authority. The partition router sends HTTP and its cancellation to that host.
The project daemon publishes its baseline in the authenticated `ServerHello`;
the session host intersects it with local and inherited policy. A restricted
session refuses a legacy owner that omits the baseline. Direct broad-owner HTTP
remains supported. Policy is never accepted from request props or ClientHello.

Restricted requests check the first destination and every redirect. Both HTTP
and HTTPS are supported, but HTTPS-to-HTTP redirects are denied. DNS `deny`
allows only numeric addresses; `proxy_only` and `allow` resolve through the
trusted host. All resolved addresses must pass the shared sandbox address
validator. The transport connects to those exact addresses with URL-hostname
TLS verification, without re-resolution or ambient proxy routing. Connection
pools are bounded and reused. Cross-origin credentials are stripped as before;
a caller-supplied Host header cannot override the admitted URL authority.

Cancellation and invocation settlement stop an in-flight request, including a
pending redirect or response body. Policy changes are startup configuration;
changing a project baseline requires rebuilding its owning composition.
