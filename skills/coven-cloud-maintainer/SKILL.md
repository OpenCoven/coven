---
name: coven-cloud-maintainer
description: >
  Coordinate, checkpoint and resume OpenCoven/coven issue work across local
  worktrees, separate Cloud VMs and later sessions, using GitHub issues as the
  only durable state. Use before editing an issue, at every delivery
  milestone, when pausing, and when resuming work another session started.
version: 1.0.0
tags: [github, checkpoint, handoff, cloud, opencoven, coven]
---

# Coven cloud maintainer

A session on a Cloud VM shares nothing with a session on a laptop: not the
filesystem, not the git common directory, not the claim registry, not the chat.
The only state both can read is GitHub. This skill defines how to read and
write that state so any later session can pick work up without hidden context.

## What this skill composes

It adds one layer, coordination across environments, and defers everything
else. When it seems to disagree with one of these, the other document wins.

- [`AGENTS.md`](../../AGENTS.md): claims, task worktrees, branch and PR
  workflow, local gates and the Policy guard, attribution.
- [`docs/work-tracking.md`](../../docs/work-tracking.md): GitHub issues are the
  sole work tracker. This skill adds no database, cache or lock service.
- [`skills/pr-agent`](../pr-agent/SKILL.md): the readiness packet in the PR
  body. A checkpoint links the PR; it does not replace the packet.
- [`skills/coven-parallel-work`](../coven-parallel-work/SKILL.md): the
  same-machine protocol (worktree isolation, claims, hooks). In this repository
  the AGENTS.md worktree layout and `coven claim` are its implementation.

## Which signal covers what

| Signal | Visible to | Tells you |
| --- | --- | --- |
| `coven claim status` | worktrees sharing this checkout's git common directory | another session in *this checkout* is editing |
| Issue assignee | everyone | who is accountable; every session may run as the same account, so it is not a session lock |
| Latest checkpoint comment | everyone | which session and environment worked last, its branch and full SHA, what comes next |
| Open PR for the issue | everyone | the delivery vehicle to reuse instead of opening another |

Two rules follow, and neither has exceptions:

1. A local claim is never a cross-VM lock. Acquire it anyway, because other
   worktrees of the same checkout rely on it.
2. An empty claim registry never means the issue is unowned. A checkpoint from
   another ENVIRONMENT posted less than two hours ago means the work may be live
   elsewhere: comment on the issue and coordinate before editing. After that
   window, resume and say so in your first checkpoint.

## Before editing: the conflict check

Use REST, not GraphQL: the GraphQL quota is shared by every session on the
account and runs out first.

```sh
gh api repos/OpenCoven/coven/issues/<N>                              # state, assignees, body
gh api 'repos/OpenCoven/coven/issues/<N>/comments?per_page=100'      # latest checkpoint
python3 scripts/delivery/checkpoint.py conflicts --issue <N> --environment "<this environment>"
```

`conflicts` gathers read-only state (issue, comments, all PRs, `git ls-remote
--heads origin`, `coven claim status`) and exits 4 on a blocking finding:

- `open-pr`: an open PR names the issue in its branch, title or body. Reuse it.
- `local-claim`: another agent in this checkout holds `issue-<N>`.
- `recent-checkpoint-elsewhere`: rule 2 above.

