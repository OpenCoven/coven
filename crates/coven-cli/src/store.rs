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
    pub created_at: String,
    pub updated_at: String,
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

    Ok(conn)
}

pub fn insert_session(conn: &Connection, record: &SessionRecord) -> Result<()> {
    conn.execute(
        "INSERT INTO sessions (
            id,
            project_root,
            harness,
            title,
            status,
            created_at,
            updated_at
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            &record.id,
            &record.project_root,
            &record.harness,
            &record.title,
            &record.status,
            &record.created_at,
            &record.updated_at,
        ],
    )
    .with_context(|| format!("failed to insert session {}", record.id))?;

    Ok(())
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
                created_at: row.get(5)?,
                updated_at: row.get(6)?,
            })
        })
        .context("failed to query sessions")?
        .collect::<std::result::Result<Vec<_>, _>>()
        .context("failed to read sessions")?;

    Ok(sessions)
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

    fn session_record(id: &str, created_at: &str) -> SessionRecord {
        SessionRecord {
            id: id.to_string(),
            project_root: "/tmp/coven-project".to_string(),
            harness: "codex".to_string(),
            title: format!("Session {id}"),
            status: "active".to_string(),
            created_at: created_at.to_string(),
            updated_at: created_at.to_string(),
        }
    }
}
