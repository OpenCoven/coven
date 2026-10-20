//! Disposable run-settlement crash fixtures: a worker process settles one run
//! whose session has ended and is killed at a settlement boundary. The run
//! launches through a no-op runtime; no runtime process starts.
use std::collections::BTreeSet;
use std::path::Path;

use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::command_process_crash_recovery::{
    abruptly_terminated, announce, spawn_worker, CHILD_GUARD, SCRATCH_ENV,
};
use super::process_crash_fixture::{
    install_pause, open_raw, raw_integrity_ok, worker_home, write_transaction_open, Pause,
};
use super::result_delivery_recovery::{at, iso, notices_for, DeliveryState, BASE};
use super::valid_case_id;
use crate::automations::definition::RoutineDefinition;
use crate::automations::delivery::{deliver_pending_results, DeliveryReport};
use crate::automations::runner::{settle_finished_runs, SettlementReport};

const INVALID: &str = "conformance vector is invalid";
const FAILED: &str = "conformance suite execution failed";
const SCHEMA_VERSION: &str = "coven.automations.run-settlement-process-crash-recovery-vectors.v1";
const WORKER: &str = "run-settlement-crash-worker";
const HOME_PREFIX: &str = "settlement-crash-";

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
enum Boundary {
    AfterOccurrence,
    AfterAttempt,
    AfterRun,
    AfterCommit,
    None,
}
impl Boundary {
    /// The worker is killed before the settlement transaction commits.
    fn before_commit(self) -> bool {
        matches!(
            self,
            Self::AfterOccurrence | Self::AfterAttempt | Self::AfterRun
        )
    }
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Case {
    case_id: String,
    boundary: Boundary,
    definition: Value,
    expected: Expected,
}

#[derive(Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Expected {
    initial_outcome: InitialOutcome,
    settlement_after_exit: Settlement,
    recovery_settled: u64,
    final_settlement: Settlement,
    final_delivery: Option<DeliveryState>,
    final_notices: u64,
    integrity_ok: bool,
    replay_is_noop: bool,
}

#[derive(Deserialize, Serialize, PartialEq, Clone, Copy)]
#[serde(rename_all = "snake_case")]
enum InitialOutcome {
    Terminated,
    Settled,
}

/// The run, its one attempt and its occurrence, read together.
#[derive(Debug, Deserialize, Serialize, PartialEq, Clone, Copy)]
#[serde(rename_all = "snake_case")]
enum Settlement {
    /// Exactly as the launch left them: the run and its occurrence `running`,
    /// the attempt `started` and unsettled, and no delivery obligation.
    Unsettled,
    /// All three succeeded, the attempt has settled, and the run carries its
    /// delivery obligation.
    Settled,
    /// Anything else, including a partial settlement.
    Mixed,
}

/// What each boundary must produce. A vector restates it, so it cannot weaken it.
fn expected_for(boundary: Boundary) -> Expected {
    let before_commit = boundary.before_commit();
    Expected {
        initial_outcome: if boundary == Boundary::None {
            InitialOutcome::Settled
        } else {
            InitialOutcome::Terminated
        },
        settlement_after_exit: if before_commit {
            Settlement::Unsettled
        } else {
            Settlement::Settled
        },
        recovery_settled: u64::from(before_commit),
        final_settlement: Settlement::Settled,
        final_delivery: Some(DeliveryState::Delivered),
        final_notices: 1,
        integrity_ok: true,
        replay_is_noop: true,
    }
}

/// The runner launches only a routine with a working directory; a no-op
/// runtime never touches it.
fn synthetic_definition() -> Value {
    json!({"schemaVersion":1,"id":"conformance-settlement-crash",
        "name":"Conformance settlement crash","status":"PAUSED",
        "rrule":"FREQ=DAILY;BYHOUR=9","timezone":"utc","misfire":"latest",
        "overlap":"forbid","timeoutMinutes":30,"runtime":"coven-code",
        "prompt":"Run the native settlement crash conformance probe.",
        "cwd":"/work/project","outputTarget":{"kind":"main-session"}})
}

