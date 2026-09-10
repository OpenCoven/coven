//! Immutable Automations v1 receipt commitment.

// These internal seams remain unwired until a producer can supply terminal
// evidence without deriving side-effect, capability, result or delivery claims.
#![allow(dead_code)]

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::Serialize;
use serde_json::Value;

use super::contract::authority::{
    validate_authority_profile, validate_authority_profile_structure, AuthorityApprovalBinding,
    AuthorityConsumerClass, AuthorityEvidenceVerifier, AuthorityProfileDisposition,
    AuthoritySideEffectClass, AuthorityValidationPhase, AutomationAuthorityExtension,
    AUTHORITY_EXTENSION_KEY, AUTHORITY_PROFILE, BASE_PROFILE, RUNTIME_AUTHORITY_CAPABILITY,
};
use super::contract::events::append_event;
use super::contract::types::{
    AutomationReceipt, EventEnvelope, EventKind, EventPayload, ExtensionBag, SideEffectClass,
    StreamKind, TerminalOutcome,
};

pub const AUTOMATION_RECEIPTS_SCHEMA_SQL: &str = "
    CREATE TABLE IF NOT EXISTS automation_receipts (
        id TEXT PRIMARY KEY NOT NULL,
        run_id TEXT UNIQUE NOT NULL,
        attempt_id TEXT UNIQUE NOT NULL,
        automation_id TEXT NOT NULL,
        automation_revision INTEGER NOT NULL CHECK (automation_revision >= 1),
        definition_digest TEXT,
        occurrence_id TEXT NOT NULL,
        occurrence_fence_generation INTEGER NOT NULL
            CHECK (occurrence_fence_generation >= 1),
        attempt_number INTEGER NOT NULL CHECK (attempt_number BETWEEN 1 AND 10),
        outcome TEXT NOT NULL CHECK (
            outcome IN ('succeeded', 'failed', 'cancelled', 'timed_out', 'ambiguous')
        ),
        receipt_digest TEXT NOT NULL CHECK (
            length(receipt_digest) = 64
            AND receipt_digest NOT GLOB '*[^0-9a-f]*'
        ),
        receipt_json TEXT NOT NULL CHECK (json_valid(receipt_json)),
        event_id TEXT UNIQUE NOT NULL,
        produced_at TEXT NOT NULL,
        FOREIGN KEY (run_id) REFERENCES automation_runs(id) ON DELETE RESTRICT,
        FOREIGN KEY (attempt_id) REFERENCES automation_attempts(id) ON DELETE RESTRICT,
        FOREIGN KEY (occurrence_id) REFERENCES automation_occurrences(id) ON DELETE RESTRICT
    );

    CREATE TRIGGER IF NOT EXISTS automation_receipts_no_update
    BEFORE UPDATE ON automation_receipts
    BEGIN
        SELECT RAISE(ABORT, 'automation receipts are immutable');
    END;

    CREATE TRIGGER IF NOT EXISTS automation_receipts_no_delete
    BEFORE DELETE ON automation_receipts
    BEGIN
        SELECT RAISE(ABORT, 'automation receipts are immutable');
    END;

    CREATE TRIGGER IF NOT EXISTS automation_run_receipt_once
    BEFORE UPDATE OF receipt_id ON automation_runs
    WHEN OLD.receipt_id IS NOT NEW.receipt_id
         AND (
             OLD.receipt_id IS NOT NULL
             OR NEW.receipt_id IS NULL
             OR NOT EXISTS (
                 SELECT 1
                 FROM automation_receipts
                 WHERE id = NEW.receipt_id AND run_id = OLD.id
             )
         )
    BEGIN
        SELECT RAISE(ABORT, 'automation run receipt is immutable');
    END;
";

pub const AUTOMATION_RECEIPT_AUTHORITY_EXTENSIONS_SCHEMA_SQL: &str = "
    CREATE TABLE IF NOT EXISTS automation_receipt_authority_extensions (
        receipt_id TEXT PRIMARY KEY NOT NULL,
        run_id TEXT UNIQUE NOT NULL,
        attempt_id TEXT UNIQUE NOT NULL,
        binding_id TEXT NOT NULL,
        binding_digest TEXT NOT NULL CHECK (
            length(binding_digest) = 64
            AND binding_digest NOT GLOB '*[^0-9a-f]*'
        ),
        base_receipt_digest TEXT NOT NULL CHECK (
            length(base_receipt_digest) = 64
            AND base_receipt_digest NOT GLOB '*[^0-9a-f]*'
        ),
        terminal_evidence_digest TEXT NOT NULL CHECK (
            length(terminal_evidence_digest) = 64
            AND terminal_evidence_digest NOT GLOB '*[^0-9a-f]*'
        ),
        authority_json TEXT NOT NULL CHECK (json_valid(authority_json)),
        produced_at TEXT NOT NULL,
        FOREIGN KEY (receipt_id) REFERENCES automation_receipts(id) ON DELETE RESTRICT,
        FOREIGN KEY (run_id) REFERENCES automation_runs(id) ON DELETE RESTRICT,
        FOREIGN KEY (attempt_id) REFERENCES automation_attempts(id) ON DELETE RESTRICT
    );

    CREATE TRIGGER IF NOT EXISTS automation_receipt_authority_extensions_no_update
    BEFORE UPDATE ON automation_receipt_authority_extensions
    BEGIN
        SELECT RAISE(ABORT, 'automation receipt authority extensions are immutable');
    END;

    CREATE TRIGGER IF NOT EXISTS automation_receipt_authority_extensions_no_delete
    BEFORE DELETE ON automation_receipt_authority_extensions
    BEGIN
        SELECT RAISE(ABORT, 'automation receipt authority extensions are immutable');
    END;
";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReceiptCommitOutcome {
    Committed,
    Replayed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AuthorizedReceipt {
    pub(crate) receipt: AutomationReceipt,
    pub(crate) authority: AutomationAuthorityExtension,
}

struct StoredReceiptEvidence {
    run_id: String,
    attempt_id: String,
    automation_id: String,
    automation_revision: i64,
    definition_digest: Option<String>,
    occurrence_id: String,
    occurrence_fence_generation: i64,
    attempt_number: i64,
    outcome: String,
    receipt_digest: String,
    receipt_json: String,
    event_id: String,
    produced_at: String,
}

struct StoredEventEvidence {
    event_id: String,
    stream_kind: String,
    stream_id: String,
    sequence: i64,
    recorded_at: String,
    recorded_at_millis: i64,
    observed_at: String,
    event_json: String,
}

struct StoredAuthoritySidecar {
    receipt_id: String,
    run_id: String,
    attempt_id: String,
    binding_id: String,
    binding_digest: String,
    base_receipt_digest: String,
    terminal_evidence_digest: String,
    authority_json: String,
    produced_at: String,
}

/// Read committed evidence in one snapshot, rechecking the body, terminal
/// correlation, run reference and event rather than trusting an indexed ID.
pub fn read_receipt(conn: &Connection, receipt_id: &str) -> Result<Option<AutomationReceipt>> {
    if conn.is_autocommit() {
        let transaction = conn.unchecked_transaction()?;
        let receipt = read_receipt_in(&transaction, receipt_id)?;
        transaction.commit()?;
        return Ok(receipt);
    }
    read_receipt_in(conn, receipt_id)
}

fn read_receipt_in(conn: &Connection, receipt_id: &str) -> Result<Option<AutomationReceipt>> {
    let stored: Option<StoredReceiptEvidence> = conn
        .query_row(
            "SELECT run_id, attempt_id, automation_id, automation_revision,
                    definition_digest, occurrence_id, occurrence_fence_generation,
                    attempt_number, outcome, receipt_digest, receipt_json, event_id,
                    produced_at
             FROM automation_receipts WHERE id = ?1",
            [receipt_id],
            |row| {
                Ok(StoredReceiptEvidence {
                    run_id: row.get(0)?,
                    attempt_id: row.get(1)?,
                    automation_id: row.get(2)?,
                    automation_revision: row.get(3)?,
                    definition_digest: row.get(4)?,
                    occurrence_id: row.get(5)?,
                    occurrence_fence_generation: row.get(6)?,
                    attempt_number: row.get(7)?,
                    outcome: row.get(8)?,
                    receipt_digest: row.get(9)?,
                    receipt_json: row.get(10)?,
                    event_id: row.get(11)?,
                    produced_at: row.get(12)?,
                })
            },
        )
        .optional()
        .context("failed to read automation receipt")?;
    let Some(stored) = stored else {
        return Ok(None);
    };
    let receipt: AutomationReceipt =
        serde_json::from_str(&stored.receipt_json).context("invalid stored automation receipt")?;
    validate_receipt_index(receipt_id, &receipt, &stored)?;
    let stored_event: StoredEventEvidence = conn
        .query_row(
            "SELECT event_id, stream_kind, stream_id, sequence, recorded_at,
                    recorded_at_millis, observed_at, event_json
             FROM automation_events WHERE event_id = ?1",
            [&stored.event_id],
            |row| {
                Ok(StoredEventEvidence {
                    event_id: row.get(0)?,
                    stream_kind: row.get(1)?,
                    stream_id: row.get(2)?,
                    sequence: row.get(3)?,
                    recorded_at: row.get(4)?,
                    recorded_at_millis: row.get(5)?,
                    observed_at: row.get(6)?,
                    event_json: row.get(7)?,
                })
            },
        )
        .context("automation receipt event is unavailable")?;
    let event: EventEnvelope = serde_json::from_str(&stored_event.event_json)
        .context("invalid stored automation receipt event")?;
    anyhow::ensure!(
        event.integrity.is_some(),
        "automation receipt event integrity is unavailable"
    );
    validate_event_index(&event, &stored_event)?;
    validate_receipt_event(&receipt, &event)?;
    validate_durable_correlation(&receipt, &durable_correlation(conn, &receipt)?)?;
    anyhow::ensure!(
        existing_commit(
            conn,
            &receipt,
            &event,
            &stored.receipt_json,
            &stored_event.event_json
        )? == Some(ReceiptCommitOutcome::Replayed),
        "automation receipt commitment is unavailable"
    );
    Ok(Some(receipt))
}

pub(crate) fn read_authorized_receipt(
    conn: &Connection,
    receipt_id: &str,
    verifier: &dyn AuthorityEvidenceVerifier,
) -> Result<Option<AuthorizedReceipt>> {
    if conn.is_autocommit() {
        let transaction = conn.unchecked_transaction()?;
        let receipt = read_authorized_receipt_in(&transaction, receipt_id, verifier)?;
        transaction.commit()?;
        return Ok(receipt);
    }
    read_authorized_receipt_in(conn, receipt_id, verifier)
}

fn read_authorized_receipt_in(
    conn: &Connection,
    receipt_id: &str,
    verifier: &dyn AuthorityEvidenceVerifier,
) -> Result<Option<AuthorizedReceipt>> {
    let Some(receipt) = read_receipt_in(conn, receipt_id)? else {
        return Ok(None);
    };
    let durable = durable_correlation(conn, &receipt)?;
    let pinned = validated_pinned_authority(&durable)?;
    let (stored, authority) =
        validated_stored_authority(conn, receipt_id, &receipt, &durable, &pinned, verifier)?;
    anyhow::ensure!(
        stored.receipt_id == receipt_id,
        "stored automation authority receipt key is invalid"
    );
    Ok(Some(AuthorizedReceipt { receipt, authority }))
}

fn validate_receipt_index(
    requested_receipt_id: &str,
    receipt: &AutomationReceipt,
    stored: &StoredReceiptEvidence,
) -> Result<()> {
    anyhow::ensure!(
        receipt.receipt_id.as_str() == requested_receipt_id
            && stored.run_id == receipt.run_id.as_str()
            && stored.attempt_id == receipt.attempt_id.as_str()
            && stored.automation_id == receipt.automation_id.as_str()
            && u64::try_from(stored.automation_revision).ok()
                == Some(receipt.automation_revision.get())
            && stored.definition_digest.as_deref()
                == receipt
                    .definition_digest
                    .as_ref()
                    .map(|digest| digest.value.as_str())
            && stored.occurrence_id == receipt.occurrence_id.as_str()
            && u64::try_from(stored.occurrence_fence_generation).ok()
                == receipt
                    .occurrence_fence_generation
                    .map(|generation| generation.get())
            && u64::try_from(stored.attempt_number).ok()
                == receipt.attempt_number.map(|attempt| attempt.get())
            && stored.outcome == terminal_outcome_name(receipt.outcome.disposition)
            && stored.receipt_digest == receipt.integrity.value.as_str()
            && stored.produced_at == receipt.produced_at.as_str(),
        "automation receipt index does not match committed evidence"
    );
    Ok(())
}

fn validate_event_index(event: &EventEnvelope, stored: &StoredEventEvidence) -> Result<()> {
    let recorded_at_millis = chrono::DateTime::parse_from_rfc3339(event.recorded_at.as_str())
        .context("invalid stored automation receipt event time")?
        .timestamp_millis();
    anyhow::ensure!(
        stored.event_id == event.event_id.as_str()
            && stored.stream_kind == "run"
            && stored.stream_id == event.stream.id.as_str()
            && u64::try_from(stored.sequence).ok() == Some(event.sequence.get())
            && stored.recorded_at == event.recorded_at.as_str()
            && stored.recorded_at_millis == recorded_at_millis
            && stored.observed_at == event.observed_at.as_str(),
        "automation receipt event index does not match committed evidence"
    );
    Ok(())
}

