use std::path::Path;

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OpenFlags};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionRecord {
    pub id: String,
    pub project_root: String,
    pub harness: String,
    pub title: String,
    pub status: String,
    pub exit_code: Option<i32>,
    pub archived_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    /// Optional grouping id so chat-style multi-turn conversations show as a
    /// single thread in `/sessions` instead of one row per turn. Distinct
    /// from `id` (which is per-session). In practice today this id is the
    /// same value the chat passes to the harness CLI for resume — claude
    /// uses a chat-generated UUID for both `--session-id` and grouping;
    /// codex uses its own captured `session id: <uuid>` for both `exec
    /// resume` and grouping. See `docs/chat-persistence.md`.
    #[serde(default)]
    pub conversation_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventRecord {
    pub seq: i64,
    pub id: String,
    pub session_id: String,
    pub kind: String,
    pub payload_json: String,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepositoryRecord {
    pub id: String,
    pub path: String,
    pub package_name: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Default)]
pub struct EventsQueryOptions {
    pub after_seq: Option<i64>,
    pub after_event_id: Option<String>,
    pub limit: Option<i64>,
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
            archived_at TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            conversation_id TEXT
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

        CREATE TABLE IF NOT EXISTS repositories (
            id TEXT PRIMARY KEY NOT NULL,
            path TEXT NOT NULL,
            package_name TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );
        ",
    )
    .context("failed to initialize Coven store schema")?;
    ensure_exit_code_column(&conn)?;
    ensure_archived_at_column(&conn)?;
    ensure_conversation_id_column(&conn)?;

    Ok(conn)
}

pub fn open_existing_store_read_only(path: &Path) -> Result<Option<Connection>> {
    if !path.exists() {
        return Ok(None);
    }

    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .with_context(|| format!("failed to open Coven store read-only at {}", path.display()))?;
    Ok(Some(conn))
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

fn ensure_archived_at_column(conn: &Connection) -> Result<()> {
    let mut statement = conn
        .prepare("PRAGMA table_info(sessions)")
        .context("failed to inspect sessions schema")?;
    let has_archived_at = statement
        .query_map([], |row| row.get::<_, String>(1))
        .context("failed to query sessions schema")?
        .collect::<std::result::Result<Vec<_>, _>>()
        .context("failed to read sessions schema")?
        .into_iter()
        .any(|column| column == "archived_at");

    if !has_archived_at {
        conn.execute("ALTER TABLE sessions ADD COLUMN archived_at TEXT", [])
            .context("failed to add sessions.archived_at column")?;
    }

    Ok(())
}

fn ensure_conversation_id_column(conn: &Connection) -> Result<()> {
    let mut statement = conn
        .prepare("PRAGMA table_info(sessions)")
        .context("failed to inspect sessions schema")?;
    let has_conversation_id = statement
        .query_map([], |row| row.get::<_, String>(1))
        .context("failed to query sessions schema")?
        .collect::<std::result::Result<Vec<_>, _>>()
        .context("failed to read sessions schema")?
        .into_iter()
        .any(|column| column == "conversation_id");

    if !has_conversation_id {
        conn.execute("ALTER TABLE sessions ADD COLUMN conversation_id TEXT", [])
            .context("failed to add sessions.conversation_id column")?;
    }
    // Idempotent — covers both the fresh-create path (column came from
    // the initial CREATE TABLE) and the migration path (column added just
    // above). Lives outside the if-block so existing stores opened by a
    // newer binary still get the index.
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_sessions_conversation_id
            ON sessions(conversation_id)",
        [],
    )
    .context("failed to create sessions.conversation_id index")?;

    Ok(())
}

pub fn upsert_repository(conn: &Connection, record: &RepositoryRecord) -> Result<()> {
    conn.execute(
        "INSERT INTO repositories (
            id,
            path,
            package_name,
            created_at,
            updated_at
        ) VALUES (?1, ?2, ?3, ?4, ?5)
        ON CONFLICT(id) DO UPDATE SET
            path = excluded.path,
            package_name = excluded.package_name,
            updated_at = excluded.updated_at",
        params![
            &record.id,
            &record.path,
            &record.package_name,
            &record.created_at,
            &record.updated_at,
        ],
    )
    .with_context(|| format!("failed to upsert repository {}", record.id))?;

    Ok(())
}

