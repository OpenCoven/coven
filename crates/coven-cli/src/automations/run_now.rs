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
pub(crate) const RESULT_COMMAND: &str = "occurrence.runNow.result.v1";
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
    execute(conn, runtime, body, now, false)
}

/// The envelope requires an immutable completed result and refuses ambiguous
/// replay. Flat callers retain their historical live-projection semantics.
pub(crate) fn execute_run_now_envelope(
    conn: &Connection,
    runtime: &dyn SessionRuntime,
    body: Value,
    now: DateTime<Utc>,
) -> Result<RunNowExecution, String> {
    execute(conn, runtime, body, now, true)
}

fn execute(
    conn: &Connection,
    runtime: &dyn SessionRuntime,
    body: Value,
    now: DateTime<Utc>,
    immutable: bool,
) -> Result<RunNowExecution, String> {
    let mut canonical = canonicalize(&body)
        .map_err(|error| format!("failed to canonicalize runNow request: {error:#}"))?;
    if immutable {
        let mut domain = b"coven.run-now-envelope.v1\0".to_vec();
        domain.append(&mut canonical);
        canonical = domain;
    }
    let digest = sha256_hex(&canonical);
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
                if immutable {
                    return Ok(RunNowExecution::Rejected(typed_error(
                        ErrorCode::AmbiguousRetryForbidden,
                        "runNow dispatch has no retained result; replay cannot launch again",
                    )));
                }
                return Ok(RunNowExecution::Success {
                    payload: live(conn, &occurrence_id)?,
                    replayed: true,
                });
            }
            Some(Adopted::Answered(answer)) => return Ok(answer),
            None => {}
        }
    }
    let dispatch_key = if immutable {
        adoption_key.as_deref().map(result_adoption_key)
    } else {
        None
    };
    if let Some(key) = dispatch_key.as_deref() {
        if adopted(&transaction, key, &digest)?.is_some() {
            return Ok(RunNowExecution::Rejected(replay_mismatch()));
        }
    }
    let claimed = match parse(&body) {
        Ok(request) => claim_in(&transaction, &request, now)?,
        Err(error) => Err(error),
    };
    let automation_id = body.get("automationId").and_then(Value::as_str);
    let stored = match &claimed {
        Ok((occurrence_id, _)) => {
            let mut response =
                json!({ "outcome": "committed", "result": { "occurrenceId": occurrence_id } });
            if let Some(key) = &dispatch_key {
                response["dispatchAdoptionKey"] = json!(key);
            }
            response
        }
        Err(error) => json!({ "outcome": "rejected", "error": error }),
    };
    if claimed.is_ok() {
        if let Some(key) = dispatch_key.as_deref() {
            transaction
                .execute(
                    "INSERT INTO automation_command_reservations
                 (adoption_key, request_digest, command, reserved_at) VALUES (?1, ?2, ?3, ?4)",
                    params![key, digest, RESULT_COMMAND, iso(now)],
                )
                .map_err(|error| format!("failed to reserve runNow result: {error}"))?;
        }
    }
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
        Ok(outcome) => {
            let mut payload = live(conn, &occurrence_id)?;
            if immutable {
                if let Some(error) = outcome.error {
                    payload["error"] = json!(error);
                }
            }
            payload
        }
        Err(error) => {
            let mut payload = live(conn, &occurrence_id)?;
            payload["error"] = json!(error);
            payload
        }
    };
    if immutable {
        if let Some(run_id) = payload.get("runId").and_then(Value::as_str) {
            // A run can retain a proposed session even when launch refused
            // ownership. Capture the original attempt's binding, which also
            // remains stable after a later same-run retry.
            let session: Option<String> = conn
                .query_row(
                    "SELECT session_id FROM automation_attempts
                     WHERE run_id = ?1 AND occurrence_id = ?2 AND attempt_number = 1",
                    params![run_id, occurrence_id],
                    |row| row.get(0),
                )
                .map_err(|error| format!("failed to read initial runNow attempt: {error}"))?;
            payload["sessionId"] = json!(session);
        }
    }
    if let Some(note) = definition_note(&body) {
        payload["note"] = json!(note);
    }
    let dispatch_error = if immutable {
        dispatch_error(&payload)
    } else {
        None
    };
    if let Some(key) = dispatch_key.as_deref() {
        validate_dispatch_payload(conn, &occurrence_id, &payload)?;
        let retained = match &dispatch_error {
            Some(error) => json!({"outcome": "rejected", "error": error, "result": payload}),
            None => json!({"outcome": "committed", "result": payload}),
        };
        // The original adoption remains append-only. Its reserved result key
        // becomes a second immutable adoption only after dispatch returns.
        let transaction =
            rusqlite::Transaction::new_unchecked(conn, TransactionBehavior::Immediate)
                .map_err(|error| format!("failed to begin runNow result retention: {error}"))?;
        let removed = transaction
            .execute(
                "DELETE FROM automation_command_reservations
             WHERE adoption_key = ?1 AND command = ?2 AND request_digest = ?3",
                params![key, RESULT_COMMAND, digest],
            )
            .map_err(|error| format!("failed to consume runNow result reservation: {error}"))?;
        if removed != 1 {
            return Err("runNow result reservation changed before retention".to_owned());
        }
        transaction.execute(
            "INSERT INTO automation_command_adoptions
             (adoption_key, command, automation_id, request_digest, outcome, revision, response_json, adopted_at)
             VALUES (?1, ?2, ?3, ?4, ?5, NULL, ?6, ?7)",
            params![key, RESULT_COMMAND, automation_id, digest, retained["outcome"].as_str(),
                retained.to_string(), iso(Utc::now())],
        ).map_err(|error| format!("failed to retain runNow dispatch result: {error}"))?;
        transaction
            .commit()
            .map_err(|error| format!("failed to commit runNow dispatch result: {error}"))?;
    }
    if let Some(error) = dispatch_error {
        return Ok(RunNowExecution::Rejected(error));
    }
    Ok(RunNowExecution::Success {
        payload,
        replayed: false,
    })
}

