//! The designated main session for a Coven instance (coven#1177, epic #1183).
//!
//! A main session is a *pointer*, not a session. It names the harness
//! conversation that Home (the single persistent chat on mobile and desktop)
//! resumes on every turn, and the `sessions` row that currently hosts that
//! conversation. The daemon owns the init-versus-resume decision through this
//! pointer so clients never have to carry a conversation id themselves.
//!
//! One pointer exists per scope key. v1 uses a single instance-wide scope
//! (`instance:main`); the key is stored so per-familiar scopes can be added
//! later without a migration.
//!
//! Lifecycle vocabulary:
//! - **resolve**: read the pointer, creating it with a fresh conversation id
//!   when absent. Idempotent.
//! - **bind**: record which `sessions` row currently hosts the conversation
//!   after a launch. Deleting that row clears the binding (FK `SET NULL`).
//! - **reset**: the user asked for a clean slate. Rotate the conversation id,
//!   drop the binding, count it.
//! - **rotate**: recovery from a conversation id the harness no longer knows.
//!   Same rotation, but not counted as a reset because the user did not ask.

use anyhow::{bail, Context, Result};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// The instance-wide scope every client resolves in v1.
pub const INSTANCE_MAIN_SCOPE_KEY: &str = "instance:main";
/// Decisions on epic #1183: Home runs on Claude stream mode and Nova fronts it.
pub const DEFAULT_MAIN_SESSION_HARNESS: &str = "claude";
pub const DEFAULT_MAIN_SESSION_FAMILIAR_ID: &str = "nova";

pub const MAIN_SESSIONS_SCHEMA_SQL: &str = "
    CREATE TABLE IF NOT EXISTS main_sessions (
        scope_key TEXT PRIMARY KEY NOT NULL,
        familiar_id TEXT,
        harness TEXT NOT NULL,
        project_root TEXT,
        conversation_id TEXT NOT NULL,
        current_session_id TEXT,
        reset_count INTEGER NOT NULL DEFAULT 0 CHECK (reset_count >= 0),
        created_at TEXT NOT NULL,
        updated_at TEXT NOT NULL,
        FOREIGN KEY (current_session_id) REFERENCES sessions(id) ON DELETE SET NULL
    );

    CREATE INDEX IF NOT EXISTS idx_main_sessions_current_session
        ON main_sessions(current_session_id);
";

const MAIN_SESSION_COLUMNS: &str = "scope_key, familiar_id, harness, project_root, \
     conversation_id, current_session_id, reset_count, created_at, updated_at";

/// Stores initialized before `project_root` existed gain the column here.
/// Idempotent; called from `initialize_store_schema` after the CREATE TABLE.
pub fn ensure_main_session_columns(conn: &Connection) -> Result<()> {
    let mut statement = conn
        .prepare("PRAGMA table_info(main_sessions)")
        .context("failed to inspect main_sessions schema")?;
    let has_project_root = statement
        .query_map([], |row| row.get::<_, String>(1))
        .context("failed to query main_sessions schema")?
        .collect::<std::result::Result<Vec<_>, _>>()
        .context("failed to read main_sessions schema")?
        .iter()
        .any(|column| column == "project_root");
    if !has_project_root {
        conn.execute("ALTER TABLE main_sessions ADD COLUMN project_root TEXT", [])
            .context("failed to add main_sessions.project_root column")?;
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MainSessionRecord {
    pub scope_key: String,
    pub familiar_id: Option<String>,
    pub harness: String,
    /// Project root every launch of this conversation uses. Fixed for the
    /// pointer's lifetime: a conversation resumed under a different root is
    /// a different conversation. `None` only for rows created before the
    /// column existed; the next reset repopulates it.
    pub project_root: Option<String>,
    /// The id handed to the harness for resume (`claude --resume`, `codex
    /// exec resume`). Rotated by reset and rotate; never edited in place.
    pub conversation_id: String,
    /// The `sessions` row currently hosting the conversation, if any. `None`
    /// before the first launch, after a reset/rotate, or once that row is
    /// deleted.
    pub current_session_id: Option<String>,
    /// Explicit user resets only. Stale-id rotations do not count.
    pub reset_count: i64,
    pub created_at: String,
    pub updated_at: String,
}

/// What a resolve did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedMainSession {
    pub record: MainSessionRecord,
    pub created: bool,
}

/// The outcome of a reset or rotate: the new pointer plus what it replaced,
/// so the caller can archive the previous session row and emit an event that
/// names both conversation ids.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RotatedMainSession {
    pub record: MainSessionRecord,
    pub previous_conversation_id: String,
    pub previous_session_id: Option<String>,
}