pub fn get_repository(conn: &Connection, id: &str) -> Result<Option<RepositoryRecord>> {
    use rusqlite::OptionalExtension;

    conn.query_row(
        "SELECT id, path, package_name, created_at, updated_at
         FROM repositories
         WHERE id = ?1
         LIMIT 1",
        params![id],
        |row| {
            Ok(RepositoryRecord {
                id: row.get(0)?,
                path: row.get(1)?,
                package_name: row.get(2)?,
                created_at: row.get(3)?,
                updated_at: row.get(4)?,
            })
        },
    )
    .optional()
    .with_context(|| format!("failed to get repository {id}"))
}

pub fn repositories_table_exists(conn: &Connection) -> Result<bool> {
    let exists = conn
        .query_row(
            "SELECT EXISTS(
                SELECT 1
                FROM sqlite_master
                WHERE type = 'table' AND name = 'repositories'
            )",
            [],
            |row| row.get::<_, bool>(0),
        )
        .context("failed to inspect repositories schema")?;

    Ok(exists)
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
            archived_at,
            created_at,
            updated_at,
            conversation_id
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            &record.id,
            &record.project_root,
            &record.harness,
            &record.title,
            &record.status,
            record.exit_code,
            &record.archived_at,
            &record.created_at,
            &record.updated_at,
            &record.conversation_id,
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

pub fn mark_running_sessions_orphaned(conn: &Connection, updated_at: &str) -> Result<usize> {
    let updated = conn
        .execute(
            "UPDATE sessions
             SET status = 'orphaned',
                 updated_at = ?1
             WHERE status = 'running'",
            params![updated_at],
        )
        .context("failed to mark running sessions orphaned")?;
    Ok(updated)
}

pub fn get_session(conn: &Connection, session_id: &str) -> Result<Option<SessionRecord>> {
    Ok(list_sessions_including_archived(conn)?
        .into_iter()
        .find(|session| session.id == session_id))
}

pub fn list_sessions(conn: &Connection) -> Result<Vec<SessionRecord>> {
    list_sessions_with_archive_filter(conn, false)
}

pub fn list_sessions_including_archived(conn: &Connection) -> Result<Vec<SessionRecord>> {
    list_sessions_with_archive_filter(conn, true)
}

fn list_sessions_with_archive_filter(
    conn: &Connection,
    include_archived: bool,
) -> Result<Vec<SessionRecord>> {
    let archive_filter = if include_archived {
        ""
    } else {
        "WHERE archived_at IS NULL"
    };
    let mut statement = conn
        .prepare(&format!(
            "SELECT
                id,
                project_root,
                harness,
                title,
                status,
                exit_code,
                archived_at,
                created_at,
                updated_at,
                conversation_id
            FROM sessions
            {archive_filter}
            ORDER BY created_at DESC, id DESC",
        ))
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
                archived_at: row.get(6)?,
                created_at: row.get(7)?,
                updated_at: row.get(8)?,
                conversation_id: row.get(9)?,
            })
        })
        .context("failed to query sessions")?
        .collect::<std::result::Result<Vec<_>, _>>()
        .context("failed to read sessions")?;

    Ok(sessions)
}

pub fn archive_session(conn: &Connection, session_id: &str, archived_at: &str) -> Result<()> {
    conn.execute(
        "UPDATE sessions
         SET archived_at = ?2,
             updated_at = ?2
         WHERE id = ?1",
        params![session_id, archived_at],
    )
    .with_context(|| format!("failed to archive session {session_id}"))?;

    Ok(())
}

pub fn summon_session(conn: &Connection, session_id: &str, updated_at: &str) -> Result<()> {
    conn.execute(
        "UPDATE sessions
         SET archived_at = NULL,
             updated_at = ?2
         WHERE id = ?1",
        params![session_id, updated_at],
    )
    .with_context(|| format!("failed to summon session {session_id}"))?;

    Ok(())
}

