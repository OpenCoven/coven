use chrono::{DateTime, SecondsFormat, Utc};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::Deserialize;
use serde_json::{json, Value};

use super::contract::authority::AUTHORITY_PROFILE;
use super::contract::canonical_json::{canonicalize, sha256_hex};
use super::contract::error::{ErrorCode, ErrorEnvelope};
use super::contract::types::AdoptionKey;
use crate::api::SessionRuntime;

#[cfg(test)]
type StopFenceClockHook = Box<dyn FnOnce() -> DateTime<Utc>>;

#[cfg(test)]
thread_local! {
    static BEFORE_STOP_FENCE_TEST_HOOK:
        std::cell::RefCell<Option<Box<dyn FnOnce()>>> = std::cell::RefCell::new(None);
    static AFTER_STOP_FENCE_TEST_HOOK:
        std::cell::RefCell<Option<Box<dyn FnOnce()>>> = std::cell::RefCell::new(None);
    static STOP_FENCE_CLOCK_TEST_HOOK:
        std::cell::RefCell<Option<StopFenceClockHook>> = std::cell::RefCell::new(None);
}

#[cfg(test)]
pub(crate) fn set_before_stop_fence_test_hook(hook: Option<Box<dyn FnOnce()>>) {
    BEFORE_STOP_FENCE_TEST_HOOK.with(|cell| *cell.borrow_mut() = hook);
}

#[cfg(test)]
pub(crate) fn set_after_stop_fence_test_hook(hook: Option<Box<dyn FnOnce()>>) {
    AFTER_STOP_FENCE_TEST_HOOK.with(|cell| *cell.borrow_mut() = hook);
}

#[cfg(test)]
pub(crate) fn set_stop_fence_clock_test_hook(hook: Option<StopFenceClockHook>) {
    STOP_FENCE_CLOCK_TEST_HOOK.with(|cell| *cell.borrow_mut() = hook);
}

#[cfg(test)]
fn before_stop_fence_test_hook() {
    let hook = BEFORE_STOP_FENCE_TEST_HOOK.with(|cell| cell.borrow_mut().take());
    if let Some(hook) = hook {
        hook();
    }
}

#[cfg(not(test))]
fn before_stop_fence_test_hook() {}

#[cfg(test)]
fn after_stop_fence_test_hook() {
    let hook = AFTER_STOP_FENCE_TEST_HOOK.with(|cell| cell.borrow_mut().take());
    if let Some(hook) = hook {
        hook();
    }
}

#[cfg(not(test))]
fn after_stop_fence_test_hook() {}

#[cfg(test)]
fn stop_fence_clock() -> DateTime<Utc> {
    STOP_FENCE_CLOCK_TEST_HOOK
        .with(|cell| cell.borrow_mut().take())
        .map(|hook| hook())
        .unwrap_or_else(Utc::now)
}

#[cfg(not(test))]
fn stop_fence_clock() -> DateTime<Utc> {
    Utc::now()
}

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
            state IN ('requested', 'stopping', 'cancelled', 'recovery_required', 'rejected')
        ),
        requested_at TEXT NOT NULL,
        execution_expires_at TEXT NOT NULL,
        acknowledged_at TEXT,
        reconciled_at TEXT,
        result_json TEXT
    );
    CREATE TABLE IF NOT EXISTS automation_stop_fences (
        run_id TEXT PRIMARY KEY NOT NULL,
        session_id TEXT NOT NULL,
        owner TEXT NOT NULL CHECK (owner IN ('cancellation', 'timeout', 'recovery')),
        operation_key TEXT,
        acquired_at TEXT NOT NULL,
        execution_expires_at TEXT NOT NULL
    );
";

pub(crate) fn ensure_cancellation_schema(conn: &Connection) -> anyhow::Result<()> {
    conn.execute_batch("SAVEPOINT automation_cancellation_schema")?;
    let result = ensure_cancellation_schema_inner(conn)
        .and_then(|()| reconcile_cancellation_adoption_ledger(conn));
    match result {
        Ok(()) => {
            conn.execute_batch("RELEASE SAVEPOINT automation_cancellation_schema")?;
            Ok(())
        }
        Err(error) => {
            let _ = conn.execute_batch(
                "ROLLBACK TO SAVEPOINT automation_cancellation_schema;
                 RELEASE SAVEPOINT automation_cancellation_schema;",
            );
            Err(error)
        }
    }
}

fn ensure_cancellation_schema_inner(conn: &Connection) -> anyhow::Result<()> {
    let legacy_exists = conn
        .query_row(
            "SELECT 1 FROM sqlite_master
             WHERE type = 'table' AND name = 'automation_cancellations_legacy'",
            [],
            |_| Ok(()),
        )
        .optional()?
        .is_some();
    if legacy_exists {
        conn.execute_batch(AUTOMATION_CANCELLATIONS_SCHEMA_SQL)?;
        conn.execute(
            "INSERT INTO automation_cancellations (
                adoption_key, request_digest, automation_id, run_id, attempt_id,
                session_id, scope, requested_by_json, reason, state, requested_at,
                execution_expires_at, acknowledged_at, reconciled_at, result_json
             )
             SELECT adoption_key, request_digest, automation_id, run_id, attempt_id,
                    session_id, scope, requested_by_json, reason,
                    CASE state WHEN 'requested' THEN 'stopping' ELSE state END,
                    requested_at, requested_at, acknowledged_at, reconciled_at, result_json
             FROM automation_cancellations_legacy",
            [],
        )?;
        conn.execute_batch("DROP TABLE automation_cancellations_legacy;")?;
        return Ok(());
    }

    let table_exists = conn
        .query_row(
            "SELECT 1 FROM sqlite_master
             WHERE type = 'table' AND name = 'automation_cancellations'",
            [],
            |_| Ok(()),
        )
        .optional()?
        .is_some();
    if !table_exists {
        conn.execute_batch(AUTOMATION_CANCELLATIONS_SCHEMA_SQL)?;
        return Ok(());
    }

    let has_execution_lease = {
        let mut statement = conn.prepare("PRAGMA table_info(automation_cancellations)")?;
        let columns = statement.query_map([], |row| row.get::<_, String>(1))?;
        let mut found = false;
        for column in columns {
            if column? == "execution_expires_at" {
                found = true;
                break;
            }
        }
        found
    };
    if has_execution_lease {
        conn.execute_batch(AUTOMATION_CANCELLATIONS_SCHEMA_SQL)?;
        return Ok(());
    }

    conn.execute_batch(
        "ALTER TABLE automation_cancellations
             RENAME TO automation_cancellations_legacy;",
    )?;
    ensure_cancellation_schema_inner(conn)
}

