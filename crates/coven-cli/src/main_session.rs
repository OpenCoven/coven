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

use anyhow::{bail, ensure, Context, Result};
use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};
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
    /// conversation's lifetime: a conversation resumed under a different
    /// root is a different conversation, so only a reset (which mints a new
    /// conversation id) may change it. `None` only for rows created before
    /// the column existed; a reset that carries a project root repopulates
    /// it.
    pub project_root: Option<String>,
    /// The id handed to the harness for resume (`claude --resume`, `codex
    /// exec resume`). Init adopts the harness-native ID; reset and rotate replace it.
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

/// Replacement settings a reset may carry; `None` keeps the current value.
/// Only a reset may change them, because it also mints a fresh conversation
/// id: no harness-native history is ever carried to another harness,
/// familiar, or root.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MainSessionSettings<'a> {
    pub familiar_id: Option<&'a str>,
    pub harness: Option<&'a str>,
    pub project_root: Option<&'a str>,
}

impl MainSessionSettings<'_> {
    fn validate(&self) -> Result<()> {
        for (field, value) in [
            ("familiar id", self.familiar_id),
            ("harness", self.harness),
            ("project root", self.project_root),
        ] {
            if value.is_some_and(|value| value.trim().is_empty()) {
                bail!("main session {field} must not be empty");
            }
        }
        Ok(())
    }
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

/// What a stale-id rotation did. The caller names the conversation id that
/// actually failed, and the pointer is rotated only if it still holds that
/// id: with concurrent clients, or a reset racing a delayed stale response,
/// rotating "whatever is current" would invalidate a conversation that was
/// just created and works.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StaleIdRotation {
    /// The pointer still held the failed id and now holds a fresh one.
    Rotated(RotatedMainSession),
    /// The pointer no longer holds the failed id; a reset or another
    /// rollover already replaced it. Nothing changed. Carries the current
    /// pointer so the caller can simply retry its turn.
    AlreadyRotated(MainSessionRecord),
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
#[cfg(test)]
pub fn bind_main_session(
    conn: &mut Connection,
    scope_key: &str,
    session_id: &str,
    now: &str,
) -> Result<MainSessionRecord> {
    let expected = load_main_session(conn, scope_key)?;
    bind_main_session_with_native_id(conn, &expected, session_id, &expected.conversation_id, now)
}

/// Atomically adopts a harness-native UUID and binds the launched session.
/// The expected pointer prevents late output from overwriting a reset or a
/// newer binding, even when that binding is on the same conversation.
pub(crate) fn bind_main_session_with_native_id(
    conn: &mut Connection,
    expected: &MainSessionRecord,
    session_id: &str,
    native_id: &str,
    now: &str,
) -> Result<MainSessionRecord> {
    Uuid::parse_str(native_id).context("harness conversation id must be a UUID")?;
    let scope_key = expected.scope_key.as_str();
    let transaction = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .context("failed to begin main session bind")?;
    let record = load_main_session(&transaction, scope_key)?;
    ensure!(
        &record == expected,
        "main session changed while the harness was launching"
    );
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
            "UPDATE sessions SET conversation_id = ?2, updated_at = ?3 WHERE id = ?1",
            params![session_id, native_id, now],
        )
        .context("failed to adopt harness conversation id")?;
    transaction
        .execute(
            "UPDATE main_sessions
             SET current_session_id = ?2, updated_at = ?3, conversation_id = ?4
             WHERE scope_key = ?1",
            params![scope_key, session_id, now, native_id],
        )
        .context("failed to bind main session")?;
    let record = load_main_session(&transaction, scope_key)?;
    transaction.commit()?;
    Ok(record)
}

pub(crate) fn begin_rotation(conn: &mut Connection) -> Result<Transaction<'_>> {
    conn.transaction_with_behavior(TransactionBehavior::Immediate)
        .context("failed to begin main session rotation")
}

