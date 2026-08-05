//! Bounded, daemon-owned persistence for high-volume live-session events.
//!
//! PTY drains must never open a SQLite connection for every raw read.  This
//! module owns one connection on one worker thread, accepts a byte-bounded
//! stream of events, and commits short batches.  Terminal events reserve space
//! and wait for a commit acknowledgement, so an accepted output chunk cannot
//! be overtaken by a following exit event.

use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::PathBuf,
    sync::{mpsc, Arc, Condvar, Mutex},
    thread,
    time::{Duration, Instant},
};

use anyhow::{anyhow, Context, Result};
use rusqlite::{Connection, ErrorCode};
use serde::Serialize;
use serde_json::json;
use uuid::Uuid;

use crate::{pty_runner::PtyRunResult, store, STORE_FILE_NAME};

const DEFAULT_CAPACITY_BYTES: usize = 2 * 1024 * 1024;
const RESERVED_CRITICAL_BYTES: usize = 128 * 1024;
const EVENT_OVERHEAD_BYTES: usize = 512;
const MAX_BATCH_EVENTS: usize = 64;
const COALESCE_WINDOW: Duration = Duration::from_millis(12);
const SQLITE_LOCK_COMMIT_ATTEMPTS: usize = 4;
const SQLITE_LOCK_RETRY_DELAY: Duration = Duration::from_millis(50);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EventWriterHealth {
    /// `healthy`, `pressured`, or `failed`.  Pressure remains visible for the
    /// daemon lifetime so a rejected raw chunk is never silently forgotten.
    pub state: String,
    #[serde(default)]
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

#[derive(Clone)]
pub struct EventWriter {
    shared: Arc<Shared>,
}

struct Shared {
    queue: Mutex<Queue>,
    available: Condvar,
    capacity_bytes: usize,
    output_capacity_bytes: usize,
    health: Mutex<EventWriterHealth>,
}

struct Queue {
    items: VecDeque<QueuedEvent>,
    queued_events: usize,
    queued_bytes: usize,
    failed: Option<String>,
    truncations: HashMap<String, OutputTruncation>,
    closing_sessions: HashSet<String>,
}

struct OutputTruncation {
    dropped_events: u64,
    dropped_bytes: u64,
    created_at: String,
}

struct QueuedEvent {
    event: PendingEvent,
    bytes: usize,
    completion: Option<mpsc::SyncSender<std::result::Result<(), String>>>,
}

enum PendingEvent {
    Output {
        session_id: String,
        data: String,
        created_at: String,
    },
    Record(store::EventRecord),
    Exit {
        session_id: String,
        result: PtyRunResult,
        created_at: String,
    },
}

impl PendingEvent {
    fn session_id(&self) -> &str {
        match self {
            Self::Output { session_id, .. } | Self::Exit { session_id, .. } => session_id,
            Self::Record(record) => &record.session_id,
        }
    }
}

impl EventWriter {
    pub fn start(coven_home: PathBuf) -> Result<Self> {
        Self::start_with_capacity(coven_home, DEFAULT_CAPACITY_BYTES)
    }

    fn start_with_capacity(coven_home: PathBuf, capacity_bytes: usize) -> Result<Self> {
        anyhow::ensure!(
            capacity_bytes > RESERVED_CRITICAL_BYTES,
            "event writer capacity must reserve room for critical events"
        );
        let shared = Arc::new(Shared {
            queue: Mutex::new(Queue {
                items: VecDeque::new(),
                queued_events: 0,
                queued_bytes: 0,
                failed: None,
                truncations: HashMap::new(),
                closing_sessions: HashSet::new(),
            }),
            available: Condvar::new(),
            capacity_bytes,
            output_capacity_bytes: capacity_bytes - RESERVED_CRITICAL_BYTES,
            health: Mutex::new(EventWriterHealth {
                state: "healthy".to_string(),
                queued_events: 0,
                queued_bytes: 0,
                capacity_bytes,
                dropped_output_events: 0,
                dropped_output_bytes: 0,
                connection_opens: 0,
                transactions: 0,
                committed_events: 0,
                last_error: None,
            }),
        });
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let worker_shared = Arc::clone(&shared);
        thread::Builder::new()
            .name("coven-event-writer".to_string())
            .spawn(move || run_worker(worker_shared, coven_home, ready_tx))
            .context("failed to spawn Coven event writer")?;
        match ready_rx
            .recv()
            .context("event writer stopped before opening its store connection")?
        {
            Ok(()) => Ok(Self { shared }),
            Err(message) => Err(anyhow!(message)),
        }
    }

    /// Queue raw PTY output without blocking the drain thread.  Raw output is
    /// the only lossy class under pressure; the health response carries the
    /// exact rejected count and byte total.
    pub fn record_output(&self, session_id: &str, data: String) -> Result<bool> {
        if data.is_empty() {
            return Ok(true);
        }
        let bytes = data.len().saturating_add(EVENT_OVERHEAD_BYTES);
        let event = PendingEvent::Output {
            session_id: session_id.to_string(),
            data,
            created_at: crate::api::current_timestamp(),
        };
        self.enqueue_output(event, bytes)
    }

    /// Persist a non-output event.  These events reserve capacity and wait for
    /// the writer's commit acknowledgement instead of being silently dropped.
    #[allow(dead_code)]
    pub fn record(&self, session_id: &str, kind: &str, payload: serde_json::Value) -> Result<()> {
        let record = store::EventRecord {
            seq: 0,
            id: Uuid::new_v4().to_string(),
            session_id: session_id.to_string(),
            kind: kind.to_string(),
            payload_json: serde_json::to_string(&payload)
                .context("failed to serialize event writer payload")?,
            created_at: crate::api::current_timestamp(),
        };
        let bytes = record
            .payload_json
            .len()
            .saturating_add(EVENT_OVERHEAD_BYTES);
        self.enqueue_critical(PendingEvent::Record(record), bytes)
    }

    /// Insert the terminal event only after every prior accepted event has
    /// committed.  The caller receives database failures synchronously.
    pub fn record_exit(&self, session_id: &str, result: PtyRunResult) -> Result<()> {
        let bytes = EVENT_OVERHEAD_BYTES;
        self.enqueue_critical(
            PendingEvent::Exit {
                session_id: session_id.to_string(),
                result,
                created_at: crate::api::current_timestamp(),
            },
            bytes,
        )
    }

    pub fn health(&self) -> EventWriterHealth {
        self.shared
            .health
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    fn enqueue_output(&self, event: PendingEvent, bytes: usize) -> Result<bool> {
        let mut queue = self.lock_queue();
        if let Some(error) = &queue.failed {
            return Err(anyhow!(error.clone()));
        }
        let (session_id, dropped_bytes, created_at) = match &event {
            PendingEvent::Output {
                session_id,
                data,
                created_at,
            } => (session_id, data.len(), created_at),
            _ => unreachable!("enqueue_output only accepts output events"),
        };
        if queue.closing_sessions.contains(session_id)
            || bytes > self.shared.output_capacity_bytes
            || queue.queued_bytes.saturating_add(bytes) > self.shared.output_capacity_bytes
        {
            record_output_drop(&mut queue, session_id, dropped_bytes, created_at);
            let mut health = self.lock_health();
            health.state = "pressured".to_string();
            health.dropped_output_events = health.dropped_output_events.saturating_add(1);
            health.dropped_output_bytes = health
                .dropped_output_bytes
                .saturating_add(dropped_bytes as u64);
            return Ok(false);
        }
        let marker = take_truncation_marker(&mut queue, session_id);
        let marker_bytes = marker.as_ref().map_or(0, |item| item.bytes);
        anyhow::ensure!(
            queue
                .queued_bytes
                .saturating_add(marker_bytes)
                .saturating_add(bytes)
                <= self.shared.capacity_bytes,
            "accepted output exceeded event writer capacity"
        );
        if let Some(marker) = marker {
            queue.queued_events += 1;
            queue.queued_bytes += marker.bytes;
            queue.items.push_back(marker);
        }
        queue.queued_events += 1;
        queue.queued_bytes += bytes;
        self.update_queue_health(queue.queued_events, queue.queued_bytes);
        queue.items.push_back(QueuedEvent {
            event,
            bytes,
            completion: None,
        });
        self.shared.available.notify_one();
        Ok(true)
    }

    fn enqueue_critical(&self, event: PendingEvent, bytes: usize) -> Result<()> {
        anyhow::ensure!(
            bytes <= self.shared.capacity_bytes,
            "critical event exceeds event writer capacity"
        );
        let session_id = event.session_id().to_string();
        let marker = {
            let mut queue = self.lock_queue();
            loop {
                if let Some(error) = &queue.failed {
                    return Err(anyhow!(error.clone()));
                }
                if queue.closing_sessions.insert(session_id.clone()) {
                    break take_truncation_marker(&mut queue, &session_id);
                }
                queue = self
                    .shared
                    .available
                    .wait(queue)
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
            }
        };

        let result = self.enqueue_closed_critical(event, bytes, marker);
        let mut queue = self.lock_queue();
        queue.closing_sessions.remove(&session_id);
        self.shared.available.notify_all();
        result
    }

    fn enqueue_closed_critical(
        &self,
        event: PendingEvent,
        bytes: usize,
        marker: Option<QueuedEvent>,
    ) -> Result<()> {
        let (completion_tx, completion_rx) = mpsc::sync_channel(1);
        let mut queue = self.lock_queue();
        if let Some(error) = &queue.failed {
            return Err(anyhow!(error.clone()));
        }
        let marker_bytes = marker.as_ref().map_or(0, |item| item.bytes);
        if marker_bytes.saturating_add(bytes) <= self.shared.capacity_bytes {
            loop {
                if let Some(error) = &queue.failed {
                    return Err(anyhow!(error.clone()));
                }
                if queue
                    .queued_bytes
                    .saturating_add(marker_bytes)
                    .saturating_add(bytes)
                    <= self.shared.capacity_bytes
                {
                    break;
                }
                queue = self
                    .shared
                    .available
                    .wait(queue)
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
            }

            if let Some(marker) = marker {
                queue.queued_events += 1;
                queue.queued_bytes += marker.bytes;
                queue.items.push_back(marker);
            }
            queue.queued_events += 1;
            queue.queued_bytes += bytes;
            self.update_queue_health(queue.queued_events, queue.queued_bytes);
            queue.items.push_back(QueuedEvent {
                event,
                bytes,
                completion: Some(completion_tx),
            });
            self.shared.available.notify_one();
            drop(queue);
            receive_completion(
                completion_rx,
                "event writer stopped before committing a critical event",
            )
        } else {
            let mut marker =
                marker.expect("marker is required when combined capacity is impossible");
            let (marker_tx, marker_rx) = mpsc::sync_channel(1);
            loop {
                if let Some(error) = &queue.failed {
                    return Err(anyhow!(error.clone()));
                }
                if queue.queued_bytes.saturating_add(marker.bytes) <= self.shared.capacity_bytes {
                    break;
                }
                queue = self
                    .shared
                    .available
                    .wait(queue)
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
            }
            marker.completion = Some(marker_tx);
            queue.queued_events += 1;
            queue.queued_bytes += marker.bytes;
            queue.items.push_back(marker);
            self.update_queue_health(queue.queued_events, queue.queued_bytes);
            self.shared.available.notify_one();
            drop(queue);
            receive_completion(
                marker_rx,
                "event writer stopped before committing truncation marker",
            )?;
            self.enqueue_closed_critical_event(event, bytes)
        }
    }

    fn enqueue_closed_critical_event(&self, event: PendingEvent, bytes: usize) -> Result<()> {
        let (completion_tx, completion_rx) = mpsc::sync_channel(1);
        let mut queue = self.lock_queue();
        loop {
            if let Some(error) = &queue.failed {
                return Err(anyhow!(error.clone()));
            }
            if queue.queued_bytes.saturating_add(bytes) <= self.shared.capacity_bytes {
                break;
            }
            queue = self
                .shared
                .available
                .wait(queue)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
        }
        queue.queued_events += 1;
        queue.queued_bytes += bytes;
        self.update_queue_health(queue.queued_events, queue.queued_bytes);
        queue.items.push_back(QueuedEvent {
            event,
            bytes,
            completion: Some(completion_tx),
        });
        self.shared.available.notify_one();
        drop(queue);
        receive_completion(
            completion_rx,
            "event writer stopped before committing a critical event",
        )
    }

    fn lock_queue(&self) -> std::sync::MutexGuard<'_, Queue> {
        self.shared
            .queue
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn lock_health(&self) -> std::sync::MutexGuard<'_, EventWriterHealth> {
        self.shared
            .health
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn update_queue_health(&self, events: usize, bytes: usize) {
        let mut health = self.lock_health();
        health.queued_events = events;
        health.queued_bytes = bytes;
    }
}

fn run_worker(
    shared: Arc<Shared>,
    coven_home: PathBuf,
    ready: mpsc::SyncSender<std::result::Result<(), String>>,
) {
    let path = coven_home.join(STORE_FILE_NAME);
    let mut conn = match store::open_initialized_store(&path) {
        Ok(conn) => {
            let mut health = lock_health(&shared);
            health.connection_opens = 1;
            let _ = ready.send(Ok(()));
            conn
        }
        Err(error) => {
            let message = format!("failed to open daemon event writer: {error:#}");
            let _ = fail_writer(&shared, message.clone());
            let _ = ready.send(Err(message));
            return;
        }
    };

    loop {
        let batch = take_batch(&shared);
        let bytes: usize = batch.iter().map(|item| item.bytes).sum();
        let result = retry_transient_sqlite_lock(
            || commit_batch(&mut conn, &coven_home, &batch),
            SQLITE_LOCK_COMMIT_ATTEMPTS,
            SQLITE_LOCK_RETRY_DELAY,
        );
        match result {
            Ok(committed) => {
                release_capacity(&shared, batch.len(), bytes);
                let mut health = lock_health(&shared);
                health.transactions += 1;
                health.committed_events += committed as u64;
                drop(health);
                complete(&batch, Ok(()));
            }
            Err(error) => {
                let message = format!("event writer commit failed: {error:#}");
                // Latch failure before releasing capacity. Otherwise a producer
                // can enqueue a critical event in the release/failure window and
                // wait forever after this worker exits.
                let pending = fail_writer(&shared, message.clone());
                complete(&batch, Err(message));
                complete(
                    &pending,
                    Err("event writer stopped after a commit failure".to_string()),
                );
                return;
            }
        }
    }
}

fn retry_transient_sqlite_lock<T>(
    mut operation: impl FnMut() -> Result<T>,
    attempts: usize,
    retry_delay: Duration,
) -> Result<T> {
    assert!(attempts > 0, "SQLite retry attempts must be non-zero");
    for attempt in 0..attempts {
        match operation() {
            Ok(value) => return Ok(value),
            Err(error) if is_transient_sqlite_lock(&error) && attempt + 1 < attempts => {
                thread::sleep(retry_delay);
            }
            Err(error) => return Err(error),
        }
    }
    unreachable!("SQLite retry loop either succeeds or returns its final error")
}

fn is_transient_sqlite_lock(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause
            .downcast_ref::<rusqlite::Error>()
            .and_then(rusqlite::Error::sqlite_error_code)
            .is_some_and(|code| matches!(code, ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked))
    })
}

