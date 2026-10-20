//! Disposable launch crash fixtures: a daemon worker claims one scheduled
//! occurrence and is killed at a launch boundary, and the parent restarts the
//! daemon's scheduler over its store. Both launch through no-op runtimes; no
//! runtime process starts.
use std::collections::BTreeSet;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

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
use super::valid_case_id;
use crate::automations::daemon_tick::{
    process_automations_pass_with_scheduler, recover_before_startup_pass, AutomationClock,
    AutomationWakeSignal, MonotonicInstant, SchedulerPassContext, SystemAutomationClock,
    WakeReason,
};
use crate::automations::definition::RoutineDefinition;
use crate::automations::leadership::SchedulerLeadership;

const INVALID: &str = "conformance vector is invalid";
const FAILED: &str = "conformance suite execution failed";
/// A production recovery step returned an error; the case fails.
const REFUSED: &str = "recovery refused";
const SCHEMA_VERSION: &str = "coven.automations.run-launch-process-crash-recovery-vectors.v1";
const WORKER: &str = "run-launch-crash-worker";
const HOME_PREFIX: &str = "launch-crash-";
/// The definition predates the schedule slot the worker claims.
const DEFINED_AT: &str = "2099-10-19T09:00:00.000Z";
/// Seconds after `BASE`: the worker claims and launches the 09:00 slot; the
/// parent restarts the daemon; a relaunched session ends; an ordinary pass
/// runs while a held run's deadline is still ahead; and a pass after the run's
/// 30-minute deadline, held or relaunched, settles it.
const CLAIM: i64 = 30;
const RESTART: i64 = 150;
const SESSION_END: i64 = 155;
const INTERVAL_PASS: i64 = 160;
const DEADLINE_PASS: i64 = 2_430;
const REPLAY_PASS: i64 = 2_490;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
enum Boundary {
    AfterSession,
    AfterDispatching,
    AfterLaunchCommit,
    AfterOwnership,
    AfterOwnershipCommit,
    None,
}

impl Boundary {
    /// What the store has committed when the worker stops.
    fn committed(self) -> Launch {
        match self {
            Self::AfterSession | Self::AfterDispatching => Launch::Claimed,
            Self::AfterLaunchCommit | Self::AfterOwnership => Launch::Launching,
            Self::AfterOwnershipCommit | Self::None => Launch::Owned,
        }
    }

