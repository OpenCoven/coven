//! Disposable scheduler-tick crash fixtures: a daemon worker is killed while
//! its fenced tick plans a due daily slot, advances the planning cursor or
//! claims the slot. The parent restarts the daemon before or after the next
//! day's slot, and the restarted scheduler must plan each slot once and launch
//! exactly one, the one the misfire policy selects. Every launch goes through a
//! no-op runtime; no runtime process starts.
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
use crate::automations::definition::RoutineDefinition;
use crate::automations::leadership::SchedulerLeadership;

const INVALID: &str = "conformance vector is invalid";
const FAILED: &str = "conformance suite execution failed";
const SCHEMA_VERSION: &str = "coven.automations.tick-process-crash-recovery-vectors.v1";
const WORKER: &str = "tick-crash-worker";
const HOME_PREFIX: &str = "tick-crash-";
const AUTOMATION_ID: &str = "conformance-tick-crash";
/// The definition predates the slot the worker plans, so that slot is the
/// first one due.
const DEFINED_AT: &str = "2099-10-19T09:00:00.000Z";
/// The slot the worker plans and claims, and the next day's.
const SLOT: &str = "2099-10-20T09:00:00.000Z";
const NEXT_SLOT: &str = "2099-10-21T09:00:00.000Z";
/// Seconds after `BASE`, the slot: the worker ticks 30 seconds after it, and
/// the parent restarts the daemon a minute after it or a minute after the next
/// day's slot. Seconds after the restart: the launched session ends cleanly, a
/// pass settles its run, and a further pass changes nothing.
const TICK: i64 = 30;
const PROMPT_RESTART: i64 = 60;
const LATE_RESTART: i64 = 86_460;
const SESSION_END: i64 = 30;
const PASS: i64 = 60;
const REPLAY_PASS: i64 = 120;
/// A missed slot's reason once the misfire policy supersedes it.
const SUPERSEDED: &str = "superseded by latest misfire policy";

/// When the parent restarts the daemon: before the next day's slot, or after
/// it.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
enum Scene {
    Prompt,
    Late,
}

impl Scene {
    const ALL: [Self; 2] = [Self::Prompt, Self::Late];

    fn restart_at(self) -> i64 {
        match self {
            Self::Prompt => PROMPT_RESTART,
            Self::Late => LATE_RESTART,
        }
    }
}

/// Where the worker can stop: inside the tick's plan, cursor and claim
/// transactions and after each of their commits, and the uninterrupted
/// control.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
enum Boundary {
    AfterPlan,
    AfterPlanCommit,
    AfterCursor,
    AfterCursorCommit,
    AfterClaim,
    AfterClaimCommit,
    None,
}

impl Boundary {
    const ALL: [Self; 7] = [
        Self::AfterPlan,
        Self::AfterPlanCommit,
        Self::AfterCursor,
        Self::AfterCursorCommit,
        Self::AfterClaim,
        Self::AfterClaimCommit,
        Self::None,
    ];

    /// What the store has committed when the worker stops: the slot and the
    /// planning cursor's revision.
    fn committed(self) -> (Slot, u64) {
        match self {
            Self::AfterPlan => (Slot::Absent, 0),
            Self::AfterPlanCommit | Self::AfterCursor => (Slot::Planned, 0),
            Self::AfterCursorCommit | Self::AfterClaim => (Slot::Planned, 1),
            Self::AfterClaimCommit | Self::None => (Slot::Claimed, 1),
        }
    }

