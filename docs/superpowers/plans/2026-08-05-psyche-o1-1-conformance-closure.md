# Psyche O1.1 Conformance Closure Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close Psyche O1.1 by proving the already-merged corrective runtime behavior against every annex invariant, adding only missing regression coverage, and recording exact merge evidence without widening Psyche scope.

**Architecture:** Treat merged PR #622 (`f68be0a0af373caf81780b70a5d3bf7d680e0f6e`) as the implementation baseline and merged PR #633 (`c183d923e6d9e9d8172f39a193d624fe40095892`) as the approved normative annex. Add focused black-box tests for the few annex statements not yet directly exercised, reuse the existing process-supervision and OpenClaw suites for the rest, then update the annex and Psyche status documents with bounded candidate evidence. Do not change runtime behavior unless a new regression test demonstrates that `origin/main` violates the approved annex.

**Tech Stack:** Rust 2021, Clap, rusqlite, Unix shell fixtures, TypeScript, Vitest, Python 3 repository guards, Markdown, GitHub CLI, Beads CLI.

---

## File map

- `crates/coven-cli/tests/stream_json_integration.rs` - add hermetic CLI-level coverage for every continuation source status, unknown-source non-creation, `--detach`/`--continue` exclusion, and malformed native-frame ledger failure.
- `crates/coven-cli/src/pty_runner.rs` - add a focused unit test for syntactically valid non-object native JSON; change production code only if that test unexpectedly fails.
- `docs/superpowers/specs/2026-08-05-psyche-o1-1-corrective-annex-design.md` - record the implementation baseline and bounded conformance-candidate status.
- `specs/psyche/O1_CONTRACT_DESIGN.md` - identify O1.1 as the corrective delivery annex without changing the O1 contract boundary.
- `specs/psyche/PLAN.md` - record O1.1 candidate evidence while preserving all later contract and gate blockers.

No new runtime module, API route, persisted field, lifecycle status, migration, or Psyche dispatch surface is planned.

### Task 1: Prove the complete continuation matrix

**Files:**
- Modify: `crates/coven-cli/tests/stream_json_integration.rs:168-380`

- [ ] **Step 1: Add a reusable Unix fixture runner**

Add these imports below the existing `use std::process::{Command, Stdio};`:

```rust
#[cfg(unix)]
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
#[cfg(unix)]
use std::path::{Path, PathBuf};
```

Add this helper before `codex_json_stream_resumes_with_a_sibling_and_preserves_terminal_evidence`:

```rust
#[cfg(unix)]
fn install_fake_codex(temp_dir: &Path) -> (PathBuf, std::ffi::OsString) {
    let fake_bin = temp_dir.join("bin");
    fs::create_dir_all(&fake_bin).expect("failed to create fake bin dir");
    let fake_codex = fake_bin.join("codex");
    fs::write(
        &fake_codex,
        r#"#!/bin/sh
printf '%s\n' '{"type":"thread.started","thread_id":"thread-source"}'
printf '%s\n' '{"type":"turn.started"}'
printf '%s\n' '{"type":"item.completed","item":{"id":"item-1","type":"agent_message","text":"continuation reply"}}'
printf '%s\n' '{"type":"turn.completed"}'
"#,
    )
    .expect("failed to write fake codex");
    let mut permissions = fs::metadata(&fake_codex)
        .expect("failed to stat fake codex")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&fake_codex, permissions).expect("failed to chmod fake codex");

    let mut paths = vec![fake_bin];
    if let Some(existing) = std::env::var_os("PATH") {
        paths.extend(std::env::split_paths(&existing));
    }
    let path = std::env::join_paths(paths).expect("test PATH should be joinable");
    (fake_codex, path)
}
```

- [ ] **Step 2: Add the all-status continuation regression test**

Add this test after `codex_json_stream_resumes_with_a_sibling_and_preserves_terminal_evidence`:

```rust
#[cfg(unix)]
#[test]
fn continuation_preserves_every_valid_source_status() {
    let statuses = [
        ("created", None),
        ("running", None),
        ("idle", Some(0)),
        ("completed", Some(0)),
        ("failed", Some(7)),
        ("killed", Some(143)),
        ("orphaned", None),
    ];

    for (index, (status, exit_code)) in statuses.into_iter().enumerate() {
        let temp_dir = tempfile::tempdir().expect("failed to create temp dir");
        let coven_home = temp_dir.path().join("coven-home");
        fs::create_dir_all(&coven_home).expect("failed to create coven home");
        let project_root = temp_dir.path().join("project");
        fs::create_dir_all(&project_root).expect("failed to create project root");
        let (_, path) = install_fake_codex(temp_dir.path());

        let seed = Command::new(env!("CARGO_BIN_EXE_coven"))
            .args(["run", "codex", "--detach", "--", "seed"])
            .current_dir(&project_root)
            .env("COVEN_HOME", &coven_home)
            .env("PATH", &path)
            .output()
            .expect("failed to seed Coven store");
        assert!(seed.status.success(), "seed failed for {status}");

        let conn = rusqlite::Connection::open(coven_home.join("coven.sqlite3"))
            .expect("failed to open Coven session ledger");
        let source_id: String = conn
            .query_row("SELECT id FROM sessions LIMIT 1", [], |row| row.get(0))
            .expect("seed session missing");
        let archived_at = (index % 2 == 1).then_some("2026-08-05T12:00:00Z");
        conn.execute(
            "UPDATE sessions
             SET status = ?2,
                 exit_code = ?3,
                 archived_at = ?4,
                 conversation_id = 'thread-source',
                 created_at = '2026-08-05T10:00:00Z',
                 updated_at = '2026-08-05T11:00:00Z'
             WHERE id = ?1",
            rusqlite::params![source_id, status, exit_code, archived_at],
        )
        .expect("failed to prepare source evidence");
        let before: (String, Option<i32>, Option<String>, String, String, Option<String>) = conn
            .query_row(
                "SELECT status, exit_code, archived_at, created_at, updated_at, conversation_id
                 FROM sessions WHERE id = ?1",
                [&source_id],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                    ))
                },
            )
            .expect("source evidence missing");
        drop(conn);

        let resumed = Command::new(env!("CARGO_BIN_EXE_coven"))
            .args([
                "run",
                "codex",
                "--stream-json",
                "--continue",
                &source_id,
                "--",
                "follow-up",
            ])
            .current_dir(&project_root)
            .env("COVEN_HOME", &coven_home)
            .env("PATH", &path)
            .output()
            .expect("failed to continue source");
        assert!(
            resumed.status.success(),
            "continuation failed for {status}: {}",
            String::from_utf8_lossy(&resumed.stderr)
        );

        let frames: Vec<serde_json::Value> = String::from_utf8(resumed.stdout)
            .expect("stdout not utf-8")
            .lines()
            .map(|line| serde_json::from_str(line).expect("Coven stdout must remain JSONL"))
            .collect();
        let sibling_id = frames[0]["session_id"]
            .as_str()
            .expect("system frame carries sibling id");
        assert_ne!(sibling_id, source_id, "{status} source row was reused");

        let conn = rusqlite::Connection::open(coven_home.join("coven.sqlite3"))
            .expect("failed to reopen Coven session ledger");
        let after: (String, Option<i32>, Option<String>, String, String, Option<String>) = conn
            .query_row(
                "SELECT status, exit_code, archived_at, created_at, updated_at, conversation_id
                 FROM sessions WHERE id = ?1",
                [&source_id],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                    ))
                },
            )
            .expect("source evidence missing after continuation");
        assert_eq!(after, before, "{status} source evidence changed");

        let sibling: (String, Option<String>, String) = conn
            .query_row(
                "SELECT status, archived_at, conversation_id FROM sessions WHERE id = ?1",
                [sibling_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("sibling row missing");
        assert_eq!(sibling.0, "completed");
        assert_eq!(sibling.1, None, "new sibling must start unarchived");
        assert_eq!(sibling.2, "thread-source");
    }
}
```

- [ ] **Step 3: Run the matrix test and confirm the baseline**

Run:

```bash
cargo test -p coven-cli --test stream_json_integration continuation_preserves_every_valid_source_status -- --exact
```

Expected: PASS on the merged PR #622 baseline. If it fails, keep the test and make the smallest correction in `crates/coven-cli/src/main.rs` or `crates/coven-cli/src/store.rs` that restores fresh-sibling creation without changing schemas or lifecycle vocabulary.

- [ ] **Step 4: Add non-creation and argument-exclusion assertions**

Add this test after the all-status matrix:

```rust
#[cfg(unix)]
#[test]
fn invalid_continuation_requests_do_not_create_rows() {
    let temp_dir = tempfile::tempdir().expect("failed to create temp dir");
    let coven_home = temp_dir.path().join("coven-home");
    fs::create_dir_all(&coven_home).expect("failed to create coven home");
    let project_root = temp_dir.path().join("project");
    fs::create_dir_all(&project_root).expect("failed to create project root");
    let (_, path) = install_fake_codex(temp_dir.path());

    let missing = Command::new(env!("CARGO_BIN_EXE_coven"))
        .args([
            "run",
            "codex",
            "--stream-json",
            "--continue",
            "missing-source",
            "--",
            "follow-up",
        ])
        .current_dir(&project_root)
        .env("COVEN_HOME", &coven_home)
        .env("PATH", &path)
        .output()
        .expect("failed to run missing-source check");
    assert!(!missing.status.success());

    let conn = rusqlite::Connection::open(coven_home.join("coven.sqlite3"))
        .expect("failed to open Coven session ledger");
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM sessions", [], |row| row.get(0))
        .expect("failed to count sessions");
    assert_eq!(count, 0, "unknown continuation created a sibling");
    drop(conn);

    let conflicting = Command::new(env!("CARGO_BIN_EXE_coven"))
        .args([
            "run",
            "codex",
            "--detach",
            "--continue",
            "missing-source",
            "--",
            "follow-up",
        ])
        .current_dir(&project_root)
        .env("COVEN_HOME", &coven_home)
        .env("PATH", &path)
        .output()
        .expect("failed to run argument-conflict check");
    assert!(!conflicting.status.success());
    let stderr = String::from_utf8_lossy(&conflicting.stderr);
    assert!(
        stderr.contains("--detach") && stderr.contains("--continue"),
        "unexpected argument-conflict error: {stderr}"
    );

    let conn = rusqlite::Connection::open(coven_home.join("coven.sqlite3"))
        .expect("failed to reopen Coven session ledger");
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM sessions", [], |row| row.get(0))
        .expect("failed to recount sessions");
    assert_eq!(count, 0, "conflicting arguments created a sibling");
}
```

- [ ] **Step 5: Run the continuation regression group**

Run:

```bash
cargo test -p coven-cli --test stream_json_integration continuation_preserves_every_valid_source_status -- --exact
cargo test -p coven-cli --test stream_json_integration invalid_continuation_requests_do_not_create_rows -- --exact
cargo test -p coven-cli --test stream_json_integration continuation_selects_and_accepts_only_the_requested_harness -- --exact
cargo test -p coven-cli --test stream_json_integration codex_json_stream_resumes_with_a_sibling_and_preserves_terminal_evidence -- --exact
```

Expected: the all-status matrix, unknown-source, harness-selection, archived-source, and sibling-evidence tests pass.

- [ ] **Step 6: Commit the continuation proof**

