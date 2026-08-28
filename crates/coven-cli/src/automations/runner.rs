//! Routine run dispatch (coven#816).
//!
//! A run is a claimed occurrence dispatched through the exact session-launch
//! path every other launch uses: the definition is re-read, the familiar and
//! runtime are re-validated per run, a SessionLaunch is built, and the
//! occurrence plus the run ledger settle together. Coven (not a harness home)
//! owns the record.

use std::time::{SystemTime, UNIX_EPOCH};

use chrono::{DateTime, Utc};
use rusqlite::Connection;

use super::definition::RoutineDefinition;
use super::occurrences::{settle_occurrence, PlanOutcome};
use super::runs::{record_run_finish, record_run_start, RunFinish};
use crate::api::{SessionLaunch, SessionRuntime};
use crate::harness::HarnessLaunchMode;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunOutcome {
    pub run_id: String,
    pub status: String,
    pub session_id: Option<String>,
    pub error: Option<String>,
}

fn fresh_id(prefix: &str) -> String {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0);
    format!("{prefix}-{millis}")
}

/// Runs a routine once, now: fences and claims an immediate occurrence,
/// records a ledger row, dispatches through the shared session-launch path,
/// and settles occurrence + ledger together. A missing cwd fails the run
/// with a recorded reason instead of guessing a project.
pub fn run_routine_now(
    conn: &Connection,
    runtime: &dyn SessionRuntime,
    definition: &RoutineDefinition,
    now: DateTime<Utc>,
) -> Result<RunOutcome, String> {
    let Some(cwd) = definition
        .cwd
        .as_deref()
        .map(str::trim)
        .filter(|cwd| !cwd.is_empty())
    else {
        return Ok(RunOutcome {
            run_id: String::new(),
            status: "failed".to_string(),
            session_id: None,
            error: Some("routine has no cwd; add a cwd before running".to_string()),
        });
    };

    let occurrence_id = fresh_id("occ");
    let now_iso = now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let inserted = conn
        .execute(
            "INSERT OR IGNORE INTO automation_occurrences
                (id, automation_id, scheduled_for, state, attempt, created_at, updated_at)
             VALUES (?1, ?2, ?3, 'planned', 0, ?3, ?3)",
            rusqlite::params![occurrence_id, definition.id, now_iso],
        )
        .map_err(|error| format!("failed to fence immediate occurrence: {error}"))?;
    if inserted == 0 {
        return Err("immediate occurrence fence collided; retry".to_string());
    }
    let _ = PlanOutcome::Planned; // keep the import meaningful if API changes

    let claimed =
        super::occurrences::claim_due_occurrence(conn, &definition.id, "manual", 60, now)?;
    let occurrence_id = claimed.unwrap_or(occurrence_id);

    let run_id = fresh_id("run");
    record_run_start(
        conn,
        &run_id,
        &definition.id,
        Some(&occurrence_id),
        definition.familiar_id.as_deref(),
        &definition.runtime,
        now,
    )
    .map_err(|error| format!("failed to record run start: {error:#}"))?;

    let launch = build_session_launch(definition, cwd)?;

    match runtime.launch_session(&launch) {
        Ok(()) => {
            let _ = settle_occurrence(conn, &occurrence_id, "succeeded", None, now);
            let _ = record_run_finish(
                conn,
                &run_id,
                RunFinish {
                    status: "succeeded",
                    exit_code: Some(0),
                    session_id: Some(launch.id.clone()),
                    log_json: None,
                    output_commit: None,
                },
                now,
            );
            Ok(RunOutcome {
                run_id,
                status: "succeeded".to_string(),
                session_id: Some(launch.id),
                error: None,
            })
        }
        Err(error) => {
            let reason = format!("{error:#}");
            let _ = settle_occurrence(conn, &occurrence_id, "failed", Some(&reason), now);
            let _ = record_run_finish(
                conn,
                &run_id,
                RunFinish {
                    status: "failed",
                    exit_code: None,
                    session_id: None,
                    log_json: None,
                    output_commit: None,
                },
                now,
            );
            Ok(RunOutcome {
                run_id,
                status: "failed".to_string(),
                session_id: None,
                error: Some(reason),
            })
        }
    }
}

