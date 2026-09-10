//! Immutable verifier-backed inbox for runtime terminal evidence.
//!
//! This module deliberately exposes only internal Rust APIs. There is no
//! daemon action or other public submission path in this slice.

use std::collections::BTreeMap;

use anyhow::{Context, Result as AnyhowResult};
use chrono::{SecondsFormat, Utc};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde_json::Value;

use super::contract::authority::{
    validate_authority_profile_structure, AuthorityConsumerClass, AuthorityProfileDisposition,
    AuthorityValidationPhase, AutomationExecutionBinding, AUTHORITY_EXTENSION_KEY,
    AUTHORITY_PROFILE, BASE_PROFILE, RUNTIME_AUTHORITY_CAPABILITY,
};
use super::contract::canonical_json::canonicalize;
use super::contract::runtime_terminal_evidence::{
    classify_runtime_terminal_evidence, verify_runtime_terminal_evidence, RuntimeTerminalEvidence,
    RuntimeTerminalEvidenceClassification, RuntimeTerminalEvidenceError,
    RuntimeTerminalEvidenceErrorCode, RuntimeTerminalEvidenceVerifier,
    VerifiedRuntimeTerminalEvidence,
};
use super::contract::types::ExtensionBag;

pub const AUTOMATION_RUNTIME_TERMINAL_EVIDENCE_SCHEMA_SQL: &str = "
    CREATE TABLE IF NOT EXISTS automation_runtime_terminal_evidence (
        evidence_id TEXT PRIMARY KEY NOT NULL,
        attempt_id TEXT UNIQUE NOT NULL,
        session_id TEXT UNIQUE NOT NULL,
        run_id TEXT NOT NULL,
        binding_id TEXT NOT NULL,
        binding_digest TEXT UNIQUE NOT NULL CHECK (
            length(binding_digest) = 64
            AND binding_digest NOT GLOB '*[^0-9a-f]*'
        ),
        runtime_id TEXT NOT NULL,
        descriptor_digest TEXT NOT NULL CHECK (
            length(descriptor_digest) = 64
            AND descriptor_digest NOT GLOB '*[^0-9a-f]*'
        ),
        producer_key_id TEXT NOT NULL,
        evidence_digest TEXT NOT NULL CHECK (
            length(evidence_digest) = 64
            AND evidence_digest NOT GLOB '*[^0-9a-f]*'
        ),
        classification TEXT NOT NULL CHECK (
            classification IN (
                'receipt_eligible_complete',
                'authenticated_partial_or_ambiguous',
                'authenticated_unknown',
                'policy_violating'
            )
        ),
        produced_at TEXT NOT NULL,
        received_at TEXT NOT NULL,
        canonical_json TEXT NOT NULL CHECK (json_valid(canonical_json)),
        FOREIGN KEY (session_id) REFERENCES sessions(id) ON DELETE RESTRICT,
        FOREIGN KEY (run_id) REFERENCES automation_runs(id) ON DELETE RESTRICT,
        FOREIGN KEY (attempt_id) REFERENCES automation_attempts(id) ON DELETE RESTRICT
    );

    CREATE INDEX IF NOT EXISTS idx_automation_runtime_terminal_evidence_run
        ON automation_runtime_terminal_evidence(run_id);

    CREATE UNIQUE INDEX IF NOT EXISTS idx_automation_runtime_terminal_evidence_binding
        ON automation_runtime_terminal_evidence(binding_id);

    CREATE TRIGGER IF NOT EXISTS automation_runtime_terminal_evidence_no_update
    BEFORE UPDATE ON automation_runtime_terminal_evidence
    BEGIN
        SELECT RAISE(ABORT, 'automation runtime terminal evidence is immutable');
    END;

    CREATE TRIGGER IF NOT EXISTS automation_runtime_terminal_evidence_no_delete
    BEFORE DELETE ON automation_runtime_terminal_evidence
    BEGIN
        SELECT RAISE(ABORT, 'automation runtime terminal evidence is immutable');
    END;
";

pub(crate) fn ensure_runtime_terminal_evidence_schema(conn: &Connection) -> AnyhowResult<()> {
    let table_sql: Option<String> = conn
        .query_row(
            "SELECT sql
             FROM sqlite_master
             WHERE type = 'table'
               AND name = 'automation_runtime_terminal_evidence'",
            [],
            |row| row.get(0),
        )
        .optional()
        .context("failed to inspect automation runtime terminal evidence schema")?;
    let classification_requires_upgrade = table_sql
        .as_deref()
        .is_some_and(|sql| !sql.contains("'authenticated_unknown'"));
    if classification_requires_upgrade || binding_index_requires_upgrade(conn)? {
        return migrate_runtime_terminal_evidence_schema(conn);
    }
    // A savepoint commits standalone initialization without owning a caller's transaction.
    let owns_transaction = conn.is_autocommit();
    conn.execute_batch("SAVEPOINT automation_runtime_terminal_evidence_schema")
        .context("failed to begin runtime evidence schema savepoint")?;
    let result = conn
        .execute_batch(AUTOMATION_RUNTIME_TERMINAL_EVIDENCE_SCHEMA_SQL)
        .context("failed to initialize automation runtime terminal evidence schema")
        .and_then(|()| {
            conn.execute_batch("RELEASE SAVEPOINT automation_runtime_terminal_evidence_schema")
                .context("failed to release runtime evidence schema savepoint")
        });
    if let Err(error) = result {
        // RELEASE can remain busy even after ROLLBACK TO an outermost savepoint.
        let rollback = if owns_transaction {
            "ROLLBACK"
        } else {
            "ROLLBACK TO SAVEPOINT automation_runtime_terminal_evidence_schema;
             RELEASE SAVEPOINT automation_runtime_terminal_evidence_schema;"
        };
        conn.execute_batch(rollback).with_context(|| {
            format!("failed to roll back runtime evidence schema after: {error:#}")
        })?;
        return Err(error);
    }
    Ok(())
}

fn binding_index_requires_upgrade(conn: &Connection) -> AnyhowResult<bool> {
    let unique: Option<i64> = conn
        .query_row(
            "SELECT [unique]
             FROM pragma_index_list('automation_runtime_terminal_evidence')
             WHERE name = 'idx_automation_runtime_terminal_evidence_binding'",
            [],
            |row| row.get(0),
        )
        .optional()
        .context("failed to inspect runtime evidence binding index")?;
    let Some(unique) = unique else {
        return Ok(false);
    };
    let columns = conn
        .prepare(
            "SELECT name
             FROM pragma_index_info('idx_automation_runtime_terminal_evidence_binding')
             ORDER BY seqno",
        )
        .context("failed to inspect runtime evidence binding index columns")?
        .query_map([], |row| row.get::<_, String>(0))
        .context("failed to query runtime evidence binding index columns")?
        .collect::<rusqlite::Result<Vec<_>>>()
        .context("failed to read runtime evidence binding index columns")?;
    Ok(unique != 1 || columns.len() != 1 || columns[0] != "binding_id")
}

fn migrate_runtime_terminal_evidence_schema(conn: &Connection) -> AnyhowResult<()> {
    anyhow::ensure!(
        conn.is_autocommit(),
        "automation runtime terminal evidence migration requires autocommit"
    );
    let legacy_exists = conn
        .query_row(
            "SELECT 1
             FROM sqlite_master
             WHERE type = 'table'
               AND name = 'automation_runtime_terminal_evidence_legacy'",
            [],
            |_| Ok(()),
        )
        .optional()
        .context("failed to inspect legacy automation runtime terminal evidence schema")?
        .is_some();
    anyhow::ensure!(
        !legacy_exists,
        "legacy automation runtime terminal evidence table already exists"
    );

    let foreign_keys_enabled: i64 = conn
        .query_row("PRAGMA foreign_keys", [], |row| row.get(0))
        .context("failed to inspect SQLite foreign-key mode")?;
    conn.pragma_update(None, "foreign_keys", false)
        .context("failed to disable foreign keys for runtime evidence migration")?;
    let migration = (|| -> AnyhowResult<()> {
        let transaction = conn
            .unchecked_transaction()
            .context("failed to begin runtime evidence schema migration")?;
        transaction
            .execute_batch(
                "DROP TRIGGER IF EXISTS automation_runtime_terminal_evidence_no_update;
                 DROP TRIGGER IF EXISTS automation_runtime_terminal_evidence_no_delete;
                 DROP INDEX IF EXISTS idx_automation_runtime_terminal_evidence_run;
                 DROP INDEX IF EXISTS idx_automation_runtime_terminal_evidence_binding;
                 ALTER TABLE automation_runtime_terminal_evidence
                 RENAME TO automation_runtime_terminal_evidence_legacy;",
            )
            .context("failed to prepare runtime evidence schema migration")?;
        transaction
            .execute_batch(AUTOMATION_RUNTIME_TERMINAL_EVIDENCE_SCHEMA_SQL)
            .context("failed to create upgraded runtime evidence schema")?;
        let legacy_rows = transaction
            .prepare(
                "SELECT evidence_id, attempt_id, session_id, run_id, binding_id,
                        binding_digest, runtime_id, descriptor_digest, producer_key_id,
                        evidence_digest, classification, produced_at, received_at,
                        canonical_json
                 FROM automation_runtime_terminal_evidence_legacy
                 ORDER BY evidence_id",
            )
            .context("failed to prepare legacy runtime evidence migration")?
            .query_map([], |row| {
                Ok(StoredRuntimeTerminalEvidence {
                    evidence_id: row.get(0)?,
                    attempt_id: row.get(1)?,
                    session_id: row.get(2)?,
                    run_id: row.get(3)?,
                    binding_id: row.get(4)?,
                    binding_digest: row.get(5)?,
                    runtime_id: row.get(6)?,
                    descriptor_digest: row.get(7)?,
                    producer_key_id: row.get(8)?,
                    evidence_digest: row.get(9)?,
                    classification: row.get(10)?,
                    produced_at: row.get(11)?,
                    received_at: row.get(12)?,
                    canonical_json: row.get(13)?,
                })
            })
            .context("failed to query legacy runtime evidence")?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("failed to read legacy runtime evidence")?;
        for stored in legacy_rows {
            let classification = migrated_classification(&transaction, &stored)?;
            transaction
                .execute(
                    "INSERT INTO automation_runtime_terminal_evidence (
                    evidence_id, attempt_id, session_id, run_id, binding_id,
                    binding_digest, runtime_id, descriptor_digest, producer_key_id,
                    evidence_digest, classification, produced_at, received_at,
                    canonical_json
                 ) VALUES (
                    ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14
                 )",
                    params![
                        stored.evidence_id,
                        stored.attempt_id,
                        stored.session_id,
                        stored.run_id,
                        stored.binding_id,
                        stored.binding_digest,
                        stored.runtime_id,
                        stored.descriptor_digest,
                        stored.producer_key_id,
                        stored.evidence_digest,
                        classification.as_str(),
                        stored.produced_at,
                        stored.received_at,
                        stored.canonical_json,
                    ],
                )
                .context("failed to copy runtime evidence into upgraded schema")?;
        }
        transaction
            .execute_batch("DROP TABLE automation_runtime_terminal_evidence_legacy;")
            .context("failed to finish runtime evidence schema migration")?;
        let foreign_key_errors: i64 = transaction
            .query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
                row.get(0)
            })
            .context("failed to verify foreign keys after runtime evidence migration")?;
        anyhow::ensure!(
            foreign_key_errors == 0,
            "runtime evidence migration produced {foreign_key_errors} foreign-key violation(s)"
        );
        transaction
            .commit()
            .context("failed to commit runtime evidence schema migration")
    })();
    let restore = conn
        .pragma_update(None, "foreign_keys", foreign_keys_enabled != 0)
        .context("failed to restore SQLite foreign-key mode");
    migration?;
    restore
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeTerminalEvidenceStoreOutcome {
    Stored,
    Replayed,
}

