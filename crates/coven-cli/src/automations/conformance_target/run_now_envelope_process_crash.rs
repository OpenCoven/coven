//! Disposable run-now envelope crash fixtures: the owner runs a routine now
//! through the command envelope, whose claim also reserves a result key and
//! whose dispatch result is retained as a second immutable adoption. A daemon
//! worker is killed while the command claims, launches or retains that result.
//! The parent restarts the daemon's scheduler, the owner sends the same request
//! again, and passes after the run's deadline and the manual claim's lease must
//! not launch it again. A replay with no retained result must be refused as
//! ambiguous. Every launch goes through a no-op runtime; no runtime process
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
use super::result_delivery_recovery::at;
use super::run_launch_process_crash::{pass, restart, CountingRuntime, REFUSED};
use super::run_now_process_crash::{
    base, end_relaunched_session, launch, launch_path, occurrence_id, snapshot, store_definition,
    synthetic_definition, InitialOutcome, Launch, ADOPTION_KEY, AUTOMATION_ID, DEADLINE_PASS,
    LEASE_PASS, PASS, REISSUE, REPLAY, RESEND, RESTART, RUN_NOW, SESSION_END,
};
use super::valid_case_id;
use crate::automations::contract::canonical_json::{canonicalize, sha256_hex};
use crate::automations::contract::error::{ErrorCode, ErrorEnvelope};
use crate::automations::leadership::SchedulerLeadership;
use crate::automations::run_now::{
    execute_run_now_envelope, result_adoption_key, RunNowExecution, RESULT_COMMAND, RUN_NOW_ACTION,
};

const INVALID: &str = "conformance vector is invalid";
const FAILED: &str = "conformance suite execution failed";
const SCHEMA_VERSION: &str = "coven.automations.run-now-envelope-process-crash-recovery-vectors.v1";
const WORKER: &str = "run-now-envelope-crash-worker";
const HOME_PREFIX: &str = "run-now-envelope-crash-";
/// The command's refusal of a replay that finds a claim and no retained
/// result.
const AMBIGUOUS: &str = "runNow dispatch has no retained result; replay cannot launch again";

/// Where the worker can stop: inside the claim transaction and after its
/// commit, after the launch's and ownership's commits, inside the result's
/// retention and after its commit, and the uninterrupted control.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
enum Boundary {
    AfterReservation,
    AfterAdoption,
    AfterClaimCommit,
    AfterLaunchCommit,
    AfterOwnershipCommit,
    AfterConsumed,
    AfterResult,
    AfterResultCommit,
    None,
}

impl Boundary {
    const ALL: [Self; 9] = [
        Self::AfterReservation,
        Self::AfterAdoption,
        Self::AfterClaimCommit,
        Self::AfterLaunchCommit,
        Self::AfterOwnershipCommit,
        Self::AfterConsumed,
        Self::AfterResult,
        Self::AfterResultCommit,
        Self::None,
    ];

    /// What the store has committed when the worker stops.
    fn committed(self) -> (Launch, Stored) {
        match self {
            Self::AfterReservation | Self::AfterAdoption => (Launch::Absent, Stored::None),
            Self::AfterClaimCommit => (Launch::Claimed, Stored::Pending),
            Self::AfterLaunchCommit => (Launch::Launching, Stored::Pending),
            Self::AfterOwnershipCommit | Self::AfterConsumed | Self::AfterResult => {
                (Launch::Owned, Stored::Pending)
            }
            Self::AfterResultCommit | Self::None => (Launch::Owned, Stored::Retained),
        }
    }

