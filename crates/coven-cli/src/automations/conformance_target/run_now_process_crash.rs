//! Disposable run-now crash fixtures: the owner runs a routine now with
//! `occurrence.runNow.v1`, and a daemon worker is killed while the command
//! claims its manual occurrence or launches it. The parent restarts the
//! daemon's scheduler, the owner sends the same request again, and passes
//! after the run's deadline and the manual claim's lease must not launch it
//! again. Every launch goes through a no-op runtime; no runtime process
//! starts.
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
use super::run_launch_process_crash::{pass, restart, snapshot_tables, CountingRuntime, REFUSED};
use super::valid_case_id;
use crate::automations::contract::canonical_json::sha256_hex;
use crate::automations::definition::RoutineDefinition;
use crate::automations::leadership::SchedulerLeadership;
use crate::automations::run_now::{execute_run_now, RunNowExecution, RUN_NOW_ACTION};

const INVALID: &str = "conformance vector is invalid";
const FAILED: &str = "conformance suite execution failed";
const SCHEMA_VERSION: &str = "coven.automations.run-now-process-crash-recovery-vectors.v1";
const WORKER: &str = "run-now-crash-worker";
const HOME_PREFIX: &str = "run-now-crash-";
pub(super) const AUTOMATION_ID: &str = "conformance-run-now-crash";
/// The one adoption key the owner's request carries, sent and re-sent.
pub(super) const ADOPTION_KEY: &str = "adopt:conformance:run-now-crash";
/// The definition is stored after the last slot its schedule had before the
/// request, and its next slot falls after every pass, so no pass claims a
/// scheduled slot beside the manual one.
const DEFINED_AT: &str = "2099-10-20T08:00:00.000Z";
/// Seconds after `BASE`: the owner runs the routine now at 30, so its run's
/// 30-minute deadline falls at 1,830 and the manual claim's 60-minute lease
/// expires at 3,630. The parent restarts the daemon 30 seconds after the
/// request, and the owner sends it again 10 seconds later. A run that request
/// launches ends cleanly at 90. Passes follow before the deadline, after it,
/// and after the lease, then the request once more and a replay.
pub(super) const RUN_NOW: i64 = 30;
pub(super) const RESTART: i64 = 60;
pub(super) const REISSUE: i64 = 70;
pub(super) const SESSION_END: i64 = 90;
pub(super) const PASS: i64 = 120;
pub(super) const DEADLINE_PASS: i64 = 1_900;
pub(super) const LEASE_PASS: i64 = 3_700;
pub(super) const RESEND: i64 = 3_710;
pub(super) const REPLAY: i64 = 3_800;
/// The occurrence's reason once a pass recovers its expired claim.
const LEASE_EXPIRED: &str = "lease expired";
const HOLD_EXPIRED: &str = "no operator retry was opened before the run's deadline";
use crate::automations::runner::MANUAL_CLAIM_RESTART_REASON;
/// The attempt's and the occurrence's reason once the deadline settles a run.
const TIMED_OUT: &str = "automation run timed out";

/// Where the worker can stop: inside the command's claim transaction and
/// after its commit, inside the launch transaction and after its commit,
/// inside the ownership transaction and after its commit, and the
/// uninterrupted control.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
enum Boundary {
    AfterClaim,
    AfterAdoption,
    AfterClaimCommit,
    AfterSession,
    AfterDispatching,
    AfterLaunchCommit,
    AfterOwnership,
    AfterOwnershipCommit,
    None,
}

impl Boundary {
    const ALL: [Self; 9] = [
        Self::AfterClaim,
        Self::AfterAdoption,
        Self::AfterClaimCommit,
        Self::AfterSession,
        Self::AfterDispatching,
        Self::AfterLaunchCommit,
        Self::AfterOwnership,
        Self::AfterOwnershipCommit,
        Self::None,
    ];

    /// What the store has committed when the worker stops.
    fn committed(self) -> Launch {
        match self {
            Self::AfterClaim | Self::AfterAdoption => Launch::Absent,
            Self::AfterClaimCommit | Self::AfterSession | Self::AfterDispatching => Launch::Claimed,
            Self::AfterLaunchCommit | Self::AfterOwnership => Launch::Launching,
            Self::AfterOwnershipCommit | Self::None => Launch::Owned,
        }
    }

