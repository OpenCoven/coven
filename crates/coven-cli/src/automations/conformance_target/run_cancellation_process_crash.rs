//! Disposable owner-cancellation crash fixtures: a daemon worker is killed
//! while it handles the owner's `run.cancel.v1` for a scheduled run. The
//! parent restarts the daemon's scheduler inside the cancellation's lease and
//! sends the same request again, before and after a pass reconciles the
//! cancellation. Both launch through no-op runtimes; no runtime process starts.
use std::collections::BTreeSet;
use std::io::Write;
use std::path::Path;

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
use super::run_launch_process_crash::{pass, restart, snapshot_tables, CountingRuntime};
use super::valid_case_id;
use crate::automations::cancellation::{
    execute_run_cancellation_at_stop_fence, CancellationExecution,
};
use crate::automations::contract::error::ErrorCode;
use crate::automations::definition::RoutineDefinition;
use crate::automations::leadership::SchedulerLeadership;

const INVALID: &str = "conformance vector is invalid";
const FAILED: &str = "conformance suite execution failed";
/// A production recovery step or the re-sent command returned an error; the
/// case fails.
const REFUSED: &str = "cancellation refused";
const SCHEMA_VERSION: &str = "coven.automations.run-cancellation-process-crash-recovery-vectors.v1";
const WORKER: &str = "run-cancellation-crash-worker";
const HOME_PREFIX: &str = "cancel-crash-";
/// The definition predates the schedule slot the worker claims.
const DEFINED_AT: &str = "2099-10-19T09:00:00.000Z";
/// The one adoption key the owner's cancellation carries, sent and re-sent.
const ADOPTION_KEY: &str = "adopt:conformance:cancel-crash";
const REASON: &str = "not needed today";
const PRINCIPAL: &str = "conformance:owner";
/// Where the worker's runtime records each stop it is asked for, inside the
/// worker's private home, so the parent can count stops a killed worker made.
pub(super) const STOP_LOG: &str = "conformance-stops";
/// Seconds after `BASE`: the worker claims and launches the 09:00 slot and
/// handles the owner's cancellation, whose execution lease ends 30 seconds
/// later. The parent restarts the daemon and the owner re-sends the request
/// inside that lease; a pass after it reconciles the cancellation, and the
/// owner sends the request again. The run's 30-minute deadline is never
/// reached.
const CLAIM: i64 = 30;
const CANCEL: i64 = 60;
const RESTART: i64 = 70;
const REISSUE: i64 = 80;
const PASS: i64 = 150;
const RESEND: i64 = 160;
const REPLAY: i64 = 240;

/// Where the worker can stop, in the cancellation's order: after each write
/// of its four transactions and after each commit, right after the runtime is
/// asked to stop the session, and the uninterrupted control.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
enum Boundary {
    AfterReservationInsert,
    AfterCancellationInsert,
    AfterReservationCommit,
    AfterFenceInsert,
    AfterStopping,
    AfterFenceCommit,
    AfterStop,
    AfterSessionEnd,
    AfterAttemptSettle,
    AfterRunFinish,
    AfterOccurrenceSettle,
    AfterFenceRelease,
    AfterSettleCommit,
    AfterCancelled,
    AfterReservationRelease,
    AfterAdoption,
    AfterAnswerCommit,
    None,
}

impl Boundary {
    const ALL: [Self; 18] = [
        Self::AfterReservationInsert,
        Self::AfterCancellationInsert,
        Self::AfterReservationCommit,
        Self::AfterFenceInsert,
        Self::AfterStopping,
        Self::AfterFenceCommit,
        Self::AfterStop,
        Self::AfterSessionEnd,
        Self::AfterAttemptSettle,
        Self::AfterRunFinish,
        Self::AfterOccurrenceSettle,
        Self::AfterFenceRelease,
        Self::AfterSettleCommit,
        Self::AfterCancelled,
        Self::AfterReservationRelease,
        Self::AfterAdoption,
        Self::AfterAnswerCommit,
        Self::None,
    ];

    /// The worker stops inside a write transaction.
    fn inside_transaction(self) -> bool {
        !matches!(
            self,
            Self::AfterReservationCommit
                | Self::AfterFenceCommit
                | Self::AfterStop
                | Self::AfterSettleCommit
                | Self::AfterAnswerCommit
                | Self::None
        )
    }

    /// What the store holds once the worker is gone.
    fn stage(self) -> Stage {
        match self {
            Self::AfterReservationInsert | Self::AfterCancellationInsert => Stage::Untouched,
            Self::AfterReservationCommit | Self::AfterFenceInsert | Self::AfterStopping => {
                Stage::Reserved
            }
            Self::AfterFenceCommit => Stage::Fenced,
            Self::AfterStop
            | Self::AfterSessionEnd
            | Self::AfterAttemptSettle
            | Self::AfterRunFinish
            | Self::AfterOccurrenceSettle
            | Self::AfterFenceRelease => Stage::Stopped,
            Self::AfterSettleCommit
            | Self::AfterCancelled
            | Self::AfterReservationRelease
            | Self::AfterAdoption => Stage::Settled,
            Self::AfterAnswerCommit | Self::None => Stage::Answered,
        }
    }
}

