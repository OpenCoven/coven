//! Disposable owner-recovery crash fixtures: a daemon worker holds a scheduled
//! run whose cancellation the runtime never confirmed, and is killed while it
//! handles the owner's `occurrence.recover.v1`. The parent restarts the
//! daemon's scheduler and sends the same request again. Both launch through
//! no-op runtimes; no runtime process starts.
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
use crate::automations::cancellation::{
    execute_run_cancellation_at_stop_fence, CancellationExecution,
};
use crate::automations::definition::RoutineDefinition;
use crate::automations::leadership::SchedulerLeadership;
use crate::automations::recovery::{
    execute_occurrence_recovery, RecoveryExecution, RECOVER_ACTION,
};

const INVALID: &str = "conformance vector is invalid";
const FAILED: &str = "conformance suite execution failed";
/// A production recovery step or the re-issued command returned an error; the
/// case fails.
const REFUSED: &str = "recovery refused";
const SCHEMA_VERSION: &str =
    "coven.automations.occurrence-recovery-process-crash-recovery-vectors.v1";
const WORKER: &str = "occurrence-recovery-crash-worker";
const HOME_PREFIX: &str = "recovery-crash-";
/// The definition predates the schedule slot the worker claims.
const DEFINED_AT: &str = "2099-10-19T09:00:00.000Z";
/// The one adoption key the owner's recovery request carries, sent and re-sent.
const ADOPTION_KEY: &str = "adopt:conformance:recovery-crash";
/// The owner's earlier cancellation, whose stop the runtime never confirmed.
const CANCEL_KEY: &str = "adopt:conformance:recovery-crash-cancel";
/// The stored answer to the recovery request: the command, committed.
const ADOPTED: &str = "occurrence.recover.v1 committed";
const STATEMENT: &str = "The probe's work is idempotent; its stop was lost with the daemon.";
/// Seconds after `BASE`: the worker claims and launches the 09:00 slot, the
/// owner cancels it and the stop is not confirmed, a daemon restart loses the
/// session, and the worker handles the recovery; the parent restarts the
/// daemon, the owner re-sends the request, a pass dispatches a retry, its
/// session ends, and a pass settles the run. The run's 30-minute deadline is
/// never reached.
const CLAIM: i64 = 30;
const CANCEL: i64 = 60;
const LOST: i64 = 90;
const RECOVER: i64 = 120;
const RESTART: i64 = 150;
const REISSUE: i64 = 160;
const RETRY_PASS: i64 = 170;
const SESSION_END: i64 = 175;
const SETTLE_PASS: i64 = 180;
const REPLAY: i64 = 240;

/// The owner's determination of the lost stop's outcome.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
enum Determination {
    RetryWithNewAttempt,
    FailedDeterministic,
}

