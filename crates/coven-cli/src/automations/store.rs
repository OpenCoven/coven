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
        revision INTEGER NOT NULL DEFAULT 1 CHECK (revision >= 1),
        definition_digest TEXT,
        lifecycle_state TEXT NOT NULL DEFAULT 'draft'
            CHECK (lifecycle_state IN ('draft', 'paused', 'active', 'disabled', 'invalid')),
        tombstoned_at TEXT,
        authority_version INTEGER NOT NULL DEFAULT 0 CHECK (authority_version IN (0, 1)),
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
    pub revision: u64,
    pub definition_digest: Option<String>,
    pub lifecycle_state: String,
    pub tombstoned_at: Option<String>,
    pub authority_version: u8,
    pub created_at: String,
    pub updated_at: String,
}

fn revision_from_row(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<u64> {
    let revision = row.get::<_, i64>(index)?;
    u64::try_from(revision).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            index,
            rusqlite::types::Type::Integer,
            Box::new(error),
        )
    })
}

pub(crate) fn ensure_definition_command_columns(conn: &Connection) -> Result<()> {
    let columns = conn
        .prepare("PRAGMA table_info(automation_definitions)")
        .context("failed to inspect automation_definitions schema")?
        .query_map([], |row| row.get::<_, String>(1))
        .context("failed to enumerate automation_definitions columns")?
        .collect::<rusqlite::Result<Vec<_>>>()
        .context("failed to read automation_definitions columns")?;
    if !columns.iter().any(|column| column == "revision") {
        conn.execute(
            "ALTER TABLE automation_definitions
             ADD COLUMN revision INTEGER NOT NULL DEFAULT 1 CHECK (revision >= 1)",
            [],
        )
        .context("failed to add automation definition revision column")?;
    }
    if !columns.iter().any(|column| column == "tombstoned_at") {
        conn.execute(
            "ALTER TABLE automation_definitions ADD COLUMN tombstoned_at TEXT",
            [],
        )
        .context("failed to add automation definition tombstone column")?;
    }
    if !columns.iter().any(|column| column == "authority_version") {
        conn.execute(
            "ALTER TABLE automation_definitions
             ADD COLUMN authority_version INTEGER NOT NULL DEFAULT 0
             CHECK (authority_version IN (0, 1))",
            [],
        )
        .context("failed to add automation definition authority version column")?;
    }
    Ok(())
}

fn now_iso() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

pub fn list_definitions(conn: &Connection) -> Result<Vec<RoutineRecord>> {
    list_definitions_with_tombstones(conn, false)
}

pub fn list_definitions_with_tombstones(
    conn: &Connection,
    include_tombstoned: bool,
) -> Result<Vec<RoutineRecord>> {
    let query = if include_tombstoned {
        "SELECT id, name, status, definition_json, revision, definition_digest, lifecycle_state,
                tombstoned_at, authority_version, created_at, updated_at
         FROM automation_definitions
         ORDER BY name ASC, id ASC"
    } else {
        "SELECT id, name, status, definition_json, revision, definition_digest, lifecycle_state,
                tombstoned_at, authority_version, created_at, updated_at
         FROM automation_definitions
         WHERE tombstoned_at IS NULL
         ORDER BY name ASC, id ASC"
    };
    let mut statement = conn
        .prepare(query)
        .context("failed to prepare routine list query")?;
    let rows = statement
        .query_map([], routine_record_from_row)
        .context("failed to list routine definitions")?;

    let mut records = Vec::new();
    for row in rows {
        records.push(row.context("failed to read routine row")?);
    }
    Ok(records)
}

pub fn get_definition(conn: &Connection, id: &str) -> Result<Option<RoutineRecord>> {
    get_definition_with_tombstone(conn, id, false)
}

pub fn get_definition_with_tombstone(
    conn: &Connection,
    id: &str,
    include_tombstoned: bool,
) -> Result<Option<RoutineRecord>> {
    let query = if include_tombstoned {
        "SELECT id, name, status, definition_json, revision, definition_digest, lifecycle_state,
                tombstoned_at, authority_version, created_at, updated_at
         FROM automation_definitions
         WHERE id = ?1"
    } else {
        "SELECT id, name, status, definition_json, revision, definition_digest, lifecycle_state,
                tombstoned_at, authority_version, created_at, updated_at
         FROM automation_definitions
         WHERE id = ?1 AND tombstoned_at IS NULL"
    };
    let mut statement = conn
        .prepare(query)
        .context("failed to prepare routine get query")?;
    let mut rows = statement
        .query_map(params![id], routine_record_from_row)
        .context("failed to get routine definition")?;

    match rows.next() {
        Some(row) => Ok(Some(row.context("failed to read routine row")?)),
        None => Ok(None),
    }
}