    /// The worker stops inside a write transaction.
    fn inside_transaction(self) -> bool {
        matches!(
            self,
            Self::AfterClaim
                | Self::AfterAdoption
                | Self::AfterSession
                | Self::AfterDispatching
                | Self::AfterOwnership
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

#[derive(Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Expected {
    initial_outcome: InitialOutcome,
    launch_after_exit: Launch,
    stored_after_exit: Stored,
    launch_after_restart: Launch,
    restart_is_noop: bool,
    reissue: Answer,
    launch_after_reissue: Launch,
    launch_after_pass: Launch,
    launch_after_deadline: Launch,
    launch_after_lease: Launch,
    resend: Answer,
    launches: u64,
    stops: u64,
    integrity_ok: bool,
    replay_is_noop: bool,
}

#[derive(Debug, Deserialize, Serialize, PartialEq, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub(super) enum InitialOutcome {
    Terminated,
    Answered,
}

/// The store's manual occurrence with its run, attempt and session, read
/// together. Every class but `Absent` has exactly the one occurrence the
/// owner's key names, of the `manual` kind and claimed once, no stop fence,
/// and at most one run, attempt and session, which belong together.
#[derive(Debug, Deserialize, Serialize, PartialEq, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub(super) enum Launch {
    /// No occurrence, run, attempt, session or stop fence.
    Absent,
    /// The occurrence claimed under the manual lease, with no run, attempt or
    /// session.
    Claimed,
    /// The launch committed: the run `running`, its attempt `dispatching`
    /// with no session, the session `created`, and the occurrence still
    /// claimed under the manual lease.
    Launching,
    /// Ownership published: the run, the session and the occurrence
    /// `running`, and the attempt `started` on that session.
    Owned,
    /// As `Owned`, with the session `orphaned` by a restart.
    Orphaned,
    /// The run, the settled attempt and the occurrence `succeeded` with no
    /// lease, and the session `completed`.
    Succeeded,
    /// The deadline settled the run: the attempt `timed_out` with the
    /// `timeout` class on the session, the run `failed`, the occurrence
    /// `failed` with no lease, each with the deadline's reason where it has
    /// one, and the session `killed`.
    TimedOut,
    /// The restart contained the previous daemon's launch, as it does a
    /// scheduled one (#1502): the session `killed`, the attempt settled
    /// `failed`, and the run held for an operator retry, the occurrence still
    /// `claimed` or `running` with no failure reason.
    Held,
    /// The held run's deadline passed with no retry opened: the run, the
    /// attempt and the occurrence `failed`, the occurrence with no lease and
    /// the hold's reason, and the session `killed`.
    HeldExpired,
    /// The claim's lease expired before any launch: the occurrence `failed`
    /// with the lease's reason and no lease, and no run, attempt or session.
    Expired,
    /// The restart found the claim unlaunched and failed it at once: the
    /// occurrence `failed` with the restart's reason and no lease, and no
    /// run, attempt or session (#1502).
    Abandoned,
    /// Anything else, including a partial claim or launch.
    Mixed,
}

/// The owner's stored answer: the store's only command adoption.
#[derive(Debug, Deserialize, Serialize, PartialEq, Clone, Copy)]
#[serde(rename_all = "snake_case")]
enum Stored {
    /// No answer is stored.
    None,
    /// The owner's key and command, committed with the manual occurrence the
    /// key names.
    Claimed,
    /// Anything else.
    Other,
}

/// What the owner's re-sent request answered, checked against the store.
#[derive(Debug, Deserialize, Serialize, PartialEq, Clone, Copy)]
#[serde(rename_all = "snake_case")]
enum Answer {
    /// A fresh claim and launch: the store's occurrence and its running run,
    /// with exactly one launch.
    Launched,
    /// The stored claim, replayed as the store now projects it, without a
    /// write or a launch.
    Replayed,
    /// Anything else.
    Mismatched,
}

/// What the restart, the re-sent request, the pass before the deadline, the
/// pass after it and the pass after the lease leave of what the worker
/// committed. A request that finds nothing claims and launches afresh, and that
/// run ends cleanly. The restart fails a stored claim that never launched at
/// once, and contains a launch the previous daemon made as it does a
/// scheduled one, holding the run for an operator retry until its deadline
/// (#1502).
pub(super) fn launch_path(committed: Launch) -> (Launch, Launch, Launch, Launch, Launch) {
    match committed {
        Launch::Absent => (
            Launch::Absent,
            Launch::Owned,
            Launch::Succeeded,
            Launch::Succeeded,
            Launch::Succeeded,
        ),
        Launch::Claimed => (
            Launch::Abandoned,
            Launch::Abandoned,
            Launch::Abandoned,
            Launch::Abandoned,
            Launch::Abandoned,
        ),
        _ => (
            Launch::Held,
            Launch::Held,
            Launch::Held,
            Launch::HeldExpired,
            Launch::HeldExpired,
        ),
    }
}

/// What each boundary must produce. A vector restates it, so it cannot weaken
/// it.
fn expected_for(boundary: Boundary) -> Expected {
    let committed = boundary.committed();
    let (after_restart, after_reissue, after_pass, after_deadline, after_lease) =
        launch_path(committed);
    Expected {
        initial_outcome: if boundary == Boundary::None {
            InitialOutcome::Answered
        } else {
            InitialOutcome::Terminated
        },
        launch_after_exit: committed,
        stored_after_exit: if committed == Launch::Absent {
            Stored::None
        } else {
            Stored::Claimed
        },
        launch_after_restart: after_restart,
        restart_is_noop: after_restart == committed,
        reissue: if committed == Launch::Absent {
            Answer::Launched
        } else {
            Answer::Replayed
        },
        launch_after_reissue: after_reissue,
        launch_after_pass: after_pass,
        launch_after_deadline: after_deadline,
        launch_after_lease: after_lease,
        resend: Answer::Replayed,
        launches: u64::from(committed == Launch::Absent),
        stops: u64::from(after_deadline == Launch::TimedOut),
        integrity_ok: true,
        replay_is_noop: true,
    }
}