fn reconcile_cancellation_adoption_ledger(conn: &Connection) -> anyhow::Result<()> {
    type CancellationLedgerRow = (
        String,
        String,
        String,
        String,
        String,
        Option<String>,
        Option<String>,
        Option<String>,
    );
    let rows: Vec<CancellationLedgerRow> = {
        let mut statement = conn.prepare(
            "SELECT adoption_key, request_digest, state, automation_id,
                    requested_at, acknowledged_at, reconciled_at, result_json
             FROM automation_cancellations
             ORDER BY adoption_key",
        )?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        rows
    };

    for (
        adoption_key,
        request_digest,
        state,
        automation_id,
        requested_at,
        acknowledged_at,
        reconciled_at,
        result_json,
    ) in rows
    {
        let reservation = conn
            .query_row(
                "SELECT command, request_digest
                 FROM automation_command_reservations
                 WHERE adoption_key = ?1",
                [&adoption_key],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?;
        let adoption = conn
            .query_row(
                "SELECT command, request_digest, outcome, response_json
                 FROM automation_command_adoptions
                 WHERE adoption_key = ?1",
                [&adoption_key],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                },
            )
            .optional()?;

        if matches!(state.as_str(), "requested" | "stopping") {
            anyhow::ensure!(
                adoption.is_none(),
                "active cancellation adoption key `{adoption_key}` already has a terminal command adoption"
            );
            match reservation {
                Some((command, digest))
                    if command == "run.cancel.v1" && digest == request_digest => {}
                Some(_) => anyhow::bail!(
                    "active cancellation adoption key `{adoption_key}` conflicts with another command reservation"
                ),
                None => {
                    conn.execute(
                        "INSERT INTO automation_command_reservations (
                             adoption_key, request_digest, command, reserved_at
                         ) VALUES (?1, ?2, 'run.cancel.v1', ?3)",
                        params![adoption_key, request_digest, requested_at],
                    )?;
                }
            }
            continue;
        }

        anyhow::ensure!(
            reservation.is_none(),
            "terminal cancellation adoption key `{adoption_key}` still has a command reservation"
        );
        let result: Value = serde_json::from_str(result_json.as_deref().ok_or_else(|| {
            anyhow::anyhow!(
                "terminal cancellation adoption key `{adoption_key}` has no durable result"
            )
        })?)
        .map_err(|error| {
            anyhow::anyhow!(
                "terminal cancellation adoption key `{adoption_key}` has an invalid durable result: {error}"
            )
        })?;
        let (outcome, response) = if state == "rejected" {
            let error = result.get("error").cloned().ok_or_else(|| {
                anyhow::anyhow!(
                    "rejected cancellation adoption key `{adoption_key}` has no durable error"
                )
            })?;
            ("rejected", json!({"outcome": "rejected", "error": error}))
        } else {
            anyhow::ensure!(
                matches!(state.as_str(), "cancelled" | "recovery_required"),
                "cancellation adoption key `{adoption_key}` has unsupported terminal state `{state}`"
            );
            (
                "committed",
                json!({"outcome": "committed", "result": result}),
            )
        };
        let response_json = serde_json::to_string(&response)?;
        match adoption {
            Some((command, digest, stored_outcome, stored_response)) => {
                let stored_response: Value =
                    serde_json::from_str(&stored_response).map_err(|error| {
                        anyhow::anyhow!(
                            "cancellation adoption key `{adoption_key}` has an invalid stored response: {error}"
                        )
                    })?;
                anyhow::ensure!(
                    command == "run.cancel.v1"
                        && digest == request_digest
                        && stored_outcome == outcome
                        && stored_response == response,
                    "terminal cancellation adoption key `{adoption_key}` conflicts with another command adoption"
                );
            }
            None => {
                let adopted_at = reconciled_at
                    .as_deref()
                    .or(acknowledged_at.as_deref())
                    .unwrap_or(&requested_at);
                conn.execute(
                    "INSERT INTO automation_command_adoptions (
                         adoption_key, request_digest, command, automation_id,
                         outcome, revision, response_json, adopted_at
                     ) VALUES (?1, ?2, 'run.cancel.v1', ?3, ?4, NULL, ?5, ?6)",
                    params![
                        adoption_key,
                        request_digest,
                        automation_id,
                        outcome,
                        response_json,
                        adopted_at
                    ],
                )?;
            }
        }
    }
    Ok(())
}

#[derive(Debug, Clone)]
pub struct CancellationSuccess {
    pub payload: Value,
    pub replayed: bool,
}

struct CancellationLifecycle {
    run_state: String,
    attempt_state: String,
    occurrence_state: String,
    session_state: String,
    settled_at: Option<String>,
    authority_profile: Option<String>,
}

fn lifecycle_requires_recovery(lifecycle: &CancellationLifecycle) -> bool {
    if lifecycle.run_state != "running" || lifecycle.occurrence_state != "recovery_required" {
        return false;
    }
    let terminal_session = matches!(
        lifecycle.session_state.as_str(),
        "completed" | "failed" | "cancelled" | "killed" | "idle"
    );
    if terminal_session {
        lifecycle.authority_profile.as_deref() == Some(AUTHORITY_PROFILE)
            && matches!(
                lifecycle.attempt_state.as_str(),
                "dispatching" | "started" | "observing" | "ambiguous"
            )
    } else {
        lifecycle.attempt_state == "ambiguous"
    }
}

fn cancellation_lifecycle(
    conn: &Connection,
    reservation: &ReservedCancellation,
) -> Result<Option<CancellationLifecycle>, String> {
    conn.query_row(
        "SELECT r.status, a.state, o.state, s.status, a.settled_at,
                r.authority_profile
         FROM automation_runs AS r
         JOIN automation_attempts AS a ON a.run_id = r.id
         JOIN automation_occurrences AS o ON o.id = r.occurrence_id
         JOIN sessions AS s ON s.id = a.session_id
         WHERE r.id = ?1 AND a.id = ?2 AND a.session_id = ?3",
        params![
            reservation.request.run_id,
            reservation.request.attempt_id,
            reservation.request.runtime_correlation.session_id
        ],
        |row| {
            Ok(CancellationLifecycle {
                run_state: row.get(0)?,
                attempt_state: row.get(1)?,
                occurrence_state: row.get(2)?,
                session_state: row.get(3)?,
                settled_at: row.get(4)?,
                authority_profile: row.get(5)?,
            })
        },
    )
    .optional()
    .map_err(|error| format!("failed to reconcile reserved cancellation: {error}"))
}

