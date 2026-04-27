use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::json;

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

pub fn health_response(daemon: Option<DaemonStatus>) -> HealthResponse {
    HealthResponse { ok: true, daemon }
}

pub fn handle_request(
    method: &str,
    path: &str,
    coven_home: &Path,
    daemon: Option<DaemonStatus>,
) -> Result<ApiResponse> {
    match (method, path) {
        ("GET", "/health") => json_response(200, &health_response(daemon)),
        ("GET", "/sessions") => {
            let conn = store::open_store(&store_path(coven_home))?;
            let sessions = store::list_sessions(&conn)?;
            json_response(200, &sessions)
        }
        ("GET", path) if path.starts_with("/sessions/") => {
            let session_id = path.trim_start_matches("/sessions/");
            let conn = store::open_store(&store_path(coven_home))?;
            match find_session(&conn, session_id)? {
                Some(session) => json_response(200, &session),
                None => json_response(404, &json!({ "error": "session not found" })),
            }
        }
        ("POST", path) if path.starts_with("/sessions/") && path.ends_with("/input") => {
            json_response(202, &json!({ "ok": true, "accepted": true }))
        }
        ("POST", path) if path.starts_with("/sessions/") && path.ends_with("/kill") => {
            json_response(202, &json!({ "ok": true, "accepted": true }))
        }
        ("GET", path) if path.starts_with("/events") => {
            json_response(200, &Vec::<serde_json::Value>::new())
        }
        _ => json_response(404, &json!({ "error": "not found" })),
    }
}

fn store_path(coven_home: &Path) -> std::path::PathBuf {
    coven_home.join("coven.sqlite3")
}

fn find_session(
    conn: &rusqlite::Connection,
    session_id: &str,
) -> Result<Option<store::SessionRecord>> {
    let sessions = store::list_sessions(conn)?;
    Ok(sessions
        .into_iter()
        .find(|session| session.id == session_id))
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
}