    /// The worker stops inside a write transaction.
    fn inside_transaction(self) -> bool {
        matches!(
            self,
            Self::AfterReservation | Self::AfterAdoption | Self::AfterConsumed | Self::AfterResult
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
    stored_after_reissue: Stored,
    launch_after_pass: Launch,
    launch_after_deadline: Launch,
    launch_after_lease: Launch,
    resend: Answer,
    stored_after_resend: Stored,
    launches: u64,
    stops: u64,
    integrity_ok: bool,
    replay_is_noop: bool,
}

/// The owner's stored answer: the store's command adoptions and reservations.
#[derive(Debug, Deserialize, Serialize, PartialEq, Clone, Copy)]
#[serde(rename_all = "snake_case")]
enum Stored {
    /// No answer and no reservation.
    None,
    /// The owner's claim, naming the manual occurrence the key names and its
    /// result key, with that key still reserved for the request and no
    /// result.
    Pending,
    /// The owner's claim, and the result retained under its result key for
    /// the same request, with no reservation. The result is the launch the
    /// store holds: the occurrence and its run `running`, on the first
    /// attempt's session.
    Retained,
    /// Anything else.
    Other,
}

/// What the owner's re-sent request answered, checked against the store.
#[derive(Debug, Deserialize, Serialize, PartialEq, Clone, Copy)]
#[serde(rename_all = "snake_case")]
enum Answer {
    /// A fresh claim and launch with exactly one launch, answering the result
    /// it retained.
    Launched,
    /// The `AMBIGUOUS_RETRY_FORBIDDEN` refusal of a claim with no retained
    /// result, without a write or a launch.
    Ambiguous,
    /// The retained result, without a write or a launch.
    Replayed,
    /// Anything else.
    Mismatched,
}

/// What each boundary must produce. A vector restates it, so it cannot weaken
/// it.
fn expected_for(boundary: Boundary) -> Expected {
    let (committed, stored) = boundary.committed();
    // The launch recovers as the flat request's does. The answer depends only
    // on whether a result was retained: a request that finds nothing claims,
    // launches and retains afresh, and a claim with no result is never
    // answered again, whatever its launch became.
    let (after_restart, after_reissue, after_pass, after_deadline, after_lease) =
        launch_path(committed);
    let (reissue, stored_after_reissue) = match stored {
        Stored::None => (Answer::Launched, Stored::Retained),
        Stored::Pending => (Answer::Ambiguous, Stored::Pending),
        Stored::Retained | Stored::Other => (Answer::Replayed, Stored::Retained),
    };
    Expected {
        initial_outcome: if boundary == Boundary::None {
            InitialOutcome::Answered
        } else {
            InitialOutcome::Terminated
        },
        launch_after_exit: committed,
        stored_after_exit: stored,
        launch_after_restart: after_restart,
        restart_is_noop: after_restart == committed,
        reissue,
        launch_after_reissue: after_reissue,
        stored_after_reissue,
        launch_after_pass: after_pass,
        launch_after_deadline: after_deadline,
        launch_after_lease: after_lease,
        resend: if reissue == Answer::Ambiguous {
            Answer::Ambiguous
        } else {
            Answer::Replayed
        },
        stored_after_resend: stored_after_reissue,
        launches: u64::from(committed == Launch::Absent),
        stops: u64::from(after_deadline == Launch::TimedOut),
        integrity_ok: true,
        replay_is_noop: true,
    }
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

/// Where the worker parks inside SQLite. The claim transaction inserts the
/// occurrence, the result key's reservation and the owner's answer, in that
/// order. The launch and ownership commit as the flat request's do. The
/// retention consumes the reservation, then inserts the result. Nothing else
/// in the command commits.
fn pause_at(boundary: Boundary) -> Option<Pause<'static>> {
    match boundary {
        Boundary::AfterReservation => Some(Pause::After(
            "AFTER INSERT ON main.automation_command_reservations
             WHEN NEW.command = 'occurrence.runNow.result.v1'",
        )),
        Boundary::AfterAdoption => Some(Pause::After(
            "AFTER INSERT ON main.automation_command_adoptions
             WHEN NEW.command = 'occurrence.runNow.v1'",
        )),
        Boundary::AfterClaimCommit => Some(Pause::AfterCommit),
        Boundary::AfterLaunchCommit => Some(Pause::AfterCommits(2)),
        Boundary::AfterOwnershipCommit => Some(Pause::AfterCommits(3)),
        Boundary::AfterConsumed => Some(Pause::After(
            "AFTER DELETE ON main.automation_command_reservations
             WHEN OLD.command = 'occurrence.runNow.result.v1'",
        )),
        Boundary::AfterResult => Some(Pause::After(
            "AFTER INSERT ON main.automation_command_adoptions
             WHEN NEW.command = 'occurrence.runNow.result.v1'",
        )),
        Boundary::AfterResultCommit => Some(Pause::AfterCommits(4)),
        Boundary::None => None,
    }
}

/// The owner's request, identical each time it is sent.
fn request() -> Value {
    json!({"action": RUN_NOW_ACTION, "adoptionKey": ADOPTION_KEY,
        "automationId": AUTOMATION_ID})
}

/// The digest the envelope stores for the request: its canonical JSON under
/// the envelope's own domain, so a flat request under the same key never
/// matches it.
fn request_digest() -> Result<String, &'static str> {
    let mut canonical = b"coven.run-now-envelope.v1\0".to_vec();
    canonical.append(&mut canonicalize(&request()).map_err(|_| FAILED)?);
    Ok(sha256_hex(&canonical))
}

/// The command's refusal of a replay that finds a claim and no retained
/// result, exactly as the command builds it.
fn ambiguous() -> Result<ErrorEnvelope, &'static str> {
    ErrorEnvelope::try_new(ErrorCode::AmbiguousRetryForbidden, AMBIGUOUS, false).map_err(|_| FAILED)
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
    let answer = execute_run_now_envelope(
        &conn,
        &crate::api::NoopSessionRuntime,
        request(),
        run_now_at,
    )
    .map_err(|_| FAILED)?;
    let answered = match &answer {
        RunNowExecution::Success {
            payload,
            replayed: false,
        } => Some(payload) == retained(&conn)?.as_ref(),
        _ => false,
    };
    if case.boundary != Boundary::None || !answered || launch(&conn)? != Launch::Owned {
        return Err(FAILED);
    }
    // Exits like a daemon that dies after answering: without the graceful
    // shutdown's reconciliation or the release of its scheduler authority.
    drop(conn);
    drop(leadership);
    announce(&json!({"state":"answered", "boundary":case.boundary, "relativeHome":relative_home}))
}

