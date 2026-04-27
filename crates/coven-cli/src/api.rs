use std::path::Path;

use anyhow::{Context, Result};
use chrono::{SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::{daemon::DaemonStatus, store};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthResponse {
    pub ok: bool,
    pub daemon: Option<DaemonStatus>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiResponse {
    pub status: u16,
    pub content_type: &'static str,
    pub body: String,
}

pub trait SessionRuntime {
    fn send_input(&self, session_id: &str, payload: &Value) -> Result<()>;
    fn kill_session(&self, session_id: &str) -> Result<()>;
}

pub struct NoopSessionRuntime;

impl SessionRuntime for NoopSessionRuntime {
    fn send_input(&self, _session_id: &str, _payload: &Value) -> Result<()> {
        Ok(())
    }

    fn kill_session(&self, _session_id: &str) -> Result<()> {
        Ok(())
    }
}

pub fn health_response(daemon: Option<DaemonStatus>) -> HealthResponse {
    HealthResponse { ok: true, daemon }
}

#[allow(dead_code)]
pub fn handle_request(
    method: &str,
    path: &str,
    coven_home: &Path,
    daemon: Option<DaemonStatus>,
) -> Result<ApiResponse> {
    handle_request_with_body(method, path, coven_home, daemon, None)
}

pub fn handle_request_with_body(
    method: &str,
    path: &str,
    coven_home: &Path,
    daemon: Option<DaemonStatus>,
    body: Option<&str>,
) -> Result<ApiResponse> {
    handle_request_with_runtime(method, path, coven_home, daemon, body, &NoopSessionRuntime)
}

pub fn handle_request_with_runtime(
    method: &str,
    path: &str,
    coven_home: &Path,
    daemon: Option<DaemonStatus>,
    body: Option<&str>,
    runtime: &dyn SessionRuntime,
) -> Result<ApiResponse> {
    let (route, query) = split_path_query(path);
    match (method, route) {
        ("GET", "/health") => json_response(200, &health_response(daemon)),
        ("GET", "/sessions") => {
            let conn = store::open_store(&store_path(coven_home))?;
            let sessions = store::list_sessions(&conn)?;
            json_response(200, &sessions)
        }
        ("POST", path) if path.starts_with("/sessions/") && path.ends_with("/input") => {
            let session_id = session_action_id(path, "/input");
            record_input(coven_home, session_id, body, runtime)
        }
        ("POST", path) if path.starts_with("/sessions/") && path.ends_with("/kill") => {
            let session_id = session_action_id(path, "/kill");
            kill_session(coven_home, session_id, runtime)
        }
        ("GET", path) if path.starts_with("/sessions/") => {
            let session_id = path.trim_start_matches("/sessions/");
            let conn = store::open_store(&store_path(coven_home))?;
            match store::get_session(&conn, session_id)? {
                Some(session) => json_response(200, &session),
                None => json_response(404, &json!({ "error": "session not found" })),
            }
        }
        ("GET", "/events") => {
            let session_id = query_param(query.unwrap_or_default(), "sessionId");
            match session_id {
                Some(session_id) => list_session_events(coven_home, session_id),
                None => json_response(400, &json!({ "error": "sessionId is required" })),
            }
        }
        _ => json_response(404, &json!({ "error": "not found" })),
    }
}

fn store_path(coven_home: &Path) -> std::path::PathBuf {
    coven_home.join("coven.sqlite3")
}

fn record_input(
    coven_home: &Path,
    session_id: &str,
    body: Option<&str>,
    runtime: &dyn SessionRuntime,
) -> Result<ApiResponse> {
    let conn = store::open_store(&store_path(coven_home))?;
    if store::get_session(&conn, session_id)?.is_none() {
        return json_response(404, &json!({ "error": "session not found" }));
    }

    let payload = parse_body(body)?;
    runtime.send_input(session_id, &payload)?;
    insert_event(&conn, session_id, "input", payload)?;
    json_response(202, &json!({ "ok": true, "accepted": true }))
}

fn kill_session(
    coven_home: &Path,
    session_id: &str,
    runtime: &dyn SessionRuntime,
) -> Result<ApiResponse> {
    let conn = store::open_store(&store_path(coven_home))?;
    if store::get_session(&conn, session_id)?.is_none() {
        return json_response(404, &json!({ "error": "session not found" }));
    }

    runtime.kill_session(session_id)?;
    let now = current_timestamp();
    store::update_session_status(&conn, session_id, "killed", None, &now)?;
    insert_event(&conn, session_id, "kill", json!({ "status": "killed" }))?;
    json_response(202, &json!({ "ok": true, "accepted": true }))
}

fn list_session_events(coven_home: &Path, session_id: &str) -> Result<ApiResponse> {
    let conn = store::open_store(&store_path(coven_home))?;
    if store::get_session(&conn, session_id)?.is_none() {
        return json_response(404, &json!({ "error": "session not found" }));
    }
    let events = store::list_events(&conn, session_id)?;
    json_response(200, &events)
}

fn insert_event(
    conn: &rusqlite::Connection,
    session_id: &str,
    kind: &str,
    payload: Value,
) -> Result<()> {
    store::insert_event(
        conn,
        &store::EventRecord {
            id: Uuid::new_v4().to_string(),
            session_id: session_id.to_string(),
            kind: kind.to_string(),
            payload_json: serde_json::to_string(&payload)
                .context("failed to serialize event payload")?,
            created_at: current_timestamp(),
        },
    )
}

fn parse_body(body: Option<&str>) -> Result<Value> {
    match body.filter(|body| !body.trim().is_empty()) {
        Some(body) => serde_json::from_str(body).context("failed to parse request body"),
        None => Ok(json!({})),
    }
}

fn split_path_query(path: &str) -> (&str, Option<&str>) {
    match path.split_once('?') {
        Some((route, query)) => (route, Some(query)),
        None => (path, None),
    }
}

fn query_param<'a>(query: &'a str, key: &str) -> Option<&'a str> {
    query.split('&').find_map(|part| {
        let (candidate, value) = part.split_once('=')?;
        (candidate == key).then_some(value)
    })
}