#[derive(Debug, Clone, Copy)]
pub enum RuntimeTerminalEvidenceLookup<'a> {
    EvidenceId(&'a str),
    AttemptId(&'a str),
}

struct StoredRuntimeTerminalEvidence {
    evidence_id: String,
    attempt_id: String,
    session_id: String,
    run_id: String,
    binding_id: String,
    binding_digest: String,
    runtime_id: String,
    descriptor_digest: String,
    producer_key_id: String,
    evidence_digest: String,
    classification: String,
    produced_at: String,
    received_at: String,
    canonical_json: String,
}

struct DurableRuntimeTerminalEvidenceBinding {
    run_id: String,
    run_automation_id: String,
    run_automation_revision: i64,
    run_definition_digest: Option<String>,
    run_occurrence_id: Option<String>,
    attempt_id: String,
    attempt_run_id: String,
    attempt_occurrence_id: String,
    attempt_number: i64,
    adoption_key: String,
    occurrence_fence_generation: i64,
    session_id: Option<String>,
    runtime_id: Option<String>,
    authority_profile: Option<String>,
    extension_json: Option<String>,
}

pub fn store_runtime_terminal_evidence(
    conn: &Connection,
    evidence: &RuntimeTerminalEvidence,
    verifier: &dyn RuntimeTerminalEvidenceVerifier,
) -> Result<RuntimeTerminalEvidenceStoreOutcome, RuntimeTerminalEvidenceError> {
    if !conn.is_autocommit() {
        return Err(evidence_error(
            RuntimeTerminalEvidenceErrorCode::StoreUnavailable,
        ));
    }
    let transaction = rusqlite::Transaction::new_unchecked(conn, TransactionBehavior::Immediate)
        .map_err(|_| evidence_error(RuntimeTerminalEvidenceErrorCode::StoreUnavailable))?;
    let outcome = store_runtime_terminal_evidence_in(&transaction, evidence, verifier)?;
    transaction
        .commit()
        .map_err(|_| evidence_error(RuntimeTerminalEvidenceErrorCode::StoreUnavailable))?;
    Ok(outcome)
}

fn store_runtime_terminal_evidence_in(
    conn: &Connection,
    evidence: &RuntimeTerminalEvidence,
    verifier: &dyn RuntimeTerminalEvidenceVerifier,
) -> Result<RuntimeTerminalEvidenceStoreOutcome, RuntimeTerminalEvidenceError> {
    let binding = pinned_binding(conn, evidence, false)?;
    let verified = verify_runtime_terminal_evidence(evidence.clone(), &binding, verifier)?;
    let canonical = canonical_evidence(&verified.evidence)?;

    let collisions = collision_rows(conn, &verified.evidence)?;
    if !collisions.is_empty() {
        if collisions.len() == 1
            && exact_stored_match(
                &collisions[0],
                &verified.evidence,
                verified.classification,
                &canonical,
            )
        {
            return Ok(RuntimeTerminalEvidenceStoreOutcome::Replayed);
        }
        return Err(evidence_error(RuntimeTerminalEvidenceErrorCode::Conflict));
    }

    conn.execute(
        "INSERT INTO automation_runtime_terminal_evidence (
            evidence_id, attempt_id, session_id, run_id, binding_id,
            binding_digest, runtime_id, descriptor_digest, producer_key_id,
            evidence_digest, classification, produced_at, received_at,
            canonical_json
         ) VALUES (
            ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14
         )",
        params![
            verified.evidence.evidence_id.as_str(),
            verified.evidence.attempt_id.as_str(),
            verified.evidence.session_id.as_str(),
            verified.evidence.run_id.as_str(),
            verified.evidence.binding.binding_id.as_str(),
            verified.evidence.binding.binding_digest.value.as_str(),
            verified.evidence.runtime.runtime_id.as_str(),
            verified.evidence.runtime.descriptor_digest.value.as_str(),
            verified.evidence.authentication.key_id.as_str(),
            verified.evidence.integrity.value.as_str(),
            verified.classification.as_str(),
            verified.evidence.produced_at.as_str(),
            Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
            canonical,
        ],
    )
    .map_err(|error| {
        if matches!(
            error,
            rusqlite::Error::SqliteFailure(ref code, _)
                if code.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_UNIQUE
                    || code.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_PRIMARYKEY
        ) {
            evidence_error(RuntimeTerminalEvidenceErrorCode::Conflict)
        } else {
            evidence_error(RuntimeTerminalEvidenceErrorCode::StoreUnavailable)
        }
    })?;
    Ok(RuntimeTerminalEvidenceStoreOutcome::Stored)
}

pub fn read_verified_runtime_terminal_evidence(
    conn: &Connection,
    lookup: RuntimeTerminalEvidenceLookup<'_>,
    verifier: &dyn RuntimeTerminalEvidenceVerifier,
) -> Result<Option<VerifiedRuntimeTerminalEvidence>, RuntimeTerminalEvidenceError> {
    if conn.is_autocommit() {
        let transaction = conn
            .unchecked_transaction()
            .map_err(|_| evidence_error(RuntimeTerminalEvidenceErrorCode::StoreUnavailable))?;
        let evidence = read_verified_runtime_terminal_evidence_in(&transaction, lookup, verifier)?;
        transaction
            .commit()
            .map_err(|_| evidence_error(RuntimeTerminalEvidenceErrorCode::StoreUnavailable))?;
        return Ok(evidence);
    }
    read_verified_runtime_terminal_evidence_in(conn, lookup, verifier)
}

fn read_verified_runtime_terminal_evidence_in(
    conn: &Connection,
    lookup: RuntimeTerminalEvidenceLookup<'_>,
    verifier: &dyn RuntimeTerminalEvidenceVerifier,
) -> Result<Option<VerifiedRuntimeTerminalEvidence>, RuntimeTerminalEvidenceError> {
    let stored = match lookup {
        RuntimeTerminalEvidenceLookup::EvidenceId(evidence_id) => {
            read_stored(conn, "evidence_id", evidence_id)?
        }
        RuntimeTerminalEvidenceLookup::AttemptId(attempt_id) => {
            read_stored(conn, "attempt_id", attempt_id)?
        }
    };
    let Some(stored) = stored else {
        return Ok(None);
    };
    let evidence: RuntimeTerminalEvidence = serde_json::from_str(&stored.canonical_json)
        .map_err(|_| evidence_error(RuntimeTerminalEvidenceErrorCode::StoredEvidenceInvalid))?;
    let canonical = canonical_evidence(&evidence)
        .map_err(|_| evidence_error(RuntimeTerminalEvidenceErrorCode::StoredEvidenceInvalid))?;
    if !exact_stored_match(
        &stored,
        &evidence,
        classification_from_str(&stored.classification)?,
        &canonical,
    ) {
        return Err(evidence_error(
            RuntimeTerminalEvidenceErrorCode::StoredEvidenceInvalid,
        ));
    }
    let binding = pinned_binding(conn, &evidence, true)?;
    let verified = verify_runtime_terminal_evidence(evidence, &binding, verifier)?;
    if verified.classification.as_str() != stored.classification {
        return Err(evidence_error(
            RuntimeTerminalEvidenceErrorCode::StoredEvidenceInvalid,
        ));
    }
    Ok(Some(verified))
}

fn read_stored(
    conn: &Connection,
    column: &str,
    value: &str,
) -> Result<Option<StoredRuntimeTerminalEvidence>, RuntimeTerminalEvidenceError> {
    let sql = format!(
        "SELECT evidence_id, attempt_id, session_id, run_id, binding_id,
                binding_digest, runtime_id, descriptor_digest, producer_key_id,
                evidence_digest, classification, produced_at, received_at,
                canonical_json
         FROM automation_runtime_terminal_evidence WHERE {column} = ?1"
    );
    conn.query_row(&sql, [value], |row| {
        Ok(StoredRuntimeTerminalEvidence {
            evidence_id: row.get(0)?,
            attempt_id: row.get(1)?,
            session_id: row.get(2)?,
            run_id: row.get(3)?,
            binding_id: row.get(4)?,
            binding_digest: row.get(5)?,
            runtime_id: row.get(6)?,
            descriptor_digest: row.get(7)?,
            producer_key_id: row.get(8)?,
            evidence_digest: row.get(9)?,
            classification: row.get(10)?,
            produced_at: row.get(11)?,
            received_at: row.get(12)?,
            canonical_json: row.get(13)?,
        })
    })
    .optional()
    .map_err(|_| evidence_error(RuntimeTerminalEvidenceErrorCode::StoreUnavailable))
}