fn recover_settled_result(
    conn: &Connection,
    reservation: &ReservedCancellation,
    now: DateTime<Utc>,
    unknown_stop_outcome: bool,
) -> Result<Option<CancellationExecution>, String> {
    let Some(mut lifecycle) = cancellation_lifecycle(conn, reservation)? else {
        return Ok(None);
    };
    if lifecycle_requires_recovery(&lifecycle) {
        let payload = cancellation_payload(reservation, "recovery_required", None, None);
        finalize_success(conn, reservation, "recovery_required", &payload, now)?;
        return Ok(Some(CancellationExecution::Success(CancellationSuccess {
            payload,
            replayed: true,
        })));
    }
    let terminal_session = matches!(
        lifecycle.session_state.as_str(),
        "completed" | "failed" | "cancelled" | "killed" | "idle"
    );
    if lifecycle.run_state == "running" && terminal_session {
        let cancellation_owns_stop = conn
            .query_row(
                "SELECT 1 FROM automation_stop_fences
                 WHERE run_id = ?1
                   AND session_id = ?2
                   AND owner = 'cancellation'
                   AND operation_key = ?3",
                params![
                    reservation.request.run_id,
                    reservation.request.runtime_correlation.session_id,
                    reservation.request.adoption_key
                ],
                |_| Ok(()),
            )
            .optional()
            .map_err(|error| format!("failed to inspect cancellation stop ownership: {error}"))?
            .is_some();
        if unknown_stop_outcome
            && cancellation_owns_stop
            && lifecycle.occurrence_state != "recovery_required"
        {
            super::runner::mark_terminal_stop_for_recovery(
                conn,
                &reservation.request.run_id,
                &reservation.request.runtime_correlation.session_id,
                "cancellation stop outcome was not durably recorded",
                now,
            )?;
            let payload = cancellation_payload(reservation, "recovery_required", None, None);
            finalize_success(conn, reservation, "recovery_required", &payload, now)?;
            return Ok(Some(CancellationExecution::Success(CancellationSuccess {
                payload,
                replayed: true,
            })));
        }
        super::runner::settle_finished_runs(conn, now)?;
        let Some(reconciled) = cancellation_lifecycle(conn, reservation)? else {
            return Ok(None);
        };
        lifecycle = reconciled;
        if lifecycle_requires_recovery(&lifecycle) {
            let payload = cancellation_payload(reservation, "recovery_required", None, None);
            finalize_success(conn, reservation, "recovery_required", &payload, now)?;
            return Ok(Some(CancellationExecution::Success(CancellationSuccess {
                payload,
                replayed: true,
            })));
        }
    }
    let CancellationLifecycle {
        run_state,
        attempt_state,
        occurrence_state,
        session_state,
        settled_at,
        authority_profile: _,
    } = lifecycle;
    let terminal_session = matches!(
        session_state.as_str(),
        "completed" | "failed" | "cancelled" | "killed" | "idle"
    );
    if run_state == "cancelled"
        && attempt_state == "cancelled"
        && occurrence_state == "cancelled"
        && session_state == "cancelled"
    {
        let settled_at = settled_at.unwrap_or_else(|| iso(now));
        let payload = cancellation_payload(
            reservation,
            "cancelled",
            Some(&settled_at),
            Some(&settled_at),
        );
        finalize_success(conn, reservation, "cancelled", &payload, now)?;
        return Ok(Some(CancellationExecution::Success(CancellationSuccess {
            payload,
            replayed: true,
        })));
    }
    if matches!(
        run_state.as_str(),
        "succeeded" | "failed" | "timed_out" | "ambiguous"
    ) && matches!(
        attempt_state.as_str(),
        "succeeded" | "failed" | "timed_out" | "ambiguous"
    ) && matches!(
        occurrence_state.as_str(),
        "succeeded" | "failed" | "timed_out" | "recovery_required"
    ) && terminal_session
    {
        let error = typed_error(
            ErrorCode::IllegalTransition,
            "automation completion already won the cancellation race",
        );
        if !finalize_rejection(conn, reservation, &error, now)? {
            if let Some(replay) =
                load_adopted_response(conn, &reservation.request.adoption_key, &reservation.digest)?
            {
                return Ok(Some(replay));
            }
            return Ok(Some(CancellationExecution::Rejected(typed_error(
                ErrorCode::CancelPending,
                "the cancellation request is already being reconciled",
            ))));
        }
        return Ok(Some(CancellationExecution::Rejected(error)));
    }
    Ok(None)
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
    execution_expires_at: String,
    owns_execution: bool,
    recover_unknown_stop: bool,
}

enum CancellationReservation {
    Reserved(ReservedCancellation),
    DeadlineExpired {
        run_id: String,
        reservation: Option<ReservedCancellation>,
    },
}

pub fn execute_run_cancellation(
    conn: &Connection,
    runtime: &dyn SessionRuntime,
    body: Value,
    now: DateTime<Utc>,
) -> Result<CancellationExecution, String> {
    let digest = sha256_hex(
        &canonicalize(&body)
            .map_err(|error| format!("failed to canonicalize cancellation request: {error:#}"))?,
    );
    let adoption_key = valid_adoption_key(&body);
    if let Some(adoption_key) = adoption_key.as_deref() {
        if let Some(replay) = load_adopted_response(conn, adoption_key, &digest)? {
            return Ok(replay);
        }
    }
    let request: CancellationRequest = match serde_json::from_value(body.clone()) {
        Ok(request) => request,
        Err(error) => {
            let error = typed_error(
                ErrorCode::ValidationFailed,
                format!("invalid run cancellation request shape: {error}"),
            );
            if let Some(conflict) =
                persist_rejection(conn, &body, &digest, adoption_key.as_deref(), &error, now)?
            {
                return Ok(CancellationExecution::Rejected(conflict));
            }
            return Ok(CancellationExecution::Rejected(error));
        }
    };
    if let Err(error) = validate_request(&request) {
        let error = typed_error(ErrorCode::ValidationFailed, error);
        if let Some(conflict) =
            persist_rejection(conn, &body, &digest, adoption_key.as_deref(), &error, now)?
        {
            return Ok(CancellationExecution::Rejected(conflict));
        }
        return Ok(CancellationExecution::Rejected(error));
    }

    let reservation = match reserve_cancellation(conn, request, digest.clone(), now)? {
        Ok(CancellationReservation::Reserved(reservation)) => reservation,
        Ok(CancellationReservation::DeadlineExpired {
            run_id,
            reservation,
        }) => {
            let error = illegal_transition_error(
                "the targeted automation run reached its timeout before cancellation",
            );
            if let Some(reservation) = reservation.as_ref() {
                if !finalize_rejection(conn, reservation, &error, now)? {
                    if let Some(replay) =
                        load_adopted_response(conn, &reservation.request.adoption_key, &digest)?
                    {
                        return Ok(replay);
                    }
                    return Ok(CancellationExecution::Rejected(typed_error(
                        ErrorCode::CancelPending,
                        "the cancellation request is already being reconciled",
                    )));
                }
            } else if let Some(conflict) =
                persist_rejection(conn, &body, &digest, adoption_key.as_deref(), &error, now)?
            {
                return Ok(CancellationExecution::Rejected(conflict));
            }
            super::runner::enforce_run_timeout(conn, runtime, &run_id, now)?;
            return Ok(CancellationExecution::Rejected(error));
        }
        Err(error) => {
            if error.code() != ErrorCode::AdoptionReplayMismatch {
                if let Some(conflict) =
                    persist_rejection(conn, &body, &digest, adoption_key.as_deref(), &error, now)?
                {
                    return Ok(CancellationExecution::Rejected(conflict));
                }
            }
            return Ok(CancellationExecution::Rejected(error));
        }
    };
    execute_reserved_cancellation(conn, runtime, reservation, now, false)
}

