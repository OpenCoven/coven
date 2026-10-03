use crate::automations::owner_grants::CommandAuthority;
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub variant_negotiation:
        Option<&'static crate::automations::capability_negotiation::CapabilityProfile>,
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
                variant_negotiation: None,
            },
            Capability {
                id: "coven.travel",
                label: "Travel profiles and offline delta reconciliation",
                adapter: "coven-daemon",
                status: CapabilityStatus::Available,
                policy: CapabilityPolicy::Allow,
                actions: vec![],
                variant_negotiation: None,
            },
            Capability {
                id: "coven.scheduler",
                label: "Multi-host scheduler decisions and recovery",
                adapter: "coven-daemon",
                status: CapabilityStatus::Available,
                policy: CapabilityPolicy::Allow,
                actions: vec![],
                variant_negotiation: None,
            },
            Capability {
                id: "coven.control.actions",
                label: "Coven control-plane action router",
                adapter: "coven-daemon",
                status: CapabilityStatus::Available,
                policy: CapabilityPolicy::Allow,
                actions: vec!["coven.capabilities.refresh"],
                variant_negotiation: None,
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
                    "coven.automations.command.v1",
                    "coven.automations.definition.create.v1",
                    "coven.automations.definition.revise.v1",
                    "coven.automations.definition.disable.v1",
                    "coven.automations.definition.activate.v1",
                    "coven.automations.definition.pause.v1",
                    "coven.automations.definition.tombstone.v1",
                    "coven.automations.run.cancel.v1",
                    "coven.automations.events.read.v1",
                    "coven.automations.events.subscribe.v1",
                    "coven.automations.tick",
                    "coven.automations.runs",
                    "coven.automations.run.history.v1",
                    "coven.automations.receipt.get.v1",
                    "coven.automations.run",
                    "coven.automations.import",
                    "coven.automations.legacy.import.v1",
                    "coven.automations.health",
                    "coven.automations.definition.health.v1",
                    "coven.automations.scheduler.status.v1",
                    "coven.automations.occurrence.list.v1",
                    "coven.automations.occurrence.get.v1",
                    "coven.automations.run.get.v1",
                    "coven.automations.occurrence.history.v1",
                    "coven.automations.unquarantine",
                ],
                variant_negotiation: Some(
                    crate::automations::capability_negotiation::capability_profile(),
                ),
            },
            Capability {
                id: "desktop.automation",
                label: "Desktop automation adapters",
                adapter: "desktop-use",
                status: CapabilityStatus::Planned,
                policy: CapabilityPolicy::RequiresApproval,
                actions: vec![],
                variant_negotiation: None,
            },
        ],
    }
}

/// Automation reads loopback TCP may serve: scheduling and health diagnostics
/// that carry neither prompts nor run logs and write nothing.
const TCP_AUTOMATION_READS: &[&str] = &[
    "coven.automations.health",
    "coven.automations.definition.health.v1",
    "coven.automations.scheduler.status.v1",
    "coven.automations.occurrence.list.v1",
    "coven.automations.occurrence.history.v1",
];

/// Automation reads restricted to owner-local IPC: definitions carry prompts,
/// runs carry `logJson`, receipts carry authority evidence, and event pages
/// issue a stored checkpoint.
const OWNER_AUTOMATION_READS: &[&str] = &[
    "coven.automations.list",
    "coven.automations.get",
    "coven.automations.definition.list.v1",
    "coven.automations.definition.get.v1",
    "coven.automations.runs",
    "coven.automations.run.history.v1",
    "coven.automations.run.get.v1",
    "coven.automations.occurrence.get.v1",
    "coven.automations.receipt.get.v1",
    "coven.automations.events.read.v1",
    "coven.automations.events.subscribe.v1",
];

/// Check transport authority before opening the store, adoption lookup, or dispatch.
/// Deny by default: an automation action not listed as a TCP read requires
/// owner-local IPC, so a new command cannot silently become reachable over
/// unauthenticated loopback TCP.
pub(crate) fn automation_transport_rejection(
    payload: &Value,
    authority: crate::request_authority::RequestAuthority,
) -> Option<(u16, ControlActionResponse)> {
    let action = payload.get("action")?.as_str()?.trim();
    if authority.allows_owner_automation_access() {
        return None;
    }
    // The familiar ledger has no TCP reads: its revisions hold declarations.
    if action.starts_with(crate::familiar_ledger::ACTION_PREFIX) {
        return Some(typed_rejection(
            action,
            automation_error(
                crate::automations::contract::error::ErrorCode::AuthorityRequired,
                "Familiar ledger commands require owner-local IPC.",
            ),
        ));
    }
    if !action.starts_with("coven.automations.") || TCP_AUTOMATION_READS.contains(&action) {
        return None;
    }
    let reason = match action {
        "coven.automations.receipt.get.v1" => "Automation receipt reads require owner-local IPC.",
        "coven.automations.events.read.v1" | "coven.automations.events.subscribe.v1" => {
            "Automation event reads require owner-local IPC."
        }
        _ if OWNER_AUTOMATION_READS.contains(&action) => {
            "Automation reads that return prompts or run logs require owner-local IPC."
        }
        _ => "Automation mutations require owner-local IPC.",
    };
    Some(typed_rejection(
        action,
        automation_error(
            crate::automations::contract::error::ErrorCode::AuthorityRequired,
            reason,
        ),
    ))
}

/// Routes one control action as the owner, as the API's default entry does.
/// Test-only, so production callers always state the authority they hold.
#[cfg(test)]
pub fn route_action(
    payload: Value,
    conn: &rusqlite::Connection,
    runtime: &dyn crate::api::SessionRuntime,
) -> (u16, ControlActionResponse) {
    route_action_with_authority(payload, conn, runtime, CommandAuthority::OwnerLocal)
}

/// Routes one control action for a caller authenticated as `authority`.
pub(crate) fn route_action_with_authority(
    payload: Value,
    conn: &rusqlite::Connection,
    runtime: &dyn crate::api::SessionRuntime,
    authority: CommandAuthority,
) -> (u16, ControlActionResponse) {
    route_action_at(payload, conn, runtime, &now_iso(), authority)
}

