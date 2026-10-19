//! Identity-bound doctor transport; never opens the store or starts a daemon.
use super::doctor::{Report, ACTION};
use anyhow::{anyhow, bail, Result};
use coven_client::{DaemonClient, DaemonEndpoint};
use serde_json::{json, Value};

pub(crate) fn run(as_json: bool) -> Result<()> {
    let home = crate::paths::coven_home_dir()
        .map_err(|_| anyhow!("cannot resolve Coven home for automation doctor"))?;
    let endpoint = DaemonEndpoint::discover(home).map_err(|_| {
        anyhow!("owner-local Coven daemon unavailable; check `coven daemon status`")
    })?;
    if !endpoint.is_owner_local() {
        bail!("automation doctor requires owner-local daemon IPC");
    }
    let response = DaemonClient::new(endpoint)
        .request_json("POST", "/api/v1/actions", Some(&json!({"action": ACTION})))
        .map_err(|_| {
            anyhow!("automation doctor could not reach a compatible owner-local daemon")
        })?;
    if response.status != 200 {
        bail!(
            "automation doctor was refused by the daemon (HTTP {})",
            response.status
        );
    }
    let body: Value = serde_json::from_slice(&response.body)
        .map_err(|_| anyhow!("automation doctor response is invalid"))?;
    let report = project(&body)?;
    if !as_json {
        println!("Automations doctor (persisted observations; explicit evidence gaps)");
    }
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

fn project(body: &Value) -> Result<Report> {
    let invalid = || anyhow!("automation doctor response is invalid");
    if body["ok"] != true
        || body["accepted"] != true
        || body["status"] != "completed"
        || body["action"] != ACTION
        || !body["event"].is_null()
        || !body["error"].is_null()
        || !body["reason"].is_null()
    {
        return Err(invalid());
    }
    let report: Report = serde_json::from_value(body["result"].clone()).map_err(|_| invalid())?;
    report.validate().map_err(|_| invalid())?;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn envelope() -> Value {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("coven.sqlite3");
        crate::store::initialize_store(&path).unwrap();
        let conn = crate::store::open_store(&path).unwrap();
        let report = super::super::doctor::inspect(&conn, chrono::Utc::now()).unwrap();
        json!({"ok": true, "accepted": true, "status": "completed", "action": ACTION,
            "event": null, "result": report})
    }

    #[test]
    fn projection_accepts_the_producer_report() {
        assert!(project(&envelope()).is_ok());
    }

    #[test]
    fn projection_refuses_a_completed_envelope_with_an_error() {
        let mut body = envelope();
        body["error"] = json!({"message": "PRIVATE-DOCTOR-ERROR"});
        assert!(
            project(&body).is_err(),
            "completed/error envelope was accepted"
        );
    }

    #[test]
    fn projection_refuses_a_completed_envelope_with_a_reason() {
        let mut body = envelope();
        body["reason"] = json!("PRIVATE-DOCTOR-REASON");
        assert_eq!(
            project(&body).unwrap_err().to_string(),
            "automation doctor response is invalid"
        );
    }

    #[test]
    fn projection_errors_do_not_echo_private_response_fields() {
        let original = envelope();
        for mutate in [
            (|body: &mut Value| body["action"] = json!("PRIVATE-DOCTOR-MARKER")) as fn(&mut Value),
            |body| body["event"] = json!({"private": "PRIVATE-DOCTOR-MARKER"}),
            |body| body["result"]["observedAt"] = json!("PRIVATE-DOCTOR-MARKER"),
            |body| body["result"]["observations"][0]["private"] = json!("PRIVATE-DOCTOR-MARKER"),
            |body| body["result"]["observations"][0]["facts"]["private_fact"] = json!(1),
            |body| body["result"]["observations"][0]["facts"]["leader_assigned"] = json!(2),
            |body| {
                body["result"]["observations"][0]["facts"]["scheduler_generation"] =
                    json!(9_007_199_254_740_992_u64)
            },
            |body| body["result"]["observations"][0]["gaps"] = json!(["PRIVATE-DOCTOR-MARKER"]),
        ] {
            let mut body = original.clone();
            mutate(&mut body);
            let error = project(&body).unwrap_err().to_string();
            assert_eq!(error, "automation doctor response is invalid");
            assert!(!error.contains("PRIVATE-DOCTOR-MARKER"));
        }
    }
}
