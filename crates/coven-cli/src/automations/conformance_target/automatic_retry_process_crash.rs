//! Disposable automatic-retry crash fixtures: a daemon worker claims one
//! scheduled occurrence whose runtime is unavailable for its first launch, and
//! is killed while its scheduler schedules the policy's retry or while it
//! re-claims and relaunches that retry. The parent restarts the daemon's
//! scheduler over its store, then runs passes after the retry's backoff. Both
//! launch through no-op runtimes; no runtime process starts.
use std::collections::BTreeSet;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};

use chrono::{DateTime, Utc};
use rusqlite::Connection;
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
use super::timeout_enforcement_process_crash::claim_slot;
use super::valid_case_id;
use crate::automations::leadership::SchedulerFence;
use crate::automations::runner::{
    dispatch_claimed_occurrences_with_clock_and_cancel_and_scheduler, DispatchReport,
};

const INVALID: &str = "conformance vector is invalid";
const FAILED: &str = "conformance suite execution failed";
const SCHEMA_VERSION: &str = "coven.automations.automatic-retry-process-crash-recovery-vectors.v1";
const WORKER: &str = "automatic-retry-crash-worker";
const HOME_PREFIX: &str = "auto-retry-crash-";
/// Seconds after `BASE`: the worker claims the 09:00 slot at 30, and its
/// launch finds the runtime unavailable, so the policy's retry waits until 90.
/// At 90 the worker re-claims the occurrence and launches the retry. The
/// parent restarts the daemon before the backoff ends when the worker died at
/// 30, and after it when the worker died at 90. It then runs a pass after the
/// backoff and a further pass that must change nothing, all well before the
/// run's 30-minute deadline.
const CLAIM: i64 = 30;
const RETRY: i64 = 90;
const EARLY_RESTART: i64 = 60;
const LATE_RESTART: i64 = 120;
const PASS: i64 = 150;
const REPLAY: i64 = 210;
/// What the runtime answers the slot's launch. The scheduler records it as the
/// first attempt's reason and the waiting occurrence's.
pub(super) const UNAVAILABLE: &str = "the conformance runtime is unavailable";
/// An attempt's reason once a restart has held it after its session was
/// killed.
const HELD: &str = "session killed; held for an operator retry until the run's deadline";

/// Where the worker can stop, in order: after the first, the retry's and the
/// last write of the transaction that settles the unavailable launch and
/// schedules the retry, and after its commit; inside the re-claim and after
/// its commit; inside the retry's launch and after its commit; inside its
/// ownership and after its commit; and the uninterrupted control.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
enum Boundary {
    AfterRejectedSession,
    AfterRetryRecorded,
    AfterSessionReleased,
    AfterRetryCommit,
    AfterReclaim,
    AfterReclaimCommit,
    AfterRelaunchDispatching,
    AfterRelaunchCommit,
    AfterRelaunchOwnership,
    AfterOwnershipCommit,
    None,
}

impl Boundary {
    const ALL: [Self; 11] = [
        Self::AfterRejectedSession,
        Self::AfterRetryRecorded,
        Self::AfterSessionReleased,
        Self::AfterRetryCommit,
        Self::AfterReclaim,
        Self::AfterReclaimCommit,
        Self::AfterRelaunchDispatching,
        Self::AfterRelaunchCommit,
        Self::AfterRelaunchOwnership,
        Self::AfterOwnershipCommit,
        Self::None,
    ];

    /// The worker stops inside a write transaction.
    fn inside_transaction(self) -> bool {
        matches!(
            self,
            Self::AfterRejectedSession
                | Self::AfterRetryRecorded
                | Self::AfterSessionReleased
                | Self::AfterReclaim
                | Self::AfterRelaunchDispatching
                | Self::AfterRelaunchOwnership
        )
    }

    /// What the store holds once the worker is gone.
    fn stage(self) -> Stage {
        match self {
            Self::AfterRejectedSession | Self::AfterRetryRecorded | Self::AfterSessionReleased => {
                Stage::Launching
            }
            Self::AfterRetryCommit | Self::AfterReclaim => Stage::Scheduled,
            Self::AfterReclaimCommit | Self::AfterRelaunchDispatching => Stage::Reclaimed,
            Self::AfterRelaunchCommit | Self::AfterRelaunchOwnership => Stage::Relaunching,
            Self::AfterOwnershipCommit | Self::None => Stage::Owned,
        }
    }

