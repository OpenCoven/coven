# Incomplete Work Recovery Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Preserve and classify every discovered local workstream, recover each viable concern through its own issue and implementation plan, and safely remove only proven stale residue.

**Architecture:** Use the repository's shared Git common directory as a non-worktree recovery archive. Run a preservation and classification phase before any cleanup, then hand each viable subsystem to an isolated issue/spec/plan/PR flow based on current `origin/main`.

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
  - `.git/agent-recovery/issue-541/viable/<workstream-id>-issue-search.json`
  - `.git/agent-recovery/issue-541/viable/<workstream-id>-issue-view.json`
  - `.git/agent-recovery/issue-541/viable/<workstream-id>-pr-blocker.txt`
  - `.git/agent-recovery/issue-541/viable/<workstream-id>-issue-blocker.txt`
  - `.git/agent-recovery/issue-541/private/issue-ledger-refresh/<workstream-id>-issues.stage.json`
  - Authoritative source-branch fetch evidence, open-PR evidence, verified
    PR-view evidence, preserved-head adoption ancestry evidence,
    per-workstream exact-title open issue-match evidence derived from paginated
    `issues.json`, blocker evidence for each viable workstream, and the
    private staged ledger used to atomically refresh `issues.json`.
- Create only when Task 8 Step 5 runs:
  - `.git/agent-recovery/issue-541/private/branch-delete-proof/<branch-proof-id>-pre-delete-d.txt`
  - `.git/agent-recovery/issue-541/private/branch-delete-proof/<branch-proof-id>-pre-delete-D.txt`
  - Private branch-ref recheck evidence for each recovered source branch,
    including missing-ref outcomes and drift blockers recorded immediately
    before each deletion command.
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
  - Status, commit identifiers, exact branch-name marker, verified
    `branch.bundle`, binary patches, and copied untracked files for each source
    worktree, so committed branch history is preserved before any later source
    branch deletion. Empty patches remain valid evidence when a formerly dirty
    worktree is now clean because its changes were committed to an active PR.
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
    known. Keep this as a local untracked file.
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

- [ ] **Step 1: Check claim state, check open pull requests, acquire `issue-541`, and confirm the design worktree state**

Run:

```bash
cd /tmp/coven-issue-541
coven claim status
gh pr list --repo OpenCoven/coven --state open --limit 100
coven claim acquire issue-541
git status --short --branch
```

Expected: the shared claim registry and open PR set are reviewed before any
publication or recovery action; `issue-541` is actively claimed from
`/tmp/coven-issue-541`; and branch
`docs/541-incomplete-work-recovery-design` shows only the plan file untracked
before it is staged.

If this publication session or the later recovery session runs long, keep the
parent claim alive from `/tmp/coven-issue-541`:

```bash
cd /tmp/coven-issue-541
coven claim heartbeat issue-541
```

Do not release `issue-541` after PR creation. Keep it active until this
recovery session stops or the full issue #541 recovery effort is complete, then
release it from `/tmp/coven-issue-541` with `coven claim release issue-541`.

- [ ] **Step 2: Run document safety checks**

Run:

```bash
cd /tmp/coven-issue-541
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
cd /tmp/coven-issue-541
COPILOT_GH_ID=223556219
COPILOT_GH_USER=Copilot
COPILOT_NOREPLY_DOMAIN=users.noreply.github.com
COPILOT_TRAILER="Co-authored-by: $COPILOT_GH_USER <${COPILOT_GH_ID}+${COPILOT_GH_USER}@${COPILOT_NOREPLY_DOMAIN}>"
git commit -s --trailer "$COPILOT_TRAILER" \
  -m "docs: plan incomplete work recovery"
```

Expected: a commit containing only the implementation plan and the
repository-required DCO sign-off plus the session-required Copilot co-author
trailer. Human contributor co-author trailers remain conditional under
`AGENTS.md` and, when required, are added separately with additional
`--trailer` arguments rather than replacing the required Copilot trailer.

- [ ] **Step 4: Push the design branch**

Run:

```bash
git -C /tmp/coven-issue-541 push -u origin docs/541-incomplete-work-recovery-design
```

Expected: the remote branch is created successfully.

- [ ] **Step 5: Open the design pull request**

Run:

```bash
cd /tmp/coven-issue-541
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
COMMON_DIR="$(git -C /tmp/coven-issue-541 rev-parse --git-common-dir)"
REPO="$(cd "$COMMON_DIR/.." && pwd)"
git -C "$REPO" fetch origin main
RECOVERY="$COMMON_DIR/agent-recovery/issue-541"
mkdir -p "$RECOVERY/dirty" "$RECOVERY/branches"
printf 'id\ttype\tsource\thead\tmerge_base\tsnapshot\n' > "$RECOVERY/manifest.tsv"
```

Expected: the recovery root exists under
`$COMMON_DIR/agent-recovery/issue-541`, and that fetched `origin/main`
becomes the baseline for every Task 2 and Task 3 `merge_base` record. Task 4
may fetch `origin/main` again before classification.

- [ ] **Step 2: Snapshot `docs-psyche-specs`**

Run:

```bash
COMMON_DIR="$(git -C /tmp/coven-issue-541 rev-parse --git-common-dir)"
REPO="$(cd "$COMMON_DIR/.." && pwd)"
SOURCE="$REPO/.worktrees/docs-psyche-specs"
DEST="$COMMON_DIR/agent-recovery/issue-541/dirty/docs-psyche-specs"
BRANCH="$(git -C "$SOURCE" branch --show-current)"
mkdir -p "$DEST/untracked"
git -C "$SOURCE" status --short --branch > "$DEST/status.txt"
printf '%s\n' "$BRANCH" > "$DEST/branch.txt"
git -C "$SOURCE" rev-parse HEAD > "$DEST/head.txt"
git -C "$SOURCE" merge-base HEAD origin/main > "$DEST/merge-base.txt"
git -C "$REPO" bundle create "$DEST/branch.bundle" "$BRANCH"
git -C "$REPO" bundle verify "$DEST/branch.bundle"
git -C "$SOURCE" diff --binary > "$DEST/worktree.patch"
git -C "$SOURCE" diff --cached --binary > "$DEST/index.patch"
git -C "$SOURCE" ls-files --others --exclude-standard > "$DEST/untracked-files.txt"
while IFS= read -r path; do
  test -n "$path" || continue
  mkdir -p "$DEST/untracked/$(dirname "$path")"
  cp -p "$SOURCE/$path" "$DEST/untracked/$path"
done < "$DEST/untracked-files.txt"
printf 'docs-psyche-specs\tdirty-worktree\t%s\t%s\t%s\t%s\n' \
  "$SOURCE" \
  "$(cat "$DEST/head.txt")" \
  "$(cat "$DEST/merge-base.txt")" \
  "$DEST" >> "$COMMON_DIR/agent-recovery/issue-541/manifest.tsv"
```

Expected: `branch.bundle` verifies against `docs/psyche-specs`, preserving its
committed history before any later branch deletion; both patch files exist; and
any copied untracked files match only the paths listed in
`untracked-files.txt`. If the worktree is still dirty, the patch files capture
those changes. If the worktree is now clean because the source branch already
backs an active PR, the patch files may be empty and `status.txt` becomes the
evidence of that clean post-commit state.

- [ ] **Step 3: Snapshot `feat-cmem-1ev-memory-promote`**

Run:

```bash
COMMON_DIR="$(git -C /tmp/coven-issue-541 rev-parse --git-common-dir)"
REPO="$(cd "$COMMON_DIR/.." && pwd)"
SOURCE="$REPO/.worktrees/feat-cmem-1ev-memory-promote"
DEST="$COMMON_DIR/agent-recovery/issue-541/dirty/memory-promote"
BRANCH="$(git -C "$SOURCE" branch --show-current)"
mkdir -p "$DEST/untracked"
git -C "$SOURCE" status --short --branch > "$DEST/status.txt"
printf '%s\n' "$BRANCH" > "$DEST/branch.txt"
git -C "$SOURCE" rev-parse HEAD > "$DEST/head.txt"
git -C "$SOURCE" merge-base HEAD origin/main > "$DEST/merge-base.txt"
git -C "$REPO" bundle create "$DEST/branch.bundle" "$BRANCH"
git -C "$REPO" bundle verify "$DEST/branch.bundle"
git -C "$SOURCE" diff --binary > "$DEST/worktree.patch"
git -C "$SOURCE" diff --cached --binary > "$DEST/index.patch"
git -C "$SOURCE" ls-files --others --exclude-standard > "$DEST/untracked-files.txt"
while IFS= read -r path; do
  test -n "$path" || continue
  mkdir -p "$DEST/untracked/$(dirname "$path")"
  cp -p "$SOURCE/$path" "$DEST/untracked/$path"
done < "$DEST/untracked-files.txt"
```