    /// The worker stops inside a write transaction.
    fn inside_transaction(self) -> bool {
        matches!(self, Self::AfterPlan | Self::AfterCursor | Self::AfterClaim)
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
    slot_after_exit: Slot,
    cursor_after_exit: u64,
    slot_after_restart: Slot,
    next_slot_after_restart: Slot,
    claims: u64,
    restart_launches: u64,
    slot_after_pass: Slot,
    next_slot_after_pass: Slot,
    launches: u64,
    stops: u64,
    integrity_ok: bool,
    replay_is_noop: bool,
}

#[derive(Debug, Deserialize, Serialize, PartialEq, Clone, Copy)]
#[serde(rename_all = "snake_case")]
enum InitialOutcome {
    Terminated,
    Ticked,
}

/// One slot's occurrence, read with its run, attempt and session. Every class
/// but `Absent` is the routine's one `scheduled` occurrence for that slot.
#[derive(Debug, Deserialize, Serialize, PartialEq, Clone, Copy)]
#[serde(rename_all = "snake_case")]
enum Slot {
    /// No occurrence for the slot.
    Absent,
    /// Planned and never claimed: no lease, no scheduler generation, no
    /// claim, no reason and no run.
    Planned,
    /// Claimed once under the `daemon` lease and the current scheduler
    /// generation, with no run.
    Claimed,
    /// Launched under the `daemon` lease and the current scheduler
    /// generation: the occurrence and its run `running`, and the first
    /// attempt `started` on the run's `running` session.
    Owned,
    /// The occurrence, its run and the settled first attempt `succeeded`,
    /// with no lease and the session `completed`.
    Succeeded,
    /// Superseded by the misfire policy while planned and never claimed:
    /// `skipped` with the policy's reason, no lease, no generation, no claim
    /// and no run.
    Skipped,
    /// Superseded by the misfire policy after its unlaunched claim was
    /// restored: as `Skipped`, but claimed once, and never by the current
    /// scheduler.
    SkippedClaim,
    /// Anything else, including a second occurrence for the slot.
    Mixed,
}

/// What each scene and boundary must produce. A vector restates it, so it
/// cannot weaken it.
fn expected_for(scene: Scene, boundary: Boundary) -> Expected {
    let (committed, cursor) = boundary.committed();
    // A restart before the next slot claims and launches this one once: a
    // slot planned afresh or left planned is claimed once, and a claim with no
    // run is restored to planned and claimed again. A restart after the next
    // slot plans, claims and launches that one instead, and supersedes this
    // one if it was planned.
    let (
        slot_after_restart,
        next_slot_after_restart,
        claims,
        slot_after_pass,
        next_slot_after_pass,
    ) = match scene {
        Scene::Prompt => (
            Slot::Owned,
            Slot::Absent,
            if committed == Slot::Claimed { 2 } else { 1 },
            Slot::Succeeded,
            Slot::Absent,
        ),
        Scene::Late => {
            let missed = match committed {
                Slot::Absent => Slot::Absent,
                Slot::Planned => Slot::Skipped,
                _ => Slot::SkippedClaim,
            };
            (missed, Slot::Owned, 1, missed, Slot::Succeeded)
        }
    };
    Expected {
        initial_outcome: if boundary == Boundary::None {
            InitialOutcome::Ticked
        } else {
            InitialOutcome::Terminated
        },
        slot_after_exit: committed,
        cursor_after_exit: cursor,
        slot_after_restart,
        next_slot_after_restart,
        claims,
        restart_launches: 1,
        slot_after_pass,
        next_slot_after_pass,
        launches: 1,
        stops: 0,
        integrity_ok: true,
        replay_is_noop: true,
    }
}

/// A daily routine at 09:00 UTC with a working directory and a 30-minute
/// deadline. The runner launches only a routine with a working directory; a
/// no-op runtime never touches it.
fn synthetic_definition() -> Value {
    json!({"schemaVersion":1,"id":AUTOMATION_ID,
        "name":"Conformance tick crash","status":"ACTIVE",
        "rrule":"FREQ=DAILY;BYHOUR=9","timezone":"utc","misfire":"latest",
        "overlap":"forbid","timeoutMinutes":30,"runtime":"coven-code",
        "prompt":"Run the native tick crash conformance probe.",
        "cwd":"/work/project"})
}

fn validate(case: &Case) -> Result<(), &'static str> {
    if !valid_case_id(&case.case_id)
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

/// Where the worker parks inside SQLite. The tick inserts the planned
/// occurrence, advances the planning cursor and claims the slot, each in its
/// own transaction. Its stale-claim and lease recovery and its misfire
/// supersession change no row for a fresh slot, so they write no WAL frame.
fn pause_at(boundary: Boundary) -> Option<Pause<'static>> {
    match boundary {
        Boundary::AfterPlan => Some(Pause::After("AFTER INSERT ON main.automation_occurrences")),
        Boundary::AfterPlanCommit => Some(Pause::AfterCommit),
        Boundary::AfterCursor => Some(Pause::After(
            "AFTER UPDATE ON main.automation_scheduler_planning_cursor",
        )),
        Boundary::AfterCursorCommit => Some(Pause::AfterCommits(2)),
        Boundary::AfterClaim => Some(Pause::After(
            "AFTER UPDATE OF state ON main.automation_occurrences
             WHEN OLD.state = 'planned' AND NEW.state = 'claimed'",
        )),
        Boundary::AfterClaimCommit => Some(Pause::AfterCommits(3)),
        Boundary::None => None,
    }
}

/// Stores the synthetic routine, defined before the slot it plans.
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

