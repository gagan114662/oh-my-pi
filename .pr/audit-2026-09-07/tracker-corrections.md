# Prepared tracker corrections

These are reviewable drafts. No GitHub issue, comment, PR, or branch-protection setting was changed. Re-read current bodies before publishing; the JSON includes body hashes to detect drift.

## #6 — omp2: prove grep routing through the environment authority

Network-specific approval is implemented in envd exec.rs, including ApprovedSandboxAmendment::Network and jit_approval_names_only_the_detected_capability_and_exact_command. Remove the missing-network-prompt premise. Retain a focused end-to-end grep-routing proof that exercises the production environment authority. Existing test source is not a fresh passing run.

## #11 — omp2: close remaining remote placement and spill proof gaps

The remote decorator and remote_rpc_contract.rs already exist. Packaging uses pickle/source/code. The optional inspect feature uses pyo3-introspection for stub generation, not AST packaging. Remove the unsupported ADR 0006 quotation and AST requirement. Specify missing production placement, cancellation, scoped-environment and large-result behavior against the implemented packaging mechanism.

## #17 — omp2: clarify Python Director ergonomics and remaining integration proof

Python Directors already exist via omp.extensions.director and PyDirector. Remove the Rust-only premise. If agent.direct() is desired, describe it as an API-design request with a migration decision, separately from integration proof for the existing decorator. Do not conflate this with the unimplemented regimes API.

## #20 — Close: the reported in-house TODO count is not reproducible

The specified comment-marker scan yields zero in chat/tui/tool/tools/app. Identifier and string hits are not TODO comments. The body arithmetic also totals 20 rather than 21. Proposed action: close this incorrect issue; any replacement must enumerate actual comment markers and define the scan scope.

## #16 — omp2: task isolation settings do not control actual backend selection

TaskIsolationMode has ten variants. The boolean conversion in spawn.rs builds Python hook metadata; it is not the actual dispatch path. run_child always creates child isolation, including mode None. EnvClient.create_worktree does not carry a backend selector; envd chooses a platform helper. Preserve mandatory isolation. Acceptance: define whether explicit preferences are supported or rejected, align the settings/hook payload with actual execution, and prove the selected behavior on supported platforms. Do not use the nonexistent --isolation CLI flag as the demo.

## #42 — omp2: fix tokio test attribute typo in omp-ai

The #[tokio::tes] attribute in crates/ai/src/auth/aws.rs prevents compilation of that test binary. Replace it with #[tokio::test] and run the existing test target. nextest already fails on test compilation errors; do not claim it silently reports a passing zero-test suite. The prepared patch fixes the attribute, but the changed crate has not compiled on this host.

## #43 — omp2: migrate 210 shell snapshots and remove 66 orphan expectations

Baseline inventory: 301 snapshots, including 211 old shell prefixes, 65 app snapshots naming a deleted source file, and 25 current agent/chat snapshots. One of the 211 shell files also names a deleted parse_program test. The prepared repair renames 210 files byte-for-byte to the current omp_shell namespace and removes 66 genuinely orphaned files. 235 snapshots remain. Acceptance: run the shell suite on a provisioned host, confirm no new snapshots and inspect namespace-only renames. Do not regenerate expected parser output blindly. See the attached hash manifest for migration evidence.

## #44 — omp2: repair contradictory dispatch assertion and preserve real omission diagnostics

The bounded test requested notrunc=false but asserted both a single text part and text plus output_bounded. Remove the contradictory single-part assertion. The audit's proposed production change is rejected: the transport spill indicates actual upstream omission, which notrunc must still disclose. Acceptance: bounded output has the diagnostic; complete notrunc output has none; actual transport omission with notrunc keeps the diagnostic and artifact across journal replay. The prepared patch adds the latter regression without suppressing correct production diagnostics.

## #54 — omp2: enable CI, repair baseline gates, then configure branch protection

