---
summary: "TTL-bounded branch claims for parallel agent sessions."
read_when:
  - Looking up claim
  - Coordinating multiple agent sessions on one checkout
title: "coven claim"
description: "Reference for coven claim: acquire, release, heartbeat, canary, and status for the TTL-bounded claims that stop parallel agent sessions from duplicating work, plus the managed hooks that enforce them."
---

Parallel agent sessions (Codex, Claude Code, familiars) frequently work on the
same checkout at once. Claims are shared, TTL-bounded locks that make intent
visible before code changes exist, so two sessions do not independently build
the same issue.

```sh
coven claim status              # what is already taken?
coven claim acquire issue-42    # claim before touching code
coven claim heartbeat issue-42  # extend the TTL on long tasks
coven claim release issue-42    # release when the PR merges or you stop
```

Claim tokens are free-form. Prefer shared, issue-keyed tokens (`issue-42`)
over working-branch names, which other sessions cannot predict.

## Where claims live

Claims are files under `<git-common-dir>/agent-claims/`, so every worktree of
the repository sees the same registry. The claiming identity comes from
`COVEN_AGENT_ID` (falling back to `USER`). A claim expires after one hour by
default; override with `COVEN_CLAIM_TTL_SECONDS`.

- `acquire` fails while another agent's claim is active.
- `release` refuses to remove another agent's active claim.
- `heartbeat` re-acquires or extends your own claim for another TTL window.
- `canary <branch>` records the current HEAD in `<git-common-dir>/AGENT_HEAD_AT_START`
  so the managed pre-commit hook can detect history rewrites.

## Scriptable status

```sh
coven claim status --json
```

```json
{
  "claims": [
    {
      "branch": "issue-42",
      "agent_id": "buns",
      "state": "active",
      "acquired_at": 1784078000,
      "acquired_at_rfc3339": "2026-07-15T01:53:20Z",
      "expires_at": 1784081600,
      "expires_at_rfc3339": "2026-07-15T02:53:20Z",
      "head": "852b794..."
    }
  ]
}
```

`state` is `active` or `expired`; expired claims stay listed until the token
is re-acquired or released.

## Enforcement: coven hooks install

```sh
coven hooks install
```

installs managed `pre-commit` and `pre-push` hooks into
`<git-common-dir>/hooks/`, shared by all worktrees. Installation refuses to
touch tracked hook directories: when `core.hooksPath` is set, run the Coven
checks from that hook directory instead (or move the tracked hook to
`.git/hooks/<hook>.local` and unset `core.hooksPath`). The managed hooks:

- **pre-commit** refuses commits on the protected primary branch
  (`COVEN_PRIMARY_BRANCH`, default `main`) unless
  `COVEN_ALLOW_PRIMARY_COMMIT=1`; refuses commits on a branch actively
  claimed by another agent; and trips when the `claim canary` HEAD is no
  longer an ancestor of the current HEAD. An executable
  `hooks/pre-commit.local` is chained afterwards, so existing hooks (for
  example the gitleaks secret scan) keep running.
- **pre-push** protects the primary branch and branches matching
  `COVEN_PROTECTED_REGEX` (default `^(release|hotfix)/`) behind an explicit
  merge-intent phrase (`COVEN_MERGE_PHRASE`).

`coven wt --doctor` reports whether the managed hooks are installed — see
[cli-wt](cli-wt.md).