```bash
git add crates/coven-cli/tests/stream_json_integration.rs
git commit -s -m "test(run): prove O1.1 continuation invariants" \
  -m "Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

### Task 2: Prove fail-closed native stream persistence

**Files:**
- Modify: `crates/coven-cli/src/pty_runner.rs:4260-4308`
- Modify: `crates/coven-cli/tests/stream_json_integration.rs:647-700`

- [ ] **Step 1: Add a unit test for syntactically valid non-object JSON**

Add this test immediately after `native_stream_malformed_json_terminates_and_reaps_harness`:

```rust
#[test]
fn native_stream_rejects_non_object_json() {
    let mut output = Vec::new();
    let error = normalize_native_stream_line("[]", "claude", "ledger-current", &mut output)
        .expect_err("a native array must not be accepted as a Coven frame");

    assert!(
        format!("{error:#}").contains("invalid JSON object from claude native stream"),
        "unexpected non-object error: {error:#}"
    );
    assert!(output.is_empty(), "invalid native data must not be forwarded");
}
```

- [ ] **Step 2: Run the unit test**

Run:

```bash
cargo test -p coven-cli pty_runner::tests::native_stream_rejects_non_object_json -- --exact
```

Expected: PASS because merged production code already calls `Value::as_object_mut()` and fails closed. If it fails, change only `normalize_native_stream_line` so non-object values return the existing `invalid JSON object from {harness_id} native stream` error.

- [ ] **Step 3: Add a CLI-level ledger failure test**

Add this test after `command_construction_failure_marks_created_row_failed`:

```rust
#[cfg(unix)]
#[test]
fn malformed_native_stream_marks_current_ledger_row_failed() {
    let temp_dir = tempfile::tempdir().expect("failed to create temp dir");
    let coven_home = temp_dir.path().join("coven-home");
    fs::create_dir_all(&coven_home).expect("failed to create coven home");
    let project_root = temp_dir.path().join("project");
    fs::create_dir_all(&project_root).expect("failed to create project root");
    let fake_bin = temp_dir.path().join("bin");
    fs::create_dir_all(&fake_bin).expect("failed to create fake bin dir");
    let fake_claude = fake_bin.join("claude");
    fs::write(&fake_claude, "#!/bin/sh\nprintf '%s\\n' '[]'\n")
        .expect("failed to write fake claude");
    let mut permissions = fs::metadata(&fake_claude)
        .expect("failed to stat fake claude")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&fake_claude, permissions).expect("failed to chmod fake claude");

    let mut paths = vec![fake_bin];
    if let Some(existing) = std::env::var_os("PATH") {
        paths.extend(std::env::split_paths(&existing));
    }
    let path = std::env::join_paths(paths).expect("test PATH should be joinable");

    let out = Command::new(env!("CARGO_BIN_EXE_coven"))
        .args(["run", "claude", "--stream-json", "--", "malformed"])
        .current_dir(&project_root)
        .env("COVEN_HOME", &coven_home)
        .env("PATH", &path)
        .output()
        .expect("failed to run malformed native fixture");
    assert!(!out.status.success(), "non-object native JSON must fail");

    let frames: Vec<serde_json::Value> = String::from_utf8(out.stdout)
        .expect("stdout not utf-8")
        .lines()
        .map(|line| serde_json::from_str(line).expect("Coven stdout must remain JSONL"))
        .collect();
    let session_id = frames[0]["session_id"]
        .as_str()
        .expect("system frame carries current ledger id");
    let result = frames.last().expect("terminal result frame missing");
    assert_eq!(result["session_id"], session_id);
    assert_eq!(result["is_error"], true);
    assert!(
        result["error"]
            .as_str()
            .is_some_and(|error| error.contains("invalid JSON object")),
        "unexpected protocol error: {result}"
    );

    let conn = rusqlite::Connection::open(coven_home.join("coven.sqlite3"))
        .expect("failed to open Coven session ledger");
    let (status, exit_code): (String, Option<i32>) = conn
        .query_row(
            "SELECT status, exit_code FROM sessions WHERE id = ?1",
            [session_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("failed row missing from ledger");
    assert_eq!(status, "failed");
    assert_ne!(exit_code, Some(0));
}
```

- [ ] **Step 4: Run all native-stream failure tests**

Run:

```bash
cargo test -p coven-cli pty_runner::tests::native_stream_ -- --nocapture
cargo test -p coven-cli --test stream_json_integration malformed_native_stream_marks_current_ledger_row_failed -- --exact
cargo test -p coven-cli --test stream_json_integration command_construction_failure_marks_created_row_failed -- --exact
```

Expected: invalid JSON, non-object JSON, cancellation, inherited-pipe, cleanup, and ledger-failure tests pass; no test leaves a harness process alive.

- [ ] **Step 5: Commit the fail-closed proof**

```bash
git add crates/coven-cli/src/pty_runner.rs crates/coven-cli/tests/stream_json_integration.rs
git commit -s -m "test(run): prove O1.1 native stream failures" \
  -m "Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

### Task 3: Re-run the merged supervision and adapter contracts

**Files:**
- Verify: `crates/coven-cli/src/pty_runner.rs`
- Verify: `crates/coven-cli/tests/stream_json_integration.rs`
- Verify: `packages/openclaw-coven/src/runtime.ts`
- Verify: `packages/openclaw-coven/src/runtime.test.ts`

- [ ] **Step 1: Run the PID, cancellation, and drain tests**

Run:

```bash
cargo test -p coven-cli pty_runner::tests::cancellation_recorded_before_spawn_is_returned_by_checks_and_finish -- --exact
cargo test -p coven-cli pty_runner::tests::cancellation_handler_has_no_process_group_side_effect_by_construction -- --exact
cargo test -p coven-cli pty_runner::tests::wait_without_reaping_keeps_child_pid_reserved -- --exact
cargo test -p coven-cli pty_runner::tests::nonblocking_wait_without_reaping_does_not_report_live_child_exited -- --exact
cargo test -p coven-cli pty_runner::tests::cancellation_guard_blocks_supervised_signals_for_spawned_helpers -- --exact
cargo test -p coven-cli pty_runner::tests::native_stream_does_not_wait_for_stdout_inheriting_descendant -- --exact
cargo test -p coven-cli pty_runner::tests::successful_native_stream_cleans_closed_output_descendant -- --exact
cargo test -p coven-cli pty_runner::tests::native_stream_sigterm_returns_promptly_and_reaps_process_tree -- --exact
cargo test -p coven-cli pty_runner::tests::codex_json_runner_reaps_a_pipe_holding_descendant_after_wrapper_exit -- --exact
cargo test -p coven-cli pty_runner::tests::codex_json_runner_reaps_a_closed_pipe_descendant_after_wrapper_exit -- --exact
cargo test -p coven-cli --test stream_json_integration codex_json_sigterm_reaps_descendants_and_marks_ledger_failed -- --exact
```

Expected on Unix: every test passes, direct children and descendants are gone, cancellation is prompt, `waitid(...WNOHANG | WNOWAIT)` never reports a live child as exited, and cleanup occurs before reaping.

- [ ] **Step 2: Run the Windows-compilable containment coverage**

On the repository's Windows CI runner, run:

```powershell
cargo test -p coven-cli --test stream_json_integration windows_codex_cmd_stream_json_emits_assistant_and_resumes_native_thread -- --exact
cargo test -p coven-cli --test stream_json_integration windows_silent_codex_cmd_emits_terminal_error_and_marks_session_failed -- --exact
cargo test -p coven-cli pty_runner::tests::codex_json_batch_shim_uses_stdin_and_emits_assistant_text -- --exact
cargo test -p coven-cli pty_runner::tests::codex_json_batch_shim_times_out_while_large_prompt_is_still_writing -- --exact
```

Expected: every test passes under the Windows Job Object path. Do not emulate Windows process ownership with a Unix-only test.

- [ ] **Step 3: Run the OpenClaw terminal mapping**

Run:

```bash
npm --prefix packages/openclaw-coven run typecheck
npm --prefix packages/openclaw-coven test -- src/runtime.test.ts
```

Expected: PASS, including `killed -> cancelled`, `orphaned -> error`, persisted-ledger precedence over late exit events, and no conversion of unresolved evidence into completion.

- [ ] **Step 4: Stop on behavioral drift**

If any existing test fails, do not weaken its assertion or broaden timing thresholds. Diagnose the current `origin/main` regression, add a focused failing test adjacent to the affected suite, implement the smallest correction, and commit it separately:

```bash
git add crates/coven-cli/src/pty_runner.rs \
  crates/coven-cli/tests/stream_json_integration.rs \
  packages/openclaw-coven/src/runtime.ts \
  packages/openclaw-coven/src/runtime.test.ts
git commit -s -m "fix(run): restore O1.1 supervision contract" \
  -m "Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

If all tests pass, make no commit for this task.

### Task 4: Record bounded O1.1 candidate evidence

**Files:**
- Modify: `docs/superpowers/specs/2026-08-05-psyche-o1-1-corrective-annex-design.md:3-12`
- Modify: `specs/psyche/O1_CONTRACT_DESIGN.md:1-12`
- Modify: `specs/psyche/PLAN.md:8-16`

- [ ] **Step 1: Update the annex status and implementation baseline**

Replace the annex status with:

```markdown
**Status:** Approved and implemented; conformance candidate assembled, with
final repository gates and merge evidence pending
```

Add this paragraph after the Scope block:

```markdown
**Implementation baseline:** Corrective runtime behavior merged in PR #622 at
`f68be0a0af373caf81780b70a5d3bf7d680e0f6e`. This annex merged in PR #633 at
`c183d923e6d9e9d8172f39a193d624fe40095892`. The conformance closure adds only
missing regression coverage and completion evidence unless a test proves a
behavioral defect.
```

- [ ] **Step 2: Link O1 to its corrective annex**

After the issue line in `specs/psyche/O1_CONTRACT_DESIGN.md`, add:

```markdown
**Corrective delivery annex:** [`docs/superpowers/specs/2026-08-05-psyche-o1-1-corrective-annex-design.md`](../../docs/superpowers/specs/2026-08-05-psyche-o1-1-corrective-annex-design.md)
```

Do not change O1's named contract, lifecycle vocabulary, or C-S coverage claims.

- [ ] **Step 3: Replace the stale O1 candidate paragraph in the Psyche plan**

Use:

```markdown
**O1/O1.1 delivery candidate:** O1 named-contract and lifecycle vocabulary
merged in PR #574. Corrective continuation, stream-identity, and
process-supervision behavior merged in PR #622, and its approved O1.1 annex
merged in PR #633. The conformance closure adds direct regression coverage for
the remaining annex assertions and reruns the frozen repository gates. O1
remains incomplete until issue #567 and Bead `coven-psy-o1` record the observed
conformance merge SHA and verification evidence. This closes only C-S1
vocabulary and C-S8 documentation; C-S3-C-S6 and C-S9-C-S12 remain planned,
and G4/G6 plus production child dispatch remain blocked.
```

- [ ] **Step 4: Run documentation and scope guards**

Run:

```bash
git add docs/superpowers/specs/2026-08-05-psyche-o1-1-corrective-annex-design.md \
  specs/psyche/O1_CONTRACT_DESIGN.md \
  specs/psyche/PLAN.md
python3 scripts/check-api-contract-docs-test.py
python3 scripts/check-api-contract-docs.py
python3 scripts/check-coven-privacy.py --staged
git diff --cached --check
```

Expected: all guards pass. The diff contains no O2-O8 field, route, migration, authorization, adoption, cancellation-acknowledgement, or production-dispatch implementation.

- [ ] **Step 5: Commit the candidate evidence**

```bash
git add docs/superpowers/specs/2026-08-05-psyche-o1-1-corrective-annex-design.md \
  specs/psyche/O1_CONTRACT_DESIGN.md \
  specs/psyche/PLAN.md
git commit -s -m "docs(psyche): record O1.1 conformance candidate" \
  -m "Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

### Task 5: Run release gates and close delivery evidence

**Files:**
- Verify: entire repository
- External record after merge: GitHub issue `OpenCoven/coven#567`
- External record after merge: Bead `coven-psy-o1`

- [ ] **Step 1: Run the complete annex gate set**

Run:

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --locked
npm --prefix packages/openclaw-coven run typecheck
npm --prefix packages/openclaw-coven test
python3 scripts/check-api-contract-docs-test.py
python3 scripts/check-api-contract-docs.py
python3 scripts/check-secrets-test.py
python3 scripts/check-secrets.py
python3 scripts/check-coven-privacy.py --range origin/main...HEAD
git diff --check origin/main...HEAD
git status --short
```

Expected: every command exits 0. The only implementation/evidence changes are
the two focused Rust test files and three Psyche documents, unless Task 3
exposed and corrected a real regression; this implementation-plan document is
also present as planning evidence.

- [ ] **Step 2: Confirm scope against the annex**

Review `git diff --stat origin/main...HEAD` and `git diff origin/main...HEAD`. Confirm:

1. every valid source status continues through a fresh sibling;
2. unknown, conflicting, and cross-harness continuation requests create no row;
3. source lifecycle and archive evidence remain immutable;
4. top-level Coven and harness-native identities remain distinct;
5. malformed and non-object native data fail the sibling ledger row;
6. process ownership, cancellation, bounded drain, PID safety, and Windows containment remain covered;
7. OpenClaw maps `killed` and `orphaned` without claiming C-S9 acknowledgement; and
8. C-S3-C-S6, C-S9-C-S12, G4, G6, and production child dispatch remain explicitly incomplete.

- [ ] **Step 3: Push and open the reviewed conformance PR**

Run:

```bash
git push -u origin docs/psyche-o1-1-conformance-closure
gh pr create \
  --repo OpenCoven/coven \
  --base main \
  --head docs/psyche-o1-1-conformance-closure \
  --title "test(psyche): close O1.1 conformance evidence" \
  --body-file docs/superpowers/plans/2026-08-05-psyche-o1-1-conformance-closure.md
```

Expected: GitHub returns the new PR URL. Do not merge until hosted Unix and Windows checks pass and review confirms the annex boundary.

- [ ] **Step 4: Record the observed merge SHA**

After the reviewed PR merges, run:

```bash
MERGE_SHA="$(gh pr view \
  docs/psyche-o1-1-conformance-closure \
  --repo OpenCoven/coven \
  --json mergeCommit \
  --jq '.mergeCommit.oid // empty')"
test -n "$MERGE_SHA"
printf '%s\n' "$MERGE_SHA"
```

Expected: the exact GitHub merge commit SHA, not a branch head or expected value.

- [ ] **Step 5: Update both completion trackers**

Run:

```bash
MERGE_SHA="$(gh pr view \
  docs/psyche-o1-1-conformance-closure \
  --repo OpenCoven/coven \
  --json mergeCommit \
  --jq '.mergeCommit.oid // empty')"
test -n "$MERGE_SHA"
EVIDENCE="O1.1 conformance merged at ${MERGE_SHA}. Corrective runtime baseline: f68be0a0af373caf81780b70a5d3bf7d680e0f6e (PR #622). Approved annex baseline: c183d923e6d9e9d8172f39a193d624fe40095892 (PR #633). Continuation matrix, stream identity, malformed native frames, command-build failure persistence, cancellation races, descendant cleanup, PID safety, Windows containment, OpenClaw terminal mapping, full Rust/OpenClaw/docs/secret/privacy gates, and hosted checks passed. Scope closed remains C-S1 vocabulary and C-S8 documentation only; C-S3-C-S6, C-S9-C-S12, G4, G6, and production child dispatch remain blocked."
gh issue comment 567 --repo OpenCoven/coven --body "$EVIDENCE"
bd comments add coven-psy-o1 "$EVIDENCE"
```

Expected: issue #567 and Bead `coven-psy-o1` both display the same observed merge SHA and bounded scope statement.

- [ ] **Step 6: Close only after both records are readable**

Run:

```bash
MERGE_SHA="$(gh pr view \
  docs/psyche-o1-1-conformance-closure \
  --repo OpenCoven/coven \
  --json mergeCommit \
  --jq '.mergeCommit.oid // empty')"
test -n "$MERGE_SHA"
gh issue view 567 --repo OpenCoven/coven --comments
bd show coven-psy-o1
gh issue close 567 --repo OpenCoven/coven \
  --comment "O1/O1.1 delivery evidence is recorded above; later Psyche contracts and G4/G6 remain open."
bd close coven-psy-o1 \
  --reason "O1/O1.1 merge and verification evidence recorded at ${MERGE_SHA}; later Psyche contracts and gates remain blocked."
```

Expected: both records contain the evidence before either is closed. Do not close a later Psyche work item, G4, G6, or any production child-dispatch gate.