fn routine_record_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RoutineRecord> {
    Ok(RoutineRecord {
        id: row.get(0)?,
        name: row.get(1)?,
        status: row.get(2)?,
        definition_json: row.get(3)?,
        revision: revision_from_row(row, 4)?,
        definition_digest: row.get(5)?,
        lifecycle_state: row.get(6)?,
        tombstoned_at: row.get(7)?,
        authority_version: row.get(8)?,
        created_at: row.get(9)?,
        updated_at: row.get(10)?,
    })
}

pub fn insert_definition(
    conn: &Connection,
    definition: &RoutineDefinition,
) -> Result<RoutineRecord> {
    let now = now_iso();
    let definition_json =
        serde_json::to_string(definition).context("failed to serialize routine definition")?;
    let definition_digest = super::contract::migration::definition_digest(&definition_json)?;
    let lifecycle_state =
        super::contract::migration::lifecycle_state(status_text(definition.status));
    conn.execute(
        "INSERT INTO automation_definitions
            (id, name, status, definition_json, revision, definition_digest, lifecycle_state,
             tombstoned_at, authority_version, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, 1, ?5, ?6, NULL, 0, ?7, ?7)",
        params![
            definition.id,
            definition.name,
            status_text(definition.status),
            definition_json,
            definition_digest,
            lifecycle_state,
            now,
        ],
    )
    .context("failed to insert routine definition")?;
    Ok(RoutineRecord {
        id: definition.id.clone(),
        name: definition.name.clone(),
        status: status_text(definition.status).to_string(),
        definition_json,
        revision: 1,
        definition_digest: Some(definition_digest),
        lifecycle_state: lifecycle_state.to_string(),
        tombstoned_at: None,
        authority_version: 0,
        created_at: now.clone(),
        updated_at: now,
    })
}

