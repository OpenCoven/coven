//! Disposable flat-cancellation crash fixtures: a daemon worker is killed
//! while it handles the owner's `occurrence.cancel.v1` or `attempt.cancel.v1`.
//! A slot that has not dispatched is withdrawn in one transaction with the
//! command's answer. A running attempt is handed to a run cancellation under a
//! key derived from the owner's: the command first stores that delegation as
//! its answer, then sends the run cancellation, which keeps its own. The
//! parent restarts the daemon's scheduler inside the run cancellation's lease
//! and sends the same flat request again, before and after a pass reconciles
//! it. Both launch through no-op runtimes; no runtime process starts.
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
use super::run_launch_process_crash::{pass, restart, snapshot_tables, CountingRuntime};
use super::valid_case_id;
use crate::automations::cancel_commands::{
    execute_attempt_cancel_at_stop_fence, execute_occurrence_cancel_at_stop_fence, CancelExecution,
};
use crate::automations::contract::canonical_json::sha256_hex;
use crate::automations::contract::error::ErrorCode;
use crate::automations::definition::RoutineDefinition;
use crate::automations::leadership::SchedulerLeadership;

const INVALID: &str = "conformance vector is invalid";
const FAILED: &str = "conformance suite execution failed";
/// A production recovery step or the re-sent command returned an error; the
/// case fails.
const REFUSED: &str = "cancellation refused";
const SCHEMA_VERSION: &str =
    "coven.automations.flat-cancellation-process-crash-recovery-vectors.v1";
const WORKER: &str = "flat-cancellation-crash-worker";
const HOME_PREFIX: &str = "flat-cancel-crash-";
/// The definition predates the schedule slot the worker claims.
const DEFINED_AT: &str = "2099-10-19T09:00:00.000Z";
/// The one adoption key the owner's flat cancellation carries, sent and
/// re-sent.
const ADOPTION_KEY: &str = "adopt:conformance:flat-cancel-crash";
const REASON: &str = "not needed today";
/// Seconds after `BASE`: the worker claims the 09:00 slot, launching it for a
/// running attempt, and handles the owner's cancellation; a run cancellation's
/// execution lease ends 30 seconds after it is requested. The parent restarts
/// the daemon and the owner re-sends the request inside that lease; a pass
/// after it reconciles the cancellation, and the owner sends the request
/// again. The run's 30-minute deadline is never reached.
const CLAIM: i64 = 30;
const CANCEL: i64 = 60;
const RESTART: i64 = 70;
const REISSUE: i64 = 80;
const PASS: i64 = 150;
const RESEND: i64 = 160;
const REPLAY: i64 = 240;

/// What the owner cancels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Scene {
    /// `occurrence.cancel.v1` for the claimed 09:00 slot, before any dispatch.
    Claimed,
    /// `attempt.cancel.v1` for the slot's running attempt.
    Launched,
}

impl Scene {
    fn command(self) -> &'static str {
        match self {
            Self::Claimed => "occurrence.cancel.v1",
            Self::Launched => "attempt.cancel.v1",
        }
    }
}

/// Where the worker can stop. For a claimed slot: after each write of the one
/// transaction, after its commit, and the uninterrupted control. For a running
/// attempt: after the delegation's write and commit, after each commit of the
/// run cancellation it sends and right after that cancellation's stop, and the
/// uninterrupted control.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
enum Boundary {
    AfterOccurrenceCancel,
    AfterWithdrawalAdoption,
    AfterWithdrawalCommit,
    ClaimedControl,
    AfterDelegationAdoption,
    AfterDelegationCommit,
    AfterReservationCommit,
    AfterFenceCommit,
    AfterStop,
    AfterSettleCommit,
    AfterAnswerCommit,
    LaunchedControl,
}

impl Boundary {
    const ALL: [Self; 12] = [
        Self::AfterOccurrenceCancel,
        Self::AfterWithdrawalAdoption,
        Self::AfterWithdrawalCommit,
        Self::ClaimedControl,
        Self::AfterDelegationAdoption,
        Self::AfterDelegationCommit,
        Self::AfterReservationCommit,
        Self::AfterFenceCommit,
        Self::AfterStop,
        Self::AfterSettleCommit,
        Self::AfterAnswerCommit,
        Self::LaunchedControl,
    ];

    fn scene(self) -> Scene {
        match self {
            Self::AfterOccurrenceCancel
            | Self::AfterWithdrawalAdoption
            | Self::AfterWithdrawalCommit
            | Self::ClaimedControl => Scene::Claimed,
            _ => Scene::Launched,
        }
    }

    fn control(self) -> bool {
        matches!(self, Self::ClaimedControl | Self::LaunchedControl)
    }

    /// The worker stops inside a write transaction.
    fn inside_transaction(self) -> bool {
        matches!(
            self,
            Self::AfterOccurrenceCancel
                | Self::AfterWithdrawalAdoption
                | Self::AfterDelegationAdoption
        )
    }

