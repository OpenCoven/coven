//! `attempt.retry.v1`: the owner retries a run held after a failure
//! (coven#1054, coven#857).
//!
//! By the maintainer's 2026-10-05 decision, a failure with no automatic retry
//! left holds its run `running` until the run's deadline, and the runner
//! settles it failed when the deadline passes. Within that window this command
//! opens the next attempt, classified `operator_retry`, carrying the prior
//! disposition the caller names. The occurrence goes back to `planned`, and
//! the scheduler dispatches the new attempt.
//!
//! The caller names the prior attempt and its disposition, so a retry is never
//! issued against an attempt it did not mean. Ambiguous work is never retried
//! here: `occurrence.recover.v1` handles it with the operator's explicit
//! determination.

use chrono::{DateTime, SecondsFormat, Utc};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::Deserialize;
use serde_json::{json, Value};

use super::contract::canonical_json::{canonicalize, sha256_hex};
use super::contract::error::{ErrorCode, ErrorEnvelope};
use super::contract::types::{AdoptionKey, RunId};
use super::runner::MAX_ATTEMPT_NUMBER;

pub const RETRY_ACTION: &str = "coven.automations.attempt.retry.v1";
const COMMAND: &str = "attempt.retry.v1";
const NOTE_MAX_CHARS: usize = 500;

/// The answer to one retry command.
#[derive(Debug, Clone, PartialEq)]
pub enum RetryExecution {
    Success { payload: Value, replayed: bool },
    Rejected(ErrorEnvelope),
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RetryRequest {
    action: String,
    adoption_key: String,
    run_id: String,
    prior_attempt_number: i64,
    prior_disposition: PriorDisposition,
    #[serde(default)]
    note: Option<String>,
}

/// The dispositions a retry may name; `ambiguous` is deliberately absent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum PriorDisposition {
    Failed,
    TimedOut,
    Cancelled,
}

impl PriorDisposition {
    fn as_str(self) -> &'static str {
        match self {
            Self::Failed => "failed",
            Self::TimedOut => "timed_out",
            Self::Cancelled => "cancelled",
        }
    }
}

/// The run being retried: its occurrence, authority profile and deadline.
type RetryRun = (String, Option<String>, Option<String>, Option<String>);

/// Executes one `attempt.retry.v1` request at `now`.
pub fn execute_attempt_retry(
    conn: &Connection,
    body: Value,
    now: DateTime<Utc>,
) -> Result<RetryExecution, String> {
    let digest = sha256_hex(
        &canonicalize(&body)
            .map_err(|error| format!("failed to canonicalize retry request: {error:#}"))?,
    );
    let adoption_key = body
        .get("adoptionKey")
        .and_then(Value::as_str)
        .filter(|key| AdoptionKey::new((*key).to_owned()).is_ok())
        .map(ToOwned::to_owned);
    let transaction = rusqlite::Transaction::new_unchecked(conn, TransactionBehavior::Immediate)
        .map_err(|error| format!("failed to begin attempt retry: {error}"))?;
    if let Some(key) = adoption_key.as_deref() {
        if let Some(replay) = adopted_response(&transaction, key, &digest)? {
            return Ok(replay);
        }
    }
    let outcome = match parse(&body) {
        Ok(request) => retry_in(&transaction, &request, now)?,
        Err(error) => Err(error),
    };
    let automation_id: Option<String> = body
        .get("runId")
        .and_then(Value::as_str)
        .map(|run_id| {
            transaction
                .query_row(
                    "SELECT automation_id FROM automation_runs WHERE id = ?1",
                    [run_id],
                    |row| row.get(0),
                )
                .optional()
        })
        .transpose()
        .map_err(|error| format!("failed to resolve the retried routine: {error}"))?
        .flatten();
    let (stored, execution) = match outcome {
        Ok(payload) => (
            json!({ "outcome": "committed", "result": payload }),
            RetryExecution::Success {
                payload,
                replayed: false,
            },
        ),
        Err(error) => (
            json!({ "outcome": "rejected", "error": error }),
            RetryExecution::Rejected(error),
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
            .map_err(|error| format!("failed to adopt the retry command: {error}"))?;
    }
    transaction
        .commit()
        .map_err(|error| format!("failed to commit attempt retry: {error}"))?;
    Ok(execution)
}

fn adopted_response(
    conn: &Connection,
    adoption_key: &str,
    digest: &str,
) -> Result<Option<RetryExecution>, String> {
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
        .map_err(|error| format!("failed to read the retry adoption: {error}"))?;
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
        return Ok(used_elsewhere.then(|| RetryExecution::Rejected(replay_mismatch())));
    };
    if command != COMMAND || stored_digest != digest {
        return Ok(Some(RetryExecution::Rejected(replay_mismatch())));
    }
    let response: Value = serde_json::from_str(&response_json)
        .map_err(|error| format!("stored retry response is invalid: {error}"))?;
    Ok(Some(match response["outcome"].as_str() {
        Some("committed") => RetryExecution::Success {
            payload: response["result"].clone(),
            replayed: true,
        },
        Some("rejected") => RetryExecution::Rejected(
            serde_json::from_value(response["error"].clone())
                .map_err(|error| format!("stored retry error is invalid: {error}"))?,
        ),
        other => return Err(format!("stored retry outcome {other:?} is invalid")),
    }))
}

