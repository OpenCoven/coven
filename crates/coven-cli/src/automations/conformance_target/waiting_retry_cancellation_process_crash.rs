//! Disposable waiting-retry cancellation crash fixtures: a daemon worker
//! claims one scheduled occurrence whose runtime is unavailable for its first
//! launch, so the policy schedules a retry whose backoff outlasts the run's
//! deadline. The owner then cancels that waiting retry, and the worker is
//! killed while the command settles it with its answer in one transaction.
//! Before the deadline the command cancels the retry, the run and the
//! occurrence; after it, the command times the retry out and refuses. The
//! parent restarts the daemon's scheduler, the owner sends the same request
//! again, and passes after the backoff must launch nothing. Every launch goes
//! through a no-op runtime; no runtime process starts.
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
use crate::automations::cancel_commands::{
    execute_attempt_cancel_at_stop_fence, execute_occurrence_cancel_at_stop_fence, CancelExecution,
};
use crate::automations::contract::error::ErrorCode;
use crate::automations::runner::DispatchReport;

const INVALID: &str = "conformance vector is invalid";
const FAILED: &str = "conformance suite execution failed";
const SCHEMA_VERSION: &str =
    "coven.automations.waiting-retry-cancellation-process-crash-recovery-vectors.v1";
const WORKER: &str = "waiting-retry-cancellation-crash-worker";
const HOME_PREFIX: &str = "retry-cancel-crash-";
/// The one adoption key the owner's cancellation carries, sent and re-sent.
const ADOPTION_KEY: &str = "adopt:conformance:retry-cancel-crash";
const REASON: &str = "not needed today";
/// Seconds after `BASE`: the worker claims the 09:00 slot at 30, and its
/// launch finds the runtime unavailable. The run's one-minute deadline falls
/// at 90, but the policy's retry waits until 150. The owner cancels the
/// waiting retry before the deadline at 60, or after it at 120 before any pass
/// has enforced it. The parent restarts the daemon ten seconds after the
/// request, and the owner sends it again ten seconds later. A pass after the
/// backoff, a further request and a replay must change nothing.
const CLAIM: i64 = 30;
const DEADLINE: i64 = 90;
const BACKOFF_END: i64 = 150;
const PASS: i64 = 160;
const RESEND: i64 = 170;
const REPLAY: i64 = 240;
/// The retry's, the occurrence's and the run's reason once the deadline
/// settles a waiting retry.
const TIMED_OUT: &str = "automation retry wait exceeded run timeout";
/// The command's refusal once it has timed the waiting retry out itself.
const TIMEOUT_REFUSAL: &str = "the targeted automation run reached its timeout before cancellation";
/// The command's refusal of an attempt whose run has already ended.
const ATTEMPT_REFUSAL: &str = "only a running run's current attempt can be cancelled";

/// When the owner cancels the waiting retry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Scene {
    /// `occurrence.cancel.v1` before the run's deadline.
    Waiting,
    /// `attempt.cancel.v1` for the retry after the run's deadline.
    Expired,
}

impl Scene {
    fn command(self) -> &'static str {
        match self {
            Self::Waiting => "occurrence.cancel.v1",
            Self::Expired => "attempt.cancel.v1",
        }
    }

    /// When the owner first sends the request.
    fn cancel_at(self) -> i64 {
        match self {
            Self::Waiting => 60,
            Self::Expired => 120,
        }
    }

    fn restart_at(self) -> i64 {
        self.cancel_at() + 10
    }

    fn reissue_at(self) -> i64 {
        self.cancel_at() + 20
    }
}

/// Where the worker can stop: after each write of the command's one
/// transaction, after its commit, and the uninterrupted control, for each
/// scene.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
enum Boundary {
    AfterRetryCancelled,
    AfterRunCancelled,
    AfterOccurrenceCancelled,
    AfterWithdrawalAdoption,
    AfterWithdrawalCommit,
    WaitingControl,
    AfterRetryTimedOut,
    AfterOccurrenceFailed,
    AfterRunFailed,
    AfterRefusalAdoption,
    AfterRefusalCommit,
    ExpiredControl,
}

impl Boundary {
    const ALL: [Self; 12] = [
        Self::AfterRetryCancelled,
        Self::AfterRunCancelled,
        Self::AfterOccurrenceCancelled,
        Self::AfterWithdrawalAdoption,
        Self::AfterWithdrawalCommit,
        Self::WaitingControl,
        Self::AfterRetryTimedOut,
        Self::AfterOccurrenceFailed,
        Self::AfterRunFailed,
        Self::AfterRefusalAdoption,
        Self::AfterRefusalCommit,
        Self::ExpiredControl,
    ];

