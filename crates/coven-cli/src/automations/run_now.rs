//! `occurrence.runNow.v1`: the owner runs a routine now (coven#1054,
//! coven#857).
//!
//! The command plans a manual occurrence, claims it, and dispatches it through
//! the same path as a scheduled one. The routine's lifecycle state and its
//! policies (timeout, overlap and retry quarantine) still apply. v1 has no
//! conditions, so `bypassEligibility` changes nothing.
//!
//! A launch is a side effect outside the store, so a crash must never turn a
//! replay into a second run. The adoption and the claimed occurrence are
//! committed together before the launch, and the occurrence's id derives from
//! the adoption key. A replay therefore finds the same occurrence and reports
//! its run, and never launches again.

use chrono::{DateTime, SecondsFormat, Utc};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::Deserialize;
use serde_json::{json, Value};

use super::contract::canonical_json::{canonicalize, sha256_hex};
use super::contract::error::{ErrorCode, ErrorEnvelope};
use super::contract::types::{AdoptionKey, AutomationId};
use super::occurrences::insert_claimed_occurrence;
use super::runs::is_retry_quarantined;
use crate::api::SessionRuntime;

pub const RUN_NOW_ACTION: &str = "coven.automations.occurrence.runNow.v1";
const COMMAND: &str = "occurrence.runNow.v1";
const NOTE_MAX_CHARS: usize = 500;
/// How long the manual claim is held for its launch.
const LEASE_MINUTES: i64 = 60;

/// The answer to one runNow command.
#[derive(Debug, Clone, PartialEq)]
pub enum RunNowExecution {
    Success { payload: Value, replayed: bool },
    Rejected(ErrorEnvelope),
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RunNowRequest {
    action: String,
    adoption_key: String,
    automation_id: String,
    #[serde(default)]
    note: Option<String>,
    #[serde(default)]
    bypass_eligibility: bool,
}

/// Executes one `occurrence.runNow.v1` request at `now`.
pub fn execute_run_now(
    conn: &Connection,
    runtime: &dyn SessionRuntime,
    body: Value,
    now: DateTime<Utc>,
) -> Result<RunNowExecution, String> {
    let digest = sha256_hex(
        &canonicalize(&body)
            .map_err(|error| format!("failed to canonicalize runNow request: {error:#}"))?,
    );
    let adoption_key = body
        .get("adoptionKey")
        .and_then(Value::as_str)
        .filter(|key| AdoptionKey::new((*key).to_owned()).is_ok())
        .map(ToOwned::to_owned);

    let transaction = rusqlite::Transaction::new_unchecked(conn, TransactionBehavior::Immediate)
        .map_err(|error| format!("failed to begin runNow: {error}"))?;
    if let Some(key) = adoption_key.as_deref() {
        match adopted(&transaction, key, &digest)? {
            Some(Adopted::Claimed(occurrence_id)) => {
                transaction
                    .commit()
                    .map_err(|error| format!("failed to close runNow replay: {error}"))?;
                return Ok(RunNowExecution::Success {
                    payload: live(conn, &occurrence_id)?,
                    replayed: true,
                });
            }
            Some(Adopted::Answered(answer)) => return Ok(answer),
            None => {}
        }
    }
    let claimed = match parse(&body) {
        Ok(request) => claim_in(&transaction, &request, now)?,
        Err(error) => Err(error),
    };
    let automation_id = body.get("automationId").and_then(Value::as_str);
    let stored = match &claimed {
        Ok((occurrence_id, _)) => {
            json!({ "outcome": "committed", "result": { "occurrenceId": occurrence_id } })
        }
        Err(error) => json!({ "outcome": "rejected", "error": error }),
    };
    if let Some(key) = adoption_key.as_deref() {
        transaction
            .execute(
                "INSERT INTO automation_command_adoptions (
                    adoption_key, command, automation_id, request_digest, outcome,
                    revision, response_json, adopted_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, NULL, ?6, ?7)",
                params![
                    key,
                    COMMAND,
                    automation_id,
                    digest,
                    stored["outcome"].as_str(),
                    stored.to_string(),
                    iso(now)
                ],
            )
            .map_err(|error| format!("failed to adopt the runNow command: {error}"))?;
    }
    transaction
        .commit()
        .map_err(|error| format!("failed to commit runNow: {error}"))?;
    let (occurrence_id, definition) = match claimed {
        Ok(claimed) => claimed,
        Err(error) => return Ok(RunNowExecution::Rejected(error)),
    };

