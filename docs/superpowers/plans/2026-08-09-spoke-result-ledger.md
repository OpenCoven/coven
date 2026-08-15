# Spoke Result Ledger Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make hub-captured spoke result envelopes append-only and idempotent to replay without changing the existing SSH dispatcher contract.

**Architecture:** Add an immutable SQLite result ledger keyed by a deterministic digest of node identity plus canonical envelope JSON. Keep the existing per-job dispatch row as a latest-state projection, and persist the ledger append plus all result-derived hub state in one transaction.

**Tech Stack:** Rust 2021, rusqlite, serde/serde_json, sha2, Coven daemon API.

---

### Task 1: Add the immutable result ledger

**Files:**
- Modify: `crates/coven-cli/src/store.rs:274-283`
- Modify: `crates/coven-cli/src/store.rs:716-727`
- Modify: `crates/coven-cli/src/store.rs:1647-1702`
- Test: `crates/coven-cli/src/store.rs:4531-4564`

- [ ] **Step 1: Write failing store tests**

Add tests that append one `ExecutorResultEnvelopeRecord` twice and assert the
second append returns `false`, then append a different envelope for the same job
and assert both rows are returned in insertion order. Reopen the database before
the final assertions to prove durability.

```rust
assert!(append_executor_result_envelope(&conn, &first)?);
assert!(!append_executor_result_envelope(&conn, &first)?);
assert!(append_executor_result_envelope(&conn, &second)?);
drop(conn);
let reopened = open_store(&path)?;
assert_eq!(list_executor_result_envelopes(&reopened, "job-1")?, vec![first, second]);
```

- [ ] **Step 2: Run the focused test and verify it fails**

Run:

```bash
cargo test -p coven-cli store::tests::executor_result_envelopes_are_append_only_and_replay_safe
```

Expected: compilation fails because the record and functions do not exist.

- [ ] **Step 3: Add schema, record type, and store functions**

Create `ExecutorResultEnvelopeRecord`, the append-only
`executor_result_envelopes` table, and an index on `(job_id, sequence)`.
Implement insertion with `INSERT OR IGNORE` and return `affected == 1`.
Implement ordered retrieval by job ID. Backfill existing non-null latest
envelopes with deterministic `legacy:<job_id>` IDs:

```sql
INSERT OR IGNORE INTO executor_result_envelopes (
    envelope_id, job_id, node_id, envelope_json, recorded_at
)
SELECT 'legacy:' || job_id, job_id, node_id, envelope_json, updated_at
FROM executor_dispatches
WHERE envelope_json IS NOT NULL;
```

- [ ] **Step 4: Run focused store tests**

Run:

```bash
cargo test -p coven-cli store::tests::executor_result_envelopes
```

Expected: all matching tests pass.

### Task 2: Persist result-derived hub state atomically

**Files:**
- Modify: `crates/coven-cli/src/hub.rs:768-920`
- Test: `crates/coven-cli/src/hub.rs:1953-2072`

- [ ] **Step 1: Write a failing hub replay test**

Extract result persistence behind a helper and call it twice with the same
envelope. Assert one ledger row exists, the latest dispatch projection remains
terminal, and a matching hub job remains in the same terminal state.

```rust
persist_dispatch_result(&mut conn, &node, &job, &envelope, &created_at, &finished_at)?;
persist_dispatch_result(&mut conn, &node, &job, &envelope, &created_at, &finished_at)?;
assert_eq!(store::list_executor_result_envelopes(&conn, &job.job_id)?.len(), 1);
assert_eq!(store::get_hub_job(&conn, &job.job_id)?.unwrap().state, "completed");
```

- [ ] **Step 2: Run the focused hub test and verify it fails**

Run:

```bash
cargo test -p coven-cli hub::tests::replaying_result_envelope_is_idempotent
```

Expected: compilation fails because `persist_dispatch_result` does not exist.

- [ ] **Step 3: Implement deterministic envelope IDs**

Serialize the typed envelope once and hash the node ID, a zero separator, and
the JSON bytes with SHA-256. Encode the digest as lowercase hex:

```rust
let mut digest = Sha256::new();
digest.update(node_id.as_bytes());
digest.update([0]);
digest.update(envelope_json.as_bytes());
let envelope_id = format!("sha256:{}", hex_digest(digest.finalize()));
```

- [ ] **Step 4: Implement transactional persistence**

Move post-dispatch writes into `persist_dispatch_result`. Start one rusqlite
transaction, append the immutable envelope, update `executor_dispatches`, apply
the terminal hub-job state, update node availability/error, transition held
jobs, and synchronize the executor subqueue before committing. Duplicate
envelopes return before projection updates, so an older replay cannot overwrite
newer hub state.

- [ ] **Step 5: Run focused hub and executor tests**

Run:

```bash
cargo test -p coven-cli hub::tests::replaying_result_envelope_is_idempotent
cargo test -p coven-cli --test executor_protocol
```

Expected: both commands pass.

### Task 3: Expose immutable result history

**Files:**
- Modify: `crates/coven-cli/src/hub.rs:922-951`
- Modify: `specs/coven-multi-host-daemon/TECH.md:302-329`
- Test: `crates/coven-cli/src/hub.rs:2023-2072`

- [ ] **Step 1: Extend the hub dispatch test**

After a successful fake-executor dispatch, assert the response from
`GET /api/v1/hub/dispatches/:jobId` includes one ledger entry with a stable
`envelopeId` and the same envelope as the compatibility projection.

```rust
assert_eq!(job["resultEnvelopes"].as_array().unwrap().len(), 1);
assert_eq!(job["resultEnvelopes"][0]["envelope"], job["envelope"]);
assert!(job["resultEnvelopes"][0]["envelopeId"]
    .as_str()
    .unwrap()
    .starts_with("sha256:"));
```

- [ ] **Step 2: Run the test and verify it fails**

Run:

```bash
cargo test -p coven-cli hub::tests::hub_polls_and_dispatches_outbound_and_records_executor_state
```

Expected: assertion fails because `resultEnvelopes` is absent.

- [ ] **Step 3: Add result history to the response**

Load the ordered ledger rows, parse each envelope JSON, and serialize them as:

```json
{
  "envelopeId": "sha256:...",
  "jobId": "job_...",
  "nodeId": "node_...",
  "envelope": {},
  "recordedAt": "..."
}
```

Update the multi-host technical spec to identify this collection as the
append-only, replay-safe authority while `envelope` remains the latest
projection.

- [ ] **Step 4: Run targeted protocol tests**

Run:

```bash
cargo test -p coven-cli hub::tests
cargo test -p coven-cli store::tests::executor_result_envelopes
cargo test -p coven-cli --test executor_protocol
```

Expected: all tests pass.

### Task 4: Validate repository gates

**Files:**
- Verify all modified files.

- [ ] **Step 1: Format and lint**

Run:

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: both commands exit successfully with no warnings.

- [ ] **Step 2: Run the workspace tests**

Run:

```bash
cargo test --workspace --locked
```

Expected: all workspace tests pass.

- [ ] **Step 3: Run safety checks**

Run:

```bash
python scripts/check-secrets.py
git add crates/coven-cli/src/store.rs crates/coven-cli/src/hub.rs specs/coven-multi-host-daemon/TECH.md docs/superpowers/specs/2026-08-09-spoke-result-ledger-design.md docs/superpowers/plans/2026-08-09-spoke-result-ledger.md
python3 scripts/check-coven-privacy.py --staged
```

Expected: both scripts report no violations.

- [ ] **Step 4: Commit the completed change**

```bash
git commit -s -m "fix: make spoke result envelopes append-only" \
  -m "Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```
