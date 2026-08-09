# PTY SIGTERM Load-Resilience Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the Linux/macOS native-stream SIGTERM test verify cancellation
and process-tree reaping without an atomic-ack race, pre-activation signalling,
or destructive PID-file cleanup.

**Architecture:** The production stream runner is unchanged. The test observer
holds its test-only lifecycle lock across `pthread_kill`; the runner's expected
cancellation error is the acknowledgement that matters. The signaler waits
outside locks until fixture identities and guard activation coexist. On
fixture-start watchdog expiry, the fallback rechecks the owner-scoped guard
lifecycle under the lifecycle mutex: if it is still active, it sends a
thread-directed SIGTERM and then records a fixture-start failure; if it is
inactive, it records the failure without signalling. The stream returns and
identity-safe cleanup plus aggregated assertions run. This containment path is
not normal cancellation or a performance assertion, and it never signals under
the restored/default handler or changes async handler semantics. Linux
identities use `/proc/<pid>/stat` start ticks; macOS identities use
`proc_pidinfo(PROC_PIDTBSDINFO)` start timestamps.

**Tech Stack:** Rust, `libc`, Cargo test runner, POSIX shell test fixture.

---

## File structure

| File | Responsibility |
| --- | --- |
| `crates/coven-cli/src/pty_runner.rs` | Linux/macOS native-stream SIGTERM unit test, durable cleanup protocol, and behavior assertions. |
| `docs/superpowers/specs/2026-08-09-pty-sigterm-load-resilience-design.md` | Safety rationale and platform identity contract. |

### Task 1: Synchronize delivery and use durable fixture cleanup

**Files:**
- Modify: `crates/coven-cli/src/pty_runner.rs:4350-4444`
- Test: `crates/coven-cli/src/pty_runner.rs:4350-4444`

- [ ] **Step 1: Run the existing targeted test**

Run:

```sh
cargo test -p coven-cli pty_runner::tests::native_stream_sigterm_cancels_and_reaps_process_tree -- --exact --nocapture
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
    // Keep the expected cancellation error and restoration assertion. Capture
    // fixture start identities before signalling, and retain a tokenized FIFO
    // cleanup sentinel created before the shell fixture starts.
```

The signaler repeatedly captures both fixture identities and calls
`send_sigterm_if_active` only when capture succeeds. A `false` result means
the lifecycle is not yet active, not that the test should fail; it releases the
lock and retries until the watchdog. Do not poll
`SUPERVISED_STREAM_CANCELLATION_SIGNAL` after `pthread_kill`: guard finish may
clear it. The final expected cancellation error is the consumption assertion.

On expiry, if the owner-scoped guard lifecycle is still active, send a
thread-directed SIGTERM while holding the lifecycle mutex, then record a
fixture-start failure; if the lifecycle is inactive, record the failure
without signalling. After the runner returns, classify each captured PID by its
recorded platform start identity. PID reuse is a failure, not evidence of
reaping. The fallback is a containment failure path, not normal cancellation or
a performance assertion, and it never signals under the restored/default
handler or changes async handler semantics.

- [ ] **Step 3: Format and run the renamed test**

Run:

```sh
cargo fmt --check
cargo test -p coven-cli pty_runner::tests::native_stream_sigterm_cancels_and_reaps_process_tree -- --exact --nocapture
```

Expected: formatting passes, and the test proves the cancellation error,
handler restoration, and durable fixture reaping. Any watchdog fallback is a
containment failure path that still runs identity-safe cleanup and aggregated
assertions.

- [ ] **Step 4: Run the targeted test under deliberate CPU load**

Run the target under a Python supervisor that starts exactly 24 local spinners
and terminates only those children:

```sh
python3 - <<'PY'
import os
import signal
import subprocess
import sys

spinners = [
    subprocess.Popen(["yes"], stdout=subprocess.DEVNULL)
    for _ in range(24)
]
try:
    sys.exit(subprocess.run([
        "cargo", "test", "-p", "coven-cli",
        "pty_runner::tests::native_stream_sigterm_cancels_and_reaps_process_tree",
        "--", "--exact", "--nocapture",
    ]).returncode)
finally:
    for spinner in spinners:
        spinner.terminate()
    for spinner in spinners:
        try:
            spinner.wait(timeout=5)
        except subprocess.TimeoutExpired:
            spinner.kill()
            spinner.wait()
PY
```

Expected: the test passes under load. It may take longer than two seconds,
which is acceptable because correctness is the cancellation error plus
process-tree reaping. If the watchdog fallback is taken, it must do so via the
owner-scoped lifecycle-mutex path described above, not via FIFO cleanup or
unconditional SIGTERM.

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
