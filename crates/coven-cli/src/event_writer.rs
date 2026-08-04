//! Bounded, daemon-owned persistence for high-volume live-session events.
//!
//! PTY drains must never open a SQLite connection for every raw read.  This
//! module owns one connection on one worker thread, accepts a byte-bounded
//! stream of events, and commits short batches.  Terminal events reserve space
//! and wait for a commit acknowledgement, so an accepted output chunk cannot
//! be overtaken by a following exit event.

use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::{mpsc, Arc, Condvar, Mutex},
    thread,
    time::{Duration, Instant},
};

use anyhow::{anyhow, Context, Result};
use rusqlite::Connection;
use serde::Serialize;
use serde_json::json;
use uuid::Uuid;

use crate::{pty_runner::PtyRunResult, store, STORE_FILE_NAME};

const DEFAULT_CAPACITY_BYTES: usize = 2 * 1024 * 1024;
const RESERVED_CRITICAL_BYTES: usize = 128 * 1024;
const EVENT_OVERHEAD_BYTES: usize = 512;
const MAX_BATCH_EVENTS: usize = 64;
const COALESCE_WINDOW: Duration = Duration::from_millis(12);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EventWriterHealth {
    /// `healthy`, `pressured`, or `failed`.  Pressure remains visible for the
    /// daemon lifetime so a rejected raw chunk is never silently forgotten.
    pub state: String,
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
    queued_bytes: usize,
    failed: Option<String>,
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
                queued_bytes: 0,
                failed: None,
            }),
            available: Condvar::new(),
            capacity_bytes,
            output_capacity_bytes: capacity_bytes - RESERVED_CRITICAL_BYTES,
            health: Mutex::new(EventWriterHealth {
                state: "healthy".to_string(),
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
        if bytes > self.shared.output_capacity_bytes
            || queue.queued_bytes.saturating_add(bytes) > self.shared.output_capacity_bytes
        {
            let mut health = self.lock_health();
            health.state = "pressured".to_string();
            health.dropped_output_events += 1;
            health.dropped_output_bytes += bytes.saturating_sub(EVENT_OVERHEAD_BYTES) as u64;
            return Ok(false);
        }
        queue.queued_bytes += bytes;
        self.update_queued_bytes(queue.queued_bytes);
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
        queue.queued_bytes += bytes;
        self.update_queued_bytes(queue.queued_bytes);
        queue.items.push_back(QueuedEvent {
            event,
            bytes,
            completion: Some(completion_tx),
        });
        self.shared.available.notify_one();
        drop(queue);
        match completion_rx
            .recv()
            .context("event writer stopped before committing a critical event")?
        {
            Ok(()) => Ok(()),
            Err(message) => Err(anyhow!(message)),
        }
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

    fn update_queued_bytes(&self, bytes: usize) {
        self.lock_health().queued_bytes = bytes;
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
        let result = commit_batch(&mut conn, &coven_home, &batch);
        match result {
            Ok(committed) => {
                release_bytes(&shared, bytes);
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

fn release_bytes(shared: &Arc<Shared>, bytes: usize) {
    let mut queue = lock_queue(shared);
    queue.queued_bytes = queue.queued_bytes.saturating_sub(bytes);
    lock_health(shared).queued_bytes = queue.queued_bytes;
    shared.available.notify_all();
}

fn complete(batch: &[QueuedEvent], result: std::result::Result<(), String>) {
    for item in batch {
        if let Some(completion) = &item.completion {
            let _ = completion.send(result.clone());
        }
    }
}

fn fail_writer(shared: &Arc<Shared>, message: String) -> Vec<QueuedEvent> {
    let mut queue = lock_queue(shared);
    queue.failed = Some(message.clone());
    queue.queued_bytes = 0;
    let pending = queue.items.drain(..).collect();
    let mut health = lock_health(shared);
    health.state = "failed".to_string();
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
    fn writer_failure_drains_queued_critical_completions() {
        let shared = Arc::new(Shared {
            queue: Mutex::new(Queue {
                items: VecDeque::new(),
                queued_bytes: EVENT_OVERHEAD_BYTES,
                failed: None,
            }),
            available: Condvar::new(),
            capacity_bytes: DEFAULT_CAPACITY_BYTES,
            output_capacity_bytes: DEFAULT_CAPACITY_BYTES - RESERVED_CRITICAL_BYTES,
            health: Mutex::new(EventWriterHealth {
                state: "healthy".to_string(),
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
        assert_eq!(health.queued_bytes, 0);
    }
}
