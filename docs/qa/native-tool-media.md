# Native tool media reaches the model through the session store

The production read-tail QA at source `0d1` returned successful video metadata,
but its tool result and following model request contained no PNG. The read tool
had produced a `PayloadPart::Blob`. The loss occurred at the native environment
transport: `forward_native_event` sent an empty canonical parts list, then the
driver's structured fallback extracted only text and JSON from the raw verdict.
That route never called the native tool's registered prompt projection.

Native completion now uses the same immutable registry that admitted and invoked
the tool to project the exact verdict under its revision, with media enabled.
Blob references must have a valid digest and media type, and their exact bytes
must be retained in the invocation's verdict-delivery store before a successful
verdict is published. The existing driver path then verifies and replicates
those references into the session blob store before journaling the projected
result. Missing, corrupt, or malformed media produces a failed outcome rather
than a successful text-only result. Detached outcomes keep their existing path.
This applies to all registered native tool prompt projections. Output limits,
driver fallback, and video PNG oracles remain unchanged.

Schema revision 19 adds `OutputProjection.complete_parts`: an invocation-scoped
JSON artifact containing the full canonical wire parts whenever their preview
exceeds the existing wire limit. Native and Python worker forwarding retain
this artifact before clipping. Raw outcome sizes, omission, and integrity checks
remain independent. The driver retrieves and decodes the full parts through the
existing hash-checked, cancellable, 64 MiB maximum artifact transfer, then applies
media replication and the agent's existing central model-output bounding. It
replaces the preview instead of appending it, so text and media are not doubled.
Central bounding can now produce truthful omission receipts and complete text
recovery while preserving media references that were outside the wire preview.
An oversized or unavailable recovery artifact fails the invocation; no clipped
success is published without recoverable content. The environment handshake
minimum is now 19: older readers must not silently ignore the recovery field.

The focused regression exercises actual native terminal forwarding with a
registered typed tool and a managed blob host: it requires the media part on
the verdict, the exact retained bytes, an invocation-scoped delivery lease,
and one terminal message. Separate cases reject absent bytes, invalid hashes,
and malformed media types. These test media transport, not PNG decoding.
Additional layered regressions cover a compact typed payload whose prompt
expands beyond the wire limit with trailing media, exact retained recovery and
media leases, driver invocation-scoped transfer and decoder restoration without
preview duplication, the retrieval ceiling, and the central omission receipt
with complete text recovery. These are component proofs, not a single joined
production request through all three components.
Existing production QA remains the independent preview/frame/timestamp PNG
oracle and must be rerun against a binary built from the new source.

Static formatting and diff checks passed. New Rust tests, complete affected
`omp-proto`/`omp-envd`/`omp-driver`/`omp-agent` targets and doctests, and production read-tail QA remain
pending; no runtime success is claimed from the source diagnosis alone.

Canonical tool-result blobs with an image MIME essence now become inference
images; other blobs remain documents. OpenAI Chat lowering moves image bytes
into an associated user image message after the full assistant tool-call batch
has received its textual tool replies. Call IDs, metadata text, result order,
and the dialect's assistant-transition requirement remain intact. Incomplete,
orphan, or duplicate image-bearing batches fail encoding; unsupported documents
continue to fail, and non-vision profiles retain the existing omission notice.
The canonical journal is unchanged by this provider-specific wire projection.

Codec regressions start from canonical thread tool calls and results containing
a valid one-pixel PNG and verify exact image URL bytes, parallel mixed results,
out-of-order completion, multiple results in one message, placement before the
next user turn, text-only profiles, and malformed batches. These tests are not
provider calls and do not replace the unchanged read-tail PNG oracle. Rust tests
and that production QA remain pending on this commit.
