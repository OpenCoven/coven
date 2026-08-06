# Session Handoff Cursor Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make live-session handoffs scalable and claimable after append-only source transcript growth.

**Architecture:** The SQLite store owns the current event cursor through a scalar `MAX(rowid)` query. Retention pins each unresolved handoff's source events through its offered cursor. The store reads and validates that cursor inside the existing `IMMEDIATE` transaction for claim and acknowledgement, so a successful state transition cannot race a pruning transaction.

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

### Task 2: Pin and transactionally validate handoff transcript prefixes

**Files:**
- Modify: `crates/coven-cli/src/api.rs:2038-2049, 2104-2145, 2168-2210, 9250-9390`
- Modify: `crates/coven-cli/src/store.rs:3040-3145, 3441-3472, 6246-6328`

- [ ] **Step 1: Write failing retention and transactional-guard tests**

Keep the append-only API assertion added by the prior task: input appended
after an offer must permit claim and acknowledgement. Add store tests with
real event rows, not synthetic cursor values:

```rust
let cursor = latest_event_seq(&conn, "session-1")?;
let offered = create_handoff(&mut conn, "handoff-1", "session-1", "{}", cursor, "{}", now)?;
assert_eq!(prune_events_older_than(&conn, cutoff)?, 0);
claim_handoff(&mut conn, &offered.id, offered.generation, "device:phone-1", "claim-1", now)?;
assert_eq!(prune_events_older_than_bounded(&conn, cutoff, 10)?, 0);
let acknowledged = acknowledge_handoff(&mut conn, &offered.id, "device:phone-1", now)?;
assert_eq!(acknowledged.state, "acknowledged");
assert_eq!(prune_events_older_than(&conn, cutoff)?, 1);
```

Create a second offered handoff with an `event_cursor` greater than the actual
latest sequence. Assert `claim_handoff` returns `transcript_diverged` without
changing state. Claim a valid handoff, delete its cursor event directly in the
test transaction, and assert `acknowledge_handoff` returns
`transcript_diverged`. These tests prove the store—not an API preflight—is the
transition authority.

- [ ] **Step 2: Run the new tests and verify current behavior fails**

Run the exact new store test names and:

```bash
cargo test -p coven-cli handoff_claim_fails_closed_when_transcript_or_workspace_diverges --locked
cargo test -p coven-cli unresolved_handoff_events_are_not_pruned --locked
cargo test -p coven-cli handoff_transition_rejects_missing_cursor --locked
```

Expected: the append-only API test already passes; the new retention and
transactional-guard tests fail because both pruning paths ignore handoffs and
the store accepts a stale caller-derived cursor.

- [ ] **Step 3: Make retention respect unresolved handoff pins**

In both `prune_events_older_than` and `prune_events_older_than_bounded`, only
delete an expired event when no `offered` or `claimed` handoff for its session
pins that row:

```sql
AND NOT EXISTS (
    SELECT 1
    FROM session_handoffs AS handoff
    WHERE handoff.session_id = event.session_id
      AND handoff.state IN ('offered', 'claimed')
      AND event.rowid <= handoff.event_cursor
)
```

Use an `events AS event` alias inside each pruning subquery. Keep the current
cutoff ordering, bounded batch limit, FTS trigger behavior, and transaction
shape intact.

- [ ] **Step 4: Move cursor validation into handoff transactions**

Remove the API-side `latest_event_seq` preflight checks from claim and
acknowledgement; retain the helper in `emit_handoff`. In `claim_handoff`, after
loading the record in its `IMMEDIATE` transaction, read the current sequence
from the same transaction and reject `actual_cursor < current.event_cursor`
with `bail!("transcript_diverged")`.

```rust
let actual_cursor = latest_event_seq(&transaction, &current.session_id)?;
if actual_cursor < current.event_cursor {
    bail!("transcript_diverged");
}
```

Change `acknowledge_handoff` to derive and validate that same transaction-local
cursor itself; remove its caller-provided `event_cursor` parameter. Update its
API caller and direct store tests. Do not weaken workspace compatibility,
generation, claimant, state, or input-lease checks.

- [ ] **Step 5: Run focused regression coverage**

Run:

```bash
cargo test -p coven-cli handoff_claim_fails_closed_when_transcript_or_workspace_diverges --locked
cargo test -p coven-cli handoff_claim_acknowledgement_import_fences_source_and_is_idempotent --locked
cargo test -p coven-cli unresolved_handoff_events_are_not_pruned --locked
cargo test -p coven-cli handoff_transition_rejects_missing_cursor --locked
```

Expected: PASS; a later source event permits claim and acknowledgement,
retention cannot remove an unresolved handoff prefix, acknowledged handoffs
release their pin, and missing cursors still fail closed.

- [ ] **Step 6: Commit pinned handoff semantics**

```bash
git add crates/coven-cli/src/api.rs crates/coven-cli/src/store.rs
git commit -s -m "fix: pin handoff transcript prefixes"
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
