//! Default-feature, real-daemon regressions for #887 / #933.
//! Extracted from PR #931's threads_e2e.rs at 8576f41e6d622f63a3576b85bd2d3142776e59ca
//! (Val Alexander). This does not enable protected authority or certify Phase 5 acceptance.

use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde_json::{json, Value};
use uuid::Uuid;

#[path = "fixtures/threads_daemon.rs"]
mod threads_daemon;
use threads_daemon::{
    run_journey, HttpResponse, ThreadsFixture, FAMILIAR_ID, PRINCIPAL_FINGERPRINT,
};

const EDITS: &str = "/api/v1/familiars/sage/edits";
const PROPOSALS: &str = "/api/v1/threads/proposals";

#[test]
fn unsigned_protected_intake_is_refused_without_staging() -> Result<()> {
    run_journey(|fixture| {
        let response = fixture.request(
            "POST",
            EDITS,
            Some(&json!({"edits": [{"target": "SOUL.md", "contents": "# Replaced identity\n"}]})),
        )?;
        assert_protected_refusal(&response, "SOUL.md");
        assert_eq!(fs::read(fixture.workspace.join("SOUL.md"))?, b"# Sage\n");
        assert!(!fixture.coven_home.join("pending").exists());
        assert_no_write_authority(fixture)
    })
}

#[test]
fn fingerprints_and_invented_approvals_never_gain_protected_authority() -> Result<()> {
    run_journey(|fixture| {
        let contents = "# Synthetic forbidden replacement\n";
        let invented_id = Uuid::new_v4();
        for after_restart in [false, true] {
            if after_restart {
                fixture.restart_daemon()?;
            }
            for fingerprint in [json!(PRINCIPAL_FINGERPRINT), Value::Null] {
                let response = fixture.request(
                    "POST",
                    EDITS,
                    Some(&json!({
                        "edits": [{"target": "SOUL.md", "contents": contents}],
                        "principalKeyFingerprint": fingerprint,
                        "approvalId": invented_id,
                    })),
                )?;
                assert_protected_refusal(&response, "SOUL.md");
                assert!(!response.body.to_string().contains(contents.trim()));
            }
            let approval = fixture.request(
                "POST",
                &format!("{PROPOSALS}/{invented_id}/approve"),
                Some(&json!({"principalKeyFingerprint": PRINCIPAL_FINGERPRINT})),
            )?;
            assert_eq!(approval.status, 404, "{approval:?}");
            assert_eq!(fs::read(fixture.workspace.join("SOUL.md"))?, b"# Sage\n");
            assert!(!fixture.coven_home.join("pending").exists());
            assert_no_write_authority(fixture)?;
        }
        Ok(())
    })
}

#[test]
fn protected_refusal_audit_is_durable_but_cannot_be_replayed_as_approval() -> Result<()> {
    run_journey(|fixture| {
        let response = fixture.request(
            "POST",
            EDITS,
            Some(&json!({"edits": [{"target": "SOUL.md", "contents": "denied"}]})),
        )?;
        assert_protected_refusal(&response, "SOUL.md");
        let conn = fixture.store()?;
        let rows: i64 = conn.query_row(
            "SELECT COUNT(*) FROM ward_audit
             WHERE familiar_id = ?1 AND event_type = 'proposal_rejected'
               AND decision = 'protected-target-not-proposable'
               AND approver IS NULL AND detail IS NULL",
            [FAMILIAR_ID],
            |row| row.get(0),
        )?;
        assert_eq!(rows, 1, "intake must record one non-authorizing refusal");
        let rejection_id: String = conn.query_row(
            "SELECT proposal_id FROM ward_audit
             WHERE familiar_id = ?1 AND event_type = 'proposal_rejected'",
            [FAMILIAR_ID],
            |row| row.get(0),
        )?;
        drop(conn);

        fixture.restart_daemon()?;
        let retry = fixture.request(
            "POST",
            &format!("{PROPOSALS}/{rejection_id}/approve"),
            Some(&json!({"principalKeyFingerprint": PRINCIPAL_FINGERPRINT})),
        )?;
        assert!(matches!(retry.status, 404 | 409), "{retry:?}");
        assert_one_rejection(fixture, &rejection_id)?;
        assert_eq!(fs::read(fixture.workspace.join("SOUL.md"))?, b"# Sage\n");
        assert!(!fixture.coven_home.join("pending").exists());
        assert_no_write_authority(fixture)
    })
}

