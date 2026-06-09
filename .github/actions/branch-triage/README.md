# branch-triage action

Composite GitHub Action that classifies, rebases, merges, and prunes
branches in a repository. Designed for repos that accumulate many
short-lived feature branches and need periodic cleanup.

## What it does

1. **Classifies** every remote branch into one of four categories:

   | Category | Definition | Action |
   |---|---|---|
   | OPEN | Has an open PR | Rebase onto base + merge |
   | MERGED | Has a merged PR | Delete local + remote |
   | SUPERSEDED | No PR, 0 unique commits vs base | Delete |
   | REVIEW | No PR, >0 unique commits | Skip — report only |

2. **Deletes** MERGED and SUPERSEDED branches (remote + local).
3. **Rebases** each OPEN-PR branch onto the base, then merges the PR.
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

## Conflict resolution policy

When rebasing an OPEN-PR branch:
- Files **not owned by the branch** (not in its diff vs base) → take base version automatically.
- Files **owned by the branch** → rebase is aborted and the PR is skipped with a warning; resolve manually.

This avoids silently dropping logic or security changes the branch intentionally introduces.

## Relationship to Coven skill

The same workflow is available as an internal OpenCoven familiar skill at
`~/.coven/skills/git-branch-triage/SKILL.md`. The skill version is
interactive (asks before destructive actions); this Action version is
automated and runs headless on a schedule.
