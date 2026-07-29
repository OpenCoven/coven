// ward_decision.rs — principal-facing Ward proposal decision commands.

use std::path::Path;

use anyhow::{bail, Context, Result};
use serde_json::Value;

use crate::{api, coven_home_dir};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum WardDecision {
    Approve,
    Reject,
}

impl WardDecision {
    fn as_str(self) -> &'static str {
        match self {
            Self::Approve => "approve",
            Self::Reject => "reject",
        }
    }
}

pub(crate) fn run(
    proposal_id: &str,
    decision: WardDecision,
    note: Option<&str>,
    json: bool,
) -> Result<()> {
    let body = decide_at(&coven_home_dir()?, proposal_id, decision, note)?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&body)
                .context("failed to serialize Ward decision response as JSON")?
        );
    } else {
        print!("{}", render_decision(&body));
    }
    Ok(())
}

fn decide_at(
    coven_home: &Path,
    proposal_id: &str,
    decision: WardDecision,
    note: Option<&str>,
) -> Result<Value> {
    let detail_path = format!("/api/v1/threads/proposals/{proposal_id}");
    let detail_response = api::handle_request("GET", &detail_path, coven_home, None)?;
    let detail: Value = parse_body(&detail_path, &detail_response.body)?;
    let expected_revision = if detail_response.status < 400 {
        Some(
            detail
                .pointer("/proposal/proposalRevision")
                .and_then(Value::as_str)
                .context("Ward proposal detail is missing proposalRevision")?,
        )
    } else if detail_response.status == 404
        && detail.pointer("/error/code").and_then(Value::as_str) == Some("proposal_not_found")
    {
        // A proposal absent from pending may already be complete. Still call
        // the decision route so its terminal audit can make completed retries
        // idempotent; an unknown id remains a not-found error.
        None
    } else {
        return api_failure(&detail_path, detail_response.status, &detail);
    };

    let mut payload = serde_json::Map::new();
    if let Some(revision) = expected_revision {
        payload.insert(
            "expectedRevision".to_string(),
            Value::String(revision.to_string()),
        );
    }
    if let Some(note) = note {
        payload.insert("note".to_string(), Value::String(note.to_string()));
    }
    let payload = Value::Object(payload).to_string();
    let decision_path = format!(
        "/api/v1/threads/proposals/{proposal_id}/{}",
        decision.as_str()
    );
    let response =
        api::handle_request_with_body("POST", &decision_path, coven_home, None, Some(&payload))?;
    let body = parse_body(&decision_path, &response.body)?;
    if response.status >= 400 {
        return api_failure(&decision_path, response.status, &body);
    }
    Ok(body)
}

fn parse_body(path: &str, body: &str) -> Result<Value> {
    serde_json::from_str(body).with_context(|| format!("failed to parse API response for {path}"))
}

fn api_failure(path: &str, status: u16, body: &Value) -> Result<Value> {
    if let Some(why) = body.get("why").and_then(Value::as_str) {
        bail!("Ward decision blocked: {why} (HTTP {status})");
    }
    let code = body
        .pointer("/error/code")
        .and_then(Value::as_str)
        .unwrap_or("unknown_error");
    let message = body
        .pointer("/error/message")
        .and_then(Value::as_str)
        .unwrap_or("Request failed.");
    bail!("{message} ({code}; HTTP {status}; {path})");
}

fn render_decision(body: &Value) -> String {
    let proposal_id = body
        .get("proposalId")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let decision = body
        .get("decision")
        .and_then(Value::as_str)
        .unwrap_or("decided");
    let retry = if body.get("idempotent").and_then(Value::as_bool) == Some(true) {
        " (already recorded)"
    } else {
        ""
    };
    let mut out = format!("Ward proposal {proposal_id}: {decision}{retry}\n");
    let files = body
        .get("filesTouched")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>();
    if !files.is_empty() {
        out.push_str(&format!("Targets: {}\n", files.join(", ")));
    }
    out
}

#[cfg(test)]
mod tests {
    use anyhow::{Context, Result};
    use serde_json::Value;

    use super::*;