/// How far the cancellation got before the worker died: nothing; the
/// reservation; the stop fence; the stop itself; its settlement; the answer.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Stage {
    Untouched,
    Reserved,
    Fenced,
    Stopped,
    Settled,
    Answered,
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
    cancellation_after_exit: Cancellation,
    worker_stops: u64,
    run_after_restart: Run,
    cancellation_after_restart: Cancellation,
    restart_is_noop: bool,
    reissue: Answer,
    run_after_pass: Run,
    cancellation_after_pass: Cancellation,
    resend: Answer,
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
    /// The same, with the owner's cancellation holding the run's stop fence on
    /// that session.
    Fenced,
    /// Behind the cancellation's fence, the restart's containment has marked
    /// the session `killed`; the attempt is still `started` and the run and
    /// occurrence `running`.
    Contained,
    /// With no fence, the restart's containment has marked the session
    /// `killed` and its startup pass has settled the attempt `failed`. The
    /// run and occurrence stay `running`, held for an operator retry until the
    /// run's deadline.
    Held,
    /// The session, the attempt, the run and the occurrence `cancelled`, the
    /// occurrence with no lease, and no stop fence.
    Cancelled,
    /// The attempt settled `ambiguous` on its `killed` session, and the run
    /// and occurrence `failed`, the occurrence with no lease, and no stop
    /// fence: the settlement released the cancellation's.
    Failed,
    /// Anything else, including a partial transaction.
    Mixed,
}

/// The owner's cancellation as the store records it: its row, the global
/// command reservation and the answer stored under its adoption key.
#[derive(Debug, Deserialize, Serialize, PartialEq, Clone, Copy)]
#[serde(rename_all = "snake_case")]
enum Cancellation {
    /// No cancellation, reservation or answer.
    None,
    /// `requested` under the reservation, with no answer.
    Requested,
    /// `stopping` under the reservation, with no answer.
    Stopping,
    /// No cancellation or reservation; a stored `ILLEGAL_TRANSITION`
    /// rejection.
    Refused,
    /// `rejected`, with no reservation and a stored `ILLEGAL_TRANSITION`
    /// rejection.
    Rejected,
    /// `recovery_required`, with no reservation and a stored committed answer
    /// of that status.
    RecoveryRequired,
    /// `cancelled`, with no reservation and a stored committed answer of that
    /// status.
    Cancelled,
    /// Anything else, including a partial transaction.
    Mixed,
}

/// What one re-sent request returned.
#[derive(Debug, Deserialize, Serialize, PartialEq, Clone, Copy)]
#[serde(rename_all = "snake_case")]
enum Answer {
    /// `CANCEL_PENDING`, with no write.
    Pending,
    /// `ILLEGAL_TRANSITION`, which this request stored as its answer.
    Refused,
    /// `ILLEGAL_TRANSITION` replayed from the store, with no write.
    Rejected,
    /// The stored `recovery_required` answer, with no write.
    RecoveryRequired,
    /// The stored `cancelled` answer, acknowledged and reconciled when the
    /// stop was settled, with no write.
    Cancelled,
    /// Anything else: another code, a write with a replay, or a response
    /// that does not name the store's run.
    Mismatched,
}

/// What each boundary must produce. A vector restates it, so it cannot weaken
/// it.
fn expected_for(boundary: Boundary) -> Expected {
    let stage = boundary.stage();
    let (run_after_exit, cancellation_after_exit) = match stage {
        Stage::Untouched => (Run::Running, Cancellation::None),
        Stage::Reserved => (Run::Running, Cancellation::Requested),
        Stage::Fenced | Stage::Stopped => (Run::Fenced, Cancellation::Stopping),
        Stage::Settled => (Run::Cancelled, Cancellation::Stopping),
        Stage::Answered => (Run::Cancelled, Cancellation::Cancelled),
    };
    // The restart's containment marks a session it finds running `killed`.
    // Behind the cancellation's fence, the startup pass leaves the attempt to
    // the cancellation's reconciliation; without one, it fails the attempt and
    // holds the run.
    let run_after_restart = match stage {
        Stage::Untouched | Stage::Reserved => Run::Held,
        Stage::Fenced | Stage::Stopped => Run::Contained,
        Stage::Settled | Stage::Answered => Run::Cancelled,
    };
    // Inside the lease, a reserved cancellation is the daemon's to reconcile.
    let reissue = match stage {
        Stage::Untouched => Answer::Refused,
        Stage::Reserved | Stage::Fenced | Stage::Stopped | Stage::Settled => Answer::Pending,
        Stage::Answered => Answer::Cancelled,
    };
    // The pass after the lease reconciles it. An unconfirmed stop needs
    // recovery, and the same pass then settles the ambiguous run on its ended
    // session and releases the cancellation's fence.
    let (run_after_pass, cancellation_after_pass, resend) = match stage {
        Stage::Untouched => (Run::Held, Cancellation::Refused, Answer::Rejected),
        Stage::Reserved => (Run::Held, Cancellation::Rejected, Answer::Rejected),
        Stage::Fenced | Stage::Stopped => (
            Run::Failed,
            Cancellation::RecoveryRequired,
            Answer::RecoveryRequired,
        ),
        Stage::Settled | Stage::Answered => {
            (Run::Cancelled, Cancellation::Cancelled, Answer::Cancelled)
        }
    };
    Expected {
        initial_outcome: if boundary == Boundary::None {
            InitialOutcome::Responded
        } else {
            InitialOutcome::Terminated
        },
        run_after_exit,
        cancellation_after_exit,
        worker_stops: u64::from(matches!(
            stage,
            Stage::Stopped | Stage::Settled | Stage::Answered
        )),
        run_after_restart,
        cancellation_after_restart: cancellation_after_exit,
        // A settled run gives the restart nothing to contain or settle.
        restart_is_noop: matches!(stage, Stage::Settled | Stage::Answered),
        reissue,
        run_after_pass,
        cancellation_after_pass,
        resend,
        later_stops: 0,
        launches: 0,
        integrity_ok: true,
        replay_is_noop: true,
    }
}

