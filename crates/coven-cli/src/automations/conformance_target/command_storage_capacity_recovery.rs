//! Real SQLite page-quota exhaustion at the command-adoption boundary. No runtime
//! launches or authority claims: both cases use disposable owner-local stores.
use std::collections::BTreeSet;

use rusqlite::{Connection, ErrorCode};
use serde::Deserialize;
use serde_json::{json, Value};

use super::{scheduler_conformance_tempdir, valid_case_id};
use crate::automations::command_adoption::{
    execute_definition_command, DefinitionCommand, DefinitionCommandOutcome,
    DefinitionCommandResponse,
};
use crate::automations::contract::types::AdoptionKey;
use crate::automations::definition::RoutineDefinition;
use crate::automations::owner_grants::CommandAuthority;

const INVALID: &str = "conformance vector is invalid";
const FAILED: &str = "conformance suite execution failed";
const FIRST_AT: &str = "2026-08-30T09:00:00.000Z";

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct VectorSet {
    schema_version: String,
    cases: Vec<Case>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Case {
    case_id: String,
    fault: Fault,
    prompt_bytes: usize,
    adoption_key: String,
    definition: Value,
    expected: Expected,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Expected {
    initial_outcome: InitialOutcome,
    baseline_preserved: bool,
    initial_rows: u64,
    final_rows: u64,
    replay_preserved: bool,
}

#[derive(Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
enum InitialOutcome {
    DiskFull,
    Committed,
}

#[derive(Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
enum Fault {
    SqliteMaxPageCount,
    None,
}

pub(super) fn evaluate(vector: &Value) -> Result<(bool, Vec<Value>), &'static str> {
    let vectors: VectorSet = serde_json::from_value(vector.clone()).map_err(|_| INVALID)?;
    if vectors.schema_version != "coven.automations.command-storage-capacity-recovery-vectors.v1"
        || vectors.cases.len() != 2
    {
        return Err(INVALID);
    }
    let mut ids = BTreeSet::new();
    let mut faults = BTreeSet::new();
    for case in &vectors.cases {
        RoutineDefinition::from_json(&case.definition).map_err(|_| INVALID)?;
        if !valid_case_id(&case.case_id)
            || !ids.insert(&case.case_id)
            || !faults.insert(&case.fault)
            || AdoptionKey::new(case.adoption_key.clone()).is_err()
            || case.prompt_bytes != 131072
            || !case.expected.baseline_preserved
            || case.expected.final_rows != 1
            || !case.expected.replay_preserved
            || case.expected.initial_rows != u64::from(case.fault == Fault::None)
            || (case.expected.initial_outcome == InitialOutcome::DiskFull)
                != (case.fault == Fault::SqliteMaxPageCount)
        {
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

fn expanded_definition(case: &Case) -> Value {
    let mut definition = case.definition.clone();
    definition["prompt"] = Value::String("x".repeat(case.prompt_bytes));
    definition
}

fn execute(conn: &Connection, case: &Case, at: &str) -> anyhow::Result<DefinitionCommandResponse> {
    execute_definition_command(
        conn,
        &case.adoption_key,
        DefinitionCommand::Create {
            definition: expanded_definition(case),
        },
        at,
        CommandAuthority::OwnerLocal,
    )
}

type Snapshot = Vec<Vec<Vec<rusqlite::types::Value>>>;
fn snapshot(conn: &Connection) -> Result<Snapshot, &'static str> {
    let mut tables = Vec::new();
    for table in [
        "automation_definitions",
        "automation_rich_definition_revisions",
        "automation_owner_principal",
        "automation_owner_grants",
        "automation_command_adoptions",
        "automation_command_reservations",
        "automation_events",
        "automation_event_stream_heads",
    ] {
        let mut statement = conn
            .prepare(&format!("SELECT * FROM {table} ORDER BY rowid"))
            .map_err(|_| FAILED)?;
        let columns = statement.column_count();
        let rows = statement
            .query_map([], |row| {
                (0..columns)
                    .map(|index| row.get(index))
                    .collect::<rusqlite::Result<Vec<rusqlite::types::Value>>>()
            })
            .map_err(|_| FAILED)?;
        tables.push(
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|_| FAILED)?,
        );
    }
    Ok(tables)
}

fn case_matches(case: &Case) -> Result<(bool, Value), &'static str> {
    let home = scheduler_conformance_tempdir()?;
    crate::daemon::ensure_private_coven_home(home.path()).map_err(|_| FAILED)?;
    let path = home.path().join("store.sqlite");
    let conn = crate::store::open_store(&path).map_err(|_| FAILED)?;
    let mut seed = case.definition.clone();
    seed["id"] = Value::String("capacity-preexisting".into());
    let seed_response = execute_definition_command(
        &conn,
        "adopt:capacity:baseline",
        DefinitionCommand::Create { definition: seed },
        FIRST_AT,
        CommandAuthority::OwnerLocal,
    )
    .map_err(|_| FAILED)?;
    if seed_response.outcome != DefinitionCommandOutcome::Committed {
        return Err(FAILED);
    }
    let stored_seed = crate::automations::store::get_definition(&conn, "capacity-preexisting")
        .map_err(|_| FAILED)?
        .ok_or(FAILED)?;
    let persisted_seed: RoutineDefinition =
        serde_json::from_str(&stored_seed.definition_json).map_err(|_| FAILED)?;
    let mut expected_seed = RoutineDefinition::from_json(&case.definition).map_err(|_| INVALID)?;
    expected_seed.id = "capacity-preexisting".into();
    if persisted_seed != expected_seed {
        return Err(FAILED);
    }
    // Owner-grant tables are initialized lazily. Initialize them through the
    // production owner-principal API before imposing the fixture quota.
    crate::automations::owner_grants::owner_principal_id(&conn, FIRST_AT).map_err(|_| FAILED)?;
    let baseline = snapshot(&conn)?;
    let failed = case.fault == Fault::SqliteMaxPageCount;
    if failed {
        let pages: i64 = conn
            .query_row("PRAGMA page_count", [], |row| row.get(0))
            .map_err(|_| FAILED)?;
        let free: i64 = conn
            .query_row("PRAGMA freelist_count", [], |row| row.get(0))
            .map_err(|_| FAILED)?;
        let size: i64 = conn
            .query_row("PRAGMA page_size", [], |row| row.get(0))
            .map_err(|_| FAILED)?;
        if free.saturating_mul(size) >= case.prompt_bytes as i64 {
            return Err(FAILED);
        }
        let quota: i64 = conn
            .query_row(&format!("PRAGMA max_page_count={pages}"), [], |row| {
                row.get(0)
            })
            .map_err(|_| FAILED)?;
        if quota != pages {
            return Err(FAILED);
        }
    }
    let initial = execute(&conn, case, FIRST_AT);
    let initial_outcome = match &initial {
        Ok(response) if response.outcome == DefinitionCommandOutcome::Committed => {
            InitialOutcome::Committed
        }
        Err(error)
            if error
                .downcast_ref::<rusqlite::Error>()
                .and_then(rusqlite::Error::sqlite_error_code)
                == Some(ErrorCode::DiskFull) =>
        {
            InitialOutcome::DiskFull
        }
        _ => return Err(FAILED),
    };
    let after_initial = snapshot(&conn)?;
    let mut passed = initial_outcome == case.expected.initial_outcome
        && (initial_outcome == InitialOutcome::DiskFull) == failed
        && (!failed || after_initial == baseline)
        && after_initial[0].len().checked_sub(baseline[0].len())
            == Some(case.expected.initial_rows as usize);
    conn.query_row("PRAGMA max_page_count=2147483646", [], |row| {
        row.get::<_, i64>(0)
    })
    .map_err(|_| FAILED)?;
    let committed = if failed {
        execute(&conn, case, FIRST_AT).map_err(|_| FAILED)?
    } else {
        initial.map_err(|_| FAILED)?
    };
    passed &= committed.outcome == DefinitionCommandOutcome::Committed
        && committed.result.is_some()
        && committed.event_ref.is_some()
        && committed.error.is_none();
    let committed_snapshot = snapshot(&conn)?;
    let event_ref =
        serde_json::to_value(committed.event_ref.as_ref().ok_or(FAILED)?).map_err(|_| FAILED)?;
    let (kind, stream_id) = event_ref["stream"]
        .as_str()
        .and_then(|stream| stream.split_once('/'))
        .ok_or(FAILED)?;
    passed &= kind == "automation" && Some(stream_id) == case.definition["id"].as_str();
    let sequence = event_ref["sequence"].as_i64().ok_or(FAILED)?;
    let event_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM automation_events WHERE stream_kind=?1 AND stream_id=?2 AND sequence=?3",
        rusqlite::params![kind, stream_id, sequence], |row| row.get(0)).map_err(|_| FAILED)?;
    passed &= event_count == 1;

    // Exactly one definition, adoption and event were added; every pre-existing row survives.
    for (index, (before, after)) in baseline.iter().zip(&committed_snapshot).enumerate() {
        passed &= before.iter().all(|row| after.contains(row));
        if [0, 4, 6].contains(&index) {
            passed &= after.len() == before.len() + case.expected.final_rows as usize;
        }
    }
    drop(conn);
    let reopened = crate::store::open_store(&path).map_err(|_| FAILED)?;
    let replay = execute(&reopened, case, "2026-08-30T09:01:00.000Z").map_err(|_| FAILED)?;
    let replay_preserved = replay.outcome == DefinitionCommandOutcome::Replayed
        && replay.error.is_none()
        && replay.revision == committed.revision
        && replay.result == committed.result
        && replay.event_ref == committed.event_ref
        && replay.replay_first_committed_at.as_deref() == Some(FIRST_AT);
    let definition =
        RoutineDefinition::from_json(&expanded_definition(case)).map_err(|_| INVALID)?;
    let stored = crate::automations::store::get_definition(&reopened, &definition.id)
        .map_err(|_| FAILED)?
        .ok_or(FAILED)?;
    let persisted: RoutineDefinition =
        serde_json::from_str(&stored.definition_json).map_err(|_| FAILED)?;
    passed &= replay_preserved == case.expected.replay_preserved
        && persisted == definition
        && snapshot(&reopened)? == committed_snapshot;
    Ok((
        passed,
        json!({
            "faultSource": if failed { "sqlite_max_page_count" } else { "none" },
            "initialOutcome": if initial_outcome == InitialOutcome::DiskFull { "disk_full" } else { "committed" },
            "sqliteErrorCode": if initial_outcome == InitialOutcome::DiskFull { Some(13) } else { None },
            "baselinePreserved": !failed || after_initial == baseline,
            "initialAddedDefinitions": after_initial[0].len().checked_sub(baseline[0].len()),
            "finalAddedDefinitions": committed_snapshot[0].len().checked_sub(baseline[0].len()),
            "replayPreserved": replay_preserved,
            "replayWritesUnchanged": snapshot(&reopened)? == committed_snapshot
        }),
    ))
}
