//! Disposable timeout-enforcement crash fixtures: a daemon worker is killed
//! while its scheduler stops and settles a scheduled run that outlived its
//! deadline. The parent restarts the daemon's scheduler inside the timeout's
//! stop lease, then runs passes after it. Both launch through no-op runtimes;
//! no runtime process starts.
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
use super::result_delivery_recovery::{at, BASE};
use super::run_cancellation_process_crash::{worker_stops, RecordingStops, STOP_LOG};
use super::run_launch_process_crash::{pass, restart, snapshot_tables, CountingRuntime};
use super::valid_case_id;
use crate::automations::definition::RoutineDefinition;
use crate::automations::leadership::SchedulerLeadership;
use crate::automations::runner::enforce_run_timeouts;

const INVALID: &str = "conformance vector is invalid";
const FAILED: &str = "conformance suite execution failed";
/// A production recovery step returned an error; the case fails.
const REFUSED: &str = "timeout recovery refused";
const SCHEMA_VERSION: &str =
    "coven.automations.timeout-enforcement-process-crash-recovery-vectors.v1";
const WORKER: &str = "timeout-enforcement-crash-worker";
const HOME_PREFIX: &str = "timeout-crash-";
/// The definition predates the schedule slot the worker claims.
const DEFINED_AT: &str = "2099-10-19T09:00:00.000Z";
/// Seconds after `BASE`: the worker claims and launches the 09:00 slot, whose
/// one-minute deadline passes at 90 seconds, and enforces it at 100. The
/// timeout's stop lease ends 30 seconds later. The parent restarts the daemon
/// inside that lease, runs a pass after it and a pass after that, and a
/// further pass that must change nothing.
const CLAIM: i64 = 30;
const ENFORCE: i64 = 100;
const RESTART: i64 = 110;
const PASS: i64 = 180;
const SETTLE: i64 = 260;
const REPLAY: i64 = 340;

/// Where the worker can stop, in the enforcement's order: inside the stop
/// fence's transaction and after its commit, right after the runtime is asked
/// to stop the session, after each write of the settlement and after its
/// commit, and the uninterrupted control.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
enum Boundary {
    AfterFenceInsert,
    AfterFenceCommit,
    AfterStop,
    AfterSessionEnd,
    AfterAttemptSettle,
    AfterRunFinish,
    AfterOccurrenceSettle,
    AfterFenceRelease,
    AfterSettleCommit,
    None,
}

impl Boundary {
    const ALL: [Self; 10] = [
        Self::AfterFenceInsert,
        Self::AfterFenceCommit,
        Self::AfterStop,
        Self::AfterSessionEnd,
        Self::AfterAttemptSettle,
        Self::AfterRunFinish,
        Self::AfterOccurrenceSettle,
        Self::AfterFenceRelease,
        Self::AfterSettleCommit,
        Self::None,
    ];

    /// The worker stops inside a write transaction.
    fn inside_transaction(self) -> bool {
        !matches!(
            self,
            Self::AfterFenceCommit | Self::AfterStop | Self::AfterSettleCommit | Self::None
        )
    }

    /// What the store holds once the worker is gone.
    fn stage(self) -> Stage {
        match self {
            Self::AfterFenceInsert => Stage::Due,
            Self::AfterFenceCommit => Stage::Fenced,
            Self::AfterStop
            | Self::AfterSessionEnd
            | Self::AfterAttemptSettle
            | Self::AfterRunFinish
            | Self::AfterOccurrenceSettle
            | Self::AfterFenceRelease => Stage::Stopped,
            Self::AfterSettleCommit | Self::None => Stage::Settled,
        }
    }
}