/// A routine with no automatic retry. The runner launches only a routine with
/// a working directory; a no-op runtime never touches it.
fn synthetic_definition() -> Value {
    json!({"schemaVersion":1,"id":"conformance-cancel-crash",
        "name":"Conformance cancel crash","status":"ACTIVE",
        "rrule":"FREQ=DAILY;BYHOUR=9","timezone":"utc","misfire":"latest",
        "overlap":"forbid","timeoutMinutes":30,"runtime":"coven-code",
        "prompt":"Run the native cancellation crash conformance probe.",
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

/// Where the worker parks inside SQLite. The cancellation reserves the global
/// adoption key and then inserts itself `requested`; claims the stop fence and
/// then moves to `stopping`; once the stop is confirmed, ends the session and
/// then settles the attempt, the run and the occurrence `cancelled` and
/// releases the fence; and finally marks itself `cancelled`, releases the
/// reservation and stores its answer. Each is one transaction, and nothing
/// else commits after the pause is installed. A pause right after the stop is
/// the runtime's, not SQLite's.
fn pause_at(boundary: Boundary) -> Option<Pause<'static>> {
    match boundary {
        Boundary::AfterReservationInsert => Some(Pause::After(
            "AFTER INSERT ON main.automation_command_reservations",
        )),
        Boundary::AfterCancellationInsert => Some(Pause::After(
            "AFTER INSERT ON main.automation_cancellations",
        )),
        Boundary::AfterReservationCommit => Some(Pause::AfterCommits(1)),
        Boundary::AfterFenceInsert => {
            Some(Pause::After("AFTER INSERT ON main.automation_stop_fences"))
        }
        Boundary::AfterStopping => Some(Pause::After(
            "AFTER UPDATE OF state ON main.automation_cancellations
             WHEN NEW.state = 'stopping'",
        )),
        Boundary::AfterFenceCommit => Some(Pause::AfterCommits(2)),
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
        Boundary::AfterSettleCommit => Some(Pause::AfterCommits(3)),
        Boundary::AfterCancelled => Some(Pause::After(
            "AFTER UPDATE OF state ON main.automation_cancellations
             WHEN NEW.state = 'cancelled'",
        )),
        Boundary::AfterReservationRelease => Some(Pause::After(
            "AFTER DELETE ON main.automation_command_reservations",
        )),
        Boundary::AfterAdoption => Some(Pause::After(
            "AFTER INSERT ON main.automation_command_adoptions",
        )),
        Boundary::AfterAnswerCommit => Some(Pause::AfterCommits(4)),
        Boundary::AfterStop | Boundary::None => None,
    }
}

/// The run the owner cancels: the store's one run, its attempt and session.
struct Target {
    run_id: String,
    attempt_id: String,
    session_id: String,
}

fn target(conn: &Connection) -> Result<Target, &'static str> {
    conn.query_row(
        "SELECT r.id, a.id, a.session_id
         FROM automation_runs AS r
         JOIN automation_attempts AS a ON a.run_id = r.id AND a.attempt_number = 1",
        [],
        |row| {
            Ok(Target {
                run_id: row.get(0)?,
                attempt_id: row.get(1)?,
                session_id: row.get(2)?,
            })
        },
    )
    .map_err(|_| FAILED)
}

/// The owner's request, identical each time it is sent.
fn request(target: &Target) -> Value {
    json!({"action": "coven.automations.run.cancel.v1", "adoptionKey": ADOPTION_KEY,
        "runId": target.run_id, "attemptId": target.attempt_id,
        "runtimeCorrelation": {"sessionId": target.session_id}, "scope": "run",
        "reason": REASON, "requestedBy": {"principalId": PRINCIPAL}})
}

/// The answer naming the store's run with `status`. A cancellation is
/// requested, and a confirmed stop settled, at the worker's request time.
fn payload(target: &Target, status: &str, base: DateTime<Utc>) -> Value {
    let requested_at = iso(at(base, CANCEL));
    let mut cancellation = json!({"scope": "run", "reason": REASON,
        "requestedBy": {"principalId": PRINCIPAL}, "requestedAt": requested_at});
    if status == "cancelled" {
        cancellation["acknowledgedAt"] = json!(requested_at);
        cancellation["reconciledAt"] = json!(requested_at);
    }
    json!({"runId": target.run_id, "attemptId": target.attempt_id,
        "runtimeCorrelation": {"sessionId": target.session_id},
        "status": status, "cancellation": cancellation})
}

/// The worker's runtime: launches nothing, confirms every stop, and records
/// each stop in the home's stop log before it returns. With a handshake, it
/// announces it and parks once the stop is recorded.
pub(super) struct RecordingStops {
    pub(super) log: std::path::PathBuf,
    pub(super) park: Option<Value>,
}

impl crate::api::SessionRuntime for RecordingStops {
    fn launch_session(&self, _launch: &crate::api::SessionLaunch) -> anyhow::Result<()> {
        Ok(())
    }

    fn send_input(&self, _session_id: &str, _payload: &Value) -> anyhow::Result<()> {
        Ok(())
    }

    fn kill_session(&self, _session_id: &str) -> anyhow::Result<()> {
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.log)?
            .write_all(b"stop\n")?;
        if let Some(handshake) = &self.park {
            park(handshake);
        }
        Ok(())
    }
}