/// Rotates from a pointer the caller loaded inside `transaction`, so the
/// decision to rotate and the rotation itself see the same snapshot. The
/// UPDATE is conditioned on that snapshot's conversation id as well; under
/// the IMMEDIATE transaction it cannot miss, and the check keeps it that way.
/// `settings` are applied in the same statement (a reset's replacements;
/// empty for a stale-id rotation).
fn rotate_in(
    transaction: &Transaction<'_>,
    previous: MainSessionRecord,
    settings: &MainSessionSettings<'_>,
    now: &str,
    count_as_reset: bool,
) -> Result<RotatedMainSession> {
    let reset_increment: i64 = if count_as_reset { 1 } else { 0 };
    let updated = transaction
        .execute(
            "UPDATE main_sessions
             SET conversation_id = ?2,
                 current_session_id = NULL,
                 reset_count = reset_count + ?3,
                 updated_at = ?4,
                 familiar_id = COALESCE(?5, familiar_id),
                 harness = COALESCE(?6, harness),
                 project_root = COALESCE(?7, project_root)
             WHERE scope_key = ?1 AND conversation_id = ?8",
            params![
                previous.scope_key,
                new_conversation_id(),
                reset_increment,
                now,
                settings.familiar_id,
                settings.harness,
                settings.project_root,
                previous.conversation_id
            ],
        )
        .context("failed to rotate main session conversation")?;
    ensure!(
        updated == 1,
        "main session {} changed during rotation",
        previous.scope_key
    );
    let record = load_main_session(transaction, &previous.scope_key)?;
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
/// binding, counts the reset, and applies any replacement `settings` in the
/// same transaction, so the new conversation starts on them. Archiving the previous session row and
/// emitting the `main_session.reset` event are the caller's job, using the
/// returned `previous_*` fields.
pub fn reset_main_session_in(
    transaction: &Transaction<'_>,
    scope_key: &str,
    settings: &MainSessionSettings<'_>,
    now: &str,
) -> Result<RotatedMainSession> {
    settings.validate()?;
    let previous = load_main_session(transaction, scope_key)?;
    rotate_in(transaction, previous, settings, now, true)
}

#[cfg(test)]
pub fn reset_main_session(
    conn: &mut Connection,
    scope_key: &str,
    settings: &MainSessionSettings<'_>,
    now: &str,
) -> Result<RotatedMainSession> {
    let transaction = begin_rotation(conn)?;
    let rotated = reset_main_session_in(&transaction, scope_key, settings, now)?;
    transaction.commit()?;
    Ok(rotated)
}

/// Recovery from a conversation id the harness no longer recognises. Same
/// rotation as a reset, but not counted, because the user did not ask for
/// it, and compare-and-swap on `failed_conversation_id`: see
/// [`StaleIdRotation`]. Settings are kept: recovery is not a request to
/// change them.
pub fn rotate_main_session_conversation_in(
    transaction: &Transaction<'_>,
    scope_key: &str,
    failed_conversation_id: &str,
    now: &str,
) -> Result<StaleIdRotation> {
    let previous = load_main_session(transaction, scope_key)?;
    if previous.conversation_id != failed_conversation_id {
        return Ok(StaleIdRotation::AlreadyRotated(previous));
    }
    let rotated = rotate_in(
        transaction,
        previous,
        &MainSessionSettings::default(),
        now,
        false,
    )?;
    Ok(StaleIdRotation::Rotated(rotated))
}

pub(crate) fn rotate_main_session_generation_in(
    transaction: &Transaction<'_>,
    expected: &MainSessionRecord,
    now: &str,
) -> Result<StaleIdRotation> {
    let current = load_main_session(transaction, &expected.scope_key)?;
    if &current != expected {
        return Ok(StaleIdRotation::AlreadyRotated(current));
    }
    Ok(StaleIdRotation::Rotated(rotate_in(
        transaction,
        current,
        &MainSessionSettings::default(),
        now,
        false,
    )?))
}

#[cfg(test)]
pub fn rotate_main_session_conversation(
    conn: &mut Connection,
    scope_key: &str,
    failed_conversation_id: &str,
    now: &str,
) -> Result<StaleIdRotation> {
    let transaction = begin_rotation(conn)?;
    let rotated =
        rotate_main_session_conversation_in(&transaction, scope_key, failed_conversation_id, now)?;
    transaction.commit()?;
    Ok(rotated)
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
    fn automatic_rotation_refuses_new_binding_on_same_conversation() {
        let (_temp, mut conn) = temp_store();
        let original = resolve_main_session(
            &mut conn,
            INSTANCE_MAIN_SCOPE_KEY,
            None,
            "claude",
            "/tmp/home",
            NOW,
        )
        .unwrap()
        .record;
        insert_session(&conn, "rejected", Some(&original.conversation_id));
        let expected =
            bind_main_session(&mut conn, INSTANCE_MAIN_SCOPE_KEY, "rejected", NOW).unwrap();
        insert_session(&conn, "newer", Some(&original.conversation_id));
        let newer = bind_main_session(&mut conn, INSTANCE_MAIN_SCOPE_KEY, "newer", LATER).unwrap();
        let transaction = begin_rotation(&mut conn).unwrap();
        let outcome = rotate_main_session_generation_in(&transaction, &expected, LATER).unwrap();
        assert_eq!(outcome, StaleIdRotation::AlreadyRotated(newer.clone()));
        transaction.commit().unwrap();
        assert_eq!(
            get_main_session(&conn, INSTANCE_MAIN_SCOPE_KEY)
                .unwrap()
                .unwrap(),
            newer
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
    fn native_id_binding_updates_pointer_and_session_atomically() {
        let (_temp, mut conn) = temp_store();
        let expected = resolve_main_session(
            &mut conn,
            INSTANCE_MAIN_SCOPE_KEY,
            None,
            "codex",
            "/tmp/home",
            NOW,
        )
        .unwrap()
        .record;
        insert_session_on(
            &conn,
            "native-session",
            "codex",
            Some(&expected.conversation_id),
        );
        let native = Uuid::new_v4().to_string();
        let bound = bind_main_session_with_native_id(
            &mut conn,
            &expected,
            "native-session",
            &native,
            LATER,
        )
        .unwrap();
        assert_eq!(bound.conversation_id, native);
        assert_eq!(bound.current_session_id.as_deref(), Some("native-session"));
        assert_eq!(
            crate::store::get_session(&conn, "native-session")
                .unwrap()
                .unwrap()
                .conversation_id
                .as_deref(),
            Some(native.as_str())
        );
        assert_eq!(
            get_main_session(&conn, INSTANCE_MAIN_SCOPE_KEY)
                .unwrap()
                .unwrap(),
            bound
        );
    }

    #[test]
    fn native_id_binding_failure_preserves_both_identities() {
        let (_temp, mut conn) = temp_store();
        let expected = resolve_main_session(
            &mut conn,
            INSTANCE_MAIN_SCOPE_KEY,
            None,
            "codex",
            "/tmp/home",
            NOW,
        )
        .unwrap()
        .record;
        insert_session_on(
            &conn,
            "native-session",
            "codex",
            Some(&expected.conversation_id),
        );
        conn.execute_batch("CREATE TRIGGER refuse_native_bind BEFORE UPDATE ON main_sessions BEGIN SELECT RAISE(ABORT, 'injected bind failure'); END;").unwrap();
        assert!(bind_main_session_with_native_id(
            &mut conn,
            &expected,
            "native-session",
            &Uuid::new_v4().to_string(),
            LATER
        )
        .is_err());
        assert_eq!(
            get_main_session(&conn, INSTANCE_MAIN_SCOPE_KEY)
                .unwrap()
                .unwrap(),
            expected
        );
        assert_eq!(
            crate::store::get_session(&conn, "native-session")
                .unwrap()
                .unwrap()
                .conversation_id
                .as_deref(),
            Some(expected.conversation_id.as_str())
        );
    }

    #[test]
    fn native_id_binding_rejects_changed_generation_and_invalid_identity() {
        let (_temp, mut conn) = temp_store();
        let expected = resolve_main_session(
            &mut conn,
            INSTANCE_MAIN_SCOPE_KEY,
            None,
            "codex",
            "/tmp/home",
            NOW,
        )
        .unwrap()
        .record;
        insert_session_on(
            &conn,
            "native-session",
            "codex",
            Some(&expected.conversation_id),
        );
        assert!(bind_main_session_with_native_id(
            &mut conn,
            &expected,
            "native-session",
            "not-a-native-uuid",
            LATER
        )
        .is_err());
        let rotated = reset_main_session(
            &mut conn,
            INSTANCE_MAIN_SCOPE_KEY,
            &MainSessionSettings::default(),
            LATER,
        )
        .unwrap();
        assert!(bind_main_session_with_native_id(
            &mut conn,
            &expected,
            "native-session",
            &Uuid::new_v4().to_string(),
            LATER
        )
        .is_err());
        assert_eq!(
            get_main_session(&conn, INSTANCE_MAIN_SCOPE_KEY)
                .unwrap()
                .unwrap(),
            rotated.record
        );
        assert_eq!(
            crate::store::get_session(&conn, "native-session")
                .unwrap()
                .unwrap()
                .conversation_id
                .as_deref(),
            Some(expected.conversation_id.as_str())
        );
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

        let reset = reset_main_session(
            &mut conn,
            INSTANCE_MAIN_SCOPE_KEY,
            &MainSessionSettings::default(),
            LATER,
        )
        .unwrap();
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
    fn reset_applies_replacement_settings_and_rollover_keeps_them() {
        let (_temp, mut conn) = temp_store();
        resolve_main_session(
            &mut conn,
            INSTANCE_MAIN_SCOPE_KEY,
            Some("nova"),
            "claude",
            "/repo/a",
            NOW,
        )
        .unwrap();
        // A pointer from before `project_root` existed.
        conn.execute("UPDATE main_sessions SET project_root = NULL", [])
            .unwrap();

        let partial = MainSessionSettings {
            project_root: Some("/repo/b"),
            ..Default::default()
        };
        let reset = reset_main_session(&mut conn, INSTANCE_MAIN_SCOPE_KEY, &partial, LATER)
            .unwrap()
            .record;
        assert_eq!(reset.project_root.as_deref(), Some("/repo/b"));
        assert_eq!(reset.harness, "claude", "omitted settings are kept");
        assert_eq!(reset.familiar_id.as_deref(), Some("nova"));

        let full = MainSessionSettings {
            familiar_id: Some("cody"),
            harness: Some("codex"),
            project_root: Some("/repo/c"),
        };
        let reset = reset_main_session(&mut conn, INSTANCE_MAIN_SCOPE_KEY, &full, LATER)
            .unwrap()
            .record;
        assert_eq!(
            (
                reset.familiar_id.as_deref(),
                reset.harness.as_str(),
                reset.project_root.as_deref()
            ),
            (Some("cody"), "codex", Some("/repo/c"))
        );

        let StaleIdRotation::Rotated(rolled) = rotate_main_session_conversation(
            &mut conn,
            INSTANCE_MAIN_SCOPE_KEY,
            &reset.conversation_id,
            LATER,
        )
        .unwrap() else {
            panic!("the current id failed, so it must rotate");
        };
        let rolled = rolled.record;
        assert_eq!(rolled.harness, "codex");
        assert_eq!(rolled.project_root.as_deref(), Some("/repo/c"));

        // Blank replacements are refused without rotating anything.
        let blank = MainSessionSettings {
            harness: Some("  "),
            ..Default::default()
        };
        assert!(reset_main_session(&mut conn, INSTANCE_MAIN_SCOPE_KEY, &blank, LATER).is_err());
        let after = get_main_session(&conn, INSTANCE_MAIN_SCOPE_KEY)
            .unwrap()
            .unwrap();
        assert_eq!(after.conversation_id, rolled.conversation_id);
        assert_eq!(after.reset_count, 2);
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
        let StaleIdRotation::Rotated(rotated) = rotate_main_session_conversation(
            &mut conn,
            INSTANCE_MAIN_SCOPE_KEY,
            &resolved.record.conversation_id,
            LATER,
        )
        .unwrap() else {
            panic!("the failed id was current, so it must rotate");
        };
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

        reset_main_session(
            &mut conn,
            INSTANCE_MAIN_SCOPE_KEY,
            &MainSessionSettings::default(),
            LATER,
        )
        .unwrap();
        reset_main_session(
            &mut conn,
            INSTANCE_MAIN_SCOPE_KEY,
            &MainSessionSettings::default(),
            LATER,
        )
        .unwrap();
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
        assert!(reset_main_session(
            &mut conn,
            INSTANCE_MAIN_SCOPE_KEY,
            &MainSessionSettings::default(),
            NOW
        )
        .is_err());
        assert!(
            rotate_main_session_conversation(&mut conn, INSTANCE_MAIN_SCOPE_KEY, "c1", NOW)
                .is_err()
        );
    }

    #[test]
    fn stale_id_rotation_only_replaces_the_conversation_that_failed() {
        // Two connections stand in for two daemon requests (or a mobile and
        // a desktop client) that both saw the same conversation fail.
        let (temp, mut first) = temp_store();
        let mut second = crate::store::open_store(&temp.path().join("store.sqlite")).unwrap();
        let failed = resolve_main_session(
            &mut first,
            INSTANCE_MAIN_SCOPE_KEY,
            None,
            "claude",
            "/tmp/home",
            NOW,
        )
        .unwrap()
        .record
        .conversation_id;

        let StaleIdRotation::Rotated(winner) =
            rotate_main_session_conversation(&mut first, INSTANCE_MAIN_SCOPE_KEY, &failed, LATER)
                .unwrap()
        else {
            panic!("first rollover must rotate");
        };
        let fresh = winner.record.conversation_id.clone();

        // The delayed duplicate must not rotate the fresh, working id away.
        let late =
            rotate_main_session_conversation(&mut second, INSTANCE_MAIN_SCOPE_KEY, &failed, LATER)
                .unwrap();
        let StaleIdRotation::AlreadyRotated(current) = late else {
            panic!("a rollover for a replaced id must not rotate: {late:?}");
        };
        assert_eq!(current.conversation_id, fresh);

        // Same for a stale response that arrives after an explicit reset.
        let reset = reset_main_session(
            &mut second,
            INSTANCE_MAIN_SCOPE_KEY,
            &MainSessionSettings::default(),
            LATER,
        )
        .unwrap();
        let after_reset =
            rotate_main_session_conversation(&mut first, INSTANCE_MAIN_SCOPE_KEY, &fresh, LATER)
                .unwrap();
        assert_eq!(
            after_reset,
            StaleIdRotation::AlreadyRotated(reset.record.clone())
        );

        let stored = get_main_session(&first, INSTANCE_MAIN_SCOPE_KEY)
            .unwrap()
            .unwrap();
        assert_eq!(stored.conversation_id, reset.record.conversation_id);
        assert_eq!(stored.reset_count, 1, "only the explicit reset counts");
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
