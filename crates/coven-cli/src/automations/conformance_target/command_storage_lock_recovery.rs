//! Real SQLite write contention at the command-adoption boundary. No runtime
//! launches or authority claims: both cases use disposable owner-local stores.
use std::collections::BTreeSet;
use std::time::Duration;

use rusqlite::{Connection, ErrorCode};
use serde::Deserialize;
use serde_json::Value;

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
    hold_write_lock: bool,
    adoption_key: String,
    definition: Value,
    expected: Expected,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Expected {
    initial_outcome: InitialOutcome,
    initial_rows: u64,
    final_rows: u64,
    replay_preserved: bool,
}

#[derive(Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
enum InitialOutcome {
    Busy,
    Committed,
}

pub(super) fn evaluate(vector: &Value) -> Result<bool, &'static str> {
    let vectors: VectorSet = serde_json::from_value(vector.clone()).map_err(|_| INVALID)?;
    if vectors.schema_version != "coven.automations.command-storage-lock-recovery-vectors.v1"
        || vectors.cases.len() != 2
    {
        return Err(INVALID);
    }
    let mut ids = BTreeSet::new();
    let mut locks = BTreeSet::new();
    for case in &vectors.cases {
        RoutineDefinition::from_json(&case.definition).map_err(|_| INVALID)?;
        if !valid_case_id(&case.case_id)
            || !ids.insert(&case.case_id)
            || !locks.insert(case.hold_write_lock)
            || AdoptionKey::new(case.adoption_key.clone()).is_err()
            || case.expected.final_rows != 1
            || !case.expected.replay_preserved
            || case.expected.initial_rows != u64::from(!case.hold_write_lock)
            || (case.expected.initial_outcome == InitialOutcome::Busy) != case.hold_write_lock
        {
            return Err(INVALID);
        }
    }
    let mut passed = true;
    for case in &vectors.cases {
        passed &= case_matches(case)?;
    }
    Ok(passed)
}

fn execute(conn: &Connection, case: &Case, at: &str) -> anyhow::Result<DefinitionCommandResponse> {
    execute_definition_command(
        conn,
        &case.adoption_key,
        DefinitionCommand::Create {
            definition: case.definition.clone(),
        },
        at,
        CommandAuthority::OwnerLocal,
    )
}

fn rows_match(conn: &Connection, expected: u64) -> Result<bool, &'static str> {
    let mut matches = true;
    for query in [
        "SELECT COUNT(*) FROM automation_definitions",
        "SELECT COUNT(*) FROM automation_command_adoptions",
        "SELECT COUNT(*) FROM automation_events",
    ] {
        let count: i64 = conn
            .query_row(query, [], |row| row.get(0))
            .map_err(|_| FAILED)?;
        matches &= u64::try_from(count).ok() == Some(expected);
    }
    Ok(matches)
}

fn case_matches(case: &Case) -> Result<bool, &'static str> {
    let home = scheduler_conformance_tempdir()?;
    crate::daemon::ensure_private_coven_home(home.path()).map_err(|_| FAILED)?;
    let path = home.path().join("store.sqlite");
    let conn = crate::store::open_store(&path).map_err(|_| FAILED)?;
    // This fixture already owns the competing lock before it calls the API.
    // Zero wait makes SQLITE_BUSY deterministic, without scheduler timing or
    // any change to production connection deadlines.
    conn.busy_timeout(Duration::ZERO).map_err(|_| FAILED)?;
    let blocker = Connection::open(&path).map_err(|_| FAILED)?;
    if case.hold_write_lock {
        blocker
            .execute_batch("BEGIN IMMEDIATE")
            .map_err(|_| FAILED)?;
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
                == Some(ErrorCode::DatabaseBusy) =>
        {
            InitialOutcome::Busy
        }
        _ => return Ok(false),
    };
    let mut passed = initial_outcome == case.expected.initial_outcome
        && rows_match(&conn, case.expected.initial_rows)?;
    // Also bind the injected fault to its actual observation, independently
    // of caller-supplied expectations.
    passed &= (initial_outcome == InitialOutcome::Busy) == case.hold_write_lock;
    if case.hold_write_lock {
        blocker.execute_batch("ROLLBACK").map_err(|_| FAILED)?;
    }
    drop(blocker);
    let committed = if case.hold_write_lock {
        execute(&conn, case, FIRST_AT).map_err(|_| FAILED)?
    } else {
        initial.map_err(|_| FAILED)?
    };
    passed &= committed.outcome == DefinitionCommandOutcome::Committed
        && committed.result.is_some()
        && committed.event_ref.is_some()
        && committed.error.is_none();
    drop(conn);
    let reopened = crate::store::open_store(&path).map_err(|_| FAILED)?;
    let replay = execute(&reopened, case, "2026-08-30T09:01:00.000Z").map_err(|_| FAILED)?;
    let replay_preserved = replay.outcome == DefinitionCommandOutcome::Replayed
        && replay.error.is_none()
        && replay.revision == committed.revision
        && replay.result == committed.result
        && replay.event_ref == committed.event_ref
        && replay.replay_first_committed_at.as_deref() == Some(FIRST_AT);
    let definition = RoutineDefinition::from_json(&case.definition).map_err(|_| INVALID)?;
    let stored = crate::automations::store::get_definition(&reopened, &definition.id)
        .map_err(|_| FAILED)?
        .ok_or(FAILED)?;
    let persisted: RoutineDefinition =
        serde_json::from_str(&stored.definition_json).map_err(|_| FAILED)?;
    passed &= replay_preserved == case.expected.replay_preserved
        && persisted == definition
        && rows_match(&reopened, case.expected.final_rows)?;
    Ok(passed)
}
