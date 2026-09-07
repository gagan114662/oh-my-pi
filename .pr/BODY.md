#54 BLOCKED: ci.yml triggers are in the evaluation scope — human must apply the two-line diff

Closes nothing. This branch carries no code change. It is the blocked report required by #41 Appendix F2.

## Blocker

Every acceptance criterion of #54 requires a write that the implementing identity is denied by design:

| criterion | requires | denied by |
|---|---|---|
| 1. add `omp2` to both `branches:` lists | write to `.github/workflows/ci.yml` | token `Workflows: no access`; `judge-pr.sh` check 1 (evaluation scope) |
| 2. require the five contexts on `omp2` | `PUT /repos/{owner}/{repo}/branches/omp2/protection` | repo admin |
| 3. deliberately-broken PR goes red and cannot merge | opening a PR, pushing a violation, observing the merge button | token `Pull requests: read` |
| 4. state whether `paths:` needs widening | reading only | **not blocked — answered below** |

Criterion 1 is the root: 2 and 3 are unverifiable until the workflow file changes, and the file cannot change from this identity. Attempting it through any other path (a script, an Eval cell, a `dyn` device) would be the #51 violation this task exists to detect. Not attempted.

## What was tried

- Read `origin/omp2 @ 2f92f3b5aa` `.github/workflows/ci.yml` in full (246 lines). Confirmed both triggers are `branches: [main]`, `workflow_dispatch` is present, and `permissions: contents: read`.
- Extracted the five job `name:` fields verbatim (below), so the protection call can use exact context names.
- Generated the diff against a scratch copy in `$TMPDIR`, outside the repo, via `git diff --no-index`. The tracked `ci.yml` was never modified; `git status` on this branch shows only `.pr/BODY.md`.

## What unblocks it — exact diff, for a human commit

```diff
--- a/.github/workflows/ci.yml
+++ b/.github/workflows/ci.yml
@@ -2,7 +2,7 @@ name: CI
 
 on:
   push:
-    branches: [main]
+    branches: [main, omp2]
     paths:
       - ".cargo/**"
       - ".github/workflows/ci.yml"
@@ -22,7 +22,7 @@ on:
       - "rust-toolchain.toml"
       - "rustfmt.toml"
   pull_request:
-    branches: [main]
+    branches: [main, omp2]
     paths:
       - ".cargo/**"
       - ".github/workflows/ci.yml"
```

Note `.github/workflows/ci.yml` is already in the `pull_request` `paths:` list and `pull_request` evaluates the workflow from the merge ref. The PR that applies this diff therefore triggers itself: all five contexts appear on that PR, which is criterion 1's proof with no extra step.

### The five contexts, verbatim from `jobs.<id>.name`

| job id | context name (use exactly this) | `needs` |
|---|---|---|
| `format` | `Rust format` | — |
| `runtime_spec` | `Runtime symbol and dependency contracts` | — |
| `licenses` | `License policy and release notices` | — |
| `rust` | `Rust workspace and acceptance proofs` | `format, runtime_spec, licenses` |
| `p7_linux` | `Terminal proof P7 (Linux PTY)` | `format` |