/// Stops the worker asked its runtime for, killed or not.
pub(super) fn worker_stops(home: &Path) -> Result<u64, &'static str> {
    match std::fs::read(home.join(STOP_LOG)) {
        Ok(log) => {
            u64::try_from(log.iter().filter(|byte| **byte == b'\n').count()).map_err(|_| FAILED)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(0),
        Err(_) => Err(FAILED),
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
    if report.dispatched.len() != 1
        || !report.failed.is_empty()
        || run(conn)? != Run::Running
        || cancellation(conn)? != Cancellation::None
    {
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
    let target = target(&conn)?;
    let handshake =
        json!({"state":"paused", "boundary":case.boundary, "relativeHome":relative_home});
    if let Some(pause_at) = pause_at(case.boundary) {
        install_pause(&conn, pause_at, handshake.clone())?;
    }
    let runtime = RecordingStops {
        log: home.join(STOP_LOG),
        park: (case.boundary == Boundary::AfterStop).then_some(handshake),
    };
    // The daemon handles the owner's request as its API does. The API fences
    // the stop on the wall clock; this fences it on the fixture's fixed clock,
    // so its times agree with the scheduler's.
    let cancel_at = at(base, CANCEL);
    let response = execute_run_cancellation_at_stop_fence(
        &conn,
        &runtime,
        request(&target),
        cancel_at,
        cancel_at,
    )
    .map_err(|_| FAILED)?;
    let answered = matches!(&response, CancellationExecution::Success(success)
        if !success.replayed && success.payload == payload(&target, "cancelled", base));
    if case.boundary != Boundary::None || !answered {
        return Err(FAILED);
    }
    // Exits like a daemon that dies after answering: without the graceful
    // shutdown's reconciliation or the release of its scheduler authority.
    drop(conn);
    drop(leadership);
    announce(&json!({"state":"responded", "boundary":case.boundary, "relativeHome":relative_home}))
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
    type Row = (String, String, String, bool, String, bool, String, String);
    let row: Option<Row> = conn
        .query_row(
            "SELECT o.state,
                    CASE
                        WHEN o.lease_owner = 'daemon' AND o.lease_expires_at IS NOT NULL
                            THEN 'daemon'
                        WHEN o.lease_owner IS NULL AND o.lease_expires_at IS NULL THEN 'none'
                        ELSE 'other'
                    END,
                    r.status, r.session_id IS a.session_id,
                    a.state, a.settled_at IS NOT NULL, s.status,
                    CASE
                        WHEN NOT EXISTS (SELECT 1 FROM automation_stop_fences) THEN 'none'
                        WHEN (SELECT COUNT(*) FROM automation_stop_fences) = 1
                            AND EXISTS (
                                SELECT 1 FROM automation_stop_fences
                                WHERE run_id = r.id AND session_id = a.session_id
                                  AND owner = 'cancellation' AND operation_key = ?1
                            ) THEN 'cancellation'
                        ELSE 'other'
                    END
             FROM automation_occurrences AS o
             JOIN automation_runs AS r ON r.occurrence_id = o.id
             JOIN automation_attempts AS a ON a.run_id = r.id AND a.attempt_number = 1
             JOIN sessions AS s ON s.id = a.session_id",
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
                ))
            },
        )
        .optional()
        .map_err(|_| FAILED)?;
    let Some((occurrence, lease, run, on_session, attempt, settled, session, fence)) = row else {
        return Ok(Run::Mixed);
    };
    if !on_session {
        return Ok(Run::Mixed);
    }
    Ok(
        match (
            occurrence.as_str(),
            lease.as_str(),
            run.as_str(),
            attempt.as_str(),
            settled,
            session.as_str(),
            fence.as_str(),
        ) {
            ("running", "daemon", "running", "started", false, "running", "none") => Run::Running,
            ("running", "daemon", "running", "started", false, "running", "cancellation") => {
                Run::Fenced
            }
            ("running", "daemon", "running", "started", false, "killed", "cancellation") => {
                Run::Contained
            }
            ("running", "daemon", "running", "failed", true, "killed", "none") => Run::Held,
            ("cancelled", "none", "cancelled", "cancelled", true, "cancelled", "none") => {
                Run::Cancelled
            }
            ("failed", "none", "failed", "ambiguous", true, "killed", "none") => Run::Failed,
            _ => Run::Mixed,
        },
    )
}

