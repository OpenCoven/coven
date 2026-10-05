//! `occurrence.recover.v1`: the owner's explicit determination for an
//! occurrence the runner could not settle on its own (coven#1054, coven#857).
//!
//! An occurrence reaches `recovery_required` when a stop could not be
//! confirmed, or a launch was abandoned. The work's outcome is then ambiguous,
//! and the runner never retries ambiguous work by itself. The owner resolves
//! it with one of two determinations, as `state-machines.json` defines them:
//!
//! - `failed_deterministic` settles the occurrence and its run `failed`;
//! - `retry_with_new_attempt` opens the next attempt, carrying the ambiguous
//!   prior disposition, and plans the occurrence for dispatch.
//!
//! Either determination is refused while the latest attempt's session is
//! still `created` or `running`. That work may still be happening, and the
//! owner cancels it first, so a retry never runs alongside it. A retry also
//! needs the run's deadline to be open. A Runtime Authority run is refused:
//! settling it needs authority-evidenced receipts, which its adapter produces.
//!
//! The command has no side effect outside the store. So its adoption, checks
//! and state change share one transaction, and an exact replay returns the
//! first answer.

use chrono::{DateTime, SecondsFormat, Utc};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::Deserialize;
use serde_json::{json, Value};

use super::contract::canonical_json::{canonicalize, sha256_hex};
use super::contract::error::{ErrorCode, ErrorEnvelope};
use super::contract::types::{AdoptionKey, OccurrenceId};
use super::occurrences::settle_occurrence;
use super::runs::{record_run_finish, RunFinish};

pub const RECOVER_ACTION: &str = "coven.automations.occurrence.recover.v1";
const COMMAND: &str = "occurrence.recover.v1";
/// The attempts table's own bound on attempt numbers.
const MAX_ATTEMPT_NUMBER: i64 = 10;
const STATEMENT_MAX_CHARS: usize = 1_000;

/// The occurrence's running run: its id, authority profile, session and
/// deadline.
type RunningRun = (String, Option<String>, Option<String>, Option<String>);

/// The answer to one recovery command.
#[derive(Debug, Clone, PartialEq)]
pub enum RecoveryExecution {
    Success { payload: Value, replayed: bool },
    Rejected(ErrorEnvelope),
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RecoverRequest {
    action: String,
    adoption_key: String,
    occurrence_id: String,
    evidence_determination: EvidenceDetermination,
    #[serde(default)]
    statement: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum EvidenceDetermination {
    FailedDeterministic,
    RetryWithNewAttempt,
}

impl EvidenceDetermination {
    fn as_str(self) -> &'static str {
        match self {
            Self::FailedDeterministic => "failed_deterministic",
            Self::RetryWithNewAttempt => "retry_with_new_attempt",
        }
    }
}

/// Executes one `occurrence.recover.v1` request at `now`.
pub fn execute_occurrence_recovery(
    conn: &Connection,
    body: Value,
    now: DateTime<Utc>,
) -> Result<RecoveryExecution, String> {
    let digest = sha256_hex(
        &canonicalize(&body)
            .map_err(|error| format!("failed to canonicalize recovery request: {error:#}"))?,
    );
    let adoption_key = body
        .get("adoptionKey")
        .and_then(Value::as_str)
        .filter(|key| AdoptionKey::new((*key).to_owned()).is_ok())
        .map(ToOwned::to_owned);
    let transaction = rusqlite::Transaction::new_unchecked(conn, TransactionBehavior::Immediate)
        .map_err(|error| format!("failed to begin occurrence recovery: {error}"))?;
    if let Some(key) = adoption_key.as_deref() {
        if let Some(replay) = adopted_response(&transaction, key, &digest)? {
            return Ok(replay);
        }
    }

    let outcome = match parse(&body) {
        Ok(request) => recover_in(&transaction, &request, now)?,
        Err(error) => Err(error),
    };
    let automation_id: Option<String> = body
        .get("occurrenceId")
        .and_then(Value::as_str)
        .map(|occurrence_id| {
            transaction
                .query_row(
                    "SELECT automation_id FROM automation_occurrences WHERE id = ?1",
                    [occurrence_id],
                    |row| row.get(0),
                )
                .optional()
        })
        .transpose()
        .map_err(|error| format!("failed to resolve the recovered routine: {error}"))?
        .flatten();
    let (stored, execution) = match outcome {
        Ok(payload) => (
            json!({ "outcome": "committed", "result": payload }),
            RecoveryExecution::Success {
                payload,
                replayed: false,
            },
        ),
        Err(error) => (
            json!({ "outcome": "rejected", "error": error }),
            RecoveryExecution::Rejected(error),
        ),
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
            .map_err(|error| format!("failed to adopt the recovery command: {error}"))?;
    }
    transaction
        .commit()
        .map_err(|error| format!("failed to commit occurrence recovery: {error}"))?;
    Ok(execution)
}

/// The adopted answer for `adoption_key`, or a mismatch when the key was used
/// for anything else.
fn adopted_response(
    conn: &Connection,
    adoption_key: &str,
    digest: &str,
) -> Result<Option<RecoveryExecution>, String> {
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
        .map_err(|error| format!("failed to read the recovery adoption: {error}"))?;
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
        return Ok(used_elsewhere.then(|| RecoveryExecution::Rejected(replay_mismatch())));
    };
    if command != COMMAND || stored_digest != digest {
        return Ok(Some(RecoveryExecution::Rejected(replay_mismatch())));
    }
    let response: Value = serde_json::from_str(&response_json)
        .map_err(|error| format!("stored recovery response is invalid: {error}"))?;
    Ok(Some(match response["outcome"].as_str() {
        Some("committed") => RecoveryExecution::Success {
            payload: response["result"].clone(),
            replayed: true,
        },
        Some("rejected") => RecoveryExecution::Rejected(
            serde_json::from_value(response["error"].clone())
                .map_err(|error| format!("stored recovery error is invalid: {error}"))?,
        ),
        other => return Err(format!("stored recovery outcome {other:?} is invalid")),
    }))
}

