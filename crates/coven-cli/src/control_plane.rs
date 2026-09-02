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
                    "coven.automations.import",
                    "coven.automations.health",
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
            automation_result(action, origin, intent_id, automation_list_payload(conn))
        }
        "coven.automations.get" => {
            let id = required_id_field(&payload, action);
            match id {
                Ok(id) => {
                    automation_result(action, origin, intent_id, automation_get_payload(conn, &id))
                }
                Err(error) => (400, rejected_action(action, error)),
            }
        }
        "coven.automations.create" => {
            let definition = required_definition_field(&payload, action);
            match definition {
                Ok(definition) => automation_result(
                    action,
                    origin,
                    intent_id,
                    automation_create_payload(conn, &definition),
                ),
                Err(error) => (400, rejected_action(action, error)),
            }
        }
        "coven.automations.update" => {
            let definition = required_definition_field(&payload, action);
            match definition {
                Ok(definition) => automation_result(
                    action,
                    origin,
                    intent_id,
                    automation_update_payload(conn, &definition),
                ),
                Err(error) => (400, rejected_action(action, error)),
            }
        }
        "coven.automations.delete" => {
            let id = required_id_field(&payload, action);
            match id {
                Ok(id) => automation_result(
                    action,
                    origin,
                    intent_id,
                    automation_delete_payload(conn, &id),
                ),
                Err(error) => (400, rejected_action(action, error)),
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
            }
        })),
        Err(error) => Err(format!("{error:#}")),
    }
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
                Ok(outcome) if outcome.status == "running" => Ok(json!({
                    "runId": outcome.run_id,
                    "status": outcome.status,
                    "sessionId": outcome.session_id,
                    "error": outcome.error,
                })),
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
            Ok(json!({ "runs": runs }))
        }
        Err(error) => Err(format!("{error:#}")),
    }
}

fn automation_list_payload(conn: &rusqlite::Connection) -> Result<Value, String> {
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

fn automation_get_payload(conn: &rusqlite::Connection, id: &str) -> Result<Value, String> {
    match crate::automations::store::get_definition(conn, id) {
        Ok(Some(record)) => match serde_json::from_str::<Value>(&record.definition_json) {
            Ok(routine) => Ok(json!({ "routine": routine })),
            Err(error) => Err(format!("stored routine is unreadable: {error}")),
        },
        Ok(None) => Ok(json!({ "routine": Value::Null })),
        Err(error) => Err(format!("{error:#}")),
    }
}

fn automation_create_payload(
    conn: &rusqlite::Connection,
    definition: &crate::automations::RoutineDefinition,
) -> Result<Value, String> {
    match crate::automations::store::insert_definition(conn, definition) {
        Ok(record) => Ok(json!({
            "routine": definition.to_json(),
            "createdAt": record.created_at,
        })),
        Err(error) => Err(format!("{error:#}")),
    }
}

fn automation_update_payload(
    conn: &rusqlite::Connection,
    definition: &crate::automations::RoutineDefinition,
) -> Result<Value, String> {
    match crate::automations::store::update_definition(conn, definition) {
        Ok(Some(record)) => Ok(json!({
            "routine": definition.to_json(),
            "updatedAt": record.updated_at,
        })),
        Ok(None) => Err(format!("no routine with id `{}`", definition.id)),
        Err(error) => Err(format!("{error:#}")),
    }
}

fn automation_delete_payload(conn: &rusqlite::Connection, id: &str) -> Result<Value, String> {
    match crate::automations::store::delete_definition(conn, id) {
        Ok(deleted) => Ok(json!({ "id": id, "deleted": deleted })),
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
        event: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::{SessionLaunch, SessionRuntime};

    struct OwnershipThenErrorRuntime;
    struct RejectedRuntime;

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
}
