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
- Create: `.git/agent-recovery/issue-541/dirty/docs-psyche-specs/`
- Create: `.git/agent-recovery/issue-541/dirty/memory-promote/`
- Create: `.git/agent-recovery/issue-541/dirty/mobile-memory-gateway/`
- Create: `.git/agent-recovery/issue-541/dirty/pr-476-review/`
- Create: `.git/agent-recovery/issue-541/dirty/docs-psyche-specs/branch.bundle`
- Create: `.git/agent-recovery/issue-541/dirty/memory-promote/branch.bundle`
- Create: `.git/agent-recovery/issue-541/dirty/mobile-memory-gateway/branch.bundle`
- Create: `.git/agent-recovery/issue-541/dirty/pr-476-review/branch.bundle`
  - Status, commit identifiers, exact branch-name marker, verified
    `branch.bundle`, binary patches, and copied untracked files for each dirty
    worktree, so committed branch history is preserved before any later source
    branch deletion.
- Create: `.git/agent-recovery/issue-541/branches/docs-universal-runtime-capability-design.bundle`
- Create: `.git/agent-recovery/issue-541/branches/feat-npm-macos-x64.bundle`
- Create: `.git/agent-recovery/issue-541/branches/fix-521-ward-surface-confinement.bundle`
  - Ref-preserving archive for each orphan branch.
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

- [ ] **Step 1: Confirm the design worktree is clean except for the plan**

Run:

```bash
git -C /tmp/coven-issue-541 status --short --branch
```

Expected: branch `docs/541-incomplete-work-recovery-design`, with only the plan
file untracked before it is staged.

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

### Task 2: Snapshot Every Dirty Worktree

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
committed history before any later branch deletion; both patch files exist;
`index.patch` preserves the staged additions for
`specs/psyche/COVEN_PREREQUISITES.md`, `specs/psyche/PLAN.md`, and the Psyche
reconciliation plan; and any copied untracked files match only the paths listed
in `untracked-files.txt`.

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

- [ ] **Step 1: Create ref-preserving bundles**

Run:

```bash
COMMON_DIR="$(git -C /tmp/coven-issue-541 rev-parse --git-common-dir)"
REPO="$(cd "$COMMON_DIR/.." && pwd)"
RECOVERY="$COMMON_DIR/agent-recovery/issue-541"
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

Expected: three bundle files are created.

- [ ] **Step 2: Verify each bundle**

Run:

```bash
COMMON_DIR="$(git -C /tmp/coven-issue-541 rev-parse --git-common-dir)"
RECOVERY="$COMMON_DIR/agent-recovery/issue-541"
git bundle verify "$RECOVERY/branches/docs-universal-runtime-capability-design.bundle"
git bundle verify "$RECOVERY/branches/feat-npm-macos-x64.bundle"
git bundle verify "$RECOVERY/branches/fix-521-ward-surface-confinement.bundle"
```

Expected: Git reports each bundle is okay and lists its branch ref.

- [ ] **Step 3: Record the archived branches**

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
  printf '%s\torphan-branch\t%s\t%s\t%s\t%s\n' \
    "$id" \
    "$branch" \
    "$(git -C "$REPO" rev-parse "$branch")" \
    "$(git -C "$REPO" merge-base "$branch" origin/main)" \
    "$RECOVERY/branches/$id.bundle" >> "$RECOVERY/manifest.tsv"
done
```

Expected: `manifest.tsv` has seven data rows plus its header.
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
source and treat `issues.json` as the broader issue-history ledger.

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
one next action before Task 4 Step 5 and Task 9 Step 1 verification.

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

- [ ] **Step 1: Create or identify one issue per viable row**

Use the matching exact query for each viable row:

```bash
gh issue list --repo OpenCoven/coven --state all --search '"mobile pairing"' --limit 20
gh issue list --repo OpenCoven/coven --state all --search '"Intel macOS" npm' --limit 20
gh issue list --repo OpenCoven/coven --state all --search '"Ward surface confinement"' --limit 20
gh issue list --repo OpenCoven/coven --state all --search '"memory promotion"' --limit 20
gh issue list --repo OpenCoven/coven --state all --search '"Psyche" specs' --limit 20
gh issue list --repo OpenCoven/coven --state all --search '"universal runtime" capability' --limit 20
gh issue list --repo OpenCoven/coven --state all --search '"runtime model parity"' --limit 20
```

If no issue accurately owns the concern, create one whose body includes:

```markdown
## Recovered source

Issue #541 recovery archive and classification ledger.

## Acceptance criteria

- Preserve the still-valid intent identified in the classification evidence.
- Rebuild against current `origin/main`; do not blindly replay obsolete code.
- Add or retain regression coverage for the recovered behavior.
- Pass all repository-required gates.
```

Expected: every viable row has exactly one issue number.

- [ ] **Step 2: Create one approved design per viable issue**

Use the brainstorming workflow separately for each issue. The design must name:

- the current-main files and contracts involved;
- the exact portion of the preserved source that remains valid;
- intentionally omitted obsolete portions;
- test and migration behavior;
- one-concern pull-request boundaries.

Expected: each viable concern has an approved and committed design document.

- [ ] **Step 3: Create one implementation plan per approved design**

Use the writing-plans workflow separately for each approved design. Each plan
must include exact paths, failing tests, targeted commands, full repository
gates, commit boundaries, explicit `git commit -s` usage, the session-required
Copilot co-author trailer on child commits, push, and PR creation. Human
contributor `Co-authored-by:` trailers remain conditional under `AGENTS.md`
and are separate from the required Copilot trailer.

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

Expected: independent plans exist for mobile pairing, Intel macOS packaging,
Ward confinement, memory promotion, Psyche specifications, universal runtime
design, or runtime parity only when their ledger row is `viable`.

- [ ] **Step 4: Recover and publish each viable concern sequentially**

For each viable row, set `ISSUE_NUMBER` to its created or reused issue number
and set `BRANCH_SLUG` from this fixed mapping:

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
git -C "$REPO" fetch origin main
git -C "$REPO" worktree add \
  -b "issue-$ISSUE_NUMBER-$BRANCH_SLUG" \
  "$RECOVERY_WORKTREE" \
  origin/main
cd "$RECOVERY_WORKTREE"
coven claim acquire "issue-$ISSUE_NUMBER"
```

Execute that issue's plan, run its full gates, commit each child change with
the Task 5 Step 3 trailer pattern, add human contributor co-author trailers
only when `AGENTS.md` requires them, push, and open its scoped pull request.

Expected: every viable row links to one open pull request from a current-main
branch.

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

These four source worktrees remain intentionally dirty until their proof is
complete. `git worktree remove --force` is allowed only in this step, only for
these four exact paths, and only after the worktree and index patch evidence,
verified branch bundle, untracked inventory, terminal ledger classification,
and either an open replacement PR or recorded non-viable/blocker evidence are
all present. Unrelated or unsnapshotted worktrees must never use force.

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

If a row is `viable`, confirm its classification row cites the open replacement
PR URL before removal. Otherwise confirm the row already records its
`already-shipped`, `superseded`, or `blocked` evidence. Then remove only the
four original dirty source worktrees:

```bash
COMMON_DIR="$(git -C /tmp/coven-issue-541 rev-parse --git-common-dir)"
REPO="$(cd "$COMMON_DIR/.." && pwd)"
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
been proven elsewhere in the archive. Empty worktree, index, and untracked
snapshot classes are still acceptable, but the retirement proof now requires
non-empty evidence files populated either with captured change summaries or
deterministic sentinel lines.

- [ ] **Step 5: Delete only proven local branch residue after worktree retirement**

For each branch in the ledger with merged or superseded evidence, or each of
the four dirty source branches once Step 4 has removed its worktree and proven
either the verified bundle or a pushed replacement recovery ref, set
`BRANCH_TO_DELETE` to its exact local branch name and run:

```bash
COMMON_DIR="$(git -C /tmp/coven-issue-541 rev-parse --git-common-dir)"
REPO="$(cd "$COMMON_DIR/.." && pwd)"
git -C "$REPO" branch -d "$BRANCH_TO_DELETE"
```

If squash history makes `-d` refuse, recheck the ledger evidence and use:

```bash
git -C "$REPO" branch -D "$BRANCH_TO_DELETE"
```

only after confirming its source worktree is already removed and its verified
snapshot bundle or pushed replacement branch still proves the committed history.

- [ ] **Step 6: Release stopped recovery claims**

After each child pull request is opened, release its claim from that worktree:

```bash
coven claim release "issue-$ISSUE_NUMBER"
```

Release the design claim when issue #541 work stops:

```bash
cd /tmp/coven-issue-541
coven claim release issue-541
```

Expected: `coven claim status` has no abandoned active claim.

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

- [ ] **Step 3: Verify the primary checkout**

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

- [ ] **Step 4: Reject unsanitized local paths before posting the ledger**

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

- [ ] **Step 5: Update issue #541**

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

- [ ] **Step 6: Close issue #541 when all machine work is delivered**

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