#[test]
fn ward_configuration_cannot_demote_protection_through_intake() -> Result<()> {
    run_journey(|fixture| {
        let ward_path = fixture.workspace.join("ward.toml");
        let before = fs::read_to_string(&ward_path)?;
        let replacement = before
            .replace(
                r#"protected_surface = ["SOUL.md"]"#,
                "protected_surface = []",
            )
            .replace(
                "path = \"SOUL.md\"\ntier = 0",
                "path = \"SOUL.md\"\ntier = 2",
            );
        assert_ne!(replacement, before);
        let response = fixture.request(
            "POST",
            EDITS,
            Some(&json!({
                "edits": [{"target": "ward.toml", "contents": replacement}],
                "principalKeyFingerprint": PRINCIPAL_FINGERPRINT,
            })),
        )?;
        assert_protected_refusal(&response, "ward.toml");
        assert!(
            response.body["error"]["message"]
                .as_str()
                .context("refusal message")?
                .contains("Ward control"),
            "refusal must identify the control-file boundary: {response:?}"
        );
        assert!(!response
            .body
            .to_string()
            .contains("principal_key_fingerprint"));
        fixture.restart_daemon()?;
        assert_eq!(fs::read_to_string(ward_path)?, before);
        assert_eq!(fs::read(fixture.workspace.join("SOUL.md"))?, b"# Sage\n");
        assert!(!fixture.coven_home.join("pending").exists());
        assert_no_write_authority(fixture)
    })
}

#[cfg(unix)]
#[test]
fn declared_protected_target_cannot_escape_its_tier_through_an_in_home_symlink() -> Result<()> {
    for target in ["SOUL.md", "./SOUL.md", "identity/../SOUL.md"] {
        run_journey(|fixture| {
            let soul_path = fixture.workspace.join("SOUL.md");
            let materialized = fixture.workspace.join("identity/live.md");
            fs::create_dir(fixture.workspace.join("identity"))?;
            fs::rename(&soul_path, &materialized)?;
            std::os::unix::fs::symlink("identity/live.md", &soul_path)?;
            let response = fixture.request(
                "POST",
                EDITS,
                Some(&json!({
                    "edits": [{"target": target, "contents": "forbidden identity replacement"}],
                    "principalKeyFingerprint": PRINCIPAL_FINGERPRINT,
                })),
            )?;
            assert_eq!(fs::read(&materialized)?, b"# Sage\n");
            assert_eq!(fs::read(&soul_path)?, b"# Sage\n");
            // A symlinked protected baseline may also prevent building safe
            // refusal evidence. That error must still precede every write.
            assert!(matches!(response.status, 403 | 500), "{response:?}");
            assert_ne!(response.body["error"]["details"]["writeApplied"], true);
            assert!(!response
                .body
                .to_string()
                .contains("forbidden identity replacement"));
            assert!(soul_path.is_symlink());
            assert!(!fixture.coven_home.join("pending").exists());
            assert_no_write_authority(fixture)
        })?;
    }
    Ok(())
}

