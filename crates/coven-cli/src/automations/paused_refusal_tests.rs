//! A manual run of a routine that cannot be retried, refused at launch, is
//! settled at once instead of being held for a retry that is always refused
//! (coven#1793, owner decision option A).
use chrono::{DateTime, Duration, Utc};
use rusqlite::Connection;
use serde_json::{json, Value};

use super::definition::RoutineDefinition;
use super::runner::run_routine_now;
use super::store::insert_definition;
use crate::api::{SessionLaunch, SessionRuntime};

/// Refuses every launch before the runtime owns a session.
struct RefusingRuntime;

impl SessionRuntime for RefusingRuntime {
    fn launch_session(&self, _launch: &SessionLaunch) -> anyhow::Result<()> {
        anyhow::bail!("unknown familiar `check-familiar`; no familiars are configured")
    }
    fn send_input(&self, _: &str, _: &Value) -> anyhow::Result<()> {
        Ok(())
    }
    fn kill_session(&self, _: &str) -> anyhow::Result<()> {
        Ok(())
    }
}

/// A journal-like routine: manual runs only in practice, ten-minute timeout.
fn routine(status: &str) -> RoutineDefinition {
    RoutineDefinition::from_json(&json!({
        "schemaVersion": 1, "id": "journal", "name": "journal", "status": status,
        "rrule": "FREQ=DAILY;BYHOUR=9", "timezone": "utc", "misfire": "latest",
        "overlap": "forbid", "timeoutMinutes": 10, "runtime": "coven-code",
        "cwd": "/work/project", "familiarId": "check-familiar", "prompt": "Write the journal."
    }))
    .unwrap()
}

fn store() -> (tempfile::TempDir, Connection) {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("store.sqlite");
    crate::store::initialize_store(&path).unwrap();
    let conn = crate::store::open_store(&path).unwrap();
    (temp, conn)
}

/// The run's status, its occurrence's state, and its latest attempt's state
/// and failure class.
fn run_state(conn: &Connection, run_id: &str) -> (String, String, String, Option<String>) {
    conn.query_row(
        "SELECT r.status, o.state, a.state, a.failure_class
         FROM automation_runs r
         JOIN automation_occurrences o ON o.id = r.occurrence_id
         JOIN automation_attempts a ON a.run_id = r.id
         WHERE r.id = ?1
         ORDER BY a.attempt_number DESC LIMIT 1",
        [run_id],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
    )
    .unwrap()
}

fn retry(conn: &Connection, run_id: &str) -> (u16, Value) {
    let request = json!({
        "action": super::attempt_retry::RETRY_ACTION,
        "adoptionKey": format!("test-retry-{run_id}"),
        "runId": run_id,
        "priorAttemptNumber": 1,
        "priorDisposition": "failed",
    });
    let (status, response) =
        crate::control_plane::route_action(request, conn, &crate::api::NoopSessionRuntime);
    (status, serde_json::to_value(response).unwrap())
}

#[test]
fn paused_routine_refused_manual_run_settles_and_admits_the_next_manual_run() {
    let (_temp, conn) = store();
    let paused = routine("PAUSED");
    insert_definition(&conn, &paused).unwrap();
    let now: DateTime<Utc> = Utc::now();

    let refused = run_routine_now(&conn, &RefusingRuntime, &paused, now).unwrap();
    // The refused run is terminal at once, with its failure class kept.
    assert_eq!(
        run_state(&conn, &refused.run_id),
        (
            "failed".into(),
            "failed".into(),
            "failed".into(),
            Some("launch_refused".into())
        ),
        "{refused:?}"
    );
    assert_eq!(refused.status, "failed");

    // There is nothing held, so an operator retry is refused and changes
    // nothing, and the daemon's reason no longer names a held run.
    let (status, response) = retry(&conn, &refused.run_id);
    assert_eq!(status, 422, "{response}");
    assert_eq!(
        response["error"]["code"], "ILLEGAL_TRANSITION",
        "{response}"
    );

    // A second manual run, once the cause is fixed, is admitted without
    // waiting for the first run's deadline.
    let second = run_routine_now(
        &conn,
        &crate::api::NoopSessionRuntime,
        &paused,
        now + Duration::seconds(30),
    )
    .unwrap();
    assert_eq!(second.status, "running", "{second:?}");
    assert!(second.error.is_none(), "{second:?}");
    assert_ne!(second.run_id, refused.run_id);
}

#[test]
fn active_routine_refused_manual_run_is_still_held_for_a_retry() {
    let (_temp, conn) = store();
    let active = routine("ACTIVE");
    insert_definition(&conn, &active).unwrap();
    let now = Utc::now();

    let refused = run_routine_now(&conn, &RefusingRuntime, &active, now).unwrap();
    assert_eq!(refused.status, "running", "{refused:?}");
    assert_eq!(
        run_state(&conn, &refused.run_id),
        (
            "running".into(),
            "claimed".into(),
            "failed".into(),
            Some("launch_refused".into())
        )
    );
    // While it is held, a second manual run is refused for overlap ...
    let overlapping = run_routine_now(
        &conn,
        &crate::api::NoopSessionRuntime,
        &active,
        now + Duration::seconds(30),
    )
    .unwrap();
    assert_eq!(overlapping.status, "failed");
    assert!(overlapping
        .error
        .as_deref()
        .is_some_and(|error| error.contains("overlap is forbidden")));
    // ... and the operator retry the hold exists for is admitted.
    let (status, response) = retry(&conn, &refused.run_id);
    assert_eq!(status, 200, "{response}");
}
