# Incomplete Work Recovery Design

**Issue:** [#541](https://github.com/OpenCoven/coven/issues/541)

## Objective

Preserve, classify, recover, and deliver viable work from stale local branches
and dirty or formerly dirty worktrees without losing data, duplicating shipped
behavior, or mixing unrelated concerns in one pull request.

The recovery ends when every discovered workstream is in exactly one of these
states:

1. Delivered through a scoped pull request.
2. Proven already shipped or superseded and recorded as such.
3. Preserved in a durable recovery snapshot with a documented blocker.

Only after reaching one of those states may its stale branch or worktree be
removed.

## Current Inventory

The primary checkout is clean and matches `origin/main`. This inventory is a
point-in-time baseline rather than a promise about later execution state:
claims, open pull requests, and source-worktree dirtiness can change while the
recovery is being published or executed, so the plan must re-query GitHub and
snapshot the current state before each recovery action.

Source worktrees with preserved or in-flight local work at inventory time:

- `docs/psyche-specs` (`.worktrees/docs-psyche-specs`): Psyche product,
  technical, parity, threat-model, prerequisites, and plan documents.
- `feat/cmem-1ev-memory-promote` (`.worktrees/feat-cmem-1ev-memory-promote`):
  memory promotion authority, CLI wiring, privacy tooling, CI, and security
  documentation.
- `feat/mobile-memory-gateway` (`.worktrees/mobile-memory-gateway`): an
  uncommitted mobile pairing change.
- `fix/476-review-threads` (`.worktrees/pr-476-review`): Cave/runtime parity
  design and implementation plans.

Clean orphan branches without open pull requests:

- `docs/universal-runtime-capability-design`
- `feat/npm-macos-x64`
- `fix/521-ward-surface-confinement`

Several additional branches and worktree registrations correspond to merged
pull requests or missing temporary directories. They are hygiene candidates,
not feature inputs, but will not be removed until their tips are verified
against GitHub history.

A formerly dirty source worktree may later appear clean because its changes
were committed and pushed to an accurate open pull request. That is expected:
the recovery still snapshots the worktree's current status, preserves its
branch history, records empty patch evidence when applicable, and then decides
whether to continue the existing pull request or create a new recovery track.

## Recovery Architecture

### 1. Preserve before mutation

Create a durable recovery directory outside the repository worktrees. For each
source worktree, store its current state even if it is now clean because its
changes were already committed to an active pull request:

- `git status --short --branch`
- the base and head commit identifiers
- tracked working-tree and index patches
- copies of untracked files with their relative paths

For each orphan branch, create a Git bundle or equivalent ref-preserving
archive and record its commits relative to current `origin/main`.

Snapshots remain until the corresponding pull request is open and its source
branch is pushed, or until the work is explicitly recorded as blocked.

### 2. Classify against current authority

Compare each workstream with current `origin/main`, merged pull requests, closed
issues, current open pull requests from the exact source branch, and current
contracts. Assign one classification:

- **Already shipped:** main contains equivalent behavior or documentation.
- **Superseded:** a later implementation intentionally replaced the work.
- **Viable:** the intent remains useful and is not present on main.
- **Blocked:** viability cannot be established safely without a maintainer or
  human authority decision.

Classification uses semantic comparison, not commit ancestry alone, because
squash merges make many merged branches appear unmerged.

### 3. Recover viable concerns in isolation

Before creating a child issue, branch, or worktree, each viable concern maps to
its exact current source branch, reads that workstream's preserved local
`head.txt`, and first runs an exact-source-branch open-pull-request query. If
that query returns zero candidates, the concern follows the normal
issue-reuse-or-create flow without fetching the source branch. If the query
returns exactly one candidate open pull request, the recovery then fetches that
exact branch from `origin`, records the freshly fetched authoritative remote
tip, and verifies that the candidate's `state` is `OPEN`, its `headRefName`
matches the expected branch, its `headRepositoryOwner` is `OpenCoven`,
`isCrossRepository` is false, its `baseRefName` is `main`, and its
`headRefOid` exactly equals the freshly fetched authoritative source-branch
tip. Only after the PR head matches the fetched authoritative tip does the
recovery verify via ancestry that the preserved local `head.txt` equals or is
an ancestor of that PR head, so a clean local worktree that is behind its open
PR by commits can still be adopted safely. If more than one
exact-source-branch pull request exists, the row moves directly to `blocked`
with saved evidence rather than fetching or guessing. If the single-candidate
branch fetch fails, if the PR head differs from the fetched branch tip, if the
preserved local head is not an ancestor of the PR head, or if the candidate
pull request fails any other identity check including closing or merging
between list and view, the row also moves to `blocked` with saved evidence
rather than falling through to duplicate-recovery work.

Each viable concern without an adopted exact-source-branch open pull request
receives:

1. A dedicated GitHub issue, unless Task 4's paginated issue ledger contains
   exactly one non-PR open issue whose title exactly equals that concern's
   fixed recovery title.
2. A fresh branch and worktree based on current `origin/main`.
3. An issue-keyed Coven claim acquired from that worktree.
4. A minimal transplant of only the still-relevant changes.
5. Targeted tests followed by all repository-required gates.
6. A conventional commit, push, and scoped pull request.

Task 4's fully paginated `issues.json` capture is the authoritative reuse
source. Each workstream derives its own filtered issue-search evidence by
running `jq` across every page and keeping only non-PR issues whose title
exactly matches the fixed `ISSUE_TITLE` and whose state is `open`.
Zero exact-title open matches create a new issue, one exact-title open match is
reused and re-verified, and more than one exact-title open match blocks the row
with evidence. Human-friendly `gh issue list` output may still be useful for
operators, but it is informational only and never drives reuse. A sole
unrelated or closed result remains preserved in `issues.json` but does not get
reused.

For example, if the source branch `docs/psyche-specs` still has exactly one
open pull request from that exact source when the query runs, its
`headRefOid` matches the freshly fetched `origin/docs/psyche-specs` tip, and
the preserved local `head.txt` is equal to or an ancestor of that PR head, the
recovery adopts whichever PR GitHub returns at execution time instead of
creating a duplicate issue, branch, worktree, claim, or pull request.

Old commits are not blindly rebased or cherry-picked when current contracts
have changed. The recovered implementation is rebuilt around current code and
tests while retaining the original intent and attribution.

### 4. Clean only after proof

An original worktree, branch, claim, or stale registration may be removed only
after one of these proofs exists:

- its replacement pull request is open from a pushed branch;
- its behavior is identified in a merged pull request or current main;
- its snapshot and blocker record preserve all remaining value.

Cleanup includes stale `/tmp` worktree registrations, merged local branches,
gone upstream references, and expired claims. Unrelated user changes are never
discarded.

Before the final audit, the primary checkout is restored non-destructively to a
clean `main`. If it is already on `main`, the recovery fetches `origin/main`
and fast-forwards only when that is safe. If it is on a recovery-owned
issue-541 branch, the recovery first proves the branch head is fully pushed to
its configured upstream, then switches to `main` and fast-forwards from
`origin/main`. Any dirty tracked or untracked state, missing upstream,
unpushed recovery commits, unrelated current branch, or non-fast-forward `main`
state blocks the audit and leaves the checkout untouched.

### 5. Reconcile long-running goals

Update `.copilot/goals.md` after the recovery outcomes are known. Closed issues
such as #401, #414, and #521 must not remain listed as future work. Active goals
should contain only current objectives and a single concrete next action;
completed or superseded objectives move to `done`.

## Work Queue

Recovery proceeds sequentially to avoid overlapping claims and file changes:

1. Snapshot every dirty and orphaned workstream.
2. Perform the supersession and merged-history pass.
3. Recover the mobile pairing patch.
4. Recover Intel macOS npm packaging.
5. Reconcile Ward surface confinement.
6. Recover memory promotion and privacy tooling.
7. Recover Psyche specifications.
8. Recover universal runtime capability design.
9. Reconcile Cave/runtime parity plans.
10. Clean verified residue and update goals.

If classification shows a queued item is already shipped or superseded, record
the evidence and skip implementation without changing the order of the
remaining queue.

## Validation

Every viable code pull request runs targeted tests first, then:

```sh
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --locked
python scripts/check-secrets.py
python3 scripts/check-coven-privacy.py --staged
```

Changes to npm or TypeScript packages also run:

```sh
npm run build
npm test
```

Documentation-only pull requests run repository-provided documentation checks
when present, plus the secret and staged privacy guards. The privacy guard runs
after staging the intended change so it evaluates the actual proposed commit.

## Failure Handling

- Snapshot creation failure blocks all cleanup for that workstream.
- Rebase or transplant conflicts are resolved from current contracts; old code
  never wins automatically.
- A failing required gate blocks push and pull-request creation.
- More than one exact-source-branch open pull request, any candidate PR state,
  branch, owner, base, or cross-repo mismatch, any single-candidate branch
  fetch failure, any PR head that differs from the freshly fetched
  authoritative branch tip, any preserved local head that is not an ancestor
  of the PR head, or more than one exact-title open matching recovery issue
  blocks the row with evidence rather than choosing a duplicate target.
- Any unsafe primary-checkout restore condition blocks the final audit rather
  than forcing a branch switch or reset.
- Ambiguous ownership, policy, or human approval moves the workstream to
  `blocked` with evidence rather than inventing authority.
- Claims are heartbeated during long work and released when the pull request
  merges or the recovery session stops.
- Pull-request descriptions identify the recovered source and explain omitted
  portions that were obsolete or superseded.

## Success Criteria

- Every dirty, formerly dirty, or orphaned workstream has a durable snapshot
  and classification.
- Every viable concern either adopts one accurate exact-source-branch open pull
  request whose queried single candidate survived authoritative branch-tip and
  ancestry verification, or has its own validated, pushed branch and open pull
  request after the zero-candidate normal flow.
- Already-shipped and superseded work has concrete GitHub or main-branch
  evidence.
- No uncommitted work is deleted.
- Stale worktrees, branches, and claims are removed only after proof.
- `.copilot/goals.md` matches current issue and pull-request reality.
- The primary checkout remains clean and on `main`.
