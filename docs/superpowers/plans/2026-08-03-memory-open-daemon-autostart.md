# Memory Open Daemon Auto-Start Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `coven memory open` start or reuse the local Coven daemon before launching the packaged dashboard.

**Architecture:** Keep daemon lifecycle ownership in the Rust CLI. Resolve the optional dashboard first, ensure the daemon through the existing lifecycle API, and launch the dashboard only after readiness succeeds.

**Tech Stack:** Rust, `anyhow`, Coven daemon lifecycle APIs, Cargo integration tests

---

### Task 1: Prepare Memory only after daemon readiness

**Files:**
- Modify: `crates/coven-cli/src/memory_dashboard.rs:1-115`
- Test: `crates/coven-cli/src/memory_dashboard.rs:117-160`

- [ ] **Step 1: Add failing preparation-order tests**

Add these tests inside `memory_dashboard::tests`:

```rust
#[test]
fn missing_dashboard_does_not_start_daemon() {
    let daemon_called = std::cell::Cell::new(false);

    let error = prepare_open_with(
        || None,
        || {
            daemon_called.set(true);
            Ok(())
        },
    )
    .expect_err("missing dashboard must fail");

    assert!(!daemon_called.get());
    assert!(error.to_string().contains("dashboard is not installed"));
}

#[test]
fn daemon_failure_prevents_dashboard_preparation() {
    let launch = LaunchCommand {
        program: PathBuf::from("dashboard"),
        args: Vec::new(),
    };

    let error = prepare_open_with(
        || Some(launch),
        || anyhow::bail!("socket did not become ready"),
    )
    .expect_err("daemon failure must stop launch preparation");

    assert!(error
        .to_string()
        .contains("failed to start or reach Coven daemon for Memory"));
}

#[test]
fn ready_daemon_returns_the_resolved_dashboard() {
    let launch = LaunchCommand {
        program: PathBuf::from("dashboard"),
        args: vec![OsString::from("entry")],
    };

    let prepared = prepare_open_with(
        || Some(LaunchCommand {
            program: launch.program.clone(),
            args: launch.args.clone(),
        }),
        || Ok(()),
    )
    .expect("ready daemon permits dashboard launch");

    assert_eq!(prepared, launch);
}
```

- [ ] **Step 2: Run the tests and verify the helper is missing**

Run:

```bash
cargo test -p coven-cli --bin coven memory_dashboard::tests --locked
```

Expected: compilation fails because `prepare_open_with` is not defined.

- [ ] **Step 3: Implement daemon preparation**

Add these helpers above `run_open`:

```rust
fn dashboard_not_installed_error() -> anyhow::Error {
    anyhow!(
        "The Coven Memory dashboard is not installed.\n\n  \
         npm install -g @opencoven/coven-memory-dashboard\n\n\
         Then rerun: coven memory open"
    )
}

fn ensure_daemon_ready() -> Result<()> {
    let coven_home = crate::coven_home_dir()?;
    let current_exe =
        std::env::current_exe().context("failed to resolve current executable")?;
    crate::daemon::ensure_background_server(
        &coven_home,
        &current_exe,
        crate::current_timestamp(),
    )
    .context("failed to start or reach Coven daemon for Memory")?;
    Ok(())
}

fn prepare_open_with(
    resolve_launch: impl FnOnce() -> Option<LaunchCommand>,
    ensure_daemon: impl FnOnce() -> Result<()>,
) -> Result<LaunchCommand> {
    let launch = resolve_launch().ok_or_else(dashboard_not_installed_error)?;
    ensure_daemon()?;
    Ok(launch)
}
```

Change the start of `run_open` to:

```rust
pub fn run_open() -> Result<()> {
    let launch = prepare_open_with(resolve, ensure_daemon_ready)?;
```

Keep the existing `Command::new`, exit-status handling, and launch errors
unchanged.

- [ ] **Step 4: Run focused unit tests**

Run:

```bash
cargo test -p coven-cli --bin coven memory_dashboard::tests --locked
```

Expected: all memory dashboard tests pass.

- [ ] **Step 5: Commit the launch preparation**

```bash
git add crates/coven-cli/src/memory_dashboard.rs
git commit -s -m "fix(memory): start daemon before dashboard"
```

### Task 2: Verify the packaged launch contract end to end

**Files:**
- Create: `crates/coven-cli/tests/memory_dashboard.rs`

- [ ] **Step 1: Add a Unix process-level regression test**

Create `crates/coven-cli/tests/memory_dashboard.rs`:

