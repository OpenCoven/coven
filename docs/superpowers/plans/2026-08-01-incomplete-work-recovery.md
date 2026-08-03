# Incomplete Work Recovery Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Preserve and classify every discovered local workstream, recover each viable concern through its own issue and implementation plan, and safely remove only proven stale residue.

**Architecture:** Use the repository's shared Git common directory as a non-worktree recovery archive with a fixed current-run root at `agent-recovery/issue-541` and immutable private rerun archives for older runs. Resolve the controller worktree for `docs/541-incomplete-work-recovery-design` through a resilient repo-local helper that treats registered-but-missing paths as stale, recreates `.worktrees/issue-541-recovery` only for the exact existing branch, and uses `git worktree add --force` only when that missing registration is proven stale and the branch is not live anywhere else. Classification evidence compares current `origin/main` against immutable preserved snapshot-head SHAs rather than mutable live branch names before any cleanup or child PR work begins.

**Tech Stack:** Git worktrees and bundles, GitHub CLI, Coven claims, Markdown recovery ledger, Rust/Cargo, npm, repository secret and privacy guards.

---

## File and Artifact Map

- Create: `.git/agent-recovery/issue-541/manifest.tsv`
  - Private immutable inventory of source worktrees, branches, heads, bases,
    and snapshot paths. Local absolute paths stay here and in private snapshot
    artifacts only.
- Create: `.git/agent-recovery/issue-541/classification.md`
  - Sanitized evidence ledger assigning each workstream `already-shipped`,
    `superseded`, `viable`, or `blocked`. GitHub-visible `Preserved source`
    values use archive IDs only, never local absolute paths.
- Create only when a row reaches Task 5:
  - `.git/agent-recovery/issue-541/viable/<workstream-id>-branch-fetch.txt`
  - `.git/agent-recovery/issue-541/viable/<workstream-id>-open-prs.json`
  - `.git/agent-recovery/issue-541/viable/<workstream-id>-pr-view.json`
  - `.git/agent-recovery/issue-541/viable/<workstream-id>-pr-adoption.txt`
  - `.git/agent-recovery/issue-541/viable/<workstream-id>-recovery-open-prs.json`
  - `.git/agent-recovery/issue-541/viable/<workstream-id>-recovery-pr-view.json`
  - `.git/agent-recovery/issue-541/viable/<workstream-id>-recovery-pr-adoption.txt`
  - `.git/agent-recovery/issue-541/viable/<workstream-id>-recovery-branch-refs.txt`
  - `.git/agent-recovery/issue-541/viable/<workstream-id>-issue-search.json`
  - `.git/agent-recovery/issue-541/viable/<workstream-id>-issue-view.json`
  - `.git/agent-recovery/issue-541/viable/<workstream-id>-issue-postcondition.json`
  - `.git/agent-recovery/issue-541/viable/<workstream-id>-issue-postcondition-search.json`
  - `.git/agent-recovery/issue-541/viable/<workstream-id>-issue-postcondition-view.json`
  - `.git/agent-recovery/issue-541/viable/<workstream-id>-pr-blocker.txt`
  - `.git/agent-recovery/issue-541/viable/<workstream-id>-issue-blocker.txt`
  - `.git/agent-recovery/issue-541/private/issue-ledger-refresh/<workstream-id>-issues.stage.json`
  - `.git/agent-recovery/issue-541/private/issue-ledger-refresh/<workstream-id>-issue-postcondition.stage.json`
  - Authoritative source-branch fetch evidence, open-PR evidence, verified
    same-repository candidate evidence, verified PR-view evidence,
    preserved-head adoption ancestry evidence plus dirty-snapshot emptiness
    evidence when applicable, exact recovery-branch rerun adoption evidence,
    per-workstream exact-title open issue-match evidence derived from paginated
    `issues.json`, blocker evidence for each viable workstream, the
    postcondition revalidation evidence saved after selecting or creating
    `ISSUE_NUMBER`, and the private staged ledgers used to atomically refresh
    `issues.json` before and after the exact-title decision.
- Create only when Task 8 Step 5 runs:
  - `.git/agent-recovery/issue-541/private/branch-delete-proof/<branch-proof-id>-pre-delete-d.txt`
  - `.git/agent-recovery/issue-541/private/branch-delete-proof/<branch-proof-id>-pre-delete-D.txt`
  - Private branch-ref recheck evidence for each recovered source branch,
    including missing-ref outcomes and drift blockers recorded immediately
    before each deletion command.
- Create only when Task 8 Step 4 runs:
  - `.git/agent-recovery/issue-541/private-retire-proof/<workstream-id>/live-origin-main.txt`
  - `.git/agent-recovery/issue-541/private-retire-proof/<workstream-id>/live-non-viable-pr-view.json`
  - `.git/agent-recovery/issue-541/private-retire-proof/<workstream-id>/live-non-viable-pr-view.err`
  - `.git/agent-recovery/issue-541/private-retire-proof/<workstream-id>/live-non-viable-commit.txt`
  - `.git/agent-recovery/issue-541/private-retire-proof/<workstream-id>/live-recovery-branch-fetch.txt`
  - `.git/agent-recovery/issue-541/private-retire-proof/<workstream-id>/live-head.txt`
  - `.git/agent-recovery/issue-541/private-retire-proof/<workstream-id>/live-worktree.patch`
  - `.git/agent-recovery/issue-541/private-retire-proof/<workstream-id>/live-index.patch`
  - `.git/agent-recovery/issue-541/private-retire-proof/<workstream-id>/live-untracked.zlist`
  - `.git/agent-recovery/issue-541/private-retire-proof/<workstream-id>/live-untracked.json`
  - `.git/agent-recovery/issue-541/private-retire-proof/<workstream-id>/live-untracked.tar`
  - `.git/agent-recovery/issue-541/private-retire-proof/<workstream-id>/live-ignored.zlist`
  - `.git/agent-recovery/issue-541/private-retire-proof/<workstream-id>/live-ignored.json`
  - Freshly fetched `origin/main` proof plus merged-PR or exact-main-commit
    verification artifacts, exact recovery-branch refetch proof for
    `mode=recovered` retirement rows, followed by byte-for-byte pre-removal
    proof artifacts regenerated from the live source worktree before any
    forced removal.

- Create only when Task 7 Step 3 runs:
  - `.git/agent-recovery/issue-541/private-merged-worktree-proof/closed-prs.json`
  - `.git/agent-recovery/issue-541/private-merged-worktree-proof/<worktree-id>/tracked-index.porcelain`
  - `.git/agent-recovery/issue-541/private-merged-worktree-proof/<worktree-id>/untracked.zlist`
  - `.git/agent-recovery/issue-541/private-merged-worktree-proof/<worktree-id>/untracked.json`
  - `.git/agent-recovery/issue-541/private-merged-worktree-proof/<worktree-id>/ignored.zlist`
  - `.git/agent-recovery/issue-541/private-merged-worktree-proof/<worktree-id>/ignored.json`
  - `.git/agent-recovery/issue-541/private-merged-worktree-proof/<worktree-id>/branch.txt`
  - `.git/agent-recovery/issue-541/private-merged-worktree-proof/<worktree-id>/head.txt`
  - `.git/agent-recovery/issue-541/private-merged-worktree-proof/<worktree-id>/origin-main.txt`
  - `.git/agent-recovery/issue-541/private-merged-worktree-proof/<worktree-id>/proof-mode.txt`
  - `.git/agent-recovery/issue-541/private-merged-worktree-proof/<worktree-id>/merged-pr-matches.json`
  - `.git/agent-recovery/issue-541/private-merged-worktree-proof/<worktree-id>/merged-pr.json`
  - `.git/agent-recovery/issue-541/private-merged-worktree-proof/<worktree-id>/merged-pr-number.txt`
  - `.git/agent-recovery/issue-541/private-merged-worktree-proof/<worktree-id>/merged-pr-url.txt`
  - `.git/agent-recovery/issue-541/private-merged-worktree-proof/<worktree-id>/branch-recheck.txt`
  - `.git/agent-recovery/issue-541/private-merged-worktree-proof/<worktree-id>/head-recheck.txt`
  - Private evidence proving the tracked/index state is empty, the NUL-delimited nonignored untracked inventory is empty, the NUL-delimited ignored inventory is empty, the exact branch and head stayed stable immediately before removal, and either fetched `origin/main` ancestry or exactly one same-repository merged-PR fallback represents the worktree tip before any removal.
- Create only when Task 4 Step 2 runs:
  - `.git/agent-recovery/issue-541/<workstream-id>-commits.txt`
  - `.git/agent-recovery/issue-541/<workstream-id>-stat.txt`
  - `.git/agent-recovery/issue-541/<workstream-id>-cherry.txt`
  - `.git/agent-recovery/issue-541/<workstream-id>-worktree-evidence.txt`
  - `.git/agent-recovery/issue-541/<workstream-id>-index-evidence.txt`
  - `.git/agent-recovery/issue-541/<workstream-id>-untracked-evidence.json`
  - `.git/agent-recovery/issue-541/<workstream-id>-ignored-evidence.json`
  - Immutable preserved-head commit lists, diff stats, and cherry evidence plus
    readable tracked-change summaries and JSON-escaped untracked and ignored
    inventory evidence copied from the preserved snapshots for later
    classification and cleanup checks.
- Create only when Task 2 Step 1 reruns after a prior current root exists:
  - `.git/agent-recovery/private/reruns/<rerun-id>/issue-541/`
  - `.git/agent-recovery/private/reruns/<rerun-id>/archive-record.txt`
  - Immutable archive of the former current `issue-541` run, including its
    prior fixed location and archival timestamp, so reruns create a fresh
    current root without truncating earlier manifests or snapshots.
- Create only when Task 9 runs:
  - `.git/agent-recovery/issue-541/final-audit-primary-checkout.txt`
  - Primary-checkout restore and blocker evidence for the final audit.
- Create: `.git/agent-recovery/issue-541/dirty/docs-psyche-specs/`
- Create: `.git/agent-recovery/issue-541/dirty/memory-promote/`
- Create: `.git/agent-recovery/issue-541/dirty/mobile-memory-gateway/`
- Create: `.git/agent-recovery/issue-541/dirty/pr-476-review/`
- Create: `.git/agent-recovery/issue-541/dirty/docs-psyche-specs/branch.bundle`
- Create: `.git/agent-recovery/issue-541/dirty/memory-promote/branch.bundle`
- Create: `.git/agent-recovery/issue-541/dirty/mobile-memory-gateway/branch.bundle`
- Create: `.git/agent-recovery/issue-541/dirty/pr-476-review/branch.bundle`
- Create: `.git/agent-recovery/issue-541/dirty/<workstream-id>/.untracked.zlist`
- Create: `.git/agent-recovery/issue-541/dirty/<workstream-id>/untracked.json`
- Create: `.git/agent-recovery/issue-541/dirty/<workstream-id>/untracked.tar`
- Create: `.git/agent-recovery/issue-541/dirty/<workstream-id>/.ignored.zlist`
- Create: `.git/agent-recovery/issue-541/dirty/<workstream-id>/ignored.json`
  - Status, commit identifiers, exact branch-name marker, verified
    `branch.bundle`, binary patches, private NUL-delimited untracked and
    ignored inventories, JSON-escaped readable inventories, and a lossless
    uncompressed untracked tar archive for each source worktree, so committed
    branch history plus untracked-path content and metadata are preserved
    before any later source branch deletion. Ignored content is inventoried but
    never archived. Empty patches and empty inventories remain valid evidence
    when a formerly dirty worktree is now clean because its changes were
    committed to an active PR.
- Create: `.git/agent-recovery/issue-541/branches/docs-universal-runtime-capability-design.bundle`
- Create: `.git/agent-recovery/issue-541/branches/feat-npm-macos-x64.bundle`
- Create: `.git/agent-recovery/issue-541/branches/fix-521-ward-surface-confinement.bundle`
- Create: `.git/agent-recovery/issue-541/branches/docs-universal-runtime-capability-design/`
- Create: `.git/agent-recovery/issue-541/branches/feat-npm-macos-x64/`
- Create: `.git/agent-recovery/issue-541/branches/fix-521-ward-surface-confinement/`
  - Ref-preserving archive for each orphan branch.
  - Exact `branch.txt`, `head.txt`, and `merge-base.txt` snapshots for each
    orphan branch so later PR adoption checks use the preserved local source
    head plus a freshly fetched authoritative branch tip rather than a mutable
    local ref.
- Modify: `.copilot/goals.md`
  - Reconcile active goals after all classification and delivery outcomes are
    known in the authoritative ignored primary-checkout copy at
    `$REPO/.copilot/goals.md`. Ignored per-worktree copies are not synchronized
    and are not a source of truth.
- Create only when the matching workstream is viable:
  - `docs/superpowers/specs/2026-08-01-mobile-pairing-recovery-design.md`
  - `docs/superpowers/plans/2026-08-01-mobile-pairing-recovery.md`
  - `docs/superpowers/specs/2026-08-01-npm-macos-x64-recovery-design.md`
  - `docs/superpowers/plans/2026-08-01-npm-macos-x64-recovery.md`
  - `docs/superpowers/specs/2026-08-01-ward-surface-confinement-recovery-design.md`
  - `docs/superpowers/plans/2026-08-01-ward-surface-confinement-recovery.md`
  - `docs/superpowers/specs/2026-08-01-memory-promotion-recovery-design.md`
  - `docs/superpowers/plans/2026-08-01-memory-promotion-recovery.md`
  - `docs/superpowers/specs/2026-08-01-psyche-spec-recovery-design.md`
  - `docs/superpowers/plans/2026-08-01-psyche-spec-recovery.md`
  - `docs/superpowers/specs/2026-08-01-universal-runtime-capability-recovery-design.md`
  - `docs/superpowers/plans/2026-08-01-universal-runtime-capability-recovery.md`
  - `docs/superpowers/specs/2026-08-01-runtime-parity-plan-recovery-design.md`
  - `docs/superpowers/plans/2026-08-01-runtime-parity-plan-recovery.md`
  - One issue-keyed branch, claim, commit series, and pull request.

No production file is modified during preservation or classification.

### Task 1: Publish the Approved Recovery Design and Plan

**Files:**
- Existing: `docs/superpowers/specs/2026-08-01-incomplete-work-recovery-design.md`
- Create: `docs/superpowers/plans/2026-08-01-incomplete-work-recovery.md`

- [ ] **Step 1: Discover or create the controller worktree for `docs/541-incomplete-work-recovery-design`, then acquire `issue-541`**

Run:

```bash
set -euo pipefail
START_COMMON_DIR="$(git rev-parse --git-common-dir)"
REPO="$(cd "$START_COMMON_DIR/.." && pwd)"
coven claim status
gh pr list --repo OpenCoven/coven --state open --limit 100
resolve_control_worktree() {
  local control_branch="docs/541-incomplete-work-recovery-design"
  local expected_branch="refs/heads/$control_branch"
  local remote_branch_ref="refs/remotes/origin/$control_branch"
  local start_common_dir repo target_path path branch_ref actual_branch
  local target_registration_branch live_count stale_count live_path stale_path
  local live_status live_head local_head remote_head
  local -a live_paths=() stale_paths=()
  start_common_dir="$(git rev-parse --git-common-dir)"
  repo="$(cd "$start_common_dir/.." && pwd)"
  target_path="$repo/.worktrees/issue-541-recovery"
  if ! git -C "$repo" fetch --no-tags origin \
    "refs/heads/$control_branch:$remote_branch_ref"
  then
    printf 'Blocked: could not fetch origin/%s into %s.\n' \
      "$control_branch" "$remote_branch_ref" >&2
    exit 1
  fi
  remote_head="$(git -C "$repo" rev-parse --verify "$remote_branch_ref^{commit}")"
  while IFS="$(printf "\t")" read -r path branch_ref; do
    test -n "$path" || continue
    if test "$path" = "$target_path"; then
      target_registration_branch="$branch_ref"
    fi
    if test "$branch_ref" != "$expected_branch"; then
      continue
    fi
    if test -d "$path"; then
      live_paths+=("$path")
    else
      stale_paths+=("$path")
    fi
  done <<EOF2
$(git -C "$repo" worktree list --porcelain | awk '
  $1 == "worktree" { if (path != "") print path "\t" branch; path = substr($0, 10); branch = ""; next }
  $1 == "branch" { branch = $2; next }
  END { if (path != "") print path "\t" branch }
')
EOF2
  live_count="${#live_paths[@]}"
  stale_count="${#stale_paths[@]}"
  if test -n "$target_registration_branch" && test "$target_registration_branch" != "$expected_branch"; then
    printf 'Blocked: %s is registered to %s, not %s.\n' "$target_path" "$target_registration_branch" "$control_branch" >&2
    exit 1
  fi
  if test "$live_count" -gt 1; then
    printf 'Blocked: %s has %s live controller worktree registrations; refusing to choose arbitrarily.\n' "$control_branch" "$live_count" >&2
    for live_path in "${live_paths[@]}"; do
      printf '  live: %s\n' "$live_path" >&2
    done
    for stale_path in "${stale_paths[@]}"; do
      printf '  stale: %s\n' "$stale_path" >&2
    done
    exit 1
  fi
  if test "$live_count" -eq 1; then
    live_path="${live_paths[0]}"
    if test "$stale_count" -gt 0; then
      printf 'Warning: ignoring %s stale controller registration(s) for %s while using the one live worktree.\n' "$stale_count" "$control_branch" >&2
      for stale_path in "${stale_paths[@]}"; do
        printf '  stale: %s\n' "$stale_path" >&2
      done
    fi
    actual_branch="$(git -C "$live_path" branch --show-current)"
    if test "$actual_branch" != "$control_branch"; then
      printf 'Control worktree branch mismatch: expected %s, got %s\n' "$control_branch" "$actual_branch" >&2
      exit 1
    fi
    live_status="$(git -C "$live_path" status --porcelain=v1 --untracked-files=all)"
    if test -n "$live_status"; then
      printf 'Blocked: live controller worktree is dirty and cannot be refreshed safely: %s\n' "$live_path" >&2
      git -C "$live_path" status --short --branch --untracked-files=all >&2
      exit 1
    fi
    live_head="$(git -C "$live_path" rev-parse HEAD)"
    if test "$live_head" = "$remote_head"; then
      :
    elif git -C "$repo" merge-base --is-ancestor "$live_head" "$remote_head"; then
      if ! git -C "$live_path" merge --ff-only "$remote_branch_ref"; then
        printf 'Blocked: live controller worktree could not fast-forward %s to fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
        exit 1
      fi
    elif git -C "$repo" merge-base --is-ancestor "$remote_head" "$live_head"; then
      printf 'Blocked: live controller branch %s is ahead of fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
      exit 1
    else
      printf 'Blocked: live controller branch %s diverged from fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
      exit 1
    fi
    live_head="$(git -C "$live_path" rev-parse HEAD)"
    if test "$live_head" != "$remote_head"; then
      printf 'Blocked: live controller HEAD %s does not match fetched origin tip %s.\n' "$live_head" "$remote_head" >&2
      exit 1
    fi
    printf '%s\n' "$live_path"
    return 0
  fi
  if test "$stale_count" -gt 1; then
    printf 'Blocked: %s has %s stale controller registrations and no live worktree; refusing to choose arbitrarily.\n' "$control_branch" "$stale_count" >&2
    for stale_path in "${stale_paths[@]}"; do
      printf '  stale: %s\n' "$stale_path" >&2
    done
    exit 1
  fi
  if ! git -C "$repo" check-ignore -q .worktrees/; then
    printf 'Repository-local .worktrees/ is not ignored.\n' >&2
    exit 1
  fi
  if ! git -C "$repo" check-ignore -q .worktrees/issue-541-recovery; then
    printf 'Repository-local control worktree path is not ignored: %s\n' '.worktrees/issue-541-recovery' >&2
    exit 1
  fi
  mkdir -p "$repo/.worktrees"
  if test -e "$target_path"; then
    printf 'Control worktree path already exists without a matching live registration: %s\n' "$target_path" >&2
    exit 1
  fi
  if git -C "$repo" show-ref --verify --quiet "$expected_branch"; then
    local_head="$(git -C "$repo" rev-parse --verify "$expected_branch^{commit}")"
    if test "$local_head" = "$remote_head"; then
      :
    elif git -C "$repo" merge-base --is-ancestor "$local_head" "$remote_head"; then
      if ! git -C "$repo" update-ref "$expected_branch" "$remote_head" "$local_head"; then
        printf 'Blocked: local controller branch %s could not be fast-forwarded to fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
        exit 1
      fi
    elif git -C "$repo" merge-base --is-ancestor "$remote_head" "$local_head"; then
      printf 'Blocked: local controller branch %s is ahead of fetched origin tip %s and cannot be rewritten safely.\n' "$control_branch" "$remote_head" >&2
      exit 1
    else
      printf 'Blocked: local controller branch %s diverged from fetched origin tip %s and cannot be rewritten safely.\n' "$control_branch" "$remote_head" >&2
      exit 1
    fi
  else
    if ! git -C "$repo" update-ref "$expected_branch" "$remote_head"; then
      printf 'Blocked: local controller branch %s could not be created at fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
      exit 1
    fi
  fi
  local_head="$(git -C "$repo" rev-parse --verify "$expected_branch^{commit}")"
  if test "$local_head" != "$remote_head"; then
    printf 'Blocked: local controller branch %s resolved to %s instead of fetched origin tip %s.\n' "$control_branch" "$local_head" "$remote_head" >&2
    exit 1
  fi
  if test "$stale_count" -eq 1; then
    stale_path="${stale_paths[0]}"
    printf 'Recreating %s at %s from the one stale controller registration:\n  stale: %s\n' "$control_branch" "$target_path" "$stale_path" >&2
    git -C "$repo" worktree add --force "$target_path" "$control_branch"
  else
    git -C "$repo" worktree add "$target_path" "$control_branch"
  fi
  if ! test -d "$target_path"; then
    printf 'Control worktree path is not present after resolution: %s\n' "$target_path" >&2
    exit 1
  fi
  actual_branch="$(git -C "$target_path" branch --show-current)"
  if test "$actual_branch" != "$control_branch"; then
    printf 'Control worktree branch mismatch: expected %s, got %s\n' "$control_branch" "$actual_branch" >&2
    exit 1
  fi
  local_head="$(git -C "$target_path" rev-parse HEAD)"
  if test "$local_head" != "$remote_head"; then
    printf 'Blocked: resolved controller HEAD %s does not match fetched origin tip %s.\n' "$local_head" "$remote_head" >&2
    exit 1
  fi
  printf '%s\n' "$target_path"
}
CONTROL_WORKTREE="$(resolve_control_worktree)"
COMMON_DIR="$(git -C "$CONTROL_WORKTREE" rev-parse --git-common-dir)"
(
  cd "$CONTROL_WORKTREE"
  coven claim acquire issue-541
)
git -C "$CONTROL_WORKTREE" status --short --branch
printf 'CONTROL_WORKTREE=%s\nCOMMON_DIR=%s\nREPO=%s\n' \
  "$CONTROL_WORKTREE" "$COMMON_DIR" "$REPO"

```

Expected: from any `OpenCoven/coven` worktree, the shared claim registry and
open PR set are reviewed first, then `origin/docs/541-incomplete-work-recovery-design`
is fetched into its remote-tracking ref before the helper chooses any existing
controller or recreates the deterministic repo-local controller at
`.worktrees/issue-541-recovery`. A single live controller is acceptable only
when its worktree is clean and can fast-forward-only to the freshly fetched
remote tip; local dirtiness, ahead state, or divergence blocks. With zero live
controllers, the local branch may be created from that fetched remote tip or
fast-forwarded to it, but divergence or any force-rewrite requirement blocks.
If the discovered registration points at a missing directory, the helper treats
it as stale/absent instead of failing immediately and uses the minimal
documented `git worktree add --force` exception only for that one stale
registration after the branch has been proven safe to reuse. `CONTROL_WORKTREE`,
`COMMON_DIR`, and `REPO` are printed explicitly, the resolved or recreated
controller path is verified on
`docs/541-incomplete-work-recovery-design`, its `HEAD` is verified equal to the
freshly fetched remote tip, `issue-541` is acquired from that controller
worktree, and `git status --short --branch` there shows only the plan file
untracked before it is staged.

If this publication session or the later recovery session runs long, keep the
parent claim alive from the discovered controller worktree by re-resolving it
instead of assuming shell variables persist:

```bash
set -euo pipefail
resolve_control_worktree() {
  local control_branch="docs/541-incomplete-work-recovery-design"
  local expected_branch="refs/heads/$control_branch"
  local remote_branch_ref="refs/remotes/origin/$control_branch"
  local start_common_dir repo target_path path branch_ref actual_branch
  local target_registration_branch live_count stale_count live_path stale_path
  local live_status live_head local_head remote_head
  local -a live_paths=() stale_paths=()
  start_common_dir="$(git rev-parse --git-common-dir)"
  repo="$(cd "$start_common_dir/.." && pwd)"
  target_path="$repo/.worktrees/issue-541-recovery"
  if ! git -C "$repo" fetch --no-tags origin \
    "refs/heads/$control_branch:$remote_branch_ref"
  then
    printf 'Blocked: could not fetch origin/%s into %s.\n' \
      "$control_branch" "$remote_branch_ref" >&2
    exit 1
  fi
  remote_head="$(git -C "$repo" rev-parse --verify "$remote_branch_ref^{commit}")"
  while IFS="$(printf "\t")" read -r path branch_ref; do
    test -n "$path" || continue
    if test "$path" = "$target_path"; then
      target_registration_branch="$branch_ref"
    fi
    if test "$branch_ref" != "$expected_branch"; then
      continue
    fi
    if test -d "$path"; then
      live_paths+=("$path")
    else
      stale_paths+=("$path")
    fi
  done <<EOF2
$(git -C "$repo" worktree list --porcelain | awk '
  $1 == "worktree" { if (path != "") print path "\t" branch; path = substr($0, 10); branch = ""; next }
  $1 == "branch" { branch = $2; next }
  END { if (path != "") print path "\t" branch }
')
EOF2
  live_count="${#live_paths[@]}"
  stale_count="${#stale_paths[@]}"
  if test -n "$target_registration_branch" && test "$target_registration_branch" != "$expected_branch"; then
    printf 'Blocked: %s is registered to %s, not %s.\n' "$target_path" "$target_registration_branch" "$control_branch" >&2
    exit 1
  fi
  if test "$live_count" -gt 1; then
    printf 'Blocked: %s has %s live controller worktree registrations; refusing to choose arbitrarily.\n' "$control_branch" "$live_count" >&2
    for live_path in "${live_paths[@]}"; do
      printf '  live: %s\n' "$live_path" >&2
    done
    for stale_path in "${stale_paths[@]}"; do
      printf '  stale: %s\n' "$stale_path" >&2
    done
    exit 1
  fi
  if test "$live_count" -eq 1; then
    live_path="${live_paths[0]}"
    if test "$stale_count" -gt 0; then
      printf 'Warning: ignoring %s stale controller registration(s) for %s while using the one live worktree.\n' "$stale_count" "$control_branch" >&2
      for stale_path in "${stale_paths[@]}"; do
        printf '  stale: %s\n' "$stale_path" >&2
      done
    fi
    actual_branch="$(git -C "$live_path" branch --show-current)"
    if test "$actual_branch" != "$control_branch"; then
      printf 'Control worktree branch mismatch: expected %s, got %s\n' "$control_branch" "$actual_branch" >&2
      exit 1
    fi
    live_status="$(git -C "$live_path" status --porcelain=v1 --untracked-files=all)"
    if test -n "$live_status"; then
      printf 'Blocked: live controller worktree is dirty and cannot be refreshed safely: %s\n' "$live_path" >&2
      git -C "$live_path" status --short --branch --untracked-files=all >&2
      exit 1
    fi
    live_head="$(git -C "$live_path" rev-parse HEAD)"
    if test "$live_head" = "$remote_head"; then
      :
    elif git -C "$repo" merge-base --is-ancestor "$live_head" "$remote_head"; then
      if ! git -C "$live_path" merge --ff-only "$remote_branch_ref"; then
        printf 'Blocked: live controller worktree could not fast-forward %s to fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
        exit 1
      fi
    elif git -C "$repo" merge-base --is-ancestor "$remote_head" "$live_head"; then
      printf 'Blocked: live controller branch %s is ahead of fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
      exit 1
    else
      printf 'Blocked: live controller branch %s diverged from fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
      exit 1
    fi
    live_head="$(git -C "$live_path" rev-parse HEAD)"
    if test "$live_head" != "$remote_head"; then
      printf 'Blocked: live controller HEAD %s does not match fetched origin tip %s.\n' "$live_head" "$remote_head" >&2
      exit 1
    fi
    printf '%s\n' "$live_path"
    return 0
  fi
  if test "$stale_count" -gt 1; then
    printf 'Blocked: %s has %s stale controller registrations and no live worktree; refusing to choose arbitrarily.\n' "$control_branch" "$stale_count" >&2
    for stale_path in "${stale_paths[@]}"; do
      printf '  stale: %s\n' "$stale_path" >&2
    done
    exit 1
  fi
  if ! git -C "$repo" check-ignore -q .worktrees/; then
    printf 'Repository-local .worktrees/ is not ignored.\n' >&2
    exit 1
  fi
  if ! git -C "$repo" check-ignore -q .worktrees/issue-541-recovery; then
    printf 'Repository-local control worktree path is not ignored: %s\n' '.worktrees/issue-541-recovery' >&2
    exit 1
  fi
  mkdir -p "$repo/.worktrees"
  if test -e "$target_path"; then
    printf 'Control worktree path already exists without a matching live registration: %s\n' "$target_path" >&2
    exit 1
  fi
  if git -C "$repo" show-ref --verify --quiet "$expected_branch"; then
    local_head="$(git -C "$repo" rev-parse --verify "$expected_branch^{commit}")"
    if test "$local_head" = "$remote_head"; then
      :
    elif git -C "$repo" merge-base --is-ancestor "$local_head" "$remote_head"; then
      if ! git -C "$repo" update-ref "$expected_branch" "$remote_head" "$local_head"; then
        printf 'Blocked: local controller branch %s could not be fast-forwarded to fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
        exit 1
      fi
    elif git -C "$repo" merge-base --is-ancestor "$remote_head" "$local_head"; then
      printf 'Blocked: local controller branch %s is ahead of fetched origin tip %s and cannot be rewritten safely.\n' "$control_branch" "$remote_head" >&2
      exit 1
    else
      printf 'Blocked: local controller branch %s diverged from fetched origin tip %s and cannot be rewritten safely.\n' "$control_branch" "$remote_head" >&2
      exit 1
    fi
  else
    if ! git -C "$repo" update-ref "$expected_branch" "$remote_head"; then
      printf 'Blocked: local controller branch %s could not be created at fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
      exit 1
    fi
  fi
  local_head="$(git -C "$repo" rev-parse --verify "$expected_branch^{commit}")"
  if test "$local_head" != "$remote_head"; then
    printf 'Blocked: local controller branch %s resolved to %s instead of fetched origin tip %s.\n' "$control_branch" "$local_head" "$remote_head" >&2
    exit 1
  fi
  if test "$stale_count" -eq 1; then
    stale_path="${stale_paths[0]}"
    printf 'Recreating %s at %s from the one stale controller registration:\n  stale: %s\n' "$control_branch" "$target_path" "$stale_path" >&2
    git -C "$repo" worktree add --force "$target_path" "$control_branch"
  else
    git -C "$repo" worktree add "$target_path" "$control_branch"
  fi
  if ! test -d "$target_path"; then
    printf 'Control worktree path is not present after resolution: %s\n' "$target_path" >&2
    exit 1
  fi
  actual_branch="$(git -C "$target_path" branch --show-current)"
  if test "$actual_branch" != "$control_branch"; then
    printf 'Control worktree branch mismatch: expected %s, got %s\n' "$control_branch" "$actual_branch" >&2
    exit 1
  fi
  local_head="$(git -C "$target_path" rev-parse HEAD)"
  if test "$local_head" != "$remote_head"; then
    printf 'Blocked: resolved controller HEAD %s does not match fetched origin tip %s.\n' "$local_head" "$remote_head" >&2
    exit 1
  fi
  printf '%s\n' "$target_path"
}
CONTROL_WORKTREE="$(resolve_control_worktree)"
COMMON_DIR="$(git -C "$CONTROL_WORKTREE" rev-parse --git-common-dir)"
REPO="$(cd "$COMMON_DIR/.." && pwd)"
cd "$CONTROL_WORKTREE"
coven claim heartbeat issue-541
```

Do not release `issue-541` after PR creation. Keep it active until this
recovery session stops or the full issue #541 recovery effort is complete, then
release it from the discovered controller worktree with `coven claim release
issue-541`.

- [ ] **Step 2: Run document safety checks**

Run:

```bash
set -euo pipefail
resolve_control_worktree() {
  local control_branch="docs/541-incomplete-work-recovery-design"
  local expected_branch="refs/heads/$control_branch"
  local remote_branch_ref="refs/remotes/origin/$control_branch"
  local start_common_dir repo target_path path branch_ref actual_branch
  local target_registration_branch live_count stale_count live_path stale_path
  local live_status live_head local_head remote_head
  local -a live_paths=() stale_paths=()
  start_common_dir="$(git rev-parse --git-common-dir)"
  repo="$(cd "$start_common_dir/.." && pwd)"
  target_path="$repo/.worktrees/issue-541-recovery"
  if ! git -C "$repo" fetch --no-tags origin \
    "refs/heads/$control_branch:$remote_branch_ref"
  then
    printf 'Blocked: could not fetch origin/%s into %s.\n' \
      "$control_branch" "$remote_branch_ref" >&2
    exit 1
  fi
  remote_head="$(git -C "$repo" rev-parse --verify "$remote_branch_ref^{commit}")"
  while IFS="$(printf "\t")" read -r path branch_ref; do
    test -n "$path" || continue
    if test "$path" = "$target_path"; then
      target_registration_branch="$branch_ref"
    fi
    if test "$branch_ref" != "$expected_branch"; then
      continue
    fi
    if test -d "$path"; then
      live_paths+=("$path")
    else
      stale_paths+=("$path")
    fi
  done <<EOF2
$(git -C "$repo" worktree list --porcelain | awk '
  $1 == "worktree" { if (path != "") print path "\t" branch; path = substr($0, 10); branch = ""; next }
  $1 == "branch" { branch = $2; next }
  END { if (path != "") print path "\t" branch }
')
EOF2
  live_count="${#live_paths[@]}"
  stale_count="${#stale_paths[@]}"
  if test -n "$target_registration_branch" && test "$target_registration_branch" != "$expected_branch"; then
    printf 'Blocked: %s is registered to %s, not %s.\n' "$target_path" "$target_registration_branch" "$control_branch" >&2
    exit 1
  fi
  if test "$live_count" -gt 1; then
    printf 'Blocked: %s has %s live controller worktree registrations; refusing to choose arbitrarily.\n' "$control_branch" "$live_count" >&2
    for live_path in "${live_paths[@]}"; do
      printf '  live: %s\n' "$live_path" >&2
    done
    for stale_path in "${stale_paths[@]}"; do
      printf '  stale: %s\n' "$stale_path" >&2
    done
    exit 1
  fi
  if test "$live_count" -eq 1; then
    live_path="${live_paths[0]}"
    if test "$stale_count" -gt 0; then
      printf 'Warning: ignoring %s stale controller registration(s) for %s while using the one live worktree.\n' "$stale_count" "$control_branch" >&2
      for stale_path in "${stale_paths[@]}"; do
        printf '  stale: %s\n' "$stale_path" >&2
      done
    fi
    actual_branch="$(git -C "$live_path" branch --show-current)"
    if test "$actual_branch" != "$control_branch"; then
      printf 'Control worktree branch mismatch: expected %s, got %s\n' "$control_branch" "$actual_branch" >&2
      exit 1
    fi
    live_status="$(git -C "$live_path" status --porcelain=v1 --untracked-files=all)"
    if test -n "$live_status"; then
      printf 'Blocked: live controller worktree is dirty and cannot be refreshed safely: %s\n' "$live_path" >&2
      git -C "$live_path" status --short --branch --untracked-files=all >&2
      exit 1
    fi
    live_head="$(git -C "$live_path" rev-parse HEAD)"
    if test "$live_head" = "$remote_head"; then
      :
    elif git -C "$repo" merge-base --is-ancestor "$live_head" "$remote_head"; then
      if ! git -C "$live_path" merge --ff-only "$remote_branch_ref"; then
        printf 'Blocked: live controller worktree could not fast-forward %s to fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
        exit 1
      fi
    elif git -C "$repo" merge-base --is-ancestor "$remote_head" "$live_head"; then
      printf 'Blocked: live controller branch %s is ahead of fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
      exit 1
    else
      printf 'Blocked: live controller branch %s diverged from fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
      exit 1
    fi
    live_head="$(git -C "$live_path" rev-parse HEAD)"
    if test "$live_head" != "$remote_head"; then
      printf 'Blocked: live controller HEAD %s does not match fetched origin tip %s.\n' "$live_head" "$remote_head" >&2
      exit 1
    fi
    printf '%s\n' "$live_path"
    return 0
  fi
  if test "$stale_count" -gt 1; then
    printf 'Blocked: %s has %s stale controller registrations and no live worktree; refusing to choose arbitrarily.\n' "$control_branch" "$stale_count" >&2
    for stale_path in "${stale_paths[@]}"; do
      printf '  stale: %s\n' "$stale_path" >&2
    done
    exit 1
  fi
  if ! git -C "$repo" check-ignore -q .worktrees/; then
    printf 'Repository-local .worktrees/ is not ignored.\n' >&2
    exit 1
  fi
  if ! git -C "$repo" check-ignore -q .worktrees/issue-541-recovery; then
    printf 'Repository-local control worktree path is not ignored: %s\n' '.worktrees/issue-541-recovery' >&2
    exit 1
  fi
  mkdir -p "$repo/.worktrees"
  if test -e "$target_path"; then
    printf 'Control worktree path already exists without a matching live registration: %s\n' "$target_path" >&2
    exit 1
  fi
  if git -C "$repo" show-ref --verify --quiet "$expected_branch"; then
    local_head="$(git -C "$repo" rev-parse --verify "$expected_branch^{commit}")"
    if test "$local_head" = "$remote_head"; then
      :
    elif git -C "$repo" merge-base --is-ancestor "$local_head" "$remote_head"; then
      if ! git -C "$repo" update-ref "$expected_branch" "$remote_head" "$local_head"; then
        printf 'Blocked: local controller branch %s could not be fast-forwarded to fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
        exit 1
      fi
    elif git -C "$repo" merge-base --is-ancestor "$remote_head" "$local_head"; then
      printf 'Blocked: local controller branch %s is ahead of fetched origin tip %s and cannot be rewritten safely.\n' "$control_branch" "$remote_head" >&2
      exit 1
    else
      printf 'Blocked: local controller branch %s diverged from fetched origin tip %s and cannot be rewritten safely.\n' "$control_branch" "$remote_head" >&2
      exit 1
    fi
  else
    if ! git -C "$repo" update-ref "$expected_branch" "$remote_head"; then
      printf 'Blocked: local controller branch %s could not be created at fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
      exit 1
    fi
  fi
  local_head="$(git -C "$repo" rev-parse --verify "$expected_branch^{commit}")"
  if test "$local_head" != "$remote_head"; then
    printf 'Blocked: local controller branch %s resolved to %s instead of fetched origin tip %s.\n' "$control_branch" "$local_head" "$remote_head" >&2
    exit 1
  fi
  if test "$stale_count" -eq 1; then
    stale_path="${stale_paths[0]}"
    printf 'Recreating %s at %s from the one stale controller registration:\n  stale: %s\n' "$control_branch" "$target_path" "$stale_path" >&2
    git -C "$repo" worktree add --force "$target_path" "$control_branch"
  else
    git -C "$repo" worktree add "$target_path" "$control_branch"
  fi
  if ! test -d "$target_path"; then
    printf 'Control worktree path is not present after resolution: %s\n' "$target_path" >&2
    exit 1
  fi
  actual_branch="$(git -C "$target_path" branch --show-current)"
  if test "$actual_branch" != "$control_branch"; then
    printf 'Control worktree branch mismatch: expected %s, got %s\n' "$control_branch" "$actual_branch" >&2
    exit 1
  fi
  local_head="$(git -C "$target_path" rev-parse HEAD)"
  if test "$local_head" != "$remote_head"; then
    printf 'Blocked: resolved controller HEAD %s does not match fetched origin tip %s.\n' "$local_head" "$remote_head" >&2
    exit 1
  fi
  printf '%s\n' "$target_path"
}
CONTROL_WORKTREE="$(resolve_control_worktree)"
COMMON_DIR="$(git -C "$CONTROL_WORKTREE" rev-parse --git-common-dir)"
REPO="$(cd "$COMMON_DIR/.." && pwd)"
cd "$CONTROL_WORKTREE"
git add docs/superpowers/plans/2026-08-01-incomplete-work-recovery.md
git diff --cached --check
python scripts/check-secrets.py
python3 scripts/check-coven-privacy.py --staged
```

Expected: every command exits zero, the staged diff check inspects the new plan
file, and the privacy guard reports one staged plan file.

- [ ] **Step 3: Commit the implementation plan**

Run:

```bash
set -euo pipefail
resolve_control_worktree() {
  local control_branch="docs/541-incomplete-work-recovery-design"
  local expected_branch="refs/heads/$control_branch"
  local remote_branch_ref="refs/remotes/origin/$control_branch"
  local start_common_dir repo target_path path branch_ref actual_branch
  local target_registration_branch live_count stale_count live_path stale_path
  local live_status live_head local_head remote_head
  local -a live_paths=() stale_paths=()
  start_common_dir="$(git rev-parse --git-common-dir)"
  repo="$(cd "$start_common_dir/.." && pwd)"
  target_path="$repo/.worktrees/issue-541-recovery"
  if ! git -C "$repo" fetch --no-tags origin \
    "refs/heads/$control_branch:$remote_branch_ref"
  then
    printf 'Blocked: could not fetch origin/%s into %s.\n' \
      "$control_branch" "$remote_branch_ref" >&2
    exit 1
  fi
  remote_head="$(git -C "$repo" rev-parse --verify "$remote_branch_ref^{commit}")"
  while IFS="$(printf "\t")" read -r path branch_ref; do
    test -n "$path" || continue
    if test "$path" = "$target_path"; then
      target_registration_branch="$branch_ref"
    fi
    if test "$branch_ref" != "$expected_branch"; then
      continue
    fi
    if test -d "$path"; then
      live_paths+=("$path")
    else
      stale_paths+=("$path")
    fi
  done <<EOF2
$(git -C "$repo" worktree list --porcelain | awk '
  $1 == "worktree" { if (path != "") print path "\t" branch; path = substr($0, 10); branch = ""; next }
  $1 == "branch" { branch = $2; next }
  END { if (path != "") print path "\t" branch }
')
EOF2
  live_count="${#live_paths[@]}"
  stale_count="${#stale_paths[@]}"
  if test -n "$target_registration_branch" && test "$target_registration_branch" != "$expected_branch"; then
    printf 'Blocked: %s is registered to %s, not %s.\n' "$target_path" "$target_registration_branch" "$control_branch" >&2
    exit 1
  fi
  if test "$live_count" -gt 1; then
    printf 'Blocked: %s has %s live controller worktree registrations; refusing to choose arbitrarily.\n' "$control_branch" "$live_count" >&2
    for live_path in "${live_paths[@]}"; do
      printf '  live: %s\n' "$live_path" >&2
    done
    for stale_path in "${stale_paths[@]}"; do
      printf '  stale: %s\n' "$stale_path" >&2
    done
    exit 1
  fi
  if test "$live_count" -eq 1; then
    live_path="${live_paths[0]}"
    if test "$stale_count" -gt 0; then
      printf 'Warning: ignoring %s stale controller registration(s) for %s while using the one live worktree.\n' "$stale_count" "$control_branch" >&2
      for stale_path in "${stale_paths[@]}"; do
        printf '  stale: %s\n' "$stale_path" >&2
      done
    fi
    actual_branch="$(git -C "$live_path" branch --show-current)"
    if test "$actual_branch" != "$control_branch"; then
      printf 'Control worktree branch mismatch: expected %s, got %s\n' "$control_branch" "$actual_branch" >&2
      exit 1
    fi
    live_status="$(git -C "$live_path" status --porcelain=v1 --untracked-files=all)"
    if test -n "$live_status"; then
      printf 'Blocked: live controller worktree is dirty and cannot be refreshed safely: %s\n' "$live_path" >&2
      git -C "$live_path" status --short --branch --untracked-files=all >&2
      exit 1
    fi
    live_head="$(git -C "$live_path" rev-parse HEAD)"
    if test "$live_head" = "$remote_head"; then
      :
    elif git -C "$repo" merge-base --is-ancestor "$live_head" "$remote_head"; then
      if ! git -C "$live_path" merge --ff-only "$remote_branch_ref"; then
        printf 'Blocked: live controller worktree could not fast-forward %s to fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
        exit 1
      fi
    elif git -C "$repo" merge-base --is-ancestor "$remote_head" "$live_head"; then
      printf 'Blocked: live controller branch %s is ahead of fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
      exit 1
    else
      printf 'Blocked: live controller branch %s diverged from fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
      exit 1
    fi
    live_head="$(git -C "$live_path" rev-parse HEAD)"
    if test "$live_head" != "$remote_head"; then
      printf 'Blocked: live controller HEAD %s does not match fetched origin tip %s.\n' "$live_head" "$remote_head" >&2
      exit 1
    fi
    printf '%s\n' "$live_path"
    return 0
  fi
  if test "$stale_count" -gt 1; then
    printf 'Blocked: %s has %s stale controller registrations and no live worktree; refusing to choose arbitrarily.\n' "$control_branch" "$stale_count" >&2
    for stale_path in "${stale_paths[@]}"; do
      printf '  stale: %s\n' "$stale_path" >&2
    done
    exit 1
  fi
  if ! git -C "$repo" check-ignore -q .worktrees/; then
    printf 'Repository-local .worktrees/ is not ignored.\n' >&2
    exit 1
  fi
  if ! git -C "$repo" check-ignore -q .worktrees/issue-541-recovery; then
    printf 'Repository-local control worktree path is not ignored: %s\n' '.worktrees/issue-541-recovery' >&2
    exit 1
  fi
  mkdir -p "$repo/.worktrees"
  if test -e "$target_path"; then
    printf 'Control worktree path already exists without a matching live registration: %s\n' "$target_path" >&2
    exit 1
  fi
  if git -C "$repo" show-ref --verify --quiet "$expected_branch"; then
    local_head="$(git -C "$repo" rev-parse --verify "$expected_branch^{commit}")"
    if test "$local_head" = "$remote_head"; then
      :
    elif git -C "$repo" merge-base --is-ancestor "$local_head" "$remote_head"; then
      if ! git -C "$repo" update-ref "$expected_branch" "$remote_head" "$local_head"; then
        printf 'Blocked: local controller branch %s could not be fast-forwarded to fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
        exit 1
      fi
    elif git -C "$repo" merge-base --is-ancestor "$remote_head" "$local_head"; then
      printf 'Blocked: local controller branch %s is ahead of fetched origin tip %s and cannot be rewritten safely.\n' "$control_branch" "$remote_head" >&2
      exit 1
    else
      printf 'Blocked: local controller branch %s diverged from fetched origin tip %s and cannot be rewritten safely.\n' "$control_branch" "$remote_head" >&2
      exit 1
    fi
  else
    if ! git -C "$repo" update-ref "$expected_branch" "$remote_head"; then
      printf 'Blocked: local controller branch %s could not be created at fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
      exit 1
    fi
  fi
  local_head="$(git -C "$repo" rev-parse --verify "$expected_branch^{commit}")"
  if test "$local_head" != "$remote_head"; then
    printf 'Blocked: local controller branch %s resolved to %s instead of fetched origin tip %s.\n' "$control_branch" "$local_head" "$remote_head" >&2
    exit 1
  fi
  if test "$stale_count" -eq 1; then
    stale_path="${stale_paths[0]}"
    printf 'Recreating %s at %s from the one stale controller registration:\n  stale: %s\n' "$control_branch" "$target_path" "$stale_path" >&2
    git -C "$repo" worktree add --force "$target_path" "$control_branch"
  else
    git -C "$repo" worktree add "$target_path" "$control_branch"
  fi
  if ! test -d "$target_path"; then
    printf 'Control worktree path is not present after resolution: %s\n' "$target_path" >&2
    exit 1
  fi
  actual_branch="$(git -C "$target_path" branch --show-current)"
  if test "$actual_branch" != "$control_branch"; then
    printf 'Control worktree branch mismatch: expected %s, got %s\n' "$control_branch" "$actual_branch" >&2
    exit 1
  fi
  local_head="$(git -C "$target_path" rev-parse HEAD)"
  if test "$local_head" != "$remote_head"; then
    printf 'Blocked: resolved controller HEAD %s does not match fetched origin tip %s.\n' "$local_head" "$remote_head" >&2
    exit 1
  fi
  printf '%s\n' "$target_path"
}
CONTROL_WORKTREE="$(resolve_control_worktree)"
COMMON_DIR="$(git -C "$CONTROL_WORKTREE" rev-parse --git-common-dir)"
REPO="$(cd "$COMMON_DIR/.." && pwd)"
cd "$CONTROL_WORKTREE"
COPILOT_GH_ID=223556219
COPILOT_GH_USER=Copilot
COPILOT_NOREPLY_DOMAIN=users.noreply.github.com
COPILOT_TRAILER="Co-authored-by: $COPILOT_GH_USER <${COPILOT_GH_ID}+${COPILOT_GH_USER}@${COPILOT_NOREPLY_DOMAIN}>"
git commit -s --trailer "$COPILOT_TRAILER" \
  -m "docs: plan incomplete work recovery"
```

Expected: a commit containing only the implementation plan, the
repository-required DCO sign-off, and the Copilot co-author trailer used for
this plan commit. Human contributor co-author trailers remain conditional
under `AGENTS.md` and, when required, are added separately with additional
`--trailer` arguments.

- [ ] **Step 4: Push the design branch**

Run:

```bash
set -euo pipefail
resolve_control_worktree() {
  local control_branch="docs/541-incomplete-work-recovery-design"
  local expected_branch="refs/heads/$control_branch"
  local remote_branch_ref="refs/remotes/origin/$control_branch"
  local start_common_dir repo target_path path branch_ref actual_branch
  local target_registration_branch live_count stale_count live_path stale_path
  local live_status live_head local_head remote_head
  local -a live_paths=() stale_paths=()
  start_common_dir="$(git rev-parse --git-common-dir)"
  repo="$(cd "$start_common_dir/.." && pwd)"
  target_path="$repo/.worktrees/issue-541-recovery"
  if ! git -C "$repo" fetch --no-tags origin \
    "refs/heads/$control_branch:$remote_branch_ref"
  then
    printf 'Blocked: could not fetch origin/%s into %s.\n' \
      "$control_branch" "$remote_branch_ref" >&2
    exit 1
  fi
  remote_head="$(git -C "$repo" rev-parse --verify "$remote_branch_ref^{commit}")"
  while IFS="$(printf "\t")" read -r path branch_ref; do
    test -n "$path" || continue
    if test "$path" = "$target_path"; then
      target_registration_branch="$branch_ref"
    fi
    if test "$branch_ref" != "$expected_branch"; then
      continue
    fi
    if test -d "$path"; then
      live_paths+=("$path")
    else
      stale_paths+=("$path")
    fi
  done <<EOF2
$(git -C "$repo" worktree list --porcelain | awk '
  $1 == "worktree" { if (path != "") print path "\t" branch; path = substr($0, 10); branch = ""; next }
  $1 == "branch" { branch = $2; next }
  END { if (path != "") print path "\t" branch }
')
EOF2
  live_count="${#live_paths[@]}"
  stale_count="${#stale_paths[@]}"
  if test -n "$target_registration_branch" && test "$target_registration_branch" != "$expected_branch"; then
    printf 'Blocked: %s is registered to %s, not %s.\n' "$target_path" "$target_registration_branch" "$control_branch" >&2
    exit 1
  fi
  if test "$live_count" -gt 1; then
    printf 'Blocked: %s has %s live controller worktree registrations; refusing to choose arbitrarily.\n' "$control_branch" "$live_count" >&2
    for live_path in "${live_paths[@]}"; do
      printf '  live: %s\n' "$live_path" >&2
    done
    for stale_path in "${stale_paths[@]}"; do
      printf '  stale: %s\n' "$stale_path" >&2
    done
    exit 1
  fi
  if test "$live_count" -eq 1; then
    live_path="${live_paths[0]}"
    if test "$stale_count" -gt 0; then
      printf 'Warning: ignoring %s stale controller registration(s) for %s while using the one live worktree.\n' "$stale_count" "$control_branch" >&2
      for stale_path in "${stale_paths[@]}"; do
        printf '  stale: %s\n' "$stale_path" >&2
      done
    fi
    actual_branch="$(git -C "$live_path" branch --show-current)"
    if test "$actual_branch" != "$control_branch"; then
      printf 'Control worktree branch mismatch: expected %s, got %s\n' "$control_branch" "$actual_branch" >&2
      exit 1
    fi
    live_status="$(git -C "$live_path" status --porcelain=v1 --untracked-files=all)"
    if test -n "$live_status"; then
      printf 'Blocked: live controller worktree is dirty and cannot be refreshed safely: %s\n' "$live_path" >&2
      git -C "$live_path" status --short --branch --untracked-files=all >&2
      exit 1
    fi
    live_head="$(git -C "$live_path" rev-parse HEAD)"
    if test "$live_head" = "$remote_head"; then
      :
    elif git -C "$repo" merge-base --is-ancestor "$live_head" "$remote_head"; then
      if ! git -C "$live_path" merge --ff-only "$remote_branch_ref"; then
        printf 'Blocked: live controller worktree could not fast-forward %s to fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
        exit 1
      fi
    elif git -C "$repo" merge-base --is-ancestor "$remote_head" "$live_head"; then
      printf 'Blocked: live controller branch %s is ahead of fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
      exit 1
    else
      printf 'Blocked: live controller branch %s diverged from fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
      exit 1
    fi
    live_head="$(git -C "$live_path" rev-parse HEAD)"
    if test "$live_head" != "$remote_head"; then
      printf 'Blocked: live controller HEAD %s does not match fetched origin tip %s.\n' "$live_head" "$remote_head" >&2
      exit 1
    fi
    printf '%s\n' "$live_path"
    return 0
  fi
  if test "$stale_count" -gt 1; then
    printf 'Blocked: %s has %s stale controller registrations and no live worktree; refusing to choose arbitrarily.\n' "$control_branch" "$stale_count" >&2
    for stale_path in "${stale_paths[@]}"; do
      printf '  stale: %s\n' "$stale_path" >&2
    done
    exit 1
  fi
  if ! git -C "$repo" check-ignore -q .worktrees/; then
    printf 'Repository-local .worktrees/ is not ignored.\n' >&2
    exit 1
  fi
  if ! git -C "$repo" check-ignore -q .worktrees/issue-541-recovery; then
    printf 'Repository-local control worktree path is not ignored: %s\n' '.worktrees/issue-541-recovery' >&2
    exit 1
  fi
  mkdir -p "$repo/.worktrees"
  if test -e "$target_path"; then
    printf 'Control worktree path already exists without a matching live registration: %s\n' "$target_path" >&2
    exit 1
  fi
  if git -C "$repo" show-ref --verify --quiet "$expected_branch"; then
    local_head="$(git -C "$repo" rev-parse --verify "$expected_branch^{commit}")"
    if test "$local_head" = "$remote_head"; then
      :
    elif git -C "$repo" merge-base --is-ancestor "$local_head" "$remote_head"; then
      if ! git -C "$repo" update-ref "$expected_branch" "$remote_head" "$local_head"; then
        printf 'Blocked: local controller branch %s could not be fast-forwarded to fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
        exit 1
      fi
    elif git -C "$repo" merge-base --is-ancestor "$remote_head" "$local_head"; then
      printf 'Blocked: local controller branch %s is ahead of fetched origin tip %s and cannot be rewritten safely.\n' "$control_branch" "$remote_head" >&2
      exit 1
    else
      printf 'Blocked: local controller branch %s diverged from fetched origin tip %s and cannot be rewritten safely.\n' "$control_branch" "$remote_head" >&2
      exit 1
    fi
  else
    if ! git -C "$repo" update-ref "$expected_branch" "$remote_head"; then
      printf 'Blocked: local controller branch %s could not be created at fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
      exit 1
    fi
  fi
  local_head="$(git -C "$repo" rev-parse --verify "$expected_branch^{commit}")"
  if test "$local_head" != "$remote_head"; then
    printf 'Blocked: local controller branch %s resolved to %s instead of fetched origin tip %s.\n' "$control_branch" "$local_head" "$remote_head" >&2
    exit 1
  fi
  if test "$stale_count" -eq 1; then
    stale_path="${stale_paths[0]}"
    printf 'Recreating %s at %s from the one stale controller registration:\n  stale: %s\n' "$control_branch" "$target_path" "$stale_path" >&2
    git -C "$repo" worktree add --force "$target_path" "$control_branch"
  else
    git -C "$repo" worktree add "$target_path" "$control_branch"
  fi
  if ! test -d "$target_path"; then
    printf 'Control worktree path is not present after resolution: %s\n' "$target_path" >&2
    exit 1
  fi
  actual_branch="$(git -C "$target_path" branch --show-current)"
  if test "$actual_branch" != "$control_branch"; then
    printf 'Control worktree branch mismatch: expected %s, got %s\n' "$control_branch" "$actual_branch" >&2
    exit 1
  fi
  local_head="$(git -C "$target_path" rev-parse HEAD)"
  if test "$local_head" != "$remote_head"; then
    printf 'Blocked: resolved controller HEAD %s does not match fetched origin tip %s.\n' "$local_head" "$remote_head" >&2
    exit 1
  fi
  printf '%s\n' "$target_path"
}
CONTROL_WORKTREE="$(resolve_control_worktree)"
COMMON_DIR="$(git -C "$CONTROL_WORKTREE" rev-parse --git-common-dir)"
REPO="$(cd "$COMMON_DIR/.." && pwd)"
git -C "$CONTROL_WORKTREE" push -u origin docs/541-incomplete-work-recovery-design
```

Expected: the remote branch is created successfully.

- [ ] **Step 5: Open the design pull request**

Run:

```bash
set -euo pipefail
resolve_control_worktree() {
  local control_branch="docs/541-incomplete-work-recovery-design"
  local expected_branch="refs/heads/$control_branch"
  local remote_branch_ref="refs/remotes/origin/$control_branch"
  local start_common_dir repo target_path path branch_ref actual_branch
  local target_registration_branch live_count stale_count live_path stale_path
  local live_status live_head local_head remote_head
  local -a live_paths=() stale_paths=()
  start_common_dir="$(git rev-parse --git-common-dir)"
  repo="$(cd "$start_common_dir/.." && pwd)"
  target_path="$repo/.worktrees/issue-541-recovery"
  if ! git -C "$repo" fetch --no-tags origin \
    "refs/heads/$control_branch:$remote_branch_ref"
  then
    printf 'Blocked: could not fetch origin/%s into %s.\n' \
      "$control_branch" "$remote_branch_ref" >&2
    exit 1
  fi
  remote_head="$(git -C "$repo" rev-parse --verify "$remote_branch_ref^{commit}")"
  while IFS="$(printf "\t")" read -r path branch_ref; do
    test -n "$path" || continue
    if test "$path" = "$target_path"; then
      target_registration_branch="$branch_ref"
    fi
    if test "$branch_ref" != "$expected_branch"; then
      continue
    fi
    if test -d "$path"; then
      live_paths+=("$path")
    else
      stale_paths+=("$path")
    fi
  done <<EOF2
$(git -C "$repo" worktree list --porcelain | awk '
  $1 == "worktree" { if (path != "") print path "\t" branch; path = substr($0, 10); branch = ""; next }
  $1 == "branch" { branch = $2; next }
  END { if (path != "") print path "\t" branch }
')
EOF2
  live_count="${#live_paths[@]}"
  stale_count="${#stale_paths[@]}"
  if test -n "$target_registration_branch" && test "$target_registration_branch" != "$expected_branch"; then
    printf 'Blocked: %s is registered to %s, not %s.\n' "$target_path" "$target_registration_branch" "$control_branch" >&2
    exit 1
  fi
  if test "$live_count" -gt 1; then
    printf 'Blocked: %s has %s live controller worktree registrations; refusing to choose arbitrarily.\n' "$control_branch" "$live_count" >&2
    for live_path in "${live_paths[@]}"; do
      printf '  live: %s\n' "$live_path" >&2
    done
    for stale_path in "${stale_paths[@]}"; do
      printf '  stale: %s\n' "$stale_path" >&2
    done
    exit 1
  fi
  if test "$live_count" -eq 1; then
    live_path="${live_paths[0]}"
    if test "$stale_count" -gt 0; then
      printf 'Warning: ignoring %s stale controller registration(s) for %s while using the one live worktree.\n' "$stale_count" "$control_branch" >&2
      for stale_path in "${stale_paths[@]}"; do
        printf '  stale: %s\n' "$stale_path" >&2
      done
    fi
    actual_branch="$(git -C "$live_path" branch --show-current)"
    if test "$actual_branch" != "$control_branch"; then
      printf 'Control worktree branch mismatch: expected %s, got %s\n' "$control_branch" "$actual_branch" >&2
      exit 1
    fi
    live_status="$(git -C "$live_path" status --porcelain=v1 --untracked-files=all)"
    if test -n "$live_status"; then
      printf 'Blocked: live controller worktree is dirty and cannot be refreshed safely: %s\n' "$live_path" >&2
      git -C "$live_path" status --short --branch --untracked-files=all >&2
      exit 1
    fi
    live_head="$(git -C "$live_path" rev-parse HEAD)"
    if test "$live_head" = "$remote_head"; then
      :
    elif git -C "$repo" merge-base --is-ancestor "$live_head" "$remote_head"; then
      if ! git -C "$live_path" merge --ff-only "$remote_branch_ref"; then
        printf 'Blocked: live controller worktree could not fast-forward %s to fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
        exit 1
      fi
    elif git -C "$repo" merge-base --is-ancestor "$remote_head" "$live_head"; then
      printf 'Blocked: live controller branch %s is ahead of fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
      exit 1
    else
      printf 'Blocked: live controller branch %s diverged from fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
      exit 1
    fi
    live_head="$(git -C "$live_path" rev-parse HEAD)"
    if test "$live_head" != "$remote_head"; then
      printf 'Blocked: live controller HEAD %s does not match fetched origin tip %s.\n' "$live_head" "$remote_head" >&2
      exit 1
    fi
    printf '%s\n' "$live_path"
    return 0
  fi
  if test "$stale_count" -gt 1; then
    printf 'Blocked: %s has %s stale controller registrations and no live worktree; refusing to choose arbitrarily.\n' "$control_branch" "$stale_count" >&2
    for stale_path in "${stale_paths[@]}"; do
      printf '  stale: %s\n' "$stale_path" >&2
    done
    exit 1
  fi
  if ! git -C "$repo" check-ignore -q .worktrees/; then
    printf 'Repository-local .worktrees/ is not ignored.\n' >&2
    exit 1
  fi
  if ! git -C "$repo" check-ignore -q .worktrees/issue-541-recovery; then
    printf 'Repository-local control worktree path is not ignored: %s\n' '.worktrees/issue-541-recovery' >&2
    exit 1
  fi
  mkdir -p "$repo/.worktrees"
  if test -e "$target_path"; then
    printf 'Control worktree path already exists without a matching live registration: %s\n' "$target_path" >&2
    exit 1
  fi
  if git -C "$repo" show-ref --verify --quiet "$expected_branch"; then
    local_head="$(git -C "$repo" rev-parse --verify "$expected_branch^{commit}")"
    if test "$local_head" = "$remote_head"; then
      :
    elif git -C "$repo" merge-base --is-ancestor "$local_head" "$remote_head"; then
      if ! git -C "$repo" update-ref "$expected_branch" "$remote_head" "$local_head"; then
        printf 'Blocked: local controller branch %s could not be fast-forwarded to fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
        exit 1
      fi
    elif git -C "$repo" merge-base --is-ancestor "$remote_head" "$local_head"; then
      printf 'Blocked: local controller branch %s is ahead of fetched origin tip %s and cannot be rewritten safely.\n' "$control_branch" "$remote_head" >&2
      exit 1
    else
      printf 'Blocked: local controller branch %s diverged from fetched origin tip %s and cannot be rewritten safely.\n' "$control_branch" "$remote_head" >&2
      exit 1
    fi
  else
    if ! git -C "$repo" update-ref "$expected_branch" "$remote_head"; then
      printf 'Blocked: local controller branch %s could not be created at fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
      exit 1
    fi
  fi
  local_head="$(git -C "$repo" rev-parse --verify "$expected_branch^{commit}")"
  if test "$local_head" != "$remote_head"; then
    printf 'Blocked: local controller branch %s resolved to %s instead of fetched origin tip %s.\n' "$control_branch" "$local_head" "$remote_head" >&2
    exit 1
  fi
  if test "$stale_count" -eq 1; then
    stale_path="${stale_paths[0]}"
    printf 'Recreating %s at %s from the one stale controller registration:\n  stale: %s\n' "$control_branch" "$target_path" "$stale_path" >&2
    git -C "$repo" worktree add --force "$target_path" "$control_branch"
  else
    git -C "$repo" worktree add "$target_path" "$control_branch"
  fi
  if ! test -d "$target_path"; then
    printf 'Control worktree path is not present after resolution: %s\n' "$target_path" >&2
    exit 1
  fi
  actual_branch="$(git -C "$target_path" branch --show-current)"
  if test "$actual_branch" != "$control_branch"; then
    printf 'Control worktree branch mismatch: expected %s, got %s\n' "$control_branch" "$actual_branch" >&2
    exit 1
  fi
  local_head="$(git -C "$target_path" rev-parse HEAD)"
  if test "$local_head" != "$remote_head"; then
    printf 'Blocked: resolved controller HEAD %s does not match fetched origin tip %s.\n' "$local_head" "$remote_head" >&2
    exit 1
  fi
  printf '%s\n' "$target_path"
}
CONTROL_WORKTREE="$(resolve_control_worktree)"
COMMON_DIR="$(git -C "$CONTROL_WORKTREE" rev-parse --git-common-dir)"
REPO="$(cd "$COMMON_DIR/.." && pwd)"
cd "$CONTROL_WORKTREE"
gh pr create \
  --title "docs: design incomplete work recovery" \
  --body $'Tracks #541\n\nDefines a preservation-first process for recovering dirty worktrees and orphan branches without losing data or duplicating shipped work.\n\nThe implementation is deliberately split into one issue/spec/plan/PR per viable subsystem after snapshot and classification.'
```

Expected: GitHub returns the URL of one open pull request tracking issue #541
without auto-closing it.

### Task 2: Snapshot Every Dirty or Formerly Dirty Source Worktree

**Artifacts:**
- Create: `.git/agent-recovery/issue-541/dirty/docs-psyche-specs/`
- Create: `.git/agent-recovery/issue-541/dirty/memory-promote/`
- Create: `.git/agent-recovery/issue-541/dirty/mobile-memory-gateway/`
- Create: `.git/agent-recovery/issue-541/dirty/pr-476-review/`
- Create: `.git/agent-recovery/issue-541/dirty/docs-psyche-specs/branch.bundle`
- Create: `.git/agent-recovery/issue-541/dirty/memory-promote/branch.bundle`
- Create: `.git/agent-recovery/issue-541/dirty/mobile-memory-gateway/branch.bundle`
- Create: `.git/agent-recovery/issue-541/dirty/pr-476-review/branch.bundle`
- Create: `.git/agent-recovery/issue-541/manifest.tsv`

- [ ] **Step 1: Create the recovery archive root**

Run:

```bash
set -euo pipefail
resolve_control_worktree() {
  local control_branch="docs/541-incomplete-work-recovery-design"
  local expected_branch="refs/heads/$control_branch"
  local remote_branch_ref="refs/remotes/origin/$control_branch"
  local start_common_dir repo target_path path branch_ref actual_branch
  local target_registration_branch live_count stale_count live_path stale_path
  local live_status live_head local_head remote_head
  local -a live_paths=() stale_paths=()
  start_common_dir="$(git rev-parse --git-common-dir)"
  repo="$(cd "$start_common_dir/.." && pwd)"
  target_path="$repo/.worktrees/issue-541-recovery"
  if ! git -C "$repo" fetch --no-tags origin \
    "refs/heads/$control_branch:$remote_branch_ref"
  then
    printf 'Blocked: could not fetch origin/%s into %s.\n' \
      "$control_branch" "$remote_branch_ref" >&2
    exit 1
  fi
  remote_head="$(git -C "$repo" rev-parse --verify "$remote_branch_ref^{commit}")"
  while IFS="$(printf "\t")" read -r path branch_ref; do
    test -n "$path" || continue
    if test "$path" = "$target_path"; then
      target_registration_branch="$branch_ref"
    fi
    if test "$branch_ref" != "$expected_branch"; then
      continue
    fi
    if test -d "$path"; then
      live_paths+=("$path")
    else
      stale_paths+=("$path")
    fi
  done <<EOF2
$(git -C "$repo" worktree list --porcelain | awk '
  $1 == "worktree" { if (path != "") print path "\t" branch; path = substr($0, 10); branch = ""; next }
  $1 == "branch" { branch = $2; next }
  END { if (path != "") print path "\t" branch }
')
EOF2
  live_count="${#live_paths[@]}"
  stale_count="${#stale_paths[@]}"
  if test -n "$target_registration_branch" && test "$target_registration_branch" != "$expected_branch"; then
    printf 'Blocked: %s is registered to %s, not %s.\n' "$target_path" "$target_registration_branch" "$control_branch" >&2
    exit 1
  fi
  if test "$live_count" -gt 1; then
    printf 'Blocked: %s has %s live controller worktree registrations; refusing to choose arbitrarily.\n' "$control_branch" "$live_count" >&2
    for live_path in "${live_paths[@]}"; do
      printf '  live: %s\n' "$live_path" >&2
    done
    for stale_path in "${stale_paths[@]}"; do
      printf '  stale: %s\n' "$stale_path" >&2
    done
    exit 1
  fi
  if test "$live_count" -eq 1; then
    live_path="${live_paths[0]}"
    if test "$stale_count" -gt 0; then
      printf 'Warning: ignoring %s stale controller registration(s) for %s while using the one live worktree.\n' "$stale_count" "$control_branch" >&2
      for stale_path in "${stale_paths[@]}"; do
        printf '  stale: %s\n' "$stale_path" >&2
      done
    fi
    actual_branch="$(git -C "$live_path" branch --show-current)"
    if test "$actual_branch" != "$control_branch"; then
      printf 'Control worktree branch mismatch: expected %s, got %s\n' "$control_branch" "$actual_branch" >&2
      exit 1
    fi
    live_status="$(git -C "$live_path" status --porcelain=v1 --untracked-files=all)"
    if test -n "$live_status"; then
      printf 'Blocked: live controller worktree is dirty and cannot be refreshed safely: %s\n' "$live_path" >&2
      git -C "$live_path" status --short --branch --untracked-files=all >&2
      exit 1
    fi
    live_head="$(git -C "$live_path" rev-parse HEAD)"
    if test "$live_head" = "$remote_head"; then
      :
    elif git -C "$repo" merge-base --is-ancestor "$live_head" "$remote_head"; then
      if ! git -C "$live_path" merge --ff-only "$remote_branch_ref"; then
        printf 'Blocked: live controller worktree could not fast-forward %s to fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
        exit 1
      fi
    elif git -C "$repo" merge-base --is-ancestor "$remote_head" "$live_head"; then
      printf 'Blocked: live controller branch %s is ahead of fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
      exit 1
    else
      printf 'Blocked: live controller branch %s diverged from fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
      exit 1
    fi
    live_head="$(git -C "$live_path" rev-parse HEAD)"
    if test "$live_head" != "$remote_head"; then
      printf 'Blocked: live controller HEAD %s does not match fetched origin tip %s.\n' "$live_head" "$remote_head" >&2
      exit 1
    fi
    printf '%s\n' "$live_path"
    return 0
  fi
  if test "$stale_count" -gt 1; then
    printf 'Blocked: %s has %s stale controller registrations and no live worktree; refusing to choose arbitrarily.\n' "$control_branch" "$stale_count" >&2
    for stale_path in "${stale_paths[@]}"; do
      printf '  stale: %s\n' "$stale_path" >&2
    done
    exit 1
  fi
  if ! git -C "$repo" check-ignore -q .worktrees/; then
    printf 'Repository-local .worktrees/ is not ignored.\n' >&2
    exit 1
  fi
  if ! git -C "$repo" check-ignore -q .worktrees/issue-541-recovery; then
    printf 'Repository-local control worktree path is not ignored: %s\n' '.worktrees/issue-541-recovery' >&2
    exit 1
  fi
  mkdir -p "$repo/.worktrees"
  if test -e "$target_path"; then
    printf 'Control worktree path already exists without a matching live registration: %s\n' "$target_path" >&2
    exit 1
  fi
  if git -C "$repo" show-ref --verify --quiet "$expected_branch"; then
    local_head="$(git -C "$repo" rev-parse --verify "$expected_branch^{commit}")"
    if test "$local_head" = "$remote_head"; then
      :
    elif git -C "$repo" merge-base --is-ancestor "$local_head" "$remote_head"; then
      if ! git -C "$repo" update-ref "$expected_branch" "$remote_head" "$local_head"; then
        printf 'Blocked: local controller branch %s could not be fast-forwarded to fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
        exit 1
      fi
    elif git -C "$repo" merge-base --is-ancestor "$remote_head" "$local_head"; then
      printf 'Blocked: local controller branch %s is ahead of fetched origin tip %s and cannot be rewritten safely.\n' "$control_branch" "$remote_head" >&2
      exit 1
    else
      printf 'Blocked: local controller branch %s diverged from fetched origin tip %s and cannot be rewritten safely.\n' "$control_branch" "$remote_head" >&2
      exit 1
    fi
  else
    if ! git -C "$repo" update-ref "$expected_branch" "$remote_head"; then
      printf 'Blocked: local controller branch %s could not be created at fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
      exit 1
    fi
  fi
  local_head="$(git -C "$repo" rev-parse --verify "$expected_branch^{commit}")"
  if test "$local_head" != "$remote_head"; then
    printf 'Blocked: local controller branch %s resolved to %s instead of fetched origin tip %s.\n' "$control_branch" "$local_head" "$remote_head" >&2
    exit 1
  fi
  if test "$stale_count" -eq 1; then
    stale_path="${stale_paths[0]}"
    printf 'Recreating %s at %s from the one stale controller registration:\n  stale: %s\n' "$control_branch" "$target_path" "$stale_path" >&2
    git -C "$repo" worktree add --force "$target_path" "$control_branch"
  else
    git -C "$repo" worktree add "$target_path" "$control_branch"
  fi
  if ! test -d "$target_path"; then
    printf 'Control worktree path is not present after resolution: %s\n' "$target_path" >&2
    exit 1
  fi
  actual_branch="$(git -C "$target_path" branch --show-current)"
  if test "$actual_branch" != "$control_branch"; then
    printf 'Control worktree branch mismatch: expected %s, got %s\n' "$control_branch" "$actual_branch" >&2
    exit 1
  fi
  local_head="$(git -C "$target_path" rev-parse HEAD)"
  if test "$local_head" != "$remote_head"; then
    printf 'Blocked: resolved controller HEAD %s does not match fetched origin tip %s.\n' "$local_head" "$remote_head" >&2
    exit 1
  fi
  printf '%s\n' "$target_path"
}
CONTROL_WORKTREE="$(resolve_control_worktree)"
COMMON_DIR="$(git -C "$CONTROL_WORKTREE" rev-parse --git-common-dir)"
REPO="$(cd "$COMMON_DIR/.." && pwd)"
git -C "$REPO" fetch origin main
RECOVERY_PARENT="$COMMON_DIR/agent-recovery"
RECOVERY="$RECOVERY_PARENT/issue-541"
RERUN_ARCHIVE_ROOT="$RECOVERY_PARENT/private/reruns"
if test -e "$RECOVERY"; then
  mkdir -p "$RERUN_ARCHIVE_ROOT"
  RERUN_TIMESTAMP="$(date -u +%Y%m%dT%H%M%SZ)"
  RERUN_ARCHIVE_DIR=''
  RERUN_ATTEMPT=0
  while [ "$RERUN_ATTEMPT" -lt 32 ]; do
    CANDIDATE_ARCHIVE_DIR="$RERUN_ARCHIVE_ROOT/issue-541-rerun-$RERUN_TIMESTAMP-$$-$RERUN_ATTEMPT"
    if mkdir "$CANDIDATE_ARCHIVE_DIR" 2>/dev/null; then
      RERUN_ARCHIVE_DIR="$CANDIDATE_ARCHIVE_DIR"
      break
    fi
    RERUN_ATTEMPT=$((RERUN_ATTEMPT + 1))
  done
  if [ -z "$RERUN_ARCHIVE_DIR" ]; then
    printf 'Blocked: could not claim a unique rerun archive directory under %s after %s attempts.\n' \
      "$RERUN_ARCHIVE_ROOT" "$RERUN_ATTEMPT" >&2
    exit 1
  fi
  ARCHIVED_RECOVERY="$RERUN_ARCHIVE_DIR/issue-541"
  mv "$RECOVERY" "$ARCHIVED_RECOVERY"
  printf 'Former current root: %s\nArchived root: %s\nArchived at (UTC): %s\n' \
    "$RECOVERY" \
    "$ARCHIVED_RECOVERY" \
    "$(date -u +%Y-%m-%dT%H:%M:%SZ)" > "$RERUN_ARCHIVE_DIR/archive-record.txt"
fi
mkdir -p "$RECOVERY/dirty" "$RECOVERY/branches" "$RECOVERY/private"
printf 'id\ttype\tsource\thead\tmerge_base\tsnapshot\n' > "$RECOVERY/manifest.tsv"
```

Expected: the recovery root exists under
`$COMMON_DIR/agent-recovery/issue-541`, and that fetched `origin/main`
becomes the baseline for every Task 2 and Task 3 `merge_base` record. If a
prior current root already existed, it is first moved atomically into a unique
private rerun archive claimed under
`$COMMON_DIR/agent-recovery/private/reruns` by a bounded `mkdir` loop using a
UTC timestamp, the current PID, and an incrementing counter. The plan must
stop if no unique archive directory can be created before the attempt limit.
Once a unique archive directory exists, move the former current root into it,
record the former fixed location and archival time in `archive-record.txt`, and
then create a fresh current root at the same fixed path so downstream
`issue-541/...` paths stay valid. If the archive claim, archival move, or
fresh-root creation fails, stop before writing any new manifest or snapshot.
Task 4 may fetch `origin/main` again before classification.

- [ ] **Step 2: Snapshot `docs-psyche-specs`**

Run:

```bash
set -euo pipefail
resolve_control_worktree() {
  local control_branch="docs/541-incomplete-work-recovery-design"
  local expected_branch="refs/heads/$control_branch"
  local remote_branch_ref="refs/remotes/origin/$control_branch"
  local start_common_dir repo target_path path branch_ref actual_branch
  local target_registration_branch live_count stale_count live_path stale_path
  local live_status live_head local_head remote_head
  local -a live_paths=() stale_paths=()
  start_common_dir="$(git rev-parse --git-common-dir)"
  repo="$(cd "$start_common_dir/.." && pwd)"
  target_path="$repo/.worktrees/issue-541-recovery"
  if ! git -C "$repo" fetch --no-tags origin \
    "refs/heads/$control_branch:$remote_branch_ref"
  then
    printf 'Blocked: could not fetch origin/%s into %s.\n' \
      "$control_branch" "$remote_branch_ref" >&2
    exit 1
  fi
  remote_head="$(git -C "$repo" rev-parse --verify "$remote_branch_ref^{commit}")"
  while IFS="$(printf "\t")" read -r path branch_ref; do
    test -n "$path" || continue
    if test "$path" = "$target_path"; then
      target_registration_branch="$branch_ref"
    fi
    if test "$branch_ref" != "$expected_branch"; then
      continue
    fi
    if test -d "$path"; then
      live_paths+=("$path")
    else
      stale_paths+=("$path")
    fi
  done <<EOF2
$(git -C "$repo" worktree list --porcelain | awk '
  $1 == "worktree" { if (path != "") print path "\t" branch; path = substr($0, 10); branch = ""; next }
  $1 == "branch" { branch = $2; next }
  END { if (path != "") print path "\t" branch }
')
EOF2
  live_count="${#live_paths[@]}"
  stale_count="${#stale_paths[@]}"
  if test -n "$target_registration_branch" && test "$target_registration_branch" != "$expected_branch"; then
    printf 'Blocked: %s is registered to %s, not %s.\n' "$target_path" "$target_registration_branch" "$control_branch" >&2
    exit 1
  fi
  if test "$live_count" -gt 1; then
    printf 'Blocked: %s has %s live controller worktree registrations; refusing to choose arbitrarily.\n' "$control_branch" "$live_count" >&2
    for live_path in "${live_paths[@]}"; do
      printf '  live: %s\n' "$live_path" >&2
    done
    for stale_path in "${stale_paths[@]}"; do
      printf '  stale: %s\n' "$stale_path" >&2
    done
    exit 1
  fi
  if test "$live_count" -eq 1; then
    live_path="${live_paths[0]}"
    if test "$stale_count" -gt 0; then
      printf 'Warning: ignoring %s stale controller registration(s) for %s while using the one live worktree.\n' "$stale_count" "$control_branch" >&2
      for stale_path in "${stale_paths[@]}"; do
        printf '  stale: %s\n' "$stale_path" >&2
      done
    fi
    actual_branch="$(git -C "$live_path" branch --show-current)"
    if test "$actual_branch" != "$control_branch"; then
      printf 'Control worktree branch mismatch: expected %s, got %s\n' "$control_branch" "$actual_branch" >&2
      exit 1
    fi
    live_status="$(git -C "$live_path" status --porcelain=v1 --untracked-files=all)"
    if test -n "$live_status"; then
      printf 'Blocked: live controller worktree is dirty and cannot be refreshed safely: %s\n' "$live_path" >&2
      git -C "$live_path" status --short --branch --untracked-files=all >&2
      exit 1
    fi
    live_head="$(git -C "$live_path" rev-parse HEAD)"
    if test "$live_head" = "$remote_head"; then
      :
    elif git -C "$repo" merge-base --is-ancestor "$live_head" "$remote_head"; then
      if ! git -C "$live_path" merge --ff-only "$remote_branch_ref"; then
        printf 'Blocked: live controller worktree could not fast-forward %s to fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
        exit 1
      fi
    elif git -C "$repo" merge-base --is-ancestor "$remote_head" "$live_head"; then
      printf 'Blocked: live controller branch %s is ahead of fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
      exit 1
    else
      printf 'Blocked: live controller branch %s diverged from fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
      exit 1
    fi
    live_head="$(git -C "$live_path" rev-parse HEAD)"
    if test "$live_head" != "$remote_head"; then
      printf 'Blocked: live controller HEAD %s does not match fetched origin tip %s.\n' "$live_head" "$remote_head" >&2
      exit 1
    fi
    printf '%s\n' "$live_path"
    return 0
  fi
  if test "$stale_count" -gt 1; then
    printf 'Blocked: %s has %s stale controller registrations and no live worktree; refusing to choose arbitrarily.\n' "$control_branch" "$stale_count" >&2
    for stale_path in "${stale_paths[@]}"; do
      printf '  stale: %s\n' "$stale_path" >&2
    done
    exit 1
  fi
  if ! git -C "$repo" check-ignore -q .worktrees/; then
    printf 'Repository-local .worktrees/ is not ignored.\n' >&2
    exit 1
  fi
  if ! git -C "$repo" check-ignore -q .worktrees/issue-541-recovery; then
    printf 'Repository-local control worktree path is not ignored: %s\n' '.worktrees/issue-541-recovery' >&2
    exit 1
  fi
  mkdir -p "$repo/.worktrees"
  if test -e "$target_path"; then
    printf 'Control worktree path already exists without a matching live registration: %s\n' "$target_path" >&2
    exit 1
  fi
  if git -C "$repo" show-ref --verify --quiet "$expected_branch"; then
    local_head="$(git -C "$repo" rev-parse --verify "$expected_branch^{commit}")"
    if test "$local_head" = "$remote_head"; then
      :
    elif git -C "$repo" merge-base --is-ancestor "$local_head" "$remote_head"; then
      if ! git -C "$repo" update-ref "$expected_branch" "$remote_head" "$local_head"; then
        printf 'Blocked: local controller branch %s could not be fast-forwarded to fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
        exit 1
      fi
    elif git -C "$repo" merge-base --is-ancestor "$remote_head" "$local_head"; then
      printf 'Blocked: local controller branch %s is ahead of fetched origin tip %s and cannot be rewritten safely.\n' "$control_branch" "$remote_head" >&2
      exit 1
    else
      printf 'Blocked: local controller branch %s diverged from fetched origin tip %s and cannot be rewritten safely.\n' "$control_branch" "$remote_head" >&2
      exit 1
    fi
  else
    if ! git -C "$repo" update-ref "$expected_branch" "$remote_head"; then
      printf 'Blocked: local controller branch %s could not be created at fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
      exit 1
    fi
  fi
  local_head="$(git -C "$repo" rev-parse --verify "$expected_branch^{commit}")"
  if test "$local_head" != "$remote_head"; then
    printf 'Blocked: local controller branch %s resolved to %s instead of fetched origin tip %s.\n' "$control_branch" "$local_head" "$remote_head" >&2
    exit 1
  fi
  if test "$stale_count" -eq 1; then
    stale_path="${stale_paths[0]}"
    printf 'Recreating %s at %s from the one stale controller registration:\n  stale: %s\n' "$control_branch" "$target_path" "$stale_path" >&2
    git -C "$repo" worktree add --force "$target_path" "$control_branch"
  else
    git -C "$repo" worktree add "$target_path" "$control_branch"
  fi
  if ! test -d "$target_path"; then
    printf 'Control worktree path is not present after resolution: %s\n' "$target_path" >&2
    exit 1
  fi
  actual_branch="$(git -C "$target_path" branch --show-current)"
  if test "$actual_branch" != "$control_branch"; then
    printf 'Control worktree branch mismatch: expected %s, got %s\n' "$control_branch" "$actual_branch" >&2
    exit 1
  fi
  local_head="$(git -C "$target_path" rev-parse HEAD)"
  if test "$local_head" != "$remote_head"; then
    printf 'Blocked: resolved controller HEAD %s does not match fetched origin tip %s.\n' "$local_head" "$remote_head" >&2
    exit 1
  fi
  printf '%s\n' "$target_path"
}
CONTROL_WORKTREE="$(resolve_control_worktree)"
COMMON_DIR="$(git -C "$CONTROL_WORKTREE" rev-parse --git-common-dir)"
REPO="$(cd "$COMMON_DIR/.." && pwd)"
SOURCE="$REPO/.worktrees/docs-psyche-specs"
DEST="$COMMON_DIR/agent-recovery/issue-541/dirty/docs-psyche-specs"
BRANCH="$(git -C "$SOURCE" branch --show-current)"
mkdir -p "$DEST"
git -C "$SOURCE" status --short --branch > "$DEST/status.txt"
printf '%s\n' "$BRANCH" > "$DEST/branch.txt"
git -C "$SOURCE" rev-parse HEAD > "$DEST/head.txt"
git -C "$SOURCE" merge-base HEAD origin/main > "$DEST/merge-base.txt"
git -C "$REPO" bundle create "$DEST/branch.bundle" "$BRANCH"
git -C "$REPO" bundle verify "$DEST/branch.bundle"
git -C "$SOURCE" diff --binary > "$DEST/worktree.patch"
git -C "$SOURCE" diff --cached --binary > "$DEST/index.patch"
git -C "$SOURCE" ls-files --others --exclude-standard -z > "$DEST/.untracked.zlist"
python3 - "$DEST/.untracked.zlist" > "$DEST/untracked.json" <<'PY'
import json
import sys
from pathlib import Path

raw = Path(sys.argv[1]).read_bytes()
entries = [] if not raw else raw.rstrip(b"\0").split(b"\0")
print(json.dumps([entry.decode("utf-8", "surrogateescape") for entry in entries], indent=2))
PY
tar -C "$SOURCE" --null -T "$DEST/.untracked.zlist" -cf "$DEST/untracked.tar"
git -C "$SOURCE" ls-files --others --ignored --exclude-standard -z > "$DEST/.ignored.zlist"
python3 - "$DEST/.ignored.zlist" > "$DEST/ignored.json" <<'PY'
import json
import sys
from pathlib import Path

raw = Path(sys.argv[1]).read_bytes()
entries = [] if not raw else raw.rstrip(b"\0").split(b"\0")
print(json.dumps([entry.decode("utf-8", "surrogateescape") for entry in entries], indent=2))
PY
printf 'docs-psyche-specs\tdirty-worktree\t%s\t%s\t%s\t%s\n' \
  "$SOURCE" \
  "$(cat "$DEST/head.txt")" \
  "$(cat "$DEST/merge-base.txt")" \
  "$DEST" >> "$COMMON_DIR/agent-recovery/issue-541/manifest.tsv"
```

Expected: `branch.bundle` verifies against `docs/psyche-specs`, preserving its
committed history before any later branch deletion; both patch files exist; the
private `.untracked.zlist` and `.ignored.zlist` inventories are NUL-delimited;
`untracked.json` and `ignored.json` are JSON-escaped readable renderings of
those inventories; and `untracked.tar` preserves exactly the listed untracked
entries from the source worktree without rewriting newline pathnames or symlink
metadata. Ignored content is inventoried only and is never archived. If the
worktree is still dirty, the patch files capture those changes. If the worktree
is now clean because the source branch already backs an active PR, the patch
files may be empty and `status.txt` becomes the evidence of that clean
post-commit state.

- [ ] **Step 3: Snapshot `feat-cmem-1ev-memory-promote`**

Run:

```bash
set -euo pipefail
resolve_control_worktree() {
  local control_branch="docs/541-incomplete-work-recovery-design"
  local expected_branch="refs/heads/$control_branch"
  local remote_branch_ref="refs/remotes/origin/$control_branch"
  local start_common_dir repo target_path path branch_ref actual_branch
  local target_registration_branch live_count stale_count live_path stale_path
  local live_status live_head local_head remote_head
  local -a live_paths=() stale_paths=()
  start_common_dir="$(git rev-parse --git-common-dir)"
  repo="$(cd "$start_common_dir/.." && pwd)"
  target_path="$repo/.worktrees/issue-541-recovery"
  if ! git -C "$repo" fetch --no-tags origin \
    "refs/heads/$control_branch:$remote_branch_ref"
  then
    printf 'Blocked: could not fetch origin/%s into %s.\n' \
      "$control_branch" "$remote_branch_ref" >&2
    exit 1
  fi
  remote_head="$(git -C "$repo" rev-parse --verify "$remote_branch_ref^{commit}")"
  while IFS="$(printf "\t")" read -r path branch_ref; do
    test -n "$path" || continue
    if test "$path" = "$target_path"; then
      target_registration_branch="$branch_ref"
    fi
    if test "$branch_ref" != "$expected_branch"; then
      continue
    fi
    if test -d "$path"; then
      live_paths+=("$path")
    else
      stale_paths+=("$path")
    fi
  done <<EOF2
$(git -C "$repo" worktree list --porcelain | awk '
  $1 == "worktree" { if (path != "") print path "\t" branch; path = substr($0, 10); branch = ""; next }
  $1 == "branch" { branch = $2; next }
  END { if (path != "") print path "\t" branch }
')
EOF2
  live_count="${#live_paths[@]}"
  stale_count="${#stale_paths[@]}"
  if test -n "$target_registration_branch" && test "$target_registration_branch" != "$expected_branch"; then
    printf 'Blocked: %s is registered to %s, not %s.\n' "$target_path" "$target_registration_branch" "$control_branch" >&2
    exit 1
  fi
  if test "$live_count" -gt 1; then
    printf 'Blocked: %s has %s live controller worktree registrations; refusing to choose arbitrarily.\n' "$control_branch" "$live_count" >&2
    for live_path in "${live_paths[@]}"; do
      printf '  live: %s\n' "$live_path" >&2
    done
    for stale_path in "${stale_paths[@]}"; do
      printf '  stale: %s\n' "$stale_path" >&2
    done
    exit 1
  fi
  if test "$live_count" -eq 1; then
    live_path="${live_paths[0]}"
    if test "$stale_count" -gt 0; then
      printf 'Warning: ignoring %s stale controller registration(s) for %s while using the one live worktree.\n' "$stale_count" "$control_branch" >&2
      for stale_path in "${stale_paths[@]}"; do
        printf '  stale: %s\n' "$stale_path" >&2
      done
    fi
    actual_branch="$(git -C "$live_path" branch --show-current)"
    if test "$actual_branch" != "$control_branch"; then
      printf 'Control worktree branch mismatch: expected %s, got %s\n' "$control_branch" "$actual_branch" >&2
      exit 1
    fi
    live_status="$(git -C "$live_path" status --porcelain=v1 --untracked-files=all)"
    if test -n "$live_status"; then
      printf 'Blocked: live controller worktree is dirty and cannot be refreshed safely: %s\n' "$live_path" >&2
      git -C "$live_path" status --short --branch --untracked-files=all >&2
      exit 1
    fi
    live_head="$(git -C "$live_path" rev-parse HEAD)"
    if test "$live_head" = "$remote_head"; then
      :
    elif git -C "$repo" merge-base --is-ancestor "$live_head" "$remote_head"; then
      if ! git -C "$live_path" merge --ff-only "$remote_branch_ref"; then
        printf 'Blocked: live controller worktree could not fast-forward %s to fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
        exit 1
      fi
    elif git -C "$repo" merge-base --is-ancestor "$remote_head" "$live_head"; then
      printf 'Blocked: live controller branch %s is ahead of fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
      exit 1
    else
      printf 'Blocked: live controller branch %s diverged from fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
      exit 1
    fi
    live_head="$(git -C "$live_path" rev-parse HEAD)"
    if test "$live_head" != "$remote_head"; then
      printf 'Blocked: live controller HEAD %s does not match fetched origin tip %s.\n' "$live_head" "$remote_head" >&2
      exit 1
    fi
    printf '%s\n' "$live_path"
    return 0
  fi
  if test "$stale_count" -gt 1; then
    printf 'Blocked: %s has %s stale controller registrations and no live worktree; refusing to choose arbitrarily.\n' "$control_branch" "$stale_count" >&2
    for stale_path in "${stale_paths[@]}"; do
      printf '  stale: %s\n' "$stale_path" >&2
    done
    exit 1
  fi
  if ! git -C "$repo" check-ignore -q .worktrees/; then
    printf 'Repository-local .worktrees/ is not ignored.\n' >&2
    exit 1
  fi
  if ! git -C "$repo" check-ignore -q .worktrees/issue-541-recovery; then
    printf 'Repository-local control worktree path is not ignored: %s\n' '.worktrees/issue-541-recovery' >&2
    exit 1
  fi
  mkdir -p "$repo/.worktrees"
  if test -e "$target_path"; then
    printf 'Control worktree path already exists without a matching live registration: %s\n' "$target_path" >&2
    exit 1
  fi
  if git -C "$repo" show-ref --verify --quiet "$expected_branch"; then
    local_head="$(git -C "$repo" rev-parse --verify "$expected_branch^{commit}")"
    if test "$local_head" = "$remote_head"; then
      :
    elif git -C "$repo" merge-base --is-ancestor "$local_head" "$remote_head"; then
      if ! git -C "$repo" update-ref "$expected_branch" "$remote_head" "$local_head"; then
        printf 'Blocked: local controller branch %s could not be fast-forwarded to fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
        exit 1
      fi
    elif git -C "$repo" merge-base --is-ancestor "$remote_head" "$local_head"; then
      printf 'Blocked: local controller branch %s is ahead of fetched origin tip %s and cannot be rewritten safely.\n' "$control_branch" "$remote_head" >&2
      exit 1
    else
      printf 'Blocked: local controller branch %s diverged from fetched origin tip %s and cannot be rewritten safely.\n' "$control_branch" "$remote_head" >&2
      exit 1
    fi
  else
    if ! git -C "$repo" update-ref "$expected_branch" "$remote_head"; then
      printf 'Blocked: local controller branch %s could not be created at fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
      exit 1
    fi
  fi
  local_head="$(git -C "$repo" rev-parse --verify "$expected_branch^{commit}")"
  if test "$local_head" != "$remote_head"; then
    printf 'Blocked: local controller branch %s resolved to %s instead of fetched origin tip %s.\n' "$control_branch" "$local_head" "$remote_head" >&2
    exit 1
  fi
  if test "$stale_count" -eq 1; then
    stale_path="${stale_paths[0]}"
    printf 'Recreating %s at %s from the one stale controller registration:\n  stale: %s\n' "$control_branch" "$target_path" "$stale_path" >&2
    git -C "$repo" worktree add --force "$target_path" "$control_branch"
  else
    git -C "$repo" worktree add "$target_path" "$control_branch"
  fi
  if ! test -d "$target_path"; then
    printf 'Control worktree path is not present after resolution: %s\n' "$target_path" >&2
    exit 1
  fi
  actual_branch="$(git -C "$target_path" branch --show-current)"
  if test "$actual_branch" != "$control_branch"; then
    printf 'Control worktree branch mismatch: expected %s, got %s\n' "$control_branch" "$actual_branch" >&2
    exit 1
  fi
  local_head="$(git -C "$target_path" rev-parse HEAD)"
  if test "$local_head" != "$remote_head"; then
    printf 'Blocked: resolved controller HEAD %s does not match fetched origin tip %s.\n' "$local_head" "$remote_head" >&2
    exit 1
  fi
  printf '%s\n' "$target_path"
}
CONTROL_WORKTREE="$(resolve_control_worktree)"
COMMON_DIR="$(git -C "$CONTROL_WORKTREE" rev-parse --git-common-dir)"
REPO="$(cd "$COMMON_DIR/.." && pwd)"
SOURCE="$REPO/.worktrees/feat-cmem-1ev-memory-promote"
DEST="$COMMON_DIR/agent-recovery/issue-541/dirty/memory-promote"
BRANCH="$(git -C "$SOURCE" branch --show-current)"
mkdir -p "$DEST"
git -C "$SOURCE" status --short --branch > "$DEST/status.txt"
printf '%s\n' "$BRANCH" > "$DEST/branch.txt"
git -C "$SOURCE" rev-parse HEAD > "$DEST/head.txt"
git -C "$SOURCE" merge-base HEAD origin/main > "$DEST/merge-base.txt"
git -C "$REPO" bundle create "$DEST/branch.bundle" "$BRANCH"
git -C "$REPO" bundle verify "$DEST/branch.bundle"
git -C "$SOURCE" diff --binary > "$DEST/worktree.patch"
git -C "$SOURCE" diff --cached --binary > "$DEST/index.patch"
git -C "$SOURCE" ls-files --others --exclude-standard -z > "$DEST/.untracked.zlist"
python3 - "$DEST/.untracked.zlist" > "$DEST/untracked.json" <<'PY'
import json
import sys
from pathlib import Path

raw = Path(sys.argv[1]).read_bytes()
entries = [] if not raw else raw.rstrip(b"\0").split(b"\0")
print(json.dumps([entry.decode("utf-8", "surrogateescape") for entry in entries], indent=2))
PY
tar -C "$SOURCE" --null -T "$DEST/.untracked.zlist" -cf "$DEST/untracked.tar"
git -C "$SOURCE" ls-files --others --ignored --exclude-standard -z > "$DEST/.ignored.zlist"
python3 - "$DEST/.ignored.zlist" > "$DEST/ignored.json" <<'PY'
import json
import sys
from pathlib import Path

raw = Path(sys.argv[1]).read_bytes()
entries = [] if not raw else raw.rstrip(b"\0").split(b"\0")
print(json.dumps([entry.decode("utf-8", "surrogateescape") for entry in entries], indent=2))
PY
printf 'memory-promote\tdirty-worktree\t%s\t%s\t%s\t%s\n' \
  "$SOURCE" \
  "$(cat "$DEST/head.txt")" \
  "$(cat "$DEST/merge-base.txt")" \
  "$DEST" >> "$COMMON_DIR/agent-recovery/issue-541/manifest.tsv"
```

Expected: `branch.bundle` verifies against
`feat/cmem-1ev-memory-promote`, preserving committed branch history before any
later branch deletion, and the untracked inventory plus `untracked.tar`
preserve `crates/coven-memory/src/promotion.rs`,
`scripts/check-coven-privacy.py`, and
`scripts/check-coven-privacy-test.py` losslessly. Any ignored paths are
captured only in `.ignored.zlist` and `ignored.json`.

- [ ] **Step 4: Snapshot `feat/mobile-memory-gateway`**

Run:

```bash
set -euo pipefail
resolve_control_worktree() {
  local control_branch="docs/541-incomplete-work-recovery-design"
  local expected_branch="refs/heads/$control_branch"
  local remote_branch_ref="refs/remotes/origin/$control_branch"
  local start_common_dir repo target_path path branch_ref actual_branch
  local target_registration_branch live_count stale_count live_path stale_path
  local live_status live_head local_head remote_head
  local -a live_paths=() stale_paths=()
  start_common_dir="$(git rev-parse --git-common-dir)"
  repo="$(cd "$start_common_dir/.." && pwd)"
  target_path="$repo/.worktrees/issue-541-recovery"
  if ! git -C "$repo" fetch --no-tags origin \
    "refs/heads/$control_branch:$remote_branch_ref"
  then
    printf 'Blocked: could not fetch origin/%s into %s.\n' \
      "$control_branch" "$remote_branch_ref" >&2
    exit 1
  fi
  remote_head="$(git -C "$repo" rev-parse --verify "$remote_branch_ref^{commit}")"
  while IFS="$(printf "\t")" read -r path branch_ref; do
    test -n "$path" || continue
    if test "$path" = "$target_path"; then
      target_registration_branch="$branch_ref"
    fi
    if test "$branch_ref" != "$expected_branch"; then
      continue
    fi
    if test -d "$path"; then
      live_paths+=("$path")
    else
      stale_paths+=("$path")
    fi
  done <<EOF2
$(git -C "$repo" worktree list --porcelain | awk '
  $1 == "worktree" { if (path != "") print path "\t" branch; path = substr($0, 10); branch = ""; next }
  $1 == "branch" { branch = $2; next }
  END { if (path != "") print path "\t" branch }
')
EOF2
  live_count="${#live_paths[@]}"
  stale_count="${#stale_paths[@]}"
  if test -n "$target_registration_branch" && test "$target_registration_branch" != "$expected_branch"; then
    printf 'Blocked: %s is registered to %s, not %s.\n' "$target_path" "$target_registration_branch" "$control_branch" >&2
    exit 1
  fi
  if test "$live_count" -gt 1; then
    printf 'Blocked: %s has %s live controller worktree registrations; refusing to choose arbitrarily.\n' "$control_branch" "$live_count" >&2
    for live_path in "${live_paths[@]}"; do
      printf '  live: %s\n' "$live_path" >&2
    done
    for stale_path in "${stale_paths[@]}"; do
      printf '  stale: %s\n' "$stale_path" >&2
    done
    exit 1
  fi
  if test "$live_count" -eq 1; then
    live_path="${live_paths[0]}"
    if test "$stale_count" -gt 0; then
      printf 'Warning: ignoring %s stale controller registration(s) for %s while using the one live worktree.\n' "$stale_count" "$control_branch" >&2
      for stale_path in "${stale_paths[@]}"; do
        printf '  stale: %s\n' "$stale_path" >&2
      done
    fi
    actual_branch="$(git -C "$live_path" branch --show-current)"
    if test "$actual_branch" != "$control_branch"; then
      printf 'Control worktree branch mismatch: expected %s, got %s\n' "$control_branch" "$actual_branch" >&2
      exit 1
    fi
    live_status="$(git -C "$live_path" status --porcelain=v1 --untracked-files=all)"
    if test -n "$live_status"; then
      printf 'Blocked: live controller worktree is dirty and cannot be refreshed safely: %s\n' "$live_path" >&2
      git -C "$live_path" status --short --branch --untracked-files=all >&2
      exit 1
    fi
    live_head="$(git -C "$live_path" rev-parse HEAD)"
    if test "$live_head" = "$remote_head"; then
      :
    elif git -C "$repo" merge-base --is-ancestor "$live_head" "$remote_head"; then
      if ! git -C "$live_path" merge --ff-only "$remote_branch_ref"; then
        printf 'Blocked: live controller worktree could not fast-forward %s to fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
        exit 1
      fi
    elif git -C "$repo" merge-base --is-ancestor "$remote_head" "$live_head"; then
      printf 'Blocked: live controller branch %s is ahead of fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
      exit 1
    else
      printf 'Blocked: live controller branch %s diverged from fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
      exit 1
    fi
    live_head="$(git -C "$live_path" rev-parse HEAD)"
    if test "$live_head" != "$remote_head"; then
      printf 'Blocked: live controller HEAD %s does not match fetched origin tip %s.\n' "$live_head" "$remote_head" >&2
      exit 1
    fi
    printf '%s\n' "$live_path"
    return 0
  fi
  if test "$stale_count" -gt 1; then
    printf 'Blocked: %s has %s stale controller registrations and no live worktree; refusing to choose arbitrarily.\n' "$control_branch" "$stale_count" >&2
    for stale_path in "${stale_paths[@]}"; do
      printf '  stale: %s\n' "$stale_path" >&2
    done
    exit 1
  fi
  if ! git -C "$repo" check-ignore -q .worktrees/; then
    printf 'Repository-local .worktrees/ is not ignored.\n' >&2
    exit 1
  fi
  if ! git -C "$repo" check-ignore -q .worktrees/issue-541-recovery; then
    printf 'Repository-local control worktree path is not ignored: %s\n' '.worktrees/issue-541-recovery' >&2
    exit 1
  fi
  mkdir -p "$repo/.worktrees"
  if test -e "$target_path"; then
    printf 'Control worktree path already exists without a matching live registration: %s\n' "$target_path" >&2
    exit 1
  fi
  if git -C "$repo" show-ref --verify --quiet "$expected_branch"; then
    local_head="$(git -C "$repo" rev-parse --verify "$expected_branch^{commit}")"
    if test "$local_head" = "$remote_head"; then
      :
    elif git -C "$repo" merge-base --is-ancestor "$local_head" "$remote_head"; then
      if ! git -C "$repo" update-ref "$expected_branch" "$remote_head" "$local_head"; then
        printf 'Blocked: local controller branch %s could not be fast-forwarded to fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
        exit 1
      fi
    elif git -C "$repo" merge-base --is-ancestor "$remote_head" "$local_head"; then
      printf 'Blocked: local controller branch %s is ahead of fetched origin tip %s and cannot be rewritten safely.\n' "$control_branch" "$remote_head" >&2
      exit 1
    else
      printf 'Blocked: local controller branch %s diverged from fetched origin tip %s and cannot be rewritten safely.\n' "$control_branch" "$remote_head" >&2
      exit 1
    fi
  else
    if ! git -C "$repo" update-ref "$expected_branch" "$remote_head"; then
      printf 'Blocked: local controller branch %s could not be created at fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
      exit 1
    fi
  fi
  local_head="$(git -C "$repo" rev-parse --verify "$expected_branch^{commit}")"
  if test "$local_head" != "$remote_head"; then
    printf 'Blocked: local controller branch %s resolved to %s instead of fetched origin tip %s.\n' "$control_branch" "$local_head" "$remote_head" >&2
    exit 1
  fi
  if test "$stale_count" -eq 1; then
    stale_path="${stale_paths[0]}"
    printf 'Recreating %s at %s from the one stale controller registration:\n  stale: %s\n' "$control_branch" "$target_path" "$stale_path" >&2
    git -C "$repo" worktree add --force "$target_path" "$control_branch"
  else
    git -C "$repo" worktree add "$target_path" "$control_branch"
  fi
  if ! test -d "$target_path"; then
    printf 'Control worktree path is not present after resolution: %s\n' "$target_path" >&2
    exit 1
  fi
  actual_branch="$(git -C "$target_path" branch --show-current)"
  if test "$actual_branch" != "$control_branch"; then
    printf 'Control worktree branch mismatch: expected %s, got %s\n' "$control_branch" "$actual_branch" >&2
    exit 1
  fi
  local_head="$(git -C "$target_path" rev-parse HEAD)"
  if test "$local_head" != "$remote_head"; then
    printf 'Blocked: resolved controller HEAD %s does not match fetched origin tip %s.\n' "$local_head" "$remote_head" >&2
    exit 1
  fi
  printf '%s\n' "$target_path"
}
CONTROL_WORKTREE="$(resolve_control_worktree)"
COMMON_DIR="$(git -C "$CONTROL_WORKTREE" rev-parse --git-common-dir)"
REPO="$(cd "$COMMON_DIR/.." && pwd)"
SOURCE="$REPO/.worktrees/mobile-memory-gateway"
DEST="$COMMON_DIR/agent-recovery/issue-541/dirty/mobile-memory-gateway"
BRANCH="$(git -C "$SOURCE" branch --show-current)"
mkdir -p "$DEST"
git -C "$SOURCE" status --short --branch > "$DEST/status.txt"
printf '%s\n' "$BRANCH" > "$DEST/branch.txt"
git -C "$SOURCE" rev-parse HEAD > "$DEST/head.txt"
git -C "$SOURCE" merge-base HEAD origin/main > "$DEST/merge-base.txt"
git -C "$REPO" bundle create "$DEST/branch.bundle" "$BRANCH"
git -C "$REPO" bundle verify "$DEST/branch.bundle"
git -C "$SOURCE" diff --binary > "$DEST/worktree.patch"
git -C "$SOURCE" diff --cached --binary > "$DEST/index.patch"
git -C "$SOURCE" ls-files --others --exclude-standard -z > "$DEST/.untracked.zlist"
python3 - "$DEST/.untracked.zlist" > "$DEST/untracked.json" <<'PY'
import json
import sys
from pathlib import Path

raw = Path(sys.argv[1]).read_bytes()
entries = [] if not raw else raw.rstrip(b"\0").split(b"\0")
print(json.dumps([entry.decode("utf-8", "surrogateescape") for entry in entries], indent=2))
PY
tar -C "$SOURCE" --null -T "$DEST/.untracked.zlist" -cf "$DEST/untracked.tar"
git -C "$SOURCE" ls-files --others --ignored --exclude-standard -z > "$DEST/.ignored.zlist"
python3 - "$DEST/.ignored.zlist" > "$DEST/ignored.json" <<'PY'
import json
import sys
from pathlib import Path

raw = Path(sys.argv[1]).read_bytes()
entries = [] if not raw else raw.rstrip(b"\0").split(b"\0")
print(json.dumps([entry.decode("utf-8", "surrogateescape") for entry in entries], indent=2))
PY
printf 'mobile-memory-gateway\tdirty-worktree\t%s\t%s\t%s\t%s\n' \
  "$SOURCE" \
  "$(cat "$DEST/head.txt")" \
  "$(cat "$DEST/merge-base.txt")" \
  "$DEST" >> "$COMMON_DIR/agent-recovery/issue-541/manifest.tsv"
```

Expected: `branch.bundle` verifies against `feat/mobile-memory-gateway`,
preserving committed branch history before any later branch deletion, and
`worktree.patch` contains the changes to
`crates/coven-cli/src/mobile_memory/pairing.rs`. Any untracked additions are
captured through `.untracked.zlist`, `untracked.json`, and `untracked.tar`,
while ignored paths remain inventory-only.

- [ ] **Step 5: Snapshot `fix/476-review-threads`**

Run:

```bash
set -euo pipefail
resolve_control_worktree() {
  local control_branch="docs/541-incomplete-work-recovery-design"
  local expected_branch="refs/heads/$control_branch"
  local remote_branch_ref="refs/remotes/origin/$control_branch"
  local start_common_dir repo target_path path branch_ref actual_branch
  local target_registration_branch live_count stale_count live_path stale_path
  local live_status live_head local_head remote_head
  local -a live_paths=() stale_paths=()
  start_common_dir="$(git rev-parse --git-common-dir)"
  repo="$(cd "$start_common_dir/.." && pwd)"
  target_path="$repo/.worktrees/issue-541-recovery"
  if ! git -C "$repo" fetch --no-tags origin \
    "refs/heads/$control_branch:$remote_branch_ref"
  then
    printf 'Blocked: could not fetch origin/%s into %s.\n' \
      "$control_branch" "$remote_branch_ref" >&2
    exit 1
  fi
  remote_head="$(git -C "$repo" rev-parse --verify "$remote_branch_ref^{commit}")"
  while IFS="$(printf "\t")" read -r path branch_ref; do
    test -n "$path" || continue
    if test "$path" = "$target_path"; then
      target_registration_branch="$branch_ref"
    fi
    if test "$branch_ref" != "$expected_branch"; then
      continue
    fi
    if test -d "$path"; then
      live_paths+=("$path")
    else
      stale_paths+=("$path")
    fi
  done <<EOF2
$(git -C "$repo" worktree list --porcelain | awk '
  $1 == "worktree" { if (path != "") print path "\t" branch; path = substr($0, 10); branch = ""; next }
  $1 == "branch" { branch = $2; next }
  END { if (path != "") print path "\t" branch }
')
EOF2
  live_count="${#live_paths[@]}"
  stale_count="${#stale_paths[@]}"
  if test -n "$target_registration_branch" && test "$target_registration_branch" != "$expected_branch"; then
    printf 'Blocked: %s is registered to %s, not %s.\n' "$target_path" "$target_registration_branch" "$control_branch" >&2
    exit 1
  fi
  if test "$live_count" -gt 1; then
    printf 'Blocked: %s has %s live controller worktree registrations; refusing to choose arbitrarily.\n' "$control_branch" "$live_count" >&2
    for live_path in "${live_paths[@]}"; do
      printf '  live: %s\n' "$live_path" >&2
    done
    for stale_path in "${stale_paths[@]}"; do
      printf '  stale: %s\n' "$stale_path" >&2
    done
    exit 1
  fi
  if test "$live_count" -eq 1; then
    live_path="${live_paths[0]}"
    if test "$stale_count" -gt 0; then
      printf 'Warning: ignoring %s stale controller registration(s) for %s while using the one live worktree.\n' "$stale_count" "$control_branch" >&2
      for stale_path in "${stale_paths[@]}"; do
        printf '  stale: %s\n' "$stale_path" >&2
      done
    fi
    actual_branch="$(git -C "$live_path" branch --show-current)"
    if test "$actual_branch" != "$control_branch"; then
      printf 'Control worktree branch mismatch: expected %s, got %s\n' "$control_branch" "$actual_branch" >&2
      exit 1
    fi
    live_status="$(git -C "$live_path" status --porcelain=v1 --untracked-files=all)"
    if test -n "$live_status"; then
      printf 'Blocked: live controller worktree is dirty and cannot be refreshed safely: %s\n' "$live_path" >&2
      git -C "$live_path" status --short --branch --untracked-files=all >&2
      exit 1
    fi
    live_head="$(git -C "$live_path" rev-parse HEAD)"
    if test "$live_head" = "$remote_head"; then
      :
    elif git -C "$repo" merge-base --is-ancestor "$live_head" "$remote_head"; then
      if ! git -C "$live_path" merge --ff-only "$remote_branch_ref"; then
        printf 'Blocked: live controller worktree could not fast-forward %s to fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
        exit 1
      fi
    elif git -C "$repo" merge-base --is-ancestor "$remote_head" "$live_head"; then
      printf 'Blocked: live controller branch %s is ahead of fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
      exit 1
    else
      printf 'Blocked: live controller branch %s diverged from fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
      exit 1
    fi
    live_head="$(git -C "$live_path" rev-parse HEAD)"
    if test "$live_head" != "$remote_head"; then
      printf 'Blocked: live controller HEAD %s does not match fetched origin tip %s.\n' "$live_head" "$remote_head" >&2
      exit 1
    fi
    printf '%s\n' "$live_path"
    return 0
  fi
  if test "$stale_count" -gt 1; then
    printf 'Blocked: %s has %s stale controller registrations and no live worktree; refusing to choose arbitrarily.\n' "$control_branch" "$stale_count" >&2
    for stale_path in "${stale_paths[@]}"; do
      printf '  stale: %s\n' "$stale_path" >&2
    done
    exit 1
  fi
  if ! git -C "$repo" check-ignore -q .worktrees/; then
    printf 'Repository-local .worktrees/ is not ignored.\n' >&2
    exit 1
  fi
  if ! git -C "$repo" check-ignore -q .worktrees/issue-541-recovery; then
    printf 'Repository-local control worktree path is not ignored: %s\n' '.worktrees/issue-541-recovery' >&2
    exit 1
  fi
  mkdir -p "$repo/.worktrees"
  if test -e "$target_path"; then
    printf 'Control worktree path already exists without a matching live registration: %s\n' "$target_path" >&2
    exit 1
  fi
  if git -C "$repo" show-ref --verify --quiet "$expected_branch"; then
    local_head="$(git -C "$repo" rev-parse --verify "$expected_branch^{commit}")"
    if test "$local_head" = "$remote_head"; then
      :
    elif git -C "$repo" merge-base --is-ancestor "$local_head" "$remote_head"; then
      if ! git -C "$repo" update-ref "$expected_branch" "$remote_head" "$local_head"; then
        printf 'Blocked: local controller branch %s could not be fast-forwarded to fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
        exit 1
      fi
    elif git -C "$repo" merge-base --is-ancestor "$remote_head" "$local_head"; then
      printf 'Blocked: local controller branch %s is ahead of fetched origin tip %s and cannot be rewritten safely.\n' "$control_branch" "$remote_head" >&2
      exit 1
    else
      printf 'Blocked: local controller branch %s diverged from fetched origin tip %s and cannot be rewritten safely.\n' "$control_branch" "$remote_head" >&2
      exit 1
    fi
  else
    if ! git -C "$repo" update-ref "$expected_branch" "$remote_head"; then
      printf 'Blocked: local controller branch %s could not be created at fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
      exit 1
    fi
  fi
  local_head="$(git -C "$repo" rev-parse --verify "$expected_branch^{commit}")"
  if test "$local_head" != "$remote_head"; then
    printf 'Blocked: local controller branch %s resolved to %s instead of fetched origin tip %s.\n' "$control_branch" "$local_head" "$remote_head" >&2
    exit 1
  fi
  if test "$stale_count" -eq 1; then
    stale_path="${stale_paths[0]}"
    printf 'Recreating %s at %s from the one stale controller registration:\n  stale: %s\n' "$control_branch" "$target_path" "$stale_path" >&2
    git -C "$repo" worktree add --force "$target_path" "$control_branch"
  else
    git -C "$repo" worktree add "$target_path" "$control_branch"
  fi
  if ! test -d "$target_path"; then
    printf 'Control worktree path is not present after resolution: %s\n' "$target_path" >&2
    exit 1
  fi
  actual_branch="$(git -C "$target_path" branch --show-current)"
  if test "$actual_branch" != "$control_branch"; then
    printf 'Control worktree branch mismatch: expected %s, got %s\n' "$control_branch" "$actual_branch" >&2
    exit 1
  fi
  local_head="$(git -C "$target_path" rev-parse HEAD)"
  if test "$local_head" != "$remote_head"; then
    printf 'Blocked: resolved controller HEAD %s does not match fetched origin tip %s.\n' "$local_head" "$remote_head" >&2
    exit 1
  fi
  printf '%s\n' "$target_path"
}
CONTROL_WORKTREE="$(resolve_control_worktree)"
COMMON_DIR="$(git -C "$CONTROL_WORKTREE" rev-parse --git-common-dir)"
REPO="$(cd "$COMMON_DIR/.." && pwd)"
SOURCE="$REPO/.worktrees/pr-476-review"
DEST="$COMMON_DIR/agent-recovery/issue-541/dirty/pr-476-review"
BRANCH="$(git -C "$SOURCE" branch --show-current)"
mkdir -p "$DEST"
git -C "$SOURCE" status --short --branch > "$DEST/status.txt"
printf '%s\n' "$BRANCH" > "$DEST/branch.txt"
git -C "$SOURCE" rev-parse HEAD > "$DEST/head.txt"
git -C "$SOURCE" merge-base HEAD origin/main > "$DEST/merge-base.txt"
git -C "$REPO" bundle create "$DEST/branch.bundle" "$BRANCH"
git -C "$REPO" bundle verify "$DEST/branch.bundle"
git -C "$SOURCE" diff --binary > "$DEST/worktree.patch"
git -C "$SOURCE" diff --cached --binary > "$DEST/index.patch"
git -C "$SOURCE" ls-files --others --exclude-standard -z > "$DEST/.untracked.zlist"
python3 - "$DEST/.untracked.zlist" > "$DEST/untracked.json" <<'PY'
import json
import sys
from pathlib import Path

raw = Path(sys.argv[1]).read_bytes()
entries = [] if not raw else raw.rstrip(b"\0").split(b"\0")
print(json.dumps([entry.decode("utf-8", "surrogateescape") for entry in entries], indent=2))
PY
tar -C "$SOURCE" --null -T "$DEST/.untracked.zlist" -cf "$DEST/untracked.tar"
git -C "$SOURCE" ls-files --others --ignored --exclude-standard -z > "$DEST/.ignored.zlist"
python3 - "$DEST/.ignored.zlist" > "$DEST/ignored.json" <<'PY'
import json
import sys
from pathlib import Path

raw = Path(sys.argv[1]).read_bytes()
entries = [] if not raw else raw.rstrip(b"\0").split(b"\0")
print(json.dumps([entry.decode("utf-8", "surrogateescape") for entry in entries], indent=2))
PY
printf 'pr-476-review\tdirty-worktree\t%s\t%s\t%s\t%s\n' \
  "$SOURCE" \
  "$(cat "$DEST/head.txt")" \
  "$(cat "$DEST/merge-base.txt")" \
  "$DEST" >> "$COMMON_DIR/agent-recovery/issue-541/manifest.tsv"
```

Expected: `branch.bundle` verifies against `fix/476-review-threads`,
preserving committed branch history before any later branch deletion, and the
untracked inventory plus `untracked.tar` contain all three runtime parity plan
files without flattening special pathnames or symlink metadata. Any ignored
paths remain captured only in `.ignored.zlist` and `ignored.json`.

- [ ] **Step 6: Verify snapshot completeness**

Run:

```bash
set -euo pipefail
resolve_control_worktree() {
  local control_branch="docs/541-incomplete-work-recovery-design"
  local expected_branch="refs/heads/$control_branch"
  local remote_branch_ref="refs/remotes/origin/$control_branch"
  local start_common_dir repo target_path path branch_ref actual_branch
  local target_registration_branch live_count stale_count live_path stale_path
  local live_status live_head local_head remote_head
  local -a live_paths=() stale_paths=()
  start_common_dir="$(git rev-parse --git-common-dir)"
  repo="$(cd "$start_common_dir/.." && pwd)"
  target_path="$repo/.worktrees/issue-541-recovery"
  if ! git -C "$repo" fetch --no-tags origin \
    "refs/heads/$control_branch:$remote_branch_ref"
  then
    printf 'Blocked: could not fetch origin/%s into %s.\n' \
      "$control_branch" "$remote_branch_ref" >&2
    exit 1
  fi
  remote_head="$(git -C "$repo" rev-parse --verify "$remote_branch_ref^{commit}")"
  while IFS="$(printf "\t")" read -r path branch_ref; do
    test -n "$path" || continue
    if test "$path" = "$target_path"; then
      target_registration_branch="$branch_ref"
    fi
    if test "$branch_ref" != "$expected_branch"; then
      continue
    fi
    if test -d "$path"; then
      live_paths+=("$path")
    else
      stale_paths+=("$path")
    fi
  done <<EOF2
$(git -C "$repo" worktree list --porcelain | awk '
  $1 == "worktree" { if (path != "") print path "\t" branch; path = substr($0, 10); branch = ""; next }
  $1 == "branch" { branch = $2; next }
  END { if (path != "") print path "\t" branch }
')
EOF2
  live_count="${#live_paths[@]}"
  stale_count="${#stale_paths[@]}"
  if test -n "$target_registration_branch" && test "$target_registration_branch" != "$expected_branch"; then
    printf 'Blocked: %s is registered to %s, not %s.\n' "$target_path" "$target_registration_branch" "$control_branch" >&2
    exit 1
  fi
  if test "$live_count" -gt 1; then
    printf 'Blocked: %s has %s live controller worktree registrations; refusing to choose arbitrarily.\n' "$control_branch" "$live_count" >&2
    for live_path in "${live_paths[@]}"; do
      printf '  live: %s\n' "$live_path" >&2
    done
    for stale_path in "${stale_paths[@]}"; do
      printf '  stale: %s\n' "$stale_path" >&2
    done
    exit 1
  fi
  if test "$live_count" -eq 1; then
    live_path="${live_paths[0]}"
    if test "$stale_count" -gt 0; then
      printf 'Warning: ignoring %s stale controller registration(s) for %s while using the one live worktree.\n' "$stale_count" "$control_branch" >&2
      for stale_path in "${stale_paths[@]}"; do
        printf '  stale: %s\n' "$stale_path" >&2
      done
    fi
    actual_branch="$(git -C "$live_path" branch --show-current)"
    if test "$actual_branch" != "$control_branch"; then
      printf 'Control worktree branch mismatch: expected %s, got %s\n' "$control_branch" "$actual_branch" >&2
      exit 1
    fi
    live_status="$(git -C "$live_path" status --porcelain=v1 --untracked-files=all)"
    if test -n "$live_status"; then
      printf 'Blocked: live controller worktree is dirty and cannot be refreshed safely: %s\n' "$live_path" >&2
      git -C "$live_path" status --short --branch --untracked-files=all >&2
      exit 1
    fi
    live_head="$(git -C "$live_path" rev-parse HEAD)"
    if test "$live_head" = "$remote_head"; then
      :
    elif git -C "$repo" merge-base --is-ancestor "$live_head" "$remote_head"; then
      if ! git -C "$live_path" merge --ff-only "$remote_branch_ref"; then
        printf 'Blocked: live controller worktree could not fast-forward %s to fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
        exit 1
      fi
    elif git -C "$repo" merge-base --is-ancestor "$remote_head" "$live_head"; then
      printf 'Blocked: live controller branch %s is ahead of fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
      exit 1
    else
      printf 'Blocked: live controller branch %s diverged from fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
      exit 1
    fi
    live_head="$(git -C "$live_path" rev-parse HEAD)"
    if test "$live_head" != "$remote_head"; then
      printf 'Blocked: live controller HEAD %s does not match fetched origin tip %s.\n' "$live_head" "$remote_head" >&2
      exit 1
    fi
    printf '%s\n' "$live_path"
    return 0
  fi
  if test "$stale_count" -gt 1; then
    printf 'Blocked: %s has %s stale controller registrations and no live worktree; refusing to choose arbitrarily.\n' "$control_branch" "$stale_count" >&2
    for stale_path in "${stale_paths[@]}"; do
      printf '  stale: %s\n' "$stale_path" >&2
    done
    exit 1
  fi
  if ! git -C "$repo" check-ignore -q .worktrees/; then
    printf 'Repository-local .worktrees/ is not ignored.\n' >&2
    exit 1
  fi
  if ! git -C "$repo" check-ignore -q .worktrees/issue-541-recovery; then
    printf 'Repository-local control worktree path is not ignored: %s\n' '.worktrees/issue-541-recovery' >&2
    exit 1
  fi
  mkdir -p "$repo/.worktrees"
  if test -e "$target_path"; then
    printf 'Control worktree path already exists without a matching live registration: %s\n' "$target_path" >&2
    exit 1
  fi
  if git -C "$repo" show-ref --verify --quiet "$expected_branch"; then
    local_head="$(git -C "$repo" rev-parse --verify "$expected_branch^{commit}")"
    if test "$local_head" = "$remote_head"; then
      :
    elif git -C "$repo" merge-base --is-ancestor "$local_head" "$remote_head"; then
      if ! git -C "$repo" update-ref "$expected_branch" "$remote_head" "$local_head"; then
        printf 'Blocked: local controller branch %s could not be fast-forwarded to fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
        exit 1
      fi
    elif git -C "$repo" merge-base --is-ancestor "$remote_head" "$local_head"; then
      printf 'Blocked: local controller branch %s is ahead of fetched origin tip %s and cannot be rewritten safely.\n' "$control_branch" "$remote_head" >&2
      exit 1
    else
      printf 'Blocked: local controller branch %s diverged from fetched origin tip %s and cannot be rewritten safely.\n' "$control_branch" "$remote_head" >&2
      exit 1
    fi
  else
    if ! git -C "$repo" update-ref "$expected_branch" "$remote_head"; then
      printf 'Blocked: local controller branch %s could not be created at fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
      exit 1
    fi
  fi
  local_head="$(git -C "$repo" rev-parse --verify "$expected_branch^{commit}")"
  if test "$local_head" != "$remote_head"; then
    printf 'Blocked: local controller branch %s resolved to %s instead of fetched origin tip %s.\n' "$control_branch" "$local_head" "$remote_head" >&2
    exit 1
  fi
  if test "$stale_count" -eq 1; then
    stale_path="${stale_paths[0]}"
    printf 'Recreating %s at %s from the one stale controller registration:\n  stale: %s\n' "$control_branch" "$target_path" "$stale_path" >&2
    git -C "$repo" worktree add --force "$target_path" "$control_branch"
  else
    git -C "$repo" worktree add "$target_path" "$control_branch"
  fi
  if ! test -d "$target_path"; then
    printf 'Control worktree path is not present after resolution: %s\n' "$target_path" >&2
    exit 1
  fi
  actual_branch="$(git -C "$target_path" branch --show-current)"
  if test "$actual_branch" != "$control_branch"; then
    printf 'Control worktree branch mismatch: expected %s, got %s\n' "$control_branch" "$actual_branch" >&2
    exit 1
  fi
  local_head="$(git -C "$target_path" rev-parse HEAD)"
  if test "$local_head" != "$remote_head"; then
    printf 'Blocked: resolved controller HEAD %s does not match fetched origin tip %s.\n' "$local_head" "$remote_head" >&2
    exit 1
  fi
  printf '%s\n' "$target_path"
}
CONTROL_WORKTREE="$(resolve_control_worktree)"
COMMON_DIR="$(git -C "$CONTROL_WORKTREE" rev-parse --git-common-dir)"
REPO="$(cd "$COMMON_DIR/.." && pwd)"
RECOVERY="$COMMON_DIR/agent-recovery/issue-541"
test "$(wc -l < "$RECOVERY/manifest.tsv" | tr -d ' ')" = 5
for id in docs-psyche-specs memory-promote mobile-memory-gateway pr-476-review; do
  SNAPSHOT="$RECOVERY/dirty/$id"
  case "$id" in
    docs-psyche-specs)
      SOURCE="$REPO/.worktrees/docs-psyche-specs"
      ;;
    memory-promote)
      SOURCE="$REPO/.worktrees/feat-cmem-1ev-memory-promote"
      ;;
    mobile-memory-gateway)
      SOURCE="$REPO/.worktrees/mobile-memory-gateway"
      ;;
    pr-476-review)
      SOURCE="$REPO/.worktrees/pr-476-review"
      ;;
  esac
  test -s "$SNAPSHOT/status.txt"
  test -s "$SNAPSHOT/branch.txt"
  test -s "$SNAPSHOT/head.txt"
  test -s "$SNAPSHOT/merge-base.txt"
  test -s "$SNAPSHOT/branch.bundle"
  test -f "$SNAPSHOT/worktree.patch"
  test -f "$SNAPSHOT/index.patch"
  test -f "$SNAPSHOT/.untracked.zlist"
  test -s "$SNAPSHOT/untracked.json"
  test -f "$SNAPSHOT/untracked.tar"
  test -f "$SNAPSHOT/.ignored.zlist"
  test -s "$SNAPSHOT/ignored.json"
  python3 - "$SNAPSHOT" "$SOURCE" <<'PY'
import json
import os
import stat
import sys
import tarfile
from pathlib import Path

snapshot = Path(sys.argv[1])
source = Path(sys.argv[2])

def read_zlist(path: Path) -> list[str]:
    raw = path.read_bytes()
    if not raw:
        return []
    return [entry.decode("utf-8", "surrogateescape") for entry in raw.rstrip(b"\0").split(b"\0")]

untracked = read_zlist(snapshot / ".untracked.zlist")
if json.loads((snapshot / "untracked.json").read_text()) != untracked:
    raise SystemExit("untracked.json does not match .untracked.zlist")

with tarfile.open(snapshot / "untracked.tar") as tf:
    members = tf.getmembers()
    if [member.name for member in members] != untracked:
        raise SystemExit("untracked.tar members do not match .untracked.zlist")
    for member in members:
        path = source / member.name
        st = os.lstat(path)
        if member.issym() != stat.S_ISLNK(st.st_mode):
            raise SystemExit(f"type mismatch for {member.name}")
        if stat.S_IMODE(st.st_mode) != stat.S_IMODE(member.mode):
            raise SystemExit(f"mode mismatch for {member.name}")
        if member.isfile() and st.st_size != member.size:
            raise SystemExit(f"size mismatch for {member.name}")
        if member.issym() and os.readlink(path) != member.linkname:
            raise SystemExit(f"symlink target mismatch for {member.name}")

ignored = read_zlist(snapshot / ".ignored.zlist")
if json.loads((snapshot / "ignored.json").read_text()) != ignored:
    raise SystemExit("ignored.json does not match .ignored.zlist")
PY
  git -C "$REPO" bundle verify "$SNAPSHOT/branch.bundle" > /dev/null
done
```

Expected: every dirty snapshot includes status, branch, head, merge-base, both
patches, private NUL-delimited untracked and ignored inventories, JSON-escaped
readable inventory files, an uncompressed `untracked.tar`, and a verified
`branch.bundle`, so committed branch history plus lossless untracked content
and metadata are preserved before any later branch deletion. Empty worktree and
index patches remain valid zero-byte files, and empty untracked or ignored
inventories remain valid when `.untracked.zlist` or `.ignored.zlist` is empty,
their JSON companions are `[]`, and `untracked.tar` is a readable empty
archive.

### Task 3: Archive Every Orphan Branch

**Artifacts:**
- Create: `.git/agent-recovery/issue-541/branches/docs-universal-runtime-capability-design.bundle`
- Create: `.git/agent-recovery/issue-541/branches/feat-npm-macos-x64.bundle`
- Create: `.git/agent-recovery/issue-541/branches/fix-521-ward-surface-confinement.bundle`
- Create: `.git/agent-recovery/issue-541/branches/docs-universal-runtime-capability-design/branch.txt`
- Create: `.git/agent-recovery/issue-541/branches/docs-universal-runtime-capability-design/head.txt`
- Create: `.git/agent-recovery/issue-541/branches/docs-universal-runtime-capability-design/merge-base.txt`
- Create: `.git/agent-recovery/issue-541/branches/feat-npm-macos-x64/branch.txt`
- Create: `.git/agent-recovery/issue-541/branches/feat-npm-macos-x64/head.txt`
- Create: `.git/agent-recovery/issue-541/branches/feat-npm-macos-x64/merge-base.txt`
- Create: `.git/agent-recovery/issue-541/branches/fix-521-ward-surface-confinement/branch.txt`
- Create: `.git/agent-recovery/issue-541/branches/fix-521-ward-surface-confinement/head.txt`
- Create: `.git/agent-recovery/issue-541/branches/fix-521-ward-surface-confinement/merge-base.txt`

- [ ] **Step 1: Create ref-preserving bundles**

Run:

```bash
set -euo pipefail
resolve_control_worktree() {
  local control_branch="docs/541-incomplete-work-recovery-design"
  local expected_branch="refs/heads/$control_branch"
  local remote_branch_ref="refs/remotes/origin/$control_branch"
  local start_common_dir repo target_path path branch_ref actual_branch
  local target_registration_branch live_count stale_count live_path stale_path
  local live_status live_head local_head remote_head
  local -a live_paths=() stale_paths=()
  start_common_dir="$(git rev-parse --git-common-dir)"
  repo="$(cd "$start_common_dir/.." && pwd)"
  target_path="$repo/.worktrees/issue-541-recovery"
  if ! git -C "$repo" fetch --no-tags origin \
    "refs/heads/$control_branch:$remote_branch_ref"
  then
    printf 'Blocked: could not fetch origin/%s into %s.\n' \
      "$control_branch" "$remote_branch_ref" >&2
    exit 1
  fi
  remote_head="$(git -C "$repo" rev-parse --verify "$remote_branch_ref^{commit}")"
  while IFS="$(printf "\t")" read -r path branch_ref; do
    test -n "$path" || continue
    if test "$path" = "$target_path"; then
      target_registration_branch="$branch_ref"
    fi
    if test "$branch_ref" != "$expected_branch"; then
      continue
    fi
    if test -d "$path"; then
      live_paths+=("$path")
    else
      stale_paths+=("$path")
    fi
  done <<EOF2
$(git -C "$repo" worktree list --porcelain | awk '
  $1 == "worktree" { if (path != "") print path "\t" branch; path = substr($0, 10); branch = ""; next }
  $1 == "branch" { branch = $2; next }
  END { if (path != "") print path "\t" branch }
')
EOF2
  live_count="${#live_paths[@]}"
  stale_count="${#stale_paths[@]}"
  if test -n "$target_registration_branch" && test "$target_registration_branch" != "$expected_branch"; then
    printf 'Blocked: %s is registered to %s, not %s.\n' "$target_path" "$target_registration_branch" "$control_branch" >&2
    exit 1
  fi
  if test "$live_count" -gt 1; then
    printf 'Blocked: %s has %s live controller worktree registrations; refusing to choose arbitrarily.\n' "$control_branch" "$live_count" >&2
    for live_path in "${live_paths[@]}"; do
      printf '  live: %s\n' "$live_path" >&2
    done
    for stale_path in "${stale_paths[@]}"; do
      printf '  stale: %s\n' "$stale_path" >&2
    done
    exit 1
  fi
  if test "$live_count" -eq 1; then
    live_path="${live_paths[0]}"
    if test "$stale_count" -gt 0; then
      printf 'Warning: ignoring %s stale controller registration(s) for %s while using the one live worktree.\n' "$stale_count" "$control_branch" >&2
      for stale_path in "${stale_paths[@]}"; do
        printf '  stale: %s\n' "$stale_path" >&2
      done
    fi
    actual_branch="$(git -C "$live_path" branch --show-current)"
    if test "$actual_branch" != "$control_branch"; then
      printf 'Control worktree branch mismatch: expected %s, got %s\n' "$control_branch" "$actual_branch" >&2
      exit 1
    fi
    live_status="$(git -C "$live_path" status --porcelain=v1 --untracked-files=all)"
    if test -n "$live_status"; then
      printf 'Blocked: live controller worktree is dirty and cannot be refreshed safely: %s\n' "$live_path" >&2
      git -C "$live_path" status --short --branch --untracked-files=all >&2
      exit 1
    fi
    live_head="$(git -C "$live_path" rev-parse HEAD)"
    if test "$live_head" = "$remote_head"; then
      :
    elif git -C "$repo" merge-base --is-ancestor "$live_head" "$remote_head"; then
      if ! git -C "$live_path" merge --ff-only "$remote_branch_ref"; then
        printf 'Blocked: live controller worktree could not fast-forward %s to fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
        exit 1
      fi
    elif git -C "$repo" merge-base --is-ancestor "$remote_head" "$live_head"; then
      printf 'Blocked: live controller branch %s is ahead of fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
      exit 1
    else
      printf 'Blocked: live controller branch %s diverged from fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
      exit 1
    fi
    live_head="$(git -C "$live_path" rev-parse HEAD)"
    if test "$live_head" != "$remote_head"; then
      printf 'Blocked: live controller HEAD %s does not match fetched origin tip %s.\n' "$live_head" "$remote_head" >&2
      exit 1
    fi
    printf '%s\n' "$live_path"
    return 0
  fi
  if test "$stale_count" -gt 1; then
    printf 'Blocked: %s has %s stale controller registrations and no live worktree; refusing to choose arbitrarily.\n' "$control_branch" "$stale_count" >&2
    for stale_path in "${stale_paths[@]}"; do
      printf '  stale: %s\n' "$stale_path" >&2
    done
    exit 1
  fi
  if ! git -C "$repo" check-ignore -q .worktrees/; then
    printf 'Repository-local .worktrees/ is not ignored.\n' >&2
    exit 1
  fi
  if ! git -C "$repo" check-ignore -q .worktrees/issue-541-recovery; then
    printf 'Repository-local control worktree path is not ignored: %s\n' '.worktrees/issue-541-recovery' >&2
    exit 1
  fi
  mkdir -p "$repo/.worktrees"
  if test -e "$target_path"; then
    printf 'Control worktree path already exists without a matching live registration: %s\n' "$target_path" >&2
    exit 1
  fi
  if git -C "$repo" show-ref --verify --quiet "$expected_branch"; then
    local_head="$(git -C "$repo" rev-parse --verify "$expected_branch^{commit}")"
    if test "$local_head" = "$remote_head"; then
      :
    elif git -C "$repo" merge-base --is-ancestor "$local_head" "$remote_head"; then
      if ! git -C "$repo" update-ref "$expected_branch" "$remote_head" "$local_head"; then
        printf 'Blocked: local controller branch %s could not be fast-forwarded to fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
        exit 1
      fi
    elif git -C "$repo" merge-base --is-ancestor "$remote_head" "$local_head"; then
      printf 'Blocked: local controller branch %s is ahead of fetched origin tip %s and cannot be rewritten safely.\n' "$control_branch" "$remote_head" >&2
      exit 1
    else
      printf 'Blocked: local controller branch %s diverged from fetched origin tip %s and cannot be rewritten safely.\n' "$control_branch" "$remote_head" >&2
      exit 1
    fi
  else
    if ! git -C "$repo" update-ref "$expected_branch" "$remote_head"; then
      printf 'Blocked: local controller branch %s could not be created at fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
      exit 1
    fi
  fi
  local_head="$(git -C "$repo" rev-parse --verify "$expected_branch^{commit}")"
  if test "$local_head" != "$remote_head"; then
    printf 'Blocked: local controller branch %s resolved to %s instead of fetched origin tip %s.\n' "$control_branch" "$local_head" "$remote_head" >&2
    exit 1
  fi
  if test "$stale_count" -eq 1; then
    stale_path="${stale_paths[0]}"
    printf 'Recreating %s at %s from the one stale controller registration:\n  stale: %s\n' "$control_branch" "$target_path" "$stale_path" >&2
    git -C "$repo" worktree add --force "$target_path" "$control_branch"
  else
    git -C "$repo" worktree add "$target_path" "$control_branch"
  fi
  if ! test -d "$target_path"; then
    printf 'Control worktree path is not present after resolution: %s\n' "$target_path" >&2
    exit 1
  fi
  actual_branch="$(git -C "$target_path" branch --show-current)"
  if test "$actual_branch" != "$control_branch"; then
    printf 'Control worktree branch mismatch: expected %s, got %s\n' "$control_branch" "$actual_branch" >&2
    exit 1
  fi
  local_head="$(git -C "$target_path" rev-parse HEAD)"
  if test "$local_head" != "$remote_head"; then
    printf 'Blocked: resolved controller HEAD %s does not match fetched origin tip %s.\n' "$local_head" "$remote_head" >&2
    exit 1
  fi
  printf '%s\n' "$target_path"
}
CONTROL_WORKTREE="$(resolve_control_worktree)"
COMMON_DIR="$(git -C "$CONTROL_WORKTREE" rev-parse --git-common-dir)"
REPO="$(cd "$COMMON_DIR/.." && pwd)"
RECOVERY="$COMMON_DIR/agent-recovery/issue-541"
mkdir -p \
  "$RECOVERY/branches/docs-universal-runtime-capability-design" \
  "$RECOVERY/branches/feat-npm-macos-x64" \
  "$RECOVERY/branches/fix-521-ward-surface-confinement"
git -C "$REPO" bundle create \
  "$RECOVERY/branches/docs-universal-runtime-capability-design.bundle" \
  docs/universal-runtime-capability-design
git -C "$REPO" bundle create \
  "$RECOVERY/branches/feat-npm-macos-x64.bundle" \
  feat/npm-macos-x64
git -C "$REPO" bundle create \
  "$RECOVERY/branches/fix-521-ward-surface-confinement.bundle" \
  fix/521-ward-surface-confinement
```

Expected: three bundle files and three metadata directories are created.

- [ ] **Step 2: Verify each bundle**

Run:

```bash
set -euo pipefail
resolve_control_worktree() {
  local control_branch="docs/541-incomplete-work-recovery-design"
  local expected_branch="refs/heads/$control_branch"
  local remote_branch_ref="refs/remotes/origin/$control_branch"
  local start_common_dir repo target_path path branch_ref actual_branch
  local target_registration_branch live_count stale_count live_path stale_path
  local live_status live_head local_head remote_head
  local -a live_paths=() stale_paths=()
  start_common_dir="$(git rev-parse --git-common-dir)"
  repo="$(cd "$start_common_dir/.." && pwd)"
  target_path="$repo/.worktrees/issue-541-recovery"
  if ! git -C "$repo" fetch --no-tags origin \
    "refs/heads/$control_branch:$remote_branch_ref"
  then
    printf 'Blocked: could not fetch origin/%s into %s.\n' \
      "$control_branch" "$remote_branch_ref" >&2
    exit 1
  fi
  remote_head="$(git -C "$repo" rev-parse --verify "$remote_branch_ref^{commit}")"
  while IFS="$(printf "\t")" read -r path branch_ref; do
    test -n "$path" || continue
    if test "$path" = "$target_path"; then
      target_registration_branch="$branch_ref"
    fi
    if test "$branch_ref" != "$expected_branch"; then
      continue
    fi
    if test -d "$path"; then
      live_paths+=("$path")
    else
      stale_paths+=("$path")
    fi
  done <<EOF2
$(git -C "$repo" worktree list --porcelain | awk '
  $1 == "worktree" { if (path != "") print path "\t" branch; path = substr($0, 10); branch = ""; next }
  $1 == "branch" { branch = $2; next }
  END { if (path != "") print path "\t" branch }
')
EOF2
  live_count="${#live_paths[@]}"
  stale_count="${#stale_paths[@]}"
  if test -n "$target_registration_branch" && test "$target_registration_branch" != "$expected_branch"; then
    printf 'Blocked: %s is registered to %s, not %s.\n' "$target_path" "$target_registration_branch" "$control_branch" >&2
    exit 1
  fi
  if test "$live_count" -gt 1; then
    printf 'Blocked: %s has %s live controller worktree registrations; refusing to choose arbitrarily.\n' "$control_branch" "$live_count" >&2
    for live_path in "${live_paths[@]}"; do
      printf '  live: %s\n' "$live_path" >&2
    done
    for stale_path in "${stale_paths[@]}"; do
      printf '  stale: %s\n' "$stale_path" >&2
    done
    exit 1
  fi
  if test "$live_count" -eq 1; then
    live_path="${live_paths[0]}"
    if test "$stale_count" -gt 0; then
      printf 'Warning: ignoring %s stale controller registration(s) for %s while using the one live worktree.\n' "$stale_count" "$control_branch" >&2
      for stale_path in "${stale_paths[@]}"; do
        printf '  stale: %s\n' "$stale_path" >&2
      done
    fi
    actual_branch="$(git -C "$live_path" branch --show-current)"
    if test "$actual_branch" != "$control_branch"; then
      printf 'Control worktree branch mismatch: expected %s, got %s\n' "$control_branch" "$actual_branch" >&2
      exit 1
    fi
    live_status="$(git -C "$live_path" status --porcelain=v1 --untracked-files=all)"
    if test -n "$live_status"; then
      printf 'Blocked: live controller worktree is dirty and cannot be refreshed safely: %s\n' "$live_path" >&2
      git -C "$live_path" status --short --branch --untracked-files=all >&2
      exit 1
    fi
    live_head="$(git -C "$live_path" rev-parse HEAD)"
    if test "$live_head" = "$remote_head"; then
      :
    elif git -C "$repo" merge-base --is-ancestor "$live_head" "$remote_head"; then
      if ! git -C "$live_path" merge --ff-only "$remote_branch_ref"; then
        printf 'Blocked: live controller worktree could not fast-forward %s to fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
        exit 1
      fi
    elif git -C "$repo" merge-base --is-ancestor "$remote_head" "$live_head"; then
      printf 'Blocked: live controller branch %s is ahead of fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
      exit 1
    else
      printf 'Blocked: live controller branch %s diverged from fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
      exit 1
    fi
    live_head="$(git -C "$live_path" rev-parse HEAD)"
    if test "$live_head" != "$remote_head"; then
      printf 'Blocked: live controller HEAD %s does not match fetched origin tip %s.\n' "$live_head" "$remote_head" >&2
      exit 1
    fi
    printf '%s\n' "$live_path"
    return 0
  fi
  if test "$stale_count" -gt 1; then
    printf 'Blocked: %s has %s stale controller registrations and no live worktree; refusing to choose arbitrarily.\n' "$control_branch" "$stale_count" >&2
    for stale_path in "${stale_paths[@]}"; do
      printf '  stale: %s\n' "$stale_path" >&2
    done
    exit 1
  fi
  if ! git -C "$repo" check-ignore -q .worktrees/; then
    printf 'Repository-local .worktrees/ is not ignored.\n' >&2
    exit 1
  fi
  if ! git -C "$repo" check-ignore -q .worktrees/issue-541-recovery; then
    printf 'Repository-local control worktree path is not ignored: %s\n' '.worktrees/issue-541-recovery' >&2
    exit 1
  fi
  mkdir -p "$repo/.worktrees"
  if test -e "$target_path"; then
    printf 'Control worktree path already exists without a matching live registration: %s\n' "$target_path" >&2
    exit 1
  fi
  if git -C "$repo" show-ref --verify --quiet "$expected_branch"; then
    local_head="$(git -C "$repo" rev-parse --verify "$expected_branch^{commit}")"
    if test "$local_head" = "$remote_head"; then
      :
    elif git -C "$repo" merge-base --is-ancestor "$local_head" "$remote_head"; then
      if ! git -C "$repo" update-ref "$expected_branch" "$remote_head" "$local_head"; then
        printf 'Blocked: local controller branch %s could not be fast-forwarded to fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
        exit 1
      fi
    elif git -C "$repo" merge-base --is-ancestor "$remote_head" "$local_head"; then
      printf 'Blocked: local controller branch %s is ahead of fetched origin tip %s and cannot be rewritten safely.\n' "$control_branch" "$remote_head" >&2
      exit 1
    else
      printf 'Blocked: local controller branch %s diverged from fetched origin tip %s and cannot be rewritten safely.\n' "$control_branch" "$remote_head" >&2
      exit 1
    fi
  else
    if ! git -C "$repo" update-ref "$expected_branch" "$remote_head"; then
      printf 'Blocked: local controller branch %s could not be created at fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
      exit 1
    fi
  fi
  local_head="$(git -C "$repo" rev-parse --verify "$expected_branch^{commit}")"
  if test "$local_head" != "$remote_head"; then
    printf 'Blocked: local controller branch %s resolved to %s instead of fetched origin tip %s.\n' "$control_branch" "$local_head" "$remote_head" >&2
    exit 1
  fi
  if test "$stale_count" -eq 1; then
    stale_path="${stale_paths[0]}"
    printf 'Recreating %s at %s from the one stale controller registration:\n  stale: %s\n' "$control_branch" "$target_path" "$stale_path" >&2
    git -C "$repo" worktree add --force "$target_path" "$control_branch"
  else
    git -C "$repo" worktree add "$target_path" "$control_branch"
  fi
  if ! test -d "$target_path"; then
    printf 'Control worktree path is not present after resolution: %s\n' "$target_path" >&2
    exit 1
  fi
  actual_branch="$(git -C "$target_path" branch --show-current)"
  if test "$actual_branch" != "$control_branch"; then
    printf 'Control worktree branch mismatch: expected %s, got %s\n' "$control_branch" "$actual_branch" >&2
    exit 1
  fi
  local_head="$(git -C "$target_path" rev-parse HEAD)"
  if test "$local_head" != "$remote_head"; then
    printf 'Blocked: resolved controller HEAD %s does not match fetched origin tip %s.\n' "$local_head" "$remote_head" >&2
    exit 1
  fi
  printf '%s\n' "$target_path"
}
CONTROL_WORKTREE="$(resolve_control_worktree)"
COMMON_DIR="$(git -C "$CONTROL_WORKTREE" rev-parse --git-common-dir)"
REPO="$(cd "$COMMON_DIR/.." && pwd)"
RECOVERY="$COMMON_DIR/agent-recovery/issue-541"
git -C "$REPO" bundle verify "$RECOVERY/branches/docs-universal-runtime-capability-design.bundle"
git -C "$REPO" bundle verify "$RECOVERY/branches/feat-npm-macos-x64.bundle"
git -C "$REPO" bundle verify "$RECOVERY/branches/fix-521-ward-surface-confinement.bundle"
```

Expected: Git reports each bundle is okay and lists its branch ref.

- [ ] **Step 3: Record the archived branches and snapshot their exact heads**

Run:

```bash
set -euo pipefail
resolve_control_worktree() {
  local control_branch="docs/541-incomplete-work-recovery-design"
  local expected_branch="refs/heads/$control_branch"
  local remote_branch_ref="refs/remotes/origin/$control_branch"
  local start_common_dir repo target_path path branch_ref actual_branch
  local target_registration_branch live_count stale_count live_path stale_path
  local live_status live_head local_head remote_head
  local -a live_paths=() stale_paths=()
  start_common_dir="$(git rev-parse --git-common-dir)"
  repo="$(cd "$start_common_dir/.." && pwd)"
  target_path="$repo/.worktrees/issue-541-recovery"
  if ! git -C "$repo" fetch --no-tags origin \
    "refs/heads/$control_branch:$remote_branch_ref"
  then
    printf 'Blocked: could not fetch origin/%s into %s.\n' \
      "$control_branch" "$remote_branch_ref" >&2
    exit 1
  fi
  remote_head="$(git -C "$repo" rev-parse --verify "$remote_branch_ref^{commit}")"
  while IFS="$(printf "\t")" read -r path branch_ref; do
    test -n "$path" || continue
    if test "$path" = "$target_path"; then
      target_registration_branch="$branch_ref"
    fi
    if test "$branch_ref" != "$expected_branch"; then
      continue
    fi
    if test -d "$path"; then
      live_paths+=("$path")
    else
      stale_paths+=("$path")
    fi
  done <<EOF2
$(git -C "$repo" worktree list --porcelain | awk '
  $1 == "worktree" { if (path != "") print path "\t" branch; path = substr($0, 10); branch = ""; next }
  $1 == "branch" { branch = $2; next }
  END { if (path != "") print path "\t" branch }
')
EOF2
  live_count="${#live_paths[@]}"
  stale_count="${#stale_paths[@]}"
  if test -n "$target_registration_branch" && test "$target_registration_branch" != "$expected_branch"; then
    printf 'Blocked: %s is registered to %s, not %s.\n' "$target_path" "$target_registration_branch" "$control_branch" >&2
    exit 1
  fi
  if test "$live_count" -gt 1; then
    printf 'Blocked: %s has %s live controller worktree registrations; refusing to choose arbitrarily.\n' "$control_branch" "$live_count" >&2
    for live_path in "${live_paths[@]}"; do
      printf '  live: %s\n' "$live_path" >&2
    done
    for stale_path in "${stale_paths[@]}"; do
      printf '  stale: %s\n' "$stale_path" >&2
    done
    exit 1
  fi
  if test "$live_count" -eq 1; then
    live_path="${live_paths[0]}"
    if test "$stale_count" -gt 0; then
      printf 'Warning: ignoring %s stale controller registration(s) for %s while using the one live worktree.\n' "$stale_count" "$control_branch" >&2
      for stale_path in "${stale_paths[@]}"; do
        printf '  stale: %s\n' "$stale_path" >&2
      done
    fi
    actual_branch="$(git -C "$live_path" branch --show-current)"
    if test "$actual_branch" != "$control_branch"; then
      printf 'Control worktree branch mismatch: expected %s, got %s\n' "$control_branch" "$actual_branch" >&2
      exit 1
    fi
    live_status="$(git -C "$live_path" status --porcelain=v1 --untracked-files=all)"
    if test -n "$live_status"; then
      printf 'Blocked: live controller worktree is dirty and cannot be refreshed safely: %s\n' "$live_path" >&2
      git -C "$live_path" status --short --branch --untracked-files=all >&2
      exit 1
    fi
    live_head="$(git -C "$live_path" rev-parse HEAD)"
    if test "$live_head" = "$remote_head"; then
      :
    elif git -C "$repo" merge-base --is-ancestor "$live_head" "$remote_head"; then
      if ! git -C "$live_path" merge --ff-only "$remote_branch_ref"; then
        printf 'Blocked: live controller worktree could not fast-forward %s to fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
        exit 1
      fi
    elif git -C "$repo" merge-base --is-ancestor "$remote_head" "$live_head"; then
      printf 'Blocked: live controller branch %s is ahead of fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
      exit 1
    else
      printf 'Blocked: live controller branch %s diverged from fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
      exit 1
    fi
    live_head="$(git -C "$live_path" rev-parse HEAD)"
    if test "$live_head" != "$remote_head"; then
      printf 'Blocked: live controller HEAD %s does not match fetched origin tip %s.\n' "$live_head" "$remote_head" >&2
      exit 1
    fi
    printf '%s\n' "$live_path"
    return 0
  fi
  if test "$stale_count" -gt 1; then
    printf 'Blocked: %s has %s stale controller registrations and no live worktree; refusing to choose arbitrarily.\n' "$control_branch" "$stale_count" >&2
    for stale_path in "${stale_paths[@]}"; do
      printf '  stale: %s\n' "$stale_path" >&2
    done
    exit 1
  fi
  if ! git -C "$repo" check-ignore -q .worktrees/; then
    printf 'Repository-local .worktrees/ is not ignored.\n' >&2
    exit 1
  fi
  if ! git -C "$repo" check-ignore -q .worktrees/issue-541-recovery; then
    printf 'Repository-local control worktree path is not ignored: %s\n' '.worktrees/issue-541-recovery' >&2
    exit 1
  fi
  mkdir -p "$repo/.worktrees"
  if test -e "$target_path"; then
    printf 'Control worktree path already exists without a matching live registration: %s\n' "$target_path" >&2
    exit 1
  fi
  if git -C "$repo" show-ref --verify --quiet "$expected_branch"; then
    local_head="$(git -C "$repo" rev-parse --verify "$expected_branch^{commit}")"
    if test "$local_head" = "$remote_head"; then
      :
    elif git -C "$repo" merge-base --is-ancestor "$local_head" "$remote_head"; then
      if ! git -C "$repo" update-ref "$expected_branch" "$remote_head" "$local_head"; then
        printf 'Blocked: local controller branch %s could not be fast-forwarded to fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
        exit 1
      fi
    elif git -C "$repo" merge-base --is-ancestor "$remote_head" "$local_head"; then
      printf 'Blocked: local controller branch %s is ahead of fetched origin tip %s and cannot be rewritten safely.\n' "$control_branch" "$remote_head" >&2
      exit 1
    else
      printf 'Blocked: local controller branch %s diverged from fetched origin tip %s and cannot be rewritten safely.\n' "$control_branch" "$remote_head" >&2
      exit 1
    fi
  else
    if ! git -C "$repo" update-ref "$expected_branch" "$remote_head"; then
      printf 'Blocked: local controller branch %s could not be created at fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
      exit 1
    fi
  fi
  local_head="$(git -C "$repo" rev-parse --verify "$expected_branch^{commit}")"
  if test "$local_head" != "$remote_head"; then
    printf 'Blocked: local controller branch %s resolved to %s instead of fetched origin tip %s.\n' "$control_branch" "$local_head" "$remote_head" >&2
    exit 1
  fi
  if test "$stale_count" -eq 1; then
    stale_path="${stale_paths[0]}"
    printf 'Recreating %s at %s from the one stale controller registration:\n  stale: %s\n' "$control_branch" "$target_path" "$stale_path" >&2
    git -C "$repo" worktree add --force "$target_path" "$control_branch"
  else
    git -C "$repo" worktree add "$target_path" "$control_branch"
  fi
  if ! test -d "$target_path"; then
    printf 'Control worktree path is not present after resolution: %s\n' "$target_path" >&2
    exit 1
  fi
  actual_branch="$(git -C "$target_path" branch --show-current)"
  if test "$actual_branch" != "$control_branch"; then
    printf 'Control worktree branch mismatch: expected %s, got %s\n' "$control_branch" "$actual_branch" >&2
    exit 1
  fi
  local_head="$(git -C "$target_path" rev-parse HEAD)"
  if test "$local_head" != "$remote_head"; then
    printf 'Blocked: resolved controller HEAD %s does not match fetched origin tip %s.\n' "$local_head" "$remote_head" >&2
    exit 1
  fi
  printf '%s\n' "$target_path"
}
CONTROL_WORKTREE="$(resolve_control_worktree)"
COMMON_DIR="$(git -C "$CONTROL_WORKTREE" rev-parse --git-common-dir)"
REPO="$(cd "$COMMON_DIR/.." && pwd)"
RECOVERY="$COMMON_DIR/agent-recovery/issue-541"
while IFS='|' read -r id branch preserved_head_source bundle_source; do
  preserved_head_file="$RECOVERY/$preserved_head_source"
  bundle_file="$RECOVERY/$bundle_source"
  test -s "$preserved_head_file"
  test -s "$bundle_file"
  source_head="$(cat "$preserved_head_file")"
  git -C "$REPO" cat-file -e "$source_head^{commit}"
  git -C "$REPO" bundle verify "$bundle_file" > /dev/null
  git -C "$REPO" --no-pager log --oneline --reverse "origin/main..$source_head" > "$RECOVERY/$id-commits.txt"
  git -C "$REPO" --no-pager diff --stat "origin/main...$source_head" > "$RECOVERY/$id-stat.txt"
  git -C "$REPO" cherry -v origin/main "$source_head" > "$RECOVERY/$id-cherry.txt"
done <<'EOF'
docs-psyche-specs|docs/psyche-specs|dirty/docs-psyche-specs/head.txt|dirty/docs-psyche-specs/branch.bundle
docs-universal-runtime-capability-design|docs/universal-runtime-capability-design|branches/docs-universal-runtime-capability-design/head.txt|branches/docs-universal-runtime-capability-design.bundle
memory-promote|feat/cmem-1ev-memory-promote|dirty/memory-promote/head.txt|dirty/memory-promote/branch.bundle
mobile-memory-gateway|feat/mobile-memory-gateway|dirty/mobile-memory-gateway/head.txt|dirty/mobile-memory-gateway/branch.bundle
feat-npm-macos-x64|feat/npm-macos-x64|branches/feat-npm-macos-x64/head.txt|branches/feat-npm-macos-x64.bundle
pr-476-review|fix/476-review-threads|dirty/pr-476-review/head.txt|dirty/pr-476-review/branch.bundle
fix-521-ward-surface-confinement|fix/521-ward-surface-confinement|branches/fix-521-ward-surface-confinement/head.txt|branches/fix-521-ward-surface-confinement.bundle
EOF
for id in \
  docs-psyche-specs \
  memory-promote \
  mobile-memory-gateway \
  pr-476-review
do
  SNAPSHOT="$RECOVERY/dirty/$id"
  if test -s "$SNAPSHOT/worktree.patch"; then
    git apply --stat --summary "$SNAPSHOT/worktree.patch" \
      > "$RECOVERY/$id-worktree-evidence.txt"
  else
    printf 'No unstaged tracked changes in snapshot.\n' \
      > "$RECOVERY/$id-worktree-evidence.txt"
  fi
  test -f "$RECOVERY/$id-worktree-evidence.txt"
  test -s "$RECOVERY/$id-worktree-evidence.txt"
  if test -s "$SNAPSHOT/index.patch"; then
    git apply --stat --summary "$SNAPSHOT/index.patch" \
      > "$RECOVERY/$id-index-evidence.txt"
  else
    printf 'No staged changes in snapshot.\n' \
      > "$RECOVERY/$id-index-evidence.txt"
  fi
  test -f "$RECOVERY/$id-index-evidence.txt"
  test -s "$RECOVERY/$id-index-evidence.txt"
  cp "$SNAPSHOT/untracked.json" "$RECOVERY/$id-untracked-evidence.json"
  test -s "$RECOVERY/$id-untracked-evidence.json"
  cp "$SNAPSHOT/ignored.json" "$RECOVERY/$id-ignored-evidence.json"
  test -s "$RECOVERY/$id-ignored-evidence.json"
done
```

Expected: every preserved-head file in the seven-row map verifies as a commit,
every paired bundle still verifies, and each `<workstream-id>-commits.txt`,
`<workstream-id>-stat.txt`, and `<workstream-id>-cherry.txt` file is generated
from that immutable preserved snapshot head SHA rather than from a mutable live
branch name. The four dirty snapshots still produce reviewable worktree, index,
untracked, and ignored evidence files, so branch evidence covers all seven
branch-backed workstreams and dirty evidence complements the four dirty rows.
Empty worktree and index snapshot classes remain valid because they emit
deterministic sentinel lines, while empty untracked or ignored inventories
remain valid because their copied JSON evidence files still contain readable
`[]`.

- [ ] **Step 3: Write the ledger with one evidence-backed row per workstream**

Create `.git/agent-recovery/issue-541/classification.md` with a five-column
table headed `Workstream`, `Classification`, `Main/PR evidence`,
`Preserved source`, and `Recovery action`. Initialize it as:

```markdown
# Issue 541 Recovery Classification

| Workstream | Classification | Main/PR evidence | Preserved source | Recovery action |
| --- | --- | --- | --- | --- |
| docs-psyche-specs | pending | Pending Task 4 Step 2 evidence review. | dirty/docs-psyche-specs | Pending Task 4 Step 4 classification. |
| memory-promote | pending | Pending Task 4 Step 2 evidence review. | dirty/memory-promote | Pending Task 4 Step 4 classification. |
| mobile-memory-gateway | pending | Pending Task 4 Step 2 evidence review. | dirty/mobile-memory-gateway | Pending Task 4 Step 4 classification. |
| pr-476-review | pending | Pending Task 4 Step 2 evidence review. | dirty/pr-476-review | Pending Task 4 Step 4 classification. |
| docs-universal-runtime-capability-design | pending | Pending Task 4 Step 2 evidence review. | branches/docs-universal-runtime-capability-design.bundle | Pending Task 4 Step 4 classification. |
| feat-npm-macos-x64 | pending | Pending Task 4 Step 2 evidence review. | branches/feat-npm-macos-x64.bundle | Pending Task 4 Step 4 classification. |
| fix-521-ward-surface-confinement | pending | Pending Task 4 Step 2 evidence review. | branches/fix-521-ward-surface-confinement.bundle | Pending Task 4 Step 4 classification. |
```

Write the selected classification, concrete commit/PR/issue/path evidence,
sanitized preserved-source archive ID, and deterministic recovery action
directly into each row. Use `Preserved source` values such as
`dirty/docs-psyche-specs`, `dirty/memory-promote`, or
`branches/feat-npm-macos-x64.bundle`; never write expanded `$REPO`,
`$COMMON_DIR`, `/tmp`, `/private`, `/Users`, or `/home` paths into
`classification.md`. Local absolute paths belong only in `manifest.tsv` and
private snapshot artifacts. A row is not complete until another engineer can
reproduce its classification from the cited source.

For `already-shipped` and `superseded` rows, use a machine-verifiable
non-viable contract:

- `Main/PR evidence` must be exactly one of these:
  - the raw canonical merged PR URL for `OpenCoven/coven`, for example
    `https://github.com/OpenCoven/coven/pull/542`; or
  - the exact 40-character commit SHA that is already reachable from current
    `main`.
- `Recovery action` must use semicolon-delimited `key=value` fields with one
  of these deterministic forms:
  - `mode=non-viable-proof; classification=already-shipped; evidence_kind=merged-pr`
  - `mode=non-viable-proof; classification=already-shipped; evidence_kind=main-commit`
  - `mode=non-viable-proof; classification=superseded; evidence_kind=merged-pr`
  - `mode=non-viable-proof; classification=superseded; evidence_kind=main-commit`
- Optional narrative may remain only as extra keys such as `note=...`, but
  cleanup authorization comes solely from the exact `Main/PR evidence` value
  plus the parsed `mode`, `classification`, and `evidence_kind` fields.

For viable rows, keep the column contract strict and deterministic:

- `Main/PR evidence` may contain narrative non-terminal evidence only until a
  pull request is actually adopted or opened. Once Task 5 adopts an existing
  source-branch PR, adopts an existing recovery-branch PR on rerun, or opens a
  new recovery PR, rewrite `Main/PR evidence` to the raw canonical PR URL
  only, for example `https://github.com/OpenCoven/coven/pull/542`.
- `Recovery action` must use semicolon-delimited `key=value` fields rather
  than prose so retirement can parse it exactly. Use these modes only:
  - `mode=continue-existing-pr; pr_kind=adopted; issue_url=...; archive_id=...; expected_branch=...; preserved_head=...`
  - `mode=continue-existing-pr; pr_kind=recovered; issue_url=...; archive_id=...; expected_branch=...; expected_head=...`
  - `mode=awaiting-recovery-pr; pr_kind=recovered; issue_url=...; archive_id=...; expected_branch=...`
  - `mode=recovery-pr-open; pr_kind=recovered; issue_url=...; archive_id=...; expected_branch=...; expected_head=...`
- Terminal recovered-row actions must record `expected_head` as the exact
  `origin/$RECOVERY_BRANCH` tip fetched immediately before PR verification so
  retirement can detect later force-pushes or divergence exactly.
- A viable row that has only reached issue reuse/creation remains explicitly
  non-terminal with `mode=awaiting-recovery-pr`; it must not look retired or
  PR-complete until the later PR-writing step rewrites both columns.

- [ ] **Step 4: Apply the classification rules**

Use these deterministic rules:

```text
already-shipped:
  Current main contains equivalent behavior or documentation, proven by either
  one merged `OpenCoven/coven` PR URL or one exact main-ancestor commit SHA.

superseded:
  A later merged `OpenCoven/coven` PR or exact main-ancestor commit
  intentionally replaces the same contract, and applying the old work would
  regress or duplicate it.

viable:
  The work's user-visible or authority-preserving intent is absent from current
  main, remains consistent with current policy, and has a testable acceptance
  boundary.

blocked:
  The work requires a maintainer decision, external authority, missing source,
  or an unresolved contract choice that cannot be inferred safely.
```

Expected: Task 4 Step 4 replaces every `pending` row with exactly one terminal
classification (`already-shipped`, `superseded`, `viable`, or `blocked`) and
one next action before Task 4 Step 5 and Task 9 Step 1 verification.
`already-shipped` and `superseded` rows are terminal only when `Main/PR
evidence` is a raw merged PR URL or exact 40-character main commit SHA and
`Recovery action` carries `mode=non-viable-proof` metadata matching that
classification. A viable row may later keep that classification while Task 5
first records `mode=awaiting-recovery-pr` after verified issue reuse/create,
then rewrites the row to a terminal PR-backed state only after one of these
outcomes:

- `mode=continue-existing-pr; pr_kind=adopted` after a fully paginated
  same-repo open-PR capture returns one exact-source-branch candidate and that
  single candidate passes the fetched-tip, preserved-head ancestry, and
  empty-dirty-snapshot checks;
- `mode=continue-existing-pr; pr_kind=recovered` after a fully paginated
  same-repo open-PR capture returns one exact recovery-branch candidate for
  `issue-<n>-<slug>` on rerun, fetches that exact `origin/<branch>` tip
  immediately before verification, and proves the candidate is the OPEN `main`
  PR in `OpenCoven/coven` whose `headRefOid` matches that fetched tip; or
- `mode=recovery-pr-open` after the zero-candidate issue flow creates and
  verifies a new OPEN recovery PR from the expected recovery branch only after
  fetching the exact `origin/<branch>` tip and proving `headRefOid` matches it.

Rows with zero same-repo exact-head candidates keep the normal
issue-reuse-or-create flow, and any `mode=awaiting-recovery-pr` row remains
non-terminal until the later PR update rewrites `Main/PR evidence` to the raw
canonical PR URL.

- [ ] **Step 5: Review the ledger against the manifest**

Run:

```bash
set -euo pipefail
resolve_control_worktree() {
  local control_branch="docs/541-incomplete-work-recovery-design"
  local expected_branch="refs/heads/$control_branch"
  local remote_branch_ref="refs/remotes/origin/$control_branch"
  local start_common_dir repo target_path path branch_ref actual_branch
  local target_registration_branch live_count stale_count live_path stale_path
  local live_status live_head local_head remote_head
  local -a live_paths=() stale_paths=()
  start_common_dir="$(git rev-parse --git-common-dir)"
  repo="$(cd "$start_common_dir/.." && pwd)"
  target_path="$repo/.worktrees/issue-541-recovery"
  if ! git -C "$repo" fetch --no-tags origin \
    "refs/heads/$control_branch:$remote_branch_ref"
  then
    printf 'Blocked: could not fetch origin/%s into %s.\n' \
      "$control_branch" "$remote_branch_ref" >&2
    exit 1
  fi
  remote_head="$(git -C "$repo" rev-parse --verify "$remote_branch_ref^{commit}")"
  while IFS="$(printf "\t")" read -r path branch_ref; do
    test -n "$path" || continue
    if test "$path" = "$target_path"; then
      target_registration_branch="$branch_ref"
    fi
    if test "$branch_ref" != "$expected_branch"; then
      continue
    fi
    if test -d "$path"; then
      live_paths+=("$path")
    else
      stale_paths+=("$path")
    fi
  done <<EOF2
$(git -C "$repo" worktree list --porcelain | awk '
  $1 == "worktree" { if (path != "") print path "\t" branch; path = substr($0, 10); branch = ""; next }
  $1 == "branch" { branch = $2; next }
  END { if (path != "") print path "\t" branch }
')
EOF2
  live_count="${#live_paths[@]}"
  stale_count="${#stale_paths[@]}"
  if test -n "$target_registration_branch" && test "$target_registration_branch" != "$expected_branch"; then
    printf 'Blocked: %s is registered to %s, not %s.\n' "$target_path" "$target_registration_branch" "$control_branch" >&2
    exit 1
  fi
  if test "$live_count" -gt 1; then
    printf 'Blocked: %s has %s live controller worktree registrations; refusing to choose arbitrarily.\n' "$control_branch" "$live_count" >&2
    for live_path in "${live_paths[@]}"; do
      printf '  live: %s\n' "$live_path" >&2
    done
    for stale_path in "${stale_paths[@]}"; do
      printf '  stale: %s\n' "$stale_path" >&2
    done
    exit 1
  fi
  if test "$live_count" -eq 1; then
    live_path="${live_paths[0]}"
    if test "$stale_count" -gt 0; then
      printf 'Warning: ignoring %s stale controller registration(s) for %s while using the one live worktree.\n' "$stale_count" "$control_branch" >&2
      for stale_path in "${stale_paths[@]}"; do
        printf '  stale: %s\n' "$stale_path" >&2
      done
    fi
    actual_branch="$(git -C "$live_path" branch --show-current)"
    if test "$actual_branch" != "$control_branch"; then
      printf 'Control worktree branch mismatch: expected %s, got %s\n' "$control_branch" "$actual_branch" >&2
      exit 1
    fi
    live_status="$(git -C "$live_path" status --porcelain=v1 --untracked-files=all)"
    if test -n "$live_status"; then
      printf 'Blocked: live controller worktree is dirty and cannot be refreshed safely: %s\n' "$live_path" >&2
      git -C "$live_path" status --short --branch --untracked-files=all >&2
      exit 1
    fi
    live_head="$(git -C "$live_path" rev-parse HEAD)"
    if test "$live_head" = "$remote_head"; then
      :
    elif git -C "$repo" merge-base --is-ancestor "$live_head" "$remote_head"; then
      if ! git -C "$live_path" merge --ff-only "$remote_branch_ref"; then
        printf 'Blocked: live controller worktree could not fast-forward %s to fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
        exit 1
      fi
    elif git -C "$repo" merge-base --is-ancestor "$remote_head" "$live_head"; then
      printf 'Blocked: live controller branch %s is ahead of fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
      exit 1
    else
      printf 'Blocked: live controller branch %s diverged from fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
      exit 1
    fi
    live_head="$(git -C "$live_path" rev-parse HEAD)"
    if test "$live_head" != "$remote_head"; then
      printf 'Blocked: live controller HEAD %s does not match fetched origin tip %s.\n' "$live_head" "$remote_head" >&2
      exit 1
    fi
    printf '%s\n' "$live_path"
    return 0
  fi
  if test "$stale_count" -gt 1; then
    printf 'Blocked: %s has %s stale controller registrations and no live worktree; refusing to choose arbitrarily.\n' "$control_branch" "$stale_count" >&2
    for stale_path in "${stale_paths[@]}"; do
      printf '  stale: %s\n' "$stale_path" >&2
    done
    exit 1
  fi
  if ! git -C "$repo" check-ignore -q .worktrees/; then
    printf 'Repository-local .worktrees/ is not ignored.\n' >&2
    exit 1
  fi
  if ! git -C "$repo" check-ignore -q .worktrees/issue-541-recovery; then
    printf 'Repository-local control worktree path is not ignored: %s\n' '.worktrees/issue-541-recovery' >&2
    exit 1
  fi
  mkdir -p "$repo/.worktrees"
  if test -e "$target_path"; then
    printf 'Control worktree path already exists without a matching live registration: %s\n' "$target_path" >&2
    exit 1
  fi
  if git -C "$repo" show-ref --verify --quiet "$expected_branch"; then
    local_head="$(git -C "$repo" rev-parse --verify "$expected_branch^{commit}")"
    if test "$local_head" = "$remote_head"; then
      :
    elif git -C "$repo" merge-base --is-ancestor "$local_head" "$remote_head"; then
      if ! git -C "$repo" update-ref "$expected_branch" "$remote_head" "$local_head"; then
        printf 'Blocked: local controller branch %s could not be fast-forwarded to fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
        exit 1
      fi
    elif git -C "$repo" merge-base --is-ancestor "$remote_head" "$local_head"; then
      printf 'Blocked: local controller branch %s is ahead of fetched origin tip %s and cannot be rewritten safely.\n' "$control_branch" "$remote_head" >&2
      exit 1
    else
      printf 'Blocked: local controller branch %s diverged from fetched origin tip %s and cannot be rewritten safely.\n' "$control_branch" "$remote_head" >&2
      exit 1
    fi
  else
    if ! git -C "$repo" update-ref "$expected_branch" "$remote_head"; then
      printf 'Blocked: local controller branch %s could not be created at fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
      exit 1
    fi
  fi
  local_head="$(git -C "$repo" rev-parse --verify "$expected_branch^{commit}")"
  if test "$local_head" != "$remote_head"; then
    printf 'Blocked: local controller branch %s resolved to %s instead of fetched origin tip %s.\n' "$control_branch" "$local_head" "$remote_head" >&2
    exit 1
  fi
  if test "$stale_count" -eq 1; then
    stale_path="${stale_paths[0]}"
    printf 'Recreating %s at %s from the one stale controller registration:\n  stale: %s\n' "$control_branch" "$target_path" "$stale_path" >&2
    git -C "$repo" worktree add --force "$target_path" "$control_branch"
  else
    git -C "$repo" worktree add "$target_path" "$control_branch"
  fi
  if ! test -d "$target_path"; then
    printf 'Control worktree path is not present after resolution: %s\n' "$target_path" >&2
    exit 1
  fi
  actual_branch="$(git -C "$target_path" branch --show-current)"
  if test "$actual_branch" != "$control_branch"; then
    printf 'Control worktree branch mismatch: expected %s, got %s\n' "$control_branch" "$actual_branch" >&2
    exit 1
  fi
  local_head="$(git -C "$target_path" rev-parse HEAD)"
  if test "$local_head" != "$remote_head"; then
    printf 'Blocked: resolved controller HEAD %s does not match fetched origin tip %s.\n' "$local_head" "$remote_head" >&2
    exit 1
  fi
  printf '%s\n' "$target_path"
}
CONTROL_WORKTREE="$(resolve_control_worktree)"
COMMON_DIR="$(git -C "$CONTROL_WORKTREE" rev-parse --git-common-dir)"
REPO="$(cd "$COMMON_DIR/.." && pwd)"
RECOVERY="$COMMON_DIR/agent-recovery/issue-541"
for id in \
  docs-psyche-specs \
  memory-promote \
  mobile-memory-gateway \
  pr-476-review \
  docs-universal-runtime-capability-design \
  feat-npm-macos-x64 \
  fix-521-ward-surface-confinement
do
  grep -F "| $id |" "$RECOVERY/classification.md"
  grep -F "$id" "$RECOVERY/manifest.tsv"
done
```

Expected: every workstream appears in both files.

### Task 5: Create One Recovery Track per Viable Workstream

**Files:**
- Create only for rows classified `viable`:
  - The exact design and plan paths listed in the File and Artifact Map.

- [ ] **Step 1: Check for an adoptable exact-source-branch open pull request before creating anything**

Set `WORKSTREAM_ID` to the viable row's exact workstream ID, then map it to its
current source branch:

```bash
set -euo pipefail
resolve_control_worktree() {
  local control_branch="docs/541-incomplete-work-recovery-design"
  local expected_branch="refs/heads/$control_branch"
  local remote_branch_ref="refs/remotes/origin/$control_branch"
  local start_common_dir repo target_path path branch_ref actual_branch
  local target_registration_branch live_count stale_count live_path stale_path
  local live_status live_head local_head remote_head
  local -a live_paths=() stale_paths=()
  start_common_dir="$(git rev-parse --git-common-dir)"
  repo="$(cd "$start_common_dir/.." && pwd)"
  target_path="$repo/.worktrees/issue-541-recovery"
  if ! git -C "$repo" fetch --no-tags origin \
    "refs/heads/$control_branch:$remote_branch_ref"
  then
    printf 'Blocked: could not fetch origin/%s into %s.\n' \
      "$control_branch" "$remote_branch_ref" >&2
    exit 1
  fi
  remote_head="$(git -C "$repo" rev-parse --verify "$remote_branch_ref^{commit}")"
  while IFS="$(printf "\t")" read -r path branch_ref; do
    test -n "$path" || continue
    if test "$path" = "$target_path"; then
      target_registration_branch="$branch_ref"
    fi
    if test "$branch_ref" != "$expected_branch"; then
      continue
    fi
    if test -d "$path"; then
      live_paths+=("$path")
    else
      stale_paths+=("$path")
    fi
  done <<EOF2
$(git -C "$repo" worktree list --porcelain | awk '
  $1 == "worktree" { if (path != "") print path "\t" branch; path = substr($0, 10); branch = ""; next }
  $1 == "branch" { branch = $2; next }
  END { if (path != "") print path "\t" branch }
')
EOF2
  live_count="${#live_paths[@]}"
  stale_count="${#stale_paths[@]}"
  if test -n "$target_registration_branch" && test "$target_registration_branch" != "$expected_branch"; then
    printf 'Blocked: %s is registered to %s, not %s.\n' "$target_path" "$target_registration_branch" "$control_branch" >&2
    exit 1
  fi
  if test "$live_count" -gt 1; then
    printf 'Blocked: %s has %s live controller worktree registrations; refusing to choose arbitrarily.\n' "$control_branch" "$live_count" >&2
    for live_path in "${live_paths[@]}"; do
      printf '  live: %s\n' "$live_path" >&2
    done
    for stale_path in "${stale_paths[@]}"; do
      printf '  stale: %s\n' "$stale_path" >&2
    done
    exit 1
  fi
  if test "$live_count" -eq 1; then
    live_path="${live_paths[0]}"
    if test "$stale_count" -gt 0; then
      printf 'Warning: ignoring %s stale controller registration(s) for %s while using the one live worktree.\n' "$stale_count" "$control_branch" >&2
      for stale_path in "${stale_paths[@]}"; do
        printf '  stale: %s\n' "$stale_path" >&2
      done
    fi
    actual_branch="$(git -C "$live_path" branch --show-current)"
    if test "$actual_branch" != "$control_branch"; then
      printf 'Control worktree branch mismatch: expected %s, got %s\n' "$control_branch" "$actual_branch" >&2
      exit 1
    fi
    live_status="$(git -C "$live_path" status --porcelain=v1 --untracked-files=all)"
    if test -n "$live_status"; then
      printf 'Blocked: live controller worktree is dirty and cannot be refreshed safely: %s\n' "$live_path" >&2
      git -C "$live_path" status --short --branch --untracked-files=all >&2
      exit 1
    fi
    live_head="$(git -C "$live_path" rev-parse HEAD)"
    if test "$live_head" = "$remote_head"; then
      :
    elif git -C "$repo" merge-base --is-ancestor "$live_head" "$remote_head"; then
      if ! git -C "$live_path" merge --ff-only "$remote_branch_ref"; then
        printf 'Blocked: live controller worktree could not fast-forward %s to fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
        exit 1
      fi
    elif git -C "$repo" merge-base --is-ancestor "$remote_head" "$live_head"; then
      printf 'Blocked: live controller branch %s is ahead of fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
      exit 1
    else
      printf 'Blocked: live controller branch %s diverged from fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
      exit 1
    fi
    live_head="$(git -C "$live_path" rev-parse HEAD)"
    if test "$live_head" != "$remote_head"; then
      printf 'Blocked: live controller HEAD %s does not match fetched origin tip %s.\n' "$live_head" "$remote_head" >&2
      exit 1
    fi
    printf '%s\n' "$live_path"
    return 0
  fi
  if test "$stale_count" -gt 1; then
    printf 'Blocked: %s has %s stale controller registrations and no live worktree; refusing to choose arbitrarily.\n' "$control_branch" "$stale_count" >&2
    for stale_path in "${stale_paths[@]}"; do
      printf '  stale: %s\n' "$stale_path" >&2
    done
    exit 1
  fi
  if ! git -C "$repo" check-ignore -q .worktrees/; then
    printf 'Repository-local .worktrees/ is not ignored.\n' >&2
    exit 1
  fi
  if ! git -C "$repo" check-ignore -q .worktrees/issue-541-recovery; then
    printf 'Repository-local control worktree path is not ignored: %s\n' '.worktrees/issue-541-recovery' >&2
    exit 1
  fi
  mkdir -p "$repo/.worktrees"
  if test -e "$target_path"; then
    printf 'Control worktree path already exists without a matching live registration: %s\n' "$target_path" >&2
    exit 1
  fi
  if git -C "$repo" show-ref --verify --quiet "$expected_branch"; then
    local_head="$(git -C "$repo" rev-parse --verify "$expected_branch^{commit}")"
    if test "$local_head" = "$remote_head"; then
      :
    elif git -C "$repo" merge-base --is-ancestor "$local_head" "$remote_head"; then
      if ! git -C "$repo" update-ref "$expected_branch" "$remote_head" "$local_head"; then
        printf 'Blocked: local controller branch %s could not be fast-forwarded to fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
        exit 1
      fi
    elif git -C "$repo" merge-base --is-ancestor "$remote_head" "$local_head"; then
      printf 'Blocked: local controller branch %s is ahead of fetched origin tip %s and cannot be rewritten safely.\n' "$control_branch" "$remote_head" >&2
      exit 1
    else
      printf 'Blocked: local controller branch %s diverged from fetched origin tip %s and cannot be rewritten safely.\n' "$control_branch" "$remote_head" >&2
      exit 1
    fi
  else
    if ! git -C "$repo" update-ref "$expected_branch" "$remote_head"; then
      printf 'Blocked: local controller branch %s could not be created at fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
      exit 1
    fi
  fi
  local_head="$(git -C "$repo" rev-parse --verify "$expected_branch^{commit}")"
  if test "$local_head" != "$remote_head"; then
    printf 'Blocked: local controller branch %s resolved to %s instead of fetched origin tip %s.\n' "$control_branch" "$local_head" "$remote_head" >&2
    exit 1
  fi
  if test "$stale_count" -eq 1; then
    stale_path="${stale_paths[0]}"
    printf 'Recreating %s at %s from the one stale controller registration:\n  stale: %s\n' "$control_branch" "$target_path" "$stale_path" >&2
    git -C "$repo" worktree add --force "$target_path" "$control_branch"
  else
    git -C "$repo" worktree add "$target_path" "$control_branch"
  fi
  if ! test -d "$target_path"; then
    printf 'Control worktree path is not present after resolution: %s\n' "$target_path" >&2
    exit 1
  fi
  actual_branch="$(git -C "$target_path" branch --show-current)"
  if test "$actual_branch" != "$control_branch"; then
    printf 'Control worktree branch mismatch: expected %s, got %s\n' "$control_branch" "$actual_branch" >&2
    exit 1
  fi
  local_head="$(git -C "$target_path" rev-parse HEAD)"
  if test "$local_head" != "$remote_head"; then
    printf 'Blocked: resolved controller HEAD %s does not match fetched origin tip %s.\n' "$local_head" "$remote_head" >&2
    exit 1
  fi
  printf '%s\n' "$target_path"
}
CONTROL_WORKTREE="$(resolve_control_worktree)"
COMMON_DIR="$(git -C "$CONTROL_WORKTREE" rev-parse --git-common-dir)"
REPO="$(cd "$COMMON_DIR/.." && pwd)"
RECOVERY="$COMMON_DIR/agent-recovery/issue-541/viable"
CLASSIFICATION="$COMMON_DIR/agent-recovery/issue-541/classification.md"
mkdir -p "$RECOVERY"
update_classification_row() {
  python3 - "$CLASSIFICATION" "$WORKSTREAM_ID" "$1" "$2" "$3" <<'PY'
from pathlib import Path
import sys

path = Path(sys.argv[1])
workstream, classification, evidence, action = sys.argv[2:6]
needle = f"| {workstream} |"
lines = path.read_text().splitlines()
for idx, line in enumerate(lines):
    if line.startswith(needle):
        parts = [part.strip() for part in line.strip().strip("|").split("|")]
        if len(parts) != 5:
            raise SystemExit(f"Unexpected classification row: {line}")
        parts[1] = classification
        parts[2] = evidence
        parts[4] = action
        lines[idx] = "| " + " | ".join(parts) + " |"
        path.write_text("\n".join(lines) + "\n")
        break
else:
    raise SystemExit(f"Missing classification row for {workstream}")
PY
}
case "$WORKSTREAM_ID" in
  mobile-memory-gateway)
    SOURCE_BRANCH="feat/mobile-memory-gateway"
    SOURCE_HEAD_FILE="$COMMON_DIR/agent-recovery/issue-541/dirty/mobile-memory-gateway/head.txt"
    ARCHIVE_ID="dirty/mobile-memory-gateway"
    DIRTY_SNAPSHOT_ID="dirty/mobile-memory-gateway"
    DIRTY_SNAPSHOT_ROOT="$COMMON_DIR/agent-recovery/issue-541/$DIRTY_SNAPSHOT_ID"
    ;;
  feat-npm-macos-x64)
    SOURCE_BRANCH="feat/npm-macos-x64"
    SOURCE_HEAD_FILE="$COMMON_DIR/agent-recovery/issue-541/branches/feat-npm-macos-x64/head.txt"
    ARCHIVE_ID="branches/feat-npm-macos-x64.bundle"
    DIRTY_SNAPSHOT_ID=""
    DIRTY_SNAPSHOT_ROOT=""
    ;;
  fix-521-ward-surface-confinement)
    SOURCE_BRANCH="fix/521-ward-surface-confinement"
    SOURCE_HEAD_FILE="$COMMON_DIR/agent-recovery/issue-541/branches/fix-521-ward-surface-confinement/head.txt"
    ARCHIVE_ID="branches/fix-521-ward-surface-confinement.bundle"
    DIRTY_SNAPSHOT_ID=""
    DIRTY_SNAPSHOT_ROOT=""
    ;;
  memory-promote)
    SOURCE_BRANCH="feat/cmem-1ev-memory-promote"
    SOURCE_HEAD_FILE="$COMMON_DIR/agent-recovery/issue-541/dirty/memory-promote/head.txt"
    ARCHIVE_ID="dirty/memory-promote"
    DIRTY_SNAPSHOT_ID="dirty/memory-promote"
    DIRTY_SNAPSHOT_ROOT="$COMMON_DIR/agent-recovery/issue-541/$DIRTY_SNAPSHOT_ID"
    ;;
  docs-psyche-specs)
    SOURCE_BRANCH="docs/psyche-specs"
    SOURCE_HEAD_FILE="$COMMON_DIR/agent-recovery/issue-541/dirty/docs-psyche-specs/head.txt"
    ARCHIVE_ID="dirty/docs-psyche-specs"
    DIRTY_SNAPSHOT_ID="dirty/docs-psyche-specs"
    DIRTY_SNAPSHOT_ROOT="$COMMON_DIR/agent-recovery/issue-541/$DIRTY_SNAPSHOT_ID"
    ;;
  docs-universal-runtime-capability-design)
    SOURCE_BRANCH="docs/universal-runtime-capability-design"
    SOURCE_HEAD_FILE="$COMMON_DIR/agent-recovery/issue-541/branches/docs-universal-runtime-capability-design/head.txt"
    ARCHIVE_ID="branches/docs-universal-runtime-capability-design.bundle"
    DIRTY_SNAPSHOT_ID=""
    DIRTY_SNAPSHOT_ROOT=""
    ;;
  pr-476-review)
    SOURCE_BRANCH="fix/476-review-threads"
    SOURCE_HEAD_FILE="$COMMON_DIR/agent-recovery/issue-541/dirty/pr-476-review/head.txt"
    ARCHIVE_ID="dirty/pr-476-review"
    DIRTY_SNAPSHOT_ID="dirty/pr-476-review"
    DIRTY_SNAPSHOT_ROOT="$COMMON_DIR/agent-recovery/issue-541/$DIRTY_SNAPSHOT_ID"
    ;;
  *)
    printf 'Unknown WORKSTREAM_ID: %s\n' "$WORKSTREAM_ID" >&2
    exit 1
    ;;
esac
BRANCH_FETCH_EVIDENCE="$RECOVERY/$WORKSTREAM_ID-branch-fetch.txt"
OPEN_PR_EVIDENCE="$RECOVERY/$WORKSTREAM_ID-open-prs.json"
PR_VIEW_EVIDENCE="$RECOVERY/$WORKSTREAM_ID-pr-view.json"
PR_ADOPTION_EVIDENCE="$RECOVERY/$WORKSTREAM_ID-pr-adoption.txt"
PR_BLOCKER_EVIDENCE="$RECOVERY/$WORKSTREAM_ID-pr-blocker.txt"
SOURCE_ISSUE_URL="https://github.com/OpenCoven/coven/issues/541"
test -s "$SOURCE_HEAD_FILE"
EXPECTED_HEAD="$(tr -d '\n' < "$SOURCE_HEAD_FILE")"
{
  printf 'Source branch: %s\n' "$SOURCE_BRANCH"
  printf 'Preserved local head: %s\n' "$EXPECTED_HEAD"
  printf 'Candidate discovery: capture every open PR for OpenCoven/coven via paginated REST API, then filter with jq only when head.ref matches the source branch and head.repo.full_name equals OpenCoven/coven.\n'
  printf 'Same-named fork PRs do not count as same-repository candidates.\n'
  printf 'Candidate discovery runs before any source-branch fetch.\n'
} > "$BRANCH_FETCH_EVIDENCE"
if ! {
  gh api --paginate --slurp \
    "repos/OpenCoven/coven/pulls?state=open&per_page=100" | \
  jq --arg SOURCE_BRANCH "$SOURCE_BRANCH" '
    [ .[] | .[]
      | select(.head.ref == $SOURCE_BRANCH)
      | select(.head.repo != null and .head.repo.full_name == "OpenCoven/coven")
      | {
          number,
          title,
          url: .html_url,
          state,
          headRefName: .head.ref,
          headRefOid: .head.sha,
          headRepoFullName: .head.repo.full_name,
          baseRefName: .base.ref
        }
    ]
  ' > "$OPEN_PR_EVIDENCE"
} 2> "$PR_BLOCKER_EVIDENCE"; then
  API_ERROR="$(cat "$PR_BLOCKER_EVIDENCE")"
  {
    printf 'Blocked %s: open-PR candidate capture failed before source-branch verification.\n' "$WORKSTREAM_ID"
    printf 'Expected source branch: %s\n' "$SOURCE_BRANCH"
    printf 'Preserved local head: %s\n' "$EXPECTED_HEAD"
    printf 'Candidate discovery uses repos/OpenCoven/coven/pulls?state=open with --paginate --slurp and jq same-repo filtering.\n'
    printf 'Failure follows:\n%s\n' "$API_ERROR"
  } > "$PR_BLOCKER_EVIDENCE"
  update_classification_row \
    "blocked" \
    "Open PR candidate capture failed for preserved head $EXPECTED_HEAD before any source-branch verification; see viable/$WORKSTREAM_ID-branch-fetch.txt and viable/$WORKSTREAM_ID-pr-blocker.txt." \
    "Blocked: could not capture same-repo open PR candidates from GitHub."
  exit 0
fi
PR_COUNT="$(jq 'length' "$OPEN_PR_EVIDENCE")"
case "$PR_COUNT" in
  1)
    printf 'Same-repo exact-head open PR count: 1\n' >> "$BRANCH_FETCH_EVIDENCE"
    printf 'Fetching origin/%s for candidate identity verification.\n' \
      "$SOURCE_BRANCH" >> "$BRANCH_FETCH_EVIDENCE"
    if ! git -C "$REPO" fetch --no-tags origin \
      "refs/heads/$SOURCE_BRANCH:refs/remotes/origin/$SOURCE_BRANCH" \
      >> "$BRANCH_FETCH_EVIDENCE" 2>&1; then
      FETCH_OUTPUT="$(cat "$BRANCH_FETCH_EVIDENCE")"
      {
        printf 'Blocked %s: same-repo exact-head candidate exists, but origin/%s could not be fetched for identity verification.\n' \
          "$WORKSTREAM_ID" "$SOURCE_BRANCH"
        printf 'Preserved local head: %s\n' "$EXPECTED_HEAD"
        printf 'Branch fetch evidence: viable/%s-branch-fetch.txt\n' "$WORKSTREAM_ID"
        printf 'Fetch output follows:\n%s\n' "$FETCH_OUTPUT"
      } > "$PR_BLOCKER_EVIDENCE"
      update_classification_row \
        "blocked" \
        "Single same-repo PR candidate could not be verified because origin/$SOURCE_BRANCH fetch failed for preserved head $EXPECTED_HEAD; see viable/$WORKSTREAM_ID-branch-fetch.txt, viable/$WORKSTREAM_ID-open-prs.json, and viable/$WORKSTREAM_ID-pr-blocker.txt." \
        "Blocked: candidate PR source branch could not be fetched from origin."
      exit 0
    fi
    FRESH_BRANCH_TIP="$(git -C "$REPO" rev-parse "refs/remotes/origin/$SOURCE_BRANCH")"
    printf 'Fresh fetched origin/%s tip: %s\n' \
      "$SOURCE_BRANCH" "$FRESH_BRANCH_TIP" >> "$BRANCH_FETCH_EVIDENCE"
    PR_NUMBER="$(jq -r '.[0].number' "$OPEN_PR_EVIDENCE")"
    if ! gh pr view --repo OpenCoven/coven "$PR_NUMBER" \
      --json number,title,url,state,headRefOid,headRefName,headRepositoryOwner,isCrossRepository,baseRefName \
      > "$PR_VIEW_EVIDENCE" 2> "$PR_BLOCKER_EVIDENCE"; then
      PR_VIEW_ERROR="$(cat "$PR_BLOCKER_EVIDENCE")"
      {
        printf 'Blocked %s: candidate PR #%s disappeared or could not be read between paginated gh api capture and gh pr view.\n' \
          "$WORKSTREAM_ID" "$PR_NUMBER"
        printf 'Expected source branch: %s\n' "$SOURCE_BRANCH"
        printf 'Preserved local head: %s\n' "$EXPECTED_HEAD"
        printf 'Fresh fetched origin/%s tip: %s\n' "$SOURCE_BRANCH" "$FRESH_BRANCH_TIP"
        printf 'Branch fetch evidence: viable/%s-branch-fetch.txt\n' "$WORKSTREAM_ID"
        printf 'Open PR evidence: viable/%s-open-prs.json\n' "$WORKSTREAM_ID"
        printf 'gh pr view failure follows:\n%s\n' "$PR_VIEW_ERROR"
      } > "$PR_BLOCKER_EVIDENCE"
      update_classification_row \
        "blocked" \
        "PR view failed after fetching origin/$SOURCE_BRANCH tip $FRESH_BRANCH_TIP for preserved head $EXPECTED_HEAD; see viable/$WORKSTREAM_ID-branch-fetch.txt, viable/$WORKSTREAM_ID-open-prs.json, and viable/$WORKSTREAM_ID-pr-blocker.txt." \
        "Blocked: candidate PR disappeared or could not be read before identity verification."
      exit 0
    fi
    ACTUAL_STATE="$(jq -r '.state' "$PR_VIEW_EVIDENCE")"
    ACTUAL_HEAD="$(jq -r '.headRefOid' "$PR_VIEW_EVIDENCE")"
    ACTUAL_BRANCH="$(jq -r '.headRefName' "$PR_VIEW_EVIDENCE")"
    ACTUAL_OWNER="$(jq -r '.headRepositoryOwner.login' "$PR_VIEW_EVIDENCE")"
    ACTUAL_CROSS="$(jq -r '.isCrossRepository' "$PR_VIEW_EVIDENCE")"
    ACTUAL_BASE="$(jq -r '.baseRefName' "$PR_VIEW_EVIDENCE")"
    if [ "$ACTUAL_HEAD" = "$FRESH_BRANCH_TIP" ] && \
       git -C "$REPO" merge-base --is-ancestor "$EXPECTED_HEAD" "$ACTUAL_HEAD"; then
      SNAPSHOT_ANCESTRY="ancestor-or-equal"
    elif [ "$ACTUAL_HEAD" = "$FRESH_BRANCH_TIP" ]; then
      SNAPSHOT_ANCESTRY="not-ancestor"
    else
      SNAPSHOT_ANCESTRY="not-checked-authoritative-tip-mismatch"
    fi
    {
      printf 'Source branch: %s\n' "$SOURCE_BRANCH"
      printf 'Preserved local head: %s\n' "$EXPECTED_HEAD"
      printf 'Fresh fetched origin/%s tip: %s\n' "$SOURCE_BRANCH" "$FRESH_BRANCH_TIP"
      printf 'PR head: %s\n' "$ACTUAL_HEAD"
      printf 'Preserved head ancestry to PR head: %s\n' "$SNAPSHOT_ANCESTRY"
    } > "$PR_ADOPTION_EVIDENCE"
    if [ "$ACTUAL_STATE" != "OPEN" ] || \
       [ "$ACTUAL_HEAD" != "$FRESH_BRANCH_TIP" ] || \
       [ "$ACTUAL_BRANCH" != "$SOURCE_BRANCH" ] || \
       [ "$ACTUAL_OWNER" != "OpenCoven" ] || \
       [ "$ACTUAL_CROSS" != "false" ] || \
       [ "$ACTUAL_BASE" != "main" ] || \
       [ "$SNAPSHOT_ANCESTRY" != "ancestor-or-equal" ]; then
      {
        printf 'Blocked %s: candidate PR #%s failed identity checks.\n' \
          "$WORKSTREAM_ID" "$PR_NUMBER"
        printf 'Expected state: OPEN\n'
        printf 'Preserved local head: %s\n' "$EXPECTED_HEAD"
        printf 'Expected fresh fetched origin/%s tip: %s\n' \
          "$SOURCE_BRANCH" "$FRESH_BRANCH_TIP"
        printf 'Expected source branch: %s\n' "$SOURCE_BRANCH"
        printf 'Expected owner/cross-repo: OpenCoven / false\n'
        printf 'Expected base branch: main\n'
        printf 'Actual state: %s\n' "$ACTUAL_STATE"
        printf 'PR head: %s\n' "$ACTUAL_HEAD"
        printf 'Actual branch: %s\n' "$ACTUAL_BRANCH"
        printf 'Actual owner: %s\n' "$ACTUAL_OWNER"
        printf 'Actual cross-repo: %s\n' "$ACTUAL_CROSS"
        printf 'Actual base branch: %s\n' "$ACTUAL_BASE"
        printf 'Preserved head ancestry to PR head: %s\n' "$SNAPSHOT_ANCESTRY"
        printf 'Branch fetch evidence: viable/%s-branch-fetch.txt\n' "$WORKSTREAM_ID"
        printf 'Open PR evidence: viable/%s-open-prs.json\n' "$WORKSTREAM_ID"
        printf 'PR view evidence: viable/%s-pr-view.json\n' "$WORKSTREAM_ID"
        printf 'PR adoption evidence: viable/%s-pr-adoption.txt\n' "$WORKSTREAM_ID"
      } > "$PR_BLOCKER_EVIDENCE"
      update_classification_row \
        "blocked" \
        "PR identity mismatch (preserved head $EXPECTED_HEAD, fetched tip $FRESH_BRANCH_TIP, PR head $ACTUAL_HEAD, ancestry $SNAPSHOT_ANCESTRY); see viable/$WORKSTREAM_ID-branch-fetch.txt, viable/$WORKSTREAM_ID-open-prs.json, viable/$WORKSTREAM_ID-pr-view.json, viable/$WORKSTREAM_ID-pr-adoption.txt, and viable/$WORKSTREAM_ID-pr-blocker.txt." \
        "Blocked: candidate PR failed OPEN-state/fresh-tip/branch/owner/non-cross-repo/base/ancestry validation."
      exit 0
    fi
    PR_URL="$(jq -r '.url' "$PR_VIEW_EVIDENCE")"
    printf 'Existing PR URL: %s\n' "$PR_URL" >> "$PR_ADOPTION_EVIDENCE"
    if test -n "$DIRTY_SNAPSHOT_ROOT"; then
      test -f "$DIRTY_SNAPSHOT_ROOT/worktree.patch"
      test -f "$DIRTY_SNAPSHOT_ROOT/index.patch"
      test -f "$DIRTY_SNAPSHOT_ROOT/.untracked.zlist"
      test -s "$DIRTY_SNAPSHOT_ROOT/untracked.json"
      test -f "$DIRTY_SNAPSHOT_ROOT/untracked.tar"
      test -f "$DIRTY_SNAPSHOT_ROOT/.ignored.zlist"
      test -s "$DIRTY_SNAPSHOT_ROOT/ignored.json"
      WORKTREE_BYTES="$(wc -c < "$DIRTY_SNAPSHOT_ROOT/worktree.patch" | tr -d ' ')"
      INDEX_BYTES="$(wc -c < "$DIRTY_SNAPSHOT_ROOT/index.patch" | tr -d ' ')"
      UNTRACKED_BYTES="$(wc -c < "$DIRTY_SNAPSHOT_ROOT/.untracked.zlist" | tr -d ' ')"
      IGNORED_BYTES="$(wc -c < "$DIRTY_SNAPSHOT_ROOT/.ignored.zlist" | tr -d ' ')"
      DIRTY_CLASS_LIST=""
      if [ "$WORKTREE_BYTES" -gt 0 ]; then DIRTY_CLASS_LIST="worktree.patch"; fi
      if [ "$INDEX_BYTES" -gt 0 ]; then DIRTY_CLASS_LIST="${DIRTY_CLASS_LIST:+$DIRTY_CLASS_LIST, }index.patch"; fi
      if [ "$UNTRACKED_BYTES" -gt 0 ]; then DIRTY_CLASS_LIST="${DIRTY_CLASS_LIST:+$DIRTY_CLASS_LIST, }.untracked.zlist"; fi
      {
        printf 'Preserved dirty snapshot: %s\n' "$DIRTY_SNAPSHOT_ID"
        printf 'worktree.patch bytes: %s\n' "$WORKTREE_BYTES"
        printf 'index.patch bytes: %s\n' "$INDEX_BYTES"
        printf '.untracked.zlist bytes: %s\n' "$UNTRACKED_BYTES"
        printf 'untracked.json: %s/untracked.json\n' "$DIRTY_SNAPSHOT_ID"
        printf 'untracked.tar: %s/untracked.tar\n' "$DIRTY_SNAPSHOT_ID"
        printf '.ignored.zlist bytes: %s\n' "$IGNORED_BYTES"
        printf 'ignored.json: %s/ignored.json\n' "$DIRTY_SNAPSHOT_ID"
      } >> "$PR_ADOPTION_EVIDENCE"
      if [ "$IGNORED_BYTES" -gt 0 ]; then
        printf 'Cleanup note: preserved ignored inventory is non-empty; Task 8 force-removal is prohibited for this source worktree.\n' >> "$PR_ADOPTION_EVIDENCE"
      fi
      if [ -n "$DIRTY_CLASS_LIST" ]; then
        {
          printf 'Blocked %s: existing PR #%s passed identity checks, but preserved dirty delta remains.\n' \
            "$WORKSTREAM_ID" "$PR_NUMBER"
          printf 'Existing PR URL: %s\n' "$PR_URL"
          printf 'Preserved local head: %s\n' "$EXPECTED_HEAD"
          printf 'Preserved snapshot archive IDs: %s/worktree.patch, %s/index.patch, %s/.untracked.zlist, %s/untracked.json, %s/untracked.tar\n' \
            "$DIRTY_SNAPSHOT_ID" "$DIRTY_SNAPSHOT_ID" "$DIRTY_SNAPSHOT_ID" "$DIRTY_SNAPSHOT_ID" "$DIRTY_SNAPSHOT_ID"
          printf 'Non-empty preserved dirty classes: %s\n' "$DIRTY_CLASS_LIST"
          printf 'Resume condition: land or reconcile PR #%s, then recover the preserved delta from current main in a separate scoped track.\n' "$PR_NUMBER"
          printf 'Do not force-remove or delete the original source worktree or branch while this blocker remains.\n'
          printf 'Branch fetch evidence: viable/%s-branch-fetch.txt\n' "$WORKSTREAM_ID"
          printf 'Open PR evidence: viable/%s-open-prs.json\n' "$WORKSTREAM_ID"
          printf 'PR view evidence: viable/%s-pr-view.json\n' "$WORKSTREAM_ID"
          printf 'PR adoption evidence: viable/%s-pr-adoption.txt\n' "$WORKSTREAM_ID"
        } > "$PR_BLOCKER_EVIDENCE"
        update_classification_row \
          "blocked" \
          "Existing PR #$PR_NUMBER ($PR_URL) matches preserved head $EXPECTED_HEAD, but preserved dirty snapshot $DIRTY_SNAPSHOT_ID still has non-empty classes ($DIRTY_CLASS_LIST); see viable/$WORKSTREAM_ID-branch-fetch.txt, viable/$WORKSTREAM_ID-open-prs.json, viable/$WORKSTREAM_ID-pr-view.json, viable/$WORKSTREAM_ID-pr-adoption.txt, and viable/$WORKSTREAM_ID-pr-blocker.txt." \
          "Blocked: land or reconcile PR #$PR_NUMBER, then recover preserved delta from current main in a separate scoped track. Leave source worktree and branch untouched."
        exit 0
      fi
      printf 'Dirty snapshot check: all preserved dirty classes are empty, so existing-PR adoption may continue.\n' >> "$PR_ADOPTION_EVIDENCE"
    else
      printf 'Dirty snapshot check: not applicable for branch-only preserved source.\n' >> "$PR_ADOPTION_EVIDENCE"
    fi
    update_classification_row \
      "viable" \
      "$PR_URL" \
      "mode=continue-existing-pr; pr_kind=adopted; issue_url=$SOURCE_ISSUE_URL; archive_id=$ARCHIVE_ID; expected_branch=$SOURCE_BRANCH; preserved_head=$EXPECTED_HEAD"
    exit 0
    ;;
  0)
    printf 'Same-repo exact-head open PR count: 0\n' >> "$BRANCH_FETCH_EVIDENCE"
    printf 'No same-repo exact-head open PR candidate; skipping source-branch fetch and continuing to issue reuse/create.\n' >> "$BRANCH_FETCH_EVIDENCE"
    ;;
  *)
    {
      printf 'Blocked %s: expected 0 or 1 same-repo open PRs for source branch %s, found %s before identity verification.\n' \
        "$WORKSTREAM_ID" "$SOURCE_BRANCH" "$PR_COUNT"
      printf 'Preserved local head: %s\n' "$EXPECTED_HEAD"
      printf 'No source-branch fetch was attempted because the same-repo exact-head query was already ambiguous.\n'
      printf 'Branch fetch evidence: viable/%s-branch-fetch.txt\n' "$WORKSTREAM_ID"
      printf 'Open PR evidence: viable/%s-open-prs.json\n' "$WORKSTREAM_ID"
    } > "$PR_BLOCKER_EVIDENCE"
    update_classification_row \
      "blocked" \
      "Open PR ambiguity for preserved head $EXPECTED_HEAD before any source-branch verification; see viable/$WORKSTREAM_ID-branch-fetch.txt, viable/$WORKSTREAM_ID-open-prs.json, and viable/$WORKSTREAM_ID-pr-blocker.txt." \
      "Blocked: multiple same-repo open PRs claim $SOURCE_BRANCH."
    exit 0
    ;;
esac
```

If `PR_COUNT=0`, do not fetch the source branch; record in
`viable/$WORKSTREAM_ID-branch-fetch.txt` that the same-repo exact-head query
found no candidate and continue directly to Step 2's issue reuse or creation
flow. If `PR_COUNT=1`, verify the candidate PR before adopting it: fetch the
exact expected source branch from `origin`, record the freshly fetched tip in
`viable/$WORKSTREAM_ID-branch-fetch.txt`, then require `.state=OPEN`,
`headRefName=$SOURCE_BRANCH`, `headRepositoryOwner.login=OpenCoven`,
`isCrossRepository=false`, `baseRefName=main`, and `headRefOid` equal to that
freshly fetched branch tip. Only after the PR head matches the authoritative
fetched tip may the plan compare the preserved local `head.txt` to the PR head
with `git merge-base --is-ancestor`; equality or ancestry is acceptable, but a
diverged local head blocks adoption. For dirty-worktree sources, identity and
ancestry success is still insufficient: inspect the preserved
`worktree.patch`, `index.patch`, and `.untracked.zlist`, and adopt the PR only
when all three classes are empty. The same preserved snapshot must also carry
`untracked.json`, `untracked.tar`, `.ignored.zlist`, and `ignored.json` so the
dirty-state evidence is readable and lossless. If any preserved dirty class is
non-empty, write `viable/$WORKSTREAM_ID-pr-blocker.txt` with the existing PR
URL, the snapshot archive IDs, and the safe resume condition to land or
reconcile that PR first and then recover the preserved delta from current
`main` in a separate scoped track; the original source worktree and branch stay
untouched while blocked. Record the preserved local head, fresh branch tip, PR
head, ancestry result, PR URL, and any dirty-class byte counts in
`viable/$WORKSTREAM_ID-pr-adoption.txt`, and also note there when the
preserved ignored inventory is non-empty so later cleanup knows forced removal
is prohibited. Only after all of those checks pass may the row rewrite
`Main/PR evidence` to the raw canonical PR URL and set `Recovery action` to
`mode=continue-existing-pr; pr_kind=adopted; issue_url=https://github.com/OpenCoven/coven/issues/541; archive_id=<preserved-source>; expected_branch=<source-branch>; preserved_head=<preserved-head>`.
If `PR_COUNT>1`, block immediately with open-PR evidence because the same-repo
exact-head query is already ambiguous.
If `gh pr view` fails because the candidate disappears or cannot be read after
the paginated REST capture, if the single-candidate branch fetch fails, if the
GitHub capture itself fails, or if any other identity, ancestry, or dirty
snapshot check fails, update the classification row to `blocked` and stop that
row only after the blocker evidence is persisted. This deterministically avoids
duplicating possibly delivered work while still allowing a clean local worktree
to adopt its open PR after that PR has advanced beyond the preserved local
snapshot. A later rerun must reclassify against current main and GitHub history
before deciding whether any replacement issue or PR is still needed. Continue
to Step 2 only when `PR_COUNT=0`.

Expected: every viable row records a paginated same-repo open-PR capture before
any issue reuse or creation begins, and `viable/$WORKSTREAM_ID-open-prs.json`
contains only same-repository candidates whose `head.ref` exactly matches
`$SOURCE_BRANCH`. Same-named fork PRs remain excluded from the candidate count.
Rows with one candidate add an evidence-backed authoritative-source-branch
fetch and a `main`-targeting PR decision; rows with zero candidates skip fetch
and continue normally; rows with multiple candidates block before branch
verification. If `docs/psyche-specs` still has exactly one same-repo open PR
from branch `docs/psyche-specs` at execution time, its `headRefOid` matches
the freshly fetched `origin/docs/psyche-specs` tip, the preserved local
`head.txt` is equal to or an ancestor of that PR head, and the preserved
`worktree.patch`, `index.patch`, and `.untracked.zlist` are all empty, this
step rewrites `Main/PR evidence` to that exact canonical PR URL and records a
deterministic adopted-PR action instead of hardcoding a PR number. If any of
those three preserved dirty classes are non-empty, the row blocks with resume
instructions instead of being treated as fully covered by the existing PR.

- [ ] **Step 2: Reuse or create one issue per viable row that does not already have an adopted PR**

Set `WORKSTREAM_ID` to the viable row's exact workstream ID, then use the
matching exact issue title:

```bash
set -euo pipefail
resolve_control_worktree() {
  local control_branch="docs/541-incomplete-work-recovery-design"
  local expected_branch="refs/heads/$control_branch"
  local remote_branch_ref="refs/remotes/origin/$control_branch"
  local start_common_dir repo target_path path branch_ref actual_branch
  local target_registration_branch live_count stale_count live_path stale_path
  local live_status live_head local_head remote_head
  local -a live_paths=() stale_paths=()
  start_common_dir="$(git rev-parse --git-common-dir)"
  repo="$(cd "$start_common_dir/.." && pwd)"
  target_path="$repo/.worktrees/issue-541-recovery"
  if ! git -C "$repo" fetch --no-tags origin \
    "refs/heads/$control_branch:$remote_branch_ref"
  then
    printf 'Blocked: could not fetch origin/%s into %s.\n' \
      "$control_branch" "$remote_branch_ref" >&2
    exit 1
  fi
  remote_head="$(git -C "$repo" rev-parse --verify "$remote_branch_ref^{commit}")"
  while IFS="$(printf "\t")" read -r path branch_ref; do
    test -n "$path" || continue
    if test "$path" = "$target_path"; then
      target_registration_branch="$branch_ref"
    fi
    if test "$branch_ref" != "$expected_branch"; then
      continue
    fi
    if test -d "$path"; then
      live_paths+=("$path")
    else
      stale_paths+=("$path")
    fi
  done <<EOF2
$(git -C "$repo" worktree list --porcelain | awk '
  $1 == "worktree" { if (path != "") print path "\t" branch; path = substr($0, 10); branch = ""; next }
  $1 == "branch" { branch = $2; next }
  END { if (path != "") print path "\t" branch }
')
EOF2
  live_count="${#live_paths[@]}"
  stale_count="${#stale_paths[@]}"
  if test -n "$target_registration_branch" && test "$target_registration_branch" != "$expected_branch"; then
    printf 'Blocked: %s is registered to %s, not %s.\n' "$target_path" "$target_registration_branch" "$control_branch" >&2
    exit 1
  fi
  if test "$live_count" -gt 1; then
    printf 'Blocked: %s has %s live controller worktree registrations; refusing to choose arbitrarily.\n' "$control_branch" "$live_count" >&2
    for live_path in "${live_paths[@]}"; do
      printf '  live: %s\n' "$live_path" >&2
    done
    for stale_path in "${stale_paths[@]}"; do
      printf '  stale: %s\n' "$stale_path" >&2
    done
    exit 1
  fi
  if test "$live_count" -eq 1; then
    live_path="${live_paths[0]}"
    if test "$stale_count" -gt 0; then
      printf 'Warning: ignoring %s stale controller registration(s) for %s while using the one live worktree.\n' "$stale_count" "$control_branch" >&2
      for stale_path in "${stale_paths[@]}"; do
        printf '  stale: %s\n' "$stale_path" >&2
      done
    fi
    actual_branch="$(git -C "$live_path" branch --show-current)"
    if test "$actual_branch" != "$control_branch"; then
      printf 'Control worktree branch mismatch: expected %s, got %s\n' "$control_branch" "$actual_branch" >&2
      exit 1
    fi
    live_status="$(git -C "$live_path" status --porcelain=v1 --untracked-files=all)"
    if test -n "$live_status"; then
      printf 'Blocked: live controller worktree is dirty and cannot be refreshed safely: %s\n' "$live_path" >&2
      git -C "$live_path" status --short --branch --untracked-files=all >&2
      exit 1
    fi
    live_head="$(git -C "$live_path" rev-parse HEAD)"
    if test "$live_head" = "$remote_head"; then
      :
    elif git -C "$repo" merge-base --is-ancestor "$live_head" "$remote_head"; then
      if ! git -C "$live_path" merge --ff-only "$remote_branch_ref"; then
        printf 'Blocked: live controller worktree could not fast-forward %s to fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
        exit 1
      fi
    elif git -C "$repo" merge-base --is-ancestor "$remote_head" "$live_head"; then
      printf 'Blocked: live controller branch %s is ahead of fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
      exit 1
    else
      printf 'Blocked: live controller branch %s diverged from fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
      exit 1
    fi
    live_head="$(git -C "$live_path" rev-parse HEAD)"
    if test "$live_head" != "$remote_head"; then
      printf 'Blocked: live controller HEAD %s does not match fetched origin tip %s.\n' "$live_head" "$remote_head" >&2
      exit 1
    fi
    printf '%s\n' "$live_path"
    return 0
  fi
  if test "$stale_count" -gt 1; then
    printf 'Blocked: %s has %s stale controller registrations and no live worktree; refusing to choose arbitrarily.\n' "$control_branch" "$stale_count" >&2
    for stale_path in "${stale_paths[@]}"; do
      printf '  stale: %s\n' "$stale_path" >&2
    done
    exit 1
  fi
  if ! git -C "$repo" check-ignore -q .worktrees/; then
    printf 'Repository-local .worktrees/ is not ignored.\n' >&2
    exit 1
  fi
  if ! git -C "$repo" check-ignore -q .worktrees/issue-541-recovery; then
    printf 'Repository-local control worktree path is not ignored: %s\n' '.worktrees/issue-541-recovery' >&2
    exit 1
  fi
  mkdir -p "$repo/.worktrees"
  if test -e "$target_path"; then
    printf 'Control worktree path already exists without a matching live registration: %s\n' "$target_path" >&2
    exit 1
  fi
  if git -C "$repo" show-ref --verify --quiet "$expected_branch"; then
    local_head="$(git -C "$repo" rev-parse --verify "$expected_branch^{commit}")"
    if test "$local_head" = "$remote_head"; then
      :
    elif git -C "$repo" merge-base --is-ancestor "$local_head" "$remote_head"; then
      if ! git -C "$repo" update-ref "$expected_branch" "$remote_head" "$local_head"; then
        printf 'Blocked: local controller branch %s could not be fast-forwarded to fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
        exit 1
      fi
    elif git -C "$repo" merge-base --is-ancestor "$remote_head" "$local_head"; then
      printf 'Blocked: local controller branch %s is ahead of fetched origin tip %s and cannot be rewritten safely.\n' "$control_branch" "$remote_head" >&2
      exit 1
    else
      printf 'Blocked: local controller branch %s diverged from fetched origin tip %s and cannot be rewritten safely.\n' "$control_branch" "$remote_head" >&2
      exit 1
    fi
  else
    if ! git -C "$repo" update-ref "$expected_branch" "$remote_head"; then
      printf 'Blocked: local controller branch %s could not be created at fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
      exit 1
    fi
  fi
  local_head="$(git -C "$repo" rev-parse --verify "$expected_branch^{commit}")"
  if test "$local_head" != "$remote_head"; then
    printf 'Blocked: local controller branch %s resolved to %s instead of fetched origin tip %s.\n' "$control_branch" "$local_head" "$remote_head" >&2
    exit 1
  fi
  if test "$stale_count" -eq 1; then
    stale_path="${stale_paths[0]}"
    printf 'Recreating %s at %s from the one stale controller registration:\n  stale: %s\n' "$control_branch" "$target_path" "$stale_path" >&2
    git -C "$repo" worktree add --force "$target_path" "$control_branch"
  else
    git -C "$repo" worktree add "$target_path" "$control_branch"
  fi
  if ! test -d "$target_path"; then
    printf 'Control worktree path is not present after resolution: %s\n' "$target_path" >&2
    exit 1
  fi
  actual_branch="$(git -C "$target_path" branch --show-current)"
  if test "$actual_branch" != "$control_branch"; then
    printf 'Control worktree branch mismatch: expected %s, got %s\n' "$control_branch" "$actual_branch" >&2
    exit 1
  fi
  local_head="$(git -C "$target_path" rev-parse HEAD)"
  if test "$local_head" != "$remote_head"; then
    printf 'Blocked: resolved controller HEAD %s does not match fetched origin tip %s.\n' "$local_head" "$remote_head" >&2
    exit 1
  fi
  printf '%s\n' "$target_path"
}
CONTROL_WORKTREE="$(resolve_control_worktree)"
COMMON_DIR="$(git -C "$CONTROL_WORKTREE" rev-parse --git-common-dir)"
REPO="$(cd "$COMMON_DIR/.." && pwd)"
RECOVERY="$COMMON_DIR/agent-recovery/issue-541/viable"
PRIVATE_RECOVERY="$COMMON_DIR/agent-recovery/issue-541/private"
CLASSIFICATION="$COMMON_DIR/agent-recovery/issue-541/classification.md"
ALL_ISSUES_EVIDENCE="$COMMON_DIR/agent-recovery/issue-541/issues.json"
mkdir -p "$RECOVERY" "$PRIVATE_RECOVERY/issue-ledger-refresh"
update_classification_row() {
  python3 - "$CLASSIFICATION" "$WORKSTREAM_ID" "$1" "$2" "$3" <<'PY'
from pathlib import Path
import sys

path = Path(sys.argv[1])
workstream, classification, evidence, action = sys.argv[2:6]
needle = f"| {workstream} |"
lines = path.read_text().splitlines()
for idx, line in enumerate(lines):
    if line.startswith(needle):
        parts = [part.strip() for part in line.strip().strip("|").split("|")]
        if len(parts) != 5:
            raise SystemExit(f"Unexpected classification row: {line}")
        parts[1] = classification
        parts[2] = evidence
        parts[4] = action
        lines[idx] = "| " + " | ".join(parts) + " |"
        path.write_text("\n".join(lines) + "\n")
        break
else:
    raise SystemExit(f"Missing classification row for {workstream}")
PY
}
case "$WORKSTREAM_ID" in
  mobile-memory-gateway)
    ISSUE_TITLE="Recover mobile pairing workstream"
    BRANCH_SLUG="mobile-pairing-recovery"
    ARCHIVE_ID="dirty/mobile-memory-gateway"
    ;;
  feat-npm-macos-x64)
    ISSUE_TITLE="Recover or block Intel macOS npm packaging workstream"
    BRANCH_SLUG="npm-macos-x64-recovery"
    ARCHIVE_ID="branches/feat-npm-macos-x64.bundle"
    ;;
  fix-521-ward-surface-confinement)
    ISSUE_TITLE="Recover Ward surface confinement workstream"
    BRANCH_SLUG="ward-surface-confinement-recovery"
    ARCHIVE_ID="branches/fix-521-ward-surface-confinement.bundle"
    ;;
  memory-promote)
    ISSUE_TITLE="Recover memory promotion workstream"
    BRANCH_SLUG="memory-promotion-recovery"
    ARCHIVE_ID="dirty/memory-promote"
    ;;
  docs-psyche-specs)
    ISSUE_TITLE="Recover Psyche specification workstream"
    BRANCH_SLUG="psyche-spec-recovery"
    ARCHIVE_ID="dirty/docs-psyche-specs"
    ;;
  docs-universal-runtime-capability-design)
    ISSUE_TITLE="Recover universal runtime capability design workstream"
    BRANCH_SLUG="universal-runtime-capability-recovery"
    ARCHIVE_ID="branches/docs-universal-runtime-capability-design.bundle"
    ;;
  pr-476-review)
    ISSUE_TITLE="Recover runtime model parity plan workstream"
    BRANCH_SLUG="runtime-parity-plan-recovery"
    ARCHIVE_ID="dirty/pr-476-review"
    ;;
  *)
    printf 'Unknown WORKSTREAM_ID: %s\n' "$WORKSTREAM_ID" >&2
    exit 1
    ;;
esac
ISSUE_SEARCH_EVIDENCE="$RECOVERY/$WORKSTREAM_ID-issue-search.json"
ISSUE_VIEW_EVIDENCE="$RECOVERY/$WORKSTREAM_ID-issue-view.json"
ISSUE_POSTCONDITION_EVIDENCE="$RECOVERY/$WORKSTREAM_ID-issue-postcondition.json"
ISSUE_POSTCONDITION_SEARCH_EVIDENCE="$RECOVERY/$WORKSTREAM_ID-issue-postcondition-search.json"
ISSUE_POSTCONDITION_VIEW_EVIDENCE="$RECOVERY/$WORKSTREAM_ID-issue-postcondition-view.json"
ISSUE_BLOCKER_EVIDENCE="$RECOVERY/$WORKSTREAM_ID-issue-blocker.txt"
ISSUE_LEDGER_EVIDENCE="$RECOVERY/$WORKSTREAM_ID-issues.json"
ISSUE_LEDGER_STAGE="$PRIVATE_RECOVERY/issue-ledger-refresh/$WORKSTREAM_ID-issues.stage.json"
ISSUE_POSTCONDITION_STAGE="$PRIVATE_RECOVERY/issue-ledger-refresh/$WORKSTREAM_ID-issue-postcondition.stage.json"
ISSUE_CREATED_BY_RUN=false
rm -f "$ISSUE_LEDGER_STAGE"
if ! gh api --paginate --slurp \
  "repos/OpenCoven/coven/issues?state=all&per_page=100" \
  > "$ISSUE_LEDGER_STAGE"
then
  rm -f "$ISSUE_LEDGER_STAGE"
  {
    printf 'Blocked %s: could not refresh the paginated issue ledger before reuse/create.\n' \
      "$WORKSTREAM_ID"
    printf 'Preserved shared issue ledger: issue-541/issues.json\n'
    printf 'Per-workstream issue ledger target: viable/%s-issues.json\n' "$WORKSTREAM_ID"
    printf 'Staging file removed after gh api failure: private/issue-ledger-refresh/%s-issues.stage.json\n' "$WORKSTREAM_ID"
  } > "$ISSUE_BLOCKER_EVIDENCE"
  update_classification_row \
    "blocked" \
    "Issue ledger refresh failed without replacing issue-541/issues.json; see viable/$WORKSTREAM_ID-issue-blocker.txt." \
    "Blocked: could not refresh live issue evidence before exact-title reuse/create."
  exit 0
fi
if ! test -s "$ISSUE_LEDGER_STAGE" || \
   ! jq -e 'type == "array" and length > 0 and all(.[]; type == "array")' "$ISSUE_LEDGER_STAGE" > /dev/null
then
  rm -f "$ISSUE_LEDGER_STAGE"
  {
    printf 'Blocked %s: staged issue ledger was empty or not a valid paginated slurped array.\n' \
      "$WORKSTREAM_ID"
    printf 'Preserved shared issue ledger: issue-541/issues.json\n'
    printf 'Per-workstream issue ledger target: viable/%s-issues.json\n' "$WORKSTREAM_ID"
    printf 'Expected staging shape: JSON array whose elements are page arrays.\n'
  } > "$ISSUE_BLOCKER_EVIDENCE"
  update_classification_row \
    "blocked" \
    "Issue ledger staging validation failed without replacing issue-541/issues.json; see viable/$WORKSTREAM_ID-issue-blocker.txt." \
    "Blocked: live issue evidence failed validation before exact-title reuse/create."
  exit 0
fi
if ! mv "$ISSUE_LEDGER_STAGE" "$ALL_ISSUES_EVIDENCE"; then
  rm -f "$ISSUE_LEDGER_STAGE"
  {
    printf 'Blocked %s: could not atomically replace the shared issue ledger.\n' \
      "$WORKSTREAM_ID"
    printf 'Shared issue ledger path: issue-541/issues.json\n'
    printf 'Per-workstream issue ledger target: viable/%s-issues.json\n' "$WORKSTREAM_ID"
  } > "$ISSUE_BLOCKER_EVIDENCE"
  update_classification_row \
    "blocked" \
    "Issue ledger replacement failed; see issue-541/issues.json and viable/$WORKSTREAM_ID-issue-blocker.txt." \
    "Blocked: could not atomically replace shared live issue evidence before exact-title reuse/create."
  exit 0
fi
if ! cp "$ALL_ISSUES_EVIDENCE" "$ISSUE_LEDGER_EVIDENCE" || \
   ! cmp -s "$ALL_ISSUES_EVIDENCE" "$ISSUE_LEDGER_EVIDENCE"
then
  rm -f "$ISSUE_LEDGER_EVIDENCE"
  {
    printf 'Blocked %s: could not persist an exact per-workstream copy of the verified issue ledger.\n' \
      "$WORKSTREAM_ID"
    printf 'Verified shared issue ledger: issue-541/issues.json\n'
    printf 'Per-workstream issue ledger target: viable/%s-issues.json\n' "$WORKSTREAM_ID"
  } > "$ISSUE_BLOCKER_EVIDENCE"
  update_classification_row \
    "blocked" \
    "Issue ledger persistence failed after shared refresh; see issue-541/issues.json and viable/$WORKSTREAM_ID-issue-blocker.txt." \
    "Blocked: could not persist verified live issue evidence before exact-title reuse/create."
  exit 0
fi
jq --arg title "$ISSUE_TITLE" \
  '[.[] | .[] | select(.pull_request? | not) | select(.title == $title and .state == "open") | {number, title, url, state}]' \
  "$ISSUE_LEDGER_EVIDENCE" > "$ISSUE_SEARCH_EVIDENCE"
ISSUE_COUNT="$(jq 'length' "$ISSUE_SEARCH_EVIDENCE")"
case "$ISSUE_COUNT" in
  1)
    ISSUE_NUMBER="$(jq -r '.[0].number' "$ISSUE_SEARCH_EVIDENCE")"
    if ! gh issue view --repo OpenCoven/coven "$ISSUE_NUMBER" \
      --json number,state,title,url > "$ISSUE_VIEW_EVIDENCE"
    then
      {
        printf 'Blocked %s: exact-title candidate issue #%s could not be read for verification.\n' \
          "$WORKSTREAM_ID" "$ISSUE_NUMBER"
        printf 'Task 4 paginated issue evidence: issue-541/issues.json\n'
        printf 'Persisted live issue ledger: viable/%s-issues.json\n' "$WORKSTREAM_ID"
        printf 'Filtered issue evidence: viable/%s-issue-search.json\n' "$WORKSTREAM_ID"
        printf 'Issue view evidence: viable/%s-issue-view.json\n' "$WORKSTREAM_ID"
      } > "$ISSUE_BLOCKER_EVIDENCE"
      update_classification_row \
        "blocked" \
        "Issue verification read failed; see issue-541/issues.json, viable/$WORKSTREAM_ID-issues.json, viable/$WORKSTREAM_ID-issue-search.json, viable/$WORKSTREAM_ID-issue-view.json, and viable/$WORKSTREAM_ID-issue-blocker.txt." \
        "Blocked: candidate issue could not be read before exact-title verification."
      exit 0
    fi
    if ! jq -e --arg title "$ISSUE_TITLE" \
      '.title == $title and .state == "OPEN"' \
      "$ISSUE_VIEW_EVIDENCE" > /dev/null
    then
      {
        printf 'Blocked %s: issue #%s did not verify as exact-title OPEN.\n' \
          "$WORKSTREAM_ID" "$ISSUE_NUMBER"
        printf 'Task 4 paginated issue evidence: issue-541/issues.json\n'
        printf 'Persisted live issue ledger: viable/%s-issues.json\n' "$WORKSTREAM_ID"
        printf 'Filtered issue evidence: viable/%s-issue-search.json\n' "$WORKSTREAM_ID"
        printf 'Issue view evidence: viable/%s-issue-view.json\n' "$WORKSTREAM_ID"
      } > "$ISSUE_BLOCKER_EVIDENCE"
      update_classification_row \
        "blocked" \
        "Issue verification mismatch; see issue-541/issues.json, viable/$WORKSTREAM_ID-issues.json, viable/$WORKSTREAM_ID-issue-search.json, viable/$WORKSTREAM_ID-issue-view.json, and viable/$WORKSTREAM_ID-issue-blocker.txt." \
        "Blocked: candidate issue failed exact-title OPEN verification."
      exit 0
    fi
    ;;
esac
case "$ISSUE_COUNT" in
  0)
    ISSUE_BODY="$(cat <<'EOF'
## Recovered source

- Source issue: #541
- Source artifacts: issue-541 recovery archive and classification ledger.

## Acceptance criteria

- Preserve the still-valid intent identified in the classification evidence.
- Rebuild against current `origin/main`; do not blindly replay obsolete code.
- Add or retain regression coverage for the recovered behavior.
- Pass all repository-required gates.
EOF
)"
    if ! ISSUE_URL="$(gh issue create --repo OpenCoven/coven --title "$ISSUE_TITLE" --body "$ISSUE_BODY")"; then
      {
        printf 'Blocked %s: gh issue create failed for exact title %s.\n' \
          "$WORKSTREAM_ID" "$ISSUE_TITLE"
        printf 'Task 4 paginated issue evidence: issue-541/issues.json\n'
        printf 'Persisted live issue ledger: viable/%s-issues.json\n' "$WORKSTREAM_ID"
        printf 'Filtered issue evidence: viable/%s-issue-search.json\n' "$WORKSTREAM_ID"
      } > "$ISSUE_BLOCKER_EVIDENCE"
      update_classification_row \
        "blocked" \
        "Issue creation failed; see issue-541/issues.json, viable/$WORKSTREAM_ID-issues.json, viable/$WORKSTREAM_ID-issue-search.json, and viable/$WORKSTREAM_ID-issue-blocker.txt." \
        "Blocked: gh issue create failed during exact-title recovery issue creation."
      exit 0
    fi
    ISSUE_NUMBER="${ISSUE_URL##*/}"
    ISSUE_CREATED_BY_RUN=true
    if ! gh issue view --repo OpenCoven/coven "$ISSUE_NUMBER" \
      --json number,state,title,url > "$ISSUE_VIEW_EVIDENCE"
    then
      {
        printf 'Blocked %s: created issue #%s could not be read for verification.\n' \
          "$WORKSTREAM_ID" "$ISSUE_NUMBER"
        printf 'Task 4 paginated issue evidence: issue-541/issues.json\n'
        printf 'Persisted live issue ledger: viable/%s-issues.json\n' "$WORKSTREAM_ID"
        printf 'Filtered issue evidence: viable/%s-issue-search.json\n' "$WORKSTREAM_ID"
        printf 'Issue view evidence: viable/%s-issue-view.json\n' "$WORKSTREAM_ID"
      } > "$ISSUE_BLOCKER_EVIDENCE"
      update_classification_row \
        "blocked" \
        "Created issue verification read failed; see issue-541/issues.json, viable/$WORKSTREAM_ID-issues.json, viable/$WORKSTREAM_ID-issue-search.json, viable/$WORKSTREAM_ID-issue-view.json, and viable/$WORKSTREAM_ID-issue-blocker.txt." \
        "Blocked: created issue could not be read before exact-title verification."
      exit 0
    fi
    if ! jq -e --arg title "$ISSUE_TITLE" \
      '.title == $title and .state == "OPEN"' \
      "$ISSUE_VIEW_EVIDENCE" > /dev/null
    then
      {
        printf 'Blocked %s: created issue #%s did not verify as exact-title OPEN.\n' \
          "$WORKSTREAM_ID" "$ISSUE_NUMBER"
        printf 'Task 4 paginated issue evidence: issue-541/issues.json\n'
        printf 'Persisted live issue ledger: viable/%s-issues.json\n' "$WORKSTREAM_ID"
        printf 'Filtered issue evidence: viable/%s-issue-search.json\n' "$WORKSTREAM_ID"
        printf 'Issue view evidence: viable/%s-issue-view.json\n' "$WORKSTREAM_ID"
      } > "$ISSUE_BLOCKER_EVIDENCE"
      update_classification_row \
        "blocked" \
        "Created issue verification mismatch; see issue-541/issues.json, viable/$WORKSTREAM_ID-issues.json, viable/$WORKSTREAM_ID-issue-search.json, viable/$WORKSTREAM_ID-issue-view.json, and viable/$WORKSTREAM_ID-issue-blocker.txt." \
        "Blocked: created issue did not verify as exact-title OPEN."
      exit 0
    fi
    ;;
  1)
    ;;
  *)
    {
      printf 'Blocked %s: expected 0 or 1 exact-title open issues for %s, found %s.\n' \
        "$WORKSTREAM_ID" "$ISSUE_TITLE" "$ISSUE_COUNT"
      printf 'Task 4 paginated issue evidence: issue-541/issues.json\n'
      printf 'Persisted live issue ledger: viable/%s-issues.json\n' "$WORKSTREAM_ID"
      printf 'Filtered issue evidence: viable/%s-issue-search.json\n' "$WORKSTREAM_ID"
    } > "$ISSUE_BLOCKER_EVIDENCE"
    update_classification_row \
      "blocked" \
      "Issue ambiguity; see issue-541/issues.json, viable/$WORKSTREAM_ID-issues.json, viable/$WORKSTREAM_ID-issue-search.json, and viable/$WORKSTREAM_ID-issue-blocker.txt." \
      "Blocked: multiple open issues exactly match $ISSUE_TITLE."
    exit 0
    ;;
esac
ISSUE_NUMBER="$(jq -r '.number' "$ISSUE_VIEW_EVIDENCE")"
ISSUE_URL="$(jq -r '.url' "$ISSUE_VIEW_EVIDENCE")"
RECOVERY_BRANCH="issue-$ISSUE_NUMBER-$BRANCH_SLUG"
rm -f "$ISSUE_POSTCONDITION_STAGE"
if ! gh api --paginate --slurp \
  "repos/OpenCoven/coven/issues?state=all&per_page=100" \
  > "$ISSUE_POSTCONDITION_STAGE"
then
  rm -f "$ISSUE_POSTCONDITION_STAGE"
  {
    printf 'Blocked %s: could not refresh the postcondition paginated issue ledger after selecting issue #%s.\n' \
      "$WORKSTREAM_ID" "$ISSUE_NUMBER"
    printf 'Verified precondition issue ledger: issue-541/issues.json\n'
    printf 'Postcondition staging target: private/issue-ledger-refresh/%s-issue-postcondition.stage.json\n' "$WORKSTREAM_ID"
  } > "$ISSUE_BLOCKER_EVIDENCE"
  update_classification_row \
    "blocked" \
    "Issue postcondition refresh failed without replacing viable/$WORKSTREAM_ID-issue-postcondition-search.json; see viable/$WORKSTREAM_ID-issue-blocker.txt." \
    "Blocked: could not refresh live issue evidence after exact-title reuse/create."
  exit 0
fi
if ! test -s "$ISSUE_POSTCONDITION_STAGE" || \
   ! jq -e 'type == "array" and length > 0 and all(.[]; type == "array")' "$ISSUE_POSTCONDITION_STAGE" > /dev/null
then
  rm -f "$ISSUE_POSTCONDITION_STAGE"
  {
    printf 'Blocked %s: staged postcondition issue ledger was empty or not a valid paginated slurped array.\n' \
      "$WORKSTREAM_ID"
    printf 'Verified precondition issue ledger: issue-541/issues.json\n'
    printf 'Postcondition staging target: private/issue-ledger-refresh/%s-issue-postcondition.stage.json\n' "$WORKSTREAM_ID"
  } > "$ISSUE_BLOCKER_EVIDENCE"
  update_classification_row \
    "blocked" \
    "Issue postcondition staging validation failed without replacing viable/$WORKSTREAM_ID-issue-postcondition-search.json; see viable/$WORKSTREAM_ID-issue-blocker.txt." \
    "Blocked: live issue evidence failed validation after exact-title reuse/create."
  exit 0
fi
if ! mv "$ISSUE_POSTCONDITION_STAGE" "$ISSUE_POSTCONDITION_EVIDENCE"; then
  rm -f "$ISSUE_POSTCONDITION_STAGE"
  {
    printf 'Blocked %s: could not atomically replace the postcondition issue ledger.\n' \
      "$WORKSTREAM_ID"
    printf 'Postcondition staging target: private/issue-ledger-refresh/%s-issue-postcondition.stage.json\n' "$WORKSTREAM_ID"
    printf 'Postcondition ledger path: viable/%s-issue-postcondition.json\n' "$WORKSTREAM_ID"
  } > "$ISSUE_BLOCKER_EVIDENCE"
  update_classification_row \
    "blocked" \
    "Issue postcondition replacement failed; see viable/$WORKSTREAM_ID-issue-blocker.txt." \
    "Blocked: could not atomically replace postcondition live issue evidence after exact-title reuse/create."
  exit 0
fi
jq --arg title "$ISSUE_TITLE" \
  '[.[] | .[] | select(.pull_request? | not) | select(.title == $title and .state == "open") | {number, title, url, state}]' \
  "$ISSUE_POSTCONDITION_EVIDENCE" > "$ISSUE_POSTCONDITION_SEARCH_EVIDENCE"
if ! jq -e 'length == 1' "$ISSUE_POSTCONDITION_SEARCH_EVIDENCE" > /dev/null || \
   ! jq -e --argjson issue_number "$ISSUE_NUMBER" '.[0].number == $issue_number' "$ISSUE_POSTCONDITION_SEARCH_EVIDENCE" > /dev/null
then
  {
    printf 'Blocked %s: postcondition issue evidence did not resolve to exactly one exact-title OPEN non-PR issue matching #%s.\n' \
      "$WORKSTREAM_ID" "$ISSUE_NUMBER"
    printf 'Postcondition paginated issue evidence: viable/%s-issue-postcondition.json\n' "$WORKSTREAM_ID"
    printf 'Postcondition filtered issue evidence: viable/%s-issue-postcondition-search.json\n' "$WORKSTREAM_ID"
    printf 'Postcondition issue view evidence: viable/%s-issue-postcondition-view.json\n' "$WORKSTREAM_ID"
    printf 'Issue created by this run: %s\n' "$ISSUE_CREATED_BY_RUN"
  } > "$ISSUE_BLOCKER_EVIDENCE"
  if [ "$ISSUE_CREATED_BY_RUN" = true ]; then
    RACE_CLOSE_COMMENT='Closing as superseded after race-detected exact-title duplicate during issue-541 recovery reconciliation.'
    printf 'Race cleanup required: this run created issue #%s.\n' "$ISSUE_NUMBER" >> "$ISSUE_BLOCKER_EVIDENCE"
    printf 'Race cleanup comment: %s\n' "$RACE_CLOSE_COMMENT" >> "$ISSUE_BLOCKER_EVIDENCE"
    if ! gh issue close --repo OpenCoven/coven "$ISSUE_NUMBER" --comment "$RACE_CLOSE_COMMENT" \
      >> "$ISSUE_BLOCKER_EVIDENCE" 2>&1
    then
      printf 'Race cleanup blocked: gh issue close failed for created issue #%s.\n' "$ISSUE_NUMBER" \
        >> "$ISSUE_BLOCKER_EVIDENCE"
      update_classification_row \
        "blocked" \
        "Issue postcondition mismatch and created-issue cleanup failed; see issue-541/issues.json, viable/$WORKSTREAM_ID-issue-postcondition.json, viable/$WORKSTREAM_ID-issue-postcondition-search.json, viable/$WORKSTREAM_ID-issue-postcondition-view.json, and viable/$WORKSTREAM_ID-issue-blocker.txt." \
        "Blocked: race-detected duplicate issue cleanup failed after exact-title creation."
      exit 0
    fi
    if ! gh issue view --repo OpenCoven/coven "$ISSUE_NUMBER" \
      --json number,state,title,url > "$ISSUE_POSTCONDITION_VIEW_EVIDENCE" 2>> "$ISSUE_BLOCKER_EVIDENCE"
    then
      printf 'Race cleanup blocked: could not verify CLOSED state for created issue #%s.\n' "$ISSUE_NUMBER" \
        >> "$ISSUE_BLOCKER_EVIDENCE"
      update_classification_row \
        "blocked" \
        "Issue postcondition mismatch and created-issue closure could not be verified; see issue-541/issues.json, viable/$WORKSTREAM_ID-issue-postcondition.json, viable/$WORKSTREAM_ID-issue-postcondition-search.json, viable/$WORKSTREAM_ID-issue-postcondition-view.json, and viable/$WORKSTREAM_ID-issue-blocker.txt." \
        "Blocked: race-detected duplicate issue closure could not be verified as CLOSED."
      exit 0
    fi
    if ! jq -e '.state == "CLOSED"' "$ISSUE_POSTCONDITION_VIEW_EVIDENCE" > /dev/null
    then
      printf 'Race cleanup blocked: created issue #%s did not verify CLOSED after gh issue close.\n' "$ISSUE_NUMBER" \
        >> "$ISSUE_BLOCKER_EVIDENCE"
      update_classification_row \
        "blocked" \
        "Issue postcondition mismatch and created issue remained open after cleanup attempt; see issue-541/issues.json, viable/$WORKSTREAM_ID-issue-postcondition.json, viable/$WORKSTREAM_ID-issue-postcondition-search.json, viable/$WORKSTREAM_ID-issue-postcondition-view.json, and viable/$WORKSTREAM_ID-issue-blocker.txt." \
        "Blocked: race-detected duplicate issue did not verify CLOSED after cleanup."
      exit 0
    fi
    printf 'Race cleanup outcome: created issue #%s verified CLOSED after superseded/race-detected closure.\n' \
      "$ISSUE_NUMBER" >> "$ISSUE_BLOCKER_EVIDENCE"
    update_classification_row \
      "blocked" \
      "Issue postcondition mismatch; created issue #$ISSUE_NUMBER was closed as superseded after race detection. See issue-541/issues.json, viable/$WORKSTREAM_ID-issue-postcondition.json, viable/$WORKSTREAM_ID-issue-postcondition-search.json, viable/$WORKSTREAM_ID-issue-postcondition-view.json, and viable/$WORKSTREAM_ID-issue-blocker.txt." \
      "Blocked: race-detected duplicate issue was closed; rerun exact-title reconciliation before continuing."
    exit 0
  fi
  printf 'Race cleanup skipped: reused pre-existing issue #%s was not created by this run.\n' "$ISSUE_NUMBER" \
    >> "$ISSUE_BLOCKER_EVIDENCE"
  update_classification_row \
    "blocked" \
    "Issue postcondition mismatch; reused issue #$ISSUE_NUMBER was preserved because it predates this run. See issue-541/issues.json, viable/$WORKSTREAM_ID-issue-postcondition.json, viable/$WORKSTREAM_ID-issue-postcondition-search.json, viable/$WORKSTREAM_ID-issue-postcondition-view.json, and viable/$WORKSTREAM_ID-issue-blocker.txt." \
    "Blocked: postcondition exact-title issue verification failed after reuse/create."
  exit 0
fi
jq '.[0]' "$ISSUE_POSTCONDITION_SEARCH_EVIDENCE" > "$ISSUE_POSTCONDITION_VIEW_EVIDENCE"
update_classification_row \
  "viable" \
  "Awaiting recovery PR; issue verified via issue-541/issues.json, viable/$WORKSTREAM_ID-issues.json, viable/$WORKSTREAM_ID-issue-search.json, viable/$WORKSTREAM_ID-issue-view.json, viable/$WORKSTREAM_ID-issue-postcondition.json, viable/$WORKSTREAM_ID-issue-postcondition-search.json, and viable/$WORKSTREAM_ID-issue-postcondition-view.json." \
  "mode=awaiting-recovery-pr; pr_kind=recovered; issue_url=$ISSUE_URL; archive_id=$ARCHIVE_ID; expected_branch=$RECOVERY_BRANCH"
```

When `ISSUE_COUNT=0`, the self-contained block above creates and verifies an
issue with this exact body:

```markdown
## Recovered source

- Source issue: #541
- Source artifacts: issue-541 recovery archive and classification ledger.

## Acceptance criteria

- Preserve the still-valid intent identified in the classification evidence.
- Rebuild against current `origin/main`; do not blindly replay obsolete code.
- Add or retain regression coverage for the recovered behavior.
- Pass all repository-required gates.
```


If `ISSUE_COUNT>1`, write `viable/$WORKSTREAM_ID-issue-blocker.txt`, update
the row to `blocked`, and stop that row rather than choosing one arbitrarily.

Expected: every viable row without an adopted PR has exactly one verified issue
number, and every reuse, create, or block decision cites the saved paginated
issue ledger, per-workstream filtered search evidence, and verification files.
After this step, those rows remain explicitly non-terminal: `Main/PR evidence`
still says the row is awaiting a recovery PR, while `Recovery action` is the
deterministic `mode=awaiting-recovery-pr; ... expected_branch=issue-<n>-<slug>`
payload that Task 5 Step 5 must later rewrite after a PR exists. If the
postcondition detects a race after this run created a duplicate issue, close
that newly created issue as superseded with an explicit `--repo OpenCoven/coven`
comment, verify it is `CLOSED`, leave any reused pre-existing issue untouched,
and keep the row blocked until a rerun revalidates uniqueness.

- [ ] **Step 3: Create one approved design per viable issue**

Use the brainstorming workflow separately for each issue that reached Step 2
without being blocked. The design must name:

- the current-main files and contracts involved;
- the exact portion of the preserved source that remains valid;
- intentionally omitted obsolete portions;
- test and migration behavior;
- one-concern pull-request boundaries.

Expected: each viable concern that is not already continuing an adopted PR has
an approved and committed design document.

- [ ] **Step 4: Create one implementation plan per approved design**

Use the writing-plans workflow separately for each approved design that reached
Step 3 without being blocked. Each plan must include exact paths, failing
tests, targeted commands, the applicable repository-native npm/package commands
below instead of root-level placeholders, full repository gates, commit
boundaries, explicit `git commit -s` usage, the session-required Copilot
co-author trailer on child commits, push, and PR creation. Human contributor
`Co-authored-by:` trailers remain conditional under `AGENTS.md` and are
separate from the required Copilot trailer.

When a child recovery plan touches npm or Node-delivered surfaces, its gate
section must use these repository-native commands:

- `packages/channels`: `npm ci`, `npm run build`, and `npm test` in
  `packages/channels` (or the exact `npm --prefix packages/channels ...`
  equivalents).
- npm CLI wrapper or platform packaging (`packages/cli`, `npm/coven`, platform
  package manifests, or the publish/prepublish scripts): run the supported
  `node scripts/test-cli-prepublish.mjs` smoke for the affected target and pair
  it with the matching release build plus the repository cargo gates. The
  supported matrix is `macos`/`aarch64-apple-darwin`,
  `linux-x64`/`x86_64-unknown-linux-gnu`, and
  `windows`/`x86_64-pc-windows-msvc`. Current main cannot validate Intel x64,
  and `--target=macos` must never be used as a proxy for Intel recovery. If the
  child design/plan has not first restored the concrete
  `macos-x64`/`@opencoven/cli-macos-x64` contract in current code with tests
  proving default darwin x64 mapping and package metadata, classify the
  Intel workstream as blocked. Once that contract exists, the exact Intel
  validation command is
  `node scripts/test-cli-prepublish.mjs --target=macos-x64 --with-cargo-gates`,
  plus any targeted Node tests documented by the child plan.
- `packages/openclaw-coven`: `npm install` and `npm exec vitest run` in
  `packages/openclaw-coven`; do not claim nonexistent package-local build or
  test scripts there.
- `packages/cli` and `npm/coven` have no package-local `npm run build` or
  `npm test` scripts; validate them through the prepublish smoke and the
  relevant Node tests that it already executes.

Every child plan must reuse this exact child-commit pattern, replacing only
the commit message and adding any conditional human contributor trailers as
separate extra `--trailer` arguments:

```bash
set -euo pipefail
COPILOT_GH_ID=223556219
COPILOT_GH_USER=Copilot
COPILOT_NOREPLY_DOMAIN=users.noreply.github.com
COPILOT_TRAILER="Co-authored-by: $COPILOT_GH_USER <${COPILOT_GH_ID}+${COPILOT_GH_USER}@${COPILOT_NOREPLY_DOMAIN}>"
git commit -s --trailer "$COPILOT_TRAILER" -m "<child commit message>"
```

Expected: independent plans exist only for viable rows that are not already
continuing an adopted exact-source-branch PR or an adopted exact
recovery-branch PR from a rerun.

- [ ] **Step 5: Recover and publish each viable concern sequentially**

Skip this step for any row whose Task 5 Step 1 action is
`mode=continue-existing-pr`.
For each remaining viable row, set `WORKSTREAM_ID` to its exact workstream ID,
set `ISSUE_NUMBER` to its created or reused issue number, and set
`BRANCH_SLUG` from this fixed mapping:

```text
mobile-memory-gateway -> mobile-pairing-recovery
feat-npm-macos-x64 -> npm-macos-x64-recovery
fix-521-ward-surface-confinement -> ward-surface-confinement-recovery
memory-promote -> memory-promotion-recovery
docs-psyche-specs -> psyche-spec-recovery
docs-universal-runtime-capability-design -> universal-runtime-capability-recovery
pr-476-review -> runtime-parity-plan-recovery
```

Then run:

```bash
set -euo pipefail
resolve_control_worktree() {
  local control_branch="docs/541-incomplete-work-recovery-design"
  local expected_branch="refs/heads/$control_branch"
  local remote_branch_ref="refs/remotes/origin/$control_branch"
  local start_common_dir repo target_path path branch_ref actual_branch
  local target_registration_branch live_count stale_count live_path stale_path
  local live_status live_head local_head remote_head
  local -a live_paths=() stale_paths=()
  start_common_dir="$(git rev-parse --git-common-dir)"
  repo="$(cd "$start_common_dir/.." && pwd)"
  target_path="$repo/.worktrees/issue-541-recovery"
  if ! git -C "$repo" fetch --no-tags origin \
    "refs/heads/$control_branch:$remote_branch_ref"
  then
    printf 'Blocked: could not fetch origin/%s into %s.\n' \
      "$control_branch" "$remote_branch_ref" >&2
    exit 1
  fi
  remote_head="$(git -C "$repo" rev-parse --verify "$remote_branch_ref^{commit}")"
  while IFS="$(printf "\t")" read -r path branch_ref; do
    test -n "$path" || continue
    if test "$path" = "$target_path"; then
      target_registration_branch="$branch_ref"
    fi
    if test "$branch_ref" != "$expected_branch"; then
      continue
    fi
    if test -d "$path"; then
      live_paths+=("$path")
    else
      stale_paths+=("$path")
    fi
  done <<EOF2
$(git -C "$repo" worktree list --porcelain | awk '
  $1 == "worktree" { if (path != "") print path "\t" branch; path = substr($0, 10); branch = ""; next }
  $1 == "branch" { branch = $2; next }
  END { if (path != "") print path "\t" branch }
')
EOF2
  live_count="${#live_paths[@]}"
  stale_count="${#stale_paths[@]}"
  if test -n "$target_registration_branch" && test "$target_registration_branch" != "$expected_branch"; then
    printf 'Blocked: %s is registered to %s, not %s.\n' "$target_path" "$target_registration_branch" "$control_branch" >&2
    exit 1
  fi
  if test "$live_count" -gt 1; then
    printf 'Blocked: %s has %s live controller worktree registrations; refusing to choose arbitrarily.\n' "$control_branch" "$live_count" >&2
    for live_path in "${live_paths[@]}"; do
      printf '  live: %s\n' "$live_path" >&2
    done
    for stale_path in "${stale_paths[@]}"; do
      printf '  stale: %s\n' "$stale_path" >&2
    done
    exit 1
  fi
  if test "$live_count" -eq 1; then
    live_path="${live_paths[0]}"
    if test "$stale_count" -gt 0; then
      printf 'Warning: ignoring %s stale controller registration(s) for %s while using the one live worktree.\n' "$stale_count" "$control_branch" >&2
      for stale_path in "${stale_paths[@]}"; do
        printf '  stale: %s\n' "$stale_path" >&2
      done
    fi
    actual_branch="$(git -C "$live_path" branch --show-current)"
    if test "$actual_branch" != "$control_branch"; then
      printf 'Control worktree branch mismatch: expected %s, got %s\n' "$control_branch" "$actual_branch" >&2
      exit 1
    fi
    live_status="$(git -C "$live_path" status --porcelain=v1 --untracked-files=all)"
    if test -n "$live_status"; then
      printf 'Blocked: live controller worktree is dirty and cannot be refreshed safely: %s\n' "$live_path" >&2
      git -C "$live_path" status --short --branch --untracked-files=all >&2
      exit 1
    fi
    live_head="$(git -C "$live_path" rev-parse HEAD)"
    if test "$live_head" = "$remote_head"; then
      :
    elif git -C "$repo" merge-base --is-ancestor "$live_head" "$remote_head"; then
      if ! git -C "$live_path" merge --ff-only "$remote_branch_ref"; then
        printf 'Blocked: live controller worktree could not fast-forward %s to fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
        exit 1
      fi
    elif git -C "$repo" merge-base --is-ancestor "$remote_head" "$live_head"; then
      printf 'Blocked: live controller branch %s is ahead of fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
      exit 1
    else
      printf 'Blocked: live controller branch %s diverged from fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
      exit 1
    fi
    live_head="$(git -C "$live_path" rev-parse HEAD)"
    if test "$live_head" != "$remote_head"; then
      printf 'Blocked: live controller HEAD %s does not match fetched origin tip %s.\n' "$live_head" "$remote_head" >&2
      exit 1
    fi
    printf '%s\n' "$live_path"
    return 0
  fi
  if test "$stale_count" -gt 1; then
    printf 'Blocked: %s has %s stale controller registrations and no live worktree; refusing to choose arbitrarily.\n' "$control_branch" "$stale_count" >&2
    for stale_path in "${stale_paths[@]}"; do
      printf '  stale: %s\n' "$stale_path" >&2
    done
    exit 1
  fi
  if ! git -C "$repo" check-ignore -q .worktrees/; then
    printf 'Repository-local .worktrees/ is not ignored.\n' >&2
    exit 1
  fi
  if ! git -C "$repo" check-ignore -q .worktrees/issue-541-recovery; then
    printf 'Repository-local control worktree path is not ignored: %s\n' '.worktrees/issue-541-recovery' >&2
    exit 1
  fi
  mkdir -p "$repo/.worktrees"
  if test -e "$target_path"; then
    printf 'Control worktree path already exists without a matching live registration: %s\n' "$target_path" >&2
    exit 1
  fi
  if git -C "$repo" show-ref --verify --quiet "$expected_branch"; then
    local_head="$(git -C "$repo" rev-parse --verify "$expected_branch^{commit}")"
    if test "$local_head" = "$remote_head"; then
      :
    elif git -C "$repo" merge-base --is-ancestor "$local_head" "$remote_head"; then
      if ! git -C "$repo" update-ref "$expected_branch" "$remote_head" "$local_head"; then
        printf 'Blocked: local controller branch %s could not be fast-forwarded to fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
        exit 1
      fi
    elif git -C "$repo" merge-base --is-ancestor "$remote_head" "$local_head"; then
      printf 'Blocked: local controller branch %s is ahead of fetched origin tip %s and cannot be rewritten safely.\n' "$control_branch" "$remote_head" >&2
      exit 1
    else
      printf 'Blocked: local controller branch %s diverged from fetched origin tip %s and cannot be rewritten safely.\n' "$control_branch" "$remote_head" >&2
      exit 1
    fi
  else
    if ! git -C "$repo" update-ref "$expected_branch" "$remote_head"; then
      printf 'Blocked: local controller branch %s could not be created at fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
      exit 1
    fi
  fi
  local_head="$(git -C "$repo" rev-parse --verify "$expected_branch^{commit}")"
  if test "$local_head" != "$remote_head"; then
    printf 'Blocked: local controller branch %s resolved to %s instead of fetched origin tip %s.\n' "$control_branch" "$local_head" "$remote_head" >&2
    exit 1
  fi
  if test "$stale_count" -eq 1; then
    stale_path="${stale_paths[0]}"
    printf 'Recreating %s at %s from the one stale controller registration:\n  stale: %s\n' "$control_branch" "$target_path" "$stale_path" >&2
    git -C "$repo" worktree add --force "$target_path" "$control_branch"
  else
    git -C "$repo" worktree add "$target_path" "$control_branch"
  fi
  if ! test -d "$target_path"; then
    printf 'Control worktree path is not present after resolution: %s\n' "$target_path" >&2
    exit 1
  fi
  actual_branch="$(git -C "$target_path" branch --show-current)"
  if test "$actual_branch" != "$control_branch"; then
    printf 'Control worktree branch mismatch: expected %s, got %s\n' "$control_branch" "$actual_branch" >&2
    exit 1
  fi
  local_head="$(git -C "$target_path" rev-parse HEAD)"
  if test "$local_head" != "$remote_head"; then
    printf 'Blocked: resolved controller HEAD %s does not match fetched origin tip %s.\n' "$local_head" "$remote_head" >&2
    exit 1
  fi
  printf '%s\n' "$target_path"
}
CONTROL_WORKTREE="$(resolve_control_worktree)"
COMMON_DIR="$(git -C "$CONTROL_WORKTREE" rev-parse --git-common-dir)"
REPO="$(cd "$COMMON_DIR/.." && pwd)"
RECOVERY="$COMMON_DIR/agent-recovery/issue-541/viable"
CLASSIFICATION="$COMMON_DIR/agent-recovery/issue-541/classification.md"
CLAIM_BLOCKER_EVIDENCE="$RECOVERY/$WORKSTREAM_ID-claim-blocker.txt"
PR_BLOCKER_EVIDENCE="$RECOVERY/$WORKSTREAM_ID-pr-blocker.txt"
ISSUE_VIEW_EVIDENCE="$RECOVERY/$WORKSTREAM_ID-issue-view.json"
RECOVERY_OPEN_PR_EVIDENCE="$RECOVERY/$WORKSTREAM_ID-recovery-open-prs.json"
RECOVERY_PR_VIEW_EVIDENCE="$RECOVERY/$WORKSTREAM_ID-recovery-pr-view.json"
RECOVERY_PR_ADOPTION_EVIDENCE="$RECOVERY/$WORKSTREAM_ID-recovery-pr-adoption.txt"
RECOVERY_BRANCH_REF_EVIDENCE="$RECOVERY/$WORKSTREAM_ID-recovery-branch-refs.txt"
mkdir -p "$RECOVERY"
update_classification_row() {
  python3 - "$CLASSIFICATION" "$WORKSTREAM_ID" "$1" "$2" "$3" <<'PY'
from pathlib import Path
import sys

path = Path(sys.argv[1])
workstream, classification, evidence, action = sys.argv[2:6]
needle = f"| {workstream} |"
lines = path.read_text().splitlines()
for idx, line in enumerate(lines):
    if line.startswith(needle):
        parts = [part.strip() for part in line.strip().strip("|").split("|")]
        if len(parts) != 5:
            raise SystemExit(f"Unexpected classification row: {line}")
        parts[1] = classification
        parts[2] = evidence
        parts[4] = action
        lines[idx] = "| " + " | ".join(parts) + " |"
        path.write_text("\n".join(lines) + "\n")
        break
else:
    raise SystemExit(f"Missing classification row for {workstream}")
PY
}
test -s "$ISSUE_VIEW_EVIDENCE"
ISSUE_URL="$(jq -r '.url' "$ISSUE_VIEW_EVIDENCE")"
RECOVERY_BRANCH="issue-$ISSUE_NUMBER-$BRANCH_SLUG"
RECOVERY_WORKTREE="$REPO/.worktrees/coven-recovery-541-$ISSUE_NUMBER-$BRANCH_SLUG"
{
  printf 'Expected recovery branch: %s\n' "$RECOVERY_BRANCH"
  printf 'Candidate discovery: capture every open PR for OpenCoven/coven via paginated REST API, then filter with jq only when head.ref matches the exact recovery branch and head.repo.full_name equals OpenCoven/coven.\n'
  printf 'This rerun check runs before any new branch, worktree, or claim creation.\n'
} > "$RECOVERY_PR_ADOPTION_EVIDENCE"
if ! {
  gh api --paginate --slurp \
    "repos/OpenCoven/coven/pulls?state=open&per_page=100" | \
  jq --arg RECOVERY_BRANCH "$RECOVERY_BRANCH" '
    [ .[] | .[]
      | select(.head.ref == $RECOVERY_BRANCH)
      | select(.head.repo != null and .head.repo.full_name == "OpenCoven/coven")
      | {
          number,
          title,
          url: .html_url,
          state,
          headRefName: .head.ref,
          headRefOid: .head.sha,
          headRepoFullName: .head.repo.full_name,
          baseRefName: .base.ref
        }
    ]
  ' > "$RECOVERY_OPEN_PR_EVIDENCE"
} 2> "$PR_BLOCKER_EVIDENCE"; then
  API_ERROR="$(cat "$PR_BLOCKER_EVIDENCE")"
  {
    printf 'Blocked %s: exact recovery-branch open-PR capture failed before any new branch/worktree creation.\n' "$WORKSTREAM_ID"
    printf 'Expected recovery branch: %s\n' "$RECOVERY_BRANCH"
    printf 'Issue evidence: viable/%s-issue-view.json\n' "$WORKSTREAM_ID"
    printf 'Failure follows:\n%s\n' "$API_ERROR"
  } > "$PR_BLOCKER_EVIDENCE"
  update_classification_row \
    "blocked" \
    "Recovery-branch PR capture failed for $RECOVERY_BRANCH; see viable/$WORKSTREAM_ID-recovery-pr-adoption.txt, viable/$WORKSTREAM_ID-recovery-open-prs.json, viable/$WORKSTREAM_ID-issue-view.json, and viable/$WORKSTREAM_ID-pr-blocker.txt." \
    "Blocked: could not query same-repo recovery PRs before rerun branch creation."
  exit 0
fi
RECOVERY_PR_COUNT="$(jq 'length' "$RECOVERY_OPEN_PR_EVIDENCE")"
case "$RECOVERY_PR_COUNT" in
  1)
    printf 'Same-repo exact recovery-branch open PR count: 1\n' >> "$RECOVERY_PR_ADOPTION_EVIDENCE"
    RECOVERY_PR_NUMBER="$(jq -r '.[0].number' "$RECOVERY_OPEN_PR_EVIDENCE")"
    printf 'Fetching origin/%s for recovery-branch identity verification.\n' \
      "$RECOVERY_BRANCH" >> "$RECOVERY_PR_ADOPTION_EVIDENCE"
    if ! git -C "$REPO" fetch --no-tags origin \
      "refs/heads/$RECOVERY_BRANCH:refs/remotes/origin/$RECOVERY_BRANCH" \
      >> "$RECOVERY_PR_ADOPTION_EVIDENCE" 2>&1; then
      FETCH_OUTPUT="$(cat "$RECOVERY_PR_ADOPTION_EVIDENCE")"
      {
        printf 'Blocked %s: exact recovery-branch candidate exists, but origin/%s could not be fetched for identity verification.\n' \
          "$WORKSTREAM_ID" "$RECOVERY_BRANCH"
        printf 'Issue evidence: viable/%s-issue-view.json\n' "$WORKSTREAM_ID"
        printf 'Open PR evidence: viable/%s-recovery-open-prs.json\n' "$WORKSTREAM_ID"
        printf 'Recovery adoption evidence: viable/%s-recovery-pr-adoption.txt\n' "$WORKSTREAM_ID"
        printf 'Fetch output follows:\n%s\n' "$FETCH_OUTPUT"
      } > "$PR_BLOCKER_EVIDENCE"
      update_classification_row \
        "blocked" \
        "Recovery-branch PR fetch failed for $RECOVERY_BRANCH; see viable/$WORKSTREAM_ID-recovery-pr-adoption.txt, viable/$WORKSTREAM_ID-recovery-open-prs.json, viable/$WORKSTREAM_ID-issue-view.json, and viable/$WORKSTREAM_ID-pr-blocker.txt." \
        "Blocked: exact recovery-branch candidate could not be fetched before rerun branch verification."
      exit 0
    fi
    FRESH_RECOVERY_BRANCH_TIP="$(git -C "$REPO" rev-parse "refs/remotes/origin/$RECOVERY_BRANCH")"
    printf 'Fresh fetched origin/%s tip: %s\n' \
      "$RECOVERY_BRANCH" "$FRESH_RECOVERY_BRANCH_TIP" >> "$RECOVERY_PR_ADOPTION_EVIDENCE"
    if ! gh pr view --repo OpenCoven/coven "$RECOVERY_PR_NUMBER" \
      --json number,title,url,state,headRefOid,headRefName,headRepositoryOwner,isCrossRepository,baseRefName \
      > "$RECOVERY_PR_VIEW_EVIDENCE" 2> "$PR_BLOCKER_EVIDENCE"; then
      PR_VIEW_ERROR="$(cat "$PR_BLOCKER_EVIDENCE")"
      {
        printf 'Blocked %s: exact recovery-branch PR #%s disappeared or could not be read.\n' \
          "$WORKSTREAM_ID" "$RECOVERY_PR_NUMBER"
        printf 'Expected recovery branch: %s\n' "$RECOVERY_BRANCH"
        printf 'Fresh fetched origin/%s tip: %s\n' "$RECOVERY_BRANCH" "$FRESH_RECOVERY_BRANCH_TIP"
        printf 'Issue evidence: viable/%s-issue-view.json\n' "$WORKSTREAM_ID"
        printf 'Open PR evidence: viable/%s-recovery-open-prs.json\n' "$WORKSTREAM_ID"
        printf 'gh pr view failure follows:\n%s\n' "$PR_VIEW_ERROR"
      } > "$PR_BLOCKER_EVIDENCE"
      update_classification_row \
        "blocked" \
        "Recovery-branch PR view failed for $RECOVERY_BRANCH; see viable/$WORKSTREAM_ID-recovery-pr-adoption.txt, viable/$WORKSTREAM_ID-recovery-open-prs.json, viable/$WORKSTREAM_ID-recovery-pr-view.json, and viable/$WORKSTREAM_ID-pr-blocker.txt." \
        "Blocked: exact recovery-branch PR could not be verified before rerun branch creation."
      exit 0
    fi
    ACTUAL_RECOVERY_PR_URL="$(jq -r '.url' "$RECOVERY_PR_VIEW_EVIDENCE")"
    ACTUAL_RECOVERY_PR_STATE="$(jq -r '.state' "$RECOVERY_PR_VIEW_EVIDENCE")"
    ACTUAL_RECOVERY_PR_HEAD="$(jq -r '.headRefOid' "$RECOVERY_PR_VIEW_EVIDENCE")"
    ACTUAL_RECOVERY_PR_BRANCH="$(jq -r '.headRefName' "$RECOVERY_PR_VIEW_EVIDENCE")"
    ACTUAL_RECOVERY_PR_OWNER="$(jq -r '.headRepositoryOwner.login' "$RECOVERY_PR_VIEW_EVIDENCE")"
    ACTUAL_RECOVERY_PR_CROSS="$(jq -r '.isCrossRepository' "$RECOVERY_PR_VIEW_EVIDENCE")"
    ACTUAL_RECOVERY_PR_BASE="$(jq -r '.baseRefName' "$RECOVERY_PR_VIEW_EVIDENCE")"
    {
      printf 'Existing recovery PR URL: %s\n' "$ACTUAL_RECOVERY_PR_URL"
      printf 'Actual state: %s\n' "$ACTUAL_RECOVERY_PR_STATE"
      printf 'Fresh fetched origin/%s tip: %s\n' "$RECOVERY_BRANCH" "$FRESH_RECOVERY_BRANCH_TIP"
      printf 'Actual PR head: %s\n' "$ACTUAL_RECOVERY_PR_HEAD"
      printf 'Actual branch: %s\n' "$ACTUAL_RECOVERY_PR_BRANCH"
      printf 'Actual owner: %s\n' "$ACTUAL_RECOVERY_PR_OWNER"
      printf 'Actual cross-repo: %s\n' "$ACTUAL_RECOVERY_PR_CROSS"
      printf 'Actual base: %s\n' "$ACTUAL_RECOVERY_PR_BASE"
    } >> "$RECOVERY_PR_ADOPTION_EVIDENCE"
    if [ "$ACTUAL_RECOVERY_PR_STATE" != "OPEN" ] || \
       [ "$ACTUAL_RECOVERY_PR_HEAD" != "$FRESH_RECOVERY_BRANCH_TIP" ] || \
       [ "$ACTUAL_RECOVERY_PR_BRANCH" != "$RECOVERY_BRANCH" ] || \
       [ "$ACTUAL_RECOVERY_PR_OWNER" != "OpenCoven" ] || \
       [ "$ACTUAL_RECOVERY_PR_CROSS" != "false" ] || \
       [ "$ACTUAL_RECOVERY_PR_BASE" != "main" ]; then
      {
        printf 'Blocked %s: exact recovery-branch PR #%s failed OPEN/main/OpenCoven verification.\n' \
          "$WORKSTREAM_ID" "$RECOVERY_PR_NUMBER"
        printf 'Expected recovery branch: %s\n' "$RECOVERY_BRANCH"
        printf 'Expected fetched head: %s\n' "$FRESH_RECOVERY_BRANCH_TIP"
        printf 'Expected owner/cross-repo: OpenCoven / false\n'
        printf 'Expected base branch: main\n'
        printf 'Actual PR head: %s\n' "$ACTUAL_RECOVERY_PR_HEAD"
        printf 'Open PR evidence: viable/%s-recovery-open-prs.json\n' "$WORKSTREAM_ID"
        printf 'PR view evidence: viable/%s-recovery-pr-view.json\n' "$WORKSTREAM_ID"
      } > "$PR_BLOCKER_EVIDENCE"
      update_classification_row \
        "blocked" \
        "Recovery-branch PR verification mismatch for $RECOVERY_BRANCH; see viable/$WORKSTREAM_ID-recovery-pr-adoption.txt, viable/$WORKSTREAM_ID-recovery-open-prs.json, viable/$WORKSTREAM_ID-recovery-pr-view.json, and viable/$WORKSTREAM_ID-pr-blocker.txt." \
        "Blocked: exact recovery-branch PR was not the one OPEN/main same-repo track to continue."
      exit 0
    fi
    update_classification_row \
      "viable" \
      "$ACTUAL_RECOVERY_PR_URL" \
      "mode=continue-existing-pr; pr_kind=recovered; issue_url=$ISSUE_URL; archive_id=$ARCHIVE_ID; expected_branch=$RECOVERY_BRANCH; expected_head=$FRESH_RECOVERY_BRANCH_TIP"
    exit 0
    ;;
  0)
    printf 'Same-repo exact recovery-branch open PR count: 0\n' >> "$RECOVERY_PR_ADOPTION_EVIDENCE"
    printf 'No same-repo OPEN recovery PR currently owns %s.\n' "$RECOVERY_BRANCH" >> "$RECOVERY_PR_ADOPTION_EVIDENCE"
    ;;
  *)
    {
      printf 'Blocked %s: expected 0 or 1 same-repo open PRs for recovery branch %s, found %s before creation.\n' \
        "$WORKSTREAM_ID" "$RECOVERY_BRANCH" "$RECOVERY_PR_COUNT"
      printf 'Recovery-branch PR evidence: viable/%s-recovery-open-prs.json\n' "$WORKSTREAM_ID"
      printf 'Recovery-branch adoption evidence: viable/%s-recovery-pr-adoption.txt\n' "$WORKSTREAM_ID"
    } > "$PR_BLOCKER_EVIDENCE"
    update_classification_row \
      "blocked" \
      "Recovery-branch PR ambiguity for $RECOVERY_BRANCH; see viable/$WORKSTREAM_ID-recovery-pr-adoption.txt, viable/$WORKSTREAM_ID-recovery-open-prs.json, and viable/$WORKSTREAM_ID-pr-blocker.txt." \
      "Blocked: multiple same-repo recovery PRs already claim the exact rerun branch."
    exit 0
    ;;
esac
{
  printf 'Recovery branch: %s\n' "$RECOVERY_BRANCH"
} > "$RECOVERY_BRANCH_REF_EVIDENCE"
LOCAL_RECOVERY_HEAD=''
if git -C "$REPO" show-ref --verify --quiet "refs/heads/$RECOVERY_BRANCH"; then
  LOCAL_RECOVERY_HEAD="$(git -C "$REPO" rev-parse "refs/heads/$RECOVERY_BRANCH")"
  printf 'Local recovery branch exists: yes (%s)\n' "$LOCAL_RECOVERY_HEAD" >> "$RECOVERY_BRANCH_REF_EVIDENCE"
else
  printf 'Local recovery branch exists: no\n' >> "$RECOVERY_BRANCH_REF_EVIDENCE"
fi
REMOTE_RECOVERY_PROOF="$RECOVERY_BRANCH_REF_EVIDENCE.remote"
if ! git -C "$REPO" ls-remote --heads origin "refs/heads/$RECOVERY_BRANCH" > "$REMOTE_RECOVERY_PROOF" 2>> "$PR_BLOCKER_EVIDENCE"; then
  REMOTE_LOOKUP_ERROR="$(cat "$PR_BLOCKER_EVIDENCE")"
  rm -f "$REMOTE_RECOVERY_PROOF"
  {
    printf 'Blocked %s: could not query origin for recovery branch %s before creation.\n' \
      "$WORKSTREAM_ID" "$RECOVERY_BRANCH"
    printf 'Recovery-branch ref evidence: viable/%s-recovery-branch-refs.txt\n' "$WORKSTREAM_ID"
    printf 'Failure follows:\n%s\n' "$REMOTE_LOOKUP_ERROR"
  } > "$PR_BLOCKER_EVIDENCE"
  update_classification_row \
    "blocked" \
    "Recovery-branch ref query failed for $RECOVERY_BRANCH; see viable/$WORKSTREAM_ID-recovery-branch-refs.txt and viable/$WORKSTREAM_ID-pr-blocker.txt." \
    "Blocked: could not verify whether the exact recovery branch already exists remotely."
  exit 0
fi
REMOTE_RECOVERY_HEAD=''
if test -s "$REMOTE_RECOVERY_PROOF"; then
  REMOTE_RECOVERY_HEAD="$(awk 'NR == 1 { print $1 }' "$REMOTE_RECOVERY_PROOF")"
  printf 'Remote recovery branch exists: yes (%s)\n' "$REMOTE_RECOVERY_HEAD" >> "$RECOVERY_BRANCH_REF_EVIDENCE"
else
  printf 'Remote recovery branch exists: no\n' >> "$RECOVERY_BRANCH_REF_EVIDENCE"
fi
rm -f "$REMOTE_RECOVERY_PROOF"
if [ -n "$LOCAL_RECOVERY_HEAD" ] || [ -n "$REMOTE_RECOVERY_HEAD" ]; then
  {
    printf 'Blocked %s: exact recovery branch %s already exists without exactly one adoptable OPEN recovery PR.\n' \
      "$WORKSTREAM_ID" "$RECOVERY_BRANCH"
    printf 'Local recovery branch head: %s\n' "${LOCAL_RECOVERY_HEAD:-absent}"
    printf 'Remote recovery branch head: %s\n' "${REMOTE_RECOVERY_HEAD:-absent}"
    printf 'Recovery-branch PR evidence: viable/%s-recovery-open-prs.json\n' "$WORKSTREAM_ID"
    printf 'Recovery-branch ref evidence: viable/%s-recovery-branch-refs.txt\n' "$WORKSTREAM_ID"
    printf 'Resume instruction: resume or reconcile the existing %s branch, publish or inspect its PR state, then rerun this step instead of creating a duplicate branch/worktree/claim.\n' "$RECOVERY_BRANCH"
  } > "$PR_BLOCKER_EVIDENCE"
  update_classification_row \
    "blocked" \
    "Recovery branch $RECOVERY_BRANCH already exists locally or remotely without exactly one adoptable OPEN recovery PR; see viable/$WORKSTREAM_ID-recovery-open-prs.json, viable/$WORKSTREAM_ID-recovery-branch-refs.txt, and viable/$WORKSTREAM_ID-pr-blocker.txt." \
    "Blocked: reconcile or resume the existing exact recovery branch before rerunning child recovery creation."
  exit 0
fi
if test -e "$RECOVERY_WORKTREE"; then
  printf 'Recovery worktree path already exists: %s\n' "$RECOVERY_WORKTREE" >&2
  exit 1
fi
if git -C "$REPO" worktree list --porcelain | awk -v path="$RECOVERY_WORKTREE" '
    $1 == "worktree" && $2 == path { found = 1 }
    END { exit found ? 0 : 1 }
  '; then
  printf 'Recovery worktree is already registered by Git: %s\n' "$RECOVERY_WORKTREE" >&2
  exit 1
fi
if ! git -C "$REPO" fetch origin main; then
  printf 'Failed to fetch origin/main before creating %s.\n' "$RECOVERY_BRANCH" >&2
  exit 1
fi
if ! git -C "$REPO" worktree add \
  -b "$RECOVERY_BRANCH" \
  "$RECOVERY_WORKTREE" \
  origin/main
then
  printf 'Failed to create recovery worktree %s.\n' "$RECOVERY_WORKTREE" >&2
  exit 1
fi
cd "$RECOVERY_WORKTREE"
if ! coven claim acquire "issue-$ISSUE_NUMBER"; then
  {
    printf 'Blocked %s: coven claim acquire issue-%s failed.\n' \
      "$WORKSTREAM_ID" "$ISSUE_NUMBER"
    printf 'Recovery worktree: %s\n' "$RECOVERY_WORKTREE"
    printf 'Recovery branch: %s\n' "$RECOVERY_BRANCH"
  } > "$CLAIM_BLOCKER_EVIDENCE"
  update_classification_row \
    "blocked" \
    "Claim acquisition failed; see viable/$WORKSTREAM_ID-claim-blocker.txt." \
    "Blocked: child claim issue-$ISSUE_NUMBER could not be acquired."
  if [ -n "$(git -C "$RECOVERY_WORKTREE" status --porcelain --untracked-files=all)" ]; then
    printf 'Cleanup blocked: child worktree is no longer clean; leave it in place for inspection.\n' \
      >> "$CLAIM_BLOCKER_EVIDENCE"
    exit 1
  fi
  cd "$REPO"
  if ! git -C "$REPO" worktree remove "$RECOVERY_WORKTREE"; then
    printf 'Cleanup blocked: could not remove clean child worktree %s without force.\n' \
      "$RECOVERY_WORKTREE" >> "$CLAIM_BLOCKER_EVIDENCE"
    exit 1
  fi
  if ! git -C "$REPO" branch -d "$RECOVERY_BRANCH"; then
    printf 'Cleanup blocked: could not delete clean child branch %s without force.\n' \
      "$RECOVERY_BRANCH" >> "$CLAIM_BLOCKER_EVIDENCE"
    exit 1
  fi
  exit 0
fi
```

For long child recovery sessions, keep the child claim alive from that child
worktree:

```bash
set -euo pipefail
CURRENT_BRANCH="$(git branch --show-current)"
case "$CURRENT_BRANCH" in
  issue-[0-9]*-*)
    ISSUE_NUMBER="${CURRENT_BRANCH#issue-}"
    ISSUE_NUMBER="${ISSUE_NUMBER%%-*}"
    ;;
  *)
    printf 'Blocked: current branch %s does not match issue-<number>-<slug>.\n' "$CURRENT_BRANCH" >&2
    exit 1
    ;;
esac
coven claim heartbeat "issue-$ISSUE_NUMBER"
```

Keep that child claim active after PR creation while follow-up commits, review
responses, or final verification continue in the same session. Release it only
when that pull request merges or the owning recovery session stops:

```bash
set -euo pipefail
CURRENT_BRANCH="$(git branch --show-current)"
case "$CURRENT_BRANCH" in
  issue-[0-9]*-*)
    ISSUE_NUMBER="${CURRENT_BRANCH#issue-}"
    ISSUE_NUMBER="${ISSUE_NUMBER%%-*}"
    ;;
  *)
    printf 'Blocked: current branch %s does not match issue-<number>-<slug>.\n' "$CURRENT_BRANCH" >&2
    exit 1
    ;;
esac
coven claim release "issue-$ISSUE_NUMBER"
```

Rows that rewrite to `mode=continue-existing-pr; pr_kind=recovered` after the
rerun recovery-branch check already belong to one exact OPEN recovery PR and
must skip new branch, worktree, and claim creation entirely. Continue into the
child recovery plan only after `coven claim acquire` succeeds for a newly
created branch/worktree. If acquisition fails, persist
`viable/$WORKSTREAM_ID-claim-blocker.txt`, rewrite the classification row to
`blocked`, remove the newly created still-clean child worktree and its exact
local branch without force, and stop that row. If either cleanup command
fails, leave the residue in place, keep the row blocked, and treat that as an
operator-visible blocker.

Execute that issue's plan, run its full gates, commit each child change with
the Task 5 Step 4 trailer pattern, add human contributor co-author trailers
only when `AGENTS.md` requires them, push, and open its scoped pull request.
Immediately after any non-adopted recovery branch opens its PR, capture and
verify that PR against a freshly fetched exact branch tip before Task 7 cleanup
or Task 9 audit continues:

```bash
set -euo pipefail
resolve_control_worktree() {
  local control_branch="docs/541-incomplete-work-recovery-design"
  local expected_branch="refs/heads/$control_branch"
  local remote_branch_ref="refs/remotes/origin/$control_branch"
  local start_common_dir repo target_path path branch_ref actual_branch
  local target_registration_branch live_count stale_count live_path stale_path
  local live_status live_head local_head remote_head
  local -a live_paths=() stale_paths=()
  start_common_dir="$(git rev-parse --git-common-dir)"
  repo="$(cd "$start_common_dir/.." && pwd)"
  target_path="$repo/.worktrees/issue-541-recovery"
  if ! git -C "$repo" fetch --no-tags origin \
    "refs/heads/$control_branch:$remote_branch_ref"
  then
    printf 'Blocked: could not fetch origin/%s into %s.\n' \
      "$control_branch" "$remote_branch_ref" >&2
    exit 1
  fi
  remote_head="$(git -C "$repo" rev-parse --verify "$remote_branch_ref^{commit}")"
  while IFS="$(printf "\t")" read -r path branch_ref; do
    test -n "$path" || continue
    if test "$path" = "$target_path"; then
      target_registration_branch="$branch_ref"
    fi
    if test "$branch_ref" != "$expected_branch"; then
      continue
    fi
    if test -d "$path"; then
      live_paths+=("$path")
    else
      stale_paths+=("$path")
    fi
  done <<EOF2
$(git -C "$repo" worktree list --porcelain | awk '
  $1 == "worktree" { if (path != "") print path "\t" branch; path = substr($0, 10); branch = ""; next }
  $1 == "branch" { branch = $2; next }
  END { if (path != "") print path "\t" branch }
')
EOF2
  live_count="${#live_paths[@]}"
  stale_count="${#stale_paths[@]}"
  if test -n "$target_registration_branch" && test "$target_registration_branch" != "$expected_branch"; then
    printf 'Blocked: %s is registered to %s, not %s.\n' "$target_path" "$target_registration_branch" "$control_branch" >&2
    exit 1
  fi
  if test "$live_count" -gt 1; then
    printf 'Blocked: %s has %s live controller worktree registrations; refusing to choose arbitrarily.\n' "$control_branch" "$live_count" >&2
    for live_path in "${live_paths[@]}"; do
      printf '  live: %s\n' "$live_path" >&2
    done
    for stale_path in "${stale_paths[@]}"; do
      printf '  stale: %s\n' "$stale_path" >&2
    done
    exit 1
  fi
  if test "$live_count" -eq 1; then
    live_path="${live_paths[0]}"
    if test "$stale_count" -gt 0; then
      printf 'Warning: ignoring %s stale controller registration(s) for %s while using the one live worktree.\n' "$stale_count" "$control_branch" >&2
      for stale_path in "${stale_paths[@]}"; do
        printf '  stale: %s\n' "$stale_path" >&2
      done
    fi
    actual_branch="$(git -C "$live_path" branch --show-current)"
    if test "$actual_branch" != "$control_branch"; then
      printf 'Control worktree branch mismatch: expected %s, got %s\n' "$control_branch" "$actual_branch" >&2
      exit 1
    fi
    live_status="$(git -C "$live_path" status --porcelain=v1 --untracked-files=all)"
    if test -n "$live_status"; then
      printf 'Blocked: live controller worktree is dirty and cannot be refreshed safely: %s\n' "$live_path" >&2
      git -C "$live_path" status --short --branch --untracked-files=all >&2
      exit 1
    fi
    live_head="$(git -C "$live_path" rev-parse HEAD)"
    if test "$live_head" = "$remote_head"; then
      :
    elif git -C "$repo" merge-base --is-ancestor "$live_head" "$remote_head"; then
      if ! git -C "$live_path" merge --ff-only "$remote_branch_ref"; then
        printf 'Blocked: live controller worktree could not fast-forward %s to fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
        exit 1
      fi
    elif git -C "$repo" merge-base --is-ancestor "$remote_head" "$live_head"; then
      printf 'Blocked: live controller branch %s is ahead of fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
      exit 1
    else
      printf 'Blocked: live controller branch %s diverged from fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
      exit 1
    fi
    live_head="$(git -C "$live_path" rev-parse HEAD)"
    if test "$live_head" != "$remote_head"; then
      printf 'Blocked: live controller HEAD %s does not match fetched origin tip %s.\n' "$live_head" "$remote_head" >&2
      exit 1
    fi
    printf '%s\n' "$live_path"
    return 0
  fi
  if test "$stale_count" -gt 1; then
    printf 'Blocked: %s has %s stale controller registrations and no live worktree; refusing to choose arbitrarily.\n' "$control_branch" "$stale_count" >&2
    for stale_path in "${stale_paths[@]}"; do
      printf '  stale: %s\n' "$stale_path" >&2
    done
    exit 1
  fi
  if ! git -C "$repo" check-ignore -q .worktrees/; then
    printf 'Repository-local .worktrees/ is not ignored.\n' >&2
    exit 1
  fi
  if ! git -C "$repo" check-ignore -q .worktrees/issue-541-recovery; then
    printf 'Repository-local control worktree path is not ignored: %s\n' '.worktrees/issue-541-recovery' >&2
    exit 1
  fi
  mkdir -p "$repo/.worktrees"
  if test -e "$target_path"; then
    printf 'Control worktree path already exists without a matching live registration: %s\n' "$target_path" >&2
    exit 1
  fi
  if git -C "$repo" show-ref --verify --quiet "$expected_branch"; then
    local_head="$(git -C "$repo" rev-parse --verify "$expected_branch^{commit}")"
    if test "$local_head" = "$remote_head"; then
      :
    elif git -C "$repo" merge-base --is-ancestor "$local_head" "$remote_head"; then
      if ! git -C "$repo" update-ref "$expected_branch" "$remote_head" "$local_head"; then
        printf 'Blocked: local controller branch %s could not be fast-forwarded to fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
        exit 1
      fi
    elif git -C "$repo" merge-base --is-ancestor "$remote_head" "$local_head"; then
      printf 'Blocked: local controller branch %s is ahead of fetched origin tip %s and cannot be rewritten safely.\n' "$control_branch" "$remote_head" >&2
      exit 1
    else
      printf 'Blocked: local controller branch %s diverged from fetched origin tip %s and cannot be rewritten safely.\n' "$control_branch" "$remote_head" >&2
      exit 1
    fi
  else
    if ! git -C "$repo" update-ref "$expected_branch" "$remote_head"; then
      printf 'Blocked: local controller branch %s could not be created at fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
      exit 1
    fi
  fi
  local_head="$(git -C "$repo" rev-parse --verify "$expected_branch^{commit}")"
  if test "$local_head" != "$remote_head"; then
    printf 'Blocked: local controller branch %s resolved to %s instead of fetched origin tip %s.\n' "$control_branch" "$local_head" "$remote_head" >&2
    exit 1
  fi
  if test "$stale_count" -eq 1; then
    stale_path="${stale_paths[0]}"
    printf 'Recreating %s at %s from the one stale controller registration:\n  stale: %s\n' "$control_branch" "$target_path" "$stale_path" >&2
    git -C "$repo" worktree add --force "$target_path" "$control_branch"
  else
    git -C "$repo" worktree add "$target_path" "$control_branch"
  fi
  if ! test -d "$target_path"; then
    printf 'Control worktree path is not present after resolution: %s\n' "$target_path" >&2
    exit 1
  fi
  actual_branch="$(git -C "$target_path" branch --show-current)"
  if test "$actual_branch" != "$control_branch"; then
    printf 'Control worktree branch mismatch: expected %s, got %s\n' "$control_branch" "$actual_branch" >&2
    exit 1
  fi
  local_head="$(git -C "$target_path" rev-parse HEAD)"
  if test "$local_head" != "$remote_head"; then
    printf 'Blocked: resolved controller HEAD %s does not match fetched origin tip %s.\n' "$local_head" "$remote_head" >&2
    exit 1
  fi
  printf '%s\n' "$target_path"
}
CONTROL_WORKTREE="$(resolve_control_worktree)"
COMMON_DIR="$(git -C "$CONTROL_WORKTREE" rev-parse --git-common-dir)"
REPO="$(cd "$COMMON_DIR/.." && pwd)"
RECOVERY="$COMMON_DIR/agent-recovery/issue-541/viable"
CLASSIFICATION="$COMMON_DIR/agent-recovery/issue-541/classification.md"
ISSUE_VIEW_EVIDENCE="$RECOVERY/$WORKSTREAM_ID-issue-view.json"
PR_VIEW_EVIDENCE="$RECOVERY/$WORKSTREAM_ID-pr-view.json"
PR_BLOCKER_EVIDENCE="$RECOVERY/$WORKSTREAM_ID-pr-blocker.txt"
RECOVERY_BRANCH_REF_EVIDENCE="$RECOVERY/$WORKSTREAM_ID-recovery-branch-refs.txt"
RECOVERY_BRANCH="issue-$ISSUE_NUMBER-$BRANCH_SLUG"
case "$WORKSTREAM_ID" in
  mobile-memory-gateway)
    ARCHIVE_ID="dirty/mobile-memory-gateway"
    ;;
  feat-npm-macos-x64)
    ARCHIVE_ID="branches/feat-npm-macos-x64.bundle"
    ;;
  fix-521-ward-surface-confinement)
    ARCHIVE_ID="branches/fix-521-ward-surface-confinement.bundle"
    ;;
  memory-promote)
    ARCHIVE_ID="dirty/memory-promote"
    ;;
  docs-psyche-specs)
    ARCHIVE_ID="dirty/docs-psyche-specs"
    ;;
  docs-universal-runtime-capability-design)
    ARCHIVE_ID="branches/docs-universal-runtime-capability-design.bundle"
    ;;
  pr-476-review)
    ARCHIVE_ID="dirty/pr-476-review"
    ;;
  *)
    printf 'Unknown WORKSTREAM_ID: %s\n' "$WORKSTREAM_ID" >&2
    exit 1
    ;;
esac
mkdir -p "$RECOVERY"
update_classification_row() {
  python3 - "$CLASSIFICATION" "$WORKSTREAM_ID" "$1" "$2" "$3" <<'PY'
from pathlib import Path
import sys

path = Path(sys.argv[1])
workstream, classification, evidence, action = sys.argv[2:6]
needle = f"| {workstream} |"
lines = path.read_text().splitlines()
for idx, line in enumerate(lines):
    if line.startswith(needle):
        parts = [part.strip() for part in line.strip().strip("|").split("|")]
        if len(parts) != 5:
            raise SystemExit(f"Unexpected classification row: {line}")
        parts[1] = classification
        parts[2] = evidence
        parts[4] = action
        lines[idx] = "| " + " | ".join(parts) + " |"
        path.write_text("\n".join(lines) + "\n")
        break
else:
    raise SystemExit(f"Missing classification row for {workstream}")
PY
}
test -s "$ISSUE_VIEW_EVIDENCE"
ISSUE_URL="$(jq -r '.url' "$ISSUE_VIEW_EVIDENCE")"
if [ -z "${RECOVERY_PR_URL:-}" ]; then
  if ! RECOVERY_PR_URL="$(gh pr view --repo OpenCoven/coven "$RECOVERY_BRANCH" \
    --json number,title,url,state,headRefName,headRepositoryOwner,isCrossRepository,baseRefName --jq '.url' 2> "$PR_BLOCKER_EVIDENCE")"; then
    PR_VIEW_ERROR="$(cat "$PR_BLOCKER_EVIDENCE")"
    {
      printf 'Blocked %s: no recovery PR URL was captured from gh pr create, and gh pr view could not recover it.\n' \
        "$WORKSTREAM_ID"
      printf 'Expected recovery branch: %s\n' "$RECOVERY_BRANCH"
      printf 'Issue evidence: viable/%s-issue-view.json\n' "$WORKSTREAM_ID"
      printf 'gh pr view failure follows:\n%s\n' "$PR_VIEW_ERROR"
    } > "$PR_BLOCKER_EVIDENCE"
    update_classification_row \
      "blocked" \
      "Recovery PR URL capture failed; see viable/$WORKSTREAM_ID-issue-view.json and viable/$WORKSTREAM_ID-pr-blocker.txt." \
      "Blocked: recovery PR URL could not be recovered for $RECOVERY_BRANCH."
    exit 0
  fi
fi
{
  printf '\nPost-push recovery PR verification fetch follows.\n'
  printf 'Fetching origin/%s immediately before PR verification.\n' "$RECOVERY_BRANCH"
} >> "$RECOVERY_BRANCH_REF_EVIDENCE"
if ! git -C "$REPO" fetch --no-tags origin \
  "refs/heads/$RECOVERY_BRANCH:refs/remotes/origin/$RECOVERY_BRANCH" \
  >> "$RECOVERY_BRANCH_REF_EVIDENCE" 2>&1; then
  FETCH_OUTPUT="$(cat "$RECOVERY_BRANCH_REF_EVIDENCE")"
  {
    printf 'Blocked %s: origin/%s could not be fetched immediately before recovery PR verification.\n' \
      "$WORKSTREAM_ID" "$RECOVERY_BRANCH"
    printf 'Issue evidence: viable/%s-issue-view.json\n' "$WORKSTREAM_ID"
    printf 'Recovery-branch ref evidence: viable/%s-recovery-branch-refs.txt\n' "$WORKSTREAM_ID"
    printf 'Fetch output follows:\n%s\n' "$FETCH_OUTPUT"
  } > "$PR_BLOCKER_EVIDENCE"
  update_classification_row \
    "blocked" \
    "Recovery PR fetch failed for $RECOVERY_BRANCH; see viable/$WORKSTREAM_ID-recovery-branch-refs.txt and viable/$WORKSTREAM_ID-pr-blocker.txt." \
    "Blocked: recovery PR branch could not be refetched before verification."
  exit 0
fi
FRESH_RECOVERY_BRANCH_TIP="$(git -C "$REPO" rev-parse "refs/remotes/origin/$RECOVERY_BRANCH")"
printf 'Fresh fetched origin/%s tip after push: %s\n' \
  "$RECOVERY_BRANCH" "$FRESH_RECOVERY_BRANCH_TIP" >> "$RECOVERY_BRANCH_REF_EVIDENCE"
if ! gh pr view "$RECOVERY_PR_URL" --repo OpenCoven/coven \
  --json number,url,state,headRefOid,headRefName,headRepositoryOwner,isCrossRepository,baseRefName \
  > "$PR_VIEW_EVIDENCE" 2> "$PR_BLOCKER_EVIDENCE"; then
  PR_VIEW_ERROR="$(cat "$PR_BLOCKER_EVIDENCE")"
  {
    printf 'Blocked %s: recovery PR URL %s could not be verified.\n' \
      "$WORKSTREAM_ID" "$RECOVERY_PR_URL"
    printf 'Expected recovery branch: %s\n' "$RECOVERY_BRANCH"
    printf 'Fresh fetched origin/%s tip: %s\n' "$RECOVERY_BRANCH" "$FRESH_RECOVERY_BRANCH_TIP"
    printf 'Issue evidence: viable/%s-issue-view.json\n' "$WORKSTREAM_ID"
    printf 'gh pr view failure follows:\n%s\n' "$PR_VIEW_ERROR"
  } > "$PR_BLOCKER_EVIDENCE"
  update_classification_row \
    "blocked" \
    "Recovery PR verification failed; see viable/$WORKSTREAM_ID-recovery-branch-refs.txt, viable/$WORKSTREAM_ID-pr-view.json, and viable/$WORKSTREAM_ID-pr-blocker.txt." \
    "Blocked: recovery PR URL could not be verified for $RECOVERY_BRANCH."
  exit 0
fi
RECOVERY_PR_NUMBER="$(jq -r '.number' "$PR_VIEW_EVIDENCE")"
ACTUAL_PR_URL="$(jq -r '.url' "$PR_VIEW_EVIDENCE")"
ACTUAL_STATE="$(jq -r '.state' "$PR_VIEW_EVIDENCE")"
ACTUAL_HEAD="$(jq -r '.headRefOid' "$PR_VIEW_EVIDENCE")"
ACTUAL_BRANCH="$(jq -r '.headRefName' "$PR_VIEW_EVIDENCE")"
ACTUAL_OWNER="$(jq -r '.headRepositoryOwner.login' "$PR_VIEW_EVIDENCE")"
ACTUAL_CROSS="$(jq -r '.isCrossRepository' "$PR_VIEW_EVIDENCE")"
ACTUAL_BASE="$(jq -r '.baseRefName' "$PR_VIEW_EVIDENCE")"
if [ "$ACTUAL_STATE" != "OPEN" ] || \
   [ "$ACTUAL_HEAD" != "$FRESH_RECOVERY_BRANCH_TIP" ] || \
   [ "$ACTUAL_BRANCH" != "$RECOVERY_BRANCH" ] || \
   [ "$ACTUAL_OWNER" != "OpenCoven" ] || \
   [ "$ACTUAL_CROSS" != "false" ] || \
   [ "$ACTUAL_BASE" != "main" ]; then
  {
    printf 'Blocked %s: recovery PR verification failed.\n' "$WORKSTREAM_ID"
    printf 'Expected PR URL: %s\n' "$RECOVERY_PR_URL"
    printf 'Expected recovery branch: %s\n' "$RECOVERY_BRANCH"
    printf 'Expected fetched head: %s\n' "$FRESH_RECOVERY_BRANCH_TIP"
    printf 'Expected base branch: main\n'
    printf 'Expected owner/cross-repo: OpenCoven / false\n'
    printf 'Expected state: OPEN\n'
    printf 'Actual PR URL: %s\n' "$ACTUAL_PR_URL"
    printf 'Actual state: %s\n' "$ACTUAL_STATE"
    printf 'Actual PR head: %s\n' "$ACTUAL_HEAD"
    printf 'Actual branch: %s\n' "$ACTUAL_BRANCH"
    printf 'Actual owner: %s\n' "$ACTUAL_OWNER"
    printf 'Actual cross-repo: %s\n' "$ACTUAL_CROSS"
    printf 'Actual base branch: %s\n' "$ACTUAL_BASE"
    printf 'PR view evidence: viable/%s-pr-view.json\n' "$WORKSTREAM_ID"
  } > "$PR_BLOCKER_EVIDENCE"
  update_classification_row \
    "blocked" \
    "Recovery PR verification mismatch; see viable/$WORKSTREAM_ID-recovery-branch-refs.txt, viable/$WORKSTREAM_ID-pr-view.json, and viable/$WORKSTREAM_ID-pr-blocker.txt." \
    "Blocked: recovery PR was not the OPEN main-targeting same-repo PR at the fetched $RECOVERY_BRANCH tip."
  exit 0
fi
update_classification_row \
  "viable" \
  "$ACTUAL_PR_URL" \
  "mode=recovery-pr-open; pr_kind=recovered; issue_url=$ISSUE_URL; archive_id=$ARCHIVE_ID; expected_branch=$RECOVERY_BRANCH; expected_head=$FRESH_RECOVERY_BRANCH_TIP"
```

Expected: every viable row either continues one adopted same-repo
exact-source-branch open pull request after the single-candidate verification
and empty-dirty-snapshot flow, continues one adopted same-repo exact
recovery-branch open pull request on rerun after the recovery-branch
single-candidate verification, or rewrites its ledger row to stay `viable`
with the raw canonical recovery PR URL in `Main/PR evidence` plus a
deterministic `mode=recovery-pr-open; ... expected_branch=issue-<n>-<slug>;
expected_head=<fetched-tip>` action after the zero-candidate normal flow, but
only after an exact `origin/$RECOVERY_BRANCH` fetch immediately precedes PR
verification and the PR `headRefOid` equals that fetched tip. The same fetched
tip rule also applies when reruns adopt an existing recovery-branch PR before
Task 7 cleanup or Task 9 audit begins.

### Task 6: Record Non-Viable Outcomes

**Artifacts:**
- Modify: `.git/agent-recovery/issue-541/classification.md`

- [ ] **Step 1: Record already-shipped proof**

For each `already-shipped` row, replace any narrative cleanup authorization
with this structured ledger state:

- `Main/PR evidence`: either the raw merged `https://github.com/OpenCoven/coven/pull/<n>`
  URL or the exact 40-character commit SHA that is already on `main`.
- `Recovery action`: `mode=non-viable-proof; classification=already-shipped; evidence_kind=merged-pr`
  or `mode=non-viable-proof; classification=already-shipped; evidence_kind=main-commit`.
- Optional human explanation may remain only as extra `key=value` metadata such
  as `note=...`; it must not be the authorization for cleanup.

- [ ] **Step 2: Record supersession proof**

For each `superseded` row, replace any narrative cleanup authorization with
this structured ledger state:

- `Main/PR evidence`: either the raw merged `https://github.com/OpenCoven/coven/pull/<n>`
  URL or the exact 40-character commit SHA that is already on `main` and
  represents the superseding contract.
- `Recovery action`: `mode=non-viable-proof; classification=superseded; evidence_kind=merged-pr`
  or `mode=non-viable-proof; classification=superseded; evidence_kind=main-commit`.
- Optional human explanation may remain only as extra `key=value` metadata such
  as `note=...`; it must not be the authorization for cleanup.

- [ ] **Step 3: Record blockers**

For each `blocked` row, add:

```markdown
- Missing authority or decision:
- Evidence that the agent cannot infer it:
- Preserved snapshot:
- Safe resume condition:
```

Expected: no non-viable row relies on branch age or lack of a PR as its sole
reason. `already-shipped` and `superseded` cleanup authorization now comes
only from the exact merged PR URL or exact main commit SHA recorded in
`Main/PR evidence` plus the parsed `mode=non-viable-proof` metadata in
`Recovery action`. Rows blocked because an active PR leaves preserved dirty
delta behind must cite that PR URL, the preserved snapshot archive IDs, and
the explicit resume condition to recover the delta later from current `main`.

### Task 7: Clean Verified Git Residue

**Files:** None. This task changes only local Git worktree and branch metadata.

- [ ] **Step 1: Recheck claims and open pull requests**

Run:

```bash
set -euo pipefail
resolve_control_worktree() {
  local control_branch="docs/541-incomplete-work-recovery-design"
  local expected_branch="refs/heads/$control_branch"
  local remote_branch_ref="refs/remotes/origin/$control_branch"
  local start_common_dir repo target_path path branch_ref actual_branch
  local target_registration_branch live_count stale_count live_path stale_path
  local live_status live_head local_head remote_head
  local -a live_paths=() stale_paths=()
  start_common_dir="$(git rev-parse --git-common-dir)"
  repo="$(cd "$start_common_dir/.." && pwd)"
  target_path="$repo/.worktrees/issue-541-recovery"
  if ! git -C "$repo" fetch --no-tags origin \
    "refs/heads/$control_branch:$remote_branch_ref"
  then
    printf 'Blocked: could not fetch origin/%s into %s.\n' \
      "$control_branch" "$remote_branch_ref" >&2
    exit 1
  fi
  remote_head="$(git -C "$repo" rev-parse --verify "$remote_branch_ref^{commit}")"
  while IFS="$(printf "\t")" read -r path branch_ref; do
    test -n "$path" || continue
    if test "$path" = "$target_path"; then
      target_registration_branch="$branch_ref"
    fi
    if test "$branch_ref" != "$expected_branch"; then
      continue
    fi
    if test -d "$path"; then
      live_paths+=("$path")
    else
      stale_paths+=("$path")
    fi
  done <<EOF2
$(git -C "$repo" worktree list --porcelain | awk '
  $1 == "worktree" { if (path != "") print path "\t" branch; path = substr($0, 10); branch = ""; next }
  $1 == "branch" { branch = $2; next }
  END { if (path != "") print path "\t" branch }
')
EOF2
  live_count="${#live_paths[@]}"
  stale_count="${#stale_paths[@]}"
  if test -n "$target_registration_branch" && test "$target_registration_branch" != "$expected_branch"; then
    printf 'Blocked: %s is registered to %s, not %s.\n' "$target_path" "$target_registration_branch" "$control_branch" >&2
    exit 1
  fi
  if test "$live_count" -gt 1; then
    printf 'Blocked: %s has %s live controller worktree registrations; refusing to choose arbitrarily.\n' "$control_branch" "$live_count" >&2
    for live_path in "${live_paths[@]}"; do
      printf '  live: %s\n' "$live_path" >&2
    done
    for stale_path in "${stale_paths[@]}"; do
      printf '  stale: %s\n' "$stale_path" >&2
    done
    exit 1
  fi
  if test "$live_count" -eq 1; then
    live_path="${live_paths[0]}"
    if test "$stale_count" -gt 0; then
      printf 'Warning: ignoring %s stale controller registration(s) for %s while using the one live worktree.\n' "$stale_count" "$control_branch" >&2
      for stale_path in "${stale_paths[@]}"; do
        printf '  stale: %s\n' "$stale_path" >&2
      done
    fi
    actual_branch="$(git -C "$live_path" branch --show-current)"
    if test "$actual_branch" != "$control_branch"; then
      printf 'Control worktree branch mismatch: expected %s, got %s\n' "$control_branch" "$actual_branch" >&2
      exit 1
    fi
    live_status="$(git -C "$live_path" status --porcelain=v1 --untracked-files=all)"
    if test -n "$live_status"; then
      printf 'Blocked: live controller worktree is dirty and cannot be refreshed safely: %s\n' "$live_path" >&2
      git -C "$live_path" status --short --branch --untracked-files=all >&2
      exit 1
    fi
    live_head="$(git -C "$live_path" rev-parse HEAD)"
    if test "$live_head" = "$remote_head"; then
      :
    elif git -C "$repo" merge-base --is-ancestor "$live_head" "$remote_head"; then
      if ! git -C "$live_path" merge --ff-only "$remote_branch_ref"; then
        printf 'Blocked: live controller worktree could not fast-forward %s to fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
        exit 1
      fi
    elif git -C "$repo" merge-base --is-ancestor "$remote_head" "$live_head"; then
      printf 'Blocked: live controller branch %s is ahead of fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
      exit 1
    else
      printf 'Blocked: live controller branch %s diverged from fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
      exit 1
    fi
    live_head="$(git -C "$live_path" rev-parse HEAD)"
    if test "$live_head" != "$remote_head"; then
      printf 'Blocked: live controller HEAD %s does not match fetched origin tip %s.\n' "$live_head" "$remote_head" >&2
      exit 1
    fi
    printf '%s\n' "$live_path"
    return 0
  fi
  if test "$stale_count" -gt 1; then
    printf 'Blocked: %s has %s stale controller registrations and no live worktree; refusing to choose arbitrarily.\n' "$control_branch" "$stale_count" >&2
    for stale_path in "${stale_paths[@]}"; do
      printf '  stale: %s\n' "$stale_path" >&2
    done
    exit 1
  fi
  if ! git -C "$repo" check-ignore -q .worktrees/; then
    printf 'Repository-local .worktrees/ is not ignored.\n' >&2
    exit 1
  fi
  if ! git -C "$repo" check-ignore -q .worktrees/issue-541-recovery; then
    printf 'Repository-local control worktree path is not ignored: %s\n' '.worktrees/issue-541-recovery' >&2
    exit 1
  fi
  mkdir -p "$repo/.worktrees"
  if test -e "$target_path"; then
    printf 'Control worktree path already exists without a matching live registration: %s\n' "$target_path" >&2
    exit 1
  fi
  if git -C "$repo" show-ref --verify --quiet "$expected_branch"; then
    local_head="$(git -C "$repo" rev-parse --verify "$expected_branch^{commit}")"
    if test "$local_head" = "$remote_head"; then
      :
    elif git -C "$repo" merge-base --is-ancestor "$local_head" "$remote_head"; then
      if ! git -C "$repo" update-ref "$expected_branch" "$remote_head" "$local_head"; then
        printf 'Blocked: local controller branch %s could not be fast-forwarded to fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
        exit 1
      fi
    elif git -C "$repo" merge-base --is-ancestor "$remote_head" "$local_head"; then
      printf 'Blocked: local controller branch %s is ahead of fetched origin tip %s and cannot be rewritten safely.\n' "$control_branch" "$remote_head" >&2
      exit 1
    else
      printf 'Blocked: local controller branch %s diverged from fetched origin tip %s and cannot be rewritten safely.\n' "$control_branch" "$remote_head" >&2
      exit 1
    fi
  else
    if ! git -C "$repo" update-ref "$expected_branch" "$remote_head"; then
      printf 'Blocked: local controller branch %s could not be created at fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
      exit 1
    fi
  fi
  local_head="$(git -C "$repo" rev-parse --verify "$expected_branch^{commit}")"
  if test "$local_head" != "$remote_head"; then
    printf 'Blocked: local controller branch %s resolved to %s instead of fetched origin tip %s.\n' "$control_branch" "$local_head" "$remote_head" >&2
    exit 1
  fi
  if test "$stale_count" -eq 1; then
    stale_path="${stale_paths[0]}"
    printf 'Recreating %s at %s from the one stale controller registration:\n  stale: %s\n' "$control_branch" "$target_path" "$stale_path" >&2
    git -C "$repo" worktree add --force "$target_path" "$control_branch"
  else
    git -C "$repo" worktree add "$target_path" "$control_branch"
  fi
  if ! test -d "$target_path"; then
    printf 'Control worktree path is not present after resolution: %s\n' "$target_path" >&2
    exit 1
  fi
  actual_branch="$(git -C "$target_path" branch --show-current)"
  if test "$actual_branch" != "$control_branch"; then
    printf 'Control worktree branch mismatch: expected %s, got %s\n' "$control_branch" "$actual_branch" >&2
    exit 1
  fi
  local_head="$(git -C "$target_path" rev-parse HEAD)"
  if test "$local_head" != "$remote_head"; then
    printf 'Blocked: resolved controller HEAD %s does not match fetched origin tip %s.\n' "$local_head" "$remote_head" >&2
    exit 1
  fi
  printf '%s\n' "$target_path"
}
CONTROL_WORKTREE="$(resolve_control_worktree)"
COMMON_DIR="$(git -C "$CONTROL_WORKTREE" rev-parse --git-common-dir)"
REPO="$(cd "$COMMON_DIR/.." && pwd)"
cd "$REPO"
coven claim status
gh pr list --state open --limit 100
```

Expected: every active recovery claim and open PR is understood before cleanup.

- [ ] **Step 2: Remove prunable registrations only after snapshot verification**

Run:

```bash
set -euo pipefail
resolve_control_worktree() {
  local control_branch="docs/541-incomplete-work-recovery-design"
  local expected_branch="refs/heads/$control_branch"
  local remote_branch_ref="refs/remotes/origin/$control_branch"
  local start_common_dir repo target_path path branch_ref actual_branch
  local target_registration_branch live_count stale_count live_path stale_path
  local live_status live_head local_head remote_head
  local -a live_paths=() stale_paths=()
  start_common_dir="$(git rev-parse --git-common-dir)"
  repo="$(cd "$start_common_dir/.." && pwd)"
  target_path="$repo/.worktrees/issue-541-recovery"
  if ! git -C "$repo" fetch --no-tags origin \
    "refs/heads/$control_branch:$remote_branch_ref"
  then
    printf 'Blocked: could not fetch origin/%s into %s.\n' \
      "$control_branch" "$remote_branch_ref" >&2
    exit 1
  fi
  remote_head="$(git -C "$repo" rev-parse --verify "$remote_branch_ref^{commit}")"
  while IFS="$(printf "\t")" read -r path branch_ref; do
    test -n "$path" || continue
    if test "$path" = "$target_path"; then
      target_registration_branch="$branch_ref"
    fi
    if test "$branch_ref" != "$expected_branch"; then
      continue
    fi
    if test -d "$path"; then
      live_paths+=("$path")
    else
      stale_paths+=("$path")
    fi
  done <<EOF2
$(git -C "$repo" worktree list --porcelain | awk '
  $1 == "worktree" { if (path != "") print path "\t" branch; path = substr($0, 10); branch = ""; next }
  $1 == "branch" { branch = $2; next }
  END { if (path != "") print path "\t" branch }
')
EOF2
  live_count="${#live_paths[@]}"
  stale_count="${#stale_paths[@]}"
  if test -n "$target_registration_branch" && test "$target_registration_branch" != "$expected_branch"; then
    printf 'Blocked: %s is registered to %s, not %s.\n' "$target_path" "$target_registration_branch" "$control_branch" >&2
    exit 1
  fi
  if test "$live_count" -gt 1; then
    printf 'Blocked: %s has %s live controller worktree registrations; refusing to choose arbitrarily.\n' "$control_branch" "$live_count" >&2
    for live_path in "${live_paths[@]}"; do
      printf '  live: %s\n' "$live_path" >&2
    done
    for stale_path in "${stale_paths[@]}"; do
      printf '  stale: %s\n' "$stale_path" >&2
    done
    exit 1
  fi
  if test "$live_count" -eq 1; then
    live_path="${live_paths[0]}"
    if test "$stale_count" -gt 0; then
      printf 'Warning: ignoring %s stale controller registration(s) for %s while using the one live worktree.\n' "$stale_count" "$control_branch" >&2
      for stale_path in "${stale_paths[@]}"; do
        printf '  stale: %s\n' "$stale_path" >&2
      done
    fi
    actual_branch="$(git -C "$live_path" branch --show-current)"
    if test "$actual_branch" != "$control_branch"; then
      printf 'Control worktree branch mismatch: expected %s, got %s\n' "$control_branch" "$actual_branch" >&2
      exit 1
    fi
    live_status="$(git -C "$live_path" status --porcelain=v1 --untracked-files=all)"
    if test -n "$live_status"; then
      printf 'Blocked: live controller worktree is dirty and cannot be refreshed safely: %s\n' "$live_path" >&2
      git -C "$live_path" status --short --branch --untracked-files=all >&2
      exit 1
    fi
    live_head="$(git -C "$live_path" rev-parse HEAD)"
    if test "$live_head" = "$remote_head"; then
      :
    elif git -C "$repo" merge-base --is-ancestor "$live_head" "$remote_head"; then
      if ! git -C "$live_path" merge --ff-only "$remote_branch_ref"; then
        printf 'Blocked: live controller worktree could not fast-forward %s to fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
        exit 1
      fi
    elif git -C "$repo" merge-base --is-ancestor "$remote_head" "$live_head"; then
      printf 'Blocked: live controller branch %s is ahead of fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
      exit 1
    else
      printf 'Blocked: live controller branch %s diverged from fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
      exit 1
    fi
    live_head="$(git -C "$live_path" rev-parse HEAD)"
    if test "$live_head" != "$remote_head"; then
      printf 'Blocked: live controller HEAD %s does not match fetched origin tip %s.\n' "$live_head" "$remote_head" >&2
      exit 1
    fi
    printf '%s\n' "$live_path"
    return 0
  fi
  if test "$stale_count" -gt 1; then
    printf 'Blocked: %s has %s stale controller registrations and no live worktree; refusing to choose arbitrarily.\n' "$control_branch" "$stale_count" >&2
    for stale_path in "${stale_paths[@]}"; do
      printf '  stale: %s\n' "$stale_path" >&2
    done
    exit 1
  fi
  if ! git -C "$repo" check-ignore -q .worktrees/; then
    printf 'Repository-local .worktrees/ is not ignored.\n' >&2
    exit 1
  fi
  if ! git -C "$repo" check-ignore -q .worktrees/issue-541-recovery; then
    printf 'Repository-local control worktree path is not ignored: %s\n' '.worktrees/issue-541-recovery' >&2
    exit 1
  fi
  mkdir -p "$repo/.worktrees"
  if test -e "$target_path"; then
    printf 'Control worktree path already exists without a matching live registration: %s\n' "$target_path" >&2
    exit 1
  fi
  if git -C "$repo" show-ref --verify --quiet "$expected_branch"; then
    local_head="$(git -C "$repo" rev-parse --verify "$expected_branch^{commit}")"
    if test "$local_head" = "$remote_head"; then
      :
    elif git -C "$repo" merge-base --is-ancestor "$local_head" "$remote_head"; then
      if ! git -C "$repo" update-ref "$expected_branch" "$remote_head" "$local_head"; then
        printf 'Blocked: local controller branch %s could not be fast-forwarded to fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
        exit 1
      fi
    elif git -C "$repo" merge-base --is-ancestor "$remote_head" "$local_head"; then
      printf 'Blocked: local controller branch %s is ahead of fetched origin tip %s and cannot be rewritten safely.\n' "$control_branch" "$remote_head" >&2
      exit 1
    else
      printf 'Blocked: local controller branch %s diverged from fetched origin tip %s and cannot be rewritten safely.\n' "$control_branch" "$remote_head" >&2
      exit 1
    fi
  else
    if ! git -C "$repo" update-ref "$expected_branch" "$remote_head"; then
      printf 'Blocked: local controller branch %s could not be created at fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
      exit 1
    fi
  fi
  local_head="$(git -C "$repo" rev-parse --verify "$expected_branch^{commit}")"
  if test "$local_head" != "$remote_head"; then
    printf 'Blocked: local controller branch %s resolved to %s instead of fetched origin tip %s.\n' "$control_branch" "$local_head" "$remote_head" >&2
    exit 1
  fi
  if test "$stale_count" -eq 1; then
    stale_path="${stale_paths[0]}"
    printf 'Recreating %s at %s from the one stale controller registration:\n  stale: %s\n' "$control_branch" "$target_path" "$stale_path" >&2
    git -C "$repo" worktree add --force "$target_path" "$control_branch"
  else
    git -C "$repo" worktree add "$target_path" "$control_branch"
  fi
  if ! test -d "$target_path"; then
    printf 'Control worktree path is not present after resolution: %s\n' "$target_path" >&2
    exit 1
  fi
  actual_branch="$(git -C "$target_path" branch --show-current)"
  if test "$actual_branch" != "$control_branch"; then
    printf 'Control worktree branch mismatch: expected %s, got %s\n' "$control_branch" "$actual_branch" >&2
    exit 1
  fi
  local_head="$(git -C "$target_path" rev-parse HEAD)"
  if test "$local_head" != "$remote_head"; then
    printf 'Blocked: resolved controller HEAD %s does not match fetched origin tip %s.\n' "$local_head" "$remote_head" >&2
    exit 1
  fi
  printf '%s\n' "$target_path"
}
CONTROL_WORKTREE="$(resolve_control_worktree)"
COMMON_DIR="$(git -C "$CONTROL_WORKTREE" rev-parse --git-common-dir)"
REPO="$(cd "$COMMON_DIR/.." && pwd)"
cd "$REPO"
git worktree prune --dry-run --verbose
```

Verify every path reported by `git worktree prune --dry-run --verbose` either
has a bundle/snapshot in the issue-541 archive or corresponds to a
merged/detached review with no unique work. Then run:

```bash
set -euo pipefail
git worktree prune --verbose
```

Expected: only registrations whose directories no longer exist are removed.

- [ ] **Step 3: Remove clean merged worktrees**

Check each known linked worktree. Persist private evidence for the tracked/index
status, the nonignored untracked inventory, the ignored inventory, and the tip
vs. `origin/main` proof before any removal. Any non-empty tracked/index status,
nonignored untracked inventory, or ignored inventory blocks removal and leaves
the worktree untouched:

```bash
set -euo pipefail
resolve_control_worktree() {
  local control_branch="docs/541-incomplete-work-recovery-design"
  local expected_branch="refs/heads/$control_branch"
  local remote_branch_ref="refs/remotes/origin/$control_branch"
  local start_common_dir repo target_path path branch_ref actual_branch
  local target_registration_branch live_count stale_count live_path stale_path
  local live_status live_head local_head remote_head
  local -a live_paths=() stale_paths=()
  start_common_dir="$(git rev-parse --git-common-dir)"
  repo="$(cd "$start_common_dir/.." && pwd)"
  target_path="$repo/.worktrees/issue-541-recovery"
  if ! git -C "$repo" fetch --no-tags origin \
    "refs/heads/$control_branch:$remote_branch_ref"
  then
    printf 'Blocked: could not fetch origin/%s into %s.\n' \
      "$control_branch" "$remote_branch_ref" >&2
    exit 1
  fi
  remote_head="$(git -C "$repo" rev-parse --verify "$remote_branch_ref^{commit}")"
  while IFS="$(printf "\t")" read -r path branch_ref; do
    test -n "$path" || continue
    if test "$path" = "$target_path"; then
      target_registration_branch="$branch_ref"
    fi
    if test "$branch_ref" != "$expected_branch"; then
      continue
    fi
    if test -d "$path"; then
      live_paths+=("$path")
    else
      stale_paths+=("$path")
    fi
  done <<EOF2
$(git -C "$repo" worktree list --porcelain | awk '
  $1 == "worktree" { if (path != "") print path "\t" branch; path = substr($0, 10); branch = ""; next }
  $1 == "branch" { branch = $2; next }
  END { if (path != "") print path "\t" branch }
')
EOF2
  live_count="${#live_paths[@]}"
  stale_count="${#stale_paths[@]}"
  if test -n "$target_registration_branch" && test "$target_registration_branch" != "$expected_branch"; then
    printf 'Blocked: %s is registered to %s, not %s.\n' "$target_path" "$target_registration_branch" "$control_branch" >&2
    exit 1
  fi
  if test "$live_count" -gt 1; then
    printf 'Blocked: %s has %s live controller worktree registrations; refusing to choose arbitrarily.\n' "$control_branch" "$live_count" >&2
    for live_path in "${live_paths[@]}"; do
      printf '  live: %s\n' "$live_path" >&2
    done
    for stale_path in "${stale_paths[@]}"; do
      printf '  stale: %s\n' "$stale_path" >&2
    done
    exit 1
  fi
  if test "$live_count" -eq 1; then
    live_path="${live_paths[0]}"
    if test "$stale_count" -gt 0; then
      printf 'Warning: ignoring %s stale controller registration(s) for %s while using the one live worktree.\n' "$stale_count" "$control_branch" >&2
      for stale_path in "${stale_paths[@]}"; do
        printf '  stale: %s\n' "$stale_path" >&2
      done
    fi
    actual_branch="$(git -C "$live_path" branch --show-current)"
    if test "$actual_branch" != "$control_branch"; then
      printf 'Control worktree branch mismatch: expected %s, got %s\n' "$control_branch" "$actual_branch" >&2
      exit 1
    fi
    live_status="$(git -C "$live_path" status --porcelain=v1 --untracked-files=all)"
    if test -n "$live_status"; then
      printf 'Blocked: live controller worktree is dirty and cannot be refreshed safely: %s\n' "$live_path" >&2
      git -C "$live_path" status --short --branch --untracked-files=all >&2
      exit 1
    fi
    live_head="$(git -C "$live_path" rev-parse HEAD)"
    if test "$live_head" = "$remote_head"; then
      :
    elif git -C "$repo" merge-base --is-ancestor "$live_head" "$remote_head"; then
      if ! git -C "$live_path" merge --ff-only "$remote_branch_ref"; then
        printf 'Blocked: live controller worktree could not fast-forward %s to fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
        exit 1
      fi
    elif git -C "$repo" merge-base --is-ancestor "$remote_head" "$live_head"; then
      printf 'Blocked: live controller branch %s is ahead of fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
      exit 1
    else
      printf 'Blocked: live controller branch %s diverged from fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
      exit 1
    fi
    live_head="$(git -C "$live_path" rev-parse HEAD)"
    if test "$live_head" != "$remote_head"; then
      printf 'Blocked: live controller HEAD %s does not match fetched origin tip %s.\n' "$live_head" "$remote_head" >&2
      exit 1
    fi
    printf '%s\n' "$live_path"
    return 0
  fi
  if test "$stale_count" -gt 1; then
    printf 'Blocked: %s has %s stale controller registrations and no live worktree; refusing to choose arbitrarily.\n' "$control_branch" "$stale_count" >&2
    for stale_path in "${stale_paths[@]}"; do
      printf '  stale: %s\n' "$stale_path" >&2
    done
    exit 1
  fi
  if ! git -C "$repo" check-ignore -q .worktrees/; then
    printf 'Repository-local .worktrees/ is not ignored.\n' >&2
    exit 1
  fi
  if ! git -C "$repo" check-ignore -q .worktrees/issue-541-recovery; then
    printf 'Repository-local control worktree path is not ignored: %s\n' '.worktrees/issue-541-recovery' >&2
    exit 1
  fi
  mkdir -p "$repo/.worktrees"
  if test -e "$target_path"; then
    printf 'Control worktree path already exists without a matching live registration: %s\n' "$target_path" >&2
    exit 1
  fi
  if git -C "$repo" show-ref --verify --quiet "$expected_branch"; then
    local_head="$(git -C "$repo" rev-parse --verify "$expected_branch^{commit}")"
    if test "$local_head" = "$remote_head"; then
      :
    elif git -C "$repo" merge-base --is-ancestor "$local_head" "$remote_head"; then
      if ! git -C "$repo" update-ref "$expected_branch" "$remote_head" "$local_head"; then
        printf 'Blocked: local controller branch %s could not be fast-forwarded to fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
        exit 1
      fi
    elif git -C "$repo" merge-base --is-ancestor "$remote_head" "$local_head"; then
      printf 'Blocked: local controller branch %s is ahead of fetched origin tip %s and cannot be rewritten safely.\n' "$control_branch" "$remote_head" >&2
      exit 1
    else
      printf 'Blocked: local controller branch %s diverged from fetched origin tip %s and cannot be rewritten safely.\n' "$control_branch" "$remote_head" >&2
      exit 1
    fi
  else
    if ! git -C "$repo" update-ref "$expected_branch" "$remote_head"; then
      printf 'Blocked: local controller branch %s could not be created at fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
      exit 1
    fi
  fi
  local_head="$(git -C "$repo" rev-parse --verify "$expected_branch^{commit}")"
  if test "$local_head" != "$remote_head"; then
    printf 'Blocked: local controller branch %s resolved to %s instead of fetched origin tip %s.\n' "$control_branch" "$local_head" "$remote_head" >&2
    exit 1
  fi
  if test "$stale_count" -eq 1; then
    stale_path="${stale_paths[0]}"
    printf 'Recreating %s at %s from the one stale controller registration:\n  stale: %s\n' "$control_branch" "$target_path" "$stale_path" >&2
    git -C "$repo" worktree add --force "$target_path" "$control_branch"
  else
    git -C "$repo" worktree add "$target_path" "$control_branch"
  fi
  if ! test -d "$target_path"; then
    printf 'Control worktree path is not present after resolution: %s\n' "$target_path" >&2
    exit 1
  fi
  actual_branch="$(git -C "$target_path" branch --show-current)"
  if test "$actual_branch" != "$control_branch"; then
    printf 'Control worktree branch mismatch: expected %s, got %s\n' "$control_branch" "$actual_branch" >&2
    exit 1
  fi
  local_head="$(git -C "$target_path" rev-parse HEAD)"
  if test "$local_head" != "$remote_head"; then
    printf 'Blocked: resolved controller HEAD %s does not match fetched origin tip %s.\n' "$local_head" "$remote_head" >&2
    exit 1
  fi
  printf '%s\n' "$target_path"
}
CONTROL_WORKTREE="$(resolve_control_worktree)"
COMMON_DIR="$(git -C "$CONTROL_WORKTREE" rev-parse --git-common-dir)"
REPO="$(cd "$COMMON_DIR/.." && pwd)"
for path in \
  "$REPO/.worktrees/docs-cli-core-guides" \
  "$REPO/.worktrees/memory-summary-source" \
  "$REPO/.worktrees/memory-open" \
  "$REPO/.worktrees/fix-coven-hq8-privacy-lockfile" \
  "$REPO/.worktrees/memory-api-review"
do
  git -C "$path" status --porcelain
done
```

Expected: empty output.

Then prove each branch's tip is reachable from freshly fetched `origin/main`
or, if ancestry fails, from exactly one freshly discovered same-repo merged PR
for that worktree branch, and remove the five clean worktrees:

```bash
set -euo pipefail
resolve_control_worktree() {
  local control_branch="docs/541-incomplete-work-recovery-design"
  local expected_branch="refs/heads/$control_branch"
  local remote_branch_ref="refs/remotes/origin/$control_branch"
  local start_common_dir repo target_path path branch_ref actual_branch
  local target_registration_branch live_count stale_count live_path stale_path
  local live_status live_head local_head remote_head
  local -a live_paths=() stale_paths=()
  start_common_dir="$(git rev-parse --git-common-dir)"
  repo="$(cd "$start_common_dir/.." && pwd)"
  target_path="$repo/.worktrees/issue-541-recovery"
  if ! git -C "$repo" fetch --no-tags origin \
    "refs/heads/$control_branch:$remote_branch_ref"
  then
    printf 'Blocked: could not fetch origin/%s into %s.\n' \
      "$control_branch" "$remote_branch_ref" >&2
    exit 1
  fi
  remote_head="$(git -C "$repo" rev-parse --verify "$remote_branch_ref^{commit}")"
  while IFS="$(printf "\t")" read -r path branch_ref; do
    test -n "$path" || continue
    if test "$path" = "$target_path"; then
      target_registration_branch="$branch_ref"
    fi
    if test "$branch_ref" != "$expected_branch"; then
      continue
    fi
    if test -d "$path"; then
      live_paths+=("$path")
    else
      stale_paths+=("$path")
    fi
  done <<EOF2
$(git -C "$repo" worktree list --porcelain | awk '
  $1 == "worktree" { if (path != "") print path "\t" branch; path = substr($0, 10); branch = ""; next }
  $1 == "branch" { branch = $2; next }
  END { if (path != "") print path "\t" branch }
')
EOF2
  live_count="${#live_paths[@]}"
  stale_count="${#stale_paths[@]}"
  if test -n "$target_registration_branch" && test "$target_registration_branch" != "$expected_branch"; then
    printf 'Blocked: %s is registered to %s, not %s.\n' "$target_path" "$target_registration_branch" "$control_branch" >&2
    exit 1
  fi
  if test "$live_count" -gt 1; then
    printf 'Blocked: %s has %s live controller worktree registrations; refusing to choose arbitrarily.\n' "$control_branch" "$live_count" >&2
    for live_path in "${live_paths[@]}"; do
      printf '  live: %s\n' "$live_path" >&2
    done
    for stale_path in "${stale_paths[@]}"; do
      printf '  stale: %s\n' "$stale_path" >&2
    done
    exit 1
  fi
  if test "$live_count" -eq 1; then
    live_path="${live_paths[0]}"
    if test "$stale_count" -gt 0; then
      printf 'Warning: ignoring %s stale controller registration(s) for %s while using the one live worktree.\n' "$stale_count" "$control_branch" >&2
      for stale_path in "${stale_paths[@]}"; do
        printf '  stale: %s\n' "$stale_path" >&2
      done
    fi
    actual_branch="$(git -C "$live_path" branch --show-current)"
    if test "$actual_branch" != "$control_branch"; then
      printf 'Control worktree branch mismatch: expected %s, got %s\n' "$control_branch" "$actual_branch" >&2
      exit 1
    fi
    live_status="$(git -C "$live_path" status --porcelain=v1 --untracked-files=all)"
    if test -n "$live_status"; then
      printf 'Blocked: live controller worktree is dirty and cannot be refreshed safely: %s\n' "$live_path" >&2
      git -C "$live_path" status --short --branch --untracked-files=all >&2
      exit 1
    fi
    live_head="$(git -C "$live_path" rev-parse HEAD)"
    if test "$live_head" = "$remote_head"; then
      :
    elif git -C "$repo" merge-base --is-ancestor "$live_head" "$remote_head"; then
      if ! git -C "$live_path" merge --ff-only "$remote_branch_ref"; then
        printf 'Blocked: live controller worktree could not fast-forward %s to fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
        exit 1
      fi
    elif git -C "$repo" merge-base --is-ancestor "$remote_head" "$live_head"; then
      printf 'Blocked: live controller branch %s is ahead of fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
      exit 1
    else
      printf 'Blocked: live controller branch %s diverged from fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
      exit 1
    fi
    live_head="$(git -C "$live_path" rev-parse HEAD)"
    if test "$live_head" != "$remote_head"; then
      printf 'Blocked: live controller HEAD %s does not match fetched origin tip %s.\n' "$live_head" "$remote_head" >&2
      exit 1
    fi
    printf '%s\n' "$live_path"
    return 0
  fi
  if test "$stale_count" -gt 1; then
    printf 'Blocked: %s has %s stale controller registrations and no live worktree; refusing to choose arbitrarily.\n' "$control_branch" "$stale_count" >&2
    for stale_path in "${stale_paths[@]}"; do
      printf '  stale: %s\n' "$stale_path" >&2
    done
    exit 1
  fi
  if ! git -C "$repo" check-ignore -q .worktrees/; then
    printf 'Repository-local .worktrees/ is not ignored.\n' >&2
    exit 1
  fi
  if ! git -C "$repo" check-ignore -q .worktrees/issue-541-recovery; then
    printf 'Repository-local control worktree path is not ignored: %s\n' '.worktrees/issue-541-recovery' >&2
    exit 1
  fi
  mkdir -p "$repo/.worktrees"
  if test -e "$target_path"; then
    printf 'Control worktree path already exists without a matching live registration: %s\n' "$target_path" >&2
    exit 1
  fi
  if git -C "$repo" show-ref --verify --quiet "$expected_branch"; then
    local_head="$(git -C "$repo" rev-parse --verify "$expected_branch^{commit}")"
    if test "$local_head" = "$remote_head"; then
      :
    elif git -C "$repo" merge-base --is-ancestor "$local_head" "$remote_head"; then
      if ! git -C "$repo" update-ref "$expected_branch" "$remote_head" "$local_head"; then
        printf 'Blocked: local controller branch %s could not be fast-forwarded to fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
        exit 1
      fi
    elif git -C "$repo" merge-base --is-ancestor "$remote_head" "$local_head"; then
      printf 'Blocked: local controller branch %s is ahead of fetched origin tip %s and cannot be rewritten safely.\n' "$control_branch" "$remote_head" >&2
      exit 1
    else
      printf 'Blocked: local controller branch %s diverged from fetched origin tip %s and cannot be rewritten safely.\n' "$control_branch" "$remote_head" >&2
      exit 1
    fi
  else
    if ! git -C "$repo" update-ref "$expected_branch" "$remote_head"; then
      printf 'Blocked: local controller branch %s could not be created at fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
      exit 1
    fi
  fi
  local_head="$(git -C "$repo" rev-parse --verify "$expected_branch^{commit}")"
  if test "$local_head" != "$remote_head"; then
    printf 'Blocked: local controller branch %s resolved to %s instead of fetched origin tip %s.\n' "$control_branch" "$local_head" "$remote_head" >&2
    exit 1
  fi
  if test "$stale_count" -eq 1; then
    stale_path="${stale_paths[0]}"
    printf 'Recreating %s at %s from the one stale controller registration:\n  stale: %s\n' "$control_branch" "$target_path" "$stale_path" >&2
    git -C "$repo" worktree add --force "$target_path" "$control_branch"
  else
    git -C "$repo" worktree add "$target_path" "$control_branch"
  fi
  if ! test -d "$target_path"; then
    printf 'Control worktree path is not present after resolution: %s\n' "$target_path" >&2
    exit 1
  fi
  actual_branch="$(git -C "$target_path" branch --show-current)"
  if test "$actual_branch" != "$control_branch"; then
    printf 'Control worktree branch mismatch: expected %s, got %s\n' "$control_branch" "$actual_branch" >&2
    exit 1
  fi
  local_head="$(git -C "$target_path" rev-parse HEAD)"
  if test "$local_head" != "$remote_head"; then
    printf 'Blocked: resolved controller HEAD %s does not match fetched origin tip %s.\n' "$local_head" "$remote_head" >&2
    exit 1
  fi
  printf '%s\n' "$target_path"
}
CONTROL_WORKTREE="$(resolve_control_worktree)"
COMMON_DIR="$(git -C "$CONTROL_WORKTREE" rev-parse --git-common-dir)"
REPO="$(cd "$COMMON_DIR/.." && pwd)"

RECOVERY="$COMMON_DIR/agent-recovery/issue-541"
MERGED_RETIRE_ROOT="$RECOVERY/private-merged-worktree-proof"
CLOSED_PRS_STAGE="$MERGED_RETIRE_ROOT/closed-prs.stage.json"
CLOSED_PRS_EVIDENCE="$MERGED_RETIRE_ROOT/closed-prs.json"
mkdir -p "$MERGED_RETIRE_ROOT"
rm -f "$CLOSED_PRS_STAGE"
if ! git -C "$REPO" fetch origin main; then
  echo "Blocked: could not refresh origin/main before merged-worktree retirement proof." >&2
  exit 1
fi
if ! gh api --paginate --slurp "repos/OpenCoven/coven/pulls?state=closed&per_page=100" > "$CLOSED_PRS_STAGE"; then
  rm -f "$CLOSED_PRS_STAGE"
  echo "Blocked: could not capture paginated closed PR evidence for OpenCoven/coven before merged-worktree retirement proof." >&2
  exit 1
fi
if ! test -s "$CLOSED_PRS_STAGE" || ! jq -e 'type == "array" and length > 0 and all(.[]; type == "array") and ([ .[] | length ] | add) > 0' "$CLOSED_PRS_STAGE" > /dev/null; then
  rm -f "$CLOSED_PRS_STAGE"
  echo "Blocked: closed PR evidence was empty or not a valid paginated slurped JSON array before merged-worktree retirement proof." >&2
  exit 1
fi
if ! mv "$CLOSED_PRS_STAGE" "$CLOSED_PRS_EVIDENCE"; then
  rm -f "$CLOSED_PRS_STAGE"
  echo "Blocked: could not atomically store closed PR evidence under $MERGED_RETIRE_ROOT." >&2
  exit 1
fi
ORIGIN_MAIN_COMMIT="$(git -C "$REPO" rev-parse "refs/remotes/origin/main^{commit}")"
for path in "$REPO/.worktrees/docs-cli-core-guides" "$REPO/.worktrees/memory-summary-source" "$REPO/.worktrees/memory-open" "$REPO/.worktrees/fix-coven-hq8-privacy-lockfile" "$REPO/.worktrees/memory-api-review"; do
  worktree_id="$(basename "$path")"
  PROOF_DIR="$MERGED_RETIRE_ROOT/$worktree_id"
  BLOCKER="$RECOVERY/$worktree_id-merged-retire-blocker.txt"
  MATCHES_FILE="$PROOF_DIR/merged-pr-matches.json"
  MERGED_PR_JSON="$PROOF_DIR/merged-pr.json"
  MERGED_PR_NUMBER_FILE="$PROOF_DIR/merged-pr-number.txt"
  MERGED_PR_URL_FILE="$PROOF_DIR/merged-pr-url.txt"
  PROOF_MODE_FILE="$PROOF_DIR/proof-mode.txt"
  BRANCH_RECHECK_FILE="$PROOF_DIR/branch-recheck.txt"
  HEAD_RECHECK_FILE="$PROOF_DIR/head-recheck.txt"
  mkdir -p "$PROOF_DIR"
  rm -f "$BLOCKER" "$MATCHES_FILE" "$MERGED_PR_JSON" "$MERGED_PR_NUMBER_FILE" "$MERGED_PR_URL_FILE" "$PROOF_MODE_FILE" "$BRANCH_RECHECK_FILE" "$HEAD_RECHECK_FILE"
  if ! test -d "$path"; then
    echo "Blocked $path: worktree is missing; leave it untouched." > "$BLOCKER"
    continue
  fi
  if ! CURRENT_BRANCH="$(git -C "$path" branch --show-current)" || test -z "$CURRENT_BRANCH"; then
    echo "Blocked $path: could not capture a non-empty current branch name; leave the worktree untouched." > "$BLOCKER"
    continue
  fi
  echo "$CURRENT_BRANCH" > "$PROOF_DIR/branch.txt"
  if ! CURRENT_HEAD="$(git -C "$path" rev-parse "HEAD^{commit}")"; then
    echo "Blocked $path: could not capture worktree tip; leave the worktree untouched." > "$BLOCKER"
    continue
  fi
  echo "$CURRENT_HEAD" > "$PROOF_DIR/head.txt"
  CAPTURED_BRANCH="$CURRENT_BRANCH"
  CAPTURED_HEAD="$CURRENT_HEAD"
  if ! git -C "$path" status --porcelain=v1 --untracked-files=no > "$PROOF_DIR/tracked-index.porcelain"; then
    echo "Blocked $path: could not capture tracked/index status; leave the worktree untouched." > "$BLOCKER"
    continue
  fi
  if ! git -C "$path" ls-files --others --exclude-standard -z > "$PROOF_DIR/untracked.zlist"; then
    echo "Blocked $path: could not capture untracked inventory; leave the worktree untouched." > "$BLOCKER"
    continue
  fi
  python3 - "$PROOF_DIR/untracked.zlist" > "$PROOF_DIR/untracked.json" <<'PY'
import json
import sys
from pathlib import Path

raw = Path(sys.argv[1]).read_bytes()
nul = bytes([0])
entries = [] if not raw else raw.rstrip(nul).split(nul)
print(json.dumps([entry.decode("utf-8", "surrogateescape") for entry in entries], indent=2))
PY
  if ! git -C "$path" ls-files --others --ignored --exclude-standard -z > "$PROOF_DIR/ignored.zlist"; then
    echo "Blocked $path: could not capture ignored inventory; leave the worktree untouched." > "$BLOCKER"
    continue
  fi
  python3 - "$PROOF_DIR/ignored.zlist" > "$PROOF_DIR/ignored.json" <<'PY'
import json
import sys
from pathlib import Path

raw = Path(sys.argv[1]).read_bytes()
nul = bytes([0])
entries = [] if not raw else raw.rstrip(nul).split(nul)
print(json.dumps([entry.decode("utf-8", "surrogateescape") for entry in entries], indent=2))
PY
  echo "$ORIGIN_MAIN_COMMIT" > "$PROOF_DIR/origin-main.txt"
  if test -s "$PROOF_DIR/tracked-index.porcelain"; then
    echo "Blocked $path: tracked/index status is non-empty; leave the worktree untouched." > "$BLOCKER"
    continue
  fi
  if test -s "$PROOF_DIR/untracked.zlist"; then
    echo "Blocked $path: nonignored untracked paths are present; leave the worktree untouched." > "$BLOCKER"
    continue
  fi
  if test -s "$PROOF_DIR/ignored.zlist"; then
    echo "Blocked $path: ignored paths are present; leave the worktree untouched." > "$BLOCKER"
    continue
  fi
  if git -C "$REPO" merge-base --is-ancestor "$CAPTURED_HEAD" "$ORIGIN_MAIN_COMMIT"; then
    echo ancestor > "$PROOF_MODE_FILE"
  else
    if ! jq --arg REPO_NAME "OpenCoven/coven" --arg CAPTURED_BRANCH "$CAPTURED_BRANCH" --arg CAPTURED_HEAD "$CAPTURED_HEAD" '[ .[] | .[] | select(.head.repo != null and .head.repo.full_name == $REPO_NAME) | select(.base.repo != null and .base.repo.full_name == $REPO_NAME) | select(.head.ref == $CAPTURED_BRANCH) | select(.head.sha == $CAPTURED_HEAD) | select(.base.ref == "main") | select(.merged_at != null) | { number, url: .html_url, merged_at, head: { ref: .head.ref, sha: .head.sha, repo: .head.repo.full_name }, base: { ref: .base.ref, repo: .base.repo.full_name } } ]' "$CLOSED_PRS_EVIDENCE" > "$MATCHES_FILE"; then
      echo "Blocked $path: could not filter closed PR evidence for branch $CAPTURED_BRANCH at head $CAPTURED_HEAD; leave the worktree untouched." > "$BLOCKER"
      continue
    fi
    MATCH_COUNT="$(jq -r 'length' "$MATCHES_FILE")"
    if [ "$MATCH_COUNT" != "1" ]; then
      {
        echo "Blocked $path: closed-PR fallback found $MATCH_COUNT same-repository merged main PR matches for branch $CAPTURED_BRANCH at head $CAPTURED_HEAD; leave the worktree untouched."
        echo "Evidence: $MATCHES_FILE"
      } > "$BLOCKER"
      continue
    fi
    if ! jq '.[0]' "$MATCHES_FILE" > "$MERGED_PR_JSON"; then
      echo "Blocked $path: could not save the unique merged-PR fallback evidence; leave the worktree untouched." > "$BLOCKER"
      continue
    fi
    MATCH_PR_NUMBER="$(jq -r '.[0].number // empty' "$MATCHES_FILE")"
    MATCH_PR_URL="$(jq -r '.[0].url // empty' "$MATCHES_FILE")"
    if test -z "$MATCH_PR_NUMBER" || test -z "$MATCH_PR_URL"; then
      echo "Blocked $path: unique merged-PR fallback evidence was missing a PR number or URL; leave the worktree untouched." > "$BLOCKER"
      continue
    fi
    echo "$MATCH_PR_NUMBER" > "$MERGED_PR_NUMBER_FILE"
    echo "$MATCH_PR_URL" > "$MERGED_PR_URL_FILE"
    echo merged-pr > "$PROOF_MODE_FILE"
  fi
  if ! RECHECK_BRANCH="$(git -C "$path" branch --show-current)" || test -z "$RECHECK_BRANCH"; then
    echo "Blocked $path: could not revalidate a non-empty branch name immediately before removal; leave the worktree untouched." > "$BLOCKER"
    continue
  fi
  echo "$RECHECK_BRANCH" > "$BRANCH_RECHECK_FILE"
  if ! RECHECK_HEAD="$(git -C "$path" rev-parse "HEAD^{commit}")"; then
    echo "Blocked $path: could not revalidate the worktree tip immediately before removal; leave the worktree untouched." > "$BLOCKER"
    continue
  fi
  echo "$RECHECK_HEAD" > "$HEAD_RECHECK_FILE"
  if [ "$RECHECK_BRANCH" != "$CAPTURED_BRANCH" ] || [ "$RECHECK_HEAD" != "$CAPTURED_HEAD" ]; then
    {
      echo "Blocked $path: worktree branch/head drifted after proof capture and before removal; leave the worktree untouched."
      echo "Captured branch: $CAPTURED_BRANCH"
      echo "Rechecked branch: $RECHECK_BRANCH"
      echo "Captured head: $CAPTURED_HEAD"
      echo "Rechecked head: $RECHECK_HEAD"
    } > "$BLOCKER"
    continue
  fi
  git -C "$REPO" worktree remove "$path"
done
```

Do not use `--force`. Any non-empty tracked/index status, nonignored untracked
inventory, or ignored inventory blocks removal.

- [ ] **Step 4: Retire the original dirty source worktrees only after proof checks**

These four source worktrees remain intentionally present until their proof is
complete. Their runtime dirtiness may change after snapshotting—for example, a
formerly dirty tree may now be clean because its changes were committed to an
accurate active PR. `git worktree remove --force` is allowed only in this
step, only for these four exact paths, and only after the worktree and index
patch evidence, verified branch bundle, lossless untracked inventory plus tar
evidence, ignored-path inventory evidence, terminal ledger classification, and
either an adopted exact-source-branch open PR, an open replacement PR, or
recorded non-viable/blocker evidence are all present. For each source tree,
re-read the preserved `dirty/<id>/branch.txt` identity, require the live
`git branch --show-current` to match it before any live-state comparisons, and
recheck that branch identity immediately before `git worktree remove --force`.
Rows blocked because an existing PR already covers the preserved head while
`worktree.patch`, `index.patch`, or `.untracked.zlist` remains non-empty are
explicitly excluded from forced source cleanup and leave their original
worktree and branch untouched. Any non-empty preserved `.ignored.zlist` also
prohibits forced source cleanup and must instead emit blocker evidence with a
resume condition. Detached, repurposed, or unrelated worktrees must never use
force and must remain untouched.

Run:

```bash
set -euo pipefail
resolve_control_worktree() {
  local control_branch="docs/541-incomplete-work-recovery-design"
  local expected_branch="refs/heads/$control_branch"
  local remote_branch_ref="refs/remotes/origin/$control_branch"
  local start_common_dir repo target_path path branch_ref actual_branch
  local target_registration_branch live_count stale_count live_path stale_path
  local live_status live_head local_head remote_head
  local -a live_paths=() stale_paths=()
  start_common_dir="$(git rev-parse --git-common-dir)"
  repo="$(cd "$start_common_dir/.." && pwd)"
  target_path="$repo/.worktrees/issue-541-recovery"
  if ! git -C "$repo" fetch --no-tags origin \
    "refs/heads/$control_branch:$remote_branch_ref"
  then
    printf 'Blocked: could not fetch origin/%s into %s.\n' \
      "$control_branch" "$remote_branch_ref" >&2
    exit 1
  fi
  remote_head="$(git -C "$repo" rev-parse --verify "$remote_branch_ref^{commit}")"
  while IFS="$(printf "\t")" read -r path branch_ref; do
    test -n "$path" || continue
    if test "$path" = "$target_path"; then
      target_registration_branch="$branch_ref"
    fi
    if test "$branch_ref" != "$expected_branch"; then
      continue
    fi
    if test -d "$path"; then
      live_paths+=("$path")
    else
      stale_paths+=("$path")
    fi
  done <<EOF2
$(git -C "$repo" worktree list --porcelain | awk '
  $1 == "worktree" { if (path != "") print path "\t" branch; path = substr($0, 10); branch = ""; next }
  $1 == "branch" { branch = $2; next }
  END { if (path != "") print path "\t" branch }
')
EOF2
  live_count="${#live_paths[@]}"
  stale_count="${#stale_paths[@]}"
  if test -n "$target_registration_branch" && test "$target_registration_branch" != "$expected_branch"; then
    printf 'Blocked: %s is registered to %s, not %s.\n' "$target_path" "$target_registration_branch" "$control_branch" >&2
    exit 1
  fi
  if test "$live_count" -gt 1; then
    printf 'Blocked: %s has %s live controller worktree registrations; refusing to choose arbitrarily.\n' "$control_branch" "$live_count" >&2
    for live_path in "${live_paths[@]}"; do
      printf '  live: %s\n' "$live_path" >&2
    done
    for stale_path in "${stale_paths[@]}"; do
      printf '  stale: %s\n' "$stale_path" >&2
    done
    exit 1
  fi
  if test "$live_count" -eq 1; then
    live_path="${live_paths[0]}"
    if test "$stale_count" -gt 0; then
      printf 'Warning: ignoring %s stale controller registration(s) for %s while using the one live worktree.\n' "$stale_count" "$control_branch" >&2
      for stale_path in "${stale_paths[@]}"; do
        printf '  stale: %s\n' "$stale_path" >&2
      done
    fi
    actual_branch="$(git -C "$live_path" branch --show-current)"
    if test "$actual_branch" != "$control_branch"; then
      printf 'Control worktree branch mismatch: expected %s, got %s\n' "$control_branch" "$actual_branch" >&2
      exit 1
    fi
    live_status="$(git -C "$live_path" status --porcelain=v1 --untracked-files=all)"
    if test -n "$live_status"; then
      printf 'Blocked: live controller worktree is dirty and cannot be refreshed safely: %s\n' "$live_path" >&2
      git -C "$live_path" status --short --branch --untracked-files=all >&2
      exit 1
    fi
    live_head="$(git -C "$live_path" rev-parse HEAD)"
    if test "$live_head" = "$remote_head"; then
      :
    elif git -C "$repo" merge-base --is-ancestor "$live_head" "$remote_head"; then
      if ! git -C "$live_path" merge --ff-only "$remote_branch_ref"; then
        printf 'Blocked: live controller worktree could not fast-forward %s to fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
        exit 1
      fi
    elif git -C "$repo" merge-base --is-ancestor "$remote_head" "$live_head"; then
      printf 'Blocked: live controller branch %s is ahead of fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
      exit 1
    else
      printf 'Blocked: live controller branch %s diverged from fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
      exit 1
    fi
    live_head="$(git -C "$live_path" rev-parse HEAD)"
    if test "$live_head" != "$remote_head"; then
      printf 'Blocked: live controller HEAD %s does not match fetched origin tip %s.\n' "$live_head" "$remote_head" >&2
      exit 1
    fi
    printf '%s\n' "$live_path"
    return 0
  fi
  if test "$stale_count" -gt 1; then
    printf 'Blocked: %s has %s stale controller registrations and no live worktree; refusing to choose arbitrarily.\n' "$control_branch" "$stale_count" >&2
    for stale_path in "${stale_paths[@]}"; do
      printf '  stale: %s\n' "$stale_path" >&2
    done
    exit 1
  fi
  if ! git -C "$repo" check-ignore -q .worktrees/; then
    printf 'Repository-local .worktrees/ is not ignored.\n' >&2
    exit 1
  fi
  if ! git -C "$repo" check-ignore -q .worktrees/issue-541-recovery; then
    printf 'Repository-local control worktree path is not ignored: %s\n' '.worktrees/issue-541-recovery' >&2
    exit 1
  fi
  mkdir -p "$repo/.worktrees"
  if test -e "$target_path"; then
    printf 'Control worktree path already exists without a matching live registration: %s\n' "$target_path" >&2
    exit 1
  fi
  if git -C "$repo" show-ref --verify --quiet "$expected_branch"; then
    local_head="$(git -C "$repo" rev-parse --verify "$expected_branch^{commit}")"
    if test "$local_head" = "$remote_head"; then
      :
    elif git -C "$repo" merge-base --is-ancestor "$local_head" "$remote_head"; then
      if ! git -C "$repo" update-ref "$expected_branch" "$remote_head" "$local_head"; then
        printf 'Blocked: local controller branch %s could not be fast-forwarded to fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
        exit 1
      fi
    elif git -C "$repo" merge-base --is-ancestor "$remote_head" "$local_head"; then
      printf 'Blocked: local controller branch %s is ahead of fetched origin tip %s and cannot be rewritten safely.\n' "$control_branch" "$remote_head" >&2
      exit 1
    else
      printf 'Blocked: local controller branch %s diverged from fetched origin tip %s and cannot be rewritten safely.\n' "$control_branch" "$remote_head" >&2
      exit 1
    fi
  else
    if ! git -C "$repo" update-ref "$expected_branch" "$remote_head"; then
      printf 'Blocked: local controller branch %s could not be created at fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
      exit 1
    fi
  fi
  local_head="$(git -C "$repo" rev-parse --verify "$expected_branch^{commit}")"
  if test "$local_head" != "$remote_head"; then
    printf 'Blocked: local controller branch %s resolved to %s instead of fetched origin tip %s.\n' "$control_branch" "$local_head" "$remote_head" >&2
    exit 1
  fi
  if test "$stale_count" -eq 1; then
    stale_path="${stale_paths[0]}"
    printf 'Recreating %s at %s from the one stale controller registration:\n  stale: %s\n' "$control_branch" "$target_path" "$stale_path" >&2
    git -C "$repo" worktree add --force "$target_path" "$control_branch"
  else
    git -C "$repo" worktree add "$target_path" "$control_branch"
  fi
  if ! test -d "$target_path"; then
    printf 'Control worktree path is not present after resolution: %s\n' "$target_path" >&2
    exit 1
  fi
  actual_branch="$(git -C "$target_path" branch --show-current)"
  if test "$actual_branch" != "$control_branch"; then
    printf 'Control worktree branch mismatch: expected %s, got %s\n' "$control_branch" "$actual_branch" >&2
    exit 1
  fi
  local_head="$(git -C "$target_path" rev-parse HEAD)"
  if test "$local_head" != "$remote_head"; then
    printf 'Blocked: resolved controller HEAD %s does not match fetched origin tip %s.\n' "$local_head" "$remote_head" >&2
    exit 1
  fi
  printf '%s\n' "$target_path"
}
CONTROL_WORKTREE="$(resolve_control_worktree)"
COMMON_DIR="$(git -C "$CONTROL_WORKTREE" rev-parse --git-common-dir)"
REPO="$(cd "$COMMON_DIR/.." && pwd)"
RECOVERY="$COMMON_DIR/agent-recovery/issue-541"
for id in \
  docs-psyche-specs \
  memory-promote \
  mobile-memory-gateway \
  pr-476-review
do
  test -s "$RECOVERY/$id-worktree-evidence.txt"
  test -s "$RECOVERY/$id-index-evidence.txt"
  test -s "$RECOVERY/$id-untracked-evidence.json"
  test -s "$RECOVERY/$id-ignored-evidence.json"
  test -f "$RECOVERY/dirty/$id/.untracked.zlist"
  test -f "$RECOVERY/dirty/$id/untracked.tar"
  test -f "$RECOVERY/dirty/$id/.ignored.zlist"
  git -C "$REPO" bundle verify "$RECOVERY/dirty/$id/branch.bundle" > /dev/null
  grep -E "^\| $id \| (already-shipped|superseded|viable|blocked) \|" \
    "$RECOVERY/classification.md"
done
```

If a row is `viable`, confirm its classification row cites either the adopted
exact-source-branch PR URL or the open replacement PR URL before removal.
Otherwise confirm the row already records its `already-shipped`, `superseded`,
or `blocked` evidence. Before any `--force` removal, recompute live proof files
under the private recovery archive and compare them byte-for-byte with the
preserved snapshot. This proof must regenerate the live `.untracked.zlist`,
`untracked.json`, `untracked.tar`, `.ignored.zlist`, and `ignored.json` with
the same lossless method used at snapshot time. Any mismatch or missing file
blocks removal and requires a fresh snapshot plus reclassification for that
row. Only unchanged rows with an empty preserved ignored inventory may use the
exact-path force-removal exception:

```bash
set -euo pipefail
resolve_control_worktree() {
  local control_branch="docs/541-incomplete-work-recovery-design"
  local expected_branch="refs/heads/$control_branch"
  local remote_branch_ref="refs/remotes/origin/$control_branch"
  local start_common_dir repo target_path path branch_ref actual_branch
  local target_registration_branch live_count stale_count live_path stale_path
  local live_status live_head local_head remote_head
  local -a live_paths=() stale_paths=()
  start_common_dir="$(git rev-parse --git-common-dir)"
  repo="$(cd "$start_common_dir/.." && pwd)"
  target_path="$repo/.worktrees/issue-541-recovery"
  if ! git -C "$repo" fetch --no-tags origin \
    "refs/heads/$control_branch:$remote_branch_ref"
  then
    printf 'Blocked: could not fetch origin/%s into %s.\n' \
      "$control_branch" "$remote_branch_ref" >&2
    exit 1
  fi
  remote_head="$(git -C "$repo" rev-parse --verify "$remote_branch_ref^{commit}")"
  while IFS="$(printf "\t")" read -r path branch_ref; do
    test -n "$path" || continue
    if test "$path" = "$target_path"; then
      target_registration_branch="$branch_ref"
    fi
    if test "$branch_ref" != "$expected_branch"; then
      continue
    fi
    if test -d "$path"; then
      live_paths+=("$path")
    else
      stale_paths+=("$path")
    fi
  done <<EOF2
$(git -C "$repo" worktree list --porcelain | awk '
  $1 == "worktree" { if (path != "") print path "\t" branch; path = substr($0, 10); branch = ""; next }
  $1 == "branch" { branch = $2; next }
  END { if (path != "") print path "\t" branch }
')
EOF2
  live_count="${#live_paths[@]}"
  stale_count="${#stale_paths[@]}"
  if test -n "$target_registration_branch" && test "$target_registration_branch" != "$expected_branch"; then
    printf 'Blocked: %s is registered to %s, not %s.\n' "$target_path" "$target_registration_branch" "$control_branch" >&2
    exit 1
  fi
  if test "$live_count" -gt 1; then
    printf 'Blocked: %s has %s live controller worktree registrations; refusing to choose arbitrarily.\n' "$control_branch" "$live_count" >&2
    for live_path in "${live_paths[@]}"; do
      printf '  live: %s\n' "$live_path" >&2
    done
    for stale_path in "${stale_paths[@]}"; do
      printf '  stale: %s\n' "$stale_path" >&2
    done
    exit 1
  fi
  if test "$live_count" -eq 1; then
    live_path="${live_paths[0]}"
    if test "$stale_count" -gt 0; then
      printf 'Warning: ignoring %s stale controller registration(s) for %s while using the one live worktree.\n' "$stale_count" "$control_branch" >&2
      for stale_path in "${stale_paths[@]}"; do
        printf '  stale: %s\n' "$stale_path" >&2
      done
    fi
    actual_branch="$(git -C "$live_path" branch --show-current)"
    if test "$actual_branch" != "$control_branch"; then
      printf 'Control worktree branch mismatch: expected %s, got %s\n' "$control_branch" "$actual_branch" >&2
      exit 1
    fi
    live_status="$(git -C "$live_path" status --porcelain=v1 --untracked-files=all)"
    if test -n "$live_status"; then
      printf 'Blocked: live controller worktree is dirty and cannot be refreshed safely: %s\n' "$live_path" >&2
      git -C "$live_path" status --short --branch --untracked-files=all >&2
      exit 1
    fi
    live_head="$(git -C "$live_path" rev-parse HEAD)"
    if test "$live_head" = "$remote_head"; then
      :
    elif git -C "$repo" merge-base --is-ancestor "$live_head" "$remote_head"; then
      if ! git -C "$live_path" merge --ff-only "$remote_branch_ref"; then
        printf 'Blocked: live controller worktree could not fast-forward %s to fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
        exit 1
      fi
    elif git -C "$repo" merge-base --is-ancestor "$remote_head" "$live_head"; then
      printf 'Blocked: live controller branch %s is ahead of fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
      exit 1
    else
      printf 'Blocked: live controller branch %s diverged from fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
      exit 1
    fi
    live_head="$(git -C "$live_path" rev-parse HEAD)"
    if test "$live_head" != "$remote_head"; then
      printf 'Blocked: live controller HEAD %s does not match fetched origin tip %s.\n' "$live_head" "$remote_head" >&2
      exit 1
    fi
    printf '%s\n' "$live_path"
    return 0
  fi
  if test "$stale_count" -gt 1; then
    printf 'Blocked: %s has %s stale controller registrations and no live worktree; refusing to choose arbitrarily.\n' "$control_branch" "$stale_count" >&2
    for stale_path in "${stale_paths[@]}"; do
      printf '  stale: %s\n' "$stale_path" >&2
    done
    exit 1
  fi
  if ! git -C "$repo" check-ignore -q .worktrees/; then
    printf 'Repository-local .worktrees/ is not ignored.\n' >&2
    exit 1
  fi
  if ! git -C "$repo" check-ignore -q .worktrees/issue-541-recovery; then
    printf 'Repository-local control worktree path is not ignored: %s\n' '.worktrees/issue-541-recovery' >&2
    exit 1
  fi
  mkdir -p "$repo/.worktrees"
  if test -e "$target_path"; then
    printf 'Control worktree path already exists without a matching live registration: %s\n' "$target_path" >&2
    exit 1
  fi
  if git -C "$repo" show-ref --verify --quiet "$expected_branch"; then
    local_head="$(git -C "$repo" rev-parse --verify "$expected_branch^{commit}")"
    if test "$local_head" = "$remote_head"; then
      :
    elif git -C "$repo" merge-base --is-ancestor "$local_head" "$remote_head"; then
      if ! git -C "$repo" update-ref "$expected_branch" "$remote_head" "$local_head"; then
        printf 'Blocked: local controller branch %s could not be fast-forwarded to fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
        exit 1
      fi
    elif git -C "$repo" merge-base --is-ancestor "$remote_head" "$local_head"; then
      printf 'Blocked: local controller branch %s is ahead of fetched origin tip %s and cannot be rewritten safely.\n' "$control_branch" "$remote_head" >&2
      exit 1
    else
      printf 'Blocked: local controller branch %s diverged from fetched origin tip %s and cannot be rewritten safely.\n' "$control_branch" "$remote_head" >&2
      exit 1
    fi
  else
    if ! git -C "$repo" update-ref "$expected_branch" "$remote_head"; then
      printf 'Blocked: local controller branch %s could not be created at fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
      exit 1
    fi
  fi
  local_head="$(git -C "$repo" rev-parse --verify "$expected_branch^{commit}")"
  if test "$local_head" != "$remote_head"; then
    printf 'Blocked: local controller branch %s resolved to %s instead of fetched origin tip %s.\n' "$control_branch" "$local_head" "$remote_head" >&2
    exit 1
  fi
  if test "$stale_count" -eq 1; then
    stale_path="${stale_paths[0]}"
    printf 'Recreating %s at %s from the one stale controller registration:\n  stale: %s\n' "$control_branch" "$target_path" "$stale_path" >&2
    git -C "$repo" worktree add --force "$target_path" "$control_branch"
  else
    git -C "$repo" worktree add "$target_path" "$control_branch"
  fi
  if ! test -d "$target_path"; then
    printf 'Control worktree path is not present after resolution: %s\n' "$target_path" >&2
    exit 1
  fi
  actual_branch="$(git -C "$target_path" branch --show-current)"
  if test "$actual_branch" != "$control_branch"; then
    printf 'Control worktree branch mismatch: expected %s, got %s\n' "$control_branch" "$actual_branch" >&2
    exit 1
  fi
  local_head="$(git -C "$target_path" rev-parse HEAD)"
  if test "$local_head" != "$remote_head"; then
    printf 'Blocked: resolved controller HEAD %s does not match fetched origin tip %s.\n' "$local_head" "$remote_head" >&2
    exit 1
  fi
  printf '%s\n' "$target_path"
}
CONTROL_WORKTREE="$(resolve_control_worktree)"
COMMON_DIR="$(git -C "$CONTROL_WORKTREE" rev-parse --git-common-dir)"
REPO="$(cd "$COMMON_DIR/.." && pwd)"
RECOVERY="$COMMON_DIR/agent-recovery/issue-541"
RETIRE_PROOF_ROOT="$RECOVERY/private-retire-proof"
CLASSIFICATION="$RECOVERY/classification.md"
mkdir -p "$RETIRE_PROOF_ROOT"
if ! git -C "$REPO" fetch origin main; then
  printf 'Blocked: could not refresh origin/main before retirement proof.\n' >&2
  exit 1
fi
ORIGIN_MAIN_COMMIT="$(git -C "$REPO" rev-parse "refs/remotes/origin/main^{commit}")"
parse_classification_row() {
  python3 - "$CLASSIFICATION" "$1" <<'PY'
import sys
from pathlib import Path

classification_path = Path(sys.argv[1])
workstream = sys.argv[2]
for raw in classification_path.read_text(encoding="utf-8").splitlines():
    line = raw.strip()
    if not line.startswith("|") or not line.endswith("|"):
        continue
    cells = [cell.strip() for cell in line.strip("|").split("|")]
    if len(cells) != 5 or cells[0] == "Workstream":
        continue
    if cells[0] != workstream:
        continue
    print(cells[1])
    print(cells[2])
    print(cells[3])
    print(cells[4])
    break
else:
    raise SystemExit(f"Missing classification row for {workstream}")
PY
}
parse_recovery_action() {
  python3 - "$1" <<'PY'
import sys

action = sys.argv[1].strip()
fields = {}
for raw_part in action.split(";"):
    part = raw_part.strip()
    if not part:
        continue
    if "=" not in part:
        raise SystemExit(f"Recovery action segment is not key=value: {part}")
    key, value = part.split("=", 1)
    key = key.strip()
    value = value.strip()
    if not key or not value:
        raise SystemExit(f"Recovery action segment has an empty key or value: {part}")
    if key in fields:
        raise SystemExit(f"Recovery action repeats key {key}")
    fields[key] = value

mode = fields.get("mode")
pr_kind = fields.get("pr_kind")
required = ["mode", "pr_kind", "issue_url", "archive_id", "expected_branch"]
if mode == "continue-existing-pr":
    if pr_kind == "adopted":
        required.append("preserved_head")
    elif pr_kind == "recovered":
        required.append("expected_head")
    else:
        raise SystemExit(f"Unsupported continue-existing-pr kind: {pr_kind}")
elif mode in {"awaiting-recovery-pr", "recovery-pr-open"}:
    if pr_kind != "recovered":
        raise SystemExit(f"{mode} must declare pr_kind=recovered, got {pr_kind}")
    if mode == "recovery-pr-open":
        required.append("expected_head")
else:
    raise SystemExit(f"Unsupported recovery action mode: {mode}")

for key in required:
    if key not in fields:
        raise SystemExit(f"Recovery action is missing {key}")

print(fields["mode"])
print(fields["pr_kind"])
print(fields["issue_url"])
print(fields["archive_id"])
print(fields["expected_branch"])
print(fields.get("expected_head", ""))
print(fields.get("preserved_head", ""))
PY
}
parse_non_viable_action() {
  python3 - "$1" "$2" <<'PY'
import sys

action = sys.argv[1].strip()
expected_classification = sys.argv[2].strip()
fields = {}
for raw_part in action.split(";"):
    part = raw_part.strip()
    if not part:
        continue
    if "=" not in part:
        raise SystemExit(f"Recovery action segment is not key=value: {part}")
    key, value = part.split("=", 1)
    key = key.strip()
    value = value.strip()
    if not key or not value:
        raise SystemExit(f"Recovery action segment has an empty key or value: {part}")
    if key in fields:
        raise SystemExit(f"Recovery action repeats key {key}")
    fields[key] = value

if fields.get("mode") != "non-viable-proof":
    raise SystemExit(f"Unsupported non-viable proof mode: {fields.get('mode')}")
if fields.get("classification") != expected_classification:
    raise SystemExit(
        f"Non-viable proof classification mismatch: expected {expected_classification}, got {fields.get('classification')}"
    )
evidence_kind = fields.get("evidence_kind")
if evidence_kind not in {"merged-pr", "main-commit"}:
    raise SystemExit(f"Unsupported non-viable evidence_kind: {evidence_kind}")

print(fields["mode"])
print(fields["classification"])
print(fields["evidence_kind"])
PY
}
block_retirement() {
  local id="$1"
  local blocker="$2"
  local classification="$3"
  local evidence="$4"
  local preserved_source="$5"
  local recovery_action="$6"
  local reason="$7"
  {
    printf 'Blocked %s: %s\n' "$id" "$reason"
    printf 'Classification: %s\n' "$classification"
    printf 'Main/PR evidence: %s\n' "$evidence"
    printf 'Preserved source: %s\n' "$preserved_source"
    printf 'Recovery action: %s\n' "$recovery_action"
  } > "$blocker"
}
for id in \
  docs-psyche-specs \
  memory-promote \
  mobile-memory-gateway \
  pr-476-review
do
  SNAPSHOT="$RECOVERY/dirty/$id"
  PROOF_DIR="$RETIRE_PROOF_ROOT/$id"
  BLOCKER="$RECOVERY/$id-retire-blocker.txt"
  mkdir -p "$PROOF_DIR"
  case "$id" in
    docs-psyche-specs)
      SOURCE="$REPO/.worktrees/docs-psyche-specs"
      ;;
    memory-promote)
      SOURCE="$REPO/.worktrees/feat-cmem-1ev-memory-promote"
      ;;
    mobile-memory-gateway)
      SOURCE="$REPO/.worktrees/mobile-memory-gateway"
      ;;
    pr-476-review)
      SOURCE="$REPO/.worktrees/pr-476-review"
      ;;
  esac
  CLASSIFICATION_FIELDS_FILE="$RECOVERY/.classification-fields.$$"
  if ! parse_classification_row "$id" >"$CLASSIFICATION_FIELDS_FILE"; then
    rm -f "$CLASSIFICATION_FIELDS_FILE"
    block_retirement "$id" "$BLOCKER" "unparsed" "missing exact row" "$SOURCE" "unparsed" \
      "classification row could not be parsed safely before any live-state checks"
    continue
  fi
  CLASSIFICATION_ROW=()
  CLASSIFICATION_FIELD=
  while IFS= read -r CLASSIFICATION_FIELD || [ -n "$CLASSIFICATION_FIELD" ]; do
    CLASSIFICATION_ROW+=("$CLASSIFICATION_FIELD")
  done < "$CLASSIFICATION_FIELDS_FILE"
  rm -f "$CLASSIFICATION_FIELDS_FILE"
  if test "${#CLASSIFICATION_ROW[@]}" -ne 4; then
    block_retirement "$id" "$BLOCKER" "unparsed" "unexpected column count" "$SOURCE" "unparsed" \
      "classification row could not be parsed safely before any live-state checks"
    continue
  fi
  printf '%s\n' "$ORIGIN_MAIN_COMMIT" > "$PROOF_DIR/live-origin-main.txt"
  CLASSIFICATION_LABEL="${CLASSIFICATION_ROW[0]}"
  MAIN_PR_EVIDENCE="${CLASSIFICATION_ROW[1]}"
  PRESERVED_SOURCE="${CLASSIFICATION_ROW[2]}"
  RECOVERY_ACTION="${CLASSIFICATION_ROW[3]}"
  case "$CLASSIFICATION_LABEL" in
    already-shipped|superseded)
      NON_VIABLE_ACTION_FIELDS_FILE="$RECOVERY/.non-viable-action-fields.$$"
      if ! parse_non_viable_action "$RECOVERY_ACTION" "$CLASSIFICATION_LABEL" >"$NON_VIABLE_ACTION_FIELDS_FILE"; then
        rm -f "$NON_VIABLE_ACTION_FIELDS_FILE"
        block_retirement "$id" "$BLOCKER" "$CLASSIFICATION_LABEL" "$MAIN_PR_EVIDENCE" "$PRESERVED_SOURCE" "$RECOVERY_ACTION" \
          "non-viable rows must encode deterministic cleanup-proof metadata"
        continue
      fi
      NON_VIABLE_ACTION_ROW=()
      NON_VIABLE_ACTION_FIELD=
      while IFS= read -r NON_VIABLE_ACTION_FIELD || [ -n "$NON_VIABLE_ACTION_FIELD" ]; do
        NON_VIABLE_ACTION_ROW+=("$NON_VIABLE_ACTION_FIELD")
      done < "$NON_VIABLE_ACTION_FIELDS_FILE"
      rm -f "$NON_VIABLE_ACTION_FIELDS_FILE"
      if test "${#NON_VIABLE_ACTION_ROW[@]}" -ne 3; then
        block_retirement "$id" "$BLOCKER" "$CLASSIFICATION_LABEL" "$MAIN_PR_EVIDENCE" "$PRESERVED_SOURCE" "$RECOVERY_ACTION" \
          "non-viable proof parsing returned an unexpected field count"
        continue
      fi
      NON_VIABLE_EVIDENCE_KIND="${NON_VIABLE_ACTION_ROW[2]}"
      case "$NON_VIABLE_EVIDENCE_KIND" in
        merged-pr)
          case "$MAIN_PR_EVIDENCE" in
            https://github.com/OpenCoven/coven/pull/*)
              ;;
            *)
              block_retirement "$id" "$BLOCKER" "$CLASSIFICATION_LABEL" "$MAIN_PR_EVIDENCE" "$PRESERVED_SOURCE" "$RECOVERY_ACTION" \
                "non-viable merged-pr rows must record a raw GitHub PR URL for OpenCoven/coven"
              continue
              ;;
          esac
          NON_VIABLE_PR_VIEW="$PROOF_DIR/live-non-viable-pr-view.json"
          NON_VIABLE_PR_ERR="$PROOF_DIR/live-non-viable-pr-view.err"
          if ! gh pr view --repo OpenCoven/coven "$MAIN_PR_EVIDENCE" \
            --json number,url,state,baseRefName,mergedAt \
            > "$NON_VIABLE_PR_VIEW" 2> "$NON_VIABLE_PR_ERR"; then
            block_retirement "$id" "$BLOCKER" "$CLASSIFICATION_LABEL" "$MAIN_PR_EVIDENCE" "$PRESERVED_SOURCE" "$RECOVERY_ACTION" \
              "recorded non-viable PR could not be freshly verified against OpenCoven/coven"
            cat "$NON_VIABLE_PR_ERR" >> "$BLOCKER"
            continue
          fi
          ACTUAL_NON_VIABLE_URL="$(jq -r '.url' "$NON_VIABLE_PR_VIEW")"
          ACTUAL_NON_VIABLE_STATE="$(jq -r '.state' "$NON_VIABLE_PR_VIEW")"
          ACTUAL_NON_VIABLE_BASE="$(jq -r '.baseRefName' "$NON_VIABLE_PR_VIEW")"
          ACTUAL_NON_VIABLE_MERGED_AT="$(jq -r '.mergedAt // empty' "$NON_VIABLE_PR_VIEW")"
          if [ "$ACTUAL_NON_VIABLE_URL" != "$MAIN_PR_EVIDENCE" ] || \
             [ "$ACTUAL_NON_VIABLE_STATE" != "MERGED" ] || \
             [ "$ACTUAL_NON_VIABLE_BASE" != "main" ] || \
             [ -z "$ACTUAL_NON_VIABLE_MERGED_AT" ]; then
            block_retirement "$id" "$BLOCKER" "$CLASSIFICATION_LABEL" "$MAIN_PR_EVIDENCE" "$PRESERVED_SOURCE" "$RECOVERY_ACTION" \
              "recorded non-viable PR did not freshly verify as MERGED into main in OpenCoven/coven"
            continue
          fi
          ;;
        main-commit)
          if ! printf '%s\n' "$MAIN_PR_EVIDENCE" | grep -Eq '^[0-9a-f]{40}$'; then
            block_retirement "$id" "$BLOCKER" "$CLASSIFICATION_LABEL" "$MAIN_PR_EVIDENCE" "$PRESERVED_SOURCE" "$RECOVERY_ACTION" \
              "non-viable main-commit rows must record an exact 40-character commit SHA"
            continue
          fi
          if ! ACTUAL_NON_VIABLE_COMMIT="$(git -C "$REPO" rev-parse --verify "$MAIN_PR_EVIDENCE^{commit}" 2>/dev/null)"; then
            block_retirement "$id" "$BLOCKER" "$CLASSIFICATION_LABEL" "$MAIN_PR_EVIDENCE" "$PRESERVED_SOURCE" "$RECOVERY_ACTION" \
              "recorded non-viable commit could not be resolved as a commit object"
            continue
          fi
          printf '%s\n' "$ACTUAL_NON_VIABLE_COMMIT" > "$PROOF_DIR/live-non-viable-commit.txt"
          if [ "$ACTUAL_NON_VIABLE_COMMIT" != "$MAIN_PR_EVIDENCE" ] || \
             ! git -C "$REPO" merge-base --is-ancestor "$ACTUAL_NON_VIABLE_COMMIT" "refs/remotes/origin/main"; then
            block_retirement "$id" "$BLOCKER" "$CLASSIFICATION_LABEL" "$MAIN_PR_EVIDENCE" "$PRESERVED_SOURCE" "$RECOVERY_ACTION" \
              "recorded non-viable commit must be an exact commit SHA and an ancestor of freshly fetched origin/main"
            continue
          fi
          ;;
        *)
          block_retirement "$id" "$BLOCKER" "$CLASSIFICATION_LABEL" "$MAIN_PR_EVIDENCE" "$PRESERVED_SOURCE" "$RECOVERY_ACTION" \
            "non-viable evidence_kind is not eligible for forced retirement"
          continue
          ;;
      esac
      ;;
    viable)
      PR_URL="$MAIN_PR_EVIDENCE"
      case "$PR_URL" in
        https://github.com/OpenCoven/coven/pull/*)
          ;;
        *)
          block_retirement "$id" "$BLOCKER" "$CLASSIFICATION_LABEL" "$MAIN_PR_EVIDENCE" "$PRESERVED_SOURCE" "$RECOVERY_ACTION" \
            "viable rows must record a GitHub PR URL for OpenCoven/coven"
          continue
          ;;
      esac
      RECOVERY_ACTION_FIELDS_FILE="$RECOVERY/.recovery-action-fields.$$"
      if ! parse_recovery_action "$RECOVERY_ACTION" >"$RECOVERY_ACTION_FIELDS_FILE"; then
        rm -f "$RECOVERY_ACTION_FIELDS_FILE"
        block_retirement "$id" "$BLOCKER" "$CLASSIFICATION_LABEL" "$MAIN_PR_EVIDENCE" "$PRESERVED_SOURCE" "$RECOVERY_ACTION" \
          "viable rows must encode a deterministic recovery action payload"
        continue
      fi
      ACTION_ROW=()
      ACTION_FIELD=
      while IFS= read -r ACTION_FIELD || [ -n "$ACTION_FIELD" ]; do
        ACTION_ROW+=("$ACTION_FIELD")
      done < "$RECOVERY_ACTION_FIELDS_FILE"
      rm -f "$RECOVERY_ACTION_FIELDS_FILE"
      if test "${#ACTION_ROW[@]}" -ne 7; then
        block_retirement "$id" "$BLOCKER" "$CLASSIFICATION_LABEL" "$MAIN_PR_EVIDENCE" "$PRESERVED_SOURCE" "$RECOVERY_ACTION" \
          "recovery action parsing returned an unexpected field count"
        continue
      fi
      ACTION_MODE="${ACTION_ROW[0]}"
      ACTION_PR_KIND="${ACTION_ROW[1]}"
      ACTION_ISSUE_URL="${ACTION_ROW[2]}"
      ACTION_ARCHIVE_ID="${ACTION_ROW[3]}"
      ACTION_EXPECTED_BRANCH="${ACTION_ROW[4]}"
      ACTION_EXPECTED_HEAD="${ACTION_ROW[5]}"
      ACTION_PRESERVED_HEAD="${ACTION_ROW[6]}"
      case "$ACTION_ISSUE_URL" in
        https://github.com/OpenCoven/coven/issues/*)
          ;;
        *)
          block_retirement "$id" "$BLOCKER" "$CLASSIFICATION_LABEL" "$MAIN_PR_EVIDENCE" "$PRESERVED_SOURCE" "$RECOVERY_ACTION" \
            "recovery action must record a GitHub issue URL for OpenCoven/coven"
          continue
          ;;
      esac
      if [ "$ACTION_ARCHIVE_ID" != "$PRESERVED_SOURCE" ]; then
        block_retirement "$id" "$BLOCKER" "$CLASSIFICATION_LABEL" "$MAIN_PR_EVIDENCE" "$PRESERVED_SOURCE" "$RECOVERY_ACTION" \
          "recovery action archive_id must match the preserved source column"
        continue
      fi
      case "$ACTION_MODE" in
        continue-existing-pr)
          case "$ACTION_PR_KIND" in
            adopted|recovered)
              ;;
            *)
              block_retirement "$id" "$BLOCKER" "$CLASSIFICATION_LABEL" "$MAIN_PR_EVIDENCE" "$PRESERVED_SOURCE" "$RECOVERY_ACTION" \
                "continue-existing-pr rows must declare pr_kind=adopted or pr_kind=recovered"
              continue
              ;;
          esac
          ;;
        recovery-pr-open)
          if [ "$ACTION_PR_KIND" != "recovered" ]; then
            block_retirement "$id" "$BLOCKER" "$CLASSIFICATION_LABEL" "$MAIN_PR_EVIDENCE" "$PRESERVED_SOURCE" "$RECOVERY_ACTION" \
              "recovery-pr-open rows must declare pr_kind=recovered"
            continue
          fi
          ;;
        awaiting-recovery-pr)
          block_retirement "$id" "$BLOCKER" "$CLASSIFICATION_LABEL" "$MAIN_PR_EVIDENCE" "$PRESERVED_SOURCE" "$RECOVERY_ACTION" \
            "rows that are still awaiting a recovery PR are never eligible for forced retirement"
          continue
          ;;
        *)
          block_retirement "$id" "$BLOCKER" "$CLASSIFICATION_LABEL" "$MAIN_PR_EVIDENCE" "$PRESERVED_SOURCE" "$RECOVERY_ACTION" \
            "recovery action mode is not eligible for forced retirement"
          continue
          ;;
      esac
      PR_VIEW_EVIDENCE="$PROOF_DIR/live-pr-view.json"
      PR_BLOCKER_EVIDENCE="$PROOF_DIR/live-pr-view.err"
      SOURCE_BRANCH_FETCH_EVIDENCE="$PROOF_DIR/live-source-branch-fetch.txt"
      RECOVERY_BRANCH_FETCH_EVIDENCE="$PROOF_DIR/live-recovery-branch-fetch.txt"
      ACTION_FETCHED_HEAD=''
      case "$ACTION_MODE/$ACTION_PR_KIND" in
        continue-existing-pr/adopted)
          {
            printf 'Expected source branch: %s\n' "$ACTION_EXPECTED_BRANCH"
            printf 'Expected preserved head from action: %s\n' "$ACTION_PRESERVED_HEAD"
            printf 'Fetching origin/%s for retirement verification.\n' "$ACTION_EXPECTED_BRANCH"
          } > "$SOURCE_BRANCH_FETCH_EVIDENCE"
          if ! git -C "$REPO" fetch --no-tags origin \
            "refs/heads/$ACTION_EXPECTED_BRANCH:refs/remotes/origin/$ACTION_EXPECTED_BRANCH" \
            >> "$SOURCE_BRANCH_FETCH_EVIDENCE" 2>&1; then
            block_retirement "$id" "$BLOCKER" "$CLASSIFICATION_LABEL" "$MAIN_PR_EVIDENCE" "$PRESERVED_SOURCE" "$RECOVERY_ACTION" \
              "could not refetch the expected adopted source branch immediately before retirement verification"
            cat "$SOURCE_BRANCH_FETCH_EVIDENCE" >> "$BLOCKER"
            continue
          fi
          ACTION_FETCHED_HEAD="$(git -C "$REPO" rev-parse "refs/remotes/origin/$ACTION_EXPECTED_BRANCH")"
          printf 'Fresh fetched origin/%s tip: %s\n' \
            "$ACTION_EXPECTED_BRANCH" "$ACTION_FETCHED_HEAD" >> "$SOURCE_BRANCH_FETCH_EVIDENCE"
          ;;
        continue-existing-pr/recovered|recovery-pr-open/recovered)
          {
            printf 'Expected recovery branch: %s\n' "$ACTION_EXPECTED_BRANCH"
            printf 'Expected recovery head from action: %s\n' "$ACTION_EXPECTED_HEAD"
            printf 'Fetching origin/%s for retirement verification.\n' "$ACTION_EXPECTED_BRANCH"
          } > "$RECOVERY_BRANCH_FETCH_EVIDENCE"
          if ! git -C "$REPO" fetch --no-tags origin \
            "refs/heads/$ACTION_EXPECTED_BRANCH:refs/remotes/origin/$ACTION_EXPECTED_BRANCH" \
            >> "$RECOVERY_BRANCH_FETCH_EVIDENCE" 2>&1; then
            block_retirement "$id" "$BLOCKER" "$CLASSIFICATION_LABEL" "$MAIN_PR_EVIDENCE" "$PRESERVED_SOURCE" "$RECOVERY_ACTION" \
              "could not refetch the expected recovery branch immediately before retirement verification"
            cat "$RECOVERY_BRANCH_FETCH_EVIDENCE" >> "$BLOCKER"
            continue
          fi
          ACTION_FETCHED_HEAD="$(git -C "$REPO" rev-parse "refs/remotes/origin/$ACTION_EXPECTED_BRANCH")"
          printf 'Fresh fetched origin/%s tip: %s\n' \
            "$ACTION_EXPECTED_BRANCH" "$ACTION_FETCHED_HEAD" >> "$RECOVERY_BRANCH_FETCH_EVIDENCE"
          ;;
      esac
      if ! gh pr view --repo OpenCoven/coven "$PR_URL" \
        --json number,title,url,state,headRefOid,headRefName,headRepositoryOwner,isCrossRepository,baseRefName \
        > "$PR_VIEW_EVIDENCE" 2> "$PR_BLOCKER_EVIDENCE"; then
        block_retirement "$id" "$BLOCKER" "$CLASSIFICATION_LABEL" "$MAIN_PR_EVIDENCE" "$PRESERVED_SOURCE" "$RECOVERY_ACTION" \
          "recorded viable PR could not be freshly verified against OpenCoven/coven"
        cat "$PR_BLOCKER_EVIDENCE" >> "$BLOCKER"
        continue
      fi
      ACTUAL_URL="$(jq -r '.url' "$PR_VIEW_EVIDENCE")"
      ACTUAL_STATE="$(jq -r '.state' "$PR_VIEW_EVIDENCE")"
      ACTUAL_HEAD="$(jq -r '.headRefOid' "$PR_VIEW_EVIDENCE")"
      ACTUAL_BRANCH="$(jq -r '.headRefName' "$PR_VIEW_EVIDENCE")"
      ACTUAL_OWNER="$(jq -r '.headRepositoryOwner.login' "$PR_VIEW_EVIDENCE")"
      ACTUAL_CROSS="$(jq -r '.isCrossRepository' "$PR_VIEW_EVIDENCE")"
      ACTUAL_BASE="$(jq -r '.baseRefName' "$PR_VIEW_EVIDENCE")"
      if [ "$ACTUAL_URL" != "$PR_URL" ] || \
         [ "$ACTUAL_STATE" != "OPEN" ] || \
         [ "$ACTUAL_OWNER" != "OpenCoven" ] || \
         [ "$ACTUAL_CROSS" != "false" ] || \
         [ "$ACTUAL_BASE" != "main" ]; then
        block_retirement "$id" "$BLOCKER" "$CLASSIFICATION_LABEL" "$MAIN_PR_EVIDENCE" "$PRESERVED_SOURCE" "$RECOVERY_ACTION" \
          "viable PR did not freshly verify as OPEN on base main in OpenCoven/coven"
        continue
      fi
      case "$ACTION_MODE" in
        continue-existing-pr)
          case "$ACTION_PR_KIND" in
            adopted)
              SNAPSHOT_HEAD="$(tr -d '\n' < "$SNAPSHOT/head.txt")"
              if [ "$ACTION_EXPECTED_BRANCH" != "$ACTUAL_BRANCH" ] || \
                 [ "$ACTION_PRESERVED_HEAD" != "$SNAPSHOT_HEAD" ] || \
                 [ "$ACTION_FETCHED_HEAD" != "$ACTUAL_HEAD" ] || \
                 ! git -C "$REPO" merge-base --is-ancestor "$ACTION_PRESERVED_HEAD" "$ACTUAL_HEAD"; then
                block_retirement "$id" "$BLOCKER" "$CLASSIFICATION_LABEL" "$MAIN_PR_EVIDENCE" "$PRESERVED_SOURCE" "$RECOVERY_ACTION" \
                  "adopted PR no longer matches the expected source branch, fetched tip, and preserved-head ancestry"
                {
                  printf 'Expected branch from recovery action: %s\n' "$ACTION_EXPECTED_BRANCH"
                  printf 'Actual branch from PR: %s\n' "$ACTUAL_BRANCH"
                  printf 'Preserved head from recovery action: %s\n' "$ACTION_PRESERVED_HEAD"
                  printf 'Preserved head from snapshot: %s\n' "$SNAPSHOT_HEAD"
                  printf 'Fresh fetched origin/%s tip: %s\n' "$ACTION_EXPECTED_BRANCH" "$ACTION_FETCHED_HEAD"
                  printf 'Current PR head: %s\n' "$ACTUAL_HEAD"
                } >> "$BLOCKER"
                continue
              fi
              ;;
            recovered)
              if [ "$ACTION_EXPECTED_BRANCH" != "$ACTUAL_BRANCH" ] || \
                 [ "$ACTION_EXPECTED_HEAD" != "$ACTION_FETCHED_HEAD" ] || \
                 [ "$ACTION_EXPECTED_HEAD" != "$ACTUAL_HEAD" ]; then
                block_retirement "$id" "$BLOCKER" "$CLASSIFICATION_LABEL" "$MAIN_PR_EVIDENCE" "$PRESERVED_SOURCE" "$RECOVERY_ACTION" \
                  "rerun-adopted recovery PR no longer matches the expected recovery branch and fetched head"
                {
                  printf 'Expected recovery branch from recovery action: %s\n' "$ACTION_EXPECTED_BRANCH"
                  printf 'Expected recovery head from recovery action: %s\n' "$ACTION_EXPECTED_HEAD"
                  printf 'Fresh fetched origin/%s tip: %s\n' "$ACTION_EXPECTED_BRANCH" "$ACTION_FETCHED_HEAD"
                  printf 'Actual branch from PR: %s\n' "$ACTUAL_BRANCH"
                  printf 'Current PR head: %s\n' "$ACTUAL_HEAD"
                } >> "$BLOCKER"
                continue
              fi
              ;;
          esac
          ;;
        recovery-pr-open)
          if [ "$ACTION_EXPECTED_BRANCH" != "$ACTUAL_BRANCH" ] || \
             [ "$ACTION_EXPECTED_HEAD" != "$ACTION_FETCHED_HEAD" ] || \
             [ "$ACTION_EXPECTED_HEAD" != "$ACTUAL_HEAD" ]; then
            block_retirement "$id" "$BLOCKER" "$CLASSIFICATION_LABEL" "$MAIN_PR_EVIDENCE" "$PRESERVED_SOURCE" "$RECOVERY_ACTION" \
              "recovery PR no longer matches the expected recovery branch and fetched head"
            {
              printf 'Expected recovery branch: %s\n' "$ACTION_EXPECTED_BRANCH"
              printf 'Expected recovery head from recovery action: %s\n' "$ACTION_EXPECTED_HEAD"
              printf 'Fresh fetched origin/%s tip: %s\n' "$ACTION_EXPECTED_BRANCH" "$ACTION_FETCHED_HEAD"
              printf 'Actual PR branch: %s\n' "$ACTUAL_BRANCH"
              printf 'Current PR head: %s\n' "$ACTUAL_HEAD"
            } >> "$BLOCKER"
            continue
          fi
          ;;
      esac
      ;;
    pending|blocked)
      block_retirement "$id" "$BLOCKER" "$CLASSIFICATION_LABEL" "$MAIN_PR_EVIDENCE" "$PRESERVED_SOURCE" "$RECOVERY_ACTION" \
        "pending and blocked rows are never eligible for forced retirement"
      continue
      ;;
    *)
      block_retirement "$id" "$BLOCKER" "$CLASSIFICATION_LABEL" "$MAIN_PR_EVIDENCE" "$PRESERVED_SOURCE" "$RECOVERY_ACTION" \
        "classification is not eligible for forced retirement"
      continue
      ;;
  esac
  if ! test -d "$SOURCE"; then
    block_retirement "$id" "$BLOCKER" "$CLASSIFICATION_LABEL" "$MAIN_PR_EVIDENCE" "$PRESERVED_SOURCE" "$RECOVERY_ACTION" \
      "source worktree is missing before retirement proof; take a fresh snapshot and reclassify"
    continue
  fi
  if ! test -s "$SNAPSHOT/branch.txt"; then
    block_retirement "$id" "$BLOCKER" "$CLASSIFICATION_LABEL" "$MAIN_PR_EVIDENCE" "$PRESERVED_SOURCE" "$RECOVERY_ACTION" \
      "preserved branch identity snapshot is missing; take a fresh snapshot and reclassify before removal"
    continue
  fi
  PRESERVED_BRANCH="$(tr -d '\n' < "$SNAPSHOT/branch.txt")"
  if ! CURRENT_BRANCH="$(git -C "$SOURCE" branch --show-current)" || test -z "$CURRENT_BRANCH"; then
    block_retirement "$id" "$BLOCKER" "$CLASSIFICATION_LABEL" "$MAIN_PR_EVIDENCE" "$PRESERVED_SOURCE" "$RECOVERY_ACTION" \
      "source worktree is detached or branchless before retirement proof; leave the worktree untouched"
    continue
  fi
  if [ "$CURRENT_BRANCH" != "$PRESERVED_BRANCH" ]; then
    block_retirement "$id" "$BLOCKER" "$CLASSIFICATION_LABEL" "$MAIN_PR_EVIDENCE" "$PRESERVED_SOURCE" "$RECOVERY_ACTION" \
      "source worktree branch drifted before live-state comparisons; leave the worktree untouched"
    {
      printf 'Preserved branch: %s\n' "$PRESERVED_BRANCH"
      printf 'Current branch: %s\n' "$CURRENT_BRANCH"
    } >> "$BLOCKER"
    continue
  fi
  git -C "$SOURCE" rev-parse HEAD > "$PROOF_DIR/live-head.txt"
  if ! cmp -s "$PROOF_DIR/live-head.txt" "$SNAPSHOT/head.txt"; then
    block_retirement "$id" "$BLOCKER" "$CLASSIFICATION_LABEL" "$MAIN_PR_EVIDENCE" "$PRESERVED_SOURCE" "$RECOVERY_ACTION" \
      "HEAD drifted since snapshot; take a fresh snapshot and reclassify before removal"
    continue
  fi
  git -C "$SOURCE" diff --binary > "$PROOF_DIR/live-worktree.patch"
  if ! cmp -s "$PROOF_DIR/live-worktree.patch" "$SNAPSHOT/worktree.patch"; then
    block_retirement "$id" "$BLOCKER" "$CLASSIFICATION_LABEL" "$MAIN_PR_EVIDENCE" "$PRESERVED_SOURCE" "$RECOVERY_ACTION" \
      "unstaged tracked changes drifted since snapshot; take a fresh snapshot and reclassify before removal"
    continue
  fi
  git -C "$SOURCE" diff --cached --binary > "$PROOF_DIR/live-index.patch"
  if ! cmp -s "$PROOF_DIR/live-index.patch" "$SNAPSHOT/index.patch"; then
    block_retirement "$id" "$BLOCKER" "$CLASSIFICATION_LABEL" "$MAIN_PR_EVIDENCE" "$PRESERVED_SOURCE" "$RECOVERY_ACTION" \
      "staged changes drifted since snapshot; take a fresh snapshot and reclassify before removal"
    continue
  fi
  git -C "$SOURCE" ls-files --others --exclude-standard -z > "$PROOF_DIR/live-untracked.zlist"
  python3 - "$PROOF_DIR/live-untracked.zlist" > "$PROOF_DIR/live-untracked.json" <<'PY'
import json
import sys
from pathlib import Path

raw = Path(sys.argv[1]).read_bytes()
entries = [] if not raw else raw.rstrip(b"\0").split(b"\0")
print(json.dumps([entry.decode("utf-8", "surrogateescape") for entry in entries], indent=2))
PY
  tar -C "$SOURCE" --null -T "$PROOF_DIR/live-untracked.zlist" -cf "$PROOF_DIR/live-untracked.tar"
  if ! cmp -s "$PROOF_DIR/live-untracked.zlist" "$SNAPSHOT/.untracked.zlist"; then
    block_retirement "$id" "$BLOCKER" "$CLASSIFICATION_LABEL" "$MAIN_PR_EVIDENCE" "$PRESERVED_SOURCE" "$RECOVERY_ACTION" \
      "untracked path inventory drifted since snapshot; take a fresh snapshot and reclassify before removal"
    continue
  fi
  if ! cmp -s "$PROOF_DIR/live-untracked.json" "$SNAPSHOT/untracked.json"; then
    block_retirement "$id" "$BLOCKER" "$CLASSIFICATION_LABEL" "$MAIN_PR_EVIDENCE" "$PRESERVED_SOURCE" "$RECOVERY_ACTION" \
      "readable untracked inventory evidence drifted since snapshot; take a fresh snapshot and reclassify before removal"
    continue
  fi
  if ! cmp -s "$PROOF_DIR/live-untracked.tar" "$SNAPSHOT/untracked.tar"; then
    block_retirement "$id" "$BLOCKER" "$CLASSIFICATION_LABEL" "$MAIN_PR_EVIDENCE" "$PRESERVED_SOURCE" "$RECOVERY_ACTION" \
      "untracked tar archive drifted since snapshot; take a fresh snapshot and reclassify before removal"
    continue
  fi
  git -C "$SOURCE" ls-files --others --ignored --exclude-standard -z > "$PROOF_DIR/live-ignored.zlist"
  python3 - "$PROOF_DIR/live-ignored.zlist" > "$PROOF_DIR/live-ignored.json" <<'PY'
import json
import sys
from pathlib import Path

raw = Path(sys.argv[1]).read_bytes()
entries = [] if not raw else raw.rstrip(b"\0").split(b"\0")
print(json.dumps([entry.decode("utf-8", "surrogateescape") for entry in entries], indent=2))
PY
  if ! cmp -s "$PROOF_DIR/live-ignored.zlist" "$SNAPSHOT/.ignored.zlist"; then
    block_retirement "$id" "$BLOCKER" "$CLASSIFICATION_LABEL" "$MAIN_PR_EVIDENCE" "$PRESERVED_SOURCE" "$RECOVERY_ACTION" \
      "ignored path inventory drifted since snapshot; leave the source worktree untouched, take a fresh snapshot, and reclassify before removal"
    continue
  fi
  if ! cmp -s "$PROOF_DIR/live-ignored.json" "$SNAPSHOT/ignored.json"; then
    block_retirement "$id" "$BLOCKER" "$CLASSIFICATION_LABEL" "$MAIN_PR_EVIDENCE" "$PRESERVED_SOURCE" "$RECOVERY_ACTION" \
      "readable ignored inventory evidence drifted since snapshot; leave the source worktree untouched, take a fresh snapshot, and reclassify before removal"
    continue
  fi
  if test -s "$SNAPSHOT/.ignored.zlist"; then
    block_retirement "$id" "$BLOCKER" "$CLASSIFICATION_LABEL" "$MAIN_PR_EVIDENCE" "$PRESERVED_SOURCE" "$RECOVERY_ACTION" \
      "preserved ignored-path inventory is non-empty, so forced source-worktree removal is prohibited"
    continue
  fi
  if ! REMOVE_BRANCH="$(git -C "$SOURCE" branch --show-current)" || test -z "$REMOVE_BRANCH"; then
    block_retirement "$id" "$BLOCKER" "$CLASSIFICATION_LABEL" "$MAIN_PR_EVIDENCE" "$PRESERVED_SOURCE" "$RECOVERY_ACTION" \
      "source worktree became detached or branchless immediately before forced removal; leave the worktree untouched"
    continue
  fi
  if [ "$REMOVE_BRANCH" != "$PRESERVED_BRANCH" ]; then
    block_retirement "$id" "$BLOCKER" "$CLASSIFICATION_LABEL" "$MAIN_PR_EVIDENCE" "$PRESERVED_SOURCE" "$RECOVERY_ACTION" \
      "source worktree branch drifted immediately before forced removal; leave the worktree untouched"
    {
      printf 'Preserved branch: %s\n' "$PRESERVED_BRANCH"
      printf 'Current branch: %s\n' "$REMOVE_BRANCH"
    } >> "$BLOCKER"
    continue
  fi
  git -C "$REPO" worktree remove --force "$SOURCE"
done
```

Expected: a row may retire only when its exact classification row says
`already-shipped` or `superseded` with structured `mode=non-viable-proof`
metadata plus either a raw merged `OpenCoven/coven` PR URL or an exact
40-character commit SHA that freshly verifies as an ancestor of fetched
`origin/main`, or `viable` with `Main/PR evidence` set to a recorded GitHub PR
URL and `Recovery action` set to a deterministic terminal PR mode. `pending`,
`blocked`, and `mode=awaiting-recovery-pr` rows never retire. At retirement,
every viable PR still has to freshly verify `OPEN`, `baseRefName=main`, and
`OpenCoven/coven`. Adopted source-branch rows additionally require their
parsed expected source branch to equal the current `headRefName`, and their
parsed preserved head to remain equal to or an ancestor of the current
`headRefOid`; they also require a freshly fetched authoritative `origin/<branch>`
tip to equal the current `headRefOid`; a force-push or divergence blocks
retirement with evidence. In later branch cleanup, exact preserved-head
equality remains the default deletion proof; the only allowed exception is a
viable adopted source-branch row whose exact expected branch matches the branch
being deleted, whose live local tip equals a freshly fetched `origin/<branch>`
tip, whose current PR is still the OPEN same-repo `main` PR at that exact
branch/tip, and whose preserved snapshot head is an ancestor of the advanced
live tip. Without that fresh proof, advanced local source-branch tips still
block deletion.
Adopted or newly opened recovery-branch rows additionally require the parsed
expected recovery branch to equal the current `headRefName`, a fresh exact
`origin/<branch>` refetch immediately before verification to resolve the
authoritative remote tip, and both that fetched tip and the current
`headRefOid` to equal parsed `expected_head`; any force-push or divergence
blocks retirement with evidence. These recovered-row checks do not require
ancestry to the old preserved local snapshot head because that work may have
been rebuilt on current `main`. Any unverifiable viable PR or non-viable proof
writes cleanup blocker evidence and leaves the original source worktree and
branch untouched. After that gate, the preserved branch identity must still
match immediately before forced removal, and the existing snapshot, live-drift,
and ignored-content comparisons still run before removal.

- [ ] **Step 5: Delete only proven local branch residue after worktree retirement**

For each branch in the ledger with merged or superseded evidence, or each of
the four dirty source branches once Step 4 has removed its worktree and proven
either the verified bundle or a pushed replacement recovery ref, set
`BRANCH_TO_DELETE` to its exact local branch name. Do not run this step for any
dirty-source branch whose classification action says `Leave source worktree and
branch untouched.` because preserved dirty delta remains behind an active PR.
Recheck that exact branch ref immediately before every deletion command against
the preserved head source for that branch; do not rely only on the earlier
worktree-retirement proof. Task 3 Step 3 already creates the explicit
orphan-branch `head.txt` files used below. Use this exact mapping for all seven
recovered source branches:

- `docs/psyche-specs` -> `dirty/docs-psyche-specs/head.txt`
- `docs/universal-runtime-capability-design` -> `branches/docs-universal-runtime-capability-design/head.txt`
- `feat/cmem-1ev-memory-promote` -> `dirty/memory-promote/head.txt`
- `feat/mobile-memory-gateway` -> `dirty/mobile-memory-gateway/head.txt`
- `feat/npm-macos-x64` -> `branches/feat-npm-macos-x64/head.txt`
- `fix/476-review-threads` -> `dirty/pr-476-review/head.txt`
- `fix/521-ward-surface-confinement` -> `branches/fix-521-ward-surface-confinement/head.txt`

Run this immediately before `branch -d`:

```bash
set -euo pipefail
resolve_control_worktree() {
  local control_branch="docs/541-incomplete-work-recovery-design"
  local expected_branch="refs/heads/$control_branch"
  local remote_branch_ref="refs/remotes/origin/$control_branch"
  local start_common_dir repo target_path path branch_ref actual_branch
  local target_registration_branch live_count stale_count live_path stale_path
  local live_status live_head local_head remote_head
  local -a live_paths=() stale_paths=()
  start_common_dir="$(git rev-parse --git-common-dir)"
  repo="$(cd "$start_common_dir/.." && pwd)"
  target_path="$repo/.worktrees/issue-541-recovery"
  if ! git -C "$repo" fetch --no-tags origin \
    "refs/heads/$control_branch:$remote_branch_ref"
  then
    printf 'Blocked: could not fetch origin/%s into %s.\n' \
      "$control_branch" "$remote_branch_ref" >&2
    exit 1
  fi
  remote_head="$(git -C "$repo" rev-parse --verify "$remote_branch_ref^{commit}")"
  while IFS="$(printf "\t")" read -r path branch_ref; do
    test -n "$path" || continue
    if test "$path" = "$target_path"; then
      target_registration_branch="$branch_ref"
    fi
    if test "$branch_ref" != "$expected_branch"; then
      continue
    fi
    if test -d "$path"; then
      live_paths+=("$path")
    else
      stale_paths+=("$path")
    fi
  done <<EOF2
$(git -C "$repo" worktree list --porcelain | awk '
  $1 == "worktree" { if (path != "") print path "\t" branch; path = substr($0, 10); branch = ""; next }
  $1 == "branch" { branch = $2; next }
  END { if (path != "") print path "\t" branch }
')
EOF2
  live_count="${#live_paths[@]}"
  stale_count="${#stale_paths[@]}"
  if test -n "$target_registration_branch" && test "$target_registration_branch" != "$expected_branch"; then
    printf 'Blocked: %s is registered to %s, not %s.\n' "$target_path" "$target_registration_branch" "$control_branch" >&2
    exit 1
  fi
  if test "$live_count" -gt 1; then
    printf 'Blocked: %s has %s live controller worktree registrations; refusing to choose arbitrarily.\n' "$control_branch" "$live_count" >&2
    for live_path in "${live_paths[@]}"; do
      printf '  live: %s\n' "$live_path" >&2
    done
    for stale_path in "${stale_paths[@]}"; do
      printf '  stale: %s\n' "$stale_path" >&2
    done
    exit 1
  fi
  if test "$live_count" -eq 1; then
    live_path="${live_paths[0]}"
    if test "$stale_count" -gt 0; then
      printf 'Warning: ignoring %s stale controller registration(s) for %s while using the one live worktree.\n' "$stale_count" "$control_branch" >&2
      for stale_path in "${stale_paths[@]}"; do
        printf '  stale: %s\n' "$stale_path" >&2
      done
    fi
    actual_branch="$(git -C "$live_path" branch --show-current)"
    if test "$actual_branch" != "$control_branch"; then
      printf 'Control worktree branch mismatch: expected %s, got %s\n' "$control_branch" "$actual_branch" >&2
      exit 1
    fi
    live_status="$(git -C "$live_path" status --porcelain=v1 --untracked-files=all)"
    if test -n "$live_status"; then
      printf 'Blocked: live controller worktree is dirty and cannot be refreshed safely: %s\n' "$live_path" >&2
      git -C "$live_path" status --short --branch --untracked-files=all >&2
      exit 1
    fi
    live_head="$(git -C "$live_path" rev-parse HEAD)"
    if test "$live_head" = "$remote_head"; then
      :
    elif git -C "$repo" merge-base --is-ancestor "$live_head" "$remote_head"; then
      if ! git -C "$live_path" merge --ff-only "$remote_branch_ref"; then
        printf 'Blocked: live controller worktree could not fast-forward %s to fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
        exit 1
      fi
    elif git -C "$repo" merge-base --is-ancestor "$remote_head" "$live_head"; then
      printf 'Blocked: live controller branch %s is ahead of fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
      exit 1
    else
      printf 'Blocked: live controller branch %s diverged from fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
      exit 1
    fi
    live_head="$(git -C "$live_path" rev-parse HEAD)"
    if test "$live_head" != "$remote_head"; then
      printf 'Blocked: live controller HEAD %s does not match fetched origin tip %s.\n' "$live_head" "$remote_head" >&2
      exit 1
    fi
    printf '%s\n' "$live_path"
    return 0
  fi
  if test "$stale_count" -gt 1; then
    printf 'Blocked: %s has %s stale controller registrations and no live worktree; refusing to choose arbitrarily.\n' "$control_branch" "$stale_count" >&2
    for stale_path in "${stale_paths[@]}"; do
      printf '  stale: %s\n' "$stale_path" >&2
    done
    exit 1
  fi
  if ! git -C "$repo" check-ignore -q .worktrees/; then
    printf 'Repository-local .worktrees/ is not ignored.\n' >&2
    exit 1
  fi
  if ! git -C "$repo" check-ignore -q .worktrees/issue-541-recovery; then
    printf 'Repository-local control worktree path is not ignored: %s\n' '.worktrees/issue-541-recovery' >&2
    exit 1
  fi
  mkdir -p "$repo/.worktrees"
  if test -e "$target_path"; then
    printf 'Control worktree path already exists without a matching live registration: %s\n' "$target_path" >&2
    exit 1
  fi
  if git -C "$repo" show-ref --verify --quiet "$expected_branch"; then
    local_head="$(git -C "$repo" rev-parse --verify "$expected_branch^{commit}")"
    if test "$local_head" = "$remote_head"; then
      :
    elif git -C "$repo" merge-base --is-ancestor "$local_head" "$remote_head"; then
      if ! git -C "$repo" update-ref "$expected_branch" "$remote_head" "$local_head"; then
        printf 'Blocked: local controller branch %s could not be fast-forwarded to fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
        exit 1
      fi
    elif git -C "$repo" merge-base --is-ancestor "$remote_head" "$local_head"; then
      printf 'Blocked: local controller branch %s is ahead of fetched origin tip %s and cannot be rewritten safely.\n' "$control_branch" "$remote_head" >&2
      exit 1
    else
      printf 'Blocked: local controller branch %s diverged from fetched origin tip %s and cannot be rewritten safely.\n' "$control_branch" "$remote_head" >&2
      exit 1
    fi
  else
    if ! git -C "$repo" update-ref "$expected_branch" "$remote_head"; then
      printf 'Blocked: local controller branch %s could not be created at fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
      exit 1
    fi
  fi
  local_head="$(git -C "$repo" rev-parse --verify "$expected_branch^{commit}")"
  if test "$local_head" != "$remote_head"; then
    printf 'Blocked: local controller branch %s resolved to %s instead of fetched origin tip %s.\n' "$control_branch" "$local_head" "$remote_head" >&2
    exit 1
  fi
  if test "$stale_count" -eq 1; then
    stale_path="${stale_paths[0]}"
    printf 'Recreating %s at %s from the one stale controller registration:\n  stale: %s\n' "$control_branch" "$target_path" "$stale_path" >&2
    git -C "$repo" worktree add --force "$target_path" "$control_branch"
  else
    git -C "$repo" worktree add "$target_path" "$control_branch"
  fi
  if ! test -d "$target_path"; then
    printf 'Control worktree path is not present after resolution: %s\n' "$target_path" >&2
    exit 1
  fi
  actual_branch="$(git -C "$target_path" branch --show-current)"
  if test "$actual_branch" != "$control_branch"; then
    printf 'Control worktree branch mismatch: expected %s, got %s\n' "$control_branch" "$actual_branch" >&2
    exit 1
  fi
  local_head="$(git -C "$target_path" rev-parse HEAD)"
  if test "$local_head" != "$remote_head"; then
    printf 'Blocked: resolved controller HEAD %s does not match fetched origin tip %s.\n' "$local_head" "$remote_head" >&2
    exit 1
  fi
  printf '%s\n' "$target_path"
}
CONTROL_WORKTREE="$(resolve_control_worktree)"
COMMON_DIR="$(git -C "$CONTROL_WORKTREE" rev-parse --git-common-dir)"
REPO="$(cd "$COMMON_DIR/.." && pwd)"
BRANCH_DELETE_PROOF_ROOT="$COMMON_DIR/agent-recovery/issue-541/private/branch-delete-proof"
mkdir -p "$BRANCH_DELETE_PROOF_ROOT"
CLASSIFICATION="$COMMON_DIR/agent-recovery/issue-541/classification.md"
parse_classification_row() {
  python3 - "$CLASSIFICATION" "$1" <<'PY'
import sys
from pathlib import Path

classification_path = Path(sys.argv[1])
workstream = sys.argv[2]
for raw in classification_path.read_text(encoding="utf-8").splitlines():
    line = raw.strip()
    if not line.startswith("|") or not line.endswith("|"):
        continue
    cells = [cell.strip() for cell in line.strip("|").split("|")]
    if len(cells) != 5 or cells[0] == "Workstream":
        continue
    if cells[0] != workstream:
        continue
    print(cells[1])
    print(cells[2])
    print(cells[3])
    print(cells[4])
    break
else:
    raise SystemExit(f"Missing classification row for {workstream}")
PY
}
parse_recovery_action() {
  python3 - "$1" <<'PY'
import sys

action = sys.argv[1].strip()
fields = {}
for raw_part in action.split(";"):
    part = raw_part.strip()
    if not part:
        continue
    if "=" not in part:
        raise SystemExit(f"Recovery action segment is not key=value: {part}")
    key, value = part.split("=", 1)
    key = key.strip()
    value = value.strip()
    if not key or not value:
        raise SystemExit(f"Recovery action segment has an empty key or value: {part}")
    if key in fields:
        raise SystemExit(f"Recovery action repeats key {key}")
    fields[key] = value

mode = fields.get("mode")
pr_kind = fields.get("pr_kind")
required = ["mode", "pr_kind", "issue_url", "archive_id", "expected_branch"]
if mode == "continue-existing-pr":
    if pr_kind == "adopted":
        required.append("preserved_head")
    elif pr_kind == "recovered":
        required.append("expected_head")
    else:
        raise SystemExit(f"Unsupported continue-existing-pr kind: {pr_kind}")
elif mode in {"awaiting-recovery-pr", "recovery-pr-open"}:
    if pr_kind != "recovered":
        raise SystemExit(f"{mode} must declare pr_kind=recovered, got {pr_kind}")
    if mode == "recovery-pr-open":
        required.append("expected_head")
else:
    raise SystemExit(f"Unsupported recovery action mode: {mode}")

for key in required:
    if key not in fields:
        raise SystemExit(f"Recovery action is missing {key}")

print(fields["mode"])
print(fields["pr_kind"])
print(fields["issue_url"])
print(fields["archive_id"])
print(fields["expected_branch"])
print(fields.get("expected_head", ""))
print(fields.get("preserved_head", ""))
PY
}
recheck_branch_ref_tip() {
  MODE="$1"
  BRANCH_PROOF_ID="$(printf '%s' "$BRANCH_TO_DELETE" | tr '/' '_')"
  case "$BRANCH_TO_DELETE" in
    docs/psyche-specs)
      WORKSTREAM_ID="docs-psyche-specs"
      PRESERVED_HEAD_SOURCE="dirty/docs-psyche-specs/head.txt"
      ;;
    docs/universal-runtime-capability-design)
      WORKSTREAM_ID="docs-universal-runtime-capability-design"
      PRESERVED_HEAD_SOURCE="branches/docs-universal-runtime-capability-design/head.txt"
      ;;
    feat/cmem-1ev-memory-promote)
      WORKSTREAM_ID="memory-promote"
      PRESERVED_HEAD_SOURCE="dirty/memory-promote/head.txt"
      ;;
    feat/mobile-memory-gateway)
      WORKSTREAM_ID="mobile-memory-gateway"
      PRESERVED_HEAD_SOURCE="dirty/mobile-memory-gateway/head.txt"
      ;;
    feat/npm-macos-x64)
      WORKSTREAM_ID="feat-npm-macos-x64"
      PRESERVED_HEAD_SOURCE="branches/feat-npm-macos-x64/head.txt"
      ;;
    fix/476-review-threads)
      WORKSTREAM_ID="pr-476-review"
      PRESERVED_HEAD_SOURCE="dirty/pr-476-review/head.txt"
      ;;
    fix/521-ward-surface-confinement)
      WORKSTREAM_ID="fix-521-ward-surface-confinement"
      PRESERVED_HEAD_SOURCE="branches/fix-521-ward-surface-confinement/head.txt"
      ;;
    *)
      printf 'Blocked: unknown recovered source branch %s.\n' "$BRANCH_TO_DELETE" \
        > "$BRANCH_DELETE_PROOF_ROOT/$BRANCH_PROOF_ID-$MODE.txt"
      return 1
      ;;
  esac
  PRESERVED_HEAD_FILE="$COMMON_DIR/agent-recovery/issue-541/$PRESERVED_HEAD_SOURCE"
  PROOF_FILE="$BRANCH_DELETE_PROOF_ROOT/$BRANCH_PROOF_ID-$MODE.txt"
  LIVE_HEAD_FILE="$BRANCH_DELETE_PROOF_ROOT/$BRANCH_PROOF_ID-$MODE-live-head.txt"
  LIVE_ERR_FILE="$BRANCH_DELETE_PROOF_ROOT/$BRANCH_PROOF_ID-$MODE-live-head.err"
  if ! test -s "$PRESERVED_HEAD_FILE"; then
    printf 'Blocked %s: preserved head file is missing: %s\n' \
      "$BRANCH_TO_DELETE" "$PRESERVED_HEAD_SOURCE" > "$PROOF_FILE"
    return 1
  fi
  if ! git -C "$REPO" rev-parse --verify "refs/heads/$BRANCH_TO_DELETE" \
    > "$LIVE_HEAD_FILE" 2> "$LIVE_ERR_FILE"
  then
    {
      printf 'Branch: %s\n' "$BRANCH_TO_DELETE"
      printf 'Workstream: %s\n' "$WORKSTREAM_ID"
      printf 'Preserved head source: %s\n' "$PRESERVED_HEAD_SOURCE"
      printf 'Outcome: local branch ref is already missing immediately before %s; no deletion command ran.\n' "$MODE"
    } > "$PROOF_FILE"
    rm -f "$LIVE_HEAD_FILE" "$LIVE_ERR_FILE"
    return 2
  fi
  PRESERVED_HEAD="$(tr -d '\n' < "$PRESERVED_HEAD_FILE")"
  LIVE_HEAD="$(tr -d '\n' < "$LIVE_HEAD_FILE")"
  {
    printf 'Branch: %s\n' "$BRANCH_TO_DELETE"
    printf 'Workstream: %s\n' "$WORKSTREAM_ID"
    printf 'Preserved head source: %s\n' "$PRESERVED_HEAD_SOURCE"
    printf 'Preserved head: %s\n' "$PRESERVED_HEAD"
    printf 'Live branch ref tip: %s\n' "$LIVE_HEAD"
  } > "$PROOF_FILE"
  if [ "$LIVE_HEAD" = "$PRESERVED_HEAD" ]; then
    return 0
  fi
  CLASSIFICATION_FIELDS_FILE="$BRANCH_DELETE_PROOF_ROOT/$BRANCH_PROOF_ID-$MODE-classification.txt"
  ACTION_FIELDS_FILE="$BRANCH_DELETE_PROOF_ROOT/$BRANCH_PROOF_ID-$MODE-action.txt"
  FETCH_EVIDENCE="$BRANCH_DELETE_PROOF_ROOT/$BRANCH_PROOF_ID-$MODE-fetch.txt"
  PR_VIEW_EVIDENCE="$BRANCH_DELETE_PROOF_ROOT/$BRANCH_PROOF_ID-$MODE-pr-view.json"
  PR_VIEW_ERR="$BRANCH_DELETE_PROOF_ROOT/$BRANCH_PROOF_ID-$MODE-pr-view.err"
  if ! parse_classification_row "$WORKSTREAM_ID" > "$CLASSIFICATION_FIELDS_FILE"; then
    printf 'Blocked: live branch ref tip differs from the preserved head, and the classification row could not be parsed for fresh adopted-PR proof.\n' \
      >> "$PROOF_FILE"
    rm -f "$CLASSIFICATION_FIELDS_FILE" "$ACTION_FIELDS_FILE" "$FETCH_EVIDENCE" "$PR_VIEW_EVIDENCE" "$PR_VIEW_ERR"
    return 1
  fi
  CLASSIFICATION_ROW=()
  CLASSIFICATION_FIELD=
  while IFS= read -r CLASSIFICATION_FIELD || [ -n "$CLASSIFICATION_FIELD" ]; do
    CLASSIFICATION_ROW+=("$CLASSIFICATION_FIELD")
  done < "$CLASSIFICATION_FIELDS_FILE"
  rm -f "$CLASSIFICATION_FIELDS_FILE"
  if test "${#CLASSIFICATION_ROW[@]}" -ne 4; then
    printf 'Blocked: classification row parsing returned an unexpected field count for adopted-PR proof.\n' >> "$PROOF_FILE"
    rm -f "$ACTION_FIELDS_FILE" "$FETCH_EVIDENCE" "$PR_VIEW_EVIDENCE" "$PR_VIEW_ERR"
    return 1
  fi
  CLASSIFICATION_LABEL="${CLASSIFICATION_ROW[0]}"
  MAIN_PR_EVIDENCE="${CLASSIFICATION_ROW[1]}"
  RECOVERY_ACTION="${CLASSIFICATION_ROW[3]}"
  case "$MAIN_PR_EVIDENCE" in
    https://github.com/OpenCoven/coven/pull/*)
      ;;
    *)
      printf 'Blocked: live branch ref tip differs from the preserved head and the exact row does not record a raw OpenCoven/coven PR URL.\n' >> "$PROOF_FILE"
      rm -f "$ACTION_FIELDS_FILE" "$FETCH_EVIDENCE" "$PR_VIEW_EVIDENCE" "$PR_VIEW_ERR"
      return 1
      ;;
  esac
  if [ "$CLASSIFICATION_LABEL" != "viable" ]; then
    printf 'Blocked: live branch ref tip differs from the preserved head and only viable adopted source-branch rows may use the advanced remote proof.\n' >> "$PROOF_FILE"
    rm -f "$ACTION_FIELDS_FILE" "$FETCH_EVIDENCE" "$PR_VIEW_EVIDENCE" "$PR_VIEW_ERR"
    return 1
  fi
  if ! parse_recovery_action "$RECOVERY_ACTION" > "$ACTION_FIELDS_FILE"; then
    printf 'Blocked: viable row recovery action could not be parsed for adopted source-branch proof.\n' >> "$PROOF_FILE"
    rm -f "$ACTION_FIELDS_FILE" "$FETCH_EVIDENCE" "$PR_VIEW_EVIDENCE" "$PR_VIEW_ERR"
    return 1
  fi
  ACTION_ROW=()
  ACTION_FIELD=
  while IFS= read -r ACTION_FIELD || [ -n "$ACTION_FIELD" ]; do
    ACTION_ROW+=("$ACTION_FIELD")
  done < "$ACTION_FIELDS_FILE"
  rm -f "$ACTION_FIELDS_FILE"
  if test "${#ACTION_ROW[@]}" -ne 7; then
    printf 'Blocked: viable row recovery action parsing returned an unexpected field count.\n' >> "$PROOF_FILE"
    rm -f "$FETCH_EVIDENCE" "$PR_VIEW_EVIDENCE" "$PR_VIEW_ERR"
    return 1
  fi
  ACTION_MODE="${ACTION_ROW[0]}"
  ACTION_PR_KIND="${ACTION_ROW[1]}"
  ACTION_EXPECTED_BRANCH="${ACTION_ROW[4]}"
  ACTION_PRESERVED_HEAD="${ACTION_ROW[6]}"
  if [ "$ACTION_MODE" != "continue-existing-pr" ] || [ "$ACTION_PR_KIND" != "adopted" ]; then
    printf 'Blocked: live branch ref tip differs from the preserved head and the row is not a viable adopted source-branch PR.\n' >> "$PROOF_FILE"
    rm -f "$FETCH_EVIDENCE" "$PR_VIEW_EVIDENCE" "$PR_VIEW_ERR"
    return 1
  fi
  if [ "$ACTION_EXPECTED_BRANCH" != "$BRANCH_TO_DELETE" ]; then
    printf 'Blocked: adopted source-branch proof expected branch %s, not %s.\n' "$ACTION_EXPECTED_BRANCH" "$BRANCH_TO_DELETE" >> "$PROOF_FILE"
    rm -f "$FETCH_EVIDENCE" "$PR_VIEW_EVIDENCE" "$PR_VIEW_ERR"
    return 1
  fi
  if [ "$ACTION_PRESERVED_HEAD" != "$PRESERVED_HEAD" ]; then
    printf 'Blocked: adopted source-branch proof preserved head %s does not match snapshot head %s.\n' "$ACTION_PRESERVED_HEAD" "$PRESERVED_HEAD" >> "$PROOF_FILE"
    rm -f "$FETCH_EVIDENCE" "$PR_VIEW_EVIDENCE" "$PR_VIEW_ERR"
    return 1
  fi
  {
    printf 'Advanced adopted-source-branch proof for %s\n' "$BRANCH_TO_DELETE"
    printf 'Fetching origin/%s immediately before deletion proof.\n' "$BRANCH_TO_DELETE"
  } > "$FETCH_EVIDENCE"
  if ! git -C "$REPO" fetch --no-tags origin \
    "refs/heads/$BRANCH_TO_DELETE:refs/remotes/origin/$BRANCH_TO_DELETE" \
    >> "$FETCH_EVIDENCE" 2>&1
  then
    printf 'Blocked: could not fetch origin/%s for advanced adopted-source-branch proof.\n' "$BRANCH_TO_DELETE" >> "$PROOF_FILE"
    cat "$FETCH_EVIDENCE" >> "$PROOF_FILE"
    rm -f "$PR_VIEW_EVIDENCE" "$PR_VIEW_ERR"
    return 1
  fi
  REMOTE_HEAD="$(git -C "$REPO" rev-parse "refs/remotes/origin/$BRANCH_TO_DELETE")"
  {
    printf 'Fetched remote tip: %s\n' "$REMOTE_HEAD"
    printf 'Live branch ref tip: %s\n' "$LIVE_HEAD"
  } >> "$FETCH_EVIDENCE"
  if [ "$LIVE_HEAD" != "$REMOTE_HEAD" ]; then
    printf 'Blocked: live branch ref tip differs from freshly fetched origin/%s, so advanced commits are not yet proven remote-backed.\n' "$BRANCH_TO_DELETE" >> "$PROOF_FILE"
    cat "$FETCH_EVIDENCE" >> "$PROOF_FILE"
    rm -f "$PR_VIEW_EVIDENCE" "$PR_VIEW_ERR"
    return 1
  fi
  if ! gh pr view --repo OpenCoven/coven "$MAIN_PR_EVIDENCE" \
    --json url,state,headRefOid,headRefName,headRepositoryOwner,isCrossRepository,baseRefName \
    > "$PR_VIEW_EVIDENCE" 2> "$PR_VIEW_ERR"
  then
    printf 'Blocked: adopted PR %s could not be freshly verified for advanced source-branch deletion proof.\n' "$MAIN_PR_EVIDENCE" >> "$PROOF_FILE"
    cat "$PR_VIEW_ERR" >> "$PROOF_FILE"
    return 1
  fi
  ACTUAL_URL="$(jq -r '.url' "$PR_VIEW_EVIDENCE")"
  ACTUAL_STATE="$(jq -r '.state' "$PR_VIEW_EVIDENCE")"
  ACTUAL_HEAD="$(jq -r '.headRefOid' "$PR_VIEW_EVIDENCE")"
  ACTUAL_BRANCH="$(jq -r '.headRefName' "$PR_VIEW_EVIDENCE")"
  ACTUAL_OWNER="$(jq -r '.headRepositoryOwner.login' "$PR_VIEW_EVIDENCE")"
  ACTUAL_CROSS="$(jq -r '.isCrossRepository' "$PR_VIEW_EVIDENCE")"
  ACTUAL_BASE="$(jq -r '.baseRefName' "$PR_VIEW_EVIDENCE")"
  {
    printf 'Verified PR URL: %s\n' "$ACTUAL_URL"
    printf 'Verified PR state: %s\n' "$ACTUAL_STATE"
    printf 'Verified PR head branch: %s\n' "$ACTUAL_BRANCH"
    printf 'Verified PR head tip: %s\n' "$ACTUAL_HEAD"
    printf 'Verified PR base: %s\n' "$ACTUAL_BASE"
    printf 'Verified PR owner: %s\n' "$ACTUAL_OWNER"
    printf 'Verified PR cross-repository: %s\n' "$ACTUAL_CROSS"
  } >> "$PROOF_FILE"
  if [ "$ACTUAL_URL" != "$MAIN_PR_EVIDENCE" ] || \
     [ "$ACTUAL_STATE" != "OPEN" ] || \
     [ "$ACTUAL_BASE" != "main" ] || \
     [ "$ACTUAL_OWNER" != "OpenCoven" ] || \
     [ "$ACTUAL_CROSS" != "false" ] || \
     [ "$ACTUAL_BRANCH" != "$BRANCH_TO_DELETE" ] || \
     [ "$ACTUAL_HEAD" != "$REMOTE_HEAD" ]; then
    printf 'Blocked: adopted PR proof must verify one OPEN same-repo main PR whose head branch and headRefOid match the fetched live branch tip.\n' >> "$PROOF_FILE"
    return 1
  fi
  if ! git -C "$REPO" merge-base --is-ancestor "$PRESERVED_HEAD" "$LIVE_HEAD"; then
    printf 'Blocked: preserved snapshot head is not an ancestor of the live adopted branch tip.\n' >> "$PROOF_FILE"
    return 1
  fi
  printf 'Advanced adopted-source-branch proof succeeded: newer commits are preserved remotely and in the OPEN PR.\n' >> "$PROOF_FILE"
  return 0
}
if recheck_branch_ref_tip pre-delete-d; then
  :
else
  CHECK_STATUS=$?
  case "$CHECK_STATUS" in
    2)
      exit 0
      ;;
    *)
      exit "$CHECK_STATUS"
      ;;
  esac
fi
git -C "$REPO" branch -d "$BRANCH_TO_DELETE"
```

If squash history makes `-d` refuse, recheck the ledger evidence, reconfirm
its source worktree is already removed and its verified snapshot bundle or
pushed replacement branch still proves the committed history, then rerun the
same branch-ref proof immediately before `branch -D`:

```bash
set -euo pipefail
resolve_control_worktree() {
  local control_branch="docs/541-incomplete-work-recovery-design"
  local expected_branch="refs/heads/$control_branch"
  local remote_branch_ref="refs/remotes/origin/$control_branch"
  local start_common_dir repo target_path path branch_ref actual_branch
  local target_registration_branch live_count stale_count live_path stale_path
  local live_status live_head local_head remote_head
  local -a live_paths=() stale_paths=()
  start_common_dir="$(git rev-parse --git-common-dir)"
  repo="$(cd "$start_common_dir/.." && pwd)"
  target_path="$repo/.worktrees/issue-541-recovery"
  if ! git -C "$repo" fetch --no-tags origin \
    "refs/heads/$control_branch:$remote_branch_ref"
  then
    printf 'Blocked: could not fetch origin/%s into %s.\n' \
      "$control_branch" "$remote_branch_ref" >&2
    exit 1
  fi
  remote_head="$(git -C "$repo" rev-parse --verify "$remote_branch_ref^{commit}")"
  while IFS="$(printf "\t")" read -r path branch_ref; do
    test -n "$path" || continue
    if test "$path" = "$target_path"; then
      target_registration_branch="$branch_ref"
    fi
    if test "$branch_ref" != "$expected_branch"; then
      continue
    fi
    if test -d "$path"; then
      live_paths+=("$path")
    else
      stale_paths+=("$path")
    fi
  done <<EOF2
$(git -C "$repo" worktree list --porcelain | awk '
  $1 == "worktree" { if (path != "") print path "\t" branch; path = substr($0, 10); branch = ""; next }
  $1 == "branch" { branch = $2; next }
  END { if (path != "") print path "\t" branch }
')
EOF2
  live_count="${#live_paths[@]}"
  stale_count="${#stale_paths[@]}"
  if test -n "$target_registration_branch" && test "$target_registration_branch" != "$expected_branch"; then
    printf 'Blocked: %s is registered to %s, not %s.\n' "$target_path" "$target_registration_branch" "$control_branch" >&2
    exit 1
  fi
  if test "$live_count" -gt 1; then
    printf 'Blocked: %s has %s live controller worktree registrations; refusing to choose arbitrarily.\n' "$control_branch" "$live_count" >&2
    for live_path in "${live_paths[@]}"; do
      printf '  live: %s\n' "$live_path" >&2
    done
    for stale_path in "${stale_paths[@]}"; do
      printf '  stale: %s\n' "$stale_path" >&2
    done
    exit 1
  fi
  if test "$live_count" -eq 1; then
    live_path="${live_paths[0]}"
    if test "$stale_count" -gt 0; then
      printf 'Warning: ignoring %s stale controller registration(s) for %s while using the one live worktree.\n' "$stale_count" "$control_branch" >&2
      for stale_path in "${stale_paths[@]}"; do
        printf '  stale: %s\n' "$stale_path" >&2
      done
    fi
    actual_branch="$(git -C "$live_path" branch --show-current)"
    if test "$actual_branch" != "$control_branch"; then
      printf 'Control worktree branch mismatch: expected %s, got %s\n' "$control_branch" "$actual_branch" >&2
      exit 1
    fi
    live_status="$(git -C "$live_path" status --porcelain=v1 --untracked-files=all)"
    if test -n "$live_status"; then
      printf 'Blocked: live controller worktree is dirty and cannot be refreshed safely: %s\n' "$live_path" >&2
      git -C "$live_path" status --short --branch --untracked-files=all >&2
      exit 1
    fi
    live_head="$(git -C "$live_path" rev-parse HEAD)"
    if test "$live_head" = "$remote_head"; then
      :
    elif git -C "$repo" merge-base --is-ancestor "$live_head" "$remote_head"; then
      if ! git -C "$live_path" merge --ff-only "$remote_branch_ref"; then
        printf 'Blocked: live controller worktree could not fast-forward %s to fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
        exit 1
      fi
    elif git -C "$repo" merge-base --is-ancestor "$remote_head" "$live_head"; then
      printf 'Blocked: live controller branch %s is ahead of fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
      exit 1
    else
      printf 'Blocked: live controller branch %s diverged from fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
      exit 1
    fi
    live_head="$(git -C "$live_path" rev-parse HEAD)"
    if test "$live_head" != "$remote_head"; then
      printf 'Blocked: live controller HEAD %s does not match fetched origin tip %s.\n' "$live_head" "$remote_head" >&2
      exit 1
    fi
    printf '%s\n' "$live_path"
    return 0
  fi
  if test "$stale_count" -gt 1; then
    printf 'Blocked: %s has %s stale controller registrations and no live worktree; refusing to choose arbitrarily.\n' "$control_branch" "$stale_count" >&2
    for stale_path in "${stale_paths[@]}"; do
      printf '  stale: %s\n' "$stale_path" >&2
    done
    exit 1
  fi
  if ! git -C "$repo" check-ignore -q .worktrees/; then
    printf 'Repository-local .worktrees/ is not ignored.\n' >&2
    exit 1
  fi
  if ! git -C "$repo" check-ignore -q .worktrees/issue-541-recovery; then
    printf 'Repository-local control worktree path is not ignored: %s\n' '.worktrees/issue-541-recovery' >&2
    exit 1
  fi
  mkdir -p "$repo/.worktrees"
  if test -e "$target_path"; then
    printf 'Control worktree path already exists without a matching live registration: %s\n' "$target_path" >&2
    exit 1
  fi
  if git -C "$repo" show-ref --verify --quiet "$expected_branch"; then
    local_head="$(git -C "$repo" rev-parse --verify "$expected_branch^{commit}")"
    if test "$local_head" = "$remote_head"; then
      :
    elif git -C "$repo" merge-base --is-ancestor "$local_head" "$remote_head"; then
      if ! git -C "$repo" update-ref "$expected_branch" "$remote_head" "$local_head"; then
        printf 'Blocked: local controller branch %s could not be fast-forwarded to fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
        exit 1
      fi
    elif git -C "$repo" merge-base --is-ancestor "$remote_head" "$local_head"; then
      printf 'Blocked: local controller branch %s is ahead of fetched origin tip %s and cannot be rewritten safely.\n' "$control_branch" "$remote_head" >&2
      exit 1
    else
      printf 'Blocked: local controller branch %s diverged from fetched origin tip %s and cannot be rewritten safely.\n' "$control_branch" "$remote_head" >&2
      exit 1
    fi
  else
    if ! git -C "$repo" update-ref "$expected_branch" "$remote_head"; then
      printf 'Blocked: local controller branch %s could not be created at fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
      exit 1
    fi
  fi
  local_head="$(git -C "$repo" rev-parse --verify "$expected_branch^{commit}")"
  if test "$local_head" != "$remote_head"; then
    printf 'Blocked: local controller branch %s resolved to %s instead of fetched origin tip %s.\n' "$control_branch" "$local_head" "$remote_head" >&2
    exit 1
  fi
  if test "$stale_count" -eq 1; then
    stale_path="${stale_paths[0]}"
    printf 'Recreating %s at %s from the one stale controller registration:\n  stale: %s\n' "$control_branch" "$target_path" "$stale_path" >&2
    git -C "$repo" worktree add --force "$target_path" "$control_branch"
  else
    git -C "$repo" worktree add "$target_path" "$control_branch"
  fi
  if ! test -d "$target_path"; then
    printf 'Control worktree path is not present after resolution: %s\n' "$target_path" >&2
    exit 1
  fi
  actual_branch="$(git -C "$target_path" branch --show-current)"
  if test "$actual_branch" != "$control_branch"; then
    printf 'Control worktree branch mismatch: expected %s, got %s\n' "$control_branch" "$actual_branch" >&2
    exit 1
  fi
  local_head="$(git -C "$target_path" rev-parse HEAD)"
  if test "$local_head" != "$remote_head"; then
    printf 'Blocked: resolved controller HEAD %s does not match fetched origin tip %s.\n' "$local_head" "$remote_head" >&2
    exit 1
  fi
  printf '%s\n' "$target_path"
}
CONTROL_WORKTREE="$(resolve_control_worktree)"
COMMON_DIR="$(git -C "$CONTROL_WORKTREE" rev-parse --git-common-dir)"
REPO="$(cd "$COMMON_DIR/.." && pwd)"
BRANCH_DELETE_PROOF_ROOT="$COMMON_DIR/agent-recovery/issue-541/private/branch-delete-proof"
mkdir -p "$BRANCH_DELETE_PROOF_ROOT"
CLASSIFICATION="$COMMON_DIR/agent-recovery/issue-541/classification.md"
parse_classification_row() {
  python3 - "$CLASSIFICATION" "$1" <<'PY'
import sys
from pathlib import Path

classification_path = Path(sys.argv[1])
workstream = sys.argv[2]
for raw in classification_path.read_text(encoding="utf-8").splitlines():
    line = raw.strip()
    if not line.startswith("|") or not line.endswith("|"):
        continue
    cells = [cell.strip() for cell in line.strip("|").split("|")]
    if len(cells) != 5 or cells[0] == "Workstream":
        continue
    if cells[0] != workstream:
        continue
    print(cells[1])
    print(cells[2])
    print(cells[3])
    print(cells[4])
    break
else:
    raise SystemExit(f"Missing classification row for {workstream}")
PY
}
parse_recovery_action() {
  python3 - "$1" <<'PY'
import sys

action = sys.argv[1].strip()
fields = {}
for raw_part in action.split(";"):
    part = raw_part.strip()
    if not part:
        continue
    if "=" not in part:
        raise SystemExit(f"Recovery action segment is not key=value: {part}")
    key, value = part.split("=", 1)
    key = key.strip()
    value = value.strip()
    if not key or not value:
        raise SystemExit(f"Recovery action segment has an empty key or value: {part}")
    if key in fields:
        raise SystemExit(f"Recovery action repeats key {key}")
    fields[key] = value

mode = fields.get("mode")
pr_kind = fields.get("pr_kind")
required = ["mode", "pr_kind", "issue_url", "archive_id", "expected_branch"]
if mode == "continue-existing-pr":
    if pr_kind == "adopted":
        required.append("preserved_head")
    elif pr_kind == "recovered":
        required.append("expected_head")
    else:
        raise SystemExit(f"Unsupported continue-existing-pr kind: {pr_kind}")
elif mode in {"awaiting-recovery-pr", "recovery-pr-open"}:
    if pr_kind != "recovered":
        raise SystemExit(f"{mode} must declare pr_kind=recovered, got {pr_kind}")
    if mode == "recovery-pr-open":
        required.append("expected_head")
else:
    raise SystemExit(f"Unsupported recovery action mode: {mode}")

for key in required:
    if key not in fields:
        raise SystemExit(f"Recovery action is missing {key}")

print(fields["mode"])
print(fields["pr_kind"])
print(fields["issue_url"])
print(fields["archive_id"])
print(fields["expected_branch"])
print(fields.get("expected_head", ""))
print(fields.get("preserved_head", ""))
PY
}
recheck_branch_ref_tip() {
  MODE="$1"
  BRANCH_PROOF_ID="$(printf '%s' "$BRANCH_TO_DELETE" | tr '/' '_')"
  case "$BRANCH_TO_DELETE" in
    docs/psyche-specs)
      WORKSTREAM_ID="docs-psyche-specs"
      PRESERVED_HEAD_SOURCE="dirty/docs-psyche-specs/head.txt"
      ;;
    docs/universal-runtime-capability-design)
      WORKSTREAM_ID="docs-universal-runtime-capability-design"
      PRESERVED_HEAD_SOURCE="branches/docs-universal-runtime-capability-design/head.txt"
      ;;
    feat/cmem-1ev-memory-promote)
      WORKSTREAM_ID="memory-promote"
      PRESERVED_HEAD_SOURCE="dirty/memory-promote/head.txt"
      ;;
    feat/mobile-memory-gateway)
      WORKSTREAM_ID="mobile-memory-gateway"
      PRESERVED_HEAD_SOURCE="dirty/mobile-memory-gateway/head.txt"
      ;;
    feat/npm-macos-x64)
      WORKSTREAM_ID="feat-npm-macos-x64"
      PRESERVED_HEAD_SOURCE="branches/feat-npm-macos-x64/head.txt"
      ;;
    fix/476-review-threads)
      WORKSTREAM_ID="pr-476-review"
      PRESERVED_HEAD_SOURCE="dirty/pr-476-review/head.txt"
      ;;
    fix/521-ward-surface-confinement)
      WORKSTREAM_ID="fix-521-ward-surface-confinement"
      PRESERVED_HEAD_SOURCE="branches/fix-521-ward-surface-confinement/head.txt"
      ;;
    *)
      printf 'Blocked: unknown recovered source branch %s.\n' "$BRANCH_TO_DELETE" \
        > "$BRANCH_DELETE_PROOF_ROOT/$BRANCH_PROOF_ID-$MODE.txt"
      return 1
      ;;
  esac
  PRESERVED_HEAD_FILE="$COMMON_DIR/agent-recovery/issue-541/$PRESERVED_HEAD_SOURCE"
  PROOF_FILE="$BRANCH_DELETE_PROOF_ROOT/$BRANCH_PROOF_ID-$MODE.txt"
  LIVE_HEAD_FILE="$BRANCH_DELETE_PROOF_ROOT/$BRANCH_PROOF_ID-$MODE-live-head.txt"
  LIVE_ERR_FILE="$BRANCH_DELETE_PROOF_ROOT/$BRANCH_PROOF_ID-$MODE-live-head.err"
  if ! test -s "$PRESERVED_HEAD_FILE"; then
    printf 'Blocked %s: preserved head file is missing: %s\n' \
      "$BRANCH_TO_DELETE" "$PRESERVED_HEAD_SOURCE" > "$PROOF_FILE"
    return 1
  fi
  if ! git -C "$REPO" rev-parse --verify "refs/heads/$BRANCH_TO_DELETE" \
    > "$LIVE_HEAD_FILE" 2> "$LIVE_ERR_FILE"
  then
    {
      printf 'Branch: %s\n' "$BRANCH_TO_DELETE"
      printf 'Workstream: %s\n' "$WORKSTREAM_ID"
      printf 'Preserved head source: %s\n' "$PRESERVED_HEAD_SOURCE"
      printf 'Outcome: local branch ref is already missing immediately before %s; no deletion command ran.\n' "$MODE"
    } > "$PROOF_FILE"
    rm -f "$LIVE_HEAD_FILE" "$LIVE_ERR_FILE"
    return 2
  fi
  PRESERVED_HEAD="$(tr -d '\n' < "$PRESERVED_HEAD_FILE")"
  LIVE_HEAD="$(tr -d '\n' < "$LIVE_HEAD_FILE")"
  {
    printf 'Branch: %s\n' "$BRANCH_TO_DELETE"
    printf 'Workstream: %s\n' "$WORKSTREAM_ID"
    printf 'Preserved head source: %s\n' "$PRESERVED_HEAD_SOURCE"
    printf 'Preserved head: %s\n' "$PRESERVED_HEAD"
    printf 'Live branch ref tip: %s\n' "$LIVE_HEAD"
  } > "$PROOF_FILE"
  if [ "$LIVE_HEAD" = "$PRESERVED_HEAD" ]; then
    return 0
  fi
  CLASSIFICATION_FIELDS_FILE="$BRANCH_DELETE_PROOF_ROOT/$BRANCH_PROOF_ID-$MODE-classification.txt"
  ACTION_FIELDS_FILE="$BRANCH_DELETE_PROOF_ROOT/$BRANCH_PROOF_ID-$MODE-action.txt"
  FETCH_EVIDENCE="$BRANCH_DELETE_PROOF_ROOT/$BRANCH_PROOF_ID-$MODE-fetch.txt"
  PR_VIEW_EVIDENCE="$BRANCH_DELETE_PROOF_ROOT/$BRANCH_PROOF_ID-$MODE-pr-view.json"
  PR_VIEW_ERR="$BRANCH_DELETE_PROOF_ROOT/$BRANCH_PROOF_ID-$MODE-pr-view.err"
  if ! parse_classification_row "$WORKSTREAM_ID" > "$CLASSIFICATION_FIELDS_FILE"; then
    printf 'Blocked: live branch ref tip differs from the preserved head, and the classification row could not be parsed for fresh adopted-PR proof.\n' \
      >> "$PROOF_FILE"
    rm -f "$CLASSIFICATION_FIELDS_FILE" "$ACTION_FIELDS_FILE" "$FETCH_EVIDENCE" "$PR_VIEW_EVIDENCE" "$PR_VIEW_ERR"
    return 1
  fi
  CLASSIFICATION_ROW=()
  CLASSIFICATION_FIELD=
  while IFS= read -r CLASSIFICATION_FIELD || [ -n "$CLASSIFICATION_FIELD" ]; do
    CLASSIFICATION_ROW+=("$CLASSIFICATION_FIELD")
  done < "$CLASSIFICATION_FIELDS_FILE"
  rm -f "$CLASSIFICATION_FIELDS_FILE"
  if test "${#CLASSIFICATION_ROW[@]}" -ne 4; then
    printf 'Blocked: classification row parsing returned an unexpected field count for adopted-PR proof.\n' >> "$PROOF_FILE"
    rm -f "$ACTION_FIELDS_FILE" "$FETCH_EVIDENCE" "$PR_VIEW_EVIDENCE" "$PR_VIEW_ERR"
    return 1
  fi
  CLASSIFICATION_LABEL="${CLASSIFICATION_ROW[0]}"
  MAIN_PR_EVIDENCE="${CLASSIFICATION_ROW[1]}"
  RECOVERY_ACTION="${CLASSIFICATION_ROW[3]}"
  case "$MAIN_PR_EVIDENCE" in
    https://github.com/OpenCoven/coven/pull/*)
      ;;
    *)
      printf 'Blocked: live branch ref tip differs from the preserved head and the exact row does not record a raw OpenCoven/coven PR URL.\n' >> "$PROOF_FILE"
      rm -f "$ACTION_FIELDS_FILE" "$FETCH_EVIDENCE" "$PR_VIEW_EVIDENCE" "$PR_VIEW_ERR"
      return 1
      ;;
  esac
  if [ "$CLASSIFICATION_LABEL" != "viable" ]; then
    printf 'Blocked: live branch ref tip differs from the preserved head and only viable adopted source-branch rows may use the advanced remote proof.\n' >> "$PROOF_FILE"
    rm -f "$ACTION_FIELDS_FILE" "$FETCH_EVIDENCE" "$PR_VIEW_EVIDENCE" "$PR_VIEW_ERR"
    return 1
  fi
  if ! parse_recovery_action "$RECOVERY_ACTION" > "$ACTION_FIELDS_FILE"; then
    printf 'Blocked: viable row recovery action could not be parsed for adopted source-branch proof.\n' >> "$PROOF_FILE"
    rm -f "$ACTION_FIELDS_FILE" "$FETCH_EVIDENCE" "$PR_VIEW_EVIDENCE" "$PR_VIEW_ERR"
    return 1
  fi
  ACTION_ROW=()
  ACTION_FIELD=
  while IFS= read -r ACTION_FIELD || [ -n "$ACTION_FIELD" ]; do
    ACTION_ROW+=("$ACTION_FIELD")
  done < "$ACTION_FIELDS_FILE"
  rm -f "$ACTION_FIELDS_FILE"
  if test "${#ACTION_ROW[@]}" -ne 7; then
    printf 'Blocked: viable row recovery action parsing returned an unexpected field count.\n' >> "$PROOF_FILE"
    rm -f "$FETCH_EVIDENCE" "$PR_VIEW_EVIDENCE" "$PR_VIEW_ERR"
    return 1
  fi
  ACTION_MODE="${ACTION_ROW[0]}"
  ACTION_PR_KIND="${ACTION_ROW[1]}"
  ACTION_EXPECTED_BRANCH="${ACTION_ROW[4]}"
  ACTION_PRESERVED_HEAD="${ACTION_ROW[6]}"
  if [ "$ACTION_MODE" != "continue-existing-pr" ] || [ "$ACTION_PR_KIND" != "adopted" ]; then
    printf 'Blocked: live branch ref tip differs from the preserved head and the row is not a viable adopted source-branch PR.\n' >> "$PROOF_FILE"
    rm -f "$FETCH_EVIDENCE" "$PR_VIEW_EVIDENCE" "$PR_VIEW_ERR"
    return 1
  fi
  if [ "$ACTION_EXPECTED_BRANCH" != "$BRANCH_TO_DELETE" ]; then
    printf 'Blocked: adopted source-branch proof expected branch %s, not %s.\n' "$ACTION_EXPECTED_BRANCH" "$BRANCH_TO_DELETE" >> "$PROOF_FILE"
    rm -f "$FETCH_EVIDENCE" "$PR_VIEW_EVIDENCE" "$PR_VIEW_ERR"
    return 1
  fi
  if [ "$ACTION_PRESERVED_HEAD" != "$PRESERVED_HEAD" ]; then
    printf 'Blocked: adopted source-branch proof preserved head %s does not match snapshot head %s.\n' "$ACTION_PRESERVED_HEAD" "$PRESERVED_HEAD" >> "$PROOF_FILE"
    rm -f "$FETCH_EVIDENCE" "$PR_VIEW_EVIDENCE" "$PR_VIEW_ERR"
    return 1
  fi
  {
    printf 'Advanced adopted-source-branch proof for %s\n' "$BRANCH_TO_DELETE"
    printf 'Fetching origin/%s immediately before deletion proof.\n' "$BRANCH_TO_DELETE"
  } > "$FETCH_EVIDENCE"
  if ! git -C "$REPO" fetch --no-tags origin \
    "refs/heads/$BRANCH_TO_DELETE:refs/remotes/origin/$BRANCH_TO_DELETE" \
    >> "$FETCH_EVIDENCE" 2>&1
  then
    printf 'Blocked: could not fetch origin/%s for advanced adopted-source-branch proof.\n' "$BRANCH_TO_DELETE" >> "$PROOF_FILE"
    cat "$FETCH_EVIDENCE" >> "$PROOF_FILE"
    rm -f "$PR_VIEW_EVIDENCE" "$PR_VIEW_ERR"
    return 1
  fi
  REMOTE_HEAD="$(git -C "$REPO" rev-parse "refs/remotes/origin/$BRANCH_TO_DELETE")"
  {
    printf 'Fetched remote tip: %s\n' "$REMOTE_HEAD"
    printf 'Live branch ref tip: %s\n' "$LIVE_HEAD"
  } >> "$FETCH_EVIDENCE"
  if [ "$LIVE_HEAD" != "$REMOTE_HEAD" ]; then
    printf 'Blocked: live branch ref tip differs from freshly fetched origin/%s, so advanced commits are not yet proven remote-backed.\n' "$BRANCH_TO_DELETE" >> "$PROOF_FILE"
    cat "$FETCH_EVIDENCE" >> "$PROOF_FILE"
    rm -f "$PR_VIEW_EVIDENCE" "$PR_VIEW_ERR"
    return 1
  fi
  if ! gh pr view --repo OpenCoven/coven "$MAIN_PR_EVIDENCE" \
    --json url,state,headRefOid,headRefName,headRepositoryOwner,isCrossRepository,baseRefName \
    > "$PR_VIEW_EVIDENCE" 2> "$PR_VIEW_ERR"
  then
    printf 'Blocked: adopted PR %s could not be freshly verified for advanced source-branch deletion proof.\n' "$MAIN_PR_EVIDENCE" >> "$PROOF_FILE"
    cat "$PR_VIEW_ERR" >> "$PROOF_FILE"
    return 1
  fi
  ACTUAL_URL="$(jq -r '.url' "$PR_VIEW_EVIDENCE")"
  ACTUAL_STATE="$(jq -r '.state' "$PR_VIEW_EVIDENCE")"
  ACTUAL_HEAD="$(jq -r '.headRefOid' "$PR_VIEW_EVIDENCE")"
  ACTUAL_BRANCH="$(jq -r '.headRefName' "$PR_VIEW_EVIDENCE")"
  ACTUAL_OWNER="$(jq -r '.headRepositoryOwner.login' "$PR_VIEW_EVIDENCE")"
  ACTUAL_CROSS="$(jq -r '.isCrossRepository' "$PR_VIEW_EVIDENCE")"
  ACTUAL_BASE="$(jq -r '.baseRefName' "$PR_VIEW_EVIDENCE")"
  {
    printf 'Verified PR URL: %s\n' "$ACTUAL_URL"
    printf 'Verified PR state: %s\n' "$ACTUAL_STATE"
    printf 'Verified PR head branch: %s\n' "$ACTUAL_BRANCH"
    printf 'Verified PR head tip: %s\n' "$ACTUAL_HEAD"
    printf 'Verified PR base: %s\n' "$ACTUAL_BASE"
    printf 'Verified PR owner: %s\n' "$ACTUAL_OWNER"
    printf 'Verified PR cross-repository: %s\n' "$ACTUAL_CROSS"
  } >> "$PROOF_FILE"
  if [ "$ACTUAL_URL" != "$MAIN_PR_EVIDENCE" ] || \
     [ "$ACTUAL_STATE" != "OPEN" ] || \
     [ "$ACTUAL_BASE" != "main" ] || \
     [ "$ACTUAL_OWNER" != "OpenCoven" ] || \
     [ "$ACTUAL_CROSS" != "false" ] || \
     [ "$ACTUAL_BRANCH" != "$BRANCH_TO_DELETE" ] || \
     [ "$ACTUAL_HEAD" != "$REMOTE_HEAD" ]; then
    printf 'Blocked: adopted PR proof must verify one OPEN same-repo main PR whose head branch and headRefOid match the fetched live branch tip.\n' >> "$PROOF_FILE"
    return 1
  fi
  if ! git -C "$REPO" merge-base --is-ancestor "$PRESERVED_HEAD" "$LIVE_HEAD"; then
    printf 'Blocked: preserved snapshot head is not an ancestor of the live adopted branch tip.\n' >> "$PROOF_FILE"
    return 1
  fi
  printf 'Advanced adopted-source-branch proof succeeded: newer commits are preserved remotely and in the OPEN PR.\n' >> "$PROOF_FILE"
  return 0
}
if recheck_branch_ref_tip pre-delete-D; then
  :
else
  CHECK_STATUS=$?
  case "$CHECK_STATUS" in
    2)
      exit 0
      ;;
    *)
      exit "$CHECK_STATUS"
      ;;
  esac
fi
git -C "$REPO" branch -D "$BRANCH_TO_DELETE"
```

If the local branch ref is already missing, record that outcome in the private
proof file and succeed without failing the step. Exact preserved-head equality
remains the default deletion proof. If the live branch ref tip differs from the
preserved head, allow deletion only for a viable
`mode=continue-existing-pr; pr_kind=adopted` source-branch row after fresh
proof that the branch being deleted is the exact expected branch, the live
local tip equals the freshly fetched `origin/<branch>` tip, the current PR is
still the OPEN same-repo `main` PR at that branch/tip, and the preserved
snapshot head is an ancestor of the live tip. Otherwise stop and do not use
`-D`; newer commits are not yet proven preserved remotely.

- [ ] **Step 6: Release only merged or stopped recovery claims**

For any child recovery workstream whose PR has merged or whose owning recovery
session has stopped, release its claim from that child worktree:

```bash
set -euo pipefail
CURRENT_BRANCH="$(git branch --show-current)"
case "$CURRENT_BRANCH" in
  issue-[0-9]*-*)
    ISSUE_NUMBER="${CURRENT_BRANCH#issue-}"
    ISSUE_NUMBER="${ISSUE_NUMBER%%-*}"
    ;;
  *)
    printf 'Blocked: current branch %s does not match issue-<number>-<slug>.\n' "$CURRENT_BRANCH" >&2
    exit 1
    ;;
esac
coven claim release "issue-$ISSUE_NUMBER"
```

Keep child claims active while follow-up work for an open PR continues. During
long-running follow-up from that worktree, use:

```bash
set -euo pipefail
CURRENT_BRANCH="$(git branch --show-current)"
case "$CURRENT_BRANCH" in
  issue-[0-9]*-*)
    ISSUE_NUMBER="${CURRENT_BRANCH#issue-}"
    ISSUE_NUMBER="${ISSUE_NUMBER%%-*}"
    ;;
  *)
    printf 'Blocked: current branch %s does not match issue-<number>-<slug>.\n' "$CURRENT_BRANCH" >&2
    exit 1
    ;;
esac
coven claim heartbeat "issue-$ISSUE_NUMBER"
```

Release the parent `issue-541` claim only when the issue #541 recovery session
stops or the full issue #541 recovery effort is complete:

```bash
set -euo pipefail
resolve_control_worktree() {
  local control_branch="docs/541-incomplete-work-recovery-design"
  local expected_branch="refs/heads/$control_branch"
  local remote_branch_ref="refs/remotes/origin/$control_branch"
  local start_common_dir repo target_path path branch_ref actual_branch
  local target_registration_branch live_count stale_count live_path stale_path
  local live_status live_head local_head remote_head
  local -a live_paths=() stale_paths=()
  start_common_dir="$(git rev-parse --git-common-dir)"
  repo="$(cd "$start_common_dir/.." && pwd)"
  target_path="$repo/.worktrees/issue-541-recovery"
  if ! git -C "$repo" fetch --no-tags origin \
    "refs/heads/$control_branch:$remote_branch_ref"
  then
    printf 'Blocked: could not fetch origin/%s into %s.\n' \
      "$control_branch" "$remote_branch_ref" >&2
    exit 1
  fi
  remote_head="$(git -C "$repo" rev-parse --verify "$remote_branch_ref^{commit}")"
  while IFS="$(printf "\t")" read -r path branch_ref; do
    test -n "$path" || continue
    if test "$path" = "$target_path"; then
      target_registration_branch="$branch_ref"
    fi
    if test "$branch_ref" != "$expected_branch"; then
      continue
    fi
    if test -d "$path"; then
      live_paths+=("$path")
    else
      stale_paths+=("$path")
    fi
  done <<EOF2
$(git -C "$repo" worktree list --porcelain | awk '
  $1 == "worktree" { if (path != "") print path "\t" branch; path = substr($0, 10); branch = ""; next }
  $1 == "branch" { branch = $2; next }
  END { if (path != "") print path "\t" branch }
')
EOF2
  live_count="${#live_paths[@]}"
  stale_count="${#stale_paths[@]}"
  if test -n "$target_registration_branch" && test "$target_registration_branch" != "$expected_branch"; then
    printf 'Blocked: %s is registered to %s, not %s.\n' "$target_path" "$target_registration_branch" "$control_branch" >&2
    exit 1
  fi
  if test "$live_count" -gt 1; then
    printf 'Blocked: %s has %s live controller worktree registrations; refusing to choose arbitrarily.\n' "$control_branch" "$live_count" >&2
    for live_path in "${live_paths[@]}"; do
      printf '  live: %s\n' "$live_path" >&2
    done
    for stale_path in "${stale_paths[@]}"; do
      printf '  stale: %s\n' "$stale_path" >&2
    done
    exit 1
  fi
  if test "$live_count" -eq 1; then
    live_path="${live_paths[0]}"
    if test "$stale_count" -gt 0; then
      printf 'Warning: ignoring %s stale controller registration(s) for %s while using the one live worktree.\n' "$stale_count" "$control_branch" >&2
      for stale_path in "${stale_paths[@]}"; do
        printf '  stale: %s\n' "$stale_path" >&2
      done
    fi
    actual_branch="$(git -C "$live_path" branch --show-current)"
    if test "$actual_branch" != "$control_branch"; then
      printf 'Control worktree branch mismatch: expected %s, got %s\n' "$control_branch" "$actual_branch" >&2
      exit 1
    fi
    live_status="$(git -C "$live_path" status --porcelain=v1 --untracked-files=all)"
    if test -n "$live_status"; then
      printf 'Blocked: live controller worktree is dirty and cannot be refreshed safely: %s\n' "$live_path" >&2
      git -C "$live_path" status --short --branch --untracked-files=all >&2
      exit 1
    fi
    live_head="$(git -C "$live_path" rev-parse HEAD)"
    if test "$live_head" = "$remote_head"; then
      :
    elif git -C "$repo" merge-base --is-ancestor "$live_head" "$remote_head"; then
      if ! git -C "$live_path" merge --ff-only "$remote_branch_ref"; then
        printf 'Blocked: live controller worktree could not fast-forward %s to fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
        exit 1
      fi
    elif git -C "$repo" merge-base --is-ancestor "$remote_head" "$live_head"; then
      printf 'Blocked: live controller branch %s is ahead of fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
      exit 1
    else
      printf 'Blocked: live controller branch %s diverged from fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
      exit 1
    fi
    live_head="$(git -C "$live_path" rev-parse HEAD)"
    if test "$live_head" != "$remote_head"; then
      printf 'Blocked: live controller HEAD %s does not match fetched origin tip %s.\n' "$live_head" "$remote_head" >&2
      exit 1
    fi
    printf '%s\n' "$live_path"
    return 0
  fi
  if test "$stale_count" -gt 1; then
    printf 'Blocked: %s has %s stale controller registrations and no live worktree; refusing to choose arbitrarily.\n' "$control_branch" "$stale_count" >&2
    for stale_path in "${stale_paths[@]}"; do
      printf '  stale: %s\n' "$stale_path" >&2
    done
    exit 1
  fi
  if ! git -C "$repo" check-ignore -q .worktrees/; then
    printf 'Repository-local .worktrees/ is not ignored.\n' >&2
    exit 1
  fi
  if ! git -C "$repo" check-ignore -q .worktrees/issue-541-recovery; then
    printf 'Repository-local control worktree path is not ignored: %s\n' '.worktrees/issue-541-recovery' >&2
    exit 1
  fi
  mkdir -p "$repo/.worktrees"
  if test -e "$target_path"; then
    printf 'Control worktree path already exists without a matching live registration: %s\n' "$target_path" >&2
    exit 1
  fi
  if git -C "$repo" show-ref --verify --quiet "$expected_branch"; then
    local_head="$(git -C "$repo" rev-parse --verify "$expected_branch^{commit}")"
    if test "$local_head" = "$remote_head"; then
      :
    elif git -C "$repo" merge-base --is-ancestor "$local_head" "$remote_head"; then
      if ! git -C "$repo" update-ref "$expected_branch" "$remote_head" "$local_head"; then
        printf 'Blocked: local controller branch %s could not be fast-forwarded to fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
        exit 1
      fi
    elif git -C "$repo" merge-base --is-ancestor "$remote_head" "$local_head"; then
      printf 'Blocked: local controller branch %s is ahead of fetched origin tip %s and cannot be rewritten safely.\n' "$control_branch" "$remote_head" >&2
      exit 1
    else
      printf 'Blocked: local controller branch %s diverged from fetched origin tip %s and cannot be rewritten safely.\n' "$control_branch" "$remote_head" >&2
      exit 1
    fi
  else
    if ! git -C "$repo" update-ref "$expected_branch" "$remote_head"; then
      printf 'Blocked: local controller branch %s could not be created at fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
      exit 1
    fi
  fi
  local_head="$(git -C "$repo" rev-parse --verify "$expected_branch^{commit}")"
  if test "$local_head" != "$remote_head"; then
    printf 'Blocked: local controller branch %s resolved to %s instead of fetched origin tip %s.\n' "$control_branch" "$local_head" "$remote_head" >&2
    exit 1
  fi
  if test "$stale_count" -eq 1; then
    stale_path="${stale_paths[0]}"
    printf 'Recreating %s at %s from the one stale controller registration:\n  stale: %s\n' "$control_branch" "$target_path" "$stale_path" >&2
    git -C "$repo" worktree add --force "$target_path" "$control_branch"
  else
    git -C "$repo" worktree add "$target_path" "$control_branch"
  fi
  if ! test -d "$target_path"; then
    printf 'Control worktree path is not present after resolution: %s\n' "$target_path" >&2
    exit 1
  fi
  actual_branch="$(git -C "$target_path" branch --show-current)"
  if test "$actual_branch" != "$control_branch"; then
    printf 'Control worktree branch mismatch: expected %s, got %s\n' "$control_branch" "$actual_branch" >&2
    exit 1
  fi
  local_head="$(git -C "$target_path" rev-parse HEAD)"
  if test "$local_head" != "$remote_head"; then
    printf 'Blocked: resolved controller HEAD %s does not match fetched origin tip %s.\n' "$local_head" "$remote_head" >&2
    exit 1
  fi
  printf '%s\n' "$target_path"
}
CONTROL_WORKTREE="$(resolve_control_worktree)"
COMMON_DIR="$(git -C "$CONTROL_WORKTREE" rev-parse --git-common-dir)"
REPO="$(cd "$COMMON_DIR/.." && pwd)"
cd "$CONTROL_WORKTREE"
coven claim release issue-541
```

Expected: `coven claim status` shows no abandoned claim; active claims remain
allowed only for still-running recovery sessions and open follow-up work.

### Task 8: Reconcile Repository Goals

**Files:**
- Modify: authoritative ignored `$REPO/.copilot/goals.md` in the primary
  checkout only. Ignored per-worktree copies are not synchronized and are not
  a source of truth.

- [ ] **Step 1: Re-read goals and live issue state**

Run:

```bash
set -euo pipefail
resolve_control_worktree() {
  local control_branch="docs/541-incomplete-work-recovery-design"
  local expected_branch="refs/heads/$control_branch"
  local remote_branch_ref="refs/remotes/origin/$control_branch"
  local start_common_dir repo target_path path branch_ref actual_branch
  local target_registration_branch live_count stale_count live_path stale_path
  local live_status live_head local_head remote_head
  local -a live_paths=() stale_paths=()
  start_common_dir="$(git rev-parse --git-common-dir)"
  repo="$(cd "$start_common_dir/.." && pwd)"
  target_path="$repo/.worktrees/issue-541-recovery"
  if ! git -C "$repo" fetch --no-tags origin \
    "refs/heads/$control_branch:$remote_branch_ref"
  then
    printf 'Blocked: could not fetch origin/%s into %s.\n' \
      "$control_branch" "$remote_branch_ref" >&2
    exit 1
  fi
  remote_head="$(git -C "$repo" rev-parse --verify "$remote_branch_ref^{commit}")"
  while IFS="$(printf "\t")" read -r path branch_ref; do
    test -n "$path" || continue
    if test "$path" = "$target_path"; then
      target_registration_branch="$branch_ref"
    fi
    if test "$branch_ref" != "$expected_branch"; then
      continue
    fi
    if test -d "$path"; then
      live_paths+=("$path")
    else
      stale_paths+=("$path")
    fi
  done <<EOF2
$(git -C "$repo" worktree list --porcelain | awk '
  $1 == "worktree" { if (path != "") print path "\t" branch; path = substr($0, 10); branch = ""; next }
  $1 == "branch" { branch = $2; next }
  END { if (path != "") print path "\t" branch }
')
EOF2
  live_count="${#live_paths[@]}"
  stale_count="${#stale_paths[@]}"
  if test -n "$target_registration_branch" && test "$target_registration_branch" != "$expected_branch"; then
    printf 'Blocked: %s is registered to %s, not %s.\n' "$target_path" "$target_registration_branch" "$control_branch" >&2
    exit 1
  fi
  if test "$live_count" -gt 1; then
    printf 'Blocked: %s has %s live controller worktree registrations; refusing to choose arbitrarily.\n' "$control_branch" "$live_count" >&2
    for live_path in "${live_paths[@]}"; do
      printf '  live: %s\n' "$live_path" >&2
    done
    for stale_path in "${stale_paths[@]}"; do
      printf '  stale: %s\n' "$stale_path" >&2
    done
    exit 1
  fi
  if test "$live_count" -eq 1; then
    live_path="${live_paths[0]}"
    if test "$stale_count" -gt 0; then
      printf 'Warning: ignoring %s stale controller registration(s) for %s while using the one live worktree.\n' "$stale_count" "$control_branch" >&2
      for stale_path in "${stale_paths[@]}"; do
        printf '  stale: %s\n' "$stale_path" >&2
      done
    fi
    actual_branch="$(git -C "$live_path" branch --show-current)"
    if test "$actual_branch" != "$control_branch"; then
      printf 'Control worktree branch mismatch: expected %s, got %s\n' "$control_branch" "$actual_branch" >&2
      exit 1
    fi
    live_status="$(git -C "$live_path" status --porcelain=v1 --untracked-files=all)"
    if test -n "$live_status"; then
      printf 'Blocked: live controller worktree is dirty and cannot be refreshed safely: %s\n' "$live_path" >&2
      git -C "$live_path" status --short --branch --untracked-files=all >&2
      exit 1
    fi
    live_head="$(git -C "$live_path" rev-parse HEAD)"
    if test "$live_head" = "$remote_head"; then
      :
    elif git -C "$repo" merge-base --is-ancestor "$live_head" "$remote_head"; then
      if ! git -C "$live_path" merge --ff-only "$remote_branch_ref"; then
        printf 'Blocked: live controller worktree could not fast-forward %s to fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
        exit 1
      fi
    elif git -C "$repo" merge-base --is-ancestor "$remote_head" "$live_head"; then
      printf 'Blocked: live controller branch %s is ahead of fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
      exit 1
    else
      printf 'Blocked: live controller branch %s diverged from fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
      exit 1
    fi
    live_head="$(git -C "$live_path" rev-parse HEAD)"
    if test "$live_head" != "$remote_head"; then
      printf 'Blocked: live controller HEAD %s does not match fetched origin tip %s.\n' "$live_head" "$remote_head" >&2
      exit 1
    fi
    printf '%s\n' "$live_path"
    return 0
  fi
  if test "$stale_count" -gt 1; then
    printf 'Blocked: %s has %s stale controller registrations and no live worktree; refusing to choose arbitrarily.\n' "$control_branch" "$stale_count" >&2
    for stale_path in "${stale_paths[@]}"; do
      printf '  stale: %s\n' "$stale_path" >&2
    done
    exit 1
  fi
  if ! git -C "$repo" check-ignore -q .worktrees/; then
    printf 'Repository-local .worktrees/ is not ignored.\n' >&2
    exit 1
  fi
  if ! git -C "$repo" check-ignore -q .worktrees/issue-541-recovery; then
    printf 'Repository-local control worktree path is not ignored: %s\n' '.worktrees/issue-541-recovery' >&2
    exit 1
  fi
  mkdir -p "$repo/.worktrees"
  if test -e "$target_path"; then
    printf 'Control worktree path already exists without a matching live registration: %s\n' "$target_path" >&2
    exit 1
  fi
  if git -C "$repo" show-ref --verify --quiet "$expected_branch"; then
    local_head="$(git -C "$repo" rev-parse --verify "$expected_branch^{commit}")"
    if test "$local_head" = "$remote_head"; then
      :
    elif git -C "$repo" merge-base --is-ancestor "$local_head" "$remote_head"; then
      if ! git -C "$repo" update-ref "$expected_branch" "$remote_head" "$local_head"; then
        printf 'Blocked: local controller branch %s could not be fast-forwarded to fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
        exit 1
      fi
    elif git -C "$repo" merge-base --is-ancestor "$remote_head" "$local_head"; then
      printf 'Blocked: local controller branch %s is ahead of fetched origin tip %s and cannot be rewritten safely.\n' "$control_branch" "$remote_head" >&2
      exit 1
    else
      printf 'Blocked: local controller branch %s diverged from fetched origin tip %s and cannot be rewritten safely.\n' "$control_branch" "$remote_head" >&2
      exit 1
    fi
  else
    if ! git -C "$repo" update-ref "$expected_branch" "$remote_head"; then
      printf 'Blocked: local controller branch %s could not be created at fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
      exit 1
    fi
  fi
  local_head="$(git -C "$repo" rev-parse --verify "$expected_branch^{commit}")"
  if test "$local_head" != "$remote_head"; then
    printf 'Blocked: local controller branch %s resolved to %s instead of fetched origin tip %s.\n' "$control_branch" "$local_head" "$remote_head" >&2
    exit 1
  fi
  if test "$stale_count" -eq 1; then
    stale_path="${stale_paths[0]}"
    printf 'Recreating %s at %s from the one stale controller registration:\n  stale: %s\n' "$control_branch" "$target_path" "$stale_path" >&2
    git -C "$repo" worktree add --force "$target_path" "$control_branch"
  else
    git -C "$repo" worktree add "$target_path" "$control_branch"
  fi
  if ! test -d "$target_path"; then
    printf 'Control worktree path is not present after resolution: %s\n' "$target_path" >&2
    exit 1
  fi
  actual_branch="$(git -C "$target_path" branch --show-current)"
  if test "$actual_branch" != "$control_branch"; then
    printf 'Control worktree branch mismatch: expected %s, got %s\n' "$control_branch" "$actual_branch" >&2
    exit 1
  fi
  local_head="$(git -C "$target_path" rev-parse HEAD)"
  if test "$local_head" != "$remote_head"; then
    printf 'Blocked: resolved controller HEAD %s does not match fetched origin tip %s.\n' "$local_head" "$remote_head" >&2
    exit 1
  fi
  printf '%s\n' "$target_path"
}
CONTROL_WORKTREE="$(resolve_control_worktree)"
COMMON_DIR="$(git -C "$CONTROL_WORKTREE" rev-parse --git-common-dir)"
REPO="$(cd "$COMMON_DIR/.." && pwd)"
GOALS_FILE="$REPO/.copilot/goals.md"
cd "$REPO"
if ! test -f "$GOALS_FILE"; then
  printf 'Blocked: authoritative goals file is missing: %s\n' "$GOALS_FILE" >&2
  ls -ld "$REPO" "$REPO/.copilot" "$GOALS_FILE" 2>&1 || true
  exit 1
fi
sed -n '1,260p' "$GOALS_FILE"
for issue in 401 414 521 541; do
  gh issue view "$issue" --json number,state,title,closedAt,url
done
```

Expected: the authoritative primary-checkout goals file is readable at
`$GOALS_FILE`; #401, #414, and #521 are closed; #541 reflects the recovery PR
state; and ignored per-worktree goals copies remain out of scope because they
are not synchronized.

- [ ] **Step 2: Close stale active goal content**

Move `usability-core-consolidation` to `done` in the authoritative ignored
primary-checkout goals file at `$REPO/.copilot/goals.md` because ignored
per-worktree copies are not synchronized and its named high-risk follow-up #401
is closed. Set:

```markdown
- completed: 2026-08-01
- outcome: |
    The five top gaps and the session-launch consolidation tracked by #401 are
    closed. Remaining translation drift is not part of this completed
    consolidation goal and requires a separately claimed issue if resumed.
```

Remove obsolete `next` text that presents #401 as future work.

- [ ] **Step 3: Reconcile contribution stewardship**

Keep `contribution-stewardship` active in the authoritative ignored
primary-checkout goals file at `$REPO/.copilot/goals.md` because ignored
per-worktree copies are not synchronized and it is an ongoing maintenance
objective. Append a 2026-08-01 checkpoint that records:

- #414 and #521 are closed;
- there were no open PRs at recovery start;
- issue #541 owns local recovery;
- viable workstream PR URLs from the classification ledger;
- already-shipped, superseded, and blocked outcomes.

Set its single `next` action to review the open recovery PRs and perform the
next external-PR sweep. Remove the duplicated stale `next` line.

- [ ] **Step 4: Verify goals structure**

Run:

```bash
set -euo pipefail
resolve_control_worktree() {
  local control_branch="docs/541-incomplete-work-recovery-design"
  local expected_branch="refs/heads/$control_branch"
  local remote_branch_ref="refs/remotes/origin/$control_branch"
  local start_common_dir repo target_path path branch_ref actual_branch
  local target_registration_branch live_count stale_count live_path stale_path
  local live_status live_head local_head remote_head
  local -a live_paths=() stale_paths=()
  start_common_dir="$(git rev-parse --git-common-dir)"
  repo="$(cd "$start_common_dir/.." && pwd)"
  target_path="$repo/.worktrees/issue-541-recovery"
  if ! git -C "$repo" fetch --no-tags origin \
    "refs/heads/$control_branch:$remote_branch_ref"
  then
    printf 'Blocked: could not fetch origin/%s into %s.\n' \
      "$control_branch" "$remote_branch_ref" >&2
    exit 1
  fi
  remote_head="$(git -C "$repo" rev-parse --verify "$remote_branch_ref^{commit}")"
  while IFS="$(printf "\t")" read -r path branch_ref; do
    test -n "$path" || continue
    if test "$path" = "$target_path"; then
      target_registration_branch="$branch_ref"
    fi
    if test "$branch_ref" != "$expected_branch"; then
      continue
    fi
    if test -d "$path"; then
      live_paths+=("$path")
    else
      stale_paths+=("$path")
    fi
  done <<EOF2
$(git -C "$repo" worktree list --porcelain | awk '
  $1 == "worktree" { if (path != "") print path "\t" branch; path = substr($0, 10); branch = ""; next }
  $1 == "branch" { branch = $2; next }
  END { if (path != "") print path "\t" branch }
')
EOF2
  live_count="${#live_paths[@]}"
  stale_count="${#stale_paths[@]}"
  if test -n "$target_registration_branch" && test "$target_registration_branch" != "$expected_branch"; then
    printf 'Blocked: %s is registered to %s, not %s.\n' "$target_path" "$target_registration_branch" "$control_branch" >&2
    exit 1
  fi
  if test "$live_count" -gt 1; then
    printf 'Blocked: %s has %s live controller worktree registrations; refusing to choose arbitrarily.\n' "$control_branch" "$live_count" >&2
    for live_path in "${live_paths[@]}"; do
      printf '  live: %s\n' "$live_path" >&2
    done
    for stale_path in "${stale_paths[@]}"; do
      printf '  stale: %s\n' "$stale_path" >&2
    done
    exit 1
  fi
  if test "$live_count" -eq 1; then
    live_path="${live_paths[0]}"
    if test "$stale_count" -gt 0; then
      printf 'Warning: ignoring %s stale controller registration(s) for %s while using the one live worktree.\n' "$stale_count" "$control_branch" >&2
      for stale_path in "${stale_paths[@]}"; do
        printf '  stale: %s\n' "$stale_path" >&2
      done
    fi
    actual_branch="$(git -C "$live_path" branch --show-current)"
    if test "$actual_branch" != "$control_branch"; then
      printf 'Control worktree branch mismatch: expected %s, got %s\n' "$control_branch" "$actual_branch" >&2
      exit 1
    fi
    live_status="$(git -C "$live_path" status --porcelain=v1 --untracked-files=all)"
    if test -n "$live_status"; then
      printf 'Blocked: live controller worktree is dirty and cannot be refreshed safely: %s\n' "$live_path" >&2
      git -C "$live_path" status --short --branch --untracked-files=all >&2
      exit 1
    fi
    live_head="$(git -C "$live_path" rev-parse HEAD)"
    if test "$live_head" = "$remote_head"; then
      :
    elif git -C "$repo" merge-base --is-ancestor "$live_head" "$remote_head"; then
      if ! git -C "$live_path" merge --ff-only "$remote_branch_ref"; then
        printf 'Blocked: live controller worktree could not fast-forward %s to fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
        exit 1
      fi
    elif git -C "$repo" merge-base --is-ancestor "$remote_head" "$live_head"; then
      printf 'Blocked: live controller branch %s is ahead of fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
      exit 1
    else
      printf 'Blocked: live controller branch %s diverged from fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
      exit 1
    fi
    live_head="$(git -C "$live_path" rev-parse HEAD)"
    if test "$live_head" != "$remote_head"; then
      printf 'Blocked: live controller HEAD %s does not match fetched origin tip %s.\n' "$live_head" "$remote_head" >&2
      exit 1
    fi
    printf '%s\n' "$live_path"
    return 0
  fi
  if test "$stale_count" -gt 1; then
    printf 'Blocked: %s has %s stale controller registrations and no live worktree; refusing to choose arbitrarily.\n' "$control_branch" "$stale_count" >&2
    for stale_path in "${stale_paths[@]}"; do
      printf '  stale: %s\n' "$stale_path" >&2
    done
    exit 1
  fi
  if ! git -C "$repo" check-ignore -q .worktrees/; then
    printf 'Repository-local .worktrees/ is not ignored.\n' >&2
    exit 1
  fi
  if ! git -C "$repo" check-ignore -q .worktrees/issue-541-recovery; then
    printf 'Repository-local control worktree path is not ignored: %s\n' '.worktrees/issue-541-recovery' >&2
    exit 1
  fi
  mkdir -p "$repo/.worktrees"
  if test -e "$target_path"; then
    printf 'Control worktree path already exists without a matching live registration: %s\n' "$target_path" >&2
    exit 1
  fi
  if git -C "$repo" show-ref --verify --quiet "$expected_branch"; then
    local_head="$(git -C "$repo" rev-parse --verify "$expected_branch^{commit}")"
    if test "$local_head" = "$remote_head"; then
      :
    elif git -C "$repo" merge-base --is-ancestor "$local_head" "$remote_head"; then
      if ! git -C "$repo" update-ref "$expected_branch" "$remote_head" "$local_head"; then
        printf 'Blocked: local controller branch %s could not be fast-forwarded to fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
        exit 1
      fi
    elif git -C "$repo" merge-base --is-ancestor "$remote_head" "$local_head"; then
      printf 'Blocked: local controller branch %s is ahead of fetched origin tip %s and cannot be rewritten safely.\n' "$control_branch" "$remote_head" >&2
      exit 1
    else
      printf 'Blocked: local controller branch %s diverged from fetched origin tip %s and cannot be rewritten safely.\n' "$control_branch" "$remote_head" >&2
      exit 1
    fi
  else
    if ! git -C "$repo" update-ref "$expected_branch" "$remote_head"; then
      printf 'Blocked: local controller branch %s could not be created at fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
      exit 1
    fi
  fi
  local_head="$(git -C "$repo" rev-parse --verify "$expected_branch^{commit}")"
  if test "$local_head" != "$remote_head"; then
    printf 'Blocked: local controller branch %s resolved to %s instead of fetched origin tip %s.\n' "$control_branch" "$local_head" "$remote_head" >&2
    exit 1
  fi
  if test "$stale_count" -eq 1; then
    stale_path="${stale_paths[0]}"
    printf 'Recreating %s at %s from the one stale controller registration:\n  stale: %s\n' "$control_branch" "$target_path" "$stale_path" >&2
    git -C "$repo" worktree add --force "$target_path" "$control_branch"
  else
    git -C "$repo" worktree add "$target_path" "$control_branch"
  fi
  if ! test -d "$target_path"; then
    printf 'Control worktree path is not present after resolution: %s\n' "$target_path" >&2
    exit 1
  fi
  actual_branch="$(git -C "$target_path" branch --show-current)"
  if test "$actual_branch" != "$control_branch"; then
    printf 'Control worktree branch mismatch: expected %s, got %s\n' "$control_branch" "$actual_branch" >&2
    exit 1
  fi
  local_head="$(git -C "$target_path" rev-parse HEAD)"
  if test "$local_head" != "$remote_head"; then
    printf 'Blocked: resolved controller HEAD %s does not match fetched origin tip %s.\n' "$local_head" "$remote_head" >&2
    exit 1
  fi
  printf '%s\n' "$target_path"
}
CONTROL_WORKTREE="$(resolve_control_worktree)"
COMMON_DIR="$(git -C "$CONTROL_WORKTREE" rev-parse --git-common-dir)"
REPO="$(cd "$COMMON_DIR/.." && pwd)"
GOALS_FILE="$REPO/.copilot/goals.md"
cd "$REPO"
if ! test -f "$GOALS_FILE"; then
  printf 'Blocked: authoritative goals file is missing: %s\n' "$GOALS_FILE" >&2
  ls -ld "$REPO" "$REPO/.copilot" "$GOALS_FILE" 2>&1 || true
  exit 1
fi
grep -n '^## active\|^## paused\|^## done\|^### goal:\|^- next:' \
  "$GOALS_FILE"
```

Expected: the authoritative primary-checkout goals file at `$GOALS_FILE`
contains one `next` field per active goal, completed goals are under `## done`,
and no active `next` references closed issues as future work.

### Task 9: Final Recovery Audit

**Artifacts:**
- Create or modify: `.git/agent-recovery/issue-541/final-audit-primary-checkout.txt`
- Modify: `.git/agent-recovery/issue-541/classification.md`

- [ ] **Step 1: Verify every manifest row has a terminal recovery state**

Run:

```bash
set -euo pipefail
resolve_control_worktree() {
  local control_branch="docs/541-incomplete-work-recovery-design"
  local expected_branch="refs/heads/$control_branch"
  local remote_branch_ref="refs/remotes/origin/$control_branch"
  local start_common_dir repo target_path path branch_ref actual_branch
  local target_registration_branch live_count stale_count live_path stale_path
  local live_status live_head local_head remote_head
  local -a live_paths=() stale_paths=()
  start_common_dir="$(git rev-parse --git-common-dir)"
  repo="$(cd "$start_common_dir/.." && pwd)"
  target_path="$repo/.worktrees/issue-541-recovery"
  if ! git -C "$repo" fetch --no-tags origin \
    "refs/heads/$control_branch:$remote_branch_ref"
  then
    printf 'Blocked: could not fetch origin/%s into %s.\n' \
      "$control_branch" "$remote_branch_ref" >&2
    exit 1
  fi
  remote_head="$(git -C "$repo" rev-parse --verify "$remote_branch_ref^{commit}")"
  while IFS="$(printf "\t")" read -r path branch_ref; do
    test -n "$path" || continue
    if test "$path" = "$target_path"; then
      target_registration_branch="$branch_ref"
    fi
    if test "$branch_ref" != "$expected_branch"; then
      continue
    fi
    if test -d "$path"; then
      live_paths+=("$path")
    else
      stale_paths+=("$path")
    fi
  done <<EOF2
$(git -C "$repo" worktree list --porcelain | awk '
  $1 == "worktree" { if (path != "") print path "\t" branch; path = substr($0, 10); branch = ""; next }
  $1 == "branch" { branch = $2; next }
  END { if (path != "") print path "\t" branch }
')
EOF2
  live_count="${#live_paths[@]}"
  stale_count="${#stale_paths[@]}"
  if test -n "$target_registration_branch" && test "$target_registration_branch" != "$expected_branch"; then
    printf 'Blocked: %s is registered to %s, not %s.\n' "$target_path" "$target_registration_branch" "$control_branch" >&2
    exit 1
  fi
  if test "$live_count" -gt 1; then
    printf 'Blocked: %s has %s live controller worktree registrations; refusing to choose arbitrarily.\n' "$control_branch" "$live_count" >&2
    for live_path in "${live_paths[@]}"; do
      printf '  live: %s\n' "$live_path" >&2
    done
    for stale_path in "${stale_paths[@]}"; do
      printf '  stale: %s\n' "$stale_path" >&2
    done
    exit 1
  fi
  if test "$live_count" -eq 1; then
    live_path="${live_paths[0]}"
    if test "$stale_count" -gt 0; then
      printf 'Warning: ignoring %s stale controller registration(s) for %s while using the one live worktree.\n' "$stale_count" "$control_branch" >&2
      for stale_path in "${stale_paths[@]}"; do
        printf '  stale: %s\n' "$stale_path" >&2
      done
    fi
    actual_branch="$(git -C "$live_path" branch --show-current)"
    if test "$actual_branch" != "$control_branch"; then
      printf 'Control worktree branch mismatch: expected %s, got %s\n' "$control_branch" "$actual_branch" >&2
      exit 1
    fi
    live_status="$(git -C "$live_path" status --porcelain=v1 --untracked-files=all)"
    if test -n "$live_status"; then
      printf 'Blocked: live controller worktree is dirty and cannot be refreshed safely: %s\n' "$live_path" >&2
      git -C "$live_path" status --short --branch --untracked-files=all >&2
      exit 1
    fi
    live_head="$(git -C "$live_path" rev-parse HEAD)"
    if test "$live_head" = "$remote_head"; then
      :
    elif git -C "$repo" merge-base --is-ancestor "$live_head" "$remote_head"; then
      if ! git -C "$live_path" merge --ff-only "$remote_branch_ref"; then
        printf 'Blocked: live controller worktree could not fast-forward %s to fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
        exit 1
      fi
    elif git -C "$repo" merge-base --is-ancestor "$remote_head" "$live_head"; then
      printf 'Blocked: live controller branch %s is ahead of fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
      exit 1
    else
      printf 'Blocked: live controller branch %s diverged from fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
      exit 1
    fi
    live_head="$(git -C "$live_path" rev-parse HEAD)"
    if test "$live_head" != "$remote_head"; then
      printf 'Blocked: live controller HEAD %s does not match fetched origin tip %s.\n' "$live_head" "$remote_head" >&2
      exit 1
    fi
    printf '%s\n' "$live_path"
    return 0
  fi
  if test "$stale_count" -gt 1; then
    printf 'Blocked: %s has %s stale controller registrations and no live worktree; refusing to choose arbitrarily.\n' "$control_branch" "$stale_count" >&2
    for stale_path in "${stale_paths[@]}"; do
      printf '  stale: %s\n' "$stale_path" >&2
    done
    exit 1
  fi
  if ! git -C "$repo" check-ignore -q .worktrees/; then
    printf 'Repository-local .worktrees/ is not ignored.\n' >&2
    exit 1
  fi
  if ! git -C "$repo" check-ignore -q .worktrees/issue-541-recovery; then
    printf 'Repository-local control worktree path is not ignored: %s\n' '.worktrees/issue-541-recovery' >&2
    exit 1
  fi
  mkdir -p "$repo/.worktrees"
  if test -e "$target_path"; then
    printf 'Control worktree path already exists without a matching live registration: %s\n' "$target_path" >&2
    exit 1
  fi
  if git -C "$repo" show-ref --verify --quiet "$expected_branch"; then
    local_head="$(git -C "$repo" rev-parse --verify "$expected_branch^{commit}")"
    if test "$local_head" = "$remote_head"; then
      :
    elif git -C "$repo" merge-base --is-ancestor "$local_head" "$remote_head"; then
      if ! git -C "$repo" update-ref "$expected_branch" "$remote_head" "$local_head"; then
        printf 'Blocked: local controller branch %s could not be fast-forwarded to fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
        exit 1
      fi
    elif git -C "$repo" merge-base --is-ancestor "$remote_head" "$local_head"; then
      printf 'Blocked: local controller branch %s is ahead of fetched origin tip %s and cannot be rewritten safely.\n' "$control_branch" "$remote_head" >&2
      exit 1
    else
      printf 'Blocked: local controller branch %s diverged from fetched origin tip %s and cannot be rewritten safely.\n' "$control_branch" "$remote_head" >&2
      exit 1
    fi
  else
    if ! git -C "$repo" update-ref "$expected_branch" "$remote_head"; then
      printf 'Blocked: local controller branch %s could not be created at fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
      exit 1
    fi
  fi
  local_head="$(git -C "$repo" rev-parse --verify "$expected_branch^{commit}")"
  if test "$local_head" != "$remote_head"; then
    printf 'Blocked: local controller branch %s resolved to %s instead of fetched origin tip %s.\n' "$control_branch" "$local_head" "$remote_head" >&2
    exit 1
  fi
  if test "$stale_count" -eq 1; then
    stale_path="${stale_paths[0]}"
    printf 'Recreating %s at %s from the one stale controller registration:\n  stale: %s\n' "$control_branch" "$target_path" "$stale_path" >&2
    git -C "$repo" worktree add --force "$target_path" "$control_branch"
  else
    git -C "$repo" worktree add "$target_path" "$control_branch"
  fi
  if ! test -d "$target_path"; then
    printf 'Control worktree path is not present after resolution: %s\n' "$target_path" >&2
    exit 1
  fi
  actual_branch="$(git -C "$target_path" branch --show-current)"
  if test "$actual_branch" != "$control_branch"; then
    printf 'Control worktree branch mismatch: expected %s, got %s\n' "$control_branch" "$actual_branch" >&2
    exit 1
  fi
  local_head="$(git -C "$target_path" rev-parse HEAD)"
  if test "$local_head" != "$remote_head"; then
    printf 'Blocked: resolved controller HEAD %s does not match fetched origin tip %s.\n' "$local_head" "$remote_head" >&2
    exit 1
  fi
  printf '%s\n' "$target_path"
}
CONTROL_WORKTREE="$(resolve_control_worktree)"
COMMON_DIR="$(git -C "$CONTROL_WORKTREE" rev-parse --git-common-dir)"
REPO="$(cd "$COMMON_DIR/.." && pwd)"
RECOVERY="$COMMON_DIR/agent-recovery/issue-541"
CLASSIFICATION="$RECOVERY/classification.md"
python3 - "$CLASSIFICATION" <<'PY'
from pathlib import Path
import re
import sys

classification_path = Path(sys.argv[1])
expected = [
    "docs-psyche-specs",
    "memory-promote",
    "mobile-memory-gateway",
    "pr-476-review",
    "docs-universal-runtime-capability-design",
    "feat-npm-macos-x64",
    "fix-521-ward-surface-confinement",
]
terminal = {"already-shipped", "superseded", "viable", "blocked"}
pr_re = re.compile(r"^https://github\.com/OpenCoven/coven/pull/\d+$")
issue_re = re.compile(r"^https://github\.com/OpenCoven/coven/issues/\d+$")
sha_re = re.compile(r"^[0-9a-f]{40}$")
rows = {}

def parse_action(action: str) -> dict[str, str]:
    fields: dict[str, str] = {}
    for raw_part in action.split(";"):
        part = raw_part.strip()
        if not part:
            continue
        if "=" not in part:
            raise SystemExit(f"Recovery action segment is not key=value: {part}")
        key, value = part.split("=", 1)
        key = key.strip()
        value = value.strip()
        if not key or not value:
            raise SystemExit(f"Recovery action segment has an empty key or value: {part}")
        if key in fields:
            raise SystemExit(f"Recovery action repeats key {key}")
        fields[key] = value
    return fields

for raw in classification_path.read_text(encoding="utf-8").splitlines():
    line = raw.strip()
    if not line.startswith("|") or not line.endswith("|"):
        continue
    cells = [cell.strip() for cell in line.strip("|").split("|")]
    if len(cells) != 5 or cells[0] == "Workstream":
        continue
    workstream, classification, evidence, preserved_source, recovery_action = cells
    if workstream not in expected:
        raise SystemExit(f"Unexpected classification row: {line}")
    if workstream in rows:
        raise SystemExit(f"Duplicate classification row for {workstream}")
    if classification not in terminal:
        raise SystemExit(f"Non-terminal classification {classification} for {workstream}")
    if classification in {"already-shipped", "superseded"}:
        if not (pr_re.fullmatch(evidence) or sha_re.fullmatch(evidence)):
            raise SystemExit(
                f"{workstream} {classification} row must use a raw OpenCoven/coven PR URL or exact main commit SHA"
            )
        action_fields = parse_action(recovery_action)
        if action_fields.get("mode") != "non-viable-proof":
            raise SystemExit(f"{workstream} {classification} row must use mode=non-viable-proof")
        if action_fields.get("classification") != classification:
            raise SystemExit(
                f"{workstream} {classification} row must repeat its classification in Recovery action"
            )
        evidence_kind = action_fields.get("evidence_kind")
        if evidence_kind == "merged-pr":
            if not pr_re.fullmatch(evidence):
                raise SystemExit(f"{workstream} merged-pr proof must use a raw PR URL")
        elif evidence_kind == "main-commit":
            if not sha_re.fullmatch(evidence):
                raise SystemExit(f"{workstream} main-commit proof must use an exact 40-character SHA")
        else:
            raise SystemExit(f"{workstream} has unsupported non-viable evidence_kind {evidence_kind}")
    elif classification == "viable":
        if not pr_re.fullmatch(evidence):
            raise SystemExit(
                f"{workstream} viable row is not terminal until Main/PR evidence is a raw OpenCoven/coven PR URL"
            )
        action_fields = parse_action(recovery_action)
        mode = action_fields.get("mode")
        pr_kind = action_fields.get("pr_kind")
        if mode not in {"continue-existing-pr", "recovery-pr-open"}:
            raise SystemExit(
                f"{workstream} viable row is not terminal with mode={mode}; awaiting-recovery-pr fails final audit"
            )
        required = {"mode", "pr_kind", "issue_url", "archive_id", "expected_branch"}
        if mode == "continue-existing-pr":
            if pr_kind == "adopted":
                required.add("preserved_head")
            elif pr_kind == "recovered":
                required.add("expected_head")
            else:
                raise SystemExit(f"{workstream} continue-existing-pr row has unsupported pr_kind {pr_kind}")
        else:
            if pr_kind != "recovered":
                raise SystemExit(f"{workstream} recovery-pr-open row must declare pr_kind=recovered")
            required.add("expected_head")
        missing = sorted(key for key in required if key not in action_fields)
        if missing:
            raise SystemExit(f"{workstream} viable terminal row is missing metadata: {', '.join(missing)}")
        if not issue_re.fullmatch(action_fields["issue_url"]):
            raise SystemExit(f"{workstream} viable row must record a raw OpenCoven/coven issue URL")
    else:
        if not evidence or not recovery_action:
            raise SystemExit(f"{workstream} blocked row must keep explicit evidence and recovery action text")
    rows[workstream] = (classification, evidence, preserved_source, recovery_action)

if len(rows) != len(expected):
    missing = [workstream for workstream in expected if workstream not in rows]
    raise SystemExit(f"Classification table is incomplete; missing rows: {', '.join(missing)}")

for workstream in expected:
    classification, evidence, _preserved_source, recovery_action = rows[workstream]
    print(f"{workstream}\t{classification}\t{evidence}\t{recovery_action}")
PY
```

Expected: the final audit parses exact classification rows rather than
label-only grep, confirms the table is complete for exactly these seven
workstreams, and rejects any viable row whose `Main/PR evidence` is not the raw
canonical `OpenCoven/coven` PR URL or whose `Recovery action` is still
`mode=awaiting-recovery-pr` or otherwise lacks the terminal PR-backed metadata
required by `continue-existing-pr` or `recovery-pr-open`. The existing
terminal rules for `already-shipped`, `superseded`, and `blocked` rows remain
in force.

- [ ] **Step 2: Verify all viable rows still have open pull requests**

For every workstream whose exact row from Step 1 says `viable`, set
`WORKSTREAM_ID` to that row's ID and run:

```bash
set -euo pipefail
resolve_control_worktree() {
  local control_branch="docs/541-incomplete-work-recovery-design"
  local expected_branch="refs/heads/$control_branch"
  local remote_branch_ref="refs/remotes/origin/$control_branch"
  local start_common_dir repo target_path path branch_ref actual_branch
  local target_registration_branch live_count stale_count live_path stale_path
  local live_status live_head local_head remote_head
  local -a live_paths=() stale_paths=()
  start_common_dir="$(git rev-parse --git-common-dir)"
  repo="$(cd "$start_common_dir/.." && pwd)"
  target_path="$repo/.worktrees/issue-541-recovery"
  if ! git -C "$repo" fetch --no-tags origin \
    "refs/heads/$control_branch:$remote_branch_ref"
  then
    printf 'Blocked: could not fetch origin/%s into %s.\n' \
      "$control_branch" "$remote_branch_ref" >&2
    exit 1
  fi
  remote_head="$(git -C "$repo" rev-parse --verify "$remote_branch_ref^{commit}")"
  while IFS="$(printf "\t")" read -r path branch_ref; do
    test -n "$path" || continue
    if test "$path" = "$target_path"; then
      target_registration_branch="$branch_ref"
    fi
    if test "$branch_ref" != "$expected_branch"; then
      continue
    fi
    if test -d "$path"; then
      live_paths+=("$path")
    else
      stale_paths+=("$path")
    fi
  done <<EOF2
$(git -C "$repo" worktree list --porcelain | awk '
  $1 == "worktree" { if (path != "") print path "\t" branch; path = substr($0, 10); branch = ""; next }
  $1 == "branch" { branch = $2; next }
  END { if (path != "") print path "\t" branch }
')
EOF2
  live_count="${#live_paths[@]}"
  stale_count="${#stale_paths[@]}"
  if test -n "$target_registration_branch" && test "$target_registration_branch" != "$expected_branch"; then
    printf 'Blocked: %s is registered to %s, not %s.\n' "$target_path" "$target_registration_branch" "$control_branch" >&2
    exit 1
  fi
  if test "$live_count" -gt 1; then
    printf 'Blocked: %s has %s live controller worktree registrations; refusing to choose arbitrarily.\n' "$control_branch" "$live_count" >&2
    for live_path in "${live_paths[@]}"; do
      printf '  live: %s\n' "$live_path" >&2
    done
    for stale_path in "${stale_paths[@]}"; do
      printf '  stale: %s\n' "$stale_path" >&2
    done
    exit 1
  fi
  if test "$live_count" -eq 1; then
    live_path="${live_paths[0]}"
    if test "$stale_count" -gt 0; then
      printf 'Warning: ignoring %s stale controller registration(s) for %s while using the one live worktree.\n' "$stale_count" "$control_branch" >&2
      for stale_path in "${stale_paths[@]}"; do
        printf '  stale: %s\n' "$stale_path" >&2
      done
    fi
    actual_branch="$(git -C "$live_path" branch --show-current)"
    if test "$actual_branch" != "$control_branch"; then
      printf 'Control worktree branch mismatch: expected %s, got %s\n' "$control_branch" "$actual_branch" >&2
      exit 1
    fi
    live_status="$(git -C "$live_path" status --porcelain=v1 --untracked-files=all)"
    if test -n "$live_status"; then
      printf 'Blocked: live controller worktree is dirty and cannot be refreshed safely: %s\n' "$live_path" >&2
      git -C "$live_path" status --short --branch --untracked-files=all >&2
      exit 1
    fi
    live_head="$(git -C "$live_path" rev-parse HEAD)"
    if test "$live_head" = "$remote_head"; then
      :
    elif git -C "$repo" merge-base --is-ancestor "$live_head" "$remote_head"; then
      if ! git -C "$live_path" merge --ff-only "$remote_branch_ref"; then
        printf 'Blocked: live controller worktree could not fast-forward %s to fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
        exit 1
      fi
    elif git -C "$repo" merge-base --is-ancestor "$remote_head" "$live_head"; then
      printf 'Blocked: live controller branch %s is ahead of fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
      exit 1
    else
      printf 'Blocked: live controller branch %s diverged from fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
      exit 1
    fi
    live_head="$(git -C "$live_path" rev-parse HEAD)"
    if test "$live_head" != "$remote_head"; then
      printf 'Blocked: live controller HEAD %s does not match fetched origin tip %s.\n' "$live_head" "$remote_head" >&2
      exit 1
    fi
    printf '%s\n' "$live_path"
    return 0
  fi
  if test "$stale_count" -gt 1; then
    printf 'Blocked: %s has %s stale controller registrations and no live worktree; refusing to choose arbitrarily.\n' "$control_branch" "$stale_count" >&2
    for stale_path in "${stale_paths[@]}"; do
      printf '  stale: %s\n' "$stale_path" >&2
    done
    exit 1
  fi
  if ! git -C "$repo" check-ignore -q .worktrees/; then
    printf 'Repository-local .worktrees/ is not ignored.\n' >&2
    exit 1
  fi
  if ! git -C "$repo" check-ignore -q .worktrees/issue-541-recovery; then
    printf 'Repository-local control worktree path is not ignored: %s\n' '.worktrees/issue-541-recovery' >&2
    exit 1
  fi
  mkdir -p "$repo/.worktrees"
  if test -e "$target_path"; then
    printf 'Control worktree path already exists without a matching live registration: %s\n' "$target_path" >&2
    exit 1
  fi
  if git -C "$repo" show-ref --verify --quiet "$expected_branch"; then
    local_head="$(git -C "$repo" rev-parse --verify "$expected_branch^{commit}")"
    if test "$local_head" = "$remote_head"; then
      :
    elif git -C "$repo" merge-base --is-ancestor "$local_head" "$remote_head"; then
      if ! git -C "$repo" update-ref "$expected_branch" "$remote_head" "$local_head"; then
        printf 'Blocked: local controller branch %s could not be fast-forwarded to fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
        exit 1
      fi
    elif git -C "$repo" merge-base --is-ancestor "$remote_head" "$local_head"; then
      printf 'Blocked: local controller branch %s is ahead of fetched origin tip %s and cannot be rewritten safely.\n' "$control_branch" "$remote_head" >&2
      exit 1
    else
      printf 'Blocked: local controller branch %s diverged from fetched origin tip %s and cannot be rewritten safely.\n' "$control_branch" "$remote_head" >&2
      exit 1
    fi
  else
    if ! git -C "$repo" update-ref "$expected_branch" "$remote_head"; then
      printf 'Blocked: local controller branch %s could not be created at fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
      exit 1
    fi
  fi
  local_head="$(git -C "$repo" rev-parse --verify "$expected_branch^{commit}")"
  if test "$local_head" != "$remote_head"; then
    printf 'Blocked: local controller branch %s resolved to %s instead of fetched origin tip %s.\n' "$control_branch" "$local_head" "$remote_head" >&2
    exit 1
  fi
  if test "$stale_count" -eq 1; then
    stale_path="${stale_paths[0]}"
    printf 'Recreating %s at %s from the one stale controller registration:\n  stale: %s\n' "$control_branch" "$target_path" "$stale_path" >&2
    git -C "$repo" worktree add --force "$target_path" "$control_branch"
  else
    git -C "$repo" worktree add "$target_path" "$control_branch"
  fi
  if ! test -d "$target_path"; then
    printf 'Control worktree path is not present after resolution: %s\n' "$target_path" >&2
    exit 1
  fi
  actual_branch="$(git -C "$target_path" branch --show-current)"
  if test "$actual_branch" != "$control_branch"; then
    printf 'Control worktree branch mismatch: expected %s, got %s\n' "$control_branch" "$actual_branch" >&2
    exit 1
  fi
  local_head="$(git -C "$target_path" rev-parse HEAD)"
  if test "$local_head" != "$remote_head"; then
    printf 'Blocked: resolved controller HEAD %s does not match fetched origin tip %s.\n' "$local_head" "$remote_head" >&2
    exit 1
  fi
  printf '%s\n' "$target_path"
}
CONTROL_WORKTREE="$(resolve_control_worktree)"
COMMON_DIR="$(git -C "$CONTROL_WORKTREE" rev-parse --git-common-dir)"
REPO="$(cd "$COMMON_DIR/.." && pwd)"
CLASSIFICATION="$COMMON_DIR/agent-recovery/issue-541/classification.md"
cd "$REPO"
mapfile -t VIABLE_ROW < <(python3 - "$CLASSIFICATION" "$WORKSTREAM_ID" <<'PY'
from pathlib import Path
import re
import sys

classification_path = Path(sys.argv[1])
workstream = sys.argv[2]
pr_re = re.compile(r"^https://github\.com/OpenCoven/coven/pull/\d+$")
issue_re = re.compile(r"^https://github\.com/OpenCoven/coven/issues/\d+$")

for raw in classification_path.read_text(encoding="utf-8").splitlines():
    line = raw.strip()
    if not line.startswith("|") or not line.endswith("|"):
        continue
    cells = [cell.strip() for cell in line.strip("|").split("|")]
    if len(cells) != 5 or cells[0] == "Workstream":
        continue
    if cells[0] != workstream:
        continue
    classification, evidence, _preserved_source, recovery_action = cells[1:]
    if classification != "viable":
        raise SystemExit(f"{workstream} is {classification}, not viable")
    if not pr_re.fullmatch(evidence):
        raise SystemExit(f"{workstream} viable row must record a raw OpenCoven/coven PR URL")
    fields = {}
    for raw_part in recovery_action.split(";"):
        part = raw_part.strip()
        if not part:
            continue
        if "=" not in part:
            raise SystemExit(f"Recovery action segment is not key=value: {part}")
        key, value = part.split("=", 1)
        key = key.strip()
        value = value.strip()
        if not key or not value:
            raise SystemExit(f"Recovery action segment has an empty key or value: {part}")
        if key in fields:
            raise SystemExit(f"Recovery action repeats key {key}")
        fields[key] = value
    mode = fields.get("mode")
    pr_kind = fields.get("pr_kind")
    if mode not in {"continue-existing-pr", "recovery-pr-open"}:
        raise SystemExit(f"{workstream} viable row is not terminal with mode={mode}")
    required = {"mode", "pr_kind", "issue_url", "archive_id", "expected_branch"}
    if mode == "continue-existing-pr":
        if pr_kind == "adopted":
            required.add("preserved_head")
        elif pr_kind == "recovered":
            required.add("expected_head")
        else:
            raise SystemExit(f"{workstream} continue-existing-pr row has unsupported pr_kind {pr_kind}")
    else:
        if pr_kind != "recovered":
            raise SystemExit(f"{workstream} recovery-pr-open row must declare pr_kind=recovered")
        required.add("expected_head")
    missing = sorted(key for key in required if key not in fields)
    if missing:
        raise SystemExit(f"{workstream} viable row is missing metadata: {', '.join(missing)}")
    if not issue_re.fullmatch(fields["issue_url"]):
        raise SystemExit(f"{workstream} viable row must record a raw OpenCoven/coven issue URL")
    print(evidence)
    print(fields["expected_branch"])
    print(mode)
    print(pr_kind)
    break
else:
    raise SystemExit(f"Missing classification row for {workstream}")
PY
)
if test "${#VIABLE_ROW[@]}" -ne 4; then
  printf 'Expected exactly four parsed fields for viable workstream %s, got %s.\n' \
    "$WORKSTREAM_ID" "${#VIABLE_ROW[@]}" >&2
  exit 1
fi
PR_URL="${VIABLE_ROW[0]}"
EXPECTED_BRANCH="${VIABLE_ROW[1]}"
ACTION_MODE="${VIABLE_ROW[2]}"
ACTION_PR_KIND="${VIABLE_ROW[3]}"
PR_VIEW_JSON="$(gh pr view "$PR_URL" --repo OpenCoven/coven \
  --json state,isDraft,mergeStateStatus,url,headRefName,headRefOid,headRepositoryOwner,isCrossRepository,baseRefName)"
ACTUAL_URL="$(printf '%s\n' "$PR_VIEW_JSON" | jq -r '.url')"
ACTUAL_STATE="$(printf '%s\n' "$PR_VIEW_JSON" | jq -r '.state')"
ACTUAL_BRANCH="$(printf '%s\n' "$PR_VIEW_JSON" | jq -r '.headRefName')"
ACTUAL_OWNER="$(printf '%s\n' "$PR_VIEW_JSON" | jq -r '.headRepositoryOwner.login')"
ACTUAL_CROSS="$(printf '%s\n' "$PR_VIEW_JSON" | jq -r '.isCrossRepository')"
ACTUAL_BASE="$(printf '%s\n' "$PR_VIEW_JSON" | jq -r '.baseRefName')"
if [ "$ACTUAL_URL" != "$PR_URL" ] || \
   [ "$ACTUAL_STATE" != "OPEN" ] || \
   [ "$ACTUAL_BRANCH" != "$EXPECTED_BRANCH" ] || \
   [ "$ACTUAL_OWNER" != "OpenCoven" ] || \
   [ "$ACTUAL_CROSS" != "false" ] || \
   [ "$ACTUAL_BASE" != "main" ]; then
  printf 'Viable row %s no longer matches its recorded OPEN same-repo main PR.\n' \
    "$WORKSTREAM_ID" >&2
  printf 'Expected URL: %s\nExpected branch: %s\nExpected mode/kind: %s/%s\n' \
    "$PR_URL" "$EXPECTED_BRANCH" "$ACTION_MODE" "$ACTION_PR_KIND" >&2
  printf 'Actual JSON:\n%s\n' "$PR_VIEW_JSON" >&2
  exit 1
fi
printf '%s\n' "$PR_VIEW_JSON"
```

Expected: each viable row is re-derived from its exact classification row
rather than from a copied label match, and its live PR still verifies as the
OPEN same-repo `main` PR recorded there. Rows still stuck at
`mode=awaiting-recovery-pr`, rows with non-canonical PR evidence, or rows whose
expected branch no longer matches the PR head fail the final audit.

- [ ] **Step 3: Restore the primary checkout before the final audit**

Run:

```bash
set -euo pipefail
resolve_control_worktree() {
  local control_branch="docs/541-incomplete-work-recovery-design"
  local expected_branch="refs/heads/$control_branch"
  local remote_branch_ref="refs/remotes/origin/$control_branch"
  local start_common_dir repo target_path path branch_ref actual_branch
  local target_registration_branch live_count stale_count live_path stale_path
  local live_status live_head local_head remote_head
  local -a live_paths=() stale_paths=()
  start_common_dir="$(git rev-parse --git-common-dir)"
  repo="$(cd "$start_common_dir/.." && pwd)"
  target_path="$repo/.worktrees/issue-541-recovery"
  if ! git -C "$repo" fetch --no-tags origin \
    "refs/heads/$control_branch:$remote_branch_ref"
  then
    printf 'Blocked: could not fetch origin/%s into %s.\n' \
      "$control_branch" "$remote_branch_ref" >&2
    exit 1
  fi
  remote_head="$(git -C "$repo" rev-parse --verify "$remote_branch_ref^{commit}")"
  while IFS="$(printf "\t")" read -r path branch_ref; do
    test -n "$path" || continue
    if test "$path" = "$target_path"; then
      target_registration_branch="$branch_ref"
    fi
    if test "$branch_ref" != "$expected_branch"; then
      continue
    fi
    if test -d "$path"; then
      live_paths+=("$path")
    else
      stale_paths+=("$path")
    fi
  done <<EOF2
$(git -C "$repo" worktree list --porcelain | awk '
  $1 == "worktree" { if (path != "") print path "\t" branch; path = substr($0, 10); branch = ""; next }
  $1 == "branch" { branch = $2; next }
  END { if (path != "") print path "\t" branch }
')
EOF2
  live_count="${#live_paths[@]}"
  stale_count="${#stale_paths[@]}"
  if test -n "$target_registration_branch" && test "$target_registration_branch" != "$expected_branch"; then
    printf 'Blocked: %s is registered to %s, not %s.\n' "$target_path" "$target_registration_branch" "$control_branch" >&2
    exit 1
  fi
  if test "$live_count" -gt 1; then
    printf 'Blocked: %s has %s live controller worktree registrations; refusing to choose arbitrarily.\n' "$control_branch" "$live_count" >&2
    for live_path in "${live_paths[@]}"; do
      printf '  live: %s\n' "$live_path" >&2
    done
    for stale_path in "${stale_paths[@]}"; do
      printf '  stale: %s\n' "$stale_path" >&2
    done
    exit 1
  fi
  if test "$live_count" -eq 1; then
    live_path="${live_paths[0]}"
    if test "$stale_count" -gt 0; then
      printf 'Warning: ignoring %s stale controller registration(s) for %s while using the one live worktree.\n' "$stale_count" "$control_branch" >&2
      for stale_path in "${stale_paths[@]}"; do
        printf '  stale: %s\n' "$stale_path" >&2
      done
    fi
    actual_branch="$(git -C "$live_path" branch --show-current)"
    if test "$actual_branch" != "$control_branch"; then
      printf 'Control worktree branch mismatch: expected %s, got %s\n' "$control_branch" "$actual_branch" >&2
      exit 1
    fi
    live_status="$(git -C "$live_path" status --porcelain=v1 --untracked-files=all)"
    if test -n "$live_status"; then
      printf 'Blocked: live controller worktree is dirty and cannot be refreshed safely: %s\n' "$live_path" >&2
      git -C "$live_path" status --short --branch --untracked-files=all >&2
      exit 1
    fi
    live_head="$(git -C "$live_path" rev-parse HEAD)"
    if test "$live_head" = "$remote_head"; then
      :
    elif git -C "$repo" merge-base --is-ancestor "$live_head" "$remote_head"; then
      if ! git -C "$live_path" merge --ff-only "$remote_branch_ref"; then
        printf 'Blocked: live controller worktree could not fast-forward %s to fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
        exit 1
      fi
    elif git -C "$repo" merge-base --is-ancestor "$remote_head" "$live_head"; then
      printf 'Blocked: live controller branch %s is ahead of fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
      exit 1
    else
      printf 'Blocked: live controller branch %s diverged from fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
      exit 1
    fi
    live_head="$(git -C "$live_path" rev-parse HEAD)"
    if test "$live_head" != "$remote_head"; then
      printf 'Blocked: live controller HEAD %s does not match fetched origin tip %s.\n' "$live_head" "$remote_head" >&2
      exit 1
    fi
    printf '%s\n' "$live_path"
    return 0
  fi
  if test "$stale_count" -gt 1; then
    printf 'Blocked: %s has %s stale controller registrations and no live worktree; refusing to choose arbitrarily.\n' "$control_branch" "$stale_count" >&2
    for stale_path in "${stale_paths[@]}"; do
      printf '  stale: %s\n' "$stale_path" >&2
    done
    exit 1
  fi
  if ! git -C "$repo" check-ignore -q .worktrees/; then
    printf 'Repository-local .worktrees/ is not ignored.\n' >&2
    exit 1
  fi
  if ! git -C "$repo" check-ignore -q .worktrees/issue-541-recovery; then
    printf 'Repository-local control worktree path is not ignored: %s\n' '.worktrees/issue-541-recovery' >&2
    exit 1
  fi
  mkdir -p "$repo/.worktrees"
  if test -e "$target_path"; then
    printf 'Control worktree path already exists without a matching live registration: %s\n' "$target_path" >&2
    exit 1
  fi
  if git -C "$repo" show-ref --verify --quiet "$expected_branch"; then
    local_head="$(git -C "$repo" rev-parse --verify "$expected_branch^{commit}")"
    if test "$local_head" = "$remote_head"; then
      :
    elif git -C "$repo" merge-base --is-ancestor "$local_head" "$remote_head"; then
      if ! git -C "$repo" update-ref "$expected_branch" "$remote_head" "$local_head"; then
        printf 'Blocked: local controller branch %s could not be fast-forwarded to fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
        exit 1
      fi
    elif git -C "$repo" merge-base --is-ancestor "$remote_head" "$local_head"; then
      printf 'Blocked: local controller branch %s is ahead of fetched origin tip %s and cannot be rewritten safely.\n' "$control_branch" "$remote_head" >&2
      exit 1
    else
      printf 'Blocked: local controller branch %s diverged from fetched origin tip %s and cannot be rewritten safely.\n' "$control_branch" "$remote_head" >&2
      exit 1
    fi
  else
    if ! git -C "$repo" update-ref "$expected_branch" "$remote_head"; then
      printf 'Blocked: local controller branch %s could not be created at fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
      exit 1
    fi
  fi
  local_head="$(git -C "$repo" rev-parse --verify "$expected_branch^{commit}")"
  if test "$local_head" != "$remote_head"; then
    printf 'Blocked: local controller branch %s resolved to %s instead of fetched origin tip %s.\n' "$control_branch" "$local_head" "$remote_head" >&2
    exit 1
  fi
  if test "$stale_count" -eq 1; then
    stale_path="${stale_paths[0]}"
    printf 'Recreating %s at %s from the one stale controller registration:\n  stale: %s\n' "$control_branch" "$target_path" "$stale_path" >&2
    git -C "$repo" worktree add --force "$target_path" "$control_branch"
  else
    git -C "$repo" worktree add "$target_path" "$control_branch"
  fi
  if ! test -d "$target_path"; then
    printf 'Control worktree path is not present after resolution: %s\n' "$target_path" >&2
    exit 1
  fi
  actual_branch="$(git -C "$target_path" branch --show-current)"
  if test "$actual_branch" != "$control_branch"; then
    printf 'Control worktree branch mismatch: expected %s, got %s\n' "$control_branch" "$actual_branch" >&2
    exit 1
  fi
  local_head="$(git -C "$target_path" rev-parse HEAD)"
  if test "$local_head" != "$remote_head"; then
    printf 'Blocked: resolved controller HEAD %s does not match fetched origin tip %s.\n' "$local_head" "$remote_head" >&2
    exit 1
  fi
  printf '%s\n' "$target_path"
}
CONTROL_WORKTREE="$(resolve_control_worktree)"
COMMON_DIR="$(git -C "$CONTROL_WORKTREE" rev-parse --git-common-dir)"
REPO="$(cd "$COMMON_DIR/.." && pwd)"
AUDIT_EVIDENCE="$COMMON_DIR/agent-recovery/issue-541/final-audit-primary-checkout.txt"
cd "$REPO"
ORIGINAL_BRANCH="$(git branch --show-current)"
CURRENT_BRANCH="$ORIGINAL_BRANCH"
UPSTREAM_REF="$(git rev-parse --abbrev-ref --symbolic-full-name @{upstream} 2>/dev/null || true)"
UPSTREAM_REMOTE="$(git config --get "branch.$CURRENT_BRANCH.remote" 2>/dev/null || true)"
UPSTREAM_MERGE_REF="$(git config --get "branch.$CURRENT_BRANCH.merge" 2>/dev/null || true)"
STATUS="$(git status --porcelain=v1 --untracked-files=all)"
SWITCHED_TO_MAIN=0
record_state() {
  local current_branch current_upstream current_remote current_merge
  current_branch="$(git branch --show-current)"
  current_upstream="$(git rev-parse --abbrev-ref --symbolic-full-name @{upstream} 2>/dev/null || true)"
  current_remote="$(git config --get "branch.$current_branch.remote" 2>/dev/null || true)"
  current_merge="$(git config --get "branch.$current_branch.merge" 2>/dev/null || true)"
  printf 'Original branch: %s\n' "$ORIGINAL_BRANCH"
  printf 'Current branch: %s\n' "$current_branch"
  printf 'Current HEAD: %s\n' "$(git rev-parse HEAD)"
  printf 'Configured upstream: %s\n' "${current_upstream:-<none>}"
  printf 'Configured upstream remote: %s\n' "${current_remote:-<none>}"
  printf 'Configured upstream merge ref: %s\n' "${current_merge:-<none>}"
  git status --short --branch --untracked-files=all
}
block_restore() {
  {
    printf '%s\n' "$1"
    record_state
  } > "$AUDIT_EVIDENCE"
  cat "$AUDIT_EVIDENCE"
  exit 1
}
rollback_or_block() {
  local message="$1" current_branch_after_rollback rollback_status
  if [ -z "$message" ]; then
    {
      printf 'Critical: rollback_or_block requires a non-empty reason.\n'
      record_state
    } > "$AUDIT_EVIDENCE"
    cat "$AUDIT_EVIDENCE"
    exit 1
  fi
  if [ "$SWITCHED_TO_MAIN" != "1" ]; then
    block_restore "$message"
  fi
  if [ "$(git branch --show-current)" != "$ORIGINAL_BRANCH" ]; then
    if ! git switch "$ORIGINAL_BRANCH"; then
      {
        printf 'Critical: rollback to the original branch failed after restore error.\n'
        printf 'Original branch: %s\n' "$ORIGINAL_BRANCH"
        printf '%s\n' "$message"
        record_state
      } > "$AUDIT_EVIDENCE"
      cat "$AUDIT_EVIDENCE"
      exit 1
    fi
  fi
  current_branch_after_rollback="$(git branch --show-current)"
  rollback_status="$(git status --porcelain --untracked-files=all)"
  if [ "$current_branch_after_rollback" != "$ORIGINAL_BRANCH" ]; then
    {
      printf 'Critical: rollback did not restore the original branch after restore error.\n'
      printf 'Expected branch: %s\n' "$ORIGINAL_BRANCH"
      record_state
    } > "$AUDIT_EVIDENCE"
    cat "$AUDIT_EVIDENCE"
    exit 1
  fi
  if [ -n "$rollback_status" ]; then
    {
      printf 'Critical: rollback restored the original branch but left the primary checkout dirty.\n'
      printf 'Original branch: %s\n' "$ORIGINAL_BRANCH"
      printf '%s\n' "$message"
      record_state
    } > "$AUDIT_EVIDENCE"
    cat "$AUDIT_EVIDENCE"
    exit 1
  fi
  {
    printf '%s\n' "$message"
    printf 'Rollback succeeded: restored the original branch %s with a clean checkout.\n' "$ORIGINAL_BRANCH"
    record_state
  } > "$AUDIT_EVIDENCE"
  cat "$AUDIT_EVIDENCE"
  exit 1
}
if [ -n "$STATUS" ]; then
  block_restore \
    "Blocked: primary checkout has tracked or untracked changes; leave it untouched."
fi
if ! git fetch origin main; then
  block_restore \
    "Blocked: could not fetch origin/main for the primary checkout; leave it untouched."
fi
if ! git show-ref --verify --quiet refs/heads/main; then
  block_restore "Blocked: local main branch is missing; leave the checkout untouched."
fi
if ! git merge-base --is-ancestor main origin/main; then
  block_restore \
    "Blocked: local main cannot be fast-forwarded safely to origin/main; leave the checkout untouched."
fi
if [ "$CURRENT_BRANCH" = "main" ]; then
  if ! git merge --ff-only origin/main; then
    block_restore \
      "Blocked: primary checkout could not fast-forward main to origin/main; leave it untouched."
  fi
elif case "$CURRENT_BRANCH" in
  docs/541-incomplete-work-recovery-design|issue-541-*)
    ;;
  *)
    case "$UPSTREAM_REF" in
      origin/docs/541-incomplete-work-recovery-design|origin/issue-541-*)
        ;;
      *)
        false
        ;;
    esac
    ;;
esac; then
  if [ -z "$UPSTREAM_REMOTE" ] || [ -z "$UPSTREAM_MERGE_REF" ]; then
    block_restore \
      "Blocked: recovery-owned branch has no configured upstream remote/branch; leave it untouched."
  fi
  if ! git remote get-url "$UPSTREAM_REMOTE" > /dev/null 2>&1; then
    block_restore \
      "Blocked: recovery-owned branch upstream remote cannot be resolved; leave it untouched."
  fi
  if ! git fetch "$UPSTREAM_REMOTE" "$UPSTREAM_MERGE_REF"; then
    block_restore \
      "Blocked: recovery-owned branch upstream could not be fetched; leave it untouched."
  fi
  FETCHED_UPSTREAM="$(git rev-parse --verify FETCH_HEAD^{commit} 2>/dev/null || true)"
  if [ -z "$FETCHED_UPSTREAM" ]; then
    block_restore \
      "Blocked: recovery-owned branch upstream fetch produced no commit tip; leave it untouched."
  fi
  if ! git merge-base --is-ancestor HEAD "$FETCHED_UPSTREAM"; then
    block_restore \
      "Blocked: recovery-owned branch HEAD is not fully pushed to its freshly fetched configured upstream; leave it untouched."
  fi
  if ! git switch main; then
    block_restore \
      "Blocked: could not switch the primary checkout to main; leave it untouched."
  fi
  SWITCHED_TO_MAIN=1
  if ! git merge --ff-only origin/main; then
    rollback_or_block \
      "Blocked: primary checkout could not fast-forward main to origin/main after switching to main."
  fi
else
  block_restore \
    "Blocked: primary checkout is on an unrelated branch; leave it untouched."
fi
FINAL_BRANCH="$(git branch --show-current)"
FINAL_STATUS="$(git status --porcelain --untracked-files=all)"
if [ "$FINAL_BRANCH" != "main" ]; then
  rollback_or_block \
    "Blocked: final primary checkout branch is not main after restore."
fi
if [ -n "$FINAL_STATUS" ]; then
  rollback_or_block \
    "Blocked: primary checkout is not clean after restore."
fi
{
  printf 'Primary checkout restored for final audit.\n'
  record_state
} > "$AUDIT_EVIDENCE"
cat "$AUDIT_EVIDENCE"
```

This step is mandatory before any final-audit branch-sensitive command. It
must stay non-interactive: do not use `git reset`, `git checkout --force`, or
any other destructive switch. Save `ORIGINAL_BRANCH` before any branch switch
and keep every possible validation, fetch, upstream, and fast-forward
feasibility check ahead of `git switch main`. If this step exits non-zero
before `git switch main` succeeds, stop Task 9 and leave the primary checkout
as-is. If `git switch main` succeeds and any later merge or final verification
fails, switch back to `ORIGINAL_BRANCH` before writing blocker evidence, verify
that rollback restored the original clean state, and emit a distinct critical
blocker if rollback itself fails. For recovery-owned branches, the safety check
must parse `branch.<name>.remote` and `branch.<name>.merge`, fetch that exact
upstream into `FETCH_HEAD`, and compare `HEAD` with the fresh upstream tip
rather than relying on a stale remote-tracking ref. Do not claim the primary
checkout remained untouched unless rollback restored the original branch and
clean state.

Expected: the primary checkout either remains on its original clean branch with
blocker evidence because no switch occurred or rollback succeeded, or it is
restored cleanly to `main` by a safe fast-forward-only path. Any rollback
failure is a distinct critical blocker in `final-audit-primary-checkout.txt`.

- [ ] **Step 4: Verify the primary checkout**

Run:

```bash
set -euo pipefail
resolve_control_worktree() {
  local control_branch="docs/541-incomplete-work-recovery-design"
  local expected_branch="refs/heads/$control_branch"
  local remote_branch_ref="refs/remotes/origin/$control_branch"
  local start_common_dir repo target_path path branch_ref actual_branch
  local target_registration_branch live_count stale_count live_path stale_path
  local live_status live_head local_head remote_head
  local -a live_paths=() stale_paths=()
  start_common_dir="$(git rev-parse --git-common-dir)"
  repo="$(cd "$start_common_dir/.." && pwd)"
  target_path="$repo/.worktrees/issue-541-recovery"
  if ! git -C "$repo" fetch --no-tags origin \
    "refs/heads/$control_branch:$remote_branch_ref"
  then
    printf 'Blocked: could not fetch origin/%s into %s.\n' \
      "$control_branch" "$remote_branch_ref" >&2
    exit 1
  fi
  remote_head="$(git -C "$repo" rev-parse --verify "$remote_branch_ref^{commit}")"
  while IFS="$(printf "\t")" read -r path branch_ref; do
    test -n "$path" || continue
    if test "$path" = "$target_path"; then
      target_registration_branch="$branch_ref"
    fi
    if test "$branch_ref" != "$expected_branch"; then
      continue
    fi
    if test -d "$path"; then
      live_paths+=("$path")
    else
      stale_paths+=("$path")
    fi
  done <<EOF2
$(git -C "$repo" worktree list --porcelain | awk '
  $1 == "worktree" { if (path != "") print path "\t" branch; path = substr($0, 10); branch = ""; next }
  $1 == "branch" { branch = $2; next }
  END { if (path != "") print path "\t" branch }
')
EOF2
  live_count="${#live_paths[@]}"
  stale_count="${#stale_paths[@]}"
  if test -n "$target_registration_branch" && test "$target_registration_branch" != "$expected_branch"; then
    printf 'Blocked: %s is registered to %s, not %s.\n' "$target_path" "$target_registration_branch" "$control_branch" >&2
    exit 1
  fi
  if test "$live_count" -gt 1; then
    printf 'Blocked: %s has %s live controller worktree registrations; refusing to choose arbitrarily.\n' "$control_branch" "$live_count" >&2
    for live_path in "${live_paths[@]}"; do
      printf '  live: %s\n' "$live_path" >&2
    done
    for stale_path in "${stale_paths[@]}"; do
      printf '  stale: %s\n' "$stale_path" >&2
    done
    exit 1
  fi
  if test "$live_count" -eq 1; then
    live_path="${live_paths[0]}"
    if test "$stale_count" -gt 0; then
      printf 'Warning: ignoring %s stale controller registration(s) for %s while using the one live worktree.\n' "$stale_count" "$control_branch" >&2
      for stale_path in "${stale_paths[@]}"; do
        printf '  stale: %s\n' "$stale_path" >&2
      done
    fi
    actual_branch="$(git -C "$live_path" branch --show-current)"
    if test "$actual_branch" != "$control_branch"; then
      printf 'Control worktree branch mismatch: expected %s, got %s\n' "$control_branch" "$actual_branch" >&2
      exit 1
    fi
    live_status="$(git -C "$live_path" status --porcelain=v1 --untracked-files=all)"
    if test -n "$live_status"; then
      printf 'Blocked: live controller worktree is dirty and cannot be refreshed safely: %s\n' "$live_path" >&2
      git -C "$live_path" status --short --branch --untracked-files=all >&2
      exit 1
    fi
    live_head="$(git -C "$live_path" rev-parse HEAD)"
    if test "$live_head" = "$remote_head"; then
      :
    elif git -C "$repo" merge-base --is-ancestor "$live_head" "$remote_head"; then
      if ! git -C "$live_path" merge --ff-only "$remote_branch_ref"; then
        printf 'Blocked: live controller worktree could not fast-forward %s to fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
        exit 1
      fi
    elif git -C "$repo" merge-base --is-ancestor "$remote_head" "$live_head"; then
      printf 'Blocked: live controller branch %s is ahead of fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
      exit 1
    else
      printf 'Blocked: live controller branch %s diverged from fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
      exit 1
    fi
    live_head="$(git -C "$live_path" rev-parse HEAD)"
    if test "$live_head" != "$remote_head"; then
      printf 'Blocked: live controller HEAD %s does not match fetched origin tip %s.\n' "$live_head" "$remote_head" >&2
      exit 1
    fi
    printf '%s\n' "$live_path"
    return 0
  fi
  if test "$stale_count" -gt 1; then
    printf 'Blocked: %s has %s stale controller registrations and no live worktree; refusing to choose arbitrarily.\n' "$control_branch" "$stale_count" >&2
    for stale_path in "${stale_paths[@]}"; do
      printf '  stale: %s\n' "$stale_path" >&2
    done
    exit 1
  fi
  if ! git -C "$repo" check-ignore -q .worktrees/; then
    printf 'Repository-local .worktrees/ is not ignored.\n' >&2
    exit 1
  fi
  if ! git -C "$repo" check-ignore -q .worktrees/issue-541-recovery; then
    printf 'Repository-local control worktree path is not ignored: %s\n' '.worktrees/issue-541-recovery' >&2
    exit 1
  fi
  mkdir -p "$repo/.worktrees"
  if test -e "$target_path"; then
    printf 'Control worktree path already exists without a matching live registration: %s\n' "$target_path" >&2
    exit 1
  fi
  if git -C "$repo" show-ref --verify --quiet "$expected_branch"; then
    local_head="$(git -C "$repo" rev-parse --verify "$expected_branch^{commit}")"
    if test "$local_head" = "$remote_head"; then
      :
    elif git -C "$repo" merge-base --is-ancestor "$local_head" "$remote_head"; then
      if ! git -C "$repo" update-ref "$expected_branch" "$remote_head" "$local_head"; then
        printf 'Blocked: local controller branch %s could not be fast-forwarded to fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
        exit 1
      fi
    elif git -C "$repo" merge-base --is-ancestor "$remote_head" "$local_head"; then
      printf 'Blocked: local controller branch %s is ahead of fetched origin tip %s and cannot be rewritten safely.\n' "$control_branch" "$remote_head" >&2
      exit 1
    else
      printf 'Blocked: local controller branch %s diverged from fetched origin tip %s and cannot be rewritten safely.\n' "$control_branch" "$remote_head" >&2
      exit 1
    fi
  else
    if ! git -C "$repo" update-ref "$expected_branch" "$remote_head"; then
      printf 'Blocked: local controller branch %s could not be created at fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
      exit 1
    fi
  fi
  local_head="$(git -C "$repo" rev-parse --verify "$expected_branch^{commit}")"
  if test "$local_head" != "$remote_head"; then
    printf 'Blocked: local controller branch %s resolved to %s instead of fetched origin tip %s.\n' "$control_branch" "$local_head" "$remote_head" >&2
    exit 1
  fi
  if test "$stale_count" -eq 1; then
    stale_path="${stale_paths[0]}"
    printf 'Recreating %s at %s from the one stale controller registration:\n  stale: %s\n' "$control_branch" "$target_path" "$stale_path" >&2
    git -C "$repo" worktree add --force "$target_path" "$control_branch"
  else
    git -C "$repo" worktree add "$target_path" "$control_branch"
  fi
  if ! test -d "$target_path"; then
    printf 'Control worktree path is not present after resolution: %s\n' "$target_path" >&2
    exit 1
  fi
  actual_branch="$(git -C "$target_path" branch --show-current)"
  if test "$actual_branch" != "$control_branch"; then
    printf 'Control worktree branch mismatch: expected %s, got %s\n' "$control_branch" "$actual_branch" >&2
    exit 1
  fi
  local_head="$(git -C "$target_path" rev-parse HEAD)"
  if test "$local_head" != "$remote_head"; then
    printf 'Blocked: resolved controller HEAD %s does not match fetched origin tip %s.\n' "$local_head" "$remote_head" >&2
    exit 1
  fi
  printf '%s\n' "$target_path"
}
CONTROL_WORKTREE="$(resolve_control_worktree)"
COMMON_DIR="$(git -C "$CONTROL_WORKTREE" rev-parse --git-common-dir)"
REPO="$(cd "$COMMON_DIR/.." && pwd)"
cd "$REPO"
git status --short --branch
git worktree list
coven claim status
gh pr list --repo OpenCoven/coven --state open --limit 100
```

Document every remaining recovery-owned worktree, claim, and open PR from these
outputs. If unrelated active worktrees, claims, or PRs also appear, identify
them generically as out-of-scope ongoing work and leave them untouched.

Expected: the primary checkout is clean on `main`; every remaining
recovery-owned worktree, claim, and open PR is documented; and unrelated active
worktrees, claims, and PRs remain allowed when they are explicitly marked
out-of-scope rather than removed or treated as audit failures.

- [ ] **Step 6: Self-review all operational bash fences**

Extract every fenced `bash` block in this plan into repo-local temporary files
under the recovery archive, then run the repository's existing bash-syntax
validation script if one already exists; otherwise run `/bin/bash -n` against
each extracted file. Do not add new tooling solely for this check. Fix any
syntax error before proceeding, and re-run the validation until every
operational fence parses cleanly.

- [ ] **Step 5: Reject unsanitized local paths before posting the ledger**

Run:

```bash
set -euo pipefail
resolve_control_worktree() {
  local control_branch="docs/541-incomplete-work-recovery-design"
  local expected_branch="refs/heads/$control_branch"
  local remote_branch_ref="refs/remotes/origin/$control_branch"
  local start_common_dir repo target_path path branch_ref actual_branch
  local target_registration_branch live_count stale_count live_path stale_path
  local live_status live_head local_head remote_head
  local -a live_paths=() stale_paths=()
  start_common_dir="$(git rev-parse --git-common-dir)"
  repo="$(cd "$start_common_dir/.." && pwd)"
  target_path="$repo/.worktrees/issue-541-recovery"
  if ! git -C "$repo" fetch --no-tags origin \
    "refs/heads/$control_branch:$remote_branch_ref"
  then
    printf 'Blocked: could not fetch origin/%s into %s.\n' \
      "$control_branch" "$remote_branch_ref" >&2
    exit 1
  fi
  remote_head="$(git -C "$repo" rev-parse --verify "$remote_branch_ref^{commit}")"
  while IFS="$(printf "\t")" read -r path branch_ref; do
    test -n "$path" || continue
    if test "$path" = "$target_path"; then
      target_registration_branch="$branch_ref"
    fi
    if test "$branch_ref" != "$expected_branch"; then
      continue
    fi
    if test -d "$path"; then
      live_paths+=("$path")
    else
      stale_paths+=("$path")
    fi
  done <<EOF2
$(git -C "$repo" worktree list --porcelain | awk '
  $1 == "worktree" { if (path != "") print path "\t" branch; path = substr($0, 10); branch = ""; next }
  $1 == "branch" { branch = $2; next }
  END { if (path != "") print path "\t" branch }
')
EOF2
  live_count="${#live_paths[@]}"
  stale_count="${#stale_paths[@]}"
  if test -n "$target_registration_branch" && test "$target_registration_branch" != "$expected_branch"; then
    printf 'Blocked: %s is registered to %s, not %s.\n' "$target_path" "$target_registration_branch" "$control_branch" >&2
    exit 1
  fi
  if test "$live_count" -gt 1; then
    printf 'Blocked: %s has %s live controller worktree registrations; refusing to choose arbitrarily.\n' "$control_branch" "$live_count" >&2
    for live_path in "${live_paths[@]}"; do
      printf '  live: %s\n' "$live_path" >&2
    done
    for stale_path in "${stale_paths[@]}"; do
      printf '  stale: %s\n' "$stale_path" >&2
    done
    exit 1
  fi
  if test "$live_count" -eq 1; then
    live_path="${live_paths[0]}"
    if test "$stale_count" -gt 0; then
      printf 'Warning: ignoring %s stale controller registration(s) for %s while using the one live worktree.\n' "$stale_count" "$control_branch" >&2
      for stale_path in "${stale_paths[@]}"; do
        printf '  stale: %s\n' "$stale_path" >&2
      done
    fi
    actual_branch="$(git -C "$live_path" branch --show-current)"
    if test "$actual_branch" != "$control_branch"; then
      printf 'Control worktree branch mismatch: expected %s, got %s\n' "$control_branch" "$actual_branch" >&2
      exit 1
    fi
    live_status="$(git -C "$live_path" status --porcelain=v1 --untracked-files=all)"
    if test -n "$live_status"; then
      printf 'Blocked: live controller worktree is dirty and cannot be refreshed safely: %s\n' "$live_path" >&2
      git -C "$live_path" status --short --branch --untracked-files=all >&2
      exit 1
    fi
    live_head="$(git -C "$live_path" rev-parse HEAD)"
    if test "$live_head" = "$remote_head"; then
      :
    elif git -C "$repo" merge-base --is-ancestor "$live_head" "$remote_head"; then
      if ! git -C "$live_path" merge --ff-only "$remote_branch_ref"; then
        printf 'Blocked: live controller worktree could not fast-forward %s to fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
        exit 1
      fi
    elif git -C "$repo" merge-base --is-ancestor "$remote_head" "$live_head"; then
      printf 'Blocked: live controller branch %s is ahead of fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
      exit 1
    else
      printf 'Blocked: live controller branch %s diverged from fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
      exit 1
    fi
    live_head="$(git -C "$live_path" rev-parse HEAD)"
    if test "$live_head" != "$remote_head"; then
      printf 'Blocked: live controller HEAD %s does not match fetched origin tip %s.\n' "$live_head" "$remote_head" >&2
      exit 1
    fi
    printf '%s\n' "$live_path"
    return 0
  fi
  if test "$stale_count" -gt 1; then
    printf 'Blocked: %s has %s stale controller registrations and no live worktree; refusing to choose arbitrarily.\n' "$control_branch" "$stale_count" >&2
    for stale_path in "${stale_paths[@]}"; do
      printf '  stale: %s\n' "$stale_path" >&2
    done
    exit 1
  fi
  if ! git -C "$repo" check-ignore -q .worktrees/; then
    printf 'Repository-local .worktrees/ is not ignored.\n' >&2
    exit 1
  fi
  if ! git -C "$repo" check-ignore -q .worktrees/issue-541-recovery; then
    printf 'Repository-local control worktree path is not ignored: %s\n' '.worktrees/issue-541-recovery' >&2
    exit 1
  fi
  mkdir -p "$repo/.worktrees"
  if test -e "$target_path"; then
    printf 'Control worktree path already exists without a matching live registration: %s\n' "$target_path" >&2
    exit 1
  fi
  if git -C "$repo" show-ref --verify --quiet "$expected_branch"; then
    local_head="$(git -C "$repo" rev-parse --verify "$expected_branch^{commit}")"
    if test "$local_head" = "$remote_head"; then
      :
    elif git -C "$repo" merge-base --is-ancestor "$local_head" "$remote_head"; then
      if ! git -C "$repo" update-ref "$expected_branch" "$remote_head" "$local_head"; then
        printf 'Blocked: local controller branch %s could not be fast-forwarded to fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
        exit 1
      fi
    elif git -C "$repo" merge-base --is-ancestor "$remote_head" "$local_head"; then
      printf 'Blocked: local controller branch %s is ahead of fetched origin tip %s and cannot be rewritten safely.\n' "$control_branch" "$remote_head" >&2
      exit 1
    else
      printf 'Blocked: local controller branch %s diverged from fetched origin tip %s and cannot be rewritten safely.\n' "$control_branch" "$remote_head" >&2
      exit 1
    fi
  else
    if ! git -C "$repo" update-ref "$expected_branch" "$remote_head"; then
      printf 'Blocked: local controller branch %s could not be created at fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
      exit 1
    fi
  fi
  local_head="$(git -C "$repo" rev-parse --verify "$expected_branch^{commit}")"
  if test "$local_head" != "$remote_head"; then
    printf 'Blocked: local controller branch %s resolved to %s instead of fetched origin tip %s.\n' "$control_branch" "$local_head" "$remote_head" >&2
    exit 1
  fi
  if test "$stale_count" -eq 1; then
    stale_path="${stale_paths[0]}"
    printf 'Recreating %s at %s from the one stale controller registration:\n  stale: %s\n' "$control_branch" "$target_path" "$stale_path" >&2
    git -C "$repo" worktree add --force "$target_path" "$control_branch"
  else
    git -C "$repo" worktree add "$target_path" "$control_branch"
  fi
  if ! test -d "$target_path"; then
    printf 'Control worktree path is not present after resolution: %s\n' "$target_path" >&2
    exit 1
  fi
  actual_branch="$(git -C "$target_path" branch --show-current)"
  if test "$actual_branch" != "$control_branch"; then
    printf 'Control worktree branch mismatch: expected %s, got %s\n' "$control_branch" "$actual_branch" >&2
    exit 1
  fi
  local_head="$(git -C "$target_path" rev-parse HEAD)"
  if test "$local_head" != "$remote_head"; then
    printf 'Blocked: resolved controller HEAD %s does not match fetched origin tip %s.\n' "$local_head" "$remote_head" >&2
    exit 1
  fi
  printf '%s\n' "$target_path"
}
CONTROL_WORKTREE="$(resolve_control_worktree)"
COMMON_DIR="$(git -C "$CONTROL_WORKTREE" rev-parse --git-common-dir)"
REPO="$(cd "$COMMON_DIR/.." && pwd)"
CLASSIFICATION="$COMMON_DIR/agent-recovery/issue-541/classification.md"
python3 - "$CLASSIFICATION" "$REPO" "$COMMON_DIR" <<'PY'
from pathlib import Path
import sys

classification = Path(sys.argv[1]).read_text()
blocked_prefixes = [sys.argv[2], sys.argv[3], "/tmp", "/private", "/Users", "/home"]
hits = [prefix for prefix in blocked_prefixes if prefix and prefix in classification]
if hits:
    raise SystemExit(
        "classification.md contains unsanitized local path prefix(es): "
        + ", ".join(hits)
    )
PY
```

Expected: the command exits zero only when `classification.md` contains
sanitized archive IDs and no local absolute path prefixes.

- [ ] **Step 6: Update issue #541**

Post a comment containing the sanitized classification table, recovery PR
links, non-viable evidence, and remaining human blockers. Do not post
`manifest.tsv` or raw snapshot files:

```bash
set -euo pipefail
resolve_control_worktree() {
  local control_branch="docs/541-incomplete-work-recovery-design"
  local expected_branch="refs/heads/$control_branch"
  local remote_branch_ref="refs/remotes/origin/$control_branch"
  local start_common_dir repo target_path path branch_ref actual_branch
  local target_registration_branch live_count stale_count live_path stale_path
  local live_status live_head local_head remote_head
  local -a live_paths=() stale_paths=()
  start_common_dir="$(git rev-parse --git-common-dir)"
  repo="$(cd "$start_common_dir/.." && pwd)"
  target_path="$repo/.worktrees/issue-541-recovery"
  if ! git -C "$repo" fetch --no-tags origin \
    "refs/heads/$control_branch:$remote_branch_ref"
  then
    printf 'Blocked: could not fetch origin/%s into %s.\n' \
      "$control_branch" "$remote_branch_ref" >&2
    exit 1
  fi
  remote_head="$(git -C "$repo" rev-parse --verify "$remote_branch_ref^{commit}")"
  while IFS="$(printf "\t")" read -r path branch_ref; do
    test -n "$path" || continue
    if test "$path" = "$target_path"; then
      target_registration_branch="$branch_ref"
    fi
    if test "$branch_ref" != "$expected_branch"; then
      continue
    fi
    if test -d "$path"; then
      live_paths+=("$path")
    else
      stale_paths+=("$path")
    fi
  done <<EOF2
$(git -C "$repo" worktree list --porcelain | awk '
  $1 == "worktree" { if (path != "") print path "\t" branch; path = substr($0, 10); branch = ""; next }
  $1 == "branch" { branch = $2; next }
  END { if (path != "") print path "\t" branch }
')
EOF2
  live_count="${#live_paths[@]}"
  stale_count="${#stale_paths[@]}"
  if test -n "$target_registration_branch" && test "$target_registration_branch" != "$expected_branch"; then
    printf 'Blocked: %s is registered to %s, not %s.\n' "$target_path" "$target_registration_branch" "$control_branch" >&2
    exit 1
  fi
  if test "$live_count" -gt 1; then
    printf 'Blocked: %s has %s live controller worktree registrations; refusing to choose arbitrarily.\n' "$control_branch" "$live_count" >&2
    for live_path in "${live_paths[@]}"; do
      printf '  live: %s\n' "$live_path" >&2
    done
    for stale_path in "${stale_paths[@]}"; do
      printf '  stale: %s\n' "$stale_path" >&2
    done
    exit 1
  fi
  if test "$live_count" -eq 1; then
    live_path="${live_paths[0]}"
    if test "$stale_count" -gt 0; then
      printf 'Warning: ignoring %s stale controller registration(s) for %s while using the one live worktree.\n' "$stale_count" "$control_branch" >&2
      for stale_path in "${stale_paths[@]}"; do
        printf '  stale: %s\n' "$stale_path" >&2
      done
    fi
    actual_branch="$(git -C "$live_path" branch --show-current)"
    if test "$actual_branch" != "$control_branch"; then
      printf 'Control worktree branch mismatch: expected %s, got %s\n' "$control_branch" "$actual_branch" >&2
      exit 1
    fi
    live_status="$(git -C "$live_path" status --porcelain=v1 --untracked-files=all)"
    if test -n "$live_status"; then
      printf 'Blocked: live controller worktree is dirty and cannot be refreshed safely: %s\n' "$live_path" >&2
      git -C "$live_path" status --short --branch --untracked-files=all >&2
      exit 1
    fi
    live_head="$(git -C "$live_path" rev-parse HEAD)"
    if test "$live_head" = "$remote_head"; then
      :
    elif git -C "$repo" merge-base --is-ancestor "$live_head" "$remote_head"; then
      if ! git -C "$live_path" merge --ff-only "$remote_branch_ref"; then
        printf 'Blocked: live controller worktree could not fast-forward %s to fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
        exit 1
      fi
    elif git -C "$repo" merge-base --is-ancestor "$remote_head" "$live_head"; then
      printf 'Blocked: live controller branch %s is ahead of fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
      exit 1
    else
      printf 'Blocked: live controller branch %s diverged from fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
      exit 1
    fi
    live_head="$(git -C "$live_path" rev-parse HEAD)"
    if test "$live_head" != "$remote_head"; then
      printf 'Blocked: live controller HEAD %s does not match fetched origin tip %s.\n' "$live_head" "$remote_head" >&2
      exit 1
    fi
    printf '%s\n' "$live_path"
    return 0
  fi
  if test "$stale_count" -gt 1; then
    printf 'Blocked: %s has %s stale controller registrations and no live worktree; refusing to choose arbitrarily.\n' "$control_branch" "$stale_count" >&2
    for stale_path in "${stale_paths[@]}"; do
      printf '  stale: %s\n' "$stale_path" >&2
    done
    exit 1
  fi
  if ! git -C "$repo" check-ignore -q .worktrees/; then
    printf 'Repository-local .worktrees/ is not ignored.\n' >&2
    exit 1
  fi
  if ! git -C "$repo" check-ignore -q .worktrees/issue-541-recovery; then
    printf 'Repository-local control worktree path is not ignored: %s\n' '.worktrees/issue-541-recovery' >&2
    exit 1
  fi
  mkdir -p "$repo/.worktrees"
  if test -e "$target_path"; then
    printf 'Control worktree path already exists without a matching live registration: %s\n' "$target_path" >&2
    exit 1
  fi
  if git -C "$repo" show-ref --verify --quiet "$expected_branch"; then
    local_head="$(git -C "$repo" rev-parse --verify "$expected_branch^{commit}")"
    if test "$local_head" = "$remote_head"; then
      :
    elif git -C "$repo" merge-base --is-ancestor "$local_head" "$remote_head"; then
      if ! git -C "$repo" update-ref "$expected_branch" "$remote_head" "$local_head"; then
        printf 'Blocked: local controller branch %s could not be fast-forwarded to fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
        exit 1
      fi
    elif git -C "$repo" merge-base --is-ancestor "$remote_head" "$local_head"; then
      printf 'Blocked: local controller branch %s is ahead of fetched origin tip %s and cannot be rewritten safely.\n' "$control_branch" "$remote_head" >&2
      exit 1
    else
      printf 'Blocked: local controller branch %s diverged from fetched origin tip %s and cannot be rewritten safely.\n' "$control_branch" "$remote_head" >&2
      exit 1
    fi
  else
    if ! git -C "$repo" update-ref "$expected_branch" "$remote_head"; then
      printf 'Blocked: local controller branch %s could not be created at fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
      exit 1
    fi
  fi
  local_head="$(git -C "$repo" rev-parse --verify "$expected_branch^{commit}")"
  if test "$local_head" != "$remote_head"; then
    printf 'Blocked: local controller branch %s resolved to %s instead of fetched origin tip %s.\n' "$control_branch" "$local_head" "$remote_head" >&2
    exit 1
  fi
  if test "$stale_count" -eq 1; then
    stale_path="${stale_paths[0]}"
    printf 'Recreating %s at %s from the one stale controller registration:\n  stale: %s\n' "$control_branch" "$target_path" "$stale_path" >&2
    git -C "$repo" worktree add --force "$target_path" "$control_branch"
  else
    git -C "$repo" worktree add "$target_path" "$control_branch"
  fi
  if ! test -d "$target_path"; then
    printf 'Control worktree path is not present after resolution: %s\n' "$target_path" >&2
    exit 1
  fi
  actual_branch="$(git -C "$target_path" branch --show-current)"
  if test "$actual_branch" != "$control_branch"; then
    printf 'Control worktree branch mismatch: expected %s, got %s\n' "$control_branch" "$actual_branch" >&2
    exit 1
  fi
  local_head="$(git -C "$target_path" rev-parse HEAD)"
  if test "$local_head" != "$remote_head"; then
    printf 'Blocked: resolved controller HEAD %s does not match fetched origin tip %s.\n' "$local_head" "$remote_head" >&2
    exit 1
  fi
  printf '%s\n' "$target_path"
}
CONTROL_WORKTREE="$(resolve_control_worktree)"
COMMON_DIR="$(git -C "$CONTROL_WORKTREE" rev-parse --git-common-dir)"
REPO="$(cd "$COMMON_DIR/.." && pwd)"
cd "$REPO"
gh issue comment 541 --repo OpenCoven/coven --body-file \
  "$COMMON_DIR/agent-recovery/issue-541/classification.md"
```

Expected: issue #541 contains the durable GitHub-visible recovery ledger sourced
from the sanitized `classification.md`.

- [ ] **Step 7: Close issue #541 when all machine work is delivered**

Close only after every viable concern has an open PR and all local residue has
been safely preserved or cleaned:

```bash
set -euo pipefail
resolve_control_worktree() {
  local control_branch="docs/541-incomplete-work-recovery-design"
  local expected_branch="refs/heads/$control_branch"
  local remote_branch_ref="refs/remotes/origin/$control_branch"
  local start_common_dir repo target_path path branch_ref actual_branch
  local target_registration_branch live_count stale_count live_path stale_path
  local live_status live_head local_head remote_head
  local -a live_paths=() stale_paths=()
  start_common_dir="$(git rev-parse --git-common-dir)"
  repo="$(cd "$start_common_dir/.." && pwd)"
  target_path="$repo/.worktrees/issue-541-recovery"
  if ! git -C "$repo" fetch --no-tags origin \
    "refs/heads/$control_branch:$remote_branch_ref"
  then
    printf 'Blocked: could not fetch origin/%s into %s.\n' \
      "$control_branch" "$remote_branch_ref" >&2
    exit 1
  fi
  remote_head="$(git -C "$repo" rev-parse --verify "$remote_branch_ref^{commit}")"
  while IFS="$(printf "\t")" read -r path branch_ref; do
    test -n "$path" || continue
    if test "$path" = "$target_path"; then
      target_registration_branch="$branch_ref"
    fi
    if test "$branch_ref" != "$expected_branch"; then
      continue
    fi
    if test -d "$path"; then
      live_paths+=("$path")
    else
      stale_paths+=("$path")
    fi
  done <<EOF2
$(git -C "$repo" worktree list --porcelain | awk '
  $1 == "worktree" { if (path != "") print path "\t" branch; path = substr($0, 10); branch = ""; next }
  $1 == "branch" { branch = $2; next }
  END { if (path != "") print path "\t" branch }
')
EOF2
  live_count="${#live_paths[@]}"
  stale_count="${#stale_paths[@]}"
  if test -n "$target_registration_branch" && test "$target_registration_branch" != "$expected_branch"; then
    printf 'Blocked: %s is registered to %s, not %s.\n' "$target_path" "$target_registration_branch" "$control_branch" >&2
    exit 1
  fi
  if test "$live_count" -gt 1; then
    printf 'Blocked: %s has %s live controller worktree registrations; refusing to choose arbitrarily.\n' "$control_branch" "$live_count" >&2
    for live_path in "${live_paths[@]}"; do
      printf '  live: %s\n' "$live_path" >&2
    done
    for stale_path in "${stale_paths[@]}"; do
      printf '  stale: %s\n' "$stale_path" >&2
    done
    exit 1
  fi
  if test "$live_count" -eq 1; then
    live_path="${live_paths[0]}"
    if test "$stale_count" -gt 0; then
      printf 'Warning: ignoring %s stale controller registration(s) for %s while using the one live worktree.\n' "$stale_count" "$control_branch" >&2
      for stale_path in "${stale_paths[@]}"; do
        printf '  stale: %s\n' "$stale_path" >&2
      done
    fi
    actual_branch="$(git -C "$live_path" branch --show-current)"
    if test "$actual_branch" != "$control_branch"; then
      printf 'Control worktree branch mismatch: expected %s, got %s\n' "$control_branch" "$actual_branch" >&2
      exit 1
    fi
    live_status="$(git -C "$live_path" status --porcelain=v1 --untracked-files=all)"
    if test -n "$live_status"; then
      printf 'Blocked: live controller worktree is dirty and cannot be refreshed safely: %s\n' "$live_path" >&2
      git -C "$live_path" status --short --branch --untracked-files=all >&2
      exit 1
    fi
    live_head="$(git -C "$live_path" rev-parse HEAD)"
    if test "$live_head" = "$remote_head"; then
      :
    elif git -C "$repo" merge-base --is-ancestor "$live_head" "$remote_head"; then
      if ! git -C "$live_path" merge --ff-only "$remote_branch_ref"; then
        printf 'Blocked: live controller worktree could not fast-forward %s to fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
        exit 1
      fi
    elif git -C "$repo" merge-base --is-ancestor "$remote_head" "$live_head"; then
      printf 'Blocked: live controller branch %s is ahead of fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
      exit 1
    else
      printf 'Blocked: live controller branch %s diverged from fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
      exit 1
    fi
    live_head="$(git -C "$live_path" rev-parse HEAD)"
    if test "$live_head" != "$remote_head"; then
      printf 'Blocked: live controller HEAD %s does not match fetched origin tip %s.\n' "$live_head" "$remote_head" >&2
      exit 1
    fi
    printf '%s\n' "$live_path"
    return 0
  fi
  if test "$stale_count" -gt 1; then
    printf 'Blocked: %s has %s stale controller registrations and no live worktree; refusing to choose arbitrarily.\n' "$control_branch" "$stale_count" >&2
    for stale_path in "${stale_paths[@]}"; do
      printf '  stale: %s\n' "$stale_path" >&2
    done
    exit 1
  fi
  if ! git -C "$repo" check-ignore -q .worktrees/; then
    printf 'Repository-local .worktrees/ is not ignored.\n' >&2
    exit 1
  fi
  if ! git -C "$repo" check-ignore -q .worktrees/issue-541-recovery; then
    printf 'Repository-local control worktree path is not ignored: %s\n' '.worktrees/issue-541-recovery' >&2
    exit 1
  fi
  mkdir -p "$repo/.worktrees"
  if test -e "$target_path"; then
    printf 'Control worktree path already exists without a matching live registration: %s\n' "$target_path" >&2
    exit 1
  fi
  if git -C "$repo" show-ref --verify --quiet "$expected_branch"; then
    local_head="$(git -C "$repo" rev-parse --verify "$expected_branch^{commit}")"
    if test "$local_head" = "$remote_head"; then
      :
    elif git -C "$repo" merge-base --is-ancestor "$local_head" "$remote_head"; then
      if ! git -C "$repo" update-ref "$expected_branch" "$remote_head" "$local_head"; then
        printf 'Blocked: local controller branch %s could not be fast-forwarded to fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
        exit 1
      fi
    elif git -C "$repo" merge-base --is-ancestor "$remote_head" "$local_head"; then
      printf 'Blocked: local controller branch %s is ahead of fetched origin tip %s and cannot be rewritten safely.\n' "$control_branch" "$remote_head" >&2
      exit 1
    else
      printf 'Blocked: local controller branch %s diverged from fetched origin tip %s and cannot be rewritten safely.\n' "$control_branch" "$remote_head" >&2
      exit 1
    fi
  else
    if ! git -C "$repo" update-ref "$expected_branch" "$remote_head"; then
      printf 'Blocked: local controller branch %s could not be created at fetched origin tip %s.\n' "$control_branch" "$remote_head" >&2
      exit 1
    fi
  fi
  local_head="$(git -C "$repo" rev-parse --verify "$expected_branch^{commit}")"
  if test "$local_head" != "$remote_head"; then
    printf 'Blocked: local controller branch %s resolved to %s instead of fetched origin tip %s.\n' "$control_branch" "$local_head" "$remote_head" >&2
    exit 1
  fi
  if test "$stale_count" -eq 1; then
    stale_path="${stale_paths[0]}"
    printf 'Recreating %s at %s from the one stale controller registration:\n  stale: %s\n' "$control_branch" "$target_path" "$stale_path" >&2
    git -C "$repo" worktree add --force "$target_path" "$control_branch"
  else
    git -C "$repo" worktree add "$target_path" "$control_branch"
  fi
  if ! test -d "$target_path"; then
    printf 'Control worktree path is not present after resolution: %s\n' "$target_path" >&2
    exit 1
  fi
  actual_branch="$(git -C "$target_path" branch --show-current)"
  if test "$actual_branch" != "$control_branch"; then
    printf 'Control worktree branch mismatch: expected %s, got %s\n' "$control_branch" "$actual_branch" >&2
    exit 1
  fi
  local_head="$(git -C "$target_path" rev-parse HEAD)"
  if test "$local_head" != "$remote_head"; then
    printf 'Blocked: resolved controller HEAD %s does not match fetched origin tip %s.\n' "$local_head" "$remote_head" >&2
    exit 1
  fi
  printf '%s\n' "$target_path"
}
CONTROL_WORKTREE="$(resolve_control_worktree)"
COMMON_DIR="$(git -C "$CONTROL_WORKTREE" rev-parse --git-common-dir)"
REPO="$(cd "$COMMON_DIR/.." && pwd)"
cd "$REPO"
gh issue close 541 --repo OpenCoven/coven --comment \
  "Recovery inventory is complete. Every viable concern has a scoped open PR; non-viable work has evidence and durable snapshots; local goals and Git hygiene are reconciled."
```

Expected: #541 is closed while child implementation PRs continue through normal
review and merge.
