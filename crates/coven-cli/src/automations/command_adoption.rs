//! Transactional adoption for definition-mutating automation commands.

use std::collections::BTreeMap;

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::capability_negotiation::{
    capability_profile, negotiate_definition, DefinitionNegotiation, UnsupportedVariant,
};
use super::contract::canonical_json::{canonicalize, sha256_hex};
use super::contract::error::{AdoptionConflictOutcome, ErrorAdoption, ErrorCode, ErrorEnvelope};
use super::contract::types::{AdoptionKey, PositiveInteger};
use super::definition::RoutineDefinition;

const DEFINITION_VALIDATION_FAILED_MESSAGE: &str = "automation definition failed validation";

pub const AUTOMATION_COMMAND_ADOPTIONS_SCHEMA_SQL: &str = "
    CREATE TABLE IF NOT EXISTS automation_command_reservations (
        adoption_key TEXT PRIMARY KEY NOT NULL,
        request_digest TEXT NOT NULL,
        command TEXT NOT NULL,
        reserved_at TEXT NOT NULL
    );

    CREATE TABLE IF NOT EXISTS automation_command_adoptions (
        adoption_key TEXT PRIMARY KEY NOT NULL,
        request_digest TEXT NOT NULL,
        command TEXT NOT NULL,
        automation_id TEXT,
        outcome TEXT NOT NULL CHECK (outcome IN ('committed', 'rejected')),
        revision INTEGER,
        response_json TEXT NOT NULL,
        adopted_at TEXT NOT NULL
    );

    CREATE INDEX IF NOT EXISTS idx_automation_command_adoptions_automation
        ON automation_command_adoptions(automation_id, adopted_at);

    CREATE TRIGGER IF NOT EXISTS automation_command_adoptions_immutable_columns
    BEFORE UPDATE OF
        adoption_key,
        request_digest,
        command,
        automation_id,
        outcome,
        revision,
        adopted_at
    ON automation_command_adoptions
    BEGIN
        SELECT RAISE(ABORT, 'automation command adoptions are append-only');
    END;

    CREATE TRIGGER IF NOT EXISTS automation_command_adoptions_immutable_rowid
    BEFORE UPDATE ON automation_command_adoptions
    WHEN NEW.rowid IS NOT OLD.rowid
    BEGIN
        SELECT RAISE(ABORT, 'automation command adoptions are append-only');
    END;

    CREATE TRIGGER IF NOT EXISTS automation_command_adoptions_response_update_guard
    BEFORE UPDATE OF response_json ON automation_command_adoptions
    WHEN NOT (
        OLD.command IN ('definition.create.v1', 'definition.revise.v1')
        AND OLD.outcome = 'rejected'
        AND json_valid(OLD.response_json)
        AND json_extract(OLD.response_json, '$.outcome') IS 'rejected'
        AND json_extract(OLD.response_json, '$.error.code') IS 'VALIDATION_FAILED'
        AND json_valid(NEW.response_json)
        AND json_type(NEW.response_json, '$') IS 'object'
        AND json_extract(NEW.response_json, '$.outcome') IS 'rejected'
        AND json_type(NEW.response_json, '$.error') IS 'object'
        AND json_extract(NEW.response_json, '$.error.code') IS 'VALIDATION_FAILED'
        AND json_extract(NEW.response_json, '$.error.httpStatus') IS 400
        AND json_extract(NEW.response_json, '$.error.message')
            IS 'automation definition failed validation'
        AND json_extract(NEW.response_json, '$.error.retryable') IS 0
        AND json_extract(NEW.response_json, '$.error.currentRevision')
            IS json_extract(OLD.response_json, '$.error.currentRevision')
        AND NOT EXISTS (
            SELECT 1
            FROM json_each(NEW.response_json)
            WHERE key NOT IN ('outcome', 'error')
        )
        AND NOT EXISTS (
            SELECT 1
            FROM json_each(NEW.response_json, '$.error')
            WHERE key NOT IN (
                'code',
                'httpStatus',
                'message',
                'retryable',
                'currentRevision'
            )
        )
    )
    BEGIN
        SELECT RAISE(ABORT, 'automation command adoptions are append-only');
    END;

    DROP TRIGGER IF EXISTS automation_command_adoptions_no_update;

    CREATE TRIGGER IF NOT EXISTS automation_command_adoptions_no_delete
    BEFORE DELETE ON automation_command_adoptions
    BEGIN
        SELECT RAISE(ABORT, 'automation command adoptions are append-only');
    END;
";

const GLOBAL_ADOPTION_KEY_GUARDS_SQL: &str = "
    CREATE TRIGGER IF NOT EXISTS automation_attempt_adoption_key_global_insert
    BEFORE INSERT ON automation_attempts
    WHEN EXISTS (
        SELECT 1 FROM automation_command_reservations
        WHERE adoption_key = NEW.adoption_key
    ) OR EXISTS (
        SELECT 1 FROM automation_command_adoptions
        WHERE adoption_key = NEW.adoption_key
    )
    BEGIN
        SELECT RAISE(ABORT, 'automation adoption key is already used by a command');
    END;

    CREATE TRIGGER IF NOT EXISTS automation_command_reservation_key_global_insert
    BEFORE INSERT ON automation_command_reservations
    WHEN EXISTS (
        SELECT 1 FROM automation_attempts
        WHERE adoption_key = NEW.adoption_key
    )
    BEGIN
        SELECT RAISE(ABORT, 'automation adoption key is already used by an attempt');
    END;

    CREATE TRIGGER IF NOT EXISTS automation_command_adoption_key_global_insert
    BEFORE INSERT ON automation_command_adoptions
    WHEN EXISTS (
        SELECT 1 FROM automation_attempts
        WHERE adoption_key = NEW.adoption_key
    )
    BEGIN
        SELECT RAISE(ABORT, 'automation adoption key is already used by an attempt');
    END;
";

pub(crate) fn ensure_global_adoption_key_guards(conn: &Connection) -> Result<()> {
    let command_collision: Option<String> = conn
        .query_row(
            "SELECT reservation.adoption_key
             FROM automation_command_reservations AS reservation
             JOIN automation_command_adoptions AS adoption
               ON adoption.adoption_key = reservation.adoption_key
             LIMIT 1",
            [],
            |row| row.get(0),
        )
        .optional()
        .context("failed to inspect automation command adoption ownership")?;
    anyhow::ensure!(
        command_collision.is_none(),
        "automation adoption key `{}` is both reserved and adopted",
        command_collision.as_deref().unwrap_or_default()
    );
    let collision: Option<String> = conn
        .query_row(
            "SELECT attempt.adoption_key
             FROM automation_attempts AS attempt
             WHERE EXISTS (
                 SELECT 1 FROM automation_command_reservations AS reservation
                 WHERE reservation.adoption_key = attempt.adoption_key
             ) OR EXISTS (
                 SELECT 1 FROM automation_command_adoptions AS adoption
                 WHERE adoption.adoption_key = attempt.adoption_key
             )
             LIMIT 1",
            [],
            |row| row.get(0),
        )
        .optional()
        .context("failed to inspect global automation adoption keys")?;
    anyhow::ensure!(
        collision.is_none(),
        "automation adoption key `{}` is already shared by an attempt and command",
        collision.as_deref().unwrap_or_default()
    );
    conn.execute_batch(GLOBAL_ADOPTION_KEY_GUARDS_SQL)
        .context("failed to initialize global automation adoption-key guards")
}

pub(crate) fn attempt_adoption_key_exists(conn: &Connection, adoption_key: &str) -> Result<bool> {
    conn.query_row(
        "SELECT 1 FROM automation_attempts WHERE adoption_key = ?1",
        [adoption_key],
        |_| Ok(()),
    )
    .optional()
    .map(|row| row.is_some())
    .context("failed to inspect automation attempt adoption key")
}

