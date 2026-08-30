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

/// One automations pass: open the store, run the full tick (plan, recover,
/// claim), dispatch every claimed occurrence through the shared
/// session-launch runtime, then reconcile running runs. Failures land in the
/// daemon recovery log via the caller.
pub fn process_automations_tick(
    coven_home: &Path,
    runtime: &dyn crate::api::SessionRuntime,
) -> Result<super::occurrences::TickReport> {
    let store_path = crate::api::store_path(coven_home);
    let conn = crate::store::open_store(&store_path)?;
    let now = chrono::Utc::now();
    let report = super::occurrences::tick(&conn, now)?;
    if !report.claimed.is_empty() {
        let _dispatch = super::runner::dispatch_claimed_occurrences(&conn, runtime, now)
            .map_err(anyhow::Error::msg)?;
    }
    let settlement =
        super::delivery::settle_finished_runs(&conn, now).map_err(anyhow::Error::msg)?;
    for failure in &settlement.failures {
        crate::daemon::append_daemon_recovery_log(
            coven_home,
            &format!("automations run failed: {failure}"),
        );
    }
    Ok(report)
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
        // from that store.
        let conn = crate::store::open_store(&home.join("coven.sqlite3")).unwrap();
        crate::store::insert_session(
            &conn,
            &crate::store::SessionRecord {
                id: session_id.clone(),
                project_root: "/work/project".to_string(),
                harness: "coven-code".to_string(),
                title: "daily".to_string(),
                status: "completed".to_string(),
                exit_code: Some(0),
                archived_at: None,
                created_at: chrono::Utc::now().to_rfc3339(),
                updated_at: chrono::Utc::now().to_rfc3339(),
                conversation_id: None,
                familiar_id: Some("charm".to_string()),
                execution_binding: None,
                labels: Vec::new(),
                visibility: "private".to_string(),
                external: false,
                transcript_path: None,
            },
        )
        .unwrap();
        crate::store::insert_event(
            &conn,
            &crate::store::EventRecord {
                seq: 0,
                id: "event-final".to_string(),
                session_id: session_id,
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
}