struct DurableCorrelation {
    automation_id: String,
    automation_revision: i64,
    definition_digest: Option<String>,
    occurrence_id: String,
    familiar_id: Option<String>,
    runtime: String,
    authority_profile: Option<String>,
    run_status: String,
    run_receipt_id: Option<String>,
    attempt_occurrence_id: String,
    attempt_number: i64,
    adoption_key: String,
    occurrence_fence_generation: i64,
    authority_extension_json: Option<String>,
    attempt_state: String,
    failure_class: Option<String>,
    state_reason: Option<String>,
    run_finished_at: Option<String>,
    attempt_settled_at: Option<String>,
}

pub fn commit_receipt(
    conn: &Connection,
    receipt: &AutomationReceipt,
    event: &EventEnvelope,
) -> Result<ReceiptCommitOutcome> {
    if conn.is_autocommit() {
        let transaction =
            rusqlite::Transaction::new_unchecked(conn, TransactionBehavior::Immediate)
                .context("failed to begin automation receipt transaction")?;
        let outcome = commit_receipt_in(&transaction, receipt, event)?;
        transaction
            .commit()
            .context("failed to commit automation receipt transaction")?;
        return Ok(outcome);
    }
    commit_receipt_in(conn, receipt, event)
}

pub(crate) fn commit_authorized_receipt(
    conn: &Connection,
    receipt: &AutomationReceipt,
    event: &EventEnvelope,
    terminal_extensions: &ExtensionBag,
    verifier: &dyn AuthorityEvidenceVerifier,
) -> Result<ReceiptCommitOutcome> {
    if conn.is_autocommit() {
        let transaction =
            rusqlite::Transaction::new_unchecked(conn, TransactionBehavior::Immediate)
                .context("failed to begin authorized automation receipt transaction")?;
        let outcome = commit_authorized_receipt_in(
            &transaction,
            receipt,
            event,
            terminal_extensions,
            verifier,
        )?;
        transaction
            .commit()
            .context("failed to commit authorized automation receipt transaction")?;
        return Ok(outcome);
    }
    commit_authorized_receipt_in(conn, receipt, event, terminal_extensions, verifier)
}

fn commit_authorized_receipt_in(
    conn: &Connection,
    receipt: &AutomationReceipt,
    event: &EventEnvelope,
    terminal_extensions: &ExtensionBag,
    verifier: &dyn AuthorityEvidenceVerifier,
) -> Result<ReceiptCommitOutcome> {
    let durable = validate_receipt_commit(conn, receipt, event)?;
    let pinned = validated_pinned_authority(&durable)?;
    let terminal = validated_authority_profile(
        terminal_extensions,
        AuthorityValidationPhase::Terminal,
        verifier,
        "terminal automation authority evidence is invalid",
    )?;
    validate_authorized_evidence(receipt, &durable, &pinned, &terminal)?;

    let receipt_json =
        serde_json::to_string(receipt).context("failed to serialize automation receipt")?;
    let event_json =
        serde_json::to_string(event).context("failed to serialize automation receipt event")?;
    let authority_json = serde_json::to_string(&terminal)
        .context("failed to serialize terminal automation authority evidence")?;
    if let Some(outcome) = existing_commit(conn, receipt, event, &receipt_json, &event_json)? {
        let (_, stored_authority) = validated_stored_authority(
            conn,
            receipt.receipt_id.as_str(),
            receipt,
            &durable,
            &pinned,
            verifier,
        )?;
        anyhow::ensure!(
            stored_authority == terminal,
            "authorized automation receipt replay conflicts with committed authority evidence"
        );
        return Ok(outcome);
    }
    anyhow::ensure!(
        durable.run_receipt_id.is_none(),
        "automation run already references a different receipt"
    );

    let receipt_evidence = terminal
        .receipt_evidence
        .0
        .as_deref()
        .context("terminal automation authority receipt evidence is unavailable")?;
    conn.execute_batch("SAVEPOINT coven_authorized_automation_receipt_commit")
        .context("failed to begin authorized automation receipt commitment")?;
    let result = (|| {
        insert_base_receipt_commit(conn, receipt, event, &receipt_json)?;
        conn.execute(
            "INSERT INTO automation_receipt_authority_extensions (
                receipt_id, run_id, attempt_id, binding_id, binding_digest,
                base_receipt_digest, terminal_evidence_digest,
                authority_json, produced_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                receipt.receipt_id.as_str(),
                receipt.run_id.as_str(),
                receipt.attempt_id.as_str(),
                terminal.execution_binding.binding_id.as_str(),
                terminal.execution_binding.integrity.value.as_str(),
                receipt_evidence.base_receipt_digest.value.as_str(),
                receipt_evidence.integrity.value.as_str(),
                authority_json,
                receipt.produced_at.as_str(),
            ],
        )
        .context("failed to insert automation receipt authority extension")?;
        Ok(ReceiptCommitOutcome::Committed)
    })();

    match result {
        Ok(outcome) => {
            conn.execute_batch("RELEASE SAVEPOINT coven_authorized_automation_receipt_commit")
                .context("failed to commit authorized automation receipt")?;
            Ok(outcome)
        }
        Err(error) => {
            let _ = conn.execute_batch(
                "ROLLBACK TO SAVEPOINT coven_authorized_automation_receipt_commit;
                 RELEASE SAVEPOINT coven_authorized_automation_receipt_commit;",
            );
            Err(error)
        }
    }
}

fn commit_receipt_in(
    conn: &Connection,
    receipt: &AutomationReceipt,
    event: &EventEnvelope,
) -> Result<ReceiptCommitOutcome> {
    let durable = validate_receipt_commit(conn, receipt, event)?;
    anyhow::ensure!(
        !is_runtime_authority_pinned(&durable),
        "Runtime Authority-pinned runs require an authorized receipt commitment"
    );

    let receipt_json =
        serde_json::to_string(receipt).context("failed to serialize automation receipt")?;
    let event_json =
        serde_json::to_string(event).context("failed to serialize automation receipt event")?;
    if let Some(outcome) = existing_commit(conn, receipt, event, &receipt_json, &event_json)? {
        return Ok(outcome);
    }
    anyhow::ensure!(
        durable.run_receipt_id.is_none(),
        "automation run already references a different receipt"
    );

    conn.execute_batch("SAVEPOINT coven_automation_receipt_commit")
        .context("failed to begin automation receipt commitment")?;
    let result = insert_base_receipt_commit(conn, receipt, event, &receipt_json);

    match result {
        Ok(()) => {
            conn.execute_batch("RELEASE SAVEPOINT coven_automation_receipt_commit")
                .context("failed to commit automation receipt")?;
            Ok(ReceiptCommitOutcome::Committed)
        }
        Err(error) => {
            let _ = conn.execute_batch(
                "ROLLBACK TO SAVEPOINT coven_automation_receipt_commit;
                 RELEASE SAVEPOINT coven_automation_receipt_commit;",
            );
            Err(error)
        }
    }
}

fn validate_receipt_commit(
    conn: &Connection,
    receipt: &AutomationReceipt,
    event: &EventEnvelope,
) -> Result<DurableCorrelation> {
    anyhow::ensure!(
        event.integrity.is_some(),
        "automation receipt event integrity is required"
    );
    receipt
        .verify_integrity()
        .context("automation receipt integrity is invalid")?;
    event
        .verify_integrity()
        .context("automation receipt event integrity is invalid")?;
    validate_receipt_event(receipt, event)?;
    let durable = durable_correlation(conn, receipt)?;
    validate_durable_correlation(receipt, &durable)?;
    Ok(durable)
}

fn is_runtime_authority_pinned(durable: &DurableCorrelation) -> bool {
    durable.authority_profile.as_deref() == Some(AUTHORITY_PROFILE)
        || durable.authority_extension_json.is_some()
}

fn insert_base_receipt_commit(
    conn: &Connection,
    receipt: &AutomationReceipt,
    event: &EventEnvelope,
    receipt_json: &str,
) -> Result<()> {
    conn.execute(
        "INSERT INTO automation_receipts (
                id, run_id, attempt_id, automation_id, automation_revision,
                definition_digest, occurrence_id, occurrence_fence_generation,
                attempt_number, outcome, receipt_digest, receipt_json, event_id, produced_at
             ) VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14
             )",
        params![
            receipt.receipt_id.as_str(),
            receipt.run_id.as_str(),
            receipt.attempt_id.as_str(),
            receipt.automation_id.as_str(),
            i64::try_from(receipt.automation_revision.get())
                .context("receipt automation revision exceeds SQLite range")?,
            receipt
                .definition_digest
                .as_ref()
                .map(|digest| digest.value.as_str()),
            receipt.occurrence_id.as_str(),
            i64::try_from(
                receipt
                    .occurrence_fence_generation
                    .context("receipt occurrence fence generation is required")?
                    .get()
            )
            .context("receipt occurrence fence generation exceeds SQLite range")?,
            i64::try_from(
                receipt
                    .attempt_number
                    .context("receipt attempt number is required")?
                    .get()
            )
            .context("receipt attempt number exceeds SQLite range")?,
            terminal_outcome_name(receipt.outcome.disposition),
            receipt.integrity.value.as_str(),
            receipt_json,
            event.event_id.as_str(),
            receipt.produced_at.as_str(),
        ],
    )
    .context("failed to insert automation receipt")?;
    let linked = conn
        .execute(
            "UPDATE automation_runs
                 SET receipt_id = ?2
                 WHERE id = ?1 AND receipt_id IS NULL",
            params![receipt.run_id.as_str(), receipt.receipt_id.as_str()],
        )
        .context("failed to link automation run receipt")?;
    anyhow::ensure!(
        linked == 1,
        "automation run changed during receipt commitment"
    );
    append_event(conn, event, event.sequence.get())
        .map_err(anyhow::Error::new)
        .context("failed to append automation receipt event")?;
    Ok(())
}

fn durable_correlation(
    conn: &Connection,
    receipt: &AutomationReceipt,
) -> Result<DurableCorrelation> {
    conn.query_row(
        "SELECT run.automation_id, run.automation_revision, run.definition_digest,
                run.occurrence_id, run.familiar_id, run.runtime, run.authority_profile,
                run.status, run.receipt_id, attempt.occurrence_id, attempt.attempt_number,
                attempt.adoption_key, attempt.occurrence_fence_generation,
                attempt.authority_extension_json, attempt.state, attempt.failure_class,
                attempt.state_reason, run.finished_at, attempt.settled_at
         FROM automation_runs AS run
         JOIN automation_attempts AS attempt
           ON attempt.run_id = run.id
         WHERE run.id = ?1 AND attempt.id = ?2",
        params![receipt.run_id.as_str(), receipt.attempt_id.as_str()],
        |row| {
            Ok(DurableCorrelation {
                automation_id: row.get(0)?,
                automation_revision: row.get(1)?,
                definition_digest: row.get(2)?,
                occurrence_id: row.get(3)?,
                familiar_id: row.get(4)?,
                runtime: row.get(5)?,
                authority_profile: row.get(6)?,
                run_status: row.get(7)?,
                run_receipt_id: row.get(8)?,
                attempt_occurrence_id: row.get(9)?,
                attempt_number: row.get(10)?,
                adoption_key: row.get(11)?,
                occurrence_fence_generation: row.get(12)?,
                authority_extension_json: row.get(13)?,
                attempt_state: row.get(14)?,
                failure_class: row.get(15)?,
                state_reason: row.get(16)?,
                run_finished_at: row.get(17)?,
                attempt_settled_at: row.get(18)?,
            })
        },
    )
    .optional()
    .context("failed to read automation receipt correlation")?
    .context("automation receipt does not identify a durable run attempt")
}

fn validate_durable_correlation(
    receipt: &AutomationReceipt,
    durable: &DurableCorrelation,
) -> Result<()> {
    anyhow::ensure!(
        durable.automation_id == receipt.automation_id.as_str(),
        "automation receipt automation does not match its run"
    );
    anyhow::ensure!(
        u64::try_from(durable.automation_revision).ok() == Some(receipt.automation_revision.get()),
        "automation receipt revision does not match its run"
    );
    anyhow::ensure!(
        durable.definition_digest.as_deref()
            == receipt
                .definition_digest
                .as_ref()
                .map(|digest| digest.value.as_str()),
        "automation receipt definition digest does not match its run"
    );
    anyhow::ensure!(
        durable.occurrence_id == receipt.occurrence_id.as_str(),
        "automation receipt occurrence does not match its run"
    );
    anyhow::ensure!(
        durable.attempt_occurrence_id == durable.occurrence_id,
        "automation receipt attempt does not belong to the run occurrence"
    );
    anyhow::ensure!(
        durable.familiar_id.as_deref() == Some(receipt.identity.familiar_id.as_str()),
        "automation receipt familiar does not match its run"
    );
    if let Some(runtime) = &receipt.runtime {
        anyhow::ensure!(
            durable.runtime == runtime.runtime_id.as_str(),
            "automation receipt runtime does not match its run"
        );
    }
    anyhow::ensure!(
        u64::try_from(durable.attempt_number).ok()
            == receipt.attempt_number.map(|attempt| attempt.get()),
        "automation receipt attempt number does not match its attempt"
    );
    anyhow::ensure!(
        u64::try_from(durable.occurrence_fence_generation).ok()
            == receipt
                .occurrence_fence_generation
                .map(|generation| generation.get()),
        "automation receipt fence does not match its attempt"
    );
    let (run_status, attempt_state) = terminal_states(receipt.outcome.disposition);
    anyhow::ensure!(
        durable.run_status == run_status && durable.attempt_state == attempt_state,
        "automation receipt outcome does not match durable terminal state"
    );
    anyhow::ensure!(
        durable.failure_class.as_deref()
            == receipt
                .outcome
                .failure_class
                .as_ref()
                .map(|failure| failure.as_str()),
        "automation receipt failure class does not match its attempt"
    );
    if let Some(detail) = &receipt.outcome.detail {
        anyhow::ensure!(
            durable.state_reason.as_deref() == Some(detail.as_str()),
            "automation receipt detail does not match its attempt"
        );
    }
    anyhow::ensure!(
        durable.run_finished_at.as_deref() == Some(receipt.produced_at.as_str())
            && durable.attempt_settled_at.as_deref() == Some(receipt.produced_at.as_str()),
        "automation receipt production time does not match terminal settlement"
    );
    Ok(())
}

