# Maintenance Gate Lock Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the unsafe stale-file maintenance lock with a bounded, cross-process advisory lock that cannot be stolen from a live holder while preserving the repository's validated lock-path open semantics.

**Architecture:** Keep `MaintenanceGate::with_lock` and all owner/writer lease records unchanged. `GateLock` will hold an open file returned by `crate::state_lock::open_lock_file(&path)`, protect it with `fs2::FileExt::try_lock_exclusive`, retry recognized contention for the existing five-second window, and release through RAII without unlinking the persistent lock file.

**Tech Stack:** Rust standard library, `fs2`, `anyhow`, Cargo test/clippy/fmt, repository privacy and secret guards.

---

## File Structure

- Modify `crates/coven-cli/src/maintenance_gate.rs`: replace stale-file takeover with a timed advisory lock opened through the existing validated helper and add focused unit tests.
- Keep `crates/coven-cli/src/state_lock.rs` unchanged: reuse its existing `is_lock_contended` helper rather than duplicating platform-specific error recognition.
- Keep public CLI/API documentation unchanged: external maintenance behavior and contracts do not change.

### Task 1: Add Regression Coverage

**Files:**
- Modify: `crates/coven-cli/src/maintenance_gate.rs:626-671`
- Test: `crates/coven-cli/src/maintenance_gate.rs`

- [ ] **Step 1: Refresh the shared issue claim**

Run:

```bash
cd /tmp/coven-fix-612-maintenance-lock
coven claim heartbeat issue-612
```

Expected: `issue-612` remains owned by `buns@coven-fix-612-maintenance-lock` with a renewed expiry.

- [ ] **Step 2: Write tests for live contention, release, stale mtime, symlink refusal, and hard-link refusal**

Add these imports and tests inside `maintenance_gate.rs`'s existing `#[cfg(test)] mod tests`:

```rust
use std::fs::FileTimes;
#[cfg(unix)]
use std::os::unix::fs::symlink;

fn assert_contended(error: &anyhow::Error) {
    assert!(error
        .downcast_ref::<GateError>()
        .is_some_and(|error| matches!(error, GateError::Contended)));
}

#[test]
fn live_gate_lock_blocks_a_second_acquirer() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let gate = MaintenanceGate::at(temp.path().to_path_buf());
    gate.ensure_layout()?;
    let _first = GateLock::acquire(gate.lock_path())?;

    let error =
        GateLock::acquire_with_wait(gate.lock_path(), Duration::ZERO).unwrap_err();

    assert_contended(&error);
    Ok(())
}

#[test]
fn dropping_gate_lock_allows_the_next_acquirer() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let gate = MaintenanceGate::at(temp.path().to_path_buf());
    gate.ensure_layout()?;
    let first = GateLock::acquire(gate.lock_path())?;
    drop(first);

    let _second =
        GateLock::acquire_with_wait(gate.lock_path(), Duration::ZERO)?;

    Ok(())
}

#[test]
fn stale_mtime_does_not_allow_live_gate_lock_takeover() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let gate = MaintenanceGate::at(temp.path().to_path_buf());
    gate.ensure_layout()?;
    let _first = GateLock::acquire(gate.lock_path())?;
    let lock_file = fs::OpenOptions::new()
        .write(true)
        .open(gate.lock_path())?;
    lock_file.set_times(
        FileTimes::new()
            .set_modified(SystemTime::now() - Duration::from_secs(120)),
    )?;

    let error =
        GateLock::acquire_with_wait(gate.lock_path(), Duration::ZERO).unwrap_err();

    assert_contended(&error);
    Ok(())
}

#[cfg(unix)]
#[test]
fn symlinked_gate_lock_is_refused_without_touching_the_target() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let gate = MaintenanceGate::at(temp.path().to_path_buf());
    gate.ensure_layout()?;
    let outside = tempfile::NamedTempFile::new()?;
    fs::write(outside.path(), b"outside-lock-target")?;
    symlink(outside.path(), gate.lock_path())?;

    let error = GateLock::acquire(gate.lock_path()).expect_err("symlinked gate lock must fail");

    assert!(format!("{error:#}").contains(&gate.lock_path().display().to_string()));
    assert_eq!(fs::read(outside.path())?, b"outside-lock-target");
    Ok(())
}

#[cfg(unix)]
#[test]
fn multiply_linked_gate_lock_is_refused() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let gate = MaintenanceGate::at(temp.path().to_path_buf());
    gate.ensure_layout()?;
    fs::write(gate.lock_path(), b"lock")?;
    let alias = temp.path().join("lock-alias");
    fs::hard_link(gate.lock_path(), &alias)?;

    let error =
        GateLock::acquire(gate.lock_path()).expect_err("multiply linked gate lock must fail");

    assert!(format!("{error:#}").contains(&gate.lock_path().display().to_string()));
    Ok(())
}
```

- [ ] **Step 3: Run the focused tests and confirm they fail before implementation**

Run:

```bash
cd /tmp/coven-fix-612-maintenance-lock
cargo test -p coven-cli maintenance_gate::tests --locked
```

Expected: the new Unix regression tests fail against the plain `std::fs::OpenOptions` implementation because it follows a symlinked maintenance lock path and accepts a multiply linked lock file.

### Task 2: Replace Stale-File Takeover with an Advisory Lock

**Files:**
- Modify: `crates/coven-cli/src/maintenance_gate.rs:7-14`
- Modify: `crates/coven-cli/src/maintenance_gate.rs:556-604`
- Test: `crates/coven-cli/src/maintenance_gate.rs`

