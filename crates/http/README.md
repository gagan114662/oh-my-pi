# omp-http

`omp-http` owns OMP's process-wide outbound Reqwest clients and Rustls provider policy.
It exposes cheap `Client` clones that share connection pools across app, driver, and
environment-host call sites.

## Philosophy

TLS configuration and connection-pool lifetime are process policy, not request policy.
Callers select the shared default or redirect-disabled client; specialized clients start
from the provider-aware builder and remain owned by the subsystem defining that policy.

`read_bounded(response, limit)` is the shared response collector used by environment
HTTP egress. It rejects both oversized Content-Length declarations and actual
streamed bytes exceeding the caller's ceiling. Request-builder timeouts remain
active while reading the body; dropping the collector cancels the owned stream.
Streaming provider transports may continue to consume responses incrementally.

The `response_contracts` integration tests use real loopback HTTP sockets. Their
slow peer offers 1 GiB as repeated 16 KiB chunks, observes connection teardown,
and never allocates a 1 GiB payload. Tests cover declared/chunked size limits,
body deadlines, mid-body cancellation, exact-limit success, and both shared
redirect policies. The opt-in CI `http_tests` checkpoint publishes the peer's
observed byte counts and nextest logs. Workspace-wide inventory and baseline
failure evidence are tracked separately by issue #45.
