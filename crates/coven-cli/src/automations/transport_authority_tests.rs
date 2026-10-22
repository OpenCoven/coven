//! Request-boundary regression tests for #857. No live harness or credentials.

use std::path::Path;

use anyhow::Result;
use rusqlite::Connection;
use serde_json::{json, Value};

use crate::api::{
    handle_request_with_runtime_and_authority, ApiResponse, SessionLaunch, SessionRuntime,
};
use crate::request_authority::RequestAuthority;

// Keep these lists independent of the production gate. New advertised actions
// are denied unless their read-only behavior has been deliberately reviewed.
const PUBLIC_READ_ACTIONS: &[&str] = &[
    "coven.automations.health",
    "coven.automations.definition.health.v1",
    "coven.automations.scheduler.status.v1",
    "coven.automations.occurrence.list.v1",
    "coven.automations.occurrence.history.v1",
];

/// Owner-local reads: sensitive records and minimized lease diagnostics.
const OWNER_LOCAL_READ_ACTIONS: &[&str] = &[
    "coven.automations.occurrence.leases.v1",
    "coven.automations.explain.v1",
    "coven.automations.list",
    "coven.automations.get",
    "coven.automations.definition.list.v1",
    "coven.automations.definition.get.v1",
    "coven.automations.runs",
    "coven.automations.run.history.v1",
    "coven.automations.run.get.v1",
    "coven.automations.occurrence.get.v1",
];

struct NoEffectsRuntime;

impl SessionRuntime for NoEffectsRuntime {
    fn launch_session(&self, _: &SessionLaunch) -> Result<()> {
        panic!("request must not launch a runtime")
    }

    fn send_input(&self, _: &str, _: &Value) -> Result<()> {
        panic!("request must not send runtime input")
    }

    fn kill_session(&self, _: &str) -> Result<()> {
        panic!("request must not cancel a runtime")
    }
}

fn request(
    home: &Path,
    route: &str,
    body: &Value,
    authority: RequestAuthority,
) -> Result<ApiResponse> {
    handle_request_with_runtime_and_authority(
        "POST",
        route,
        home,
        None,
        Some(&body.to_string()),
        &NoEffectsRuntime,
        authority,
    )
}

fn assert_authority_refusal(response: &ApiResponse, action: &str) -> Result<()> {
    assert_eq!(response.status, 403, "{action}: {}", response.body);
    let body: Value = serde_json::from_str(&response.body)?;
    assert_eq!(body["action"], action.trim());
    assert_eq!(body["ok"], false);
    assert_eq!(body["accepted"], false);
    assert_eq!(body["status"], "rejected");
    assert_eq!(body["error"]["code"], "AUTHORITY_REQUIRED");
    assert_eq!(body["error"]["httpStatus"], 403);
    assert_eq!(body["error"]["retryable"], false);
    assert!(body.get("event").is_none());
    assert!(body.get("result").is_none());
    assert!(body["error"].get("adoption").is_none());
    Ok(())
}

fn create_payload(key: &str) -> Value {
    json!({
        "action": "coven.automations.definition.create.v1",
        "adoptionKey": key,
        "origin": "owner-local-ipc",
        "intentId": "caller-asserted-not-authority",
        "principalId": "owner",
        "authority": "OwnerLocalIpc",
        "definition": {
            "schemaVersion": 1,
            "id": "authority-boundary-fixture",
            "name": "Transport authority fixture",
            "status": "PAUSED",
            "rrule": "FREQ=DAILY;BYHOUR=9",
            "timezone": "utc",
            "misfire": "latest",
            "overlap": "forbid",
            "timeoutMinutes": 30,
            "runtime": "coven-code",
            "prompt": "A paused test definition; never execute."
        }
    })
}

