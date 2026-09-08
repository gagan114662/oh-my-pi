# OMP adoption of the Anthropic reference (#151)

This is a bounded reference/discovery contribution, not completed Anthropic
runtime acceptance. Source audit base: `82660622db76008f46b0a6b8392c4c79afe19841`.
Upstream: [anthropics/skills at the pinned revision](https://github.com/anthropics/skills/tree/41bbe19d1a1a7eaab5e7bb9050a417e5c6cffc8f/skills/claude-api).
All 70 upstream files (1,164,364 bytes) are included unchanged under `upstream/`,
including the Apache-2.0 `LICENSE.txt`. `UPSTREAM.json` records each original Git
blob identity, SHA-256, and size. The OMP wrapper and verifier are separately
authored; no upstream file was edited. No upstream NOTICE file was present in
this pinned skill subtree.

## Install and load

Within this repository, the tracked `.omp/skills/claude-api` directory is already
in the native project skill discovery root. To install in another project,
copy that whole directory, including `upstream/`, to its `.omp/skills/claude-api`.
From the target project, run:

```sh
python3 .omp/skills/claude-api/verify.py
```

A failed verification names missing/changed files; restore the complete pinned
bundle before use. No download, credentials, dependency installation, or model
request is involved. `disable-model-invocation: true` keeps this reference out
of automatic prompt facts. Explicit native invocation remains available:

```text
/skill:claude-api Audit OMP's Rust Anthropic streaming and token accounting; preserve the current provider and authentication constraints.
```

This is a representative command for an already configured OMP session, not a
record of executed production invocation. The native skill expander supplies
the skill's absolute base directory. References are readable as, for example,
`skill://claude-api/upstream/shared/token-counting.md`. The existing resolver
rejects traversal and reports `File not found: <path>` for missing assets.
The verifier additionally detects missing files before reference use.

## Source-to-code gap table

“Already satisfied” below means an implementation exists at the audited source;
it does not mean this contribution ran its tests or verified the live service.

| Reference | OMP source / behavior | Disposition and next proof |
| --- | --- | --- |
| `upstream/SKILL.md`, relative language/shared files | Native roots and explicit prompt expansion in `crates/driver/src/discovery/skills.rs`; this complete pinned bundle and preflight verifier | Implemented in this slice. Discovery/expansion/reference regression added; production invocation remains pending under [#151](https://github.com/gagan114662/oh-my-pi/issues/151). |
| `upstream/SKILL.md` unsupported-language guidance; `curl/examples.md` | Rust uses `crates/ai/src/codec/anthropic.rs`, including native Messages and `lower_count_tokens` HTTP lowering | Already satisfied as an architectural route. The pinned skill supplies no Rust SDK; translating Python/TS SDK methods into imagined Rust APIs is inapplicable. No SDK replacement proposed. |
| `python/claude-api/streaming.md`, `shared/tool-use-concepts.md` | `anthropic.rs` decodes `input_json_delta`, maps tool results/stop reasons; `repairable_tool_arguments_remain_raw_until_recovery` and `empty_successful_tool_results_lower_to_the_empty_string_not_an_empty_array` exist | Already satisfied in source. Run affected codec tests and actual source-bound streaming/tool proof; live protocol drift remains pending under #151. |
| `shared/prompt-caching.md` | `anthropic.rs` serializes `cache_control` and 5m/1h retention; `merge_usage` distinguishes cache reads, writes, and the one-hour subset | Already satisfied in source. `cache_creation_breakdown_carries_one_hour_subset` covers replacement/zeroing in code; runtime cache efficacy and A/A comparability remain [#19](https://github.com/gagan114662/oh-my-pi/issues/19) / [#50](https://github.com/gagan114662/oh-my-pi/issues/50). |
| `shared/token-counting.md` | `lower_count_tokens`, `encoded_count_tokens`, and `count_tokens_response_is_exact_and_provenanced` in `anthropic.rs` | Already satisfied for direct transport in source; cloud-adapter rejection is explicit. Current service compatibility and measured token accounting remain pending #151. |
| `shared/error-codes.md` | `classify_http_error` maps provider errors to typed errors without retaining the message; tests include `direct_and_vertex_errors_are_typed` | Already satisfied in source. Broader recovery/commit semantics and actual rate-limit behavior are not claimed verified here. |
| `shared/models.md`, `shared/model-migration.md`, `shared/live-sources.md`, `shared/cost-optimization.md` | Catalog model/provider compilation in `crates/catalog/src/compile.rs`; configurable wire/reasoning policies consumed by `anthropic.rs` | Pending current official-source audit under #151 before any defaults/pricing/capability change. No such changes in this contribution. Pinned example defaults are not current-service evidence. |
| Agent SDK / Managed Agents reference sections | OMP composes its own production kernel in `crates/driver/src/headless/kernel.rs` | Inapplicable as a drop-in kernel replacement. A Claude Code or Agent SDK run alone cannot qualify OMP's production path. |
| API authentication examples | `crates/ai/src/auth/oauth/custom/anthropic.rs` contains an OAuth implementation | Pending supported-route validation under #151. Code existence does not establish permission/support to relay the owner's subscription token. No tokens read, copied, or used here; no API-key fallback. |
| Benchmark and quality examples | Existing benchmark adapter and provider proof work | Pending [#19](https://github.com/gagan114662/oh-my-pi/issues/19), [#50](https://github.com/gagan114662/oh-my-pi/issues/50), [#62](https://github.com/gagan114662/oh-my-pi/issues/62), [#67](https://github.com/gagan114662/oh-my-pi/issues/67); this reference bundle does not satisfy them. |

Code links at the audited source:
[skill discovery](https://github.com/gagan114662/oh-my-pi/blob/82660622db76008f46b0a6b8392c4c79afe19841/crates/driver/src/discovery/skills.rs),
[Anthropic codec](https://github.com/gagan114662/oh-my-pi/blob/82660622db76008f46b0a6b8392c4c79afe19841/crates/ai/src/codec/anthropic.rs),
[catalog compilation](https://github.com/gagan114662/oh-my-pi/blob/82660622db76008f46b0a6b8392c4c79afe19841/crates/catalog/src/compile.rs),
[OAuth implementation](https://github.com/gagan114662/oh-my-pi/blob/82660622db76008f46b0a6b8392c4c79afe19841/crates/ai/src/auth/oauth/custom/anthropic.rs).

## Authentication boundary and remaining acceptance

The owner requires **Claude subscription only, with no API-key fallback**.
A supported authentication route for direct OMP subscription requests has not
been established in this work. Official Claude Code/Agent SDK authentication
is a separate client route; replacing OMP with it would not verify OMP's kernel.
This skill neither authorizes billing nor proves compatibility of existing
subscription credentials with another client.

The complete bundle passes offline identity verification. Both offline Python
checks passed (complete bundle; missing and corrupted reference with restoration),
via `PYTHONDONTWRITEBYTECODE=1 python3 scripts/test_claude_api_reference.py -v`.
Rust formatting and diff checks passed. Native Rust discovery,
explicit prompt expansion and missing-reference checks are added but unrun here;
full affected driver tests/docs, actual production invocation, current official
API/model/pricing validation, and a supported subscription-only OMP runtime proof
remain open. No model requests, backend changes, new test ignores, or claims of
mocked traffic as real Anthropic acceptance are part of this slice.
