//! Routine run delivery and settlement (coven#816).
//!
//! Dispatch records that a run *started*; this module finishes it. The
//! reconciliation pass watches every running ledger row, reads the terminal
//! outcome from the Coven session store (the same normalized stream every
//! session produces), captures a bounded log, and — when the definition
//! configures an output target — atomically commits the final assistant
//! payload. Coven, not the model, performs the delivery, and a failed output
//! commit fails the run visibly instead of reporting success.

use std::path::{Path, PathBuf};
use std::{fs::OpenOptions, io::Write};

use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::runs::{record_run_finish, RunFinish, LOG_ENTRY_MAX_CHARS};

/// How many trailing normalized-stream events a bounded log captures. The
/// budget keeps the newest events; older entries are replaced by a marker.
const BOUNDED_LOG_EVENT_LIMIT: usize = 200;

/// Terminal session statuses, mirroring
/// `store::update_session_terminal_if_active`.
const SESSION_TERMINAL_STATUSES: [&str; 6] = [
    "completed",
    "failed",
    "cancelled",
    "killed",
    "idle",
    "orphaned",
];

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ReconcileReport {
    pub settled_succeeded: usize,
    pub settled_failed: usize,
    pub still_running: usize,
    pub failures: Vec<String>,
}

fn is_terminal_session_status(status: &str) -> bool {
    SESSION_TERMINAL_STATUSES.contains(&status)
}

struct StreamEvent {
    kind: String,
    payload: Value,
    created_at: String,
}

/// Reads the trailing normalized stream for one session, newest last.
fn read_stream_tail(
    conn: &Connection,
    session_id: &str,
    limit: usize,
) -> Result<Vec<StreamEvent>, String> {
    let mut statement = conn
        .prepare(
            "SELECT kind, payload_json, created_at FROM events
             WHERE session_id = ?1
             ORDER BY rowid DESC
             LIMIT ?2",
        )
        .map_err(|error| format!("failed to read session stream: {error}"))?;
    let rows = statement
        .query_map(params![session_id, limit as i64], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .map_err(|error| format!("failed to read session stream: {error}"))?;
    let mut events = Vec::new();
    for row in rows {
        let (kind, payload_json, created_at) =
            row.map_err(|error| format!("failed to read session stream: {error}"))?;
        let payload = serde_json::from_str(&payload_json).unwrap_or(Value::Null);
        events.push(StreamEvent {
            kind,
            payload,
            created_at,
        });
    }
    events.reverse();
    Ok(events)
}

/// Captures a bounded JSON log of the session's normalized stream: the newest
/// entries that fit the ledger's per-run budget, prefixed by a truncation
/// marker when older entries were dropped. `None` when the session recorded
/// nothing.
pub fn capture_bounded_log(conn: &Connection, session_id: &str) -> Option<String> {
    let events = read_stream_tail(conn, session_id, BOUNDED_LOG_EVENT_LIMIT).ok()?;
    if events.is_empty() {
        return None;
    }
    let entries: Vec<Value> = events
        .iter()
        .map(|event| {
            json!({
                "kind": event.kind,
                "createdAt": event.created_at,
                "payload": event.payload,
            })
        })
        .collect();

    // Keep the tail within the character budget: walk newest-first and stop
    // at the first entry that no longer fits.
    let mut kept: Vec<&Value> = Vec::new();
    let mut budget = LOG_ENTRY_MAX_CHARS;
    for entry in entries.iter().rev() {
        let entry_chars = entry.to_string().chars().count() + 1;
        if entry_chars > budget {
            break;
        }
        budget -= entry_chars;
        kept.push(entry);
    }
    kept.reverse();
    let dropped = entries.len() - kept.len();

    let mut log_entries = Vec::with_capacity(kept.len() + 1);
    if dropped > 0 {
        log_entries.push(json!({
            "kind": "logTruncated",
            "droppedEntries": dropped,
        }));
    }
    if kept.is_empty() && dropped == 0 {
        return None;
    }
    for entry in &kept {
        log_entries.push((*entry).clone());
    }
    serde_json::to_string(&log_entries).ok()
}

/// The complete ordered assistant payload of a session. Event-writer batches
/// coalesce adjacent chunks, but a long response may span many batches, so
/// delivery reconstructs every `output` event without the bounded-log tail
/// limit. `None` when the session produced no output.
pub fn final_output_text(conn: &Connection, session_id: &str) -> Option<String> {
    let mut statement = conn
        .prepare(
            "SELECT payload_json FROM events
             WHERE session_id = ?1 AND kind = 'output'
             ORDER BY rowid ASC",
        )
        .ok()?;
    let rows = statement
        .query_map(params![session_id], |row| row.get::<_, String>(0))
        .ok()?;
    let mut output = String::new();
    for row in rows {
        let payload_json = row.ok()?;
        let payload: Value = serde_json::from_str(&payload_json).ok()?;
        if let Some(data) = payload.get("data").and_then(Value::as_str) {
            output.push_str(data);
        }
    }
    (!output.is_empty()).then_some(output)
}

/// Atomically commits `payload` to `target`: the bytes land in a temp file in
/// the target's directory and are renamed into place, so readers never see a
/// partial file. Every failure is reported as `output commit failed: …`.
#[cfg(test)]
pub fn deliver_output(target: &str, payload: &str) -> Result<(), String> {
    deliver_output_detailed(target, payload).map_err(|failure| failure.message)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeliveryIoStage {
    BeforeRename,
    AfterRename,
}

struct DeliveryIoFailure {
    stage: DeliveryIoStage,
    message: String,
}

fn deliver_output_detailed(target: &str, payload: &str) -> Result<(), DeliveryIoFailure> {
    let target_path = Path::new(target);
    let parent = match target_path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
        _ => PathBuf::from("."),
    };
    std::fs::create_dir_all(&parent).map_err(|error| DeliveryIoFailure {
        stage: DeliveryIoStage::BeforeRename,
        message: format!(
            "output commit failed: cannot create {}: {error}",
            parent.display()
        ),
    })?;
    let file_name = target_path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .ok_or_else(|| DeliveryIoFailure {
            stage: DeliveryIoStage::BeforeRename,
            message: "output commit failed: output target has no file name".to_string(),
        })?;
    let temp = parent.join(format!(
        ".coven-delivery-{}-{file_name}",
        uuid::Uuid::new_v4()
    ));
    let result = write_atomically(&temp, target_path, payload);
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result
}

fn write_atomically(temp: &Path, target: &Path, payload: &str) -> Result<(), DeliveryIoFailure> {
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(temp)
        .map_err(|error| DeliveryIoFailure {
            stage: DeliveryIoStage::BeforeRename,
            message: format!(
                "output commit failed: cannot create {}: {error}",
                temp.display()
            ),
        })?;
    file.write_all(payload.as_bytes())
        .map_err(|error| DeliveryIoFailure {
            stage: DeliveryIoStage::BeforeRename,
            message: format!(
                "output commit failed: cannot write {}: {error}",
                temp.display()
            ),
        })?;
    file.sync_all().map_err(|error| DeliveryIoFailure {
        stage: DeliveryIoStage::BeforeRename,
        message: format!(
            "output commit failed: cannot sync {}: {error}",
            temp.display()
        ),
    })?;
    drop(file);
    std::fs::rename(temp, target).map_err(|error| DeliveryIoFailure {
        stage: DeliveryIoStage::BeforeRename,
        message: format!(
            "output commit failed: cannot rename {} → {}: {error}",
            temp.display(),
            target.display()
        ),
    })?;
    sync_parent_directory(target).map_err(|message| DeliveryIoFailure {
        stage: DeliveryIoStage::AfterRename,
        message,
    })
}

#[cfg(unix)]
fn sync_parent_directory(target: &Path) -> Result<(), String> {
    let parent = target.parent().unwrap_or_else(|| Path::new("."));
    if take_parent_sync_failure_for_test() {
        return Err(format!(
            "output commit failed: cannot sync directory {}: synthetic parent sync failure",
            parent.display()
        ));
    }
    std::fs::File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| {
            format!(
                "output commit failed: cannot sync directory {}: {error}",
                parent.display()
            )
        })
}

