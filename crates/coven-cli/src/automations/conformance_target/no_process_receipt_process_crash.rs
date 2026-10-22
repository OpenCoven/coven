//! Disposable no-process receipt crash fixtures: a daemon worker claims a due
//! daily slot and launches it while it shuts down. Its runtime refuses the
//! launch as a closing runtime does, recording the receipt that proves no
//! process started, and the dispatcher restores the launch for retry. The
//! worker is killed after the receipt, inside the restoration or after it
//! commits. The restarted daemon must launch the slot exactly once, through
//! the receipt-proven recovery its retry policy selects. Every launch goes
//! through a no-op runtime; no runtime process starts.
use std::collections::BTreeSet;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::command_process_crash_recovery::{
    abruptly_terminated, announce, spawn_worker, CHILD_GUARD, SCRATCH_ENV,
};
use super::process_crash_fixture::{
    install_pause, open_raw, park, raw_integrity_ok, worker_home, write_transaction_open, Pause,
};
use super::result_delivery_recovery::{at, iso, BASE};
use super::run_launch_process_crash::{pass, restart, snapshot_tables, CountingRuntime, REFUSED};
use super::valid_case_id;
use crate::automations::definition::RoutineDefinition;
use crate::automations::leadership::SchedulerLeadership;

const INVALID: &str = "conformance vector is invalid";
const FAILED: &str = "conformance suite execution failed";
const SCHEMA_VERSION: &str =
    "coven.automations.no-process-receipt-process-crash-recovery-vectors.v1";
const WORKER: &str = "no-process-receipt-crash-worker";
const HOME_PREFIX: &str = "receipt-crash-";
const AUTOMATION_ID: &str = "conformance-receipt-crash";
/// The definition predates the slot the worker claims.
const DEFINED_AT: &str = "2099-10-19T09:00:00.000Z";
/// Seconds after `BASE`, the slot: the worker claims and launches it; the
/// parent restarts the daemon; a pass runs after any retry's 60-second delay;
/// the launched session ends cleanly; a pass settles its run; and a further
/// pass changes nothing.
const CLAIM: i64 = 30;
const RESTART: i64 = 150;
/// A receipt-proven attempt's retry is due the policy's 60 seconds after the
/// restart settles it.
const RETRY_DUE: i64 = RESTART + 60;
const RETRY_PASS: i64 = 220;
const SESSION_END: i64 = 230;
const SETTLE_PASS: i64 = 240;
const REPLAY_PASS: i64 = 300;
/// The reason a receipt-proven attempt settles with, and its replanned
/// occurrence carries.
const PROVEN: &str = "claim lease expired with proof that no process started";

/// The routine's retry policy: none, or one that retries a lease expiry.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
enum Scene {
    NoRetry,
    LeaseRetry,
}

impl Scene {
    const ALL: [Self; 2] = [Self::NoRetry, Self::LeaseRetry];
}

/// Where the worker can stop: after its runtime records the receipt, inside
/// the dispatcher's restoration and after it commits, and the uninterrupted
/// control.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
enum Boundary {
    AfterReceipt,
    AfterRunDelete,
    AfterRestore,
    AfterRestoreCommit,
    None,
}

impl Boundary {
    const ALL: [Self; 5] = [
        Self::AfterReceipt,
        Self::AfterRunDelete,
        Self::AfterRestore,
        Self::AfterRestoreCommit,
        Self::None,
    ];

    /// What the store has committed when the worker stops.
    fn committed(self) -> Launch {
        match self {
            Self::AfterReceipt | Self::AfterRunDelete | Self::AfterRestore => Launch::Launching,
            Self::AfterRestoreCommit | Self::None => Launch::Restored,
        }
    }

    /// The worker stops inside a write transaction.
    fn inside_transaction(self) -> bool {
        matches!(self, Self::AfterRunDelete | Self::AfterRestore)
    }
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Case {
    case_id: String,
    scene: Scene,
    boundary: Boundary,
    definition: Value,
    expected: Expected,
}

#[derive(Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Expected {
    initial_outcome: InitialOutcome,
    launch_after_exit: Launch,
    receipt_after_exit: bool,
    launch_after_restart: Launch,
    restart_launches: u64,
    launch_after_retry_pass: Launch,
    launch_after_settle_pass: Launch,
    claims: u64,
    launches: u64,
    stops: u64,
    integrity_ok: bool,
    replay_is_noop: bool,
}

#[derive(Debug, Deserialize, Serialize, PartialEq, Clone, Copy)]
#[serde(rename_all = "snake_case")]
enum InitialOutcome {
    Terminated,
    Restored,
}

/// The store's one occurrence, read with its runs, attempts and sessions.
#[derive(Debug, Deserialize, Serialize, PartialEq, Clone, Copy)]
#[serde(rename_all = "snake_case")]
enum Launch {
    /// Claimed under the `daemon` lease with its run `running`, its first
    /// attempt `dispatching` with no session of its own, and the run's
    /// session `created`.
    Launching,
    /// Back to `planned` with no lease and no run, attempt or session.
    Restored,
    /// A receipt-proven first attempt `failed` for `lease_expired`, its
    /// session `failed`; the second attempt waits on the run, which holds no
    /// session, and the occurrence is `planned` with the proof as its reason.
    RetryWaiting,
    /// Launched once under the `daemon` lease: the occurrence and its run
    /// `running`, and the run's one attempt `started` on its `running`
    /// session.
    Owned,
    /// As `Owned`, but the run's second attempt is `started`, after a
    /// receipt-proven first attempt, and the occurrence keeps the proof as its
    /// reason, as an automatic retry keeps the failure it retries.
    RetryOwned,
    /// The occurrence, its run and the run's one attempt `succeeded`, with no
    /// lease and the session `completed`.
    Succeeded,
    /// As `Succeeded`, but by the run's second attempt, after a
    /// receipt-proven first attempt.
    RetrySucceeded,
    /// Anything else.
    Mixed,
}