fn validate_dispatch_payload(
    conn: &Connection,
    occurrence_id: &str,
    payload: &Value,
) -> Result<(), String> {
    let invalid = || "stored runNow dispatch result has invalid identity or shape".to_owned();
    let object = payload.as_object().ok_or_else(invalid)?;
    if object.keys().any(|key| {
        !matches!(
            key.as_str(),
            "automationId"
                | "occurrenceId"
                | "occurrenceState"
                | "runId"
                | "status"
                | "sessionId"
                | "note"
                | "error"
        )
    }) || payload["occurrenceId"] != occurrence_id
        || !matches!(
            payload["occurrenceState"].as_str(),
            Some(
                "planned"
                    | "claimed"
                    | "running"
                    | "succeeded"
                    | "failed"
                    | "cancelled"
                    | "timed_out"
                    | "recovery_required"
                    | "skipped"
            )
        )
        || object.get("note").is_some_and(|note| {
            note.as_str()
                .is_none_or(|note| note.chars().count() > NOTE_MAX_CHARS)
        })
        || object
            .get("error")
            .is_some_and(|error| error.as_str().is_none_or(str::is_empty))
    {
        return Err(invalid());
    }
    let automation_id: String = conn
        .query_row(
            "SELECT automation_id FROM automation_occurrences WHERE id = ?1",
            [occurrence_id],
            |row| row.get(0),
        )
        .map_err(|error| format!("failed to correlate runNow occurrence: {error}"))?;
    if payload["automationId"] != automation_id {
        return Err(invalid());
    }
    if let Some(run_id) = object.get("runId") {
        let run_id = run_id
            .as_str()
            .filter(|id| !id.is_empty())
            .ok_or_else(invalid)?;
        if !matches!(
            payload["status"].as_str(),
            Some(
                "running"
                    | "succeeded"
                    | "failed"
                    | "cancelled"
                    | "timed_out"
                    | "ambiguous"
                    | "recovery_required"
            )
        ) {
            return Err(invalid());
        }
        let session: Option<String> = conn.query_row(
            "SELECT attempt.session_id FROM automation_runs AS run
             JOIN automation_attempts AS attempt ON attempt.run_id = run.id AND attempt.attempt_number = 1
             WHERE run.id = ?1 AND run.occurrence_id = ?2 AND run.automation_id = ?3
               AND attempt.occurrence_id = ?2",
            params![run_id, occurrence_id, automation_id], |row| row.get(0),
        ).map_err(|error| format!("failed to correlate retained runNow dispatch: {error}"))?;
        let expected_session = session.map(Value::String).unwrap_or(Value::Null);
        if object.get("sessionId") != Some(&expected_session) {
            return Err(invalid());
        }
    } else if object.get("error").is_none()
        || object.contains_key("status")
        || object.contains_key("sessionId")
    {
        return Err(invalid());
    }
    Ok(())
}

