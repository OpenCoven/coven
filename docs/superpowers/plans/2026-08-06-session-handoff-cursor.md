# Session Handoff Cursor Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make live-session handoffs scalable and claimable after append-only source transcript growth.

**Architecture:** The SQLite store owns the current event cursor through a scalar `MAX(rowid)` query. API claim and acknowledgement treat the offered cursor as a transcript prefix: later events are safe, while a cursor lower than the offer remains a divergence. The transactional store acknowledgement repeats that lower-bound guard.

**Tech Stack:** Rust, rusqlite, existing `coven-cli` API and store unit tests.

---

### Task 1: Add scalar event-cursor storage

**Files:**
- Modify: `crates/coven-cli/src/store.rs:2959-2977, 6246-6267`

- [ ] **Step 1: Write the failing cursor test**

Add this test next to `events_have_monotonic_seq_fields`:

```rust
#[test]
fn latest_event_seq_returns_zero_for_empty_and_last_rowid_for_session() -> Result<()> {
    let temp_dir = tempfile::tempdir()?;
    let conn = open_store(&temp_dir.path().join("coven.db"))?;
    insert_session(&conn, &session_record("session-1", "2026-04-27T06:00:00Z"))?;
    assert_eq!(latest_event_seq(&conn, "session-1")?, 0);

    for i in 1..=3 {
        insert_json_event(
            &conn,
            "session-1",
            "output",
            &serde_json::json!({ "data": format!("line {i}") }),
            "2026-04-27T06:01:00Z",
        )?;
    }
    assert_eq!(latest_event_seq(&conn, "session-1")?, list_events(&conn, "session-1")?[2].seq);
    Ok(())
}
```

- [ ] **Step 2: Run the new test and verify it fails**

Run: `cargo test -p coven-cli latest_event_seq_returns_zero_for_empty_and_last_rowid_for_session --locked`

Expected: FAIL because `latest_event_seq` does not exist.

- [ ] **Step 3: Implement the scalar helper**

Add beside `list_events`:

```rust
pub fn latest_event_seq(conn: &Connection, session_id: &str) -> Result<i64> {
    conn.query_row(
        "SELECT COALESCE(MAX(rowid), 0) FROM events WHERE session_id = ?1",
        [session_id],
        |row| row.get(0),
    )
    .context("failed to read latest event sequence")
}
```

Do not add a cache or load `payload_json`; `list_events_with_options` remains
the API for callers that actually need event records.

- [ ] **Step 4: Run the focused store test**

Run: `cargo test -p coven-cli latest_event_seq_returns_zero_for_empty_and_last_rowid_for_session --locked`

Expected: PASS.

- [ ] **Step 5: Commit the store helper**

```bash
git add crates/coven-cli/src/store.rs
git commit -s -m "fix: query handoff event cursors directly"
```

### Task 2: Permit append-only handoff transcript growth

**Files:**
- Modify: `crates/coven-cli/src/api.rs:2038-2049, 2119-2135, 2183-2199, 9332-9358`
- Modify: `crates/coven-cli/src/store.rs:3108-3125`

- [ ] **Step 1: Replace the failing API expectation**

In `handoff_claim_fails_closed_when_transcript_or_workspace_diverges`, replace
the `transcript_conflict` assertion after the accepted input with a successful
claim and acknowledgement:

```rust
assert_eq!(claimed.status, 200);
let acknowledged = handle_request_with_body(
    "POST",
    &format!("/sessions/session-1/handoffs/{handoff_id}/ack"),
    temp.path(),
    None,
    Some(r#"{"claimant":"device:phone-1"}"#),
)?;
assert_eq!(acknowledged.status, 200);
```

Keep the later wrong-workspace assertion, but emit a new handoff only after
the first handoff is acknowledged.

- [ ] **Step 2: Add a lower-cursor store guard test**

Add a store test that creates a handoff with `event_cursor: 3`, claims it, then
calls `acknowledge_handoff(..., 2, ...)` and asserts the error string is
`transcript_diverged`. Call it again with `4` and assert the record reaches
`acknowledged`.

- [ ] **Step 3: Run the altered tests and verify current behavior fails**

Run:

```bash
cargo test -p coven-cli handoff_claim_fails_closed_when_transcript_or_workspace_diverges --locked
cargo test -p coven-cli acknowledge_handoff --locked
```

Expected: the API test fails with HTTP 409 after the appended input and the
store test fails with `transcript_diverged` for cursor `4`.

- [ ] **Step 4: Implement prefix semantics and direct cursor reads**

Replace all three `store::list_events(&conn, session_id)?.last()` expressions
in `emit_handoff`, `claim_session_handoff`, and
`acknowledge_session_handoff` with:

```rust
let event_cursor = store::latest_event_seq(&conn, session_id)?;
```

In both API checks, reject only a truncated cursor:

```rust
if current_cursor < handoff.event_cursor {
    return api_error(
        409,
        "transcript_diverged",
        "Source transcript no longer contains the handoff snapshot.",
        Some(json!({
            "handoffId": handoff_id,
            "expectedCursor": handoff.event_cursor,
            "actualCursor": current_cursor,
        })),
    );
}
```

In `store::acknowledge_handoff`, replace the equality guard with:

```rust
if event_cursor < current.event_cursor {
    bail!("transcript_diverged");
}
```

Do not weaken workspace compatibility, generation, claimant, state, or input
lease checks.

- [ ] **Step 5: Run focused regression coverage**

Run:

```bash
cargo test -p coven-cli handoff_claim_fails_closed_when_transcript_or_workspace_diverges --locked
cargo test -p coven-cli handoff_claim_acknowledgement_import_fences_source_and_is_idempotent --locked
cargo test -p coven-cli acknowledge_handoff --locked
```

Expected: PASS; a later source event permits claim and acknowledgement, while
a lower cursor and the existing workspace conflict still fail closed.

- [ ] **Step 6: Commit handoff semantics**

```bash
git add crates/coven-cli/src/api.rs crates/coven-cli/src/store.rs
git commit -s -m "fix: preserve append-only handoff cursors"
```

### Task 3: Run the issue acceptance gates

**Files:**
- Modify: none

- [ ] **Step 1: Format and lint the changed crate**

Run:

```bash
cargo fmt --check
cargo clippy -p coven-cli --all-targets -- -D warnings
```

Expected: both commands exit 0.

- [ ] **Step 2: Run the complete affected test suite**

Run: `cargo test -p coven-cli --locked`

Expected: PASS.

- [ ] **Step 3: Run repository hygiene gates**

Run:

```bash
python scripts/check-secrets.py
git add crates/coven-cli/src/api.rs crates/coven-cli/src/store.rs
python3 scripts/check-coven-privacy.py --staged
git diff --cached --check
```

Expected: every command exits 0.

- [ ] **Step 4: Push and open the issue-linked pull request**

```bash
git push -u origin fix/613-handoff-cursors
gh pr create --base main --title "fix: preserve append-only handoff cursors" \
  --body "Fixes #613."
```

Expected: the PR references #613 and contains the two implementation commits.