#[derive(Debug, Clone)]
pub enum DefinitionCommand {
    Invalid {
        command: String,
        request: Value,
        message: String,
    },
    LegacyCreate {
        definition: Value,
    },
    LegacyRevise {
        definition: Value,
    },
    LegacyDelete {
        automation_id: String,
    },
    Create {
        definition: Value,
    },
    Revise {
        definition: Value,
        expected_revision: Option<u64>,
    },
    Disable {
        automation_id: String,
        expected_revision: Option<u64>,
        reason: Option<String>,
    },
    Delete {
        automation_id: String,
        expected_revision: Option<u64>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DefinitionCommandOutcome {
    Committed,
    Replayed,
    Rejected,
}

#[derive(Debug, Clone)]
pub struct DefinitionCommandResponse {
    pub outcome: DefinitionCommandOutcome,
    pub revision: Option<u64>,
    pub result: Option<Value>,
    pub error: Option<ErrorEnvelope>,
    pub replay_first_committed_at: Option<String>,
    pub event_ref: Option<super::contract::events::EventRef>,
    mutation_committed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
enum StoredResponse {
    Committed {
        result: Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        event_ref: Option<super::contract::events::EventRef>,
    },
    Rejected {
        error: ErrorEnvelope,
    },
}

struct StoredAdoption {
    request_digest: String,
    command: String,
    automation_id: Option<String>,
    outcome: String,
    revision: Option<u64>,
    response: StoredResponse,
    adopted_at: String,
}

struct DefinitionState {
    revision: u64,
    tombstoned: bool,
    authority_version: u8,
    lifecycle_state: String,
}

pub fn execute_definition_command(
    conn: &Connection,
    adoption_key: &str,
    command: DefinitionCommand,
    adopted_at: &str,
) -> Result<DefinitionCommandResponse> {
    let adoption_key = match AdoptionKey::new(adoption_key.to_owned()) {
        Ok(adoption_key) => adoption_key,
        Err(_) => {
            return Ok(rejected(
                ErrorCode::ValidationFailed,
                "automation command adoptionKey is invalid",
                None,
            ));
        }
    };
    let compatible_request_digests = compatible_request_digests(&command)?;
    let request_digest = compatible_request_digests
        .first()
        .expect("every automation command has a current request digest");
    let transaction = rusqlite::Transaction::new_unchecked(conn, TransactionBehavior::Immediate)
        .context("failed to begin automation command adoption transaction")?;

    if attempt_adoption_key_exists(&transaction, adoption_key.as_str())? {
        transaction
            .rollback()
            .context("failed to close conflicting attempt adoption transaction")?;
        return Ok(rejected(
            ErrorCode::AdoptionReplayMismatch,
            "adoption key was already used by an automation attempt",
            None,
        ));
    }

    if let Some((reserved_command, reserved_digest)) = transaction
        .query_row(
            "SELECT command, request_digest
             FROM automation_command_reservations
             WHERE adoption_key = ?1",
            [adoption_key.as_str()],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()
        .context("failed to inspect automation command reservation")?
    {
        let response = if reserved_command == command_identity(&command).0
            && request_digest_matches(&reserved_digest, &compatible_request_digests)
        {
            rejected(
                ErrorCode::CancelPending,
                "automation command adoption is still in progress",
                None,
            )
        } else {
            rejected(
                ErrorCode::AdoptionReplayMismatch,
                "adoption key is reserved for a different automation command",
                None,
            )
        };
        transaction
            .rollback()
            .context("failed to close reserved automation command transaction")?;
        return Ok(response);
    }

    if let Some(mut stored) = load_adoption(&transaction, adoption_key.as_str())? {
        if request_digest_matches(&stored.request_digest, &compatible_request_digests) {
            let sanitized = sanitize_v1_validation_replay(&mut stored);
            if sanitized {
                transaction
                    .execute(
                        "UPDATE automation_command_adoptions
                         SET response_json = ?2
                         WHERE adoption_key = ?1",
                        params![
                            adoption_key.as_str(),
                            serde_json::to_string(&stored.response).context(
                                "failed to serialize sanitized automation command response"
                            )?,
                        ],
                    )
                    .context("failed to persist sanitized automation command response")?;
                transaction
                    .commit()
                    .context("failed to commit sanitized automation command replay")?;
            } else {
                transaction
                    .rollback()
                    .context("failed to close automation command replay transaction")?;
            }
            return Ok(replay_response(stored));
        }
        let response = replay_mismatch_response(&adoption_key, &stored);
        transaction
            .rollback()
            .context("failed to close automation command replay transaction")?;
        return Ok(response);
    }

    let (command_name, automation_id) = command_identity(&command);
    let effective_adopted_at = match automation_id.as_deref() {
        Some(automation_id) => {
            super::store::monotonic_definition_timestamp(&transaction, automation_id, adopted_at)?
        }
        None => adopted_at.to_owned(),
    };
    let mut response = apply_command(&transaction, command, &effective_adopted_at)?;
    if response.mutation_committed {
        if let (Some(automation_id), Some(revision)) = (automation_id.as_deref(), response.revision)
        {
            let record =
                super::store::get_definition_with_tombstone(&transaction, automation_id, true)?
                    .with_context(|| {
                        format!("committed automation definition `{automation_id}` is missing")
                    })?;
            let lifecycle_state = if record.tombstoned_at.is_some() {
                "tombstoned"
            } else {
                record.lifecycle_state.as_str()
            };
            response.event_ref = Some(super::contract::events::append_definition_event(
                &transaction,
                super::contract::events::DefinitionEventInput {
                    command: command_name,
                    automation_id,
                    revision,
                    definition_digest: record.definition_digest.as_deref(),
                    lifecycle_state,
                    adoption_key: adoption_key.as_str(),
                    observed_at: &effective_adopted_at,
                },
            )?);
        }
    }
    let stored = match (&response.result, &response.error) {
        (Some(result), None) => StoredResponse::Committed {
            result: result.clone(),
            event_ref: response.event_ref.clone(),
        },
        (None, Some(error)) => StoredResponse::Rejected {
            error: error.clone(),
        },
        _ => anyhow::bail!("automation command produced an invalid response shape"),
    };
    let outcome = match response.outcome {
        DefinitionCommandOutcome::Committed => "committed",
        DefinitionCommandOutcome::Rejected => "rejected",
        DefinitionCommandOutcome::Replayed => {
            anyhow::bail!("new automation command cannot produce a replay response")
        }
    };
    let stored_revision = response.revision.map(sqlite_revision).transpose()?;
    transaction
        .execute(
            "INSERT INTO automation_command_adoptions (
                adoption_key, request_digest, command, automation_id, outcome,
                revision, response_json, adopted_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                adoption_key.as_str(),
                request_digest,
                command_name,
                automation_id,
                outcome,
                stored_revision,
                serde_json::to_string(&stored)
                    .context("failed to serialize automation command outcome")?,
                effective_adopted_at,
            ],
        )
        .context("failed to persist automation command adoption")?;
    transaction
        .commit()
        .context("failed to commit automation command adoption")?;
    Ok(response)
}

fn canonical_command(command: &DefinitionCommand) -> Result<Value> {
    Ok(match command {
        DefinitionCommand::Invalid {
            command, request, ..
        } => json!({
            "command": command,
            "request": lossless_json_fingerprint(request),
        }),
        DefinitionCommand::LegacyCreate { definition } => json!({
            "command": "legacy.definition.create.v1",
            "definition": legacy_adoption_definition_preimage(definition)?,
        }),
        DefinitionCommand::LegacyRevise { definition } => json!({
            "command": "legacy.definition.revise.v1",
            "definition": legacy_adoption_definition_preimage(definition)?,
        }),
        DefinitionCommand::LegacyDelete { automation_id } => json!({
            "command": "legacy.definition.delete.v1",
            "automationId": automation_id,
        }),
        DefinitionCommand::Create { definition } => json!({
            "command": "definition.create.v1",
            "definition": adoption_definition_preimage(definition)?,
        }),
        DefinitionCommand::Revise {
            definition,
            expected_revision,
        } => json!({
            "command": "definition.revise.v1",
            "expectedRevision": expected_revision,
            "definition": adoption_definition_preimage(definition)?,
        }),
        DefinitionCommand::Disable {
            automation_id,
            expected_revision,
            reason,
        } => json!({
            "command": "definition.disable.v1",
            "automationId": automation_id,
            "expectedRevision": expected_revision,
            "reason": reason,
        }),
        DefinitionCommand::Delete {
            automation_id,
            expected_revision,
        } => json!({
            "command": "definition.tombstone.v1",
            "automationId": automation_id,
            "expectedRevision": expected_revision,
        }),
    })
}

fn canonical_command_digest(command: &Value) -> Result<String> {
    Ok(sha256_hex(&canonicalize(command).context(
        "failed to canonicalize automation definition command",
    )?))
}

fn compatible_request_digests(command: &DefinitionCommand) -> Result<Vec<String>> {
    let mut digests = vec![canonical_command_digest(&canonical_command(command)?)?];
    let (command_name, definition, expected_revision) = match command {
        DefinitionCommand::Create { definition } => ("definition.create.v1", definition, None),
        DefinitionCommand::Revise {
            definition,
            expected_revision,
        } => ("definition.revise.v1", definition, *expected_revision),
        _ => return Ok(digests),
    };

    let mut legacy_preimages = Vec::with_capacity(3);
    if let Ok(parsed) = RoutineDefinition::from_json(definition) {
        let normalized = serde_json::to_value(parsed)
            .context("failed to normalize routine definition for legacy adoption replay")?;
        if definition == &normalized {
            legacy_preimages.push(json!({
                "kind": "valid",
                "value": normalized,
            }));
        }
    }
    let wire = lossless_json_fingerprint(definition);
    legacy_preimages.push(json!({
        "kind": "invalid",
        "value": wire,
    }));
    legacy_preimages.push(json!({
        "kind": "unsupported",
        "value": lossless_json_fingerprint(definition),
    }));

    for definition_preimage in legacy_preimages {
        let mut legacy = json!({
            "command": command_name,
            "definition": definition_preimage,
        });
        if command_name == "definition.revise.v1" {
            legacy["expectedRevision"] = json!(expected_revision);
        }
        let digest = canonical_command_digest(&legacy)?;
        if !digests.contains(&digest) {
            digests.push(digest);
        }
    }
    Ok(digests)
}

fn request_digest_matches(stored: &str, compatible: &[String]) -> bool {
    compatible.iter().any(|digest| digest == stored)
}

fn sanitize_v1_validation_replay(stored: &mut StoredAdoption) -> bool {
    if !matches!(
        stored.command.as_str(),
        "definition.create.v1" | "definition.revise.v1"
    ) {
        return false;
    }
    let StoredResponse::Rejected { error } = &stored.response else {
        return false;
    };
    if error.code() != ErrorCode::ValidationFailed {
        return false;
    }

    let mut sanitized = ErrorEnvelope::try_new(
        ErrorCode::ValidationFailed,
        DEFINITION_VALIDATION_FAILED_MESSAGE,
        false,
    )
    .expect("static automation definition validation message is valid");
    if let Some(current_revision) = error.current_revision {
        sanitized = sanitized.with_current_revision(current_revision);
    }
    if error == &sanitized {
        return false;
    }
    stored.response = StoredResponse::Rejected { error: sanitized };
    true
}

fn adoption_definition_preimage(definition: &Value) -> Result<Value> {
    Ok(json!({
        "kind": "wire",
        "value": lossless_json_fingerprint(definition),
    }))
}

fn legacy_adoption_definition_preimage(definition: &Value) -> Result<Value> {
    let definition = RoutineDefinition::legacy_wire_projection(definition);
    match RoutineDefinition::from_json(&definition) {
        Ok(definition) => Ok(json!({
            "kind": "valid",
            "value": serde_json::to_value(definition)
                .context("failed to normalize legacy routine definition for adoption")?,
        })),
        Err(_) => Ok(json!({
            "kind": "invalid",
            "value": lossless_json_fingerprint(&definition),
        })),
    }
}

fn lossless_json_fingerprint(value: &Value) -> Value {
    match value {
        Value::Null => json!(["null"]),
        Value::Bool(value) => json!(["bool", value]),
        Value::Number(value) => json!(["number", value.to_string()]),
        Value::String(value) => json!(["string", value]),
        Value::Array(values) => json!([
            "array",
            values
                .iter()
                .map(lossless_json_fingerprint)
                .collect::<Vec<_>>()
        ]),
        Value::Object(values) => json!([
            "object",
            values
                .iter()
                .map(|(key, value)| json!([key, lossless_json_fingerprint(value)]))
                .collect::<Vec<_>>()
        ]),
    }
}

fn command_identity(command: &DefinitionCommand) -> (&'static str, Option<String>) {
    match command {
        DefinitionCommand::Invalid {
            command, request, ..
        } => (
            match command.as_str() {
                "definition.create.v1" => "definition.create.v1",
                "definition.revise.v1" => "definition.revise.v1",
                "definition.disable.v1" => "definition.disable.v1",
                "definition.tombstone.v1" => "definition.tombstone.v1",
                _ => "definition.invalid.v1",
            },
            request
                .get("id")
                .or_else(|| request.get("definition").and_then(|value| value.get("id")))
                .and_then(Value::as_str)
                .map(ToOwned::to_owned),
        ),
        DefinitionCommand::LegacyCreate { definition } => (
            "legacy.definition.create.v1",
            definition
                .get("id")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned),
        ),
        DefinitionCommand::LegacyRevise { definition } => (
            "legacy.definition.revise.v1",
            definition
                .get("id")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned),
        ),
        DefinitionCommand::LegacyDelete { automation_id } => {
            ("legacy.definition.delete.v1", Some(automation_id.clone()))
        }
        DefinitionCommand::Create { definition } => (
            "definition.create.v1",
            definition
                .get("id")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned),
        ),
        DefinitionCommand::Revise { definition, .. } => (
            "definition.revise.v1",
            definition
                .get("id")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned),
        ),
        DefinitionCommand::Disable { automation_id, .. } => {
            ("definition.disable.v1", Some(automation_id.clone()))
        }
        DefinitionCommand::Delete { automation_id, .. } => {
            ("definition.tombstone.v1", Some(automation_id.clone()))
        }
    }
}

fn load_adoption(conn: &Connection, adoption_key: &str) -> Result<Option<StoredAdoption>> {
    conn.query_row(
        "SELECT request_digest, command, automation_id, outcome, revision,
                response_json, adopted_at
         FROM automation_command_adoptions
         WHERE adoption_key = ?1",
        [adoption_key],
        |row| {
            let response_json: String = row.get(5)?;
            let response = serde_json::from_str(&response_json).map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    5,
                    rusqlite::types::Type::Text,
                    Box::new(error),
                )
            })?;
            let revision = row
                .get::<_, Option<i64>>(4)?
                .map(|revision| {
                    u64::try_from(revision).map_err(|error| {
                        rusqlite::Error::FromSqlConversionFailure(
                            4,
                            rusqlite::types::Type::Integer,
                            Box::new(error),
                        )
                    })
                })
                .transpose()?;
            Ok(StoredAdoption {
                request_digest: row.get(0)?,
                command: row.get(1)?,
                automation_id: row.get(2)?,
                outcome: row.get(3)?,
                revision,
                response,
                adopted_at: row.get(6)?,
            })
        },
    )
    .optional()
    .context("failed to load automation command adoption")
}

fn replay_response(stored: StoredAdoption) -> DefinitionCommandResponse {
    match stored.response {
        StoredResponse::Committed { result, event_ref } => DefinitionCommandResponse {
            outcome: DefinitionCommandOutcome::Replayed,
            revision: stored.revision,
            result: Some(result),
            error: None,
            replay_first_committed_at: Some(stored.adopted_at),
            event_ref,
            mutation_committed: false,
        },
        StoredResponse::Rejected { error } => DefinitionCommandResponse {
            outcome: DefinitionCommandOutcome::Rejected,
            revision: stored.revision,
            result: None,
            error: Some(error),
            replay_first_committed_at: None,
            event_ref: None,
            mutation_committed: false,
        },
    }
}

fn replay_mismatch_response(
    adoption_key: &AdoptionKey,
    stored: &StoredAdoption,
) -> DefinitionCommandResponse {
    let conflict_outcome = match stored.outcome.as_str() {
        "committed" => AdoptionConflictOutcome::Committed,
        "rejected" => AdoptionConflictOutcome::Rejected,
        _ => unreachable!("database constraint limits stored adoption outcomes"),
    };
    let error = ErrorEnvelope::try_new(
        ErrorCode::AdoptionReplayMismatch,
        "adoption key was already used for a different automation command",
        false,
    )
    .expect("static adoption mismatch message is valid")
    .with_adoption(ErrorAdoption {
        key: adoption_key.clone(),
        conflict_outcome: Some(conflict_outcome),
    })
    .with_details(BTreeMap::from([
        (
            "committedCommand".to_owned(),
            Value::String(stored.command.clone()),
        ),
        (
            "committedOutcome".to_owned(),
            Value::String(stored.outcome.clone()),
        ),
        (
            "committedRevision".to_owned(),
            stored.revision.map_or(Value::Null, Value::from),
        ),
        (
            "automationId".to_owned(),
            stored
                .automation_id
                .clone()
                .map_or(Value::Null, Value::String),
        ),
    ]));
    DefinitionCommandResponse {
        outcome: DefinitionCommandOutcome::Rejected,
        revision: stored.revision,
        result: None,
        error: Some(error),
        replay_first_committed_at: None,
        event_ref: None,
        mutation_committed: false,
    }
}

fn apply_command(
    conn: &Connection,
    command: DefinitionCommand,
    adopted_at: &str,
) -> Result<DefinitionCommandResponse> {
    match command {
        DefinitionCommand::Invalid { message, .. } => {
            Ok(rejected(ErrorCode::ValidationFailed, message, None))
        }
        DefinitionCommand::LegacyCreate { definition } => {
            apply_legacy_create(conn, &definition, adopted_at)
        }
        DefinitionCommand::LegacyRevise { definition } => {
            apply_legacy_revise(conn, &definition, adopted_at)
        }
        DefinitionCommand::LegacyDelete { automation_id } => {
            apply_legacy_delete(conn, &automation_id, adopted_at)
        }
        DefinitionCommand::Create { definition } => apply_create(conn, &definition, adopted_at),
        DefinitionCommand::Revise {
            definition,
            expected_revision,
        } => apply_revise(conn, &definition, expected_revision, adopted_at),
        DefinitionCommand::Disable {
            automation_id,
            expected_revision,
            reason,
        } => apply_disable(
            conn,
            &automation_id,
            expected_revision,
            reason.as_deref(),
            adopted_at,
        ),
        DefinitionCommand::Delete {
            automation_id,
            expected_revision,
        } => apply_delete(conn, &automation_id, expected_revision, adopted_at),
    }
}

fn apply_legacy_create(
    conn: &Connection,
    definition_value: &Value,
    adopted_at: &str,
) -> Result<DefinitionCommandResponse> {
    let definition = match RoutineDefinition::from_legacy_json(definition_value)
        .and_then(RoutineDefinition::resolve_timezone_for_persistence)
    {
        Ok(definition) => definition,
        Err(error) => {
            return Ok(rejected(ErrorCode::ValidationFailed, error, None));
        }
    };
    if definition.status == super::definition::RoutineStatus::Disabled {
        return Ok(rejected(
            ErrorCode::IllegalTransition,
            "legacy create cannot create a disabled definition",
            None,
        ));
    }
    if let Some(current) = current_definition_state(conn, &definition.id)? {
        if current.tombstoned && current.authority_version == 0 {
            let next_revision = next_revision(current.revision)?;
            let definition_json = serde_json::to_string(&definition)
                .context("failed to serialize legacy routine definition")?;
            let definition_digest =
                super::contract::migration::definition_digest(&definition_json)?;
            let lifecycle_state =
                super::contract::migration::lifecycle_state(status_text(definition.status));
            let changed = conn
                .execute(
                    "UPDATE automation_definitions
                     SET name = ?2,
                         status = ?3,
                         definition_json = ?4,
                         definition_digest = ?5,
                         lifecycle_state = ?6,
                         revision = ?7,
                         tombstoned_at = NULL,
                         created_at = ?8,
                         updated_at = ?8
                     WHERE id = ?1
                       AND revision = ?9
                       AND tombstoned_at IS NOT NULL
                       AND authority_version = 0",
                    params![
                        definition.id,
                        definition.name,
                        status_text(definition.status),
                        definition_json,
                        definition_digest,
                        lifecycle_state,
                        sqlite_revision(next_revision)?,
                        adopted_at,
                        sqlite_revision(current.revision)?,
                    ],
                )
                .context("failed to revive legacy automation definition")?;
            anyhow::ensure!(
                changed == 1,
                "automation definition changed inside legacy revival transaction"
            );
            return Ok(committed(
                next_revision,
                json!({
                    "routine": definition,
                    "createdAt": adopted_at,
                }),
            ));
        }
        return Ok(if current.tombstoned {
            rejected(
                ErrorCode::GoneTombstoned,
                format!("routine `{}` is tombstoned", definition.id),
                Some(current.revision),
            )
        } else {
            revision_conflict(current.revision)
        });
    }
    let definition_json =
        serde_json::to_string(&definition).context("failed to serialize routine definition")?;
    let definition_digest = super::contract::migration::definition_digest(&definition_json)?;
    let lifecycle_state =
        super::contract::migration::lifecycle_state(status_text(definition.status));
    conn.execute(
        "INSERT INTO automation_definitions (
            id, name, status, definition_json, revision, definition_digest, lifecycle_state,
            tombstoned_at, authority_version, created_at, updated_at
         ) VALUES (?1, ?2, ?3, ?4, 1, ?5, ?6, NULL, 0, ?7, ?7)",
        params![
            definition.id,
            definition.name,
            status_text(definition.status),
            definition_json,
            definition_digest,
            lifecycle_state,
            adopted_at,
        ],
    )
    .context("failed to insert legacy automation definition")?;
    Ok(committed(
        1,
        json!({
            "routine": definition,
            "createdAt": adopted_at,
        }),
    ))
}

