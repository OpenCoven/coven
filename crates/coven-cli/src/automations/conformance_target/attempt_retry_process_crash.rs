//! Disposable operator-retry crash fixtures: a daemon worker holds a failed
//! scheduled run and is killed while it handles `attempt.retry.v1`, and the
//! parent restarts the daemon's scheduler and sends the same request again.
//! Both launch through no-op runtimes; no runtime process starts.
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
use super::run_launch_process_crash::{pass, restart, snapshot_tables, CountingRuntime};
use super::valid_case_id;
use crate::automations::attempt_retry::{execute_attempt_retry, RetryExecution, RETRY_ACTION};
use crate::automations::definition::RoutineDefinition;
use crate::automations::leadership::SchedulerLeadership;

const INVALID: &str = "conformance vector is invalid";
const FAILED: &str = "conformance suite execution failed";
/// A production recovery step or the re-issued command returned an error; the
/// case fails.
const REFUSED: &str = "recovery refused";
const SCHEMA_VERSION: &str = "coven.automations.attempt-retry-process-crash-recovery-vectors.v1";
const WORKER: &str = "attempt-retry-crash-worker";
const HOME_PREFIX: &str = "retry-crash-";
/// The definition predates the schedule slot the worker claims.
const DEFINED_AT: &str = "2099-10-19T09:00:00.000Z";
/// The one adoption key the operator's request carries, sent and re-sent.
const ADOPTION_KEY: &str = "adopt:conformance:retry-crash";
/// The stored answer to that request: the retry command, committed.
const ADOPTED: &str = "attempt.retry.v1 committed";
/// Seconds after `BASE`: the worker claims and launches the 09:00 slot, its
/// session fails, a pass holds the run, and the worker handles the retry; the
/// parent restarts the daemon, the operator re-sends the request, a pass
/// dispatches the retry, its session ends, and a pass settles the run. The
/// run's 30-minute deadline is never reached.
const CLAIM: i64 = 30;
const FAILURE: i64 = 60;
const HOLD_PASS: i64 = 90;
const RETRY: i64 = 120;
const RESTART: i64 = 150;
const REISSUE: i64 = 160;
const RETRY_PASS: i64 = 170;
const SESSION_END: i64 = 175;
const SETTLE_PASS: i64 = 180;
const REPLAY: i64 = 240;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
enum Boundary {
    AfterAttempt,
    AfterReplan,
    AfterRelease,
    AfterAdoption,
    AfterCommit,
    None,
}

impl Boundary {
    /// What the store has committed when the worker stops.
    fn committed(self) -> Retry {
        match self {
            Self::AfterAttempt | Self::AfterReplan | Self::AfterRelease | Self::AfterAdoption => {
                Retry::Held
            }
            Self::AfterCommit | Self::None => Retry::Retried,
        }
    }