/// Builds the shared SessionLaunch for a routine run. Every run — manual or
/// scheduled — dispatches through this exact launch shape.
pub fn build_session_launch(
    definition: &RoutineDefinition,
    cwd: &str,
) -> Result<SessionLaunch, String> {
    Ok(SessionLaunch {
        id: fresh_id("session"),
        project_root: cwd.to_string(),
        cwd: cwd.to_string(),
        harness: definition.runtime.clone(),
        model: definition.model.clone(),
        launch_mode: HarnessLaunchMode::NonInteractive,
        launch_policy: None,
        prompt: definition.prompt.clone(),
        title: definition.name.clone(),
        conversation: None,
        conversation_id: None,
        familiar_id: definition.familiar_id.clone(),
        caller_familiar_id: None,
    })
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct DispatchReport {
    pub dispatched: Vec<String>,
    pub failed: Vec<String>,
}

/// Dispatches every claimed occurrence that the scheduler has fenced: builds
/// the launch, records the ledger row, launches through the shared runtime
/// path, and settles occurrence + ledger together. Claimed occurrences whose
/// routine has no cwd fail with a recorded reason instead of guessing a
/// project.
pub fn dispatch_claimed_occurrences(
    conn: &Connection,
    runtime: &dyn SessionRuntime,
    now: DateTime<Utc>,
) -> Result<DispatchReport, String> {
    let mut report = DispatchReport::default();

    let claimed: Vec<(String, String)> = {
        let mut statement = conn
            .prepare(
                "SELECT id, automation_id FROM automation_occurrences
                 WHERE state = 'claimed' ORDER BY scheduled_for ASC",
            )
            .map_err(|error| format!("failed to list claimed occurrences: {error}"))?;
        let rows = statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .map_err(|error| format!("failed to list claimed occurrences: {error}"))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|error| format!("failed to read claim: {error}"))?);
        }
        out
    };

    for (occurrence_id, automation_id) in claimed {
        let Some(definition) = load_definition_for_run(conn, &automation_id)? else {
            let reason = format!("routine `{automation_id}` vanished during dispatch");
            let _ = settle_occurrence(conn, &occurrence_id, "failed", Some(&reason), now);
            report.failed.push(reason);
            continue;
        };

        let Some(cwd) = definition
            .cwd
            .as_deref()
            .map(str::trim)
            .filter(|cwd| !cwd.is_empty())
        else {
            let reason = format!("{automation_id}: routine has no cwd; add a cwd before running");
            let _ = settle_occurrence(conn, &occurrence_id, "failed", Some(&reason), now);
            report.failed.push(reason);
            continue;
        };

        let run_id = fresh_id("run");
        let started = record_run_start(
            conn,
            &run_id,
            &automation_id,
            Some(&occurrence_id),
            definition.familiar_id.as_deref(),
            &definition.runtime,
            now,
        );
        if let Err(error) = started {
            report.failed.push(format!("{automation_id}: {error:#}"));
            continue;
        }

        match build_session_launch(&definition, cwd).and_then(|launch| {
            runtime
                .launch_session(&launch)
                .map(|()| launch)
                .map_err(|error| format!("{error:#}"))
        }) {
            Ok(launch) => {
                let _ = settle_occurrence(conn, &occurrence_id, "succeeded", None, now);
                let _ = record_run_finish(
                    conn,
                    &run_id,
                    RunFinish {
                        status: "succeeded",
                        exit_code: Some(0),
                        session_id: Some(launch.id),
                        log_json: None,
                        output_commit: None,
                    },
                    now,
                );
                report.dispatched.push(run_id);
            }
            Err(reason) => {
                let _ = settle_occurrence(conn, &occurrence_id, "failed", Some(&reason), now);
                let _ = record_run_finish(
                    conn,
                    &run_id,
                    RunFinish {
                        status: "failed",
                        exit_code: None,
                        session_id: None,
                        log_json: None,
                        output_commit: None,
                    },
                    now,
                );
                report.failed.push(format!("{automation_id}: {reason}"));
            }
        }
    }

    Ok(report)
}

