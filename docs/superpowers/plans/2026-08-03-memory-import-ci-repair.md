# Memory Import CI Repair Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make PR #568 pass stable Windows compilation and parallel Ubuntu workspace tests without changing memory migration production behavior.

**Architecture:** Remove a test-only Windows identity helper whose callers are already Unix-only. Add one test-only mutex shared by migration unit fixtures and the canonical reader migration regression, with the guard lifetime tied to each temporary fixture.

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

Delete the `#[cfg(windows)]` and `#[cfg(not(any(unix, windows)))]` variants.
Do not change the production `windows_opened_file_identity` function.

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
git commit -s -m "fix(memory): keep Windows tests on stable Rust" \
  -m "Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

### Task 2: Serialize only migration filesystem fixtures

**Files:**
- Modify: `crates/coven-cli/src/memory_import.rs:1-30`
- Modify: `crates/coven-cli/src/memory_import.rs:4583-4595`
- Modify: `crates/coven-cli/src/memory_import.rs:8730-8745`
- Modify: `crates/coven-cli/src/cockpit_sources.rs:1445-1455`

- [ ] **Step 1: Reproduce resource contention with the focused suite**

Run:

```bash
(ulimit -n 256; cargo test -p coven-cli --bin coven --locked memory_import::tests -- --test-threads=8)
```

Expected before the repair: one or more tests fail during private-directory
setup or before their apply hook runs. Record the exact failure. If the host
does not reproduce GitHub's limit, continue using the hosted failure evidence:
61 related failures headed by `unable to secure private import directory`.

- [ ] **Step 2: Add the shared test-only guard**

Near the top-level imports in `memory_import.rs`, add:

```rust
#[cfg(all(test, unix))]
use std::sync::{Mutex, MutexGuard, OnceLock};

#[cfg(all(test, unix))]
static MEMORY_IMPORT_TEST_GUARD: OnceLock<Mutex<()>> = OnceLock::new();

#[cfg(all(test, unix))]
pub(crate) fn acquire_memory_import_test_guard() -> MutexGuard<'static, ()> {
    MEMORY_IMPORT_TEST_GUARD
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
```

The mutex contains no protected data, so recovering a poisoned guard is safe
and prevents one assertion panic from cascading into later tests.

- [ ] **Step 3: Bind the guard to trusted temporary-directory lifetime**

In the `memory_import` test module, replace the `tempfile::TempDir` return type
with this wrapper:

```rust
struct TrustedTempDir {
    inner: tempfile::TempDir,
    #[cfg(unix)]
    _guard: MutexGuard<'static, ()>,
}

impl TrustedTempDir {
    fn path(&self) -> &Path {
        self.inner.path()
    }
}

fn trusted_tempdir() -> Result<TrustedTempDir> {
    #[cfg(unix)]
    let guard = acquire_memory_import_test_guard();

    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let worktree = manifest_dir
        .parent()
        .and_then(Path::parent)
        .expect("coven-cli manifest must be inside the repository");
    let repository = worktree
        .parent()
        .filter(|parent| parent.file_name() == Some(std::ffi::OsStr::new(".worktrees")))
        .and_then(Path::parent)
        .unwrap_or(worktree);
    let test_root = repository.join("target/m");
    fs::create_dir_all(&test_root)?;
    let inner = tempfile::Builder::new()
        .prefix("m")
        .tempdir_in(test_root)?;

    Ok(TrustedTempDir {
        inner,
        #[cfg(unix)]
        _guard: guard,
    })
}
```

Keep `inner` before `_guard` so the temporary directory is removed before the
mutex guard is released.

- [ ] **Step 4: Put the canonical reader migration regression behind the same guard**

At the start of
`cockpit_sources::tests::opened_memory_record_rechecks_logical_restore_state`,
add:

```rust
let _migration_guard = crate::memory_import::acquire_memory_import_test_guard();
```

The test already has `#[cfg(not(windows))]`; the helper is available on its
supported Unix CI platform.

- [ ] **Step 5: Run focused tests under parallel pressure**

Run:

```bash
(ulimit -n 256; cargo test -p coven-cli --bin coven --locked memory_import::tests -- --test-threads=8)
cargo test -p coven-cli --bin coven --locked cockpit_sources::tests
```

Expected: both commands pass, including the stale-record logical restore
regression.

- [ ] **Step 6: Commit fixture isolation**

```bash
git add crates/coven-cli/src/memory_import.rs crates/coven-cli/src/cockpit_sources.rs
git commit -s -m "test(memory): isolate migration filesystem fixtures" \
  -m "Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
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
git commit -s -m "docs(memory): plan CI repair" \
  -m "Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
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