    /// The worker stops inside a write transaction.
    fn inside_transaction(self) -> bool {
        self.committed() == Retry::Held
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
    restart_launches: u64,
    retry_after_restart: Retry,
    reissue: Reissue,
    retry_after_reissue: Retry,
    launches: u64,
    final_retry: Retry,
    final_sessions: u64,
    integrity_ok: bool,
    replay_is_noop: bool,
}

#[derive(Deserialize, Serialize, PartialEq, Clone, Copy)]
#[serde(rename_all = "snake_case")]
enum InitialOutcome {
    Terminated,
    Responded,
}

/// The store's one occurrence with its run, both attempts, their sessions and
/// the answer stored under the operator's adoption key, read together. Attempt
/// 1 is always settled `failed` on its `failed` session.
#[derive(Debug, Deserialize, Serialize, PartialEq, Clone, Copy)]
#[serde(rename_all = "snake_case")]
enum Retry {
    /// Held for an operator retry: the run `running` on attempt 1's session,
    /// the occurrence `running` under the daemon's lease, and no attempt 2 or
    /// stored answer.
    Held,
    /// The retry committed: attempt 2 `adopted` as an operator retry of
    /// attempt 1's failure, with no session; the run released from its
    /// session; the occurrence `planned` with no lease; and the request's
    /// committed adoption.
    Retried,
    /// Attempt 2 dispatched: `started` on a second, `running` session that the
    /// run is on, with the occurrence `running` under the daemon's lease.
    Relaunched,
    /// The run, attempt 2 and the occurrence `succeeded`, and attempt 2's
    /// session `completed`.
    Succeeded,
    /// Anything else, including a partial retry.
    Mixed,
}

/// What the operator's re-sent request returned.
#[derive(Debug, Deserialize, Serialize, PartialEq, Clone, Copy)]
#[serde(rename_all = "snake_case")]
enum Reissue {
    /// The retry committed now: a fresh response naming attempt 2, opened at
    /// the re-sent request's time.
    Committed,
    /// The stored response: a replay naming attempt 2, opened earlier.
    Replayed,
    /// A refusal, or a response that does not name the store's attempt 2.
    Mismatched,
}

/// What each boundary must produce. A vector restates it, so it cannot weaken it.
fn expected_for(boundary: Boundary) -> Expected {
    let committed = boundary.committed() == Retry::Retried;
    Expected {
        initial_outcome: if boundary == Boundary::None {
            InitialOutcome::Responded
        } else {
            InitialOutcome::Terminated
        },
        retry_after_exit: boundary.committed(),
        restart_launches: u64::from(committed),
        retry_after_restart: if committed {
            Retry::Relaunched
        } else {
            Retry::Held
        },
        reissue: if committed {
            Reissue::Replayed
        } else {
            Reissue::Committed
        },
        retry_after_reissue: if committed {
            Retry::Relaunched
        } else {
            Retry::Retried
        },
        launches: 1,
        final_retry: Retry::Succeeded,
        final_sessions: 2,
        integrity_ok: true,
        replay_is_noop: true,
    }
}

/// A routine with no automatic retry, so its failure holds the run. The runner
/// launches only a routine with a working directory; a no-op runtime never
/// touches it.
fn synthetic_definition() -> Value {
    json!({"schemaVersion":1,"id":"conformance-retry-crash",
        "name":"Conformance retry crash","status":"ACTIVE",
        "rrule":"FREQ=DAILY;BYHOUR=9","timezone":"utc","misfire":"latest",
        "overlap":"forbid","timeoutMinutes":30,"runtime":"coven-code",
        "prompt":"Run the native retry crash conformance probe.",
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

/// Where the worker parks. The retry transaction opens attempt 2, returns the
/// occurrence to `planned`, releases the run's session, and stores the
/// response last. Nothing else commits after the pause is installed.
fn pause_at(boundary: Boundary) -> Option<Pause<'static>> {
    match boundary {
        Boundary::AfterAttempt => Some(Pause::After("AFTER INSERT ON main.automation_attempts")),
        Boundary::AfterReplan => Some(Pause::After(
            "AFTER UPDATE OF state ON main.automation_occurrences
             WHEN OLD.state IS NOT NEW.state",
        )),
        Boundary::AfterRelease => Some(Pause::After(
            "AFTER UPDATE OF session_id ON main.automation_runs
             WHEN OLD.session_id IS NOT NEW.session_id",
        )),
        Boundary::AfterAdoption => Some(Pause::After(
            "AFTER INSERT ON main.automation_command_adoptions",
        )),
        Boundary::AfterCommit => Some(Pause::AfterCommit),
        Boundary::None => None,
    }
}

/// The operator's request: retry the run's failed first attempt.
fn request(run_id: &str) -> Value {
    json!({"action": RETRY_ACTION, "adoptionKey": ADOPTION_KEY, "runId": run_id,
        "priorAttemptNumber": 1, "priorDisposition": "failed"})
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
    let claim_at = at(base, CLAIM);
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
    if report.dispatched.len() != 1 || !report.failed.is_empty() {
        return Err(FAILED);
    }
    // The session fails the way a runtime reports a nonzero exit, and the
    // daemon's next pass holds the run for an operator retry.
    let (run_id, session_id): (String, String) = conn
        .query_row("SELECT id, session_id FROM automation_runs", [], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .map_err(|_| FAILED)?;
    let failed = crate::store::update_session_terminal_if_active(
        &conn,
        &session_id,
        "failed",
        Some(1),
        &iso(at(base, FAILURE)),
    )
    .map_err(|_| FAILED)?;
    if !failed {
        return Err(FAILED);
    }
    pass(
        &home,
        &CountingRuntime::default(),
        &leadership,
        at(base, HOLD_PASS),
        "deadline",
    )
    .map_err(|_| FAILED)?;
    if retry(&conn)?.0 != Retry::Held {
        return Err(FAILED);
    }
    if let Some(pause_at) = pause_at(case.boundary) {
        let handshake =
            json!({"state":"paused", "boundary":case.boundary, "relativeHome":relative_home});
        install_pause(&conn, pause_at, handshake)?;
    }
    // The daemon handles the operator's request as its API does.
    let response =
        execute_attempt_retry(&conn, request(&run_id), at(base, RETRY)).map_err(|_| FAILED)?;
    if case.boundary != Boundary::None
        || !matches!(
            response,
            RetryExecution::Success {
                replayed: false,
                ..
            }
        )
    {
        return Err(FAILED);
    }
    // Exits like a daemon that dies after answering: without the graceful
    // shutdown's reconciliation or leadership release.
    drop(conn);
    drop(leadership);
    announce(&json!({"state":"responded", "boundary":case.boundary, "relativeHome":relative_home}))
}

/// Classifies the store's one occurrence with its run, attempts, sessions and
/// stored answer, and counts its sessions.
fn retry(conn: &Connection) -> Result<(Retry, u64), &'static str> {
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
    let session_count = u64::try_from(sessions).map_err(|_| FAILED)?;
    if occurrences != 1 || runs != 1 || !(1..=2).contains(&attempts) || !(1..=2).contains(&sessions)
    {
        return Ok((Retry::Mixed, session_count));
    }
    // A run whose first attempt or its session is missing matches no row.
    type Row = (
        String,
        bool,
        String,
        String,
        bool,
        Option<String>,
        bool,
        bool,
        Option<String>,
        Option<String>,
    );
    let row: Option<Row> = conn
        .query_row(
            "SELECT o.state, o.lease_owner IS 'daemon', r.status,
                    CASE
                        WHEN r.session_id IS NULL THEN 'none'
                        WHEN r.session_id = a1.session_id THEN 'first'
                        WHEN r.session_id = a2.session_id THEN 'second'
                        ELSE 'other'
                    END,
                    a1.state = 'failed' AND a1.settled_at IS NOT NULL AND s1.status = 'failed',
                    a2.state, a2.settled_at IS NOT NULL,
                    a2.retry_classification IS 'operator_retry'
                        AND a2.prior_attempt_number IS 1
                        AND a2.prior_disposition IS 'failed',
                    s2.status,
                    (SELECT command || ' ' || outcome FROM automation_command_adoptions
                     WHERE adoption_key = ?1)
             FROM automation_occurrences AS o
             JOIN automation_runs AS r ON r.occurrence_id = o.id
             JOIN automation_attempts AS a1 ON a1.run_id = r.id AND a1.attempt_number = 1
             JOIN sessions AS s1 ON s1.id = a1.session_id
             LEFT JOIN automation_attempts AS a2 ON a2.run_id = r.id AND a2.attempt_number = 2
             LEFT JOIN sessions AS s2 ON s2.id = a2.session_id",
            [ADOPTION_KEY],
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
                    row.get(8)?,
                    row.get(9)?,
                ))
            },
        )
        .optional()
        .map_err(|_| FAILED)?;
    let row = row.as_ref().map(
        |(
            occurrence,
            leased,
            run,
            run_on,
            first_failed,
            second,
            second_settled,
            second_retries_first,
            second_session,
            adoption,
        )| {
            (
                attempts,
                sessions,
                occurrence.as_str(),
                *leased,
                run.as_str(),
                run_on.as_str(),
                *first_failed,
                second.as_deref(),
                *second_settled,
                *second_retries_first,
                second_session.as_deref(),
                adoption.as_deref(),
            )
        },
    );
    let class = match row {
        Some((1, 1, "running", true, "running", "first", true, None, _, _, None, None)) => {
            Retry::Held
        }
        Some((
            2,
            1,
            "planned",
            false,
            "running",
            "none",
            true,
            Some("adopted"),
            false,
            true,
            None,
            Some(ADOPTED),
        )) => Retry::Retried,
        Some((
            2,
            2,
            "running",
            true,
            "running",
            "second",
            true,
            Some("started"),
            false,
            true,
            Some("running"),
            Some(ADOPTED),
        )) => Retry::Relaunched,
        Some((
            2,
            2,
            "succeeded",
            false,
            "succeeded",
            "second",
            true,
            Some("succeeded"),
            true,
            true,
            Some("completed"),
            Some(ADOPTED),
        )) => Retry::Succeeded,
        _ => Retry::Mixed,
    };
    Ok((class, session_count))
}

