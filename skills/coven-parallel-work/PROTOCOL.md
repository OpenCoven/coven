# Coven Parallel Work Protocol — Specification

**Status:** Draft v0.1 · 2026-06-03 · POSIX-shell prototype
**Canonical home (target):** `OpenCoven/coven` — see [issue #167](https://github.com/OpenCoven/coven/issues/167)
**Prototype home (current):** local copies in individual user workspaces

This document specifies the **Coven Parallel Work Protocol** — a portable
contract for safely running multiple agents (or agent + human) on the same
git repository on the same machine. It is **harness-agnostic**: it presumes
nothing about Codex, Claude Code, OpenClaw, Hermes, Nova, or any future
runtime. Any harness that respects the protocol can interleave with any
other harness that respects the protocol.

Tonight's POSIX implementation (`cv-wt`, `cv-claim`, `pre-commit`,
`pre-push`) is a **prototype** of this spec. The intended end state is for
the canonical implementation to ship inside Coven itself — surfaced as
`coven wt`, `coven claim`, `coven hooks install`, or equivalent — at which
point this spec becomes the contract that prototype and canonical agree on.

## 1. Problem

When two or more agents share a working tree, they collide:

- HEAD gets reset out from under an in-flight refactor.
- A stash from another session auto-applies into the current checkout.
- A version-bump bot direct-merges to `main` mid-PR.
- Untracked files from another session pollute the build.
- Force-pushes destroy each other's commits.

The git CLI cannot distinguish agents; everything looks like the same user
on the same FS path. Mitigations layered on top of git can.

## 2. Design principles

1. **Harness-agnostic.** The protocol must work for any agent that runs
   `git`. No assumption that Coven, OpenClaw, or any specific runtime is
   present.
2. **Local-first.** No network service required. Coordination is
   filesystem-mediated under the repo's `.git` directory.
3. **Crash-safe.** A crashed agent must not block the repo forever.
   Every soft lock is TTL-bounded.
4. **Override-friendly for humans.** Every hard refusal has a documented
   bypass path so a human operator can recover.
5. **Composable with existing repo conventions.** Repos that already have
   `core.hooksPath` set, secret-scanning hooks, husky, etc. must keep
   working. The protocol chains, never silently replaces.
6. **Identifiable.** Each participant has a stable, opaque identity
   (`COVEN_AGENT_ID`). Defaults to `$USER` if unset, so the protocol
   degrades gracefully on machines with one human and one AI.

## 3. Concepts

### 3.1 Primary repo and worktrees

Every shared repo has a **primary working directory** (the canonical
clone). The primary directory:

- Stays checked out on the **primary branch** (default `main`).
- Is **read-mostly**. No agent commits feature work there.

Each active feature branch is checked out as a sibling worktree:

```
<REPO_ROOT>/                       primary clone, branch=primary, READ-MOSTLY
<REPO_ROOT>.wt/<branch-slug>/      one worktree per active branch
```

**Branch slug** = branch name with `/` replaced by `-`. Example:
`feat/foo` → `feat-foo`. Slugs need only round-trip back to a unique
filesystem-safe name; they are not required to encode the full branch
name losslessly.

The sibling-dir choice is deliberate. Subdirs (`<REPO_ROOT>/.worktrees/`)
end up in editor trees, search results, and grep output, and confuse
recursive build tools. The protocol REQUIRES sibling-dir layout for
interop; agents that respect a different layout (subdir, scratch dir,
absolute path elsewhere) are non-conforming for protocol purposes.

### 3.2 Agent identity

`$COVEN_AGENT_ID` — short, stable, opaque string that identifies the
agent occupying a shell. Examples: `nova`, `codex-a`, `claude-code`,
`hermes`. If unset, identity falls back to `$USER`.

Agents SHOULD set this once at session start (in their familiar's startup
file, harness config, or shell rc).

### 3.3 Branch claims