/// The daemon's store as its scheduler finds it before the tick: the routine
/// stored, and the scheduler's leadership taken in a commit of its own.
///
/// Every state update compiles the occurrence transition trigger's insert into
/// the AUTOINCREMENT event feed, and SQLite seeds the feed's `sqlite_sequence`
/// counter the first time such a statement runs, even when it updates no row.
/// A daemon's store has had that counter since its first event. This fresh one
/// would commit it in the tick's stale-claim recovery, ahead of the plan.
/// Seeding it here, with an update that matches no row, leaves the plan, the
/// cursor and the claim as the tick's only commits.
fn prepare(
    home: &Path,
    definition: &Value,
    tick_at: DateTime<Utc>,
) -> Result<(Connection, SchedulerLeadership), &'static str> {
    let conn = crate::store::open_store(&crate::api::store_path(home)).map_err(|_| FAILED)?;
    store_definition(&conn, definition)?;
    let leadership = SchedulerLeadership::acquire(home, &conn, tick_at).map_err(|_| FAILED)?;
    conn.execute(
        "UPDATE automation_occurrences SET state = state WHERE 0",
        [],
    )
    .map_err(|_| FAILED)?;
    Ok((conn, leadership))
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
    let tick_at = at(base()?, TICK);
    let (conn, leadership) = prepare(&home, &case.definition, tick_at)?;
    let fence = leadership.fence();
    if let Some(pause_at) = pause_at(case.boundary) {
        let handshake = json!({"state":"paused", "boundary":case.boundary,
            "relativeHome":relative_home});
        install_pause(&conn, pause_at, handshake)?;
    }
    let tick = crate::automations::occurrences::tick_with_scheduler_fence(&conn, tick_at, &fence)
        .map_err(|_| FAILED)?;
    if case.boundary != Boundary::None
        || tick.planned.len() != 1
        || tick.claimed.len() != 1
        || !tick.failed.is_empty()
        || slots(&conn)? != (Slot::Claimed, Slot::Absent)
    {
        return Err(FAILED);
    }
    // Exits like a daemon that dies after a tick: without the graceful
    // shutdown's reconciliation or leadership release.
    drop(conn);
    drop(leadership);
    announce(&json!({"state":"ticked", "boundary":case.boundary, "relativeHome":relative_home}))
}

/// One slot's occurrence with its run, first attempt and session, as the
/// classes read them.
struct SlotRow {
    state: String,
    kind: String,
    claims: i64,
    lease: String,
    generation: Option<i64>,
    reason: Option<String>,
    runs: i64,
    run: Option<String>,
    run_finished: bool,
    attempt: Option<String>,
    attempt_on_session: bool,
    settled: bool,
    session: Option<String>,
}

/// Classifies the slot scheduled for `scheduled_for`, against the store's
/// current scheduler generation.
fn slot(conn: &Connection, scheduled_for: &str, generation: i64) -> Result<Slot, &'static str> {
    let rows: Vec<SlotRow> = conn
        .prepare(
            "SELECT o.state, o.kind, o.attempt,
                    CASE
                        WHEN o.lease_owner = 'daemon' AND o.lease_expires_at IS NOT NULL
                            THEN 'daemon'
                        WHEN o.lease_owner IS NULL AND o.lease_expires_at IS NULL THEN 'none'
                        ELSE 'other'
                    END,
                    o.scheduler_generation, o.failure_reason,
                    (SELECT COUNT(*) FROM automation_runs WHERE occurrence_id = o.id),
                    r.status, r.finished_at IS NOT NULL, a.state,
                    a.session_id IS NOT NULL AND a.session_id IS r.session_id,
                    a.settled_at IS NOT NULL, s.status
             FROM automation_occurrences AS o
             LEFT JOIN automation_runs AS r ON r.occurrence_id = o.id
             LEFT JOIN automation_attempts AS a
                 ON a.run_id = r.id AND a.occurrence_id = o.id AND a.attempt_number = 1
             LEFT JOIN sessions AS s ON s.id = r.session_id
             WHERE o.automation_id = ?1 AND o.scheduled_for = ?2",
        )
        .and_then(|mut statement| {
            statement
                .query_map([AUTOMATION_ID, scheduled_for], |row| {
                    Ok(SlotRow {
                        state: row.get(0)?,
                        kind: row.get(1)?,
                        claims: row.get(2)?,
                        lease: row.get(3)?,
                        generation: row.get(4)?,
                        reason: row.get(5)?,
                        runs: row.get(6)?,
                        run: row.get(7)?,
                        run_finished: row.get(8)?,
                        attempt: row.get(9)?,
                        attempt_on_session: row.get(10)?,
                        settled: row.get(11)?,
                        session: row.get(12)?,
                    })
                })?
                .collect()
        })
        .map_err(|_| FAILED)?;
    let [row] = rows.as_slice() else {
        return Ok(if rows.is_empty() {
            Slot::Absent
        } else {
            Slot::Mixed
        });
    };
    if row.kind != "scheduled" {
        return Ok(Slot::Mixed);
    }
    let current = row.generation == Some(generation);
    let unclaimed = row.claims == 0 && row.generation.is_none();
    let launch = (
        row.run.as_deref(),
        row.run_finished,
        row.attempt.as_deref(),
        row.attempt_on_session,
        row.settled,
        row.session.as_deref(),
    );
    Ok(
        match (
            row.state.as_str(),
            row.lease.as_str(),
            row.reason.as_deref(),
            row.runs,
        ) {
            ("planned", "none", None, 0) if unclaimed => Slot::Planned,
            ("claimed", "daemon", None, 0) if current && row.claims == 1 => Slot::Claimed,
            ("running", "daemon", None, 1)
                if current
                    && launch
                        == (
                            Some("running"),
                            false,
                            Some("started"),
                            true,
                            false,
                            Some("running"),
                        ) =>
            {
                Slot::Owned
            }
            ("succeeded", "none", None, 1)
                if current
                    && launch
                        == (
                            Some("succeeded"),
                            true,
                            Some("succeeded"),
                            true,
                            true,
                            Some("completed"),
                        ) =>
            {
                Slot::Succeeded
            }
            ("skipped", "none", Some(SUPERSEDED), 0) if unclaimed => Slot::Skipped,
            ("skipped", "none", Some(SUPERSEDED), 0) if row.claims == 1 && !current => {
                Slot::SkippedClaim
            }
            _ => Slot::Mixed,
        },
    )
}

