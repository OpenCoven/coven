use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{OnceLock, RwLock},
};

#[cfg(test)]
use std::sync::Mutex;

use anyhow::{bail, Context, Result};
use base64::Engine;
use chrono::{Duration, SecondsFormat, Utc};
use rusqlite::{params, Connection, ErrorCode, OpenFlags, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::{
    encrypted_artifacts::{EncryptedPayload, SensitiveArtifactStore},
    privacy::{self, PrivacyConfig},
};

const FTS_BACKFILL_BATCH_SIZE: i64 = 1_000;
const FTS_BACKFILL_COMPLETE_KEY: &str = "events_fts_backfill_complete";
const MAINTENANCE_LAST_PRUNE_KEY: &str = "maintenance_last_prune_at";
const MAINTENANCE_LAST_CHECKPOINT_KEY: &str = "maintenance_last_checkpoint_at";
const MAINTENANCE_LAST_ERROR_KEY: &str = "maintenance_last_error";
const MAINTENANCE_LAST_ERROR_PUBLIC_MESSAGE: &str = "maintenance pass failed";
const MAINTENANCE_EVENT_BATCH_SIZE: i64 = 500;
const MAINTENANCE_ARTIFACT_BATCH_SIZE: i64 = 500;
const MAINTENANCE_MAX_BATCHES_PER_TICK: i64 = 10;
const MAINTENANCE_CHECKPOINT_WAL_BYTES: u64 = 16 * 1024 * 1024;
pub const MAINTENANCE_MIN_FREE_DISK_BYTES: u64 = 256 * 1024 * 1024;
const MAINTENANCE_WARN_FREE_DISK_BYTES: u64 = 1024 * 1024 * 1024;
const BOUNDED_PRUNE_SENSITIVE_ARTIFACTS_BY_EXPIRY_SQL: &str = "DELETE FROM sensitive_artifacts
     WHERE rowid IN (
        SELECT rowid FROM sensitive_artifacts
        INDEXED BY idx_sensitive_artifacts_expires_at
        WHERE expires_at < ?1
        ORDER BY expires_at, rowid
        LIMIT ?2
     )";
const BOUNDED_PRUNE_SENSITIVE_ARTIFACTS_BY_CREATED_AT_SQL: &str = "DELETE FROM sensitive_artifacts
     WHERE rowid IN (
        SELECT rowid FROM sensitive_artifacts
        INDEXED BY idx_sensitive_artifacts_created_at
        WHERE created_at < ?1
        ORDER BY created_at, rowid
        LIMIT ?2
     )";
const EVENT_NOT_PINNED_BY_UNRESOLVED_HANDOFF_SQL: &str = "NOT EXISTS (
  SELECT 1 FROM session_handoffs AS handoff
  WHERE handoff.session_id = event.session_id
    AND handoff.state IN ('offered', 'claimed')
    AND event.rowid <= handoff.event_cursor
)";
pub const DEFAULT_SESSION_PAGE_LIMIT: usize = 100;
pub const MAX_SESSION_PAGE_LIMIT: usize = 1_000;

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
    /// Immutable `psyche.execution_binding.v1` identity bound at session
    /// creation (see `execution_binding`). `None` for sessions launched
    /// outside Psyche delegation. Coven never mutates this once set — there
    /// is deliberately no update path for this column.
    #[serde(default)]
    pub execution_binding: Option<crate::execution_binding::ExecutionBinding>,
    #[serde(default)]
    pub labels: Vec<String>,
    #[serde(default = "default_visibility")]
    pub visibility: String,
    /// True when the session was launched outside the daemon (e.g. by the
    /// coven engine TUI) and registered via POST /sessions/external. The
    /// daemon does not own the PTY; it only holds the ledger row.
    #[serde(default)]
    pub external: bool,
    /// Absolute path to the transcript file written by an external session.
    /// Only meaningful when `external` is true.
    #[serde(default)]
    pub transcript_path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestAdoptionRecord {
    pub id: String,
    pub adoption_key: Option<String>,
    pub contract: Option<String>,
    pub operation: RequestAdoptionOperation,
    pub request_digest: String,
    pub session_id: String,
    pub execution_binding_json: String,
    pub principal_ref: Option<String>,
    pub project_digest: Option<String>,
    pub graph_id: Option<String>,
    pub node_id: Option<String>,
    pub attempt_id: Option<String>,
    pub adopted_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestAdoptionOperation {
    Launch,
    Input,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AdoptionRetentionError;

impl std::fmt::Display for AdoptionRetentionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(
            "session adoption evidence is retained; sacrifice is unavailable until an approved retention/fence contract resolves it",
        )
    }
}

impl std::error::Error for AdoptionRetentionError {}

#[cfg_attr(not(test), allow(dead_code))]
#[allow(clippy::large_enum_variant)]
#[derive(Debug)]
pub enum AdoptionResolution {
    Absent,
    Replay {
        adoption_id: String,
        session: SessionRecord,
    },
    Conflict {
        field: &'static str,
    },
}

#[derive(Debug, PartialEq, Eq)]
pub enum InputAdoptionResult {
    Adopted {
        adoption_id: String,
        lease_id: String,
    },
    Replay,
    Conflict,
    NotLive,
    HandoffFenced,
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

/// Durable handoff state. A handoff is offered from one source session, then
/// claimed, acknowledged by that source, and finally imported or launched by
/// the destination. Generation is scoped to a source session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HandoffRecord {
    pub id: String,
    pub session_id: String,
    pub generation: i64,
    pub packet_json: String,
    pub event_cursor: i64,
    pub workspace_json: String,
    pub state: String,
    pub claimant: Option<String>,
    pub idempotency_key: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HandoffContinuationRecord {
    pub id: String,
    pub handoff_id: String,
    pub source_session_id: String,
    pub generation: i64,
    pub destination: String,
    pub created_at: String,
}

/// Storage and maintenance state exposed through the daemon health contract.
///
/// The backlog fields mirror the optional live event-writer snapshot supplied
/// by the health route and default to zero when no runtime snapshot exists.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageHealth {
    pub status: String,
    pub database_bytes: u64,
    pub wal_bytes: u64,
    pub oldest_retained_event_at: Option<String>,
    pub last_prune_at: Option<String>,
    pub prune_age_seconds: Option<u64>,
    pub last_checkpoint_at: Option<String>,
    pub checkpoint_age_seconds: Option<u64>,
    pub writer_backlog_events: u64,
    pub writer_backlog_bytes: u64,
    pub free_disk_bytes: u64,
    pub maintenance_blocked: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_maintenance_error: Option<String>,
}

#[derive(Clone)]
struct StorageHealthSnapshot {
    health: StorageHealth,
    retention_lagging: bool,
    degraded: bool,
}

static STORAGE_HEALTH_SNAPSHOTS: OnceLock<RwLock<HashMap<PathBuf, StorageHealthSnapshot>>> =
    OnceLock::new();

fn storage_health_snapshots() -> &'static RwLock<HashMap<PathBuf, StorageHealthSnapshot>> {
    STORAGE_HEALTH_SNAPSHOTS.get_or_init(|| RwLock::new(HashMap::new()))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduledMaintenanceReport {
    pub raw_artifacts_pruned: usize,
    pub events_pruned: usize,
    pub checkpoint_ran: bool,
    pub blocked_by_free_disk: bool,
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
pub struct ExecutorResultEnvelopeRecord {
    pub envelope_id: String,
    pub job_id: String,
    pub node_id: String,
    pub envelope_json: String,
    pub recorded_at: String,
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct WardedSurfaceCommitment {
    familiar_id: String,
    surface: String,
    entry_hash: Vec<u8>,
}

#[derive(Debug, Default)]
pub struct EventsQueryOptions {
    pub after_seq: Option<i64>,
    pub after_event_id: Option<String>,
    pub limit: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EventCandidate {
    pub(crate) seq: i64,
    pub(crate) event_id: Option<String>,
    pub(crate) allocation_bytes: usize,
    pub(crate) encoded_lower_bound_bytes: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EventCandidatePage {
    pub(crate) candidates: Vec<EventCandidate>,
}

fn load_ward_audit_schema_state(conn: &Connection) -> Result<String> {
    use coven_threads_core::WARD_AUDIT_SCHEMA_STATE_SQL;

    conn.query_row(WARD_AUDIT_SCHEMA_STATE_SQL, [], |row| row.get(0))
        .context("failed to fingerprint ward_audit schema")
}

fn execute_guarded_ward_audit_batch(conn: &Connection, sql: &str, operation: &str) -> Result<()> {
    if let Err(error) = conn.execute_batch(sql) {
        if !conn.is_autocommit() {
            if let Err(rollback_error) = conn.execute_batch("ROLLBACK") {
                anyhow::bail!("{operation}: {error}; rollback failed: {rollback_error}");
            }
        }
        return Err(error).with_context(|| operation.to_string());
    }
    Ok(())
}

fn apply_ward_audit_schema_state(conn: &Connection, schema_state: &str) -> Result<()> {
    use coven_threads_core::{
        WARD_AUDIT_MIGRATION_V020_SQL, WARD_AUDIT_SCHEMA_SQL, WARD_AUDIT_SCHEMA_STATE_CURRENT_V020,
        WARD_AUDIT_SCHEMA_STATE_LEGACY_V013, WARD_AUDIT_SCHEMA_STATE_MISSING,
        WARD_AUDIT_SCHEMA_STATE_UNKNOWN,
    };

    match schema_state {
        WARD_AUDIT_SCHEMA_STATE_MISSING => execute_guarded_ward_audit_batch(
            conn,
            WARD_AUDIT_SCHEMA_SQL,
            "failed to initialize ward_audit schema",
        ),
        WARD_AUDIT_SCHEMA_STATE_LEGACY_V013 => {
            match execute_guarded_ward_audit_batch(
                conn,
                WARD_AUDIT_MIGRATION_V020_SQL,
                "failed to migrate legacy ward_audit schema",
            ) {
                Ok(()) => Ok(()),
                Err(migration_error) => match load_ward_audit_schema_state(conn) {
                    Ok(state) if state == WARD_AUDIT_SCHEMA_STATE_CURRENT_V020 => Ok(()),
                    Ok(_) => Err(migration_error),
                    Err(reclassification_error) => Err(migration_error).with_context(|| {
                        format!(
                            "failed to reclassify ward_audit after migration error: \
                             {reclassification_error}"
                        )
                    }),
                },
            }
        }
        WARD_AUDIT_SCHEMA_STATE_CURRENT_V020 => Ok(()),
        WARD_AUDIT_SCHEMA_STATE_UNKNOWN => {
            anyhow::bail!("unsupported ward_audit schema fingerprint")
        }
        _ => anyhow::bail!("unsupported ward_audit schema fingerprint state: {schema_state}"),
    }
}

fn ensure_ward_audit_schema(conn: &Connection) -> Result<()> {
    let schema_state = load_ward_audit_schema_state(conn)?;
    apply_ward_audit_schema_state(conn, &schema_state)
}

fn initialized_store_paths() -> &'static RwLock<HashSet<PathBuf>> {
    static PATHS: OnceLock<RwLock<HashSet<PathBuf>>> = OnceLock::new();
    PATHS.get_or_init(|| RwLock::new(HashSet::new()))
}

fn store_was_initialized(path: &Path) -> bool {
    initialized_store_paths()
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .contains(path)
}

fn remember_initialized_store(path: &Path) {
    initialized_store_paths()
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(path.to_path_buf());
    #[cfg(test)]
    {
        let mut counts = initialization_counts()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *counts.entry(path.to_path_buf()).or_default() += 1;
    }
}

#[cfg(test)]
fn initialization_counts() -> &'static Mutex<HashMap<PathBuf, usize>> {
    static COUNTS: OnceLock<Mutex<HashMap<PathBuf, usize>>> = OnceLock::new();
    COUNTS.get_or_init(|| Mutex::new(HashMap::new()))
}

#[cfg(test)]
fn initialization_count(path: &Path) -> usize {
    initialization_counts()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(path)
        .copied()
        .unwrap_or_default()
}

/// Opens a writable store for a standalone CLI caller. The first open for a
/// path in this process initializes or upgrades it; later opens only configure
/// the connection. Daemon startup calls [`initialize_store`] explicitly before
/// it starts accepting requests, so its request paths always take the latter.
pub fn open_store(path: &Path) -> Result<Connection> {
    if !store_was_initialized(path) {
        initialize_store(path)?;
    }
    open_initialized_store(path)
}

/// Performs the idempotent, write-capable store initialization and migration
/// sequence. Call this before serving requests; ordinary request connections
/// should use [`open_initialized_store`] after this succeeds.
pub fn initialize_store(path: &Path) -> Result<()> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create store directory {}", parent.display()))?;
    }

    let conn = Connection::open(path)
        .with_context(|| format!("failed to open Coven store at {}", path.display()))?;
    configure_initializing_connection(&conn)?;
    // The Ward audit migrator owns its own transaction because a legacy table
    // rebuild must be atomic. Run it before our transaction for the remaining
    // idempotent store schema work; nesting these transactions is invalid in
    // SQLite. Its transaction also serializes concurrent Ward upgrades.
    ensure_ward_audit_schema(&conn)?;
    conn.execute_batch("BEGIN IMMEDIATE")
        .context("failed to acquire SQLite initialization transaction")?;
    let result = initialize_store_schema(&conn);
    match result {
        Ok(()) => conn
            .execute_batch("COMMIT")
            .context("failed to commit SQLite initialization transaction")?,
        Err(error) => {
            let _ = conn.execute_batch("ROLLBACK");
            return Err(error);
        }
    }
    remember_initialized_store(path);
    Ok(())
}

/// Opens a writable connection after [`initialize_store`] has completed. This
/// deliberately omits schema DDL, compatibility checks, FTS backfill, and WAL
/// changes so it is safe for per-request use on the daemon hot path.
pub fn open_initialized_store(path: &Path) -> Result<Connection> {
    let conn = Connection::open(path)
        .with_context(|| format!("failed to open Coven store at {}", path.display()))?;
    configure_runtime_writable_connection(&conn)?;
    Ok(conn)
}

fn initialize_store_schema(conn: &Connection) -> Result<()> {
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
            familiar_id TEXT,
            execution_binding_json TEXT,
            external INTEGER NOT NULL DEFAULT 0,
            transcript_path TEXT
        );

        CREATE TABLE IF NOT EXISTS request_adoptions (
            id TEXT PRIMARY KEY NOT NULL,
            adoption_key TEXT,
            contract TEXT,
            operation TEXT NOT NULL CHECK (operation IN ('launch', 'input')),
            request_digest TEXT NOT NULL,
            session_id TEXT NOT NULL,
            execution_binding_json TEXT NOT NULL,
            principal_ref TEXT,
            project_digest TEXT,
            graph_id TEXT,
            node_id TEXT,
            attempt_id TEXT,
            adopted_at TEXT NOT NULL,
            FOREIGN KEY (session_id) REFERENCES sessions(id) ON DELETE RESTRICT,
            CHECK (
                (adoption_key IS NULL AND contract IS NULL AND operation = 'launch')
                OR
                (adoption_key IS NOT NULL AND contract IS NOT NULL)
            ),
            CHECK (
                (operation = 'launch'
                    AND principal_ref IS NOT NULL
                    AND project_digest IS NOT NULL
                    AND graph_id IS NOT NULL
                    AND node_id IS NOT NULL
                    AND attempt_id IS NOT NULL)
                OR
                (operation = 'input'
                    AND adoption_key IS NOT NULL
                    AND principal_ref IS NULL
                    AND project_digest IS NULL
                    AND graph_id IS NULL
                    AND node_id IS NULL
                    AND attempt_id IS NULL)
            )
        );

        CREATE TABLE IF NOT EXISTS events (
            id TEXT PRIMARY KEY NOT NULL,
            session_id TEXT NOT NULL,
            kind TEXT NOT NULL,
            payload_json TEXT NOT NULL,
            created_at TEXT NOT NULL,
            redaction_status TEXT NOT NULL DEFAULT 'redacted',
            sensitive INTEGER NOT NULL DEFAULT 0,
            request_adoption_id TEXT REFERENCES request_adoptions(id) ON DELETE RESTRICT,
            FOREIGN KEY (session_id) REFERENCES sessions(id) ON DELETE CASCADE
        );

        CREATE INDEX IF NOT EXISTS idx_sessions_created_at
            ON sessions(created_at DESC);

        CREATE INDEX IF NOT EXISTS idx_events_session_created_at
            ON events(session_id, created_at);

        CREATE TABLE IF NOT EXISTS session_handoffs (
            id TEXT PRIMARY KEY NOT NULL,
            session_id TEXT NOT NULL,
            generation INTEGER NOT NULL,
            packet_json TEXT NOT NULL,
            event_cursor INTEGER NOT NULL,
            workspace_json TEXT NOT NULL,
            state TEXT NOT NULL,
            claimant TEXT,
            idempotency_key TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            UNIQUE(session_id, generation),
            FOREIGN KEY (session_id) REFERENCES sessions(id) ON DELETE CASCADE
        );

        CREATE INDEX IF NOT EXISTS idx_session_handoffs_session
            ON session_handoffs(session_id, generation DESC);

        CREATE TABLE IF NOT EXISTS session_input_leases (
            id TEXT PRIMARY KEY NOT NULL,
            session_id TEXT NOT NULL,
            created_at TEXT NOT NULL,
            FOREIGN KEY (session_id) REFERENCES sessions(id) ON DELETE CASCADE
        );

        CREATE INDEX IF NOT EXISTS idx_session_input_leases_session
            ON session_input_leases(session_id);

        CREATE TABLE IF NOT EXISTS handoff_continuations (
            id TEXT PRIMARY KEY NOT NULL,
            handoff_id TEXT NOT NULL,
            source_session_id TEXT NOT NULL,
            generation INTEGER NOT NULL,
            destination TEXT NOT NULL,
            created_at TEXT NOT NULL,
            UNIQUE(handoff_id, destination),
            FOREIGN KEY (handoff_id) REFERENCES session_handoffs(id) ON DELETE CASCADE
        );

        -- Scheduled retention walks this index in bounded oldest-first
        -- batches. The session-scoped index above cannot serve that scan.
        CREATE INDEX IF NOT EXISTS idx_events_created_at
            ON events(created_at);

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

        CREATE INDEX IF NOT EXISTS idx_sensitive_artifacts_created_at
            ON sensitive_artifacts(created_at);

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

        CREATE TABLE IF NOT EXISTS executor_result_envelopes (
            sequence INTEGER PRIMARY KEY AUTOINCREMENT,
            envelope_id TEXT UNIQUE NOT NULL,
            job_id TEXT NOT NULL,
            node_id TEXT NOT NULL,
            envelope_json TEXT NOT NULL,
            recorded_at TEXT NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_executor_result_envelopes_job
            ON executor_result_envelopes(job_id, sequence);

        INSERT OR IGNORE INTO executor_result_envelopes (
            envelope_id,
            job_id,
            node_id,
            envelope_json,
            recorded_at
        )
        SELECT
            'legacy:' || job_id,
            job_id,
            node_id,
            envelope_json,
            updated_at
        FROM executor_dispatches
        WHERE envelope_json IS NOT NULL;

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
    // The per-familiar surface baseline manifest is idempotent. The Ward audit
    // ledger ran before this transaction because legacy migration SQL owns its
    // own transaction.
    conn.execute_batch(crate::threads_gate::WARD_MANIFEST_SCHEMA_SQL)
        .context("failed to initialize ward_manifest schema")?;
    ensure_exit_code_column(conn)?;
    ensure_archived_at_column(conn)?;
    ensure_conversation_id_column(conn)?;
    ensure_event_privacy_columns(conn)?;
    ensure_sensitive_artifacts_table(conn)?;
    ensure_labels_column(conn)?;
    ensure_visibility_column(conn)?;
    ensure_familiar_id_column(conn)?;
    ensure_execution_binding_column(conn)?;
    ensure_request_adoption_event_column(conn)?;
    ensure_request_adoption_indexes_and_triggers(conn)?;
    migrate_historical_request_adoptions(conn)?;
    ensure_node_registry_dispatch_columns(conn)?;
    ensure_session_external_columns(conn)?;

    backfill_events_fts_if_needed(conn)?;

    Ok(())
}

fn configure_initializing_connection(conn: &Connection) -> Result<()> {
    // WAL mode allows concurrent readers alongside a single writer and avoids
    // "database is locked" errors under typical daemon + API concurrency.
    // busy_timeout gives writers up to 5 s to retry before returning SQLITE_BUSY.
    // recursive_triggers must be ON so that the implicit DELETE performed by
    // `INSERT OR REPLACE` / `REPLACE INTO` conflict resolution (including
    // conflicts on a rowid table's hidden rowid) still fires BEFORE/AFTER
    // DELETE triggers such as `request_adoptions_no_delete`. Without it, a
    // raw REPLACE that targets an existing hidden rowid with otherwise fresh
    // logical identities can bypass the `request_adoptions_no_replace`
    // logical-conflict guard entirely.
    conn.execute_batch(
        "PRAGMA busy_timeout = 5000;
         PRAGMA foreign_keys = ON;
         PRAGMA recursive_triggers = ON;",
    )
    .context("failed to configure writable Coven store connection")?;
    enable_wal_with_retry(conn)?;
    Ok(())
}

fn enable_wal_with_retry(conn: &Connection) -> Result<()> {
    const ATTEMPTS: usize = 50;
    const RETRY_DELAY: std::time::Duration = std::time::Duration::from_millis(100);

    for attempt in 0..ATTEMPTS {
        match conn.query_row("PRAGMA journal_mode = WAL", [], |row| {
            row.get::<_, String>(0)
        }) {
            Ok(mode) if mode.eq_ignore_ascii_case("wal") => return Ok(()),
            Ok(mode) => anyhow::bail!("SQLite refused WAL mode and reported `{mode}`"),
            Err(error)
                if matches!(
                    error.sqlite_error_code(),
                    Some(ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked)
                ) && attempt + 1 < ATTEMPTS =>
            {
                std::thread::sleep(RETRY_DELAY);
            }
            Err(error) => return Err(error).context("failed to enable WAL mode for Coven store"),
        }
    }

    unreachable!("WAL retry loop either succeeds or returns its final error")
}

fn configure_runtime_writable_connection(conn: &Connection) -> Result<()> {
    // See `configure_initializing_connection` for why recursive_triggers must
    // be ON: it closes the hidden-rowid REPLACE bypass around the
    // `request_adoptions_no_delete` / `request_adoptions_no_replace` guards.
    conn.execute_batch(
        "PRAGMA busy_timeout = 5000;
         PRAGMA foreign_keys = ON;
         PRAGMA recursive_triggers = ON;",
    )
    .context("failed to configure writable Coven store connection")?;
    Ok(())
}