/// How far the enforcement got before the worker died: nothing durable; the
/// stop fence; the stop itself; its settlement.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Stage {
    Due,
    Fenced,
    Stopped,
    Settled,
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
    worker_stops: u64,
    run_after_restart: Run,
    restart_is_noop: bool,
    run_after_pass: Run,
    run_after_settle: Run,
    later_stops: u64,
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
/// session and the run's stop fence, read together.
#[derive(Debug, Deserialize, Serialize, PartialEq, Clone, Copy)]
#[serde(rename_all = "snake_case")]
enum Run {
    /// Launched: the occurrence `running` under the daemon's lease, the run
    /// `running` on the attempt's `running` session, the attempt `started`,
    /// and no stop fence.
    Running,
    /// The same, with the timeout holding the run's stop fence on that
    /// session.
    Fenced,
    /// Behind the timeout's fence, the restart's containment has marked the
    /// session `killed`; the attempt is still `started` and the run and
    /// occurrence `running`.
    Contained,
    /// Behind the expired fence, the attempt settled `ambiguous` on its
    /// `killed` session and the occurrence `recovery_required` with no lease,
    /// both for the expired stop owner; the run is still `running`.
    Ambiguous,
    /// The session `killed`, the attempt `timed_out` with the timeout failure
    /// class, the run finished `failed` with no exit code and the occurrence
    /// `failed` with no lease, both for the timeout, and no stop fence.
    TimedOut,
    /// The attempt settled `ambiguous` for the expired stop owner on its
    /// `killed` session, the run finished `failed` with no exit code and the
    /// occurrence `failed` with no lease for the terminal evidence, and no
    /// stop fence.
    Failed,
    /// Anything else, including a partial transaction.
    Mixed,
}

/// What each boundary must produce. A vector restates it, so it cannot weaken
/// it.
fn expected_for(boundary: Boundary) -> Expected {
    let stage = boundary.stage();
    let run_after_exit = match stage {
        Stage::Due => Run::Running,
        Stage::Fenced | Stage::Stopped => Run::Fenced,
        Stage::Settled => Run::TimedOut,
    };
    // The restart's containment marks a session it finds running `killed`.
    // With no fence, its startup pass settles the run as timed out: the
    // session ended after the deadline. Behind the fence, it leaves the run
    // to the fence's owner until the fence's lease ends.
    let run_after_restart = match stage {
        Stage::Due | Stage::Settled => Run::TimedOut,
        Stage::Fenced | Stage::Stopped => Run::Contained,
    };
    // The pass after the lease takes the expired fence as an unknown stop and
    // needs recovery. The next pass settles the run `failed` on its ended
    // session and releases the fence (#1461).
    let (run_after_pass, run_after_settle) = match stage {
        Stage::Due | Stage::Settled => (Run::TimedOut, Run::TimedOut),
        Stage::Fenced | Stage::Stopped => (Run::Ambiguous, Run::Failed),
    };
    Expected {
        initial_outcome: if boundary == Boundary::None {
            InitialOutcome::Responded
        } else {
            InitialOutcome::Terminated
        },
        run_after_exit,
        worker_stops: u64::from(matches!(stage, Stage::Stopped | Stage::Settled)),
        run_after_restart,
        // A settled run gives the restart nothing to contain or settle.
        restart_is_noop: stage == Stage::Settled,
        run_after_pass,
        run_after_settle,
        later_stops: 0,
        launches: 0,
        integrity_ok: true,
        replay_is_noop: true,
    }
}