/// Sends the operator's request again at `now` and classifies the answer
/// against the attempt 2 the store holds.
fn reissue(conn: &Connection, run_id: &str, now: DateTime<Utc>) -> Result<Reissue, &'static str> {
    let execution = execute_attempt_retry(conn, request(run_id), now).map_err(|_| REFUSED)?;
    let RetryExecution::Success { payload, replayed } = execution else {
        return Ok(Reissue::Mismatched);
    };
    let opened: Option<(String, String, String)> = conn
        .query_row(
            "SELECT a.id, a.opened_at, r.occurrence_id
             FROM automation_attempts AS a
             JOIN automation_runs AS r ON r.id = a.run_id
             WHERE a.run_id = ?1 AND a.attempt_number = 2",
            [run_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(|_| FAILED)?;
    let Some((attempt_id, opened_at, occurrence_id)) = opened else {
        return Ok(Reissue::Mismatched);
    };
    let names_attempt = payload
        == json!({"runId": run_id, "occurrenceId": occurrence_id, "attemptId": attempt_id,
            "attemptNumber": 2, "priorDisposition": "failed", "status": "planned",
            "retriedAt": opened_at});
    Ok(match (names_attempt, replayed, opened_at == iso(now)) {
        (true, false, true) => Reissue::Committed,
        (true, true, false) => Reissue::Replayed,
        _ => Reissue::Mismatched,
    })
}

/// Every occurrence, run, attempt, event, session and adoption row, for an
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
            "automation_command_adoptions",
        ],
    )
}

