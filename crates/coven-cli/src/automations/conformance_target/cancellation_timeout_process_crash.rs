//! Disposable cancellation-timeout race crash fixtures: a daemon worker is
//! killed while the owner's `run.cancel.v1` and a scheduled run's deadline
//! race for the run's stop. In one scene the worker handles the cancellation
//! 20 seconds before the deadline, and the deadline passes while the daemon is
//! down. In the other it enforces the deadline, and the owner's cancellation
//! reaches the restarted daemon. The parent restarts the daemon's scheduler
//! inside the dead stop owner's lease, sends the owner's request, and runs
//! passes after the lease. Both launch through no-op runtimes; no runtime
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
use super::run_cancellation_process_crash::{worker_stops, RecordingStops, STOP_LOG};
use super::run_launch_process_crash::{pass, restart, snapshot_tables, CountingRuntime, REFUSED};
use super::timeout_enforcement_process_crash::launch_slot;
use super::valid_case_id;
use crate::automations::cancellation::{
    execute_run_cancellation_at_stop_fence, CancellationExecution,
};
use crate::automations::contract::error::ErrorCode;
use crate::automations::runner::enforce_run_timeouts;

const INVALID: &str = "conformance vector is invalid";
const FAILED: &str = "conformance suite execution failed";
const SCHEMA_VERSION: &str =
    "coven.automations.cancellation-timeout-process-crash-recovery-vectors.v1";
const WORKER: &str = "cancellation-timeout-crash-worker";
const HOME_PREFIX: &str = "race-crash-";
/// The one adoption key the owner's cancellation carries, sent and re-sent.
const ADOPTION_KEY: &str = "adopt:conformance:race-crash";
const REASON: &str = "not needed today";
const PRINCIPAL: &str = "conformance:owner";
/// The stored refusal of a request that finds the session already ended.
const ENDED: &str = "ILLEGAL_TRANSITION: the targeted automation attempt is no longer cancellable";
/// The stored rejection of a reserved cancellation the deadline overtook.
const OVERTAKEN: &str =
    "ILLEGAL_TRANSITION: the targeted automation run reached its timeout before cancellation";
/// The tables a request may change, besides the stored answers.
const STATE_TABLES: [&str; 8] = [
    "automation_occurrences",
    "automation_runs",
    "automation_attempts",
    "automation_events",
    "sessions",
    "automation_command_reservations",
    "automation_cancellations",
    "automation_stop_fences",
];
/// Seconds after `BASE`. The worker claims and launches the 09:00 slot, whose
/// one-minute deadline passes at 90 seconds. In the cancellation scene it
/// handles the owner's request at 70, and the cancellation's execution lease
/// ends at 100. In the timeout scene it enforces the deadline at 92, and the
/// timeout's stop lease ends at 122. The parent restarts the daemon inside
/// either lease and after the deadline, and the owner sends the request; a
/// pass after both leases, the request again, a second pass, and the request
/// and a pass once more, which must change nothing.
const CLAIM: i64 = 30;
const CANCEL: i64 = 70;
const ENFORCE: i64 = 92;
const RESTART: i64 = 95;
const REQUEST: i64 = 97;
const PASS: i64 = 150;
const RESEND: i64 = 160;
const SETTLE: i64 = 230;
const REPLAY: i64 = 300;

/// Which side of the race the worker is running when it dies.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
enum Scene {
    /// The owner's `run.cancel.v1`, 20 seconds before the deadline.
    Cancellation,
    /// The scheduler's enforcement of the deadline.
    Timeout,
}

impl Scene {
    const ALL: [Self; 2] = [Self::Cancellation, Self::Timeout];

    /// Where the worker can stop in this scene, in its order.
    fn boundaries(self) -> &'static [Boundary] {
        match self {
            Self::Cancellation => &[
                Boundary::InsideReservation,
                Boundary::AfterReservationCommit,
                Boundary::InsideFence,
                Boundary::AfterFenceCommit,
                Boundary::AfterStop,
                Boundary::InsideSettlement,
                Boundary::AfterSettleCommit,
                Boundary::InsideAnswer,
                Boundary::AfterAnswerCommit,
                Boundary::None,
            ],
            Self::Timeout => &[
                Boundary::InsideFence,
                Boundary::AfterFenceCommit,
                Boundary::AfterStop,
                Boundary::InsideSettlement,
                Boundary::AfterSettleCommit,
                Boundary::None,
            ],
        }
    }
}

/// Where the worker can stop: inside each of the stop's transactions and
/// after each commit, right after the runtime is asked to stop the session,
/// and the uninterrupted control. Only the cancellation reserves and answers.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
enum Boundary {
    InsideReservation,
    AfterReservationCommit,
    InsideFence,
    AfterFenceCommit,
    AfterStop,
    InsideSettlement,
    AfterSettleCommit,
    InsideAnswer,
    AfterAnswerCommit,
    None,
}

impl Boundary {
    /// The worker stops inside a write transaction.
    fn inside_transaction(self) -> bool {
        matches!(
            self,
            Self::InsideReservation
                | Self::InsideFence
                | Self::InsideSettlement
                | Self::InsideAnswer
        )
    }
}

/// How far the scene's stop got before the worker died: nothing durable; the
/// cancellation's reservation; the stop fence; the stop itself; its
/// settlement; the cancellation's answer.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Stage {
    Untouched,
    Reserved,
    Fenced,
    Stopped,
    Settled,
    Answered,
}