```bash
printf 'memory-promote\tdirty-worktree\t%s\t%s\t%s\t%s\n' \
  "$SOURCE" \
  "$(cat "$DEST/head.txt")" \
  "$(cat "$DEST/merge-base.txt")" \
  "$DEST" >> "$COMMON_DIR/agent-recovery/issue-541/manifest.tsv"
```

Expected: `branch.bundle` verifies against
`feat/cmem-1ev-memory-promote`, preserving committed branch history before any
later branch deletion, and the untracked tree includes
`crates/coven-memory/src/promotion.rs`, `scripts/check-coven-privacy.py`, and
`scripts/check-coven-privacy-test.py`.

- [ ] **Step 4: Snapshot `feat/mobile-memory-gateway`**

Run:

```bash
COMMON_DIR="$(git -C /tmp/coven-issue-541 rev-parse --git-common-dir)"
REPO="$(cd "$COMMON_DIR/.." && pwd)"
SOURCE="$REPO/.worktrees/mobile-memory-gateway"
DEST="$COMMON_DIR/agent-recovery/issue-541/dirty/mobile-memory-gateway"
BRANCH="$(git -C "$SOURCE" branch --show-current)"
mkdir -p "$DEST/untracked"
git -C "$SOURCE" status --short --branch > "$DEST/status.txt"
printf '%s\n' "$BRANCH" > "$DEST/branch.txt"
git -C "$SOURCE" rev-parse HEAD > "$DEST/head.txt"
git -C "$SOURCE" merge-base HEAD origin/main > "$DEST/merge-base.txt"
git -C "$REPO" bundle create "$DEST/branch.bundle" "$BRANCH"
git -C "$REPO" bundle verify "$DEST/branch.bundle"
git -C "$SOURCE" diff --binary > "$DEST/worktree.patch"
git -C "$SOURCE" diff --cached --binary > "$DEST/index.patch"
git -C "$SOURCE" ls-files --others --exclude-standard > "$DEST/untracked-files.txt"
while IFS= read -r path; do
  test -n "$path" || continue
  mkdir -p "$DEST/untracked/$(dirname "$path")"
  cp -p "$SOURCE/$path" "$DEST/untracked/$path"
done < "$DEST/untracked-files.txt"
```

```bash
printf 'mobile-memory-gateway\tdirty-worktree\t%s\t%s\t%s\t%s\n' \
  "$SOURCE" \
  "$(cat "$DEST/head.txt")" \
  "$(cat "$DEST/merge-base.txt")" \
  "$DEST" >> "$COMMON_DIR/agent-recovery/issue-541/manifest.tsv"
```

Expected: `branch.bundle` verifies against `feat/mobile-memory-gateway`,
preserving committed branch history before any later branch deletion, and
`worktree.patch` contains the changes to
`crates/coven-cli/src/mobile_memory/pairing.rs`.

- [ ] **Step 5: Snapshot `fix/476-review-threads`**

Run:

```bash
COMMON_DIR="$(git -C /tmp/coven-issue-541 rev-parse --git-common-dir)"
REPO="$(cd "$COMMON_DIR/.." && pwd)"
SOURCE="$REPO/.worktrees/pr-476-review"
DEST="$COMMON_DIR/agent-recovery/issue-541/dirty/pr-476-review"
BRANCH="$(git -C "$SOURCE" branch --show-current)"
mkdir -p "$DEST/untracked"
git -C "$SOURCE" status --short --branch > "$DEST/status.txt"
printf '%s\n' "$BRANCH" > "$DEST/branch.txt"
git -C "$SOURCE" rev-parse HEAD > "$DEST/head.txt"
git -C "$SOURCE" merge-base HEAD origin/main > "$DEST/merge-base.txt"
git -C "$REPO" bundle create "$DEST/branch.bundle" "$BRANCH"
git -C "$REPO" bundle verify "$DEST/branch.bundle"
git -C "$SOURCE" diff --binary > "$DEST/worktree.patch"
git -C "$SOURCE" diff --cached --binary > "$DEST/index.patch"
git -C "$SOURCE" ls-files --others --exclude-standard > "$DEST/untracked-files.txt"
while IFS= read -r path; do
  test -n "$path" || continue
  mkdir -p "$DEST/untracked/$(dirname "$path")"
  cp -p "$SOURCE/$path" "$DEST/untracked/$path"
done < "$DEST/untracked-files.txt"
```

```bash
printf 'pr-476-review\tdirty-worktree\t%s\t%s\t%s\t%s\n' \
  "$SOURCE" \
  "$(cat "$DEST/head.txt")" \
  "$(cat "$DEST/merge-base.txt")" \
  "$DEST" >> "$COMMON_DIR/agent-recovery/issue-541/manifest.tsv"
```

Expected: `branch.bundle` verifies against `fix/476-review-threads`,
preserving committed branch history before any later branch deletion, and the
untracked tree contains all three runtime parity plan files.

- [ ] **Step 6: Verify snapshot completeness**

Run:

```bash
COMMON_DIR="$(git -C /tmp/coven-issue-541 rev-parse --git-common-dir)"
REPO="$(cd "$COMMON_DIR/.." && pwd)"
RECOVERY="$COMMON_DIR/agent-recovery/issue-541"
test "$(wc -l < "$RECOVERY/manifest.tsv" | tr -d ' ')" = 5
for id in docs-psyche-specs memory-promote mobile-memory-gateway pr-476-review; do
  SNAPSHOT="$RECOVERY/dirty/$id"
  test -s "$SNAPSHOT/status.txt"
  test -s "$SNAPSHOT/branch.txt"
  test -s "$SNAPSHOT/head.txt"
  test -s "$SNAPSHOT/merge-base.txt"
  test -s "$SNAPSHOT/branch.bundle"
  test -f "$SNAPSHOT/worktree.patch"
  test -f "$SNAPSHOT/index.patch"
  test -f "$SNAPSHOT/untracked-files.txt"
  while IFS= read -r path; do
    test -n "$path" || continue
    test -e "$SNAPSHOT/untracked/$path"
  done < "$SNAPSHOT/untracked-files.txt"
  git -C "$REPO" bundle verify "$SNAPSHOT/branch.bundle" > /dev/null
done
```

Expected: every dirty snapshot includes status, branch, head, merge-base, both
patches, an explicit untracked inventory, and a verified `branch.bundle`, so
committed branch history is preserved before any later branch deletion. Empty
worktree, index, and untracked artifact classes are valid and appear as
existing zero-byte files; non-empty untracked inventories still verify every
copied path, and an empty inventory passes.

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
COMMON_DIR="$(git -C /tmp/coven-issue-541 rev-parse --git-common-dir)"
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
COMMON_DIR="$(git -C /tmp/coven-issue-541 rev-parse --git-common-dir)"
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
COMMON_DIR="$(git -C /tmp/coven-issue-541 rev-parse --git-common-dir)"
REPO="$(cd "$COMMON_DIR/.." && pwd)"
RECOVERY="$COMMON_DIR/agent-recovery/issue-541"
for entry in \
  'docs-universal-runtime-capability-design|docs/universal-runtime-capability-design' \
  'feat-npm-macos-x64|feat/npm-macos-x64' \
  'fix-521-ward-surface-confinement|fix/521-ward-surface-confinement'
