# Per-Session Output Truncation Markers Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Persist one exact `output_truncated` event for each per-session raw-output pressure episode.

**Architecture:** The event-writer queue tracks active truncation episodes by
session while rejected PTY output remains non-blocking. The next accepted event
for that session closes the episode by queueing an immutable critical marker
immediately before that event, preserving append-only ordering and cursor
semantics.

**Tech Stack:** Rust, rusqlite, serde_json, std synchronization primitives,
Markdown API documentation

---

### Task 1: Track and close output pressure episodes

**Files:**
- Modify: `crates/coven-cli/src/event_writer.rs`

- [x] **Step 1: Write failing recovery-boundary tests**

Add these tests beside `pressure_is_visible_when_raw_output_exceeds_its_budget`:

```rust
#[test]
fn recovered_output_is_preceded_by_one_exact_truncation_marker() -> Result<()> {
    let home = tempfile::tempdir()?;
    let conn = store::open_store(&home.path().join(STORE_FILE_NAME))?;
    store::insert_session(&conn, &session("s-1"))?;
    let writer = EventWriter::start_with_capacity(
        home.path().to_path_buf(),
        RESERVED_CRITICAL_BYTES + 1024,
    )?;

    assert!(!writer.record_output("s-1", "x".repeat(2048))?);
    assert!(!writer.record_output("s-1", "y".repeat(3072))?);
    assert!(writer.record_output("s-1", "recovered".to_string())?);
    writer.record_exit(
        "s-1",
        PtyRunResult {
            status: "completed",
            exit_code: Some(0),
        },
    )?;

    let events = store::list_events(&conn, "s-1")?;
    assert_eq!(
        events.iter().map(|event| event.kind.as_str()).collect::<Vec<_>>(),
        vec!["output_truncated", "output", "exit"]
    );
    let payload: serde_json::Value = serde_json::from_str(&events[0].payload_json)?;
    assert_eq!(payload["droppedEvents"], 2);
    assert_eq!(payload["droppedBytes"], 5120);
    assert!(events[0].created_at <= events[1].created_at);
    Ok(())
}

#[test]
fn accepted_output_without_pressure_has_no_truncation_marker() -> Result<()> {
    let home = tempfile::tempdir()?;
    let conn = store::open_store(&home.path().join(STORE_FILE_NAME))?;
    store::insert_session(&conn, &session("s-1"))?;
    let writer = EventWriter::start(home.path().to_path_buf())?;

    assert!(writer.record_output("s-1", "complete".to_string())?);
    writer.record_exit(
        "s-1",
        PtyRunResult {
            status: "completed",
            exit_code: Some(0),
        },
    )?;

    let events = store::list_events(&conn, "s-1")?;
    assert!(events.iter().all(|event| event.kind != "output_truncated"));
    Ok(())
}
```

- [x] **Step 2: Run the tests to verify they fail**

Run:

```bash
cargo test -p coven-cli event_writer::tests::recovered_output_is_preceded_by_one_exact_truncation_marker
```

Expected: FAIL because no `output_truncated` event exists.

- [x] **Step 3: Add queue-owned episode state**

Import `HashMap` and add the episode type and queue field:

```rust
use std::{
    collections::{HashMap, VecDeque},
    // existing imports
};

struct Queue {
    items: VecDeque<QueuedEvent>,
    truncations: HashMap<String, OutputTruncation>,
    queued_events: usize,
    queued_bytes: usize,
    failed: Option<String>,
}

struct OutputTruncation {
    dropped_events: u64,
    dropped_bytes: u64,
    created_at: String,
}
```

Initialize `truncations: HashMap::new()` in production and test `Queue`
constructors.

- [x] **Step 4: Add marker construction helpers**

Add these helpers above `take_batch`:

```rust
fn record_output_drop(
    queue: &mut Queue,
    session_id: &str,
    dropped_bytes: usize,
    created_at: &str,
) {
    let truncation = queue
        .truncations
        .entry(session_id.to_string())
        .or_insert_with(|| OutputTruncation {
            dropped_events: 0,
            dropped_bytes: 0,
            created_at: created_at.to_string(),
        });
    truncation.dropped_events = truncation.dropped_events.saturating_add(1);
    truncation.dropped_bytes = truncation
        .dropped_bytes
        .saturating_add(dropped_bytes as u64);
}

fn take_truncation_marker(queue: &mut Queue, session_id: &str) -> Option<QueuedEvent> {
    let truncation = queue.truncations.remove(session_id)?;
    let payload_json = json!({
        "droppedEvents": truncation.dropped_events,
        "droppedBytes": truncation.dropped_bytes,
    })
    .to_string();
    let bytes = payload_json.len().saturating_add(EVENT_OVERHEAD_BYTES);
    Some(QueuedEvent {
        event: PendingEvent::Record(store::EventRecord {
            seq: 0,
            id: Uuid::new_v4().to_string(),
            session_id: session_id.to_string(),
            kind: "output_truncated".to_string(),
            payload_json,
            created_at: truncation.created_at,
        }),
        bytes,
        completion: None,
    })
}
```