fn valid_timestamp(at: &str) -> bool {
    super::contract::types::Timestamp::new(at.to_owned()).is_ok()
        && DateTime::parse_from_rfc3339(at).is_ok()
}

pub(crate) fn result_adoption_key(key: &str) -> String {
    format!("adopt:run-now-result:{}", sha256_hex(key.as_bytes()))
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
        Some("committed") => {
            let occurrence_id = response["result"]["occurrenceId"]
                .as_str()
                .ok_or("stored runNow response has no occurrence")?
                .to_owned();
            let retained = if let Some(key) = response.get("dispatchAdoptionKey") {
                let key = key
                    .as_str()
                    .ok_or("stored runNow dispatch key is invalid")?;
                let expected = result_adoption_key(adoption_key);
                if key != expected {
                    return Err("stored runNow dispatch key has invalid correlation".to_owned());
                }
                let stored = conn.query_row(
                    "SELECT command, request_digest, outcome, automation_id, adopted_at, response_json
                     FROM automation_command_adoptions WHERE adoption_key = ?1",
                    [key],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?, row.get::<_, Option<String>>(3)?,
                        row.get::<_, String>(4)?, row.get::<_, String>(5)?)),
                ).optional().map_err(|error| format!("failed to read runNow dispatch result: {error}"))?;
                let reservation = conn.query_row(
                    "SELECT command, request_digest, reserved_at FROM automation_command_reservations
                     WHERE adoption_key = ?1", [key],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?)),
                ).optional().map_err(|error| format!("failed to read runNow result reservation: {error}"))?;
                if let Some((command, request_digest, outcome, automation_id, at, retained)) =
                    stored
                {
                    let expected_automation: String = conn
                        .query_row(
                            "SELECT automation_id FROM automation_occurrences WHERE id = ?1",
                            [&occurrence_id],
                            |row| row.get(0),
                        )
                        .map_err(|error| format!("failed to correlate runNow result: {error}"))?;
                    if command != RESULT_COMMAND
                        || request_digest != digest
                        || !matches!(outcome.as_str(), "committed" | "rejected")
                        || automation_id.as_deref() != Some(expected_automation.as_str())
                        || !valid_timestamp(&at)
                        || reservation.is_some()
                    {
                        return Err("stored runNow dispatch result metadata is invalid".to_owned());
                    }
                    let response: Value = serde_json::from_str(&retained)
                        .map_err(|error| format!("invalid runNow retained response: {error}"))?;
                    if response["outcome"] != outcome {
                        return Err(
                            "runNow retained outcome does not match its adoption".to_owned()
                        );
                    }
                    Some(retained)
                } else {
                    let Some((command, request_digest, at)) = reservation else {
                        return Err(
                            "runNow dispatch result and reservation are both missing".to_owned()
                        );
                    };
                    if command != RESULT_COMMAND
                        || request_digest != digest
                        || !valid_timestamp(&at)
                    {
                        return Err(
                            "stored runNow result reservation metadata is invalid".to_owned()
                        );
                    }
                    None
                }
            } else {
                None
            };
            if let Some(retained) = retained {
                let retained: Value = serde_json::from_str(&retained).map_err(|error| {
                    format!("stored runNow dispatch result is invalid: {error}")
                })?;
                let payload = &retained["result"];
                validate_dispatch_payload(conn, &occurrence_id, payload)?;
                match retained["outcome"].as_str() {
                    Some("committed") if payload.get("error").is_none() => {
                        Adopted::Answered(RunNowExecution::Success {
                            payload: payload.clone(),
                            replayed: true,
                        })
                    }
                    Some("rejected") => {
                        let error = dispatch_error(payload)
                            .ok_or("retained runNow rejection has no dispatch error")?;
                        if serde_json::to_value(&error).map_err(|error| error.to_string())?
                            != retained["error"]
                        {
                            return Err(
                                "retained runNow rejection has an invalid typed error".to_owned()
                            );
                        }
                        Adopted::Answered(RunNowExecution::Rejected(error))
                    }
                    _ => return Err("retained runNow dispatch outcome is invalid".to_owned()),
                }
            } else {
                Adopted::Claimed(occurrence_id)
            }
        }
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

