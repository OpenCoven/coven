//! Transactional adoption for definition-mutating automation commands.

use std::collections::BTreeMap;

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::contract::canonical_json::{canonicalize, sha256_hex};
use super::contract::error::{AdoptionConflictOutcome, ErrorAdoption, ErrorCode, ErrorEnvelope};
use super::contract::types::{AdoptionKey, PositiveInteger};
use super::definition::RoutineDefinition;

pub const AUTOMATION_COMMAND_ADOPTIONS_SCHEMA_SQL: &str = "
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

    CREATE TRIGGER IF NOT EXISTS automation_command_adoptions_no_update
    BEFORE UPDATE ON automation_command_adoptions
    BEGIN
        SELECT RAISE(ABORT, 'automation command adoptions are append-only');
    END;

    CREATE TRIGGER IF NOT EXISTS automation_command_adoptions_no_delete
    BEFORE DELETE ON automation_command_adoptions
    BEGIN
        SELECT RAISE(ABORT, 'automation command adoptions are append-only');
    END;
";

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
    let canonical_command = canonical_command(&command)?;
    let request_digest = sha256_hex(
        &canonicalize(&canonical_command)
            .context("failed to canonicalize automation definition command")?,
    );
    let transaction = rusqlite::Transaction::new_unchecked(conn, TransactionBehavior::Immediate)
        .context("failed to begin automation command adoption transaction")?;

    if let Some(stored) = load_adoption(&transaction, adoption_key.as_str())? {
        let response = if stored.request_digest == request_digest {
            replay_response(stored)
        } else {
            replay_mismatch_response(&adoption_key, &stored)
        };
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

fn adoption_definition_preimage(definition: &Value) -> Result<Value> {
    match RoutineDefinition::from_json(definition) {
        Ok(definition) => Ok(json!({
            "kind": "valid",
            "value": serde_json::to_value(definition)
                .context("failed to normalize routine definition for adoption")?,
        })),
        Err(_) => Ok(json!({
            "kind": "invalid",
            "value": lossless_json_fingerprint(definition),
        })),
    }
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
    let definition = match RoutineDefinition::from_json(definition_value)
        .and_then(RoutineDefinition::resolve_timezone_for_persistence)
    {
        Ok(definition) => definition,
        Err(error) => {
            return Ok(rejected(ErrorCode::ValidationFailed, error, None));
        }
    };
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
    let definition = match RoutineDefinition::from_json(definition_value)
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
    if expected_revision.is_some_and(|expected| current.revision != expected) {
        return Ok(revision_conflict(current.revision));
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
            "SELECT revision, tombstoned_at IS NOT NULL, authority_version
             FROM automation_definitions
             WHERE id = ?1",
            [automation_id],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, bool>(1)?,
                    row.get::<_, u8>(2)?,
                ))
            },
        )
        .optional()
        .context("failed to load automation definition state")?;
    state
        .map(|(revision, tombstoned, authority_version)| {
            Ok(DefinitionState {
                revision: u64::try_from(revision)
                    .context("automation definition revision must be non-negative")?,
                tombstoned,
                authority_version,
            })
        })
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

    #[test]
    fn adoption_ledger_is_append_only() {
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
                "DELETE FROM automation_command_adoptions
                 WHERE adoption_key = 'adopt:create:immutable:0001'",
                [],
            )
            .is_err());
        assert_eq!(adoption_count(&conn), 1);
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
    fn exact_replay_returns_first_result_without_a_second_mutation() {
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
        assert_eq!(replay.outcome, DefinitionCommandOutcome::Replayed);
        assert_eq!(replay.result, first.result);
        assert_eq!(replay.revision, Some(1));
        assert_eq!(
            replay.replay_first_committed_at.as_deref(),
            Some("2026-09-03T09:00:00.000Z")
        );
        assert_eq!(list_definitions(&conn).unwrap().len(), 1);
        assert_eq!(adoption_count(&conn), 1);
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
        assert!(response
            .error
            .unwrap()
            .message
            .as_str()
            .contains("valid IANA timezone"));
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
