//! Occurrence planning with misfire-latest semantics (coven#816).
//!
//! The planner computes each ACTIVE routine's true latest due slot directly:
//! if the daemon was down
//! for three days of a daily routine, only the most recent missed slot is
//! fenced — earlier slots are collapsed, never backfilled. The
//! `UNIQUE(automation_id, scheduled_for)` fence makes planning idempotent
//! across ticks, replicas, and restarts.

use std::collections::BTreeSet;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};

use super::definition::{RoutineDefinition, RoutineStatus};
use crate::automations::schedule::latest_due;

const OCCURRENCE_ID_NAMESPACE: uuid::Uuid =
    uuid::Uuid::from_u128(0x6f80_c451_5b65_4b02_a67c_0a4e_1279_eef1);

pub const AUTOMATION_OCCURRENCES_SCHEMA_SQL: &str = "
    CREATE TABLE IF NOT EXISTS automation_occurrences (
        id TEXT PRIMARY KEY NOT NULL,
        automation_id TEXT NOT NULL,
        scheduled_for TEXT NOT NULL,
        state TEXT NOT NULL DEFAULT 'planned',
        lease_owner TEXT,
        lease_expires_at TEXT,
        attempt INTEGER NOT NULL DEFAULT 0,
        failure_reason TEXT,
        definition_revision INTEGER,
        definition_digest TEXT,
        definition_json TEXT,
        output_target TEXT,
        deadline_at TEXT,
        delivery_state TEXT NOT NULL DEFAULT 'none',
        delivery_token TEXT,
        delivery_digest TEXT,
        delivery_error TEXT,
        created_at TEXT NOT NULL,
        updated_at TEXT NOT NULL,
        UNIQUE(automation_id, scheduled_for)
    );

    CREATE INDEX IF NOT EXISTS idx_automation_occurrences_scheduled
        ON automation_occurrences(automation_id, scheduled_for);

    CREATE INDEX IF NOT EXISTS idx_automation_occurrences_state
        ON automation_occurrences(state, lease_expires_at);
";

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
#[cfg(test)]
const MAX_LEASE_MINUTES: i64 = super::definition::AUTOMATION_TIMEOUT_MAX_MINUTES as i64;

struct ActiveDefinition {
    definition: RoutineDefinition,
    revision: i64,
    digest: String,
}

/// Claims the earliest due PLANNED occurrence for a routine with a bounded
/// lease. Returns the claimed occurrence id, or `None` when nothing is due
/// — including when the routine already has a live claimed/running
/// occurrence, which `overlap: forbid` keeps from racing a second run. The
/// compare-and-set WHERE clause makes claims race-safe across callers.
///
/// The daemon tick claims through this path; manual run-now claims a specific
/// fresh occurrence via `claim_occurrence_by_id`.
#[cfg(test)]
pub fn claim_due_occurrence(
    conn: &Connection,
    automation_id: &str,
    owner: &str,
    lease_minutes: i64,
    now: DateTime<Utc>,
) -> Result<Option<String>, String> {
    if lease_minutes <= 0 || lease_minutes > MAX_LEASE_MINUTES {
        return Err("lease minutes must be 1..=44640".to_string());
    }
    let record = super::store::get_definition(conn, automation_id)
        .map_err(|error| format!("failed to read routine before claim: {error:#}"))?
        .ok_or_else(|| format!("routine `{automation_id}` vanished before claim"))?;
    claim_due_occurrence_at_revision(
        conn,
        automation_id,
        owner,
        record.revision,
        &record.definition_digest,
        now,
    )
}