/// Classifies both slots. A store with any other occurrence, or a run,
/// attempt or session that belongs to neither, reads `Mixed` for both.
fn slots(conn: &Connection) -> Result<(Slot, Slot), &'static str> {
    let generation: i64 = conn
        .query_row(
            "SELECT generation FROM automation_scheduler_authority WHERE id = 1",
            [],
            |row| row.get(0),
        )
        .map_err(|_| FAILED)?;
    let pair = (
        slot(conn, SLOT, generation)?,
        slot(conn, NEXT_SLOT, generation)?,
    );
    let counts: [i64; 4] = conn
        .query_row(
            "SELECT
                (SELECT COUNT(*) FROM automation_occurrences),
                (SELECT COUNT(*) FROM automation_runs),
                (SELECT COUNT(*) FROM automation_attempts),
                (SELECT COUNT(*) FROM sessions)",
            [],
            |row| Ok([row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?]),
        )
        .map_err(|_| FAILED)?;
    let present = [pair.0, pair.1]
        .iter()
        .filter(|slot| **slot != Slot::Absent)
        .count();
    let launched = [pair.0, pair.1]
        .iter()
        .filter(|slot| matches!(slot, Slot::Owned | Slot::Succeeded))
        .count();
    let expected = [present, launched, launched, launched].map(|count| count as i64);
    Ok(if counts == expected {
        pair
    } else {
        (Slot::Mixed, Slot::Mixed)
    })
}

/// The planning cursor's revision.
fn cursor(conn: &Connection) -> Result<u64, &'static str> {
    let revision: i64 = conn
        .query_row(
            "SELECT revision FROM automation_scheduler_planning_cursor WHERE id = 1",
            [],
            |row| row.get(0),
        )
        .map_err(|_| FAILED)?;
    u64::try_from(revision).map_err(|_| FAILED)
}

/// The launched slot's claim count.
fn claims(conn: &Connection) -> Result<u64, &'static str> {
    let claims: Option<i64> = conn
        .query_row(
            "SELECT o.attempt FROM automation_occurrences AS o
             JOIN automation_runs AS r ON r.occurrence_id = o.id",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(|_| FAILED)?;
    u64::try_from(claims.unwrap_or(0)).map_err(|_| FAILED)
}

/// Every occurrence, run, attempt, event, session and stop-fence row, for an
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
            "automation_stop_fences",
        ],
    )
}

/// Ends the launched session the way a runtime reports a clean exit.
fn end_session(conn: &Connection, now: DateTime<Utc>) -> Result<(), &'static str> {
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

/// What the restarted daemon and its passes did with the store the worker
/// left.
struct Recovery {
    slot_after_restart: Slot,
    next_slot_after_restart: Slot,
    claims: u64,
    restart_launches: u64,
    slot_after_pass: Slot,
    next_slot_after_pass: Slot,
    launches: u64,
    stops: u64,
    replay_is_noop: bool,
}

