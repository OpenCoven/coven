# Psyche O1.1 Delivery Closure Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Finish O1.1 by reconciling the canonical Psyche documents and closed O1 trackers with the conformance implementation that already merged.

**Architecture:** Preserve the merged runtime and regression baseline unchanged. Update only the O1 annex, parent O1 design, and Psyche program plan to record the observed plan, corrective-runtime, annex, and conformance merge commits; then append the same bounded evidence to the already-closed O1 issue and Bead without reopening or re-closing them.

**Tech Stack:** Markdown, Python 3 repository guards, Git, GitHub CLI, Beads CLI.

---

## Current evidence

| Artifact | Pull request | Observed merge commit |
|---|---:|---|
| O1 corrective runtime | #622 | `f68be0a0af373caf81780b70a5d3bf7d680e0f6e` |
| O1.1 corrective annex | #633 | `c183d923e6d9e9d8172f39a193d624fe40095892` |
| O1.1 conformance plan | #639 | `7f713ca3b3247b06d0fcff5aff3c756aa63c25eb` |
| O1.1 conformance implementation | #664 | `3d18a53c309fad64cc177ad2d984e6b09de0ffc1` |

PR #664 merged on 2026-08-07 with all 12 hosted checks successful,
including Ubuntu and Windows Rust checks, OpenClaw bridge, secret guard,
dependency audit, engine contract, channels package, CLI performance, and npm
onboarding checks.

GitHub issue #567 and Bead `coven-psy-o1` are already closed with the original
O1 merge evidence. O1.1 closure must append evidence to those records; it must
not reopen them, close them a second time, or claim completion of C-S3-C-S6,
C-S9-C-S12, G4, G6, or production child dispatch.

## File map

- `docs/superpowers/specs/2026-08-05-psyche-o1-1-corrective-annex-design.md` - replace the pre-delivery status with observed O1.1 completion evidence.
- `specs/psyche/O1_CONTRACT_DESIGN.md` - link the approved corrective annex and state that O1 plus O1.1 delivery evidence is complete.
- `specs/psyche/PLAN.md` - replace the stale pre-merge O1 candidate paragraph with the bounded completed state.

No Rust, TypeScript, schema, route, lifecycle, authorization, or dispatch file
is in scope.

### Task 1: Reconcile the O1.1 annex

**Files:**
- Modify: `docs/superpowers/specs/2026-08-05-psyche-o1-1-corrective-annex-design.md:3-12`
- Modify: `docs/superpowers/specs/2026-08-05-psyche-o1-1-corrective-annex-design.md:338-349`

- [ ] **Step 1: Replace the stale status**

Replace:

```markdown
**Status:** Approved design; implementation evidence remains part of the O1
delivery candidate
```

with:

```markdown
**Status:** Complete - corrective runtime, approved annex, conformance coverage,
hosted verification, and observed merge evidence are recorded
```

- [ ] **Step 2: Add the immutable implementation ledger**

After the Scope paragraph, add:

```markdown
**Observed delivery evidence:**

- corrective runtime PR #622 merged at
  `f68be0a0af373caf81780b70a5d3bf7d680e0f6e`;
- this approved annex merged in PR #633 at
  `c183d923e6d9e9d8172f39a193d624fe40095892`;
- the conformance plan merged in PR #639 at
  `7f713ca3b3247b06d0fcff5aff3c756aa63c25eb`; and
- the remaining conformance coverage merged in PR #664 at
  `3d18a53c309fad64cc177ad2d984e6b09de0ffc1`, with all 12 hosted
  checks successful.

This evidence closes O1.1 only. It does not satisfy C-S3-C-S6, C-S9-C-S12,
G4, G6, or production Psyche child dispatch.
```

- [ ] **Step 3: Replace conditional completion language**

Replace the numbered list under `## 10. Completion evidence` with:

```markdown
O1.1 is complete because:

1. this corrective annex was approved and merged in PR #633;
2. the corrective implementation was reviewed against O1 and O1.1;
3. the focused continuation, stream-identity, process-ownership, cancellation,
   PID-safety, and malformed-frame regressions pass;
4. PR #664 passed all 12 hosted checks, including Unix and Windows Rust,
   OpenClaw, secret, dependency, package, and performance checks;
5. the observed implementation merge is
   `3d18a53c309fad64cc177ad2d984e6b09de0ffc1`; and
6. C-S3-C-S6, C-S9-C-S12, G4, G6, and production Psyche child dispatch remain
   explicitly incomplete.
```

