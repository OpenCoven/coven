//! Owner-local, read-only operator views over the daemon's control actions.
//! Keep diagnostics separate from raw run records, which can contain logs.

use anyhow::{anyhow, bail, Result};
use coven_client::{DaemonClient, DaemonEndpoint};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Clone, Copy, Debug)]
pub(crate) enum View<'a> {
    Status,
    Occurrence(&'a str),
    Run(&'a str),
    Attempts(&'a str),
}

impl View<'_> {
    fn action(self) -> &'static str {
        match self {
            Self::Status => "coven.automations.scheduler.status.v1",
            Self::Occurrence(_) => "coven.automations.occurrence.get.v1",
            Self::Run(_) | Self::Attempts(_) => "coven.automations.run.get.v1",
        }
    }

    fn request(self) -> Result<Value> {
        let mut request = json!({"action": self.action()});
        if let Self::Occurrence(id) | Self::Run(id) | Self::Attempts(id) = self {
            if id.is_empty()
                || id.len() > 512
                || id.chars().any(char::is_control)
                || id.trim() != id
            {
                bail!("automation inspection id is invalid");
            }
            request["id"] = json!(id);
        }
        Ok(request)
    }
}

pub(crate) fn run(view: View<'_>, as_json: bool) -> Result<()> {
    let request = view.request()?;
    let home = crate::paths::coven_home_dir()
        .map_err(|_| anyhow!("cannot resolve Coven home for automation inspection"))?;
    let endpoint = DaemonEndpoint::discover(home).map_err(|_| {
        anyhow!("owner-local Coven daemon unavailable; check `coven daemon status`")
    })?;
    if !endpoint.is_owner_local() {
        bail!("automation inspection requires owner-local daemon IPC");
    }
    let response = DaemonClient::new(endpoint)
        .request_json("POST", "/api/v1/actions", Some(&request))
        .map_err(|_| anyhow!("automation inspection could not reach a compatible owner-local daemon; check `coven daemon status`"))?;
    // Never propagate daemon error text: it can include database paths or logs.
    if response.status != 200 {
        bail!(
            "automation inspection was refused by the daemon (HTTP {})",
            response.status
        );
    }
    let response: Value = serde_json::from_slice(&response.body)
        .map_err(|_| anyhow!("automation inspection response is invalid"))?;
    let output = project(view, &response)?;
    if !as_json {
        println!("Automations inspection (owner-local, read-only)");
    }
    // JSON escaping also prevents persisted ids from injecting terminal controls.
    println!("{}", serde_json::to_string_pretty(&output)?);
    Ok(())
}