/// Classifies the owner's cancellation, its reservation and its stored answer.
fn cancellation(conn: &Connection) -> Result<Cancellation, &'static str> {
    type Row = (
        i64,
        i64,
        i64,
        Option<String>,
        bool,
        Option<(String, String)>,
    );
    let (cancellations, reservations, adoptions, state, reserved, adoption): Row = conn
        .query_row(
            "SELECT
                (SELECT COUNT(*) FROM automation_cancellations),
                (SELECT COUNT(*) FROM automation_command_reservations),
                (SELECT COUNT(*) FROM automation_command_adoptions),
                (SELECT state FROM automation_cancellations WHERE adoption_key = ?1),
                EXISTS (
                    SELECT 1 FROM automation_command_reservations
                    WHERE adoption_key = ?1 AND command = 'run.cancel.v1'
                ),
                (SELECT outcome FROM automation_command_adoptions
                 WHERE adoption_key = ?1 AND command = 'run.cancel.v1'),
                (SELECT response_json FROM automation_command_adoptions
                 WHERE adoption_key = ?1 AND command = 'run.cancel.v1')",
            [ADOPTION_KEY],
            |row| {
                let outcome: Option<String> = row.get(5)?;
                let response: Option<String> = row.get(6)?;
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    outcome.zip(response),
                ))
            },
        )
        .map_err(|_| FAILED)?;
    // The stored answer's outcome with its error code or result status.
    let answer = adoption
        .map(|(outcome, response)| {
            let response: Value = serde_json::from_str(&response).map_err(|_| FAILED)?;
            let detail = match outcome.as_str() {
                "rejected" => response["error"]["code"].as_str(),
                "committed" => response["result"]["status"].as_str(),
                _ => None,
            };
            Ok::<_, &'static str>((outcome, detail.unwrap_or_default().to_owned()))
        })
        .transpose()?;
    let answer = answer
        .as_ref()
        .map(|(outcome, detail)| (outcome.as_str(), detail.as_str()));
    Ok(
        match (
            cancellations,
            reservations,
            adoptions,
            state.as_deref(),
            reserved,
            answer,
        ) {
            (0, 0, 0, None, false, None) => Cancellation::None,
            (1, 1, 0, Some("requested"), true, None) => Cancellation::Requested,
            (1, 1, 0, Some("stopping"), true, None) => Cancellation::Stopping,
            (0, 0, 1, None, false, Some(("rejected", "ILLEGAL_TRANSITION"))) => {
                Cancellation::Refused
            }
            (1, 0, 1, Some("rejected"), false, Some(("rejected", "ILLEGAL_TRANSITION"))) => {
                Cancellation::Rejected
            }
            (
                1,
                0,
                1,
                Some("recovery_required"),
                false,
                Some(("committed", "recovery_required")),
            ) => Cancellation::RecoveryRequired,
            (1, 0, 1, Some("cancelled"), false, Some(("committed", "cancelled"))) => {
                Cancellation::Cancelled
            }
            _ => Cancellation::Mixed,
        },
    )
}

/// Every row the cancellation, its recovery or a restart can write, for an
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
            "automation_command_reservations",
            "automation_command_adoptions",
            "automation_cancellations",
            "automation_stop_fences",
        ],
    )
}

/// Sends the owner's request again at `now` through `runtime` and classifies
/// the answer against the store.
fn reissue(
    conn: &Connection,
    runtime: &CountingRuntime,
    target: &Target,
    now: DateTime<Utc>,
) -> Result<Answer, &'static str> {
    let before = snapshot(conn)?;
    let execution =
        execute_run_cancellation_at_stop_fence(conn, runtime, request(target), now, now)
            .map_err(|_| REFUSED)?;
    let wrote = snapshot(conn)? != before;
    let base = base()?;
    Ok(match execution {
        CancellationExecution::Rejected(error) => match (error.code(), wrote) {
            (ErrorCode::CancelPending, false) => Answer::Pending,
            (ErrorCode::IllegalTransition, true)
                if cancellation(conn)? == Cancellation::Refused =>
            {
                Answer::Refused
            }
            (ErrorCode::IllegalTransition, false) => Answer::Rejected,
            _ => Answer::Mismatched,
        },
        CancellationExecution::Success(success) if !wrote => {
            if success.payload == payload(target, "recovery_required", base) {
                Answer::RecoveryRequired
            } else if success.payload == payload(target, "cancelled", base) {
                Answer::Cancelled
            } else {
                Answer::Mismatched
            }
        }
        CancellationExecution::Success(_) => Answer::Mismatched,
    })
}

/// What the restarted daemon, its pass and the re-sent requests did with the
/// store the worker left.
struct Restarted {
    run_after_restart: Run,
    cancellation_after_restart: Cancellation,
    restart_is_noop: bool,
    reissue: Answer,
    run_after_pass: Run,
    cancellation_after_pass: Cancellation,
    resend: Answer,
    later_stops: u64,
    launches: u64,
    replay_is_noop: bool,
}