    fn stage_reviewed_edit(home: &Path, after: &str) -> Result<(String, std::path::PathBuf)> {
        std::fs::write(
            home.join("familiars.toml"),
            r#"[[familiar]]
id = "sage"
display_name = "Sage"
role = "Research"
description = "Reads and synthesizes."
"#,
        )?;
        let workspace = home.join("familiars").join("sage");
        std::fs::create_dir_all(workspace.join("reviewed"))?;
        std::fs::write(workspace.join("reviewed").join("skill.md"), "before")?;
        std::fs::write(
            workspace.join("ward.toml"),
            r#"principal_key_fingerprint = "fpr-val"

[[surface]]
path = "reviewed/"
tier = 1

[[probe]]
surface = "reviewed/**"
id = "size-delta"
"#,
        )?;
        let response = crate::api::handle_request_with_body(
            "POST",
            "/api/v1/familiars/sage/edits",
            home,
            None,
            Some(
                &serde_json::json!({
                    "edits": [{
                        "target": "reviewed/skill.md",
                        "contents": after,
                    }],
                })
                .to_string(),
            ),
        )?;
        anyhow::ensure!(response.status == 202, "staging failed: {}", response.body);
        let body: Value = serde_json::from_str(&response.body)?;
        let proposal_id = body["proposalId"]
            .as_str()
            .context("staged response carries proposalId")?
            .to_string();
        let pending_path = body["pendingPath"]
            .as_str()
            .context("staged response carries pendingPath")?
            .into();
        Ok((proposal_id, pending_path))
    }

    #[test]
    fn approve_reads_revision_then_applies_through_api() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let (proposal_id, pending_path) = stage_reviewed_edit(temp.path(), "after")?;

        let body = decide_at(
            temp.path(),
            &proposal_id,
            WardDecision::Approve,
            Some("reviewed coherence evidence"),
        )?;

        assert_eq!(body["decision"], "approved");
        assert_eq!(body["proposalId"], proposal_id);
        assert_eq!(
            std::fs::read_to_string(
                temp.path()
                    .join("familiars")
                    .join("sage")
                    .join("reviewed")
                    .join("skill.md"),
            )?,
            "after"
        );
        assert!(!pending_path.exists());
        Ok(())
    }

    #[test]
    fn reject_passes_note_and_leaves_staged_bytes_unapplied() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let (proposal_id, pending_path) = stage_reviewed_edit(temp.path(), "rejected bytes")?;

        let body = decide_at(
            temp.path(),
            &proposal_id,
            WardDecision::Reject,
            Some("needs revision"),
        )?;

        assert_eq!(body["decision"], "rejected");
        assert_eq!(body["note"], "needs revision");
        assert_eq!(
            std::fs::read_to_string(
                temp.path()
                    .join("familiars")
                    .join("sage")
                    .join("reviewed")
                    .join("skill.md"),
            )?,
            "before"
        );
        assert!(!pending_path.exists());
        Ok(())
    }

    #[test]
    fn completed_decision_retries_through_terminal_audit() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let (proposal_id, _) = stage_reviewed_edit(temp.path(), "after")?;

        decide_at(
            temp.path(),
            &proposal_id,
            WardDecision::Approve,
            Some("reviewed"),
        )?;
        let retry = decide_at(
            temp.path(),
            &proposal_id,
            WardDecision::Approve,
            Some("reviewed"),
        )?;

        assert_eq!(retry["decision"], "approved");
        assert_eq!(retry["idempotent"], true);
        Ok(())
    }

    #[test]
    fn opposite_completed_decision_reports_terminal_conflict() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let (proposal_id, _) = stage_reviewed_edit(temp.path(), "after")?;

        decide_at(
            temp.path(),
            &proposal_id,
            WardDecision::Approve,
            Some("reviewed"),
        )?;
        let error = decide_at(
            temp.path(),
            &proposal_id,
            WardDecision::Reject,
            Some("changed my mind"),
        )
        .expect_err("opposite terminal decision must fail");

        assert!(
            error.to_string().contains("proposal-already-decided"),
            "{error:#}"
        );
        Ok(())
    }

    #[test]
    fn decision_renderer_marks_idempotent_terminal_result() {
        let rendered = render_decision(&serde_json::json!({
            "decision": "approved",
            "proposalId": "proposal-1",
            "filesTouched": ["reviewed/skill.md"],
            "idempotent": true,
        }));

        assert!(rendered.contains("proposal-1: approved (already recorded)"));
        assert!(rendered.contains("Targets: reviewed/skill.md"));
    }
}
