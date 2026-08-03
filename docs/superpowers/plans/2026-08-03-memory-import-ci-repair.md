# Memory Import CI Repair Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make PR #568 pass stable Windows compilation and parallel Ubuntu workspace tests without changing memory migration production behavior.

**Architecture:** Remove a test-only Windows identity helper whose callers are already Unix-only. Keep private-directory permission and durability operations capability-relative while reopening a normal directory descriptor when Linux `O_PATH` cannot support `fchmod` or `fsync`.

**Tech Stack:** Rust, `std::sync`, `tempfile`, Cargo test harness, GitHub Actions

---

### Task 1: Remove the unstable Windows test helper

**Files:**
- Modify: `crates/coven-cli/src/memory_import.rs:8704-8722`

- [ ] **Step 1: Reproduce the stable Windows compile failure**

Run:

```bash
cargo test -p coven-cli --bin coven --target x86_64-pc-windows-gnu --locked --no-run
```

Expected: compilation fails at `opened_metadata_stable_std` with `E0658` for
`volume_serial_number()` and `file_index()`.

- [ ] **Step 2: Delete the unreachable non-Unix helper variants**

Keep only the implementation used by the Unix-only callers:

```rust
#[cfg(unix)]
fn opened_metadata_stable_std(before: &fs::Metadata, after: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;

    before.dev() == after.dev() && before.ino() == after.ino()
}
```

Delete the `#[cfg(windows)]` variant. Retain the generic fallback and do not
change the production `windows_opened_file_identity` function.

- [ ] **Step 3: Verify stable Windows compilation**

Run:

```bash
cargo test -p coven-cli --bin coven --target x86_64-pc-windows-gnu --locked --no-run
```

Expected: the `windows_by_handle` `E0658` errors are absent. If the local GNU
linker is unavailable, run:

```bash
cargo check -p coven-cli --bin coven --tests --target x86_64-pc-windows-gnu --locked
```

and expect successful type checking.

- [ ] **Step 4: Commit the Windows repair**

```bash
git add crates/coven-cli/src/memory_import.rs
git commit -s -m "fix(memory): keep Windows tests on stable Rust"
```

### Task 2: Harden private directories through the pinned capability

**Files:**
- Modify: `crates/coven-cli/src/memory_import.rs:2100-2130`

- [ ] **Step 1: Confirm the Linux-specific failure shape**

Run:

```bash
gh run view 30780788323 --job 91584868249 --log-failed
```

Expected before the repair: the first apply test and every later test that
creates a private import directory fail, while discovery-only tests pass.

- [ ] **Step 2: Use the capability-relative permission API**

Replace `secure_private_directory_handle` with:

```rust
#[cfg(unix)]
fn secure_private_directory_handle(directory: &Dir) -> Result<()> {
    use cap_std::fs::PermissionsExt;

    directory
        .set_permissions(".", cap_std::fs::Permissions::from_mode(0o700))
        .map_err(|_| anyhow!("unable to secure private import directory"))
}
```

This keeps the operation relative to the pinned directory. On Linux,
`cap-std` handles `O_PATH` by reopening safely through its capability root.

- [ ] **Step 3: Reopen a normal directory descriptor for durability sync**

Replace `sync_dir_handle` with:

```rust
#[cfg(unix)]
fn sync_dir_handle(directory: &Dir) -> Result<()> {
    let mut options = OpenOptions::new();
    options
        .read(true)
        .follow(FollowSymlinks::No)
        .maybe_dir(true);
    directory
        .open_with(".", &options)
        .and_then(|file| file.sync_all())
        .map_err(|_| anyhow!("unable to sync import directory"))
}
```

This opens `"."` under the pinned capability instead of calling `fsync` on the
Linux `O_PATH` descriptor.

- [ ] **Step 4: Run focused tests**

Run:

```bash
cargo test -p coven-cli --bin coven --locked memory_import::tests
cargo test -p coven-cli --bin coven --locked cockpit_sources::tests
```

Expected: both commands pass, including the stale-record logical restore
regression.

- [ ] **Step 5: Commit Linux directory hardening**

```bash
git add crates/coven-cli/src/memory_import.rs
git commit -s -m "fix(memory): secure Linux import directories"
```

### Task 3: Validate and publish the CI repair

**Files:**
- Modify: `docs/superpowers/specs/2026-08-03-memory-import-ci-repair-design.md`
- Create: `docs/superpowers/plans/2026-08-03-memory-import-ci-repair.md`

- [ ] **Step 1: Validate formatting and lint**

Run:

```bash
cargo fmt --check
cargo clippy --workspace --all-targets --locked -- -D warnings
git diff --check
```

Expected: all commands exit successfully.

- [ ] **Step 2: Run migration and workspace test gates**

Run:

```bash
cargo test -p coven-cli --bin coven --locked memory_import::tests
cargo test -p coven-cli --bin coven --locked cockpit_sources::tests
cargo test -p coven-cli --test memory_import --locked
cargo test --workspace --locked -- --skip mobile_memory::gateway::tests::mobile_listener_requires_pinned_tls13
```

Expected: every command passes. The skip remains limited to the documented,
unrelated mobile TLS `EAGAIN` test.

- [ ] **Step 3: Commit the finalized design and plan**

```bash
git add \
  docs/superpowers/specs/2026-08-03-memory-import-ci-repair-design.md \
  docs/superpowers/plans/2026-08-03-memory-import-ci-repair.md
git commit -s -m "docs(memory): plan CI repair"
```

- [ ] **Step 4: Push the existing PR branch**

```bash
git push origin feat/cmem-0b9-memory-import-v2
```

Expected: PR #568 updates with the repair commits.

- [ ] **Step 5: Confirm hosted CI**

Run:

```bash
gh pr checks 568 --watch
```

Expected: both `Rust checks (ubuntu-latest)` and
`Rust checks (windows-latest)` pass.

- [ ] **Step 6: Update Beads**

Run:

```bash
bd comments add cmem-0b9 "PR #568 CI repaired: stable Windows compilation and isolated Unix migration fixtures now pass hosted checks."
bd close cmem-0b9 --reason="PR #568 implementation and hosted CI are complete."
```

Expected: `cmem-0b9` is closed with current CI evidence.