- [ ] **Step 1: Import the advisory-lock trait and monotonic timer**

Change the relevant imports to:

```rust
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::{Arc, Condvar, Mutex},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
```

- [ ] **Step 2: Remove the stale-lock age constant**

Delete:

```rust
const LOCK_STALE_AFTER: Duration = Duration::from_secs(30);
```

Keep `LOCK_WAIT` at five seconds and the existing 10-millisecond retry interval.

- [ ] **Step 3: Replace `GateLock` with a file-handle-backed advisory lock**

Replace the current `GateLock`, its `Drop` implementation, and `is_stale` with:

```rust
#[derive(Debug)]
struct GateLock {
    file: fs::File,
}

impl GateLock {
    fn acquire(path: PathBuf) -> Result<Self> {
        Self::acquire_with_wait(path, LOCK_WAIT)
    }

    fn acquire_with_wait(path: PathBuf, wait: Duration) -> Result<Self> {
        let file = crate::state_lock::open_lock_file(&path)
            .with_context(|| format!("failed to open maintenance lock {}", path.display()))?;
        let started = Instant::now();
        loop {
            match file.try_lock_exclusive() {
                Ok(()) => return Ok(Self { file }),
                Err(error) if crate::state_lock::is_lock_contended(&error) => {
                    if started.elapsed() >= wait {
                        return Err(GateError::Contended.into());
                    }
                    thread::sleep(Duration::from_millis(10));
                }
                Err(error) => {
                    return Err(error)
                        .with_context(|| format!("failed to acquire {}", path.display()));
                }
            }
        }
    }
}

impl Drop for GateLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}
```

This deliberately leaves the lock file on disk. The file's mtime is no longer consulted, and no holder removes a path that another holder may own.

- [ ] **Step 4: Run the focused tests**

Run:

```bash
cd /tmp/coven-fix-612-maintenance-lock
cargo test -p coven-cli maintenance_gate::tests --locked
```

Expected: all maintenance-gate unit tests pass, including the contention/release tests plus the new Unix symlink and hard-link regression tests.

- [ ] **Step 5: Format and inspect the focused diff**

Run:

```bash
cd /tmp/coven-fix-612-maintenance-lock
cargo fmt
git --no-pager diff --check
git --no-pager diff -- crates/coven-cli/src/maintenance_gate.rs
```

Expected: formatting succeeds, `diff --check` prints nothing, and the diff contains only the validated helper open, focused regression tests, and the aligned design artifacts.

- [ ] **Step 6: Commit the implementation**

Run:

```bash
cd /tmp/coven-fix-612-maintenance-lock
git add crates/coven-cli/src/maintenance_gate.rs
python3 scripts/check-coven-privacy.py --staged
git commit -m "fix: prevent maintenance gate lock takeover" \
  -m "Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

Expected: the privacy guard passes, then one implementation commit records the production fix and regression tests.

### Task 3: Run Repository Gates and Open the Fix PR

**Files:**
- Verify: `crates/coven-cli/src/maintenance_gate.rs`
- Verify: `docs/superpowers/specs/2026-08-05-maintenance-gate-lock-design.md`
- Verify: `docs/superpowers/plans/2026-08-05-maintenance-gate-lock.md`

- [ ] **Step 1: Run Rust formatting and lint gates**

Run:

```bash
cd /tmp/coven-fix-612-maintenance-lock
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: both commands exit successfully with no warnings.

- [ ] **Step 2: Run the locked workspace test suite**

Run:

```bash
cd /tmp/coven-fix-612-maintenance-lock
cargo test --workspace --locked
```

Expected: all workspace tests pass.

- [ ] **Step 3: Run the secret guard**

Run:

```bash
cd /tmp/coven-fix-612-maintenance-lock
python scripts/check-secrets.py
```

Expected: the secret scan reports a clean result. The staged privacy guard already ran before each commit through the repository hook and explicitly before the implementation commit.

- [ ] **Step 4: Confirm branch scope and commit history**

Run:

```bash
cd /tmp/coven-fix-612-maintenance-lock
git status --short
git --no-pager log --oneline origin/main..HEAD
git --no-pager diff --stat origin/main...HEAD
```

Expected: the worktree is clean; history contains the design, plan, and implementation commits; the diff is limited to the two design artifacts and `maintenance_gate.rs`.

- [ ] **Step 5: Push and open the pull request**

Run:

```bash
cd /tmp/coven-fix-612-maintenance-lock
git push -u origin fix/612-maintenance-lock
gh pr create \
  --repo OpenCoven/coven \
  --base main \
  --head fix/612-maintenance-lock \
  --title "fix: prevent maintenance gate lock takeover" \
  --body $'## Summary\n- replace stale-file takeover with the existing validated lock-file helper plus `fs2` advisory locking\n- preserve the five-second contention timeout and maintenance API behavior\n- add regression coverage for live contention, release, stale mtimes, symlinks, and hard links\n\n## Upgrade note\nRestart long-running Coven daemons after upgrading so every process uses the advisory-lock protocol.\n\nCloses #612'
```

Expected: GitHub creates a pull request targeting `main` and linking issue #612.

- [ ] **Step 6: Keep the claim until integration completes**

Run:

```bash
cd /tmp/coven-fix-612-maintenance-lock
coven claim heartbeat issue-612
```

Expected: the claim remains active while the pull request is under review. Release it only when the PR merges or work stops.
