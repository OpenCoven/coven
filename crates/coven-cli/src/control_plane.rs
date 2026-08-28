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
                    "coven.automations.tick",
                    "coven.automations.runs",
                    "coven.automations.run",
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
                    event: Some(event),
                },
            )
        }
        "coven.automations.list" => {
            let event = automation_event(action, origin, intent_id, automation_list_payload(conn));
            (200, event)
        }
        "coven.automations.get" => {
            let id = required_id_field(&payload, action);
            let event = match id {
                Ok(id) => {
                    automation_event(action, origin, intent_id, automation_get_payload(conn, &id))
                }
                Err(error) => return (400, rejected_action(action, error)),
            };
            (200, event)
        }
        "coven.automations.create" => {
            let definition = required_definition_field(&payload, action);
            let event = match definition {
                Ok(definition) => automation_event(
                    action,
                    origin,
                    intent_id,
                    automation_create_payload(conn, &definition),
                ),
                Err(error) => return (400, rejected_action(action, error)),
            };
            (200, event)
        }
        "coven.automations.update" => {
            let definition = required_definition_field(&payload, action);
            let event = match definition {
                Ok(definition) => automation_event(
                    action,
                    origin,
                    intent_id,
                    automation_update_payload(conn, &definition),
                ),
                Err(error) => return (400, rejected_action(action, error)),
            };
            (200, event)
        }
        "coven.automations.delete" => {
            let id = required_id_field(&payload, action);
            let event = match id {
                Ok(id) => automation_event(
                    action,
                    origin,
                    intent_id,
                    automation_delete_payload(conn, &id),
                ),
                Err(error) => return (400, rejected_action(action, error)),
            };
            (200, event)
        }
        "coven.automations.tick" => {
            let now = chrono::Utc::now();
            let event = automation_event(
                action,
                origin,
                intent_id,
                automation_tick_payload(conn, now),
            );
            (200, event)
        }
        "coven.automations.runs" => {
            let id = required_id_field(&payload, action);
            let limit = payload.get("limit").and_then(Value::as_i64).unwrap_or(20);
            let event = match id {
                Ok(id) => automation_event(
                    action,
                    origin,
                    intent_id,
                    automation_runs_payload(conn, &id, limit),
                ),
                Err(error) => return (400, rejected_action(action, error)),
            };
            (200, event)
        }
        "coven.automations.run" => {
            let id = required_id_field(&payload, action);
            let now = chrono::Utc::now();
            let event = match id {
                Ok(id) => automation_event(
                    action,
                    origin,
                    intent_id,
                    automation_run_payload(conn, runtime, &id, now),
                ),
                Err(error) => return (400, rejected_action(action, error)),
            };
            (200, event)
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
        event: Some(ControlEvent {
            kind: "automations.changed",
            action: action.to_string(),
            origin,
            intent_id,
            payload,
        }),
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

fn required_definition_field(
    payload: &Value,
    action: &str,
) -> Result<crate::automations::RoutineDefinition, String> {
    let Some(definition) = payload.get("definition") else {
        return Err(format!("{action} requires object field `definition`"));
    };
    crate::automations::RoutineDefinition::from_json(definition)
        .map_err(|error| format!("{action}: {error}"))
}

fn automation_tick_payload(
    conn: &rusqlite::Connection,
    now: chrono::DateTime<chrono::Utc>,
) -> Value {
    match crate::automations::occurrences::tick(conn, now) {
        Ok(report) => json!({
            "planned": report.planned,
            "alreadyFenced": report.already_fenced,
            "pausedSkipped": report.paused_skipped,
            "recovered": report.recovered,
            "claimed": report.claimed,
            "failed": report.failed,
        }),
        Err(error) => json!({ "error": format!("{error:#}") }),
    }
}

fn automation_run_payload(
    conn: &rusqlite::Connection,
    runtime: &dyn crate::api::SessionRuntime,
    id: &str,
    now: chrono::DateTime<chrono::Utc>,
) -> Value {
    match crate::automations::runner::load_definition_for_run(conn, id) {
        Ok(Some(definition)) => {
            match crate::automations::runner::run_routine_now(conn, runtime, &definition, now) {
                Ok(outcome) => json!({
                    "runId": outcome.run_id,
                    "status": outcome.status,
                    "sessionId": outcome.session_id,
                    "error": outcome.error,
                }),
                Err(error) => json!({ "error": error }),
            }
        }
        Ok(None) => json!({ "error": format!("no routine with id `{id}`") }),
        Err(error) => json!({ "error": error }),
    }
}

fn automation_runs_payload(conn: &rusqlite::Connection, id: &str, limit: i64) -> Value {
    match crate::automations::runs::list_runs(conn, id, limit) {
        Ok(records) => {
            let runs: Vec<Value> = records
                .iter()
                .map(|record| {
                    json!({
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
                    })
                })
                .collect();
            json!({ "runs": runs })
        }
        Err(error) => json!({ "error": format!("{error:#}") }),
    }
}

fn automation_list_payload(conn: &rusqlite::Connection) -> Value {
    let records = crate::automations::store::list_definitions(conn);
    match records {
        Ok(records) => {
            let routines: Vec<Value> = records
                .iter()
                .filter_map(|record| serde_json::from_str::<Value>(&record.definition_json).ok())
                .collect();
            json!({ "routines": routines })
        }
        Err(error) => json!({ "error": format!("{error:#}") }),
    }
}

fn automation_get_payload(conn: &rusqlite::Connection, id: &str) -> Value {
    match crate::automations::store::get_definition(conn, id) {
        Ok(Some(record)) => match serde_json::from_str::<Value>(&record.definition_json) {
            Ok(routine) => json!({ "routine": routine }),
            Err(error) => json!({ "error": format!("stored routine is unreadable: {error}") }),
        },
        Ok(None) => json!({ "routine": Value::Null }),
        Err(error) => json!({ "error": format!("{error:#}") }),
    }
}

fn automation_create_payload(
    conn: &rusqlite::Connection,
    definition: &crate::automations::RoutineDefinition,
) -> Value {
    match crate::automations::store::insert_definition(conn, definition) {
        Ok(record) => json!({
            "routine": definition.to_json(),
            "createdAt": record.created_at,
        }),
        Err(error) => json!({ "error": format!("{error:#}") }),
    }
}

fn automation_update_payload(
    conn: &rusqlite::Connection,
    definition: &crate::automations::RoutineDefinition,
) -> Value {
    match crate::automations::store::update_definition(conn, definition) {
        Ok(Some(record)) => json!({
            "routine": definition.to_json(),
            "updatedAt": record.updated_at,
        }),
        Ok(None) => json!({ "error": format!("no routine with id `{}`", definition.id) }),
        Err(error) => json!({ "error": format!("{error:#}") }),
    }
}

fn automation_delete_payload(conn: &rusqlite::Connection, id: &str) -> Value {
    match crate::automations::store::delete_definition(conn, id) {
        Ok(deleted) => json!({ "id": id, "deleted": deleted }),
        Err(error) => json!({ "error": format!("{error:#}") }),
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
        event: None,
    }
}
