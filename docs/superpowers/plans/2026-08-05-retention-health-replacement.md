# Retention Health Replacement Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Port PR #608's bounded SQLite retention and storage-health work onto current `main` without regressing live event-writer or session-handoff health.

**Architecture:** Preserve current `main` as the authority and import PR #608 file-by-file from its exact head. Add accepted-but-uncommitted event counts to the existing writer snapshot, feed that snapshot into the imported storage-health collector, and expose `eventWriter` and `storage` side by side.

**Tech Stack:** Rust, rusqlite/SQLite, fs2, serde, Cargo, Git three-way patches, GitHub CLI.

---

## File Structure

- Modify `crates/coven-cli/src/event_writer.rs`: maintain exact queued event and byte counts through commit completion.
- Modify `crates/coven-cli/src/store.rs`: port bounded retention, storage health, indexes, deterministic seams, and tests from PR #608.
- Modify `crates/coven-cli/src/daemon.rs`: port recovery-log rotation and the scheduled maintenance thread.
- Modify `crates/coven-cli/src/api.rs`: preserve current health fields and add storage health populated from the writer snapshot.
- Modify `docs/daemon/health.md`: document both event-writer and storage health without stale future-work claims.
- Modify `docs/reference/api.md`: add the storage response while retaining current capability and event-writer documentation.
- Modify `docs/reference/cli-logs.md`: document scheduled retention and explicit compaction.

## Fixed Port Inputs

Resolve PR #608's head and base through the GitHub REST API and fetch them as
`origin/pr-608` and `origin/pr-608-base` throughout. Run every command from the
dedicated `fix/597-retention-health-v2` worktree; command blocks normalize to
that worktree's root with `git rev-parse`.

Every implementation commit must use `git commit -s` and include verified
GitHub-linked `Co-authored-by` trailers for Timothy Wayne Gregg
(`CompleteDotTech`) and Copilot. Resolve their numeric IDs with `gh api
users/<login>` rather than embedding addresses in this document.

Before committing, populate the trailer variables from REST-resolved IDs:

```bash
TIMOTHY_ID="$(gh api users/CompleteDotTech --jq .id)"
COPILOT_ID="$(gh api users/Copilot --jq .id)"
TIMOTHY_COAUTHOR_TRAILER="Co-authored-by: Timothy Wayne Gregg <${TIMOTHY_ID}+CompleteDotTech@users.noreply.github.com>"
COPILOT_COAUTHOR_TRAILER="Co-authored-by: Copilot <${COPILOT_ID}+Copilot@users.noreply.github.com>"
```

### Task 1: Add Exact Event-Writer Backlog Counts

**Files:**
- Modify: `crates/coven-cli/src/event_writer.rs:31-115`
- Modify: `crates/coven-cli/src/event_writer.rs:191-275`
- Modify: `crates/coven-cli/src/event_writer.rs:482-506`
- Test: `crates/coven-cli/src/event_writer.rs:524-703`

- [ ] **Step 1: Refresh the claim and verify the source PR**

Run:

```bash
cd "$(git rev-parse --show-toplevel)"
coven claim heartbeat issue-597
PR_HEAD="$(gh api repos/OpenCoven/coven/pulls/608 --jq .head.sha)"
PR_BASE="$(gh api repos/OpenCoven/coven/pulls/608 --jq .base.sha)"
git fetch origin "$PR_HEAD":refs/remotes/origin/pr-608 --force
git fetch origin "$PR_BASE":refs/remotes/origin/pr-608-base --force
test "$(git rev-parse origin/pr-608)" = "$PR_HEAD"
test "$(git rev-parse origin/pr-608-base)" = "$PR_BASE"
```

Expected: claim renewal succeeds and the fetched PR head matches the fixed SHA.

- [ ] **Step 2: Write a failing queue-accounting unit test**

Add this test to `event_writer.rs`:

```rust
#[test]
fn queue_health_counts_events_until_completion() {
    let shared = Arc::new(Shared {
        queue: Mutex::new(Queue {
            items: VecDeque::new(),
            queued_events: 2,
            queued_bytes: EVENT_OVERHEAD_BYTES * 2,
            failed: None,
        }),
        available: Condvar::new(),
        capacity_bytes: DEFAULT_CAPACITY_BYTES,
        output_capacity_bytes: DEFAULT_CAPACITY_BYTES - RESERVED_CRITICAL_BYTES,
        health: Mutex::new(EventWriterHealth {
            state: "healthy".to_string(),
            queued_events: 2,
            queued_bytes: EVENT_OVERHEAD_BYTES * 2,
            capacity_bytes: DEFAULT_CAPACITY_BYTES,
            dropped_output_events: 0,
            dropped_output_bytes: 0,
            connection_opens: 0,
            transactions: 0,
            committed_events: 0,
            last_error: None,
        }),
    });

    release_capacity(&shared, 1, EVENT_OVERHEAD_BYTES);

    let health = lock_health(&shared);
    assert_eq!(health.queued_events, 1);
    assert_eq!(health.queued_bytes, EVENT_OVERHEAD_BYTES);
}
```

- [ ] **Step 3: Run the test and verify RED**

Run:

```bash
cd "$(git rev-parse --show-toplevel)"
cargo test -p coven-cli queue_health_counts_events_until_completion --locked
```

Expected: compilation fails because `queued_events` and `release_capacity` do not exist.

- [ ] **Step 4: Add queued-event state**

Update the structs and initializers:

```rust
pub struct EventWriterHealth {
    pub state: String,
    pub queued_events: usize,
    pub queued_bytes: usize,
    pub capacity_bytes: usize,
    pub dropped_output_events: u64,
    pub dropped_output_bytes: u64,
    pub connection_opens: u64,
    pub transactions: u64,
    pub committed_events: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

struct Queue {
    items: VecDeque<QueuedEvent>,
    queued_events: usize,
    queued_bytes: usize,
    failed: Option<String>,
}
```

Set both new fields to zero in `EventWriter::start_with_capacity`.

- [ ] **Step 5: Update enqueue and health synchronization**

For both `enqueue_output` and `enqueue_critical`, increment before publishing:

```rust
queue.queued_events += 1;
queue.queued_bytes += bytes;
self.update_queue_health(queue.queued_events, queue.queued_bytes);
```

Replace `update_queued_bytes` with:

```rust
fn update_queue_health(&self, events: usize, bytes: usize) {
    let mut health = self.lock_health();
    health.queued_events = events;
    health.queued_bytes = bytes;
}
```

- [ ] **Step 6: Release counts only after commit completion**

Replace `release_bytes` with:

```rust
fn release_capacity(shared: &Arc<Shared>, events: usize, bytes: usize) {
    let mut queue = lock_queue(shared);
    queue.queued_events = queue.queued_events.saturating_sub(events);
    queue.queued_bytes = queue.queued_bytes.saturating_sub(bytes);
    let mut health = lock_health(shared);
    health.queued_events = queue.queued_events;
    health.queued_bytes = queue.queued_bytes;
    shared.available.notify_all();
}
```

After a successful batch commit, call:

```rust
release_capacity(&shared, batch.len(), bytes);
```

In `fail_writer`, set both queue and health event/byte counts to zero. Add
`queued_events` to every test fixture that constructs `Queue` or
`EventWriterHealth`.

- [ ] **Step 7: Run focused writer tests**

Run:

```bash
cd "$(git rev-parse --show-toplevel)"
cargo test -p coven-cli event_writer::tests --locked
cargo fmt --check
git diff --check
```

Expected: all event-writer tests pass and formatting checks are clean.

- [ ] **Step 8: Commit writer accounting**

Run:

```bash
cd "$(git rev-parse --show-toplevel)"
git add crates/coven-cli/src/event_writer.rs
python3 scripts/check-coven-privacy.py --staged
git commit -s -m "feat(runtime): expose queued event backlog" \
  -m "$TIMOTHY_COAUTHOR_TRAILER" -m "$COPILOT_COAUTHOR_TRAILER"
```

Expected: one signed commit containing only event-writer accounting and tests.

### Task 2: Port Bounded Retention and Storage Health

**Files:**
- Modify: `crates/coven-cli/src/store.rs`
- Test: `crates/coven-cli/src/store.rs`

- [ ] **Step 1: Apply PR #608's store patch with three-way context**

