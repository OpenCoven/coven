//! `occurrence.cancel.v1` and `attempt.cancel.v1` (coven#1054, coven#857).
//!
//! Cancellation is a request until it is acknowledged or reconciled, as the
//! contract's invariant says. How these commands handle it depends on what
//! has started:
//!
//! - **Nothing dispatched.** For an occurrence still planned, eligible or
//!   claimed, or an attempt still adopted and waiting, the cancellation is
//!   acknowledged at once. The occurrence settles `cancelled`, and so do any
//!   waiting attempt and its run, in one transaction with the command's
//!   adoption.
//! - **An attempt dispatching or running.** The command hands the run to the
//!   existing run cancellation (`run.cancel.v1`), which kills the session
//!   behind its stop fence and settles it, or answers `CANCEL_PENDING`. It does
//!   so under a key derived from the command's own. The command's adoption
//!   records the exact run cancellation it made, so a replay makes the same
//!   one and gets that cancellation's own adopted answer.
//!
//! An occurrence that needs recovery is recovered with `occurrence.recover.v1`
//! instead. Anything already settled refuses.

use chrono::{DateTime, SecondsFormat, Utc};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::Deserialize;
use serde_json::{json, Value};

use super::cancellation::{execute_run_cancellation, CancellationExecution};
use super::contract::canonical_json::{canonicalize, sha256_hex};
use super::contract::error::{ErrorCode, ErrorEnvelope};
use super::contract::types::{AdoptionKey, AttemptId, OccurrenceId};
use super::occurrences::settle_occurrence;
use super::runs::{record_run_finish, RunFinish};
use crate::api::SessionRuntime;

pub const OCCURRENCE_CANCEL_ACTION: &str = "coven.automations.occurrence.cancel.v1";
pub const ATTEMPT_CANCEL_ACTION: &str = "coven.automations.attempt.cancel.v1";
const REASON_MAX_CHARS: usize = 500;

/// The answer to one cancel command.
#[derive(Debug, Clone, PartialEq)]
pub enum CancelExecution {
    Success { payload: Value, replayed: bool },
    Rejected(ErrorEnvelope),
}

/// What a command cancels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Target {
    Occurrence,
    Attempt,
}