impl Determination {
    fn as_str(self) -> &'static str {
        match self {
            Self::RetryWithNewAttempt => "retry_with_new_attempt",
            Self::FailedDeterministic => "failed_deterministic",
        }
    }

    /// Where the worker can stop: after each write of the determination's
    /// transaction in its order, after its commit, and the uninterrupted
    /// control.
    fn boundaries(self) -> &'static [Boundary] {
        match self {
            Self::RetryWithNewAttempt => &[
                Boundary::AfterAttempt,
                Boundary::AfterReplan,
                Boundary::AfterRelease,
                Boundary::AfterFenceRelease,
                Boundary::AfterAdoption,
                Boundary::AfterCommit,
                Boundary::None,
            ],
            Self::FailedDeterministic => &[
                Boundary::AfterSettle,
                Boundary::AfterFinish,
                Boundary::AfterFenceRelease,
                Boundary::AfterAdoption,
                Boundary::AfterCommit,
                Boundary::None,
            ],
        }
    }

    fn retries(self) -> bool {
        self == Self::RetryWithNewAttempt
    }

    /// The store once the determination has committed.
    fn determined(self) -> Occurrence {
        if self.retries() {
            Occurrence::Retried
        } else {
            Occurrence::Failed
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
enum Boundary {
    AfterAttempt,
    AfterReplan,
    AfterRelease,
    AfterSettle,
    AfterFinish,
    AfterFenceRelease,
    AfterAdoption,
    AfterCommit,
    None,
}

impl Boundary {
    /// The worker stops inside a write transaction.
    fn inside_transaction(self) -> bool {
        !matches!(self, Self::AfterCommit | Self::None)
    }
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Case {
    case_id: String,
    determination: Determination,
    boundary: Boundary,
    definition: Value,
    expected: Expected,
}

#[derive(Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Expected {
    initial_outcome: InitialOutcome,
    occurrence_after_exit: Occurrence,
    restart_launches: u64,
    occurrence_after_restart: Occurrence,
    restart_is_noop: bool,
    reissue: Reissue,
    occurrence_after_reissue: Occurrence,
    launches: u64,
    final_occurrence: Occurrence,
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

/// The store's one occurrence with its run, its attempts, their sessions, the
/// run's stop fence and the answer stored under the owner's adoption key, read
/// together. Attempt 1 is always settled `ambiguous` on its `orphaned` session.
#[derive(Debug, Deserialize, Serialize, PartialEq, Clone, Copy)]
#[serde(rename_all = "snake_case")]
enum Occurrence {
    /// Waiting for the owner: the occurrence `recovery_required` with no lease,
    /// the run `running` on attempt 1's session under the cancellation's stop
    /// fence, and no attempt 2 or stored answer.
    Unresolved,
    /// The retry committed: attempt 2 `adopted` as an operator recovery of
    /// attempt 1's ambiguity, with no session; the run released from its
    /// session; the occurrence `planned` with no lease; no stop fence; and the
    /// request's committed adoption.
    Retried,
    /// Attempt 2 dispatched: `started` on a second, `running` session that the
    /// run is on, with the occurrence `running` under the daemon's lease, still
    /// with no stop fence.
    Relaunched,
    /// The run, attempt 2 and the occurrence `succeeded`, and attempt 2's
    /// session `completed`, with no stop fence.
    Succeeded,
    /// The failed determination committed: the occurrence `failed` with no
    /// lease, the run `failed` on attempt 1's session, no attempt 2, no stop
    /// fence, and the request's committed adoption.
    Failed,
    /// Anything else, including a partial determination.
    Mixed,
}

/// What the owner's re-sent request returned.
#[derive(Debug, Deserialize, Serialize, PartialEq, Clone, Copy)]
#[serde(rename_all = "snake_case")]
enum Reissue {
    /// The determination committed now: a fresh response naming the store's
    /// run (and attempt 2, for a retry), recovered at the re-sent request's time.
    Committed,
    /// The stored response: a replay naming the same, recovered earlier.
    Replayed,
    /// A refusal, or a response that does not name what the store holds.
    Mismatched,
}

/// What each determination and boundary must produce. A vector restates it, so
/// it cannot weaken it.
fn expected_for(determination: Determination, boundary: Boundary) -> Expected {
    let committed = !boundary.inside_transaction();
    let retries = determination.retries();
    let relaunched = |otherwise| {
        if retries {
            Occurrence::Relaunched
        } else {
            otherwise
        }
    };
    Expected {
        initial_outcome: if boundary == Boundary::None {
            InitialOutcome::Responded
        } else {
            InitialOutcome::Terminated
        },
        occurrence_after_exit: if committed {
            determination.determined()
        } else {
            Occurrence::Unresolved
        },
        restart_launches: u64::from(retries && committed),
        occurrence_after_restart: if committed {
            relaunched(Occurrence::Failed)
        } else {
            Occurrence::Unresolved
        },
        // Only a committed retry gives the restart anything to do.
        restart_is_noop: !(retries && committed),
        reissue: if committed {
            Reissue::Replayed
        } else {
            Reissue::Committed
        },
        occurrence_after_reissue: if committed {
            relaunched(Occurrence::Failed)
        } else {
            determination.determined()
        },
        launches: u64::from(retries),
        final_occurrence: if retries {
            Occurrence::Succeeded
        } else {
            Occurrence::Failed
        },
        final_sessions: if retries { 2 } else { 1 },
        integrity_ok: true,
        replay_is_noop: true,
    }
}

/// A routine with no automatic retry. The runner launches only a routine with
/// a working directory; a no-op runtime never touches it.
fn synthetic_definition() -> Value {
    json!({"schemaVersion":1,"id":"conformance-recovery-crash",
        "name":"Conformance recovery crash","status":"ACTIVE",
        "rrule":"FREQ=DAILY;BYHOUR=9","timezone":"utc","misfire":"latest",
        "overlap":"forbid","timeoutMinutes":30,"runtime":"coven-code",
        "prompt":"Run the native recovery crash conformance probe.",
        "cwd":"/work/project"})
}

fn validate(case: &Case) -> Result<(), &'static str> {
    if !valid_case_id(&case.case_id)
        || case.definition != synthetic_definition()
        || !case.determination.boundaries().contains(&case.boundary)
        || case.expected != expected_for(case.determination, case.boundary)
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

/// Where the worker parks. A retry opens attempt 2, returns the occurrence to
/// `planned` and releases the run's session; a failed determination settles
/// the occurrence and then the run `failed`. Either then releases the run's
/// stop fence, and the response is stored last. Nothing else commits after the
/// pause is installed.
fn pause_at(boundary: Boundary) -> Option<Pause<'static>> {
    match boundary {
        Boundary::AfterAttempt => Some(Pause::After("AFTER INSERT ON main.automation_attempts")),
        Boundary::AfterReplan | Boundary::AfterSettle => Some(Pause::After(
            "AFTER UPDATE OF state ON main.automation_occurrences
             WHEN OLD.state IS NOT NEW.state",
        )),
        Boundary::AfterRelease => Some(Pause::After(
            "AFTER UPDATE OF session_id ON main.automation_runs
             WHEN OLD.session_id IS NOT NEW.session_id",
        )),
        Boundary::AfterFinish => Some(Pause::After(
            "AFTER UPDATE OF status ON main.automation_runs
             WHEN OLD.status IS NOT NEW.status",
        )),
        Boundary::AfterFenceRelease => {
            Some(Pause::After("AFTER DELETE ON main.automation_stop_fences"))
        }
        Boundary::AfterAdoption => Some(Pause::After(
            "AFTER INSERT ON main.automation_command_adoptions",
        )),
        Boundary::AfterCommit => Some(Pause::AfterCommit),
        Boundary::None => None,
    }
}

/// The owner's request: their determination of the lost stop's outcome.
fn request(occurrence_id: &str, determination: Determination) -> Value {
    json!({"action": RECOVER_ACTION, "adoptionKey": ADOPTION_KEY, "occurrenceId": occurrence_id,
        "evidenceDetermination": determination.as_str(), "statement": STATEMENT})
}

/// A runtime that never confirms a stop, so the owner's cancellation leaves
/// the attempt's outcome unknown.
struct UnconfirmedStops;

impl crate::api::SessionRuntime for UnconfirmedStops {
    fn launch_session(&self, _launch: &crate::api::SessionLaunch) -> anyhow::Result<()> {
        Ok(())
    }

    fn send_input(&self, _session_id: &str, _payload: &Value) -> anyhow::Result<()> {
        Ok(())
    }

    fn kill_session(&self, _session_id: &str) -> anyhow::Result<()> {
        anyhow::bail!("the runtime did not confirm the stop")
    }
}

/// Launches the routine's 09:00 slot under `home`'s scheduler leadership, has
/// the owner cancel it through a runtime that never confirms the stop, and
/// restarts the daemon, which loses the session. Returns the restarted
/// daemon's leadership and the occurrence's id.
fn unresolved(
    home: &Path,
    base: DateTime<Utc>,
) -> Result<(SchedulerLeadership, String), &'static str> {
    let path = crate::api::store_path(home);
    let conn = &crate::store::open_store(&path).map_err(|_| FAILED)?;
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
    if report.dispatched.len() != 1 || !report.failed.is_empty() {
        return Err(FAILED);
    }
    let (occurrence_id, run_id, attempt_id, session_id): (String, String, String, String) = conn
        .query_row(
            "SELECT r.occurrence_id, r.id, a.id, r.session_id
             FROM automation_runs AS r
             JOIN automation_attempts AS a ON a.run_id = r.id",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .map_err(|_| FAILED)?;
    // The owner cancels the run through the production cancellation. The
    // daemon's API fences the stop on the wall clock; this fences it on the
    // fixture's fixed clock, so its times agree with the scheduler's. The
    // runtime never confirms the stop, so the attempt's outcome is unknown and
    // only the owner can decide it.
    let cancel_at = at(base, CANCEL);
    let cancelled = execute_run_cancellation_at_stop_fence(
        conn,
        &UnconfirmedStops,
        json!({"action": "coven.automations.run.cancel.v1", "adoptionKey": CANCEL_KEY,
            "runId": run_id, "attemptId": attempt_id,
            "runtimeCorrelation": {"sessionId": session_id}, "scope": "run",
            "reason": "not needed today", "requestedBy": {"principalId": "conformance:owner"}}),
        cancel_at,
        cancel_at,
    )
    .map_err(|_| FAILED)?;
    let CancellationExecution::Success(cancelled) = cancelled else {
        return Err(FAILED);
    };
    if cancelled.payload["status"] != "recovery_required" {
        return Err(FAILED);
    }
    // The daemon then dies without its graceful shutdown, and its restart
    // finds the session lost. It launches nothing.
    drop(leadership);
    let runtime = CountingRuntime::default();
    let leadership = restart(home, &runtime, at(base, LOST)).map_err(|_| FAILED)?;
    let conn = crate::store::open_store(&path).map_err(|_| FAILED)?;
    if runtime.launches() != 0 || occurrence(&conn)?.0 != Occurrence::Unresolved {
        return Err(FAILED);
    }
    Ok((leadership, occurrence_id))
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
    let (leadership, occurrence_id) = unresolved(&home, base)?;
    let conn = crate::store::open_store(&crate::api::store_path(&home)).map_err(|_| FAILED)?;
    if let Some(pause_at) = pause_at(case.boundary) {
        let handshake =
            json!({"state":"paused", "boundary":case.boundary, "relativeHome":relative_home});
        install_pause(&conn, pause_at, handshake)?;
    }
    // The restarted daemon handles the owner's request as its API does.
    let response = execute_occurrence_recovery(
        &conn,
        request(&occurrence_id, case.determination),
        at(base, RECOVER),
    )
    .map_err(|_| FAILED)?;
    if case.boundary != Boundary::None
        || !matches!(
            response,
            RecoveryExecution::Success {
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

/// Classifies the store's one occurrence with its run, attempts, sessions,
/// stop fence and stored answer, and counts its sessions.
fn occurrence(conn: &Connection) -> Result<(Occurrence, u64), &'static str> {
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
        return Ok((Occurrence::Mixed, session_count));
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
        String,
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
                    a1.state = 'ambiguous' AND a1.settled_at IS NOT NULL
                        AND s1.status = 'orphaned',
                    a2.state, a2.settled_at IS NOT NULL,
                    a2.retry_classification IS 'operator_recovery'
                        AND a2.prior_attempt_number IS 1
                        AND a2.prior_disposition IS 'ambiguous',
                    s2.status,
                    CASE
                        WHEN NOT EXISTS (SELECT 1 FROM automation_stop_fences) THEN 'none'
                        WHEN (SELECT COUNT(*) FROM automation_stop_fences) = 1
                            AND EXISTS (
                                SELECT 1 FROM automation_stop_fences
                                WHERE run_id = r.id AND session_id = a1.session_id
                                  AND owner = 'cancellation'
                            ) THEN 'cancellation'
                        ELSE 'other'
                    END,
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
                    row.get(10)?,
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
            first_ambiguous,
            second,
            second_settled,
            second_recovers_first,
            second_session,
            fence,
            adoption,
        )| {
            (
                attempts,
                sessions,
                occurrence.as_str(),
                *leased,
                run.as_str(),
                run_on.as_str(),
                *first_ambiguous,
                second.as_deref(),
                *second_settled,
                *second_recovers_first,
                second_session.as_deref(),
                fence.as_str(),
                adoption.as_deref(),
            )
        },
    );
    let class = match row {
        Some((
            1,
            1,
            "recovery_required",
            false,
            "running",
            "first",
            true,
            None,
            _,
            _,
            None,
            "cancellation",
            None,
        )) => Occurrence::Unresolved,
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
            "none",
            Some(ADOPTED),
        )) => Occurrence::Retried,
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
            "none",
            Some(ADOPTED),
        )) => Occurrence::Relaunched,
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
            "none",
            Some(ADOPTED),
        )) => Occurrence::Succeeded,
        Some((
            1,
            1,
            "failed",
            false,
            "failed",
            "first",
            true,
            None,
            _,
            _,
            None,
            "none",
            Some(ADOPTED),
        )) => Occurrence::Failed,
        _ => Occurrence::Mixed,
    };
    Ok((class, session_count))
}

