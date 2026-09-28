//! Request-boundary regression tests for #857. No live harness or credentials.

use std::path::Path;

use anyhow::Result;
use rusqlite::Connection;
use serde_json::{json, Value};

use crate::api::{
    handle_request_with_runtime_and_authority, ApiResponse, SessionLaunch, SessionRuntime,
};
use crate::request_authority::RequestAuthority;

// Keep this list independent of the production gate. New advertised actions
// are denied unless their read-only behavior has been deliberately reviewed.
const PUBLIC_READ_ACTIONS: &[&str] = &[
    "coven.automations.list",
    "coven.automations.get",
    "coven.automations.definition.list.v1",
    "coven.automations.definition.get.v1",
    "coven.automations.events.read.v1",
    "coven.automations.events.subscribe.v1",
    "coven.automations.runs",
    "coven.automations.run.history.v1",
    "coven.automations.health",
    "coven.automations.definition.health.v1",
    "coven.automations.scheduler.status.v1",
    "coven.automations.occurrence.list.v1",
    "coven.automations.occurrence.get.v1",
    "coven.automations.run.get.v1",
    "coven.automations.occurrence.history.v1",
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