- [x] **Step 5: Record drops and queue a marker before recovered output**

In `enqueue_output`, extract the output fields before the pressure check. On
rejection, update the episode and existing health counters. On acceptance,
remove the marker and push it before the output:

```rust
let PendingEvent::Output {
    session_id,
    data,
    created_at,
} = &event
else {
    unreachable!("enqueue_output only accepts output events");
};

if bytes > self.shared.output_capacity_bytes
    || queue.queued_bytes.saturating_add(bytes) > self.shared.output_capacity_bytes
{
    record_output_drop(&mut queue, session_id, data.len(), created_at);
    let mut health = self.lock_health();
    health.state = "pressured".to_string();
    health.dropped_output_events += 1;
    health.dropped_output_bytes += data.len() as u64;
    return Ok(false);
}

let marker = take_truncation_marker(&mut queue, session_id);
let marker_bytes = marker.as_ref().map_or(0, |item| item.bytes);
let marker_events = usize::from(marker.is_some());
debug_assert!(
    queue
        .queued_bytes
        .saturating_add(bytes)
        .saturating_add(marker_bytes)
        <= self.shared.capacity_bytes
);
queue.queued_events += 1 + marker_events;
queue.queued_bytes += bytes + marker_bytes;
self.update_queue_health(queue.queued_events, queue.queued_bytes);
if let Some(marker) = marker {
    queue.items.push_back(marker);
}
queue.items.push_back(QueuedEvent {
    event,
    bytes,
    completion: None,
});
```

- [x] **Step 6: Run the event-writer tests**

Run:

```bash
cargo test -p coven-cli event_writer::tests::
```

Expected: all event-writer tests pass.

- [x] **Step 7: Commit the recovery-boundary implementation**

```bash
git add crates/coven-cli/src/event_writer.rs
git commit -s -m "fix: mark recovered output truncation"
```

### Task 2: Close episodes before critical and terminal events

**Files:**
- Modify: `crates/coven-cli/src/event_writer.rs`

- [x] **Step 1: Write failing critical-boundary tests**

Add tests covering exit, independent sessions, and an event that cannot share
one capacity window with its marker:

```rust
#[test]
fn exit_closes_pressure_episode_before_terminal_event() -> Result<()> {
    let home = tempfile::tempdir()?;
    let conn = store::open_store(&home.path().join(STORE_FILE_NAME))?;
    store::insert_session(&conn, &session("s-1"))?;
    let writer = EventWriter::start_with_capacity(
        home.path().to_path_buf(),
        RESERVED_CRITICAL_BYTES + 1024,
    )?;

    assert!(!writer.record_output("s-1", "x".repeat(2048))?);
    writer.record_exit(
        "s-1",
        PtyRunResult {
            status: "failed",
            exit_code: Some(1),
        },
    )?;

    let events = store::list_events(&conn, "s-1")?;
    assert_eq!(events[0].kind, "output_truncated");
    assert_eq!(events[1].kind, "exit");
    Ok(())
}

#[test]
fn pressure_episodes_are_isolated_per_session() -> Result<()> {
    let home = tempfile::tempdir()?;
    let conn = store::open_store(&home.path().join(STORE_FILE_NAME))?;
    store::insert_session(&conn, &session("s-1"))?;
    store::insert_session(&conn, &session("s-2"))?;
    let writer = EventWriter::start_with_capacity(
        home.path().to_path_buf(),
        RESERVED_CRITICAL_BYTES + 1024,
    )?;

    assert!(!writer.record_output("s-1", "a".repeat(2048))?);
    assert!(!writer.record_output("s-2", "b".repeat(3072))?);
    writer.record_exit(
        "s-1",
        PtyRunResult {
            status: "failed",
            exit_code: Some(1),
        },
    )?;
    writer.record_exit(
        "s-2",
        PtyRunResult {
            status: "failed",
            exit_code: Some(1),
        },
    )?;

    let first: serde_json::Value =
        serde_json::from_str(&store::list_events(&conn, "s-1")?[0].payload_json)?;
    let second: serde_json::Value =
        serde_json::from_str(&store::list_events(&conn, "s-2")?[0].payload_json)?;
    assert_eq!(first["droppedBytes"], 2048);
    assert_eq!(second["droppedBytes"], 3072);
    Ok(())
}

#[test]
fn oversized_critical_event_commits_marker_before_waiting_for_its_own_capacity() -> Result<()> {
    let home = tempfile::tempdir()?;
    let conn = store::open_store(&home.path().join(STORE_FILE_NAME))?;
    store::insert_session(&conn, &session("s-1"))?;
    let capacity = RESERVED_CRITICAL_BYTES + 1024;
    let writer = EventWriter::start_with_capacity(home.path().to_path_buf(), capacity)?;
    assert!(!writer.record_output("s-1", "x".repeat(2048))?);

    writer.enqueue_critical(
        PendingEvent::Record(store::EventRecord {
            seq: 0,
            id: "large-critical".to_string(),
            session_id: "s-1".to_string(),
            kind: "error".to_string(),
            payload_json: "{}".to_string(),
            created_at: "2026-08-05T00:00:01Z".to_string(),
        }),
        capacity - 1,
    )?;

    let events = store::list_events(&conn, "s-1")?;
    assert_eq!(events[0].kind, "output_truncated");
    assert_eq!(events[1].kind, "error");
    Ok(())
}
```