/// One command adoption, as the classes read it.
struct Adoption {
    key: String,
    command: String,
    digest: String,
    automation_id: Option<String>,
    outcome: String,
    response: Value,
}

/// Classifies the owner's stored answer.
fn stored(conn: &Connection) -> Result<Stored, &'static str> {
    let adoptions: Vec<Adoption> = conn
        .prepare(
            "SELECT adoption_key, command, request_digest, automation_id, outcome,
                    response_json
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
                        row.get::<_, String>(5)?,
                    ))
                })?
                .collect::<Result<Vec<_>, _>>()
        })
        .map_err(|_| FAILED)?
        .into_iter()
        .map(
            |(key, command, digest, automation_id, outcome, response)| -> Result<_, &'static str> {
                Ok(Adoption {
                    key,
                    command,
                    digest,
                    automation_id,
                    outcome,
                    response: serde_json::from_str(&response).map_err(|_| FAILED)?,
                })
            },
        )
        .collect::<Result<_, _>>()?;
    let reservations: Vec<(String, String, String)> = conn
        .prepare(
            "SELECT adoption_key, command, request_digest FROM automation_command_reservations",
        )
        .and_then(|mut statement| {
            statement
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
                .collect()
        })
        .map_err(|_| FAILED)?;
    if adoptions.is_empty() && reservations.is_empty() {
        return Ok(Stored::None);
    }
    let result_key = result_adoption_key(ADOPTION_KEY);
    let Some(claim) = adoptions
        .iter()
        .find(|adoption| adoption.key == ADOPTION_KEY)
    else {
        return Ok(Stored::Other);
    };
    if claim.command != "occurrence.runNow.v1"
        || claim.digest != request_digest()?
        || claim.automation_id.as_deref() != Some(AUTOMATION_ID)
        || claim.outcome != "committed"
        || claim.response
            != json!({"outcome": "committed", "result": {"occurrenceId": occurrence_id()},
                "dispatchAdoptionKey": result_key})
    {
        return Ok(Stored::Other);
    }
    let digest = claim.digest.as_str();
    Ok(match (adoptions.as_slice(), reservations.as_slice()) {
        ([_], [(key, command, reserved)])
            if *key == result_key && command == RESULT_COMMAND && reserved == digest =>
        {
            Stored::Pending
        }
        ([_, _], []) => {
            let Some(result) = adoptions.iter().find(|adoption| adoption.key == result_key) else {
                return Ok(Stored::Other);
            };
            let Some(launched) = launched_result(conn)? else {
                return Ok(Stored::Other);
            };
            if result.command == RESULT_COMMAND
                && result.digest == digest
                && result.automation_id.as_deref() == Some(AUTOMATION_ID)
                && result.outcome == "committed"
                && result.response == json!({"outcome": "committed", "result": launched})
            {
                Stored::Retained
            } else {
                Stored::Other
            }
        }
        _ => Stored::Other,
    })
}