    // The launch, through the same path as a scheduled occurrence. A refusal
    // settles the occurrence failed, and the answer says why.
    let mut payload = match super::runner::dispatch_manual_occurrence(
        conn,
        runtime,
        &definition,
        &occurrence_id,
        now,
    ) {
        Ok(_) => live(conn, &occurrence_id)?,
        Err(error) => {
            let mut payload = live(conn, &occurrence_id)?;
            payload["error"] = json!(error);
            payload
        }
    };
    if let Some(note) = definition_note(&body) {
        payload["note"] = json!(note);
    }
    Ok(RunNowExecution::Success {
        payload,
        replayed: false,
    })
}

enum Adopted {
    /// The command claimed this occurrence; it may or may not have launched.
    Claimed(String),
    /// A stored refusal, or a mismatch for a key used for something else.
    Answered(RunNowExecution),
}

fn adopted(conn: &Connection, adoption_key: &str, digest: &str) -> Result<Option<Adopted>, String> {
    let row = conn
        .query_row(
            "SELECT command, request_digest, response_json
             FROM automation_command_adoptions WHERE adoption_key = ?1",
            [adoption_key],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .optional()
        .map_err(|error| format!("failed to read the runNow adoption: {error}"))?;
    let Some((command, stored_digest, response_json)) = row else {
        let used_elsewhere =
            super::command_adoption::attempt_adoption_key_exists(conn, adoption_key)
                .map_err(|error| format!("failed to inspect attempt adoption keys: {error:#}"))?
                || conn
                    .query_row(
                        "SELECT 1 FROM automation_command_reservations WHERE adoption_key = ?1",
                        [adoption_key],
                        |_| Ok(()),
                    )
                    .optional()
                    .map_err(|error| format!("failed to inspect command reservations: {error}"))?
                    .is_some();
        return Ok(
            used_elsewhere.then(|| Adopted::Answered(RunNowExecution::Rejected(replay_mismatch())))
        );
    };
    if command != COMMAND || stored_digest != digest {
        return Ok(Some(Adopted::Answered(RunNowExecution::Rejected(
            replay_mismatch(),
        ))));
    }
    let response: Value = serde_json::from_str(&response_json)
        .map_err(|error| format!("stored runNow response is invalid: {error}"))?;
    Ok(Some(match response["outcome"].as_str() {
        Some("committed") => Adopted::Claimed(
            response["result"]["occurrenceId"]
                .as_str()
                .ok_or("stored runNow response has no occurrence")?
                .to_owned(),
        ),
        Some("rejected") => Adopted::Answered(RunNowExecution::Rejected(
            serde_json::from_value(response["error"].clone())
                .map_err(|error| format!("stored runNow error is invalid: {error}"))?,
        )),
        other => return Err(format!("stored runNow outcome {other:?} is invalid")),
    }))
}

fn parse(body: &Value) -> Result<RunNowRequest, ErrorEnvelope> {
    let request: RunNowRequest = serde_json::from_value(body.clone()).map_err(|error| {
        typed_error(
            ErrorCode::ValidationFailed,
            format!("invalid runNow request: {error}"),
        )
    })?;
    if request.action != RUN_NOW_ACTION {
        return Err(typed_error(
            ErrorCode::ValidationFailed,
            format!("action must be {RUN_NOW_ACTION}"),
        ));
    }
    if AdoptionKey::new(request.adoption_key.clone()).is_err() {
        return Err(typed_error(
            ErrorCode::ValidationFailed,
            "adoptionKey is not a valid adoption key",
        ));
    }
    if AutomationId::new(request.automation_id.clone()).is_err() {
        return Err(typed_error(
            ErrorCode::ValidationFailed,
            "automationId is not a valid automation id",
        ));
    }
    if request
        .note
        .as_ref()
        .is_some_and(|note| note.chars().count() > NOTE_MAX_CHARS)
    {
        return Err(typed_error(
            ErrorCode::ValidationFailed,
            format!("note must be at most {NOTE_MAX_CHARS} characters"),
        ));
    }
    // v1 has no conditions to evaluate, so eligibility is the same either way.
    let _ = request.bypass_eligibility;
    Ok(request)
}

/// Claims a manual occurrence for the request, inside the caller's
/// transaction: the occurrence's id and the routine, or the refusal.
fn claim_in(
    conn: &Connection,
    request: &RunNowRequest,
    now: DateTime<Utc>,
) -> Result<Result<(String, super::definition::RoutineDefinition), ErrorEnvelope>, String> {
    let refuse = |code, message: String| Ok(Err(typed_error(code, message)));
    let row: Option<(String, Option<String>, String)> = conn
        .query_row(
            "SELECT definition_json, tombstoned_at, lifecycle_state
             FROM automation_definitions WHERE id = ?1",
            [&request.automation_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(|error| format!("failed to read the routine: {error}"))?;
    let Some((definition_json, tombstoned_at, lifecycle)) = row else {
        return refuse(
            ErrorCode::NotFound,
            format!("no routine `{}`", request.automation_id),
        );
    };
    if tombstoned_at.is_some() {
        return refuse(
            ErrorCode::GoneTombstoned,
            format!("routine `{}` is tombstoned", request.automation_id),
        );
    }
    if lifecycle != "active" {
        return refuse(
            ErrorCode::IllegalTransition,
            format!(
                "routine `{}` is `{lifecycle}`, not active",
                request.automation_id
            ),
        );
    }
    if is_retry_quarantined(conn, &request.automation_id)
        .map_err(|error| format!("failed to read the routine's quarantine: {error:#}"))?
    {
        return refuse(
            ErrorCode::IllegalTransition,
            "the routine is quarantined after retry exhaustion; unquarantine it first".to_owned(),
        );
    }
    let definition: super::definition::RoutineDefinition =
        serde_json::from_str(&definition_json)
            .map_err(|error| format!("stored routine is invalid: {error}"))?;
    if definition
        .cwd
        .as_deref()
        .map(str::trim)
        .is_none_or(str::is_empty)
    {
        return refuse(
            ErrorCode::ValidationFailed,
            "the routine has no cwd; add one before running it".to_owned(),
        );
    }
    let occurrence_id = format!(
        "occ-manual-{}",
        &sha256_hex(request.adoption_key.as_bytes())[..32]
    );
    if !insert_claimed_occurrence(
        conn,
        &occurrence_id,
        &request.automation_id,
        "manual",
        LEASE_MINUTES,
        now,
    )? {
        return refuse(
            ErrorCode::OverlapForbidden,
            "the routine already has a nonterminal run; overlap is forbidden".to_owned(),
        );
    }
    Ok(Ok((occurrence_id, definition)))
}

/// The occurrence and its run as they stand: a replay reports what the first
/// answer started.
fn live(conn: &Connection, occurrence_id: &str) -> Result<Value, String> {
    let (automation_id, state): (String, String) = conn
        .query_row(
            "SELECT automation_id, state FROM automation_occurrences WHERE id = ?1",
            [occurrence_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|error| format!("failed to read the run-now occurrence: {error}"))?;
    let run: Option<(String, String, Option<String>)> = conn
        .query_row(
            "SELECT id, status, session_id FROM automation_runs
             WHERE occurrence_id = ?1 ORDER BY started_at DESC LIMIT 1",
            [occurrence_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(|error| format!("failed to read the run-now run: {error}"))?;
    let mut payload = json!({
        "occurrenceId": occurrence_id,
        "automationId": automation_id,
        "occurrenceState": state,
    });
    if let Some((run_id, status, session_id)) = run {
        payload["runId"] = json!(run_id);
        payload["status"] = json!(status);
        payload["sessionId"] = json!(session_id);
    }
    Ok(payload)
}

fn definition_note(body: &Value) -> Option<&str> {
    body.get("note").and_then(Value::as_str)
}

fn replay_mismatch() -> ErrorEnvelope {
    typed_error(
        ErrorCode::AdoptionReplayMismatch,
        "adoption key was already used for a different request",
    )
}

fn typed_error(code: ErrorCode, message: impl Into<String>) -> ErrorEnvelope {
    ErrorEnvelope::try_new(code, message, false)
        .expect("runNow error messages satisfy protocol bounds")
}

fn iso(now: DateTime<Utc>) -> String {
    now.to_rfc3339_opts(SecondsFormat::Millis, true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::SessionLaunch;
    use crate::automations::definition::RoutineDefinition;
    use crate::automations::store::insert_definition;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Counts launches; each one establishes ownership, as a live runtime does.
    #[derive(Default)]
    struct CountingRuntime(AtomicUsize);

    impl SessionRuntime for CountingRuntime {
        fn launch_session(&self, _launch: &SessionLaunch) -> anyhow::Result<()> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        fn send_input(&self, _session_id: &str, _payload: &Value) -> anyhow::Result<()> {
            Ok(())
        }

        fn kill_session(&self, _session_id: &str) -> anyhow::Result<()> {
            Ok(())
        }
    }

    fn store(status: &str) -> (tempfile::TempDir, Connection) {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("store.sqlite");
        crate::store::initialize_store(&path).unwrap();
        let conn = crate::store::open_store(&path).unwrap();
        let routine = RoutineDefinition::from_json(&json!({
            "schemaVersion": 1, "id": "notes", "name": "Notes", "status": status,
            "rrule": "FREQ=DAILY;BYHOUR=9", "timezone": "utc", "misfire": "latest",
            "overlap": "forbid", "timeoutMinutes": 30, "runtime": "coven-code",
            "cwd": temp.path().display().to_string(), "prompt": "Summarise the notes."
        }))
        .unwrap();
        insert_definition(&conn, &routine).unwrap();
        (temp, conn)
    }

    fn request(key: &str) -> Value {
        json!({ "action": RUN_NOW_ACTION, "adoptionKey": key, "automationId": "notes",
                "note": "before the review" })
    }

    fn refused(conn: &Connection, runtime: &CountingRuntime, body: Value) -> ErrorCode {
        match execute_run_now(conn, runtime, body, Utc::now()).unwrap() {
            RunNowExecution::Rejected(error) => error.code(),
            success => panic!("expected a refusal, got {success:?}"),
        }
    }

    fn runs(conn: &Connection) -> i64 {
        conn.query_row("SELECT COUNT(*) FROM automation_runs", [], |row| row.get(0))
            .unwrap()
    }

    #[test]
    fn an_active_routine_runs_once_per_adoption() {
        let (_temp, conn) = store("ACTIVE");
        let runtime = CountingRuntime::default();
        let RunNowExecution::Success { payload, replayed } =
            execute_run_now(&conn, &runtime, request("adopt:run:1"), Utc::now()).unwrap()
        else {
            panic!("refused");
        };
        assert!(!replayed);
        assert_eq!(payload["status"], json!("running"));
        assert_eq!(payload["note"], json!("before the review"));
        assert_eq!(runtime.0.load(Ordering::SeqCst), 1);

        // An exact replay reports the same run and launches nothing.
        let RunNowExecution::Success {
            payload: replay,
            replayed,
        } = execute_run_now(&conn, &runtime, request("adopt:run:1"), Utc::now()).unwrap()
        else {
            panic!("refused");
        };
        assert!(replayed);
        assert_eq!(replay["runId"], payload["runId"]);
        assert_eq!(replay["occurrenceId"], payload["occurrenceId"]);
        assert_eq!((runtime.0.load(Ordering::SeqCst), runs(&conn)), (1, 1));

        // The key with another request is a mismatch; another run overlaps.
        let mut changed = request("adopt:run:1");
        changed["note"] = json!("changed");
        assert_eq!(
            refused(&conn, &runtime, changed),
            ErrorCode::AdoptionReplayMismatch
        );
        assert_eq!(
            refused(&conn, &runtime, request("adopt:run:2")),
            ErrorCode::OverlapForbidden
        );
        assert_eq!(runtime.0.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn an_adoption_whose_launch_never_happened_replays_without_launching() {
        let (_temp, conn) = store("ACTIVE");
        let runtime = CountingRuntime::default();
        // The first answer committed its claim, then the daemon stopped before
        // the launch.
        let body = request("adopt:run:crash");
        let digest = sha256_hex(&canonicalize(&body).unwrap());
        let occurrence = format!("occ-manual-{}", &sha256_hex(b"adopt:run:crash")[..32]);
        assert!(
            insert_claimed_occurrence(&conn, &occurrence, "notes", "manual", 60, Utc::now())
                .unwrap()
        );
        conn.execute(
            "INSERT INTO automation_command_adoptions (
                adoption_key, command, automation_id, request_digest, outcome,
                revision, response_json, adopted_at
             ) VALUES ('adopt:run:crash', ?1, 'notes', ?2, 'committed', NULL, ?3, ?4)",
            params![
                COMMAND,
                digest,
                json!({ "outcome": "committed", "result": { "occurrenceId": occurrence } })
                    .to_string(),
                iso(Utc::now())
            ],
        )
        .unwrap();
        let RunNowExecution::Success { payload, replayed } =
            execute_run_now(&conn, &runtime, body, Utc::now()).unwrap()
        else {
            panic!("refused");
        };
        assert!(replayed);
        assert_eq!(payload["occurrenceState"], json!("claimed"));
        assert_eq!((runtime.0.load(Ordering::SeqCst), runs(&conn)), (0, 0));
    }

    #[test]
    fn only_an_active_routine_runs() {
        let runtime = CountingRuntime::default();
        let (_temp, paused) = store("PAUSED");
        assert_eq!(
            refused(&paused, &runtime, request("adopt:run:paused")),
            ErrorCode::IllegalTransition
        );
        let (_temp, conn) = store("ACTIVE");
        let mut unknown = request("adopt:run:unknown");
        unknown["automationId"] = json!("missing");
        assert_eq!(refused(&conn, &runtime, unknown), ErrorCode::NotFound);
        let mut malformed = request("adopt:run:malformed");
        malformed["priority"] = json!("high");
        assert_eq!(
            refused(&conn, &runtime, malformed),
            ErrorCode::ValidationFailed
        );
        conn.execute(
            "UPDATE automation_definitions SET tombstoned_at = '2026-10-05T00:00:00.000Z'",
            [],
        )
        .unwrap();
        assert_eq!(
            refused(&conn, &runtime, request("adopt:run:tombstoned")),
            ErrorCode::GoneTombstoned
        );
        assert_eq!(runtime.0.load(Ordering::SeqCst), 0);
    }
}