fn validated_pinned_authority(
    durable: &DurableCorrelation,
) -> Result<AutomationAuthorityExtension> {
    anyhow::ensure!(
        durable.authority_profile.as_deref() == Some(AUTHORITY_PROFILE),
        "automation run is not pinned to Runtime Authority"
    );
    let raw = durable
        .authority_extension_json
        .as_deref()
        .context("stored pre-dispatch automation authority evidence is unavailable")?;
    let extensions = stored_extension_bag(
        raw,
        "stored pre-dispatch automation authority evidence is invalid",
    )?;
    let disposition = validate_authority_profile_structure(
        &extensions,
        AuthorityConsumerClass::RuntimeAuthorityV1,
        &[BASE_PROFILE, AUTHORITY_PROFILE],
        &[RUNTIME_AUTHORITY_CAPABILITY],
        AuthorityValidationPhase::PreDispatch,
    )
    .map_err(|error| {
        anyhow::anyhow!(
            "stored pre-dispatch automation authority evidence is invalid: {}",
            error.code().as_str()
        )
    })?;
    let AuthorityProfileDisposition::Validated(extension) = disposition else {
        anyhow::bail!("stored pre-dispatch automation authority evidence is invalid");
    };
    Ok(*extension)
}

fn stored_extension_bag(raw: &str, error_message: &'static str) -> Result<ExtensionBag> {
    let extension: Value = serde_json::from_str(raw).map_err(|_| anyhow::anyhow!(error_message))?;
    ExtensionBag::new(BTreeMap::from([(
        AUTHORITY_EXTENSION_KEY.to_owned(),
        extension,
    )]))
    .map_err(|_| anyhow::anyhow!(error_message))
}

fn validated_authority_profile(
    extensions: &ExtensionBag,
    phase: AuthorityValidationPhase,
    verifier: &dyn AuthorityEvidenceVerifier,
    error_message: &'static str,
) -> Result<AutomationAuthorityExtension> {
    let disposition = validate_authority_profile(
        extensions,
        AuthorityConsumerClass::RuntimeAuthorityV1,
        &[BASE_PROFILE, AUTHORITY_PROFILE],
        &[RUNTIME_AUTHORITY_CAPABILITY],
        phase,
        Some(verifier),
    )
    .map_err(|error| anyhow::anyhow!("{error_message}: {}", error.code().as_str()))?;
    let AuthorityProfileDisposition::Validated(extension) = disposition else {
        anyhow::bail!("{error_message}");
    };
    Ok(*extension)
}

fn validate_authorized_evidence(
    receipt: &AutomationReceipt,
    durable: &DurableCorrelation,
    pinned: &AutomationAuthorityExtension,
    terminal: &AutomationAuthorityExtension,
) -> Result<()> {
    anyhow::ensure!(
        terminal.execution_binding == pinned.execution_binding,
        "terminal automation authority binding does not match the pinned dispatch binding"
    );
    validate_pinned_binding(receipt, durable, pinned)?;
    let evidence = terminal
        .receipt_evidence
        .0
        .as_deref()
        .context("terminal automation authority receipt evidence is unavailable")?;
    anyhow::ensure!(
        evidence.receipt_id == receipt.receipt_id
            && evidence.automation_id == receipt.automation_id
            && evidence.automation_revision == receipt.automation_revision
            && receipt.definition_digest.as_ref() == Some(&evidence.definition_digest)
            && evidence.occurrence_id == receipt.occurrence_id
            && receipt.occurrence_fence_generation == Some(evidence.occurrence_fence_generation)
            && evidence.run_id == receipt.run_id
            && evidence.attempt_id == receipt.attempt_id
            && receipt.attempt_number == Some(evidence.attempt_number)
            && evidence.base_receipt_digest.algorithm == receipt.integrity.algorithm
            && evidence.base_receipt_digest.canonicalization == receipt.integrity.canonicalization
            && evidence.base_receipt_digest.value == receipt.integrity.value,
        "terminal automation authority evidence does not match the base receipt"
    );
    validate_overlapping_claims(receipt, terminal)
}

fn validate_pinned_binding(
    receipt: &AutomationReceipt,
    durable: &DurableCorrelation,
    pinned: &AutomationAuthorityExtension,
) -> Result<()> {
    let base = &pinned.execution_binding.base;
    anyhow::ensure!(
        base.automation_id == receipt.automation_id
            && base.automation_revision == receipt.automation_revision
            && receipt.definition_digest.as_ref() == Some(&base.definition_digest)
            && base.occurrence_id == receipt.occurrence_id
            && receipt.occurrence_fence_generation == Some(base.occurrence_fence_generation)
            && base.run_id == receipt.run_id
            && base.attempt_id == receipt.attempt_id
            && receipt.attempt_number == Some(base.attempt_number)
            && base.adoption_key.as_str() == durable.adoption_key
            && pinned.execution_binding.runtime.runtime_id.as_str() == durable.runtime,
        "stored pre-dispatch automation authority binding is not correlated to the durable attempt"
    );
    Ok(())
}

fn validate_overlapping_claims(
    receipt: &AutomationReceipt,
    terminal: &AutomationAuthorityExtension,
) -> Result<()> {
    let evidence = terminal
        .receipt_evidence
        .0
        .as_deref()
        .context("terminal automation authority receipt evidence is unavailable")?;
    let exercised = receipt
        .exercised_capabilities
        .as_ref()
        .context("authorized automation receipts require explicit exercised capabilities")?;
    anyhow::ensure!(
        serialized_string_set(exercised)?
            == serialized_string_set(&evidence.capabilities.exercised)?,
        "base and authority exercised capabilities do not match"
    );
    anyhow::ensure!(
        base_side_effect_rank(receipt.side_effect_class)
            <= authority_side_effect_rank(evidence.risk.side_effect_class),
        "base receipt side effect exceeds the authorized envelope"
    );
    if let Some(authority) = &receipt.authority {
        anyhow::ensure!(
            authority.principal.principal_id == evidence.principal_id,
            "base and authority principal references do not match"
        );
        if let Some(record_ref) = authority
            .approval
            .as_ref()
            .and_then(|approval| approval.approval_record_ref.as_ref())
        {
            anyhow::ensure!(
                authority_approval_id(&evidence.approval) == Some(record_ref.as_str()),
                "base and authority approval references do not match"
            );
        }
    }
    if let Some(runtime) = &receipt.runtime {
        anyhow::ensure!(
            runtime.runtime_id == evidence.runtime.runtime_id
                && serialized_string_set(&runtime.capabilities)?
                    == serialized_string_set(&evidence.runtime.capabilities)?,
            "base and authority runtime descriptors do not match"
        );
    }
    Ok(())
}

const fn base_side_effect_rank(side_effect: SideEffectClass) -> u8 {
    match side_effect {
        SideEffectClass::None => 0,
        SideEffectClass::LocalRead => 1,
        SideEffectClass::LocalWrite => 2,
        SideEffectClass::ExternalRead => 3,
        SideEffectClass::ExternalMutation => 4,
        SideEffectClass::IrreversibleExternalMutation => 5,
    }
}

const fn authority_side_effect_rank(side_effect: AuthoritySideEffectClass) -> u8 {
    match side_effect {
        AuthoritySideEffectClass::None => 0,
        AuthoritySideEffectClass::LocalRead => 1,
        AuthoritySideEffectClass::LocalWrite => 2,
        AuthoritySideEffectClass::ExternalRead => 3,
        AuthoritySideEffectClass::ExternalMutation => 4,
        AuthoritySideEffectClass::IrreversibleExternalMutation => 5,
    }
}

fn authority_approval_id(approval: &AuthorityApprovalBinding) -> Option<&str> {
    match approval {
        AuthorityApprovalBinding::NotRequired { .. } => None,
        AuthorityApprovalBinding::HumanPerRun { evidence, .. }
        | AuthorityApprovalBinding::ProtectedOwnerPerRun { evidence, .. }
        | AuthorityApprovalBinding::BoundedRecurring { evidence, .. } => {
            Some(evidence.approval_id.as_str())
        }
    }
}

fn serialized_string_set(value: &impl Serialize) -> Result<BTreeSet<String>> {
    let Value::Array(values) = serde_json::to_value(value)
        .map_err(|_| anyhow::anyhow!("automation authority claim encoding is invalid"))?
    else {
        anyhow::bail!("automation authority claim encoding is invalid");
    };
    values
        .into_iter()
        .map(|value| {
            value
                .as_str()
                .map(str::to_owned)
                .context("automation authority capability evidence is invalid")
        })
        .collect()
}

fn validated_stored_authority(
    conn: &Connection,
    requested_receipt_id: &str,
    receipt: &AutomationReceipt,
    durable: &DurableCorrelation,
    pinned: &AutomationAuthorityExtension,
    verifier: &dyn AuthorityEvidenceVerifier,
) -> Result<(StoredAuthoritySidecar, AutomationAuthorityExtension)> {
    let stored = conn
        .query_row(
            "SELECT receipt_id, run_id, attempt_id, binding_id, binding_digest,
                    base_receipt_digest, terminal_evidence_digest,
                    authority_json, produced_at
             FROM automation_receipt_authority_extensions
             WHERE receipt_id = ?1",
            [requested_receipt_id],
            |row| {
                Ok(StoredAuthoritySidecar {
                    receipt_id: row.get(0)?,
                    run_id: row.get(1)?,
                    attempt_id: row.get(2)?,
                    binding_id: row.get(3)?,
                    binding_digest: row.get(4)?,
                    base_receipt_digest: row.get(5)?,
                    terminal_evidence_digest: row.get(6)?,
                    authority_json: row.get(7)?,
                    produced_at: row.get(8)?,
                })
            },
        )
        .optional()
        .context("failed to read automation receipt authority extension")?
        .context("automation receipt authority extension is unavailable")?;
    let extensions = stored_extension_bag(
        &stored.authority_json,
        "stored terminal automation authority evidence is invalid",
    )?;
    let authority = validated_authority_profile(
        &extensions,
        AuthorityValidationPhase::Terminal,
        verifier,
        "stored terminal automation authority evidence is invalid",
    )?;
    validate_authorized_evidence(receipt, durable, pinned, &authority)?;
    validate_authority_sidecar_index(requested_receipt_id, receipt, &stored, &authority)?;
    Ok((stored, authority))
}

fn validate_authority_sidecar_index(
    requested_receipt_id: &str,
    receipt: &AutomationReceipt,
    stored: &StoredAuthoritySidecar,
    authority: &AutomationAuthorityExtension,
) -> Result<()> {
    let evidence = authority
        .receipt_evidence
        .0
        .as_deref()
        .context("stored terminal automation authority receipt evidence is unavailable")?;
    anyhow::ensure!(
        stored.receipt_id == requested_receipt_id
            && stored.receipt_id == receipt.receipt_id.as_str()
            && stored.run_id == receipt.run_id.as_str()
            && stored.attempt_id == receipt.attempt_id.as_str()
            && stored.binding_id == authority.execution_binding.binding_id.as_str()
            && stored.binding_digest == authority.execution_binding.integrity.value.as_str()
            && stored.base_receipt_digest == receipt.integrity.value.as_str()
            && stored.base_receipt_digest == evidence.base_receipt_digest.value.as_str()
            && stored.terminal_evidence_digest == evidence.integrity.value.as_str()
            && stored.produced_at == receipt.produced_at.as_str(),
        "automation receipt authority index does not match committed evidence"
    );
    Ok(())
}

fn validate_receipt_event(receipt: &AutomationReceipt, event: &EventEnvelope) -> Result<()> {
    anyhow::ensure!(
        event.kind == EventKind::ReceiptRecorded,
        "automation receipt event has the wrong kind"
    );
    anyhow::ensure!(
        event.stream.kind == StreamKind::Run && event.stream.id.as_str() == receipt.run_id.as_str(),
        "automation receipt event has the wrong stream"
    );
    anyhow::ensure!(
        event.automation_id.as_ref().map(|id| id.as_str()) == Some(receipt.automation_id.as_str())
            && event.occurrence_id.as_ref().map(|id| id.as_str())
                == Some(receipt.occurrence_id.as_str())
            && event.run_id.as_ref().map(|id| id.as_str()) == Some(receipt.run_id.as_str())
            && event.attempt_id.as_ref().map(|id| id.as_str()) == Some(receipt.attempt_id.as_str()),
        "automation receipt event correlation does not match the receipt"
    );
    let EventPayload::Receipt(payload) = &event.payload else {
        anyhow::bail!("automation receipt event has the wrong payload");
    };
    anyhow::ensure!(
        payload.receipt_ref == receipt.receipt_id
            && payload.outcome == receipt.outcome.disposition
            && payload.side_effect_class == Some(receipt.side_effect_class),
        "automation receipt event payload does not match the receipt"
    );
    anyhow::ensure!(
        event.recorded_at == receipt.produced_at && event.producer == receipt.producer,
        "automation receipt event producer or time does not match the receipt"
    );
    anyhow::ensure!(
        event.privacy.classification == receipt.privacy.classification
            && event.privacy.retention == receipt.privacy.retention,
        "automation receipt event privacy does not match the receipt"
    );
    Ok(())
}