fn session_action_id<'a>(path: &'a str, suffix: &str) -> &'a str {
    path.trim_start_matches("/sessions/")
        .strip_suffix(suffix)
        .unwrap_or_default()
}

fn current_timestamp() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
}

fn json_response<T: Serialize>(status: u16, body: &T) -> Result<ApiResponse> {
    Ok(ApiResponse {
        status,
        content_type: "application/json",
        body: serde_json::to_string(body).context("failed to serialize API response")?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_health_response() {
        let response = health_response(None);

        assert!(response.ok);
        assert_eq!(response.daemon, None);
    }

    #[test]
    fn routes_health_request_to_json() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let daemon = DaemonStatus {
            pid: 12345,
            started_at: "2026-04-27T10:00:00Z".to_string(),
            socket: temp_dir
                .path()
                .join("coven.sock")
                .to_string_lossy()
                .into_owned(),
        };

        let response = handle_request("GET", "/health", temp_dir.path(), Some(daemon))?;

        assert_eq!(response.status, 200);
        assert_eq!(response.content_type, "application/json");
        assert!(response.body.contains(r#""ok":true"#));
        assert!(response.body.contains(r#""pid":12345"#));
        Ok(())
    }

    #[test]
    fn routes_sessions_list_and_detail_requests_to_json() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let conn = crate::store::open_store(&temp_dir.path().join("coven.sqlite3"))?;
        let session = crate::store::SessionRecord {
            id: "session-1".to_string(),
            project_root: "/repo".to_string(),
            harness: "codex".to_string(),
            title: "hello from coven".to_string(),
            status: "created".to_string(),
            exit_code: None,
            created_at: "2026-04-27T10:00:00Z".to_string(),
            updated_at: "2026-04-27T10:00:00Z".to_string(),
        };
        crate::store::insert_session(&conn, &session)?;

        let list = handle_request("GET", "/sessions", temp_dir.path(), None)?;
        let detail = handle_request("GET", "/sessions/session-1", temp_dir.path(), None)?;

        assert_eq!(list.status, 200);
        assert!(list.body.contains(r#""id":"session-1""#));
        assert_eq!(detail.status, 200);
        assert!(detail.body.contains(r#""title":"hello from coven""#));
        Ok(())
    }

    #[test]
    fn returns_not_found_for_unknown_session() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let response = handle_request("GET", "/sessions/missing", temp_dir.path(), None)?;

        assert_eq!(response.status, 404);
        assert!(response.body.contains("session not found"));
        Ok(())
    }

    #[test]
    fn input_request_records_session_event() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        insert_test_session(temp_dir.path(), "session-1")?;

        let response = handle_request_with_body(
            "POST",
            "/sessions/session-1/input",
            temp_dir.path(),
            None,
            Some(r#"{"data":"hello coven"}"#),
        )?;
        let events = handle_request("GET", "/events?sessionId=session-1", temp_dir.path(), None)?;

        assert_eq!(response.status, 202);
        assert!(response.body.contains(r#""accepted":true"#));
        assert_eq!(events.status, 200);
        assert!(events.body.contains(r#""kind":"input""#));
        assert!(events.body.contains("hello coven"));
        Ok(())
    }

    #[test]
    fn input_request_invokes_live_runtime_hook() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        insert_test_session(temp_dir.path(), "session-1")?;
        let runtime = RecordingRuntime::default();

        let response = handle_request_with_runtime(
            "POST",
            "/sessions/session-1/input",
            temp_dir.path(),
            None,
            Some(r#"{"data":"hello coven"}"#),
            &runtime,
        )?;

        assert_eq!(response.status, 202);
        assert_eq!(
            runtime.inputs.borrow().as_slice(),
            &["session-1:hello coven"]
        );
        Ok(())
    }

    #[test]
    fn kill_request_marks_session_killed_and_records_event() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        insert_test_session(temp_dir.path(), "session-1")?;

        let response = handle_request("POST", "/sessions/session-1/kill", temp_dir.path(), None)?;
        let detail = handle_request("GET", "/sessions/session-1", temp_dir.path(), None)?;
        let events = handle_request("GET", "/events?sessionId=session-1", temp_dir.path(), None)?;

        assert_eq!(response.status, 202);
        assert!(detail.body.contains(r#""status":"killed""#));
        assert!(events.body.contains(r#""kind":"kill""#));
        Ok(())
    }

    #[test]
    fn kill_request_invokes_live_runtime_hook() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        insert_test_session(temp_dir.path(), "session-1")?;
        let runtime = RecordingRuntime::default();

        let response = handle_request_with_runtime(
            "POST",
            "/sessions/session-1/kill",
            temp_dir.path(),
            None,
            None,
            &runtime,
        )?;

        assert_eq!(response.status, 202);
        assert_eq!(runtime.kills.borrow().as_slice(), &["session-1"]);
        Ok(())
    }

    #[test]
    fn input_and_kill_reject_unknown_sessions() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;

        let input = handle_request_with_body(
            "POST",
            "/sessions/missing/input",
            temp_dir.path(),
            None,
            Some(r#"{"data":"hello"}"#),
        )?;
        let kill = handle_request("POST", "/sessions/missing/kill", temp_dir.path(), None)?;

        assert_eq!(input.status, 404);
        assert_eq!(kill.status, 404);
        assert!(input.body.contains("session not found"));
        assert!(kill.body.contains("session not found"));
        Ok(())
    }

    #[derive(Default)]
    struct RecordingRuntime {
        inputs: std::cell::RefCell<Vec<String>>,
        kills: std::cell::RefCell<Vec<String>>,
    }

    impl SessionRuntime for RecordingRuntime {
        fn send_input(&self, session_id: &str, payload: &Value) -> Result<()> {
            let data = payload
                .get("data")
                .and_then(Value::as_str)
                .unwrap_or_default();
            self.inputs
                .borrow_mut()
                .push(format!("{session_id}:{data}"));
            Ok(())
        }

        fn kill_session(&self, session_id: &str) -> Result<()> {
            self.kills.borrow_mut().push(session_id.to_string());
            Ok(())
        }
    }

    fn insert_test_session(coven_home: &std::path::Path, id: &str) -> anyhow::Result<()> {
        let conn = crate::store::open_store(&coven_home.join("coven.sqlite3"))?;
        let session = crate::store::SessionRecord {
            id: id.to_string(),
            project_root: "/repo".to_string(),
            harness: "codex".to_string(),
            title: "hello from coven".to_string(),
            status: "running".to_string(),
            exit_code: None,
            created_at: "2026-04-27T10:00:00Z".to_string(),
            updated_at: "2026-04-27T10:00:00Z".to_string(),
        };
        crate::store::insert_session(&conn, &session)?;
        Ok(())
    }
}