fn parse(body: &Value) -> Result<RetryRequest, ErrorEnvelope> {
    let invalid = |message: String| typed_error(ErrorCode::ValidationFailed, message);
    let request: RetryRequest = serde_json::from_value(body.clone()).map_err(|error| {
        if body["priorDisposition"] == json!("ambiguous") {
            typed_error(
                ErrorCode::AmbiguousRetryForbidden,
                "ambiguous work is recovered with occurrence.recover.v1, never retried",
            )
        } else {
            invalid(format!("invalid attempt retry request: {error}"))
        }
    })?;
    if request.action != RETRY_ACTION {
        return Err(invalid(format!("action must be {RETRY_ACTION}")));
    }
    if AdoptionKey::new(request.adoption_key.clone()).is_err() {
        return Err(invalid(
            "adoptionKey is not a valid adoption key".to_owned(),
        ));
    }
    if RunId::new(request.run_id.clone()).is_err() {
        return Err(invalid("runId is not a valid run id".to_owned()));
    }
    if request.prior_attempt_number < 1 {
        return Err(invalid("priorAttemptNumber must be at least 1".to_owned()));
    }
    if request
        .note
        .as_ref()
        .is_some_and(|note| note.chars().count() > NOTE_MAX_CHARS)
    {
        return Err(invalid(format!(
            "note must be at most {NOTE_MAX_CHARS} characters"
        )));
    }
    Ok(request)
}