fn collision_rows(
    conn: &Connection,
    evidence: &RuntimeTerminalEvidence,
) -> Result<Vec<StoredRuntimeTerminalEvidence>, RuntimeTerminalEvidenceError> {
    let rows = conn
        .prepare(
            "SELECT evidence_id, attempt_id, session_id, run_id, binding_id,
                    binding_digest, runtime_id, descriptor_digest, producer_key_id,
                    evidence_digest, classification, produced_at, received_at,
                    canonical_json
             FROM automation_runtime_terminal_evidence
             WHERE evidence_id = ?1
                OR attempt_id = ?2
                OR session_id = ?3
                OR binding_id = ?4
                OR binding_digest = ?5
             LIMIT 2",
        )
        .and_then(|mut statement| {
            statement
                .query_map(
                    params![
                        evidence.evidence_id.as_str(),
                        evidence.attempt_id.as_str(),
                        evidence.session_id.as_str(),
                        evidence.binding.binding_id.as_str(),
                        evidence.binding.binding_digest.value.as_str(),
                    ],
                    |row| {
                        Ok(StoredRuntimeTerminalEvidence {
                            evidence_id: row.get(0)?,
                            attempt_id: row.get(1)?,
                            session_id: row.get(2)?,
                            run_id: row.get(3)?,
                            binding_id: row.get(4)?,
                            binding_digest: row.get(5)?,
                            runtime_id: row.get(6)?,
                            descriptor_digest: row.get(7)?,
                            producer_key_id: row.get(8)?,
                            evidence_digest: row.get(9)?,
                            classification: row.get(10)?,
                            produced_at: row.get(11)?,
                            received_at: row.get(12)?,
                            canonical_json: row.get(13)?,
                        })
                    },
                )?
                .collect::<rusqlite::Result<Vec<_>>>()
        })
        .map_err(|_| evidence_error(RuntimeTerminalEvidenceErrorCode::StoreUnavailable))?;
    Ok(rows)
}

fn exact_stored_match(
    stored: &StoredRuntimeTerminalEvidence,
    evidence: &RuntimeTerminalEvidence,
    classification: RuntimeTerminalEvidenceClassification,
    canonical: &str,
) -> bool {
    stored.evidence_id == evidence.evidence_id.as_str()
        && stored.attempt_id == evidence.attempt_id.as_str()
        && stored.session_id == evidence.session_id.as_str()
        && stored.run_id == evidence.run_id.as_str()
        && stored.binding_id == evidence.binding.binding_id.as_str()
        && stored.binding_digest == evidence.binding.binding_digest.value.as_str()
        && stored.runtime_id == evidence.runtime.runtime_id.as_str()
        && stored.descriptor_digest == evidence.runtime.descriptor_digest.value.as_str()
        && stored.producer_key_id == evidence.authentication.key_id.as_str()
        && stored.evidence_digest == evidence.integrity.value.as_str()
        && stored.classification == classification.as_str()
        && stored.produced_at == evidence.produced_at.as_str()
        && chrono::DateTime::parse_from_rfc3339(&stored.received_at).is_ok()
        && stored.canonical_json == canonical
}

fn pinned_binding(
    conn: &Connection,
    evidence: &RuntimeTerminalEvidence,
    stored_read: bool,
) -> Result<AutomationExecutionBinding, RuntimeTerminalEvidenceError> {
    let durable: Option<DurableRuntimeTerminalEvidenceBinding> = conn
        .query_row(
            "SELECT run.id, run.automation_id, run.automation_revision,
                    run.definition_digest, run.occurrence_id,
                    attempt.id, attempt.run_id, attempt.occurrence_id,
                    attempt.attempt_number, attempt.adoption_key,
                    attempt.occurrence_fence_generation, attempt.session_id,
                    run.runtime, run.authority_profile, attempt.authority_extension_json
             FROM automation_attempts AS attempt
             JOIN automation_runs AS run ON run.id = attempt.run_id
             JOIN sessions AS session ON session.id = attempt.session_id
             WHERE attempt.id = ?1",
            [evidence.attempt_id.as_str()],
            |row| {
                Ok(DurableRuntimeTerminalEvidenceBinding {
                    run_id: row.get(0)?,
                    run_automation_id: row.get(1)?,
                    run_automation_revision: row.get(2)?,
                    run_definition_digest: row.get(3)?,
                    run_occurrence_id: row.get(4)?,
                    attempt_id: row.get(5)?,
                    attempt_run_id: row.get(6)?,
                    attempt_occurrence_id: row.get(7)?,
                    attempt_number: row.get(8)?,
                    adoption_key: row.get(9)?,
                    occurrence_fence_generation: row.get(10)?,
                    session_id: row.get(11)?,
                    runtime_id: row.get(12)?,
                    authority_profile: row.get(13)?,
                    extension_json: row.get(14)?,
                })
            },
        )
        .optional()
        .map_err(|_| evidence_error(RuntimeTerminalEvidenceErrorCode::StoreUnavailable))?;
    let Some(durable) = durable else {
        return Err(correlation_error(stored_read));
    };
    if durable.authority_profile.as_deref() != Some(AUTHORITY_PROFILE) {
        return Err(evidence_error(if stored_read {
            RuntimeTerminalEvidenceErrorCode::StoredEvidenceInvalid
        } else {
            RuntimeTerminalEvidenceErrorCode::AuthorityProfileRequired
        }));
    }
    let extension_json = durable
        .extension_json
        .as_deref()
        .ok_or_else(|| correlation_error(stored_read))?;
    let extension: Value = serde_json::from_str(extension_json)
        .map_err(|_| evidence_error(RuntimeTerminalEvidenceErrorCode::StoredEvidenceInvalid))?;
    let extensions = ExtensionBag::new(BTreeMap::from([(
        AUTHORITY_EXTENSION_KEY.to_string(),
        extension,
    )]))
    .map_err(|_| evidence_error(RuntimeTerminalEvidenceErrorCode::StoredEvidenceInvalid))?;
    let disposition = validate_authority_profile_structure(
        &extensions,
        AuthorityConsumerClass::RuntimeAuthorityV1,
        &[BASE_PROFILE, AUTHORITY_PROFILE],
        &[RUNTIME_AUTHORITY_CAPABILITY],
        AuthorityValidationPhase::PreDispatch,
    )
    .map_err(|_| evidence_error(RuntimeTerminalEvidenceErrorCode::StoredEvidenceInvalid))?;
    let AuthorityProfileDisposition::Validated(extension) = disposition else {
        return Err(evidence_error(
            RuntimeTerminalEvidenceErrorCode::StoredEvidenceInvalid,
        ));
    };
    validate_durable_correlation(
        &durable,
        &extension.execution_binding,
        evidence,
        stored_read,
    )?;
    Ok(extension.execution_binding)
}

fn validate_durable_correlation(
    durable: &DurableRuntimeTerminalEvidenceBinding,
    binding: &AutomationExecutionBinding,
    evidence: &RuntimeTerminalEvidence,
    stored_read: bool,
) -> Result<(), RuntimeTerminalEvidenceError> {
    let base = &binding.base;
    if durable.run_id != base.run_id.as_str()
        || durable.run_automation_id != base.automation_id.as_str()
        || u64::try_from(durable.run_automation_revision).ok()
            != Some(base.automation_revision.get())
        || durable.run_definition_digest.as_deref() != Some(base.definition_digest.value.as_str())
        || durable.run_occurrence_id.as_deref() != Some(base.occurrence_id.as_str())
        || durable.attempt_id != base.attempt_id.as_str()
        || durable.attempt_run_id != base.run_id.as_str()
        || durable.attempt_occurrence_id != base.occurrence_id.as_str()
        || u64::try_from(durable.attempt_number).ok() != Some(base.attempt_number.get())
        || durable.adoption_key != base.adoption_key.as_str()
        || u64::try_from(durable.occurrence_fence_generation).ok()
            != Some(base.occurrence_fence_generation.get())
        || durable.session_id.as_deref() != Some(evidence.session_id.as_str())
        || durable.runtime_id.as_deref() != Some(binding.runtime.runtime_id.as_str())
        || durable.run_id != evidence.run_id.as_str()
        || durable.attempt_id != evidence.attempt_id.as_str()
        || durable.runtime_id.as_deref() != Some(evidence.runtime.runtime_id.as_str())
    {
        return Err(correlation_error(stored_read));
    }
    Ok(())
}

fn migrated_classification(
    conn: &Connection,
    stored: &StoredRuntimeTerminalEvidence,
) -> AnyhowResult<RuntimeTerminalEvidenceClassification> {
    let invalid = || anyhow::anyhow!("legacy automation runtime terminal evidence is invalid");
    let evidence: RuntimeTerminalEvidence =
        serde_json::from_str(&stored.canonical_json).map_err(|_| invalid())?;
    let canonical = canonical_evidence(&evidence).map_err(|_| invalid())?;
    let stored_classification =
        classification_from_str(&stored.classification).map_err(|_| invalid())?;
    anyhow::ensure!(
        exact_stored_match(stored, &evidence, stored_classification, &canonical),
        "legacy automation runtime terminal evidence is invalid"
    );
    if stored_classification
        != RuntimeTerminalEvidenceClassification::AuthenticatedPartialOrAmbiguous
    {
        return Ok(stored_classification);
    }
    let binding = pinned_binding(conn, &evidence, true).map_err(|_| invalid())?;
    Ok(classify_runtime_terminal_evidence(&evidence, &binding))
}

fn canonical_evidence(
    evidence: &RuntimeTerminalEvidence,
) -> Result<String, RuntimeTerminalEvidenceError> {
    let canonical = canonicalize(evidence)
        .map_err(|_| evidence_error(RuntimeTerminalEvidenceErrorCode::IjsonInvalid))?;
    String::from_utf8(canonical)
        .map_err(|_| evidence_error(RuntimeTerminalEvidenceErrorCode::IjsonInvalid))
}