fn execute_reserved_cancellation(
    conn: &Connection,
    runtime: &dyn SessionRuntime,
    reservation: ReservedCancellation,
    now: DateTime<Utc>,
    replayed: bool,
) -> Result<CancellationExecution, String> {
    if let Some(replay) =
        load_adopted_response(conn, &reservation.request.adoption_key, &reservation.digest)?
    {
        return Ok(replay);
    }
    if !reservation.owns_execution && !reservation.recover_unknown_stop {
        return Ok(CancellationExecution::Rejected(typed_error(
            ErrorCode::CancelPending,
            "the cancellation request is already being reconciled",
        )));
    }
    if let Some(execution) =
        recover_settled_result(conn, &reservation, now, reservation.recover_unknown_stop)?
    {
        return Ok(execution);
    }
    if reservation.recover_unknown_stop {
        if let Err(mark_error) = super::runner::mark_unconfirmed_stop_for_recovery(
            conn,
            &reservation.request.run_id,
            "cancellation stop outcome was not durably recorded",
            now,
        ) {
            if let Some(execution) = recover_settled_result(conn, &reservation, now, true)? {
                return Ok(execution);
            }
            return Err(mark_error);
        }
        let payload = cancellation_payload(&reservation, "recovery_required", None, None);
        finalize_success(conn, &reservation, "recovery_required", &payload, now)?;
        return Ok(CancellationExecution::Success(CancellationSuccess {
            payload,
            replayed: true,
        }));
    }
    before_stop_fence_test_hook();
    let claim_now = stop_fence_clock();
    if reserved_deadline_expired(conn, &reservation, claim_now)? {
        let error = illegal_transition_error(
            "the targeted automation run reached its timeout before cancellation",
        );
        if !finalize_rejection(conn, &reservation, &error, claim_now)? {
            if let Some(replay) =
                load_adopted_response(conn, &reservation.request.adoption_key, &reservation.digest)?
            {
                return Ok(replay);
            }
            return Ok(CancellationExecution::Rejected(typed_error(
                ErrorCode::CancelPending,
                "the cancellation request is already being reconciled",
            )));
        }
        super::runner::enforce_run_timeout(conn, runtime, &reservation.request.run_id, claim_now)?;
        return Ok(CancellationExecution::Rejected(error));
    }
    let stop_claim = super::runner::claim_stop_fence(
        conn,
        &reservation.request.run_id,
        &reservation.request.runtime_correlation.session_id,
        "cancellation",
        Some(&reservation.request.adoption_key),
        Some(&reservation.execution_expires_at),
        claim_now,
    )?;
    match stop_claim {
        super::runner::StopFenceClaim::Acquired => after_stop_fence_test_hook(),
        super::runner::StopFenceClaim::InProgress => {}
        super::runner::StopFenceClaim::UnknownOutcome => {
            super::runner::mark_unconfirmed_stop_for_recovery(
                conn,
                &reservation.request.run_id,
                "cancellation stop outcome was not durably recorded",
                now,
            )?;
            let payload = cancellation_payload(&reservation, "recovery_required", None, None);
            finalize_success(conn, &reservation, "recovery_required", &payload, now)?;
            return Ok(CancellationExecution::Success(CancellationSuccess {
                payload,
                replayed: true,
            }));
        }
        super::runner::StopFenceClaim::Conflict => {
            if let Some(replay) =
                load_adopted_response(conn, &reservation.request.adoption_key, &reservation.digest)?
            {
                return Ok(replay);
            }
            if let Some(execution) = recover_settled_result(conn, &reservation, now, false)? {
                return Ok(execution);
            }
            let error = typed_error(
                ErrorCode::CancelPending,
                "another stop operation already owns the automation run",
            );
            return Ok(CancellationExecution::Rejected(error));
        }
    }

    match runtime.kill_session(&reservation.request.runtime_correlation.session_id) {
        Ok(()) => {
            let settlement = super::runner::settle_confirmed_stop(
                conn,
                &reservation.request.run_id,
                &reservation.request.runtime_correlation.session_id,
                super::runner::ConfirmedStop::Cancelled,
                now,
            )?;
            match settlement {
                super::runner::ConfirmedStopSettlement::LostRace => {
                    let error = typed_error(
                        ErrorCode::IllegalTransition,
                        "automation completion already won the cancellation race",
                    );
                    if !finalize_rejection(conn, &reservation, &error, now)? {
                        if let Some(replay) = load_adopted_response(
                            conn,
                            &reservation.request.adoption_key,
                            &reservation.digest,
                        )? {
                            return Ok(replay);
                        }
                        return Ok(CancellationExecution::Rejected(typed_error(
                            ErrorCode::CancelPending,
                            "the cancellation request is already being reconciled",
                        )));
                    }
                    Ok(CancellationExecution::Rejected(error))
                }
                super::runner::ConfirmedStopSettlement::RecoveryRequired => {
                    let payload =
                        cancellation_payload(&reservation, "recovery_required", None, None);
                    finalize_success(conn, &reservation, "recovery_required", &payload, now)?;
                    Ok(CancellationExecution::Success(CancellationSuccess {
                        payload,
                        replayed,
                    }))
                }
                super::runner::ConfirmedStopSettlement::Settled => {
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
                        replayed,
                    }))
                }
            }
        }
        Err(_) => {
            if let Err(mark_error) = super::runner::mark_unconfirmed_stop_for_recovery(
                conn,
                &reservation.request.run_id,
                "cancellation stop was not confirmed",
                now,
            ) {
                if let Some(execution) = recover_settled_result(conn, &reservation, now, false)? {
                    return Ok(execution);
                }
                return Err(mark_error);
            }
            let payload = cancellation_payload(&reservation, "recovery_required", None, None);
            finalize_success(conn, &reservation, "recovery_required", &payload, now)?;
            Ok(CancellationExecution::Success(CancellationSuccess {
                payload,
                replayed,
            }))
        }
    }
}

