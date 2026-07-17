---
summary: "Create, list, diagnose, and prune Coven protocol worktrees."
read_when:
  - Looking up wt
  - Isolating parallel agent sessions in worktrees
title: "coven wt"
description: "Reference for coven wt: create or enter a branch worktree in the sibling <repo>.wt directory, list worktrees with claim and dirty state, run the layout doctor, and prune merged or stale worktrees."
---

`coven wt` keeps every parallel session in its own git worktree so git
operations do not race. Worktrees live in a sibling directory named after the
repository: a repo at `~/src/coven` keeps its worktrees under
`~/src/coven.wt/<branch>`.

```sh
coven wt fix/output-polish   # create (or re-enter) a worktree for the branch
coven wt --list              # branch, dirty state, active claim, path
coven wt --doctor            # layout + hook check; exits 1 on problems
coven wt --doctor --json     # same checks, machine-readable
coven wt --prune-merged      # remove clean worktrees merged into the primary
coven wt --prune-stale 14    # remove clean worktrees untouched for 14 days
```

Exactly one action per invocation. `coven worktree` and `coven worktrees` are
aliases.

## Creating and entering

`coven wt <branch>` prints the worktree path. If the worktree already exists
it is reused; if the branch exists it is checked out, otherwise the branch is
created. The printed path is designed for command substitution:

```sh
cd "$(coven wt fix/output-polish)"
```

## Listing with claims

`--list` joins each worktree against the shared claim registry (see
[cli-claim](cli-claim.md)) and the working-tree state:

```sh
coven wt --list --json
```

```json
{
  "worktrees": [
    {
      "branch": "fix/output-polish",
      "dirty": false,
      "claimed_by": "buns",
      "path": "/home/alex/src/coven.wt/fix/output-polish"
    }
  ]
}
```

`claimed_by` is null unless an active (unexpired) claim exists for the
branch.

## Doctor

`--doctor` prints the repo root, the expected worktree root, the claims
directory, and whether the managed `pre-commit`/`pre-push` hooks are
installed. It warns about worktrees outside the `<repo>.wt` layout and exits
1 when hooks are missing or the layout is broken — install the hooks with
`coven hooks install` ([cli-claim](cli-claim.md)).

`--doctor --json` emits the same machine-readable shape as the other doctor
surfaces (`coven doctor --json`, `coven adapter doctor --json`), with the
identical exit-code semantics:

```sh
coven wt --doctor --json
```

```json
{
  "ok": false,
  "blocking": true,
  "repo": "/home/alex/src/coven",
  "worktree_root": "/home/alex/src/coven.wt",
  "claims_dir": "/home/alex/src/coven/.git/agent-claims",
  "checks": [
    {
      "id": "hook:pre-commit",
      "status": "pass",
      "message": "managed pre-commit hook installed"
    },
    {
      "id": "hook:pre-push",
      "status": "fail",
      "message": "managed pre-push hook missing",
      "hint": "install the managed hooks with `coven hooks install`"
    },
    {
      "id": "layout",
      "status": "pass",
      "message": "all worktrees are under /home/alex/src/coven.wt"
    }
  ]
}
```

Check `id`s are stable: `hook:pre-commit`, `hook:pre-push`, and `layout`.
Every `fail` is blocking (exit 1).

## Pruning

Both prune modes only ever remove **clean** worktrees. `--prune-merged`
removes worktrees whose branches are merged into the primary branch
(`COVEN_PRIMARY_BRANCH`, default `main`); `--prune-stale <DAYS>` removes
worktrees not modified for the given number of days.
