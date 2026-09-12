use std::path::Path;

use anyhow::{bail, Context, Result};
use rusqlite::{params, Connection, ErrorCode, OptionalExtension};

use super::{sqlite_object_exists, StoreInitializationPhase, WARD_AUDIT_CAPACITY_SQLITE_MESSAGE};

pub const DEFAULT_WARD_AUDIT_CAPACITY_BYTES: u64 = 256 * 1024 * 1024;
pub const WARD_AUDIT_WAL_LIMIT_BYTES: u64 = 128 * 1024 * 1024;
const FTS_BACKFILL_BATCH_SIZE: i64 = 1_000;
const FTS_BACKFILL_COMPLETE_KEY: &str = "events_fts_backfill_complete";
const WARD_AUDIT_WAL_AUTOCHECKPOINT_BYTES: u64 = 4 * 1024 * 1024;
const WARD_AUDIT_JOURNAL_RETAIN_BYTES: u64 = 16 * 1024 * 1024;
const WARD_AUDIT_ROW_OVERHEAD_PAGES: u64 = 4;
const WARD_AUDIT_CAPACITY_TRIGGER: &str = "coven_ward_audit_capacity_insert";

pub(super) fn load_ward_audit_schema_state(conn: &Connection) -> Result<String> {
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

pub(super) fn apply_ward_audit_schema_state(conn: &Connection, schema_state: &str) -> Result<()> {
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

pub(super) fn initialize_store_with_observer(
    path: &Path,
    mut observe: impl FnMut(StoreInitializationPhase),
) -> Result<()> {
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
    observe(StoreInitializationPhase::ConnectionConfigured);
    // Table-rebuild migrators own their transactions so they can change SQLite
    // foreign-key mode safely and roll back atomically. Run them before the
    // transaction for the remaining idempotent schema work.
    ensure_ward_audit_schema(&conn)?;
    observe(StoreInitializationPhase::WardComplete);
    crate::automations::runs::ensure_runtime_authority_unsupported_failure_class(&conn)?;
    crate::automations::runtime_terminal_evidence::ensure_runtime_terminal_evidence_schema(&conn)?;
    observe(StoreInitializationPhase::RuntimeComplete);
    conn.execute_batch("BEGIN IMMEDIATE")
        .context("failed to acquire SQLite initialization transaction")?;
    observe(StoreInitializationPhase::MainLockAcquired);
    let result = initialize_store_schema(&conn);
    match result {
        Ok(()) => {
            observe(StoreInitializationPhase::MainSchemaComplete);
            conn.execute_batch("COMMIT")
                .context("failed to commit SQLite initialization transaction")?;
            observe(StoreInitializationPhase::CommitComplete);
        }
        Err(error) => {
            let _ = conn.execute_batch("ROLLBACK");
            return Err(error);
        }
    }
    Ok(())
}

pub(super) fn open_initialized_store(path: &Path) -> Result<Connection> {
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

        CREATE TABLE IF NOT EXISTS coven_ward_audit_capacity (
            singleton          INTEGER PRIMARY KEY CHECK (singleton = 1),
            limit_bytes        INTEGER NOT NULL CHECK (limit_bytes > 0),
            used_bytes         INTEGER NOT NULL CHECK (used_bytes >= 0),
            row_overhead_bytes INTEGER NOT NULL CHECK (row_overhead_bytes > 0),
            wal_limit_bytes    INTEGER NOT NULL CHECK (wal_limit_bytes > 0)
        );

        CREATE TABLE IF NOT EXISTS coven_ward_audit_reservations (
            token          TEXT PRIMARY KEY NOT NULL,
            purpose        TEXT NOT NULL,
            reserved_bytes INTEGER NOT NULL CHECK (reserved_bytes >= 0),
            created_at     TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
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
    ensure_ward_audit_capacity_wal_limit_column(conn)?;
    initialize_ward_audit_capacity(conn)?;
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
    conn.execute_batch(crate::automations::store::AUTOMATION_DEFINITIONS_SCHEMA_SQL)
        .context("failed to initialize automation_definitions schema")?;
    crate::automations::store::ensure_definition_command_columns(conn)?;
    conn.execute_batch(
        crate::automations::command_adoption::AUTOMATION_COMMAND_ADOPTIONS_SCHEMA_SQL,
    )
    .context("failed to initialize automation command adoption schema")?;
    crate::automations::cancellation::ensure_cancellation_schema(conn)
        .context("failed to initialize automation cancellation schema")?;
    conn.execute_batch(crate::automations::leadership::AUTOMATION_SCHEDULER_AUTHORITY_SCHEMA_SQL)
        .context("failed to initialize automation scheduler authority schema")?;
    conn.execute_batch(
        crate::automations::diagnostics::AUTOMATION_SCHEDULER_DIAGNOSTICS_SCHEMA_SQL,
    )
    .context("failed to initialize automation scheduler diagnostics schema")?;
    conn.execute_batch(crate::automations::occurrences::AUTOMATION_OCCURRENCES_SCHEMA_SQL)
        .context("failed to initialize automation_occurrences schema")?;
    crate::automations::occurrences::ensure_occurrence_kind(conn)?;
    conn.execute_batch(crate::automations::runs::AUTOMATION_RUNS_SCHEMA_SQL)
        .context("failed to initialize automation_runs schema")?;
    crate::automations::runs::ensure_timeout_column(conn)?;
    conn.execute_batch(crate::automations::runs::AUTOMATION_ATTEMPTS_SCHEMA_SQL)
        .context("failed to initialize automation attempts and retry state schema")?;
    crate::automations::runs::ensure_authority_columns(conn)?;
    crate::automations::command_adoption::ensure_global_adoption_key_guards(conn)?;
    crate::automations::contract::migration::migrate_legacy_contract_metadata(conn)?;
    conn.execute_batch(crate::automations::contract::events::AUTOMATION_EVENTS_SCHEMA_SQL)
        .context("failed to initialize automation events schema")?;
    conn.execute_batch(crate::automations::receipts::AUTOMATION_RECEIPTS_SCHEMA_SQL)
        .context("failed to initialize automation receipts schema")?;
    conn.execute_batch(
        crate::automations::receipts::AUTOMATION_RECEIPT_AUTHORITY_EXTENSIONS_SCHEMA_SQL,
    )
    .context("failed to initialize automation receipt authority extensions schema")?;
    crate::automations::contract::events::backfill_definition_event_baselines(conn)
        .context("failed to backfill automation definition event baselines")?;
    crate::automations::store::migrate_durable_local_timezones(conn)?;

    backfill_events_fts_if_needed(conn)?;

    Ok(())
}

pub(super) fn configure_initializing_connection(conn: &Connection) -> Result<()> {
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
    configure_ward_audit_wal(conn)?;
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

pub(super) fn configure_runtime_writable_connection(conn: &Connection) -> Result<()> {
    // See `configure_initializing_connection` for why recursive_triggers must
    // be ON: it closes the hidden-rowid REPLACE bypass around the
    // `request_adoptions_no_delete` / `request_adoptions_no_replace` guards.
    conn.execute_batch(
        "PRAGMA busy_timeout = 5000;
         PRAGMA foreign_keys = ON;
         PRAGMA recursive_triggers = ON;",
    )
    .context("failed to configure writable Coven store connection")?;
    configure_ward_audit_wal(conn)?;
    let ward_audit_exists = sqlite_object_exists(conn, "table", "ward_audit")?;
    let capacity_exists = sqlite_object_exists(conn, "table", "coven_ward_audit_capacity")?;
    let reservations_exist = sqlite_object_exists(conn, "table", "coven_ward_audit_reservations")?;
    match (ward_audit_exists, capacity_exists, reservations_exist) {
        (true, true, true) => install_ward_audit_capacity_trigger(conn)?,
        (false, false, false) => {}
        _ => anyhow::bail!("Ward audit capacity schema is incomplete"),
    }
    Ok(())
}

fn configure_ward_audit_wal(conn: &Connection) -> Result<()> {
    let page_size: i64 = conn
        .query_row("PRAGMA page_size", [], |row| row.get(0))
        .context("failed to read SQLite page size for Ward audit policy")?;
    anyhow::ensure!(page_size > 0, "SQLite reported a zero page size");
    let page_size = u64::try_from(page_size).context("SQLite page size is negative")?;
    let autocheckpoint_pages =
        WARD_AUDIT_WAL_AUTOCHECKPOINT_BYTES.saturating_add(page_size - 1) / page_size;
    let autocheckpoint_pages = i64::try_from(autocheckpoint_pages)
        .context("Ward audit WAL autocheckpoint exceeds SQLite range")?;
    conn.pragma_update(None, "wal_autocheckpoint", autocheckpoint_pages)
        .context("failed to configure Ward audit WAL autocheckpoint")?;
    let journal_retain = i64::try_from(WARD_AUDIT_JOURNAL_RETAIN_BYTES)
        .context("Ward audit journal retention exceeds SQLite range")?;
    conn.pragma_update(None, "journal_size_limit", journal_retain)
        .context("failed to configure Ward audit WAL retention")?;
    Ok(())
}

fn ward_audit_row_charge_sql(prefix: &str) -> String {
    let column = |name: &str| format!("COALESCE(length(CAST({prefix}.{name} AS BLOB)), 0)");
    format!(
        "(SELECT row_overhead_bytes FROM coven_ward_audit_capacity WHERE singleton = 1)
         + ({event_type} * 2)
         + {proposal_id}
         + ({familiar_id} * 2)
         + {ward_version}
         + {ward_hash}
         + {tier}
         + {decision}
         + {approver}
         + {diff_hash}
         + {detail}
         + {files_touched}
         + {channel}
         + {thread_id}
         + {submitted_at}
         + {decided_at}
         + ({recorded_at} * 3)",
        event_type = column("event_type"),
        proposal_id = column("proposal_id"),
        familiar_id = column("familiar_id"),
        ward_version = column("ward_version"),
        ward_hash = column("ward_hash"),
        tier = column("tier"),
        decision = column("decision"),
        approver = column("approver"),
        diff_hash = column("diff_hash"),
        detail = column("detail"),
        files_touched = column("files_touched"),
        channel = column("channel"),
        thread_id = column("thread_id"),
        submitted_at = column("submitted_at"),
        decided_at = column("decided_at"),
        recorded_at = column("recorded_at"),
    )
}

fn initialize_ward_audit_capacity(conn: &Connection) -> Result<()> {
    let page_size: i64 = conn
        .query_row("PRAGMA page_size", [], |row| row.get(0))
        .context("failed to read SQLite page size for Ward audit capacity")?;
    anyhow::ensure!(page_size > 0, "SQLite reported a zero page size");
    let page_size = u64::try_from(page_size).context("SQLite page size is negative")?;
    let row_overhead = page_size
        .checked_mul(WARD_AUDIT_ROW_OVERHEAD_PAGES)
        .context("Ward audit row overhead overflowed")?;
    let default_limit = i64::try_from(DEFAULT_WARD_AUDIT_CAPACITY_BYTES)
        .context("default Ward audit capacity exceeds SQLite integer range")?;
    let default_wal_limit = i64::try_from(WARD_AUDIT_WAL_LIMIT_BYTES)
        .context("default Ward audit WAL limit exceeds SQLite integer range")?;
    let row_overhead =
        i64::try_from(row_overhead).context("Ward audit row overhead exceeds SQLite range")?;
    conn.execute(
        "INSERT OR IGNORE INTO coven_ward_audit_capacity (
            singleton, limit_bytes, used_bytes, row_overhead_bytes, wal_limit_bytes
         ) VALUES (1, ?1, 0, ?2, ?3)",
        params![default_limit, row_overhead, default_wal_limit],
    )
    .context("failed to initialize Ward audit capacity policy")?;
    conn.execute(
        "UPDATE coven_ward_audit_capacity
         SET row_overhead_bytes = ?1
         WHERE singleton = 1",
        [row_overhead],
    )
    .context("failed to update Ward audit row overhead")?;
    let charge = ward_audit_row_charge_sql("audit");
    conn.execute(
        &format!(
            "UPDATE coven_ward_audit_capacity
             SET used_bytes = (
                SELECT COALESCE(SUM({charge}), 0)
                FROM ward_audit AS audit
             )
             WHERE singleton = 1"
        ),
        [],
    )
    .context("failed to reconcile durable Ward audit capacity usage")?;
    Ok(())
}

fn ensure_ward_audit_capacity_wal_limit_column(conn: &Connection) -> Result<()> {
    let has_column: bool = conn
        .prepare("PRAGMA table_info(coven_ward_audit_capacity)")?
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?
        .into_iter()
        .any(|column| column == "wal_limit_bytes");
    if !has_column {
        conn.execute_batch(
            "ALTER TABLE coven_ward_audit_capacity
             ADD COLUMN wal_limit_bytes INTEGER NOT NULL DEFAULT 134217728
             CHECK (wal_limit_bytes > 0);",
        )
        .context("failed to add Ward audit WAL capacity limit")?;
    }
    Ok(())
}

fn install_ward_audit_capacity_trigger(conn: &Connection) -> Result<()> {
    let charge = ward_audit_row_charge_sql("NEW");
    conn.execute_batch(&format!(
        "CREATE TEMP TABLE IF NOT EXISTS coven_active_ward_audit_reservation (
             singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
             token     TEXT NOT NULL
         );
         DROP TRIGGER IF EXISTS temp.{WARD_AUDIT_CAPACITY_TRIGGER};
         CREATE TEMP TRIGGER {WARD_AUDIT_CAPACITY_TRIGGER}
         BEFORE INSERT ON main.ward_audit
         BEGIN
             SELECT CASE
                 WHEN NOT EXISTS (
                     SELECT 1
                     FROM coven_ward_audit_capacity
                     WHERE singleton = 1
                 )
                 THEN RAISE(ABORT, 'Ward audit capacity policy is unavailable')
             END;
             SELECT CASE
                 WHEN EXISTS (
                     SELECT 1 FROM coven_active_ward_audit_reservation
                 )
                  AND NOT EXISTS (
                     SELECT 1
                     FROM coven_ward_audit_reservations
                     WHERE token = (
                         SELECT token
                         FROM coven_active_ward_audit_reservation
                         WHERE singleton = 1
                     )
                 )
                 THEN RAISE(ABORT, 'Ward audit reservation is unavailable')
             END;
             SELECT CASE
                 WHEN EXISTS (
                     SELECT 1 FROM coven_active_ward_audit_reservation
                 )
                  AND (
                     SELECT reserved_bytes
                     FROM coven_ward_audit_reservations
                     WHERE token = (
                         SELECT token
                         FROM coven_active_ward_audit_reservation
                         WHERE singleton = 1
                     )
                  ) < ({charge})
                 THEN RAISE(ABORT, 'Ward audit reservation is exhausted')
             END;
             SELECT CASE
                 WHEN NOT EXISTS (
                     SELECT 1 FROM coven_active_ward_audit_reservation
                 )
                  AND (
                     used_bytes > limit_bytes
                     OR COALESCE((
                         SELECT SUM(reserved_bytes)
                         FROM coven_ward_audit_reservations
                     ), 0) > (limit_bytes - used_bytes)
                     OR ({charge}) > (
                         limit_bytes
                         - used_bytes
                         - COALESCE((
                             SELECT SUM(reserved_bytes)
                             FROM coven_ward_audit_reservations
                         ), 0)
                     )
                  )
                 THEN RAISE(ABORT, '{WARD_AUDIT_CAPACITY_SQLITE_MESSAGE}')
             END
             FROM coven_ward_audit_capacity
             WHERE singleton = 1;
             UPDATE coven_ward_audit_reservations
             SET reserved_bytes = reserved_bytes - ({charge})
             WHERE token = (
                 SELECT token
                 FROM coven_active_ward_audit_reservation
                 WHERE singleton = 1
             );
             UPDATE coven_ward_audit_capacity
             SET used_bytes = used_bytes + ({charge})
             WHERE singleton = 1;
         END;"
    ))
    .context("failed to install Ward audit capacity trigger")?;
    Ok(())
}

pub(super) fn configure_read_only_connection(conn: &Connection) -> Result<()> {
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

pub(super) fn backfill_events_fts_if_needed(conn: &Connection) -> Result<()> {
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

pub(super) fn migrate_historical_request_adoptions(conn: &Connection) -> Result<()> {
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
        let binding = super::parse_stored_execution_binding(&stored_json)
            .map_err(|_| anyhow::anyhow!("failed to parse historical session execution binding"))?;
        let deterministic_json = serde_json::to_string(&binding)
            .context("failed to serialize historical session execution binding")?;
        if deterministic_json != stored_json {
            bail!("historical session execution binding is not deterministic");
        }

        if super::load_launch_adoption_for_session(conn, &session_id)?.is_some() {
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