fn recover(home: &Path, path: &Path, scene: Scene) -> Result<Recovery, &'static str> {
    let restart_at = at(base()?, scene.restart_at());
    let after = |seconds: i64| restart_at + chrono::Duration::seconds(seconds);
    // One runtime counts every stop and launch for the restart and each pass.
    let runtime = CountingRuntime::default();
    let leadership = restart(home, &runtime, restart_at)?;
    let conn = crate::store::open_store(path).map_err(|_| FAILED)?;
    let (slot_after_restart, next_slot_after_restart) = slots(&conn)?;
    let claims = claims(&conn)?;
    let restart_launches = runtime.launches();
    if matches!(
        (slot_after_restart, next_slot_after_restart),
        (Slot::Owned, _) | (_, Slot::Owned)
    ) {
        end_session(&conn, after(SESSION_END))?;
    }
    drop(conn);
    pass(home, &runtime, &leadership, after(PASS), "deadline")?;
    let conn = crate::store::open_store(path).map_err(|_| FAILED)?;
    let (slot_after_pass, next_slot_after_pass) = slots(&conn)?;
    let launches = runtime.launches();
    let stops = runtime.stops();
    let before_replay = snapshot(&conn)?;
    drop(conn);
    // A further pass must change nothing.
    pass(home, &runtime, &leadership, after(REPLAY_PASS), "deadline")?;
    let conn = crate::store::open_store(path).map_err(|_| FAILED)?;
    let replay_is_noop = snapshot(&conn)? == before_replay
        && runtime.stops() == stops
        && runtime.launches() == launches;
    drop(conn);
    drop(leadership);
    Ok(Recovery {
        slot_after_restart,
        next_slot_after_restart,
        claims,
        restart_launches,
        slot_after_pass,
        next_slot_after_pass,
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
/// worker's write transaction is still open, and what it has committed.
fn observe_pause(path: &Path) -> Result<(bool, Slot, u64), &'static str> {
    let probe = open_raw(path)?;
    let open = write_transaction_open(&probe)?;
    Ok((open, slots(&probe)?.0, cursor(&probe)?))
}

fn case_matches(case: &Case) -> Result<(bool, Value), &'static str> {
    let root = super::scheduler_conformance_tempdir()?;
    crate::daemon::ensure_private_coven_home(root.path()).map_err(|_| FAILED)?;
    let payload = serde_json::to_vec(case).map_err(|_| INVALID)?;
    let (mut process, receiver) = spawn_worker::<Handshake>(WORKER, payload, root.path())?;
    let ready = receiver.recv_timeout(CHILD_GUARD).map_err(|_| FAILED)??;
    let crash = case.boundary != Boundary::None;
    if ready.boundary != case.boundary || ready.state != if crash { "paused" } else { "ticked" } {
        return Err(FAILED);
    }
    let home = worker_home(root.path(), &ready.relative_home, HOME_PREFIX)?;
    let path = crate::api::store_path(&home);
    let pause = crash.then(|| observe_pause(&path)).transpose()?;
    let pause_consistent = pause.is_none_or(|(open, slot, cursor)| {
        open == case.boundary.inside_transaction()
            && slot == case.expected.slot_after_exit
            && cursor == case.expected.cursor_after_exit
    });
    let status = process.finish(crash)?;
    let terminated = abruptly_terminated(&status);
    if crash != terminated || (!crash && !status.success()) {
        return Err(FAILED);
    }
    drop(process);
    let integrity_ok = raw_integrity_ok(&path)?;

    let (slot_after_exit, cursor_after_exit) = {
        let conn = crate::store::open_store(&path).map_err(|_| FAILED)?;
        (slots(&conn)?.0, cursor(&conn)?)
    };
    let recovery = match recover(&home, &path, case.scene) {
        Ok(recovery) => Some(recovery),
        Err(error) if error == REFUSED => None,
        Err(error) => return Err(error),
    };
    let recovery_refused = recovery.is_none();
    let initial_outcome = if terminated {
        InitialOutcome::Terminated
    } else {
        InitialOutcome::Ticked
    };
    let observed = match &recovery {
        Some(recovery) => Expected {
            initial_outcome,
            slot_after_exit,
            cursor_after_exit,
            slot_after_restart: recovery.slot_after_restart,
            next_slot_after_restart: recovery.next_slot_after_restart,
            claims: recovery.claims,
            restart_launches: recovery.restart_launches,
            slot_after_pass: recovery.slot_after_pass,
            next_slot_after_pass: recovery.next_slot_after_pass,
            launches: recovery.launches,
            stops: recovery.stops,
            integrity_ok,
            replay_is_noop: recovery.replay_is_noop,
        },
        None => Expected {
            initial_outcome,
            slot_after_exit,
            cursor_after_exit,
            slot_after_restart: Slot::Mixed,
            next_slot_after_restart: Slot::Mixed,
            claims: 0,
            restart_launches: 0,
            slot_after_pass: Slot::Mixed,
            next_slot_after_pass: Slot::Mixed,
            launches: 0,
            stops: 0,
            integrity_ok,
            replay_is_noop: false,
        },
    };
    let passed = observed == case.expected && pause_consistent && !recovery_refused;
    let (transaction_open_at_pause, slot_at_pause, cursor_at_pause) = match pause {
        Some((open, slot, cursor)) => (Some(open), Some(slot), Some(cursor)),
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
            "slotAtPause": slot_at_pause,
            "cursorAtPause": cursor_at_pause,
            "pauseConsistent": pause_consistent,
            "integrityOk": integrity_ok,
            "slotAfterExit": observed.slot_after_exit,
            "cursorAfterExit": observed.cursor_after_exit,
            "recoveryRefused": recovery_refused,
            "slotAfterRestart": observed.slot_after_restart,
            "nextSlotAfterRestart": observed.next_slot_after_restart,
            "claims": observed.claims,
            "restartLaunches": observed.restart_launches,
            "slotAfterPass": observed.slot_after_pass,
            "nextSlotAfterPass": observed.next_slot_after_pass,
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
            "../../../../../conformance/automations/runner/tick-process-crash-recovery.vectors.json"
        ))
        .unwrap()
    }

    #[test]
    fn tick_crash_vectors_restate_each_scene_and_boundary_outcome() {
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
    fn tick_crash_vectors_require_each_scene_boundary_and_control() {
        let original = vectors();
        let mut missing = original.clone();
        missing["cases"].as_array_mut().unwrap().pop();
        assert_eq!(evaluate(&missing).unwrap_err(), INVALID);
        // A second late control under its own case id, replacing a boundary.
        let mut duplicate = original.clone();
        let mut control = duplicate["cases"][13].clone();
        control["caseId"] = json!("second-late-restart-uninterrupted-tick-control");
        duplicate["cases"][7] = control;
        assert_eq!(evaluate(&duplicate).unwrap_err(), INVALID);
        // A prompt case relabelled late duplicates that scene's boundary.
        let mut relabelled = original.clone();
        relabelled["cases"][0]["scene"] = json!("late");
        relabelled["cases"][0]["expected"] = relabelled["cases"][7]["expected"].clone();
        assert_eq!(evaluate(&relabelled).unwrap_err(), INVALID);
        let mut duplicate_id = original;
        duplicate_id["cases"][1]["caseId"] = duplicate_id["cases"][0]["caseId"].clone();
        assert_eq!(evaluate(&duplicate_id).unwrap_err(), INVALID);
    }

    #[test]
    fn tick_crash_vectors_refuse_malformed_or_weakened_inputs() {
        let original = vectors();
        for (case, field, value) in [
            (0, "slotAfterExit", json!("planned")),
            (0, "slotAfterPass", json!("owned")),
            (0, "stops", json!(1)),
            (1, "cursorAfterExit", json!(1)),
            (2, "slotAfterExit", json!("absent")),
            (3, "cursorAfterExit", json!(0)),
            (4, "slotAfterExit", json!("claimed")),
            (5, "claims", json!(1)),
            (5, "slotAfterExit", json!("planned")),
            (6, "initialOutcome", json!("terminated")),
            (6, "restartLaunches", json!(0)),
            (6, "launches", json!(2)),
            (7, "slotAfterRestart", json!("skipped")),
            (7, "nextSlotAfterRestart", json!("absent")),
            (8, "slotAfterRestart", json!("absent")),
            (8, "slotAfterRestart", json!("owned")),
            (12, "slotAfterPass", json!("skipped")),
            (13, "claims", json!(2)),
            (13, "nextSlotAfterPass", json!("owned")),
            (13, "integrityOk", json!(false)),
            (13, "replayIsNoop", json!(false)),
        ] {
            let mut invalid = original.clone();
            invalid["cases"][case]["expected"][field] = value;
            assert_eq!(evaluate(&invalid).unwrap_err(), INVALID, "{case} {field}");
        }
        for (field, value) in [
            ("name", json!("Caller chosen")),
            ("status", json!("PAUSED")),
            ("misfire", json!("all")),
            ("rrule", json!("FREQ=DAILY;BYHOUR=10")),
            ("cwd", json!("/private-input")),
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
            json!("coven.automations.run-now-process-crash-recovery-vectors.v1");
        assert_eq!(evaluate(&invalid).unwrap_err(), INVALID);
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

    /// Runs the fenced tick and returns the rows each of its WAL commits
    /// changed, ignoring the event feed's bookkeeping tables.
    fn tick_commits(conn: &Connection, leadership: &SchedulerLeadership) -> Vec<Vec<String>> {
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
        let tick_at = at(base().unwrap(), TICK);
        let tick = crate::automations::occurrences::tick_with_scheduler_fence(
            conn,
            tick_at,
            &leadership.fence(),
        )
        .unwrap();
        assert_eq!((tick.planned.len(), tick.claimed.len()), (1, 1));
        conn.wal_hook(None);
        conn.update_hook(None::<fn(rusqlite::hooks::Action, &str, &str, i64)>)
            .unwrap();
        COMMITS.with(|commits| commits.take())
    }

    fn private_home() -> (tempfile::TempDir, PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        crate::daemon::ensure_private_coven_home(&home).unwrap();
        (temp, home)
    }

    #[test]
    fn the_tick_commits_its_plan_cursor_and_claim_in_three_transactions() {
        let tick_at = at(base().unwrap(), TICK);
        let (_temp, home) = private_home();
        let (conn, leadership) = prepare(&home, &synthetic_definition(), tick_at).unwrap();
        assert_eq!(
            tick_commits(&conn, &leadership),
            [
                vec!["insert automation_occurrences".to_owned()],
                vec!["update automation_scheduler_planning_cursor".to_owned()],
                vec!["update automation_occurrences".to_owned()],
            ]
        );
        // Without the seeded event counter, the tick's stale-claim recovery
        // commits it first, changing no row: a fourth commit the pause would
        // count ahead of the plan.
        let (_temp, home) = private_home();
        let conn = crate::store::open_store(&crate::api::store_path(&home)).unwrap();
        store_definition(&conn, &synthetic_definition()).unwrap();
        let leadership = SchedulerLeadership::acquire(&home, &conn, tick_at).unwrap();
        let commits = tick_commits(&conn, &leadership);
        assert_eq!(commits.len(), 4);
        assert!(commits[0].is_empty(), "{commits:?}");
    }

    /// The store a worker killed at `boundary` leaves. A kill inside the plan
    /// leaves the store as the tick found it. A kill inside or after a later
    /// transaction leaves what a TEMP trigger leaves when it aborts the first
    /// uncommitted transaction at the clause the worker pauses at: aborting
    /// the plan itself would not do, because the tick records a failed plan
    /// and still advances the cursor.
    fn killed_store(boundary: Boundary) -> (tempfile::TempDir, PathBuf) {
        let (temp, home) = private_home();
        let tick_at = at(base().unwrap(), TICK);
        let (conn, leadership) = prepare(&home, &synthetic_definition(), tick_at).unwrap();
        let uncommitted = match boundary {
            Boundary::AfterPlan => Some(Boundary::AfterPlan),
            Boundary::AfterPlanCommit | Boundary::AfterCursor => Some(Boundary::AfterCursor),
            Boundary::AfterCursorCommit | Boundary::AfterClaim => Some(Boundary::AfterClaim),
            Boundary::AfterClaimCommit | Boundary::None => None,
        };
        if uncommitted != Some(Boundary::AfterPlan) {
            if let Some(Pause::After(clause)) = uncommitted.and_then(pause_at) {
                conn.execute_batch(&format!(
                    "CREATE TEMP TRIGGER refuse_commit {clause}
                     BEGIN SELECT RAISE(ABORT, 'killed'); END;"
                ))
                .unwrap();
            }
            let tick = crate::automations::occurrences::tick_with_scheduler_fence(
                &conn,
                tick_at,
                &leadership.fence(),
            );
            assert_eq!(
                uncommitted.is_none(),
                tick.is_ok_and(|tick| tick.failed.is_empty())
            );
        }
        assert_eq!(
            (slots(&conn).unwrap().0, cursor(&conn).unwrap()),
            boundary.committed(),
            "{boundary:?}"
        );
        (temp, home)
    }

    #[test]
    fn each_boundary_store_recovers_as_its_vector_says() {
        for scene in Scene::ALL {
            for boundary in Boundary::ALL {
                let (_temp, home) = killed_store(boundary);
                let recovery = recover(&home, &crate::api::store_path(&home), scene).unwrap();
                let expected = expected_for(scene, boundary);
                assert_eq!(
                    (
                        recovery.slot_after_restart,
                        recovery.next_slot_after_restart,
                        recovery.claims,
                        recovery.restart_launches,
                        recovery.slot_after_pass,
                        recovery.next_slot_after_pass,
                        recovery.launches,
                        recovery.stops,
                        recovery.replay_is_noop,
                    ),
                    (
                        expected.slot_after_restart,
                        expected.next_slot_after_restart,
                        expected.claims,
                        expected.restart_launches,
                        expected.slot_after_pass,
                        expected.next_slot_after_pass,
                        expected.launches,
                        expected.stops,
                        expected.replay_is_noop,
                    ),
                    "{scene:?} {boundary:?}"
                );
            }
        }
    }

    #[test]
    fn slots_are_whole_or_mixed() {
        let (_temp, home) = killed_store(Boundary::None);
        let conn = crate::store::open_store(&crate::api::store_path(&home)).unwrap();
        assert_eq!(slots(&conn).unwrap(), (Slot::Claimed, Slot::Absent));
        for torn in [
            // A claim under another lease, or another scheduler's.
            "UPDATE automation_occurrences SET lease_owner = 'other'",
            "UPDATE automation_occurrences SET scheduler_generation = scheduler_generation + 1",
            // A restored claim reads neither planned nor claimed.
            "UPDATE automation_occurrences
             SET state = 'planned', lease_owner = NULL, lease_expires_at = NULL",
        ] {
            conn.execute_batch("BEGIN").unwrap();
            conn.execute(torn, []).unwrap();
            assert_eq!(slots(&conn).unwrap(), (Slot::Mixed, Slot::Absent), "{torn}");
            conn.execute_batch("ROLLBACK").unwrap();
        }
        // An occurrence for neither slot makes the whole store mixed.
        conn.execute_batch("BEGIN").unwrap();
        conn.execute(
            "INSERT INTO automation_occurrences
                (id, automation_id, automation_revision, definition_digest, scheduled_for, kind,
                 state, attempt, created_at, updated_at)
             SELECT 'occurrence-for-neither-slot', id, revision, definition_digest,
                    '2099-10-19T09:00:00.000Z', 'scheduled', 'planned', 0, ?1, ?1
             FROM automation_definitions",
            [DEFINED_AT],
        )
        .unwrap();
        assert_eq!(slots(&conn).unwrap(), (Slot::Mixed, Slot::Mixed));
        conn.execute_batch("ROLLBACK").unwrap();
        drop(conn);

        // A launched slot whose run, attempt or session disagrees is mixed, and
        // then its run, attempt and session belong to no slot.
        let runtime = CountingRuntime::default();
        let leadership = restart(&home, &runtime, at(base().unwrap(), PROMPT_RESTART)).unwrap();
        let conn = crate::store::open_store(&crate::api::store_path(&home)).unwrap();
        assert_eq!(slots(&conn).unwrap(), (Slot::Owned, Slot::Absent));
        for torn in [
            "UPDATE sessions SET status = 'created'",
            "UPDATE automation_attempts SET state = 'dispatching', session_id = NULL",
            "UPDATE automation_occurrences SET lease_owner = 'other'",
            "UPDATE automation_occurrences SET scheduler_generation = scheduler_generation - 1",
        ] {
            conn.execute_batch("BEGIN").unwrap();
            conn.execute(torn, []).unwrap();
            assert_eq!(slots(&conn).unwrap(), (Slot::Mixed, Slot::Mixed), "{torn}");
            conn.execute_batch("ROLLBACK").unwrap();
        }
        drop(leadership);
    }

    #[test]
    fn a_superseded_slot_is_read_with_its_claim_history() {
        let (_temp, home) = killed_store(Boundary::None);
        recover(&home, &crate::api::store_path(&home), Scene::Late).unwrap();
        let conn = crate::store::open_store(&crate::api::store_path(&home)).unwrap();
        assert_eq!(slots(&conn).unwrap(), (Slot::SkippedClaim, Slot::Succeeded));
        // Whether a restoration keeps the killed daemon's generation or clears
        // it, the claim is the killed daemon's.
        conn.execute_batch("BEGIN").unwrap();
        conn.execute(
            "UPDATE automation_occurrences SET scheduler_generation = NULL
             WHERE scheduled_for = '2099-10-20T09:00:00.000Z'",
            [],
        )
        .unwrap();
        assert_eq!(slots(&conn).unwrap(), (Slot::SkippedClaim, Slot::Succeeded));
        conn.execute_batch("ROLLBACK").unwrap();
        for torn in [
            // Another reason, a second claim, or a claim by the restarted
            // scheduler.
            "UPDATE automation_occurrences SET failure_reason = 'other'
             WHERE scheduled_for = '2099-10-20T09:00:00.000Z'",
            "UPDATE automation_occurrences SET attempt = 2
             WHERE scheduled_for = '2099-10-20T09:00:00.000Z'",
            "UPDATE automation_occurrences
             SET scheduler_generation =
                 (SELECT generation FROM automation_scheduler_authority WHERE id = 1)
             WHERE scheduled_for = '2099-10-20T09:00:00.000Z'",
        ] {
            conn.execute_batch("BEGIN").unwrap();
            conn.execute(torn, []).unwrap();
            assert_eq!(
                slots(&conn).unwrap(),
                (Slot::Mixed, Slot::Succeeded),
                "{torn}"
            );
            conn.execute_batch("ROLLBACK").unwrap();
        }
    }

    #[test]
    fn worker_refuses_invalid_or_weakened_input() {
        let mut case = vectors()["cases"][0].clone();
        case["storePath"] = json!("private-input");
        assert_eq!(worker(&case).unwrap_err(), INVALID);
        let mut case = vectors()["cases"][5].clone();
        case["expected"]["claims"] = json!(1);
        assert_eq!(worker(&case).unwrap_err(), INVALID);
        let mut case = vectors()["cases"][13].clone();
        case["boundary"] = json!("after_claim");
        assert_eq!(worker(&case).unwrap_err(), INVALID);
    }
}
