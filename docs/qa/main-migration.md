# Rust primary-system migration

The owner delegated the main-branch decision on 2026-09-08 and requested a
fully functioning system. The chosen destination is the Rust implementation
at the repository root. TypeScript remains recoverable from Git history and
the annotated tag `legacy/typescript-before-rust-20260908`, pointing to
`72b2d32e5f83f65d25b5fbe24fc98039c4dd3bc6`.

## Integration boundary

`fix/integration-foundation-20260908` is the shared integration branch.
Issue branches are inputs to this branch, not separately deployable systems.
Its migration history join preserves both previously unrelated histories and
deliberately retains the Rust tree. The join does not claim TypeScript source
was mechanically ported or that any acceptance test passed. Its tree was
checked byte-for-byte against its Rust parent; main is an ancestor afterward.

Do not force-push main. Before merging, fetch main again, review any changes
since the preserved revision, integrate them deliberately, and verify that
the proposed merge has no conflicts. The migration itself is not permission
to discard subsequent main changes.

## Release requirements

- Account for every open issue and its explicit acceptance criteria.
- Exercise the combined implementation, including real terminal behavior,
  restart and failure recovery, output completeness, and required duration
  and resource limits. A compile check is not an executed test.
- Preserve the frozen CI gate, test assertions, and acceptance thresholds.
  Skipped tests and missing evidence do not count as passes.
- Use the owner's existing Anthropic subscription for required real-model
  evidence through a supported authentication path. Do not substitute API
  billing or mock requests for real-model acceptance.
- Publish exact tested revisions, raw results, failures, and remaining gaps.
- Merge to main only after the complete acceptance gate is satisfied.

This document records the destination and merge policy, not a readiness
claim. The all-issues goal remains incomplete. Local fixes and passing
individual proofs do not establish that the combined system is ready.