fn dispatch_error(payload: &Value) -> Option<ErrorEnvelope> {
    let message = payload.get("error")?.as_str()?;
    let correlation = payload
        .as_object()?
        .iter()
        .filter(|(key, _)| {
            matches!(
                key.as_str(),
                "automationId"
                    | "occurrenceId"
                    | "occurrenceState"
                    | "runId"
                    | "status"
                    | "sessionId"
            )
        })
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect::<serde_json::Map<_, _>>();
    Some(
        typed_error(ErrorCode::Internal, message).with_details(std::collections::BTreeMap::from([
            ("dispatchResult".to_owned(), Value::Object(correlation)),
        ])),
    )
}

fn typed_error(code: ErrorCode, message: impl Into<String>) -> ErrorEnvelope {
    // Runtime diagnostics are not protocol-bounded. Normalize identically
    // before initial retention and replay, without splitting UTF-8 characters.
    let message: String = message.into().chars().take(1_000).collect();
    let message = if message.is_empty() {
        "runNow failed without a diagnostic".to_owned()
    } else {
        message
    };
    ErrorEnvelope::try_new(code, message, false)
        .expect("normalized runNow error messages satisfy protocol bounds")
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

    fn envelope_request(key: &str) -> Value {
        json!({
            "action": super::super::command_envelope::ACTION,
            "envelope": {
                "schemaVersion": "coven.automations.v1",
                "command": COMMAND,
                "adoptionKey": key,
                "origin": {
                    "principal": { "principalId": "principal:owner" },
                    "channel": "sdk", "correlationId": "corr-run-now-replay"
                },
                "intent": { "statement": "Run the notes routine once." },
                "payload": { "automationId": "notes", "note": "before the review" }
            }
        })
    }

    #[test]
    fn envelope_dispatch_replays_the_original_result_after_restart() {
        let (temp, conn) = store("ACTIVE");
        let runtime = CountingRuntime::default();
        let body = envelope_request("adopt:run:envelope");
        let (status, first) = crate::control_plane::route_action(body.clone(), &conn, &runtime);
        assert_eq!(status, 200, "{first:?}");
        let first = first.result.unwrap();
        assert_eq!(first["outcome"], "committed");
        assert_eq!(first["result"]["status"], "running");
        conn.execute("UPDATE automation_runs SET status = 'cancelled'", [])
            .unwrap();
        drop(conn);
        let conn = crate::store::open_store(&temp.path().join("store.sqlite")).unwrap();
        let (status, replay) = crate::control_plane::route_action(body, &conn, &runtime);
        assert_eq!(status, 200, "{replay:?}");
        let replay = replay.result.unwrap();
        assert_eq!(replay["outcome"], "replayed");
        assert_eq!(replay["result"], first["result"]);
        assert!(replay["replay"]["firstCommittedAt"].is_string());
        assert_eq!(runtime.0.load(Ordering::SeqCst), 1);
    }

    struct ReentrantReplayRuntime {
        path: std::path::PathBuf,
        body: Value,
        launches: AtomicUsize,
    }

    impl SessionRuntime for ReentrantReplayRuntime {
        fn launch_session(&self, _: &SessionLaunch) -> anyhow::Result<()> {
            self.launches.fetch_add(1, Ordering::SeqCst);
            let conn = crate::store::open_store(&self.path)?;
            let duplicate_runtime = CountingRuntime::default();
            let (status, answer) =
                crate::control_plane::route_action(self.body.clone(), &conn, &duplicate_runtime);
            assert_eq!(status, 422, "{answer:?}");
            assert_eq!(
                answer.error.unwrap()["code"],
                json!(ErrorCode::AmbiguousRetryForbidden)
            );
            assert!(answer.event.is_none());
            assert_eq!(duplicate_runtime.0.load(Ordering::SeqCst), 0);
            Ok(())
        }
        fn send_input(&self, _: &str, _: &Value) -> anyhow::Result<()> {
            Ok(())
        }
        fn kill_session(&self, _: &str) -> anyhow::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn an_inflight_envelope_duplicate_cannot_launch_again() {
        let (temp, conn) = store("ACTIVE");
        let body = envelope_request("adopt:run:inflight");
        let runtime = ReentrantReplayRuntime {
            path: temp.path().join("store.sqlite"),
            body: body.clone(),
            launches: AtomicUsize::new(0),
        };
        let (status, first) = crate::control_plane::route_action(body.clone(), &conn, &runtime);
        assert_eq!(status, 200, "{first:?}");
        let (status, replay) = crate::control_plane::route_action(body, &conn, &runtime);
        assert_eq!(status, 200, "{replay:?}");
        assert_eq!(
            replay.result.unwrap()["result"],
            first.result.unwrap()["result"]
        );
        assert_eq!(runtime.launches.load(Ordering::SeqCst), 1);
    }

    #[derive(Default)]
    struct AcknowledgementFailureRuntime(AtomicUsize);

    impl SessionRuntime for AcknowledgementFailureRuntime {
        fn launch_session(&self, _: &SessionLaunch) -> anyhow::Result<()> {
            anyhow::bail!("expected adopted launch")
        }
        fn launch_adopted_session(
            &self,
            _: &SessionLaunch,
            _: Option<crate::maintenance_gate::WriterLease>,
            ownership_established: &mut dyn FnMut() -> anyhow::Result<()>,
        ) -> anyhow::Result<()> {
            self.0.fetch_add(1, Ordering::SeqCst);
            ownership_established()?;
            anyhow::bail!("synthetic acknowledgement failure")
        }
        fn send_input(&self, _: &str, _: &Value) -> anyhow::Result<()> {
            Ok(())
        }
        fn kill_session(&self, _: &str) -> anyhow::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn ownership_acknowledgement_failure_retains_correlation_and_replays() {
        let (temp, conn) = store("ACTIVE");
        let runtime = AcknowledgementFailureRuntime::default();
        let body = envelope_request("adopt:run:acknowledgement");
        let (status, first) = crate::control_plane::route_action(body.clone(), &conn, &runtime);
        assert_eq!(status, 500, "{first:?}");
        let error = first.error.unwrap();
        assert!(error["message"]
            .as_str()
            .unwrap()
            .contains("ownership was established"));
        let correlation = &error["details"]["dispatchResult"];
        assert!(correlation["runId"].is_string(), "{error}");
        assert!(correlation["sessionId"].is_string(), "{error}");
        assert_eq!(correlation["status"], "running");
        assert_eq!(correlation["occurrenceState"], "running");
        assert!(first.event.is_none());
        drop(conn);
        let conn = crate::store::open_store(&temp.path().join("store.sqlite")).unwrap();
        let (replay_status, replay) = crate::control_plane::route_action(body, &conn, &runtime);
        assert_eq!(replay_status, status);
        assert_eq!(replay.error, Some(error));
        assert!(replay.event.is_none());
        assert_eq!(runtime.0.load(Ordering::SeqCst), 1);
        let state: String = conn
            .query_row("SELECT status FROM automation_runs", [], |r| r.get(0))
            .unwrap();
        assert_eq!(state, "running");
    }

    #[test]
    fn refusal_replay_keeps_original_attempt_after_manual_retry() {
        let (temp, conn) = store("ACTIVE");
        let refusal_runtime = RefusingRuntime::default();
        let body = envelope_request("adopt:run:before-retry");
        let (status, first) =
            crate::control_plane::route_action(body.clone(), &conn, &refusal_runtime);
        assert_eq!(status, 500, "{first:?}");
        let error = first.error.unwrap();
        let run_id: String = conn
            .query_row("SELECT id FROM automation_runs", [], |r| r.get(0))
            .unwrap();
        let now = Utc::now();
        let retry = super::super::attempt_retry::execute_attempt_retry(&conn, json!({
            "action": "coven.automations.attempt.retry.v1", "adoptionKey": "adopt:retry:run-now",
            "runId": run_id, "priorAttemptNumber": 1, "priorDisposition": "failed"
        }), now).unwrap();
        assert!(
            matches!(
                retry,
                super::super::attempt_retry::RetryExecution::Success { .. }
            ),
            "{retry:?}"
        );
        assert!(
            super::super::occurrences::claim_due_occurrence(&conn, "notes", "daemon", 60, now)
                .unwrap()
                .is_some()
        );
        let retry_runtime = CountingRuntime::default();
        let report = super::super::runner::dispatch_claimed_occurrences_with_clock(
            &conn,
            &retry_runtime,
            now,
            || now,
        )
        .unwrap();
        assert_eq!(report.dispatched, vec![run_id]);
        drop(conn);
        let conn = crate::store::open_store(&temp.path().join("store.sqlite")).unwrap();
        let (replay_status, replay) =
            crate::control_plane::route_action(body, &conn, &retry_runtime);
        assert_eq!(replay_status, status, "{replay:?}");
        assert_eq!(replay.error, Some(error));
        assert!(replay.event.is_none());
        assert_eq!(refusal_runtime.0.load(Ordering::SeqCst), 1);
        assert_eq!(retry_runtime.0.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn envelope_dispatch_without_retained_result_never_relaunches() {
        let (_temp, conn) = store("ACTIVE");
        let runtime = CountingRuntime::default();
        conn.execute_batch(
            "CREATE TRIGGER fail_run_now_result BEFORE INSERT ON automation_command_adoptions
             WHEN NEW.command = 'occurrence.runNow.result.v1'
             BEGIN SELECT RAISE(ABORT, 'synthetic crash after dispatch'); END;",
        )
        .unwrap();
        let body = envelope_request("adopt:run:envelope-crash");
        let (_, first) = crate::control_plane::route_action(body.clone(), &conn, &runtime);
        assert_eq!(
            first.error.as_ref().unwrap()["code"],
            json!(ErrorCode::Internal)
        );
        assert_eq!(runtime.0.load(Ordering::SeqCst), 1);
        conn.execute_batch("DROP TRIGGER fail_run_now_result")
            .unwrap();
        let (_, replay) = crate::control_plane::route_action(body, &conn, &runtime);
        assert_eq!(
            replay.error.as_ref().unwrap()["code"],
            json!(ErrorCode::AmbiguousRetryForbidden)
        );
        assert!(replay.event.is_none());
        assert_eq!(runtime.0.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn envelope_and_flat_adoption_keys_cannot_cross_wire() {
        for envelope_first in [false, true] {
            let (_temp, conn) = store("ACTIVE");
            let runtime = CountingRuntime::default();
            let key = "adopt:run:cross-wire";
            if envelope_first {
                let (status, answer) =
                    crate::control_plane::route_action(envelope_request(key), &conn, &runtime);
                assert_eq!(status, 200, "{answer:?}");
                assert_eq!(
                    refused(&conn, &runtime, request(key)),
                    ErrorCode::AdoptionReplayMismatch
                );
            } else {
                assert!(matches!(
                    execute_run_now(&conn, &runtime, request(key), Utc::now()).unwrap(),
                    RunNowExecution::Success {
                        replayed: false,
                        ..
                    }
                ));
                let (status, answer) =
                    crate::control_plane::route_action(envelope_request(key), &conn, &runtime);
                assert_eq!(status, 409, "{answer:?}");
                assert_eq!(
                    answer.error.unwrap()["code"],
                    json!(ErrorCode::AdoptionReplayMismatch)
                );
                assert!(answer.event.is_none());
            }
            assert_eq!(runtime.0.load(Ordering::SeqCst), 1);
            assert_eq!(runs(&conn), 1);
        }
    }

    #[test]
    fn occupied_result_key_is_refused_before_claim_or_dispatch() {
        let (_temp, conn) = store("ACTIVE");
        let runtime = CountingRuntime::default();
        let key = "adopt:run:occupied-result";
        conn.execute(
            "INSERT INTO automation_command_reservations
             (adoption_key, request_digest, command, reserved_at) VALUES (?1, 'other', 'other', ?2)",
            params![result_adoption_key(key), iso(Utc::now())],
        ).unwrap();
        let (status, answer) =
            crate::control_plane::route_action(envelope_request(key), &conn, &runtime);
        assert_eq!(status, 409, "{answer:?}");
        assert!(answer.event.is_none());
        assert_eq!(runtime.0.load(Ordering::SeqCst), 0);
        assert_eq!(runs(&conn), 0);
        let occurrences: i64 = conn
            .query_row("SELECT COUNT(*) FROM automation_occurrences", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(occurrences, 0);
    }

    #[test]
    fn corrupt_retained_dispatch_metadata_is_internal_failure() {
        for (column, value) in [
            ("command", "wrong.command"),
            ("request_digest", "wrong-digest"),
            ("outcome", "rejected"),
            ("automation_id", "wrong-automation"),
            ("adopted_at", "invalid-time"),
        ] {
            let (_temp, conn) = store("ACTIVE");
            let runtime = CountingRuntime::default();
            let key = "adopt:run:corrupt-result";
            let body = envelope_request(key);
            let (status, answer) =
                crate::control_plane::route_action(body.clone(), &conn, &runtime);
            assert_eq!(status, 200, "{answer:?}");
            // Deliberately corrupt durable evidence; production retains these guards.
            conn.execute_batch("DROP TRIGGER automation_command_adoptions_immutable_columns")
                .unwrap();
            conn.execute(
                &format!(
                    "UPDATE automation_command_adoptions SET {column} = ?1 WHERE adoption_key = ?2"
                ),
                params![value, result_adoption_key(key)],
            )
            .unwrap();
            let (status, answer) = crate::control_plane::route_action(body, &conn, &runtime);
            assert_eq!(status, 500, "corrupt {column}: {answer:?}");
            assert_eq!(answer.error.unwrap()["code"], json!(ErrorCode::Internal));
            assert!(answer.event.is_none());
            assert_eq!(runtime.0.load(Ordering::SeqCst), 1);
        }
    }

    #[derive(Default)]
    struct RefusingRuntime(AtomicUsize);

    #[derive(Default)]
    struct OversizedRefusalRuntime(AtomicUsize);

    impl SessionRuntime for OversizedRefusalRuntime {
        fn launch_session(&self, _: &SessionLaunch) -> anyhow::Result<()> {
            self.0.fetch_add(1, Ordering::SeqCst);
            anyhow::bail!("synthetic oversized launch refusal {}", "🦊".repeat(2000))
        }
        fn send_input(&self, _: &str, _: &Value) -> anyhow::Result<()> {
            Ok(())
        }
        fn kill_session(&self, _: &str) -> anyhow::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn oversized_launch_error_is_a_bounded_replayable_rejection() {
        let (temp, conn) = store("ACTIVE");
        let runtime = OversizedRefusalRuntime::default();
        let body = envelope_request("adopt:run:oversized-refusal");
        let (status, first) = crate::control_plane::route_action(body.clone(), &conn, &runtime);
        assert!(status >= 400, "{first:?}");
        let error = first.error.unwrap();
        assert!(error["message"].as_str().unwrap().chars().count() <= 1000);
        drop(conn);
        let conn = crate::store::open_store(&temp.path().join("store.sqlite")).unwrap();
        let (replay_status, replay) = crate::control_plane::route_action(body, &conn, &runtime);
        assert_eq!(replay_status, status);
        assert_eq!(replay.error, Some(error));
        assert!(replay.event.is_none());
        assert_eq!(runtime.0.load(Ordering::SeqCst), 1);
    }

    impl SessionRuntime for RefusingRuntime {
        fn launch_session(&self, _: &SessionLaunch) -> anyhow::Result<()> {
            self.0.fetch_add(1, Ordering::SeqCst);
            anyhow::bail!("synthetic launch refusal")
        }
        fn send_input(&self, _: &str, _: &Value) -> anyhow::Result<()> {
            Ok(())
        }
        fn kill_session(&self, _: &str) -> anyhow::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn envelope_launch_refusal_replays_a_typed_rejection() {
        let (temp, conn) = store("ACTIVE");
        let runtime = RefusingRuntime::default();
        let body = envelope_request("adopt:run:launch-refusal");
        let (status, first) = crate::control_plane::route_action(body.clone(), &conn, &runtime);
        assert!(status >= 400, "{first:?}");
        assert!(!first.ok && !first.accepted);
        assert!(first.event.is_none());
        let error = first.error.unwrap();
        assert!(error["code"].is_string());
        assert!(
            error["message"]
                .as_str()
                .unwrap()
                .contains("synthetic launch refusal"),
            "{error}"
        );
        assert_eq!(first.result.unwrap()["outcome"], "rejected");
        drop(conn);
        let conn = crate::store::open_store(&temp.path().join("store.sqlite")).unwrap();
        let (replay_status, replay) = crate::control_plane::route_action(body, &conn, &runtime);
        assert_eq!(replay_status, status);
        assert_eq!(replay.error, Some(error));
        assert_eq!(replay.result.unwrap()["outcome"], "rejected");
        assert!(replay.event.is_none());
        assert_eq!(runtime.0.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn retained_dispatch_payload_identity_and_shape_are_validated() {
        for (field, value) in [
            ("automationId", json!("other-automation")),
            ("runId", json!("run-other")),
            ("sessionId", json!("session-other")),
            ("status", json!("invented-state")),
            ("occurrenceState", Value::Null),
            ("runId", Value::Null),
        ] {
            let (_temp, conn) = store("ACTIVE");
            let runtime = CountingRuntime::default();
            let key = "adopt:run:corrupt-payload";
            let body = envelope_request(key);
            let (status, first) = crate::control_plane::route_action(body.clone(), &conn, &runtime);
            assert_eq!(status, 200, "{first:?}");
            let mut payload = first.result.unwrap()["result"].clone();
            payload[field] = value;
            // Deliberate corruption fixture; immutable production rows are unchanged.
            conn.execute_batch("DROP TRIGGER automation_command_adoptions_response_update_guard")
                .unwrap();
            conn.execute(
                "UPDATE automation_command_adoptions SET response_json = ?1 WHERE adoption_key = ?2",
                params![json!({"outcome": "committed", "result": payload}).to_string(), result_adoption_key(key)],
            ).unwrap();
            let (status, answer) = crate::control_plane::route_action(body, &conn, &runtime);
            assert_eq!(status, 500, "corrupt {field}: {answer:?}");
            assert!(answer.event.is_none());
            assert_eq!(runtime.0.load(Ordering::SeqCst), 1);
        }
    }

    #[test]
    fn retained_result_timestamp_is_captured_after_dispatch() {
        let (_temp, conn) = store("ACTIVE");
        let runtime = CountingRuntime::default();
        let key = "adopt:run:result-time";
        let claim_time = Utc::now() - chrono::Duration::seconds(30);
        assert!(matches!(
            execute_run_now_envelope(&conn, &runtime, request(key), claim_time).unwrap(),
            RunNowExecution::Success {
                replayed: false,
                ..
            }
        ));
        let at: String = conn
            .query_row(
                "SELECT adopted_at FROM automation_command_adoptions WHERE adoption_key = ?1",
                [result_adoption_key(key)],
                |row| row.get(0),
            )
            .unwrap();
        assert!(
            DateTime::parse_from_rfc3339(&at).unwrap() > claim_time,
            "result commitment must not reuse claim-entry timestamp {at}"
        );
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
    fn flat_replay_keeps_live_projection_after_restart() {
        let (temp, conn) = store("ACTIVE");
        let runtime = CountingRuntime::default();
        let body = request("adopt:run:immutable");
        let RunNowExecution::Success {
            payload: original, ..
        } = execute_run_now(&conn, &runtime, body.clone(), Utc::now()).unwrap()
        else {
            panic!("first dispatch was refused");
        };
        conn.execute(
            "UPDATE automation_occurrences SET state = 'cancelled' WHERE id = ?1",
            [original["occurrenceId"].as_str().unwrap()],
        )
        .unwrap();
        conn.execute(
            "UPDATE automation_runs SET status = 'cancelled' WHERE id = ?1",
            [original["runId"].as_str().unwrap()],
        )
        .unwrap();
        drop(conn);
        let conn = crate::store::open_store(&temp.path().join("store.sqlite")).unwrap();
        let RunNowExecution::Success {
            payload: replay,
            replayed,
        } = execute_run_now(&conn, &runtime, body, Utc::now()).unwrap()
        else {
            panic!("replay was refused");
        };
        assert!(replayed);
        assert_eq!(replay["status"], "cancelled");
        assert_eq!(replay["occurrenceState"], "cancelled");
        assert!(replay.get("note").is_none());
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
