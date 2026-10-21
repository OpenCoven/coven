//! Disposable retry-timeout crash fixtures: a daemon worker claims one
//! scheduled occurrence whose runtime is unavailable for its first launch, so
//! the policy schedules a retry whose backoff outlasts the run's deadline. The
//! worker is killed while the retry waits or while its timeout enforcement
//! settles it. The parent restarts the daemon's scheduler over its store once
//! both the deadline and the backoff have passed, then runs a further pass.
//! Every launch goes through a no-op runtime; no runtime process starts.
use std::collections::BTreeSet;
use std::path::Path;

use chrono::{DateTime, Utc};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::automatic_retry_process_crash::{dispatch, UnavailableOnceRuntime, UNAVAILABLE};
use super::command_process_crash_recovery::{
    abruptly_terminated, announce, spawn_worker, CHILD_GUARD, SCRATCH_ENV,
};
use super::process_crash_fixture::{
    install_pause, open_raw, raw_integrity_ok, worker_home, write_transaction_open, Pause,
};
use super::result_delivery_recovery::{at, iso, BASE};
use super::run_launch_process_crash::{pass, restart, snapshot_tables, CountingRuntime, REFUSED};
use super::timeout_enforcement_process_crash::claim_slot;
use super::valid_case_id;
use crate::automations::runner::{enforce_run_timeouts, DispatchReport};

const INVALID: &str = "conformance vector is invalid";
const FAILED: &str = "conformance suite execution failed";
const SCHEMA_VERSION: &str = "coven.automations.retry-timeout-process-crash-recovery-vectors.v1";
const WORKER: &str = "retry-timeout-crash-worker";
const HOME_PREFIX: &str = "retry-timeout-crash-";
/// Seconds after `BASE`: the worker claims the 09:00 slot at 30, and its
/// launch finds the runtime unavailable. The run's one-minute deadline falls
/// at 90, but the policy's retry waits until 150. At 120 the worker's timeout
/// enforcement settles the waiting retry. The parent restarts the daemon at
/// 180, after both the deadline and the backoff, then runs a further pass that
/// must change nothing.
const CLAIM: i64 = 30;
const DEADLINE: i64 = 90;
const BACKOFF_END: i64 = 150;
const SETTLE: i64 = 120;
const RESTART: i64 = 180;
const REPLAY: i64 = 240;
/// The retry's, the occurrence's and the run's reason once the deadline
/// settles a waiting retry.
const TIMED_OUT: &str = "automation retry wait exceeded run timeout";

/// Where the worker can stop, in order: after the transaction that settles
/// the unavailable launch and schedules the retry commits; after each write of
/// the timeout settlement and after its commit; and the uninterrupted control.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
enum Boundary {
    AfterRetryCommit,
    AfterRetryTimedOut,
    AfterOccurrenceFailed,
    AfterRunFailed,
    AfterSettlementCommit,
    None,
}

impl Boundary {
    const ALL: [Self; 6] = [
        Self::AfterRetryCommit,
        Self::AfterRetryTimedOut,
        Self::AfterOccurrenceFailed,
        Self::AfterRunFailed,
        Self::AfterSettlementCommit,
        Self::None,
    ];

    /// The worker stops inside the timeout settlement's transaction.
    fn inside_transaction(self) -> bool {
        matches!(
            self,
            Self::AfterRetryTimedOut | Self::AfterOccurrenceFailed | Self::AfterRunFailed
        )
    }

