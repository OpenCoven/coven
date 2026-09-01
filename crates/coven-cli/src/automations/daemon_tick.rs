//! Daemon-side automations tick (coven#816).
//!
//! The daemon reconciles terminal session evidence before planning, recovering,
//! claiming, and dispatching occurrences on a 60-second cadence.

use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};

/// One automations pass: open the store, run the full tick (plan, recover,
/// claim), then dispatch every claimed occurrence through the shared
/// session-launch runtime. Failures land in the daemon recovery log via the
/// caller.
pub fn process_automations_tick(
    coven_home: &Path,
    runtime: &dyn crate::api::SessionRuntime,
) -> Result<super::occurrences::TickReport> {
    let store_path = crate::api::store_path(coven_home);
    let conn = crate::store::open_store(&store_path)?;
    let now = chrono::Utc::now();
    reconcile_automation_runs(coven_home, &conn, runtime, now)?;
    let report = super::occurrences::tick(&conn, now)?;
    let _dispatch = super::runner::dispatch_claimed_occurrences(&conn, runtime, now)
        .map_err(anyhow::Error::msg)?;
    Ok(report)
}

fn reconcile_automation_runs(
    coven_home: &Path,
    conn: &rusqlite::Connection,
    runtime: &dyn crate::api::SessionRuntime,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<()> {
    super::runner::recover_restart_containment(coven_home, conn, now, false)
        .map_err(anyhow::Error::msg)?;
    for failure in
        super::runner::recover_abandoned_launches(conn, runtime, now).map_err(anyhow::Error::msg)?
    {
        crate::daemon::append_daemon_recovery_log(coven_home, &failure);
    }
    for failure in
        super::runner::enforce_run_timeouts(conn, runtime, now).map_err(anyhow::Error::msg)?
    {
        crate::daemon::append_daemon_recovery_log(coven_home, &failure);
    }
    super::runner::settle_finished_runs(conn, now).map_err(anyhow::Error::msg)?;
    super::runner::cleanup_terminal_containment_receipts(coven_home, conn)
        .map_err(anyhow::Error::msg)?;
    Ok(())
}

/// Starts the automations scheduler thread on the daemon's 60s cadence.
pub fn start_automations_scheduler(
    coven_home: &Path,
    runtime: std::sync::Arc<dyn crate::api::SessionRuntime + Send + Sync>,
) -> Result<()> {
    let store_path = crate::api::store_path(coven_home);
    let conn = crate::store::open_store(&store_path)?;
    let now = chrono::Utc::now();
    super::runner::recover_restart_containment(coven_home, &conn, now, true)
        .map_err(anyhow::Error::msg)?;
    reconcile_automation_runs(coven_home, &conn, runtime.as_ref(), now)?;
    let _dispatch = super::runner::dispatch_claimed_occurrences(&conn, runtime.as_ref(), now)
        .map_err(anyhow::Error::msg)?;
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
    fn tick_plans_and_claims_against_the_daemon_store() {
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
        // A launch acknowledgement proves only that the runtime accepted the
        // session. Terminal settlement waits for completion evidence.
        assert_eq!(state, "running");
        let runs = super::super::runs::list_runs(&conn, "daily", 10).unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].status, "running");
        assert!(runs[0].session_id.is_some());
        assert_eq!(runs[0].finished_at, None);
    }

    #[test]
    fn tick_dispatches_a_preexisting_daemon_claim_without_new_claims() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path();
        crate::store::initialize_store(&home.join("coven.sqlite3")).unwrap();
        let conn = crate::store::open_store(&home.join("coven.sqlite3")).unwrap();
        let routine = definition("preclaimed");
        insert_definition(&conn, &routine).unwrap();
        let now = chrono::Utc::now();
        assert!(super::super::occurrences::insert_claimed_occurrence(
            &conn,
            "preexisting-daemon-claim",
            &routine.id,
            "daemon",
            60,
            now,
        )
        .unwrap());
        drop(conn);

        let report = process_automations_tick(home, &crate::api::NoopSessionRuntime).unwrap();
        assert!(report.claimed.is_empty());

        let conn = crate::store::open_store(&home.join("coven.sqlite3")).unwrap();
        let state: String = conn
            .query_row(
                "SELECT state FROM automation_occurrences WHERE id = 'preexisting-daemon-claim'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(state, "running");
        assert_eq!(
            super::super::runs::list_runs(&conn, &routine.id, 10)
                .unwrap()
                .len(),
            1
        );
    }
}