fn configure_read_only_connection(conn: &Connection) -> Result<()> {
    // Read-only connections cannot write, but recursive_triggers is set
    // consistently here too so every connection path shares one
    // configuration story and no writable path is ever accidentally opened
    // without it.
    conn.execute_batch(
        "PRAGMA busy_timeout = 5000;
         PRAGMA foreign_keys = ON;
         PRAGMA recursive_triggers = ON;",
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

fn ensure_session_external_columns(conn: &Connection) -> Result<()> {
    ensure_column(
        conn,
        "sessions",
        "external",
        "ALTER TABLE sessions ADD COLUMN external INTEGER NOT NULL DEFAULT 0",
    )?;
    ensure_column(
        conn,
        "sessions",
        "transcript_path",
        "ALTER TABLE sessions ADD COLUMN transcript_path TEXT",
    )?;
    ensure_column(
        conn,
        "sessions",
        "transcript_indexed_at",
        "ALTER TABLE sessions ADD COLUMN transcript_indexed_at TEXT",
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
            ON sensitive_artifacts(expires_at);

        CREATE INDEX IF NOT EXISTS idx_sensitive_artifacts_created_at
            ON sensitive_artifacts(created_at);",
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

fn open_existing_store_writable(path: &Path) -> Result<Option<Connection>> {
    if !path.exists() {
        return Ok(None);
    }

    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE)
        .with_context(|| format!("failed to open Coven store writable at {}", path.display()))?;
    configure_runtime_writable_connection(&conn)?;
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

/// The Psyche execution binding is a single immutable value bound at launch:
/// no separate table, no update path, just a nullable column carrying the
/// serialized `psyche.execution_binding.v1` tuple.
fn ensure_execution_binding_column(conn: &Connection) -> Result<()> {
    ensure_column(
        conn,
        "sessions",
        "execution_binding_json",
        "ALTER TABLE sessions ADD COLUMN execution_binding_json TEXT",
    )
}

fn ensure_request_adoption_event_column(conn: &Connection) -> Result<()> {
    ensure_column(
        conn,
        "events",
        "request_adoption_id",
        "ALTER TABLE events ADD COLUMN request_adoption_id TEXT REFERENCES request_adoptions(id) ON DELETE RESTRICT",
    )
}

fn ensure_request_adoption_indexes_and_triggers(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE UNIQUE INDEX IF NOT EXISTS request_adoptions_key
            ON request_adoptions(adoption_key) WHERE adoption_key IS NOT NULL;
         CREATE UNIQUE INDEX IF NOT EXISTS request_adoptions_launch_attempt
            ON request_adoptions(principal_ref, project_digest, graph_id, node_id, attempt_id)
            WHERE operation = 'launch';
         CREATE UNIQUE INDEX IF NOT EXISTS request_adoptions_launch_session
            ON request_adoptions(session_id) WHERE operation = 'launch';
         CREATE INDEX IF NOT EXISTS request_adoptions_session
            ON request_adoptions(session_id);
         CREATE UNIQUE INDEX IF NOT EXISTS events_request_adoption
            ON events(request_adoption_id) WHERE request_adoption_id IS NOT NULL;

         CREATE TRIGGER IF NOT EXISTS events_request_adoption_integrity
         BEFORE INSERT ON events
         WHEN NEW.request_adoption_id IS NOT NULL
         BEGIN
           SELECT CASE WHEN NEW.kind != 'input' OR NOT EXISTS (
             SELECT 1 FROM request_adoptions
             WHERE id = NEW.request_adoption_id AND operation = 'input' AND session_id = NEW.session_id
           ) THEN RAISE(ABORT, 'invalid request adoption event correlation') END;
         END;

         CREATE TRIGGER IF NOT EXISTS events_request_adoption_update_integrity
         BEFORE UPDATE OF session_id, kind, request_adoption_id ON events
         WHEN NEW.request_adoption_id IS NOT NULL
         BEGIN
           SELECT CASE WHEN NEW.kind != 'input' OR NOT EXISTS (
             SELECT 1 FROM request_adoptions
             WHERE id = NEW.request_adoption_id AND operation = 'input' AND session_id = NEW.session_id
           ) THEN RAISE(ABORT, 'invalid request adoption event correlation') END;
         END;

         CREATE TRIGGER IF NOT EXISTS events_request_adoption_no_rebind
         BEFORE UPDATE OF session_id, kind, request_adoption_id ON events
         WHEN (OLD.request_adoption_id IS NOT NULL OR NEW.request_adoption_id IS NOT NULL)
           AND (NEW.request_adoption_id IS NOT OLD.request_adoption_id OR NEW.session_id IS NOT OLD.session_id OR NEW.kind IS NOT OLD.kind)
         BEGIN
           SELECT RAISE(ABORT, 'request adoption event correlation is immutable');
         END;

         CREATE TRIGGER IF NOT EXISTS request_adoptions_no_update
         BEFORE UPDATE ON request_adoptions
         BEGIN
           SELECT RAISE(ABORT, 'request adoptions are immutable');
         END;

         CREATE TRIGGER IF NOT EXISTS request_adoptions_no_delete
         BEFORE DELETE ON request_adoptions
         BEGIN
           SELECT RAISE(ABORT, 'request adoptions are retained');
         END;

         CREATE TRIGGER IF NOT EXISTS request_adoptions_no_replace
         BEFORE INSERT ON request_adoptions
         BEGIN
           SELECT CASE
             WHEN EXISTS (
               SELECT 1 FROM request_adoptions WHERE id = NEW.id
             ) THEN RAISE(ABORT, 'request adoptions are retained')
             WHEN NEW.adoption_key IS NOT NULL AND EXISTS (
               SELECT 1 FROM request_adoptions WHERE adoption_key = NEW.adoption_key
             ) THEN RAISE(ABORT, 'request adoptions are retained')
             WHEN NEW.operation = 'launch' AND EXISTS (
               SELECT 1 FROM request_adoptions
               WHERE operation = 'launch'
                 AND principal_ref = NEW.principal_ref
                 AND project_digest = NEW.project_digest
                 AND graph_id = NEW.graph_id
                 AND node_id = NEW.node_id
                 AND attempt_id = NEW.attempt_id
             ) THEN RAISE(ABORT, 'request adoptions are retained')
             WHEN NEW.operation = 'launch' AND EXISTS (
               SELECT 1 FROM request_adoptions
               WHERE operation = 'launch' AND session_id = NEW.session_id
             ) THEN RAISE(ABORT, 'request adoptions are retained')
           END;
         END;",
    )
    .context("failed to initialize request adoption indexes and triggers")
}

fn historical_request_adoption_id(session_id: &str) -> String {
    let namespace = uuid::Uuid::new_v5(
        &uuid::Uuid::NAMESPACE_URL,
        b"https://opencoven.dev/psyche/o3/request-adoptions",
    );
    uuid::Uuid::new_v5(
        &namespace,
        format!("historical-launch:{session_id}").as_bytes(),
    )
    .to_string()
}

fn migrate_historical_request_adoptions(conn: &Connection) -> Result<()> {
    let sessions = {
        let mut statement = conn
            .prepare(
                "SELECT id, execution_binding_json, created_at
                 FROM sessions
                 WHERE execution_binding_json IS NOT NULL
                 ORDER BY id",
            )
            .context("failed to prepare historical request adoption migration")?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .context("failed to query historical bound sessions")?
            .collect::<std::result::Result<Vec<_>, _>>()
            .context("failed to read historical bound sessions")?;
        rows
    };

    for (session_id, stored_json, created_at) in sessions {
        let binding = parse_stored_execution_binding(&stored_json)
            .map_err(|_| anyhow::anyhow!("failed to parse historical session execution binding"))?;
        let deterministic_json = serde_json::to_string(&binding)
            .context("failed to serialize historical session execution binding")?;
        if deterministic_json != stored_json {
            bail!("historical session execution binding is not deterministic");
        }

        if load_launch_adoption_for_session(conn, &session_id)?.is_some() {
            continue;
        }

        conn.execute(
            "INSERT INTO request_adoptions (
                id, adoption_key, contract, operation, request_digest, session_id,
                execution_binding_json, principal_ref, project_digest, graph_id,
                node_id, attempt_id, adopted_at
             ) VALUES (
                ?1, NULL, NULL, 'launch', ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10
             )",
            params![
                historical_request_adoption_id(&session_id),
                &binding.request_digest,
                &session_id,
                deterministic_json,
                &binding.principal_ref,
                &binding.project_digest,
                &binding.graph_id,
                &binding.node_id,
                &binding.attempt_id,
                &created_at,
            ],
        )
        .context("failed to migrate historical request adoption")?;
    }
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

pub fn insert_executor_dispatch_if_absent(
    conn: &Connection,
    record: &ExecutorDispatchRecord,
) -> Result<bool> {
    let affected = conn
        .execute(
            "INSERT INTO executor_dispatches (
                job_id,
                node_id,
                status,
                job_json,
                envelope_json,
                created_at,
                updated_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
            ON CONFLICT(job_id) DO NOTHING",
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
        .with_context(|| format!("failed to insert executor dispatch {}", record.job_id))?;
    Ok(affected == 1)
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

pub fn append_executor_result_envelope(
    conn: &Connection,
    record: &ExecutorResultEnvelopeRecord,
) -> Result<bool> {
    let affected = conn
        .execute(
            "INSERT INTO executor_result_envelopes (
                envelope_id,
                job_id,
                node_id,
                envelope_json,
                recorded_at
            ) VALUES (?1, ?2, ?3, ?4, ?5)
            ON CONFLICT(envelope_id) DO NOTHING",
            params![
                &record.envelope_id,
                &record.job_id,
                &record.node_id,
                &record.envelope_json,
                &record.recorded_at,
            ],
        )
        .with_context(|| {
            format!(
                "failed to append executor result envelope {}",
                record.envelope_id
            )
        })?;
    Ok(affected == 1)
}

pub fn list_executor_result_envelopes(
    conn: &Connection,
    job_id: &str,
) -> Result<Vec<ExecutorResultEnvelopeRecord>> {
    let mut statement = conn
        .prepare(
            "SELECT envelope_id, job_id, node_id, envelope_json, recorded_at
             FROM executor_result_envelopes
             WHERE job_id = ?1
             ORDER BY sequence",
        )
        .with_context(|| format!("failed to prepare executor result envelope list {job_id}"))?;
    let rows = statement
        .query_map(params![job_id], |row| {
            Ok(ExecutorResultEnvelopeRecord {
                envelope_id: row.get(0)?,
                job_id: row.get(1)?,
                node_id: row.get(2)?,
                envelope_json: row.get(3)?,
                recorded_at: row.get(4)?,
            })
        })
        .with_context(|| format!("failed to list executor result envelopes {job_id}"))?;
    let mut records = Vec::new();
    for row in rows {
        records.push(
            row.with_context(|| format!("failed to read executor result envelope {job_id}"))?,
        );
    }
    Ok(records)
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
    let execution_binding_json: Option<String> = record
        .execution_binding
        .as_ref()
        .map(serde_json::to_string)
        .transpose()
        .context("failed to serialize session execution binding")?;
    conn.execute(
        "INSERT INTO sessions (
            id, project_root, harness, title, status, exit_code, archived_at,
            created_at, updated_at, conversation_id, labels, visibility, familiar_id,
            external, transcript_path, execution_binding_json
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
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
            record.external as i32,
            &record.transcript_path,
            execution_binding_json,
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
    let execution_binding_json: Option<String> = record
        .execution_binding
        .as_ref()
        .map(serde_json::to_string)
        .transpose()
        .context("failed to serialize session execution binding")?;
    let affected = conn
        .execute(
            "INSERT OR IGNORE INTO sessions (
                id, project_root, harness, title, status, exit_code, archived_at,
                created_at, updated_at, conversation_id, labels, visibility, familiar_id,
                external, transcript_path, execution_binding_json
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
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
                record.external as i32,
                &record.transcript_path,
                execution_binding_json,
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

pub fn update_session_status_if_current(
    conn: &Connection,
    session_id: &str,
    current_status: &str,
    status: &str,
    exit_code: Option<i32>,
    updated_at: &str,
) -> Result<bool> {
    let affected = conn
        .execute(
            "UPDATE sessions
             SET status = ?3,
                 exit_code = ?4,
                 updated_at = ?5
             WHERE id = ?1 AND status = ?2",
            params![session_id, current_status, status, exit_code, updated_at],
        )
        .with_context(|| format!("failed to update session {session_id}"))?;

    Ok(affected > 0)
}

pub fn update_session_terminal_if_active(
    conn: &Connection,
    session_id: &str,
    status: &str,
    exit_code: Option<i32>,
    updated_at: &str,
) -> Result<bool> {
    if !matches!(
        status,
        "completed" | "failed" | "cancelled" | "killed" | "idle" | "orphaned"
    ) {
        bail!("invalid terminal session status `{status}`");
    }
    let affected = conn
        .execute(
            "UPDATE sessions
             SET status = ?2, exit_code = ?3, updated_at = ?4
             WHERE id = ?1 AND status IN ('created', 'running')",
            params![session_id, status, exit_code, updated_at],
        )
        .with_context(|| format!("failed to update session {session_id}"))?;
    Ok(affected > 0)
}

/// Persist the harness-native id that continues a multi-turn conversation.
///
/// Coven's `id` remains the stable ledger/session id exposed to callers.
/// Harnesses such as Codex mint a different id for their own resume API, so
/// callers can safely keep passing the Coven id while the runner resolves the
/// native conversation id internally.
pub fn update_session_conversation_id(
    conn: &Connection,
    session_id: &str,
    conversation_id: &str,
    updated_at: &str,
) -> Result<()> {
    conn.execute(
        "UPDATE sessions
         SET conversation_id = ?2,
             updated_at = ?3
         WHERE id = ?1",
        params![session_id, conversation_id, updated_at],
    )
    .with_context(|| format!("failed to update conversation id for session {session_id}"))?;

    Ok(())
}

pub fn mark_running_sessions_orphaned(conn: &Connection, updated_at: &str) -> Result<usize> {
    let updated = conn
        .execute(
            "UPDATE sessions
             SET status = 'orphaned',
                 updated_at = ?1
             WHERE status = 'running'
               AND external = 0",
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
/// launch is never clobbered. A launch adoption or historical launch
/// reservation is durable ownership evidence, so those rows are excluded
/// regardless of age.
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
             WHERE status = 'created' AND created_at < ?1
               AND NOT EXISTS (
                 SELECT 1 FROM request_adoptions
                 WHERE request_adoptions.session_id = sessions.id
                  AND request_adoptions.operation = 'launch'
               )",
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

/// Resolve the most recently updated ledger row for a harness-native
/// conversation id. This keeps `--continue <Codex thread id>` compatible with
/// callers that persist the native id instead of Coven's ledger id.
pub fn get_latest_session_by_conversation_id(
    conn: &Connection,
    conversation_id: &str,
) -> Result<Option<SessionRecord>> {
    let mut statement = conn
        .prepare(&format!(
            "SELECT
                {SESSION_COLUMNS}
            FROM sessions
            WHERE conversation_id = ?1
            ORDER BY updated_at DESC, created_at DESC
            LIMIT 1",
        ))
        .context("failed to prepare conversation session lookup query")?;

    statement
        .query_row(params![conversation_id], session_record_from_row)
        .optional()
        .with_context(|| format!("failed to read conversation {conversation_id}"))
}

pub fn list_sessions(conn: &Connection) -> Result<Vec<SessionRecord>> {
    list_sessions_with_archive_filter(conn, false)
}

pub fn list_sessions_including_archived(conn: &Connection) -> Result<Vec<SessionRecord>> {
    list_sessions_with_archive_filter(conn, true)
}

#[derive(Debug, Clone, Copy)]
pub struct SessionListQuery<'a> {
    pub limit: usize,
    pub cursor: Option<&'a str>,
    pub include_archived: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionPage {
    pub sessions: Vec<SessionRecord>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct SessionCursor {
    created_at: String,
    id: String,
}

pub fn list_session_page(conn: &Connection, query: SessionListQuery<'_>) -> Result<SessionPage> {
    if query.limit == 0 || query.limit > MAX_SESSION_PAGE_LIMIT {
        anyhow::bail!(
            "session page limit must be between 1 and {MAX_SESSION_PAGE_LIMIT}, got {}",
            query.limit
        );
    }
    let cursor = query.cursor.map(decode_session_cursor).transpose()?;
    let archive_filter = if query.include_archived {
        ""
    } else {
        "WHERE archived_at IS NULL"
    };
    let cursor_filter = if cursor.is_some() {
        if query.include_archived {
            "WHERE (created_at < ?1 OR (created_at = ?1 AND id < ?2))"
        } else {
            "AND (created_at < ?1 OR (created_at = ?1 AND id < ?2))"
        }
    } else {
        ""
    };
    let mut statement = conn
        .prepare(&format!(
            "SELECT
                {SESSION_COLUMNS}
            FROM sessions
            {archive_filter}
            {cursor_filter}
            ORDER BY created_at DESC, id DESC
            LIMIT ?3"
        ))
        .context("failed to prepare paginated session list query")?;
    let limit = i64::try_from(query.limit + 1).expect("bounded session page limit fits i64");
    let mut sessions = match cursor {
        Some(cursor) => statement
            .query_map(
                params![cursor.created_at, cursor.id, limit],
                session_record_from_row,
            )
            .context("failed to query paginated sessions")?
            .collect::<std::result::Result<Vec<_>, _>>()
            .context("failed to read paginated sessions")?,
        None => statement
            .query_map(params!["", "", limit], session_record_from_row)
            .context("failed to query paginated sessions")?
            .collect::<std::result::Result<Vec<_>, _>>()
            .context("failed to read paginated sessions")?,
    };
    let has_next_page = sessions.len() > query.limit;
    sessions.truncate(query.limit);
    let next_cursor = has_next_page
        .then(|| sessions.last())
        .flatten()
        .map(encode_session_cursor)
        .transpose()?;
    Ok(SessionPage {
        sessions,
        next_cursor,
    })
}

fn encode_session_cursor(session: &SessionRecord) -> Result<String> {
    let cursor = SessionCursor {
        created_at: session.created_at.clone(),
        id: session.id.clone(),
    };
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(serde_json::to_vec(&cursor).context("failed to encode session cursor")?))
}

fn decode_session_cursor(cursor: &str) -> Result<SessionCursor> {
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(cursor)
        .context("session cursor is not valid URL-safe base64")?;
    serde_json::from_slice(&bytes).context("session cursor is malformed")
}

pub fn validate_session_cursor(cursor: &str) -> Result<()> {
    decode_session_cursor(cursor).map(|_| ())
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

// NOTE: column ORDER here must stay in sync with the positional indices in
// `session_record_from_row`; append new columns at the END only.
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
                familiar_id,
                external,
                transcript_path,
                execution_binding_json";

/// Deserializes and fully validates a non-null stored `execution_binding_json`
/// value. Never returns a partially-trusted binding: invalid JSON, an
/// unsupported contract, or an invalid digest/shape are all reported as a
/// conversion failure rather than silently collapsing to `None`. Deliberately
/// does not recheck expiry — that is a launch-time/read-time policy decision
/// for callers, not a store invariant.
fn parse_stored_execution_binding(
    raw: &str,
) -> Result<crate::execution_binding::ExecutionBinding, Box<dyn std::error::Error + Send + Sync>> {
    let value: serde_json::Value = serde_json::from_str(raw)?;
    crate::execution_binding::parse(&value).map_err(|err| Box::new(err) as _)
}

const REQUEST_ADOPTION_COLUMNS: &str = "id,
    adoption_key,
    contract,
    operation,
    request_digest,
    session_id,
    execution_binding_json,
    principal_ref,
    project_digest,
    graph_id,
    node_id,
    attempt_id,
    adopted_at";

#[derive(Debug)]
struct RawRequestAdoptionRecord {
    id: String,
    adoption_key: Option<String>,
    contract: Option<String>,
    operation: String,
    request_digest: String,
    session_id: String,
    execution_binding_json: String,
    principal_ref: Option<String>,
    project_digest: Option<String>,
    graph_id: Option<String>,
    node_id: Option<String>,
    attempt_id: Option<String>,
    adopted_at: String,
}

fn raw_request_adoption_from_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<RawRequestAdoptionRecord> {
    Ok(RawRequestAdoptionRecord {
        id: row.get(0)?,
        adoption_key: row.get(1)?,
        contract: row.get(2)?,
        operation: row.get(3)?,
        request_digest: row.get(4)?,
        session_id: row.get(5)?,
        execution_binding_json: row.get(6)?,
        principal_ref: row.get(7)?,
        project_digest: row.get(8)?,
        graph_id: row.get(9)?,
        node_id: row.get(10)?,
        attempt_id: row.get(11)?,
        adopted_at: row.get(12)?,
    })
}

fn valid_request_digest(value: &str) -> bool {
    value.len() == 71
        && value.starts_with("sha256:")
        && value[7..]
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn invalid_stored_request_adoption(field: &'static str) -> anyhow::Error {
    anyhow::anyhow!("invalid stored request adoption at {field}")
}

fn strict_request_adoption_record(
    conn: &Connection,
    raw: RawRequestAdoptionRecord,
) -> Result<RequestAdoptionRecord> {
    let operation = match raw.operation.as_str() {
        "launch" => RequestAdoptionOperation::Launch,
        "input" => RequestAdoptionOperation::Input,
        _ => return Err(invalid_stored_request_adoption("operation")),
    };
    if !valid_request_digest(&raw.request_digest) {
        return Err(invalid_stored_request_adoption(
            "requestAdoption.requestDigest",
        ));
    }

    match (&raw.adoption_key, &raw.contract) {
        (None, None) if operation == RequestAdoptionOperation::Launch => {}
        (Some(key), Some(contract)) => {
            crate::request_adoption::RequestAdoption {
                contract: contract.clone(),
                key: key.clone(),
                request_digest: raw.request_digest.clone(),
            }
            .validate()
            .map_err(|_| invalid_stored_request_adoption("requestAdoption"))?;
        }
        _ => return Err(invalid_stored_request_adoption("requestAdoption")),
    }
    if operation == RequestAdoptionOperation::Input && raw.adoption_key.is_none() {
        return Err(invalid_stored_request_adoption("requestAdoption.key"));
    }

    let binding = parse_stored_execution_binding(&raw.execution_binding_json)
        .map_err(|_| invalid_stored_request_adoption("executionBinding"))?;
    let deterministic_json = serde_json::to_string(&binding)
        .context("failed to serialize stored request adoption execution binding")?;
    if deterministic_json != raw.execution_binding_json {
        return Err(invalid_stored_request_adoption("executionBinding"));
    }

    match operation {
        RequestAdoptionOperation::Launch => {
            let scope_matches = raw.principal_ref.as_deref() == Some(&binding.principal_ref)
                && raw.project_digest.as_deref() == Some(&binding.project_digest)
                && raw.graph_id.as_deref() == Some(&binding.graph_id)
                && raw.node_id.as_deref() == Some(&binding.node_id)
                && raw.attempt_id.as_deref() == Some(&binding.attempt_id);
            if !scope_matches {
                return Err(invalid_stored_request_adoption(
                    "executionBinding.attemptId",
                ));
            }
            if raw.request_digest != binding.request_digest {
                return Err(invalid_stored_request_adoption(
                    "requestAdoption.requestDigest",
                ));
            }
        }
        RequestAdoptionOperation::Input => {
            if raw.principal_ref.is_some()
                || raw.project_digest.is_some()
                || raw.graph_id.is_some()
                || raw.node_id.is_some()
                || raw.attempt_id.is_some()
            {
                return Err(invalid_stored_request_adoption("executionBinding"));
            }
        }
    }

    let session_binding = conn
        .query_row(
            "SELECT execution_binding_json FROM sessions WHERE id = ?1",
            [&raw.session_id],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()
        .context("failed to validate request adoption session")?;
    match session_binding {
        Some(Some(stored)) if stored == raw.execution_binding_json => {}
        _ => return Err(invalid_stored_request_adoption("sessionId")),
    }

    Ok(RequestAdoptionRecord {
        id: raw.id,
        adoption_key: raw.adoption_key,
        contract: raw.contract,
        operation,
        request_digest: raw.request_digest,
        session_id: raw.session_id,
        execution_binding_json: raw.execution_binding_json,
        principal_ref: raw.principal_ref,
        project_digest: raw.project_digest,
        graph_id: raw.graph_id,
        node_id: raw.node_id,
        attempt_id: raw.attempt_id,
        adopted_at: raw.adopted_at,
    })
}

fn load_request_adoption<P>(
    conn: &Connection,
    predicate: &str,
    params: P,
) -> Result<Option<RequestAdoptionRecord>>
where
    P: rusqlite::Params,
{
    let sql = format!(
        "SELECT {REQUEST_ADOPTION_COLUMNS}
         FROM request_adoptions
         WHERE {predicate}
         LIMIT 1"
    );
    let raw = conn
        .query_row(&sql, params, raw_request_adoption_from_row)
        .optional()
        .context("failed to read request adoption")?;
    raw.map(|record| strict_request_adoption_record(conn, record))
        .transpose()
}

#[cfg_attr(not(test), allow(dead_code))]
fn load_request_adoption_by_key(
    conn: &Connection,
    adoption_key: &str,
) -> Result<Option<RequestAdoptionRecord>> {
    load_request_adoption(conn, "adoption_key = ?1", [adoption_key])
}

#[cfg_attr(not(test), allow(dead_code))]
fn load_launch_adoption_by_scope(
    conn: &Connection,
    binding: &crate::execution_binding::ExecutionBinding,
) -> Result<Option<RequestAdoptionRecord>> {
    load_request_adoption(
        conn,
        "operation = 'launch'
         AND principal_ref = ?1
         AND project_digest = ?2
         AND graph_id = ?3
         AND node_id = ?4
         AND attempt_id = ?5",
        params![
            &binding.principal_ref,
            &binding.project_digest,
            &binding.graph_id,
            &binding.node_id,
            &binding.attempt_id,
        ],
    )
}

fn load_launch_adoption_for_session(
    conn: &Connection,
    session_id: &str,
) -> Result<Option<RequestAdoptionRecord>> {
    load_request_adoption(
        conn,
        "operation = 'launch' AND session_id = ?1",
        [session_id],
    )
}

#[cfg(test)]
fn load_request_adoption_by_id(
    conn: &Connection,
    adoption_id: &str,
) -> Result<Option<RequestAdoptionRecord>> {
    load_request_adoption(conn, "id = ?1", [adoption_id])
}

// O3 forbids leaking ledger session ids (or any other retained identity —
// adoption id/key/digest/binding) into errors and diagnostics. `get_session`'s
// error path interpolates the session id into its message (see
// `failed to read session {session_id}`), so any lookup or readback failure
// at this shared replay boundary — used by both `resolve_launch_adoption`
// and `resolve_input_adoption` — is replaced with a static internal error
// rather than propagated. Adding context on top would not be enough: the
// original message would still render in the debug/`{:#}` chain, so the
// source error is discarded, not wrapped.
#[cfg_attr(not(test), allow(dead_code))]
fn replay_resolution(
    conn: &Connection,
    record: RequestAdoptionRecord,
) -> Result<AdoptionResolution> {
    let session = get_session(conn, &record.session_id)
        .map_err(|_| invalid_stored_request_adoption("sessionId"))?
        .ok_or_else(|| invalid_stored_request_adoption("sessionId"))?;
    Ok(AdoptionResolution::Replay {
        adoption_id: record.id,
        session,
    })
}

// O3 contract §6: a row matched by the submitted global adoption key that is
// not an exact replay is a key collision. The response must report only
// `requestAdoption.key` — it must not leak which hidden identity member
// (contract, operation, digest, session, or binding) actually differs.
// Distinct scope from a *different* key colliding with an in-flight launch
// attempt (see `load_launch_adoption_by_scope`), which stays reported at
// `executionBinding.attemptId`.
#[cfg_attr(not(test), allow(dead_code))]
fn keyed_adoption_mismatch(
    record: &RequestAdoptionRecord,
    operation: RequestAdoptionOperation,
    session_id: Option<&str>,
    request: &crate::request_adoption::RequestAdoption,
    binding: &crate::execution_binding::ExecutionBinding,
) -> Result<Option<&'static str>> {
    if record.operation != operation {
        return Ok(Some("requestAdoption.key"));
    }
    if record.contract.as_deref() != Some(request.contract.as_str()) {
        return Ok(Some("requestAdoption.key"));
    }
    if record.request_digest != request.request_digest {
        return Ok(Some("requestAdoption.key"));
    }
    if operation == RequestAdoptionOperation::Input
        && record.session_id != session_id.expect("input resolution supplies session id")
    {
        return Ok(Some("requestAdoption.key"));
    }
    let stored_binding = parse_stored_execution_binding(&record.execution_binding_json)
        .map_err(|_| invalid_stored_request_adoption("executionBinding"))?;
    if stored_binding.first_mismatch_path(binding).is_some() {
        return Ok(Some("requestAdoption.key"));
    }
    Ok(None)
}

#[cfg(test)]
thread_local! {
    // The exact key-miss/scope-read seam needed to reproduce a split-view
    // resolver without exposing a production hook.
    static LAUNCH_ADOPTION_KEY_MISS_TEST_HOOK:
        std::cell::RefCell<Option<Box<dyn FnOnce()>>> = std::cell::RefCell::new(None);
}

#[cfg(test)]
pub(crate) fn set_launch_adoption_key_miss_test_hook(hook: Option<Box<dyn FnOnce()>>) {
    LAUNCH_ADOPTION_KEY_MISS_TEST_HOOK.with(|cell| *cell.borrow_mut() = hook);
}

#[cfg(test)]
fn launch_adoption_key_miss_test_hook() {
    let hook = LAUNCH_ADOPTION_KEY_MISS_TEST_HOOK.with(|cell| cell.borrow_mut().take());
    if let Some(hook) = hook {
        hook();
    }
}

#[cfg_attr(not(test), allow(dead_code))]
pub fn resolve_launch_adoption(
    conn: &Connection,
    request: &crate::request_adoption::RequestAdoption,
    binding: &crate::execution_binding::ExecutionBinding,
) -> Result<AdoptionResolution> {
    if let Some(record) = load_request_adoption_by_key(conn, &request.key)? {
        if let Some(field) = keyed_adoption_mismatch(
            &record,
            RequestAdoptionOperation::Launch,
            None,
            request,
            binding,
        )? {
            return Ok(AdoptionResolution::Conflict { field });
        }
        return replay_resolution(conn, record);
    }
    #[cfg(test)]
    launch_adoption_key_miss_test_hook();
    if load_launch_adoption_by_scope(conn, binding)?.is_some() {
        return Ok(AdoptionResolution::Conflict {
            field: "executionBinding.attemptId",
        });
    }
    Ok(AdoptionResolution::Absent)
}

#[cfg_attr(not(test), allow(dead_code))]
pub fn resolve_input_adoption(
    conn: &Connection,
    session_id: &str,
    request: &crate::request_adoption::RequestAdoption,
    binding: &crate::execution_binding::ExecutionBinding,
) -> Result<AdoptionResolution> {
    let Some(record) = load_request_adoption_by_key(conn, &request.key)? else {
        return Ok(AdoptionResolution::Absent);
    };
    if let Some(field) = keyed_adoption_mismatch(
        &record,
        RequestAdoptionOperation::Input,
        Some(session_id),
        request,
        binding,
    )? {
        return Ok(AdoptionResolution::Conflict { field });
    }
    replay_resolution(conn, record)
}

#[cfg_attr(not(test), allow(dead_code))]
fn validated_adoption_binding_json(
    conn: &Connection,
    session_id: &str,
    request: &crate::request_adoption::RequestAdoption,
    binding: &crate::execution_binding::ExecutionBinding,
) -> Result<String> {
    request
        .validate()
        .context("invalid request adoption identity")?;
    binding
        .validate_shape()
        .context("invalid request adoption execution binding")?;
    let binding_json =
        serde_json::to_string(binding).context("failed to serialize request adoption binding")?;
    let session_binding = conn
        .query_row(
            "SELECT execution_binding_json FROM sessions WHERE id = ?1",
            [session_id],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()
        .context("failed to validate request adoption session binding")?;
    match session_binding {
        Some(Some(stored)) if stored == binding_json => Ok(binding_json),
        _ => bail!("request adoption session binding does not match"),
    }
}

#[cfg_attr(not(test), allow(dead_code))]
pub fn insert_launch_adoption(
    conn: &Connection,
    adoption_id: &str,
    session_id: &str,
    request: &crate::request_adoption::RequestAdoption,
    binding: &crate::execution_binding::ExecutionBinding,
    adopted_at: &str,
) -> Result<()> {
    let binding_json = validated_adoption_binding_json(conn, session_id, request, binding)?;
    if request.request_digest != binding.request_digest {
        bail!("launch request adoption digest does not match execution binding");
    }
    conn.execute(
        "INSERT INTO request_adoptions (
            id, adoption_key, contract, operation, request_digest, session_id,
            execution_binding_json, principal_ref, project_digest, graph_id,
            node_id, attempt_id, adopted_at
         ) VALUES (
            ?1, ?2, ?3, 'launch', ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12
         )",
        params![
            adoption_id,
            &request.key,
            &request.contract,
            &request.request_digest,
            session_id,
            binding_json,
            &binding.principal_ref,
            &binding.project_digest,
            &binding.graph_id,
            &binding.node_id,
            &binding.attempt_id,
            adopted_at,
        ],
    )
    .context("failed to insert launch request adoption")?;
    Ok(())
}

#[cfg_attr(not(test), allow(dead_code))]
pub fn insert_input_adoption(
    conn: &Connection,
    adoption_id: &str,
    session_id: &str,
    request: &crate::request_adoption::RequestAdoption,
    binding: &crate::execution_binding::ExecutionBinding,
    adopted_at: &str,
) -> Result<()> {
    let binding_json = validated_adoption_binding_json(conn, session_id, request, binding)?;
    conn.execute(
        "INSERT INTO request_adoptions (
            id, adoption_key, contract, operation, request_digest, session_id,
            execution_binding_json, principal_ref, project_digest, graph_id,
            node_id, attempt_id, adopted_at
         ) VALUES (
            ?1, ?2, ?3, 'input', ?4, ?5, ?6, NULL, NULL, NULL, NULL, NULL, ?7
         )",
        params![
            adoption_id,
            &request.key,
            &request.contract,
            &request.request_digest,
            session_id,
            binding_json,
            adopted_at,
        ],
    )
    .context("failed to insert input request adoption")?;
    Ok(())
}

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
    let external_int: i32 = row.get(13)?;
    let execution_binding_json: Option<String> = row.get(15)?;
    let execution_binding = execution_binding_json
        .as_deref()
        .map(parse_stored_execution_binding)
        .transpose()
        .map_err(|err| {
            rusqlite::Error::FromSqlConversionFailure(15, rusqlite::types::Type::Text, err)
        })?;
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
        execution_binding,
        labels,
        visibility,
        external: external_int != 0,
        transcript_path: row.get(14)?,
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

fn session_has_request_adoption(conn: &Connection, session_id: &str) -> Result<bool> {
    conn.query_row(
        "SELECT EXISTS (
             SELECT 1 FROM request_adoptions WHERE session_id = ?1
         )",
        [session_id],
        |row| row.get(0),
    )
    .context("failed to check session adoption retention")
}

pub fn ensure_session_sacrificable(conn: &Connection, session_id: &str) -> Result<()> {
    let Some(session) = get_session(conn, session_id)? else {
        bail!("session `{session_id}` not found");
    };
    if session.status == crate::RUNNING_SESSION_STATUS {
        bail!("session `{session_id}` is still running; do not sacrifice live work");
    }
    if session_has_request_adoption(conn, session_id)? {
        return Err(AdoptionRetentionError.into());
    }
    Ok(())
}

fn is_foreign_key_constraint(error: &rusqlite::Error) -> bool {
    let rusqlite::Error::SqliteFailure(code, message) = error else {
        return false;
    };
    code.code == ErrorCode::ConstraintViolation
        && matches!(
            code.extended_code,
            rusqlite::ffi::SQLITE_CONSTRAINT_FOREIGNKEY | rusqlite::ffi::SQLITE_CONSTRAINT_TRIGGER
        )
        && message.as_deref() == Some("FOREIGN KEY constraint failed")
}

fn sacrifice_session_with_pre_delete_hook<F>(
    conn: &Connection,
    session_id: &str,
    after_preflight: F,
) -> Result<()>
where
    F: FnOnce() -> Result<()>,
{
    ensure_session_sacrificable(conn, session_id)?;
    after_preflight()?;

    match conn.execute(
        "DELETE FROM sessions
         WHERE id = ?1 AND status != ?2",
        params![session_id, crate::RUNNING_SESSION_STATUS],
    ) {
        Ok(affected) if affected > 0 => Ok(()),
        Ok(_) => match get_session(conn, session_id)? {
            None => Ok(()),
            Some(session) => {
                ensure_session_sacrificable(conn, session_id)?;
                bail!(
                    "session `{session_id}` changed during sacrifice (current status: {}); retry",
                    session.status
                )
            }
        },
        Err(error)
            if is_foreign_key_constraint(&error)
                && session_has_request_adoption(conn, session_id)? =>
        {
            Err(AdoptionRetentionError.into())
        }
        Err(error) => {
            Err(error).with_context(|| format!("failed to sacrifice session {session_id}"))
        }
    }
}

pub fn sacrifice_session(conn: &Connection, session_id: &str) -> Result<()> {
    sacrifice_session_with_pre_delete_hook(conn, session_id, || Ok(()))
}

pub fn latest_active_for_project(
    conn: &Connection,
    project_root: &str,
    harness: &str,
) -> Result<Option<String>> {
    conn.query_row(
        "SELECT id FROM sessions
         WHERE project_root = ?1 AND harness = ?2 AND archived_at IS NULL
         ORDER BY created_at DESC
         LIMIT 1",
        params![project_root, harness],
        |row| row.get::<_, String>(0),
    )
    .optional()
    .context("failed to query latest active session for project and harness")
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

/// Maximum number of JSONL lines to read from an external transcript during
/// ingestion. Lines beyond this cap are silently dropped (the session will
/// still be marked as indexed so the cap applies only once, on first search).
const TRANSCRIPT_INGEST_LINE_LIMIT: usize = 10_000;

/// Maximum total bytes of extracted text to index from a single transcript.
const TRANSCRIPT_INGEST_BYTE_LIMIT: usize = 512 * 1024; // 512 KiB

/// Query for external sessions that have a transcript path but have not yet
/// been indexed into the FTS table. Returns (session_id, transcript_path) pairs.
pub fn list_uningest_external_sessions(conn: &Connection) -> Result<Vec<(String, String)>> {
    let mut stmt = conn
        .prepare(
            "SELECT id, transcript_path
             FROM sessions
             WHERE external = 1
               AND transcript_path IS NOT NULL
               AND transcript_indexed_at IS NULL",
        )
        .context("failed to prepare un-ingested external session query")?;
    let rows = stmt
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .context("failed to query un-ingested external sessions")?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.context("failed to read un-ingested session row")?);
    }
    Ok(out)
}

/// Read an external session's engine JSONL transcript and index its text into
/// the events table (redacted, retention-bounded) so it becomes searchable.
///
/// # Design decisions
///
/// - **Index-once**: once `transcript_indexed_at` is set the function returns 0
///   immediately. Growing transcripts are not re-indexed (a follow-up task).
/// - **Missing file**: if the file does not exist yet (session still running but
///   the transcript hasn't been written) we return 0 and leave
///   `transcript_indexed_at` NULL so the next search will retry. If the file
///   exists but can't be read, we log a warning and return 0 without marking
///   as indexed (same retry-on-next-search semantics). Permanently missing files
///   are handled by a bounded retry: a caller willing to never retry can set
///   `transcript_indexed_at` themselves.
/// - **Bounds**: at most `TRANSCRIPT_INGEST_LINE_LIMIT` lines and
///   `TRANSCRIPT_INGEST_BYTE_LIMIT` bytes of extracted text are indexed. On
///   truncation a log message is emitted.
/// - **Text extraction**: each line is parsed as `serde_json::Value`. Text is
///   extracted using two heuristics (in priority order):
///     1. `message.content` array → each element with `type == "text"` yields
///        its `text` field. This covers the coven-code `TranscriptEntry` shape:
///        `{"type":"user"|"assistant","message":{"content":[{"type":"text","text":"…"}]}}`.
///     2. Top-level `text` string field (fallback for simpler shapes).
///
///   Lines with no extractable text are skipped.
/// - **Privacy**: text is inserted via `insert_event` which runs
///   `redact_payload_json_with_config` with `PrivacyConfig::default()` — same
///   path as normal daemon events. If `coven_home` is provided and has a
///   `privacy.toml`, the caller can instead use `insert_event_with_privacy`
///   directly; we accept the home dir here and dispatch accordingly.
///
/// Returns the number of event rows inserted.
pub fn ingest_external_transcript(
    conn: &Connection,
    session_id: &str,
    coven_home: &Path,
    now: &str,
) -> Result<usize> {
    use std::io::BufRead as _;

    // Load the session; bail if already indexed or not eligible.
    let session = match get_session(conn, session_id)? {
        Some(s) => s,
        None => return Ok(0),
    };
    if !session.external {
        return Ok(0);
    }
    let transcript_path = match &session.transcript_path {
        Some(p) => p.clone(),
        None => return Ok(0),
    };

    // Already indexed — skip.
    let already_indexed: Option<String> = conn
        .query_row(
            "SELECT transcript_indexed_at FROM sessions WHERE id = ?1",
            params![session_id],
            |row| row.get(0),
        )
        .optional()
        .context("failed to query transcript_indexed_at")?
        .flatten();
    if already_indexed.is_some() {
        return Ok(0);
    }

    // Open the transcript file. If it doesn't exist yet, return 0 without
    // marking as indexed so a future search will retry.
    let file = match std::fs::File::open(&transcript_path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // File doesn't exist yet (session may still be starting). Retry later.
            return Ok(0);
        }
        Err(e) => {
            eprintln!(
                "warning: ingest_external_transcript: cannot open transcript \
                 {transcript_path}: {e}; will retry on next search"
            );
            return Ok(0);
        }
    };

    let reader = std::io::BufReader::new(file);
    let mut count = 0usize;
    let mut total_bytes = 0usize;
    let mut truncated = false;

    for (line_idx, line_result) in reader.lines().enumerate() {
        if line_idx >= TRANSCRIPT_INGEST_LINE_LIMIT {
            truncated = true;
            break;
        }
        let line = match line_result {
            Ok(l) => l,
            Err(e) => {
                eprintln!(
                    "warning: ingest_external_transcript: read error at line \
                     {line_idx} in {transcript_path}: {e}; skipping rest"
                );
                break;
            }
        };
        if line.trim().is_empty() {
            continue;
        }
        let value: serde_json::Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue, // non-JSON line (e.g. header), skip
        };

        // Extract human-readable text from the parsed JSON.
        let texts = extract_transcript_texts(&value);
        for text in texts {
            if text.is_empty() {
                continue;
            }
            if total_bytes + text.len() > TRANSCRIPT_INGEST_BYTE_LIMIT {
                truncated = true;
                break;
            }
            total_bytes += text.len();

            let payload = serde_json::json!({ "text": text });
            let record = EventRecord {
                seq: 0,
                id: uuid::Uuid::new_v4().to_string(),
                session_id: session_id.to_string(),
                kind: "transcript_text".to_string(),
                payload_json: payload.to_string(),
                created_at: now.to_string(),
            };
            insert_event_with_privacy(conn, coven_home, &record)?;
            count += 1;
        }
        if truncated {
            break;
        }
    }

    if truncated {
        eprintln!(
            "warning: ingest_external_transcript: session {session_id} transcript truncated at \
             {TRANSCRIPT_INGEST_LINE_LIMIT} lines / {TRANSCRIPT_INGEST_BYTE_LIMIT} bytes \
             ({count} chunks indexed)"
        );
    }

    // Mark as indexed regardless of how many chunks were extracted (including zero,
    // which means the file was present but had no parseable text).
    conn.execute(
        "UPDATE sessions SET transcript_indexed_at = ?2 WHERE id = ?1",
        params![session_id, now],
    )
    .with_context(|| format!("failed to set transcript_indexed_at for session {session_id}"))?;

    Ok(count)
}

/// Extract human-readable text strings from a single transcript JSONL line.
///
/// Targets the coven-code `TranscriptEntry` shape:
/// ```json
/// {"type":"user","message":{"content":[{"type":"text","text":"hello"}]}}
/// ```
/// Also handles a simpler top-level `"text": "..."` field as a fallback.
fn extract_transcript_texts(value: &serde_json::Value) -> Vec<String> {
    let mut out = Vec::new();

    // Heuristic 1: message.content array of {type:"text", text:"..."} blocks.
    if let Some(content) = value
        .get("message")
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_array())
    {
        for block in content {
            if block.get("type").and_then(|t| t.as_str()) == Some("text") {
                if let Some(text) = block.get("text").and_then(|t| t.as_str()) {
                    let trimmed = text.trim().to_string();
                    if !trimmed.is_empty() {
                        out.push(trimmed);
                    }
                }
            }
        }
    }

    // Heuristic 2: top-level "text" string (simpler/older shapes).
    if out.is_empty() {
        if let Some(text) = value.get("text").and_then(|t| t.as_str()) {
            let trimmed = text.trim().to_string();
            if !trimmed.is_empty() {
                out.push(trimmed);
            }
        }
    }

    out
}

pub fn vacuum_store_path(path: &Path) -> Result<StoreVacuumReport> {
    let conn = open_store(path)?;
    let pre_compaction = load_warded_surface_commitments(&conn)?;

    let event_index_rebuilt = sqlite_object_exists(&conn, "table", "events_fts")?;
    if event_index_rebuilt {
        conn.execute("INSERT INTO events_fts(events_fts) VALUES('rebuild')", [])
            .context("failed to rebuild events_fts")?;
    }

    conn.execute_batch("PRAGMA optimize; VACUUM;")
        .context("failed to vacuum Coven store")?;
    let _ = conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);");
    let integrity_check = pragma_integrity_check(&conn)?;
    append_compaction_ledger_events(&conn, &pre_compaction)?;

    Ok(StoreVacuumReport {
        event_index_rebuilt,
        integrity_check,
    })
}

fn load_warded_surface_commitments(conn: &Connection) -> Result<Vec<WardedSurfaceCommitment>> {
    let mut stmt = conn
        .prepare(
            "SELECT familiar_id, surface, entry_hash
             FROM ward_manifest
             ORDER BY familiar_id, surface",
        )
        .context("loading warded surface commitments before compaction")?;
    let rows = stmt.query_map([], |row| {
        Ok(WardedSurfaceCommitment {
            familiar_id: row.get(0)?,
            surface: row.get(1)?,
            entry_hash: row.get(2)?,
        })
    })?;
    rows.collect::<std::result::Result<Vec<_>, _>>()
        .context("reading warded surface commitments before compaction")
}

fn append_compaction_ledger_events(
    conn: &Connection,
    pre_compaction: &[WardedSurfaceCommitment],
) -> Result<()> {
    if pre_compaction.is_empty() {
        return Ok(());
    }

    let format = time::format_description::well_known::Rfc3339;
    let now = time::OffsetDateTime::now_utc().format(&format)?;
    for pre in pre_compaction {
        let post = load_warded_surface_commitment(conn, &pre.familiar_id, &pre.surface)?;
        let decision = match post.as_deref() {
            Some(bytes) if bytes == pre.entry_hash.as_slice() => "compacted:unchanged",
            Some(_) => "compacted:changed",
            None => "compacted:missing_post",
        };
        let files_touched = serde_json::to_string(&[pre.surface.as_str()])?;
        conn.execute(
            "INSERT INTO ward_audit (
                event_type, proposal_id, familiar_id, ward_version, ward_hash,
                tier, decision, approver, diff_hash, files_touched, channel,
                thread_id, submitted_at, decided_at
            ) VALUES (?1, NULL, ?2, NULL, ?3, NULL, ?4, NULL, ?5, ?6, ?7, NULL, ?8, ?8)",
            params![
                coven_threads_core::AuditEventType::CompactionLedger.tag(),
                pre.familiar_id,
                pre.entry_hash,
                decision,
                post,
                files_touched,
                format!("{:?}", coven_threads_core::Channel::Forced).to_lowercase(),
                now,
            ],
        )
        .with_context(|| {
            format!(
                "appending WARD-C6 compaction ledger for {}:{}",
                pre.familiar_id, pre.surface
            )
        })?;
    }
    Ok(())
}

fn load_warded_surface_commitment(
    conn: &Connection,
    familiar_id: &str,
    surface: &str,
) -> Result<Option<Vec<u8>>> {
    conn.query_row(
        "SELECT entry_hash FROM ward_manifest WHERE familiar_id = ?1 AND surface = ?2",
        params![familiar_id, surface],
        |row| row.get(0),
    )
    .optional()
    .context("loading warded surface commitment after compaction")
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
    insert_event_raw(
        conn,
        record,
        &redacted_payload,
        redaction_status,
        sensitive,
        None,
    )
}

pub fn insert_event_with_privacy(
    conn: &Connection,
    coven_home: &Path,
    record: &EventRecord,
) -> Result<()> {
    insert_event_with_privacy_and_adoption(conn, coven_home, record, None)
}

pub fn insert_event_with_privacy_and_adoption(
    conn: &Connection,
    coven_home: &Path,
    record: &EventRecord,
    request_adoption_id: Option<&str>,
) -> Result<()> {
    let config = privacy::load_config(coven_home).unwrap_or_default();
    let redacted_payload = privacy::redact_payload_json_with_config(&record.payload_json, &config);
    let sensitive = redacted_payload != record.payload_json;
    let mut redaction_status = if sensitive { "redacted" } else { "clean" };
    insert_event_raw(
        conn,
        record,
        &redacted_payload,
        redaction_status,
        sensitive,
        request_adoption_id,
    )?;

    if config.persist_raw_artifacts && sensitive {
        let artifact_result = retention_expires_at(
            &record.created_at,
            config.raw_artifact_retention_days.max(1),
        )
        .and_then(|expires_at| {
            SensitiveArtifactStore::load(coven_home)
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
                            expires_at,
                        },
                    )
                })
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
    request_adoption_id: Option<&str>,
) -> Result<()> {
    conn.execute(
        "INSERT INTO events (
            id,
            session_id,
            kind,
            payload_json,
            created_at,
            redaction_status,
            sensitive,
            request_adoption_id
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            &record.id,
            &record.session_id,
            &record.kind,
            payload_json,
            &record.created_at,
            redaction_status,
            if sensitive { 1 } else { 0 },
            request_adoption_id,
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

fn handoff_record_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<HandoffRecord> {
    Ok(HandoffRecord {
        id: row.get(0)?,
        session_id: row.get(1)?,
        generation: row.get(2)?,
        packet_json: row.get(3)?,
        event_cursor: row.get(4)?,
        workspace_json: row.get(5)?,
        state: row.get(6)?,
        claimant: row.get(7)?,
        idempotency_key: row.get(8)?,
        created_at: row.get(9)?,
        updated_at: row.get(10)?,
    })
}

const HANDOFF_COLUMNS: &str = "id, session_id, generation, packet_json, event_cursor, workspace_json, state, claimant, idempotency_key, created_at, updated_at";

pub fn create_handoff(
    conn: &mut Connection,
    id: &str,
    session_id: &str,
    packet_json: &str,
    workspace_json: &str,
    now: &str,
) -> Result<HandoffRecord> {
    let transaction = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let generation: i64 = transaction.query_row(
        "SELECT COALESCE(MAX(generation), 0) + 1 FROM session_handoffs WHERE session_id = ?1",
        [session_id],
        |row| row.get(0),
    )?;
    let event_cursor: i64 = transaction.query_row(
        "SELECT COALESCE(MAX(rowid), 0) FROM events WHERE session_id = ?1",
        [session_id],
        |row| row.get(0),
    )?;
    transaction.execute(
        "INSERT INTO session_handoffs (
            id, session_id, generation, packet_json, event_cursor, workspace_json,
            state, claimant, idempotency_key, created_at, updated_at
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'offered', NULL, NULL, ?7, ?7)",
        params![
            id,
            session_id,
            generation,
            packet_json,
            event_cursor,
            workspace_json,
            now
        ],
    )?;
    let record = transaction.query_row(
        &format!("SELECT {HANDOFF_COLUMNS} FROM session_handoffs WHERE id = ?1"),
        [id],
        handoff_record_from_row,
    )?;
    transaction.commit()?;
    Ok(record)
}

pub fn get_handoff(conn: &Connection, handoff_id: &str) -> Result<Option<HandoffRecord>> {
    conn.query_row(
        &format!("SELECT {HANDOFF_COLUMNS} FROM session_handoffs WHERE id = ?1"),
        [handoff_id],
        handoff_record_from_row,
    )
    .optional()
    .context("failed to read session handoff")
}

pub fn list_handoffs(conn: &Connection, session_id: &str) -> Result<Vec<HandoffRecord>> {
    let mut statement = conn.prepare(&format!(
        "SELECT {HANDOFF_COLUMNS} FROM session_handoffs WHERE session_id = ?1 ORDER BY generation ASC"
    ))?;
    let records = statement
        .query_map([session_id], handoff_record_from_row)?
        .collect::<std::result::Result<Vec<_>, _>>()
        .context("failed to list session handoffs")?;
    Ok(records)
}

/// Atomically claims an offered handoff. A live source input lease makes the
/// claim fail rather than allowing a last-writer-wins handoff race.
pub fn claim_handoff(
    conn: &mut Connection,
    handoff_id: &str,
    expected_generation: i64,
    claimant: &str,
    idempotency_key: &str,
    now: &str,
) -> Result<HandoffRecord> {
    let transaction = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let current = transaction.query_row(
        &format!("SELECT {HANDOFF_COLUMNS} FROM session_handoffs WHERE id = ?1"),
        [handoff_id],
        handoff_record_from_row,
    )?;
    if current.generation != expected_generation {
        bail!("stale_generation");
    }
    if current.state != "offered" {
        if current.claimant.as_deref() == Some(claimant)
            && current.idempotency_key.as_deref() == Some(idempotency_key)
        {
            transaction.commit()?;
            return Ok(current);
        }
        bail!("handoff_already_claimed");
    }
    let actual_cursor: i64 = transaction
        .query_row(
            "SELECT COALESCE(MAX(rowid), 0) FROM events WHERE session_id = ?1",
            [&current.session_id],
            |row| row.get(0),
        )
        .context("failed to read latest event sequence")?;
    if actual_cursor < current.event_cursor {
        bail!("transcript_diverged");
    }
    let input_in_flight: bool = transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM session_input_leases WHERE session_id = ?1)",
        [&current.session_id],
        |row| row.get(0),
    )?;
    if input_in_flight {
        bail!("source_input_in_flight");
    }
    transaction.execute(
        "UPDATE session_handoffs
         SET state = 'claimed', claimant = ?2, idempotency_key = ?3, updated_at = ?4
         WHERE id = ?1 AND state = 'offered'",
        params![handoff_id, claimant, idempotency_key, now],
    )?;
    let claimed = transaction.query_row(
        &format!("SELECT {HANDOFF_COLUMNS} FROM session_handoffs WHERE id = ?1"),
        [handoff_id],
        handoff_record_from_row,
    )?;
    transaction.commit()?;
    Ok(claimed)
}

pub fn acknowledge_handoff(
    conn: &mut Connection,
    handoff_id: &str,
    claimant: &str,
    now: &str,
) -> Result<HandoffRecord> {
    let transaction = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let current = transaction.query_row(
        &format!("SELECT {HANDOFF_COLUMNS} FROM session_handoffs WHERE id = ?1"),
        [handoff_id],
        handoff_record_from_row,
    )?;
    if current.claimant.as_deref() != Some(claimant) {
        bail!("claimant_mismatch");
    }
    match current.state.as_str() {
        "acknowledged" => {
            transaction.commit()?;
            return Ok(current);
        }
        "claimed" => {}
        _ => bail!("handoff_not_claimed"),
    }
    let actual_cursor: i64 = transaction
        .query_row(
            "SELECT COALESCE(MAX(rowid), 0) FROM events WHERE session_id = ?1",
            [&current.session_id],
            |row| row.get(0),
        )
        .context("failed to read latest event sequence")?;
    if actual_cursor < current.event_cursor {
        bail!("transcript_diverged");
    }
    transaction.execute(
        "UPDATE session_handoffs SET state = 'acknowledged', updated_at = ?2 WHERE id = ?1",
        params![handoff_id, now],
    )?;
    let acknowledged = transaction.query_row(
        &format!("SELECT {HANDOFF_COLUMNS} FROM session_handoffs WHERE id = ?1"),
        [handoff_id],
        handoff_record_from_row,
    )?;
    transaction.commit()?;
    Ok(acknowledged)
}

pub fn create_handoff_continuation(
    conn: &mut Connection,
    id: &str,
    handoff_id: &str,
    destination: &str,
    now: &str,
) -> Result<HandoffContinuationRecord> {
    let transaction = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let handoff = transaction.query_row(
        &format!("SELECT {HANDOFF_COLUMNS} FROM session_handoffs WHERE id = ?1"),
        [handoff_id],
        handoff_record_from_row,
    )?;
    if handoff.state != "acknowledged" && handoff.state != "continued" {
        bail!("source_acknowledgement_required");
    }
    let existing = transaction
        .query_row(
            "SELECT id, handoff_id, source_session_id, generation, destination, created_at
             FROM handoff_continuations WHERE handoff_id = ?1 AND destination = ?2",
            params![handoff_id, destination],
            |row| {
                Ok(HandoffContinuationRecord {
                    id: row.get(0)?,
                    handoff_id: row.get(1)?,
                    source_session_id: row.get(2)?,
                    generation: row.get(3)?,
                    destination: row.get(4)?,
                    created_at: row.get(5)?,
                })
            },
        )
        .optional()?;
    if let Some(existing) = existing {
        transaction.commit()?;
        return Ok(existing);
    }
    transaction.execute(
        "INSERT INTO handoff_continuations (id, handoff_id, source_session_id, generation, destination, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![id, handoff_id, handoff.session_id, handoff.generation, destination, now],
    )?;
    transaction.execute(
        "UPDATE session_handoffs SET state = 'continued', updated_at = ?2 WHERE id = ?1",
        params![handoff_id, now],
    )?;
    transaction.commit()?;
    Ok(HandoffContinuationRecord {
        id: id.to_string(),
        handoff_id: handoff_id.to_string(),
        source_session_id: handoff.session_id,
        generation: handoff.generation,
        destination: destination.to_string(),
        created_at: now.to_string(),
    })
}

/// Acquire a short source-input lease. A concurrent claim either sees this
/// lease and rejects safely, or commits first and prevents the lease entirely.
pub fn acquire_session_input_lease(
    conn: &mut Connection,
    lease_id: &str,
    session_id: &str,
    now: &str,
) -> Result<bool> {
    let transaction = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let fenced: bool = transaction.query_row(
        "SELECT EXISTS(
             SELECT 1 FROM session_handoffs
             WHERE session_id = ?1 AND state IN ('claimed', 'acknowledged', 'continued')
         )",
        [session_id],
        |row| row.get(0),
    )?;
    if fenced {
        transaction.commit()?;
        return Ok(false);
    }
    transaction.execute(
        "INSERT INTO session_input_leases (id, session_id, created_at) VALUES (?1, ?2, ?3)",
        params![lease_id, session_id, now],
    )?;
    transaction.commit()?;
    Ok(true)
}

