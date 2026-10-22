//! Disposable event-feed crash fixtures: a client follows the global
//! automation event feed through a daemon worker that ticks two daily routines
//! and serves the client's pages. The worker is killed inside the second
//! routine's claim, after it commits, inside the read that issues the client's
//! next checkpoint, or after that checkpoint commits with the page never
//! delivered. After the restart the client resumes from the
//! last page it received and must see every committed event exactly once, in
//! order, and no event the killed transaction wrote. Every launch goes through
//! a no-op runtime; no runtime process starts.
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

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
use super::run_launch_process_crash::{restart, CountingRuntime, REFUSED};
use super::valid_case_id;
use crate::automations::contract::events::{read_events, resume_events, EventPage};
use crate::automations::definition::RoutineDefinition;
use crate::automations::leadership::SchedulerLeadership;

const INVALID: &str = "conformance vector is invalid";
const FAILED: &str = "conformance suite execution failed";
const SCHEMA_VERSION: &str = "coven.automations.event-feed-process-crash-recovery-vectors.v1";
const WORKER: &str = "event-feed-crash-worker";
const HOME_PREFIX: &str = "feed-crash-";
/// The routine the worker ticks before the client's first page, and the one
/// it stores and ticks after.
const FIRST: &str = "conformance-feed-crash-first";
const SECOND: &str = "conformance-feed-crash-second";
/// Both definitions predate the slot the worker claims.
const DEFINED_AT: &str = "2099-10-19T09:00:00.000Z";
/// The global feed, the stream every event joins.
const FEED: (&str, &str) = ("feed", "all");
/// `subscribe.v1` serves at most this many events a page.
const PAGE: usize = 100;
/// `read.v1` takes the client's limit: the cursor resume reads three events a
/// page and the whole-feed read one, so those pages stop at their limit.
const CURSOR_PAGE: usize = 3;
const FEED_PAGE: usize = 1;
/// Seconds after `BASE`, the shared 09:00 slot: the worker ticks the first
/// routine, serves the client's first page, stores and ticks the second
/// routine, and serves the next page; the parent restarts the daemon and the
/// client resumes.
const FIRST_TICK: i64 = 30;
const FIRST_READ: i64 = 35;
const SECOND_TICK: i64 = 45;
const SECOND_READ: i64 = 50;
const RESTART: i64 = 150;
const RESUME: i64 = 160;

/// Where the worker can stop: inside the second routine's claim, after its
/// event is written; after the claim commits; inside the read that issues the
/// client's next checkpoint, before the checkpoint is written and after it
/// commits, with the page never delivered; and the uninterrupted control.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
enum Boundary {
    InsideClaim,
    AfterClaimCommit,
    InsideCheckpoint,
    AfterCheckpointCommit,
    None,
}

impl Boundary {
    const ALL: [Self; 5] = [
        Self::InsideClaim,
        Self::AfterClaimCommit,
        Self::InsideCheckpoint,
        Self::AfterCheckpointCommit,
        Self::None,
    ];

    /// The worker stops inside a write transaction.
    fn inside_transaction(self) -> bool {
        matches!(self, Self::InsideClaim | Self::InsideCheckpoint)
    }

    /// The second routine's claim committed before the worker stopped.
    fn claim_committed(self) -> bool {
        self != Self::InsideClaim
    }

    /// The read issuing the second checkpoint committed it.
    fn second_checkpoint_committed(self) -> bool {
        matches!(self, Self::AfterCheckpointCommit | Self::None)
    }
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Case {
    case_id: String,
    boundary: Boundary,
    definitions: [Value; 2],
    expected: Expected,
}

#[derive(Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Expected {
    initial_outcome: InitialOutcome,
    feed_after_exit: Vec<String>,
    checkpoints_after_exit: u64,
    delivered_pages: u64,
    delivered_events: u64,
    delivered_checkpoints_durable: bool,
    resumed_events: u64,
    feed_events: u64,
    first_occurrence_path: Vec<String>,
    second_occurrence_path: Vec<String>,
    first_run_path: Vec<String>,
    second_run_path: Vec<String>,
    exactly_once: bool,
    resume_paths_agree: bool,
    resume_refused: bool,
    streams_contiguous: bool,
    final_resume_empty: bool,
    launches: u64,
    integrity_ok: bool,
}

#[derive(Debug, Deserialize, Serialize, PartialEq, Clone, Copy)]
#[serde(rename_all = "snake_case")]
enum InitialOutcome {
    Terminated,
    Served,
}

/// The labels of the occurrence events the store has committed when the
/// worker stops: each routine's slot is planned and claimed by its tick.
fn feed_after_exit(boundary: Boundary) -> Vec<String> {
    let mut labels = vec![
        "first:occurrence:planned",
        "first:occurrence:claimed",
        "second:occurrence:planned",
    ];
    if boundary.claim_committed() {
        labels.push("second:occurrence:claimed");
    }
    labels.into_iter().map(str::to_owned).collect()
}

/// What each boundary must produce. A vector restates it, so it cannot weaken
/// it.
fn expected_for(boundary: Boundary) -> Expected {
    // A committed claim with no run is restored at the restart, then claimed
    // and launched again. A planned slot is claimed and launched.
    let restored = ["planned", "claimed", "planned", "claimed", "running"];
    let claimed_once = ["planned", "claimed", "running"];
    let second_path: &[&str] = if boundary.claim_committed() {
        &restored
    } else {
        &claimed_once
    };
    let path = |states: &[&str]| states.iter().map(|state| (*state).to_owned()).collect();
    let first_occurrence_path: Vec<String> = path(&restored);
    let second_occurrence_path: Vec<String> = path(second_path);
    // Each launch writes the run's `running`, then its attempt's `adopted`,
    // `dispatching` and `started`.
    let run_path: Vec<String> = path(&[
        "run:running",
        "attempt:adopted",
        "attempt:dispatching",
        "attempt:started",
    ]);
    let feed_events =
        first_occurrence_path.len() + second_occurrence_path.len() + 2 * run_path.len();
    let delivered = boundary == Boundary::None;
    let delivered_events = if delivered { 4 } else { 2 };
    Expected {
        initial_outcome: if delivered {
            InitialOutcome::Served
        } else {
            InitialOutcome::Terminated
        },
        feed_after_exit: feed_after_exit(boundary),
        checkpoints_after_exit: if boundary.second_checkpoint_committed() {
            2
        } else {
            1
        },
        delivered_pages: if delivered { 2 } else { 1 },
        delivered_events,
        delivered_checkpoints_durable: true,
        resumed_events: feed_events as u64 - delivered_events,
        feed_events: feed_events as u64,
        first_occurrence_path,
        second_occurrence_path,
        first_run_path: run_path.clone(),
        second_run_path: run_path,
        exactly_once: true,
        resume_paths_agree: true,
        resume_refused: false,
        streams_contiguous: true,
        final_resume_empty: true,
        launches: 2,
        integrity_ok: true,
    }
}

