# PTY SIGTERM Load-Resilience Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the Unix native-stream SIGTERM test verify cancellation and process-tree reaping without asserting a machine-load-dependent completion time.

**Architecture:** The production stream runner is unchanged. Its Unix unit test will retain the expected cancellation error, restored signal-handler, and `ESRCH` process-tree assertions, replacing only short fixture/reaping deadlines with a shared watchdog and deleting the 2-second elapsed-time assertion.

**Tech Stack:** Rust, `libc`, Cargo test runner, POSIX shell test fixture.

---

## File structure

| File | Responsibility |
| --- | --- |
| `crates/coven-cli/src/pty_runner.rs` | Unix native-stream SIGTERM unit test and its behavior assertions. |

### Task 1: Replace the timing proxy with behavior checks

**Files:**
- Modify: `crates/coven-cli/src/pty_runner.rs:4350-4444`
- Test: `crates/coven-cli/src/pty_runner.rs:4350-4444`

- [ ] **Step 1: Run the existing targeted test**

Run:

```sh
cargo test -p coven-cli native_stream_sigterm_returns_promptly_and_reaps_process_tree -- --exact --nocapture
```

Expected: PASS on an unloaded machine. The existing body contains
`started.elapsed() < Duration::from_secs(2)`, which is the load-sensitive
assertion this task removes.

- [ ] **Step 2: Edit the test body**

Rename the test and replace its short deadlines with the shared local watchdog:

```rust
#[cfg(unix)]
#[test]
fn native_stream_sigterm_cancels_and_reaps_process_tree() -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let watchdog = Duration::from_secs(30);
    // Keep the existing fixture, signaler, cancellation-error assertion,
    // SIGTERM-handler restoration assertion, and harness/descendant loop.
```

In the signaler closure, replace:

```rust
let deadline = Instant::now() + Duration::from_secs(3);
```

with:

```rust
let deadline = Instant::now() + watchdog;
```

Delete:

```rust
let started = Instant::now();
```

and delete the entire assertion:

```rust
assert!(
    started.elapsed() < Duration::from_secs(2),
    "native stream cancellation was not prompt"
);
```

In the final harness/descendant loop, replace:

```rust
let deadline = Instant::now() + Duration::from_secs(1);
```

with:

```rust
let deadline = Instant::now() + watchdog;
```

Do not change the expected cancellation-error text, signal-handler restoration
check, `kill(pid, 0) == -1` assertion, or `ESRCH` assertion. The watchdog
prevents a broken test fixture from hanging; it is not a performance
requirement.

- [ ] **Step 3: Format and run the renamed test**

Run:

```sh
cargo fmt --check
cargo test -p coven-cli native_stream_sigterm_cancels_and_reaps_process_tree -- --exact --nocapture
```

Expected: formatting passes, and the renamed test passes while still reporting
the cancellation error and reaping both fixture PIDs.

- [ ] **Step 4: Run the targeted test under deliberate CPU load**

Run the test while 24 exact, locally started spinner PIDs consume CPU; terminate
only those captured PIDs after the test:

```sh
pids=()
for _ in $(seq 1 24); do
  yes > /dev/null &
  pids+=("$!")
done
trap 'for pid in "${pids[@]}"; do kill "$pid" 2>/dev/null || true; done; wait' EXIT
cargo test -p coven-cli native_stream_sigterm_cancels_and_reaps_process_tree -- --exact --nocapture
```

Expected: the test passes under load. It may take longer than two seconds,
which is acceptable because correctness is the cancellation error plus
process-tree reaping.

- [ ] **Step 5: Commit the test fix**

```sh
git add crates/coven-cli/src/pty_runner.rs
git commit -m "test: remove flaky PTY SIGTERM timing bound" \
  -m "Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

### Task 2: Verify the package and workspace gates

**Files:**
- No additional files.

- [ ] **Step 1: Run the CLI package tests**

Run:

```sh
cargo test -p coven-cli --locked
```

Expected: all non-ignored `coven-cli` tests pass.

- [ ] **Step 2: Run required workspace checks**

Run:

```sh
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --locked
python scripts/check-secrets.py
git add crates/coven-cli/src/pty_runner.rs
python3 scripts/check-coven-privacy.py --staged
```

Expected: every command exits successfully. The staged privacy guard examines
only the PTY test change.

- [ ] **Step 3: Inspect the scoped diff**

Run:

```sh
git diff --check origin/main...HEAD
git diff --stat origin/main...HEAD
```

Expected: no whitespace errors; the implementation diff changes only
`crates/coven-cli/src/pty_runner.rs` in addition to the already committed
design and plan records.