    /// The timeout settlement committed before the worker was gone.
    fn settled(self) -> bool {
        matches!(self, Self::AfterSettlementCommit | Self::None)
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
    retry_after_exit: Retry,
    retry_after_restart: Retry,
    restart_is_noop: bool,
    retry_after_pass: Retry,
    pass_is_noop: bool,
    launches: u64,
    stops: u64,
    integrity_ok: bool,
}

#[derive(Deserialize, Serialize, PartialEq, Clone, Copy)]
#[serde(rename_all = "snake_case")]
enum InitialOutcome {
    Terminated,
    Settled,
}

/// The store's one occurrence with its run, attempts and session, read
/// together. Both classes have one occurrence, claimed only for the slot, one
/// run, two attempts, one session and no stop fence. The first attempt settled `failed` for the
/// unavailable runtime with no session bound, and its session `failed`. The
/// retry is the policy's, after the first attempt, bound to the occurrence's
/// claim count, and never bound to a session. The run is on no session, with
/// its deadline before the retry's backoff ends.
#[derive(Debug, Deserialize, Serialize, PartialEq, Clone, Copy)]
#[serde(rename_all = "snake_case")]
enum Retry {
    /// The retry `adopted` until its backoff ends, the run `running` and
    /// unfinished, and the occurrence `planned` with no lease and the
    /// runtime's answer as its reason.
    Scheduled,
    /// The deadline settled the waiting retry: the retry `timed_out` with the
    /// `timeout` class, the occurrence `failed` with no lease, and the run
    /// `failed` and finished, each with the timeout's reason where it has one.
    TimedOut,
    /// Anything else, including a partial transaction.
    Mixed,
}

/// What each boundary must produce. A vector restates it, so it cannot weaken
/// it.
fn expected_for(boundary: Boundary) -> Expected {
    // A restart after the deadline settles a retry still waiting, even once
    // its backoff has ended, and finds nothing to do once it has settled.
    let settled = boundary.settled();
    Expected {
        initial_outcome: if boundary == Boundary::None {
            InitialOutcome::Settled
        } else {
            InitialOutcome::Terminated
        },
        retry_after_exit: if settled {
            Retry::TimedOut
        } else {
            Retry::Scheduled
        },
        retry_after_restart: Retry::TimedOut,
        restart_is_noop: settled,
        retry_after_pass: Retry::TimedOut,
        pass_is_noop: true,
        launches: 0,
        stops: 0,
        integrity_ok: true,
    }
}

/// A routine whose policy retries an unavailable runtime once, two minutes
/// after the failure, within a one-minute deadline. The runner launches only
/// a routine with a working directory; a no-op runtime never touches it.
fn synthetic_definition() -> Value {
    json!({"schemaVersion":1,"id":"conformance-retry-timeout-crash",
        "name":"Conformance retry timeout crash","status":"ACTIVE",
        "rrule":"FREQ=DAILY;BYHOUR=9","timezone":"utc","misfire":"latest",
        "overlap":"forbid","timeoutMinutes":1,"runtime":"coven-code",
        "prompt":"Run the native retry timeout crash conformance probe.",
        "cwd":"/work/project",
        "retry":{"maxAttempts":2,"backoffPolicy":"fixed","backoffSeconds":120,
            "retryableClasses":["runtime_unavailable"]}})
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

/// Where the worker parks inside SQLite. The slot's launch commits first, and
/// the rejection that schedules the retry second. The timeout settlement then
/// marks the retry `timed_out` first, the occurrence `failed` next, and the
/// run `failed` last. No other transaction after the pause is installed
/// writes, so the WAL hook counts only these three commits.
fn pause_at(boundary: Boundary) -> Option<Pause<'static>> {
    match boundary {
        Boundary::AfterRetryCommit => Some(Pause::AfterCommits(2)),
        Boundary::AfterRetryTimedOut => Some(Pause::After(
            "AFTER UPDATE OF state ON main.automation_attempts
             WHEN NEW.state = 'timed_out'",
        )),
        Boundary::AfterOccurrenceFailed => Some(Pause::After(
            "AFTER UPDATE OF state ON main.automation_occurrences
             WHEN NEW.state = 'failed'",
        )),
        Boundary::AfterRunFailed => Some(Pause::After(
            "AFTER UPDATE OF status ON main.automation_runs
             WHEN NEW.status = 'failed'",
        )),
        Boundary::AfterSettlementCommit => Some(Pause::AfterCommits(3)),
        Boundary::None => None,
    }
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
    let leadership = claim_slot(&home, &case.definition, at(base, CLAIM))?;
    let conn = crate::store::open_store(&crate::api::store_path(&home)).map_err(|_| FAILED)?;
    if let Some(pause_at) = pause_at(case.boundary) {
        let handshake =
            json!({"state":"paused", "boundary":case.boundary, "relativeHome":relative_home});
        install_pause(&conn, pause_at, handshake)?;
    }
    let runtime = UnavailableOnceRuntime::default();
    // The slot's launch finds the runtime unavailable, so the dispatch settles
    // the first attempt and schedules the policy's retry.
    let scheduled = dispatch(&conn, &runtime, at(base, CLAIM), &leadership.fence())?;
    // Once the deadline passes with the retry still waiting for its backoff,
    // the pass's timeout enforcement settles it.
    let failures = enforce_run_timeouts(&conn, &runtime, at(base, SETTLE)).map_err(|_| FAILED)?;
    if case.boundary != Boundary::None
        || scheduled != DispatchReport::default()
        || !failures.is_empty()
        || runtime.launches() != 1
    {
        return Err(FAILED);
    }
    // Exits like a daemon that dies after a completed settlement: without the
    // graceful shutdown's reconciliation or leadership release.
    drop(conn);
    drop(leadership);
    announce(&json!({"state":"settled", "boundary":case.boundary, "relativeHome":relative_home}))
}

/// One attempt's row, as the classes read it.
struct Attempt {
    state: String,
    failure_class: Option<String>,
    reason: Option<String>,
    session_id: Option<String>,
    settled: bool,
    not_before: String,
    /// Bound to the occurrence's current claim count.
    on_claim: bool,
    classification: String,
    prior: Option<(i64, String)>,
}

/// Classifies the store's one occurrence with its run, attempts and session.
fn retry(conn: &Connection) -> Result<Retry, &'static str> {
    // The last count is of occurrences claimed only for the slot: a re-claim
    // for the retry raises the claim count.
    let counts: [i64; 6] = conn
        .query_row(
            "SELECT
                (SELECT COUNT(*) FROM automation_occurrences),
                (SELECT COUNT(*) FROM automation_runs),
                (SELECT COUNT(*) FROM automation_attempts),
                (SELECT COUNT(*) FROM sessions),
                (SELECT COUNT(*) FROM automation_stop_fences),
                (SELECT COUNT(*) FROM automation_occurrences WHERE attempt = 1)",
            [],
            |row| {
                Ok([
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                ])
            },
        )
        .map_err(|_| FAILED)?;
    if counts != [1, 1, 2, 1, 0, 1] {
        return Ok(Retry::Mixed);
    }
    type Row = (
        String,
        String,
        Option<String>,
        String,
        Option<String>,
        bool,
        bool,
        bool,
    );
    let (occurrence, lease, occurrence_reason, run, run_session, finished, exit_code, on_deadline): Row = conn
        .query_row(
            "SELECT o.state,
                    CASE
                        WHEN o.lease_owner = 'daemon' AND o.lease_expires_at IS NOT NULL
                            THEN 'daemon'
                        WHEN o.lease_owner IS NULL AND o.lease_expires_at IS NULL THEN 'none'
                        ELSE 'other'
                    END,
                    o.failure_reason, r.status, r.session_id, r.finished_at IS NOT NULL,
                    r.exit_code IS NOT NULL, r.timeout_at IS ?1
             FROM automation_occurrences AS o
             JOIN automation_runs AS r ON r.occurrence_id = o.id",
            [iso(at(base()?, DEADLINE))],
            |row| {
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
            },
        )
        .map_err(|_| FAILED)?;
    let session: String = conn
        .query_row("SELECT status FROM sessions", [], |row| row.get(0))
        .map_err(|_| FAILED)?;
    let attempts: Vec<Attempt> = conn
        .prepare(
            "SELECT a.state, a.failure_class, a.state_reason, a.session_id,
                    a.settled_at IS NOT NULL, a.not_before,
                    a.occurrence_fence_generation = o.attempt, a.retry_classification,
                    a.prior_attempt_number, a.prior_disposition
             FROM automation_attempts AS a
             JOIN automation_occurrences AS o ON o.id = a.occurrence_id
             ORDER BY a.attempt_number",
        )
        .and_then(|mut statement| {
            statement
                .query_map([], |row| {
                    let prior: Option<i64> = row.get(8)?;
                    let disposition: Option<String> = row.get(9)?;
                    Ok(Attempt {
                        state: row.get(0)?,
                        failure_class: row.get(1)?,
                        reason: row.get(2)?,
                        session_id: row.get(3)?,
                        settled: row.get(4)?,
                        not_before: row.get(5)?,
                        on_claim: row.get(6)?,
                        classification: row.get(7)?,
                        prior: prior.zip(disposition),
                    })
                })?
                .collect()
        })
        .map_err(|_| FAILED)?;
    let [first, retry] = attempts.as_slice() else {
        return Ok(Retry::Mixed);
    };
    // The slot's attempt settled for the unavailable runtime and its session
    // ended. The retry follows it after the backoff, bound to the claim, and
    // no session ever reaches the retry or the run.
    if first.classification != "initial"
        || first.prior.is_some()
        || first.session_id.is_some()
        || first.state != "failed"
        || first.failure_class.as_deref() != Some("runtime_unavailable")
        || first.reason.as_deref() != Some(UNAVAILABLE)
        || !first.settled
        || session != "failed"
        || retry.classification != "automatic_retry"
        || retry.prior != Some((1, "failed".to_string()))
        || retry.not_before != iso(at(base()?, BACKOFF_END))
        || !retry.on_claim
        || retry.session_id.is_some()
        || run_session.is_some()
        || !on_deadline
        || exit_code
        || lease != "none"
    {
        return Ok(Retry::Mixed);
    }
    Ok(
        match (
            retry.state.as_str(),
            retry.failure_class.as_deref(),
            retry.reason.as_deref(),
            retry.settled,
            occurrence.as_str(),
            occurrence_reason.as_deref(),
            run.as_str(),
            finished,
        ) {
            ("adopted", None, None, false, "planned", Some(UNAVAILABLE), "running", false) => {
                Retry::Scheduled
            }
            (
                "timed_out",
                Some("timeout"),
                Some(TIMED_OUT),
                true,
                "failed",
                Some(TIMED_OUT),
                "failed",
                true,
            ) => Retry::TimedOut,
            _ => Retry::Mixed,
        },
    )
}