/// What the store holds once the worker is gone. Only the cancellation
/// reserves first, and only it answers after the settlement.
fn stage(scene: Scene, boundary: Boundary) -> Stage {
    match boundary {
        Boundary::InsideReservation => Stage::Untouched,
        Boundary::AfterReservationCommit => Stage::Reserved,
        Boundary::InsideFence => match scene {
            Scene::Cancellation => Stage::Reserved,
            Scene::Timeout => Stage::Untouched,
        },
        Boundary::AfterFenceCommit => Stage::Fenced,
        Boundary::AfterStop | Boundary::InsideSettlement => Stage::Stopped,
        Boundary::AfterSettleCommit | Boundary::InsideAnswer => Stage::Settled,
        Boundary::AfterAnswerCommit => Stage::Answered,
        Boundary::None => match scene {
            Scene::Cancellation => Stage::Answered,
            Scene::Timeout => Stage::Settled,
        },
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
    run_after_exit: Run,
    cancellation_after_exit: Cancellation,
    worker_stops: u64,
    run_after_restart: Run,
    cancellation_after_restart: Cancellation,
    restart_is_noop: bool,
    request: Answer,
    run_after_pass: Run,
    cancellation_after_pass: Cancellation,
    resend: Answer,
    run_after_settle: Run,
    later_stops: u64,
    launches: u64,
    integrity_ok: bool,
    replay_is_noop: bool,
}

#[derive(Debug, Deserialize, Serialize, PartialEq, Clone, Copy)]
#[serde(rename_all = "snake_case")]
enum InitialOutcome {
    Terminated,
    Finished,
}

/// The store's one run with its occurrence, its one attempt, that attempt's
/// session and the run's stop fence, read together. Every terminal class
/// also pins why the attempt and the occurrence settled.
#[derive(Debug, Deserialize, Serialize, PartialEq, Clone, Copy)]
#[serde(rename_all = "snake_case")]
enum Run {
    /// Launched: the occurrence `running` under the daemon's lease, the run
    /// `running` on the attempt's `running` session, the attempt `started`,
    /// and no stop fence.
    Running,
    /// The same, with the owner's cancellation holding the run's stop fence.
    CancellationFenced,
    /// The same, with the timeout holding the run's stop fence.
    TimeoutFenced,
    /// Behind the cancellation's fence, the restart's containment has marked
    /// the session `killed`; the attempt is still `started` and the run and
    /// occurrence `running`.
    CancellationContained,
    /// The same behind the timeout's fence.
    TimeoutContained,
    /// Behind the timeout's expired fence, the attempt settled `ambiguous` on
    /// its `killed` session and the occurrence `recovery_required` with no
    /// lease, both for the expired stop owner; the run is still `running`.
    Ambiguous,
    /// The enforcement's own settlement: the session `killed`, the attempt
    /// `timed_out` with the timeout failure class, the run finished `failed`
    /// and the occurrence `failed` with no lease, both because the run timed
    /// out, and no stop fence.
    TimedOut,
    /// The same timed-out run, settled instead by the restart: its
    /// containment ended the session after the deadline, and its startup pass
    /// settled the attempt and the occurrence for that.
    EndedAfterDeadline,
    /// The session, the attempt, the run and the occurrence `cancelled` by
    /// the operator, the occurrence with no lease, and no stop fence.
    Cancelled,
    /// The attempt settled `ambiguous` on its `killed` session because the
    /// cancellation's stop outcome was never recorded, the run finished
    /// `failed` and the occurrence `failed` with no lease on the terminal
    /// evidence, and no stop fence.
    FailedAfterCancellation,
    /// The attempt settled `ambiguous` for the timeout's expired stop owner,
    /// the run finished `failed` and the occurrence `failed` with no lease on
    /// the terminal evidence, and no stop fence.
    FailedAfterTimeout,
    /// Anything else, including a partial transaction or two stop fences.
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
    /// refusing an attempt that is no longer cancellable.
    Refused,
    /// `rejected`, with no reservation and a stored `ILLEGAL_TRANSITION`
    /// that the run reached its timeout before cancellation.
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

/// What one request from the owner returned.
#[derive(Debug, Deserialize, Serialize, PartialEq, Clone, Copy)]
#[serde(rename_all = "snake_case")]
enum Answer {
    /// `CANCEL_PENDING`, with no write.
    Pending,
    /// `ILLEGAL_TRANSITION`, which this request stored as its answer and its
    /// only write.
    Refused,
    /// The stored `ILLEGAL_TRANSITION` replayed, with no write.
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

/// What each scene and boundary must produce. A vector restates it, so it
/// cannot weaken it.
fn expected_for(scene: Scene, boundary: Boundary) -> Expected {
    let stage = stage(scene, boundary);
    let (run_after_exit, cancellation_after_exit) = match stage {
        Stage::Untouched => (Run::Running, Cancellation::None),
        Stage::Reserved => (Run::Running, Cancellation::Requested),
        Stage::Fenced | Stage::Stopped => match scene {
            Scene::Cancellation => (Run::CancellationFenced, Cancellation::Stopping),
            Scene::Timeout => (Run::TimeoutFenced, Cancellation::None),
        },
        Stage::Settled => match scene {
            Scene::Cancellation => (Run::Cancelled, Cancellation::Stopping),
            Scene::Timeout => (Run::TimedOut, Cancellation::None),
        },
        Stage::Answered => (Run::Cancelled, Cancellation::Cancelled),
    };
    // The restart's containment marks a session it finds running `killed`.
    // With no fence, the deadline has passed, and its startup pass settles
    // the run as timed out, even under a reserved cancellation. Behind
    // either fence, it leaves the run to the fence's owner until the fence's
    // lease ends.
    let run_after_restart = match stage {
        Stage::Untouched | Stage::Reserved => Run::EndedAfterDeadline,
        Stage::Fenced | Stage::Stopped => match scene {
            Scene::Cancellation => Run::CancellationContained,
            Scene::Timeout => Run::TimeoutContained,
        },
        Stage::Settled | Stage::Answered => run_after_exit,
    };
    // Inside the lease, a reserved cancellation is the dead daemon's to
    // reconcile. With none reserved, the request finds the session the
    // restart ended, and is refused as no longer cancellable, whoever holds
    // the fence. Its own deadline check is never reached.
    let request = match (scene, stage) {
        (Scene::Timeout, _) | (Scene::Cancellation, Stage::Untouched) => Answer::Refused,
        (Scene::Cancellation, Stage::Answered) => Answer::Cancelled,
        (Scene::Cancellation, _) => Answer::Pending,
    };
    // The pass after the leases reconciles what the dead daemon left. A
    // cancellation reserved before the deadline is rejected. A cancellation's
    // unconfirmed stop needs recovery, and the same pass settles the
    // ambiguous run on its ended session and releases the fence. The
    // timeout's expired fence settles the attempt ambiguous and needs
    // recovery; the next pass settles the run and releases the fence (#1461).
    let (run_after_pass, cancellation_after_pass, resend, run_after_settle) = match (scene, stage) {
        (Scene::Cancellation, Stage::Untouched) => (
            Run::EndedAfterDeadline,
            Cancellation::Refused,
            Answer::Rejected,
            Run::EndedAfterDeadline,
        ),
        (Scene::Cancellation, Stage::Reserved) => (
            Run::EndedAfterDeadline,
            Cancellation::Rejected,
            Answer::Rejected,
            Run::EndedAfterDeadline,
        ),
        (Scene::Cancellation, Stage::Fenced | Stage::Stopped) => (
            Run::FailedAfterCancellation,
            Cancellation::RecoveryRequired,
            Answer::RecoveryRequired,
            Run::FailedAfterCancellation,
        ),
        (Scene::Cancellation, Stage::Settled | Stage::Answered) => (
            Run::Cancelled,
            Cancellation::Cancelled,
            Answer::Cancelled,
            Run::Cancelled,
        ),
        (Scene::Timeout, Stage::Fenced | Stage::Stopped) => (
            Run::Ambiguous,
            Cancellation::Refused,
            Answer::Rejected,
            Run::FailedAfterTimeout,
        ),
        // The run was already timed out: nothing is left to reconcile.
        (Scene::Timeout, _) => (
            run_after_restart,
            Cancellation::Refused,
            Answer::Rejected,
            run_after_restart,
        ),
    };
    Expected {
        initial_outcome: if boundary == Boundary::None {
            InitialOutcome::Finished
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
        request,
        run_after_pass,
        cancellation_after_pass,
        resend,
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
    json!({"schemaVersion":1,"id":"conformance-race-crash",
        "name":"Conformance race crash","status":"ACTIVE",
        "rrule":"FREQ=DAILY;BYHOUR=9","timezone":"utc","misfire":"latest",
        "overlap":"forbid","timeoutMinutes":1,"runtime":"coven-code",
        "prompt":"Run the native cancellation timeout race conformance probe.",
        "cwd":"/work/project"})
}

fn validate(case: &Case) -> Result<(), &'static str> {
    if !valid_case_id(&case.case_id)
        || !case.scene.boundaries().contains(&case.boundary)
        || case.definition != synthetic_definition()
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

/// Where the worker parks inside SQLite. The cancellation reserves the
/// adoption key and inserts itself `requested`; claims the stop fence and
/// moves to `stopping`; once the stop is confirmed, settles the session, the
/// attempt, the run and the occurrence and releases the fence; and marks
/// itself `cancelled`, releases the reservation and stores its answer: four
/// transactions. The enforcement claims the stop fence in one transaction and
/// settles in another. Nothing else commits after the pause is installed. A
/// pause right after the stop is the runtime's, not SQLite's.
fn pause_at(scene: Scene, boundary: Boundary) -> Option<Pause<'static>> {
    let (fenced, settled) = match scene {
        Scene::Cancellation => (2, 3),
        Scene::Timeout => (1, 2),
    };
    match boundary {
        Boundary::InsideReservation => Some(Pause::After(
            "AFTER INSERT ON main.automation_cancellations",
        )),
        Boundary::AfterReservationCommit => Some(Pause::AfterCommits(1)),
        Boundary::InsideFence => Some(Pause::After("AFTER INSERT ON main.automation_stop_fences")),
        Boundary::AfterFenceCommit => Some(Pause::AfterCommits(fenced)),
        Boundary::InsideSettlement => Some(Pause::After(
            "AFTER UPDATE OF status ON main.automation_runs
             WHEN OLD.status IS NOT NEW.status",
        )),
        Boundary::AfterSettleCommit => Some(Pause::AfterCommits(settled)),
        Boundary::InsideAnswer => Some(Pause::After(
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
    let leadership = launch_slot(&home, &synthetic_definition(), at(base, CLAIM))?;
    let conn = crate::store::open_store(&crate::api::store_path(&home)).map_err(|_| FAILED)?;
    if run(&conn)? != Run::Running || cancellation(&conn)? != Cancellation::None {
        return Err(FAILED);
    }
    let target = target(&conn)?;
    let handshake = json!({"state":"paused", "scene":case.scene, "boundary":case.boundary,
        "relativeHome":relative_home});
    if let Some(pause_at) = pause_at(case.scene, case.boundary) {
        install_pause(&conn, pause_at, handshake.clone())?;
    }
    let runtime = RecordingStops {
        log: home.join(STOP_LOG),
        park: (case.boundary == Boundary::AfterStop).then_some(handshake),
    };
    let finished = match case.scene {
        // The daemon handles the owner's request as its API does. The API
        // fences the stop on the wall clock; this fences it on the fixture's
        // fixed clock, so its times agree with the scheduler's.
        Scene::Cancellation => {
            let cancel_at = at(base, CANCEL);
            let response = execute_run_cancellation_at_stop_fence(
                &conn,
                &runtime,
                request(&target),
                cancel_at,
                cancel_at,
            )
            .map_err(|_| FAILED)?;
            matches!(&response, CancellationExecution::Success(success)
                if !success.replayed && success.payload == payload(&target, "cancelled", base))
        }
        // The daemon's pass enforces every deadline that has passed.
        Scene::Timeout => enforce_run_timeouts(&conn, &runtime, at(base, ENFORCE))
            .map_err(|_| FAILED)?
            .is_empty(),
    };
    if case.boundary != Boundary::None || !finished {
        return Err(FAILED);
    }
    // Exits like a daemon that dies after the stop: without the graceful
    // shutdown's reconciliation or the release of its scheduler authority.
    drop(conn);
    drop(leadership);
    announce(
        &json!({"state":"finished", "scene":case.scene, "boundary":case.boundary,
        "relativeHome":relative_home}),
    )
}

/// Why the attempt and the occurrence settled, read together.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Reason {
    /// Neither has settled.
    None,
    /// Both because the enforcement found the run timed out.
    TimedOut,
    /// Both because the restart's containment ended the session after the
    /// deadline.
    EndedAfterDeadline,
    /// Both for the operator's cancellation.
    Cancelled,
    /// Both for the timeout's expired stop owner.
    Expired,
    /// The attempt for the timeout's expired stop owner, and the occurrence
    /// for the terminal evidence that settled it.
    ExpiredSettled,
    /// The attempt because the cancellation's stop outcome was never
    /// recorded, and the occurrence for the terminal evidence that settled it.
    Unconfirmed,
    Other,
}

fn reason(attempt: Option<&str>, occurrence: Option<&str>) -> Reason {
    const EXPIRED: &str = "stop owner expired after terminal session evidence";
    const SETTLED: &str = "terminal session evidence observed after an ambiguous stop outcome";
    match (attempt, occurrence) {
        (None, None) => Reason::None,
        (Some("automation run timed out"), Some("automation run timed out")) => Reason::TimedOut,
        (
            Some("session killed at or after automation timeout"),
            Some("session killed at or after automation timeout"),
        ) => Reason::EndedAfterDeadline,
        (
            Some("automation run cancelled by operator"),
            Some("automation run cancelled by operator"),
        ) => Reason::Cancelled,
        (Some(EXPIRED), Some(EXPIRED)) => Reason::Expired,
        (Some(EXPIRED), Some(SETTLED)) => Reason::ExpiredSettled,
        (Some("cancellation stop outcome was not durably recorded"), Some(SETTLED)) => {
            Reason::Unconfirmed
        }
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
                        WHEN (SELECT COUNT(*) FROM automation_stop_fences) != 1 THEN 'other'
                        WHEN EXISTS (
                            SELECT 1 FROM automation_stop_fences
                            WHERE run_id = r.id AND session_id = a.session_id
                              AND owner = 'cancellation' AND operation_key = ?1
                        ) THEN 'cancellation'
                        WHEN EXISTS (
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
            [ADOPTION_KEY],
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
    let live = (
        occurrence_state,
        lease,
        run,
        attempt_state,
        class.as_deref(),
    );
    let launched = ("running", "daemon", "running", "started", None);
    let reason = reason(attempt.as_deref(), occurrence.as_deref());
    Ok(match (live, settled, finished, session, fence, reason) {
        (live, false, false, "running", "none", Reason::None) if live == launched => Run::Running,
        (live, false, false, "running", "cancellation", Reason::None) if live == launched => {
            Run::CancellationFenced
        }
        (live, false, false, "running", "timeout", Reason::None) if live == launched => {
            Run::TimeoutFenced
        }
        (live, false, false, "killed", "cancellation", Reason::None) if live == launched => {
            Run::CancellationContained
        }
        (live, false, false, "killed", "timeout", Reason::None) if live == launched => {
            Run::TimeoutContained
        }
        (
            ("recovery_required", "none", "running", "ambiguous", Some("ambiguous_evidence")),
            true,
            false,
            "killed",
            "timeout",
            Reason::Expired,
        ) => Run::Ambiguous,
        (
            ("failed", "none", "failed", "timed_out", Some("timeout")),
            true,
            true,
            "killed",
            "none",
            Reason::TimedOut,
        ) => Run::TimedOut,
        (
            ("failed", "none", "failed", "timed_out", Some("timeout")),
            true,
            true,
            "killed",
            "none",
            Reason::EndedAfterDeadline,
        ) => Run::EndedAfterDeadline,
        (
            ("cancelled", "none", "cancelled", "cancelled", Some("cancelled")),
            true,
            true,
            "cancelled",
            "none",
            Reason::Cancelled,
        ) => Run::Cancelled,
        (
            ("failed", "none", "failed", "ambiguous", Some("ambiguous_evidence")),
            true,
            true,
            "killed",
            "none",
            Reason::Unconfirmed,
        ) => Run::FailedAfterCancellation,
        (
            ("failed", "none", "failed", "ambiguous", Some("ambiguous_evidence")),
            true,
            true,
            "killed",
            "none",
            Reason::ExpiredSettled,
        ) => Run::FailedAfterTimeout,
        _ => Run::Mixed,
    })
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
    // The stored answer's outcome with its error code and message, or its
    // result status.
    let answer = adoption
        .map(|(outcome, response)| {
            let response: Value = serde_json::from_str(&response).map_err(|_| FAILED)?;
            let error = &response["error"];
            let detail = match outcome.as_str() {
                "rejected" => error["code"]
                    .as_str()
                    .zip(error["message"].as_str())
                    .map(|(code, message)| format!("{code}: {message}")),
                "committed" => response["result"]["status"].as_str().map(str::to_owned),
                _ => None,
            };
            Ok::<_, &'static str>((outcome, detail.unwrap_or_default()))
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
            (0, 0, 1, None, false, Some(("rejected", ENDED))) => Cancellation::Refused,
            (1, 0, 1, Some("rejected"), false, Some(("rejected", OVERTAKEN))) => {
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

/// Every row the race, its recovery or a restart can write, for an exact
/// before/after comparison.
fn snapshot(conn: &Connection) -> Result<Vec<Vec<rusqlite::types::Value>>, &'static str> {
    let mut rows = snapshot_tables(conn, &STATE_TABLES)?;
    rows.extend(snapshot_tables(conn, &["automation_command_adoptions"])?);
    Ok(rows)
}

/// The error the store holds as the owner's answer, as it was returned.
fn stored_error(conn: &Connection) -> Result<Option<Value>, &'static str> {
    let response: Option<String> = conn
        .query_row(
            "SELECT response_json FROM automation_command_adoptions
             WHERE adoption_key = ?1 AND command = 'run.cancel.v1' AND outcome = 'rejected'",
            [ADOPTION_KEY],
            |row| row.get(0),
        )
        .optional()
        .map_err(|_| FAILED)?;
    response
        .map(|response| {
            let mut response: Value = serde_json::from_str(&response).map_err(|_| FAILED)?;
            Ok(response["error"].take())
        })
        .transpose()
}

/// Sends the owner's request at `now` through `runtime` and classifies the
/// answer against the store. A request that errors is refused recovery.
fn send(
    conn: &Connection,
    runtime: &CountingRuntime,
    target: &Target,
    now: DateTime<Utc>,
) -> Result<Answer, &'static str> {
    let state = snapshot_tables(conn, &STATE_TABLES)?;
    let answers = snapshot_tables(conn, &["automation_command_adoptions"])?;
    let execution =
        execute_run_cancellation_at_stop_fence(conn, runtime, request(target), now, now)
            .map_err(|_| REFUSED)?;
    let state_kept = snapshot_tables(conn, &STATE_TABLES)? == state;
    let answers_after = snapshot_tables(conn, &["automation_command_adoptions"])?;
    let wrote = !state_kept || answers_after != answers;
    let one_more_answer = answers_after.len() == answers.len() + 1
        && answers.iter().all(|row| answers_after.contains(row));
    let base = base()?;
    Ok(match execution {
        CancellationExecution::Rejected(error) => {
            // An `ILLEGAL_TRANSITION` is the one the store holds, word for
            // word. Storing it is the only write a refusal makes.
            let returned = serde_json::to_value(&error).map_err(|_| FAILED)?;
            let as_stored = stored_error(conn)? == Some(returned);
            match (error.code(), wrote) {
                (ErrorCode::CancelPending, false) => Answer::Pending,
                (ErrorCode::IllegalTransition, true)
                    if as_stored
                        && state_kept
                        && one_more_answer
                        && cancellation(conn)? == Cancellation::Refused =>
                {
                    Answer::Refused
                }
                (ErrorCode::IllegalTransition, false) if as_stored => Answer::Rejected,
                _ => Answer::Mismatched,
            }
        }
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

/// What the restarted daemon, its passes and the owner's requests did with
/// the store the worker left.
#[derive(Debug)]
struct Restarted {
    run_after_restart: Run,
    cancellation_after_restart: Cancellation,
    restart_is_noop: bool,
    request: Answer,
    run_after_pass: Run,
    cancellation_after_pass: Cancellation,
    resend: Answer,
    run_after_settle: Run,
    later_stops: u64,
    launches: u64,
    replay_is_noop: bool,
}

fn recover(home: &Path, path: &Path, base: DateTime<Utc>) -> Result<Restarted, &'static str> {
    // One runtime confirms every stop and counts it, with every launch, for
    // the restart, each pass and each request.
    let runtime = CountingRuntime::default();
    let before_restart = snapshot(&crate::store::open_store(path).map_err(|_| FAILED)?)?;
    let leadership = restart(home, &runtime, at(base, RESTART))?;
    let conn = crate::store::open_store(path).map_err(|_| FAILED)?;
    let run_after_restart = run(&conn)?;
    let cancellation_after_restart = cancellation(&conn)?;
    let restart_is_noop = snapshot(&conn)? == before_restart;
    let target = target(&conn)?;
    // The owner never saw an answer, or never got to ask, so they send the
    // request to the restarted daemon.
    let request = send(&conn, &runtime, &target, at(base, REQUEST))?;
    drop(conn);
    pass(home, &runtime, &leadership, at(base, PASS), "deadline")?;
    let conn = crate::store::open_store(path).map_err(|_| FAILED)?;
    let run_after_pass = run(&conn)?;
    let cancellation_after_pass = cancellation(&conn)?;
    let resend = send(&conn, &runtime, &target, at(base, RESEND))?;
    drop(conn);
    pass(home, &runtime, &leadership, at(base, SETTLE), "deadline")?;
    let conn = crate::store::open_store(path).map_err(|_| FAILED)?;
    let run_after_settle = run(&conn)?;
    let later_stops = runtime.stops();
    let launches = runtime.launches();
    let before_replay = snapshot(&conn)?;
    // The same request once more, then a pass on its own connection, must
    // answer the same without a write, a stop or a launch.
    let replayed = send(&conn, &runtime, &target, at(base, REPLAY))? == resend;
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
        request,
        run_after_pass,
        cancellation_after_pass,
        resend,
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
    scene: Scene,
    boundary: Boundary,
    relative_home: String,
}

pub(super) fn evaluate(vector: &Value) -> Result<(bool, Vec<Value>), &'static str> {
    let vectors: VectorSet = serde_json::from_value(vector.clone()).map_err(|_| INVALID)?;
    // Every scene's boundaries, once each, in order.
    let pairs: Vec<(Scene, Boundary)> = Scene::ALL
        .iter()
        .flat_map(|scene| {
            scene
                .boundaries()
                .iter()
                .map(|boundary| (*scene, *boundary))
        })
        .collect();
    if vectors.schema_version != SCHEMA_VERSION || vectors.cases.len() != pairs.len() {
        return Err(INVALID);
    }
    let mut ids = BTreeSet::new();
    for (case, pair) in vectors.cases.iter().zip(&pairs) {
        validate(case)?;
        if !ids.insert(&case.case_id) || (case.scene, case.boundary) != *pair {
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
    if ready.scene != case.scene
        || ready.boundary != case.boundary
        || ready.state != if crash { "paused" } else { "finished" }
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
            InitialOutcome::Finished
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
        request: recovery
            .as_ref()
            .map_or(Answer::Mismatched, |recovery| recovery.request),
        run_after_pass: recovery
            .as_ref()
            .map_or(Run::Mixed, |recovery| recovery.run_after_pass),
        cancellation_after_pass: recovery.as_ref().map_or(Cancellation::Mixed, |recovery| {
            recovery.cancellation_after_pass
        }),
        resend: recovery
            .as_ref()
            .map_or(Answer::Mismatched, |recovery| recovery.resend),
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
    let (transaction_open_at_pause, run_at_pause, cancellation_at_pause) = match pause {
        Some((open, run, cancellation)) => (Some(open), Some(run), Some(cancellation)),
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
            "request": observed.request,
            "runAfterPass": observed.run_after_pass,
            "cancellationAfterPass": observed.cancellation_after_pass,
            "resend": observed.resend,
            "runAfterSettle": observed.run_after_settle,
            "laterStops": observed.later_stops,
            "launches": observed.launches,
            "replayIsNoop": observed.replay_is_noop,
        }),
    ))
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::path::PathBuf;

    use super::*;
    use crate::automations::leadership::SchedulerLeadership;

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

    /// Records each commit's row changes, leaving out the event feed's own
    /// bookkeeping, which rides inside the transition it records.
    fn record_changes(conn: &Connection) {
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
    }

    fn vectors() -> Value {
        serde_json::from_str(include_str!(
            "../../../../../conformance/automations/runner/cancellation-timeout-process-crash-recovery.vectors.json"
        ))
        .unwrap()
    }

    fn pairs() -> Vec<(Scene, Boundary)> {
        Scene::ALL
            .iter()
            .flat_map(|scene| {
                scene
                    .boundaries()
                    .iter()
                    .map(move |boundary| (*scene, *boundary))
            })
            .collect()
    }

    #[test]
    fn race_crash_vectors_restate_each_case_outcome() {
        let vectors: VectorSet = serde_json::from_value(vectors()).unwrap();
        assert_eq!(vectors.schema_version, SCHEMA_VERSION);
        let cases: Vec<(Scene, Boundary)> = vectors
            .cases
            .iter()
            .map(|case| (case.scene, case.boundary))
            .collect();
        assert_eq!(cases, pairs());
        assert_eq!(cases.len(), 16);
        for case in &vectors.cases {
            validate(case).unwrap();
        }
    }

    #[test]
    fn race_crash_vectors_require_every_case_once_in_order() {
        let mut missing = vectors();
        missing["cases"].as_array_mut().unwrap().pop();
        let mut repeated = vectors();
        repeated["cases"][1] = repeated["cases"][0].clone();
        let mut reordered = vectors();
        reordered["cases"].as_array_mut().unwrap().swap(0, 1);
        let mut renamed = vectors();
        renamed["cases"][1]["caseId"] = renamed["cases"][0]["caseId"].clone();
        let mut versioned = vectors();
        versioned["schemaVersion"] =
            json!("coven.automations.run-cancellation-process-crash-recovery-vectors.v1");
        for vector in [missing, repeated, reordered, renamed, versioned] {
            assert_eq!(evaluate(&vector).unwrap_err(), INVALID);
        }
        // A boundary the timeout scene never reaches.
        let mut case = vectors()["cases"][10].clone();
        case["boundary"] = json!("inside_reservation");
        case["expected"] = serde_json::to_value(expected_for(
            Scene::Cancellation,
            Boundary::InsideReservation,
        ))
        .unwrap();
        let case: Case = serde_json::from_value(case).unwrap();
        assert_eq!(validate(&case).unwrap_err(), INVALID);
    }

    #[test]
    fn race_crash_vectors_refuse_malformed_or_weakened_inputs() {
        let original = vectors()["cases"][3].clone();
        let refused = |case: Value| match serde_json::from_value::<Case>(case) {
            Ok(case) => validate(&case).is_err(),
            Err(_) => true,
        };
        let mut unknown = original.clone();
        unknown["storePath"] = json!("private-input");
        assert!(refused(unknown));
        for (field, value) in [
            ("timeoutMinutes", json!(30)),
            ("id", json!("conformance-cancel-crash")),
            ("status", json!("PAUSED")),
            ("prompt", json!("Run something else.")),
            ("cwd", Value::Null),
        ] {
            let mut case = original.clone();
            case["definition"][field] = value;
            assert!(refused(case), "{field}");
        }
        let mut scene = original.clone();
        scene["scene"] = json!("timeout");
        assert!(refused(scene));
        // Every expectation of every case, changed to any other value that
        // still parses, is refused.
        const CLASSES: [&str; 22] = [
            "terminated",
            "finished",
            "running",
            "cancellation_fenced",
            "timeout_fenced",
            "cancellation_contained",
            "timeout_contained",
            "ambiguous",
            "timed_out",
            "ended_after_deadline",
            "cancelled",
            "failed_after_cancellation",
            "failed_after_timeout",
            "mixed",
            "none",
            "requested",
            "stopping",
            "refused",
            "rejected",
            "recovery_required",
            "pending",
            "mismatched",
        ];
        let mut weakened = 0;
        for case in vectors()["cases"].as_array().unwrap() {
            for (field, value) in case["expected"].as_object().unwrap() {
                let alternatives: Vec<Value> = match value {
                    Value::Bool(flag) => vec![json!(!flag)],
                    Value::Number(number) => vec![json!(number.as_u64().unwrap() + 1)],
                    Value::String(class) => CLASSES
                        .iter()
                        .filter(|other| *other != class)
                        .map(|other| json!(other))
                        .collect(),
                    _ => panic!("{field}"),
                };
                for alternative in alternatives {
                    let mut changed = case.clone();
                    changed["expected"][field] = alternative;
                    if serde_json::from_value::<Case>(changed.clone()).is_ok() {
                        assert!(refused(changed), "{field}");
                        weakened += 1;
                    }
                }
            }
        }
        assert!(weakened > 16 * 16, "{weakened}");
    }

    /// A store whose routine has launched its 09:00 slot, as the worker
    /// leaves it before the race.
    fn launched_store() -> (
        tempfile::TempDir,
        PathBuf,
        Connection,
        SchedulerLeadership,
        Target,
    ) {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        crate::daemon::ensure_private_coven_home(&home).unwrap();
        let leadership =
            launch_slot(&home, &synthetic_definition(), at(base().unwrap(), CLAIM)).unwrap();
        let conn = crate::store::open_store(&crate::api::store_path(&home)).unwrap();
        assert_eq!(run(&conn).unwrap(), Run::Running);
        let target = target(&conn).unwrap();
        (temp, home, conn, leadership, target)
    }

    fn recording(home: &Path) -> RecordingStops {
        RecordingStops {
            log: home.join(STOP_LOG),
            park: None,
        }
    }

    #[test]
    fn the_cancellation_commits_four_times_and_the_enforcement_twice() {
        let base = base().unwrap();
        let (_temp, home, conn, _leadership, target) = launched_store();
        record_changes(&conn);
        execute_run_cancellation_at_stop_fence(
            &conn,
            &recording(&home),
            request(&target),
            at(base, CANCEL),
            at(base, CANCEL),
        )
        .unwrap();
        conn.wal_hook(None);
        assert_eq!(
            COMMITS.with(|commits| commits.take()),
            [
                vec![
                    "insert automation_command_reservations",
                    "insert automation_cancellations",
                ],
                vec![
                    "insert automation_stop_fences",
                    "update automation_cancellations"
                ],
                vec![
                    "update sessions",
                    "update automation_attempts",
                    "update automation_runs",
                    "update automation_occurrences",
                    "delete automation_stop_fences",
                ],
                vec![
                    "update automation_cancellations",
                    "delete automation_command_reservations",
                    "insert automation_command_adoptions",
                ],
            ]
        );
        let (_temp, home, conn, _leadership, _target) = launched_store();
        record_changes(&conn);
        assert!(
            enforce_run_timeouts(&conn, &recording(&home), at(base, ENFORCE))
                .unwrap()
                .is_empty()
        );
        conn.wal_hook(None);
        assert_eq!(
            COMMITS.with(|commits| commits.take()),
            [
                vec!["insert automation_stop_fences"],
                vec![
                    "update sessions",
                    "update automation_attempts",
                    "update automation_runs",
                    "update automation_occurrences",
                    "delete automation_stop_fences",
                ],
            ]
        );
    }

    /// The write a worker stopped at `boundary` never committed: the one a
    /// kill inside a transaction interrupts, which is where its pause sits,
    /// or the first write of the transaction after a commit.
    fn killed_at(scene: Scene, boundary: Boundary) -> Option<&'static str> {
        match boundary {
            Boundary::InsideReservation
            | Boundary::InsideFence
            | Boundary::InsideSettlement
            | Boundary::InsideAnswer => match pause_at(scene, boundary) {
                Some(Pause::After(clause)) => Some(clause),
                _ => panic!("{boundary:?}"),
            },
            Boundary::AfterReservationCommit => Some("AFTER INSERT ON main.automation_stop_fences"),
            Boundary::AfterFenceCommit | Boundary::AfterStop => {
                Some("AFTER UPDATE OF status ON main.sessions WHEN OLD.status IS NOT NEW.status")
            }
            Boundary::AfterSettleCommit => match scene {
                Scene::Cancellation => Some(
                    "AFTER UPDATE OF state ON main.automation_cancellations
                     WHEN NEW.state = 'cancelled'",
                ),
                Scene::Timeout => None,
            },
            Boundary::AfterAnswerCommit | Boundary::None => None,
        }
    }

    /// The store a worker stopped at `boundary` in `scene` leaves. A TEMP
    /// trigger aborts the write it never committed. Killed before the stop,
    /// the runtime records none.
    fn stopped_store(scene: Scene, boundary: Boundary) -> (tempfile::TempDir, PathBuf) {
        let (temp, home, conn, leadership, target) = launched_store();
        let base = base().unwrap();
        if let Some(clause) = killed_at(scene, boundary) {
            conn.execute_batch(&format!(
                "CREATE TEMP TRIGGER killed {clause} BEGIN SELECT RAISE(ABORT, 'killed'); END;"
            ))
            .unwrap();
        }
        let recording = recording(&home);
        let runtime: &dyn crate::api::SessionRuntime = if boundary == Boundary::AfterFenceCommit {
            &crate::api::NoopSessionRuntime
        } else {
            &recording
        };
        let finished = match scene {
            Scene::Cancellation => matches!(
                execute_run_cancellation_at_stop_fence(
                    &conn,
                    runtime,
                    request(&target),
                    at(base, CANCEL),
                    at(base, CANCEL),
                ),
                Ok(CancellationExecution::Success(_))
            ),
            Scene::Timeout => enforce_run_timeouts(&conn, runtime, at(base, ENFORCE))
                .is_ok_and(|failures| failures.is_empty()),
        };
        assert_eq!(
            finished,
            killed_at(scene, boundary).is_none(),
            "{boundary:?}"
        );
        let expected = expected_for(scene, boundary);
        assert_eq!(
            (
                run(&conn).unwrap(),
                cancellation(&conn).unwrap(),
                worker_stops(&home).unwrap()
            ),
            (
                expected.run_after_exit,
                expected.cancellation_after_exit,
                expected.worker_stops
            ),
            "{scene:?} {boundary:?}"
        );
        drop(conn);
        drop(leadership);
        (temp, home)
    }

    #[test]
    fn each_case_store_recovers_as_its_vector_says() {
        for (scene, boundary) in pairs() {
            let (_temp, home) = stopped_store(scene, boundary);
            let recovery = recover(&home, &crate::api::store_path(&home), base().unwrap()).unwrap();
            let expected = expected_for(scene, boundary);
            assert_eq!(
                (
                    recovery.run_after_restart,
                    recovery.cancellation_after_restart,
                    recovery.restart_is_noop,
                    recovery.request,
                    recovery.run_after_pass,
                    recovery.cancellation_after_pass,
                    recovery.resend,
                    recovery.run_after_settle,
                    recovery.later_stops,
                    recovery.launches,
                    recovery.replay_is_noop,
                ),
                (
                    expected.run_after_restart,
                    expected.cancellation_after_restart,
                    expected.restart_is_noop,
                    expected.request,
                    expected.run_after_pass,
                    expected.cancellation_after_pass,
                    expected.resend,
                    expected.run_after_settle,
                    expected.later_stops,
                    expected.launches,
                    expected.replay_is_noop,
                ),
                "{scene:?} {boundary:?}"
            );
        }
    }

    /// The store a worker stopped at `boundary` in `scene` leaves, restarted
    /// and passed at each of `passes` when they are given.
    fn later_store(
        scene: Scene,
        boundary: Boundary,
        passes: Option<&[i64]>,
    ) -> (tempfile::TempDir, PathBuf) {
        let (temp, home) = stopped_store(scene, boundary);
        if let Some(passes) = passes {
            let base = base().unwrap();
            let runtime = CountingRuntime::default();
            let leadership = restart(&home, &runtime, at(base, RESTART)).unwrap();
            for &seconds in passes {
                pass(&home, &runtime, &leadership, at(base, seconds), "deadline").unwrap();
            }
            drop(leadership);
        }
        (temp, home)
    }

    /// Applies `torn` inside a transaction that is rolled back, after
    /// dropping every trigger that would refuse it, and classifies what it
    /// leaves.
    fn torn<T>(
        conn: &Connection,
        torn: &str,
        classify: fn(&Connection) -> Result<T, &'static str>,
    ) -> T {
        conn.execute_batch("BEGIN").unwrap();
        let triggers: Vec<String> = conn
            .prepare("SELECT name FROM sqlite_master WHERE type = 'trigger'")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        for trigger in triggers {
            conn.execute_batch(&format!("DROP TRIGGER \"{trigger}\""))
                .unwrap();
        }
        conn.execute_batch(torn).unwrap();
        let class = classify(conn).unwrap();
        conn.execute_batch("ROLLBACK").unwrap();
        class
    }

    /// A store after `passes`, the class it must hold, and a write that tears
    /// it.
    type Tear<'a> = (Scene, Boundary, Option<&'a [i64]>, Run, &'a str);

    #[test]
    fn runs_are_whole_or_mixed() {
        const SETTLED: &str = "terminal session evidence observed after an ambiguous stop outcome";
        // Each class, torn by one of the facts that distinguishes it.
        let cases: [Tear; 15] = [
            (
                Scene::Cancellation,
                Boundary::AfterFenceCommit,
                None,
                Run::CancellationFenced,
                "UPDATE automation_stop_fences SET operation_key = 'adopt:other'",
            ),
            (
                Scene::Timeout,
                Boundary::AfterFenceCommit,
                None,
                Run::TimeoutFenced,
                "UPDATE automation_stop_fences SET owner = 'cancellation'",
            ),
            (
                Scene::Cancellation,
                Boundary::InsideReservation,
                None,
                Run::Running,
                "UPDATE automation_occurrences SET lease_owner = NULL, lease_expires_at = NULL",
            ),
            (
                Scene::Timeout,
                Boundary::None,
                None,
                Run::TimedOut,
                "UPDATE automation_attempts
                 SET state_reason = 'session killed at or after automation timeout'",
            ),
            (
                Scene::Timeout,
                Boundary::InsideFence,
                None,
                Run::Running,
                "UPDATE automation_runs SET exit_code = 0",
            ),
            (
                Scene::Cancellation,
                Boundary::None,
                None,
                Run::Cancelled,
                "UPDATE sessions SET status = 'killed'",
            ),
            (
                Scene::Cancellation,
                Boundary::None,
                None,
                Run::Cancelled,
                "UPDATE automation_occurrences SET failure_reason = 'automation run timed out'",
            ),
            (
                Scene::Timeout,
                Boundary::AfterSettleCommit,
                None,
                Run::TimedOut,
                "UPDATE automation_runs SET finished_at = NULL",
            ),
            (
                Scene::Timeout,
                Boundary::AfterSettleCommit,
                None,
                Run::TimedOut,
                "INSERT INTO automation_stop_fences
                    (run_id, session_id, owner, operation_key, acquired_at, execution_expires_at)
                 SELECT id, session_id, 'timeout', NULL, started_at, started_at
                 FROM automation_runs",
            ),
            (
                Scene::Cancellation,
                Boundary::AfterStop,
                Some(&[]),
                Run::CancellationContained,
                "UPDATE automation_runs SET finished_at = started_at",
            ),
            (
                Scene::Timeout,
                Boundary::AfterStop,
                Some(&[]),
                Run::TimeoutContained,
                "UPDATE automation_stop_fences SET operation_key = 'adopt:other'",
            ),
            (
                Scene::Cancellation,
                Boundary::InsideReservation,
                Some(&[]),
                Run::EndedAfterDeadline,
                "UPDATE automation_occurrences SET failure_reason = 'automation run timed out'",
            ),
            (
                Scene::Timeout,
                Boundary::AfterStop,
                Some(&[PASS]),
                Run::Ambiguous,
                "UPDATE automation_occurrences SET failure_reason =
                 'terminal session evidence observed after an ambiguous stop outcome'",
            ),
            (
                Scene::Cancellation,
                Boundary::AfterStop,
                Some(&[PASS]),
                Run::FailedAfterCancellation,
                "UPDATE automation_attempts SET failure_class = 'timeout'",
            ),
            (
                Scene::Timeout,
                Boundary::AfterStop,
                Some(&[PASS, SETTLE]),
                Run::FailedAfterTimeout,
                "UPDATE automation_occurrences SET state = 'recovery_required'",
            ),
        ];
        for (scene, boundary, passes, whole, sql) in cases {
            let (_temp, home) = later_store(scene, boundary, passes);
            let conn = crate::store::open_store(&crate::api::store_path(&home)).unwrap();
            assert_eq!(run(&conn).unwrap(), whole, "{sql}");
            assert_eq!(torn(&conn, sql, run), Run::Mixed, "{sql}");
            assert_eq!(run(&conn).unwrap(), whole, "{sql}");
        }
        // The settlement reasons tell the two ambiguous failures apart.
        let (_temp, home) = later_store(Scene::Cancellation, Boundary::AfterStop, Some(&[PASS]));
        let conn = crate::store::open_store(&crate::api::store_path(&home)).unwrap();
        assert_eq!(run(&conn).unwrap(), Run::FailedAfterCancellation);
        assert_eq!(
            torn(
                &conn,
                "UPDATE automation_attempts
                 SET state_reason = 'stop owner expired after terminal session evidence'",
                run
            ),
            Run::FailedAfterTimeout
        );
        assert_eq!(
            torn(
                &conn,
                &format!("UPDATE automation_occurrences SET failure_reason = '{SETTLED}'"),
                run
            ),
            Run::FailedAfterCancellation
        );
    }

    #[test]
    fn cancellations_are_whole_or_mixed() {
        const RESERVE: &str = "INSERT INTO automation_command_reservations
            (adoption_key, request_digest, command, reserved_at)
            VALUES ('adopt:conformance:race-crash', 'digest', 'run.cancel.v1',
                    '2099-01-01T09:00:00Z')";
        // Each class, torn by one of the facts that distinguishes it. A
        // refusal and a rejection differ by their stored message alone.
        let cases: [(Scene, Boundary, bool, Cancellation, &str); 7] = [
            (
                Scene::Cancellation,
                Boundary::InsideReservation,
                false,
                Cancellation::None,
                RESERVE,
            ),
            (
                Scene::Cancellation,
                Boundary::AfterReservationCommit,
                false,
                Cancellation::Requested,
                "DELETE FROM automation_command_reservations",
            ),
            (
                Scene::Cancellation,
                Boundary::AfterFenceCommit,
                false,
                Cancellation::Stopping,
                "UPDATE automation_command_reservations SET command = 'occurrence.cancel.v1'",
            ),
            (
                Scene::Cancellation,
                Boundary::InsideReservation,
                true,
                Cancellation::Refused,
                "UPDATE automation_command_adoptions SET response_json = json_set(
                    response_json, '$.error.message',
                    'the targeted automation run reached its timeout before cancellation')",
            ),
            (
                Scene::Cancellation,
                Boundary::AfterReservationCommit,
                true,
                Cancellation::Rejected,
                "UPDATE automation_command_adoptions SET response_json = json_set(
                    response_json, '$.error.message',
                    'the targeted automation attempt is no longer cancellable')",
            ),
            (
                Scene::Cancellation,
                Boundary::AfterStop,
                true,
                Cancellation::RecoveryRequired,
                "UPDATE automation_command_adoptions SET outcome = 'rejected'",
            ),
            (
                Scene::Cancellation,
                Boundary::None,
                false,
                Cancellation::Cancelled,
                RESERVE,
            ),
        ];
        for (scene, boundary, recovered, whole, sql) in cases {
            let (_temp, home) = stopped_store(scene, boundary);
            let path = crate::api::store_path(&home);
            if recovered {
                recover(&home, &path, base().unwrap()).unwrap();
            }
            let conn = crate::store::open_store(&path).unwrap();
            assert_eq!(cancellation(&conn).unwrap(), whole, "{sql}");
            assert_eq!(torn(&conn, sql, cancellation), Cancellation::Mixed, "{sql}");
            assert_eq!(cancellation(&conn).unwrap(), whole, "{sql}");
        }
    }

    #[test]
    fn a_refusal_that_writes_anything_else_is_mismatched() {
        let base = base().unwrap();
        // Storing the refusal is all a refusal may write: one that also
        // renews the timeout's fence is not a refusal.
        let renew = format!(
            "UPDATE automation_stop_fences SET execution_expires_at = '{}'",
            iso(at(base, 127))
        );
        for (extra, answer) in [(None, Answer::Refused), (Some(renew), Answer::Mismatched)] {
            let (_temp, home) = later_store(Scene::Timeout, Boundary::AfterStop, Some(&[]));
            let conn = crate::store::open_store(&crate::api::store_path(&home)).unwrap();
            if let Some(sql) = &extra {
                conn.execute_batch(&format!(
                    "CREATE TEMP TRIGGER extra AFTER INSERT ON main.automation_command_adoptions
                     BEGIN {sql}; END;"
                ))
                .unwrap();
            }
            let runtime = CountingRuntime::default();
            let target = target(&conn).unwrap();
            assert_eq!(
                send(&conn, &runtime, &target, at(base, REQUEST)).unwrap(),
                answer,
                "{extra:?}"
            );
        }
    }

    #[test]
    fn worker_refuses_invalid_or_weakened_input() {
        let mut case = vectors()["cases"][0].clone();
        case["storePath"] = json!("private-input");
        assert_eq!(worker(&case).unwrap_err(), INVALID);
        let mut case = vectors()["cases"][4].clone();
        case["expected"]["workerStops"] = json!(0);
        assert_eq!(worker(&case).unwrap_err(), INVALID);
        let mut case = vectors()["cases"][0].clone();
        case["scene"] = json!("timeout");
        assert_eq!(worker(&case).unwrap_err(), INVALID);
    }
}