    /// The worker died at the slot, before the retry's backoff ended.
    fn early(self) -> bool {
        matches!(
            self,
            Self::AfterRejectedSession
                | Self::AfterRetryRecorded
                | Self::AfterSessionReleased
                | Self::AfterRetryCommit
        )
    }

    /// When the parent restarts the daemon: before the backoff ends if the
    /// worker died at the slot, after it otherwise.
    fn restart_at(self) -> i64 {
        if self.early() {
            EARLY_RESTART
        } else {
            LATE_RESTART
        }
    }
}

/// How far the occurrence got before the worker died: the slot's launch
/// committed; the retry was scheduled; re-claimed; launched; or owned.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Stage {
    Launching,
    Scheduled,
    Reclaimed,
    Relaunching,
    Owned,
}

impl Stage {
    fn retry(self) -> Retry {
        match self {
            Self::Launching => Retry::Launching,
            Self::Scheduled => Retry::Scheduled,
            Self::Reclaimed => Retry::Reclaimed,
            Self::Relaunching => Retry::Relaunching,
            Self::Owned => Retry::Retried,
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
    retry_after_exit: Retry,
    retry_after_restart: Retry,
    restart_is_noop: bool,
    restart_launches: u64,
    retry_after_pass: Retry,
    pass_is_noop: bool,
    launches: u64,
    stops: u64,
    integrity_ok: bool,
    replay_is_noop: bool,
}

#[derive(Deserialize, Serialize, PartialEq, Clone, Copy)]
#[serde(rename_all = "snake_case")]
enum InitialOutcome {
    Terminated,
    Relaunched,
}

/// The store's one occurrence with its run, attempts and sessions, read
/// together. Every class has one occurrence and one `running`, unfinished run,
/// and no stop fence.
#[derive(Debug, Deserialize, Serialize, PartialEq, Clone, Copy)]
#[serde(rename_all = "snake_case")]
enum Retry {
    /// The slot's launch committed: the occurrence `claimed` under the
    /// daemon's lease, the run on its `created` session, and the first
    /// attempt `dispatching` with no session bound. No retry.
    Launching,
    /// The same launch held for an operator retry: the first attempt settled
    /// `failed` for the hold with no failure class, and its session `killed`.
    /// No retry.
    Held,
    /// The policy's retry scheduled: the first attempt settled `failed` for
    /// the unavailable runtime and its session `failed`; the retry `adopted`
    /// until the backoff ends and bound to the occurrence's claim count; the
    /// run on no session; and the occurrence `planned` with no lease and the
    /// runtime's answer as its reason.
    Scheduled,
    /// The same, with the occurrence `claimed` under the daemon's lease.
    Reclaimed,
    /// The retry's launch committed: the retry `dispatching` with no session
    /// bound, the run on the retry's `created` session, and the occurrence
    /// `claimed` under the daemon's lease.
    Relaunching,
    /// The retry owned: the retry `started` on the run's `running` session,
    /// and the occurrence `running` under the daemon's lease.
    Retried,
    /// The retry held for an operator retry: settled `failed` for the hold
    /// with no failure class, and its session `killed`. The occurrence and the
    /// retry's session binding stay as its launch or ownership committed them.
    RelaunchHeld,
    /// Anything else, including a partial transaction.
    Mixed,
}

/// What each boundary must produce. A vector restates it, so it cannot weaken
/// it.
fn expected_for(boundary: Boundary) -> Expected {
    let stage = boundary.stage();
    // A restart holds a launch it finds committed, the slot's or the retry's,
    // as the launch suite does. It launches a scheduled retry once the backoff
    // has ended, and leaves it waiting before then.
    let waits = stage == Stage::Scheduled && boundary.early();
    let retry_after_restart = match stage {
        Stage::Launching => Retry::Held,
        Stage::Scheduled if waits => Retry::Scheduled,
        Stage::Scheduled | Stage::Reclaimed => Retry::Retried,
        Stage::Relaunching | Stage::Owned => Retry::RelaunchHeld,
    };
    let relaunches = matches!(stage, Stage::Scheduled | Stage::Reclaimed);
    Expected {
        initial_outcome: if boundary == Boundary::None {
            InitialOutcome::Relaunched
        } else {
            InitialOutcome::Terminated
        },
        retry_after_exit: stage.retry(),
        retry_after_restart,
        restart_is_noop: waits,
        restart_launches: u64::from(relaunches && !waits),
        // The pass after the backoff launches a retry the restart left
        // waiting, and finds nothing to do otherwise.
        retry_after_pass: if waits {
            Retry::Retried
        } else {
            retry_after_restart
        },
        pass_is_noop: !waits,
        launches: u64::from(relaunches),
        stops: 0,
        integrity_ok: true,
        replay_is_noop: true,
    }
}

/// A routine whose policy retries an unavailable runtime once, a minute after
/// the failure, within a 30-minute deadline. The runner launches only a
/// routine with a working directory; a no-op runtime never touches it.
fn synthetic_definition() -> Value {
    json!({"schemaVersion":1,"id":"conformance-auto-retry-crash",
        "name":"Conformance automatic retry crash","status":"ACTIVE",
        "rrule":"FREQ=DAILY;BYHOUR=9","timezone":"utc","misfire":"latest",
        "overlap":"forbid","timeoutMinutes":30,"runtime":"coven-code",
        "prompt":"Run the native automatic retry crash conformance probe.",
        "cwd":"/work/project",
        "retry":{"maxAttempts":2,"backoffPolicy":"fixed","backoffSeconds":60,
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

/// Where the worker parks inside SQLite. The slot's launch commits first. The
/// rejection then ends the session first, records the retry, and releases the
/// run's session last. At the retry, the tick advances its planning cursor
/// before the re-claim, which marks the occurrence `claimed` before it binds
/// the retry to the claim. The retry's launch marks it `dispatching` before
/// it binds the run's session, and its ownership marks the occurrence
/// `running` last. No other transaction after the pause is installed writes,
/// so the WAL hook counts only these six commits.
fn pause_at(boundary: Boundary) -> Option<Pause<'static>> {
    match boundary {
        Boundary::AfterRejectedSession => Some(Pause::After(
            "AFTER UPDATE OF status ON main.sessions
             WHEN NEW.status = 'failed'",
        )),
        Boundary::AfterRetryRecorded => Some(Pause::After(
            "AFTER INSERT ON main.automation_attempts
             WHEN NEW.attempt_number = 2",
        )),
        Boundary::AfterSessionReleased => Some(Pause::After(
            "AFTER UPDATE OF session_id ON main.automation_runs
             WHEN NEW.session_id IS NULL",
        )),
        Boundary::AfterRetryCommit => Some(Pause::AfterCommits(2)),
        Boundary::AfterReclaim => Some(Pause::After(
            "AFTER UPDATE OF state ON main.automation_occurrences
             WHEN NEW.state = 'claimed'",
        )),
        Boundary::AfterReclaimCommit => Some(Pause::AfterCommits(4)),
        Boundary::AfterRelaunchDispatching => Some(Pause::After(
            "AFTER UPDATE OF state ON main.automation_attempts
             WHEN NEW.state = 'dispatching' AND NEW.attempt_number = 2",
        )),
        Boundary::AfterRelaunchCommit => Some(Pause::AfterCommits(5)),
        Boundary::AfterRelaunchOwnership => Some(Pause::After(
            "AFTER UPDATE OF state ON main.automation_occurrences
             WHEN NEW.state = 'running'",
        )),
        Boundary::AfterOwnershipCommit => Some(Pause::AfterCommits(6)),
        Boundary::None => None,
    }
}

/// A no-op runtime that is unavailable for its first launch and accepts
/// every later one. It counts the launches it is asked for.
#[derive(Default)]
pub(super) struct UnavailableOnceRuntime {
    launches: AtomicUsize,
}

impl UnavailableOnceRuntime {
    /// The launches it was asked for, the refused first one included.
    pub(super) fn launches(&self) -> usize {
        self.launches.load(Ordering::SeqCst)
    }
}

impl crate::api::SessionRuntime for UnavailableOnceRuntime {
    fn launch_session(&self, _launch: &crate::api::SessionLaunch) -> anyhow::Result<()> {
        if self.launches.fetch_add(1, Ordering::SeqCst) == 0 {
            return Err(std::io::Error::new(std::io::ErrorKind::NotFound, UNAVAILABLE).into());
        }
        Ok(())
    }

    fn send_input(&self, _session_id: &str, _payload: &Value) -> anyhow::Result<()> {
        Ok(())
    }

    fn kill_session(&self, _session_id: &str) -> anyhow::Result<()> {
        Ok(())
    }
}

/// The daemon's dispatch of its claimed occurrences at `now`.
pub(super) fn dispatch(
    conn: &Connection,
    runtime: &UnavailableOnceRuntime,
    now: DateTime<Utc>,
    fence: &SchedulerFence,
) -> Result<DispatchReport, &'static str> {
    dispatch_claimed_occurrences_with_clock_and_cancel_and_scheduler(
        conn,
        runtime,
        now,
        || now,
        || false,
        fence,
    )
    .map_err(|_| FAILED)
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
    let fence = leadership.fence();
    let conn = crate::store::open_store(&crate::api::store_path(&home)).map_err(|_| FAILED)?;
    if let Some(pause_at) = pause_at(case.boundary) {
        let handshake =
            json!({"state":"paused", "boundary":case.boundary, "relativeHome":relative_home});
        install_pause(&conn, pause_at, handshake)?;
    }
    let runtime = UnavailableOnceRuntime::default();
    // The slot's launch finds the runtime unavailable, so the dispatch settles
    // the first attempt and schedules the policy's retry.
    let scheduled = dispatch(&conn, &runtime, at(base, CLAIM), &fence)?;
    // Once the backoff ends, the scheduler re-claims the occurrence and
    // launches the retry.
    let retry_at = at(base, RETRY);
    let tick = crate::automations::occurrences::tick_with_scheduler_fence(&conn, retry_at, &fence)
        .map_err(|_| FAILED)?;
    let relaunched = dispatch(&conn, &runtime, retry_at, &fence)?;
    if case.boundary != Boundary::None
        || scheduled != DispatchReport::default()
        || tick.claimed.len() != 1
        || !tick.failed.is_empty()
        || relaunched.dispatched.len() != 1
        || !relaunched.failed.is_empty()
        || runtime.launches() != 2
    {
        return Err(FAILED);
    }
    // Exits like a daemon that dies after a completed launch: without the
    // graceful shutdown's reconciliation or leadership release.
    drop(conn);
    drop(leadership);
    announce(&json!({"state":"relaunched", "boundary":case.boundary, "relativeHome":relative_home}))
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

/// Classifies the store's one occurrence with its run, attempts and sessions.
fn retry(conn: &Connection) -> Result<Retry, &'static str> {
    let counts: [i64; 5] = conn
        .query_row(
            "SELECT
                (SELECT COUNT(*) FROM automation_occurrences),
                (SELECT COUNT(*) FROM automation_runs),
                (SELECT COUNT(*) FROM automation_attempts),
                (SELECT COUNT(*) FROM sessions),
                (SELECT COUNT(*) FROM automation_stop_fences)",
            [],
            |row| {
                Ok([
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ])
            },
        )
        .map_err(|_| FAILED)?;
    let [occurrences, runs, attempt_count, session_count, fences] = counts;
    if (occurrences, runs, fences) != (1, 1, 0)
        || !matches!((attempt_count, session_count), (1, 1) | (2, 1) | (2, 2))
    {
        return Ok(Retry::Mixed);
    }
    type Row = (String, String, Option<String>, String, Option<String>, bool);
    let (occurrence, lease, occurrence_reason, run, run_session, finished): Row = conn
        .query_row(
            "SELECT o.state,
                    CASE
                        WHEN o.lease_owner = 'daemon' AND o.lease_expires_at IS NOT NULL
                            THEN 'daemon'
                        WHEN o.lease_owner IS NULL AND o.lease_expires_at IS NULL THEN 'none'
                        ELSE 'other'
                    END,
                    o.failure_reason, r.status, r.session_id,
                    r.finished_at IS NOT NULL OR r.exit_code IS NOT NULL
             FROM automation_occurrences AS o
             JOIN automation_runs AS r ON r.occurrence_id = o.id",
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
        .map_err(|_| FAILED)?;
    if run != "running" || finished {
        return Ok(Retry::Mixed);
    }
    // Sessions in the order the launches created them.
    let sessions: Vec<(String, String)> = conn
        .prepare("SELECT id, status FROM sessions ORDER BY created_at, rowid")
        .and_then(|mut statement| {
            statement
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                .collect()
        })
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
    // Every attempt belongs to the one occurrence.
    if usize::try_from(attempt_count).ok() != Some(attempts.len()) {
        return Ok(Retry::Mixed);
    }
    let (Some(first), Some(first_session)) = (attempts.first(), sessions.first()) else {
        return Ok(Retry::Mixed);
    };
    let claimed = occurrence == "claimed" && lease == "daemon";
    // The slot's attempt is never owned, so no session is ever bound to it.
    if first.classification != "initial" || first.session_id.is_some() || first.prior.is_some() {
        return Ok(Retry::Mixed);
    }
    let Some(retry) = attempts.get(1) else {
        // No retry: the run stays on the slot's session, and the claim keeps
        // the slot's attempt bound to it.
        if !claimed
            || occurrence_reason.is_some()
            || run_session.as_ref() != Some(&first_session.0)
            || !first.on_claim
            || first.failure_class.is_some()
        {
            return Ok(Retry::Mixed);
        }
        return Ok(
            match (
                first.state.as_str(),
                first.reason.as_deref(),
                first.settled,
                first_session.1.as_str(),
            ) {
                ("dispatching", None, false, "created") => Retry::Launching,
                ("failed", Some(HELD), true, "killed") => Retry::Held,
                _ => Retry::Mixed,
            },
        );
    };
    // The slot's attempt settled for the unavailable runtime, its session
    // ended, and the retry scheduled after the backoff, bound to the claim.
    if first.state != "failed"
        || first.failure_class.as_deref() != Some("runtime_unavailable")
        || first.reason.as_deref() != Some(UNAVAILABLE)
        || !first.settled
        || first_session.1 != "failed"
        || retry.classification != "automatic_retry"
        || retry.prior != Some((1, "failed".to_string()))
        || retry.not_before != iso(at(base()?, RETRY))
        || !retry.on_claim
    {
        return Ok(Retry::Mixed);
    }
    let Some(retry_session) = sessions.get(1) else {
        // Waiting for the backoff or its launch: the run on no session, and
        // the occurrence holding the runtime's answer.
        if run_session.is_some()
            || occurrence_reason.as_deref() != Some(UNAVAILABLE)
            || retry.state != "adopted"
            || retry.failure_class.is_some()
            || retry.reason.is_some()
            || retry.session_id.is_some()
            || retry.settled
        {
            return Ok(Retry::Mixed);
        }
        return Ok(match (occurrence.as_str(), lease.as_str()) {
            ("planned", "none") => Retry::Scheduled,
            ("claimed", "daemon") => Retry::Reclaimed,
            _ => Retry::Mixed,
        });
    };
    // Launched: the run on the retry's session. The occurrence's reason is
    // not read: a restart that restores an unlaunched claim clears it, and
    // every other path keeps the runtime's answer.
    if run_session.as_ref() != Some(&retry_session.0) || retry.failure_class.is_some() {
        return Ok(Retry::Mixed);
    }
    let unbound = retry.session_id.is_none();
    let bound = retry.session_id.as_ref() == Some(&retry_session.0);
    let running = occurrence == "running" && lease == "daemon";
    Ok(
        match (
            retry.state.as_str(),
            retry.reason.as_deref(),
            retry.settled,
            retry_session.1.as_str(),
        ) {
            ("dispatching", None, false, "created") if claimed && unbound => Retry::Relaunching,
            ("started", None, false, "running") if running && bound => Retry::Retried,
            ("failed", Some(HELD), true, "killed")
                if (claimed && unbound) || (running && bound) =>
            {
                Retry::RelaunchHeld
            }
            _ => Retry::Mixed,
        },
    )
}

/// Every row the dispatch, the re-claim, their recovery or a restart can
/// write, for an exact before/after comparison.
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
    retry_after_restart: Retry,
    restart_is_noop: bool,
    restart_launches: u64,
    retry_after_pass: Retry,
    pass_is_noop: bool,
    launches: u64,
    stops: u64,
    replay_is_noop: bool,
}

fn recover(
    home: &Path,
    path: &Path,
    base: DateTime<Utc>,
    boundary: Boundary,
) -> Result<Restarted, &'static str> {
    // One runtime counts every stop and launch for the restart and each pass.
    let runtime = CountingRuntime::default();
    let before_restart = snapshot(&crate::store::open_store(path).map_err(|_| FAILED)?)?;
    let leadership = restart(home, &runtime, at(base, boundary.restart_at()))?;
    let conn = crate::store::open_store(path).map_err(|_| FAILED)?;
    let retry_after_restart = retry(&conn)?;
    let restart_is_noop = snapshot(&conn)? == before_restart;
    let restart_launches = runtime.launches();
    let before_pass = snapshot(&conn)?;
    drop(conn);
    pass(home, &runtime, &leadership, at(base, PASS), "deadline")?;
    let conn = crate::store::open_store(path).map_err(|_| FAILED)?;
    let retry_after_pass = retry(&conn)?;
    let pass_is_noop = snapshot(&conn)? == before_pass;
    let launches = runtime.launches();
    let stops = runtime.stops();
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
        retry_after_restart,
        restart_is_noop,
        restart_launches,
        retry_after_pass,
        pass_is_noop,
        launches,
        stops,
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
    if ready.boundary != case.boundary || ready.state != if crash { "paused" } else { "relaunched" }
    {
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

    let base = base()?;
    let retry_after_exit = retry(&crate::store::open_store(&path).map_err(|_| FAILED)?)?;
    let recovery = match recover(&home, &path, base, case.boundary) {
        Ok(recovery) => Some(recovery),
        Err(error) if error == REFUSED => None,
        Err(error) => return Err(error),
    };
    let recovery_refused = recovery.is_none();
    let observed = Expected {
        initial_outcome: if terminated {
            InitialOutcome::Terminated
        } else {
            InitialOutcome::Relaunched
        },
        retry_after_exit,
        retry_after_restart: recovery
            .as_ref()
            .map_or(Retry::Mixed, |recovery| recovery.retry_after_restart),
        restart_is_noop: recovery
            .as_ref()
            .is_some_and(|recovery| recovery.restart_is_noop),
        restart_launches: recovery
            .as_ref()
            .map_or(0, |recovery| recovery.restart_launches),
        retry_after_pass: recovery
            .as_ref()
            .map_or(Retry::Mixed, |recovery| recovery.retry_after_pass),
        pass_is_noop: recovery
            .as_ref()
            .is_some_and(|recovery| recovery.pass_is_noop),
        launches: recovery.as_ref().map_or(0, |recovery| recovery.launches),
        stops: recovery.as_ref().map_or(0, |recovery| recovery.stops),
        integrity_ok,
        replay_is_noop: recovery
            .as_ref()
            .is_some_and(|recovery| recovery.replay_is_noop),
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
            "restartLaunches": observed.restart_launches,
            "retryAfterPass": observed.retry_after_pass,
            "passIsNoop": observed.pass_is_noop,
            "launches": observed.launches,
            "stops": observed.stops,
            "replayIsNoop": observed.replay_is_noop,
        }),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vectors() -> Value {
        serde_json::from_str(include_str!(
            "../../../../../conformance/automations/runner/automatic-retry-process-crash-recovery.vectors.json"
        ))
        .unwrap()
    }

    #[test]
    fn automatic_retry_crash_vectors_restate_each_boundary_outcome() {
        let vectors: VectorSet = serde_json::from_value(vectors()).unwrap();
        assert_eq!(vectors.schema_version, SCHEMA_VERSION);
        for case in &vectors.cases {
            validate(case).unwrap();
        }
        let boundaries: Vec<_> = vectors.cases.iter().map(|case| case.boundary).collect();
        assert_eq!(boundaries, Boundary::ALL);
    }

    #[test]
    fn automatic_retry_crash_vectors_require_each_boundary_and_the_control() {
        let original = vectors();
        let mut missing = original.clone();
        missing["cases"].as_array_mut().unwrap().pop();
        assert_eq!(evaluate(&missing).unwrap_err(), INVALID);
        // A second control under its own case id, replacing a boundary.
        let mut duplicate = original.clone();
        let mut control = duplicate["cases"][10].clone();
        control["caseId"] = json!("second-uninterrupted-retry-control");
        duplicate["cases"][0] = control;
        assert_eq!(evaluate(&duplicate).unwrap_err(), INVALID);
        let mut duplicate_id = original;
        duplicate_id["cases"][1]["caseId"] = duplicate_id["cases"][0]["caseId"].clone();
        assert_eq!(evaluate(&duplicate_id).unwrap_err(), INVALID);
    }

    #[test]
    fn automatic_retry_crash_vectors_refuse_malformed_or_weakened_inputs() {
        let original = vectors();
        for (case, field, value) in [
            (0, "retryAfterExit", json!("scheduled")),
            (0, "retryAfterRestart", json!("retried")),
            (0, "launches", json!(1)),
            (1, "retryAfterPass", json!("scheduled")),
            (2, "restartIsNoop", json!(true)),
            (3, "retryAfterRestart", json!("retried")),
            (3, "restartLaunches", json!(1)),
            (3, "restartIsNoop", json!(false)),
            (3, "passIsNoop", json!(true)),
            (4, "retryAfterExit", json!("reclaimed")),
            (4, "launches", json!(2)),
            (5, "retryAfterExit", json!("scheduled")),
            (6, "retryAfterRestart", json!("relaunch_held")),
            (7, "retryAfterRestart", json!("retried")),
            (7, "restartLaunches", json!(1)),
            (8, "retryAfterExit", json!("retried")),
            (9, "retryAfterPass", json!("retried")),
            (9, "stops", json!(1)),
            (10, "initialOutcome", json!("terminated")),
            (10, "integrityOk", json!(false)),
            (10, "replayIsNoop", json!(false)),
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
        invalid["cases"][0]["definition"]["retry"]["backoffSeconds"] = json!(1);
        assert_eq!(evaluate(&invalid).unwrap_err(), INVALID, "backoffSeconds");
        for (field, value) in [
            ("storePath", json!("private-input")),
            ("runId", json!("private-input")),
            ("notBefore", json!("2099-10-20T09:00:31.000Z")),
            ("boundary", json!("after_launch")),
            ("caseId", json!("")),
        ] {
            let mut invalid = original.clone();
            invalid["cases"][0][field] = value;
            assert_eq!(evaluate(&invalid).unwrap_err(), INVALID, "{field}");
        }
        let mut invalid = original;
        invalid["schemaVersion"] =
            json!("coven.automations.held-settlement-process-crash-recovery-vectors.v1");
        assert_eq!(evaluate(&invalid).unwrap_err(), INVALID);
    }

    /// A home whose routine's 09:00 slot the daemon's scheduler has claimed,
    /// with a connection to its store.
    fn claimed_store() -> (
        tempfile::TempDir,
        crate::automations::leadership::SchedulerLeadership,
        Connection,
    ) {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        crate::daemon::ensure_private_coven_home(&home).unwrap();
        let leadership =
            claim_slot(&home, &synthetic_definition(), at(base().unwrap(), CLAIM)).unwrap();
        let conn = crate::store::open_store(&crate::api::store_path(&home)).unwrap();
        (temp, leadership, conn)
    }

    #[test]
    fn the_retry_waits_for_its_backoff_and_launches_once() {
        let (_temp, leadership, conn) = claimed_store();
        let (fence, base) = (leadership.fence(), base().unwrap());
        let runtime = UnavailableOnceRuntime::default();
        assert_eq!(
            dispatch(&conn, &runtime, at(base, CLAIM), &fence).unwrap(),
            DispatchReport::default()
        );
        assert_eq!(retry(&conn).unwrap(), Retry::Scheduled);
        let tick = |second| {
            crate::automations::occurrences::tick_with_scheduler_fence(
                &conn,
                at(base, second),
                &fence,
            )
            .unwrap()
        };
        // A tick and a dispatch before the backoff ends leave the retry
        // waiting.
        assert!(tick(RETRY - 1).claimed.is_empty());
        assert_eq!(
            dispatch(&conn, &runtime, at(base, RETRY - 1), &fence).unwrap(),
            DispatchReport::default()
        );
        assert_eq!(retry(&conn).unwrap(), Retry::Scheduled);
        // Its end re-claims the occurrence, and the dispatch launches the
        // retry.
        assert_eq!(tick(RETRY).claimed.len(), 1);
        assert_eq!(retry(&conn).unwrap(), Retry::Reclaimed);
        assert_eq!(
            dispatch(&conn, &runtime, at(base, RETRY), &fence)
                .unwrap()
                .dispatched
                .len(),
            1
        );
        assert_eq!(retry(&conn).unwrap(), Retry::Retried);
        // Nothing launches again.
        assert!(tick(REPLAY).claimed.is_empty());
        assert_eq!(
            dispatch(&conn, &runtime, at(base, REPLAY), &fence).unwrap(),
            DispatchReport::default()
        );
        assert_eq!(retry(&conn).unwrap(), Retry::Retried);
        assert_eq!(runtime.launches(), 2);
    }

    #[test]
    fn each_transaction_is_whole_or_a_state_no_boundary_leaves() {
        let left: Vec<Retry> = Boundary::ALL
            .iter()
            .map(|boundary| expected_for(*boundary).retry_after_exit)
            .collect();
        let (_temp, leadership, conn) = claimed_store();
        let base = base().unwrap();
        // The slot's launch commits. Refusing the rejection's first write and
        // the dispatcher's settlement of the occurrence leaves it there.
        conn.execute_batch(
            "CREATE TEMP TRIGGER refuse_rejection BEFORE UPDATE OF status ON main.sessions
             WHEN NEW.status = 'failed' BEGIN SELECT RAISE(ABORT, 'refused'); END;
             CREATE TEMP TRIGGER refuse_settlement
             BEFORE UPDATE OF state ON main.automation_occurrences
             WHEN NEW.state = 'failed' BEGIN SELECT RAISE(ABORT, 'refused'); END;",
        )
        .unwrap();
        let runtime = UnavailableOnceRuntime::default();
        dispatch(&conn, &runtime, at(base, CLAIM), &leadership.fence()).unwrap_err();
        conn.execute_batch(
            "DROP TRIGGER temp.refuse_rejection; DROP TRIGGER temp.refuse_settlement;",
        )
        .unwrap();
        assert_eq!(retry(&conn).unwrap(), Retry::Launching);
        let step = |sql: &str, whole: Option<Retry>| {
            conn.execute_batch(sql).unwrap();
            let now = retry(&conn).unwrap();
            match whole {
                Some(whole) => assert_eq!(now, whole, "{sql}"),
                None => assert!(!left.contains(&now), "{sql}: {now:?}"),
            }
        };
        let (claim_at, retry_at) = (iso(at(base, CLAIM)), iso(at(base, RETRY)));
        let lease = iso(at(base, RETRY + 600));
        // Each transaction's writes in the scheduler's order: the rejection,
        // the re-claim, the retry's launch and its ownership. The store holds
        // one occurrence and run; the writes read them.
        conn.execute_batch("BEGIN").unwrap();
        step(
            &format!("UPDATE sessions SET status = 'failed', updated_at = '{claim_at}'"),
            None,
        );
        step(
            &format!(
                "UPDATE automation_attempts
                 SET state = 'failed', failure_class = 'runtime_unavailable',
                     state_reason = '{UNAVAILABLE}', settled_at = '{claim_at}'"
            ),
            None,
        );
        step(
            &format!(
                "INSERT INTO automation_attempts
                    (id, run_id, occurrence_id, attempt_number, adoption_key,
                     occurrence_fence_generation, dispatch_generation, state,
                     prior_attempt_number, prior_disposition, retry_classification,
                     not_before, opened_at)
                 SELECT 'retry-2', a.run_id, a.occurrence_id, 2, 'retry-2', o.attempt, 0,
                        'adopted', 1, 'failed', 'automatic_retry', '{retry_at}', '{claim_at}'
                 FROM automation_attempts AS a
                 JOIN automation_occurrences AS o ON o.id = a.occurrence_id"
            ),
            None,
        );
        step(
            &format!(
                "UPDATE automation_occurrences
                 SET state = 'planned', lease_owner = NULL, lease_expires_at = NULL,
                     failure_reason = '{UNAVAILABLE}', updated_at = '{claim_at}'"
            ),
            None,
        );
        step(
            "UPDATE automation_runs SET session_id = NULL",
            Some(Retry::Scheduled),
        );
        step(
            &format!(
                "UPDATE automation_occurrences
                 SET state = 'claimed', lease_owner = 'daemon', lease_expires_at = '{lease}',
                     attempt = attempt + 1, updated_at = '{retry_at}'"
            ),
            None,
        );
        step(
            "UPDATE automation_attempts
             SET occurrence_fence_generation = (SELECT attempt FROM automation_occurrences)
             WHERE attempt_number = 2",
            Some(Retry::Reclaimed),
        );
        step(
            &format!(
                "INSERT INTO sessions
                    (id, project_root, harness, title, status, created_at, updated_at)
                 SELECT 'retry-session', project_root, harness, title, 'created',
                        '{retry_at}', '{retry_at}'
                 FROM sessions"
            ),
            None,
        );
        step(
            "UPDATE automation_attempts
             SET state = 'dispatching', dispatch_generation = dispatch_generation + 1
             WHERE attempt_number = 2",
            None,
        );
        step(
            "UPDATE automation_runs SET session_id = 'retry-session'",
            Some(Retry::Relaunching),
        );
        step(
            "UPDATE sessions SET status = 'running' WHERE id = 'retry-session'",
            None,
        );
        step(
            "UPDATE automation_attempts SET state = 'started', session_id = 'retry-session'
             WHERE attempt_number = 2",
            None,
        );
        step(
            "UPDATE automation_occurrences SET state = 'running'",
            Some(Retry::Retried),
        );
        conn.execute_batch("ROLLBACK").unwrap();
        assert_eq!(retry(&conn).unwrap(), Retry::Launching);
    }

    #[test]
    fn worker_refuses_invalid_or_weakened_input() {
        let mut case = vectors()["cases"][0].clone();
        case["storePath"] = json!("private-input");
        assert_eq!(worker(&case).unwrap_err(), INVALID);
        let mut case = vectors()["cases"][3].clone();
        case["expected"]["restartIsNoop"] = json!(false);
        assert_eq!(worker(&case).unwrap_err(), INVALID);
        let mut case = vectors()["cases"][10].clone();
        case["boundary"] = json!("after_ownership_commit");
        assert_eq!(worker(&case).unwrap_err(), INVALID);
    }
}
