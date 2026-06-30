---
name: coven-parallel-work
description: Coven Parallel Work Protocol — how to use it from inside an OpenClaw familiar so multiple agents (Codex, Claude Code, Nova, Hermes, etc.) can share a repo without overwrites or clashes. The protocol itself is harness-agnostic; this skill is the OpenClaw entry point.
---

# Coven Parallel Work Protocol — OpenClaw entry point

## What this is

The **Coven Parallel Work Protocol** is a portable, harness-agnostic
contract for safely running multiple agents on the same git repo on the
same machine. It works for Codex, Claude Code, OpenClaw, Hermes, Nova,
and any future runtime — anyone who runs `git`.

This skill file is the **OpenClaw familiar's entry point**: it tells you
when to use the protocol from inside an OpenClaw session, the local
prototype tooling available, and the daily workflow.

For the full spec (harness-agnostic, no OpenClaw assumptions), read
[`PROTOCOL.md`](./PROTOCOL.md) — that's the contract any conforming
implementation must respect.

## Where this lives

- **Spec**: [`PROTOCOL.md`](./PROTOCOL.md) (in this directory; portable, will move upstream)
- **Reference prototype**: POSIX-shell scripts in `~/.openclaw/workspace/bin/cv-wt`, `cv-claim` and hook templates in this directory's `hooks/`
- **Canonical implementation (target)**: `OpenCoven/coven` — surfaced eventually as `coven wt`, `coven claim`, `coven hooks install`. **Not yet built.** Tracked upstream in [issue #167](https://github.com/OpenCoven/coven/issues/167).
- **This skill**: how an OpenClaw familiar uses the protocol day-to-day

The local prototype is intentionally a prototype. It's working code that
respects the spec, sized to fit one user's setup. Once the canonical
Coven implementation lands, this skill will redirect to it and the
prototype scripts will become a thin shim or get retired.

## When to apply

Use the protocol when:

- A repo is touched by **2+ concurrent agents** (or agent + human, or
  long-lived background bots like release-bumpers).
- You've seen any of: HEAD reset out from under you, stash auto-applied
  someone else's WIP, force-push collisions, a bot direct-merging to
  `main` mid-PR, untracked files leaking between sessions.
- A new Coven user is setting up shared agents on one workspace.

If only one agent ever touches a repo, you don't need the protocol.
Don't over-apply.

## The three layers (TL;DR)

```
┌─ Layer 1: Worktree isolation ─────────┐  Mechanical
│  one cwd per agent, no shared dirs    │
├─ Layer 2: Agent claims ───────────────┤  Logical
│  per-branch lockfile, TTL-bounded     │
├─ Layer 3: Branch protection ──────────┤  Policy
│  no direct main commits/pushes        │
└───────────────────────────────────────┘
```

All three are needed for full coverage. L1 alone gets ~70% of value; L3
becomes non-negotiable once any background bot can push.

## Layout

Two paths matter:

```
<REPO_ROOT>/                       primary clone, branch=main, READ-MOSTLY
<REPO_ROOT>.wt/<branch-slug>/      one worktree per active branch
```

The primary clone stays on `main` and only fast-forwards. **No agent
commits feature work there.** Every feature branch is checked out in a
sibling worktree under `<REPO_ROOT>.wt/`. Branch slug is the branch
name with `/` replaced by `-` (so `feat/foo` → `feat-foo`).

## Daily usage from an OpenClaw familiar

### Starting work on a branch

```bash
# Spawn or enter the worktree for your branch
cv-wt --cd feat/my-thing

# Claim the branch for your familiar (TTL 1h by default)
cv-claim acquire feat/my-thing
cv-claim canary  feat/my-thing      # snapshots HEAD for the canary check
```

Set `COVEN_AGENT_ID` per familiar (e.g. `nova`, `cody`, `kitty`,
`charm`, `astra`, `echo`). For OpenClaw familiars, this should be in
your familiar's config or shell rc. If unset, identity falls back to
`$USER`, which is fine on a single-human-one-AI machine but fails the
moment you have multiple AIs.

### During long sessions

Refresh the claim periodically so it doesn't expire:

```bash
cv-claim heartbeat feat/my-thing 3600
```

### Finishing work

```bash
git push -u origin feat/my-thing
gh pr create ...
cv-claim release feat/my-thing
```

When the branch is merged, sweep:

```bash
cv-wt --prune-merged
```

## Hook behavior (summary)

**`pre-commit`** refuses if:

1. Branch is `main` (default; configurable via `$COVEN_PRIMARY_BRANCH`)
2. Branch has an unexpired claim by a different agent
3. HEAD moved out from under your session (canary tripped)

**`pre-push`** refuses if:

1. Force-push to a protected branch — always blocked
2. Push to `main` / `release/*` without `.git/MERGE_INTENT` containing
   the canonical phrase (`$COVEN_MERGE_PHRASE`, default
   `Enchant merge to main.`)

Hooks chain to `<hook>.local` so they coexist with repos that already
have their own pre-commit (secret scanning, etc.).

Full refusal semantics, configuration vars, and recovery flows are in
[`PROTOCOL.md`](./PROTOCOL.md) §4 and §5.

## Per-repo setup checklist

```bash
cd <repo>
cv-wt --doctor                 # see what's wrong now
cv-wt --install-hooks          # install pre-commit + pre-push
cv-wt --prune-merged           # clean up merged worktrees
cv-wt --prune-stale 7          # remove worktrees untouched > 7 days
```

If `cv-wt --install-hooks` refuses because the repo has tracked hooks
(`core.hooksPath` pointing at a versioned dir, like coven-cave's
`scripts/git-hooks/`), it will print two integration paths. Pick one.
The protocol does not silently rewrite tracked files.

GitHub-side branch protection complements local hooks and is strongly
recommended:

- Require PR for `main` + `release/*`
- Require status checks to pass
- Disallow force pushes
- Disallow deletions

## Recovering from collisions

If the canary trips, HEAD moved out from under you. **Do not push.**

```bash
cd <your-worktree>
git status -sb                                  # see where HEAD is now
git reflog -10                                  # find the commit you expected
git reset --hard <expected-sha>                 # restore
rm $(git rev-parse --git-common-dir)/AGENT_HEAD_AT_START  # acknowledge
cv-claim canary <your-branch>                   # rearm canary
```

If a stash auto-applied someone else's WIP into your checkout:

```bash
git stash list                                  # find the suspicious one
git stash show -p stash@{N}                     # inspect
git checkout -- <files>                         # discard the alien diff
git stash drop stash@{N}                        # remove the stash
```

## What this protocol does NOT solve

- **Cross-repo races.** Solve at the release-coordination layer.
- **Long-running daemons inside worktrees.** Dev servers, watchers, test
  runners. Each familiar owns its own background-process lifecycle.
- **External state.** Databases, generated files outside git, OS-level
  locks. Out of scope.

## Migration plan to canonical Coven CLI

Tonight's prototype is **option B** in our June 3 design discussion:
keep the working POSIX prototype, file an issue upstream so the team
owns the design, migrate to `coven wt` / `coven claim` / `coven hooks`
once the Coven CLI subcommand surface stabilizes. When that lands:

1. Spec stays at `PROTOCOL.md` (or moves into `OpenCoven/coven/docs/`)
2. `cv-wt` and `cv-claim` become thin shims that exec `coven wt` /
   `coven claim`, with a deprecation warning
3. This SKILL.md updates to point at `coven` as the entry point

The spec is what matters. Implementations are interchangeable as long
as they conform.

## See also

- [`PROTOCOL.md`](./PROTOCOL.md) — the full harness-agnostic spec
- `using-git-worktrees` skill — general git worktree patterns
- AGENTS.md "Hard Rules" — repo-level merge gate phrase requirements
- Hermes Self-Evolution `PLAN.md` — *"All changes go through human
  review, never direct commit"* — same principle, agent-level