pub(crate) fn route_action_at(
    payload: Value,
    conn: &rusqlite::Connection,
    runtime: &dyn crate::api::SessionRuntime,
    recorded_at: &str,
    authority: CommandAuthority,
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
                        recorded_at,
                        authority,
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
                        recorded_at,
                        authority,
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
                        recorded_at,
                        authority,
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
                            recorded_at,
                            authority,
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
                            recorded_at,
                            authority,
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
                            recorded_at,
                            authority,
                        ),
                    )
                }
                Err(error) => validation_rejection(action, error),
            }
        }
        "coven.automations.definition.disable.v1" => definition_status_command(
            conn,
            &payload,
            action,
            (origin, intent_id),
            (recorded_at, authority),
            "definition.disable.v1",
            |automation_id, expected_revision, reason| {
                crate::automations::command_adoption::DefinitionCommand::Disable {
                    automation_id,
                    expected_revision,
                    reason,
                }
            },
        ),
        "coven.automations.definition.activate.v1" => definition_status_command(
            conn,
            &payload,
            action,
            (origin, intent_id),
            (recorded_at, authority),
            "definition.activate.v1",
            |automation_id, expected_revision, reason| {
                crate::automations::command_adoption::DefinitionCommand::Activate {
                    automation_id,
                    expected_revision,
                    reason,
                }
            },
        ),
        "coven.automations.definition.pause.v1" => definition_status_command(
            conn,
            &payload,
            action,
            (origin, intent_id),
            (recorded_at, authority),
            "definition.pause.v1",
            |automation_id, expected_revision, reason| {
                crate::automations::command_adoption::DefinitionCommand::Pause {
                    automation_id,
                    expected_revision,
                    reason,
                }
            },
        ),
        "coven.automations.legacy.import.v1" => {
            let adoption_key = match required_adoption_key(&payload, action) {
                Ok(adoption_key) => adoption_key,
                Err(error) => return validation_rejection(action, error),
            };
            let source = match payload.get("source").and_then(Value::as_str) {
                Some("codex-automation-toml") => Ok(()),
                _ => Err(format!(
                    "{action} field `source` must be `codex-automation-toml`"
                )),
            };
            let dry_run = match payload.get("dryRun") {
                None => Ok(false),
                Some(Value::Bool(dry_run)) => Ok(*dry_run),
                Some(_) => Err(format!("{action} field `dryRun` must be a boolean")),
            };
            let command = match (source, dry_run) {
                (Ok(()), Ok(dry_run)) => {
                    crate::automations::command_adoption::DefinitionCommand::LegacyImport {
                        dry_run,
                    }
                }
                (Err(error), _) | (_, Err(error)) => {
                    crate::automations::command_adoption::DefinitionCommand::Invalid {
                        command: "legacy.import.v1".to_owned(),
                        request: command_request_fields(&payload, &["source", "dryRun"]),
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
                    recorded_at,
                    authority,
                ),
            )
        }
        crate::automations::command_envelope::ACTION => {
            crate::automations::command_envelope::route(
                &payload,
                conn,
                runtime,
                recorded_at,
                authority,
            )
        }
        "coven.automations.run.cancel.v1" => {
            match crate::automations::cancellation::execute_run_cancellation(
                conn,
                runtime,
                payload.clone(),
                chrono::Utc::now(),
            ) {
                Ok(crate::automations::cancellation::CancellationExecution::Success(success)) => {
                    let event = ControlEvent {
                        kind: "automations.run.cancellation",
                        action: action.to_string(),
                        origin,
                        intent_id,
                        payload: success.payload.clone(),
                    };
                    (
                        200,
                        ControlActionResponse {
                            ok: true,
                            accepted: true,
                            action: action.to_string(),
                            status: ActionStatus::Completed,
                            reason: success
                                .replayed
                                .then(|| "replayed previously adopted cancellation".to_string()),
                            error: None,
                            result: Some(success.payload),
                            event: Some(event),
                        },
                    )
                }
                Ok(crate::automations::cancellation::CancellationExecution::Rejected(error)) => {
                    typed_rejection(action, error)
                }
                Err(error) => typed_rejection(
                    action,
                    automation_error(
                        crate::automations::contract::error::ErrorCode::Internal,
                        error,
                    ),
                ),
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
                        recorded_at,
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
                                recorded_at,
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
                            recorded_at,
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
        "coven.automations.run.history.v1" => {
            automation_run_history_result(conn, action, origin, intent_id, &payload)
        }
        "coven.automations.receipt.get.v1" => automation_receipt_result(conn, action, &payload),
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
        "coven.automations.definition.health.v1" => match required_id_field(&payload, action) {
            Ok(id) => automation_definition_health_result(conn, action, origin, intent_id, &id),
            Err(error) => validation_rejection(action, error),
        },
        "coven.automations.scheduler.status.v1" => automation_result(
            action,
            origin,
            intent_id,
            automation_scheduler_status_payload(conn, chrono::Utc::now()),
        ),
        "coven.automations.occurrence.list.v1" => {
            let view = required_occurrence_view(&payload, action);
            let limit = optional_inspection_limit(&payload, action);
            let automation_id = optional_automation_filter(&payload, action);
            match (view, limit, automation_id) {
                (Ok(view), Ok(limit), Ok(automation_id)) => automation_result(
                    action,
                    origin,
                    intent_id,
                    automation_occurrence_list_payload(conn, view, limit, automation_id.as_deref()),
                ),
                (Err(error), _, _) | (_, Err(error), _) | (_, _, Err(error)) => {
                    (400, rejected_action(action, error))
                }
            }
        }
        "coven.automations.occurrence.history.v1" => {
            let automation_id = required_history_automation_id(&payload, action);
            let limit = optional_inspection_limit(&payload, action);
            let cursor = optional_history_cursor(&payload, action);
            match (automation_id, limit, cursor) {
                (Ok(automation_id), Ok(limit), Ok(cursor)) => automation_result(
                    action,
                    origin,
                    intent_id,
                    automation_occurrence_history_payload(
                        conn,
                        &automation_id,
                        limit,
                        cursor.as_ref(),
                    ),
                ),
                (Err(error), _, _) | (_, Err(error), _) | (_, _, Err(error)) => {
                    (400, rejected_action(action, error))
                }
            }
        }
        "coven.automations.run.get.v1" => {
            let id = required_id_field(&payload, action);
            match id {
                Ok(id) => automation_result(
                    action,
                    origin,
                    intent_id,
                    automation_run_get_payload(conn, &id),
                ),
                Err(error) => (400, rejected_action(action, error)),
            }
        }
        "coven.automations.occurrence.get.v1" => {
            let id = required_id_field(&payload, action);
            match id {
                Ok(id) => automation_result(
                    action,
                    origin,
                    intent_id,
                    automation_occurrence_get_payload(conn, &id),
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
        _ => match crate::automations::command_matrix::refused_command(action) {
            // A spec command without a versioned adapter is refused before
            // any adoption, definition, event or execution write.
            Some(entry) => typed_rejection(
                action,
                automation_error(
                    crate::automations::contract::error::ErrorCode::CapabilityUnsupported,
                    crate::automations::command_matrix::refusal_message(entry),
                ),
            ),
            None => (
                400,
                rejected_action(action, format!("unknown action `{action}`")),
            ),
        },
    }
}

/// Disable, activate and pause share one request shape: `id`, `adoptionKey`,
/// `expectedRevision` and an optional `reason`.
fn definition_status_command(
    conn: &rusqlite::Connection,
    payload: &Value,
    action: &str,
    (origin, intent_id): (Option<String>, Option<String>),
    (recorded_at, authority): (&str, CommandAuthority),
    command: &str,
    build: impl FnOnce(
        String,
        Option<u64>,
        Option<String>,
    ) -> crate::automations::command_adoption::DefinitionCommand,
) -> (u16, ControlActionResponse) {
    let id = required_id_field(payload, action);
    let adoption_key = required_adoption_key(payload, action);
    let expected_revision = required_expected_revision(payload, action);
    let reason = optional_reason(payload, action);
    let adoption_key = match adoption_key {
        Ok(adoption_key) => adoption_key,
        Err(error) => return validation_rejection(action, error),
    };
    let command = match (id, expected_revision, reason) {
        (Ok(id), Ok(expected_revision), Ok(reason)) => build(id, Some(expected_revision), reason),
        (Err(error), _, _) | (_, Err(error), _) | (_, _, Err(error)) => {
            crate::automations::command_adoption::DefinitionCommand::Invalid {
                command: command.to_owned(),
                request: command_request_fields(payload, &["id", "expectedRevision", "reason"]),
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
            recorded_at,
            authority,
        ),
    )
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

fn automation_receipt_result(
    conn: &rusqlite::Connection,
    action: &str,
    payload: &Value,
) -> (u16, ControlActionResponse) {
    use crate::automations::contract::error::ErrorCode;
    use crate::automations::contract::types::{PrivacyClassification, ReceiptId};

    let id = match required_id_field(payload, action)
        .and_then(|id| ReceiptId::new(id).map_err(|_| "receipt id is invalid".to_owned()))
    {
        Ok(id) => id,
        Err(error) => return validation_rejection(action, error),
    };
    let receipt = match crate::automations::receipts::read_receipt(conn, id.as_str()) {
        Ok(Some(receipt)) => receipt,
        Ok(None) => {
            return typed_rejection(
                action,
                automation_error(ErrorCode::NotFound, "Automation receipt is unavailable."),
            );
        }
        Err(_) => {
            return typed_rejection(
                action,
                automation_error(
                    ErrorCode::Internal,
                    "Stored automation receipt evidence could not be validated.",
                ),
            );
        }
    };
    if !matches!(
        receipt.privacy.classification,
        PrivacyClassification::Public | PrivacyClassification::Operational
    ) {
        return typed_rejection(
            action,
            automation_error(
                ErrorCode::AuthorityRequired,
                "Receipt privacy requires a principal-aware read policy that is unavailable.",
            ),
        );
    }
    (
        200,
        ControlActionResponse {
            ok: true,
            accepted: true,
            action: action.to_owned(),
            status: ActionStatus::Completed,
            reason: None,
            error: None,
            result: Some(json!({
                "receipt": receipt,
                "verification": {
                    "status": "unverifiable",
                    "integrity": "valid",
                    "correlation": "valid",
                    "receiptAuthentication": {
                        "status": "unverified",
                        "evidence": "unavailable"
                    },
                    "runtimeAuthority": {
                        "status": "unverified",
                        "evidence": "unavailable"
                    },
                    "reasons": [
                        "PRODUCER_AUTHENTICATION_UNVERIFIED",
                        "RUNTIME_AUTHORITY_UNVERIFIED"
                    ]
                }
            })),
            event: None,
        },
    )
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

pub(crate) fn automation_command_result(
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

pub(crate) fn validation_rejection(action: &str, reason: String) -> (u16, ControlActionResponse) {
    let error = automation_error(
        crate::automations::contract::error::ErrorCode::ValidationFailed,
        reason,
    );
    typed_rejection(action, error)
}

pub(crate) fn automation_error(
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

pub(crate) fn typed_rejection(
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

fn optional_reason(payload: &Value, action: &str) -> Result<Option<String>, String> {
    match payload.get("reason") {
        None => Ok(None),
        Some(Value::String(reason)) if !reason.trim().is_empty() && reason.len() <= 500 => {
            Ok(Some(reason.trim().to_owned()))
        }
        Some(_) => Err(format!(
            "{action} field `reason` must be a non-empty string of at most 500 bytes"
        )),
    }
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

fn required_occurrence_view(
    payload: &Value,
    action: &str,
) -> Result<crate::automations::inspection::OccurrenceView, String> {
    match payload.get("view").and_then(Value::as_str) {
        Some("due") => Ok(crate::automations::inspection::OccurrenceView::Due),
        Some("eligible") => Ok(crate::automations::inspection::OccurrenceView::Eligible),
        Some("claimed") => Ok(crate::automations::inspection::OccurrenceView::Claimed),
        Some("running") => Ok(crate::automations::inspection::OccurrenceView::Running),
        Some("recovery_required") => {
            Ok(crate::automations::inspection::OccurrenceView::RecoveryRequired)
        }
        _ => Err(format!(
            "{action} requires field `view` equal to due, eligible, claimed, running, or recovery_required"
        )),
    }
}

fn required_history_automation_id(payload: &Value, action: &str) -> Result<String, String> {
    payload
        .get("automationId")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(ToOwned::to_owned)
        .ok_or_else(|| format!("{action} requires non-empty string field `automationId`"))
}

const OCCURRENCE_HISTORY_CURSOR_MAX_CHARS: usize = 512;

/// Occurrence history cursors are opaque to clients: unpadded base64url of
/// a JSON `[scheduledFor, id]` keyset position. Only a cursor this producer
/// could have issued is accepted, so any other spelling is refused.
/// Unpadded base64url of the JSON pair `[first, second]`: an opaque keyset
/// position for a newest-first history read.
fn encode_keyset_cursor(first: &str, second: &str) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(json!([first, second]).to_string())
}

/// Decode an optional `cursor` field. Only the canonical spelling of a pair
/// this producer could have issued, within `max_chars`, is accepted.
fn optional_keyset_cursor(
    payload: &Value,
    action: &str,
    max_chars: usize,
) -> Result<Option<(String, String)>, String> {
    use base64::Engine as _;
    let Some(value) = payload.get("cursor") else {
        return Ok(None);
    };
    let invalid = || format!("{action} field `cursor` is not a cursor this producer issued");
    let cursor = value.as_str().ok_or_else(invalid)?;
    if cursor.is_empty() || cursor.len() > max_chars {
        return Err(invalid());
    }
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(cursor)
        .map_err(|_| invalid())?;
    let position: Vec<String> = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
    let [first, second] = <[String; 2]>::try_from(position).map_err(|_| invalid())?;
    if first.is_empty() || second.is_empty() {
        return Err(invalid());
    }
    // Canonical spelling only: re-encoding must reproduce the exact input.
    if encode_keyset_cursor(&first, &second) != cursor {
        return Err(invalid());
    }
    Ok(Some((first, second)))
}

fn encode_history_cursor(
    position: &crate::automations::inspection::OccurrenceHistoryPosition,
) -> String {
    encode_keyset_cursor(&position.scheduled_for, &position.id)
}

fn optional_history_cursor(
    payload: &Value,
    action: &str,
) -> Result<Option<crate::automations::inspection::OccurrenceHistoryPosition>, String> {
    Ok(
        optional_keyset_cursor(payload, action, OCCURRENCE_HISTORY_CURSOR_MAX_CHARS)?.map(
            |(scheduled_for, id)| crate::automations::inspection::OccurrenceHistoryPosition {
                scheduled_for,
                id,
            },
        ),
    )
}

/// An absent `automationId` keeps the global view; a present one must be a
/// non-empty string, so a malformed filter is refused rather than ignored.
fn optional_automation_filter(payload: &Value, action: &str) -> Result<Option<String>, String> {
    match payload.get("automationId") {
        None => Ok(None),
        Some(value) => value
            .as_str()
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .map(|id| Some(id.to_owned()))
            .ok_or_else(|| format!("{action} field `automationId` must be a non-empty string")),
    }
}

fn optional_inspection_limit(payload: &Value, action: &str) -> Result<usize, String> {
    match payload.get("limit") {
        None => Ok(20),
        Some(value) => value
            .as_u64()
            .filter(|value| (1..=100).contains(value))
            .and_then(|value| usize::try_from(value).ok())
            .ok_or_else(|| format!("{action} field `limit` must be an integer from 1 to 100")),
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
    match crate::automations::occurrences::tick_planning(conn, now) {
        Ok(report) => Ok(json!({
            "planned": report.planned,
            "alreadyFenced": report.already_fenced,
            "pausedSkipped": report.paused_skipped,
            "recovered": 0,
            "claimed": [],
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

/// `definition.health.v1`: the legacy health projection behind typed
/// absence, tombstone and internal errors.
fn automation_definition_health_result(
    conn: &rusqlite::Connection,
    action: &str,
    origin: Option<String>,
    intent_id: Option<String>,
    id: &str,
) -> (u16, ControlActionResponse) {
    use crate::automations::contract::error::ErrorCode;
    match crate::automations::store::get_definition_with_tombstone(conn, id, true) {
        Ok(None) => typed_rejection(
            action,
            automation_error(ErrorCode::NotFound, format!("no routine with id `{id}`")),
        ),
        Ok(Some(record)) if record.tombstoned_at.is_some() => typed_rejection(
            action,
            automation_error(
                ErrorCode::GoneTombstoned,
                format!("routine `{id}` is tombstoned"),
            ),
        ),
        Ok(Some(_)) => match automation_health_payload(conn, id, chrono::Utc::now()) {
            Ok(payload) => (200, automation_event(action, origin, intent_id, payload)),
            Err(error) => typed_rejection(action, automation_error(ErrorCode::Internal, error)),
        },
        Err(error) => typed_rejection(
            action,
            automation_error(ErrorCode::Internal, format!("{error:#}")),
        ),
    }
}

fn automation_scheduler_status_payload(
    conn: &rusqlite::Connection,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<Value, String> {
    crate::automations::diagnostics::scheduler_status(conn, now)
        .map(|status| {
            let last_pass = status.last_pass.map(|pass| {
                json!({
                    "generation": pass.generation,
                    "trigger": pass.trigger,
                    "scheduledAt": pass.scheduled_at,
                    "startedAt": pass.started_at,
                    "startLagMs": pass.start_lag_ms,
                    "finishedAt": pass.finished_at,
                    "durationMs": pass.duration_ms,
                    "status": pass.status,
                    "errorClass": pass.error_class,
                    "planned": pass.planned,
                    "recovered": pass.recovered,
                    "claimed": pass.claimed,
                    "dispatched": pass.dispatched,
                    "failures": pass.failures,
                })
            });
            json!({
                "scheduler": {
                    "authorityAssigned": status.authority_assigned,
                    "generation": status.generation,
                    "ownerId": status.owner_id,
                    "acquiredAt": status.acquired_at,
                    "lastPass": last_pass,
                    "queue": {
                        "planned": status.queue.planned,
                        "claimed": status.queue.claimed,
                        "running": status.queue.running,
                        "recoveryRequired": status.queue.recovery_required,
                        "batchLimit": status.queue.batch_limit,
                        "planningBatchLimit": status.queue.planning_batch_limit,
                        "planningAfterDefinitionId": status.queue.planning_after_definition_id,
                        "oldestEligibleAt": status.queue.oldest_eligible_at,
                        "oldestEligibleAgeMs": status.queue.oldest_eligible_age_ms,
                    }
                }
            })
        })
        .map_err(|error| format!("{error:#}"))
}

fn automation_occurrence_list_payload(
    conn: &rusqlite::Connection,
    view: crate::automations::inspection::OccurrenceView,
    limit: usize,
    automation_id: Option<&str>,
) -> Result<Value, String> {
    crate::automations::inspection::list_occurrences(
        conn,
        view,
        chrono::Utc::now(),
        limit,
        automation_id,
    )
    .map(|records| {
        let occurrences = records
            .into_iter()
            .map(automation_occurrence_value)
            .collect::<Vec<_>>();
        json!({ "occurrences": occurrences })
    })
    .map_err(|error| format!("{error:#}"))
}

fn automation_occurrence_get_payload(
    conn: &rusqlite::Connection,
    id: &str,
) -> Result<Value, String> {
    crate::automations::inspection::inspect_occurrence(conn, id)
        .map(|inspection| {
            let Some(inspection) = inspection else {
                return json!({ "occurrence": null });
            };
            let mut occurrence = automation_occurrence_value(inspection.occurrence);
            occurrence["runsTruncated"] = json!(inspection.runs_truncated);
            occurrence["runs"] = Value::Array(
                inspection
                    .runs
                    .into_iter()
                    .map(automation_run_value)
                    .collect(),
            );
            json!({ "occurrence": occurrence })
        })
        .map_err(|error| format!("{error:#}"))
}

fn automation_occurrence_history_payload(
    conn: &rusqlite::Connection,
    automation_id: &str,
    limit: usize,
    after: Option<&crate::automations::inspection::OccurrenceHistoryPosition>,
) -> Result<Value, String> {
    crate::automations::inspection::occurrence_history(conn, automation_id, limit, after)
        .map(|page| {
            let has_more = page.next.is_some();
            let mut cursor = json!({ "hasMore": has_more });
            // Echo the requested position so a pager can confirm which page it read.
            if let Some(after) = after {
                cursor["current"] = json!(encode_history_cursor(after));
            }
            if let Some(next) = &page.next {
                cursor["next"] = json!(encode_history_cursor(next));
            }
            json!({
                "automationId": automation_id,
                "occurrences": page
                    .occurrences
                    .into_iter()
                    .map(automation_occurrence_value)
                    .collect::<Vec<_>>(),
                "cursor": cursor,
            })
        })
        .map_err(|error| format!("{error:#}"))
}

fn automation_run_get_payload(conn: &rusqlite::Connection, id: &str) -> Result<Value, String> {
    crate::automations::inspection::inspect_run(conn, id)
        .map(|run| json!({ "run": run.map(automation_run_value) }))
        .map_err(|error| format!("{error:#}"))
}

fn automation_run_value(run: crate::automations::inspection::RunInspection) -> Value {
    let attempts = run
        .attempts
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
    json!({
        "id": run.id,
        "automationId": run.automation_id,
        "automationRevision": run.automation_revision,
        "definitionDigest": run.definition_digest,
        "occurrenceId": run.occurrence_id,
        "authorityProfile": run.authority_profile,
        "receiptId": run.receipt_id,
        "sessionId": run.session_id,
        "familiarId": run.familiar_id,
        "runtime": run.runtime,
        "status": run.status,
        "exitCode": run.exit_code,
        "logJson": run.log_json,
        "outputCommit": run.output_commit,
        "startedAt": run.started_at,
        "timeoutAt": run.timeout_at,
        "finishedAt": run.finished_at,
        "attempts": attempts,
    })
}

fn automation_occurrence_value(record: crate::automations::inspection::OccurrenceRecord) -> Value {
    json!({
        "id": record.id,
        "automationId": record.automation_id,
        "automationRevision": record.automation_revision,
        "definitionDigest": record.definition_digest,
        "scheduledFor": record.scheduled_for,
        "kind": record.kind,
        "state": record.state,
        "leaseOwner": record.lease_owner,
        "leaseExpiresAt": record.lease_expires_at,
        "schedulerGeneration": record.scheduler_generation,
        "fenceGeneration": record.fence_generation,
        "failureReason": record.failure_reason,
        "createdAt": record.created_at,
        "updatedAt": record.updated_at,
    })
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

fn attempt_value(attempt: &crate::automations::runs::AttemptRecord) -> Value {
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
}

/// The compatibility run projection shared by `runs` and `run.history.v1`.
fn run_value(
    record: &crate::automations::runs::RunRecord,
    attempts: Vec<Value>,
    cancellation: Option<Value>,
) -> Value {
    let mut run = json!({
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
        "receiptId": record.receipt_id,
        "attempts": attempts,
    });
    if let Some(cancellation) = cancellation {
        run["cancellation"] = cancellation;
    }
    run
}

/// Spec bound for `run.history.v1` cursors (`command-envelope.schema.json`).
const RUN_HISTORY_CURSOR_MAX_CHARS: usize = 256;

fn automation_run_history_result(
    conn: &rusqlite::Connection,
    action: &str,
    origin: Option<String>,
    intent_id: Option<String>,
    payload: &Value,
) -> (u16, ControlActionResponse) {
    use crate::automations::contract::error::ErrorCode;
    let automation_id = match required_history_automation_id(payload, action) {
        Ok(automation_id) => automation_id,
        Err(error) => return validation_rejection(action, error),
    };
    let occurrence_id = match payload.get("occurrenceId") {
        None => None,
        Some(value) => match value.as_str().map(str::trim).filter(|id| !id.is_empty()) {
            Some(id) => Some(id.to_owned()),
            None => {
                return validation_rejection(
                    action,
                    format!("{action} field `occurrenceId` must be a non-empty string"),
                )
            }
        },
    };
    let limit = match optional_inspection_limit(payload, action) {
        Ok(limit) => limit,
        Err(error) => return validation_rejection(action, error),
    };
    let after = match optional_keyset_cursor(payload, action, RUN_HISTORY_CURSOR_MAX_CHARS) {
        Ok(after) => after.map(
            |(started_at, id)| crate::automations::runs::RunHistoryPosition { started_at, id },
        ),
        Err(error) => return validation_rejection(action, error),
    };
    let internal =
        |error: String| typed_rejection(action, automation_error(ErrorCode::Internal, error));
    // One snapshot for the page, its attempts and its cancellations.
    let page = (|| -> anyhow::Result<Value> {
        let transaction = conn.unchecked_transaction()?;
        let page = crate::automations::runs::run_history(
            &transaction,
            &automation_id,
            occurrence_id.as_deref(),
            limit,
            after.as_ref(),
        )?;
        let mut runs = Vec::with_capacity(page.runs.len());
        for record in &page.runs {
            let attempts = crate::automations::runs::list_attempts(&transaction, &record.id)?
                .iter()
                .map(attempt_value)
                .collect();
            let cancellation =
                crate::automations::cancellation::cancellation_for_run(&transaction, &record.id)
                    .map_err(anyhow::Error::msg)?;
            runs.push(run_value(record, attempts, cancellation));
        }
        transaction.commit()?;
        let mut cursor = json!({ "hasMore": page.next.is_some() });
        if let Some(after) = &after {
            cursor["current"] = json!(encode_keyset_cursor(&after.started_at, &after.id));
        }
        if let Some(next) = &page.next {
            let next = encode_keyset_cursor(&next.started_at, &next.id);
            anyhow::ensure!(
                next.len() <= RUN_HISTORY_CURSOR_MAX_CHARS,
                "run history position does not fit the cursor bound"
            );
            cursor["next"] = json!(next);
        }
        let mut result = json!({
            "automationId": automation_id,
            "runs": runs,
            "cursor": cursor,
        });
        if let Some(occurrence_id) = &occurrence_id {
            result["occurrenceId"] = json!(occurrence_id);
        }
        Ok(result)
    })();
    match page {
        Ok(payload) => (200, automation_event(action, origin, intent_id, payload)),
        Err(error) => internal(format!("{error:#}")),
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
                let cancellation =
                    crate::automations::cancellation::cancellation_for_run(conn, &record.id)?;
                let attempts = attempts_by_run
                    .remove(&record.id)
                    .unwrap_or_default()
                    .into_iter()
                    .map(|attempt| attempt_value(&attempt))
                    .collect::<Vec<_>>();
                let run = run_value(record, attempts, cancellation);
                runs.push(run);
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
    // One snapshot, so the routine and its rich form come from one revision.
    let snapshot = conn
        .unchecked_transaction()
        .map_err(|error| format!("failed to begin definition read: {error}"))?;
    let conn: &rusqlite::Connection = &snapshot;
    match crate::automations::store::get_definition_with_tombstone(conn, id, true) {
        Ok(Some(record)) => match serde_json::from_str::<Value>(&record.definition_json) {
            Ok(routine) => {
                let mut payload = json!({
                    "routine": routine,
                    "revision": record.revision,
                    "tombstonedAt": record.tombstoned_at,
                });
                // Richly authored definitions also return their rich form,
                // regenerated for the current revision and lifecycle state.
                match crate::automations::rich_definition::current_view(conn, id) {
                    Ok(Some(definition)) => payload["definition"] = definition,
                    Ok(None) => {}
                    Err(error) => {
                        return Err(format!("stored rich definition is unreadable: {error:#}"))
                    }
                }
                Ok(payload)
            }
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
    use chrono::{TimeZone, Timelike};

    struct OwnershipThenErrorRuntime;
    struct RejectedRuntime;
    struct RetryableRejectedRuntime;

    #[test]
    fn automation_capability_negotiation_matches_the_packaged_profile() {
        let catalog = serde_json::to_value(capabilities()).unwrap();
        let expected: Value = serde_json::from_str(include_str!(
            "../../../spec/coven-automations/v1/capabilities.json"
        ))
        .unwrap();
        let capabilities = catalog["capabilities"].as_array().unwrap();
        let automations = capabilities
            .iter()
            .find(|capability| capability["id"] == "coven.automations")
            .unwrap();

        assert_eq!(automations["variantNegotiation"], expected);
        for capability in capabilities
            .iter()
            .filter(|capability| capability["id"] != "coven.automations")
        {
            assert!(
                capability.get("variantNegotiation").is_none(),
                "unrelated capability changed: {capability}"
            );
        }
    }

    #[test]
    fn tick_action_plans_but_does_not_claim_without_scheduler_authority() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("store.sqlite");
        crate::store::initialize_store(&path).unwrap();
        let conn = crate::store::open_store(&path).unwrap();
        let now = chrono::Utc::now();
        let definition = crate::automations::RoutineDefinition::from_json(&json!({
            "schemaVersion": 1,
            "id": "tick-authority",
            "name": "Tick authority",
            "status": "ACTIVE",
            "rrule": format!("FREQ=DAILY;BYHOUR={}", now.hour()),
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
        conn.execute(
            "UPDATE automation_definitions
             SET created_at = ?1, updated_at = ?1
             WHERE id = ?2",
            rusqlite::params![
                (now - chrono::Duration::days(1))
                    .to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                definition.id
            ],
        )
        .unwrap();

        let (status, response) = route_action(
            json!({"action": "coven.automations.tick"}),
            &conn,
            &crate::api::NoopSessionRuntime,
        );

        assert_eq!(status, 200);
        assert!(response.ok);
        assert_eq!(
            response.event.as_ref().unwrap().payload["claimed"],
            json!([])
        );
        let occurrence_state: String = conn
            .query_row(
                "SELECT state
                 FROM automation_occurrences
                 WHERE automation_id = ?1",
                [&definition.id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(occurrence_state, "planned");
    }

    #[test]
    fn scheduler_status_action_exposes_idle_authority_and_queue_state() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("store.sqlite");
        crate::store::initialize_store(&path).unwrap();
        let conn = crate::store::open_store(&path).unwrap();

        let (status, response) = route_action(
            json!({"action": "coven.automations.scheduler.status.v1"}),
            &conn,
            &crate::api::NoopSessionRuntime,
        );

        assert_eq!(status, 200);
        assert!(response.ok);
        let scheduler = &response.event.as_ref().unwrap().payload["scheduler"];
        assert!(scheduler["active"].is_null());
        assert_eq!(scheduler["authorityAssigned"], false);
        assert_eq!(scheduler["generation"], 0);
        assert!(scheduler["ownerId"].is_null());
        assert!(scheduler["acquiredAt"].is_null());
        assert!(scheduler["lastPass"].is_null());
        assert_eq!(
            scheduler["queue"],
            json!({
                "planned": 0,
                "claimed": 0,
                "running": 0,
                "recoveryRequired": 0,
                "batchLimit": 64,
                "planningBatchLimit": 64,
                "planningAfterDefinitionId": null,
                "oldestEligibleAt": null,
                "oldestEligibleAgeMs": null,
            })
        );
        assert!(capabilities()
            .capabilities
            .iter()
            .find(|capability| capability.id == "coven.automations")
            .unwrap()
            .actions
            .contains(&"coven.automations.scheduler.status.v1"));
    }

    #[test]
    fn occurrence_list_action_distinguishes_due_from_eligible() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("store.sqlite");
        crate::store::initialize_store(&path).unwrap();
        let conn = crate::store::open_store(&path).unwrap();
        let definition = crate::automations::RoutineDefinition::from_json(&json!({
            "schemaVersion": 1,
            "id": "inspection",
            "name": "inspection",
            "status": "ACTIVE",
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
        let definition_digest: String = conn
            .query_row(
                "SELECT definition_digest
                 FROM automation_definitions
                 WHERE id = 'inspection'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        conn.execute_batch(
            "INSERT INTO automation_occurrences
                (id, automation_id, automation_revision, definition_digest, scheduled_for,
                 kind, state, attempt, created_at, updated_at)
             SELECT 'older-due', id, revision, definition_digest,
                    '2026-09-01T08:00:00.000Z', 'scheduled', 'planned', 0,
                    '2026-09-01T08:00:00.000Z', '2026-09-01T08:00:00.000Z'
             FROM automation_definitions WHERE id = 'inspection';
             INSERT INTO automation_occurrences
                (id, automation_id, automation_revision, definition_digest, scheduled_for,
                 kind, state, attempt, created_at, updated_at)
             SELECT 'latest-due', id, revision, definition_digest,
                    '2026-09-01T09:00:00.000Z', 'scheduled', 'planned', 0,
                    '2026-09-01T09:00:00.000Z', '2026-09-01T09:00:00.000Z'
             FROM automation_definitions WHERE id = 'inspection';",
        )
        .unwrap();

        let (due_status, due_response) = route_action(
            json!({
                "action": "coven.automations.occurrence.list.v1",
                "view": "due"
            }),
            &conn,
            &crate::api::NoopSessionRuntime,
        );
        let (eligible_status, eligible_response) = route_action(
            json!({
                "action": "coven.automations.occurrence.list.v1",
                "view": "eligible"
            }),
            &conn,
            &crate::api::NoopSessionRuntime,
        );

        assert_eq!(due_status, 200);
        assert_eq!(
            due_response.event.as_ref().unwrap().payload["occurrences"],
            json!([
                {
                    "id": "older-due",
                    "automationId": "inspection",
                    "automationRevision": 1,
                    "definitionDigest": definition_digest,
                    "scheduledFor": "2026-09-01T08:00:00.000Z",
                    "kind": "scheduled",
                    "state": "planned",
                    "leaseOwner": null,
                    "leaseExpiresAt": null,
                    "schedulerGeneration": null,
                    "fenceGeneration": 0,
                    "failureReason": null,
                    "createdAt": "2026-09-01T08:00:00.000Z",
                    "updatedAt": "2026-09-01T08:00:00.000Z"
                },
                {
                    "id": "latest-due",
                    "automationId": "inspection",
                    "automationRevision": 1,
                    "definitionDigest": definition_digest,
                    "scheduledFor": "2026-09-01T09:00:00.000Z",
                    "kind": "scheduled",
                    "state": "planned",
                    "leaseOwner": null,
                    "leaseExpiresAt": null,
                    "schedulerGeneration": null,
                    "fenceGeneration": 0,
                    "failureReason": null,
                    "createdAt": "2026-09-01T09:00:00.000Z",
                    "updatedAt": "2026-09-01T09:00:00.000Z"
                }
            ])
        );
        assert_eq!(eligible_status, 200);
        assert_eq!(
            eligible_response.event.as_ref().unwrap().payload["occurrences"],
            json!([{
                "id": "latest-due",
                "automationId": "inspection",
                "automationRevision": 1,
                "definitionDigest": definition_digest,
                "scheduledFor": "2026-09-01T09:00:00.000Z",
                "kind": "scheduled",
                "state": "planned",
                "leaseOwner": null,
                "leaseExpiresAt": null,
                "schedulerGeneration": null,
                "fenceGeneration": 0,
                "failureReason": null,
                "createdAt": "2026-09-01T09:00:00.000Z",
                "updatedAt": "2026-09-01T09:00:00.000Z"
            }])
        );
        assert!(capabilities()
            .capabilities
            .iter()
            .find(|capability| capability.id == "coven.automations")
            .unwrap()
            .actions
            .contains(&"coven.automations.occurrence.list.v1"));
    }

    #[test]
    fn occurrence_list_action_exposes_exact_nonterminal_authority() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("store.sqlite");
        crate::store::initialize_store(&path).unwrap();
        let conn = crate::store::open_store(&path).unwrap();
        let definition = crate::automations::RoutineDefinition::from_json(&json!({
            "schemaVersion": 1,
            "id": "authority-inspection",
            "name": "authority-inspection",
            "status": "ACTIVE",
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
        conn.execute_batch(
            "INSERT INTO automation_occurrences
                (id, automation_id, automation_revision, definition_digest, scheduled_for,
                 kind, state, lease_owner, lease_expires_at, scheduler_generation, attempt,
                 failure_reason, created_at, updated_at)
             SELECT 'claimed-inspection', id, revision, definition_digest,
                    '2026-09-01T08:00:00.000Z', 'scheduled', 'claimed', 'daemon-a',
                    '2026-09-01T08:05:00.000Z', 7, 2, NULL,
                    '2026-09-01T08:00:00.000Z', '2026-09-01T08:01:00.000Z'
             FROM automation_definitions WHERE id = 'authority-inspection';
             INSERT INTO automation_occurrences
                (id, automation_id, automation_revision, definition_digest, scheduled_for,
                 kind, state, lease_owner, lease_expires_at, scheduler_generation, attempt,
                 failure_reason, created_at, updated_at)
             SELECT 'running-inspection', id, revision, definition_digest,
                    '2026-09-01T09:00:00.000Z', 'manual', 'running', 'manual-owner',
                    '2026-09-01T09:05:00.000Z', NULL, 3, NULL,
                    '2026-09-01T09:00:00.000Z', '2026-09-01T09:01:00.000Z'
             FROM automation_definitions WHERE id = 'authority-inspection';
             INSERT INTO automation_occurrences
                (id, automation_id, automation_revision, definition_digest, scheduled_for,
                 kind, state, lease_owner, lease_expires_at, scheduler_generation, attempt,
                 failure_reason, created_at, updated_at)
             SELECT 'recovery-inspection', id, revision, definition_digest,
                    '2026-09-01T10:00:00.000Z', 'scheduled', 'recovery_required', NULL,
                    NULL, 6, 4, 'runtime ownership is ambiguous',
                    '2026-09-01T10:00:00.000Z', '2026-09-01T10:01:00.000Z'
             FROM automation_definitions WHERE id = 'authority-inspection';",
        )
        .unwrap();

        for (view, expected) in [
            (
                "claimed",
                json!({
                    "id": "claimed-inspection",
                    "state": "claimed",
                    "leaseOwner": "daemon-a",
                    "leaseExpiresAt": "2026-09-01T08:05:00.000Z",
                    "schedulerGeneration": 7,
                    "fenceGeneration": 2,
                    "failureReason": null
                }),
            ),
            (
                "running",
                json!({
                    "id": "running-inspection",
                    "state": "running",
                    "leaseOwner": "manual-owner",
                    "leaseExpiresAt": "2026-09-01T09:05:00.000Z",
                    "schedulerGeneration": null,
                    "fenceGeneration": 3,
                    "failureReason": null
                }),
            ),
            (
                "recovery_required",
                json!({
                    "id": "recovery-inspection",
                    "state": "recovery_required",
                    "leaseOwner": null,
                    "leaseExpiresAt": null,
                    "schedulerGeneration": 6,
                    "fenceGeneration": 4,
                    "failureReason": "runtime ownership is ambiguous"
                }),
            ),
        ] {
            let (status, response) = route_action(
                json!({
                    "action": "coven.automations.occurrence.list.v1",
                    "view": view
                }),
                &conn,
                &crate::api::NoopSessionRuntime,
            );

            assert_eq!(status, 200);
            let occurrences = response.event.as_ref().unwrap().payload["occurrences"]
                .as_array()
                .unwrap();
            assert_eq!(occurrences.len(), 1);
            for field in [
                "id",
                "state",
                "leaseOwner",
                "leaseExpiresAt",
                "schedulerGeneration",
                "fenceGeneration",
                "failureReason",
            ] {
                assert_eq!(occurrences[0][field], expected[field], "{view} {field}");
            }
        }
    }

    #[test]
    fn occurrence_list_keeps_ready_legacy_retry_eligible_without_timeout() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("store.sqlite");
        crate::store::initialize_store(&path).unwrap();
        let conn = crate::store::open_store(&path).unwrap();
        let definition = crate::automations::RoutineDefinition::from_json(&json!({
            "schemaVersion": 1,
            "id": "legacy-timeout-retry",
            "name": "legacy-timeout-retry",
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
        conn.execute_batch(
            "INSERT INTO automation_occurrences
                (id, automation_id, automation_revision, definition_digest, scheduled_for,
                 kind, state, attempt, created_at, updated_at)
             SELECT 'legacy-timeout-occurrence', id, revision, definition_digest,
                    '2026-09-01T09:00:00.000Z', 'scheduled', 'planned', 1,
                    '2026-09-01T09:00:00.000Z', '2026-09-01T09:00:00.000Z'
             FROM automation_definitions WHERE id = 'legacy-timeout-retry';
             INSERT INTO automation_runs
                (id, automation_id, automation_revision, definition_digest, occurrence_id,
                 runtime, status, started_at, timeout_at)
             SELECT 'legacy-timeout-run', automation_id, automation_revision,
                    definition_digest, id, 'coven-code', 'running',
                    '2026-09-01T09:00:00.000Z', NULL
             FROM automation_occurrences WHERE id = 'legacy-timeout-occurrence';
             INSERT INTO automation_attempts
                (id, run_id, occurrence_id, attempt_number, adoption_key,
                 occurrence_fence_generation, dispatch_generation, state,
                 retry_classification, not_before, opened_at)
             VALUES
                ('legacy-timeout-attempt', 'legacy-timeout-run',
                 'legacy-timeout-occurrence', 1, 'legacy-timeout-adoption',
                 1, 0, 'adopted', 'initial', '2026-09-01T09:00:00.000Z',
                 '2026-09-01T09:00:00.000Z');",
        )
        .unwrap();

        let (status, response) = route_action(
            json!({
                "action": "coven.automations.occurrence.list.v1",
                "view": "eligible"
            }),
            &conn,
            &crate::api::NoopSessionRuntime,
        );

        assert_eq!(status, 200);
        assert_eq!(
            response.event.as_ref().unwrap().payload["occurrences"][0]["id"],
            "legacy-timeout-occurrence"
        );
    }

    #[test]
    fn occurrence_list_validates_only_the_bounded_eligible_page() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("store.sqlite");
        crate::store::initialize_store(&path).unwrap();
        let conn = crate::store::open_store(&path).unwrap();
        for id in ["valid-time", "invalid-time"] {
            let definition = crate::automations::RoutineDefinition::from_json(&json!({
                "schemaVersion": 1,
                "id": id,
                "name": id,
                "status": "ACTIVE",
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
        }
        conn.execute_batch(
            "INSERT INTO automation_occurrences
                (id, automation_id, automation_revision, definition_digest, scheduled_for,
                 kind, state, attempt, created_at, updated_at)
             SELECT 'valid-time-occurrence', id, revision, definition_digest,
                    '2026-09-01T09:00:00.000Z', 'scheduled', 'planned', 0,
                    '2026-09-01T09:00:00.000Z', '2026-09-01T09:00:00.000Z'
             FROM automation_definitions WHERE id = 'valid-time';
             INSERT INTO automation_occurrences
                (id, automation_id, automation_revision, definition_digest, scheduled_for,
                 kind, state, attempt, created_at, updated_at)
             SELECT 'invalid-time-occurrence', id, revision, definition_digest,
                    '2026-09-01T09:30:00.000Z-bad', 'scheduled', 'planned', 0,
                    '2026-09-01T09:30:00.000Z', '2026-09-01T09:30:00.000Z'
             FROM automation_definitions WHERE id = 'invalid-time';",
        )
        .unwrap();

        let (bounded_status, bounded_response) = route_action(
            json!({
                "action": "coven.automations.occurrence.list.v1",
                "view": "eligible",
                "limit": 1
            }),
            &conn,
            &crate::api::NoopSessionRuntime,
        );
        let (corrupt_status, corrupt_response) = route_action(
            json!({
                "action": "coven.automations.occurrence.list.v1",
                "view": "eligible",
                "limit": 2
            }),
            &conn,
            &crate::api::NoopSessionRuntime,
        );

        assert_eq!(bounded_status, 200);
        assert_eq!(
            bounded_response.event.as_ref().unwrap().payload["occurrences"][0]["id"],
            "valid-time-occurrence"
        );
        assert_eq!(corrupt_status, 400);
        assert!(corrupt_response
            .reason
            .as_deref()
            .unwrap()
            .contains("invalid scheduled occurrence timestamp"));
    }

    #[test]
    fn occurrence_get_action_exposes_correlated_run_attempt_and_fences() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("store.sqlite");
        crate::store::initialize_store(&path).unwrap();
        let conn = crate::store::open_store(&path).unwrap();
        let definition = crate::automations::RoutineDefinition::from_json(&json!({
            "schemaVersion": 1,
            "id": "exact-inspection",
            "name": "exact-inspection",
            "status": "ACTIVE",
            "rrule": "FREQ=DAILY;BYHOUR=9",
            "timezone": "utc",
            "misfire": "latest",
            "overlap": "forbid",
            "timeoutMinutes": 30,
            "runtime": "coven-code",
            "familiarId": "familiar-1",
            "cwd": "/work/project",
            "prompt": "Do the thing."
        }))
        .unwrap();
        crate::automations::store::insert_definition(&conn, &definition).unwrap();
        conn.execute_batch(
            "INSERT INTO automation_occurrences
                (id, automation_id, automation_revision, definition_digest, scheduled_for,
                 kind, state, lease_owner, lease_expires_at, scheduler_generation, attempt,
                 created_at, updated_at)
             SELECT 'exact-occurrence', id, revision, definition_digest,
                    '2026-09-01T09:00:00.000Z', 'scheduled', 'running', 'daemon-a',
                    '2026-09-01T09:05:00.000Z', 11, 4,
                    '2026-09-01T09:00:00.000Z', '2026-09-01T09:01:00.000Z'
             FROM automation_definitions WHERE id = 'exact-inspection';
             INSERT INTO automation_runs
                (id, automation_id, automation_revision, definition_digest, occurrence_id,
                 authority_profile, receipt_id, session_id, familiar_id, runtime, status,
                 started_at, timeout_at)
             SELECT 'exact-run', automation_id, automation_revision, definition_digest, id,
                    'coven.automations.authority.v1', 'receipt-1', NULL, 'familiar-1',
                    'coven-code', 'running', '2026-09-01T09:01:00.000Z',
                    '2026-09-01T09:31:00.000Z'
             FROM automation_occurrences WHERE id = 'exact-occurrence';
             INSERT INTO automation_attempts
                (id, run_id, occurrence_id, attempt_number, adoption_key,
                 occurrence_fence_generation, dispatch_generation, state, failure_class,
                 prior_attempt_number, prior_disposition, retry_classification, not_before,
                 session_id, state_reason, opened_at, settled_at)
             VALUES
                ('exact-attempt', 'exact-run', 'exact-occurrence', 2, 'adopt-exact',
                 4, 9, 'observing', NULL, 1, 'failed', 'automatic_retry',
                 '2026-09-01T09:00:30.000Z', NULL, 'runtime ownership published',
                 '2026-09-01T09:01:00.000Z', NULL);",
        )
        .unwrap();

        let (status, response) = route_action(
            json!({
                "action": "coven.automations.occurrence.get.v1",
                "id": "exact-occurrence"
            }),
            &conn,
            &crate::api::NoopSessionRuntime,
        );

        assert_eq!(status, 200);
        let occurrence = &response.event.as_ref().unwrap().payload["occurrence"];
        assert_eq!(occurrence["id"], "exact-occurrence");
        assert_eq!(occurrence["automationRevision"], 1);
        assert_eq!(occurrence["leaseOwner"], "daemon-a");
        assert_eq!(occurrence["leaseExpiresAt"], "2026-09-01T09:05:00.000Z");
        assert_eq!(occurrence["schedulerGeneration"], 11);
        assert_eq!(occurrence["fenceGeneration"], 4);
        assert_eq!(occurrence["runs"].as_array().unwrap().len(), 1);
        let run = &occurrence["runs"][0];
        assert_eq!(run["id"], "exact-run");
        assert_eq!(run["authorityProfile"], "coven.automations.authority.v1");
        assert_eq!(run["receiptId"], "receipt-1");
        assert_eq!(run["timeoutAt"], "2026-09-01T09:31:00.000Z");
        assert_eq!(run["attempts"].as_array().unwrap().len(), 1);
        let attempt = &run["attempts"][0];
        assert_eq!(attempt["id"], "exact-attempt");
        assert_eq!(attempt["attemptNumber"], 2);
        assert_eq!(attempt["adoptionKey"], "adopt-exact");
        assert_eq!(attempt["occurrenceFenceGeneration"], 4);
        assert_eq!(attempt["dispatchGeneration"], 9);
        assert_eq!(attempt["state"], "observing");
        assert_eq!(attempt["priorAttemptNumber"], 1);
        assert_eq!(attempt["priorDisposition"], "failed");
        assert_eq!(attempt["retryClassification"], "automatic_retry");
        assert_eq!(attempt["stateReason"], "runtime ownership published");
        assert!(capabilities()
            .capabilities
            .iter()
            .find(|capability| capability.id == "coven.automations")
            .unwrap()
            .actions
            .contains(&"coven.automations.occurrence.get.v1"));
    }

    #[test]
    fn occurrence_get_bounds_corrupt_duplicate_run_history() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("store.sqlite");
        crate::store::initialize_store(&path).unwrap();
        let conn = crate::store::open_store(&path).unwrap();
        let definition = crate::automations::RoutineDefinition::from_json(&json!({
            "schemaVersion": 1,
            "id": "bounded-inspection",
            "name": "bounded-inspection",
            "status": "ACTIVE",
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
        conn.execute(
            "INSERT INTO automation_occurrences
                (id, automation_id, automation_revision, definition_digest, scheduled_for,
                 kind, state, attempt, created_at, updated_at)
             SELECT 'bounded-occurrence', id, revision, definition_digest,
                    '2026-09-01T09:00:00.000Z', 'scheduled', 'failed', 1,
                    '2026-09-01T09:00:00.000Z', '2026-09-01T09:00:00.000Z'
             FROM automation_definitions WHERE id = 'bounded-inspection'",
            [],
        )
        .unwrap();
        for index in 0..21 {
            conn.execute(
                "INSERT INTO automation_runs
                    (id, automation_id, automation_revision, definition_digest, occurrence_id,
                     runtime, status, started_at, finished_at)
                 SELECT ?1, automation_id, automation_revision, definition_digest, id,
                        'coven-code', 'failed', ?2, ?2
                 FROM automation_occurrences WHERE id = 'bounded-occurrence'",
                rusqlite::params![
                    format!("bounded-run-{index:02}"),
                    format!("2026-09-01T09:{index:02}:00.000Z")
                ],
            )
            .unwrap();
        }

        let (status, response) = route_action(
            json!({
                "action": "coven.automations.occurrence.get.v1",
                "id": "bounded-occurrence"
            }),
            &conn,
            &crate::api::NoopSessionRuntime,
        );

        assert_eq!(status, 200);
        let occurrence = &response.event.as_ref().unwrap().payload["occurrence"];
        assert_eq!(occurrence["runs"].as_array().unwrap().len(), 20);
        assert_eq!(occurrence["runsTruncated"], true);
    }

    /// Every row of every table, as sorted text, so a test can prove a request
    /// wrote nothing anywhere in the store.
    fn store_snapshot(conn: &rusqlite::Connection) -> Vec<String> {
        let tables = conn
            .prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        let mut snapshot = Vec::new();
        for table in tables {
            let mut statement = conn.prepare(&format!("SELECT * FROM \"{table}\"")).unwrap();
            let columns = statement.column_count();
            let mut rows = statement
                .query_map([], |row| {
                    (0..columns)
                        .map(|index| {
                            row.get::<_, rusqlite::types::Value>(index)
                                .map(|value| format!("{value:?}"))
                        })
                        .collect::<Result<Vec<_>, _>>()
                })
                .unwrap()
                .map(|row| format!("{table}: {}", row.unwrap().join(" | ")))
                .collect::<Vec<_>>();
            rows.sort();
            snapshot.push(format!("{table}: {} rows", rows.len()));
            snapshot.extend(rows);
        }
        snapshot
    }

    #[test]
    fn command_matrix_matches_advertisement_and_dispatch() {
        use crate::automations::command_matrix::{
            action_name, CommandSupport, COMMAND_MATRIX, PRODUCER_READ_EXTENSIONS,
        };
        let advertised = capabilities()
            .capabilities
            .into_iter()
            .find(|capability| capability.id == "coven.automations")
            .unwrap()
            .actions;
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("store.sqlite");
        crate::store::initialize_store(&path).unwrap();
        let conn = crate::store::open_store(&path).unwrap();

        for entry in COMMAND_MATRIX {
            let action = action_name(entry.command);
            let implemented = entry.support == CommandSupport::Implemented;
            assert_eq!(
                advertised.contains(&action.as_str()),
                implemented,
                "{action} must be advertised exactly when implemented"
            );
            if let CommandSupport::CompatibilityOnly { legacy_action } = entry.support {
                assert!(advertised.contains(&legacy_action), "{legacy_action}");
            }
            let (_, response) = route_action(
                json!({ "action": action }),
                &conn,
                &crate::api::NoopSessionRuntime,
            );
            let code = response
                .error
                .as_ref()
                .and_then(|error| error["code"].as_str());
            if implemented {
                assert_ne!(code, Some("CAPABILITY_UNSUPPORTED"), "{action}");
                assert!(
                    !response
                        .reason
                        .as_deref()
                        .unwrap_or_default()
                        .starts_with("unknown action"),
                    "{action} must be dispatched"
                );
            } else {
                assert_eq!(code, Some("CAPABILITY_UNSUPPORTED"), "{action}");
            }
        }

        // A versioned action is advertised only as an implemented command or
        // a named read extension; nothing slips in outside the matrix.
        for action in advertised.iter().filter(|action| action.ends_with(".v1")) {
            let command = action.strip_prefix("coven.automations.").unwrap();
            assert!(
                COMMAND_MATRIX.iter().any(|entry| entry.command == command
                    && entry.support == CommandSupport::Implemented)
                    || PRODUCER_READ_EXTENSIONS.contains(action)
                    || *action == crate::automations::command_envelope::ACTION,
                "{action} is advertised but absent from the command matrix"
            );
        }
        for extension in PRODUCER_READ_EXTENSIONS {
            assert!(advertised.contains(extension), "{extension}");
        }
    }

    #[test]
    fn refused_commands_write_nothing() {
        use crate::automations::command_matrix::{action_name, CommandSupport, COMMAND_MATRIX};
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("store.sqlite");
        crate::store::initialize_store(&path).unwrap();
        let conn = crate::store::open_store(&path).unwrap();
        filter_fixture_definition(&conn, "alpha");
        filter_fixture_occurrence(
            &conn,
            "alpha-1",
            "alpha",
            "2026-09-01T09:00:00.000Z",
            "failed",
        );
        let before = store_snapshot(&conn);

        let refused = COMMAND_MATRIX
            .iter()
            .filter(|entry| entry.support != CommandSupport::Implemented)
            .collect::<Vec<_>>();
        assert!(!refused.is_empty());
        for entry in refused {
            // Every field any of these commands could plausibly act on, so a
            // partial adapter would have something to write.
            let request = json!({
                "action": action_name(entry.command),
                "schemaVersion": "coven.automations.v1",
                "command": entry.command,
                "adoptionKey": format!("adopt:{}", entry.command),
                "expectedRevision": 1,
                "id": "alpha",
                "automationId": "alpha",
                "occurrenceId": "alpha-1",
                "runId": "run-1",
                "attemptId": "attempt-1",
                "priorDisposition": "failed",
                "origin": "cli",
                "intentId": "intent-1",
                "intent": { "statement": "Exercise a refused command." },
                "payload": { "automationId": "alpha", "occurrenceId": "alpha-1" }
            });
            let (status, response) = route_action(request, &conn, &crate::api::NoopSessionRuntime);
            let error = response.error.as_ref().expect("typed error");
            assert_eq!(error["code"], "CAPABILITY_UNSUPPORTED", "{}", entry.command);
            assert_eq!(status, error["httpStatus"].as_u64().unwrap() as u16);
            assert!(!response.ok && !response.accepted);
            assert!(response.event.is_none() && response.result.is_none());
            let reason = response.reason.as_deref().unwrap();
            assert!(reason.contains(entry.command), "{reason}");
            if let CommandSupport::CompatibilityOnly { legacy_action } = entry.support {
                assert!(reason.contains(legacy_action), "{reason}");
            }
        }
        assert_eq!(store_snapshot(&conn), before);
    }

    fn filter_fixture_definition(conn: &rusqlite::Connection, id: &str) {
        let definition = crate::automations::RoutineDefinition::from_json(&json!({
            "schemaVersion": 1,
            "id": id,
            "name": id,
            "status": "ACTIVE",
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
        crate::automations::store::insert_definition(conn, &definition).unwrap();
    }

    fn filter_fixture_occurrence(
        conn: &rusqlite::Connection,
        id: &str,
        automation_id: &str,
        scheduled_for: &str,
        state: &str,
    ) {
        conn.execute(
            "INSERT INTO automation_occurrences
                (id, automation_id, automation_revision, definition_digest, scheduled_for,
                 kind, state, attempt, created_at, updated_at)
             SELECT ?1, id, revision, definition_digest, ?3, 'scheduled', ?4, 0, ?3, ?3
             FROM automation_definitions WHERE id = ?2",
            rusqlite::params![id, automation_id, scheduled_for, state],
        )
        .unwrap();
    }

    #[test]
    fn occurrence_list_filters_every_view_by_automation_before_the_limit() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("store.sqlite");
        crate::store::initialize_store(&path).unwrap();
        let conn = crate::store::open_store(&path).unwrap();
        filter_fixture_definition(&conn, "alpha");
        filter_fixture_definition(&conn, "beta");
        // Three alpha rows sort before beta's, so a global page of one would
        // never contain beta: the filter must apply before the limit.
        for (index, hour) in ["06", "07", "08"].iter().enumerate() {
            filter_fixture_occurrence(
                &conn,
                &format!("alpha-due-{index}"),
                "alpha",
                &format!("2026-09-01T{hour}:00:00.000Z"),
                "planned",
            );
        }
        filter_fixture_occurrence(
            &conn,
            "beta-due",
            "beta",
            "2026-09-01T09:00:00.000Z",
            "planned",
        );
        filter_fixture_occurrence(
            &conn,
            "alpha-claimed",
            "alpha",
            "2026-09-02T09:00:00.000Z",
            "claimed",
        );
        filter_fixture_occurrence(
            &conn,
            "beta-claimed",
            "beta",
            "2026-09-03T09:00:00.000Z",
            "claimed",
        );

        let list = |view: &str, automation_id: Option<&str>, limit: u64| {
            let mut request = json!({
                "action": "coven.automations.occurrence.list.v1",
                "view": view,
                "limit": limit
            });
            if let Some(automation_id) = automation_id {
                request["automationId"] = json!(automation_id);
            }
            let (status, response) = route_action(request, &conn, &crate::api::NoopSessionRuntime);
            assert_eq!(status, 200, "{view} {automation_id:?}");
            response.event.as_ref().unwrap().payload["occurrences"]
                .as_array()
                .unwrap()
                .iter()
                .map(|occurrence| occurrence["id"].as_str().unwrap().to_owned())
                .collect::<Vec<_>>()
        };

        assert_eq!(list("due", None, 1), vec!["alpha-due-0"]);
        assert_eq!(list("due", Some("beta"), 1), vec!["beta-due"]);
        assert_eq!(
            list("due", Some("alpha"), 100),
            vec!["alpha-due-0", "alpha-due-1", "alpha-due-2"]
        );
        // Eligibility is latest-only per automation, and stays scoped to it.
        assert_eq!(list("eligible", Some("beta"), 1), Vec::<String>::new());
        assert_eq!(list("claimed", Some("beta"), 100), vec!["beta-claimed"]);
        assert_eq!(
            list("claimed", None, 100),
            vec!["alpha-claimed", "beta-claimed"]
        );
        assert_eq!(list("running", Some("alpha"), 100), Vec::<String>::new());
        assert_eq!(list("due", Some("missing"), 100), Vec::<String>::new());
    }

    #[test]
    fn occurrence_list_scopes_the_eligible_queue_to_one_automation() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("store.sqlite");
        crate::store::initialize_store(&path).unwrap();
        let conn = crate::store::open_store(&path).unwrap();
        filter_fixture_definition(&conn, "alpha");
        filter_fixture_definition(&conn, "beta");
        filter_fixture_occurrence(
            &conn,
            "alpha-old",
            "alpha",
            "2026-09-01T07:00:00.000Z",
            "planned",
        );
        filter_fixture_occurrence(
            &conn,
            "alpha-latest",
            "alpha",
            "2026-09-01T08:00:00.000Z",
            "planned",
        );
        filter_fixture_occurrence(
            &conn,
            "beta-latest",
            "beta",
            "2026-09-01T09:00:00.000Z",
            "planned",
        );

        let eligible = |automation_id: Option<&str>, limit: u64| {
            let mut request = json!({
                "action": "coven.automations.occurrence.list.v1",
                "view": "eligible",
                "limit": limit
            });
            if let Some(automation_id) = automation_id {
                request["automationId"] = json!(automation_id);
            }
            let (status, response) = route_action(request, &conn, &crate::api::NoopSessionRuntime);
            assert_eq!(status, 200);
            response.event.as_ref().unwrap().payload["occurrences"]
                .as_array()
                .unwrap()
                .iter()
                .map(|occurrence| occurrence["id"].as_str().unwrap().to_owned())
                .collect::<Vec<_>>()
        };

        assert_eq!(eligible(None, 100), vec!["alpha-latest", "beta-latest"]);
        assert_eq!(eligible(None, 1), vec!["alpha-latest"]);
        assert_eq!(eligible(Some("beta"), 1), vec!["beta-latest"]);
        assert_eq!(eligible(Some("alpha"), 100), vec!["alpha-latest"]);
    }

    #[test]
    fn occurrence_list_refuses_a_malformed_automation_filter() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("store.sqlite");
        crate::store::initialize_store(&path).unwrap();
        let conn = crate::store::open_store(&path).unwrap();
        for automation_id in [
            json!(""),
            json!("   "),
            json!(7),
            json!(null),
            json!(["alpha"]),
        ] {
            let (status, _) = route_action(
                json!({
                    "action": "coven.automations.occurrence.list.v1",
                    "view": "due",
                    "automationId": automation_id
                }),
                &conn,
                &crate::api::NoopSessionRuntime,
            );
            assert_eq!(status, 400, "automationId {automation_id}");
        }
    }

    fn history_page(
        conn: &rusqlite::Connection,
        automation_id: &str,
        limit: u64,
        cursor: Option<&str>,
    ) -> (Vec<String>, Value) {
        let mut request = json!({
            "action": "coven.automations.occurrence.history.v1",
            "automationId": automation_id,
            "limit": limit
        });
        if let Some(cursor) = cursor {
            request["cursor"] = json!(cursor);
        }
        let (status, response) = route_action(request, conn, &crate::api::NoopSessionRuntime);
        assert_eq!(status, 200);
        let payload = response.event.as_ref().unwrap().payload.clone();
        assert_eq!(payload["automationId"], automation_id);
        let ids = payload["occurrences"]
            .as_array()
            .unwrap()
            .iter()
            .map(|occurrence| occurrence["id"].as_str().unwrap().to_owned())
            .collect();
        (ids, payload["cursor"].clone())
    }

    #[test]
    fn occurrence_history_pages_every_state_newest_first_by_keyset() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("store.sqlite");
        crate::store::initialize_store(&path).unwrap();
        let conn = crate::store::open_store(&path).unwrap();
        filter_fixture_definition(&conn, "alpha");
        filter_fixture_definition(&conn, "beta");
        for (id, day, state) in [
            ("alpha-1", "01", "failed"),
            ("alpha-2", "02", "succeeded"),
            ("alpha-3", "03", "running"),
            ("alpha-4", "04", "claimed"),
            ("alpha-5", "05", "planned"),
        ] {
            filter_fixture_occurrence(
                &conn,
                id,
                "alpha",
                &format!("2026-09-{day}T09:00:00.000Z"),
                state,
            );
        }
        filter_fixture_occurrence(
            &conn,
            "beta-1",
            "beta",
            "2026-09-06T09:00:00.000Z",
            "planned",
        );

        let (first, cursor) = history_page(&conn, "alpha", 2, None);
        assert_eq!(first, vec!["alpha-5", "alpha-4"]);
        assert_eq!(cursor["hasMore"], true);
        let next = cursor["next"].as_str().unwrap().to_owned();

        // A row inserted above the cursor must not shift the next page.
        filter_fixture_occurrence(
            &conn,
            "alpha-6",
            "alpha",
            "2026-09-07T09:00:00.000Z",
            "planned",
        );
        let (second, cursor) = history_page(&conn, "alpha", 2, Some(&next));
        assert_eq!(second, vec!["alpha-3", "alpha-2"]);
        assert_eq!(cursor["current"], next.as_str());
        let next = cursor["next"].as_str().unwrap().to_owned();

        let (third, cursor) = history_page(&conn, "alpha", 2, Some(&next));
        assert_eq!(third, vec!["alpha-1"]);
        assert_eq!(cursor, json!({ "hasMore": false, "current": next }));

        let (whole, cursor) = history_page(&conn, "alpha", 100, None);
        assert_eq!(
            whole,
            vec!["alpha-6", "alpha-5", "alpha-4", "alpha-3", "alpha-2", "alpha-1"]
        );
        assert_eq!(cursor, json!({ "hasMore": false }));
        let (none, cursor) = history_page(&conn, "missing", 20, None);
        assert!(none.is_empty());
        assert_eq!(cursor, json!({ "hasMore": false }));
        assert!(capabilities()
            .capabilities
            .iter()
            .find(|capability| capability.id == "coven.automations")
            .unwrap()
            .actions
            .contains(&"coven.automations.occurrence.history.v1"));
    }

    #[test]
    fn occurrence_history_orders_mixed_precision_by_instant() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("store.sqlite");
        crate::store::initialize_store(&path).unwrap();
        let conn = crate::store::open_store(&path).unwrap();
        filter_fixture_definition(&conn, "alpha");
        // Manual rows store nanoseconds, scheduled rows milliseconds; raw TEXT
        // order would put `.123Z` above `.123100000Z`.
        for (id, scheduled_for) in [
            ("scheduled-ms", "2026-09-10T09:00:00.123Z"),
            ("manual-ns", "2026-09-10T09:00:00.123100000Z"),
            ("tie-a", "2026-09-09T09:00:00.500Z"),
            ("tie-b", "2026-09-09T09:00:00.500000000Z"),
            ("whole-second", "2026-09-08T09:00:00Z"),
        ] {
            filter_fixture_occurrence(&conn, id, "alpha", scheduled_for, "succeeded");
        }
        let expected = vec![
            "manual-ns",
            "scheduled-ms",
            "tie-b",
            "tie-a",
            "whole-second",
        ];
        let (whole, _) = history_page(&conn, "alpha", 100, None);
        assert_eq!(whole, expected);

        // One row per page walks across the equal-instant pair without a gap
        // or a repeat.
        let mut walked = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let (page, position) = history_page(&conn, "alpha", 1, cursor.as_deref());
            walked.extend(page);
            match position["next"].as_str() {
                Some(next) => cursor = Some(next.to_owned()),
                None => break,
            }
        }
        assert_eq!(walked, expected);
    }

    #[test]
    fn occurrence_history_refuses_malformed_requests() {
        use base64::Engine as _;
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("store.sqlite");
        crate::store::initialize_store(&path).unwrap();
        let conn = crate::store::open_store(&path).unwrap();
        let encode = |text: &str| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(text);
        let padded =
            base64::engine::general_purpose::URL_SAFE.encode(r#"["2026-09-01T09:00:00.000Z","x"]"#);
        let spaced = encode(r#"[ "2026-09-01T09:00:00.000Z", "x" ]"#);
        let bad_cursors = vec![
            json!(""),
            json!(7),
            json!("not base64!"),
            json!(padded),
            json!(spaced),
            json!(encode(r#"{"s":"2026","i":"x"}"#)),
            json!(encode(r#"["2026-09-01T09:00:00.000Z"]"#)),
            json!(encode(r#"["2026","x","y"]"#)),
            json!(encode(r#"["","x"]"#)),
            json!(encode(r#"["2026",""]"#)),
            json!(encode(r#"["2026",5]"#)),
            json!("A".repeat(OCCURRENCE_HISTORY_CURSOR_MAX_CHARS + 1)),
        ];
        for cursor in bad_cursors {
            let (status, _) = route_action(
                json!({
                    "action": "coven.automations.occurrence.history.v1",
                    "automationId": "alpha",
                    "cursor": cursor
                }),
                &conn,
                &crate::api::NoopSessionRuntime,
            );
            assert_eq!(status, 400, "cursor {cursor}");
        }
        for request in [
            json!({ "action": "coven.automations.occurrence.history.v1" }),
            json!({ "action": "coven.automations.occurrence.history.v1", "automationId": " " }),
            json!({ "action": "coven.automations.occurrence.history.v1", "automationId": 3 }),
            json!({ "action": "coven.automations.occurrence.history.v1", "automationId": "a", "limit": 0 }),
            json!({ "action": "coven.automations.occurrence.history.v1", "automationId": "a", "limit": 101 }),
        ] {
            let (status, _) = route_action(request.clone(), &conn, &crate::api::NoopSessionRuntime);
            assert_eq!(status, 400, "request {request}");
        }
    }

    #[test]
    fn run_get_action_reads_one_run_with_its_attempts() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("store.sqlite");
        crate::store::initialize_store(&path).unwrap();
        let conn = crate::store::open_store(&path).unwrap();
        filter_fixture_definition(&conn, "run-read");
        conn.execute_batch(
            "INSERT INTO automation_occurrences
                (id, automation_id, automation_revision, definition_digest, scheduled_for,
                 kind, state, attempt, created_at, updated_at)
             SELECT 'run-read-occurrence', id, revision, definition_digest,
                    '2026-09-01T09:00:00.000Z', 'scheduled', 'running', 4,
                    '2026-09-01T09:00:00.000Z', '2026-09-01T09:01:00.000Z'
             FROM automation_definitions WHERE id = 'run-read';
             INSERT INTO automation_runs
                (id, automation_id, automation_revision, definition_digest, occurrence_id,
                 authority_profile, receipt_id, runtime, status, started_at, timeout_at)
             SELECT 'run-read-run', automation_id, automation_revision, definition_digest, id,
                    'coven.automations.authority.v1', 'receipt-7', 'coven-code', 'running',
                    '2026-09-01T09:01:00.000Z', '2026-09-01T09:31:00.000Z'
             FROM automation_occurrences WHERE id = 'run-read-occurrence';
             INSERT INTO automation_attempts
                (id, run_id, occurrence_id, attempt_number, adoption_key,
                 occurrence_fence_generation, dispatch_generation, state, failure_class,
                 prior_attempt_number, prior_disposition, retry_classification, not_before,
                 opened_at, settled_at)
             VALUES
                ('run-read-attempt-2', 'run-read-run', 'run-read-occurrence', 2, 'adopt-2',
                 4, 9, 'observing', NULL, 1, 'failed', 'automatic_retry',
                 '2026-09-01T09:01:30.000Z', '2026-09-01T09:02:00.000Z', NULL),
                ('run-read-attempt-1', 'run-read-run', 'run-read-occurrence', 1, 'adopt-1',
                 3, 8, 'failed', 'runtime_error', NULL, NULL, 'initial',
                 '2026-09-01T09:01:00.000Z', '2026-09-01T09:01:00.000Z',
                 '2026-09-01T09:01:20.000Z');",
        )
        .unwrap();

        let (status, response) = route_action(
            json!({ "action": "coven.automations.run.get.v1", "id": "run-read-run" }),
            &conn,
            &crate::api::NoopSessionRuntime,
        );
        assert_eq!(status, 200);
        let run = response.event.as_ref().unwrap().payload["run"].clone();
        assert_eq!(run["id"], "run-read-run");
        assert_eq!(run["automationId"], "run-read");
        assert_eq!(run["occurrenceId"], "run-read-occurrence");
        assert_eq!(run["authorityProfile"], "coven.automations.authority.v1");
        assert_eq!(run["receiptId"], "receipt-7");
        assert_eq!(run["status"], "running");
        let attempts = run["attempts"].as_array().unwrap();
        assert_eq!(
            attempts
                .iter()
                .map(|attempt| attempt["attemptNumber"].clone())
                .collect::<Vec<_>>(),
            vec![json!(1), json!(2)]
        );

        // The detail read and the occurrence detail project a run identically.
        let (_, occurrence) = route_action(
            json!({ "action": "coven.automations.occurrence.get.v1", "id": "run-read-occurrence" }),
            &conn,
            &crate::api::NoopSessionRuntime,
        );
        assert_eq!(
            occurrence.event.as_ref().unwrap().payload["occurrence"]["runs"][0],
            run
        );

        let (absent_status, absent) = route_action(
            json!({ "action": "coven.automations.run.get.v1", "id": "no-such-run" }),
            &conn,
            &crate::api::NoopSessionRuntime,
        );
        assert_eq!(absent_status, 200);
        assert_eq!(
            absent.event.as_ref().unwrap().payload,
            json!({ "run": null })
        );

        for request in [
            json!({ "action": "coven.automations.run.get.v1" }),
            json!({ "action": "coven.automations.run.get.v1", "id": "  " }),
            json!({ "action": "coven.automations.run.get.v1", "id": 7 }),
        ] {
            let (status, _) = route_action(request, &conn, &crate::api::NoopSessionRuntime);
            assert_eq!(status, 400);
        }
        assert!(capabilities()
            .capabilities
            .iter()
            .find(|capability| capability.id == "coven.automations")
            .unwrap()
            .actions
            .contains(&"coven.automations.run.get.v1"));
    }

    #[test]
    fn scheduler_status_reports_the_oldest_actually_eligible_occurrence() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("store.sqlite");
        crate::store::initialize_store(&path).unwrap();
        let conn = crate::store::open_store(&path).unwrap();
        for (id, status) in [("paused-oldest", "PAUSED"), ("active-next", "ACTIVE")] {
            let definition = crate::automations::RoutineDefinition::from_json(&json!({
                "schemaVersion": 1,
                "id": id,
                "name": id,
                "status": status,
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
        }
        conn.execute_batch(
            "INSERT INTO automation_occurrences
                (id, automation_id, automation_revision, definition_digest, scheduled_for,
                 kind, state, attempt, created_at, updated_at)
             SELECT 'paused-occurrence', id, revision, definition_digest,
                    '2026-09-01T08:00:00.000Z', 'scheduled', 'planned', 0,
                    '2026-09-01T08:00:00.000Z', '2026-09-01T08:00:00.000Z'
             FROM automation_definitions WHERE id = 'paused-oldest';
             INSERT INTO automation_occurrences
                (id, automation_id, automation_revision, definition_digest, scheduled_for,
                 kind, state, attempt, created_at, updated_at)
             SELECT 'active-occurrence', id, revision, definition_digest,
                    '2026-09-01T09:00:00.000Z', 'scheduled', 'planned', 0,
                    '2026-09-01T09:00:00.000Z', '2026-09-01T09:00:00.000Z'
             FROM automation_definitions WHERE id = 'active-next';
             INSERT INTO automation_occurrences
                (id, automation_id, automation_revision, definition_digest, scheduled_for,
                 kind, state, attempt, created_at, updated_at)
             SELECT 'active-superseded-occurrence', id, revision, definition_digest,
                    '2026-09-01T08:30:00.000Z', 'scheduled', 'planned', 0,
                    '2026-09-01T08:30:00.000Z', '2026-09-01T08:30:00.000Z'
             FROM automation_definitions WHERE id = 'active-next';",
        )
        .unwrap();

        let payload = automation_scheduler_status_payload(
            &conn,
            chrono::Utc.with_ymd_and_hms(2026, 9, 1, 10, 0, 0).unwrap(),
        )
        .unwrap();

        assert_eq!(payload["scheduler"]["queue"]["planned"], 3);
        assert_eq!(
            payload["scheduler"]["queue"]["oldestEligibleAt"],
            "2026-09-01T09:00:00.000Z"
        );
        assert_eq!(
            payload["scheduler"]["queue"]["oldestEligibleAgeMs"],
            3_600_000
        );
    }

    #[test]
    fn scheduler_status_keeps_a_ready_retry_eligible_while_paused() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("store.sqlite");
        crate::store::initialize_store(&path).unwrap();
        let conn = crate::store::open_store(&path).unwrap();
        let definition = crate::automations::RoutineDefinition::from_json(&json!({
            "schemaVersion": 1,
            "id": "paused-retry",
            "name": "paused-retry",
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
        let (run_status, run_response) = route_action(
            json!({"action": "coven.automations.run", "id": "paused-retry"}),
            &conn,
            &RetryableRejectedRuntime,
        );
        assert_eq!(run_status, 200);
        assert_eq!(
            run_response.event.as_ref().unwrap().payload["status"],
            "retry_scheduled"
        );
        conn.execute(
            "UPDATE automation_definitions
             SET status = 'PAUSED'
             WHERE id = 'paused-retry'",
            [],
        )
        .unwrap();
        let scheduled_for: String = conn
            .query_row(
                "SELECT scheduled_for
                 FROM automation_occurrences
                 WHERE automation_id = 'paused-retry'",
                [],
                |row| row.get(0),
            )
            .unwrap();

        let payload = automation_scheduler_status_payload(
            &conn,
            chrono::Utc::now() + chrono::Duration::minutes(1),
        )
        .unwrap();

        assert_eq!(
            payload["scheduler"]["queue"]["oldestEligibleAt"],
            scheduled_for
        );
    }

    #[test]
    fn scheduler_status_excludes_invalid_active_definitions_from_eligibility() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("store.sqlite");
        crate::store::initialize_store(&path).unwrap();
        let conn = crate::store::open_store(&path).unwrap();
        let definition = crate::automations::RoutineDefinition::from_json(&json!({
            "schemaVersion": 1,
            "id": "invalid-active",
            "name": "invalid-active",
            "status": "ACTIVE",
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
        conn.execute(
            "UPDATE automation_definitions
             SET definition_json = '{}', lifecycle_state = 'invalid'
             WHERE id = 'invalid-active'",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO automation_occurrences
                (id, automation_id, automation_revision, definition_digest, scheduled_for,
                 kind, state, attempt, created_at, updated_at)
             SELECT 'invalid-active-occurrence', id, revision, definition_digest,
                    '2026-09-01T09:00:00.000Z', 'scheduled', 'planned', 0,
                    '2026-09-01T09:00:00.000Z', '2026-09-01T09:00:00.000Z'
             FROM automation_definitions WHERE id = 'invalid-active'",
            [],
        )
        .unwrap();

        let payload = automation_scheduler_status_payload(
            &conn,
            chrono::Utc.with_ymd_and_hms(2026, 9, 1, 10, 0, 0).unwrap(),
        )
        .unwrap();

        assert!(payload["scheduler"]["queue"]["oldestEligibleAt"].is_null());
    }

    #[test]
    fn scheduler_status_action_exposes_the_last_completed_pass() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path();
        let path = home.join("coven.sqlite3");
        crate::store::initialize_store(&path).unwrap();
        let handle = crate::automations::daemon_tick::start_automations_scheduler(
            home,
            std::sync::Arc::new(crate::api::NoopSessionRuntime),
        )
        .unwrap();
        let conn = crate::store::open_store(&path).unwrap();
        let started = std::time::Instant::now();
        let last_pass = loop {
            let (status, response) = route_action(
                json!({"action": "coven.automations.scheduler.status.v1"}),
                &conn,
                &crate::api::NoopSessionRuntime,
            );
            assert_eq!(status, 200);
            let last_pass =
                response.event.as_ref().unwrap().payload["scheduler"]["lastPass"].clone();
            if last_pass.is_object() {
                break last_pass;
            }
            assert!(
                started.elapsed() < std::time::Duration::from_secs(5),
                "scheduler pass status was not published after {:?}",
                started.elapsed()
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        };

        assert_eq!(last_pass["generation"], 1);
        assert_eq!(last_pass["trigger"], "startup");
        assert!(last_pass["scheduledAt"].is_string());
        assert!(last_pass["startedAt"].is_string());
        assert!(last_pass["startLagMs"].as_u64().is_some());
        assert!(last_pass["finishedAt"].is_string());
        assert!(last_pass["durationMs"].as_u64().is_some());
        assert_eq!(last_pass["status"], "succeeded");
        assert!(last_pass["errorClass"].is_null());
        assert_eq!(last_pass["planned"], 0);
        assert_eq!(last_pass["recovered"], 0);
        assert_eq!(last_pass["claimed"], 0);
        assert_eq!(last_pass["dispatched"], 0);
        assert_eq!(last_pass["failures"], 0);
        let (_, active_response) = route_action(
            json!({"action": "coven.automations.scheduler.status.v1"}),
            &conn,
            &crate::api::NoopSessionRuntime,
        );
        assert_eq!(
            active_response.event.as_ref().unwrap().payload["scheduler"]["authorityAssigned"],
            true
        );

        handle.request_shutdown();
        handle
            .finish_shutdown(std::time::Instant::now() + std::time::Duration::from_secs(5))
            .unwrap();
    }

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

    fn run_history_fixture_run(
        conn: &rusqlite::Connection,
        id: &str,
        automation_id: &str,
        occurrence_id: Option<&str>,
        started_at: &str,
    ) {
        conn.execute(
            "INSERT INTO automation_runs
                (id, automation_id, occurrence_id, runtime, status, started_at)
             VALUES (?1, ?2, ?3, 'coven-code', 'succeeded', ?4)",
            rusqlite::params![id, automation_id, occurrence_id, started_at],
        )
        .unwrap();
    }

    fn run_history_page(conn: &rusqlite::Connection, request: Value) -> (u16, Vec<String>, Value) {
        let mut request = request;
        request["action"] = json!("coven.automations.run.history.v1");
        let (status, response) = route_action(request, conn, &crate::api::NoopSessionRuntime);
        let Some(event) = response.event else {
            return (status, Vec::new(), response.error.unwrap_or_default());
        };
        let ids = event.payload["runs"]
            .as_array()
            .unwrap()
            .iter()
            .map(|run| run["id"].as_str().unwrap().to_owned())
            .collect();
        (status, ids, event.payload)
    }

    #[test]
    fn run_history_pages_newest_first_by_instant_with_an_occurrence_filter() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("store.sqlite");
        crate::store::initialize_store(&path).unwrap();
        let conn = crate::store::open_store(&path).unwrap();
        filter_fixture_definition(&conn, "alpha");
        filter_fixture_definition(&conn, "beta");
        filter_fixture_occurrence(
            &conn,
            "occ-1",
            "alpha",
            "2026-09-01T09:00:00.000Z",
            "succeeded",
        );
        filter_fixture_occurrence(
            &conn,
            "occ-2",
            "alpha",
            "2026-09-02T09:00:00.000Z",
            "succeeded",
        );
        // Millisecond and nanosecond start times, an equal-instant pair broken
        // by id, a whole-second value, a run without an occurrence, and a run
        // of another automation.
        for (id, automation_id, occurrence_id, started_at) in [
            ("run-a", "alpha", Some("occ-1"), "2026-09-01T09:00:00.100Z"),
            (
                "run-b",
                "alpha",
                Some("occ-1"),
                "2026-09-01T09:00:00.100500000Z",
            ),
            ("run-c", "alpha", Some("occ-2"), "2026-09-02T09:00:00.000Z"),
            (
                "run-d",
                "alpha",
                Some("occ-2"),
                "2026-09-02T09:00:00.000000000Z",
            ),
            ("run-e", "alpha", None, "2026-09-03T09:00:00Z"),
            ("run-x", "beta", None, "2026-09-04T09:00:00.000Z"),
        ] {
            run_history_fixture_run(&conn, id, automation_id, occurrence_id, started_at);
        }
        conn.execute(
            "INSERT INTO automation_attempts
                (id, run_id, occurrence_id, attempt_number, adoption_key,
                 occurrence_fence_generation, dispatch_generation, state,
                 retry_classification, not_before, opened_at)
             VALUES ('attempt-c', 'run-c', 'occ-2', 1, 'adopt:attempt-c', 1, 0,
                     'succeeded', 'initial', '2026-09-02T09:00:00.000Z',
                     '2026-09-02T09:00:00.000Z')",
            [],
        )
        .unwrap();

        let (status, whole, payload) = run_history_page(&conn, json!({ "automationId": "alpha" }));
        assert_eq!(status, 200, "{payload}");
        assert_eq!(whole, vec!["run-e", "run-d", "run-c", "run-b", "run-a"]);
        assert_eq!(payload["cursor"], json!({ "hasMore": false }));
        assert_eq!(payload["runs"][2]["attempts"][0]["id"], "attempt-c");

        // One run per page, with a newer run added mid-walk: every page echoes
        // its cursor, and the walk neither skips nor repeats.
        let mut walked = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let mut request = json!({ "automationId": "alpha", "limit": 1 });
            if let Some(cursor) = &cursor {
                request["cursor"] = json!(cursor);
            }
            let (status, page, payload) = run_history_page(&conn, request);
            assert_eq!(status, 200, "{payload}");
            assert_eq!(payload["cursor"]["current"].as_str(), cursor.as_deref());
            walked.extend(page);
            if walked.len() == 2 {
                run_history_fixture_run(&conn, "run-z", "alpha", None, "2026-09-05T09:00:00.000Z");
            }
            match payload["cursor"]["next"].as_str() {
                Some(next) => cursor = Some(next.to_owned()),
                None => break,
            }
        }
        assert_eq!(walked, vec!["run-e", "run-d", "run-c", "run-b", "run-a"]);

        let (_, filtered, payload) = run_history_page(
            &conn,
            json!({ "automationId": "alpha", "occurrenceId": "occ-1" }),
        );
        assert_eq!(filtered, vec!["run-b", "run-a"]);
        assert_eq!(payload["occurrenceId"], "occ-1");
    }

    #[test]
    fn run_history_refuses_malformed_requests_with_typed_errors() {
        use base64::Engine as _;
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("store.sqlite");
        crate::store::initialize_store(&path).unwrap();
        let conn = crate::store::open_store(&path).unwrap();
        let long_position = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(json!(["2026", "x".repeat(200)]).to_string());
        assert!(long_position.len() > RUN_HISTORY_CURSOR_MAX_CHARS);
        for request in [
            json!({}),
            json!({ "automationId": "" }),
            json!({ "automationId": "alpha", "occurrenceId": "" }),
            json!({ "automationId": "alpha", "occurrenceId": 7 }),
            json!({ "automationId": "alpha", "limit": 0 }),
            json!({ "automationId": "alpha", "limit": 101 }),
            json!({ "automationId": "alpha", "cursor": "not base64!" }),
            json!({ "automationId": "alpha", "cursor": long_position }),
        ] {
            let (status, runs, error) = run_history_page(&conn, request.clone());
            assert_eq!(status, 400, "{request}");
            assert!(runs.is_empty());
            assert_eq!(error["code"], "VALIDATION_FAILED", "{request}");
        }
        let (status, runs, payload) = run_history_page(&conn, json!({ "automationId": "absent" }));
        assert_eq!(status, 200);
        assert!(runs.is_empty());
        assert_eq!(payload["cursor"], json!({ "hasMore": false }));
    }

    #[test]
    fn legacy_import_v1_rejects_unknown_sources_and_malformed_flags_durably() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("store.sqlite");
        crate::store::initialize_store(&path).unwrap();
        let conn = crate::store::open_store(&path).unwrap();
        for (key, request) in [
            ("adopt:import:no-source", json!({})),
            ("adopt:import:other-source", json!({ "source": "cron" })),
            (
                "adopt:import:bad-dry-run",
                json!({ "source": "codex-automation-toml", "dryRun": "yes" }),
            ),
        ] {
            let mut request = request;
            request["action"] = json!("coven.automations.legacy.import.v1");
            request["adoptionKey"] = json!(key);
            let (status, response) =
                route_action(request.clone(), &conn, &crate::api::NoopSessionRuntime);
            assert_eq!(status, 400, "{request}");
            assert_eq!(
                response.error.unwrap()["code"],
                "VALIDATION_FAILED",
                "{request}"
            );
            // The rejection is adopted: the exact request replays it.
            let (_, replay) = route_action(request, &conn, &crate::api::NoopSessionRuntime);
            assert_eq!(replay.error.unwrap()["code"], "VALIDATION_FAILED");
        }
        let (status, response) = route_action(
            json!({ "action": "coven.automations.legacy.import.v1", "source": "codex-automation-toml" }),
            &conn,
            &crate::api::NoopSessionRuntime,
        );
        assert_eq!(status, 400);
        assert_eq!(response.error.unwrap()["code"], "VALIDATION_FAILED");
        let definitions: i64 = conn
            .query_row("SELECT COUNT(*) FROM automation_definitions", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(definitions, 0);
    }

    #[test]
    fn definition_health_v1_types_absence_and_tombstones() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("store.sqlite");
        crate::store::initialize_store(&path).unwrap();
        let conn = crate::store::open_store(&path).unwrap();
        filter_fixture_definition(&conn, "healthy");
        let route = |request: Value| route_action(request, &conn, &crate::api::NoopSessionRuntime);
        let code = |response: &ControlActionResponse| {
            response.error.as_ref().unwrap()["code"]
                .as_str()
                .unwrap()
                .to_owned()
        };

        let (status, response) = route(json!({
            "action": "coven.automations.definition.health.v1",
            "id": "healthy",
        }));
        assert_eq!(status, 200, "{response:?}");
        let legacy = route(json!({ "action": "coven.automations.health", "id": "healthy" })).1;
        let health = response.event.unwrap().payload;
        assert_eq!(health["health"]["automationId"], "healthy");
        assert_eq!(health, legacy.event.unwrap().payload);

        let (status, response) = route(json!({
            "action": "coven.automations.definition.health.v1",
            "id": "absent",
        }));
        assert_eq!((status, code(&response).as_str()), (404, "NOT_FOUND"));

        let (status, response) = route(json!({
            "action": "coven.automations.definition.health.v1",
        }));
        assert_eq!(
            (status, code(&response).as_str()),
            (400, "VALIDATION_FAILED")
        );

        let (status, _) = route(json!({
            "action": "coven.automations.definition.tombstone.v1",
            "adoptionKey": "adopt:tombstone:healthy",
            "id": "healthy",
            "expectedRevision": 1,
        }));
        assert_eq!(status, 200);
        let (status, response) = route(json!({
            "action": "coven.automations.definition.health.v1",
            "id": "healthy",
        }));
        assert_eq!((status, code(&response).as_str()), (410, "GONE_TOMBSTONED"));
    }

    #[test]
    fn definition_activate_and_pause_route_over_the_flat_wire() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("store.sqlite");
        crate::store::initialize_store(&path).unwrap();
        let conn = crate::store::open_store(&path).unwrap();
        let route = |request: Value| route_action(request, &conn, &crate::api::NoopSessionRuntime);
        let (status, _) = route(json!({
            "action": "coven.automations.definition.create.v1",
            "adoptionKey": "adopt:create:wire-switch",
            "definition": {
                "schemaVersion": 1,
                "id": "wire-switch",
                "name": "wire-switch",
                "status": "PAUSED",
                "rrule": "FREQ=DAILY;BYHOUR=9",
                "timezone": "utc",
                "misfire": "latest",
                "overlap": "forbid",
                "timeoutMinutes": 30,
                "runtime": "coven-code",
                "prompt": "Do the thing."
            },
        }));
        assert_eq!(status, 200);

        let (status, response) = route(json!({
            "action": "coven.automations.definition.activate.v1",
            "adoptionKey": "adopt:activate:wire-switch",
            "id": "wire-switch",
            "expectedRevision": 1,
        }));
        assert_eq!(status, 200, "{response:?}");
        let event = response.event.unwrap();
        assert_eq!(event.payload["outcome"], "committed");
        assert_eq!(event.payload["revision"], 2);
        assert_eq!(event.payload["result"]["status"], "ACTIVE");

        let (status, response) = route(json!({
            "action": "coven.automations.definition.pause.v1",
            "adoptionKey": "adopt:pause:wire-switch",
            "id": "wire-switch",
            "expectedRevision": 2,
            "reason": "Holiday freeze.",
        }));
        assert_eq!(status, 200, "{response:?}");
        assert_eq!(
            response.event.unwrap().payload["result"]["status"],
            "PAUSED"
        );

        // Missing expectedRevision is a durable typed validation rejection.
        let (status, response) = route(json!({
            "action": "coven.automations.definition.activate.v1",
            "adoptionKey": "adopt:activate:wire-switch:no-revision",
            "id": "wire-switch",
        }));
        assert_eq!(status, 400);
        assert_eq!(response.error.unwrap()["code"], "VALIDATION_FAILED");
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