fn existing_commit(
    conn: &Connection,
    receipt: &AutomationReceipt,
    event: &EventEnvelope,
    receipt_json: &str,
    event_json: &str,
) -> Result<Option<ReceiptCommitOutcome>> {
    let mut statement = conn
        .prepare(
            "SELECT id, run_id, attempt_id, event_id, receipt_json
             FROM automation_receipts
             WHERE id = ?1 OR run_id = ?2 OR attempt_id = ?3",
        )
        .context("failed to prepare automation receipt replay query")?;
    let rows = statement
        .query_map(
            params![
                receipt.receipt_id.as_str(),
                receipt.run_id.as_str(),
                receipt.attempt_id.as_str()
            ],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                ))
            },
        )
        .context("failed to query automation receipt replay")?;
    let existing = rows
        .collect::<std::result::Result<Vec<_>, _>>()
        .context("failed to read automation receipt replay")?;
    if existing.is_empty() {
        return Ok(None);
    }
    anyhow::ensure!(
        existing.len() == 1,
        "automation receipt conflicts with existing receipt correlations"
    );
    let (id, run_id, attempt_id, event_id, stored_receipt_json) = &existing[0];
    let stored_event_json = conn
        .query_row(
            "SELECT event_json FROM automation_events WHERE event_id = ?1",
            [event_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .context("failed to read replayed automation receipt event")?;
    let run_receipt_id = conn
        .query_row(
            "SELECT receipt_id FROM automation_runs WHERE id = ?1",
            [receipt.run_id.as_str()],
            |row| row.get::<_, Option<String>>(0),
        )
        .context("failed to read replayed automation run receipt")?;
    anyhow::ensure!(
        id == receipt.receipt_id.as_str()
            && run_id == receipt.run_id.as_str()
            && attempt_id == receipt.attempt_id.as_str()
            && event_id == event.event_id.as_str()
            && stored_receipt_json == receipt_json
            && stored_event_json.as_deref() == Some(event_json)
            && run_receipt_id.as_deref() == Some(receipt.receipt_id.as_str()),
        "automation receipt replay conflicts with committed evidence"
    );
    Ok(Some(ReceiptCommitOutcome::Replayed))
}

const fn terminal_outcome_name(outcome: TerminalOutcome) -> &'static str {
    match outcome {
        TerminalOutcome::Succeeded => "succeeded",
        TerminalOutcome::Failed => "failed",
        TerminalOutcome::Cancelled => "cancelled",
        TerminalOutcome::TimedOut => "timed_out",
        TerminalOutcome::Ambiguous => "ambiguous",
    }
}