#[cfg(not(unix))]
fn sync_parent_directory(_target: &Path) -> Result<(), String> {
    if take_parent_sync_failure_for_test() {
        return Err(
            "output commit failed: cannot sync directory: synthetic parent sync failure"
                .to_string(),
        );
    }
    Ok(())
}

#[cfg(test)]
thread_local! {
    static FAIL_PARENT_SYNC: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[cfg(test)]
fn set_parent_sync_failure_for_test(enabled: bool) {
    FAIL_PARENT_SYNC.set(enabled);
}

#[cfg(test)]
fn take_parent_sync_failure_for_test() -> bool {
    FAIL_PARENT_SYNC.replace(false)
}

#[cfg(not(test))]
fn take_parent_sync_failure_for_test() -> bool {
    false
}

struct Settlement {
    status: &'static str,
    exit_code: Option<i64>,
    log: Option<String>,
    output_commit: Option<String>,
    reason: Option<String>,
    delivery: Option<DeliveryPlan>,
}

struct DeliveryPlan {
    target: String,
    payload: String,
    digest: String,
}

struct RunningLedgerRow {
    run_id: String,
    automation_id: String,
    session_id: Option<String>,
    occurrence_id: Option<String>,
    occurrence_state: Option<String>,
    occurrence_failure: Option<String>,
    output_target: Option<String>,
    deadline_at: Option<String>,
    delivery_state: String,
    delivery_token: Option<String>,
    delivery_digest: Option<String>,
}

fn running_ledger_rows(conn: &Connection) -> Result<Vec<RunningLedgerRow>, String> {
    let mut statement = conn
        .prepare(
            "SELECT r.id, r.automation_id, r.session_id, r.occurrence_id,
                    o.state, o.failure_reason, r.output_target, r.deadline_at,
                    r.delivery_state, r.delivery_token, r.delivery_digest
             FROM automation_runs AS r
             LEFT JOIN automation_occurrences AS o ON o.id = r.occurrence_id
             WHERE r.status = 'running'",
        )
        .map_err(|error| format!("failed to list running runs: {error}"))?;
    let rows = statement
        .query_map([], |row| {
            Ok(RunningLedgerRow {
                run_id: row.get(0)?,
                automation_id: row.get(1)?,
                session_id: row.get(2)?,
                occurrence_id: row.get(3)?,
                occurrence_state: row.get(4)?,
                occurrence_failure: row.get(5)?,
                output_target: row.get(6)?,
                deadline_at: row.get(7)?,
                delivery_state: row.get(8)?,
                delivery_token: row.get(9)?,
                delivery_digest: row.get(10)?,
            })
        })
        .map_err(|error| format!("failed to list running runs: {error}"))?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row.map_err(|error| format!("failed to read running run: {error}"))?);
    }
    Ok(out)
}

/// Builds the settlement for a run whose session exists, or `None` while the
/// session is still live (or has no sessions row yet — the occurrence lease
/// bounds how long that can block the routine).
fn session_settlement(
    conn: &Connection,
    session_id: &str,
    output_target: Option<&str>,
    deadline: DateTime<Utc>,
    now: DateTime<Utc>,
) -> Result<Option<Settlement>, String> {
    let session = crate::store::get_session(conn, session_id)
        .map_err(|error| format!("failed to read session {session_id}: {error:#}"))?;
    let Some(session) = session else {
        if now >= deadline {
            return Ok(Some(timeout_settlement(None, None, session_id)));
        }
        return Ok(None);
    };
    if !is_terminal_session_status(&session.status) {
        if now >= deadline {
            return Ok(Some(timeout_settlement(
                session.exit_code.map(i64::from),
                capture_bounded_log(conn, session_id),
                session_id,
            )));
        }
        return Ok(None);
    }

    let log = capture_bounded_log(conn, session_id);
    let completed_at = chrono::DateTime::parse_from_rfc3339(&session.updated_at)
        .map(|instant| instant.with_timezone(&Utc))
        .map_err(|error| format!("session {session_id} has invalid completion time: {error}"))?;
    if completed_at > deadline {
        return Ok(Some(timeout_settlement(
            session.exit_code.map(i64::from),
            log,
            session_id,
        )));
    }
    let exit_zero = session.exit_code.unwrap_or(0) == 0;
    let succeeded = matches!(session.status.as_str(), "completed" | "idle") && exit_zero;
    if !succeeded {
        let reason = match session.exit_code {
            Some(code) => format!("session {} (exit code {code})", session.status),
            None => format!("session {}", session.status),
        };
        return Ok(Some(Settlement {
            status: "failed",
            exit_code: session.exit_code.map(i64::from),
            log,
            output_commit: None,
            reason: Some(reason),
            delivery: None,
        }));
    }

    // The run succeeded at the runtime. Delivery is Coven's job: commit the
    // final assistant payload to the configured target, and a failed commit
    // fails the run visibly (never reported as success).
    let (status, reason, output_commit, delivery) = match output_target {
        None => ("succeeded", None, None, None),
        Some(target) => match final_output_text(conn, session_id) {
            None => (
                "failed",
                Some(format!(
                    "output commit failed: no assistant output captured for session {session_id}"
                )),
                None,
                None,
            ),
            Some(payload) => (
                "succeeded",
                None,
                None,
                Some(DeliveryPlan {
                    digest: delivery_digest(target, &payload),
                    target: target.to_string(),
                    payload,
                }),
            ),
        },
    };
    Ok(Some(Settlement {
        status,
        exit_code: session.exit_code.map(i64::from),
        log,
        output_commit,
        reason,
        delivery,
    }))
}

