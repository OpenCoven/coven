use serde::Serialize;
use serde_json::{json, Value};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityCatalog {
    pub capabilities: Vec<Capability>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Capability {
    pub id: &'static str,
    pub label: &'static str,
    pub adapter: &'static str,
    pub status: CapabilityStatus,
    pub policy: CapabilityPolicy,
    pub actions: Vec<&'static str>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum CapabilityStatus {
    Available,
    Planned,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum CapabilityPolicy {
    Allow,
    RequiresApproval,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ControlActionResponse {
    pub ok: bool,
    pub accepted: bool,
    pub action: String,
    pub status: ActionStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event: Option<ControlEvent>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ActionStatus {
    Completed,
    Rejected,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ControlEvent {
    pub kind: &'static str,
    pub action: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub intent_id: Option<String>,
    pub payload: Value,
}

pub fn capabilities() -> CapabilityCatalog {
    CapabilityCatalog {
        capabilities: vec![
            Capability {
                id: "coven.sessions",
                label: "Project-scoped harness sessions",
                adapter: "coven-daemon",
                status: CapabilityStatus::Available,
                policy: CapabilityPolicy::Allow,
                actions: vec![],
            },
            Capability {
                id: "coven.travel",
                label: "Travel profiles and offline delta reconciliation",
                adapter: "coven-daemon",
                status: CapabilityStatus::Available,
                policy: CapabilityPolicy::Allow,
                actions: vec![],
            },
            Capability {
                id: "coven.scheduler",
                label: "Multi-host scheduler decisions and recovery",
                adapter: "coven-daemon",
                status: CapabilityStatus::Available,
                policy: CapabilityPolicy::Allow,
                actions: vec![],
            },
            Capability {
                id: "coven.control.actions",
                label: "Coven control-plane action router",
                adapter: "coven-daemon",
                status: CapabilityStatus::Available,
                policy: CapabilityPolicy::Allow,
                actions: vec!["coven.capabilities.refresh"],
            },
            Capability {
                id: "coven.automations",
                label: "Coven-native routine automations",
                adapter: "coven-daemon",
                status: CapabilityStatus::Available,
                policy: CapabilityPolicy::Allow,
                actions: vec![
                    "coven.automations.list",
                    "coven.automations.get",
                    "coven.automations.create",
                    "coven.automations.update",
                    "coven.automations.delete",
                    "coven.automations.definition.list.v1",
                    "coven.automations.definition.get.v1",
                    "coven.automations.definition.create.v1",
                    "coven.automations.definition.revise.v1",
                    "coven.automations.definition.tombstone.v1",
                    "coven.automations.events.read.v1",
                    "coven.automations.events.subscribe.v1",
                    "coven.automations.tick",
                    "coven.automations.runs",
                    "coven.automations.run",
                    "coven.automations.import",
                    "coven.automations.health",
                    "coven.automations.unquarantine",
                ],
            },
            Capability {
                id: "desktop.automation",
                label: "Desktop automation adapters",
                adapter: "desktop-use",
                status: CapabilityStatus::Planned,
                policy: CapabilityPolicy::RequiresApproval,
                actions: vec![],
            },
        ],
    }
}

pub fn route_action(
    payload: Value,
    conn: &rusqlite::Connection,
    runtime: &dyn crate::api::SessionRuntime,
) -> (u16, ControlActionResponse) {
    if !payload.is_object() {
        return (
            400,
            rejected_action("(unknown)", "request body must be a JSON object"),
        );
    }

    let Some(action) = payload
        .get("action")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|action| !action.is_empty())
    else {
        return (
            400,
            rejected_action("", "request body requires string field `action`"),
        );
    };

    let origin = payload
        .get("origin")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|origin| !origin.is_empty())
        .map(ToOwned::to_owned);
    let intent_id = payload
        .get("intentId")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|intent_id| !intent_id.is_empty())
        .map(ToOwned::to_owned);

    match action {
        "coven.capabilities.refresh" => {
            let event = ControlEvent {
                kind: "capabilities.refreshed",
                action: action.to_string(),
                origin: origin.clone(),
                intent_id: intent_id.clone(),
                payload: json!({
                    "capabilities": capabilities().capabilities.len(),
                }),
            };
            (
                200,
                ControlActionResponse {
                    ok: true,
                    accepted: true,
                    action: action.to_string(),
                    status: ActionStatus::Completed,
                    reason: None,
                    error: None,
                    result: None,
                    event: Some(event),
                },
            )
        }
        "coven.automations.list" => automation_result(
            action,
            origin,
            intent_id,
            automation_list_legacy_payload(conn),
        ),
        "coven.automations.get" => {
            let id = required_id_field(&payload, action);
            match id {
                Ok(id) => automation_result(
                    action,
                    origin,
                    intent_id,
                    automation_get_legacy_payload(conn, &id),
                ),
                Err(error) => (400, rejected_action(action, error)),
            }
        }
        "coven.automations.create" => {
            let definition = required_definition_field(&payload, action);
            match definition {
                Ok(definition) => automation_legacy_command_result(
                    action,
                    origin,
                    intent_id.clone(),
                    crate::automations::command_adoption::execute_definition_command(
                        conn,
                        &legacy_adoption_key(action, intent_id.as_deref()),
                        crate::automations::command_adoption::DefinitionCommand::LegacyCreate {
                            definition,
                        },
                        &now_iso(),
                    ),
                ),
                Err(error) => (400, rejected_action(action, error)),
            }
        }
        "coven.automations.update" => {
            let definition = required_definition_field(&payload, action);
            match definition {
                Ok(definition) => automation_legacy_command_result(
                    action,
                    origin,
                    intent_id.clone(),
                    crate::automations::command_adoption::execute_definition_command(
                        conn,
                        &legacy_adoption_key(action, intent_id.as_deref()),
                        crate::automations::command_adoption::DefinitionCommand::LegacyRevise {
                            definition,
                        },
                        &now_iso(),
                    ),
                ),
                Err(error) => (400, rejected_action(action, error)),
            }
        }
        "coven.automations.delete" => {
            let id = required_id_field(&payload, action);
            match id {
                Ok(id) => automation_legacy_command_result(
                    action,
                    origin,
                    intent_id.clone(),
                    crate::automations::command_adoption::execute_definition_command(
                        conn,
                        &legacy_adoption_key(action, intent_id.as_deref()),
                        crate::automations::command_adoption::DefinitionCommand::LegacyDelete {
                            automation_id: id,
                        },
                        &now_iso(),
                    ),
                ),
                Err(error) => (400, rejected_action(action, error)),
            }
        }
        "coven.automations.definition.list.v1" => {
            let include_tombstoned = payload
                .get("includeTombstoned")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            automation_result(
                action,
                origin,
                intent_id,
                automation_list_payload(conn, include_tombstoned),
            )
        }
        "coven.automations.definition.get.v1" => {
            let id = required_id_field(&payload, action);
            match id {
                Ok(id) => {
                    automation_result(action, origin, intent_id, automation_get_payload(conn, &id))
                }
                Err(error) => (400, rejected_action(action, error)),
            }
        }
        "coven.automations.definition.create.v1" => {
            let definition = required_definition_field(&payload, action);
            let adoption_key = required_adoption_key(&payload, action);
            let expected_revision = forbidden_expected_revision(&payload, action);
            match adoption_key {
                Ok(adoption_key) => {
                    let command = match (definition, expected_revision) {
                        (Ok(definition), Ok(())) => {
                            crate::automations::command_adoption::DefinitionCommand::Create {
                                definition,
                            }
                        }
                        (Err(error), _) | (_, Err(error)) => {
                            crate::automations::command_adoption::DefinitionCommand::Invalid {
                                command: "definition.create.v1".to_owned(),
                                request: command_request_fields(
                                    &payload,
                                    &["definition", "expectedRevision"],
                                ),
                                message: error,
                            }
                        }
                    };
                    automation_command_result(
                        action,
                        origin,
                        intent_id,
                        crate::automations::command_adoption::execute_definition_command(
                            conn,
                            &adoption_key,
                            command,
                            &now_iso(),
                        ),
                    )
                }
                Err(error) => validation_rejection(action, error),
            }
        }
        "coven.automations.definition.revise.v1" => {
            let definition = required_definition_field(&payload, action);
            let adoption_key = required_adoption_key(&payload, action);
            let expected_revision = required_expected_revision(&payload, action);
            match adoption_key {
                Ok(adoption_key) => {
                    let command = match (definition, expected_revision) {
                        (Ok(definition), Ok(expected_revision)) => {
                            crate::automations::command_adoption::DefinitionCommand::Revise {
                                definition,
                                expected_revision: Some(expected_revision),
                            }
                        }
                        (Err(error), _) | (_, Err(error)) => {
                            crate::automations::command_adoption::DefinitionCommand::Invalid {
                                command: "definition.revise.v1".to_owned(),
                                request: command_request_fields(
                                    &payload,
                                    &["definition", "expectedRevision"],
                                ),
                                message: error,
                            }
                        }
                    };
                    automation_command_result(
                        action,
                        origin,
                        intent_id,
                        crate::automations::command_adoption::execute_definition_command(
                            conn,
                            &adoption_key,
                            command,
                            &now_iso(),
                        ),
                    )
                }
                Err(error) => validation_rejection(action, error),
            }
        }
        "coven.automations.definition.tombstone.v1" => {
            let id = required_id_field(&payload, action);
            let adoption_key = required_adoption_key(&payload, action);
            let expected_revision = required_expected_revision(&payload, action);
            match adoption_key {
                Ok(adoption_key) => {
                    let command = match (id, expected_revision) {
                        (Ok(id), Ok(expected_revision)) => {
                            crate::automations::command_adoption::DefinitionCommand::Delete {
                                automation_id: id,
                                expected_revision: Some(expected_revision),
                            }
                        }
                        (Err(error), _) | (_, Err(error)) => {
                            crate::automations::command_adoption::DefinitionCommand::Invalid {
                                command: "definition.tombstone.v1".to_owned(),
                                request: command_request_fields(
                                    &payload,
                                    &["id", "expectedRevision"],
                                ),
                                message: error,
                            }
                        }
                    };
                    automation_command_result(
                        action,
                        origin,
                        intent_id,
                        crate::automations::command_adoption::execute_definition_command(
                            conn,
                            &adoption_key,
                            command,
                            &now_iso(),
                        ),
                    )
                }
                Err(error) => validation_rejection(action, error),
            }
        }
        "coven.automations.events.read.v1" => {
            let stream = required_event_stream(&payload, action);
            let after = optional_event_after(&payload, action);
            let from = optional_event_from(&payload, action);
            let limit = optional_event_limit(&payload, action);
            match (stream, after, from, limit) {
                (Ok((kind, id)), Ok(after), Ok(from), Ok(limit)) => automation_event_store_result(
                    action,
                    origin,
                    intent_id,
                    crate::automations::contract::events::read_events(
                        conn,
                        &kind,
                        &id,
                        after,
                        from.as_deref(),
                        limit,
                        &now_iso(),
                    ),
                ),
                (Err(error), _, _, _)
                | (_, Err(error), _, _)
                | (_, _, Err(error), _)
                | (_, _, _, Err(error)) => validation_rejection(action, error),
            }
        }
        "coven.automations.events.subscribe.v1" => {
            let stream = required_event_stream(&payload, action);
            let after = optional_event_after(&payload, action);
            let checkpoint = optional_event_checkpoint(&payload, action);
            let limit = forbidden_event_limit(&payload, action);
            match (stream, after, checkpoint, limit) {
                (Ok((kind, id)), Ok(after), Ok(checkpoint), Ok(())) => {
                    let result = if let Some(checkpoint) = checkpoint {
                        if after.is_some() {
                            Err(
                                crate::automations::contract::events::EventStoreError::InvalidRead(
                                    format!("{action} accepts `after` or `checkpoint`, not both"),
                                ),
                            )
                        } else {
                            crate::automations::contract::events::resume_events(
                                conn,
                                &checkpoint,
                                &kind,
                                &id,
                                100,
                                &now_iso(),
                            )
                        }
                    } else {
                        crate::automations::contract::events::read_events(
                            conn,
                            &kind,
                            &id,
                            after,
                            None,
                            100,
                            &now_iso(),
                        )
                    };
                    automation_event_store_result(action, origin, intent_id, result)
                }
                (Err(error), _, _, _)
                | (_, Err(error), _, _)
                | (_, _, Err(error), _)
                | (_, _, _, Err(error)) => validation_rejection(action, error),
            }
        }
        "coven.automations.tick" => {
            let now = chrono::Utc::now();
            automation_result(
                action,
                origin,
                intent_id,
                automation_tick_payload(conn, now),
            )
        }
        "coven.automations.runs" => {
            let id = required_id_field(&payload, action);
            let limit = payload.get("limit").and_then(Value::as_i64).unwrap_or(20);
            match id {
                Ok(id) => automation_result(
                    action,
                    origin,
                    intent_id,
                    automation_runs_payload(conn, &id, limit),
                ),
                Err(error) => (400, rejected_action(action, error)),
            }
        }
        "coven.automations.health" => {
            let id = required_id_field(&payload, action);
            let now = chrono::Utc::now();
            match id {
                Ok(id) => automation_result(
                    action,
                    origin,
                    intent_id,
                    automation_health_payload(conn, &id, now),
                ),
                Err(error) => (400, rejected_action(action, error)),
            }
        }
        "coven.automations.unquarantine" => {
            let id = required_id_field(&payload, action);
            let now = chrono::Utc::now();
            match id {
                Ok(id) => automation_result(
                    action,
                    origin,
                    intent_id,
                    automation_unquarantine_payload(conn, &id, now),
                ),
                Err(error) => (400, rejected_action(action, error)),
            }
        }
        "coven.automations.import" => {
            automation_result(action, origin, intent_id, automation_import_payload(conn))
        }
        "coven.automations.run" => {
            let id = required_id_field(&payload, action);
            let now = chrono::Utc::now();
            match id {
                Ok(id) => automation_result(
                    action,
                    origin,
                    intent_id,
                    automation_run_payload(conn, runtime, &id, now),
                ),
                Err(error) => (400, rejected_action(action, error)),
            }
        }
        _ => (
            400,
            rejected_action(action, format!("unknown action `{action}`")),
        ),
    }
}

fn automation_event(
    action: &str,
    origin: Option<String>,
    intent_id: Option<String>,
    payload: Value,
) -> ControlActionResponse {
    ControlActionResponse {
        ok: true,
        accepted: true,
        action: action.to_string(),
        status: ActionStatus::Completed,
        reason: None,
        error: None,
        result: None,
        event: Some(ControlEvent {
            kind: "automations.changed",
            action: action.to_string(),
            origin,
            intent_id,
            payload,
        }),
    }
}

fn automation_result(
    action: &str,
    origin: Option<String>,
    intent_id: Option<String>,
    result: Result<Value, String>,
) -> (u16, ControlActionResponse) {
    match result {
        Ok(payload) => (200, automation_event(action, origin, intent_id, payload)),
        Err(reason) => (400, rejected_action(action, reason)),
    }
}

fn automation_legacy_command_result(
    action: &str,
    origin: Option<String>,
    intent_id: Option<String>,
    result: anyhow::Result<crate::automations::command_adoption::DefinitionCommandResponse>,
) -> (u16, ControlActionResponse) {
    match result {
        Ok(response)
            if matches!(
                response.outcome,
                crate::automations::command_adoption::DefinitionCommandOutcome::Committed
                    | crate::automations::command_adoption::DefinitionCommandOutcome::Replayed
            ) =>
        {
            (
                200,
                automation_event(
                    action,
                    origin,
                    intent_id,
                    response.result.unwrap_or_else(|| json!({})),
                ),
            )
        }
        Ok(response) => {
            let reason = response
                .error
                .map(|error| error.message.as_str().to_owned())
                .unwrap_or_else(|| "automation command was rejected".to_owned());
            (400, rejected_action(action, reason))
        }
        Err(error) => (400, rejected_action(action, format!("{error:#}"))),
    }
}

fn automation_command_result(
    action: &str,
    origin: Option<String>,
    intent_id: Option<String>,
    result: anyhow::Result<crate::automations::command_adoption::DefinitionCommandResponse>,
) -> (u16, ControlActionResponse) {
    use crate::automations::command_adoption::DefinitionCommandOutcome;
    use crate::automations::contract::error::ErrorCode;

    let response = match result {
        Ok(response) => response,
        Err(error) => {
            let error = automation_error(ErrorCode::Internal, format!("{error:#}"));
            return typed_rejection(action, error);
        }
    };
    match response.outcome {
        DefinitionCommandOutcome::Committed | DefinitionCommandOutcome::Replayed => {
            let outcome = match response.outcome {
                DefinitionCommandOutcome::Committed => "committed",
                DefinitionCommandOutcome::Replayed => "replayed",
                DefinitionCommandOutcome::Rejected => unreachable!(),
            };
            let kind = if response.outcome == DefinitionCommandOutcome::Committed {
                "automations.changed"
            } else {
                "automations.replayed"
            };
            let mut payload = json!({
                "outcome": outcome,
                "revision": response.revision,
                "result": response.result,
            });
            if let Some(event_ref) = response.event_ref {
                payload["eventRef"] =
                    serde_json::to_value(event_ref).expect("automation event reference serializes");
            }
            if let Some(first_committed_at) = response.replay_first_committed_at {
                payload["replay"] = json!({
                    "firstCommittedAt": first_committed_at,
                });
            }
            let event = if response.outcome == DefinitionCommandOutcome::Committed {
                Some(ControlEvent {
                    kind,
                    action: action.to_string(),
                    origin,
                    intent_id,
                    payload: payload.clone(),
                })
            } else {
                None
            };
            (
                200,
                ControlActionResponse {
                    ok: true,
                    accepted: true,
                    action: action.to_string(),
                    status: ActionStatus::Completed,
                    reason: None,
                    error: None,
                    result: Some(payload),
                    event,
                },
            )
        }
        DefinitionCommandOutcome::Rejected => typed_rejection(
            action,
            response
                .error
                .expect("rejected automation command carries typed error"),
        ),
    }
}

fn automation_event_store_result(
    action: &str,
    _origin: Option<String>,
    _intent_id: Option<String>,
    result: Result<
        crate::automations::contract::events::EventPage,
        crate::automations::contract::events::EventStoreError,
    >,
) -> (u16, ControlActionResponse) {
    use crate::automations::contract::error::ErrorCode;
    match result {
        Ok(page) => match serde_json::to_value(page) {
            Ok(page) => (
                200,
                ControlActionResponse {
                    ok: true,
                    accepted: true,
                    action: action.to_owned(),
                    status: ActionStatus::Completed,
                    reason: None,
                    error: None,
                    result: Some(page),
                    event: None,
                },
            ),
            Err(error) => typed_rejection(
                action,
                automation_error(ErrorCode::Internal, format!("{error:#}")),
            ),
        },
        Err(error) => {
            let code = match error.code() {
                "CURSOR_EXPIRED" => ErrorCode::CursorExpired,
                "STREAM_OUT_OF_ORDER" => ErrorCode::StreamOutOfOrder,
                "CHECKPOINT_NOT_FOUND" => ErrorCode::NotFound,
                "VALIDATION_FAILED" => ErrorCode::ValidationFailed,
                "DUPLICATE_EVENT_ID" | "INTERNAL" => ErrorCode::Internal,
                _ => ErrorCode::Internal,
            };
            let mut envelope = automation_error(code, error.to_string());
            if let Some(expired_at) = error.expired_at() {
                envelope.details = Some(
                    [("expiredAt".to_owned(), json!(expired_at))]
                        .into_iter()
                        .collect(),
                );
            }
            typed_rejection(action, envelope)
        }
    }
}

fn validation_rejection(action: &str, reason: String) -> (u16, ControlActionResponse) {
    let error = automation_error(
        crate::automations::contract::error::ErrorCode::ValidationFailed,
        reason,
    );
    typed_rejection(action, error)
}

fn automation_error(
    code: crate::automations::contract::error::ErrorCode,
    message: impl Into<String>,
) -> crate::automations::contract::error::ErrorEnvelope {
    let message = message.into();
    let bounded = if message.is_empty() {
        "automation command failed".to_owned()
    } else {
        message.chars().take(1_000).collect()
    };
    crate::automations::contract::error::ErrorEnvelope::try_new(code, bounded, false)
        .expect("bounded non-empty automation error message is valid")
}

fn typed_rejection(
    action: &str,
    error: crate::automations::contract::error::ErrorEnvelope,
) -> (u16, ControlActionResponse) {
    let status = error.http_status();
    let reason = error.message.as_str().to_owned();
    let error = serde_json::to_value(error).expect("typed automation error serializes");
    (
        status,
        ControlActionResponse {
            ok: false,
            accepted: false,
            action: action.to_owned(),
            status: ActionStatus::Rejected,
            reason: Some(reason),
            error: Some(error),
            result: None,
            event: None,
        },
    )
}

fn legacy_adoption_key(action: &str, intent_id: Option<&str>) -> String {
    match intent_id {
        Some(intent_id) => {
            let digest = crate::automations::contract::sha256_hex(
                format!("{action}\0{intent_id}").as_bytes(),
            );
            format!("legacy:{digest}")
        }
        None => format!("legacy:{}", uuid::Uuid::new_v4().simple()),
    }
}

fn required_id_field(payload: &Value, action: &str) -> Result<String, String> {
    payload
        .get("id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(ToOwned::to_owned)
        .ok_or_else(|| format!("{action} requires string field `id`"))
}

fn required_definition_field(payload: &Value, action: &str) -> Result<Value, String> {
    let Some(definition) = payload.get("definition") else {
        return Err(format!("{action} requires object field `definition`"));
    };
    if !definition.is_object() {
        return Err(format!("{action} requires object field `definition`"));
    }
    Ok(definition.clone())
}

fn required_adoption_key(payload: &Value, action: &str) -> Result<String, String> {
    payload
        .get("adoptionKey")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|key| !key.is_empty())
        .map(ToOwned::to_owned)
        .ok_or_else(|| format!("{action} requires string field `adoptionKey`"))
}

fn required_expected_revision(payload: &Value, action: &str) -> Result<u64, String> {
    payload
        .get("expectedRevision")
        .and_then(Value::as_u64)
        .filter(|revision| (1..=9_007_199_254_740_991).contains(revision))
        .ok_or_else(|| format!("{action} requires positive safe-integer field `expectedRevision`"))
}

fn forbidden_expected_revision(payload: &Value, action: &str) -> Result<(), String> {
    if payload.get("expectedRevision").is_some() {
        Err(format!("{action} forbids field `expectedRevision`"))
    } else {
        Ok(())
    }
}

fn required_event_stream(payload: &Value, action: &str) -> Result<(String, String), String> {
    let stream = payload
        .get("stream")
        .cloned()
        .ok_or_else(|| format!("{action} requires object field `stream`"))?;
    let stream: crate::automations::contract::types::CommandStreamRef =
        serde_json::from_value(stream)
            .map_err(|error| format!("{action} has invalid stream: {error}"))?;
    let kind = match stream.kind {
        crate::automations::contract::types::StreamKind::Automation => "automation",
        crate::automations::contract::types::StreamKind::Occurrence => "occurrence",
        crate::automations::contract::types::StreamKind::Run => "run",
        crate::automations::contract::types::StreamKind::Feed => "feed",
    };
    Ok((kind.to_owned(), stream.id.as_str().to_owned()))
}

fn optional_event_after(payload: &Value, action: &str) -> Result<Option<u64>, String> {
    match payload.get("after") {
        None => Ok(None),
        Some(value) => value
            .as_u64()
            .filter(|value| *value <= 9_007_199_254_740_991)
            .map(Some)
            .ok_or_else(|| format!("{action} field `after` must be a non-negative safe integer")),
    }
}

fn optional_event_from(payload: &Value, action: &str) -> Result<Option<String>, String> {
    match payload.get("from") {
        None => Ok(None),
        Some(value) => {
            serde_json::from_value::<crate::automations::contract::types::Timestamp>(value.clone())
                .map_err(|error| format!("{action} has invalid `from` timestamp: {error}"))
                .and_then(|timestamp| {
                    chrono::DateTime::parse_from_rfc3339(timestamp.as_str())
                        .map(|_| Some(timestamp.as_str().to_owned()))
                        .map_err(|error| format!("{action} has invalid `from` timestamp: {error}"))
                })
        }
    }
}

fn optional_event_checkpoint(payload: &Value, action: &str) -> Result<Option<String>, String> {
    match payload.get("checkpoint") {
        None => Ok(None),
        Some(Value::String(value)) if !value.is_empty() && value.len() <= 512 => {
            Ok(Some(value.clone()))
        }
        Some(_) => Err(format!(
            "{action} field `checkpoint` must be a non-empty string of at most 512 bytes"
        )),
    }
}

fn optional_event_limit(payload: &Value, action: &str) -> Result<usize, String> {
    match payload.get("limit") {
        None => Ok(100),
        Some(value) => value
            .as_u64()
            .filter(|value| (1..=1_000).contains(value))
            .and_then(|value| usize::try_from(value).ok())
            .ok_or_else(|| format!("{action} field `limit` must be an integer from 1 to 1000")),
    }
}

fn forbidden_event_limit(payload: &Value, action: &str) -> Result<(), String> {
    if payload.get("limit").is_some() {
        Err(format!("{action} forbids field `limit`"))
    } else {
        Ok(())
    }
}

fn command_request_fields(payload: &Value, fields: &[&str]) -> Value {
    Value::Object(
        fields
            .iter()
            .filter_map(|field| {
                payload
                    .get(*field)
                    .map(|value| ((*field).to_owned(), value.clone()))
            })
            .collect(),
    )
}

fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

fn automation_tick_payload(
    conn: &rusqlite::Connection,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<Value, String> {
    match crate::automations::occurrences::tick(conn, now) {
        Ok(report) => Ok(json!({
            "planned": report.planned,
            "alreadyFenced": report.already_fenced,
            "pausedSkipped": report.paused_skipped,
            "recovered": report.recovered,
            "claimed": report.claimed,
            "failed": report.failed,
        })),
        Err(error) => Err(format!("{error:#}")),
    }
}

fn automation_health_payload(
    conn: &rusqlite::Connection,
    id: &str,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<Value, String> {
    match crate::automations::health::routine_health(conn, id, now) {
        Ok(health) => Ok(json!({
            "health": {
                "automationId": health.automation_id,
                "nextDueAt": health.next_due_at,
                "lastPlannedAt": health.last_planned_at,
                "lastStartedAt": health.last_started_at,
                "lastSuccessAt": health.last_success_at,
                "consecutiveFailures": health.consecutive_failures,
                "leaseOwner": health.lease_owner,
                "leaseExpiresAt": health.lease_expires_at,
                "staleReason": health.stale_reason,
                "currentAttempt": health.current_attempt,
                "maxAttempts": health.max_attempts,
                "retryNotBefore": health.retry_not_before,
                "consecutiveExhaustions": health.consecutive_exhaustions,
                "quarantinedAt": health.quarantined_at,
                "quarantineFailureClass": health.quarantine_failure_class,
                "quarantineReason": health.quarantine_reason,
            }
        })),
        Err(error) => Err(format!("{error:#}")),
    }
}

fn automation_unquarantine_payload(
    conn: &rusqlite::Connection,
    id: &str,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<Value, String> {
    if crate::automations::store::get_definition(conn, id)
        .map_err(|error| format!("{error:#}"))?
        .is_none()
    {
        return Err(format!("no routine with id `{id}`"));
    }
    crate::automations::runs::clear_retry_quarantine(conn, id, now)
        .map(|released| json!({ "automationId": id, "released": released }))
        .map_err(|error| format!("{error:#}"))
}

fn automation_import_payload(conn: &rusqlite::Connection) -> Result<Value, String> {
    match crate::automations::import_legacy::import_legacy_codex_automations(conn) {
        Ok(report) => Ok(json!({
            "imported": report.imported,
            "skipped": report.skipped,
            "failures": report.failures,
        })),
        Err(error) => Err(format!("{error:#}")),
    }
}

fn automation_run_payload(
    conn: &rusqlite::Connection,
    runtime: &dyn crate::api::SessionRuntime,
    id: &str,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<Value, String> {
    match crate::automations::runner::load_definition_for_run(conn, id) {
        Ok(Some(definition)) => {
            match crate::automations::runner::run_routine_now(conn, runtime, &definition, now) {
                Ok(outcome) if matches!(outcome.status.as_str(), "running" | "retry_scheduled") => {
                    Ok(json!({
                    "runId": outcome.run_id,
                    "status": outcome.status,
                    "sessionId": outcome.session_id,
                    "error": outcome.error,
                    }))
                }
                Ok(outcome) => Err(outcome
                    .error
                    .unwrap_or_else(|| "routine run did not enter running state".to_string())),
                Err(error) => Err(error),
            }
        }
        Ok(None) => Err(format!("no routine with id `{id}`")),
        Err(error) => Err(error),
    }
}

fn automation_runs_payload(
    conn: &rusqlite::Connection,
    id: &str,
    limit: i64,
) -> Result<Value, String> {
    match crate::automations::runs::list_runs(conn, id, limit) {
        Ok(records) => {
            let mut attempts_by_run = std::collections::HashMap::new();
            for attempt in crate::automations::runs::list_attempts_for_automation(conn, id, limit)
                .map_err(|error| format!("{error:#}"))?
            {
                attempts_by_run
                    .entry(attempt.run_id.clone())
                    .or_insert_with(Vec::new)
                    .push(attempt);
            }
            let mut runs = Vec::with_capacity(records.len());
            for record in &records {
                let attempts = attempts_by_run
                    .remove(&record.id)
                    .unwrap_or_default()
                    .into_iter()
                    .map(|attempt| {
                        json!({
                            "id": attempt.id,
                            "runId": attempt.run_id,
                            "occurrenceId": attempt.occurrence_id,
                            "attemptNumber": attempt.attempt_number,
                            "adoptionKey": attempt.adoption_key,
                            "occurrenceFenceGeneration": attempt.occurrence_fence_generation,
                            "dispatchGeneration": attempt.dispatch_generation,
                            "state": attempt.state,
                            "failureClass": attempt.failure_class,
                            "priorAttemptNumber": attempt.prior_attempt_number,
                            "priorDisposition": attempt.prior_disposition,
                            "retryClassification": attempt.retry_classification,
                            "notBefore": attempt.not_before,
                            "sessionId": attempt.session_id,
                            "stateReason": attempt.state_reason,
                            "openedAt": attempt.opened_at,
                            "settledAt": attempt.settled_at,
                        })
                    })
                    .collect::<Vec<_>>();
                runs.push(json!({
                    "id": record.id,
                    "automationId": record.automation_id,
                    "occurrenceId": record.occurrence_id,
                    "sessionId": record.session_id,
                    "familiarId": record.familiar_id,
                    "runtime": record.runtime,
                    "status": record.status,
                    "exitCode": record.exit_code,
                    "logJson": record.log_json,
                    "outputCommit": record.output_commit,
                    "startedAt": record.started_at,
                    "finishedAt": record.finished_at,
                    "attempts": attempts,
                }));
            }
            Ok(json!({ "runs": runs }))
        }
        Err(error) => Err(format!("{error:#}")),
    }
}

fn automation_list_legacy_payload(conn: &rusqlite::Connection) -> Result<Value, String> {
    let records = crate::automations::store::list_definitions(conn);
    match records {
        Ok(records) => {
            let mut routines = Vec::with_capacity(records.len());
            for record in records {
                let routine =
                    serde_json::from_str::<Value>(&record.definition_json).map_err(|error| {
                        format!("stored routine `{}` is unreadable: {error}", record.id)
                    })?;
                routines.push(routine);
            }
            Ok(json!({ "routines": routines }))
        }
        Err(error) => Err(format!("{error:#}")),
    }
}

fn automation_get_legacy_payload(conn: &rusqlite::Connection, id: &str) -> Result<Value, String> {
    match crate::automations::store::get_definition(conn, id) {
        Ok(Some(record)) => match serde_json::from_str::<Value>(&record.definition_json) {
            Ok(routine) => Ok(json!({ "routine": routine })),
            Err(error) => Err(format!("stored routine is unreadable: {error}")),
        },
        Ok(None) => Ok(json!({ "routine": Value::Null })),
        Err(error) => Err(format!("{error:#}")),
    }
}

fn automation_list_payload(
    conn: &rusqlite::Connection,
    include_tombstoned: bool,
) -> Result<Value, String> {
    let records =
        crate::automations::store::list_definitions_with_tombstones(conn, include_tombstoned);
    match records {
        Ok(records) => {
            let mut routines = Vec::with_capacity(records.len());
            let mut revision_by_id = std::collections::BTreeMap::new();
            let mut tombstoned_at_by_id = std::collections::BTreeMap::new();
            for record in records {
                let id = record.id.clone();
                let routine =
                    serde_json::from_str::<Value>(&record.definition_json).map_err(|error| {
                        format!("stored routine `{}` is unreadable: {error}", record.id)
                    })?;
                revision_by_id.insert(id.clone(), record.revision);
                if let Some(tombstoned_at) = record.tombstoned_at {
                    tombstoned_at_by_id.insert(id, tombstoned_at);
                }
                routines.push(routine);
            }
            Ok(json!({
                "routines": routines,
                "revisionById": revision_by_id,
                "tombstonedAtById": tombstoned_at_by_id,
            }))
        }
        Err(error) => Err(format!("{error:#}")),
    }
}

fn automation_get_payload(conn: &rusqlite::Connection, id: &str) -> Result<Value, String> {
    match crate::automations::store::get_definition_with_tombstone(conn, id, true) {
        Ok(Some(record)) => match serde_json::from_str::<Value>(&record.definition_json) {
            Ok(routine) => Ok(json!({
                "routine": routine,
                "revision": record.revision,
                "tombstonedAt": record.tombstoned_at,
            })),
            Err(error) => Err(format!("stored routine is unreadable: {error}")),
        },
        Ok(None) => Ok(json!({ "routine": Value::Null })),
        Err(error) => Err(format!("{error:#}")),
    }
}

pub fn rejected_action(
    action: impl Into<String>,
    reason: impl Into<String>,
) -> ControlActionResponse {
    ControlActionResponse {
        ok: false,
        accepted: false,
        action: action.into(),
        status: ActionStatus::Rejected,
        reason: Some(reason.into()),
        error: None,
        result: None,
        event: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::{SessionLaunch, SessionRuntime};

    struct OwnershipThenErrorRuntime;
    struct RejectedRuntime;
    struct RetryableRejectedRuntime;

    impl SessionRuntime for OwnershipThenErrorRuntime {
        fn launch_session(&self, _launch: &SessionLaunch) -> anyhow::Result<()> {
            unreachable!("automation dispatch uses strict adopted containment")
        }

        fn launch_contained_adopted_session(
            &self,
            _launch: &SessionLaunch,
            _writer: Option<crate::maintenance_gate::WriterLease>,
            ownership_established: &mut dyn FnMut() -> anyhow::Result<()>,
        ) -> anyhow::Result<()> {
            ownership_established()?;
            anyhow::bail!("synthetic acknowledgement failure")
        }

        fn send_input(
            &self,
            _session_id: &str,
            _payload: &serde_json::Value,
        ) -> anyhow::Result<()> {
            Ok(())
        }

        fn kill_session(&self, _session_id: &str) -> anyhow::Result<()> {
            Ok(())
        }
    }

    impl SessionRuntime for RejectedRuntime {
        fn launch_session(&self, _launch: &SessionLaunch) -> anyhow::Result<()> {
            unreachable!("automation dispatch uses strict adopted containment")
        }

        fn launch_contained_adopted_session(
            &self,
            _launch: &SessionLaunch,
            _writer: Option<crate::maintenance_gate::WriterLease>,
            _ownership_established: &mut dyn FnMut() -> anyhow::Result<()>,
        ) -> anyhow::Result<()> {
            anyhow::bail!("synthetic launch rejection")
        }

        fn send_input(
            &self,
            _session_id: &str,
            _payload: &serde_json::Value,
        ) -> anyhow::Result<()> {
            Ok(())
        }

        fn kill_session(&self, _session_id: &str) -> anyhow::Result<()> {
            Ok(())
        }
    }

    impl SessionRuntime for RetryableRejectedRuntime {
        fn launch_session(&self, _launch: &SessionLaunch) -> anyhow::Result<()> {
            unreachable!("automation dispatch uses strict adopted containment")
        }

        fn launch_contained_adopted_session(
            &self,
            _launch: &SessionLaunch,
            _writer: Option<crate::maintenance_gate::WriterLease>,
            _ownership_established: &mut dyn FnMut() -> anyhow::Result<()>,
        ) -> anyhow::Result<()> {
            Err(
                std::io::Error::new(std::io::ErrorKind::ConnectionRefused, "runtime unavailable")
                    .into(),
            )
        }

        fn send_input(
            &self,
            _session_id: &str,
            _payload: &serde_json::Value,
        ) -> anyhow::Result<()> {
            Ok(())
        }

        fn kill_session(&self, _session_id: &str) -> anyhow::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn live_run_with_diagnostic_remains_an_accepted_action() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("store.sqlite");
        crate::store::initialize_store(&path).unwrap();
        let conn = crate::store::open_store(&path).unwrap();
        let definition = crate::automations::RoutineDefinition::from_json(&json!({
            "schemaVersion": 1,
            "id": "daily",
            "name": "Daily",
            "status": "PAUSED",
            "rrule": "FREQ=DAILY;BYHOUR=9",
            "timezone": "utc",
            "misfire": "latest",
            "overlap": "forbid",
            "timeoutMinutes": 30,
            "runtime": "coven-code",
            "cwd": "/work/project",
            "prompt": "Do the thing."
        }))
        .unwrap();
        crate::automations::store::insert_definition(&conn, &definition).unwrap();

        let (status, response) = route_action(
            json!({"action": "coven.automations.run", "id": "daily"}),
            &conn,
            &OwnershipThenErrorRuntime,
        );

        assert_eq!(status, 200);
        assert!(response.ok);
        assert!(response.accepted);
        let payload = &response.event.as_ref().unwrap().payload;
        assert_eq!(payload["status"], "running");
        assert!(payload["runId"].as_str().is_some_and(|id| !id.is_empty()));
        assert!(payload["sessionId"]
            .as_str()
            .is_some_and(|id| !id.is_empty()));
        assert!(payload["error"]
            .as_str()
            .is_some_and(|error| error.contains("acknowledgement failed")));
    }

    #[test]
    fn automation_list_rejects_unreadable_stored_definitions() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("store.sqlite");
        crate::store::initialize_store(&path).unwrap();
        let conn = crate::store::open_store(&path).unwrap();
        conn.execute(
            "INSERT INTO automation_definitions
                (id, name, status, definition_json, created_at, updated_at)
             VALUES ('broken', 'Broken', 'ACTIVE', '{', ?1, ?1)",
            rusqlite::params!["2026-08-30T09:00:00.000Z"],
        )
        .unwrap();

        let (status, response) = route_action(
            json!({"action": "coven.automations.list"}),
            &conn,
            &crate::api::NoopSessionRuntime,
        );

        assert_eq!(status, 400);
        assert!(!response.ok);
        assert!(!response.accepted);
        assert!(response
            .reason
            .as_deref()
            .is_some_and(|reason| reason.contains("stored routine `broken` is unreadable")));
    }

    #[test]
    fn synchronous_manual_run_failure_is_rejected() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("store.sqlite");
        crate::store::initialize_store(&path).unwrap();
        let conn = crate::store::open_store(&path).unwrap();
        let definition = crate::automations::RoutineDefinition::from_json(&json!({
            "schemaVersion": 1,
            "id": "missing-cwd",
            "name": "Missing cwd",
            "status": "PAUSED",
            "rrule": "FREQ=DAILY;BYHOUR=9",
            "timezone": "utc",
            "misfire": "latest",
            "overlap": "forbid",
            "timeoutMinutes": 30,
            "runtime": "coven-code",
            "prompt": "Do the thing."
        }))
        .unwrap();
        crate::automations::store::insert_definition(&conn, &definition).unwrap();

        let (status, response) = route_action(
            json!({"action": "coven.automations.run", "id": "missing-cwd"}),
            &conn,
            &crate::api::NoopSessionRuntime,
        );

        assert_eq!(status, 400);
        assert!(!response.ok);
        assert!(!response.accepted);
        assert!(response
            .reason
            .as_deref()
            .is_some_and(|reason| reason.contains("routine has no cwd")));
    }

    #[test]
    fn overlapping_manual_run_is_rejected() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("store.sqlite");
        crate::store::initialize_store(&path).unwrap();
        let conn = crate::store::open_store(&path).unwrap();
        let definition = crate::automations::RoutineDefinition::from_json(&json!({
            "schemaVersion": 1,
            "id": "overlap",
            "name": "Overlap",
            "status": "PAUSED",
            "rrule": "FREQ=DAILY;BYHOUR=9",
            "timezone": "utc",
            "misfire": "latest",
            "overlap": "forbid",
            "timeoutMinutes": 30,
            "runtime": "coven-code",
            "cwd": "/work/project",
            "prompt": "Do the thing."
        }))
        .unwrap();
        crate::automations::store::insert_definition(&conn, &definition).unwrap();
        assert!(crate::automations::occurrences::insert_claimed_occurrence(
            &conn,
            "existing",
            &definition.id,
            "manual",
            60,
            chrono::Utc::now(),
        )
        .unwrap());

        let (status, response) = route_action(
            json!({"action": "coven.automations.run", "id": "overlap"}),
            &conn,
            &crate::api::NoopSessionRuntime,
        );

        assert_eq!(status, 400);
        assert!(!response.accepted);
        assert!(response
            .reason
            .as_deref()
            .is_some_and(|reason| reason.contains("overlap is forbidden")));
    }

    #[test]
    fn preownership_launch_rejection_is_rejected() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("store.sqlite");
        crate::store::initialize_store(&path).unwrap();
        let conn = crate::store::open_store(&path).unwrap();
        let definition = crate::automations::RoutineDefinition::from_json(&json!({
            "schemaVersion": 1,
            "id": "rejected",
            "name": "Rejected",
            "status": "PAUSED",
            "rrule": "FREQ=DAILY;BYHOUR=9",
            "timezone": "utc",
            "misfire": "latest",
            "overlap": "forbid",
            "timeoutMinutes": 30,
            "runtime": "coven-code",
            "cwd": "/work/project",
            "prompt": "Do the thing."
        }))
        .unwrap();
        crate::automations::store::insert_definition(&conn, &definition).unwrap();

        let (status, response) = route_action(
            json!({"action": "coven.automations.run", "id": "rejected"}),
            &conn,
            &RejectedRuntime,
        );

        assert_eq!(status, 400);
        assert!(!response.accepted);
        assert!(response
            .reason
            .as_deref()
            .is_some_and(|reason| reason.contains("synthetic launch rejection")));
    }

    #[test]
    fn retry_scheduled_manual_run_remains_an_accepted_action() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("store.sqlite");
        crate::store::initialize_store(&path).unwrap();
        let conn = crate::store::open_store(&path).unwrap();
        let definition = crate::automations::RoutineDefinition::from_json(&json!({
            "schemaVersion": 1,
            "id": "retrying",
            "name": "Retrying",
            "status": "PAUSED",
            "rrule": "FREQ=DAILY;BYHOUR=9",
            "timezone": "utc",
            "misfire": "latest",
            "overlap": "forbid",
            "timeoutMinutes": 30,
            "retry": {
                "maxAttempts": 2,
                "backoffPolicy": "none",
                "retryableClasses": ["runtime_unavailable"]
            },
            "runtime": "coven-code",
            "cwd": "/work/project",
            "prompt": "Do the thing."
        }))
        .unwrap();
        crate::automations::store::insert_definition(&conn, &definition).unwrap();

        let (status, response) = route_action(
            json!({"action": "coven.automations.run", "id": "retrying"}),
            &conn,
            &RetryableRejectedRuntime,
        );

        assert_eq!(status, 200);
        assert!(response.ok);
        assert!(response.accepted);
        let payload = &response.event.as_ref().unwrap().payload;
        assert_eq!(payload["status"], "retry_scheduled");
        assert!(payload["runId"].as_str().is_some_and(|id| !id.is_empty()));
        assert!(payload["sessionId"].is_null());
        assert!(payload["error"]
            .as_str()
            .is_some_and(|error| error.contains("runtime unavailable")));
    }

    #[test]
    fn automation_health_exposes_quarantine_and_unquarantine_releases_it() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("store.sqlite");
        crate::store::initialize_store(&path).unwrap();
        let conn = crate::store::open_store(&path).unwrap();
        let definition = crate::automations::RoutineDefinition::from_json(&json!({
            "schemaVersion": 1,
            "id": "quarantined",
            "name": "Quarantined",
            "status": "ACTIVE",
            "rrule": "FREQ=DAILY;BYHOUR=9",
            "timezone": "utc",
            "misfire": "latest",
            "overlap": "forbid",
            "timeoutMinutes": 30,
            "retry": {
                "maxAttempts": 2,
                "backoffPolicy": "none",
                "retryableClasses": ["runtime_unavailable"]
            },
            "runtime": "coven-code",
            "cwd": "/work/project",
            "prompt": "Do the thing."
        }))
        .unwrap();
        crate::automations::store::insert_definition(&conn, &definition).unwrap();
        conn.execute(
            "INSERT INTO automation_retry_state
                (automation_id, consecutive_exhaustions, quarantined_at,
                 failure_class, reason, updated_at)
             VALUES ('quarantined', 1, '2026-08-27T09:05:00.000Z',
                     'runtime_unavailable', 'runtime stayed offline',
                     '2026-08-27T09:05:00.000Z')",
            [],
        )
        .unwrap();

        let (health_status, health_response) = route_action(
            json!({"action": "coven.automations.health", "id": "quarantined"}),
            &conn,
            &crate::api::NoopSessionRuntime,
        );
        assert_eq!(health_status, 200);
        let health = &health_response.event.as_ref().unwrap().payload["health"];
        assert_eq!(health["maxAttempts"], 2);
        assert_eq!(health["quarantineFailureClass"], "runtime_unavailable");
        assert_eq!(health["quarantineReason"], "runtime stayed offline");
        assert!(health["quarantinedAt"].is_string());

        let (release_status, release_response) = route_action(
            json!({"action": "coven.automations.unquarantine", "id": "quarantined"}),
            &conn,
            &crate::api::NoopSessionRuntime,
        );
        assert_eq!(release_status, 200);
        assert_eq!(
            release_response.event.as_ref().unwrap().payload["released"],
            true
        );
        assert!(!crate::automations::runs::is_retry_quarantined(&conn, "quarantined").unwrap());
        assert!(capabilities()
            .capabilities
            .iter()
            .find(|capability| capability.id == "coven.automations")
            .unwrap()
            .actions
            .contains(&"coven.automations.unquarantine"));
    }

    #[test]
    fn automation_events_read_and_subscribe_resume_after_exclusive_checkpoint() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("store.sqlite");
        crate::store::initialize_store(&path).unwrap();
        let conn = crate::store::open_store(&path).unwrap();
        let definition = json!({
            "schemaVersion": 1,
            "id": "event-control",
            "name": "Event control",
            "status": "PAUSED",
            "rrule": "FREQ=DAILY;BYHOUR=9",
            "timezone": "utc",
            "misfire": "latest",
            "overlap": "forbid",
            "timeoutMinutes": 30,
            "runtime": "coven-code",
            "prompt": "Do the thing."
        });
        let (create_status, _) = route_action(
            json!({
                "action": "coven.automations.definition.create.v1",
                "adoptionKey": "adopt:create:event-control:0001",
                "definition": definition,
            }),
            &conn,
            &crate::api::NoopSessionRuntime,
        );
        assert_eq!(create_status, 200);

        let (read_status, read_response) = route_action(
            json!({
                "action": "coven.automations.events.read.v1",
                "stream": {"kind": "automation", "id": "event-control"},
                "limit": 1,
            }),
            &conn,
            &crate::api::NoopSessionRuntime,
        );
        assert_eq!(read_status, 200);
        let read = read_response.result.unwrap();
        assert_eq!(read["events"].as_array().unwrap().len(), 1);
        assert_eq!(read["events"][0]["sequence"], 0);
        let checkpoint = read["checkpoint"].as_str().unwrap();

        let (subscribe_status, subscribe_response) = route_action(
            json!({
                "action": "coven.automations.events.subscribe.v1",
                "stream": {"kind": "automation", "id": "event-control"},
                "checkpoint": checkpoint,
            }),
            &conn,
            &crate::api::NoopSessionRuntime,
        );
        assert_eq!(subscribe_status, 200);
        assert!(subscribe_response.result.unwrap()["events"]
            .as_array()
            .unwrap()
            .is_empty());
    }

    #[test]
    fn automation_events_subscribe_surfaces_typed_expired_checkpoint() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("store.sqlite");
        crate::store::initialize_store(&path).unwrap();
        let conn = crate::store::open_store(&path).unwrap();
        conn.execute(
            "INSERT INTO automation_event_checkpoints (
                checkpoint, stream_kind, stream_id, after_sequence, issued_at, expires_at
             ) VALUES (
                'ecpexpired00000000000000000001',
                'automation',
                'expired',
                -1,
                '2020-01-01T00:00:00.000Z',
                '2020-01-02T00:00:00.000Z'
             )",
            [],
        )
        .unwrap();

        let (status, response) = route_action(
            json!({
                "action": "coven.automations.events.subscribe.v1",
                "stream": {"kind": "automation", "id": "expired"},
                "checkpoint": "ecpexpired00000000000000000001",
            }),
            &conn,
            &crate::api::NoopSessionRuntime,
        );

        assert_eq!(status, 410);
        assert_eq!(response.error.unwrap()["code"], "CURSOR_EXPIRED");
    }

    #[test]
    fn automation_events_subscribe_rejects_non_contract_limit() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("store.sqlite");
        crate::store::initialize_store(&path).unwrap();
        let conn = crate::store::open_store(&path).unwrap();

        let (status, response) = route_action(
            json!({
                "action": "coven.automations.events.subscribe.v1",
                "stream": {"kind": "automation", "id": "limited"},
                "limit": 10,
            }),
            &conn,
            &crate::api::NoopSessionRuntime,
        );

        assert_eq!(status, 400);
        assert_eq!(response.error.unwrap()["code"], "VALIDATION_FAILED");
    }

    #[test]
    fn cross_stream_checkpoint_rejection_does_not_create_a_checkpoint() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("store.sqlite");
        crate::store::initialize_store(&path).unwrap();
        let conn = crate::store::open_store(&path).unwrap();
        let first = crate::automations::contract::events::read_events(
            &conn,
            "automation",
            "source",
            None,
            None,
            100,
            &now_iso(),
        )
        .unwrap();
        let before = conn
            .query_row(
                "SELECT COUNT(*) FROM automation_event_checkpoints",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap();

        let (status, response) = route_action(
            json!({
                "action": "coven.automations.events.subscribe.v1",
                "stream": {"kind": "automation", "id": "different"},
                "checkpoint": first.checkpoint,
            }),
            &conn,
            &crate::api::NoopSessionRuntime,
        );

        assert_eq!(status, 400);
        assert_eq!(response.error.unwrap()["code"], "VALIDATION_FAILED");
        let after = conn
            .query_row(
                "SELECT COUNT(*) FROM automation_event_checkpoints",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap();
        assert_eq!(after, before);
    }

    #[test]
    fn automation_events_read_rejects_calendar_invalid_from_timestamp() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("store.sqlite");
        crate::store::initialize_store(&path).unwrap();
        let conn = crate::store::open_store(&path).unwrap();

        let (status, response) = route_action(
            json!({
                "action": "coven.automations.events.read.v1",
                "stream": {"kind": "automation", "id": "invalid-time"},
                "from": "2026-99-99T99:99:99.000Z",
            }),
            &conn,
            &crate::api::NoopSessionRuntime,
        );

        assert_eq!(status, 400);
        assert_eq!(response.error.unwrap()["code"], "VALIDATION_FAILED");
    }
}
