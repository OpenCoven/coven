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
   python3 scripts/task-worktree.py create --issue <N> --branch <branch> --path /tmp/coven-<branch-slug>
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

Task allocation must use `scripts/task-worktree.py create`; do not bypass its shared allocation lock or budget refusal with raw `git worktree add`. It caps task worktrees at 12, local task branches at 32 and origin task branches at 48, excluding `main` and origin's symbolic `HEAD`. It also counts stray remote refs, meaning remote-tracking refs outside `origin` and every configured remote, with a limit of 0: `check` and `create` refuse while any exist. Existing work may continue above the cap, but new allocation stops until safe cleanup brings the counts within budget. Follow [the pruning procedure](docs/development/worktree-lifecycle.md); never delete uncertain work to make room. A budget change requires an issue-backed reviewed policy change, not an ad hoc command-line override.

Keep PR-head evidence ephemeral: use `git ls-remote origin refs/pull/<n>/head` for the SHA or `git fetch origin refs/pull/<n>/head`, which stores it only in `FETCH_HEAD`. Never fetch a wildcard refspec into `refs/remotes/`.

Publish task branches with `python3 scripts/task-worktree.py publish --issue <N>` after required checks. It reads live origin heads under the same lock, refuses main/detached publication and new remote growth above 48, and keeps existing updates fast-forward-only. A necessary one-branch publication exception requires `--exception-issue <open-issue> --reason <reason>`; the helper records the exact branch, SHA and counts on that issue before pushing. This records a bounded exception, not a limit change or release authorization. Do not bypass publication checks with raw pushes.

## Durable work tracking

GitHub issues in the owning repository are the sole work tracker. Record
assignees, priority, acceptance criteria, blockers/sub-issues, and exact delivery
evidence there. Follow [`docs/work-tracking.md`](docs/work-tracking.md).
Repository coordination still uses issue-keyed Coven claims and task worktrees.
Do not initialize a separate local tracking database or sync branch.

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

CI (`.github/workflows/ci.yml`) rejects a PR through the required `PR gate`.
Reproduce it with the repository's verification runner, not a hand-copied
command list:

```sh
git fetch origin main                      # the runner never fetches for you
bash scripts/dev-verify.sh --pr            # pre-PR gate over origin/main...HEAD
bash scripts/dev-verify.sh --pr --plan     # print what it would run; runs nothing
bash scripts/dev-verify.sh --quick         # fast inner loop while you work
```

`--pr` runs the toolchain pin checks, `cargo fmt --check`, `cargo check`,
`cargo clippy --workspace --all-targets -- -D warnings`,
`cargo test --workspace --locked`, the secret scan, the privacy and
documentation-ownership guards, every command of the CI `Policy guard` job,
`cargo deny`, and the npm/TypeScript package checks that
`scripts/classify-ci-changes.py` routes for your changed paths. Pass a different
base with `--range <base>...HEAD`. The full contract, including the CI-only
suites that no local tier proves, is in
[`docs/development/dev-verification.md`](docs/development/dev-verification.md).

`-D warnings` has **no exceptions**. Fix lints; don't `#[allow(...)]` without a
justifying comment.

### What the runner does and does not prove

- **The Policy guard inventory is drift-checked.** `cargo test` runs none of
  the repo's guard scripts, so a branch can be green locally and still fail
  `Policy guard`. Recent examples are the documentation ownership guard (#1089)
  and the CI workflow guard (#1118). The runner's copy of that job,
  `scripts/dev-verify-policy.json`, is pinned to a SHA-256 of the `policy-guard`
  job in `ci.yml`. If the job changes, the run fails with
  `policy inventory drift` instead of silently running a stale list. A PR that
  changes the job must update the inventory and its digest in the same change.
- **Read the result, not the tail of the output.** Independent checks keep
  running after a failure, and several guards print nothing when they pass.
  Each check ends in a `PASS`, `FAIL` or `SKIP` line. The last line is
  `Local pr result: <status>`, and only `0` (also the exit status) is a pass.
  Search the output for `FAIL` before concluding anything; an early failure can
  be thousands of lines up.
- **A missing tool is a failure, not a skip.** The runner installs nothing.
  `cargo-deny`, Node at `.node-version`, Python 3.11+ on `PATH` as `python3`
  (the Cloud suites need it) and pnpm (for OpenClaw changes) must already be
  installed. If one is missing, its check fails and the run stays red. Install
  the tool rather than reading the remaining checks as a pass.
- **Verify the committed checkout.** The secret scanner reads tracked files and
  history. It does not read the index or untracked files. Commit, then run
  `--pr` on the exact head you will publish.
- **CI remains final merge authority.** Other-OS runners, the Automations
  bundle artifacts and the release-target matrices are CI-only. The run's
  summary lists them as unverified. For the host-runnable feature suites, use
  `--full` (see the contract above).

### Two guards that only inspect what your branch changed

`--pr` runs the documentation-ownership and privacy guards over its committed
range, which matches CI's PR base...head. To rerun one alone, pass the same
range. `--staged` is not equivalent, because it misses anything you have
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
- **Short budgets passed into code under test also select races.** Audit
  `recv_timeout(...)` and timeout arguments as well as `elapsed()` assertions.
  For test sequencing, wait for an explicit readiness/completion event, and
  announce early failure so the wait cannot silently wedge. For negative
  channel assertions, join the producer before checking the final sequence.
  Keep genuine timeout tests: make the slow path explicit (for example a
  reader that always returns `WouldBlock`), and assert its failure class.
  If asynchronous cleanup can validly choose multiple errors, retain the
  status and containment assertions and exercise each permitted branch
  deterministically. Do not widen production deadlines or all test budgets.
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
