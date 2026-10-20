//! Actual result-delivery deferral, rollback and failure isolation through the
//! production discharge path. No runtime launch, inbox turn or release claim.
use std::collections::BTreeSet;

use chrono::{DateTime, SecondsFormat, Utc};
use rusqlite::{ffi::SQLITE_CONSTRAINT_TRIGGER, Connection};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::{scheduler_conformance_tempdir, valid_case_id};
use crate::automations::definition::RoutineDefinition;
use crate::automations::delivery::{
    deliver_pending_results, DeliveryReport, GATE_UNAVAILABLE_REASON, NOTICE_SOURCE_KIND,
    RESULT_UNREADABLE_REASON,
};
use crate::automations::runs::{get_run, record_run_finish, record_run_start, RunFinish, RunStart};
use crate::main_session::INSTANCE_MAIN_SCOPE_KEY;
use crate::main_session_notices::{self, NewNotice, MAX_PENDING_NOTICES_PER_SCOPE};
use crate::main_session_routes::{ScopeGate, SCOPE_LOCK_DIR};

const INVALID: &str = "conformance vector is invalid";
const FAILED: &str = "conformance suite execution failed";
const RUN_ID: &str = "delivery-conformance-run";
const HEALTHY_RUN_ID: &str = "delivery-conformance-healthy-run";
const FIXTURE_FAULT: &str = "conformance delivery fault";
/// The abort the production delivery-immutability trigger raises (`runs.rs`).
const DELIVERY_IMMUTABLE: &str = "settled automation run delivery is immutable";
/// A fixed future instant. The production runner dispatches at the later of
/// its argument and the wall clock, so a future base keeps the claim lease
/// valid without making any case depend on the time it runs.
pub(super) const BASE: &str = "2099-10-20T09:00:00.000Z";
/// The runner launches only a routine with a working directory; a no-op
/// runtime never touches it.
const FIXTURE_CWD: &str = "/work/project";

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct VectorSet {
    schema_version: String,
    cases: Vec<Case>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Case {
    case_id: String,
    fault: Fault,
    definition: Value,
    expected: Expected,
}

#[derive(Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Expected {
    first_pass: FirstPass,
    failure_reason: Option<FailureReason>,
    final_state: DeliveryState,
    final_notices: u64,
    replay_is_noop: bool,
    settled_record_immutable: bool,
}

#[derive(Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord, Clone, Copy)]
#[serde(rename_all = "snake_case")]
enum Fault {
    None,
    GateHeld,
    InboxFull,
    DischargeRollback,
    GateUnavailable,
    UnreadableResult,
}

#[derive(Deserialize, Serialize, PartialEq, Clone, Copy)]
#[serde(rename_all = "snake_case")]
enum FirstPass {
    Delivered,
    Deferred,
    PassFailed,
    Failed,
}

#[derive(Deserialize, Serialize, PartialEq, Clone, Copy)]
#[serde(rename_all = "snake_case")]
enum FailureReason {
    GateUnavailable,
    ResultUnreadable,
}

#[derive(Debug, Deserialize, Serialize, PartialEq, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub(super) enum DeliveryState {
    Pending,
    Delivered,
    Failed,
}

/// What each fault must produce. A vector restates it, so it cannot weaken it.
fn expected_for(fault: Fault) -> Expected {
    let (first_pass, failure_reason, final_state, final_notices) = match fault {
        Fault::None => (FirstPass::Delivered, None, DeliveryState::Delivered, 1),
        Fault::GateHeld | Fault::InboxFull => {
            (FirstPass::Deferred, None, DeliveryState::Delivered, 1)
        }
        Fault::DischargeRollback => (FirstPass::PassFailed, None, DeliveryState::Delivered, 1),
        Fault::GateUnavailable => (
            FirstPass::Failed,
            Some(FailureReason::GateUnavailable),
            DeliveryState::Failed,
            0,
        ),
        Fault::UnreadableResult => (
            FirstPass::Failed,
            Some(FailureReason::ResultUnreadable),
            DeliveryState::Failed,
            0,
        ),
    };
    Expected {
        first_pass,
        failure_reason,
        final_state,
        final_notices,
        replay_is_noop: true,
        settled_record_immutable: true,
    }
}