/// A daily routine at 09:00 UTC with a working directory and a 30-minute
/// deadline. The runner launches only a routine with a working directory; a
/// no-op runtime never touches it.
fn synthetic_definition(id: &str) -> Value {
    json!({"schemaVersion":1,"id":id,
        "name":"Conformance event feed crash","status":"ACTIVE",
        "rrule":"FREQ=DAILY;BYHOUR=9","timezone":"utc","misfire":"latest",
        "overlap":"forbid","timeoutMinutes":30,"runtime":"coven-code",
        "prompt":"Run the native event feed crash conformance probe.",
        "cwd":"/work/project"})
}

fn validate(case: &Case) -> Result<(), &'static str> {
    if !valid_case_id(&case.case_id)
        || case.definitions != [synthetic_definition(FIRST), synthetic_definition(SECOND)]
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

/// Where the worker parks inside SQLite. A claim boundary is installed once
/// the second routine is stored, before its tick plans the slot, advances the
/// cursor and claims it in three commits. A checkpoint boundary is installed
/// before the read, whose prune of expired checkpoints writes nothing here.
fn pause_at(boundary: Boundary) -> Option<Pause<'static>> {
    match boundary {
        Boundary::InsideClaim => Some(Pause::After(
            "AFTER INSERT ON main.automation_events
             WHEN json_extract(NEW.event_json, '$.payload.to') = 'claimed'",
        )),
        Boundary::AfterClaimCommit => Some(Pause::AfterCommits(3)),
        Boundary::InsideCheckpoint => Some(Pause::After(
            "BEFORE INSERT ON main.automation_event_checkpoints",
        )),
        Boundary::AfterCheckpointCommit => Some(Pause::AfterCommits(1)),
        Boundary::None => None,
    }
}

/// Stores a synthetic routine, defined before the slot it claims.
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

/// One page as the client received it: the checkpoint and cursor it resumes
/// from, and the ids of the events it carried, in order.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Delivered {
    checkpoint: String,
    next_after: Option<u64>,
    event_ids: Vec<String>,
}

impl Delivered {
    fn from_page(page: &EventPage) -> Self {
        Self {
            checkpoint: page.checkpoint.clone(),
            next_after: page.next_after,
            event_ids: page
                .events
                .iter()
                .map(|event| event.event_id.as_str().to_owned())
                .collect(),
        }
    }
}

/// Runs the scheduler's fenced tick; it must plan and claim one slot.
fn tick(
    conn: &Connection,
    now: DateTime<Utc>,
    leadership: &SchedulerLeadership,
) -> Result<(), &'static str> {
    let tick =
        crate::automations::occurrences::tick_with_scheduler_fence(conn, now, &leadership.fence())
            .map_err(|_| FAILED)?;
    if tick.planned.len() != 1 || tick.claimed.len() != 1 || !tick.failed.is_empty() {
        return Err(FAILED);
    }
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
    let base = base()?;
    let conn = crate::store::open_store(&crate::api::store_path(&home)).map_err(|_| FAILED)?;
    let [first, second] = &case.definitions;
    store_definition(&conn, first)?;
    // The daemon's scheduler ticks under its leadership fence.
    let leadership =
        SchedulerLeadership::acquire(&home, &conn, at(base, FIRST_TICK)).map_err(|_| FAILED)?;
    tick(&conn, at(base, FIRST_TICK), &leadership)?;
    // The daemon serves the client's first subscription, with no checkpoint.
    let first_page = read_events(
        &conn,
        FEED.0,
        FEED.1,
        None,
        None,
        PAGE,
        &iso(at(base, FIRST_READ)),
    )
    .map_err(|_| FAILED)?;
    let mut delivered = vec![Delivered::from_page(&first_page)];
    store_definition(&conn, second)?;
    let handshake = |delivered: &[Delivered]| {
        json!({"state":"paused", "boundary":case.boundary, "relativeHome":relative_home,
            "delivered":delivered})
    };
    let mut pause = pause_at(case.boundary);
    if matches!(
        case.boundary,
        Boundary::InsideClaim | Boundary::AfterClaimCommit
    ) {
        install_pause(&conn, pause.take().ok_or(FAILED)?, handshake(&delivered))?;
    }
    tick(&conn, at(base, SECOND_TICK), &leadership)?;
    if let Some(pause) = pause {
        install_pause(&conn, pause, handshake(&delivered))?;
    }
    // The client resubscribes from the checkpoint its first page carried.
    let second_page = resume_events(
        &conn,
        &first_page.checkpoint,
        FEED.0,
        FEED.1,
        PAGE,
        &iso(at(base, SECOND_READ)),
    )
    .map_err(|_| FAILED)?;
    if case.boundary != Boundary::None {
        return Err(FAILED);
    }
    delivered.push(Delivered::from_page(&second_page));
    // Exits like a daemon that served the page, then died before releasing
    // its leadership.
    drop(conn);
    drop(leadership);
    announce(&json!({"state":"served", "boundary":case.boundary,
        "relativeHome":relative_home, "delivered":delivered}))
}

