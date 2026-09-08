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

Gap: explicit legacy migration and the complete operation acceptance gate remain
pending. The hosted results below establish some CLI and lift behavior, but no
complete #49 acceptance claim follows from this implementation.

### Operation regression coverage

The existing production `Session::compaction` regression now verifies every
physical frame, its summary/frame blob identities and retained boundary, and
checks the same seal after replay. The import regression invokes `import_file`,
verifies the native chain and causal links, reads the original foreign bytes
back from the addressed CAS object, and checks the projected message. This
covers conversion; it does not exercise picker selection or atomic publication.
The rewind regression additionally prunes the actual Session journal, checks
all retained entries against the selected branch, and compares the projected
conversation after replay. Internal DOM handles may be reassigned by pruning.
No existing behavioral assertion was removed.

P10 verifies the journal from real dispatch of lifted arguments, its call
revision/arguments, terminal result cause and replayed snapshot. The original
synthetic proto test is retained. Additional cases now admit and settle an
actual Session call, reopen its verified journal, and apply the production
history projection/lift. Recorded `rep.1` calls must transform to the live
revision; calls without a recorded family must remain unchanged. Both cases
require byte-for-byte and seal stability across projection.

New registry-backed calls retain their original revision family alongside the
numeric `tool.call@1` revision. The optional field preserves historical payload
decoding without inventing missing provenance. Session admission accepts a
full `&Rev` or an explicitly unknown-family numeric revision through one
`CallRevision` representation; all registry-backed kernel/local admission
paths pass the resolved full revision, including streaming calls. Replay
materializes `omp/tool-rev`, and projection carries that property and the
stored terminal verdict needed for a deterministic lift. It never consults
today's registry to guess an absent historical family. This is journal
provenance, not a claim that an unkeyed seal authenticates its author.

The session regression covers both complete and streamed admission followed
by settlement/reopen; a decoder regression preserves absent historical family.
Local journal/session execution before the additional blob-GC assertions passed
112 tests with three skips. This does not validate the later GC changes.
Hosted results and their remaining gaps are recorded below; explicit legacy
migration remains pending.

### Browser operation evidence

The leaf preserves a fresh JUnit report after each complete app, journal,
session and agent run, and separately runs the complete P10 target plus
paired e2e doctests. The summary requires exactly one passing, non-skipped
named testcase for rewind, branch pruning, blob GC, compaction, import and
lift. Missing, duplicate, malformed, retried or failed case evidence fails the
summary even if a package command reports success. Raw JUnit, logs and hashes
of the operation test sources remain in the artifact; skipped counts are
reported. Compaction and actual blob collection share the Session fixture,
which checks orphan removal, retained summary/frame bytes, unchanged journal
seal and replay state. The complete set still needs a passing hosted run.

The CLI corruption fixture now records a call and terminal result via Session,
then a successor entry. It edits one byte within that middle `tool.result@1`
payload; the evidence checker verifies the declared kind, causal call link,
frame bounds, exact mutation offset and the verifier/Session refusal results.
This is an explicitly deterministic fixture, not evidence that a provider or
external tool executed. Existing legacy/torn/empty and expected-tip refusal
checks remain in place.

### Hosted result at `859e0af6ba`

[Run 34276417194](https://github.com/gagan114662/oh-my-pi/actions/runs/34276417194)
failed overall. Its actual CLI corruption/refusal fixture passed. Independent
comparison of the retained original and edited journals found exactly one
changed byte at offset 2004, within the middle `tool.result@1` frame spanning
1744 through 2022 (end exclusive). The CLI reported the matching entry ID and
frame start 1744. The original and edited bytes and raw CLI reports are retained
in the run artifact. This validates the deterministic Session/CLI fixture, not
a real-model tool run.

The full journal package passed 51 tests with three skips, the agent package
passed 229 with zero skips, and the complete P10 target passed its one test.
Their paired doctest commands succeeded with zero examples. The app and session
test suites did not execute: compilation failed on a test's `CoerceIssue`
`Debug` requirement and an iterator borrowing a session being closed. As a
result, the operation report correctly failed rewind, pruning, GC, compaction,
and import rows while accepting the executed lift row.

Commit `5b8e359f7d` corrects both compile errors without removing assertions.
All app/session targets then passed local `cargo check` through `just`; that is
compilation evidence only. The replacement hosted run must execute the missing
suites and operation rows before those requirements can be marked satisfied.

## References

- [0003](0003-one-authoritative-session-tree.md): session authority and fold
- [0006](0006-host-policy-sandbox-stub.md): trusted writer placement
- Issue #49: journal integrity and browser-visible verification