do
  id=${entry%%|*}
  branch=${entry#*|}
  mkdir -p "$RECOVERY/branches/$id"
  printf '%s\n' "$branch" > "$RECOVERY/branches/$id/branch.txt"
  git -C "$REPO" rev-parse "$branch" > "$RECOVERY/branches/$id/head.txt"
  git -C "$REPO" merge-base "$branch" origin/main \
    > "$RECOVERY/branches/$id/merge-base.txt"
  printf '%s\torphan-branch\t%s\t%s\t%s\t%s\n' \
    "$id" \
    "$branch" \
    "$(cat "$RECOVERY/branches/$id/head.txt")" \
    "$(cat "$RECOVERY/branches/$id/merge-base.txt")" \
    "$RECOVERY/branches/$id.bundle" >> "$RECOVERY/manifest.tsv"
done
```

Expected: `manifest.tsv` has seven data rows plus its header, and each orphan
branch now has immutable `branch.txt`, `head.txt`, and `merge-base.txt`
metadata beside its bundle.
Every Task 3 `merge_base` value still uses the `origin/main` fetched in Task 2
Step 1.

### Task 4: Build the Classification Ledger

**Artifacts:**
- Create: `.git/agent-recovery/issue-541/classification.md`

- [ ] **Step 1: Capture branch and GitHub evidence**

Run:

```bash
COMMON_DIR="$(git -C /tmp/coven-issue-541 rev-parse --git-common-dir)"
REPO="$(cd "$COMMON_DIR/.." && pwd)"
RECOVERY="$COMMON_DIR/agent-recovery/issue-541"
git -C "$REPO" fetch origin main
git -C "$REPO" --no-pager branch -vv > "$RECOVERY/branches.txt"
git -C "$REPO" worktree list --porcelain > "$RECOVERY/worktrees.txt"
cd "$REPO"
coven claim status > "$RECOVERY/claims.txt"
gh api --paginate --slurp \
  "repos/OpenCoven/coven/pulls?state=all&per_page=100" \
  > "$RECOVERY/pulls.json"
gh api --paginate --slurp \
  "repos/OpenCoven/coven/issues?state=all&per_page=100" \
  > "$RECOVERY/issues.json"
```

Expected: all five evidence files exist and are non-empty, and both
`pulls.json` and `issues.json` are valid paginated GitHub API JSON captures.
Because the issues API returns both issues and pull requests, PR-specific
classification must use `pulls.json` as the dedicated pull-request evidence
source and treat `issues.json` as the broader issue-history ledger and the
authoritative fully paginated source for later exact-title issue-reuse filters.

- [ ] **Step 2: Compare each source with current main**

Run:

```bash
COMMON_DIR="$(git -C /tmp/coven-issue-541 rev-parse --git-common-dir)"
REPO="$(cd "$COMMON_DIR/.." && pwd)"
RECOVERY="$COMMON_DIR/agent-recovery/issue-541"
for branch in \
  docs/psyche-specs \
  docs/universal-runtime-capability-design \
  feat/cmem-1ev-memory-promote \
  feat/mobile-memory-gateway \
  feat/npm-macos-x64 \
  fix/476-review-threads \
  fix/521-ward-surface-confinement
do
  id=$(printf '%s' "$branch" | tr '/' '-')
  git -C "$REPO" --no-pager log --oneline --reverse \
    "origin/main..$branch" > "$RECOVERY/$id-commits.txt"
  git -C "$REPO" --no-pager diff --stat \
    "origin/main...$branch" > "$RECOVERY/$id-stat.txt"
  git -C "$REPO" cherry -v origin/main "$branch" \
    > "$RECOVERY/$id-cherry.txt"
done
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
  if test -s "$SNAPSHOT/untracked-files.txt"; then
    cp "$SNAPSHOT/untracked-files.txt" "$RECOVERY/$id-untracked-evidence.txt"
  else
    printf 'No untracked files in snapshot.\n' \
      > "$RECOVERY/$id-untracked-evidence.txt"
  fi
  test -f "$RECOVERY/$id-untracked-evidence.txt"
  test -s "$RECOVERY/$id-untracked-evidence.txt"
done
```

Expected: the seven historical branch comparisons still provide complementary
commit, stat, and cherry evidence, and the four dirty snapshots now each have
reviewable worktree, index, and inventory-backed untracked evidence files, so
branch evidence covers all seven branch-backed workstreams and dirty evidence
complements the four dirty rows. Empty worktree, index, and untracked snapshot
classes remain valid, but each generated evidence file is still non-empty
because it contains either `git apply --stat --summary` output or a
deterministic sentinel line documenting that the snapshot class was empty.

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

- [ ] **Step 4: Apply the classification rules**

Use these deterministic rules:

```text
already-shipped:
  Current main contains equivalent behavior or documentation, with a merged PR
  or direct file/commit evidence.

superseded:
  A later merged change intentionally replaces the same contract, and applying
  the old work would regress or duplicate it.

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
one next action before Task 4 Step 5 and Task 9 Step 1 verification. A viable
row may later keep that classification while Task 5 updates its action to
`continue existing PR` only after an exact-head query returns one open PR from
the source branch and that single candidate passes the fetched-tip and
preserved-head ancestry checks. Rows with zero exact-head candidates keep the
normal issue-reuse-or-create flow.

- [ ] **Step 5: Review the ledger against the manifest**

Run:

```bash
COMMON_DIR="$(git -C /tmp/coven-issue-541 rev-parse --git-common-dir)"
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
COMMON_DIR="$(git -C /tmp/coven-issue-541 rev-parse --git-common-dir)"
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
    ;;
  feat-npm-macos-x64)
    SOURCE_BRANCH="feat/npm-macos-x64"
    SOURCE_HEAD_FILE="$COMMON_DIR/agent-recovery/issue-541/branches/feat-npm-macos-x64/head.txt"
    ;;
  fix-521-ward-surface-confinement)
    SOURCE_BRANCH="fix/521-ward-surface-confinement"
    SOURCE_HEAD_FILE="$COMMON_DIR/agent-recovery/issue-541/branches/fix-521-ward-surface-confinement/head.txt"
    ;;
  memory-promote)
    SOURCE_BRANCH="feat/cmem-1ev-memory-promote"
    SOURCE_HEAD_FILE="$COMMON_DIR/agent-recovery/issue-541/dirty/memory-promote/head.txt"
    ;;
  docs-psyche-specs)
    SOURCE_BRANCH="docs/psyche-specs"
    SOURCE_HEAD_FILE="$COMMON_DIR/agent-recovery/issue-541/dirty/docs-psyche-specs/head.txt"
    ;;
  docs-universal-runtime-capability-design)
    SOURCE_BRANCH="docs/universal-runtime-capability-design"
    SOURCE_HEAD_FILE="$COMMON_DIR/agent-recovery/issue-541/branches/docs-universal-runtime-capability-design/head.txt"
    ;;
  pr-476-review)
    SOURCE_BRANCH="fix/476-review-threads"
    SOURCE_HEAD_FILE="$COMMON_DIR/agent-recovery/issue-541/dirty/pr-476-review/head.txt"
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
test -s "$SOURCE_HEAD_FILE"
EXPECTED_HEAD="$(tr -d '\n' < "$SOURCE_HEAD_FILE")"
{
  printf 'Source branch: %s\n' "$SOURCE_BRANCH"
  printf 'Preserved local head: %s\n' "$EXPECTED_HEAD"
  printf 'Exact-head PR query runs before any source-branch fetch.\n'
} > "$BRANCH_FETCH_EVIDENCE"
gh pr list \
  --repo OpenCoven/coven \
  --state open \
  --head "$SOURCE_BRANCH" \
  --json number,title,url,headRefName > "$OPEN_PR_EVIDENCE"
PR_COUNT="$(jq 'length' "$OPEN_PR_EVIDENCE")"
```

Then branch on the exact-source-branch result:

```bash
case "$PR_COUNT" in
  1)
    printf 'Exact-head open PR count: 1\n' >> "$BRANCH_FETCH_EVIDENCE"
    printf 'Fetching origin/%s for candidate identity verification.\n' \
      "$SOURCE_BRANCH" >> "$BRANCH_FETCH_EVIDENCE"
    if ! git -C "$REPO" fetch --no-tags origin \
      "refs/heads/$SOURCE_BRANCH:refs/remotes/origin/$SOURCE_BRANCH" \
      >> "$BRANCH_FETCH_EVIDENCE" 2>&1; then
      FETCH_OUTPUT="$(cat "$BRANCH_FETCH_EVIDENCE")"
      {
        printf 'Blocked %s: exact-head candidate exists, but origin/%s could not be fetched for identity verification.\n' \
          "$WORKSTREAM_ID" "$SOURCE_BRANCH"
        printf 'Preserved local head: %s\n' "$EXPECTED_HEAD"
        printf 'Branch fetch evidence: viable/%s-branch-fetch.txt\n' "$WORKSTREAM_ID"
        printf 'Fetch output follows:\n%s\n' "$FETCH_OUTPUT"
      } > "$PR_BLOCKER_EVIDENCE"
      update_classification_row \
        "blocked" \
        "Single PR candidate could not be verified because origin/$SOURCE_BRANCH fetch failed for preserved head $EXPECTED_HEAD; see viable/$WORKSTREAM_ID-branch-fetch.txt and viable/$WORKSTREAM_ID-pr-blocker.txt." \
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
        printf 'Blocked %s: candidate PR #%s disappeared or could not be read between gh pr list and gh pr view.\n' \
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
    update_classification_row \
      "viable" \
      "Preserved head $EXPECTED_HEAD is ancestor of PR head $ACTUAL_HEAD, which matches fetched origin/$SOURCE_BRANCH tip $FRESH_BRANCH_TIP; see viable/$WORKSTREAM_ID-branch-fetch.txt, viable/$WORKSTREAM_ID-open-prs.json, viable/$WORKSTREAM_ID-pr-view.json, and viable/$WORKSTREAM_ID-pr-adoption.txt." \
      "continue existing PR #$PR_NUMBER ($PR_URL)"
    exit 0
    ;;
  0)
    printf 'Exact-head open PR count: 0\n' >> "$BRANCH_FETCH_EVIDENCE"
    printf 'No exact-head open PR candidate; skipping source-branch fetch and continuing to issue reuse/create.\n' >> "$BRANCH_FETCH_EVIDENCE"
    ;;
  *)
    {
      printf 'Blocked %s: expected 0 or 1 open PRs for source branch %s, found %s before identity verification.\n' \
        "$WORKSTREAM_ID" "$SOURCE_BRANCH" "$PR_COUNT"
      printf 'Preserved local head: %s\n' "$EXPECTED_HEAD"
      printf 'No source-branch fetch was attempted because the exact-head query was already ambiguous.\n'
      printf 'Branch fetch evidence: viable/%s-branch-fetch.txt\n' "$WORKSTREAM_ID"
      printf 'Open PR evidence: viable/%s-open-prs.json\n' "$WORKSTREAM_ID"
    } > "$PR_BLOCKER_EVIDENCE"
    update_classification_row \
      "blocked" \
      "Open PR ambiguity for preserved head $EXPECTED_HEAD before any source-branch verification; see viable/$WORKSTREAM_ID-branch-fetch.txt, viable/$WORKSTREAM_ID-open-prs.json, and viable/$WORKSTREAM_ID-pr-blocker.txt." \
      "Blocked: multiple open PRs claim $SOURCE_BRANCH."
    exit 0
    ;;
esac
```

If `PR_COUNT=0`, do not fetch the source branch; record in
`viable/$WORKSTREAM_ID-branch-fetch.txt` that the exact-head query found no
candidate and continue directly to Step 2's issue reuse or creation flow. If
`PR_COUNT=1`, verify the candidate PR before adopting it: fetch the exact
expected source branch from `origin`, record the freshly fetched tip in
`viable/$WORKSTREAM_ID-branch-fetch.txt`, then require `.state=OPEN`,
`headRefName=$SOURCE_BRANCH`, `headRepositoryOwner.login=OpenCoven`,
`isCrossRepository=false`, `baseRefName=main`, and `headRefOid` equal to that
freshly fetched branch tip. Only after the PR head matches the authoritative
fetched tip may the plan compare the preserved local `head.txt` to the PR head
with `git merge-base --is-ancestor`; equality or ancestry is acceptable, but a
diverged local head blocks adoption. Record the preserved local head, fresh
branch tip, PR head, and ancestry result in
`viable/$WORKSTREAM_ID-pr-adoption.txt`, and record `continue existing PR`
only after all of those checks pass. If `PR_COUNT>1`, block immediately with
open-PR evidence because the exact-head query is already ambiguous. If
`gh pr view` fails because the candidate disappears or cannot be read after
`gh pr list`, if the single-candidate branch fetch fails, or if any other
identity or ancestry check fails, write
`viable/$WORKSTREAM_ID-pr-blocker.txt`, update the classification row to
`blocked`, and stop that row only after the blocker evidence is persisted.
This deterministically avoids duplicating possibly delivered work while still
allowing a clean local worktree to adopt its open PR after that PR has advanced
beyond the preserved local snapshot. A later rerun must reclassify against
current main and GitHub history before deciding whether any replacement issue
or PR is still needed. Continue to Step 2 only when `PR_COUNT=0`.

Expected: every viable row records the exact-head open-PR query before any
issue reuse or creation begins. Rows with one candidate add an evidence-backed
authoritative-source-branch fetch and a `main`-targeting PR decision; rows
with zero candidates skip fetch and continue normally; rows with multiple
candidates block before branch verification. If `docs/psyche-specs` still has
exactly one open PR from branch `docs/psyche-specs` at execution time, its
`headRefOid` matches the freshly fetched `origin/docs/psyche-specs` tip, and
the preserved local `head.txt` is equal to or an ancestor of that PR head,
this step adopts the exact-source-branch PR that GitHub returns at that moment
rather than hardcoding a PR number.

- [ ] **Step 2: Reuse or create one issue per viable row that does not already have an adopted PR**

Set `WORKSTREAM_ID` to the viable row's exact workstream ID, then use the
matching exact issue title:

```bash
COMMON_DIR="$(git -C /tmp/coven-issue-541 rev-parse --git-common-dir)"
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
    ;;
  feat-npm-macos-x64)
    ISSUE_TITLE="Recover Intel macOS npm packaging workstream"
    ;;
  fix-521-ward-surface-confinement)
    ISSUE_TITLE="Recover Ward surface confinement workstream"
    ;;
  memory-promote)
    ISSUE_TITLE="Recover memory promotion workstream"
    ;;
  docs-psyche-specs)
    ISSUE_TITLE="Recover Psyche specification workstream"
    ;;
  docs-universal-runtime-capability-design)
    ISSUE_TITLE="Recover universal runtime capability design workstream"
    ;;
  pr-476-review)
    ISSUE_TITLE="Recover runtime model parity plan workstream"
    ;;
  *)
    printf 'Unknown WORKSTREAM_ID: %s\n' "$WORKSTREAM_ID" >&2
    exit 1
    ;;
esac
ISSUE_SEARCH_EVIDENCE="$RECOVERY/$WORKSTREAM_ID-issue-search.json"
ISSUE_VIEW_EVIDENCE="$RECOVERY/$WORKSTREAM_ID-issue-view.json"
ISSUE_BLOCKER_EVIDENCE="$RECOVERY/$WORKSTREAM_ID-issue-blocker.txt"
ISSUE_LEDGER_EVIDENCE="$RECOVERY/$WORKSTREAM_ID-issues.json"
ISSUE_LEDGER_STAGE="$PRIVATE_RECOVERY/issue-ledger-refresh/$WORKSTREAM_ID-issues.stage.json"
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
```

Immediately before every exact-title reuse/create decision, refresh the fully
paginated shared `issue-541/issues.json` ledger into
`private/issue-ledger-refresh/$WORKSTREAM_ID-issues.stage.json`. If `gh api`
fails, preserve the existing `issue-541/issues.json`, remove the staging file,
persist blocker evidence, and stop that row. Only a non-empty JSON value that
passes `jq -e 'type == "array" and length > 0 and all(.[]; type == "array")'` may replace the
shared ledger, and that replacement must happen atomically with `mv`. Only
after the verified shared ledger is in place may the plan copy it to
`viable/$WORKSTREAM_ID-issues.json`, filter exact-title non-PR `open` matches,
and count them. Each workstream's `viable/$WORKSTREAM_ID-issue-search.json`
stores only the filtered matches from that verified persisted ledger, so
default-limited `gh issue list` output never drives reuse, and a refresh
failure never truncates the last known-good shared ledger. A sole unrelated or
closed result in the refreshed ledger leaves `ISSUE_COUNT=0` and must not be
reused.

If exactly one exact-title open issue already exists, reuse it without creating
a duplicate:

```bash
case "$ISSUE_COUNT" in
  1)
    ISSUE_NUMBER="$(jq -r '.[0].number' "$ISSUE_SEARCH_EVIDENCE")"
    gh issue view --repo OpenCoven/coven "$ISSUE_NUMBER" \
      --json number,state,title,url > "$ISSUE_VIEW_EVIDENCE"
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
```

If zero issues match, create one and verify it immediately:

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

```bash
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
    ISSUE_URL="$(gh issue create --repo OpenCoven/coven --title "$ISSUE_TITLE" --body "$ISSUE_BODY")"
    ISSUE_NUMBER="${ISSUE_URL##*/}"
    gh issue view --repo OpenCoven/coven "$ISSUE_NUMBER" \
      --json number,state,title,url > "$ISSUE_VIEW_EVIDENCE"
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
update_classification_row \
  "viable" \
  "Issue verified via issue-541/issues.json, viable/$WORKSTREAM_ID-issues.json, viable/$WORKSTREAM_ID-issue-search.json, and viable/$WORKSTREAM_ID-issue-view.json." \
  "Recover via issue #$ISSUE_NUMBER ($ISSUE_URL)."
```

If `ISSUE_COUNT>1`, write `viable/$WORKSTREAM_ID-issue-blocker.txt`, update
the row to `blocked`, and stop that row rather than choosing one arbitrarily.

Expected: every viable row without an adopted PR has exactly one verified issue
number, and every reuse, create, or block decision cites the saved paginated
issue ledger, per-workstream filtered search evidence, and verification files.

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
tests, targeted commands, full repository gates, commit boundaries, explicit
`git commit -s` usage, the session-required Copilot co-author trailer on child
commits, push, and PR creation. Human contributor `Co-authored-by:` trailers
remain conditional under `AGENTS.md` and are separate from the required
Copilot trailer.

Every child plan must reuse this exact child-commit pattern, replacing only
the commit message and adding any conditional human contributor trailers as
separate extra `--trailer` arguments:

```bash
COPILOT_GH_ID=223556219
COPILOT_GH_USER=Copilot
COPILOT_NOREPLY_DOMAIN=users.noreply.github.com
COPILOT_TRAILER="Co-authored-by: $COPILOT_GH_USER <${COPILOT_GH_ID}+${COPILOT_GH_USER}@${COPILOT_NOREPLY_DOMAIN}>"
git commit -s --trailer "$COPILOT_TRAILER" -m "<child commit message>"
```

Expected: independent plans exist only for viable rows that are not already
continuing an adopted exact-source-branch PR.

- [ ] **Step 5: Recover and publish each viable concern sequentially**

Skip this step for any row whose Task 5 Step 1 action is `continue existing PR`.
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
COMMON_DIR="$(git -C /tmp/coven-issue-541 rev-parse --git-common-dir)"
REPO="$(cd "$COMMON_DIR/.." && pwd)"
RECOVERY="$COMMON_DIR/agent-recovery/issue-541/viable"
CLASSIFICATION="$COMMON_DIR/agent-recovery/issue-541/classification.md"
CLAIM_BLOCKER_EVIDENCE="$RECOVERY/$WORKSTREAM_ID-claim-blocker.txt"
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
RECOVERY_BRANCH="issue-$ISSUE_NUMBER-$BRANCH_SLUG"
RECOVERY_WORKTREE="$REPO/.worktrees/coven-recovery-541-$ISSUE_NUMBER-$BRANCH_SLUG"
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
  -b "issue-$ISSUE_NUMBER-$BRANCH_SLUG" \
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
cd "$RECOVERY_WORKTREE"
coven claim heartbeat "issue-$ISSUE_NUMBER"
```

Keep that child claim active after PR creation while follow-up commits, review
responses, or final verification continue in the same session. Release it only
when that pull request merges or the owning recovery session stops:

```bash
cd "$RECOVERY_WORKTREE"
coven claim release "issue-$ISSUE_NUMBER"
```

Continue into the child recovery plan only after `coven claim acquire`
succeeds. If acquisition fails, persist
`viable/$WORKSTREAM_ID-claim-blocker.txt`, rewrite the classification row to
`blocked`, remove the newly created still-clean child worktree and its exact
local branch without force, and stop that row. If either cleanup command
fails, leave the residue in place, keep the row blocked, and treat that as an
operator-visible blocker.

Execute that issue's plan, run its full gates, commit each child change with
the Task 5 Step 4 trailer pattern, add human contributor co-author trailers
only when `AGENTS.md` requires them, push, and open its scoped pull request.
Immediately after any non-adopted recovery branch opens its PR, capture and
verify that PR before Task 7 cleanup or Task 9 audit continues:

```bash
COMMON_DIR="$(git -C /tmp/coven-issue-541 rev-parse --git-common-dir)"
REPO="$(cd "$COMMON_DIR/.." && pwd)"
RECOVERY="$COMMON_DIR/agent-recovery/issue-541/viable"
CLASSIFICATION="$COMMON_DIR/agent-recovery/issue-541/classification.md"
ISSUE_VIEW_EVIDENCE="$RECOVERY/$WORKSTREAM_ID-issue-view.json"
PR_VIEW_EVIDENCE="$RECOVERY/$WORKSTREAM_ID-pr-view.json"
PR_BLOCKER_EVIDENCE="$RECOVERY/$WORKSTREAM_ID-pr-blocker.txt"
RECOVERY_BRANCH="issue-$ISSUE_NUMBER-$BRANCH_SLUG"
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
    --json url --jq '.url' 2> "$PR_BLOCKER_EVIDENCE")"; then
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
if ! gh pr view "$RECOVERY_PR_URL" --repo OpenCoven/coven \
  --json number,url,state,headRefName,baseRefName \
  > "$PR_VIEW_EVIDENCE" 2> "$PR_BLOCKER_EVIDENCE"; then
  PR_VIEW_ERROR="$(cat "$PR_BLOCKER_EVIDENCE")"
  {
    printf 'Blocked %s: recovery PR URL %s could not be verified.\n' \
      "$WORKSTREAM_ID" "$RECOVERY_PR_URL"
    printf 'Expected recovery branch: %s\n' "$RECOVERY_BRANCH"
    printf 'Issue evidence: viable/%s-issue-view.json\n' "$WORKSTREAM_ID"
    printf 'gh pr view failure follows:\n%s\n' "$PR_VIEW_ERROR"
  } > "$PR_BLOCKER_EVIDENCE"
  update_classification_row \
    "blocked" \
    "Recovery PR verification failed; see viable/$WORKSTREAM_ID-pr-view.json and viable/$WORKSTREAM_ID-pr-blocker.txt." \
    "Blocked: recovery PR URL could not be verified for $RECOVERY_BRANCH."
  exit 0