/// One committed event as the client reads it.
struct FeedEvent {
    id: String,
    label: String,
}

/// Labels an event by its routine, its entity and the state it moved to:
/// `first:occurrence:planned`. An event that is not a transition is labelled
/// by its kind. Ids never appear in a label.
fn label(event: &Value) -> Result<String, &'static str> {
    let routine = match event["automationId"].as_str() {
        Some(FIRST) => "first",
        Some(SECOND) => "second",
        _ => "other",
    };
    let payload = &event["payload"];
    match (payload["entity"].as_str(), payload["to"].as_str()) {
        (Some(entity), Some(to)) => Ok(format!("{routine}:{entity}:{to}")),
        _ => Ok(format!(
            "{routine}:{}",
            event["kind"].as_str().ok_or(FAILED)?
        )),
    }
}

fn feed_events(page: &EventPage) -> Result<Vec<FeedEvent>, &'static str> {
    page.events
        .iter()
        .map(|event| {
            let value = serde_json::to_value(event).map_err(|_| FAILED)?;
            Ok(FeedEvent {
                id: event.event_id.as_str().to_owned(),
                label: label(&value)?,
            })
        })
        .collect()
}

/// The labels of every committed occurrence event, read straight from the
/// store in feed order.
fn committed_labels(conn: &Connection) -> Result<Vec<String>, &'static str> {
    let rows: Vec<String> = conn
        .prepare("SELECT event_json FROM automation_events ORDER BY feed_position")
        .and_then(|mut statement| statement.query_map([], |row| row.get(0))?.collect())
        .map_err(|_| FAILED)?;
    rows.iter()
        .map(|row| label(&serde_json::from_str(row).map_err(|_| FAILED)?))
        .collect()
}

/// Every committed event's id, read straight from the store in feed order.
fn committed_ids(conn: &Connection) -> Result<Vec<String>, &'static str> {
    conn.prepare("SELECT event_id FROM automation_events ORDER BY feed_position")
        .and_then(|mut statement| statement.query_map([], |row| row.get(0))?.collect())
        .map_err(|_| FAILED)
}

fn checkpoints(conn: &Connection) -> Result<u64, &'static str> {
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM automation_event_checkpoints",
            [],
            |row| row.get(0),
        )
        .map_err(|_| FAILED)?;
    u64::try_from(count).map_err(|_| FAILED)
}

/// Whether every stream's events carry sequences 0, 1, 2… in feed order, and
/// each stream head names the next one.
fn streams_contiguous(conn: &Connection) -> Result<bool, &'static str> {
    let rows: Vec<(String, String, i64)> = conn
        .prepare(
            "SELECT stream_kind, stream_id, sequence FROM automation_events
             ORDER BY feed_position",
        )
        .and_then(|mut statement| {
            statement
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
                .collect()
        })
        .map_err(|_| FAILED)?;
    let mut next: BTreeMap<(String, String), i64> = BTreeMap::new();
    for (kind, id, sequence) in rows {
        let expected = next.entry((kind, id)).or_insert(0);
        if sequence != *expected {
            return Ok(false);
        }
        *expected += 1;
    }
    let heads: Vec<(String, String, i64)> = conn
        .prepare("SELECT stream_kind, stream_id, next_sequence FROM automation_event_stream_heads")
        .and_then(|mut statement| {
            statement
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
                .collect()
        })
        .map_err(|_| FAILED)?;
    Ok(heads.len() == next.len()
        && heads
            .into_iter()
            .all(|(kind, id, head)| next.get(&(kind, id)) == Some(&head)))
}

/// More pages than this suite's feed can fill: a resume that stops advancing
/// fails its case instead of paging forever.
const MAX_PAGES: usize = 64;

/// How one client's paging ended.
struct Followed {
    events: Vec<FeedEvent>,
    /// The last page's checkpoint.
    checkpoint: Option<String>,
    /// It ended on a page with no event, within `MAX_PAGES`.
    settled: bool,
    /// The store refused a read as it refuses a client.
    refused: bool,
}

impl Followed {
    /// Paging ended on an empty page, and no read was refused.
    fn whole(&self) -> bool {
        self.settled && !self.refused
    }

    fn ids(&self) -> Vec<&str> {
        self.events.iter().map(|event| event.id.as_str()).collect()
    }
}

/// A page the client asked for, or `None` when the store refused the read as
/// it refuses a client: a missing or expired checkpoint, or one for another
/// stream.
fn client_read(
    read: Result<EventPage, crate::automations::contract::events::EventStoreError>,
) -> Result<Option<EventPage>, &'static str> {
    use crate::automations::contract::events::EventStoreError;
    match read {
        Ok(page) => Ok(Some(page)),
        Err(
            EventStoreError::CheckpointNotFound
            | EventStoreError::CursorExpired { .. }
            | EventStoreError::InvalidRead(_),
        ) => Ok(None),
        Err(_) => Err(FAILED),
    }
}

/// Pages through the feed, `limit` events a page, until a page carries no
/// event. With a checkpoint the client resubscribes from each page's
/// checkpoint (`subscribe.v1`); otherwise it reads after each page's cursor
/// (`read.v1`).
fn follow(
    conn: &Connection,
    mut checkpoint: Option<String>,
    mut after: Option<u64>,
    limit: usize,
    now: &str,
) -> Result<Followed, &'static str> {
    let resubscribes = checkpoint.is_some();
    let mut followed = Followed {
        events: Vec::new(),
        checkpoint: None,
        settled: false,
        refused: false,
    };
    for _ in 0..MAX_PAGES {
        let read = match &checkpoint {
            Some(checkpoint) => resume_events(conn, checkpoint, FEED.0, FEED.1, limit, now),
            None => read_events(conn, FEED.0, FEED.1, after, None, limit, now),
        };
        let Some(page) = client_read(read)? else {
            followed.refused = true;
            return Ok(followed);
        };
        let page_events = feed_events(&page)?;
        let done = page_events.is_empty();
        followed.events.extend(page_events);
        followed.checkpoint = Some(page.checkpoint.clone());
        if resubscribes {
            checkpoint = Some(page.checkpoint);
        } else {
            after = page.next_after;
        }
        if done {
            followed.settled = true;
            return Ok(followed);
        }
    }
    Ok(followed)
}

