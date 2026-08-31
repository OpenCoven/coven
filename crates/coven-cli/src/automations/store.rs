//! SQLite persistence for routine definitions (coven#816).
//!
//! Definitions live in the single Coven store as `definition_json` rows. The
//! scheduler and run ledger join on `id`; the definition row is the identity
//! anchor, so updates mutate in place while the id stays stable.

use anyhow::{Context, Result};
use chrono::{SecondsFormat, Utc};
use rusqlite::{params, Connection};
use sha2::{Digest, Sha256};

use super::definition::RoutineDefinition;

pub const AUTOMATION_DEFINITIONS_SCHEMA_SQL: &str = "
    CREATE TABLE IF NOT EXISTS automation_definitions (
        id TEXT PRIMARY KEY NOT NULL,
        name TEXT NOT NULL,
        status TEXT NOT NULL,
        definition_json TEXT NOT NULL,
        revision INTEGER NOT NULL DEFAULT 1,
        definition_digest TEXT NOT NULL,
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
    pub revision: i64,
    pub definition_digest: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DefinitionSnapshot {
    pub revision: i64,
    pub digest: String,
    pub definition_json: String,
    pub output_target: Option<String>,
}

fn now_iso() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

pub fn list_definitions(conn: &Connection) -> Result<Vec<RoutineRecord>> {
    let mut statement = conn
        .prepare(
            "SELECT id, name, status, definition_json, revision, definition_digest,
                    created_at, updated_at
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
                revision: row.get(4)?,
                definition_digest: row.get(5)?,
                created_at: row.get(6)?,
                updated_at: row.get(7)?,
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
            "SELECT id, name, status, definition_json, revision, definition_digest,
                    created_at, updated_at
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
                revision: row.get(4)?,
                definition_digest: row.get(5)?,
                created_at: row.get(6)?,
                updated_at: row.get(7)?,
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
    let definition_digest = definition_digest(&definition_json);
    conn.execute(
        "INSERT INTO automation_definitions
            (id, name, status, definition_json, revision, definition_digest,
             created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, 1, ?5, ?6, ?6)",
        params![
            definition.id,
            definition.name,
            status_text(definition.status),
            definition_json,
            definition_digest,
            now
        ],
    )
    .context("failed to insert routine definition")?;
    Ok(RoutineRecord {
        id: definition.id.clone(),
        name: definition.name.clone(),
        status: status_text(definition.status).to_string(),
        definition_json,
        revision: 1,
        definition_digest,
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
    let definition_digest = definition_digest(&definition_json);
    let changed = conn
        .execute(
            "UPDATE automation_definitions
             SET name = ?2,
                 status = ?3,
                 definition_json = ?4,
                 revision = revision + 1,
                 definition_digest = ?5,
                 updated_at = ?6
             WHERE id = ?1",
            params![
                definition.id,
                definition.name,
                status_text(definition.status),
                definition_json,
                definition_digest,
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

pub fn definition_snapshot(record: &RoutineRecord) -> Result<DefinitionSnapshot> {
    let definition: RoutineDefinition = serde_json::from_str(&record.definition_json)
        .with_context(|| format!("stored routine `{}` is unreadable", record.id))?;
    Ok(DefinitionSnapshot {
        revision: record.revision,
        digest: record.definition_digest.clone(),
        definition_json: record.definition_json.clone(),
        output_target: definition.output_target,
    })
}

fn definition_digest(definition_json: &str) -> String {
    let digest = Sha256::digest(definition_json.as_bytes());
    let hex: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    format!("sha256:{hex}")
}

pub(crate) fn ensure_snapshot_schema(conn: &Connection) -> Result<()> {
    for (table, column, sql) in [
        (
            "automation_definitions",
            "revision",
            "ALTER TABLE automation_definitions
                 ADD COLUMN revision INTEGER NOT NULL DEFAULT 1",
        ),
        (
            "automation_definitions",
            "definition_digest",
            "ALTER TABLE automation_definitions
                 ADD COLUMN definition_digest TEXT NOT NULL DEFAULT ''",
        ),
        (
            "automation_occurrences",
            "definition_revision",
            "ALTER TABLE automation_occurrences ADD COLUMN definition_revision INTEGER",
        ),
        (
            "automation_occurrences",
            "definition_digest",
            "ALTER TABLE automation_occurrences ADD COLUMN definition_digest TEXT",
        ),
        (
            "automation_occurrences",
            "definition_json",
            "ALTER TABLE automation_occurrences ADD COLUMN definition_json TEXT",
        ),
        (
            "automation_occurrences",
            "output_target",
            "ALTER TABLE automation_occurrences ADD COLUMN output_target TEXT",
        ),
        (
            "automation_occurrences",
            "deadline_at",
            "ALTER TABLE automation_occurrences ADD COLUMN deadline_at TEXT",
        ),
        (
            "automation_runs",
            "definition_revision",
            "ALTER TABLE automation_runs ADD COLUMN definition_revision INTEGER",
        ),
        (
            "automation_runs",
            "definition_digest",
            "ALTER TABLE automation_runs ADD COLUMN definition_digest TEXT",
        ),
        (
            "automation_runs",
            "definition_json",
            "ALTER TABLE automation_runs ADD COLUMN definition_json TEXT",
        ),
        (
            "automation_runs",
            "output_target",
            "ALTER TABLE automation_runs ADD COLUMN output_target TEXT",
        ),
        (
            "automation_runs",
            "deadline_at",
            "ALTER TABLE automation_runs ADD COLUMN deadline_at TEXT",
        ),
    ] {
        ensure_column(conn, table, column, sql)?;
    }
    backfill_definition_digests(conn)?;
    backfill_historical_snapshots(conn)?;
    Ok(())
}

fn ensure_column(conn: &Connection, table: &str, column: &str, sql: &str) -> Result<()> {
    let mut statement = conn
        .prepare(&format!("PRAGMA table_info({table})"))
        .with_context(|| format!("failed to inspect {table} schema"))?;
    let exists = statement
        .query_map([], |row| row.get::<_, String>(1))
        .with_context(|| format!("failed to read {table} schema"))?
        .collect::<std::result::Result<Vec<_>, _>>()?
        .iter()
        .any(|name| name == column);
    if !exists {
        conn.execute(sql, [])
            .with_context(|| format!("failed to add {table}.{column}"))?;
    }
    Ok(())
}

fn backfill_definition_digests(conn: &Connection) -> Result<()> {
    let rows = {
        let mut statement = conn
            .prepare(
                "SELECT id, definition_json FROM automation_definitions
                     WHERE definition_digest = ''",
            )
            .context("failed to prepare routine digest backfill")?;
        let rows = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .context("failed to read routine digest backfill")?;
        rows.collect::<std::result::Result<Vec<_>, _>>()?
    };
    for (id, definition_json) in rows {
        conn.execute(
                "UPDATE automation_definitions
                 SET definition_digest = ?2, revision = CASE WHEN revision < 1 THEN 1 ELSE revision END
                 WHERE id = ?1",
                params![id, definition_digest(&definition_json)],
            )
            .with_context(|| format!("failed to backfill routine digest for {id}"))?;
    }
    Ok(())
}

fn backfill_historical_snapshots(conn: &Connection) -> Result<()> {
    let definitions = list_definitions(conn)?;
    for record in definitions {
        let snapshot = definition_snapshot(&record)?;
        conn.execute(
            "UPDATE automation_occurrences
                 SET definition_revision = COALESCE(definition_revision, ?2),
                     definition_digest = COALESCE(definition_digest, ?3),
                     definition_json = COALESCE(definition_json, ?4),
                     output_target = COALESCE(output_target, ?5),
                     deadline_at = COALESCE(deadline_at, lease_expires_at)
                 WHERE automation_id = ?1 AND definition_json IS NULL",
            params![
                record.id,
                snapshot.revision,
                snapshot.digest,
                snapshot.definition_json,
                snapshot.output_target
            ],
        )
        .with_context(|| format!("failed to backfill occurrence snapshots for {}", record.id))?;
        conn.execute(
            "UPDATE automation_runs
                 SET definition_revision = COALESCE(definition_revision, ?2),
                     definition_digest = COALESCE(definition_digest, ?3),
                     definition_json = COALESCE(definition_json, ?4),
                     output_target = COALESCE(output_target, ?5),
                     deadline_at = COALESCE(
                         deadline_at,
                         (SELECT deadline_at FROM automation_occurrences
                          WHERE id = automation_runs.occurrence_id)
                     )
                 WHERE automation_id = ?1 AND definition_json IS NULL",
            params![
                record.id,
                snapshot.revision,
                snapshot.digest,
                snapshot.definition_json,
                snapshot.output_target
            ],
        )
        .with_context(|| format!("failed to backfill run snapshots for {}", record.id))?;
    }
    Ok(())
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
        let (revision, digest): (i64, String) = conn
            .query_row(
                "SELECT revision, definition_digest
                 FROM automation_definitions WHERE id = 'round-trip'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(revision, 1);
        assert!(digest.starts_with("sha256:"), "{digest}");

        let fetched = get_definition(&conn, "round-trip").unwrap().unwrap();
        assert_eq!(fetched.id, "round-trip");

        let listed = list_definitions(&conn).unwrap();
        assert_eq!(listed.len(), 1);

        let mut active = definition("round-trip");
        active.status = RoutineStatus::Active;
        let updated = update_definition(&conn, &active).unwrap().unwrap();
        assert_eq!(updated.status, "ACTIVE");
        let updated_revision: i64 = conn
            .query_row(
                "SELECT revision FROM automation_definitions WHERE id = 'round-trip'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(updated_revision, 2);

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

    #[test]
    fn legacy_automation_tables_migrate_snapshot_columns_in_place() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("legacy.sqlite");
        let definition_json = serde_json::to_string(&definition("legacy")).unwrap();
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE automation_definitions (
                    id TEXT PRIMARY KEY NOT NULL,
                    name TEXT NOT NULL,
                    status TEXT NOT NULL,
                    definition_json TEXT NOT NULL,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL
                 );
                 CREATE TABLE automation_occurrences (
                    id TEXT PRIMARY KEY NOT NULL,
                    automation_id TEXT NOT NULL,
                    scheduled_for TEXT NOT NULL,
                    state TEXT NOT NULL DEFAULT 'planned',
                    lease_owner TEXT,
                    lease_expires_at TEXT,
                    attempt INTEGER NOT NULL DEFAULT 0,
                    failure_reason TEXT,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL,
                    UNIQUE(automation_id, scheduled_for)
                 );
                 CREATE TABLE automation_runs (
                    id TEXT PRIMARY KEY NOT NULL,
                    automation_id TEXT NOT NULL,
                    occurrence_id TEXT,
                    session_id TEXT,
                    familiar_id TEXT,
                    runtime TEXT,
                    status TEXT NOT NULL,
                    exit_code INTEGER,
                    log_json TEXT,
                    output_commit TEXT,
                    started_at TEXT NOT NULL,
                    finished_at TEXT
                 );",
            )
            .unwrap();
            conn.execute(
                "INSERT INTO automation_definitions
                    (id, name, status, definition_json, created_at, updated_at)
                 VALUES ('legacy', 'legacy', 'PAUSED', ?1,
                         '2026-01-01T00:00:00.000Z', '2026-01-01T00:00:00.000Z')",
                rusqlite::params![definition_json],
            )
            .unwrap();
        }

        initialize_store(&path).unwrap();
        let conn = crate::store::open_store(&path).unwrap();
        let (revision, digest): (i64, String) = conn
            .query_row(
                "SELECT revision, definition_digest
                 FROM automation_definitions WHERE id = 'legacy'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(revision, 1);
        assert!(digest.starts_with("sha256:"), "{digest}");
        for (table, column) in [
            ("automation_occurrences", "definition_json"),
            ("automation_occurrences", "deadline_at"),
            ("automation_runs", "definition_json"),
            ("automation_runs", "deadline_at"),
            ("automation_runs", "output_target"),
        ] {
            let found: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM pragma_table_info(?1) WHERE name = ?2",
                    rusqlite::params![table, column],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(found, 1, "{table}.{column} was not migrated");
        }
    }
}