fn validate(case: &Case) -> Result<(), &'static str> {
    if !valid_case_id(&case.case_id)
        || case.definition != synthetic_definition()
        || case.expected != expected_for(case.boundary)
    {
        return Err(INVALID);
    }
    Ok(())
}

fn base() -> Result<DateTime<Utc>, &'static str> {
    Ok(DateTime::parse_from_rfc3339(BASE)
        .map_err(|_| FAILED)?
        .with_timezone(&Utc))
}

/// Hidden worker: its input cannot choose a database. Every invocation creates
/// a fresh random child directory under the caller's conformance scratch root.
/// The parent owns that root and removes it after killing/reaping this worker.
pub(super) fn worker(input: &Value) -> Result<(), &'static str> {
    let case: Case = serde_json::from_value(input.clone()).map_err(|_| INVALID)?;
    validate(&case)?;
    let scratch = std::env::var_os(SCRATCH_ENV).ok_or(FAILED)?;
    let temporary = tempfile::Builder::new()
        .prefix(HOME_PREFIX)
        .tempdir_in(scratch)
        .map_err(|_| FAILED)?;
    crate::daemon::ensure_private_coven_home(temporary.path()).map_err(|_| FAILED)?;
    // A killed process cannot run TempDir::drop; the parent removes the root.
    let home = temporary.keep();
    let relative_home = home
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or(FAILED)?
        .to_owned();
    let base = base()?;
    let conn = crate::store::open_store(&home.join("store.sqlite")).map_err(|_| FAILED)?;
    let definition = RoutineDefinition::from_json(&case.definition).map_err(|_| INVALID)?;
    crate::automations::store::insert_definition(&conn, &definition).map_err(|_| FAILED)?;
    let outcome = crate::automations::runner::run_routine_now(
        &conn,
        &crate::api::NoopSessionRuntime,
        &definition,
        at(base, 0),
    )
    .map_err(|_| FAILED)?;
    if outcome.status != "running" || outcome.error.is_some() {
        return Err(FAILED);
    }
    let session_id = outcome.session_id.ok_or(FAILED)?;
    let ended = crate::store::update_session_terminal_if_active(
        &conn,
        &session_id,
        "completed",
        Some(0),
        &iso(at(base, 5)),
    )
    .map_err(|_| FAILED)?;
    if !ended || settlement(&conn)?.0 != Settlement::Unsettled {
        return Err(FAILED);
    }
    let pause_at = match case.boundary {
        // Settlement is the only state change after the launch.
        Boundary::AfterOccurrence => Some(Pause::After(
            "AFTER UPDATE OF state ON main.automation_occurrences
             WHEN OLD.state IS NOT NEW.state",
        )),
        Boundary::AfterAttempt => Some(Pause::After(
            "AFTER UPDATE OF state ON main.automation_attempts
             WHEN OLD.state IS NOT NEW.state",
        )),
        Boundary::AfterRun => Some(Pause::After(
            "AFTER UPDATE OF status ON main.automation_runs
             WHEN OLD.status IS NOT NEW.status",
        )),
        Boundary::AfterCommit => Some(Pause::AfterCommit),
        Boundary::None => None,
    };
    if let Some(pause_at) = pause_at {
        let handshake =
            json!({"state":"paused", "boundary":case.boundary, "relativeHome":relative_home});
        install_pause(&conn, pause_at, handshake)?;
    }
    let report = settle_finished_runs(&conn, at(base, 60)).map_err(|_| FAILED)?;
    if case.boundary != Boundary::None || report != settled_one() {
        return Err(FAILED);
    }
    drop(conn);
    announce(&json!({"state":"settled", "boundary":case.boundary, "relativeHome":relative_home}))
}

