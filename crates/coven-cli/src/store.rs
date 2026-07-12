use std::path::Path;

use anyhow::{Context, Result};
use chrono::{Duration, SecondsFormat, Utc};
use rusqlite::{params, Connection, OpenFlags, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::{
    encrypted_artifacts::{EncryptedPayload, SensitiveArtifactStore},
    privacy::{self, PrivacyConfig},
};

const FTS_BACKFILL_BATCH_SIZE: i64 = 1_000;
const FTS_BACKFILL_COMPLETE_KEY: &str = "events_fts_backfill_complete";

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
    /// Familiar id this session was launched with (`coven run --familiar <id>`).
    /// Lets clients group sessions by familiar without maintaining a sidecar map.
    /// `None` for legacy sessions and direct `coven run` invocations without
    /// the flag. Backfilled by `cwd → ~/.openclaw/workspace/<id>` heuristics
    /// remains the responsibility of the client (e.g. coven-cave); the daemon
    /// only persists what the launcher explicitly passed in.
    #[serde(default)]
    pub familiar_id: Option<String>,
    #[serde(default)]
    pub labels: Vec<String>,
    #[serde(default = "default_visibility")]
    pub visibility: String,
}

fn default_visibility() -> String {
    "private".to_string()
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
pub struct TravelProfileRecord {
    pub id: String,
    pub familiar_id: String,
    pub workspace_id: String,
    pub version: String,
    pub generated_at: String,
    pub expires_at: String,
    pub stale_after: String,
    pub source_hub_id: String,
    pub source_revision_json: String,
    pub permissions_json: String,
    pub payload_json: String,
    pub encoding: String,
    pub content_hash: String,
    pub profile_blob: String,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TravelDeltaRecord {
    pub id: String,
    pub profile_id: String,
    pub source_hub_id: String,
    pub client_id: String,
    pub state: String,
    pub raw_delta_json: String,
    pub accepted_events: i64,
    pub accepted_artifacts: i64,
    pub memory_review_state: String,
    pub canonical_memory_overwrite_applied: bool,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchedulerDecisionRecord {
    pub id: String,
    pub job_id: String,
    pub target_role: String,
    pub target_node_id: Option<String>,
    pub target_json: String,
    pub reason: String,
    pub inputs_json: String,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchedulerLoopStateRecord {
    pub loop_id: String,
    pub job_id: String,
    pub state: String,
    pub decision_id: String,
    pub target_json: String,
    pub preserved_subqueue_node_id: String,
    pub node_availability_json: String,
    pub reason: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutorQueueRecord {
    pub node_id: String,
    pub job_ids_json: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeRecord {
    pub node_id: String,
    pub role: String,
    pub transport: String,
    /// Structured hub-outbound dispatch config (`executor_node::TransportConfig`
    /// JSON). `None` means the node cannot be polled/dispatched yet.
    pub transport_config_json: Option<String>,
    pub capabilities_json: String,
    pub available: bool,
    pub queue_pressure: i64,
    pub last_health_at: String,
    /// Last hub-initiated poll/dispatch failure, cleared on success.
    pub last_error: Option<String>,
    pub registered_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutorDispatchRecord {
    pub job_id: String,
    pub node_id: String,
    pub status: String,
    pub job_json: String,
    pub envelope_json: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HubJobRecord {
    pub job_id: String,
    pub state: String,
    pub priority: i64,
    pub required_capabilities_json: String,
    pub assigned_node_id: Option<String>,
    pub loop_id: Option<String>,
    pub payload_json: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RouteRecord {
    pub job_id: String,
    pub node_id: String,
    pub decision_id: Option<String>,
    pub reason: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepositoryRecord {
    pub id: String,
    pub path: String,
    pub package_name: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SensitiveArtifactRecord {
    pub id: String,
    pub session_id: String,
    pub event_id: String,
    pub kind: String,
    pub nonce: Vec<u8>,
    pub ciphertext: Vec<u8>,
    pub created_at: String,
    pub expires_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SearchHit {
    pub event_id: String,
    pub session_id: String,
    pub kind: String,
    pub snippet: String,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StoreVacuumReport {
    pub event_index_rebuilt: bool,
    pub integrity_check: Vec<String>,
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
    configure_writable_connection(&conn)?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS sessions (
            id TEXT PRIMARY KEY NOT NULL,
            project_root TEXT NOT NULL,
            harness TEXT NOT NULL,
            title TEXT NOT NULL,
            status TEXT NOT NULL,
            exit_code INTEGER,
            archived_at TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            conversation_id TEXT,
            labels TEXT,
            visibility TEXT NOT NULL DEFAULT 'private',
            familiar_id TEXT
        );

        CREATE TABLE IF NOT EXISTS events (
            id TEXT PRIMARY KEY NOT NULL,
            session_id TEXT NOT NULL,
            kind TEXT NOT NULL,
            payload_json TEXT NOT NULL,
            created_at TEXT NOT NULL,
            redaction_status TEXT NOT NULL DEFAULT 'redacted',
            sensitive INTEGER NOT NULL DEFAULT 0,
            FOREIGN KEY (session_id) REFERENCES sessions(id) ON DELETE CASCADE
        );

        CREATE INDEX IF NOT EXISTS idx_sessions_created_at
            ON sessions(created_at DESC);

        CREATE INDEX IF NOT EXISTS idx_events_session_created_at
            ON events(session_id, created_at);

        CREATE TABLE IF NOT EXISTS sensitive_artifacts (
            id TEXT PRIMARY KEY NOT NULL,
            session_id TEXT NOT NULL,
            event_id TEXT NOT NULL,
            kind TEXT NOT NULL,
            nonce BLOB NOT NULL,
            ciphertext BLOB NOT NULL,
            created_at TEXT NOT NULL,
            expires_at TEXT NOT NULL,
            FOREIGN KEY (session_id) REFERENCES sessions(id) ON DELETE CASCADE,
            FOREIGN KEY (event_id) REFERENCES events(id) ON DELETE CASCADE
        );

        CREATE INDEX IF NOT EXISTS idx_sensitive_artifacts_session
            ON sensitive_artifacts(session_id, created_at);

        CREATE INDEX IF NOT EXISTS idx_sensitive_artifacts_expires_at
            ON sensitive_artifacts(expires_at);

        CREATE TABLE IF NOT EXISTS repositories (
            id TEXT PRIMARY KEY NOT NULL,
            path TEXT NOT NULL,
            package_name TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS store_meta (
            key TEXT PRIMARY KEY NOT NULL,
            value TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS travel_profiles (
            id TEXT PRIMARY KEY NOT NULL,
            familiar_id TEXT NOT NULL,
            workspace_id TEXT NOT NULL,
            version TEXT NOT NULL,
            generated_at TEXT NOT NULL,
            expires_at TEXT NOT NULL,
            stale_after TEXT NOT NULL,
            source_hub_id TEXT NOT NULL,
            source_revision_json TEXT NOT NULL,
            permissions_json TEXT NOT NULL,
            payload_json TEXT NOT NULL,
            encoding TEXT NOT NULL,
            content_hash TEXT NOT NULL,
            profile_blob TEXT NOT NULL,
            created_at TEXT NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_travel_profiles_scope
            ON travel_profiles(familiar_id, workspace_id, generated_at DESC);

        CREATE TABLE IF NOT EXISTS travel_deltas (
            id TEXT PRIMARY KEY NOT NULL,
            profile_id TEXT NOT NULL,
            source_hub_id TEXT NOT NULL,
            client_id TEXT NOT NULL,
            state TEXT NOT NULL,
            raw_delta_json TEXT NOT NULL,
            accepted_events INTEGER NOT NULL,
            accepted_artifacts INTEGER NOT NULL,
            memory_review_state TEXT NOT NULL,
            canonical_memory_overwrite_applied INTEGER NOT NULL DEFAULT 0,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            FOREIGN KEY (profile_id) REFERENCES travel_profiles(id) ON DELETE CASCADE
        );

        CREATE INDEX IF NOT EXISTS idx_travel_deltas_client
            ON travel_deltas(client_id, updated_at DESC);

        CREATE TABLE IF NOT EXISTS scheduler_decisions (
            id TEXT PRIMARY KEY NOT NULL,
            job_id TEXT NOT NULL,
            target_role TEXT NOT NULL,
            target_node_id TEXT,
            target_json TEXT NOT NULL,
            reason TEXT NOT NULL,
            inputs_json TEXT NOT NULL,
            created_at TEXT NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_scheduler_decisions_job
            ON scheduler_decisions(job_id, created_at DESC);

        CREATE TABLE IF NOT EXISTS executor_queue (
            node_id TEXT PRIMARY KEY NOT NULL,
            job_ids_json TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS node_registry (
            node_id TEXT PRIMARY KEY NOT NULL,
            role TEXT NOT NULL,
            transport TEXT NOT NULL,
            transport_config_json TEXT,
            capabilities_json TEXT NOT NULL,
            available INTEGER NOT NULL DEFAULT 0,
            queue_pressure INTEGER NOT NULL DEFAULT 0,
            last_health_at TEXT NOT NULL,
            last_error TEXT,
            registered_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_node_registry_available
            ON node_registry(available, queue_pressure);

        CREATE TABLE IF NOT EXISTS executor_dispatches (
            job_id TEXT PRIMARY KEY NOT NULL,
            node_id TEXT NOT NULL,
            status TEXT NOT NULL,
            job_json TEXT NOT NULL,
            envelope_json TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_executor_dispatches_node
            ON executor_dispatches(node_id, updated_at DESC);

        CREATE TABLE IF NOT EXISTS hub_jobs (
            job_id TEXT PRIMARY KEY NOT NULL,
            state TEXT NOT NULL,
            priority INTEGER NOT NULL DEFAULT 0,
            required_capabilities_json TEXT NOT NULL,
            assigned_node_id TEXT,
            loop_id TEXT,
            payload_json TEXT NOT NULL,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_hub_jobs_state
            ON hub_jobs(state, priority DESC, created_at);

        CREATE INDEX IF NOT EXISTS idx_hub_jobs_assigned_node
            ON hub_jobs(assigned_node_id, state);

        CREATE TABLE IF NOT EXISTS routing_table (
            job_id TEXT PRIMARY KEY NOT NULL,
            node_id TEXT NOT NULL,
            decision_id TEXT,
            reason TEXT NOT NULL,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_routing_table_node
            ON routing_table(node_id, updated_at DESC);

        CREATE TABLE IF NOT EXISTS loop_state (
            loop_id TEXT PRIMARY KEY NOT NULL,
            job_id TEXT NOT NULL,
            state TEXT NOT NULL,
            decision_id TEXT NOT NULL,
            target_json TEXT NOT NULL,
            preserved_subqueue_node_id TEXT NOT NULL,
            node_availability_json TEXT NOT NULL,
            reason TEXT NOT NULL,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            FOREIGN KEY (decision_id) REFERENCES scheduler_decisions(id) ON DELETE CASCADE
        );

        CREATE INDEX IF NOT EXISTS idx_loop_state_job
            ON loop_state(job_id, updated_at DESC);

        CREATE VIRTUAL TABLE IF NOT EXISTS events_fts USING fts5(
            payload_json,
            content='events',
            content_rowid='rowid'
        );

        CREATE TRIGGER IF NOT EXISTS events_fts_insert AFTER INSERT ON events BEGIN
            INSERT INTO events_fts(rowid, payload_json) VALUES (new.rowid, new.payload_json);
        END;

        CREATE TRIGGER IF NOT EXISTS events_fts_delete AFTER DELETE ON events BEGIN
            INSERT INTO events_fts(events_fts, rowid, payload_json) VALUES('delete', old.rowid, old.payload_json);
        END;

        CREATE TRIGGER IF NOT EXISTS events_fts_update AFTER UPDATE ON events BEGIN
            INSERT INTO events_fts(events_fts, rowid, payload_json) VALUES('delete', old.rowid, old.payload_json);
            INSERT INTO events_fts(rowid, payload_json) VALUES (new.rowid, new.payload_json);
        END;
        ",
    )
    .context("failed to initialize Coven store schema")?;
    ensure_exit_code_column(&conn)?;
    ensure_archived_at_column(&conn)?;
    ensure_conversation_id_column(&conn)?;
    ensure_event_privacy_columns(&conn)?;
    ensure_sensitive_artifacts_table(&conn)?;
    ensure_labels_column(&conn)?;
    ensure_visibility_column(&conn)?;
    ensure_familiar_id_column(&conn)?;
    ensure_node_registry_dispatch_columns(&conn)?;

    backfill_events_fts_if_needed(&conn)?;

    Ok(conn)
}

fn configure_writable_connection(conn: &Connection) -> Result<()> {
    // WAL mode allows concurrent readers alongside a single writer and avoids
    // "database is locked" errors under typical daemon + API concurrency.
    // busy_timeout gives writers up to 5 s to retry before returning SQLITE_BUSY.
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA busy_timeout = 5000;
         PRAGMA foreign_keys = ON;",
    )
    .context("failed to configure writable Coven store connection")?;
    Ok(())
}

fn configure_read_only_connection(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "PRAGMA busy_timeout = 5000;
         PRAGMA foreign_keys = ON;",
    )
    .context("failed to configure read-only Coven store connection")?;
    Ok(())
}

fn backfill_events_fts_if_needed(conn: &Connection) -> Result<()> {
    let already_complete: Option<String> = conn
        .query_row(
            "SELECT value FROM store_meta WHERE key = ?1",
            [FTS_BACKFILL_COMPLETE_KEY],
            |row| row.get(0),
        )
        .optional()
        .context("failed to read events_fts backfill state")?;
    if already_complete.as_deref() == Some("1") {
        return Ok(());
    }

    loop {
        let inserted = match conn.execute(
            "INSERT INTO events_fts(rowid, payload_json)
             SELECT e.rowid, e.payload_json
             FROM events e
             LEFT JOIN events_fts_docsize d ON d.id = e.rowid
             WHERE d.id IS NULL
             ORDER BY e.rowid
             LIMIT ?1",
            [FTS_BACKFILL_BATCH_SIZE],
        ) {
            Ok(inserted) => inserted,
            Err(error) => {
                eprintln!(
                    "warning: events_fts backfill skipped for now; session dispatch will continue ({error})"
                );
                return Ok(());
            }
        };
        if inserted == 0 {
            break;
        }
    }

    if let Err(error) = conn.execute(
        "INSERT INTO store_meta(key, value)
         VALUES(?1, '1')
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        [FTS_BACKFILL_COMPLETE_KEY],
    ) {
        eprintln!(
            "warning: events_fts backfill completed but could not record completion ({error})"
        );
    }
    Ok(())
}

fn ensure_event_privacy_columns(conn: &Connection) -> Result<()> {
    ensure_column(
        conn,
        "events",
        "redaction_status",
        "ALTER TABLE events ADD COLUMN redaction_status TEXT NOT NULL DEFAULT 'legacy'",
    )?;
    ensure_column(
        conn,
        "events",
        "sensitive",
        "ALTER TABLE events ADD COLUMN sensitive INTEGER NOT NULL DEFAULT 0",
    )?;
    Ok(())
}

fn ensure_column(conn: &Connection, table: &str, column: &str, sql: &str) -> Result<()> {
    let mut statement = conn
        .prepare(&format!("PRAGMA table_info({table})"))
        .with_context(|| format!("failed to inspect {table} schema"))?;
    let has_column = statement
        .query_map([], |row| row.get::<_, String>(1))
        .with_context(|| format!("failed to query {table} schema"))?
        .collect::<std::result::Result<Vec<_>, _>>()
        .with_context(|| format!("failed to read {table} schema"))?
        .into_iter()
        .any(|candidate| candidate == column);

    if !has_column {
        conn.execute(sql, [])
            .with_context(|| format!("failed to add {table}.{column} column"))?;
    }
    Ok(())
}

fn ensure_sensitive_artifacts_table(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS sensitive_artifacts (
            id TEXT PRIMARY KEY NOT NULL,
            session_id TEXT NOT NULL,
            event_id TEXT NOT NULL,
            kind TEXT NOT NULL,
            nonce BLOB NOT NULL,
            ciphertext BLOB NOT NULL,
            created_at TEXT NOT NULL,
            expires_at TEXT NOT NULL,
            FOREIGN KEY (session_id) REFERENCES sessions(id) ON DELETE CASCADE,
            FOREIGN KEY (event_id) REFERENCES events(id) ON DELETE CASCADE
        );

        CREATE INDEX IF NOT EXISTS idx_sensitive_artifacts_session
            ON sensitive_artifacts(session_id, created_at);

        CREATE INDEX IF NOT EXISTS idx_sensitive_artifacts_expires_at
            ON sensitive_artifacts(expires_at);",
    )
    .context("failed to initialize sensitive artifact schema")
}

pub fn open_existing_store_read_only(path: &Path) -> Result<Option<Connection>> {
    if !path.exists() {
        return Ok(None);
    }

    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .with_context(|| format!("failed to open Coven store read-only at {}", path.display()))?;
    configure_read_only_connection(&conn)?;
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

fn ensure_labels_column(conn: &Connection) -> Result<()> {
    let mut statement = conn
        .prepare("PRAGMA table_info(sessions)")
        .context("failed to inspect sessions schema")?;
    let has_labels = statement
        .query_map([], |row| row.get::<_, String>(1))
        .context("failed to query sessions schema")?
        .collect::<std::result::Result<Vec<_>, _>>()
        .context("failed to read sessions schema")?
        .into_iter()
        .any(|column| column == "labels");
    if !has_labels {
        conn.execute("ALTER TABLE sessions ADD COLUMN labels TEXT", [])
            .context("failed to add sessions.labels column")?;
    }
    Ok(())
}

fn ensure_visibility_column(conn: &Connection) -> Result<()> {
    let mut statement = conn
        .prepare("PRAGMA table_info(sessions)")
        .context("failed to inspect sessions schema")?;
    let has_visibility = statement
        .query_map([], |row| row.get::<_, String>(1))
        .context("failed to query sessions schema")?
        .collect::<std::result::Result<Vec<_>, _>>()
        .context("failed to read sessions schema")?
        .into_iter()
        .any(|column| column == "visibility");
    if !has_visibility {
        conn.execute(
            "ALTER TABLE sessions ADD COLUMN visibility TEXT NOT NULL DEFAULT 'private'",
            [],
        )
        .context("failed to add sessions.visibility column")?;
    }
    Ok(())
}

fn ensure_familiar_id_column(conn: &Connection) -> Result<()> {
    ensure_column(
        conn,
        "sessions",
        "familiar_id",
        "ALTER TABLE sessions ADD COLUMN familiar_id TEXT",
    )?;
    // Index makes "sessions for familiar X" cheap. The column is sparse on
    // existing stores (legacy sessions are NULL until the client migrates),
    // so a partial index keeps it small.
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_sessions_familiar_id
            ON sessions(familiar_id) WHERE familiar_id IS NOT NULL",
        [],
    )
    .context("failed to create sessions.familiar_id index")?;
    Ok(())
}

/// Stores created at the initial node_registry schema (#266) predate the
/// hub-outbound dispatch columns (#267); add them idempotently.
fn ensure_node_registry_dispatch_columns(conn: &Connection) -> Result<()> {
    ensure_column(
        conn,
        "node_registry",
        "transport_config_json",
        "ALTER TABLE node_registry ADD COLUMN transport_config_json TEXT",
    )?;
    ensure_column(
        conn,
        "node_registry",
        "last_error",
        "ALTER TABLE node_registry ADD COLUMN last_error TEXT",
    )?;
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

pub fn get_or_insert_store_meta(
    conn: &Connection,
    key: &str,
    default_value: &str,
) -> Result<String> {
    conn.execute(
        "INSERT OR IGNORE INTO store_meta(key, value) VALUES(?1, ?2)",
        params![key, default_value],
    )
    .with_context(|| format!("failed to initialize store_meta key {key}"))?;
    conn.query_row(
        "SELECT value FROM store_meta WHERE key = ?1",
        params![key],
        |row| row.get(0),
    )
    .with_context(|| format!("failed to read store_meta key {key}"))
}

pub fn insert_travel_profile(conn: &Connection, record: &TravelProfileRecord) -> Result<()> {
    conn.execute(
        "INSERT INTO travel_profiles (
            id,
            familiar_id,
            workspace_id,
            version,
            generated_at,
            expires_at,
            stale_after,
            source_hub_id,
            source_revision_json,
            permissions_json,
            payload_json,
            encoding,
            content_hash,
            profile_blob,
            created_at
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
        params![
            &record.id,
            &record.familiar_id,
            &record.workspace_id,
            &record.version,
            &record.generated_at,
            &record.expires_at,
            &record.stale_after,
            &record.source_hub_id,
            &record.source_revision_json,
            &record.permissions_json,
            &record.payload_json,
            &record.encoding,
            &record.content_hash,
            &record.profile_blob,
            &record.created_at,
        ],
    )
    .with_context(|| format!("failed to insert travel profile {}", record.id))?;
    Ok(())
}

pub fn get_travel_profile(conn: &Connection, id: &str) -> Result<Option<TravelProfileRecord>> {
    conn.query_row(
        "SELECT
            id,
            familiar_id,
            workspace_id,
            version,
            generated_at,
            expires_at,
            stale_after,
            source_hub_id,
            source_revision_json,
            permissions_json,
            payload_json,
            encoding,
            content_hash,
            profile_blob,
            created_at
         FROM travel_profiles
         WHERE id = ?1
         LIMIT 1",
        params![id],
        travel_profile_from_row,
    )
    .optional()
    .with_context(|| format!("failed to read travel profile {id}"))
}

fn travel_profile_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<TravelProfileRecord> {
    Ok(TravelProfileRecord {
        id: row.get(0)?,
        familiar_id: row.get(1)?,
        workspace_id: row.get(2)?,
        version: row.get(3)?,
        generated_at: row.get(4)?,
        expires_at: row.get(5)?,
        stale_after: row.get(6)?,
        source_hub_id: row.get(7)?,
        source_revision_json: row.get(8)?,
        permissions_json: row.get(9)?,
        payload_json: row.get(10)?,
        encoding: row.get(11)?,
        content_hash: row.get(12)?,
        profile_blob: row.get(13)?,
        created_at: row.get(14)?,
    })
}

pub fn insert_travel_delta(conn: &Connection, record: &TravelDeltaRecord) -> Result<()> {
    conn.execute(
        "INSERT INTO travel_deltas (
            id,
            profile_id,
            source_hub_id,
            client_id,
            state,
            raw_delta_json,
            accepted_events,
            accepted_artifacts,
            memory_review_state,
            canonical_memory_overwrite_applied,
            created_at,
            updated_at
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        params![
            &record.id,
            &record.profile_id,
            &record.source_hub_id,
            &record.client_id,
            &record.state,
            &record.raw_delta_json,
            record.accepted_events,
            record.accepted_artifacts,
            &record.memory_review_state,
            if record.canonical_memory_overwrite_applied {
                1
            } else {
                0
            },
            &record.created_at,
            &record.updated_at,
        ],
    )
    .with_context(|| format!("failed to insert travel delta {}", record.id))?;
    Ok(())
}

pub fn latest_travel_delta_for_client(
    conn: &Connection,
    client_id: &str,
) -> Result<Option<TravelDeltaRecord>> {
    conn.query_row(
        "SELECT
            id,
            profile_id,
            source_hub_id,
            client_id,
            state,
            raw_delta_json,
            accepted_events,
            accepted_artifacts,
            memory_review_state,
            canonical_memory_overwrite_applied,
            created_at,
            updated_at
         FROM travel_deltas
         WHERE client_id = ?1
         ORDER BY updated_at DESC, id DESC
         LIMIT 1",
        params![client_id],
        travel_delta_from_row,
    )
    .optional()
    .with_context(|| format!("failed to read latest travel delta for client {client_id}"))
}

fn travel_delta_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<TravelDeltaRecord> {
    let overwrite_applied: i64 = row.get(9)?;
    Ok(TravelDeltaRecord {
        id: row.get(0)?,
        profile_id: row.get(1)?,
        source_hub_id: row.get(2)?,
        client_id: row.get(3)?,
        state: row.get(4)?,
        raw_delta_json: row.get(5)?,
        accepted_events: row.get(6)?,
        accepted_artifacts: row.get(7)?,
        memory_review_state: row.get(8)?,
        canonical_memory_overwrite_applied: overwrite_applied != 0,
        created_at: row.get(10)?,
        updated_at: row.get(11)?,
    })
}

pub fn insert_scheduler_decision(
    conn: &Connection,
    record: &SchedulerDecisionRecord,
) -> Result<()> {
    conn.execute(
        "INSERT INTO scheduler_decisions (
            id,
            job_id,
            target_role,
            target_node_id,
            target_json,
            reason,
            inputs_json,
            created_at
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            &record.id,
            &record.job_id,
            &record.target_role,
            &record.target_node_id,
            &record.target_json,
            &record.reason,
            &record.inputs_json,
            &record.created_at,
        ],
    )
    .with_context(|| format!("failed to insert scheduler decision {}", record.id))?;
    Ok(())
}

pub fn get_scheduler_decision(
    conn: &Connection,
    id: &str,
) -> Result<Option<SchedulerDecisionRecord>> {
    conn.query_row(
        "SELECT
            id,
            job_id,
            target_role,
            target_node_id,
            target_json,
            reason,
            inputs_json,
            created_at
         FROM scheduler_decisions
         WHERE id = ?1
         LIMIT 1",
        params![id],
        |row| {
            Ok(SchedulerDecisionRecord {
                id: row.get(0)?,
                job_id: row.get(1)?,
                target_role: row.get(2)?,
                target_node_id: row.get(3)?,
                target_json: row.get(4)?,
                reason: row.get(5)?,
                inputs_json: row.get(6)?,
                created_at: row.get(7)?,
            })
        },
    )
    .optional()
    .with_context(|| format!("failed to read scheduler decision {id}"))
}

pub fn upsert_executor_queue(conn: &Connection, record: &ExecutorQueueRecord) -> Result<()> {
    conn.execute(
        "INSERT INTO executor_queue (
            node_id,
            job_ids_json,
            updated_at
        ) VALUES (?1, ?2, ?3)
        ON CONFLICT(node_id) DO UPDATE SET
            job_ids_json = excluded.job_ids_json,
            updated_at = excluded.updated_at",
        params![&record.node_id, &record.job_ids_json, &record.updated_at],
    )
    .with_context(|| format!("failed to upsert executor queue {}", record.node_id))?;
    Ok(())
}

pub fn get_executor_queue(conn: &Connection, node_id: &str) -> Result<Option<ExecutorQueueRecord>> {
    conn.query_row(
        "SELECT node_id, job_ids_json, updated_at
         FROM executor_queue
         WHERE node_id = ?1
         LIMIT 1",
        params![node_id],
        |row| {
            Ok(ExecutorQueueRecord {
                node_id: row.get(0)?,
                job_ids_json: row.get(1)?,
                updated_at: row.get(2)?,
            })
        },
    )
    .optional()
    .with_context(|| format!("failed to read executor queue {node_id}"))
}

pub fn list_executor_queues(conn: &Connection) -> Result<Vec<ExecutorQueueRecord>> {
    let mut statement = conn
        .prepare(
            "SELECT node_id, job_ids_json, updated_at
             FROM executor_queue
             ORDER BY node_id",
        )
        .context("failed to prepare executor queue list")?;
    let rows = statement
        .query_map([], |row| {
            Ok(ExecutorQueueRecord {
                node_id: row.get(0)?,
                job_ids_json: row.get(1)?,
                updated_at: row.get(2)?,
            })
        })
        .context("failed to list executor queues")?;
    let mut records = Vec::new();
    for row in rows {
        records.push(row.context("failed to read executor queue row")?);
    }
    Ok(records)
}

pub fn upsert_node(conn: &Connection, record: &NodeRecord) -> Result<()> {
    conn.execute(
        "INSERT INTO node_registry (
            node_id,
            role,
            transport,
            transport_config_json,
            capabilities_json,
            available,
            queue_pressure,
            last_health_at,
            last_error,
            registered_at,
            updated_at
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
        ON CONFLICT(node_id) DO UPDATE SET
            role = excluded.role,
            transport = excluded.transport,
            transport_config_json = excluded.transport_config_json,
            capabilities_json = excluded.capabilities_json,
            available = excluded.available,
            queue_pressure = excluded.queue_pressure,
            last_health_at = excluded.last_health_at,
            last_error = excluded.last_error,
            updated_at = excluded.updated_at",
        params![
            &record.node_id,
            &record.role,
            &record.transport,
            &record.transport_config_json,
            &record.capabilities_json,
            if record.available { 1 } else { 0 },
            record.queue_pressure,
            &record.last_health_at,
            &record.last_error,
            &record.registered_at,
            &record.updated_at,
        ],
    )
    .with_context(|| format!("failed to upsert node {}", record.node_id))?;
    Ok(())
}

fn node_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<NodeRecord> {
    let available: i64 = row.get(5)?;
    Ok(NodeRecord {
        node_id: row.get(0)?,
        role: row.get(1)?,
        transport: row.get(2)?,
        transport_config_json: row.get(3)?,
        capabilities_json: row.get(4)?,
        available: available != 0,
        queue_pressure: row.get(6)?,
        last_health_at: row.get(7)?,
        last_error: row.get(8)?,
        registered_at: row.get(9)?,
        updated_at: row.get(10)?,
    })
}

const NODE_COLUMNS: &str = "node_id, role, transport, transport_config_json, capabilities_json, \
     available, queue_pressure, last_health_at, last_error, registered_at, updated_at";

pub fn get_node(conn: &Connection, node_id: &str) -> Result<Option<NodeRecord>> {
    conn.query_row(
        &format!("SELECT {NODE_COLUMNS} FROM node_registry WHERE node_id = ?1 LIMIT 1"),
        params![node_id],
        node_from_row,
    )
    .optional()
    .with_context(|| format!("failed to read node {node_id}"))
}

pub fn list_nodes(conn: &Connection) -> Result<Vec<NodeRecord>> {
    let mut statement = conn
        .prepare(&format!(
            "SELECT {NODE_COLUMNS} FROM node_registry ORDER BY node_id"
        ))
        .context("failed to prepare node registry list")?;
    let rows = statement
        .query_map([], node_from_row)
        .context("failed to list node registry")?;
    let mut records = Vec::new();
    for row in rows {
        records.push(row.context("failed to read node registry row")?);
    }
    Ok(records)
}

pub fn upsert_executor_dispatch(conn: &Connection, record: &ExecutorDispatchRecord) -> Result<()> {
    conn.execute(
        "INSERT INTO executor_dispatches (
            job_id,
            node_id,
            status,
            job_json,
            envelope_json,
            created_at,
            updated_at
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
        ON CONFLICT(job_id) DO UPDATE SET
            node_id = excluded.node_id,
            status = excluded.status,
            job_json = excluded.job_json,
            envelope_json = excluded.envelope_json,
            updated_at = excluded.updated_at",
        params![
            &record.job_id,
            &record.node_id,
            &record.status,
            &record.job_json,
            &record.envelope_json,
            &record.created_at,
            &record.updated_at,
        ],
    )
    .with_context(|| format!("failed to upsert executor dispatch {}", record.job_id))?;
    Ok(())
}

pub fn get_executor_dispatch(
    conn: &Connection,
    job_id: &str,
) -> Result<Option<ExecutorDispatchRecord>> {
    conn.query_row(
        "SELECT job_id, node_id, status, job_json, envelope_json, created_at, updated_at
         FROM executor_dispatches
         WHERE job_id = ?1
         LIMIT 1",
        params![job_id],
        |row| {
            Ok(ExecutorDispatchRecord {
                job_id: row.get(0)?,
                node_id: row.get(1)?,
                status: row.get(2)?,
                job_json: row.get(3)?,
                envelope_json: row.get(4)?,
                created_at: row.get(5)?,
                updated_at: row.get(6)?,
            })
        },
    )
    .optional()
    .with_context(|| format!("failed to read executor dispatch {job_id}"))
}

pub fn upsert_hub_job(conn: &Connection, record: &HubJobRecord) -> Result<()> {
    conn.execute(
        "INSERT INTO hub_jobs (
            job_id,
            state,
            priority,
            required_capabilities_json,
            assigned_node_id,
            loop_id,
            payload_json,
            created_at,
            updated_at
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
        ON CONFLICT(job_id) DO UPDATE SET
            state = excluded.state,
            priority = excluded.priority,
            required_capabilities_json = excluded.required_capabilities_json,
            assigned_node_id = excluded.assigned_node_id,
            loop_id = excluded.loop_id,
            payload_json = excluded.payload_json,
            updated_at = excluded.updated_at",
        params![
            &record.job_id,
            &record.state,
            record.priority,
            &record.required_capabilities_json,
            &record.assigned_node_id,
            &record.loop_id,
            &record.payload_json,
            &record.created_at,
            &record.updated_at,
        ],
    )
    .with_context(|| format!("failed to upsert hub job {}", record.job_id))?;
    Ok(())
}

fn hub_job_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<HubJobRecord> {
    Ok(HubJobRecord {
        job_id: row.get(0)?,
        state: row.get(1)?,
        priority: row.get(2)?,
        required_capabilities_json: row.get(3)?,
        assigned_node_id: row.get(4)?,
        loop_id: row.get(5)?,
        payload_json: row.get(6)?,
        created_at: row.get(7)?,
        updated_at: row.get(8)?,
    })
}

const HUB_JOB_COLUMNS: &str = "job_id, state, priority, required_capabilities_json, \
     assigned_node_id, loop_id, payload_json, created_at, updated_at";

pub fn get_hub_job(conn: &Connection, job_id: &str) -> Result<Option<HubJobRecord>> {
    conn.query_row(
        &format!("SELECT {HUB_JOB_COLUMNS} FROM hub_jobs WHERE job_id = ?1 LIMIT 1"),
        params![job_id],
        hub_job_from_row,
    )
    .optional()
    .with_context(|| format!("failed to read hub job {job_id}"))
}

pub fn list_hub_jobs(conn: &Connection, state: Option<&str>) -> Result<Vec<HubJobRecord>> {
    let mut records = Vec::new();
    match state {
        Some(state) => {
            let mut statement = conn
                .prepare(&format!(
                    "SELECT {HUB_JOB_COLUMNS} FROM hub_jobs
                     WHERE state = ?1
                     ORDER BY priority DESC, created_at, job_id"
                ))
                .context("failed to prepare hub job list")?;
            let rows = statement
                .query_map(params![state], hub_job_from_row)
                .context("failed to list hub jobs")?;
            for row in rows {
                records.push(row.context("failed to read hub job row")?);
            }
        }
        None => {
            let mut statement = conn
                .prepare(&format!(
                    "SELECT {HUB_JOB_COLUMNS} FROM hub_jobs
                     ORDER BY priority DESC, created_at, job_id"
                ))
                .context("failed to prepare hub job list")?;
            let rows = statement
                .query_map([], hub_job_from_row)
                .context("failed to list hub jobs")?;
            for row in rows {
                records.push(row.context("failed to read hub job row")?);
            }
        }
    }
    Ok(records)
}

pub fn list_hub_jobs_for_node(conn: &Connection, node_id: &str) -> Result<Vec<HubJobRecord>> {
    let mut statement = conn
        .prepare(&format!(
            "SELECT {HUB_JOB_COLUMNS} FROM hub_jobs
             WHERE assigned_node_id = ?1
             ORDER BY priority DESC, created_at, job_id"
        ))
        .context("failed to prepare hub job node list")?;
    let rows = statement
        .query_map(params![node_id], hub_job_from_row)
        .context("failed to list hub jobs for node")?;
    let mut records = Vec::new();
    for row in rows {
        records.push(row.context("failed to read hub job row")?);
    }
    Ok(records)
}

pub fn update_hub_job_state(
    conn: &Connection,
    job_id: &str,
    state: &str,
    assigned_node_id: Option<&str>,
    updated_at: &str,
) -> Result<()> {
    conn.execute(
        "UPDATE hub_jobs
         SET state = ?2, assigned_node_id = ?3, updated_at = ?4
         WHERE job_id = ?1",
        params![job_id, state, assigned_node_id, updated_at],
    )
    .with_context(|| format!("failed to update hub job {job_id}"))?;
    Ok(())
}

pub fn upsert_route(conn: &Connection, record: &RouteRecord) -> Result<()> {
    conn.execute(
        "INSERT INTO routing_table (
            job_id,
            node_id,
            decision_id,
            reason,
            created_at,
            updated_at
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
        ON CONFLICT(job_id) DO UPDATE SET
            node_id = excluded.node_id,
            decision_id = excluded.decision_id,
            reason = excluded.reason,
            updated_at = excluded.updated_at",
        params![
            &record.job_id,
            &record.node_id,
            &record.decision_id,
            &record.reason,
            &record.created_at,
            &record.updated_at,
        ],
    )
    .with_context(|| format!("failed to upsert route for job {}", record.job_id))?;
    Ok(())
}

fn route_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RouteRecord> {
    Ok(RouteRecord {
        job_id: row.get(0)?,
        node_id: row.get(1)?,
        decision_id: row.get(2)?,
        reason: row.get(3)?,
        created_at: row.get(4)?,
        updated_at: row.get(5)?,
    })
}

pub fn get_route(conn: &Connection, job_id: &str) -> Result<Option<RouteRecord>> {
    conn.query_row(
        "SELECT job_id, node_id, decision_id, reason, created_at, updated_at
         FROM routing_table
         WHERE job_id = ?1
         LIMIT 1",
        params![job_id],
        route_from_row,
    )
    .optional()
    .with_context(|| format!("failed to read route for job {job_id}"))
}

pub fn list_routes(conn: &Connection) -> Result<Vec<RouteRecord>> {
    let mut statement = conn
        .prepare(
            "SELECT job_id, node_id, decision_id, reason, created_at, updated_at
             FROM routing_table
             ORDER BY updated_at DESC, job_id",
        )
        .context("failed to prepare routing table list")?;
    let rows = statement
        .query_map([], route_from_row)
        .context("failed to list routing table")?;
    let mut records = Vec::new();
    for row in rows {
        records.push(row.context("failed to read routing table row")?);
    }
    Ok(records)
}

pub fn upsert_scheduler_loop_state(
    conn: &Connection,
    record: &SchedulerLoopStateRecord,
) -> Result<()> {
    conn.execute(
        "INSERT INTO loop_state (
            loop_id,
            job_id,
            state,
            decision_id,
            target_json,
            preserved_subqueue_node_id,
            node_availability_json,
            reason,
            created_at,
            updated_at
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
        ON CONFLICT(loop_id) DO UPDATE SET
            job_id = excluded.job_id,
            state = excluded.state,
            decision_id = excluded.decision_id,
            target_json = excluded.target_json,
            preserved_subqueue_node_id = excluded.preserved_subqueue_node_id,
            node_availability_json = excluded.node_availability_json,
            reason = excluded.reason,
            updated_at = excluded.updated_at",
        params![
            &record.loop_id,
            &record.job_id,
            &record.state,
            &record.decision_id,
            &record.target_json,
            &record.preserved_subqueue_node_id,
            &record.node_availability_json,
            &record.reason,
            &record.created_at,
            &record.updated_at,
        ],
    )
    .with_context(|| format!("failed to upsert loop state {}", record.loop_id))?;
    Ok(())
}

pub fn get_scheduler_loop_state(
    conn: &Connection,
    loop_id: &str,
) -> Result<Option<SchedulerLoopStateRecord>> {
    conn.query_row(
        "SELECT
            loop_id,
            job_id,
            state,
            decision_id,
            target_json,
            preserved_subqueue_node_id,
            node_availability_json,
            reason,
            created_at,
            updated_at
         FROM loop_state
         WHERE loop_id = ?1
         LIMIT 1",
        params![loop_id],
        |row| {
            Ok(SchedulerLoopStateRecord {
                loop_id: row.get(0)?,
                job_id: row.get(1)?,
                state: row.get(2)?,
                decision_id: row.get(3)?,
                target_json: row.get(4)?,
                preserved_subqueue_node_id: row.get(5)?,
                node_availability_json: row.get(6)?,
                reason: row.get(7)?,
                created_at: row.get(8)?,
                updated_at: row.get(9)?,
            })
        },
    )
    .optional()
    .with_context(|| format!("failed to read loop state {loop_id}"))
}

pub fn insert_session(conn: &Connection, record: &SessionRecord) -> Result<()> {
    let labels_json: Option<String> = if record.labels.is_empty() {
        None
    } else {
        Some(serde_json::to_string(&record.labels).context("failed to serialize session labels")?)
    };
    conn.execute(
        "INSERT INTO sessions (
            id, project_root, harness, title, status, exit_code, archived_at,
            created_at, updated_at, conversation_id, labels, visibility, familiar_id
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
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
            labels_json,
            &record.visibility,
            &record.familiar_id,
        ],
    )
    .with_context(|| format!("failed to insert session {}", record.id))?;

    Ok(())
}

pub fn insert_session_if_absent(conn: &Connection, record: &SessionRecord) -> Result<bool> {
    let labels_json: Option<String> = if record.labels.is_empty() {
        None
    } else {
        Some(serde_json::to_string(&record.labels).context("failed to serialize session labels")?)
    };
    let affected = conn
        .execute(
            "INSERT OR IGNORE INTO sessions (
                id, project_root, harness, title, status, exit_code, archived_at,
                created_at, updated_at, conversation_id, labels, visibility, familiar_id
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
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
                labels_json,
                &record.visibility,
                &record.familiar_id,
            ],
        )
        .with_context(|| format!("failed to upsert session {}", record.id))?;
    Ok(affected > 0)
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

/// Companion reaper to [`mark_running_sessions_orphaned`]: `coven run`
/// inserts the session row as `created` and only flips it to `running`
/// right before launching the harness. A run process that dies between
/// those two writes (fork exhaustion, missing adapter, crash) leaves a row
/// no process owns, so only age can prove it dead. Rows created before the
/// cutoff become `failed`; newer rows stay untouched so a slow-but-live
/// launch is never clobbered.
pub fn mark_stale_created_sessions_failed(
    conn: &Connection,
    created_before: &str,
    updated_at: &str,
) -> Result<usize> {
    let updated = conn
        .execute(
            "UPDATE sessions
             SET status = 'failed',
                 updated_at = ?2
             WHERE status = 'created' AND created_at < ?1",
            params![created_before, updated_at],
        )
        .context("failed to mark stale created sessions failed")?;
    Ok(updated)
}

pub fn get_session(conn: &Connection, session_id: &str) -> Result<Option<SessionRecord>> {
    let mut statement = conn
        .prepare(&format!(
            "SELECT
                {SESSION_COLUMNS}
            FROM sessions
            WHERE id = ?1",
        ))
        .context("failed to prepare session lookup query")?;

    statement
        .query_row(params![session_id], session_record_from_row)
        .optional()
        .with_context(|| format!("failed to read session {session_id}"))
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
                {SESSION_COLUMNS}
            FROM sessions
            {archive_filter}
            ORDER BY created_at DESC, id DESC",
        ))
        .context("failed to prepare session list query")?;

    let sessions = statement
        .query_map([], session_record_from_row)
        .context("failed to query sessions")?
        .collect::<std::result::Result<Vec<_>, _>>()
        .context("failed to read sessions")?;

    Ok(sessions)
}

const SESSION_COLUMNS: &str = "id,
                project_root,
                harness,
                title,
                status,
                exit_code,
                archived_at,
                created_at,
                updated_at,
                conversation_id,
                labels,
                visibility,
                familiar_id";

fn session_record_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<SessionRecord> {
    let labels_str: Option<String> = row.get(10)?;
    let labels: Vec<String> = labels_str
        .as_deref()
        .map(serde_json::from_str)
        .transpose()
        .map_err(|err| {
            rusqlite::Error::FromSqlConversionFailure(
                10,
                rusqlite::types::Type::Text,
                Box::new(err),
            )
        })?
        .unwrap_or_default();
    let visibility: String = row.get(11)?;
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
        familiar_id: row.get(12)?,
        labels,
        visibility,
    })
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

pub fn latest_active_for_project(conn: &Connection, project_root: &str) -> Result<Option<String>> {
    conn.query_row(
        "SELECT id FROM sessions
         WHERE project_root = ?1 AND archived_at IS NULL
         ORDER BY created_at DESC
         LIMIT 1",
        params![project_root],
        |row| row.get::<_, String>(0),
    )
    .optional()
    .context("failed to query latest active session for project")
}

fn fts_literal_query(query: &str) -> Option<String> {
    let terms = query.split_whitespace().filter(|term| !term.is_empty());
    let mut out = Vec::new();
    for term in terms {
        out.push(format!("\"{}\"", term.replace('"', "\"\"")));
    }
    (!out.is_empty()).then(|| out.join(" "))
}

pub fn search_events(conn: &Connection, query: &str) -> Result<Vec<SearchHit>> {
    let Some(fts_query) = fts_literal_query(query) else {
        return Ok(Vec::new());
    };
    let mut stmt = conn
        .prepare(
            "SELECT e.id, e.session_id, e.kind, snippet(events_fts, 0, '[', ']', '…', 16), e.created_at
             FROM events_fts
             JOIN events e ON e.rowid = events_fts.rowid
             WHERE events_fts MATCH ?1
             ORDER BY e.created_at DESC
             LIMIT 100",
        )
        .context("failed to prepare events_fts search")?;
    let rows = stmt
        .query_map([fts_query], |row| {
            Ok(SearchHit {
                event_id: row.get(0)?,
                session_id: row.get(1)?,
                kind: row.get(2)?,
                snippet: row.get(3)?,
                created_at: row.get(4)?,
            })
        })
        .context("failed to run events_fts search")?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.context("failed to read events_fts row")?);
    }
    Ok(out)
}

pub fn vacuum_store_path(path: &Path) -> Result<StoreVacuumReport> {
    let conn = open_store(path)?;

    let event_index_rebuilt = sqlite_object_exists(&conn, "table", "events_fts")?;
    if event_index_rebuilt {
        conn.execute("INSERT INTO events_fts(events_fts) VALUES('rebuild')", [])
            .context("failed to rebuild events_fts")?;
    }

    conn.execute_batch("PRAGMA optimize; VACUUM;")
        .context("failed to vacuum Coven store")?;
    let _ = conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);");
    let integrity_check = pragma_integrity_check(&conn)?;

    Ok(StoreVacuumReport {
        event_index_rebuilt,
        integrity_check,
    })
}

fn sqlite_object_exists(conn: &Connection, object_type: &str, name: &str) -> Result<bool> {
    conn.query_row(
        "SELECT EXISTS(
            SELECT 1 FROM sqlite_master WHERE type = ?1 AND name = ?2
        )",
        params![object_type, name],
        |row| row.get::<_, bool>(0),
    )
    .with_context(|| format!("failed to inspect sqlite object {name}"))
}

fn pragma_integrity_check(conn: &Connection) -> Result<Vec<String>> {
    let mut stmt = conn
        .prepare("PRAGMA integrity_check")
        .context("failed to prepare integrity_check")?;
    let rows = stmt
        .query_map([], |row| row.get::<_, String>(0))
        .context("failed to run integrity_check")?
        .collect::<std::result::Result<Vec<_>, _>>()
        .context("failed to read integrity_check")?;
    Ok(rows)
}

pub fn insert_event(conn: &Connection, record: &EventRecord) -> Result<()> {
    let config = PrivacyConfig::default();
    let redacted_payload = privacy::redact_payload_json_with_config(&record.payload_json, &config);
    let sensitive = redacted_payload != record.payload_json;
    let redaction_status = if sensitive { "redacted" } else { "clean" };
    insert_event_raw(conn, record, &redacted_payload, redaction_status, sensitive)
}

pub fn insert_event_with_privacy(
    conn: &Connection,
    coven_home: &Path,
    record: &EventRecord,
) -> Result<()> {
    let config = privacy::load_config(coven_home).unwrap_or_default();
    let redacted_payload = privacy::redact_payload_json_with_config(&record.payload_json, &config);
    let sensitive = redacted_payload != record.payload_json;
    let mut redaction_status = if sensitive { "redacted" } else { "clean" };
    insert_event_raw(conn, record, &redacted_payload, redaction_status, sensitive)?;

    if config.persist_raw_artifacts && sensitive {
        let artifact_result = SensitiveArtifactStore::load(coven_home)
            .and_then(|store| {
                store.encrypt(
                    &record.session_id,
                    &record.id,
                    &record.kind,
                    record.payload_json.as_bytes(),
                )
            })
            .and_then(|encrypted| {
                insert_sensitive_artifact(
                    conn,
                    &SensitiveArtifactRecord {
                        id: record.id.clone(),
                        session_id: record.session_id.clone(),
                        event_id: record.id.clone(),
                        kind: record.kind.clone(),
                        nonce: encrypted.nonce,
                        ciphertext: encrypted.ciphertext,
                        created_at: record.created_at.clone(),
                        expires_at: retention_expires_at(
                            &record.created_at,
                            config.raw_artifact_retention_days,
                        ),
                    },
                )
            });
        redaction_status = if artifact_result.is_ok() {
            "redacted_raw_encrypted"
        } else {
            "redacted_raw_unavailable"
        };
        set_event_redaction_status(conn, &record.id, redaction_status)?;
    }

    Ok(())
}

fn insert_event_raw(
    conn: &Connection,
    record: &EventRecord,
    payload_json: &str,
    redaction_status: &str,
    sensitive: bool,
) -> Result<()> {
    conn.execute(
        "INSERT INTO events (
            id,
            session_id,
            kind,
            payload_json,
            created_at,
            redaction_status,
            sensitive
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            &record.id,
            &record.session_id,
            &record.kind,
            payload_json,
            &record.created_at,
            redaction_status,
            if sensitive { 1 } else { 0 },
        ],
    )
    .with_context(|| format!("failed to insert event {}", record.id))?;

    Ok(())
}

fn set_event_redaction_status(conn: &Connection, event_id: &str, status: &str) -> Result<()> {
    conn.execute(
        "UPDATE events SET redaction_status = ?2 WHERE id = ?1",
        params![event_id, status],
    )
    .with_context(|| format!("failed to update redaction status for event {event_id}"))?;
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
        "SELECT rowid AS seq, id, session_id, kind, payload_json, created_at, redaction_status
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
        let mut record = EventRecord {
            seq: row.get(0)?,
            id: row.get(1)?,
            session_id: row.get(2)?,
            kind: row.get(3)?,
            payload_json: row.get(4)?,
            created_at: row.get(5)?,
        };
        let redaction_status: String = row.get(6)?;
        if redaction_status == "legacy" {
            record.payload_json = privacy::redact_payload_json(&record.payload_json);
        }
        Ok(record)
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

pub fn insert_sensitive_artifact(
    conn: &Connection,
    record: &SensitiveArtifactRecord,
) -> Result<()> {
    conn.execute(
        "INSERT INTO sensitive_artifacts (
            id, session_id, event_id, kind, nonce, ciphertext, created_at, expires_at
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            &record.id,
            &record.session_id,
            &record.event_id,
            &record.kind,
            &record.nonce,
            &record.ciphertext,
            &record.created_at,
            &record.expires_at,
        ],
    )
    .with_context(|| format!("failed to insert sensitive artifact {}", record.id))?;
    Ok(())
}

pub fn get_sensitive_artifact(
    conn: &Connection,
    session_id: &str,
    artifact_id: &str,
) -> Result<Option<SensitiveArtifactRecord>> {
    use rusqlite::OptionalExtension;

    conn.query_row(
        "SELECT id, session_id, event_id, kind, nonce, ciphertext, created_at, expires_at
         FROM sensitive_artifacts
         WHERE id = ?1 AND session_id = ?2
         LIMIT 1",
        params![artifact_id, session_id],
        |row| {
            Ok(SensitiveArtifactRecord {
                id: row.get(0)?,
                session_id: row.get(1)?,
                event_id: row.get(2)?,
                kind: row.get(3)?,
                nonce: row.get(4)?,
                ciphertext: row.get(5)?,
                created_at: row.get(6)?,
                expires_at: row.get(7)?,
            })
        },
    )
    .optional()
    .with_context(|| format!("failed to get sensitive artifact {artifact_id}"))
}

#[cfg_attr(not(test), allow(dead_code))]
pub fn count_sensitive_artifacts(conn: &Connection) -> Result<i64> {
    conn.query_row("SELECT COUNT(*) FROM sensitive_artifacts", [], |row| {
        row.get(0)
    })
    .context("failed to count sensitive artifacts")
}

pub fn count_prunable_sensitive_artifacts(
    conn: &Connection,
    now: &str,
    retention_cutoff: &str,
) -> Result<i64> {
    conn.query_row(
        "SELECT COUNT(*) FROM sensitive_artifacts WHERE expires_at < ?1 OR created_at < ?2",
        params![now, retention_cutoff],
        |row| row.get(0),
    )
    .context("failed to count prunable sensitive artifacts")
}

pub fn count_events_older_than(conn: &Connection, cutoff: &str) -> Result<i64> {
    conn.query_row(
        "SELECT COUNT(*) FROM events WHERE created_at < ?1",
        params![cutoff],
        |row| row.get(0),
    )
    .context("failed to count old events")
}

pub fn prune_sensitive_artifacts(
    conn: &Connection,
    now: &str,
    retention_cutoff: &str,
) -> Result<usize> {
    conn.execute(
        "DELETE FROM sensitive_artifacts WHERE expires_at < ?1 OR created_at < ?2",
        params![now, retention_cutoff],
    )
    .context("failed to prune sensitive artifacts")
}

pub fn prune_events_older_than(conn: &Connection, cutoff: &str) -> Result<usize> {
    conn.execute("DELETE FROM events WHERE created_at < ?1", params![cutoff])
        .context("failed to prune events")
}

pub fn retention_cutoff(now: &str, days: u64) -> String {
    let parsed = chrono::DateTime::parse_from_rfc3339(now)
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now());
    (parsed - Duration::days(days as i64)).to_rfc3339_opts(SecondsFormat::Nanos, true)
}

fn retention_expires_at(created_at: &str, days: u64) -> String {
    let parsed = chrono::DateTime::parse_from_rfc3339(created_at)
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now());
    (parsed + Duration::days(days as i64)).to_rfc3339_opts(SecondsFormat::Nanos, true)
}

pub fn artifact_payload(record: &SensitiveArtifactRecord) -> EncryptedPayload {
    EncryptedPayload {
        nonce: record.nonce.clone(),
        ciphertext: record.ciphertext.clone(),
    }
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
    fn node_dispatch_transport_and_last_error_persist_across_reopen() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let path = temp_dir.path().join("coven.db");
        let conn = open_store(&path)?;
        let node = NodeRecord {
            node_id: "node-gpu".to_string(),
            role: "compute_executor".to_string(),
            transport: "ssh".to_string(),
            transport_config_json: Some(r#"{"kind":"ssh","host":"executor.internal"}"#.to_string()),
            capabilities_json: r#"["shell","gpu"]"#.to_string(),
            available: false,
            queue_pressure: 0,
            last_health_at: "2026-07-06T00:00:00Z".to_string(),
            last_error: Some("connection refused".to_string()),
            registered_at: "2026-07-06T00:00:00Z".to_string(),
            updated_at: "2026-07-06T00:00:00Z".to_string(),
        };
        upsert_node(&conn, &node)?;
        drop(conn);

        let reopened = open_store(&path)?;
        let record = get_node(&reopened, "node-gpu")?.expect("node persists");
        assert_eq!(
            record.transport_config_json.as_deref(),
            Some(r#"{"kind":"ssh","host":"executor.internal"}"#)
        );
        assert_eq!(record.last_error.as_deref(), Some("connection refused"));
        Ok(())
    }

    #[test]
    fn executor_dispatch_records_persist_envelopes_across_reopen() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let path = temp_dir.path().join("coven.db");
        let conn = open_store(&path)?;
        let dispatched = ExecutorDispatchRecord {
            job_id: "job-1".to_string(),
            node_id: "node-gpu".to_string(),
            status: "dispatched".to_string(),
            job_json: r#"{"jobId":"job-1"}"#.to_string(),
            envelope_json: None,
            created_at: "2026-07-06T00:00:00Z".to_string(),
            updated_at: "2026-07-06T00:00:00Z".to_string(),
        };
        upsert_executor_dispatch(&conn, &dispatched)?;

        let mut completed = dispatched.clone();
        completed.status = "completed".to_string();
        completed.envelope_json = Some(r#"{"status":"completed"}"#.to_string());
        completed.updated_at = "2026-07-06T00:01:00Z".to_string();
        upsert_executor_dispatch(&conn, &completed)?;
        drop(conn);

        let reopened = open_store(&path)?;
        let record = get_executor_dispatch(&reopened, "job-1")?.expect("dispatch persists");
        assert_eq!(record.status, "completed");
        assert_eq!(
            record.envelope_json.as_deref(),
            Some(r#"{"status":"completed"}"#)
        );
        assert_eq!(record.created_at, "2026-07-06T00:00:00Z");
        assert!(get_executor_dispatch(&reopened, "job-missing")?.is_none());
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
    fn marks_only_stale_created_sessions_failed() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let conn = open_store(&temp_dir.path().join("coven.db"))?;
        let mut stale = session_record("stale-created", "2026-04-27T06:00:00Z");
        stale.status = "created".to_string();
        let mut fresh = session_record("fresh-created", "2026-04-27T06:55:00Z");
        fresh.status = "created".to_string();
        let mut running = session_record("running", "2026-04-27T06:00:00Z");
        running.status = "running".to_string();
        insert_session(&conn, &stale)?;
        insert_session(&conn, &fresh)?;
        insert_session(&conn, &running)?;

        // Cutoff falls between the stale and fresh rows' created_at, so only
        // the stale row is provably dead.
        let updated = mark_stale_created_sessions_failed(
            &conn,
            "2026-04-27T06:50:00Z",
            "2026-04-27T07:00:00Z",
        )?;
        let sessions = list_sessions(&conn)?;
        let by_id = |id: &str| sessions.iter().find(|session| session.id == id).unwrap();

        assert_eq!(updated, 1);
        assert_eq!(by_id("stale-created").status, "failed");
        assert_eq!(by_id("stale-created").updated_at, "2026-04-27T07:00:00Z");
        assert_eq!(by_id("fresh-created").status, "created");
        assert_eq!(by_id("running").status, "running");
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
    fn get_session_reads_only_the_requested_session_row() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let conn = open_store(&temp_dir.path().join("coven.db"))?;
        let target = session_record("target", "2026-04-27T06:00:00Z");
        insert_session(&conn, &target)?;
        conn.execute(
            "INSERT INTO sessions (
                id, project_root, harness, title, status, created_at, updated_at, labels
            ) VALUES (
                'unrelated', '/tmp/coven-project', 'codex', 'Unrelated',
                'active', '2026-04-27T07:00:00Z', '2026-04-27T07:00:00Z', '['
            )",
            [],
        )?;

        assert_eq!(get_session(&conn, "target")?, Some(target));
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
    fn event_schema_adds_privacy_columns_to_existing_store() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let path = temp_dir.path().join("legacy.db");
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
                );
                CREATE TABLE events (
                    id TEXT PRIMARY KEY NOT NULL,
                    session_id TEXT NOT NULL,
                    kind TEXT NOT NULL,
                    payload_json TEXT NOT NULL,
                    created_at TEXT NOT NULL
                );",
            )?;
        }

        let conn = open_store(&path)?;
        let event_columns = table_columns(&conn, "events")?;
        let artifact_columns = table_columns(&conn, "sensitive_artifacts")?;

        assert!(event_columns.contains(&"redaction_status".to_string()));
        assert!(event_columns.contains(&"sensitive".to_string()));
        assert!(artifact_columns.contains(&"ciphertext".to_string()));
        assert!(artifact_columns.contains(&"nonce".to_string()));
        Ok(())
    }

    #[test]
    fn event_insert_stores_redacted_payload_by_default() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let conn = open_store(&temp_dir.path().join("coven.db"))?;
        insert_session(&conn, &session_record("session-1", "2026-04-27T06:00:00Z"))?;
        let fake = fake_openai_key();

        insert_json_event(
            &conn,
            "session-1",
            "input",
            &serde_json::json!({ "data": format!("token={fake}") }),
            "2026-04-27T06:01:00Z",
        )?;

        let (payload, status, sensitive): (String, String, i64) = conn.query_row(
            "SELECT payload_json, redaction_status, sensitive FROM events WHERE id IS NOT NULL",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;
        assert!(!payload.contains(&fake));
        assert!(payload.contains("[REDACTED]"));
        assert_eq!(status, "redacted");
        assert_eq!(sensitive, 1);
        Ok(())
    }

    #[test]
    fn legacy_plaintext_rows_are_redacted_when_listed() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let path = temp_dir.path().join("legacy.db");
        let fake = fake_github_token();
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
                );
                CREATE TABLE events (
                    id TEXT PRIMARY KEY NOT NULL,
                    session_id TEXT NOT NULL,
                    kind TEXT NOT NULL,
                    payload_json TEXT NOT NULL,
                    created_at TEXT NOT NULL
                );",
            )?;
            conn.execute(
                "INSERT INTO sessions (id, project_root, harness, title, status, created_at, updated_at)
                 VALUES ('session-1', '/repo', 'codex', 'Legacy', 'completed', '2026-04-27T06:00:00Z', '2026-04-27T06:00:00Z')",
                [],
            )?;
            conn.execute(
                "INSERT INTO events (id, session_id, kind, payload_json, created_at)
                 VALUES ('event-1', 'session-1', 'output', ?1, '2026-04-27T06:01:00Z')",
                params![
                    serde_json::json!({ "data": format!("Authorization: Bearer {fake}") })
                        .to_string()
                ],
            )?;
        }
        let conn = open_store(&path)?;

        let events = list_events(&conn, "session-1")?;

        assert_eq!(events.len(), 1);
        assert!(!events[0].payload_json.contains(&fake));
        assert!(events[0].payload_json.contains("[REDACTED]"));
        Ok(())
    }

    #[test]
    fn raw_artifacts_are_encrypted_when_explicitly_enabled() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        std::fs::write(
            temp_dir.path().join("privacy.toml"),
            "persist_raw_artifacts = true\nraw_artifact_retention_days = 7\n",
        )?;
        let conn = open_store(&temp_dir.path().join("coven.db"))?;
        insert_session(&conn, &session_record("session-1", "2026-04-27T06:00:00Z"))?;
        let fake = fake_openai_key();
        let raw_payload = serde_json::json!({ "data": format!("secret {fake}") }).to_string();
        let record = EventRecord {
            seq: 0,
            id: "event-raw".to_string(),
            session_id: "session-1".to_string(),
            kind: "output".to_string(),
            payload_json: raw_payload.clone(),
            created_at: "2026-04-27T06:01:00Z".to_string(),
        };

        insert_event_with_privacy(&conn, temp_dir.path(), &record)?;

        let stored_payload: String = conn.query_row(
            "SELECT payload_json FROM events WHERE id = 'event-raw'",
            [],
            |row| row.get(0),
        )?;
        assert!(!stored_payload.contains(&fake));
        let artifact = get_sensitive_artifact(&conn, "session-1", "event-raw")?
            .expect("artifact should exist");
        assert_ne!(artifact.ciphertext, raw_payload.as_bytes());
        let decrypted = crate::encrypted_artifacts::SensitiveArtifactStore::load(temp_dir.path())?
            .decrypt(
                "session-1",
                "event-raw",
                "output",
                &artifact_payload(&artifact),
            )?;
        assert_eq!(String::from_utf8(decrypted)?, raw_payload);
        Ok(())
    }

    #[test]
    fn raw_artifact_key_failure_keeps_redacted_event_only() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        std::fs::write(
            temp_dir.path().join("privacy.toml"),
            "persist_raw_artifacts = true\n",
        )?;
        let keys = temp_dir.path().join("keys");
        std::fs::create_dir_all(&keys)?;
        std::fs::write(keys.join("session-artifacts.key"), "invalid-key-material")?;
        let conn = open_store(&temp_dir.path().join("coven.db"))?;
        insert_session(&conn, &session_record("session-1", "2026-04-27T06:00:00Z"))?;
        let fake = fake_openai_key();
        let record = EventRecord {
            seq: 0,
            id: "event-fail".to_string(),
            session_id: "session-1".to_string(),
            kind: "input".to_string(),
            payload_json: serde_json::json!({ "data": format!("secret {fake}") }).to_string(),
            created_at: "2026-04-27T06:01:00Z".to_string(),
        };

        insert_event_with_privacy(&conn, temp_dir.path(), &record)?;

        let (payload, status): (String, String) = conn.query_row(
            "SELECT payload_json, redaction_status FROM events WHERE id = 'event-fail'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        assert!(!payload.contains(&fake));
        assert_eq!(status, "redacted_raw_unavailable");
        assert_eq!(count_sensitive_artifacts(&conn)?, 0);
        Ok(())
    }

    #[test]
    fn pruning_removes_expired_artifacts_and_old_events() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let conn = open_store(&temp_dir.path().join("coven.db"))?;
        insert_session(&conn, &session_record("session-1", "2026-04-27T06:00:00Z"))?;
        for (id, created_at) in [
            ("old-event", "2026-04-01T00:00:00Z"),
            ("fresh-event", "2026-04-26T00:00:00Z"),
        ] {
            insert_event(
                &conn,
                &EventRecord {
                    seq: 0,
                    id: id.to_string(),
                    session_id: "session-1".to_string(),
                    kind: "output".to_string(),
                    payload_json: serde_json::json!({ "data": id }).to_string(),
                    created_at: created_at.to_string(),
                },
            )?;
        }
        insert_sensitive_artifact(
            &conn,
            &SensitiveArtifactRecord {
                id: "expired".to_string(),
                session_id: "session-1".to_string(),
                event_id: "old-event".to_string(),
                kind: "output".to_string(),
                nonce: vec![0; 24],
                ciphertext: vec![1, 2, 3],
                created_at: "2026-04-01T00:00:00Z".to_string(),
                expires_at: "2026-04-08T00:00:00Z".to_string(),
            },
        )?;

        let pruned_artifacts =
            prune_sensitive_artifacts(&conn, "2026-05-01T00:00:00Z", "2026-04-24T00:00:00Z")?;
        let cutoff = retention_cutoff("2026-05-01T00:00:00Z", 7);
        let pruned_events = prune_events_older_than(&conn, &cutoff)?;

        assert_eq!(pruned_artifacts, 1);
        assert_eq!(pruned_events, 1);
        let events = list_events(&conn, "session-1")?;
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].payload_json, r#"{"data":"fresh-event"}"#);
        Ok(())
    }

    #[test]
    fn pruning_sensitive_artifacts_honors_expires_at_and_created_at_cutoff() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let conn = open_store(&temp_dir.path().join("coven.db"))?;
        insert_session(&conn, &session_record("session-1", "2026-04-27T06:00:00Z"))?;
        insert_event(
            &conn,
            &EventRecord {
                seq: 0,
                id: "event-1".to_string(),
                session_id: "session-1".to_string(),
                kind: "output".to_string(),
                payload_json: serde_json::json!({ "data": "old raw payload" }).to_string(),
                created_at: "2026-04-20T00:00:00Z".to_string(),
            },
        )?;
        insert_sensitive_artifact(
            &conn,
            &SensitiveArtifactRecord {
                id: "older-than-override".to_string(),
                session_id: "session-1".to_string(),
                event_id: "event-1".to_string(),
                kind: "output".to_string(),
                nonce: vec![0; 24],
                ciphertext: vec![1, 2, 3],
                created_at: "2026-04-20T00:00:00Z".to_string(),
                expires_at: "2026-05-04T00:00:00Z".to_string(),
            },
        )?;
        insert_sensitive_artifact(
            &conn,
            &SensitiveArtifactRecord {
                id: "expired-by-record".to_string(),
                session_id: "session-1".to_string(),
                event_id: "event-1".to_string(),
                kind: "output".to_string(),
                nonce: vec![0; 24],
                ciphertext: vec![4, 5, 6],
                created_at: "2026-04-26T00:00:00Z".to_string(),
                expires_at: "2026-04-26T12:00:00Z".to_string(),
            },
        )?;

        let cutoff = retention_cutoff("2026-04-27T00:00:00Z", 1);

        assert_eq!(
            count_prunable_sensitive_artifacts(&conn, "2026-04-27T00:00:00Z", &cutoff)?,
            2
        );
        assert_eq!(
            prune_sensitive_artifacts(&conn, "2026-04-27T00:00:00Z", &cutoff)?,
            2
        );
        assert_eq!(count_sensitive_artifacts(&conn)?, 0);
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
            familiar_id: None,
            labels: Vec::new(),
            visibility: "private".to_string(),
        }
    }

    #[test]
    fn latest_active_returns_newest_non_archived_for_project() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let conn = open_store(&temp.path().join("test.sqlite3"))?;
        conn.execute_batch(
            "INSERT INTO sessions(id, project_root, harness, title, status, created_at, updated_at)
               VALUES ('older', '/p', 'codex', 't', 'created', '2026-01-01', '2026-01-01'),
                      ('newer', '/p', 'claude', 't', 'created', '2026-01-02', '2026-01-02'),
                      ('archived', '/p', 'claude', 't', 'created', '2026-01-03', '2026-01-03'),
                      ('other_proj', '/other', 'claude', 't', 'created', '2026-01-04', '2026-01-04');
             UPDATE sessions SET archived_at='2026-01-03' WHERE id='archived';",
        )?;
        let hit = latest_active_for_project(&conn, "/p")?;
        assert_eq!(hit.as_deref(), Some("newer"));
        Ok(())
    }

    #[test]
    fn search_events_finds_match_in_payload() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let conn = open_store(&temp.path().join("test.sqlite3"))?;
        conn.execute(
            "INSERT INTO sessions(id, project_root, harness, title, status, created_at, updated_at)
             VALUES('s1', '/tmp', 'codex', 't', 'created', '2026-01-01', '2026-01-01')",
            [],
        )?;
        conn.execute(
            "INSERT INTO events(id, session_id, kind, payload_json, created_at)
             VALUES('e1', 's1', 'stdout', '{\"text\":\"phoenix rises\"}', '2026-01-01')",
            [],
        )?;
        let hits = search_events(&conn, "phoenix")?;
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].event_id, "e1");
        assert_eq!(hits[0].session_id, "s1");
        Ok(())
    }

    #[test]
    fn search_events_treats_numeric_colon_query_as_literal() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let conn = open_store(&temp.path().join("test.sqlite3"))?;
        conn.execute(
            "INSERT INTO sessions(id, project_root, harness, title, status, created_at, updated_at)
             VALUES('s1', '/tmp', 'codex', 't', 'created', '2026-01-01', '2026-01-01')",
            [],
        )?;
        conn.execute(
            "INSERT INTO events(id, session_id, kind, payload_json, created_at)
             VALUES('e1', 's1', 'stdout', '{\"text\":\"demo step 0:\"}', '2026-01-01')",
            [],
        )?;
        let hits = search_events(&conn, "0:")?;
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].event_id, "e1");
        Ok(())
    }

    #[test]
    fn open_store_backfills_events_predating_the_fts_index() -> Result<()> {
        // Reproduce a real pre-FTS store: `sessions`/`events` populated before
        // the FTS index and its triggers existed (so the rows were never
        // trigger-indexed). The first `open_store` after upgrade must index
        // them via the backfill — and the *conditional* backfill must behave
        // exactly like the original unconditional one.
        let temp = tempfile::tempdir()?;
        let path = temp.path().join("test.sqlite3");
        let legacy = Connection::open(&path)?;
        legacy.execute_batch(
            "CREATE TABLE sessions (
                 id TEXT PRIMARY KEY NOT NULL, project_root TEXT NOT NULL,
                 harness TEXT NOT NULL, title TEXT NOT NULL, status TEXT NOT NULL,
                 created_at TEXT NOT NULL, updated_at TEXT NOT NULL
             );
             CREATE TABLE events (
                 id TEXT PRIMARY KEY NOT NULL, session_id TEXT NOT NULL,
                 kind TEXT NOT NULL, payload_json TEXT NOT NULL, created_at TEXT NOT NULL
             );
             INSERT INTO sessions(id, project_root, harness, title, status, created_at, updated_at)
                 VALUES('s1', '/tmp', 'codex', 't', 'created', '2026-01-01', '2026-01-01');
             INSERT INTO events(id, session_id, kind, payload_json, created_at)
                 VALUES('e1', 's1', 'stdout', '{\"text\":\"phoenix rises\"}', '2026-01-01');",
        )?;
        drop(legacy);

        // Upgrade open: creates events_fts + triggers, then backfills the
        // pre-existing event (no trigger ever fired for it).
        let upgraded = open_store(&path)?;
        let hits = search_events(&upgraded, "phoenix")?;
        assert_eq!(
            hits.len(),
            1,
            "pre-FTS event should be backfilled into the index"
        );
        assert_eq!(hits[0].event_id, "e1");

        // Re-opening an already-indexed store stays a no-op and keeps working.
        drop(upgraded);
        let reopened = open_store(&path)?;
        assert_eq!(search_events(&reopened, "phoenix")?.len(), 1);
        Ok(())
    }

    #[test]
    fn events_fts_backfill_busy_is_non_fatal() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let path = temp.path().join("test.sqlite3");
        let conn = open_store(&path)?;
        insert_session(&conn, &session_record("session-1", "2026-04-27T06:00:00Z"))?;
        insert_event(
            &conn,
            &EventRecord {
                seq: 0,
                id: "event-1".to_string(),
                session_id: "session-1".to_string(),
                kind: "output".to_string(),
                payload_json: r#"{"data":"phoenix rises"}"#.to_string(),
                created_at: "2026-04-27T06:01:00Z".to_string(),
            },
        )?;
        conn.execute(
            "INSERT INTO events_fts(events_fts) VALUES('delete-all')",
            [],
        )?;
        conn.execute(
            "DELETE FROM store_meta WHERE key = 'events_fts_backfill_complete'",
            [],
        )?;
        conn.execute_batch("PRAGMA busy_timeout = 1")?;

        let locker = Connection::open(&path)?;
        locker.execute_batch("PRAGMA busy_timeout = 1; BEGIN IMMEDIATE")?;

        backfill_events_fts_if_needed(&conn)?;

        let complete: Option<String> = conn
            .query_row(
                "SELECT value FROM store_meta WHERE key = 'events_fts_backfill_complete'",
                [],
                |row| row.get(0),
            )
            .optional()?;
        assert_eq!(complete, None);
        Ok(())
    }

    #[test]
    fn vacuum_rebuilds_stale_event_fts_index() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let path = temp.path().join("test.sqlite3");
        let conn = open_store(&path)?;
        conn.execute(
            "INSERT INTO sessions(id, project_root, harness, title, status, created_at, updated_at)
             VALUES('s1', '/tmp', 'codex', 't', 'created', '2026-01-01', '2026-01-01')",
            [],
        )?;
        conn.execute(
            "INSERT INTO events(id, session_id, kind, payload_json, created_at)
             VALUES('e1', 's1', 'stdout', '{\"text\":\"phoenix rises\"}', '2026-01-01')",
            [],
        )?;
        conn.execute(
            "INSERT INTO events_fts(events_fts) VALUES('delete-all')",
            [],
        )?;
        assert!(search_events(&conn, "phoenix")?.is_empty());
        drop(conn);

        let report = vacuum_store_path(&path)?;

        assert!(report.event_index_rebuilt);
        let conn = open_store(&path)?;
        let hits = search_events(&conn, "phoenix")?;
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].event_id, "e1");
        Ok(())
    }

    #[test]
    fn new_columns_default_correctly() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let conn = open_store(&temp.path().join("test.sqlite3"))?;
        conn.execute(
            "INSERT INTO sessions(id, project_root, harness, title, status, created_at, updated_at)
             VALUES('s1', '/tmp', 'codex', 't', 'created', '2026-01-01', '2026-01-01')",
            [],
        )?;
        let labels: Option<String> =
            conn.query_row("SELECT labels FROM sessions WHERE id='s1'", [], |row| {
                row.get(0)
            })?;
        let visibility: String =
            conn.query_row("SELECT visibility FROM sessions WHERE id='s1'", [], |row| {
                row.get(0)
            })?;
        let familiar_id: Option<String> = conn.query_row(
            "SELECT familiar_id FROM sessions WHERE id='s1'",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(labels, None);
        assert_eq!(visibility, "private");
        assert_eq!(familiar_id, None);
        Ok(())
    }

    #[test]
    fn familiar_id_round_trips_through_insert_and_list() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let conn = open_store(&temp.path().join("test.sqlite3"))?;
        let mut nova = session_record("with-fam", "2026-06-03T00:00:00Z");
        nova.familiar_id = Some("nova".to_string());
        let plain = session_record("no-fam", "2026-06-03T00:00:01Z");
        insert_session(&conn, &nova)?;
        insert_session(&conn, &plain)?;

        let listed = list_sessions(&conn)?;
        let with_fam = listed.iter().find(|s| s.id == "with-fam").unwrap();
        let no_fam = listed.iter().find(|s| s.id == "no-fam").unwrap();
        assert_eq!(with_fam.familiar_id.as_deref(), Some("nova"));
        assert_eq!(no_fam.familiar_id, None);
        Ok(())
    }

    #[test]
    fn familiar_id_index_exists_after_open() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let conn = open_store(&temp.path().join("test.sqlite3"))?;
        // Sanity: column + index were created by open_store / ensure_familiar_id_column.
        let cols = table_columns(&conn, "sessions")?;
        assert!(
            cols.iter().any(|c| c == "familiar_id"),
            "sessions.familiar_id column missing; cols={cols:?}"
        );
        let mut stmt = conn.prepare("SELECT name FROM sqlite_master WHERE type='index'")?;
        let indexes: Vec<String> = stmt
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        assert!(
            indexes.iter().any(|i| i == "idx_sessions_familiar_id"),
            "idx_sessions_familiar_id missing; indexes={indexes:?}"
        );
        Ok(())
    }

    #[test]
    fn legacy_db_without_familiar_id_column_migrates_in_place() -> Result<()> {
        // Simulate a pre-feature store: a session row that pre-dates the
        // familiar_id column. open_store must add the column without
        // dropping or rewriting any existing rows.
        let temp = tempfile::tempdir()?;
        let path = temp.path().join("legacy.sqlite3");
        {
            let legacy = Connection::open(&path)?;
            legacy.execute_batch(
                "CREATE TABLE sessions (
                    id TEXT PRIMARY KEY NOT NULL,
                    project_root TEXT NOT NULL,
                    harness TEXT NOT NULL,
                    title TEXT NOT NULL,
                    status TEXT NOT NULL,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL
                );
                INSERT INTO sessions(id, project_root, harness, title, status, created_at, updated_at)
                  VALUES ('legacy-1', '/tmp', 'codex', 'old', 'completed', '2026-01-01', '2026-01-01');",
            )?;
        }
        let conn = open_store(&path)?;
        let cols = table_columns(&conn, "sessions")?;
        assert!(cols.iter().any(|c| c == "familiar_id"));
        let familiar_id: Option<String> = conn.query_row(
            "SELECT familiar_id FROM sessions WHERE id='legacy-1'",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(familiar_id, None);
        Ok(())
    }

    fn table_columns(conn: &Connection, table: &str) -> Result<Vec<String>> {
        let mut statement = conn.prepare(&format!("PRAGMA table_info({table})"))?;
        let columns = statement
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(anyhow::Error::from)?;
        Ok(columns)
    }

    fn fake_openai_key() -> String {
        format!("sk-{}", "a".repeat(40))
    }

    fn fake_github_token() -> String {
        format!("ghp_{}", "b".repeat(40))
    }
}