    /// What the store holds once the worker is gone.
    fn stage(self) -> Stage {
        match self {
            Self::AfterOccurrenceCancel | Self::AfterWithdrawalAdoption => Stage::Claimed,
            Self::AfterWithdrawalCommit | Self::ClaimedControl => Stage::Withdrawn,
            Self::AfterDelegationAdoption => Stage::Untouched,
            Self::AfterDelegationCommit => Stage::Delegated,
            Self::AfterReservationCommit => Stage::Reserved,
            Self::AfterFenceCommit => Stage::Fenced,
            Self::AfterStop => Stage::Stopped,
            Self::AfterSettleCommit => Stage::Settled,
            Self::AfterAnswerCommit | Self::LaunchedControl => Stage::Answered,
        }
    }
}

/// How far the cancellation got before the worker died. A claimed slot is
/// untouched or withdrawn. A running attempt's cancellation got nothing
/// committed; the delegation; the run cancellation's reservation; its stop
/// fence; the stop itself; its settlement; its answer.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Stage {
    Claimed,
    Withdrawn,
    Untouched,
    Delegated,
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

/// The store's one occurrence, with its run, that run's one attempt, the
/// attempt's session and the run's stop fence when it has launched, read
/// together.
#[derive(Debug, Deserialize, Serialize, PartialEq, Clone, Copy)]
#[serde(rename_all = "snake_case")]
enum Run {
    /// The occurrence `claimed` under the daemon's lease, with no run.
    Claimed,
    /// The occurrence `cancelled` with no lease, and no run.
    Withdrawn,
    /// Launched: the occurrence `running` under the daemon's lease, the run
    /// `running` on the attempt's `running` session, the attempt `started`,
    /// and no stop fence.
    Running,
    /// The same, with the delegated run cancellation holding the run's stop
    /// fence on that session.
    Fenced,
    /// Behind that fence, the restart's containment has marked the session
    /// `killed`; the attempt is still `started` and the run and occurrence
    /// `running`.
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
    /// fence.
    Failed,
    /// Anything else, including a partial transaction.
    Mixed,
}

/// The owner's flat cancellation as the store records it: the answer stored
/// under the owner's adoption key and, once delegated, the run cancellation
/// under the derived key with its reservation and stored answer.
#[derive(Debug, Deserialize, Serialize, PartialEq, Clone, Copy)]
#[serde(rename_all = "snake_case")]
enum Cancellation {
    /// No answer, run cancellation or reservation.
    None,
    /// A committed `cancelled` answer that withdrew the slot itself, and no
    /// run cancellation.
    Withdrawn,
    /// A stored `ILLEGAL_TRANSITION` rejection, and no run cancellation.
    Refused,
    /// A committed answer delegating to a run cancellation under the derived
    /// key, which has not been sent.
    Delegated,
    /// The delegation, with the run cancellation `requested` under its
    /// reservation and no answer of its own.
    Requested,
    /// The delegation, with the run cancellation `stopping` under its
    /// reservation and no answer of its own.
    Stopping,
    /// The delegation, with the run cancellation's own stored
    /// `ILLEGAL_TRANSITION` rejection and no cancellation row.
    DelegationRefused,
    /// The delegation, with the run cancellation `rejected` and its stored
    /// `ILLEGAL_TRANSITION` rejection.
    Rejected,
    /// The delegation, with the run cancellation `recovery_required` and its
    /// stored committed answer of that status.
    RecoveryRequired,
    /// The delegation, with the run cancellation `cancelled` and its stored
    /// committed answer of that status.
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
    /// `ILLEGAL_TRANSITION`, which this request stored, under the owner's key
    /// or the derived one.
    Refused,
    /// `ILLEGAL_TRANSITION` replayed from the store, with no write.
    Rejected,
    /// The stored withdrawal of the slot, replayed with no write.
    Withdrawn,
    /// A run cancellation this request delegated, sent and settled
    /// `cancelled`.
    Stopped,
    /// The delegated run cancellation's stored `recovery_required` answer,
    /// replayed with no write.
    RecoveryRequired,
    /// The delegated run cancellation's stored `cancelled` answer,
    /// acknowledged and reconciled when the stop was settled, replayed with no
    /// write.
    Cancelled,
    /// Anything else: another code, a write with a replay, or a response that
    /// does not name the store's occurrence or run.
    Mismatched,
}