    fn scene(self) -> Scene {
        match self {
            Self::AfterRetryCancelled
            | Self::AfterRunCancelled
            | Self::AfterOccurrenceCancelled
            | Self::AfterWithdrawalAdoption
            | Self::AfterWithdrawalCommit
            | Self::WaitingControl => Scene::Waiting,
            _ => Scene::Expired,
        }
    }

    fn control(self) -> bool {
        matches!(self, Self::WaitingControl | Self::ExpiredControl)
    }

    /// The command's transaction committed before the worker was gone.
    fn committed(self) -> bool {
        matches!(
            self,
            Self::AfterWithdrawalCommit
                | Self::WaitingControl
                | Self::AfterRefusalCommit
                | Self::ExpiredControl
        )
    }

    /// The worker stops inside the command's transaction.
    fn inside_transaction(self) -> bool {
        !self.committed()
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
    stored_after_exit: Stored,
    retry_after_restart: Retry,
    stored_after_restart: Stored,
    restart_is_noop: bool,
    reissue: Answer,
    retry_after_reissue: Retry,
    stored_after_reissue: Stored,
    pass_is_noop: bool,
    resend: Answer,
    launches: u64,
    stops: u64,
    integrity_ok: bool,
    replay_is_noop: bool,
}

#[derive(Deserialize, Serialize, PartialEq, Clone, Copy)]
#[serde(rename_all = "snake_case")]
enum InitialOutcome {
    Terminated,
    Answered,
}

/// The store's one occurrence with its run, attempts and session, read
/// together. Every class has one occurrence, claimed only for the slot, one
/// run, two attempts, one session and no stop fence. The first attempt settled
/// `failed` for the unavailable runtime with no session bound, and its session
/// `failed`. The retry is the policy's, after the first attempt, bound to the
/// occurrence's claim count, and never bound to a session. The run is on no
/// session, with its deadline before the retry's backoff ends.
#[derive(Debug, Deserialize, Serialize, PartialEq, Clone, Copy)]
#[serde(rename_all = "snake_case")]
enum Retry {
    /// The retry `adopted` until its backoff ends, the run `running` and
    /// unfinished, and the occurrence `planned` with no lease and the
    /// runtime's answer as its reason.
    Scheduled,
    /// The owner cancelled the waiting retry: the retry `cancelled` with the
    /// `cancelled` class, the run `cancelled` and finished, and the occurrence
    /// `cancelled` with no lease, each with the owner's reason where it has
    /// one.
    Cancelled,
    /// The deadline settled the waiting retry: the retry `timed_out` with the
    /// `timeout` class, the occurrence `failed` with no lease, and the run
    /// `failed` and finished, each with the timeout's reason where it has one.
    TimedOut,
    /// Anything else, including a partial transaction.
    Mixed,
}

/// The owner's stored answer: the store's only command adoption, under the
/// owner's key for the scene's command.
#[derive(Debug, Deserialize, Serialize, PartialEq, Clone, Copy)]
#[serde(rename_all = "snake_case")]
enum Stored {
    /// No answer is stored.
    None,
    /// The waiting retry withdrawn with the owner's reason.
    Withdrawn,
    /// Refused because the command found the run past its deadline.
    TimeoutRefusal,
    /// Refused because the attempt's run had already ended.
    AttemptRefusal,
    /// Anything else.
    Other,
}

/// What the owner's re-sent request answered, checked against the store.
#[derive(Debug, Deserialize, Serialize, PartialEq, Clone, Copy)]
#[serde(rename_all = "snake_case")]
enum Answer {
    /// A fresh withdrawal at the request's time, stored as it answered.
    Withdrawn,
    /// The stored withdrawal, replayed without a write.
    Replayed,
    /// A fresh refusal, stored as it answered.
    Refused,
    /// The stored refusal, replayed without a write.
    RefusalReplayed,
    /// Anything else.
    Mismatched,
}

/// What each boundary must produce. A vector restates it, so it cannot weaken
/// it.
fn expected_for(boundary: Boundary) -> Expected {
    let committed = boundary.committed();
    let (retry_after_exit, stored_after_exit) = match (boundary.scene(), committed) {
        (_, false) => (Retry::Scheduled, Stored::None),
        (Scene::Waiting, true) => (Retry::Cancelled, Stored::Withdrawn),
        (Scene::Expired, true) => (Retry::TimedOut, Stored::TimeoutRefusal),
    };
    let (retry_after_restart, reissue, retry_after_reissue, stored_after_reissue) =
        match boundary.scene() {
            // The restart comes before the deadline and leaves the retry
            // waiting. The re-sent request withdraws it unless a withdrawal is
            // stored, which it replays.
            Scene::Waiting => (
                retry_after_exit,
                if committed {
                    Answer::Replayed
                } else {
                    Answer::Withdrawn
                },
                Retry::Cancelled,
                Stored::Withdrawn,
            ),
            // The restart comes after the deadline and times a retry still
            // waiting out. The re-sent request replays a stored refusal, or
            // is refused afresh once the run has ended.
            Scene::Expired => (
                Retry::TimedOut,
                if committed {
                    Answer::RefusalReplayed
                } else {
                    Answer::Refused
                },
                Retry::TimedOut,
                if committed {
                    Stored::TimeoutRefusal
                } else {
                    Stored::AttemptRefusal
                },
            ),
        };
    Expected {
        initial_outcome: if boundary.control() {
            InitialOutcome::Answered
        } else {
            InitialOutcome::Terminated
        },
        retry_after_exit,
        stored_after_exit,
        retry_after_restart,
        stored_after_restart: stored_after_exit,
        restart_is_noop: retry_after_restart == retry_after_exit,
        reissue,
        retry_after_reissue,
        stored_after_reissue,
        pass_is_noop: true,
        resend: match stored_after_reissue {
            Stored::Withdrawn => Answer::Replayed,
            _ => Answer::RefusalReplayed,
        },
        launches: 0,
        stops: 0,
        integrity_ok: true,
        replay_is_noop: true,
    }
}

/// A routine whose policy retries an unavailable runtime once, two minutes
/// after the failure, within a one-minute deadline. The runner launches only
/// a routine with a working directory; a no-op runtime never touches it.
fn synthetic_definition() -> Value {
    json!({"schemaVersion":1,"id":"conformance-retry-cancel-crash",
        "name":"Conformance waiting retry cancellation crash","status":"ACTIVE",
        "rrule":"FREQ=DAILY;BYHOUR=9","timezone":"utc","misfire":"latest",
        "overlap":"forbid","timeoutMinutes":1,"runtime":"coven-code",
        "prompt":"Run the native waiting retry cancellation crash conformance probe.",
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
/// the rejection that schedules the retry second. Before the deadline, the
/// command then cancels the retry first, finishes the run next, cancels the
/// occurrence and stores its answer last. After it, the command times the
/// retry out first, fails the occurrence next, fails the run and stores its
/// refusal last. No other transaction after the pause is installed writes, so
/// the WAL hook counts only these three commits.
fn pause_at(boundary: Boundary) -> Option<Pause<'static>> {
    match boundary {
        Boundary::AfterRetryCancelled => Some(Pause::After(
            "AFTER UPDATE OF state ON main.automation_attempts
             WHEN NEW.state = 'cancelled'",
        )),
        Boundary::AfterRunCancelled => Some(Pause::After(
            "AFTER UPDATE OF status ON main.automation_runs
             WHEN NEW.status = 'cancelled'",
        )),
        Boundary::AfterOccurrenceCancelled => Some(Pause::After(
            "AFTER UPDATE OF state ON main.automation_occurrences
             WHEN NEW.state = 'cancelled'",
        )),
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
        Boundary::AfterWithdrawalAdoption | Boundary::AfterRefusalAdoption => Some(Pause::After(
            "AFTER INSERT ON main.automation_command_adoptions",
        )),
        Boundary::AfterWithdrawalCommit | Boundary::AfterRefusalCommit => {
            Some(Pause::AfterCommits(3))
        }
        Boundary::WaitingControl | Boundary::ExpiredControl => None,
    }
}

/// The owner's request, identical each time it is sent: the store's one
/// occurrence before the deadline, or its run's waiting retry after it.
struct Request {
    scene: Scene,
    target_id: String,
}

impl Request {
    fn read(conn: &Connection, scene: Scene) -> Result<Self, &'static str> {
        let sql = match scene {
            Scene::Waiting => "SELECT id FROM automation_occurrences",
            Scene::Expired => "SELECT id FROM automation_attempts WHERE attempt_number = 2",
        };
        let target_id = conn
            .query_row(sql, [], |row| row.get(0))
            .map_err(|_| FAILED)?;
        Ok(Self { scene, target_id })
    }

    fn body(&self) -> Value {
        match self.scene {
            Scene::Waiting => json!({"action": "coven.automations.occurrence.cancel.v1",
                "adoptionKey": ADOPTION_KEY, "occurrenceId": self.target_id,
                "reason": REASON}),
            Scene::Expired => json!({"action": "coven.automations.attempt.cancel.v1",
                "adoptionKey": ADOPTION_KEY, "attemptId": self.target_id,
                "reason": REASON}),
        }
    }

    /// Sends the request at `now` as the daemon's API does, with any stop
    /// fenced at `now` so its times agree with the scheduler's.
    fn send(
        &self,
        conn: &Connection,
        runtime: &dyn crate::api::SessionRuntime,
        now: DateTime<Utc>,
    ) -> Result<CancelExecution, String> {
        match self.scene {
            Scene::Waiting => {
                execute_occurrence_cancel_at_stop_fence(conn, runtime, self.body(), now, now)
            }
            Scene::Expired => {
                execute_attempt_cancel_at_stop_fence(conn, runtime, self.body(), now, now)
            }
        }
    }
}

/// The answer that withdrew the waiting retry at `cancelled_at`, naming the
/// store's occurrence, run and retry.
fn withdrawn(conn: &Connection, cancelled_at: DateTime<Utc>) -> Result<Value, &'static str> {
    let (occurrence_id, run_id, attempt_id): (String, String, String) = conn
        .query_row(
            "SELECT r.occurrence_id, r.id, a.id
             FROM automation_runs AS r
             JOIN automation_attempts AS a ON a.run_id = r.id
             WHERE a.attempt_number = 2",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(|_| FAILED)?;
    Ok(
        json!({"occurrenceId": occurrence_id, "status": "cancelled", "reason": REASON,
        "cancelledAt": iso(cancelled_at), "runId": run_id, "attemptId": attempt_id}),
    )
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
    if scheduled != DispatchReport::default() {
        return Err(FAILED);
    }
    // The owner cancels the waiting retry.
    let cancel_at = at(base, scene.cancel_at());
    let response = Request::read(&conn, scene)?
        .send(&conn, &runtime, cancel_at)
        .map_err(|_| FAILED)?;
    let answered = match (scene, &response) {
        (
            Scene::Waiting,
            CancelExecution::Success {
                payload,
                replayed: false,
                ..
            },
        ) => *payload == withdrawn(&conn, cancel_at)?,
        (Scene::Expired, CancelExecution::Rejected(error)) => {
            error.code() == ErrorCode::IllegalTransition
                && error.message.as_str() == TIMEOUT_REFUSAL
        }
        _ => false,
    };
    if !case.boundary.control() || !answered || runtime.launches() != 1 {
        return Err(FAILED);
    }
    // Exits like a daemon that dies after answering: without the graceful
    // shutdown's reconciliation or the release of its scheduler authority.
    drop(conn);
    drop(leadership);
    announce(&json!({"state":"answered", "boundary":case.boundary, "relativeHome":relative_home}))
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
                "cancelled",
                Some("cancelled"),
                Some(REASON),
                true,
                "cancelled",
                Some(REASON),
                "cancelled",
                true,
            ) => Retry::Cancelled,
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

/// The store's only command adoption, if it is the owner's for the scene's
/// command: its stored response.
fn stored_response(conn: &Connection, scene: Scene) -> Result<Option<Value>, &'static str> {
    let adoptions: Vec<(String, String, String, String)> = conn
        .prepare(
            "SELECT adoption_key, command, outcome, response_json
             FROM automation_command_adoptions",
        )
        .and_then(|mut statement| {
            statement
                .query_map([], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
                })?
                .collect()
        })
        .map_err(|_| FAILED)?;
    let [(key, command, outcome, response)] = adoptions.as_slice() else {
        return Ok(None);
    };
    let response: Value = serde_json::from_str(response).map_err(|_| FAILED)?;
    Ok((key == ADOPTION_KEY
        && command == scene.command()
        && response["outcome"].as_str() == Some(outcome.as_str()))
    .then_some(response))
}