fn timeout_settlement(exit_code: Option<i64>, log: Option<String>, session_id: &str) -> Settlement {
    Settlement {
        status: "failed",
        exit_code,
        log,
        output_commit: None,
        reason: Some(format!(
            "deadline exceeded before session {session_id} completed"
        )),
        delivery: None,
    }
}

fn delivery_digest(target: &str, payload: &str) -> String {
    let mut digest = Sha256::new();
    digest.update((target.len() as u64).to_be_bytes());
    digest.update(target.as_bytes());
    digest.update((payload.len() as u64).to_be_bytes());
    digest.update(payload.as_bytes());
    let hash = digest.finalize();
    let hex: String = hash.iter().map(|byte| format!("{byte:02x}")).collect();
    format!("sha256:{hex}")
}

struct DeliveryReservation {
    token: String,
    digest: String,
}

enum DeliveryTerminalState {
    Committed,
    Failed(String),
    Ambiguous(String),
}

struct SettlementTarget<'a> {
    run_id: &'a str,
    occurrence_id: Option<&'a str>,
    occurrence_state: Option<&'a str>,
    occurrence_failure: Option<&'a str>,
    session_id: Option<String>,
}

fn reserve_delivery(
    conn: &Connection,
    target: &SettlementTarget<'_>,
    plan: &DeliveryPlan,
) -> Result<DeliveryReservation, String> {
    let occurrence_id = target
        .occurrence_id
        .ok_or_else(|| format!("run {} has no occurrence for delivery", target.run_id))?;
    let reservation = DeliveryReservation {
        token: format!("delivery-{}", uuid::Uuid::new_v4()),
        digest: plan.digest.clone(),
    };
    let transaction =
        rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Immediate)
            .map_err(|error| format!("failed to begin delivery reservation: {error}"))?;
    let run_changed = transaction
        .execute(
            "UPDATE automation_runs
             SET delivery_state = 'pending',
                 delivery_token = ?2,
                 delivery_digest = ?3,
                 delivery_error = NULL
             WHERE id = ?1
               AND status = 'running'
               AND delivery_state = 'none'",
            params![target.run_id, reservation.token, reservation.digest],
        )
        .map_err(|error| format!("failed to reserve run delivery: {error}"))?;
    if run_changed != 1 {
        return Err(format!(
            "delivery reservation CAS rejected for run {}",
            target.run_id
        ));
    }
    let occurrence_changed = transaction
        .execute(
            "UPDATE automation_occurrences
             SET delivery_state = 'pending',
                 delivery_token = ?2,
                 delivery_digest = ?3,
                 delivery_error = NULL
             WHERE id = ?1
               AND delivery_state = 'none'
               AND (
                   state IN ('claimed', 'running')
                   OR (state = 'failed' AND failure_reason = 'lease expired')
               )",
            params![occurrence_id, reservation.token, reservation.digest],
        )
        .map_err(|error| format!("failed to reserve occurrence delivery: {error}"))?;
    if occurrence_changed != 1 {
        return Err(format!(
            "delivery reservation CAS rejected for occurrence {occurrence_id}"
        ));
    }
    transaction
        .commit()
        .map_err(|error| format!("failed to commit delivery reservation: {error}"))?;
    Ok(reservation)
}

fn finalize_reserved_delivery(
    conn: &Connection,
    target: SettlementTarget<'_>,
    mut settlement: Settlement,
    reservation: &DeliveryReservation,
    terminal: DeliveryTerminalState,
    output_target: &str,
    now: DateTime<Utc>,
) -> Result<(&'static str, Option<String>), String> {
    let occurrence_id = target
        .occurrence_id
        .ok_or_else(|| format!("run {} has no occurrence for delivery", target.run_id))?;
    let (delivery_state, status, reason, output_commit) = match terminal {
        DeliveryTerminalState::Committed => (
            "committed",
            "succeeded",
            None,
            Some(output_target.to_string()),
        ),
        DeliveryTerminalState::Failed(reason) => ("failed", "failed", Some(reason), None),
        DeliveryTerminalState::Ambiguous(reason) => ("ambiguous", "failed", Some(reason), None),
    };
    settlement.status = status;
    settlement.reason = reason.clone();
    settlement.output_commit = output_commit;
    settlement.delivery = None;

    let transaction =
        rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Immediate)
            .map_err(|error| format!("failed to begin delivery finalization: {error}"))?;
    let run_changed = transaction
        .execute(
            "UPDATE automation_runs
             SET delivery_state = ?4,
                 delivery_error = ?5
             WHERE id = ?1
               AND status = 'running'
               AND delivery_state = 'pending'
               AND delivery_token = ?2
               AND delivery_digest = ?3",
            params![
                target.run_id,
                reservation.token,
                reservation.digest,
                delivery_state,
                reason
            ],
        )
        .map_err(|error| format!("failed to finalize run delivery reservation: {error}"))?;
    if run_changed != 1 {
        return Err(format!(
            "delivery finalization CAS rejected for run {}",
            target.run_id
        ));
    }
    let occurrence_changed = transaction
        .execute(
            "UPDATE automation_occurrences
             SET state = ?4,
                 failure_reason = ?5,
                 lease_owner = NULL,
                 lease_expires_at = NULL,
                 delivery_state = ?6,
                 delivery_error = ?5,
                 updated_at = ?7
             WHERE id = ?1
               AND delivery_state = 'pending'
               AND delivery_token = ?2
               AND delivery_digest = ?3
               AND (
                   state IN ('claimed', 'running')
                   OR (state = 'failed' AND failure_reason = 'lease expired')
               )",
            params![
                occurrence_id,
                reservation.token,
                reservation.digest,
                status,
                reason,
                delivery_state,
                now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
            ],
        )
        .map_err(|error| format!("failed to finalize occurrence delivery reservation: {error}"))?;
    if occurrence_changed != 1 {
        return Err(format!(
            "delivery finalization CAS rejected for occurrence {occurrence_id}"
        ));
    }
    let finished = record_run_finish(
        &transaction,
        target.run_id,
        RunFinish {
            status,
            exit_code: settlement.exit_code,
            session_id: target.session_id,
            log_json: settlement.log,
            output_commit: settlement.output_commit,
        },
        now,
    )
    .map_err(|error| format!("failed to finalize delivered run: {error:#}"))?;
    if !finished {
        return Err(format!(
            "run {} changed during delivery finalization",
            target.run_id
        ));
    }
    transaction
        .commit()
        .map_err(|error| format!("failed to commit delivery finalization: {error}"))?;
    Ok((status, reason))
}