fn apply_legacy_revise(
    conn: &Connection,
    definition_value: &Value,
    adopted_at: &str,
) -> Result<DefinitionCommandResponse> {
    let definition = match RoutineDefinition::from_legacy_json(definition_value)
        .and_then(RoutineDefinition::resolve_timezone_for_persistence)
    {
        Ok(definition) => definition,
        Err(error) => {
            return Ok(rejected(ErrorCode::ValidationFailed, error, None));
        }
    };
    let Some(current) = current_definition_state(conn, &definition.id)? else {
        return Ok(rejected(
            ErrorCode::NotFound,
            format!("no routine with id `{}`", definition.id),
            None,
        ));
    };
    if current.tombstoned {
        return Ok(rejected(
            ErrorCode::GoneTombstoned,
            format!("routine `{}` is tombstoned", definition.id),
            Some(current.revision),
        ));
    }
    if current.lifecycle_state == "disabled" {
        return Ok(rejected(
            ErrorCode::IllegalTransition,
            "legacy update cannot reactivate a disabled definition",
            Some(current.revision),
        ));
    }
    if definition.status == super::definition::RoutineStatus::Disabled {
        return Ok(rejected(
            ErrorCode::IllegalTransition,
            "legacy update cannot disable a definition",
            Some(current.revision),
        ));
    }
    if current.authority_version == 1 {
        return Ok(rejected(
            ErrorCode::IllegalTransition,
            format!(
                "routine `{}` is managed by the versioned authority API",
                definition.id
            ),
            Some(current.revision),
        ));
    }
    let next_revision = next_revision(current.revision)?;
    let definition_json =
        serde_json::to_string(&definition).context("failed to serialize routine definition")?;
    let definition_digest = super::contract::migration::definition_digest(&definition_json)?;
    let lifecycle_state =
        super::contract::migration::lifecycle_state(status_text(definition.status));
    let changed = conn
        .execute(
            "UPDATE automation_definitions
             SET name = ?2,
                 status = ?3,
                 definition_json = ?4,
                 definition_digest = ?5,
                 lifecycle_state = ?6,
                 revision = ?7,
                 updated_at = ?8
             WHERE id = ?1
               AND revision = ?9
               AND tombstoned_at IS NULL
               AND authority_version = 0",
            params![
                definition.id,
                definition.name,
                status_text(definition.status),
                definition_json,
                definition_digest,
                lifecycle_state,
                sqlite_revision(next_revision)?,
                adopted_at,
                sqlite_revision(current.revision)?,
            ],
        )
        .context("failed to revise legacy automation definition")?;
    anyhow::ensure!(
        changed == 1,
        "automation definition revision changed inside adoption transaction"
    );
    Ok(committed(
        next_revision,
        json!({
            "routine": definition,
            "updatedAt": adopted_at,
        }),
    ))
}

fn apply_legacy_delete(
    conn: &Connection,
    automation_id: &str,
    adopted_at: &str,
) -> Result<DefinitionCommandResponse> {
    let Some(current) = current_definition_state(conn, automation_id)? else {
        return Ok(committed_delete(false, automation_id, None));
    };
    if current.tombstoned || current.authority_version == 1 {
        return Ok(committed_delete(
            false,
            automation_id,
            Some(current.revision),
        ));
    }
    let next_revision = next_revision(current.revision)?;
    let changed = conn
        .execute(
            "UPDATE automation_definitions
             SET revision = ?2,
                 tombstoned_at = ?3,
                 updated_at = ?3
             WHERE id = ?1
               AND revision = ?4
               AND tombstoned_at IS NULL
               AND authority_version = 0",
            params![
                automation_id,
                sqlite_revision(next_revision)?,
                adopted_at,
                sqlite_revision(current.revision)?,
            ],
        )
        .context("failed to delete legacy automation definition")?;
    anyhow::ensure!(
        changed == 1,
        "automation definition changed inside legacy delete transaction"
    );
    Ok(committed_delete(true, automation_id, Some(next_revision)))
}

fn apply_create(
    conn: &Connection,
    definition_value: &Value,
    adopted_at: &str,
) -> Result<DefinitionCommandResponse> {
    let definition = match negotiate_definition(definition_value) {
        Ok(DefinitionNegotiation::Supported(definition)) => {
            match definition.resolve_timezone_for_persistence() {
                Ok(definition) => definition,
                Err(_) => {
                    return Ok(rejected(
                        ErrorCode::ValidationFailed,
                        DEFINITION_VALIDATION_FAILED_MESSAGE,
                        None,
                    ));
                }
            }
        }
        Ok(DefinitionNegotiation::Unsupported(unsupported)) => {
            return Ok(capability_unsupported(unsupported));
        }
        Err(_) => {
            return Ok(rejected(
                ErrorCode::ValidationFailed,
                DEFINITION_VALIDATION_FAILED_MESSAGE,
                None,
            ));
        }
    };
    if definition.status == super::definition::RoutineStatus::Disabled {
        return Ok(rejected(
            ErrorCode::IllegalTransition,
            "definition.create.v1 cannot create a disabled definition",
            None,
        ));
    }
    if let Some(current) = current_definition_state(conn, &definition.id)? {
        if current.tombstoned {
            return Ok(rejected(
                ErrorCode::GoneTombstoned,
                format!("routine `{}` is tombstoned", definition.id),
                Some(current.revision),
            ));
        }
        return Ok(revision_conflict(current.revision));
    }
    let definition_json =
        serde_json::to_string(&definition).context("failed to serialize routine definition")?;
    let definition_digest = super::contract::migration::definition_digest(&definition_json)?;
    let lifecycle_state =
        super::contract::migration::lifecycle_state(status_text(definition.status));
    conn.execute(
        "INSERT INTO automation_definitions (
            id, name, status, definition_json, revision, definition_digest, lifecycle_state,
            tombstoned_at, authority_version, created_at, updated_at
         ) VALUES (?1, ?2, ?3, ?4, 1, ?5, ?6, NULL, 1, ?7, ?7)",
        params![
            definition.id,
            definition.name,
            status_text(definition.status),
            definition_json,
            definition_digest,
            lifecycle_state,
            adopted_at,
        ],
    )
    .context("failed to insert adopted automation definition")?;
    Ok(committed(
        1,
        json!({
            "routine": definition,
            "revision": 1,
        }),
    ))
}

fn apply_revise(
    conn: &Connection,
    definition_value: &Value,
    expected_revision: Option<u64>,
    adopted_at: &str,
) -> Result<DefinitionCommandResponse> {
    let definition = match negotiate_definition(definition_value) {
        Ok(DefinitionNegotiation::Supported(definition)) => {
            match definition.resolve_timezone_for_persistence() {
                Ok(definition) => definition,
                Err(_) => {
                    return Ok(rejected(
                        ErrorCode::ValidationFailed,
                        DEFINITION_VALIDATION_FAILED_MESSAGE,
                        None,
                    ));
                }
            }
        }
        Ok(DefinitionNegotiation::Unsupported(unsupported)) => {
            return Ok(capability_unsupported(unsupported));
        }
        Err(_) => {
            return Ok(rejected(
                ErrorCode::ValidationFailed,
                DEFINITION_VALIDATION_FAILED_MESSAGE,
                None,
            ));
        }
    };
    let Some(current) = current_definition_state(conn, &definition.id)? else {
        return Ok(rejected(
            ErrorCode::NotFound,
            format!("no routine with id `{}`", definition.id),
            None,
        ));
    };
    if current.tombstoned {
        return Ok(rejected(
            ErrorCode::GoneTombstoned,
            format!("routine `{}` is tombstoned", definition.id),
            Some(current.revision),
        ));
    }
    if expected_revision.is_some_and(|expected| current.revision != expected) {
        return Ok(revision_conflict(current.revision));
    }
    if current.lifecycle_state == "disabled" {
        return Ok(rejected(
            ErrorCode::IllegalTransition,
            "disabled definitions must use an explicit lifecycle transition",
            Some(current.revision),
        ));
    }
    if definition.status == super::definition::RoutineStatus::Disabled {
        return Ok(rejected(
            ErrorCode::IllegalTransition,
            "definition.revise.v1 cannot disable a definition",
            Some(current.revision),
        ));
    }
    let next_revision = next_revision(current.revision)?;
    let next_revision_sql = sqlite_revision(next_revision)?;
    let current_revision_sql = sqlite_revision(current.revision)?;
    let definition_json =
        serde_json::to_string(&definition).context("failed to serialize routine definition")?;
    let definition_digest = super::contract::migration::definition_digest(&definition_json)?;
    let lifecycle_state =
        super::contract::migration::lifecycle_state(status_text(definition.status));
    let changed = conn
        .execute(
            "UPDATE automation_definitions
             SET name = ?2,
                 status = ?3,
                 definition_json = ?4,
                 definition_digest = ?5,
                 lifecycle_state = ?6,
                 revision = ?7,
                 authority_version = 1,
                 updated_at = ?8
             WHERE id = ?1 AND revision = ?9",
            params![
                definition.id,
                definition.name,
                status_text(definition.status),
                definition_json,
                definition_digest,
                lifecycle_state,
                next_revision_sql,
                adopted_at,
                current_revision_sql,
            ],
        )
        .context("failed to revise adopted automation definition")?;
    anyhow::ensure!(
        changed == 1,
        "automation definition revision changed inside adoption transaction"
    );
    Ok(committed(
        next_revision,
        json!({
            "routine": definition,
            "revision": next_revision,
        }),
    ))
}

fn apply_disable(
    conn: &Connection,
    automation_id: &str,
    expected_revision: Option<u64>,
    reason: Option<&str>,
    adopted_at: &str,
) -> Result<DefinitionCommandResponse> {
    let Some(current) = current_definition_state(conn, automation_id)? else {
        return Ok(rejected(
            ErrorCode::NotFound,
            format!("no routine with id `{automation_id}`"),
            None,
        ));
    };
    if current.tombstoned {
        return Ok(rejected(
            ErrorCode::GoneTombstoned,
            format!("routine `{automation_id}` is tombstoned"),
            Some(current.revision),
        ));
    }
    if expected_revision.is_some_and(|expected| current.revision != expected) {
        return Ok(revision_conflict(current.revision));
    }
    if current.lifecycle_state == "disabled" {
        return Ok(rejected(
            ErrorCode::IllegalTransition,
            "automation definition is already disabled",
            Some(current.revision),
        ));
    }
    let record = super::store::get_definition(conn, automation_id)?
        .with_context(|| format!("automation definition `{automation_id}` disappeared"))?;
    let mut definition: RoutineDefinition = serde_json::from_str(&record.definition_json)
        .context("failed to parse automation definition for disable")?;
    definition.status = super::definition::RoutineStatus::Disabled;
    let definition_json =
        serde_json::to_string(&definition).context("failed to serialize disabled definition")?;
    let definition_digest = super::contract::migration::definition_digest(&definition_json)?;
    let next_revision = next_revision(current.revision)?;
    let changed = conn
        .execute(
            "UPDATE automation_definitions
             SET status = 'DISABLED',
                 definition_json = ?3,
                 definition_digest = ?4,
                 lifecycle_state = 'disabled',
                 revision = ?5,
                 authority_version = 1,
                 updated_at = ?6
             WHERE id = ?1 AND revision = ?2 AND tombstoned_at IS NULL",
            params![
                automation_id,
                sqlite_revision(current.revision)?,
                definition_json,
                definition_digest,
                sqlite_revision(next_revision)?,
                adopted_at,
            ],
        )
        .context("failed to disable adopted automation definition")?;
    anyhow::ensure!(
        changed == 1,
        "automation definition revision changed inside disable transaction"
    );
    Ok(committed(
        next_revision,
        json!({
            "disabled": true,
            "id": automation_id,
            "revision": next_revision,
            "reason": reason,
        }),
    ))
}

fn apply_delete(
    conn: &Connection,
    automation_id: &str,
    expected_revision: Option<u64>,
    adopted_at: &str,
) -> Result<DefinitionCommandResponse> {
    let Some(current) = current_definition_state(conn, automation_id)? else {
        return Ok(rejected(
            ErrorCode::NotFound,
            format!("no routine with id `{automation_id}`"),
            None,
        ));
    };
    if current.tombstoned {
        return Ok(rejected(
            ErrorCode::GoneTombstoned,
            format!("routine `{automation_id}` is tombstoned"),
            Some(current.revision),
        ));
    }
    if expected_revision.is_some_and(|expected| current.revision != expected) {
        return Ok(revision_conflict(current.revision));
    }
    let next_revision = next_revision(current.revision)?;
    let current_revision_sql = sqlite_revision(current.revision)?;
    let changed = conn
        .execute(
            "UPDATE automation_definitions
             SET revision = ?3,
                 tombstoned_at = ?4,
                 authority_version = 1,
                 updated_at = ?4
             WHERE id = ?1 AND revision = ?2 AND tombstoned_at IS NULL",
            params![
                automation_id,
                current_revision_sql,
                sqlite_revision(next_revision)?,
                adopted_at,
            ],
        )
        .context("failed to delete adopted automation definition")?;
    anyhow::ensure!(
        changed == 1,
        "automation definition revision changed inside adoption transaction"
    );
    Ok(committed(
        next_revision,
        json!({
            "deleted": true,
            "id": automation_id,
            "revision": next_revision,
        }),
    ))
}