pub fn claim_due_occurrence_at_revision(
    conn: &Connection,
    automation_id: &str,
    owner: &str,
    expected_revision: i64,
    expected_digest: &str,
    now: DateTime<Utc>,
) -> Result<Option<String>, String> {
    let tx = rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Immediate)
        .map_err(|error| format!("failed to begin occurrence claim: {error}"))?;
    let snapshot = snapshot_for_claim(&tx, automation_id, expected_revision, expected_digest)?;
    let expires_iso = iso(now + chrono::Duration::minutes(snapshot.timeout_minutes));
    let now_iso = iso(now);
    let changed = tx
        .execute(
            "UPDATE automation_occurrences
             SET state = 'claimed',
                 lease_owner = ?3,
                 lease_expires_at = ?4,
                 attempt = attempt + 1,
                 definition_revision = ?5,
                 definition_digest = ?6,
                 definition_json = ?7,
                 output_target = ?8,
                 deadline_at = ?4,
                 updated_at = ?2
             WHERE automation_id = ?1
               AND state = 'planned'
               AND scheduled_for <= ?2
               AND id = (
                   SELECT id FROM automation_occurrences
                   WHERE automation_id = ?1
                     AND state = 'planned'
                     AND scheduled_for <= ?2
                   ORDER BY scheduled_for ASC
                   LIMIT 1
               )
               AND NOT EXISTS (
                   SELECT 1 FROM automation_occurrences AS live
                   WHERE live.automation_id = ?1
                     AND live.state IN ('claimed', 'running')
               )",
            params![
                automation_id,
                now_iso,
                owner,
                expires_iso,
                snapshot.revision,
                snapshot.digest,
                snapshot.definition_json,
                snapshot.output_target
            ],
        )
        .map_err(|error| format!("failed to claim occurrence: {error}"))?;
    if changed == 0 {
        tx.commit()
            .map_err(|error| format!("failed to commit empty occurrence claim: {error}"))?;
        return Ok(None);
    }
    let id: String = tx
        .query_row(
            "SELECT id FROM automation_occurrences WHERE automation_id = ?1 AND state = 'claimed' AND lease_owner = ?2 ORDER BY scheduled_for ASC LIMIT 1",
            params![automation_id, owner],
            |row| row.get(0),
        )
        .map_err(|error| format!("failed to read claim: {error}"))?;
    tx.commit()
        .map_err(|error| format!("failed to commit occurrence claim: {error}"))?;
    Ok(Some(id))
}

/// Claims one specific occurrence (the manual run-now fence) with a bounded
/// lease. Refused — returning `Ok(None)` — when the occurrence is not
/// claimable, including when a sibling occurrence of the same routine is
/// still live (`overlap: forbid`).
#[cfg(test)]
pub fn claim_occurrence_by_id(
    conn: &Connection,
    occurrence_id: &str,
    owner: &str,
    lease_minutes: i64,
    now: DateTime<Utc>,
) -> Result<Option<String>, String> {
    if lease_minutes <= 0 || lease_minutes > MAX_LEASE_MINUTES {
        return Err("lease minutes must be 1..=44640".to_string());
    }
    let automation_id: String = conn
        .query_row(
            "SELECT automation_id FROM automation_occurrences WHERE id = ?1",
            params![occurrence_id],
            |row| row.get(0),
        )
        .map_err(|error| format!("failed to read occurrence routine: {error}"))?;
    let record = super::store::get_definition(conn, automation_id.as_str())
        .map_err(|error| format!("failed to read routine before claim: {error:#}"))?
        .ok_or_else(|| format!("routine `{automation_id}` vanished before claim"))?;
    claim_occurrence_by_id_at_revision(
        conn,
        occurrence_id,
        owner,
        record.revision,
        &record.definition_digest,
        now,
    )
}

