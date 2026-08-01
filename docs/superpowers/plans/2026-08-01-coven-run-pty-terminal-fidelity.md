# Coven Run PTY Terminal Fidelity Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make interactive `coven run` sessions start at the caller's real terminal size and remain synchronized as the pane resizes, preserving the native harness UI without redraw artifacts.

**Architecture:** Keep all behavior in `crates/coven-cli/src/pty_runner.rs`. A pure geometry resolver chooses live terminal dimensions before environment/default fallbacks; a scoped polling watcher owns the attached PTY master, deduplicates geometry samples, calls `MasterPty::resize`, and joins before output teardown. Detached, captured, stream-JSON, and command-building paths remain unchanged.

**Tech Stack:** Rust, crossterm 0.29 terminal geometry, portable-pty 0.9 `MasterPty`, standard-library threads and channels, existing Cargo test/Clippy/privacy gates.

---

## File map

- Modify: `crates/coven-cli/src/pty_runner.rs`
  - resolve startup geometry from the actual terminal;
  - apply and relay live PTY resizes;
  - scope watcher lifetime inside the attached runner;
  - host unit and Unix real-PTY regressions.
- Reference: `docs/superpowers/specs/2026-08-01-coven-run-pty-terminal-fidelity-design.md`
  - approved behavior and acceptance criteria; no implementation edit required.

No dependency, command-builder, daemon, store, or harness-adapter files should change.

### Task 1: Resolve startup geometry from the connected terminal

**Files:**
- Modify: `crates/coven-cli/src/pty_runner.rs:1-18`
- Modify: `crates/coven-cli/src/pty_runner.rs:2297-2310`
- Test: `crates/coven-cli/src/pty_runner.rs` inline `tests` module

- [x] **Step 1: Write failing geometry-precedence tests**

Add these helpers and tests near the start of the existing `tests` module:

```rust
fn pty_size(rows: u16, cols: u16, pixel_width: u16, pixel_height: u16) -> PtySize {
    PtySize {
        rows,
        cols,
        pixel_width,
        pixel_height,
    }
}

#[test]
fn pty_geometry_prefers_live_terminal_size_and_pixels() {
    let live = pty_size(52, 151, 1812, 936);

    assert_eq!(
        terminal_size_from_sources(Some(live), Some(24), Some(80)),
        live,
    );
}

#[test]
fn pty_geometry_rejects_zero_live_dimensions() {
    let invalid = pty_size(0, 151, 1812, 936);

    assert_eq!(
        terminal_size_from_sources(Some(invalid), Some(41), Some(132)),
        pty_size(41, 132, 0, 0),
    );
}

#[test]
fn pty_geometry_uses_each_environment_fallback_independently() {
    assert_eq!(
        terminal_size_from_sources(None, Some(41), None),
        pty_size(41, 80, 0, 0),
    );
    assert_eq!(
        terminal_size_from_sources(None, None, Some(132)),
        pty_size(24, 132, 0, 0),
    );
}

#[test]
fn pty_geometry_defaults_when_every_source_is_unavailable() {
    assert_eq!(
        terminal_size_from_sources(None, None, None),
        pty_size(24, 80, 0, 0),
    );
}
```

- [x] **Step 2: Run the tests and confirm the intended RED failure**

Run:

```sh
cargo test -p coven-cli pty_runner::tests::pty_geometry_ -- --nocapture
```

Expected: compilation fails because `terminal_size_from_sources` does not exist. A pass is not acceptable at this step.

- [x] **Step 3: Implement live geometry detection and fallback resolution**

Extend the crossterm import and replace the current environment-only resolver with:

```rust
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, window_size};

const DEFAULT_PTY_ROWS: u16 = 24;
const DEFAULT_PTY_COLS: u16 = 80;

fn detected_terminal_size() -> Option<PtySize> {
    if !io::stdout().is_terminal() {
        return None;
    }
    let window = window_size().ok()?;
    valid_pty_size(PtySize {
        rows: window.rows,
        cols: window.columns,
        pixel_width: window.width,
        pixel_height: window.height,
    })
}

fn valid_pty_size(size: PtySize) -> Option<PtySize> {
    (size.rows > 0 && size.cols > 0).then_some(size)
}

fn terminal_size_from_sources(
    terminal: Option<PtySize>,
    env_rows: Option<u16>,
    env_cols: Option<u16>,
) -> PtySize {
    terminal.and_then(valid_pty_size).unwrap_or(PtySize {
        rows: env_rows.unwrap_or(DEFAULT_PTY_ROWS),
        cols: env_cols.unwrap_or(DEFAULT_PTY_COLS),
        pixel_width: 0,
        pixel_height: 0,
    })
}

fn terminal_size() -> PtySize {
    terminal_size_from_sources(
        None,
        env_u16("LINES"),
        env_u16("COLUMNS"),
    )
}

fn attached_terminal_size() -> PtySize {
    terminal_size_from_sources(
        detected_terminal_size(),
        env_u16("LINES"),
        env_u16("COLUMNS"),
    )
}
```

Keep `env_u16` unchanged so malformed, zero, and missing values remain filtered.
Keep the existing captured and detached `openpty(terminal_size())` calls unchanged;
only the interactive attached runner will call `attached_terminal_size()`.

- [x] **Step 4: Run focused tests and formatting**

Run:

```sh
cargo test -p coven-cli pty_runner::tests::pty_geometry_ -- --nocapture
cargo fmt --check
git diff --check
```

Expected: four geometry tests pass; formatting and diff checks exit zero.

- [x] **Step 5: Stage, run privacy/secret verification, and commit**

Run:

```sh
git add crates/coven-cli/src/pty_runner.rs
python3 scripts/check-coven-privacy.py --staged
python scripts/check-secrets.py
git diff --cached --check
git commit -m "fix(run): detect attached terminal geometry"
```

Expected: both guards pass and the commit contains only the resolver and its tests.

### Task 2: Make resize decisions deterministic and testable

**Files:**
- Modify: `crates/coven-cli/src/pty_runner.rs:1-20`
- Modify: `crates/coven-cli/src/pty_runner.rs` immediately before `run_attached_with_pty_system`
- Test: `crates/coven-cli/src/pty_runner.rs` inline `tests` module

- [x] **Step 1: Write failing resize-decision tests**

Add a focused test target and these tests:

```rust
#[derive(Clone)]
struct RecordingResizeTarget {
    sizes: Arc<Mutex<Vec<PtySize>>>,
    fail: bool,
}

impl PtyResizeTarget for RecordingResizeTarget {
    fn resize_pty(&self, size: PtySize) -> Result<()> {
        if self.fail {
            anyhow::bail!("synthetic resize failure");
        }
        self.sizes.lock().unwrap().push(size);
        Ok(())
    }
}

#[test]
fn pty_resize_ignores_missing_invalid_and_unchanged_samples() {
    let sizes = Arc::new(Mutex::new(Vec::new()));
    let target = RecordingResizeTarget {
        sizes: Arc::clone(&sizes),
        fail: false,
    };
    let initial = pty_size(24, 80, 0, 0);
    let mut current = initial;

    assert!(apply_pty_resize(&target, &mut current, None));
    assert!(apply_pty_resize(
        &target,
        &mut current,
        Some(pty_size(0, 120, 0, 0)),
    ));
    assert!(apply_pty_resize(&target, &mut current, Some(initial)));
    assert!(sizes.lock().unwrap().is_empty());
    assert_eq!(current, initial);
}

#[test]
fn pty_resize_applies_each_distinct_complete_geometry_once() {
    let sizes = Arc::new(Mutex::new(Vec::new()));
    let target = RecordingResizeTarget {
        sizes: Arc::clone(&sizes),
        fail: false,
    };
    let initial = pty_size(24, 80, 0, 0);
    let resized = pty_size(40, 120, 1440, 800);
    let pixels_only = pty_size(40, 120, 1680, 900);
    let mut current = initial;

    assert!(apply_pty_resize(&target, &mut current, Some(resized)));
    assert!(apply_pty_resize(&target, &mut current, Some(resized)));
    assert!(apply_pty_resize(&target, &mut current, Some(pixels_only)));

    assert_eq!(*sizes.lock().unwrap(), vec![resized, pixels_only]);
    assert_eq!(current, pixels_only);
}

#[test]
fn pty_resize_failure_stops_relay_without_advancing_geometry() {
    let target = RecordingResizeTarget {
        sizes: Arc::new(Mutex::new(Vec::new())),
        fail: true,
    };
    let initial = pty_size(24, 80, 0, 0);
    let mut current = initial;

    assert!(!apply_pty_resize(
        &target,
        &mut current,
        Some(pty_size(40, 120, 0, 0)),
    ));
    assert_eq!(current, initial);
}
```