fn settle_reserved_delivery(
    conn: &Connection,
    target: SettlementTarget<'_>,
    settlement: Settlement,
    plan: DeliveryPlan,
    now: DateTime<Utc>,
) -> Result<(&'static str, Option<String>), String> {
    let reservation = reserve_delivery(conn, &target, &plan)?;
    let terminal = match deliver_output_detailed(&plan.target, &plan.payload) {
        Ok(()) => DeliveryTerminalState::Committed,
        Err(failure) if failure.stage == DeliveryIoStage::BeforeRename => {
            DeliveryTerminalState::Failed(failure.message)
        }
        Err(failure) => DeliveryTerminalState::Ambiguous(failure.message),
    };
    finalize_reserved_delivery(
        conn,
        target,
        settlement,
        &reservation,
        terminal,
        &plan.target,
        now,
    )
}

fn settle_interrupted_delivery(
    conn: &Connection,
    target: SettlementTarget<'_>,
    mut settlement: Settlement,
    token: String,
    digest: String,
    output_target: &str,
    now: DateTime<Utc>,
) -> Result<(&'static str, Option<String>), String> {
    settlement.delivery = None;
    let reason = "delivery outcome ambiguous after an interrupted pending reservation".to_string();
    finalize_reserved_delivery(
        conn,
        target,
        settlement,
        &DeliveryReservation { token, digest },
        DeliveryTerminalState::Ambiguous(reason),
        output_target,
        now,
    )
}

fn settle_linked_state(
    conn: &Connection,
    target: SettlementTarget<'_>,
    settlement: Settlement,
    now: DateTime<Utc>,
) -> Result<(), String> {
    let transaction =
        rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Immediate)
            .map_err(|error| format!("failed to begin run settlement: {error}"))?;

    if let Some(occurrence_id) = target.occurrence_id {
        let recoverable_lease_failure = target.occurrence_state == Some("failed")
            && target.occurrence_failure == Some("lease expired");
        let unsettled = matches!(target.occurrence_state, Some("claimed" | "running"));
        if !unsettled && !recoverable_lease_failure {
            return Err(format!(
                "contradictory terminal occurrence state `{}` for running run {}",
                target.occurrence_state.unwrap_or("missing"),
                target.run_id
            ));
        }
        let changed = transaction
            .execute(
                "UPDATE automation_occurrences
                 SET state = ?2,
                     failure_reason = ?3,
                     lease_owner = NULL,
                     lease_expires_at = NULL,
                     updated_at = ?4
                 WHERE id = ?1
                   AND (
                       state IN ('claimed', 'running')
                       OR (state = 'failed' AND failure_reason = 'lease expired')
                   )",
                params![
                    occurrence_id,
                    settlement.status,
                    settlement.reason,
                    now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
                ],
            )
            .map_err(|error| format!("failed to settle occurrence with run: {error}"))?;
        if changed != 1 {
            return Err(format!(
                "occurrence {occurrence_id} changed during run settlement"
            ));
        }
    }
    let finished = record_run_finish(
        &transaction,
        target.run_id,
        RunFinish {
            status: settlement.status,
            exit_code: settlement.exit_code,
            session_id: target.session_id,
            log_json: settlement.log,
            output_commit: settlement.output_commit,
        },
        now,
    )
    .map_err(|error| format!("failed to settle run: {error:#}"))?;
    if !finished {
        return Err(format!("run {} changed during settlement", target.run_id));
    }
    transaction
        .commit()
        .map_err(|error| format!("failed to commit run settlement: {error}"))
}