fn current_definition_state(
    conn: &Connection,
    automation_id: &str,
) -> Result<Option<DefinitionState>> {
    let state = conn
        .query_row(
            "SELECT revision, tombstoned_at IS NOT NULL, authority_version, lifecycle_state
             FROM automation_definitions
             WHERE id = ?1",
            [automation_id],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, bool>(1)?,
                    row.get::<_, u8>(2)?,
                    row.get::<_, String>(3)?,
                ))
            },
        )
        .optional()
        .context("failed to load automation definition state")?;
    state
        .map(
            |(revision, tombstoned, authority_version, lifecycle_state)| {
                Ok(DefinitionState {
                    revision: u64::try_from(revision)
                        .context("automation definition revision must be non-negative")?,
                    tombstoned,
                    authority_version,
                    lifecycle_state,
                })
            },
        )
        .transpose()
}

fn committed(revision: u64, result: Value) -> DefinitionCommandResponse {
    DefinitionCommandResponse {
        outcome: DefinitionCommandOutcome::Committed,
        revision: Some(revision),
        result: Some(result),
        error: None,
        replay_first_committed_at: None,
        event_ref: None,
        mutation_committed: true,
    }
}

fn committed_delete(
    deleted: bool,
    automation_id: &str,
    revision: Option<u64>,
) -> DefinitionCommandResponse {
    DefinitionCommandResponse {
        outcome: DefinitionCommandOutcome::Committed,
        revision,
        result: Some(json!({
            "deleted": deleted,
            "id": automation_id,
        })),
        error: None,
        replay_first_committed_at: None,
        event_ref: None,
        mutation_committed: deleted,
    }
}

fn rejected(
    code: ErrorCode,
    message: impl Into<String>,
    revision: Option<u64>,
) -> DefinitionCommandResponse {
    let mut error = protocol_error(code, message);
    if let Some(revision) = revision {
        error = error.with_current_revision(
            PositiveInteger::new(revision)
                .expect("stored automation revisions are positive safe integers"),
        );
    }
    DefinitionCommandResponse {
        outcome: DefinitionCommandOutcome::Rejected,
        revision,
        result: None,
        error: Some(error),
        replay_first_committed_at: None,
        event_ref: None,
        mutation_committed: false,
    }
}

fn capability_unsupported(unsupported: UnsupportedVariant) -> DefinitionCommandResponse {
    let error = ErrorEnvelope::try_new(
        ErrorCode::CapabilityUnsupported,
        "automation definition uses a variant not supported by the negotiated contract profile",
        false,
    )
    .expect("static unsupported-capability message is valid")
    .with_details(BTreeMap::from([
        (
            "contractProfile".to_owned(),
            Value::String(capability_profile().contract_profile.clone()),
        ),
        ("reason".to_owned(), Value::String(unsupported.reason)),
        ("variant".to_owned(), Value::String(unsupported.variant)),
    ]));
    DefinitionCommandResponse {
        outcome: DefinitionCommandOutcome::Rejected,
        revision: None,
        result: None,
        error: Some(error),
        replay_first_committed_at: None,
        event_ref: None,
        mutation_committed: false,
    }
}

fn protocol_error(code: ErrorCode, message: impl Into<String>) -> ErrorEnvelope {
    let message = message.into();
    let bounded = if message.is_empty() {
        "automation command failed".to_owned()
    } else {
        message.chars().take(1_000).collect()
    };
    ErrorEnvelope::try_new(code, bounded, false)
        .expect("bounded non-empty automation error message is valid")
}

fn revision_conflict(current_revision: u64) -> DefinitionCommandResponse {
    rejected(
        ErrorCode::RevisionConflict,
        "automation definition revision does not match expectedRevision",
        Some(current_revision),
    )
}

fn status_text(status: super::definition::RoutineStatus) -> &'static str {
    match status {
        super::definition::RoutineStatus::Active => "ACTIVE",
        super::definition::RoutineStatus::Paused => "PAUSED",
        super::definition::RoutineStatus::Disabled => "DISABLED",
    }
}

fn sqlite_revision(revision: u64) -> Result<i64> {
    i64::try_from(revision).context("automation definition revision exceeds SQLite integer range")
}

