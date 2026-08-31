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

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct FullTickReport {
    pub occurrences: super::occurrences::TickReport,
    pub dispatch: super::runner::DispatchReport,
    pub settlement: super::delivery::ReconcileReport,
}

pub fn run_full_tick(
    conn: &rusqlite::Connection,
    coven_home: &Path,
    runtime: &dyn crate::api::SessionRuntime,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<FullTickReport> {
    let mut settlement =
        super::delivery::settle_finished_runs(conn, now).map_err(anyhow::Error::msg)?;
    let occurrences = super::occurrences::tick(conn, now)?;
    let dispatch = super::runner::dispatch_claimed_occurrences(conn, coven_home, runtime, now)
        .map_err(anyhow::Error::msg)?;
    let after_dispatch =
        super::delivery::settle_finished_runs(conn, now).map_err(anyhow::Error::msg)?;
    settlement.settled_succeeded += after_dispatch.settled_succeeded;
    settlement.settled_failed += after_dispatch.settled_failed;
    settlement.still_running = after_dispatch.still_running;
    settlement.failures.extend(after_dispatch.failures);
    Ok(FullTickReport {
        occurrences,
        dispatch,
        settlement,
    })
}

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
    let full = run_full_tick(&conn, coven_home, runtime, now)?;
    for failure in full
        .settlement
        .failures
        .iter()
        .chain(full.dispatch.failed.iter())
    {
        crate::daemon::append_daemon_recovery_log(
            coven_home,
            &format!("automations run failed: {failure}"),
        );
    }
    Ok(full.occurrences)
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
        let project = home.join("project");
        std::fs::create_dir_all(&project).unwrap();
        crate::store::initialize_store(&home.join("coven.sqlite3")).unwrap();
        let conn = crate::store::open_store(&home.join("coven.sqlite3")).unwrap();
        let mut routine = definition("daily");
        routine.cwd = Some(project.to_string_lossy().into_owned());
        insert_definition(&conn, &routine).unwrap();
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
        crate::store::update_session_status(
            &conn,
            &session_id,
            "completed",
            Some(0),
            &chrono::Utc::now().to_rfc3339(),
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
    fn tick_dispatches_a_valid_claim_left_by_an_interrupted_prior_tick() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path();
        let project = home.join("project");
        std::fs::create_dir_all(&project).unwrap();
        crate::store::initialize_store(&home.join("coven.sqlite3")).unwrap();
        let conn = crate::store::open_store(&home.join("coven.sqlite3")).unwrap();
        let mut routine = definition("interrupted");
        routine.status = crate::automations::definition::RoutineStatus::Paused;
        routine.cwd = Some(project.to_string_lossy().into_owned());
        insert_definition(&conn, &routine).unwrap();
        let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        conn.execute(
            "INSERT INTO automation_occurrences
                (id, automation_id, scheduled_for, state, attempt, created_at, updated_at)
             VALUES ('occ-interrupted', 'interrupted', ?1, 'planned', 0, ?1, ?1)",
            rusqlite::params![now],
        )
        .unwrap();
        crate::automations::occurrences::claim_occurrence_by_id(
            &conn,
            "occ-interrupted",
            "prior-tick",
            30,
            chrono::Utc::now(),
        )
        .unwrap()
        .unwrap();
        drop(conn);

        let report = process_automations_tick(home, &crate::api::NoopSessionRuntime).unwrap();

        assert!(
            report.claimed.is_empty(),
            "this reproduction must not depend on a newly claimed occurrence"
        );
        let conn = crate::store::open_store(&home.join("coven.sqlite3")).unwrap();
        let state: String = conn
            .query_row(
                "SELECT state FROM automation_occurrences WHERE id = 'occ-interrupted'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(state, "running");
        assert_eq!(
            super::super::runs::list_runs(&conn, "interrupted", 10)
                .unwrap()
                .len(),
            1
        );
        drop(conn);

        process_automations_tick(home, &crate::api::NoopSessionRuntime).unwrap();
        let conn = crate::store::open_store(&home.join("coven.sqlite3")).unwrap();
        assert_eq!(
            super::super::runs::list_runs(&conn, "interrupted", 10)
                .unwrap()
                .len(),
            1,
            "a recovered claim must not be dispatched twice"
        );
    }

    #[test]
    fn tick_does_not_redispatch_a_claim_that_already_has_a_running_ledger_row() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path();
        let project = home.join("project");
        std::fs::create_dir_all(&project).unwrap();
        crate::store::initialize_store(&home.join("coven.sqlite3")).unwrap();
        let conn = crate::store::open_store(&home.join("coven.sqlite3")).unwrap();
        let mut routine = definition("adopted-claim");
        routine.status = crate::automations::definition::RoutineStatus::Paused;
        routine.cwd = Some(project.to_string_lossy().into_owned());
        insert_definition(&conn, &routine).unwrap();
        let now = chrono::Utc::now();
        let now_iso = now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        conn.execute(
            "INSERT INTO automation_occurrences
                (id, automation_id, scheduled_for, state, attempt, created_at, updated_at)
             VALUES ('occ-adopted', 'adopted-claim', ?1, 'planned', 0, ?1, ?1)",
            rusqlite::params![now_iso],
        )
        .unwrap();
        crate::automations::occurrences::claim_occurrence_by_id(
            &conn,
            "occ-adopted",
            "prior-tick",
            30,
            now,
        )
        .unwrap()
        .unwrap();
        let record = crate::automations::store::get_definition(&conn, "adopted-claim")
            .unwrap()
            .unwrap();
        let snapshot = crate::automations::store::definition_snapshot(&record).unwrap();
        let deadline: String = conn
            .query_row(
                "SELECT deadline_at FROM automation_occurrences WHERE id = 'occ-adopted'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        crate::automations::runs::record_run_start_pinned(
            &conn,
            crate::automations::runs::PinnedRunStart {
                run_id: "run-adopted",
                automation_id: "adopted-claim",
                occurrence_id: "occ-adopted",
                session_id: Some("session-adopted"),
                familiar_id: None,
                runtime: "coven-code",
                snapshot: &snapshot,
                deadline_at: &deadline,
                now,
            },
        )
        .unwrap();
        drop(conn);

        process_automations_tick(home, &crate::api::NoopSessionRuntime).unwrap();

        let conn = crate::store::open_store(&home.join("coven.sqlite3")).unwrap();
        assert_eq!(
            super::super::runs::list_runs(&conn, "adopted-claim", 10)
                .unwrap()
                .len(),
            1,
            "an adopted dispatch must not spawn a second run"
        );
        let state: String = conn
            .query_row(
                "SELECT state FROM automation_occurrences WHERE id = 'occ-adopted'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(state, "running");
    }
}