- [x] **Step 2: Run the tests to verify they fail**

Run:

```bash
cargo test -p coven-cli event_writer::tests::exit_closes_pressure_episode_before_terminal_event
```

Expected: FAIL because exit currently bypasses the active episode map.

- [x] **Step 3: Add a session-id accessor**

Add:

```rust
impl PendingEvent {
    fn session_id(&self) -> &str {
        match self {
            Self::Output { session_id, .. } | Self::Exit { session_id, .. } => session_id,
            Self::Record(record) => &record.session_id,
        }
    }
}
```

- [x] **Step 4: Queue marker and critical event atomically when they fit**

Refactor `enqueue_critical` so it removes the session episode while holding the
queue lock, waits for `marker.bytes + bytes`, pushes the marker first, and gives
only the caller's event the external completion channel:

```rust
let session_id = event.session_id().to_string();
let (completion_tx, completion_rx) = mpsc::sync_channel(1);
let mut queue = self.lock_queue();
let mut marker = take_truncation_marker(&mut queue, &session_id);
let marker_bytes = marker.as_ref().map_or(0, |item| item.bytes);

if marker_bytes.saturating_add(bytes) <= self.shared.capacity_bytes {
    let required = marker_bytes.saturating_add(bytes);
    while queue.queued_bytes.saturating_add(required) > self.shared.capacity_bytes {
        if let Some(error) = &queue.failed {
            return Err(anyhow!(error.clone()));
        }
        queue = self
            .shared
            .available
            .wait(queue)
            .unwrap_or_else(|poisoned| poisoned.into_inner());
    }
    queue.queued_events += 1 + usize::from(marker.is_some());
    queue.queued_bytes += required;
    self.update_queue_health(queue.queued_events, queue.queued_bytes);
    if let Some(marker) = marker.take() {
        queue.items.push_back(marker);
    }
    queue.items.push_back(QueuedEvent {
        event,
        bytes,
        completion: Some(completion_tx),
    });
    self.shared.available.notify_one();
    drop(queue);
    return receive_completion(completion_rx);
}
```

Extract the existing receiver match into:

```rust
fn receive_completion(
    completion_rx: mpsc::Receiver<std::result::Result<(), String>>,
) -> Result<()> {
    match completion_rx
        .recv()
        .context("event writer stopped before committing a critical event")?
    {
        Ok(()) => Ok(()),
        Err(message) => Err(anyhow!(message)),
    }
}
```

- [x] **Step 5: Handle the oversized sequential case without deadlock**

When `marker_bytes + bytes > capacity`, queue the marker alone with an internal
completion sender, wait for its commit, then call `enqueue_critical` again for
the original event:

```rust
let marker = marker.expect("combined size exceeds capacity only with a marker");
let (marker_tx, marker_rx) = mpsc::sync_channel(1);
let marker = QueuedEvent {
    completion: Some(marker_tx),
    ..marker
};
while queue.queued_bytes.saturating_add(marker.bytes) > self.shared.capacity_bytes {
    if let Some(error) = &queue.failed {
        return Err(anyhow!(error.clone()));
    }
    queue = self
        .shared
        .available
        .wait(queue)
        .unwrap_or_else(|poisoned| poisoned.into_inner());
}
queue.queued_events += 1;
queue.queued_bytes += marker.bytes;
self.update_queue_health(queue.queued_events, queue.queued_bytes);
queue.items.push_back(marker);
self.shared.available.notify_one();
drop(queue);
receive_completion(marker_rx)?;
self.enqueue_critical(event, bytes)
```

