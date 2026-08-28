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
        scheduled_for TEXT NOT NULL,
        state TEXT NOT NULL DEFAULT 'planned',
        lease_owner TEXT,
        lease_expires_at TEXT,
        attempt INTEGER NOT NULL DEFAULT 0,
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

fn iso(instant: DateTime<Utc>) -> String {
    instant.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// Reads the ACTIVE definitions from the store as validated records.
fn active_definitions(conn: &Connection) -> Result<Vec<RoutineDefinition>> {
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
        definitions.push(definition);
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
    let mut walk = cursor;
    let mut latest: Option<DateTime<Utc>> = None;

    for _ in 0..96 {
        let Some(next) = next_due(&definition.rrule, definition.timezone, walk)? else {
            break;
        };
        if next > now {
            break;
        }
        latest = Some(next);
        walk = next;
    }

    Ok(latest)
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
    let id = format!("{}-{}", definition.id, slot.timestamp_millis());
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
/// tick fences nothing twice.
pub fn tick_planning(conn: &Connection, now: DateTime<Utc>) -> Result<PlanTickReport> {
    let mut report = PlanTickReport::default();
    let mut seen: BTreeSet<String> = BTreeSet::new();

    let definitions = active_definitions(conn)?;
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
    use crate::automations::store::insert_definition;
    use crate::store::initialize_store;
    use chrono::Timelike;
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