/// Classifies the owner's stored answer.
fn stored(conn: &Connection, scene: Scene) -> Result<Stored, &'static str> {
    let adoptions: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM automation_command_adoptions",
            [],
            |row| row.get(0),
        )
        .map_err(|_| FAILED)?;
    if adoptions == 0 {
        return Ok(Stored::None);
    }
    let Some(response) = stored_response(conn, scene)? else {
        return Ok(Stored::Other);
    };
    let refusal = |message: &str| {
        response["outcome"] == "rejected"
            && response["error"]["code"] == "ILLEGAL_TRANSITION"
            && response["error"]["message"] == message
    };
    Ok(
        if response["outcome"] == "committed"
            && response["result"]["status"] == "cancelled"
            && response["result"]["reason"] == REASON
        {
            Stored::Withdrawn
        } else if refusal(TIMEOUT_REFUSAL) {
            Stored::TimeoutRefusal
        } else if refusal(ATTEMPT_REFUSAL) {
            Stored::AttemptRefusal
        } else {
            Stored::Other
        },
    )
}

/// Every row the dispatch, the command, a restart or a pass can write, for an
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

/// Sends the owner's request again at `now` and classifies the answer against
/// the store. A stored withdrawal must replay the answer given at
/// `answered_at`.
fn reissue(
    conn: &Connection,
    runtime: &CountingRuntime,
    request: &Request,
    now: DateTime<Utc>,
    answered_at: DateTime<Utc>,
) -> Result<Answer, &'static str> {
    let before = snapshot(conn)?;
    let execution = request.send(conn, runtime, now).map_err(|_| REFUSED)?;
    let wrote = snapshot(conn)? != before;
    let stored_now = stored(conn, request.scene)?;
    Ok(match execution {
        CancelExecution::Rejected(error) => {
            // The answer is exactly the refusal the store now holds.
            let held = stored_response(conn, request.scene)?
                .is_some_and(|response| json!(error) == response["error"]);
            match (error.code(), wrote, stored_now) {
                (ErrorCode::IllegalTransition, true, Stored::AttemptRefusal) if held => {
                    Answer::Refused
                }
                (
                    ErrorCode::IllegalTransition,
                    false,
                    Stored::TimeoutRefusal | Stored::AttemptRefusal,
                ) if held => Answer::RefusalReplayed,
                _ => Answer::Mismatched,
            }
        }
        CancelExecution::Success {
            payload,
            replayed,
            terminal,
        } => {
            // The owner's own adoption answers, replayed exactly when the
            // answer is.
            let own = terminal.replayed == replayed
                && terminal.key == ADOPTION_KEY
                && terminal.command == request.scene.command();
            let held = stored_response(conn, request.scene)?
                .is_some_and(|response| response["result"] == payload);
            match (wrote, replayed) {
                (true, false)
                    if own
                        && held
                        && stored_now == Stored::Withdrawn
                        && payload == withdrawn(conn, now)? =>
                {
                    Answer::Withdrawn
                }
                (false, true)
                    if own
                        && held
                        && stored_now == Stored::Withdrawn
                        && payload == withdrawn(conn, answered_at)? =>
                {
                    Answer::Replayed
                }
                _ => Answer::Mismatched,
            }
        }
    })
}

