//! Disposable held-settlement crash fixtures: a daemon worker is killed while
//! its scheduler holds a scheduled run whose session failed before its
//! deadline for an operator retry, or while it settles that held run once the
//! deadline passes. The parent restarts the daemon's scheduler, then runs
//! passes after the deadline. Both launch through no-op runtimes; no runtime
//! process starts.
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
use super::result_delivery_recovery::{at, iso, BASE};
use super::run_launch_process_crash::{pass, restart, snapshot_tables, CountingRuntime, REFUSED};
use super::timeout_enforcement_process_crash::launch_slot;
use super::valid_case_id;
use crate::automations::runner::{settle_finished_runs, SettlementReport};

const INVALID: &str = "conformance vector is invalid";
const FAILED: &str = "conformance suite execution failed";
const SCHEMA_VERSION: &str = "coven.automations.held-settlement-process-crash-recovery-vectors.v1";
const WORKER: &str = "held-settlement-crash-worker";
const HOME_PREFIX: &str = "held-crash-";
/// Seconds after `BASE`: the worker claims and launches the 09:00 slot, whose
/// one-minute deadline passes at 90 seconds. Its session fails at 40, the
/// daemon's pass holds the run at 50, and its pass after the deadline settles
/// it at 100. The parent restarts the daemon inside the hold window, 10
/// seconds after the hold, when the worker died before that settlement began,
/// and 10 seconds after the settlement otherwise. It then runs a pass after
/// the deadline and a further pass that must change nothing.
const CLAIM: i64 = 30;
const END: i64 = 40;
const HOLD: i64 = 50;
const HOLD_RESTART: i64 = 60;
const DEADLINE: i64 = 100;
const DEADLINE_RESTART: i64 = 110;
const PASS: i64 = 120;
const REPLAY: i64 = 200;
/// The attempt's reason once the hold has settled it.
const HELD: &str =
    "session failed (exit code 1); held for an operator retry until the run's deadline";
/// The occurrence's reason once the deadline has settled the held run.
const UNRETRIED: &str = "no operator retry was opened before the run's deadline";

/// Where the worker can stop, in order: after the session's failure commits,
/// inside the hold's transaction and after its commit, after each write of
/// the deadline's settlement and after its commit, and the uninterrupted
/// control.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
enum Boundary {
    AfterSessionEnd,
    AfterHoldWrite,
    AfterHoldCommit,
    AfterOccurrenceSettle,
    AfterRunFinish,
    AfterSettleCommit,
    None,
}

impl Boundary {
    const ALL: [Self; 7] = [
        Self::AfterSessionEnd,
        Self::AfterHoldWrite,
        Self::AfterHoldCommit,
        Self::AfterOccurrenceSettle,
        Self::AfterRunFinish,
        Self::AfterSettleCommit,
        Self::None,
    ];

    /// The worker stops inside a write transaction.
    fn inside_transaction(self) -> bool {
        matches!(
            self,
            Self::AfterHoldWrite | Self::AfterOccurrenceSettle | Self::AfterRunFinish
        )
    }

    /// What the store holds once the worker is gone, and when.
    fn stage(self) -> Stage {
        match self {
            Self::AfterSessionEnd | Self::AfterHoldWrite => Stage::Ended,
            Self::AfterHoldCommit => Stage::Held,
            Self::AfterOccurrenceSettle | Self::AfterRunFinish => Stage::Overdue,
            Self::AfterSettleCommit | Self::None => Stage::Settled,
        }
    }
}

/// How far the run got before the worker died: its session ended; the hold
/// committed, before the deadline; the hold committed, with the deadline's
/// settlement under way; the settlement committed.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Stage {
    Ended,
    Held,
    Overdue,
    Settled,
}