pub(super) fn evaluate(vector: &Value) -> Result<(bool, Vec<Value>), &'static str> {
    let vectors: VectorSet = serde_json::from_value(vector.clone()).map_err(|_| INVALID)?;
    if vectors.schema_version != "coven.automations.result-delivery-recovery-vectors.v1"
        || vectors.cases.len() != 6
    {
        return Err(INVALID);
    }
    let mut ids = BTreeSet::new();
    let mut faults = BTreeSet::new();
    for case in &vectors.cases {
        let definition = RoutineDefinition::from_json(&case.definition).map_err(|_| INVALID)?;
        if !valid_case_id(&case.case_id)
            || !ids.insert(&case.case_id)
            || !faults.insert(case.fault)
            || case.definition["status"] != "PAUSED"
            || case.definition["outputTarget"] != json!({"kind": "main-session"})
            || case.definition.get("cwd").is_some()
            || definition.id.is_empty()
            || case.expected != expected_for(case.fault)
        {
            return Err(INVALID);
        }
    }
    let mut passed = true;
    let mut observations = Vec::new();
    for case in &vectors.cases {
        let (matches, observation) = case_matches(case)?;
        passed &= matches;
        observations.push(observation);
    }
    Ok((passed, observations))
}

pub(super) fn at(base: DateTime<Utc>, seconds: i64) -> DateTime<Utc> {
    base + chrono::Duration::seconds(seconds)
}

pub(super) fn iso(instant: DateTime<Utc>) -> String {
    instant.to_rfc3339_opts(SecondsFormat::Millis, true)
}

/// Starts and settles one run through the production run record, which writes
/// its `pending` delivery obligation in the same guarded update.
pub(super) fn settle_run(
    conn: &Connection,
    base: DateTime<Utc>,
    automation_id: &str,
    run_id: &str,
    started: i64,
) -> Result<(), &'static str> {
    record_run_start(
        conn,
        run_id,
        RunStart {
            automation_id,
            occurrence_id: None,
            authority_profile: None,
            session_id: None,
            familiar_id: None,
            runtime: "coven-code",
            timeout_at: at(base, started + 1800),
        },
        at(base, started),
    )
    .map_err(|_| FAILED)?;
    let finished = record_run_finish(
        conn,
        run_id,
        RunFinish {
            status: "succeeded",
            exit_code: None,
            session_id: None,
            log_json: None,
            output_commit: None,
        },
        at(base, started + 60),
    )
    .map_err(|_| FAILED)?;
    finished.then_some(()).ok_or(FAILED)
}

/// Launches one run through the production runner with a no-op runtime, gives
/// its session a malformed output event, and settles it, so composing its
/// result fails.
fn settle_unreadable_run(
    conn: &Connection,
    base: DateTime<Utc>,
    definition: &RoutineDefinition,
) -> Result<String, &'static str> {
    let outcome = crate::automations::runner::run_routine_now(
        conn,
        &crate::api::NoopSessionRuntime,
        definition,
        at(base, 0),
    )
    .map_err(|_| FAILED)?;
    let session_id = outcome.session_id.ok_or(FAILED)?;
    crate::store::insert_event(
        conn,
        &crate::store::EventRecord {
            seq: 0,
            id: "delivery-conformance-unreadable-output".to_owned(),
            session_id: session_id.clone(),
            kind: "output".to_owned(),
            payload_json: json!({"data": 7}).to_string(),
            created_at: iso(at(base, 1)),
        },
    )
    .map_err(|_| FAILED)?;
    let terminal = crate::store::update_session_terminal_if_active(
        conn,
        &session_id,
        "completed",
        Some(0),
        &iso(at(base, 5)),
    )
    .map_err(|_| FAILED)?;
    if !terminal {
        return Err(FAILED);
    }
    crate::automations::runner::settle_finished_runs(conn, at(base, 5)).map_err(|_| FAILED)?;
    Ok(outcome.run_id)
}