/// What the restarted daemon, its pass and the re-sent requests did with the
/// store the worker left.
struct Restarted {
    retry_after_restart: Retry,
    stored_after_restart: Stored,
    restart_is_noop: bool,
    reissue: Answer,
    retry_after_reissue: Retry,
    stored_after_reissue: Stored,
    pass_is_noop: bool,
    resend: Answer,
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
    let scene = boundary.scene();
    // A stored withdrawal answers as it did when it was committed: by the
    // worker, or by the owner's first re-sent request.
    let answered_at = at(
        base,
        if boundary.committed() {
            scene.cancel_at()
        } else {
            scene.reissue_at()
        },
    );
    // One runtime counts every stop and launch for the restart, each pass and
    // each re-sent request.
    let runtime = CountingRuntime::default();
    let (request, before_restart) = {
        let conn = crate::store::open_store(path).map_err(|_| FAILED)?;
        (Request::read(&conn, scene)?, snapshot(&conn)?)
    };
    let leadership = restart(home, &runtime, at(base, scene.restart_at()))?;
    let conn = crate::store::open_store(path).map_err(|_| FAILED)?;
    let retry_after_restart = retry(&conn)?;
    let stored_after_restart = stored(&conn, scene)?;
    let restart_is_noop = snapshot(&conn)? == before_restart;
    // The owner never saw an answer, so they send the same request again.
    let reissued = reissue(
        &conn,
        &runtime,
        &request,
        at(base, scene.reissue_at()),
        answered_at,
    )?;
    let retry_after_reissue = retry(&conn)?;
    let stored_after_reissue = stored(&conn, scene)?;
    let before_pass = snapshot(&conn)?;
    drop(conn);
    // A pass after the backoff must launch nothing.
    pass(home, &runtime, &leadership, at(base, PASS), "deadline")?;
    let conn = crate::store::open_store(path).map_err(|_| FAILED)?;
    let pass_is_noop = snapshot(&conn)? == before_pass;
    let resent = reissue(&conn, &runtime, &request, at(base, RESEND), answered_at)?;
    let launches = runtime.launches();
    let stops = runtime.stops();
    let before_replay = snapshot(&conn)?;
    // The same request once more, then a pass on its own connection, must
    // answer the same without a write, a stop or a launch.
    let replayed = reissue(&conn, &runtime, &request, at(base, REPLAY), answered_at)? == resent;
    drop(conn);
    pass(home, &runtime, &leadership, at(base, REPLAY), "deadline")?;
    let conn = crate::store::open_store(path).map_err(|_| FAILED)?;
    let replay_is_noop = replayed
        && snapshot(&conn)? == before_replay
        && runtime.stops() == stops
        && runtime.launches() == launches;
    drop(conn);
    drop(leadership);
    Ok(Restarted {
        retry_after_restart,
        stored_after_restart,
        restart_is_noop,
        reissue: reissued,
        retry_after_reissue,
        stored_after_reissue,
        pass_is_noop,
        resend: resent,
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
fn observe_pause(path: &Path, scene: Scene) -> Result<(bool, Retry, Stored), &'static str> {
    let probe = open_raw(path)?;
    let open = write_transaction_open(&probe)?;
    Ok((open, retry(&probe)?, stored(&probe, scene)?))
}

fn case_matches(case: &Case) -> Result<(bool, Value), &'static str> {
    let root = super::scheduler_conformance_tempdir()?;
    crate::daemon::ensure_private_coven_home(root.path()).map_err(|_| FAILED)?;
    let payload = serde_json::to_vec(case).map_err(|_| INVALID)?;
    let (mut process, receiver) = spawn_worker::<Handshake>(WORKER, payload, root.path())?;
    let ready = receiver.recv_timeout(CHILD_GUARD).map_err(|_| FAILED)??;
    let crash = !case.boundary.control();
    if ready.boundary != case.boundary || ready.state != if crash { "paused" } else { "answered" } {
        return Err(FAILED);
    }
    let home = worker_home(root.path(), &ready.relative_home, HOME_PREFIX)?;
    let path = crate::api::store_path(&home);
    let scene = case.boundary.scene();
    let pause = crash.then(|| observe_pause(&path, scene)).transpose()?;
    let pause_consistent = pause.is_none_or(|(open, retry, stored)| {
        open == case.boundary.inside_transaction()
            && retry == case.expected.retry_after_exit
            && stored == case.expected.stored_after_exit
    });
    let status = process.finish(crash)?;
    let terminated = abruptly_terminated(&status);
    if crash != terminated || (!crash && !status.success()) {
        return Err(FAILED);
    }
    drop(process);
    let integrity_ok = raw_integrity_ok(&path)?;

    let (retry_after_exit, stored_after_exit) = {
        let conn = crate::store::open_store(&path).map_err(|_| FAILED)?;
        (retry(&conn)?, stored(&conn, scene)?)
    };
    let recovery = match recover(&home, &path, base()?, case.boundary) {
        Ok(recovery) => Some(recovery),
        Err(error) if error == REFUSED => None,
        Err(error) => return Err(error),
    };
    let recovery_refused = recovery.is_none();
    let observed = match &recovery {
        Some(recovery) => Expected {
            initial_outcome: if terminated {
                InitialOutcome::Terminated
            } else {
                InitialOutcome::Answered
            },
            retry_after_exit,
            stored_after_exit,
            retry_after_restart: recovery.retry_after_restart,
            stored_after_restart: recovery.stored_after_restart,
            restart_is_noop: recovery.restart_is_noop,
            reissue: recovery.reissue,
            retry_after_reissue: recovery.retry_after_reissue,
            stored_after_reissue: recovery.stored_after_reissue,
            pass_is_noop: recovery.pass_is_noop,
            resend: recovery.resend,
            launches: recovery.launches,
            stops: recovery.stops,
            integrity_ok,
            replay_is_noop: recovery.replay_is_noop,
        },
        None => Expected {
            initial_outcome: if terminated {
                InitialOutcome::Terminated
            } else {
                InitialOutcome::Answered
            },
            retry_after_exit,
            stored_after_exit,
            retry_after_restart: Retry::Mixed,
            stored_after_restart: Stored::Other,
            restart_is_noop: false,
            reissue: Answer::Mismatched,
            retry_after_reissue: Retry::Mixed,
            stored_after_reissue: Stored::Other,
            pass_is_noop: false,
            resend: Answer::Mismatched,
            launches: 0,
            stops: 0,
            integrity_ok,
            replay_is_noop: false,
        },
    };
    let passed = observed == case.expected && pause_consistent && !recovery_refused;
    let (transaction_open_at_pause, retry_at_pause, stored_at_pause) = match pause {
        Some((open, retry, stored)) => (Some(open), Some(retry), Some(stored)),
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
            "retryAtPause": retry_at_pause,
            "storedAtPause": stored_at_pause,
            "pauseConsistent": pause_consistent,
            "integrityOk": integrity_ok,
            "retryAfterExit": observed.retry_after_exit,
            "storedAfterExit": observed.stored_after_exit,
            "recoveryRefused": recovery_refused,
            "retryAfterRestart": observed.retry_after_restart,
            "storedAfterRestart": observed.stored_after_restart,
            "restartIsNoop": observed.restart_is_noop,
            "reissue": observed.reissue,
            "retryAfterReissue": observed.retry_after_reissue,
            "storedAfterReissue": observed.stored_after_reissue,
            "passIsNoop": observed.pass_is_noop,
            "resend": observed.resend,
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
            "../../../../../conformance/automations/runner/waiting-retry-cancellation-process-crash-recovery.vectors.json"
        ))
        .unwrap()
    }

    #[test]
    fn retry_cancellation_crash_vectors_restate_each_boundary_outcome() {
        let vectors: VectorSet = serde_json::from_value(vectors()).unwrap();
        assert_eq!(vectors.schema_version, SCHEMA_VERSION);
        for case in &vectors.cases {
            validate(case).unwrap();
        }
        let boundaries: Vec<_> = vectors.cases.iter().map(|case| case.boundary).collect();
        assert_eq!(boundaries, Boundary::ALL);
    }

    #[test]
    fn retry_cancellation_crash_vectors_require_each_boundary_and_both_controls() {
        let original = vectors();
        let mut missing = original.clone();
        missing["cases"].as_array_mut().unwrap().pop();
        assert_eq!(evaluate(&missing).unwrap_err(), INVALID);
        // A second control under its own case id, replacing a boundary.
        let mut duplicate = original.clone();
        let mut control = duplicate["cases"][5].clone();
        control["caseId"] = json!("second-uninterrupted-retry-cancellation-control");
        duplicate["cases"][0] = control;
        assert_eq!(evaluate(&duplicate).unwrap_err(), INVALID);
        let mut duplicate_id = original;
        duplicate_id["cases"][1]["caseId"] = duplicate_id["cases"][0]["caseId"].clone();
        assert_eq!(evaluate(&duplicate_id).unwrap_err(), INVALID);
    }

    #[test]
    fn retry_cancellation_crash_vectors_refuse_malformed_or_weakened_inputs() {
        let original = vectors();
        for (case, field, value) in [
            (0, "retryAfterExit", json!("cancelled")),
            (0, "storedAfterExit", json!("withdrawn")),
            (0, "reissue", json!("replayed")),
            (1, "restartIsNoop", json!(false)),
            (2, "retryAfterReissue", json!("scheduled")),
            (3, "storedAfterReissue", json!("none")),
            (4, "reissue", json!("withdrawn")),
            (4, "storedAfterRestart", json!("none")),
            (5, "resend", json!("withdrawn")),
            (5, "initialOutcome", json!("terminated")),
            (6, "retryAfterRestart", json!("scheduled")),
            (6, "restartIsNoop", json!(true)),
            (7, "reissue", json!("refusal_replayed")),
            (8, "storedAfterReissue", json!("timeout_refusal")),
            (9, "launches", json!(1)),
            (10, "reissue", json!("refused")),
            (10, "storedAfterExit", json!("none")),
            (11, "stops", json!(1)),
            (11, "passIsNoop", json!(false)),
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
            ("adoptionKey", json!("adopt:caller:chosen")),
            ("reason", json!("caller chosen")),
            ("boundary", json!("after_cancellation")),
            ("caseId", json!("")),
        ] {
            let mut invalid = original.clone();
            invalid["cases"][0][field] = value;
            assert_eq!(evaluate(&invalid).unwrap_err(), INVALID, "{field}");
        }
        let mut invalid = original;
        invalid["schemaVersion"] =
            json!("coven.automations.retry-timeout-process-crash-recovery-vectors.v1");
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
        assert_eq!(stored(&conn, Scene::Waiting).unwrap(), Stored::None);
        (temp, leadership, conn)
    }

    #[test]
    fn the_owner_withdraws_a_waiting_retry_once_and_it_never_launches() {
        let (_temp, leadership, conn) = scheduled_store();
        let (fence, base) = (leadership.fence(), base().unwrap());
        let runtime = CountingRuntime::default();
        let request = Request::read(&conn, Scene::Waiting).unwrap();
        let cancel_at = at(base, Scene::Waiting.cancel_at());
        assert_eq!(
            reissue(&conn, &runtime, &request, cancel_at, cancel_at).unwrap(),
            Answer::Withdrawn
        );
        assert_eq!(retry(&conn).unwrap(), Retry::Cancelled);
        assert_eq!(stored(&conn, Scene::Waiting).unwrap(), Stored::Withdrawn);
        // The backoff's end re-claims and launches nothing, and the request
        // replays its first answer.
        let tick = crate::automations::occurrences::tick_with_scheduler_fence(
            &conn,
            at(base, BACKOFF_END),
            &fence,
        )
        .unwrap();
        assert!(tick.claimed.is_empty());
        assert_eq!(
            reissue(&conn, &runtime, &request, at(base, RESEND), cancel_at).unwrap(),
            Answer::Replayed
        );
        assert_eq!(runtime.launches(), 0);
    }

    #[test]
    fn the_owner_is_refused_once_the_deadline_has_passed() {
        let (_temp, _leadership, conn) = scheduled_store();
        let base = base().unwrap();
        let runtime = CountingRuntime::default();
        let request = Request::read(&conn, Scene::Expired).unwrap();
        let cancel_at = at(base, Scene::Expired.cancel_at());
        // The command times the waiting retry out itself and stores its
        // refusal, which the same request replays.
        let before = snapshot(&conn).unwrap();
        match request.send(&conn, &runtime, cancel_at).unwrap() {
            CancelExecution::Rejected(error) => {
                assert_eq!(error.code(), ErrorCode::IllegalTransition);
            }
            _ => panic!("expected a refusal"),
        }
        assert_ne!(snapshot(&conn).unwrap(), before);
        assert_eq!(retry(&conn).unwrap(), Retry::TimedOut);
        assert_eq!(
            stored(&conn, Scene::Expired).unwrap(),
            Stored::TimeoutRefusal
        );
        assert_eq!(
            reissue(&conn, &runtime, &request, at(base, RESEND), cancel_at).unwrap(),
            Answer::RefusalReplayed
        );
        assert_eq!(runtime.launches(), 0);
    }

    #[test]
    fn each_command_is_whole_or_a_state_no_boundary_leaves() {
        let left: Vec<(Retry, Stored)> = Boundary::ALL
            .iter()
            .map(|boundary| {
                let expected = expected_for(*boundary);
                (expected.retry_after_exit, expected.stored_after_exit)
            })
            .collect();
        let (_temp, _leadership, conn) = scheduled_store();
        let base = base().unwrap();
        for scene in [Scene::Waiting, Scene::Expired] {
            let step = |sql: &str, whole: Option<(Retry, Stored)>| {
                conn.execute_batch(sql).unwrap();
                let now = (retry(&conn).unwrap(), stored(&conn, scene).unwrap());
                match whole {
                    Some(whole) => assert_eq!(now, whole, "{sql}"),
                    None => assert!(!left.contains(&now), "{sql}: {now:?}"),
                }
            };
            let now = iso(at(base, scene.cancel_at()));
            let command = scene.command();
            // Each command's writes in its order. The store holds one
            // occurrence and run; the writes read them.
            conn.execute_batch("BEGIN").unwrap();
            let (writes, whole) = match scene {
                Scene::Waiting => (
                    [
                        format!(
                            "UPDATE automation_attempts
                             SET state = 'cancelled', failure_class = 'cancelled',
                                 state_reason = '{REASON}', settled_at = '{now}'
                             WHERE state = 'adopted'"
                        ),
                        format!(
                            "UPDATE automation_runs
                             SET status = 'cancelled', finished_at = '{now}'"
                        ),
                        format!(
                            "UPDATE automation_occurrences
                             SET state = 'cancelled', failure_reason = '{REASON}',
                                 updated_at = '{now}'"
                        ),
                        format!(
                            "INSERT INTO automation_command_adoptions
                                (adoption_key, command, request_digest, outcome,
                                 response_json, adopted_at)
                             VALUES ('{ADOPTION_KEY}', '{command}', 'digest', 'committed',
                                     json_object('outcome', 'committed', 'result',
                                         json_object('status', 'cancelled',
                                             'reason', '{REASON}')),
                                     '{now}')"
                        ),
                    ],
                    (Retry::Cancelled, Stored::Withdrawn),
                ),
                Scene::Expired => (
                    [
                        format!(
                            "UPDATE automation_attempts
                             SET state = 'timed_out', failure_class = 'timeout',
                                 state_reason = '{TIMED_OUT}', settled_at = '{now}'
                             WHERE state = 'adopted'"
                        ),
                        format!(
                            "UPDATE automation_occurrences
                             SET state = 'failed', failure_reason = '{TIMED_OUT}',
                                 updated_at = '{now}'"
                        ),
                        format!(
                            "UPDATE automation_runs
                             SET status = 'failed', finished_at = '{now}'"
                        ),
                        format!(
                            "INSERT INTO automation_command_adoptions
                                (adoption_key, command, request_digest, outcome,
                                 response_json, adopted_at)
                             VALUES ('{ADOPTION_KEY}', '{command}', 'digest', 'rejected',
                                     json_object('outcome', 'rejected', 'error',
                                         json_object('code', 'ILLEGAL_TRANSITION',
                                             'message', '{TIMEOUT_REFUSAL}')),
                                     '{now}')"
                        ),
                    ],
                    (Retry::TimedOut, Stored::TimeoutRefusal),
                ),
            };
            let [first, second, third, last] = writes;
            step(&first, None);
            step(&second, None);
            step(&third, None);
            step(&last, Some(whole));
            conn.execute_batch("ROLLBACK").unwrap();
            assert_eq!(
                (retry(&conn).unwrap(), stored(&conn, scene).unwrap()),
                (Retry::Scheduled, Stored::None)
            );
        }
    }

    #[test]
    fn worker_refuses_invalid_or_weakened_input() {
        let mut case = vectors()["cases"][0].clone();
        case["storePath"] = json!("private-input");
        assert_eq!(worker(&case).unwrap_err(), INVALID);
        let mut case = vectors()["cases"][6].clone();
        case["expected"]["restartIsNoop"] = json!(true);
        assert_eq!(worker(&case).unwrap_err(), INVALID);
        let mut case = vectors()["cases"][11].clone();
        case["boundary"] = json!("waiting_control");
        assert_eq!(worker(&case).unwrap_err(), INVALID);
    }
}