/// Reads and validates a stored definition for dispatch.
pub fn load_definition_for_run(
    conn: &Connection,
    id: &str,
) -> Result<Option<RoutineDefinition>, String> {
    let Some(record) =
        super::store::get_definition(conn, id).map_err(|error| format!("{error:#}"))?
    else {
        return Ok(None);
    };
    let definition: RoutineDefinition = serde_json::from_str(&record.definition_json)
        .map_err(|error| format!("stored routine `{id}` is unreadable: {error}"))?;
    Ok(Some(definition))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::automations::definition::RoutineDefinition;
    use crate::automations::store::insert_definition;
    use crate::store::initialize_store;
    use serde_json::json;

    struct RejectingRuntime;

    impl SessionRuntime for RejectingRuntime {
        fn launch_session(&self, _launch: &SessionLaunch) -> anyhow::Result<()> {
            anyhow::bail!("synthetic launch failure")
        }

        fn send_input(
            &self,
            _session_id: &str,
            _payload: &serde_json::Value,
        ) -> anyhow::Result<()> {
            Ok(())
        }

        fn kill_session(&self, _session_id: &str) -> anyhow::Result<()> {
            Ok(())
        }
    }

    fn definition(id: &str) -> RoutineDefinition {
        RoutineDefinition::from_json(&json!({
            "schemaVersion": 1,
            "id": id,
            "name": id,
            "status": "PAUSED",
            "rrule": "FREQ=DAILY;BYHOUR=9",
            "timezone": "utc",
            "misfire": "latest",
            "overlap": "forbid",
            "timeoutMinutes": 30,
            "runtime": "coven-code",
            "cwd": "/work/project",
            "familiarId": "charm",
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
    fn successful_run_settles_occurrence_and_ledger() {
        let (_temp, conn) = temp_store();
        insert_definition(&conn, &definition("daily")).unwrap();
        let outcome = run_routine_now(
            &conn,
            &crate::api::NoopSessionRuntime,
            &definition("daily"),
            Utc::now(),
        )
        .unwrap();
        assert_eq!(outcome.status, "succeeded");
        assert!(outcome.session_id.is_some());

        let runs = super::super::runs::list_runs(&conn, "daily", 10).unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].status, "succeeded");
        assert_eq!(runs[0].familiar_id.as_deref(), Some("charm"));

        let state: String = conn
            .query_row(
                "SELECT state FROM automation_occurrences WHERE automation_id = 'daily'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(state, "succeeded");
    }

    #[test]
    fn failed_launch_records_a_failed_run() {
        let (_temp, conn) = temp_store();
        insert_definition(&conn, &definition("daily")).unwrap();
        let outcome =
            run_routine_now(&conn, &RejectingRuntime, &definition("daily"), Utc::now()).unwrap();
        assert_eq!(outcome.status, "failed");
        assert!(outcome.error.as_deref().unwrap().contains("synthetic"));

        let runs = super::super::runs::list_runs(&conn, "daily", 10).unwrap();
        assert_eq!(runs[0].status, "failed");

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
    fn missing_cwd_fails_without_launching() {
        let (_temp, conn) = temp_store();
        let mut definition = definition("nocwd");
        definition.cwd = None;
        insert_definition(&conn, &definition).unwrap();

        let outcome = run_routine_now(
            &conn,
            &crate::api::NoopSessionRuntime,
            &definition,
            Utc::now(),
        )
        .unwrap();
        assert_eq!(outcome.status, "failed");
        assert!(outcome.error.as_deref().unwrap().contains("no cwd"));
    }
}
