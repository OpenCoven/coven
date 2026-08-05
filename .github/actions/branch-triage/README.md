# branch-triage action

Composite GitHub Action that classifies, merges, and prunes branches in a
repository. Designed for repos that accumulate many short-lived feature
branches and need periodic cleanup.

## What it does

1. **Classifies** every remote branch into one of four categories:

   | Category | Definition | Action |
   |---|---|---|
   | OPEN | Has an open PR | Merge if it passes the gate below |
   | MERGED | Has a merged PR | Delete local + remote |
   | SUPERSEDED | No PR, 0 unique commits vs base | Delete |
   | REVIEW | No PR, >0 unique commits | Skip — report only |

2. **Deletes** MERGED and SUPERSEDED branches (remote + local).
3. **Merges** each OPEN-PR branch that clears the merge gate.
4. **Skips** REVIEW branches — reports them in the job summary so a human can decide.
5. **Writes** a Markdown summary to the GitHub job summary.

## Inputs

| Input | Required | Default | Description |
|---|---|---|---|
| `base-branch` | no | `main` | Protected base branch |
| `merge-strategy` | no | `squash` | `squash`, `merge`, or `rebase` |
| `dry-run` | no | `false` | Classify only; no deletes or merges |
| `stale-days` | no | `30` | Zero-commit branches older than this are SUPERSEDED (0 = off) |
| `github-token` | no | `${{ github.token }}` | Token with repo write + PR write |

## Outputs

| Output | Description |
|---|---|
| `merged-count` | PRs merged |
| `deleted-count` | Branches deleted |
| `kept-count` | REVIEW branches left untouched |
| `skipped-count` | Open PRs the merge gate refused |

## Usage

```yaml
# Scheduled weekly + manual dispatch
on:
  schedule:
    - cron: "0 9 * * 1"   # every Monday 09:00 UTC
  workflow_dispatch:
    inputs:
      dry-run:
        default: "false"

permissions:
  contents: write
  pull-requests: write

jobs:
  triage:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
        with:
          fetch-depth: 0
      - uses: ./.github/actions/branch-triage
        with:
          dry-run: ${{ inputs.dry-run || 'false' }}
          github-token: ${{ secrets.GITHUB_TOKEN }}
```

## Merge gate

This action runs unattended, and on a repository without branch protection it
is the only thing standing between a red or contested PR and the default
branch. It therefore fails closed: a PR is merged only when **every** condition
below holds, and is otherwise reported in the job summary and left alone.

| Condition | Skipped when |
|---|---|
| Not a draft | PR is a draft |
| No blocking review | `reviewDecision` is `CHANGES_REQUESTED` |
| Mergeable | `mergeable` is not `MERGEABLE` (conflicting, or not yet computed) |
| No failing checks | any check is `FAILURE`, `TIMED_OUT`, `CANCELLED`, `STARTUP_FAILURE`, `ACTION_REQUIRED`, or `ERROR` |
| No in-flight checks | any check is `QUEUED`, `IN_PROGRESS`, `WAITING`, or `PENDING` |
| CI actually ran | the check rollup is empty — no CI ran at all |
| Merge state is clean | `mergeStateStatus` is anything other than `CLEAN` |

Mergeability that GitHub has not finished computing counts as a skip, not a
pass — a PR held for that reason merges on the next run.

The gate is evaluated **before** the action writes anything to the branch, so a
blocked PR is never rebased or force-pushed.

## Why there is no rebase step

Earlier versions rebased each OPEN-PR branch onto the base and force-pushed
before merging. That was removed:

- The force-push invalidated the very CI results the merge decision rests on —
  the action merged against checks that described the pre-rebase commits.
- Being behind the base does not block a merge unless branch protection
  requires strictly up-to-date branches; where it does, `mergeStateStatus` is
  `BEHIND` or `BLOCKED` and the gate skips the PR for a human to refresh.
- The conflict auto-resolver it depended on could not work: its
  `git diff | while read` loop ran in a subshell, so the `continue 2` meant to
  abandon a conflicted PR never propagated to the outer loop, and
  `git rebase --abort` ran against a subshell that was about to exit.

A PR that conflicts with the base is now reported and left for manual
resolution.

## Relationship to Coven skill

The same workflow is available as an internal OpenCoven familiar skill at
`~/.coven/skills/git-branch-triage/SKILL.md`. The skill version is
interactive (asks before destructive actions); this Action version is
automated and runs headless on a schedule.