- [ ] **Step 4: Inspect the annex-only diff**

Run:

```bash
git diff -- docs/superpowers/specs/2026-08-05-psyche-o1-1-corrective-annex-design.md
```

Expected: only status and evidence prose changes; no normative invariant or
scope exclusion is weakened.

### Task 2: Reconcile the parent O1 records

**Files:**
- Modify: `specs/psyche/O1_CONTRACT_DESIGN.md:3-12`
- Modify: `specs/psyche/PLAN.md:7-16`

- [ ] **Step 1: Update the O1 status**

Replace the status in `specs/psyche/O1_CONTRACT_DESIGN.md` with:

```markdown
**Status:** Complete - O1 merged in PR #574; corrective O1.1 delivery and
conformance evidence merged in PRs #622, #633, #639, and #664
```

- [ ] **Step 2: Link the corrective annex**

After the GitHub issue line, add:

```markdown
**Corrective delivery annex:** [`docs/superpowers/specs/2026-08-05-psyche-o1-1-corrective-annex-design.md`](../../docs/superpowers/specs/2026-08-05-psyche-o1-1-corrective-annex-design.md)
```

- [ ] **Step 3: Replace the stale program-plan candidate**

Replace `**O1 implementation candidate:**` and its paragraph in
`specs/psyche/PLAN.md` with:

```markdown
**O1/O1.1 complete:** O1 named-contract negotiation and lifecycle vocabulary
merged in PR #574. Corrective continuation, stream-identity, and
process-supervision behavior merged in PR #622; the approved O1.1 annex merged
in PR #633; its implementation plan merged in PR #639; and the remaining
conformance coverage merged in PR #664 with all 12 hosted checks successful.
Issue #567 and Bead `coven-psy-o1` remain the closed O1 trackers and receive an
append-only O1.1 evidence note. This closes only C-S1 vocabulary and C-S8
documentation. C-S3-C-S6 and C-S9-C-S12 remain separate work, while G4, G6,
and production child dispatch remain blocked.
```

- [ ] **Step 4: Confirm later gates remain blocked**

Run:

```bash
git diff -- specs/psyche/O1_CONTRACT_DESIGN.md specs/psyche/PLAN.md
rg -n 'C-S3-C-S6|C-S9-C-S12|G4|G6|production child dispatch' \
  specs/psyche/O1_CONTRACT_DESIGN.md \
  specs/psyche/PLAN.md \
  docs/superpowers/specs/2026-08-05-psyche-o1-1-corrective-annex-design.md
```

Expected: the diff records observed delivery only, and the search still finds
explicit later-contract and production-dispatch blockers.

### Task 3: Verify and commit the documentation closure

**Files:**
- Verify: `docs/superpowers/specs/2026-08-05-psyche-o1-1-corrective-annex-design.md`
- Verify: `specs/psyche/O1_CONTRACT_DESIGN.md`
- Verify: `specs/psyche/PLAN.md`

- [ ] **Step 1: Run documentation contract tests**

Run:

```bash
python3 scripts/check-api-contract-docs-test.py
python3 scripts/check-api-contract-docs.py
```

Expected: both commands exit 0.

- [ ] **Step 2: Scan for placeholders and stale status**

Run:

```bash
! rg -n 'TBD|TODO|FIXME|PLACEHOLDER|O1 remains incomplete|O1 implementation candidate' \
  docs/superpowers/specs/2026-08-05-psyche-o1-1-corrective-annex-design.md \
  specs/psyche/O1_CONTRACT_DESIGN.md \
  specs/psyche/PLAN.md
```

Expected: exit 0 with no matches.

- [ ] **Step 3: Stage the exact documentation set**

Run:

```bash
git add \
  docs/superpowers/specs/2026-08-05-psyche-o1-1-corrective-annex-design.md \
  specs/psyche/O1_CONTRACT_DESIGN.md \
  specs/psyche/PLAN.md
python3 scripts/check-coven-privacy.py --staged
python3 scripts/check-secrets.py
git diff --cached --check
git diff --cached --stat
```