/// The result a launch that published ownership answers: the occurrence and
/// its run `running`, with the run's id and the first attempt's session.
fn launched_result(conn: &Connection) -> Result<Option<Value>, &'static str> {
    let run: Option<(String, Option<String>)> = conn
        .query_row(
            "SELECT r.id, a.session_id
             FROM automation_runs AS r
             JOIN automation_attempts AS a ON a.run_id = r.id
             WHERE r.occurrence_id = ?1 AND a.attempt_number = 1",
            [occurrence_id()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|_| FAILED)?;
    Ok(run.map(|(run_id, session_id)| {
        json!({"occurrenceId": occurrence_id(), "automationId": AUTOMATION_ID,
            "occurrenceState": "running", "runId": run_id, "status": "running",
            "sessionId": session_id})
    }))
}

/// The retained result's payload, when the store holds one.
fn retained(conn: &Connection) -> Result<Option<Value>, &'static str> {
    if stored(conn)? != Stored::Retained {
        return Ok(None);
    }
    launched_result(conn)
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
    let execution = execute_run_now_envelope(conn, runtime, request(), now).map_err(|_| REFUSED)?;
    let wrote = snapshot(conn)? != before;
    let launched = runtime.launches() - launches;
    let retained = retained(conn)?;
    Ok(match execution {
        RunNowExecution::Success {
            payload,
            replayed: false,
        } if wrote && launched == 1 && Some(&payload) == retained.as_ref() => Answer::Launched,
        RunNowExecution::Success {
            payload,
            replayed: true,
        } if !wrote && launched == 0 && Some(&payload) == retained.as_ref() => Answer::Replayed,
        RunNowExecution::Rejected(error)
            if !wrote
                && launched == 0
                && error == ambiguous()?
                && stored(conn)? == Stored::Pending =>
        {
            Answer::Ambiguous
        }
        _ => Answer::Mismatched,
    })
}

/// What the restarted daemon, its passes and the re-sent requests did with
/// the store the worker left.
struct Recovery {
    launch_after_restart: Launch,
    restart_is_noop: bool,
    reissue: Answer,
    launch_after_reissue: Launch,
    stored_after_reissue: Stored,
    launch_after_pass: Launch,
    launch_after_deadline: Launch,
    launch_after_lease: Launch,
    resend: Answer,
    stored_after_resend: Stored,
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
    let stored_after_reissue = stored(&conn)?;
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
    let stored_after_resend = stored(&conn)?;
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
        stored_after_reissue,
        launch_after_pass,
        launch_after_deadline,
        launch_after_lease,
        resend: resent,
        stored_after_resend,
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
            stored_after_reissue: recovery.stored_after_reissue,
            launch_after_pass: recovery.launch_after_pass,
            launch_after_deadline: recovery.launch_after_deadline,
            launch_after_lease: recovery.launch_after_lease,
            resend: recovery.resend,
            stored_after_resend: recovery.stored_after_resend,
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
            stored_after_reissue: Stored::Other,
            launch_after_pass: Launch::Mixed,
            launch_after_deadline: Launch::Mixed,
            launch_after_lease: Launch::Mixed,
            resend: Answer::Mismatched,
            stored_after_resend: Stored::Other,
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
            "storedAfterReissue": observed.stored_after_reissue,
            "launchAfterPass": observed.launch_after_pass,
            "launchAfterDeadline": observed.launch_after_deadline,
            "launchAfterLease": observed.launch_after_lease,
            "resend": observed.resend,
            "storedAfterResend": observed.stored_after_resend,
            "launches": observed.launches,
            "stops": observed.stops,
            "replayIsNoop": observed.replay_is_noop,
        }),
    ))
}

#[cfg(test)]
mod tests {
    use super::super::result_delivery_recovery::iso;
    use super::*;

    fn vectors() -> Value {
        serde_json::from_str(include_str!(
            "../../../../../conformance/automations/runner/run-now-envelope-process-crash-recovery.vectors.json"
        ))
        .unwrap()
    }

    #[test]
    fn run_now_envelope_crash_vectors_restate_each_boundary_outcome() {
        let vectors: VectorSet = serde_json::from_value(vectors()).unwrap();
        assert_eq!(vectors.schema_version, SCHEMA_VERSION);
        for case in &vectors.cases {
            validate(case).unwrap();
        }
        let boundaries: Vec<_> = vectors.cases.iter().map(|case| case.boundary).collect();
        assert_eq!(boundaries, Boundary::ALL);
    }