pub(crate) fn reconcile_expired_cancellations(
    conn: &Connection,
    runtime: &dyn SessionRuntime,
    now: DateTime<Utc>,
) -> Result<usize, String> {
    let now_iso = iso(now);
    let candidates = {
        let mut statement = conn
            .prepare(
                "SELECT adoption_key, request_digest, automation_id, run_id, attempt_id,
                        session_id, scope, requested_by_json, reason, requested_at,
                        execution_expires_at, state
                 FROM automation_cancellations
                 WHERE state IN ('requested', 'stopping')
                   AND execution_expires_at <= ?1",
            )
            .map_err(|error| format!("failed to prepare cancellation reconciliation: {error}"))?;
        let rows = statement
            .query_map([&now_iso], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, Option<String>>(8)?,
                    row.get::<_, String>(9)?,
                    row.get::<_, String>(10)?,
                    row.get::<_, String>(11)?,
                ))
            })
            .map_err(|error| format!("failed to query cancellation reconciliation: {error}"))?;
        let mut candidates = Vec::new();
        for row in rows {
            candidates.push(
                row.map_err(|error| format!("failed to read cancellation recovery row: {error}"))?,
            );
        }
        candidates
    };

    let mut reconciled = 0;
    for (
        adoption_key,
        digest,
        automation_id,
        run_id,
        attempt_id,
        session_id,
        scope,
        requested_by_json,
        reason,
        requested_at,
        execution_expires_at,
        state,
    ) in candidates
    {
        let next_expiry = iso(now + chrono::Duration::seconds(30));
        let transaction =
            rusqlite::Transaction::new_unchecked(conn, TransactionBehavior::Immediate).map_err(
                |error| format!("failed to begin cancellation reconciliation claim: {error}"),
            )?;
        let timeout_at = transaction
            .query_row(
                "SELECT timeout_at FROM automation_runs WHERE id = ?1",
                [&run_id],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()
            .map_err(|error| {
                format!("failed to inspect cancellation reconciliation deadline: {error}")
            })?
            .flatten();
        let claimed = transaction
            .execute(
                "UPDATE automation_cancellations
                 SET execution_expires_at = ?2
                 WHERE adoption_key = ?1
                   AND state = ?3
                   AND execution_expires_at = ?4",
                params![adoption_key, next_expiry, state, execution_expires_at],
            )
            .map_err(|error| format!("failed to claim cancellation reconciliation: {error}"))?;
        transaction.commit().map_err(|error| {
            format!("failed to commit cancellation reconciliation claim: {error}")
        })?;
        if claimed != 1 {
            continue;
        }
        let requested_by: RequestedBy = serde_json::from_str(&requested_by_json)
            .map_err(|error| format!("stored cancellation requester is invalid: {error}"))?;
        let reservation = ReservedCancellation {
            request: CancellationRequest {
                action: "coven.automations.run.cancel.v1".to_string(),
                adoption_key,
                run_id,
                attempt_id,
                runtime_correlation: RuntimeCorrelation { session_id },
                scope,
                reason: reason.unwrap_or_else(|| "automation cancellation requested".to_string()),
                requested_by,
            },
            digest,
            automation_id,
            requested_at,
            execution_expires_at: next_expiry,
            owns_execution: state == "requested",
            recover_unknown_stop: state == "stopping",
        };
        let deadline_expired = timeout_at
            .map(|timeout_at| {
                DateTime::parse_from_rfc3339(&timeout_at)
                    .map(|timeout_at| timeout_at.with_timezone(&Utc) <= now)
                    .map_err(|error| {
                        format!(
                            "stored cancellation target timeout is invalid during reconciliation: {error}"
                        )
                    })
            })
            .transpose()?
            .unwrap_or(false);
        if state == "requested" && deadline_expired {
            let error = illegal_transition_error(
                "the targeted automation run reached its timeout before cancellation",
            );
            if finalize_rejection(conn, &reservation, &error, now)? {
                super::runner::enforce_run_timeout(
                    conn,
                    runtime,
                    &reservation.request.run_id,
                    now,
                )?;
            }
            reconciled += 1;
            continue;
        }
        execute_reserved_cancellation(conn, runtime, reservation, now, true)?;
        reconciled += 1;
    }
    Ok(reconciled)
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

fn reserved_deadline_expired(
    conn: &Connection,
    reservation: &ReservedCancellation,
    now: DateTime<Utc>,
) -> Result<bool, String> {
    let timeout_at = conn
        .query_row(
            "SELECT r.timeout_at
             FROM automation_cancellations AS c
             JOIN automation_runs AS r ON r.id = c.run_id
             WHERE c.adoption_key = ?1
               AND c.run_id = ?2
               AND c.session_id = ?3
               AND c.state = 'requested'
               AND c.execution_expires_at = ?4",
            params![
                reservation.request.adoption_key,
                reservation.request.run_id,
                reservation.request.runtime_correlation.session_id,
                reservation.execution_expires_at
            ],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()
        .map_err(|error| format!("failed to inspect cancellation claim deadline: {error}"))?
        .flatten();
    timeout_at
        .map(|timeout_at| {
            DateTime::parse_from_rfc3339(&timeout_at)
                .map(|timeout_at| timeout_at.with_timezone(&Utc) <= now)
                .map_err(|error| format!("stored cancellation target timeout is invalid: {error}"))
        })
        .transpose()
        .map(|expired| expired.unwrap_or(false))
}

fn reserve_cancellation(
    conn: &Connection,
    request: CancellationRequest,
    digest: String,
    now: DateTime<Utc>,
) -> Result<Result<CancellationReservation, ErrorEnvelope>, String> {
    let transaction = rusqlite::Transaction::new_unchecked(conn, TransactionBehavior::Immediate)
        .map_err(|error| format!("failed to begin cancellation reservation: {error}"))?;
    let existing = transaction
        .query_row(
            "SELECT request_digest, automation_id, run_id, attempt_id, session_id,
                    requested_at, execution_expires_at, state
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
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                ))
            },
        )
        .optional()
        .map_err(|error| format!("failed to query cancellation reservation: {error}"))?;
    if let Some((
        existing_digest,
        automation_id,
        run_id,
        attempt_id,
        session_id,
        requested_at,
        execution_expires_at,
        state,
    )) = existing
    {
        if existing_digest != digest {
            return Ok(Err(replay_mismatch_error()));
        }
        if run_id != request.run_id
            || attempt_id != request.attempt_id
            || session_id != request.runtime_correlation.session_id
        {
            transaction
                .rollback()
                .map_err(|error| format!("failed to close cancellation mismatch: {error}"))?;
            return Ok(Err(replay_mismatch_error()));
        }
        if matches!(state.as_str(), "requested" | "stopping") {
            let global_reservation = transaction
                .query_row(
                    "SELECT command, request_digest
                     FROM automation_command_reservations
                     WHERE adoption_key = ?1",
                    [&request.adoption_key],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                )
                .optional()
                .map_err(|error| {
                    format!("failed to query global cancellation reservation: {error}")
                })?;
            if global_reservation.as_ref() != Some(&("run.cancel.v1".to_string(), digest.clone())) {
                transaction.rollback().map_err(|error| {
                    format!("failed to close cancellation reservation drift: {error}")
                })?;
                return Ok(Err(replay_mismatch_error()));
            }
        }
        let expires_at = DateTime::parse_from_rfc3339(&execution_expires_at)
            .map_err(|error| format!("stored cancellation lease is invalid: {error}"))?
            .with_timezone(&Utc);
        let next_expiry = iso(now + chrono::Duration::seconds(30));
        let (owns_execution, recover_unknown_stop, owned_expiry) =
            if matches!(state.as_str(), "requested" | "stopping") && expires_at <= now {
                let changed = transaction
                    .execute(
                        "UPDATE automation_cancellations
                 SET execution_expires_at = ?2
                 WHERE adoption_key = ?1
                   AND request_digest = ?3
                   AND state = ?5
                   AND execution_expires_at = ?4",
                        params![
                            request.adoption_key,
                            next_expiry,
                            digest,
                            execution_expires_at,
                            state
                        ],
                    )
                    .map_err(|error| {
                        format!("failed to reclaim cancellation execution: {error}")
                    })?;
                (
                    changed == 1 && state == "requested",
                    changed == 1 && state == "stopping",
                    if changed == 1 {
                        next_expiry.clone()
                    } else {
                        execution_expires_at.clone()
                    },
                )
            } else {
                (false, false, execution_expires_at.clone())
            };
        let reservation = ReservedCancellation {
            request,
            digest,
            automation_id,
            requested_at,
            execution_expires_at: owned_expiry,
            owns_execution,
            recover_unknown_stop,
        };
        if reservation.owns_execution {
            let timeout_at = transaction
                .query_row(
                    "SELECT timeout_at FROM automation_runs WHERE id = ?1",
                    [&reservation.request.run_id],
                    |row| row.get::<_, Option<String>>(0),
                )
                .optional()
                .map_err(|error| {
                    format!("failed to inspect reclaimed cancellation deadline: {error}")
                })?
                .flatten();
            let deadline_expired = timeout_at
                .map(|timeout_at| {
                    DateTime::parse_from_rfc3339(&timeout_at)
                        .map(|timeout_at| timeout_at.with_timezone(&Utc) <= now)
                        .map_err(|error| {
                            format!("stored cancellation target timeout is invalid: {error}")
                        })
                })
                .transpose()?
                .unwrap_or(false);
            if deadline_expired {
                let run_id = reservation.request.run_id.clone();
                transaction.commit().map_err(|error| {
                    format!("failed to commit expired cancellation reclaim: {error}")
                })?;
                return Ok(Ok(CancellationReservation::DeadlineExpired {
                    run_id,
                    reservation: Some(reservation),
                }));
            }
        }
        transaction.commit().map_err(|error| {
            format!("failed to commit cancellation replay reservation: {error}")
        })?;
        return Ok(Ok(CancellationReservation::Reserved(reservation)));
    }
    let conflicting_reservation = transaction
        .query_row(
            "SELECT command, request_digest
             FROM automation_command_reservations
             WHERE adoption_key = ?1",
            [&request.adoption_key],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()
        .map_err(|error| format!("failed to query global adoption reservation: {error}"))?;
    if conflicting_reservation.is_some() {
        transaction
            .rollback()
            .map_err(|error| format!("failed to close conflicting reservation: {error}"))?;
        return Ok(Err(replay_mismatch_error()));
    }
    if super::command_adoption::attempt_adoption_key_exists(&transaction, &request.adoption_key)
        .map_err(|error| format!("failed to inspect attempt adoption key: {error:#}"))?
    {
        transaction
            .rollback()
            .map_err(|error| format!("failed to close conflicting attempt key: {error}"))?;
        return Ok(Err(replay_mismatch_error()));
    }
    let conflicting_adoption = transaction
        .query_row(
            "SELECT command, request_digest
             FROM automation_command_adoptions
             WHERE adoption_key = ?1",
            [&request.adoption_key],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()
        .map_err(|error| format!("failed to query global adoption key: {error}"))?;
    if conflicting_adoption.is_some() {
        transaction
            .rollback()
            .map_err(|error| format!("failed to close conflicting adoption lookup: {error}"))?;
        return Ok(Err(replay_mismatch_error()));
    }
    let pending_adoption_key = transaction
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
        transaction
            .rollback()
            .map_err(|error| format!("failed to close pending cancellation lookup: {error}"))?;
        return Ok(Err(typed_error(
            ErrorCode::CancelPending,
            "the automation run already has a cancellation request",
        )));
    }

    let live = transaction
        .query_row(
            "SELECT r.automation_id, r.status, a.state, s.status, r.timeout_at
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
                    row.get::<_, Option<String>>(4)?,
                ))
            },
        )
        .optional()
        .map_err(|error| format!("failed to resolve cancellation target: {error}"))?;
    let Some((automation_id, run_state, attempt_state, session_state, timeout_at)) = live else {
        transaction
            .rollback()
            .map_err(|error| format!("failed to close stale cancellation lookup: {error}"))?;
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
        transaction
            .rollback()
            .map_err(|error| format!("failed to close terminal cancellation lookup: {error}"))?;
        return Ok(Err(illegal_transition_error(
            "the targeted automation attempt is no longer cancellable",
        )));
    }
    if let Some(timeout_at) = timeout_at {
        let timeout_at = DateTime::parse_from_rfc3339(&timeout_at)
            .map_err(|error| format!("stored cancellation target timeout is invalid: {error}"))?
            .with_timezone(&Utc);
        if timeout_at <= now {
            transaction.rollback().map_err(|error| {
                format!("failed to close expired cancellation reservation: {error}")
            })?;
            return Ok(Ok(CancellationReservation::DeadlineExpired {
                run_id: request.run_id,
                reservation: None,
            }));
        }
    }

    let requested_at = iso(now);
    let execution_expires_at = iso(now + chrono::Duration::seconds(30));
    transaction
        .execute(
            "INSERT INTO automation_command_reservations (
                adoption_key, request_digest, command, reserved_at
             ) VALUES (?1, ?2, 'run.cancel.v1', ?3)",
            params![request.adoption_key, digest, requested_at],
        )
        .map_err(|error| format!("failed to reserve global adoption key: {error}"))?;
    transaction
        .execute(
            "INSERT INTO automation_cancellations (
            adoption_key, request_digest, automation_id, run_id, attempt_id,
            session_id, scope, requested_by_json, reason, state, requested_at,
            execution_expires_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'run', ?7, ?8, 'requested', ?9, ?10)",
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
                execution_expires_at,
            ],
        )
        .map_err(|error| format!("failed to reserve automation cancellation: {error}"))?;
    transaction.commit().map_err(|error| {
        format!("failed to commit automation cancellation reservation: {error}")
    })?;

    Ok(Ok(CancellationReservation::Reserved(
        ReservedCancellation {
            request,
            digest,
            automation_id,
            requested_at,
            execution_expires_at,
            owns_execution: true,
            recover_unknown_stop: false,
        },
    )))
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
    let changed = transaction
        .execute(
            "UPDATE automation_cancellations
             SET state = ?2,
                 acknowledged_at = ?3,
                 reconciled_at = ?4,
                 result_json = ?5
             WHERE adoption_key = ?1 AND state IN ('requested', 'stopping')",
            params![
                reservation.request.adoption_key,
                state,
                acknowledged_at,
                reconciled_at,
                result_json
            ],
        )
        .map_err(|error| format!("failed to finalize automation cancellation: {error}"))?;
    if changed != 1 {
        return Err("cancellation was already finalized by another request".to_string());
    }
    transaction
        .execute(
            "DELETE FROM automation_command_reservations
             WHERE adoption_key = ?1
               AND command = 'run.cancel.v1'
               AND request_digest = ?2",
            params![reservation.request.adoption_key, reservation.digest],
        )
        .map_err(|error| format!("failed to release cancellation adoption reservation: {error}"))?;
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
) -> Result<bool, String> {
    let transaction = conn
        .unchecked_transaction()
        .map_err(|error| format!("failed to begin cancellation rejection: {error}"))?;
    let changed = transaction
        .execute(
            "UPDATE automation_cancellations
             SET state = 'rejected',
                 reconciled_at = ?2,
                 result_json = ?3
             WHERE adoption_key = ?1
               AND state IN ('requested', 'stopping')
               AND execution_expires_at = ?4",
            params![
                reservation.request.adoption_key,
                iso(now),
                serde_json::to_string(&json!({"error": error}))
                    .map_err(|error| format!("failed to serialize cancellation error: {error}"))?,
                reservation.execution_expires_at
            ],
        )
        .map_err(|error| format!("failed to finalize cancellation rejection: {error}"))?;
    if changed != 1 {
        transaction
            .rollback()
            .map_err(|error| format!("failed to close stale cancellation rejection: {error}"))?;
        return Ok(false);
    }
    transaction
        .execute(
            "DELETE FROM automation_command_reservations
             WHERE adoption_key = ?1
               AND command = 'run.cancel.v1'
               AND request_digest = ?2",
            params![reservation.request.adoption_key, reservation.digest],
        )
        .map_err(|error| format!("failed to release rejected cancellation reservation: {error}"))?;
    transaction
        .execute(
            "DELETE FROM automation_stop_fences
             WHERE run_id = ?1
               AND session_id = ?2
               AND owner = 'cancellation'
               AND operation_key = ?3",
            params![
                reservation.request.run_id,
                reservation.request.runtime_correlation.session_id,
                reservation.request.adoption_key
            ],
        )
        .map_err(|error| format!("failed to release rejected cancellation stop fence: {error}"))?;
    insert_adoption(
        &transaction,
        reservation,
        "rejected",
        &json!({"outcome": "rejected", "error": error}),
        now,
    )?;
    transaction
        .commit()
        .map_err(|error| format!("failed to commit cancellation rejection: {error}"))?;
    Ok(true)
}