fi
RECOVERY_PR_NUMBER="$(jq -r '.number' "$PR_VIEW_EVIDENCE")"
ACTUAL_PR_URL="$(jq -r '.url' "$PR_VIEW_EVIDENCE")"
ACTUAL_STATE="$(jq -r '.state' "$PR_VIEW_EVIDENCE")"
ACTUAL_BRANCH="$(jq -r '.headRefName' "$PR_VIEW_EVIDENCE")"
ACTUAL_BASE="$(jq -r '.baseRefName' "$PR_VIEW_EVIDENCE")"
if [ "$ACTUAL_STATE" != "OPEN" ] || \
   [ "$ACTUAL_BRANCH" != "$RECOVERY_BRANCH" ] || \
   [ "$ACTUAL_BASE" != "main" ]; then
  {
    printf 'Blocked %s: recovery PR verification failed.\n' "$WORKSTREAM_ID"
    printf 'Expected PR URL: %s\n' "$RECOVERY_PR_URL"
    printf 'Expected recovery branch: %s\n' "$RECOVERY_BRANCH"
    printf 'Expected base branch: main\n'
    printf 'Expected state: OPEN\n'
    printf 'Actual PR URL: %s\n' "$ACTUAL_PR_URL"
    printf 'Actual state: %s\n' "$ACTUAL_STATE"
    printf 'Actual branch: %s\n' "$ACTUAL_BRANCH"
    printf 'Actual base branch: %s\n' "$ACTUAL_BASE"
    printf 'PR view evidence: viable/%s-pr-view.json\n' "$WORKSTREAM_ID"
  } > "$PR_BLOCKER_EVIDENCE"
  update_classification_row \
    "blocked" \
    "Recovery PR verification mismatch; see viable/$WORKSTREAM_ID-pr-view.json and viable/$WORKSTREAM_ID-pr-blocker.txt." \
    "Blocked: recovery PR was not OPEN on $RECOVERY_BRANCH targeting main."
  exit 0