/// A routine with a working directory and a 30-minute deadline. The runner
/// launches only a routine with a working directory; a no-op runtime never
/// touches it.
pub(super) fn synthetic_definition() -> Value {
    json!({"schemaVersion":1,"id":AUTOMATION_ID,
        "name":"Conformance run-now crash","status":"ACTIVE",
        "rrule":"FREQ=DAILY;BYHOUR=21","timezone":"utc","misfire":"latest",
        "overlap":"forbid","timeoutMinutes":30,"runtime":"coven-code",
        "prompt":"Run the native run-now crash conformance probe.",
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

pub(super) fn base() -> Result<DateTime<Utc>, &'static str> {
    Ok(DateTime::parse_from_rfc3339(BASE)
        .map_err(|_| FAILED)?
        .with_timezone(&Utc))
}

/// The manual occurrence the owner's key names.
pub(super) fn occurrence_id() -> String {
    format!("occ-manual-{}", &sha256_hex(ADOPTION_KEY.as_bytes())[..32])
}

/// Where the worker parks inside SQLite. The claim transaction inserts the
/// occurrence first and the answer last. The launch transaction inserts the
/// session first and marks the attempt `dispatching` last; the ownership
/// transaction marks the occurrence `running` last. Nothing else in the
/// command commits.
fn pause_at(boundary: Boundary) -> Option<Pause<'static>> {
    match boundary {
        Boundary::AfterClaim => Some(Pause::After("AFTER INSERT ON main.automation_occurrences")),
        Boundary::AfterAdoption => Some(Pause::After(
            "AFTER INSERT ON main.automation_command_adoptions",
        )),
        Boundary::AfterClaimCommit => Some(Pause::AfterCommit),
        Boundary::AfterSession => Some(Pause::After("AFTER INSERT ON main.sessions")),
        Boundary::AfterDispatching => Some(Pause::After(
            "AFTER UPDATE OF state ON main.automation_attempts
             WHEN OLD.state IS NOT NEW.state",
        )),
        Boundary::AfterLaunchCommit => Some(Pause::AfterCommits(2)),
        Boundary::AfterOwnership => Some(Pause::After(
            "AFTER UPDATE OF state ON main.automation_occurrences
             WHEN OLD.state IS NOT NEW.state",
        )),
        Boundary::AfterOwnershipCommit => Some(Pause::AfterCommits(3)),
        Boundary::None => None,
    }
}

/// The owner's request, identical each time it is sent.
fn request() -> Value {
    json!({"action": RUN_NOW_ACTION, "adoptionKey": ADOPTION_KEY,
        "automationId": AUTOMATION_ID})
}

/// Stores the synthetic routine, defined before the request.
pub(super) fn store_definition(conn: &Connection, definition: &Value) -> Result<(), &'static str> {
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
    let run_now_at = at(base()?, RUN_NOW);
    let conn = crate::store::open_store(&crate::api::store_path(&home)).map_err(|_| FAILED)?;
    store_definition(&conn, &case.definition)?;
    // The daemon holds its scheduler's leadership while it serves the API.
    let leadership = SchedulerLeadership::acquire(&home, &conn, run_now_at).map_err(|_| FAILED)?;
    if let Some(pause_at) = pause_at(case.boundary) {
        let handshake =
            json!({"state":"paused", "boundary":case.boundary, "relativeHome":relative_home});
        install_pause(&conn, pause_at, handshake)?;
    }
    let answer = execute_run_now(
        &conn,
        &crate::api::NoopSessionRuntime,
        request(),
        run_now_at,
    )
    .map_err(|_| FAILED)?;
    let launched = match &answer {
        RunNowExecution::Success {
            payload,
            replayed: false,
        } => *payload == projection(&conn)? && payload["status"] == "running",
        _ => false,
    };
    if case.boundary != Boundary::None || !launched || launch(&conn)? != Launch::Owned {
        return Err(FAILED);
    }
    // Exits like a daemon that dies after answering: without the graceful
    // shutdown's reconciliation or the release of its scheduler authority.
    drop(conn);
    drop(leadership);
    announce(&json!({"state":"answered", "boundary":case.boundary, "relativeHome":relative_home}))
}