/// Sends the owner's request again at `now` and classifies the answer against
/// the determination the store holds.
fn reissue(
    conn: &Connection,
    occurrence_id: &str,
    determination: Determination,
    now: DateTime<Utc>,
) -> Result<Reissue, &'static str> {
    let execution = execute_occurrence_recovery(conn, request(occurrence_id, determination), now)
        .map_err(|_| REFUSED)?;
    let RecoveryExecution::Success { payload, replayed } = execution else {
        return Ok(Reissue::Mismatched);
    };
    type Recorded = (
        String,
        String,
        Option<String>,
        Option<String>,
        Option<String>,
    );
    let recorded: Option<Recorded> = conn
        .query_row(
            "SELECT o.automation_id, r.id, r.finished_at, a2.id, a2.opened_at
             FROM automation_occurrences AS o
             JOIN automation_runs AS r ON r.occurrence_id = o.id
             LEFT JOIN automation_attempts AS a2 ON a2.run_id = r.id AND a2.attempt_number = 2
             WHERE o.id = ?1",
            [occurrence_id],
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
        .optional()
        .map_err(|_| FAILED)?;
    let Some((automation_id, run_id, finished_at, attempt_id, opened_at)) = recorded else {
        return Ok(Reissue::Mismatched);
    };
    let mut expected = json!({"occurrenceId": occurrence_id, "automationId": automation_id,
        "evidenceDetermination": determination.as_str(), "runId": run_id,
        "statement": STATEMENT});
    // A retry is recovered when it opens attempt 2; a failed determination
    // when it finishes the run.
    let recovered_at = if determination.retries() {
        let (Some(attempt_id), Some(opened_at)) = (attempt_id, opened_at) else {
            return Ok(Reissue::Mismatched);
        };
        expected["attemptId"] = json!(attempt_id);
        expected["attemptNumber"] = json!(2);
        expected["status"] = json!("planned");
        opened_at
    } else {
        let (None, Some(finished_at)) = (attempt_id, finished_at) else {
            return Ok(Reissue::Mismatched);
        };
        expected["status"] = json!("failed");
        finished_at
    };
    expected["recoveredAt"] = json!(recovered_at);
    Ok(
        match (payload == expected, replayed, recovered_at == iso(now)) {
            (true, false, true) => Reissue::Committed,
            (true, true, false) => Reissue::Replayed,
            _ => Reissue::Mismatched,
        },
    )
}