fn retry_in(
    conn: &Connection,
    request: &RetryRequest,
    now: DateTime<Utc>,
) -> Result<Result<Value, ErrorEnvelope>, String> {
    let refuse = |code, message: String| Ok(Err(typed_error(code, message)));
    let run: Option<(String, RetryRun)> = conn
        .query_row(
            "SELECT status, automation_id, occurrence_id, authority_profile, timeout_at
             FROM automation_runs WHERE id = ?1",
            [&request.run_id],
            |row| {
                Ok((
                    row.get(0)?,
                    (row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?),
                ))
            },
        )
        .optional()
        .map_err(|error| format!("failed to read the run: {error}"))?;
    let Some((status, (automation_id, occurrence_id, authority_profile, timeout_at))) = run else {
        return refuse(ErrorCode::NotFound, format!("no run `{}`", request.run_id));
    };
    if authority_profile.is_some() {
        return refuse(
            ErrorCode::AuthorityRequired,
            "a Runtime Authority run is retried through its adapter, which this command \
             cannot reach yet"
                .to_owned(),
        );
    }
    if status != "running" {
        return refuse(
            ErrorCode::IllegalTransition,
            format!(
                "the run is `{status}`; only a run held after a failure can be retried before \
                 its deadline"
            ),
        );
    }
    let Some(occurrence_id) = occurrence_id else {
        return refuse(
            ErrorCode::IllegalTransition,
            "the run has no occurrence to plan".to_owned(),
        );
    };
    let latest: Option<(i64, String)> = conn
        .query_row(
            "SELECT attempt_number, state FROM automation_attempts
             WHERE run_id = ?1 ORDER BY attempt_number DESC LIMIT 1",
            [&request.run_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|error| format!("failed to read the latest attempt: {error}"))?;
    let Some((attempt_number, attempt_state)) = latest else {
        return refuse(
            ErrorCode::IllegalTransition,
            "the run has no attempt".to_owned(),
        );
    };
    if attempt_state == "ambiguous" {
        return refuse(
            ErrorCode::AmbiguousRetryForbidden,
            "the latest attempt is ambiguous; recover it with occurrence.recover.v1".to_owned(),
        );
    }
    if matches!(
        attempt_state.as_str(),
        "adopted" | "dispatching" | "started" | "observing"
    ) {
        return refuse(
            ErrorCode::IllegalTransition,
            format!("attempt #{attempt_number} is still `{attempt_state}`"),
        );
    }
    if attempt_number != request.prior_attempt_number
        || attempt_state != request.prior_disposition.as_str()
    {
        return refuse(
            ErrorCode::RetryDispositionInvalid,
            format!(
                "the run's latest attempt is #{attempt_number}, `{attempt_state}`, not #{}, `{}`",
                request.prior_attempt_number,
                request.prior_disposition.as_str()
            ),
        );
    }
    if attempt_number >= MAX_ATTEMPT_NUMBER {
        return refuse(
            ErrorCode::IllegalTransition,
            format!("the run has used all {MAX_ATTEMPT_NUMBER} attempts"),
        );
    }
    if super::cancellation::has_unresolved_stop(conn, &request.run_id)? {
        return refuse(
            ErrorCode::CancelPending,
            "the run has an unresolved stop; reconcile it before retrying".to_owned(),
        );
    }
    let deadline_open = match timeout_at.as_deref() {
        Some(timeout_at) => {
            DateTime::parse_from_rfc3339(timeout_at).map_err(|error| {
                format!("run `{}` has an invalid timeout: {error}", request.run_id)
            })? > now
        }
        None => {
            return refuse(
                ErrorCode::IllegalTransition,
                "the run has no original deadline, so it cannot be held for an operator retry"
                    .to_owned(),
            );
        }
    };
    if !deadline_open {
        return refuse(
            ErrorCode::DeadlineExceeded,
            "the run's deadline has passed".to_owned(),
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
    if super::runs::is_retry_quarantined(conn, &automation_id)
        .map_err(|error| format!("failed to inspect retry quarantine: {error:#}"))?
    {
        return refuse(
            ErrorCode::IllegalTransition,
            "the routine is quarantined; explicitly unquarantine it before retrying".to_owned(),
        );
    }

    let next = attempt_number + 1;
    let attempt_id = format!("attempt-{}-{next}", request.run_id);
    let reason = match request.note.as_deref() {
        Some(note) => format!("operator retry: {note}"),
        None => "operator retry".to_owned(),
    };
    let opened = conn
        .execute(
            "INSERT INTO automation_attempts
                (id, run_id, occurrence_id, attempt_number, adoption_key,
                 occurrence_fence_generation, dispatch_generation, state,
                 prior_attempt_number, prior_disposition, retry_classification,
                 state_reason, not_before, opened_at)
             SELECT ?1, ?2, ?3, ?4, ?5, attempt, 0, 'adopted',
                    ?6, ?7, 'operator_retry', ?8, ?9, ?9
             FROM automation_occurrences
             WHERE id = ?3 AND state IN ('claimed', 'running')",
            params![
                attempt_id,
                request.run_id,
                occurrence_id,
                next,
                format!("automation:{}:{next}", request.run_id),
                attempt_number,
                request.prior_disposition.as_str(),
                reason,
                iso(now),
            ],
        )
        .map_err(|error| format!("failed to open the retry attempt: {error}"))?;
    if opened != 1 {
        return refuse(
            ErrorCode::IllegalTransition,
            "the run's occurrence is not held for a retry".to_owned(),
        );
    }
    let replanned = conn
        .execute(
            "UPDATE automation_occurrences
             SET state = 'planned', lease_owner = NULL, lease_expires_at = NULL,
                 failure_reason = NULL, updated_at = ?2
             WHERE id = ?1 AND state IN ('claimed', 'running')",
            params![occurrence_id, iso(now)],
        )
        .map_err(|error| format!("failed to plan the retried occurrence: {error}"))?;
    let released = conn
        .execute(
            "UPDATE automation_runs SET session_id = NULL WHERE id = ?1 AND status = 'running'",
            [&request.run_id],
        )
        .map_err(|error| format!("failed to release the run's session: {error}"))?;
    if replanned != 1 || released != 1 {
        return Err("the run changed during the retry".to_owned());
    }
    Ok(Ok(json!({
        "runId": request.run_id,
        "occurrenceId": occurrence_id,
        "attemptId": attempt_id,
        "attemptNumber": next,
        "priorDisposition": request.prior_disposition.as_str(),
        "status": "planned",
        "retriedAt": iso(now),
    })))
}

fn replay_mismatch() -> ErrorEnvelope {
    typed_error(
        ErrorCode::AdoptionReplayMismatch,
        "adoption key was already used for a different request",
    )
}

fn typed_error(code: ErrorCode, message: impl Into<String>) -> ErrorEnvelope {
    ErrorEnvelope::try_new(code, message, false)
        .expect("attempt retry error messages satisfy protocol bounds")
}

fn iso(now: DateTime<Utc>) -> String {
    now.to_rfc3339_opts(SecondsFormat::Millis, true)
}

#[cfg(test)]
pub(crate) fn assert_operator_retry_transition(before: &str, after: &str) {
    let machines: Value = serde_json::from_str(include_str!(
        "../../../../spec/coven-automations/v1/state-machines.json"
    ))
    .unwrap();
    let occurrence = machines["machines"]
        .as_array()
        .unwrap()
        .iter()
        .find(|machine| machine["id"] == "occurrence.v1")
        .unwrap();
    assert!(
        occurrence["transitions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|edge| edge["from"] == before
                && edge["to"] == after
                && edge["on"] == "operator_retry"
                && edge["actor"] == "command_handler"),
        "observed operator retry {before} -> {after} is absent from the protocol artifact"
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::NoopSessionRuntime;
    use crate::automations::cancel_commands::{
        execute_attempt_cancel, execute_occurrence_cancel, CancelExecution, ATTEMPT_CANCEL_ACTION,
        OCCURRENCE_CANCEL_ACTION,
    };
    use crate::automations::definition::RoutineDefinition;
    use crate::automations::occurrences::{claim_due_occurrence, insert_claimed_occurrence};
    use crate::automations::runner::{
        dispatch_claimed_occurrences_with_clock, settle_finished_runs,
    };
    use crate::automations::store::insert_definition;

    struct Fixture {
        _temp: tempfile::TempDir,
        conn: Connection,
        run_id: String,
        now: DateTime<Utc>,
    }

    /// A routine whose first run is dispatched at `now` and still running.
    fn running() -> Fixture {
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
        let now = Utc::now();
        assert!(
            insert_claimed_occurrence(&conn, "occ.notes-1", "notes", "daemon", 60, now).unwrap()
        );
        let report =
            dispatch_claimed_occurrences_with_clock(&conn, &NoopSessionRuntime, now, || now)
                .unwrap();
        Fixture {
            _temp: temp,
            run_id: report.dispatched[0].clone(),
            conn,
            now,
        }
    }

    /// The run's session fails, and settlement holds the run for a retry.
    fn held() -> Fixture {
        let fixture = running();
        let session: String = fixture
            .conn
            .query_row(
                "SELECT session_id FROM automation_runs WHERE id = ?1",
                [&fixture.run_id],
                |row| row.get(0),
            )
            .unwrap();
        crate::store::update_session_terminal_if_active(
            &fixture.conn,
            &session,
            "failed",
            Some(1),
            &iso(fixture.now),
        )
        .unwrap();
        assert_eq!(
            settle_finished_runs(&fixture.conn, fixture.now)
                .unwrap()
                .failed,
            0
        );
        fixture
    }

    fn request(fixture: &Fixture, key: &str, number: i64, disposition: &str) -> Value {
        json!({ "action": RETRY_ACTION, "adoptionKey": key, "runId": fixture.run_id,
                "priorAttemptNumber": number, "priorDisposition": disposition,
                "note": "the disk was full" })
    }

    fn refused(fixture: &Fixture, body: Value, at: DateTime<Utc>) -> ErrorCode {
        match execute_attempt_retry(&fixture.conn, body, at).unwrap() {
            RetryExecution::Rejected(error) => error.code(),
            success => panic!("expected a refusal, got {success:?}"),
        }
    }

    #[derive(Default)]
    struct CountStops(std::cell::Cell<usize>, bool);

    impl crate::api::SessionRuntime for CountStops {
        fn launch_session(&self, _: &crate::api::SessionLaunch) -> anyhow::Result<()> {
            Ok(())
        }
        fn send_input(&self, _: &str, _: &Value) -> anyhow::Result<()> {
            Ok(())
        }
        fn kill_session(&self, _: &str) -> anyhow::Result<()> {
            self.0.set(self.0.get() + 1);
            if self.1 {
                anyhow::bail!("stop acknowledgement was lost");
            }
            Ok(())
        }
    }

    fn retry_after_losing_cancellation() -> (Fixture, CountStops, Value, CancelExecution) {
        let fixture = running();
        let runtime = CountStops::default();
        let session: String = fixture
            .conn
            .query_row(
                "SELECT session_id FROM automation_runs WHERE id = ?1",
                [&fixture.run_id],
                |row| row.get(0),
            )
            .unwrap();
        let path = fixture._temp.path().join("store.sqlite");
        let now = fixture.now;
        super::super::cancellation::set_after_stop_fence_test_hook(Some(Box::new(move || {
            let conn = crate::store::open_store(&path).unwrap();
            // Natural failure is recorded after the stop fence, before the
            // acknowledged stop can settle it. Cancellation loses this race.
            crate::store::update_session_terminal_if_active(
                &conn,
                &session,
                "failed",
                Some(1),
                &iso(now),
            )
            .unwrap();
        })));
        let body = json!({"action": ATTEMPT_CANCEL_ACTION,
            "adoptionKey": "adopt:cancel:first", "attemptId": format!("attempt-{}-1", fixture.run_id)});
        let outcome = execute_attempt_cancel(&fixture.conn, &runtime, body.clone(), now).unwrap();
        let CancelExecution::Rejected(error) = &outcome else {
            panic!("{outcome:?}");
        };
        assert_eq!(error.code(), ErrorCode::IllegalTransition);
        assert_eq!(runtime.0.get(), 1);
        assert_eq!(settle_finished_runs(&fixture.conn, now).unwrap().failed, 0);
        assert!(matches!(
            execute_attempt_retry(
                &fixture.conn,
                request(&fixture, "adopt:retry:after-cancel", 1, "failed"),
                now
            )
            .unwrap(),
            RetryExecution::Success { .. }
        ));
        (fixture, runtime, body, outcome)
    }

    #[test]
    fn cancellation_that_loses_before_stop_ownership_reconciles_its_held_failure() {
        let fixture = running();
        let runtime = CountStops::default();
        let session: String = fixture
            .conn
            .query_row(
                "SELECT session_id FROM automation_runs WHERE id = ?1",
                [&fixture.run_id],
                |row| row.get(0),
            )
            .unwrap();
        let path = fixture._temp.path().join("store.sqlite");
        let run_id = fixture.run_id.clone();
        let now = fixture.now;
        super::super::cancellation::set_before_stop_fence_test_hook(Some(Box::new(move || {
            let conn = crate::store::open_store(&path).unwrap();
            crate::store::update_session_terminal_if_active(
                &conn,
                &session,
                "failed",
                Some(1),
                &iso(now),
            )
            .unwrap();
            assert_eq!(settle_finished_runs(&conn, now).unwrap().failed, 0);
            let retry = execute_attempt_retry(
                &conn,
                json!({
                    "action": RETRY_ACTION, "adoptionKey": "adopt:retry:pending-race",
                    "runId": run_id, "priorAttemptNumber": 1, "priorDisposition": "failed",
                }),
                now,
            )
            .unwrap();
            let RetryExecution::Rejected(error) = retry else {
                panic!("pending cancellation was bypassed");
            };
            assert_eq!(error.code(), ErrorCode::CancelPending);
        })));
        let body = json!({"action": ATTEMPT_CANCEL_ACTION,
            "adoptionKey": "adopt:cancel:before-stop", "attemptId": format!("attempt-{}-1", fixture.run_id)});
        let result = execute_attempt_cancel(&fixture.conn, &runtime, body.clone(), now).unwrap();
        let CancelExecution::Rejected(error) = &result else {
            panic!("{result:?}");
        };
        assert_eq!(
            error.code(),
            ErrorCode::IllegalTransition,
            "known failed completion must finish the losing request"
        );
        assert_eq!(
            execute_attempt_cancel(&fixture.conn, &runtime, body, now).unwrap(),
            result
        );
        assert_eq!(runtime.0.get(), 0, "no stop was owned or issued");
        assert!(matches!(
            execute_attempt_retry(
                &fixture.conn,
                request(&fixture, "adopt:retry:reconciled-race", 1, "failed"),
                now
            )
            .unwrap(),
            RetryExecution::Success { .. }
        ));
    }

    #[test]
    fn lost_stop_acknowledgement_retains_recovery_instead_of_retry_hold() {
        let fixture = running();
        let runtime = CountStops(std::cell::Cell::new(0), true);
        let session: String = fixture
            .conn
            .query_row(
                "SELECT session_id FROM automation_runs WHERE id = ?1",
                [&fixture.run_id],
                |row| row.get(0),
            )
            .unwrap();
        let path = fixture._temp.path().join("store.sqlite");
        let now = fixture.now;
        super::super::cancellation::set_after_stop_fence_test_hook(Some(Box::new(move || {
            let conn = crate::store::open_store(&path).unwrap();
            crate::store::update_session_terminal_if_active(
                &conn,
                &session,
                "failed",
                Some(1),
                &iso(now),
            )
            .unwrap();
            assert_eq!(settle_finished_runs(&conn, now).unwrap().failed, 0);
        })));
        let body = json!({"action": ATTEMPT_CANCEL_ACTION,
            "adoptionKey": "adopt:cancel:lost-ack", "attemptId": format!("attempt-{}-1", fixture.run_id)});
        let result = execute_attempt_cancel(&fixture.conn, &runtime, body.clone(), now).unwrap();
        let CancelExecution::Success { payload, .. } = &result else {
            panic!("{result:?}");
        };
        assert_eq!(payload["status"], "recovery_required");
        assert_eq!(
            refused(
                &fixture,
                request(&fixture, "adopt:retry:lost-ack", 1, "failed"),
                now
            ),
            ErrorCode::AmbiguousRetryForbidden
        );
        assert!(matches!(
            execute_attempt_cancel(&fixture.conn, &runtime, body, now).unwrap(),
            CancelExecution::Success { replayed: true, .. }
        ));
        assert_eq!(runtime.0.get(), 1);
    }

    #[test]
    fn retry_projection_omits_the_prior_attempts_rejected_cancellation() {
        let (fixture, runtime, old_body, old_outcome) = retry_after_losing_cancellation();
        assert!(
            super::super::cancellation::cancellation_for_run(&fixture.conn, &fixture.run_id)
                .unwrap()
                .is_none(),
            "adopted retry must not project a prior attempt's cancellation"
        );
        assert_eq!(
            execute_attempt_cancel(&fixture.conn, &runtime, old_body, fixture.now).unwrap(),
            old_outcome
        );
        assert_eq!(
            runtime.0.get(),
            1,
            "historical replay must not stop the retry"
        );
    }

    #[test]
    fn retried_attempt_can_be_cancelled_after_a_prior_cancellation_lost() {
        let (fixture, runtime, old_body, old_outcome) = retry_after_losing_cancellation();
        let now = fixture.now;
        assert!(
            claim_due_occurrence(&fixture.conn, "notes", "daemon", 60, now)
                .unwrap()
                .is_some()
        );
        assert_eq!(
            dispatch_claimed_occurrences_with_clock(&fixture.conn, &runtime, now, || now)
                .unwrap()
                .dispatched,
            std::slice::from_ref(&fixture.run_id)
        );
        let body = json!({"action": ATTEMPT_CANCEL_ACTION,
            "adoptionKey": "adopt:cancel:second", "attemptId": format!("attempt-{}-2", fixture.run_id)});
        let result = execute_attempt_cancel(&fixture.conn, &runtime, body.clone(), now).unwrap();
        assert!(
            matches!(result, CancelExecution::Success { .. }),
            "{result:?}"
        );
        assert_eq!(runtime.0.get(), 2);
        assert!(matches!(
            execute_attempt_cancel(&fixture.conn, &runtime, body, now).unwrap(),
            CancelExecution::Success { replayed: true, .. }
        ));
        assert_eq!(
            execute_attempt_cancel(&fixture.conn, &runtime, old_body, now).unwrap(),
            old_outcome
        );
        assert_eq!(runtime.0.get(), 2);
        let states: Vec<String> = fixture
            .conn
            .prepare(
                "SELECT state FROM automation_cancellations WHERE run_id = ?1 ORDER BY attempt_id",
            )
            .unwrap()
            .query_map([&fixture.run_id], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(states, ["rejected", "cancelled"]);
        assert_eq!(
            super::super::cancellation::cancellation_for_run(&fixture.conn, &fixture.run_id)
                .unwrap()
                .unwrap()["status"],
            "cancelled"
        );
    }

    #[test]
    fn a_held_run_is_retried_and_its_new_attempt_dispatches() {
        let fixture = held();
        let before: String = fixture
            .conn
            .query_row(
                "SELECT state FROM automation_occurrences WHERE id = 'occ.notes-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(before, "running");
        let body = request(&fixture, "adopt:retry:1", 1, "failed");
        let RetryExecution::Success { payload, replayed } =
            execute_attempt_retry(&fixture.conn, body.clone(), fixture.now).unwrap()
        else {
            panic!("refused");
        };
        let after: String = fixture
            .conn
            .query_row(
                "SELECT state FROM automation_occurrences WHERE id = 'occ.notes-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_operator_retry_transition(&before, &after);
        assert!(!replayed);
        assert_eq!(payload["attemptNumber"], json!(2));
        let (classification, prior): (String, String) = fixture
            .conn
            .query_row(
                "SELECT retry_classification, prior_disposition FROM automation_attempts
                 WHERE id = ?1",
                [payload["attemptId"].as_str().unwrap()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            (classification.as_str(), prior.as_str()),
            ("operator_retry", "failed")
        );
        // An exact replay answers the same; the key with another request does not.
        assert_eq!(
            execute_attempt_retry(&fixture.conn, body, fixture.now).unwrap(),
            RetryExecution::Success {
                payload,
                replayed: true
            }
        );
        assert_eq!(
            refused(
                &fixture,
                request(&fixture, "adopt:retry:1", 1, "cancelled"),
                fixture.now
            ),
            ErrorCode::AdoptionReplayMismatch
        );
        // The scheduler claims the occurrence again and dispatches attempt 2.
        assert_eq!(
            claim_due_occurrence(&fixture.conn, "notes", "daemon", 60, fixture.now).unwrap(),
            Some("occ.notes-1".to_owned())
        );
        let report = dispatch_claimed_occurrences_with_clock(
            &fixture.conn,
            &NoopSessionRuntime,
            fixture.now,
            || fixture.now,
        )
        .unwrap();
        assert_eq!(report.dispatched, std::slice::from_ref(&fixture.run_id));
    }

    #[test]
    fn a_retry_names_the_attempt_it_means_and_comes_in_time() {
        let fixture = running();
        // Still running: nothing to retry yet.
        assert_eq!(
            refused(
                &fixture,
                request(&fixture, "adopt:retry:early", 1, "failed"),
                fixture.now
            ),
            ErrorCode::IllegalTransition
        );
        let fixture = held();
        for (key, number, disposition, code) in [
            (
                "adopt:retry:number",
                2,
                "failed",
                ErrorCode::RetryDispositionInvalid,
            ),
            (
                "adopt:retry:kind",
                1,
                "timed_out",
                ErrorCode::RetryDispositionInvalid,
            ),
            (
                "adopt:retry:ambiguous",
                1,
                "ambiguous",
                ErrorCode::AmbiguousRetryForbidden,
            ),
        ] {
            assert_eq!(
                refused(
                    &fixture,
                    request(&fixture, key, number, disposition),
                    fixture.now
                ),
                code,
                "{key}"
            );
        }
        let mut unknown = request(&fixture, "adopt:retry:unknown", 1, "failed");
        unknown["runId"] = json!("run-none");
        assert_eq!(refused(&fixture, unknown, fixture.now), ErrorCode::NotFound);
        // Past the deadline, before settlement catches up.
        let late = fixture.now + chrono::TimeDelta::minutes(31);
        assert_eq!(
            refused(
                &fixture,
                request(&fixture, "adopt:retry:late", 1, "failed"),
                late
            ),
            ErrorCode::DeadlineExceeded
        );
        // A paused routine is not retried.
        fixture
            .conn
            .execute(
                "UPDATE automation_definitions SET lifecycle_state = 'paused' WHERE id = 'notes'",
                [],
            )
            .unwrap();
        assert_eq!(
            refused(
                &fixture,
                request(&fixture, "adopt:retry:paused", 1, "failed"),
                fixture.now
            ),
            ErrorCode::IllegalTransition
        );
        // Once settled, the run cannot be retried.
        assert_eq!(settle_finished_runs(&fixture.conn, late).unwrap().failed, 1);
        assert_eq!(
            refused(
                &fixture,
                request(&fixture, "adopt:retry:settled", 1, "failed"),
                fixture.now
            ),
            ErrorCode::IllegalTransition
        );
    }

    #[test]
    fn stale_settlement_cannot_fail_a_dispatched_operator_retry() {
        for already_held in [false, true] {
            let fixture = if already_held { held() } else { running() };
            if !already_held {
                let session: String = fixture
                    .conn
                    .query_row(
                        "SELECT session_id FROM automation_runs WHERE id = ?1",
                        [&fixture.run_id],
                        |row| row.get(0),
                    )
                    .unwrap();
                crate::store::update_session_terminal_if_active(
                    &fixture.conn,
                    &session,
                    "failed",
                    Some(1),
                    &iso(fixture.now),
                )
                .unwrap();
            }
            let path = fixture._temp.path().join("store.sqlite");
            let run_id = fixture.run_id.clone();
            let now = fixture.now;
            super::super::runner::set_after_settlement_snapshot_test_hook(Box::new(move || {
                let conn = crate::store::open_store(&path).unwrap();
                assert_eq!(settle_finished_runs(&conn, now).unwrap().failed, 0);
                let result = execute_attempt_retry(
                    &conn,
                    json!({
                        "action": RETRY_ACTION, "adoptionKey": "adopt:retry:concurrent",
                        "runId": run_id, "priorAttemptNumber": 1, "priorDisposition": "failed",
                    }),
                    now,
                )
                .unwrap();
                assert!(
                    matches!(result, RetryExecution::Success { .. }),
                    "{result:?}"
                );
                assert_eq!(
                    claim_due_occurrence(&conn, "notes", "daemon", 60, now).unwrap(),
                    Some("occ.notes-1".to_owned())
                );
                assert_eq!(
                    dispatch_claimed_occurrences_with_clock(
                        &conn,
                        &NoopSessionRuntime,
                        now,
                        || now,
                    )
                    .unwrap()
                    .dispatched,
                    [run_id]
                );
            }));
            // The held-deadline worker can have captured a later clock value
            // while an operator request that started before the deadline wins.
            let at = if already_held {
                fixture.now + chrono::TimeDelta::minutes(31)
            } else {
                fixture.now
            };
            assert_eq!(settle_finished_runs(&fixture.conn, at).unwrap().failed, 0);
            let states: (String, String, String) = fixture
                .conn
                .query_row(
                    "SELECT r.status, a.state, s.status FROM automation_runs r
                 JOIN automation_attempts a ON a.run_id = r.id AND a.attempt_number = 2
                 JOIN sessions s ON s.id = a.session_id WHERE r.id = ?1",
                    [&fixture.run_id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .unwrap();
            assert_eq!(
                states,
                ("running".into(), "started".into(), "running".into()),
                "already_held={already_held}"
            );
        }
    }

    #[test]
    fn settlement_does_not_create_retry_hold_while_stop_ownership_is_unresolved() {
        let fixture = running();
        let session: String = fixture
            .conn
            .query_row(
                "SELECT session_id FROM automation_runs WHERE id = ?1",
                [&fixture.run_id],
                |row| row.get(0),
            )
            .unwrap();
        assert!(matches!(
            super::super::runner::claim_stop_fence(
                &fixture.conn,
                &fixture.run_id,
                &session,
                "timeout",
                None,
                None,
                fixture.now,
            )
            .unwrap(),
            super::super::runner::StopFenceClaim::Acquired
        ));
        crate::store::update_session_terminal_if_active(
            &fixture.conn,
            &session,
            "failed",
            Some(1),
            &iso(fixture.now),
        )
        .unwrap();
        let before_lease_expiry = fixture.now + chrono::TimeDelta::seconds(29);
        assert_eq!(
            settle_finished_runs(&fixture.conn, before_lease_expiry)
                .unwrap()
                .failed,
            0
        );
        let state: String = fixture
            .conn
            .query_row(
                "SELECT state FROM automation_attempts WHERE run_id = ?1",
                [&fixture.run_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            state, "started",
            "an unresolved stop must retain recovery options"
        );
        // Once the owner resolves its fence, normal failure reconciliation can proceed.
        fixture
            .conn
            .execute(
                "DELETE FROM automation_stop_fences WHERE run_id = ?1",
                [&fixture.run_id],
            )
            .unwrap();
        assert_eq!(
            settle_finished_runs(&fixture.conn, before_lease_expiry)
                .unwrap()
                .failed,
            0
        );
        let state: String = fixture
            .conn
            .query_row(
                "SELECT state FROM automation_attempts WHERE run_id = ?1",
                [&fixture.run_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(state, "failed");
    }

    #[test]
    fn retry_requires_an_original_run_deadline() {
        let fixture = held();
        fixture
            .conn
            .execute(
                "UPDATE automation_runs SET timeout_at = NULL WHERE id = ?1",
                [&fixture.run_id],
            )
            .unwrap();
        assert_eq!(
            refused(
                &fixture,
                request(&fixture, "adopt:retry:no-deadline", 1, "failed"),
                fixture.now
            ),
            ErrorCode::IllegalTransition
        );
    }

    #[test]
    fn held_cancellation_cannot_override_the_deadline() {
        let fixture = held();
        let deadline = fixture.now + chrono::TimeDelta::minutes(30);
        let body = json!({
            "action": OCCURRENCE_CANCEL_ACTION,
            "adoptionKey": "adopt:cancel:expired-hold",
            "occurrenceId": "occ.notes-1"
        });
        let result =
            execute_occurrence_cancel(&fixture.conn, &NoopSessionRuntime, body.clone(), deadline)
                .unwrap();
        let CancelExecution::Rejected(error) = &result else {
            panic!("expired hold was cancelled: {result:?}");
        };
        assert_eq!(error.code(), ErrorCode::IllegalTransition);
        assert_eq!(
            settle_finished_runs(&fixture.conn, deadline)
                .unwrap()
                .failed,
            1
        );
        assert_eq!(
            execute_occurrence_cancel(&fixture.conn, &NoopSessionRuntime, body, deadline,).unwrap(),
            result
        );
        let states: (String, String, String) = fixture
            .conn
            .query_row(
                "SELECT r.status, o.state, a.state FROM automation_runs r
             JOIN automation_occurrences o ON o.id = r.occurrence_id
             JOIN automation_attempts a ON a.run_id = r.id WHERE r.id = ?1",
                [&fixture.run_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(states, ("failed".into(), "failed".into(), "failed".into()));
    }

    #[test]
    fn operator_retry_requires_explicit_unquarantine() {
        let fixture = held();
        super::super::runs::record_retry_exhaustion(
            &fixture.conn,
            "notes",
            "launch_failed",
            "automatic retries exhausted",
            fixture.now,
        )
        .unwrap();
        let body = request(&fixture, "adopt:retry:quarantined", 1, "failed");
        assert_eq!(
            refused(&fixture, body.clone(), fixture.now),
            ErrorCode::IllegalTransition
        );
        assert!(
            super::super::runs::clear_retry_quarantine(&fixture.conn, "notes", fixture.now)
                .unwrap()
        );
        // The adopted refusal remains immutable; a fresh request can retry.
        assert_eq!(
            refused(&fixture, body, fixture.now),
            ErrorCode::IllegalTransition
        );
        assert!(matches!(
            execute_attempt_retry(
                &fixture.conn,
                request(&fixture, "adopt:retry:unquarantined", 1, "failed"),
                fixture.now,
            )
            .unwrap(),
            RetryExecution::Success { .. }
        ));
    }

    #[test]
    fn held_commands_refuse_unresolved_stop_fences_even_after_expiry() {
        for owner in ["cancellation", "timeout", "recovery"] {
            let fixture = held();
            // Persisted uncertainty may outlive the stop worker and its lease.
            fixture
                .conn
                .execute(
                    "INSERT INTO automation_stop_fences
                 (run_id, session_id, owner, operation_key, acquired_at, execution_expires_at)
                 SELECT id, session_id, ?2, 'pending-stop', ?3, ?3
                 FROM automation_runs WHERE id = ?1",
                    params![
                        fixture.run_id,
                        owner,
                        iso(fixture.now - chrono::TimeDelta::seconds(1))
                    ],
                )
                .unwrap();
            assert_eq!(
                refused(
                    &fixture,
                    request(&fixture, "adopt:retry:stop-owned", 1, "failed"),
                    fixture.now,
                ),
                ErrorCode::CancelPending,
                "{owner}"
            );
            let result = execute_occurrence_cancel(
                &fixture.conn,
                &NoopSessionRuntime,
                json!({"action": OCCURRENCE_CANCEL_ACTION,
                       "adoptionKey": "adopt:cancel:stop-owned", "occurrenceId": "occ.notes-1"}),
                fixture.now,
            )
            .unwrap();
            let CancelExecution::Rejected(error) = result else {
                panic!("held cancellation bypassed {owner} stop ownership");
            };
            assert_eq!(error.code(), ErrorCode::CancelPending);
            let (status, attempts, fences): (String, i64, i64) = fixture
                .conn
                .query_row(
                    "SELECT status,
                    (SELECT COUNT(*) FROM automation_attempts WHERE run_id = r.id),
                    (SELECT COUNT(*) FROM automation_stop_fences WHERE run_id = r.id)
                 FROM automation_runs r WHERE id = ?1",
                    [&fixture.run_id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .unwrap();
            assert_eq!((status.as_str(), attempts, fences), ("running", 1, 1));
        }
    }

    #[test]
    fn held_commands_refuse_pending_cancellation_records() {
        for state in ["requested", "stopping"] {
            let fixture = held();
            fixture.conn.execute(
                "INSERT INTO automation_cancellations
                    (adoption_key, request_digest, automation_id, run_id, attempt_id,
                     session_id, scope, requested_by_json, state, requested_at, execution_expires_at)
                 SELECT 'adopt:pending-stop', 'digest', r.automation_id, r.id, a.id,
                        a.session_id, 'run', '{\"principalId\":\"owner\"}', ?2, ?3, ?3
                 FROM automation_runs r JOIN automation_attempts a ON a.run_id = r.id
                 WHERE r.id = ?1",
                params![fixture.run_id, state, iso(fixture.now)],
            ).unwrap();
            assert_eq!(
                refused(
                    &fixture,
                    request(&fixture, "adopt:retry:pending-stop", 1, "failed"),
                    fixture.now,
                ),
                ErrorCode::CancelPending,
                "{state}"
            );
            let result = execute_occurrence_cancel(
                &fixture.conn,
                &NoopSessionRuntime,
                json!({"action": OCCURRENCE_CANCEL_ACTION,
                       "adoptionKey": "adopt:cancel:pending-stop", "occurrenceId": "occ.notes-1"}),
                fixture.now,
            )
            .unwrap();
            let CancelExecution::Rejected(error) = result else {
                panic!("held cancellation bypassed {state} request");
            };
            assert_eq!(error.code(), ErrorCode::CancelPending);
        }
    }

    #[test]
    fn cancelling_the_occurrence_releases_a_held_run() {
        let fixture = held();
        // The failed attempt has settled, so it is not cancelled itself.
        let attempt_id = format!("attempt-{}-1", fixture.run_id);
        match execute_attempt_cancel(
            &fixture.conn,
            &NoopSessionRuntime,
            json!({ "action": ATTEMPT_CANCEL_ACTION, "adoptionKey": "adopt:cancel:attempt",
                    "attemptId": attempt_id }),
            fixture.now,
        )
        .unwrap()
        {
            CancelExecution::Rejected(error) => {
                assert_eq!(error.code(), ErrorCode::IllegalTransition)
            }
            success => panic!("expected a refusal, got {success:?}"),
        }
        let CancelExecution::Success { payload, .. } = execute_occurrence_cancel(
            &fixture.conn,
            &NoopSessionRuntime,
            json!({ "action": OCCURRENCE_CANCEL_ACTION, "adoptionKey": "adopt:cancel:held",
                    "occurrenceId": "occ.notes-1" }),
            fixture.now,
        )
        .unwrap() else {
            panic!("refused");
        };
        assert_eq!(payload["runId"], json!(fixture.run_id));
        let states: (String, String, String) = fixture
            .conn
            .query_row(
                "SELECT run.status, occurrence.state, attempt.state
                 FROM automation_runs AS run
                 JOIN automation_occurrences AS occurrence ON occurrence.id = run.occurrence_id
                 JOIN automation_attempts AS attempt ON attempt.run_id = run.id
                 WHERE run.id = ?1",
                [&fixture.run_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(
            (states.0.as_str(), states.1.as_str(), states.2.as_str()),
            ("cancelled", "cancelled", "failed")
        );
        // Released: nothing is left to retry or to settle at the deadline.
        assert_eq!(
            refused(
                &fixture,
                request(&fixture, "adopt:retry:cancelled", 1, "failed"),
                fixture.now
            ),
            ErrorCode::IllegalTransition
        );
        let late = fixture.now + chrono::TimeDelta::minutes(31);
        assert_eq!(settle_finished_runs(&fixture.conn, late).unwrap().failed, 0);
    }
}