fn recover(home: &Path, path: &Path, base: DateTime<Utc>) -> Result<Restarted, &'static str> {
    // One runtime confirms every stop and counts it, with every launch, for
    // the restart, each pass and each re-sent request.
    let runtime = CountingRuntime::default();
    let before_restart = snapshot(&crate::store::open_store(path).map_err(|_| FAILED)?)?;
    let leadership = restart(home, &runtime, at(base, RESTART))?;
    let conn = crate::store::open_store(path).map_err(|_| FAILED)?;
    let run_after_restart = run(&conn)?;
    let cancellation_after_restart = cancellation(&conn)?;
    let restart_is_noop = snapshot(&conn)? == before_restart;
    let target = target(&conn)?;
    // The owner never saw an answer, so they send the same request again.
    let reissued = reissue(&conn, &runtime, &target, at(base, REISSUE))?;
    drop(conn);
    pass(home, &runtime, &leadership, at(base, PASS), "deadline")?;
    let conn = crate::store::open_store(path).map_err(|_| FAILED)?;
    let run_after_pass = run(&conn)?;
    let cancellation_after_pass = cancellation(&conn)?;
    let resent = reissue(&conn, &runtime, &target, at(base, RESEND))?;
    let later_stops = runtime.stops();
    let launches = runtime.launches();
    let before_replay = snapshot(&conn)?;
    // The same request once more, then a pass on its own connection, must
    // answer the same without a write, a stop or a launch.
    let replayed = reissue(&conn, &runtime, &target, at(base, REPLAY))? == resent;
    drop(conn);
    pass(home, &runtime, &leadership, at(base, REPLAY), "deadline")?;
    let conn = crate::store::open_store(path).map_err(|_| FAILED)?;
    let replay_is_noop = replayed
        && snapshot(&conn)? == before_replay
        && runtime.stops() == later_stops
        && runtime.launches() == launches;
    drop(conn);
    drop(leadership);
    Ok(Restarted {
        run_after_restart,
        cancellation_after_restart,
        restart_is_noop,
        reissue: reissued,
        run_after_pass,
        cancellation_after_pass,
        resend: resent,
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
fn observe_pause(path: &Path) -> Result<(bool, Run, Cancellation), &'static str> {
    let probe = open_raw(path)?;
    let open = write_transaction_open(&probe)?;
    Ok((open, run(&probe)?, cancellation(&probe)?))
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
    let pause_consistent = pause.is_none_or(|(open, run, cancellation)| {
        open == case.boundary.inside_transaction()
            && run == case.expected.run_after_exit
            && cancellation == case.expected.cancellation_after_exit
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
    let conn = crate::store::open_store(&path).map_err(|_| FAILED)?;
    let run_after_exit = run(&conn)?;
    let cancellation_after_exit = cancellation(&conn)?;
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
        run_after_exit,
        cancellation_after_exit,
        worker_stops,
        run_after_restart: recovery
            .as_ref()
            .map_or(Run::Mixed, |recovery| recovery.run_after_restart),
        cancellation_after_restart: recovery.as_ref().map_or(Cancellation::Mixed, |recovery| {
            recovery.cancellation_after_restart
        }),
        restart_is_noop: recovery
            .as_ref()
            .is_some_and(|recovery| recovery.restart_is_noop),
        reissue: recovery
            .as_ref()
            .map_or(Answer::Mismatched, |recovery| recovery.reissue),
        run_after_pass: recovery
            .as_ref()
            .map_or(Run::Mixed, |recovery| recovery.run_after_pass),
        cancellation_after_pass: recovery.as_ref().map_or(Cancellation::Mixed, |recovery| {
            recovery.cancellation_after_pass
        }),
        resend: recovery
            .as_ref()
            .map_or(Answer::Mismatched, |recovery| recovery.resend),
        later_stops: recovery.as_ref().map_or(0, |recovery| recovery.later_stops),
        launches: recovery.as_ref().map_or(0, |recovery| recovery.launches),
        integrity_ok,
        replay_is_noop: recovery
            .as_ref()
            .is_some_and(|recovery| recovery.replay_is_noop),
    };
    let passed = observed == case.expected && pause_consistent && !recovery_refused;
    let (transaction_open_at_pause, run_at_pause, cancellation_at_pause) = match pause {
        Some((open, run, cancellation)) => (Some(open), Some(run), Some(cancellation)),
        None => (None, None, None),
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
            "cancellationAtPause": cancellation_at_pause,
            "pauseConsistent": pause_consistent,
            "integrityOk": integrity_ok,
            "runAfterExit": observed.run_after_exit,
            "cancellationAfterExit": observed.cancellation_after_exit,
            "workerStops": observed.worker_stops,
            "recoveryRefused": recovery_refused,
            "runAfterRestart": observed.run_after_restart,
            "cancellationAfterRestart": observed.cancellation_after_restart,
            "restartIsNoop": observed.restart_is_noop,
            "reissue": observed.reissue,
            "runAfterPass": observed.run_after_pass,
            "cancellationAfterPass": observed.cancellation_after_pass,
            "resend": observed.resend,
            "laterStops": observed.later_stops,
            "launches": observed.launches,
            "replayIsNoop": observed.replay_is_noop,
        }),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    fn vectors() -> Value {
        serde_json::from_str(include_str!(
            "../../../../../conformance/automations/runner/run-cancellation-process-crash-recovery.vectors.json"
        ))
        .unwrap()
    }

    #[test]
    fn cancellation_crash_vectors_restate_each_boundary_outcome() {
        let vectors: VectorSet = serde_json::from_value(vectors()).unwrap();
        assert_eq!(vectors.schema_version, SCHEMA_VERSION);
        for case in &vectors.cases {
            validate(case).unwrap();
        }
        let boundaries: Vec<_> = vectors.cases.iter().map(|case| case.boundary).collect();
        assert_eq!(boundaries, Boundary::ALL);
    }

    #[test]
    fn cancellation_crash_vectors_require_each_boundary_and_the_control() {
        let original = vectors();
        let mut missing = original.clone();
        missing["cases"].as_array_mut().unwrap().pop();
        assert_eq!(evaluate(&missing).unwrap_err(), INVALID);
        // A second control under its own case id, replacing a boundary.
        let mut duplicate = original.clone();
        let mut control = duplicate["cases"][17].clone();
        control["caseId"] = json!("second-uninterrupted-cancellation-control");
        duplicate["cases"][0] = control;
        assert_eq!(evaluate(&duplicate).unwrap_err(), INVALID);
        let mut duplicate_id = original;
        duplicate_id["cases"][1]["caseId"] = duplicate_id["cases"][0]["caseId"].clone();
        assert_eq!(evaluate(&duplicate_id).unwrap_err(), INVALID);
    }

    #[test]
    fn cancellation_crash_vectors_refuse_malformed_or_weakened_inputs() {
        let original = vectors();
        for (case, field, value) in [
            (0, "initialOutcome", json!("responded")),
            (0, "reissue", json!("pending")),
            (1, "cancellationAfterExit", json!("requested")),
            (2, "runAfterRestart", json!("running")),
            (2, "reissue", json!("rejected")),
            (3, "cancellationAfterPass", json!("cancelled")),
            (4, "runAfterExit", json!("fenced")),
            (5, "workerStops", json!(1)),
            (5, "runAfterRestart", json!("held")),
            (6, "workerStops", json!(0)),
            (6, "laterStops", json!(1)),
            (7, "runAfterPass", json!("cancelled")),
            (8, "resend", json!("pending")),
            (9, "cancellationAfterPass", json!("rejected")),
            (11, "runAfterPass", json!("contained")),
            (12, "runAfterExit", json!("fenced")),
            (12, "reissue", json!("cancelled")),
            (13, "cancellationAfterRestart", json!("cancelled")),
            (0, "restartIsNoop", json!(true)),
            (14, "restartIsNoop", json!(false)),
            (15, "resend", json!("recovery_required")),
            (16, "reissue", json!("pending")),
            (16, "launches", json!(1)),
            (17, "integrityOk", json!(false)),
            (17, "replayIsNoop", json!(false)),
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
            ("boundary", json!("after_launch")),
            ("caseId", json!("")),
        ] {
            let mut invalid = original.clone();
            invalid["cases"][0][field] = value;
            assert_eq!(evaluate(&invalid).unwrap_err(), INVALID, "{field}");
        }
        let mut invalid = original;
        invalid["schemaVersion"] =
            json!("coven.automations.occurrence-recovery-process-crash-recovery-vectors.v1");
        assert_eq!(evaluate(&invalid).unwrap_err(), INVALID);
    }

    /// A store whose routine has launched its 09:00 slot, as the worker
    /// leaves it before the owner's request.
    fn launched_store() -> (
        tempfile::TempDir,
        std::path::PathBuf,
        Connection,
        SchedulerLeadership,
        Target,
    ) {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        crate::daemon::ensure_private_coven_home(&home).unwrap();
        let leadership = launched(&home, base().unwrap()).unwrap();
        let conn = crate::store::open_store(&crate::api::store_path(&home)).unwrap();
        let target = target(&conn).unwrap();
        (temp, home, conn, leadership, target)
    }

    /// Reads the store on its own connection while the cancellation asks it
    /// to stop the session.
    struct Observing {
        path: std::path::PathBuf,
        seen: Mutex<Vec<(bool, Run, Cancellation)>>,
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
            let state = (open, run(&probe).unwrap(), cancellation(&probe).unwrap());
            self.seen.lock().unwrap().push(state);
            Ok(())
        }
    }

    #[test]
    fn the_stop_is_issued_once_between_the_fence_and_the_settlement() {
        let (_temp, home, conn, _leadership, target) = launched_store();
        let base = base().unwrap();
        let runtime = Observing {
            path: crate::api::store_path(&home),
            seen: Mutex::new(Vec::new()),
        };
        let cancel_at = at(base, CANCEL);
        let answer = execute_run_cancellation_at_stop_fence(
            &conn,
            &runtime,
            request(&target),
            cancel_at,
            cancel_at,
        )
        .unwrap();
        assert!(
            matches!(&answer, CancellationExecution::Success(success)
                if !success.replayed && success.payload == payload(&target, "cancelled", base)),
            "{answer:?}"
        );
        // No write is open while the runtime stops the session, and the
        // fence and `stopping` are committed.
        assert_eq!(
            *runtime.seen.lock().unwrap(),
            [(false, Run::Fenced, Cancellation::Stopping)]
        );
        assert_eq!(run(&conn).unwrap(), Run::Cancelled);
        assert_eq!(cancellation(&conn).unwrap(), Cancellation::Cancelled);
        // The same request replays, through a runtime that is never asked.
        let replaying = CountingRuntime::default();
        assert_eq!(
            reissue(&conn, &replaying, &target, at(base, REISSUE)).unwrap(),
            Answer::Cancelled
        );
        assert_eq!(replaying.stops(), 0);
    }

    #[test]
    fn each_transaction_is_whole_or_a_state_the_suite_never_accepts() {
        let accepted: Vec<(Run, Cancellation)> = Boundary::ALL
            .iter()
            .flat_map(|boundary| {
                let expected = expected_for(*boundary);
                [
                    (expected.run_after_exit, expected.cancellation_after_exit),
                    (
                        expected.run_after_restart,
                        expected.cancellation_after_restart,
                    ),
                    (expected.run_after_pass, expected.cancellation_after_pass),
                ]
            })
            .collect();
        let (_temp, _home, conn, _leadership, _target) = launched_store();
        let state = |conn: &Connection| (run(conn).unwrap(), cancellation(conn).unwrap());
        let step = |sql: &str, whole: Option<(Run, Cancellation)>| {
            conn.execute(sql, [&ADOPTION_KEY]).unwrap();
            let now = state(&conn);
            match whole {
                Some(whole) => assert_eq!(now, whole, "{sql}"),
                None => assert!(!accepted.contains(&now), "{sql}: {now:?}"),
            }
        };
        assert_eq!(state(&conn), (Run::Running, Cancellation::None));
        let requested_at = iso(at(base().unwrap(), CANCEL));
        // The four transactions' writes, in the cancellation's order.
        conn.execute_batch("BEGIN").unwrap();
        step(
            &format!(
                "INSERT INTO automation_command_reservations
                    (adoption_key, request_digest, command, reserved_at)
                 VALUES (?1, 'digest', 'run.cancel.v1', '{requested_at}')"
            ),
            None,
        );
        // The store holds one run, attempt and session; the writes read them.
        step(
            &format!(
                "INSERT INTO automation_cancellations (
                    adoption_key, request_digest, automation_id, run_id, attempt_id,
                    session_id, scope, requested_by_json, reason, state, requested_at,
                    execution_expires_at
                 ) SELECT ?1, 'digest', 'conformance-cancel-crash', r.id, a.id,
                          a.session_id, 'run', '{{}}', 'reason', 'requested',
                          '{requested_at}', '{requested_at}'
                   FROM automation_runs AS r JOIN automation_attempts AS a ON a.run_id = r.id"
            ),
            Some((Run::Running, Cancellation::Requested)),
        );
        step(
            &format!(
                "INSERT INTO automation_stop_fences
                    (run_id, session_id, owner, operation_key, acquired_at, execution_expires_at)
                 SELECT id, session_id, 'cancellation', ?1, '{requested_at}', '{requested_at}'
                 FROM automation_runs"
            ),
            None,
        );
        step(
            "UPDATE automation_cancellations SET state = 'stopping' WHERE adoption_key = ?1",
            Some((Run::Fenced, Cancellation::Stopping)),
        );
        for sql in [
            "UPDATE sessions SET status = 'cancelled' WHERE ?1 IS NOT NULL",
            &format!(
                "UPDATE automation_attempts SET state = 'cancelled', settled_at = '{requested_at}'
                 WHERE ?1 IS NOT NULL"
            ),
            "UPDATE automation_runs SET status = 'cancelled' WHERE ?1 IS NOT NULL",
            "UPDATE automation_occurrences
             SET state = 'cancelled', lease_owner = NULL, lease_expires_at = NULL
             WHERE ?1 IS NOT NULL",
        ] {
            step(sql, None);
        }
        step(
            "DELETE FROM automation_stop_fences WHERE operation_key = ?1",
            Some((Run::Cancelled, Cancellation::Stopping)),
        );
        step(
            "UPDATE automation_cancellations SET state = 'cancelled' WHERE adoption_key = ?1",
            None,
        );
        step(
            "DELETE FROM automation_command_reservations WHERE adoption_key = ?1",
            None,
        );
        step(
            &format!(
                "INSERT INTO automation_command_adoptions (
                    adoption_key, command, automation_id, request_digest, outcome,
                    revision, response_json, adopted_at
                 ) VALUES (?1, 'run.cancel.v1', 'conformance-cancel-crash', 'digest',
                           'committed', NULL, '{{\"result\":{{\"status\":\"cancelled\"}}}}',
                           '{requested_at}')"
            ),
            Some((Run::Cancelled, Cancellation::Cancelled)),
        );
        conn.execute_batch("ROLLBACK").unwrap();
        // An answer that disagrees with its cancellation is not one.
        conn.execute_batch("BEGIN").unwrap();
        conn.execute(
            &format!(
                "INSERT INTO automation_command_adoptions (
                    adoption_key, command, automation_id, request_digest, outcome,
                    revision, response_json, adopted_at
                 ) VALUES (?1, 'run.cancel.v1', 'conformance-cancel-crash', 'digest',
                           'committed', NULL, '{{\"result\":{{\"status\":\"cancelled\"}}}}',
                           '{requested_at}')"
            ),
            [ADOPTION_KEY],
        )
        .unwrap();
        assert_eq!(cancellation(&conn).unwrap(), Cancellation::Mixed);
        conn.execute_batch("ROLLBACK").unwrap();
        assert_eq!(state(&conn), (Run::Running, Cancellation::None));
    }

    #[test]
    fn worker_refuses_invalid_or_weakened_input() {
        let mut case = vectors()["cases"][0].clone();
        case["storePath"] = json!("private-input");
        assert_eq!(worker(&case).unwrap_err(), INVALID);
        let mut case = vectors()["cases"][6].clone();
        case["expected"]["workerStops"] = json!(0);
        assert_eq!(worker(&case).unwrap_err(), INVALID);
        let mut case = vectors()["cases"][17].clone();
        case["boundary"] = json!("after_answer_commit");
        assert_eq!(worker(&case).unwrap_err(), INVALID);
    }
}