PR #59 already contains the trigger change; PR #58 is a blocked report, not an implementation. Preserve push branches main/omp2 and unfiltered PR triggers. Before requiring all five contexts, repair the distinct baseline failures tracked by #60: format drift; runtime-spec path/metadata drift and its TOML document-parser bug; cargo-deny policy failures (unused terminfo exception and missing package-scoped uluru 3.1.0 MPL allowance); and Linux P7 embedded-Python setup failing on the archive's missing zlib-ng license file. The Python notice failure is separate from the cargo-deny license-policy job.

Prepared changes also add .config/docs/tools push coverage, run P9/P10/tool_sources, enforce model lints/current ADR paths, and verify regenerated dependency notices. Dispatch run 34154484336 passed the format job. Its runtime-spec job now reaches and reports contract failures rather than the TOML parse panic. Linux P7 now completes Python setup but job 101843684072 failed because cargo-nextest was not installed; the workflow repair adds that installation, with a fresh hosted proof still required. No branch-protection settings have been changed. Do not claim every context is green or close #54 while those checks remain unresolved.

## #55 — omp2: reconcile declared settings groups with registered convars

Own the shared settings-group invariant here; remove duplicate copies from #22/#23. Empty headings are already filtered. The important contract is that every UI-declared group maps to a live convar and every intended UI convar reaches a declared group; do not rely on a projection that silently drops unknown groups. Correct the unrelated #35 reference. Preserve capability-specific work in #22/#23. No settings-invariant implementation is claimed by this repair.

## #60 — omp2: repair CI baseline formatting, runtime-spec drift and license failures

The initial observation of run 34149276596 recorded format, runtime_spec and licenses failures, with dependent proof jobs skipped at that observation. That is not the complete baseline diagnosis: the Linux P7 path also failed during embedded-Python preparation because python-build-standalone referenced `licenses/LICENSE.zlib-ng.txt` but did not ship that file. `crates/py/scripts/gen-py-notices.py` now uses the checked-in, audited zlib-ng-2.2.4 license fallback; this is distinct from cargo-deny's terminfo/uluru failures.

Runtime-spec repair has two layers. Deleted `crates/app/src/envd/server.rs` and stale settings/telemetry/policy references, plus canonical `sv_interrupt_grace` metadata, were repaired. The checker also parsed manifests with `toml::Value::from_str`, which in toml 1.x parses a value rather than a document; `scripts/check-spec.rs::parse_toml` now parses `toml::Table` and wraps it as `TomlValue::Table`. Fixing that checker bug exposes contract drift; it does not make the contract checks pass. The earlier static inventory of 36 unregistered Python CONTROL literals and 12 environment literals remains a triage aid, not a successful gate result or an exact enumeration of the latest hosted failures; some entries are sentinels. Do not add guessed metadata or suppress failures merely to make the gate green.

Dispatch run 34154484336 provides newer, narrower evidence: format passed; runtime_spec failed on actual contract checks; Linux P7 Python preparation succeeded, then job 101843684072 failed because nextest was absent. The workflow repair installs cargo-nextest for that job; its presence in the patch is not a passing P7 execution. The terminfo/uluru cargo-deny repairs passed the pinned local check, which likewise does not establish that all hosted license/notices checks passed. Keep #60 open until the repaired runtime contract gate, dependency notices, and provisioned Rust/PTY suites produce successful hosted results.

## #41 — replace the conflict calculation and scheduling claims

The original 11-row table produces 25 pairs, not 33. Removing #27 from AGENTS.md removes five pairs; removing #5 from read.rs removes one: **19 pairs remain in that table**, before removing closed issues, separating directory-level references from actual edits, or adding omitted ADR/CI overlaps. The audit's instruction to remove those edges yet retain 25 is also wrong. Do not claim the revised number is a complete scheduling graph.

Use 55 issue numbers in the original #1–#56 set (there is no #53 issue); #41 is the roadmap, so separate it when counting implementation issues. #60 is a later baseline issue. Prefer an explicit enumerated scope to the incompatible 40/47/52 counts. The workspace has 47 crate directories. Replace the old four-wave schedule with dependency constraints: CI baseline repair and test-compilation/snapshot repair first; one coordinated docs batch; runtime fixes by concrete edited-file overlap; feature/architecture work after its acceptance contract is corrected. #24 edits chat/settings.rs, not overlays/settings.rs. #55 owns the shared settings invariant. No “27 conflict-free issues” assertion survives without a new edited-file graph.

