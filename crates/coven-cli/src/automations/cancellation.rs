use chrono::{DateTime, SecondsFormat, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Value};

use super::contract::canonical_json::{canonicalize, sha256_hex};
use super::contract::error::{ErrorCode, ErrorEnvelope};
use super::contract::types::AdoptionKey;
use crate::api::SessionRuntime;

pub const AUTOMATION_CANCELLATIONS_SCHEMA_SQL: &str = "
    CREATE TABLE IF NOT EXISTS automation_cancellations (
        adoption_key TEXT PRIMARY KEY NOT NULL,
        request_digest TEXT NOT NULL,
        automation_id TEXT NOT NULL,
        run_id TEXT NOT NULL UNIQUE,
        attempt_id TEXT NOT NULL,
        session_id TEXT NOT NULL,
        scope TEXT NOT NULL CHECK (scope = 'run'),
        requested_by_json TEXT NOT NULL,
        reason TEXT,
        state TEXT NOT NULL CHECK (
            state IN ('requested', 'cancelled', 'recovery_required', 'rejected')
        ),
        requested_at TEXT NOT NULL,
        acknowledged_at TEXT,
        reconciled_at TEXT,
        result_json TEXT
    );
";

#[derive(Debug, Clone)]
pub struct CancellationSuccess {
    pub payload: Value,
    pub replayed: bool,
}

#[derive(Debug, Clone)]
pub enum CancellationExecution {
    Success(CancellationSuccess),
    Rejected(ErrorEnvelope),
}