Expected: privacy, secret, and whitespace checks pass; the staged diff contains
exactly the three documentation files.

- [ ] **Step 4: Commit the reconciled evidence**

Run:

```bash
git commit -s -m "docs(psyche): close O1.1 delivery evidence" \
  -m "Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

Expected: one documentation-only commit.

### Task 4: Merge the closure record

**Files:**
- Verify: branch diff against `origin/main`

- [ ] **Step 1: Run whole-branch guards**

Run:

```bash
python3 scripts/check-coven-privacy.py --range origin/main...HEAD
git diff --check origin/main...HEAD
git diff --stat origin/main...HEAD
git status --short
```

Expected: guards pass; the branch contains this plan plus the three reconciled
Psyche documents and no runtime changes.

- [ ] **Step 2: Push and open the documentation PR**

Run:

```bash
git push -u origin docs/psyche-o1-1-delivery-closure
gh pr create \
  --repo OpenCoven/coven \
  --base main \
  --head docs/psyche-o1-1-delivery-closure \
  --title "docs(psyche): close O1.1 delivery evidence" \
  --body "Records the already-merged O1.1 runtime, annex, plan, conformance implementation, and hosted verification. No runtime behavior or later Psyche gate changes."
```

Expected: GitHub returns a PR URL for a documentation-only branch.

- [ ] **Step 3: Merge only after hosted checks and review pass**

Run:

```bash
gh pr checks docs/psyche-o1-1-delivery-closure --repo OpenCoven/coven
gh pr view docs/psyche-o1-1-delivery-closure \
  --repo OpenCoven/coven \
  --json reviewDecision,mergeStateStatus \
  --jq '{reviewDecision,mergeStateStatus}'
```

Expected: required checks pass and `mergeStateStatus` is `CLEAN`. Follow the
repository's protected-branch merge procedure; do not bypass a failing check.

### Task 5: Append final evidence to the closed trackers

**Files:**
- External append-only record: GitHub issue `OpenCoven/coven#567`
- External append-only record: Bead `coven-psy-o1`

- [ ] **Step 1: Resolve the observed closure merge**

Run after the documentation PR merges:

```bash
CLOSURE_SHA="$(gh pr view \
  docs/psyche-o1-1-delivery-closure \
  --repo OpenCoven/coven \
  --json mergeCommit \
  --jq '.mergeCommit.oid // empty')"
test -n "$CLOSURE_SHA"
printf '%s\n' "$CLOSURE_SHA"
```

Expected: the exact GitHub merge commit, not a branch head.

- [ ] **Step 2: Build one bounded evidence statement**

Run:

```bash
EVIDENCE="O1.1 delivery closure merged at ${CLOSURE_SHA}. Corrective runtime: PR #622 at f68be0a0af373caf81780b70a5d3bf7d680e0f6e. Approved annex: PR #633 at c183d923e6d9e9d8172f39a193d624fe40095892. Conformance plan: PR #639 at 7f713ca3b3247b06d0fcff5aff3c756aa63c25eb. Conformance implementation: PR #664 at 3d18a53c309fad64cc177ad2d984e6b09de0ffc1 with all 12 hosted checks successful. Scope remains C-S1 vocabulary and C-S8 documentation only; C-S3-C-S6, C-S9-C-S12, G4, G6, and production child dispatch remain blocked."
printf '%s\n' "$EVIDENCE"
```

Expected: one statement containing every observed merge and every retained
scope exclusion.

- [ ] **Step 3: Append without changing tracker state**

Run:

```bash
gh issue comment 567 --repo OpenCoven/coven --body "$EVIDENCE"
bd comments add coven-psy-o1 "$EVIDENCE"
```

Expected: both closed trackers accept the append-only evidence.

- [ ] **Step 4: Verify both trackers remain closed**

Run:

```bash
gh issue view 567 --repo OpenCoven/coven \
  --json state,comments \
  --jq '{state,lastComment:.comments[-1].body}'
bd show coven-psy-o1
```

Expected: issue #567 and Bead `coven-psy-o1` remain closed and display the same
O1.1 closure evidence. Do not run an issue-close, issue-reopen, `bd close`, or
`bd reopen` command.
