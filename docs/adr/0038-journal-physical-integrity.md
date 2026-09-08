# 0038. Journal physical integrity is distinct from journal authority

Status: proposed
Date: 2026-09-08
Area: state

## Context

The session tree is the fold of journal entries (ADR 0003). The `prior` id is a
semantic branch link. It intentionally skips abandoned history after rewind;
it cannot also identify the immediately preceding physical frame. Previously,
SSE syntax and branch validation accepted a well-formed payload edit as history.

## Decision

New journal writes prepend one canonical header to each existing SSE frame:

```
integrity: sha256-v1 <previous-physical-seal> <this-frame-seal>
```

Each lowercase digest is 64 hexadecimal characters. The genesis predecessor is
32 zero bytes. `omp_core::Hash32` computes SHA-256 over the exact domain bytes
`omp/journal/physical-frame/sha256-v1\0`, the predecessor's 32 binary bytes, and
every byte of the existing SSE body, including comments, JSON spelling, links,
unknown fields and the committing blank line. The header's fixed spelling and
lowercase hex are mandatory. Duplicate or misplaced integrity fields fail.
There is no signature, MAC, secret key or identity authentication.

A current-frame digest is required, so editing the last complete frame is
observable without a successor. The chain follows physical order across every
branch, including abandoned entries. Existing SSE codec fixtures remain valid
codec fixtures; encoding an `Entry` with the legacy codec does not seal it.

Verification precedes payload decoding and fold. Errors identify the first
bad physical frame's start offset and its parseable id, if any. An id whose
bytes have been destroyed cannot honestly be recovered. `Journal::verify`
and read-only `Journal::verify_path` report sealed/legacy counts, committed
bytes, incomplete-tail bytes and the final seal. No unsealed entry is counted
as verified. `Journal::open_verified` verifies the held file before truncation
and supports an independently supplied expected tip. Both `Session::open`
variants use that strict boundary. Legacy inspection remains available, but
session replay, append and GC refuse legacy entries. Migration must be an
explicit subsequent operation preserving source bytes and provenance; it
must not assert the historical truth of newly hashed legacy bytes.

A write or sync failure poisons the live writer until it is closed and reopened.
Its cached tip cannot authorize another append after an uncertain persistence
result. Reopen verifies actual complete bytes and recovers only a torn tail.

The blank line remains the commit boundary. An incomplete suffix is reported
and writable open recovers the complete prefix as before. This is not evidence
that missing bytes were harmless. Complete suffix deletion and an edited last
commit delimiter can resemble an earlier valid prefix or crash; an external
expected tip detects loss of the expected history and is checked before any
truncation. A colocated writable sidecar is not an independent trust anchor.

## Consequences

An edit, insertion, deletion or reordering without recomputing affected seals
is detected. An attacker able to rewrite all affected seals can forge an
internally consistent history. An attacker able to submit false entries to
the legitimate writer can create a false, well-chained history. Neither case
is prevented by unkeyed hashing. ADR 0006's writer placement and host filesystem
authority must keep the writer and any independent tip outside the agent's
reach to defend against those attackers; this implementation does not assume
that isolation already exists.

GC verifies its source before any mutation. Abandoned-frame pruning is an
intentional physical rewrite: retained logical entries keep their ids/links,
and receive a newly computed physical chain. The expected tip changes and any
external anchor must be updated through an authorized operation. Blob GC does
not edit frame payloads. A seal proves consistency of encoded blob references,
not existence or integrity of separately stored bytes (the CAS owns that).

## Status in omp

**Partial.** First implementation slice: `crates/journal/src/integrity.rs`, journal
append/open/scan/GC, and the session open boundary. Added tests cover valid
frames, byte edits in middle/final payloads, physical insertion/deletion/
reordering, branch/prune behavior, torn suffixes, expected-tip mismatch and
legacy rejection. Existing frozen tests and fixtures are unchanged.

Local validation at `322e01521e`: the complete `omp-journal` and `omp-session`
all-targets nextest run passed 110 tests. Three unchanged subprocess helper
entrypoints were skipped by the outer runner and exercised by passing parent
tests. Both crate doctest commands succeeded with zero doctests. These local
results do not substitute for browser-visible production evidence.

The application now exposes read-only `omp session verify PATH [--json]
[--expected-tip HEX]`. Its text and JSON reports include status and the first
divergent frame id/offset when available. Invalid, legacy, torn and empty files
return failure. The new actual-executable regression covers verification while
a writer is held, byte corruption, legacy/empty/torn status and expected-tip
mismatch; its runtime validation is pending separately from the core run above.

The standalone `.github/workflows/journal-integrity.yml` leaf records actual
CLI stdout/stderr, original and edited `.oms` files, the independently captured
fixture id/offset, session-open refusal and expected-tip mismatch. Its summary
checks artifact contents as well as every command/log exit. Complete affected
app/journal/session targets and doctests continue after a proof failure. The
fixture's process owner uses bounded polling and kill/reap cleanup, retaining
failure logs. No authentication, model call or fabricated session is used.

Gap: CLI runtime validation and hosted proof execution remain pending. Explicit legacy
migration, browser-readable production session corruption evidence, and
operation-specific compaction/lift/import proofs are not delivered by this slice. No complete #49 acceptance
claim follows from the implementation or from unit test existence.

## References

- [0003](0003-one-authoritative-session-tree.md): session authority and fold
- [0006](0006-host-policy-sandbox-stub.md): trusted writer placement
- Issue #49: journal integrity and browser-visible verification