/// Every row the dispatch, the timeout settlement or a restart can write, for
/// an exact before/after comparison.
fn snapshot(conn: &Connection) -> Result<Vec<Vec<rusqlite::types::Value>>, &'static str> {
    snapshot_tables(
        conn,
        &[
            "automation_occurrences",
            "automation_runs",
            "automation_attempts",
            "automation_events",
            "sessions",
            "automation_stop_fences",
        ],
    )
}

/// What the restarted daemon and its further pass did with the store the
/// worker left.
struct Restarted {
    retry_after_restart: Retry,
    restart_is_noop: bool,
    retry_after_pass: Retry,
    pass_is_noop: bool,
    launches: u64,
    stops: u64,
}

fn recover(home: &Path, path: &Path, base: DateTime<Utc>) -> Result<Restarted, &'static str> {
    // One runtime counts every stop and launch for the restart and the pass.
    let runtime = CountingRuntime::default();
    let before_restart = snapshot(&crate::store::open_store(path).map_err(|_| FAILED)?)?;
    let leadership = restart(home, &runtime, at(base, RESTART))?;
    let conn = crate::store::open_store(path).map_err(|_| FAILED)?;
    let retry_after_restart = retry(&conn)?;
    let restart_is_noop = snapshot(&conn)? == before_restart;
    let before_pass = snapshot(&conn)?;
    drop(conn);
    // A further pass must change nothing and issue no stop or launch.
    pass(home, &runtime, &leadership, at(base, REPLAY), "deadline")?;
    let conn = crate::store::open_store(path).map_err(|_| FAILED)?;
    let retry_after_pass = retry(&conn)?;
    let pass_is_noop = snapshot(&conn)? == before_pass;
    drop(conn);
    drop(leadership);
    Ok(Restarted {
        retry_after_restart,
        restart_is_noop,
        retry_after_pass,
        pass_is_noop,
        launches: runtime.launches(),
        stops: runtime.stops(),
    })
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
    // Every boundary, once each.
    if vectors.schema_version != SCHEMA_VERSION || vectors.cases.len() != Boundary::ALL.len() {
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
/// worker's write transaction is still open, and what it has committed.
fn observe_pause(path: &Path) -> Result<(bool, Retry), &'static str> {
    let probe = open_raw(path)?;
    let open = write_transaction_open(&probe)?;
    Ok((open, retry(&probe)?))
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
    let path = crate::api::store_path(&home);
    let pause = crash.then(|| observe_pause(&path)).transpose()?;
    let pause_consistent = pause.is_none_or(|(open, retry)| {
        open == case.boundary.inside_transaction() && retry == case.expected.retry_after_exit
    });
    let status = process.finish(crash)?;
    let terminated = abruptly_terminated(&status);
    if crash != terminated || (!crash && !status.success()) {
        return Err(FAILED);
    }
    drop(process);
    let integrity_ok = raw_integrity_ok(&path)?;

    let retry_after_exit = retry(&crate::store::open_store(&path).map_err(|_| FAILED)?)?;
    let recovery = match recover(&home, &path, base()?) {
        Ok(recovery) => Some(recovery),
        Err(error) if error == REFUSED => None,
        Err(error) => return Err(error),
    };
    let recovery_refused = recovery.is_none();
    let observed = Expected {
        initial_outcome: if terminated {
            InitialOutcome::Terminated
        } else {
            InitialOutcome::Settled
        },
        retry_after_exit,
        retry_after_restart: recovery
            .as_ref()
            .map_or(Retry::Mixed, |recovery| recovery.retry_after_restart),
        restart_is_noop: recovery
            .as_ref()
            .is_some_and(|recovery| recovery.restart_is_noop),
        retry_after_pass: recovery
            .as_ref()
            .map_or(Retry::Mixed, |recovery| recovery.retry_after_pass),
        pass_is_noop: recovery
            .as_ref()
            .is_some_and(|recovery| recovery.pass_is_noop),
        launches: recovery.as_ref().map_or(0, |recovery| recovery.launches),
        stops: recovery.as_ref().map_or(0, |recovery| recovery.stops),
        integrity_ok,
    };
    let passed = observed == case.expected && pause_consistent && !recovery_refused;
    let (transaction_open_at_pause, retry_at_pause) = match pause {
        Some((open, retry)) => (Some(open), Some(retry)),
        None => (None, None),
    };
    Ok((
        passed,
        json!({
            "caseId": case.case_id,
            "boundary": case.boundary,
            "initialOutcome": observed.initial_outcome,
            "abruptlyTerminated": terminated,
            "transactionOpenAtPause": transaction_open_at_pause,
            "retryAtPause": retry_at_pause,
            "pauseConsistent": pause_consistent,
            "integrityOk": integrity_ok,
            "retryAfterExit": observed.retry_after_exit,
            "recoveryRefused": recovery_refused,
            "retryAfterRestart": observed.retry_after_restart,
            "restartIsNoop": observed.restart_is_noop,
            "retryAfterPass": observed.retry_after_pass,
            "passIsNoop": observed.pass_is_noop,
            "launches": observed.launches,
            "stops": observed.stops,
        }),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vectors() -> Value {
        serde_json::from_str(include_str!(
            "../../../../../conformance/automations/runner/retry-timeout-process-crash-recovery.vectors.json"
        ))
        .unwrap()
    }

    #[test]
    fn retry_timeout_crash_vectors_restate_each_boundary_outcome() {
        let vectors: VectorSet = serde_json::from_value(vectors()).unwrap();
        assert_eq!(vectors.schema_version, SCHEMA_VERSION);
        for case in &vectors.cases {
            validate(case).unwrap();
        }
        let boundaries: Vec<_> = vectors.cases.iter().map(|case| case.boundary).collect();
        assert_eq!(boundaries, Boundary::ALL);
    }

    #[test]
    fn retry_timeout_crash_vectors_require_each_boundary_and_the_control() {
        let original = vectors();
        let mut missing = original.clone();
        missing["cases"].as_array_mut().unwrap().pop();
        assert_eq!(evaluate(&missing).unwrap_err(), INVALID);
        // A second control under its own case id, replacing a boundary.
        let mut duplicate = original.clone();
        let mut control = duplicate["cases"][5].clone();
        control["caseId"] = json!("second-uninterrupted-retry-timeout-control");
        duplicate["cases"][0] = control;
        assert_eq!(evaluate(&duplicate).unwrap_err(), INVALID);
        let mut duplicate_id = original;
        duplicate_id["cases"][1]["caseId"] = duplicate_id["cases"][0]["caseId"].clone();
        assert_eq!(evaluate(&duplicate_id).unwrap_err(), INVALID);
    }

    #[test]
    fn retry_timeout_crash_vectors_refuse_malformed_or_weakened_inputs() {
        let original = vectors();
        for (case, field, value) in [
            (0, "retryAfterExit", json!("timed_out")),
            (0, "retryAfterRestart", json!("scheduled")),
            (0, "restartIsNoop", json!(true)),
            (0, "launches", json!(1)),
            (1, "retryAfterExit", json!("mixed")),
            (1, "restartIsNoop", json!(true)),
            (2, "retryAfterPass", json!("scheduled")),
            (3, "retryAfterExit", json!("timed_out")),
            (3, "passIsNoop", json!(false)),
            (4, "retryAfterExit", json!("scheduled")),
            (4, "restartIsNoop", json!(false)),
            (4, "stops", json!(1)),
            (5, "initialOutcome", json!("terminated")),
            (5, "integrityOk", json!(false)),
            (5, "launches", json!(1)),
        ] {
            let mut invalid = original.clone();
            invalid["cases"][case]["expected"][field] = value;
            assert_eq!(evaluate(&invalid).unwrap_err(), INVALID, "{case} {field}");
        }
        for (field, value) in [
            ("name", json!("Caller chosen")),
            ("status", json!("PAUSED")),
            ("cwd", json!("/private-input")),
            ("timeoutMinutes", json!(30)),
            ("retry", json!({"maxAttempts": 1})),
        ] {
            let mut invalid = original.clone();
            invalid["cases"][0]["definition"][field] = value;
            assert_eq!(
                evaluate(&invalid).unwrap_err(),
                INVALID,
                "definition {field}"
            );
        }
        let mut invalid = original.clone();
        invalid["cases"][0]["definition"]["retry"]["backoffSeconds"] = json!(30);
        assert_eq!(evaluate(&invalid).unwrap_err(), INVALID, "backoffSeconds");
        for (field, value) in [
            ("storePath", json!("private-input")),
            ("runId", json!("private-input")),
            ("timeoutAt", json!("2099-10-20T09:00:31.000Z")),
            ("boundary", json!("after_settlement")),
            ("caseId", json!("")),
        ] {
            let mut invalid = original.clone();
            invalid["cases"][0][field] = value;
            assert_eq!(evaluate(&invalid).unwrap_err(), INVALID, "{field}");
        }
        let mut invalid = original;
        invalid["schemaVersion"] =
            json!("coven.automations.automatic-retry-process-crash-recovery-vectors.v1");
        assert_eq!(evaluate(&invalid).unwrap_err(), INVALID);
    }

    /// A home whose routine's 09:00 slot the daemon's scheduler has claimed
    /// and whose launch found the runtime unavailable, so the policy's retry
    /// waits, with a connection to its store.
    fn scheduled_store() -> (
        tempfile::TempDir,
        crate::automations::leadership::SchedulerLeadership,
        Connection,
    ) {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        crate::daemon::ensure_private_coven_home(&home).unwrap();
        let base = base().unwrap();
        let leadership = claim_slot(&home, &synthetic_definition(), at(base, CLAIM)).unwrap();
        let conn = crate::store::open_store(&crate::api::store_path(&home)).unwrap();
        let runtime = UnavailableOnceRuntime::default();
        assert_eq!(
            dispatch(&conn, &runtime, at(base, CLAIM), &leadership.fence()).unwrap(),
            DispatchReport::default()
        );
        assert_eq!(retry(&conn).unwrap(), Retry::Scheduled);
        (temp, leadership, conn)
    }

    #[test]
    fn the_deadline_settles_the_waiting_retry_and_it_never_launches() {
        let (_temp, leadership, conn) = scheduled_store();
        let (fence, base) = (leadership.fence(), base().unwrap());
        let runtime = UnavailableOnceRuntime::default();
        let tick = |second| {
            crate::automations::occurrences::tick_with_scheduler_fence(
                &conn,
                at(base, second),
                &fence,
            )
            .unwrap()
        };
        // Before the deadline, the retry keeps waiting.
        assert!(
            enforce_run_timeouts(&conn, &runtime, at(base, DEADLINE - 1))
                .unwrap()
                .is_empty()
        );
        assert_eq!(retry(&conn).unwrap(), Retry::Scheduled);
        // Once its backoff has ended past the deadline, the scheduler still
        // does not re-claim it.
        assert!(tick(BACKOFF_END).claimed.is_empty());
        assert_eq!(retry(&conn).unwrap(), Retry::Scheduled);
        // Enforcement settles the waiting retry.
        assert!(enforce_run_timeouts(&conn, &runtime, at(base, BACKOFF_END))
            .unwrap()
            .is_empty());
        assert_eq!(retry(&conn).unwrap(), Retry::TimedOut);
        // Nothing re-claims or launches it afterwards.
        assert!(tick(REPLAY).claimed.is_empty());
        assert_eq!(
            dispatch(&conn, &runtime, at(base, REPLAY), &fence).unwrap(),
            DispatchReport::default()
        );
        assert_eq!(retry(&conn).unwrap(), Retry::TimedOut);
        assert_eq!(runtime.launches(), 0);
    }

    #[test]
    fn the_settlement_is_whole_or_a_state_no_boundary_leaves() {
        let left: Vec<Retry> = Boundary::ALL
            .iter()
            .map(|boundary| expected_for(*boundary).retry_after_exit)
            .collect();
        let (_temp, _leadership, conn) = scheduled_store();
        let step = |sql: &str, whole: Option<Retry>| {
            conn.execute_batch(sql).unwrap();
            let now = retry(&conn).unwrap();
            match whole {
                Some(whole) => assert_eq!(now, whole, "{sql}"),
                None => assert!(!left.contains(&now), "{sql}: {now:?}"),
            }
        };
        let base = base().unwrap();
        let (settle_at, lease) = (iso(at(base, SETTLE)), iso(at(base, RESTART + 600)));
        for reclaimed in [false, true] {
            // The settlement's writes in the scheduler's order. The store
            // holds one occurrence and run; the writes read them.
            conn.execute_batch("BEGIN").unwrap();
            if reclaimed {
                // A re-claim for the retry raises the occurrence's claim
                // count, so even a whole settlement after it is a state no
                // boundary leaves.
                step(
                    &format!(
                        "UPDATE automation_occurrences
                         SET state = 'claimed', lease_owner = 'daemon',
                             lease_expires_at = '{lease}', attempt = attempt + 1"
                    ),
                    None,
                );
                step(
                    "UPDATE automation_attempts
                     SET occurrence_fence_generation = (SELECT attempt FROM automation_occurrences)
                     WHERE attempt_number = 2",
                    None,
                );
            }
            step(
                &format!(
                    "UPDATE automation_attempts
                     SET state = 'timed_out', failure_class = 'timeout',
                         state_reason = '{TIMED_OUT}', settled_at = '{settle_at}'
                     WHERE state = 'adopted'"
                ),
                None,
            );
            step(
                &format!(
                    "UPDATE automation_occurrences
                     SET state = 'failed', failure_reason = '{TIMED_OUT}', lease_owner = NULL,
                         lease_expires_at = NULL, updated_at = '{settle_at}'"
                ),
                None,
            );
            step(
                &format!(
                    "UPDATE automation_runs SET status = 'failed', finished_at = '{settle_at}'"
                ),
                (!reclaimed).then_some(Retry::TimedOut),
            );
            conn.execute_batch("ROLLBACK").unwrap();
            assert_eq!(retry(&conn).unwrap(), Retry::Scheduled);
        }
    }

    #[test]
    fn worker_refuses_invalid_or_weakened_input() {
        let mut case = vectors()["cases"][0].clone();
        case["storePath"] = json!("private-input");
        assert_eq!(worker(&case).unwrap_err(), INVALID);
        let mut case = vectors()["cases"][1].clone();
        case["expected"]["restartIsNoop"] = json!(true);
        assert_eq!(worker(&case).unwrap_err(), INVALID);
        let mut case = vectors()["cases"][5].clone();
        case["boundary"] = json!("after_settlement_commit");
        assert_eq!(worker(&case).unwrap_err(), INVALID);
    }
}
