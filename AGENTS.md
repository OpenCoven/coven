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

## Check, enter a worktree, then claim it

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

2. **Create or enter the task worktree.** The automatic fallback identity is
   worktree-scoped, so enter the worktree before acquiring the claim:
   ```sh
   git fetch origin main
   git worktree add -b <branch> /tmp/coven-<branch-slug> origin/main
   cd /tmp/coven-<branch-slug>
   ```

3. **Claim it with a shared, issue-keyed token** — not your working branch name,
   which no other session can predict:
   ```sh
   coven claim acquire issue-<N>     # e.g. issue-311; a TTL-bounded lock
   ```
   Claims live in the repo's shared `--git-common-dir/agent-claims/`, so every
   worktree and session sees them. For long tasks, extend the TTL with
   `coven claim heartbeat issue-<N>`.

4. **Release from the same worktree when your PR merges or you stop:**
   `coven claim release issue-<N>`.

This step is cheap and it is the single thing that prevents duplicate-PR churn.
Without an explicit `COVEN_AGENT_ID`, Coven identifies the owner as
`$USER@<worktree-slug>`. Set distinct explicit IDs only when multiple agents
must share one worktree.

## Branch & PR workflow (all agents)

- **Coordinate before editing** (see above) — check `coven claim status` and
  `gh pr list`, enter a fresh worktree, then acquire `issue-<N>` from inside it.
- **Never push to `main`.** Every change lands via a PR with green CI. Branch
  from current `origin/main`.
- **Fresh branch per task.** If multiple sessions may touch this repo, use the
  task worktree created before claim acquisition so operations don't race.
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
python3 scripts/check-coven-privacy.py --staged   # privacy guard on your staged changes
```

If you touched the **npm/TypeScript** packages, also:

```sh
npm run build
npm test
```

`-D warnings` has **no exceptions**. Fix lints; don't `#[allow(...)]` without a
justifying comment.

### The `Policy guard` job

The five commands above are **not** the whole gate. `cargo test` does not run
any of the repo's guard scripts, so a branch can be green locally and still fail
the required `PR gate` on `Policy guard` — a ~24-step job whose failures show up
only after you push. Two of its steps have bitten recent PRs: the documentation
ownership guard (#1089) and the CI workflow guard (#1118).

Reproduce the whole job before pushing. The subshell keeps a failing guard from
closing your terminal, and still reports a non-zero status, so a printed
`FAILED:` line cannot be lost in the scroll:

```sh
(
  fail=0
  for s in check-coven-privacy-test.py check-secrets-test.py \
           check-api-contract-docs-test.py check-api-contract-docs.py \
           check-docs-ownership-test.py check-security-policy-test.py \
           check-security-policy.py check-theme-tokens-test.py \
           check-theme-tokens.py check-secrets.py \
           classify-ci-changes-test.py check-reliability-scorecard-test.py \
           check-reliability-scorecard.py check-workflows-test.py \
           check-ci-workflow-test.py install-native-link-dependencies-test.py; do
    python3 "scripts/$s" || { echo "FAILED: $s"; fail=1; }
  done

  node --test scripts/package-github-release-test.mjs || fail=1
  node --test scripts/package-automations-protocol.test.mjs scripts/script-entrypoint.test.mjs || fail=1
  node --test scripts/package-automations-authority-profile.test.mjs || fail=1
  node --test conformance/automations/runner/conformance.test.mjs conformance/automations/runner/audit.test.mjs || fail=1
  node --test scripts/release-stress-test.mjs || fail=1
  node --test scripts/benchmark-cli.test.mjs scripts/benchmark-chaos.test.mjs || fail=1
  bash scripts/check-workflows.sh || fail=1

  exit "$fail"
)
echo "Policy guard (local): $?"
```

A trailing `Policy guard (local): 0` is the pass signal. Do not read a clean
tail of the output as success — several of these guards print nothing when they
pass, and the earlier failure may be thousands of lines up.

### Two guards that only inspect what your branch changed

These take a revision range, and CI passes the PR's base...head. Run them the
same way — `--staged` is not equivalent, because it misses anything you have
already committed:

```sh
python3 scripts/check-docs-ownership.py --range "origin/main...HEAD"
python3 scripts/check-coven-privacy.py  --range "origin/main...HEAD"
```

Because they are range-scoped, **a file that is already non-compliant on `main`
fails only once your branch touches it.** A one-line edit to a long-untouched
page can surface a problem you did not introduce. That is the guard working as
designed: fix the file (for docs ownership, usually by adding a
`source_adjacent_reason` to its frontmatter), rather than reverting your edit.

### Changing `.github/workflows/` needs more than CI green

Two things do not show up in a local `cargo test`:

- **Other workflows run `cargo test --workspace` too.** `release-npm.yml` gates
  the published release on it, and `release-stress.yml` compiles with it. A
  change to which targets that command builds — adding `required-features` to a
  `[[test]]`, say — silently changes the **release** gate as well as CI. Check
  every hit of `grep -rn 'cargo test --workspace' .github/workflows/` before
  assuming CI is the only consumer.
- **Pushing a workflow file needs the `workflow` OAuth scope.** An otherwise
  valid `git push` over HTTPS is rejected with *"refusing to allow an OAuth App
  to create or update workflow"*. Either `gh auth refresh -h github.com -s
  workflow`, or push that branch over SSH.

## Repo-specific invariants (don't break these)

- **Keep the Rust authority boundary clean.** Business/authority logic lives in
  the Rust crates. Don't push core decisions into the TS packages.
- **Supported harness set is Codex, Claude Code, and GitHub Copilot CLI** until
  policy and adapter contracts are stable. Don't add speculative harness adapters.
- **Never weaken the secret scan.** If `check-secrets.py` flags something, fix
  the content — don't allowlist your way past it.
- **Wall-clock assertions in tests must clear scheduler jitter.** A bound like
  `assert!(started.elapsed() < …)` runs on shared CI runners, so a threshold
  close to the nominal duration will flake. Before adding or "fixing" one, work
  out which kind it is — they are not interchangeable, and a blanket bump
  deletes real coverage:
  - **A documented contract** (e.g. the two-second `daemon stop` budget in
    `daemon.rs`): keep it strict. This is the assertion's whole point.
  - **A discriminating threshold** separating a fast path from an injected slow
    path: put the bound near the *midpoint*, not just above the fast path.
  - **A hang guard** that only turns a wedge into a readable failure: set it far
    above anything load can produce, and say so in a comment.
  - **A promptness claim already proven deterministically** in the same test
    (a call count, an ordering, an error string): delete it rather than tune it.

  Report the observed duration in the failure message, or the next flake tells
  you nothing.
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