- [x] **Step 6: Clear active episodes on writer failure**

In `fail_writer`, add:

```rust
queue.truncations.clear();
```

- [x] **Step 7: Run event-writer and daemon observer tests**

Run:

```bash
cargo test -p coven-cli event_writer::tests::
cargo test -p coven-cli daemon::tests::output_observer
```

Expected: all selected tests pass.

- [x] **Step 8: Commit critical-boundary behavior**

```bash
git add crates/coven-cli/src/event_writer.rs
git commit -s -m "fix: preserve truncation markers before terminal events"
```

### Task 3: Document the additive event contract

**Files:**
- Modify: `docs/API-CONTRACT.md`
- Modify: `docs/reference/api-contract.md`
- Modify: `docs/daemon/health.md`

- [x] **Step 1: Document pressure visibility and output coalescing**

After the `eventWriter` health description in `docs/API-CONTRACT.md`, add:

```markdown
Rejected raw output remains visible in the affected session. Coven emits one
`output_truncated` event per contiguous pressure episode before that session's
next accepted event. Its payload contains `droppedEvents` and `droppedBytes`;
the latter counts rejected UTF-8 payload bytes and excludes queue overhead.

Adjacent accepted `output` callbacks for the same session may coalesce into one
event. The merged event keeps the first accepted callback's `created_at`.
```

In the events response section, add an `output_truncated` example:

```json
{
  "seq": 43,
  "id": "event-uuid",
  "session_id": "session-uuid",
  "kind": "output_truncated",
  "payload_json": "{\"droppedEvents\":3,\"droppedBytes\":8192}",
  "created_at": "2026-05-09T06:43:11Z"
}
```

- [x] **Step 2: Update concise health references**

Add one sentence to both `docs/reference/api-contract.md` and
`docs/daemon/health.md`:

```markdown
Each affected session also receives one ordered `output_truncated` event with
exact dropped-event and dropped-byte totals when its pressure episode closes.
```

- [x] **Step 3: Check documentation consistency**

Run:

```bash
rg -n "output_truncated|droppedEvents|coalesce" \
  docs/API-CONTRACT.md docs/reference/api-contract.md docs/daemon/health.md
git diff --check
```

Expected: all three documents describe the same event name and camelCase
payload fields; `git diff --check` exits 0.

- [x] **Step 4: Commit the API documentation**

```bash
git add docs/API-CONTRACT.md docs/reference/api-contract.md docs/daemon/health.md
git commit -s -m "docs: define output truncation events"
```

### Task 4: Verify and publish

**Files:**
- Modify: `docs/superpowers/plans/2026-08-05-output-truncation-markers.md`

- [x] **Step 1: Run focused behavior tests**

Run:

```bash
cargo test -p coven-cli event_writer::tests::
cargo test -p coven-cli daemon::tests::output_observer
```

Expected: all selected tests pass with zero failures.

- [x] **Step 2: Run repository gates**

Run:

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --locked
python3 scripts/check-secrets.py
python3 scripts/check-coven-privacy.py --range origin/main..HEAD
git diff --check origin/main...HEAD
```

Expected: every command exits 0. Run the workspace test from a worktree inside
`.worktrees/` so Unix socket paths stay below `SUN_LEN`.

- [x] **Step 3: Request independent code review**

Review the full `origin/main...HEAD` diff against issue #611. Require the
reviewer to check queue bounds, per-session isolation, ordering, critical-event
acknowledgements, append-only event semantics, and privacy.

- [x] **Step 4: Mark the plan complete and commit**

Change all completed checkboxes in this plan to `[x]`, then run:

```bash
git add docs/superpowers/plans/2026-08-05-output-truncation-markers.md
git commit -s -m "docs: record output truncation verification"
```

- [ ] **Step 5: Push and open the issue-linked PR**

```bash
git push -u origin fix/611-output-truncation-marker
gh pr create \
  --repo OpenCoven/coven \
  --base main \
  --head fix/611-output-truncation-marker \
  --title "fix: mark per-session output truncation" \
  --body-file -
```

Use the repository PR template, include `Closes #611`, list exact focused and
repository-wide verification, and explain that markers close at the next
accepted event boundary to preserve immutable cursor semantics.

- [ ] **Step 6: Monitor review and CI**

Run:

```bash
gh pr checks --repo OpenCoven/coven --watch --interval 15
```

Address branch-specific failures and technically valid review feedback. After
merge, release `issue-611`, delete the remote branch, and remove the worktree.