fn persist_rejection(
    conn: &Connection,
    body: &Value,
    digest: &str,
    adoption_key: Option<&str>,
    error: &ErrorEnvelope,
    now: DateTime<Utc>,
) -> Result<Option<ErrorEnvelope>, String> {
    let Some(adoption_key) = adoption_key else {
        return Ok(None);
    };
    let run_id = body.get("runId").and_then(Value::as_str);
    let transaction = rusqlite::Transaction::new_unchecked(conn, TransactionBehavior::Immediate)
        .map_err(|error| format!("failed to begin cancellation rejection: {error}"))?;
    let existing_adoption = transaction
        .query_row(
            "SELECT command, request_digest
             FROM automation_command_adoptions
             WHERE adoption_key = ?1",
            [adoption_key],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()
        .map_err(|query_error| {
            format!("failed to inspect rejected cancellation adoption: {query_error}")
        })?;
    if let Some((command, existing_digest)) = existing_adoption {
        transaction.rollback().map_err(|rollback_error| {
            format!("failed to close rejected cancellation replay: {rollback_error}")
        })?;
        return Ok(
            (command != "run.cancel.v1" || existing_digest != digest).then(replay_mismatch_error)
        );
    }
    if super::command_adoption::attempt_adoption_key_exists(&transaction, adoption_key).map_err(
        |query_error| format!("failed to inspect rejected attempt adoption key: {query_error:#}"),
    )? {
        transaction.rollback().map_err(|rollback_error| {
            format!("failed to close rejected attempt adoption conflict: {rollback_error}")
        })?;
        return Ok(Some(replay_mismatch_error()));
    }
    let reserved = transaction
        .query_row(
            "SELECT 1 FROM automation_command_reservations WHERE adoption_key = ?1",
            [adoption_key],
            |_| Ok(()),
        )
        .optional()
        .map_err(|query_error| {
            format!("failed to inspect rejected cancellation reservation: {query_error}")
        })?
        .is_some();
    if reserved {
        transaction.rollback().map_err(|rollback_error| {
            format!("failed to close rejected cancellation reservation: {rollback_error}")
        })?;
        return Ok(Some(replay_mismatch_error()));
    }
    let automation_id = transaction
        .query_row(
            "SELECT automation_id FROM automation_runs WHERE id = ?1",
            [run_id.unwrap_or_default()],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|query_error| format!("failed to resolve rejected cancellation: {query_error}"))?;
    transaction
        .execute(
            "INSERT INTO automation_command_adoptions (
            adoption_key, command, automation_id, request_digest, outcome,
            revision, response_json, adopted_at
         ) VALUES (?1, 'run.cancel.v1', ?2, ?3, 'rejected', NULL, ?4, ?5)",
            params![
                adoption_key,
                automation_id,
                digest,
                serde_json::to_string(&json!({"outcome": "rejected", "error": error})).map_err(
                    |error| format!("failed to serialize cancellation rejection: {error}")
                )?,
                iso(now)
            ],
        )
        .map_err(|error| format!("failed to persist rejected cancellation adoption: {error}"))?;
    transaction
        .commit()
        .map_err(|error| format!("failed to commit rejected cancellation adoption: {error}"))?;
    Ok(None)
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

fn valid_adoption_key(body: &Value) -> Option<String> {
    let value = body.get("adoptionKey")?.as_str()?.to_owned();
    AdoptionKey::new(value.clone()).ok().map(|_| value)
}

fn iso(now: DateTime<Utc>) -> String {
    now.to_rfc3339_opts(SecondsFormat::Millis, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    const LEGACY_SCHEMA: &str = "
        CREATE TABLE automation_cancellations (
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

    #[test]
    fn migration_conservatively_fences_legacy_in_flight_cancellations() -> anyhow::Result<()> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch(
            super::super::command_adoption::AUTOMATION_COMMAND_ADOPTIONS_SCHEMA_SQL,
        )?;
        conn.execute_batch(LEGACY_SCHEMA)?;
        conn.execute(
            "INSERT INTO automation_cancellations (
                adoption_key, request_digest, automation_id, run_id, attempt_id,
                session_id, scope, requested_by_json, reason, state, requested_at
             ) VALUES (?1, 'digest', 'automation', 'run', 'attempt', 'session',
                       'run', '{\"principalId\":\"operator\"}', 'reason',
                       'requested', ?2)",
            params!["adopt:cancel:legacy:0001", "2026-01-01T00:00:00.000Z"],
        )?;

        ensure_cancellation_schema(&conn)?;

        let migrated: (String, String) = conn.query_row(
            "SELECT state, execution_expires_at
             FROM automation_cancellations
             WHERE adoption_key = 'adopt:cancel:legacy:0001'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        assert_eq!(migrated.0, "stopping");
        assert_eq!(migrated.1, "2026-01-01T00:00:00.000Z");
        let reservation: (String, String) = conn.query_row(
            "SELECT command, request_digest
             FROM automation_command_reservations
             WHERE adoption_key = 'adopt:cancel:legacy:0001'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        assert_eq!(reservation.0, "run.cancel.v1");
        assert_eq!(reservation.1, "digest");
        Ok(())
    }

    #[test]
    fn migration_backfills_terminal_legacy_cancellation_adoptions() -> anyhow::Result<()> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch(
            super::super::command_adoption::AUTOMATION_COMMAND_ADOPTIONS_SCHEMA_SQL,
        )?;
        conn.execute_batch(LEGACY_SCHEMA)?;
        conn.execute(
            "INSERT INTO automation_cancellations (
                adoption_key, request_digest, automation_id, run_id, attempt_id,
                session_id, scope, requested_by_json, reason, state, requested_at,
                acknowledged_at, reconciled_at, result_json
             ) VALUES (?1, 'digest', 'automation', 'run', 'attempt', 'session',
                       'run', '{\"principalId\":\"operator\"}', 'reason',
                       'cancelled', ?2, ?2, ?2, '{}')",
            params!["adopt:cancel:legacy:0002", "2026-01-01T00:00:00.000Z"],
        )?;

        ensure_cancellation_schema(&conn)?;

        let state: String = conn.query_row(
            "SELECT state FROM automation_cancellations
             WHERE adoption_key = 'adopt:cancel:legacy:0002'",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(state, "cancelled");
        let reservations: i64 = conn.query_row(
            "SELECT COUNT(*) FROM automation_command_reservations",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(reservations, 0);
        let adoption: (String, String, String) = conn.query_row(
            "SELECT command, request_digest, response_json
             FROM automation_command_adoptions
             WHERE adoption_key = 'adopt:cancel:legacy:0002'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;
        assert_eq!(adoption.0, "run.cancel.v1");
        assert_eq!(adoption.1, "digest");
        assert_eq!(
            serde_json::from_str::<Value>(&adoption.2)?,
            json!({"outcome": "committed", "result": {}})
        );
        Ok(())
    }

    #[test]
    fn migration_rejects_conflicting_cancellation_adoption_keys() -> anyhow::Result<()> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch(
            super::super::command_adoption::AUTOMATION_COMMAND_ADOPTIONS_SCHEMA_SQL,
        )?;
        conn.execute_batch(LEGACY_SCHEMA)?;
        conn.execute(
            "INSERT INTO automation_command_reservations (
                 adoption_key, request_digest, command, reserved_at
             ) VALUES (?1, 'other-digest', 'definition.create.v1', ?2)",
            params!["adopt:cancel:legacy:conflict", "2026-01-01T00:00:00.000Z"],
        )?;
        conn.execute(
            "INSERT INTO automation_cancellations (
                adoption_key, request_digest, automation_id, run_id, attempt_id,
                session_id, scope, requested_by_json, reason, state, requested_at
             ) VALUES (?1, 'digest', 'automation', 'run', 'attempt', 'session',
                       'run', '{\"principalId\":\"operator\"}', 'reason',
                       'requested', ?2)",
            params!["adopt:cancel:legacy:conflict", "2026-01-01T00:00:00.000Z"],
        )?;

        let error = ensure_cancellation_schema(&conn)
            .expect_err("migration must reject a command-owned cancellation adoption key");
        assert!(
            error
                .to_string()
                .contains("conflicts with another command reservation"),
            "{error:#}"
        );
        Ok(())
    }

    #[test]
    fn migration_resumes_after_a_legacy_table_rename() -> anyhow::Result<()> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch(
            super::super::command_adoption::AUTOMATION_COMMAND_ADOPTIONS_SCHEMA_SQL,
        )?;
        conn.execute_batch(LEGACY_SCHEMA)?;
        conn.execute(
            "INSERT INTO automation_cancellations (
                adoption_key, request_digest, automation_id, run_id, attempt_id,
                session_id, scope, requested_by_json, state, requested_at
             ) VALUES ('adopt:cancel:legacy:0003', 'digest', 'automation', 'run',
                       'attempt', 'session', 'run',
                       '{\"principalId\":\"operator\"}', 'requested',
                       '2026-01-01T00:00:00.000Z')",
            [],
        )?;
        conn.execute_batch(
            "ALTER TABLE automation_cancellations
             RENAME TO automation_cancellations_legacy;",
        )?;

        ensure_cancellation_schema(&conn)?;

        let state: String = conn.query_row(
            "SELECT state FROM automation_cancellations
             WHERE adoption_key = 'adopt:cancel:legacy:0003'",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(state, "stopping");
        let legacy_exists: bool = conn
            .query_row(
                "SELECT 1 FROM sqlite_master
                 WHERE type = 'table' AND name = 'automation_cancellations_legacy'",
                [],
                |_| Ok(true),
            )
            .optional()?
            .unwrap_or(false);
        assert!(!legacy_exists);
        Ok(())
    }
}