A **claim** is a per-branch advisory lock asserting "I, agent X, am
actively working on branch B until time T." Claims live as files inside
the repo's git common dir:

```
<git-common-dir>/agent-claims/<branch-slug>
```

Claim file format (key=value lines):

```
agent=<COVEN_AGENT_ID>
branch=<branch>
acquired=<unix-seconds>
ttl_until=<unix-seconds>
pid=<process-id>
host=<hostname>
```

Claims are **TTL-bounded** (default 3600 seconds). Past `ttl_until`, the
claim is logically released regardless of file presence. Agents SHOULD
heartbeat their claim periodically for long sessions.

### 3.4 HEAD canary

A **canary** records the HEAD a session started on so a pre-commit hook
can detect "HEAD moved out from under me" between session start and
commit. Canary lives at:

```
<git-common-dir>/AGENT_HEAD_AT_START
```

Single line, space-separated:

```
<head-sha> <branch> <agent-id> <unix-seconds>
```

The canary catches the failure mode where a parallel agent resets or
rebases the same branch after the current session opened it.

### 3.5 Merge intent

A **merge intent** is explicit human consent to push to a protected
branch. Lives at:

```
<git-common-dir>/MERGE_INTENT
```

Single line containing the canonical merge phrase. The phrase is
**configurable** per repo or per harness via `$COVEN_MERGE_PHRASE`. The
default is `Enchant merge to main.` for OpenCoven repos; other groups can
choose their own.

Intent files are **consumed** — pre-push deletes them on success. Each
push to a protected branch needs fresh consent.

## 4. Behavior

### 4.1 Pre-commit refusals

A conforming pre-commit hook MUST refuse the commit if any of:

1. **Primary-branch guard.** Current branch equals the primary branch
   AND `$COVEN_ALLOW_PRIMARY_COMMIT` is not `1`.
2. **Claim conflict.** A claim file exists for the current branch, the
   claim is unexpired, and `agent=` differs from `$COVEN_AGENT_ID`.
3. **HEAD canary.** Canary file exists, the recorded HEAD differs from
   current HEAD, and recorded HEAD is not an ancestor of current HEAD.

Each refusal MUST print a human-readable message naming the specific
rule and a documented recovery path. Bypass via `git commit --no-verify`
remains available; conforming hooks SHOULD log a warning when bypassed
but MUST NOT prevent it.

### 4.2 Pre-push refusals

A conforming pre-push hook MUST refuse the push if any of:

1. **Force-push to a protected branch.** Always refused, regardless of
   intent. Protected = the primary branch OR matches
   `$COVEN_PROTECTED_REGEX` (default `^(release|hotfix)/`).
2. **Push to a protected branch without merge intent.** No intent file,
   or intent file does not contain the canonical phrase exactly.

On a successful protected-branch push, the hook MUST consume (delete)
the intent file.

### 4.3 Hook chaining

Hooks installed by this protocol MUST chain to a repo's pre-existing
hook implementation rather than replace it. Convention:

- Existing hook at install time is renamed to `<hook>.local`.
- Protocol hook runs its checks first.
- On success, protocol hook execs the `<hook>.local` (passing original
  args and stdin where applicable).

This preserves repo-specific guarantees (secret scanning, lint, husky)
while layering protocol guarantees on top.

### 4.4 Repos with `core.hooksPath`

If a repo sets `core.hooksPath` to a tracked directory (committed in the
repo, not under `.git/`), the protocol installer MUST refuse to
auto-modify those hooks. The installer MUST instead:

- Detect the configured path.
- Print a human message identifying the conflict.
- Offer two documented integration paths:
  1. Manually merge protocol checks into the tracked hook(s).
  2. `git config --unset core.hooksPath`, install protocol hooks under
     `.git/hooks/`, and re-add a `<hook>.local` that execs the tracked
     hook to keep its checks active.

This is a deliberate choice: silently rewriting tracked files is worse
than refusing.

## 5. Configuration

