//! Minimized owner-local lease inspection. Never opens the store or starts a daemon.
use anyhow::{anyhow, bail, Result};
use coven_client::{DaemonClient, DaemonEndpoint};
use serde_json::{json, Value};

use super::leases::{Expiry, Page, ACTION, SCHEMA};

pub(super) fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 512
        && value.trim() == value
        && !value.chars().any(char::is_control)
}

pub(crate) fn run(
    automation_id: Option<&str>,
    limit: u8,
    cursor: Option<&str>,
    as_json: bool,
) -> Result<()> {
    if !(1..=100).contains(&limit)
        || automation_id.is_some_and(|id| !valid_id(id))
        || cursor.is_some_and(|value| {
            value.is_empty()
                || value.len() > 4096
                || !value
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        })
    {
        bail!("automation lease inspection arguments are invalid");
    }
    let mut request = json!({"action": ACTION, "limit": limit});
    if let Some(id) = automation_id {
        request["automationId"] = json!(id);
    }
    if let Some(cursor) = cursor {
        request["cursor"] = json!(cursor);
    }
    let home = crate::paths::coven_home_dir()
        .map_err(|_| anyhow!("cannot resolve Coven home for lease inspection"))?;
    let endpoint = DaemonEndpoint::discover(home).map_err(|_| {
        anyhow!("owner-local Coven daemon unavailable; check `coven daemon status`")
    })?;
    if !endpoint.is_owner_local() {
        bail!("automation lease inspection requires owner-local daemon IPC");
    }
    let response = DaemonClient::new(endpoint)
        .request_json("POST", "/api/v1/actions", Some(&request))
        .map_err(|_| anyhow!("automation lease inspection could not reach a compatible owner-local daemon; check `coven daemon status`"))?;
    if response.status != 200 {
        bail!(
            "automation lease inspection was refused by the daemon (HTTP {})",
            response.status
        );
    }
    let response: Value = serde_json::from_slice(&response.body)
        .map_err(|_| anyhow!("automation lease inspection response is invalid"))?;
    let output = project(&response, automation_id, limit, cursor)?;
    if !as_json {
        println!("Automations leases (owner-local metadata; not process liveness)");
    }
    // Serialize only the checked DTO; never print daemon reasons or arbitrary fields.
    println!("{}", serde_json::to_string_pretty(&output)?);
    Ok(())
}

