//! Occurrence planning with misfire-latest semantics (coven#816).
//!
//! The planner walks each ACTIVE routine's schedule forward from a bounded
//! lookback and plans exactly the latest due slot: if the daemon was down
//! for three days of a daily routine, only the most recent missed slot is
//! fenced — earlier slots are collapsed, never backfilled. The
//! `UNIQUE(automation_id, scheduled_for)` fence makes planning idempotent
//! across ticks, replicas, and restarts.

use std::collections::BTreeSet;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};

use super::definition::{RoutineDefinition, RoutineStatus};
use crate::automations::schedule::next_due;

pub const AUTOMATION_OCCURRENCES_SCHEMA_SQL: &str = "
    CREATE TABLE IF NOT EXISTS automation_occurrences (
        id TEXT PRIMARY KEY NOT NULL,
        automation_id TEXT NOT NULL,
        automation_revision INTEGER NOT NULL DEFAULT 1 CHECK (automation_revision >= 1),
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

    CREATE INDEX IF NOT EXISTS idx_automation_occurrences_scheduled
        ON automation_occurrences(automation_id, scheduled_for);

    CREATE INDEX IF NOT EXISTS idx_automation_occurrences_state
        ON automation_occurrences(state, lease_expires_at);
";

/// Migrates the occurrence source discriminator inside the store
/// initialization transaction owned by `initialize_store`.
pub fn ensure_occurrence_kind(conn: &Connection) -> Result<()> {
    let has_kind = conn
        .prepare("PRAGMA table_info(automation_occurrences)")
        .context("failed to inspect automation_occurrences schema")?
        .query_map([], |row| row.get::<_, String>(1))
        .context("failed to query automation_occurrences schema")?
        .collect::<std::result::Result<Vec<_>, _>>()
        .context("failed to read automation_occurrences schema")?
        .into_iter()
        .any(|column| column == "kind");
    if has_kind {
        return Ok(());
    }

    conn.execute(
        "ALTER TABLE automation_occurrences
         ADD COLUMN kind TEXT NOT NULL DEFAULT 'scheduled'",
        [],
    )
    .context("failed to add automation_occurrences.kind")?;

    let rows: Vec<(String, String, String)> = {
        let mut statement = conn
            .prepare("SELECT id, automation_id, scheduled_for FROM automation_occurrences")
            .context("failed to prepare automation occurrence migration")?;
        let rows = statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .context("failed to query automation occurrences for migration")?
            .collect::<std::result::Result<Vec<_>, _>>()
            .context("failed to read automation occurrences for migration")?;
        rows
    };
    for (id, automation_id, scheduled_for) in rows {
        let scheduled_at = chrono::DateTime::parse_from_rfc3339(&scheduled_for)
            .with_context(|| format!("occurrence `{id}` has invalid scheduled_for"))?
            .with_timezone(&Utc);
        let scheduled_id = format!("{automation_id}-{}", scheduled_at.timestamp_millis());
        if id == scheduled_id {
            continue;
        }
        conn.execute(
            "UPDATE automation_occurrences
             SET kind = 'manual', scheduled_for = ?2
             WHERE id = ?1",
            params![
                id,
                scheduled_at.to_rfc3339_opts(chrono::SecondsFormat::Nanos, true)
            ],
        )
        .with_context(|| format!("failed to migrate manual occurrence `{id}`"))?;
    }
    Ok(())
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
#[allow(dead_code)] // planning-only report; the production tick reports the superset TickReport
pub struct PlanTickReport {
    pub planned: Vec<String>,
    pub already_fenced: usize,
    pub paused_skipped: usize,
    pub failed: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedOccurrence {
    pub id: String,
    pub automation_id: String,
    pub scheduled_for: String,
    pub state: String,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct TickReport {
    pub planned: Vec<String>,
    pub already_fenced: usize,
    pub paused_skipped: usize,
    pub recovered: usize,
    pub claimed: Vec<String>,
    pub failed: Vec<String>,
}

const OCCURRENCE_TERMINAL_STATES: [&str; 2] = ["succeeded", "failed"];

/// Claims the earliest due PLANNED occurrence for a routine with a bounded
/// lease. Returns the claimed occurrence id, or `None` when nothing is due.
/// The compare-and-set WHERE clause makes claims race-safe across callers.
///
/// The runner (coven#816 part 4) is the production caller; until then tests
/// and the daemon tick exercise the path directly.
#[allow(dead_code)]
pub fn claim_due_occurrence(
    conn: &Connection,
    automation_id: &str,
    owner: &str,
    lease_minutes: i64,
    now: DateTime<Utc>,
) -> Result<Option<String>, String> {
    if lease_minutes <= 0 || lease_minutes > 24 * 60 {
        return Err("lease minutes must be 1..=1440".to_string());
    }
    let expires = now + chrono::Duration::minutes(lease_minutes);
    let now_iso = iso(now);
    let expires_iso = iso(expires);
    let transaction = conn
        .unchecked_transaction()
        .map_err(|error| format!("failed to start occurrence claim transaction: {error}"))?;
    transaction
        .execute(
            "UPDATE automation_occurrences
             SET state = 'skipped',
                 failure_reason = 'superseded by latest misfire policy',
                 updated_at = ?2
             WHERE automation_id = ?1
               AND state = 'planned'
               AND scheduled_for <= ?2
               AND scheduled_for < (
                   SELECT MAX(scheduled_for)
                   FROM automation_occurrences
                   WHERE automation_id = ?1
                     AND state = 'planned'
                     AND scheduled_for <= ?2
               )",
            params![automation_id, now_iso],
        )
        .map_err(|error| format!("failed to supersede stale occurrence fences: {error}"))?;
    let changed = transaction
        .execute(
            "UPDATE automation_occurrences
             SET state = 'claimed',
                 lease_owner = ?3,
                 lease_expires_at = ?4,
                 attempt = attempt + 1,
                 updated_at = ?2
             WHERE automation_id = ?1
               AND state = 'planned'
               AND scheduled_for <= ?2
               AND NOT EXISTS (
                   SELECT 1 FROM automation_occurrences
                   WHERE automation_id = ?1 AND state IN ('claimed', 'running')
               )
               AND NOT EXISTS (
                   SELECT 1 FROM automation_runs
                   WHERE automation_id = ?1 AND status = 'running'
               )
               AND id = (
                   SELECT id FROM automation_occurrences
                   WHERE automation_id = ?1
                     AND state = 'planned'
                     AND scheduled_for <= ?2
                   ORDER BY scheduled_for DESC
                   LIMIT 1
               )",
            params![automation_id, now_iso, owner, expires_iso],
        )
        .map_err(|error| format!("failed to claim occurrence: {error}"))?;
    if changed == 0 {
        transaction
            .commit()
            .map_err(|error| format!("failed to commit occurrence claim: {error}"))?;
        return Ok(None);
    }
    let id: String = transaction
        .query_row(
            "SELECT id FROM automation_occurrences WHERE automation_id = ?1 AND state = 'claimed' AND lease_owner = ?2 ORDER BY scheduled_for DESC LIMIT 1",
            params![automation_id, owner],
            |row| row.get(0),
        )
        .map_err(|error| format!("failed to read claim: {error}"))?;
    transaction
        .commit()
        .map_err(|error| format!("failed to commit occurrence claim: {error}"))?;
    Ok(Some(id))
}

/// Creates a manually requested occurrence already claimed by its caller.
/// The single insert prevents a scheduler connection from claiming the row
/// between manual fencing and ownership publication.
pub fn insert_claimed_occurrence(
    conn: &Connection,
    occurrence_id: &str,
    automation_id: &str,
    owner: &str,
    lease_minutes: i64,
    now: DateTime<Utc>,
) -> Result<bool, String> {
    if lease_minutes <= 0 || lease_minutes > 24 * 60 {
        return Err("lease minutes must be 1..=1440".to_string());
    }
    let expires = now + chrono::Duration::minutes(lease_minutes);
    let now_iso = iso(now);
    let manual_scheduled_for = now.to_rfc3339_opts(chrono::SecondsFormat::Nanos, true);
    let expires_iso = iso(expires);
    conn.execute(
        "INSERT OR IGNORE INTO automation_occurrences
            (id, automation_id, automation_revision, definition_digest, scheduled_for, kind,
             state, lease_owner, lease_expires_at, attempt, created_at, updated_at)
         SELECT ?1, ?2, definition.revision, definition.definition_digest, ?5, 'manual',
                'claimed', ?3, ?4, 1, ?6, ?6
         FROM automation_definitions AS definition
         WHERE NOT EXISTS (
             SELECT 1 FROM automation_occurrences
             WHERE automation_id = ?2 AND state IN ('claimed', 'running')
         )
           AND NOT EXISTS (
             SELECT 1 FROM automation_runs
             WHERE automation_id = ?2 AND status = 'running'
         )
           AND definition.id = ?2
           AND definition.tombstoned_at IS NULL
           AND definition.definition_digest IS NOT NULL",
        params![
            occurrence_id,
            automation_id,
            owner,
            expires_iso,
            manual_scheduled_for,
            now_iso
        ],
    )
    .map(|inserted| inserted == 1)
    .map_err(|error| format!("failed to insert claimed occurrence `{occurrence_id}`: {error}"))
}

/// Marks pre-dispatch claims whose lease has expired as failed. Once runtime
/// ownership is published, only terminal session evidence may settle the
/// occurrence; an execution lease is not completion evidence.
pub fn recover_expired_leases(conn: &Connection, now: DateTime<Utc>) -> Result<usize, String> {
    let now_iso = iso(now);
    let changed = conn
        .execute(
            "UPDATE automation_occurrences
             SET state = 'failed',
                 failure_reason = 'lease expired',
                 lease_owner = NULL,
                 lease_expires_at = NULL,
                 updated_at = ?1
             WHERE state = 'claimed'
               AND lease_expires_at IS NOT NULL
               AND lease_expires_at <= ?1
               AND NOT EXISTS (
                   SELECT 1 FROM automation_runs
                   WHERE automation_runs.occurrence_id = automation_occurrences.id
                     AND automation_runs.status = 'running'
               )",
            params![now_iso],
        )
        .map_err(|error| format!("failed to recover expired leases: {error}"))?;
    Ok(changed)
}

/// Finalizes an occurrence into a terminal state. Releasing a PLANNED
/// occurrence is refused — only claimed work can settle.
#[allow(dead_code)]
pub fn settle_occurrence(
    conn: &Connection,
    occurrence_id: &str,
    terminal_state: &str,
    failure_reason: Option<&str>,
    now: DateTime<Utc>,
) -> Result<bool, String> {
    if !OCCURRENCE_TERMINAL_STATES.contains(&terminal_state) {
        return Err(format!(
            "terminal state must be one of {OCCURRENCE_TERMINAL_STATES:?}"
        ));
    }
    let now_iso = iso(now);
    let changed = conn
        .execute(
            "UPDATE automation_occurrences
             SET state = ?2,
                 failure_reason = ?3,
                 lease_owner = NULL,
                 lease_expires_at = NULL,
                 updated_at = ?4
             WHERE id = ?1 AND state IN ('claimed', 'running')",
            params![occurrence_id, terminal_state, failure_reason, now_iso],
        )
        .map_err(|error| format!("failed to settle occurrence: {error}"))?;
    Ok(changed > 0)
}

/// Publishes runtime ownership for a claimed occurrence without settling it.
/// The lease remains attached until terminal session evidence is reconciled.
pub fn mark_occurrence_running(
    conn: &Connection,
    occurrence_id: &str,
    now: DateTime<Utc>,
) -> Result<bool, String> {
    let changed = conn
        .execute(
            "UPDATE automation_occurrences
             SET state = 'running', updated_at = ?2
             WHERE id = ?1 AND state = 'claimed'",
            params![occurrence_id, iso(now)],
        )
        .map_err(|error| format!("failed to mark occurrence running: {error}"))?;
    Ok(changed > 0)
}

/// One full tick: plan due slots, recover expired leases, then claim the
/// earliest due occurrence of every ACTIVE routine that has one.
pub fn tick(conn: &Connection, now: DateTime<Utc>) -> Result<TickReport> {
    let mut report = TickReport::default();
    let mut seen: BTreeSet<String> = BTreeSet::new();

    let (definitions, failures) = active_definitions(conn)?;
    report.failed.extend(failures);
    for definition in &definitions {
        if !seen.insert(definition.id.clone()) {
            continue;
        }
        let created_at = definition_created_at(conn, &definition.id).unwrap_or(now);
        match plan_latest_due_occurrence(conn, definition, created_at, now) {
            Ok(PlanOutcome::Planned(occurrence)) => report.planned.push(occurrence.id),
            Ok(PlanOutcome::AlreadyFenced) => report.already_fenced += 1,
            Ok(PlanOutcome::NotDue) => {}
            Err(error) => report.failed.push(format!("{}: {error}", definition.id)),
        }
    }

    report.recovered = recover_expired_leases(conn, now).unwrap_or(0);

    for definition in &definitions {
        match claim_due_occurrence(conn, &definition.id, "daemon", 60, now) {
            Ok(Some(id)) => report.claimed.push(id),
            Ok(None) => {}
            Err(error) => report.failed.push(format!("{}: {error}", definition.id)),
        }
    }

    let paused: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM automation_definitions WHERE status = 'PAUSED'",
            [],
            |row| row.get(0),
        )
        .context("failed to count paused routines")?;
    report.paused_skipped = paused as usize;

    Ok(report)
}

fn iso(instant: DateTime<Utc>) -> String {
    instant.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// Reads the ACTIVE definitions from the store as validated records.
fn active_definitions(conn: &Connection) -> Result<(Vec<RoutineDefinition>, Vec<String>)> {
    let records = super::store::list_definitions(conn)?;
    let mut definitions = Vec::new();
    let mut failures = Vec::new();
    for record in records {
        if record.status != "ACTIVE" {
            continue;
        }
        let definition: RoutineDefinition = match serde_json::from_str(&record.definition_json) {
            Ok(definition) => definition,
            Err(error) => {
                failures.push(format!(
                    "stored routine `{}` is unreadable: {error}",
                    record.id
                ));
                continue;
            }
        };
        if let Err(error) = definition.validate() {
            failures.push(format!(
                "stored routine `{}` is invalid: {error}",
                record.id
            ));
            continue;
        }
        if let Err(error) = definition.validate_durable() {
            failures.push(format!(
                "stored routine `{}` is invalid: {error}",
                record.id
            ));
            continue;
        }
        if definition.status != RoutineStatus::Active {
            continue;
        }
        definitions.push(definition);
    }
    Ok((definitions, failures))
}

fn definition_created_at(conn: &Connection, id: &str) -> Result<DateTime<Utc>> {
    let created: String = conn
        .query_row(
            "SELECT created_at FROM automation_definitions WHERE id = ?1",
            params![id],
            |row| row.get(0),
        )
        .with_context(|| format!("routine `{id}` has no definition row"))?;
    chrono::DateTime::parse_from_rfc3339(&created)
        .map(|parsed| parsed.with_timezone(&Utc))
        .context("routine created_at is not a valid RFC3339 timestamp")
}

/// Latest due slot for `definition` at or before `now`, walking forward from
/// `cursor` (never further back than the routine's creation time). Returns
/// `None` when the next slot is still in the future.
fn latest_due_slot_after(
    definition: &RoutineDefinition,
    cursor: DateTime<Utc>,
    now: DateTime<Utc>,
) -> Result<Option<DateTime<Utc>>, String> {
    // A weekly local-time slot can be skipped by a DST gap, making the next
    // valid occurrence two weeks later. Starting just beyond that maximum
    // gap finds the latest slot without replaying the entire downtime.
    let mut walk = cursor.max(now - chrono::Duration::days(15));
    let mut latest: Option<DateTime<Utc>> = None;

    while let Some(next) = next_due(&definition.rrule, definition.timezone, walk)? {
        if next <= walk {
            return Err("schedule did not advance while finding the latest due slot".to_string());
        }
        if next > now {
            break;
        }
        latest = Some(next);
        walk = next;
    }

    Ok(latest)
}

fn latest_scheduled_slot(
    conn: &Connection,
    automation_id: &str,
    automation_revision: u64,
    definition_digest: &str,
) -> Result<Option<DateTime<Utc>>, String> {
    let latest: Option<String> = conn
        .query_row(
            "SELECT MAX(scheduled_for)
             FROM automation_occurrences
             WHERE automation_id = ?1
               AND automation_revision = ?2
               AND definition_digest = ?3
               AND kind = 'scheduled'",
            params![
                automation_id,
                i64::try_from(automation_revision)
                    .map_err(|_| "definition revision exceeds SQLite range")?,
                definition_digest,
            ],
            |row| row.get(0),
        )
        .map_err(|error| format!("failed to read latest scheduled occurrence: {error}"))?;
    latest
        .map(|timestamp| {
            DateTime::parse_from_rfc3339(&timestamp)
                .map(|timestamp| timestamp.with_timezone(&Utc))
                .map_err(|error| {
                    format!(
                        "routine `{automation_id}` has an invalid scheduled occurrence timestamp: {error}"
                    )
                })
        })
        .transpose()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanOutcome {
    Planned(PlannedOccurrence),
    NotDue,
    AlreadyFenced,
}

/// Fences the latest due slot for one routine.
///
/// The bounded walk starts at the definition's creation time. Stored
/// occurrences are only idempotency fences; manual-run timestamps never
/// advance the schedule cursor.
pub fn plan_latest_due_occurrence(
    conn: &Connection,
    definition: &RoutineDefinition,
    created_at: DateTime<Utc>,
    now: DateTime<Utc>,
) -> Result<PlanOutcome, String> {
    let owns_transaction = conn.is_autocommit();
    if owns_transaction {
        conn.execute_batch("BEGIN IMMEDIATE")
            .map_err(|error| format!("failed to begin occurrence planning transaction: {error}"))?;
    } else {
        conn.execute_batch("SAVEPOINT coven_automation_occurrence_planning")
            .map_err(|error| format!("failed to create occurrence planning savepoint: {error}"))?;
    }

    let result = plan_latest_due_occurrence_in_transaction(conn, definition, created_at, now);
    match result {
        Ok(outcome) => {
            let committed = if owns_transaction {
                conn.execute_batch("COMMIT")
                    .map_err(|error| format!("failed to commit occurrence planning: {error}"))
            } else {
                conn.execute_batch("RELEASE SAVEPOINT coven_automation_occurrence_planning")
                    .map_err(|error| {
                        format!("failed to release occurrence planning savepoint: {error}")
                    })
            };
            if let Err(error) = committed {
                if owns_transaction {
                    let _ = conn.execute_batch("ROLLBACK");
                } else {
                    let _ = conn.execute_batch(
                        "ROLLBACK TO SAVEPOINT coven_automation_occurrence_planning;
                         RELEASE SAVEPOINT coven_automation_occurrence_planning;",
                    );
                }
                return Err(error);
            }
            Ok(outcome)
        }
        Err(error) => {
            if owns_transaction {
                let _ = conn.execute_batch("ROLLBACK");
            } else {
                let _ = conn.execute_batch(
                    "ROLLBACK TO SAVEPOINT coven_automation_occurrence_planning;
                     RELEASE SAVEPOINT coven_automation_occurrence_planning;",
                );
            }
            Err(error)
        }
    }
}

fn plan_latest_due_occurrence_in_transaction(
    conn: &Connection,
    definition: &RoutineDefinition,
    created_at: DateTime<Utc>,
    now: DateTime<Utc>,
) -> Result<PlanOutcome, String> {
    if definition.status != RoutineStatus::Active {
        return Ok(PlanOutcome::NotDue);
    }
    let current = super::store::get_definition(conn, &definition.id)
        .map_err(|error| format!("failed to load routine `{}`: {error:#}", definition.id))?
        .ok_or_else(|| format!("routine `{}` vanished while planning", definition.id))?;
    let Some(current_digest) = current.definition_digest else {
        return Err(format!(
            "routine `{}` has unverifiable definition metadata",
            definition.id
        ));
    };
    let persisted_digest = super::contract::migration::definition_digest(&current.definition_json)
        .map_err(|error| {
            format!(
                "failed to digest stored routine `{}`: {error:#}",
                definition.id
            )
        })?;
    let persisted_definition: RoutineDefinition = serde_json::from_str(&current.definition_json)
        .map_err(|error| {
            format!(
                "stored routine `{}` is unreadable while planning: {error}",
                definition.id
            )
        })?;
    if current.status != "ACTIVE"
        || persisted_digest != current_digest
        || persisted_definition != *definition
    {
        return Err(format!(
            "routine `{}` definition changed while planning",
            definition.id
        ));
    }
    let revision_updated_at = DateTime::parse_from_rfc3339(&current.updated_at)
        .map(|timestamp| timestamp.with_timezone(&Utc))
        .map_err(|error| {
            format!(
                "routine `{}` has invalid revision updated_at: {error}",
                definition.id
            )
        })?;
    let revision_effective_at = created_at.max(revision_updated_at);
    let latest_scheduled =
        latest_scheduled_slot(conn, &definition.id, current.revision, &current_digest)?;
    let cursor = latest_scheduled.map_or(revision_effective_at, |latest| {
        revision_effective_at.max(latest)
    });
    let Some(slot) = latest_due_slot_after(definition, cursor, now)? else {
        return Ok(if latest_scheduled.is_some_and(|latest| latest <= now) {
            PlanOutcome::AlreadyFenced
        } else {
            PlanOutcome::NotDue
        });
    };

    let slot_iso = iso(slot);
    let now_iso = iso(now);
    let id = format!("{}-{}", definition.id, slot.timestamp_millis());
    let changed = conn
        .execute(
            "INSERT OR IGNORE INTO automation_occurrences
                (id, automation_id, automation_revision, definition_digest, scheduled_for, kind,
                 state, attempt, created_at, updated_at)
             SELECT ?1, ?2, definition.revision, definition.definition_digest, ?3,
                    'scheduled', 'planned', 0, ?4, ?4
             FROM automation_definitions AS definition
             WHERE definition.id = ?2
               AND definition.revision = ?5
               AND definition.definition_digest = ?6
               AND definition.status = 'ACTIVE'
               AND definition.tombstoned_at IS NULL
               AND definition.definition_digest IS NOT NULL",
            params![
                id,
                definition.id,
                slot_iso,
                now_iso,
                i64::try_from(current.revision)
                    .map_err(|_| "definition revision exceeds SQLite range")?,
                current_digest,
            ],
        )
        .map_err(|error| format!("failed to fence occurrence: {error}"))?;

    if changed == 0 {
        let already_fenced: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM automation_occurrences WHERE id = ?1)",
                [&id],
                |row| row.get(0),
            )
            .map_err(|error| format!("failed to inspect occurrence fence: {error}"))?;
        return if already_fenced {
            Ok(PlanOutcome::AlreadyFenced)
        } else {
            Err(format!(
                "routine `{}` definition changed while planning",
                definition.id
            ))
        };
    }

    Ok(PlanOutcome::Planned(PlannedOccurrence {
        id,
        automation_id: definition.id.clone(),
        scheduled_for: slot_iso,
        state: "planned".to_string(),
    }))
}

/// One planning tick across every stored routine. Idempotent: a repeated
/// tick fences nothing twice. Production ticks go through `tick`, which
/// adds lease recovery and claiming; this stays public for planning-only
/// callers and tests.
#[allow(dead_code)]
pub fn tick_planning(conn: &Connection, now: DateTime<Utc>) -> Result<PlanTickReport> {
    let mut report = PlanTickReport::default();
    let mut seen: BTreeSet<String> = BTreeSet::new();

    let (definitions, failures) = active_definitions(conn)?;
    report.failed.extend(failures);
    for definition in definitions {
        if !seen.insert(definition.id.clone()) {
            continue;
        }
        let created_at = definition_created_at(conn, &definition.id).unwrap_or(now);
        match plan_latest_due_occurrence(conn, &definition, created_at, now) {
            Ok(PlanOutcome::Planned(occurrence)) => report.planned.push(occurrence.id),
            Ok(PlanOutcome::AlreadyFenced) => report.already_fenced += 1,
            Ok(PlanOutcome::NotDue) => {}
            Err(error) => report.failed.push(format!("{}: {error}", definition.id)),
        }
    }

    // Count paused routines for observability.
    let paused: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM automation_definitions WHERE status = 'PAUSED'",
            [],
            |row| row.get(0),
        )
        .context("failed to count paused routines")?;
    report.paused_skipped = paused as usize;

    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::automations::definition::RoutineDefinition;
    use crate::automations::store::{insert_definition, update_definition};
    use crate::store::initialize_store;
    use chrono::{TimeZone, Timelike};
    use serde_json::json;

    fn temp_store() -> (tempfile::TempDir, Connection) {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("store.sqlite");
        initialize_store(&path).unwrap();
        let conn = crate::store::open_store(&path).unwrap();
        (temp, conn)
    }

    #[test]
    fn migration_classifies_existing_manual_occurrences() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE automation_occurrences (
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
            );",
        )
        .unwrap();
        let slot = Utc.with_ymd_and_hms(2026, 8, 30, 9, 0, 0).unwrap();
        let scheduled_id = format!("daily-{}", slot.timestamp_millis());
        conn.execute(
            "INSERT INTO automation_occurrences
                (id, automation_id, scheduled_for, state, attempt, created_at, updated_at)
             VALUES
                (?1, 'daily', '2026-08-30T09:00:00.000Z', 'succeeded', 1, ?2, ?2),
                ('occ-manual', 'daily', '2026-08-30T09:30:00.000Z', 'failed', 1, ?2, ?2)",
            rusqlite::params![scheduled_id, iso(slot)],
        )
        .unwrap();

        conn.execute_batch("BEGIN IMMEDIATE").unwrap();
        ensure_occurrence_kind(&conn).unwrap();
        conn.execute_batch("COMMIT").unwrap();

        let rows: Vec<(String, String, String)> = conn
            .prepare(
                "SELECT id, kind, scheduled_for
                 FROM automation_occurrences
                 ORDER BY id",
            )
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(rows[0].1, "scheduled");
        assert_eq!(rows[0].2, "2026-08-30T09:00:00.000Z");
        assert_eq!(rows[1].1, "manual");
        assert_eq!(rows[1].2, "2026-08-30T09:30:00.000000000Z");
    }

    #[test]
    fn initialize_store_migrates_the_previous_occurrence_schema() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("store.sqlite");
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE automation_occurrences (
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
            INSERT INTO automation_occurrences
                (id, automation_id, scheduled_for, state, attempt, created_at, updated_at)
            VALUES
                ('occ-manual', 'daily', '2026-08-30T09:30:00.000Z', 'failed', 1,
                 '2026-08-30T09:30:00.000Z', '2026-08-30T09:30:00.000Z');",
        )
        .unwrap();
        drop(conn);

        crate::store::initialize_store(&path).unwrap();

        let conn = crate::store::open_store(&path).unwrap();
        let migrated: (String, String) = conn
            .query_row(
                "SELECT kind, scheduled_for
                 FROM automation_occurrences
                 WHERE id = 'occ-manual'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(migrated.0, "manual");
        assert_eq!(migrated.1, "2026-08-30T09:30:00.000000000Z");
    }

    fn definition(id: &str, status: &str, rrule: &str) -> RoutineDefinition {
        RoutineDefinition::from_json(&json!({
            "schemaVersion": 1,
            "id": id,
            "name": id,
            "status": status,
            "rrule": rrule,
            "timezone": "utc",
            "misfire": "latest",
            "overlap": "forbid",
            "timeoutMinutes": 30,
            "runtime": "coven-code",
            "prompt": "Do the thing."
        }))
        .unwrap()
    }

    /// The definition row is stamped with the real current time by
    /// insert_definition, so every test ticks at the real now (or later) to
    /// stay past the routine's creation.
    fn real_now() -> DateTime<Utc> {
        Utc::now()
    }

    #[test]
    fn plans_the_latest_missed_daily_slot() {
        let (_temp, conn) = temp_store();
        insert_definition(&conn, &definition("daily", "ACTIVE", "FREQ=DAILY;BYHOUR=9")).unwrap();
        // Backdate creation one day so at least one 09:00 slot is missed at
        // any tick hour (today's 09:00 when now is past it, yesterday's
        // otherwise).
        let old_created = (real_now() - chrono::Duration::days(1))
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        conn.execute(
            "UPDATE automation_definitions
             SET created_at = ?1, updated_at = ?1
             WHERE id = 'daily'",
            rusqlite::params![old_created],
        )
        .unwrap();

        let report = tick_planning(&conn, real_now()).unwrap();
        assert_eq!(report.planned.len(), 1);
        assert_eq!(report.already_fenced, 0);

        // A second tick finds the same latest slot, but the unique fence keeps
        // it from being planned twice.
        let second = tick_planning(&conn, real_now()).unwrap();
        assert!(second.planned.is_empty());
        assert_eq!(second.already_fenced, 1);
    }

    #[test]
    fn new_occurrences_pin_the_current_definition_revision_and_digest() {
        let (_temp, conn) = temp_store();
        let initial = definition("pinned", "ACTIVE", "FREQ=DAILY;BYHOUR=9");
        insert_definition(&conn, &initial).unwrap();
        let mut routine = initial;
        routine.prompt = "Revision two.".to_string();
        let revised = update_definition(&conn, &routine).unwrap().unwrap();
        let expected_digest = revised.definition_digest.unwrap();
        let created_at = Utc.with_ymd_and_hms(2026, 8, 29, 8, 0, 0).unwrap();
        let now = Utc.with_ymd_and_hms(2026, 8, 30, 10, 0, 0).unwrap();
        conn.execute(
            "UPDATE automation_definitions SET updated_at = ?1 WHERE id = ?2",
            rusqlite::params![iso(created_at), routine.id],
        )
        .unwrap();

        let PlanOutcome::Planned(planned) =
            plan_latest_due_occurrence(&conn, &routine, created_at, now).unwrap()
        else {
            panic!("expected a planned occurrence");
        };
        assert!(insert_claimed_occurrence(
            &conn,
            "manual-pinned",
            "pinned",
            "operator",
            60,
            now + chrono::Duration::minutes(1),
        )
        .unwrap());

        for occurrence_id in [planned.id.as_str(), "manual-pinned"] {
            let pin: (i64, String) = conn
                .query_row(
                    "SELECT automation_revision, definition_digest
                     FROM automation_occurrences
                     WHERE id = ?1",
                    [occurrence_id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .unwrap();
            assert_eq!(pin, (2, expected_digest.clone()));
        }
    }

    #[test]
    fn planning_refuses_a_definition_that_changed_after_it_was_loaded() {
        let (_temp, conn) = temp_store();
        let stale = definition("planning-race", "ACTIVE", "FREQ=DAILY;BYHOUR=9");
        insert_definition(&conn, &stale).unwrap();
        let paused = definition("planning-race", "PAUSED", "FREQ=DAILY;BYHOUR=10");
        update_definition(&conn, &paused).unwrap().unwrap();
        let created_at = Utc.with_ymd_and_hms(2026, 8, 29, 8, 0, 0).unwrap();
        let now = Utc.with_ymd_and_hms(2026, 8, 30, 10, 0, 0).unwrap();

        let error = plan_latest_due_occurrence(&conn, &stale, created_at, now).unwrap_err();

        assert!(error.contains("definition changed while planning"));
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM automation_occurrences WHERE automation_id = 'planning-race'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn planning_accepts_legacy_optional_fields_that_normalize_on_serialize() {
        let (_temp, conn) = temp_store();
        let definition_json = r#"{"schemaVersion":1,"id":"legacy-normalized","name":"Legacy normalized","status":"ACTIVE","rrule":"FREQ=DAILY;BYHOUR=9","timezone":"utc","misfire":"latest","overlap":"forbid","timeoutMinutes":30,"runtime":"coven-code","familiarId":null,"cwd":"/tmp/project","outputTarget":null,"prompt":"Do the thing.","model":null,"tags":[]}"#;
        let digest =
            crate::automations::contract::migration::definition_digest(definition_json).unwrap();
        conn.execute(
            "INSERT INTO automation_definitions (
                id, name, status, definition_json, revision, definition_digest, lifecycle_state,
                tombstoned_at, authority_version, created_at, updated_at
             ) VALUES (
                'legacy-normalized', 'Legacy normalized', 'ACTIVE', ?1, 1, ?2, 'active',
                NULL, 0, '2026-08-29T08:00:00.000Z', '2026-08-29T08:00:00.000Z'
             )",
            rusqlite::params![definition_json, digest],
        )
        .unwrap();
        let definition: RoutineDefinition = serde_json::from_str(definition_json).unwrap();

        let outcome = plan_latest_due_occurrence(
            &conn,
            &definition,
            Utc.with_ymd_and_hms(2026, 8, 29, 8, 0, 0).unwrap(),
            Utc.with_ymd_and_hms(2026, 8, 30, 10, 0, 0).unwrap(),
        )
        .unwrap();

        assert!(matches!(outcome, PlanOutcome::Planned(_)));
    }

    #[test]
    fn collapses_three_missed_days_to_the_latest() {
        let (_temp, conn) = temp_store();
        insert_definition(&conn, &definition("daily", "ACTIVE", "FREQ=DAILY;BYHOUR=9")).unwrap();
        // Simulate a routine created four days ago.
        let old_created = (real_now() - chrono::Duration::days(4))
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        conn.execute(
            "UPDATE automation_definitions
             SET created_at = ?1, updated_at = ?1
             WHERE id = 'daily'",
            rusqlite::params![old_created],
        )
        .unwrap();

        let report = tick_planning(&conn, real_now()).unwrap();
        assert_eq!(
            report.planned.len(),
            1,
            "misfire latest plans exactly one slot"
        );

        let scheduled: String = conn
            .query_row(
                "SELECT scheduled_for FROM automation_occurrences WHERE automation_id = 'daily'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(scheduled.ends_with("T09:00:00.000Z"), "{scheduled}");
        // The fenced slot is within the last day-and-a-half, not a four-day
        // backfill of every missed morning.
        let scheduled_at = chrono::DateTime::parse_from_rfc3339(&scheduled)
            .unwrap()
            .with_timezone(&Utc);
        assert!(
            scheduled_at > real_now() - chrono::Duration::hours(36),
            "{scheduled}"
        );
    }

    #[test]
    fn long_downtime_still_collapses_to_the_latest_slot() {
        let (_temp, conn) = temp_store();
        let routine = definition(
            "hourly-slots",
            "ACTIVE",
            "FREQ=DAILY;BYHOUR=0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23",
        );
        insert_definition(&conn, &routine).unwrap();
        let created_at = Utc.with_ymd_and_hms(2026, 8, 20, 0, 0, 0).unwrap();
        let now = Utc.with_ymd_and_hms(2026, 8, 30, 23, 30, 0).unwrap();
        conn.execute(
            "UPDATE automation_definitions
             SET created_at = ?1, updated_at = ?1
             WHERE id = ?2",
            rusqlite::params![iso(created_at), routine.id],
        )
        .unwrap();
        conn.execute(
            "UPDATE automation_definitions
             SET created_at = ?1, updated_at = ?1
             WHERE id = ?2",
            rusqlite::params![iso(created_at), routine.id],
        )
        .unwrap();
        conn.execute(
            "UPDATE automation_definitions SET created_at = ?1, updated_at = ?1 WHERE id = ?2",
            rusqlite::params![iso(created_at), routine.id],
        )
        .unwrap();

        let outcome = plan_latest_due_occurrence(&conn, &routine, created_at, now).unwrap();
        let PlanOutcome::Planned(occurrence) = outcome else {
            panic!("expected the latest missed occurrence, got {outcome:?}");
        };
        assert_eq!(occurrence.scheduled_for, "2026-08-30T23:00:00.000Z");
    }

    #[test]
    fn clock_forward_collapses_to_latest_and_backward_replan_adds_no_older_fence() {
        let (_temp, conn) = temp_store();
        let routine = definition("clock-jump", "ACTIVE", "FREQ=DAILY;BYHOUR=9");
        insert_definition(&conn, &routine).unwrap();
        let created_at = Utc.with_ymd_and_hms(2026, 1, 1, 8, 0, 0).unwrap();
        conn.execute(
            "UPDATE automation_definitions SET created_at = ?1, updated_at = ?1 WHERE id = ?2",
            rusqlite::params![iso(created_at), routine.id],
        )
        .unwrap();

        let first_now = Utc.with_ymd_and_hms(2026, 1, 1, 10, 0, 0).unwrap();
        assert!(matches!(
            plan_latest_due_occurrence(&conn, &routine, created_at, first_now).unwrap(),
            PlanOutcome::Planned(_)
        ));

        let forward_now = Utc.with_ymd_and_hms(2026, 1, 5, 12, 0, 0).unwrap();
        let PlanOutcome::Planned(forward) =
            plan_latest_due_occurrence(&conn, &routine, created_at, forward_now).unwrap()
        else {
            panic!("forward jump should plan the latest due slot");
        };
        assert_eq!(forward.scheduled_for, "2026-01-05T09:00:00.000Z");

        let backward_now = Utc.with_ymd_and_hms(2026, 1, 3, 12, 0, 0).unwrap();
        assert_eq!(
            plan_latest_due_occurrence(&conn, &routine, created_at, backward_now).unwrap(),
            PlanOutcome::NotDue
        );

        let slots: Vec<String> = conn
            .prepare(
                "SELECT scheduled_for FROM automation_occurrences
                 WHERE automation_id = ?1 ORDER BY scheduled_for",
            )
            .unwrap()
            .query_map([&routine.id], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(
            slots,
            vec!["2026-01-01T09:00:00.000Z", "2026-01-05T09:00:00.000Z"]
        );
    }

    #[test]
    fn revised_schedule_plans_only_slots_after_the_revision_effective_time() {
        let (_temp, conn) = temp_store();
        let initial = definition("future-only", "ACTIVE", "FREQ=DAILY;BYHOUR=9");
        insert_definition(&conn, &initial).unwrap();
        let created_at = Utc.with_ymd_and_hms(2026, 8, 30, 8, 0, 0).unwrap();
        conn.execute(
            "UPDATE automation_definitions
             SET created_at = ?1, updated_at = ?1
             WHERE id = ?2",
            rusqlite::params![iso(created_at), initial.id],
        )
        .unwrap();
        let first_now = Utc.with_ymd_and_hms(2026, 8, 30, 10, 0, 0).unwrap();
        let PlanOutcome::Planned(first) =
            plan_latest_due_occurrence(&conn, &initial, created_at, first_now).unwrap()
        else {
            panic!("expected the original revision's 09:00 occurrence");
        };
        assert_eq!(first.scheduled_for, "2026-08-30T09:00:00.000Z");

        let revised = definition("future-only", "ACTIVE", "FREQ=DAILY;BYHOUR=11");
        update_definition(&conn, &revised).unwrap().unwrap();
        let revised_at = Utc.with_ymd_and_hms(2026, 8, 30, 12, 0, 0).unwrap();
        conn.execute(
            "UPDATE automation_definitions SET updated_at = ?1 WHERE id = ?2",
            rusqlite::params![iso(revised_at), revised.id],
        )
        .unwrap();

        let same_day = Utc.with_ymd_and_hms(2026, 8, 30, 12, 30, 0).unwrap();
        assert_eq!(
            plan_latest_due_occurrence(&conn, &revised, created_at, same_day).unwrap(),
            PlanOutcome::NotDue
        );

        let older_revision_slot = Utc.with_ymd_and_hms(2026, 8, 31, 11, 0, 0).unwrap();
        conn.execute(
            "INSERT INTO automation_occurrences (
                id, automation_id, automation_revision, scheduled_for, kind,
                state, attempt, created_at, updated_at
             ) VALUES (?1, ?2, 1, ?3, 'scheduled', 'succeeded', 1, ?3, ?3)",
            rusqlite::params![
                format!("{}-{}", revised.id, older_revision_slot.timestamp_millis()),
                revised.id,
                iso(older_revision_slot),
            ],
        )
        .unwrap();
        let duplicate_time = Utc.with_ymd_and_hms(2026, 8, 31, 11, 30, 0).unwrap();
        assert_eq!(
            plan_latest_due_occurrence(&conn, &revised, created_at, duplicate_time).unwrap(),
            PlanOutcome::AlreadyFenced
        );

        let next_day = Utc.with_ymd_and_hms(2026, 9, 1, 11, 30, 0).unwrap();
        let PlanOutcome::Planned(next) =
            plan_latest_due_occurrence(&conn, &revised, created_at, next_day).unwrap()
        else {
            panic!("expected the first post-revision 11:00 occurrence");
        };
        assert_eq!(next.scheduled_for, "2026-09-01T11:00:00.000Z");

        let pins: Vec<(String, i64)> = conn
            .prepare(
                "SELECT scheduled_for, automation_revision
                 FROM automation_occurrences
                 WHERE automation_id = ?1
                 ORDER BY scheduled_for",
            )
            .unwrap()
            .query_map([&revised.id], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(
            pins,
            vec![
                ("2026-08-30T09:00:00.000Z".to_string(), 1),
                ("2026-08-31T11:00:00.000Z".to_string(), 1),
                ("2026-09-01T11:00:00.000Z".to_string(), 2),
            ]
        );
    }

    #[test]
    fn migrated_revision_one_uses_updated_at_as_its_effective_time() {
        let (_temp, conn) = temp_store();
        let routine = definition("migrated-revision-one", "ACTIVE", "FREQ=DAILY;BYHOUR=11");
        insert_definition(&conn, &routine).unwrap();
        let created_at = Utc.with_ymd_and_hms(2026, 8, 1, 8, 0, 0).unwrap();
        let updated_at = Utc.with_ymd_and_hms(2026, 8, 30, 12, 0, 0).unwrap();
        conn.execute(
            "UPDATE automation_definitions
             SET created_at = ?1, updated_at = ?2
             WHERE id = ?3",
            rusqlite::params![iso(created_at), iso(updated_at), routine.id],
        )
        .unwrap();

        let now = Utc.with_ymd_and_hms(2026, 8, 30, 12, 30, 0).unwrap();

        assert_eq!(
            plan_latest_due_occurrence(&conn, &routine, created_at, now).unwrap(),
            PlanOutcome::NotDue
        );
    }

    #[test]
    fn unverifiable_reused_id_history_does_not_advance_the_current_revision_cursor() {
        let (_temp, conn) = temp_store();
        let routine = definition("reused-id", "ACTIVE", "FREQ=DAILY;BYHOUR=13");
        let record = insert_definition(&conn, &routine).unwrap();
        let effective_at = Utc.with_ymd_and_hms(2026, 8, 30, 12, 0, 0).unwrap();
        conn.execute(
            "UPDATE automation_definitions
             SET created_at = ?1, updated_at = ?1
             WHERE id = ?2",
            rusqlite::params![iso(effective_at), routine.id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO automation_occurrences (
                id, automation_id, automation_revision, definition_digest,
                scheduled_for, kind, state, attempt, created_at, updated_at
             ) VALUES (
                'old-unverifiable', ?1, 1, NULL,
                '2026-09-05T09:00:00.000Z', 'scheduled', 'succeeded', 1,
                '2026-08-01T09:00:00.000Z', '2026-08-01T09:05:00.000Z'
             )",
            [&routine.id],
        )
        .unwrap();

        let now = Utc.with_ymd_and_hms(2026, 8, 30, 13, 30, 0).unwrap();
        let PlanOutcome::Planned(planned) =
            plan_latest_due_occurrence(&conn, &routine, effective_at, now).unwrap()
        else {
            panic!("unverifiable reused-id history delayed the current definition");
        };

        assert_eq!(planned.scheduled_for, "2026-08-30T13:00:00.000Z");
        let current_pin: (i64, String) = conn
            .query_row(
                "SELECT automation_revision, definition_digest
                 FROM automation_occurrences
                 WHERE id = ?1",
                [&planned.id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            current_pin,
            (
                1,
                record
                    .definition_digest
                    .expect("inserted definition digest")
            )
        );
    }

    #[test]
    fn latest_misfire_supersedes_an_older_planned_fence() {
        let (_temp, conn) = temp_store();
        let routine = definition(
            "hourly-slots",
            "ACTIVE",
            "FREQ=DAILY;BYHOUR=0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23",
        );
        insert_definition(&conn, &routine).unwrap();
        let created_at = Utc.with_ymd_and_hms(2026, 8, 20, 0, 0, 0).unwrap();
        let now = Utc.with_ymd_and_hms(2026, 8, 30, 23, 30, 0).unwrap();
        conn.execute(
            "UPDATE automation_definitions
             SET created_at = ?1, updated_at = ?1
             WHERE id = ?2",
            rusqlite::params![iso(created_at), routine.id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO automation_occurrences
                (id, automation_id, scheduled_for, state, attempt, created_at, updated_at)
             VALUES ('stale', ?1, '2026-08-20T01:00:00.000Z', 'planned', 0, ?2, ?2)",
            rusqlite::params![routine.id, iso(created_at)],
        )
        .unwrap();

        let outcome = plan_latest_due_occurrence(&conn, &routine, created_at, now).unwrap();
        let PlanOutcome::Planned(latest) = outcome else {
            panic!("expected latest occurrence, got {outcome:?}");
        };
        let claimed = claim_due_occurrence(&conn, &routine.id, "daemon", 60, now)
            .unwrap()
            .expect("latest occurrence should be claimable");

        assert_eq!(claimed, latest.id);
        let stale_state: String = conn
            .query_row(
                "SELECT state FROM automation_occurrences WHERE id = 'stale'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(stale_state, "skipped");
        let stale_reason: String = conn
            .query_row(
                "SELECT failure_reason FROM automation_occurrences WHERE id = 'stale'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(stale_reason, "superseded by latest misfire policy");
    }

    #[test]
    fn stored_output_target_definition_is_not_planned() {
        let (_temp, conn) = temp_store();
        let mut routine = definition("legacy-output", "ACTIVE", "FREQ=DAILY;BYHOUR=9");
        routine.output_target = Some("result.md".to_string());
        insert_definition(&conn, &routine).unwrap();

        let report = tick_planning(&conn, real_now()).unwrap();

        assert!(report.planned.is_empty());
        assert_eq!(report.failed.len(), 1);
        assert!(report.failed[0].contains("outputTarget is not supported"));
    }

    #[test]
    fn invalid_stored_definition_does_not_block_valid_planning() {
        let (_temp, conn) = temp_store();
        let valid = definition("valid", "ACTIVE", "FREQ=DAILY;BYHOUR=9");
        let mut invalid = definition("invalid", "ACTIVE", "FREQ=DAILY;BYHOUR=9");
        invalid.output_target = Some("result.md".to_string());
        insert_definition(&conn, &valid).unwrap();
        insert_definition(&conn, &invalid).unwrap();
        let created_at = Utc.with_ymd_and_hms(2026, 8, 28, 8, 0, 0).unwrap();
        conn.execute(
            "UPDATE automation_definitions SET created_at = ?1, updated_at = ?1",
            rusqlite::params![iso(created_at)],
        )
        .unwrap();

        let report =
            tick_planning(&conn, Utc.with_ymd_and_hms(2026, 8, 28, 10, 0, 0).unwrap()).unwrap();

        assert_eq!(report.planned.len(), 1);
        assert!(report.planned[0].starts_with("valid-"));
        assert_eq!(report.failed.len(), 1);
        assert!(report.failed[0].contains("stored routine `invalid` is invalid"));
        assert!(report.failed[0].contains("outputTarget is not supported"));
    }

    #[test]
    fn future_slots_are_not_fenced() {
        let (_temp, conn) = temp_store();
        let now = real_now();
        // A slot strictly later today (wrapping into tomorrow when late in
        // the evening) is in the future for this routine.
        let future_hour = (now.hour() + 2) % 24;
        let rrule = format!("FREQ=DAILY;BYHOUR={future_hour}");
        insert_definition(&conn, &definition("future", "ACTIVE", &rrule)).unwrap();

        let report = tick_planning(&conn, now).unwrap();
        assert!(
            report.planned.is_empty(),
            "future slots must not be fenced: {report:?}"
        );
        assert_eq!(report.already_fenced, 0);
    }

    #[test]
    fn claims_the_earliest_due_occurrence_with_a_lease() {
        let (_temp, conn) = temp_store();
        insert_definition(&conn, &definition("daily", "ACTIVE", "FREQ=DAILY;BYHOUR=9")).unwrap();
        let old_created = (real_now() - chrono::Duration::days(1))
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        conn.execute(
            "UPDATE automation_definitions
             SET created_at = ?1, updated_at = ?1
             WHERE id = 'daily'",
            rusqlite::params![old_created],
        )
        .unwrap();
        tick_planning(&conn, real_now()).unwrap();

        let claimed = claim_due_occurrence(&conn, "daily", "daemon-a", 60, real_now()).unwrap();
        assert!(claimed.is_some());

        // A second claimant finds nothing left to claim.
        let second = claim_due_occurrence(&conn, "daily", "daemon-b", 60, real_now()).unwrap();
        assert!(second.is_none());
    }

    #[test]
    fn terminal_manual_occurrences_do_not_supersede_a_scheduled_plan() {
        for manual_state in ["failed", "succeeded"] {
            let (_temp, conn) = temp_store();
            let now = Utc.with_ymd_and_hms(2026, 8, 30, 11, 0, 0).unwrap();
            conn.execute(
                "INSERT INTO automation_occurrences
                    (id, automation_id, scheduled_for, state, attempt, created_at, updated_at)
                 VALUES
                    ('scheduled', 'daily', '2026-08-30T09:00:00.000Z', 'planned', 0, ?1, ?1),
                    ('manual', 'daily', '2026-08-30T10:00:00.000Z', ?2, 1, ?1, ?1)",
                rusqlite::params![iso(now), manual_state],
            )
            .unwrap();

            assert_eq!(
                claim_due_occurrence(&conn, "daily", "daemon", 60, now).unwrap(),
                Some("scheduled".to_string()),
                "manual state {manual_state} suppressed scheduled work"
            );
        }
    }

    #[test]
    fn claimed_manual_occurrence_does_not_supersede_a_scheduled_plan() {
        let (_temp, conn) = temp_store();
        let now = Utc.with_ymd_and_hms(2026, 8, 30, 11, 0, 0).unwrap();
        conn.execute(
            "INSERT INTO automation_occurrences
                (id, automation_id, scheduled_for, state, lease_owner, lease_expires_at,
                 attempt, created_at, updated_at)
             VALUES
                ('scheduled', 'daily', '2026-08-30T09:00:00.000Z', 'planned', NULL, NULL, 0, ?1, ?1),
                ('manual', 'daily', '2026-08-30T10:00:00.000Z', 'claimed', 'manual', ?2, 1, ?1, ?1)",
            rusqlite::params![iso(now), iso(now + chrono::Duration::minutes(60))],
        )
        .unwrap();

        assert_eq!(
            claim_due_occurrence(&conn, "daily", "daemon", 60, now).unwrap(),
            None
        );
        let scheduled_state: String = conn
            .query_row(
                "SELECT state FROM automation_occurrences WHERE id = 'scheduled'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(scheduled_state, "planned");
    }

    #[test]
    fn manual_occurrence_does_not_advance_the_schedule_cursor() {
        for manual_state in ["claimed", "failed", "succeeded"] {
            let (_temp, conn) = temp_store();
            let routine = definition("daily", "ACTIVE", "FREQ=DAILY;BYHOUR=9");
            insert_definition(&conn, &routine).unwrap();
            let created_at = Utc.with_ymd_and_hms(2026, 8, 29, 8, 0, 0).unwrap();
            let now = Utc.with_ymd_and_hms(2026, 8, 30, 9, 59, 0).unwrap();
            conn.execute(
                "UPDATE automation_definitions
                 SET created_at = ?1, updated_at = ?1
                 WHERE id = ?2",
                rusqlite::params![iso(created_at), routine.id],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO automation_occurrences
                    (id, automation_id, scheduled_for, kind, state, lease_owner, lease_expires_at,
                     attempt, created_at, updated_at)
                 VALUES ('manual', ?1, '2026-08-30T09:30:00.000Z', 'manual', ?2, 'manual', ?3, 1, ?3, ?3)",
                rusqlite::params![
                    routine.id,
                    manual_state,
                    iso(now + chrono::Duration::minutes(60))
                ],
            )
            .unwrap();

            let outcome = plan_latest_due_occurrence(&conn, &routine, created_at, now).unwrap();

            let PlanOutcome::Planned(occurrence) = outcome else {
                panic!("manual state {manual_state} advanced schedule cursor: {outcome:?}");
            };
            assert_eq!(occurrence.scheduled_for, "2026-08-30T09:00:00.000Z");
        }
    }

    #[test]
    fn manual_run_at_exact_slot_does_not_consume_the_schedule_fence() {
        let (_temp, conn) = temp_store();
        let routine = definition("daily", "ACTIVE", "FREQ=DAILY;BYHOUR=9");
        insert_definition(&conn, &routine).unwrap();
        let created_at = Utc.with_ymd_and_hms(2026, 8, 29, 8, 0, 0).unwrap();
        let slot = Utc.with_ymd_and_hms(2026, 8, 30, 9, 0, 0).unwrap();
        conn.execute(
            "UPDATE automation_definitions
             SET created_at = ?1, updated_at = ?1
             WHERE id = ?2",
            rusqlite::params![iso(created_at), routine.id],
        )
        .unwrap();
        assert!(insert_claimed_occurrence(
            &conn,
            "manual-exact-slot",
            &routine.id,
            "manual",
            60,
            slot,
        )
        .unwrap());
        assert!(settle_occurrence(
            &conn,
            "manual-exact-slot",
            "failed",
            Some("synthetic manual failure"),
            slot + chrono::Duration::minutes(1),
        )
        .unwrap());

        let outcome = plan_latest_due_occurrence(
            &conn,
            &routine,
            created_at,
            slot + chrono::Duration::minutes(2),
        )
        .unwrap();

        let PlanOutcome::Planned(occurrence) = outcome else {
            panic!("manual occurrence consumed schedule fence: {outcome:?}");
        };
        assert_eq!(occurrence.scheduled_for, "2026-08-30T09:00:00.000Z");
    }

    #[test]
    fn overlap_forbid_does_not_claim_while_an_occurrence_is_running() {
        let (_temp, conn) = temp_store();
        conn.execute(
            "INSERT INTO automation_occurrences
                (id, automation_id, scheduled_for, state, attempt, created_at, updated_at)
             VALUES
                ('running', 'daily', '2020-01-01T00:00:00.000Z', 'running', 1,
                 '2020-01-01T00:00:00.000Z', '2020-01-01T00:00:00.000Z'),
                ('next', 'daily', '2020-01-01T00:01:00.000Z', 'planned', 0,
                 '2020-01-01T00:01:00.000Z', '2020-01-01T00:01:00.000Z')",
            [],
        )
        .unwrap();

        let claimed = claim_due_occurrence(&conn, "daily", "daemon", 60, real_now()).unwrap();
        assert!(claimed.is_none());
        let next_state: String = conn
            .query_row(
                "SELECT state FROM automation_occurrences WHERE id = 'next'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(next_state, "planned");
    }

    #[test]
    fn recovers_expired_leases_to_failed() {
        let (_temp, conn) = temp_store();
        insert_definition(&conn, &definition("daily", "ACTIVE", "FREQ=DAILY;BYHOUR=9")).unwrap();
        let old_created = (real_now() - chrono::Duration::days(1))
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        conn.execute(
            "UPDATE automation_definitions
             SET created_at = ?1, updated_at = ?1
             WHERE id = 'daily'",
            rusqlite::params![old_created],
        )
        .unwrap();
        tick_planning(&conn, real_now()).unwrap();
        claim_due_occurrence(&conn, "daily", "daemon-a", 60, real_now()).unwrap();

        // Expire the lease by hand, then tick: recovery marks it failed.
        conn.execute(
            "UPDATE automation_occurrences SET lease_expires_at = '2020-01-01T00:00:00.000Z'",
            [],
        )
        .unwrap();
        let report = tick(&conn, real_now()).unwrap();
        assert_eq!(report.recovered, 1);

        let state: String = conn
            .query_row(
                "SELECT state FROM automation_occurrences WHERE automation_id = 'daily'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(state, "failed");
    }

    #[test]
    fn lease_is_expired_at_its_exact_deadline() {
        let (_temp, conn) = temp_store();
        insert_definition(&conn, &definition("daily", "ACTIVE", "FREQ=DAILY;BYHOUR=9")).unwrap();
        let now = Utc.with_ymd_and_hms(2026, 8, 28, 10, 0, 0).unwrap();
        assert!(insert_claimed_occurrence(
            &conn,
            "deadline",
            "daily",
            "daemon",
            60,
            now - chrono::Duration::minutes(60),
        )
        .unwrap());

        assert_eq!(recover_expired_leases(&conn, now).unwrap(), 1);
        let state: String = conn
            .query_row(
                "SELECT state FROM automation_occurrences WHERE id = 'deadline'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(state, "failed");
    }

    #[test]
    fn does_not_fail_running_work_when_its_claim_lease_expires() {
        let (_temp, conn) = temp_store();
        conn.execute(
            "INSERT INTO automation_occurrences
                (id, automation_id, scheduled_for, state, lease_owner, lease_expires_at,
                 attempt, created_at, updated_at)
             VALUES ('occ-running', 'daily', '2020-01-01T00:00:00.000Z', 'running',
                     'daemon-a', '2020-01-01T01:00:00.000Z', 1,
                     '2020-01-01T00:00:00.000Z', '2020-01-01T00:00:00.000Z')",
            [],
        )
        .unwrap();

        assert_eq!(recover_expired_leases(&conn, real_now()).unwrap(), 0);
        let state: String = conn
            .query_row(
                "SELECT state FROM automation_occurrences WHERE id = 'occ-running'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(state, "running");
    }

    #[test]
    fn settles_claimed_work_but_never_planned() {
        let (_temp, conn) = temp_store();
        insert_definition(&conn, &definition("daily", "ACTIVE", "FREQ=DAILY;BYHOUR=9")).unwrap();
        let old_created = (real_now() - chrono::Duration::days(1))
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        conn.execute(
            "UPDATE automation_definitions
             SET created_at = ?1, updated_at = ?1
             WHERE id = 'daily'",
            rusqlite::params![old_created],
        )
        .unwrap();
        tick_planning(&conn, real_now()).unwrap();

        let planned: String = conn
            .query_row(
                "SELECT id FROM automation_occurrences WHERE automation_id = 'daily'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(!settle_occurrence(&conn, &planned, "succeeded", None, real_now()).unwrap());

        claim_due_occurrence(&conn, "daily", "daemon-a", 60, real_now()).unwrap();
        assert!(settle_occurrence(&conn, &planned, "succeeded", None, real_now()).unwrap());
    }

    #[test]
    fn paused_routines_never_plan() {
        let (_temp, conn) = temp_store();
        insert_definition(
            &conn,
            &definition("paused", "PAUSED", "FREQ=DAILY;BYHOUR=9"),
        )
        .unwrap();

        let report = tick_planning(&conn, real_now()).unwrap();
        assert!(report.planned.is_empty());
        assert_eq!(report.paused_skipped, 1);
    }
}
