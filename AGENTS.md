# AGENTS.md — coven

Guidance for **AI agents** (Codex, Claude Code, Hermes, and any Coven familiar)
opening pull requests against this repo. Humans: your canonical guide is
[`CONTRIBUTING.md`](CONTRIBUTING.md) — this is the agent-specific layer on top.

> **Read first:** [`README.md`](README.md) for what this repo is, and
> [`CONTRIBUTING.md`](CONTRIBUTING.md) for the full contribution bar (including
> the "Contributor First 10 Minutes" checkout path).

---

## What this repo is (one line)

Coven is a small, boring **Rust authority layer** with TypeScript integration
packages around it. The development loop must keep that boundary clear: core
logic stays in Rust; the npm packages are thin integration surface.

## Claim your work first — parallel sessions duplicate otherwise

Multiple agent sessions (Codex, Claude Code, familiars) frequently run against
**the same checkout at once**, each in its own worktree. Worktrees keep git
operations from racing, but they do **not** stop two sessions from independently
building the *same issue* — which has happened repeatedly, producing duplicate
PRs that a session then has to close. Before you touch code:

1. **Check what's already taken.** Duplication hides behind divergent branch
   names — one issue once spawned `fix/output-polish`, `fix/311-output-polish`,
   *and* `fix/output-polish-311` — so branch names alone won't tell you. Check
   both the shared claim registry and open PRs:
   ```sh
   coven claim status          # active claims, shared across every worktree of this repo
   gh pr list --state open     # is there already a PR for this issue?
   ```
   If the issue is claimed or already has a PR, pick different work or coordinate.

2. **Claim it with a shared, issue-keyed token** — not your working branch name,
   which no other session can predict:
   ```sh
   coven claim acquire issue-<N>     # e.g. issue-311; a TTL-bounded lock
   ```
   Claims live in the repo's shared `--git-common-dir/agent-claims/`, so every
   worktree and session sees them. For long tasks, extend the TTL with
   `coven claim heartbeat issue-<N>`.

3. **Release when your PR merges or you stop:** `coven claim release issue-<N>`.

This step is cheap and it is the single thing that prevents duplicate-PR churn.

## Branch & PR workflow (all agents)

- **Claim the issue first** (see above) — `coven claim status` + `gh pr list`
  before starting, then `coven claim acquire issue-<N>`.
- **Never push to `main`.** Every change lands via a PR with green CI. Branch
  from current `origin/main`.
- **Fresh branch per task.** If multiple sessions may touch this repo, work in a
  git worktree so operations don't race:
  ```sh
  git fetch origin main
  git worktree add -b <branch> /tmp/coven-<branch> origin/main
  ```
- Keep the diff **scoped to one concern**; no drive-by refactors in a feature PR.
- Conventional-commit subjects: `feat:`, `fix:`, `docs:`, `chore:`, `refactor:`.
- For larger changes, **start from an issue** and include the readiness packet
  the PR template asks for.
- After merge: delete the remote branch, remove your local worktree/branch.

## CI gates — run locally before opening the PR

CI (`.github/workflows/ci.yml`) rejects on any of these. Run them first:

```sh
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --locked
python scripts/check-secrets.py        # secret scan — must be clean
```

If you touched the **npm/TypeScript** packages, also:

```sh
npm run build
npm test
```

`-D warnings` has **no exceptions**. Fix lints; don't `#[allow(...)]` without a
justifying comment.

## Repo-specific invariants (don't break these)

- **Keep the Rust authority boundary clean.** Business/authority logic lives in
  the Rust crates. Don't push core decisions into the TS packages.
- **Supported harness set is Codex, Claude Code, and GitHub Copilot CLI** until
  policy and adapter contracts are stable. Don't add speculative harness adapters.
- **Never weaken the secret scan.** If `check-secrets.py` flags something, fix
  the content — don't allowlist your way past it.
- Prefer the fast loop (`cargo check`, debug builds) over `--release` unless you
  specifically need optimized output.

## Attribution — credit contributors correctly

When you re-land or build on someone else's work (a fork PR, an issue author's
proposal, a co-author), **credit the human contributor with a working
GitHub-linked trailer** so they appear in the contributors graph and on their
profile:

```
Co-authored-by: Full Name <ID+username@users.noreply.github.com>
```

- Use the **numeric-id no-reply form**. Get the id with
  `gh api users/<login> --jq .id`.
- **Never** use a machine/`.local` email (e.g. `name@Someones-Mac.local`) in a
  co-author trailer — it links to no account and gives **zero** credit.
- When a squash-merge collapses a contributor's PR into an internal branch,
  preserve their `Co-authored-by:` line in the squash commit message.

## Secrets & safety

- Never commit secrets, tokens, or private emails. Use `*.noreply.github.com`
  for attribution.
- Don't disable CI gates or branch protection to land a change. If it can't go
  through a green PR, surface the blocker instead of working around it.

## Claude Code

`CLAUDE.md` points here — this file is the source of truth for both.