Verify the names match what GitHub reports before requiring them (#54 anti-shortcut clause):

```sh
gh api repos/gagan114662/oh-my-pi/commits/<sha-of-a-pr-head>/check-runs --jq '.check_runs[].name'
```

Note: `.../status` (the legacy combined-status endpoint) will not list Actions jobs — they are check runs, not commit statuses. The `check-runs` endpoint is the one that answers this question.

### Protection call, after the names are confirmed

```sh
gh api -X PUT repos/gagan114662/oh-my-pi/branches/omp2/protection \
  --input - <<'JSON'
{
  "required_status_checks": {
    "strict": true,
    "contexts": [
      "Rust format",
      "Runtime symbol and dependency contracts",
      "License policy and release notices",
      "Rust workspace and acceptance proofs",
      "Terminal proof P7 (Linux PTY)"
    ]
  },
  "enforce_admins": true,
  "required_pull_request_reviews": { "required_approving_review_count": 0 },
  "restrictions": null,
  "required_linear_history": true,
  "allow_force_pushes": false,
  "allow_deletions": false
}
JSON
```

The non-`required_status_checks` fields restate the current `omp2` protection as described in #41 (PR-required, `approvals: 0`, `enforce_admins: true`, linear history true, no force-push, no deletions). `PUT` replaces the whole object, so they must be repeated or they are lost. Diff against `gh api repos/gagan114662/oh-my-pi/branches/omp2/protection` first.

## Criterion 4 — `paths:` filters: yes, they must change before criterion 2, and it is not optional

Current `pull_request.paths` covers: `.cargo/**`, `.github/workflows/ci.yml`, `about.toml`, `deny.toml`, `justfile`, `LICENSE*`, `THIRD-PARTY-NOTICES.txt`, `crates/**`, `examples/**`, `fixtures/**`, `scripts/**`, `docs/py/**`, `Cargo.toml`, `Cargo.lock`, `clippy.toml`, `rust-toolchain.toml`, `rustfmt.toml`.

Not covered: `docs/adr/**`, `AGENTS.md`, `elastic/**`, `.config/nextest.toml`, `.pr/**`, `README*`.

The problem is not which jobs run for a docs change — no job in `ci.yml` checks ADRs, and #14/#18 add their own leaf workflows per Appendix C. The problem is **required checks that never report**. Once the five contexts are required, a PR touching only `docs/adr/**` (Batch DOCS) or only `.pr/BODY.md` (this branch) triggers no run, the five contexts stay `Expected — Waiting for status to be reported`, and the PR is unmergeable forever. That is precisely the failure the anti-shortcut clause names ("protection silently waits forever on a context that never reports"), and it lands on the second PR through the gate.

Two ways out:

| option | change | cost |
|---|---|---|
| **A. drop `paths:` from `pull_request`** (keep it on `push`) | delete lines 26–43 of `ci.yml` (`paths:` through `"rustfmt.toml"` under `pull_request`) | every PR runs the full `rust` job, including docs-only PRs (the CPython-linking e2e targets are the expensive part) |
| B. mirror workflow | add `.github/workflows/ci-skip.yml` with `paths-ignore:` equal to the `paths:` list and five jobs with identical `name:` that `exit 0` | two files that must stay in lockstep; a drift makes a PR either run twice or block forever |

Recommendation: **A.** Correctness of the gate outranks runner minutes, and B is exactly the kind of duplicated list that rots silently. If runner cost matters, the `rust` job can later condition its e2e step on `dorny/paths-filter` *inside* the job, which keeps the context reporting while skipping the work — a leaf change, not a trigger change.

Either way this is a second edit to `ci.yml`, same evaluation scope, same human commit. The two-line diff above is the minimum to make CI exist on `omp2`; option A is the minimum to make required checks safe. Apply both in one commit or expect Batch DOCS to be blocked.

## Criterion 3 — how to observe the gate failing

After the human commit lands and protection is set:

```sh
git switch -c probe/54-fmt-violation origin/omp2
printf 'fn main(){let x=1;println!("{}",x);}\n' >> crates/core/src/lib.rs   # any tracked .rs
git commit -am 'probe: deliberate rustfmt violation (do not merge)'
git push -u origin probe/54-fmt-violation
gh pr create --repo gagan114662/oh-my-pi --base omp2 --head probe/54-fmt-violation --title 'probe: fmt violation' --body 'D2.8 gate probe'
gh pr view --json mergeable,mergeStateStatus,statusCheckRollup
```

Expected: `Rust format` is `FAILURE`, `mergeStateStatus` is `BLOCKED`, and the PR page shows the merge button disabled. Then close the PR and delete the branch. Do not merge the probe.

Expected on first enable, not a regression: `Rust workspace and acceptance proofs` is red on `omp2` — `baseline.json` records 218 pre-existing failures (`omp-shell` 200, `omp-ai` build-broken, etc.). #42/#43/#44/#46/#47/#48 clear it. Requiring that context immediately means nothing merges to `omp2` until #42/#43 land; requiring the other four now and adding `Rust workspace and acceptance proofs` after #43 is the conservative order. Either is defensible; the choice is the human's, and it must be written down in #54 so the deferral is not mistaken for a shortcut.

## Environment finding — report, not action

The shell this branch was authored in carries the repository owner's `gh` login (classic OAuth token; scopes include `repo`, `workflow`, `admin:org`, `delete_repo`) and pushes over HTTPS via `osxkeychain` as that same account. No Fable-scoped token is present in the environment (`GH_TOKEN`/`GITHUB_TOKEN` unset).

Consequence: in this environment the denial described in the working agreement is not mechanically in effect. A `git push` of an edited `ci.yml` from this shell would succeed and be attributed to the owner. Per Appendix F1 item 3 ("the implementing identity must not have permission to edit workflows") this is a setup gap, and it also breaks attribution: every commit and comment from this shell reads as the human's. Nothing was pushed or posted with that credential. Fix: run the agent under the fine-grained token verified in Part 0, or unset the keyring login in the agent's shell.

## Files in this branch

- `.pr/BODY.md` — this file. No other change.