pub fn session_input_handoff_fenced(conn: &Connection, session_id: &str) -> Result<bool> {
    conn.query_row(
        "SELECT EXISTS(
             SELECT 1 FROM session_handoffs
             WHERE session_id = ?1 AND state IN ('claimed', 'acknowledged', 'continued')
         )",
        [session_id],
        |row| row.get(0),
    )
    .context("failed to check session input handoff fence")
}

pub fn acquire_session_input_lease_and_adopt(
    conn: &mut Connection,
    session_id: &str,
    request: &crate::request_adoption::RequestAdoption,
    binding: &crate::execution_binding::ExecutionBinding,
    now: &str,
) -> Result<InputAdoptionResult> {
    let transaction = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    match resolve_input_adoption(&transaction, session_id, request, binding)? {
        AdoptionResolution::Replay { .. } => {
            transaction.commit()?;
            return Ok(InputAdoptionResult::Replay);
        }
        AdoptionResolution::Conflict { .. } => {
            transaction.commit()?;
            return Ok(InputAdoptionResult::Conflict);
        }
        AdoptionResolution::Absent => {}
    }
    let status = transaction
        .query_row(
            "SELECT status FROM sessions WHERE id = ?1",
            [session_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .context("failed to check adopted input session liveness")?;
    if status.as_deref() != Some(crate::RUNNING_SESSION_STATUS) {
        transaction.rollback()?;
        return Ok(InputAdoptionResult::NotLive);
    }
    if session_input_handoff_fenced(&transaction, session_id)? {
        transaction.commit()?;
        return Ok(InputAdoptionResult::HandoffFenced);
    }

    let adoption_id = uuid::Uuid::new_v4().to_string();
    let lease_id = uuid::Uuid::new_v4().to_string();
    transaction.execute(
        "INSERT INTO session_input_leases (id, session_id, created_at) VALUES (?1, ?2, ?3)",
        params![&lease_id, session_id, now],
    )?;
    insert_input_adoption(
        &transaction,
        &adoption_id,
        session_id,
        request,
        binding,
        now,
    )?;
    transaction.commit()?;
    Ok(InputAdoptionResult::Adopted {
        adoption_id,
        lease_id,
    })
}

pub fn release_session_input_lease(conn: &Connection, lease_id: &str) -> Result<()> {
    conn.execute("DELETE FROM session_input_leases WHERE id = ?1", [lease_id])?;
    Ok(())
}

pub fn list_events(conn: &Connection, session_id: &str) -> Result<Vec<EventRecord>> {
    list_events_with_options(conn, session_id, &EventsQueryOptions::default())
}

#[cfg(test)]
pub fn latest_event_seq(conn: &Connection, session_id: &str) -> Result<i64> {
    conn.query_row(
        "SELECT COALESCE(MAX(rowid), 0) FROM events WHERE session_id = ?1",
        [session_id],
        |row| row.get(0),
    )
    .context("failed to read latest event sequence")
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

pub(crate) fn resolve_event_after_rowid(
    conn: &Connection,
    session_id: &str,
    opts: &EventsQueryOptions,
) -> Result<Option<i64>> {
    if let Some(seq) = opts.after_seq {
        return Ok(Some(seq));
    }
    let Some(event_id) = opts.after_event_id.as_ref() else {
        return Ok(None);
    };
    conn.query_row(
        "SELECT rowid FROM events WHERE id = ?1 AND session_id = ?2 LIMIT 1",
        params![event_id, session_id],
        |row| row.get::<_, i64>(0),
    )
    .optional()
    .context("failed to resolve event cursor by event id")
}

pub(crate) fn list_event_candidates(
    conn: &Connection,
    session_id: &str,
    after_rowid: Option<i64>,
    limit: usize,
) -> Result<EventCandidatePage> {
    const ERROR_EVENT_ID_MAX_BYTES: i64 = 512;

    if limit == 0 {
        return Ok(EventCandidatePage {
            candidates: Vec::new(),
        });
    }
    let limit = i64::try_from(limit).context("event candidate limit exceeded SQLite range")?;
    let (sql, params): (&str, Vec<rusqlite::types::Value>) = if let Some(after_rowid) = after_rowid
    {
        (
            "SELECT
                rowid,
                CASE WHEN octet_length(id) <= ?3 THEN id ELSE NULL END,
                octet_length(id),
                octet_length(session_id),
                octet_length(kind),
                octet_length(payload_json),
                octet_length(created_at),
                redaction_status = 'legacy'
             FROM events
             WHERE session_id = ?1 AND rowid > ?2
             ORDER BY rowid ASC
             LIMIT ?4",
            vec![
                session_id.to_owned().into(),
                after_rowid.into(),
                ERROR_EVENT_ID_MAX_BYTES.into(),
                limit.into(),
            ],
        )
    } else {
        (
            "SELECT
                rowid,
                CASE WHEN octet_length(id) <= ?2 THEN id ELSE NULL END,
                octet_length(id),
                octet_length(session_id),
                octet_length(kind),
                octet_length(payload_json),
                octet_length(created_at),
                redaction_status = 'legacy'
             FROM events
             WHERE session_id = ?1
             ORDER BY rowid ASC
             LIMIT ?3",
            vec![
                session_id.to_owned().into(),
                ERROR_EVENT_ID_MAX_BYTES.into(),
                limit.into(),
            ],
        )
    };
    let mut statement = conn
        .prepare(sql)
        .context("failed to prepare bounded event candidate query")?;
    let mut rows = statement
        .query(rusqlite::params_from_iter(params))
        .context("failed to query bounded event candidates")?;
    let mut candidates = Vec::with_capacity(usize::try_from(limit).unwrap_or_default());
    while let Some(row) = rows
        .next()
        .context("failed to read bounded event candidate")?
    {
        let mut allocation_bytes = 0_usize;
        for column in 2..=6 {
            let field_bytes: i64 = row
                .get(column)
                .context("event candidate contained an invalid field length")?;
            let field_bytes = usize::try_from(field_bytes)
                .context("event candidate contained a negative or oversized field length")?;
            allocation_bytes = allocation_bytes
                .checked_add(field_bytes)
                .context("event candidate allocation size overflowed")?;
        }
        let is_legacy_redaction: bool = row
            .get(7)
            .context("event candidate contained an invalid redaction status")?;
        candidates.push(EventCandidate {
            seq: row.get(0).context("event candidate had no sequence")?,
            event_id: row
                .get(1)
                .context("event candidate had an invalid bounded id")?,
            allocation_bytes,
            encoded_lower_bound_bytes: (!is_legacy_redaction).then_some(allocation_bytes),
        });
    }
    Ok(EventCandidatePage { candidates })
}

pub(crate) fn get_event_by_seq(
    conn: &Connection,
    session_id: &str,
    seq: i64,
) -> Result<Option<EventRecord>> {
    conn.query_row(
        "SELECT rowid AS seq, id, session_id, kind, payload_json, created_at, redaction_status
         FROM events
         WHERE session_id = ?1 AND rowid = ?2",
        params![session_id, seq],
        event_record_from_row,
    )
    .optional()
    .context("failed to load bounded event candidate")
}

fn event_record_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<EventRecord> {
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
}

pub fn list_events_with_options(
    conn: &Connection,
    session_id: &str,
    opts: &EventsQueryOptions,
) -> Result<Vec<EventRecord>> {
    let after_rowid = resolve_event_after_rowid(conn, session_id, opts)?;

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

    let events = match (after_rowid, opts.limit) {
        (Some(after), Some(limit)) => statement
            .query_map(params![session_id, after, limit], event_record_from_row)
            .context("failed to query events")?,
        (Some(after), None) => statement
            .query_map(params![session_id, after], event_record_from_row)
            .context("failed to query events")?,
        (None, Some(limit)) => statement
            .query_map(params![session_id, limit], event_record_from_row)
            .context("failed to query events")?,
        (None, None) => statement
            .query_map(params![session_id], event_record_from_row)
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
        &format!(
            "SELECT COUNT(*) FROM events AS event
             WHERE event.created_at < ?1
               AND {EVENT_NOT_PINNED_BY_UNRESOLVED_HANDOFF_SQL}"
        ),
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
    conn.execute(
        &format!(
            "DELETE FROM events AS event
             WHERE event.created_at < ?1
               AND {EVENT_NOT_PINNED_BY_UNRESOLVED_HANDOFF_SQL}"
        ),
        params![cutoff],
    )
    .context("failed to prune events")
}

/// Delete at most one maintenance batch of expired events in a single
/// transaction. The FTS external-content delete trigger runs in that same
/// transaction, so a crash or interrupted process leaves both tables at the
/// previous committed state instead of exposing a half-pruned index.
pub fn prune_events_older_than_bounded(
    conn: &Connection,
    cutoff: &str,
    limit: i64,
) -> Result<usize> {
    let tx = conn
        .unchecked_transaction()
        .context("failed to start bounded event-prune transaction")?;
    let pruned = tx
        .execute(
            &format!(
                "DELETE FROM events AS event
                 WHERE event.rowid IN (
                    SELECT event.rowid FROM events AS event
                    WHERE event.created_at < ?1
                      AND {EVENT_NOT_PINNED_BY_UNRESOLVED_HANDOFF_SQL}
                    ORDER BY event.created_at, event.rowid
                    LIMIT ?2
                 )"
            ),
            params![cutoff, limit.max(1)],
        )
        .context("failed to prune bounded event batch")?;
    tx.commit()
        .context("failed to commit bounded event-prune transaction")?;
    Ok(pruned)
}

fn prune_sensitive_artifacts_bounded(
    conn: &Connection,
    now: &str,
    retention_cutoff: &str,
    limit: i64,
) -> Result<usize> {
    let limit = limit.max(1);
    let tx = conn
        .unchecked_transaction()
        .context("failed to start bounded artifact-prune transaction")?;
    let expired_pruned = tx
        .execute(
            BOUNDED_PRUNE_SENSITIVE_ARTIFACTS_BY_EXPIRY_SQL,
            params![now, limit],
        )
        .context("failed to prune bounded expired sensitive-artifact batch")?;
    let remaining_capacity = limit.saturating_sub(
        i64::try_from(expired_pruned).context("bounded artifact prune deleted too many rows")?,
    );
    let aged_pruned = if remaining_capacity > 0 {
        tx.execute(
            BOUNDED_PRUNE_SENSITIVE_ARTIFACTS_BY_CREATED_AT_SQL,
            params![retention_cutoff, remaining_capacity],
        )
        .context("failed to prune bounded aged sensitive-artifact batch")?
    } else {
        0
    };
    tx.commit()
        .context("failed to commit bounded artifact-prune transaction")?;
    expired_pruned
        .checked_add(aged_pruned)
        .context("bounded sensitive-artifact prune overflowed")
}

/// Run the daemon's bounded retention tick. It deliberately never invokes
/// `VACUUM`: compaction remains an explicit operator action because it can
/// monopolize the database and violate session-launch latency expectations.
pub fn run_scheduled_maintenance(
    coven_home: &Path,
    now: &str,
) -> Result<ScheduledMaintenanceReport> {
    let free_disk_bytes = fs2::available_space(coven_home)
        .with_context(|| format!("failed to inspect free disk for {}", coven_home.display()))?;
    run_scheduled_maintenance_with_free_disk(coven_home, now, free_disk_bytes)
}

fn run_scheduled_maintenance_with_free_disk(
    coven_home: &Path,
    now: &str,
    free_disk_bytes: u64,
) -> Result<ScheduledMaintenanceReport> {
    if free_disk_bytes < MAINTENANCE_MIN_FREE_DISK_BYTES {
        return Ok(ScheduledMaintenanceReport {
            raw_artifacts_pruned: 0,
            events_pruned: 0,
            checkpoint_ran: false,
            blocked_by_free_disk: true,
        });
    }

    let config = privacy::load_with_settings(coven_home, crate::settings::cached())
        .context("failed to load privacy settings for scheduled maintenance")?;
    run_scheduled_maintenance_with_config_and_free_disk(coven_home, now, free_disk_bytes, &config)
}

fn run_scheduled_maintenance_with_config_and_free_disk(
    coven_home: &Path,
    now: &str,
    free_disk_bytes: u64,
    config: &PrivacyConfig,
) -> Result<ScheduledMaintenanceReport> {
    if free_disk_bytes < MAINTENANCE_MIN_FREE_DISK_BYTES {
        // Do not record this in SQLite: opening a write transaction here would
        // add WAL pressure precisely when the watermark is meant to prevent it.
        // `storage_health` derives the blocked state from free disk directly.
        return Ok(ScheduledMaintenanceReport {
            raw_artifacts_pruned: 0,
            events_pruned: 0,
            checkpoint_ran: false,
            blocked_by_free_disk: true,
        });
    }

    let raw_cutoff = retention_cutoff(now, config.raw_artifact_retention_days.max(1))?;
    let event_cutoff = retention_cutoff(now, config.log_retention_days.max(1))?;
    let store_path = coven_home.join("coven.sqlite3");
    let conn = open_store(&store_path)?;
    let mut raw_artifacts_pruned = 0usize;
    let mut events_pruned = 0usize;

    for _ in 0..MAINTENANCE_MAX_BATCHES_PER_TICK {
        let raw_batch = prune_sensitive_artifacts_bounded(
            &conn,
            now,
            &raw_cutoff,
            MAINTENANCE_ARTIFACT_BATCH_SIZE,
        )?;
        let event_batch =
            prune_events_older_than_bounded(&conn, &event_cutoff, MAINTENANCE_EVENT_BATCH_SIZE)?;

        raw_artifacts_pruned += raw_batch;
        events_pruned += event_batch;

        // Convergence loop keeps startup backlog from stalling one minute behind
        // while still enforcing a predictable per-tick upper bound.
        // The prune helpers report rows deleted as `usize`; the batch bounds are
        // `i64` because they are bound directly to SQL `LIMIT`. Compare in `i64`
        // rather than widening the constants, so the value handed to SQLite and
        // the value compared here stay the same type.
        if (raw_batch as i64) < MAINTENANCE_ARTIFACT_BATCH_SIZE
            && (event_batch as i64) < MAINTENANCE_EVENT_BATCH_SIZE
        {
            break;
        }
    }

    // Record a successful pass even when nothing was expired. Operators need
    // the age of the maintenance loop itself, not merely the most recent row
    // deletion, to tell a healthy empty ledger from a stalled scheduler.
    set_store_meta(&conn, MAINTENANCE_LAST_PRUNE_KEY, now)?;

    let wal_bytes = file_size(&wal_path(&store_path));
    let checkpoint_ran = if wal_bytes >= MAINTENANCE_CHECKPOINT_WAL_BYTES {
        // PASSIVE does as much as it can without waiting on a reader or writer.
        // A blocked checkpoint is still harmless; the WAL size remains visible
        // through `StorageHealth` for an operator to investigate.
        let _ = conn.query_row("PRAGMA wal_checkpoint(PASSIVE)", [], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })?;
        set_store_meta(&conn, MAINTENANCE_LAST_CHECKPOINT_KEY, now)?;
        true
    } else {
        false
    };
    set_store_meta(&conn, MAINTENANCE_LAST_ERROR_KEY, "")?;

    Ok(ScheduledMaintenanceReport {
        raw_artifacts_pruned,
        events_pruned,
        checkpoint_ran,
        blocked_by_free_disk: false,
    })
}

/// Gather storage pressure without mutating the database. Health callers use
/// this after daemon startup has initialized the schema.
#[cfg(test)]
pub fn storage_health(
    coven_home: &Path,
    event_writer: Option<&crate::event_writer::EventWriterHealth>,
) -> Result<StorageHealth> {
    let free_disk_bytes = fs2::available_space(coven_home)
        .with_context(|| format!("failed to inspect free disk for {}", coven_home.display()))?;
    Ok(storage_health_with_free_disk_or_unavailable(
        coven_home,
        free_disk_bytes,
        event_writer,
    ))
}

pub fn cached_storage_health(
    coven_home: &Path,
    event_writer: Option<&crate::event_writer::EventWriterHealth>,
) -> Result<StorageHealth> {
    let free_disk_bytes = fs2::available_space(coven_home)
        .with_context(|| format!("failed to inspect free disk for {}", coven_home.display()))?;
    cached_storage_health_with_free_disk(coven_home, free_disk_bytes, event_writer)
}

pub(crate) fn cached_storage_health_with_free_disk(
    coven_home: &Path,
    free_disk_bytes: u64,
    event_writer: Option<&crate::event_writer::EventWriterHealth>,
) -> Result<StorageHealth> {
    let snapshot = storage_health_snapshots()
        .read()
        .map_err(|_| anyhow::anyhow!("storage health snapshot lock poisoned"))?
        .get(coven_home)
        .cloned()
        .context("storage health snapshot is unavailable")?;
    let mut health = snapshot.health;
    let store_path = coven_home.join("coven.sqlite3");
    health.database_bytes = file_size(&store_path);
    health.wal_bytes = file_size(&wal_path(&store_path));
    health.free_disk_bytes = free_disk_bytes;
    health.maintenance_blocked = health.free_disk_bytes < MAINTENANCE_MIN_FREE_DISK_BYTES;
    if event_writer.is_some() {
        (health.writer_backlog_events, health.writer_backlog_bytes) = writer_backlog(event_writer);
    }
    health.prune_age_seconds = maintenance_age_seconds(health.last_prune_at.as_deref());
    health.checkpoint_age_seconds = maintenance_age_seconds(health.last_checkpoint_at.as_deref());
    // Live free-disk state outranks a stale degraded snapshot: a maintenance
    // failure recorded earlier must not mask the store being unwritable now.
    // `last_maintenance_error` is carried over from the snapshot either way, so
    // the degraded reason survives the promotion to critical.
    if health.maintenance_blocked {
        health.status = "critical".to_string();
    } else if snapshot.degraded {
        health.status = "degraded".to_string();
    } else if snapshot.retention_lagging
        || health.free_disk_bytes < MAINTENANCE_WARN_FREE_DISK_BYTES
        || health.wal_bytes >= MAINTENANCE_CHECKPOINT_WAL_BYTES
        || health.last_maintenance_error.is_some()
    {
        health.status = "warning".to_string();
    } else {
        health.status = "ok".to_string();
    }
    Ok(health)
}

pub fn refresh_storage_health_snapshot_from_connection(
    coven_home: &Path,
    conn: &Connection,
    event_writer: Option<&crate::event_writer::EventWriterHealth>,
) -> Result<()> {
    let free_disk_bytes = fs2::available_space(coven_home)
        .with_context(|| format!("failed to inspect free disk for {}", coven_home.display()))?;
    refresh_storage_health_snapshot_from_connection_with_free_disk(
        coven_home,
        conn,
        free_disk_bytes,
        event_writer,
    )
}

pub(crate) fn refresh_storage_health_snapshot_from_connection_with_free_disk(
    coven_home: &Path,
    conn: &Connection,
    free_disk_bytes: u64,
    event_writer: Option<&crate::event_writer::EventWriterHealth>,
) -> Result<()> {
    let previous = if free_disk_bytes < MAINTENANCE_MIN_FREE_DISK_BYTES {
        storage_health_snapshots()
            .read()
            .map_err(|_| anyhow::anyhow!("storage health snapshot lock poisoned"))?
            .get(coven_home)
            .cloned()
    } else {
        None
    };
    let mut snapshot =
        storage_health_snapshot_from_connection(coven_home, conn, free_disk_bytes, event_writer)?;
    if let Some(previous) = previous {
        let database_bytes = snapshot.health.database_bytes;
        let wal_bytes = snapshot.health.wal_bytes;
        snapshot = previous;
        snapshot.health.status = "critical".to_string();
        snapshot.health.database_bytes = database_bytes;
        snapshot.health.wal_bytes = wal_bytes;
        snapshot.health.free_disk_bytes = free_disk_bytes;
        snapshot.health.maintenance_blocked = true;
        if event_writer.is_some() {
            (
                snapshot.health.writer_backlog_events,
                snapshot.health.writer_backlog_bytes,
            ) = writer_backlog(event_writer);
        }
    }
    storage_health_snapshots()
        .write()
        .map_err(|_| anyhow::anyhow!("storage health snapshot lock poisoned"))?
        .insert(coven_home.to_path_buf(), snapshot);
    Ok(())
}

pub fn cache_unavailable_storage_health(
    coven_home: &Path,
    event_writer: Option<&crate::event_writer::EventWriterHealth>,
) -> Result<()> {
    let known_free_disk_bytes = fs2::available_space(coven_home).ok();
    mark_storage_health_snapshot_degraded(
        coven_home,
        known_free_disk_bytes,
        event_writer,
        "storage health unavailable",
    )
}

pub(crate) fn mark_storage_health_snapshot_maintenance_failure(
    coven_home: &Path,
    known_free_disk_bytes: Option<u64>,
    event_writer: Option<&crate::event_writer::EventWriterHealth>,
) -> Result<()> {
    mark_storage_health_snapshot_degraded(
        coven_home,
        known_free_disk_bytes,
        event_writer,
        MAINTENANCE_LAST_ERROR_PUBLIC_MESSAGE,
    )
}

fn mark_storage_health_snapshot_degraded(
    coven_home: &Path,
    known_free_disk_bytes: Option<u64>,
    event_writer: Option<&crate::event_writer::EventWriterHealth>,
    prior_snapshot_error: &str,
) -> Result<()> {
    let mut snapshots = storage_health_snapshots()
        .write()
        .map_err(|_| anyhow::anyhow!("storage health snapshot lock poisoned"))?;
    if let Some(snapshot) = snapshots.get_mut(coven_home) {
        snapshot.degraded = true;
        snapshot.health.status = "degraded".to_string();
        snapshot.health.last_maintenance_error = Some(prior_snapshot_error.to_string());
        if let Some(free_disk_bytes) = known_free_disk_bytes {
            snapshot.health.free_disk_bytes = free_disk_bytes;
            snapshot.health.maintenance_blocked = free_disk_bytes < MAINTENANCE_MIN_FREE_DISK_BYTES;
        }
        if event_writer.is_some() {
            (
                snapshot.health.writer_backlog_events,
                snapshot.health.writer_backlog_bytes,
            ) = writer_backlog(event_writer);
        }
        return Ok(());
    }

    let health = unavailable_storage_health(
        coven_home,
        "storage health snapshot refresh failed",
        known_free_disk_bytes,
        event_writer,
    );
    snapshots.insert(
        coven_home.to_path_buf(),
        StorageHealthSnapshot {
            health,
            retention_lagging: false,
            degraded: true,
        },
    );
    Ok(())
}

#[cfg(test)]
fn storage_health_with_free_disk(
    coven_home: &Path,
    free_disk_bytes: u64,
    event_writer: Option<&crate::event_writer::EventWriterHealth>,
) -> Result<StorageHealth> {
    if free_disk_bytes < MAINTENANCE_MIN_FREE_DISK_BYTES {
        let store_path = coven_home.join("coven.sqlite3");
        let (writer_backlog_events, writer_backlog_bytes) = writer_backlog(event_writer);
        return Ok(StorageHealth {
            status: "critical".to_string(),
            database_bytes: file_size(&store_path),
            wal_bytes: file_size(&wal_path(&store_path)),
            oldest_retained_event_at: None,
            last_prune_at: None,
            prune_age_seconds: None,
            last_checkpoint_at: None,
            checkpoint_age_seconds: None,
            writer_backlog_events,
            writer_backlog_bytes,
            free_disk_bytes,
            maintenance_blocked: true,
            last_maintenance_error: None,
        });
    }
    let config = privacy::load_with_settings(coven_home, crate::settings::cached())
        .context("failed to load privacy settings for storage health")?;
    let retention_cutoff = retention_cutoff(
        &Utc::now().to_rfc3339_opts(SecondsFormat::Nanos, true),
        config.log_retention_days.max(1),
    )?;
    parse_rfc3339_utc(&retention_cutoff).context("failed to parse calculated retention cutoff")?;
    let store_path = coven_home.join("coven.sqlite3");
    let conn = open_existing_store_read_only(&store_path)?
        .ok_or_else(|| anyhow::anyhow!("Coven store is unavailable"))?;
    Ok(
        storage_health_snapshot_from_connection(coven_home, &conn, free_disk_bytes, event_writer)?
            .health,
    )
}

fn storage_health_snapshot_from_connection(
    coven_home: &Path,
    conn: &Connection,
    free_disk_bytes: u64,
    event_writer: Option<&crate::event_writer::EventWriterHealth>,
) -> Result<StorageHealthSnapshot> {
    let store_path = coven_home.join("coven.sqlite3");
    let database_bytes = file_size(&store_path);
    let wal_bytes = file_size(&wal_path(&store_path));
    let maintenance_blocked = free_disk_bytes < MAINTENANCE_MIN_FREE_DISK_BYTES;
    let (writer_backlog_events, writer_backlog_bytes) = writer_backlog(event_writer);
    if maintenance_blocked {
        return Ok(StorageHealthSnapshot {
            health: StorageHealth {
                status: "critical".to_string(),
                database_bytes,
                wal_bytes,
                oldest_retained_event_at: None,
                last_prune_at: None,
                prune_age_seconds: None,
                last_checkpoint_at: None,
                checkpoint_age_seconds: None,
                writer_backlog_events,
                writer_backlog_bytes,
                free_disk_bytes,
                maintenance_blocked: true,
                last_maintenance_error: None,
            },
            retention_lagging: false,
            degraded: false,
        });
    }

    let config = privacy::load_with_settings(coven_home, crate::settings::cached())
        .context("failed to load privacy settings for storage health")?;
    let retention_cutoff = retention_cutoff(
        &Utc::now().to_rfc3339_opts(SecondsFormat::Nanos, true),
        config.log_retention_days.max(1),
    )?;
    let retention_cutoff = parse_rfc3339_utc(&retention_cutoff)
        .context("failed to parse calculated retention cutoff")?;
    // `MIN(created_at)` is NULL on an empty ledger, so the column type has to be
    // stated: `row.get` cannot infer `Option<String>` from the comparison below.
    let oldest_retained_event_at: Option<String> = conn
        .query_row("SELECT MIN(created_at) FROM events", [], |row| row.get(0))
        .context("failed to read oldest retained event")?;
    let retention_lagging = oldest_retained_event_at
        .as_deref()
        .and_then(parse_rfc3339_utc)
        .is_some_and(|oldest_at| oldest_at < retention_cutoff);
    let last_prune_at = get_store_meta(conn, MAINTENANCE_LAST_PRUNE_KEY)?;
    let last_checkpoint_at = get_store_meta(conn, MAINTENANCE_LAST_CHECKPOINT_KEY)?;
    let last_maintenance_error =
        public_maintenance_error(get_store_meta(conn, MAINTENANCE_LAST_ERROR_KEY)?);
    let status = if maintenance_blocked {
        "critical"
    } else if retention_lagging
        || free_disk_bytes < MAINTENANCE_WARN_FREE_DISK_BYTES
        || wal_bytes >= MAINTENANCE_CHECKPOINT_WAL_BYTES
        || last_maintenance_error.is_some()
    {
        "warning"
    } else {
        "ok"
    };

    Ok(StorageHealthSnapshot {
        health: StorageHealth {
            status: status.to_string(),
            database_bytes,
            wal_bytes,
            oldest_retained_event_at,
            prune_age_seconds: maintenance_age_seconds(last_prune_at.as_deref()),
            last_prune_at,
            checkpoint_age_seconds: maintenance_age_seconds(last_checkpoint_at.as_deref()),
            last_checkpoint_at,
            writer_backlog_events,
            writer_backlog_bytes,
            free_disk_bytes,
            maintenance_blocked,
            last_maintenance_error,
        },
        retention_lagging,
        degraded: false,
    })
}

#[cfg(test)]
fn storage_health_with_free_disk_or_unavailable(
    coven_home: &Path,
    free_disk_bytes: u64,
    event_writer: Option<&crate::event_writer::EventWriterHealth>,
) -> StorageHealth {
    storage_health_with_free_disk(coven_home, free_disk_bytes, event_writer).unwrap_or_else(
        |error| unavailable_storage_health(coven_home, error, Some(free_disk_bytes), event_writer),
    )
}

pub fn unavailable_storage_health(
    coven_home: &Path,
    _error: impl ToString,
    known_free_disk_bytes: Option<u64>,
    event_writer: Option<&crate::event_writer::EventWriterHealth>,
) -> StorageHealth {
    let store_path = coven_home.join("coven.sqlite3");
    let (writer_backlog_events, writer_backlog_bytes) = writer_backlog(event_writer);
    StorageHealth {
        status: "degraded".to_string(),
        database_bytes: file_size(&store_path),
        wal_bytes: file_size(&wal_path(&store_path)),
        oldest_retained_event_at: None,
        last_prune_at: None,
        prune_age_seconds: None,
        last_checkpoint_at: None,
        checkpoint_age_seconds: None,
        writer_backlog_events,
        writer_backlog_bytes,
        free_disk_bytes: known_free_disk_bytes.unwrap_or(0),
        maintenance_blocked: false,
        // The socket API is a compatibility boundary. Do not turn an I/O
        // failure into a COVEN_HOME path disclosure; detailed diagnostics stay
        // in the local recovery log.
        last_maintenance_error: Some("storage health unavailable".to_string()),
    }
}

fn writer_backlog(event_writer: Option<&crate::event_writer::EventWriterHealth>) -> (u64, u64) {
    event_writer
        .map(|health| (health.queued_events as u64, health.queued_bytes as u64))
        .unwrap_or_default()
}

pub fn record_maintenance_error(coven_home: &Path, _details: impl ToString) {
    if let Ok(Some(conn)) = open_existing_store_writable(&coven_home.join("coven.sqlite3")) {
        let _ = set_store_meta(
            &conn,
            MAINTENANCE_LAST_ERROR_KEY,
            MAINTENANCE_LAST_ERROR_PUBLIC_MESSAGE,
        );
    }
}

fn public_maintenance_error(value: Option<String>) -> Option<String> {
    value
        .filter(|value| !value.is_empty())
        .map(|_| MAINTENANCE_LAST_ERROR_PUBLIC_MESSAGE.to_string())
}

fn maintenance_age_seconds(timestamp: Option<&str>) -> Option<u64> {
    let timestamp = timestamp?;
    let parsed = chrono::DateTime::parse_from_rfc3339(timestamp).ok()?;
    (Utc::now() - parsed.with_timezone(&Utc))
        .to_std()
        .ok()
        .map(|duration| duration.as_secs())
}

fn parse_rfc3339_utc(timestamp: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    chrono::DateTime::parse_from_rfc3339(timestamp)
        .ok()
        .map(|timestamp| timestamp.with_timezone(&chrono::Utc))
}

fn wal_path(store_path: &Path) -> PathBuf {
    let mut wal_path = store_path.as_os_str().to_os_string();
    wal_path.push("-wal");
    PathBuf::from(wal_path)
}

fn file_size(path: &Path) -> u64 {
    std::fs::metadata(path)
        .map(|metadata| metadata.len())
        .unwrap_or(0)
}

fn get_store_meta(conn: &Connection, key: &str) -> Result<Option<String>> {
    conn.query_row(
        "SELECT value FROM store_meta WHERE key = ?1",
        params![key],
        |row| row.get(0),
    )
    .optional()
    .with_context(|| format!("failed to read store metadata key {key}"))
}

fn set_store_meta(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO store_meta(key, value) VALUES(?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )
    .with_context(|| format!("failed to update store metadata key {key}"))?;
    Ok(())
}

fn retention_duration(days: u64) -> Result<Duration> {
    let days = i64::try_from(days).context("retention days exceed the supported integer range")?;
    Duration::try_days(days).context("retention days exceed the supported duration range")
}

pub fn retention_cutoff(now: &str, days: u64) -> Result<String> {
    let parsed = chrono::DateTime::parse_from_rfc3339(now)
        .map(|dt| dt.with_timezone(&Utc))
        .context("invalid retention cutoff timestamp")?;
    let cutoff = parsed
        .checked_sub_signed(retention_duration(days)?)
        .context("retention cutoff timestamp exceeds the supported date range")?;
    Ok(cutoff.to_rfc3339_opts(SecondsFormat::Nanos, true))
}

fn retention_expires_at(created_at: &str, days: u64) -> Result<String> {
    let parsed = chrono::DateTime::parse_from_rfc3339(created_at)
        .map(|dt| dt.with_timezone(&Utc))
        .context("invalid retention expiry timestamp")?;
    let expires_at = parsed
        .checked_add_signed(retention_duration(days)?)
        .context("retention expiry timestamp exceeds the supported date range")?;
    Ok(expires_at.to_rfc3339_opts(SecondsFormat::Nanos, true))
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

    const LEGACY_WARD_AUDIT_V013_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS ward_audit (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    event_type    TEXT    NOT NULL CHECK (event_type IN (
                      'proposal_submitted','proposal_approved','proposal_rejected',
                      'proposal_vetoed','ward_updated','validation_verdict',
                      'compaction_ledger')),
    proposal_id   TEXT,
    familiar_id   TEXT    NOT NULL,
    ward_version  TEXT,
    ward_hash     BLOB    NOT NULL,
    tier          TEXT,
    decision      TEXT    NOT NULL,
    approver      TEXT,
    diff_hash     BLOB,
    files_touched TEXT    NOT NULL, -- JSON array of surface ids
    channel       TEXT,
    thread_id     TEXT,
    submitted_at  TEXT    NOT NULL, -- RFC 3339
    decided_at    TEXT    NOT NULL, -- RFC 3339
    recorded_at   TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);

CREATE INDEX IF NOT EXISTS ward_audit_familiar_idx ON ward_audit (familiar_id, recorded_at);
CREATE INDEX IF NOT EXISTS ward_audit_event_idx    ON ward_audit (event_type, recorded_at);

CREATE TRIGGER IF NOT EXISTS ward_audit_append_only_update
BEFORE UPDATE ON ward_audit
BEGIN
    SELECT RAISE(ABORT, 'ward_audit is append-only (RFC-0001 §5.6)');
END;

CREATE TRIGGER IF NOT EXISTS ward_audit_append_only_delete
BEFORE DELETE ON ward_audit
BEGIN
    SELECT RAISE(ABORT, 'ward_audit is append-only (RFC-0001 §5.6)');
END;
    "#;

    const UNKNOWN_WARD_AUDIT_WITH_DETAIL_SQL: &str = r#"
        CREATE TABLE ward_audit (
            id            INTEGER PRIMARY KEY AUTOINCREMENT,
            event_type    TEXT    NOT NULL,
            proposal_id   TEXT,
            familiar_id   TEXT    NOT NULL,
            ward_version  TEXT,
            ward_hash     BLOB    NOT NULL,
            tier          TEXT,
            decision      TEXT    NOT NULL,
            approver      TEXT,
            diff_hash     BLOB,
            detail        TEXT,
            files_touched TEXT    NOT NULL,
            channel       TEXT,
            thread_id     TEXT,
            submitted_at  TEXT    NOT NULL,
            decided_at    TEXT    NOT NULL,
            recorded_at   TEXT    NOT NULL
        );
    "#;

    #[derive(Debug, PartialEq, Eq)]
    struct LegacyWardAuditRow {
        id: i64,
        event_type: String,
        proposal_id: Option<String>,
        familiar_id: String,
        ward_version: Option<String>,
        ward_hash: Vec<u8>,
        tier: Option<String>,
        decision: String,
        approver: Option<String>,
        diff_hash: Option<Vec<u8>>,
        detail: Option<String>,
        files_touched: String,
        channel: Option<String>,
        thread_id: Option<String>,
        submitted_at: String,
        decided_at: String,
        recorded_at: String,
    }

    fn assert_ward_audit_schema_state(conn: &Connection, expected: &str) -> Result<()> {
        assert_eq!(load_ward_audit_schema_state(conn)?, expected);
        Ok(())
    }

    fn insert_legacy_ward_audit_row(conn: &Connection, detail: Option<&str>) -> Result<()> {
        if let Some(detail) = detail {
            conn.execute(
                "INSERT INTO ward_audit (
                    id, event_type, proposal_id, familiar_id, ward_version, ward_hash,
                    tier, decision, approver, diff_hash, detail, files_touched,
                    channel, thread_id, submitted_at, decided_at, recorded_at
                 ) VALUES (
                    7, 'proposal_submitted', NULL, 'sage', 'legacy-v1', ?1,
                    'reviewed', 'pending', NULL, ?2, ?4, ?3,
                    'mutation', 'thread-legacy', '2026-07-01T00:00:00Z',
                    '2026-07-01T00:00:01Z', '2026-07-01T00:00:02Z'
                 )",
                params![
                    vec![0x11_u8; 32],
                    vec![0x22_u8; 32],
                    r#"["SOUL.md"]"#,
                    detail
                ],
            )?;
        } else {
            conn.execute(
                "INSERT INTO ward_audit (
                    id, event_type, proposal_id, familiar_id, ward_version, ward_hash,
                    tier, decision, approver, diff_hash, files_touched,
                    channel, thread_id, submitted_at, decided_at, recorded_at
                 ) VALUES (
                    7, 'proposal_submitted', NULL, 'sage', 'legacy-v1', ?1,
                    'reviewed', 'pending', NULL, ?2, ?3,
                    'mutation', 'thread-legacy', '2026-07-01T00:00:00Z',
                    '2026-07-01T00:00:01Z', '2026-07-01T00:00:02Z'
                 )",
                params![vec![0x11_u8; 32], vec![0x22_u8; 32], r#"["SOUL.md"]"#],
            )?;
        }
        Ok(())
    }

    fn assert_legacy_ward_audit_row(
        conn: &Connection,
        expected_detail: Option<&str>,
    ) -> Result<()> {
        let row = conn.query_row(
            "SELECT id, event_type, proposal_id, familiar_id, ward_version, ward_hash,
                    tier, decision, approver, diff_hash, detail, files_touched,
                    channel, thread_id, submitted_at, decided_at, recorded_at
             FROM ward_audit WHERE id = 7",
            [],
            |row| {
                Ok(LegacyWardAuditRow {
                    id: row.get(0)?,
                    event_type: row.get(1)?,
                    proposal_id: row.get(2)?,
                    familiar_id: row.get(3)?,
                    ward_version: row.get(4)?,
                    ward_hash: row.get(5)?,
                    tier: row.get(6)?,
                    decision: row.get(7)?,
                    approver: row.get(8)?,
                    diff_hash: row.get(9)?,
                    detail: row.get(10)?,
                    files_touched: row.get(11)?,
                    channel: row.get(12)?,
                    thread_id: row.get(13)?,
                    submitted_at: row.get(14)?,
                    decided_at: row.get(15)?,
                    recorded_at: row.get(16)?,
                })
            },
        )?;
        assert_eq!(
            row,
            LegacyWardAuditRow {
                id: 7,
                event_type: "proposal_submitted".to_string(),
                proposal_id: None,
                familiar_id: "sage".to_string(),
                ward_version: Some("legacy-v1".to_string()),
                ward_hash: vec![0x11; 32],
                tier: Some("reviewed".to_string()),
                decision: "pending".to_string(),
                approver: None,
                diff_hash: Some(vec![0x22; 32]),
                detail: expected_detail.map(str::to_string),
                files_touched: r#"["SOUL.md"]"#.to_string(),
                channel: Some("mutation".to_string()),
                thread_id: Some("thread-legacy".to_string()),
                submitted_at: "2026-07-01T00:00:00Z".to_string(),
                decided_at: "2026-07-01T00:00:01Z".to_string(),
                recorded_at: "2026-07-01T00:00:02Z".to_string(),
            }
        );
        Ok(())
    }

    #[test]
    fn ward_audit_fresh_store_has_exact_current_fingerprint() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let conn = open_store(&temp.path().join("coven.db"))?;

        assert_ward_audit_schema_state(
            &conn,
            coven_threads_core::WARD_AUDIT_SCHEMA_STATE_CURRENT_V020,
        )?;
        Ok(())
    }

    #[test]
    fn ward_audit_exact_legacy_schema_migrates_without_losing_history() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let path = temp.path().join("coven.db");
        let conn = Connection::open(&path)?;
        conn.execute_batch(LEGACY_WARD_AUDIT_V013_SQL)?;
        insert_legacy_ward_audit_row(&conn, None)?;
        drop(conn);

        let conn = open_store(&path)?;

        assert_legacy_ward_audit_row(&conn, None)?;
        assert_ward_audit_schema_state(
            &conn,
            coven_threads_core::WARD_AUDIT_SCHEMA_STATE_CURRENT_V020,
        )?;
        Ok(())
    }

    #[test]
    fn ward_audit_concurrent_legacy_migrators_converge_on_current() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let path = temp.path().join("coven.db");
        let setup = Connection::open(&path)?;
        setup.execute_batch(LEGACY_WARD_AUDIT_V013_SQL)?;
        insert_legacy_ward_audit_row(&setup, None)?;
        drop(setup);

        let connections = (0..2)
            .map(|_| {
                let conn = Connection::open(&path)?;
                configure_initializing_connection(&conn)?;
                let state = load_ward_audit_schema_state(&conn)?;
                assert_eq!(
                    state,
                    coven_threads_core::WARD_AUDIT_SCHEMA_STATE_LEGACY_V013
                );
                Ok((conn, state))
            })
            .collect::<Result<Vec<_>>>()?;
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let handles = connections
            .into_iter()
            .map(|(conn, state)| {
                let barrier = std::sync::Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    apply_ward_audit_schema_state(&conn, &state)
                })
            })
            .collect::<Vec<_>>();

        for handle in handles {
            handle.join().expect("migrator thread panicked")?;
        }
        let conn = Connection::open(&path)?;
        assert_legacy_ward_audit_row(&conn, None)?;
        assert_ward_audit_schema_state(
            &conn,
            coven_threads_core::WARD_AUDIT_SCHEMA_STATE_CURRENT_V020,
        )?;
        Ok(())
    }

    #[test]
    fn ward_audit_unknown_schema_with_detail_fails_closed_and_preserves_history() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let path = temp.path().join("coven.db");
        let conn = Connection::open(&path)?;
        conn.execute_batch(UNKNOWN_WARD_AUDIT_WITH_DETAIL_SQL)?;
        let detail = r#"{"legacy":true}"#;
        insert_legacy_ward_audit_row(&conn, Some(detail))?;
        drop(conn);

        let error = initialize_store(&path).expect_err("unknown schema must fail closed");

        assert!(
            error
                .to_string()
                .contains("unsupported ward_audit schema fingerprint"),
            "unexpected error: {error:#}"
        );
        let conn = Connection::open(&path)?;
        assert_legacy_ward_audit_row(&conn, Some(detail))?;
        Ok(())
    }

    #[test]
    fn ward_audit_current_schema_ignores_obsolete_component_metadata() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let path = temp.path().join("coven.db");
        let conn = open_store(&path)?;
        conn.execute_batch(
            "CREATE TABLE ward_schema_meta (
                component TEXT PRIMARY KEY NOT NULL,
                version INTEGER NOT NULL
             );
             INSERT INTO ward_schema_meta (component, version)
             VALUES ('ward_audit', 21);",
        )?;
        drop(conn);

        initialize_store(&path)?;
        let conn = open_initialized_store(&path)?;

        assert_ward_audit_schema_state(
            &conn,
            coven_threads_core::WARD_AUDIT_SCHEMA_STATE_CURRENT_V020,
        )?;
        Ok(())
    }

    #[test]
    fn ward_audit_current_fingerprint_is_idempotent_across_reopen() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let path = temp.path().join("coven.db");
        drop(open_store(&path)?);

        let conn = open_store(&path)?;

        assert_ward_audit_schema_state(
            &conn,
            coven_threads_core::WARD_AUDIT_SCHEMA_STATE_CURRENT_V020,
        )?;
        Ok(())
    }

    #[test]
    fn ward_audit_unknown_schema_fails_closed_without_rebuild() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let path = temp.path().join("coven.db");
        let conn = Connection::open(&path)?;
        conn.execute_batch(coven_threads_core::WARD_AUDIT_SCHEMA_SQL)?;
        conn.execute_batch("ALTER TABLE ward_audit ADD COLUMN unexpected TEXT")?;
        drop(conn);

        let error = initialize_store(&path).expect_err("schema drift must fail closed");

        assert!(
            error
                .to_string()
                .contains("unsupported ward_audit schema fingerprint"),
            "unexpected error: {error:#}"
        );
        let conn = Connection::open(&path)?;
        let unexpected_column_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM pragma_table_info('ward_audit', 'main')
             WHERE name = 'unexpected'",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(unexpected_column_count, 1);
        Ok(())
    }

    #[test]
    fn ward_audit_unrecognized_fingerprint_state_names_the_value() {
        let conn = Connection::open_in_memory().expect("in-memory database should open");

        let error = apply_ward_audit_schema_state(&conn, "future_v999")
            .expect_err("unrecognized fingerprint state must fail closed");

        assert!(
            error.to_string().contains("future_v999"),
            "unexpected error: {error:#}"
        );
    }

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
    fn executor_result_envelopes_are_append_only_and_replay_safe() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let path = temp_dir.path().join("coven.db");
        let conn = open_store(&path)?;
        let first = ExecutorResultEnvelopeRecord {
            envelope_id: "sha256:first".to_string(),
            job_id: "job-1".to_string(),
            node_id: "node-gpu".to_string(),
            envelope_json: r#"{"jobId":"job-1","status":"completed"}"#.to_string(),
            recorded_at: "2026-08-09T00:00:00Z".to_string(),
        };
        let second = ExecutorResultEnvelopeRecord {
            envelope_id: "sha256:second".to_string(),
            job_id: "job-1".to_string(),
            node_id: "node-gpu".to_string(),
            envelope_json: r#"{"jobId":"job-1","status":"failed"}"#.to_string(),
            recorded_at: "2026-08-09T00:01:00Z".to_string(),
        };

        assert!(append_executor_result_envelope(&conn, &first)?);
        assert!(!append_executor_result_envelope(&conn, &first)?);
        assert!(append_executor_result_envelope(&conn, &second)?);
        drop(conn);

        let reopened = open_store(&path)?;
        assert_eq!(
            list_executor_result_envelopes(&reopened, "job-1")?,
            vec![first, second]
        );
        Ok(())
    }

    #[test]
    fn executor_result_envelopes_backfill_legacy_dispatch_results() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let path = temp_dir.path().join("legacy.db");
        let legacy = Connection::open(&path)?;
        legacy.execute_batch(
            "CREATE TABLE executor_dispatches (
                job_id TEXT PRIMARY KEY NOT NULL,
                node_id TEXT NOT NULL,
                status TEXT NOT NULL,
                job_json TEXT NOT NULL,
                envelope_json TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );
            INSERT INTO executor_dispatches (
                job_id, node_id, status, job_json, envelope_json, created_at, updated_at
            ) VALUES (
                'job-legacy',
                'node-legacy',
                'completed',
                '{\"jobId\":\"job-legacy\"}',
                '{\"jobId\":\"job-legacy\",\"status\":\"completed\"}',
                '2026-07-06T00:00:00Z',
                '2026-07-06T00:01:00Z'
            );",
        )?;
        drop(legacy);

        let conn = open_store(&path)?;
        let records = list_executor_result_envelopes(&conn, "job-legacy")?;

        assert_eq!(records.len(), 1);
        assert_eq!(records[0].envelope_id, "legacy:job-legacy");
        assert_eq!(records[0].node_id, "node-legacy");
        assert_eq!(records[0].recorded_at, "2026-07-06T00:01:00Z");
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
    fn store_initialization_boundary_creates_schema_once_before_lightweight_opens() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let path = temp_dir.path().join("coven.db");

        let unopened = open_initialized_store(&path)?;
        assert!(list_sessions(&unopened).is_err());
        drop(unopened);

        initialize_store(&path)?;
        let conn = open_initialized_store(&path)?;
        assert!(list_sessions(&conn)?.is_empty());
        Ok(())
    }

    #[test]
    fn open_store_only_initializes_an_unseen_path_once_per_process() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let path = temp_dir.path().join("coven.db");

        drop(open_store(&path)?);
        assert_eq!(initialization_count(&path), 1);
        drop(open_store(&path)?);
        assert_eq!(initialization_count(&path), 1);
        Ok(())
    }

    #[test]
    fn concurrent_store_initialization_serializes_migrations() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let path = temp_dir.path().join("coven.db");
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let handles = (0..2)
            .map(|_| {
                let path = path.clone();
                let barrier = std::sync::Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    initialize_store(&path)
                })
            })
            .collect::<Vec<_>>();

        for handle in handles {
            handle.join().expect("initializer thread panicked")?;
        }
        let conn = open_initialized_store(&path)?;
        assert!(list_sessions(&conn)?.is_empty());
        assert_ward_audit_schema_state(
            &conn,
            coven_threads_core::WARD_AUDIT_SCHEMA_STATE_CURRENT_V020,
        )?;
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
    fn list_session_page_continues_in_created_at_and_id_order() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let conn = open_store(&temp_dir.path().join("coven.db"))?;
        for (id, created_at) in [
            ("oldest", "2026-04-27T06:00:00Z"),
            ("middle-a", "2026-04-27T07:00:00Z"),
            ("middle-b", "2026-04-27T07:00:00Z"),
            ("newest", "2026-04-27T08:00:00Z"),
        ] {
            insert_session(&conn, &session_record(id, created_at))?;
        }

        let first = list_session_page(
            &conn,
            SessionListQuery {
                limit: 2,
                cursor: None,
                include_archived: false,
            },
        )?;
        assert_eq!(
            first
                .sessions
                .iter()
                .map(|session| session.id.as_str())
                .collect::<Vec<_>>(),
            vec!["newest", "middle-b"]
        );
        let second = list_session_page(
            &conn,
            SessionListQuery {
                limit: 2,
                cursor: first.next_cursor.as_deref(),
                include_archived: false,
            },
        )?;
        assert_eq!(
            second
                .sessions
                .iter()
                .map(|session| session.id.as_str())
                .collect::<Vec<_>>(),
            vec!["middle-a", "oldest"]
        );
        assert!(second.next_cursor.is_none());
        Ok(())
    }

    #[test]
    fn list_session_page_rejects_invalid_limits_and_cursors() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let conn = open_store(&temp_dir.path().join("coven.db"))?;

        for limit in [0, MAX_SESSION_PAGE_LIMIT + 1] {
            assert!(list_session_page(
                &conn,
                SessionListQuery {
                    limit,
                    cursor: None,
                    include_archived: false,
                },
            )
            .is_err());
        }
        assert!(list_session_page(
            &conn,
            SessionListQuery {
                limit: 1,
                cursor: Some("not-a-valid-cursor"),
                include_archived: false,
            },
        )
        .is_err());
        Ok(())
    }

    #[test]
    fn list_session_page_respects_archived_filter() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let conn = open_store(&temp_dir.path().join("coven.db"))?;
        insert_session(&conn, &session_record("active", "2026-04-27T06:00:00Z"))?;
        insert_session(&conn, &session_record("archived", "2026-04-27T07:00:00Z"))?;
        archive_session(&conn, "archived", "2026-04-27T08:00:00Z")?;

        for (include_archived, expected_ids) in
            [(false, vec!["active"]), (true, vec!["archived", "active"])]
        {
            let page = list_session_page(
                &conn,
                SessionListQuery {
                    limit: 10,
                    cursor: None,
                    include_archived,
                },
            )?;
            assert_eq!(
                page.sessions
                    .iter()
                    .map(|session| session.id.as_str())
                    .collect::<Vec<_>>(),
                expected_ids
            );
        }
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
    fn conditionally_updates_session_status_only_from_expected_current_status() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let conn = open_store(&temp_dir.path().join("coven.db"))?;
        let mut session = session_record("session-1", "2026-04-27T06:00:00Z");
        session.status = "running".to_string();
        insert_session(&conn, &session)?;

        assert!(update_session_status_if_current(
            &conn,
            "session-1",
            "running",
            "killed",
            None,
            "2026-04-27T06:01:00Z",
        )?);
        assert!(!update_session_status_if_current(
            &conn,
            "session-1",
            "running",
            "failed",
            Some(1),
            "2026-04-27T06:02:00Z",
        )?);

        let stored = get_session(&conn, "session-1")?.expect("session should exist");
        assert_eq!(stored.status, "killed");
        assert_eq!(stored.exit_code, None);
        assert_eq!(stored.updated_at, "2026-04-27T06:01:00Z");
        Ok(())
    }

    #[test]
    fn activation_compare_and_set_never_overwrites_terminal() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let conn = open_store(&temp_dir.path().join("coven.db"))?;

        for status in ["failed", "completed", "cancelled"] {
            let mut session = session_record(status, "2026-04-27T06:00:00Z");
            session.status = status.to_string();
            session.exit_code = Some(17);
            insert_session(&conn, &session)?;

            assert!(!update_session_status_if_current(
                &conn,
                status,
                "created",
                "running",
                None,
                "2026-04-27T07:00:00Z",
            )?);
            assert_eq!(get_session(&conn, status)?, Some(session));
        }
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
    fn orphan_reaper_skips_external_sessions() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let conn = open_store(&temp_dir.path().join("coven.db"))?;

        // A normal running session: should be orphaned.
        let mut non_external = session_record("non-external-running", "2026-04-27T06:00:00Z");
        non_external.status = "running".to_string();
        non_external.external = false;

        // An external running session: must NOT be orphaned.
        let mut external = session_record("external-running", "2026-04-27T06:00:00Z");
        external.status = "running".to_string();
        external.external = true;

        insert_session(&conn, &non_external)?;
        insert_session(&conn, &external)?;

        let updated = mark_running_sessions_orphaned(&conn, "2026-04-27T07:00:00Z")?;
        assert_eq!(updated, 1, "only the non-external session should be reaped");

        let sessions = list_sessions(&conn)?;
        let ne = sessions
            .iter()
            .find(|s| s.id == "non-external-running")
            .unwrap();
        let ex = sessions
            .iter()
            .find(|s| s.id == "external-running")
            .unwrap();
        assert_eq!(ne.status, "orphaned");
        assert_eq!(ex.status, "running", "external session must stay running");
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
    fn stale_created_recovery_excludes_launch_adoptions_and_reservations() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let conn = open_store(&temp_dir.path().join("coven.db"))?;
        let keyed_binding = execution_binding_fixture();
        let reserved_binding = execution_binding_with_attempt("reserved-attempt");

        let mut unadopted = session_record("unadopted", "2026-04-27T06:00:00Z");
        unadopted.status = "created".to_string();
        let mut keyed = session_record("keyed", "2026-04-27T06:00:01Z");
        keyed.status = "created".to_string();
        keyed.execution_binding = Some(keyed_binding.clone());
        let mut reserved = session_record("reserved", "2026-04-27T06:00:02Z");
        reserved.status = "created".to_string();
        reserved.execution_binding = Some(reserved_binding.clone());
        insert_session(&conn, &unadopted)?;
        insert_session(&conn, &keyed)?;
        insert_session(&conn, &reserved)?;

        insert_launch_adoption(
            &conn,
            "keyed-adoption",
            "keyed",
            &request_adoption_fixture("keyed-launch", &keyed_binding.request_digest),
            &keyed_binding,
            "2026-04-27T06:00:01Z",
        )?;
        migrate_historical_request_adoptions(&conn)?;
        let keyed_before = get_session(&conn, "keyed")?.expect("keyed session");
        let reserved_before = get_session(&conn, "reserved")?.expect("reserved session");

        assert_eq!(
            mark_stale_created_sessions_failed(
                &conn,
                "2026-04-27T06:50:00Z",
                "2026-04-27T07:00:00Z",
            )?,
            1
        );
        assert_eq!(
            get_session(&conn, "unadopted")?
                .expect("unadopted session")
                .status,
            "failed"
        );
        assert_eq!(get_session(&conn, "keyed")?, Some(keyed_before));
        assert_eq!(get_session(&conn, "reserved")?, Some(reserved_before));
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
    fn adopted_session_sacrifice_returns_typed_retention_error() -> Result<()> {
        const DENIAL: &str = "session adoption evidence is retained; sacrifice is unavailable until an approved retention/fence contract resolves it";

        for evidence in ["launch", "reservation", "input"] {
            let temp_dir = tempfile::tempdir()?;
            let conn = open_store(&temp_dir.path().join(format!("{evidence}.db")))?;
            let binding = execution_binding_fixture();
            let mut session = session_record("retained-session", "2026-04-27T06:00:00Z");
            session.status = "completed".to_string();
            session.execution_binding = Some(binding.clone());
            insert_session(&conn, &session)?;

            match evidence {
                "launch" => insert_launch_adoption(
                    &conn,
                    "launch-adoption",
                    "retained-session",
                    &request_adoption_fixture("launch-key", &binding.request_digest),
                    &binding,
                    "2026-04-27T06:01:00Z",
                )?,
                "reservation" => migrate_historical_request_adoptions(&conn)?,
                "input" => insert_input_adoption(
                    &conn,
                    "input-adoption",
                    "retained-session",
                    &request_adoption_fixture("input-key", &digest_fixture('d')),
                    &binding,
                    "2026-04-27T06:01:00Z",
                )?,
                _ => unreachable!(),
            }

            let preflight_error = ensure_session_sacrificable(&conn, "retained-session")
                .expect_err("preflight must reject retained adoption evidence");
            assert!(preflight_error.is::<AdoptionRetentionError>());
            assert_eq!(preflight_error.to_string(), DENIAL);
            let error = sacrifice_session(&conn, "retained-session")
                .expect_err("retained adoption evidence must block sacrifice");
            assert!(error.is::<AdoptionRetentionError>());
            assert_eq!(error.to_string(), DENIAL);
            assert_eq!(
                format!("{:?}", error.root_cause()),
                "AdoptionRetentionError"
            );
            assert_eq!(
                get_session(&conn, "retained-session")?,
                Some(session),
                "{evidence} evidence must preserve the session"
            );
            assert_eq!(
                conn.query_row(
                    "SELECT COUNT(*) FROM request_adoptions WHERE session_id = ?1",
                    ["retained-session"],
                    |row| row.get::<_, i64>(0),
                )?,
                1,
                "{evidence} evidence must remain retained"
            );
        }
        Ok(())
    }

    #[test]
    fn foreign_key_blocks_concurrent_sacrifice_after_preflight() -> Result<()> {
        const DENIAL: &str = "session adoption evidence is retained; sacrifice is unavailable until an approved retention/fence contract resolves it";
        let temp_dir = tempfile::tempdir()?;
        let path = temp_dir.path().join("race.db");
        let delete_conn = open_store(&path)?;
        let binding = execution_binding_fixture();
        let mut session = session_record("race-session", "2026-04-27T06:00:00Z");
        session.status = "completed".to_string();
        session.execution_binding = Some(binding.clone());
        insert_session(&delete_conn, &session)?;

        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let insert_barrier = std::sync::Arc::clone(&barrier);
        let insert_path = path.clone();
        let insert_binding = binding.clone();
        let insert_handle = std::thread::spawn(move || -> Result<()> {
            let insert_conn = open_store(&insert_path)?;
            insert_barrier.wait();
            insert_launch_adoption(
                &insert_conn,
                "racing-adoption",
                "race-session",
                &request_adoption_fixture("racing-key", &insert_binding.request_digest),
                &insert_binding,
                "2026-04-27T06:01:00Z",
            )
        });

        let error = sacrifice_session_with_pre_delete_hook(
            &delete_conn,
            "race-session",
            || -> Result<()> {
                barrier.wait();
                insert_handle.join().expect("adoption insert thread")
            },
        )
        .expect_err("the racing adoption must block the session delete");

        assert!(error.is::<AdoptionRetentionError>());
        assert_eq!(error.to_string(), DENIAL);
        assert_eq!(get_session(&delete_conn, "race-session")?, Some(session));
        assert_eq!(
            load_launch_adoption_for_session(&delete_conn, "race-session")?
                .expect("racing adoption must remain")
                .id,
            "racing-adoption"
        );
        Ok(())
    }

    #[test]
    fn sacrifice_refuses_session_that_becomes_running_after_preflight() -> Result<()> {
        const DENIAL: &str = "session `race-session` is still running; do not sacrifice live work";
        let temp_dir = tempfile::tempdir()?;
        let path = temp_dir.path().join("status-race.db");
        let delete_conn = open_store(&path)?;
        let mut session = session_record("race-session", "2026-04-27T06:00:00Z");
        session.status = "created".to_string();
        insert_session(&delete_conn, &session)?;
        insert_json_event(
            &delete_conn,
            "race-session",
            "output",
            &serde_json::json!({ "data": "keep me" }),
            "2026-04-27T06:01:00Z",
        )?;
        let status_conn = open_store(&path)?;

        let error = sacrifice_session_with_pre_delete_hook(&delete_conn, "race-session", || {
            update_session_status(
                &status_conn,
                "race-session",
                "running",
                None,
                "2026-04-27T06:02:00Z",
            )
        })
        .expect_err("a session that becomes running must survive sacrifice");

        assert_eq!(error.to_string(), DENIAL);
        let retained = get_session(&delete_conn, "race-session")?.expect("session retained");
        assert_eq!(retained.status, "running");
        assert_eq!(list_events(&delete_conn, "race-session")?.len(), 1);
        Ok(())
    }

    #[test]
    fn unadopted_non_running_session_remains_sacrificable() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let conn = open_store(&temp_dir.path().join("coven.db"))?;
        let mut session = session_record("session-1", "2026-04-27T06:00:00Z");
        session.status = "completed".to_string();
        insert_session(&conn, &session)?;

        sacrifice_session(&conn, "session-1")?;

        assert!(get_session(&conn, "session-1")?.is_none());
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
    fn raw_artifact_zero_day_retention_uses_the_one_day_minimum() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        std::fs::write(
            temp_dir.path().join("privacy.toml"),
            "persist_raw_artifacts = true\nraw_artifact_retention_days = 0\n",
        )?;
        let conn = open_store(&temp_dir.path().join("coven.db"))?;
        insert_session(&conn, &session_record("session-1", "2026-04-27T06:00:00Z"))?;
        let fake = fake_openai_key();
        let record = EventRecord {
            seq: 0,
            id: "event-zero-retention".to_string(),
            session_id: "session-1".to_string(),
            kind: "output".to_string(),
            payload_json: serde_json::json!({ "data": format!("secret {fake}") }).to_string(),
            created_at: "2026-04-27T06:01:00Z".to_string(),
        };

        insert_event_with_privacy(&conn, temp_dir.path(), &record)?;

        let artifact = get_sensitive_artifact(&conn, "session-1", "event-zero-retention")?
            .expect("artifact should use the minimum retention instead of expiring immediately");
        assert_eq!(artifact.expires_at, "2026-04-28T06:01:00.000000000Z");
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
    fn raw_artifact_unrepresentable_retention_keeps_redacted_event_only() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        std::fs::write(
            temp_dir.path().join("privacy.toml"),
            "persist_raw_artifacts = true\nraw_artifact_retention_days = 100000000\n",
        )?;
        let conn = open_store(&temp_dir.path().join("coven.db"))?;
        insert_session(&conn, &session_record("session-1", "2026-04-27T06:00:00Z"))?;
        let fake = fake_openai_key();
        let record = EventRecord {
            seq: 0,
            id: "event-overflow".to_string(),
            session_id: "session-1".to_string(),
            kind: "input".to_string(),
            payload_json: serde_json::json!({ "data": format!("secret {fake}") }).to_string(),
            created_at: "2026-04-27T06:01:00Z".to_string(),
        };

        insert_event_with_privacy(&conn, temp_dir.path(), &record)?;

        let (payload, status): (String, String) = conn.query_row(
            "SELECT payload_json, redaction_status FROM events WHERE id = 'event-overflow'",
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
        let cutoff = retention_cutoff("2026-05-01T00:00:00Z", 7)?;
        let pruned_events = prune_events_older_than(&conn, &cutoff)?;

        assert_eq!(pruned_artifacts, 1);
        assert_eq!(pruned_events, 1);
        let events = list_events(&conn, "session-1")?;
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].payload_json, r#"{"data":"fresh-event"}"#);
        Ok(())
    }

    #[test]
    fn bounded_event_pruning_keeps_fts_consistent_across_an_interruption() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let path = temp_dir.path().join("coven.db");
        let conn = open_store(&path)?;
        insert_session(&conn, &session_record("session-1", "2026-04-27T06:00:00Z"))?;
        for id in ["old-one", "old-two", "old-three"] {
            insert_event(
                &conn,
                &EventRecord {
                    seq: 0,
                    id: id.to_string(),
                    session_id: "session-1".to_string(),
                    kind: "output".to_string(),
                    payload_json: serde_json::json!({ "data": id }).to_string(),
                    created_at: "2026-04-01T00:00:00Z".to_string(),
                },
            )?;
        }

        // Model a process interruption after the DELETE starts but before its
        // transaction commits. Dropping the transaction must restore both the
        // events table and the FTS trigger side effects.
        {
            let tx = conn.unchecked_transaction()?;
            tx.execute(
                "DELETE FROM events WHERE rowid IN (
                    SELECT rowid FROM events WHERE created_at < ?1 LIMIT 1
                 )",
                ["2026-04-15T00:00:00Z"],
            )?;
        }
        assert_eq!(search_events(&conn, "old-one")?.len(), 1);

        let pruned = prune_events_older_than_bounded(&conn, "2026-04-15T00:00:00Z", 2)?;
        assert_eq!(pruned, 2);
        assert_eq!(list_events(&conn, "session-1")?.len(), 1);
        assert_eq!(search_events(&conn, "old-one")?.len(), 0);
        assert_eq!(pragma_integrity_check(&conn)?, vec!["ok"]);
        Ok(())
    }

    #[test]
    fn scheduled_maintenance_prunes_retention_without_vacuuming() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let home = temp_dir.path();
        let path = home.join("coven.sqlite3");
        let conn = open_store(&path)?;
        insert_session(&conn, &session_record("session-1", "2026-04-27T06:00:00Z"))?;
        insert_event(
            &conn,
            &EventRecord {
                seq: 0,
                id: "expired".to_string(),
                session_id: "session-1".to_string(),
                kind: "output".to_string(),
                payload_json: r#"{"data":"expired"}"#.to_string(),
                created_at: "2026-01-01T00:00:00Z".to_string(),
            },
        )?;
        drop(conn);

        let report = run_scheduled_maintenance_with_free_disk(
            home,
            "2026-04-27T06:00:00Z",
            MAINTENANCE_WARN_FREE_DISK_BYTES,
        )?;
        assert_eq!(report.events_pruned, 1);
        assert!(!report.checkpoint_ran);
        assert!(!report.blocked_by_free_disk);

        let health = storage_health(home, None)?;
        assert_eq!(
            health.last_prune_at.as_deref(),
            Some("2026-04-27T06:00:00Z")
        );
        assert_eq!(health.oldest_retained_event_at, None);
        assert_eq!(health.writer_backlog_events, 0);
        assert_eq!(health.writer_backlog_bytes, 0);
        Ok(())
    }

    #[test]
    fn scheduled_maintenance_honors_configured_event_retention() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let home = temp_dir.path();
        let path = home.join("coven.sqlite3");
        let conn = open_store(&path)?;
        insert_session(&conn, &session_record("session-1", "2026-04-27T06:00:00Z"))?;
        insert_event(
            &conn,
            &EventRecord {
                seq: 0,
                id: "configured-expired".to_string(),
                session_id: "session-1".to_string(),
                kind: "output".to_string(),
                payload_json: r#"{"data":"expired"}"#.to_string(),
                created_at: "2026-04-25T00:00:00Z".to_string(),
            },
        )?;
        drop(conn);

        let config = PrivacyConfig {
            persist_raw_artifacts: false,
            raw_artifact_retention_days: 7,
            log_retention_days: 1,
            extra_patterns: Vec::new(),
        };
        let report = run_scheduled_maintenance_with_config_and_free_disk(
            home,
            "2026-04-27T06:00:00Z",
            MAINTENANCE_MIN_FREE_DISK_BYTES,
            &config,
        )?;

        assert_eq!(report.events_pruned, 1);
        let conn = open_store(&path)?;
        assert!(list_events(&conn, "session-1")?.is_empty());
        Ok(())
    }

    #[test]
    fn scheduled_maintenance_unrepresentable_retention_preserves_existing_rows() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let home = temp_dir.path();
        let path = home.join("coven.sqlite3");
        let conn = open_store(&path)?;
        insert_session(&conn, &session_record("session-1", "2026-04-27T06:00:00Z"))?;
        insert_event(
            &conn,
            &EventRecord {
                seq: 0,
                id: "retained-event".to_string(),
                session_id: "session-1".to_string(),
                kind: "output".to_string(),
                payload_json: r#"{"data":"retain me"}"#.to_string(),
                created_at: "2026-04-25T00:00:00Z".to_string(),
            },
        )?;
        drop(conn);
        let config = PrivacyConfig {
            persist_raw_artifacts: false,
            raw_artifact_retention_days: 7,
            log_retention_days: u64::MAX,
            extra_patterns: Vec::new(),
        };

        run_scheduled_maintenance_with_config_and_free_disk(
            home,
            "2026-04-27T06:00:00Z",
            MAINTENANCE_MIN_FREE_DISK_BYTES,
            &config,
        )
        .expect_err("unrepresentable retention must fail closed");

        let conn = open_store(&path)?;
        assert_eq!(list_events(&conn, "session-1")?.len(), 1);
        Ok(())
    }

    #[test]
    fn scheduled_maintenance_unrepresentable_retention_does_not_create_store() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let home = temp_dir.path();
        let config = PrivacyConfig {
            persist_raw_artifacts: false,
            raw_artifact_retention_days: u64::MAX,
            log_retention_days: 30,
            extra_patterns: Vec::new(),
        };

        run_scheduled_maintenance_with_config_and_free_disk(
            home,
            "2026-04-27T06:00:00Z",
            MAINTENANCE_MIN_FREE_DISK_BYTES,
            &config,
        )
        .expect_err("unrepresentable retention must fail before opening the store");

        assert!(!home.join("coven.sqlite3").exists());
        Ok(())
    }

    #[test]
    fn scheduled_maintenance_invalid_timestamp_does_not_create_store() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let home = temp_dir.path();

        run_scheduled_maintenance_with_config_and_free_disk(
            home,
            "not-a-timestamp",
            MAINTENANCE_MIN_FREE_DISK_BYTES,
            &PrivacyConfig::default(),
        )
        .expect_err("invalid maintenance timestamps must fail before opening the store");

        assert!(!home.join("coven.sqlite3").exists());
        Ok(())
    }

    #[test]
    fn scheduled_maintenance_timestamp_range_overflow_does_not_create_store() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let home = temp_dir.path();
        let config = PrivacyConfig {
            persist_raw_artifacts: false,
            raw_artifact_retention_days: 7,
            log_retention_days: 100_000_000,
            extra_patterns: Vec::new(),
        };

        run_scheduled_maintenance_with_config_and_free_disk(
            home,
            "2026-04-27T06:00:00Z",
            MAINTENANCE_MIN_FREE_DISK_BYTES,
            &config,
        )
        .expect_err("timestamp range overflow must fail before opening the store");

        assert!(!home.join("coven.sqlite3").exists());
        Ok(())
    }

    #[test]
    fn scheduled_maintenance_below_watermark_does_not_open_or_write_the_store() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let home = temp_dir.path();

        let report = run_scheduled_maintenance_with_free_disk(
            home,
            "2026-04-27T06:00:00Z",
            MAINTENANCE_MIN_FREE_DISK_BYTES - 1,
        )?;

        assert!(report.blocked_by_free_disk);
        assert_eq!(report.events_pruned, 0);
        assert_eq!(report.raw_artifacts_pruned, 0);
        assert!(
            !home.join("coven.sqlite3").exists(),
            "the safety path must not create a database or WAL writes"
        );
        Ok(())
    }

    #[test]
    fn scheduled_maintenance_with_malformed_privacy_config_returns_err_and_preserves_rows(
    ) -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let home = temp_dir.path();
        let path = home.join("coven.sqlite3");
        let conn = open_store(&path)?;
        insert_session(&conn, &session_record("session-1", "2026-04-27T06:00:00Z"))?;
        insert_event(
            &conn,
            &EventRecord {
                seq: 0,
                id: "prunable-event".to_string(),
                session_id: "session-1".to_string(),
                kind: "output".to_string(),
                payload_json: r#"{"data":"expired"}"#.to_string(),
                created_at: "2026-04-25T00:00:00Z".to_string(),
            },
        )?;
        drop(conn);
        std::fs::write(
            home.join("privacy.toml"),
            "log_retention_days = \"broken\"\n",
        )?;

        let error = run_scheduled_maintenance(home, "2026-04-27T06:00:00Z")
            .expect_err("malformed privacy config must fail closed");

        assert!(error.to_string().contains("privacy"));
        let conn = open_store(&path)?;
        assert_eq!(list_events(&conn, "session-1")?.len(), 1);
        Ok(())
    }

    #[test]
    fn storage_health_below_watermark_does_not_create_missing_store_and_returns_critical(
    ) -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let home = temp_dir.path();
        std::fs::write(
            home.join("privacy.toml"),
            "log_retention_days = \"broken\"\n",
        )?;
        let writer = crate::event_writer::EventWriterHealth {
            state: "pressured".to_string(),
            queued_events: 7,
            queued_bytes: 8192,
            capacity_bytes: 2 * 1024 * 1024,
            dropped_output_events: 1,
            dropped_output_bytes: 512,
            connection_opens: 1,
            transactions: 3,
            committed_events: 12,
            last_error: None,
        };

        let health = storage_health_with_free_disk(
            home,
            MAINTENANCE_MIN_FREE_DISK_BYTES - 1,
            Some(&writer),
        )?;

        assert_eq!(health.status, "critical");
        assert!(health.maintenance_blocked);
        assert_eq!(health.database_bytes, 0);
        assert_eq!(health.wal_bytes, 0);
        assert_eq!(health.writer_backlog_events, 7);
        assert_eq!(health.writer_backlog_bytes, 8192);
        assert_eq!(health.free_disk_bytes, MAINTENANCE_MIN_FREE_DISK_BYTES - 1);
        assert!(health.oldest_retained_event_at.is_none());
        assert!(health.last_prune_at.is_none());
        assert!(health.last_checkpoint_at.is_none());
        assert!(health.last_maintenance_error.is_none());
        assert!(!home.join("coven.sqlite3").exists());
        Ok(())
    }

    fn storage_health_after_sampling_free_disk_for_test(
        coven_home: &Path,
        free_disk_bytes: u64,
        event_writer: Option<&crate::event_writer::EventWriterHealth>,
    ) -> StorageHealth {
        storage_health_with_free_disk_or_unavailable(coven_home, free_disk_bytes, event_writer)
    }

    #[test]
    fn storage_health_fallback_with_known_healthy_free_disk_is_degraded_without_blocking(
    ) -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let home = temp_dir.path();
        let store_path = home.join("coven.sqlite3");
        open_store(&store_path)?;
        let expected_database_bytes = std::fs::metadata(&store_path)?.len();
        let wal_path = wal_path(&store_path);
        std::fs::write(&wal_path, b"wal")?;
        std::fs::write(
            home.join("privacy.toml"),
            "log_retention_days = \"broken\"\n",
        )?;
        let writer = crate::event_writer::EventWriterHealth {
            state: "pressured".to_string(),
            queued_events: 7,
            queued_bytes: 8192,
            capacity_bytes: 2 * 1024 * 1024,
            dropped_output_events: 1,
            dropped_output_bytes: 512,
            connection_opens: 1,
            transactions: 3,
            committed_events: 12,
            last_error: None,
        };

        let health = storage_health_after_sampling_free_disk_for_test(
            home,
            MAINTENANCE_MIN_FREE_DISK_BYTES,
            Some(&writer),
        );

        assert_eq!(health.status, "degraded");
        assert_eq!(health.database_bytes, expected_database_bytes);
        assert_eq!(health.wal_bytes, 3);
        assert_eq!(health.free_disk_bytes, MAINTENANCE_MIN_FREE_DISK_BYTES);
        assert!(!health.maintenance_blocked);
        assert_eq!(health.writer_backlog_events, 7);
        assert_eq!(health.writer_backlog_bytes, 8192);
        assert_eq!(
            health.last_maintenance_error.as_deref(),
            Some("storage health unavailable")
        );
        Ok(())
    }

    #[test]
    fn storage_health_above_watermark_errors_for_missing_store_without_creation() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let home = temp_dir.path();

        let error = storage_health_with_free_disk(home, MAINTENANCE_MIN_FREE_DISK_BYTES, None)
            .expect_err("missing store above the watermark must be unavailable");

        assert!(error.to_string().contains("store"));
        assert!(!home.join("coven.sqlite3").exists());
    }

    #[test]
    fn storage_health_above_watermark_returns_err_for_malformed_privacy_config() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let home = temp_dir.path();
        open_store(&home.join("coven.sqlite3"))?;
        std::fs::write(
            home.join("privacy.toml"),
            "log_retention_days = \"broken\"\n",
        )?;

        let error = storage_health_with_free_disk(home, MAINTENANCE_MIN_FREE_DISK_BYTES, None)
            .expect_err("malformed privacy config must fail closed above the watermark");

        assert!(error.to_string().contains("privacy"));
        Ok(())
    }

    #[test]
    fn cached_storage_health_clears_recovered_disk_and_wal_status() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let home = temp_dir.path();
        open_store(&home.join("coven.sqlite3"))?;
        storage_health_snapshots().write().unwrap().insert(
            home.to_path_buf(),
            StorageHealthSnapshot {
                health: StorageHealth {
                    status: "critical".to_string(),
                    database_bytes: 1,
                    wal_bytes: MAINTENANCE_CHECKPOINT_WAL_BYTES,
                    oldest_retained_event_at: None,
                    last_prune_at: None,
                    prune_age_seconds: None,
                    last_checkpoint_at: None,
                    checkpoint_age_seconds: None,
                    writer_backlog_events: 0,
                    writer_backlog_bytes: 0,
                    free_disk_bytes: MAINTENANCE_MIN_FREE_DISK_BYTES - 1,
                    maintenance_blocked: true,
                    last_maintenance_error: None,
                },
                retention_lagging: false,
                degraded: false,
            },
        );

        let health =
            cached_storage_health_with_free_disk(home, MAINTENANCE_WARN_FREE_DISK_BYTES, None)?;

        assert_eq!(health.wal_bytes, 0);
        assert!(!health.maintenance_blocked);
        assert_eq!(health.status, "ok");
        Ok(())
    }

    #[test]
    fn critical_low_disk_outranks_a_degraded_snapshot_until_disk_recovers() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let home = temp_dir.path();
        let conn = open_store(&home.join("coven.sqlite3"))?;
        refresh_storage_health_snapshot_from_connection_with_free_disk(
            home,
            &conn,
            MAINTENANCE_WARN_FREE_DISK_BYTES,
            None,
        )?;
        mark_storage_health_snapshot_maintenance_failure(
            home,
            Some(MAINTENANCE_WARN_FREE_DISK_BYTES),
            None,
        )?;

        let degraded =
            cached_storage_health_with_free_disk(home, MAINTENANCE_WARN_FREE_DISK_BYTES, None)?;
        assert_eq!(degraded.status, "degraded");
        assert!(!degraded.maintenance_blocked);
        assert_eq!(
            degraded.last_maintenance_error.as_deref(),
            Some(MAINTENANCE_LAST_ERROR_PUBLIC_MESSAGE)
        );

        let critical =
            cached_storage_health_with_free_disk(home, MAINTENANCE_MIN_FREE_DISK_BYTES - 1, None)?;
        assert_eq!(critical.status, "critical");
        assert!(critical.maintenance_blocked);
        assert_eq!(
            critical.last_maintenance_error.as_deref(),
            Some(MAINTENANCE_LAST_ERROR_PUBLIC_MESSAGE)
        );

        let recovered =
            cached_storage_health_with_free_disk(home, MAINTENANCE_WARN_FREE_DISK_BYTES, None)?;
        assert_eq!(recovered.status, "degraded");
        assert!(!recovered.maintenance_blocked);
        assert_eq!(
            recovered.last_maintenance_error.as_deref(),
            Some(MAINTENANCE_LAST_ERROR_PUBLIC_MESSAGE)
        );
        Ok(())
    }

    #[test]
    fn low_disk_refresh_retains_prior_lag_and_error_causes() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let home = temp_dir.path();
        let path = home.join("coven.sqlite3");
        let conn = open_store(&path)?;
        insert_session(&conn, &session_record("session-1", "2010-01-01T00:00:00Z"))?;
        insert_event(
            &conn,
            &EventRecord {
                seq: 0,
                id: "ancient".to_string(),
                session_id: "session-1".to_string(),
                kind: "output".to_string(),
                payload_json: r#"{"data":"ancient"}"#.to_string(),
                created_at: "2010-01-01T00:00:00Z".to_string(),
            },
        )?;
        record_maintenance_error(home, "failed to read /private/home");

        refresh_storage_health_snapshot_from_connection_with_free_disk(
            home,
            &conn,
            MAINTENANCE_WARN_FREE_DISK_BYTES,
            None,
        )?;
        let before =
            cached_storage_health_with_free_disk(home, MAINTENANCE_WARN_FREE_DISK_BYTES, None)?;
        assert_eq!(before.status, "warning");
        assert_eq!(
            before.oldest_retained_event_at.as_deref(),
            Some("2010-01-01T00:00:00Z")
        );
        assert_eq!(
            before.last_maintenance_error.as_deref(),
            Some(MAINTENANCE_LAST_ERROR_PUBLIC_MESSAGE)
        );

        refresh_storage_health_snapshot_from_connection_with_free_disk(
            home,
            &conn,
            MAINTENANCE_MIN_FREE_DISK_BYTES - 1,
            None,
        )?;
        let recovered =
            cached_storage_health_with_free_disk(home, MAINTENANCE_WARN_FREE_DISK_BYTES, None)?;

        assert_eq!(recovered.status, "warning");
        assert_eq!(
            recovered.oldest_retained_event_at,
            before.oldest_retained_event_at
        );
        assert_eq!(
            recovered.last_maintenance_error,
            before.last_maintenance_error
        );
        Ok(())
    }

    #[test]
    fn storage_health_unrepresentable_retention_does_not_open_missing_store() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let home = temp_dir.path();
        std::fs::write(
            home.join("privacy.toml"),
            "log_retention_days = 9223372036854775807\n",
        )?;

        let error = storage_health_with_free_disk(home, MAINTENANCE_MIN_FREE_DISK_BYTES, None)
            .expect_err("unrepresentable retention must fail before opening the store");

        assert!(error.to_string().contains("retention"));
        assert!(!home.join("coven.sqlite3").exists());
        Ok(())
    }

    #[test]
    fn unavailable_storage_health_without_known_free_disk_preserves_backlog_without_blocking(
    ) -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let writer = crate::event_writer::EventWriterHealth {
            state: "pressured".to_string(),
            queued_events: 7,
            queued_bytes: 8192,
            capacity_bytes: 2 * 1024 * 1024,
            dropped_output_events: 1,
            dropped_output_bytes: 512,
            connection_opens: 1,
            transactions: 3,
            committed_events: 12,
            last_error: None,
        };

        let health = unavailable_storage_health(temp_dir.path(), "boom", None, Some(&writer));

        assert_eq!(health.status, "degraded");
        assert_eq!(health.free_disk_bytes, 0);
        assert!(!health.maintenance_blocked);
        assert_eq!(health.writer_backlog_events, 7);
        assert_eq!(health.writer_backlog_bytes, 8192);
        assert_eq!(
            health.last_maintenance_error.as_deref(),
            Some("storage health unavailable")
        );
        Ok(())
    }

    #[test]
    fn thirty_day_synthetic_workload_converges_to_a_stable_store_size() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let home = temp_dir.path();
        let path = home.join("coven.sqlite3");
        let conn = open_store(&path)?;
        insert_session(&conn, &session_record("session-1", "2026-01-01T00:00:00Z"))?;
        drop(conn);

        let payload = "x".repeat(2_048);
        let mut pages_at_steady_state = None;
        let start =
            chrono::DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")?.with_timezone(&Utc);
        for day in 0..65 {
            let created_at =
                (start + Duration::days(day)).to_rfc3339_opts(SecondsFormat::Nanos, true);
            let conn = open_store(&path)?;
            insert_event(
                &conn,
                &EventRecord {
                    seq: 0,
                    id: format!("event-{day}"),
                    session_id: "session-1".to_string(),
                    kind: "output".to_string(),
                    payload_json: serde_json::json!({ "data": payload }).to_string(),
                    created_at: created_at.clone(),
                },
            )?;
            drop(conn);
            run_scheduled_maintenance_with_free_disk(
                home,
                &created_at,
                MAINTENANCE_WARN_FREE_DISK_BYTES,
            )?;

            if day == 45 {
                let conn = open_store(&path)?;
                pages_at_steady_state =
                    Some(conn.query_row("PRAGMA page_count", [], |row| row.get::<_, i64>(0))?);
            }
        }

        let conn = open_store(&path)?;
        let retained: i64 = conn.query_row("SELECT COUNT(*) FROM events", [], |row| row.get(0))?;
        let final_pages: i64 =
            conn.query_row("PRAGMA page_count", [], |row| row.get::<_, i64>(0))?;
        assert!(
            retained <= 31,
            "retention must bound a daily 30-day workload"
        );
        assert!(
            final_pages <= pages_at_steady_state.expect("steady-state sample") + 2,
            "page count should converge once expiration matches ingestion"
        );
        Ok(())
    }

    #[test]
    fn scheduled_maintenance_catches_up_after_backlog() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let home = temp_dir.path();
        let path = home.join("coven.sqlite3");
        let conn = open_store(&path)?;
        insert_session(&conn, &session_record("session-1", "2026-01-01T00:00:00Z"))?;
        for day in 0..1200 {
            insert_event(
                &conn,
                &EventRecord {
                    seq: 0,
                    id: format!("event-{day}"),
                    session_id: "session-1".to_string(),
                    kind: "output".to_string(),
                    payload_json: serde_json::json!({ "data": day }).to_string(),
                    created_at: "2025-01-01T00:00:00Z".to_string(),
                },
            )?;
        }
        drop(conn);

        let report = run_scheduled_maintenance_with_free_disk(
            home,
            "2026-01-31T00:00:00Z",
            MAINTENANCE_WARN_FREE_DISK_BYTES,
        )?;
        assert!(
            report.events_pruned > MAINTENANCE_EVENT_BATCH_SIZE as usize,
            "single tick should advance through multiple bounded maintenance batches"
        );

        let conn = open_store(&path)?;
        let retained: i64 = conn.query_row("SELECT COUNT(*) FROM events", [], |row| row.get(0))?;
        assert_eq!(retained, 0);
        Ok(())
    }

    #[test]
    fn storage_health_flags_stale_retention() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let home = temp_dir.path();
        let path = home.join("coven.sqlite3");
        let conn = open_store(&path)?;
        insert_session(&conn, &session_record("session-1", "2010-01-01T00:00:00Z"))?;
        insert_event(
            &conn,
            &EventRecord {
                seq: 0,
                id: "ancient".to_string(),
                session_id: "session-1".to_string(),
                kind: "output".to_string(),
                payload_json: r#"{"data":"ancient"}"#.to_string(),
                created_at: "2010-01-01T00:00:00Z".to_string(),
            },
        )?;

        let health = storage_health(home, None)?;
        assert_eq!(
            health.oldest_retained_event_at.as_deref(),
            Some("2010-01-01T00:00:00Z")
        );
        assert_ne!(health.status, "ok");
        Ok(())
    }

    #[test]
    fn storage_health_uses_live_writer_backlog() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let home = temp_dir.path();
        open_store(&home.join("coven.sqlite3"))?;
        let writer = crate::event_writer::EventWriterHealth {
            state: "pressured".to_string(),
            queued_events: 7,
            queued_bytes: 8192,
            capacity_bytes: 2 * 1024 * 1024,
            dropped_output_events: 1,
            dropped_output_bytes: 512,
            connection_opens: 1,
            transactions: 3,
            committed_events: 12,
            last_error: None,
        };

        let health = storage_health(home, Some(&writer))?;

        assert_eq!(health.writer_backlog_events, 7);
        assert_eq!(health.writer_backlog_bytes, 8192);
        Ok(())
    }

    #[test]
    fn storage_health_sanitizes_persisted_maintenance_errors() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let home = temp_dir.path();
        open_store(&home.join("coven.sqlite3"))?;

        let path_bearing_error = "store maintenance pass failed: failed to read /private/home";
        record_maintenance_error(home, path_bearing_error);

        let health = storage_health_with_free_disk(home, MAINTENANCE_MIN_FREE_DISK_BYTES, None)?;

        assert_eq!(
            health.last_maintenance_error.as_deref(),
            Some(MAINTENANCE_LAST_ERROR_PUBLIC_MESSAGE)
        );
        assert_ne!(
            health.last_maintenance_error.as_deref(),
            Some(path_bearing_error)
        );
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

        let cutoff = retention_cutoff("2026-04-27T00:00:00Z", 1)?;

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
    fn bounded_sensitive_artifact_pruning_respects_total_limit_and_converges() -> Result<()> {
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
                payload_json: serde_json::json!({ "data": "raw payload" }).to_string(),
                created_at: "2026-04-20T00:00:00Z".to_string(),
            },
        )?;

        for record in [
            SensitiveArtifactRecord {
                id: "expired-only".to_string(),
                session_id: "session-1".to_string(),
                event_id: "event-1".to_string(),
                kind: "output".to_string(),
                nonce: vec![0; 24],
                ciphertext: vec![1, 2, 3],
                created_at: "2026-04-26T12:00:00Z".to_string(),
                expires_at: "2026-04-26T18:00:00Z".to_string(),
            },
            SensitiveArtifactRecord {
                id: "age-only".to_string(),
                session_id: "session-1".to_string(),
                event_id: "event-1".to_string(),
                kind: "output".to_string(),
                nonce: vec![0; 24],
                ciphertext: vec![4, 5, 6],
                created_at: "2026-04-20T00:00:00Z".to_string(),
                expires_at: "2026-05-20T00:00:00Z".to_string(),
            },
            SensitiveArtifactRecord {
                id: "expired-and-aged-1".to_string(),
                session_id: "session-1".to_string(),
                event_id: "event-1".to_string(),
                kind: "output".to_string(),
                nonce: vec![0; 24],
                ciphertext: vec![7, 8, 9],
                created_at: "2026-04-18T00:00:00Z".to_string(),
                expires_at: "2026-04-19T00:00:00Z".to_string(),
            },
            SensitiveArtifactRecord {
                id: "expired-and-aged-2".to_string(),
                session_id: "session-1".to_string(),
                event_id: "event-1".to_string(),
                kind: "output".to_string(),
                nonce: vec![0; 24],
                ciphertext: vec![10, 11, 12],
                created_at: "2026-04-17T00:00:00Z".to_string(),
                expires_at: "2026-04-18T00:00:00Z".to_string(),
            },
            SensitiveArtifactRecord {
                id: "fresh".to_string(),
                session_id: "session-1".to_string(),
                event_id: "event-1".to_string(),
                kind: "output".to_string(),
                nonce: vec![0; 24],
                ciphertext: vec![13, 14, 15],
                created_at: "2026-04-27T00:00:00Z".to_string(),
                expires_at: "2026-05-27T00:00:00Z".to_string(),
            },
        ] {
            insert_sensitive_artifact(&conn, &record)?;
        }

        let now = "2026-04-27T00:00:00Z";
        let cutoff = retention_cutoff(now, 1)?;
        assert_eq!(count_prunable_sensitive_artifacts(&conn, now, &cutoff)?, 4);

        let first_pruned = prune_sensitive_artifacts_bounded(&conn, now, &cutoff, 2)?;
        assert_eq!(first_pruned, 2);
        assert_eq!(count_sensitive_artifacts(&conn)?, 3);
        assert_eq!(count_prunable_sensitive_artifacts(&conn, now, &cutoff)?, 2);

        let second_pruned = prune_sensitive_artifacts_bounded(&conn, now, &cutoff, 2)?;
        assert_eq!(second_pruned, 2);
        assert_eq!(count_sensitive_artifacts(&conn)?, 1);
        assert_eq!(count_prunable_sensitive_artifacts(&conn, now, &cutoff)?, 0);

        let third_pruned = prune_sensitive_artifacts_bounded(&conn, now, &cutoff, 2)?;
        assert_eq!(third_pruned, 0);
        assert!(get_sensitive_artifact(&conn, "session-1", "fresh")?.is_some());
        assert_eq!(pragma_integrity_check(&conn)?, vec!["ok"]);
        Ok(())
    }

    fn explain_query_plan<P: rusqlite::Params>(
        conn: &Connection,
        sql: &str,
        params: P,
    ) -> Result<Vec<String>> {
        let explain_sql = format!("EXPLAIN QUERY PLAN {sql}");
        let mut stmt = conn
            .prepare(&explain_sql)
            .with_context(|| format!("failed to prepare query plan for {sql}"))?;
        let rows = stmt
            .query_map(params, |row| row.get::<_, String>(3))
            .context("failed to run query plan")?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .context("failed to read query plan")
    }

    #[test]
    fn bounded_sensitive_artifact_prune_queries_use_supporting_indexes() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let conn = open_store(&temp_dir.path().join("coven.db"))?;

        let expiry_plan = explain_query_plan(
            &conn,
            BOUNDED_PRUNE_SENSITIVE_ARTIFACTS_BY_EXPIRY_SQL,
            params!["2026-04-27T00:00:00Z", 2],
        )?;
        let retention_plan = explain_query_plan(
            &conn,
            BOUNDED_PRUNE_SENSITIVE_ARTIFACTS_BY_CREATED_AT_SQL,
            params!["2026-04-26T00:00:00Z", 2],
        )?;

        for (plan, expected_index) in [
            (&expiry_plan, "idx_sensitive_artifacts_expires_at"),
            (&retention_plan, "idx_sensitive_artifacts_created_at"),
        ] {
            assert!(
                plan.iter().any(|detail| detail.contains(expected_index)),
                "expected {expected_index} in query plan: {plan:?}"
            );
            assert!(
                !plan
                    .iter()
                    .any(|detail| detail.contains("SCAN sensitive_artifacts")),
                "unexpected full table scan: {plan:?}"
            );
            assert!(
                !plan
                    .iter()
                    .any(|detail| detail.contains("USE TEMP B-TREE FOR ORDER BY")),
                "unexpected temp b-tree sort: {plan:?}"
            );
        }

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
    fn latest_event_seq_returns_zero_for_empty_and_last_rowid_for_session() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let conn = open_store(&temp_dir.path().join("coven.db"))?;
        insert_session(&conn, &session_record("session-1", "2026-04-27T06:00:00Z"))?;

        assert_eq!(latest_event_seq(&conn, "session-1")?, 0);

        for i in 1..=3 {
            insert_json_event(
                &conn,
                "session-1",
                "output",
                &serde_json::json!({ "data": format!("line {i}") }),
                "2026-04-27T06:01:00Z",
            )?;
        }

        assert_eq!(
            latest_event_seq(&conn, "session-1")?,
            list_events(&conn, "session-1")?[2].seq
        );
        Ok(())
    }

    #[test]
    fn unresolved_handoff_events_are_not_pruned() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let mut conn = open_store(&temp_dir.path().join("coven.db"))?;
        let now = "2026-04-27T06:00:00Z";
        let cutoff = "2026-04-15T00:00:00Z";
        insert_session(&conn, &session_record("session-1", now))?;
        insert_json_event(
            &conn,
            "session-1",
            "output",
            &serde_json::json!({ "data": "old handoff event" }),
            "2026-04-01T00:00:00Z",
        )?;
        let offered = create_handoff(&mut conn, "handoff-1", "session-1", "{}", "{}", now)?;
        assert_eq!(offered.event_cursor, latest_event_seq(&conn, "session-1")?);
        assert_eq!(count_events_older_than(&conn, cutoff)?, 0);
        assert_eq!(prune_events_older_than(&conn, cutoff)?, 0);
        assert_eq!(prune_events_older_than_bounded(&conn, cutoff, 1)?, 0);

        claim_handoff(
            &mut conn,
            &offered.id,
            offered.generation,
            "device:phone-1",
            "claim-1",
            now,
        )?;
        assert_eq!(count_events_older_than(&conn, cutoff)?, 0);
        assert_eq!(prune_events_older_than(&conn, cutoff)?, 0);
        assert_eq!(prune_events_older_than_bounded(&conn, cutoff, 1)?, 0);

        let acknowledged = acknowledge_handoff(&mut conn, &offered.id, "device:phone-1", now)?;
        assert_eq!(count_events_older_than(&conn, cutoff)?, 1);
        assert_eq!(prune_events_older_than(&conn, cutoff)?, 1);
        assert!(list_events(&conn, "session-1")?.is_empty());

        assert_eq!(
            acknowledge_handoff(&mut conn, &offered.id, "device:phone-1", now)?,
            acknowledged
        );
        assert_eq!(
            claim_handoff(
                &mut conn,
                &offered.id,
                offered.generation,
                "device:phone-1",
                "claim-1",
                now,
            )?,
            acknowledged
        );
        Ok(())
    }

    #[test]
    fn handoff_transition_rejects_missing_cursor() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let mut conn = open_store(&temp_dir.path().join("coven.db"))?;
        let now = "2026-04-27T06:00:00Z";
        insert_session(&conn, &session_record("session-1", now))?;
        insert_json_event(
            &conn,
            "session-1",
            "output",
            &serde_json::json!({ "data": "handoff event" }),
            now,
        )?;
        let invalid_offered =
            create_handoff(&mut conn, "handoff-missing", "session-1", "{}", "{}", now)?;
        let latest_cursor = latest_event_seq(&conn, "session-1")?;
        conn.execute(
            "UPDATE session_handoffs SET event_cursor = ?2 WHERE id = ?1",
            params![invalid_offered.id, latest_cursor + 1],
        )?;
        let error = claim_handoff(
            &mut conn,
            &invalid_offered.id,
            invalid_offered.generation,
            "device:phone-1",
            "claim-1",
            now,
        )
        .expect_err("claim must reject a missing handoff cursor");
        assert_eq!(error.to_string(), "transcript_diverged");
        assert_eq!(
            get_handoff(&conn, &invalid_offered.id)?.unwrap().state,
            "offered"
        );

        let claimed_offered =
            create_handoff(&mut conn, "handoff-claimed", "session-1", "{}", "{}", now)?;
        claim_handoff(
            &mut conn,
            &claimed_offered.id,
            claimed_offered.generation,
            "device:phone-1",
            "claim-2",
            now,
        )?;
        conn.execute(
            "DELETE FROM events WHERE rowid = ?1",
            [claimed_offered.event_cursor],
        )?;

        let error = acknowledge_handoff(&mut conn, &claimed_offered.id, "device:phone-1", now)
            .expect_err("acknowledgement must reject a removed handoff cursor");
        assert_eq!(error.to_string(), "transcript_diverged");
        assert_eq!(
            get_handoff(&conn, &claimed_offered.id)?.unwrap().state,
            "claimed"
        );
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
            execution_binding: None,
            labels: Vec::new(),
            visibility: "private".to_string(),
            external: false,
            transcript_path: None,
        }
    }

    #[test]
    fn latest_active_returns_newest_non_archived_for_project_and_harness() -> Result<()> {
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
        let hit = latest_active_for_project(&conn, "/p", "codex")?;
        assert_eq!(hit.as_deref(), Some("older"));
        Ok(())
    }

    #[test]
    fn native_conversation_id_round_trips_for_resume_lookup() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let conn = open_store(&temp.path().join("test.sqlite3"))?;
        insert_session(&conn, &session_record("ledger-1", "2026-01-01T00:00:00Z"))?;

        update_session_conversation_id(
            &conn,
            "ledger-1",
            "codex-thread-123",
            "2026-01-02T00:00:00Z",
        )?;

        let ledger = get_session(&conn, "ledger-1")?.expect("ledger row should exist");
        assert_eq!(ledger.conversation_id.as_deref(), Some("codex-thread-123"));
        let resumed = get_latest_session_by_conversation_id(&conn, "codex-thread-123")?
            .expect("native thread id should resolve back to the ledger row");
        assert_eq!(resumed.id, "ledger-1");
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
    fn vacuum_appends_compaction_ledger_for_warded_surface() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let path = temp.path().join("test.sqlite3");
        let conn = open_store(&path)?;
        let pre_commitment = vec![0x42; 32];
        conn.execute(
            "INSERT INTO ward_manifest (familiar_id, surface, manifest_id, entry_hash)
             VALUES ('sage', 'SOUL.md', '11111111-1111-1111-1111-111111111111', ?1)",
            [&pre_commitment],
        )?;
        drop(conn);

        vacuum_store_path(&path)?;

        let conn = open_store(&path)?;
        let row = conn.query_row(
            "SELECT familiar_id, ward_hash, diff_hash, files_touched, channel, decision
             FROM ward_audit WHERE event_type = 'compaction_ledger'",
            [],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Vec<u8>>(1)?,
                    row.get::<_, Option<Vec<u8>>>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, String>(5)?,
                ))
            },
        )?;
        assert_eq!(row.0, "sage");
        assert_eq!(row.1, pre_commitment);
        assert_eq!(row.2, Some(vec![0x42; 32]));
        assert_eq!(
            serde_json::from_str::<Vec<String>>(&row.3)?,
            vec!["SOUL.md".to_string()]
        );
        assert_eq!(row.4.as_deref(), Some("forced"));
        assert_eq!(row.5, "compacted:unchanged");
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM ward_audit WHERE event_type = 'compaction_ledger'",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(count, 1);
        Ok(())
    }

    #[test]
    fn vacuum_without_warded_surfaces_appends_no_compaction_ledger() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let path = temp.path().join("test.sqlite3");
        drop(open_store(&path)?);

        vacuum_store_path(&path)?;

        let conn = open_store(&path)?;
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM ward_audit WHERE event_type = 'compaction_ledger'",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(count, 0);
        Ok(())
    }

    #[test]
    fn compaction_ledger_keeps_ward_audit_append_only() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let path = temp.path().join("test.sqlite3");
        let conn = open_store(&path)?;
        conn.execute(
            "INSERT INTO ward_manifest (familiar_id, surface, manifest_id, entry_hash)
             VALUES ('sage', 'SOUL.md', '11111111-1111-1111-1111-111111111111', ?1)",
            [&vec![0x24; 32]],
        )?;
        drop(conn);

        vacuum_store_path(&path)?;

        let conn = open_store(&path)?;
        let update = conn.execute(
            "UPDATE ward_audit SET decision = 'tampered' WHERE event_type = 'compaction_ledger'",
            [],
        );
        assert!(update.is_err(), "UPDATE must abort on ward_audit");
        let delete = conn.execute(
            "DELETE FROM ward_audit WHERE event_type = 'compaction_ledger'",
            [],
        );
        assert!(delete.is_err(), "DELETE must abort on ward_audit");
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

    #[test]
    fn external_session_fields_round_trip_via_get_and_list() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let conn = open_store(&temp.path().join("test.sqlite3"))?;

        // Insert an external session with a transcript path.
        let external = SessionRecord {
            id: "ext-sess-1".to_string(),
            project_root: "/tmp/proj".to_string(),
            harness: "engine".to_string(),
            title: "Engine run".to_string(),
            status: "running".to_string(),
            exit_code: None,
            archived_at: None,
            created_at: "2026-07-12T10:00:00Z".to_string(),
            updated_at: "2026-07-12T10:00:00Z".to_string(),
            conversation_id: None,
            familiar_id: None,
            execution_binding: None,
            labels: Vec::new(),
            visibility: "private".to_string(),
            external: true,
            transcript_path: Some("/tmp/proj/.claude/transcripts/ext-sess-1.jsonl".to_string()),
        };
        insert_session(&conn, &external)?;

        // Insert a regular (non-external) session without a transcript path.
        let internal = SessionRecord {
            id: "int-sess-1".to_string(),
            project_root: "/tmp/proj".to_string(),
            harness: "codex".to_string(),
            title: "Normal run".to_string(),
            status: "created".to_string(),
            exit_code: None,
            archived_at: None,
            created_at: "2026-07-12T10:01:00Z".to_string(),
            updated_at: "2026-07-12T10:01:00Z".to_string(),
            conversation_id: None,
            familiar_id: None,
            execution_binding: None,
            labels: Vec::new(),
            visibility: "private".to_string(),
            external: false,
            transcript_path: None,
        };
        insert_session(&conn, &internal)?;

        // Round-trip via get_session.
        let got_ext = get_session(&conn, "ext-sess-1")?.expect("external session should exist");
        assert!(got_ext.external, "external flag should be true");
        assert_eq!(
            got_ext.transcript_path.as_deref(),
            Some("/tmp/proj/.claude/transcripts/ext-sess-1.jsonl")
        );

        let got_int = get_session(&conn, "int-sess-1")?.expect("internal session should exist");
        assert!(
            !got_int.external,
            "internal session external flag should be false"
        );
        assert!(
            got_int.transcript_path.is_none(),
            "internal session should have no transcript"
        );

        // Round-trip via list_sessions.
        let all = list_sessions(&conn)?;
        let ext_in_list = all
            .iter()
            .find(|s| s.id == "ext-sess-1")
            .expect("ext in list");
        let int_in_list = all
            .iter()
            .find(|s| s.id == "int-sess-1")
            .expect("int in list");
        assert!(ext_in_list.external);
        assert!(!int_in_list.external);
        assert!(ext_in_list.transcript_path.is_some());
        assert!(int_in_list.transcript_path.is_none());

        Ok(())
    }

    // -------------------------------------------------------------------------
    // execution_binding_json tests (Task 2, #728)
    // -------------------------------------------------------------------------

    fn digest_fixture(byte: char) -> String {
        format!("sha256:{}", byte.to_string().repeat(64))
    }

    /// A single canonical valid binding, reused across round-trip tests so
    /// raw-byte comparisons stay stable.
    fn execution_binding_fixture() -> crate::execution_binding::ExecutionBinding {
        crate::execution_binding::ExecutionBinding {
            contract: crate::execution_binding::CONTRACT.to_string(),
            principal_ref: "principal:operator".to_string(),
            familiar_id: "sage".to_string(),
            familiar_snapshot_digest: digest_fixture('a'),
            project_digest: digest_fixture('b'),
            graph_id: "graph-1".to_string(),
            node_id: "node-1".to_string(),
            attempt_id: "attempt-1".to_string(),
            request_digest: digest_fixture('c'),
            policy_revision: "policy:7".to_string(),
            expires_at: "2099-01-01T00:00:00Z".to_string(),
            parent: None,
            delegation_digest: None,
        }
    }

    fn execution_binding_with_attempt(
        attempt_id: &str,
    ) -> crate::execution_binding::ExecutionBinding {
        let mut binding = execution_binding_fixture();
        binding.attempt_id = attempt_id.to_string();
        binding
    }

    fn request_adoption_fixture(
        key: &str,
        request_digest: &str,
    ) -> crate::request_adoption::RequestAdoption {
        crate::request_adoption::RequestAdoption {
            contract: crate::request_adoption::CONTRACT.to_string(),
            key: key.to_string(),
            request_digest: request_digest.to_string(),
        }
    }

    fn create_pre_o3_store(
        path: &Path,
        sessions: &[(
            &str,
            Option<crate::execution_binding::ExecutionBinding>,
            &str,
        )],
    ) -> Result<()> {
        let conn = Connection::open(path)?;
        conn.execute_batch(
            "PRAGMA foreign_keys = ON;
             CREATE TABLE sessions (
                 id TEXT PRIMARY KEY NOT NULL,
                 project_root TEXT NOT NULL,
                 harness TEXT NOT NULL,
                 title TEXT NOT NULL,
                 status TEXT NOT NULL,
                 created_at TEXT NOT NULL,
                 updated_at TEXT NOT NULL,
                 execution_binding_json TEXT
             );
             CREATE TABLE events (
                 id TEXT PRIMARY KEY NOT NULL,
                 session_id TEXT NOT NULL,
                 kind TEXT NOT NULL,
                 payload_json TEXT NOT NULL,
                 created_at TEXT NOT NULL,
                 FOREIGN KEY (session_id) REFERENCES sessions(id) ON DELETE CASCADE
             );",
        )?;
        for (id, binding, created_at) in sessions {
            let binding_json = binding.as_ref().map(serde_json::to_string).transpose()?;
            conn.execute(
                "INSERT INTO sessions (
                    id, project_root, harness, title, status, created_at, updated_at,
                    execution_binding_json
                 ) VALUES (?1, '/project', 'codex', ?1, 'created', ?2, ?2, ?3)",
                params![id, created_at, binding_json],
            )?;
        }
        Ok(())
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct RawAdoptionSnapshot {
        id: String,
        adoption_key: Option<String>,
        contract: Option<String>,
        operation: String,
        request_digest: String,
        session_id: String,
        execution_binding_json: String,
        principal_ref: Option<String>,
        project_digest: Option<String>,
        graph_id: Option<String>,
        node_id: Option<String>,
        attempt_id: Option<String>,
        adopted_at: String,
    }

    fn raw_adoption_rows(conn: &Connection) -> Result<Vec<RawAdoptionSnapshot>> {
        let mut statement = conn.prepare(
            "SELECT
                id, adoption_key, contract, operation, request_digest, session_id,
                execution_binding_json, principal_ref, project_digest, graph_id,
                node_id, attempt_id, adopted_at
             FROM request_adoptions
             ORDER BY session_id, operation, id",
        )?;
        let rows = statement
            .query_map([], |row| {
                Ok(RawAdoptionSnapshot {
                    id: row.get(0)?,
                    adoption_key: row.get(1)?,
                    contract: row.get(2)?,
                    operation: row.get(3)?,
                    request_digest: row.get(4)?,
                    session_id: row.get(5)?,
                    execution_binding_json: row.get(6)?,
                    principal_ref: row.get(7)?,
                    project_digest: row.get(8)?,
                    graph_id: row.get(9)?,
                    node_id: row.get(10)?,
                    attempt_id: row.get(11)?,
                    adopted_at: row.get(12)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(anyhow::Error::from)?;
        Ok(rows)
    }

    fn event_foreign_keys(conn: &Connection) -> Result<Vec<(String, String, String, String)>> {
        let mut statement = conn.prepare("PRAGMA foreign_key_list(events)")?;
        let foreign_keys = statement
            .query_map([], |row| {
                Ok((row.get(2)?, row.get(3)?, row.get(4)?, row.get(6)?))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(anyhow::Error::from)?;
        Ok(foreign_keys)
    }

    fn assert_event_adoption_foreign_key(conn: &Connection) -> Result<()> {
        let foreign_keys = event_foreign_keys(conn)?;
        assert!(
            foreign_keys.contains(&(
                "request_adoptions".to_string(),
                "request_adoption_id".to_string(),
                "id".to_string(),
                "RESTRICT".to_string(),
            )),
            "events adoption foreign key missing: {foreign_keys:?}"
        );
        assert!(
            foreign_keys.contains(&(
                "sessions".to_string(),
                "session_id".to_string(),
                "id".to_string(),
                "CASCADE".to_string(),
            )),
            "events session foreign key missing: {foreign_keys:?}"
        );
        Ok(())
    }

    /// `request_adoptions_session` must exist as a plain, non-unique index
    /// over every row (launch and input) so retention lookups can seek
    /// instead of scanning the append-only ledger. It must not introduce any
    /// new uniqueness constraint; `request_adoptions_launch_session` remains
    /// the sole (partial, launch-only) unique index on this column.
    fn assert_request_adoptions_session_index_is_non_unique(conn: &Connection) -> Result<()> {
        let mut statement = conn.prepare("PRAGMA index_list(request_adoptions)")?;
        let mut found = false;
        let rows = statement.query_map([], |row| {
            Ok((row.get::<_, String>(1)?, row.get::<_, i64>(2)?))
        })?;
        for row in rows {
            let (name, unique) = row?;
            if name == "request_adoptions_session" {
                found = true;
                assert_eq!(
                    unique, 0,
                    "request_adoptions_session must not be a unique index"
                );
            }
        }
        assert!(found, "request_adoptions_session index is missing");
        Ok(())
    }

    fn assert_sql_error_contains(result: rusqlite::Result<usize>, expected: &str) {
        let error = result.expect_err("raw SQL mutation must fail");
        assert!(
            error.to_string().contains(expected),
            "expected {expected:?}, got {error}"
        );
    }

    #[test]
    fn request_adoptions_fresh_schema_has_required_constraints() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let path = temp.path().join("fresh.sqlite3");
        initialize_store(&path)?;
        let conn = open_initialized_store(&path)?;

        let table_sql: String = conn.query_row(
            "SELECT sql FROM sqlite_master
             WHERE type = 'table' AND name = 'request_adoptions'",
            [],
            |row| row.get(0),
        )?;
        let compact = table_sql.split_whitespace().collect::<Vec<_>>().join(" ");
        for required in [
            "id TEXT PRIMARY KEY NOT NULL",
            "operation TEXT NOT NULL CHECK (operation IN ('launch', 'input'))",
            "FOREIGN KEY (session_id) REFERENCES sessions(id) ON DELETE RESTRICT",
            "adoption_key IS NULL AND contract IS NULL AND operation = 'launch'",
            "adoption_key IS NOT NULL AND contract IS NOT NULL",
            "operation = 'input'",
            "attempt_id IS NULL",
        ] {
            assert!(
                compact.contains(required),
                "missing schema fragment {required:?}: {compact}"
            );
        }

        let schema_objects = {
            let mut statement = conn.prepare(
                "SELECT name FROM sqlite_master
                 WHERE type IN ('index', 'trigger')
                 ORDER BY name",
            )?;
            let names = statement
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            names
        };
        for required in [
            "request_adoptions_key",
            "request_adoptions_launch_attempt",
            "request_adoptions_launch_session",
            "request_adoptions_session",
            "events_request_adoption",
            "events_request_adoption_integrity",
            "events_request_adoption_update_integrity",
            "events_request_adoption_no_rebind",
            "request_adoptions_no_update",
            "request_adoptions_no_delete",
            "request_adoptions_no_replace",
        ] {
            assert!(
                schema_objects.iter().any(|name| name == required),
                "missing schema object {required}: {schema_objects:?}"
            );
        }
        assert_request_adoptions_session_index_is_non_unique(&conn)?;

        assert!(table_columns(&conn, "events")?
            .iter()
            .any(|column| column == "request_adoption_id"));
        assert_event_adoption_foreign_key(&conn)?;
        assert_eq!(
            conn.query_row("PRAGMA foreign_keys", [], |row| row.get::<_, i64>(0))?,
            1
        );
        assert_eq!(
            conn.query_row("PRAGMA recursive_triggers", [], |row| row.get::<_, i64>(0))?,
            1,
            "recursive_triggers must be ON so REPLACE's implicit rowid delete \
             still fires request_adoptions_no_delete"
        );

        let mut session = session_record("bound", "2026-08-15T00:00:00Z");
        session.execution_binding = Some(execution_binding_fixture());
        insert_session(&conn, &session)?;
        let raw = serde_json::to_string(&execution_binding_fixture())?;
        assert!(conn
            .execute(
                "INSERT INTO request_adoptions (
                    id, adoption_key, contract, operation, request_digest, session_id,
                    execution_binding_json, principal_ref, project_digest, graph_id,
                    node_id, attempt_id, adopted_at
                 ) VALUES (
                    'bad-operation', 'key', ?1, 'delete', ?2, 'bound', ?3,
                    NULL, NULL, NULL, NULL, NULL, '2026-08-15T00:00:00Z'
                 )",
                params![crate::request_adoption::CONTRACT, digest_fixture('c'), raw],
            )
            .is_err());
        assert!(conn
            .execute(
                "INSERT INTO request_adoptions (
                    id, adoption_key, contract, operation, request_digest, session_id,
                    execution_binding_json, principal_ref, project_digest, graph_id,
                    node_id, attempt_id, adopted_at
                 ) VALUES (
                    'bad-input', NULL, NULL, 'input', ?1, 'bound', ?2,
                    NULL, NULL, NULL, NULL, NULL, '2026-08-15T00:00:00Z'
                 )",
                params![digest_fixture('d'), raw],
            )
            .is_err());
        Ok(())
    }

    #[test]
    fn every_store_connection_helper_enables_recursive_triggers() -> Result<()> {
        // Every connection path (initializing, runtime writable, read-only,
        // and the writable path reopened after the store already exists)
        // must apply `PRAGMA recursive_triggers = ON`. Without it on any one
        // path, a raw `INSERT OR REPLACE` / `REPLACE INTO` reaching that
        // connection could bypass `request_adoptions_no_delete` via the
        // hidden-rowid conflict path.
        let temp = tempfile::tempdir()?;
        let path = temp.path().join("recursive-triggers.sqlite3");

        fn recursive_triggers_enabled(conn: &Connection) -> Result<bool> {
            Ok(conn.query_row("PRAGMA recursive_triggers", [], |row| row.get::<_, i64>(0))? == 1)
        }

        // `open_store` initializes the schema (configure_initializing_connection)
        // and then hands back a runtime writable connection.
        let opened = open_store(&path)?;
        assert!(recursive_triggers_enabled(&opened)?);
        drop(opened);

        // `open_initialized_store` is the ordinary per-request writable path.
        let initialized = open_initialized_store(&path)?;
        assert!(recursive_triggers_enabled(&initialized)?);
        drop(initialized);

        // `open_existing_store_read_only` is the read-only path used by
        // reporting/inspection commands.
        let read_only = open_existing_store_read_only(&path)?.expect("store exists");
        assert!(recursive_triggers_enabled(&read_only)?);
        drop(read_only);

        // `open_existing_store_writable` is the writable path reopened
        // against a store that already exists.
        let writable = open_existing_store_writable(&path)?.expect("store exists");
        assert!(recursive_triggers_enabled(&writable)?);
        drop(writable);

        Ok(())
    }

    #[test]
    fn request_adoptions_migrate_every_bound_session_once() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let path = temp.path().join("pre-o3.sqlite3");
        let first = execution_binding_fixture();
        let second = execution_binding_with_attempt("attempt-2");
        create_pre_o3_store(
            &path,
            &[
                ("bound-1", Some(first.clone()), "2026-08-01T00:00:00Z"),
                ("unbound", None, "2026-08-01T00:00:01Z"),
                ("bound-2", Some(second.clone()), "2026-08-01T00:00:02Z"),
            ],
        )?;

        initialize_store(&path)?;
        let conn = open_initialized_store(&path)?;
        let rows = raw_adoption_rows(&conn)?;
        assert_eq!(rows.len(), 2);
        assert_request_adoptions_session_index_is_non_unique(&conn)?;
        for (row, binding, created_at) in [
            (&rows[0], &first, "2026-08-01T00:00:00Z"),
            (&rows[1], &second, "2026-08-01T00:00:02Z"),
        ] {
            assert_eq!(row.adoption_key, None);
            assert_eq!(row.contract, None);
            assert_eq!(row.operation, "launch");
            assert_eq!(row.request_digest, binding.request_digest);
            assert_eq!(row.execution_binding_json, serde_json::to_string(binding)?);
            assert_eq!(row.principal_ref.as_deref(), Some(&*binding.principal_ref));
            assert_eq!(
                row.project_digest.as_deref(),
                Some(&*binding.project_digest)
            );
            assert_eq!(row.graph_id.as_deref(), Some(&*binding.graph_id));
            assert_eq!(row.node_id.as_deref(), Some(&*binding.node_id));
            assert_eq!(row.attempt_id.as_deref(), Some(&*binding.attempt_id));
            assert_eq!(row.adopted_at, created_at);
            uuid::Uuid::parse_str(&row.id)?;
        }
        assert_eq!(rows[0].session_id, "bound-1");
        assert_eq!(rows[1].session_id, "bound-2");
        assert_event_adoption_foreign_key(&conn)?;
        drop(conn);

        initialize_store(&path)?;
        let reopened = open_initialized_store(&path)?;
        assert_eq!(raw_adoption_rows(&reopened)?, rows);
        assert_event_adoption_foreign_key(&reopened)?;
        assert_request_adoptions_session_index_is_non_unique(&reopened)?;
        Ok(())
    }

    #[test]
    fn request_adoptions_repeated_migration_repairs_only_missing_reservations() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let path = temp.path().join("pre-o3-repeated.sqlite3");
        let first = execution_binding_fixture();
        let second = execution_binding_with_attempt("attempt-2");
        let third = execution_binding_with_attempt("attempt-3");
        create_pre_o3_store(
            &path,
            &[
                ("bound-1", Some(first), "2026-08-01T00:00:00Z"),
                ("bound-2", Some(second.clone()), "2026-08-01T00:00:01Z"),
                ("bound-3", Some(third), "2026-08-01T00:00:02Z"),
            ],
        )?;
        initialize_store(&path)?;

        {
            let conn = open_initialized_store(&path)?;
            conn.execute_batch(
                "DROP TRIGGER request_adoptions_no_update;
                 DROP TRIGGER request_adoptions_no_delete;
                 DELETE FROM request_adoptions WHERE session_id IN ('bound-2', 'bound-3');",
            )?;
            conn.execute(
                "INSERT INTO request_adoptions (
                    id, adoption_key, contract, operation, request_digest, session_id,
                    execution_binding_json, principal_ref, project_digest, graph_id,
                    node_id, attempt_id, adopted_at
                 ) VALUES (
                    'keyed-bound-2', 'launch:keyed-2', ?1, 'launch', ?2, 'bound-2',
                    ?3, ?4, ?5, ?6, ?7, ?8, '2026-08-02T00:00:00Z'
                 )",
                params![
                    crate::request_adoption::CONTRACT,
                    &second.request_digest,
                    serde_json::to_string(&second)?,
                    &second.principal_ref,
                    &second.project_digest,
                    &second.graph_id,
                    &second.node_id,
                    &second.attempt_id,
                ],
            )?;
        }

        let before = {
            let conn = open_initialized_store(&path)?;
            let rows = raw_adoption_rows(&conn)?;
            assert_eq!(rows.len(), 2);
            assert_event_adoption_foreign_key(&conn)?;
            assert_request_adoptions_session_index_is_non_unique(&conn)?;
            rows
        };
        initialize_store(&path)?;
        let after = {
            let conn = open_initialized_store(&path)?;
            let rows = raw_adoption_rows(&conn)?;
            assert_eq!(rows.len(), 3);
            assert_event_adoption_foreign_key(&conn)?;
            assert_request_adoptions_session_index_is_non_unique(&conn)?;
            rows
        };
        for existing in &before {
            assert!(
                after.contains(existing),
                "initialization modified existing row {existing:?}: {after:?}"
            );
        }
        let repaired = after
            .iter()
            .find(|row| row.session_id == "bound-3")
            .expect("missing reservation must be repaired");
        assert_eq!(repaired.adoption_key, None);
        assert_eq!(repaired.adopted_at, "2026-08-01T00:00:02Z");
        let keyed = after
            .iter()
            .find(|row| row.session_id == "bound-2")
            .expect("keyed row must survive");
        assert_eq!(keyed.id, "keyed-bound-2");
        assert_eq!(keyed.adopted_at, "2026-08-02T00:00:00Z");

        initialize_store(&path)?;
        let reopened = open_initialized_store(&path)?;
        assert_eq!(raw_adoption_rows(&reopened)?, after);
        assert_event_adoption_foreign_key(&reopened)?;
        assert_request_adoptions_session_index_is_non_unique(&reopened)?;
        Ok(())
    }

    #[test]
    fn request_adoptions_session_lookup_uses_index_not_full_scan() -> Result<()> {
        // Regression test for the retention preflight / ON DELETE RESTRICT
        // foreign key check holding SQLite writer time proportional to the
        // size of the append-only `request_adoptions` ledger. Build a
        // realistically sized ledger of unrelated input adoptions (the
        // partial unique `request_adoptions_launch_session` index does not
        // cover `operation = 'input'` rows) plus a single target session and
        // assert the exact retention query plans through
        // `request_adoptions_session` rather than scanning the table.
        let temp = tempfile::tempdir()?;
        let conn = open_store(&temp.path().join("test.sqlite3"))?;
        let binding = execution_binding_fixture();

        const NOISE_ROWS: usize = 500;
        for i in 0..NOISE_ROWS {
            let session_id = format!("noise-session-{i}");
            let mut session = session_record(&session_id, "2026-08-01T00:00:00Z");
            session.execution_binding = Some(binding.clone());
            insert_session(&conn, &session)?;
            insert_input_adoption(
                &conn,
                &format!("noise-adoption-{i}"),
                &session_id,
                &request_adoption_fixture(&format!("noise-key-{i}"), &digest_fixture('c')),
                &binding,
                "2026-08-01T00:00:00Z",
            )?;
        }

        let target_session_id = "target-session";
        let mut target = session_record(target_session_id, "2026-08-01T00:00:00Z");
        target.execution_binding = Some(binding.clone());
        insert_session(&conn, &target)?;
        insert_input_adoption(
            &conn,
            "target-adoption",
            target_session_id,
            &request_adoption_fixture("target-key", &digest_fixture('c')),
            &binding,
            "2026-08-01T00:00:00Z",
        )?;

        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM request_adoptions", [], |row| row
                .get::<_, i64>(0),)?,
            (NOISE_ROWS + 1) as i64,
            "test fixture must build a realistically sized append-only ledger"
        );

        // The exact query used by `session_has_request_adoption` (retention
        // preflight, exercised on every session delete via the `sessions(id)`
        // ON DELETE RESTRICT foreign key check).
        let mut statement = conn.prepare(
            "EXPLAIN QUERY PLAN \
             SELECT EXISTS(SELECT 1 FROM request_adoptions WHERE session_id=?1)",
        )?;
        let plan: Vec<String> = statement
            .query_map([target_session_id], |row| row.get::<_, String>(3))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        assert!(!plan.is_empty(), "query plan must not be empty");
        let plan_text = plan.join(" | ").to_ascii_uppercase();

        assert!(
            plan_text.contains("REQUEST_ADOPTIONS_SESSION"),
            "retention lookup must use the request_adoptions_session index: {plan_text}"
        );
        assert!(
            !plan_text.contains("SCAN REQUEST_ADOPTIONS")
                && !plan_text.contains("SCAN TABLE REQUEST_ADOPTIONS"),
            "retention lookup must not fall back to a full table scan: {plan_text}"
        );

        // The production helper must observe the same result the query plan
        // was checked against, for both a bound and an unbound session.
        assert!(session_has_request_adoption(&conn, target_session_id)?);
        assert!(!session_has_request_adoption(
            &conn,
            "never-adopted-session"
        )?);

        Ok(())
    }

    #[test]
    fn request_adoptions_duplicate_historical_attempt_scope_fails_startup() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let path = temp.path().join("duplicate-pre-o3.sqlite3");
        let binding = execution_binding_fixture();
        let request_digest = binding.request_digest.clone();
        create_pre_o3_store(
            &path,
            &[
                ("bound-1", Some(binding.clone()), "2026-08-01T00:00:00Z"),
                ("bound-2", Some(binding), "2026-08-01T00:00:01Z"),
            ],
        )?;

        let error = initialize_store(&path).expect_err("duplicate scope must fail startup");
        let rendered = format!("{error:#}");
        // The `request_adoptions_no_replace` trigger checks retained identity
        // conflicts before SQLite's unique-index constraint check runs, so it
        // is what now raises for this duplicate attempt scope.
        assert!(
            rendered.contains("request adoptions are retained"),
            "unexpected error: {rendered}"
        );
        // O3 forbids leaking ledger session IDs, digests, or bindings into
        // diagnostics: the migration failure context must stay static.
        assert!(
            rendered.contains("failed to migrate historical request adoption"),
            "unexpected error: {rendered}"
        );
        for sensitive in ["bound-1", "bound-2", request_digest.as_str()] {
            assert!(
                !rendered.contains(sensitive),
                "error must not leak {sensitive}: {rendered}"
            );
        }
        {
            let conn = Connection::open(&path)?;
            assert!(!sqlite_object_exists(&conn, "table", "request_adoptions")?);
            assert!(!table_columns(&conn, "events")?
                .iter()
                .any(|column| column == "request_adoption_id"));
            assert_eq!(
                event_foreign_keys(&conn)?,
                vec![(
                    "sessions".to_string(),
                    "session_id".to_string(),
                    "id".to_string(),
                    "CASCADE".to_string()
                )]
            );
        }

        let repaired = execution_binding_with_attempt("attempt-repaired");
        {
            let conn = Connection::open(&path)?;
            conn.execute(
                "UPDATE sessions SET execution_binding_json = ?1 WHERE id = 'bound-2'",
                [serde_json::to_string(&repaired)?],
            )?;
        }
        initialize_store(&path)?;
        let conn = open_initialized_store(&path)?;
        assert_eq!(raw_adoption_rows(&conn)?.len(), 2);
        assert_event_adoption_foreign_key(&conn)?;
        Ok(())
    }

    #[test]
    fn request_adoptions_migration_rolls_back_on_corrupt_binding() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let path = temp.path().join("corrupt-pre-o3.sqlite3");
        create_pre_o3_store(
            &path,
            &[(
                "bound-corrupt",
                Some(execution_binding_fixture()),
                "2026-08-01T00:00:00Z",
            )],
        )?;
        {
            let conn = Connection::open(&path)?;
            conn.execute(
                "UPDATE sessions SET execution_binding_json = '{not-json'
                 WHERE id = 'bound-corrupt'",
                [],
            )?;
        }

        let error = initialize_store(&path).expect_err("corrupt binding must fail startup");
        assert!(
            format!("{error:#}").contains("execution binding"),
            "unexpected error: {error:#}"
        );
        {
            let conn = Connection::open(&path)?;
            assert!(!sqlite_object_exists(&conn, "table", "request_adoptions")?);
            assert!(!table_columns(&conn, "events")?
                .iter()
                .any(|column| column == "request_adoption_id"));
            assert_eq!(event_foreign_keys(&conn)?.len(), 1);
        }

        {
            let conn = Connection::open(&path)?;
            conn.execute(
                "UPDATE sessions SET execution_binding_json = ?1
                 WHERE id = 'bound-corrupt'",
                [serde_json::to_string(&execution_binding_fixture())?],
            )?;
        }
        initialize_store(&path)?;
        let conn = open_initialized_store(&path)?;
        assert_eq!(raw_adoption_rows(&conn)?.len(), 1);
        assert_event_adoption_foreign_key(&conn)?;
        Ok(())
    }

    #[test]
    fn request_adoptions_strict_readback_rejects_corrupt_rows() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let path = temp.path().join("strict.sqlite3");
        let conn = open_store(&path)?;
        let binding = execution_binding_fixture();
        for (index, key) in [
            "corrupt-contract",
            "corrupt-operation",
            "corrupt-digest",
            "corrupt-binding-json",
            "corrupt-binding-bytes",
            "corrupt-nullability",
            "corrupt-input-scope",
            "corrupt-session-binding",
        ]
        .into_iter()
        .enumerate()
        {
            let session_id = format!("strict-session-{index}");
            let mut session = session_record(&session_id, "2026-08-15T00:00:00Z");
            session.execution_binding = Some(binding.clone());
            insert_session(&conn, &session)?;
            let request = request_adoption_fixture(key, &digest_fixture('d'));
            insert_input_adoption(
                &conn,
                &format!("strict-adoption-{index}"),
                &session_id,
                &request,
                &binding,
                "2026-08-15T00:00:00Z",
            )?;
        }
        conn.execute_batch(
            "DROP TRIGGER request_adoptions_no_update;
             DROP TRIGGER request_adoptions_no_delete;
             PRAGMA ignore_check_constraints = ON;",
        )?;
        conn.execute(
            "UPDATE request_adoptions SET contract = 'psyche.request_adoption.v0'
             WHERE adoption_key = 'corrupt-contract'",
            [],
        )?;
        conn.execute(
            "UPDATE request_adoptions SET operation = 'deliver'
             WHERE adoption_key = 'corrupt-operation'",
            [],
        )?;
        conn.execute(
            "UPDATE request_adoptions SET request_digest = ?1
             WHERE adoption_key = 'corrupt-digest'",
            [format!("sha256:{}", "A".repeat(64))],
        )?;
        conn.execute(
            "UPDATE request_adoptions SET execution_binding_json = '{not-json'
             WHERE adoption_key = 'corrupt-binding-json'",
            [],
        )?;
        conn.execute(
            "UPDATE request_adoptions SET execution_binding_json = execution_binding_json || ' '
             WHERE adoption_key = 'corrupt-binding-bytes'",
            [],
        )?;
        conn.execute(
            "UPDATE request_adoptions SET contract = NULL
             WHERE adoption_key = 'corrupt-nullability'",
            [],
        )?;
        conn.execute(
            "UPDATE request_adoptions SET principal_ref = 'principal:operator'
             WHERE adoption_key = 'corrupt-input-scope'",
            [],
        )?;
        conn.execute(
            "UPDATE sessions SET execution_binding_json = ?1
             WHERE id = 'strict-session-7'",
            [serde_json::to_string(&execution_binding_with_attempt(
                "other-attempt",
            ))?],
        )?;
        conn.execute_batch("PRAGMA ignore_check_constraints = OFF;")?;

        for (index, key) in [
            "corrupt-contract",
            "corrupt-operation",
            "corrupt-digest",
            "corrupt-binding-json",
            "corrupt-binding-bytes",
            "corrupt-nullability",
            "corrupt-input-scope",
            "corrupt-session-binding",
        ]
        .into_iter()
        .enumerate()
        {
            let request = request_adoption_fixture(key, &digest_fixture('d'));
            let error = resolve_input_adoption(
                &conn,
                &format!("strict-session-{index}"),
                &request,
                &binding,
            )
            .expect_err("corrupt adoption must be an internal store error");
            assert!(
                format!("{error:#}").contains("invalid stored request adoption"),
                "unexpected error for {key}: {error:#}"
            );
        }
        Ok(())
    }

    #[test]
    fn request_adoptions_raw_update_and_delete_are_rejected() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let conn = open_store(&temp.path().join("immutable.sqlite3"))?;
        let binding = execution_binding_fixture();
        let mut session = session_record("immutable-session", "2026-08-15T00:00:00Z");
        session.execution_binding = Some(binding.clone());
        insert_session(&conn, &session)?;
        let request = request_adoption_fixture("immutable-key", &binding.request_digest);
        insert_launch_adoption(
            &conn,
            "immutable-adoption",
            "immutable-session",
            &request,
            &binding,
            "2026-08-15T00:00:00Z",
        )?;
        let before = raw_adoption_rows(&conn)?;

        assert_sql_error_contains(
            conn.execute(
                "UPDATE request_adoptions SET adopted_at = '2099-01-01T00:00:00Z'
                 WHERE id = 'immutable-adoption'",
                [],
            ),
            "request adoptions are immutable",
        );
        assert_sql_error_contains(
            conn.execute(
                "DELETE FROM request_adoptions WHERE id = 'immutable-adoption'",
                [],
            ),
            "request adoptions are retained",
        );
        assert_eq!(raw_adoption_rows(&conn)?, before);
        Ok(())
    }

    #[test]
    fn request_adoptions_raw_replace_cannot_bypass_retention() -> Result<()> {
        // `INSERT OR REPLACE` / `REPLACE INTO` resolve a conflicting unique
        // index by deleting the old row first; when `recursive_triggers` is
        // off that implicit delete does not fire `request_adoptions_no_delete`.
        // The `request_adoptions_no_replace` BEFORE INSERT trigger must abort
        // before any such delete can happen, for every retained identity:
        // existing id, existing adoption_key, existing launch attempt scope,
        // and existing launch session_id.
        let temp = tempfile::tempdir()?;
        let conn = open_store(&temp.path().join("replace-immutable.sqlite3"))?;
        let binding = execution_binding_fixture();
        let mut session = session_record("replace-session", "2026-08-15T00:00:00Z");
        session.execution_binding = Some(binding.clone());
        insert_session(&conn, &session)?;
        let other_binding = execution_binding_with_attempt("other-attempt");
        let mut other_session = session_record("replace-session-2", "2026-08-15T00:00:00Z");
        other_session.execution_binding = Some(other_binding.clone());
        insert_session(&conn, &other_session)?;

        let request = request_adoption_fixture("replace-key", &binding.request_digest);
        insert_launch_adoption(
            &conn,
            "replace-adoption",
            "replace-session",
            &request,
            &binding,
            "2026-08-15T00:00:00Z",
        )?;
        let before = raw_adoption_rows(&conn)?;
        assert_eq!(before.len(), 1);

        let other_binding_json = serde_json::to_string(&other_binding)?;
        let binding_json = serde_json::to_string(&binding)?;

        // Same primary-key `INSERT OR REPLACE`: must not delete-then-reinsert.
        assert_sql_error_contains(
            conn.execute(
                "INSERT OR REPLACE INTO request_adoptions (
                    id, adoption_key, contract, operation, request_digest, session_id,
                    execution_binding_json, principal_ref, project_digest, graph_id,
                    node_id, attempt_id, adopted_at
                 ) VALUES (
                    'replace-adoption', 'replace-key', ?1, 'launch', ?2, 'replace-session', ?3,
                    ?4, ?5, ?6, ?7, ?8, '2099-01-01T00:00:00Z'
                 )",
                params![
                    crate::request_adoption::CONTRACT,
                    digest_fixture('f'),
                    other_binding_json,
                    other_binding.principal_ref,
                    other_binding.project_digest,
                    other_binding.graph_id,
                    other_binding.node_id,
                    other_binding.attempt_id,
                ],
            ),
            "request adoptions are retained",
        );
        assert_eq!(raw_adoption_rows(&conn)?, before);

        // `REPLACE INTO` is sugar for `INSERT OR REPLACE`; same primary key.
        assert_sql_error_contains(
            conn.execute(
                "REPLACE INTO request_adoptions (
                    id, adoption_key, contract, operation, request_digest, session_id,
                    execution_binding_json, principal_ref, project_digest, graph_id,
                    node_id, attempt_id, adopted_at
                 ) VALUES (
                    'replace-adoption', 'replace-key', ?1, 'launch', ?2, 'replace-session', ?3,
                    ?4, ?5, ?6, ?7, ?8, '2099-01-01T00:00:00Z'
                 )",
                params![
                    crate::request_adoption::CONTRACT,
                    digest_fixture('f'),
                    other_binding_json,
                    other_binding.principal_ref,
                    other_binding.project_digest,
                    other_binding.graph_id,
                    other_binding.node_id,
                    other_binding.attempt_id,
                ],
            ),
            "request adoptions are retained",
        );
        assert_eq!(raw_adoption_rows(&conn)?, before);

        // Different id, but the `adoption_key` collides with the retained
        // row's key: must not silently replace it.
        assert_sql_error_contains(
            conn.execute(
                "INSERT OR REPLACE INTO request_adoptions (
                    id, adoption_key, contract, operation, request_digest, session_id,
                    execution_binding_json, principal_ref, project_digest, graph_id,
                    node_id, attempt_id, adopted_at
                 ) VALUES (
                    'replace-adoption-by-key', 'replace-key', ?1, 'launch', ?2, 'replace-session-2', ?3,
                    ?4, ?5, ?6, ?7, ?8, '2099-01-01T00:00:00Z'
                 )",
                params![
                    crate::request_adoption::CONTRACT,
                    digest_fixture('f'),
                    other_binding_json,
                    other_binding.principal_ref,
                    other_binding.project_digest,
                    other_binding.graph_id,
                    other_binding.node_id,
                    other_binding.attempt_id,
                ],
            ),
            "request adoptions are retained",
        );
        assert_eq!(raw_adoption_rows(&conn)?, before);

        // Different id, no adoption_key collision, but the same launch
        // five-field attempt scope: must not silently replace it.
        assert_sql_error_contains(
            conn.execute(
                "REPLACE INTO request_adoptions (
                    id, adoption_key, contract, operation, request_digest, session_id,
                    execution_binding_json, principal_ref, project_digest, graph_id,
                    node_id, attempt_id, adopted_at
                 ) VALUES (
                    'replace-adoption-by-scope', NULL, NULL, 'launch', ?1, 'replace-session-2', ?2,
                    ?3, ?4, ?5, ?6, ?7, '2099-01-01T00:00:00Z'
                 )",
                params![
                    digest_fixture('f'),
                    binding_json,
                    binding.principal_ref,
                    binding.project_digest,
                    binding.graph_id,
                    binding.node_id,
                    binding.attempt_id,
                ],
            ),
            "request adoptions are retained",
        );
        assert_eq!(raw_adoption_rows(&conn)?, before);

        // Different id, no key or scope collision, but the same launch
        // session_id: must not silently replace it.
        assert_sql_error_contains(
            conn.execute(
                "REPLACE INTO request_adoptions (
                    id, adoption_key, contract, operation, request_digest, session_id,
                    execution_binding_json, principal_ref, project_digest, graph_id,
                    node_id, attempt_id, adopted_at
                 ) VALUES (
                    'replace-adoption-by-session', NULL, NULL, 'launch', ?1, 'replace-session', ?2,
                    ?3, ?4, ?5, ?6, ?7, '2099-01-01T00:00:00Z'
                 )",
                params![
                    digest_fixture('f'),
                    other_binding_json,
                    other_binding.principal_ref,
                    other_binding.project_digest,
                    other_binding.graph_id,
                    "different-node",
                    "different-attempt",
                ],
            ),
            "request adoptions are retained",
        );
        assert_eq!(raw_adoption_rows(&conn)?, before);

        // Hidden-rowid bypass: `request_adoptions` is a rowid table, so a raw
        // REPLACE can target the retained row's hidden `rowid` directly while
        // supplying entirely fresh logical identities (fresh id, no
        // adoption_key, a fresh launch scope, and a fresh session). None of
        // the `request_adoptions_no_replace` logical WHEN clauses match, so
        // only `recursive_triggers = ON` firing `request_adoptions_no_delete`
        // on the implicit conflict-resolution delete stops the retained row
        // from being silently deleted and replaced.
        let original_rowid: i64 = conn.query_row(
            "SELECT rowid FROM request_adoptions WHERE id = 'replace-adoption'",
            [],
            |row| row.get(0),
        )?;
        let rowid_bypass_binding = execution_binding_with_attempt("rowid-bypass-attempt");
        let mut rowid_bypass_session =
            session_record("replace-session-rowid-bypass", "2026-08-15T00:00:00Z");
        rowid_bypass_session.execution_binding = Some(rowid_bypass_binding.clone());
        insert_session(&conn, &rowid_bypass_session)?;
        let rowid_bypass_binding_json = serde_json::to_string(&rowid_bypass_binding)?;

        assert_sql_error_contains(
            conn.execute(
                "INSERT OR REPLACE INTO request_adoptions (
                    rowid, id, adoption_key, contract, operation, request_digest, session_id,
                    execution_binding_json, principal_ref, project_digest, graph_id,
                    node_id, attempt_id, adopted_at
                 ) VALUES (
                    ?1, 'rowid-bypass-insert', NULL, NULL, 'launch', ?2,
                    'replace-session-rowid-bypass', ?3, ?4, ?5, ?6, ?7, ?8, '2099-01-01T00:00:00Z'
                 )",
                params![
                    original_rowid,
                    digest_fixture('g'),
                    rowid_bypass_binding_json,
                    rowid_bypass_binding.principal_ref,
                    rowid_bypass_binding.project_digest,
                    rowid_bypass_binding.graph_id,
                    rowid_bypass_binding.node_id,
                    rowid_bypass_binding.attempt_id,
                ],
            ),
            "request adoptions are retained",
        );
        assert_eq!(raw_adoption_rows(&conn)?, before);

        assert_sql_error_contains(
            conn.execute(
                "REPLACE INTO request_adoptions (
                    rowid, id, adoption_key, contract, operation, request_digest, session_id,
                    execution_binding_json, principal_ref, project_digest, graph_id,
                    node_id, attempt_id, adopted_at
                 ) VALUES (
                    ?1, 'rowid-bypass-replace', NULL, NULL, 'launch', ?2,
                    'replace-session-rowid-bypass', ?3, ?4, ?5, ?6, ?7, ?8, '2099-01-01T00:00:00Z'
                 )",
                params![
                    original_rowid,
                    digest_fixture('g'),
                    rowid_bypass_binding_json,
                    rowid_bypass_binding.principal_ref,
                    rowid_bypass_binding.project_digest,
                    rowid_bypass_binding.graph_id,
                    rowid_bypass_binding.node_id,
                    rowid_bypass_binding.attempt_id,
                ],
            ),
            "request adoptions are retained",
        );
        assert_eq!(raw_adoption_rows(&conn)?, before);
        Ok(())
    }

    #[test]
    fn request_adoption_event_correlation_rejects_invalid_insert_and_rebind() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let conn = open_store(&temp.path().join("event-correlation.sqlite3"))?;
        assert_eq!(
            conn.query_row("PRAGMA foreign_keys", [], |row| row.get::<_, i64>(0))?,
            1
        );
        assert_event_adoption_foreign_key(&conn)?;
        let first_binding = execution_binding_fixture();
        let second_binding = execution_binding_with_attempt("attempt-2");
        for (id, binding) in [
            ("event-session-1", &first_binding),
            ("event-session-2", &second_binding),
        ] {
            let mut session = session_record(id, "2026-08-15T00:00:00Z");
            session.execution_binding = Some(binding.clone());
            insert_session(&conn, &session)?;
        }
        insert_input_adoption(
            &conn,
            "input-adoption-1",
            "event-session-1",
            &request_adoption_fixture("input-key-1", &digest_fixture('d')),
            &first_binding,
            "2026-08-15T00:00:00Z",
        )?;
        insert_input_adoption(
            &conn,
            "input-adoption-2",
            "event-session-1",
            &request_adoption_fixture("input-key-2", &digest_fixture('e')),
            &first_binding,
            "2026-08-15T00:00:00Z",
        )?;
        insert_launch_adoption(
            &conn,
            "launch-adoption",
            "event-session-2",
            &request_adoption_fixture("launch-key", &second_binding.request_digest),
            &second_binding,
            "2026-08-15T00:00:00Z",
        )?;

        conn.execute(
            "INSERT INTO events (
                id, session_id, kind, payload_json, created_at, request_adoption_id
             ) VALUES (
                'event-valid', 'event-session-1', 'input', '{}',
                '2026-08-15T00:00:00Z', 'input-adoption-1'
             )",
            [],
        )?;
        conn.execute(
            "INSERT INTO events (id, session_id, kind, payload_json, created_at)
             VALUES (
                'event-null', 'event-session-1', 'input', '{}',
                '2026-08-15T00:00:00Z'
             )",
            [],
        )?;

        for (id, session_id, kind, adoption_id) in [
            (
                "event-wrong-session",
                "event-session-2",
                "input",
                "input-adoption-2",
            ),
            (
                "event-launch-adoption",
                "event-session-2",
                "input",
                "launch-adoption",
            ),
            (
                "event-wrong-kind",
                "event-session-1",
                "output",
                "input-adoption-2",
            ),
        ] {
            assert_sql_error_contains(
                conn.execute(
                    "INSERT INTO events (
                        id, session_id, kind, payload_json, created_at, request_adoption_id
                     ) VALUES (?1, ?2, ?3, '{}', '2026-08-15T00:00:00Z', ?4)",
                    params![id, session_id, kind, adoption_id],
                ),
                "invalid request adoption event correlation",
            );
        }
        assert!(
            conn.execute(
                "INSERT INTO events (
                    id, session_id, kind, payload_json, created_at, request_adoption_id
                 ) VALUES (
                    'event-duplicate', 'event-session-1', 'input', '{}',
                    '2026-08-15T00:00:00Z', 'input-adoption-1'
                 )",
                [],
            )
            .is_err(),
            "one adoption must correlate with at most one event"
        );

        for sql in [
            "UPDATE events SET request_adoption_id = 'input-adoption-2'
             WHERE id = 'event-valid'",
            "UPDATE events SET request_adoption_id = NULL WHERE id = 'event-valid'",
            "UPDATE events SET session_id = 'event-session-2' WHERE id = 'event-valid'",
            "UPDATE events SET kind = 'output' WHERE id = 'event-valid'",
            "UPDATE events SET request_adoption_id = 'input-adoption-2'
             WHERE id = 'event-null'",
        ] {
            assert_sql_error_contains(
                conn.execute(sql, []),
                "request adoption event correlation is immutable",
            );
        }
        assert_eq!(
            conn.execute(
                "UPDATE events SET request_adoption_id = request_adoption_id,
                    session_id = session_id, kind = kind
                 WHERE id = 'event-valid'",
                [],
            )?,
            1
        );
        Ok(())
    }

    #[test]
    fn request_adoptions_survive_status_archive_summon_and_event_retention() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let path = temp.path().join("retention.sqlite3");
        let conn = open_store(&path)?;
        let binding = execution_binding_fixture();
        let mut session = session_record("retained-session", "2026-08-01T00:00:00Z");
        session.execution_binding = Some(binding.clone());
        insert_session(&conn, &session)?;
        insert_launch_adoption(
            &conn,
            "retained-launch",
            "retained-session",
            &request_adoption_fixture("retained-launch-key", &binding.request_digest),
            &binding,
            "2026-08-01T00:00:00Z",
        )?;
        insert_input_adoption(
            &conn,
            "retained-input",
            "retained-session",
            &request_adoption_fixture("retained-input-key", &digest_fixture('d')),
            &binding,
            "2026-08-01T00:01:00Z",
        )?;
        conn.execute(
            "INSERT INTO events (
                id, session_id, kind, payload_json, created_at, request_adoption_id
             ) VALUES (
                'retained-event', 'retained-session', 'input', '{}',
                '2026-08-01T00:01:00Z', 'retained-input'
             )",
            [],
        )?;
        let before = raw_adoption_rows(&conn)?;

        update_session_status(
            &conn,
            "retained-session",
            "completed",
            Some(0),
            "2026-08-01T00:02:00Z",
        )?;
        archive_session(&conn, "retained-session", "2026-08-01T00:03:00Z")?;
        summon_session(&conn, "retained-session", "2026-08-01T00:04:00Z")?;
        assert_eq!(
            prune_events_older_than_bounded(&conn, "2026-08-02T00:00:00Z", 10)?,
            1
        );
        assert_eq!(raw_adoption_rows(&conn)?, before);
        drop(conn);

        initialize_store(&path)?;
        let reopened = open_initialized_store(&path)?;
        assert_eq!(raw_adoption_rows(&reopened)?, before);
        assert_event_adoption_foreign_key(&reopened)?;
        Ok(())
    }

    fn assert_adoption_conflict(resolution: AdoptionResolution, expected_field: &'static str) {
        match resolution {
            AdoptionResolution::Conflict { field } => assert_eq!(field, expected_field),
            other => panic!("expected conflict at {expected_field}, got {other:?}"),
        }
    }

    fn assert_adoption_replay(
        resolution: AdoptionResolution,
        expected_adoption_id: &str,
        expected_session: &SessionRecord,
    ) {
        match resolution {
            AdoptionResolution::Replay {
                adoption_id,
                session,
            } => {
                assert_eq!(adoption_id, expected_adoption_id);
                assert_eq!(&session, expected_session);
            }
            other => panic!("expected replay, got {other:?}"),
        }
    }

    #[test]
    fn request_adoptions_exact_launch_and_input_replay() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let conn = open_store(&temp.path().join("exact-replay.sqlite3"))?;
        let launch_binding = execution_binding_fixture();
        let input_binding = execution_binding_with_attempt("input-session-attempt");
        let mut launch_session = session_record("launch-session", "2026-08-15T00:00:00Z");
        launch_session.execution_binding = Some(launch_binding.clone());
        let mut input_session = session_record("input-session", "2026-08-15T00:00:01Z");
        input_session.execution_binding = Some(input_binding.clone());
        insert_session(&conn, &launch_session)?;
        insert_session(&conn, &input_session)?;

        let launch_request =
            request_adoption_fixture("exact-launch", &launch_binding.request_digest);
        let input_request = request_adoption_fixture("exact-input", &digest_fixture('d'));
        assert!(matches!(
            resolve_launch_adoption(&conn, &launch_request, &launch_binding)?,
            AdoptionResolution::Absent
        ));
        assert!(matches!(
            resolve_input_adoption(&conn, "input-session", &input_request, &input_binding)?,
            AdoptionResolution::Absent
        ));
        insert_launch_adoption(
            &conn,
            "launch-adoption",
            "launch-session",
            &launch_request,
            &launch_binding,
            "2026-08-15T00:01:00Z",
        )?;
        insert_input_adoption(
            &conn,
            "input-adoption",
            "input-session",
            &input_request,
            &input_binding,
            "2026-08-15T00:02:00Z",
        )?;

        assert_adoption_replay(
            resolve_launch_adoption(&conn, &launch_request, &launch_binding)?,
            "launch-adoption",
            &launch_session,
        );
        assert_adoption_replay(
            resolve_input_adoption(&conn, "input-session", &input_request, &input_binding)?,
            "input-adoption",
            &input_session,
        );
        let launch_record =
            load_request_adoption_by_id(&conn, "launch-adoption")?.expect("launch record");
        assert_eq!(launch_record.operation, RequestAdoptionOperation::Launch);
        assert_eq!(launch_record.adoption_key.as_deref(), Some("exact-launch"));
        let input_record =
            load_request_adoption_by_id(&conn, "input-adoption")?.expect("input record");
        assert_eq!(input_record.operation, RequestAdoptionOperation::Input);
        assert_eq!(input_record.adoption_key.as_deref(), Some("exact-input"));
        Ok(())
    }

    // O3 forbids leaking ledger session ids (or any other retained identity)
    // into errors and diagnostics. If a stored session row is malformed
    // (here: unparsable `labels`), `get_session`'s failure formats the
    // session id straight into its message. Exact replay must redact that
    // failure at the shared `replay_resolution` boundary rather than
    // propagate it, for both launch and input adoption resolution.
    #[test]
    fn request_adoptions_replay_redacts_malformed_session_readback_error() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let conn = open_store(&temp.path().join("replay-redaction.sqlite3"))?;
        let launch_binding = execution_binding_fixture();
        let input_binding = execution_binding_with_attempt("input-session-attempt");
        let mut launch_session = session_record("launch-session", "2026-08-15T00:00:00Z");
        launch_session.execution_binding = Some(launch_binding.clone());
        let mut input_session = session_record("input-session", "2026-08-15T00:00:01Z");
        input_session.execution_binding = Some(input_binding.clone());
        insert_session(&conn, &launch_session)?;
        insert_session(&conn, &input_session)?;

        let launch_request =
            request_adoption_fixture("exact-launch", &launch_binding.request_digest);
        let input_request = request_adoption_fixture("exact-input", &digest_fixture('d'));
        insert_launch_adoption(
            &conn,
            "launch-adoption",
            "launch-session",
            &launch_request,
            &launch_binding,
            "2026-08-15T00:01:00Z",
        )?;
        insert_input_adoption(
            &conn,
            "input-adoption",
            "input-session",
            &input_request,
            &input_binding,
            "2026-08-15T00:02:00Z",
        )?;

        // Corrupt retained session data directly, bypassing the store API,
        // the way a reviewer-tampered or otherwise malformed row would.
        conn.execute(
            "UPDATE sessions SET labels = '{not valid json' WHERE id = ?1",
            params!["launch-session"],
        )?;
        conn.execute(
            "UPDATE sessions SET labels = '{not valid json' WHERE id = ?1",
            params!["input-session"],
        )?;

        let launch_error = resolve_launch_adoption(&conn, &launch_request, &launch_binding)
            .expect_err("malformed session data must surface as an internal failure");
        let input_error =
            resolve_input_adoption(&conn, "input-session", &input_request, &input_binding)
                .expect_err("malformed session data must surface as an internal failure");

        let sensitive = [
            "launch-session",
            "input-session",
            "launch-adoption",
            "input-adoption",
            "exact-launch",
            "exact-input",
            launch_binding.request_digest.as_str(),
            input_binding.request_digest.as_str(),
            launch_binding.attempt_id.as_str(),
            input_binding.attempt_id.as_str(),
        ];
        for error in [&launch_error, &input_error] {
            let rendered = format!("{error:#}");
            let debug_rendered = format!("{error:?}");
            for value in sensitive {
                assert!(
                    !rendered.contains(value),
                    "error display must not leak {value}: {rendered}"
                );
                assert!(
                    !debug_rendered.contains(value),
                    "error debug chain must not leak {value}: {debug_rendered}"
                );
            }
        }
        Ok(())
    }

    #[test]
    fn request_adoptions_each_same_key_identity_difference_conflicts() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let conn = open_store(&temp.path().join("identity-conflicts.sqlite3"))?;
        let mut binding = execution_binding_fixture();
        binding.parent = Some(crate::execution_binding::ExecutionBindingParent {
            session_id: "parent-session".to_string(),
            graph_id: "parent-graph".to_string(),
            node_id: "parent-node".to_string(),
            attempt_id: "parent-attempt".to_string(),
        });
        binding.delegation_digest = Some(digest_fixture('d'));
        let mut session = session_record("identity-session", "2026-08-15T00:00:00Z");
        session.execution_binding = Some(binding.clone());
        insert_session(&conn, &session)?;
        let request = request_adoption_fixture("identity-key", &binding.request_digest);
        insert_launch_adoption(
            &conn,
            "identity-adoption",
            "identity-session",
            &request,
            &binding,
            "2026-08-15T00:00:00Z",
        )?;

        // O3 contract §6: every hidden identity member (contract, operation,
        // digest, input session, binding) must collapse to the same
        // `requestAdoption.key` conflict when the submitted key matches an
        // existing row that is not an exact replay - the specific field that
        // differs must never leak.
        let mut changed_contract = request.clone();
        changed_contract.contract = "psyche.request_adoption.v2".to_string();
        assert_adoption_conflict(
            resolve_launch_adoption(&conn, &changed_contract, &binding)?,
            "requestAdoption.key",
        );
        let mut changed_digest = request.clone();
        changed_digest.request_digest = digest_fixture('e');
        assert_adoption_conflict(
            resolve_launch_adoption(&conn, &changed_digest, &binding)?,
            "requestAdoption.key",
        );
        assert_adoption_conflict(
            resolve_input_adoption(&conn, "identity-session", &request, &binding)?,
            "requestAdoption.key",
        );

        let mut other_session = session_record("identity-session-2", "2026-08-15T00:00:00Z");
        other_session.execution_binding = Some(binding.clone());
        insert_session(&conn, &other_session)?;
        let input_request = request_adoption_fixture("identity-input-key", &binding.request_digest);
        insert_input_adoption(
            &conn,
            "identity-input-adoption",
            "identity-session",
            &input_request,
            &binding,
            "2026-08-15T00:00:01Z",
        )?;
        assert_adoption_conflict(
            resolve_input_adoption(&conn, "identity-session-2", &input_request, &binding)?,
            "requestAdoption.key",
        );

        type BindingMutation = Box<dyn Fn(&mut crate::execution_binding::ExecutionBinding)>;
        let binding_cases: Vec<(&'static str, BindingMutation)> = vec![
            (
                "executionBinding.contract",
                Box::new(|value| value.contract = "psyche.execution_binding.v2".to_string()),
            ),
            (
                "executionBinding.principalRef",
                Box::new(|value| value.principal_ref = "principal:other".to_string()),
            ),
            (
                "executionBinding.familiarId",
                Box::new(|value| value.familiar_id = "other-familiar".to_string()),
            ),
            (
                "executionBinding.familiarSnapshotDigest",
                Box::new(|value| value.familiar_snapshot_digest = digest_fixture('e')),
            ),
            (
                "executionBinding.projectDigest",
                Box::new(|value| value.project_digest = digest_fixture('e')),
            ),
            (
                "executionBinding.graphId",
                Box::new(|value| value.graph_id = "other-graph".to_string()),
            ),
            (
                "executionBinding.nodeId",
                Box::new(|value| value.node_id = "other-node".to_string()),
            ),
            (
                "executionBinding.attemptId",
                Box::new(|value| value.attempt_id = "other-attempt".to_string()),
            ),
            (
                "executionBinding.requestDigest",
                Box::new(|value| value.request_digest = digest_fixture('e')),
            ),
            (
                "executionBinding.policyRevision",
                Box::new(|value| value.policy_revision = "policy:other".to_string()),
            ),
            (
                "executionBinding.expiresAt",
                Box::new(|value| value.expires_at = "2098-01-01T00:00:00Z".to_string()),
            ),
            (
                "parent.sessionId",
                Box::new(|value| {
                    value.parent.as_mut().expect("parent").session_id = "other-parent".to_string();
                }),
            ),
            (
                "parent.graphId",
                Box::new(|value| {
                    value.parent.as_mut().expect("parent").graph_id =
                        "other-parent-graph".to_string();
                }),
            ),
            (
                "parent.nodeId",
                Box::new(|value| {
                    value.parent.as_mut().expect("parent").node_id =
                        "other-parent-node".to_string();
                }),
            ),
            (
                "parent.attemptId",
                Box::new(|value| {
                    value.parent.as_mut().expect("parent").attempt_id =
                        "other-parent-attempt".to_string();
                }),
            ),
            ("parent", Box::new(|value| value.parent = None)),
            (
                "executionBinding.delegationDigest",
                Box::new(|value| value.delegation_digest = Some(digest_fixture('e'))),
            ),
        ];
        for (mutated_field, mutate) in binding_cases {
            let mut changed = binding.clone();
            mutate(&mut changed);
            let resolution = resolve_launch_adoption(&conn, &request, &changed)?;
            match resolution {
                AdoptionResolution::Conflict { field } => assert_eq!(
                    field, "requestAdoption.key",
                    "binding mismatch at {mutated_field} must not leak its field path"
                ),
                other => panic!(
                    "expected conflict at requestAdoption.key for {mutated_field}, got {other:?}"
                ),
            }
        }
        Ok(())
    }

    #[test]
    fn request_adoptions_global_key_and_attempt_scope_conflicts() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let conn = open_store(&temp.path().join("global-conflicts.sqlite3"))?;
        let binding = execution_binding_fixture();
        let second_binding = execution_binding_with_attempt("second-attempt");
        for (id, value) in [
            ("global-session-1", &binding),
            ("global-session-2", &second_binding),
        ] {
            let mut session = session_record(id, "2026-08-15T00:00:00Z");
            session.execution_binding = Some(value.clone());
            insert_session(&conn, &session)?;
        }
        let launch_request = request_adoption_fixture("global-key", &binding.request_digest);
        insert_launch_adoption(
            &conn,
            "global-launch",
            "global-session-1",
            &launch_request,
            &binding,
            "2026-08-15T00:00:00Z",
        )?;

        assert_adoption_conflict(
            resolve_input_adoption(
                &conn,
                "global-session-2",
                &request_adoption_fixture("global-key", &digest_fixture('d')),
                &second_binding,
            )?,
            "requestAdoption.key",
        );
        assert_adoption_conflict(
            resolve_launch_adoption(
                &conn,
                &request_adoption_fixture("different-key", &binding.request_digest),
                &binding,
            )?,
            "executionBinding.attemptId",
        );

        let input_request = request_adoption_fixture("input-global-key", &digest_fixture('e'));
        insert_input_adoption(
            &conn,
            "global-input",
            "global-session-2",
            &input_request,
            &second_binding,
            "2026-08-15T00:00:00Z",
        )?;
        assert_adoption_conflict(
            resolve_input_adoption(&conn, "global-session-1", &input_request, &binding)?,
            "requestAdoption.key",
        );
        assert_adoption_conflict(
            resolve_launch_adoption(&conn, &input_request, &second_binding)?,
            "requestAdoption.key",
        );
        Ok(())
    }

    #[test]
    fn request_adoptions_different_attempt_id_succeeds() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let conn = open_store(&temp.path().join("different-attempt.sqlite3"))?;
        let first = execution_binding_fixture();
        let second = execution_binding_with_attempt("attempt-2");
        for (id, binding) in [
            ("attempt-session-1", &first),
            ("attempt-session-2", &second),
        ] {
            let mut session = session_record(id, "2026-08-15T00:00:00Z");
            session.execution_binding = Some(binding.clone());
            insert_session(&conn, &session)?;
        }
        insert_launch_adoption(
            &conn,
            "attempt-adoption-1",
            "attempt-session-1",
            &request_adoption_fixture("attempt-key-1", &first.request_digest),
            &first,
            "2026-08-15T00:00:00Z",
        )?;
        let second_request = request_adoption_fixture("attempt-key-2", &second.request_digest);
        assert!(matches!(
            resolve_launch_adoption(&conn, &second_request, &second)?,
            AdoptionResolution::Absent
        ));
        insert_launch_adoption(
            &conn,
            "attempt-adoption-2",
            "attempt-session-2",
            &second_request,
            &second,
            "2026-08-15T00:00:00Z",
        )?;
        assert_eq!(raw_adoption_rows(&conn)?.len(), 2);
        Ok(())
    }

    #[test]
    fn request_adoption_insert_helpers_reject_incomplete_identity() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let conn = open_store(&temp.path().join("insert-validation.sqlite3"))?;
        let binding = execution_binding_fixture();
        let other_binding = execution_binding_with_attempt("other-attempt");
        let mut session = session_record("validated-session", "2026-08-15T00:00:00Z");
        session.execution_binding = Some(binding.clone());
        insert_session(&conn, &session)?;

        let mut invalid_request =
            request_adoption_fixture("invalid key with spaces", &binding.request_digest);
        assert!(insert_launch_adoption(
            &conn,
            "invalid-key",
            "validated-session",
            &invalid_request,
            &binding,
            "2026-08-15T00:00:00Z",
        )
        .is_err());
        invalid_request = request_adoption_fixture("valid-key", &digest_fixture('d'));
        assert!(insert_launch_adoption(
            &conn,
            "mismatched-digest",
            "validated-session",
            &invalid_request,
            &binding,
            "2026-08-15T00:00:00Z",
        )
        .is_err());
        assert!(insert_input_adoption(
            &conn,
            "mismatched-session-binding",
            "validated-session",
            &request_adoption_fixture("input-key", &digest_fixture('d')),
            &other_binding,
            "2026-08-15T00:00:00Z",
        )
        .is_err());
        let mut invalid_binding = binding.clone();
        invalid_binding.project_digest = "not-a-digest".to_string();
        assert!(insert_input_adoption(
            &conn,
            "invalid-binding",
            "validated-session",
            &request_adoption_fixture("input-key-2", &digest_fixture('d')),
            &invalid_binding,
            "2026-08-15T00:00:00Z",
        )
        .is_err());
        assert!(insert_input_adoption(
            &conn,
            "missing-session",
            "missing-session",
            &request_adoption_fixture("input-key-3", &digest_fixture('d')),
            &binding,
            "2026-08-15T00:00:00Z",
        )
        .is_err());
        assert!(raw_adoption_rows(&conn)?.is_empty());
        Ok(())
    }

    #[test]
    fn request_adoptions_separate_connection_insert_races_have_one_winner() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let path = temp.path().join("races.sqlite3");
        let conn = open_store(&path)?;
        let launch_binding = execution_binding_fixture();
        let input_binding = execution_binding_with_attempt("input-attempt");
        let cross_launch_binding = execution_binding_with_attempt("cross-launch-attempt");
        let cross_input_binding = execution_binding_with_attempt("cross-input-attempt");
        for (id, binding) in [
            ("race-launch-session", &launch_binding),
            ("race-input-session", &input_binding),
            ("race-cross-launch-session", &cross_launch_binding),
            ("race-cross-input-session", &cross_input_binding),
        ] {
            let mut session = session_record(id, "2026-08-15T00:00:00Z");
            session.execution_binding = Some(binding.clone());
            insert_session(&conn, &session)?;
        }
        drop(conn);

        let launch_barrier = std::sync::Arc::new(std::sync::Barrier::new(3));
        let mut launch_handles = Vec::new();
        for suffix in ["a", "b"] {
            let path = path.clone();
            let barrier = std::sync::Arc::clone(&launch_barrier);
            let binding = launch_binding.clone();
            launch_handles.push(std::thread::spawn(move || {
                let conn = open_initialized_store(&path)?;
                let request = request_adoption_fixture("race-launch-key", &binding.request_digest);
                barrier.wait();
                insert_launch_adoption(
                    &conn,
                    &format!("race-launch-{suffix}"),
                    "race-launch-session",
                    &request,
                    &binding,
                    "2026-08-15T00:00:00Z",
                )
            }));
        }
        launch_barrier.wait();
        let launch_results = launch_handles
            .into_iter()
            .map(|handle| handle.join().expect("launch race thread"))
            .collect::<Vec<_>>();
        assert_eq!(
            launch_results
                .iter()
                .filter(|result| result.is_ok())
                .count(),
            1,
            "{launch_results:?}"
        );

        let input_barrier = std::sync::Arc::new(std::sync::Barrier::new(3));
        let mut input_handles = Vec::new();
        for suffix in ["a", "b"] {
            let path = path.clone();
            let barrier = std::sync::Arc::clone(&input_barrier);
            let binding = input_binding.clone();
            input_handles.push(std::thread::spawn(move || {
                let conn = open_initialized_store(&path)?;
                let request = request_adoption_fixture("race-input-key", &digest_fixture('d'));
                barrier.wait();
                insert_input_adoption(
                    &conn,
                    &format!("race-input-{suffix}"),
                    "race-input-session",
                    &request,
                    &binding,
                    "2026-08-15T00:00:00Z",
                )
            }));
        }
        input_barrier.wait();
        let input_results = input_handles
            .into_iter()
            .map(|handle| handle.join().expect("input race thread"))
            .collect::<Vec<_>>();
        assert_eq!(
            input_results.iter().filter(|result| result.is_ok()).count(),
            1,
            "{input_results:?}"
        );

        let cross_barrier = std::sync::Arc::new(std::sync::Barrier::new(3));
        let launch_path = path.clone();
        let launch_barrier = std::sync::Arc::clone(&cross_barrier);
        let launch_binding_for_thread = cross_launch_binding.clone();
        let cross_launch = std::thread::spawn(move || {
            let conn = open_initialized_store(&launch_path)?;
            let request = request_adoption_fixture(
                "race-cross-key",
                &launch_binding_for_thread.request_digest,
            );
            launch_barrier.wait();
            insert_launch_adoption(
                &conn,
                "race-cross-launch",
                "race-cross-launch-session",
                &request,
                &launch_binding_for_thread,
                "2026-08-15T00:00:00Z",
            )
        });
        let input_path = path.clone();
        let input_barrier = std::sync::Arc::clone(&cross_barrier);
        let input_binding_for_thread = cross_input_binding.clone();
        let cross_input = std::thread::spawn(move || {
            let conn = open_initialized_store(&input_path)?;
            let request = request_adoption_fixture("race-cross-key", &digest_fixture('d'));
            input_barrier.wait();
            insert_input_adoption(
                &conn,
                "race-cross-input",
                "race-cross-input-session",
                &request,
                &input_binding_for_thread,
                "2026-08-15T00:00:00Z",
            )
        });
        cross_barrier.wait();
        let cross_results = [
            cross_launch.join().expect("cross-operation launch race"),
            cross_input.join().expect("cross-operation input race"),
        ];
        assert_eq!(
            cross_results.iter().filter(|result| result.is_ok()).count(),
            1,
            "{cross_results:?}"
        );

        let conn = open_initialized_store(&path)?;
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM request_adoptions
                 WHERE adoption_key IN (
                    'race-launch-key', 'race-input-key', 'race-cross-key'
                 )",
                [],
                |row| row.get::<_, i64>(0),
            )?,
            3
        );
        Ok(())
    }

    #[test]
    fn request_adoptions_close_reopen_preserves_exact_record_and_session() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let path = temp.path().join("reopen.sqlite3");
        let binding = execution_binding_fixture();
        let request = request_adoption_fixture("reopen-key", &binding.request_digest);
        let input_request = request_adoption_fixture("reopen-input-key", &digest_fixture('d'));
        let expected_session = {
            let conn = open_store(&path)?;
            let mut session = session_record("reopen-session", "2026-08-15T00:00:00Z");
            session.execution_binding = Some(binding.clone());
            insert_session(&conn, &session)?;
            insert_launch_adoption(
                &conn,
                "reopen-adoption",
                "reopen-session",
                &request,
                &binding,
                "2026-08-15T00:01:00Z",
            )?;
            insert_input_adoption(
                &conn,
                "reopen-input-adoption",
                "reopen-session",
                &input_request,
                &binding,
                "2026-08-15T00:02:00Z",
            )?;
            let launch_before =
                load_request_adoption_by_id(&conn, "reopen-adoption")?.expect("record");
            let input_before =
                load_request_adoption_by_id(&conn, "reopen-input-adoption")?.expect("record");
            assert_adoption_replay(
                resolve_launch_adoption(&conn, &request, &binding)?,
                "reopen-adoption",
                &session,
            );
            assert_adoption_replay(
                resolve_input_adoption(&conn, "reopen-session", &input_request, &binding)?,
                "reopen-input-adoption",
                &session,
            );
            (launch_before, input_before, session)
        };

        initialize_store(&path)?;
        let conn = open_initialized_store(&path)?;
        let after = load_request_adoption_by_id(&conn, "reopen-adoption")?.expect("record");
        assert_eq!(after, expected_session.0);
        let input_after =
            load_request_adoption_by_id(&conn, "reopen-input-adoption")?.expect("record");
        assert_eq!(input_after, expected_session.1);
        assert_adoption_replay(
            resolve_launch_adoption(&conn, &request, &binding)?,
            "reopen-adoption",
            &expected_session.2,
        );
        assert_adoption_replay(
            resolve_input_adoption(&conn, "reopen-session", &input_request, &binding)?,
            "reopen-input-adoption",
            &expected_session.2,
        );
        Ok(())
    }

    #[test]
    fn adopted_input_atomic_not_live_rolls_back_without_rows() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let mut conn = open_store(&temp.path().join("not-live-input.sqlite3"))?;
        let binding = execution_binding_fixture();
        let mut session = session_record("not-live-input", "2026-08-15T00:00:00Z");
        session.status = "completed".to_string();
        session.execution_binding = Some(binding.clone());
        insert_session(&conn, &session)?;
        let request = request_adoption_fixture("not-live-input-key", &digest_fixture('d'));

        let result = acquire_session_input_lease_and_adopt(
            &mut conn,
            &session.id,
            &request,
            &binding,
            "2026-08-15T00:01:00Z",
        )?;

        assert_eq!(result, InputAdoptionResult::NotLive);
        assert!(conn.is_autocommit(), "helper must close its transaction");
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM request_adoptions WHERE session_id = ?1",
                [&session.id],
                |row| row.get::<_, i64>(0),
            )?,
            0
        );
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM session_input_leases WHERE session_id = ?1",
                [&session.id],
                |row| row.get::<_, i64>(0),
            )?,
            0
        );
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM events WHERE session_id = ?1",
                [&session.id],
                |row| row.get::<_, i64>(0),
            )?,
            0
        );
        let transaction =
            conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        transaction.rollback()?;
        Ok(())
    }

    #[test]
    fn request_adoptions_have_no_production_mutation_or_prune_helpers() {
        let source = include_str!("store.rs");
        for verb in ["update", "delete", "prune"] {
            let forbidden = ["pub fn ", verb, "_request_", "adoption"].concat();
            assert!(
                !source.contains(&forbidden),
                "append-only ledger exposed forbidden helper {forbidden}"
            );
        }
    }

    #[test]
    fn execution_binding_fresh_schema_has_column() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let conn = open_store(&temp.path().join("test.sqlite3"))?;
        let cols = table_columns(&conn, "sessions")?;
        assert!(
            cols.iter().any(|c| c == "execution_binding_json"),
            "sessions.execution_binding_json column missing; cols={cols:?}"
        );
        Ok(())
    }

    #[test]
    fn execution_binding_legacy_migration_defaults_null() -> Result<()> {
        // Simulate a pre-feature store that predates the execution_binding_json
        // column (same shape used by legacy_db_without_familiar_id_column_migrates_in_place).
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
        assert!(cols.iter().any(|c| c == "execution_binding_json"));
        let raw: Option<String> = conn.query_row(
            "SELECT execution_binding_json FROM sessions WHERE id='legacy-1'",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(raw, None);
        let migrated = get_session(&conn, "legacy-1")?.expect("legacy row should still read back");
        assert_eq!(migrated.execution_binding, None);
        Ok(())
    }

    #[test]
    fn execution_binding_round_trips_through_insert_get_list_and_reopen() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let path = temp.path().join("test.sqlite3");
        let mut bound = session_record("bound", "2026-06-03T00:00:00Z");
        bound.execution_binding = Some(execution_binding_fixture());
        let unbound = session_record("unbound", "2026-06-03T00:00:01Z");
        {
            let conn = open_store(&path)?;
            insert_session(&conn, &bound)?;
            insert_session(&conn, &unbound)?;

            let got_bound = get_session(&conn, "bound")?.expect("bound session exists");
            assert_eq!(
                got_bound.execution_binding,
                Some(execution_binding_fixture())
            );
            let got_unbound = get_session(&conn, "unbound")?.expect("unbound session exists");
            assert_eq!(got_unbound.execution_binding, None);

            let listed = list_sessions(&conn)?;
            let listed_bound = listed.iter().find(|s| s.id == "bound").unwrap();
            let listed_unbound = listed.iter().find(|s| s.id == "unbound").unwrap();
            assert_eq!(
                listed_bound.execution_binding,
                Some(execution_binding_fixture())
            );
            assert_eq!(listed_unbound.execution_binding, None);
        }
        // Reopen the store from disk — the typed value must round-trip
        // through a fresh connection, not just survive in-process.
        {
            let conn = open_store(&path)?;
            let reopened = get_session(&conn, "bound")?.expect("bound session exists after reopen");
            assert_eq!(
                reopened.execution_binding,
                Some(execution_binding_fixture())
            );
        }
        Ok(())
    }

    #[test]
    fn execution_binding_raw_json_is_byte_identical_across_reopen() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let path = temp.path().join("test.sqlite3");
        let mut bound = session_record("bound", "2026-06-03T00:00:00Z");
        bound.execution_binding = Some(execution_binding_fixture());
        let before = {
            let conn = open_store(&path)?;
            insert_session(&conn, &bound)?;
            conn.query_row(
                "SELECT execution_binding_json FROM sessions WHERE id='bound'",
                [],
                |row| row.get::<_, String>(0),
            )?
        };
        let after = {
            let conn = open_store(&path)?;
            conn.query_row(
                "SELECT execution_binding_json FROM sessions WHERE id='bound'",
                [],
                |row| row.get::<_, String>(0),
            )?
        };
        assert_eq!(
            before, after,
            "raw stored JSON must be byte-identical across reopen"
        );
        assert_eq!(
            before,
            serde_json::to_string(&execution_binding_fixture()).unwrap()
        );
        Ok(())
    }

    #[test]
    fn execution_binding_invalid_json_stored_text_is_a_store_error() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let conn = open_store(&temp.path().join("test.sqlite3"))?;
        insert_session(&conn, &session_record("s1", "2026-06-03T00:00:00Z"))?;
        conn.execute(
            "UPDATE sessions SET execution_binding_json = ?1 WHERE id = 's1'",
            params!["{not valid json"],
        )?;
        let error = get_session(&conn, "s1").expect_err("invalid JSON must be a store error");
        assert!(
            error.to_string().to_lowercase().contains("session"),
            "unexpected error: {error:#}"
        );
        Ok(())
    }

    #[test]
    fn execution_binding_unsupported_contract_stored_text_is_a_store_error() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let conn = open_store(&temp.path().join("test.sqlite3"))?;
        insert_session(&conn, &session_record("s1", "2026-06-03T00:00:00Z"))?;
        let mut binding = execution_binding_fixture();
        binding.contract = "psyche.execution_binding.v0".to_string();
        let raw = serde_json::to_string(&binding).unwrap();
        conn.execute(
            "UPDATE sessions SET execution_binding_json = ?1 WHERE id = 's1'",
            params![raw],
        )?;
        let result = get_session(&conn, "s1");
        assert!(
            result.is_err(),
            "unsupported contract stored text must be a store error, got: {result:?}"
        );
        Ok(())
    }

    #[test]
    fn execution_binding_invalid_digest_stored_text_is_a_store_error() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let conn = open_store(&temp.path().join("test.sqlite3"))?;
        insert_session(&conn, &session_record("s1", "2026-06-03T00:00:00Z"))?;
        let mut binding = execution_binding_fixture();
        binding.project_digest = "not-a-digest".to_string();
        let raw = serde_json::to_string(&binding).unwrap();
        conn.execute(
            "UPDATE sessions SET execution_binding_json = ?1 WHERE id = 's1'",
            params![raw],
        )?;
        let result = get_session(&conn, "s1");
        assert!(
            result.is_err(),
            "invalid digest shape stored text must be a store error, got: {result:?}"
        );
        Ok(())
    }

    #[test]
    fn execution_binding_invalid_shape_stored_text_is_a_store_error() -> Result<()> {
        // A syntactically valid JSON object that is missing required
        // executionBinding members must still be rejected, never silently
        // treated as None.
        let temp = tempfile::tempdir()?;
        let conn = open_store(&temp.path().join("test.sqlite3"))?;
        insert_session(&conn, &session_record("s1", "2026-06-03T00:00:00Z"))?;
        conn.execute(
            "UPDATE sessions SET execution_binding_json = ?1 WHERE id = 's1'",
            params![
                serde_json::json!({ "contract": crate::execution_binding::CONTRACT }).to_string()
            ],
        )?;
        let result = get_session(&conn, "s1");
        assert!(
            result.is_err(),
            "shape violation stored text must be a store error, got: {result:?}"
        );
        Ok(())
    }

    #[test]
    fn execution_binding_unbound_session_serializes_execution_binding_null() -> Result<()> {
        let record = session_record("s1", "2026-06-03T00:00:00Z");
        let value = serde_json::to_value(&record).unwrap();
        assert_eq!(value["execution_binding"], serde_json::Value::Null);
        Ok(())
    }

    #[test]
    fn execution_binding_bound_session_serializes_full_typed_object() -> Result<()> {
        let mut record = session_record("s1", "2026-06-03T00:00:00Z");
        record.execution_binding = Some(execution_binding_fixture());
        let value = serde_json::to_value(&record).unwrap();
        let binding = &value["execution_binding"];
        assert_eq!(binding["contract"], crate::execution_binding::CONTRACT);
        assert_eq!(binding["familiarId"], "sage");
        assert_eq!(binding["projectDigest"], digest_fixture('b'));
        assert_eq!(binding["parent"], serde_json::Value::Null);
        Ok(())
    }

    #[test]
    fn execution_binding_two_sessions_with_identical_bindings_both_insert() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let conn = open_store(&temp.path().join("test.sqlite3"))?;
        let mut first = session_record("s1", "2026-06-03T00:00:00Z");
        first.execution_binding = Some(execution_binding_fixture());
        let mut second = session_record("s2", "2026-06-03T00:00:01Z");
        second.execution_binding = Some(execution_binding_fixture());
        insert_session(&conn, &first)?;
        insert_session(&conn, &second)?;

        let got_first = get_session(&conn, "s1")?.expect("first session exists");
        let got_second = get_session(&conn, "s2")?.expect("second session exists");
        assert_eq!(
            got_first.execution_binding,
            Some(execution_binding_fixture())
        );
        assert_eq!(
            got_second.execution_binding,
            Some(execution_binding_fixture())
        );
        Ok(())
    }

    #[test]
    fn execution_binding_survives_status_and_archive_updates_unchanged() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let conn = open_store(&temp.path().join("test.sqlite3"))?;
        let mut bound = session_record("s1", "2026-06-03T00:00:00Z");
        bound.execution_binding = Some(execution_binding_fixture());
        insert_session(&conn, &bound)?;
        let raw_before: String = conn.query_row(
            "SELECT execution_binding_json FROM sessions WHERE id='s1'",
            [],
            |row| row.get(0),
        )?;

        update_session_status(&conn, "s1", "completed", Some(0), "2026-06-03T00:01:00Z")?;
        archive_session(&conn, "s1", "2026-06-03T00:02:00Z")?;
        summon_session(&conn, "s1", "2026-06-03T00:03:00Z")?;

        let raw_after: String = conn.query_row(
            "SELECT execution_binding_json FROM sessions WHERE id='s1'",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(
            raw_before, raw_after,
            "normal status/archive updates must not touch the binding bytes"
        );
        let reread = get_session(&conn, "s1")?.expect("session still exists");
        assert_eq!(reread.execution_binding, Some(execution_binding_fixture()));
        Ok(())
    }

    #[test]
    fn execution_binding_insert_if_absent_persists_binding() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let conn = open_store(&temp.path().join("test.sqlite3"))?;
        let mut bound = session_record("s1", "2026-06-03T00:00:00Z");
        bound.execution_binding = Some(execution_binding_fixture());

        let inserted = insert_session_if_absent(&conn, &bound)?;
        assert!(
            inserted,
            "first insert_session_if_absent call should insert"
        );
        let got = get_session(&conn, "s1")?.expect("session exists");
        assert_eq!(got.execution_binding, Some(execution_binding_fixture()));

        // A second call with a different binding must be ignored (row already
        // exists), proving the original stored bytes are untouched.
        let mut other = bound.clone();
        other.execution_binding = None;
        let inserted_again = insert_session_if_absent(&conn, &other)?;
        assert!(
            !inserted_again,
            "row already exists; insert must be ignored"
        );
        let still_bound = get_session(&conn, "s1")?.expect("session exists");
        assert_eq!(
            still_bound.execution_binding,
            Some(execution_binding_fixture())
        );
        Ok(())
    }

    // -------------------------------------------------------------------------
    // ingest_external_transcript tests
    // -------------------------------------------------------------------------

    fn write_transcript(path: &std::path::Path, lines: &[&str]) -> std::io::Result<()> {
        use std::io::Write as _;
        let mut f = std::fs::File::create(path)?;
        for line in lines {
            writeln!(f, "{line}")?;
        }
        Ok(())
    }

    #[test]
    fn ingest_external_transcript_indexes_text_and_makes_it_searchable() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let db_path = temp.path().join("coven.db");
        let conn = open_store(&db_path)?;

        // Write a small fixture transcript (coven-code TranscriptEntry shape).
        let transcript_path = temp.path().join("ext-sess.jsonl");
        write_transcript(
            &transcript_path,
            &[
                r#"{"type":"user","message":{"content":[{"type":"text","text":"SEARCHABLE_TOKEN the quick brown fox"}]}}"#,
                r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Hello, I can help with that."}]}}"#,
                r#"{"type":"unknown_type"}"#, // no extractable text — should be skipped
            ],
        )?;

        // Insert the external session pointing at the transcript.
        let mut sess = session_record("ext-ingest-1", "2026-07-01T10:00:00Z");
        sess.external = true;
        sess.transcript_path = Some(transcript_path.to_string_lossy().to_string());
        insert_session(&conn, &sess)?;

        let now = "2026-07-01T10:00:01Z";
        let n = ingest_external_transcript(&conn, "ext-ingest-1", temp.path(), now)?;
        assert_eq!(n, 2, "two text chunks should be indexed");

        // FTS search must find the token.
        let hits = search_events(&conn, "SEARCHABLE_TOKEN")?;
        assert_eq!(hits.len(), 1, "search should return one hit");
        assert_eq!(hits[0].session_id, "ext-ingest-1");
        assert_eq!(hits[0].kind, "transcript_text");

        Ok(())
    }

    #[test]
    fn ingest_external_transcript_is_idempotent() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let conn = open_store(&temp.path().join("coven.db"))?;

        let transcript_path = temp.path().join("idem.jsonl");
        write_transcript(
            &transcript_path,
            &[
                r#"{"type":"user","message":{"content":[{"type":"text","text":"idempotency check"}]}}"#,
            ],
        )?;

        let mut sess = session_record("ext-idem", "2026-07-01T11:00:00Z");
        sess.external = true;
        sess.transcript_path = Some(transcript_path.to_string_lossy().to_string());
        insert_session(&conn, &sess)?;

        let now = "2026-07-01T11:00:01Z";
        let first = ingest_external_transcript(&conn, "ext-idem", temp.path(), now)?;
        assert_eq!(first, 1, "first call should index one chunk");

        // Second call must be a no-op: transcript_indexed_at is already set.
        let second = ingest_external_transcript(&conn, "ext-idem", temp.path(), now)?;
        assert_eq!(second, 0, "second call should be a no-op");

        // Exactly one event should exist — no duplicates.
        let events = list_events(&conn, "ext-idem")?;
        assert_eq!(events.len(), 1, "no duplicate events on re-ingest");

        Ok(())
    }

    #[test]
    fn ingest_external_transcript_skips_non_external_session() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let conn = open_store(&temp.path().join("coven.db"))?;

        // A normal (non-external) session.
        let sess = session_record("internal-sess", "2026-07-01T12:00:00Z");
        insert_session(&conn, &sess)?;

        let now = "2026-07-01T12:00:01Z";
        let n = ingest_external_transcript(&conn, "internal-sess", temp.path(), now)?;
        assert_eq!(n, 0, "non-external session should be skipped");

        let events = list_events(&conn, "internal-sess")?;
        assert!(events.is_empty(), "no events should be inserted");

        Ok(())
    }

    #[test]
    fn ingest_external_transcript_skips_when_no_transcript_path() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let conn = open_store(&temp.path().join("coven.db"))?;

        let mut sess = session_record("ext-no-path", "2026-07-01T13:00:00Z");
        sess.external = true;
        // transcript_path is intentionally None.
        insert_session(&conn, &sess)?;

        let now = "2026-07-01T13:00:01Z";
        let n = ingest_external_transcript(&conn, "ext-no-path", temp.path(), now)?;
        assert_eq!(n, 0, "session without transcript_path should be skipped");

        Ok(())
    }

    #[test]
    fn ingest_external_transcript_returns_zero_for_missing_file() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let conn = open_store(&temp.path().join("coven.db"))?;

        let mut sess = session_record("ext-missing", "2026-07-01T14:00:00Z");
        sess.external = true;
        sess.transcript_path = Some("/tmp/nonexistent-coven-transcript.jsonl".to_string());
        insert_session(&conn, &sess)?;

        let now = "2026-07-01T14:00:01Z";
        let n = ingest_external_transcript(&conn, "ext-missing", temp.path(), now)?;
        assert_eq!(n, 0, "missing file should return 0 without error");

        // transcript_indexed_at must remain NULL so a future retry is possible.
        let indexed_at: Option<String> = conn
            .query_row(
                "SELECT transcript_indexed_at FROM sessions WHERE id = 'ext-missing'",
                [],
                |row| row.get(0),
            )
            .optional()?
            .flatten();
        assert!(
            indexed_at.is_none(),
            "transcript_indexed_at should stay NULL so the session retries later"
        );

        Ok(())
    }

    #[test]
    fn ingest_external_transcript_fallback_top_level_text() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let conn = open_store(&temp.path().join("coven.db"))?;

        let transcript_path = temp.path().join("toplevel.jsonl");
        write_transcript(&transcript_path, &[r#"{"text":"TOPLEVEL_FALLBACK_TOKEN"}"#])?;

        let mut sess = session_record("ext-fallback", "2026-07-01T15:00:00Z");
        sess.external = true;
        sess.transcript_path = Some(transcript_path.to_string_lossy().to_string());
        insert_session(&conn, &sess)?;

        let now = "2026-07-01T15:00:01Z";
        let n = ingest_external_transcript(&conn, "ext-fallback", temp.path(), now)?;
        assert_eq!(n, 1, "fallback text extraction should yield one chunk");

        let hits = search_events(&conn, "TOPLEVEL_FALLBACK_TOKEN")?;
        assert_eq!(hits.len(), 1, "top-level text should be searchable");

        Ok(())
    }
}
