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
The wire schema, driver fallback, output limits, and video PNG oracles are
unchanged. This applies to all registered native tool prompt projections, not
only video reads; Python worker result projection is unchanged.

The focused regression exercises actual native terminal forwarding with a
registered typed tool and a managed blob host: it requires the media part on
the verdict, the exact retained bytes, an invocation-scoped delivery lease,
and one terminal message. Separate cases reject absent bytes, invalid hashes,
and malformed media types. These test media transport, not PNG decoding.
Existing production QA remains the independent preview/frame/timestamp PNG
oracle and must be rerun against a binary built from the new source.

Static formatting and diff checks passed. New Rust tests, complete affected
`omp-envd`/`omp-driver` targets and doctests, and production read-tail QA remain
pending; no runtime success is claimed from the source diagnosis alone.