/// Every occurrence, run, attempt, event, session, adoption, cancellation and
/// stop-fence row, for an exact before/after comparison.
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
            "automation_cancellations",
            "automation_stop_fences",
        ],
    )
}

/// What the restarted daemon and the re-sent request did with the store the
/// worker left.
struct Restarted {
    restart_launches: u64,
    occurrence_after_restart: Occurrence,
    restart_is_noop: bool,
    reissue: Reissue,
    occurrence_after_reissue: Occurrence,
    launches: u64,
    final_occurrence: Occurrence,
    final_sessions: u64,
    replay_is_noop: bool,
}

fn recover(
    home: &Path,
    path: &Path,
    base: DateTime<Utc>,
    determination: Determination,
) -> Result<Restarted, &'static str> {
    let before_restart = snapshot(&crate::store::open_store(path).map_err(|_| FAILED)?)?;
    let runtime = CountingRuntime::default();
    let leadership = restart(home, &runtime, at(base, RESTART))?;
    let conn = crate::store::open_store(path).map_err(|_| FAILED)?;
    let (occurrence_after_restart, _) = occurrence(&conn)?;
    let restart_launches = runtime.launches();
    let restart_is_noop = snapshot(&conn)? == before_restart;
    let occurrence_id: String = conn
        .query_row("SELECT id FROM automation_occurrences", [], |row| {
            row.get(0)
        })
        .map_err(|_| FAILED)?;
    // The owner never saw an answer, so they send the same request again.
    let reissued = reissue(&conn, &occurrence_id, determination, at(base, REISSUE))?;
    let (occurrence_after_reissue, _) = occurrence(&conn)?;
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
    if occurrence(&conn)?.0 == Occurrence::Relaunched {
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
    let (final_occurrence, final_sessions) = occurrence(&conn)?;
    let launches = runtime.launches();
    let before_replay = snapshot(&conn)?;
    // The same request once more, then a pass, each on its own connection,
    // must replay without a write or a launch.
    let replayed =
        reissue(&conn, &occurrence_id, determination, at(base, REPLAY))? == Reissue::Replayed;
    drop(conn);
    pass(home, &runtime, &leadership, at(base, REPLAY), "deadline")?;
    let conn = crate::store::open_store(path).map_err(|_| FAILED)?;
    let replay_is_noop =
        replayed && snapshot(&conn)? == before_replay && runtime.launches() == launches;
    drop(conn);
    drop(leadership);
    Ok(Restarted {
        restart_launches,
        occurrence_after_restart,
        restart_is_noop,
        reissue: reissued,
        occurrence_after_reissue,
        launches,
        final_occurrence,
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
    // Every boundary of both determinations, once each.
    let required = Determination::RetryWithNewAttempt.boundaries().len()
        + Determination::FailedDeterministic.boundaries().len();
    if vectors.schema_version != SCHEMA_VERSION || vectors.cases.len() != required {
        return Err(INVALID);
    }
    let mut ids = BTreeSet::new();
    let mut boundaries = BTreeSet::new();
    for case in &vectors.cases {
        validate(case)?;
        if !ids.insert(&case.case_id) || !boundaries.insert((case.determination, case.boundary)) {
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
fn observe_pause(path: &Path) -> Result<(bool, Occurrence), &'static str> {
    let probe = open_raw(path)?;
    let open = write_transaction_open(&probe)?;
    Ok((open, occurrence(&probe)?.0))
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
    let (transaction_open_at_pause, occurrence_at_pause) = pause.unzip();
    let committed_at_exit = case.expected.occurrence_after_exit;
    let pause_consistent = pause.is_none_or(|(open, committed)| {
        open == case.boundary.inside_transaction() && committed == committed_at_exit
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
    let (occurrence_after_exit, sessions_after_exit) = occurrence(&conn)?;
    drop(conn);
    let recovery = match recover(&home, &path, base, case.determination) {
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
        occurrence_after_exit,
        restart_launches: recovery
            .as_ref()
            .map_or(0, |recovery| recovery.restart_launches),
        occurrence_after_restart: recovery.as_ref().map_or(Occurrence::Mixed, |recovery| {
            recovery.occurrence_after_restart
        }),
        restart_is_noop: recovery
            .as_ref()
            .is_some_and(|recovery| recovery.restart_is_noop),
        reissue: recovery
            .as_ref()
            .map_or(Reissue::Mismatched, |recovery| recovery.reissue),
        occurrence_after_reissue: recovery.as_ref().map_or(Occurrence::Mixed, |recovery| {
            recovery.occurrence_after_reissue
        }),
        launches: recovery.as_ref().map_or(0, |recovery| recovery.launches),
        final_occurrence: recovery
            .as_ref()
            .map_or(Occurrence::Mixed, |recovery| recovery.final_occurrence),
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
            "determination": case.determination,
            "boundary": case.boundary,
            "initialOutcome": observed.initial_outcome,
            "abruptlyTerminated": terminated,
            "transactionOpenAtPause": transaction_open_at_pause,
            "occurrenceAtPause": occurrence_at_pause,
            "pauseConsistent": pause_consistent,
            "integrityOk": integrity_ok,
            "occurrenceAfterExit": occurrence_after_exit,
            "sessionsAfterExit": sessions_after_exit,
            "recoveryRefused": recovery_refused,
            "restartLaunches": observed.restart_launches,
            "occurrenceAfterRestart": observed.occurrence_after_restart,
            "restartIsNoop": observed.restart_is_noop,
            "reissue": observed.reissue,
            "occurrenceAfterReissue": observed.occurrence_after_reissue,
            "launches": observed.launches,
            "finalOccurrence": observed.final_occurrence,
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
            "../../../../../conformance/automations/runner/occurrence-recovery-process-crash-recovery.vectors.json"
        ))
        .unwrap()
    }

    /// The boundaries in vector order: each determination's in its order.
    fn cases() -> Vec<(Determination, Boundary)> {
        [
            Determination::RetryWithNewAttempt,
            Determination::FailedDeterministic,
        ]
        .into_iter()
        .flat_map(|determination| {
            determination
                .boundaries()
                .iter()
                .map(move |boundary| (determination, *boundary))
        })
        .collect()
    }

    #[test]
    fn recovery_crash_vectors_restate_each_boundary_outcome() {
        let vectors: VectorSet = serde_json::from_value(vectors()).unwrap();
        assert_eq!(vectors.schema_version, SCHEMA_VERSION);
        for case in &vectors.cases {
            validate(case).unwrap();
        }
        let boundaries: Vec<_> = vectors
            .cases
            .iter()
            .map(|case| (case.determination, case.boundary))
            .collect();
        assert_eq!(boundaries, cases());
        assert_eq!(boundaries.len(), 13);
    }

    #[test]
    fn recovery_crash_vectors_require_each_boundary_and_the_control() {
        let original = vectors();
        let mut missing = original.clone();
        missing["cases"].as_array_mut().unwrap().pop();
        assert_eq!(evaluate(&missing).unwrap_err(), INVALID);
        // A second retry control under its own case id, replacing a boundary.
        let mut duplicate = original.clone();
        let mut control = duplicate["cases"][6].clone();
        control["caseId"] = json!("second-uninterrupted-retry-control");
        duplicate["cases"][0] = control;
        assert_eq!(evaluate(&duplicate).unwrap_err(), INVALID);
        let mut duplicate_id = original.clone();
        duplicate_id["cases"][1]["caseId"] = duplicate_id["cases"][0]["caseId"].clone();
        assert_eq!(evaluate(&duplicate_id).unwrap_err(), INVALID);
        // A boundary only the other determination writes, with outcomes that
        // would otherwise be consistent.
        for (case, determination, boundary) in [
            (0, Determination::RetryWithNewAttempt, Boundary::AfterSettle),
            (
                7,
                Determination::FailedDeterministic,
                Boundary::AfterAttempt,
            ),
        ] {
            let mut foreign = original.clone();
            foreign["cases"][case]["boundary"] = json!(boundary);
            foreign["cases"][case]["expected"] =
                serde_json::to_value(expected_for(determination, boundary)).unwrap();
            assert_eq!(evaluate(&foreign).unwrap_err(), INVALID, "{boundary:?}");
        }
    }

    #[test]
    fn recovery_crash_vectors_refuse_malformed_or_weakened_inputs() {
        let original = vectors();
        for (case, field, value) in [
            (0, "initialOutcome", json!("responded")),
            (0, "occurrenceAfterExit", json!("retried")),
            (1, "restartLaunches", json!(1)),
            (1, "reissue", json!("replayed")),
            (1, "restartIsNoop", json!(false)),
            (2, "occurrenceAfterRestart", json!("relaunched")),
            (2, "occurrenceAfterReissue", json!("relaunched")),
            (3, "occurrenceAfterExit", json!("mixed")),
            (4, "launches", json!(2)),
            (5, "occurrenceAfterExit", json!("unresolved")),
            (5, "reissue", json!("committed")),
            (5, "restartLaunches", json!(0)),
            (5, "restartIsNoop", json!(true)),
            (6, "finalOccurrence", json!("relaunched")),
            (6, "finalSessions", json!(1)),
            (7, "occurrenceAfterReissue", json!("unresolved")),
            (8, "launches", json!(1)),
            (9, "finalOccurrence", json!("succeeded")),
            (10, "finalSessions", json!(2)),
            (11, "restartLaunches", json!(1)),
            (11, "occurrenceAfterRestart", json!("relaunched")),
            (12, "integrityOk", json!(false)),
            (12, "replayIsNoop", json!(false)),
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
            ("determination", json!("cancelled")),
            ("boundary", json!("after_dispatch")),
            ("caseId", json!("")),
        ] {
            let mut invalid = original.clone();
            invalid["cases"][0][field] = value;
            assert_eq!(evaluate(&invalid).unwrap_err(), INVALID, "{field}");
        }
        let mut invalid = original;
        invalid["schemaVersion"] =
            json!("coven.automations.attempt-retry-process-crash-recovery-vectors.v1");
        assert_eq!(evaluate(&invalid).unwrap_err(), INVALID);
    }

    /// A store whose occurrence waits for the owner, as the worker leaves it
    /// before the request.
    fn unresolved_store() -> (
        tempfile::TempDir,
        std::path::PathBuf,
        Connection,
        SchedulerLeadership,
        String,
    ) {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        crate::daemon::ensure_private_coven_home(&home).unwrap();
        let (leadership, occurrence_id) = unresolved(&home, base().unwrap()).unwrap();
        let conn = crate::store::open_store(&crate::api::store_path(&home)).unwrap();
        (temp, home, conn, leadership, occurrence_id)
    }

    fn adopt(conn: &Connection, key: &str, command: &str) {
        conn.execute(
            "INSERT INTO automation_command_adoptions (
                adoption_key, command, automation_id, request_digest, outcome,
                revision, response_json, adopted_at
             ) VALUES (?1, ?2, 'conformance-recovery-crash', 'digest', 'committed', NULL,
                       '{}', ?3)",
            [key, command, &iso(at(base().unwrap(), RECOVER))],
        )
        .unwrap();
    }

    #[test]
    fn a_retry_is_whole_or_mixed() {
        let (_temp, home, conn, leadership, occurrence_id) = unresolved_store();
        let base = base().unwrap();
        assert_eq!(occurrence(&conn).unwrap(), (Occurrence::Unresolved, 1));
        // The retry transaction's writes, in its order: every proper prefix is
        // mixed, and all five are the retry.
        conn.execute_batch("BEGIN").unwrap();
        conn.execute(
            "INSERT INTO automation_attempts
                (id, run_id, occurrence_id, attempt_number, adoption_key,
                 occurrence_fence_generation, dispatch_generation, state,
                 prior_attempt_number, prior_disposition, retry_classification,
                 state_reason, not_before, opened_at)
             SELECT 'attempt-prefix-2', r.id, o.id, 2, 'automation:prefix:2', o.attempt, 0,
                    'adopted', 1, 'ambiguous', 'operator_recovery', 'operator recovery', ?1, ?1
             FROM automation_runs AS r
             JOIN automation_occurrences AS o ON o.id = r.occurrence_id",
            [iso(at(base, RECOVER))],
        )
        .unwrap();
        assert_eq!(occurrence(&conn).unwrap().0, Occurrence::Mixed);
        conn.execute(
            "UPDATE automation_occurrences
             SET state = 'planned', lease_owner = NULL, lease_expires_at = NULL",
            [],
        )
        .unwrap();
        assert_eq!(occurrence(&conn).unwrap().0, Occurrence::Mixed);
        conn.execute("UPDATE automation_runs SET session_id = NULL", [])
            .unwrap();
        assert_eq!(occurrence(&conn).unwrap().0, Occurrence::Mixed);
        conn.execute("DELETE FROM automation_stop_fences", [])
            .unwrap();
        assert_eq!(occurrence(&conn).unwrap().0, Occurrence::Mixed);
        adopt(&conn, ADOPTION_KEY, "occurrence.recover.v1");
        assert_eq!(occurrence(&conn).unwrap().0, Occurrence::Retried);
        conn.execute_batch("ROLLBACK").unwrap();
        // Any answer under the owner's key, even another command's, means the
        // request was handled, so a still-waiting occurrence with one is mixed.
        conn.execute_batch("BEGIN").unwrap();
        adopt(&conn, ADOPTION_KEY, "attempt.retry.v1");
        assert_eq!(occurrence(&conn).unwrap().0, Occurrence::Mixed);
        conn.execute_batch("ROLLBACK").unwrap();
        let retry = Determination::RetryWithNewAttempt;
        assert_eq!(
            reissue(&conn, &occurrence_id, retry, at(base, RECOVER)).unwrap(),
            Reissue::Committed
        );
        assert_eq!(occurrence(&conn).unwrap(), (Occurrence::Retried, 1));
        // The same request replays; the other determination under the same key
        // is refused, not committed, and leaves the retry as it was.
        assert_eq!(
            reissue(&conn, &occurrence_id, retry, at(base, REISSUE)).unwrap(),
            Reissue::Replayed
        );
        assert_eq!(
            reissue(
                &conn,
                &occurrence_id,
                Determination::FailedDeterministic,
                at(base, REISSUE)
            )
            .unwrap(),
            Reissue::Mismatched
        );
        assert_eq!(occurrence(&conn).unwrap(), (Occurrence::Retried, 1));
        let runtime = CountingRuntime::default();
        pass(
            &home,
            &runtime,
            &leadership,
            at(base, RETRY_PASS),
            "deadline",
        )
        .unwrap();
        assert_eq!(occurrence(&conn).unwrap(), (Occurrence::Relaunched, 2));
        assert_eq!(runtime.launches(), 1);
        // A stop fence back on the lost session is not this retry.
        conn.execute_batch("BEGIN").unwrap();
        conn.execute(
            "INSERT INTO automation_stop_fences
                (run_id, session_id, owner, operation_key, acquired_at, execution_expires_at)
             SELECT r.id, a.session_id, 'cancellation', 'pending-stop', ?1, ?1
             FROM automation_runs AS r
             JOIN automation_attempts AS a ON a.run_id = r.id AND a.attempt_number = 1",
            [iso(at(base, RETRY_PASS))],
        )
        .unwrap();
        assert_eq!(occurrence(&conn).unwrap().0, Occurrence::Mixed);
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
        assert_eq!(occurrence(&conn).unwrap(), (Occurrence::Succeeded, 2));
        assert_eq!(
            reissue(&conn, &occurrence_id, retry, at(base, REPLAY)).unwrap(),
            Reissue::Replayed
        );
    }

    #[test]
    fn a_failed_determination_is_whole_or_mixed() {
        let (_temp, home, conn, leadership, occurrence_id) = unresolved_store();
        let base = base().unwrap();
        // The failed determination's writes, in its order: every proper prefix
        // is mixed, and all four are the determination.
        conn.execute_batch("BEGIN").unwrap();
        conn.execute(
            "UPDATE automation_occurrences
             SET state = 'failed', lease_owner = NULL, lease_expires_at = NULL",
            [],
        )
        .unwrap();
        assert_eq!(occurrence(&conn).unwrap().0, Occurrence::Mixed);
        conn.execute("UPDATE automation_runs SET status = 'failed'", [])
            .unwrap();
        assert_eq!(occurrence(&conn).unwrap().0, Occurrence::Mixed);
        conn.execute("DELETE FROM automation_stop_fences", [])
            .unwrap();
        assert_eq!(occurrence(&conn).unwrap().0, Occurrence::Mixed);
        adopt(&conn, ADOPTION_KEY, "occurrence.recover.v1");
        assert_eq!(occurrence(&conn).unwrap().0, Occurrence::Failed);
        conn.execute_batch("ROLLBACK").unwrap();
        let failed = Determination::FailedDeterministic;
        assert_eq!(
            reissue(&conn, &occurrence_id, failed, at(base, RECOVER)).unwrap(),
            Reissue::Committed
        );
        assert_eq!(occurrence(&conn).unwrap(), (Occurrence::Failed, 1));
        assert_eq!(
            reissue(&conn, &occurrence_id, failed, at(base, REISSUE)).unwrap(),
            Reissue::Replayed
        );
        // A failed run back on its fence is not this determination.
        conn.execute_batch("BEGIN").unwrap();
        conn.execute(
            "INSERT INTO automation_stop_fences
                (run_id, session_id, owner, operation_key, acquired_at, execution_expires_at)
             SELECT id, session_id, 'cancellation', 'pending-stop', ?1, ?1
             FROM automation_runs",
            [iso(at(base, REISSUE))],
        )
        .unwrap();
        assert_eq!(occurrence(&conn).unwrap().0, Occurrence::Mixed);
        conn.execute_batch("ROLLBACK").unwrap();
        // Nothing launches the failed occurrence again.
        let runtime = CountingRuntime::default();
        for at_seconds in [RETRY_PASS, SETTLE_PASS] {
            pass(
                &home,
                &runtime,
                &leadership,
                at(base, at_seconds),
                "deadline",
            )
            .unwrap();
        }
        assert_eq!(runtime.launches(), 0);
        assert_eq!(occurrence(&conn).unwrap(), (Occurrence::Failed, 1));
    }

    #[test]
    fn worker_refuses_invalid_or_weakened_input() {
        let mut case = vectors()["cases"][0].clone();
        case["storePath"] = json!("private-input");
        assert_eq!(worker(&case).unwrap_err(), INVALID);
        let mut case = vectors()["cases"][0].clone();
        case["expected"]["launches"] = json!(2);
        assert_eq!(worker(&case).unwrap_err(), INVALID);
        let mut case = vectors()["cases"][7].clone();
        case["determination"] = json!("retry_with_new_attempt");
        assert_eq!(worker(&case).unwrap_err(), INVALID);
    }
}