/// What the restarted daemon and the resuming client saw.
struct Recovery {
    resumed_events: u64,
    feed_events: u64,
    first_occurrence_path: Vec<String>,
    second_occurrence_path: Vec<String>,
    first_run_path: Vec<String>,
    second_run_path: Vec<String>,
    exactly_once: bool,
    resume_paths_agree: bool,
    resume_refused: bool,
    streams_contiguous: bool,
    final_resume_empty: bool,
    launches: u64,
}

fn recover(home: &Path, path: &Path, delivered: &[Delivered]) -> Result<Recovery, &'static str> {
    let base = base()?;
    let runtime = CountingRuntime::default();
    let leadership = restart(home, &runtime, at(base, RESTART))?;
    let launches = runtime.launches();
    drop(leadership);
    let conn = crate::store::open_store(path).map_err(|_| FAILED)?;
    let now = iso(at(base, RESUME));
    let last = delivered.last().ok_or(FAILED)?;
    // The client resumes from the last page it received, by its checkpoint
    // and by its cursor, and the two must agree.
    let resumed = follow(&conn, Some(last.checkpoint.clone()), None, PAGE, &now)?;
    let by_cursor = follow(&conn, None, last.next_after, CURSOR_PAGE, &now)?;
    let resume_paths_agree =
        resumed.whole() && by_cursor.whole() && resumed.ids() == by_cursor.ids();
    // Everything the client received, against the whole committed feed. The
    // feed's order is the store's feed-position order.
    let feed = follow(&conn, None, None, FEED_PAGE, &now)?;
    let committed = committed_ids(&conn)?;
    let received: Vec<&str> = delivered
        .iter()
        .flat_map(|page| page.event_ids.iter().map(String::as_str))
        .chain(resumed.ids())
        .collect();
    let exactly_once = resumed.whole()
        && feed.whole()
        && received == feed.ids()
        && feed.ids() == committed.iter().map(String::as_str).collect::<Vec<_>>();
    // A resume from the last page's checkpoint returns nothing.
    let final_read = match &resumed.checkpoint {
        Some(checkpoint) if resumed.whole() => {
            client_read(resume_events(&conn, checkpoint, FEED.0, FEED.1, PAGE, &now))?
                .map(|page| page.events.is_empty())
        }
        _ => None,
    };
    let resume_refused = resumed.refused
        || by_cursor.refused
        || feed.refused
        || (resumed.whole() && final_read.is_none());
    let occurrence_path = |routine: &str| {
        feed.events
            .iter()
            .filter_map(|event| {
                event
                    .label
                    .strip_prefix(&format!("{routine}:occurrence:"))
                    .map(str::to_owned)
            })
            .collect::<Vec<_>>()
    };
    let run_path = |routine: &str| {
        feed.events
            .iter()
            .filter_map(|event| event.label.strip_prefix(&format!("{routine}:")))
            .filter(|rest| rest.starts_with("run:") || rest.starts_with("attempt:"))
            .map(str::to_owned)
            .collect::<Vec<_>>()
    };
    Ok(Recovery {
        resumed_events: resumed.events.len() as u64,
        feed_events: feed.events.len() as u64,
        first_occurrence_path: occurrence_path("first"),
        second_occurrence_path: occurrence_path("second"),
        first_run_path: run_path("first"),
        second_run_path: run_path("second"),
        exactly_once,
        resume_paths_agree,
        resume_refused,
        streams_contiguous: streams_contiguous(&conn)?,
        final_resume_empty: final_read == Some(true),
        launches,
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
    delivered: Vec<Delivered>,
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
/// worker's write transaction is still open, the occurrence events it has
/// committed, and how many checkpoints exist.
fn observe_pause(path: &Path) -> Result<(bool, Vec<String>, u64), &'static str> {
    let probe = open_raw(path)?;
    let open = write_transaction_open(&probe)?;
    Ok((open, committed_labels(&probe)?, checkpoints(&probe)?))
}

/// Whether every checkpoint the client received exists, for the stream and
/// cursor of the page that carried it.
fn checkpoints_durable(conn: &Connection, delivered: &[Delivered]) -> Result<bool, &'static str> {
    for page in delivered {
        let stored: Option<(String, String, i64)> = conn
            .query_row(
                "SELECT stream_kind, stream_id, after_sequence
                 FROM automation_event_checkpoints WHERE checkpoint = ?1",
                [&page.checkpoint],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .ok();
        let after = page
            .next_after
            .map_or(Ok(-1), i64::try_from)
            .map_err(|_| FAILED)?;
        if stored != Some((FEED.0.to_owned(), FEED.1.to_owned(), after)) {
            return Ok(false);
        }
    }
    Ok(true)
}

fn case_matches(case: &Case) -> Result<(bool, Value), &'static str> {
    let root = super::scheduler_conformance_tempdir()?;
    crate::daemon::ensure_private_coven_home(root.path()).map_err(|_| FAILED)?;
    let payload = serde_json::to_vec(case).map_err(|_| INVALID)?;
    let (mut process, receiver) = spawn_worker::<Handshake>(WORKER, payload, root.path())?;
    let ready = receiver.recv_timeout(CHILD_GUARD).map_err(|_| FAILED)??;
    let crash = case.boundary != Boundary::None;
    if ready.boundary != case.boundary || ready.state != if crash { "paused" } else { "served" } {
        return Err(FAILED);
    }
    let home = worker_home(root.path(), &ready.relative_home, HOME_PREFIX)?;
    let path = crate::api::store_path(&home);
    let pause = crash.then(|| observe_pause(&path)).transpose()?;
    let pause_consistent = pause.as_ref().is_none_or(|(open, labels, checkpoints)| {
        *open == case.boundary.inside_transaction()
            && *labels == case.expected.feed_after_exit
            && *checkpoints == case.expected.checkpoints_after_exit
    });
    let status = process.finish(crash)?;
    let terminated = abruptly_terminated(&status);
    if crash != terminated || (!crash && !status.success()) {
        return Err(FAILED);
    }
    drop(process);
    let integrity_ok = raw_integrity_ok(&path)?;

    let delivered = ready.delivered;
    let (feed_after_exit, checkpoints_after_exit, delivered_checkpoints_durable) = {
        let conn = crate::store::open_store(&path).map_err(|_| FAILED)?;
        (
            committed_labels(&conn)?,
            checkpoints(&conn)?,
            checkpoints_durable(&conn, &delivered)?,
        )
    };
    let delivered_events = delivered
        .iter()
        .map(|page| page.event_ids.len() as u64)
        .sum();
    let recovery = match recover(&home, &path, &delivered) {
        Ok(recovery) => Some(recovery),
        Err(error) if error == REFUSED => None,
        Err(error) => return Err(error),
    };
    let recovery_refused = recovery.is_none();
    let recovery = recovery.unwrap_or(Recovery {
        resumed_events: 0,
        feed_events: 0,
        first_occurrence_path: Vec::new(),
        second_occurrence_path: Vec::new(),
        first_run_path: Vec::new(),
        second_run_path: Vec::new(),
        exactly_once: false,
        resume_paths_agree: false,
        resume_refused: false,
        streams_contiguous: false,
        final_resume_empty: false,
        launches: 0,
    });
    let observed = Expected {
        initial_outcome: if terminated {
            InitialOutcome::Terminated
        } else {
            InitialOutcome::Served
        },
        feed_after_exit,
        checkpoints_after_exit,
        delivered_pages: delivered.len() as u64,
        delivered_events,
        delivered_checkpoints_durable,
        resumed_events: recovery.resumed_events,
        feed_events: recovery.feed_events,
        first_occurrence_path: recovery.first_occurrence_path,
        second_occurrence_path: recovery.second_occurrence_path,
        first_run_path: recovery.first_run_path,
        second_run_path: recovery.second_run_path,
        exactly_once: recovery.exactly_once,
        resume_paths_agree: recovery.resume_paths_agree,
        resume_refused: recovery.resume_refused,
        streams_contiguous: recovery.streams_contiguous,
        final_resume_empty: recovery.final_resume_empty,
        launches: recovery.launches,
        integrity_ok,
    };
    let passed = observed == case.expected && pause_consistent && !recovery_refused;
    let (transaction_open_at_pause, feed_at_pause, checkpoints_at_pause) = match pause {
        Some((open, labels, checkpoints)) => (Some(open), Some(labels), Some(checkpoints)),
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
            "feedAtPause": feed_at_pause,
            "checkpointsAtPause": checkpoints_at_pause,
            "pauseConsistent": pause_consistent,
            "integrityOk": integrity_ok,
            "feedAfterExit": observed.feed_after_exit,
            "checkpointsAfterExit": observed.checkpoints_after_exit,
            "deliveredPages": observed.delivered_pages,
            "deliveredEvents": observed.delivered_events,
            "deliveredCheckpointsDurable": observed.delivered_checkpoints_durable,
            "recoveryRefused": recovery_refused,
            "resumedEvents": observed.resumed_events,
            "feedEvents": observed.feed_events,
            "firstOccurrencePath": observed.first_occurrence_path,
            "secondOccurrencePath": observed.second_occurrence_path,
            "firstRunPath": observed.first_run_path,
            "secondRunPath": observed.second_run_path,
            "exactlyOnce": observed.exactly_once,
            "resumePathsAgree": observed.resume_paths_agree,
            "resumeRefused": observed.resume_refused,
            "streamsContiguous": observed.streams_contiguous,
            "finalResumeEmpty": observed.final_resume_empty,
            "launches": observed.launches,
        }),
    ))
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::path::PathBuf;

    use super::*;

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

    fn record_changes(conn: &Connection) {
        COMMITS.with(|commits| commits.take());
        CHANGES.with(|changes| changes.take());
        conn.update_hook(Some(
            |action: rusqlite::hooks::Action, _: &str, table: &str, _: i64| {
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
            "../../../../../conformance/automations/runner/event-feed-process-crash-recovery.vectors.json"
        ))
        .unwrap()
    }

    #[test]
    fn feed_crash_vectors_restate_each_boundary_outcome() {
        let vectors: VectorSet = serde_json::from_value(vectors()).unwrap();
        assert_eq!(vectors.schema_version, SCHEMA_VERSION);
        for case in &vectors.cases {
            validate(case).unwrap();
        }
        let boundaries: Vec<_> = vectors.cases.iter().map(|case| case.boundary).collect();
        assert_eq!(boundaries, Boundary::ALL);
    }

    #[test]
    fn feed_crash_vectors_require_each_boundary_and_the_control() {
        let original = vectors();
        let mut missing = original.clone();
        missing["cases"].as_array_mut().unwrap().pop();
        assert_eq!(evaluate(&missing).unwrap_err(), INVALID);
        // A second control under its own case id, replacing a boundary.
        let mut duplicate = original.clone();
        let mut control = duplicate["cases"][4].clone();
        control["caseId"] = json!("second-uninterrupted-control");
        duplicate["cases"][0] = control;
        assert_eq!(evaluate(&duplicate).unwrap_err(), INVALID);
        let mut duplicate_id = original;
        duplicate_id["cases"][1]["caseId"] = duplicate_id["cases"][0]["caseId"].clone();
        assert_eq!(evaluate(&duplicate_id).unwrap_err(), INVALID);
    }

    #[test]
    fn feed_crash_vectors_refuse_malformed_or_weakened_inputs() {
        let original = vectors();
        for (case, field, value) in [
            (
                0,
                "feedAfterExit",
                json!([
                    "first:occurrence:planned",
                    "first:occurrence:claimed",
                    "second:occurrence:planned",
                    "second:occurrence:claimed"
                ]),
            ),
            (
                0,
                "secondOccurrencePath",
                json!(["planned", "claimed", "planned", "claimed", "running"]),
            ),
            (
                1,
                "feedAfterExit",
                json!([
                    "first:occurrence:planned",
                    "first:occurrence:claimed",
                    "second:occurrence:planned"
                ]),
            ),
            (2, "checkpointsAfterExit", json!(2)),
            (3, "checkpointsAfterExit", json!(1)),
            (3, "deliveredPages", json!(2)),
            (4, "initialOutcome", json!("terminated")),
            (4, "deliveredEvents", json!(2)),
            (0, "deliveredCheckpointsDurable", json!(false)),
            (1, "resumedEvents", json!(18)),
            (1, "feedEvents", json!(17)),
            (
                2,
                "firstOccurrencePath",
                json!(["planned", "claimed", "running"]),
            ),
            (2, "firstRunPath", json!(["run:running", "attempt:adopted"])),
            (2, "secondRunPath", json!([])),
            (3, "exactlyOnce", json!(false)),
            (3, "resumePathsAgree", json!(false)),
            (3, "resumeRefused", json!(true)),
            (4, "streamsContiguous", json!(false)),
            (4, "finalResumeEmpty", json!(false)),
            (4, "launches", json!(1)),
            (4, "integrityOk", json!(false)),
        ] {
            let mut invalid = original.clone();
            invalid["cases"][case]["expected"][field] = value;
            assert_eq!(evaluate(&invalid).unwrap_err(), INVALID, "{case} {field}");
        }
        for (index, field, value) in [
            (0, "id", json!(SECOND)),
            (1, "id", json!(FIRST)),
            (0, "status", json!("PAUSED")),
            (1, "rrule", json!("FREQ=DAILY;BYHOUR=10")),
            (1, "cwd", json!("/private-input")),
        ] {
            let mut invalid = original.clone();
            invalid["cases"][0]["definitions"][index][field] = value;
            assert_eq!(
                evaluate(&invalid).unwrap_err(),
                INVALID,
                "definition {index} {field}"
            );
        }
        for (field, value) in [
            ("storePath", json!("private-input")),
            ("boundary", json!("after_read")),
            ("caseId", json!("")),
            ("definitions", json!([synthetic_definition(FIRST)])),
        ] {
            let mut invalid = original.clone();
            invalid["cases"][0][field] = value;
            assert_eq!(evaluate(&invalid).unwrap_err(), INVALID, "{field}");
        }
        let mut invalid = original;
        invalid["schemaVersion"] =
            json!("coven.automations.no-process-receipt-process-crash-recovery-vectors.v1");
        assert_eq!(evaluate(&invalid).unwrap_err(), INVALID);
    }

    /// A store after the worker's first tick and first page, with the second
    /// routine stored.
    fn first_page_store() -> (
        tempfile::TempDir,
        PathBuf,
        Connection,
        SchedulerLeadership,
        Delivered,
    ) {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        crate::daemon::ensure_private_coven_home(&home).unwrap();
        let conn = crate::store::open_store(&crate::api::store_path(&home)).unwrap();
        let base = base().unwrap();
        store_definition(&conn, &synthetic_definition(FIRST)).unwrap();
        let leadership = SchedulerLeadership::acquire(&home, &conn, at(base, FIRST_TICK)).unwrap();
        tick(&conn, at(base, FIRST_TICK), &leadership).unwrap();
        let first = read_events(
            &conn,
            FEED.0,
            FEED.1,
            None,
            None,
            PAGE,
            &iso(at(base, FIRST_READ)),
        )
        .unwrap();
        store_definition(&conn, &synthetic_definition(SECOND)).unwrap();
        (temp, home, conn, leadership, Delivered::from_page(&first))
    }

    #[test]
    fn the_second_tick_commits_three_times_and_the_read_once() {
        let (_temp, _home, conn, leadership, first) = first_page_store();
        let base = base().unwrap();
        record_changes(&conn);
        tick(&conn, at(base, SECOND_TICK), &leadership).unwrap();
        // The event feed's own bookkeeping rides inside each commit.
        let main_rows = |commits: Vec<Vec<String>>| -> Vec<Vec<String>> {
            commits
                .into_iter()
                .map(|commit| {
                    commit
                        .into_iter()
                        .filter(|change| !change.contains("automation_event"))
                        .collect()
                })
                .collect()
        };
        let commits = COMMITS.with(|commits| commits.take());
        let events: Vec<usize> = commits
            .iter()
            .map(|commit| {
                commit
                    .iter()
                    .filter(|change| *change == "insert automation_events")
                    .count()
            })
            .collect();
        assert_eq!(
            main_rows(commits),
            [
                vec!["insert automation_occurrences"],
                vec!["update automation_scheduler_planning_cursor"],
                vec!["update automation_occurrences"],
            ]
        );
        assert_eq!(events, [1, 0, 1]);
        resume_events(
            &conn,
            &first.checkpoint,
            FEED.0,
            FEED.1,
            PAGE,
            &iso(at(base, SECOND_READ)),
        )
        .unwrap();
        conn.wal_hook(None);
        assert_eq!(
            COMMITS.with(|commits| commits.take()),
            [vec!["insert automation_event_checkpoints"]]
        );
    }

    /// The store, and the pages the client received, that a worker stopped
    /// at `boundary` leaves. A TEMP trigger aborts the statement a kill
    /// inside a transaction leaves uncommitted.
    fn stopped_store(boundary: Boundary) -> (tempfile::TempDir, PathBuf, Vec<Delivered>) {
        let (temp, home, conn, leadership, first) = first_page_store();
        let base = base().unwrap();
        let mut delivered = vec![first];
        if boundary == Boundary::InsideClaim {
            conn.execute_batch(
                "CREATE TEMP TRIGGER killed AFTER INSERT ON main.automation_events
                 WHEN json_extract(NEW.event_json, '$.payload.to') = 'claimed'
                 BEGIN SELECT RAISE(ABORT, 'killed'); END;",
            )
            .unwrap();
            let tick = crate::automations::occurrences::tick_with_scheduler_fence(
                &conn,
                at(base, SECOND_TICK),
                &leadership.fence(),
            )
            .unwrap();
            assert!(tick.claimed.is_empty());
        } else {
            tick(&conn, at(base, SECOND_TICK), &leadership).unwrap();
        }
        if boundary == Boundary::InsideCheckpoint {
            conn.execute_batch(
                "CREATE TEMP TRIGGER killed BEFORE INSERT ON main.automation_event_checkpoints
                 BEGIN SELECT RAISE(ABORT, 'killed'); END;",
            )
            .unwrap();
        }
        if matches!(
            boundary,
            Boundary::InsideCheckpoint | Boundary::AfterCheckpointCommit | Boundary::None
        ) {
            let second = resume_events(
                &conn,
                &delivered[0].checkpoint,
                FEED.0,
                FEED.1,
                PAGE,
                &iso(at(base, SECOND_READ)),
            );
            assert_eq!(second.is_ok(), boundary != Boundary::InsideCheckpoint);
            if boundary == Boundary::None {
                delivered.push(Delivered::from_page(&second.unwrap()));
            }
        }
        let expected = expected_for(boundary);
        assert_eq!(committed_labels(&conn).unwrap(), expected.feed_after_exit);
        assert_eq!(checkpoints(&conn).unwrap(), expected.checkpoints_after_exit);
        assert!(checkpoints_durable(&conn, &delivered).unwrap());
        drop(conn);
        drop(leadership);
        (temp, home, delivered)
    }

    #[test]
    fn each_boundary_store_recovers_as_its_vector_says() {
        for boundary in Boundary::ALL {
            let (_temp, home, delivered) = stopped_store(boundary);
            let recovery = recover(&home, &crate::api::store_path(&home), &delivered).unwrap();
            let expected = expected_for(boundary);
            assert_eq!(
                delivered
                    .iter()
                    .map(|page| page.event_ids.len() as u64)
                    .sum::<u64>(),
                expected.delivered_events,
                "{boundary:?}"
            );
            assert_eq!(
                (
                    recovery.resumed_events,
                    recovery.feed_events,
                    recovery.first_occurrence_path,
                    recovery.second_occurrence_path,
                    recovery.first_run_path,
                    recovery.second_run_path,
                    recovery.exactly_once,
                    recovery.resume_paths_agree,
                    recovery.resume_refused,
                    recovery.streams_contiguous,
                    recovery.final_resume_empty,
                    recovery.launches,
                ),
                (
                    expected.resumed_events,
                    expected.feed_events,
                    expected.first_occurrence_path,
                    expected.second_occurrence_path,
                    expected.first_run_path,
                    expected.second_run_path,
                    expected.exactly_once,
                    expected.resume_paths_agree,
                    expected.resume_refused,
                    expected.streams_contiguous,
                    expected.final_resume_empty,
                    expected.launches,
                ),
                "{boundary:?}"
            );
        }
    }

    #[test]
    fn a_client_that_skips_repeats_or_reorders_an_event_is_not_exactly_once() {
        let tampered: [fn(&mut Vec<Delivered>); 3] = [
            // Repeats the first page's last event.
            |delivered| {
                let repeated = delivered[0].event_ids[1].clone();
                delivered[1].event_ids.insert(0, repeated);
            },
            |delivered| {
                delivered[0].event_ids.pop();
            },
            |delivered| delivered[0].event_ids.swap(0, 1),
        ];
        for tamper in tampered {
            let (_temp, home, mut delivered) = stopped_store(Boundary::None);
            tamper(&mut delivered);
            let path = crate::api::store_path(&home);
            assert!(!recover(&home, &path, &delivered).unwrap().exactly_once);
        }
    }

    #[test]
    fn a_resume_that_stalls_or_is_refused_ends_instead_of_hanging() {
        let (_temp, home, delivered) = stopped_store(Boundary::None);
        let conn = crate::store::open_store(&crate::api::store_path(&home)).unwrap();
        let now = iso(at(base().unwrap(), RESUME));
        // Every checkpoint issued from now on points back at the feed's start.
        conn.execute_batch(
            "CREATE TEMP TRIGGER stall AFTER INSERT ON main.automation_event_checkpoints
             BEGIN
                 UPDATE main.automation_event_checkpoints SET after_sequence = -1
                 WHERE checkpoint = NEW.checkpoint;
             END;",
        )
        .unwrap();
        let stalled = follow(
            &conn,
            Some(delivered[0].checkpoint.clone()),
            None,
            PAGE,
            &now,
        )
        .unwrap();
        assert!(!stalled.settled && !stalled.refused && !stalled.whole());
        assert!(stalled.events.len() > 4);
        conn.execute_batch("DROP TRIGGER temp.stall").unwrap();
        let missing = follow(&conn, Some("ecp-missing".to_owned()), None, PAGE, &now).unwrap();
        assert!(missing.refused && missing.events.is_empty() && !missing.whole());
        let expired = follow(
            &conn,
            Some(delivered[0].checkpoint.clone()),
            None,
            PAGE,
            "2099-10-21T09:00:35.000Z",
        )
        .unwrap();
        assert!(expired.refused && expired.events.is_empty());
        // A checkpoint the store issued for another stream.
        conn.execute(
            "UPDATE automation_event_checkpoints SET stream_kind = 'run' WHERE checkpoint = ?1",
            [&delivered[1].checkpoint],
        )
        .unwrap();
        let foreign = follow(
            &conn,
            Some(delivered[1].checkpoint.clone()),
            None,
            PAGE,
            &now,
        )
        .unwrap();
        assert!(foreign.refused && foreign.events.is_empty());
        let whole = follow(
            &conn,
            Some(delivered[0].checkpoint.clone()),
            None,
            PAGE,
            &now,
        )
        .unwrap();
        assert!(whole.whole());
        assert_eq!(whole.events.len(), 2);
    }

    #[test]
    fn pages_stop_at_their_limit_and_still_follow_every_event() {
        let (_temp, home, _delivered) = stopped_store(Boundary::None);
        let conn = crate::store::open_store(&crate::api::store_path(&home)).unwrap();
        let now = iso(at(base().unwrap(), RESUME));
        let committed = committed_ids(&conn).unwrap();
        assert_eq!(committed.len(), 4);
        for limit in [FEED_PAGE, CURSOR_PAGE, PAGE] {
            let by_cursor = follow(&conn, None, None, limit, &now).unwrap();
            assert!(by_cursor.whole(), "{limit}");
            assert_eq!(by_cursor.ids(), committed, "{limit}");
            let first = read_events(&conn, FEED.0, FEED.1, None, None, limit, &now).unwrap();
            let by_checkpoint =
                follow(&conn, Some(first.checkpoint.clone()), None, limit, &now).unwrap();
            let mut ids: Vec<String> = feed_events(&first)
                .unwrap()
                .into_iter()
                .map(|event| event.id)
                .collect();
            ids.extend(by_checkpoint.ids().into_iter().map(str::to_owned));
            assert!(by_checkpoint.whole(), "{limit}");
            assert_eq!(ids, committed, "{limit}");
        }
    }

    #[test]
    fn a_torn_stream_is_not_contiguous() {
        let (_temp, home, _delivered) = stopped_store(Boundary::AfterClaimCommit);
        let conn = crate::store::open_store(&crate::api::store_path(&home)).unwrap();
        assert!(streams_contiguous(&conn).unwrap());
        for torn in [
            "UPDATE automation_event_stream_heads SET next_sequence = next_sequence + 1
             WHERE stream_id = (SELECT MIN(stream_id) FROM automation_event_stream_heads)",
            "INSERT INTO automation_event_stream_heads
                 (stream_kind, stream_id, next_sequence, earliest_sequence, updated_at)
             VALUES ('run', 'run-orphan', 0, 0, '2099-10-20T09:00:00.000Z')",
            "DROP TRIGGER automation_events_no_delete;
             DELETE FROM automation_events
             WHERE feed_position = (SELECT MIN(feed_position) FROM automation_events)",
            // A gap in the middle of a stream whose head still names the next
            // sequence.
            "DROP TRIGGER automation_events_no_update;
             UPDATE automation_events SET sequence = 2
             WHERE sequence = 1 AND stream_id = (SELECT MIN(stream_id) FROM automation_events);
             UPDATE automation_event_stream_heads SET next_sequence = 3
             WHERE stream_id = (SELECT MIN(stream_id) FROM automation_events)",
            // A stream's two events out of feed order.
            "DROP TRIGGER automation_events_no_update;
             UPDATE automation_events SET sequence = 99
             WHERE sequence = 0 AND stream_id = (SELECT MIN(stream_id) FROM automation_events);
             UPDATE automation_events SET sequence = 0
             WHERE sequence = 1 AND stream_id = (SELECT MIN(stream_id) FROM automation_events);
             UPDATE automation_events SET sequence = 1
             WHERE sequence = 99 AND stream_id = (SELECT MIN(stream_id) FROM automation_events)",
        ] {
            conn.execute_batch("BEGIN").unwrap();
            conn.execute_batch(torn).unwrap();
            assert!(!streams_contiguous(&conn).unwrap(), "{torn}");
            conn.execute_batch("ROLLBACK").unwrap();
        }
        assert!(streams_contiguous(&conn).unwrap());
    }

    #[test]
    fn a_checkpoint_counts_only_for_its_page() {
        let (_temp, home, delivered) = stopped_store(Boundary::None);
        let conn = crate::store::open_store(&crate::api::store_path(&home)).unwrap();
        assert!(checkpoints_durable(&conn, &delivered).unwrap());
        let mut moved = delivered.clone();
        moved[1].next_after = Some(1);
        assert!(!checkpoints_durable(&conn, &moved).unwrap());
        let mut missing = delivered;
        missing[0].checkpoint = "ecp-missing".to_owned();
        assert!(!checkpoints_durable(&conn, &missing).unwrap());
    }

    #[test]
    fn labels_never_carry_ids() {
        let (_temp, home, _delivered) = stopped_store(Boundary::None);
        let conn = crate::store::open_store(&crate::api::store_path(&home)).unwrap();
        let labels = committed_labels(&conn).unwrap();
        assert_eq!(labels.len(), 4);
        for label in labels {
            assert!(!label.contains("conformance"), "{label}");
            assert!(!label.contains("run-"), "{label}");
        }
        assert_eq!(
            label(&json!({"automationId":"other-routine",
                "payload":{"entity":"occurrence","to":"planned"}}))
            .unwrap(),
            "other:occurrence:planned"
        );
        // An event that is not a transition is labelled by its kind.
        assert_eq!(
            label(&json!({"automationId":FIRST, "kind":"run.receipt.recorded",
                "payload":{"receiptId":"receipt-1"}}))
            .unwrap(),
            "first:run.receipt.recorded"
        );
    }

    #[test]
    fn worker_refuses_invalid_or_weakened_input() {
        let mut case = vectors()["cases"][0].clone();
        case["storePath"] = json!("private-input");
        assert_eq!(worker(&case).unwrap_err(), INVALID);
        let mut case = vectors()["cases"][3].clone();
        case["expected"]["checkpointsAfterExit"] = json!(1);
        assert_eq!(worker(&case).unwrap_err(), INVALID);
        let mut case = vectors()["cases"][4].clone();
        case["boundary"] = json!("inside_claim");
        assert_eq!(worker(&case).unwrap_err(), INVALID);
    }
}