fn next_revision(current_revision: u64) -> Result<u64> {
    current_revision
        .checked_add(1)
        .filter(|revision| *revision <= 9_007_199_254_740_991)
        .context("automation definition revision overflow")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::automations::contract::error::ErrorCode;
    use crate::automations::store::{get_definition, list_definitions};
    use crate::store::initialize_store;
    use serde_json::json;

    fn temp_store() -> (tempfile::TempDir, Connection) {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("store.sqlite");
        initialize_store(&path).unwrap();
        let conn = crate::store::open_store(&path).unwrap();
        (temp, conn)
    }

    fn definition(id: &str, name: &str) -> Value {
        json!({
            "schemaVersion": 1,
            "id": id,
            "name": name,
            "status": "PAUSED",
            "rrule": "FREQ=DAILY;BYHOUR=9",
            "timezone": "local",
            "misfire": "latest",
            "overlap": "forbid",
            "timeoutMinutes": 30,
            "runtime": "coven-code",
            "prompt": "Do the thing."
        })
    }

    fn adoption_count(conn: &Connection) -> i64 {
        conn.query_row(
            "SELECT COUNT(*) FROM automation_command_adoptions",
            [],
            |row| row.get(0),
        )
        .unwrap()
    }

    fn definition_event_count(conn: &Connection, automation_id: &str) -> i64 {
        conn.query_row(
            "SELECT COUNT(*) FROM automation_events
             WHERE stream_kind = 'automation' AND stream_id = ?1",
            [automation_id],
            |row| row.get(0),
        )
        .unwrap()
    }

    fn definition_command_fingerprint(command: &DefinitionCommand) -> String {
        sha256_hex(&canonicalize(&canonical_command(command).unwrap()).unwrap())
    }

    fn legacy_v1_command_digest(command: &DefinitionCommand, kind: &str) -> String {
        let (command_name, definition, expected_revision) = match command {
            DefinitionCommand::Create { definition } => ("definition.create.v1", definition, None),
            DefinitionCommand::Revise {
                definition,
                expected_revision,
            } => ("definition.revise.v1", definition, *expected_revision),
            _ => panic!("legacy v1 definition digest requires create or revise"),
        };
        let value = match kind {
            "valid" => serde_json::to_value(RoutineDefinition::from_json(definition).unwrap())
                .expect("normalized definition serializes"),
            "invalid" | "unsupported" => lossless_json_fingerprint(definition),
            _ => panic!("unknown legacy definition preimage kind"),
        };
        let mut canonical = json!({
            "command": command_name,
            "definition": {
                "kind": kind,
                "value": value,
            },
        });
        if command_name == "definition.revise.v1" {
            canonical["expectedRevision"] = json!(expected_revision);
        }
        canonical_command_digest(&canonical).unwrap()
    }

    fn stored_capability_rejection(message: &str) -> (ErrorEnvelope, String) {
        let error = rejected(ErrorCode::CapabilityUnsupported, message, None)
            .error
            .unwrap();
        let stored = StoredResponse::Rejected {
            error: error.clone(),
        };
        (error, serde_json::to_string(&stored).unwrap())
    }

    #[test]
    fn v1_definition_preimages_use_stable_wire_form_across_parser_and_profile_status() {
        use super::super::capability_negotiation::{
            preflight_definition, preflight_definition_with_profile, VariantCapability,
        };

        let valid = definition("valid-wire-preimage", "Valid wire preimage");
        assert!(RoutineDefinition::from_json(&valid).is_ok());
        assert_eq!(
            adoption_definition_preimage(&valid).unwrap(),
            json!({
                "kind": "wire",
                "value": lossless_json_fingerprint(&valid),
            })
        );

        let mut parser_invalid = definition(
            "parser-invalid-wire-preimage",
            "Parser-invalid wire preimage",
        );
        parser_invalid["timezone"] = json!("not a valid timezone");
        assert!(RoutineDefinition::from_json(&parser_invalid).is_err());
        assert_eq!(
            adoption_definition_preimage(&parser_invalid).unwrap(),
            json!({
                "kind": "wire",
                "value": lossless_json_fingerprint(&parser_invalid),
            })
        );

        for mut non_flat in [
            definition("unsupported-flat-preimage", "Unsupported flat preimage"),
            definition("unsupported-rich-preimage", "Unsupported rich preimage"),
        ] {
            if non_flat["id"] == "unsupported-flat-preimage" {
                non_flat["outputTarget"] = json!("result.md");
            } else {
                non_flat["action"] = json!({
                    "variant": "pipeline",
                    "version": 1,
                    "steps": [{"prompt": "First step"}]
                });
            }

            assert!(
                preflight_definition(&non_flat).is_some(),
                "fixture must currently be capability-unsupported"
            );
            assert_eq!(
                adoption_definition_preimage(&non_flat).unwrap(),
                json!({
                    "kind": "wire",
                    "value": lossless_json_fingerprint(&non_flat),
                })
            );
        }

        let mut promoted_definition =
            definition("promoted-flat-preimage", "Promoted flat preimage");
        promoted_definition["outputTarget"] = json!("result.md");
        let promoted_preimage = adoption_definition_preimage(&promoted_definition).unwrap();
        let mut promoted_profile = capability_profile().clone();
        promoted_profile
            .supported
            .delivery_policies
            .push(VariantCapability {
                variant: "outputTarget.atomic".to_owned(),
                profile: None,
                notes: None,
            });
        assert!(preflight_definition(&promoted_definition).is_some());
        assert_eq!(
            preflight_definition_with_profile(&promoted_definition, &promoted_profile),
            None
        );
        assert_eq!(
            adoption_definition_preimage(&promoted_definition).unwrap(),
            promoted_preimage
        );

        let mut withdrawn_definition =
            definition("withdrawn-flat-preimage", "Withdrawn flat preimage");
        withdrawn_definition["retry"] = json!({
            "maxAttempts": 2,
            "backoffPolicy": "none",
            "retryableClasses": ["runtime_unavailable"]
        });
        let withdrawn_preimage = adoption_definition_preimage(&withdrawn_definition).unwrap();
        let mut withdrawn_profile = capability_profile().clone();
        withdrawn_profile
            .supported
            .trigger_policies
            .retain(|supported| supported.variant != "retry.safe-classes");
        assert_eq!(preflight_definition(&withdrawn_definition), None);
        assert_eq!(
            preflight_definition_with_profile(&withdrawn_definition, &withdrawn_profile)
                .expect("withdrawn category must refuse the definition")
                .variant,
            "retry.safe-classes"
        );
        assert_eq!(
            adoption_definition_preimage(&withdrawn_definition).unwrap(),
            withdrawn_preimage
        );
    }

    #[test]
    fn exact_replay_accepts_every_legacy_v1_definition_digest_form() {
        let (_temp, conn) = temp_store();
        let (stored_error, stored_response) =
            stored_capability_rejection("stored legacy capability rejection");

        for command_kind in ["create", "revise"] {
            for legacy_kind in ["valid", "invalid", "unsupported"] {
                let id = format!("legacy-{command_kind}-{legacy_kind}-digest");
                let body = definition(&id, "Legacy digest");
                let command = match command_kind {
                    "create" => DefinitionCommand::Create {
                        definition: body.clone(),
                    },
                    "revise" => DefinitionCommand::Revise {
                        definition: body.clone(),
                        expected_revision: Some(7),
                    },
                    _ => unreachable!(),
                };
                let adoption_key = format!("adopt:{command_kind}:legacy-{legacy_kind}-digest:0001");
                conn.execute(
                    "INSERT INTO automation_command_adoptions (
                        adoption_key, request_digest, command, automation_id, outcome,
                        revision, response_json, adopted_at
                     ) VALUES (?1, ?2, ?3, ?4, 'rejected', NULL, ?5, ?6)",
                    params![
                        adoption_key,
                        legacy_v1_command_digest(&command, legacy_kind),
                        command_identity(&command).0,
                        id,
                        stored_response,
                        "2026-09-03T09:00:00.000Z",
                    ],
                )
                .unwrap();

                let replay = execute_definition_command(
                    &conn,
                    &adoption_key,
                    command.clone(),
                    "2026-09-03T09:01:00.000Z",
                )
                .unwrap();

                assert_eq!(
                    replay.outcome,
                    DefinitionCommandOutcome::Rejected,
                    "{command_kind} {legacy_kind}"
                );
                assert_eq!(
                    replay.error,
                    Some(stored_error.clone()),
                    "{command_kind} {legacy_kind}"
                );

                if legacy_kind == "valid" {
                    let mut default_explicit_body = body.clone();
                    default_explicit_body["retry"] = json!({
                        "maxAttempts": 1,
                        "backoffPolicy": "none",
                        "retryableClasses": []
                    });
                    let default_explicit_command = match command_kind {
                        "create" => DefinitionCommand::Create {
                            definition: default_explicit_body,
                        },
                        "revise" => DefinitionCommand::Revise {
                            definition: default_explicit_body,
                            expected_revision: Some(7),
                        },
                        _ => unreachable!(),
                    };
                    assert_eq!(
                        legacy_v1_command_digest(&command, legacy_kind),
                        legacy_v1_command_digest(&default_explicit_command, legacy_kind),
                        "{command_kind} fixture must reproduce the old normalized digest alias"
                    );
                    let default_explicit = execute_definition_command(
                        &conn,
                        &adoption_key,
                        default_explicit_command,
                        "2026-09-03T09:01:30.000Z",
                    )
                    .unwrap();
                    assert_eq!(
                        default_explicit.error.as_ref().map(ErrorEnvelope::code),
                        Some(ErrorCode::AdoptionReplayMismatch),
                        "{command_kind} explicit default retry must not match an omitted retry"
                    );
                }

                let mut changed_body = body.clone();
                changed_body["prompt"] = json!("Changed payload must not replay.");
                let changed_command = match command_kind {
                    "create" => DefinitionCommand::Create {
                        definition: changed_body,
                    },
                    "revise" => DefinitionCommand::Revise {
                        definition: changed_body,
                        expected_revision: Some(7),
                    },
                    _ => unreachable!(),
                };
                let changed = execute_definition_command(
                    &conn,
                    &adoption_key,
                    changed_command,
                    "2026-09-03T09:02:00.000Z",
                )
                .unwrap();
                assert_eq!(
                    changed.error.as_ref().map(ErrorEnvelope::code),
                    Some(ErrorCode::AdoptionReplayMismatch),
                    "{command_kind} {legacy_kind} changed payload"
                );

                if command_kind == "revise" {
                    let changed_revision = execute_definition_command(
                        &conn,
                        &adoption_key,
                        DefinitionCommand::Revise {
                            definition: body,
                            expected_revision: Some(8),
                        },
                        "2026-09-03T09:03:00.000Z",
                    )
                    .unwrap();
                    assert_eq!(
                        changed_revision.error.as_ref().map(ErrorEnvelope::code),
                        Some(ErrorCode::AdoptionReplayMismatch),
                        "{legacy_kind} changed expectedRevision"
                    );
                }
            }
        }

        assert_eq!(adoption_count(&conn), 6);
    }

    #[test]
    fn exact_v1_validation_replay_sanitizes_legacy_response_and_mismatch_preserves_storage() {
        let (_temp, conn) = temp_store();

        for command_kind in ["create", "revise"] {
            let id = format!("legacy-secret-{command_kind}");
            let body = definition(&id, "Legacy secret response");
            let command = match command_kind {
                "create" => DefinitionCommand::Create {
                    definition: body.clone(),
                },
                "revise" => DefinitionCommand::Revise {
                    definition: body.clone(),
                    expected_revision: Some(7),
                },
                _ => unreachable!(),
            };
            let current_revision = PositiveInteger::new(4).unwrap();
            let secret = format!("SECRET_{command_kind}_VALIDATION_VALUE");
            let legacy_error = ErrorEnvelope::try_new(
                ErrorCode::ValidationFailed,
                format!("legacy validation exposed {secret}"),
                true,
            )
            .unwrap()
            .with_details(BTreeMap::from([(
                "submittedValue".to_owned(),
                Value::String(secret.clone()),
            )]))
            .with_adoption(ErrorAdoption {
                key: AdoptionKey::new(format!("adopt:legacy:{command_kind}:secret")).unwrap(),
                conflict_outcome: Some(AdoptionConflictOutcome::Rejected),
            })
            .with_current_revision(current_revision);
            let legacy_response = serde_json::to_string(&StoredResponse::Rejected {
                error: legacy_error,
            })
            .unwrap();
            let adoption_key = format!("adopt:{command_kind}:legacy-secret:0001");
            conn.execute(
                "INSERT INTO automation_command_adoptions (
                    adoption_key, request_digest, command, automation_id, outcome,
                    revision, response_json, adopted_at
                 ) VALUES (?1, ?2, ?3, ?4, 'rejected', 4, ?5, ?6)",
                params![
                    adoption_key,
                    legacy_v1_command_digest(&command, "valid"),
                    command_identity(&command).0,
                    id,
                    legacy_response,
                    "2026-09-03T09:00:00.000Z",
                ],
            )
            .unwrap();

            let replay = execute_definition_command(
                &conn,
                &adoption_key,
                command.clone(),
                "2026-09-03T09:01:00.000Z",
            )
            .unwrap();
            assert_eq!(replay.outcome, DefinitionCommandOutcome::Rejected);
            let replay_error = replay.error.as_ref().unwrap();
            assert_eq!(replay_error.code(), ErrorCode::ValidationFailed);
            assert_eq!(
                replay_error.message.as_str(),
                DEFINITION_VALIDATION_FAILED_MESSAGE
            );
            assert!(!replay_error.retryable);
            assert!(replay_error.details.is_none());
            assert!(replay_error.adoption.is_none());
            assert_eq!(
                replay_error.current_revision.map(PositiveInteger::get),
                Some(4)
            );
            let replay_json = serde_json::to_string(replay_error).unwrap();
            assert!(!replay_json.contains(&secret));

            let sanitized_response: String = conn
                .query_row(
                    "SELECT response_json
                     FROM automation_command_adoptions
                     WHERE adoption_key = ?1",
                    [&adoption_key],
                    |row| row.get(0),
                )
                .unwrap();
            assert!(!sanitized_response.contains(&secret));
            assert_eq!(
                sanitized_response,
                serde_json::to_string(&StoredResponse::Rejected {
                    error: replay_error.clone(),
                })
                .unwrap()
            );

            let repeated = execute_definition_command(
                &conn,
                &adoption_key,
                command.clone(),
                "2026-09-03T09:02:00.000Z",
            )
            .unwrap();
            assert_eq!(repeated.error, replay.error);
            let repeated_response: String = conn
                .query_row(
                    "SELECT response_json
                     FROM automation_command_adoptions
                     WHERE adoption_key = ?1",
                    [&adoption_key],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(repeated_response, sanitized_response);

            let mismatch_key = format!("adopt:{command_kind}:legacy-secret-mismatch:0001");
            let mismatch_secret = format!("SECRET_{command_kind}_MISMATCH_VALUE");
            let mismatch_error = ErrorEnvelope::try_new(
                ErrorCode::ValidationFailed,
                format!("legacy validation exposed {mismatch_secret}"),
                false,
            )
            .unwrap()
            .with_details(BTreeMap::from([(
                "submittedValue".to_owned(),
                Value::String(mismatch_secret.clone()),
            )]));
            let mismatch_response = serde_json::to_string(&StoredResponse::Rejected {
                error: mismatch_error,
            })
            .unwrap();
            conn.execute(
                "INSERT INTO automation_command_adoptions (
                    adoption_key, request_digest, command, automation_id, outcome,
                    revision, response_json, adopted_at
                 ) VALUES (?1, ?2, ?3, ?4, 'rejected', NULL, ?5, ?6)",
                params![
                    mismatch_key,
                    legacy_v1_command_digest(&command, "valid"),
                    command_identity(&command).0,
                    id,
                    mismatch_response,
                    "2026-09-03T09:00:00.000Z",
                ],
            )
            .unwrap();
            let mut changed_body = body;
            changed_body["retry"] = json!({
                "maxAttempts": 1,
                "backoffPolicy": "none",
                "retryableClasses": []
            });
            let changed_command = match command_kind {
                "create" => DefinitionCommand::Create {
                    definition: changed_body,
                },
                "revise" => DefinitionCommand::Revise {
                    definition: changed_body,
                    expected_revision: Some(7),
                },
                _ => unreachable!(),
            };
            let mismatch = execute_definition_command(
                &conn,
                &mismatch_key,
                changed_command,
                "2026-09-03T09:03:00.000Z",
            )
            .unwrap();
            assert_eq!(
                mismatch.error.as_ref().map(ErrorEnvelope::code),
                Some(ErrorCode::AdoptionReplayMismatch)
            );
            let unchanged_response: String = conn
                .query_row(
                    "SELECT response_json
                     FROM automation_command_adoptions
                     WHERE adoption_key = ?1",
                    [&mismatch_key],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(unchanged_response, mismatch_response);
            assert!(unchanged_response.contains(&mismatch_secret));
        }
    }

    #[test]
    fn reservations_accept_every_legacy_v1_definition_digest_form_only_for_exact_commands() {
        let (_temp, conn) = temp_store();

        for command_kind in ["create", "revise"] {
            for legacy_kind in ["valid", "invalid", "unsupported"] {
                let id = format!("reserved-{command_kind}-{legacy_kind}-digest");
                let body = definition(&id, "Reserved legacy digest");
                let command = match command_kind {
                    "create" => DefinitionCommand::Create {
                        definition: body.clone(),
                    },
                    "revise" => DefinitionCommand::Revise {
                        definition: body.clone(),
                        expected_revision: Some(3),
                    },
                    _ => unreachable!(),
                };
                let adoption_key =
                    format!("adopt:{command_kind}:reserved-{legacy_kind}-digest:0001");
                conn.execute(
                    "INSERT INTO automation_command_reservations (
                         adoption_key, request_digest, command, reserved_at
                     ) VALUES (?1, ?2, ?3, ?4)",
                    params![
                        adoption_key,
                        legacy_v1_command_digest(&command, legacy_kind),
                        command_identity(&command).0,
                        "2026-09-03T09:00:00.000Z",
                    ],
                )
                .unwrap();

                let pending = execute_definition_command(
                    &conn,
                    &adoption_key,
                    command.clone(),
                    "2026-09-03T09:01:00.000Z",
                )
                .unwrap();
                assert_eq!(
                    pending.error.as_ref().map(ErrorEnvelope::code),
                    Some(ErrorCode::CancelPending),
                    "{command_kind} {legacy_kind}"
                );

                let mut changed_body = body;
                changed_body["name"] = json!("Changed reservation payload");
                let changed_command = match command_kind {
                    "create" => DefinitionCommand::Create {
                        definition: changed_body,
                    },
                    "revise" => DefinitionCommand::Revise {
                        definition: changed_body,
                        expected_revision: Some(3),
                    },
                    _ => unreachable!(),
                };
                let mismatch = execute_definition_command(
                    &conn,
                    &adoption_key,
                    changed_command,
                    "2026-09-03T09:02:00.000Z",
                )
                .unwrap();
                assert_eq!(
                    mismatch.error.as_ref().map(ErrorEnvelope::code),
                    Some(ErrorCode::AdoptionReplayMismatch),
                    "{command_kind} {legacy_kind} changed payload"
                );
            }
        }
    }

    #[test]
    fn unsupported_create_is_durably_rejected_without_mutation_or_event() {
        let (_temp, conn) = temp_store();
        let mut unsupported = definition("unsupported-create", "Unsupported");
        unsupported["outputTarget"] = json!("result.md");
        let command = DefinitionCommand::Create {
            definition: unsupported.clone(),
        };

        let first = execute_definition_command(
            &conn,
            "adopt:create:unsupported:0001",
            command.clone(),
            "2026-09-03T09:00:00.000Z",
        )
        .unwrap();
        assert_eq!(first.outcome, DefinitionCommandOutcome::Rejected);
        let first_error = first.error.as_ref().unwrap();
        assert_eq!(first_error.code(), ErrorCode::CapabilityUnsupported);
        let first_error = serde_json::to_value(first_error).unwrap();
        assert_eq!(
            first_error["details"],
            json!({
                "contractProfile": "coven.automations.v1",
                "reason": "The wire shape is reserved, but no current implementation has a pinned-revision, crash-recoverable delivery state machine. Refuse with CAPABILITY_UNSUPPORTED.",
                "variant": "outputTarget.atomic"
            })
        );
        assert!(get_definition(&conn, "unsupported-create")
            .unwrap()
            .is_none());
        assert_eq!(definition_event_count(&conn, "unsupported-create"), 0);
        assert_eq!(adoption_count(&conn), 1);

        let replay = execute_definition_command(
            &conn,
            "adopt:create:unsupported:0001",
            command,
            "2026-09-03T09:01:00.000Z",
        )
        .unwrap();
        assert_eq!(replay.outcome, DefinitionCommandOutcome::Rejected);
        assert_eq!(
            serde_json::to_value(replay.error.unwrap()).unwrap(),
            first_error
        );
        assert_eq!(adoption_count(&conn), 1);
        assert_eq!(definition_event_count(&conn, "unsupported-create"), 0);

        unsupported["outputTarget"] = json!("different.md");
        let mismatch = execute_definition_command(
            &conn,
            "adopt:create:unsupported:0001",
            DefinitionCommand::Create {
                definition: unsupported,
            },
            "2026-09-03T09:02:00.000Z",
        )
        .unwrap();
        assert_eq!(
            mismatch.error.unwrap().code(),
            ErrorCode::AdoptionReplayMismatch
        );
        assert_eq!(adoption_count(&conn), 1);
        assert_eq!(definition_event_count(&conn, "unsupported-create"), 0);
    }

    #[test]
    fn unsupported_partial_create_remains_a_durable_validation_rejection() {
        let (_temp, conn) = temp_store();
        let command = DefinitionCommand::Create {
            definition: json!({"misfire": "backfill"}),
        };

        let first = execute_definition_command(
            &conn,
            "adopt:create:partial-unsupported:0001",
            command.clone(),
            "2026-09-03T09:00:00.000Z",
        )
        .unwrap();

        assert_eq!(first.outcome, DefinitionCommandOutcome::Rejected);
        assert_eq!(
            first.error.as_ref().map(ErrorEnvelope::code),
            Some(ErrorCode::ValidationFailed)
        );
        assert_eq!(adoption_count(&conn), 1);
        assert!(list_definitions(&conn).unwrap().is_empty());

        let replay = execute_definition_command(
            &conn,
            "adopt:create:partial-unsupported:0001",
            command,
            "2026-09-03T09:01:00.000Z",
        )
        .unwrap();
        assert_eq!(replay.outcome, DefinitionCommandOutcome::Rejected);
        assert_eq!(replay.error, first.error);
        assert_eq!(adoption_count(&conn), 1);

        let changed = execute_definition_command(
            &conn,
            "adopt:create:partial-unsupported:0001",
            DefinitionCommand::Create {
                definition: definition("partial-unsupported", "Corrected"),
            },
            "2026-09-03T09:02:00.000Z",
        )
        .unwrap();
        assert_eq!(
            changed.error.as_ref().map(ErrorEnvelope::code),
            Some(ErrorCode::AdoptionReplayMismatch)
        );
        assert!(list_definitions(&conn).unwrap().is_empty());
    }

    #[test]
    fn unsupported_create_does_not_mask_independent_validation_failures() {
        let (_temp, conn) = temp_store();

        let mut malformed_retry = definition("malformed-retry", "Malformed retry");
        malformed_retry["outputTarget"] = json!("result.md");
        malformed_retry["retry"] = json!({
            "maxAttempts": 3,
            "backoffPolicy": ["linear"]
        });

        let mut unknown_field = definition("unknown-field", "Unknown field");
        unknown_field["outputTarget"] = json!("result.md");
        unknown_field["futureField"] = json!("must fail closed");

        let mut malformed_rich_policy =
            definition("malformed-rich-policy", "Malformed rich policy");
        malformed_rich_policy["outputTarget"] = json!("result.md");
        malformed_rich_policy["policies"] = json!({
            "retry": {
                "maxAttempts": 2,
                "backoffPolicy": "none",
                "retryableClasses": "runtime_unavailable"
            }
        });

        let mut malformed_rrule = definition("malformed-rrule", "Malformed RRULE");
        malformed_rrule["outputTarget"] = json!("result.md");
        malformed_rrule["rrule"] = json!("FREQ=DAILY;BYHOUR=not-a-number");

        let mut malformed_retention = definition("malformed-retention", "Malformed retention");
        malformed_retention["outputTarget"] = json!("result.md");
        malformed_retention["policies"] = json!({
            "retention": {
                "occurrenceHistory": {"classification": 1}
            }
        });

        for (index, invalid) in [
            malformed_retry,
            unknown_field,
            malformed_rich_policy,
            malformed_rrule,
            malformed_retention,
        ]
        .into_iter()
        .enumerate()
        {
            let response = execute_definition_command(
                &conn,
                &format!("adopt:create:unsupported-invalid:{index:04}"),
                DefinitionCommand::Create {
                    definition: invalid,
                },
                "2026-09-03T09:00:00.000Z",
            )
            .unwrap();

            assert_eq!(
                response.error.as_ref().map(ErrorEnvelope::code),
                Some(ErrorCode::ValidationFailed),
                "case {index}"
            );
        }

        assert_eq!(adoption_count(&conn), 5);
        assert!(list_definitions(&conn).unwrap().is_empty());
    }

    #[test]
    fn v1_create_and_revise_validation_failures_are_secret_free_and_durable() {
        let (_temp, conn) = temp_store();
        let cases = [
            (
                "retryable-class",
                "SECRET_RETRYABLE_CLASS must not escape",
                {
                    let mut value = definition("secret-retryable-class", "Secret retry class");
                    value["retry"] = json!({
                        "maxAttempts": 2,
                        "backoffPolicy": "none",
                        "retryableClasses": ["SECRET_RETRYABLE_CLASS must not escape"]
                    });
                    value
                },
            ),
            (
                "union-discriminator",
                "SECRET_UNION_DISCRIMINATOR must not escape",
                {
                    let mut value =
                        definition("secret-union-discriminator", "Secret union discriminator");
                    value["action"] = json!({
                        "variant": "SECRET_UNION_DISCRIMINATOR must not escape",
                        "version": 1
                    });
                    value
                },
            ),
            ("timezone", "SECRET_TIMEZONE must not escape", {
                let mut value = definition("secret-timezone", "Secret timezone");
                value["timezone"] = json!("SECRET_TIMEZONE must not escape");
                value
            }),
            ("rrule", "SECRET_RRULE_VALUE", {
                let mut value = definition("secret-rrule", "Secret RRULE");
                value["rrule"] = json!("FREQ=DAILY;BYHOUR=SECRET_RRULE_VALUE");
                value
            }),
        ];

        for command_kind in ["create", "revise"] {
            for (case, secret, invalid) in &cases {
                let command = match command_kind {
                    "create" => DefinitionCommand::Create {
                        definition: invalid.clone(),
                    },
                    "revise" => DefinitionCommand::Revise {
                        definition: invalid.clone(),
                        expected_revision: Some(1),
                    },
                    _ => unreachable!(),
                };
                let adoption_key = format!("adopt:{command_kind}:secret-free-{case}:0001");
                let first = execute_definition_command(
                    &conn,
                    &adoption_key,
                    command.clone(),
                    "2026-09-03T09:00:00.000Z",
                )
                .unwrap();

                assert_eq!(
                    first.error.as_ref().map(ErrorEnvelope::code),
                    Some(ErrorCode::ValidationFailed),
                    "{command_kind} {case}"
                );
                assert_eq!(
                    first.error.as_ref().map(|error| error.message.as_str()),
                    Some("automation definition failed validation"),
                    "{command_kind} {case}"
                );
                let first_error = serde_json::to_string(first.error.as_ref().unwrap()).unwrap();
                assert!(!first_error.contains(secret), "{command_kind} {case}");
                assert!(
                    !first_error.contains("CAPABILITY_UNSUPPORTED"),
                    "{command_kind} {case}"
                );

                let stored_json: String = conn
                    .query_row(
                        "SELECT response_json
                         FROM automation_command_adoptions
                         WHERE adoption_key = ?1",
                        [&adoption_key],
                        |row| row.get(0),
                    )
                    .unwrap();
                assert!(!stored_json.contains(secret), "{command_kind} {case}");
                assert!(
                    stored_json.contains("automation definition failed validation"),
                    "{command_kind} {case}"
                );

                let replay = execute_definition_command(
                    &conn,
                    &adoption_key,
                    command,
                    "2026-09-03T09:01:00.000Z",
                )
                .unwrap();
                assert_eq!(replay.error, first.error, "{command_kind} {case} replay");
                let replay_error = serde_json::to_string(replay.error.as_ref().unwrap()).unwrap();
                assert!(
                    !replay_error.contains(secret),
                    "{command_kind} {case} replay"
                );
            }
        }
    }

    #[test]
    fn rich_policy_variants_are_refused_only_when_the_flat_definition_is_valid() {
        let (_temp, conn) = temp_store();

        let mut retry = definition("unsupported-retry-class", "Unsupported retry class");
        retry["policies"] = json!({
            "retry": {
                "maxAttempts": 2,
                "backoffPolicy": "none",
                "retryableClasses": ["transient_dispatch", "ambiguous"]
            }
        });
        let retry_response = execute_definition_command(
            &conn,
            "adopt:create:unsupported-retry-class:0001",
            DefinitionCommand::Create { definition: retry },
            "2026-09-03T09:00:00.000Z",
        )
        .unwrap();
        assert_eq!(
            retry_response.error.as_ref().map(ErrorEnvelope::code),
            Some(ErrorCode::CapabilityUnsupported)
        );
        assert_eq!(
            serde_json::to_value(retry_response.error.unwrap()).unwrap()["details"]["variant"],
            "retry.safe-classes.ambiguous"
        );

        let mut retention = definition("unsupported-retention", "Unsupported retention");
        retention["policies"] = json!({
            "retention": {
                "occurrenceHistory": {"classification": "standard"},
                "receipts": {"classification": "extended"}
            }
        });
        let retention_response = execute_definition_command(
            &conn,
            "adopt:create:unsupported-retention:0001",
            DefinitionCommand::Create {
                definition: retention,
            },
            "2026-09-03T09:00:00.000Z",
        )
        .unwrap();
        assert_eq!(
            retention_response.error.as_ref().map(ErrorEnvelope::code),
            Some(ErrorCode::CapabilityUnsupported)
        );
        assert_eq!(
            serde_json::to_value(retention_response.error.unwrap()).unwrap()["details"]["variant"],
            "retention.extended"
        );

        assert_eq!(adoption_count(&conn), 2);
        assert!(list_definitions(&conn).unwrap().is_empty());
    }

    #[test]
    fn unsupported_revise_is_durably_rejected_without_revision_or_event_change() {
        let (_temp, conn) = temp_store();
        execute_definition_command(
            &conn,
            "adopt:create:revise-target:0001",
            DefinitionCommand::Create {
                definition: definition("revise-target", "Original"),
            },
            "2026-09-03T09:00:00.000Z",
        )
        .unwrap();
        assert_eq!(definition_event_count(&conn, "revise-target"), 1);

        let mut unsupported = definition("revise-target", "Must not land");
        unsupported["misfire"] = json!("backfill");
        let command = DefinitionCommand::Revise {
            definition: unsupported.clone(),
            expected_revision: Some(1),
        };
        let first = execute_definition_command(
            &conn,
            "adopt:revise:unsupported:0002",
            command.clone(),
            "2026-09-03T09:01:00.000Z",
        )
        .unwrap();
        assert_eq!(first.outcome, DefinitionCommandOutcome::Rejected);
        let first_error = first.error.as_ref().unwrap();
        assert_eq!(first_error.code(), ErrorCode::CapabilityUnsupported);
        let first_error = serde_json::to_value(first_error).unwrap();
        assert_eq!(first_error["details"]["variant"], "misfire.backfill");
        assert_eq!(
            first_error["details"]["contractProfile"],
            "coven.automations.v1"
        );
        let stored = get_definition(&conn, "revise-target").unwrap().unwrap();
        assert_eq!(stored.revision, 1);
        assert!(stored.definition_json.contains(r#""name":"Original""#));
        assert_eq!(definition_event_count(&conn, "revise-target"), 1);

        let replay = execute_definition_command(
            &conn,
            "adopt:revise:unsupported:0002",
            command,
            "2026-09-03T09:02:00.000Z",
        )
        .unwrap();
        assert_eq!(replay.outcome, DefinitionCommandOutcome::Rejected);
        assert_eq!(
            serde_json::to_value(replay.error.unwrap()).unwrap(),
            first_error
        );
        assert_eq!(definition_event_count(&conn, "revise-target"), 1);

        unsupported["misfire"] = json!("all");
        let mismatch = execute_definition_command(
            &conn,
            "adopt:revise:unsupported:0002",
            DefinitionCommand::Revise {
                definition: unsupported,
                expected_revision: Some(1),
            },
            "2026-09-03T09:03:00.000Z",
        )
        .unwrap();
        assert_eq!(
            mismatch.error.unwrap().code(),
            ErrorCode::AdoptionReplayMismatch
        );
        assert_eq!(adoption_count(&conn), 2);
        assert_eq!(definition_event_count(&conn, "revise-target"), 1);
    }

    #[test]
    fn legacy_create_keeps_output_target_validation_behavior() {
        let (_temp, conn) = temp_store();
        let mut unsupported = definition("legacy-output", "Legacy output");
        unsupported["outputTarget"] = json!("result.md");

        let response = execute_definition_command(
            &conn,
            "legacy-output-target",
            DefinitionCommand::LegacyCreate {
                definition: unsupported,
            },
            "2026-09-03T09:00:00.000Z",
        )
        .unwrap();

        assert_eq!(response.error.unwrap().code(), ErrorCode::ValidationFailed);
    }

    #[test]
    fn adoption_ledger_blocks_arbitrary_updates_and_deletes() {
        let (_temp, conn) = temp_store();
        execute_definition_command(
            &conn,
            "adopt:create:immutable:0001",
            DefinitionCommand::Create {
                definition: definition("immutable", "Immutable"),
            },
            "2026-09-03T09:00:00.000Z",
        )
        .unwrap();

        assert!(conn
            .execute(
                "UPDATE automation_command_adoptions
                 SET request_digest = 'changed'
                 WHERE adoption_key = 'adopt:create:immutable:0001'",
                [],
            )
            .is_err());
        assert!(conn
            .execute(
                "UPDATE automation_command_adoptions
                 SET response_json = '{\"outcome\":\"rejected\",\"error\":{\"code\":\"VALIDATION_FAILED\",\"httpStatus\":400,\"message\":\"automation definition failed validation\",\"retryable\":false}}'
                 WHERE adoption_key = 'adopt:create:immutable:0001'",
                [],
            )
            .is_err());
        for rowid_alias in ["rowid", "_rowid_", "oid"] {
            assert!(conn
                .execute(
                    &format!(
                        "UPDATE automation_command_adoptions
                         SET {rowid_alias} = {rowid_alias} + 1
                         WHERE adoption_key = 'adopt:create:immutable:0001'"
                    ),
                    [],
                )
                .is_err());
        }
        assert!(conn
            .execute(
                "DELETE FROM automation_command_adoptions
                 WHERE adoption_key = 'adopt:create:immutable:0001'",
                [],
            )
            .is_err());
        assert_eq!(adoption_count(&conn), 1);
    }

    #[test]
    fn startup_rejects_keys_that_are_both_reserved_and_adopted() {
        let (_temp, conn) = temp_store();
        conn.execute(
            "INSERT INTO automation_command_reservations (
                 adoption_key, request_digest, command, reserved_at
             ) VALUES (?1, 'digest', 'definition.create.v1', ?2)",
            params![
                "adopt:create:reservation-adoption-collision",
                "2026-09-03T09:00:00.000Z"
            ],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO automation_command_adoptions (
                 adoption_key, request_digest, command, automation_id, outcome,
                 revision, response_json, adopted_at
             ) VALUES (?1, 'digest', 'definition.create.v1', NULL, 'rejected',
                       NULL, ?2, ?3)",
            params![
                "adopt:create:reservation-adoption-collision",
                r#"{"outcome":"rejected","error":{"code":"VALIDATION_FAILED","message":"invalid","retryable":false}}"#,
                "2026-09-03T09:00:00.000Z"
            ],
        )
        .unwrap();

        let error = ensure_global_adoption_key_guards(&conn)
            .expect_err("startup must reject split command adoption ownership");
        assert!(
            error.to_string().contains("is both reserved and adopted"),
            "{error:#}"
        );
    }

    #[test]
    fn invalid_adoption_key_is_a_typed_validation_rejection() {
        let (_temp, conn) = temp_store();
        let rejected = execute_definition_command(
            &conn,
            "short",
            DefinitionCommand::Create {
                definition: definition("invalid-key", "Invalid key"),
            },
            "2026-09-03T09:00:00.000Z",
        )
        .unwrap();

        assert_eq!(rejected.outcome, DefinitionCommandOutcome::Rejected);
        assert_eq!(
            rejected.error.as_ref().map(ErrorEnvelope::code),
            Some(ErrorCode::ValidationFailed)
        );
        assert!(list_definitions(&conn).unwrap().is_empty());
        assert_eq!(adoption_count(&conn), 0);
    }

    #[test]
    fn unsafe_integer_payload_is_durably_rejected_instead_of_returning_internal_error() {
        let (_temp, conn) = temp_store();
        let mut invalid = definition("unsafe-integer", "Unsafe integer");
        invalid["timeoutMinutes"] = json!(9_007_199_254_740_992_u64);

        let rejected = execute_definition_command(
            &conn,
            "adopt:create:unsafe-integer:0001",
            DefinitionCommand::Create {
                definition: invalid,
            },
            "2026-09-03T09:00:00.000Z",
        )
        .unwrap();
        assert_eq!(rejected.outcome, DefinitionCommandOutcome::Rejected);
        assert_eq!(
            rejected.error.as_ref().map(ErrorEnvelope::code),
            Some(ErrorCode::ValidationFailed)
        );
        assert_eq!(adoption_count(&conn), 1);

        let changed = execute_definition_command(
            &conn,
            "adopt:create:unsafe-integer:0001",
            DefinitionCommand::Create {
                definition: definition("unsafe-integer", "Corrected"),
            },
            "2026-09-03T09:01:00.000Z",
        )
        .unwrap();
        assert_eq!(
            changed.error.as_ref().map(ErrorEnvelope::code),
            Some(ErrorCode::AdoptionReplayMismatch)
        );
        assert!(list_definitions(&conn).unwrap().is_empty());
        assert_eq!(adoption_count(&conn), 1);
    }

    #[test]
    fn changed_wire_payload_does_not_replay_first_result() {
        let (_temp, conn) = temp_store();
        let command = DefinitionCommand::Create {
            definition: definition("daily", "Daily"),
        };
        let mut replay_definition = definition("daily", "Daily");
        replay_definition["familiarId"] = Value::Null;

        let first = execute_definition_command(
            &conn,
            "adopt:create:daily:0001",
            command,
            "2026-09-03T09:00:00.000Z",
        )
        .unwrap();
        let replay = execute_definition_command(
            &conn,
            "adopt:create:daily:0001",
            DefinitionCommand::Create {
                definition: replay_definition,
            },
            "2026-09-03T09:01:00.000Z",
        )
        .unwrap();

        assert_eq!(first.outcome, DefinitionCommandOutcome::Committed);
        assert_eq!(replay.outcome, DefinitionCommandOutcome::Rejected);
        assert_eq!(
            replay.error.as_ref().map(ErrorEnvelope::code),
            Some(ErrorCode::AdoptionReplayMismatch)
        );
        assert_eq!(first.revision, Some(1));
        assert_eq!(list_definitions(&conn).unwrap().len(), 1);
        assert_eq!(adoption_count(&conn), 1);
    }

    #[cfg(unix)]
    #[test]
    fn local_command_fingerprint_is_stable_across_timezone_environments() {
        const CHILD_ENV: &str = "COVEN_TEST_LOCAL_COMMAND_FINGERPRINT_CHILD";
        const TEST_NAME: &str =
            "automations::command_adoption::tests::local_command_fingerprint_is_stable_across_timezone_environments";

        if std::env::var_os(CHILD_ENV).is_some() {
            let command = DefinitionCommand::Create {
                definition: definition("stable-local-fingerprint", "Stable local fingerprint"),
            };
            let canonical = canonical_command(&command).unwrap();
            assert_eq!(canonical["definition"]["kind"], "wire");
            println!("fingerprint={}", definition_command_fingerprint(&command));
            return;
        }

        let fingerprint_for = |timezone: &str| {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", TEST_NAME, "--nocapture"])
                .env(CHILD_ENV, "1")
                .env("TZ", timezone)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "child fingerprint assertion failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            String::from_utf8(output.stdout)
                .unwrap()
                .lines()
                .find_map(|line| line.strip_prefix("fingerprint="))
                .expect("child emitted fingerprint")
                .to_owned()
        };

        assert_eq!(
            fingerprint_for("Pacific/Kiritimati"),
            fingerprint_for("America/Chicago")
        );
    }

    #[cfg(unix)]
    #[test]
    fn local_resolution_failure_is_durably_rejected_and_replayed() {
        const CHILD_ENV: &str = "COVEN_TEST_LOCAL_RESOLUTION_REJECTION_CHILD";
        const TEST_NAME: &str =
            "automations::command_adoption::tests::local_resolution_failure_is_durably_rejected_and_replayed";
        const SECRET: &str = "SECRET_TZ_OVERRIDE must not escape";

        if std::env::var_os(CHILD_ENV).is_some() {
            let (_temp, conn) = temp_store();
            let command = DefinitionCommand::Create {
                definition: definition("local-resolution-failure", "Local resolution failure"),
            };
            unsafe {
                std::env::set_var("TZ", SECRET);
            }
            let first = execute_definition_command(
                &conn,
                "adopt:create:local-resolution-failure:0001",
                command.clone(),
                "2026-09-03T09:00:00.000Z",
            )
            .unwrap();

            assert_eq!(first.outcome, DefinitionCommandOutcome::Rejected);
            assert_eq!(
                first.error.as_ref().map(ErrorEnvelope::code),
                Some(ErrorCode::ValidationFailed)
            );
            assert_eq!(
                first.error.as_ref().map(|error| error.message.as_str()),
                Some("automation definition failed validation")
            );
            assert!(!serde_json::to_string(first.error.as_ref().unwrap())
                .unwrap()
                .contains(SECRET));
            assert!(get_definition(&conn, "local-resolution-failure")
                .unwrap()
                .is_none());
            assert_eq!(definition_event_count(&conn, "local-resolution-failure"), 0);
            assert_eq!(adoption_count(&conn), 1);
            let stored_json: String = conn
                .query_row(
                    "SELECT response_json
                     FROM automation_command_adoptions
                     WHERE adoption_key = 'adopt:create:local-resolution-failure:0001'",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert!(!stored_json.contains(SECRET));

            unsafe {
                std::env::set_var("TZ", "Pacific/Kiritimati");
            }
            let replay = execute_definition_command(
                &conn,
                "adopt:create:local-resolution-failure:0001",
                command,
                "2026-09-03T09:01:00.000Z",
            )
            .unwrap();

            assert_eq!(replay.outcome, DefinitionCommandOutcome::Rejected);
            assert_eq!(replay.error, first.error);
            assert!(get_definition(&conn, "local-resolution-failure")
                .unwrap()
                .is_none());
            assert_eq!(definition_event_count(&conn, "local-resolution-failure"), 0);
            assert_eq!(adoption_count(&conn), 1);
            return;
        }

        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", TEST_NAME, "--nocapture"])
            .env(CHILD_ENV, "1")
            .status()
            .unwrap();

        assert!(status.success(), "child timezone assertion failed");
    }

    #[cfg(unix)]
    #[test]
    fn revise_local_resolution_failure_is_durable_without_mutation_or_event() {
        const CHILD_ENV: &str = "COVEN_TEST_REVISE_LOCAL_RESOLUTION_REJECTION_CHILD";
        const TEST_NAME: &str =
            "automations::command_adoption::tests::revise_local_resolution_failure_is_durable_without_mutation_or_event";

        if std::env::var_os(CHILD_ENV).is_some() {
            let (_temp, conn) = temp_store();
            let mut original = definition("local-revise-failure", "Original");
            original["timezone"] = json!("utc");
            execute_definition_command(
                &conn,
                "adopt:create:local-revise-failure:0001",
                DefinitionCommand::Create {
                    definition: original,
                },
                "2026-09-03T09:00:00.000Z",
            )
            .unwrap();

            let command = DefinitionCommand::Revise {
                definition: definition("local-revise-failure", "Must not land"),
                expected_revision: Some(1),
            };
            unsafe {
                std::env::set_var("TZ", ":/tmp/coven-invalid-zoneinfo");
            }
            let first = execute_definition_command(
                &conn,
                "adopt:revise:local-resolution-failure:0002",
                command.clone(),
                "2026-09-03T09:01:00.000Z",
            )
            .unwrap();

            assert_eq!(
                first.error.as_ref().map(ErrorEnvelope::code),
                Some(ErrorCode::ValidationFailed)
            );
            let stored = get_definition(&conn, "local-revise-failure")
                .unwrap()
                .unwrap();
            assert_eq!(stored.revision, 1);
            assert_eq!(stored.name, "Original");
            assert_eq!(definition_event_count(&conn, "local-revise-failure"), 1);

            unsafe {
                std::env::set_var("TZ", "America/Chicago");
            }
            let replay = execute_definition_command(
                &conn,
                "adopt:revise:local-resolution-failure:0002",
                command,
                "2026-09-03T09:02:00.000Z",
            )
            .unwrap();

            assert_eq!(replay.outcome, DefinitionCommandOutcome::Rejected);
            assert_eq!(replay.error, first.error);
            let stored = get_definition(&conn, "local-revise-failure")
                .unwrap()
                .unwrap();
            assert_eq!(stored.revision, 1);
            assert_eq!(stored.name, "Original");
            assert_eq!(definition_event_count(&conn, "local-revise-failure"), 1);
            assert_eq!(adoption_count(&conn), 2);
            return;
        }

        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", TEST_NAME, "--nocapture"])
            .env(CHILD_ENV, "1")
            .status()
            .unwrap();

        assert!(status.success(), "child timezone assertion failed");
    }

    #[cfg(unix)]
    #[test]
    fn local_compatibility_input_persists_the_exact_tz_override() {
        const CHILD_ENV: &str = "COVEN_TEST_LOCAL_PERSISTENCE_CHILD";
        const TEST_NAME: &str =
            "automations::command_adoption::tests::local_compatibility_input_persists_the_exact_tz_override";

        if std::env::var_os(CHILD_ENV).is_some() {
            let (_temp, conn) = temp_store();
            let response = execute_definition_command(
                &conn,
                "adopt:create:exact-local-normalized:0001",
                DefinitionCommand::Create {
                    definition: definition("exact-local-normalized", "Exact local normalized"),
                },
                "2026-09-03T09:00:00.000Z",
            )
            .unwrap();

            assert_eq!(response.outcome, DefinitionCommandOutcome::Committed);
            let record = get_definition(&conn, "exact-local-normalized")
                .unwrap()
                .unwrap();
            let stored: Value = serde_json::from_str(&record.definition_json).unwrap();
            assert_eq!(stored["timezone"], "Pacific/Kiritimati");
            assert_eq!(
                response.result.unwrap()["routine"]["timezone"],
                "Pacific/Kiritimati"
            );
            return;
        }

        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", TEST_NAME, "--nocapture"])
            .env(CHILD_ENV, "1")
            .env("TZ", "Pacific/Kiritimati")
            .status()
            .unwrap();

        assert!(status.success(), "child timezone assertion failed");
    }

    #[test]
    fn local_compatibility_input_is_resolved_before_commit() {
        let (_temp, conn) = temp_store();

        let response = execute_definition_command(
            &conn,
            "adopt:create:local-normalized:0001",
            DefinitionCommand::Create {
                definition: definition("local-normalized", "Local normalized"),
            },
            "2026-09-03T09:00:00.000Z",
        )
        .unwrap();

        assert_eq!(response.outcome, DefinitionCommandOutcome::Committed);
        let record = get_definition(&conn, "local-normalized").unwrap().unwrap();
        let stored: Value = serde_json::from_str(&record.definition_json).unwrap();
        assert_ne!(stored["timezone"], "local");
        assert_eq!(
            response.result.unwrap()["routine"]["timezone"],
            stored["timezone"]
        );
    }

    #[test]
    fn unknown_iana_timezone_is_a_durable_validation_rejection() {
        let (_temp, conn) = temp_store();
        let mut invalid = definition("invalid-timezone", "Invalid timezone");
        invalid["timezone"] = json!("Mars/Olympus");

        let response = execute_definition_command(
            &conn,
            "adopt:create:invalid-timezone:0001",
            DefinitionCommand::Create {
                definition: invalid,
            },
            "2026-09-03T09:00:00.000Z",
        )
        .unwrap();

        assert_eq!(response.outcome, DefinitionCommandOutcome::Rejected);
        assert_eq!(
            response.error.as_ref().map(ErrorEnvelope::code),
            Some(ErrorCode::ValidationFailed)
        );
        assert_eq!(
            response.error.unwrap().message.as_str(),
            "automation definition failed validation"
        );
        assert!(get_definition(&conn, "invalid-timezone").unwrap().is_none());
        assert_eq!(adoption_count(&conn), 1);
    }

    #[test]
    fn committed_definition_command_appends_one_typed_event_and_replay_appends_none() {
        let (_temp, conn) = temp_store();
        let command = DefinitionCommand::Create {
            definition: definition("evented", "Evented"),
        };

        let first = execute_definition_command(
            &conn,
            "adopt:create:evented:0001",
            command.clone(),
            "2026-09-03T09:00:00.000Z",
        )
        .unwrap();
        let replay = execute_definition_command(
            &conn,
            "adopt:create:evented:0001",
            command,
            "2026-09-03T09:01:00.000Z",
        )
        .unwrap();

        let events: Vec<(i64, String)> = conn
            .prepare(
                "SELECT sequence, event_json
                 FROM automation_events
                 WHERE stream_kind = 'automation' AND stream_id = 'evented'
                 ORDER BY sequence",
            )
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].0, 0);
        let event: crate::automations::contract::types::EventEnvelope =
            serde_json::from_str(&events[0].1).unwrap();
        let value = serde_json::to_value(event).unwrap();
        assert_eq!(value["schemaVersion"], "coven.automations.v1");
        assert_eq!(
            value["stream"],
            json!({"kind": "automation", "id": "evented"})
        );
        assert_eq!(value["automationId"], "evented");
        assert_eq!(value["kind"], "definition.created");
        assert_eq!(value["payload"]["revision"], 1);
        assert_eq!(
            value["causation"]["adoptionKey"],
            "adopt:create:evented:0001"
        );
        assert_eq!(first.event_ref, replay.event_ref);
        assert_eq!(first.event_ref.unwrap().sequence.get(), 0);
    }

    #[test]
    fn definition_lifecycle_events_are_gapless_and_rejections_append_nothing() {
        let (_temp, conn) = temp_store();
        execute_definition_command(
            &conn,
            "adopt:create:lifecycle:0001",
            DefinitionCommand::Create {
                definition: definition("lifecycle", "Lifecycle"),
            },
            "2026-09-03T09:00:00.000Z",
        )
        .unwrap();
        let mut revised = definition("lifecycle", "Lifecycle revised");
        revised["status"] = json!("ACTIVE");
        execute_definition_command(
            &conn,
            "adopt:revise:lifecycle:0002",
            DefinitionCommand::Revise {
                definition: revised.clone(),
                expected_revision: Some(1),
            },
            "2026-09-03T09:01:00.000Z",
        )
        .unwrap();
        let rejected = execute_definition_command(
            &conn,
            "adopt:revise:lifecycle:stale",
            DefinitionCommand::Revise {
                definition: revised,
                expected_revision: Some(1),
            },
            "2026-09-03T09:02:00.000Z",
        )
        .unwrap();
        assert_eq!(rejected.outcome, DefinitionCommandOutcome::Rejected);
        execute_definition_command(
            &conn,
            "adopt:tombstone:lifecycle:0003",
            DefinitionCommand::Delete {
                automation_id: "lifecycle".to_owned(),
                expected_revision: Some(2),
            },
            "2026-09-03T09:03:00.000Z",
        )
        .unwrap();

        let events: Vec<Value> = conn
            .prepare(
                "SELECT event_json
                 FROM automation_events
                 WHERE stream_kind = 'automation' AND stream_id = 'lifecycle'
                 ORDER BY sequence",
            )
            .unwrap()
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .map(|row| serde_json::from_str(&row.unwrap()).unwrap())
            .collect();
        assert_eq!(events.len(), 3);
        assert_eq!(
            events
                .iter()
                .map(|event| event["sequence"].as_u64().unwrap())
                .collect::<Vec<_>>(),
            vec![0, 1, 2]
        );
        assert_eq!(
            events
                .iter()
                .map(|event| event["kind"].as_str().unwrap())
                .collect::<Vec<_>>(),
            vec![
                "definition.created",
                "definition.revised",
                "definition.tombstoned"
            ]
        );
        assert_eq!(events[0]["payload"]["revision"], 1);
        assert_eq!(events[1]["payload"]["revision"], 2);
        assert_eq!(events[1]["payload"]["lifecycleState"], "active");
        assert_eq!(events[2]["payload"]["revision"], 3);
        assert_eq!(events[2]["payload"]["lifecycleState"], "tombstoned");
    }

    #[test]
    fn event_append_failure_rolls_back_definition_and_adoption() {
        let (_temp, conn) = temp_store();
        conn.execute_batch(
            "CREATE TRIGGER reject_automation_event
             BEFORE INSERT ON automation_events
             BEGIN
                 SELECT RAISE(ABORT, 'synthetic event failure');
             END;",
        )
        .unwrap();

        let error = execute_definition_command(
            &conn,
            "adopt:create:atomic-event:0001",
            DefinitionCommand::Create {
                definition: definition("atomic-event", "Atomic event"),
            },
            "2026-09-03T09:00:00.000Z",
        )
        .unwrap_err();

        assert!(format!("{error:#}").contains("synthetic event failure"));
        assert!(get_definition(&conn, "atomic-event").unwrap().is_none());
        assert_eq!(adoption_count(&conn), 0);
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM automation_events", [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM automation_event_stream_heads",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
            0
        );
    }

    #[test]
    fn committed_legacy_no_op_delete_does_not_fabricate_a_tombstone_event() {
        let (_temp, conn) = temp_store();
        execute_definition_command(
            &conn,
            "adopt:create:no-op-delete:0001",
            DefinitionCommand::Create {
                definition: definition("no-op-delete", "No-op delete"),
            },
            "2026-09-03T09:00:00.000Z",
        )
        .unwrap();

        let response = execute_definition_command(
            &conn,
            "legacy:delete:no-op-delete:0001",
            DefinitionCommand::LegacyDelete {
                automation_id: "no-op-delete".to_owned(),
            },
            "2026-09-03T09:01:00.000Z",
        )
        .unwrap();

        assert_eq!(response.outcome, DefinitionCommandOutcome::Committed);
        assert_eq!(response.result.unwrap()["deleted"], false);
        assert!(response.event_ref.is_none());
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM automation_events
                 WHERE stream_kind = 'automation' AND stream_id = 'no-op-delete'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
            1
        );
    }

    #[test]
    fn legacy_replay_ignores_unknown_fields_in_request_identity() {
        let (_temp, conn) = temp_store();
        let mut with_extension = definition("legacy-replay", "Legacy replay");
        with_extension["legacyExtension"] = json!({ "ignored": true });

        let first = execute_definition_command(
            &conn,
            "legacy:create:legacy-replay:0001",
            DefinitionCommand::LegacyCreate {
                definition: with_extension,
            },
            "2026-09-03T09:00:00.000Z",
        )
        .unwrap();
        let replay = execute_definition_command(
            &conn,
            "legacy:create:legacy-replay:0001",
            DefinitionCommand::LegacyCreate {
                definition: definition("legacy-replay", "Legacy replay"),
            },
            "2026-09-03T09:01:00.000Z",
        )
        .unwrap();

        assert_eq!(first.outcome, DefinitionCommandOutcome::Committed);
        assert_eq!(replay.outcome, DefinitionCommandOutcome::Replayed);
        assert_eq!(replay.result, first.result);
        assert_eq!(adoption_count(&conn), 1);
    }

    #[test]
    fn changed_request_with_same_key_is_rejected_as_replay_mismatch() {
        let (_temp, conn) = temp_store();
        execute_definition_command(
            &conn,
            "adopt:create:daily:0002",
            DefinitionCommand::Create {
                definition: definition("daily", "Daily"),
            },
            "2026-09-03T09:00:00.000Z",
        )
        .unwrap();

        let mismatch = execute_definition_command(
            &conn,
            "adopt:create:daily:0002",
            DefinitionCommand::Create {
                definition: definition("daily", "Changed"),
            },
            "2026-09-03T09:01:00.000Z",
        )
        .unwrap();

        assert_eq!(mismatch.outcome, DefinitionCommandOutcome::Rejected);
        assert_eq!(
            mismatch.error.as_ref().map(ErrorEnvelope::code),
            Some(ErrorCode::AdoptionReplayMismatch)
        );
        assert_eq!(
            get_definition(&conn, "daily").unwrap().unwrap().name,
            "Daily"
        );
        assert_eq!(adoption_count(&conn), 1);
    }

    #[test]
    fn matching_expected_revision_increments_exactly_once() {
        let (_temp, conn) = temp_store();
        execute_definition_command(
            &conn,
            "adopt:create:daily:0003",
            DefinitionCommand::Create {
                definition: definition("daily", "Daily"),
            },
            "2026-09-03T09:00:00.000Z",
        )
        .unwrap();
        let command = DefinitionCommand::Revise {
            definition: definition("daily", "Revised"),
            expected_revision: Some(1),
        };

        let revised = execute_definition_command(
            &conn,
            "adopt:revise:daily:0002",
            command.clone(),
            "2026-09-03T09:01:00.000Z",
        )
        .unwrap();
        let replay = execute_definition_command(
            &conn,
            "adopt:revise:daily:0002",
            command,
            "2026-09-03T09:02:00.000Z",
        )
        .unwrap();

        assert_eq!(revised.revision, Some(2));
        assert_eq!(replay.outcome, DefinitionCommandOutcome::Replayed);
        let stored = get_definition(&conn, "daily").unwrap().unwrap();
        assert_eq!(stored.name, "Revised");
        assert_eq!(stored.revision, 2);
    }

    #[test]
    fn backward_clock_revision_keeps_a_monotonic_effective_timestamp() {
        let (_temp, conn) = temp_store();
        execute_definition_command(
            &conn,
            "adopt:create:clock-regression:0001",
            DefinitionCommand::Create {
                definition: definition("clock-regression", "Clock regression"),
            },
            "2026-09-03T12:00:00.000Z",
        )
        .unwrap();
        let mut revised = definition("clock-regression", "Clock regression revised");
        revised["rrule"] = json!("FREQ=DAILY;BYHOUR=11");

        let response = execute_definition_command(
            &conn,
            "adopt:revise:clock-regression:0002",
            DefinitionCommand::Revise {
                definition: revised,
                expected_revision: Some(1),
            },
            "2026-09-03T10:00:00.000Z",
        )
        .unwrap();

        assert_eq!(response.outcome, DefinitionCommandOutcome::Committed);
        let (updated_at, adoption_at): (String, String) = conn
            .query_row(
                "SELECT definition.updated_at, adoption.adopted_at
                 FROM automation_definitions AS definition
                 JOIN automation_command_adoptions AS adoption
                   ON adoption.automation_id = definition.id
                  AND adoption.revision = definition.revision
                 WHERE definition.id = 'clock-regression'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(updated_at, "2026-09-03T12:00:00.000Z");
        assert_eq!(adoption_at, updated_at);
        let revised_event: Value = conn
            .query_row(
                "SELECT event_json
                 FROM automation_events
                 WHERE stream_kind = 'automation'
                   AND stream_id = 'clock-regression'
                   AND sequence = 1",
                [],
                |row| row.get::<_, String>(0),
            )
            .map(|event| serde_json::from_str(&event).unwrap())
            .unwrap();
        assert_eq!(revised_event["recordedAt"], updated_at);
        assert_eq!(revised_event["observedAt"], updated_at);
    }

    #[test]
    fn revision_timestamp_does_not_precede_the_latest_historical_event() {
        let (_temp, conn) = temp_store();
        let body = definition("event-clock-regression", "Event clock regression");
        let definition: RoutineDefinition = RoutineDefinition::from_json(&body)
            .unwrap()
            .resolve_timezone_for_persistence()
            .unwrap();
        let definition_json = serde_json::to_string(&definition).unwrap();
        let digest =
            super::super::contract::migration::definition_digest(&definition_json).unwrap();
        conn.execute(
            "INSERT INTO automation_definitions (
                id, name, status, definition_json, revision, definition_digest, lifecycle_state,
                tombstoned_at, authority_version, created_at, updated_at
             ) VALUES (
                'event-clock-regression', 'Event clock regression', 'PAUSED', ?1, 1, ?2,
                'paused', NULL, 1, '2026-09-03T10:00:00.000Z', '2026-09-03T10:00:00.000Z'
             )",
            params![definition_json, digest],
        )
        .unwrap();
        super::super::contract::events::append_imported_definition_event(
            &conn,
            super::super::contract::events::ImportedDefinitionEventInput {
                automation_id: "event-clock-regression",
                revision: 1,
                definition_digest: Some(&digest),
                lifecycle_state: "paused",
                imported_from: "legacy-coven-store",
                recorded_at: "2026-09-03T12:00:00.000Z",
                observed_at: "2026-09-03T10:00:00.000Z",
            },
        )
        .unwrap();
        super::super::contract::events::append_migrated_definition_event(
            &conn,
            super::super::contract::events::MigratedDefinitionEventInput {
                automation_id: "event-clock-regression",
                revision: 1,
                definition_digest: Some(&digest),
                lifecycle_state: "paused",
                migration: "pre-fix-clock-regression",
                recorded_at: "2026-09-03T10:00:00.000Z",
                observed_at: "2026-09-03T10:00:00.000Z",
            },
        )
        .unwrap();
        let mut revised = body;
        revised["name"] = json!("Event clock regression revised");

        execute_definition_command(
            &conn,
            "adopt:revise:event-clock-regression:0002",
            DefinitionCommand::Revise {
                definition: revised,
                expected_revision: Some(1),
            },
            "2026-09-03T11:00:00.000Z",
        )
        .unwrap();

        let updated_at: String = conn
            .query_row(
                "SELECT updated_at FROM automation_definitions
                 WHERE id = 'event-clock-regression'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(updated_at, "2026-09-03T12:00:00.000Z");
        let event: Value = conn
            .query_row(
                "SELECT event_json FROM automation_events
                 WHERE stream_kind = 'automation'
                   AND stream_id = 'event-clock-regression'
                  AND sequence = 2",
                [],
                |row| row.get::<_, String>(0),
            )
            .map(|event| serde_json::from_str(&event).unwrap())
            .unwrap();
        assert_eq!(event["recordedAt"], updated_at);
    }

    #[test]
    fn stale_expected_revision_rejects_without_mutation() {
        let (_temp, conn) = temp_store();
        execute_definition_command(
            &conn,
            "adopt:create:daily:0004",
            DefinitionCommand::Create {
                definition: definition("daily", "Daily"),
            },
            "2026-09-03T09:00:00.000Z",
        )
        .unwrap();

        let stale = execute_definition_command(
            &conn,
            "adopt:revise:daily:stale",
            DefinitionCommand::Revise {
                definition: definition("daily", "Should not land"),
                expected_revision: Some(7),
            },
            "2026-09-03T09:01:00.000Z",
        )
        .unwrap();

        assert_eq!(stale.outcome, DefinitionCommandOutcome::Rejected);
        assert_eq!(
            stale.error.as_ref().map(ErrorEnvelope::code),
            Some(ErrorCode::RevisionConflict)
        );
        assert_eq!(stale.revision, Some(1));
        let stored = get_definition(&conn, "daily").unwrap().unwrap();
        assert_eq!(stored.name, "Daily");
        assert_eq!(stored.revision, 1);
    }

    #[test]
    fn domain_validation_failure_is_persisted_as_rejected() {
        let (_temp, conn) = temp_store();
        let invalid = DefinitionCommand::Create {
            definition: definition("bad id!", "Invalid"),
        };

        let rejected = execute_definition_command(
            &conn,
            "adopt:create:invalid:0001",
            invalid.clone(),
            "2026-09-03T09:00:00.000Z",
        )
        .unwrap();
        let replay = execute_definition_command(
            &conn,
            "adopt:create:invalid:0001",
            invalid,
            "2026-09-03T09:01:00.000Z",
        )
        .unwrap();

        assert_eq!(rejected.outcome, DefinitionCommandOutcome::Rejected);
        assert_eq!(
            rejected.error.as_ref().map(ErrorEnvelope::code),
            Some(ErrorCode::ValidationFailed)
        );
        assert_eq!(replay.outcome, DefinitionCommandOutcome::Rejected);
        assert_eq!(replay.error, rejected.error);
        assert!(list_definitions(&conn).unwrap().is_empty());
        assert_eq!(adoption_count(&conn), 1);

        let changed = execute_definition_command(
            &conn,
            "adopt:create:invalid:0001",
            DefinitionCommand::Create {
                definition: definition("valid-now", "Corrected"),
            },
            "2026-09-03T09:02:00.000Z",
        )
        .unwrap();
        assert_eq!(changed.outcome, DefinitionCommandOutcome::Rejected);
        assert_eq!(
            changed.error.as_ref().map(ErrorEnvelope::code),
            Some(ErrorCode::AdoptionReplayMismatch)
        );
        assert!(list_definitions(&conn).unwrap().is_empty());
        assert_eq!(adoption_count(&conn), 1);
    }

    #[test]
    fn delete_tombstones_the_definition_and_replays_without_erasing_history() {
        let (_temp, conn) = temp_store();
        execute_definition_command(
            &conn,
            "adopt:create:tombstone:0001",
            DefinitionCommand::Create {
                definition: definition("tombstone", "Retained"),
            },
            "2026-09-03T09:00:00.000Z",
        )
        .unwrap();
        let command = DefinitionCommand::Delete {
            automation_id: "tombstone".to_owned(),
            expected_revision: Some(1),
        };

        let deleted = execute_definition_command(
            &conn,
            "adopt:delete:tombstone:0002",
            command.clone(),
            "2026-09-03T09:01:00.000Z",
        )
        .unwrap();
        let replay = execute_definition_command(
            &conn,
            "adopt:delete:tombstone:0002",
            command,
            "2026-09-03T09:02:00.000Z",
        )
        .unwrap();

        assert_eq!(deleted.outcome, DefinitionCommandOutcome::Committed);
        assert_eq!(deleted.revision, Some(2));
        assert_eq!(replay.outcome, DefinitionCommandOutcome::Replayed);
        assert!(get_definition(&conn, "tombstone").unwrap().is_none());
        let retained: (i64, String) = conn
            .query_row(
                "SELECT revision, tombstoned_at
                 FROM automation_definitions
                 WHERE id = 'tombstone'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(retained, (2, "2026-09-03T09:01:00.000Z".to_owned()));
        assert_eq!(adoption_count(&conn), 2);
    }
}