    #[test]
    fn run_now_envelope_crash_vectors_require_each_boundary_and_the_control() {
        let original = vectors();
        let mut missing = original.clone();
        missing["cases"].as_array_mut().unwrap().pop();
        assert_eq!(evaluate(&missing).unwrap_err(), INVALID);
        // A second control under its own case id, replacing a boundary.
        let mut duplicate = original.clone();
        let mut control = duplicate["cases"][8].clone();
        control["caseId"] = json!("second-uninterrupted-run-now-envelope-control");
        duplicate["cases"][0] = control;
        assert_eq!(evaluate(&duplicate).unwrap_err(), INVALID);
        let mut duplicate_id = original;
        duplicate_id["cases"][1]["caseId"] = duplicate_id["cases"][0]["caseId"].clone();
        assert_eq!(evaluate(&duplicate_id).unwrap_err(), INVALID);
    }

    #[test]
    fn run_now_envelope_crash_vectors_refuse_malformed_or_weakened_inputs() {
        let original = vectors();
        for (case, field, value) in [
            (0, "reissue", json!("ambiguous")),
            (0, "storedAfterReissue", json!("pending")),
            (1, "launchAfterExit", json!("claimed")),
            (1, "launches", json!(0)),
            (2, "reissue", json!("launched")),
            (2, "resend", json!("replayed")),
            (2, "launchAfterLease", json!("claimed")),
            (3, "storedAfterExit", json!("retained")),
            (3, "stops", json!(0)),
            (4, "launchAfterRestart", json!("owned")),
            (4, "reissue", json!("replayed")),
            (5, "storedAfterExit", json!("other")),
            (6, "resend", json!("launched")),
            (7, "storedAfterExit", json!("pending")),
            (7, "reissue", json!("ambiguous")),
            (8, "initialOutcome", json!("terminated")),
            (8, "storedAfterResend", json!("pending")),
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
            ("boundary", json!("after_claim")),
            ("caseId", json!("")),
        ] {
            let mut invalid = original.clone();
            invalid["cases"][0][field] = value;
            assert_eq!(evaluate(&invalid).unwrap_err(), INVALID, "{field}");
        }
        let mut invalid = original;
        invalid["schemaVersion"] =
            json!("coven.automations.run-now-process-crash-recovery-vectors.v1");
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

    /// The owner's request, claimed and launched, whose result retention
    /// failed: the claim, the launch and ownership committed, and the result
    /// key still reserved.
    fn pending_store(runtime: &CountingRuntime) -> (tempfile::TempDir, Connection) {
        let (temp, conn) = defined_store();
        conn.execute_batch(
            "CREATE TEMP TRIGGER refuse_retention
             BEFORE INSERT ON main.automation_command_adoptions
             WHEN NEW.command = 'occurrence.runNow.result.v1'
             BEGIN SELECT RAISE(ABORT, 'retention refused'); END;",
        )
        .unwrap();
        assert!(
            execute_run_now_envelope(&conn, runtime, request(), at(base().unwrap(), RUN_NOW))
                .is_err()
        );
        conn.execute_batch("DROP TRIGGER temp.refuse_retention")
            .unwrap();
        (temp, conn)
    }

    #[test]
    fn the_owner_runs_a_routine_once_and_a_replay_repeats_its_retained_result() {
        let (_temp, conn) = defined_store();
        let base = base().unwrap();
        let runtime = CountingRuntime::default();
        assert_eq!(stored(&conn).unwrap(), Stored::None);
        assert_eq!(
            reissue(&conn, &runtime, at(base, RUN_NOW)).unwrap(),
            Answer::Launched
        );
        assert_eq!(
            (launch(&conn).unwrap(), stored(&conn).unwrap()),
            (Launch::Owned, Stored::Retained)
        );
        assert_eq!(
            reissue(&conn, &runtime, at(base, REISSUE)).unwrap(),
            Answer::Replayed
        );
        // The retained result stays what the launch answered after the run
        // ends.
        end_relaunched_session(&conn, at(base, SESSION_END)).unwrap();
        assert_eq!(stored(&conn).unwrap(), Stored::Retained);
        assert_eq!(runtime.launches(), 1);
    }

    #[test]
    fn a_claim_with_no_retained_result_is_refused_as_ambiguous_and_never_launched_again() {
        let runtime = CountingRuntime::default();
        let (_temp, conn) = pending_store(&runtime);
        assert_eq!(runtime.launches(), 1);
        assert_eq!(
            (launch(&conn).unwrap(), stored(&conn).unwrap()),
            (Launch::Owned, Stored::Pending)
        );
        assert_eq!(
            reissue(&conn, &runtime, at(base().unwrap(), REISSUE)).unwrap(),
            Answer::Ambiguous
        );
        assert_eq!(runtime.launches(), 1);
    }

    #[test]
    fn each_transaction_is_whole_or_a_state_no_boundary_leaves() {
        let left: Vec<(Launch, Stored)> = Boundary::ALL
            .iter()
            .map(|boundary| {
                let expected = expected_for(*boundary);
                (expected.launch_after_exit, expected.stored_after_exit)
            })
            .collect();
        let now = at(base().unwrap(), RUN_NOW);
        // The claim's writes in production's order: the occurrence, the
        // result key's reservation, then the owner's answer.
        let (_temp, conn) = defined_store();
        let result_key = result_adoption_key(ADOPTION_KEY);
        let digest = request_digest().unwrap();
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
        let mut halves = vec![(launch(&conn).unwrap(), stored(&conn).unwrap())];
        conn.execute(
            "INSERT INTO automation_command_reservations
                (adoption_key, request_digest, command, reserved_at)
             VALUES (?1, ?2, ?3, ?4)",
            [result_key.as_str(), &digest, RESULT_COMMAND, &iso(now)],
        )
        .unwrap();
        halves.push((launch(&conn).unwrap(), stored(&conn).unwrap()));
        assert_eq!(
            halves,
            [
                (Launch::Claimed, Stored::None),
                (Launch::Claimed, Stored::Other)
            ]
        );
        for half in &halves {
            assert!(!left.contains(half), "{half:?}");
        }
        conn.execute(
            "INSERT INTO automation_command_adoptions
                (adoption_key, command, automation_id, request_digest, outcome, revision,
                 response_json, adopted_at)
             VALUES (?1, 'occurrence.runNow.v1', ?2, ?3, 'committed', NULL, ?4, ?5)",
            [
                ADOPTION_KEY,
                AUTOMATION_ID,
                &digest,
                &json!({"outcome": "committed", "result": {"occurrenceId": occurrence_id()},
                    "dispatchAdoptionKey": result_key})
                .to_string(),
                &iso(now),
            ],
        )
        .unwrap();
        assert_eq!(
            (launch(&conn).unwrap(), stored(&conn).unwrap()),
            (Launch::Claimed, Stored::Pending)
        );
        conn.execute_batch("ROLLBACK").unwrap();
        assert_eq!(
            (launch(&conn).unwrap(), stored(&conn).unwrap()),
            (Launch::Absent, Stored::None)
        );
        drop(conn);
        // The retention's writes in production's order: the reservation
        // consumed, then the result.
        let runtime = CountingRuntime::default();
        let (_temp, conn) = pending_store(&runtime);
        conn.execute_batch("BEGIN").unwrap();
        conn.execute("DELETE FROM automation_command_reservations", [])
            .unwrap();
        let half = (launch(&conn).unwrap(), stored(&conn).unwrap());
        assert_eq!(half, (Launch::Owned, Stored::Other));
        assert!(!left.contains(&half), "{half:?}");
        let result = json!({"outcome": "committed",
            "result": launched_result(&conn).unwrap().unwrap()});
        conn.execute(
            "INSERT INTO automation_command_adoptions
                (adoption_key, command, automation_id, request_digest, outcome, revision,
                 response_json, adopted_at)
             VALUES (?1, ?2, ?3, ?4, 'committed', NULL, ?5, ?6)",
            [
                result_key.as_str(),
                RESULT_COMMAND,
                AUTOMATION_ID,
                &digest,
                &result.to_string(),
                &iso(now),
            ],
        )
        .unwrap();
        assert_eq!(
            (launch(&conn).unwrap(), stored(&conn).unwrap()),
            (Launch::Owned, Stored::Retained)
        );
        conn.execute_batch("ROLLBACK").unwrap();
        assert_eq!(
            (launch(&conn).unwrap(), stored(&conn).unwrap()),
            (Launch::Owned, Stored::Pending)
        );
    }

    #[test]
    fn worker_refuses_invalid_or_weakened_input() {
        let mut case = vectors()["cases"][0].clone();
        case["storePath"] = json!("private-input");
        assert_eq!(worker(&case).unwrap_err(), INVALID);
        let mut case = vectors()["cases"][2].clone();
        case["expected"]["reissue"] = json!("launched");
        assert_eq!(worker(&case).unwrap_err(), INVALID);
        let mut case = vectors()["cases"][8].clone();
        case["boundary"] = json!("after_reservation");
        assert_eq!(worker(&case).unwrap_err(), INVALID);
    }
}