impl Target {
    fn action(self) -> &'static str {
        match self {
            Self::Occurrence => OCCURRENCE_CANCEL_ACTION,
            Self::Attempt => ATTEMPT_CANCEL_ACTION,
        }
    }

    fn command(self) -> &'static str {
        match self {
            Self::Occurrence => "occurrence.cancel.v1",
            Self::Attempt => "attempt.cancel.v1",
        }
    }

    fn id_field(self) -> &'static str {
        match self {
            Self::Occurrence => "occurrenceId",
            Self::Attempt => "attemptId",
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct OccurrenceCancelRequest {
    action: String,
    adoption_key: String,
    occurrence_id: String,
    #[serde(default)]
    reason: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AttemptCancelRequest {
    action: String,
    adoption_key: String,
    attempt_id: String,
    #[serde(default)]
    reason: Option<String>,
}

/// One validated request: the target's id and the reason.
struct CancelRequest {
    adoption_key: String,
    target_id: String,
    reason: Option<String>,
}

/// What the store says about the target, read in the command's transaction.
enum Plan {
    /// Settled here, with this payload.
    Settled(Value),
    /// A run cancellation must stop running work: the body to send it.
    Delegate(Value),
}

pub fn execute_occurrence_cancel(
    conn: &Connection,
    runtime: &dyn SessionRuntime,
    body: Value,
    now: DateTime<Utc>,
) -> Result<CancelExecution, String> {
    execute(conn, runtime, Target::Occurrence, body, now)
}

pub fn execute_attempt_cancel(
    conn: &Connection,
    runtime: &dyn SessionRuntime,
    body: Value,
    now: DateTime<Utc>,
) -> Result<CancelExecution, String> {
    execute(conn, runtime, Target::Attempt, body, now)
}

fn execute(
    conn: &Connection,
    runtime: &dyn SessionRuntime,
    target: Target,
    body: Value,
    now: DateTime<Utc>,
) -> Result<CancelExecution, String> {
    let digest = sha256_hex(
        &canonicalize(&body)
            .map_err(|error| format!("failed to canonicalize cancel request: {error:#}"))?,
    );
    let adoption_key = body
        .get("adoptionKey")
        .and_then(Value::as_str)
        .filter(|key| AdoptionKey::new((*key).to_owned()).is_ok())
        .map(ToOwned::to_owned);
    let transaction = rusqlite::Transaction::new_unchecked(conn, TransactionBehavior::Immediate)
        .map_err(|error| format!("failed to begin cancellation: {error}"))?;
    if let Some(key) = adoption_key.as_deref() {
        match adopted(&transaction, target, key, &digest)? {
            Some(Adopted::Delegated(run_cancel)) => {
                transaction
                    .commit()
                    .map_err(|error| format!("failed to close cancel replay: {error}"))?;
                return delegate(conn, runtime, target, &body, &run_cancel, now, true);
            }
            Some(Adopted::Answered(answer)) => return Ok(answer),
            None => {}
        }
    }
    let plan = match parse(target, &body) {
        Ok(request) => plan_in(&transaction, target, &request, now)?,
        Err(error) => Err(error),
    };
    let (stored, answer) = match &plan {
        Ok(Plan::Settled(payload)) => (
            json!({ "outcome": "committed", "result": payload }),
            Some(CancelExecution::Success {
                payload: payload.clone(),
                replayed: false,
            }),
        ),
        Ok(Plan::Delegate(run_cancel)) => (
            json!({ "outcome": "committed", "result": { "runCancellation": run_cancel } }),
            None,
        ),
        Err(error) => (
            json!({ "outcome": "rejected", "error": error }),
            Some(CancelExecution::Rejected(error.clone())),
        ),
    };
    if let Some(key) = adoption_key.as_deref() {
        let automation_id: Option<String> = automation_of(&transaction, target, &body)?;
        transaction
            .execute(
                "INSERT INTO automation_command_adoptions (
                    adoption_key, command, automation_id, request_digest, outcome,
                    revision, response_json, adopted_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, NULL, ?6, ?7)",
                params![
                    key,
                    target.command(),
                    automation_id,
                    digest,
                    stored["outcome"].as_str(),
                    stored.to_string(),
                    iso(now)
                ],
            )
            .map_err(|error| format!("failed to adopt the cancel command: {error}"))?;
    }
    transaction
        .commit()
        .map_err(|error| format!("failed to commit cancellation: {error}"))?;
    match (plan, answer) {
        (Ok(Plan::Delegate(run_cancel)), _) => {
            delegate(conn, runtime, target, &body, &run_cancel, now, false)
        }
        (_, Some(answer)) => Ok(answer),
        _ => Err("a cancellation produced no answer".to_owned()),
    }
}

/// Sends the run cancellation and answers with its outcome.
fn delegate(
    conn: &Connection,
    runtime: &dyn SessionRuntime,
    target: Target,
    body: &Value,
    run_cancel: &Value,
    now: DateTime<Utc>,
    replayed: bool,
) -> Result<CancelExecution, String> {
    // The run owns its intended session ID before the runtime publishes the
    // attempt binding. Keep the immutable delegated target replayable during
    // that window; run.cancel would otherwise durably reject the missing
    // binding. Publication only advances this exact attempt, so a retry cannot
    // redirect this command to a later session.
    let awaiting_ownership: bool = conn
        .query_row(
            "SELECT EXISTS (
                SELECT 1 FROM automation_attempts a
                JOIN automation_runs r ON r.id = a.run_id
                WHERE r.id = ?1 AND a.id = ?2 AND r.session_id = ?3
                  AND r.status = 'running' AND a.state = 'dispatching'
                  AND a.session_id IS NULL
             )",
            params![
                run_cancel["runId"].as_str(),
                run_cancel["attemptId"].as_str(),
                run_cancel["runtimeCorrelation"]["sessionId"].as_str(),
            ],
            |row| row.get(0),
        )
        .map_err(|error| format!("failed to inspect cancellation ownership: {error}"))?;
    if awaiting_ownership {
        return Ok(CancelExecution::Rejected(
            ErrorEnvelope::try_new(
                ErrorCode::CancelPending,
                "the attempt is publishing runtime ownership; replay this cancellation once it has started",
                true,
            )
            .expect("the pending ownership message satisfies protocol bounds"),
        ));
    }
    match execute_run_cancellation(conn, runtime, run_cancel.clone(), now)? {
        CancellationExecution::Success(success) => {
            let mut payload = success.payload;
            payload[target.id_field()] = body[target.id_field()].clone();
            Ok(CancelExecution::Success {
                payload,
                replayed: replayed || success.replayed,
            })
        }
        CancellationExecution::Rejected(error) => Ok(CancelExecution::Rejected(error)),
    }
}

enum Adopted {
    Delegated(Value),
    Answered(CancelExecution),
}