const fn terminal_states(outcome: TerminalOutcome) -> (&'static str, &'static str) {
    match outcome {
        TerminalOutcome::Succeeded => ("succeeded", "succeeded"),
        TerminalOutcome::Failed => ("failed", "failed"),
        TerminalOutcome::Cancelled => ("cancelled", "cancelled"),
        TerminalOutcome::TimedOut => ("failed", "timed_out"),
        TerminalOutcome::Ambiguous => ("failed", "ambiguous"),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        commit_authorized_receipt, commit_receipt, read_authorized_receipt, read_receipt,
        ReceiptCommitOutcome,
    };
    use crate::api::{
        handle_request_with_runtime_and_authority, NoopSessionRuntime, RequestAuthority,
    };
    use crate::automations::contract::authority::test_support::{
        authority_extensions_value, extension_bag, resign_authority_extensions, AcceptingVerifier,
    };
    use crate::automations::contract::authority::{
        AuthorityEvidenceVerifier, AuthorityProfileError, AuthorityProfileErrorCode,
        AuthorityValidationPhase, AutomationAuthorityExtension, AUTHORITY_EXTENSION_KEY,
        AUTHORITY_PROFILE,
    };
    use crate::automations::contract::canonical_json::{
        canonicalize_without_integrity, sha256_hex,
    };
    use crate::automations::contract::events::stream_head;
    use crate::automations::contract::types::{AutomationReceipt, EventEnvelope, ExtensionBag};
    use crate::automations::definition::RoutineDefinition;
    use crate::automations::occurrences::insert_claimed_occurrence;
    use crate::automations::runs::{record_run_finish, record_run_start, RunFinish, RunStart};
    use crate::automations::store::insert_definition;
    use crate::store::initialize_store;
    use chrono::{DateTime, SecondsFormat, Utc};
    use rusqlite::Connection;
    use serde_json::{json, Value};
    use std::path::PathBuf;
    use std::sync::{Arc, Barrier, Mutex};

    struct Fixture {
        _temp: tempfile::TempDir,
        store_path: PathBuf,
        conn: Connection,
        definition_digest: String,
        produced_at: DateTime<Utc>,
    }

    #[derive(Debug, PartialEq, Eq)]
    struct ReceiptCommitState {
        receipt_row_count: i64,
        receipt_table_count: i64,
        run_receipt_id: Option<String>,
        event_row_count: i64,
        event_table_count: i64,
        stream_head: Option<u64>,
        sidecar_row_count: i64,
        sidecar_table_count: i64,
    }

    fn fixture() -> Fixture {
        fixture_with_options("occurrence-daily-1", false)
    }

    fn authorized_fixture() -> Fixture {
        fixture_with_options("occurrence-daily-1", true)
    }

    fn fixture_with_attempt_occurrence(attempt_occurrence_id: &str) -> Fixture {
        fixture_with_options(attempt_occurrence_id, false)
    }

    fn fixture_with_options(attempt_occurrence_id: &str, authorized: bool) -> Fixture {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("coven.sqlite3");
        initialize_store(&path).unwrap();
        let conn = crate::store::open_store(&path).unwrap();
        let definition = RoutineDefinition::from_json(&json!({
            "schemaVersion": 1,
            "id": "daily",
            "name": "daily",
            "status": "PAUSED",
            "rrule": "FREQ=DAILY;BYHOUR=9",
            "timezone": "utc",
            "misfire": "latest",
            "overlap": "forbid",
            "timeoutMinutes": 30,
            "runtime": "coven-code",
            "familiarId": "charm",
            "prompt": "Do the thing."
        }))
        .unwrap();
        insert_definition(&conn, &definition).unwrap();
        let definition_digest: String = conn
            .query_row(
                "SELECT definition_digest
                 FROM automation_definitions
                 WHERE id = 'daily'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let opened_at = Utc::now();
        insert_claimed_occurrence(
            &conn,
            "occurrence-daily-1",
            "daily",
            "daemon",
            60,
            opened_at,
        )
        .unwrap();
        if attempt_occurrence_id != "occurrence-daily-1" {
            conn.execute(
                "INSERT INTO automation_occurrences (
                    id, automation_id, automation_revision, definition_digest,
                    scheduled_for, kind, state, lease_owner, lease_expires_at,
                    attempt, created_at, updated_at
                 )
                 SELECT ?1, automation_id, automation_revision, definition_digest,
                        ?2, kind, state, lease_owner, lease_expires_at,
                        attempt, created_at, updated_at
                 FROM automation_occurrences
                 WHERE id = 'occurrence-daily-1'",
                rusqlite::params![
                    attempt_occurrence_id,
                    (opened_at + chrono::Duration::seconds(1))
                        .to_rfc3339_opts(SecondsFormat::Nanos, true),
                ],
            )
            .unwrap();
        }
        record_run_start(
            &conn,
            "run-daily-1",
            RunStart {
                automation_id: "daily",
                occurrence_id: Some("occurrence-daily-1"),
                authority_profile: authorized.then_some(AUTHORITY_PROFILE),
                session_id: None,
                familiar_id: Some("charm"),
                runtime: "coven-code",
                timeout_at: opened_at + chrono::Duration::minutes(30),
            },
            opened_at,
        )
        .unwrap();
        let authority_extension_json = authorized.then(|| {
            let value =
                serde_json::to_value(authority_extensions_for(&definition_digest, None)).unwrap();
            serde_json::to_string(&value[AUTHORITY_EXTENSION_KEY]).unwrap()
        });
        conn.execute(
            "INSERT INTO automation_attempts (
                id, run_id, occurrence_id, attempt_number, adoption_key,
                occurrence_fence_generation, state, retry_classification,
                authority_extension_json, not_before, opened_at, settled_at
             ) VALUES (?1, ?2, ?3, 1, ?4, 1, 'succeeded', 'initial', ?5, ?6, ?6, ?7)",
            rusqlite::params![
                "attempt-daily-1",
                "run-daily-1",
                attempt_occurrence_id,
                "automation:run-daily-1:1",
                authority_extension_json,
                opened_at.to_rfc3339_opts(SecondsFormat::Millis, true),
                (opened_at + chrono::Duration::seconds(5))
                    .to_rfc3339_opts(SecondsFormat::Millis, true),
            ],
        )
        .unwrap();
        let produced_at = opened_at + chrono::Duration::seconds(5);
        record_run_finish(
            &conn,
            "run-daily-1",
            RunFinish {
                status: "succeeded",
                exit_code: Some(0),
                session_id: None,
                log_json: None,
                output_commit: None,
            },
            produced_at,
        )
        .unwrap();
        Fixture {
            _temp: temp,
            store_path: path,
            conn,
            definition_digest,
            produced_at,
        }
    }

    fn make_receipt(fixture: &Fixture, receipt_id: &str) -> AutomationReceipt {
        let mut value = json!({
            "schemaVersion": "coven.automations.v1",
            "receiptId": receipt_id,
            "automationId": "daily",
            "automationRevision": 1,
            "definitionDigest": {
                "algorithm": "sha256",
                "canonicalization": "jcs-rfc8785",
                "value": fixture.definition_digest,
            },
            "occurrenceId": "occurrence-daily-1",
            "occurrenceFenceGeneration": 1,
            "runId": "run-daily-1",
            "attemptId": "attempt-daily-1",
            "attemptNumber": 1,
            "identity": {"familiarId": "charm"},
            "sideEffectClass": "external_mutation",
            "outcome": {
                "disposition": "succeeded",
                "recoveryDisposition": "not_required"
            },
            "producedAt": fixture.produced_at.to_rfc3339_opts(SecondsFormat::Millis, true),
            "producer": {
                "component": "coven-daemon",
                "instanceId": "local-authority",
                "implementationVersion": env!("CARGO_PKG_VERSION")
            },
            "privacy": {
                "classification": "operational",
                "retention": {"classification": "standard"}
            }
        });
        let digest = sha256_hex(&canonicalize_without_integrity(&value).unwrap());
        value["integrity"] = json!({
            "algorithm": "sha256",
            "canonicalization": "jcs-rfc8785",
            "value": digest,
            "authentication": "none"
        });
        serde_json::from_value(value).unwrap()
    }

    fn make_authorized_receipt(fixture: &Fixture, receipt_id: &str) -> AutomationReceipt {
        mutate_receipt(make_receipt(fixture, receipt_id), |value| {
            value["authority"] = json!({
                "principal": {"principalId": "principal:val"},
                "approval": {
                    "approvalPolicyRef": "policy://authority/familiars/charm",
                    "approvalRecordRef": "approval:daily-notes-1"
                }
            });
            value["runtime"] = json!({
                "runtimeId": "coven-code",
                "capabilities": ["analysis.read", "artifact.write"]
            });
            value["exercisedCapabilities"] = json!(["analysis.read", "artifact.write"]);
            value["sideEffectClass"] = json!("none");
        })
    }

    fn mutate_receipt(
        receipt: AutomationReceipt,
        mutate: impl FnOnce(&mut Value),
    ) -> AutomationReceipt {
        let mut value = serde_json::to_value(receipt).unwrap();
        mutate(&mut value);
        value["integrity"]["value"] =
            json!(sha256_hex(&canonicalize_without_integrity(&value).unwrap()));
        serde_json::from_value(value).unwrap()
    }

    fn authority_extensions_for(
        definition_digest: &str,
        receipt: Option<&AutomationReceipt>,
    ) -> ExtensionBag {
        let mut value = authority_extensions_value();
        let extension = &mut value[AUTHORITY_EXTENSION_KEY];
        let binding = &mut extension["executionBinding"];
        binding["base"]["automationId"] = json!("daily");
        binding["base"]["automationRevision"] = json!(1);
        binding["base"]["definitionDigest"]["value"] = json!(definition_digest);
        binding["base"]["occurrenceId"] = json!("occurrence-daily-1");
        binding["base"]["occurrenceFenceGeneration"] = json!(1);
        binding["base"]["runId"] = json!("run-daily-1");
        binding["base"]["attemptId"] = json!("attempt-daily-1");
        binding["base"]["attemptNumber"] = json!(1);
        binding["base"]["adoptionKey"] = json!("automation:run-daily-1:1");
        binding["approval"]["use"]["occurrencePrefix"] = json!("occurrence-");
        binding["approval"]["consumption"]["occurrenceId"] = json!("occurrence-daily-1");
        binding["approval"]["consumption"]["runId"] = json!("run-daily-1");
        binding["approval"]["consumption"]["attemptNumber"] = json!(1);
        binding["approval"]["consumption"]["fenceGeneration"] = json!(1);
        binding["risk"]["sideEffectClass"] = json!("external_mutation");
        binding["runtime"]["runtimeId"] = json!("coven-code");

        if let Some(receipt) = receipt {
            let evidence = &mut extension["receiptEvidence"];
            evidence["receiptId"] = json!(receipt.receipt_id.as_str());
            evidence["automationId"] = json!(receipt.automation_id.as_str());
            evidence["automationRevision"] = json!(receipt.automation_revision.get());
            evidence["definitionDigest"] =
                serde_json::to_value(receipt.definition_digest.as_ref().unwrap()).unwrap();
            evidence["occurrenceId"] = json!(receipt.occurrence_id.as_str());
            evidence["occurrenceFenceGeneration"] =
                json!(receipt.occurrence_fence_generation.unwrap().get());
            evidence["runId"] = json!(receipt.run_id.as_str());
            evidence["attemptId"] = json!(receipt.attempt_id.as_str());
            evidence["attemptNumber"] = json!(receipt.attempt_number.unwrap().get());
            evidence["baseReceiptDigest"]["value"] = json!(receipt.integrity.value.as_str());
            evidence["approval"]["use"]["occurrencePrefix"] = json!("occurrence-");
            evidence["approval"]["consumption"]["occurrenceId"] =
                json!(receipt.occurrence_id.as_str());
            evidence["approval"]["consumption"]["runId"] = json!(receipt.run_id.as_str());
            evidence["approval"]["consumption"]["attemptNumber"] =
                json!(receipt.attempt_number.unwrap().get());
            evidence["approval"]["consumption"]["fenceGeneration"] =
                json!(receipt.occurrence_fence_generation.unwrap().get());
            evidence["risk"]["sideEffectClass"] = json!("external_mutation");
            evidence["runtime"]["runtimeId"] = json!("coven-code");
        } else {
            extension["receiptEvidence"] = Value::Null;
        }
        resign_authority_extensions(&mut value);
        extension_bag(value)
    }

    fn terminal_authority(fixture: &Fixture, receipt: &AutomationReceipt) -> ExtensionBag {
        authority_extensions_for(&fixture.definition_digest, Some(receipt))
    }

    fn mutate_authority(extensions: ExtensionBag, mutate: impl FnOnce(&mut Value)) -> ExtensionBag {
        let mut value = serde_json::to_value(extensions).unwrap();
        mutate(&mut value);
        resign_authority_extensions(&mut value);
        extension_bag(value)
    }

    #[derive(Debug)]
    struct RefusingVerifier;

    impl AuthorityEvidenceVerifier for RefusingVerifier {
        fn verify(
            &self,
            _extension: &AutomationAuthorityExtension,
            _phase: AuthorityValidationPhase,
        ) -> Result<(), AuthorityProfileError> {
            Err(AuthorityProfileError::new(
                AuthorityProfileErrorCode::Stale,
                "private verifier detail",
            ))
        }
    }

    #[derive(Debug, Default)]
    struct TerminalOnlyVerifier {
        phases: Mutex<Vec<AuthorityValidationPhase>>,
    }

    impl AuthorityEvidenceVerifier for TerminalOnlyVerifier {
        fn verify(
            &self,
            _extension: &AutomationAuthorityExtension,
            phase: AuthorityValidationPhase,
        ) -> Result<(), AuthorityProfileError> {
            self.phases.lock().unwrap().push(phase);
            if phase == AuthorityValidationPhase::PreDispatch {
                return Err(AuthorityProfileError::new(
                    AuthorityProfileErrorCode::Replayed,
                    "the dispatch evidence is already consumed",
                ));
            }
            Ok(())
        }
    }

    fn receipt_event(receipt: &AutomationReceipt, sequence: u64) -> EventEnvelope {
        let mut value = json!({
            "schemaVersion": "coven.automations.v1",
            "eventId": "evt00000000000000000000000000001",
            "stream": {"kind": "run", "id": receipt.run_id.as_str()},
            "sequence": sequence,
            "recordedAt": receipt.produced_at.as_str(),
            "observedAt": receipt.produced_at.as_str(),
            "producer": {
                "component": "coven-daemon",
                "instanceId": "local-authority",
                "implementationVersion": env!("CARGO_PKG_VERSION")
            },
            "automationId": receipt.automation_id.as_str(),
            "occurrenceId": receipt.occurrence_id.as_str(),
            "runId": receipt.run_id.as_str(),
            "attemptId": receipt.attempt_id.as_str(),
            "kind": "receipt.recorded",
            "summary": "automation run receipt recorded",
            "payload": {
                "receiptRef": receipt.receipt_id.as_str(),
                "outcome": "succeeded",
                "sideEffectClass": receipt.side_effect_class
            },
            "privacy": {
                "classification": &receipt.privacy.classification,
                "retention": &receipt.privacy.retention
            }
        });
        value["integrity"] = json!({
            "algorithm": "sha256",
            "canonicalization": "jcs-rfc8785",
            "value": sha256_hex(&canonicalize_without_integrity(&value).unwrap())
        });
        serde_json::from_value(value).unwrap()
    }

    fn receipt_commit_state(
        fixture: &Fixture,
        receipt: &AutomationReceipt,
        event: &EventEnvelope,
    ) -> ReceiptCommitState {
        ReceiptCommitState {
            receipt_row_count: fixture
                .conn
                .query_row(
                    "SELECT COUNT(*) FROM automation_receipts WHERE id = ?1",
                    [receipt.receipt_id.as_str()],
                    |row| row.get(0),
                )
                .unwrap(),
            receipt_table_count: fixture
                .conn
                .query_row("SELECT COUNT(*) FROM automation_receipts", [], |row| {
                    row.get(0)
                })
                .unwrap(),
            run_receipt_id: fixture
                .conn
                .query_row(
                    "SELECT receipt_id FROM automation_runs WHERE id = ?1",
                    [receipt.run_id.as_str()],
                    |row| row.get(0),
                )
                .unwrap(),
            event_row_count: fixture
                .conn
                .query_row(
                    "SELECT COUNT(*) FROM automation_events WHERE event_id = ?1",
                    [event.event_id.as_str()],
                    |row| row.get(0),
                )
                .unwrap(),
            event_table_count: fixture
                .conn
                .query_row("SELECT COUNT(*) FROM automation_events", [], |row| {
                    row.get(0)
                })
                .unwrap(),
            stream_head: stream_head(&fixture.conn, "run", receipt.run_id.as_str()).unwrap(),
            sidecar_row_count: fixture
                .conn
                .query_row(
                    "SELECT COUNT(*) FROM automation_receipt_authority_extensions
                     WHERE receipt_id = ?1",
                    [receipt.receipt_id.as_str()],
                    |row| row.get(0),
                )
                .unwrap(),
            sidecar_table_count: fixture
                .conn
                .query_row(
                    "SELECT COUNT(*) FROM automation_receipt_authority_extensions",
                    [],
                    |row| row.get(0),
                )
                .unwrap(),
        }
    }

    fn assert_authorized_failure_is_atomic(
        fixture: &Fixture,
        receipt: &AutomationReceipt,
        event: &EventEnvelope,
        extensions: &ExtensionBag,
        verifier: &dyn AuthorityEvidenceVerifier,
    ) -> anyhow::Error {
        let before = receipt_commit_state(fixture, receipt, event);
        let error = commit_authorized_receipt(&fixture.conn, receipt, event, extensions, verifier)
            .expect_err("invalid authorized commitment must fail closed");
        let after = receipt_commit_state(fixture, receipt, event);
        assert_eq!(after, before, "rejected commitment must be atomic");
        error
    }

    fn corrupt_pinned_authority(fixture: &Fixture, mutate: impl FnOnce(&mut Value)) {
        let pinned_json: String = fixture
            .conn
            .query_row(
                "SELECT authority_extension_json
                 FROM automation_attempts
                 WHERE id = 'attempt-daily-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let mut pinned = json!({
            AUTHORITY_EXTENSION_KEY: serde_json::from_str::<Value>(&pinned_json).unwrap()
        });
        mutate(&mut pinned);
        fixture
            .conn
            .execute_batch(
                "DROP TRIGGER automation_attempts_terminal_immutable;
                 DROP TRIGGER automation_attempt_authority_immutable;",
            )
            .unwrap();
        fixture
            .conn
            .execute(
                "UPDATE automation_attempts
                 SET authority_extension_json = ?2
                 WHERE id = ?1",
                rusqlite::params![
                    "attempt-daily-1",
                    serde_json::to_string(&pinned[AUTHORITY_EXTENSION_KEY]).unwrap()
                ],
            )
            .unwrap();
    }

    fn mutate_reopened_authority_claim(fixture: &Fixture, claim: &str) {
        let pinned_json: String = fixture
            .conn
            .query_row(
                "SELECT authority_extension_json
                 FROM automation_attempts
                 WHERE id = 'attempt-daily-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let sidecar_json: String = fixture
            .conn
            .query_row(
                "SELECT authority_json
                 FROM automation_receipt_authority_extensions
                 WHERE receipt_id = 'receipt-daily-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let mut pinned = json!({
            AUTHORITY_EXTENSION_KEY: serde_json::from_str::<Value>(&pinned_json).unwrap()
        });
        let mut sidecar = json!({
            AUTHORITY_EXTENSION_KEY: serde_json::from_str::<Value>(&sidecar_json).unwrap()
        });
        match claim {
            "exercised" => {
                sidecar[AUTHORITY_EXTENSION_KEY]["receiptEvidence"]["capabilities"]["exercised"] =
                    json!(["analysis.read"]);
            }
            "principal" => {
                pinned[AUTHORITY_EXTENSION_KEY]["executionBinding"]["principal"]["principalId"] =
                    json!("principal:other");
                sidecar[AUTHORITY_EXTENSION_KEY]["executionBinding"]["principal"]["principalId"] =
                    json!("principal:other");
                sidecar[AUTHORITY_EXTENSION_KEY]["receiptEvidence"]["principalId"] =
                    json!("principal:other");
            }
            "approval" => {
                pinned[AUTHORITY_EXTENSION_KEY]["executionBinding"]["approval"]["evidence"]
                    ["approvalId"] = json!("approval:other");
                sidecar[AUTHORITY_EXTENSION_KEY]["executionBinding"]["approval"]["evidence"]
                    ["approvalId"] = json!("approval:other");
                sidecar[AUTHORITY_EXTENSION_KEY]["receiptEvidence"]["approval"]["evidence"]
                    ["approvalId"] = json!("approval:other");
            }
            "runtime-id" => {
                pinned[AUTHORITY_EXTENSION_KEY]["executionBinding"]["runtime"]["runtimeId"] =
                    json!("runtime:other");
                sidecar[AUTHORITY_EXTENSION_KEY]["executionBinding"]["runtime"]["runtimeId"] =
                    json!("runtime:other");
                sidecar[AUTHORITY_EXTENSION_KEY]["receiptEvidence"]["runtime"]["runtimeId"] =
                    json!("runtime:other");
            }
            "runtime-capabilities" => {
                let expanded = json!(["analysis.read", "artifact.write", "network.publish"]);
                pinned[AUTHORITY_EXTENSION_KEY]["executionBinding"]["runtime"]["capabilities"] =
                    expanded.clone();
                sidecar[AUTHORITY_EXTENSION_KEY]["executionBinding"]["runtime"]["capabilities"] =
                    expanded.clone();
                sidecar[AUTHORITY_EXTENSION_KEY]["receiptEvidence"]["runtime"]["capabilities"] =
                    expanded;
            }
            "side-effect-envelope" => {
                pinned[AUTHORITY_EXTENSION_KEY]["executionBinding"]["risk"]["sideEffectClass"] =
                    json!("local_write");
                sidecar[AUTHORITY_EXTENSION_KEY]["executionBinding"]["risk"]["sideEffectClass"] =
                    json!("local_write");
                sidecar[AUTHORITY_EXTENSION_KEY]["receiptEvidence"]["risk"]["sideEffectClass"] =
                    json!("local_write");
            }
            _ => unreachable!(),
        }
        resign_authority_extensions(&mut pinned);
        resign_authority_extensions(&mut sidecar);
        let extension = &sidecar[AUTHORITY_EXTENSION_KEY];
        let binding_digest = extension["executionBinding"]["integrity"]["value"]
            .as_str()
            .unwrap();
        let terminal_digest = extension["receiptEvidence"]["integrity"]["value"]
            .as_str()
            .unwrap();

        fixture
            .conn
            .execute_batch(
                "DROP TRIGGER automation_attempts_terminal_immutable;
                 DROP TRIGGER automation_attempt_authority_immutable;
                 DROP TRIGGER automation_receipt_authority_extensions_no_update;",
            )
            .unwrap();
        fixture
            .conn
            .execute(
                "UPDATE automation_attempts
                 SET authority_extension_json = ?2
                 WHERE id = ?1",
                rusqlite::params![
                    "attempt-daily-1",
                    serde_json::to_string(&pinned[AUTHORITY_EXTENSION_KEY]).unwrap()
                ],
            )
            .unwrap();
        fixture
            .conn
            .execute(
                "UPDATE automation_receipt_authority_extensions
                 SET binding_digest = ?2,
                     terminal_evidence_digest = ?3,
                     authority_json = ?4
                 WHERE receipt_id = ?1",
                rusqlite::params![
                    "receipt-daily-1",
                    binding_digest,
                    terminal_digest,
                    serde_json::to_string(extension).unwrap(),
                ],
            )
            .unwrap();
    }

    #[test]
    fn receipt_commit_is_atomic_immutable_and_replay_safe() {
        let fixture = fixture();
        let receipt = make_receipt(&fixture, "receipt-daily-1");
        let event = receipt_event(&receipt, 0);

        assert_eq!(
            commit_receipt(&fixture.conn, &receipt, &event).unwrap(),
            ReceiptCommitOutcome::Committed
        );
        assert_eq!(
            commit_receipt(&fixture.conn, &receipt, &event).unwrap(),
            ReceiptCommitOutcome::Replayed
        );

        let (run_receipt_id, receipt_json): (String, String) = fixture
            .conn
            .query_row(
                "SELECT run.receipt_id, receipt.receipt_json
                 FROM automation_runs AS run
                 JOIN automation_receipts AS receipt ON receipt.id = run.receipt_id
                 WHERE run.id = 'run-daily-1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(run_receipt_id, receipt.receipt_id.as_str());
        assert_eq!(
            serde_json::from_str::<AutomationReceipt>(&receipt_json).unwrap(),
            receipt
        );
        let event_count: i64 = fixture
            .conn
            .query_row(
                "SELECT COUNT(*) FROM automation_events
                 WHERE stream_kind = 'run' AND stream_id = 'run-daily-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(event_count, 1);
        assert!(fixture
            .conn
            .execute(
                "UPDATE automation_receipts SET receipt_json = '{}' WHERE id = ?1",
                [receipt.receipt_id.as_str()],
            )
            .is_err());
        assert!(fixture
            .conn
            .execute(
                "DELETE FROM automation_receipts WHERE id = ?1",
                [receipt.receipt_id.as_str()],
            )
            .is_err());
        assert!(fixture
            .conn
            .execute(
                "UPDATE automation_runs SET receipt_id = NULL WHERE id = 'run-daily-1'",
                [],
            )
            .is_err());
    }

    #[test]
    fn authorized_receipt_commit_reopens_with_validated_terminal_sidecar() {
        let fixture = authorized_fixture();
        let receipt = make_authorized_receipt(&fixture, "receipt-daily-1");
        let event = receipt_event(&receipt, 0);
        let terminal = terminal_authority(&fixture, &receipt);
        let pinned_before: String = fixture
            .conn
            .query_row(
                "SELECT authority_extension_json
                 FROM automation_attempts
                 WHERE id = 'attempt-daily-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();

        assert_eq!(
            commit_authorized_receipt(
                &fixture.conn,
                &receipt,
                &event,
                &terminal,
                &AcceptingVerifier,
            )
            .unwrap(),
            ReceiptCommitOutcome::Committed
        );
        assert_eq!(
            receipt_commit_state(&fixture, &receipt, &event).sidecar_row_count,
            1
        );

        let reopened = crate::store::open_store(&fixture.store_path).unwrap();
        let authorized =
            read_authorized_receipt(&reopened, receipt.receipt_id.as_str(), &AcceptingVerifier)
                .unwrap()
                .expect("authorized receipt");
        assert_eq!(authorized.receipt, receipt);
        assert_eq!(
            serde_json::to_value(authorized.authority).unwrap(),
            serde_json::to_value(&terminal).unwrap()[AUTHORITY_EXTENSION_KEY]
        );
        let pinned_after: String = reopened
            .query_row(
                "SELECT authority_extension_json
                 FROM automation_attempts
                 WHERE id = 'attempt-daily-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(pinned_after, pinned_before);
    }

    #[test]
    fn authorized_commit_and_read_only_apply_live_terminal_verification() {
        let fixture = authorized_fixture();
        let receipt = make_authorized_receipt(&fixture, "receipt-daily-1");
        let event = receipt_event(&receipt, 0);
        let terminal = terminal_authority(&fixture, &receipt);
        let commit_verifier = TerminalOnlyVerifier::default();
        commit_authorized_receipt(&fixture.conn, &receipt, &event, &terminal, &commit_verifier)
            .unwrap();
        assert_eq!(
            *commit_verifier.phases.lock().unwrap(),
            vec![AuthorityValidationPhase::Terminal]
        );

        let read_verifier = TerminalOnlyVerifier::default();
        read_authorized_receipt(&fixture.conn, receipt.receipt_id.as_str(), &read_verifier)
            .unwrap();
        assert_eq!(
            *read_verifier.phases.lock().unwrap(),
            vec![AuthorityValidationPhase::Terminal]
        );
    }

    #[test]
    fn authority_sidecar_is_immutable_and_unique_per_receipt_run_and_attempt() {
        let fixture = authorized_fixture();
        let receipt = make_authorized_receipt(&fixture, "receipt-daily-1");
        let event = receipt_event(&receipt, 0);
        let terminal = terminal_authority(&fixture, &receipt);
        commit_authorized_receipt(
            &fixture.conn,
            &receipt,
            &event,
            &terminal,
            &AcceptingVerifier,
        )
        .unwrap();

        assert!(fixture
            .conn
            .execute(
                "UPDATE automation_receipt_authority_extensions
                 SET produced_at = produced_at || 'x'
                 WHERE receipt_id = ?1",
                [receipt.receipt_id.as_str()],
            )
            .is_err());
        assert!(fixture
            .conn
            .execute(
                "DELETE FROM automation_receipt_authority_extensions
                 WHERE receipt_id = ?1",
                [receipt.receipt_id.as_str()],
            )
            .is_err());
        assert!(fixture
            .conn
            .execute(
                "INSERT INTO automation_receipt_authority_extensions (
                    receipt_id, run_id, attempt_id, binding_id, binding_digest,
                    base_receipt_digest, terminal_evidence_digest,
                    authority_json, produced_at
                 )
                 SELECT receipt_id, run_id, attempt_id, binding_id, binding_digest,
                        base_receipt_digest, terminal_evidence_digest,
                        authority_json, produced_at
                 FROM automation_receipt_authority_extensions
                 WHERE receipt_id = ?1",
                [receipt.receipt_id.as_str()],
            )
            .is_err());
        assert_eq!(
            fixture
                .conn
                .query_row(
                    "SELECT COUNT(*) FROM automation_receipt_authority_extensions",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            1
        );
        for column in ["receipt_id", "run_id", "attempt_id"] {
            let unique_index_count: i64 = fixture
                .conn
                .query_row(
                    "SELECT COUNT(*)
                     FROM pragma_index_list('automation_receipt_authority_extensions') AS indexes
                     WHERE indexes.\"unique\" = 1
                       AND (
                           SELECT group_concat(name, ',')
                           FROM pragma_index_info(indexes.name)
                       ) = ?1",
                    [column],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(unique_index_count, 1, "missing UNIQUE({column})");
        }
    }

    #[test]
    fn authorized_receipt_requires_exact_base_digest_correlation() {
        let fixture = authorized_fixture();
        let receipt = make_authorized_receipt(&fixture, "receipt-daily-1");
        let event = receipt_event(&receipt, 0);
        let terminal = mutate_authority(terminal_authority(&fixture, &receipt), |value| {
            value[AUTHORITY_EXTENSION_KEY]["receiptEvidence"]["baseReceiptDigest"]["value"] =
                json!("0".repeat(64));
        });

        assert_authorized_failure_is_atomic(
            &fixture,
            &receipt,
            &event,
            &terminal,
            &AcceptingVerifier,
        );
    }

    #[test]
    fn authorized_receipt_accepts_lower_or_equal_actual_side_effects() {
        for actual in ["none", "external_mutation"] {
            let fixture = authorized_fixture();
            let receipt = mutate_receipt(
                make_authorized_receipt(&fixture, "receipt-daily-1"),
                |value| value["sideEffectClass"] = json!(actual),
            );
            let event = receipt_event(&receipt, 0);
            let terminal = terminal_authority(&fixture, &receipt);

            assert_eq!(
                commit_authorized_receipt(
                    &fixture.conn,
                    &receipt,
                    &event,
                    &terminal,
                    &AcceptingVerifier,
                )
                .unwrap(),
                ReceiptCommitOutcome::Committed,
                "{actual}"
            );
        }
    }

    #[test]
    fn authorized_receipt_rejects_initial_side_effect_escalation_atomically() {
        let fixture = authorized_fixture();
        let receipt = mutate_receipt(
            make_authorized_receipt(&fixture, "receipt-daily-1"),
            |value| value["sideEffectClass"] = json!("irreversible_external_mutation"),
        );
        let event = receipt_event(&receipt, 0);
        let terminal = terminal_authority(&fixture, &receipt);

        assert_authorized_failure_is_atomic(
            &fixture,
            &receipt,
            &event,
            &terminal,
            &AcceptingVerifier,
        );
    }

    #[test]
    fn authorized_receipt_rejects_a_spliced_execution_binding() {
        let fixture = authorized_fixture();
        let receipt = make_authorized_receipt(&fixture, "receipt-daily-1");
        let event = receipt_event(&receipt, 0);
        let terminal = mutate_authority(terminal_authority(&fixture, &receipt), |value| {
            value[AUTHORITY_EXTENSION_KEY]["executionBinding"]["bindingId"] =
                json!("binding:spliced");
            value[AUTHORITY_EXTENSION_KEY]["receiptEvidence"]["bindingId"] =
                json!("binding:spliced");
        });

        assert_authorized_failure_is_atomic(
            &fixture,
            &receipt,
            &event,
            &terminal,
            &AcceptingVerifier,
        );
    }

    #[test]
    fn authorized_receipt_rejects_missing_or_malformed_terminal_evidence() {
        for malformed in [
            Value::Null,
            json!("private malformed terminal evidence"),
            json!({"kind": "AutomationReceiptAuthorityEvidence"}),
        ] {
            let fixture = authorized_fixture();
            let receipt = make_authorized_receipt(&fixture, "receipt-daily-1");
            let event = receipt_event(&receipt, 0);
            let mut value = serde_json::to_value(terminal_authority(&fixture, &receipt)).unwrap();
            value[AUTHORITY_EXTENSION_KEY]["receiptEvidence"] = malformed;
            let terminal = extension_bag(value);

            let error = assert_authorized_failure_is_atomic(
                &fixture,
                &receipt,
                &event,
                &terminal,
                &AcceptingVerifier,
            );
            assert!(
                !format!("{error:#}").contains("private malformed terminal evidence"),
                "{error:#}"
            );
        }
    }

    #[test]
    fn authorized_receipt_requires_verifier_acceptance() {
        let fixture = authorized_fixture();
        let receipt = make_authorized_receipt(&fixture, "receipt-daily-1");
        let event = receipt_event(&receipt, 0);
        let terminal = terminal_authority(&fixture, &receipt);

        let error = assert_authorized_failure_is_atomic(
            &fixture,
            &receipt,
            &event,
            &terminal,
            &RefusingVerifier,
        );
        assert!(!format!("{error:#}").contains("private verifier detail"));
    }

    #[test]
    fn authorized_receipt_replay_is_exact_and_conflicts_fail_closed() {
        let fixture = authorized_fixture();
        let receipt = make_authorized_receipt(&fixture, "receipt-daily-1");
        let event = receipt_event(&receipt, 0);
        let terminal = terminal_authority(&fixture, &receipt);

        assert_eq!(
            commit_authorized_receipt(
                &fixture.conn,
                &receipt,
                &event,
                &terminal,
                &AcceptingVerifier,
            )
            .unwrap(),
            ReceiptCommitOutcome::Committed
        );
        assert_eq!(
            commit_authorized_receipt(
                &fixture.conn,
                &receipt,
                &event,
                &terminal,
                &AcceptingVerifier,
            )
            .unwrap(),
            ReceiptCommitOutcome::Replayed
        );

        let conflicting = mutate_authority(terminal.clone(), |value| {
            value[AUTHORITY_EXTENSION_KEY]["executionBinding"]["principal"]["principalId"] =
                json!("principal:other");
            value[AUTHORITY_EXTENSION_KEY]["receiptEvidence"]["principalId"] =
                json!("principal:other");
        });
        assert_authorized_failure_is_atomic(
            &fixture,
            &receipt,
            &event,
            &conflicting,
            &AcceptingVerifier,
        );
        assert_eq!(
            receipt_commit_state(&fixture, &receipt, &event).sidecar_row_count,
            1
        );
    }

    #[test]
    fn base_only_commit_rejects_authority_pinned_runs_before_fresh_or_replay_acceptance() {
        let fixture = authorized_fixture();
        let receipt = make_authorized_receipt(&fixture, "receipt-daily-1");
        let event = receipt_event(&receipt, 0);
        let before = receipt_commit_state(&fixture, &receipt, &event);
        assert!(commit_receipt(&fixture.conn, &receipt, &event).is_err());
        assert_eq!(receipt_commit_state(&fixture, &receipt, &event), before);

        let terminal = terminal_authority(&fixture, &receipt);
        commit_authorized_receipt(
            &fixture.conn,
            &receipt,
            &event,
            &terminal,
            &AcceptingVerifier,
        )
        .unwrap();
        assert!(commit_receipt(&fixture.conn, &receipt, &event).is_err());
        assert_eq!(
            receipt_commit_state(&fixture, &receipt, &event).sidecar_row_count,
            1
        );
    }

    #[test]
    fn structural_v1_base_receipt_commit_remains_available() {
        let fixture = fixture();
        let receipt = make_receipt(&fixture, "receipt-daily-1");
        let event = receipt_event(&receipt, 0);

        assert_eq!(
            commit_receipt(&fixture.conn, &receipt, &event).unwrap(),
            ReceiptCommitOutcome::Committed
        );
        assert_eq!(
            receipt_commit_state(&fixture, &receipt, &event).sidecar_table_count,
            0
        );
    }

    #[test]
    fn authorized_receipt_rolls_back_base_event_head_and_sidecar_when_sidecar_insert_fails() {
        let fixture = authorized_fixture();
        let receipt = make_authorized_receipt(&fixture, "receipt-daily-1");
        let event = receipt_event(&receipt, 0);
        let terminal = terminal_authority(&fixture, &receipt);
        fixture
            .conn
            .execute_batch(
                "CREATE TRIGGER fail_authority_sidecar_insert
                 BEFORE INSERT ON automation_receipt_authority_extensions
                 BEGIN
                     SELECT RAISE(ABORT, 'injected sidecar failure');
                 END;",
            )
            .unwrap();

        assert_authorized_failure_is_atomic(
            &fixture,
            &receipt,
            &event,
            &terminal,
            &AcceptingVerifier,
        );
    }

    #[test]
    fn authorized_receipt_base_validation_and_event_append_failures_are_atomic() {
        for failure in ["missing-integrity", "out-of-order"] {
            let fixture = authorized_fixture();
            let receipt = make_authorized_receipt(&fixture, "receipt-daily-1");
            let mut event = receipt_event(&receipt, u64::from(failure == "out-of-order"));
            if failure == "missing-integrity" {
                event.integrity = None;
            }
            let terminal = terminal_authority(&fixture, &receipt);

            assert_authorized_failure_is_atomic(
                &fixture,
                &receipt,
                &event,
                &terminal,
                &AcceptingVerifier,
            );
        }
    }

    #[test]
    fn authorized_receipt_initial_claim_contradictions_fail_atomically() {
        for claim in [
            "missing-exercised",
            "exercised",
            "principal",
            "approval",
            "runtime-id",
            "runtime-capabilities",
        ] {
            let fixture = authorized_fixture();
            let receipt = mutate_receipt(
                make_authorized_receipt(&fixture, "receipt-daily-1"),
                |value| match claim {
                    "missing-exercised" => {
                        value
                            .as_object_mut()
                            .unwrap()
                            .remove("exercisedCapabilities");
                    }
                    "exercised" => {
                        value["exercisedCapabilities"] = json!(["analysis.read"]);
                    }
                    "principal" => {
                        value["authority"]["principal"]["principalId"] = json!("principal:other");
                    }
                    "approval" => {
                        value["authority"]["approval"]["approvalRecordRef"] =
                            json!("approval:other");
                    }
                    "runtime-id" => {
                        value["runtime"]["runtimeId"] = json!("runtime:other");
                    }
                    "runtime-capabilities" => {
                        value["runtime"]["capabilities"] =
                            json!(["analysis.read", "artifact.write", "network.publish"]);
                    }
                    _ => unreachable!(),
                },
            );
            let event = receipt_event(&receipt, 0);
            let terminal = terminal_authority(&fixture, &receipt);

            assert_authorized_failure_is_atomic(
                &fixture,
                &receipt,
                &event,
                &terminal,
                &AcceptingVerifier,
            );
        }
    }

    #[test]
    fn authorized_receipt_reopen_rejects_contradictory_overlapping_claims() {
        for claim in [
            "exercised",
            "principal",
            "approval",
            "runtime-id",
            "runtime-capabilities",
            "side-effect-envelope",
        ] {
            let fixture = authorized_fixture();
            let receipt = if claim == "side-effect-envelope" {
                mutate_receipt(
                    make_authorized_receipt(&fixture, "receipt-daily-1"),
                    |value| value["sideEffectClass"] = json!("external_mutation"),
                )
            } else {
                make_authorized_receipt(&fixture, "receipt-daily-1")
            };
            let event = receipt_event(&receipt, 0);
            let terminal = terminal_authority(&fixture, &receipt);
            commit_authorized_receipt(
                &fixture.conn,
                &receipt,
                &event,
                &terminal,
                &AcceptingVerifier,
            )
            .unwrap();
            mutate_reopened_authority_claim(&fixture, claim);

            let error = read_authorized_receipt(
                &fixture.conn,
                receipt.receipt_id.as_str(),
                &AcceptingVerifier,
            )
            .expect_err("contradictory reopened claims must fail closed");
            let expected_error = if claim == "side-effect-envelope" {
                "side effect"
            } else {
                "authority"
            };
            assert!(
                format!("{error:#}").contains(expected_error),
                "{claim}: {error:#}"
            );
        }
    }

    #[test]
    fn authorized_receipt_read_and_replay_reject_tampered_sidecar_json_and_indexes() {
        for corrupt_sql in [
            "PRAGMA foreign_keys = OFF;
             DROP TRIGGER automation_receipt_authority_extensions_no_update;
             UPDATE automation_receipt_authority_extensions
             SET authority_json = json_set(
                 authority_json,
                 '$.receiptEvidence.principalId',
                 'principal:tampered'
             );
             PRAGMA foreign_keys = ON;",
            "PRAGMA foreign_keys = OFF;
             DROP TRIGGER automation_receipt_authority_extensions_no_update;
             UPDATE automation_receipt_authority_extensions
             SET binding_id = 'binding:tampered';
             PRAGMA foreign_keys = ON;",
            "PRAGMA foreign_keys = OFF;
             DROP TRIGGER automation_receipt_authority_extensions_no_update;
             UPDATE automation_receipt_authority_extensions
             SET binding_digest = printf('%064d', 0);
             PRAGMA foreign_keys = ON;",
            "PRAGMA foreign_keys = OFF;
             DROP TRIGGER automation_receipt_authority_extensions_no_update;
             UPDATE automation_receipt_authority_extensions
             SET base_receipt_digest = printf('%064d', 0);
             PRAGMA foreign_keys = ON;",
            "PRAGMA foreign_keys = OFF;
             DROP TRIGGER automation_receipt_authority_extensions_no_update;
             UPDATE automation_receipt_authority_extensions
             SET terminal_evidence_digest = printf('%064d', 0);
             PRAGMA foreign_keys = ON;",
            "PRAGMA foreign_keys = OFF;
             DROP TRIGGER automation_receipt_authority_extensions_no_update;
             UPDATE automation_receipt_authority_extensions
             SET run_id = 'run-moved';
             PRAGMA foreign_keys = ON;",
            "PRAGMA foreign_keys = OFF;
             DROP TRIGGER automation_receipt_authority_extensions_no_update;
             UPDATE automation_receipt_authority_extensions
             SET attempt_id = 'attempt-moved';
             PRAGMA foreign_keys = ON;",
            "PRAGMA foreign_keys = OFF;
             DROP TRIGGER automation_receipt_authority_extensions_no_update;
             UPDATE automation_receipt_authority_extensions
             SET produced_at = produced_at || 'x';
             PRAGMA foreign_keys = ON;",
            "PRAGMA foreign_keys = OFF;
             DROP TRIGGER automation_receipt_authority_extensions_no_update;
             UPDATE automation_receipt_authority_extensions
             SET receipt_id = 'receipt-moved';
             PRAGMA foreign_keys = ON;",
        ] {
            let fixture = authorized_fixture();
            let receipt = make_authorized_receipt(&fixture, "receipt-daily-1");
            let event = receipt_event(&receipt, 0);
            let terminal = terminal_authority(&fixture, &receipt);
            commit_authorized_receipt(
                &fixture.conn,
                &receipt,
                &event,
                &terminal,
                &AcceptingVerifier,
            )
            .unwrap();
            fixture.conn.execute_batch(corrupt_sql).unwrap();

            assert!(
                read_authorized_receipt(
                    &fixture.conn,
                    receipt.receipt_id.as_str(),
                    &AcceptingVerifier,
                )
                .is_err(),
                "{corrupt_sql}"
            );
            assert!(
                commit_authorized_receipt(
                    &fixture.conn,
                    &receipt,
                    &event,
                    &terminal,
                    &AcceptingVerifier,
                )
                .is_err(),
                "{corrupt_sql}"
            );
        }
    }

    #[test]
    fn authorized_receipt_read_rejects_malformed_sidecar_without_disclosure() {
        let fixture = authorized_fixture();
        let receipt = make_authorized_receipt(&fixture, "receipt-daily-1");
        let event = receipt_event(&receipt, 0);
        let terminal = terminal_authority(&fixture, &receipt);
        commit_authorized_receipt(
            &fixture.conn,
            &receipt,
            &event,
            &terminal,
            &AcceptingVerifier,
        )
        .unwrap();
        fixture
            .conn
            .execute_batch(
                "DROP TRIGGER automation_receipt_authority_extensions_no_update;
                 PRAGMA ignore_check_constraints = ON;
                 UPDATE automation_receipt_authority_extensions
                 SET authority_json = '{private malformed authority sidecar';
                 PRAGMA ignore_check_constraints = OFF;",
            )
            .unwrap();

        let error = read_authorized_receipt(
            &fixture.conn,
            receipt.receipt_id.as_str(),
            &AcceptingVerifier,
        )
        .expect_err("malformed sidecar must fail closed");
        assert!(
            !format!("{error:#}").contains("private malformed authority sidecar"),
            "{error:#}"
        );
    }

    #[test]
    fn corrupt_pinned_authority_profile_and_enum_are_privacy_safe() {
        for corruption in ["profile", "enum", "malformed"] {
            let fixture = authorized_fixture();
            let receipt = make_authorized_receipt(&fixture, "receipt-daily-1");
            let event = receipt_event(&receipt, 0);
            let terminal = terminal_authority(&fixture, &receipt);
            let sentinel = format!("private-{corruption}-sentinel");
            if corruption == "malformed" {
                fixture
                    .conn
                    .execute_batch(
                        "DROP TRIGGER automation_attempts_terminal_immutable;
                         DROP TRIGGER automation_attempt_authority_immutable;",
                    )
                    .unwrap();
                fixture
                    .conn
                    .execute(
                        "UPDATE automation_attempts
                         SET authority_extension_json = ?2
                         WHERE id = ?1",
                        rusqlite::params!["attempt-daily-1", format!("{{{sentinel}")],
                    )
                    .unwrap();
            } else {
                corrupt_pinned_authority(&fixture, |value| match corruption {
                    "profile" => {
                        value[AUTHORITY_EXTENSION_KEY]["profile"] = json!(sentinel);
                    }
                    "enum" => {
                        value[AUTHORITY_EXTENSION_KEY]["executionBinding"]["kind"] =
                            json!(sentinel);
                    }
                    _ => unreachable!(),
                });
            }

            let error = assert_authorized_failure_is_atomic(
                &fixture,
                &receipt,
                &event,
                &terminal,
                &AcceptingVerifier,
            );
            assert!(!format!("{error:#}").contains(&sentinel), "{error:#}");
        }
    }

    fn read_response(fixture: &Fixture, authority: RequestAuthority) -> crate::api::ApiResponse {
        handle_request_with_runtime_and_authority(
            "POST",
            "/api/v1/actions",
            fixture._temp.path(),
            None,
            Some(r#"{"action":"coven.automations.receipt.get.v1","id":"receipt-daily-1"}"#),
            &NoopSessionRuntime,
            authority,
        )
        .unwrap()
    }

    #[test]
    fn automation_receipt_read_survives_reopen_without_claiming_authentication() {
        let fixture = fixture();
        let receipt = make_receipt(&fixture, "receipt-daily-1");
        let event = receipt_event(&receipt, 0);
        commit_receipt(&fixture.conn, &receipt, &event).unwrap();

        let reopened = crate::store::open_store(&fixture.store_path).unwrap();
        assert_eq!(
            read_receipt(&reopened, "receipt-daily-1").unwrap(),
            Some(receipt.clone())
        );
        let response = read_response(&fixture, RequestAuthority::OwnerLocalIpc);
        assert_eq!(response.status, 200, "{}", response.body);
        let body: Value = serde_json::from_str(&response.body).unwrap();
        assert_eq!(
            body["result"]["receipt"],
            serde_json::to_value(&receipt).unwrap()
        );
        assert_eq!(body["result"]["verification"]["status"], "unverifiable");
        assert_eq!(body["result"]["verification"]["integrity"], "valid");
        assert_eq!(
            body["result"]["verification"]["receiptAuthentication"]["status"],
            "unverified"
        );
        assert_eq!(
            body["result"]["verification"]["receiptAuthentication"]["evidence"],
            "unavailable"
        );
        assert_eq!(
            body["result"]["verification"]["runtimeAuthority"]["status"],
            "unverified"
        );
        assert_eq!(
            body["result"]["verification"]["runtimeAuthority"]["evidence"],
            "unavailable"
        );
        assert!(
            body.get("event").is_none(),
            "a read must not emit a mutation event"
        );
        let runs = crate::control_plane::route_action(
            json!({"action": "coven.automations.runs", "id": "daily"}),
            &fixture.conn,
            &NoopSessionRuntime,
        );
        assert_eq!(
            runs.1.event.unwrap().payload["runs"][0]["receiptId"],
            "receipt-daily-1"
        );
        assert_eq!(
            commit_receipt(&fixture.conn, &receipt, &event).unwrap(),
            ReceiptCommitOutcome::Replayed,
            "reads must leave committed evidence unchanged"
        );
    }

    #[test]
    fn automation_receipt_read_denies_tcp_before_id_or_store_access() {
        let temp = tempfile::tempdir().unwrap();
        for action in [
            "coven.automations.receipt.get.v1",
            " coven.automations.receipt.get.v1 ",
        ] {
            let response = handle_request_with_runtime_and_authority(
                "POST",
                "/api/v1/actions",
                temp.path(),
                None,
                Some(
                    &json!({
                        "action": action,
                        "id": null,
                        "principal": "owner",
                        "authority": "OwnerLocalIpc",
                        "origin": "local",
                        "includeSensitive": true
                    })
                    .to_string(),
                ),
                &NoopSessionRuntime,
                RequestAuthority::Tcp,
            )
            .unwrap();
            assert_eq!(response.status, 403, "{}", response.body);
            assert!(response.body.contains("AUTHORITY_REQUIRED"));
            assert!(!temp.path().join("coven.sqlite3").exists());
        }
    }

    #[test]
    fn automation_receipt_read_does_not_trust_authentication_labels() {
        for authentication in ["none", "producer-hmac", "cosign"] {
            let fixture = fixture();
            let mut value =
                serde_json::to_value(make_receipt(&fixture, "receipt-daily-1")).unwrap();
            value["integrity"]["authentication"] = json!(authentication);
            let receipt: AutomationReceipt = serde_json::from_value(value).unwrap();
            commit_receipt(&fixture.conn, &receipt, &receipt_event(&receipt, 0)).unwrap();
            let response = read_response(&fixture, RequestAuthority::OwnerLocalIpc);
            assert_eq!(response.status, 200);
            let body: Value = serde_json::from_str(&response.body).unwrap();
            assert_eq!(
                body["result"]["verification"]["receiptAuthentication"]["status"],
                "unverified"
            );
            assert_eq!(
                body["result"]["verification"]["receiptAuthentication"]["evidence"],
                "unavailable"
            );
            assert_eq!(body["result"]["verification"]["status"], "unverifiable");
        }
    }

    #[test]
    fn automation_receipt_read_rejects_malformed_ids() {
        let fixture = fixture();
        for id in [Value::Null, json!(false), json!(""), json!("x".repeat(161))] {
            let response = crate::control_plane::route_action(
                json!({"action": "coven.automations.receipt.get.v1", "id": id}),
                &fixture.conn,
                &NoopSessionRuntime,
            );
            assert_eq!(response.0, 400);
            assert_eq!(response.1.error.unwrap()["code"], "VALIDATION_FAILED");
            assert!(response.1.event.is_none());
        }
    }

    #[test]
    fn automation_receipt_read_does_not_turn_missing_or_restricted_evidence_into_success() {
        let fixture = fixture();
        let missing = read_response(&fixture, RequestAuthority::OwnerLocalIpc);
        assert_eq!(missing.status, 404);
        assert!(!missing.body.contains("\"receipt\":"));
        assert_eq!(read_receipt(&fixture.conn, "absent").unwrap(), None);

        for classification in ["sensitive", "restricted"] {
            let fixture = fixture_with_attempt_occurrence("occurrence-daily-1");
            let mut value =
                serde_json::to_value(make_receipt(&fixture, "receipt-daily-1")).unwrap();
            value["privacy"]["classification"] = json!(classification);
            value["privacy"]["notes"] = json!("private receipt content");
            value["integrity"]["value"] =
                json!(sha256_hex(&canonicalize_without_integrity(&value).unwrap()));
            let receipt: AutomationReceipt = serde_json::from_value(value).unwrap();
            commit_receipt(&fixture.conn, &receipt, &receipt_event(&receipt, 0)).unwrap();
            let response = read_response(&fixture, RequestAuthority::OwnerLocalIpc);
            assert_eq!(response.status, 403);
            assert!(!response.body.contains("private receipt content"));
            assert!(!response.body.contains("\"receipt\":"));
        }
    }

    fn assert_private_corruption_is_rejected(fixture: &Fixture, private_content: &str) {
        let error = read_receipt(&fixture.conn, "receipt-daily-1")
            .expect_err("corrupt evidence must not produce a receipt");
        assert!(!format!("{error:#}").contains(private_content));

        let response = read_response(fixture, RequestAuthority::OwnerLocalIpc);
        assert_eq!(response.status, 500, "{}", response.body);
        let body: Value = serde_json::from_str(&response.body).unwrap();
        assert_eq!(body["error"]["code"], "INTERNAL");
        assert_eq!(
            body["error"]["message"],
            "Stored automation receipt evidence could not be validated."
        );
        assert!(body["result"].is_null());
        assert!(!response.body.contains(private_content));
        assert!(!response.body.contains("\"receipt\":"));
    }

    #[test]
    fn automation_receipt_read_requires_integrity_for_non_correlation_event_fields() {
        let fixture = fixture();
        let receipt = make_receipt(&fixture, "receipt-daily-1");
        commit_receipt(&fixture.conn, &receipt, &receipt_event(&receipt, 0)).unwrap();
        fixture
            .conn
            .execute_batch(
                "DROP TRIGGER automation_events_no_update;
                 UPDATE automation_events
                 SET event_json = json_remove(
                     json_set(event_json, '$.summary', 'private tampered summary'),
                     '$.integrity'
                 );",
            )
            .unwrap();

        assert_private_corruption_is_rejected(&fixture, "private tampered summary");
    }

    #[test]
    fn automation_receipt_read_rejects_malformed_receipt_json_without_disclosure() {
        let fixture = fixture();
        let receipt = make_receipt(&fixture, "receipt-daily-1");
        commit_receipt(&fixture.conn, &receipt, &receipt_event(&receipt, 0)).unwrap();
        fixture
            .conn
            .execute_batch(
                "DROP TRIGGER automation_receipts_no_update;
                 PRAGMA ignore_check_constraints = ON;
                 UPDATE automation_receipts
                 SET receipt_json = '{private malformed receipt content';
                 PRAGMA ignore_check_constraints = OFF;",
            )
            .unwrap();

        assert_private_corruption_is_rejected(&fixture, "private malformed receipt content");
    }

    #[test]
    fn automation_receipt_read_rejects_malformed_event_json_without_disclosure() {
        let fixture = fixture();
        let receipt = make_receipt(&fixture, "receipt-daily-1");
        commit_receipt(&fixture.conn, &receipt, &receipt_event(&receipt, 0)).unwrap();
        fixture
            .conn
            .execute_batch(
                "DROP TRIGGER automation_events_no_update;
                 UPDATE automation_events
                 SET event_json = '{private malformed event content';",
            )
            .unwrap();

        assert_private_corruption_is_rejected(&fixture, "private malformed event content");
    }

    #[test]
    fn automation_receipt_read_refuses_tampered_body_and_terminal_correlation() {
        for corrupt_sql in [
            "DROP TRIGGER automation_receipts_no_update;
             UPDATE automation_receipts SET receipt_json =
                 json_set(receipt_json, '$.privacy.notes', 'private tampered content');",
            "DROP TRIGGER automation_attempts_terminal_immutable;
             UPDATE automation_attempts SET state = 'failed' WHERE id = 'attempt-daily-1';",
            "DROP TRIGGER automation_run_receipt_once;
             UPDATE automation_runs SET receipt_id = NULL WHERE id = 'run-daily-1';",
            "DROP TRIGGER automation_events_no_update;
             UPDATE automation_events SET event_json =
                 json_set(event_json, '$.payload.receiptRef', 'receipt-other');",
            "DROP TRIGGER automation_events_no_update;
             UPDATE automation_events SET stream_id = 'unrelated-run';",
            "DROP TRIGGER automation_receipts_no_update;
             UPDATE automation_receipts SET receipt_digest = printf('%064d', 0);",
            "DROP TRIGGER automation_receipts_no_update;
             UPDATE automation_receipts SET outcome = 'failed';",
            "DROP TRIGGER automation_events_no_update;
             UPDATE automation_events SET observed_at = observed_at || 'x';",
            "DROP TRIGGER automation_events_no_update;
             UPDATE automation_events SET recorded_at_millis = recorded_at_millis + 1;",
        ] {
            let fixture = fixture();
            let receipt = make_receipt(&fixture, "receipt-daily-1");
            commit_receipt(&fixture.conn, &receipt, &receipt_event(&receipt, 0)).unwrap();
            fixture.conn.execute_batch(corrupt_sql).unwrap();
            assert!(
                read_receipt(&fixture.conn, "receipt-daily-1").is_err(),
                "{corrupt_sql}"
            );
            let response = read_response(&fixture, RequestAuthority::OwnerLocalIpc);
            assert_eq!(response.status, 500, "{corrupt_sql}: {}", response.body);
            assert!(!response.body.contains("private tampered content"));
            assert!(!response.body.contains("\"receipt\":"));
        }
    }

    #[test]
    fn receipt_commit_refuses_conflict_or_correlation_mismatch() {
        let fixture = fixture();
        let receipt = make_receipt(&fixture, "receipt-daily-1");
        let event = receipt_event(&receipt, 0);
        assert_eq!(
            commit_receipt(&fixture.conn, &receipt, &event).unwrap(),
            ReceiptCommitOutcome::Committed
        );

        let conflicting = make_receipt(&fixture, "receipt-daily-2");
        let conflicting_event = receipt_event(&conflicting, 1);
        assert!(commit_receipt(&fixture.conn, &conflicting, &conflicting_event).is_err());

        let stored: String = fixture
            .conn
            .query_row(
                "SELECT receipt_id FROM automation_runs WHERE id = 'run-daily-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(stored, receipt.receipt_id.as_str());
    }

    #[test]
    fn receipt_commit_rolls_back_when_event_append_fails() {
        let fixture = fixture();
        let receipt = make_receipt(&fixture, "receipt-daily-1");
        let out_of_order_event = receipt_event(&receipt, 1);

        assert!(commit_receipt(&fixture.conn, &receipt, &out_of_order_event).is_err());
        let receipt_count: i64 = fixture
            .conn
            .query_row("SELECT COUNT(*) FROM automation_receipts", [], |row| {
                row.get(0)
            })
            .unwrap();
        let run_receipt_id: Option<String> = fixture
            .conn
            .query_row(
                "SELECT receipt_id FROM automation_runs WHERE id = 'run-daily-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(receipt_count, 0);
        assert_eq!(run_receipt_id, None);
    }

    #[test]
    fn receipt_commit_refuses_missing_event_integrity_without_writes() {
        let fixture = fixture();
        let receipt = make_receipt(&fixture, "receipt-daily-1");
        let mut event = receipt_event(&receipt, 0);
        event.integrity = None;

        let before = receipt_commit_state(&fixture, &receipt, &event);

        let result = commit_receipt(&fixture.conn, &receipt, &event);

        let after = receipt_commit_state(&fixture, &receipt, &event);

        let error = result.expect_err("receipt commitment must require event integrity");
        assert!(
            format!("{error:#}").contains("automation receipt event integrity is required"),
            "{error:#}"
        );
        assert_eq!(after, before, "rejected commitment must be atomic");
    }

    #[test]
    fn receipt_commit_requires_exact_terminal_attempt_correlation() {
        let fixture = fixture();
        let receipt = make_receipt(&fixture, "receipt-daily-1");
        let mut value = serde_json::to_value(&receipt).unwrap();
        value["attemptId"] = Value::String("attempt-other".to_string());
        let digest = sha256_hex(&canonicalize_without_integrity(&value).unwrap());
        value["integrity"]["value"] = Value::String(digest);
        let mismatched: AutomationReceipt = serde_json::from_value(value).unwrap();
        let event = receipt_event(&mismatched, 0);

        assert!(commit_receipt(&fixture.conn, &mismatched, &event).is_err());
        let receipt_count: i64 = fixture
            .conn
            .query_row("SELECT COUNT(*) FROM automation_receipts", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(receipt_count, 0);
    }

    #[test]
    fn receipt_commit_requires_attempt_and_run_to_share_the_occurrence() {
        let fixture = fixture_with_attempt_occurrence("occurrence-daily-2");
        let receipt = make_receipt(&fixture, "receipt-daily-1");
        let event = receipt_event(&receipt, 0);

        assert!(commit_receipt(&fixture.conn, &receipt, &event).is_err());
        let receipt_count: i64 = fixture
            .conn
            .query_row("SELECT COUNT(*) FROM automation_receipts", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(receipt_count, 0);
    }

    #[test]
    fn concurrent_identical_receipt_commits_converge_to_replay() {
        let fixture = fixture();
        let receipt = make_receipt(&fixture, "receipt-daily-1");
        let event = receipt_event(&receipt, 0);
        let barrier = Arc::new(Barrier::new(2));
        let mut workers = Vec::new();
        for _ in 0..2 {
            let path = fixture.store_path.clone();
            let receipt = receipt.clone();
            let event = event.clone();
            let barrier = Arc::clone(&barrier);
            workers.push(std::thread::spawn(move || {
                let conn = crate::store::open_store(&path).unwrap();
                barrier.wait();
                commit_receipt(&conn, &receipt, &event)
            }));
        }
        let mut outcomes = workers
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .collect::<anyhow::Result<Vec<_>>>()
            .unwrap();
        outcomes.sort_by_key(|outcome| match outcome {
            ReceiptCommitOutcome::Committed => 0,
            ReceiptCommitOutcome::Replayed => 1,
        });
        assert_eq!(
            outcomes,
            vec![
                ReceiptCommitOutcome::Committed,
                ReceiptCommitOutcome::Replayed
            ]
        );
    }
}