fn project(
    response: &Value,
    filter: Option<&str>,
    limit: u8,
    cursor: Option<&str>,
) -> Result<Page> {
    let invalid = || anyhow!("automation lease inspection response is invalid");
    if response["ok"] != true
        || response["accepted"] != true
        || response["status"] != "completed"
        || response["action"] != ACTION
        || !response["event"].is_null()
    {
        return Err(invalid());
    }
    let page: Page = serde_json::from_value(response["result"].clone()).map_err(|_| invalid())?;
    let observed =
        chrono::DateTime::parse_from_rfc3339(&page.observed_at).map_err(|_| invalid())?;
    if page.schema_version != SCHEMA
        || page.observed_at.len() > 64
        || page.leases.len() > usize::from(limit)
        || page.cursor.current.as_deref() != cursor
        || page.cursor.has_more != page.cursor.next.is_some()
        || (page.cursor.has_more && page.leases.len() != usize::from(limit))
    {
        return Err(invalid());
    }
    let decode = |value: &str| -> Result<String> {
        use base64::Engine as _;
        if value.len() > 4096 {
            return Err(invalid());
        }
        let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(value)
            .map_err(|_| invalid())?;
        let decoded: (Option<String>, String) =
            serde_json::from_slice(&bytes).map_err(|_| invalid())?;
        if decoded.0.as_deref() != filter
            || !valid_id(&decoded.1)
            || serde_json::to_vec(&decoded).map_err(|_| invalid())? != bytes
            || base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&bytes) != value
        {
            return Err(invalid());
        }
        Ok(decoded.1)
    };
    let mut previous = cursor.map(decode).transpose()?;
    for lease in &page.leases {
        if !valid_id(&lease.occurrence_id)
            || !valid_id(&lease.automation_id)
            || filter.is_some_and(|id| lease.automation_id != id)
            || previous
                .as_ref()
                .is_some_and(|id| lease.occurrence_id <= *id)
            || lease.owner_ref.as_ref().is_some_and(|reference| {
                !lease.owner_present
                    || !reference.strip_prefix("sha256:").is_some_and(|digest| {
                        digest.len() == 64
                            && digest
                                .bytes()
                                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
                    })
            })
            || lease
                .scheduler_generation
                .is_some_and(|value| !(1..=9_007_199_254_740_991).contains(&value))
            || lease
                .fence_generation
                .is_some_and(|value| !(0..=9_007_199_254_740_991).contains(&value))
            || lease.issues.len() > 6
            || lease.issues.iter().any(|issue| {
                !matches!(
                    issue.as_str(),
                    "missing_owner"
                        | "invalid_owner"
                        | "missing_expiry"
                        | "invalid_expiry"
                        | "lease_outside_active_state"
                        | "invalid_scheduler_generation"
                        | "invalid_fence_generation"
                )
            })
        {
            return Err(invalid());
        }
        match (&lease.lease_expires_at, &lease.expiry) {
            (Some(value), Expiry::Active | Expiry::Expired) if value.len() <= 64 => {
                let at = chrono::DateTime::parse_from_rfc3339(value).map_err(|_| invalid())?;
                if (at > observed) != matches!(lease.expiry, Expiry::Active) {
                    return Err(invalid());
                }
            }
            (None, Expiry::Missing | Expiry::Invalid) => {}
            _ => return Err(invalid()),
        }
        previous = Some(lease.occurrence_id.clone());
    }
    if let Some(next) = &page.cursor.next {
        if Some(decode(next)?) != previous {
            return Err(invalid());
        }
    }
    Ok(page)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn response() -> Value {
        json!({"ok":true,"accepted":true,"status":"completed","action":ACTION,"event":null,
          "result":{"schemaVersion":SCHEMA,"observedAt":"2026-09-01T09:00:00Z",
          "leases":[{"occurrenceId":"a","automationId":"target","state":"failed",
          "ownerPresent":true,"ownerRef":format!("sha256:{}", "a".repeat(64)),
          "leaseExpiresAt":"2026-09-01T08:00:00Z","expiry":"expired",
          "schedulerGeneration":7,"fenceGeneration":2,"issues":["lease_outside_active_state"]}],
          "cursor":{"current":null,"next":null,"hasMore":false}}})
    }
    #[test]
    fn lease_projection_rejects_private_metadata_and_inconsistent_pages() {
        assert!(project(&response(), Some("target"), 20, None).is_ok());
        for (pointer, value) in [
            ("/result/leases/0/ownerRef", json!("private-owner-path")),
            ("/result/leases/0/issues", json!(["private-reason"])),
            ("/result/leases/0/occurrenceId", json!("a\u{0}suffix")),
            ("/result/leases/0/automationId", json!("other")),
            ("/result/leases/0/state", json!("private-state")),
            ("/result/leases/0/expiry", json!("active")),
            ("/result/leases/0/leaseExpiresAt", json!("private-expiry")),
            ("/result/leases/0/schedulerGeneration", json!(-1)),
            (
                "/result/leases/0/fenceGeneration",
                json!(9_007_199_254_740_992_i64),
            ),
            ("/result/cursor/hasMore", json!(true)),
            ("/result/cursor/current", json!("private-cursor")),
            ("/result/schemaVersion", json!("unknown")),
            ("/result/observedAt", json!("private-clock")),
            ("/event", json!({"private":"event"})),
            ("/ok", json!(false)),
        ] {
            let mut response = response();
            *response.pointer_mut(pointer).unwrap() = value;
            let error = match project(&response, Some("target"), 20, None) {
                Ok(_) => panic!("accepted invalid field {pointer}"),
                Err(error) => error,
            };
            assert_eq!(
                error.to_string(),
                "automation lease inspection response is invalid"
            );
        }
        let mut response = response();
        response["result"]["private"] = json!("private-prompt");
        let page = project(&response, None, 20, None).unwrap();
        assert!(!serde_json::to_string(&page)
            .unwrap()
            .contains("private-prompt"));
    }
}