- [x] **Step 2: Run the tests and confirm the intended RED failure**

Run:

```sh
cargo test -p coven-cli pty_runner::tests::pty_resize_ -- --nocapture
```

Expected: compilation fails because `PtyResizeTarget` and `apply_pty_resize` do not exist.

- [x] **Step 3: Add the private resize target boundary and decision helper**

Import `MasterPty` and add:

```rust
use portable_pty::{
    native_pty_system, ChildKiller, CommandBuilder, MasterPty, PtySize, PtySystem,
};

trait PtyResizeTarget: Send {
    fn resize_pty(&self, size: PtySize) -> Result<()>;
}

impl PtyResizeTarget for Box<dyn MasterPty + Send> {
    fn resize_pty(&self, size: PtySize) -> Result<()> {
        self.resize(size)
    }
}

fn apply_pty_resize(
    target: &dyn PtyResizeTarget,
    current: &mut PtySize,
    next: Option<PtySize>,
) -> bool {
    let Some(next) = next.and_then(valid_pty_size) else {
        return true;
    };
    if next == *current {
        return true;
    }
    if target.resize_pty(next).is_err() {
        return false;
    }
    *current = next;
    true
}
```

The boolean means “continue watching”; resize errors remain quiet so they cannot corrupt native TUI output.

- [x] **Step 4: Run focused tests and formatting**

Run:

```sh
cargo test -p coven-cli pty_runner::tests::pty_resize_ -- --nocapture
cargo fmt --check
git diff --check
```

Expected: three resize-decision tests pass and checks exit zero.

- [x] **Step 5: Stage, verify, and commit**

Run:

```sh
git add crates/coven-cli/src/pty_runner.rs
python3 scripts/check-coven-privacy.py --staged
python scripts/check-secrets.py
git diff --cached --check
git commit -m "fix(run): deduplicate attached PTY resizes"
```

Expected: verified commit containing only the private resize boundary/helper and tests.

### Task 3: Relay live geometry for the attached runner

**Files:**
- Modify: `crates/coven-cli/src/pty_runner.rs:1-20`
- Modify: `crates/coven-cli/src/pty_runner.rs:2216-2273`
- Test: `crates/coven-cli/src/pty_runner.rs` inline `tests` module

- [x] **Step 1: Write failing watcher lifecycle tests**

Add imports inside the test module as needed and add:

```rust
use std::collections::VecDeque;

struct DropAwareResizeTarget {
    sizes: mpsc::Sender<PtySize>,
    dropped: Option<mpsc::Sender<()>>,
    fail: bool,
}

impl PtyResizeTarget for DropAwareResizeTarget {
    fn resize_pty(&self, size: PtySize) -> Result<()> {
        if self.fail {
            anyhow::bail!("synthetic resize failure");
        }
        self.sizes
            .send(size)
            .map_err(|error| anyhow::anyhow!(error.to_string()))
    }
}

impl Drop for DropAwareResizeTarget {
    fn drop(&mut self) {
        if let Some(dropped) = self.dropped.take() {
            let _ = dropped.send(());
        }
    }
}

#[test]
fn pty_resize_watcher_skips_duplicates_and_survives_missing_samples() {
    let initial = pty_size(24, 80, 0, 0);
    let resized = pty_size(40, 120, 1440, 800);
    let samples = Arc::new(Mutex::new(VecDeque::from([
        None,
        Some(initial),
        Some(resized),
        Some(resized),
    ])));
    let samples_for_source = Arc::clone(&samples);
    let (sizes_tx, sizes_rx) = mpsc::channel();
    let (dropped_tx, dropped_rx) = mpsc::channel();
    let target = DropAwareResizeTarget {
        sizes: sizes_tx,
        dropped: Some(dropped_tx),
        fail: false,
    };

    let mut watcher = PtyResizeWatcher::spawn_with_source(
        target,
        initial,
        move || samples_for_source.lock().unwrap().pop_front().flatten(),
        Duration::from_millis(1),
    );

    assert_eq!(sizes_rx.recv_timeout(Duration::from_secs(1)).unwrap(), resized);
    assert!(sizes_rx.recv_timeout(Duration::from_millis(25)).is_err());
    watcher.stop();
    dropped_rx.recv_timeout(Duration::from_secs(1)).unwrap();
}

#[test]
fn pty_resize_watcher_drop_interrupts_long_poll_and_drops_target() {
    let (sizes_tx, _sizes_rx) = mpsc::channel();
    let (dropped_tx, dropped_rx) = mpsc::channel();
    let target = DropAwareResizeTarget {
        sizes: sizes_tx,
        dropped: Some(dropped_tx),
        fail: false,
    };

    let watcher = PtyResizeWatcher::spawn_with_source(
        target,
        pty_size(24, 80, 0, 0),
        || None,
        Duration::from_secs(60),
    );
    drop(watcher);

    dropped_rx.recv_timeout(Duration::from_secs(1)).unwrap();
}

#[test]
fn pty_resize_watcher_exits_and_drops_target_after_resize_failure() {
    let (sizes_tx, _sizes_rx) = mpsc::channel();
    let (dropped_tx, dropped_rx) = mpsc::channel();
    let target = DropAwareResizeTarget {
        sizes: sizes_tx,
        dropped: Some(dropped_tx),
        fail: true,
    };

    let _watcher = PtyResizeWatcher::spawn_with_source(
        target,
        pty_size(24, 80, 0, 0),
        || Some(pty_size(40, 120, 0, 0)),
        Duration::from_millis(1),
    );

    dropped_rx.recv_timeout(Duration::from_secs(1)).unwrap();
}
```

- [x] **Step 2: Write the failing Unix real-PTY resize regression**

Add:

```rust
#[cfg(unix)]
#[test]
fn pty_resize_watcher_updates_real_child_geometry() -> anyhow::Result<()> {
    let initial = pty_size(24, 80, 0, 0);
    let resized = pty_size(40, 120, 0, 0);
    let pair = native_pty_system().openpty(initial)?;
    let mut command = CommandBuilder::new("sh");
    command.args([
        "-c",
        "trap 'stty size; exit 0' WINCH; stty size; while :; do sleep 1; done",
    ]);
    let mut child = pair.slave.spawn_command(command)?;
    drop(pair.slave);
    let reader = pair.master.try_clone_reader()?;
    let _writer = pair.master.take_writer()?;
    let mut reader = BufReader::new(reader);
    let mut first = String::new();
    reader.read_line(&mut first)?;
    assert_eq!(first.trim(), "24 80");

    let mut watcher = PtyResizeWatcher::spawn_with_source(
        pair.master,
        initial,
        move || Some(resized),
        Duration::from_millis(1),
    );
    let status = child.wait()?;
    watcher.stop();

    let mut remainder = String::new();
    reader.read_to_string(&mut remainder)?;
    assert!(status.success());
    assert!(remainder.lines().any(|line| line.trim() == "40 120"));
    Ok(())
}
```

- [x] **Step 3: Run the tests and confirm the intended RED failure**

Run:

```sh
cargo test -p coven-cli pty_runner::tests::pty_resize_watcher_ -- --nocapture
```

Expected: compilation fails because `PtyResizeWatcher` does not exist.

- [x] **Step 4: Implement the scoped polling watcher**

Add this before `run_attached_with_pty_system`:

```rust
const PTY_RESIZE_POLL_INTERVAL: Duration = Duration::from_millis(100);

struct PtyResizeWatcher {
    stop: Option<mpsc::Sender<()>>,
    join: Option<thread::JoinHandle<()>>,
}

impl PtyResizeWatcher {
    fn spawn(master: Box<dyn MasterPty + Send>, initial: PtySize) -> Self {
        Self::spawn_with_source(
            master,
            initial,
            detected_terminal_size,
            PTY_RESIZE_POLL_INTERVAL,
        )
    }

    fn spawn_with_source<T, S>(
        target: T,
        initial: PtySize,
        mut size_source: S,
        interval: Duration,
    ) -> Self
    where
        T: PtyResizeTarget + 'static,
        S: FnMut() -> Option<PtySize> + Send + 'static,
    {
        let (stop, stopped) = mpsc::channel();
        let join = thread::spawn(move || {
            let mut current = initial;
            loop {
                match stopped.recv_timeout(interval) {
                    Ok(()) | Err(RecvTimeoutError::Disconnected) => break,
                    Err(RecvTimeoutError::Timeout) => {}
                }
                if !apply_pty_resize(&target, &mut current, size_source()) {
                    break;
                }
            }
        });
        Self {
            stop: Some(stop),
            join: Some(join),
        }
    }

    fn stop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

impl Drop for PtyResizeWatcher {
    fn drop(&mut self) {
        self.stop();
    }
}
```

- [x] **Step 5: Integrate watcher ownership into the attached runner**

Update `run_attached_with_pty_system` so the startup size is retained and the master is either watched or held locally:

```rust
fn run_attached_with_pty_system(
    command: &HarnessCommand,
    pty_system: &(dyn PtySystem + Send),
) -> Result<PtyRunResult> {
    let initial_size = attached_terminal_size();
    let pair = pty_system
        .openpty(initial_size)
        .context("failed to open PTY")?;
    let mut child = pair
        .slave
        .spawn_command(command.to_command_builder())
        .with_context(|| format!("failed to spawn harness `{}`", command.program()))?;

    drop(pair.slave);

    let master = pair.master;
    let mut reader = master
        .try_clone_reader()
        .context("failed to clone PTY reader")?;
    let mut writer = master
        .take_writer()
        .context("failed to open PTY writer")?;
    let interactive = io::stdin().is_terminal() && io::stdout().is_terminal();
    let mut master = Some(master);
    let mut resize_watcher = interactive.then(|| {
        PtyResizeWatcher::spawn(
            master.take().expect("interactive PTY master is available"),
            initial_size,
        )
    });
    let _held_master = master;
    let _raw_mode =
        RawModeGuard::enable_if_terminal().context("failed to enable raw terminal mode")?;

    let output_thread = thread::spawn(move || {
        let mut stdout = io::stdout().lock();
        io::copy(&mut reader, &mut stdout)?;
        stdout.flush()
    });

    if io::stdin().is_terminal() {
        thread::spawn(move || {
            let mut stdin = io::stdin().lock();
            let _ = io::copy(&mut stdin, &mut writer);
        });
    }

    let exit_status = child.wait().context("failed to wait for harness process")?;
    if let Some(watcher) = resize_watcher.as_mut() {
        watcher.stop();
    }
    let _ = output_thread.join();
    let exit_code = i32::try_from(exit_status.exit_code()).unwrap_or(i32::MAX);
    let status = if exit_status.success() {
        "completed"
    } else {
        "failed"
    };

    Ok(PtyRunResult {
        status,
        exit_code: Some(exit_code),
    })
}
```

Do not alter stdin/stdout copying, raw-mode behavior, command args, captured output, detached sessions, or stream-JSON routing.

- [x] **Step 6: Run focused and surrounding PTY tests**

Run:

```sh
cargo test -p coven-cli pty_runner::tests::pty_ -- --nocapture
cargo test -p coven-cli pty_runner::tests::builds_codex_command_without_shell_interpolation -- --exact
cargo fmt --check
git diff --check
```

Expected: all new geometry/watcher tests and the existing Codex command test pass; formatting and diff checks exit zero.

- [x] **Step 7: Stage, verify, and commit**

Run:

```sh
git add crates/coven-cli/src/pty_runner.rs
python3 scripts/check-coven-privacy.py --staged
python scripts/check-secrets.py
git diff --cached --check
git commit -m "fix(run): forward live attached PTY geometry"
```

Expected: verified commit containing watcher lifecycle, attached-runner integration, and regressions.

### Task 4: Verify the screenshot-derived user experience and repository gates

**Files:**
- Verify: `crates/coven-cli/src/pty_runner.rs`
- Verify: `docs/superpowers/specs/2026-08-01-coven-run-pty-terminal-fidelity-design.md`
- Verify: `docs/superpowers/plans/2026-08-01-coven-run-pty-terminal-fidelity.md`