/// Scope keys are stored, logged, and later used in event payloads and route
/// paths; keep them to an unambiguous charset.
pub fn validate_scope_key(scope_key: &str) -> Result<()> {
    if scope_key.is_empty() || scope_key.len() > 128 {
        bail!("main session scope key must be 1..=128 characters");
    }
    if !scope_key
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, ':' | '-' | '_'))
    {
        bail!("main session scope key must contain only [a-z0-9:_-]");
    }
    Ok(())
}

fn new_conversation_id() -> String {
    // Must satisfy the launch route's conversation.id charset
    // (`[A-Za-z0-9._-]`), which a v4 UUID does.
    Uuid::new_v4().to_string()
}

fn record_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<MainSessionRecord> {
    Ok(MainSessionRecord {
        scope_key: row.get(0)?,
        familiar_id: row.get(1)?,
        harness: row.get(2)?,
        project_root: row.get(3)?,
        conversation_id: row.get(4)?,
        current_session_id: row.get(5)?,
        reset_count: row.get(6)?,
        created_at: row.get(7)?,
        updated_at: row.get(8)?,
    })
}

pub fn get_main_session(conn: &Connection, scope_key: &str) -> Result<Option<MainSessionRecord>> {
    conn.query_row(
        &format!("SELECT {MAIN_SESSION_COLUMNS} FROM main_sessions WHERE scope_key = ?1"),
        [scope_key],
        record_from_row,
    )
    .optional()
    .context("failed to read main session")
}

fn load_main_session(conn: &Connection, scope_key: &str) -> Result<MainSessionRecord> {
    get_main_session(conn, scope_key)?
        .with_context(|| format!("main session {scope_key} does not exist"))
}

/// Reads the pointer for `scope_key`, creating it when absent. `familiar_id`,
/// `harness`, and `project_root` are only consulted on creation: an existing
/// pointer keeps what it was created with, because the conversation id it
/// holds is meaningless to any other harness or root. Callers that want to
/// switch must reset first.
pub fn resolve_main_session(
    conn: &mut Connection,
    scope_key: &str,
    familiar_id: Option<&str>,
    harness: &str,
    project_root: &str,
    now: &str,
) -> Result<ResolvedMainSession> {
    validate_scope_key(scope_key)?;
    if harness.trim().is_empty() {
        bail!("main session harness must not be empty");
    }
    if project_root.trim().is_empty() {
        bail!("main session project root must not be empty");
    }
    let transaction = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .context("failed to begin main session resolve")?;
    if let Some(record) = get_main_session(&transaction, scope_key)? {
        transaction.commit()?;
        return Ok(ResolvedMainSession {
            record,
            created: false,
        });
    }
    transaction
        .execute(
            "INSERT INTO main_sessions (
                scope_key, familiar_id, harness, project_root, conversation_id,
                current_session_id, reset_count, created_at, updated_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, NULL, 0, ?6, ?6)",
            params![
                scope_key,
                familiar_id,
                harness,
                project_root,
                new_conversation_id(),
                now
            ],
        )
        .context("failed to create main session")?;
    let record = load_main_session(&transaction, scope_key)?;
    transaction.commit()?;
    Ok(ResolvedMainSession {
        record,
        created: true,
    })
}

