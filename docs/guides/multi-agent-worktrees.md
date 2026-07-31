---
title: "Coordinate parallel work with Coven"
summary: "Use managed worktrees, TTL-bounded claims, and hooks to keep concurrent agent work from colliding."
read_when:
  - Starting a parallel change in a Coven-enabled repository
  - Checking whether another agent already owns work
description: "A practical workflow for coven wt, coven claim, and coven hooks."
---

# Coordinate parallel work with Coven

The parallel-work commands prevent duplicate work; they do not replace normal review or pull-request discipline. Run them from the repository you intend to change.

## 1. Check existing work

```sh
coven claim status
git worktree list
```

Look for an active claim that matches the issue or task before creating a branch. Claims are shared through git's common directory, so sibling worktrees see the same ownership state.

## 2. Create or enter an isolated worktree

```sh
coven wt docs/cli-core-guides
cd "$(coven wt docs/cli-core-guides)"
```

The printed path is designed for command substitution. Use a task-specific branch name; do not share an active worktree with another agent unless you have explicitly coordinated that work.

## 3. Acquire and maintain ownership

```sh
coven claim acquire issue-<N>
coven claim heartbeat issue-<N>
```

Claims are TTL-bounded. Heartbeat a long-running task before the claim expires. Release it from the same worktree after the pull request merges or when you stop:

```sh
coven claim release issue-<N>
```

## 4. Install managed hooks when the repository uses them

```sh
coven hooks install
coven wt --doctor
```

Managed hooks preserve existing hooks through a `.local` chain when appropriate. Do not replace a repository's tracked hook configuration by hand.

## Related

- [Worktree reference](/reference/cli-wt)
- [Claim reference](/reference/cli-claim)
- [Developer core-functionality guide](/development/cli-core-functionality)
- [Merge guide](/MERGE-GUIDE)