/// A routine with a one-minute deadline and no automatic retry. The runner
/// launches only a routine with a working directory; a no-op runtime never
/// touches it.
fn synthetic_definition() -> Value {
    json!({"schemaVersion":1,"id":"conformance-timeout-crash",
        "name":"Conformance timeout crash","status":"ACTIVE",
        "rrule":"FREQ=DAILY;BYHOUR=9","timezone":"utc","misfire":"latest",
        "overlap":"forbid","timeoutMinutes":1,"runtime":"coven-code",
        "prompt":"Run the native timeout crash conformance probe.",
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

/// Where the worker parks inside SQLite. The enforcement claims the stop
/// fence in one transaction; once the stop is confirmed, it ends the session
/// `killed`, settles the attempt `timed_out` and the run and the occurrence
/// `failed`, and releases the fence in another. Nothing else commits after
/// the pause is installed. A pause right after the stop is the runtime's, not
/// SQLite's.
fn pause_at(boundary: Boundary) -> Option<Pause<'static>> {
    match boundary {
        Boundary::AfterFenceInsert => {
            Some(Pause::After("AFTER INSERT ON main.automation_stop_fences"))
        }
        Boundary::AfterFenceCommit => Some(Pause::AfterCommits(1)),
        Boundary::AfterSessionEnd => Some(Pause::After(
            "AFTER UPDATE OF status ON main.sessions WHEN OLD.status IS NOT NEW.status",
        )),
        Boundary::AfterAttemptSettle => Some(Pause::After(
            "AFTER UPDATE OF state ON main.automation_attempts
             WHEN OLD.state IS NOT NEW.state",
        )),
        Boundary::AfterRunFinish => Some(Pause::After(
            "AFTER UPDATE OF status ON main.automation_runs
             WHEN OLD.status IS NOT NEW.status",
        )),
        Boundary::AfterOccurrenceSettle => Some(Pause::After(
            "AFTER UPDATE OF state ON main.automation_occurrences
             WHEN OLD.state IS NOT NEW.state",
        )),
        Boundary::AfterFenceRelease => {
            Some(Pause::After("AFTER DELETE ON main.automation_stop_fences"))
        }
        Boundary::AfterSettleCommit => Some(Pause::AfterCommits(2)),
        Boundary::AfterStop | Boundary::None => None,
    }
}

/// Claims and launches the routine's 09:00 slot under `home`'s scheduler
/// leadership, through a no-op runtime. Returns the leadership.
fn launched(home: &Path, base: DateTime<Utc>) -> Result<SchedulerLeadership, &'static str> {
    let conn = &crate::store::open_store(&crate::api::store_path(home)).map_err(|_| FAILED)?;
    let claim_at = at(base, CLAIM);
    let definition = RoutineDefinition::from_json(&synthetic_definition()).map_err(|_| INVALID)?;
    crate::automations::store::insert_definition(conn, &definition).map_err(|_| FAILED)?;
    conn.execute(
        "UPDATE automation_definitions SET created_at = ?1, updated_at = ?1 WHERE id = ?2",
        [DEFINED_AT, definition.id.as_str()],
    )
    .map_err(|_| FAILED)?;
    // The daemon's scheduler claims and dispatches under its leadership fence.
    let leadership = SchedulerLeadership::acquire(home, conn, claim_at).map_err(|_| FAILED)?;
    let fence = leadership.fence();
    let tick = crate::automations::occurrences::tick_with_scheduler_fence(conn, claim_at, &fence)
        .map_err(|_| FAILED)?;
    if tick.planned.len() != 1 || tick.claimed.len() != 1 || !tick.failed.is_empty() {
        return Err(FAILED);
    }
    let report =
        crate::automations::runner::dispatch_claimed_occurrences_with_clock_and_cancel_and_scheduler(
            conn,
            &crate::api::NoopSessionRuntime,
            claim_at,
            || claim_at,
            || false,
            &fence,
        )
        .map_err(|_| FAILED)?;
    if report.dispatched.len() != 1 || !report.failed.is_empty() || run(conn)? != Run::Running {
        return Err(FAILED);
    }
    Ok(leadership)
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
    let leadership = launched(&home, base)?;
    let conn = crate::store::open_store(&crate::api::store_path(&home)).map_err(|_| FAILED)?;
    let handshake =
        json!({"state":"paused", "boundary":case.boundary, "relativeHome":relative_home});
    if let Some(pause_at) = pause_at(case.boundary) {
        install_pause(&conn, pause_at, handshake.clone())?;
    }
    let runtime = RecordingStops {
        log: home.join(STOP_LOG),
        park: (case.boundary == Boundary::AfterStop).then_some(handshake),
    };
    // The daemon's pass enforces every deadline that has passed.
    let failures = enforce_run_timeouts(&conn, &runtime, at(base, ENFORCE)).map_err(|_| FAILED)?;
    if case.boundary != Boundary::None || !failures.is_empty() || run(&conn)? != Run::TimedOut {
        return Err(FAILED);
    }
    // Exits like a daemon that dies after the pass: without the graceful
    // shutdown's reconciliation or the release of its scheduler authority.
    drop(conn);
    drop(leadership);
    announce(&json!({"state":"settled", "boundary":case.boundary, "relativeHome":relative_home}))
}

/// Why the attempt and the occurrence settled, read together.
#[derive(Clone, Copy, PartialEq)]
enum Reason {
    /// Neither has settled.
    None,
    /// Both timed out: the enforcement's own settlement, or the startup
    /// pass's of a session contained after the deadline.
    TimedOut,
    /// Both record the expired stop owner.
    Expired,
    /// The attempt records the expired stop owner, and the occurrence the
    /// terminal evidence that settled it.
    Settled,
    Other,
}

fn reason(attempt: Option<&str>, occurrence: Option<&str>) -> Reason {
    const EXPIRED: &str = "stop owner expired after terminal session evidence";
    match (attempt, occurrence) {
        (None, None) => Reason::None,
        (Some(attempt), Some(occurrence))
            if attempt == occurrence
                && matches!(
                    attempt,
                    "automation run timed out" | "session killed at or after automation timeout"
                ) =>
        {
            Reason::TimedOut
        }
        (Some(EXPIRED), Some(EXPIRED)) => Reason::Expired,
        (
            Some(EXPIRED),
            Some("terminal session evidence observed after an ambiguous stop outcome"),
        ) => Reason::Settled,
        _ => Reason::Other,
    }
}

/// Classifies the store's one run with its occurrence, attempt, session and
/// stop fence.
fn run(conn: &Connection) -> Result<Run, &'static str> {
    let (occurrences, runs, attempts, sessions): (i64, i64, i64, i64) = conn
        .query_row(
            "SELECT
                (SELECT COUNT(*) FROM automation_occurrences),
                (SELECT COUNT(*) FROM automation_runs),
                (SELECT COUNT(*) FROM automation_attempts),
                (SELECT COUNT(*) FROM sessions)",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .map_err(|_| FAILED)?;
    if (occurrences, runs, attempts, sessions) != (1, 1, 1, 1) {
        return Ok(Run::Mixed);
    }
    // The six states, the attempt's failure class, four facts and the two
    // settlement reasons.
    type Row = (Vec<String>, Option<String>, [bool; 4], [Option<String>; 2]);
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
                    CASE
                        WHEN NOT EXISTS (SELECT 1 FROM automation_stop_fences) THEN 'none'
                        WHEN (SELECT COUNT(*) FROM automation_stop_fences) = 1
                            AND EXISTS (
                                SELECT 1 FROM automation_stop_fences
                                WHERE run_id = r.id AND session_id = a.session_id
                                  AND owner = 'timeout' AND operation_key IS NULL
                            ) THEN 'timeout'
                        ELSE 'other'
                    END,
                    a.failure_class,
                    r.session_id IS a.session_id, a.settled_at IS NOT NULL,
                    r.finished_at IS NOT NULL, r.exit_code IS NULL,
                    a.state_reason, o.failure_reason
             FROM automation_occurrences AS o
             JOIN automation_runs AS r ON r.occurrence_id = o.id
             JOIN automation_attempts AS a ON a.run_id = r.id AND a.attempt_number = 1
             JOIN sessions AS s ON s.id = a.session_id",
            [],
            |row| {
                Ok((
                    (0..6)
                        .map(|column| row.get(column))
                        .collect::<rusqlite::Result<_>>()?,
                    row.get(6)?,
                    [row.get(7)?, row.get(8)?, row.get(9)?, row.get(10)?],
                    [row.get(11)?, row.get(12)?],
                ))
            },
        )
        .optional()
        .map_err(|_| FAILED)?;
    let Some((states, class, [on_session, settled, finished, no_exit_code], [attempt, occurrence])) =
        row
    else {
        return Ok(Run::Mixed);
    };
    if !on_session || !no_exit_code {
        return Ok(Run::Mixed);
    }
    let states: Vec<&str> = states.iter().map(String::as_str).collect();
    let Ok([occurrence_state, lease, run, attempt_state, session, fence]) =
        <[&str; 6]>::try_from(states)
    else {
        return Ok(Run::Mixed);
    };
    Ok(
        match (
            occurrence_state,
            lease,
            run,
            attempt_state,
            class.as_deref(),
            settled,
            finished,
            session,
            fence,
            reason(attempt.as_deref(), occurrence.as_deref()),
        ) {
            (
                "running",
                "daemon",
                "running",
                "started",
                None,
                false,
                false,
                "running",
                "none",
                Reason::None,
            ) => Run::Running,
            (
                "running",
                "daemon",
                "running",
                "started",
                None,
                false,
                false,
                "running",
                "timeout",
                Reason::None,
            ) => Run::Fenced,
            (
                "running",
                "daemon",
                "running",
                "started",
                None,
                false,
                false,
                "killed",
                "timeout",
                Reason::None,
            ) => Run::Contained,
            (
                "recovery_required",
                "none",
                "running",
                "ambiguous",
                Some("ambiguous_evidence"),
                true,
                false,
                "killed",
                "timeout",
                Reason::Expired,
            ) => Run::Ambiguous,
            (
                "failed",
                "none",
                "failed",
                "timed_out",
                Some("timeout"),
                true,
                true,
                "killed",
                "none",
                Reason::TimedOut,
            ) => Run::TimedOut,
            (
                "failed",
                "none",
                "failed",
                "ambiguous",
                Some("ambiguous_evidence"),
                true,
                true,
                "killed",
                "none",
                Reason::Settled,
            ) => Run::Failed,
            _ => Run::Mixed,
        },
    )
}