pub(super) fn state(
    conn: &Connection,
    run_id: &str,
) -> Result<(DeliveryState, Option<String>), &'static str> {
    let delivery = get_run(conn, run_id)
        .map_err(|_| FAILED)?
        .ok_or(FAILED)?
        .delivery
        .ok_or(FAILED)?;
    let state = match delivery.state.as_str() {
        "pending" => DeliveryState::Pending,
        "delivered" => DeliveryState::Delivered,
        "failed" => DeliveryState::Failed,
        _ => return Err(FAILED),
    };
    Ok((state, delivery.notice_id))
}

fn finished_at(conn: &Connection, run_id: &str) -> Result<String, &'static str> {
    get_run(conn, run_id)
        .map_err(|_| FAILED)?
        .ok_or(FAILED)?
        .finished_at
        .ok_or(FAILED)
}

pub(super) fn notices_for(conn: &Connection, run_id: &str) -> Result<Vec<String>, &'static str> {
    let mut statement = conn
        .prepare(
            "SELECT id FROM session_notices
             WHERE source_kind = ?1 AND source_id = ?2 ORDER BY created_at, id",
        )
        .map_err(|_| FAILED)?;
    let ids = statement
        .query_map([NOTICE_SOURCE_KIND, run_id], |row| row.get(0))
        .map_err(|_| FAILED)?
        .collect::<rusqlite::Result<Vec<String>>>()
        .map_err(|_| FAILED)?;
    Ok(ids)
}

fn pending_notice_ids(conn: &Connection) -> Result<Vec<String>, &'static str> {
    Ok(
        main_session_notices::pending_notices(conn, INSTANCE_MAIN_SCOPE_KEY)
            .map_err(|_| FAILED)?
            .into_iter()
            .map(|notice| notice.id)
            .collect(),
    )
}

/// True only for the fixture trigger's own abort.
fn is_fixture_fault(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        matches!(
            cause.downcast_ref::<rusqlite::Error>(),
            Some(rusqlite::Error::SqliteFailure(failure, Some(message)))
                if failure.extended_code == SQLITE_CONSTRAINT_TRIGGER && message == FIXTURE_FAULT
        )
    })
}

/// A settled record refuses to change: a delivered one to point at another
/// notice, a failed one to reopen. Only the production immutability trigger's
/// own abort counts, and the record must read back unchanged.
fn settled_record_refuses_change(
    conn: &Connection,
    run_id: &str,
    final_state: DeliveryState,
    notice_id: Option<&str>,
) -> Result<bool, &'static str> {
    let statement = match final_state {
        DeliveryState::Delivered => {
            "UPDATE automation_runs SET delivery_notice_id = 'conformance-other' WHERE id = ?1"
        }
        DeliveryState::Failed => {
            "UPDATE automation_runs SET delivery_state = 'pending' WHERE id = ?1"
        }
        DeliveryState::Pending => return Ok(false),
    };
    let refused = matches!(
        conn.execute(statement, [run_id]),
        Err(rusqlite::Error::SqliteFailure(failure, Some(message)))
            if failure.extended_code == SQLITE_CONSTRAINT_TRIGGER && message == DELIVERY_IMMUTABLE
    );
    let (state_after, notice_after) = state(conn, run_id)?;
    Ok(refused && state_after == final_state && notice_after.as_deref() == notice_id)
}