impl Stage {
    /// When the parent restarts the daemon: inside the hold window if the
    /// deadline's settlement had not begun, after the deadline otherwise.
    fn restart_at(self) -> i64 {
        match self {
            Self::Ended | Self::Held => HOLD_RESTART,
            Self::Overdue | Self::Settled => DEADLINE_RESTART,
        }
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
    run_after_exit: Run,
    run_after_restart: Run,
    restart_is_noop: bool,
    run_after_pass: Run,
    pass_is_noop: bool,
    stops: u64,
    launches: u64,
    integrity_ok: bool,
    replay_is_noop: bool,
}

#[derive(Deserialize, Serialize, PartialEq, Clone, Copy)]
#[serde(rename_all = "snake_case")]
enum InitialOutcome {
    Terminated,
    Responded,
}

/// The store's one run with its occurrence, its one attempt, that attempt's
/// session and any stop fence, read together.
#[derive(Debug, Deserialize, Serialize, PartialEq, Clone, Copy)]
#[serde(rename_all = "snake_case")]
enum Run {
    /// Launched: the occurrence `running` under the daemon's lease, the run
    /// `running` and unfinished on the attempt's `running` session, the
    /// attempt `started`, and no stop fence.
    Running,
    /// The same, with the session ended `failed` (exit code 1).
    Ended,
    /// Held for an operator retry: the attempt settled `failed` for the
    /// hold before the deadline, with no failure class; the run and the
    /// occurrence still `running` under the daemon's lease on the `failed`
    /// session.
    Held,
    /// The held attempt as the hold settled it, the run finished `failed`
    /// with the session's exit code at or after the deadline and the
    /// occurrence `failed` with no lease because no retry was opened before
    /// it, and no stop fence.
    Failed,
    /// Anything else, including a partial transaction.
    Mixed,
}

/// What each boundary must produce. A vector restates it, so it cannot weaken
/// it.
fn expected_for(boundary: Boundary) -> Expected {
    let stage = boundary.stage();
    let run_after_exit = match stage {
        Stage::Ended => Run::Ended,
        Stage::Held | Stage::Overdue => Run::Held,
        Stage::Settled => Run::Failed,
    };
    // Inside the hold window, the restart's startup pass holds an ended run
    // and leaves a held one waiting. After the deadline, it settles a held run.
    let run_after_restart = match stage {
        Stage::Ended | Stage::Held => Run::Held,
        Stage::Overdue | Stage::Settled => Run::Failed,
    };
    Expected {
        initial_outcome: if boundary == Boundary::None {
            InitialOutcome::Responded
        } else {
            InitialOutcome::Terminated
        },
        run_after_exit,
        run_after_restart,
        restart_is_noop: matches!(stage, Stage::Held | Stage::Settled),
        // The pass after the deadline settles every held run once, and finds
        // nothing to do once the restart has.
        run_after_pass: Run::Failed,
        pass_is_noop: matches!(stage, Stage::Overdue | Stage::Settled),
        stops: 0,
        launches: 0,
        integrity_ok: true,
        replay_is_noop: true,
    }
}

/// A routine with a one-minute deadline and no automatic retry, so a plain
/// failure holds its run for an operator retry. The runner launches only a
/// routine with a working directory; a no-op runtime never touches it.
fn synthetic_definition() -> Value {
    json!({"schemaVersion":1,"id":"conformance-held-crash",
        "name":"Conformance held crash","status":"ACTIVE",
        "rrule":"FREQ=DAILY;BYHOUR=9","timezone":"utc","misfire":"latest",
        "overlap":"forbid","timeoutMinutes":1,"runtime":"coven-code",
        "prompt":"Run the native held settlement crash conformance probe.",
        "cwd":"/work/project"})
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

/// Where the worker parks inside SQLite. The session's failure commits on its
/// own. The hold settles the attempt in one transaction; the deadline's
/// settlement settles the occurrence and finishes the run in another. Nothing
/// else commits after the pause is installed.
fn pause_at(boundary: Boundary) -> Option<Pause<'static>> {
    match boundary {
        Boundary::AfterSessionEnd => Some(Pause::AfterCommits(1)),
        Boundary::AfterHoldWrite => Some(Pause::After(
            "AFTER UPDATE OF state ON main.automation_attempts
             WHEN OLD.state IS NOT NEW.state",
        )),
        Boundary::AfterHoldCommit => Some(Pause::AfterCommits(2)),
        Boundary::AfterOccurrenceSettle => Some(Pause::After(
            "AFTER UPDATE OF state ON main.automation_occurrences
             WHEN OLD.state IS NOT NEW.state",
        )),
        Boundary::AfterRunFinish => Some(Pause::After(
            "AFTER UPDATE OF status ON main.automation_runs
             WHEN OLD.status IS NOT NEW.status",
        )),
        Boundary::AfterSettleCommit => Some(Pause::AfterCommits(3)),
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
    let leadership = launch_slot(&home, &case.definition, at(base, CLAIM))?;
    let conn = crate::store::open_store(&crate::api::store_path(&home)).map_err(|_| FAILED)?;
    if run(&conn)? != Run::Running {
        return Err(FAILED);
    }
    let session_id: String = conn
        .query_row("SELECT session_id FROM automation_runs", [], |row| {
            row.get(0)
        })
        .map_err(|_| FAILED)?;
    if let Some(pause_at) = pause_at(case.boundary) {
        let handshake =
            json!({"state":"paused", "boundary":case.boundary, "relativeHome":relative_home});
        install_pause(&conn, pause_at, handshake)?;
    }
    // The runtime records that the session failed before the run's deadline.
    let ended = crate::store::update_session_terminal_if_active(
        &conn,
        &session_id,
        "failed",
        Some(1),
        &iso(at(base, END)),
    )
    .map_err(|_| FAILED)?;
    if !ended {
        return Err(FAILED);
    }
    // The daemon's pass inside the hold window holds the run for an operator
    // retry, and its pass after the deadline settles the run.
    let held = settle_finished_runs(&conn, at(base, HOLD)).map_err(|_| FAILED)?;
    let settled = settle_finished_runs(&conn, at(base, DEADLINE)).map_err(|_| FAILED)?;
    if case.boundary != Boundary::None
        || held != SettlementReport::default()
        || settled != failed_one()
        || run(&conn)? != Run::Failed
    {
        return Err(FAILED);
    }
    // Exits like a daemon that dies after the pass: without the graceful
    // shutdown's reconciliation or the release of its scheduler authority.
    drop(conn);
    drop(leadership);
    announce(&json!({"state":"settled", "boundary":case.boundary, "relativeHome":relative_home}))
}

fn failed_one() -> SettlementReport {
    SettlementReport {
        failed: 1,
        ..SettlementReport::default()
    }
}

/// Classifies the store's one run with its occurrence, attempt, session and
/// any stop fence.
fn run(conn: &Connection) -> Result<Run, &'static str> {
    let (occurrences, runs, attempts, sessions, fences): (i64, i64, i64, i64, i64) = conn
        .query_row(
            "SELECT
                (SELECT COUNT(*) FROM automation_occurrences),
                (SELECT COUNT(*) FROM automation_runs),
                (SELECT COUNT(*) FROM automation_attempts),
                (SELECT COUNT(*) FROM sessions),
                (SELECT COUNT(*) FROM automation_stop_fences)",
            [],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )
        .map_err(|_| FAILED)?;
    if (occurrences, runs, attempts, sessions, fences) != (1, 1, 1, 1, 0) {
        return Ok(Run::Mixed);
    }
    // The five states, the attempt's failure class, five facts, the run's and
    // the session's exit codes and the two settlement reasons.
    type Row = (
        Vec<String>,
        Option<String>,
        [bool; 5],
        [Option<i64>; 2],
        [Option<String>; 2],
    );
    let row: Option<Row> = conn
        .query_row(
            "SELECT o.state,
                    CASE
                        WHEN o.lease_owner = 'daemon' AND o.lease_expires_at IS NOT NULL
                            THEN 'daemon'
                        WHEN o.lease_owner IS NULL AND o.lease_expires_at IS NULL THEN 'none'
                        ELSE 'other'
                    END,
                    r.status, a.state, s.status,
                    a.failure_class,
                    r.session_id IS a.session_id,
                    a.settled_at IS NOT NULL, COALESCE(a.settled_at < r.timeout_at, 0),
                    r.finished_at IS NOT NULL, COALESCE(r.finished_at >= r.timeout_at, 0),
                    r.exit_code, s.exit_code,
                    a.state_reason, o.failure_reason
             FROM automation_occurrences AS o
             JOIN automation_runs AS r ON r.occurrence_id = o.id
             JOIN automation_attempts AS a ON a.run_id = r.id AND a.attempt_number = 1
             JOIN sessions AS s ON s.id = a.session_id",
            [],
            |row| {
                Ok((
                    (0..5)
                        .map(|column| row.get(column))
                        .collect::<rusqlite::Result<_>>()?,
                    row.get(5)?,
                    [
                        row.get(6)?,
                        row.get(7)?,
                        row.get(8)?,
                        row.get(9)?,
                        row.get(10)?,
                    ],
                    [row.get(11)?, row.get(12)?],
                    [row.get(13)?, row.get(14)?],
                ))
            },
        )
        .optional()
        .map_err(|_| FAILED)?;
    let Some((
        states,
        class,
        [on_session, settled, held_in_time, finished, finished_late],
        [run_exit, session_exit],
        [attempt, occurrence],
    )) = row
    else {
        return Ok(Run::Mixed);
    };
    // A settled attempt settled before the deadline, and a finished run
    // finished at or after it.
    if !on_session || class.is_some() || settled != held_in_time || finished != finished_late {
        return Ok(Run::Mixed);
    }
    let states: Vec<&str> = states.iter().map(String::as_str).collect();
    let Ok([occurrence_state, lease, run, attempt_state, session]) = <[&str; 5]>::try_from(states)
    else {
        return Ok(Run::Mixed);
    };
    Ok(
        match (
            occurrence_state,
            lease,
            run,
            attempt_state,
            session,
            settled,
            finished,
            run_exit,
            session_exit,
            attempt.as_deref(),
            occurrence.as_deref(),
        ) {
            (
                "running",
                "daemon",
                "running",
                "started",
                "running",
                false,
                false,
                None,
                None,
                None,
                None,
            ) => Run::Running,
            (
                "running",
                "daemon",
                "running",
                "started",
                "failed",
                false,
                false,
                None,
                Some(1),
                None,
                None,
            ) => Run::Ended,
            (
                "running",
                "daemon",
                "running",
                "failed",
                "failed",
                true,
                false,
                None,
                Some(1),
                Some(HELD),
                None,
            ) => Run::Held,
            (
                "failed",
                "none",
                "failed",
                "failed",
                "failed",
                true,
                true,
                Some(1),
                Some(1),
                Some(HELD),
                Some(UNRETRIED),
            ) => Run::Failed,
            _ => Run::Mixed,
        },
    )
}

/// Every row the hold, the settlement, its recovery or a restart can write,
/// for an exact before/after comparison.
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

/// What the restarted daemon and its passes did with the store the worker
/// left.
struct Restarted {
    run_after_restart: Run,
    restart_is_noop: bool,
    run_after_pass: Run,
    pass_is_noop: bool,
    stops: u64,
    launches: u64,
    replay_is_noop: bool,
}

fn recover(
    home: &Path,
    path: &Path,
    base: DateTime<Utc>,
    stage: Stage,
) -> Result<Restarted, &'static str> {
    // One runtime counts every stop and launch for the restart and each pass.
    let runtime = CountingRuntime::default();
    let before_restart = snapshot(&crate::store::open_store(path).map_err(|_| FAILED)?)?;
    let leadership = restart(home, &runtime, at(base, stage.restart_at()))?;
    let conn = crate::store::open_store(path).map_err(|_| FAILED)?;
    let run_after_restart = run(&conn)?;
    let restart_is_noop = snapshot(&conn)? == before_restart;
    let before_pass = snapshot(&conn)?;
    drop(conn);
    pass(home, &runtime, &leadership, at(base, PASS), "deadline")?;
    let conn = crate::store::open_store(path).map_err(|_| FAILED)?;
    let run_after_pass = run(&conn)?;
    let pass_is_noop = snapshot(&conn)? == before_pass;
    let stops = runtime.stops();
    let launches = runtime.launches();
    let before_replay = snapshot(&conn)?;
    drop(conn);
    // A further pass must change nothing and issue no stop or launch.
    pass(home, &runtime, &leadership, at(base, REPLAY), "deadline")?;
    let conn = crate::store::open_store(path).map_err(|_| FAILED)?;
    let replay_is_noop = snapshot(&conn)? == before_replay
        && runtime.stops() == stops
        && runtime.launches() == launches;
    drop(conn);
    drop(leadership);
    Ok(Restarted {
        run_after_restart,
        restart_is_noop,
        run_after_pass,
        pass_is_noop,
        stops,
        launches,
        replay_is_noop,
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
fn observe_pause(path: &Path) -> Result<(bool, Run), &'static str> {
    let probe = open_raw(path)?;
    let open = write_transaction_open(&probe)?;
    Ok((open, run(&probe)?))
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
    let pause_consistent = pause.is_none_or(|(open, run)| {
        open == case.boundary.inside_transaction() && run == case.expected.run_after_exit
    });
    let status = process.finish(crash)?;
    let terminated = abruptly_terminated(&status);
    if crash != terminated || (!crash && !status.success()) {
        return Err(FAILED);
    }
    drop(process);
    let integrity_ok = raw_integrity_ok(&path)?;

    let base = base()?;
    let run_after_exit = run(&crate::store::open_store(&path).map_err(|_| FAILED)?)?;
    let recovery = match recover(&home, &path, base, case.boundary.stage()) {
        Ok(recovery) => Some(recovery),
        Err(error) if error == REFUSED => None,
        Err(error) => return Err(error),
    };
    let recovery_refused = recovery.is_none();
    let observed = Expected {
        initial_outcome: if terminated {
            InitialOutcome::Terminated
        } else {
            InitialOutcome::Responded
        },
        run_after_exit,
        run_after_restart: recovery
            .as_ref()
            .map_or(Run::Mixed, |recovery| recovery.run_after_restart),
        restart_is_noop: recovery
            .as_ref()
            .is_some_and(|recovery| recovery.restart_is_noop),
        run_after_pass: recovery
            .as_ref()
            .map_or(Run::Mixed, |recovery| recovery.run_after_pass),
        pass_is_noop: recovery
            .as_ref()
            .is_some_and(|recovery| recovery.pass_is_noop),
        stops: recovery.as_ref().map_or(0, |recovery| recovery.stops),
        launches: recovery.as_ref().map_or(0, |recovery| recovery.launches),
        integrity_ok,
        replay_is_noop: recovery
            .as_ref()
            .is_some_and(|recovery| recovery.replay_is_noop),
    };
    let passed = observed == case.expected && pause_consistent && !recovery_refused;
    let (transaction_open_at_pause, run_at_pause) = match pause {
        Some((open, run)) => (Some(open), Some(run)),
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
            "runAtPause": run_at_pause,
            "pauseConsistent": pause_consistent,
            "integrityOk": integrity_ok,
            "runAfterExit": observed.run_after_exit,
            "recoveryRefused": recovery_refused,
            "runAfterRestart": observed.run_after_restart,
            "restartIsNoop": observed.restart_is_noop,
            "runAfterPass": observed.run_after_pass,
            "passIsNoop": observed.pass_is_noop,
            "stops": observed.stops,
            "launches": observed.launches,
            "replayIsNoop": observed.replay_is_noop,
        }),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vectors() -> Value {
        serde_json::from_str(include_str!(
            "../../../../../conformance/automations/runner/held-settlement-process-crash-recovery.vectors.json"
        ))
        .unwrap()
    }

    #[test]
    fn held_crash_vectors_restate_each_boundary_outcome() {
        let vectors: VectorSet = serde_json::from_value(vectors()).unwrap();
        assert_eq!(vectors.schema_version, SCHEMA_VERSION);
        for case in &vectors.cases {
            validate(case).unwrap();
        }
        let boundaries: Vec<_> = vectors.cases.iter().map(|case| case.boundary).collect();
        assert_eq!(boundaries, Boundary::ALL);
    }

    #[test]
    fn held_crash_vectors_require_each_boundary_and_the_control() {
        let original = vectors();
        let mut missing = original.clone();
        missing["cases"].as_array_mut().unwrap().pop();
        assert_eq!(evaluate(&missing).unwrap_err(), INVALID);
        // A second control under its own case id, replacing a boundary.
        let mut duplicate = original.clone();
        let mut control = duplicate["cases"][6].clone();
        control["caseId"] = json!("second-uninterrupted-held-control");
        duplicate["cases"][0] = control;
        assert_eq!(evaluate(&duplicate).unwrap_err(), INVALID);
        let mut duplicate_id = original;
        duplicate_id["cases"][1]["caseId"] = duplicate_id["cases"][0]["caseId"].clone();
        assert_eq!(evaluate(&duplicate_id).unwrap_err(), INVALID);
    }

    #[test]
    fn held_crash_vectors_refuse_malformed_or_weakened_inputs() {
        let original = vectors();
        for (case, field, value) in [
            (0, "runAfterExit", json!("running")),
            (0, "runAfterRestart", json!("failed")),
            (0, "restartIsNoop", json!(true)),
            (1, "runAfterExit", json!("held")),
            (1, "runAfterPass", json!("held")),
            (2, "runAfterExit", json!("ended")),
            (2, "runAfterRestart", json!("failed")),
            (2, "restartIsNoop", json!(false)),
            (2, "passIsNoop", json!(true)),
            (3, "runAfterExit", json!("mixed")),
            (3, "runAfterRestart", json!("held")),
            (4, "restartIsNoop", json!(true)),
            (4, "passIsNoop", json!(false)),
            (4, "stops", json!(1)),
            (5, "runAfterExit", json!("held")),
            (5, "restartIsNoop", json!(false)),
            (6, "initialOutcome", json!("terminated")),
            (6, "launches", json!(1)),
            (6, "integrityOk", json!(false)),
            (6, "replayIsNoop", json!(false)),
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
            ("retry", json!({"maxAttempts": 2})),
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
            ("timeoutAt", json!("2099-10-20T09:00:00.000Z")),
            ("boundary", json!("after_launch")),
            ("caseId", json!("")),
        ] {
            let mut invalid = original.clone();
            invalid["cases"][0][field] = value;
            assert_eq!(evaluate(&invalid).unwrap_err(), INVALID, "{field}");
        }
        let mut invalid = original;
        invalid["schemaVersion"] =
            json!("coven.automations.timeout-enforcement-process-crash-recovery-vectors.v1");
        assert_eq!(evaluate(&invalid).unwrap_err(), INVALID);
    }

    /// A store whose routine has launched its 09:00 slot and whose session
    /// has failed before the deadline, as the worker leaves it before the
    /// hold.
    fn ended_store() -> (tempfile::TempDir, Connection) {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        crate::daemon::ensure_private_coven_home(&home).unwrap();
        let base = base().unwrap();
        drop(launch_slot(&home, &synthetic_definition(), at(base, CLAIM)).unwrap());
        let conn = crate::store::open_store(&crate::api::store_path(&home)).unwrap();
        assert_eq!(run(&conn).unwrap(), Run::Running);
        let session_id: String = conn
            .query_row("SELECT session_id FROM automation_runs", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert!(crate::store::update_session_terminal_if_active(
            &conn,
            &session_id,
            "failed",
            Some(1),
            &iso(at(base, END)),
        )
        .unwrap());
        assert_eq!(run(&conn).unwrap(), Run::Ended);
        (temp, conn)
    }

    #[test]
    fn the_hold_waits_for_the_deadline_and_settles_once() {
        let (_temp, conn) = ended_store();
        let base = base().unwrap();
        // Inside the hold window, the pass holds the run and then leaves it.
        for second in [HOLD, 89] {
            assert_eq!(
                settle_finished_runs(&conn, at(base, second)).unwrap(),
                SettlementReport::default()
            );
            assert_eq!(run(&conn).unwrap(), Run::Held, "{second}");
        }
        // The deadline itself ends the hold.
        assert_eq!(
            settle_finished_runs(&conn, at(base, 90)).unwrap(),
            failed_one()
        );
        assert_eq!(run(&conn).unwrap(), Run::Failed);
        for second in [DEADLINE, REPLAY] {
            assert_eq!(
                settle_finished_runs(&conn, at(base, second)).unwrap(),
                SettlementReport::default()
            );
            assert_eq!(run(&conn).unwrap(), Run::Failed, "{second}");
        }
    }

    #[test]
    fn each_transaction_is_whole_or_a_state_no_boundary_leaves() {
        let left: Vec<Run> = Boundary::ALL
            .iter()
            .map(|boundary| expected_for(*boundary).run_after_exit)
            .collect();
        let (_temp, conn) = ended_store();
        let step = |sql: &str, whole: Option<Run>| {
            conn.execute_batch(sql).unwrap();
            let now = run(&conn).unwrap();
            match whole {
                Some(whole) => assert_eq!(now, whole, "{sql}"),
                None => assert!(!left.contains(&now), "{sql}: {now:?}"),
            }
        };
        let base = base().unwrap();
        let (held_at, settled_at) = (iso(at(base, HOLD)), iso(at(base, DEADLINE)));
        // Both reasons hold an apostrophe.
        let (held, unretried) = (HELD.replace('\'', "''"), UNRETRIED.replace('\'', "''"));
        // The hold's one write, then the deadline's two, in the scheduler's
        // order. The store holds one run, attempt and occurrence; the writes
        // read them.
        conn.execute_batch("BEGIN").unwrap();
        step(
            &format!(
                "UPDATE automation_attempts
                 SET state = 'failed', state_reason = '{held}', settled_at = '{held_at}'"
            ),
            Some(Run::Held),
        );
        step(
            &format!(
                "UPDATE automation_occurrences
                 SET state = 'failed', failure_reason = '{unretried}', updated_at = '{settled_at}',
                     lease_owner = NULL, lease_expires_at = NULL"
            ),
            None,
        );
        step(
            &format!(
                "UPDATE automation_runs
                 SET status = 'failed', exit_code = 1, finished_at = '{settled_at}'"
            ),
            Some(Run::Failed),
        );
        conn.execute_batch("ROLLBACK").unwrap();
        assert_eq!(run(&conn).unwrap(), Run::Ended);
    }

    #[test]
    fn worker_refuses_invalid_or_weakened_input() {
        let mut case = vectors()["cases"][0].clone();
        case["storePath"] = json!("private-input");
        assert_eq!(worker(&case).unwrap_err(), INVALID);
        let mut case = vectors()["cases"][2].clone();
        case["expected"]["restartIsNoop"] = json!(false);
        assert_eq!(worker(&case).unwrap_err(), INVALID);
        let mut case = vectors()["cases"][6].clone();
        case["boundary"] = json!("after_settle_commit");
        assert_eq!(worker(&case).unwrap_err(), INVALID);
    }
}
