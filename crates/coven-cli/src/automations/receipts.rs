//! Immutable Automations v1 receipt commitment.

// The next #857 producer slice will call this seam once terminal side-effect
// evidence exists; focused tests exercise the commitment invariants today.
#![allow(dead_code)]

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};

use super::contract::events::append_event;
use super::contract::types::{
    AutomationReceipt, EventEnvelope, EventKind, EventPayload, StreamKind, TerminalOutcome,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReceiptCommitOutcome {
    Committed,
    Replayed,
}

struct DurableCorrelation {
    automation_id: String,
    automation_revision: i64,
    definition_digest: Option<String>,
    occurrence_id: String,
    familiar_id: Option<String>,
    runtime: String,
    run_status: String,
    run_receipt_id: Option<String>,
    attempt_occurrence_id: String,
    attempt_number: i64,
    occurrence_fence_generation: i64,
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

fn commit_receipt_in(
    conn: &Connection,
    receipt: &AutomationReceipt,
    event: &EventEnvelope,
) -> Result<ReceiptCommitOutcome> {
    receipt
        .verify_integrity()
        .context("automation receipt integrity is invalid")?;
    event
        .verify_integrity()
        .context("automation receipt event integrity is invalid")?;
    validate_receipt_event(receipt, event)?;
    let durable = durable_correlation(conn, receipt)?;
    validate_durable_correlation(receipt, &durable)?;

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
    let result = (|| {
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
        Ok(ReceiptCommitOutcome::Committed)
    })();

    match result {
        Ok(outcome) => {
            conn.execute_batch("RELEASE SAVEPOINT coven_automation_receipt_commit")
                .context("failed to commit automation receipt")?;
            Ok(outcome)
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

fn durable_correlation(
    conn: &Connection,
    receipt: &AutomationReceipt,
) -> Result<DurableCorrelation> {
    conn.query_row(
        "SELECT run.automation_id, run.automation_revision, run.definition_digest,
                run.occurrence_id, run.familiar_id, run.runtime, run.status, run.receipt_id,
                attempt.occurrence_id, attempt.attempt_number,
                attempt.occurrence_fence_generation, attempt.state,
                attempt.failure_class, attempt.state_reason, run.finished_at, attempt.settled_at
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
                run_status: row.get(6)?,
                run_receipt_id: row.get(7)?,
                attempt_occurrence_id: row.get(8)?,
                attempt_number: row.get(9)?,
                occurrence_fence_generation: row.get(10)?,
                attempt_state: row.get(11)?,
                failure_class: row.get(12)?,
                state_reason: row.get(13)?,
                run_finished_at: row.get(14)?,
                attempt_settled_at: row.get(15)?,
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
    use super::{commit_receipt, ReceiptCommitOutcome};
    use crate::automations::contract::canonical_json::{
        canonicalize_without_integrity, sha256_hex,
    };
    use crate::automations::contract::types::{AutomationReceipt, EventEnvelope};
    use crate::automations::definition::RoutineDefinition;
    use crate::automations::occurrences::insert_claimed_occurrence;
    use crate::automations::runs::{record_run_finish, record_run_start, RunFinish, RunStart};
    use crate::automations::store::insert_definition;
    use crate::store::initialize_store;
    use chrono::{DateTime, SecondsFormat, Utc};
    use rusqlite::Connection;
    use serde_json::{json, Value};
    use std::path::PathBuf;
    use std::sync::{Arc, Barrier};

    struct Fixture {
        _temp: tempfile::TempDir,
        store_path: PathBuf,
        conn: Connection,
        definition_digest: String,
        produced_at: DateTime<Utc>,
    }

    fn fixture() -> Fixture {
        fixture_with_attempt_occurrence("occurrence-daily-1")
    }

    fn fixture_with_attempt_occurrence(attempt_occurrence_id: &str) -> Fixture {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("store.sqlite");
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
        let definition_digest = conn
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
                authority_profile: None,
                session_id: None,
                familiar_id: Some("charm"),
                runtime: "coven-code",
                timeout_at: opened_at + chrono::Duration::minutes(30),
            },
            opened_at,
        )
        .unwrap();
        conn.execute(
            "INSERT INTO automation_attempts (
                id, run_id, occurrence_id, attempt_number, adoption_key,
                occurrence_fence_generation, state, retry_classification,
                not_before, opened_at, settled_at
             ) VALUES (?1, ?2, ?3, 1, ?4, 1, 'succeeded', 'initial', ?5, ?5, ?6)",
            rusqlite::params![
                "attempt-daily-1",
                "run-daily-1",
                attempt_occurrence_id,
                "automation:run-daily-1:1",
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

    fn receipt_event(receipt: &AutomationReceipt, sequence: u64) -> EventEnvelope {
        serde_json::from_value(json!({
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
                "sideEffectClass": "external_mutation"
            },
            "privacy": {
                "classification": "operational",
                "retention": {"classification": "standard"}
            }
        }))
        .unwrap()
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