/// What each scene and boundary must produce. A vector restates it, so it
/// cannot weaken it.
fn expected_for(scene: Scene, boundary: Boundary) -> Expected {
    let committed = boundary.committed();
    // A launch the dispatcher restored is claimed and launched again at the
    // restart. One the worker left launching is recovered by its receipt:
    // restored and launched again, or, under a lease-expiry retry policy,
    // settled and retried after the policy's delay.
    let retried = scene == Scene::LeaseRetry && committed == Launch::Launching;
    let (launch_after_restart, restart_launches, launch_after_retry_pass, launch_after_settle_pass) =
        if retried {
            (
                Launch::RetryWaiting,
                0,
                Launch::RetryOwned,
                Launch::RetrySucceeded,
            )
        } else {
            (Launch::Owned, 1, Launch::Owned, Launch::Succeeded)
        };
    Expected {
        initial_outcome: if boundary == Boundary::None {
            InitialOutcome::Restored
        } else {
            InitialOutcome::Terminated
        },
        launch_after_exit: committed,
        receipt_after_exit: true,
        launch_after_restart,
        restart_launches,
        launch_after_retry_pass,
        launch_after_settle_pass,
        claims: 2,
        launches: 1,
        stops: 0,
        integrity_ok: true,
        replay_is_noop: true,
    }
}

/// A daily routine at 09:00 UTC with a working directory and a 30-minute
/// deadline, and the scene's retry policy. The runner launches only a routine
/// with a working directory; a no-op runtime never touches it.
fn synthetic_definition(scene: Scene) -> Value {
    let mut definition = json!({"schemaVersion":1,"id":AUTOMATION_ID,
        "name":"Conformance no-process receipt crash","status":"ACTIVE",
        "rrule":"FREQ=DAILY;BYHOUR=9","timezone":"utc","misfire":"latest",
        "overlap":"forbid","timeoutMinutes":30,"runtime":"coven-code",
        "prompt":"Run the native no-process receipt crash conformance probe.",
        "cwd":"/work/project"});
    if scene == Scene::LeaseRetry {
        definition["retry"] = json!({"maxAttempts":2,"backoffPolicy":"fixed",
            "backoffSeconds":60,"retryableClasses":["lease_expired"]});
    }
    definition
}

fn validate(case: &Case) -> Result<(), &'static str> {
    if !valid_case_id(&case.case_id)
        || case.definition != synthetic_definition(case.scene)
        || case.expected != expected_for(case.scene, case.boundary)
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

/// Where the worker parks inside SQLite once the tick has claimed the slot.
/// The dispatcher commits the launch, then restores it in a second
/// transaction that deletes the run with its attempt, then the session, and
/// returns the occurrence to `planned` last.
fn pause_at(boundary: Boundary) -> Option<Pause<'static>> {
    match boundary {
        Boundary::AfterRunDelete => Some(Pause::After("AFTER DELETE ON main.automation_runs")),
        Boundary::AfterRestore => Some(Pause::After(
            "AFTER UPDATE OF state ON main.automation_occurrences
             WHEN OLD.state = 'claimed' AND NEW.state = 'planned'",
        )),
        Boundary::AfterRestoreCommit => Some(Pause::AfterCommits(2)),
        Boundary::AfterReceipt | Boundary::None => None,
    }
}

/// The worker's runtime: a daemon that has begun to shut down. It refuses the
/// launch as the closing runtime does, after recording the receipt that no
/// process started at the path the restart reads, and from then on the
/// dispatcher sees the shutdown. With a stop, it stops once the receipt is
/// written.
struct ClosingRuntime<'a> {
    home: &'a Path,
    closed: AtomicBool,
    stop: Option<Stop>,
}

/// Where the closing runtime stops after it records the receipt.
enum Stop {
    /// Announces the handshake and parks until the parent kills the worker.
    Park(Value),
    /// Unwinds the dispatch without running the panic hook, leaving what a
    /// kill there leaves; a unit test catches it.
    #[cfg(test)]
    Unwind,
}

impl crate::api::SessionRuntime for ClosingRuntime<'_> {
    fn launch_session(&self, launch: &crate::api::SessionLaunch) -> anyhow::Result<()> {
        let receipt = crate::automations::runner::containment_receipt_path(self.home, &launch.id);
        if let Some(directory) = receipt.parent() {
            std::fs::create_dir_all(directory)?;
        }
        crate::pty_runner::write_containment_receipt(
            &receipt,
            crate::pty_runner::CONTAINMENT_NO_PROCESS_RECEIPT,
        )?;
        self.closed.store(true, Ordering::SeqCst);
        match &self.stop {
            Some(Stop::Park(handshake)) => park(handshake),
            #[cfg(test)]
            Some(Stop::Unwind) => std::panic::resume_unwind(Box::new(Boundary::AfterReceipt)),
            None => {}
        }
        Err(anyhow::Error::new(
            crate::api::RuntimeLaunchAdmissionClosedError,
        ))
    }

    fn send_input(&self, _session_id: &str, _payload: &Value) -> anyhow::Result<()> {
        Ok(())
    }

    fn kill_session(&self, _session_id: &str) -> anyhow::Result<()> {
        Ok(())
    }
}

