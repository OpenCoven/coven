//! Additive adoption of the Automations v1 history sidecars.

use anyhow::{Context, Result};
use chrono::{SecondsFormat, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value;

use super::canonical_json::sha256_digest;

const PROFILE: &str = "coven.automations.v1";

pub const AUTOMATION_CONTRACT_MIGRATIONS_SCHEMA_SQL: &str = "
    CREATE TABLE IF NOT EXISTS automation_contract_migrations (
        profile TEXT PRIMARY KEY NOT NULL,
        definitions_migrated INTEGER NOT NULL,
        occurrences_migrated INTEGER NOT NULL,
        runs_migrated INTEGER NOT NULL,
        unverifiable_definitions INTEGER NOT NULL,
        unverifiable_occurrences INTEGER NOT NULL,
        unverifiable_runs INTEGER NOT NULL,
        migrated_at TEXT NOT NULL
    );
";

pub fn definition_digest(definition_json: &str) -> Result<String> {
    let definition: Value =
        serde_json::from_str(definition_json).context("stored definition is not valid JSON")?;
    sha256_digest(&definition).context("failed to digest stored definition with JCS")
}

pub fn lifecycle_state(status: &str) -> &'static str {
    match status {
        "ACTIVE" => "active",
        "PAUSED" => "paused",
        _ => "draft",
    }
}

