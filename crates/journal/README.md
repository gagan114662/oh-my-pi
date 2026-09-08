# omp-journal

`omp-journal` owns the durable, crash-tolerant history of an omp session. A session is one flat `.oms` file containing raw Server-Sent Events frames, plus content-addressed blobs for payloads that should not be repeated inline.

The crate is intentionally structural rather than behavioral. It assigns monotonic identities, commits complete frames, recovers a torn tail, and selects a branch through `prior` links. It does not interpret session state: replay and materialization belong to the DOM/session layer, so the journal remains the sole durable authority without becoming a second state model.

Journal GC holds an exclusive namespace lease from authoritative `.oms` inventory through the content-addressed-store sweep. Complete branch histories, child jobs, checkpoints, and imported sessions retain their referenced blobs until their journal history is pruned. Dry-run and apply use the same age and reachability plan; traversal bounds are proven before CAS deletion begins, and cancellation stops at filesystem and destructive-operation boundaries.


## Read-only verification

`omp session verify /path/to/session.oms` inspects physical SHA-256 seals without
acquiring the journal writer lock or truncating a torn tail. Add `--json` for a
structured status report, or `--expected-tip <64-hex-digest>` to compare with a
tip retained independently of the journal. Only a nonempty, fully sealed file
with no torn tail returns success. Legacy unsealed, torn, empty and invalid
files return nonzero and remain unchanged. Integrity failures name the first
bad physical frame's id (when parseable) and byte offset.

A checksum chain proves internal consistency, not authorship. Whole-chain
rewriting and false records from a legitimate writer require a trusted writer
boundary; detecting complete suffix loss requires an independently retained
tip. See [ADR0038](../../docs/adr/0038-journal-physical-integrity.md).