/// What the restarted daemon and the re-sent request did with the store the
/// worker left.
struct Recovery {
    restart_launches: u64,
    retry_after_restart: Retry,
    reissue: Reissue,
    retry_after_reissue: Retry,
    launches: u64,
    final_retry: Retry,
    final_sessions: u64,
    replay_is_noop: bool,
}

fn recover(home: &Path, path: &Path, base: DateTime<Utc>) -> Result<Recovery, &'static str> {
    let runtime = CountingRuntime::default();
    let leadership = restart(home, &runtime, at(base, RESTART))?;
    let conn = crate::store::open_store(path).map_err(|_| FAILED)?;
    let (retry_after_restart, _) = retry(&conn)?;
    let restart_launches = runtime.launches();
    let run_id: String = conn
        .query_row("SELECT id FROM automation_runs", [], |row| row.get(0))
        .map_err(|_| FAILED)?;
    // The operator never saw an answer, so they send the same request again.
    let reissued = reissue(&conn, &run_id, at(base, REISSUE))?;
    let (retry_after_reissue, _) = retry(&conn)?;
    drop(conn);
    pass(
        home,
        &runtime,
        &leadership,
        at(base, RETRY_PASS),
        "deadline",
    )?;
    let conn = crate::store::open_store(path).map_err(|_| FAILED)?;
    // The retried session ends the way a runtime reports a clean exit.
    if retry(&conn)?.0 == Retry::Relaunched {
        let session_id: String = conn
            .query_row("SELECT session_id FROM automation_runs", [], |row| {
                row.get(0)
            })
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
    pass(
        home,
        &runtime,
        &leadership,
        at(base, SETTLE_PASS),
        "deadline",
    )?;
    let conn = crate::store::open_store(path).map_err(|_| FAILED)?;
    let (final_retry, final_sessions) = retry(&conn)?;
    let launches = runtime.launches();
    let before_replay = snapshot(&conn)?;
    // The same request once more, then a pass, each on its own connection,
    // must replay without a write or a launch.
    let replayed = reissue(&conn, &run_id, at(base, REPLAY))? == Reissue::Replayed;
    drop(conn);
    pass(home, &runtime, &leadership, at(base, REPLAY), "deadline")?;
    let conn = crate::store::open_store(path).map_err(|_| FAILED)?;
    let replay_is_noop =
        replayed && snapshot(&conn)? == before_replay && runtime.launches() == launches;
    drop(conn);
    drop(leadership);
    Ok(Recovery {
        restart_launches,
        retry_after_restart,
        reissue: reissued,
        retry_after_reissue,
        launches,
        final_retry,
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
fn observe_pause(path: &Path) -> Result<(bool, Retry), &'static str> {
    let probe = open_raw(path)?;
    let open = write_transaction_open(&probe)?;
    Ok((open, retry(&probe)?.0))
}

fn case_matches(case: &Case) -> Result<(bool, Value), &'static str> {
    let root = super::scheduler_conformance_tempdir()?;
    crate::daemon::ensure_private_coven_home(root.path()).map_err(|_| FAILED)?;
    let payload = serde_json::to_vec(case).map_err(|_| INVALID)?;
    let (mut process, receiver) = spawn_worker::<Handshake>(WORKER, payload, root.path())?;
    let ready = receiver.recv_timeout(CHILD_GUARD).map_err(|_| FAILED)??;
    let crash = case.boundary != Boundary::None;
    if ready.boundary != case.boundary || ready.state != if crash { "paused" } else { "responded" }
    {
        return Err(FAILED);
    }
    let home = worker_home(root.path(), &ready.relative_home, HOME_PREFIX)?;
    let path = crate::api::store_path(&home);
    let pause = crash.then(|| observe_pause(&path)).transpose()?;
    let (transaction_open_at_pause, retry_at_pause) = pause.unzip();
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
    let (retry_after_exit, sessions_after_exit) = retry(&conn)?;
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
            InitialOutcome::Responded
        },
        retry_after_exit,
        restart_launches: recovery
            .as_ref()
            .map_or(0, |recovery| recovery.restart_launches),
        retry_after_restart: recovery
            .as_ref()
            .map_or(Retry::Mixed, |recovery| recovery.retry_after_restart),
        reissue: recovery
            .as_ref()
            .map_or(Reissue::Mismatched, |recovery| recovery.reissue),
        retry_after_reissue: recovery
            .as_ref()
            .map_or(Retry::Mixed, |recovery| recovery.retry_after_reissue),
        launches: recovery.as_ref().map_or(0, |recovery| recovery.launches),
        final_retry: recovery
            .as_ref()
            .map_or(Retry::Mixed, |recovery| recovery.final_retry),
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
            "retryAtPause": retry_at_pause,
            "pauseConsistent": pause_consistent,
            "integrityOk": integrity_ok,
            "retryAfterExit": retry_after_exit,
            "sessionsAfterExit": sessions_after_exit,
            "recoveryRefused": recovery_refused,
            "restartLaunches": observed.restart_launches,
            "retryAfterRestart": observed.retry_after_restart,
            "reissue": observed.reissue,
            "retryAfterReissue": observed.retry_after_reissue,
            "launches": observed.launches,
            "finalRetry": observed.final_retry,
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
            "../../../../../conformance/automations/runner/attempt-retry-process-crash-recovery.vectors.json"
        ))
        .unwrap()
    }

    #[test]
    fn retry_crash_vectors_restate_each_boundary_outcome() {
        let vectors: VectorSet = serde_json::from_value(vectors()).unwrap();
        assert_eq!(vectors.schema_version, SCHEMA_VERSION);
        for case in &vectors.cases {
            validate(case).unwrap();
        }
        let boundaries: Vec<_> = vectors.cases.iter().map(|case| case.boundary).collect();
        assert_eq!(
            boundaries,
            [
                Boundary::AfterAttempt,
                Boundary::AfterReplan,
                Boundary::AfterRelease,
                Boundary::AfterAdoption,
                Boundary::AfterCommit,
                Boundary::None
            ]
        );
    }

    #[test]
    fn retry_crash_vectors_require_each_boundary_and_the_control() {
        let original = vectors();
        let mut missing = original.clone();
        missing["cases"].as_array_mut().unwrap().pop();
        assert_eq!(evaluate(&missing).unwrap_err(), INVALID);
        // A second control under its own case id, replacing a boundary.
        let mut duplicate = original.clone();
        let mut control = duplicate["cases"][5].clone();
        control["caseId"] = json!("second-uninterrupted-retry-control");
        duplicate["cases"][0] = control;
        assert_eq!(evaluate(&duplicate).unwrap_err(), INVALID);
        let mut duplicate_id = original;
        duplicate_id["cases"][1]["caseId"] = duplicate_id["cases"][0]["caseId"].clone();
        assert_eq!(evaluate(&duplicate_id).unwrap_err(), INVALID);
    }

    #[test]
    fn retry_crash_vectors_refuse_malformed_or_weakened_inputs() {
        let original = vectors();
        for (case, field, value) in [
            (0, "initialOutcome", json!("responded")),
            (0, "retryAfterExit", json!("retried")),
            (1, "restartLaunches", json!(1)),
            (1, "reissue", json!("replayed")),
            (2, "retryAfterRestart", json!("relaunched")),
            (2, "retryAfterReissue", json!("relaunched")),
            (3, "retryAfterExit", json!("mixed")),
            (3, "launches", json!(2)),
            (4, "retryAfterExit", json!("held")),
            (4, "reissue", json!("committed")),
            (4, "restartLaunches", json!(0)),
            (5, "finalRetry", json!("relaunched")),
            (5, "finalSessions", json!(1)),
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
            ("adoptionKey", json!("adopt:caller:chosen")),
            ("boundary", json!("after_dispatch")),
            ("caseId", json!("")),
        ] {
            let mut invalid = original.clone();
            invalid["cases"][0][field] = value;
            assert_eq!(evaluate(&invalid).unwrap_err(), INVALID, "{field}");
        }
        let mut invalid = original;
        invalid["schemaVersion"] =
            json!("coven.automations.run-launch-process-crash-recovery-vectors.v1");
        assert_eq!(evaluate(&invalid).unwrap_err(), INVALID);
    }

    #[test]
    fn retry_is_whole_or_mixed() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        crate::daemon::ensure_private_coven_home(&home).unwrap();
        let conn = crate::store::open_store(&crate::api::store_path(&home)).unwrap();
        let definition = RoutineDefinition::from_json(&synthetic_definition()).unwrap();
        crate::automations::store::insert_definition(&conn, &definition).unwrap();
        let base = base().unwrap();
        let claim_at = at(base, CLAIM);
        let leadership = SchedulerLeadership::acquire(&home, &conn, claim_at).unwrap();
        let fence = leadership.fence();
        crate::automations::occurrences::tick_with_scheduler_fence(&conn, claim_at, &fence)
            .unwrap();
        crate::automations::runner::dispatch_claimed_occurrences_with_clock_and_cancel_and_scheduler(
            &conn,
            &crate::api::NoopSessionRuntime,
            claim_at,
            || claim_at,
            || false,
            &fence,
        )
        .unwrap();
        let (run_id, first_session): (String, String) = conn
            .query_row("SELECT id, session_id FROM automation_runs", [], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .unwrap();
        assert!(crate::store::update_session_terminal_if_active(
            &conn,
            &first_session,
            "failed",
            Some(1),
            &iso(at(base, FAILURE)),
        )
        .unwrap());
        let runtime = CountingRuntime::default();
        pass(
            &home,
            &runtime,
            &leadership,
            at(base, HOLD_PASS),
            "deadline",
        )
        .unwrap();
        assert_eq!(retry(&conn).unwrap(), (Retry::Held, 1));
        // The retry transaction's writes, in its order: every proper prefix is
        // mixed, and all four are the retry.
        let adopt = |key: &str, command: &str| {
            conn.execute(
                "INSERT INTO automation_command_adoptions (
                    adoption_key, command, automation_id, request_digest, outcome,
                    revision, response_json, adopted_at
                 ) VALUES (?1, ?2, 'conformance-retry-crash', 'digest', 'committed', NULL,
                           '{}', ?3)",
                [key, command, &iso(at(base, RETRY))],
            )
            .unwrap();
        };
        conn.execute_batch("BEGIN").unwrap();
        conn.execute(
            "INSERT INTO automation_attempts
                (id, run_id, occurrence_id, attempt_number, adoption_key,
                 occurrence_fence_generation, dispatch_generation, state,
                 prior_attempt_number, prior_disposition, retry_classification,
                 state_reason, not_before, opened_at)
             SELECT 'attempt-prefix-2', r.id, o.id, 2, 'automation:prefix:2', o.attempt, 0,
                    'adopted', 1, 'failed', 'operator_retry', 'operator retry', ?1, ?1
             FROM automation_runs AS r
             JOIN automation_occurrences AS o ON o.id = r.occurrence_id",
            [iso(at(base, RETRY))],
        )
        .unwrap();
        assert_eq!(retry(&conn).unwrap().0, Retry::Mixed);
        conn.execute(
            "UPDATE automation_occurrences
             SET state = 'planned', lease_owner = NULL, lease_expires_at = NULL",
            [],
        )
        .unwrap();
        assert_eq!(retry(&conn).unwrap().0, Retry::Mixed);
        conn.execute("UPDATE automation_runs SET session_id = NULL", [])
            .unwrap();
        assert_eq!(retry(&conn).unwrap().0, Retry::Mixed);
        adopt(ADOPTION_KEY, "attempt.retry.v1");
        assert_eq!(retry(&conn).unwrap().0, Retry::Retried);
        conn.execute_batch("ROLLBACK").unwrap();
        // Another command's answer under the operator's key leaves no held run.
        conn.execute_batch("BEGIN").unwrap();
        adopt(ADOPTION_KEY, "occurrence.recover.v1");
        assert_eq!(retry(&conn).unwrap().0, Retry::Mixed);
        conn.execute_batch("ROLLBACK").unwrap();
        assert_eq!(
            reissue(&conn, &run_id, at(base, RETRY)).unwrap(),
            Reissue::Committed
        );
        assert_eq!(retry(&conn).unwrap(), (Retry::Retried, 1));
        // The same request replays. Another key is refused, not reopened: its
        // refusal is stored under its own key and leaves the retry as it was.
        assert_eq!(
            reissue(&conn, &run_id, at(base, REISSUE)).unwrap(),
            Reissue::Replayed
        );
        let mut other = request(&run_id);
        other["adoptionKey"] = json!("adopt:conformance:another-retry");
        assert!(matches!(
            execute_attempt_retry(&conn, other, at(base, REISSUE)).unwrap(),
            RetryExecution::Rejected(_)
        ));
        assert_eq!(retry(&conn).unwrap(), (Retry::Retried, 1));
        pass(
            &home,
            &runtime,
            &leadership,
            at(base, RETRY_PASS),
            "deadline",
        )
        .unwrap();
        assert_eq!(retry(&conn).unwrap(), (Retry::Relaunched, 2));
        assert_eq!(runtime.launches(), 1);
        // A run left on its first session after relaunch is not this retry.
        conn.execute_batch("BEGIN").unwrap();
        conn.execute(
            "UPDATE automation_runs SET session_id = ?1",
            [&first_session],
        )
        .unwrap();
        assert_eq!(retry(&conn).unwrap().0, Retry::Mixed);
        conn.execute_batch("ROLLBACK").unwrap();
        let second_session: String = conn
            .query_row("SELECT session_id FROM automation_runs", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert!(crate::store::update_session_terminal_if_active(
            &conn,
            &second_session,
            "completed",
            Some(0),
            &iso(at(base, SESSION_END)),
        )
        .unwrap());
        pass(
            &home,
            &runtime,
            &leadership,
            at(base, SETTLE_PASS),
            "deadline",
        )
        .unwrap();
        assert_eq!(retry(&conn).unwrap(), (Retry::Succeeded, 2));
        assert_eq!(
            reissue(&conn, &run_id, at(base, REPLAY)).unwrap(),
            Reissue::Replayed
        );
    }

    #[test]
    fn worker_refuses_invalid_or_weakened_input() {
        let mut case = vectors()["cases"][0].clone();
        case["storePath"] = json!("private-input");
        assert_eq!(worker(&case).unwrap_err(), INVALID);
        let mut case = vectors()["cases"][0].clone();
        case["expected"]["launches"] = json!(2);
        assert_eq!(worker(&case).unwrap_err(), INVALID);
    }
}
