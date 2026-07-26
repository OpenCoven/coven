# Worktree-Scoped Claim Identity Design

## Problem

When `COVEN_AGENT_ID` is unset, the claim CLI and managed pre-commit hook use
`USER` as the owner identity. Two agent sessions running as the same OS user
therefore look like one owner, even when they occupy different worktrees. A
second session can re-acquire, heartbeat, release, or commit against the first
session's claim.

## Identity contract

An explicit, non-blank `COVEN_AGENT_ID` remains authoritative and unchanged.
Without it, Coven derives:

```text
<USER-or-unknown-agent>@<worktree-slug>
```

`worktree-slug` is the sanitized basename of `git rev-parse --show-toplevel`,
using the claim filename sanitizer. This is stable across commands in one
worktree, distinct for the repository's normal sibling worktrees, readable in
`claim status`, and implementable identically in Rust and POSIX shell. If the
basename sanitizes to an empty string, Coven uses `worktree`.

Two agents sharing one worktree still share the fallback identity. Harnesses
that need finer separation must set distinct `COVEN_AGENT_ID` values.

## Workflow contract

Because the fallback identity is worktree-scoped, sessions must enter or create
their task worktree before acquiring an issue-keyed claim. Repository agent
guidance and merge documentation will use this order:

1. Check the shared claim registry and open PRs.
2. Create or enter the task worktree.
3. Acquire `issue-<N>` from inside that worktree before editing code.
4. Heartbeat and release from the same worktree.

## Surfaces

- `crates/coven-cli/src/parallel_protocol.rs`: derive the fallback identity
  from `Repo::root` and use the same shell algorithm in the managed hook.
- `crates/coven-cli/tests/parallel_protocol.rs`: prove cross-worktree
  exclusion, same-worktree lifecycle behavior, explicit-ID compatibility, and
  managed-hook parity.
- `skills/coven-parallel-work/hooks/pre-commit`: keep the reference hook
  aligned with the canonical managed hook.
- `AGENTS.md`, `docs/MERGE-GUIDE.md`,
  `docs/familiars/parallel-lanes.md`, `docs/reference/cli-claim.md`,
  `docs/reference/cli.md`, `skills/coven-parallel-work/PROTOCOL.md`, and
  `skills/coven-parallel-work/SKILL.md`: document the new default and command
  ordering.

## Error handling

Explicit blank IDs fall back instead of creating an empty owner. A missing or
blank `USER` uses `unknown-agent`. Identity derivation never prevents claim
operations solely because a worktree basename is unusual.

## Acceptance evidence

- Same `USER`, different worktrees, no explicit ID: the first acquire succeeds
  and the second is refused.
- One worktree, no explicit ID: acquire, heartbeat, and release all succeed.
- One explicit ID across worktrees retains the existing explicit-owner
  behavior.
- A managed pre-commit hook computes the same fallback owner as the CLI.
- All claim-protocol tests and repository CI gates pass.
