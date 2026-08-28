//! SQLite persistence for routine definitions (coven#816).
//!
//! Definitions live in the single Coven store as `definition_json` rows. The
//! scheduler and run ledger join on `id`; the definition row is the identity
//! anchor, so updates mutate in place while the id stays stable.

use anyhow::{Context, Result};
use chrono::{SecondsFormat, Utc};
use rusqlite::{params, Connection};

use super::definition::RoutineDefinition;

pub const AUTOMATION_DEFINITIONS_SCHEMA_SQL: &str = "
    CREATE TABLE IF NOT EXISTS automation_definitions (
        id TEXT PRIMARY KEY NOT NULL,
        name TEXT NOT NULL,
        status TEXT NOT NULL,
        definition_json TEXT NOT NULL,
        created_at TEXT NOT NULL,
        updated_at TEXT NOT NULL
    );

    CREATE INDEX IF NOT EXISTS idx_automation_definitions_updated_at
        ON automation_definitions(updated_at DESC);
";

#[allow(dead_code)]
pub struct RoutineRecord {
    pub id: String,
    pub name: String,
    pub status: String,
    pub definition_json: String,
    pub created_at: String,
    pub updated_at: String,
}

fn now_iso() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

pub fn list_definitions(conn: &Connection) -> Result<Vec<RoutineRecord>> {
    let mut statement = conn
        .prepare(
            "SELECT id, name, status, definition_json, created_at, updated_at
             FROM automation_definitions
             ORDER BY name ASC, id ASC",
        )
        .context("failed to prepare routine list query")?;
    let rows = statement
        .query_map([], |row| {
            Ok(RoutineRecord {
                id: row.get(0)?,
                name: row.get(1)?,
                status: row.get(2)?,
                definition_json: row.get(3)?,
                created_at: row.get(4)?,
                updated_at: row.get(5)?,
            })
        })
        .context("failed to list routine definitions")?;

    let mut records = Vec::new();
    for row in rows {
        records.push(row.context("failed to read routine row")?);
    }
    Ok(records)
}

pub fn get_definition(conn: &Connection, id: &str) -> Result<Option<RoutineRecord>> {
    let mut statement = conn
        .prepare(
            "SELECT id, name, status, definition_json, created_at, updated_at
             FROM automation_definitions
             WHERE id = ?1",
        )
        .context("failed to prepare routine get query")?;
    let mut rows = statement
        .query_map(params![id], |row| {
            Ok(RoutineRecord {
                id: row.get(0)?,
                name: row.get(1)?,
                status: row.get(2)?,
                definition_json: row.get(3)?,
                created_at: row.get(4)?,
                updated_at: row.get(5)?,
            })
        })
        .context("failed to get routine definition")?;

    match rows.next() {
        Some(row) => Ok(Some(row.context("failed to read routine row")?)),
        None => Ok(None),
    }
}

pub fn insert_definition(
    conn: &Connection,
    definition: &RoutineDefinition,
) -> Result<RoutineRecord> {
    let now = now_iso();
    let definition_json =
        serde_json::to_string(definition).context("failed to serialize routine definition")?;
    conn.execute(
        "INSERT INTO automation_definitions
            (id, name, status, definition_json, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?5)",
        params![
            definition.id,
            definition.name,
            status_text(definition.status),
            definition_json,
            now,
        ],
    )
    .context("failed to insert routine definition")?;
    Ok(RoutineRecord {
        id: definition.id.clone(),
        name: definition.name.clone(),
        status: status_text(definition.status).to_string(),
        definition_json,
        created_at: now.clone(),
        updated_at: now,
    })
}

pub fn update_definition(
    conn: &Connection,
    definition: &RoutineDefinition,
) -> Result<Option<RoutineRecord>> {
    let updated_at = now_iso();
    let definition_json =
        serde_json::to_string(definition).context("failed to serialize routine definition")?;
    let changed = conn
        .execute(
            "UPDATE automation_definitions
             SET name = ?2,
                 status = ?3,
                 definition_json = ?4,
                 updated_at = ?5
             WHERE id = ?1",
            params![
                definition.id,
                definition.name,
                status_text(definition.status),
                definition_json,
                updated_at,
            ],
        )
        .context("failed to update routine definition")?;
    if changed == 0 {
        return Ok(None);
    }
    let record = get_definition(conn, &definition.id)?
        .ok_or_else(|| anyhow::anyhow!("routine vanished during update"))?;
    Ok(Some(record))
}

pub fn delete_definition(conn: &Connection, id: &str) -> Result<bool> {
    let changed = conn
        .execute(
            "DELETE FROM automation_definitions WHERE id = ?1",
            params![id],
        )
        .context("failed to delete routine definition")?;
    Ok(changed > 0)
}

fn status_text(status: super::definition::RoutineStatus) -> &'static str {
    match status {
        super::definition::RoutineStatus::Active => "ACTIVE",
        super::definition::RoutineStatus::Paused => "PAUSED",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::automations::definition::{RoutineDefinition, RoutineStatus};
    use crate::store::initialize_store;
    use serde_json::json;

    fn temp_store() -> (tempfile::TempDir, Connection) {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("store.sqlite");
        initialize_store(&path).unwrap();
        let conn = crate::store::open_store(&path).unwrap();
        (temp, conn)
    }

    fn definition(id: &str) -> RoutineDefinition {
        RoutineDefinition::from_json(&json!({
            "schemaVersion": 1,
            "id": id,
            "name": "Test routine",
            "status": "PAUSED",
            "rrule": "FREQ=DAILY;BYHOUR=9",
            "timezone": "local",
            "misfire": "latest",
            "overlap": "forbid",
            "timeoutMinutes": 30,
            "runtime": "coven-code",
            "prompt": "Do the thing."
        }))
        .unwrap()
    }

    #[test]
    fn insert_get_list_update_delete_round_trip() {
        let (_temp, conn) = temp_store();
        let inserted = insert_definition(&conn, &definition("round-trip")).unwrap();
        assert_eq!(inserted.status, "PAUSED");

        let fetched = get_definition(&conn, "round-trip").unwrap().unwrap();
        assert_eq!(fetched.id, "round-trip");

        let listed = list_definitions(&conn).unwrap();
        assert_eq!(listed.len(), 1);

        let mut active = definition("round-trip");
        active.status = RoutineStatus::Active;
        let updated = update_definition(&conn, &active).unwrap().unwrap();
        assert_eq!(updated.status, "ACTIVE");

        assert!(delete_definition(&conn, "round-trip").unwrap());
        assert!(get_definition(&conn, "round-trip").unwrap().is_none());
    }

    #[test]
    fn update_of_missing_id_reports_none() {
        let (_temp, conn) = temp_store();
        let updated = update_definition(&conn, &definition("missing")).unwrap();
        assert!(updated.is_none());
    }

    #[test]
    fn delete_of_missing_id_reports_false() {
        let (_temp, conn) = temp_store();
        assert!(!delete_definition(&conn, "missing").unwrap());
    }
}