pub fn migrate_legacy_contract_metadata(conn: &Connection) -> Result<()> {
    ensure_column(
        conn,
        "automation_definitions",
        "definition_digest",
        "ALTER TABLE automation_definitions ADD COLUMN definition_digest TEXT",
    )?;
    ensure_column(
        conn,
        "automation_definitions",
        "lifecycle_state",
        "ALTER TABLE automation_definitions ADD COLUMN lifecycle_state TEXT",
    )?;
    ensure_column(
        conn,
        "automation_occurrences",
        "automation_revision",
        "ALTER TABLE automation_occurrences
         ADD COLUMN automation_revision INTEGER NOT NULL DEFAULT 1
         CHECK (automation_revision >= 1)",
    )?;
    ensure_column(
        conn,
        "automation_occurrences",
        "definition_digest",
        "ALTER TABLE automation_occurrences ADD COLUMN definition_digest TEXT",
    )?;
    ensure_column(
        conn,
        "automation_runs",
        "automation_revision",
        "ALTER TABLE automation_runs
         ADD COLUMN automation_revision INTEGER NOT NULL DEFAULT 1
         CHECK (automation_revision >= 1)",
    )?;
    ensure_column(
        conn,
        "automation_runs",
        "definition_digest",
        "ALTER TABLE automation_runs ADD COLUMN definition_digest TEXT",
    )?;
    ensure_column(
        conn,
        "automation_runs",
        "receipt_id",
        "ALTER TABLE automation_runs ADD COLUMN receipt_id TEXT",
    )?;
    conn.execute_batch(AUTOMATION_CONTRACT_MIGRATIONS_SCHEMA_SQL)
        .context("failed to initialize automation contract migration ledger")?;

    let already_migrated = conn
        .query_row(
            "SELECT 1 FROM automation_contract_migrations WHERE profile = ?1",
            [PROFILE],
            |_| Ok(()),
        )
        .optional()
        .context("failed to inspect automation contract migration ledger")?
        .is_some();
    if already_migrated {
        return Ok(());
    }

    let definitions = {
        let mut statement = conn
            .prepare(
                "SELECT id, status, definition_json
                 FROM automation_definitions
                 WHERE definition_digest IS NULL OR lifecycle_state IS NULL",
            )
            .context("failed to prepare legacy automation definition migration")?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .context("failed to query legacy automation definitions")?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("failed to read legacy automation definitions")?;
        rows
    };
    for (id, status, definition_json) in &definitions {
        match definition_digest(definition_json) {
            Ok(digest) => conn.execute(
                "UPDATE automation_definitions
                 SET definition_digest = ?2, lifecycle_state = ?3
                 WHERE id = ?1",
                params![id, digest, lifecycle_state(status)],
            ),
            Err(_) => conn.execute(
                "UPDATE automation_definitions
                 SET lifecycle_state = 'invalid'
                 WHERE id = ?1",
                [id],
            ),
        }
        .with_context(|| format!("failed to migrate automation definition `{id}`"))?;
    }

    let occurrence_pins_migrated = conn
        .execute(
            "UPDATE automation_occurrences
             SET automation_revision = (
                     SELECT revision
                     FROM automation_definitions
                     WHERE automation_definitions.id = automation_occurrences.automation_id
                 ),
                 definition_digest = (
                     SELECT definition_digest
                     FROM automation_definitions
                     WHERE automation_definitions.id = automation_occurrences.automation_id
                 )
             WHERE definition_digest IS NULL
               AND EXISTS (
                   SELECT 1
                   FROM automation_definitions
                   WHERE automation_definitions.id = automation_occurrences.automation_id
                     AND automation_definitions.definition_digest IS NOT NULL
                     AND julianday(automation_definitions.created_at)
                         <= julianday(automation_occurrences.created_at)
                     AND julianday(automation_definitions.updated_at)
                         <= julianday(automation_occurrences.created_at)
               )",
            [],
        )
        .context("failed to pin legacy automation occurrences")?;
    let occurrence_states_migrated = conn
        .execute(
            "UPDATE automation_occurrences
         SET state = 'superseded'
         WHERE state = 'skipped'",
            [],
        )
        .context("failed to map legacy skipped occurrences")?;
    let occurrences_migrated = occurrence_pins_migrated
        .checked_add(occurrence_states_migrated)
        .context("occurrence migration count overflow")?;
    let runs_migrated = conn
        .execute(
            "UPDATE automation_runs
             SET automation_revision = CASE
                     WHEN occurrence_id IS NOT NULL THEN (
                         SELECT automation_revision
                         FROM automation_occurrences
                         WHERE automation_occurrences.id = automation_runs.occurrence_id
                     )
                     ELSE automation_revision
                 END,
                 definition_digest = CASE
                     WHEN occurrence_id IS NOT NULL THEN (
                         SELECT definition_digest
                         FROM automation_occurrences
                         WHERE automation_occurrences.id = automation_runs.occurrence_id
                     )
                     ELSE NULL
                 END
             WHERE definition_digest IS NULL
               AND occurrence_id IS NOT NULL
               AND EXISTS (
                   SELECT 1
                   FROM automation_occurrences
                   WHERE automation_occurrences.id = automation_runs.occurrence_id
                     AND automation_occurrences.automation_id = automation_runs.automation_id
                     AND automation_occurrences.definition_digest IS NOT NULL
               )",
            [],
        )
        .context("failed to pin legacy automation runs")?;
    let unverifiable_definitions: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM automation_definitions WHERE definition_digest IS NULL",
            [],
            |row| row.get(0),
        )
        .context("failed to count unverifiable automation definitions")?;
    let unverifiable_occurrences: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM automation_occurrences WHERE definition_digest IS NULL",
            [],
            |row| row.get(0),
        )
        .context("failed to count unverifiable automation occurrences")?;
    let unverifiable_runs: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM automation_runs WHERE definition_digest IS NULL",
            [],
            |row| row.get(0),
        )
        .context("failed to count unverifiable automation runs")?;

    conn.execute(
        "INSERT INTO automation_contract_migrations (
            profile, definitions_migrated, occurrences_migrated, runs_migrated,
            unverifiable_definitions, unverifiable_occurrences, unverifiable_runs, migrated_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            PROFILE,
            i64::try_from(definitions.len()).context("definition migration count overflow")?,
            i64::try_from(occurrences_migrated).context("occurrence migration count overflow")?,
            i64::try_from(runs_migrated).context("run migration count overflow")?,
            unverifiable_definitions,
            unverifiable_occurrences,
            unverifiable_runs,
            Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
        ],
    )
    .context("failed to record automation contract migration")?;
    Ok(())
}

fn ensure_column(conn: &Connection, table: &str, column: &str, sql: &str) -> Result<()> {
    let present = conn
        .prepare(&format!("PRAGMA table_info({table})"))
        .with_context(|| format!("failed to inspect {table} schema"))?
        .query_map([], |row| row.get::<_, String>(1))
        .with_context(|| format!("failed to enumerate {table} columns"))?
        .collect::<rusqlite::Result<Vec<_>>>()
        .with_context(|| format!("failed to read {table} columns"))?
        .iter()
        .any(|candidate| candidate == column);
    if !present {
        conn.execute_batch(sql)
            .with_context(|| format!("failed to add {table}.{column}"))?;
    }
    Ok(())
}