#[cfg(unix)]
#[test]
fn symlinked_ward_control_is_refused_by_declared_and_materialized_path() -> Result<()> {
    for target in [
        "ward.toml",
        "./ward.toml",
        "config/../ward.toml",
        "config/ward.toml",
    ] {
        run_journey(|fixture| {
            let ward_path = fixture.workspace.join("ward.toml");
            let materialized = fixture.workspace.join("config/ward.toml");
            let before = fs::read_to_string(&ward_path)?;
            fs::create_dir(fixture.workspace.join("config"))?;
            fs::rename(&ward_path, &materialized)?;
            std::os::unix::fs::symlink("config/ward.toml", &ward_path)?;
            let replacement = before
                .replace(
                    r#"protected_surface = ["SOUL.md"]"#,
                    "protected_surface = []",
                )
                .replace(
                    "path = \"SOUL.md\"\ntier = 0",
                    "path = \"SOUL.md\"\ntier = 2",
                );
            assert_ne!(replacement, before);
            let response = fixture.request(
                "POST",
                EDITS,
                Some(&json!({
                    "edits": [{"target": target, "contents": replacement}],
                    "principalKeyFingerprint": PRINCIPAL_FINGERPRINT,
                })),
            )?;
            assert_protected_refusal(&response, "config/ward.toml");
            assert_eq!(fs::read_to_string(&materialized)?, before);
            assert_eq!(fs::read_to_string(&ward_path)?, before);
            assert!(ward_path.is_symlink());
            assert!(!fixture.coven_home.join("pending").exists());
            fixture.restart_daemon()?;
            assert_eq!(fs::read_to_string(&ward_path)?, before);
            assert_no_write_authority(fixture)
        })?;
    }
    Ok(())
}

#[test]
fn tier_one_proposal_promoted_to_protected_is_terminally_rejected() -> Result<()> {
    assert_protected_promotion(false)
}

#[test]
fn stale_staged_protected_proposal_is_rejected_after_daemon_restart() -> Result<()> {
    assert_protected_promotion(true)
}

fn assert_protected_promotion(restart_before_approval: bool) -> Result<()> {
    run_journey(|fixture| {
        let ward_path = fixture.workspace.join("ward.toml");
        let protected_config = fs::read_to_string(&ward_path)?;
        // Owner-side fixture setup, not a forged proposal or an intake bypass:
        // SOUL.md is genuinely Tier 1 when the daemon stages this edit.
        let reviewed_config = protected_config
            .replace(
                r#"protected_surface = ["SOUL.md"]"#,
                "protected_surface = []",
            )
            .replace(
                "path = \"SOUL.md\"\ntier = 0",
                "path = \"SOUL.md\"\ntier = 1",
            );
        assert_ne!(reviewed_config, protected_config);
        fs::write(&ward_path, reviewed_config)?;
        let staged = fixture.request(
            "POST",
            EDITS,
            Some(&json!({"edits": [{"target": "SOUL.md", "contents": "reviewed replacement"}]})),
        )?;
        assert_eq!(staged.status, 202, "{staged:?}");
        assert_eq!(staged.body["disposition"], "staged", "{staged:?}");
        assert_eq!(staged.body["reviewKind"], "coherence", "{staged:?}");
        let proposal_id = staged.body["proposalId"].as_str().context("proposal id")?;
        let pending = PathBuf::from(
            staged.body["pendingPath"]
                .as_str()
                .context("pending path")?,
        );
        assert!(
            pending.is_file(),
            "daemon did not persist the staged proposal"
        );
        assert_eq!(fs::read(fixture.workspace.join("SOUL.md"))?, b"# Sage\n");

        if restart_before_approval {
            fixture.stop_daemon()?;
        }
        fs::write(&ward_path, &protected_config)?;
        if restart_before_approval {
            fixture.start_daemon()?;
        }
        let approval_path = format!("{PROPOSALS}/{proposal_id}/approve");
        let refused = fixture.request("POST", &approval_path, Some(&json!({})))?;
        assert_eq!(refused.status, 409, "{refused:?}");
        // Either the scheduler or this approval can win reclassification.
        // Both must leave the same durable rejection, never an apply.
        match refused.body["why"].as_str() {
            Some("protected-target-not-proposable") => {
                assert_eq!(refused.body["targets"], json!(["SOUL.md"]));
                assert_eq!(refused.body["terminal"], true);
            }
            Some("proposal-already-decided") => {
                assert_eq!(refused.body["eventType"], "proposal_rejected");
            }
            _ => panic!("unexpected protected approval response: {refused:?}"),
        }
        assert!(
            !pending.exists(),
            "terminal refusal left the proposal pending"
        );
        assert_eq!(fs::read(fixture.workspace.join("SOUL.md"))?, b"# Sage\n");
        assert_one_rejection(fixture, proposal_id)?;

        fixture.restart_daemon()?;
        let retry = fixture.request("POST", &approval_path, Some(&json!({})))?;
        assert_eq!(retry.status, 409, "{retry:?}");
        assert_one_rejection(fixture, proposal_id)?;
        assert_eq!(fs::read(fixture.workspace.join("SOUL.md"))?, b"# Sage\n");
        assert_eq!(fs::read_to_string(ward_path)?, protected_config);
        assert!(!pending.exists(), "restart resurrected a terminal proposal");
        assert_no_write_authority(fixture)
    })
}

