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

Create a durable recovery directory outside the repository worktrees. The fixed
current-run path remains `agent-recovery/issue-541`, but reruns must never
reuse it in place: if that path already exists, move the entire prior run into
an immutable private archive directory under the shared recovery archive, record
its former location and archival time, then create a fresh current root at the
same fixed path before writing any new manifest or snapshot. Publication and
execution are driven from a controller worktree for
`docs/541-incomplete-work-recovery-design` that is discovered with `git worktree
list --porcelain` from any `OpenCoven/coven` worktree or, when absent,
recreated at the deterministic repo-local `.worktrees/issue-541-recovery`
location after verifying `.worktrees/` is ignored. A discovered registration
whose directory no longer exists is treated as stale/absent rather than as an
immediate hard failure; the recovery may use the minimal documented
`git worktree add --force` exception only when that missing registration is
proven stale and the controller branch is not present in any live worktree.
Before any live controller is used or any replacement controller is created,
the recovery first fetches `origin/docs/541-incomplete-work-recovery-design`
into its remote-tracking ref. One live controller is acceptable only when its
worktree is clean and can fast-forward-only to that freshly fetched remote tip;
local ahead state, divergence, or dirtiness blocks. With zero live controllers,
the local controller branch may be created from the fetched remote tip or
fast-forwarded to it, but any force-rewrite requirement blocks. Every
operational command block re-derives `CONTROL_WORKTREE`, `COMMON_DIR`, and
`REPO` instead of assuming a fixed local path or persistent shell variables,
and every resolved or recreated controller path is re-verified on the exact
branch with `HEAD` equal to that freshly fetched remote tip before the plan
continues. For each source worktree, store its current
state even if it is now clean because its changes were already committed to an
active pull request:

- `git status --short --branch`
- the base and head commit identifiers
- tracked working-tree and index patches
- a private NUL-delimited `.untracked.zlist` plus JSON-escaped untracked
  evidence derived from that inventory
- an uncompressed `untracked.tar` created from the source worktree with
  `tar --null -T` so newline pathnames, symlink entries, and file metadata are
  preserved losslessly
- a private NUL-delimited `.ignored.zlist` plus JSON-escaped ignored evidence
  derived from that inventory, without archiving ignored content itself

For each orphan branch, create a Git bundle or equivalent ref-preserving
archive and record its commits relative to current `origin/main`.

Reruns therefore create a new current run while every prior run remains
immutable under the archive directory. Snapshots remain until the corresponding
pull request is open and its source branch is pushed, or until the work is
explicitly recorded as blocked. Snapshot completeness validates the
NUL-delimited inventories, their JSON evidence, and the uncompressed untracked
tar archives, including valid empty inventories.

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
squash merges make many merged branches appear unmerged. Branch commit/stat/
cherry evidence also uses a fixed seven-row map from workstream IDs to the
preserved `head.txt` files captured in the dirty-worktree and orphan-branch
snapshots. Before any comparison runs, each referenced SHA must still verify as
a commit and its paired bundle must still verify, so the source side remains
immutable even if a live local branch later moves.

### 3. Recover viable concerns in isolation

Before creating a child issue, branch, or worktree, each viable concern maps to
its exact current source branch, reads that workstream's preserved local
`head.txt`, and first captures a fully paginated REST/API list of open pull
requests for `OpenCoven/coven`. Candidate selection then happens with local
`jq` filtering only when `.head.ref` equals the expected source branch and
`.head.repo.full_name` equals `OpenCoven/coven`, so same-named fork PRs never
count as same-repository candidates. If that same-repo exact-source-branch
filter returns zero candidates, the concern follows the normal
issue-reuse-or-create flow without fetching the source branch. If the filter
returns exactly one candidate open pull request, the recovery then fetches that
exact branch from `origin`, records the freshly fetched authoritative remote
tip, and verifies that the candidate's `state` is `OPEN`, its `headRefName`
matches the expected branch, its `headRepositoryOwner` is `OpenCoven`,
`isCrossRepository` is false, its `baseRefName` is `main`, and its
`headRefOid` exactly equals the freshly fetched authoritative source-branch
tip. Only after the PR head matches the fetched authoritative tip does the
recovery verify via ancestry that the preserved local `head.txt` equals or is
an ancestor of that PR head, so a clean local worktree that is behind its open
PR by commits can still be adopted safely.