/// What the command answers for the store's manual occurrence: the
/// occurrence, and its run with that run's session, if any.
fn projection(conn: &Connection) -> Result<Value, &'static str> {
    let (automation_id, state): (String, String) = conn
        .query_row(
            "SELECT automation_id, state FROM automation_occurrences WHERE id = ?1",
            [occurrence_id()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|_| FAILED)?;
    let run: Option<(String, String, Option<String>)> = conn
        .query_row(
            "SELECT id, status, session_id FROM automation_runs
             WHERE occurrence_id = ?1 ORDER BY started_at DESC LIMIT 1",
            [occurrence_id()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(|_| FAILED)?;
    let mut payload = json!({"occurrenceId": occurrence_id(), "automationId": automation_id,
        "occurrenceState": state});
    if let Some((run_id, status, session_id)) = run {
        payload["runId"] = json!(run_id);
        payload["status"] = json!(status);
        payload["sessionId"] = json!(session_id);
    }
    Ok(payload)
}

/// One run with its first attempt, its session and the occurrence, as the
/// classes read them.
struct RunRow {
    run: String,
    run_finished: bool,
    attempt: String,
    failure_class: Option<String>,
    attempt_reason: Option<String>,
    attempt_on_session: bool,
    settled: bool,
    session: String,
}

/// Classifies the store's manual occurrence with its run, attempt and session.
pub(super) fn launch(conn: &Connection) -> Result<Launch, &'static str> {
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
    if counts == [0, 0, 0, 0, 0] {
        return Ok(Launch::Absent);
    }
    // The one occurrence must be the owner's, manual and claimed once.
    let occurrence: Option<(String, String, Option<String>)> = conn
        .query_row(
            "SELECT state,
                    CASE
                        WHEN lease_owner = 'manual' AND lease_expires_at IS NOT NULL
                            THEN 'manual'
                        WHEN lease_owner IS NULL AND lease_expires_at IS NULL THEN 'none'
                        ELSE 'other'
                    END,
                    failure_reason
             FROM automation_occurrences
             WHERE id = ?1 AND kind = 'manual' AND attempt = 1
               AND automation_id = ?2",
            [occurrence_id().as_str(), AUTOMATION_ID],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(|_| FAILED)?;
    let Some((state, lease, reason)) = occurrence else {
        return Ok(Launch::Mixed);
    };
    let occurrence = (state.as_str(), lease.as_str(), reason.as_deref());
    match counts {
        [1, 0, 0, 0, 0] => {
            return Ok(match occurrence {
                ("claimed", "manual", None) => Launch::Claimed,
                ("failed", "none", Some(LEASE_EXPIRED)) => Launch::Expired,
                ("failed", "none", Some(MANUAL_CLAIM_RESTART_REASON)) => Launch::Abandoned,
                _ => Launch::Mixed,
            })
        }
        [1, 1, 1, 1, 0] => {}
        _ => return Ok(Launch::Mixed),
    }
    // A run, attempt and session that do not belong together match no row.
    let row = conn
        .query_row(
            "SELECT r.status, r.finished_at IS NOT NULL, a.state, a.failure_class,
                    a.state_reason, a.session_id IS NOT NULL, a.settled_at IS NOT NULL,
                    s.status
             FROM automation_runs AS r
             JOIN automation_attempts AS a ON a.run_id = r.id
             JOIN sessions AS s ON s.id = r.session_id
             WHERE r.occurrence_id = ?1 AND r.automation_id = ?2
               AND a.occurrence_id = ?1 AND a.attempt_number = 1
               AND (a.session_id IS NULL OR a.session_id = s.id)",
            [occurrence_id().as_str(), AUTOMATION_ID],
            |row| {
                Ok(RunRow {
                    run: row.get(0)?,
                    run_finished: row.get(1)?,
                    attempt: row.get(2)?,
                    failure_class: row.get(3)?,
                    attempt_reason: row.get(4)?,
                    attempt_on_session: row.get(5)?,
                    settled: row.get(6)?,
                    session: row.get(7)?,
                })
            },
        )
        .optional()
        .map_err(|_| FAILED)?;
    let Some(row) = row else {
        return Ok(Launch::Mixed);
    };
    Ok(
        match (
            row.run.as_str(),
            row.run_finished,
            row.attempt.as_str(),
            row.failure_class.as_deref(),
            row.attempt_reason.as_deref(),
            row.attempt_on_session,
            row.settled,
            row.session.as_str(),
            occurrence,
        ) {
            (
                "running",
                false,
                "dispatching",
                None,
                None,
                false,
                false,
                "created",
                ("claimed", "manual", None),
            ) => Launch::Launching,
            (
                "running",
                false,
                "started",
                None,
                None,
                true,
                false,
                "running",
                ("running", "manual", None),
            ) => Launch::Owned,
            (
                "running",
                false,
                "started",
                None,
                None,
                true,
                false,
                "orphaned",
                ("running", "manual", None),
            ) => Launch::Orphaned,
            (
                "succeeded",
                true,
                "succeeded",
                None,
                None,
                true,
                true,
                "completed",
                ("succeeded", "none", None),
            ) => Launch::Succeeded,
            (
                "failed",
                true,
                "timed_out",
                Some("timeout"),
                Some(TIMED_OUT),
                true,
                true,
                "killed",
                ("failed", "none", Some(TIMED_OUT)),
            ) => Launch::TimedOut,
            (
                "running",
                false,
                "failed",
                _,
                _,
                _,
                true,
                "killed",
                ("claimed" | "running", _, None),
            ) => Launch::Held,
            (
                "failed",
                true,
                "failed",
                _,
                _,
                _,
                true,
                "killed",
                ("failed", "none", Some(HOLD_EXPIRED)),
            ) => Launch::HeldExpired,
            _ => Launch::Mixed,
        },
    )
}

/// Classifies the owner's stored answer. The flat request reserves no result
/// key, so a reservation is never this answer.
fn stored(conn: &Connection) -> Result<Stored, &'static str> {
    let adoptions: Vec<(String, String, Option<String>, String, String)> = conn
        .prepare(
            "SELECT adoption_key, command, automation_id, outcome, response_json
             FROM automation_command_adoptions",
        )
        .and_then(|mut statement| {
            statement
                .query_map([], |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                })?
                .collect()
        })
        .map_err(|_| FAILED)?;
    let reservations: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM automation_command_reservations",
            [],
            |row| row.get(0),
        )
        .map_err(|_| FAILED)?;
    if reservations != 0 {
        return Ok(Stored::Other);
    }
    let [(key, command, automation_id, outcome, response)] = adoptions.as_slice() else {
        return Ok(if adoptions.is_empty() {
            Stored::None
        } else {
            Stored::Other
        });
    };
    let response: Value = serde_json::from_str(response).map_err(|_| FAILED)?;
    Ok(
        if key == ADOPTION_KEY
            && command == "occurrence.runNow.v1"
            && automation_id.as_deref() == Some(AUTOMATION_ID)
            && outcome == "committed"
            && response
                == json!({"outcome": "committed", "result": {"occurrenceId": occurrence_id()}})
        {
            Stored::Claimed
        } else {
            Stored::Other
        },
    )
}