Run:

```bash
cd "$(git rev-parse --show-toplevel)"
git diff --binary origin/pr-608-base..origin/pr-608 -- \
  crates/coven-cli/src/store.rs > /tmp/coven-pr608-store.patch
git apply --3way /tmp/coven-pr608-store.patch
```

Expected: the PR's store additions are imported. If Git reports conflicts,
retain every current-main schema/session-handoff change and add the PR's
maintenance constants, `StorageHealth`, `ScheduledMaintenanceReport`, indexes,
bounded prune functions, scheduler helpers, storage-health helpers, and tests.

- [ ] **Step 2: Write the writer-snapshot storage test before integration**

Update the imported storage-health test to construct:

```rust
let writer = crate::event_writer::EventWriterHealth {
    state: "pressured".to_string(),
    queued_events: 7,
    queued_bytes: 8192,
    capacity_bytes: 2 * 1024 * 1024,
    dropped_output_events: 1,
    dropped_output_bytes: 512,
    connection_opens: 1,
    transactions: 3,
    committed_events: 12,
    last_error: None,
};

let health = storage_health(home, Some(&writer))?;
assert_eq!(health.writer_backlog_events, 7);
assert_eq!(health.writer_backlog_bytes, 8192);
```

Update existing imported calls to pass `None`.

- [ ] **Step 3: Run the imported focused tests and verify RED**

Run:

```bash
cd "$(git rev-parse --show-toplevel)"
cargo test -p coven-cli scheduled_maintenance --locked
cargo test -p coven-cli storage_health_flags_stale_retention --locked
```

Expected: compilation fails until the imported PR code is reconciled with
current types and the new writer-snapshot signature.

- [ ] **Step 4: Fix the convergence-loop compile errors**

Keep SQL limits as `i64` and compare returned delete counts as `usize`:

```rust
if raw_batch < MAINTENANCE_ARTIFACT_BATCH_SIZE as usize
    && event_batch < MAINTENANCE_EVENT_BATCH_SIZE as usize
{
    break;
}
```

Give the oldest event an explicit type:

```rust
let oldest_retained_event_at: Option<String> = conn
    .query_row("SELECT MIN(created_at) FROM events", [], |row| row.get(0))
    .context("failed to read oldest retained event")?;
```

Adjust the exact query form to the imported code while preserving
`Option<String>`.

- [ ] **Step 5: Derive storage backlog from the writer snapshot**

Change the public collector to:

```rust
pub fn storage_health(
    coven_home: &Path,
    event_writer: Option<&crate::event_writer::EventWriterHealth>,
) -> Result<StorageHealth>
```

Derive the fields with:

```rust
let (writer_backlog_events, writer_backlog_bytes) = event_writer
    .map(|health| {
        (
            health.queued_events as u64,
            health.queued_bytes as u64,
        )
    })
    .unwrap_or_default();
```

Use those variables in the returned `StorageHealth`.

Change the unavailable constructor to:

```rust
pub fn unavailable_storage_health(
    coven_home: &Path,
    _error: impl ToString,
    known_free_disk_bytes: Option<u64>,
    event_writer: Option<&crate::event_writer::EventWriterHealth>,
) -> StorageHealth
```

Populate its backlog fields from the same snapshot helper instead of literals.

- [ ] **Step 6: Preserve the reviewed retention behavior**

Verify the imported code includes all of these exact symbols:

```text
idx_events_created_at
idx_sensitive_artifacts_created_at
prune_events_older_than_bounded
prune_sensitive_artifacts_bounded
run_scheduled_maintenance
run_scheduled_maintenance_with_free_disk
run_scheduled_maintenance_with_config_and_free_disk
storage_health
unavailable_storage_health
record_maintenance_error
```

Verify the low-disk return occurs before `open_store`, automatic maintenance
never calls `VACUUM`, checkpoint mode remains `PRAGMA wal_checkpoint(PASSIVE)`,
and the catch-up loop remains bounded by
`MAINTENANCE_MAX_BATCHES_PER_TICK`.

- [ ] **Step 7: Run focused store tests**

Run:

```bash
cd "$(git rev-parse --show-toplevel)"
cargo test -p coven-cli bounded_event_pruning_keeps_fts_consistent --locked
cargo test -p coven-cli scheduled_maintenance --locked
cargo test -p coven-cli thirty_day_synthetic_workload_converges --locked
cargo test -p coven-cli storage_health_flags_stale_retention --locked
cargo fmt --check
git diff --check
```

Expected: all imported and updated retention/storage tests pass.

- [ ] **Step 8: Commit the store port**

Run:

```bash
cd "$(git rev-parse --show-toplevel)"
git add crates/coven-cli/src/store.rs
python3 scripts/check-coven-privacy.py --staged
git commit -s -m "feat(store): automate bounded retention health" \
  -m "$TIMOTHY_COAUTHOR_TRAILER" -m "$COPILOT_COAUTHOR_TRAILER"
```

Expected: one signed store commit preserving contributor credit.

### Task 3: Port Maintenance Scheduling and Recovery-Log Rotation

**Files:**
- Modify: `crates/coven-cli/src/daemon.rs`
- Test: `crates/coven-cli/src/daemon.rs`

- [ ] **Step 1: Apply the daemon patch**

Run:

```bash
cd "$(git rev-parse --show-toplevel)"
git diff --binary origin/pr-608-base..origin/pr-608 -- \
  crates/coven-cli/src/daemon.rs > /tmp/coven-pr608-daemon.patch
git apply --3way /tmp/coven-pr608-daemon.patch
```

Expected: import `DAEMON_RECOVERY_LOG_MAX_BYTES`,
`DAEMON_RECOVERY_LOG_BACKUPS`, `recovery_log_lock`,
`rotate_recovery_log`, `start_store_maintenance_scheduler`, both daemon-start
call sites, and the recovery rotation test. Preserve current main's event
writer, maintenance gate, session handoff, and lifecycle behavior.

- [ ] **Step 2: Verify scheduler failure behavior**

The imported scheduler must retain:

```rust
let details = format!("store maintenance pass failed: {error:#}");
crate::store::record_maintenance_error(&home, &details);
append_daemon_recovery_log(&home, &details);
```

The scheduler starts after the live runtime is created on both Unix and
Windows, waits one interval before the first pass, uses a capped catch-up loop,
and never blocks daemon startup on maintenance work.

- [ ] **Step 3: Run daemon tests**

Run:

```bash
cd "$(git rev-parse --show-toplevel)"
cargo test -p coven-cli append_daemon_recovery_log_creates_and_appends --locked
cargo test -p coven-cli recovery_log_rotation_keeps_a_bounded_history --locked
cargo fmt --check
git diff --check
```

Expected: recovery log tests pass and no current daemon behavior is deleted.

- [ ] **Step 4: Commit daemon integration**

Run:

```bash
cd "$(git rev-parse --show-toplevel)"
git add crates/coven-cli/src/daemon.rs
python3 scripts/check-coven-privacy.py --staged
git commit -s -m "feat(daemon): schedule bounded store maintenance" \
  -m "$TIMOTHY_COAUTHOR_TRAILER" -m "$COPILOT_COAUTHOR_TRAILER"
```

Expected: one signed daemon commit.

### Task 4: Integrate Storage into the Current Health Contract

**Files:**
- Modify: `crates/coven-cli/src/api.rs`
- Modify: `docs/daemon/health.md`
- Modify: `docs/reference/api.md`
- Modify: `docs/reference/cli-logs.md`
- Test: `crates/coven-cli/src/api.rs`

- [ ] **Step 1: Apply the API and documentation patches**

Run:

```bash
cd "$(git rev-parse --show-toplevel)"
git diff --binary origin/pr-608-base..origin/pr-608 -- \
  crates/coven-cli/src/api.rs \
  docs/daemon/health.md \
  docs/reference/api.md \
  docs/reference/cli-logs.md > /tmp/coven-pr608-health-docs.patch
git apply --3way /tmp/coven-pr608-health-docs.patch
```

Expected: `docs/reference/cli-logs.md` applies cleanly. Resolve other conflicts
by preserving all current `HealthCapabilities`, `event_writer`, runtime trait,
session-handoff, and current documentation, then adding storage health.

- [ ] **Step 2: Preserve both health objects**

The response must contain:

```rust
pub struct HealthResponse {
    pub ok: bool,
    pub api_version: String,
    pub coven_version: String,
    pub capabilities: HealthCapabilities,
    pub daemon: Option<DaemonStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hub: Option<HubHealth>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_writer: Option<crate::event_writer::EventWriterHealth>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub storage: Option<store::StorageHealth>,
}
```

Keep `session_handoff: true` in `health_response`.

- [ ] **Step 3: Build storage and event-writer health from one snapshot**

Use:

```rust
fn health_response_with_hub(
    coven_home: &Path,
    daemon: Option<DaemonStatus>,
    event_writer: Option<crate::event_writer::EventWriterHealth>,
) -> HealthResponse {
    let mut response = health_response(daemon);
    if let Ok(summary) = crate::hub::hub_health_summary(coven_home) {
        response.hub = serde_json::from_value(summary).ok();
    }
    response.storage = Some(
        store::storage_health(coven_home, event_writer.as_ref()).unwrap_or_else(|error| {
            store::unavailable_storage_health(coven_home, error, None, event_writer.as_ref())
        }),
    );
    response.event_writer = event_writer;
    response
}
```

Keep the `/health` route passing `runtime.event_writer_health()`.

- [ ] **Step 4: Write a health integration test with a real snapshot fixture**

Add a local runtime fixture:

```rust
struct HealthRuntime;

impl SessionRuntime for HealthRuntime {
    fn launch_session(&self, _launch: &SessionLaunch) -> Result<()> {
        Ok(())
    }

    fn send_input(&self, _session_id: &str, _payload: &Value) -> Result<()> {
        Ok(())
    }

    fn kill_session(&self, _session_id: &str) -> Result<()> {
        Ok(())
    }

    fn event_writer_health(&self) -> Option<crate::event_writer::EventWriterHealth> {
        Some(crate::event_writer::EventWriterHealth {
            state: "pressured".to_string(),
            queued_events: 3,
            queued_bytes: 4096,
            capacity_bytes: 2 * 1024 * 1024,
            dropped_output_events: 1,
            dropped_output_bytes: 256,
            connection_opens: 1,
            transactions: 4,
            committed_events: 20,
            last_error: None,
        })
    }
}
```

Call `handle_request_with_runtime` for `/health`, parse the JSON, and assert:

```rust
assert_eq!(body["capabilities"]["sessionHandoff"], true);
assert_eq!(body["eventWriter"]["queuedEvents"], 3);
assert_eq!(body["eventWriter"]["queuedBytes"], 4096);
assert_eq!(body["storage"]["writerBacklogEvents"], 3);
assert_eq!(body["storage"]["writerBacklogBytes"], 4096);
```

- [ ] **Step 5: Correct stale documentation**

In `docs/daemon/health.md`, remove the PR text claiming a future writer will
populate backlog. State that `storage.writerBacklogEvents` and
`storage.writerBacklogBytes` mirror the live `eventWriter` queue snapshot.

In `docs/reference/api.md`, retain all current capability fields including
`sessionHandoff`, document both optional `eventWriter` and `storage` objects,
and do not remove current session-handoff or event-writer content.

- [ ] **Step 6: Run health and documentation tests**

Run:

```bash
cd "$(git rev-parse --show-toplevel)"
cargo test -p coven-cli builds_health_response --locked
cargo test -p coven-cli routes_health_request_to_json --locked
cargo test -p coven-cli health_is_the_named_contract_handshake --locked
python3 scripts/check-api-contract-docs.py
cargo fmt --check
git diff --check
```

Expected: health tests prove both surfaces and the API docs checker passes.

- [ ] **Step 7: Commit API and docs integration**

Run:

```bash
cd "$(git rev-parse --show-toplevel)"
git add crates/coven-cli/src/api.rs \
  docs/daemon/health.md \
  docs/reference/api.md \
  docs/reference/cli-logs.md
python3 scripts/check-coven-privacy.py --staged
git commit -s -m "feat(api): expose integrated storage health" \
  -m "$TIMOTHY_COAUTHOR_TRAILER" -m "$COPILOT_COAUTHOR_TRAILER"
```

Expected: one signed integration commit preserving current API behavior.