For dirty-worktree sources, successful identity and ancestry verification is
still not enough to adopt the existing PR. The recovery must next inspect the
preserved snapshot's `worktree.patch`, `index.patch`, and
`.untracked.zlist`; adoption is allowed only when all three dirty classes
are empty. If any preserved dirty delta remains, the row moves to `blocked`
with evidence naming the existing PR URL, the preserved snapshot archive IDs,
and the safe resume condition: land or reconcile that PR first, then recover
the preserved delta from current `main` in a separate scoped track. That
evidence cites the preserved `.untracked.zlist`, `untracked.json`, and
`untracked.tar` alongside the tracked patch artifacts. While that blocker
stands, the original source worktree and branch remain untouched. If more than
one same-repo exact-source-branch pull request exists, the row moves directly
to `blocked` with saved evidence rather than fetching or guessing. If the
single-candidate branch fetch fails, if the PR head differs from the fetched
branch tip, if the preserved local head is not an ancestor of the PR head, or
if the candidate pull request fails any other identity check including closing
or merging between list and view, the row also moves to `blocked` with saved
evidence rather than falling through to duplicate-recovery work.

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
same-repo open pull request from that exact source when the query runs, its
`headRefOid` matches the freshly fetched `origin/docs/psyche-specs` tip, the
preserved local `head.txt` is equal to or an ancestor of that PR head, and the
preserved `worktree.patch`, `index.patch`, and `.untracked.zlist` are all
empty, the recovery adopts whichever PR GitHub returns at execution time
instead of creating a duplicate issue, branch, worktree, claim, or pull
request.

Old commits are not blindly rebased or cherry-picked when current contracts
have changed. The recovered implementation is rebuilt around current code and
tests while retaining the original intent and attribution.

### 4. Clean only after proof

An original worktree, branch, claim, or stale registration may be removed only
after one of these proofs exists:

- its replacement pull request is open from a pushed branch;
- its behavior is identified in a merged pull request or current main;
- its snapshot and blocker record preserve all remaining value.

Cleanup includes stale worktree registrations, merged local branches,
gone upstream references, and expired claims. Rows blocked because an existing
PR already covers the preserved head while additional dirty snapshot state
remains do not qualify for source-worktree or source-branch cleanup; those
original sources stay in place until the blocked delta is recovered separately.
Force-removal is also prohibited for any source worktree whose preserved
`.ignored.zlist` is non-empty. Before any forced removal, the recovery
regenerates the live `.untracked.zlist`, `untracked.tar`, and `.ignored.zlist`
with the same lossless method used for the snapshot and compares them
byte-for-byte to the preserved artifacts. Any ignored content or inventory
drift blocks removal, records blocker evidence with a resume condition, and
leaves the source worktree untouched. Unrelated user changes are never
discarded. Exact preserved-head equality remains the default local branch
deletion proof. The only allowed exception is a viable adopted source-branch
row whose exact expected branch matches the branch being deleted, whose live
local tip equals a freshly fetched `origin/<branch>` tip, whose current PR is
still the OPEN same-repo `main` PR at that exact branch/tip, and whose
preserved snapshot head is an ancestor of the advanced live tip.

