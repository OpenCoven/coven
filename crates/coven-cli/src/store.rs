use std::path::Path;

use anyhow::{Context, Result};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionRecord {
    pub id: String,
    pub project_root: String,
    pub harness: String,
    pub title: String,
    pub status: String,
    pub exit_code: Option<i32>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventRecord {
    pub id: String,
    pub session_id: String,
    pub kind: String,
    pub payload_json: String,
    pub created_at: String,
}

pub fn open_store(path: &Path) -> Result<Connection> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create store directory {}", parent.display()))?;
    }

    let conn = Connection::open(path)
        .with_context(|| format!("failed to open Coven store at {}", path.display()))?;
    conn.execute_batch(
        "PRAGMA foreign_keys = ON;

        CREATE TABLE IF NOT EXISTS sessions (
            id TEXT PRIMARY KEY NOT NULL,
            project_root TEXT NOT NULL,
            harness TEXT NOT NULL,
            title TEXT NOT NULL,
            status TEXT NOT NULL,
            exit_code INTEGER,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS events (
            id TEXT PRIMARY KEY NOT NULL,
            session_id TEXT NOT NULL,
            kind TEXT NOT NULL,
            payload_json TEXT NOT NULL,
            created_at TEXT NOT NULL,
            FOREIGN KEY (session_id) REFERENCES sessions(id) ON DELETE CASCADE
        );

        CREATE INDEX IF NOT EXISTS idx_sessions_created_at
            ON sessions(created_at DESC);

        CREATE INDEX IF NOT EXISTS idx_events_session_created_at
            ON events(session_id, created_at);
        ",
    )
    .context("failed to initialize Coven store schema")?;
    ensure_exit_code_column(&conn)?;

    Ok(conn)
}

fn ensure_exit_code_column(conn: &Connection) -> Result<()> {
    let mut statement = conn
        .prepare("PRAGMA table_info(sessions)")
        .context("failed to inspect sessions schema")?;
    let has_exit_code = statement
        .query_map([], |row| row.get::<_, String>(1))
        .context("failed to query sessions schema")?
        .collect::<std::result::Result<Vec<_>, _>>()
        .context("failed to read sessions schema")?
        .into_iter()
        .any(|column| column == "exit_code");

    if !has_exit_code {
        conn.execute("ALTER TABLE sessions ADD COLUMN exit_code INTEGER", [])
            .context("failed to add sessions.exit_code column")?;
    }

    Ok(())
}

pub fn insert_session(conn: &Connection, record: &SessionRecord) -> Result<()> {
    conn.execute(
        "INSERT INTO sessions (
            id,
            project_root,
            harness,
            title,
            status,
            exit_code,
            created_at,
            updated_at
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            &record.id,
            &record.project_root,
            &record.harness,
            &record.title,
            &record.status,
            record.exit_code,
            &record.created_at,
            &record.updated_at,
        ],
    )
    .with_context(|| format!("failed to insert session {}", record.id))?;

    Ok(())
}

pub fn update_session_status(
    conn: &Connection,
    session_id: &str,
    status: &str,
    exit_code: Option<i32>,
    updated_at: &str,
) -> Result<()> {
    conn.execute(
        "UPDATE sessions
         SET status = ?2,
             exit_code = ?3,
             updated_at = ?4
         WHERE id = ?1",
        params![session_id, status, exit_code, updated_at],
    )
    .with_context(|| format!("failed to update session {session_id}"))?;

    Ok(())
}

pub fn get_session(conn: &Connection, session_id: &str) -> Result<Option<SessionRecord>> {
    Ok(list_sessions(conn)?
        .into_iter()
        .find(|session| session.id == session_id))
}

pub fn list_sessions(conn: &Connection) -> Result<Vec<SessionRecord>> {
    let mut statement = conn
        .prepare(
            "SELECT
                id,
                project_root,
                harness,
                title,
                status,
                exit_code,
                created_at,
                updated_at
            FROM sessions
            ORDER BY created_at DESC, id DESC",
        )
        .context("failed to prepare session list query")?;

    let sessions = statement
        .query_map([], |row| {
            Ok(SessionRecord {
                id: row.get(0)?,
                project_root: row.get(1)?,
                harness: row.get(2)?,
                title: row.get(3)?,
                status: row.get(4)?,
                exit_code: row.get(5)?,
                created_at: row.get(6)?,
                updated_at: row.get(7)?,
            })
        })
        .context("failed to query sessions")?
        .collect::<std::result::Result<Vec<_>, _>>()
        .context("failed to read sessions")?;

    Ok(sessions)
}

pub fn insert_event(conn: &Connection, record: &EventRecord) -> Result<()> {
    conn.execute(
        "INSERT INTO events (
            id,
            session_id,
            kind,
            payload_json,
            created_at
        ) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            &record.id,
            &record.session_id,
            &record.kind,
            &record.payload_json,
            &record.created_at,
        ],
    )
    .with_context(|| format!("failed to insert event {}", record.id))?;

    Ok(())
}