fn database_snapshot(conn: &Connection) -> Result<Vec<(String, Vec<String>)>> {
    let names = conn
        .prepare("SELECT name FROM sqlite_schema WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name")?
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut snapshot = Vec::new();
    for name in names {
        let sql = format!("SELECT * FROM \"{}\"", name.replace('"', "\"\""));
        let mut statement = conn.prepare(&sql)?;
        let columns = statement.column_count();
        let mut rows = statement
            .query_map([], |row| {
                let values = (0..columns)
                    .map(|index| row.get::<_, rusqlite::types::Value>(index))
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                Ok(format!("{values:?}"))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows.sort();
        snapshot.push((name, rows));
    }
    Ok(snapshot)
}

#[test]
fn mutations_are_refused_before_store_open_or_runtime_effects() -> Result<()> {
    let catalog = crate::control_plane::capabilities();
    let mut actions = catalog
        .capabilities
        .iter()
        .find(|capability| capability.id == "coven.automations")
        .expect("automation catalog")
        .actions
        .clone();
    // Unsupported/future commands must not become an authority bypass when
    // their implementation is added, even before the catalog is updated.
    actions.extend([
        "coven.automations.definition.futureMutation.v2",
        "coven.automations.legacy.import.v1",
        "coven.automations.occurrence.runNow.v1",
        "coven.automations.attempt.retry.v1",
        "coven.automations.occurrence.recover.v1",
    ]);
    for action in actions {
        if PUBLIC_READ_ACTIONS.contains(&action) {
            continue;
        }
        for route in ["/actions", "/api/v1/actions"] {
            let temp = tempfile::tempdir()?;
            let mut body = create_payload("transport-refusal-does-not-adopt");
            body["action"] = json!(format!("  {action}\n"));
            body["id"] = json!("missing");
            body["expectedRevision"] = json!(1);
            let response = request(temp.path(), route, &body, RequestAuthority::Tcp)?;
            assert_authority_refusal(&response, action)?;
            assert_eq!(std::fs::read_dir(temp.path())?.count(), 0, "{action}");
        }
    }
    Ok(())
}

#[test]
fn refusal_cannot_reserve_an_owners_adoption_key() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let body = create_payload("transport-retry-after-refusal");
    for _ in 0..2 {
        let response = request(temp.path(), "/actions", &body, RequestAuthority::Tcp)?;
        assert_authority_refusal(&response, body["action"].as_str().unwrap())?;
    }
    assert!(!temp.path().join("coven.sqlite3").exists());
    let owner = request(
        temp.path(),
        "/actions",
        &body,
        RequestAuthority::OwnerLocalIpc,
    )?;
    assert_eq!(owner.status, 200, "{}", owner.body);
    assert!(owner.body.contains(r#""outcome":"committed""#));
    Ok(())
}

#[test]
fn untrusted_transport_cannot_replay_or_mutate_an_owners_committed_definition() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let body = create_payload("transport-owner-committed-adoption");
    let owner = request(
        temp.path(),
        "/actions",
        &body,
        RequestAuthority::OwnerLocalIpc,
    )?;
    assert_eq!(owner.status, 200, "{}", owner.body);
    let conn = crate::store::open_store(&temp.path().join("coven.sqlite3"))?;
    let before = database_snapshot(&conn)?;
    for action in [
        "coven.automations.definition.create.v1",
        "coven.automations.definition.revise.v1",
        "coven.automations.definition.activate.v1",
        "coven.automations.definition.pause.v1",
        "coven.automations.definition.disable.v1",
        "coven.automations.definition.tombstone.v1",
        "coven.automations.create",
        "coven.automations.update",
        "coven.automations.delete",
    ] {
        let mut mutation = body.clone();
        mutation["action"] = json!(action);
        // Exact create replay reaches the existing adoption lookup on the
        // vulnerable implementation. Other commands carry fresh keys.
        if action != "coven.automations.definition.create.v1" {
            mutation["adoptionKey"] = json!(format!("transport-blocked:{action}"));
            mutation["id"] = body["definition"]["id"].clone();
            mutation["expectedRevision"] = json!(1);
        }
        let response = request(temp.path(), "/actions", &mutation, RequestAuthority::Tcp)?;
        assert_authority_refusal(&response, action)?;
        assert_eq!(database_snapshot(&conn)?, before, "{action}");
    }
    let replay = request(
        temp.path(),
        "/actions",
        &body,
        RequestAuthority::OwnerLocalIpc,
    )?;
    assert_eq!(replay.status, 200, "{}", replay.body);
    assert!(replay.body.contains(r#""outcome":"replayed""#));
    assert_eq!(database_snapshot(&conn)?, before);
    Ok(())
}

#[test]
fn transport_gate_preserves_read_and_non_automation_validation() -> Result<()> {
    for action in PUBLIC_READ_ACTIONS
        .iter()
        .copied()
        .chain(["coven.capabilities.refresh"])
    {
        let temp = tempfile::tempdir()?;
        let body = json!({"action": action});
        let owner = request(
            temp.path(),
            "/actions",
            &body,
            RequestAuthority::OwnerLocalIpc,
        )?;
        let tcp = request(temp.path(), "/actions", &body, RequestAuthority::Tcp)?;
        assert_ne!(tcp.status, 403, "{action}: {}", tcp.body);
        assert_eq!(tcp.status, owner.status, "{action}");
    }
    for body in [
        json!({}),
        json!({"action": 7}),
        json!({"action": " "}),
        json!([]),
    ] {
        let temp = tempfile::tempdir()?;
        let response = request(temp.path(), "/actions", &body, RequestAuthority::Tcp)?;
        assert_eq!(response.status, 400, "{}", response.body);
    }
    Ok(())
}

#[test]
fn event_reads_require_owner_ipc_because_they_issue_checkpoints() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let owner = request(
        temp.path(),
        "/actions",
        &create_payload("transport-events-fixture"),
        RequestAuthority::OwnerLocalIpc,
    )?;
    assert_eq!(owner.status, 200, "{}", owner.body);
    let conn = crate::store::open_store(&temp.path().join("coven.sqlite3"))?;
    let checkpoints = |conn: &Connection| -> Result<i64> {
        Ok(conn.query_row(
            "SELECT COUNT(*) FROM automation_event_checkpoints",
            [],
            |row| row.get(0),
        )?)
    };
    let stream = json!({"kind": "automation", "id": "authority-boundary-fixture"});
    for action in [
        "coven.automations.events.read.v1",
        "coven.automations.events.subscribe.v1",
    ] {
        let body = json!({"action": action, "stream": stream});
        let before = checkpoints(&conn)?;
        let tcp = request(temp.path(), "/actions", &body, RequestAuthority::Tcp)?;
        assert_authority_refusal(&tcp, action)?;
        assert_eq!(
            checkpoints(&conn)?,
            before,
            "{action} over TCP issued a checkpoint"
        );
        let local = request(
            temp.path(),
            "/actions",
            &body,
            RequestAuthority::OwnerLocalIpc,
        )?;
        assert_eq!(local.status, 200, "{action}: {}", local.body);
        assert_eq!(checkpoints(&conn)?, before + 1, "{action} over owner IPC");
    }
    Ok(())
}

#[test]
fn prompt_and_log_reads_require_owner_ipc() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let owner = request(
        temp.path(),
        "/actions",
        &create_payload("transport-sensitive-read-fixture"),
        RequestAuthority::OwnerLocalIpc,
    )?;
    assert_eq!(owner.status, 200, "{}", owner.body);
    for action in OWNER_LOCAL_READ_ACTIONS {
        let body = json!({
            "action": action,
            "id": "authority-boundary-fixture",
            "automationId": "authority-boundary-fixture",
        });
        for route in ["/actions", "/api/v1/actions"] {
            let tcp = request(temp.path(), route, &body, RequestAuthority::Tcp)?;
            assert_authority_refusal(&tcp, action)?;
            assert!(
                !tcp.body.contains("A paused test definition"),
                "{action} leaked the prompt over TCP"
            );
        }
        let local = request(
            temp.path(),
            "/actions",
            &body,
            RequestAuthority::OwnerLocalIpc,
        )?;
        assert_ne!(local.status, 403, "{action}: {}", local.body);
    }
    let local = request(
        temp.path(),
        "/actions",
        &json!({"action": "coven.automations.definition.get.v1", "id": "authority-boundary-fixture"}),
        RequestAuthority::OwnerLocalIpc,
    )?;
    assert!(
        local.body.contains("A paused test definition"),
        "{}",
        local.body
    );
    Ok(())
}

#[test]
fn run_control_envelopes_are_refused_before_tcp_store_access() -> Result<()> {
    for (command, payload) in [
        ("occurrence.runNow.v1", json!({"automationId": "notes"})),
        (
            "attempt.retry.v1",
            json!({"runId": "run-1", "priorAttemptNumber": 1, "priorDisposition": "failed"}),
        ),
        (
            "occurrence.recover.v1",
            json!({"occurrenceId": "occurrence-1", "evidenceDetermination": "failed_deterministic"}),
        ),
        (
            "occurrence.cancel.v1",
            json!({"occurrenceId": "occurrence-1"}),
        ),
        ("attempt.cancel.v1", json!({"attemptId": "attempt-1"})),
        ("run.cancel.v1", json!({"runId": "run-1"})),
    ] {
        for route in ["/actions", "/api/v1/actions"] {
            let temp = tempfile::tempdir()?;
            let body = json!({"action": "coven.automations.command.v1", "envelope": {
                "schemaVersion": "coven.automations.v1", "command": command,
                "adoptionKey": "adopt:tcp:run-control",
                "origin": {"principal": {"principalId": "principal:owner"}, "channel": "sdk"},
                "intent": {"statement": "Caller labels cannot authorize a mutation."}, "payload": payload
            }});
            let response = request(temp.path(), route, &body, RequestAuthority::Tcp)?;
            assert_authority_refusal(&response, "coven.automations.command.v1")?;
            assert_eq!(std::fs::read_dir(temp.path())?.count(), 0);
        }
    }
    Ok(())
}

#[test]
fn doctor_requires_owner_ipc_and_leaves_the_store_unchanged() -> Result<()> {
    let body = json!({
        "action": "coven.automations.doctor.v1",
        "principalId": "owner",
        "authority": "OwnerLocalIpc",
        "origin": "owner-local-ipc"
    });
    for route in ["/actions", "/api/v1/actions"] {
        let unopened = tempfile::tempdir()?;
        let response = request(unopened.path(), route, &body, RequestAuthority::Tcp)?;
        assert_authority_refusal(&response, "coven.automations.doctor.v1")?;
        assert_eq!(std::fs::read_dir(unopened.path())?.count(), 0);
    }
    let owner = tempfile::tempdir()?;
    let created = request(
        owner.path(),
        "/actions",
        &create_payload("doctor-owner-read"),
        RequestAuthority::OwnerLocalIpc,
    )?;
    assert_eq!(created.status, 200);
    let conn = crate::store::open_store(&owner.path().join("coven.sqlite3"))?;
    let before = database_snapshot(&conn)?;
    let response = request(
        owner.path(),
        "/api/v1/actions",
        &body,
        RequestAuthority::OwnerLocalIpc,
    )?;
    assert_eq!(response.status, 200);
    let value: Value = serde_json::from_str(&response.body)?;
    assert_eq!(
        value["result"]["schemaVersion"],
        "coven.automations.doctor.v1"
    );
    assert!(value.get("event").is_none());
    assert!(!response.body.contains("A paused test definition"));
    assert!(
        before == database_snapshot(&conn)?,
        "doctor changed durable state"
    );
    Ok(())
}

#[test]
fn doctor_uses_an_owned_read_connection_without_touching_caller_handlers() -> Result<()> {
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };
    let root = tempfile::tempdir()?;
    let conn = crate::store::open_store(&root.path().join("coven.sqlite3"))?;
    conn.busy_timeout(std::time::Duration::from_millis(1234))?;
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = Arc::clone(&calls);
    conn.progress_handler(
        1,
        Some(move || {
            observed.fetch_add(1, Ordering::Relaxed);
            false
        }),
    )?;
    let (status, response) = crate::control_plane::route_action(
        json!({"action":"coven.automations.doctor.v1"}),
        &conn,
        &NoEffectsRuntime,
    );
    assert_eq!(status, 200);
    assert!(response.ok);
    assert_eq!(
        calls.load(Ordering::Relaxed),
        0,
        "doctor SQL must run on its own connection"
    );
    let timeout: i64 = conn.query_row("PRAGMA busy_timeout", [], |row| row.get(0))?;
    assert_eq!(timeout, 1234, "doctor must not replace caller busy policy");
    assert!(
        calls.load(Ordering::Relaxed) > 0,
        "caller progress handler must remain installed"
    );
    Ok(())
}

#[cfg(unix)]
#[test]
fn doctor_preserves_existing_store_permissions() -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir()?;
    let path = root.path().join("coven.sqlite3");
    let conn = crate::store::open_store(&path)?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640))?;
    let (status, response) = crate::control_plane::route_action(
        json!({"action":"coven.automations.doctor.v1"}),
        &conn,
        &NoEffectsRuntime,
    );
    assert_eq!(status, 200);
    assert!(response.ok);
    assert_eq!(
        std::fs::metadata(&path)?.permissions().mode() & 0o777,
        0o640,
        "doctor must not repair filesystem permissions"
    );
    Ok(())
}