fi
update_classification_row \
  "viable" \
  "Issue verified via issue-541/issues.json, viable/$WORKSTREAM_ID-issue-search.json, and viable/$WORKSTREAM_ID-issue-view.json; recovery PR verified via viable/$WORKSTREAM_ID-pr-view.json." \
  "Recover via issue #$ISSUE_NUMBER ($ISSUE_URL) with open PR #$RECOVERY_PR_NUMBER ($ACTUAL_PR_URL)."
```

Expected: every viable row either continues one adopted exact-source-branch
open pull request after the single-candidate verification flow, or rewrites
its ledger row to stay `viable` with one verified OPEN recovery PR URL from
the expected current-main recovery branch after the zero-candidate normal
flow, before Task 7 cleanup or Task 9 audit begins.

### Task 6: Record Non-Viable Outcomes

**Artifacts:**
- Modify: `.git/agent-recovery/issue-541/classification.md`

- [ ] **Step 1: Record already-shipped evidence**

For each `already-shipped` row, add:

```markdown
- Equivalent current file or behavior:
- Merged pull request or commit:
- Why replay would duplicate rather than extend main:
```

Fill every field with a path and commit or pull-request URL.

- [ ] **Step 2: Record supersession evidence**

For each `superseded` row, add:

```markdown
- Superseding contract:
- Superseding pull request or commit:
- Regression or duplication caused by replay:
```

Fill every field with concrete evidence.

- [ ] **Step 3: Record blockers**

For each `blocked` row, add:

```markdown
- Missing authority or decision:
- Evidence that the agent cannot infer it:
- Preserved snapshot:
- Safe resume condition:
```

Expected: no non-viable row relies on branch age or lack of a PR as its sole
reason.

### Task 7: Clean Verified Git Residue

**Files:** None. This task changes only local Git worktree and branch metadata.

- [ ] **Step 1: Recheck claims and open pull requests**

Run:

```bash
COMMON_DIR="$(git -C /tmp/coven-issue-541 rev-parse --git-common-dir)"
REPO="$(cd "$COMMON_DIR/.." && pwd)"
cd "$REPO"
coven claim status
gh pr list --state open --limit 100
```

Expected: every active recovery claim and open PR is understood before cleanup.

- [ ] **Step 2: Remove prunable registrations only after snapshot verification**

Run:

```bash
COMMON_DIR="$(git -C /tmp/coven-issue-541 rev-parse --git-common-dir)"
REPO="$(cd "$COMMON_DIR/.." && pwd)"
cd "$REPO"
git worktree prune --dry-run --verbose
```

Verify every listed `/private/tmp` path either has a bundle/snapshot in the
issue-541 archive or corresponds to a merged/detached review with no unique
work. Then run:

```bash
git worktree prune --verbose
```

Expected: only registrations whose directories no longer exist are removed.

- [ ] **Step 3: Remove clean merged worktrees**

Check each known linked worktree:

```bash
COMMON_DIR="$(git -C /tmp/coven-issue-541 rev-parse --git-common-dir)"
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