pub fn sacrifice_session(conn: &Connection, session_id: &str) -> Result<()> {
    conn.execute("DELETE FROM sessions WHERE id = ?1", params![session_id])
        .with_context(|| format!("failed to sacrifice session {session_id}"))?;

    Ok(())
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

pub fn insert_json_event(
    conn: &Connection,
    session_id: &str,
    kind: &str,
    payload: &serde_json::Value,
    created_at: &str,
) -> Result<()> {
    let record = EventRecord {
        // seq is populated by SQLite's rowid on insertion; 0 is a placeholder
        // that the INSERT statement ignores.
        seq: 0,
        id: uuid::Uuid::new_v4().to_string(),
        session_id: session_id.to_string(),
        kind: kind.to_string(),
        payload_json: payload.to_string(),
        created_at: created_at.to_string(),
    };
    insert_event(conn, &record)
}

pub fn list_events(conn: &Connection, session_id: &str) -> Result<Vec<EventRecord>> {
    list_events_with_options(conn, session_id, &EventsQueryOptions::default())
}

pub fn event_kind_exists(conn: &Connection, session_id: &str, kind: &str) -> Result<bool> {
    use rusqlite::OptionalExtension;

    let exists = conn
        .query_row(
            "SELECT 1 FROM events WHERE session_id = ?1 AND kind = ?2 LIMIT 1",
            params![session_id, kind],
            |_| Ok(()),
        )
        .optional()
        .context("failed to query event kind existence")?
        .is_some();
    Ok(exists)
}

pub fn list_events_with_options(
    conn: &Connection,
    session_id: &str,
    opts: &EventsQueryOptions,
) -> Result<Vec<EventRecord>> {
    use rusqlite::OptionalExtension;

    // Resolve the cursor to a rowid lower bound.
    let after_rowid: Option<i64> = if let Some(seq) = opts.after_seq {
        Some(seq)
    } else if let Some(ref event_id) = opts.after_event_id {
        conn.query_row(
            "SELECT rowid FROM events WHERE id = ?1 AND session_id = ?2 LIMIT 1",
            params![event_id, session_id],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .context("failed to resolve event cursor by event id")?
    } else {
        None
    };

    // The query is built dynamically based on which optional parameters are
    // present.  All user-provided values are bound via parameterized placeholders
    // (?1, ?2, ?3), so there is no SQL injection risk.
    let mut sql = String::from(
        "SELECT rowid AS seq, id, session_id, kind, payload_json, created_at
         FROM events WHERE session_id = ?1",
    );
    let has_cursor = after_rowid.is_some();
    if has_cursor {
        sql.push_str(" AND rowid > ?2");
    }
    sql.push_str(" ORDER BY rowid ASC");
    if opts.limit.is_some() {
        let pos = if has_cursor { "?3" } else { "?2" };
        sql.push_str(&format!(" LIMIT {pos}"));
    }

    let mut statement = conn
        .prepare(&sql)
        .context("failed to prepare event list query")?;

    let map_row = |row: &rusqlite::Row<'_>| {
        Ok(EventRecord {
            seq: row.get(0)?,
            id: row.get(1)?,
            session_id: row.get(2)?,
            kind: row.get(3)?,
            payload_json: row.get(4)?,
            created_at: row.get(5)?,
        })
    };

    let events = match (after_rowid, opts.limit) {
        (Some(after), Some(limit)) => statement
            .query_map(params![session_id, after, limit], map_row)
            .context("failed to query events")?,
        (Some(after), None) => statement
            .query_map(params![session_id, after], map_row)
            .context("failed to query events")?,
        (None, Some(limit)) => statement
            .query_map(params![session_id, limit], map_row)
            .context("failed to query events")?,
        (None, None) => statement
            .query_map(params![session_id], map_row)
            .context("failed to query events")?,
    }
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
    fn stores_and_retrieves_repository_locations() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let conn = open_store(&temp_dir.path().join("coven.db"))?;
        let repo = RepositoryRecord {
            id: "openclaw".to_string(),
            path: "/repo/openclaw".to_string(),
            package_name: Some("openclaw".to_string()),
            created_at: "2026-05-24T05:00:00Z".to_string(),
            updated_at: "2026-05-24T05:00:00Z".to_string(),
        };

        upsert_repository(&conn, &repo)?;

        assert_eq!(get_repository(&conn, "openclaw")?, Some(repo));
        Ok(())
    }

    #[test]
    fn repository_locations_are_updated_without_changing_created_at() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let conn = open_store(&temp_dir.path().join("coven.db"))?;
        upsert_repository(
            &conn,
            &RepositoryRecord {
                id: "openclaw".to_string(),
                path: "/old/openclaw".to_string(),
                package_name: Some("openclaw".to_string()),
                created_at: "2026-05-24T05:00:00Z".to_string(),
                updated_at: "2026-05-24T05:00:00Z".to_string(),
            },
        )?;

        upsert_repository(
            &conn,
            &RepositoryRecord {
                id: "openclaw".to_string(),
                path: "/new/openclaw".to_string(),
                package_name: Some("@openclaw/openclaw".to_string()),
                created_at: "2026-05-24T06:00:00Z".to_string(),
                updated_at: "2026-05-24T06:00:00Z".to_string(),
            },
        )?;

        assert_eq!(
            get_repository(&conn, "openclaw")?,
            Some(RepositoryRecord {
                id: "openclaw".to_string(),
                path: "/new/openclaw".to_string(),
                package_name: Some("@openclaw/openclaw".to_string()),
                created_at: "2026-05-24T05:00:00Z".to_string(),
                updated_at: "2026-05-24T06:00:00Z".to_string(),
            })
        );
        Ok(())
    }

    #[test]
    fn missing_store_does_not_open_read_only() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let store_path = temp_dir.path().join("missing.db");

        let conn = open_existing_store_read_only(&store_path)?;

        assert!(conn.is_none());
        assert!(!store_path.exists());
        Ok(())
    }

    #[test]
    fn repositories_table_exists_detects_missing_table() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let store_path = temp_dir.path().join("legacy.db");
        let conn = Connection::open(&store_path)?;
        conn.execute(
            "CREATE TABLE sessions (
                id TEXT PRIMARY KEY NOT NULL
            )",
            [],
        )?;
        drop(conn);

        let conn = open_existing_store_read_only(&store_path)?.expect("store should exist");

        assert!(!repositories_table_exists(&conn)?);
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
    fn marks_only_running_sessions_orphaned() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let conn = open_store(&temp_dir.path().join("coven.db"))?;
        let mut running = session_record("running", "2026-04-27T06:00:00Z");
        running.status = "running".to_string();
        let mut killed = session_record("killed", "2026-04-27T06:00:00Z");
        killed.status = "killed".to_string();
        insert_session(&conn, &running)?;
        insert_session(&conn, &killed)?;

        let updated = mark_running_sessions_orphaned(&conn, "2026-04-27T07:00:00Z")?;
        let sessions = list_sessions(&conn)?;

        assert_eq!(updated, 1);
        let running = sessions
            .iter()
            .find(|session| session.id == "running")
            .unwrap();
        let killed = sessions
            .iter()
            .find(|session| session.id == "killed")
            .unwrap();
        assert_eq!(running.status, "orphaned");
        assert_eq!(running.updated_at, "2026-04-27T07:00:00Z");
        assert_eq!(killed.status, "killed");
        Ok(())
    }

    #[test]
    fn archives_and_summons_sessions_without_losing_status() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let conn = open_store(&temp_dir.path().join("coven.db"))?;
        let session = session_record("session-1", "2026-04-27T06:00:00Z");
        insert_session(&conn, &session)?;

        archive_session(&conn, "session-1", "2026-04-27T07:00:00Z")?;

        assert!(list_sessions(&conn)?.is_empty());
        let archived = list_sessions_including_archived(&conn)?;
        assert_eq!(archived.len(), 1);
        assert_eq!(archived[0].status, "active");
        assert_eq!(
            archived[0].archived_at.as_deref(),
            Some("2026-04-27T07:00:00Z")
        );

        summon_session(&conn, "session-1", "2026-04-27T08:00:00Z")?;

        let active = list_sessions(&conn)?;
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].status, "active");
        assert_eq!(active[0].archived_at, None);
        assert_eq!(active[0].updated_at, "2026-04-27T08:00:00Z");
        Ok(())
    }

    #[test]
    fn sacrifices_session_and_cascades_events() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let conn = open_store(&temp_dir.path().join("coven.db"))?;
        insert_session(&conn, &session_record("session-1", "2026-04-27T06:00:00Z"))?;
        insert_json_event(
            &conn,
            "session-1",
            "output",
            &serde_json::json!({ "data": "hello" }),
            "2026-04-27T06:01:00Z",
        )?;

        sacrifice_session(&conn, "session-1")?;

        assert!(get_session(&conn, "session-1")?.is_none());
        assert!(list_events(&conn, "session-1")?.is_empty());
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
                seq: 0,
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

    #[test]
    fn inserts_json_event() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let conn = open_store(&temp_dir.path().join("coven.db"))?;
        let session = session_record("session-1", "2026-04-27T06:00:00Z");
        insert_session(&conn, &session)?;

        insert_json_event(
            &conn,
            "session-1",
            "patch_metadata",
            &serde_json::json!({"target":"openclaw"}),
            "2026-04-27T06:01:00Z",
        )?;

        let events = list_events(&conn, "session-1")?;
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, "patch_metadata");
        assert!(events[0].payload_json.contains("openclaw"));
        assert!(events[0].seq > 0);
        Ok(())
    }

    #[test]
    fn events_have_monotonic_seq_fields() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let conn = open_store(&temp_dir.path().join("coven.db"))?;
        insert_session(&conn, &session_record("session-1", "2026-04-27T06:00:00Z"))?;

        for i in 1..=3 {
            insert_json_event(
                &conn,
                "session-1",
                "output",
                &serde_json::json!({ "data": format!("line {i}") }),
                "2026-04-27T06:01:00Z",
            )?;
        }

        let events = list_events(&conn, "session-1")?;
        assert_eq!(events.len(), 3);
        assert!(events[0].seq > 0);
        assert!(events[1].seq > events[0].seq);
        assert!(events[2].seq > events[1].seq);
        Ok(())
    }

    #[test]
    fn list_events_with_after_seq_returns_tail() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let conn = open_store(&temp_dir.path().join("coven.db"))?;
        insert_session(&conn, &session_record("session-1", "2026-04-27T06:00:00Z"))?;

        for i in 1..=4 {
            insert_json_event(
                &conn,
                "session-1",
                "output",
                &serde_json::json!({ "data": format!("line {i}") }),
                "2026-04-27T06:01:00Z",
            )?;
        }

        let all = list_events(&conn, "session-1")?;
        let after_seq = all[1].seq;
        let tail = list_events_with_options(
            &conn,
            "session-1",
            &EventsQueryOptions {
                after_seq: Some(after_seq),
                ..Default::default()
            },
        )?;

        assert_eq!(tail.len(), 2);
        assert!(tail[0].seq > after_seq);
        Ok(())
    }

    #[test]
    fn event_kind_exists_detects_kind_without_loading_event_payloads() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let conn = open_store(&temp_dir.path().join("coven.db"))?;
        insert_session(&conn, &session_record("session-1", "2026-04-27T06:00:00Z"))?;
        insert_json_event(
            &conn,
            "session-1",
            "output",
            &serde_json::json!({ "data": "hello" }),
            "2026-04-27T06:01:00Z",
        )?;
        insert_json_event(
            &conn,
            "session-1",
            "cast.summary",
            &serde_json::json!({ "status": "completed", "exitCode": 0 }),
            "2026-04-27T06:02:00Z",
        )?;

        assert!(!event_kind_exists(&conn, "session-1", "input")?);
        assert!(event_kind_exists(&conn, "session-1", "cast.summary")?);
        Ok(())
    }

    #[test]
    fn list_events_with_after_event_id_returns_tail() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let conn = open_store(&temp_dir.path().join("coven.db"))?;
        insert_session(&conn, &session_record("session-1", "2026-04-27T06:00:00Z"))?;

        insert_event(
            &conn,
            &EventRecord {
                seq: 0,
                id: "event-a".to_string(),
                session_id: "session-1".to_string(),
                kind: "output".to_string(),
                payload_json: r#"{"data":"a"}"#.to_string(),
                created_at: "2026-04-27T06:01:00Z".to_string(),
            },
        )?;
        insert_event(
            &conn,
            &EventRecord {
                seq: 0,
                id: "event-b".to_string(),
                session_id: "session-1".to_string(),
                kind: "output".to_string(),
                payload_json: r#"{"data":"b"}"#.to_string(),
                created_at: "2026-04-27T06:02:00Z".to_string(),
            },
        )?;
        insert_event(
            &conn,
            &EventRecord {
                seq: 0,
                id: "event-c".to_string(),
                session_id: "session-1".to_string(),
                kind: "output".to_string(),
                payload_json: r#"{"data":"c"}"#.to_string(),
                created_at: "2026-04-27T06:03:00Z".to_string(),
            },
        )?;

        let tail = list_events_with_options(
            &conn,
            "session-1",
            &EventsQueryOptions {
                after_event_id: Some("event-a".to_string()),
                ..Default::default()
            },
        )?;

        assert_eq!(tail.len(), 2);
        assert_eq!(tail[0].id, "event-b");
        assert_eq!(tail[1].id, "event-c");
        Ok(())
    }

    #[test]
    fn list_events_with_limit_returns_at_most_n_events() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let conn = open_store(&temp_dir.path().join("coven.db"))?;
        insert_session(&conn, &session_record("session-1", "2026-04-27T06:00:00Z"))?;

        for i in 1..=5 {
            insert_json_event(
                &conn,
                "session-1",
                "output",
                &serde_json::json!({ "data": format!("line {i}") }),
                "2026-04-27T06:01:00Z",
            )?;
        }

        let limited = list_events_with_options(
            &conn,
            "session-1",
            &EventsQueryOptions {
                limit: Some(3),
                ..Default::default()
            },
        )?;

        assert_eq!(limited.len(), 3);
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
            archived_at: None,
            created_at: created_at.to_string(),
            updated_at: created_at.to_string(),
            conversation_id: None,
        }
    }
}