#[test]
fn doctor_reports_a_fixed_budget_failure_without_partial_results() -> Result<()> {
    let root = tempfile::tempdir()?;
    let conn = crate::store::open_store(&root.path().join("coven.sqlite3"))?;
    conn.execute_batch(
        "PRAGMA foreign_keys=OFF; DROP TABLE main.automation_occurrences;
        CREATE TABLE main.automation_occurrences(state,PRIVATE_payload);
        WITH RECURSIVE n(i) AS (VALUES(0) UNION ALL SELECT i+1 FROM n WHERE i<499999)
        INSERT INTO main.automation_occurrences SELECT 'planned','PRIVATE_VALUE' FROM n;",
    )?;
    let before = conn.total_changes();
    let (status, response) = crate::control_plane::route_action(
        json!({"action":"coven.automations.doctor.v1"}),
        &conn,
        &NoEffectsRuntime,
    );
    assert_eq!(status, 400);
    assert_eq!(
        response.reason.as_deref(),
        Some("Automation doctor read budget exceeded.")
    );
    assert!(response.result.is_none());
    assert!(!serde_json::to_string(&response)?.contains("PRIVATE"));
    assert_eq!(conn.total_changes(), before);
    assert_eq!(
        conn.query_row(
            "SELECT count(*) FROM main.automation_occurrences",
            [],
            |row| row.get::<_, i64>(0)
        )?,
        500000
    );
    Ok(())
}

#[test]
fn doctor_refuses_a_locked_store_without_changing_caller_transaction() -> Result<()> {
    let root = tempfile::tempdir()?;
    let conn = crate::store::open_store(&root.path().join("coven.sqlite3"))?;
    conn.execute_batch("PRAGMA journal_mode=DELETE; BEGIN EXCLUSIVE")?;
    let (status, response) = crate::control_plane::route_action(
        json!({"action":"coven.automations.doctor.v1"}),
        &conn,
        &NoEffectsRuntime,
    );
    assert_eq!(status, 400);
    assert_eq!(
        response.reason.as_deref(),
        Some("Automation doctor observation unavailable.")
    );
    assert!(
        !conn.is_autocommit(),
        "doctor must leave caller transaction alone"
    );
    conn.execute_batch("ROLLBACK")?;
    Ok(())
}
