//! Daemon-side automations tick (coven#816).
//!
//! The daemon runs the full ownership loop on a 60-second cadence: plan due
//! occurrences, recover expired leases, claim work, dispatch claimed
//! occurrences through the shared session-launch runtime, then settle
//! finished runs (terminal status, bounded log, output delivery) from the
//! Coven session store. Coven owns every step; the runtime is a replaceable
//! worker.

use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};

/// One automations pass: the shared full tick (`automations::full_tick`) —
/// plan, recover, claim, dispatch every valid existing claim, and settle
/// finished runs. Failures land in the daemon recovery log via the caller.
pub fn process_automations_tick(
    coven_home: &Path,
    runtime: &dyn crate::api::SessionRuntime,
) -> Result<super::occurrences::TickReport> {
    let store_path = crate::api::store_path(coven_home);
    let conn = crate::store::open_store(&store_path)?;
    let now = chrono::Utc::now();
    let report = super::full_tick(coven_home, &conn, runtime, now).map_err(anyhow::Error::msg)?;
    for failure in &report.settlement.failures {
        crate::daemon::append_daemon_recovery_log(
            coven_home,
            &format!("automations run failed: {failure}"),
        );
    }
    Ok(report.tick)
}

/// Starts the automations scheduler thread on the daemon's 60s cadence.
pub fn start_automations_scheduler(
    coven_home: &Path,
    runtime: std::sync::Arc<dyn crate::api::SessionRuntime + Send + Sync>,
) -> Result<()> {
    let home = coven_home.to_path_buf();
    std::thread::Builder::new()
        .name("coven-automations-scheduler".into())
        .spawn(move || loop {
            std::thread::sleep(Duration::from_secs(60));
            if let Err(error) = process_automations_tick(&home, runtime.as_ref()) {
                crate::daemon::append_daemon_recovery_log(
                    &home,
                    &format!("automations tick failed: {error:#}"),
                );
            }
        })
        .context("failed to spawn automations scheduler")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::automations::definition::RoutineDefinition;
    use crate::automations::store::insert_definition;
    use serde_json::json;

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

    #[test]
    fn tick_plans_claims_dispatches_and_settles() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path();
        crate::store::initialize_store(&home.join("coven.sqlite3")).unwrap();
        let conn = crate::store::open_store(&home.join("coven.sqlite3")).unwrap();
        insert_definition(&conn, &definition("daily")).unwrap();
        // Backdate creation so the 09:00 slot is due at any tick hour.
        let old_created = (chrono::Utc::now() - chrono::Duration::days(1))
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        conn.execute(
            "UPDATE automation_definitions SET created_at = ?1 WHERE id = 'daily'",
            rusqlite::params![old_created],
        )
        .unwrap();
        drop(conn);

        let report = process_automations_tick(home, &crate::api::NoopSessionRuntime).unwrap();
        assert_eq!(report.planned.len(), 1);
        assert_eq!(report.claimed.len(), 1);

        let conn = crate::store::open_store(&home.join("coven.sqlite3")).unwrap();
        let state: String = conn
            .query_row(
                "SELECT state FROM automation_occurrences WHERE automation_id = 'daily'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        // The tick dispatched the claimed occurrence through the (noop)
        // runtime: the run is in flight, never instantly "successful".
        assert_eq!(state, "running");
        let runs = super::super::runs::list_runs(&conn, "daily", 10).unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].status, "running");
        let session_id = runs[0].session_id.clone().unwrap();
        drop(conn);

        // The session then finishes (the PTY writer flips the sessions row
        // and records the normalized stream); the next tick settles the run
        // from that store. The durable launch primitive already persisted
        // the session row before spawn — the writer only flips it terminal.
        let conn = crate::store::open_store(&home.join("coven.sqlite3")).unwrap();
        conn.execute(
            "UPDATE sessions SET status = 'completed', exit_code = 0, updated_at = ?2
             WHERE id = ?1",
            rusqlite::params![session_id, chrono::Utc::now().to_rfc3339()],
        )
        .unwrap();
        crate::store::insert_event(
            &conn,
            &crate::store::EventRecord {
                seq: 0,
                id: "event-final".to_string(),
                session_id,
                kind: "output".to_string(),
                payload_json: serde_json::json!({ "data": "done" }).to_string(),
                created_at: chrono::Utc::now().to_rfc3339(),
            },
        )
        .unwrap();
        drop(conn);

        process_automations_tick(home, &crate::api::NoopSessionRuntime).unwrap();
        let conn = crate::store::open_store(&home.join("coven.sqlite3")).unwrap();
        let state: String = conn
            .query_row(
                "SELECT state FROM automation_occurrences WHERE automation_id = 'daily'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(state, "succeeded");
        let runs = super::super::runs::list_runs(&conn, "daily", 10).unwrap();
        assert_eq!(runs[0].status, "succeeded");
        assert_eq!(runs[0].exit_code, Some(0));
    }

    #[test]
    fn tick_dispatches_a_valid_pre_existing_claim() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path();
        crate::store::initialize_store(&home.join("coven.sqlite3")).unwrap();
        let conn = crate::store::open_store(&home.join("coven.sqlite3")).unwrap();
        insert_definition(&conn, &definition("daily")).unwrap();
        // A claim a previous process made and never dispatched (crash gap):
        // live lease, state claimed, nothing new planned this tick.
        let now = chrono::Utc::now();
        let millis = chrono::SecondsFormat::Millis;
        conn.execute(
            "INSERT INTO automation_occurrences
                (id, automation_id, scheduled_for, state, lease_owner, lease_expires_at,
                 attempt, created_at, updated_at)
             VALUES ('daily-stuck', 'daily', ?2, 'claimed', 'daemon-a', ?3, 1, ?2, ?2)",
            rusqlite::params![
                "daily-stuck",
                (now - chrono::Duration::hours(1)).to_rfc3339_opts(millis, true),
                (now + chrono::Duration::hours(1)).to_rfc3339_opts(millis, true),
            ],
        )
        .unwrap();
        drop(conn);

        process_automations_tick(home, &crate::api::NoopSessionRuntime).unwrap();

        // The pre-existing claim is dispatched by this pass — never left
        // stuck (coven#816 finding 2).
        let conn = crate::store::open_store(&home.join("coven.sqlite3")).unwrap();
        let state: String = conn
            .query_row(
                "SELECT state FROM automation_occurrences WHERE id = 'daily-stuck'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(state, "running");
    }
}
