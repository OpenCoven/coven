//! Transaction aborts at event publication and final command adoption.
//! Uses private disposable stores; no runtime dispatch or authority claims.
use std::collections::BTreeSet;

use rusqlite::Connection;
use serde::Deserialize;
use serde_json::Value;

use super::{scheduler_conformance_tempdir, valid_case_id};
use crate::automations::command_adoption::{
    execute_definition_command, DefinitionCommand, DefinitionCommandOutcome,
    DefinitionCommandResponse,
};
use crate::automations::contract::types::AdoptionKey;
use crate::automations::owner_grants::CommandAuthority;

const INVALID: &str = "conformance vector is invalid";
const FAILED: &str = "conformance suite execution failed";
const FIRST_AT: &str = "2026-08-30T09:00:00.000Z";
const FAULT_MESSAGE: &str = "conformance transaction abort";

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
    adoption_key: String,
    definition: Value,
    expected: Expected,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Fault {
    EventInsert,
    AdoptionInsert,
    None,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Expected {
    initial_rows: u64,
    final_rows: u64,
    replay_preserved: bool,
}

pub(super) fn evaluate(vector: &Value) -> Result<bool, &'static str> {
    let vectors: VectorSet = serde_json::from_value(vector.clone()).map_err(|_| INVALID)?;
    if vectors.schema_version != "coven.automations.command-transaction-rollback-vectors.v1"
        || vectors.cases.len() != 3
    {
        return Err(INVALID);
    }
    let mut ids = BTreeSet::new();
    let mut faults = BTreeSet::new();
    for case in &vectors.cases {
        let projection =
            crate::automations::rich_definition::project(&case.definition).map_err(|_| INVALID)?;
        if !valid_case_id(&case.case_id)
            || !ids.insert(&case.case_id)
            || !faults.insert(case.fault)
            || AdoptionKey::new(case.adoption_key.clone()).is_err()
            || projection.revision != 1
            || projection.lifecycle_state != "draft"
            || case.expected.initial_rows != u64::from(case.fault == Fault::None)
            || case.expected.final_rows != 1
            || !case.expected.replay_preserved
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
        DefinitionCommand::RichCreate {
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
        "SELECT COUNT(*) FROM automation_rich_definition_revisions",
        "SELECT COUNT(*) FROM automation_events",
        "SELECT COUNT(*) FROM automation_event_stream_heads",
        "SELECT COUNT(*) FROM automation_command_adoptions",
    ] {
        let count: i64 = conn
            .query_row(query, [], |row| row.get(0))
            .map_err(|_| FAILED)?;
        matches &= u64::try_from(count).ok() == Some(expected);
    }
    Ok(matches)
}

fn is_injected_abort(result: &anyhow::Result<DefinitionCommandResponse>) -> bool {
    result.as_ref().err().is_some_and(|error| {
        error.chain().any(|cause| {
            matches!(cause.downcast_ref::<rusqlite::Error>(),
                Some(rusqlite::Error::SqliteFailure(code, Some(message)))
                    if code.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_TRIGGER
                        && message == FAULT_MESSAGE)
        })
    })
}

fn case_matches(case: &Case) -> Result<bool, &'static str> {
    let home = scheduler_conformance_tempdir()?;
    crate::daemon::ensure_private_coven_home(home.path()).map_err(|_| FAILED)?;
    let path = home.path().join("store.sqlite");
    let conn = crate::store::open_store(&path).map_err(|_| FAILED)?;
    // The table is selected exclusively by this enum, never by vector SQL.
    let table = match case.fault {
        Fault::EventInsert => Some("automation_events"),
        Fault::AdoptionInsert => Some("automation_command_adoptions"),
        Fault::None => None,
    };
    if let Some(table) = table {
        conn.execute_batch(&format!(
            "CREATE TRIGGER conformance_abort BEFORE INSERT ON {table}
             BEGIN SELECT RAISE(ABORT, '{FAULT_MESSAGE}'); END;"
        ))
        .map_err(|_| FAILED)?;
    }
    let initial = execute(&conn, case, FIRST_AT);
    let mut passed = rows_match(&conn, case.expected.initial_rows)?;
    let committed = if table.is_some() {
        passed &= is_injected_abort(&initial);
        drop(conn);
        let reopened = crate::store::open_store(&path).map_err(|_| FAILED)?;
        passed &= rows_match(&reopened, 0)?;
        // The durable trigger survives reopening: another same-key attempt
        // must hit the same fault and leave no partial publication behind.
        passed &= is_injected_abort(&execute(&reopened, case, FIRST_AT));
        passed &= rows_match(&reopened, 0)?;
        reopened
            .execute_batch("DROP TRIGGER conformance_abort")
            .map_err(|_| FAILED)?;
        execute(&reopened, case, FIRST_AT).map_err(|_| FAILED)?
    } else {
        drop(conn);
        initial.map_err(|_| FAILED)?
    };
    passed &= committed.outcome == DefinitionCommandOutcome::Committed
        && committed.revision == Some(1)
        && committed.result.is_some()
        && committed.event_ref.is_some()
        && committed.error.is_none();
    let reopened = crate::store::open_store(&path).map_err(|_| FAILED)?;
    let replay = execute(&reopened, case, "2026-08-30T09:01:00.000Z").map_err(|_| FAILED)?;
    passed &= replay.outcome == DefinitionCommandOutcome::Replayed
        && replay.error.is_none()
        && replay.result == committed.result
        && replay.revision == committed.revision
        && replay.event_ref == committed.event_ref
        && replay.replay_first_committed_at.as_deref() == Some(FIRST_AT)
        && rows_match(&reopened, case.expected.final_rows)?;
    let automation_id = case.definition["automationId"].as_str().ok_or(INVALID)?;
    let stored = crate::automations::rich_definition::current_view(&reopened, automation_id)
        .map_err(|_| FAILED)?;
    passed &= stored.as_ref() == Some(&case.definition);
    // Bind the durable event to the returned reference, rather than merely
    // counting an arbitrary event row alongside a nonempty response reference.
    let (stream_id, sequence, event_json): (String, i64, String) = reopened
        .query_row(
            "SELECT stream_id, sequence, event_json FROM automation_events",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(|_| FAILED)?;
    let event: Value = serde_json::from_str(&event_json).map_err(|_| FAILED)?;
    let reference = serde_json::to_value(&committed.event_ref).map_err(|_| FAILED)?;
    passed &= reference["stream"] == format!("automation/{automation_id}")
        && stream_id == automation_id
        && reference["sequence"] == sequence
        && event["payload"]["revision"] == 1;
    Ok(passed)
}