pub fn list_events(conn: &Connection, session_id: &str) -> Result<Vec<EventRecord>> {
    let mut statement = conn
        .prepare(
            "SELECT
                id,
                session_id,
                kind,
                payload_json,
                created_at
            FROM events
            WHERE session_id = ?1
            ORDER BY created_at ASC, id ASC",
        )
        .context("failed to prepare event list query")?;

    let events = statement
        .query_map(params![session_id], |row| {
            Ok(EventRecord {
                id: row.get(0)?,
                session_id: row.get(1)?,
                kind: row.get(2)?,
                payload_json: row.get(3)?,
                created_at: row.get(4)?,
            })
        })
        .context("failed to query events")?
        .collect::<std::result::Result<Vec<_>, _>>()
        .context("failed to read events")?;

    Ok(events)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inserts_and_lists_sessions() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let conn = open_store(&temp_dir.path().join("coven.db"))?;
        let session = session_record("session-1", "2026-04-27T06:00:00Z");

        insert_session(&conn, &session)?;

        assert_eq!(list_sessions(&conn)?, vec![session]);
        Ok(())
    }

    #[test]
    fn creates_schema_idempotently_by_opening_same_db_twice() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let path = temp_dir.path().join("coven.db");
        let first_conn = open_store(&path)?;
        insert_session(
            &first_conn,
            &session_record("session-1", "2026-04-27T06:00:00Z"),
        )?;
        drop(first_conn);

        let second_conn = open_store(&path)?;
        let sessions = list_sessions(&second_conn)?;

        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id, "session-1");
        Ok(())
    }

    #[test]
    fn lists_newest_sessions_first() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let conn = open_store(&temp_dir.path().join("coven.db"))?;
        let older = session_record("older", "2026-04-27T06:00:00Z");
        let newer = session_record("newer", "2026-04-27T07:00:00Z");

        insert_session(&conn, &older)?;
        insert_session(&conn, &newer)?;

        let ids = list_sessions(&conn)?
            .into_iter()
            .map(|session| session.id)
            .collect::<Vec<_>>();

        assert_eq!(ids, vec!["newer", "older"]);
        Ok(())
    }

    #[test]
    fn adds_exit_code_column_to_existing_store() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let path = temp_dir.path().join("coven.db");
        {
            let conn = Connection::open(&path)?;
            conn.execute_batch(
                "CREATE TABLE sessions (
                    id TEXT PRIMARY KEY NOT NULL,
                    project_root TEXT NOT NULL,
                    harness TEXT NOT NULL,
                    title TEXT NOT NULL,
                    status TEXT NOT NULL,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL
                );",
            )?;
        }

        let conn = open_store(&path)?;
        let session = session_record("session-1", "2026-04-27T06:00:00Z");
        insert_session(&conn, &session)?;
        update_session_status(
            &conn,
            "session-1",
            "completed",
            Some(0),
            "2026-04-27T06:01:00Z",
        )?;

        assert_eq!(list_sessions(&conn)?[0].exit_code, Some(0));
        Ok(())
    }

    #[test]
    fn updates_session_status_and_exit_code() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let conn = open_store(&temp_dir.path().join("coven.db"))?;
        let session = session_record("session-1", "2026-04-27T06:00:00Z");
        insert_session(&conn, &session)?;

        update_session_status(
            &conn,
            "session-1",
            "completed",
            Some(0),
            "2026-04-27T06:01:00Z",
        )?;

        let sessions = list_sessions(&conn)?;
        assert_eq!(sessions[0].status, "completed");
        assert_eq!(sessions[0].exit_code, Some(0));
        assert_eq!(sessions[0].updated_at, "2026-04-27T06:01:00Z");
        Ok(())
    }

    #[test]
    fn inserts_and_lists_events_for_session() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let conn = open_store(&temp_dir.path().join("coven.db"))?;
        insert_session(&conn, &session_record("session-1", "2026-04-27T06:00:00Z"))?;
        insert_event(
            &conn,
            &EventRecord {
                id: "event-1".to_string(),
                session_id: "session-1".to_string(),
                kind: "input".to_string(),
                payload_json: r#"{"data":"hello"}"#.to_string(),
                created_at: "2026-04-27T06:01:00Z".to_string(),
            },
        )?;

        let events = list_events(&conn, "session-1")?;

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, "input");
        assert_eq!(events[0].payload_json, r#"{"data":"hello"}"#);
        Ok(())
    }

    fn session_record(id: &str, created_at: &str) -> SessionRecord {
        SessionRecord {
            id: id.to_string(),
            project_root: "/tmp/coven-project".to_string(),
            harness: "codex".to_string(),
            title: format!("Session {id}"),
            status: "active".to_string(),
            exit_code: None,
            created_at: created_at.to_string(),
            updated_at: created_at.to_string(),
        }
    }
}
