# Worktree-Scoped Claim Identity Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make default claim ownership distinguish same-user sessions in different Git worktrees while preserving explicit agent IDs.

**Architecture:** The Rust CLI derives a fallback owner from the OS user and current worktree root basename. The managed and reference pre-commit hooks reproduce that algorithm, and documentation moves claim acquisition inside the task worktree.

**Tech Stack:** Rust, Git worktrees, POSIX shell, Rust integration tests, Markdown.

---

### Task 1: Prove the broken default and lifecycle contract

**Files:**
- Modify: `crates/coven-cli/tests/parallel_protocol.rs`

- [ ] **Step 1: Add worktree-aware test helpers and failing tests**

Add helpers that run the built `coven` binary from a supplied working
directory while removing `COVEN_AGENT_ID`. Add tests covering:

```rust
#[test]
fn default_claim_identity_blocks_same_user_in_another_worktree() -> anyhow::Result<()> {
    // Create two linked worktrees, acquire from the first with USER=val,
    // and assert the second USER=val acquire fails as already claimed.
}

#[test]
fn default_claim_identity_supports_same_worktree_lifecycle() -> anyhow::Result<()> {
    // Acquire, heartbeat, and release from one worktree with USER=val.
}

#[test]
fn explicit_agent_identity_remains_authoritative_across_worktrees() -> anyhow::Result<()> {
    // Use the same explicit COVEN_AGENT_ID from two worktrees and assert the
    // second invocation refreshes the same owner's claim.
}
```

- [ ] **Step 2: Run the focused tests and verify RED**

Run:

```sh
cargo test -p coven-cli --test parallel_protocol default_claim_identity -- --nocapture
```

Expected: the cross-worktree test fails because both owners are `val`.

### Task 2: Derive one canonical fallback identity

**Files:**
- Modify: `crates/coven-cli/src/parallel_protocol.rs`

- [ ] **Step 1: Pass repository context into identity resolution**

Change claim acquire, release, and heartbeat to call `agent_id(&repo)`.
Implement:

```rust
fn agent_id(repo: &Repo) -> String {
    if let Some(explicit) = std::env::var("COVEN_AGENT_ID")
        .ok()
        .filter(|value| !value.trim().is_empty())
    {
        return explicit;
    }

    let user = std::env::var("USER")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "unknown-agent".to_string());
    let worktree = repo
        .root
        .file_name()
        .map(|name| branch_slug(&name.to_string_lossy()))
        .filter(|slug| !slug.is_empty())
        .unwrap_or_else(|| "worktree".to_string());
    format!("{user}@{worktree}")
}
```

- [ ] **Step 2: Run the focused lifecycle tests and verify GREEN**

Run:

```sh
cargo test -p coven-cli --test parallel_protocol default_claim_identity -- --nocapture
cargo test -p coven-cli --test parallel_protocol explicit_agent_identity -- --nocapture
```

Expected: all new identity tests pass.

### Task 3: Keep hooks behaviorally identical

**Files:**
- Modify: `crates/coven-cli/src/parallel_protocol.rs`
- Modify: `skills/coven-parallel-work/hooks/pre-commit`
- Modify: `crates/coven-cli/tests/parallel_protocol.rs`

- [ ] **Step 1: Add a failing managed-hook parity test**

Install the managed hook, acquire a branch claim with only `USER` set, and
commit from the same worktree with the same environment. Assert the commit is
accepted; then seed a different worktree-scoped owner and assert rejection.

- [ ] **Step 2: Verify the parity test fails**

Run:

```sh
cargo test -p coven-cli --test parallel_protocol managed_hook_uses_worktree_scoped_default_identity -- --nocapture
```

Expected: the current `$USER`-only hook disagrees with the new claim owner.

- [ ] **Step 3: Implement the shared shell derivation**

In both hook implementations, derive:

```sh
repo_root="$(git rev-parse --show-toplevel)"
worktree_name="${repo_root##*/}"
worktree_slug="$(slug_branch "$worktree_name")"
[ -n "$worktree_slug" ] || worktree_slug="worktree"
fallback_agent="${USER:-unknown-agent}@${worktree_slug}"
agent="${COVEN_AGENT_ID:-$fallback_agent}"
```

- [ ] **Step 4: Verify hook parity**

Run the focused parity test and the full `parallel_protocol` integration test.
Expected: all tests pass.

### Task 4: Update the public workflow contract

**Files:**
- Modify: `AGENTS.md`
- Modify: `docs/MERGE-GUIDE.md`
- Modify: `docs/familiars/parallel-lanes.md`
- Modify: `docs/reference/cli-claim.md`
- Modify: `docs/reference/cli.md`
- Modify: `skills/coven-parallel-work/PROTOCOL.md`
- Modify: `skills/coven-parallel-work/SKILL.md`

- [ ] **Step 1: Replace the `$USER` default**

Document `$USER@<worktree-slug>` everywhere the default is described, state
that explicit `COVEN_AGENT_ID` wins, and disclose that shared-worktree agents
still need explicit IDs.

- [ ] **Step 2: Put worktree entry before acquisition**

Update contributor/agent steps so sessions check status, enter a worktree,
then acquire the shared issue token before editing.

- [ ] **Step 3: Verify documentation consistency**

Run:

```sh
rg -n 'falling back to USER|Defaults? to `?\\$USER|\\| `COVEN_AGENT_ID` \\| `\\$USER`' AGENTS.md docs skills/coven-parallel-work
```

Expected: no stale claim-identity contract remains.

### Task 5: Verify and publish

**Files:**
- Verify all modified files.

- [ ] **Step 1: Run repository gates**

```sh
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --locked
python scripts/check-secrets.py
```

Expected: all commands exit 0.

- [ ] **Step 2: Commit and push**

```sh
git add AGENTS.md crates/coven-cli/src/parallel_protocol.rs crates/coven-cli/tests/parallel_protocol.rs docs skills/coven-parallel-work
git commit -m "fix: scope default claim identity to worktrees"
git push -u origin fix/499-claim-worktree-identity
```

- [ ] **Step 3: Open the PR**

Create a PR with `Closes #499`, exact test evidence, and the documented
contract change. Resolve review conversations only after their concerns are
addressed.