/// Stores the synthetic routine, defined before the slot it claims.
fn store_definition(conn: &Connection, definition: &Value) -> Result<(), &'static str> {
    let definition = RoutineDefinition::from_json(definition).map_err(|_| INVALID)?;
    crate::automations::store::insert_definition(conn, &definition).map_err(|_| FAILED)?;
    conn.execute(
        "UPDATE automation_definitions SET created_at = ?1, updated_at = ?1 WHERE id = ?2",
        [DEFINED_AT, definition.id.as_str()],
    )
    .map_err(|_| FAILED)?;
    Ok(())
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
    store_definition(&conn, &case.definition)?;
    // The daemon's scheduler claims and dispatches under its leadership fence.
    let leadership = SchedulerLeadership::acquire(&home, &conn, claim_at).map_err(|_| FAILED)?;
    let fence = leadership.fence();
    let tick = crate::automations::occurrences::tick_with_scheduler_fence(&conn, claim_at, &fence)
        .map_err(|_| FAILED)?;
    if tick.planned.len() != 1 || tick.claimed.len() != 1 || !tick.failed.is_empty() {
        return Err(FAILED);
    }
    let handshake =
        json!({"state":"paused", "boundary":case.boundary, "relativeHome":relative_home});
    if let Some(pause_at) = pause_at(case.boundary) {
        install_pause(&conn, pause_at, handshake.clone())?;
    }
    let runtime = ClosingRuntime {
        home: &home,
        closed: AtomicBool::new(false),
        stop: (case.boundary == Boundary::AfterReceipt).then_some(Stop::Park(handshake)),
    };
    let report =
        crate::automations::runner::dispatch_claimed_occurrences_with_clock_and_cancel_and_scheduler(
            &conn,
            &runtime,
            claim_at,
            || claim_at,
            || runtime.closed.load(Ordering::SeqCst),
            &fence,
        )
        .map_err(|_| FAILED)?;
    if case.boundary != Boundary::None
        || !report.dispatched.is_empty()
        || !report.failed.is_empty()
        || launch(&conn)? != Launch::Restored
    {
        return Err(FAILED);
    }
    // Exits like a daemon whose shutdown restored the launch, then died before
    // releasing its leadership.
    drop(conn);
    drop(leadership);
    announce(&json!({"state":"restored", "boundary":case.boundary, "relativeHome":relative_home}))
}

/// One attempt as the classes read it.
struct AttemptRow {
    state: String,
    failure_class: Option<String>,
    reason: Option<String>,
    /// `none`, the run's session `run`, or `other`.
    session: String,
    settled: bool,
    not_before: String,
}

/// Classifies the store's one occurrence with its runs, attempts and
/// sessions. A stop fence, or a retry exhaustion or quarantine recorded for
/// the routine, makes any store mixed.
fn launch(conn: &Connection) -> Result<Launch, &'static str> {
    let counts: [i64; 5] = conn
        .query_row(
            "SELECT
                (SELECT COUNT(*) FROM automation_occurrences),
                (SELECT COUNT(*) FROM automation_runs),
                (SELECT COUNT(*) FROM automation_attempts),
                (SELECT COUNT(*) FROM sessions),
                (SELECT COUNT(*) FROM automation_stop_fences)
                    + (SELECT COUNT(*) FROM automation_retry_state
                       WHERE consecutive_exhaustions > 0 OR quarantined_at IS NOT NULL)",
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
    if counts[0] != 1 || counts[4] != 0 {
        return Ok(Launch::Mixed);
    }
    let occurrence: Option<(String, String, Option<String>)> = conn
        .query_row(
            "SELECT state,
                    CASE
                        WHEN lease_owner = 'daemon' AND lease_expires_at IS NOT NULL
                            THEN 'daemon'
                        WHEN lease_owner IS NULL AND lease_expires_at IS NULL THEN 'none'
                        ELSE 'other'
                    END,
                    failure_reason
             FROM automation_occurrences
             WHERE automation_id = ?1 AND kind = 'scheduled'",
            [AUTOMATION_ID],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(|_| FAILED)?;
    let Some((state, lease, reason)) = occurrence else {
        return Ok(Launch::Mixed);
    };
    let occurrence = (state.as_str(), lease.as_str(), reason.as_deref());
    if counts[1..4] == [0, 0, 0] {
        return Ok(if occurrence == ("planned", "none", None) {
            Launch::Restored
        } else {
            Launch::Mixed
        });
    }
    if counts[1] != 1 {
        return Ok(Launch::Mixed);
    }
    let run: (String, bool, Option<String>) = conn
        .query_row(
            "SELECT r.status, r.finished_at IS NOT NULL, s.status
             FROM automation_runs AS r
             JOIN automation_occurrences AS o ON o.id = r.occurrence_id
             LEFT JOIN sessions AS s ON s.id = r.session_id",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(|_| FAILED)?;
    let attempts: Vec<AttemptRow> = conn
        .prepare(
            "SELECT a.state, a.failure_class, a.state_reason,
                    CASE
                        WHEN a.session_id IS NULL THEN 'none'
                        WHEN a.session_id = r.session_id THEN 'run'
                        ELSE 'other'
                    END,
                    a.settled_at IS NOT NULL, a.not_before
             FROM automation_attempts AS a
             JOIN automation_runs AS r ON r.id = a.run_id
             WHERE a.occurrence_id = r.occurrence_id
             ORDER BY a.attempt_number",
        )
        .and_then(|mut statement| {
            statement
                .query_map([], |row| {
                    Ok(AttemptRow {
                        state: row.get(0)?,
                        failure_class: row.get(1)?,
                        reason: row.get(2)?,
                        session: row.get(3)?,
                        settled: row.get(4)?,
                        not_before: row.get(5)?,
                    })
                })?
                .collect()
        })
        .map_err(|_| FAILED)?;
    let numbers: Vec<i64> = conn
        .prepare("SELECT attempt_number FROM automation_attempts ORDER BY attempt_number")
        .and_then(|mut statement| statement.query_map([], |row| row.get(0))?.collect())
        .map_err(|_| FAILED)?;
    let sessions: Vec<String> = conn
        .prepare("SELECT status FROM sessions ORDER BY status")
        .and_then(|mut statement| statement.query_map([], |row| row.get(0))?.collect())
        .map_err(|_| FAILED)?;
    // A receipt-proven first attempt, and the run's current attempt.
    let (proven, current) = match attempts.as_slice() {
        [current] if numbers == [1] => (false, current),
        [first, current] if numbers == [1, 2] => {
            let proven = first.state == "failed"
                && first.failure_class.as_deref() == Some("lease_expired")
                && first.reason.as_deref() == Some(PROVEN)
                && first.settled
                && first.session == "none";
            if !proven {
                return Ok(Launch::Mixed);
            }
            (true, current)
        }
        _ => return Ok(Launch::Mixed),
    };
    // A retry is due the policy's delay after the restart that settled the
    // proven attempt.
    if proven && current.not_before != iso(at(base()?, RETRY_DUE)) {
        return Ok(Launch::Mixed);
    }
    let sessions: Vec<&str> = sessions.iter().map(String::as_str).collect();
    let attempt = (
        current.state.as_str(),
        current.session.as_str(),
        current.settled,
    );
    let run = (run.0.as_str(), run.1, run.2.as_deref());
    Ok(
        match (proven, occurrence, run, attempt, sessions.as_slice()) {
            (
                false,
                ("claimed", "daemon", None),
                ("running", false, Some("created")),
                ("dispatching", "none", false),
                ["created"],
            ) => Launch::Launching,
            (
                true,
                ("planned", "none", Some(PROVEN)),
                ("running", false, None),
                ("adopted", "none", false),
                ["failed"],
            ) => Launch::RetryWaiting,
            (
                false,
                ("running", "daemon", None),
                ("running", false, Some("running")),
                ("started", "run", false),
                ["running"],
            ) => Launch::Owned,
            (
                true,
                ("running", "daemon", Some(PROVEN)),
                ("running", false, Some("running")),
                ("started", "run", false),
                ["failed", "running"],
            ) => Launch::RetryOwned,
            (
                false,
                ("succeeded", "none", None),
                ("succeeded", true, Some("completed")),
                ("succeeded", "run", true),
                ["completed"],
            ) => Launch::Succeeded,
            (
                true,
                ("succeeded", "none", None),
                ("succeeded", true, Some("completed")),
                ("succeeded", "run", true),
                ["completed", "failed"],
            ) => Launch::RetrySucceeded,
            _ => Launch::Mixed,
        },
    )
}

/// Whether the home's receipt directory holds exactly one receipt, with the
/// exact no-process bytes the closing runtime records. Its name is not matched
/// against a session: a restored launch's session is gone. The restart looks a
/// receipt up by its session, so a misnamed one makes the recovery mixed.
fn receipt_recorded(home: &Path) -> Result<bool, &'static str> {
    let directory = crate::automations::runner::containment_receipt_path(home, "probe")
        .parent()
        .ok_or(FAILED)?
        .to_path_buf();
    let receipts: Vec<_> = match std::fs::read_dir(&directory) {
        Ok(entries) => entries.collect::<Result<_, _>>().map_err(|_| FAILED)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(_) => return Err(FAILED),
    };
    let [receipt] = receipts.as_slice() else {
        return Ok(false);
    };
    Ok(std::fs::read(receipt.path()).map_err(|_| FAILED)?
        == crate::pty_runner::CONTAINMENT_NO_PROCESS_RECEIPT)
}

/// The occurrence's claim count.
fn claims(conn: &Connection) -> Result<u64, &'static str> {
    let claims: i64 = conn
        .query_row("SELECT attempt FROM automation_occurrences", [], |row| {
            row.get(0)
        })
        .map_err(|_| FAILED)?;
    u64::try_from(claims).map_err(|_| FAILED)
}