```rust
#[cfg(unix)]
mod unix {
    use anyhow::Result;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    fn coven_bin() -> PathBuf {
        PathBuf::from(env!("CARGO_BIN_EXE_coven"))
    }

    fn stop_daemon(coven_home: &Path) {
        let _ = Command::new(coven_bin())
            .args(["daemon", "stop"])
            .env("COVEN_HOME", coven_home)
            .output();
    }

    #[test]
    fn memory_open_starts_daemon_before_dashboard() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let coven_home = temp.path().join("coven-home");
        fs::create_dir_all(&coven_home)?;
        let marker = coven_home.join("dashboard-launched");
        let dashboard = temp.path().join("fake-dashboard");
        fs::write(
            &dashboard,
            "#!/bin/sh\n\
             test -S \"$COVEN_HOME/coven.sock\" || exit 42\n\
             printf launched > \"$COVEN_TEST_DASHBOARD_MARKER\"\n",
        )?;
        fs::set_permissions(&dashboard, fs::Permissions::from_mode(0o755))?;

        let output = Command::new(coven_bin())
            .args(["memory", "open"])
            .env("COVEN_HOME", &coven_home)
            .env("COVEN_MEMORY_DASHBOARD_BIN", &dashboard)
            .env("COVEN_TEST_DASHBOARD_MARKER", &marker)
            .output()?;
        stop_daemon(&coven_home);

        assert!(
            output.status.success(),
            "memory open failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(fs::read_to_string(marker)?, "launched");
        Ok(())
    }
}
```

- [ ] **Step 2: Run the process-level test**

Run:

```bash
cargo test -p coven-cli --test memory_dashboard --locked
```

Expected: the test passes and its temporary daemon is stopped during cleanup.

- [ ] **Step 3: Commit the regression test**

```bash
git add crates/coven-cli/tests/memory_dashboard.rs
git commit -s -m "test(memory): cover daemon-backed dashboard launch"
```

### Task 3: Document the standalone packaged workflow

**Files:**
- Modify: `README.md:151-160`
- Modify: `README.md:304-308`
- Modify: `docs/reference/cli-observe.md:35-45`

- [ ] **Step 1: Update the root installation guidance**

After the `coven memory open` installation example, add:

```markdown
`coven memory open` starts or reuses the installed local Coven daemon before
launching the packaged dashboard. It does not require a checkout or running
development server from the `coven-memory` repository.
```

In the architecture paragraph near the bottom, state that the native CLI
establishes daemon readiness before delegating to the installed dashboard.

- [ ] **Step 2: Update the command reference**

In `docs/reference/cli-observe.md`, replace the first paragraph under
`## coven memory open` with:

```markdown
`coven memory open` starts or reuses the local Coven daemon, then launches the
optional `@opencoven/coven-memory-dashboard` companion on a validated loopback
address. It does not require the dashboard source repository or a separate
`coven daemon start` step. It does not accept `--json`; `coven memory` and
`coven memory --json` retain the read-only list behavior above.
```

- [ ] **Step 3: Commit documentation**

```bash
git add README.md docs/reference/cli-observe.md
git commit -s -m "docs(memory): explain standalone dashboard launch"
```

### Task 4: Validate and publish

**Files:**
- Create: `docs/superpowers/plans/2026-08-03-memory-open-daemon-autostart.md`

- [ ] **Step 1: Run focused tests and formatting**

Run:

```bash
cargo test -p coven-cli --bin coven memory_dashboard::tests --locked
cargo test -p coven-cli --test memory_dashboard --locked
cargo fmt --check
git diff --check
```

Expected: all commands pass.

- [ ] **Step 2: Run workspace quality gates**

Run:

```bash
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

Expected: all commands pass. Investigate any failure before publishing; do not
skip a newly introduced failure.

- [ ] **Step 3: Commit the implementation plan**

```bash
git add docs/superpowers/plans/2026-08-03-memory-open-daemon-autostart.md
git commit -s -m "docs(memory): plan daemon auto-start"
```

- [ ] **Step 4: Push and open the pull request**

```bash
git push -u origin fix/memory-open-auto-daemon
gh pr create \
  --base main \
  --head fix/memory-open-auto-daemon \
  --title "fix(memory): start daemon before dashboard" \
  --body "$(cat <<'EOF'
## Summary
- start or reuse the local Coven daemon before launching the packaged Memory dashboard
- fail in the terminal instead of opening a dashboard that can only return 503
- add unit, process-level, and documentation coverage for the standalone workflow

## Validation
- `cargo test -p coven-cli --bin coven memory_dashboard::tests --locked`
- `cargo test -p coven-cli --test memory_dashboard --locked`
- `cargo clippy --workspace --all-targets --locked -- -D warnings`
- `cargo test --workspace --locked`
EOF
)"
```

The PR body must summarize daemon auto-start, fail-before-launch behavior,
process-level coverage, documentation, and validation.

- [ ] **Step 5: Update Beads**

```bash
bd comments add cmem-h06 "Implemented daemon auto-start for coven memory open and opened the Coven pull request."
bd close cmem-h06 --reason="Packaged Memory launch now establishes daemon readiness before opening the dashboard."
```
