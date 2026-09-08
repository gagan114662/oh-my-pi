---
name: claude-api
description: Explicitly requested review of OMP's Anthropic integration against a pinned official reference, preserving provider choices and authentication constraints.
disable-model-invocation: true
---

# Anthropic reference for OMP

This is an OMP integration wrapper around Anthropic's Apache-2.0 reference
skill at revision `41bbe19d1a1a7eaab5e7bb9050a417e5c6cffc8f`.

1. Run `python3 verify.py` from this skill directory before using the reference.
   A missing or changed file is an actionable failure: report the named file
   and restore the complete pinned bundle before relying on it.
2. Read `upstream/SKILL.md` as reference material. Resolve its relative paths
   against `upstream/`, including `shared/`, `curl/`, and language directories.
   OMP can read them through `skill://claude-api/upstream/<relative-path>`.
3. Keep the user's chosen provider, model, and billing method. OMP is a
   multi-provider Rust application. Upstream provider-switching instructions
   and example defaults are not authorization to change OMP defaults. No
   language-specific Rust SDK is supplied in this pinned bundle; audit OMP's
   existing native Anthropic HTTP codec rather than inventing SDK APIs.
4. For the current owner's work, use Claude subscription only: no Anthropic
   API-key fallback, no paid API calls, and no subscription credential relay
   into an unsupported client. This API reference does not establish a
   supported subscription authentication route for direct OMP requests.
5. Treat model IDs, capabilities, API/SDK examples and prices as dated reference
   information. Verify relevant current official sources before proposing any
   default or integration change. No model request is needed for source review.
6. Distinguish source inspection, local mocked checks, and actual OMP/provider
   execution in findings. Keep unresolved acceptance explicit.

Start the Rust source review with `upstream/curl/examples.md`,
`upstream/shared/tool-use-concepts.md`, `upstream/shared/prompt-caching.md`,
`upstream/shared/token-counting.md`, and `upstream/shared/error-codes.md`.
See `OMP-INTEGRATION.md` for the gap table and
pending runtime evidence. Preserve `upstream/LICENSE.txt` and `UPSTREAM.json`
when copying this whole directory to another project's `.omp/skills/`.