Then prove each branch's PR merged or its tip is represented by current main,
and remove the five clean worktrees:

```bash
COMMON_DIR="$(git -C /tmp/coven-issue-541 rev-parse --git-common-dir)"
REPO="$(cd "$COMMON_DIR/.." && pwd)"
for path in \
  "$REPO/.worktrees/docs-cli-core-guides" \
  "$REPO/.worktrees/memory-summary-source" \
  "$REPO/.worktrees/memory-open" \
  "$REPO/.worktrees/fix-coven-hq8-privacy-lockfile" \
  "$REPO/.worktrees/memory-api-review"
do
  git -C "$REPO" worktree remove "$path"
done
```

Do not use `--force`. Any non-empty status blocks removal.

- [ ] **Step 4: Retire the original dirty source worktrees only after proof checks**

These four source worktrees remain intentionally present until their proof is
complete. Their runtime dirtiness may change after snapshotting—for example, a
formerly dirty tree may now be clean because its changes were committed to an
accurate active PR. `git worktree remove --force` is allowed only in this
step, only for these four exact paths, and only after the worktree and index
patch evidence, verified branch bundle, untracked inventory, terminal ledger
classification, and either an adopted exact-source-branch open PR, an open
replacement PR, or recorded non-viable/blocker evidence are all present.
Unrelated or
unsnapshotted worktrees must never use force.

Run:

```bash
COMMON_DIR="$(git -C /tmp/coven-issue-541 rev-parse --git-common-dir)"
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
  test -s "$RECOVERY/$id-untracked-evidence.txt"
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
preserved snapshot. Any mismatch or missing file blocks removal and requires a
fresh snapshot plus reclassification for that row. Only unchanged rows may use
the exact-path force-removal exception:

```bash
COMMON_DIR="$(git -C /tmp/coven-issue-541 rev-parse --git-common-dir)"
REPO="$(cd "$COMMON_DIR/.." && pwd)"
RECOVERY="$COMMON_DIR/agent-recovery/issue-541"
RETIRE_PROOF_ROOT="$RECOVERY/private-retire-proof"
mkdir -p "$RETIRE_PROOF_ROOT"
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
  if ! test -d "$SOURCE"; then
    printf 'Blocked %s: source worktree is missing before retirement proof; take a fresh snapshot and reclassify.\n' \
      "$id" > "$BLOCKER"
    exit 1
  fi
  git -C "$SOURCE" rev-parse HEAD > "$PROOF_DIR/live-head.txt"
  if ! cmp -s "$PROOF_DIR/live-head.txt" "$SNAPSHOT/head.txt"; then
    printf 'Blocked %s: HEAD drifted since snapshot; take a fresh snapshot and reclassify before removal.\n' \
      "$id" > "$BLOCKER"
    exit 1
  fi
  git -C "$SOURCE" diff --binary > "$PROOF_DIR/live-worktree.patch"
  if ! cmp -s "$PROOF_DIR/live-worktree.patch" "$SNAPSHOT/worktree.patch"; then
    printf 'Blocked %s: unstaged tracked changes drifted since snapshot; take a fresh snapshot and reclassify before removal.\n' \
      "$id" > "$BLOCKER"
    exit 1
  fi
  git -C "$SOURCE" diff --cached --binary > "$PROOF_DIR/live-index.patch"
  if ! cmp -s "$PROOF_DIR/live-index.patch" "$SNAPSHOT/index.patch"; then
    printf 'Blocked %s: staged changes drifted since snapshot; take a fresh snapshot and reclassify before removal.\n' \
      "$id" > "$BLOCKER"
    exit 1
  fi
  git -C "$SOURCE" ls-files --others --exclude-standard > "$PROOF_DIR/live-untracked-files.txt"
  if ! cmp -s "$PROOF_DIR/live-untracked-files.txt" "$SNAPSHOT/untracked-files.txt"; then
    printf 'Blocked %s: untracked path inventory drifted since snapshot; take a fresh snapshot and reclassify before removal.\n' \
      "$id" > "$BLOCKER"
    exit 1
  fi
  while IFS= read -r path; do
    test -n "$path" || continue
    if ! test -e "$SOURCE/$path"; then
      printf 'Blocked %s: live untracked file is missing: %s. Take a fresh snapshot and reclassify before removal.\n' \
        "$id" "$path" > "$BLOCKER"
      exit 1
    fi
    if ! test -e "$SNAPSHOT/untracked/$path"; then
      printf 'Blocked %s: snapshotted untracked file copy is missing: %s. Take a fresh snapshot and reclassify before removal.\n' \
        "$id" "$path" > "$BLOCKER"
      exit 1
    fi
    if ! cmp -s "$SOURCE/$path" "$SNAPSHOT/untracked/$path"; then
      printf 'Blocked %s: untracked file content drifted for %s; take a fresh snapshot and reclassify before removal.\n' \
        "$id" "$path" > "$BLOCKER"
      exit 1
    fi
  done < "$SNAPSHOT/untracked-files.txt"
