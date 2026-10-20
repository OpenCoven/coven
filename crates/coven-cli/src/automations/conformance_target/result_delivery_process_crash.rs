//! Disposable result-delivery crash fixtures: a worker process discharges one
//! settled run's result and is killed at a delivery boundary. No runtime
//! launch, inbox turn or release claim.
use std::collections::BTreeSet;
use std::path::Path;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::command_process_crash_recovery::{
    abruptly_terminated, announce, spawn_worker, CHILD_GUARD, SCRATCH_ENV,
};
use super::process_crash_fixture::{
    install_pause, open_raw, raw_integrity_ok, worker_home, write_transaction_open, Pause,
};
use super::result_delivery_recovery::{at, notices_for, settle_run, state, DeliveryState, BASE};
use super::valid_case_id;
use crate::automations::definition::RoutineDefinition;
use crate::automations::delivery::{deliver_pending_results, DeliveryReport};
use crate::main_session::INSTANCE_MAIN_SCOPE_KEY;
use crate::main_session_routes::ScopeGate;

const INVALID: &str = "conformance vector is invalid";
const FAILED: &str = "conformance suite execution failed";
const SCHEMA_VERSION: &str = "coven.automations.result-delivery-process-crash-recovery-vectors.v1";
const WORKER: &str = "result-delivery-crash-worker";
const HOME_PREFIX: &str = "delivery-crash-";
const RUN_ID: &str = "delivery-crash-run";

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
enum Boundary {
    AfterNotice,
    AfterRecord,
    AfterCommit,
    None,
}
impl Boundary {
    /// The worker is killed before the delivery transaction commits.
    fn before_commit(self) -> bool {
        matches!(self, Self::AfterNotice | Self::AfterRecord)
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
    state_after_exit: DeliveryState,
    notices_after_exit: u64,
    recovery_pass: RecoveryPass,
    final_state: DeliveryState,
    final_notices: u64,
    integrity_ok: bool,
    gate_released: bool,
    replay_is_noop: bool,
}

#[derive(Deserialize, Serialize, PartialEq, Clone, Copy)]
#[serde(rename_all = "snake_case")]
enum InitialOutcome {
    Terminated,
    Delivered,
}

#[derive(Deserialize, Serialize, PartialEq, Clone, Copy)]
#[serde(rename_all = "snake_case")]
enum RecoveryPass {
    Delivered,
    Unchanged,
    Other,
}

/// What each boundary must produce. A vector restates it, so it cannot weaken it.
fn expected_for(boundary: Boundary) -> Expected {
    let before_commit = boundary.before_commit();
    Expected {
        initial_outcome: if boundary == Boundary::None {
            InitialOutcome::Delivered
        } else {
            InitialOutcome::Terminated
        },
        state_after_exit: if before_commit {
            DeliveryState::Pending
        } else {
            DeliveryState::Delivered
        },
        notices_after_exit: u64::from(!before_commit),
        recovery_pass: if before_commit {
            RecoveryPass::Delivered
        } else {
            RecoveryPass::Unchanged
        },
        final_state: DeliveryState::Delivered,
        final_notices: 1,
        integrity_ok: true,
        gate_released: true,
        replay_is_noop: true,
    }
}

fn synthetic_definition() -> Value {
    json!({"schemaVersion":1,"id":"conformance-delivery-crash","name":"Conformance delivery crash",
        "status":"PAUSED","rrule":"FREQ=DAILY;BYHOUR=9","timezone":"utc",
        "misfire":"latest","overlap":"forbid","timeoutMinutes":30,
        "runtime":"coven-code","prompt":"Run the native delivery crash conformance probe.",
        "outputTarget":{"kind":"main-session"}})
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
    settle_run(&conn, base, &definition.id, RUN_ID, 0)?;
    let pause_at = match case.boundary {
        Boundary::AfterNotice => Some(Pause::After("AFTER INSERT ON main.session_notices")),
        Boundary::AfterRecord => Some(Pause::After(
            "AFTER UPDATE OF delivery_state ON main.automation_runs
             WHEN NEW.delivery_state = 'delivered'",
        )),
        Boundary::AfterCommit => Some(Pause::AfterCommit),
        Boundary::None => None,
    };
    if let Some(pause_at) = pause_at {
        let handshake =
            json!({"state":"paused", "boundary":case.boundary, "relativeHome":relative_home});
        install_pause(&conn, pause_at, handshake)?;
    }
    let report = deliver_pending_results(&home, &conn, None, at(base, 180)).map_err(|_| FAILED)?;
    if case.boundary != Boundary::None || report.delivered != [RUN_ID] {
        return Err(FAILED);
    }
    drop(conn);
    announce(&json!({"state":"delivered", "boundary":case.boundary, "relativeHome":relative_home}))
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
    if vectors.schema_version != SCHEMA_VERSION || vectors.cases.len() != 4 {
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
/// worker's write transaction is still open, and the run's committed state.
fn observe_pause(path: &Path) -> Result<(bool, DeliveryState), &'static str> {
    let probe = open_raw(path)?;
    let open = write_transaction_open(&probe)?;
    let (state, _) = state(&probe, RUN_ID)?;
    Ok((open, state))
}

/// Whether this process can take the main-session gate now.
fn gate_free(home: &Path) -> Result<bool, &'static str> {
    Ok(ScopeGate::try_acquire(home, INSTANCE_MAIN_SCOPE_KEY)
        .map_err(|_| FAILED)?
        .is_some())
}

/// The OS releases a dead holder's lock, though Windows documents that it may
/// take a moment. Fixture hang guard only, not a recovery deadline.
fn gate_released(home: &Path) -> Result<bool, &'static str> {
    let deadline = std::time::Instant::now() + CHILD_GUARD;
    loop {
        if gate_free(home)? {
            return Ok(true);
        }
        if std::time::Instant::now() >= deadline {
            return Ok(false);
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

fn recovery_class(report: &DeliveryReport) -> RecoveryPass {
    if *report == DeliveryReport::default() {
        RecoveryPass::Unchanged
    } else if report.delivered == [RUN_ID]
        && report.deferred.is_empty()
        && report.failed.is_empty()
        && report.warnings.is_empty()
    {
        RecoveryPass::Delivered
    } else {
        RecoveryPass::Other
    }
}

fn case_matches(case: &Case) -> Result<(bool, Value), &'static str> {
    let root = super::scheduler_conformance_tempdir()?;
    crate::daemon::ensure_private_coven_home(root.path()).map_err(|_| FAILED)?;
    let payload = serde_json::to_vec(case).map_err(|_| INVALID)?;
    let (mut process, receiver) = spawn_worker::<Handshake>(WORKER, payload, root.path())?;
    let ready = receiver.recv_timeout(CHILD_GUARD).map_err(|_| FAILED)??;
    let crash = case.boundary != Boundary::None;
    if ready.boundary != case.boundary || ready.state != if crash { "paused" } else { "delivered" }
    {
        return Err(FAILED);
    }
    let home = worker_home(root.path(), &ready.relative_home, HOME_PREFIX)?;
    let path = home.join("store.sqlite");
    // The paused worker is inside delivery, so it still holds the gate.
    let gate_held_at_pause = crash
        .then(|| gate_free(&home).map(|free| !free))
        .transpose()?;
    let pause = crash.then(|| observe_pause(&path)).transpose()?;
    let (transaction_open_at_pause, state_at_pause) = pause.unzip();
    // Before the commit, the worker's transaction is open and nothing it wrote
    // is visible; after it, the delivery is committed while the gate is held.
    let pause_consistent = pause.is_none_or(|(open, state)| {
        let before_commit = case.boundary.before_commit();
        open == before_commit
            && state
                == if before_commit {
                    DeliveryState::Pending
                } else {
                    DeliveryState::Delivered
                }
    });
    let status = process.finish(crash)?;
    let terminated = abruptly_terminated(&status);
    if crash != terminated || (!crash && !status.success()) {
        return Err(FAILED);
    }
    drop(process);
    let gate_released = gate_released(&home)?;
    let integrity_ok = raw_integrity_ok(&path)?;

    let base = base()?;
    let conn = crate::store::open_store(&path).map_err(|_| FAILED)?;
    let (state_after_exit, notice_after_exit) = state(&conn, RUN_ID)?;
    let notices_after_exit = notices_for(&conn, RUN_ID)?;
    // A committed delivery must name its one notice; an uncommitted one must
    // leave neither behind.
    let exit_consistent = match state_after_exit {
        DeliveryState::Delivered => {
            notice_after_exit.is_some()
                && notices_after_exit == notice_after_exit.into_iter().collect::<Vec<_>>()
        }
        DeliveryState::Pending => notice_after_exit.is_none() && notices_after_exit.is_empty(),
        DeliveryState::Failed => false,
    };
    let recovery =
        deliver_pending_results(&home, &conn, None, at(base, 240)).map_err(|_| FAILED)?;
    let recovery_pass = recovery_class(&recovery);
    let (final_state, final_notice) = state(&conn, RUN_ID)?;
    let final_notices = notices_for(&conn, RUN_ID)?;
    let record_names_notice = final_notice.is_some()
        && final_notices == final_notice.clone().into_iter().collect::<Vec<_>>();
    drop(conn);
    // A pass from a fresh connection after recovery must change nothing.
    let reopened = crate::store::open_store(&path).map_err(|_| FAILED)?;
    let replay =
        deliver_pending_results(&home, &reopened, None, at(base, 300)).map_err(|_| FAILED)?;
    let replay_is_noop = replay == DeliveryReport::default()
        && state(&reopened, RUN_ID)? == (final_state, final_notice)
        && notices_for(&reopened, RUN_ID)? == final_notices;
    drop(reopened);

    let observed = Expected {
        initial_outcome: if crash {
            InitialOutcome::Terminated
        } else {
            InitialOutcome::Delivered
        },
        state_after_exit,
        notices_after_exit: notices_after_exit.len() as u64,
        recovery_pass,
        final_state,
        final_notices: final_notices.len() as u64,
        integrity_ok,
        gate_released,
        replay_is_noop,
    };
    let passed = observed == case.expected
        && gate_held_at_pause.unwrap_or(true)
        && pause_consistent
        && exit_consistent
        && record_names_notice;
    Ok((
        passed,
        json!({
            "caseId": case.case_id,
            "boundary": case.boundary,
            "initialOutcome": observed.initial_outcome,
            "abruptlyTerminated": terminated,
            "gateHeldAtPause": gate_held_at_pause,
            "transactionOpenAtPause": transaction_open_at_pause,
            "stateAtPause": state_at_pause,
            "gateReleased": gate_released,
            "integrityOk": integrity_ok,
            "stateAfterExit": state_after_exit,
            "noticesAfterExit": observed.notices_after_exit,
            "exitConsistent": exit_consistent,
            "recoveryPass": recovery_pass,
            "finalState": final_state,
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
            "../../../../../conformance/automations/runner/result-delivery-process-crash-recovery.vectors.json"
        ))
        .unwrap()
    }

    #[test]
    fn delivery_crash_vectors_restate_each_boundary_outcome() {
        let vectors: VectorSet = serde_json::from_value(vectors()).unwrap();
        assert_eq!(vectors.schema_version, SCHEMA_VERSION);
        for case in &vectors.cases {
            validate(case).unwrap();
        }
        let boundaries: Vec<_> = vectors.cases.iter().map(|case| case.boundary).collect();
        assert_eq!(
            boundaries,
            [
                Boundary::AfterNotice,
                Boundary::AfterRecord,
                Boundary::AfterCommit,
                Boundary::None
            ]
        );
    }

    #[test]
    fn delivery_crash_vectors_require_each_boundary_and_the_control() {
        let original = vectors();
        let mut missing = original.clone();
        missing["cases"].as_array_mut().unwrap().pop();
        assert_eq!(evaluate(&missing).unwrap_err(), INVALID);
        // A second control under its own case id, replacing a boundary.
        let mut duplicate = original.clone();
        let mut control = duplicate["cases"][3].clone();
        control["caseId"] = json!("second-uninterrupted-delivery-control");
        duplicate["cases"][0] = control;
        assert_eq!(evaluate(&duplicate).unwrap_err(), INVALID);
        let mut duplicate_id = original;
        duplicate_id["cases"][1]["caseId"] = duplicate_id["cases"][0]["caseId"].clone();
        assert_eq!(evaluate(&duplicate_id).unwrap_err(), INVALID);
    }

    #[test]
    fn delivery_crash_vectors_refuse_malformed_or_weakened_inputs() {
        let original = vectors();
        for (case, field, value) in [
            (0, "initialOutcome", json!("delivered")),
            (0, "stateAfterExit", json!("delivered")),
            (0, "noticesAfterExit", json!(1)),
            (0, "recoveryPass", json!("unchanged")),
            (1, "finalNotices", json!(2)),
            (1, "finalState", json!("pending")),
            (2, "stateAfterExit", json!("pending")),
            (2, "recoveryPass", json!("delivered")),
            (2, "gateReleased", json!(false)),
            (3, "integrityOk", json!(false)),
            (3, "replayIsNoop", json!(false)),
        ] {
            let mut invalid = original.clone();
            invalid["cases"][case]["expected"][field] = value;
            assert_eq!(evaluate(&invalid).unwrap_err(), INVALID, "{case} {field}");
        }
        for (field, value) in [
            ("name", json!("Caller chosen")),
            ("status", json!("ACTIVE")),
            ("cwd", json!("/work/project")),
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
            ("boundary", json!("after_gate_release")),
            ("caseId", json!("")),
        ] {
            let mut invalid = original.clone();
            invalid["cases"][0][field] = value;
            assert_eq!(evaluate(&invalid).unwrap_err(), INVALID, "{field}");
        }
        let mut invalid = original;
        invalid["schemaVersion"] = json!("coven.automations.result-delivery-recovery-vectors.v1");
        assert_eq!(evaluate(&invalid).unwrap_err(), INVALID);
    }

    #[test]
    fn worker_refuses_invalid_or_weakened_input() {
        let mut case = vectors()["cases"][0].clone();
        case["storePath"] = json!("private-input");
        assert_eq!(worker(&case).unwrap_err(), INVALID);
        let mut case = vectors()["cases"][0].clone();
        case["expected"]["gateReleased"] = json!(false);
        assert_eq!(worker(&case).unwrap_err(), INVALID);
    }
}
