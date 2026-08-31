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
    pub timeout_minutes: i64,
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
    let actual_digest = definition_digest(&record.definition_json);
    anyhow::ensure!(
        record.definition_digest == actual_digest,
        "stored routine `{}` definition digest does not match its JSON",
        record.id
    );
    let definition: RoutineDefinition = serde_json::from_str(&record.definition_json)
        .with_context(|| format!("stored routine `{}` is unreadable", record.id))?;
    Ok(DefinitionSnapshot {
        revision: record.revision,
        digest: record.definition_digest.clone(),
        definition_json: record.definition_json.clone(),
        output_target: definition.output_target,
        timeout_minutes: i64::from(definition.timeout_minutes),
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
            "automation_occurrences",
            "delivery_state",
            "ALTER TABLE automation_occurrences
             ADD COLUMN delivery_state TEXT NOT NULL DEFAULT 'none'",
        ),
        (
            "automation_occurrences",
            "delivery_token",
            "ALTER TABLE automation_occurrences ADD COLUMN delivery_token TEXT",
        ),
        (
            "automation_occurrences",
            "delivery_digest",
            "ALTER TABLE automation_occurrences ADD COLUMN delivery_digest TEXT",
        ),
        (
            "automation_occurrences",
            "delivery_error",
            "ALTER TABLE automation_occurrences ADD COLUMN delivery_error TEXT",
        ),
        (
            "automation_occurrences",
            "legacy_reconciled_at",
            "ALTER TABLE automation_occurrences ADD COLUMN legacy_reconciled_at TEXT",
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
        (
            "automation_runs",
            "delivery_state",
            "ALTER TABLE automation_runs
             ADD COLUMN delivery_state TEXT NOT NULL DEFAULT 'none'",
        ),
        (
            "automation_runs",
            "delivery_token",
            "ALTER TABLE automation_runs ADD COLUMN delivery_token TEXT",
        ),
        (
            "automation_runs",
            "delivery_digest",
            "ALTER TABLE automation_runs ADD COLUMN delivery_digest TEXT",
        ),
        (
            "automation_runs",
            "delivery_error",
            "ALTER TABLE automation_runs ADD COLUMN delivery_error TEXT",
        ),
        (
            "automation_runs",
            "legacy_reconciled_at",
            "ALTER TABLE automation_runs ADD COLUMN legacy_reconciled_at TEXT",
        ),
    ] {
        ensure_column(conn, table, column, sql)?;
    }
    backfill_definition_digests(conn)?;
    fail_unproven_legacy_executions(conn)?;
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

struct LegacyLinkedPair {
    occurrence_id: String,
    run_id: String,
    occurrence_state: String,
    run_status: String,
    occurrence_delivery_state: String,
    run_delivery_state: String,
    output_commit: Option<String>,
}

fn fail_unproven_legacy_executions(conn: &Connection) -> Result<()> {
    const BASE_REASON: &str = "legacy automation immutable snapshot unavailable";
    let now = now_iso();
    let pairs = {
        let mut statement = conn
            .prepare(
                "SELECT o.id, r.id, o.state, r.status,
                        o.delivery_state, r.delivery_state, r.output_commit
                 FROM automation_occurrences AS o
                 JOIN automation_runs AS r ON r.occurrence_id = o.id
                 WHERE (
                       o.definition_json IS NULL
                       OR r.definition_json IS NULL
                   )
                   AND (
                       o.legacy_reconciled_at IS NULL
                       OR r.legacy_reconciled_at IS NULL
                   )
                 ORDER BY o.id, r.id",
            )
            .context("failed to prepare legacy linked automation reconciliation")?;
        let rows = statement
            .query_map([], |row| {
                Ok(LegacyLinkedPair {
                    occurrence_id: row.get(0)?,
                    run_id: row.get(1)?,
                    occurrence_state: row.get(2)?,
                    run_status: row.get(3)?,
                    occurrence_delivery_state: row.get(4)?,
                    run_delivery_state: row.get(5)?,
                    output_commit: row.get(6)?,
                })
            })
            .context("failed to read legacy linked automation reconciliation")?;
        rows.collect::<std::result::Result<Vec<_>, _>>()?
    };

    for pair in pairs {
        let prior_committed = pair.output_commit.is_some()
            || pair.occurrence_delivery_state == "committed"
            || pair.run_delivery_state == "committed";
        let prior_ambiguous =
            pair.occurrence_delivery_state == "ambiguous" || pair.run_delivery_state == "ambiguous";
        let (delivery_state, delivery_note) = if prior_committed {
            (
                "committed",
                "committed delivery evidence preserved; terminal outcome remains ambiguous",
            )
        } else if prior_ambiguous {
            (
                "ambiguous",
                "prior ambiguous delivery evidence preserved; delivery outcome remains ambiguous",
            )
        } else if pair.occurrence_state == "failed" && pair.run_status == "failed" {
            (
                "failed",
                "coherent failed evidence excludes successful delivery",
            )
        } else {
            (
                "ambiguous",
                "delivery outcome ambiguous; pre-ledger file delivery cannot be excluded",
            )
        };
        let reason = format!(
            "{BASE_REASON}; original occurrence={}; original run={}; \
             reconciled outcome=failed; {delivery_note}",
            pair.occurrence_state, pair.run_status
        );
        let occurrence_changed = conn
            .execute(
                "UPDATE automation_occurrences
                 SET state = 'failed',
                     failure_reason = CASE
                         WHEN failure_reason IS NULL OR trim(failure_reason) = '' THEN ?2
                         ELSE failure_reason || '; ' || ?2
                     END,
                     lease_owner = NULL,
                     lease_expires_at = NULL,
                     delivery_state = ?3,
                     delivery_error = CASE
                         WHEN delivery_error IS NULL OR trim(delivery_error) = '' THEN ?2
                         ELSE delivery_error || '; ' || ?2
                     END,
                     legacy_reconciled_at = ?4
                 WHERE id = ?1",
                params![pair.occurrence_id, reason, delivery_state, now],
            )
            .with_context(|| {
                format!(
                    "failed to reconcile legacy occurrence {}",
                    pair.occurrence_id
                )
            })?;
        let run_changed = conn
            .execute(
                "UPDATE automation_runs
                 SET status = 'failed',
                     delivery_state = ?3,
                     delivery_error = CASE
                         WHEN delivery_error IS NULL OR trim(delivery_error) = '' THEN ?2
                         ELSE delivery_error || '; ' || ?2
                     END,
                     finished_at = COALESCE(finished_at, ?4),
                     legacy_reconciled_at = ?4
                 WHERE id = ?1",
                params![pair.run_id, reason, delivery_state, now],
            )
            .with_context(|| format!("failed to reconcile legacy run {}", pair.run_id))?;
        anyhow::ensure!(
            occurrence_changed == 1 && run_changed == 1,
            "legacy linked automation pair changed during reconciliation"
        );
    }

    conn.execute(
        "UPDATE automation_occurrences
             SET state = 'failed',
                 failure_reason = CASE
                     WHEN failure_reason IS NULL OR trim(failure_reason) = '' THEN ?1
                     ELSE failure_reason || '; ' || ?1
                 END,
                 lease_owner = NULL,
                 lease_expires_at = NULL,
                 delivery_state = CASE
                     WHEN delivery_state IN ('committed', 'ambiguous') THEN delivery_state
                     ELSE 'failed'
                 END,
                 delivery_error = CASE
                     WHEN delivery_error IS NULL OR trim(delivery_error) = '' THEN ?1
                     ELSE delivery_error || '; ' || ?1
                 END,
                 legacy_reconciled_at = ?2
             WHERE state IN ('claimed', 'running')
               AND definition_json IS NULL
               AND legacy_reconciled_at IS NULL
               AND NOT EXISTS (
                   SELECT 1 FROM automation_runs
                   WHERE automation_runs.occurrence_id = automation_occurrences.id
               )",
        params![BASE_REASON, now],
    )
    .context("failed to fail unlinked legacy occurrences without immutable snapshots")?;
    conn.execute(
        "UPDATE automation_runs
             SET status = 'failed',
                 delivery_state = CASE
                     WHEN output_commit IS NOT NULL OR delivery_state = 'committed'
                         THEN 'committed'
                     WHEN delivery_state = 'ambiguous' THEN 'ambiguous'
                     WHEN status = 'failed' THEN 'failed'
                     ELSE 'ambiguous'
                 END,
                 delivery_error = CASE
                     WHEN delivery_error IS NULL OR trim(delivery_error) = '' THEN ?1
                     ELSE delivery_error || '; ' || ?1
                 END,
                 finished_at = COALESCE(finished_at, ?2),
                 legacy_reconciled_at = ?2
             WHERE status = 'running'
               AND definition_json IS NULL
               AND legacy_reconciled_at IS NULL
               AND (
                   occurrence_id IS NULL
                   OR NOT EXISTS (
                       SELECT 1 FROM automation_occurrences
                       WHERE automation_occurrences.id = automation_runs.occurrence_id
                   )
               )",
        params![BASE_REASON, now],
    )
    .context("failed to fail unlinked legacy runs without immutable snapshots")?;
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

    fn create_legacy_automation_tables(conn: &Connection) {
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
    }

    fn insert_legacy_unsettled(conn: &Connection, definition_json: &str, occurrence_state: &str) {
        insert_legacy_pair(conn, definition_json, occurrence_state, "running", None);
    }

    fn insert_legacy_pair(
        conn: &Connection,
        definition_json: &str,
        occurrence_state: &str,
        run_status: &str,
        output_commit: Option<&str>,
    ) {
        conn.execute(
            "INSERT INTO automation_definitions
                (id, name, status, definition_json, created_at, updated_at)
             VALUES ('legacy', 'legacy', 'ACTIVE', ?1,
                     '2026-01-01T00:00:00.000Z', '2026-01-01T00:00:00.000Z')",
            rusqlite::params![definition_json],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO automation_occurrences
                (id, automation_id, scheduled_for, state, lease_owner, lease_expires_at,
                 attempt, created_at, updated_at)
             VALUES ('legacy-occ', 'legacy', '2026-01-02T09:00:00.000Z', ?1,
                     'legacy-daemon', '2026-01-02T10:00:00.000Z', 1,
                     '2026-01-02T09:00:00.000Z', '2026-01-02T09:00:00.000Z')",
            rusqlite::params![occurrence_state],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO automation_runs
                (id, automation_id, occurrence_id, session_id, familiar_id, runtime,
                 status, output_commit, started_at)
             VALUES ('legacy-run', 'legacy', 'legacy-occ', 'legacy-session', NULL,
                     'coven-code', ?1, ?2, '2026-01-02T09:00:00.000Z')",
            rusqlite::params![run_status, output_commit],
        )
        .unwrap();
    }

    #[test]
    fn legacy_automation_tables_migrate_snapshot_columns_in_place() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("legacy.sqlite");
        let definition_json = serde_json::to_string(&definition("legacy")).unwrap();
        {
            let conn = Connection::open(&path).unwrap();
            create_legacy_automation_tables(&conn);
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
            ("automation_occurrences", "delivery_state"),
            ("automation_occurrences", "legacy_reconciled_at"),
            ("automation_runs", "definition_json"),
            ("automation_runs", "deadline_at"),
            ("automation_runs", "output_target"),
            ("automation_runs", "delivery_state"),
            ("automation_runs", "legacy_reconciled_at"),
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

    #[test]
    fn legacy_unsettled_run_does_not_adopt_an_edited_current_definition() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("edited.sqlite");
        let mut original = definition("legacy");
        original.status = RoutineStatus::Active;
        original.timeout_minutes = 5;
        original.prompt = "original prompt".to_string();
        original.output_target = Some("/original/output".to_string());
        {
            let conn = Connection::open(&path).unwrap();
            create_legacy_automation_tables(&conn);
            insert_legacy_unsettled(&conn, &serde_json::to_string(&original).unwrap(), "running");
            let mut edited = original.clone();
            edited.timeout_minutes = 60;
            edited.prompt = "edited prompt".to_string();
            edited.output_target = Some("/edited/output".to_string());
            conn.execute(
                "UPDATE automation_definitions
                 SET definition_json = ?1, updated_at = '2026-01-03T00:00:00.000Z'",
                rusqlite::params![serde_json::to_string(&edited).unwrap()],
            )
            .unwrap();
        }

        initialize_store(&path).unwrap();
        let conn = crate::store::open_store(&path).unwrap();
        let occurrence: (String, String, Option<String>, Option<String>) = conn
            .query_row(
                "SELECT state, failure_reason, definition_json, output_target
                 FROM automation_occurrences WHERE id = 'legacy-occ'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(occurrence.0, "failed");
        assert!(occurrence.1.contains("immutable snapshot unavailable"));
        assert_eq!(occurrence.2, None);
        assert_eq!(occurrence.3, None);
        let run: (String, Option<String>, Option<String>) = conn
            .query_row(
                "SELECT status, definition_json, output_target
                 FROM automation_runs WHERE id = 'legacy-run'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(run, ("failed".to_string(), None, None));
    }

    #[test]
    fn legacy_unsettled_run_with_deleted_definition_is_preserved_and_failed() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("deleted.sqlite");
        let mut original = definition("legacy");
        original.status = RoutineStatus::Active;
        {
            let conn = Connection::open(&path).unwrap();
            create_legacy_automation_tables(&conn);
            insert_legacy_unsettled(&conn, &serde_json::to_string(&original).unwrap(), "claimed");
            conn.execute("DELETE FROM automation_definitions WHERE id = 'legacy'", [])
                .unwrap();
        }

        initialize_store(&path).unwrap();
        let conn = crate::store::open_store(&path).unwrap();
        let occurrence: (String, String, Option<String>) = conn
            .query_row(
                "SELECT state, failure_reason, definition_json
                 FROM automation_occurrences WHERE id = 'legacy-occ'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(occurrence.0, "failed");
        assert!(occurrence.1.contains("immutable snapshot unavailable"));
        assert_eq!(occurrence.2, None);
        let run: (String, Option<String>, Option<String>) = conn
            .query_row(
                "SELECT status, definition_json, output_target
                 FROM automation_runs WHERE id = 'legacy-run'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(run, ("failed".to_string(), None, None));
    }

    fn migrated_legacy_pair(
        occurrence_state: &str,
        run_status: &str,
        output_commit: Option<&str>,
    ) -> (
        String,
        String,
        String,
        String,
        String,
        String,
        Option<String>,
    ) {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("coherence.sqlite");
        let mut routine = definition("legacy");
        routine.status = RoutineStatus::Active;
        {
            let conn = Connection::open(&path).unwrap();
            create_legacy_automation_tables(&conn);
            insert_legacy_pair(
                &conn,
                &serde_json::to_string(&routine).unwrap(),
                occurrence_state,
                run_status,
                output_commit,
            );
        }
        initialize_store(&path).unwrap();
        let conn = crate::store::open_store(&path).unwrap();
        conn.query_row(
            "SELECT o.state, o.failure_reason, o.delivery_state,
                    r.status, r.delivery_state, r.delivery_error, r.output_commit
             FROM automation_occurrences AS o
             JOIN automation_runs AS r ON r.occurrence_id = o.id
             WHERE o.id = 'legacy-occ'",
            [],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                ))
            },
        )
        .unwrap()
    }

    #[test]
    fn legacy_occurrence_failed_run_succeeded_reconciles_coherently() {
        let migrated = migrated_legacy_pair("failed", "succeeded", None);

        assert_eq!(migrated.0, "failed");
        assert_eq!(migrated.3, "failed");
        assert_eq!(migrated.2, "ambiguous");
        assert_eq!(migrated.4, "ambiguous");
        assert!(migrated.1.contains("occurrence=failed"));
        assert!(migrated.1.contains("run=succeeded"));
        assert!(migrated.5.contains("delivery outcome ambiguous"));
    }

    #[test]
    fn legacy_occurrence_succeeded_run_failed_reconciles_coherently() {
        let migrated = migrated_legacy_pair("succeeded", "failed", None);

        assert_eq!(migrated.0, "failed");
        assert_eq!(migrated.3, "failed");
        assert_eq!(migrated.2, "ambiguous");
        assert_eq!(migrated.4, "ambiguous");
        assert!(migrated.1.contains("occurrence=succeeded"));
        assert!(migrated.1.contains("run=failed"));
        assert!(migrated.5.contains("delivery outcome ambiguous"));
    }

    #[test]
    fn legacy_possible_delivery_before_ledger_crash_is_explicitly_ambiguous() {
        let migrated = migrated_legacy_pair("running", "running", None);

        assert_eq!(migrated.0, "failed");
        assert_eq!(migrated.3, "failed");
        assert_eq!(migrated.2, "ambiguous");
        assert_eq!(migrated.4, "ambiguous");
        assert!(migrated.1.contains("occurrence=running"));
        assert!(migrated.1.contains("run=running"));
        assert!(migrated.5.contains("delivery outcome ambiguous"));
        assert_eq!(migrated.6, None);
    }

    #[test]
    fn legacy_linked_pair_reconciliation_rolls_back_both_sides_on_failure() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("atomic.sqlite");
        let mut routine = definition("legacy");
        routine.status = RoutineStatus::Active;
        {
            let conn = Connection::open(&path).unwrap();
            create_legacy_automation_tables(&conn);
            insert_legacy_pair(
                &conn,
                &serde_json::to_string(&routine).unwrap(),
                "succeeded",
                "failed",
                None,
            );
            conn.execute_batch(
                "CREATE TRIGGER reject_legacy_run_reconciliation
                 BEFORE UPDATE ON automation_runs
                 BEGIN
                     SELECT RAISE(ABORT, 'synthetic legacy reconciliation failure');
                 END;",
            )
            .unwrap();
        }

        let error = initialize_store(&path).unwrap_err();
        assert!(
            format!("{error:#}").contains("synthetic legacy reconciliation failure"),
            "{error:#}"
        );
        let conn = Connection::open(&path).unwrap();
        let states: (String, String) = conn
            .query_row(
                "SELECT o.state, r.status
                 FROM automation_occurrences AS o
                 JOIN automation_runs AS r ON r.occurrence_id = o.id
                 WHERE o.id = 'legacy-occ'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(states, ("succeeded".to_string(), "failed".to_string()));
    }

    #[test]
    fn legacy_linked_pair_reconciliation_is_identical_after_second_startup() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("idempotent.sqlite");
        let mut routine = definition("legacy");
        routine.status = RoutineStatus::Active;
        {
            let conn = Connection::open(&path).unwrap();
            create_legacy_automation_tables(&conn);
            insert_legacy_pair(
                &conn,
                &serde_json::to_string(&routine).unwrap(),
                "failed",
                "succeeded",
                None,
            );
            conn.execute(
                "UPDATE automation_occurrences
                 SET failure_reason = 'original occurrence diagnostic'
                 WHERE id = 'legacy-occ'",
                [],
            )
            .unwrap();
            conn.execute_batch(
                "ALTER TABLE automation_runs
                     ADD COLUMN delivery_state TEXT NOT NULL DEFAULT 'none';
                 ALTER TABLE automation_runs ADD COLUMN delivery_error TEXT;
                 UPDATE automation_runs
                 SET delivery_error = 'original delivery diagnostic'
                 WHERE id = 'legacy-run';",
            )
            .unwrap();
        }

        initialize_store(&path).unwrap();
        let first = {
            let conn = crate::store::open_store(&path).unwrap();
            conn.query_row(
                "SELECT o.state, o.failure_reason, o.delivery_state,
                        r.status, r.delivery_state, r.delivery_error
                 FROM automation_occurrences AS o
                 JOIN automation_runs AS r ON r.occurrence_id = o.id
                 WHERE o.id = 'legacy-occ'",
                [],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, String>(5)?,
                    ))
                },
            )
            .unwrap()
        };
        initialize_store(&path).unwrap();
        let second = {
            let conn = crate::store::open_store(&path).unwrap();
            conn.query_row(
                "SELECT o.state, o.failure_reason, o.delivery_state,
                        r.status, r.delivery_state, r.delivery_error
                 FROM automation_occurrences AS o
                 JOIN automation_runs AS r ON r.occurrence_id = o.id
                 WHERE o.id = 'legacy-occ'",
                [],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, String>(5)?,
                    ))
                },
            )
            .unwrap()
        };

        assert_eq!(second, first);
        assert_eq!(first.0, "failed");
        assert_eq!(first.2, "ambiguous");
        assert_eq!(first.3, "failed");
        assert_eq!(first.4, "ambiguous");
        assert!(first.1.contains("original occurrence diagnostic"));
        assert!(first.5.contains("original delivery diagnostic"));
        assert!(first.1.contains("occurrence=failed"));
        assert!(first.5.contains("run=succeeded"));
    }

    #[test]
    fn unlinked_legacy_occurrence_preserves_diagnostics_and_delivery_across_startups() {
        for delivery_state in ["committed", "ambiguous"] {
            let temp = tempfile::tempdir().unwrap();
            let path = temp
                .path()
                .join(format!("unlinked-occurrence-{delivery_state}.sqlite"));
            {
                let conn = Connection::open(&path).unwrap();
                create_legacy_automation_tables(&conn);
                conn.execute_batch(
                    "ALTER TABLE automation_occurrences
                         ADD COLUMN delivery_state TEXT NOT NULL DEFAULT 'none';
                     ALTER TABLE automation_occurrences ADD COLUMN delivery_error TEXT;",
                )
                .unwrap();
                conn.execute(
                    "INSERT INTO automation_occurrences
                        (id, automation_id, scheduled_for, state, lease_owner, lease_expires_at,
                         attempt, failure_reason, delivery_state, delivery_error,
                         created_at, updated_at)
                     VALUES ('unlinked-occ', 'missing-definition',
                             '2026-01-02T09:00:00.000Z', 'running', 'legacy-daemon',
                             '2026-01-02T10:00:00.000Z', 1,
                             'original occurrence diagnostic', ?1,
                             'original occurrence delivery diagnostic',
                             '2026-01-02T09:00:00.000Z',
                             '2026-01-02T09:00:00.000Z')",
                    rusqlite::params![delivery_state],
                )
                .unwrap();
            }

            initialize_store(&path).unwrap();
            let first: (String, String, String, String, Option<String>) = {
                let conn = crate::store::open_store(&path).unwrap();
                conn.query_row(
                    "SELECT state, failure_reason, delivery_state, delivery_error,
                            legacy_reconciled_at
                     FROM automation_occurrences WHERE id = 'unlinked-occ'",
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
                .unwrap()
            };
            initialize_store(&path).unwrap();
            let second: (String, String, String, String, Option<String>) = {
                let conn = crate::store::open_store(&path).unwrap();
                conn.query_row(
                    "SELECT state, failure_reason, delivery_state, delivery_error,
                            legacy_reconciled_at
                     FROM automation_occurrences WHERE id = 'unlinked-occ'",
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
                .unwrap()
            };

            assert_eq!(second, first);
            assert_eq!(first.0, "failed");
            assert_eq!(first.2, delivery_state);
            assert!(first.1.contains("original occurrence diagnostic"));
            assert!(first.1.contains("immutable snapshot unavailable"));
            assert!(first.3.contains("original occurrence delivery diagnostic"));
            assert!(first.4.is_some());
        }
    }

    #[test]
    fn unlinked_legacy_run_preserves_diagnostics_and_delivery_across_startups() {
        for delivery_state in ["committed", "ambiguous"] {
            let temp = tempfile::tempdir().unwrap();
            let path = temp
                .path()
                .join(format!("unlinked-run-{delivery_state}.sqlite"));
            {
                let conn = Connection::open(&path).unwrap();
                create_legacy_automation_tables(&conn);
                conn.execute_batch(
                    "ALTER TABLE automation_runs
                         ADD COLUMN delivery_state TEXT NOT NULL DEFAULT 'none';
                     ALTER TABLE automation_runs ADD COLUMN delivery_error TEXT;",
                )
                .unwrap();
                conn.execute(
                    "INSERT INTO automation_runs
                        (id, automation_id, occurrence_id, session_id, familiar_id, runtime,
                         status, output_commit, delivery_state, delivery_error, started_at)
                     VALUES ('unlinked-run', 'missing-definition', NULL, 'legacy-session',
                             NULL, 'coven-code', 'running', NULL, ?1,
                             'original run delivery diagnostic',
                             '2026-01-02T09:00:00.000Z')",
                    rusqlite::params![delivery_state],
                )
                .unwrap();
            }

            initialize_store(&path).unwrap();
            let first: (String, String, String, Option<String>, Option<String>) = {
                let conn = crate::store::open_store(&path).unwrap();
                conn.query_row(
                    "SELECT status, delivery_state, delivery_error,
                            finished_at, legacy_reconciled_at
                     FROM automation_runs WHERE id = 'unlinked-run'",
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
                .unwrap()
            };
            initialize_store(&path).unwrap();
            let second: (String, String, String, Option<String>, Option<String>) = {
                let conn = crate::store::open_store(&path).unwrap();
                conn.query_row(
                    "SELECT status, delivery_state, delivery_error,
                            finished_at, legacy_reconciled_at
                     FROM automation_runs WHERE id = 'unlinked-run'",
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
                .unwrap()
            };

            assert_eq!(second, first);
            assert_eq!(first.0, "failed");
            assert_eq!(first.1, delivery_state);
            assert!(first.2.contains("original run delivery diagnostic"));
            assert!(first.2.contains("immutable snapshot unavailable"));
            assert!(first.3.is_some());
            assert!(first.4.is_some());
        }
    }
}