/// What each boundary must produce. A vector restates it, so it cannot weaken
/// it.
fn expected_for(boundary: Boundary) -> Expected {
    let stage = boundary.stage();
    let (run_after_exit, cancellation_after_exit) = match stage {
        Stage::Claimed => (Run::Claimed, Cancellation::None),
        Stage::Withdrawn => (Run::Withdrawn, Cancellation::Withdrawn),
        Stage::Untouched => (Run::Running, Cancellation::None),
        Stage::Delegated => (Run::Running, Cancellation::Delegated),
        Stage::Reserved => (Run::Running, Cancellation::Requested),
        Stage::Fenced | Stage::Stopped => (Run::Fenced, Cancellation::Stopping),
        Stage::Settled => (Run::Cancelled, Cancellation::Stopping),
        Stage::Answered => (Run::Cancelled, Cancellation::Cancelled),
    };
    // The restart launches a claimed slot nobody withdrew, and never a
    // withdrawn one. Its containment marks a session it finds running
    // `killed`. Behind the run cancellation's fence, the startup pass leaves
    // the attempt to that cancellation's reconciliation; without one, it fails
    // the attempt and holds the run.
    let run_after_restart = match stage {
        Stage::Claimed => Run::Running,
        Stage::Withdrawn => Run::Withdrawn,
        Stage::Untouched | Stage::Delegated | Stage::Reserved => Run::Held,
        Stage::Fenced | Stage::Stopped => Run::Contained,
        Stage::Settled | Stage::Answered => Run::Cancelled,
    };
    // A request that committed nothing is new: it delegates the launched
    // slot's stop, or finds the held attempt no longer cancellable. A stored
    // delegation re-sends its run cancellation, which a failed attempt refuses
    // and which, while reserved inside its lease, is the daemon's to
    // reconcile.
    let reissue = match stage {
        Stage::Claimed => Answer::Stopped,
        Stage::Withdrawn => Answer::Withdrawn,
        Stage::Untouched | Stage::Delegated => Answer::Refused,
        Stage::Reserved | Stage::Fenced | Stage::Stopped | Stage::Settled => Answer::Pending,
        Stage::Answered => Answer::Cancelled,
    };
    // The pass after the lease reconciles the run cancellation, as it does
    // one the owner sent directly.
    let (run_after_pass, cancellation_after_pass, resend) = match stage {
        Stage::Claimed => (Run::Cancelled, Cancellation::Cancelled, Answer::Cancelled),
        Stage::Withdrawn => (Run::Withdrawn, Cancellation::Withdrawn, Answer::Withdrawn),
        Stage::Untouched => (Run::Held, Cancellation::Refused, Answer::Rejected),
        Stage::Delegated => (Run::Held, Cancellation::DelegationRefused, Answer::Rejected),
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
        initial_outcome: if boundary.control() {
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
        // A withdrawn slot or a settled run gives the restart nothing to
        // launch, contain or settle.
        restart_is_noop: matches!(stage, Stage::Withdrawn | Stage::Settled | Stage::Answered),
        reissue,
        run_after_pass,
        cancellation_after_pass,
        resend,
        // Only a slot nobody withdrew launches, and the request re-sent after
        // the restart stops it.
        later_stops: u64::from(stage == Stage::Claimed),
        launches: u64::from(stage == Stage::Claimed),
        integrity_ok: true,
        replay_is_noop: true,
    }
}

/// A routine with no automatic retry. The runner launches only a routine with
/// a working directory; a no-op runtime never touches it.
fn synthetic_definition() -> Value {
    json!({"schemaVersion":1,"id":"conformance-flat-cancel-crash",
        "name":"Conformance flat cancel crash","status":"ACTIVE",
        "rrule":"FREQ=DAILY;BYHOUR=9","timezone":"utc","misfire":"latest",
        "overlap":"forbid","timeoutMinutes":30,"runtime":"coven-code",
        "prompt":"Run the native flat cancellation crash conformance probe.",
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

/// The run cancellation's adoption key, derived from the owner's.
fn derived_key() -> String {
    format!("rc:{}", &sha256_hex(ADOPTION_KEY.as_bytes())[..40])
}

/// Where the worker parks inside SQLite. A claimed slot is withdrawn, and its
/// answer stored, in one transaction. For a running attempt, the command
/// stores its delegation in one transaction; the run cancellation then
/// reserves, fences, settles and answers in four. Nothing else commits after
/// the pause is installed. A pause right after the stop is the runtime's, not
/// SQLite's.
fn pause_at(boundary: Boundary) -> Option<Pause<'static>> {
    match boundary {
        Boundary::AfterOccurrenceCancel => Some(Pause::After(
            "AFTER UPDATE OF state ON main.automation_occurrences
             WHEN NEW.state = 'cancelled'",
        )),
        Boundary::AfterWithdrawalAdoption => Some(Pause::After(
            "AFTER INSERT ON main.automation_command_adoptions",
        )),
        Boundary::AfterDelegationAdoption => Some(Pause::After(
            "AFTER INSERT ON main.automation_command_adoptions
             WHEN NEW.command = 'attempt.cancel.v1'",
        )),
        Boundary::AfterWithdrawalCommit | Boundary::AfterDelegationCommit => {
            Some(Pause::AfterCommits(1))
        }
        Boundary::AfterReservationCommit => Some(Pause::AfterCommits(2)),
        Boundary::AfterFenceCommit => Some(Pause::AfterCommits(3)),
        Boundary::AfterSettleCommit => Some(Pause::AfterCommits(4)),
        Boundary::AfterAnswerCommit => Some(Pause::AfterCommits(5)),
        Boundary::AfterStop | Boundary::ClaimedControl | Boundary::LaunchedControl => None,
    }
}

/// The owner's request, identical each time it is sent: the store's one
/// occurrence for a claimed slot, or its run's first attempt.
struct Request {
    scene: Scene,
    target_id: String,
}

impl Request {
    fn read(conn: &Connection, scene: Scene) -> Result<Self, &'static str> {
        let sql = match scene {
            Scene::Claimed => "SELECT id FROM automation_occurrences",
            Scene::Launched => "SELECT id FROM automation_attempts WHERE attempt_number = 1",
        };
        let target_id = conn
            .query_row(sql, [], |row| row.get(0))
            .map_err(|_| FAILED)?;
        Ok(Self { scene, target_id })
    }

    fn body(&self) -> Value {
        match self.scene {
            Scene::Claimed => json!({"action": "coven.automations.occurrence.cancel.v1",
                "adoptionKey": ADOPTION_KEY, "occurrenceId": self.target_id,
                "reason": REASON}),
            Scene::Launched => json!({"action": "coven.automations.attempt.cancel.v1",
                "adoptionKey": ADOPTION_KEY, "attemptId": self.target_id,
                "reason": REASON}),
        }
    }

    /// Sends the request at `now` as the daemon's API does. The API fences any
    /// delegated stop on the wall clock; this fences it at `now`, so its times
    /// agree with the scheduler's.
    fn send(
        &self,
        conn: &Connection,
        runtime: &dyn crate::api::SessionRuntime,
        now: DateTime<Utc>,
    ) -> Result<CancelExecution, String> {
        match self.scene {
            Scene::Claimed => {
                execute_occurrence_cancel_at_stop_fence(conn, runtime, self.body(), now, now)
            }
            Scene::Launched => {
                execute_attempt_cancel_at_stop_fence(conn, runtime, self.body(), now, now)
            }
        }
    }

    /// The answer that withdrew the claimed slot at the worker's request time.
    fn withdrawn(&self, base: DateTime<Utc>) -> Value {
        json!({"occurrenceId": self.target_id, "status": "cancelled", "reason": REASON,
            "cancelledAt": iso(at(base, CANCEL))})
    }

    /// The delegated run cancellation's answer with `status`, naming the
    /// store's run, or none while the store has no run. The worker delegates a
    /// running attempt's stop; for a claimed slot, the first request re-sent
    /// after the restart delegates the stop of the run the restart launched. A
    /// confirmed stop is settled at the time it was requested.
    fn delegated(
        &self,
        conn: &Connection,
        status: &str,
        base: DateTime<Utc>,
    ) -> Result<Option<Value>, &'static str> {
        let Some(StoreRun {
            run_id,
            attempt_id,
            session_id,
            principal,
        }) = StoreRun::read(conn)?
        else {
            return Ok(None);
        };
        let requested_at = iso(at(
            base,
            match self.scene {
                Scene::Claimed => REISSUE,
                Scene::Launched => CANCEL,
            },
        ));
        let mut cancellation = json!({"scope": "run", "reason": REASON,
            "requestedBy": {"principalId": principal}, "requestedAt": requested_at});
        if status == "cancelled" {
            cancellation["acknowledgedAt"] = json!(requested_at);
            cancellation["reconciledAt"] = json!(requested_at);
        }
        let mut payload = json!({"runId": run_id, "attemptId": attempt_id,
            "runtimeCorrelation": {"sessionId": session_id},
            "status": status, "cancellation": cancellation});
        if self.scene == Scene::Claimed {
            payload["occurrenceId"] = json!(self.target_id);
        }
        Ok(Some(payload))
    }
}

/// The store's one run with its first attempt, that attempt's session and the
/// owner principal.
struct StoreRun {
    run_id: String,
    attempt_id: String,
    session_id: String,
    principal: String,
}

impl StoreRun {
    fn read(conn: &Connection) -> Result<Option<Self>, &'static str> {
        conn.query_row(
            "SELECT r.id, a.id, a.session_id,
                    (SELECT principal_id FROM automation_owner_principal WHERE singleton = 1)
             FROM automation_runs AS r
             JOIN automation_attempts AS a ON a.run_id = r.id AND a.attempt_number = 1",
            [],
            |row| {
                Ok(Self {
                    run_id: row.get(0)?,
                    attempt_id: row.get(1)?,
                    session_id: row.get(2)?,
                    principal: row.get(3)?,
                })
            },
        )
        .optional()
        .map_err(|_| FAILED)
    }

    /// The run cancellation the owner's command delegates this run's stop to.
    fn delegation(&self) -> Value {
        json!({"action": "coven.automations.run.cancel.v1", "adoptionKey": derived_key(),
            "runId": self.run_id, "attemptId": self.attempt_id,
            "runtimeCorrelation": {"sessionId": self.session_id}, "scope": "run",
            "reason": REASON, "requestedBy": {"principalId": self.principal}})
    }
}