fn parse(body: &Value) -> Result<RecoverRequest, ErrorEnvelope> {
    let request: RecoverRequest = serde_json::from_value(body.clone()).map_err(|error| {
        typed_error(
            ErrorCode::ValidationFailed,
            format!("invalid occurrence recovery request: {error}"),
        )
    })?;
    if request.action != RECOVER_ACTION {
        return Err(typed_error(
            ErrorCode::ValidationFailed,
            format!("action must be {RECOVER_ACTION}"),
        ));
    }
    if AdoptionKey::new(request.adoption_key.clone()).is_err() {
        return Err(typed_error(
            ErrorCode::ValidationFailed,
            "adoptionKey is not a valid adoption key",
        ));
    }
    if OccurrenceId::new(request.occurrence_id.clone()).is_err() {
        return Err(typed_error(
            ErrorCode::ValidationFailed,
            "occurrenceId is not a valid occurrence id",
        ));
    }
    if let Some(statement) = &request.statement {
        let chars = statement.chars().count();
        if chars == 0 || chars > STATEMENT_MAX_CHARS {
            return Err(typed_error(
                ErrorCode::ValidationFailed,
                format!("statement must be 1..={STATEMENT_MAX_CHARS} characters"),
            ));
        }
    }
    Ok(request)
}