/// Settles every running ledger row whose work has finished. A row settles
/// when its session reached a terminal state, or when its occurrence already
/// settled through lease recovery. The matching occurrence settles alongside
/// the ledger row; nothing here ever reports a run success that did not
/// actually happen.
pub fn settle_finished_runs(
    conn: &Connection,
    now: DateTime<Utc>,
) -> Result<ReconcileReport, String> {
    let mut report = ReconcileReport::default();

    for row in running_ledger_rows(conn)? {
        let RunningLedgerRow {
            run_id,
            automation_id,
            session_id,
            occurrence_id,
            occurrence_state,
            occurrence_failure,
            output_target,
            deadline_at,
            delivery_state,
            delivery_token,
            delivery_digest,
        } = row;

        let deadline_at =
            deadline_at.ok_or_else(|| format!("running run {run_id} has no pinned deadline"))?;
        let deadline = chrono::DateTime::parse_from_rfc3339(&deadline_at)
            .map(|instant| instant.with_timezone(&Utc))
            .map_err(|error| format!("running run {run_id} has invalid deadline: {error}"))?;
        let mut settlement = match session_id.as_deref() {
            Some(session_id) => {
                session_settlement(conn, session_id, output_target.as_deref(), deadline, now)?
            }
            None if now >= deadline => Some(Settlement {
                status: "failed",
                exit_code: None,
                log: None,
                output_commit: None,
                reason: Some("deadline exceeded before a session was attached".to_string()),
                delivery: None,
            }),
            None => None,
        };
        if settlement.is_none() {
            if occurrence_state.as_deref() == Some("failed") {
                let reason = occurrence_failure
                    .clone()
                    .filter(|reason| !reason.trim().is_empty())
                    .unwrap_or_else(|| "lease expired".to_string());
                settlement = Some(Settlement {
                    status: "failed",
                    exit_code: None,
                    log: None,
                    output_commit: None,
                    reason: Some(reason),
                    delivery: None,
                });
            } else if occurrence_state.as_deref() == Some("succeeded") {
                // Defensive: an occurrence cannot legitimately settle success
                // before its run does. Record a visible failure rather than
                // inventing a result.
                settlement = Some(Settlement {
                    status: "failed",
                    exit_code: None,
                    log: None,
                    output_commit: None,
                    reason: Some("occurrence settled without a run result".to_string()),
                    delivery: None,
                });
            }
        }

        let Some(mut settlement) = settlement else {
            report.still_running += 1;
            continue;
        };

        let target = SettlementTarget {
            run_id: &run_id,
            occurrence_id: occurrence_id.as_deref(),
            occurrence_state: occurrence_state.as_deref(),
            occurrence_failure: occurrence_failure.as_deref(),
            session_id: session_id.clone(),
        };
        let (status, reason) = match settlement.delivery.take() {
            Some(plan) if delivery_state == "none" => {
                settle_reserved_delivery(conn, target, settlement, plan, now)?
            }
            Some(_) if delivery_state == "pending" => {
                let delivery_reservation_id = delivery_token
                    .ok_or_else(|| format!("pending delivery for run {run_id} has no token"))?;
                let digest = delivery_digest
                    .ok_or_else(|| format!("pending delivery for run {run_id} has no digest"))?;
                let output_target = output_target
                    .as_deref()
                    .ok_or_else(|| format!("pending delivery for run {run_id} has no target"))?;
                settle_interrupted_delivery(
                    conn,
                    target,
                    settlement,
                    delivery_reservation_id,
                    digest,
                    output_target,
                    now,
                )?
            }
            Some(_) => {
                return Err(format!(
                    "running run {run_id} has invalid delivery state `{delivery_state}`"
                ));
            }
            None if delivery_state == "none" => {
                let status = settlement.status;
                let reason = settlement.reason.clone();
                settle_linked_state(conn, target, settlement, now)?;
                (status, reason)
            }
            None => {
                return Err(format!(
                    "run {run_id} has delivery state `{delivery_state}` without a delivery plan"
                ));
            }
        };
        if status == "succeeded" {
            report.settled_succeeded += 1;
        } else {
            report.settled_failed += 1;
            if let Some(reason) = reason {
                report.failures.push(format!("{automation_id}: {reason}"));
            }
        }
    }

    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::automations::definition::RoutineDefinition;
    use crate::automations::store::insert_definition;
    use crate::store::{initialize_store, insert_event, insert_session, SessionRecord};
    use serde_json::json;

    fn temp_store() -> (tempfile::TempDir, Connection) {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("store.sqlite");
        initialize_store(&path).unwrap();
        let conn = crate::store::open_store(&path).unwrap();
        (temp, conn)
    }

    fn definition(id: &str, output_target: Option<&str>) -> RoutineDefinition {
        let mut value = json!({
            "schemaVersion": 1,
            "id": id,
            "name": id,
            "status": "ACTIVE",
            "rrule": "FREQ=DAILY;BYHOUR=9",
            "timezone": "utc",
            "misfire": "latest",
            "overlap": "forbid",
            "timeoutMinutes": 30,
            "runtime": "coven-code",
            "cwd": "/work/project",
            "prompt": "Do the thing."
        });
        if let Some(output_target) = output_target {
            let object = value.as_object_mut().unwrap();
            object.insert("outputTarget".to_string(), json!(output_target));
        }
        RoutineDefinition::from_json(&value).unwrap()
    }

    fn session_record(conn: &Connection, id: &str, status: &str, exit_code: Option<i32>) {
        insert_session(
            conn,
            &SessionRecord {
                id: id.to_string(),
                project_root: "/work/project".to_string(),
                harness: "coven-code".to_string(),
                title: "routine run".to_string(),
                status: status.to_string(),
                exit_code,
                archived_at: None,
                created_at: "2026-08-28T09:00:00Z".to_string(),
                updated_at: "2026-08-28T09:05:00Z".to_string(),
                conversation_id: None,
                familiar_id: None,
                execution_binding: None,
                labels: Vec::new(),
                visibility: "private".to_string(),
                external: false,
                transcript_path: None,
            },
        )
        .unwrap();
    }

    fn event(conn: &Connection, session_id: &str, kind: &str, data: &str) {
        static EVENT_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let event_number = EVENT_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        insert_event(
            conn,
            &crate::store::EventRecord {
                seq: 0,
                id: format!("event-{event_number}"),
                session_id: session_id.to_string(),
                kind: kind.to_string(),
                payload_json: json!({ "data": data }).to_string(),
                created_at: "2026-08-28T09:01:00Z".to_string(),
            },
        )
        .unwrap();
    }

    /// Seeds a live run: claimed occurrence + running ledger row, as dispatch
    /// leaves them before reconciliation.
    fn live_run(conn: &Connection, automation_id: &str, session_id: &str) -> String {
        let now = "2026-08-28T09:00:00.000Z";
        let record = super::super::store::get_definition(conn, automation_id)
            .unwrap()
            .unwrap();
        let snapshot = super::super::store::definition_snapshot(&record).unwrap();
        conn.execute(
            "INSERT INTO automation_occurrences
                (id, automation_id, scheduled_for, state, lease_owner, lease_expires_at,
                 attempt, definition_revision, definition_digest, definition_json,
                 output_target, deadline_at, created_at, updated_at)
             VALUES (?1, ?2, ?3, 'running', 'daemon', '2026-08-28T10:00:00.000Z', 1,
                     ?4, ?5, ?6, ?7, '2026-08-28T10:00:00.000Z', ?3, ?3)",
            rusqlite::params![
                format!("occ-{session_id}"),
                automation_id,
                now,
                snapshot.revision,
                snapshot.digest,
                snapshot.definition_json,
                snapshot.output_target
            ],
        )
        .unwrap();
        let run_id = format!("run-{session_id}");
        record_run_start_raw(conn, &run_id, automation_id, session_id, now);
        run_id
    }

    fn record_run_start_raw(
        conn: &Connection,
        run_id: &str,
        automation_id: &str,
        session_id: &str,
        now: &str,
    ) {
        let record = super::super::store::get_definition(conn, automation_id)
            .unwrap()
            .unwrap();
        let snapshot = super::super::store::definition_snapshot(&record).unwrap();
        conn.execute(
            "INSERT INTO automation_runs
                (id, automation_id, occurrence_id, session_id, familiar_id, runtime,
                 status, definition_revision, definition_digest, definition_json,
                 output_target, deadline_at, started_at)
             VALUES (?1, ?2, ?3, ?4, 'charm', 'coven-code', 'running',
                     ?5, ?6, ?7, ?8, '2026-08-28T10:00:00.000Z', ?9)",
            rusqlite::params![
                run_id,
                automation_id,
                format!("occ-{session_id}"),
                session_id,
                snapshot.revision,
                snapshot.digest,
                snapshot.definition_json,
                snapshot.output_target,
                now
            ],
        )
        .unwrap();
    }

    #[test]
    fn deliver_output_replaces_atomically_without_temp_litter() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("out").join("payload.md");
        deliver_output(target.to_str().unwrap(), "first").unwrap();
        deliver_output(target.to_str().unwrap(), "second version").unwrap();

        assert_eq!(std::fs::read_to_string(&target).unwrap(), "second version");
        let entries: Vec<_> = std::fs::read_dir(temp.path().join("out"))
            .unwrap()
            .collect();
        assert_eq!(entries.len(), 1, "no temp files may remain");
    }

    #[test]
    fn deliver_output_failure_is_visible() {
        let temp = tempfile::tempdir().unwrap();
        let blocker = temp.path().join("blocker");
        std::fs::write(&blocker, "a file, not a directory").unwrap();
        let target = blocker.join("payload.md");

        let error = deliver_output(target.to_str().unwrap(), "payload").unwrap_err();
        assert!(error.contains("output commit failed"), "{error}");
    }

    #[test]
    fn bounded_log_keeps_the_tail_within_the_budget() {
        let (_temp, conn) = temp_store();
        session_record(&conn, "session-1", "completed", Some(0));
        let huge = "x".repeat(50 * 1024);
        event(&conn, "session-1", "output", &huge);
        event(&conn, "session-1", "output", &huge);
        event(&conn, "session-1", "output", "final answer");

        let log = capture_bounded_log(&conn, "session-1").unwrap();
        assert!(
            log.chars().count() <= LOG_ENTRY_MAX_CHARS,
            "log is bounded: {}",
            log.chars().count()
        );
        assert!(log.contains("logTruncated"), "{log}");
        assert!(log.contains("final answer"), "tail is kept: {log}");
    }

    #[test]
    fn final_output_text_reconstructs_every_ordered_output_chunk() {
        let (_temp, conn) = temp_store();
        session_record(&conn, "session-1", "completed", Some(0));
        event(&conn, "session-1", "output", "first ");
        event(&conn, "session-1", "input", "keep going");
        event(&conn, "session-1", "output", "second ");
        for _ in 0..205 {
            event(&conn, "session-1", "output", "x");
        }
        event(&conn, "session-1", "output", " final");

        let expected = format!("first second {} final", "x".repeat(205));
        assert_eq!(
            final_output_text(&conn, "session-1").as_deref(),
            Some(expected.as_str())
        );
        assert_eq!(final_output_text(&conn, "session-missing"), None);
    }

    #[test]
    fn settlement_uses_the_output_target_pinned_when_the_run_started() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        let first_target = first.path().join("payload.md");
        let second_target = second.path().join("payload.md");
        let (_temp, conn) = temp_store();
        insert_definition(
            &conn,
            &definition("daily", Some(first_target.to_str().unwrap())),
        )
        .unwrap();
        live_run(&conn, "daily", "session-1");
        super::super::store::update_definition(
            &conn,
            &definition("daily", Some(second_target.to_str().unwrap())),
        )
        .unwrap();
        super::super::store::delete_definition(&conn, "daily").unwrap();
        session_record(&conn, "session-1", "completed", Some(0));
        event(&conn, "session-1", "output", "immutable delivery");

        let report = settle_finished_runs(
            &conn,
            chrono::DateTime::parse_from_rfc3339("2026-08-28T09:30:00.000Z")
                .unwrap()
                .with_timezone(&Utc),
        )
        .unwrap();

        assert_eq!(report.settled_succeeded, 1);
        assert_eq!(
            std::fs::read_to_string(&first_target).unwrap(),
            "immutable delivery"
        );
        assert!(
            !second_target.exists(),
            "an in-flight run must never reload a revised delivery target"
        );
    }

    #[test]
    fn settle_delivers_output_and_records_success() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("out").join("payload.md");
        let (_temp, conn) = temp_store();
        insert_definition(&conn, &definition("daily", Some(target.to_str().unwrap()))).unwrap();
        live_run(&conn, "daily", "session-1");
        session_record(&conn, "session-1", "completed", Some(0));
        event(&conn, "session-1", "output", "the delivered payload");

        let report = settle_finished_runs(&conn, Utc::now()).unwrap();
        assert_eq!(report.settled_succeeded, 1);
        assert_eq!(report.settled_failed, 0);

        let run: (
            String,
            Option<i64>,
            Option<String>,
            String,
            Option<String>,
            Option<String>,
        ) = conn
            .query_row(
                "SELECT status, exit_code, output_commit, delivery_state,
                        delivery_token, delivery_digest
                 FROM automation_runs WHERE id = 'run-session-1'",
                [],
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
            .unwrap();
        assert_eq!(run.0, "succeeded");
        assert_eq!(run.1, Some(0));
        assert_eq!(run.2.as_deref(), Some(target.to_str().unwrap()));
        assert_eq!(run.3, "committed");
        assert!(run
            .4
            .as_deref()
            .is_some_and(|token| token.starts_with("delivery-")));
        assert!(run
            .5
            .as_deref()
            .is_some_and(|digest| digest.starts_with("sha256:")));
        assert_eq!(
            std::fs::read_to_string(&target).unwrap(),
            "the delivered payload"
        );

        let state: String = conn
            .query_row(
                "SELECT state FROM automation_occurrences WHERE automation_id = 'daily'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(state, "succeeded");
    }

    #[test]
    fn failed_output_commit_fails_the_run_visibly() {
        let temp = tempfile::tempdir().unwrap();
        let blocker = temp.path().join("blocker");
        std::fs::write(&blocker, "a file, not a directory").unwrap();
        let target = blocker.join("payload.md");

        let (_temp, conn) = temp_store();
        insert_definition(&conn, &definition("daily", Some(target.to_str().unwrap()))).unwrap();
        live_run(&conn, "daily", "session-1");
        session_record(&conn, "session-1", "completed", Some(0));
        event(&conn, "session-1", "output", "the delivered payload");

        let report = settle_finished_runs(&conn, Utc::now()).unwrap();
        assert_eq!(report.settled_failed, 1);
        assert!(
            report
                .failures
                .iter()
                .any(|failure| failure.contains("output commit failed")),
            "{report:?}"
        );

        let (status, delivery_state): (String, String) = conn
            .query_row(
                "SELECT status, delivery_state
                 FROM automation_runs WHERE id = 'run-session-1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            status, "failed",
            "a failed delivery must not report success"
        );
        assert_eq!(delivery_state, "failed");
        let reason: String = conn
            .query_row(
                "SELECT failure_reason FROM automation_occurrences WHERE automation_id = 'daily'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(reason.contains("output commit failed"), "{reason}");
    }

    #[test]
    fn delivery_reservation_cas_failure_prevents_file_write() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("payload.md");
        let (_temp, conn) = temp_store();
        insert_definition(&conn, &definition("daily", Some(target.to_str().unwrap()))).unwrap();
        live_run(&conn, "daily", "session-1");
        session_record(&conn, "session-1", "completed", Some(0));
        event(&conn, "session-1", "output", "must not be delivered");
        conn.execute_batch(
            "CREATE TRIGGER reject_delivery_reservation
             BEFORE UPDATE OF delivery_state ON automation_runs
             WHEN NEW.delivery_state = 'pending'
             BEGIN
                 SELECT RAISE(ABORT, 'synthetic reservation rejection');
             END;",
        )
        .unwrap();

        let error = settle_finished_runs(&conn, Utc::now()).unwrap_err();

        assert!(error.contains("reservation"), "{error}");
        assert!(!target.exists(), "I/O must follow a committed reservation");
        let states: (String, String) = conn
            .query_row(
                "SELECT o.state, r.status
                 FROM automation_occurrences AS o
                 JOIN automation_runs AS r ON r.occurrence_id = o.id
                 WHERE r.id = 'run-session-1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(states, ("running".to_string(), "running".to_string()));
    }

    #[test]
    fn pending_delivery_retry_becomes_ambiguous_without_repeating_io() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("payload.md");
        let (_temp, conn) = temp_store();
        insert_definition(&conn, &definition("daily", Some(target.to_str().unwrap()))).unwrap();
        live_run(&conn, "daily", "session-1");
        session_record(&conn, "session-1", "completed", Some(0));
        event(&conn, "session-1", "output", "must not be replayed");
        conn.execute(
            "UPDATE automation_runs
             SET delivery_state = 'pending',
                 delivery_token = 'delivery-token',
                 delivery_digest = 'sha256:pending'",
            [],
        )
        .unwrap();
        conn.execute(
            "UPDATE automation_occurrences
             SET delivery_state = 'pending',
                 delivery_token = 'delivery-token',
                 delivery_digest = 'sha256:pending'",
            [],
        )
        .unwrap();

        let report = settle_finished_runs(&conn, Utc::now()).unwrap();

        assert_eq!(report.settled_failed, 1);
        assert!(
            !target.exists(),
            "an uncertain prior delivery is never repeated"
        );
        let states: (String, String, String, String) = conn
            .query_row(
                "SELECT o.state, o.delivery_state, r.status, r.delivery_state
                 FROM automation_occurrences AS o
                 JOIN automation_runs AS r ON r.occurrence_id = o.id
                 WHERE r.id = 'run-session-1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(
            states,
            (
                "failed".to_string(),
                "ambiguous".to_string(),
                "failed".to_string(),
                "ambiguous".to_string()
            )
        );
        let second = settle_finished_runs(&conn, Utc::now()).unwrap();
        assert_eq!(second.settled_succeeded + second.settled_failed, 0);
        assert!(!target.exists());
    }

    #[test]
    fn delivery_finalization_cas_failure_is_not_replayed_silently() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("payload.md");
        let (_temp, conn) = temp_store();
        insert_definition(&conn, &definition("daily", Some(target.to_str().unwrap()))).unwrap();
        live_run(&conn, "daily", "session-1");
        session_record(&conn, "session-1", "completed", Some(0));
        event(&conn, "session-1", "output", "delivered once");
        conn.execute_batch(
            "CREATE TRIGGER reject_delivery_finalization
             BEFORE UPDATE OF delivery_state ON automation_runs
             WHEN OLD.delivery_state = 'pending' AND NEW.delivery_state = 'committed'
             BEGIN
                 SELECT RAISE(ABORT, 'synthetic finalization rejection');
             END;",
        )
        .unwrap();

        let error = settle_finished_runs(&conn, Utc::now()).unwrap_err();

        assert!(error.contains("finalize"), "{error}");
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "delivered once");
        std::fs::write(&target, "external edit after uncertain commit").unwrap();
        conn.execute_batch("DROP TRIGGER reject_delivery_finalization")
            .unwrap();
        let report = settle_finished_runs(&conn, Utc::now()).unwrap();
        assert_eq!(report.settled_failed, 1);
        assert_eq!(
            std::fs::read_to_string(&target).unwrap(),
            "external edit after uncertain commit",
            "retry must not repeat an unrecorded delivery"
        );
    }

    #[test]
    fn post_rename_parent_sync_failure_is_recorded_as_ambiguous() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("payload.md");
        let (_temp, conn) = temp_store();
        insert_definition(&conn, &definition("daily", Some(target.to_str().unwrap()))).unwrap();
        live_run(&conn, "daily", "session-1");
        session_record(&conn, "session-1", "completed", Some(0));
        event(
            &conn,
            "session-1",
            "output",
            "renamed but not directory-synced",
        );
        set_parent_sync_failure_for_test(true);

        let report = settle_finished_runs(&conn, Utc::now()).unwrap();

        assert_eq!(report.settled_failed, 1);
        assert_eq!(
            std::fs::read_to_string(&target).unwrap(),
            "renamed but not directory-synced"
        );
        let states: (String, String, String, String) = conn
            .query_row(
                "SELECT o.state, o.delivery_state, r.status, r.delivery_state
                 FROM automation_occurrences AS o
                 JOIN automation_runs AS r ON r.occurrence_id = o.id
                 WHERE r.id = 'run-session-1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(
            states,
            (
                "failed".to_string(),
                "ambiguous".to_string(),
                "failed".to_string(),
                "ambiguous".to_string()
            )
        );
        assert!(report
            .failures
            .iter()
            .any(|failure| failure.contains("sync directory")));
    }

    #[test]
    fn failed_session_fails_the_run_without_delivery() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("payload.md");
        let (_temp, conn) = temp_store();
        insert_definition(&conn, &definition("daily", Some(target.to_str().unwrap()))).unwrap();
        live_run(&conn, "daily", "session-1");
        session_record(&conn, "session-1", "failed", Some(1));
        event(&conn, "session-1", "output", "partial output");

        let report = settle_finished_runs(&conn, Utc::now()).unwrap();
        assert_eq!(report.settled_failed, 1);

        let (status, exit_code, output_commit): (String, Option<i64>, Option<String>) = conn
            .query_row(
                "SELECT status, exit_code, output_commit FROM automation_runs WHERE id = 'run-session-1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(status, "failed");
        assert_eq!(exit_code, Some(1));
        assert_eq!(output_commit, None);
        assert!(!target.exists(), "nothing is delivered for a failed run");
    }

    #[test]
    fn recovered_occurrence_settles_the_ledger_as_failed() {
        let (_temp, conn) = temp_store();
        insert_definition(&conn, &definition("daily", None)).unwrap();
        live_run(&conn, "daily", "session-1");
        // Lease recovery already failed the occurrence (no sessions row was
        // ever written — the daemon died mid-run).
        conn.execute(
            "UPDATE automation_occurrences SET state = 'failed',
                 failure_reason = 'lease expired'",
            [],
        )
        .unwrap();

        let report = settle_finished_runs(&conn, Utc::now()).unwrap();
        assert_eq!(report.settled_failed, 1);

        let (status, log): (String, Option<String>) = conn
            .query_row(
                "SELECT status, log_json FROM automation_runs WHERE id = 'run-session-1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(status, "failed");
        assert!(log.is_none(), "no stream exists to capture");
    }

    #[test]
    fn completion_before_deadline_repairs_a_stale_lease_failure_consistently() {
        let (_temp, conn) = temp_store();
        insert_definition(&conn, &definition("daily", None)).unwrap();
        live_run(&conn, "daily", "session-1");
        session_record(&conn, "session-1", "completed", Some(0));
        conn.execute(
            "UPDATE automation_occurrences
             SET state = 'failed', failure_reason = 'lease expired'
             WHERE automation_id = 'daily'",
            [],
        )
        .unwrap();

        let report = settle_finished_runs(&conn, Utc::now()).unwrap();

        assert_eq!(report.settled_succeeded, 1);
        let (occurrence, run): (String, String) = conn
            .query_row(
                "SELECT o.state, r.status
                 FROM automation_occurrences AS o
                 JOIN automation_runs AS r ON r.occurrence_id = o.id
                 WHERE o.automation_id = 'daily'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            (occurrence.as_str(), run.as_str()),
            ("succeeded", "succeeded")
        );
    }

    #[test]
    fn completion_after_deadline_fails_occurrence_and_run_consistently() {
        let (_temp, conn) = temp_store();
        insert_definition(&conn, &definition("daily", None)).unwrap();
        live_run(&conn, "daily", "session-1");
        session_record(&conn, "session-1", "completed", Some(0));
        conn.execute(
            "UPDATE sessions SET updated_at = '2026-08-28T10:05:00.000Z'
             WHERE id = 'session-1'",
            [],
        )
        .unwrap();

        let report = settle_finished_runs(&conn, Utc::now()).unwrap();

        assert_eq!(report.settled_failed, 1);
        let (occurrence, run, reason): (String, String, String) = conn
            .query_row(
                "SELECT o.state, r.status, o.failure_reason
                 FROM automation_occurrences AS o
                 JOIN automation_runs AS r ON r.occurrence_id = o.id
                 WHERE o.automation_id = 'daily'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!((occurrence.as_str(), run.as_str()), ("failed", "failed"));
        assert!(reason.contains("deadline"), "{reason}");
    }

    #[test]
    fn contradictory_terminal_occurrence_is_reported_not_silently_overwritten() {
        let (_temp, conn) = temp_store();
        insert_definition(&conn, &definition("daily", None)).unwrap();
        live_run(&conn, "daily", "session-1");
        session_record(&conn, "session-1", "completed", Some(0));
        conn.execute(
            "UPDATE automation_occurrences
             SET state = 'failed', failure_reason = 'operator decision'
             WHERE automation_id = 'daily'",
            [],
        )
        .unwrap();

        let error = settle_finished_runs(&conn, Utc::now()).unwrap_err();

        assert!(error.contains("contradictory"), "{error}");
        let status: String = conn
            .query_row(
                "SELECT status FROM automation_runs WHERE id = 'run-session-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(status, "running");
    }

    #[test]
    fn still_running_rows_are_left_alone() {
        let (_temp, conn) = temp_store();
        insert_definition(&conn, &definition("daily", None)).unwrap();
        live_run(&conn, "daily", "session-1");
        session_record(&conn, "session-1", "running", None);

        let report = settle_finished_runs(
            &conn,
            chrono::DateTime::parse_from_rfc3339("2026-08-28T09:30:00.000Z")
                .unwrap()
                .with_timezone(&Utc),
        )
        .unwrap();
        assert_eq!(report.still_running, 1);
        let status: String = conn
            .query_row(
                "SELECT status FROM automation_runs WHERE id = 'run-session-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(status, "running");
    }
}