pub fn cancellation_for_run(conn: &Connection, run_id: &str) -> Result<Option<Value>, String> {
    let row = conn
        .query_row(
            "SELECT scope, requested_by_json, reason, state, requested_at,
                    acknowledged_at, reconciled_at
             FROM automation_cancellations
             WHERE run_id = ?1",
            [run_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, Option<String>>(6)?,
                ))
            },
        )
        .optional()
        .map_err(|error| format!("failed to query run cancellation: {error}"))?;
    let Some((
        scope,
        requested_by_json,
        reason,
        state,
        requested_at,
        acknowledged_at,
        reconciled_at,
    )) = row
    else {
        return Ok(None);
    };
    let requested_by: Value = serde_json::from_str(&requested_by_json)
        .map_err(|error| format!("stored cancellation requester is invalid: {error}"))?;
    let mut cancellation = json!({
        "scope": scope,
        "requestedBy": requested_by,
        "status": state,
        "requestedAt": requested_at,
    });
    if let Some(reason) = reason {
        cancellation["reason"] = json!(reason);
    }
    if let Some(acknowledged_at) = acknowledged_at {
        cancellation["acknowledgedAt"] = json!(acknowledged_at);
    }
    if let Some(reconciled_at) = reconciled_at {
        cancellation["reconciledAt"] = json!(reconciled_at);
    }
    Ok(Some(cancellation))
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CancellationRequest {
    action: String,
    adoption_key: String,
    run_id: String,
    attempt_id: String,
    runtime_correlation: RuntimeCorrelation,
    scope: String,
    reason: String,
    requested_by: RequestedBy,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RuntimeCorrelation {
    session_id: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RequestedBy {
    principal_id: String,
}

#[derive(Debug)]
struct ReservedCancellation {
    request: CancellationRequest,
    digest: String,
    automation_id: String,
    requested_at: String,
}

pub fn execute_run_cancellation(
    conn: &Connection,
    runtime: &dyn SessionRuntime,
    body: Value,
    now: DateTime<Utc>,
) -> Result<CancellationExecution, String> {
    let request: CancellationRequest = match serde_json::from_value(body.clone()) {
        Ok(request) => request,
        Err(error) => {
            return Ok(CancellationExecution::Rejected(typed_error(
                ErrorCode::ValidationFailed,
                format!("invalid run cancellation request shape: {error}"),
            )));
        }
    };
    if let Err(error) = validate_request(&request) {
        return Ok(CancellationExecution::Rejected(typed_error(
            ErrorCode::ValidationFailed,
            error,
        )));
    }
    let digest = sha256_hex(
        &canonicalize(&body)
            .map_err(|error| format!("failed to canonicalize cancellation request: {error:#}"))?,
    );

    if let Some(replay) = load_adopted_response(conn, &request.adoption_key, &digest)? {
        return Ok(replay);
    }

    let reservation = match reserve_cancellation(conn, request, digest, now)? {
        Ok(reservation) => reservation,
        Err(error) => {
            persist_rejection(conn, &body, &error, now)?;
            return Ok(CancellationExecution::Rejected(error));
        }
    };

    if let Some(payload) = load_reserved_result(conn, &reservation)? {
        return Ok(CancellationExecution::Success(CancellationSuccess {
            payload,
            replayed: true,
        }));
    }

    match runtime.kill_session(&reservation.request.runtime_correlation.session_id) {
        Ok(()) => {
            let settled = super::runner::settle_confirmed_stop(
                conn,
                &reservation.request.run_id,
                &reservation.request.runtime_correlation.session_id,
                super::runner::ConfirmedStop::Cancelled,
                now,
            )?;
            if !settled {
                let error = typed_error(
                    ErrorCode::IllegalTransition,
                    "automation completion already won the cancellation race",
                );
                finalize_rejection(conn, &reservation, &error, now)?;
                return Ok(CancellationExecution::Rejected(error));
            }
            let timestamp = iso(now);
            let payload = cancellation_payload(
                &reservation,
                "cancelled",
                Some(&timestamp),
                Some(&timestamp),
            );
            finalize_success(conn, &reservation, "cancelled", &payload, now)?;
            Ok(CancellationExecution::Success(CancellationSuccess {
                payload,
                replayed: false,
            }))
        }
        Err(_) => {
            super::runner::mark_unconfirmed_stop_for_recovery(
                conn,
                &reservation.request.run_id,
                "cancellation stop was not confirmed",
                now,
            )?;
            let payload = cancellation_payload(&reservation, "recovery_required", None, None);
            finalize_success(conn, &reservation, "recovery_required", &payload, now)?;
            Ok(CancellationExecution::Success(CancellationSuccess {
                payload,
                replayed: false,
            }))
        }
    }
}

fn validate_request(request: &CancellationRequest) -> Result<(), String> {
    if request.action != "coven.automations.run.cancel.v1" {
        return Err("run cancellation action is invalid".to_string());
    }
    AdoptionKey::new(request.adoption_key.clone())
        .map_err(|error| format!("invalid adoptionKey: {error}"))?;
    if request.run_id.trim().is_empty()
        || request.attempt_id.trim().is_empty()
        || request.runtime_correlation.session_id.trim().is_empty()
        || request.reason.trim().is_empty()
        || request.requested_by.principal_id.trim().is_empty()
    {
        return Err(
            "runId, attemptId, runtimeCorrelation.sessionId, reason, and requestedBy.principalId are required"
                .to_string(),
        );
    }
    if request.scope != "run" {
        return Err("run cancellation scope must be `run`".to_string());
    }
    Ok(())
}

fn reserve_cancellation(
    conn: &Connection,
    request: CancellationRequest,
    digest: String,
    now: DateTime<Utc>,
) -> Result<Result<ReservedCancellation, ErrorEnvelope>, String> {
    let existing = conn
        .query_row(
            "SELECT request_digest, automation_id, run_id, attempt_id, session_id,
                    requested_at
             FROM automation_cancellations
             WHERE adoption_key = ?1",
            [&request.adoption_key],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                ))
            },
        )
        .optional()
        .map_err(|error| format!("failed to query cancellation reservation: {error}"))?;
    if let Some((existing_digest, automation_id, run_id, attempt_id, session_id, requested_at)) =
        existing
    {
        if existing_digest != digest {
            return Ok(Err(replay_mismatch_error()));
        }
        if run_id != request.run_id
            || attempt_id != request.attempt_id
            || session_id != request.runtime_correlation.session_id
        {
            return Ok(Err(replay_mismatch_error()));
        }
        return Ok(Ok(ReservedCancellation {
            request,
            digest,
            automation_id,
            requested_at,
        }));
    }
    let pending_adoption_key = conn
        .query_row(
            "SELECT adoption_key
             FROM automation_cancellations
             WHERE run_id = ?1",
            [&request.run_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|error| format!("failed to query pending run cancellation: {error}"))?;
    if pending_adoption_key.is_some() {
        return Ok(Err(typed_error(
            ErrorCode::CancelPending,
            "the automation run already has a cancellation request",
        )));
    }

    let live = conn
        .query_row(
            "SELECT r.automation_id, r.status, a.state, s.status
             FROM automation_runs AS r
             JOIN automation_attempts AS a ON a.run_id = r.id
             JOIN sessions AS s ON s.id = a.session_id
             WHERE r.id = ?1
               AND a.id = ?2
               AND a.session_id = ?3",
            params![
                request.run_id,
                request.attempt_id,
                request.runtime_correlation.session_id
            ],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            },
        )
        .optional()
        .map_err(|error| format!("failed to resolve cancellation target: {error}"))?;
    let Some((automation_id, run_state, attempt_state, session_state)) = live else {
        return Ok(Err(illegal_transition_error(
            "run, attempt, and runtime correlation do not identify one live attempt",
        )));
    };
    if run_state != "running"
        || !matches!(
            attempt_state.as_str(),
            "dispatching" | "started" | "observing"
        )
        || !matches!(session_state.as_str(), "created" | "running" | "orphaned")
    {
        return Ok(Err(illegal_transition_error(
            "the targeted automation attempt is no longer cancellable",
        )));
    }

    let requested_at = iso(now);
    conn.execute(
        "INSERT INTO automation_cancellations (
            adoption_key, request_digest, automation_id, run_id, attempt_id,
            session_id, scope, requested_by_json, reason, state, requested_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'run', ?7, ?8, 'requested', ?9)",
        params![
            request.adoption_key,
            digest,
            automation_id,
            request.run_id,
            request.attempt_id,
            request.runtime_correlation.session_id,
            serde_json::to_string(&json!({
                "principalId": request.requested_by.principal_id
            }))
            .map_err(|error| format!("failed to serialize cancellation requester: {error}"))?,
            request.reason,
            requested_at,
        ],
    )
    .map_err(|error| format!("failed to reserve automation cancellation: {error}"))?;

    Ok(Ok(ReservedCancellation {
        request,
        digest,
        automation_id,
        requested_at,
    }))
}