/// Records the `sessions` row that now hosts the conversation. The row must
/// exist (FK), carry the pointer's conversation id, and run on the pointer's
/// harness, so a binding can never point Home at some other thread. The
/// harness check matters because conversation ids are harness-native: the
/// same string on a Codex row names a different conversation than on a
/// Claude row, and a follow-up turn would be routed to the wrong process.
pub fn bind_main_session(
    conn: &mut Connection,
    scope_key: &str,
    session_id: &str,
    now: &str,
) -> Result<MainSessionRecord> {
    let transaction = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .context("failed to begin main session bind")?;
    let record = load_main_session(&transaction, scope_key)?;
    let session: Option<(String, Option<String>)> = transaction
        .query_row(
            "SELECT harness, conversation_id FROM sessions WHERE id = ?1",
            [session_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .context("failed to read session for main session bind")?;
    match session {
        None => bail!("session {session_id} does not exist"),
        Some((_, conversation))
            if conversation.as_deref() != Some(record.conversation_id.as_str()) =>
        {
            bail!(
                "session {session_id} does not belong to main session conversation {}",
                record.conversation_id
            )
        }
        Some((harness, _)) if harness != record.harness => {
            bail!(
                "session {session_id} runs on harness {harness}, but main session {scope_key} is bound to {}",
                record.harness
            )
        }
        Some(_) => {}
    }
    transaction
        .execute(
            "UPDATE main_sessions
             SET current_session_id = ?2, updated_at = ?3
             WHERE scope_key = ?1",
            params![scope_key, session_id, now],
        )
        .context("failed to bind main session")?;
    let record = load_main_session(&transaction, scope_key)?;
    transaction.commit()?;
    Ok(record)
}

fn rotate(
    conn: &mut Connection,
    scope_key: &str,
    now: &str,
    count_as_reset: bool,
) -> Result<RotatedMainSession> {
    let transaction = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .context("failed to begin main session rotation")?;
    let previous = load_main_session(&transaction, scope_key)?;
    let reset_increment: i64 = if count_as_reset { 1 } else { 0 };
    transaction
        .execute(
            "UPDATE main_sessions
             SET conversation_id = ?2,
                 current_session_id = NULL,
                 reset_count = reset_count + ?3,
                 updated_at = ?4
             WHERE scope_key = ?1",
            params![scope_key, new_conversation_id(), reset_increment, now],
        )
        .context("failed to rotate main session conversation")?;
    let record = load_main_session(&transaction, scope_key)?;
    transaction.commit()?;
    Ok(RotatedMainSession {
        record,
        previous_conversation_id: previous.conversation_id,
        previous_session_id: previous.current_session_id,
    })
}

/// Removes a pointer outright. Used only to unwind a pointer whose very
/// first launch was refused, so a bad first turn leaves nothing behind; a
/// pointer with history is reset, never deleted.
pub fn delete_main_session(conn: &Connection, scope_key: &str) -> Result<bool> {
    let deleted = conn
        .execute(
            "DELETE FROM main_sessions WHERE scope_key = ?1",
            [scope_key],
        )
        .context("failed to delete main session")?;
    Ok(deleted > 0)
}

/// The user asked for a clean slate. Rotates the conversation id, clears the
/// binding, and counts the reset. Archiving the previous session row and
/// emitting the `main_session.reset` event are the caller's job, using the
/// returned `previous_*` fields.
pub fn reset_main_session(
    conn: &mut Connection,
    scope_key: &str,
    now: &str,
) -> Result<RotatedMainSession> {
    rotate(conn, scope_key, now, true)
}

/// Recovery from a conversation id the harness no longer recognises. Same
/// rotation as a reset, but not counted, because the user did not ask for it.
pub fn rotate_main_session_conversation(
    conn: &mut Connection,
    scope_key: &str,
    now: &str,
) -> Result<RotatedMainSession> {
    rotate(conn, scope_key, now, false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::initialize_store;

    const NOW: &str = "2026-09-28T12:00:00.000Z";
    const LATER: &str = "2026-09-28T12:05:00.000Z";

    fn temp_store() -> (tempfile::TempDir, Connection) {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("store.sqlite");
        initialize_store(&path).unwrap();
        let conn = crate::store::open_store(&path).unwrap();
        (temp, conn)
    }

    fn insert_session(conn: &Connection, id: &str, conversation_id: Option<&str>) {
        insert_session_on(conn, id, "claude", conversation_id);
    }

    fn insert_session_on(
        conn: &Connection,
        id: &str,
        harness: &str,
        conversation_id: Option<&str>,
    ) {
        conn.execute(
            "INSERT INTO sessions (
                id, project_root, harness, title, status, created_at, updated_at, conversation_id
            ) VALUES (?1, '/tmp/project', ?2, 'Home', 'running', ?3, ?3, ?4)",
            params![id, harness, NOW, conversation_id],
        )
        .unwrap();
    }

    fn assert_launchable_conversation_id(id: &str) {
        // Mirrors the charset the launch route enforces on conversation.id.
        assert!(
            id.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')),
            "{id}"
        );
    }

    #[test]
    fn scope_key_charset_is_enforced() {
        for ok in ["instance:main", "familiar:nova:main", "a-b_c9"] {
            validate_scope_key(ok).unwrap();
        }
        for bad in ["", "Instance:Main", "has space", "semi;colon", "slash/x"] {
            assert!(validate_scope_key(bad).is_err(), "{bad:?}");
        }
        let too_long = "a".repeat(129);
        assert!(validate_scope_key(&too_long).is_err());
    }

    #[test]
    fn resolve_creates_once_then_returns_the_same_pointer() {
        let (_temp, mut conn) = temp_store();
        assert!(get_main_session(&conn, INSTANCE_MAIN_SCOPE_KEY)
            .unwrap()
            .is_none());

        let first = resolve_main_session(
            &mut conn,
            INSTANCE_MAIN_SCOPE_KEY,
            Some("nova"),
            "claude",
            "/tmp/home",
            NOW,
        )
        .unwrap();
        assert!(first.created);
        assert_eq!(first.record.scope_key, INSTANCE_MAIN_SCOPE_KEY);
        assert_eq!(first.record.familiar_id.as_deref(), Some("nova"));
        assert_eq!(first.record.harness, "claude");
        assert_eq!(first.record.project_root.as_deref(), Some("/tmp/home"));
        assert_eq!(first.record.current_session_id, None);
        assert_eq!(first.record.reset_count, 0);
        assert_eq!(first.record.created_at, NOW);
        assert_launchable_conversation_id(&first.record.conversation_id);

        // A second resolve with different creation inputs does not overwrite:
        // the stored harness owns the conversation id.
        let second = resolve_main_session(
            &mut conn,
            INSTANCE_MAIN_SCOPE_KEY,
            Some("cody"),
            "codex",
            "/tmp/home",
            LATER,
        )
        .unwrap();
        assert!(!second.created);
        assert_eq!(second.record, first.record);
    }

    #[test]
    fn resolve_rejects_bad_inputs_without_creating() {
        let (_temp, mut conn) = temp_store();
        assert!(
            resolve_main_session(&mut conn, "Bad Key", None, "claude", "/tmp/home", NOW).is_err()
        );
        assert!(
            resolve_main_session(&mut conn, "instance:main", None, "  ", "/tmp/home", NOW).is_err()
        );
        assert!(get_main_session(&conn, "instance:main").unwrap().is_none());
    }

    #[test]
    fn scopes_are_independent() {
        let (_temp, mut conn) = temp_store();
        let a = resolve_main_session(
            &mut conn,
            "instance:main",
            Some("nova"),
            "claude",
            "/tmp/home",
            NOW,
        )
        .unwrap();
        let b = resolve_main_session(
            &mut conn,
            "familiar:cody:main",
            Some("cody"),
            "claude",
            "/tmp/home",
            NOW,
        )
        .unwrap();
        assert!(a.created && b.created);
        assert_ne!(a.record.conversation_id, b.record.conversation_id);
    }

    #[test]
    fn bind_requires_a_session_on_the_pointers_conversation() {
        let (_temp, mut conn) = temp_store();
        let resolved = resolve_main_session(
            &mut conn,
            INSTANCE_MAIN_SCOPE_KEY,
            None,
            "claude",
            "/tmp/home",
            NOW,
        )
        .unwrap();
        let conversation = resolved.record.conversation_id.clone();

        // Missing session row.
        assert!(bind_main_session(&mut conn, INSTANCE_MAIN_SCOPE_KEY, "missing", LATER).is_err());

        // A session on some other thread must never become Home.
        insert_session(&conn, "other", Some("some-other-conversation"));
        assert!(bind_main_session(&mut conn, INSTANCE_MAIN_SCOPE_KEY, "other", LATER).is_err());
        insert_session(&conn, "no-conversation", None);
        assert!(
            bind_main_session(&mut conn, INSTANCE_MAIN_SCOPE_KEY, "no-conversation", LATER)
                .is_err()
        );
        // The same conversation id on another harness names a different
        // conversation; it must not become Home either.
        insert_session_on(&conn, "codex-twin", "codex", Some(&conversation));
        let refused = bind_main_session(&mut conn, INSTANCE_MAIN_SCOPE_KEY, "codex-twin", LATER)
            .unwrap_err()
            .to_string();
        assert!(refused.contains("harness codex"), "{refused}");
        assert_eq!(
            get_main_session(&conn, INSTANCE_MAIN_SCOPE_KEY)
                .unwrap()
                .unwrap()
                .current_session_id,
            None
        );

        insert_session(&conn, "home-1", Some(&conversation));
        let bound = bind_main_session(&mut conn, INSTANCE_MAIN_SCOPE_KEY, "home-1", LATER).unwrap();
        assert_eq!(bound.current_session_id.as_deref(), Some("home-1"));
        assert_eq!(bound.updated_at, LATER);
        assert_eq!(bound.conversation_id, conversation);
    }

    #[test]
    fn deleting_the_bound_session_clears_the_binding_but_keeps_the_pointer() {
        let (_temp, mut conn) = temp_store();
        let resolved = resolve_main_session(
            &mut conn,
            INSTANCE_MAIN_SCOPE_KEY,
            None,
            "claude",
            "/tmp/home",
            NOW,
        )
        .unwrap();
        insert_session(&conn, "home-1", Some(&resolved.record.conversation_id));
        bind_main_session(&mut conn, INSTANCE_MAIN_SCOPE_KEY, "home-1", LATER).unwrap();

        conn.execute("DELETE FROM sessions WHERE id = 'home-1'", [])
            .unwrap();

        let record = get_main_session(&conn, INSTANCE_MAIN_SCOPE_KEY)
            .unwrap()
            .unwrap();
        assert_eq!(record.current_session_id, None);
        assert_eq!(record.conversation_id, resolved.record.conversation_id);
    }

    #[test]
    fn reset_rotates_counts_and_reports_what_it_replaced() {
        let (_temp, mut conn) = temp_store();
        let resolved = resolve_main_session(
            &mut conn,
            INSTANCE_MAIN_SCOPE_KEY,
            None,
            "claude",
            "/tmp/home",
            NOW,
        )
        .unwrap();
        let old_conversation = resolved.record.conversation_id.clone();
        insert_session(&conn, "home-1", Some(&old_conversation));
        bind_main_session(&mut conn, INSTANCE_MAIN_SCOPE_KEY, "home-1", NOW).unwrap();

        let reset = reset_main_session(&mut conn, INSTANCE_MAIN_SCOPE_KEY, LATER).unwrap();
        assert_eq!(reset.previous_conversation_id, old_conversation);
        assert_eq!(reset.previous_session_id.as_deref(), Some("home-1"));
        assert_ne!(reset.record.conversation_id, old_conversation);
        assert_launchable_conversation_id(&reset.record.conversation_id);
        assert_eq!(reset.record.current_session_id, None);
        assert_eq!(reset.record.reset_count, 1);
        assert_eq!(reset.record.updated_at, LATER);
        assert_eq!(reset.record.created_at, NOW);
        assert_eq!(reset.record.harness, "claude");

        // The old session row is untouched: archiving it is the caller's job.
        let status: String = conn
            .query_row(
                "SELECT status FROM sessions WHERE id = 'home-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(status, "running");

        // A stale binding can no longer be re-established on the old thread.
        assert!(bind_main_session(&mut conn, INSTANCE_MAIN_SCOPE_KEY, "home-1", LATER).is_err());
    }

    #[test]
    fn stale_id_rotation_is_not_counted_as_a_reset() {
        let (_temp, mut conn) = temp_store();
        let resolved = resolve_main_session(
            &mut conn,
            INSTANCE_MAIN_SCOPE_KEY,
            None,
            "claude",
            "/tmp/home",
            NOW,
        )
        .unwrap();
        let rotated =
            rotate_main_session_conversation(&mut conn, INSTANCE_MAIN_SCOPE_KEY, LATER).unwrap();
        assert_eq!(
            rotated.previous_conversation_id,
            resolved.record.conversation_id
        );
        assert_eq!(rotated.previous_session_id, None);
        assert_ne!(
            rotated.record.conversation_id,
            resolved.record.conversation_id
        );
        assert_eq!(rotated.record.reset_count, 0);

        reset_main_session(&mut conn, INSTANCE_MAIN_SCOPE_KEY, LATER).unwrap();
        reset_main_session(&mut conn, INSTANCE_MAIN_SCOPE_KEY, LATER).unwrap();
        assert_eq!(
            get_main_session(&conn, INSTANCE_MAIN_SCOPE_KEY)
                .unwrap()
                .unwrap()
                .reset_count,
            2
        );
    }

    #[test]
    fn delete_removes_only_the_named_pointer() {
        let (_temp, mut conn) = temp_store();
        resolve_main_session(&mut conn, "instance:main", None, "claude", "/tmp/home", NOW).unwrap();
        resolve_main_session(
            &mut conn,
            "familiar:cody:main",
            None,
            "claude",
            "/tmp/home",
            NOW,
        )
        .unwrap();
        assert!(delete_main_session(&conn, "instance:main").unwrap());
        assert!(!delete_main_session(&conn, "instance:main").unwrap());
        assert!(get_main_session(&conn, "instance:main").unwrap().is_none());
        assert!(get_main_session(&conn, "familiar:cody:main")
            .unwrap()
            .is_some());
    }

    #[test]
    fn rotation_of_a_missing_pointer_fails_closed() {
        let (_temp, mut conn) = temp_store();
        assert!(reset_main_session(&mut conn, INSTANCE_MAIN_SCOPE_KEY, NOW).is_err());
        assert!(rotate_main_session_conversation(&mut conn, INSTANCE_MAIN_SCOPE_KEY, NOW).is_err());
    }

    #[test]
    fn schema_survives_reopening_an_existing_store() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("store.sqlite");
        initialize_store(&path).unwrap();
        {
            let mut conn = crate::store::open_store(&path).unwrap();
            resolve_main_session(
                &mut conn,
                INSTANCE_MAIN_SCOPE_KEY,
                None,
                "claude",
                "/tmp/home",
                NOW,
            )
            .unwrap();
        }
        // Re-running initialization must be idempotent and keep the row.
        initialize_store(&path).unwrap();
        let conn = crate::store::open_store(&path).unwrap();
        assert!(get_main_session(&conn, INSTANCE_MAIN_SCOPE_KEY)
            .unwrap()
            .is_some());
    }

    #[test]
    fn legacy_pointer_table_gains_project_root_on_reopen() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("store.sqlite");
        initialize_store(&path).unwrap();
        {
            let conn = crate::store::open_store(&path).unwrap();
            conn.execute_batch(
                "DROP TABLE main_sessions;
                 CREATE TABLE main_sessions (
                    scope_key TEXT PRIMARY KEY NOT NULL,
                    familiar_id TEXT,
                    harness TEXT NOT NULL,
                    conversation_id TEXT NOT NULL,
                    current_session_id TEXT,
                    reset_count INTEGER NOT NULL DEFAULT 0,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL
                 );
                 INSERT INTO main_sessions VALUES ('instance:main', 'nova', 'claude', 'c', NULL, 0, 'a', 'a');",
            )
            .unwrap();
            ensure_main_session_columns(&conn).unwrap();
            ensure_main_session_columns(&conn).unwrap();
        }
        let conn = crate::store::open_store(&path).unwrap();
        let record = get_main_session(&conn, INSTANCE_MAIN_SCOPE_KEY)
            .unwrap()
            .unwrap();
        assert_eq!(record.project_root, None);
        assert_eq!(record.conversation_id, "c");
    }

    #[test]
    fn record_serializes_camel_case_for_the_api() {
        let record = MainSessionRecord {
            scope_key: "instance:main".into(),
            familiar_id: Some("nova".into()),
            harness: "claude".into(),
            project_root: Some("/tmp/home".into()),
            conversation_id: "c".into(),
            current_session_id: None,
            reset_count: 0,
            created_at: NOW.into(),
            updated_at: NOW.into(),
        };
        let json = serde_json::to_value(&record).unwrap();
        assert_eq!(json["scopeKey"], "instance:main");
        assert_eq!(json["projectRoot"], "/tmp/home");
        assert_eq!(json["conversationId"], "c");
        assert!(json["currentSessionId"].is_null());
        assert_eq!(json["resetCount"], 0);
    }
}
