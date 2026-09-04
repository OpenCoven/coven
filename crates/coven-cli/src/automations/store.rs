//! SQLite persistence for routine definitions (coven#816).
//!
//! Definitions live in the single Coven store as `definition_json` rows. The
//! scheduler and run ledger join on `id`; the definition row is the identity
//! anchor, so updates mutate in place while the id stays stable.

use anyhow::{Context, Result};
use chrono::{SecondsFormat, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value;

use super::definition::{RoutineDefinition, RoutineTimezone};

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

pub const AUTOMATION_TIMEZONE_MIGRATIONS_SCHEMA_SQL: &str = "
    CREATE TABLE IF NOT EXISTS automation_timezone_migrations (
        automation_id TEXT NOT NULL,
        from_revision INTEGER NOT NULL,
        to_revision INTEGER NOT NULL,
        from_timezone TEXT NOT NULL,
        to_timezone TEXT NOT NULL,
        previous_definition_json TEXT NOT NULL,
        previous_definition_digest TEXT,
        definition_digest TEXT NOT NULL,
        migrated_at TEXT NOT NULL,
        PRIMARY KEY (automation_id, from_revision)
    );
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

pub(crate) fn monotonic_definition_timestamp(
    conn: &Connection,
    automation_id: &str,
    candidate: &str,
) -> Result<String> {
    let mut latest = candidate.to_owned();
    let definition_timestamp = conn
        .query_row(
            "SELECT updated_at FROM automation_definitions WHERE id = ?1",
            [automation_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .context("failed to read automation definition timestamp")?;
    let event_timestamp = conn
        .query_row(
            "SELECT recorded_at
             FROM automation_events
             WHERE stream_kind = 'automation' AND stream_id = ?1
             ORDER BY recorded_at_millis DESC, sequence DESC
             LIMIT 1",
            [automation_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .context("failed to read automation event stream timestamp")?;
    for timestamp in [definition_timestamp, event_timestamp]
        .into_iter()
        .flatten()
    {
        latest = later_timestamp(&latest, &timestamp)?;
    }
    Ok(latest)
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
    let definition = definition
        .clone()
        .resolve_timezone_for_persistence()
        .map_err(anyhow::Error::msg)?;
    let now = now_iso();
    let definition_json =
        serde_json::to_string(&definition).context("failed to serialize routine definition")?;
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
    let definition = definition
        .clone()
        .resolve_timezone_for_persistence()
        .map_err(anyhow::Error::msg)?;
    let updated_at = now_iso();
    let definition_json =
        serde_json::to_string(&definition).context("failed to serialize routine definition")?;
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

pub fn migrate_durable_local_timezones(conn: &Connection) -> Result<()> {
    let owns_transaction = conn.is_autocommit();
    if owns_transaction {
        conn.execute_batch("BEGIN IMMEDIATE")
            .context("failed to begin automation timezone migration transaction")?;
    } else {
        conn.execute_batch("SAVEPOINT coven_automation_timezone_migration")
            .context("failed to create automation timezone migration savepoint")?;
    }

    let result = migrate_durable_local_timezones_in_transaction(conn);
    match result {
        Ok(()) => {
            if owns_transaction {
                if let Err(error) = conn.execute_batch("COMMIT") {
                    let _ = conn.execute_batch("ROLLBACK");
                    return Err(error)
                        .context("failed to commit automation timezone migration transaction");
                }
            } else {
                if let Err(error) =
                    conn.execute_batch("RELEASE SAVEPOINT coven_automation_timezone_migration")
                {
                    let _ = conn.execute_batch(
                        "ROLLBACK TO SAVEPOINT coven_automation_timezone_migration;
                         RELEASE SAVEPOINT coven_automation_timezone_migration;",
                    );
                    return Err(error)
                        .context("failed to release automation timezone migration savepoint");
                }
            }
            Ok(())
        }
        Err(error) => {
            if owns_transaction {
                let _ = conn.execute_batch("ROLLBACK");
            } else {
                let _ = conn.execute_batch(
                    "ROLLBACK TO SAVEPOINT coven_automation_timezone_migration;
                     RELEASE SAVEPOINT coven_automation_timezone_migration;",
                );
            }
            Err(error)
        }
    }
}

fn migrate_durable_local_timezones_in_transaction(conn: &Connection) -> Result<()> {
    conn.execute_batch(AUTOMATION_TIMEZONE_MIGRATIONS_SCHEMA_SQL)
        .context("failed to initialize automation timezone migration ledger")?;

    let rows: Vec<(String, u64, String, Option<String>, String)> = conn
        .prepare(
            "SELECT id, revision, definition_json, definition_digest,
                    CASE
                        WHEN tombstoned_at IS NULL THEN lifecycle_state
                        ELSE 'tombstoned'
                    END
             FROM automation_definitions
             ORDER BY id",
        )
        .context("failed to prepare durable local timezone migration")?
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                revision_from_row(row, 1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, String>(4)?,
            ))
        })
        .context("failed to query durable local timezone definitions")?
        .collect::<rusqlite::Result<_>>()
        .context("failed to read durable local timezone definitions")?;

    let mut local_rows = Vec::new();
    for (id, revision, definition_json, definition_digest, lifecycle_state) in rows {
        if let Some(stored_digest) = definition_digest.as_deref() {
            let canonical_digest = super::contract::migration::definition_digest(&definition_json)
                .with_context(|| format!("failed to verify stored definition digest for `{id}`"))?;
            if stored_digest != canonical_digest {
                anyhow::bail!(
                    "automation definition `{id}` definition digest mismatch before timezone migration"
                );
            }
        }
        let Ok(definition) = serde_json::from_str::<Value>(&definition_json) else {
            continue;
        };
        if definition.get("timezone").and_then(Value::as_str) == Some("local") {
            local_rows.push((
                id,
                revision,
                definition_json,
                definition,
                definition_digest,
                lifecycle_state,
            ));
        }
    }
    if local_rows.is_empty() {
        return Ok(());
    }

    let resolved = RoutineTimezone::Local
        .resolve_for_persistence()
        .map_err(anyhow::Error::msg)?;
    let resolved_name = resolved.as_str();
    let migration_now = now_iso();

    for (
        id,
        from_revision,
        previous_definition_json,
        mut definition,
        previous_digest,
        lifecycle_state,
    ) in local_rows
    {
        let migrated_at = monotonic_definition_timestamp(conn, &id, &migration_now)?;
        definition["timezone"] = Value::String(resolved_name.to_string());
        let definition_json = serde_json::to_string(&definition)
            .with_context(|| format!("failed to serialize timezone migration for `{id}`"))?;
        let definition_digest = super::contract::migration::definition_digest(&definition_json)?;
        let to_revision = from_revision
            .checked_add(1)
            .context("automation definition revision overflow during timezone migration")?;
        let changed = conn
            .execute(
                "UPDATE automation_definitions
                 SET definition_json = ?2,
                     definition_digest = ?3,
                     revision = ?4,
                     updated_at = ?5
                 WHERE id = ?1
                   AND revision = ?6",
                params![
                    id,
                    definition_json,
                    definition_digest,
                    i64::try_from(to_revision)
                        .context("automation definition revision exceeds SQLite range")?,
                    migrated_at,
                    i64::try_from(from_revision)
                        .context("automation definition revision exceeds SQLite range")?,
                ],
            )
            .with_context(|| format!("failed to normalize durable local timezone for `{id}`"))?;
        anyhow::ensure!(
            changed == 1,
            "automation definition `{id}` changed during timezone migration"
        );
        conn.execute(
            "INSERT INTO automation_timezone_migrations (
                automation_id, from_revision, to_revision, from_timezone, to_timezone,
                previous_definition_json, previous_definition_digest, definition_digest,
                migrated_at
             ) VALUES (?1, ?2, ?3, 'local', ?4, ?5, ?6, ?7, ?8)",
            params![
                id,
                i64::try_from(from_revision)
                    .context("automation definition revision exceeds SQLite range")?,
                i64::try_from(to_revision)
                    .context("automation definition revision exceeds SQLite range")?,
                resolved_name,
                previous_definition_json,
                previous_digest,
                definition_digest,
                migrated_at,
            ],
        )
        .with_context(|| format!("failed to record timezone migration for `{id}`"))?;
        super::contract::events::append_migrated_definition_event(
            conn,
            super::contract::events::MigratedDefinitionEventInput {
                automation_id: &id,
                revision: to_revision,
                definition_digest: Some(&definition_digest),
                lifecycle_state: &lifecycle_state,
                migration: "local-timezone-normalization-v1",
                recorded_at: &migrated_at,
                observed_at: &migrated_at,
            },
        )
        .with_context(|| format!("failed to append timezone migration event for `{id}`"))?;
    }
    Ok(())
}

fn later_timestamp(left: &str, right: &str) -> Result<String> {
    let left_instant =
        chrono::DateTime::parse_from_rfc3339(left).context("migration timestamp is invalid")?;
    let right_instant = chrono::DateTime::parse_from_rfc3339(right)
        .context("stored automation definition timestamp is invalid")?;
    Ok(if right_instant > left_instant {
        right.to_owned()
    } else {
        left.to_owned()
    })
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
        assert_eq!(migrated.revision, 2);
        let definition: Value = serde_json::from_str(&migrated.definition_json).unwrap();
        assert_ne!(definition["timezone"], "local");
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
    fn initialization_normalizes_the_current_definition_without_rewriting_history() {
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
        let migrated_definition: Value = serde_json::from_str(&definition.0).unwrap();
        assert_ne!(migrated_definition["timezone"], "local");
        assert_eq!(definition.1, 2);
        assert_ne!(definition.2, expected_digest);
        assert_eq!(definition.3, "active");
        let lifecycle_column: (i64, Option<String>) = conn
            .prepare("PRAGMA table_info(automation_definitions)")
            .unwrap()
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, Option<String>>(4)?,
                ))
            })
            .unwrap()
            .find_map(|row| {
                let (name, not_null, default_value) = row.unwrap();
                (name == "lifecycle_state").then_some((not_null, default_value))
            })
            .unwrap();
        assert_eq!(lifecycle_column, (1, Some("'draft'".to_string())));
        assert!(conn
            .execute(
                "UPDATE automation_definitions
                 SET lifecycle_state = 'unknown'
                 WHERE id = 'legacy'",
                [],
            )
            .is_err());

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
        assert_eq!(run, (1, expected_digest.clone(), None));
        let timezone_migration: (i64, i64, String, String, String, Option<String>) = conn
            .query_row(
                "SELECT from_revision, to_revision, from_timezone, to_timezone,
                        previous_definition_json, previous_definition_digest
                 FROM automation_timezone_migrations
                 WHERE automation_id = 'legacy'",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(timezone_migration.0, 1);
        assert_eq!(timezone_migration.1, 2);
        assert_eq!(timezone_migration.2, "local");
        assert_ne!(timezone_migration.3, "local");
        assert_eq!(timezone_migration.4, legacy_definition);
        assert_eq!(
            timezone_migration.5.as_deref(),
            Some(expected_digest.as_str())
        );
        let lifecycle_events: Vec<(i64, Value)> = conn
            .prepare(
                "SELECT sequence, event_json
                 FROM automation_events
                 WHERE stream_kind = 'automation' AND stream_id = 'legacy'
                 ORDER BY sequence",
            )
            .unwrap()
            .query_map([], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })
            .unwrap()
            .map(|row| {
                let (sequence, event_json) = row.unwrap();
                (sequence, serde_json::from_str(&event_json).unwrap())
            })
            .collect();
        assert_eq!(lifecycle_events.len(), 2);
        assert_eq!(lifecycle_events[0].0, 0);
        assert_eq!(lifecycle_events[0].1["kind"], "definition.imported");
        assert_eq!(lifecycle_events[0].1["payload"]["revision"], 1);
        assert_eq!(lifecycle_events[1].0, 1);
        assert_eq!(lifecycle_events[1].1["kind"], "definition.revised");
        assert_eq!(lifecycle_events[1].1["payload"]["revision"], 2);
        assert_eq!(
            lifecycle_events[1].1["payload"]["definitionDigest"]["value"],
            definition.2
        );

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
    fn initialization_normalizes_durable_local_as_a_new_definition_revision() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("legacy-local-timezone.sqlite");
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE automation_definitions (
                id TEXT PRIMARY KEY NOT NULL,
                name TEXT NOT NULL,
                status TEXT NOT NULL,
                definition_json TEXT NOT NULL,
                revision INTEGER NOT NULL DEFAULT 1 CHECK (revision >= 1),
                definition_digest TEXT,
                lifecycle_state TEXT NOT NULL DEFAULT 'draft',
                tombstoned_at TEXT,
                authority_version INTEGER NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
             );
             CREATE TABLE automation_occurrences (
                id TEXT PRIMARY KEY NOT NULL,
                automation_id TEXT NOT NULL,
                automation_revision INTEGER NOT NULL DEFAULT 1,
                definition_digest TEXT,
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
                automation_revision INTEGER NOT NULL DEFAULT 1,
                definition_digest TEXT,
                receipt_id TEXT,
                timeout_at TEXT,
                started_at TEXT NOT NULL,
                finished_at TEXT
             );",
        )
        .unwrap();
        let local_definition = r#"{"schemaVersion":1,"id":"legacy-local","name":"Legacy local","status":"ACTIVE","rrule":"FREQ=DAILY;BYHOUR=9","timezone":"local","misfire":"latest","overlap":"forbid","timeoutMinutes":30,"runtime":"coven-code","prompt":"Preserve history."}"#;
        let old_digest =
            crate::automations::contract::migration::definition_digest(local_definition).unwrap();
        conn.execute(
            "INSERT INTO automation_definitions (
                id, name, status, definition_json, revision, definition_digest, lifecycle_state,
                tombstoned_at, authority_version, created_at, updated_at
             ) VALUES (
                'legacy-local', 'Legacy local', 'ACTIVE', ?1, 1, ?2, 'active',
                NULL, 0, '2026-09-01T00:00:00.000Z', '2026-09-01T00:00:00.000Z'
             )",
            params![local_definition, old_digest],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO automation_occurrences (
                id, automation_id, automation_revision, definition_digest, scheduled_for, kind,
                state, attempt, created_at, updated_at
             ) VALUES (
                'legacy-occurrence', 'legacy-local', 1, ?1,
                '2026-09-01T09:00:00.000Z', 'scheduled', 'succeeded', 1,
                '2026-09-01T09:00:00.000Z', '2026-09-01T09:05:00.000Z'
             )",
            params![old_digest],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO automation_runs (
                id, automation_id, occurrence_id, runtime, status, automation_revision,
                definition_digest, started_at, finished_at
             ) VALUES (
                'legacy-run', 'legacy-local', 'legacy-occurrence', 'coven-code', 'succeeded',
                1, ?1, '2026-09-01T09:00:01.000Z', '2026-09-01T09:05:00.000Z'
             )",
            params![old_digest],
        )
        .unwrap();
        drop(conn);

        initialize_store(&path).unwrap();

        let conn = crate::store::open_initialized_store(&path).unwrap();
        let record = get_definition(&conn, "legacy-local").unwrap().unwrap();
        let migrated: serde_json::Value = serde_json::from_str(&record.definition_json).unwrap();
        assert_ne!(migrated["timezone"], "local");
        assert_eq!(record.revision, 2);
        assert_ne!(
            record.definition_digest.as_deref(),
            Some(old_digest.as_str())
        );
        let historical_pins: (i64, String, i64, String) = conn
            .query_row(
                "SELECT occurrence.automation_revision, occurrence.definition_digest,
                        run.automation_revision, run.definition_digest
                 FROM automation_occurrences AS occurrence
                 JOIN automation_runs AS run ON run.occurrence_id = occurrence.id
                 WHERE occurrence.id = 'legacy-occurrence'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(historical_pins, (1, old_digest.clone(), 1, old_digest));
        let migration_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM automation_timezone_migrations
                 WHERE automation_id = 'legacy-local'
                   AND from_timezone = 'local'
                   AND to_timezone <> 'local'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(migration_count, 1);
    }

    #[test]
    fn timezone_migration_normalizes_tombstones_and_retains_the_original_body() {
        let (_temp, conn) = temp_store();
        let definition_json = r#"{"schemaVersion":1,"id":"legacy-tombstone","name":"Legacy tombstone","status":"PAUSED","rrule":"FREQ=DAILY;BYHOUR=9","timezone":"local","misfire":"latest","overlap":"forbid","timeoutMinutes":30,"runtime":"coven-code","prompt":"Retain exact history."}"#;
        let digest =
            crate::automations::contract::migration::definition_digest(definition_json).unwrap();
        conn.execute(
            "INSERT INTO automation_definitions (
                id, name, status, definition_json, revision, definition_digest, lifecycle_state,
                tombstoned_at, authority_version, created_at, updated_at
             ) VALUES (
                'legacy-tombstone', 'Legacy tombstone', 'PAUSED', ?1, 3, ?2, 'paused',
                '2026-09-02T00:00:00.000Z', 0,
                '2026-09-01T00:00:00.000Z', '2026-09-02T00:00:00.000Z'
             )",
            params![definition_json, digest],
        )
        .unwrap();

        migrate_durable_local_timezones(&conn).unwrap();

        let record = get_definition_with_tombstone(&conn, "legacy-tombstone", true)
            .unwrap()
            .unwrap();
        let migrated: Value = serde_json::from_str(&record.definition_json).unwrap();
        assert_ne!(migrated["timezone"], "local");
        assert_eq!(record.revision, 4);
        assert_eq!(
            record.tombstoned_at.as_deref(),
            Some("2026-09-02T00:00:00.000Z")
        );
        let retained: String = conn
            .query_row(
                "SELECT previous_definition_json
                 FROM automation_timezone_migrations
                 WHERE automation_id = 'legacy-tombstone'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(retained, definition_json);
    }

    #[test]
    fn timezone_migration_rolls_back_the_full_batch_when_event_append_fails() {
        let (_temp, conn) = temp_store();
        let definition_json = r#"{"schemaVersion":1,"id":"atomic-local","name":"Atomic local","status":"ACTIVE","rrule":"FREQ=DAILY;BYHOUR=9","timezone":"local","misfire":"latest","overlap":"forbid","timeoutMinutes":30,"runtime":"coven-code","prompt":"Migrate atomically."}"#;
        let digest =
            crate::automations::contract::migration::definition_digest(definition_json).unwrap();
        conn.execute(
            "INSERT INTO automation_definitions (
                id, name, status, definition_json, revision, definition_digest, lifecycle_state,
                tombstoned_at, authority_version, created_at, updated_at
             ) VALUES (
                'atomic-local', 'Atomic local', 'ACTIVE', ?1, 1, ?2, 'active',
                NULL, 0, '2026-09-01T00:00:00.000Z', '2026-09-01T00:00:00.000Z'
             )",
            params![definition_json, digest],
        )
        .unwrap();
        super::super::contract::events::append_imported_definition_event(
            &conn,
            super::super::contract::events::ImportedDefinitionEventInput {
                automation_id: "atomic-local",
                revision: 1,
                definition_digest: Some(&digest),
                lifecycle_state: "active",
                imported_from: "legacy-coven-store",
                recorded_at: "2026-09-01T00:00:00.000Z",
                observed_at: "2026-09-01T00:00:00.000Z",
            },
        )
        .unwrap();

        let definition_before: (String, i64, String, String) = conn
            .query_row(
                "SELECT definition_json, revision, definition_digest, updated_at
                 FROM automation_definitions
                 WHERE id = 'atomic-local'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        let events_before: Vec<String> = conn
            .prepare(
                "SELECT event_json
                 FROM automation_events
                 WHERE stream_kind = 'automation' AND stream_id = 'atomic-local'
                 ORDER BY sequence",
            )
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(events_before.len(), 1);

        conn.execute_batch(
            "CREATE TRIGGER reject_atomic_timezone_revision_event
             BEFORE INSERT ON automation_events
             WHEN NEW.stream_kind = 'automation'
              AND NEW.stream_id = 'atomic-local'
              AND json_extract(NEW.event_json, '$.kind') = 'definition.revised'
             BEGIN
                 SELECT RAISE(ABORT, 'synthetic timezone migration event failure');
             END;",
        )
        .unwrap();

        let error = migrate_durable_local_timezones(&conn).unwrap_err();
        assert!(
            format!("{error:#}").contains("synthetic timezone migration event failure"),
            "{error:#}"
        );

        let definition_after_failure: (String, i64, String, String) = conn
            .query_row(
                "SELECT definition_json, revision, definition_digest, updated_at
                 FROM automation_definitions
                 WHERE id = 'atomic-local'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        let migration_count_after_failure: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM automation_timezone_migrations
                 WHERE automation_id = 'atomic-local'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let events_after_failure: Vec<String> = conn
            .prepare(
                "SELECT event_json
                 FROM automation_events
                 WHERE stream_kind = 'automation' AND stream_id = 'atomic-local'
                 ORDER BY sequence",
            )
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();

        assert_eq!(definition_after_failure, definition_before);
        assert_eq!(migration_count_after_failure, 0);
        assert_eq!(events_after_failure, events_before);

        conn.execute_batch("DROP TRIGGER reject_atomic_timezone_revision_event")
            .unwrap();
        migrate_durable_local_timezones(&conn).unwrap();
        let state_after_success: (String, i64, String, i64, i64) = conn
            .query_row(
                "SELECT definition.definition_json,
                        definition.revision,
                        definition.definition_digest,
                        (SELECT COUNT(*) FROM automation_timezone_migrations
                         WHERE automation_id = definition.id),
                        (SELECT COUNT(*) FROM automation_events
                         WHERE stream_kind = 'automation' AND stream_id = definition.id)
                 FROM automation_definitions AS definition
                 WHERE definition.id = 'atomic-local'",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .unwrap();
        assert_ne!(
            serde_json::from_str::<Value>(&state_after_success.0).unwrap()["timezone"],
            "local"
        );
        assert_eq!(state_after_success.1, 2);
        assert_ne!(state_after_success.2, digest);
        assert_eq!(state_after_success.3, 1);
        assert_eq!(state_after_success.4, 2);

        migrate_durable_local_timezones(&conn).unwrap();
        let state_after_rerun: (String, i64, String, i64, i64) = conn
            .query_row(
                "SELECT definition.definition_json,
                        definition.revision,
                        definition.definition_digest,
                        (SELECT COUNT(*) FROM automation_timezone_migrations
                         WHERE automation_id = definition.id),
                        (SELECT COUNT(*) FROM automation_events
                         WHERE stream_kind = 'automation' AND stream_id = definition.id)
                 FROM automation_definitions AS definition
                 WHERE definition.id = 'atomic-local'",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(state_after_rerun, state_after_success);
    }

    #[test]
    fn timezone_migration_uses_a_savepoint_inside_store_initialization_transaction() {
        let (_temp, conn) = temp_store();
        let definition_json = r#"{"schemaVersion":1,"id":"nested-local","name":"Nested local","status":"ACTIVE","rrule":"FREQ=DAILY;BYHOUR=9","timezone":"local","misfire":"latest","overlap":"forbid","timeoutMinutes":30,"runtime":"coven-code","prompt":"Use a savepoint."}"#;
        let digest =
            crate::automations::contract::migration::definition_digest(definition_json).unwrap();
        conn.execute(
            "INSERT INTO automation_definitions (
                id, name, status, definition_json, revision, definition_digest, lifecycle_state,
                tombstoned_at, authority_version, created_at, updated_at
             ) VALUES (
                'nested-local', 'Nested local', 'ACTIVE', ?1, 1, ?2, 'active',
                NULL, 0, '2026-09-01T00:00:00.000Z', '2026-09-01T00:00:00.000Z'
             )",
            params![definition_json, digest],
        )
        .unwrap();
        super::super::contract::events::append_imported_definition_event(
            &conn,
            super::super::contract::events::ImportedDefinitionEventInput {
                automation_id: "nested-local",
                revision: 1,
                definition_digest: Some(&digest),
                lifecycle_state: "active",
                imported_from: "legacy-coven-store",
                recorded_at: "2026-09-01T00:00:00.000Z",
                observed_at: "2026-09-01T00:00:00.000Z",
            },
        )
        .unwrap();
        conn.execute_batch(
            "CREATE TRIGGER reject_nested_timezone_revision_event
             BEFORE INSERT ON automation_events
             WHEN NEW.stream_kind = 'automation'
              AND NEW.stream_id = 'nested-local'
              AND json_extract(NEW.event_json, '$.kind') = 'definition.revised'
             BEGIN
                 SELECT RAISE(ABORT, 'synthetic nested timezone migration failure');
             END;
             BEGIN IMMEDIATE;",
        )
        .unwrap();

        let error = migrate_durable_local_timezones(&conn).unwrap_err();
        assert!(
            format!("{error:#}").contains("synthetic nested timezone migration failure"),
            "{error:#}"
        );
        assert!(!conn.is_autocommit());
        let unchanged: (String, i64, String, i64, i64) = conn
            .query_row(
                "SELECT definition.definition_json,
                        definition.revision,
                        definition.definition_digest,
                        (SELECT COUNT(*) FROM automation_timezone_migrations
                         WHERE automation_id = definition.id),
                        (SELECT COUNT(*) FROM automation_events
                         WHERE stream_kind = 'automation' AND stream_id = definition.id)
                 FROM automation_definitions AS definition
                 WHERE definition.id = 'nested-local'",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(unchanged, (definition_json.to_string(), 1, digest, 0, 1));

        conn.execute_batch("COMMIT").unwrap();
    }

    #[test]
    fn timezone_migration_rejects_a_mismatched_historical_digest_without_mutation() {
        let (_temp, conn) = temp_store();
        let valid_json = r#"{"schemaVersion":1,"id":"a-valid-local","name":"Valid local","status":"ACTIVE","rrule":"FREQ=DAILY;BYHOUR=9","timezone":"local","misfire":"latest","overlap":"forbid","timeoutMinutes":30,"runtime":"coven-code","prompt":"Must roll back too."}"#;
        let valid_digest =
            crate::automations::contract::migration::definition_digest(valid_json).unwrap();
        let mismatched_json = r#"{"schemaVersion":1,"id":"z-mismatched-local","name":"Mismatched local","status":"ACTIVE","rrule":"FREQ=DAILY;BYHOUR=10","timezone":"utc","misfire":"latest","overlap":"forbid","timeoutMinutes":30,"runtime":"coven-code","prompt":"Do not re-bless this body."}"#;
        let mismatched_digest = "0".repeat(64);
        for (id, name, definition_json, digest) in [
            (
                "a-valid-local",
                "Valid local",
                valid_json,
                valid_digest.as_str(),
            ),
            (
                "z-mismatched-local",
                "Mismatched local",
                mismatched_json,
                mismatched_digest.as_str(),
            ),
        ] {
            conn.execute(
                "INSERT INTO automation_definitions (
                    id, name, status, definition_json, revision, definition_digest, lifecycle_state,
                    tombstoned_at, authority_version, created_at, updated_at
                 ) VALUES (
                    ?1, ?2, 'ACTIVE', ?3, 1, ?4, 'active',
                    NULL, 0, '2026-09-01T00:00:00.000Z', '2026-09-01T00:00:00.000Z'
                 )",
                params![id, name, definition_json, digest],
            )
            .unwrap();
            super::super::contract::events::append_imported_definition_event(
                &conn,
                super::super::contract::events::ImportedDefinitionEventInput {
                    automation_id: id,
                    revision: 1,
                    definition_digest: Some(digest),
                    lifecycle_state: "active",
                    imported_from: "legacy-coven-store",
                    recorded_at: "2026-09-01T00:00:00.000Z",
                    observed_at: "2026-09-01T00:00:00.000Z",
                },
            )
            .unwrap();
        }

        let definitions_before: Vec<(String, String, i64, String, String)> = conn
            .prepare(
                "SELECT id, definition_json, revision, definition_digest, updated_at
                 FROM automation_definitions
                 WHERE id IN ('a-valid-local', 'z-mismatched-local')
                 ORDER BY id",
            )
            .unwrap()
            .query_map([], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            })
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        let events_before: Vec<(String, i64, String)> = conn
            .prepare(
                "SELECT stream_id, sequence, event_json
                 FROM automation_events
                 WHERE stream_kind = 'automation'
                   AND stream_id IN ('a-valid-local', 'z-mismatched-local')
                 ORDER BY stream_id, sequence",
            )
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        let heads_before: Vec<(String, i64, i64, String)> = conn
            .prepare(
                "SELECT stream_id, next_sequence, earliest_sequence, updated_at
                 FROM automation_event_stream_heads
                 WHERE stream_kind = 'automation'
                   AND stream_id IN ('a-valid-local', 'z-mismatched-local')
                 ORDER BY stream_id",
            )
            .unwrap()
            .query_map([], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();

        let error = migrate_durable_local_timezones(&conn).unwrap_err();
        assert!(format!("{error:#}").contains("definition digest mismatch"));

        let definitions_after: Vec<(String, String, i64, String, String)> = conn
            .prepare(
                "SELECT id, definition_json, revision, definition_digest, updated_at
                 FROM automation_definitions
                 WHERE id IN ('a-valid-local', 'z-mismatched-local')
                 ORDER BY id",
            )
            .unwrap()
            .query_map([], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            })
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        let migration_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM automation_timezone_migrations
                 WHERE automation_id IN ('a-valid-local', 'z-mismatched-local')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let events_after: Vec<(String, i64, String)> = conn
            .prepare(
                "SELECT stream_id, sequence, event_json
                 FROM automation_events
                 WHERE stream_kind = 'automation'
                   AND stream_id IN ('a-valid-local', 'z-mismatched-local')
                 ORDER BY stream_id, sequence",
            )
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        let heads_after: Vec<(String, i64, i64, String)> = conn
            .prepare(
                "SELECT stream_id, next_sequence, earliest_sequence, updated_at
                 FROM automation_event_stream_heads
                 WHERE stream_kind = 'automation'
                   AND stream_id IN ('a-valid-local', 'z-mismatched-local')
                 ORDER BY stream_id",
            )
            .unwrap()
            .query_map([], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();

        assert_eq!(definitions_after, definitions_before);
        assert_eq!(migration_count, 0);
        assert_eq!(events_after, events_before);
        assert_eq!(heads_after, heads_before);
    }

    #[test]
    fn timezone_migration_does_not_regress_the_revision_effective_timestamp() {
        let (_temp, conn) = temp_store();
        let definition_json = r#"{"schemaVersion":1,"id":"future-updated-local","name":"Future updated local","status":"ACTIVE","rrule":"FREQ=DAILY;BYHOUR=9","timezone":"local","misfire":"latest","overlap":"forbid","timeoutMinutes":30,"runtime":"coven-code","prompt":"Keep monotonic time."}"#;
        let digest =
            crate::automations::contract::migration::definition_digest(definition_json).unwrap();
        let previous_updated_at = "2099-01-01T12:00:00.000Z";
        conn.execute(
            "INSERT INTO automation_definitions (
                id, name, status, definition_json, revision, definition_digest, lifecycle_state,
                tombstoned_at, authority_version, created_at, updated_at
             ) VALUES (
                'future-updated-local', 'Future updated local', 'ACTIVE', ?1, 1, ?2, 'active',
                NULL, 0, '2026-09-01T00:00:00.000Z', ?3
             )",
            params![definition_json, digest, previous_updated_at],
        )
        .unwrap();
        super::super::contract::events::append_imported_definition_event(
            &conn,
            super::super::contract::events::ImportedDefinitionEventInput {
                automation_id: "future-updated-local",
                revision: 1,
                definition_digest: Some(&digest),
                lifecycle_state: "active",
                imported_from: "legacy-coven-store",
                recorded_at: previous_updated_at,
                observed_at: previous_updated_at,
            },
        )
        .unwrap();

        migrate_durable_local_timezones(&conn).unwrap();

        let (updated_at, migrated_at): (String, String) = conn
            .query_row(
                "SELECT definition.updated_at, migration.migrated_at
                 FROM automation_definitions AS definition
                 JOIN automation_timezone_migrations AS migration
                   ON migration.automation_id = definition.id
                 WHERE definition.id = 'future-updated-local'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(updated_at, previous_updated_at);
        assert_eq!(migrated_at, updated_at);
        let revised_event: Value = conn
            .query_row(
                "SELECT event_json
                 FROM automation_events
                 WHERE stream_kind = 'automation'
                   AND stream_id = 'future-updated-local'
                   AND sequence = 1",
                [],
                |row| row.get::<_, String>(0),
            )
            .map(|event| serde_json::from_str(&event).unwrap())
            .unwrap();
        assert_eq!(revised_event["recordedAt"], updated_at);
        assert_eq!(revised_event["observedAt"], updated_at);
    }

    #[test]
    fn timezone_migration_clamps_each_definition_timestamp_independently() {
        let (_temp, conn) = temp_store();
        for (id, updated_at) in [
            ("a-normal-local", "2026-09-01T12:00:00.000Z"),
            ("z-future-local", "2099-01-01T12:00:00.000Z"),
        ] {
            let definition_json = format!(
                r#"{{"schemaVersion":1,"id":"{id}","name":"{id}","status":"ACTIVE","rrule":"FREQ=DAILY;BYHOUR=9","timezone":"local","misfire":"latest","overlap":"forbid","timeoutMinutes":30,"runtime":"coven-code","prompt":"Keep per-row time."}}"#
            );
            let digest =
                crate::automations::contract::migration::definition_digest(&definition_json)
                    .unwrap();
            conn.execute(
                "INSERT INTO automation_definitions (
                    id, name, status, definition_json, revision, definition_digest,
                    lifecycle_state, tombstoned_at, authority_version, created_at, updated_at
                 ) VALUES (?1, ?1, 'ACTIVE', ?2, 1, ?3, 'active', NULL, 0, ?4, ?4)",
                params![id, definition_json, digest, updated_at],
            )
            .unwrap();
        }

        migrate_durable_local_timezones(&conn).unwrap();

        let normal_updated_at: String = conn
            .query_row(
                "SELECT updated_at FROM automation_definitions
                 WHERE id = 'a-normal-local'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let future_updated_at: String = conn
            .query_row(
                "SELECT updated_at FROM automation_definitions
                 WHERE id = 'z-future-local'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(normal_updated_at < future_updated_at);
        assert_eq!(future_updated_at, "2099-01-01T12:00:00.000Z");
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