Before the final audit, the primary checkout is restored non-destructively to a
clean `main`. If it is already on `main`, the recovery fetches `origin/main`
and fast-forwards only when that is safe. If it is on a recovery-owned
issue-541 branch, the recovery first proves the branch head is fully pushed to
its configured upstream, then switches to `main` and fast-forwards from
`origin/main`. Any dirty tracked or untracked state, missing upstream,
unpushed recovery commits, unrelated current branch, or non-fast-forward `main`
state blocks the audit and leaves the checkout untouched.
The final audit parses exact classification rows rather than label-only grep.
Viable rows are terminal only when `Main/PR evidence` is the raw
`OpenCoven/coven` PR URL and `Recovery action` is already in a terminal
PR-backed mode (`continue-existing-pr` or `recovery-pr-open`) with the
required metadata; `mode=awaiting-recovery-pr` fails the terminal audit.

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
4. Recover or block Intel macOS npm packaging.
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

Use repository-native npm and Node validation based on the touched paths:

- If `packages/channels` is touched, run `npm ci`, `npm run build`, and
  `npm test` with `working-directory=packages/channels` (or the exact
  `npm --prefix packages/channels ...` equivalents).
- If npm CLI wrapper or platform packaging is touched (`packages/cli`,
  `npm/coven`, platform package manifests, or the publish/prepublish scripts),
  run the supported `node scripts/test-cli-prepublish.mjs` smoke for the
  affected target and pair it with the matching release build plus the cargo
  gates above. The current supported matrix is
  `macos`/`aarch64-apple-darwin`, `linux-x64`/`x86_64-unknown-linux-gnu`, and
  `windows`/`x86_64-pc-windows-msvc`. Current main cannot validate Intel x64,
  and `--target=macos` must never be used as a proxy for Intel recovery. A
  viable child design/plan must first restore the concrete
  `macos-x64`/`@opencoven/cli-macos-x64` contract in current code, with tests
  proving default darwin x64 mapping and package metadata. After that contract
  exists, the exact Intel validation command is
  `node scripts/test-cli-prepublish.mjs --target=macos-x64 --with-cargo-gates`,
  plus any targeted Node tests documented by the child plan.
- If `packages/openclaw-coven` is touched, run `npm install` and
  `npm exec vitest run` with `working-directory=packages/openclaw-coven`.
  Do not claim nonexistent package-local build or test scripts there.
- `packages/cli` and `npm/coven` have no package-local `npm run build` or
  `npm test` scripts; validate them through the prepublish smoke and the
  relevant Node tests that it already executes.

Documentation-only pull requests run repository-provided documentation checks
when present, plus the secret and staged privacy guards. The privacy guard runs
after staging the intended change so it evaluates the actual proposed commit.

## Failure Handling

- Snapshot creation failure blocks all cleanup for that workstream.
- Rebase or transplant conflicts are resolved from current contracts; old code
  never wins automatically.
- A failing required gate blocks push and pull-request creation.
- More than one same-repo exact-source-branch open pull request, any candidate
  PR state, branch, owner, base, or cross-repo mismatch, any single-candidate
  branch fetch failure, any PR head that differs from the freshly fetched
  authoritative branch tip, any preserved local head that is not an ancestor
  of the PR head, any non-empty preserved dirty snapshot class behind an active
  PR, or more than one exact-title open matching recovery issue blocks the row
  with evidence rather than choosing a duplicate target.
- If `agent-recovery/issue-541` already exists, archival move or fresh-current
  root creation failure blocks the rerun before any new manifest or snapshot is
  written.
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
- Every viable concern either adopts one accurate same-repo exact-source-branch
  open pull request whose queried single candidate survived authoritative
  branch-tip, ancestry, and empty-dirty-snapshot verification, or has its own
  validated, pushed branch and open pull request after the zero-candidate
  normal flow.
- Already-shipped and superseded work has concrete GitHub or main-branch
  evidence.
- No uncommitted work is deleted.
- Stale worktrees, branches, and claims are removed only after proof.
- Every rerun leaves the prior `issue-541` recovery root immutable under the
  private archive and recreates a fresh current root at the fixed path.
- `.copilot/goals.md` matches current issue and pull-request reality.
- The primary checkout remains clean and on `main`.