fn settled_one() -> SettlementReport {
    SettlementReport {
        succeeded: 1,
        ..SettlementReport::default()
    }
}

/// Classifies the store's only run with its one attempt and its occurrence,
/// and returns the run id.
fn settlement(conn: &Connection) -> Result<(Settlement, String), &'static str> {
    let mut statement = conn
        .prepare(
            "SELECT r.id, r.status, r.delivery_state, a.state, a.settled_at IS NULL, o.state
             FROM automation_runs AS r
             JOIN automation_attempts AS a ON a.run_id = r.id
             JOIN automation_occurrences AS o ON o.id = r.occurrence_id",
        )
        .map_err(|_| FAILED)?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, bool>(4)?,
                row.get::<_, String>(5)?,
            ))
        })
        .map_err(|_| FAILED)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|_| FAILED)?;
    let [(run_id, run, delivery, attempt, attempt_open, occurrence)] =
        <[_; 1]>::try_from(rows).map_err(|_| FAILED)?;
    let unsettled = run == "running"
        && delivery.is_none()
        && attempt == "started"
        && attempt_open
        && occurrence == "running";
    let settled = run == "succeeded"
        && matches!(delivery.as_deref(), Some("pending" | "delivered"))
        && attempt == "succeeded"
        && !attempt_open
        && occurrence == "succeeded";
    let class = if unsettled {
        Settlement::Unsettled
    } else if settled {
        Settlement::Settled
    } else {
        Settlement::Mixed
    };
    Ok((class, run_id))
}