fn project(view: View<'_>, response: &Value) -> Result<Value> {
    let invalid = || anyhow!("automation inspection response is invalid");
    if response["ok"] != true
        || response["accepted"] != true
        || response["status"] != "completed"
        || response["action"] != view.action()
        || response["event"]["action"] != view.action()
    {
        return Err(invalid());
    }
    let payload = &response["event"]["payload"];
    let mut output = json!({"schemaVersion": "coven.automations.inspection.v1"});
    match view {
        View::Status => {
            let status: Scheduler =
                serde_json::from_value(payload["scheduler"].clone()).map_err(|_| invalid())?;
            output["scheduler"] = serde_json::to_value(status)?;
        }
        View::Occurrence(id) => {
            if payload.get("occurrence") == Some(&Value::Null) {
                bail!("automation occurrence was not found");
            }
            let occurrence: Occurrence =
                serde_json::from_value(payload["occurrence"].clone()).map_err(|_| invalid())?;
            if occurrence.id != id {
                return Err(invalid());
            }
            output["occurrence"] = serde_json::to_value(occurrence)?;
        }
        View::Run(id) | View::Attempts(id) => {
            if payload.get("run") == Some(&Value::Null) {
                bail!("automation run was not found");
            }
            let run: Run = serde_json::from_value(payload["run"].clone()).map_err(|_| invalid())?;
            if run.id != id {
                return Err(invalid());
            }
            if matches!(view, View::Attempts(_)) {
                output["runId"] = json!(run.id);
                output["attempts"] = serde_json::to_value(run.attempts)?;
            } else {
                output["run"] = serde_json::to_value(run)?;
            }
        }
    }
    Ok(output)
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct Scheduler {
    authority_assigned: bool,
    generation: i64,
    owner_id: Option<String>,
    acquired_at: Option<String>,
    last_pass: Option<Pass>,
    queue: Queue,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct Pass {
    generation: i64,
    trigger: String,
    scheduled_at: String,
    started_at: String,
    start_lag_ms: i64,
    finished_at: String,
    duration_ms: i64,
    status: String,
    error_class: Option<String>,
    planned: Option<i64>,
    recovered: Option<i64>,
    claimed: Option<i64>,
    dispatched: Option<i64>,
    failures: Option<i64>,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct Queue {
    planned: i64,
    claimed: i64,
    running: i64,
    recovery_required: i64,
    batch_limit: usize,
    planning_batch_limit: usize,
    oldest_eligible_at: Option<String>,
    oldest_eligible_age_ms: Option<i64>,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct Occurrence {
    id: String,
    automation_id: String,
    automation_revision: u64,
    scheduled_for: String,
    kind: String,
    state: String,
    lease_owner: Option<String>,
    lease_expires_at: Option<String>,
    scheduler_generation: Option<i64>,
    fence_generation: i64,
    runs_truncated: bool,
    runs: Vec<Run>,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct Run {
    id: String,
    automation_id: String,
    automation_revision: u64,
    occurrence_id: Option<String>,
    authority_profile: Option<String>,
    receipt_id: Option<String>,
    session_id: Option<String>,
    status: String,
    exit_code: Option<i64>,
    started_at: String,
    timeout_at: Option<String>,
    finished_at: Option<String>,
    attempts: Vec<Attempt>,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct Attempt {
    id: String,
    run_id: String,
    occurrence_id: String,
    attempt_number: i64,
    occurrence_fence_generation: i64,
    dispatch_generation: i64,
    state: String,
    failure_class: Option<String>,
    prior_attempt_number: Option<i64>,
    prior_disposition: Option<String>,
    retry_classification: String,
    not_before: String,
    session_id: Option<String>,
    opened_at: String,
    settled_at: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn response(view: View<'_>, payload: Value) -> Value {
        json!({"ok": true, "accepted": true, "status": "completed",
            "action": view.action(), "event": {"action": view.action(), "payload": payload}})
    }

    fn run_fixture() -> Value {
        json!({"id": "run-1", "automationId": "routine-1", "automationRevision": 1,
            "status": "recovery_required", "startedAt": "2026-10-18T00:00:00Z",
            "logJson": "private sentinel", "outputCommit": "private sentinel",
            "attempts": [{"id": "attempt-1", "runId": "run-1", "occurrenceId": "occurrence-1",
                "attemptNumber": 2, "occurrenceFenceGeneration": 4, "dispatchGeneration": 6,
                "state": "recovery_required", "retryClassification": "ambiguous",
                "notBefore": "2026-10-18T00:00:00Z", "openedAt": "2026-10-18T00:00:00Z",
                "adoptionKey": "private sentinel", "stateReason": "private sentinel"}]})
    }

    #[test]
    fn inspection_minimizes_nested_records_without_hiding_recovery_or_truncation() {
        let occurrence = json!({"id": "occurrence-1", "automationId": "routine-1",
            "automationRevision": 1, "scheduledFor": "2026-10-18T00:00:00Z", "kind": "scheduled",
            "state": "recovery_required", "fenceGeneration": 4, "runsTruncated": true,
            "failureReason": "private sentinel", "runs": [run_fixture()]});
        let view = View::Occurrence("occurrence-1");
        let output = project(view, &response(view, json!({"occurrence": occurrence}))).unwrap();
        assert!(!output.to_string().contains("private sentinel"));
        assert_eq!(output["occurrence"]["runsTruncated"], true);
        assert_eq!(
            output["occurrence"]["runs"][0]["status"],
            "recovery_required"
        );
        let attempt = &output["occurrence"]["runs"][0]["attempts"][0];
        assert_eq!(attempt["dispatchGeneration"], 6);
        assert_eq!(attempt["retryClassification"], "ambiguous");
        assert_eq!(attempt["settledAt"], Value::Null);

        let view = View::Attempts("run-1");
        let attempts = project(view, &response(view, json!({"run": run_fixture()}))).unwrap();
        assert_eq!(attempts["attempts"][0], *attempt);
        assert_eq!(attempts["runId"], "run-1");
        assert!(attempts.get("run").is_none());

        let mut unknown = run_fixture();
        unknown["status"] = json!("future_unknown_state");
        unknown["attempts"][0]["state"] = json!("future_unknown_state");
        let view = View::Run("run-1");
        let output = project(view, &response(view, json!({"run": unknown}))).unwrap();
        assert_eq!(output["run"]["status"], "future_unknown_state");
        assert_eq!(
            output["run"]["attempts"][0]["state"],
            "future_unknown_state"
        );
    }

    #[test]
    fn inspection_rejects_missing_mismatched_and_malformed_records_privately() {
        let view = View::Run("run-1");
        assert_eq!(
            project(view, &response(view, json!({"run": null})))
                .unwrap_err()
                .to_string(),
            "automation run was not found"
        );
        let valid = response(view, json!({"run": run_fixture()}));
        for path in [
            "/ok",
            "/accepted",
            "/status",
            "/action",
            "/event/action",
            "/event/payload/run/id",
            "/event/payload/run/attempts",
        ] {
            let mut invalid = valid.clone();
            *invalid.pointer_mut(path).unwrap() = json!("private sentinel");
            assert_eq!(
                project(view, &invalid).unwrap_err().to_string(),
                "automation inspection response is invalid",
                "{path}"
            );
        }
        assert!(project(view, &json!({})).is_err());
        let occurrence = View::Occurrence("occurrence-1");
        for payload in [
            json!({}),
            json!({"occurrence": {}}),
            json!({"occurrence": "private sentinel"}),
        ] {
            assert_eq!(
                project(occurrence, &response(occurrence, payload))
                    .unwrap_err()
                    .to_string(),
                "automation inspection response is invalid"
            );
        }
        assert_eq!(
            project(
                occurrence,
                &response(occurrence, json!({"occurrence": null}))
            )
            .unwrap_err()
            .to_string(),
            "automation occurrence was not found"
        );
        for id in ["", " secret", "secret\n", "\u{1b}[31m"] {
            assert_eq!(
                View::Run(id).request().unwrap_err().to_string(),
                "automation inspection id is invalid"
            );
        }
    }

    #[test]
    fn scheduler_projection_accepts_real_empty_store_without_claiming_health() {
        let temp = tempfile::tempdir().unwrap();
        let conn = crate::store::open_store(&temp.path().join("store.sqlite")).unwrap();
        let (status, result) = crate::control_plane::route_action(
            View::Status.request().unwrap(),
            &conn,
            &crate::api::NoopSessionRuntime,
        );
        assert_eq!(status, 200);
        let response = serde_json::to_value(result).unwrap();
        let output = project(View::Status, &response).unwrap();
        assert_eq!(output["scheduler"]["authorityAssigned"], false);
        assert_eq!(output["scheduler"]["lastPass"], Value::Null);
        assert_eq!(output["scheduler"]["queue"]["recoveryRequired"], 0);
        assert!(output.get("healthy").is_none());
        for path in [
            "/event/payload/scheduler/authorityAssigned",
            "/event/payload/scheduler/queue/planned",
            "/event/payload/scheduler/lastPass",
        ] {
            let mut invalid = response.clone();
            *invalid.pointer_mut(path).unwrap() = json!("private sentinel");
            assert_eq!(
                project(View::Status, &invalid).unwrap_err().to_string(),
                "automation inspection response is invalid",
                "{path}"
            );
        }
    }
}