/// Claims the routine's 09:00 slot under `home`'s scheduler leadership and,
/// for a running attempt, launches it through a no-op runtime. The owner
/// principal already exists, as for a daemon that has answered its owner
/// before. Returns the leadership.
fn prepared(
    home: &Path,
    base: DateTime<Utc>,
    scene: Scene,
) -> Result<SchedulerLeadership, &'static str> {
    let conn = &crate::store::open_store(&crate::api::store_path(home)).map_err(|_| FAILED)?;
    let claim_at = at(base, CLAIM);
    let definition = RoutineDefinition::from_json(&synthetic_definition()).map_err(|_| INVALID)?;
    crate::automations::store::insert_definition(conn, &definition).map_err(|_| FAILED)?;
    conn.execute(
        "UPDATE automation_definitions SET created_at = ?1, updated_at = ?1 WHERE id = ?2",
        [DEFINED_AT, definition.id.as_str()],
    )
    .map_err(|_| FAILED)?;
    crate::automations::owner_grants::owner_principal_id(conn, &iso(claim_at))
        .map_err(|_| FAILED)?;
    // The daemon's scheduler claims and dispatches under its leadership fence.
    let leadership = SchedulerLeadership::acquire(home, conn, claim_at).map_err(|_| FAILED)?;
    let fence = leadership.fence();
    let tick = crate::automations::occurrences::tick_with_scheduler_fence(conn, claim_at, &fence)
        .map_err(|_| FAILED)?;
    if tick.planned.len() != 1 || tick.claimed.len() != 1 || !tick.failed.is_empty() {
        return Err(FAILED);
    }
    if scene == Scene::Launched {
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
        if report.dispatched.len() != 1 || !report.failed.is_empty() {
            return Err(FAILED);
        }
    }
    let ready = match scene {
        Scene::Claimed => Run::Claimed,
        Scene::Launched => Run::Running,
    };
    if run(conn)? != ready || cancellation(conn, scene)? != Cancellation::None {
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
    let scene = case.boundary.scene();
    let leadership = prepared(&home, base, scene)?;
    let conn = crate::store::open_store(&crate::api::store_path(&home)).map_err(|_| FAILED)?;
    let request = Request::read(&conn, scene)?;
    let handshake =
        json!({"state":"paused", "boundary":case.boundary, "relativeHome":relative_home});
    if let Some(pause_at) = pause_at(case.boundary) {
        install_pause(&conn, pause_at, handshake.clone())?;
    }
    let runtime = RecordingStops {
        log: home.join(STOP_LOG),
        park: (case.boundary == Boundary::AfterStop).then_some(handshake),
    };
    let response = request
        .send(&conn, &runtime, at(base, CANCEL))
        .map_err(|_| FAILED)?;
    let answer = match scene {
        Scene::Claimed => request.withdrawn(base),
        Scene::Launched => request.delegated(&conn, "cancelled", base)?.ok_or(FAILED)?,
    };
    let answered = matches!(&response, CancelExecution::Success { payload, replayed: false, .. }
        if *payload == answer);
    if !case.boundary.control() || !answered {
        return Err(FAILED);
    }
    // Exits like a daemon that dies after answering: without the graceful
    // shutdown's reconciliation or the release of its scheduler authority.
    drop(conn);
    drop(leadership);
    announce(&json!({"state":"responded", "boundary":case.boundary, "relativeHome":relative_home}))
}