## Remaining issue amendments

- #1: create an actual hostile-repository envelope test before requiring it; Factorio is a design-envelope row, not an existing test.
- #2: preserve the existing strict-schema budget degradation proof and narrow remaining dialect/client-repair gaps. The Anthropic tool_strict_mode key is not evidence of consumed OpenAI strict behavior.
- #3/#5/#18: use current implementation paths and explicit gaps. The patch supplies a real `just adr-paths` check for current Status/References sections. #18 includes ADR 0012/0016, architecture docs, deleted agent-module references, ADOPTION, README and public-vs-private planning references. Coordinate ownership of ADR 0023.
- #4: kitty passthrough exists and has test source; separate missing proof from absent sixel encoding.
- #8: do not assume criterion is already a dependency; distinguish highlighter allocation from truncation allocation and define a measurable contract first.
- #9/#29: share browser-projection/client ownership; remove claims that a layout! implementation already exists.
- #10: clippy without -D warnings is advisory. Inline PR annotations need actual wiring. The repaired model linter is now enforced and has an injected-failure proof; it is not a proof of every allocation/error discipline rule.
- #12/#13/#36: speculative/provider/snapcompact behaviors need real executors and contracts. The patch rejects unavailable compact modes; it does not implement those features.
- #14: define the missing TLC configuration, include model paths in workflow triggers, and distinguish model constants from claimed coverage.
- #15: the historical index has 113 rows and no example directories. Do not cite an unrun zero-drift gate as proof.
- #18/#33: historical Python proposals and unimplemented omp.regimes are now labelled explicitly; this is documentation repair, not subsystem implementation.
- #21: lsp/browser are dynamically registered, with live prompt gating. The remaining static-help omission is repaired. Remove irrelevant settings boilerplate.
- #22/#23: retain capability work; move duplicate group-invariant acceptance to #55. Misleading magic-keyword tips are removed.
- #24: demonstrate Settings → Appearance, not nonexistent /theme; include cl_theme_light and the unconstructed watcher. Remove pasted anti-shortcut language unrelated to themes.
- #25: separate deterministic fixture-based CI from longitudinal real-session evaluation; a week of sessions cannot be a per-PR prerequisite.
- #26: extension registration is ADR 0036 and commands ADR 0014, not Director ADR 0015.
- #28: specify the Bun/RPC migration or explicitly approve a different SDK product direction; do not assume a Node runtime.
- #30: verify 16 transport variants and 24 search-provider variants (including Auto/Public); separate OAuth flows from guided API-key UX.
- #39: document LLD/Python/native prerequisites and run workspace plus e2e explicitly. just doctor does not exist and just test excludes e2e. Full proof needs a provisioned host with roughly 20 GiB free, not the current constrained host.
- #45: first define the intended HTTP timeout/size/cancellation contract. Tests cannot establish a nonexistent configured API.
- #46: distinguish malformed-input harness work from existing disk/sha assertions in other tests; do not conflate them.
- #47: separate Python helper integration from the several wording-only failures.
- #48: remove unrelated ADR anti-shortcut boilerplate and keep live CONTROL sink failure as the concrete repro.
- #49/#50: do not cite a journal-integrity workflow that does not exist. Hash-chain/evaluation authority changes require their own architectural implementation and proofs.
- #52: hub.rs line-count differences are cosmetic, not behavioral evidence.
- #56: the patch makes unsupported local - return failure with stderr; option-stack restoration itself remains unimplemented. Run its new regression before closure.

## Shared verification wording

Only cite steps actually present in CI. Current additions cover ADR path resolution, model-name ownership and a format-job summary, not all of render-bench/examples/python-directors/journal-integrity. Future leaf workflows must be authored and tested before an issue uses them as acceptance evidence. A source test, a static path check, a local passing test, and a hosted CI run are different evidence levels.
