# Incomplete Work Recovery Design

**Issue:** [#541](https://github.com/OpenCoven/coven/issues/541)

## Objective

Preserve, classify, recover, and deliver viable work from stale local branches
and dirty worktrees without losing data, duplicating shipped behavior, or
mixing unrelated concerns in one pull request.

The recovery ends when every discovered workstream is in exactly one of these
states:

1. Delivered through a scoped pull request.
2. Proven already shipped or superseded and recorded as such.
3. Preserved in a durable recovery snapshot with a documented blocker.

Only after reaching one of those states may its stale branch or worktree be
removed.

## Current Inventory

The primary checkout is clean and matches `origin/main`. There are no open pull
requests, active claims, stashes, or session todos predating this recovery.

Dirty worktrees:

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

## Recovery Architecture

### 1. Preserve before mutation

Create a durable recovery directory outside the repository worktrees. For each
dirty worktree, store:

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
issues, and current contracts. Assign one classification:

- **Already shipped:** main contains equivalent behavior or documentation.
- **Superseded:** a later implementation intentionally replaced the work.
- **Viable:** the intent remains useful and is not present on main.
- **Blocked:** viability cannot be established safely without a maintainer or
  human authority decision.

Classification uses semantic comparison, not commit ancestry alone, because
squash merges make many merged branches appear unmerged.

### 3. Recover viable concerns in isolation

Each viable concern receives:

1. A dedicated GitHub issue, unless an existing issue accurately owns it.
2. A fresh branch and worktree based on current `origin/main`.
3. An issue-keyed Coven claim acquired from that worktree.
4. A minimal transplant of only the still-relevant changes.
5. Targeted tests followed by all repository-required gates.
6. A conventional commit, push, and scoped pull request.

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
- Ambiguous ownership, policy, or human approval moves the workstream to
  `blocked` with evidence rather than inventing authority.
- Claims are heartbeated during long work and released when the pull request
  merges or the recovery session stops.
- Pull-request descriptions identify the recovered source and explain omitted
  portions that were obsolete or superseded.

## Success Criteria

- Every dirty or orphaned workstream has a durable snapshot and classification.
- Every viable concern has its own validated, pushed branch and open pull
  request.
- Already-shipped and superseded work has concrete GitHub or main-branch
  evidence.
- No uncommitted work is deleted.
- Stale worktrees, branches, and claims are removed only after proof.
- `.copilot/goals.md` matches current issue and pull-request reality.
- The primary checkout remains clean and on `main`.