#[test]
fn normal_non_protected_edit_still_applies_and_survives_restart() -> Result<()> {
    run_journey(|fixture| {
        let contents = "hello from the real daemon";
        let response = fixture.request(
            "POST",
            EDITS,
            Some(&json!({
                "edits": [{"target": "notes/today.md", "contents": contents}],
                "principalKeyFingerprint": PRINCIPAL_FINGERPRINT,
            })),
        )?;
        assert_eq!(response.status, 200, "{response:?}");
        assert_eq!(response.body["disposition"], "applied", "{response:?}");
        assert_eq!(response.body["threadsGate"]["outcome"]["kind"], "permitted");
        let audit = &response.body["changes"][0]["audit"];
        assert!(audit["nextSha256"].is_string(), "{audit}");
        assert_eq!(audit["bytesWritten"], contents.len());
        fixture.restart_daemon()?;
        assert_eq!(
            fs::read_to_string(fixture.workspace.join("notes/today.md"))?,
            contents
        );
        assert_eq!(fs::read(fixture.workspace.join("SOUL.md"))?, b"# Sage\n");
        assert!(!fixture.coven_home.join("pending").exists());
        let rows: i64 = fixture.store()?.query_row(
            "SELECT COUNT(*) FROM ward_audit
             WHERE familiar_id = ?1 AND event_type = 'apply_audit'",
            [FAMILIAR_ID],
            |row| row.get(0),
        )?;
        assert_eq!(rows, 1, "ordinary edit must retain its apply audit");
        Ok(())
    })
}

fn assert_protected_refusal(response: &HttpResponse, target: &str) {
    assert_eq!(response.status, 403, "{response:?}");
    assert_eq!(
        response.body["error"]["code"],
        "protected_proposal_forbidden"
    );
    assert_eq!(
        response.body["error"]["details"]["targets"],
        json!([target])
    );
}

fn assert_no_write_authority(fixture: &ThreadsFixture) -> Result<()> {
    let proposals = fixture.request("GET", PROPOSALS, None)?;
    assert_eq!(proposals.status, 200, "{proposals:?}");
    assert_eq!(proposals.body["proposals"], json!([]), "{proposals:?}");
    let applied: i64 = fixture.store()?.query_row(
        "SELECT COUNT(*) FROM ward_audit WHERE familiar_id = ?1
         AND event_type IN ('apply_audit', 'proposal_approved')",
        [FAMILIAR_ID],
        |row| row.get(0),
    )?;
    assert_eq!(applied, 0, "forbidden proposal produced apply evidence");
    Ok(())
}

fn assert_one_rejection(fixture: &ThreadsFixture, proposal_id: &str) -> Result<()> {
    let conn = fixture.store()?;
    let terminals: i64 = conn.query_row(
        "SELECT COUNT(*) FROM ward_audit WHERE proposal_id = ?1
         AND event_type IN ('proposal_rejected', 'proposal_approved', 'proposal_vetoed')",
        [proposal_id],
        |row| row.get(0),
    )?;
    assert_eq!(
        terminals, 1,
        "refused proposal acquired another terminal row"
    );
    let (decision, detail): (String, Option<String>) = conn.query_row(
        "SELECT decision, detail FROM ward_audit
         WHERE proposal_id = ?1 AND event_type = 'proposal_rejected'",
        [proposal_id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    assert_eq!(decision, "protected-target-not-proposable");
    assert_eq!(detail, None, "no-window refusal synthesized close detail");
    Ok(())
}