It also reports a pushed branch with no PR, closed PRs, and a stale checkpoint
(the checkpoint's branch has moved past its FULL_SHA). Divergent branch names
are matched by issue number. A duplicate whose branch, title and body never
mention the number can only be found by keyword:
`gh api 'search/issues?q=repo:OpenCoven/coven+is:pr+<keywords>'`.

Then follow AGENTS.md: create or enter the task worktree, run
`coven claim acquire issue-<N>` inside it, and assign the issue if nobody is.

## Checks

The check surface is `scripts/dev-verify.sh`:

```sh
bash scripts/dev-verify.sh --quick   # while iterating: pins, fmt, check, secrets, privacy
bash scripts/dev-verify.sh --pr      # before every push: adds clippy, workspace tests, the CI Policy guard inventory, dependency audit
bash scripts/dev-verify.sh --full    # native, release-sensitive or feature-suite changes
bash scripts/dev-verify.sh --pr --plan   # print the plan and skips; never reports PASS
```

Put the exact command and its final `Local <tier> result: <n>` line in CHECKS,
plus any focused suite you ran. A missing tool fails its check (exit 127):
record it under BLOCKERS rather than dropping to a lower tier. A local pass is
not CI acceptance, and CI evidence is a run URL, never "should pass".

## The checkpoint

Post one checkpoint, in one fenced `checkpoint` block, at every material
milestone: the plan, before a push, PR opened, CI result, blocked, paused,
merged, verified on main. Prose, tables and links go around the block.

````markdown
```checkpoint
ISSUE: #1451 (parent #1444)
OWNER: @BunsDev; claude-code session; claim issue-1451 in worktree coven-1451-cloud-maintainer
ENVIRONMENT: local macOS arm64 worktree, not a managed Cloud VM
BRANCH: feat/1451-cloud-maintainer-skill
FULL_SHA: <40-character commit>
PR: none
COMPLETED: checkpoint schema and validator; conflict check
CHECKS: `bash scripts/dev-verify.sh --pr` -> Local pr result: 0
  `python3 scripts/delivery/checkpoint-test.py` -> OK
DECISIONS: none
BLOCKERS: none
NEXT: open the PR, then post its CI run here
```
````

| Field | Rule |
| --- | --- |
| ISSUE | starts with `#<n>` or `owner/repo#<n>` |
| OWNER | the accountable login and the session/claim identity |
| ENVIRONMENT | where this ran; say plainly when it is not a managed Cloud VM |
| BRANCH | one branch name, or `(detached HEAD)` |
| FULL_SHA | the full 40-character commit; short SHAs are rejected |
| PR | `none`, `#<n>` or a pull request URL |
| COMPLETED | acceptance rows done, with evidence links |
| CHECKS | exact commands and results; failures and skips included |
| DECISIONS | owner or design decisions taken; `none` if none |
| BLOCKERS | each blocker and who can clear it; `none` if none |
| NEXT | the first exact action or the human decision needed; never `none` |
| FILES | optional: files changed |

Continuation lines are indented. OWNER, ENVIRONMENT, CHECKS and NEXT cannot be
`none`. Never put tokens, home-directory paths or raw tool stderr in a
checkpoint. Validate before posting:

```sh
python3 scripts/delivery/checkpoint.py render --from-git ISSUE='#<N>' OWNER=... ENVIRONMENT=... PR=none \
  COMPLETED=... CHECKS=... DECISIONS=none BLOCKERS=none NEXT=... > checkpoint.md
python3 scripts/delivery/checkpoint.py validate checkpoint.md
```

Older free-form checkpoints predate the block. `checkpoint.py audit <file>`
lists which fields one names and which it lacks; treat a missing field as
unknown, never as "none".

## Pausing

1. Push the branch only if the AGENTS.md gates pass; otherwise leave it local
   and say so under BLOCKERS.
2. Post a checkpoint whose NEXT is the first exact action for whoever resumes.
3. `coven claim release issue-<N>` from the same worktree.

## Resuming without hidden context

Everything needed is on GitHub; do not rely on a previous chat or a local file.

1. Run the conflict check above.
2. Validate the latest checkpoint against the live branch head:
   `python3 scripts/delivery/checkpoint.py validate checkpoint.md --head "$(git ls-remote origin refs/heads/<branch> | cut -f1)"`.
   Exit 3 means the branch moved: read `git log <FULL_SHA>..origin/<branch>`
   and the PR timeline before trusting COMPLETED or NEXT.
3. Recreate the worktree from the PR branch, acquire the claim, and rerun the
   checks the new commits invalidate.
4. Post a checkpoint that says you resumed, from which comment, and at which SHA.