/// Every row the command, a restart or a pass can write, for an exact
/// before/after comparison.
pub(super) fn snapshot(
    conn: &Connection,
) -> Result<Vec<Vec<rusqlite::types::Value>>, &'static str> {
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
/// the store and the runtime's launches.
fn reissue(
    conn: &Connection,
    runtime: &CountingRuntime,
    now: DateTime<Utc>,
) -> Result<Answer, &'static str> {
    let before = snapshot(conn)?;
    let launches = runtime.launches();
    let execution = execute_run_now(conn, runtime, request(), now).map_err(|_| REFUSED)?;
    let wrote = snapshot(conn)? != before;
    let launched = runtime.launches() - launches;
    if stored(conn)? != Stored::Claimed {
        return Ok(Answer::Mismatched);
    }
    let projected = projection(conn)?;
    Ok(match execution {
        RunNowExecution::Success {
            payload,
            replayed: false,
        } if wrote && launched == 1 && payload["status"] == "running" && payload == projected => {
            Answer::Launched
        }
        RunNowExecution::Success {
            payload,
            replayed: true,
        } if !wrote && launched == 0 && payload == projected => Answer::Replayed,
        _ => Answer::Mismatched,
    })
}

/// Ends the session the re-sent request launched, the way a runtime reports a
/// clean exit.
pub(super) fn end_relaunched_session(
    conn: &Connection,
    now: DateTime<Utc>,
) -> Result<(), &'static str> {
    let session_id: String = conn
        .query_row("SELECT id FROM sessions", [], |row| row.get(0))
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

/// What the restarted daemon, its passes and the re-sent requests did with
/// the store the worker left.
struct Recovery {
    launch_after_restart: Launch,
    restart_is_noop: bool,
    reissue: Answer,
    launch_after_reissue: Launch,
    launch_after_pass: Launch,
    launch_after_deadline: Launch,
    launch_after_lease: Launch,
    resend: Answer,
    launches: u64,
    stops: u64,
    replay_is_noop: bool,
}