fn load_reserved_result(
    conn: &Connection,
    reservation: &ReservedCancellation,
) -> Result<Option<Value>, String> {
    let result_json = conn
        .query_row(
            "SELECT result_json
             FROM automation_cancellations
             WHERE adoption_key = ?1 AND request_digest = ?2",
            params![reservation.request.adoption_key, reservation.digest],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()
        .map_err(|error| format!("failed to query reserved cancellation result: {error}"))?
        .flatten();
    result_json
        .map(|value| {
            serde_json::from_str(&value)
                .map_err(|error| format!("stored cancellation result is invalid: {error}"))
        })
        .transpose()
}

fn load_adopted_response(
    conn: &Connection,
    adoption_key: &str,
    digest: &str,
) -> Result<Option<CancellationExecution>, String> {
    let row = conn
        .query_row(
            "SELECT command, request_digest, outcome, response_json
             FROM automation_command_adoptions
             WHERE adoption_key = ?1",
            [adoption_key],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            },
        )
        .optional()
        .map_err(|error| format!("failed to query cancellation adoption: {error}"))?;
    let Some((command, stored_digest, outcome, response_json)) = row else {
        return Ok(None);
    };
    if command != "run.cancel.v1" || stored_digest != digest {
        return Ok(Some(CancellationExecution::Rejected(
            replay_mismatch_error(),
        )));
    }
    let response: Value = serde_json::from_str(&response_json)
        .map_err(|error| format!("stored cancellation response is invalid: {error}"))?;
    match outcome.as_str() {
        "committed" => {
            let payload = response.get("result").cloned().ok_or_else(|| {
                "stored committed cancellation response is missing result".to_string()
            })?;
            Ok(Some(CancellationExecution::Success(CancellationSuccess {
                payload,
                replayed: true,
            })))
        }
        "rejected" => {
            let error =
                serde_json::from_value(response.get("error").cloned().ok_or_else(|| {
                    "stored rejected cancellation response is missing error".to_string()
                })?)
                .map_err(|error| format!("stored cancellation error is invalid: {error}"))?;
            Ok(Some(CancellationExecution::Rejected(error)))
        }
        other => Err(format!(
            "stored cancellation adoption has invalid outcome `{other}`"
        )),
    }
}

fn finalize_success(
    conn: &Connection,
    reservation: &ReservedCancellation,
    state: &str,
    payload: &Value,
    now: DateTime<Utc>,
) -> Result<(), String> {
    let transaction = conn
        .unchecked_transaction()
        .map_err(|error| format!("failed to begin cancellation finalization: {error}"))?;
    let now_iso = iso(now);
    let (acknowledged_at, reconciled_at) = if state == "cancelled" {
        (Some(now_iso.as_str()), Some(now_iso.as_str()))
    } else {
        (None, None)
    };
    let result_json = serde_json::to_string(payload)
        .map_err(|error| format!("failed to serialize cancellation result: {error}"))?;
    transaction
        .execute(
            "UPDATE automation_cancellations
             SET state = ?2,
                 acknowledged_at = ?3,
                 reconciled_at = ?4,
                 result_json = ?5
             WHERE adoption_key = ?1 AND state = 'requested'",
            params![
                reservation.request.adoption_key,
                state,
                acknowledged_at,
                reconciled_at,
                result_json
            ],
        )
        .map_err(|error| format!("failed to finalize automation cancellation: {error}"))?;
    insert_adoption(
        &transaction,
        reservation,
        "committed",
        &json!({"outcome": "committed", "result": payload}),
        now,
    )?;
    transaction
        .commit()
        .map_err(|error| format!("failed to commit cancellation finalization: {error}"))
}