| Variable | Default | Scope | Purpose |
|----------|---------|-------|---------|
| `COVEN_PRIMARY_BRANCH` | `main` | repo or session | Protected primary branch name |
| `COVEN_PROTECTED_REGEX` | `^(release\|hotfix)/` | repo or session | Other protected branch pattern (POSIX ERE) |
| `COVEN_MERGE_PHRASE` | `Enchant merge to main.` | repo or session | Canonical user intent phrase, exact match |
| `COVEN_AGENT_ID` | `$USER` | session | Stable agent identity |
| `COVEN_REPO_ROOT` | autodetect | session | Override primary repo root |
| `COVEN_ALLOW_PRIMARY_COMMIT` | unset | session | Allow commits on primary (rare) |

Conforming implementations MAY add additional vars. Vars listed here
MUST be honored.

## 6. Implementations

### 6.1 Reference prototype (POSIX shell)

POSIX-shell scripts that implement this spec are available as a
prototype. Tonight's prototype:

- `cv-wt` — worktree wrapper (create / list / prune-merged / prune-stale
  / remove / doctor / install-hooks)
- `cv-claim` — claim helper (acquire / release / heartbeat / canary /
  status)
- `pre-commit`, `pre-push` — chained hook scripts

The prototype is **not the canonical implementation**. It is a working
reference that any harness can adapt or call directly while the
canonical implementation matures upstream.

### 6.2 Canonical implementation (target)

The canonical implementation is intended to ship inside the Coven daemon
+ CLI (`OpenCoven/coven`), surfaced as something like:

```
coven wt <branch>
coven wt --list
coven wt --doctor
coven claim acquire <branch>
coven claim heartbeat <branch>
coven hooks install
```

Spec-conformance is what matters. A harness that respects the spec —
filesystem layout, claim file format, canary file format, intent file
location and phrase, hook refusal semantics — can interleave with any
other conforming harness, regardless of language or distribution.

## 7. Adoption path

The protocol is incrementally adoptable:

1. **Worktree convention only** (Section 3.1). Solves ~70% of collisions
   for small teams. Costs nothing.
2. **+ branch claims** (Section 3.3). Adds value once 3+ agents touch
   the same repo regularly.
3. **+ branch protection** (Section 4.2). Becomes non-negotiable as soon
   as any background bot can push (release-bumpers, dependabot mirrors,
   scheduled commit jobs).

Repos can stop at any layer.

## 8. Out of scope

- **Cross-repo coordination.** If two agents work on different repos and
  one publishes a package the other depends on, this protocol does
  nothing for them. Solve at the release-coordination layer.
- **Long-running daemons inside worktrees.** Dev servers, watchers, test
  runners. They survive worktree removes and can corrupt state. Each
  agent owns its own background-process lifecycle.
- **External state.** Databases, generated files outside git, OS-level
  locks, browser profiles, etc.

## 9. Open questions

- Should claim file format be JSON instead of key=value? Key=value is
  shell-trivial; JSON is harness-trivial. Probably JSON for canonical.
- Should the canary be auto-rearmed by `cv-wt --cd` / `coven wt`? Most
  agents would benefit; some prefer explicit canary writes.
- Should there be a `coven status` aggregator that shows all claims +
  worktree health across all known repos in one view? Likely yes,
  Coven-side.
- Should the merge phrase be derivable from the repo (e.g. read from
  `.coven/protocol.json`) rather than a single env var? Yes, for repos
  that want different phrases per protected branch.

## 10. Versioning

This document is **v0.1**. Breaking changes require a version bump.
Conforming implementations SHOULD declare which version they target.

## 11. References

- Hermes Self-Evolution PLAN.md — *"All changes go through human review,
  never direct commit."* Same principle applied at the agent level.
- `git worktree` — the underlying primitive this protocol structures.
- `core.hooksPath`, `pre-commit(5)`, `pre-push(5)` — git hook surfaces
  the protocol layers on.