/// Every occurrence, run, attempt, event, session, stop-fence and retry-state
/// row, for an exact before/after comparison.
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
            "automation_retry_state",
        ],
    )
}

/// Ends the launched session the way a runtime reports a clean exit.
fn end_session(conn: &Connection, now: DateTime<Utc>) -> Result<(), &'static str> {
    let session_id: String = conn
        .query_row(
            "SELECT id FROM sessions WHERE status = 'running'",
            [],
            |row| row.get(0),
        )
        .map_err(|_| FAILED)?;
    let ended = crate::store::update_session_terminal_if_active(
        conn,
        &session_id,
        "completed",
        Some(0),
        &iso(now),
    )
    .map_err(|_| FAILED)?;
    if !ended {
        return Err(FAILED);
    }
    Ok(())
}

/// What the restarted daemon and its passes did with the store the worker
/// left.
struct Recovery {
    launch_after_restart: Launch,
    restart_launches: u64,
    launch_after_retry_pass: Launch,
    launch_after_settle_pass: Launch,
    claims: u64,
    launches: u64,
    stops: u64,
    replay_is_noop: bool,
}

fn recover(home: &Path, path: &Path) -> Result<Recovery, &'static str> {
    let base = base()?;
    // One runtime counts every stop and launch for the restart and each pass.
    let runtime = CountingRuntime::default();
    let leadership = restart(home, &runtime, at(base, RESTART))?;
    let conn = crate::store::open_store(path).map_err(|_| FAILED)?;
    let launch_after_restart = launch(&conn)?;
    let restart_launches = runtime.launches();
    drop(conn);
    pass(
        home,
        &runtime,
        &leadership,
        at(base, RETRY_PASS),
        "deadline",
    )?;
    let conn = crate::store::open_store(path).map_err(|_| FAILED)?;
    let launch_after_retry_pass = launch(&conn)?;
    if matches!(launch_after_retry_pass, Launch::Owned | Launch::RetryOwned) {
        end_session(&conn, at(base, SESSION_END))?;
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
    let launch_after_settle_pass = launch(&conn)?;
    let claims = claims(&conn)?;
    let launches = runtime.launches();
    let stops = runtime.stops();
    let before_replay = snapshot(&conn)?;
    drop(conn);
    // A further pass must change nothing.
    pass(
        home,
        &runtime,
        &leadership,
        at(base, REPLAY_PASS),
        "deadline",
    )?;
    let conn = crate::store::open_store(path).map_err(|_| FAILED)?;
    let replay_is_noop = snapshot(&conn)? == before_replay
        && runtime.stops() == stops
        && runtime.launches() == launches;
    drop(conn);
    drop(leadership);
    Ok(Recovery {
        launch_after_restart,
        restart_launches,
        launch_after_retry_pass,
        launch_after_settle_pass,
        claims,
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
    // Every scene with every boundary, once each.
    if vectors.schema_version != SCHEMA_VERSION
        || vectors.cases.len() != Scene::ALL.len() * Boundary::ALL.len()
    {
        return Err(INVALID);
    }
    let mut ids = BTreeSet::new();
    let mut pairs = BTreeSet::new();
    for case in &vectors.cases {
        validate(case)?;
        if !ids.insert(&case.case_id) || !pairs.insert((case.scene, case.boundary)) {
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
/// worker's write transaction is still open, what it has committed, and
/// whether the receipt is recorded.
fn observe_pause(home: &Path, path: &Path) -> Result<(bool, Launch, bool), &'static str> {
    let probe = open_raw(path)?;
    let open = write_transaction_open(&probe)?;
    Ok((open, launch(&probe)?, receipt_recorded(home)?))
}

fn case_matches(case: &Case) -> Result<(bool, Value), &'static str> {
    let root = super::scheduler_conformance_tempdir()?;
    crate::daemon::ensure_private_coven_home(root.path()).map_err(|_| FAILED)?;
    let payload = serde_json::to_vec(case).map_err(|_| INVALID)?;
    let (mut process, receiver) = spawn_worker::<Handshake>(WORKER, payload, root.path())?;
    let ready = receiver.recv_timeout(CHILD_GUARD).map_err(|_| FAILED)??;
    let crash = case.boundary != Boundary::None;
    if ready.boundary != case.boundary || ready.state != if crash { "paused" } else { "restored" } {
        return Err(FAILED);
    }
    let home = worker_home(root.path(), &ready.relative_home, HOME_PREFIX)?;
    let path = crate::api::store_path(&home);
    let pause = crash.then(|| observe_pause(&home, &path)).transpose()?;
    let pause_consistent = pause.is_none_or(|(open, committed, receipt)| {
        open == case.boundary.inside_transaction()
            && committed == case.expected.launch_after_exit
            && receipt
    });
    let status = process.finish(crash)?;
    let terminated = abruptly_terminated(&status);
    if crash != terminated || (!crash && !status.success()) {
        return Err(FAILED);
    }
    drop(process);
    let integrity_ok = raw_integrity_ok(&path)?;

    let launch_after_exit = {
        let conn = crate::store::open_store(&path).map_err(|_| FAILED)?;
        launch(&conn)?
    };
    let receipt_after_exit = receipt_recorded(&home)?;
    let recovery = match recover(&home, &path) {
        Ok(recovery) => Some(recovery),
        Err(error) if error == REFUSED => None,
        Err(error) => return Err(error),
    };
    let recovery_refused = recovery.is_none();
    let initial_outcome = if terminated {
        InitialOutcome::Terminated
    } else {
        InitialOutcome::Restored
    };
    let observed = match &recovery {
        Some(recovery) => Expected {
            initial_outcome,
            launch_after_exit,
            receipt_after_exit,
            launch_after_restart: recovery.launch_after_restart,
            restart_launches: recovery.restart_launches,
            launch_after_retry_pass: recovery.launch_after_retry_pass,
            launch_after_settle_pass: recovery.launch_after_settle_pass,
            claims: recovery.claims,
            launches: recovery.launches,
            stops: recovery.stops,
            integrity_ok,
            replay_is_noop: recovery.replay_is_noop,
        },
        None => Expected {
            initial_outcome,
            launch_after_exit,
            receipt_after_exit,
            launch_after_restart: Launch::Mixed,
            restart_launches: 0,
            launch_after_retry_pass: Launch::Mixed,
            launch_after_settle_pass: Launch::Mixed,
            claims: 0,
            launches: 0,
            stops: 0,
            integrity_ok,
            replay_is_noop: false,
        },
    };
    let passed = observed == case.expected && pause_consistent && !recovery_refused;
    let (transaction_open_at_pause, launch_at_pause, receipt_at_pause) = match pause {
        Some((open, committed, receipt)) => (Some(open), Some(committed), Some(receipt)),
        None => (None, None, None),
    };
    Ok((
        passed,
        json!({
            "caseId": case.case_id,
            "scene": case.scene,
            "boundary": case.boundary,
            "initialOutcome": observed.initial_outcome,
            "abruptlyTerminated": terminated,
            "transactionOpenAtPause": transaction_open_at_pause,
            "launchAtPause": launch_at_pause,
            "receiptAtPause": receipt_at_pause,
            "pauseConsistent": pause_consistent,
            "integrityOk": integrity_ok,
            "launchAfterExit": observed.launch_after_exit,
            "receiptAfterExit": observed.receipt_after_exit,
            "recoveryRefused": recovery_refused,
            "launchAfterRestart": observed.launch_after_restart,
            "restartLaunches": observed.restart_launches,
            "launchAfterRetryPass": observed.launch_after_retry_pass,
            "launchAfterSettlePass": observed.launch_after_settle_pass,
            "claims": observed.claims,
            "launches": observed.launches,
            "stops": observed.stops,
            "replayIsNoop": observed.replay_is_noop,
        }),
    ))
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::path::PathBuf;

    use super::*;

    fn vectors() -> Value {
        serde_json::from_str(include_str!(
            "../../../../../conformance/automations/runner/no-process-receipt-process-crash-recovery.vectors.json"
        ))
        .unwrap()
    }

    thread_local! {
        /// The rows each WAL commit on this thread changed, in commit order.
        static COMMITS: RefCell<Vec<Vec<String>>> = const { RefCell::new(Vec::new()) };
        static CHANGES: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    }

    fn record_commit(_: &rusqlite::hooks::Wal, _: std::os::raw::c_int) -> rusqlite::Result<()> {
        let changes = CHANGES.with(|changes| changes.take());
        COMMITS.with(|commits| commits.borrow_mut().push(changes));
        Ok(())
    }

    /// A private home whose store holds the scene's routine, claimed by the
    /// tick under the returned leadership.
    fn claimed_store(
        scene: Scene,
    ) -> (tempfile::TempDir, PathBuf, Connection, SchedulerLeadership) {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        crate::daemon::ensure_private_coven_home(&home).unwrap();
        let conn = crate::store::open_store(&crate::api::store_path(&home)).unwrap();
        store_definition(&conn, &synthetic_definition(scene)).unwrap();
        let claim_at = at(base().unwrap(), CLAIM);
        let leadership = SchedulerLeadership::acquire(&home, &conn, claim_at).unwrap();
        let tick = crate::automations::occurrences::tick_with_scheduler_fence(
            &conn,
            claim_at,
            &leadership.fence(),
        )
        .unwrap();
        assert_eq!((tick.planned.len(), tick.claimed.len()), (1, 1));
        (temp, home, conn, leadership)
    }

    fn dispatch(
        home: &Path,
        conn: &Connection,
        leadership: &SchedulerLeadership,
        stop: Option<Stop>,
    ) -> Result<crate::automations::runner::DispatchReport, String> {
        let claim_at = at(base().unwrap(), CLAIM);
        let runtime = ClosingRuntime {
            home,
            closed: AtomicBool::new(false),
            stop,
        };
        crate::automations::runner::dispatch_claimed_occurrences_with_clock_and_cancel_and_scheduler(
            conn,
            &runtime,
            claim_at,
            || claim_at,
            || runtime.closed.load(Ordering::SeqCst),
            &leadership.fence(),
        )
    }

    #[test]
    fn receipt_crash_vectors_restate_each_scene_and_boundary_outcome() {
        let vectors: VectorSet = serde_json::from_value(vectors()).unwrap();
        assert_eq!(vectors.schema_version, SCHEMA_VERSION);
        for case in &vectors.cases {
            validate(case).unwrap();
        }
        let pairs: Vec<_> = vectors
            .cases
            .iter()
            .map(|case| (case.scene, case.boundary))
            .collect();
        let all: Vec<_> = Scene::ALL
            .iter()
            .flat_map(|scene| Boundary::ALL.iter().map(|boundary| (*scene, *boundary)))
            .collect();
        assert_eq!(pairs, all);
    }

    #[test]
    fn receipt_crash_vectors_require_each_scene_boundary_and_control() {
        let original = vectors();
        let mut missing = original.clone();
        missing["cases"].as_array_mut().unwrap().pop();
        assert_eq!(evaluate(&missing).unwrap_err(), INVALID);
        // A second control under its own case id, replacing a boundary.
        let mut duplicate = original.clone();
        let mut control = duplicate["cases"][9].clone();
        control["caseId"] = json!("second-lease-retry-uninterrupted-shutdown-control");
        duplicate["cases"][5] = control;
        assert_eq!(evaluate(&duplicate).unwrap_err(), INVALID);
        let mut duplicate_id = original;
        duplicate_id["cases"][1]["caseId"] = duplicate_id["cases"][0]["caseId"].clone();
        assert_eq!(evaluate(&duplicate_id).unwrap_err(), INVALID);
    }

    #[test]
    fn receipt_crash_vectors_refuse_malformed_or_weakened_inputs() {
        let original = vectors();
        for (case, field, value) in [
            (0, "launchAfterExit", json!("restored")),
            (0, "receiptAfterExit", json!(false)),
            (0, "restartLaunches", json!(0)),
            (1, "launchAfterRestart", json!("launching")),
            (2, "launches", json!(2)),
            (3, "launchAfterExit", json!("launching")),
            (4, "initialOutcome", json!("terminated")),
            (4, "claims", json!(1)),
            (5, "launchAfterRestart", json!("owned")),
            (5, "restartLaunches", json!(1)),
            (6, "launchAfterRetryPass", json!("owned")),
            (7, "launchAfterSettlePass", json!("succeeded")),
            (8, "launchAfterRestart", json!("retry_waiting")),
            (9, "stops", json!(1)),
            (9, "integrityOk", json!(false)),
            (9, "replayIsNoop", json!(false)),
        ] {
            let mut invalid = original.clone();
            invalid["cases"][case]["expected"][field] = value;
            assert_eq!(evaluate(&invalid).unwrap_err(), INVALID, "{case} {field}");
        }
        for (case, field, value) in [
            (0, "name", json!("Caller chosen")),
            (0, "status", json!("PAUSED")),
            (0, "cwd", json!("/private-input")),
            (
                0,
                "retry",
                json!({"maxAttempts":2,"backoffPolicy":"fixed",
                "backoffSeconds":60,"retryableClasses":["lease_expired"]}),
            ),
            (
                5,
                "retry",
                json!({"maxAttempts":3,"backoffPolicy":"fixed",
                "backoffSeconds":60,"retryableClasses":["lease_expired"]}),
            ),
        ] {
            let mut invalid = original.clone();
            invalid["cases"][case]["definition"][field] = value;
            assert_eq!(
                evaluate(&invalid).unwrap_err(),
                INVALID,
                "definition {case} {field}"
            );
        }
        for (field, value) in [
            ("storePath", json!("private-input")),
            ("scene", json!("never")),
            ("boundary", json!("after_launch")),
            ("caseId", json!("")),
        ] {
            let mut invalid = original.clone();
            invalid["cases"][0][field] = value;
            assert_eq!(evaluate(&invalid).unwrap_err(), INVALID, "{field}");
        }
        let mut invalid = original;
        invalid["schemaVersion"] =
            json!("coven.automations.tick-process-crash-recovery-vectors.v1");
        assert_eq!(evaluate(&invalid).unwrap_err(), INVALID);
    }

    #[test]
    fn the_dispatcher_commits_the_launch_then_restores_it_in_a_second_transaction() {
        let (_temp, home, conn, leadership) = claimed_store(Scene::NoRetry);
        COMMITS.with(|commits| commits.take());
        CHANGES.with(|changes| changes.take());
        conn.update_hook(Some(
            |action: rusqlite::hooks::Action, _: &str, table: &str, _: i64| {
                if table.starts_with("automation_event") {
                    return;
                }
                let action = match action {
                    rusqlite::hooks::Action::SQLITE_INSERT => "insert",
                    rusqlite::hooks::Action::SQLITE_UPDATE => "update",
                    _ => "delete",
                };
                CHANGES.with(|changes| changes.borrow_mut().push(format!("{action} {table}")));
            },
        ))
        .unwrap();
        conn.wal_hook(Some(record_commit));
        let report = dispatch(&home, &conn, &leadership, None).unwrap();
        conn.wal_hook(None);
        assert!(report.dispatched.is_empty() && report.failed.is_empty());
        assert_eq!(
            COMMITS.with(|commits| commits.take()),
            [
                vec![
                    "insert sessions",
                    "insert automation_runs",
                    "insert automation_attempts",
                    "update automation_attempts",
                ],
                vec![
                    "delete automation_runs",
                    "delete automation_attempts",
                    "delete sessions",
                    "update automation_occurrences",
                ],
            ]
        );
        assert_eq!(launch(&conn).unwrap(), Launch::Restored);
        assert!(receipt_recorded(&home).unwrap());
    }

    /// The store a worker stopped at `boundary` leaves. A kill after the
    /// receipt, or inside the restoration, leaves the committed launch and the
    /// receipt: the runtime unwinds the dispatch there. A kill after the
    /// restoration commits leaves what the uninterrupted dispatch leaves.
    fn stopped_store(scene: Scene, boundary: Boundary) -> (tempfile::TempDir, PathBuf) {
        let (temp, home, conn, leadership) = claimed_store(scene);
        if boundary.committed() == Launch::Launching {
            let unwound = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                dispatch(&home, &conn, &leadership, Some(Stop::Unwind))
            }));
            assert!(unwound.is_err());
        } else {
            dispatch(&home, &conn, &leadership, None).unwrap();
        }
        assert_eq!(launch(&conn).unwrap(), boundary.committed(), "{boundary:?}");
        assert!(receipt_recorded(&home).unwrap());
        (temp, home)
    }

    #[test]
    fn each_boundary_store_recovers_as_its_vector_says() {
        for scene in Scene::ALL {
            for boundary in Boundary::ALL {
                let (_temp, home) = stopped_store(scene, boundary);
                let recovery = recover(&home, &crate::api::store_path(&home)).unwrap();
                let expected = expected_for(scene, boundary);
                assert_eq!(
                    (
                        recovery.launch_after_restart,
                        recovery.restart_launches,
                        recovery.launch_after_retry_pass,
                        recovery.launch_after_settle_pass,
                        recovery.claims,
                        recovery.launches,
                        recovery.stops,
                        recovery.replay_is_noop,
                    ),
                    (
                        expected.launch_after_restart,
                        expected.restart_launches,
                        expected.launch_after_retry_pass,
                        expected.launch_after_settle_pass,
                        expected.claims,
                        expected.launches,
                        expected.stops,
                        expected.replay_is_noop,
                    ),
                    "{scene:?} {boundary:?}"
                );
            }
        }
    }

    /// Each torn statement runs in a transaction that is rolled back. One may
    /// drop the attempts' terminal-immutability trigger first, to tear a
    /// settled attempt.
    fn assert_torn(conn: &Connection, whole: Launch, torn: &[&str]) {
        assert_eq!(launch(conn).unwrap(), whole);
        for statements in torn {
            conn.execute_batch("BEGIN").unwrap();
            conn.execute_batch(statements).unwrap();
            assert_eq!(launch(conn).unwrap(), Launch::Mixed, "{statements}");
            conn.execute_batch("ROLLBACK").unwrap();
        }
        assert_eq!(launch(conn).unwrap(), whole);
    }

    const UNLOCK: &str = "DROP TRIGGER automation_attempts_terminal_immutable;";
    const EXTRA_OCCURRENCE: &str = "INSERT INTO automation_occurrences
            (id, automation_id, scheduled_for, kind, state, created_at, updated_at)
        VALUES ('conformance-receipt-crash:extra', 'conformance-receipt-crash',
            '2099-10-22T09:00:00.000Z', 'scheduled', 'planned',
            '2099-10-20T09:00:00.000Z', '2099-10-20T09:00:00.000Z')";
    const EXHAUSTED: &str = "INSERT INTO automation_retry_state
            (automation_id, consecutive_exhaustions, updated_at)
        VALUES ('conformance-receipt-crash', 1, '2099-10-20T09:00:00.000Z')
        ON CONFLICT (automation_id) DO UPDATE SET consecutive_exhaustions = 1";
    const FENCED: &str = "INSERT INTO automation_stop_fences
        SELECT id, session_id, 'recovery', NULL, '2099-10-20T09:00:00.000Z',
            '2099-10-20T10:00:00.000Z'
        FROM automation_runs";

    /// Ends the launched session and runs the pass that settles its run.
    fn settle(home: &Path, runtime: &CountingRuntime, leadership: &SchedulerLeadership) {
        let base = base().unwrap();
        let conn = crate::store::open_store(&crate::api::store_path(home)).unwrap();
        end_session(&conn, at(base, SESSION_END)).unwrap();
        drop(conn);
        pass(home, runtime, leadership, at(base, SETTLE_PASS), "deadline").unwrap();
    }

    #[test]
    fn retried_launches_are_whole_or_mixed() {
        let base = base().unwrap();
        let (_temp, home) = stopped_store(Scene::LeaseRetry, Boundary::AfterReceipt);
        let path = crate::api::store_path(&home);
        let conn = crate::store::open_store(&path).unwrap();
        let launching = format!(
            "{UNLOCK} UPDATE automation_attempts SET state = 'adopted', dispatch_generation = 0"
        );
        assert_torn(
            &conn,
            Launch::Launching,
            &[
                "UPDATE sessions SET status = 'running'",
                "UPDATE automation_occurrences SET lease_owner = NULL, lease_expires_at = NULL",
                "UPDATE automation_runs SET session_id = NULL",
                "UPDATE automation_attempts SET session_id = (SELECT id FROM sessions)",
                &launching,
                EXTRA_OCCURRENCE,
                EXHAUSTED,
            ],
        );
        drop(conn);
        let runtime = CountingRuntime::default();
        let leadership = restart(&home, &runtime, at(base, RESTART)).unwrap();
        let conn = crate::store::open_store(&path).unwrap();
        let proven = [
            "failure_class = 'runtime_unavailable'",
            "state_reason = 'claim lease expired'",
            "settled_at = NULL",
            "session_id = (SELECT id FROM sessions WHERE status = 'failed')",
        ]
        .map(|change| {
            format!("{UNLOCK} UPDATE automation_attempts SET {change} WHERE attempt_number = 1")
        });
        let mut torn = vec![
            "UPDATE automation_occurrences SET failure_reason = NULL",
            "UPDATE automation_occurrences SET state = 'claimed', lease_owner = 'daemon',
                 lease_expires_at = '2099-10-20T10:00:00.000Z'",
            "UPDATE automation_attempts SET not_before = '2099-10-20T09:03:00.000Z'
             WHERE attempt_number = 2",
            "UPDATE automation_attempts SET session_id = (SELECT id FROM sessions)
             WHERE attempt_number = 2",
            "UPDATE sessions SET status = 'created'",
            EXTRA_OCCURRENCE,
            EXHAUSTED,
        ];
        torn.extend(proven.iter().map(String::as_str));
        assert_torn(&conn, Launch::RetryWaiting, &torn);
        drop(conn);
        pass(
            &home,
            &runtime,
            &leadership,
            at(base, RETRY_PASS),
            "deadline",
        )
        .unwrap();
        let conn = crate::store::open_store(&path).unwrap();
        let mut torn = vec![
            "UPDATE automation_occurrences SET failure_reason = NULL",
            "UPDATE automation_runs SET session_id = NULL",
            "UPDATE automation_attempts SET session_id = NULL WHERE attempt_number = 2",
            FENCED,
            EXHAUSTED,
        ];
        torn.extend(proven.iter().map(String::as_str));
        assert_torn(&conn, Launch::RetryOwned, &torn);
        drop(conn);
        settle(&home, &runtime, &leadership);
        let conn = crate::store::open_store(&path).unwrap();
        let unsettled = format!(
            "{UNLOCK} UPDATE automation_attempts SET settled_at = NULL WHERE attempt_number = 2"
        );
        let mut torn = vec![
            "UPDATE sessions SET status = 'failed' WHERE status = 'completed'",
            "UPDATE automation_occurrences SET failure_reason = 'other'",
            "UPDATE automation_runs SET finished_at = NULL",
            &unsettled,
            EXTRA_OCCURRENCE,
            EXHAUSTED,
        ];
        torn.extend(proven.iter().map(String::as_str));
        assert_torn(&conn, Launch::RetrySucceeded, &torn);
        drop(leadership);
    }

    #[test]
    fn restored_launches_are_whole_or_mixed() {
        let base = base().unwrap();
        let (_temp, home) = stopped_store(Scene::NoRetry, Boundary::None);
        let path = crate::api::store_path(&home);
        let conn = crate::store::open_store(&path).unwrap();
        assert_torn(
            &conn,
            Launch::Restored,
            &[
                "UPDATE automation_occurrences SET failure_reason = 'other'",
                "UPDATE automation_occurrences SET lease_owner = 'daemon',
                     lease_expires_at = '2099-10-20T10:00:00.000Z'",
                EXTRA_OCCURRENCE,
                EXHAUSTED,
            ],
        );
        drop(conn);
        let runtime = CountingRuntime::default();
        let leadership = restart(&home, &runtime, at(base, RESTART)).unwrap();
        let conn = crate::store::open_store(&path).unwrap();
        assert_torn(
            &conn,
            Launch::Owned,
            &[
                "UPDATE sessions SET status = 'created'",
                "UPDATE automation_occurrences SET failure_reason = 'other'",
                "UPDATE automation_occurrences SET lease_owner = 'other'",
                "UPDATE automation_attempts SET session_id = NULL",
                EXTRA_OCCURRENCE,
                FENCED,
            ],
        );
        drop(conn);
        settle(&home, &runtime, &leadership);
        let conn = crate::store::open_store(&path).unwrap();
        let unsettled = format!("{UNLOCK} UPDATE automation_attempts SET settled_at = NULL");
        assert_torn(
            &conn,
            Launch::Succeeded,
            &[
                "UPDATE sessions SET status = 'failed'",
                "UPDATE automation_occurrences SET failure_reason = 'other'",
                "UPDATE automation_runs SET finished_at = NULL",
                &unsettled,
                EXTRA_OCCURRENCE,
                EXHAUSTED,
            ],
        );
        drop(leadership);
    }

    #[test]
    fn only_the_exact_no_process_receipt_counts() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path();
        assert!(!receipt_recorded(home).unwrap());
        let receipt = crate::automations::runner::containment_receipt_path(home, "session-one");
        std::fs::create_dir_all(receipt.parent().unwrap()).unwrap();
        std::fs::write(&receipt, crate::pty_runner::CONTAINMENT_QUIESCENT_RECEIPT).unwrap();
        assert!(!receipt_recorded(home).unwrap());
        std::fs::write(&receipt, crate::pty_runner::CONTAINMENT_NO_PROCESS_RECEIPT).unwrap();
        assert!(receipt_recorded(home).unwrap());
        let second = crate::automations::runner::containment_receipt_path(home, "session-two");
        std::fs::write(&second, crate::pty_runner::CONTAINMENT_NO_PROCESS_RECEIPT).unwrap();
        assert!(!receipt_recorded(home).unwrap());
    }

    #[test]
    fn worker_refuses_invalid_or_weakened_input() {
        let mut case = vectors()["cases"][0].clone();
        case["storePath"] = json!("private-input");
        assert_eq!(worker(&case).unwrap_err(), INVALID);
        let mut case = vectors()["cases"][5].clone();
        case["expected"]["restartLaunches"] = json!(1);
        assert_eq!(worker(&case).unwrap_err(), INVALID);
        let mut case = vectors()["cases"][9].clone();
        case["scene"] = json!("no_retry");
        assert_eq!(worker(&case).unwrap_err(), INVALID);
    }
}