fn classification_from_str(
    value: &str,
) -> Result<RuntimeTerminalEvidenceClassification, RuntimeTerminalEvidenceError> {
    match value {
        "receipt_eligible_complete" => {
            Ok(RuntimeTerminalEvidenceClassification::ReceiptEligibleComplete)
        }
        "authenticated_partial_or_ambiguous" => {
            Ok(RuntimeTerminalEvidenceClassification::AuthenticatedPartialOrAmbiguous)
        }
        "authenticated_unknown" => Ok(RuntimeTerminalEvidenceClassification::AuthenticatedUnknown),
        "policy_violating" => Ok(RuntimeTerminalEvidenceClassification::PolicyViolating),
        _ => Err(evidence_error(
            RuntimeTerminalEvidenceErrorCode::StoredEvidenceInvalid,
        )),
    }
}

fn correlation_error(stored_read: bool) -> RuntimeTerminalEvidenceError {
    evidence_error(if stored_read {
        RuntimeTerminalEvidenceErrorCode::StoredEvidenceInvalid
    } else {
        RuntimeTerminalEvidenceErrorCode::CorrelationMismatch
    })
}

const fn evidence_error(code: RuntimeTerminalEvidenceErrorCode) -> RuntimeTerminalEvidenceError {
    RuntimeTerminalEvidenceError::new(code)
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;
    use rusqlite::{params, Connection};
    use serde_json::{json, Value};

    use super::{
        canonical_evidence, ensure_runtime_terminal_evidence_schema,
        read_verified_runtime_terminal_evidence, store_runtime_terminal_evidence,
        RuntimeTerminalEvidenceLookup, RuntimeTerminalEvidenceStoreOutcome,
        AUTOMATION_RUNTIME_TERMINAL_EVIDENCE_SCHEMA_SQL,
    };
    use crate::automations::contract::authority::test_support::{fixture, resign_binding};
    use crate::automations::contract::canonical_json::{canonicalize, sha256_hex};
    use crate::automations::contract::runtime_terminal_evidence::{
        RuntimeTerminalEvidence, RuntimeTerminalEvidenceClassification,
        RuntimeTerminalEvidenceError, RuntimeTerminalEvidenceErrorCode,
        RuntimeTerminalEvidenceVerifier, RUNTIME_TERMINAL_EVIDENCE_AUTHENTICATION_DOMAIN,
        RUNTIME_TERMINAL_EVIDENCE_PROFILE,
    };
    use crate::store::initialize_store;

    struct FixtureVerifier {
        key_id: &'static str,
        stale: bool,
    }

    impl RuntimeTerminalEvidenceVerifier for FixtureVerifier {
        fn verify(
            &self,
            evidence: &RuntimeTerminalEvidence,
        ) -> Result<(), RuntimeTerminalEvidenceError> {
            if self.stale {
                return Err(RuntimeTerminalEvidenceError::new(
                    RuntimeTerminalEvidenceErrorCode::AuthenticationStale,
                ));
            }
            if evidence.authentication.key_id.as_str() != self.key_id
                || evidence.authentication.signature.as_str()
                    != format!(
                        "{}{}",
                        evidence.authentication.signed_digest.value.as_str(),
                        evidence.authentication.signed_digest.value.as_str()
                    )
            {
                return Err(RuntimeTerminalEvidenceError::new(
                    RuntimeTerminalEvidenceErrorCode::AuthenticationInvalid,
                ));
            }
            Ok(())
        }
    }

    fn verifier() -> FixtureVerifier {
        FixtureVerifier {
            key_id: "key:runtime-instance-1",
            stale: false,
        }
    }

    fn digest(value: &str) -> Value {
        json!({
            "algorithm": "sha256",
            "canonicalization": "jcs-rfc8785",
            "value": value
        })
    }

    fn seal(mut value: Value) -> Value {
        let object = value.as_object_mut().unwrap();
        object.remove("integrity");
        object.remove("authentication");
        let canonical = canonicalize(&value).unwrap();
        let integrity = sha256_hex(&canonical);
        let mut authentication_preimage = Vec::new();
        authentication_preimage.extend_from_slice(RUNTIME_TERMINAL_EVIDENCE_AUTHENTICATION_DOMAIN);
        authentication_preimage.push(0);
        authentication_preimage.extend_from_slice(&canonical);
        let signed_digest = sha256_hex(&authentication_preimage);
        value["integrity"] = digest(&integrity);
        value["authentication"] = json!({
            "method": "ed25519",
            "keyId": "key:runtime-instance-1",
            "proofRef": "proof:runtime-instance-1",
            "signedDigest": digest(&signed_digest),
            "signature": format!("{signed_digest}{signed_digest}")
        });
        value
    }

    fn evidence_value(binding: &Value, session_id: &str, evidence_id: &str) -> Value {
        json!({
            "profile": RUNTIME_TERMINAL_EVIDENCE_PROFILE,
            "evidenceId": evidence_id,
            "sessionId": session_id,
            "runId": binding["base"]["runId"],
            "attemptId": binding["base"]["attemptId"],
            "binding": {
                "bindingId": binding["bindingId"],
                "bindingDigest": binding["integrity"]
            },
            "runtime": {
                "runtimeId": binding["runtime"]["runtimeId"],
                "descriptorDigest": binding["runtime"]["descriptorDigest"]
            },
            "producedAt": "2026-09-03T12:30:00.000Z",
            "disposition": "succeeded",
            "sideEffects": {
                "state": "observed",
                "maximumClass": "local_write",
                "coverage": "complete"
            },
            "exercisedCapabilities": {
                "state": "observed",
                "values": ["analysis.read", "artifact.write"],
                "coverage": "complete"
            },
            "result": {
                "state": "produced",
                "digest": digest(
                    "8888888888888888888888888888888888888888888888888888888888888888"
                )
            },
            "delivery": {
                "state": "not_attempted"
            },
            "producer": {
                "component": "runtime-adapter",
                "instanceId": "runtime-instance-1",
                "implementationVersion": "1.0.0"
            },
            "privacy": {
                "classification": "operational",
                "retention": {
                    "classification": "standard"
                }
            },
            "integrity": digest(
                "0000000000000000000000000000000000000000000000000000000000000000"
            ),
            "authentication": {
                "method": "ed25519",
                "keyId": "key:runtime-instance-1",
                "proofRef": "proof:runtime-instance-1",
                "signedDigest": digest(
                    "0000000000000000000000000000000000000000000000000000000000000000"
                ),
                "signature": "0".repeat(128)
            }
        })
    }

    struct Fixture {
        _temp: tempfile::TempDir,
        path: std::path::PathBuf,
        conn: Connection,
        binding: Value,
        evidence: RuntimeTerminalEvidence,
    }

    fn seed() -> Fixture {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("runtime-evidence.sqlite");
        initialize_store(&path).unwrap();
        let conn = crate::store::open_initialized_store(&path).unwrap();
        let binding = fixture("binding");
        let extension = json!({
            "profile": "coven.automations.authority.v1",
            "kind": "AutomationAuthorityExtension",
            "executionBinding": binding,
            "receiptEvidence": null
        });
        conn.execute(
            "INSERT INTO sessions (
                id, project_root, harness, title, status, created_at, updated_at
             ) VALUES (
                'session-daily-notes-1', '/work/project', 'runtime:coven-code',
                'Runtime evidence fixture', 'completed',
                '2026-09-03T12:00:00.000Z', '2026-09-03T12:30:00.000Z'
             )",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO automation_occurrences (
                id, automation_id, automation_revision, definition_digest,
                scheduled_for, kind, state, attempt, created_at, updated_at
             ) VALUES (
                'occurrence.daily-notes-20260903', 'daily-notes', 4,
                '1111111111111111111111111111111111111111111111111111111111111111',
                '2026-09-03T12:00:00.000Z', 'scheduled', 'running', 1,
                '2026-09-03T12:00:00.000Z', '2026-09-03T12:00:00.000Z'
             )",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO automation_runs (
                id, automation_id, automation_revision, definition_digest,
                occurrence_id, authority_profile, session_id, familiar_id,
                runtime, status, started_at
             ) VALUES (
                'run.daily-notes-1', 'daily-notes', 4,
                '1111111111111111111111111111111111111111111111111111111111111111',
                'occurrence.daily-notes-20260903', 'coven.automations.authority.v1',
                'session-daily-notes-1', 'charm', 'runtime:coven-code',
                'running', '2026-09-03T12:00:00.000Z'
             )",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO automation_attempts (
                id, run_id, occurrence_id, attempt_number, adoption_key,
                occurrence_fence_generation, dispatch_generation, state,
                retry_classification, authority_extension_json, not_before,
                session_id, opened_at
             ) VALUES (
                'attempt.daily-notes-1-1', 'run.daily-notes-1',
                'occurrence.daily-notes-20260903', 1, 'adopt:daily-notes-1-1',
                7, 1, 'observing', 'initial', ?1,
                '2026-09-03T12:00:00.000Z', 'session-daily-notes-1',
                '2026-09-03T12:00:00.000Z'
             )",
            [serde_json::to_string(&extension).unwrap()],
        )
        .unwrap();
        let evidence = serde_json::from_value(seal(evidence_value(
            &extension["executionBinding"],
            "session-daily-notes-1",
            "evidence:daily-notes-1",
        )))
        .unwrap();
        Fixture {
            _temp: temp,
            path,
            conn,
            binding: extension["executionBinding"].clone(),
            evidence,
        }
    }

    fn replace_evidence(
        fixture: &Fixture,
        mutate: impl FnOnce(&mut Value),
    ) -> RuntimeTerminalEvidence {
        let mut value = evidence_value(
            &fixture.binding,
            "session-daily-notes-1",
            "evidence:daily-notes-1",
        );
        mutate(&mut value);
        serde_json::from_value(seal(value)).unwrap()
    }

    fn replace_inbox_with_legacy_row(
        fixture: &Fixture,
        evidence: &RuntimeTerminalEvidence,
        classification: &str,
        canonical_override: Option<&str>,
    ) {
        fixture
            .conn
            .execute_batch("DROP TABLE automation_runtime_terminal_evidence;")
            .unwrap();
        let legacy_schema = AUTOMATION_RUNTIME_TERMINAL_EVIDENCE_SCHEMA_SQL
            .replace("                'authenticated_unknown',\n", "");
        fixture.conn.execute_batch(&legacy_schema).unwrap();
        fixture
            .conn
            .pragma_update(None, "foreign_keys", false)
            .unwrap();
        let canonical = canonical_override
            .map(str::to_string)
            .unwrap_or_else(|| canonical_evidence(evidence).unwrap());
        fixture
            .conn
            .execute(
                "INSERT INTO automation_runtime_terminal_evidence (
                    evidence_id, attempt_id, session_id, run_id, binding_id,
                    binding_digest, runtime_id, descriptor_digest, producer_key_id,
                    evidence_digest, classification, produced_at, received_at,
                    canonical_json
                 ) VALUES (
                    ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14
                 )",
                params![
                    evidence.evidence_id.as_str(),
                    evidence.attempt_id.as_str(),
                    evidence.session_id.as_str(),
                    evidence.run_id.as_str(),
                    evidence.binding.binding_id.as_str(),
                    evidence.binding.binding_digest.value.as_str(),
                    evidence.runtime.runtime_id.as_str(),
                    evidence.runtime.descriptor_digest.value.as_str(),
                    evidence.authentication.key_id.as_str(),
                    evidence.integrity.value.as_str(),
                    classification,
                    evidence.produced_at.as_str(),
                    "2026-09-03T12:31:00.000Z",
                    canonical,
                ],
            )
            .unwrap();
        fixture
            .conn
            .pragma_update(None, "foreign_keys", true)
            .unwrap();
    }

    fn advance_run_to_second_attempt(fixture: &Fixture, binding_id: &str) -> Value {
        let mut second_binding = fixture.binding.clone();
        second_binding["bindingId"] = json!(binding_id);
        second_binding["base"]["attemptId"] = json!("attempt.daily-notes-1-2");
        second_binding["base"]["attemptNumber"] = json!(2);
        second_binding["base"]["adoptionKey"] = json!("adopt:daily-notes-1-2");
        second_binding["approval"]["consumption"]["attemptNumber"] = json!(2);
        resign_binding(&mut second_binding);
        let extension = json!({
            "profile": "coven.automations.authority.v1",
            "kind": "AutomationAuthorityExtension",
            "executionBinding": second_binding,
            "receiptEvidence": null
        });
        fixture
            .conn
            .execute(
                "UPDATE automation_attempts
                 SET state = 'failed',
                     failure_class = 'runtime_error',
                     state_reason = 'retrying after first attempt',
                     settled_at = '2026-09-03T12:31:00.000Z'
                 WHERE id = 'attempt.daily-notes-1-1'",
                [],
            )
            .unwrap();
        fixture
            .conn
            .execute_batch(
                "INSERT INTO sessions (
                    id, project_root, harness, title, status, created_at, updated_at
                 ) VALUES (
                    'session-daily-notes-2', '/work/project', 'runtime:coven-code',
                    'Second runtime evidence fixture', 'running',
                    '2026-09-03T12:32:00.000Z', '2026-09-03T12:32:00.000Z'
                 );",
            )
            .unwrap();
        fixture
            .conn
            .execute(
                "INSERT INTO automation_attempts (
                    id, run_id, occurrence_id, attempt_number, adoption_key,
                    occurrence_fence_generation, dispatch_generation, state,
                    prior_attempt_number, prior_disposition, retry_classification,
                    authority_extension_json, not_before, session_id, opened_at
                 ) VALUES (
                    'attempt.daily-notes-1-2', 'run.daily-notes-1',
                    'occurrence.daily-notes-20260903', 2, 'adopt:daily-notes-1-2',
                    7, 2, 'started', 1, 'failed', 'automatic_retry', ?1,
                    '2026-09-03T12:32:00.000Z', 'session-daily-notes-2',
                    '2026-09-03T12:32:00.000Z'
                 )",
                [serde_json::to_string(&extension).unwrap()],
            )
            .unwrap();
        fixture
            .conn
            .execute(
                "UPDATE automation_runs
                 SET session_id = 'session-daily-notes-2'
                 WHERE id = 'run.daily-notes-1'",
                [],
            )
            .unwrap();
        extension["executionBinding"].clone()
    }

    fn schema_objects(conn: &Connection) -> Vec<(String, String)> {
        conn.prepare("SELECT name, sql FROM sqlite_master WHERE sql IS NOT NULL ORDER BY name")
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    }

    #[test]
    fn schema_late_ddl_failure_leaves_no_new_prefix() {
        for existing in [false, true] {
            for nested in [false, true] {
                let temp = tempfile::tempdir().unwrap();
                let path = temp.path().join("runtime-evidence.sqlite");
                let conn = Connection::open(&path).unwrap();
                if existing {
                    conn.execute_batch(AUTOMATION_RUNTIME_TERMINAL_EVIDENCE_SCHEMA_SQL)
                        .unwrap();
                    conn.execute_batch(
                        "DROP INDEX idx_automation_runtime_terminal_evidence_run;
                         DROP INDEX idx_automation_runtime_terminal_evidence_binding;
                         DROP TRIGGER automation_runtime_terminal_evidence_no_update;
                         DROP TRIGGER automation_runtime_terminal_evidence_no_delete;",
                    )
                    .unwrap();
                }
                // The third DDL statement fails after the table and run index succeed.
                conn.execute_batch(
                    "CREATE TABLE idx_automation_runtime_terminal_evidence_binding (id INTEGER);
                     CREATE TABLE caller_state (id INTEGER);",
                )
                .unwrap();
                let before = schema_objects(&conn);
                if nested {
                    conn.execute_batch("BEGIN; INSERT INTO caller_state VALUES (1);")
                        .unwrap();
                }

                let error = ensure_runtime_terminal_evidence_schema(&conn).unwrap_err();
                assert!(
                    format!("{error:#}").contains(
                        "there is already a table named idx_automation_runtime_terminal_evidence_binding"
                    ),
                    "{error:#}"
                );
                assert_eq!(conn.is_autocommit(), !nested);
                assert_eq!(
                    schema_objects(&conn),
                    before,
                    "existing={existing}, nested={nested}"
                );
                if nested {
                    assert_eq!(
                        conn.query_row("SELECT COUNT(*) FROM caller_state", [], |row| row
                            .get::<_, i64>(0))
                            .unwrap(),
                        1
                    );
                    conn.execute_batch("COMMIT").unwrap();
                }
                drop(conn);
                assert_eq!(schema_objects(&Connection::open(&path).unwrap()), before);
            }
        }
    }

    #[test]
    fn schema_success_commits_all_five_objects_without_changing_sql() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("runtime-evidence.sqlite");
        let conn = Connection::open(&path).unwrap();
        let reference = Connection::open_in_memory().unwrap();
        reference
            .execute_batch(AUTOMATION_RUNTIME_TERMINAL_EVIDENCE_SCHEMA_SQL)
            .unwrap();
        let expected = schema_objects(&reference);
        assert_eq!(expected.len(), 5);

        for _ in 0..2 {
            ensure_runtime_terminal_evidence_schema(&conn).unwrap();
            assert!(conn.is_autocommit());
            assert_eq!(schema_objects(&conn), expected);
            assert_eq!(schema_objects(&Connection::open(&path).unwrap()), expected);
        }
    }

    #[test]
    fn schema_commit_failure_rolls_back_and_allows_retry() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("runtime-evidence.sqlite");
        let conn = Connection::open(&path).unwrap();
        conn.busy_timeout(std::time::Duration::ZERO).unwrap();
        conn.execute_batch("CREATE TABLE caller_state (id INTEGER);")
            .unwrap();
        let before = schema_objects(&conn);
        let reader = Connection::open(&path).unwrap();
        // A held rollback-journal reader allows DDL but prevents the final commit.
        reader
            .execute_batch("BEGIN; SELECT * FROM caller_state;")
            .unwrap();

        let error = ensure_runtime_terminal_evidence_schema(&conn).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("failed to release runtime evidence schema savepoint"),
            "{error:#}"
        );
        assert!(matches!(
            error.downcast_ref::<rusqlite::Error>(),
            Some(rusqlite::Error::SqliteFailure(code, _))
                if code.code == rusqlite::ErrorCode::DatabaseBusy
        ));
        assert!(conn.is_autocommit());
        assert_eq!(schema_objects(&conn), before);
        reader.execute_batch("ROLLBACK").unwrap();
        assert_eq!(schema_objects(&reader), before);

        ensure_runtime_terminal_evidence_schema(&conn).unwrap();
        assert!(conn.is_autocommit());
        assert_eq!(schema_objects(&reader).len(), before.len() + 5);
    }

    #[test]
    fn schema_success_preserves_caller_transaction_ownership() {
        for commit in [false, true] {
            let conn = Connection::open_in_memory().unwrap();
            conn.execute_batch("CREATE TABLE caller_state (id INTEGER); BEGIN; INSERT INTO caller_state VALUES (1);")
                .unwrap();
            ensure_runtime_terminal_evidence_schema(&conn).unwrap();
            ensure_runtime_terminal_evidence_schema(&conn).unwrap();
            assert!(!conn.is_autocommit());
            assert_eq!(schema_objects(&conn).len(), 6);
            conn.execute_batch(if commit { "COMMIT" } else { "ROLLBACK" })
                .unwrap();
            assert_eq!(schema_objects(&conn).len(), if commit { 6 } else { 1 });
            assert_eq!(
                conn.query_row("SELECT COUNT(*) FROM caller_state", [], |row| row
                    .get::<_, i64>(0))
                    .unwrap(),
                i64::from(commit)
            );
        }
    }

    #[test]
    fn initialized_store_contains_the_immutable_runtime_terminal_evidence_schema() {
        let fixture = seed();
        assert!(AUTOMATION_RUNTIME_TERMINAL_EVIDENCE_SCHEMA_SQL
            .contains("automation_runtime_terminal_evidence"));
        let table_sql: String = fixture
            .conn
            .query_row(
                "SELECT sql FROM sqlite_master
                 WHERE type = 'table' AND name = 'automation_runtime_terminal_evidence'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(table_sql.contains("attempt_id TEXT UNIQUE NOT NULL"));
        assert!(table_sql.contains("session_id TEXT UNIQUE NOT NULL"));
        assert!(table_sql.contains("binding_digest TEXT UNIQUE NOT NULL"));
        assert!(!table_sql.contains("run_id TEXT UNIQUE"));
        for trigger in [
            "automation_runtime_terminal_evidence_no_update",
            "automation_runtime_terminal_evidence_no_delete",
        ] {
            let count: i64 = fixture
                .conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master
                     WHERE type = 'trigger' AND name = ?1",
                    [trigger],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(count, 1, "{trigger}");
        }
    }

    #[test]
    fn complete_evidence_is_stored_replayed_read_and_survives_reopen() {
        let fixture = seed();
        assert_eq!(
            store_runtime_terminal_evidence(&fixture.conn, &fixture.evidence, &verifier()).unwrap(),
            RuntimeTerminalEvidenceStoreOutcome::Stored
        );
        assert_eq!(
            store_runtime_terminal_evidence(&fixture.conn, &fixture.evidence, &verifier()).unwrap(),
            RuntimeTerminalEvidenceStoreOutcome::Replayed
        );
        let by_id = read_verified_runtime_terminal_evidence(
            &fixture.conn,
            RuntimeTerminalEvidenceLookup::EvidenceId("evidence:daily-notes-1"),
            &verifier(),
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            by_id.classification,
            RuntimeTerminalEvidenceClassification::ReceiptEligibleComplete
        );
        let by_attempt = read_verified_runtime_terminal_evidence(
            &fixture.conn,
            RuntimeTerminalEvidenceLookup::AttemptId("attempt.daily-notes-1-1"),
            &verifier(),
        )
        .unwrap()
        .unwrap();
        assert_eq!(by_attempt, by_id);

        assert!(fixture
            .conn
            .execute(
                "UPDATE automation_runtime_terminal_evidence
                 SET received_at = '2026-09-03T13:00:00.000Z'",
                [],
            )
            .is_err());
        assert!(fixture
            .conn
            .execute("DELETE FROM automation_runtime_terminal_evidence", [])
            .is_err());
        drop(fixture.conn);
        let reopened = crate::store::open_initialized_store(&fixture.path).unwrap();
        assert!(read_verified_runtime_terminal_evidence(
            &reopened,
            RuntimeTerminalEvidenceLookup::AttemptId("attempt.daily-notes-1-1"),
            &verifier(),
        )
        .unwrap()
        .is_some());
    }

    #[test]
    fn store_requires_its_own_immediate_transaction_for_replay_safety() {
        let fixture = seed();
        let transaction = fixture.conn.unchecked_transaction().unwrap();

        let error = store_runtime_terminal_evidence(&transaction, &fixture.evidence, &verifier())
            .unwrap_err();

        assert_eq!(
            error.code(),
            RuntimeTerminalEvidenceErrorCode::StoreUnavailable
        );
        let count: i64 = transaction
            .query_row(
                "SELECT COUNT(*) FROM automation_runtime_terminal_evidence",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn existing_inbox_schema_is_upgraded_for_unknown_and_unique_binding_ids() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("legacy-runtime-evidence.sqlite");
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE automation_runtime_terminal_evidence (
                evidence_id TEXT PRIMARY KEY NOT NULL,
                attempt_id TEXT UNIQUE NOT NULL,
                session_id TEXT UNIQUE NOT NULL,
                run_id TEXT NOT NULL,
                binding_id TEXT NOT NULL,
                binding_digest TEXT UNIQUE NOT NULL,
                runtime_id TEXT NOT NULL,
                descriptor_digest TEXT NOT NULL,
                producer_key_id TEXT NOT NULL,
                evidence_digest TEXT NOT NULL,
                classification TEXT NOT NULL CHECK (
                    classification IN (
                        'receipt_eligible_complete',
                        'authenticated_partial_or_ambiguous',
                        'policy_violating'
                    )
                ),
                produced_at TEXT NOT NULL,
                received_at TEXT NOT NULL,
                canonical_json TEXT NOT NULL CHECK (json_valid(canonical_json))
            );",
        )
        .unwrap();
        drop(conn);

        initialize_store(&path).unwrap();
        let reopened = crate::store::open_initialized_store(&path).unwrap();
        let table_sql: String = reopened
            .query_row(
                "SELECT sql FROM sqlite_master
                 WHERE type = 'table' AND name = 'automation_runtime_terminal_evidence'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(table_sql.contains("'authenticated_unknown'"));
        let binding_index: (String, i64) = reopened
            .query_row(
                "SELECT name, [unique]
                 FROM pragma_index_list('automation_runtime_terminal_evidence')
                 WHERE name = 'idx_automation_runtime_terminal_evidence_binding'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            binding_index,
            (
                "idx_automation_runtime_terminal_evidence_binding".to_string(),
                1
            )
        );
    }

    #[test]
    fn migration_reclassifies_legacy_unknown_and_preserves_partial_and_policy_precedence() {
        for (name, mutate, legacy_classification, expected) in [
            (
                "unknown",
                (|value: &mut Value| {
                    value["delivery"] = json!({
                        "state": "unknown",
                        "reasonCode": "runtime_delivery_unavailable"
                    });
                }) as fn(&mut Value),
                "authenticated_partial_or_ambiguous",
                RuntimeTerminalEvidenceClassification::AuthenticatedUnknown,
            ),
            (
                "partial",
                (|value: &mut Value| value["sideEffects"]["coverage"] = json!("partial"))
                    as fn(&mut Value),
                "authenticated_partial_or_ambiguous",
                RuntimeTerminalEvidenceClassification::AuthenticatedPartialOrAmbiguous,
            ),
            (
                "policy-violating-unknown",
                (|value: &mut Value| {
                    value["sideEffects"] = json!({
                        "state": "unknown",
                        "reasonCode": "runtime_observation_unavailable"
                    });
                    value["exercisedCapabilities"]["values"] =
                        json!(["analysis.read", "artifact.write", "network.publish"]);
                }) as fn(&mut Value),
                "policy_violating",
                RuntimeTerminalEvidenceClassification::PolicyViolating,
            ),
        ] {
            let fixture = seed();
            let evidence = replace_evidence(&fixture, mutate);
            replace_inbox_with_legacy_row(&fixture, &evidence, legacy_classification, None);
            drop(fixture.conn);

            initialize_store(&fixture.path).unwrap();
            let reopened = crate::store::open_initialized_store(&fixture.path).unwrap();
            let persisted: String = reopened
                .query_row(
                    "SELECT classification
                     FROM automation_runtime_terminal_evidence
                     WHERE evidence_id = 'evidence:daily-notes-1'",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(persisted, expected.as_str(), "{name}");
            let verified = read_verified_runtime_terminal_evidence(
                &reopened,
                RuntimeTerminalEvidenceLookup::EvidenceId("evidence:daily-notes-1"),
                &verifier(),
            )
            .unwrap()
            .unwrap();
            assert_eq!(verified.evidence, evidence, "{name}");
            assert_eq!(verified.classification, expected, "{name}");
            assert_eq!(
                store_runtime_terminal_evidence(&reopened, &evidence, &verifier()).unwrap(),
                RuntimeTerminalEvidenceStoreOutcome::Replayed,
                "{name}"
            );
        }
    }

    #[test]
    fn malformed_legacy_evidence_rolls_back_without_disclosure() {
        let fixture = seed();
        replace_inbox_with_legacy_row(
            &fixture,
            &fixture.evidence,
            "authenticated_partial_or_ambiguous",
            Some(r#"{"private":"migration-secret"}"#),
        );
        drop(fixture.conn);

        let error = initialize_store(&fixture.path)
            .expect_err("malformed legacy evidence must abort initialization");
        let chain = format!("{error:#}");
        assert!(chain.contains("legacy automation runtime terminal evidence is invalid"));
        assert!(!chain.contains("migration-secret"));

        let reopened = Connection::open(&fixture.path).unwrap();
        let table_sql: String = reopened
            .query_row(
                "SELECT sql FROM sqlite_master
                 WHERE type = 'table' AND name = 'automation_runtime_terminal_evidence'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(!table_sql.contains("'authenticated_unknown'"));
        let preserved: (String, String) = reopened
            .query_row(
                "SELECT classification, canonical_json
                 FROM automation_runtime_terminal_evidence",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            preserved,
            (
                "authenticated_partial_or_ambiguous".to_string(),
                r#"{"private":"migration-secret"}"#.to_string(),
            )
        );
        let legacy_table_count: i64 = reopened
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master
                 WHERE type = 'table'
                   AND name = 'automation_runtime_terminal_evidence_legacy'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(legacy_table_count, 0);
    }

    #[test]
    fn orphaned_legacy_evidence_rolls_back_the_schema_and_data() {
        let fixture = seed();
        let orphan: RuntimeTerminalEvidence = serde_json::from_value(seal(json!({
            "profile": RUNTIME_TERMINAL_EVIDENCE_PROFILE,
            "evidenceId": "evidence:private-orphan",
            "sessionId": "session-private-orphan",
            "runId": "run.private-orphan",
            "attemptId": "attempt.private-orphan",
            "binding": {
                "bindingId": fixture.binding["bindingId"],
                "bindingDigest": fixture.binding["integrity"]
            },
            "runtime": {
                "runtimeId": fixture.binding["runtime"]["runtimeId"],
                "descriptorDigest": fixture.binding["runtime"]["descriptorDigest"]
            },
            "producedAt": "2026-09-03T12:30:00.000Z",
            "disposition": "succeeded",
            "sideEffects": {
                "state": "observed",
                "maximumClass": "local_write",
                "coverage": "complete"
            },
            "exercisedCapabilities": {
                "state": "observed",
                "values": ["analysis.read", "artifact.write"],
                "coverage": "complete"
            },
            "result": {
                "state": "not_produced"
            },
            "delivery": {
                "state": "not_attempted"
            },
            "producer": {
                "component": "runtime-adapter",
                "instanceId": "runtime-instance-1",
                "implementationVersion": "1.0.0"
            },
            "privacy": {
                "classification": "operational",
                "retention": {
                    "classification": "standard"
                }
            },
            "integrity": digest(
                "0000000000000000000000000000000000000000000000000000000000000000"
            ),
            "authentication": {
                "method": "ed25519",
                "keyId": "key:runtime-instance-1",
                "proofRef": "proof:runtime-instance-1",
                "signedDigest": digest(
                    "0000000000000000000000000000000000000000000000000000000000000000"
                ),
                "signature": "0".repeat(128)
            }
        })))
        .unwrap();
        replace_inbox_with_legacy_row(&fixture, &orphan, "receipt_eligible_complete", None);
        drop(fixture.conn);

        let error =
            initialize_store(&fixture.path).expect_err("orphaned legacy evidence must fail");
        let chain = format!("{error:#}");
        assert!(chain.contains("foreign-key violation"));
        assert!(!chain.contains("private-orphan"));

        let reopened = Connection::open(&fixture.path).unwrap();
        let table_sql: String = reopened
            .query_row(
                "SELECT sql FROM sqlite_master
                 WHERE type = 'table' AND name = 'automation_runtime_terminal_evidence'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(!table_sql.contains("'authenticated_unknown'"));
        let preserved: (String, String, String, String) = reopened
            .query_row(
                "SELECT evidence_id, attempt_id, session_id, run_id
                 FROM automation_runtime_terminal_evidence",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(
            preserved,
            (
                "evidence:private-orphan".to_string(),
                "attempt.private-orphan".to_string(),
                "session-private-orphan".to_string(),
                "run.private-orphan".to_string(),
            )
        );
        let legacy_table_count: i64 = reopened
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master
                 WHERE type = 'table'
                   AND name = 'automation_runtime_terminal_evidence_legacy'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(legacy_table_count, 0);
    }

    #[test]
    fn same_name_non_unique_binding_index_is_upgraded() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("non-unique-binding-index.sqlite");
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE automation_runtime_terminal_evidence (
                evidence_id TEXT PRIMARY KEY NOT NULL,
                attempt_id TEXT UNIQUE NOT NULL,
                session_id TEXT UNIQUE NOT NULL,
                run_id TEXT NOT NULL,
                binding_id TEXT NOT NULL,
                binding_digest TEXT UNIQUE NOT NULL,
                runtime_id TEXT NOT NULL,
                descriptor_digest TEXT NOT NULL,
                producer_key_id TEXT NOT NULL,
                evidence_digest TEXT NOT NULL,
                classification TEXT NOT NULL CHECK (
                    classification IN (
                        'receipt_eligible_complete',
                        'authenticated_partial_or_ambiguous',
                        'authenticated_unknown',
                        'policy_violating'
                    )
                ),
                produced_at TEXT NOT NULL,
                received_at TEXT NOT NULL,
                canonical_json TEXT NOT NULL CHECK (json_valid(canonical_json))
            );
            CREATE INDEX idx_automation_runtime_terminal_evidence_binding
                ON automation_runtime_terminal_evidence(binding_id);",
        )
        .unwrap();
        drop(conn);

        initialize_store(&path).unwrap();
        let reopened = crate::store::open_initialized_store(&path).unwrap();
        let binding_index: i64 = reopened
            .query_row(
                "SELECT [unique]
                 FROM pragma_index_list('automation_runtime_terminal_evidence')
                 WHERE name = 'idx_automation_runtime_terminal_evidence_binding'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(binding_index, 1);
    }

    #[test]
    fn authenticated_partial_unknown_and_policy_violating_evidence_survives_reopen() {
        for (name, mutate, expected) in [
            (
                "partial",
                (|value: &mut Value| value["sideEffects"]["coverage"] = json!("partial"))
                    as fn(&mut Value),
                "authenticated_partial_or_ambiguous",
            ),
            (
                "unknown",
                (|value: &mut Value| {
                    value["sideEffects"] = json!({
                        "state": "unknown",
                        "reasonCode": "runtime_observation_unavailable"
                    });
                    value["exercisedCapabilities"] = json!({
                        "state": "unknown",
                        "reasonCode": "runtime_observation_unavailable"
                    });
                    value["result"] = json!({
                        "state": "unknown",
                        "reasonCode": "runtime_result_unavailable"
                    });
                    value["delivery"] = json!({
                        "state": "unknown",
                        "reasonCode": "runtime_delivery_unavailable"
                    });
                }) as fn(&mut Value),
                "authenticated_unknown",
            ),
            (
                "violation",
                (|value: &mut Value| {
                    value["exercisedCapabilities"]["values"] =
                        json!(["analysis.read", "network.publish"]);
                }) as fn(&mut Value),
                "policy_violating",
            ),
        ] {
            let fixture = seed();
            let evidence = replace_evidence(&fixture, mutate);
            assert_eq!(
                store_runtime_terminal_evidence(&fixture.conn, &evidence, &verifier()).unwrap(),
                RuntimeTerminalEvidenceStoreOutcome::Stored,
                "{name}"
            );
            let persisted: String = fixture
                .conn
                .query_row(
                    "SELECT classification
                     FROM automation_runtime_terminal_evidence
                     WHERE evidence_id = 'evidence:daily-notes-1'",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(persisted, expected, "{name}");
            drop(fixture.conn);
            let reopened = crate::store::open_initialized_store(&fixture.path).unwrap();
            let stored = read_verified_runtime_terminal_evidence(
                &reopened,
                RuntimeTerminalEvidenceLookup::EvidenceId("evidence:daily-notes-1"),
                &verifier(),
            )
            .unwrap()
            .unwrap();
            assert_eq!(stored.classification.as_str(), expected, "{name}");
        }
    }

    #[test]
    fn earlier_attempt_evidence_survives_a_later_session_launch_and_reopen() {
        let fixture = seed();
        store_runtime_terminal_evidence(&fixture.conn, &fixture.evidence, &verifier()).unwrap();
        advance_run_to_second_attempt(&fixture, "binding:daily-notes-1-second");

        drop(fixture.conn);
        let reopened = crate::store::open_initialized_store(&fixture.path).unwrap();
        let stored = read_verified_runtime_terminal_evidence(
            &reopened,
            RuntimeTerminalEvidenceLookup::AttemptId("attempt.daily-notes-1-1"),
            &verifier(),
        )
        .unwrap()
        .unwrap();
        assert_eq!(stored.evidence.session_id.as_str(), "session-daily-notes-1");
        assert_eq!(stored.evidence.run_id.as_str(), "run.daily-notes-1");
    }

    #[test]
    fn binding_id_conflict_is_atomic_while_identical_replay_remains_idempotent() {
        let fixture = seed();
        assert_eq!(
            store_runtime_terminal_evidence(&fixture.conn, &fixture.evidence, &verifier()).unwrap(),
            RuntimeTerminalEvidenceStoreOutcome::Stored
        );
        assert_eq!(
            store_runtime_terminal_evidence(&fixture.conn, &fixture.evidence, &verifier()).unwrap(),
            RuntimeTerminalEvidenceStoreOutcome::Replayed
        );
        let original_binding_id = fixture.evidence.binding.binding_id.as_str().to_string();
        let second_binding = advance_run_to_second_attempt(&fixture, &original_binding_id);
        let second: RuntimeTerminalEvidence = serde_json::from_value(seal(evidence_value(
            &second_binding,
            "session-daily-notes-2",
            "evidence:daily-notes-2",
        )))
        .unwrap();

        let error =
            store_runtime_terminal_evidence(&fixture.conn, &second, &verifier()).unwrap_err();
        assert_eq!(error.code(), RuntimeTerminalEvidenceErrorCode::Conflict);
        let rows: i64 = fixture
            .conn
            .query_row(
                "SELECT COUNT(*) FROM automation_runtime_terminal_evidence",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(rows, 1);
    }

    #[test]
    fn store_rejects_exact_correlation_mismatches_and_unbound_authority_profiles() {
        for mutate in [
            (|value: &mut Value| value["sessionId"] = json!("session-other")) as fn(&mut Value),
            |value: &mut Value| value["runId"] = json!("run.other"),
            |value: &mut Value| value["attemptId"] = json!("attempt.other"),
            |value: &mut Value| value["binding"]["bindingId"] = json!("binding:other"),
            |value: &mut Value| {
                value["binding"]["bindingDigest"]["value"] = json!("1".repeat(64));
            },
            |value: &mut Value| value["runtime"]["runtimeId"] = json!("runtime:other"),
            |value: &mut Value| {
                value["runtime"]["descriptorDigest"]["value"] = json!("2".repeat(64));
            },
        ] {
            let fixture = seed();
            let evidence = replace_evidence(&fixture, mutate);
            let error =
                store_runtime_terminal_evidence(&fixture.conn, &evidence, &verifier()).unwrap_err();
            assert_eq!(
                error.code(),
                RuntimeTerminalEvidenceErrorCode::CorrelationMismatch
            );
        }

        let fixture = seed();
        fixture
            .conn
            .execute_batch(
                "DROP TRIGGER automation_run_authority_profile_immutable;
                 UPDATE automation_runs SET authority_profile = NULL;",
            )
            .unwrap();
        let error = store_runtime_terminal_evidence(&fixture.conn, &fixture.evidence, &verifier())
            .unwrap_err();
        assert_eq!(
            error.code(),
            RuntimeTerminalEvidenceErrorCode::AuthorityProfileRequired
        );
    }

    #[test]
    fn durable_run_and_attempt_identity_tampering_refuses_store_and_read_privacy_safely() {
        for (tamper, private_marker) in [
            (
                "UPDATE automation_runs
                 SET automation_id = 'private-run-automation-id'",
                "private-run-automation-id",
            ),
            (
                "UPDATE automation_runs
                 SET definition_digest =
                     '9999999999999999999999999999999999999999999999999999999999999999'",
                "9999999999999999999999999999999999999999999999999999999999999999",
            ),
            (
                "INSERT INTO automation_occurrences (
                    id, automation_id, automation_revision, definition_digest,
                    scheduled_for, kind, state, attempt, created_at, updated_at
                 ) VALUES (
                    'occurrence.private-other', 'daily-notes', 4,
                    '1111111111111111111111111111111111111111111111111111111111111111',
                    '2026-09-04T12:00:00.000Z', 'scheduled', 'running', 1,
                    '2026-09-04T12:00:00.000Z', '2026-09-04T12:00:00.000Z'
                 );
                 UPDATE automation_attempts
                 SET occurrence_id = 'occurrence.private-other'",
                "occurrence.private-other",
            ),
            (
                "UPDATE automation_attempts
                 SET attempt_number = 2,
                     prior_attempt_number = 1,
                     prior_disposition = 'failed'",
                "attempt_number",
            ),
            (
                "UPDATE automation_attempts
                 SET adoption_key = 'private-attempt-adoption-key'",
                "private-attempt-adoption-key",
            ),
            (
                "UPDATE automation_attempts
                 SET occurrence_fence_generation = 8",
                "occurrence_fence_generation",
            ),
        ] {
            let fixture = seed();
            fixture.conn.execute_batch(tamper).unwrap();
            let error =
                store_runtime_terminal_evidence(&fixture.conn, &fixture.evidence, &verifier())
                    .unwrap_err();
            assert_eq!(
                error.code(),
                RuntimeTerminalEvidenceErrorCode::CorrelationMismatch
            );
            assert!(!format!("{error:#}").contains(private_marker));

            let fixture = seed();
            store_runtime_terminal_evidence(&fixture.conn, &fixture.evidence, &verifier()).unwrap();
            fixture.conn.execute_batch(tamper).unwrap();
            let error = read_verified_runtime_terminal_evidence(
                &fixture.conn,
                RuntimeTerminalEvidenceLookup::EvidenceId("evidence:daily-notes-1"),
                &verifier(),
            )
            .unwrap_err();
            assert_eq!(
                error.code(),
                RuntimeTerminalEvidenceErrorCode::StoredEvidenceInvalid
            );
            assert!(!format!("{error:#}").contains(private_marker));
        }
    }

    #[test]
    fn verifier_refusals_are_privacy_safe_and_never_store_rows() {
        for verifier in [
            FixtureVerifier {
                key_id: "key:wrong",
                stale: false,
            },
            FixtureVerifier {
                key_id: "key:runtime-instance-1",
                stale: true,
            },
        ] {
            let fixture = seed();
            let error =
                store_runtime_terminal_evidence(&fixture.conn, &fixture.evidence, &verifier)
                    .unwrap_err();
            assert!(matches!(
                error.code(),
                RuntimeTerminalEvidenceErrorCode::AuthenticationInvalid
                    | RuntimeTerminalEvidenceErrorCode::AuthenticationStale
            ));
            assert!(!format!("{error:#}").contains("proof:runtime-instance-1"));
            let count: i64 = fixture
                .conn
                .query_row(
                    "SELECT COUNT(*) FROM automation_runtime_terminal_evidence",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(count, 0);
        }
    }

    #[test]
    fn conflicting_replay_is_refused() {
        let fixture = seed();
        assert_eq!(
            store_runtime_terminal_evidence(&fixture.conn, &fixture.evidence, &verifier()).unwrap(),
            RuntimeTerminalEvidenceStoreOutcome::Stored
        );
        let conflicting = replace_evidence(&fixture, |value| {
            value["evidenceId"] = json!("evidence:conflict");
            value["delivery"] = json!({
                "state": "failed",
                "digest": digest(
                    "9999999999999999999999999999999999999999999999999999999999999999"
                )
            });
        });
        let error =
            store_runtime_terminal_evidence(&fixture.conn, &conflicting, &verifier()).unwrap_err();
        assert_eq!(error.code(), RuntimeTerminalEvidenceErrorCode::Conflict);
    }

    #[test]
    fn read_rejects_tampered_json_and_denormalized_indexes_without_disclosure() {
        for tamper in [
            "UPDATE automation_runtime_terminal_evidence
             SET canonical_json = '{\"private\":\"attacker-content\"}'",
            "UPDATE automation_runtime_terminal_evidence
             SET producer_key_id = 'key:tampered-private'",
            "UPDATE automation_runtime_terminal_evidence
             SET received_at = 'not-a-timestamp'",
        ] {
            let fixture = seed();
            store_runtime_terminal_evidence(&fixture.conn, &fixture.evidence, &verifier()).unwrap();
            fixture
                .conn
                .execute_batch(
                    "DROP TRIGGER automation_runtime_terminal_evidence_no_update;
                     DROP TRIGGER automation_runtime_terminal_evidence_no_delete;",
                )
                .unwrap();
            fixture.conn.execute_batch(tamper).unwrap();

            let error = read_verified_runtime_terminal_evidence(
                &fixture.conn,
                RuntimeTerminalEvidenceLookup::EvidenceId("evidence:daily-notes-1"),
                &verifier(),
            )
            .unwrap_err();
            assert_eq!(
                error.code(),
                RuntimeTerminalEvidenceErrorCode::StoredEvidenceInvalid
            );
            let chain = format!("{error:#}");
            assert!(!chain.contains("attacker-content"));
            assert!(!chain.contains("tampered-private"));
        }
    }

    #[test]
    fn missing_evidence_returns_none() {
        let fixture = seed();
        assert_eq!(
            read_verified_runtime_terminal_evidence(
                &fixture.conn,
                RuntimeTerminalEvidenceLookup::EvidenceId("evidence:missing"),
                &verifier(),
            )
            .unwrap(),
            None
        );
    }

    #[test]
    fn run_id_is_indexed_but_all_replay_identity_columns_are_unique() {
        let fixture = seed();
        let indexes: Vec<(String, i64)> = fixture
            .conn
            .prepare("PRAGMA index_list(automation_runtime_terminal_evidence)")
            .unwrap()
            .query_map([], |row| Ok((row.get(1)?, row.get(2)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert!(indexes.iter().any(|(name, unique)| name
            == "idx_automation_runtime_terminal_evidence_run"
            && *unique == 0));
        assert!(indexes.iter().any(|(name, unique)| name
            == "idx_automation_runtime_terminal_evidence_binding"
            && *unique == 1));
        assert!(indexes.iter().filter(|(_, unique)| *unique == 1).count() >= 3);
    }

    #[test]
    fn canonical_replay_conflicts_across_a_second_session_and_attempt() {
        let fixture = seed();
        store_runtime_terminal_evidence(&fixture.conn, &fixture.evidence, &verifier()).unwrap();

        let mut second_binding = fixture.binding.clone();
        second_binding["base"]["occurrenceId"] = json!("occurrence.daily-notes-20260904");
        second_binding["base"]["occurrenceKey"] = json!("daily-notes@2026-09-04T12:00:00.000Z");
        second_binding["base"]["runId"] = json!("run.daily-notes-2");
        second_binding["base"]["attemptId"] = json!("attempt.daily-notes-2-1");
        second_binding["base"]["adoptionKey"] = json!("adopt:daily-notes-2-1");
        second_binding["approval"]["consumption"]["occurrenceId"] =
            json!("occurrence.daily-notes-20260904");
        second_binding["approval"]["consumption"]["runId"] = json!("run.daily-notes-2");
        resign_binding(&mut second_binding);
        let extension = json!({
            "profile": "coven.automations.authority.v1",
            "kind": "AutomationAuthorityExtension",
            "executionBinding": second_binding,
            "receiptEvidence": null
        });
        fixture
            .conn
            .execute_batch(
                "INSERT INTO sessions (
                    id, project_root, harness, title, status, created_at, updated_at
                 ) VALUES (
                    'session-daily-notes-2', '/work/project', 'runtime:coven-code',
                    'Second runtime evidence fixture', 'completed',
                    '2026-09-04T12:00:00.000Z', '2026-09-04T12:30:00.000Z'
                 );
                 INSERT INTO automation_occurrences (
                    id, automation_id, automation_revision, definition_digest,
                    scheduled_for, kind, state, attempt, created_at, updated_at
                 ) VALUES (
                    'occurrence.daily-notes-20260904', 'daily-notes', 4,
                    '1111111111111111111111111111111111111111111111111111111111111111',
                    '2026-09-04T12:00:00.000Z', 'scheduled', 'running', 1,
                    '2026-09-04T12:00:00.000Z', '2026-09-04T12:00:00.000Z'
                 );",
            )
            .unwrap();
        fixture
            .conn
            .execute(
                "INSERT INTO automation_runs (
                    id, automation_id, automation_revision, definition_digest,
                    occurrence_id, authority_profile, session_id, familiar_id,
                    runtime, status, started_at
                 ) VALUES (
                    'run.daily-notes-2', 'daily-notes', 4,
                    '1111111111111111111111111111111111111111111111111111111111111111',
                    'occurrence.daily-notes-20260904', 'coven.automations.authority.v1',
                    'session-daily-notes-2', 'charm', 'runtime:coven-code',
                    'running', '2026-09-04T12:00:00.000Z'
                 )",
                [],
            )
            .unwrap();
        fixture
            .conn
            .execute(
                "INSERT INTO automation_attempts (
                    id, run_id, occurrence_id, attempt_number, adoption_key,
                    occurrence_fence_generation, dispatch_generation, state,
                    retry_classification, authority_extension_json, not_before,
                    session_id, opened_at
                 ) VALUES (
                    'attempt.daily-notes-2-1', 'run.daily-notes-2',
                    'occurrence.daily-notes-20260904', 1, 'adopt:daily-notes-2-1',
                    7, 1, 'observing', 'initial', ?1,
                    '2026-09-04T12:00:00.000Z', 'session-daily-notes-2',
                    '2026-09-04T12:00:00.000Z'
                 )",
                [serde_json::to_string(&extension).unwrap()],
            )
            .unwrap();
        let second: RuntimeTerminalEvidence = serde_json::from_value(seal(evidence_value(
            &extension["executionBinding"],
            "session-daily-notes-2",
            "evidence:daily-notes-1",
        )))
        .unwrap();

        let error =
            store_runtime_terminal_evidence(&fixture.conn, &second, &verifier()).unwrap_err();
        assert_eq!(error.code(), RuntimeTerminalEvidenceErrorCode::Conflict);
    }

    #[test]
    fn received_at_is_coven_owned_and_parseable() {
        let fixture = seed();
        store_runtime_terminal_evidence(&fixture.conn, &fixture.evidence, &verifier()).unwrap();
        let received_at: String = fixture
            .conn
            .query_row(
                "SELECT received_at FROM automation_runtime_terminal_evidence",
                [],
                |row| row.get(0),
            )
            .unwrap();
        chrono::DateTime::parse_from_rfc3339(&received_at).unwrap();
        assert!(
            chrono::Utc.with_ymd_and_hms(2026, 9, 3, 12, 30, 0).unwrap()
                <= chrono::DateTime::parse_from_rfc3339(&received_at)
                    .unwrap()
                    .with_timezone(&chrono::Utc)
        );
    }
}