/// Classifies the store's one occurrence with its run, attempt, session and
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
    let lease = "CASE
            WHEN o.lease_owner = 'daemon' AND o.lease_expires_at IS NOT NULL THEN 'daemon'
            WHEN o.lease_owner IS NULL AND o.lease_expires_at IS NULL THEN 'none'
            ELSE 'other'
        END";
    match (occurrences, runs, attempts, sessions) {
        (1, 0, 0, 0) => {
            let (occurrence, lease, fences): (String, String, i64) = conn
                .query_row(
                    &format!(
                        "SELECT o.state, {lease},
                                (SELECT COUNT(*) FROM automation_stop_fences)
                         FROM automation_occurrences AS o"
                    ),
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .map_err(|_| FAILED)?;
            return Ok(match (occurrence.as_str(), lease.as_str(), fences) {
                ("claimed", "daemon", 0) => Run::Claimed,
                ("cancelled", "none", 0) => Run::Withdrawn,
                _ => Run::Mixed,
            });
        }
        (1, 1, 1, 1) => {}
        _ => return Ok(Run::Mixed),
    }
    type Row = (String, String, String, bool, String, bool, String, String);
    let row: Option<Row> = conn
        .query_row(
            &format!(
                "SELECT o.state, {lease},
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
                 JOIN sessions AS s ON s.id = a.session_id"
            ),
            [derived_key()],
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

/// The answer stored under the owner's key: none, a withdrawal, a delegation
/// to the derived key, or an `ILLEGAL_TRANSITION` rejection.
#[derive(Clone, Copy, PartialEq)]
enum Stored {
    None,
    Withdrawn,
    Delegated,
    Refused,
    Other,
}

/// An adoption's outcome with its error code or result status.
fn detail(outcome: &str, response: &Value) -> String {
    match outcome {
        "rejected" => response["error"]["code"].as_str(),
        "committed" => response["result"]["status"].as_str(),
        _ => None,
    }
    .unwrap_or_default()
    .to_owned()
}

/// Classifies the owner's flat cancellation and the run cancellation it
/// delegated, with that cancellation's reservation and stored answer.
fn cancellation(conn: &Connection, scene: Scene) -> Result<Cancellation, &'static str> {
    type Row = (
        i64,
        i64,
        i64,
        Option<String>,
        bool,
        Option<(String, String)>,
        Option<(String, String)>,
    );
    let derived = derived_key();
    let (cancellations, reservations, adoptions, state, reserved, outer, inner): Row = conn
        .query_row(
            "SELECT
                (SELECT COUNT(*) FROM automation_cancellations),
                (SELECT COUNT(*) FROM automation_command_reservations),
                (SELECT COUNT(*) FROM automation_command_adoptions),
                (SELECT state FROM automation_cancellations WHERE adoption_key = ?2),
                EXISTS (
                    SELECT 1 FROM automation_command_reservations
                    WHERE adoption_key = ?2 AND command = 'run.cancel.v1'
                ),
                (SELECT outcome FROM automation_command_adoptions
                 WHERE adoption_key = ?1 AND command = ?3),
                (SELECT response_json FROM automation_command_adoptions
                 WHERE adoption_key = ?1 AND command = ?3),
                (SELECT outcome FROM automation_command_adoptions
                 WHERE adoption_key = ?2 AND command = 'run.cancel.v1'),
                (SELECT response_json FROM automation_command_adoptions
                 WHERE adoption_key = ?2 AND command = 'run.cancel.v1')",
            [ADOPTION_KEY, derived.as_str(), scene.command()],
            |row| {
                let outer: Option<String> = row.get(5)?;
                let outer_response: Option<String> = row.get(6)?;
                let inner: Option<String> = row.get(7)?;
                let inner_response: Option<String> = row.get(8)?;
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    outer.zip(outer_response),
                    inner.zip(inner_response),
                ))
            },
        )
        .map_err(|_| FAILED)?;
    let parse = |response: &str| serde_json::from_str::<Value>(response).map_err(|_| FAILED);
    // A stored answer must be exactly the one the command stores for the
    // store's occurrence or run.
    let withdrawal = Request::read(conn, Scene::Claimed)?.withdrawn(base()?);
    let delegation = StoreRun::read(conn)?.map(|run| run.delegation());
    let stored = match &outer {
        None => Stored::None,
        Some((outcome, response)) => {
            let response = parse(response)?;
            match (outcome.as_str(), detail(outcome, &response).as_str()) {
                ("committed", _)
                    if delegation.as_ref().is_some_and(|delegation| {
                        response
                            == json!({"outcome": "committed",
                            "result": {"runCancellation": delegation}})
                    }) =>
                {
                    Stored::Delegated
                }
                ("committed", "cancelled")
                    if response == json!({"outcome": "committed", "result": withdrawal}) =>
                {
                    Stored::Withdrawn
                }
                ("rejected", "ILLEGAL_TRANSITION") => Stored::Refused,
                _ => Stored::Other,
            }
        }
    };
    let inner = inner
        .map(|(outcome, response)| {
            Ok::<_, &'static str>((detail(&outcome, &parse(&response)?), outcome))
        })
        .transpose()?;
    let inner = inner
        .as_ref()
        .map(|(detail, outcome)| (outcome.as_str(), detail.as_str()));
    Ok(
        match (
            cancellations,
            reservations,
            adoptions,
            state.as_deref(),
            reserved,
            stored,
            inner,
        ) {
            (0, 0, 0, None, false, Stored::None, None) => Cancellation::None,
            (0, 0, 1, None, false, Stored::Withdrawn, None) => Cancellation::Withdrawn,
            (0, 0, 1, None, false, Stored::Refused, None) => Cancellation::Refused,
            (0, 0, 1, None, false, Stored::Delegated, None) => Cancellation::Delegated,
            (1, 1, 1, Some("requested"), true, Stored::Delegated, None) => Cancellation::Requested,
            (1, 1, 1, Some("stopping"), true, Stored::Delegated, None) => Cancellation::Stopping,
            (0, 0, 2, None, false, Stored::Delegated, Some(("rejected", "ILLEGAL_TRANSITION"))) => {
                Cancellation::DelegationRefused
            }
            (
                1,
                0,
                2,
                Some("rejected"),
                false,
                Stored::Delegated,
                Some(("rejected", "ILLEGAL_TRANSITION")),
            ) => Cancellation::Rejected,
            (
                1,
                0,
                2,
                Some("recovery_required"),
                false,
                Stored::Delegated,
                Some(("committed", "recovery_required")),
            ) => Cancellation::RecoveryRequired,
            (
                1,
                0,
                2,
                Some("cancelled"),
                false,
                Stored::Delegated,
                Some(("committed", "cancelled")),
            ) => Cancellation::Cancelled,
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
    request: &Request,
    now: DateTime<Utc>,
) -> Result<Answer, &'static str> {
    let before = snapshot(conn)?;
    let execution = request.send(conn, runtime, now).map_err(|_| REFUSED)?;
    let wrote = snapshot(conn)? != before;
    let base = base()?;
    let refused = matches!(
        cancellation(conn, request.scene)?,
        Cancellation::Refused | Cancellation::DelegationRefused
    );
    let withdrawn = Some(request.withdrawn(base));
    let cancelled = request.delegated(conn, "cancelled", base)?;
    let recovery_required = request.delegated(conn, "recovery_required", base)?;
    Ok(match execution {
        CancelExecution::Rejected(error) => match (error.code(), wrote) {
            (ErrorCode::CancelPending, false) => Answer::Pending,
            (ErrorCode::IllegalTransition, true) if refused => Answer::Refused,
            (ErrorCode::IllegalTransition, false) => Answer::Rejected,
            _ => Answer::Mismatched,
        },
        CancelExecution::Success {
            payload,
            replayed,
            terminal,
        } => {
            // The answer's terminal adoption: the owner's own for a
            // withdrawal, the derived run cancellation's for a delegation,
            // replayed exactly when the answer is.
            let payload = Some(payload);
            let replays = terminal.replayed == replayed;
            let own = replays
                && terminal.key == ADOPTION_KEY
                && terminal.command == "occurrence.cancel.v1";
            let delegated =
                replays && terminal.key == derived_key() && terminal.command == "run.cancel.v1";
            match (wrote, replayed) {
                (true, false) if delegated && payload == cancelled => Answer::Stopped,
                (false, true) if own && payload == withdrawn => Answer::Withdrawn,
                (false, true) if delegated && payload == recovery_required => {
                    Answer::RecoveryRequired
                }
                (false, true) if delegated && payload == cancelled => Answer::Cancelled,
                _ => Answer::Mismatched,
            }
        }
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

fn recover(
    home: &Path,
    path: &Path,
    base: DateTime<Utc>,
    scene: Scene,
) -> Result<Restarted, &'static str> {
    // One runtime confirms every stop and counts it, with every launch, for
    // the restart, each pass and each re-sent request.
    let runtime = CountingRuntime::default();
    let (request, before_restart) = {
        let conn = crate::store::open_store(path).map_err(|_| FAILED)?;
        (Request::read(&conn, scene)?, snapshot(&conn)?)
    };
    let leadership = restart(home, &runtime, at(base, RESTART))?;
    let conn = crate::store::open_store(path).map_err(|_| FAILED)?;
    let run_after_restart = run(&conn)?;
    let cancellation_after_restart = cancellation(&conn, scene)?;
    let restart_is_noop = snapshot(&conn)? == before_restart;
    // The owner never saw an answer, so they send the same request again.
    let reissued = reissue(&conn, &runtime, &request, at(base, REISSUE))?;
    drop(conn);
    pass(home, &runtime, &leadership, at(base, PASS), "deadline")?;
    let conn = crate::store::open_store(path).map_err(|_| FAILED)?;
    let run_after_pass = run(&conn)?;
    let cancellation_after_pass = cancellation(&conn, scene)?;
    let resent = reissue(&conn, &runtime, &request, at(base, RESEND))?;
    let later_stops = runtime.stops();
    let launches = runtime.launches();
    let before_replay = snapshot(&conn)?;
    // The same request once more, then a pass on its own connection, must
    // answer the same without a write, a stop or a launch.
    let replayed = reissue(&conn, &runtime, &request, at(base, REPLAY))? == resent;
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
fn observe_pause(path: &Path, scene: Scene) -> Result<(bool, Run, Cancellation), &'static str> {
    let probe = open_raw(path)?;
    let open = write_transaction_open(&probe)?;
    Ok((open, run(&probe)?, cancellation(&probe, scene)?))
}

fn case_matches(case: &Case) -> Result<(bool, Value), &'static str> {
    let root = super::scheduler_conformance_tempdir()?;
    crate::daemon::ensure_private_coven_home(root.path()).map_err(|_| FAILED)?;
    let payload = serde_json::to_vec(case).map_err(|_| INVALID)?;
    let (mut process, receiver) = spawn_worker::<Handshake>(WORKER, payload, root.path())?;
    let ready = receiver.recv_timeout(CHILD_GUARD).map_err(|_| FAILED)??;
    let crash = !case.boundary.control();
    let scene = case.boundary.scene();
    if ready.boundary != case.boundary || ready.state != if crash { "paused" } else { "responded" }
    {
        return Err(FAILED);
    }
    let home = worker_home(root.path(), &ready.relative_home, HOME_PREFIX)?;
    let path = crate::api::store_path(&home);
    let pause = crash.then(|| observe_pause(&path, scene)).transpose()?;
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
    let cancellation_after_exit = cancellation(&conn, scene)?;
    drop(conn);
    let recovery = match recover(&home, &path, base, scene) {
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

    fn vectors() -> Value {
        serde_json::from_str(include_str!(
            "../../../../../conformance/automations/runner/flat-cancellation-process-crash-recovery.vectors.json"
        ))
        .unwrap()
    }

    #[test]
    fn flat_cancellation_crash_vectors_restate_each_boundary_outcome() {
        let vectors: VectorSet = serde_json::from_value(vectors()).unwrap();
        assert_eq!(vectors.schema_version, SCHEMA_VERSION);
        for case in &vectors.cases {
            validate(case).unwrap();
        }
        let boundaries: Vec<_> = vectors.cases.iter().map(|case| case.boundary).collect();
        assert_eq!(boundaries, Boundary::ALL);
    }

    #[test]
    fn flat_cancellation_crash_vectors_require_each_boundary_and_both_controls() {
        let original = vectors();
        let mut missing = original.clone();
        missing["cases"].as_array_mut().unwrap().pop();
        assert_eq!(evaluate(&missing).unwrap_err(), INVALID);
        // A second control under its own case id, replacing a boundary.
        let mut duplicate = original.clone();
        let mut control = duplicate["cases"][11].clone();
        control["caseId"] = json!("second-uninterrupted-flat-cancellation-control");
        duplicate["cases"][4] = control;
        assert_eq!(evaluate(&duplicate).unwrap_err(), INVALID);
        let mut duplicate_id = original;
        duplicate_id["cases"][1]["caseId"] = duplicate_id["cases"][0]["caseId"].clone();
        assert_eq!(evaluate(&duplicate_id).unwrap_err(), INVALID);
    }

    #[test]
    fn flat_cancellation_crash_vectors_refuse_malformed_or_weakened_inputs() {
        let original = vectors();
        for (case, field, value) in [
            (0, "runAfterExit", json!("withdrawn")),
            (0, "runAfterRestart", json!("claimed")),
            (0, "launches", json!(0)),
            (1, "reissue", json!("withdrawn")),
            (1, "laterStops", json!(0)),
            (2, "cancellationAfterExit", json!("none")),
            (2, "restartIsNoop", json!(false)),
            (3, "initialOutcome", json!("terminated")),
            (4, "reissue", json!("pending")),
            (5, "cancellationAfterExit", json!("none")),
            (5, "cancellationAfterPass", json!("refused")),
            (6, "reissue", json!("refused")),
            (7, "workerStops", json!(1)),
            (8, "workerStops", json!(0)),
            (8, "runAfterPass", json!("cancelled")),
            (9, "reissue", json!("cancelled")),
            (10, "laterStops", json!(1)),
            (11, "integrityOk", json!(false)),
            (11, "replayIsNoop", json!(false)),
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
            ("occurrenceId", json!("private-input")),
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
            json!("coven.automations.run-cancellation-process-crash-recovery-vectors.v1");
        assert_eq!(evaluate(&invalid).unwrap_err(), INVALID);
    }

    #[test]
    fn the_delegated_key_is_the_one_the_command_derives() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        crate::daemon::ensure_private_coven_home(&home).unwrap();
        let base = base().unwrap();
        let _leadership = prepared(&home, base, Scene::Launched).unwrap();
        let conn = crate::store::open_store(&crate::api::store_path(&home)).unwrap();
        let request = Request::read(&conn, Scene::Launched).unwrap();
        let answered = request.send(&conn, &CountingRuntime::default(), at(base, CANCEL));
        let expected = request.delegated(&conn, "cancelled", base).unwrap();
        // The answer names the session, so the failure message does not print it.
        assert!(
            matches!(&answered, Ok(CancelExecution::Success { payload, replayed: false, .. })
                if Some(payload) == expected.as_ref()),
            "the attempt cancellation did not answer with its delegated stop"
        );
        assert_eq!(run(&conn).unwrap(), Run::Cancelled);
        assert_eq!(
            cancellation(&conn, Scene::Launched).unwrap(),
            Cancellation::Cancelled
        );
    }

    #[test]
    fn worker_refuses_invalid_or_weakened_input() {
        let mut case = vectors()["cases"][0].clone();
        case["storePath"] = json!("private-input");
        assert_eq!(worker(&case).unwrap_err(), INVALID);
        let mut case = vectors()["cases"][8].clone();
        case["expected"]["workerStops"] = json!(0);
        assert_eq!(worker(&case).unwrap_err(), INVALID);
        let mut case = vectors()["cases"][11].clone();
        case["boundary"] = json!("claimed_control");
        assert_eq!(worker(&case).unwrap_err(), INVALID);
    }
}