fn case_matches(case: &Case) -> Result<(bool, Value), &'static str> {
    let base = DateTime::parse_from_rfc3339(BASE)
        .map_err(|_| FAILED)?
        .with_timezone(&Utc);
    let home = scheduler_conformance_tempdir()?;
    crate::daemon::ensure_private_coven_home(home.path()).map_err(|_| FAILED)?;
    let path = crate::api::store_path(home.path());
    let conn = crate::store::open_store(&path).map_err(|_| FAILED)?;
    let mut definition = case.definition.clone();
    if case.fault == Fault::UnreadableResult {
        definition["cwd"] = json!(FIXTURE_CWD);
    }
    let definition = RoutineDefinition::from_json(&definition).map_err(|_| INVALID)?;
    crate::automations::store::insert_definition(&conn, &definition).map_err(|_| FAILED)?;
    let run_id = match case.fault {
        Fault::UnreadableResult => settle_unreadable_run(&conn, base, &definition)?,
        _ => {
            settle_run(&conn, base, &definition.id, RUN_ID, 0)?;
            RUN_ID.to_owned()
        }
    };
    // A healthy run that settles after the unreadable one, so the pass reaches
    // the unreadable run first and must not stop there.
    if case.fault == Fault::UnreadableResult {
        settle_run(&conn, base, &definition.id, HEALTHY_RUN_ID, 30)?;
        if finished_at(&conn, &run_id)? >= finished_at(&conn, HEALTHY_RUN_ID)? {
            return Err(FAILED);
        }
    }
    let pending_before_fault = state(&conn, &run_id)?.0 == DeliveryState::Pending;

    let mut gate = None;
    let mut waiting = Vec::new();
    match case.fault {
        Fault::GateHeld => {
            gate =
                Some(ScopeGate::acquire(home.path(), INSTANCE_MAIN_SCOPE_KEY).map_err(|_| FAILED)?);
        }
        Fault::InboxFull => {
            let mut writer = crate::store::open_store(&path).map_err(|_| FAILED)?;
            for index in 0..MAX_PENDING_NOTICES_PER_SCOPE {
                let source = format!("conformance-handoff-{index}");
                let notice = NewNotice {
                    scope_key: INSTANCE_MAIN_SCOPE_KEY,
                    source_kind: "handoff",
                    source_id: Some(&source),
                    text: "Conformance handoff waiting for a turn.",
                    context_key: None,
                };
                waiting.push(
                    main_session_notices::enqueue_notice(&mut writer, &notice, &iso(at(base, 61)))
                        .map_err(|_| FAILED)?
                        .id,
                );
            }
        }
        Fault::DischargeRollback => {
            // Aborts the `delivered` record inside the discharge transaction,
            // and only once the notice it names is visible there, so a
            // discharge that recorded before inserting would deliver instead.
            // TEMP, so it lives only on this connection.
            conn.execute_batch(&format!(
                "CREATE TEMP TRIGGER conformance_delivery_fault
                 BEFORE UPDATE OF delivery_state ON main.automation_runs
                 WHEN NEW.delivery_state = 'delivered'
                  AND EXISTS (SELECT 1 FROM main.session_notices
                              WHERE id = NEW.delivery_notice_id)
                 BEGIN SELECT RAISE(ABORT, '{FIXTURE_FAULT}'); END;"
            ))
            .map_err(|_| FAILED)?;
        }
        Fault::GateUnavailable => {
            // The gate's lock directory cannot be created where a file stands.
            std::fs::write(home.path().join(SCOPE_LOCK_DIR), b"conformance fixture")
                .map_err(|_| FAILED)?;
        }
        Fault::None | Fault::UnreadableResult => {}
    }

    let first = deliver_pending_results(home.path(), &conn, None, at(base, 120));
    let mut pass_failure_from_fixture = None;
    let mut healthy_run_delivered = None;
    let (first_pass, failure_reason) = match &first {
        Err(error) => {
            pass_failure_from_fixture = Some(is_fixture_fault(error));
            (FirstPass::PassFailed, None)
        }
        Ok(report) => {
            if case.fault == Fault::UnreadableResult {
                healthy_run_delivered = Some(report.delivered == [HEALTHY_RUN_ID]);
            }
            let failed = report
                .failed
                .iter()
                .find(|(id, _)| *id == run_id)
                .map(|(_, reason)| reason.as_str());
            if report.delivered.contains(&run_id) {
                (FirstPass::Delivered, None)
            } else if report.deferred.contains(&run_id) {
                (FirstPass::Deferred, None)
            } else if failed == Some(GATE_UNAVAILABLE_REASON) {
                (FirstPass::Failed, Some(FailureReason::GateUnavailable))
            } else if failed == Some(RESULT_UNREADABLE_REASON) {
                (FirstPass::Failed, Some(FailureReason::ResultUnreadable))
            } else {
                return Err(FAILED);
            }
        }
    };
    let (state_after_first, _) = state(&conn, &run_id)?;
    let notices_after_first = notices_for(&conn, &run_id)?.len();

    let mut waiting_preserved = None;
    match case.fault {
        Fault::GateHeld => drop(gate.take()),
        Fault::InboxFull => {
            waiting_preserved = Some(pending_notice_ids(&conn)? == waiting);
            // A turn drains the inbox; the waiting result may then post.
            main_session_notices::mark_delivered(
                &conn,
                &waiting,
                "conformance-turn",
                &iso(at(base, 150)),
            )
            .map_err(|_| FAILED)?;
        }
        Fault::DischargeRollback => {
            conn.execute_batch("DROP TRIGGER temp.conformance_delivery_fault")
                .map_err(|_| FAILED)?;
        }
        Fault::GateUnavailable => {
            std::fs::remove_file(home.path().join(SCOPE_LOCK_DIR)).map_err(|_| FAILED)?;
        }
        Fault::None | Fault::UnreadableResult => {}
    }
    deliver_pending_results(home.path(), &conn, None, at(base, 180)).map_err(|_| FAILED)?;
    // A pass after the obligation settled must change nothing.
    let replay =
        deliver_pending_results(home.path(), &conn, None, at(base, 240)).map_err(|_| FAILED)?;
    let replay_is_noop = replay == DeliveryReport::default();

    let (final_state, notice_id) = state(&conn, &run_id)?;
    let notices = notices_for(&conn, &run_id)?;
    let record_names_notice = (final_state == DeliveryState::Delivered)
        .then(|| notices.len() == 1 && notice_id.as_deref() == notices.first().map(String::as_str));
    let settled_record_immutable =
        settled_record_refuses_change(&conn, &run_id, final_state, notice_id.as_deref())?;
    let expected = &case.expected;
    let passed = pending_before_fault
        && first_pass == expected.first_pass
        && failure_reason == expected.failure_reason
        && state_after_first
            == match first_pass {
                FirstPass::Delivered => DeliveryState::Delivered,
                FirstPass::Deferred | FirstPass::PassFailed => DeliveryState::Pending,
                FirstPass::Failed => DeliveryState::Failed,
            }
        && notices_after_first as u64 == u64::from(first_pass == FirstPass::Delivered)
        && final_state == expected.final_state
        && notices.len() as u64 == expected.final_notices
        && record_names_notice != Some(false)
        && (final_state != DeliveryState::Failed || notice_id.is_none())
        && replay_is_noop == expected.replay_is_noop
        && settled_record_immutable == expected.settled_record_immutable
        && pass_failure_from_fixture == (case.fault == Fault::DischargeRollback).then_some(true)
        && waiting_preserved == (case.fault == Fault::InboxFull).then_some(true)
        && healthy_run_delivered == (case.fault == Fault::UnreadableResult).then_some(true);
    Ok((
        passed,
        json!({
            "caseId": case.case_id,
            "faultSource": case.fault,
            "obligationPendingBeforeFault": pending_before_fault,
            "firstPass": first_pass,
            "failureReason": failure_reason,
            "stateAfterFirstPass": state_after_first,
            "noticesAfterFirstPass": notices_after_first,
            "passFailureFromFixture": pass_failure_from_fixture,
            "waitingNoticesPreserved": waiting_preserved,
            "healthyRunDelivered": healthy_run_delivered,
            "finalState": final_state,
            "noticesForRun": notices.len(),
            "recordNamesNotice": record_names_notice,
            "replayIsNoop": replay_is_noop,
            "settledRecordImmutable": settled_record_immutable,
        }),
    ))
}