pub fn claim_occurrence_by_id_at_revision(
    conn: &Connection,
    occurrence_id: &str,
    owner: &str,
    expected_revision: i64,
    expected_digest: &str,
    now: DateTime<Utc>,
) -> Result<Option<String>, String> {
    let tx = rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Immediate)
        .map_err(|error| format!("failed to begin occurrence claim: {error}"))?;
    let automation_id: String = tx
        .query_row(
            "SELECT automation_id FROM automation_occurrences WHERE id = ?1",
            params![occurrence_id],
            |row| row.get(0),
        )
        .map_err(|error| format!("failed to read occurrence routine: {error}"))?;
    let snapshot = snapshot_for_claim(&tx, &automation_id, expected_revision, expected_digest)?;
    let expires_iso = iso(now + chrono::Duration::minutes(snapshot.timeout_minutes));
    let now_iso = iso(now);
    let changed = tx
        .execute(
            "UPDATE automation_occurrences
             SET state = 'claimed',
                 lease_owner = ?3,
                 lease_expires_at = ?4,
                 attempt = attempt + 1,
                 definition_revision = ?5,
                 definition_digest = ?6,
                 definition_json = ?7,
                 output_target = ?8,
                 deadline_at = ?4,
                 updated_at = ?2
             WHERE id = ?1
               AND state = 'planned'
               AND scheduled_for <= ?2
               AND NOT EXISTS (
                   SELECT 1 FROM automation_occurrences AS live
                   WHERE live.automation_id = (
                       SELECT automation_id FROM automation_occurrences WHERE id = ?1
                   )
                     AND live.state IN ('claimed', 'running')
                     AND live.id != ?1
             )",
            params![
                occurrence_id,
                now_iso,
                owner,
                expires_iso,
                snapshot.revision,
                snapshot.digest,
                snapshot.definition_json,
                snapshot.output_target
            ],
        )
        .map_err(|error| format!("failed to claim occurrence: {error}"))?;
    if changed == 0 {
        tx.commit()
            .map_err(|error| format!("failed to commit empty occurrence claim: {error}"))?;
        return Ok(None);
    }
    tx.commit()
        .map_err(|error| format!("failed to commit occurrence claim: {error}"))?;
    Ok(Some(occurrence_id.to_string()))
}

fn snapshot_for_claim(
    conn: &Connection,
    automation_id: &str,
    expected_revision: i64,
    expected_digest: &str,
) -> Result<super::store::DefinitionSnapshot, String> {
    let record = super::store::get_definition(conn, automation_id)
        .map_err(|error| format!("failed to read routine snapshot: {error:#}"))?
        .ok_or_else(|| format!("routine `{automation_id}` vanished before claim"))?;
    if record.revision != expected_revision || record.definition_digest != expected_digest {
        return Err(format!(
            "routine `{automation_id}` changed before claim; retry with revision {}",
            record.revision
        ));
    }
    let snapshot = super::store::definition_snapshot(&record)
        .map_err(|error| format!("failed to snapshot routine: {error:#}"))?;
    Ok(snapshot)
}

/// Compare-and-sets a claimed occurrence to `running` with a fresh bounded
/// lease that outlives a healthy run of the routine. Linked running rows are
/// reconciled against their pinned deadline; only unlinked stale rows use
/// lease recovery.
pub fn mark_occurrence_running(
    conn: &Connection,
    occurrence_id: &str,
    owner: &str,
    deadline_at: &str,
    now: DateTime<Utc>,
) -> Result<bool, String> {
    let now_iso = iso(now);
    chrono::DateTime::parse_from_rfc3339(deadline_at)
        .map_err(|error| format!("occurrence deadline is invalid: {error}"))?;
    let changed = conn
        .execute(
            "UPDATE automation_occurrences
             SET state = 'running',
                 lease_owner = ?3,
                 lease_expires_at = ?4,
                 updated_at = ?2
             WHERE id = ?1 AND state = 'claimed'",
            params![occurrence_id, now_iso, owner, deadline_at],
        )
        .map_err(|error| format!("failed to mark occurrence running: {error}"))?;
    Ok(changed > 0)
}

/// Marks unresolved occurrences whose lease has expired as failed. Running
/// rows with a linked running ledger entry are reconciled by `delivery`, which
/// compares the session's actual completion time with the pinned deadline;
/// blindly failing them here could create occurrence/run contradictions.
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
             WHERE state IN ('claimed', 'running')
               AND lease_expires_at IS NOT NULL
               AND lease_expires_at < ?1
               AND NOT EXISTS (
                   SELECT 1 FROM automation_runs AS run
                   WHERE run.occurrence_id = automation_occurrences.id
                     AND run.status = 'running'
               )",
            params![now_iso],
        )
        .map_err(|error| format!("failed to recover expired leases: {error}"))?;
    Ok(changed)
}