fn take_batch(shared: &Arc<Shared>) -> Vec<QueuedEvent> {
    let mut queue = lock_queue(shared);
    while queue.items.is_empty() {
        queue = shared
            .available
            .wait(queue)
            .unwrap_or_else(|poisoned| poisoned.into_inner());
    }
    let mut batch = vec![queue.items.pop_front().expect("queue was checked")];
    let deadline = Instant::now() + COALESCE_WINDOW;
    while batch.len() < MAX_BATCH_EVENTS {
        if let Some(item) = queue.items.pop_front() {
            batch.push(item);
            continue;
        }
        let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
            break;
        };
        let (next, timeout) = shared
            .available
            .wait_timeout(queue, remaining)
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        queue = next;
        if timeout.timed_out() {
            break;
        }
    }
    batch
}

fn commit_batch(
    conn: &mut Connection,
    coven_home: &std::path::Path,
    batch: &[QueuedEvent],
) -> Result<usize> {
    let transaction = conn
        .transaction()
        .context("failed to begin event writer transaction")?;
    let mut committed = 0;
    let mut output: Option<store::EventRecord> = None;
    for item in batch {
        match &item.event {
            PendingEvent::Output {
                session_id,
                data,
                created_at,
            } => {
                if let Some(pending) = output
                    .as_mut()
                    .filter(|pending| pending.session_id == *session_id)
                {
                    let mut payload: serde_json::Value =
                        serde_json::from_str(&pending.payload_json)
                            .context("event writer output payload was not JSON")?;
                    let existing = payload
                        .get_mut("data")
                        .and_then(|value| value.as_str())
                        .unwrap_or_default()
                        .to_string();
                    payload["data"] = serde_json::Value::String(format!("{existing}{data}"));
                    pending.payload_json =
                        serde_json::to_string(&payload).context("failed to coalesce raw output")?;
                } else {
                    flush_output(&transaction, coven_home, &mut output, &mut committed)?;
                    output = Some(output_record(session_id, data, created_at)?);
                }
            }
            PendingEvent::Record(record) => {
                flush_output(&transaction, coven_home, &mut output, &mut committed)?;
                store::insert_event_with_privacy(&transaction, coven_home, record)?;
                committed += 1;
            }
            PendingEvent::Exit {
                session_id,
                result,
                created_at,
            } => {
                flush_output(&transaction, coven_home, &mut output, &mut committed)?;
                record_exit(&transaction, coven_home, session_id, result, created_at)?;
                committed += 1;
            }
        }
    }
    flush_output(&transaction, coven_home, &mut output, &mut committed)?;
    transaction
        .commit()
        .context("failed to commit event writer transaction")?;
    Ok(committed)
}