/// The run's delivery state and the notice its record names.
fn delivery(
    conn: &Connection,
    run_id: &str,
) -> Result<(Option<DeliveryState>, Option<String>), &'static str> {
    let (state, notice): (Option<String>, Option<String>) = conn
        .query_row(
            "SELECT delivery_state, delivery_notice_id FROM automation_runs WHERE id = ?1",
            [run_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|_| FAILED)?
        .ok_or(FAILED)?;
    let state = match state.as_deref() {
        None => None,
        Some("pending") => Some(DeliveryState::Pending),
        Some("delivered") => Some(DeliveryState::Delivered),
        Some("failed") => Some(DeliveryState::Failed),
        Some(_) => return Err(FAILED),
    };
    Ok((state, notice))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct VectorSet {
    schema_version: String,
    cases: Vec<Case>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Handshake {
    state: String,
    boundary: Boundary,
    relative_home: String,
}

pub(super) fn evaluate(vector: &Value) -> Result<(bool, Vec<Value>), &'static str> {
    let vectors: VectorSet = serde_json::from_value(vector.clone()).map_err(|_| INVALID)?;
    if vectors.schema_version != SCHEMA_VERSION || vectors.cases.len() != 5 {
        return Err(INVALID);
    }
    let mut ids = BTreeSet::new();
    let mut boundaries = BTreeSet::new();
    for case in &vectors.cases {
        validate(case)?;
        if !ids.insert(&case.case_id) || !boundaries.insert(case.boundary) {
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

/// The parent's view of the store while the worker is paused: whether the
/// worker's write transaction is still open, the committed settlement, and the
/// run's delivery state.
fn observe_pause(path: &Path) -> Result<(bool, Settlement, Option<DeliveryState>), &'static str> {
    let probe = open_raw(path)?;
    let open = write_transaction_open(&probe)?;
    let (settlement, run_id) = settlement(&probe)?;
    Ok((open, settlement, delivery(&probe, &run_id)?.0))
}

/// The settlement and delivery steps of the daemon's reconciliation, in its
/// order (`daemon_tick.rs`), with no runtime. The steps before them recover
/// containment, expired cancellations, abandoned launches and timeouts, none
/// of which these cases hold.
fn reconcile(
    home: &Path,
    conn: &Connection,
    now: DateTime<Utc>,
) -> Result<(SettlementReport, DeliveryReport), &'static str> {
    let settled = settle_finished_runs(conn, now).map_err(|_| FAILED)?;
    let delivered = deliver_pending_results(home, conn, None, now).map_err(|_| FAILED)?;
    Ok((settled, delivered))
}

fn case_matches(case: &Case) -> Result<(bool, Value), &'static str> {
    let root = super::scheduler_conformance_tempdir()?;
    crate::daemon::ensure_private_coven_home(root.path()).map_err(|_| FAILED)?;
    let payload = serde_json::to_vec(case).map_err(|_| INVALID)?;
    let (mut process, receiver) = spawn_worker::<Handshake>(WORKER, payload, root.path())?;
    let ready = receiver.recv_timeout(CHILD_GUARD).map_err(|_| FAILED)??;
    let crash = case.boundary != Boundary::None;
    if ready.boundary != case.boundary || ready.state != if crash { "paused" } else { "settled" } {
        return Err(FAILED);
    }
    let home = worker_home(root.path(), &ready.relative_home, HOME_PREFIX)?;
    let path = home.join("store.sqlite");
    let pause = crash.then(|| observe_pause(&path)).transpose()?;
    let transaction_open_at_pause = pause.map(|(open, _, _)| open);
    let settlement_at_pause = pause.map(|(_, settlement, _)| settlement);
    // Before the commit, the worker's transaction is open and nothing it wrote
    // is visible; after it, the settlement and its pending obligation are
    // committed and no write is open.
    let pause_consistent = pause.is_none_or(|(open, settlement, delivery)| {
        if case.boundary.before_commit() {
            open && settlement == Settlement::Unsettled
        } else {
            !open && settlement == Settlement::Settled && delivery == Some(DeliveryState::Pending)
        }
    });
    let status = process.finish(crash)?;
    let terminated = abruptly_terminated(&status);
    if crash != terminated || (!crash && !status.success()) {
        return Err(FAILED);
    }
    drop(process);
    let integrity_ok = raw_integrity_ok(&path)?;

    let base = base()?;
    let conn = crate::store::open_store(&path).map_err(|_| FAILED)?;
    let (settlement_after_exit, run_id) = settlement(&conn)?;
    let (delivery_after_exit, _) = delivery(&conn, &run_id)?;
    let notices_after_exit = notices_for(&conn, &run_id)?.len() as u64;
    // A store that recovery refuses fails the case; it is evidence, not a
    // fixture failure.
    let recovery = reconcile(&home, &conn, at(base, 120)).ok();
    let recovery_refused = recovery.is_none();
    let recovery_consistent = recovery.as_ref().is_some_and(|(settled, delivered)| {
        settled.failed == 0
            && settled.cancelled == 0
            && delivered.delivered == [run_id.as_str()]
            && delivered.deferred.is_empty()
            && delivered.failed.is_empty()
            && delivered.warnings.is_empty()
    });
    let (final_settlement, _) = settlement(&conn)?;
    let (final_delivery, final_notice) = delivery(&conn, &run_id)?;
    let final_notices = notices_for(&conn, &run_id)?;
    let record_names_notice = final_notice.is_some()
        && final_notices == final_notice.clone().into_iter().collect::<Vec<_>>();
    drop(conn);
    // A reconciliation from a fresh connection must change nothing.
    let reopened = crate::store::open_store(&path).map_err(|_| FAILED)?;
    let replay = reconcile(&home, &reopened, at(base, 180)).ok();
    let replay_is_noop = replay == Some((SettlementReport::default(), DeliveryReport::default()))
        && settlement(&reopened)?.0 == final_settlement
        && delivery(&reopened, &run_id)? == (final_delivery, final_notice)
        && notices_for(&reopened, &run_id)? == final_notices;
    drop(reopened);

    let observed = Expected {
        initial_outcome: if crash {
            InitialOutcome::Terminated
        } else {
            InitialOutcome::Settled
        },
        settlement_after_exit,
        recovery_settled: recovery
            .as_ref()
            .map_or(0, |(settled, _)| settled.succeeded as u64),
        final_settlement,
        final_delivery,
        final_notices: final_notices.len() as u64,
        integrity_ok,
        replay_is_noop,
    };
    // An unsettled run carries no obligation; a settled one had not yet been
    // delivered when the worker stopped, so nothing reached the inbox.
    let exit_consistent = notices_after_exit == 0
        && match settlement_after_exit {
            Settlement::Unsettled => delivery_after_exit.is_none(),
            Settlement::Settled => delivery_after_exit == Some(DeliveryState::Pending),
            Settlement::Mixed => false,
        };
    let passed = observed == case.expected
        && pause_consistent
        && exit_consistent
        && recovery_consistent
        && record_names_notice;
    Ok((
        passed,
        json!({
            "caseId": case.case_id,
            "boundary": case.boundary,
            "initialOutcome": observed.initial_outcome,
            "abruptlyTerminated": terminated,
            "transactionOpenAtPause": transaction_open_at_pause,
            "settlementAtPause": settlement_at_pause,
            "integrityOk": integrity_ok,
            "settlementAfterExit": settlement_after_exit,
            "deliveryAfterExit": delivery_after_exit,
            "noticesAfterExit": notices_after_exit,
            "exitConsistent": exit_consistent,
            "recoveryRefused": recovery_refused,
            "recoverySettled": observed.recovery_settled,
            "recoveryConsistent": recovery_consistent,
            "finalSettlement": final_settlement,
            "finalDelivery": observed.final_delivery,
            "finalNotices": observed.final_notices,
            "recordNamesNotice": record_names_notice,
            "replayIsNoop": replay_is_noop,
        }),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vectors() -> Value {
        serde_json::from_str(include_str!(
            "../../../../../conformance/automations/runner/run-settlement-process-crash-recovery.vectors.json"
        ))
        .unwrap()
    }

    #[test]
    fn settlement_crash_vectors_restate_each_boundary_outcome() {
        let vectors: VectorSet = serde_json::from_value(vectors()).unwrap();
        assert_eq!(vectors.schema_version, SCHEMA_VERSION);
        for case in &vectors.cases {
            validate(case).unwrap();
        }
        let boundaries: Vec<_> = vectors.cases.iter().map(|case| case.boundary).collect();
        assert_eq!(
            boundaries,
            [
                Boundary::AfterOccurrence,
                Boundary::AfterAttempt,
                Boundary::AfterRun,
                Boundary::AfterCommit,
                Boundary::None
            ]
        );
    }

    #[test]
    fn settlement_crash_vectors_require_each_boundary_and_the_control() {
        let original = vectors();
        let mut missing = original.clone();
        missing["cases"].as_array_mut().unwrap().pop();
        assert_eq!(evaluate(&missing).unwrap_err(), INVALID);
        // A second control under its own case id, replacing a boundary.
        let mut duplicate = original.clone();
        let mut control = duplicate["cases"][4].clone();
        control["caseId"] = json!("second-uninterrupted-settlement-control");
        duplicate["cases"][0] = control;
        assert_eq!(evaluate(&duplicate).unwrap_err(), INVALID);
        let mut duplicate_id = original;
        duplicate_id["cases"][1]["caseId"] = duplicate_id["cases"][0]["caseId"].clone();
        assert_eq!(evaluate(&duplicate_id).unwrap_err(), INVALID);
    }

    #[test]
    fn settlement_crash_vectors_refuse_malformed_or_weakened_inputs() {
        let original = vectors();
        for (case, field, value) in [
            (0, "initialOutcome", json!("settled")),
            (0, "settlementAfterExit", json!("mixed")),
            (1, "settlementAfterExit", json!("settled")),
            (1, "recoverySettled", json!(0)),
            (2, "finalSettlement", json!("unsettled")),
            (2, "finalNotices", json!(2)),
            (3, "settlementAfterExit", json!("unsettled")),
            (3, "recoverySettled", json!(1)),
            (3, "finalDelivery", json!("pending")),
            (4, "integrityOk", json!(false)),
            (4, "replayIsNoop", json!(false)),
        ] {
            let mut invalid = original.clone();
            invalid["cases"][case]["expected"][field] = value;
            assert_eq!(evaluate(&invalid).unwrap_err(), INVALID, "{case} {field}");
        }
        for (field, value) in [
            ("name", json!("Caller chosen")),
            ("status", json!("ACTIVE")),
            ("cwd", json!("/private-input")),
            ("outputTarget", json!({"kind": "none"})),
        ] {
            let mut invalid = original.clone();
            invalid["cases"][0]["definition"][field] = value;
            assert_eq!(
                evaluate(&invalid).unwrap_err(),
                INVALID,
                "definition {field}"
            );
        }
        for (field, value) in [
            ("storePath", json!("private-input")),
            ("runId", json!("private-input")),
            ("boundary", json!("after_dispatch")),
            ("caseId", json!("")),
        ] {
            let mut invalid = original.clone();
            invalid["cases"][0][field] = value;
            assert_eq!(evaluate(&invalid).unwrap_err(), INVALID, "{field}");
        }
        let mut invalid = original;
        invalid["schemaVersion"] =
            json!("coven.automations.result-delivery-process-crash-recovery-vectors.v1");
        assert_eq!(evaluate(&invalid).unwrap_err(), INVALID);
    }

    #[test]
    fn settlement_is_whole_or_mixed() {
        let home = tempfile::tempdir().unwrap();
        let conn = crate::store::open_store(&home.path().join("store.sqlite")).unwrap();
        let definition = RoutineDefinition::from_json(&synthetic_definition()).unwrap();
        crate::automations::store::insert_definition(&conn, &definition).unwrap();
        let base = base().unwrap();
        let outcome = crate::automations::runner::run_routine_now(
            &conn,
            &crate::api::NoopSessionRuntime,
            &definition,
            at(base, 0),
        )
        .unwrap();
        assert!(crate::store::update_session_terminal_if_active(
            &conn,
            &outcome.session_id.unwrap(),
            "completed",
            Some(0),
            &iso(at(base, 5)),
        )
        .unwrap());
        assert_eq!(settlement(&conn).unwrap().0, Settlement::Unsettled);
        // What a settlement split after each write would leave committed.
        conn.execute_batch("BEGIN").unwrap();
        conn.execute("UPDATE automation_occurrences SET state = 'succeeded'", [])
            .unwrap();
        assert_eq!(settlement(&conn).unwrap().0, Settlement::Mixed);
        conn.execute(
            "UPDATE automation_attempts SET state = 'succeeded', settled_at = ?1",
            [iso(at(base, 60))],
        )
        .unwrap();
        assert_eq!(settlement(&conn).unwrap().0, Settlement::Mixed);
        conn.execute_batch("ROLLBACK").unwrap();
        assert_eq!(settlement(&conn).unwrap().0, Settlement::Unsettled);
        assert_eq!(
            settle_finished_runs(&conn, at(base, 60)).unwrap(),
            settled_one()
        );
        let (class, run_id) = settlement(&conn).unwrap();
        assert_eq!(class, Settlement::Settled);
        assert_eq!(
            delivery(&conn, &run_id).unwrap(),
            (Some(DeliveryState::Pending), None)
        );
    }

    #[test]
    fn worker_refuses_invalid_or_weakened_input() {
        let mut case = vectors()["cases"][0].clone();
        case["storePath"] = json!("private-input");
        assert_eq!(worker(&case).unwrap_err(), INVALID);
        let mut case = vectors()["cases"][0].clone();
        case["expected"]["recoverySettled"] = json!(0);
        assert_eq!(worker(&case).unwrap_err(), INVALID);
    }
}