/// Every row the enforcement, its recovery or a restart can write, for an
/// exact before/after comparison.
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
    run_after_settle: Run,
    later_stops: u64,
    launches: u64,
    replay_is_noop: bool,
}

fn recover(home: &Path, path: &Path, base: DateTime<Utc>) -> Result<Restarted, &'static str> {
    // One runtime confirms every stop and counts it, with every launch, for
    // the restart and each pass.
    let runtime = CountingRuntime::default();
    let before_restart = snapshot(&crate::store::open_store(path).map_err(|_| FAILED)?)?;
    let leadership = restart(home, &runtime, at(base, RESTART))?;
    let conn = crate::store::open_store(path).map_err(|_| FAILED)?;
    let run_after_restart = run(&conn)?;
    let restart_is_noop = snapshot(&conn)? == before_restart;
    drop(conn);
    pass(home, &runtime, &leadership, at(base, PASS), "deadline")?;
    let run_after_pass = run(&crate::store::open_store(path).map_err(|_| FAILED)?)?;
    pass(home, &runtime, &leadership, at(base, SETTLE), "deadline")?;
    let conn = crate::store::open_store(path).map_err(|_| FAILED)?;
    let run_after_settle = run(&conn)?;
    let later_stops = runtime.stops();
    let launches = runtime.launches();
    let before_replay = snapshot(&conn)?;
    drop(conn);
    // A further pass must change nothing and issue no stop or launch.
    pass(home, &runtime, &leadership, at(base, REPLAY), "deadline")?;
    let conn = crate::store::open_store(path).map_err(|_| FAILED)?;
    let replay_is_noop = snapshot(&conn)? == before_replay
        && runtime.stops() == later_stops
        && runtime.launches() == launches;
    drop(conn);
    drop(leadership);
    Ok(Restarted {
        run_after_restart,
        restart_is_noop,
        run_after_pass,
        run_after_settle,
        later_stops,
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
    let worker_stops = worker_stops(&home)?;

    let base = base()?;
    let run_after_exit = run(&crate::store::open_store(&path).map_err(|_| FAILED)?)?;
    let recovery = match recover(&home, &path, base) {
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
        worker_stops,
        run_after_restart: recovery
            .as_ref()
            .map_or(Run::Mixed, |recovery| recovery.run_after_restart),
        restart_is_noop: recovery
            .as_ref()
            .is_some_and(|recovery| recovery.restart_is_noop),
        run_after_pass: recovery
            .as_ref()
            .map_or(Run::Mixed, |recovery| recovery.run_after_pass),
        run_after_settle: recovery
            .as_ref()
            .map_or(Run::Mixed, |recovery| recovery.run_after_settle),
        later_stops: recovery.as_ref().map_or(0, |recovery| recovery.later_stops),
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
            "workerStops": observed.worker_stops,
            "recoveryRefused": recovery_refused,
            "runAfterRestart": observed.run_after_restart,
            "restartIsNoop": observed.restart_is_noop,
            "runAfterPass": observed.run_after_pass,
            "runAfterSettle": observed.run_after_settle,
            "laterStops": observed.later_stops,
            "launches": observed.launches,
            "replayIsNoop": observed.replay_is_noop,
        }),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::automations::conformance_target::result_delivery_recovery::iso;
    use std::sync::Mutex;

    fn vectors() -> Value {
        serde_json::from_str(include_str!(
            "../../../../../conformance/automations/runner/timeout-enforcement-process-crash-recovery.vectors.json"
        ))
        .unwrap()
    }

    #[test]
    fn timeout_crash_vectors_restate_each_boundary_outcome() {
        let vectors: VectorSet = serde_json::from_value(vectors()).unwrap();
        assert_eq!(vectors.schema_version, SCHEMA_VERSION);
        for case in &vectors.cases {
            validate(case).unwrap();
        }
        let boundaries: Vec<_> = vectors.cases.iter().map(|case| case.boundary).collect();
        assert_eq!(boundaries, Boundary::ALL);
    }

    #[test]
    fn timeout_crash_vectors_require_each_boundary_and_the_control() {
        let original = vectors();
        let mut missing = original.clone();
        missing["cases"].as_array_mut().unwrap().pop();
        assert_eq!(evaluate(&missing).unwrap_err(), INVALID);
        // A second control under its own case id, replacing a boundary.
        let mut duplicate = original.clone();
        let mut control = duplicate["cases"][9].clone();
        control["caseId"] = json!("second-uninterrupted-timeout-control");
        duplicate["cases"][0] = control;
        assert_eq!(evaluate(&duplicate).unwrap_err(), INVALID);
        let mut duplicate_id = original;
        duplicate_id["cases"][1]["caseId"] = duplicate_id["cases"][0]["caseId"].clone();
        assert_eq!(evaluate(&duplicate_id).unwrap_err(), INVALID);
    }

    #[test]
    fn timeout_crash_vectors_refuse_malformed_or_weakened_inputs() {
        let original = vectors();
        for (case, field, value) in [
            (0, "runAfterExit", json!("fenced")),
            (0, "workerStops", json!(1)),
            (0, "runAfterRestart", json!("contained")),
            (0, "restartIsNoop", json!(true)),
            (1, "workerStops", json!(1)),
            (1, "runAfterRestart", json!("timed_out")),
            (1, "runAfterPass", json!("failed")),
            (2, "workerStops", json!(0)),
            (2, "runAfterSettle", json!("ambiguous")),
            (3, "runAfterExit", json!("contained")),
            (4, "runAfterPass", json!("timed_out")),
            (5, "laterStops", json!(1)),
            (6, "runAfterSettle", json!("timed_out")),
            (7, "runAfterExit", json!("timed_out")),
            (8, "restartIsNoop", json!(false)),
            (8, "runAfterRestart", json!("failed")),
            (9, "initialOutcome", json!("terminated")),
            (9, "launches", json!(1)),
            (9, "integrityOk", json!(false)),
            (9, "replayIsNoop", json!(false)),
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
            json!("coven.automations.run-cancellation-process-crash-recovery-vectors.v1");
        assert_eq!(evaluate(&invalid).unwrap_err(), INVALID);
    }

    /// A store whose routine has launched its 09:00 slot, as the worker
    /// leaves it before its deadline is enforced.
    fn launched_store() -> (
        tempfile::TempDir,
        std::path::PathBuf,
        Connection,
        SchedulerLeadership,
    ) {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        crate::daemon::ensure_private_coven_home(&home).unwrap();
        let leadership = launched(&home, base().unwrap()).unwrap();
        let conn = crate::store::open_store(&crate::api::store_path(&home)).unwrap();
        (temp, home, conn, leadership)
    }

    /// Reads the store on its own connection while the enforcement asks it to
    /// stop the session.
    struct Observing {
        path: std::path::PathBuf,
        seen: Mutex<Vec<(bool, Run)>>,
    }

    impl crate::api::SessionRuntime for Observing {
        fn launch_session(&self, _launch: &crate::api::SessionLaunch) -> anyhow::Result<()> {
            Ok(())
        }

        fn send_input(&self, _session_id: &str, _payload: &Value) -> anyhow::Result<()> {
            Ok(())
        }

        fn kill_session(&self, _session_id: &str) -> anyhow::Result<()> {
            let probe = open_raw(&self.path).unwrap();
            let open = write_transaction_open(&probe).unwrap();
            self.seen.lock().unwrap().push((open, run(&probe).unwrap()));
            Ok(())
        }
    }

    #[test]
    fn the_stop_is_issued_once_between_the_fence_and_the_settlement() {
        let (_temp, home, conn, _leadership) = launched_store();
        let base = base().unwrap();
        let runtime = Observing {
            path: crate::api::store_path(&home),
            seen: Mutex::new(Vec::new()),
        };
        // Before the deadline, nothing is enforced.
        assert!(enforce_run_timeouts(&conn, &runtime, at(base, 89))
            .unwrap()
            .is_empty());
        assert!(runtime.seen.lock().unwrap().is_empty());
        assert_eq!(run(&conn).unwrap(), Run::Running);
        assert!(enforce_run_timeouts(&conn, &runtime, at(base, ENFORCE))
            .unwrap()
            .is_empty());
        // No write is open while the runtime stops the session, and the fence
        // is committed.
        assert_eq!(*runtime.seen.lock().unwrap(), [(false, Run::Fenced)]);
        assert_eq!(run(&conn).unwrap(), Run::TimedOut);
        // A later enforcement finds nothing to stop.
        let later = CountingRuntime::default();
        assert!(enforce_run_timeouts(&conn, &later, at(base, PASS))
            .unwrap()
            .is_empty());
        assert_eq!(later.stops(), 0);
    }

    #[test]
    fn each_transaction_is_whole_or_a_state_no_boundary_leaves() {
        let left: Vec<Run> = Boundary::ALL
            .iter()
            .map(|boundary| expected_for(*boundary).run_after_exit)
            .collect();
        let (_temp, _home, conn, _leadership) = launched_store();
        let step = |sql: &str, whole: Option<Run>| {
            conn.execute_batch(sql).unwrap();
            let now = run(&conn).unwrap();
            match whole {
                Some(whole) => assert_eq!(now, whole, "{sql}"),
                None => assert!(!left.contains(&now), "{sql}: {now:?}"),
            }
        };
        assert_eq!(run(&conn).unwrap(), Run::Running);
        let enforced_at = iso(at(base().unwrap(), ENFORCE));
        // The two transactions' writes, in the enforcement's order. The store
        // holds one run, attempt and session; the writes read them.
        conn.execute_batch("BEGIN").unwrap();
        step(
            &format!(
                "INSERT INTO automation_stop_fences
                    (run_id, session_id, owner, operation_key, acquired_at, execution_expires_at)
                 SELECT id, session_id, 'timeout', NULL, '{enforced_at}', '{enforced_at}'
                 FROM automation_runs"
            ),
            Some(Run::Fenced),
        );
        for sql in [
            "UPDATE sessions SET status = 'killed'".to_owned(),
            format!(
                "UPDATE automation_attempts
                 SET state = 'timed_out', failure_class = 'timeout',
                     state_reason = 'automation run timed out', settled_at = '{enforced_at}'"
            ),
            format!("UPDATE automation_runs SET status = 'failed', finished_at = '{enforced_at}'"),
            "UPDATE automation_occurrences
             SET state = 'failed', failure_reason = 'automation run timed out',
                 lease_owner = NULL, lease_expires_at = NULL"
                .to_owned(),
        ] {
            step(&sql, None);
        }
        step("DELETE FROM automation_stop_fences", Some(Run::TimedOut));
        conn.execute_batch("ROLLBACK").unwrap();
        assert_eq!(run(&conn).unwrap(), Run::Running);
    }

    #[test]
    fn worker_refuses_invalid_or_weakened_input() {
        let mut case = vectors()["cases"][0].clone();
        case["storePath"] = json!("private-input");
        assert_eq!(worker(&case).unwrap_err(), INVALID);
        let mut case = vectors()["cases"][2].clone();
        case["expected"]["workerStops"] = json!(0);
        assert_eq!(worker(&case).unwrap_err(), INVALID);
        let mut case = vectors()["cases"][9].clone();
        case["boundary"] = json!("after_settle_commit");
        assert_eq!(worker(&case).unwrap_err(), INVALID);
    }
}