    /// The worker stops inside a write transaction.
    fn inside_transaction(self) -> bool {
        matches!(
            self,
            Self::AfterSession | Self::AfterDispatching | Self::AfterOwnership
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
    launch_after_exit: Launch,
    restart_launches: u64,
    launch_after_restart: Launch,
    launch_after_pass: Launch,
    final_launch: Launch,
    final_sessions: u64,
    integrity_ok: bool,
    replay_is_noop: bool,
}

#[derive(Deserialize, Serialize, PartialEq, Clone, Copy)]
#[serde(rename_all = "snake_case")]
enum InitialOutcome {
    Terminated,
    Dispatched,
}

/// The store's one occurrence with its run, attempt and session, read together.
#[derive(Debug, Deserialize, Serialize, PartialEq, Clone, Copy)]
#[serde(rename_all = "snake_case")]
enum Launch {
    /// The occurrence is claimed by the daemon, with no run, attempt or session.
    Claimed,
    /// The launch committed: the run `running`, its attempt `dispatching` with
    /// no session, the session `created`, and the occurrence still `claimed`.
    Launching,
    /// Ownership published: the run, the session and the occurrence `running`,
    /// and the attempt `started` on that session.
    Owned,
    /// Held for an operator retry: the run `running`, the attempt settled
    /// `failed`, and the session `killed`. The attempt's session binding and
    /// the occurrence stay as the launch or ownership committed them.
    Held,
    /// The run, the settled attempt and the occurrence `succeeded`, and the
    /// session `completed`.
    Succeeded,
    /// The run, the settled attempt and the occurrence `failed`, and the
    /// session `killed`.
    Failed,
    /// Anything else, including a partial launch.
    Mixed,
}

/// What each boundary must produce. A vector restates it, so it cannot weaken it.
fn expected_for(boundary: Boundary) -> Expected {
    let relaunch = boundary.committed() == Launch::Claimed;
    Expected {
        initial_outcome: if boundary == Boundary::None {
            InitialOutcome::Dispatched
        } else {
            InitialOutcome::Terminated
        },
        launch_after_exit: boundary.committed(),
        restart_launches: u64::from(relaunch),
        launch_after_restart: if relaunch {
            Launch::Owned
        } else {
            Launch::Held
        },
        launch_after_pass: if relaunch {
            Launch::Succeeded
        } else {
            Launch::Held
        },
        final_launch: if relaunch {
            Launch::Succeeded
        } else {
            Launch::Failed
        },
        final_sessions: 1,
        integrity_ok: true,
        replay_is_noop: true,
    }
}

/// The runner launches only a routine with a working directory; a no-op
/// runtime never touches it.
fn synthetic_definition() -> Value {
    json!({"schemaVersion":1,"id":"conformance-launch-crash",
        "name":"Conformance launch crash","status":"ACTIVE",
        "rrule":"FREQ=DAILY;BYHOUR=9","timezone":"utc","misfire":"latest",
        "overlap":"forbid","timeoutMinutes":30,"runtime":"coven-code",
        "prompt":"Run the native launch crash conformance probe.",
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

/// Where the worker parks. The launch transaction inserts the session first
/// and marks the attempt `dispatching` last; the ownership transaction marks
/// the occurrence `running` last. Nothing else in the dispatch commits.
fn pause_at(boundary: Boundary) -> Option<Pause<'static>> {
    match boundary {
        Boundary::AfterSession => Some(Pause::After("AFTER INSERT ON main.sessions")),
        Boundary::AfterDispatching => Some(Pause::After(
            "AFTER UPDATE OF state ON main.automation_attempts
             WHEN OLD.state IS NOT NEW.state",
        )),
        Boundary::AfterLaunchCommit => Some(Pause::AfterCommit),
        Boundary::AfterOwnership => Some(Pause::After(
            "AFTER UPDATE OF state ON main.automation_occurrences
             WHEN OLD.state IS NOT NEW.state",
        )),
        Boundary::AfterOwnershipCommit => Some(Pause::AfterCommits(2)),
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
    let claim_at = at(base()?, CLAIM);
    let conn = crate::store::open_store(&crate::api::store_path(&home)).map_err(|_| FAILED)?;
    let definition = RoutineDefinition::from_json(&case.definition).map_err(|_| INVALID)?;
    crate::automations::store::insert_definition(&conn, &definition).map_err(|_| FAILED)?;
    conn.execute(
        "UPDATE automation_definitions SET created_at = ?1, updated_at = ?1 WHERE id = ?2",
        [DEFINED_AT, definition.id.as_str()],
    )
    .map_err(|_| FAILED)?;
    // The daemon's scheduler claims and dispatches under its leadership fence.
    let leadership = SchedulerLeadership::acquire(&home, &conn, claim_at).map_err(|_| FAILED)?;
    let fence = leadership.fence();
    let tick = crate::automations::occurrences::tick_with_scheduler_fence(&conn, claim_at, &fence)
        .map_err(|_| FAILED)?;
    if tick.planned.len() != 1 || tick.claimed.len() != 1 || !tick.failed.is_empty() {
        return Err(FAILED);
    }
    if let Some(pause_at) = pause_at(case.boundary) {
        let handshake =
            json!({"state":"paused", "boundary":case.boundary, "relativeHome":relative_home});
        install_pause(&conn, pause_at, handshake)?;
    }
    let report =
        crate::automations::runner::dispatch_claimed_occurrences_with_clock_and_cancel_and_scheduler(
            &conn,
            &crate::api::NoopSessionRuntime,
            claim_at,
            || claim_at,
            || false,
            &fence,
        )
        .map_err(|_| FAILED)?;
    if case.boundary != Boundary::None || report.dispatched.len() != 1 || !report.failed.is_empty()
    {
        return Err(FAILED);
    }
    // Exits like a daemon that dies after a completed launch: without the
    // graceful shutdown's reconciliation or leadership release.
    drop(conn);
    drop(leadership);
    announce(&json!({"state":"dispatched", "boundary":case.boundary, "relativeHome":relative_home}))
}

/// Classifies the store's one occurrence with its run, attempt and session,
/// and counts its sessions.
fn launch(conn: &Connection) -> Result<(Launch, u64), &'static str> {
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
    let sessions = u64::try_from(sessions).map_err(|_| FAILED)?;
    let class = match (occurrences, runs, attempts, sessions) {
        (1, 0, 0, 0) => {
            let claimed: bool = conn
                .query_row(
                    "SELECT state = 'claimed' AND lease_owner IS 'daemon'
                     FROM automation_occurrences",
                    [],
                    |row| row.get(0),
                )
                .map_err(|_| FAILED)?;
            if claimed {
                Launch::Claimed
            } else {
                Launch::Mixed
            }
        }
        (1, 1, 1, 1) => {
            // A run, attempt and session that do not belong together match no row.
            let row: Option<(String, String, Option<String>, bool, String, String)> = conn
                .query_row(
                    "SELECT r.status, a.state, a.session_id, a.settled_at IS NOT NULL,
                            s.status, o.state
                     FROM automation_runs AS r
                     JOIN automation_attempts AS a ON a.run_id = r.id
                     JOIN sessions AS s ON s.id = r.session_id
                     JOIN automation_occurrences AS o ON o.id = r.occurrence_id
                     WHERE a.attempt_number = 1
                       AND (a.session_id IS NULL OR a.session_id = s.id)",
                    [],
                    |row| {
                        Ok((
                            row.get(0)?,
                            row.get(1)?,
                            row.get(2)?,
                            row.get(3)?,
                            row.get(4)?,
                            row.get(5)?,
                        ))
                    },
                )
                .optional()
                .map_err(|_| FAILED)?;
            let row = row.as_ref().map(
                |(run, attempt, attempt_session, settled, session, occurrence)| {
                    (
                        run.as_str(),
                        attempt.as_str(),
                        attempt_session.is_some(),
                        *settled,
                        session.as_str(),
                        occurrence.as_str(),
                    )
                },
            );
            match row {
                Some(("running", "dispatching", false, false, "created", "claimed")) => {
                    Launch::Launching
                }
                Some(("running", "started", true, false, "running", "running")) => Launch::Owned,
                Some(
                    ("running", "failed", false, true, "killed", "claimed")
                    | ("running", "failed", true, true, "killed", "running"),
                ) => Launch::Held,
                Some(("succeeded", "succeeded", true, true, "completed", "succeeded")) => {
                    Launch::Succeeded
                }
                Some(("failed", "failed", _, true, "killed", "failed")) => Launch::Failed,
                _ => Launch::Mixed,
            }
        }
        _ => Launch::Mixed,
    };
    Ok((class, sessions))
}

/// Every occurrence, run, attempt, event and session row, for an exact
/// before/after comparison.
fn snapshot(conn: &Connection) -> Result<Vec<Vec<rusqlite::types::Value>>, &'static str> {
    let mut rows = Vec::new();
    for table in [
        "automation_occurrences",
        "automation_runs",
        "automation_attempts",
        "automation_events",
        "sessions",
    ] {
        let mut statement = conn
            .prepare(&format!("SELECT * FROM {table} ORDER BY 1"))
            .map_err(|_| FAILED)?;
        let columns = statement.column_count();
        let mut query = statement.query([]).map_err(|_| FAILED)?;
        while let Some(row) = query.next().map_err(|_| FAILED)? {
            rows.push(
                (0..columns)
                    .map(|column| row.get(column))
                    .collect::<rusqlite::Result<_>>()
                    .map_err(|_| FAILED)?,
            );
        }
    }
    Ok(rows)
}

/// A no-op runtime that counts the launches the restarted daemon makes.
#[derive(Default)]
struct CountingRuntime {
    launches: AtomicUsize,
}

impl CountingRuntime {
    fn launches(&self) -> u64 {
        self.launches.load(Ordering::SeqCst) as u64
    }
}

impl crate::api::SessionRuntime for CountingRuntime {
    fn launch_session(&self, _launch: &crate::api::SessionLaunch) -> anyhow::Result<()> {
        self.launches.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn send_input(&self, _session_id: &str, _payload: &Value) -> anyhow::Result<()> {
        Ok(())
    }

    fn kill_session(&self, _session_id: &str) -> anyhow::Result<()> {
        Ok(())
    }
}

/// A clock frozen at one instant. The passes it drives never sleep.
struct FixedClock {
    now: DateTime<Utc>,
    system: SystemAutomationClock,
}

impl AutomationClock for FixedClock {
    fn now_utc(&self) -> DateTime<Utc> {
        self.now
    }

    fn monotonic_now(&self) -> MonotonicInstant {
        self.system.monotonic_now()
    }

    fn sleep_until_or_wake(
        &self,
        _deadline: MonotonicInstant,
        _wake: &AutomationWakeSignal,
        _observed_generation: u64,
    ) -> WakeReason {
        WakeReason::Shutdown
    }
}

/// One scheduler pass at `now`, as the daemon's scheduler thread runs it.
fn pass(
    home: &Path,
    runtime: &CountingRuntime,
    leadership: &SchedulerLeadership,
    now: DateTime<Utc>,
    trigger: &str,
) -> Result<(), &'static str> {
    let clock = FixedClock {
        now,
        system: SystemAutomationClock::default(),
    };
    process_automations_pass_with_scheduler(
        home,
        runtime,
        &clock,
        &AutomationWakeSignal::default(),
        (trigger == "startup").then_some(now),
        &leadership.fence(),
        SchedulerPassContext {
            trigger,
            scheduled_at: now,
        },
    )
    .map(|_| ())
    .map_err(|_| REFUSED)
}

/// Takes scheduler leadership. Windows documents a delay before it releases
/// a killed holder's lock, so the wait is bounded rather than immediate. It
/// retries any error rather than only contention: the home's owner-only check
/// can also fail briefly while the dead holder's handles close. A lasting
/// error still ends as a refused case.
fn acquire_leadership(
    home: &Path,
    conn: &Connection,
    now: DateTime<Utc>,
) -> Result<SchedulerLeadership, &'static str> {
    let deadline = Instant::now() + CHILD_GUARD;
    loop {
        match SchedulerLeadership::acquire(home, conn, now) {
            Ok(leadership) => return Ok(leadership),
            Err(_) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            Err(_) => return Err(REFUSED),
        }
    }
}

/// The daemon's startup order without its scheduler thread: session recovery
/// (`daemon.rs`), then scheduler leadership, its startup recovery and its
/// startup pass (`daemon_tick.rs`).
fn restart(
    home: &Path,
    runtime: &CountingRuntime,
    now: DateTime<Utc>,
) -> Result<SchedulerLeadership, &'static str> {
    let now_iso = iso(now);
    crate::daemon::recover_orphaned_sessions(home, &now_iso).map_err(|_| REFUSED)?;
    crate::daemon::recover_stale_created_sessions(home, &now_iso).map_err(|_| REFUSED)?;
    let conn = crate::store::open_store(&crate::api::store_path(home)).map_err(|_| FAILED)?;
    let leadership = acquire_leadership(home, &conn, now)?;
    recover_before_startup_pass(home, &conn, now).map_err(|_| REFUSED)?;
    drop(conn);
    pass(home, runtime, &leadership, now, "startup")?;
    Ok(leadership)
}

/// What the restarted daemon did with the store the worker left.
struct Recovery {
    launch_after_restart: Launch,
    restart_launches: u64,
    launch_after_pass: Launch,
    final_launch: Launch,
    final_sessions: u64,
    replay_is_noop: bool,
}

fn recover(home: &Path, path: &Path, base: DateTime<Utc>) -> Result<Recovery, &'static str> {
    let runtime = CountingRuntime::default();
    let leadership = restart(home, &runtime, at(base, RESTART))?;
    let conn = crate::store::open_store(path).map_err(|_| FAILED)?;
    let (launch_after_restart, _) = launch(&conn)?;
    let restart_launches = runtime.launches();
    // The relaunched session ends the way a runtime reports a clean exit.
    if launch_after_restart == Launch::Owned {
        let session_id: String = conn
            .query_row("SELECT id FROM sessions", [], |row| row.get(0))
            .map_err(|_| FAILED)?;
        let ended = crate::store::update_session_terminal_if_active(
            &conn,
            &session_id,
            "completed",
            Some(0),
            &iso(at(base, SESSION_END)),
        )
        .map_err(|_| FAILED)?;
        if !ended {
            return Err(FAILED);
        }
    }
    drop(conn);
    // An ordinary pass while a held run's deadline is still ahead.
    pass(
        home,
        &runtime,
        &leadership,
        at(base, INTERVAL_PASS),
        "deadline",
    )?;
    let conn = crate::store::open_store(path).map_err(|_| FAILED)?;
    let (launch_after_pass, _) = launch(&conn)?;
    drop(conn);
    pass(
        home,
        &runtime,
        &leadership,
        at(base, DEADLINE_PASS),
        "deadline",
    )?;
    let conn = crate::store::open_store(path).map_err(|_| FAILED)?;
    let (final_launch, final_sessions) = launch(&conn)?;
    let before_replay = snapshot(&conn)?;
    drop(conn);
    // A further pass, on its own connection, must change nothing, and no pass
    // since the restart may have launched.
    pass(
        home,
        &runtime,
        &leadership,
        at(base, REPLAY_PASS),
        "deadline",
    )?;
    let conn = crate::store::open_store(path).map_err(|_| FAILED)?;
    let replay_is_noop =
        snapshot(&conn)? == before_replay && runtime.launches() == restart_launches;
    drop(conn);
    drop(leadership);
    Ok(Recovery {
        launch_after_restart,
        restart_launches,
        launch_after_pass,
        final_launch,
        final_sessions,
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
    if vectors.schema_version != SCHEMA_VERSION || vectors.cases.len() != 6 {
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
fn observe_pause(path: &Path) -> Result<(bool, Launch), &'static str> {
    let probe = open_raw(path)?;
    let open = write_transaction_open(&probe)?;
    Ok((open, launch(&probe)?.0))
}

fn case_matches(case: &Case) -> Result<(bool, Value), &'static str> {
    let root = super::scheduler_conformance_tempdir()?;
    crate::daemon::ensure_private_coven_home(root.path()).map_err(|_| FAILED)?;
    let payload = serde_json::to_vec(case).map_err(|_| INVALID)?;
    let (mut process, receiver) = spawn_worker::<Handshake>(WORKER, payload, root.path())?;
    let ready = receiver.recv_timeout(CHILD_GUARD).map_err(|_| FAILED)??;
    let crash = case.boundary != Boundary::None;
    if ready.boundary != case.boundary || ready.state != if crash { "paused" } else { "dispatched" }
    {
        return Err(FAILED);
    }
    let home = worker_home(root.path(), &ready.relative_home, HOME_PREFIX)?;
    let path = crate::api::store_path(&home);
    let pause = crash.then(|| observe_pause(&path)).transpose()?;
    let (transaction_open_at_pause, launch_at_pause) = pause.unzip();
    let pause_consistent = pause.is_none_or(|(open, committed)| {
        open == case.boundary.inside_transaction() && committed == case.boundary.committed()
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
    let (launch_after_exit, sessions_after_exit) = launch(&conn)?;
    drop(conn);
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
            InitialOutcome::Dispatched
        },
        launch_after_exit,
        restart_launches: recovery
            .as_ref()
            .map_or(0, |recovery| recovery.restart_launches),
        launch_after_restart: recovery
            .as_ref()
            .map_or(Launch::Mixed, |recovery| recovery.launch_after_restart),
        launch_after_pass: recovery
            .as_ref()
            .map_or(Launch::Mixed, |recovery| recovery.launch_after_pass),
        final_launch: recovery
            .as_ref()
            .map_or(Launch::Mixed, |recovery| recovery.final_launch),
        final_sessions: recovery
            .as_ref()
            .map_or(0, |recovery| recovery.final_sessions),
        integrity_ok,
        replay_is_noop: recovery
            .as_ref()
            .is_some_and(|recovery| recovery.replay_is_noop),
    };
    let passed = observed == case.expected && pause_consistent && !recovery_refused;
    Ok((
        passed,
        json!({
            "caseId": case.case_id,
            "boundary": case.boundary,
            "initialOutcome": observed.initial_outcome,
            "abruptlyTerminated": terminated,
            "transactionOpenAtPause": transaction_open_at_pause,
            "launchAtPause": launch_at_pause,
            "pauseConsistent": pause_consistent,
            "integrityOk": integrity_ok,
            "launchAfterExit": launch_after_exit,
            "sessionsAfterExit": sessions_after_exit,
            "recoveryRefused": recovery_refused,
            "restartLaunches": observed.restart_launches,
            "launchAfterRestart": observed.launch_after_restart,
            "launchAfterPass": observed.launch_after_pass,
            "finalLaunch": observed.final_launch,
            "finalSessions": observed.final_sessions,
            "replayIsNoop": observed.replay_is_noop,
        }),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vectors() -> Value {
        serde_json::from_str(include_str!(
            "../../../../../conformance/automations/runner/run-launch-process-crash-recovery.vectors.json"
        ))
        .unwrap()
    }

    #[test]
    fn launch_crash_vectors_restate_each_boundary_outcome() {
        let vectors: VectorSet = serde_json::from_value(vectors()).unwrap();
        assert_eq!(vectors.schema_version, SCHEMA_VERSION);
        for case in &vectors.cases {
            validate(case).unwrap();
        }
        let boundaries: Vec<_> = vectors.cases.iter().map(|case| case.boundary).collect();
        assert_eq!(
            boundaries,
            [
                Boundary::AfterSession,
                Boundary::AfterDispatching,
                Boundary::AfterLaunchCommit,
                Boundary::AfterOwnership,
                Boundary::AfterOwnershipCommit,
                Boundary::None
            ]
        );
    }

    #[test]
    fn launch_crash_vectors_require_each_boundary_and_the_control() {
        let original = vectors();
        let mut missing = original.clone();
        missing["cases"].as_array_mut().unwrap().pop();
        assert_eq!(evaluate(&missing).unwrap_err(), INVALID);
        // A second control under its own case id, replacing a boundary.
        let mut duplicate = original.clone();
        let mut control = duplicate["cases"][5].clone();
        control["caseId"] = json!("second-uninterrupted-launch-control");
        duplicate["cases"][0] = control;
        assert_eq!(evaluate(&duplicate).unwrap_err(), INVALID);
        let mut duplicate_id = original;
        duplicate_id["cases"][1]["caseId"] = duplicate_id["cases"][0]["caseId"].clone();
        assert_eq!(evaluate(&duplicate_id).unwrap_err(), INVALID);
    }

    #[test]
    fn launch_crash_vectors_refuse_malformed_or_weakened_inputs() {
        let original = vectors();
        for (case, field, value) in [
            (0, "initialOutcome", json!("dispatched")),
            (0, "launchAfterExit", json!("launching")),
            (1, "restartLaunches", json!(0)),
            (1, "finalLaunch", json!("failed")),
            (2, "launchAfterExit", json!("claimed")),
            (2, "restartLaunches", json!(1)),
            (3, "launchAfterRestart", json!("owned")),
            (3, "launchAfterPass", json!("failed")),
            (0, "launchAfterPass", json!("held")),
            (3, "finalSessions", json!(2)),
            (4, "launchAfterExit", json!("launching")),
            (4, "finalLaunch", json!("succeeded")),
            (5, "integrityOk", json!(false)),
            (5, "replayIsNoop", json!(false)),
        ] {
            let mut invalid = original.clone();
            invalid["cases"][case]["expected"][field] = value;
            assert_eq!(evaluate(&invalid).unwrap_err(), INVALID, "{case} {field}");
        }
        for (field, value) in [
            ("name", json!("Caller chosen")),
            ("status", json!("PAUSED")),
            ("cwd", json!("/private-input")),
            ("timeoutMinutes", json!(1)),
            ("outputTarget", json!({"kind": "main-session"})),
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
            ("boundary", json!("after_settlement")),
            ("caseId", json!("")),
        ] {
            let mut invalid = original.clone();
            invalid["cases"][0][field] = value;
            assert_eq!(evaluate(&invalid).unwrap_err(), INVALID, "{field}");
        }
        let mut invalid = original;
        invalid["schemaVersion"] =
            json!("coven.automations.run-settlement-process-crash-recovery-vectors.v1");
        assert_eq!(evaluate(&invalid).unwrap_err(), INVALID);
    }

    #[test]
    fn launch_is_whole_or_mixed() {
        let home = tempfile::tempdir().unwrap();
        let conn = crate::store::open_store(&home.path().join("store.sqlite")).unwrap();
        let definition = RoutineDefinition::from_json(&synthetic_definition()).unwrap();
        crate::automations::store::insert_definition(&conn, &definition).unwrap();
        let claim_at = at(base().unwrap(), CLAIM);
        assert!(crate::automations::occurrences::insert_claimed_occurrence(
            &conn,
            "occurrence-launch-classes",
            &definition.id,
            "daemon",
            60,
            claim_at,
        )
        .unwrap());
        assert_eq!(launch(&conn).unwrap(), (Launch::Claimed, 0));
        let outcome = crate::automations::runner::dispatch_claimed_occurrences_with_clock(
            &conn,
            &crate::api::NoopSessionRuntime,
            claim_at,
            || claim_at,
        )
        .unwrap();
        assert_eq!(outcome.dispatched.len(), 1);
        assert_eq!(launch(&conn).unwrap(), (Launch::Owned, 1));
        // What an ownership publication split after each write would leave.
        conn.execute_batch("BEGIN").unwrap();
        conn.execute(
            "UPDATE automation_attempts SET state = 'dispatching', session_id = NULL",
            [],
        )
        .unwrap();
        assert_eq!(launch(&conn).unwrap().0, Launch::Mixed);
        conn.execute("UPDATE sessions SET status = 'created'", [])
            .unwrap();
        assert_eq!(launch(&conn).unwrap().0, Launch::Mixed);
        conn.execute("UPDATE automation_occurrences SET state = 'claimed'", [])
            .unwrap();
        assert_eq!(launch(&conn).unwrap().0, Launch::Launching);
        // A held run keeps its binding: an attempt never bound to its session
        // cannot be held on a running occurrence.
        conn.execute(
            "UPDATE automation_attempts SET state = 'failed', settled_at = ?1",
            [iso(claim_at)],
        )
        .unwrap();
        conn.execute("UPDATE sessions SET status = 'killed'", [])
            .unwrap();
        assert_eq!(launch(&conn).unwrap().0, Launch::Held);
        conn.execute("UPDATE automation_occurrences SET state = 'running'", [])
            .unwrap();
        assert_eq!(launch(&conn).unwrap().0, Launch::Mixed);
        conn.execute_batch("ROLLBACK").unwrap();
        // A run that has released its session is not this launch.
        conn.execute_batch("BEGIN").unwrap();
        conn.execute("UPDATE automation_runs SET session_id = NULL", [])
            .unwrap();
        assert_eq!(launch(&conn).unwrap().0, Launch::Mixed);
        conn.execute_batch("ROLLBACK").unwrap();
        // A second session is never one launch.
        conn.execute_batch("BEGIN").unwrap();
        conn.execute_batch(
            "CREATE TEMP TABLE twin AS SELECT * FROM main.sessions;
             UPDATE twin SET id = 'second-' || id;
             INSERT INTO main.sessions SELECT * FROM twin;
             DROP TABLE twin;",
        )
        .unwrap();
        assert_eq!(launch(&conn).unwrap(), (Launch::Mixed, 2));
        conn.execute_batch("ROLLBACK").unwrap();
        assert_eq!(launch(&conn).unwrap(), (Launch::Owned, 1));
    }

    #[test]
    fn worker_refuses_invalid_or_weakened_input() {
        let mut case = vectors()["cases"][0].clone();
        case["storePath"] = json!("private-input");
        assert_eq!(worker(&case).unwrap_err(), INVALID);
        let mut case = vectors()["cases"][0].clone();
        case["expected"]["restartLaunches"] = json!(0);
        assert_eq!(worker(&case).unwrap_err(), INVALID);
    }
}
