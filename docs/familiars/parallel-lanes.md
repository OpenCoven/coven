---
summary: "Run specialist familiars in parallel on the same task."
read_when:
  - Splitting work across familiars
title: "Parallel specialist lanes"
description: "Parallel specialist lanes in OpenCoven: running multiple familiars side-by-side so each focuses on its own role and harness without blocking the others."
---

Parallel lanes let several agents work in the same repository without sharing
the same mutable checkout. Coven treats this as a harness-agnostic protocol:
Codex, Claude Code, OpenClaw familiars, CastCodes agents, and future runtimes
can all participate as long as they use the same git worktree, claim, and hook
contracts.

## The protocol

The Coven Parallel Work Protocol has three layers:

1. Worktree isolation: feature work happens in a sibling `<repo>.wt/<branch>/`
   checkout instead of the primary repository checkout.
2. Agent claims: a branch can be claimed under git's common directory with a
   TTL so other agents know not to commit into the same branch.
3. Branch protection hooks: local git hooks block accidental primary-branch
   commits, cross-agent claim conflicts, protected force-pushes, and protected
   pushes without explicit merge intent.

Use layer 1 by default. Add layers 2 and 3 whenever more than one agent,
automation, or background worker can touch the same repository.

## CLI

Create or enter an isolated worktree:

```bash
coven wt feature/my-branch
```

List and diagnose worktrees:

```bash
coven wt --list
coven wt --doctor
```

Prune clean worktrees:

```bash
coven wt --prune-merged
coven wt --prune-stale 14
```

Claim a branch for the current agent:

```bash
COVEN_AGENT_ID=cody coven claim acquire feature/my-branch
coven claim heartbeat feature/my-branch
coven claim release feature/my-branch
coven claim status
```

Record a HEAD canary for the current branch:

```bash
coven claim canary feature/my-branch
```

Install local guard hooks:

```bash
coven hooks install
```

The installer writes managed `pre-commit` and `pre-push` hooks into
`.git/hooks`. If a hook already exists, Coven moves it to `<hook>.local` and
chains it. If `core.hooksPath` points at a tracked hook directory, Coven refuses
to modify it automatically and prints integration options.

## Environment

| Variable | Default | Purpose |
| --- | --- | --- |
| `COVEN_PRIMARY_BRANCH` | `main` | Primary branch protected by pre-commit and pre-push. |
| `COVEN_PROTECTED_REGEX` | `^(release\|hotfix)/` | Additional protected branch names for pre-push. |
| `COVEN_MERGE_PHRASE` | `Enchant merge to main.` | Required exact text in `.git/MERGE_INTENT` before protected pushes. |
| `COVEN_AGENT_ID` | `$USER` | Stable agent identity for claim ownership. |
| `COVEN_CLAIM_TTL_SECONDS` | `3600` | Claim lifetime before another agent may acquire it. |
| `COVEN_ALLOW_PRIMARY_COMMIT` | unset | Set to `1` only for explicit human-approved primary commits. |

## Files

All claim and hook state lives under git's common directory, so linked worktrees
share the same protocol state:

```text
<git-common-dir>/agent-claims/<branch-slug>
<git-common-dir>/AGENT_HEAD_AT_START
<git-common-dir>/MERGE_INTENT
<repo>.wt/<branch-slug>/
```

Claim files use simple `key=value` fields so shell hooks can read them without a
runtime dependency.