fn output_record(session_id: &str, data: &str, created_at: &str) -> Result<store::EventRecord> {
    Ok(store::EventRecord {
        seq: 0,
        id: Uuid::new_v4().to_string(),
        session_id: session_id.to_string(),
        kind: "output".to_string(),
        payload_json: serde_json::to_string(&json!({ "data": data }))
            .context("failed to serialize raw output")?,
        created_at: created_at.to_string(),
    })
}

fn record_output_drop(queue: &mut Queue, session_id: &str, dropped_bytes: usize, created_at: &str) {
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
    let payload_json = serde_json::to_string(&json!({
        "droppedEvents": truncation.dropped_events,
        "droppedBytes": truncation.dropped_bytes,
    }))
    .expect("truncation marker payload is always serializable");
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

fn flush_output(
    conn: &Connection,
    coven_home: &std::path::Path,
    output: &mut Option<store::EventRecord>,
    committed: &mut usize,
) -> Result<()> {
    if let Some(record) = output.take() {
        store::insert_event_with_privacy(conn, coven_home, &record)?;
        *committed += 1;
    }
    Ok(())
}

fn record_exit(
    conn: &Connection,
    coven_home: &std::path::Path,
    session_id: &str,
    result: &PtyRunResult,
    created_at: &str,
) -> Result<()> {
    if let Some(session) = store::get_session(conn, session_id)? {
        if session.status == "running" {
            let persisted_status =
                if session.conversation_id.is_some() && result.status == "completed" {
                    "idle"
                } else {
                    result.status
                };
            store::update_session_status_if_current(
                conn,
                session_id,
                "running",
                persisted_status,
                result.exit_code,
                created_at,
            )?;
        }
    }
    store::insert_event_with_privacy(
        conn,
        coven_home,
        &store::EventRecord {
            seq: 0,
            id: Uuid::new_v4().to_string(),
            session_id: session_id.to_string(),
            kind: "exit".to_string(),
            payload_json: serde_json::to_string(&json!({
                "status": result.status,
                "exitCode": result.exit_code,
            }))
            .context("failed to serialize exit event payload")?,
            created_at: created_at.to_string(),
        },
    )
}

fn release_capacity(shared: &Arc<Shared>, events: usize, bytes: usize) {
    let mut queue = lock_queue(shared);
    queue.queued_events = queue.queued_events.saturating_sub(events);
    queue.queued_bytes = queue.queued_bytes.saturating_sub(bytes);
    let mut health = lock_health(shared);
    health.queued_events = queue.queued_events;
    health.queued_bytes = queue.queued_bytes;
    shared.available.notify_all();
}

fn complete(batch: &[QueuedEvent], result: std::result::Result<(), String>) {
    for item in batch {
        if let Some(completion) = &item.completion {
            let _ = completion.send(result.clone());
        }
    }
}

fn receive_completion(
    receiver: mpsc::Receiver<std::result::Result<(), String>>,
    context: &'static str,
) -> Result<()> {
    match receiver.recv().context(context)? {
        Ok(()) => Ok(()),
        Err(message) => Err(anyhow!(message)),
    }
}

fn fail_writer(shared: &Arc<Shared>, message: String) -> Vec<QueuedEvent> {
    let mut queue = lock_queue(shared);
    queue.failed = Some(message.clone());
    queue.truncations.clear();
    queue.closing_sessions.clear();
    queue.queued_events = 0;
    queue.queued_bytes = 0;
    let pending = queue.items.drain(..).collect();
    let mut health = lock_health(shared);
    health.state = "failed".to_string();
    health.queued_events = 0;
    health.queued_bytes = 0;
    health.last_error = Some(message);
    shared.available.notify_all();
    pending
}

fn lock_queue(shared: &Arc<Shared>) -> std::sync::MutexGuard<'_, Queue> {
    shared
        .queue
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn lock_health(shared: &Arc<Shared>) -> std::sync::MutexGuard<'_, EventWriterHealth> {
    shared
        .health
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn health_deserializes_payload_without_queued_event_count() -> Result<()> {
        let health: EventWriterHealth = serde_json::from_value(serde_json::json!({
            "state": "healthy",
            "queuedBytes": 0,
            "capacityBytes": DEFAULT_CAPACITY_BYTES,
            "droppedOutputEvents": 0,
            "droppedOutputBytes": 0,
            "connectionOpens": 1,
            "transactions": 2,
            "committedEvents": 3
        }))?;

        assert_eq!(health.queued_events, 0);
        Ok(())
    }

    #[test]
    fn transient_sqlite_lock_is_retried() -> Result<()> {
        let home = tempfile::tempdir()?;
        let path = home.path().join("retry.sqlite");
        let locker = Connection::open(&path)?;
        locker.execute_batch(
            "CREATE TABLE events (id INTEGER PRIMARY KEY);
             BEGIN IMMEDIATE;
             INSERT INTO events DEFAULT VALUES;",
        )?;
        let contender = Connection::open(&path)?;
        contender.busy_timeout(Duration::ZERO)?;
        let mut attempts = 0;

        let inserted = retry_transient_sqlite_lock(
            || {
                attempts += 1;
                if attempts == 2 {
                    locker.execute_batch("ROLLBACK")?;
                }
                contender
                    .execute("INSERT INTO events DEFAULT VALUES", [])
                    .map_err(Into::into)
            },
            3,
            Duration::ZERO,
        )?;

        assert_eq!(inserted, 1);
        assert_eq!(attempts, 2);
        Ok(())
    }

    #[test]
    fn non_lock_sqlite_error_is_not_retried() -> Result<()> {
        let conn = Connection::open_in_memory()?;
        let mut attempts = 0;

        let error = retry_transient_sqlite_lock(
            || {
                attempts += 1;
                conn.execute("INSERT INTO missing_table DEFAULT VALUES", [])
                    .map_err(Into::into)
            },
            3,
            Duration::ZERO,
        )
        .unwrap_err();

        assert_eq!(attempts, 1);
        assert!(format!("{error:#}").contains("no such table"));
        Ok(())
    }

    #[test]
    fn persistent_sqlite_lock_stops_after_attempt_limit() -> Result<()> {
        let home = tempfile::tempdir()?;
        let path = home.path().join("persistent-lock.sqlite");
        let locker = Connection::open(&path)?;
        locker.execute_batch(
            "CREATE TABLE events (id INTEGER PRIMARY KEY);
             BEGIN IMMEDIATE;
             INSERT INTO events DEFAULT VALUES;",
        )?;
        let contender = Connection::open(&path)?;
        contender.busy_timeout(Duration::ZERO)?;
        let mut attempts = 0;

        let error = retry_transient_sqlite_lock(
            || {
                attempts += 1;
                contender
                    .execute("INSERT INTO events DEFAULT VALUES", [])
                    .map_err(Into::into)
            },
            3,
            Duration::ZERO,
        )
        .unwrap_err();

        assert_eq!(attempts, 3);
        assert!(is_transient_sqlite_lock(&error));
        Ok(())
    }

    #[test]
    fn queue_health_counts_events_until_completion() {
        let shared = Arc::new(Shared {
            queue: Mutex::new(Queue {
                items: VecDeque::new(),
                queued_events: 2,
                queued_bytes: EVENT_OVERHEAD_BYTES * 2,
                failed: None,
                truncations: HashMap::new(),
                closing_sessions: HashSet::new(),
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

    #[test]
    fn accepted_output_updates_queue_health_immediately() -> Result<()> {
        let shared = Arc::new(Shared {
            queue: Mutex::new(Queue {
                items: VecDeque::new(),
                queued_events: 0,
                queued_bytes: 0,
                failed: None,
                truncations: HashMap::new(),
                closing_sessions: HashSet::new(),
            }),
            available: Condvar::new(),
            capacity_bytes: DEFAULT_CAPACITY_BYTES,
            output_capacity_bytes: DEFAULT_CAPACITY_BYTES - RESERVED_CRITICAL_BYTES,
            health: Mutex::new(EventWriterHealth {
                state: "healthy".to_string(),
                queued_events: 0,
                queued_bytes: 0,
                capacity_bytes: DEFAULT_CAPACITY_BYTES,
                dropped_output_events: 0,
                dropped_output_bytes: 0,
                connection_opens: 0,
                transactions: 0,
                committed_events: 0,
                last_error: None,
            }),
        });
        let writer = EventWriter {
            shared: Arc::clone(&shared),
        };

        assert!(writer.enqueue_output(
            PendingEvent::Output {
                session_id: "s-1".to_string(),
                data: "hello".to_string(),
                created_at: "2026-08-04T00:00:00Z".to_string(),
            },
            EVENT_OVERHEAD_BYTES + "hello".len()
        )?);

        let queue = lock_queue(&shared);
        assert_eq!(queue.queued_events, 1);
        assert_eq!(queue.queued_bytes, EVENT_OVERHEAD_BYTES + "hello".len());
        drop(queue);

        let health = lock_health(&shared);
        assert_eq!(health.queued_events, 1);
        assert_eq!(health.queued_bytes, EVENT_OVERHEAD_BYTES + "hello".len());
        Ok(())
    }

    #[test]
    fn critical_boundary_prevents_same_session_output_from_overtaking_marker() -> Result<()> {
        let capacity = RESERVED_CRITICAL_BYTES + 1024;
        let blocked_bytes = capacity - 256;
        let shared = Arc::new(Shared {
            queue: Mutex::new(Queue {
                items: VecDeque::from([QueuedEvent {
                    event: PendingEvent::Output {
                        session_id: "other".to_string(),
                        data: "blocked".to_string(),
                        created_at: "2026-08-04T00:00:00Z".to_string(),
                    },
                    bytes: blocked_bytes,
                    completion: None,
                }]),
                queued_events: 1,
                queued_bytes: blocked_bytes,
                failed: None,
                truncations: HashMap::from([(
                    "s-1".to_string(),
                    OutputTruncation {
                        dropped_events: 1,
                        dropped_bytes: 2048,
                        created_at: "2026-08-04T00:00:01Z".to_string(),
                    },
                )]),
                closing_sessions: HashSet::new(),
            }),
            available: Condvar::new(),
            capacity_bytes: capacity,
            output_capacity_bytes: capacity - RESERVED_CRITICAL_BYTES,
            health: Mutex::new(EventWriterHealth {
                state: "pressured".to_string(),
                queued_events: 1,
                queued_bytes: blocked_bytes,
                capacity_bytes: capacity,
                dropped_output_events: 1,
                dropped_output_bytes: 2048,
                connection_opens: 0,
                transactions: 0,
                committed_events: 0,
                last_error: None,
            }),
        });
        let writer = EventWriter {
            shared: Arc::clone(&shared),
        };
        let critical_writer = writer.clone();
        let critical = thread::spawn(move || {
            critical_writer.enqueue_critical(
                PendingEvent::Record(store::EventRecord {
                    seq: 0,
                    id: "critical".to_string(),
                    session_id: "s-1".to_string(),
                    kind: "tool_result".to_string(),
                    payload_json: "{}".to_string(),
                    created_at: "2026-08-04T00:00:02Z".to_string(),
                }),
                EVENT_OVERHEAD_BYTES,
            )
        });

        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            if lock_queue(&shared).closing_sessions.contains("s-1") {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "critical event did not claim its session boundary"
            );
            thread::sleep(Duration::from_millis(1));
        }

        assert!(!writer.enqueue_output(
            PendingEvent::Output {
                session_id: "s-1".to_string(),
                data: "later".to_string(),
                created_at: "2026-08-04T00:00:03Z".to_string(),
            },
            EVENT_OVERHEAD_BYTES + "later".len(),
        )?);

        {
            let mut queue = lock_queue(&shared);
            queue.items.clear();
            queue.queued_events = 0;
            queue.queued_bytes = 0;
            shared.available.notify_all();
        }

        let queued = loop {
            let mut queue = lock_queue(&shared);
            if queue.items.len() == 2 {
                queue.queued_events = 0;
                queue.queued_bytes = 0;
                break queue.items.drain(..).collect::<Vec<_>>();
            }
            drop(queue);
            assert!(
                Instant::now() < deadline,
                "critical boundary events were not queued"
            );
            thread::sleep(Duration::from_millis(1));
        };

        assert_eq!(
            queued
                .iter()
                .map(|item| match &item.event {
                    PendingEvent::Record(record) => record.kind.as_str(),
                    _ => "unexpected",
                })
                .collect::<Vec<_>>(),
            ["output_truncated", "tool_result"]
        );
        let marker = match &queued[0].event {
            PendingEvent::Record(record) => {
                serde_json::from_str::<serde_json::Value>(&record.payload_json)?
            }
            _ => unreachable!("first boundary event must be the truncation marker"),
        };
        assert_eq!(marker["droppedEvents"], 1);
        assert_eq!(marker["droppedBytes"], 2048);
        let queue = lock_queue(&shared);
        let later_episode = queue
            .truncations
            .get("s-1")
            .expect("output after the claimed boundary starts the next episode");
        assert_eq!(later_episode.dropped_events, 1);
        assert_eq!(later_episode.dropped_bytes, 5);
        drop(queue);

        complete(&queued, Ok(()));
        critical.join().expect("critical producer panicked")?;
        assert!(!lock_queue(&shared).closing_sessions.contains("s-1"));
        Ok(())
    }

    fn session(id: &str) -> store::SessionRecord {
        store::SessionRecord {
            id: id.to_string(),
            project_root: "/repo".to_string(),
            harness: "codex".to_string(),
            title: id.to_string(),
            status: "running".to_string(),
            exit_code: None,
            archived_at: None,
            created_at: "2026-08-04T00:00:00Z".to_string(),
            updated_at: "2026-08-04T00:00:00Z".to_string(),
            conversation_id: None,
            familiar_id: None,
            labels: Vec::new(),
            visibility: "private".to_string(),
            external: false,
            transcript_path: None,
        }
    }

    #[test]
    fn coalesces_output_and_flushes_it_before_exit() -> Result<()> {
        let home = tempfile::tempdir()?;
        let conn = store::open_store(&home.path().join(STORE_FILE_NAME))?;
        store::insert_session(&conn, &session("s-1"))?;
        let writer = EventWriter::start(home.path().to_path_buf())?;
        assert!(writer.record_output("s-1", "hello ".to_string())?);
        assert!(writer.record_output("s-1", "world".to_string())?);
        writer.record_exit(
            "s-1",
            PtyRunResult {
                status: "completed",
                exit_code: Some(0),
            },
        )?;
        let events = store::list_events(&conn, "s-1")?;
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].kind, "output");
        assert!(events[0].payload_json.contains("hello world"));
        assert_eq!(events[1].kind, "exit");
        assert_eq!(
            store::get_session(&conn, "s-1")?.unwrap().status,
            "completed"
        );
        Ok(())
    }

    #[test]
    fn flushes_live_output_after_the_coalesce_window() -> Result<()> {
        let home = tempfile::tempdir()?;
        let conn = store::open_store(&home.path().join(STORE_FILE_NAME))?;
        store::insert_session(&conn, &session("s-1"))?;
        let writer = EventWriter::start(home.path().to_path_buf())?;
        assert!(writer.record_output("s-1", "still running".to_string())?);

        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            let events = store::list_events(&conn, "s-1")?;
            if events.iter().any(|event| event.kind == "output") {
                break;
            }
            anyhow::ensure!(Instant::now() < deadline, "live output did not flush");
            thread::sleep(Duration::from_millis(10));
        }
        Ok(())
    }

    #[test]
    fn one_connection_and_batched_transactions_handle_noisy_output() -> Result<()> {
        let home = tempfile::tempdir()?;
        let conn = store::open_store(&home.path().join(STORE_FILE_NAME))?;
        for id in 0..8 {
            store::insert_session(&conn, &session(&format!("s-{id}")))?;
        }
        let writer = EventWriter::start_with_capacity(home.path().to_path_buf(), 512 * 1024)?;
        for index in 0..128 {
            assert!(writer.record_output(&format!("s-{}", index % 8), "x".repeat(256))?);
        }
        for id in 0..8 {
            writer.record_exit(
                &format!("s-{id}"),
                PtyRunResult {
                    status: "completed",
                    exit_code: Some(0),
                },
            )?;
        }
        let health = writer.health();
        assert_eq!(health.connection_opens, 1);
        // The old path opened a connection and committed once per callback
        // (136 times here).  This stays well below the issue's 80% reduction
        // target even though terminal events synchronously acknowledge.
        assert!(
            health.transactions <= 16,
            "expected batching, got {health:?}"
        );
        assert_eq!(health.dropped_output_events, 0);
        Ok(())
    }

    #[test]
    fn pressure_is_visible_when_raw_output_exceeds_its_budget() -> Result<()> {
        let home = tempfile::tempdir()?;
        let conn = store::open_store(&home.path().join(STORE_FILE_NAME))?;
        store::insert_session(&conn, &session("s-1"))?;
        let writer = EventWriter::start_with_capacity(
            home.path().to_path_buf(),
            RESERVED_CRITICAL_BYTES + 1024,
        )?;
        assert!(!writer.record_output("s-1", "x".repeat(2048))?);
        let health = writer.health();
        assert_eq!(health.state, "pressured");
        assert_eq!(health.dropped_output_events, 1);
        assert_eq!(health.queued_events, 0);
        assert_eq!(health.queued_bytes, 0);
        writer.record_exit(
            "s-1",
            PtyRunResult {
                status: "failed",
                exit_code: Some(1),
            },
        )?;
        Ok(())
    }

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
        assert_eq!(
            events
                .iter()
                .map(|event| event.kind.as_str())
                .collect::<Vec<_>>(),
            ["output_truncated", "exit"]
        );
        let marker_payload: serde_json::Value = serde_json::from_str(&events[0].payload_json)?;
        assert_eq!(marker_payload["droppedEvents"], 1);
        assert_eq!(marker_payload["droppedBytes"], 2048);
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

        assert!(!writer.record_output("s-1", "x".repeat(2048))?);
        assert!(!writer.record_output("s-2", "x".repeat(3072))?);
        writer.record_exit(
            "s-1",
            PtyRunResult {
                status: "completed",
                exit_code: Some(0),
            },
        )?;
        writer.record_exit(
            "s-2",
            PtyRunResult {
                status: "completed",
                exit_code: Some(0),
            },
        )?;

        for (session_id, dropped_bytes) in [("s-1", 2048), ("s-2", 3072)] {
            let events = store::list_events(&conn, session_id)?;
            assert_eq!(events[0].kind, "output_truncated");
            let marker_payload: serde_json::Value = serde_json::from_str(&events[0].payload_json)?;
            assert_eq!(marker_payload["droppedEvents"], 1);
            assert_eq!(marker_payload["droppedBytes"], dropped_bytes);
            assert_eq!(events[1].kind, "exit");
        }
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
        let record = store::EventRecord {
            seq: 0,
            id: "event".to_string(),
            session_id: "s-1".to_string(),
            kind: "error".to_string(),
            payload_json: "{}".to_string(),
            created_at: "2026-08-04T00:00:00Z".to_string(),
        };
        writer.enqueue_critical(PendingEvent::Record(record), capacity - 1)?;

        let events = store::list_events(&conn, "s-1")?;
        assert_eq!(
            events
                .iter()
                .map(|event| event.kind.as_str())
                .collect::<Vec<_>>(),
            ["output_truncated", "error"]
        );
        Ok(())
    }

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
        assert!(!writer.record_output("s-1", "x".repeat(3072))?);
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
            events
                .iter()
                .map(|event| event.kind.as_str())
                .collect::<Vec<_>>(),
            ["output_truncated", "output", "exit"]
        );
        let marker_payload: serde_json::Value = serde_json::from_str(&events[0].payload_json)?;
        assert_eq!(marker_payload["droppedEvents"], 2);
        assert_eq!(marker_payload["droppedBytes"], 5120);
        assert!(events[0].created_at <= events[1].created_at);
        Ok(())
    }

    #[test]
    fn accepted_output_without_pressure_has_no_truncation_marker() -> Result<()> {
        let home = tempfile::tempdir()?;
        let conn = store::open_store(&home.path().join(STORE_FILE_NAME))?;
        store::insert_session(&conn, &session("s-1"))?;
        let writer = EventWriter::start_with_capacity(
            home.path().to_path_buf(),
            RESERVED_CRITICAL_BYTES + 1024,
        )?;

        assert!(writer.record_output("s-1", "accepted".to_string())?);
        writer.record_exit(
            "s-1",
            PtyRunResult {
                status: "completed",
                exit_code: Some(0),
            },
        )?;

        let events = store::list_events(&conn, "s-1")?;
        assert_eq!(
            events
                .iter()
                .map(|event| event.kind.as_str())
                .collect::<Vec<_>>(),
            ["output", "exit"]
        );
        Ok(())
    }

    #[test]
    fn writer_failure_drains_queued_critical_completions() {
        let shared = Arc::new(Shared {
            queue: Mutex::new(Queue {
                items: VecDeque::new(),
                queued_events: 1,
                queued_bytes: EVENT_OVERHEAD_BYTES,
                failed: None,
                truncations: HashMap::new(),
                closing_sessions: HashSet::new(),
            }),
            available: Condvar::new(),
            capacity_bytes: DEFAULT_CAPACITY_BYTES,
            output_capacity_bytes: DEFAULT_CAPACITY_BYTES - RESERVED_CRITICAL_BYTES,
            health: Mutex::new(EventWriterHealth {
                state: "healthy".to_string(),
                queued_events: 1,
                queued_bytes: EVENT_OVERHEAD_BYTES,
                capacity_bytes: DEFAULT_CAPACITY_BYTES,
                dropped_output_events: 0,
                dropped_output_bytes: 0,
                connection_opens: 0,
                transactions: 0,
                committed_events: 0,
                last_error: None,
            }),
        });
        let (completion, receiver) = mpsc::sync_channel(1);
        lock_queue(&shared).items.push_back(QueuedEvent {
            event: PendingEvent::Record(store::EventRecord {
                seq: 0,
                id: "event".to_string(),
                session_id: "s-1".to_string(),
                kind: "error".to_string(),
                payload_json: "{}".to_string(),
                created_at: "2026-08-04T00:00:00Z".to_string(),
            }),
            bytes: EVENT_OVERHEAD_BYTES,
            completion: Some(completion),
        });

        let pending = fail_writer(&shared, "simulated failure".to_string());
        complete(
            &pending,
            Err("event writer stopped after a commit failure".to_string()),
        );

        assert!(receiver
            .recv_timeout(Duration::from_secs(1))
            .unwrap()
            .is_err());
        assert!(lock_queue(&shared).items.is_empty());
        let health = lock_health(&shared);
        assert_eq!(health.state, "failed");
        assert_eq!(health.queued_events, 0);
        assert_eq!(health.queued_bytes, 0);
    }
}