fn finalize_rejection(
    conn: &Connection,
    reservation: &ReservedCancellation,
    error: &ErrorEnvelope,
    now: DateTime<Utc>,
) -> Result<(), String> {
    let transaction = conn
        .unchecked_transaction()
        .map_err(|error| format!("failed to begin cancellation rejection: {error}"))?;
    transaction
        .execute(
            "UPDATE automation_cancellations
             SET state = 'rejected',
                 reconciled_at = ?2,
                 result_json = ?3
             WHERE adoption_key = ?1 AND state = 'requested'",
            params![
                reservation.request.adoption_key,
                iso(now),
                serde_json::to_string(&json!({"error": error}))
                    .map_err(|error| format!("failed to serialize cancellation error: {error}"))?
            ],
        )
        .map_err(|error| format!("failed to finalize cancellation rejection: {error}"))?;
    insert_adoption(
        &transaction,
        reservation,
        "rejected",
        &json!({"outcome": "rejected", "error": error}),
        now,
    )?;
    transaction
        .commit()
        .map_err(|error| format!("failed to commit cancellation rejection: {error}"))
}

fn persist_rejection(
    conn: &Connection,
    body: &Value,
    error: &ErrorEnvelope,
    now: DateTime<Utc>,
) -> Result<(), String> {
    let request: CancellationRequest = serde_json::from_value(body.clone())
        .map_err(|parse_error| format!("failed to parse rejected cancellation: {parse_error}"))?;
    let digest = sha256_hex(
        &canonicalize(body)
            .map_err(|error| format!("failed to canonicalize rejected cancellation: {error:#}"))?,
    );
    let automation_id = conn
        .query_row(
            "SELECT automation_id FROM automation_runs WHERE id = ?1",
            [&request.run_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|query_error| format!("failed to resolve rejected cancellation: {query_error}"))?;
    let reservation = ReservedCancellation {
        request,
        digest,
        automation_id: automation_id.unwrap_or_default(),
        requested_at: iso(now),
    };
    insert_adoption(
        conn,
        &reservation,
        "rejected",
        &json!({"outcome": "rejected", "error": error}),
        now,
    )
}

fn insert_adoption(
    conn: &Connection,
    reservation: &ReservedCancellation,
    outcome: &str,
    response: &Value,
    now: DateTime<Utc>,
) -> Result<(), String> {
    conn.execute(
        "INSERT INTO automation_command_adoptions (
            adoption_key, command, automation_id, request_digest, outcome,
            revision, response_json, adopted_at
         ) VALUES (?1, 'run.cancel.v1', ?2, ?3, ?4, NULL, ?5, ?6)",
        params![
            reservation.request.adoption_key,
            reservation.automation_id,
            reservation.digest,
            outcome,
            serde_json::to_string(response)
                .map_err(|error| format!("failed to serialize cancellation adoption: {error}"))?,
            iso(now)
        ],
    )
    .map_err(|error| format!("failed to persist cancellation adoption: {error}"))?;
    Ok(())
}

fn cancellation_payload(
    reservation: &ReservedCancellation,
    status: &str,
    acknowledged_at: Option<&str>,
    reconciled_at: Option<&str>,
) -> Value {
    let mut cancellation = json!({
        "scope": reservation.request.scope,
        "reason": reservation.request.reason,
        "requestedBy": {
            "principalId": reservation.request.requested_by.principal_id
        },
        "requestedAt": reservation.requested_at,
    });
    if let Some(acknowledged_at) = acknowledged_at {
        cancellation["acknowledgedAt"] = json!(acknowledged_at);
    }
    if let Some(reconciled_at) = reconciled_at {
        cancellation["reconciledAt"] = json!(reconciled_at);
    }
    json!({
        "runId": reservation.request.run_id,
        "attemptId": reservation.request.attempt_id,
        "runtimeCorrelation": {
            "sessionId": reservation.request.runtime_correlation.session_id
        },
        "status": status,
        "cancellation": cancellation,
    })
}

fn illegal_transition_error(message: &str) -> ErrorEnvelope {
    typed_error(ErrorCode::IllegalTransition, message)
}

fn replay_mismatch_error() -> ErrorEnvelope {
    typed_error(
        ErrorCode::AdoptionReplayMismatch,
        "adoption key was already used for a different request",
    )
}

fn typed_error(code: ErrorCode, message: impl Into<String>) -> ErrorEnvelope {
    ErrorEnvelope::try_new(code, message, false)
        .expect("automation cancellation error messages satisfy protocol bounds")
}

fn iso(now: DateTime<Utc>) -> String {
    now.to_rfc3339_opts(SecondsFormat::Millis, true)
}
