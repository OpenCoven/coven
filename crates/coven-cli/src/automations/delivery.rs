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

use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};
use serde_json::{json, Value};

use super::occurrences::settle_occurrence;
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

/// The final assistant payload of a session: the text of its last `output`
/// event. `None` when the session produced no output.
pub fn final_output_text(conn: &Connection, session_id: &str) -> Option<String> {
    let events = read_stream_tail(conn, session_id, BOUNDED_LOG_EVENT_LIMIT).ok()?;
    events
        .iter()
        .rev()
        .find(|event| event.kind == "output")
        .and_then(|event| event.payload.get("data"))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .filter(|text| !text.is_empty())
}

/// Atomically commits `payload` to `target`: the bytes land in a temp file in
/// the target's directory and are renamed into place, so readers never see a
/// partial file. Every failure is reported as `output commit failed: …`.
pub fn deliver_output(target: &str, payload: &str) -> Result<(), String> {
    let target_path = Path::new(target);
    let parent = match target_path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
        _ => PathBuf::from("."),
    };
    std::fs::create_dir_all(&parent).map_err(|error| {
        format!(
            "output commit failed: cannot create {}: {error}",
            parent.display()
        )
    })?;
    let file_name = target_path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .ok_or_else(|| "output commit failed: output target has no file name".to_string())?;
    let pid = std::process::id();
    let millis = chrono::Utc::now().timestamp_millis();
    let temp = parent.join(format!(".coven-delivery-{pid}-{millis}-{file_name}"));
    let result = write_atomically(&temp, target_path, payload);
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result
}

fn write_atomically(temp: &Path, target: &Path, payload: &str) -> Result<(), String> {
    std::fs::write(temp, payload).map_err(|error| {
        format!(
            "output commit failed: cannot write {}: {error}",
            temp.display()
        )
    })?;
    std::fs::rename(temp, target).map_err(|error| {
        format!(
            "output commit failed: cannot rename {} → {}: {error}",
            temp.display(),
            target.display()
        )
    })
}

struct Settlement {
    status: &'static str,
    exit_code: Option<i64>,
    log: Option<String>,
    output_commit: Option<String>,
    reason: Option<String>,
}

struct RunningLedgerRow {
    run_id: String,
    automation_id: String,
    session_id: Option<String>,
    occurrence_id: Option<String>,
    occurrence_state: Option<String>,
    occurrence_failure: Option<String>,
}

fn running_ledger_rows(conn: &Connection) -> Result<Vec<RunningLedgerRow>, String> {
    let mut statement = conn
        .prepare(
            "SELECT r.id, r.automation_id, r.session_id, r.occurrence_id,
                    o.state, o.failure_reason
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
    automation_id: &str,
    session_id: &str,
) -> Result<Option<Settlement>, String> {
    let session = crate::store::get_session(conn, session_id)
        .map_err(|error| format!("failed to read session {session_id}: {error:#}"))?;
    let Some(session) = session else {
        return Ok(None);
    };
    if !is_terminal_session_status(&session.status) {
        return Ok(None);
    }

    let log = capture_bounded_log(conn, session_id);
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
        }));
    }

    // The run succeeded at the runtime. Delivery is Coven's job: commit the
    // final assistant payload to the configured target, and a failed commit
    // fails the run visibly (never reported as success).
    let definition = super::runner::load_definition_for_run(conn, automation_id)
        .map_err(|error| format!("failed to load routine {automation_id}: {error}"))?;
    let output_target = definition.and_then(|routine| routine.output_target);
    let (status, reason, output_commit) = match output_target.as_deref() {
        None => ("succeeded", None, None),
        Some(target) => match final_output_text(conn, session_id) {
            None => (
                "failed",
                Some(format!(
                    "output commit failed: no assistant output captured for session {session_id}"
                )),
                None,
            ),
            Some(payload) => match deliver_output(target, &payload) {
                Ok(()) => ("succeeded", None, Some(target.to_string())),
                Err(error) => ("failed", Some(error), None),
            },
        },
    };
    Ok(Some(Settlement {
        status,
        exit_code: session.exit_code.map(i64::from),
        log,
        output_commit,
        reason,
    }))
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
        } = row;

        let mut settlement = match session_id.as_deref() {
            Some(session_id) => session_settlement(conn, &automation_id, session_id)?,
            None => None,
        };
        if settlement.is_none() {
            if occurrence_state.as_deref() == Some("failed") {
                let reason = occurrence_failure
                    .filter(|reason| !reason.trim().is_empty())
                    .unwrap_or_else(|| "lease expired".to_string());
                settlement = Some(Settlement {
                    status: "failed",
                    exit_code: None,
                    log: None,
                    output_commit: None,
                    reason: Some(reason),
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
                });
            }
        }

        let Some(settlement) = settlement else {
            report.still_running += 1;
            continue;
        };

        if let Some(occurrence_id) = occurrence_id.as_deref() {
            let _ = settle_occurrence(
                conn,
                occurrence_id,
                settlement.status,
                settlement.reason.as_deref(),
                now,
            );
        }
        let _ = record_run_finish(
            conn,
            &run_id,
            RunFinish {
                status: settlement.status,
                exit_code: settlement.exit_code,
                session_id: session_id.clone(),
                log_json: settlement.log,
                output_commit: settlement.output_commit,
            },
            now,
        );
        if settlement.status == "succeeded" {
            report.settled_succeeded += 1;
        } else {
            report.settled_failed += 1;
            if let Some(reason) = settlement.reason {
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
        conn.execute(
            "INSERT INTO automation_occurrences
                (id, automation_id, scheduled_for, state, lease_owner, lease_expires_at,
                 attempt, created_at, updated_at)
             VALUES (?1, ?2, ?3, 'running', 'daemon', '2026-08-28T10:00:00.000Z', 1, ?3, ?3)",
            rusqlite::params![format!("occ-{session_id}"), automation_id, now],
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
        conn.execute(
            "INSERT INTO automation_runs
                (id, automation_id, occurrence_id, session_id, familiar_id, runtime,
                 status, started_at)
             VALUES (?1, ?2, ?3, ?4, 'charm', 'coven-code', 'running', ?5)",
            rusqlite::params![
                run_id,
                automation_id,
                format!("occ-{session_id}"),
                session_id,
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
    fn final_output_text_picks_the_last_output_event() {
        let (_temp, conn) = temp_store();
        session_record(&conn, "session-1", "completed", Some(0));
        event(&conn, "session-1", "output", "draft");
        event(&conn, "session-1", "input", "keep going");
        event(&conn, "session-1", "output", "final answer");

        assert_eq!(
            final_output_text(&conn, "session-1").as_deref(),
            Some("final answer")
        );
        assert_eq!(final_output_text(&conn, "session-missing"), None);
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

        let run: (String, Option<i64>, Option<String>) = conn
            .query_row(
                "SELECT status, exit_code, output_commit FROM automation_runs WHERE id = 'run-session-1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(run.0, "succeeded");
        assert_eq!(run.1, Some(0));
        assert_eq!(run.2.as_deref(), Some(target.to_str().unwrap()));
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

        let status: String = conn
            .query_row(
                "SELECT status FROM automation_runs WHERE id = 'run-session-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            status, "failed",
            "a failed delivery must not report success"
        );
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
    fn still_running_rows_are_left_alone() {
        let (_temp, conn) = temp_store();
        insert_definition(&conn, &definition("daily", None)).unwrap();
        live_run(&conn, "daily", "session-1");
        session_record(&conn, "session-1", "running", None);

        let report = settle_finished_runs(&conn, Utc::now()).unwrap();
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