- [x] **Step 1: Run focused regression proof without concurrent Cargo jobs**

Run:

```sh
cargo test -p coven-cli pty_runner::tests::pty_ -- --nocapture
```

Expected: all new tests pass, including the Unix child transition from `24 80` to `40 120`.

- [x] **Step 2: Run the required Rust gates**

Run serially:

```sh
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --locked
```

Expected: each exits zero. If the known baseline test
`codex_json_sigterm_reaps_descendants_and_marks_ledger_failed` misses its fixed
three-second deadline under unrelated machine load, record the full output, allow
all competing Cargo jobs to finish, and rerun that exact test once from the already
built test binary. Do not change its timeout in this PR.

- [x] **Step 3: Run secret, privacy, and diff gates**

Run:

```sh
python scripts/check-secrets.py
git add crates/coven-cli/src/pty_runner.rs docs/superpowers/plans/2026-08-01-coven-run-pty-terminal-fidelity.md
python3 scripts/check-coven-privacy.py --staged
git diff --cached --check
```

Expected: all checks pass. If there are no uncommitted implementation changes, the staged privacy scan should still cover the plan update only.

- [x] **Step 4: Perform a wide-terminal manual smoke**

From a real terminal pane wider than 120 columns, run:

```sh
cargo build -p coven-cli
target/debug/coven run codex "Reply with exactly PTY_OK."
```

While the native Codex composer remains open:

1. Confirm the frame spans the current pane rather than stopping near column 80.
2. Narrow the pane and confirm one coherent reflow with a single composer.
3. Widen the pane and confirm content uses the restored width.
4. Exit Codex and confirm the shell's echo/canonical mode is restored.

Expected: full-width native layout, correct wrapping, no stale duplicate composer, and a healthy terminal after exit.

- [x] **Step 5: Audit the exact acceptance criteria**

Record evidence for every approved requirement:

```text
[x] startup columns/rows come from the connected terminal
[x] startup pixel geometry is preserved when available
[x] environment/default fallbacks remain valid
[x] live resizes reach the child PTY
[x] identical samples do not redraw the child
[x] watcher exits on shutdown and resize failure
[x] native input/output and raw-mode restoration are unchanged
[x] detached/captured/stream-json paths have no diff
[x] focused tests, broad gates, secret scan, and privacy scan pass
[x] wide-terminal Codex smoke shows one aligned composer
```

Expected: every box has direct command, diff, or visual evidence. Any unchecked item keeps issue #540 incomplete.

#### Verification record (2026-08-01)

- `cargo test -p coven-cli pty_runner::tests::pty_ -- --nocapture` passed all
  13 focused tests, including the Unix child transition from `24 80` to `40 120`.
- `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
  and `cargo test --workspace --locked` exited zero. The main `coven-cli` suite
  reported 1,558 passed, zero failed, and two ignored; the known fixed-deadline
  SIGTERM regression also passed in the normal workspace run.
- Geometry tests cover live cell and pixel precedence, cell-only terminal fallback,
  invalid terminal samples, independent environment dimensions, and 24-by-80
  defaults. Resize tests cover deduplication, transient missing samples, resize
  failure without advancing geometry, prompt shutdown, target lifetime, and the
  real child PTY resize.
- The cumulative diff changes the attached runner plus its inline tests only;
  detached and captured `openpty(terminal_size())` call sites and stream-JSON
  routing have no functional diff. The native stdin/stdout byte copies and
  `RawModeGuard` remain in place.
- A real macOS PTY started at 40 rows by 140 columns, ran
  `target/debug/coven run codex "Reply with exactly PTY_OK."`, and returned
  `PTY_OK`. Changing the same device to 28-by-72 produced a coherent narrow
  reflow; restoring 40-by-140 restored the wide layout with one composer. The
  command exited zero, and `stty -g` was byte-identical before and after.

- [x] **Step 6: Commit any final plan-only checkbox updates after verification**

If execution checkboxes were updated after the implementation commits, run:

```sh
git add docs/superpowers/plans/2026-08-01-coven-run-pty-terminal-fidelity.md
python3 scripts/check-coven-privacy.py --staged
git diff --cached --check
git commit -m "docs: record PTY fidelity verification"
```

Expected: clean worktree with a squashable sequence and no unverified code commit.