### Task 5: Validate, Open the Replacement, and Supersede PR #608

**Files:**
- Verify all seven modified implementation/documentation files.
- Verify `docs/superpowers/specs/2026-08-05-retention-health-replacement-design.md`.
- Verify `docs/superpowers/plans/2026-08-05-retention-health-replacement.md`.

- [ ] **Step 1: Run the complete repository gates**

Run:

```bash
cd "$(git rev-parse --show-toplevel)"
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --locked
python scripts/check-secrets.py
python3 scripts/check-api-contract-docs.py
```

Expected: every command passes.

- [ ] **Step 2: Confirm DCO, attribution, and scope**

Run:

```bash
cd "$(git rev-parse --show-toplevel)"
git status --short
git --no-pager log --format='%h%n%B%n---' origin/main..HEAD
git --no-pager diff --stat origin/main...HEAD
git diff --name-only origin/main...HEAD
```

Expected: worktree clean; every implementation commit has `Signed-off-by`,
the Timothy Wayne Gregg and Copilot co-author trailers are present, and the diff
contains only:

```text
crates/coven-cli/src/api.rs
crates/coven-cli/src/daemon.rs
crates/coven-cli/src/event_writer.rs
crates/coven-cli/src/store.rs
docs/daemon/health.md
docs/reference/api.md
docs/reference/cli-logs.md
docs/superpowers/plans/2026-08-05-retention-health-replacement.md
docs/superpowers/specs/2026-08-05-retention-health-replacement-design.md
```

- [ ] **Step 3: Push and create the replacement PR**

Run:

```bash
cd "$(git rev-parse --show-toplevel)"
git push -u origin fix/597-retention-health-v2
gh pr create \
  --repo OpenCoven/coven \
  --base main \
  --head fix/597-retention-health-v2 \
  --title "feat(store): automate bounded retention health" \
  --body $'## Context\n\n- Summary: Port the reviewed retention and storage-health work from #608 onto current main without regressing event-writer or session-handoff health.\n- Files changed: Rust daemon/store/API/event-writer code and directly related health/log documentation.\n- Closes #597\n\n## Implementation\n\n- Approach: Squash-port #608 onto current main as signed maintainer commits with original contributor attribution.\n- User-visible behavior: Bounded scheduled retention, storage pressure in `/health`, real writer backlog values, and bounded recovery-log rotation.\n- Compatibility notes: `eventWriter` and `sessionHandoff` remain present; `storage` is additive. Automatic maintenance never runs `VACUUM`.\n\n## Verification\n\n- [x] `cargo fmt --check`\n- [x] `cargo clippy --workspace --all-targets -- -D warnings`\n- [x] `cargo test --workspace --locked`\n- [x] `python3 scripts/check-secrets.py`\n- [x] Additional manual checks: `python3 scripts/check-api-contract-docs.py`; focused retention, writer-health, watermark, FTS, and recovery-log tests.\n\n## Risk and Rollback\n\n- Risk level: Medium; changes background SQLite maintenance and additive health fields.\n- Rollback plan: Revert this PR to stop scheduled retention and remove the additive storage health block.\n\n## Agent Handoff\n\n- Current state: Replacement for conflicted/DCO-blocked #608.\n- Follow-ups: Non-blocking review notes from #608 remain separate.\n- Known gaps: No automatic `VACUUM`; compaction remains operator-triggered.'
```

Expected: a new PR targeting `main` is created.

- [ ] **Step 4: Supersede PR #608**

Store the new URL and comment before closing:

```bash
NEW_PR_URL="$(gh pr view --repo OpenCoven/coven --json url --jq .url)"
gh pr comment 608 --repo OpenCoven/coven \
  --body "Superseded by ${NEW_PR_URL}, which ports this work onto current main, integrates live event-writer health, preserves contributor attribution, and resolves the DCO/compile blockers."
gh pr close 608 --repo OpenCoven/coven
```

Expected: PR #608 is closed with a clear pointer to the replacement.

- [ ] **Step 5: Keep the claim active through integration**

Run:

```bash
cd "$(git rev-parse --show-toplevel)"
coven claim heartbeat issue-597
```

Expected: `issue-597` remains active until the replacement merges or work stops.