done
for path in \
  "$REPO/.worktrees/docs-psyche-specs" \
  "$REPO/.worktrees/feat-cmem-1ev-memory-promote" \
  "$REPO/.worktrees/mobile-memory-gateway" \
  "$REPO/.worktrees/pr-476-review"
do
  git -C "$REPO" worktree remove --force "$path"
done
```

Expected: only those four exact dirty source paths are force-removed, because
their committed history, dirty state, and recovery disposition have already
been proven elsewhere in the archive and reconfirmed as unchanged against the
preserved snapshot immediately before removal. Empty worktree, index, and
untracked snapshot classes are still acceptable, but the retirement proof now
requires non-empty evidence files populated either with captured change
summaries or deterministic sentinel lines, plus matching live proof files under
`private-retire-proof/`.

- [ ] **Step 5: Delete only proven local branch residue after worktree retirement**

For each branch in the ledger with merged or superseded evidence, or each of
the four dirty source branches once Step 4 has removed its worktree and proven
either the verified bundle or a pushed replacement recovery ref, set
`BRANCH_TO_DELETE` to its exact local branch name. Recheck that exact branch
ref immediately before every deletion command against the preserved head source
for that branch; do not rely only on the earlier worktree-retirement proof.
Task 3 Step 3 already creates the explicit orphan-branch `head.txt` files used
below. Use this exact mapping for all seven recovered source branches:

- `docs/psyche-specs` -> `dirty/docs-psyche-specs/head.txt`
- `docs/universal-runtime-capability-design` -> `branches/docs-universal-runtime-capability-design/head.txt`
- `feat/cmem-1ev-memory-promote` -> `dirty/memory-promote/head.txt`
- `feat/mobile-memory-gateway` -> `dirty/mobile-memory-gateway/head.txt`
- `feat/npm-macos-x64` -> `branches/feat-npm-macos-x64/head.txt`
- `fix/476-review-threads` -> `dirty/pr-476-review/head.txt`
- `fix/521-ward-surface-confinement` -> `branches/fix-521-ward-surface-confinement/head.txt`

Run this immediately before `branch -d`:

```bash
COMMON_DIR="$(git -C /tmp/coven-issue-541 rev-parse --git-common-dir)"
REPO="$(cd "$COMMON_DIR/.." && pwd)"
BRANCH_DELETE_PROOF_ROOT="$COMMON_DIR/agent-recovery/issue-541/private/branch-delete-proof"
mkdir -p "$BRANCH_DELETE_PROOF_ROOT"
recheck_branch_ref_tip() {
  MODE="$1"
  BRANCH_PROOF_ID="$(printf '%s' "$BRANCH_TO_DELETE" | tr '/' '_')"
  case "$BRANCH_TO_DELETE" in
    docs/psyche-specs)
      PRESERVED_HEAD_SOURCE="dirty/docs-psyche-specs/head.txt"
      ;;
    docs/universal-runtime-capability-design)
      PRESERVED_HEAD_SOURCE="branches/docs-universal-runtime-capability-design/head.txt"
      ;;
    feat/cmem-1ev-memory-promote)
      PRESERVED_HEAD_SOURCE="dirty/memory-promote/head.txt"
      ;;
    feat/mobile-memory-gateway)
      PRESERVED_HEAD_SOURCE="dirty/mobile-memory-gateway/head.txt"
      ;;
    feat/npm-macos-x64)
      PRESERVED_HEAD_SOURCE="branches/feat-npm-macos-x64/head.txt"
      ;;
    fix/476-review-threads)
      PRESERVED_HEAD_SOURCE="dirty/pr-476-review/head.txt"
      ;;
    fix/521-ward-surface-confinement)
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
    printf 'Preserved head source: %s\n' "$PRESERVED_HEAD_SOURCE"
    printf 'Preserved head: %s\n' "$PRESERVED_HEAD"
    printf 'Live branch ref tip: %s\n' "$LIVE_HEAD"
  } > "$PROOF_FILE"
  if [ "$LIVE_HEAD" != "$PRESERVED_HEAD" ]; then
    printf 'Blocked: live branch ref tip differs from the preserved head; newer commits are unpreserved.\n' \
      >> "$PROOF_FILE"
    return 1
  fi
  return 0
}
recheck_branch_ref_tip pre-delete-d
CHECK_STATUS=$?
if [ "$CHECK_STATUS" -eq 2 ]; then
  exit 0
fi
if [ "$CHECK_STATUS" -ne 0 ]; then
  exit "$CHECK_STATUS"
fi
git -C "$REPO" branch -d "$BRANCH_TO_DELETE"
```

If squash history makes `-d` refuse, recheck the ledger evidence, reconfirm
its source worktree is already removed and its verified snapshot bundle or
pushed replacement branch still proves the committed history, then rerun the
same branch-ref proof immediately before `branch -D`:

```bash
COMMON_DIR="$(git -C /tmp/coven-issue-541 rev-parse --git-common-dir)"
REPO="$(cd "$COMMON_DIR/.." && pwd)"
BRANCH_DELETE_PROOF_ROOT="$COMMON_DIR/agent-recovery/issue-541/private/branch-delete-proof"
mkdir -p "$BRANCH_DELETE_PROOF_ROOT"
recheck_branch_ref_tip() {
  MODE="$1"
  BRANCH_PROOF_ID="$(printf '%s' "$BRANCH_TO_DELETE" | tr '/' '_')"
  case "$BRANCH_TO_DELETE" in
    docs/psyche-specs)
      PRESERVED_HEAD_SOURCE="dirty/docs-psyche-specs/head.txt"
      ;;
    docs/universal-runtime-capability-design)
      PRESERVED_HEAD_SOURCE="branches/docs-universal-runtime-capability-design/head.txt"
      ;;
    feat/cmem-1ev-memory-promote)
      PRESERVED_HEAD_SOURCE="dirty/memory-promote/head.txt"
      ;;
    feat/mobile-memory-gateway)
      PRESERVED_HEAD_SOURCE="dirty/mobile-memory-gateway/head.txt"
      ;;
    feat/npm-macos-x64)
      PRESERVED_HEAD_SOURCE="branches/feat-npm-macos-x64/head.txt"
      ;;
    fix/476-review-threads)
      PRESERVED_HEAD_SOURCE="dirty/pr-476-review/head.txt"
      ;;
    fix/521-ward-surface-confinement)
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
    printf 'Preserved head source: %s\n' "$PRESERVED_HEAD_SOURCE"
    printf 'Preserved head: %s\n' "$PRESERVED_HEAD"
    printf 'Live branch ref tip: %s\n' "$LIVE_HEAD"
  } > "$PROOF_FILE"
  if [ "$LIVE_HEAD" != "$PRESERVED_HEAD" ]; then
    printf 'Blocked: live branch ref tip differs from the preserved head; newer commits are unpreserved.\n' \
      >> "$PROOF_FILE"
    return 1
  fi
  return 0
}
recheck_branch_ref_tip pre-delete-D
CHECK_STATUS=$?
if [ "$CHECK_STATUS" -eq 2 ]; then
  exit 0
fi
if [ "$CHECK_STATUS" -ne 0 ]; then
  exit "$CHECK_STATUS"