fn adopted(
    conn: &Connection,
    target: Target,
    adoption_key: &str,
    digest: &str,
) -> Result<Option<Adopted>, String> {
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
        .map_err(|error| format!("failed to read the cancel adoption: {error}"))?;
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
            used_elsewhere.then(|| Adopted::Answered(CancelExecution::Rejected(replay_mismatch())))
        );
    };
    if command != target.command() || stored_digest != digest {
        return Ok(Some(Adopted::Answered(CancelExecution::Rejected(
            replay_mismatch(),
        ))));
    }
    let response: Value = serde_json::from_str(&response_json)
        .map_err(|error| format!("stored cancel response is invalid: {error}"))?;
    Ok(Some(match response["outcome"].as_str() {
        Some("committed") => match response["result"].get("runCancellation") {
            Some(run_cancel) => Adopted::Delegated(run_cancel.clone()),
            None => Adopted::Answered(CancelExecution::Success {
                payload: response["result"].clone(),
                replayed: true,
            }),
        },
        Some("rejected") => Adopted::Answered(CancelExecution::Rejected(
            serde_json::from_value(response["error"].clone())
                .map_err(|error| format!("stored cancel error is invalid: {error}"))?,
        )),
        other => return Err(format!("stored cancel outcome {other:?} is invalid")),
    }))
}

fn parse(target: Target, body: &Value) -> Result<CancelRequest, ErrorEnvelope> {
    let invalid = |message: String| typed_error(ErrorCode::ValidationFailed, message);
    let (action, adoption_key, target_id, reason) = match target {
        Target::Occurrence => {
            let request: OccurrenceCancelRequest = serde_json::from_value(body.clone())
                .map_err(|error| invalid(format!("invalid occurrence cancel request: {error}")))?;
            if OccurrenceId::new(request.occurrence_id.clone()).is_err() {
                return Err(invalid(
                    "occurrenceId is not a valid occurrence id".to_owned(),
                ));
            }
            (
                request.action,
                request.adoption_key,
                request.occurrence_id,
                request.reason,
            )
        }
        Target::Attempt => {
            let request: AttemptCancelRequest = serde_json::from_value(body.clone())
                .map_err(|error| invalid(format!("invalid attempt cancel request: {error}")))?;
            if AttemptId::new(request.attempt_id.clone()).is_err() {
                return Err(invalid("attemptId is not a valid attempt id".to_owned()));
            }
            (
                request.action,
                request.adoption_key,
                request.attempt_id,
                request.reason,
            )
        }
    };
    if action != target.action() {
        return Err(invalid(format!("action must be {}", target.action())));
    }
    if AdoptionKey::new(adoption_key.clone()).is_err() {
        return Err(invalid(
            "adoptionKey is not a valid adoption key".to_owned(),
        ));
    }
    if reason
        .as_ref()
        .is_some_and(|reason| reason.chars().count() > REASON_MAX_CHARS)
    {
        return Err(invalid(format!(
            "reason must be at most {REASON_MAX_CHARS} characters"
        )));
    }
    Ok(CancelRequest {
        adoption_key,
        target_id,
        reason,
    })
}