/// The determination inside the caller's transaction: the payload, or the
/// typed refusal. An `Err` from the outer result is an internal failure.
fn recover_in(
    conn: &Connection,
    request: &RecoverRequest,
    now: DateTime<Utc>,
) -> Result<Result<Value, ErrorEnvelope>, String> {
    let refuse = |code, message: String| Ok(Err(typed_error(code, message)));
    let occurrence: Option<(String, String)> = conn
        .query_row(
            "SELECT state, automation_id FROM automation_occurrences WHERE id = ?1",
            [&request.occurrence_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|error| format!("failed to read the occurrence: {error}"))?;
    let Some((state, automation_id)) = occurrence else {
        return refuse(
            ErrorCode::NotFound,
            format!("no occurrence `{}`", request.occurrence_id),
        );
    };
    if state != "recovery_required" {
        return refuse(
            ErrorCode::IllegalTransition,
            format!("occurrence is `{state}`, not `recovery_required`"),
        );
    }
    // The run the occurrence is waiting on, with its latest attempt.
    let run: Option<RunningRun> = conn
        .query_row(
            "SELECT id, authority_profile, session_id, timeout_at
             FROM automation_runs WHERE occurrence_id = ?1 AND status = 'running'",
            [&request.occurrence_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()
        .map_err(|error| format!("failed to read the occurrence's run: {error}"))?;
    let attempt: Option<(i64, String, Option<String>)> = match &run {
        Some((run_id, ..)) => conn
            .query_row(
                "SELECT attempt.attempt_number, attempt.state, session.status
                 FROM automation_attempts AS attempt
                 LEFT JOIN sessions AS session ON session.id = attempt.session_id
                 WHERE attempt.run_id = ?1
                 ORDER BY attempt.attempt_number DESC LIMIT 1",
                [run_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()
            .map_err(|error| format!("failed to read the latest attempt: {error}"))?,
        None => None,
    };
    if run
        .as_ref()
        .is_some_and(|(_, profile, ..)| profile.is_some())
    {
        return refuse(
            ErrorCode::AuthorityRequired,
            "a Runtime Authority run settles only with authority-evidenced receipts, \
             which this command cannot produce yet"
                .to_owned(),
        );
    }
    if let Some((_, attempt_state, session_status)) = &attempt {
        if matches!(
            attempt_state.as_str(),
            "dispatching" | "started" | "observing"
        ) || matches!(session_status.as_deref(), Some("created" | "running"))
        {
            return refuse(
                ErrorCode::IllegalTransition,
                "the latest attempt may still be running; cancel it before recovering".to_owned(),
            );
        }
    }

    let statement = request.statement.as_deref();
    let reason = match statement {
        Some(statement) => format!(
            "operator recovery ({}): {statement}",
            request.evidence_determination.as_str()
        ),
        None => format!(
            "operator recovery ({})",
            request.evidence_determination.as_str()
        ),
    };
    let mut payload = json!({
        "occurrenceId": request.occurrence_id,
        "automationId": automation_id,
        "evidenceDetermination": request.evidence_determination.as_str(),
        "recoveredAt": iso(now),
    });
    if let Some(statement) = statement {
        payload["statement"] = json!(statement);
    }

    match request.evidence_determination {
        EvidenceDetermination::FailedDeterministic => {
            if !settle_occurrence(conn, &request.occurrence_id, "failed", Some(&reason), now)? {
                return Err("the occurrence changed during recovery".to_owned());
            }
            if let Some((run_id, _, session_id, _)) = &run {
                record_run_finish(
                    conn,
                    run_id,
                    RunFinish {
                        status: "failed",
                        exit_code: None,
                        session_id: session_id.clone(),
                        log_json: None,
                        output_commit: None,
                    },
                    now,
                )
                .map_err(|error| format!("failed to settle the recovered run: {error:#}"))?;
                payload["runId"] = json!(run_id);
            }
            payload["status"] = json!("failed");
        }
        EvidenceDetermination::RetryWithNewAttempt => {
            let (Some((run_id, _, _, timeout_at)), Some((attempt_number, attempt_state, _))) =
                (&run, &attempt)
            else {
                return refuse(
                    ErrorCode::IllegalTransition,
                    "the occurrence has no run to retry; settle it failed_deterministic".to_owned(),
                );
            };
            if attempt_state != "ambiguous" {
                return refuse(
                    ErrorCode::IllegalTransition,
                    format!("the latest attempt is `{attempt_state}`, not `ambiguous`"),
                );
            }
            if *attempt_number >= MAX_ATTEMPT_NUMBER {
                return refuse(
                    ErrorCode::IllegalTransition,
                    format!("the run has used all {MAX_ATTEMPT_NUMBER} attempts"),
                );
            }
            // A run without a deadline has none to pass.
            let deadline_open = match timeout_at {
                Some(timeout_at) => {
                    DateTime::parse_from_rfc3339(timeout_at).map_err(|error| {
                        format!("run `{run_id}` has an invalid timeout: {error}")
                    })? > now
                }
                None => true,
            };
            if !deadline_open {
                return refuse(
                    ErrorCode::DeadlineExceeded,
                    "the run's deadline has passed; settle it failed_deterministic".to_owned(),
                );
            }
            let lifecycle: Option<String> = conn
                .query_row(
                    "SELECT lifecycle_state FROM automation_definitions
                     WHERE id = ?1 AND tombstoned_at IS NULL",
                    [&automation_id],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|error| format!("failed to read the routine: {error}"))?;
            if lifecycle.as_deref() != Some("active") {
                return refuse(
                    ErrorCode::IllegalTransition,
                    format!("routine `{automation_id}` is not active, so it cannot be retried"),
                );
            }
            let next = attempt_number + 1;
            let attempt_id = format!("attempt-{run_id}-{next}");
            let opened = conn
                .execute(
                    "INSERT INTO automation_attempts
                        (id, run_id, occurrence_id, attempt_number, adoption_key,
                         occurrence_fence_generation, dispatch_generation, state,
                         prior_attempt_number, prior_disposition, retry_classification,
                         state_reason, not_before, opened_at)
                     SELECT ?1, ?2, ?3, ?4, ?5, attempt, 0, 'adopted',
                            ?6, 'ambiguous', 'operator_recovery', ?7, ?8, ?8
                     FROM automation_occurrences
                     WHERE id = ?3 AND state = 'recovery_required'",
                    params![
                        attempt_id,
                        run_id,
                        request.occurrence_id,
                        next,
                        format!("automation:{run_id}:{next}"),
                        attempt_number,
                        reason,
                        iso(now),
                    ],
                )
                .map_err(|error| format!("failed to open the recovery attempt: {error}"))?;
            let replanned = conn
                .execute(
                    "UPDATE automation_occurrences
                     SET state = 'planned', lease_owner = NULL, lease_expires_at = NULL,
                         failure_reason = ?2, updated_at = ?3
                     WHERE id = ?1 AND state = 'recovery_required'",
                    params![request.occurrence_id, reason, iso(now)],
                )
                .map_err(|error| format!("failed to plan the recovered occurrence: {error}"))?;
            let released = conn
                .execute(
                    "UPDATE automation_runs SET session_id = NULL
                     WHERE id = ?1 AND status = 'running'",
                    [run_id],
                )
                .map_err(|error| format!("failed to release the run's session: {error}"))?;
            if opened != 1 || replanned != 1 || released != 1 {
                return Err("the occurrence changed during recovery".to_owned());
            }
            payload["runId"] = json!(run_id);
            payload["attemptId"] = json!(attempt_id);
            payload["attemptNumber"] = json!(next);
            payload["status"] = json!("planned");
        }
    }
    Ok(Ok(payload))
}

fn replay_mismatch() -> ErrorEnvelope {
    typed_error(
        ErrorCode::AdoptionReplayMismatch,
        "adoption key was already used for a different request",
    )
}

fn typed_error(code: ErrorCode, message: impl Into<String>) -> ErrorEnvelope {
    ErrorEnvelope::try_new(code, message, false)
        .expect("occurrence recovery error messages satisfy protocol bounds")
}

fn iso(now: DateTime<Utc>) -> String {
    now.to_rfc3339_opts(SecondsFormat::Millis, true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::NoopSessionRuntime;
    use crate::automations::definition::RoutineDefinition;
    use crate::automations::occurrences::{claim_due_occurrence, insert_claimed_occurrence};
    use crate::automations::runner::{
        dispatch_claimed_occurrences_with_clock, mark_unconfirmed_stop_for_recovery,
    };
    use crate::automations::store::insert_definition;
    use chrono::TimeZone;

    const OCCURRENCE: &str = "occurrence.notes-1";

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 5, 9, 0, 0).unwrap()
    }

    fn store() -> (tempfile::TempDir, Connection) {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("store.sqlite");
        crate::store::initialize_store(&path).unwrap();
        let conn = crate::store::open_store(&path).unwrap();
        (temp, conn)
    }

    /// A store with routine `notes` whose first occurrence ran, lost its stop
    /// confirmation, and whose session the daemon has since lost.
    fn recovering() -> (tempfile::TempDir, Connection, String) {
        let (temp, conn) = store();
        let routine = RoutineDefinition::from_json(&json!({
            "schemaVersion": 1, "id": "notes", "name": "Notes", "status": "ACTIVE",
            "rrule": "FREQ=DAILY;BYHOUR=9", "timezone": "utc", "misfire": "latest",
            "overlap": "forbid", "timeoutMinutes": 30, "runtime": "coven-code",
            "cwd": temp.path().display().to_string(), "prompt": "Summarise the notes."
        }))
        .unwrap();
        insert_definition(&conn, &routine).unwrap();
        assert!(
            insert_claimed_occurrence(&conn, OCCURRENCE, "notes", "daemon", 60, now()).unwrap()
        );
        let report =
            dispatch_claimed_occurrences_with_clock(&conn, &NoopSessionRuntime, now(), now)
                .unwrap();
        let run_id = report.dispatched[0].clone();
        mark_unconfirmed_stop_for_recovery(&conn, &run_id, "timeout stop was not confirmed", now())
            .unwrap();
        conn.execute(
            "UPDATE sessions SET status = 'orphaned'
             WHERE id = (SELECT session_id FROM automation_runs WHERE id = ?1)",
            [&run_id],
        )
        .unwrap();
        (temp, conn, run_id)
    }

    fn request(key: &str, determination: &str) -> Value {
        json!({
            "action": RECOVER_ACTION, "adoptionKey": key, "occurrenceId": OCCURRENCE,
            "evidenceDetermination": determination,
            "statement": "The report job is idempotent; the timeout was a slow disk."
        })
    }

    fn refused(conn: &Connection, body: Value, at: DateTime<Utc>) -> ErrorCode {
        match execute_occurrence_recovery(conn, body, at).unwrap() {
            RecoveryExecution::Rejected(error) => error.code(),
            success => panic!("expected a refusal, got {success:?}"),
        }
    }

    fn state(conn: &Connection, sql: &str, id: &str) -> String {
        conn.query_row(sql, [id], |row| row.get(0)).unwrap()
    }

    #[test]
    fn a_failed_determination_settles_the_occurrence_and_its_run() {
        let (_temp, conn, run_id) = recovering();
        let body = request("adopt:recover:failed", "failed_deterministic");
        let RecoveryExecution::Success { payload, replayed } =
            execute_occurrence_recovery(&conn, body.clone(), now()).unwrap()
        else {
            panic!("refused");
        };
        assert!(!replayed);
        assert_eq!(payload["status"], json!("failed"));
        assert_eq!(payload["runId"], json!(run_id));
        assert_eq!(
            state(
                &conn,
                "SELECT state FROM automation_occurrences WHERE id = ?1",
                OCCURRENCE
            ),
            "failed"
        );
        assert!(state(
            &conn,
            "SELECT failure_reason FROM automation_occurrences WHERE id = ?1",
            OCCURRENCE
        )
        .contains("slow disk"));
        assert_eq!(
            state(
                &conn,
                "SELECT status FROM automation_runs WHERE id = ?1",
                &run_id
            ),
            "failed"
        );
        // An exact replay answers the same; the key with another request does not.
        assert_eq!(
            execute_occurrence_recovery(&conn, body, now()).unwrap(),
            RecoveryExecution::Success {
                payload,
                replayed: true
            }
        );
        assert_eq!(
            refused(
                &conn,
                request("adopt:recover:failed", "retry_with_new_attempt"),
                now()
            ),
            ErrorCode::AdoptionReplayMismatch
        );
        // The occurrence is settled, so there is nothing left to recover.
        assert_eq!(
            refused(
                &conn,
                request("adopt:recover:again", "failed_deterministic"),
                now()
            ),
            ErrorCode::IllegalTransition
        );
    }

    #[test]
    fn a_retry_opens_the_next_attempt_and_the_scheduler_dispatches_it() {
        let (_temp, conn, run_id) = recovering();
        let RecoveryExecution::Success { payload, .. } = execute_occurrence_recovery(
            &conn,
            request("adopt:recover:retry", "retry_with_new_attempt"),
            now(),
        )
        .unwrap() else {
            panic!("refused");
        };
        assert_eq!(payload["status"], json!("planned"));
        assert_eq!(payload["attemptNumber"], json!(2));
        let (prior, classification): (String, String) = conn
            .query_row(
                "SELECT prior_disposition, retry_classification FROM automation_attempts
                 WHERE run_id = ?1 AND attempt_number = 2",
                [&run_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            (prior.as_str(), classification.as_str()),
            ("ambiguous", "operator_recovery")
        );

        // The scheduler claims the occurrence again, and its new attempt
        // dispatches.
        assert_eq!(
            claim_due_occurrence(&conn, "notes", "daemon", 60, now()).unwrap(),
            Some(OCCURRENCE.to_owned())
        );
        let report =
            dispatch_claimed_occurrences_with_clock(&conn, &NoopSessionRuntime, now(), now)
                .unwrap();
        assert_eq!(
            report.dispatched,
            std::slice::from_ref(&run_id),
            "{:?}",
            report.failed
        );
        assert_eq!(
            state(
                &conn,
                "SELECT state FROM automation_attempts WHERE id = ?1",
                &format!("attempt-{run_id}-2")
            ),
            "started"
        );
    }

    #[test]
    fn recovery_is_refused_when_it_cannot_be_decided_safely() {
        let (_temp, conn, run_id) = recovering();
        let mut unknown = request("adopt:recover:unknown", "failed_deterministic");
        unknown["occurrenceId"] = json!("occurrence.none");
        assert_eq!(refused(&conn, unknown, now()), ErrorCode::NotFound);
        let mut malformed = request("adopt:recover:malformed", "retry_soon");
        malformed["evidenceDetermination"] = json!("retry_soon");
        assert_eq!(
            refused(&conn, malformed, now()),
            ErrorCode::ValidationFailed
        );
        // The run's deadline has passed: only a failed determination remains.
        assert_eq!(
            refused(
                &conn,
                request("adopt:recover:late", "retry_with_new_attempt"),
                now() + chrono::TimeDelta::hours(1)
            ),
            ErrorCode::DeadlineExceeded
        );
        // A paused routine is not retried.
        conn.execute(
            "UPDATE automation_definitions SET lifecycle_state = 'paused' WHERE id = 'notes'",
            [],
        )
        .unwrap();
        assert_eq!(
            refused(
                &conn,
                request("adopt:recover:paused", "retry_with_new_attempt"),
                now()
            ),
            ErrorCode::IllegalTransition
        );
        // A session that may still be running is cancelled first.
        conn.execute(
            "UPDATE sessions SET status = 'running'
             WHERE id = (SELECT session_id FROM automation_runs WHERE id = ?1)",
            [&run_id],
        )
        .unwrap();
        assert_eq!(
            refused(
                &conn,
                request("adopt:recover:live", "failed_deterministic"),
                now()
            ),
            ErrorCode::IllegalTransition
        );
        // Every refusal adopted its key, and none changed the occurrence.
        assert_eq!(
            state(
                &conn,
                "SELECT state FROM automation_occurrences WHERE id = ?1",
                OCCURRENCE
            ),
            "recovery_required"
        );
    }

    #[test]
    fn a_runtime_authority_run_needs_its_adapter() {
        let (_temp, conn) = store();
        conn.execute_batch(
            "INSERT INTO automation_occurrences (
                id, automation_id, automation_revision, definition_digest,
                scheduled_for, kind, state, attempt, created_at, updated_at
             ) VALUES (
                'occurrence.notes-1', 'notes', 1,
                '1111111111111111111111111111111111111111111111111111111111111111',
                '2026-10-05T09:00:00.000Z', 'scheduled', 'recovery_required', 1,
                '2026-10-05T09:00:00.000Z', '2026-10-05T09:00:00.000Z'
             );
             INSERT INTO automation_runs (
                id, automation_id, automation_revision, definition_digest,
                occurrence_id, authority_profile, runtime, status, started_at
             ) VALUES (
                'run.notes-1', 'notes', 1,
                '1111111111111111111111111111111111111111111111111111111111111111',
                'occurrence.notes-1', 'coven.automations.authority.v1', 'claude',
                'running', '2026-10-05T09:00:00.000Z'
             );",
        )
        .unwrap();
        assert_eq!(
            refused(
                &conn,
                request("adopt:recover:ra", "failed_deterministic"),
                now()
            ),
            ErrorCode::AuthorityRequired
        );
    }
}