fi
git -C "$REPO" branch -D "$BRANCH_TO_DELETE"
```

If the local branch ref is already missing, record that outcome in the private
proof file and succeed without failing the step. If the live branch ref tip
exists but differs from the preserved head, stop and do not use `-D`; newer
commits are unpreserved until a fresh archive captures them.

- [ ] **Step 6: Release only merged or stopped recovery claims**

For any child recovery workstream whose PR has merged or whose owning recovery
session has stopped, release its claim from that child worktree:

```bash
cd "$RECOVERY_WORKTREE"
coven claim release "issue-$ISSUE_NUMBER"
```

Keep child claims active while follow-up work for an open PR continues. During
long-running follow-up from that worktree, use:

```bash
cd "$RECOVERY_WORKTREE"
coven claim heartbeat "issue-$ISSUE_NUMBER"
```

Release the parent `issue-541` claim only when the issue #541 recovery session
stops or the full issue #541 recovery effort is complete:

```bash
cd /tmp/coven-issue-541
coven claim release issue-541
```

Expected: `coven claim status` shows no abandoned claim; active claims remain
allowed only for still-running recovery sessions and open follow-up work.

### Task 8: Reconcile Repository Goals

**Files:**
- Modify: `.copilot/goals.md`

- [ ] **Step 1: Re-read goals and live issue state**

Run:

```bash
COMMON_DIR="$(git -C /tmp/coven-issue-541 rev-parse --git-common-dir)"
REPO="$(cd "$COMMON_DIR/.." && pwd)"
cd "$REPO"
sed -n '1,260p' .copilot/goals.md
for issue in 401 414 521 541; do
  gh issue view "$issue" --json number,state,title,closedAt,url
done
```

Expected: #401, #414, and #521 are closed; #541 reflects the recovery PR state.

- [ ] **Step 2: Close stale active goal content**

Move `usability-core-consolidation` to `done` because its named high-risk
follow-up #401 is closed. Set:

```markdown
- completed: 2026-08-01
- outcome: |
    The five top gaps and the session-launch consolidation tracked by #401 are
    closed. Remaining translation drift is not part of this completed
    consolidation goal and requires a separately claimed issue if resumed.
```

Remove obsolete `next` text that presents #401 as future work.

- [ ] **Step 3: Reconcile contribution stewardship**

Keep `contribution-stewardship` active because it is an ongoing maintenance
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
COMMON_DIR="$(git -C /tmp/coven-issue-541 rev-parse --git-common-dir)"
REPO="$(cd "$COMMON_DIR/.." && pwd)"
cd "$REPO"
grep -n '^## active\|^## paused\|^## done\|^### goal:\|^- next:' \
  .copilot/goals.md
```

Expected: each active goal has one `next` field, completed goals are under
`## done`, and no active `next` references closed issues as future work.

### Task 9: Final Recovery Audit

**Artifacts:**
- Create or modify: `.git/agent-recovery/issue-541/final-audit-primary-checkout.txt`
- Modify: `.git/agent-recovery/issue-541/classification.md`

- [ ] **Step 1: Verify every manifest row has a terminal recovery state**

Run:

```bash
COMMON_DIR="$(git -C /tmp/coven-issue-541 rev-parse --git-common-dir)"
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
  grep -E "^\| $id \| (already-shipped|superseded|viable|blocked) \|" \
    "$RECOVERY/classification.md"
done
```

Expected: all seven workstreams match exactly one terminal classification.

- [ ] **Step 2: Verify all viable rows have open pull requests**

For every `viable` row, set `PR_URL` to the recorded pull-request URL and run:

```bash
COMMON_DIR="$(git -C /tmp/coven-issue-541 rev-parse --git-common-dir)"
REPO="$(cd "$COMMON_DIR/.." && pwd)"
cd "$REPO"
gh pr view "$PR_URL" --repo OpenCoven/coven \
  --json state,isDraft,mergeStateStatus,url
```

Expected: state is `OPEN`; draft status may reflect repository readiness, and
the URL matches the ledger.

- [ ] **Step 3: Restore the primary checkout before the final audit**

Run:

```bash
COMMON_DIR="$(git -C /tmp/coven-issue-541 rev-parse --git-common-dir)"
REPO="$(cd "$COMMON_DIR/.." && pwd)"
AUDIT_EVIDENCE="$COMMON_DIR/agent-recovery/issue-541/final-audit-primary-checkout.txt"
cd "$REPO"
CURRENT_BRANCH="$(git branch --show-current)"
UPSTREAM_REF="$(git rev-parse --abbrev-ref --symbolic-full-name @{upstream} 2>/dev/null || true)"
UPSTREAM_REMOTE="$(git config --get "branch.$CURRENT_BRANCH.remote" 2>/dev/null || true)"
UPSTREAM_MERGE_REF="$(git config --get "branch.$CURRENT_BRANCH.merge" 2>/dev/null || true)"
STATUS="$(git status --porcelain=v1 --untracked-files=all)"
record_state() {
  local current_branch current_upstream current_remote current_merge
  current_branch="$(git branch --show-current)"
  current_upstream="$(git rev-parse --abbrev-ref --symbolic-full-name @{upstream} 2>/dev/null || true)"
  current_remote="$(git config --get "branch.$current_branch.remote" 2>/dev/null || true)"
  current_merge="$(git config --get "branch.$current_branch.merge" 2>/dev/null || true)"
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
elif printf '%s\n%s\n' "$CURRENT_BRANCH" "$UPSTREAM_REF" | grep -Eq '(^|[-/])issue-541($|[-/])|(^|[-/])541($|[-/])'; then
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
  if ! git merge --ff-only origin/main; then
    block_restore \
      "Blocked: primary checkout could not fast-forward main to origin/main after switching; leave it untouched."
  fi
else
  block_restore \
    "Blocked: primary checkout is on an unrelated branch; leave it untouched."
fi
FINAL_BRANCH="$(git branch --show-current)"
FINAL_STATUS="$(git status --porcelain --untracked-files=all)"
if [ "$FINAL_BRANCH" != "main" ]; then
  block_restore \
    "Blocked: final primary checkout branch is not main after restore; leave it untouched."
fi
if [ -n "$FINAL_STATUS" ]; then
  block_restore \
    "Blocked: primary checkout is not clean after restore; leave it untouched."
fi
{
  printf 'Primary checkout restored for final audit.\n'
  record_state
} > "$AUDIT_EVIDENCE"
cat "$AUDIT_EVIDENCE"
```

This step is mandatory before any final-audit branch-sensitive command. It
must stay non-interactive: do not use `git reset`, `git checkout --force`, or
any other destructive switch. If this step exits non-zero, stop Task 9 and
leave the primary checkout exactly as it was. For recovery-owned branches, the
safety check must parse `branch.<name>.remote` and `branch.<name>.merge`,
fetch that exact upstream into `FETCH_HEAD`, and compare `HEAD` with the fresh
upstream tip rather than relying on a stale remote-tracking ref.

Expected: the primary checkout either remains untouched with blocker evidence
in `final-audit-primary-checkout.txt`, or it is restored cleanly to `main` by
a safe fast-forward-only path.

- [ ] **Step 4: Verify the primary checkout**

Run:

```bash
COMMON_DIR="$(git -C /tmp/coven-issue-541 rev-parse --git-common-dir)"
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

- [ ] **Step 5: Reject unsanitized local paths before posting the ledger**

Run:

```bash
COMMON_DIR="$(git -C /tmp/coven-issue-541 rev-parse --git-common-dir)"
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
COMMON_DIR="$(git -C /tmp/coven-issue-541 rev-parse --git-common-dir)"
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
COMMON_DIR="$(git -C /tmp/coven-issue-541 rev-parse --git-common-dir)"
REPO="$(cd "$COMMON_DIR/.." && pwd)"
cd "$REPO"
gh issue close 541 --repo OpenCoven/coven --comment \
  "Recovery inventory is complete. Every viable concern has a scoped open PR; non-viable work has evidence and durable snapshots; local goals and Git hygiene are reconciled."
```

Expected: #541 is closed while child implementation PRs continue through normal
review and merge.