/// The target's state, and what cancelling it takes. Settling happens here,
/// in the caller's transaction.
fn plan_in(
    conn: &Connection,
    target: Target,
    request: &CancelRequest,
    now: DateTime<Utc>,
) -> Result<Result<Plan, ErrorEnvelope>, String> {
    let refuse = |code, message: String| Ok(Err(typed_error(code, message)));
    // The occurrence, its running run, and that run's latest attempt.
    let occurrence_id: Option<String> = match target {
        Target::Occurrence => Some(request.target_id.clone()),
        Target::Attempt => conn
            .query_row(
                "SELECT occurrence_id FROM automation_attempts WHERE id = ?1",
                [&request.target_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| format!("failed to read the attempt: {error}"))?,
    };
    let Some(occurrence_id) = occurrence_id else {
        return refuse(
            ErrorCode::NotFound,
            format!("no attempt `{}`", request.target_id),
        );
    };
    let Some(occurrence_state) = conn
        .query_row(
            "SELECT state FROM automation_occurrences WHERE id = ?1",
            [&occurrence_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|error| format!("failed to read the occurrence: {error}"))?
    else {
        return refuse(
            ErrorCode::NotFound,
            format!("no occurrence `{occurrence_id}`"),
        );
    };
    let run: Option<(String, Option<String>)> = conn
        .query_row(
            "SELECT id, session_id FROM automation_runs
             WHERE occurrence_id = ?1 AND status = 'running'",
            [&occurrence_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|error| format!("failed to read the occurrence's run: {error}"))?;
    let latest: Option<(String, String)> = match &run {
        Some((run_id, _)) => conn
            .query_row(
                "SELECT id, state FROM automation_attempts
                 WHERE run_id = ?1 ORDER BY attempt_number DESC LIMIT 1",
                [run_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(|error| format!("failed to read the latest attempt: {error}"))?,
        None => None,
    };
    if target == Target::Attempt && latest.as_ref().map(|(id, _)| id) != Some(&request.target_id) {
        return refuse(
            ErrorCode::IllegalTransition,
            "only a running run's current attempt can be cancelled".to_owned(),
        );
    }
    if occurrence_state == "recovery_required" {
        return refuse(
            ErrorCode::IllegalTransition,
            "the occurrence needs recovery; use occurrence.recover.v1".to_owned(),
        );
    }
    let reason = request
        .reason
        .clone()
        .filter(|reason| !reason.trim().is_empty())
        .unwrap_or_else(|| "cancelled by the owner".to_owned());
    match latest
        .as_ref()
        .map(|(id, state)| (id.as_str(), state.as_str()))
    {
        // Running work: the run cancellation stops it.
        Some((attempt_id, "dispatching" | "started" | "observing")) => {
            let (run_id, session_id) = run.clone().expect("a latest attempt has its run");
            let Some(session_id) = session_id else {
                return refuse(
                    ErrorCode::CancelPending,
                    "the attempt has no session yet; retry once it has started".to_owned(),
                );
            };
            let principal = super::owner_grants::owner_principal_id(conn, &iso(now))
                .map_err(|error| format!("failed to read the owner principal: {error:#}"))?;
            Ok(Ok(Plan::Delegate(json!({
                "action": "coven.automations.run.cancel.v1",
                "adoptionKey": format!("rc:{}", &sha256_hex(request.adoption_key.as_bytes())[..40]),
                "runId": run_id,
                "attemptId": attempt_id,
                "runtimeCorrelation": { "sessionId": session_id },
                "scope": "run",
                "reason": reason,
                "requestedBy": { "principalId": principal },
            }))))
        }
        // A waiting attempt, or nothing dispatched yet: acknowledged now.
        latest
            if matches!(
                occurrence_state.as_str(),
                "planned" | "eligible" | "claimed"
            ) && latest.is_none_or(|(_, state)| state == "adopted") =>
        {
            let mut payload = json!({
                "occurrenceId": occurrence_id,
                "status": "cancelled",
                "reason": reason,
                "cancelledAt": iso(now),
            });
            if let (Some((attempt_id, _)), Some((run_id, session_id))) = (latest, &run) {
                let timeout_at: Option<String> = conn
                    .query_row(
                        "SELECT timeout_at FROM automation_runs WHERE id = ?1",
                        [run_id],
                        |row| row.get(0),
                    )
                    .map_err(|error| format!("failed to read cancellation deadline: {error}"))?;
                let expired = timeout_at
                    .map(|value| DateTime::parse_from_rfc3339(&value))
                    .transpose()
                    .map_err(|error| format!("invalid cancellation deadline: {error}"))?
                    .is_some_and(|deadline| deadline <= now);
                if expired {
                    super::runner::settle_waiting_retry_timeout_in(
                        conn,
                        run_id,
                        &occurrence_id,
                        now,
                    )?;
                    return refuse(
                        ErrorCode::IllegalTransition,
                        "the targeted automation run reached its timeout before cancellation"
                            .to_owned(),
                    );
                }
                let cancelled = conn
                    .execute(
                        "UPDATE automation_attempts
                         SET state = 'cancelled', failure_class = 'cancelled',
                             state_reason = ?2, settled_at = ?3
                         WHERE id = ?1 AND state = 'adopted'",
                        params![attempt_id, reason, iso(now)],
                    )
                    .map_err(|error| format!("failed to cancel the waiting attempt: {error}"))?;
                let finished = record_run_finish(
                    conn,
                    run_id,
                    RunFinish {
                        status: "cancelled",
                        exit_code: None,
                        session_id: session_id.clone(),
                        log_json: None,
                        output_commit: None,
                    },
                    now,
                )
                .map_err(|error| format!("failed to cancel the run: {error:#}"))?;
                if cancelled != 1 || !finished {
                    return Err("the run changed during cancellation".to_owned());
                }
                payload["runId"] = json!(run_id);
                payload["attemptId"] = json!(attempt_id);
            }
            // `settle_occurrence` settles a claimed one; a planned or eligible
            // one is cancelled before any claim, as the state machine allows.
            let settled = if occurrence_state == "claimed" {
                settle_occurrence(conn, &occurrence_id, "cancelled", Some(&reason), now)?
            } else {
                conn.execute(
                    "UPDATE automation_occurrences
                     SET state = 'cancelled', failure_reason = ?2, lease_owner = NULL,
                         lease_expires_at = NULL, updated_at = ?3
                     WHERE id = ?1 AND state IN ('planned', 'eligible')",
                    params![occurrence_id, reason, iso(now)],
                )
                .map_err(|error| format!("failed to cancel the occurrence: {error}"))?
                    == 1
            };
            if !settled {
                return Err("the occurrence changed during cancellation".to_owned());
            }
            Ok(Ok(Plan::Settled(payload)))
        }
        // A run held for an operator retry (`attempt.retry.v1`): its attempt
        // has settled, so the run and occurrence are released now instead of
        // at the run's deadline. The settled attempt itself is not cancelled.
        Some((_, "failed"))
            if target == Target::Occurrence
                && matches!(occurrence_state.as_str(), "claimed" | "running") =>
        {
            let (run_id, session_id) = run.clone().expect("a latest attempt has its run");
            if super::cancellation::has_unresolved_stop(conn, &run_id)? {
                return refuse(
                    ErrorCode::CancelPending,
                    "the held run has an unresolved stop; reconcile it before cancellation"
                        .to_owned(),
                );
            }
            let timeout_at: Option<String> = conn
                .query_row(
                    "SELECT timeout_at FROM automation_runs WHERE id = ?1",
                    [&run_id],
                    |row| row.get(0),
                )
                .map_err(|error| format!("failed to read held cancellation deadline: {error}"))?;
            let deadline_open = timeout_at
                .map(|value| DateTime::parse_from_rfc3339(&value))
                .transpose()
                .map_err(|error| format!("invalid held cancellation deadline: {error}"))?
                .is_some_and(|deadline| deadline > now);
            if !deadline_open {
                // Reconciliation settles the run failed without rewriting its
                // immutable failed attempt. Cancellation cannot win afterward.
                return refuse(
                    ErrorCode::IllegalTransition,
                    "the held run has no remaining retry window; reconcile its failure".to_owned(),
                );
            }
            let finished = record_run_finish(
                conn,
                &run_id,
                RunFinish {
                    status: "cancelled",
                    exit_code: None,
                    session_id,
                    log_json: None,
                    output_commit: None,
                },
                now,
            )
            .map_err(|error| format!("failed to cancel the held run: {error:#}"))?;
            if !finished
                || !settle_occurrence(conn, &occurrence_id, "cancelled", Some(&reason), now)?
            {
                return Err("the held run changed during cancellation".to_owned());
            }
            Ok(Ok(Plan::Settled(json!({
                "occurrenceId": occurrence_id,
                "status": "cancelled",
                "reason": reason,
                "cancelledAt": iso(now),
                "runId": run_id,
            }))))
        }
        _ => refuse(
            ErrorCode::IllegalTransition,
            format!("the occurrence is `{occurrence_state}`, with nothing left to cancel"),
        ),
    }
}

fn automation_of(
    conn: &Connection,
    target: Target,
    body: &Value,
) -> Result<Option<String>, String> {
    let Some(id) = body.get(target.id_field()).and_then(Value::as_str) else {
        return Ok(None);
    };
    let sql = match target {
        Target::Occurrence => "SELECT automation_id FROM automation_occurrences WHERE id = ?1",
        Target::Attempt => {
            "SELECT run.automation_id FROM automation_attempts AS attempt
             JOIN automation_runs AS run ON run.id = attempt.run_id WHERE attempt.id = ?1"
        }
    };
    conn.query_row(sql, [id], |row| row.get(0))
        .optional()
        .map_err(|error| format!("failed to resolve the cancelled routine: {error}"))
}

fn replay_mismatch() -> ErrorEnvelope {
    typed_error(
        ErrorCode::AdoptionReplayMismatch,
        "adoption key was already used for a different request",
    )
}

fn typed_error(code: ErrorCode, message: impl Into<String>) -> ErrorEnvelope {
    ErrorEnvelope::try_new(code, message, false)
        .expect("cancel command error messages satisfy protocol bounds")
}

fn iso(now: DateTime<Utc>) -> String {
    now.to_rfc3339_opts(SecondsFormat::Millis, true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::SessionLaunch;
    use crate::automations::definition::RoutineDefinition;
    use crate::automations::occurrences::insert_claimed_occurrence;
    use crate::automations::recovery::{execute_occurrence_recovery, RECOVER_ACTION};
    use crate::automations::runner::{
        dispatch_claimed_occurrences_with_clock, mark_unconfirmed_stop_for_recovery,
    };
    use crate::automations::store::insert_definition;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Launches and kills succeed; kills are counted.
    #[derive(Default)]
    struct KillCounting(AtomicUsize);

    impl SessionRuntime for KillCounting {
        fn launch_session(&self, _launch: &SessionLaunch) -> anyhow::Result<()> {
            Ok(())
        }

        fn send_input(&self, _session_id: &str, _payload: &Value) -> anyhow::Result<()> {
            Ok(())
        }

        fn kill_session(&self, _session_id: &str) -> anyhow::Result<()> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    /// Run cancellation fences stops on the real clock, so these tests do too.
    fn now() -> DateTime<Utc> {
        Utc::now()
    }

    fn store() -> (tempfile::TempDir, Connection) {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("store.sqlite");
        crate::store::initialize_store(&path).unwrap();
        let conn = crate::store::open_store(&path).unwrap();
        let routine = RoutineDefinition::from_json(&json!({
            "schemaVersion": 1, "id": "notes", "name": "Notes", "status": "ACTIVE",
            "rrule": "FREQ=DAILY;BYHOUR=9", "timezone": "utc", "misfire": "latest",
            "overlap": "forbid", "timeoutMinutes": 30, "runtime": "coven-code",
            "cwd": temp.path().display().to_string(), "prompt": "Summarise the notes."
        }))
        .unwrap();
        insert_definition(&conn, &routine).unwrap();
        (temp, conn)
    }

    fn occurrence_cancel(key: &str, occurrence: &str) -> Value {
        json!({ "action": OCCURRENCE_CANCEL_ACTION, "adoptionKey": key,
                "occurrenceId": occurrence, "reason": "not needed today" })
    }

    fn attempt_cancel(key: &str, attempt: &str) -> Value {
        json!({ "action": ATTEMPT_CANCEL_ACTION, "adoptionKey": key, "attemptId": attempt })
    }

    fn state(conn: &Connection, sql: &str, id: &str) -> String {
        conn.query_row(sql, [id], |row| row.get(0)).unwrap()
    }

    fn refused(
        conn: &Connection,
        runtime: &KillCounting,
        body: Value,
        target: Target,
    ) -> ErrorCode {
        match execute(conn, runtime, target, body, now()).unwrap() {
            CancelExecution::Rejected(error) => error.code(),
            success => panic!("expected a refusal, got {success:?}"),
        }
    }

    #[test]
    fn an_occurrence_that_has_not_dispatched_is_cancelled_at_once() {
        let (_temp, conn) = store();
        let runtime = KillCounting::default();
        assert!(
            insert_claimed_occurrence(&conn, "occ.notes-1", "notes", "daemon", 60, now()).unwrap()
        );
        let body = occurrence_cancel("adopt:cancel:claimed", "occ.notes-1");
        let CancelExecution::Success { payload, replayed } =
            execute_occurrence_cancel(&conn, &runtime, body.clone(), now()).unwrap()
        else {
            panic!("refused");
        };
        assert!(!replayed);
        assert_eq!(payload["status"], json!("cancelled"));
        assert_eq!(
            state(
                &conn,
                "SELECT state FROM automation_occurrences WHERE id = ?1",
                "occ.notes-1"
            ),
            "cancelled"
        );
        assert_eq!(
            execute_occurrence_cancel(&conn, &runtime, body, now()).unwrap(),
            CancelExecution::Success {
                payload,
                replayed: true
            }
        );
        assert_eq!(
            refused(
                &conn,
                &runtime,
                occurrence_cancel("adopt:cancel:again", "occ.notes-1"),
                Target::Occurrence
            ),
            ErrorCode::IllegalTransition
        );
        assert_eq!(runtime.0.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn a_running_occurrence_is_stopped_through_run_cancellation() {
        let (_temp, conn) = store();
        let runtime = KillCounting::default();
        assert!(
            insert_claimed_occurrence(&conn, "occ.notes-1", "notes", "daemon", 60, now()).unwrap()
        );
        let report = dispatch_claimed_occurrences_with_clock(&conn, &runtime, now(), now).unwrap();
        let run_id = report.dispatched[0].clone();
        let body = occurrence_cancel("adopt:cancel:running", "occ.notes-1");
        let answer = execute_occurrence_cancel(&conn, &runtime, body.clone(), now()).unwrap();
        let CancelExecution::Success { payload, .. } = answer else {
            panic!("refused: {answer:?}");
        };
        assert_eq!(payload["occurrenceId"], json!("occ.notes-1"));
        assert_eq!(payload["runId"], json!(run_id));
        assert_eq!(
            runtime.0.load(Ordering::SeqCst),
            1,
            "the session was killed"
        );
        assert_eq!(
            state(
                &conn,
                "SELECT status FROM automation_runs WHERE id = ?1",
                &run_id
            ),
            "cancelled"
        );
        // A replay repeats the same run cancellation, which answers from its
        // own adoption and kills nothing.
        let CancelExecution::Success { replayed, .. } =
            execute_occurrence_cancel(&conn, &runtime, body, now()).unwrap()
        else {
            panic!("refused");
        };
        assert!(replayed);
        assert_eq!(runtime.0.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn dispatch_window_cancellation_can_be_replayed_after_ownership_publication() {
        struct CancelDuringLaunch {
            path: std::path::PathBuf,
            target: Target,
            request: std::sync::Mutex<Option<Value>>,
            kills: AtomicUsize,
        }
        impl SessionRuntime for CancelDuringLaunch {
            fn launch_session(&self, launch: &SessionLaunch) -> anyhow::Result<()> {
                let conn = crate::store::open_store(&self.path)?;
                let attempt: String = conn.query_row(
                    "SELECT a.id FROM automation_attempts a
                     JOIN automation_runs r ON r.id = a.run_id
                     WHERE r.session_id = ?1 AND a.state = 'dispatching'
                       AND a.session_id IS NULL",
                    [&launch.id],
                    |row| row.get(0),
                )?;
                let body = match self.target {
                    Target::Occurrence => occurrence_cancel("adopt:cancel:dispatch", "occ.notes-1"),
                    Target::Attempt => attempt_cancel("adopt:cancel:dispatch", &attempt),
                };
                let answer = execute(&conn, self, self.target, body.clone(), now()).unwrap();
                assert!(
                    matches!(&answer, CancelExecution::Rejected(error)
                        if error.code() == ErrorCode::CancelPending && error.retryable),
                    "{answer:?}"
                );
                assert_eq!(self.kills.load(Ordering::SeqCst), 0);
                *self.request.lock().unwrap() = Some(body);
                Ok(())
            }
            fn send_input(&self, _: &str, _: &Value) -> anyhow::Result<()> {
                Ok(())
            }
            fn kill_session(&self, _: &str) -> anyhow::Result<()> {
                self.kills.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
        }
        for target in [Target::Occurrence, Target::Attempt] {
            let (temp, conn) = store();
            let runtime = CancelDuringLaunch {
                path: temp.path().join("store.sqlite"),
                target,
                request: std::sync::Mutex::new(None),
                kills: AtomicUsize::new(0),
            };
            insert_claimed_occurrence(&conn, "occ.notes-1", "notes", "daemon", 60, now()).unwrap();
            let report =
                dispatch_claimed_occurrences_with_clock(&conn, &runtime, now(), now).unwrap();
            assert_eq!(report.dispatched.len(), 1);
            let body = runtime.request.lock().unwrap().clone().unwrap();
            for _ in 0..2 {
                let answer = execute(&conn, &runtime, target, body.clone(), now()).unwrap();
                assert!(
                    matches!(answer, CancelExecution::Success { replayed: true, .. }),
                    "{answer:?}"
                );
                assert_eq!(runtime.kills.load(Ordering::SeqCst), 1);
            }
        }
    }

    #[test]
    fn blank_reasons_use_the_default_for_running_targets() {
        for target in [Target::Occurrence, Target::Attempt] {
            for reason in ["", "   "] {
                let (_temp, conn) = store();
                let runtime = KillCounting::default();
                insert_claimed_occurrence(&conn, "occ.notes-1", "notes", "daemon", 60, now())
                    .unwrap();
                let report =
                    dispatch_claimed_occurrences_with_clock(&conn, &runtime, now(), now).unwrap();
                let run_id = &report.dispatched[0];
                let mut body = match target {
                    Target::Occurrence => occurrence_cancel("adopt:cancel:blank", "occ.notes-1"),
                    Target::Attempt => {
                        attempt_cancel("adopt:cancel:blank", &format!("attempt-{run_id}-1"))
                    }
                };
                body["reason"] = json!(reason);
                let answer = execute(&conn, &runtime, target, body.clone(), now()).unwrap();
                assert!(
                    matches!(answer, CancelExecution::Success { .. }),
                    "{answer:?}"
                );
                assert_eq!(runtime.0.load(Ordering::SeqCst), 1);
                assert!(matches!(
                    execute(&conn, &runtime, target, body, now()).unwrap(),
                    CancelExecution::Success { replayed: true, .. }
                ));
                assert_eq!(runtime.0.load(Ordering::SeqCst), 1);
            }
        }
    }

    #[test]
    fn waiting_cancellation_preserves_an_expired_deadline() {
        for target in [Target::Occurrence, Target::Attempt] {
            let (_temp, conn) = store();
            let runtime = KillCounting::default();
            insert_claimed_occurrence(&conn, "occ.notes-1", "notes", "daemon", 60, now()).unwrap();
            let report =
                dispatch_claimed_occurrences_with_clock(&conn, &runtime, now(), now).unwrap();
            let run_id = &report.dispatched[0];
            mark_unconfirmed_stop_for_recovery(&conn, run_id, "stop not confirmed", now()).unwrap();
            conn.execute(
                "UPDATE sessions SET status = 'orphaned'
                 WHERE id = (SELECT session_id FROM automation_runs WHERE id = ?1)",
                [run_id],
            )
            .unwrap();
            execute_occurrence_recovery(
                &conn,
                json!({ "action": RECOVER_ACTION, "adoptionKey": "adopt:recover:deadline",
                    "occurrenceId": "occ.notes-1",
                    "evidenceDetermination": "retry_with_new_attempt" }),
                now(),
            )
            .unwrap();
            let waiting = format!("attempt-{run_id}-2");
            let deadline = DateTime::parse_from_rfc3339(&state(
                &conn,
                "SELECT timeout_at FROM automation_runs WHERE id = ?1",
                run_id,
            ))
            .unwrap()
            .with_timezone(&Utc);
            let body = match target {
                Target::Occurrence => occurrence_cancel("adopt:cancel:deadline", "occ.notes-1"),
                Target::Attempt => attempt_cancel("adopt:cancel:deadline", &waiting),
            };
            let answer = execute(&conn, &runtime, target, body.clone(), deadline).unwrap();
            assert!(
                matches!(&answer, CancelExecution::Rejected(error)
                if error.code() == ErrorCode::IllegalTransition),
                "{answer:?}"
            );
            assert_eq!(
                state(
                    &conn,
                    "SELECT state FROM automation_attempts WHERE id = ?1",
                    &waiting
                ),
                "timed_out"
            );
            assert_eq!(
                state(
                    &conn,
                    "SELECT status FROM automation_runs WHERE id = ?1",
                    run_id
                ),
                "failed"
            );
            assert_eq!(
                state(
                    &conn,
                    "SELECT state FROM automation_occurrences WHERE id = ?1",
                    "occ.notes-1"
                ),
                "failed"
            );
            assert_eq!(
                execute(&conn, &runtime, target, body, deadline).unwrap(),
                answer
            );
            assert_eq!(runtime.0.load(Ordering::SeqCst), 0);
        }
    }

    #[test]
    fn a_waiting_retry_attempt_is_cancelled_with_its_run() {
        let (_temp, conn) = store();
        let runtime = KillCounting::default();
        assert!(
            insert_claimed_occurrence(&conn, "occ.notes-1", "notes", "daemon", 60, now()).unwrap()
        );
        let report = dispatch_claimed_occurrences_with_clock(&conn, &runtime, now(), now).unwrap();
        let run_id = report.dispatched[0].clone();
        mark_unconfirmed_stop_for_recovery(&conn, &run_id, "stop not confirmed", now()).unwrap();
        conn.execute(
            "UPDATE sessions SET status = 'orphaned'
             WHERE id = (SELECT session_id FROM automation_runs WHERE id = ?1)",
            [&run_id],
        )
        .unwrap();
        // While the occurrence needs recovery, cancelling is refused, and the
        // answer names the command that resolves it.
        let CancelExecution::Rejected(error) = execute_occurrence_cancel(
            &conn,
            &runtime,
            occurrence_cancel("adopt:cancel:recovering", "occ.notes-1"),
            now(),
        )
        .unwrap() else {
            panic!("a recovering occurrence was cancelled");
        };
        assert_eq!(error.code(), ErrorCode::IllegalTransition);
        assert!(
            serde_json::to_string(&error)
                .unwrap()
                .contains("occurrence.recover.v1"),
            "{error:?}"
        );
        execute_occurrence_recovery(
            &conn,
            json!({ "action": RECOVER_ACTION, "adoptionKey": "adopt:recover:retry",
                    "occurrenceId": "occ.notes-1",
                    "evidenceDetermination": "retry_with_new_attempt" }),
            now(),
        )
        .unwrap();
        let waiting = format!("attempt-{run_id}-2");
        // The settled first attempt is no longer the run's current one.
        assert_eq!(
            refused(
                &conn,
                &runtime,
                attempt_cancel("adopt:cancel:old", &format!("attempt-{run_id}-1")),
                Target::Attempt
            ),
            ErrorCode::IllegalTransition
        );
        let CancelExecution::Success { payload, .. } = execute_attempt_cancel(
            &conn,
            &runtime,
            attempt_cancel("adopt:cancel:waiting", &waiting),
            now(),
        )
        .unwrap() else {
            panic!("refused");
        };
        assert_eq!(payload["attemptId"], json!(waiting));
        for (sql, id, expected) in [
            (
                "SELECT state FROM automation_attempts WHERE id = ?1",
                waiting.as_str(),
                "cancelled",
            ),
            (
                "SELECT status FROM automation_runs WHERE id = ?1",
                run_id.as_str(),
                "cancelled",
            ),
            (
                "SELECT state FROM automation_occurrences WHERE id = ?1",
                "occ.notes-1",
                "cancelled",
            ),
        ] {
            assert_eq!(state(&conn, sql, id), expected, "{sql}");
        }
        assert_eq!(runtime.0.load(Ordering::SeqCst), 0, "nothing was running");
        assert_eq!(
            refused(
                &conn,
                &runtime,
                attempt_cancel("adopt:cancel:unknown", "attempt-none-1"),
                Target::Attempt
            ),
            ErrorCode::NotFound
        );
    }
}