/// Finalizes an occurrence into a terminal state. Releasing a PLANNED
/// occurrence is refused — only claimed work can settle.
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

pub fn fail_occurrence_nonterminal(
    conn: &Connection,
    occurrence_id: &str,
    failure_reason: &str,
    now: DateTime<Utc>,
) -> Result<bool, String> {
    let changed = conn
        .execute(
            "UPDATE automation_occurrences
             SET state = 'failed',
                 failure_reason = ?2,
                 lease_owner = NULL,
                 lease_expires_at = NULL,
                 updated_at = ?3
             WHERE id = ?1 AND state IN ('planned', 'claimed', 'running')",
            params![occurrence_id, failure_reason, iso(now)],
        )
        .map_err(|error| format!("failed to fail nonterminal occurrence: {error}"))?;
    Ok(changed > 0)
}

/// One full tick: plan due slots, recover expired leases, then claim the
/// earliest due occurrence of every ACTIVE routine that has one.
pub fn tick(conn: &Connection, now: DateTime<Utc>) -> Result<TickReport> {
    let mut report = TickReport::default();
    let mut seen: BTreeSet<String> = BTreeSet::new();

    let definitions = active_definitions(conn)?;
    for active in &definitions {
        let definition = &active.definition;
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

    report.recovered = recover_expired_leases(conn, now).map_err(anyhow::Error::msg)?;

    for active in &definitions {
        match claim_due_occurrence_at_revision(
            conn,
            &active.definition.id,
            "daemon",
            active.revision,
            &active.digest,
            now,
        ) {
            Ok(Some(id)) => report.claimed.push(id),
            Ok(None) => {}
            Err(error) => report
                .failed
                .push(format!("{}: {error}", active.definition.id)),
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
fn active_definitions(conn: &Connection) -> Result<Vec<ActiveDefinition>> {
    let records = super::store::list_definitions(conn)?;
    let mut definitions = Vec::new();
    for record in records {
        if record.status != "ACTIVE" {
            continue;
        }
        let definition: RoutineDefinition = serde_json::from_str(&record.definition_json)
            .with_context(|| format!("stored routine `{}` is unreadable", record.id))?;
        if definition.status != RoutineStatus::Active {
            continue;
        }
        definitions.push(ActiveDefinition {
            definition,
            revision: record.revision,
            digest: record.definition_digest,
        });
    }
    Ok(definitions)
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
    latest_due(&definition.rrule, definition.timezone, cursor, now)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanOutcome {
    Planned(PlannedOccurrence),
    NotDue,
    AlreadyFenced,
}

/// Fences the latest due slot for one routine.
///
/// The walk starts at the later of the definition's creation time and its
/// latest fenced occurrence: slots before the routine existed are never
/// backfilled, and slots already fenced are never re-planned.
pub fn plan_latest_due_occurrence(
    conn: &Connection,
    definition: &RoutineDefinition,
    created_at: DateTime<Utc>,
    now: DateTime<Utc>,
) -> Result<PlanOutcome, String> {
    if definition.status != RoutineStatus::Active {
        return Ok(PlanOutcome::NotDue);
    }
    let latest_fenced: Option<String> = conn
        .query_row(
            "SELECT MAX(scheduled_for) FROM automation_occurrences WHERE automation_id = ?1",
            params![definition.id],
            |row| row.get(0),
        )
        .map_err(|error| format!("failed to read occurrence fence: {error}"))?;
    let cursor = match latest_fenced {
        Some(iso_text) => match chrono::DateTime::parse_from_rfc3339(&iso_text) {
            Ok(parsed) => parsed.with_timezone(&Utc).max(created_at),
            Err(_) => created_at,
        },
        None => created_at,
    };
    let Some(slot) = latest_due_slot_after(definition, cursor, now)? else {
        return Ok(PlanOutcome::NotDue);
    };

    let slot_iso = iso(slot);
    let now_iso = iso(now);
    let occurrence_key = format!("{}@{slot_iso}", definition.id);
    let id = format!(
        "occ-{}",
        uuid::Uuid::new_v5(&OCCURRENCE_ID_NAMESPACE, occurrence_key.as_bytes())
    );
    let changed = conn
        .execute(
            "INSERT OR IGNORE INTO automation_occurrences
                (id, automation_id, scheduled_for, state, attempt, created_at, updated_at)
             VALUES (?1, ?2, ?3, 'planned', 0, ?4, ?4)",
            params![id, definition.id, slot_iso, now_iso],
        )
        .map_err(|error| format!("failed to fence occurrence: {error}"))?;

    if changed == 0 {
        return Ok(PlanOutcome::AlreadyFenced);
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

    let definitions = active_definitions(conn)?;
    for active in definitions {
        let definition = active.definition;
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
    use crate::automations::store::insert_definition;
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
            "UPDATE automation_definitions SET created_at = ?1 WHERE id = 'daily'",
            rusqlite::params![old_created],
        )
        .unwrap();

        let report = tick_planning(&conn, real_now()).unwrap();
        assert_eq!(report.planned.len(), 1);
        assert_eq!(report.already_fenced, 0);

        // A second tick has the same slot fenced and the next slot still in
        // the future: nothing new is planned and nothing is double-counted.
        let second = tick_planning(&conn, real_now()).unwrap();
        assert!(second.planned.is_empty());
        assert_eq!(second.already_fenced, 0);
    }

    #[test]
    fn collapses_three_missed_days_to_the_latest() {
        let (_temp, conn) = temp_store();
        insert_definition(&conn, &definition("daily", "ACTIVE", "FREQ=DAILY;BYHOUR=9")).unwrap();
        // Simulate a routine created four days ago.
        let old_created = (real_now() - chrono::Duration::days(4))
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        conn.execute(
            "UPDATE automation_definitions SET created_at = ?1 WHERE id = 'daily'",
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
    fn daily_latest_misfire_is_correct_beyond_ninety_six_slots() {
        let (_temp, conn) = temp_store();
        let routine = definition("long-gap", "ACTIVE", "FREQ=DAILY;BYHOUR=9");
        let created = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        let now = Utc.with_ymd_and_hms(2026, 8, 28, 12, 0, 0).unwrap();

        plan_latest_due_occurrence(&conn, &routine, created, now).unwrap();

        let scheduled: String = conn
            .query_row(
                "SELECT scheduled_for FROM automation_occurrences WHERE automation_id = 'long-gap'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(scheduled, "2026-08-28T09:00:00.000Z");
        let occurrence_id: String = conn
            .query_row(
                "SELECT id FROM automation_occurrences WHERE automation_id = 'long-gap'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        uuid::Uuid::parse_str(occurrence_id.trim_start_matches("occ-")).unwrap();
    }

    #[test]
    fn twice_daily_latest_misfire_is_correct_beyond_ninety_six_slots() {
        let (_temp, conn) = temp_store();
        let routine = definition("twice-long-gap", "ACTIVE", "FREQ=DAILY;BYHOUR=9,17");
        let created = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        let now = Utc.with_ymd_and_hms(2026, 8, 28, 20, 0, 0).unwrap();

        plan_latest_due_occurrence(&conn, &routine, created, now).unwrap();

        let scheduled: String = conn
            .query_row(
                "SELECT scheduled_for FROM automation_occurrences
                 WHERE automation_id = 'twice-long-gap'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(scheduled, "2026-08-28T17:00:00.000Z");
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
            "UPDATE automation_definitions SET created_at = ?1 WHERE id = 'daily'",
            rusqlite::params![old_created],
        )
        .unwrap();
        tick_planning(&conn, real_now()).unwrap();

        let claimed = claim_due_occurrence(&conn, "daily", "daemon-a", 60, real_now()).unwrap();
        assert!(claimed.is_some());
        let (revision, digest, snapshot, deadline): (
            Option<i64>,
            Option<String>,
            Option<String>,
            Option<String>,
        ) = conn
            .query_row(
                "SELECT definition_revision, definition_digest, definition_json, deadline_at
                 FROM automation_occurrences WHERE id = ?1",
                rusqlite::params![claimed.as_deref().unwrap()],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(revision, Some(1));
        assert!(digest
            .as_deref()
            .is_some_and(|value| value.starts_with("sha256:")));
        assert!(snapshot
            .as_deref()
            .is_some_and(|value| value.contains("\"id\":\"daily\"")));
        assert!(deadline.is_some());

        // A second claimant finds nothing left to claim.
        let second = claim_due_occurrence(&conn, "daily", "daemon-b", 60, real_now()).unwrap();
        assert!(second.is_none());
    }

    #[test]
    fn claim_deadline_uses_the_same_revision_as_the_pinned_inputs() {
        let (_temp, conn) = temp_store();
        let mut first = definition("coherent", "ACTIVE", "FREQ=DAILY;BYHOUR=9");
        first.timeout_minutes = 5;
        first.prompt = "revision A".to_string();
        insert_definition(&conn, &first).unwrap();
        let now = real_now();
        let now_iso = now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        conn.execute(
            "INSERT INTO automation_occurrences
                (id, automation_id, scheduled_for, state, attempt, created_at, updated_at)
             VALUES ('coherent-occ', 'coherent', ?1, 'planned', 0, ?1, ?1)",
            rusqlite::params![now_iso],
        )
        .unwrap();

        let mut second = first.clone();
        second.timeout_minutes = 60;
        second.prompt = "revision B".to_string();
        super::super::store::update_definition(&conn, &second)
            .unwrap()
            .unwrap();

        claim_occurrence_by_id(&conn, "coherent-occ", "daemon-a", 5, now)
            .unwrap()
            .unwrap();

        let (revision, snapshot_json, deadline): (i64, String, String) = conn
            .query_row(
                "SELECT definition_revision, definition_json, deadline_at
                 FROM automation_occurrences WHERE id = 'coherent-occ'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(revision, 2);
        assert!(snapshot_json.contains("revision B"), "{snapshot_json}");
        assert_eq!(
            deadline,
            (now + chrono::Duration::minutes(60))
                .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
        );
    }

    #[test]
    fn claim_refuses_a_stale_definition_revision_without_partial_state() {
        let (_temp, conn) = temp_store();
        let first = definition("revision-race", "ACTIVE", "FREQ=DAILY;BYHOUR=9");
        insert_definition(&conn, &first).unwrap();
        let expected = super::super::store::get_definition(&conn, "revision-race")
            .unwrap()
            .unwrap();
        let now = real_now();
        let now_iso = now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        conn.execute(
            "INSERT INTO automation_occurrences
                (id, automation_id, scheduled_for, state, attempt, created_at, updated_at)
             VALUES ('revision-race-occ', 'revision-race', ?1, 'planned', 0, ?1, ?1)",
            rusqlite::params![now_iso],
        )
        .unwrap();
        let mut changed = first;
        changed.timeout_minutes = 90;
        super::super::store::update_definition(&conn, &changed)
            .unwrap()
            .unwrap();

        let error = claim_occurrence_by_id_at_revision(
            &conn,
            "revision-race-occ",
            "daemon-a",
            expected.revision,
            &expected.definition_digest,
            now,
        )
        .unwrap_err();

        assert!(error.contains("changed before claim"), "{error}");
        let state: (String, Option<String>, Option<String>) = conn
            .query_row(
                "SELECT state, definition_json, deadline_at
                 FROM automation_occurrences WHERE id = 'revision-race-occ'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(state, ("planned".to_string(), None, None));
    }

    #[test]
    fn concurrent_definition_update_is_serialized_before_revision_claim() {
        let (temp, conn) = temp_store();
        let first = definition("concurrent-revision", "ACTIVE", "FREQ=DAILY;BYHOUR=9");
        insert_definition(&conn, &first).unwrap();
        let expected = super::super::store::get_definition(&conn, "concurrent-revision")
            .unwrap()
            .unwrap();
        let now = real_now();
        let now_iso = now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        conn.execute(
            "INSERT INTO automation_occurrences
                (id, automation_id, scheduled_for, state, attempt, created_at, updated_at)
             VALUES ('concurrent-revision-occ', 'concurrent-revision', ?1,
                     'planned', 0, ?1, ?1)",
            rusqlite::params![now_iso],
        )
        .unwrap();

        let update_conn = crate::store::open_store(&temp.path().join("store.sqlite")).unwrap();
        let update = rusqlite::Transaction::new_unchecked(
            &update_conn,
            rusqlite::TransactionBehavior::Immediate,
        )
        .unwrap();
        let mut changed = first;
        changed.timeout_minutes = 120;
        changed.prompt = "concurrent revision B".to_string();
        super::super::store::update_definition(&update, &changed)
            .unwrap()
            .unwrap();

        let store_path = temp.path().join("store.sqlite");
        let expected_digest = expected.definition_digest.clone();
        let expected_revision = expected.revision;
        let claimant = std::thread::spawn(move || {
            let claim_conn = crate::store::open_store(&store_path).unwrap();
            claim_occurrence_by_id_at_revision(
                &claim_conn,
                "concurrent-revision-occ",
                "daemon-a",
                expected_revision,
                &expected_digest,
                now,
            )
        });
        update.commit().unwrap();

        let error = claimant.join().unwrap().unwrap_err();
        assert!(error.contains("changed before claim"), "{error}");
        let state: String = conn
            .query_row(
                "SELECT state FROM automation_occurrences
                 WHERE id = 'concurrent-revision-occ'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(state, "planned");
    }

    #[test]
    fn recovers_expired_leases_to_failed() {
        let (_temp, conn) = temp_store();
        insert_definition(&conn, &definition("daily", "ACTIVE", "FREQ=DAILY;BYHOUR=9")).unwrap();
        let old_created = (real_now() - chrono::Duration::days(1))
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        conn.execute(
            "UPDATE automation_definitions SET created_at = ?1 WHERE id = 'daily'",
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
    fn overlap_forbid_blocks_claims_while_a_run_is_live() {
        let (_temp, conn) = temp_store();
        insert_definition(&conn, &definition("daily", "ACTIVE", "FREQ=DAILY;BYHOUR=9")).unwrap();
        let old_created = (real_now() - chrono::Duration::days(1))
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        conn.execute(
            "UPDATE automation_definitions SET created_at = ?1 WHERE id = 'daily'",
            rusqlite::params![old_created],
        )
        .unwrap();
        // Two due slots: the collapse test's fence keeps only the latest, so
        // insert a second, earlier planned occurrence by hand.
        tick_planning(&conn, real_now()).unwrap();
        let planned: String = conn
            .query_row(
                "SELECT id FROM automation_occurrences WHERE automation_id = 'daily'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        conn.execute(
            "INSERT INTO automation_occurrences
                (id, automation_id, scheduled_for, state, attempt, created_at, updated_at)
             VALUES ('daily-earlier', 'daily', '2020-01-01T09:00:00.000Z', 'planned', 0,
                     '2020-01-01T09:00:00.000Z', '2020-01-01T09:00:00.000Z')",
            [],
        )
        .unwrap();

        // First claim wins; the second must be refused while it stays live,
        // even though another planned occurrence is due.
        let first = claim_due_occurrence(&conn, "daily", "daemon-a", 60, real_now()).unwrap();
        assert!(first.is_some());
        let second = claim_due_occurrence(&conn, "daily", "daemon-b", 60, real_now()).unwrap();
        assert!(
            second.is_none(),
            "overlap=forbid must reject a second claim"
        );

        // Settling the live run unblocks the next claim.
        let claimed_id = first.unwrap();
        assert!(settle_occurrence(&conn, &claimed_id, "succeeded", None, real_now()).unwrap());
        let third = claim_occurrence_by_id(&conn, &planned, "daemon-b", 60, real_now()).unwrap();
        assert!(third.is_some());
    }

    #[test]
    fn running_occurrences_carry_a_bounded_recoverable_lease() {
        let (_temp, conn) = temp_store();
        insert_definition(&conn, &definition("daily", "ACTIVE", "FREQ=DAILY;BYHOUR=9")).unwrap();
        let old_created = (real_now() - chrono::Duration::days(1))
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        conn.execute(
            "UPDATE automation_definitions SET created_at = ?1 WHERE id = 'daily'",
            rusqlite::params![old_created],
        )
        .unwrap();
        tick_planning(&conn, real_now()).unwrap();
        let occurrence_id = claim_due_occurrence(&conn, "daily", "daemon-a", 60, real_now())
            .unwrap()
            .unwrap();
        let deadline: String = conn
            .query_row(
                "SELECT deadline_at FROM automation_occurrences WHERE id = ?1",
                rusqlite::params![occurrence_id],
                |row| row.get(0),
            )
            .unwrap();

        // Running keeps the lease alive, so recovery leaves it alone.
        assert!(
            mark_occurrence_running(&conn, &occurrence_id, "daemon-a", &deadline, real_now())
                .unwrap()
        );
        let (state, owner): (String, Option<String>) = conn
            .query_row(
                "SELECT state, lease_owner FROM automation_occurrences WHERE id = ?1",
                rusqlite::params![occurrence_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(state, "running");
        assert_eq!(owner.as_deref(), Some("daemon-a"));
        let report = tick(&conn, real_now()).unwrap();
        assert_eq!(report.recovered, 0);

        // After the lease expires, recovery fails the row: a stale running
        // record never blocks the routine forever (coven#816).
        conn.execute(
            "UPDATE automation_occurrences SET lease_expires_at = '2020-01-01T00:00:00.000Z'",
            [],
        )
        .unwrap();
        let report = tick(&conn, real_now()).unwrap();
        assert_eq!(report.recovered, 1);
        let state: String = conn
            .query_row(
                "SELECT state FROM automation_occurrences WHERE id = ?1",
                rusqlite::params![occurrence_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(state, "failed");
    }

    #[test]
    fn maximum_timeout_claim_preserves_the_full_deadline() {
        let (_temp, conn) = temp_store();
        let mut routine = definition("maximum-timeout", "ACTIVE", "FREQ=DAILY;BYHOUR=9");
        routine.timeout_minutes = 44_640;
        insert_definition(&conn, &routine).unwrap();
        let now = real_now();
        let now_iso = now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        conn.execute(
            "INSERT INTO automation_occurrences
                (id, automation_id, scheduled_for, state, attempt, created_at, updated_at)
             VALUES ('maximum-timeout-occ', 'maximum-timeout', ?1,
                     'planned', 0, ?1, ?1)",
            rusqlite::params![now_iso],
        )
        .unwrap();

        claim_occurrence_by_id(&conn, "maximum-timeout-occ", "daemon-a", 44_640, now)
            .unwrap()
            .unwrap();

        let deadline: String = conn
            .query_row(
                "SELECT deadline_at FROM automation_occurrences
                 WHERE id = 'maximum-timeout-occ'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            deadline,
            (now + chrono::Duration::minutes(44_640))
                .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
        );
    }

    #[test]
    fn settles_claimed_work_but_never_planned() {
        let (_temp, conn) = temp_store();
        insert_definition(&conn, &definition("daily", "ACTIVE", "FREQ=DAILY;BYHOUR=9")).unwrap();
        let old_created = (real_now() - chrono::Duration::days(1))
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        conn.execute(
            "UPDATE automation_definitions SET created_at = ?1 WHERE id = 'daily'",
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