fn recover(home: &Path, path: &Path, base: DateTime<Utc>) -> Result<Recovery, &'static str> {
    // One runtime counts every stop and launch for the restart, each pass and
    // each re-sent request.
    let runtime = CountingRuntime::default();
    let before_restart = {
        let conn = crate::store::open_store(path).map_err(|_| FAILED)?;
        snapshot(&conn)?
    };
    let leadership = restart(home, &runtime, at(base, RESTART))?;
    let conn = crate::store::open_store(path).map_err(|_| FAILED)?;
    let launch_after_restart = launch(&conn)?;
    let restart_is_noop = snapshot(&conn)? == before_restart;
    // The owner never saw an answer, so they send the same request again.
    let reissued = reissue(&conn, &runtime, at(base, REISSUE))?;
    let launch_after_reissue = launch(&conn)?;
    if reissued == Answer::Launched && launch_after_reissue == Launch::Owned {
        end_relaunched_session(&conn, at(base, SESSION_END))?;
    }
    drop(conn);
    let pass_at = |second: i64| -> Result<Launch, &'static str> {
        pass(home, &runtime, &leadership, at(base, second), "deadline")?;
        let conn = crate::store::open_store(path).map_err(|_| FAILED)?;
        launch(&conn)
    };
    let launch_after_pass = pass_at(PASS)?;
    let launch_after_deadline = pass_at(DEADLINE_PASS)?;
    let launch_after_lease = pass_at(LEASE_PASS)?;
    let conn = crate::store::open_store(path).map_err(|_| FAILED)?;
    let resent = reissue(&conn, &runtime, at(base, RESEND))?;
    let launches = runtime.launches();
    let stops = runtime.stops();
    let before_replay = snapshot(&conn)?;
    // The same request once more, then a pass on its own connection, must
    // answer the same without a write, a stop or a launch.
    let replayed = reissue(&conn, &runtime, at(base, REPLAY))? == resent;
    drop(conn);
    pass(home, &runtime, &leadership, at(base, REPLAY), "deadline")?;
    let conn = crate::store::open_store(path).map_err(|_| FAILED)?;
    let replay_is_noop = replayed
        && snapshot(&conn)? == before_replay
        && runtime.stops() == stops
        && runtime.launches() == launches;
    drop(conn);
    drop(leadership);
    Ok(Recovery {
        launch_after_restart,
        restart_is_noop,
        reissue: reissued,
        launch_after_reissue,
        launch_after_pass,
        launch_after_deadline,
        launch_after_lease,
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
fn observe_pause(path: &Path) -> Result<(bool, Launch, Stored), &'static str> {
    let probe = open_raw(path)?;
    let open = write_transaction_open(&probe)?;
    Ok((open, launch(&probe)?, stored(&probe)?))
}

fn case_matches(case: &Case) -> Result<(bool, Value), &'static str> {
    let root = super::scheduler_conformance_tempdir()?;
    crate::daemon::ensure_private_coven_home(root.path()).map_err(|_| FAILED)?;
    let payload = serde_json::to_vec(case).map_err(|_| INVALID)?;
    let (mut process, receiver) = spawn_worker::<Handshake>(WORKER, payload, root.path())?;
    let ready = receiver.recv_timeout(CHILD_GUARD).map_err(|_| FAILED)??;
    let crash = case.boundary != Boundary::None;
    if ready.boundary != case.boundary || ready.state != if crash { "paused" } else { "answered" } {
        return Err(FAILED);
    }
    let home = worker_home(root.path(), &ready.relative_home, HOME_PREFIX)?;
    let path = crate::api::store_path(&home);
    let pause = crash.then(|| observe_pause(&path)).transpose()?;
    let pause_consistent = pause.is_none_or(|(open, launch, stored)| {
        open == case.boundary.inside_transaction()
            && launch == case.expected.launch_after_exit
            && stored == case.expected.stored_after_exit
    });
    let status = process.finish(crash)?;
    let terminated = abruptly_terminated(&status);
    if crash != terminated || (!crash && !status.success()) {
        return Err(FAILED);
    }
    drop(process);
    let integrity_ok = raw_integrity_ok(&path)?;

    let (launch_after_exit, stored_after_exit) = {
        let conn = crate::store::open_store(&path).map_err(|_| FAILED)?;
        (launch(&conn)?, stored(&conn)?)
    };
    let recovery = match recover(&home, &path, base()?) {
        Ok(recovery) => Some(recovery),
        Err(error) if error == REFUSED => None,
        Err(error) => return Err(error),
    };
    let recovery_refused = recovery.is_none();
    let initial_outcome = if terminated {
        InitialOutcome::Terminated
    } else {
        InitialOutcome::Answered
    };
    let observed = match &recovery {
        Some(recovery) => Expected {
            initial_outcome,
            launch_after_exit,
            stored_after_exit,
            launch_after_restart: recovery.launch_after_restart,
            restart_is_noop: recovery.restart_is_noop,
            reissue: recovery.reissue,
            launch_after_reissue: recovery.launch_after_reissue,
            launch_after_pass: recovery.launch_after_pass,
            launch_after_deadline: recovery.launch_after_deadline,
            launch_after_lease: recovery.launch_after_lease,
            resend: recovery.resend,
            launches: recovery.launches,
            stops: recovery.stops,
            integrity_ok,
            replay_is_noop: recovery.replay_is_noop,
        },
        None => Expected {
            initial_outcome,
            launch_after_exit,
            stored_after_exit,
            launch_after_restart: Launch::Mixed,
            restart_is_noop: false,
            reissue: Answer::Mismatched,
            launch_after_reissue: Launch::Mixed,
            launch_after_pass: Launch::Mixed,
            launch_after_deadline: Launch::Mixed,
            launch_after_lease: Launch::Mixed,
            resend: Answer::Mismatched,
            launches: 0,
            stops: 0,
            integrity_ok,
            replay_is_noop: false,
        },
    };
    let passed = observed == case.expected && pause_consistent && !recovery_refused;
    let (transaction_open_at_pause, launch_at_pause, stored_at_pause) = match pause {
        Some((open, launch, stored)) => (Some(open), Some(launch), Some(stored)),
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
            "launchAtPause": launch_at_pause,
            "storedAtPause": stored_at_pause,
            "pauseConsistent": pause_consistent,
            "integrityOk": integrity_ok,
            "launchAfterExit": observed.launch_after_exit,
            "storedAfterExit": observed.stored_after_exit,
            "recoveryRefused": recovery_refused,
            "launchAfterRestart": observed.launch_after_restart,
            "restartIsNoop": observed.restart_is_noop,
            "reissue": observed.reissue,
            "launchAfterReissue": observed.launch_after_reissue,
            "launchAfterPass": observed.launch_after_pass,
            "launchAfterDeadline": observed.launch_after_deadline,
            "launchAfterLease": observed.launch_after_lease,
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
            "../../../../../conformance/automations/runner/run-now-process-crash-recovery.vectors.json"
        ))
        .unwrap()
    }

    #[test]
    fn run_now_crash_vectors_restate_each_boundary_outcome() {
        let vectors: VectorSet = serde_json::from_value(vectors()).unwrap();
        assert_eq!(vectors.schema_version, SCHEMA_VERSION);
        for case in &vectors.cases {
            validate(case).unwrap();
        }
        let boundaries: Vec<_> = vectors.cases.iter().map(|case| case.boundary).collect();
        assert_eq!(boundaries, Boundary::ALL);
    }

    #[test]
    fn run_now_crash_vectors_require_each_boundary_and_the_control() {
        let original = vectors();
        let mut missing = original.clone();
        missing["cases"].as_array_mut().unwrap().pop();
        assert_eq!(evaluate(&missing).unwrap_err(), INVALID);
        // A second control under its own case id, replacing a boundary.
        let mut duplicate = original.clone();
        let mut control = duplicate["cases"][8].clone();
        control["caseId"] = json!("second-uninterrupted-run-now-control");
        duplicate["cases"][0] = control;
        assert_eq!(evaluate(&duplicate).unwrap_err(), INVALID);
        let mut duplicate_id = original;
        duplicate_id["cases"][1]["caseId"] = duplicate_id["cases"][0]["caseId"].clone();
        assert_eq!(evaluate(&duplicate_id).unwrap_err(), INVALID);
    }

    #[test]
    fn run_now_crash_vectors_refuse_malformed_or_weakened_inputs() {
        let original = vectors();
        for (case, field, value) in [
            (0, "launchAfterExit", json!("claimed")),
            (0, "reissue", json!("replayed")),
            (1, "launchAfterExit", json!("claimed")),
            (1, "launches", json!(0)),
            (1, "launchAfterPass", json!("owned")),
            (2, "storedAfterExit", json!("none")),
            (2, "reissue", json!("launched")),
            (2, "launchAfterPass", json!("expired")),
            (3, "launchAfterLease", json!("claimed")),
            (4, "launches", json!(1)),
            (5, "launchAfterRestart", json!("timed_out")),
            (5, "stops", json!(1)),
            (6, "launchAfterDeadline", json!("launching")),
            (7, "launchAfterRestart", json!("owned")),
            (7, "restartIsNoop", json!(true)),
            (8, "initialOutcome", json!("terminated")),
            (8, "resend", json!("launched")),
            (8, "integrityOk", json!(false)),
            (8, "replayIsNoop", json!(false)),
        ] {
            let mut invalid = original.clone();
            invalid["cases"][case]["expected"][field] = value;
            assert_eq!(evaluate(&invalid).unwrap_err(), INVALID, "{case} {field}");
        }
        for (field, value) in [
            ("name", json!("Caller chosen")),
            ("status", json!("PAUSED")),
            ("cwd", json!("/private-input")),
            ("timeoutMinutes", json!(90)),
            ("rrule", json!("FREQ=DAILY;BYHOUR=9")),
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
            json!("coven.automations.run-launch-process-crash-recovery-vectors.v1");
        assert_eq!(evaluate(&invalid).unwrap_err(), INVALID);
    }

    /// A home holding the synthetic routine, with a connection to its store.
    fn defined_store() -> (tempfile::TempDir, Connection) {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        crate::daemon::ensure_private_coven_home(&home).unwrap();
        let conn = crate::store::open_store(&crate::api::store_path(&home)).unwrap();
        store_definition(&conn, &synthetic_definition()).unwrap();
        (temp, conn)
    }

    #[test]
    fn the_owner_runs_a_routine_once_and_a_replay_never_launches_it_again() {
        let (_temp, conn) = defined_store();
        let base = base().unwrap();
        let runtime = CountingRuntime::default();
        assert_eq!(launch(&conn).unwrap(), Launch::Absent);
        assert_eq!(stored(&conn).unwrap(), Stored::None);
        assert_eq!(
            reissue(&conn, &runtime, at(base, RUN_NOW)).unwrap(),
            Answer::Launched
        );
        assert_eq!(launch(&conn).unwrap(), Launch::Owned);
        assert_eq!(
            reissue(&conn, &runtime, at(base, REISSUE)).unwrap(),
            Answer::Replayed
        );
        assert_eq!(runtime.launches(), 1);
    }

    #[test]
    fn the_claim_is_whole_or_a_state_no_boundary_leaves() {
        let left: Vec<(Launch, Stored)> = Boundary::ALL
            .iter()
            .map(|boundary| {
                let expected = expected_for(*boundary);
                (expected.launch_after_exit, expected.stored_after_exit)
            })
            .collect();
        let (_temp, conn) = defined_store();
        let now = at(base().unwrap(), RUN_NOW);
        // The claim transaction's writes in its order: the occurrence, then
        // the answer.
        conn.execute_batch("BEGIN").unwrap();
        assert!(crate::automations::occurrences::insert_claimed_occurrence(
            &conn,
            &occurrence_id(),
            AUTOMATION_ID,
            "manual",
            60,
            now,
        )
        .unwrap());
        let half = (launch(&conn).unwrap(), stored(&conn).unwrap());
        assert_eq!(half, (Launch::Claimed, Stored::None));
        assert!(!left.contains(&half), "{half:?}");
        conn.execute(
            "INSERT INTO automation_command_adoptions
                (adoption_key, command, automation_id, request_digest, outcome, revision,
                 response_json, adopted_at)
             VALUES (?1, 'occurrence.runNow.v1', ?2, 'digest', 'committed', NULL, ?3, ?4)",
            [
                ADOPTION_KEY,
                AUTOMATION_ID,
                &json!({"outcome": "committed", "result": {"occurrenceId": occurrence_id()}})
                    .to_string(),
                &iso(now),
            ],
        )
        .unwrap();
        assert_eq!(
            (launch(&conn).unwrap(), stored(&conn).unwrap()),
            (Launch::Claimed, Stored::Claimed)
        );
        conn.execute_batch("ROLLBACK").unwrap();
        assert_eq!(
            (launch(&conn).unwrap(), stored(&conn).unwrap()),
            (Launch::Absent, Stored::None)
        );
    }

    #[test]
    fn launch_is_whole_or_mixed() {
        let (_temp, conn) = defined_store();
        let base = base().unwrap();
        let runtime = CountingRuntime::default();
        assert_eq!(
            reissue(&conn, &runtime, at(base, RUN_NOW)).unwrap(),
            Answer::Launched
        );
        assert_eq!(launch(&conn).unwrap(), Launch::Owned);
        // What an ownership publication split after each write would leave.
        conn.execute_batch("BEGIN").unwrap();
        conn.execute(
            "UPDATE automation_attempts SET state = 'dispatching', session_id = NULL",
            [],
        )
        .unwrap();
        assert_eq!(launch(&conn).unwrap(), Launch::Mixed);
        conn.execute("UPDATE sessions SET status = 'created'", [])
            .unwrap();
        assert_eq!(launch(&conn).unwrap(), Launch::Mixed);
        conn.execute("UPDATE automation_occurrences SET state = 'claimed'", [])
            .unwrap();
        assert_eq!(launch(&conn).unwrap(), Launch::Launching);
        // A launch the daemon's lease holds is not the owner's.
        conn.execute(
            "UPDATE automation_occurrences SET lease_owner = 'daemon'",
            [],
        )
        .unwrap();
        assert_eq!(launch(&conn).unwrap(), Launch::Mixed);
        conn.execute_batch("ROLLBACK").unwrap();
        // What a restart's orphaning, then a deadline settlement split after
        // each write, would leave.
        let deadline = iso(at(base, DEADLINE_PASS));
        conn.execute_batch("BEGIN").unwrap();
        conn.execute("UPDATE sessions SET status = 'orphaned'", [])
            .unwrap();
        assert_eq!(launch(&conn).unwrap(), Launch::Orphaned);
        conn.execute(
            "UPDATE automation_attempts
             SET state = 'timed_out', failure_class = 'timeout', state_reason = ?1,
                 settled_at = ?2",
            [TIMED_OUT, deadline.as_str()],
        )
        .unwrap();
        assert_eq!(launch(&conn).unwrap(), Launch::Mixed);
        conn.execute(
            "UPDATE automation_runs SET status = 'failed', finished_at = ?1",
            [deadline.as_str()],
        )
        .unwrap();
        assert_eq!(launch(&conn).unwrap(), Launch::Mixed);
        conn.execute(
            "UPDATE automation_occurrences
             SET state = 'failed', lease_owner = NULL, lease_expires_at = NULL,
                 failure_reason = ?1",
            [TIMED_OUT],
        )
        .unwrap();
        assert_eq!(launch(&conn).unwrap(), Launch::Mixed);
        conn.execute("UPDATE sessions SET status = 'killed'", [])
            .unwrap();
        assert_eq!(launch(&conn).unwrap(), Launch::TimedOut);
        conn.execute_batch("ROLLBACK").unwrap();
        // A run that has released its session is not this launch.
        conn.execute_batch("BEGIN").unwrap();
        conn.execute("UPDATE automation_runs SET session_id = NULL", [])
            .unwrap();
        assert_eq!(launch(&conn).unwrap(), Launch::Mixed);
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
        assert_eq!(launch(&conn).unwrap(), Launch::Mixed);
        conn.execute_batch("ROLLBACK").unwrap();
        // A result reservation is never the flat request's answer.
        assert_eq!(stored(&conn).unwrap(), Stored::Claimed);
        conn.execute_batch("BEGIN").unwrap();
        conn.execute(
            "INSERT INTO automation_command_reservations
                (adoption_key, request_digest, command, reserved_at)
             VALUES ('adopt:conformance:run-now-crash:result', 'digest',
                     'occurrence.runNow.result.v1', ?1)",
            [iso(at(base, RUN_NOW))],
        )
        .unwrap();
        assert_eq!(stored(&conn).unwrap(), Stored::Other);
        conn.execute_batch("ROLLBACK").unwrap();
        assert_eq!(stored(&conn).unwrap(), Stored::Claimed);
        assert_eq!(launch(&conn).unwrap(), Launch::Owned);
    }

    #[test]
    fn worker_refuses_invalid_or_weakened_input() {
        let mut case = vectors()["cases"][0].clone();
        case["storePath"] = json!("private-input");
        assert_eq!(worker(&case).unwrap_err(), INVALID);
        let mut case = vectors()["cases"][5].clone();
        case["expected"]["stops"] = json!(1);
        assert_eq!(worker(&case).unwrap_err(), INVALID);
        let mut case = vectors()["cases"][8].clone();
        case["boundary"] = json!("after_claim");
        assert_eq!(worker(&case).unwrap_err(), INVALID);
    }
}