#[allow(dead_code)]
pub fn update_definition(
    conn: &Connection,
    definition: &RoutineDefinition,
) -> Result<Option<RoutineRecord>> {
    let updated_at = now_iso();
    let definition_json =
        serde_json::to_string(definition).context("failed to serialize routine definition")?;
    let definition_digest = super::contract::migration::definition_digest(&definition_json)?;
    let lifecycle_state =
        super::contract::migration::lifecycle_state(status_text(definition.status));
    let changed = conn
        .execute(
            "UPDATE automation_definitions
             SET name = ?2,
                 status = ?3,
                 definition_json = ?4,
                 definition_digest = ?5,
                 lifecycle_state = ?6,
                 revision = revision + 1,
                 updated_at = ?7
             WHERE id = ?1 AND tombstoned_at IS NULL AND authority_version = 0",
            params![
                definition.id,
                definition.name,
                status_text(definition.status),
                definition_json,
                definition_digest,
                lifecycle_state,
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

#[cfg(test)]
pub(crate) fn remove_definition_for_test(conn: &Connection, id: &str) -> Result<bool> {
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

        assert!(remove_definition_for_test(&conn, "round-trip").unwrap());
        assert!(get_definition(&conn, "round-trip").unwrap().is_none());
    }

    #[test]
    fn update_of_missing_id_reports_none() {
        let (_temp, conn) = temp_store();
        let updated = update_definition(&conn, &definition("missing")).unwrap();
        assert!(updated.is_none());
    }

    #[test]
    fn test_only_remove_of_missing_id_reports_false() {
        let (_temp, conn) = temp_store();
        assert!(!remove_definition_for_test(&conn, "missing").unwrap());
    }

    #[test]
    fn initialization_adds_command_metadata_to_legacy_definitions() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("legacy.sqlite");
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE automation_definitions (
                id TEXT PRIMARY KEY NOT NULL,
                name TEXT NOT NULL,
                status TEXT NOT NULL,
                definition_json TEXT NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );",
        )
        .unwrap();
        let legacy = definition("legacy");
        conn.execute(
            "INSERT INTO automation_definitions (
                id, name, status, definition_json, created_at, updated_at
             ) VALUES (?1, ?2, 'PAUSED', ?3, ?4, ?4)",
            params![
                legacy.id,
                legacy.name,
                serde_json::to_string(&legacy).unwrap(),
                "2026-09-01T00:00:00.000Z",
            ],
        )
        .unwrap();
        drop(conn);

        initialize_store(&path).unwrap();
        let conn = crate::store::open_store(&path).unwrap();
        let migrated = get_definition(&conn, "legacy").unwrap().unwrap();
        assert_eq!(migrated.revision, 1);
        let (tombstoned_at, authority_version): (Option<String>, i64) = conn
            .query_row(
                "SELECT tombstoned_at, authority_version
                 FROM automation_definitions
                 WHERE id = 'legacy'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert!(tombstoned_at.is_none());
        assert_eq!(authority_version, 0);
    }

    #[test]
    fn initialization_migrates_legacy_automation_history_without_rewriting_it() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("legacy-contract.sqlite");
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "PRAGMA foreign_keys = ON;
             CREATE TABLE automation_definitions (
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
                kind TEXT NOT NULL DEFAULT 'scheduled',
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
                finished_at TEXT,
                FOREIGN KEY (occurrence_id) REFERENCES automation_occurrences(id) ON DELETE SET NULL
             );",
        )
        .unwrap();
        let legacy_definition = r#"{ "schemaVersion": 1, "id": "legacy", "name": "Legacy", "status": "ACTIVE", "rrule": "FREQ=DAILY;BYHOUR=9", "timezone": "local", "misfire": "latest", "overlap": "forbid", "timeoutMinutes": 30, "runtime": "coven-code", "prompt": "Preserve these exact bytes." }"#;
        conn.execute(
            "INSERT INTO automation_definitions (
                id, name, status, definition_json, created_at, updated_at
             ) VALUES ('legacy', 'Legacy', 'ACTIVE', ?1, ?2, ?2)",
            params![legacy_definition, "2026-09-01T00:00:00.000Z"],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO automation_occurrences (
                id, automation_id, scheduled_for, kind, state, attempt, created_at, updated_at
             ) VALUES ('occ-legacy', 'legacy', ?1, 'scheduled', 'succeeded', 1, ?1, ?2)",
            params!["2026-09-01T09:00:00.000Z", "2026-09-01T09:05:00.000Z"],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO automation_runs (
                id, automation_id, occurrence_id, session_id, familiar_id, runtime, status,
                exit_code, log_json, output_commit, started_at, finished_at
             ) VALUES (
                'run-legacy', 'legacy', 'occ-legacy', 'session-legacy', 'cody',
                'coven-code', 'succeeded', 0, '[\"done\"]', 'committed', ?1, ?2
             )",
            params!["2026-09-01T09:00:01.000Z", "2026-09-01T09:05:00.000Z"],
        )
        .unwrap();
        drop(conn);

        initialize_store(&path).unwrap();
        let conn = crate::store::open_initialized_store(&path).unwrap();
        let legacy_json: serde_json::Value = serde_json::from_str(legacy_definition).unwrap();
        let expected_digest =
            crate::automations::contract::canonical_json::sha256_digest(&legacy_json).unwrap();
        let definition: (String, i64, String, String) = conn
            .query_row(
                "SELECT definition_json, revision, definition_digest, lifecycle_state
                 FROM automation_definitions
                 WHERE id = 'legacy'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(definition.0, legacy_definition);
        assert_eq!(definition.1, 1);
        assert_eq!(definition.2, expected_digest);
        assert_eq!(definition.3, "active");

        let occurrence: (i64, String, String) = conn
            .query_row(
                "SELECT automation_revision, definition_digest, state
                 FROM automation_occurrences
                 WHERE id = 'occ-legacy'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(
            occurrence,
            (1, expected_digest.clone(), "succeeded".to_string())
        );

        let run: (i64, String, Option<String>) = conn
            .query_row(
                "SELECT automation_revision, definition_digest, receipt_id
                 FROM automation_runs
                 WHERE id = 'run-legacy'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(run, (1, expected_digest, None));

        for table in [
            "automation_definitions",
            "automation_occurrences",
            "automation_runs",
        ] {
            let count: i64 = conn
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(count, 1, "{table} row count changed");
        }
        conn.execute_batch(
            "CREATE TABLE automation_migration_writes (
                table_name TEXT NOT NULL,
                operation TEXT NOT NULL
             );
             CREATE TRIGGER track_definition_update
             AFTER UPDATE ON automation_definitions
             BEGIN
                INSERT INTO automation_migration_writes VALUES ('automation_definitions', 'update');
             END;
             CREATE TRIGGER track_occurrence_update
             AFTER UPDATE ON automation_occurrences
             BEGIN
                INSERT INTO automation_migration_writes VALUES ('automation_occurrences', 'update');
             END;
             CREATE TRIGGER track_run_update
             AFTER UPDATE ON automation_runs
             BEGIN
                INSERT INTO automation_migration_writes VALUES ('automation_runs', 'update');
             END;",
        )
        .unwrap();
        drop(conn);

        initialize_store(&path).unwrap();
        let conn = crate::store::open_initialized_store(&path).unwrap();
        let migrated_rows: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM automation_contract_migrations
                 WHERE profile = 'coven.automations.v1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(migrated_rows, 1);
        let write_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM automation_migration_writes",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(write_count, 0);
    }

    #[test]
    fn migration_does_not_attribute_old_history_to_a_recreated_definition() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("recreated.sqlite");
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
                kind TEXT NOT NULL DEFAULT 'scheduled',
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
             );
             INSERT INTO automation_definitions VALUES (
                'recreated', 'Recreated', 'PAUSED',
                '{\"schemaVersion\":1,\"id\":\"recreated\",\"name\":\"Recreated\",\"status\":\"PAUSED\",\"rrule\":\"FREQ=DAILY;BYHOUR=9\",\"timezone\":\"local\",\"misfire\":\"latest\",\"overlap\":\"forbid\",\"timeoutMinutes\":30,\"runtime\":\"coven-code\",\"prompt\":\"new body\"}',
                '2026-09-02T00:00:00.000Z', '2026-09-02T00:00:00.000Z'
             );
             INSERT INTO automation_occurrences VALUES (
                'old-occurrence', 'recreated', '2026-09-01T09:00:00.000Z', 'scheduled',
                'skipped', NULL, NULL, 0, 'superseded by latest misfire policy',
                '2026-09-01T09:00:00.000Z', '2026-09-01T09:01:00.000Z'
             );
             INSERT INTO automation_runs VALUES (
                'old-run', 'recreated', 'old-occurrence', NULL, NULL, 'coven-code',
                'failed', 1, NULL, NULL, '2026-09-01T09:00:01.000Z',
                '2026-09-01T09:01:00.000Z'
             );",
        )
        .unwrap();
        drop(conn);

        initialize_store(&path).unwrap();
        let conn = crate::store::open_initialized_store(&path).unwrap();
        let occurrence: (i64, Option<String>, String) = conn
            .query_row(
                "SELECT automation_revision, definition_digest, state
                 FROM automation_occurrences
                 WHERE id = 'old-occurrence'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(occurrence, (1, None, "superseded".to_string()));
        let run: (i64, Option<String>) = conn
            .query_row(
                "SELECT automation_revision, definition_digest
                 FROM automation_runs
                 WHERE id = 'old-run'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(run, (1, None));
        let unresolved: (i64, i64) = conn
            .query_row(
                "SELECT unverifiable_occurrences, unverifiable_runs
                 FROM automation_contract_migrations
                 WHERE profile = 'coven.automations.v1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(unresolved, (1, 1));
    }

    #[test]
    fn migration_retains_malformed_legacy_definitions_as_unverifiable() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("malformed.sqlite");
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
                kind TEXT NOT NULL DEFAULT 'scheduled',
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
             );
             INSERT INTO automation_definitions VALUES (
                'malformed', 'Malformed', 'ACTIVE', '{not-json',
                '2026-09-01T00:00:00.000Z', '2026-09-01T00:00:00.000Z'
             );",
        )
        .unwrap();
        drop(conn);

        initialize_store(&path).unwrap();
        let conn = crate::store::open_initialized_store(&path).unwrap();
        let migrated: (String, Option<String>, String) = conn
            .query_row(
                "SELECT definition_json, definition_digest, lifecycle_state
                 FROM automation_definitions
                 WHERE id = 'malformed'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(
            migrated,
            ("{not-json".to_string(), None, "invalid".to_string())
        );
        let unverifiable: i64 = conn
            .query_row(
                "SELECT unverifiable_definitions
                 FROM automation_contract_migrations
                 WHERE profile = 'coven.automations.v1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(unverifiable, 1);
    }
}
