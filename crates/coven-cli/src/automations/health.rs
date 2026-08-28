//! Routine health snapshot (coven#816).
//!
//! One structured summary per routine that the Cave UI renders: when the
//! routine was last planned, started, and succeeded, the next due slot,
//! consecutive failures, and any live lease or stale/degraded reason. The
//! values come only from the Coven store — no harness history files.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};

use super::definition::RoutineDefinition;
use super::schedule::next_due;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoutineHealth {
    pub automation_id: String,
    pub next_due_at: Option<String>,
    pub last_planned_at: Option<String>,
    pub last_started_at: Option<String>,
    pub last_success_at: Option<String>,
    pub consecutive_failures: i64,
    pub lease_owner: Option<String>,
    pub lease_expires_at: Option<String>,
    pub stale_reason: Option<String>,
}

fn max_iso(
    conn: &Connection,
    sql: &str,
    params: &[&dyn rusqlite::ToSql],
) -> Result<Option<String>> {
    let value: Option<String> = conn
        .query_row(sql, params, |row| row.get(0))
        .with_context(|| format!("health query failed: {sql}"))?;
    Ok(value)
}

/// Computes the health snapshot for one routine. Reads-only.
pub fn routine_health(conn: &Connection, id: &str, now: DateTime<Utc>) -> Result<RoutineHealth> {
    let definition: RoutineDefinition =
        super::runner::load_definition_for_run(conn, id)
            .map_err(anyhow::Error::msg)?
            .ok_or_else(|| anyhow::anyhow!("no routine with id `{id}`"))?;

    let next_due_at = next_due(&definition.rrule, definition.timezone, now)
        .map_err(|error| anyhow::anyhow!("{error}"))?
        .map(|slot| slot.to_rfc3339_opts(chrono::SecondsFormat::Millis, true));

    let last_planned_at = max_iso(
        conn,
        "SELECT MAX(scheduled_for) FROM automation_occurrences WHERE automation_id = ?1 AND state = 'planned'",
        &[&id],
    )?;
    let last_started_at = max_iso(
        conn,
        "SELECT MAX(scheduled_for) FROM automation_occurrences WHERE automation_id = ?1 AND state IN ('claimed', 'running')",
        &[&id],
    )?;
    let last_success_at = max_iso(
        conn,
        "SELECT MAX(scheduled_for) FROM automation_occurrences WHERE automation_id = ?1 AND state = 'succeeded'",
        &[&id],
    )?;
    let consecutive_failures: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM (
                 SELECT scheduled_for, state FROM automation_occurrences
                 WHERE automation_id = ?1 AND state = 'failed'
                   AND scheduled_for > COALESCE(
                       (SELECT MAX(scheduled_for) FROM automation_occurrences
                        WHERE automation_id = ?1 AND state = 'succeeded'), '')
                 ORDER BY scheduled_for DESC
             )",
            params![id],
            |row| row.get(0),
        )
        .context("failed to count consecutive failures")?;

    let (lease_owner, lease_expires_at, stale_reason) = match conn.query_row(
        "SELECT lease_owner, lease_expires_at, failure_reason
             FROM automation_occurrences
             WHERE automation_id = ?1 AND state IN ('claimed', 'running')
             ORDER BY scheduled_for DESC LIMIT 1",
        params![id],
        |row| {
            Ok((
                row.get::<_, Option<String>>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        },
    ) {
        Ok(row) => row,
        Err(rusqlite::Error::QueryReturnedNoRows) => (None, None, None),
        Err(error) => return Err(error).context("failed to read live lease"),
    };

    Ok(RoutineHealth {
        automation_id: id.to_string(),
        next_due_at,
        last_planned_at,
        last_started_at,
        last_success_at,
        consecutive_failures,
        lease_owner,
        lease_expires_at,
        stale_reason,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::automations::definition::RoutineDefinition;
    use crate::automations::store::insert_definition;
    use crate::store::initialize_store;
    use chrono::TimeZone;
    use serde_json::json;

    fn utc(y: i32, m: u32, d: u32, h: u32, min: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, m, d, h, min, 0).unwrap()
    }

    fn definition(id: &str) -> RoutineDefinition {
        RoutineDefinition::from_json(&json!({
            "schemaVersion": 1,
            "id": id,
            "name": id,
            "status": "ACTIVE",
            "rrule": "FREQ=DAILY;BYHOUR=9",
            "timezone": "utc",
            "misfire": "latest",
            "overlap": "forbid",
            "timeoutMinutes": 30,
            "runtime": "coven-code",
            "cwd": "/work/project",
            "prompt": "Do the thing."
        }))
        .unwrap()
    }

    fn temp_store() -> (tempfile::TempDir, Connection) {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("store.sqlite");
        initialize_store(&path).unwrap();
        let conn = crate::store::open_store(&path).unwrap();
        (temp, conn)
    }

    #[test]
    fn health_reports_next_due_and_history() {
        let (_temp, conn) = temp_store();
        insert_definition(&conn, &definition("daily")).unwrap();
        conn.execute(
            "INSERT INTO automation_occurrences
                (id, automation_id, scheduled_for, state, attempt, created_at, updated_at)
             VALUES
                ('o1', 'daily', '2026-08-26T09:00:00.000Z', 'succeeded', 1, '2026-08-26T09:00:00.000Z', '2026-08-26T09:05:00.000Z'),
                ('o2', 'daily', '2026-08-27T09:00:00.000Z', 'failed', 1, '2026-08-27T09:00:00.000Z', '2026-08-27T09:05:00.000Z')",
            [],
        )
        .unwrap();

        let health = routine_health(&conn, "daily", utc(2026, 8, 27, 10, 0)).unwrap();
        assert_eq!(
            health.next_due_at.as_deref(),
            Some("2026-08-28T09:00:00.000Z")
        );
        assert_eq!(
            health.last_success_at.as_deref(),
            Some("2026-08-26T09:00:00.000Z")
        );
        assert_eq!(health.consecutive_failures, 1);
    }

    #[test]
    fn health_reports_a_live_lease() {
        let (_temp, conn) = temp_store();
        insert_definition(&conn, &definition("daily")).unwrap();
        conn.execute(
            "INSERT INTO automation_occurrences
                (id, automation_id, scheduled_for, state, lease_owner, lease_expires_at, attempt, created_at, updated_at)
             VALUES ('o1', 'daily', '2026-08-27T09:00:00.000Z', 'claimed', 'daemon-a',
                     '2026-08-27T10:00:00.000Z', 1, '2026-08-27T09:00:00.000Z', '2026-08-27T09:00:00.000Z')",
            [],
        )
        .unwrap();

        let health = routine_health(&conn, "daily", utc(2026, 8, 27, 9, 30)).unwrap();
        assert_eq!(health.lease_owner.as_deref(), Some("daemon-a"));
        assert_eq!(
            health.lease_expires_at.as_deref(),
            Some("2026-08-27T10:00:00.000Z")
        );
    }

    #[test]
    fn health_requires_an_existing_routine() {
        let (_temp, conn) = temp_store();
        let error = routine_health(&conn, "missing", utc(2026, 8, 27, 10, 0)).unwrap_err();
        assert!(format!("{error:#}").contains("no routine"));
    }
}
