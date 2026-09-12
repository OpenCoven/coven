//! Real-daemon Threads boundary journeys with isolated state and provenance.
//! Controlled replay coverage requires `threads-test-clock`. This suite does
//! not certify the pending signed-authorization profile or human freeze gates.

use std::ffi::OsString;
use std::fs;
#[cfg(feature = "threads-test-clock")]
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use rusqlite::Connection;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use uuid::Uuid;

const FAMILIAR_ID: &str = "sage";
const PRINCIPAL_FINGERPRINT: &str = "fpr-e2e-synthetic";
const REQUIRE_OVERRIDE_ENV: &str = "COVEN_THREADS_E2E_REQUIRE_LOCAL_OVERRIDE";
const ARTIFACT_ROOT_ENV: &str = "COVEN_THREADS_E2E_ARTIFACT_ROOT";
const REPRODUCTION_COMMAND: &str = if cfg!(feature = "threads-test-clock") {
    "cargo test --locked -p coven-cli --test threads_e2e --features threads-test-clock -- --nocapture"
} else {
    "cargo test --locked -p coven-cli --test threads_e2e -- --nocapture"
};

#[cfg(feature = "threads-test-clock")]
mod final_commit_cases {
    include!("support/threads_final_commit_cases.rs");
}

#[test]
fn smoke_bounded_ward_apply_over_real_daemon() -> Result<()> {
    run_journey("smoke-bounded-ward-apply", |fixture| {
        let request = json!({
            "edits": [{
                "target": "notes/today.md",
                "contents": "hello from the real daemon"
            }],
            "principalKeyFingerprint": PRINCIPAL_FINGERPRINT
        });
        let response = fixture.request("POST", "/api/v1/familiars/sage/edits", Some(&request))?;

        anyhow::ensure!(response.status == 200, "unexpected response: {response:?}");
        anyhow::ensure!(
            response.body["disposition"] == "applied",
            "write was not applied: {}",
            response.body
        );
        anyhow::ensure!(
            response.body["threadsGate"]["outcome"]["kind"] == "permitted",
            "Threads did not permit the write: {}",
            response.body
        );
        anyhow::ensure!(
            fs::read_to_string(fixture.workspace.join("notes/today.md"))?
                == "hello from the real daemon",
            "governed bytes do not match the applied request"
        );
        anyhow::ensure!(
            !fixture.coven_home.join("pending").exists(),
            "an applied write left pending proposal state"
        );

        let conn = fixture.store()?;
        let apply_rows: i64 = conn.query_row(
            "SELECT COUNT(*) FROM ward_audit
             WHERE familiar_id = ?1 AND event_type = 'apply_audit'",
            [FAMILIAR_ID],
            |row| row.get(0),
        )?;
        anyhow::ensure!(
            apply_rows == 1,
            "expected one apply audit, got {apply_rows}"
        );

        let audit = &response.body["changes"][0]["audit"];
        anyhow::ensure!(audit["nextSha256"].is_string(), "missing next hash");
        anyhow::ensure!(
            audit["bytesWritten"] == "hello from the real daemon".len(),
            "unexpected applied byte count"
        );
        Ok(())
    })
}

#[test]
fn smoke_unsigned_protected_rejection_over_real_daemon() -> Result<()> {
    run_journey("smoke-unsigned-protected-rejection", |fixture| {
        let request = json!({
            "edits": [{
                "target": "SOUL.md",
                "contents": "# Replaced identity\n"
            }]
        });
        let response = fixture.request("POST", "/api/v1/familiars/sage/edits", Some(&request))?;

        anyhow::ensure!(response.status == 403, "unexpected response: {response:?}");
        anyhow::ensure!(
            response.body["error"]["code"] == "protected_proposal_forbidden",
            "unexpected refusal: {}",
            response.body
        );
        anyhow::ensure!(
            fs::read_to_string(fixture.workspace.join("SOUL.md"))? == "# Sage\n",
            "unsigned request changed protected bytes"
        );
        anyhow::ensure!(
            !fixture.coven_home.join("pending").exists(),
            "unsigned protected request entered the proposal route"
        );

        let conn = fixture.store()?;
        let apply_rows: i64 = conn.query_row(
            "SELECT COUNT(*) FROM ward_audit
             WHERE familiar_id = ?1 AND event_type = 'apply_audit'",
            [FAMILIAR_ID],
            |row| row.get(0),
        )?;
        anyhow::ensure!(apply_rows == 0, "refused request appended an apply audit");
        Ok(())
    })
}

#[test]
fn protected_proposal_routes_never_gain_write_authority() -> Result<()> {
    run_journey("protected-proposal-route-prohibition", |fixture| {
        let contents = "# Synthetic forbidden replacement\n";
        for fingerprint in [json!(PRINCIPAL_FINGERPRINT), Value::Null] {
            let request = json!({
                "edits": [{"target": "SOUL.md", "contents": contents}],
                "principalKeyFingerprint": fingerprint,
                "approvalId": Uuid::new_v4().to_string(),
            });
            let response =
                fixture.request("POST", "/api/v1/familiars/sage/edits", Some(&request))?;
            anyhow::ensure!(
                response.status == 403
                    && response.body["error"]["code"] == "protected_proposal_forbidden",
                "protected proposal endpoint accepted a claimed authority: {response:?}"
            );
            anyhow::ensure!(
                !response.body.to_string().contains(contents),
                "protected rejection echoed proposed content"
            );
        }
        let proposals = fixture.request("GET", "/api/v1/threads/proposals", None)?;
        anyhow::ensure!(
            proposals.status == 200 && proposals.body["proposals"] == json!([]),
            "protected intake created proposal authority: {proposals:?}"
        );
        let invented_id = Uuid::new_v4();
        let approval = fixture.request(
            "POST",
            &format!("/api/v1/threads/proposals/{invented_id}/approve"),
            Some(&json!({"principalKeyFingerprint": PRINCIPAL_FINGERPRINT})),
        )?;
        anyhow::ensure!(
            approval.status == 404,
            "invented approval id was not refused: {approval:?}"
        );
        fixture.restart_daemon()?;
        anyhow::ensure!(
            fs::read_to_string(fixture.workspace.join("SOUL.md"))? == "# Sage\n",
            "a proposal or restart changed protected bytes"
        );
        let conn = fixture.store()?;
        let applied: i64 = conn.query_row(
            "SELECT COUNT(*) FROM ward_audit
             WHERE event_type IN ('apply_audit', 'proposal_approved')",
            [],
            |row| row.get(0),
        )?;
        anyhow::ensure!(applied == 0, "forbidden proposal produced apply evidence");
        Ok(())
    })
}

#[test]
fn protected_rejection_has_durable_non_authorizing_audit() -> Result<()> {
    run_journey("protected-rejection-audit", |fixture| {
        let response = fixture.request(
            "POST",
            "/api/v1/familiars/sage/edits",
            Some(&json!({"edits": [{"target": "SOUL.md", "contents": "denied"}]})),
        )?;
        anyhow::ensure!(response.status == 403, "unexpected response: {response:?}");
        let conn = fixture.store()?;
        let rows: i64 = conn.query_row(
            "SELECT COUNT(*) FROM ward_audit
             WHERE familiar_id = ?1 AND event_type = 'proposal_rejected'
               AND decision = 'protected-target-not-proposable' AND detail IS NULL",
            [FAMILIAR_ID],
            |row| row.get(0),
        )?;
        anyhow::ensure!(
            rows == 1,
            "protected admission refusal must have exactly one non-authorizing audit row, got {rows}"
        );
        let rejection_id: String = conn.query_row(
            "SELECT proposal_id FROM ward_audit
             WHERE familiar_id = ?1 AND event_type = 'proposal_rejected'
               AND decision = 'protected-target-not-proposable'",
            [FAMILIAR_ID],
            |row| row.get(0),
        )?;
        drop(conn);
        fixture.restart_daemon()?;
        let retry = fixture.request(
            "POST",
            &format!("/api/v1/threads/proposals/{rejection_id}/approve"),
            Some(&json!({"principalKeyFingerprint": PRINCIPAL_FINGERPRINT})),
        )?;
        anyhow::ensure!(
            matches!(retry.status, 404 | 409),
            "refusal audit id became approval authority after restart: {retry:?}"
        );
        let conn = fixture.store()?;
        let terminals: i64 = conn.query_row(
            "SELECT COUNT(*) FROM ward_audit WHERE proposal_id = ?1
             AND event_type IN ('proposal_rejected', 'proposal_approved', 'proposal_vetoed')",
            [&rejection_id],
            |row| row.get(0),
        )?;
        anyhow::ensure!(
            terminals == 1,
            "refused intake acquired another terminal row"
        );
        anyhow::ensure!(
            fs::read_to_string(fixture.workspace.join("SOUL.md"))? == "# Sage\n",
            "audited rejection changed protected bytes"
        );
        Ok(())
    })
}

#[test]
#[cfg(feature = "threads-test-clock")]
fn clock_control_and_audit_time_survive_real_daemon_restart() -> Result<()> {
    run_clocked_journey(
        "deterministic-clock-restart",
        |_, _| Ok(()),
        |fixture, capability| {
            let advanced = "2099-01-01T00:01:00Z";
            advance_clock(fixture, capability, advanced)?;
            let refusal = fixture.request(
                "POST",
                "/api/v1/familiars/sage/edits",
                Some(&json!({
                    "edits": [{"target": "SOUL.md", "contents": "# Forbidden fixture\n"}],
                })),
            )?;
            anyhow::ensure!(refusal.status == 403, "unexpected refusal: {refusal:?}");
            let decided_at: String = fixture.store()?.query_row(
                "SELECT decided_at FROM ward_audit
                 WHERE event_type = 'proposal_rejected'",
                [],
                |row| row.get(0),
            )?;
            anyhow::ensure!(
                decided_at == advanced,
                "audit escaped the logical clock: {decided_at}"
            );
            fixture.restart_daemon()?;
            let tick = fixture.request(
                "POST",
                "/api/v1/internal/threads/test-clock/tick",
                Some(&json!({"capability": capability})),
            )?;
            anyhow::ensure!(
                tick.status == 200
                    && tick.body["source"] == "deterministic_fixture"
                    && tick.body["now"] == advanced
                    && tick.body["processed"] == 0,
                "restart lost controlled scheduler time: {tick:?}"
            );
            let backwards = fixture.request(
                "POST",
                "/api/v1/internal/threads/test-clock",
                Some(&json!({
                    "capability": capability,
                    "now": "2099-01-01T00:00:59Z",
                })),
            )?;
            anyhow::ensure!(
                backwards.status == 409,
                "restart allowed time to move backwards: {backwards:?}"
            );
            anyhow::ensure!(
                fixture
                    .last_request
                    .as_ref()
                    .context("clock request recorded")?["body"]["capability"]
                    == "<fixture-capability>",
                "artifact request retained the fixture capability"
            );
            Ok(())
        },
    )
}

#[test]
#[cfg(feature = "threads-test-clock")]
fn retired_corpus_scheduled_intake_survives_restart_and_applies_once() -> Result<()> {
    let corpus = retired_ward_corpus()?;
    let case = retired_review_case(&corpus)?;
    run_clocked_journey(
        "retired-corpus-scheduled-restart",
        |home, workspace| seed_retired_review_case(home, workspace, case),
        |fixture, capability| {
            fs::create_dir_all(&fixture.artifact_dir)?;
            fs::write(
                fixture.artifact_dir.join("corpus.json"),
                serde_json::to_vec_pretty(&corpus)?,
            )?;
            let protected_before = fs::read(fixture.workspace.join("SOUL.md"))?;
            let staged = submit_retired_case(fixture, case)?;
            let id = staged["proposalId"]
                .as_str()
                .context("scheduled proposal id")?;
            let pending: Value = serde_json::from_slice(&fs::read(
                staged["pendingPath"].as_str().context("pending path")?,
            )?)?;
            anyhow::ensure!(
                pending["schema"] == "phase5_v1"
                    && pending["pending"]["id"] == id
                    && pending["classification"] == staged["scheduledProposal"]["classification"]
                    && pending["region_evidence"] == staged["scheduledProposal"]["region_evidence"],
                "response and durable canonical evidence disagree"
            );
            let submitted_detail: String = fixture.store()?.query_row(
                "SELECT detail FROM ward_audit WHERE proposal_id = ?1 AND event_type = 'proposal_submitted'",
                [id],
                |row| row.get(0),
            )?;
            let submitted_detail: Value = serde_json::from_str(&submitted_detail)?;
            anyhow::ensure!(
                submitted_detail["classification"] == pending["classification"],
                "submission audit does not bind the classified diff and replay evidence"
            );
            let minimum = case["approval"]["veto"]["min_visible_seconds"]
                .as_i64()
                .context("corpus minimum visibility")?;
            let duration = case["approval"]["veto"]["duration_seconds"]
                .as_i64()
                .context("corpus veto duration")?;
            let minimum_at: time::OffsetDateTime =
                serde_json::from_value(pending["earliest_close"].clone())?;
            let deadline: time::OffsetDateTime =
                serde_json::from_value(pending["veto_deadline"].clone())?;
            anyhow::ensure!(
                minimum_at == fixture_time(minimum)? && deadline == fixture_time(duration)?,
                "intake changed corpus scheduling policy"
            );
            tick_scheduler(fixture, capability)?;
            advance_clock_to_offset(fixture, capability, minimum - 1)?;
            tick_scheduler(fixture, capability)?;
            assert_corpus_bytes(fixture, case, "before")?;
            fixture.restart_daemon()?;
            advance_clock_to_offset(fixture, capability, minimum)?;
            tick_scheduler(fixture, capability)?;
            assert_corpus_bytes(fixture, case, "before")?;
            let pending_list = fixture.request("GET", "/api/v1/threads/proposals", None)?;
            anyhow::ensure!(
                pending_list.status == 200
                    && pending_list.body["proposals"]
                        .as_array()
                        .is_some_and(|proposals| proposals
                            .iter()
                            .any(|proposal| proposal["proposalId"] == id)),
                "restart lost the visible pending interval: {pending_list:?}"
            );
            advance_clock_to_offset(fixture, capability, duration + 1)?;
            tick_scheduler(fixture, capability)?;
            assert_corpus_bytes(fixture, case, "after")?;
            assert_window_terminal(fixture, id, "proposal_approved", "applied", json!(true))?;
            fixture.restart_daemon()?;
            tick_scheduler(fixture, capability)?;
            let repeated = fixture.request(
                "POST",
                &format!("/api/v1/threads/proposals/{id}/approve"),
                Some(&json!({"principalKeyFingerprint": PRINCIPAL_FINGERPRINT})),
            )?;
            anyhow::ensure!(
                matches!(repeated.status, 404 | 409)
                    || (repeated.status == 200
                        && repeated.body["idempotent"] == true
                        && repeated.body["decision"] == "approved"),
                "terminal proposal did not return its existing receipt: {repeated:?}"
            );
            assert_corpus_bytes(fixture, case, "after")?;
            assert_window_terminal(fixture, id, "proposal_approved", "applied", json!(true))?;
            anyhow::ensure!(
                fs::read(fixture.workspace.join("SOUL.md"))? == protected_before,
                "scheduled reviewed writes changed protected identity"
            );
            let remaining = fixture.request("GET", "/api/v1/threads/proposals", None)?;
            anyhow::ensure!(
                remaining.status == 200 && remaining.body["proposals"] == json!([]),
                "terminal proposal remains pending: {remaining:?}"
            );
            Ok(())
        },
    )
}

#[test]
#[cfg(feature = "threads-test-clock")]
fn invalid_identity_at_reviewed_intake_cannot_stage_or_apply() -> Result<()> {
    let corpus = retired_ward_corpus()?;
    let case = retired_review_case(&corpus)?;
    for missing in [false, true] {
        run_clocked_journey(
            if missing {
                "identity-intake-missing"
            } else {
                "identity-intake-changed"
            },
            |home, workspace| seed_retired_review_case(home, workspace, case),
            |fixture, _| {
                let identity = fixture.workspace.join("IDENTITY.md");
                if missing {
                    fs::remove_file(&identity)?;
                } else {
                    fs::write(
                        &identity,
                        "# IDENTITY.md - Synthetic-unbound\n- **Pronouns:** they/them\n",
                    )?;
                }
                let response = fixture.request(
                    "POST",
                    "/api/v1/familiars/sage/edits",
                    Some(&json!({
                        "edits": [{"target": "TOOLS.md", "contents": "Synthetic candidate"}],
                        "principalKeyFingerprint": PRINCIPAL_FINGERPRINT,
                    })),
                )?;
                anyhow::ensure!(
                    response.status == 403 && response.body["error"]["code"] == "ward_refused",
                    "invalid identity entered reviewed intake: {response:?}"
                );
                let listed = fixture.request("GET", "/api/v1/threads/proposals", None)?;
                anyhow::ensure!(
                    listed.body["proposals"] == json!([]),
                    "invalid identity was staged"
                );
                assert_corpus_bytes(fixture, case, "before")?;
                let rejected: i64 = fixture.store()?.query_row(
                    "SELECT COUNT(*) FROM ward_audit WHERE event_type = 'validation_verdict'
                     AND decision LIKE 'reject:%'",
                    [],
                    |row| row.get(0),
                )?;
                anyhow::ensure!(
                    rejected == 1,
                    "identity refusal lacks its authoritative verdict"
                );
                Ok(())
            },
        )?;
    }
    Ok(())
}

#[test]
#[cfg(feature = "threads-test-clock")]
fn unsupported_corpus_duplicate_surface_is_refused_at_intake() -> Result<()> {
    let corpus = retired_ward_corpus()?;
    let case = retired_review_case(&corpus)?;
    let unsupported = corpus["unsupported_cases"]
        .as_array()
        .context("unsupported cases")?
        .iter()
        .find(|case| case["id"] == "duplicate-materialized-surface")
        .context("canonical duplicate-surface case")?;
    run_clocked_journey(
        "unsupported-corpus-duplicate-surface",
        |home, workspace| seed_retired_review_case(home, workspace, case),
        |fixture, _| {
            let edits: Vec<Value> = unsupported["surfaces"]
                .as_array()
                .context("unsupported surfaces")?
                .iter()
                .map(|surface| json!({"target": surface["path"], "contents": surface["after"]}))
                .collect();
            let response = fixture.request(
                "POST",
                "/api/v1/familiars/sage/edits",
                Some(&json!({"edits": edits, "principalKeyFingerprint": PRINCIPAL_FINGERPRINT})),
            )?;
            anyhow::ensure!(
                response.status == 400 && response.body["error"]["code"] == "invalid_request",
                "unsupported corpus input was not refused: {response:?}"
            );
            assert_corpus_bytes(fixture, case, "before")?;
            let listed = fixture.request("GET", "/api/v1/threads/proposals", None)?;
            anyhow::ensure!(
                listed.body["proposals"] == json!([]),
                "unsupported input was staged"
            );
            let applied: i64 = fixture.store()?.query_row(
                "SELECT COUNT(*) FROM ward_audit WHERE event_type IN ('apply_audit', 'proposal_approved')",
                [], |row| row.get(0),
            )?;
            anyhow::ensure!(
                applied == 0,
                "unsupported input produced applied-write evidence"
            );
            Ok(())
        },
    )
}

#[test]
#[cfg(feature = "threads-test-clock")]
fn changed_human_path_cannot_erase_a_real_opened_window() -> Result<()> {
    let corpus = retired_ward_corpus()?;
    let case = retired_review_case(&corpus)?;
    run_clocked_journey(
        "changed-human-path-opened-window",
        |home, workspace| seed_retired_review_case(home, workspace, case),
        |fixture, capability| {
            let staged = submit_retired_case(fixture, case)?;
            let id = staged["proposalId"].as_str().context("proposal id")?;
            tick_scheduler(fixture, capability)?;
            fixture.stop_daemon()?;
            let path = Path::new(staged["pendingPath"].as_str().context("pending path")?);
            let mut altered: Value = serde_json::from_slice(&fs::read(path)?)?;
            altered["classification"]["approval_path"] = json!({"kind": "human_approval"});
            altered["lifecycle"] = json!({"state": "awaiting_human_approval"});
            altered["veto_deadline"] = Value::Null;
            altered["earliest_close"] = Value::Null;
            fs::write(path, serde_json::to_vec(&altered)?)?;
            fixture.start_daemon()?;
            let payload =
                proposal_decision_payload(fixture, id, "Synthetic contradictory history")?;
            let refused = fixture.request(
                "POST",
                &format!("/api/v1/threads/proposals/{id}/approve"),
                Some(&payload),
            )?;
            anyhow::ensure!(
                refused.status == 409
                    && refused.body["why"] == "proposal-window-state-inconsistent",
                "changed proposal bytes bypassed opened-window history: {refused:?}"
            );
            assert_window_terminal(
                fixture,
                id,
                "proposal_rejected",
                "revalidation_failed",
                json!(false),
            )?;
            assert_corpus_bytes(fixture, case, "before")?;
            fixture.restart_daemon()?;
            tick_scheduler(fixture, capability)?;
            assert_window_terminal(
                fixture,
                id,
                "proposal_rejected",
                "revalidation_failed",
                json!(false),
            )?;
            Ok(())
        },
    )
}

#[test]
#[cfg(feature = "threads-test-clock")]
fn scheduled_window_replay_fails_closed_across_real_daemon_restart() -> Result<()> {
    let corpus = retired_ward_corpus()?;
    let case = retired_review_case(&corpus)?;
    for scenario in [
        "vetoed",
        "surface-diverged",
        "ward-unavailable",
        "identity-changed",
        "identity-unavailable",
        "binding-revoked",
        "approval-policy-changed",
        "approval-policy-removed",
    ] {
        run_clocked_journey(
            &format!("scheduled-window-{scenario}"),
            |home, workspace| seed_retired_review_case(home, workspace, case),
            |fixture, capability| {
                let staged = submit_retired_case(fixture, case)?;
                let id = staged["proposalId"].as_str().context("proposal id")?;
                tick_scheduler(fixture, capability)?;
                match scenario {
                    "surface-diverged" => fs::write(
                        fixture.workspace.join("TOOLS.md"),
                        "Synthetic out-of-band tool change",
                    )?,
                    "ward-unavailable" => fs::remove_file(fixture.workspace.join("ward.toml"))?,
                    "identity-changed" => fs::write(
                        fixture.workspace.join("IDENTITY.md"),
                        "# IDENTITY.md - Synthetic-other\n- **Pronouns:** they/them\n",
                    )?,
                    "identity-unavailable" => {
                        fs::remove_file(fixture.workspace.join("IDENTITY.md"))?
                    }
                    "binding-revoked" => {
                        let path = fixture.workspace.join("ward.toml");
                        let ward = fs::read_to_string(&path)?;
                        anyhow::ensure!(
                            ward.contains(PRINCIPAL_FINGERPRINT),
                            "fixture has no original principal binding"
                        );
                        fs::write(
                            path,
                            ward.replace(PRINCIPAL_FINGERPRINT, "fpr-synthetic-revoked"),
                        )?;
                    }
                    "approval-policy-changed" => {
                        let path = fixture.workspace.join("ward.toml");
                        let mut ward: toml::Value = toml::from_str(&fs::read_to_string(&path)?)?;
                        ward["approval_tiers"]["familiar_review"]["human_veto_window_hours"] =
                            toml::Value::Integer(3);
                        fs::write(path, toml::to_string(&ward)?)?;
                    }
                    "approval-policy-removed" => {
                        let path = fixture.workspace.join("ward.toml");
                        let mut ward: toml::Value = toml::from_str(&fs::read_to_string(&path)?)?;
                        let table = ward.as_table_mut().context("Ward table")?;
                        table.remove("editable");
                        table.remove("approval_tiers");
                        fs::write(path, toml::to_string(&ward)?)?;
                    }
                    "vetoed" => {}
                    _ => unreachable!("scenarios are enumerated above"),
                }
                fixture.restart_daemon()?;
                let (event, reason, replay) = if scenario == "vetoed" {
                    let payload =
                        proposal_decision_payload(fixture, id, "Synthetic principal veto")?;
                    let rejected = fixture.request(
                        "POST",
                        &format!("/api/v1/threads/proposals/{id}/reject"),
                        Some(&payload),
                    )?;
                    anyhow::ensure!(
                        rejected.status == 200,
                        "supported veto failed: {rejected:?}"
                    );
                    ("proposal_vetoed", "vetoed", Value::Null)
                } else {
                    advance_clock_to_offset(fixture, capability, 7201)?;
                    tick_scheduler(fixture, capability)?;
                    (
                        "proposal_rejected",
                        if scenario == "surface-diverged" {
                            "evidence_diverged"
                        } else {
                            "revalidation_failed"
                        },
                        json!(false),
                    )
                };
                assert_window_terminal(fixture, id, event, reason, replay.clone())?;
                fixture.restart_daemon()?;
                tick_scheduler(fixture, capability)?;
                assert_window_terminal(fixture, id, event, reason, replay)?;
                let applied: i64 = fixture.store()?.query_row(
                    "SELECT COUNT(*) FROM ward_audit WHERE event_type IN ('proposal_approved', 'apply_audit')",
                    [],
                    |row| row.get(0),
                )?;
                anyhow::ensure!(
                    applied == 0,
                    "refused proposal produced applied-write evidence"
                );
                if scenario == "surface-diverged" {
                    anyhow::ensure!(
                        fs::read_to_string(fixture.workspace.join("TOOLS.md"))?
                            == "Synthetic out-of-band tool change",
                        "replay overwrote out-of-band bytes"
                    );
                } else {
                    assert_corpus_bytes(fixture, case, "before")?;
                }
                Ok(())
            },
        )?;
    }
    Ok(())
}

#[test]
#[cfg(feature = "threads-test-clock")]
fn explicit_supersession_closes_only_the_replaced_window() -> Result<()> {
    let corpus = retired_ward_corpus()?;
    let case = retired_review_case(&corpus)?;
    let mut replacement_case = case.clone();
    replacement_case["id"] = json!("synthetic-replacement");
    for surface in replacement_case["surfaces"]
        .as_array_mut()
        .context("replacement surfaces")?
    {
        surface["after"] = json!(format!(
            "{} replacement",
            surface["after"].as_str().context("replacement contents")?
        ));
    }
    run_clocked_journey(
        "explicit-supersession",
        |home, workspace| seed_retired_review_case(home, workspace, case),
        |fixture, capability| {
            let original = submit_retired_case(fixture, case)?;
            let original_id = original["proposalId"].as_str().context("original id")?;
            tick_scheduler(fixture, capability)?;
            advance_clock_to_offset(fixture, capability, 1)?;
            let replacement = submit_retired_case(fixture, &replacement_case)?;
            let replacement_id = replacement["proposalId"]
                .as_str()
                .context("replacement id")?;
            let replacement_payload = proposal_decision_payload(fixture, replacement_id, "")?;
            let mut payload =
                proposal_decision_payload(fixture, original_id, "Synthetic explicit replacement")?;
            payload["replacementProposalId"] = json!(replacement_id);
            payload["replacementProposalRevision"] =
                replacement_payload["expectedRevision"].clone();
            let superseded = fixture.request(
                "POST",
                &format!("/api/v1/threads/proposals/{original_id}/reject"),
                Some(&payload),
            )?;
            anyhow::ensure!(
                superseded.status == 200,
                "explicit replacement failed: {superseded:?}"
            );
            assert_window_terminal(
                fixture,
                original_id,
                "proposal_rejected",
                "superseded",
                Value::Null,
            )?;
            assert_corpus_bytes(fixture, case, "before")?;
            fixture.restart_daemon()?;
            tick_scheduler(fixture, capability)?;
            let repeated = fixture.request(
                "POST",
                &format!("/api/v1/threads/proposals/{original_id}/reject"),
                Some(&payload),
            )?;
            anyhow::ensure!(
                repeated.status == 200 && repeated.body["idempotent"] == true,
                "supersession retry lost its durable terminal receipt: {repeated:?}"
            );
            assert_window_terminal(
                fixture,
                original_id,
                "proposal_rejected",
                "superseded",
                Value::Null,
            )?;
            advance_clock_to_offset(fixture, capability, 7202)?;
            tick_scheduler(fixture, capability)?;
            assert_corpus_bytes(fixture, &replacement_case, "after")?;
            assert_window_terminal(
                fixture,
                replacement_id,
                "proposal_approved",
                "applied",
                json!(true),
            )?;
            Ok(())
        },
    )
}

#[test]
#[cfg(feature = "threads-test-clock")]
fn reviewed_human_approval_validates_and_audits_without_veto_window() -> Result<()> {
    let corpus = retired_ward_corpus()?;
    let case = retired_review_case(&corpus)?;
    run_clocked_journey(
        "reviewed-human-no-window",
        |home, workspace| seed_reviewed_human_case(home, workspace, case),
        |fixture, capability| {
            let staged = submit_retired_case(fixture, case)?;
            let id = staged["proposalId"].as_str().context("proposal id")?;
            anyhow::ensure!(
                staged["scheduledProposal"]["veto_deadline"].is_null()
                    && staged["scheduledProposal"]["earliest_close"].is_null(),
                "human path acquired a veto window"
            );
            tick_scheduler(fixture, capability)?;
            assert_corpus_bytes(fixture, case, "before")?;
            let payload = proposal_decision_payload(fixture, id, "Synthetic human approval")?;
            let approved = fixture.request(
                "POST",
                &format!("/api/v1/threads/proposals/{id}/approve"),
                Some(&payload),
            )?;
            anyhow::ensure!(
                approved.status == 200 && approved.body["decision"] == "approved",
                "human approval failed: {approved:?}"
            );
            assert_corpus_bytes(fixture, case, "after")?;
            fixture.restart_daemon()?;
            tick_scheduler(fixture, capability)?;
            let conn = fixture.store()?;
            let opened: i64 = conn.query_row(
                "SELECT COUNT(*) FROM ward_audit WHERE proposal_id = ?1 AND event_type = 'proposal_window_opened'",
                [id],
                |row| row.get(0),
            )?;
            let approved: i64 = conn.query_row(
                "SELECT COUNT(*) FROM ward_audit WHERE proposal_id = ?1 AND event_type = 'proposal_approved'
                 AND (detail IS NULL OR json_extract(detail, '$.window_close') IS NULL)",
                [id],
                |row| row.get(0),
            )?;
            anyhow::ensure!(
                opened == 0 && approved == 1,
                "human no-window history is inconsistent"
            );
            let validated: i64 = conn.query_row(
                "SELECT COUNT(*) FROM ward_audit WHERE event_type = 'validation_verdict'
                 AND familiar_id = ?1 AND decision = 'permit'",
                [FAMILIAR_ID],
                |row| row.get(0),
            )?;
            anyhow::ensure!(
                validated > 0,
                "bounded approval bypassed authoritative validation"
            );
            let (next, detail, touched): (Vec<u8>, String, String) = conn.query_row(
                "SELECT diff_hash, detail, files_touched FROM ward_audit WHERE event_type = 'apply_audit'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )?;
            let detail: Value = serde_json::from_str(&detail)?;
            let before = case["surfaces"][1]["before"]
                .as_str()
                .context("logged before")?;
            let after = case["surfaces"][1]["after"]
                .as_str()
                .context("logged after")?;
            anyhow::ensure!(
                next == Sha256::digest(after.as_bytes()).to_vec()
                    && detail["prev_sha256"] == hex_bytes(&Sha256::digest(before.as_bytes()))
                    && detail["bytes_written"] == after.len()
                    && serde_json::from_str::<Value>(&touched)? == json!(["HEARTBEAT.md"]),
                "applied-write receipt does not bind exact before/after bytes"
            );
            let count: i64 = conn.query_row(
                "SELECT COUNT(*) FROM ward_audit WHERE event_type = 'apply_audit'",
                [],
                |row| row.get(0),
            )?;
            anyhow::ensure!(count == 1, "logged write was applied more than once");
            drop(conn);
            let remaining = fixture.request("GET", "/api/v1/threads/proposals", None)?;
            anyhow::ensure!(
                remaining.body["proposals"] == json!([]),
                "applied write remains pending"
            );
            Ok(())
        },
    )
}

#[test]
#[cfg(feature = "threads-test-clock")]
fn out_of_band_reviewed_drift_is_refused_without_execution() -> Result<()> {
    let corpus = retired_ward_corpus()?;
    let case = retired_review_case(&corpus)?;
    run_clocked_journey(
        "out-of-band-reviewed-drift",
        |home, workspace| seed_reviewed_human_case(home, workspace, case),
        |fixture, _| {
            let protected_before = fs::read(fixture.workspace.join("SOUL.md"))?;
            let original = submit_retired_case(fixture, case)?;
            let id = original["proposalId"].as_str().context("proposal id")?;
            let payload = proposal_decision_payload(fixture, id, "Synthetic conflicting approval")?;
            let drift = "Synthetic out-of-band tool contents";
            fs::write(fixture.workspace.join("TOOLS.md"), drift)?;
            let refused = fixture.request(
                "POST",
                &format!("/api/v1/threads/proposals/{id}/approve"),
                Some(&payload),
            )?;
            anyhow::ensure!(
                refused.status == 409 && refused.body["why"] == "proposal-evidence-diverged",
                "materialized drift was not detected: {refused:?}"
            );
            let conflicting = submit_retired_case(fixture, case)?;
            let pending: Value = serde_json::from_slice(&fs::read(
                conflicting["pendingPath"]
                    .as_str()
                    .context("pending path")?,
            )?)?;
            let materialized = pending["materialized_diff"]["surfaces"]
                .as_array()
                .context("materialized surfaces")?;
            let tools = materialized
                .iter()
                .find(|surface| surface["surface"] == "TOOLS.md")
                .context("materialized tool surface")?;
            let before: Vec<u8> = serde_json::from_value(tools["before"].clone())?;
            anyhow::ensure!(
                before == drift.as_bytes(),
                "staging used stale before-image bytes"
            );
            let detail: String = fixture.store()?.query_row(
                "SELECT detail FROM ward_audit WHERE proposal_id = ?1 AND event_type = 'proposal_submitted'",
                [conflicting["proposalId"].as_str().context("conflicting proposal id")?],
                |row| row.get(0),
            )?;
            let detail: Value = serde_json::from_str(&detail)?;
            anyhow::ensure!(
                pending["classification"] == conflicting["scheduledProposal"]["classification"]
                    && detail["classification"] == pending["classification"],
                "pending, response, and audit disagree on the conflicting diff"
            );
            anyhow::ensure!(
                fs::read_to_string(fixture.workspace.join("TOOLS.md"))? == drift
                    && fs::read(fixture.workspace.join("SOUL.md"))? == protected_before,
                "a non-executed proposal overwrote governed bytes"
            );
            let applied: i64 = fixture.store()?.query_row(
                "SELECT COUNT(*) FROM ward_audit WHERE event_type IN ('apply_audit', 'proposal_approved')",
                [], |row| row.get(0),
            )?;
            anyhow::ensure!(
                applied == 0,
                "drift rejection produced applied-write evidence"
            );
            Ok(())
        },
    )
}

#[test]
fn same_home_daemon_lifecycle_helpers_survive_restart_and_crash() -> Result<()> {
    let evidence = EvidenceContext::new("same-home-daemon-lifecycle");
    let mut fixture = ThreadsFixture::start(&evidence)?;
    let journey_result = (|| {
        fixture.restart_daemon()?;
        let restarted = fixture.request("GET", "/health", None)?;
        anyhow::ensure!(
            restarted.status == 200 && restarted.body["ok"] == true,
            "daemon was not healthy after same-home restart: {restarted:?}"
        );

        fixture.stop_daemon()?;
        fixture.start_daemon()?;
        let restarted_after_stop = fixture.request("GET", "/health", None)?;
        anyhow::ensure!(
            restarted_after_stop.status == 200 && restarted_after_stop.body["ok"] == true,
            "daemon was not healthy after same-home stop/start: {restarted_after_stop:?}"
        );

        fixture.stop_daemon()?;
        fixture.restart_daemon()?;
        let restarted_from_stopped = fixture.request("GET", "/health", None)?;
        anyhow::ensure!(
            restarted_from_stopped.status == 200 && restarted_from_stopped.body["ok"] == true,
            "daemon was not healthy after restart from stopped: {restarted_from_stopped:?}"
        );

        fixture.crash_daemon()?;
        fixture.start_daemon()?;
        let restarted_after_crash = fixture.request("GET", "/health", None)?;
        anyhow::ensure!(
            restarted_after_crash.status == 200 && restarted_after_crash.body["ok"] == true,
            "daemon was not healthy after crash recovery start: {restarted_after_crash:?}"
        );
        Ok(())
    })();
    finalize_journey_with_artifact_check(&mut fixture, journey_result, |fixture| {
        let manifest: Value =
            serde_json::from_slice(&fs::read(fixture.artifact_dir.join("manifest.json"))?)?;
        anyhow::ensure!(
            manifest["result"] == "passed",
            "successful lifecycle run did not persist passed provenance: {manifest}"
        );
        let lifecycle = manifest["daemon_lifecycle"]
            .as_array()
            .context("success manifest is missing daemon_lifecycle")?;
        let operations = lifecycle
            .iter()
            .map(|event| {
                event["operation"]
                    .as_str()
                    .context("daemon lifecycle entry is missing operation")
            })
            .collect::<Result<Vec<_>>>()?;
        anyhow::ensure!(
            operations
                == [
                    "daemon start",
                    "daemon restart",
                    "daemon stop",
                    "daemon start",
                    "daemon stop",
                    "daemon restart",
                    "daemon crash",
                    "daemon start",
                    "daemon stop",
                ],
            "unexpected lifecycle sequence in success provenance: {operations:?}"
        );

        let daemon_log = fs::read_to_string(fixture.artifact_dir.join("logs/daemon.log"))?;
        anyhow::ensure!(
            !daemon_log.contains(&fixture.coven_home.display().to_string())
                && !daemon_log.contains(&fixture.workspace.display().to_string()),
            "success daemon log leaked unsanitized fixture paths:\n{daemon_log}"
        );
        anyhow::ensure!(
            daemon_log.contains(sanitized_daemon_endpoint())
                && !daemon_log.contains("/private<coven-home>"),
            "success daemon log did not retain the sanitized socket placeholder:\n{daemon_log}"
        );

        let response: Value =
            serde_json::from_slice(&fs::read(fixture.artifact_dir.join("response.json"))?)?;
        anyhow::ensure!(
            response["body"]["daemon"]["socket"] == sanitized_daemon_endpoint(),
            "success response provenance did not sanitize the socket path: {response}"
        );
        Ok(())
    })
}

fn run_journey(name: &str, journey: impl FnOnce(&mut ThreadsFixture) -> Result<()>) -> Result<()> {
    run_fixture_journey(name, ThreadsFixture::start, journey)
}

#[test]
fn startup_failure_retains_partial_fixture_evidence_before_cleanup() -> Result<()> {
    let artifacts = tempfile::tempdir()?;
    let mut evidence = EvidenceContext::new("startup-retention-regression");
    evidence.artifact_dir = artifacts.path().to_path_buf();
    let mut fixture_home = None;
    let result = ThreadsFixture::start_with_setup_and_start(
        &evidence,
        |home, workspace| {
            fs::create_dir_all(home.join("pending"))?;
            fs::write(home.join("pending/retained.txt"), b"synthetic pending")?;
            fs::write(workspace.join("retained.txt"), b"synthetic workspace")?;
            fs::write(
                home.join("daemon-recovery.log"),
                format!("startup checkpoint at {}\n", home.display()),
            )?;
            fs::write(home.join("daemon.json"), b"{\"pid\":0}")?;
            // A partially initialized store must not prevent the other captures.
            Connection::open(home.join("coven.sqlite3"))?;
            Ok(())
        },
        |fixture| {
            fixture_home = Some(fixture.coven_home.clone());
            // No daemon was launched by this deterministic failure seam.
            fixture.stopped = true;
            #[cfg(unix)]
            let status = {
                use std::os::unix::process::ExitStatusExt;
                std::process::ExitStatus::from_raw(256)
            };
            #[cfg(windows)]
            let status = {
                use std::os::windows::process::ExitStatusExt;
                std::process::ExitStatus::from_raw(1)
            };
            fixture.complete_daemon_start(
                None,
                Output {
                    status,
                    stdout: Vec::new(),
                    stderr: b"synthetic startup failure".to_vec(),
                },
            )
        },
    );
    let error = result.err().context("setup should fail")?;
    assert!(
        !fixture_home.as_ref().unwrap().exists(),
        "temporary home leaked"
    );
    evidence.write_setup_failure(&error)?;
    let manifest: Value =
        serde_json::from_slice(&fs::read(artifacts.path().join("manifest.json"))?)?;
    assert_manifest_command_matches_build(&manifest);
    assert_eq!(manifest["setup_completed"], false);
    assert!(manifest["coven_commit"].is_string());
    assert_eq!(manifest["daemon_lifecycle"][0]["command_status"], 1);
    let log = fs::read_to_string(artifacts.path().join("logs/daemon.log"))?;
    assert!(log.contains("synthetic startup failure"));
    assert!(log.contains("startup checkpoint at <coven-home>"));
    assert!(!log.contains(&fixture_home.unwrap().display().to_string()));
    assert!(
        fs::read_to_string(artifacts.path().join("state/pending-tree.txt"))?
            .contains("retained.txt")
    );
    assert!(
        fs::read_to_string(artifacts.path().join("state/workspace-tree.txt"))?
            .contains("retained.txt")
    );
    assert!(
        fs::read_to_string(artifacts.path().join("state/ward-audit.jsonl"))?
            .contains("capture-error")
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&fs::read(
            artifacts.path().join("state/daemon-status.json")
        )?)?["pid"],
        0
    );
    Ok(())
}

fn run_fixture_journey(
    name: &str,
    initialize: impl FnOnce(&EvidenceContext) -> Result<ThreadsFixture>,
    journey: impl FnOnce(&mut ThreadsFixture) -> Result<()>,
) -> Result<()> {
    let evidence = EvidenceContext::new(name);
    let mut fixture = match initialize(&evidence) {
        Ok(fixture) => fixture,
        Err(error) => {
            evidence.write_setup_failure(&error)?;
            return Err(error);
        }
    };
    let journey_result = journey(&mut fixture);
    finalize_journey(&mut fixture, journey_result)
}

#[cfg(feature = "threads-test-clock")]
fn run_clocked_journey(
    name: &str,
    prepare: impl FnOnce(&Path, &Path) -> Result<()>,
    journey: impl FnOnce(&mut ThreadsFixture, &str) -> Result<()>,
) -> Result<()> {
    let capability = Uuid::new_v4().to_string();
    run_fixture_journey(
        name,
        |evidence| {
            ThreadsFixture::start_with_setup(evidence, |home, workspace| {
                seed_clock(home, &capability)?;
                prepare(home, workspace)
            })
        },
        |fixture| journey(fixture, &capability),
    )
}

#[cfg(feature = "threads-test-clock")]
fn seed_clock(coven_home: &Path, capability: &str) -> Result<()> {
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    let root = coven_home.join("test-fixtures");
    let directory = root.join("threads-deterministic-clock");
    fs::create_dir_all(&directory)?;
    #[cfg(unix)]
    {
        for path in [&root, &directory] {
            fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
        }
    }
    for (name, contents) in [
        ("enabled", "threads_test_clock_v1\n"),
        ("capability", capability),
        ("state.json", r#"{"now":"2099-01-01T00:00:00Z"}"#),
    ] {
        write_private_fixture_file(&directory.join(name), contents)?;
    }
    Ok(())
}

#[cfg(feature = "threads-test-clock")]
fn write_private_fixture_file(path: &Path, contents: &str) -> Result<()> {
    #[cfg(unix)]
    use std::os::unix::fs::OpenOptionsExt;

    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options
        .open(path)
        .with_context(|| format!("creating private fixture file {}", path.display()))?;
    file.write_all(contents.as_bytes())
        .with_context(|| format!("writing private fixture file {}", path.display()))?;
    file.sync_all()
        .with_context(|| format!("syncing private fixture file {}", path.display()))?;
    Ok(())
}

#[cfg(feature = "threads-test-clock")]
fn advance_clock(fixture: &mut ThreadsFixture, capability: &str, now: &str) -> Result<()> {
    let response = fixture.request(
        "POST",
        "/api/v1/internal/threads/test-clock",
        Some(&json!({"capability": capability, "now": now})),
    )?;
    anyhow::ensure!(
        response.status == 200
            && response.body["now"] == now
            && response.body["source"] == "deterministic_fixture",
        "clock advance failed: {response:?}"
    );
    Ok(())
}

#[cfg(feature = "threads-test-clock")]
fn retired_ward_corpus() -> Result<Value> {
    let dependency = threads_dependency(&workspace_root())?;
    let output = Command::new("cargo")
        .args(["run", "--quiet", "--locked", "--manifest-path"])
        .arg(&dependency.manifest_path)
        .args([
            "--example",
            "generate_phase5_retired_ward_corpus",
            "--target-dir",
        ])
        .arg(workspace_root().join("target/threads-corpus-generator"))
        .env("CARGO_INCREMENTAL", "0")
        .output()
        .context("generating corpus from the resolved Threads checkout")?;
    anyhow::ensure!(
        output.status.success(),
        "canonical corpus generator failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let corpus: Value = serde_json::from_slice(&output.stdout)?;
    anyhow::ensure!(
        corpus["schema_version"] == "phase5-retired-ward-synthetic-v1"
            && corpus["provenance"]["kind"] == "synthetic"
            && corpus["provenance"]["historical_data_used"] == false,
        "fixture is not the repository-owned synthetic corpus"
    );
    Ok(corpus)
}

#[cfg(feature = "threads-test-clock")]
fn retired_review_case(corpus: &Value) -> Result<&Value> {
    corpus["valid_cases"]
        .as_array()
        .context("corpus valid cases")?
        .iter()
        .find(|case| case["id"] == "familiar-review")
        .context("canonical familiar-review case")
}

#[cfg(feature = "threads-test-clock")]
fn seed_retired_review_case(home: &Path, workspace: &Path, case: &Value) -> Result<()> {
    let facts = case["candidate_facts"]
        .as_array()
        .context("candidate facts")?;
    let fact = |name: &str| -> Result<&str> {
        facts
            .iter()
            .find(|fact| fact["fact"] == name)
            .and_then(|fact| fact["value"].as_str())
            .with_context(|| format!("missing corpus fact {name}"))
    };
    let name = fact("name")?;
    fs::write(
        home.join("familiars.toml"),
        format!(
            "[[familiar]]\nid = \"{FAMILIAR_ID}\"\ndisplay_name = {}\nname = {}\n\
             person = {}\npronouns = {}\ncoven = {}\nrole = \"Synthetic fixture\"\n\
             description = \"Repository-authored synthetic corpus fixture.\"\n",
            serde_json::to_string(name)?,
            serde_json::to_string(name)?,
            serde_json::to_string(fact("person")?)?,
            serde_json::to_string(fact("pronouns")?)?,
            serde_json::to_string(fact("coven")?)?,
        ),
    )?;
    fs::write(
        workspace.join("SOUL.md"),
        format!("# I am {name}\nMy purpose is {}\n", fact("purpose")?),
    )?;
    fs::write(
        workspace.join("IDENTITY.md"),
        format!(
            "# IDENTITY.md - {name}\n- **Pronouns:** {}\n",
            fact("pronouns")?
        ),
    )?;
    let surfaces = case["surfaces"].as_array().context("corpus surfaces")?;
    let mut paths = Vec::new();
    for surface in surfaces {
        let path = surface["path"].as_str().context("corpus surface path")?;
        anyhow::ensure!(
            matches!(path, "TOOLS.md" | "HEARTBEAT.md"),
            "review fixture contains an unsupported surface"
        );
        paths.push(path);
        fs::write(
            workspace.join(path),
            surface["before"].as_str().context("before image")?,
        )?;
    }
    let duration = case["approval"]["veto"]["duration_seconds"]
        .as_u64()
        .context("corpus veto duration")?;
    let minimum = case["approval"]["veto"]["min_visible_seconds"]
        .as_u64()
        .context("corpus minimum visibility")?;
    anyhow::ensure!(duration % 3600 == 0, "retired Ward requires whole hours");
    let legacy = format!(
        "[meta]\nversion = \"0.1.0\"\nowner = \"{FAMILIAR_ID}\"\n\
         [protected]\nfiles = [\"SOUL.md\", \"IDENTITY.md\"]\ninvariants = {}\n\
         [editable]\npaths = {}\nharness_blocks = {}\n\
         [approval_tiers.familiar_review]\nblocks = {}\n\
         gate = \"familiar_coherence_check\"\nhuman_veto_window_hours = {}\n\
         min_visible_seconds = {minimum}\n",
        case["declarations"],
        serde_json::to_string(&paths)?,
        case["expected"]["regions"],
        case["expected"]["regions"],
        duration / 3600,
    );
    fs::write(workspace.join("ward.toml"), &legacy)?;
    let migrated = run_coven(
        Path::new(env!("CARGO_BIN_EXE_coven")),
        home,
        &std::env::var_os("PATH").unwrap_or_default(),
        &[
            "ward",
            "migrate",
            "--familiar",
            FAMILIAR_ID,
            "--fingerprint",
            PRINCIPAL_FINGERPRINT,
            "--apply",
        ],
    )?;
    anyhow::ensure!(
        migrated.status.success(),
        "real migration rejected the corpus: {} {}",
        String::from_utf8_lossy(&migrated.stdout),
        String::from_utf8_lossy(&migrated.stderr)
    );
    anyhow::ensure!(
        fs::read_to_string(workspace.join("ward.toml.v01.bak"))? == legacy,
        "migration did not preserve the exact original declaration"
    );
    Ok(())
}

#[cfg(feature = "threads-test-clock")]
fn seed_reviewed_human_case(home: &Path, workspace: &Path, case: &Value) -> Result<()> {
    seed_retired_review_case(home, workspace, case)?;
    let path = workspace.join("ward.toml");
    let mut ward: toml::Value = toml::from_str(&fs::read_to_string(&path)?)?;
    let tiers = ward["approval_tiers"]
        .as_table_mut()
        .context("approval tiers")?;
    let mut human = tiers
        .remove("familiar_review")
        .context("review declaration")?;
    let declaration = human.as_table_mut().context("review declaration table")?;
    declaration.insert(
        "gate".to_owned(),
        toml::Value::String("human_approval".to_owned()),
    );
    declaration.remove("human_veto_window_hours");
    declaration.remove("min_visible_seconds");
    tiers.insert("human_review".to_owned(), human);
    for surface in ward["surface"].as_array_mut().context("Ward surfaces")? {
        if surface["path"].as_str() == Some("HEARTBEAT.md") {
            surface["tier"] = toml::Value::Integer(2);
        }
    }
    fs::write(path, toml::to_string(&ward)?)?;
    Ok(())
}

#[cfg(feature = "threads-test-clock")]
fn submit_retired_case(fixture: &mut ThreadsFixture, case: &Value) -> Result<Value> {
    let edits: Vec<Value> = case["surfaces"]
        .as_array()
        .context("corpus surfaces")?
        .iter()
        .map(|surface| json!({"target": surface["path"], "contents": surface["after"]}))
        .collect();
    let response = fixture.request(
        "POST",
        "/api/v1/familiars/sage/edits",
        Some(&json!({"edits": edits, "principalKeyFingerprint": PRINCIPAL_FINGERPRINT})),
    )?;
    anyhow::ensure!(
        response.status == 202
            && response.body["disposition"] == "staged"
            && response.body["scheduledProposal"]["schema"] == "phase5_v1",
        "supported intake did not publish canonical scheduled evidence: {response:?}"
    );
    Ok(response.body)
}

#[cfg(feature = "threads-test-clock")]
fn proposal_decision_payload(fixture: &mut ThreadsFixture, id: &str, note: &str) -> Result<Value> {
    let listed = fixture.request("GET", "/api/v1/threads/proposals", None)?;
    anyhow::ensure!(
        listed.status == 200,
        "proposal inspection failed: {listed:?}"
    );
    let proposal = listed.body["proposals"]
        .as_array()
        .context("listed proposals")?
        .iter()
        .find(|proposal| proposal["proposalId"] == id)
        .context("proposal is not pending")?;
    let revision = proposal["proposalRevision"]
        .as_str()
        .context("proposal revision")?;
    Ok(json!({
        "expectedRevision": revision,
        "principalKeyFingerprint": PRINCIPAL_FINGERPRINT,
        "note": note,
    }))
}

#[cfg(feature = "threads-test-clock")]
fn fixture_time(seconds: i64) -> Result<time::OffsetDateTime> {
    Ok(time::OffsetDateTime::parse(
        "2099-01-01T00:00:00Z",
        &time::format_description::well_known::Rfc3339,
    )? + time::Duration::seconds(seconds))
}

#[cfg(feature = "threads-test-clock")]
fn advance_clock_to_offset(
    fixture: &mut ThreadsFixture,
    capability: &str,
    seconds: i64,
) -> Result<()> {
    advance_clock(
        fixture,
        capability,
        &fixture_time(seconds)?.format(&time::format_description::well_known::Rfc3339)?,
    )
}

#[cfg(feature = "threads-test-clock")]
fn tick_scheduler(fixture: &mut ThreadsFixture, capability: &str) -> Result<()> {
    let response = fixture.request(
        "POST",
        "/api/v1/internal/threads/test-clock/tick",
        Some(&json!({"capability": capability})),
    )?;
    anyhow::ensure!(
        response.status == 200 && response.body["source"] == "deterministic_fixture",
        "controlled scheduler failed: {response:?}"
    );
    Ok(())
}

#[cfg(feature = "threads-test-clock")]
fn assert_corpus_bytes(fixture: &ThreadsFixture, case: &Value, image: &str) -> Result<()> {
    for surface in case["surfaces"].as_array().context("corpus surfaces")? {
        anyhow::ensure!(
            fs::read_to_string(
                fixture
                    .workspace
                    .join(surface["path"].as_str().context("surface")?)
            )? == surface[image].as_str().context("corpus image")?,
            "corpus bytes differ from the required {image} image"
        );
    }
    Ok(())
}

#[cfg(feature = "threads-test-clock")]
fn assert_window_terminal(
    fixture: &ThreadsFixture,
    id: &str,
    event: &str,
    reason: &str,
    replay_matched: Value,
) -> Result<()> {
    let conn = fixture.store()?;
    let opened: i64 = conn.query_row(
        "SELECT COUNT(*) FROM ward_audit WHERE proposal_id = ?1 AND event_type = 'proposal_window_opened'",
        [id],
        |row| row.get(0),
    )?;
    let mut statement = conn.prepare(
        "SELECT event_type, detail FROM ward_audit WHERE proposal_id = ?1
         AND event_type IN ('proposal_approved', 'proposal_vetoed', 'proposal_rejected')",
    )?;
    let rows = statement
        .query_map([id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    anyhow::ensure!(
        opened == 1 && rows.len() == 1,
        "window/terminal multiplicity mismatch: {opened}, {rows:?}"
    );
    let detail: Value = serde_json::from_str(&rows[0].1)?;
    let close = if event == "proposal_approved" {
        &detail["window_close"]
    } else {
        &detail
    };
    anyhow::ensure!(
        rows[0].0 == event
            && close["reason"] == reason
            && close["replay_hash_matched"] == replay_matched,
        "wrong typed terminal family: {rows:?}"
    );
    Ok(())
}

fn finalize_journey(fixture: &mut ThreadsFixture, journey_result: Result<()>) -> Result<()> {
    finalize_journey_with_artifact_check(fixture, journey_result, |_| Ok(()))
}

fn finalize_journey_with_artifact_check(
    fixture: &mut ThreadsFixture,
    journey_result: Result<()>,
    verify_artifacts: impl FnOnce(&ThreadsFixture) -> Result<()>,
) -> Result<()> {
    let shutdown_result = fixture.shutdown();
    let mut result = match (journey_result, shutdown_result) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) => Err(error),
        (Ok(()), Err(error)) => Err(error.context("journey passed but daemon shutdown failed")),
        (Err(error), Err(shutdown)) => Err(error.context(format!(
            "daemon shutdown also failed after the journey error: {shutdown:#}"
        ))),
    };
    fixture.write_junit(result.as_ref().err())?;
    fixture.write_run_provenance(result.as_ref().err())?;
    if result.is_ok() {
        if let Err(error) = verify_artifacts(fixture) {
            result = Err(error);
            fixture.write_junit(result.as_ref().err())?;
            fixture.write_run_provenance(result.as_ref().err())?;
        }
    }
    if result.is_err() {
        fixture.write_failure_evidence()?;
    }
    result
}

#[derive(Debug)]
struct HttpResponse {
    status: u16,
    body: Value,
}

#[derive(Debug)]
struct DaemonLifecycleEvent {
    operation: &'static str,
    pid_before: Option<u32>,
    pid_after: Option<u32>,
    command_status: Option<i32>,
    stdout: Option<String>,
    stderr: Option<String>,
    note: Option<String>,
}

impl DaemonLifecycleEvent {
    fn from_output(
        operation: &'static str,
        pid_before: Option<u32>,
        pid_after: Option<u32>,
        output: &Output,
        note: Option<String>,
    ) -> Self {
        Self {
            operation,
            pid_before,
            pid_after,
            command_status: output.status.code(),
            stdout: Some(String::from_utf8_lossy(&output.stdout).into_owned()),
            stderr: Some(String::from_utf8_lossy(&output.stderr).into_owned()),
            note,
        }
    }

    fn note(operation: &'static str, pid_before: Option<u32>, note: impl Into<String>) -> Self {
        Self {
            operation,
            pid_before,
            pid_after: None,
            command_status: None,
            stdout: None,
            stderr: None,
            note: Some(note.into()),
        }
    }

    fn as_json(&self) -> Value {
        json!({
            "operation": self.operation,
            "pid_before": self.pid_before,
            "pid_after": self.pid_after,
            "command_status": self.command_status,
            "note": self.note,
        })
    }

    fn render_log(&self) -> String {
        let mut output = format!("event: {}\n", self.operation);
        if let Some(pid_before) = self.pid_before {
            output.push_str(&format!("pid_before: {pid_before}\n"));
        }
        if let Some(pid_after) = self.pid_after {
            output.push_str(&format!("pid_after: {pid_after}\n"));
        }
        if let Some(command_status) = self.command_status {
            output.push_str(&format!("command_status: {command_status}\n"));
        }
        if let Some(note) = &self.note {
            output.push_str(&format!("note: {note}\n"));
        }
        if let Some(stdout) = &self.stdout {
            output.push_str("stdout:\n");
            output.push_str(stdout);
            if !stdout.ends_with('\n') {
                output.push('\n');
            }
        }
        if let Some(stderr) = &self.stderr {
            output.push_str("stderr:\n");
            output.push_str(stderr);
            if !stderr.ends_with('\n') {
                output.push('\n');
            }
        }
        output
    }
}

struct ThreadsFixture {
    _temp: tempfile::TempDir,
    coven: PathBuf,
    coven_home: PathBuf,
    workspace: PathBuf,
    #[cfg(windows)]
    pipe_names: [String; 3],
    path: OsString,
    run_id: String,
    scenario: String,
    artifact_dir: PathBuf,
    coven_state: GitState,
    threads_state: GitState,
    coven_manifest_sha256: String,
    coven_lock_sha256: String,
    threads_manifest_sha256: String,
    local_threads_override_active: bool,
    last_request: Option<Value>,
    last_response: Option<Value>,
    daemon_events: Vec<DaemonLifecycleEvent>,
    daemon_pid: Option<u32>,
    stopped: bool,
}

impl ThreadsFixture {
    fn start(evidence: &EvidenceContext) -> Result<Self> {
        Self::start_with_setup(evidence, |_, _| Ok(()))
    }

    fn start_with_setup(
        evidence: &EvidenceContext,
        prepare: impl FnOnce(&Path, &Path) -> Result<()>,
    ) -> Result<Self> {
        Self::start_with_setup_and_start(evidence, prepare, Self::start_daemon)
    }

    fn start_with_setup_and_start(
        evidence: &EvidenceContext,
        prepare: impl FnOnce(&Path, &Path) -> Result<()>,
        start: impl FnOnce(&mut Self) -> Result<()>,
    ) -> Result<Self> {
        let workspace_root = workspace_root();
        let dependency = threads_dependency(&workspace_root)?;
        if std::env::var_os(REQUIRE_OVERRIDE_ENV).is_some() {
            anyhow::ensure!(
                dependency.local_override_active,
                "{REQUIRE_OVERRIDE_ENV}=1, but cargo metadata resolved coven-threads-core from {}",
                dependency.manifest_path.display()
            );
        }
        let coven_state = git_state(&workspace_root)?;
        let threads_state = git_state(
            dependency
                .manifest_path
                .parent()
                .context("Threads manifest has no parent directory")?,
        )?;
        let coven_manifest_sha256 =
            file_sha256(&workspace_root.join("crates/coven-cli/Cargo.toml"))?;
        let coven_lock_sha256 = file_sha256(&workspace_root.join("Cargo.lock"))?;
        let threads_manifest_sha256 = file_sha256(&dependency.manifest_path)?;
        let local_threads_override_active = dependency.local_override_active;

        let temp = tempfile::tempdir()?;
        let coven_home = temp.path().join("coven-home");
        let workspace = coven_home.join("familiars").join(FAMILIAR_ID);
        fs::create_dir_all(&workspace)?;
        seed_familiar(&coven_home, &workspace)?;
        prepare(&coven_home, &workspace)?;
        #[cfg(windows)]
        let pipe_names = coven_client::supported_windows_pipe_names(&coven_home)?;

        let coven = PathBuf::from(env!("CARGO_BIN_EXE_coven"));
        let path = std::env::var_os("PATH").unwrap_or_default();
        let mut fixture = Self {
            _temp: temp,
            coven,
            coven_home,
            workspace,
            #[cfg(windows)]
            pipe_names,
            path,
            run_id: evidence.run_id.clone(),
            scenario: evidence.scenario.clone(),
            artifact_dir: evidence.artifact_dir.clone(),
            coven_state,
            threads_state,
            coven_manifest_sha256,
            coven_lock_sha256,
            threads_manifest_sha256,
            local_threads_override_active,
            last_request: None,
            last_response: None,
            daemon_events: Vec::new(),
            daemon_pid: None,
            stopped: false,
        };
        if let Err(error) = start(&mut fixture) {
            let capture = (|| {
                fixture.write_failure_evidence()?;
                fixture.write_junit(Some(&error))?;
                fixture.write_run_provenance_with_setup(Some(&error), false)
            })();
            if let Err(capture_error) = capture {
                return Err(error.context(format!(
                    "capturing setup failure evidence also failed: {capture_error:#}"
                )));
            }
            return Err(error);
        }
        Ok(fixture)
    }

    fn request(
        &mut self,
        method: &'static str,
        path: &str,
        body: Option<&Value>,
    ) -> Result<HttpResponse> {
        let mut request = body.cloned().unwrap_or(Value::Null);
        if let Some(capability) = request.get_mut("capability") {
            *capability = json!("<fixture-capability>");
        }
        self.last_request = Some(json!({
            "method": method,
            "path": path,
            "body": request,
        }));
        let (status, response) = daemon_http_request(&self.coven_home, method, path, body)?;
        let parsed: Value = serde_json::from_str(&response)
            .with_context(|| format!("daemon returned non-JSON response: {response}"))?;
        self.last_response = Some(json!({
            "status": status,
            "body": parsed,
        }));
        Ok(HttpResponse {
            status,
            body: parsed,
        })
    }

    fn store(&self) -> Result<Connection> {
        Connection::open(self.coven_home.join("coven.sqlite3")).map_err(Into::into)
    }

    fn current_daemon_pid(&self) -> Option<u32> {
        self.daemon_pid
            .filter(|pid| pid_is_alive(*pid))
            .or_else(|| {
                daemon_pid(&self.coven_home)
                    .ok()
                    .filter(|pid| pid_is_alive(*pid))
            })
    }

    fn daemon_command(&self, args: &[&str]) -> Result<Output> {
        run_coven(&self.coven, &self.coven_home, &self.path, args)
    }

    fn start_daemon(&mut self) -> Result<()> {
        let pid_before = self.current_daemon_pid();
        self.stopped = false;
        let mut output = self.daemon_command(&["daemon", "start"])?;
        if cfg!(windows)
            && !output.status.success()
            && daemon_start_timed_out_waiting_for_health_connect(output.stderr.as_slice())
        {
            output = self.daemon_command(&["daemon", "start"])?;
        }
        self.complete_daemon_start(pid_before, output)
    }

    fn complete_daemon_start(&mut self, pid_before: Option<u32>, output: Output) -> Result<()> {
        let pid_after = daemon_pid(&self.coven_home).ok();
        self.daemon_events.push(DaemonLifecycleEvent::from_output(
            "daemon start",
            pid_before,
            pid_after,
            &output,
            (!output.status.success())
                .then(|| "CLI startup failed before fixture health check".to_owned()),
        ));
        anyhow::ensure!(
            output.status.success(),
            "daemon start failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let health_result = wait_for_daemon_health(&self.coven_home);
        let pid_after = daemon_pid(&self.coven_home).ok();
        let event = self
            .daemon_events
            .last_mut()
            .context("startup event was not recorded")?;
        event.pid_after = pid_after;
        event.note = health_result
            .as_ref()
            .err()
            .map(|error| format!("daemon health check failed after start: {error:#}"));
        health_result?;

        let pid_after = pid_after.context("daemon status is missing pid after start")?;
        self.daemon_pid = Some(pid_after);
        Ok(())
    }

    fn stop_daemon(&mut self) -> Result<()> {
        if self.stopped {
            return Ok(());
        }
        let pid_before = self.current_daemon_pid();
        let output = self.daemon_command(&["daemon", "stop"])?;
        self.daemon_events.push(DaemonLifecycleEvent::from_output(
            "daemon stop",
            pid_before,
            None,
            &output,
            None,
        ));
        anyhow::ensure!(
            output.status.success(),
            "daemon stop failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        wait_for_daemon_shutdown(&self.coven_home, pid_before)?;
        self.daemon_pid = None;
        self.stopped = true;
        Ok(())
    }

    fn restart_daemon(&mut self) -> Result<()> {
        let pid_before = self.current_daemon_pid();
        self.stopped = false;
        let output = self.daemon_command(&["daemon", "restart"])?;
        anyhow::ensure!(
            output.status.success(),
            "daemon restart failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );

        let health_result = wait_for_daemon_health(&self.coven_home);
        let pid_after = daemon_pid(&self.coven_home).ok();
        self.daemon_events.push(DaemonLifecycleEvent::from_output(
            "daemon restart",
            pid_before,
            pid_after,
            &output,
            health_result
                .as_ref()
                .err()
                .map(|error| format!("daemon health check failed after restart: {error:#}")),
        ));
        health_result?;

        let pid_after = pid_after.context("daemon status is missing pid after restart")?;
        self.daemon_pid = Some(pid_after);
        anyhow::ensure!(
            pid_before != Some(pid_after),
            "daemon restart did not replace the running process {pid_after}"
        );
        Ok(())
    }

    fn crash_daemon(&mut self) -> Result<()> {
        let pid = self
            .current_daemon_pid()
            .context("daemon crash requires a running daemon")?;
        terminate_daemon_process(&self.coven_home, pid)?;
        wait_for_process_exit(pid, "crashed daemon", Duration::from_secs(3))?;
        self.daemon_events.push(DaemonLifecycleEvent::note(
            "daemon crash",
            Some(pid),
            format!("forcibly terminated daemon pid {pid}"),
        ));
        self.daemon_pid = None;
        self.stopped = false;
        Ok(())
    }

    fn shutdown(&mut self) -> Result<()> {
        self.stop_daemon()
    }

    fn write_junit(&self, error: Option<&anyhow::Error>) -> Result<()> {
        fs::create_dir_all(&self.artifact_dir)?;
        let failure = error
            .map(|error| {
                let sanitized = self.sanitize_fixture_text(&format!("{error:#}"));
                format!(
                    "<failure message=\"{}\">{}</failure>",
                    xml_escape(&self.sanitize_fixture_text(&error.to_string())),
                    xml_escape(&sanitized)
                )
            })
            .unwrap_or_default();
        let failures = usize::from(error.is_some());
        fs::write(
            self.artifact_dir.join("junit.xml"),
            format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
                 <testsuite name=\"threads-e2e\" tests=\"1\" failures=\"{failures}\">\n\
                 <testcase classname=\"threads_e2e\" name=\"{}\">{failure}</testcase>\n\
                 </testsuite>\n",
                xml_escape(&self.scenario)
            ),
        )?;
        Ok(())
    }

    fn write_run_provenance(&self, error: Option<&anyhow::Error>) -> Result<()> {
        self.write_run_provenance_with_setup(error, true)
    }

    fn write_run_provenance_with_setup(
        &self,
        error: Option<&anyhow::Error>,
        setup_completed: bool,
    ) -> Result<()> {
        fs::create_dir_all(&self.artifact_dir)?;
        let logs = self.artifact_dir.join("logs");
        fs::create_dir_all(&logs)?;
        let manifest = json!({
            "run_id": self.run_id,
            "scenario": self.scenario,
            "command": REPRODUCTION_COMMAND,
            "platform": std::env::consts::OS,
            "setup_completed": setup_completed,
            "fixture_evidence_captured": true,
            "result": if error.is_some() { "failed" } else { "passed" },
            "coven_commit": self.coven_state.commit,
            "coven_dirty": self.coven_state.dirty,
            "coven_state_sha256": self.coven_state.state_sha256,
            "coven_manifest_sha256": self.coven_manifest_sha256,
            "coven_lock_sha256": self.coven_lock_sha256,
            "threads_commit": self.threads_state.commit,
            "threads_dirty": self.threads_state.dirty,
            "threads_state_sha256": self.threads_state.state_sha256,
            "threads_manifest_sha256": self.threads_manifest_sha256,
            "local_threads_override_active": self.local_threads_override_active,
            "authorization_limitation": "synthetic principal fingerprint uses the strongest current daemon-owned Ward path; signed principal proof is not yet available",
            "daemon_lifecycle": self.daemon_events.iter().map(DaemonLifecycleEvent::as_json).collect::<Vec<_>>(),
            "failure": error.map(|error| self.sanitize_fixture_text(&format!("{error:#}"))),
        });
        fs::write(
            self.artifact_dir.join("request.json"),
            serde_json::to_vec_pretty(&self.sanitized_json(&self.last_request))?,
        )?;
        fs::write(
            self.artifact_dir.join("response.json"),
            serde_json::to_vec_pretty(&self.sanitized_json(&self.last_response))?,
        )?;

        let recovery_log = match fs::read(self.coven_home.join("daemon-recovery.log")) {
            Ok(log) => log,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                b"<no daemon recovery log>\n".to_vec()
            }
            Err(error) => format!("<capture-error: {error}>\n").into_bytes(),
        };
        let mut daemon_log = String::new();
        if self.daemon_events.is_empty() {
            daemon_log.push_str("<no daemon lifecycle events recorded>\n");
        } else {
            for event in &self.daemon_events {
                daemon_log.push_str(&event.render_log());
                daemon_log.push('\n');
            }
        }
        daemon_log.push_str("daemon recovery log:\n");
        daemon_log.push_str(&String::from_utf8_lossy(&recovery_log));
        fs::write(
            logs.join("daemon.log"),
            self.sanitize_fixture_text(&daemon_log),
        )?;
        fs::write(
            self.artifact_dir.join("manifest.json"),
            serde_json::to_vec_pretty(&sanitize_json_strings(&manifest, &|text| {
                self.sanitize_fixture_text(text)
            }))?,
        )?;
        Ok(())
    }

    fn write_failure_evidence(&self) -> Result<()> {
        let state = self.artifact_dir.join("state");
        fs::create_dir_all(&state)?;
        for (name, capture) in [
            (
                "ward-audit.jsonl",
                ward_audit_jsonl(&self.coven_home.join("coven.sqlite3")),
            ),
            (
                "pending-tree.txt",
                inventory(&self.coven_home.join("pending")),
            ),
            ("workspace-tree.txt", inventory(&self.workspace)),
            (
                "sqlite-schema.txt",
                sqlite_schema(&self.coven_home.join("coven.sqlite3")),
            ),
        ] {
            let contents = match capture {
                Ok(contents) => contents,
                Err(error) => format!("<capture-error: {error:#}>\n"),
            };
            fs::write(state.join(name), self.sanitize_fixture_text(&contents))?;
        }
        let status = capture_daemon_status(&self.coven_home);
        fs::write(
            state.join("daemon-status.json"),
            serde_json::to_vec_pretty(&sanitize_json_strings(&status, &|text| {
                self.sanitize_fixture_text(text)
            }))?,
        )?;
        Ok(())
    }

    fn sanitized_json(&self, value: &Option<Value>) -> Option<Value> {
        value
            .as_ref()
            .map(|value| sanitize_json_strings(value, &|text| self.sanitize_fixture_text(text)))
    }

    fn sanitize_fixture_text(&self, value: &str) -> String {
        #[cfg(windows)]
        let value = self.pipe_names.iter().fold(value.to_owned(), |text, name| {
            text.replace(name, sanitized_daemon_endpoint())
        });
        #[cfg(windows)]
        let value = value.as_str();
        sanitize_for_artifact(&replace_sanitized_path(
            replace_sanitized_path(value.to_owned(), &self.workspace, "<familiar-workspace>"),
            &self.coven_home,
            "<coven-home>",
        ))
    }
}

impl Drop for ThreadsFixture {
    fn drop(&mut self) {
        if self.stopped {
            return;
        }
        let pid = self.current_daemon_pid();
        let stopped = self
            .daemon_command(&["daemon", "stop"])
            .is_ok_and(|output| {
                output.status.success() && wait_for_daemon_shutdown(&self.coven_home, pid).is_ok()
            });
        if let Some(pid) = pid.filter(|pid| pid_is_alive(*pid)) {
            eprintln!(
                "threads E2E fallback is terminating daemon pid {} after graceful stop success={stopped}",
                pid,
            );
            if let Err(error) = terminate_daemon_process(&self.coven_home, pid) {
                eprintln!("failed terminating fixture daemon pid {pid}: {error:#}");
            }
        }
    }
}

struct ThreadsDependency {
    manifest_path: PathBuf,
    local_override_active: bool,
}

struct EvidenceContext {
    run_id: String,
    scenario: String,
    artifact_dir: PathBuf,
}

fn assert_manifest_command_matches_build(manifest: &Value) {
    let mut expected = vec![
        "cargo",
        "test",
        "--locked",
        "-p",
        "coven-cli",
        "--test",
        "threads_e2e",
    ];
    if cfg!(feature = "threads-test-clock") {
        expected.extend(["--features", "threads-test-clock"]);
    }
    expected.extend(["--", "--nocapture"]);
    let command = manifest["command"].as_str().expect("manifest command");
    assert_eq!(command.split_whitespace().collect::<Vec<_>>(), expected);
}

fn write_missing_setup_artifact(path: impl AsRef<Path>, contents: impl AsRef<[u8]>) -> Result<()> {
    use std::io::Write;

    let mut file = match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    file.write_all(contents.as_ref())?;
    Ok(())
}

#[test]
fn startup_failure_fallback_preserves_partially_captured_artifacts() -> Result<()> {
    let artifacts = tempfile::tempdir()?;
    let mut evidence = EvidenceContext::new("startup-partial-retention");
    evidence.artifact_dir = artifacts.path().to_path_buf();
    fs::create_dir_all(artifacts.path().join("logs"))?;
    fs::write(
        artifacts.path().join("logs/daemon.log"),
        b"retained startup evidence\n",
    )?;
    evidence.write_setup_failure(&anyhow::anyhow!("synthetic capture failure"))?;
    assert_eq!(
        fs::read_to_string(artifacts.path().join("logs/daemon.log"))?,
        "retained startup evidence\n"
    );
    Ok(())
}

#[test]
fn artifact_root_override_isolates_setup_failures_from_cached_evidence() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let cached = temp.path().join("target/e2e-artifacts/historical");
    fs::create_dir_all(&cached)?;
    fs::write(cached.join("manifest.json"), b"historical evidence")?;
    let root = temp.path().join("runner-temp/run-123-attempt-2");
    let evidence =
        EvidenceContext::with_artifact_root("isolated-root", Some(root.clone().into_os_string()));
    assert_eq!(evidence.artifact_dir.parent(), Some(root.as_path()));
    evidence.write_setup_failure(&anyhow::anyhow!("synthetic setup failure"))?;
    let manifest: Value =
        serde_json::from_slice(&fs::read(evidence.artifact_dir.join("manifest.json"))?)?;
    assert_manifest_command_matches_build(&manifest);
    assert_eq!(fs::read_dir(&root)?.count(), 1);
    assert_eq!(
        fs::read(cached.join("manifest.json"))?,
        b"historical evidence"
    );
    let next = EvidenceContext::with_artifact_root("isolated-root", Some(root.into_os_string()));
    assert_ne!(evidence.artifact_dir, next.artifact_dir);
    Ok(())
}

#[test]
fn artifact_root_default_preserves_local_evidence_location() {
    let evidence = EvidenceContext::with_artifact_root("local-root", None);
    assert_eq!(
        evidence.artifact_dir,
        workspace_root()
            .join("target/e2e-artifacts")
            .join(&evidence.run_id)
    );
}

impl EvidenceContext {
    fn new(scenario: &str) -> Self {
        Self::with_artifact_root(scenario, std::env::var_os(ARTIFACT_ROOT_ENV))
    }

    fn with_artifact_root(scenario: &str, root: Option<OsString>) -> Self {
        let run_id = format!("{}-{}-{}", scenario, std::process::id(), Uuid::new_v4());
        let artifact_dir = root
            .map(PathBuf::from)
            .unwrap_or_else(|| workspace_root().join("target/e2e-artifacts"))
            .join(&run_id);
        Self {
            run_id,
            scenario: scenario.to_owned(),
            artifact_dir,
        }
    }

    fn write_setup_failure(&self, error: &anyhow::Error) -> Result<()> {
        let manifest_path = self.artifact_dir.join("manifest.json");
        if manifest_path.exists() {
            let manifest: Value = serde_json::from_slice(&fs::read(&manifest_path)?)?;
            if manifest["fixture_evidence_captured"] == true {
                return Ok(());
            }
        }
        fs::create_dir_all(&self.artifact_dir)?;
        let logs = self.artifact_dir.join("logs");
        let state = self.artifact_dir.join("state");
        fs::create_dir_all(&logs)?;
        fs::create_dir_all(&state)?;
        let sanitized = sanitize_for_artifact(&format!("{error:#}"));
        let failure = format!(
            "<failure message=\"{}\">{}</failure>",
            xml_escape(&sanitize_for_artifact(&error.to_string())),
            xml_escape(&sanitized)
        );
        write_missing_setup_artifact(
            self.artifact_dir.join("junit.xml"),
            format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
                 <testsuite name=\"threads-e2e\" tests=\"1\" failures=\"1\">\n\
                 <testcase classname=\"threads_e2e\" name=\"{}\">{failure}</testcase>\n\
                 </testsuite>\n",
                xml_escape(&self.scenario)
            ),
        )?;
        write_missing_setup_artifact(
            self.artifact_dir.join("manifest.json"),
            serde_json::to_vec_pretty(&json!({
                "run_id": self.run_id,
                "scenario": self.scenario,
                "command": REPRODUCTION_COMMAND,
                "platform": std::env::consts::OS,
                "setup_completed": false,
                "coven_commit": Value::Null,
                "threads_commit": Value::Null,
                "local_threads_override_active": Value::Null,
                "failure": sanitized,
            }))?,
        )?;
        write_missing_setup_artifact(self.artifact_dir.join("request.json"), b"null\n")?;
        write_missing_setup_artifact(self.artifact_dir.join("response.json"), b"null\n")?;
        write_missing_setup_artifact(
            logs.join("daemon.log"),
            "<daemon unavailable: fixture setup did not complete>\n",
        )?;
        for name in [
            "ward-audit.jsonl",
            "pending-tree.txt",
            "workspace-tree.txt",
            "sqlite-schema.txt",
        ] {
            write_missing_setup_artifact(
                state.join(name),
                "<unavailable: fixture setup did not complete>\n",
            )?;
        }
        Ok(())
    }
}

struct GitState {
    commit: String,
    dirty: bool,
    state_sha256: String,
}

fn threads_dependency(workspace_root: &Path) -> Result<ThreadsDependency> {
    let workspace_root = fs::canonicalize(workspace_root)
        .context("canonicalizing the Coven workspace for dependency proof")?;
    let coven_cli_manifest =
        fs::canonicalize(Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml"))
            .context("canonicalizing the coven-cli manifest path")?;
    let output = Command::new("cargo")
        .args(["metadata", "--format-version", "1", "--locked"])
        .current_dir(&workspace_root)
        .output()
        .context("running cargo metadata for the Threads override proof")?;
    anyhow::ensure!(
        output.status.success(),
        "cargo metadata failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let metadata: Value = serde_json::from_slice(&output.stdout)?;
    let packages = metadata["packages"]
        .as_array()
        .context("cargo metadata packages is not an array")?;
    let coven_cli = packages
        .iter()
        .find(|package| {
            package["name"] == "coven-cli"
                && package["manifest_path"]
                    .as_str()
                    .and_then(|path| fs::canonicalize(path).ok())
                    .as_ref()
                    == Some(&coven_cli_manifest)
        })
        .context("cargo metadata did not contain this coven-cli package")?;
    let coven_cli_id = coven_cli["id"]
        .as_str()
        .context("coven-cli package is missing its package id")?;
    let nodes = metadata["resolve"]["nodes"]
        .as_array()
        .context("cargo metadata did not contain a resolve graph")?;
    let coven_cli_node = nodes
        .iter()
        .find(|node| node["id"].as_str() == Some(coven_cli_id))
        .context("cargo metadata resolve graph did not contain coven-cli")?;
    let threads_id = coven_cli_node["deps"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|dependency| dependency["name"] == "coven_threads_core")
        .and_then(|dependency| dependency["pkg"].as_str())
        .context("coven-cli does not resolve a coven-threads-core dependency")?;
    let package = packages
        .iter()
        .find(|package| package["id"].as_str() == Some(threads_id))
        .context("resolved coven-threads-core package is missing from metadata")?;
    let manifest_path = PathBuf::from(
        package["manifest_path"]
            .as_str()
            .context("Threads package is missing manifest_path")?,
    );
    let manifest_path = fs::canonicalize(&manifest_path).with_context(|| {
        format!(
            "canonicalizing resolved Threads manifest {}",
            manifest_path.display()
        )
    })?;
    let local_override_active =
        package["source"].is_null() && !manifest_path.starts_with(workspace_root.join("crates"));
    Ok(ThreadsDependency {
        manifest_path,
        local_override_active,
    })
}

fn seed_familiar(coven_home: &Path, workspace: &Path) -> Result<()> {
    fs::write(
        coven_home.join("familiars.toml"),
        r#"[[familiar]]
id = "sage"
display_name = "Sage"
role = "Research"
description = "Synthetic Threads E2E familiar."
"#,
    )?;
    fs::write(workspace.join("SOUL.md"), "# Sage\n")?;
    fs::write(
        workspace.join("ward.toml"),
        format!(
            r#"principal_key_fingerprint = "{PRINCIPAL_FINGERPRINT}"
protected_surface = ["SOUL.md"]

[[surface]]
path = "SOUL.md"
tier = 0

[[surface]]
path = "reviewed/"
tier = 1

[[probe]]
surface = "reviewed/**"
id = "size-delta"

[[probe]]
surface = "reviewed/**"
id = "pattern-lint"
forbidden = ["(?i)ignore previous"]
"#
        ),
    )?;
    Ok(())
}

fn run_coven(coven: &Path, coven_home: &Path, path: &OsString, args: &[&str]) -> Result<Output> {
    Command::new(coven)
        .args(args)
        .env("COVEN_HOME", coven_home)
        .env("PATH", path)
        .output()
        .map_err(Into::into)
}

fn daemon_start_timed_out_waiting_for_health_connect(stderr: &[u8]) -> bool {
    String::from_utf8_lossy(stderr).contains(
        "timed out waiting for Coven daemon startup health; last_readiness=connect-timeout",
    )
}

fn wait_for_daemon_health(coven_home: &Path) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut last_error = None;
    while Instant::now() < deadline {
        if coven_home.join("daemon.json").exists() {
            match daemon_http_request(coven_home, "GET", "/health", None) {
                Ok((200, body)) if body.contains(r#""ok":true"#) => return Ok(()),
                Ok(_) => {}
                Err(error) => last_error = Some(error),
            }
        }
        thread::sleep(Duration::from_millis(50));
    }
    match last_error {
        Some(error) => anyhow::bail!("daemon did not become ready: {error:#}"),
        None => anyhow::bail!("daemon did not become ready"),
    }
}

fn sanitized_daemon_endpoint() -> &'static str {
    if cfg!(windows) {
        "<daemon-pipe>"
    } else {
        "<coven-home>/coven.sock"
    }
}

fn daemon_pid(coven_home: &Path) -> Result<u32> {
    let status: Value = serde_json::from_slice(&fs::read(coven_home.join("daemon.json"))?)?;
    let pid = status["pid"]
        .as_u64()
        .context("daemon status is missing pid")?;
    u32::try_from(pid).context("daemon pid does not fit u32")
}

fn wait_for_process_exit(pid: u32, label: &str, timeout: Duration) -> Result<()> {
    let started = Instant::now();
    let deadline = started + timeout;
    while Instant::now() < deadline {
        if !pid_is_alive(pid) {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(25));
    }
    anyhow::bail!(
        "{label} pid {pid} remained observable for {:?}",
        started.elapsed()
    )
}

fn wait_for_daemon_shutdown(coven_home: &Path, pid: Option<u32>) -> Result<()> {
    let started = Instant::now();
    let deadline = started + Duration::from_secs(3);
    while Instant::now() < deadline {
        if pid.map(|pid| !pid_is_alive(pid)).unwrap_or(true)
            && !coven_home.join("daemon.json").exists()
            && !coven_home.join("coven.sock").exists()
        {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(25));
    }
    if let Some(pid) = pid.filter(|pid| pid_is_alive(*pid)) {
        anyhow::bail!(
            "daemon pid {pid} remained observable after checked stop for {:?}",
            started.elapsed()
        );
    }
    anyhow::bail!(
        "daemon status artifacts remained observable after checked stop for {:?}",
        started.elapsed()
    )
}

#[cfg(unix)]
fn pid_is_alive(pid: u32) -> bool {
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

#[cfg(windows)]
fn pid_is_alive(pid: u32) -> bool {
    use windows_sys::Win32::{
        Foundation::{
            CloseHandle, GetLastError, ERROR_INVALID_PARAMETER, WAIT_OBJECT_0, WAIT_TIMEOUT,
        },
        System::Threading::{OpenProcess, WaitForSingleObject},
    };

    const SYNCHRONIZE_ACCESS: u32 = 0x0010_0000;
    let handle = unsafe { OpenProcess(SYNCHRONIZE_ACCESS, 0, pid) };
    if handle.is_null() {
        let error = unsafe { GetLastError() };
        assert_eq!(
            error, ERROR_INVALID_PARAMETER,
            "cannot establish fixture daemon pid {pid} liveness: Windows error {error}"
        );
        return false;
    }
    let result = unsafe { WaitForSingleObject(handle, 0) };
    unsafe { CloseHandle(handle) };
    match result {
        WAIT_TIMEOUT => true,
        WAIT_OBJECT_0 => false,
        _ => panic!("cannot establish fixture daemon pid {pid} liveness: wait result {result}"),
    }
}

#[cfg(unix)]
fn terminate_daemon_process(_coven_home: &Path, pid: u32) -> Result<()> {
    let status = Command::new("kill")
        .args(["-KILL", &pid.to_string()])
        .status()
        .context("sending SIGKILL to the fixture daemon")?;
    anyhow::ensure!(
        status.success(),
        "SIGKILL failed for fixture daemon pid {pid}"
    );
    Ok(())
}

#[cfg(windows)]
fn terminate_daemon_process(coven_home: &Path, pid: u32) -> Result<()> {
    let status: Value = serde_json::from_slice(&fs::read(coven_home.join("daemon.json"))?)?;
    let pipe = status["socket"]
        .as_str()
        .context("daemon status has no pipe")?;
    let deadline = Instant::now() + Duration::from_secs(3);
    let probe = coven_client::probe_windows_daemon_health_with_identity_until(pipe, deadline)?
        .context("fixture daemon pipe disappeared before crash injection")?;
    anyhow::ensure!(
        probe.server_pid == pid,
        "fixture daemon process changed before crash injection"
    );
    let process = coven_client::open_windows_daemon_process_for_stop_until(
        pipe,
        pid,
        Some(probe.process_creation_time),
        deadline,
    )?
    .context("fixture daemon identity could not be verified for crash injection")?;
    anyhow::ensure!(
        process.terminate_and_wait_until(deadline)?,
        "verified fixture daemon pid {pid} did not exit after crash injection"
    );
    Ok(())
}

fn daemon_http_request(
    coven_home: &Path,
    method: &'static str,
    path: &str,
    body: Option<&Value>,
) -> Result<(u16, String)> {
    let endpoint = coven_client::DaemonEndpoint::discover(coven_home)
        .context("discovering owner-local fixture daemon")?;
    let mut client = coven_client::DaemonClient::new(endpoint);
    let response = client
        .request_json(method, path, body)
        .with_context(|| format!("fixture daemon request {method} {path}"))?;
    Ok((
        response.status,
        String::from_utf8(response.body).context("fixture daemon response is not UTF-8")?,
    ))
}

#[test]
fn startup_status_capture_is_bounded_and_excludes_unknown_fields() -> Result<()> {
    let home = tempfile::tempdir()?;
    assert_eq!(
        capture_daemon_status(home.path()),
        json!({"present": false})
    );
    let path = home.path().join("daemon.json");
    fs::write(&path, b"{")?;
    assert_eq!(
        capture_daemon_status(home.path()),
        json!({"capture_error": "invalid-status-json"})
    );
    fs::write(&path, vec![b' '; coven_client::MAX_DAEMON_STATUS_BYTES + 1])?;
    assert_eq!(
        capture_daemon_status(home.path()),
        json!({"capture_error": "status-too-large"})
    );
    fs::write(&path, br#"{"pid":42,"unknown":"synthetic-private-value"}"#)?;
    let captured = capture_daemon_status(home.path());
    assert_eq!(captured["pid"], 42);
    assert!(captured.get("unknown").is_none());
    assert!(!captured.to_string().contains("synthetic-private-value"));
    Ok(())
}

fn capture_daemon_status(coven_home: &Path) -> Value {
    use std::io::Read;

    let file = match fs::File::open(coven_home.join("daemon.json")) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return json!({"present": false})
        }
        Err(error) => {
            return json!({"capture_error": "open-status", "io_kind": format!("{:?}", error.kind())})
        }
    };
    let mut bytes = Vec::new();
    if let Err(error) = file
        .take(coven_client::MAX_DAEMON_STATUS_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
    {
        return json!({"capture_error": "read-status", "io_kind": format!("{:?}", error.kind())});
    }
    if bytes.len() > coven_client::MAX_DAEMON_STATUS_BYTES {
        return json!({"capture_error": "status-too-large"});
    }
    match serde_json::from_slice::<Value>(&bytes) {
        Ok(status) => json!({
            "present": true,
            "pid": status.get("pid").and_then(Value::as_u64),
            "socket": status.get("socket").and_then(Value::as_str),
            "started_at": status.get("started_at").and_then(Value::as_str),
            "process_creation_time": status.get("process_creation_time").and_then(Value::as_u64),
        }),
        Err(_) => json!({"capture_error": "invalid-status-json"}),
    }
}

fn ward_audit_jsonl(store: &Path) -> Result<String> {
    if !store.exists() {
        return Ok(String::new());
    }
    let conn = Connection::open(store)?;
    let mut statement = conn.prepare(
        "SELECT id, event_type, proposal_id, familiar_id, tier, decision,
                approver, hex(diff_hash), detail, files_touched, channel,
                thread_id, submitted_at, decided_at, recorded_at
         FROM ward_audit ORDER BY id",
    )?;
    let rows = statement.query_map([], |row| {
        Ok(json!({
            "id": row.get::<_, i64>(0)?,
            "event_type": row.get::<_, String>(1)?,
            "proposal_id": row.get::<_, Option<String>>(2)?,
            "familiar_id": row.get::<_, String>(3)?,
            "tier": row.get::<_, Option<String>>(4)?,
            "decision": row.get::<_, String>(5)?,
            "approver": row.get::<_, Option<String>>(6)?,
            "diff_hash": row.get::<_, String>(7)?,
            "detail": row.get::<_, Option<String>>(8)?,
            "files_touched": row.get::<_, String>(9)?,
            "channel": row.get::<_, Option<String>>(10)?,
            "thread_id": row.get::<_, Option<String>>(11)?,
            "submitted_at": row.get::<_, String>(12)?,
            "decided_at": row.get::<_, String>(13)?,
            "recorded_at": row.get::<_, String>(14)?,
        }))
    })?;
    let mut output = String::new();
    for row in rows {
        output.push_str(&serde_json::to_string(&row?)?);
        output.push('\n');
    }
    Ok(output)
}

fn sqlite_schema(store: &Path) -> Result<String> {
    if !store.exists() {
        return Ok(String::new());
    }
    let conn = Connection::open(store)?;
    let mut statement = conn.prepare(
        "SELECT type, name, sql FROM sqlite_master
         WHERE sql IS NOT NULL ORDER BY type, name",
    )?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        ))
    })?;
    let mut output = String::new();
    for row in rows {
        let (kind, name, sql) = row?;
        output.push_str(&format!("-- {kind} {name}\n{sql};\n\n"));
    }
    Ok(output)
}

fn inventory(root: &Path) -> Result<String> {
    if !root.exists() {
        return Ok("<absent>\n".to_owned());
    }
    let mut entries = Vec::new();
    collect_inventory(root, root, &mut entries)?;
    entries.sort();
    Ok(entries.join("\n") + "\n")
}

fn collect_inventory(root: &Path, path: &Path, entries: &mut Vec<String>) -> Result<()> {
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let entry_path = entry.path();
        let relative = entry_path.strip_prefix(root)?;
        let metadata = fs::symlink_metadata(&entry_path)?;
        if metadata.is_dir() {
            entries.push(format!("{}/", relative.display()));
            collect_inventory(root, &entry_path, entries)?;
        } else if metadata.is_file() {
            let bytes = fs::read(&entry_path)?;
            entries.push(format!(
                "{} bytes={} sha256={}",
                relative.display(),
                bytes.len(),
                hex_bytes(&Sha256::digest(&bytes))
            ));
        } else {
            entries.push(format!("{} <non-regular>", relative.display()));
        }
    }
    Ok(())
}

fn hex_bytes(bytes: &[u8]) -> String {
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        write!(&mut encoded, "{byte:02x}").expect("writing to a String cannot fail");
    }
    encoded
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("coven-cli manifest is nested under crates/")
        .to_path_buf()
}

fn git_state(path: &Path) -> Result<GitState> {
    let root_output = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(path)
        .output()
        .with_context(|| format!("locating git root from {}", path.display()))?;
    anyhow::ensure!(
        root_output.status.success(),
        "git rev-parse --show-toplevel failed in {}: {}",
        path.display(),
        String::from_utf8_lossy(&root_output.stderr)
    );
    let root = PathBuf::from(String::from_utf8(root_output.stdout)?.trim());
    let commit_output = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(&root)
        .output()?;
    anyhow::ensure!(
        commit_output.status.success(),
        "git rev-parse HEAD failed in {}: {}",
        root.display(),
        String::from_utf8_lossy(&commit_output.stderr)
    );
    let status_output = Command::new("git")
        .args(["status", "--porcelain=v1", "-z", "--untracked-files=all"])
        .current_dir(&root)
        .output()?;
    anyhow::ensure!(
        status_output.status.success(),
        "git status failed in {}: {}",
        root.display(),
        String::from_utf8_lossy(&status_output.stderr)
    );
    let diff_output = Command::new("git")
        .args(["diff", "--binary", "HEAD", "--"])
        .current_dir(&root)
        .output()?;
    anyhow::ensure!(
        diff_output.status.success(),
        "git diff failed in {}: {}",
        root.display(),
        String::from_utf8_lossy(&diff_output.stderr)
    );

    let mut fingerprint = Sha256::new();
    fingerprint.update(&status_output.stdout);
    fingerprint.update(&diff_output.stdout);
    for record in status_output.stdout.split(|byte| *byte == 0) {
        if record.starts_with(b"?? ") {
            let relative = std::str::from_utf8(&record[3..])
                .context("git reported a non-UTF-8 untracked path")?;
            let untracked = root.join(relative);
            let metadata = fs::symlink_metadata(&untracked)?;
            if metadata.file_type().is_symlink() {
                fingerprint.update(relative.as_bytes());
                fingerprint.update(fs::read_link(untracked)?.as_os_str().as_encoded_bytes());
            } else if metadata.is_file() {
                fingerprint.update(relative.as_bytes());
                fingerprint.update(fs::read(untracked)?);
            }
        }
    }

    Ok(GitState {
        commit: String::from_utf8(commit_output.stdout)?.trim().to_owned(),
        dirty: !status_output.stdout.is_empty(),
        state_sha256: hex_bytes(&fingerprint.finalize()),
    })
}

fn file_sha256(path: &Path) -> Result<String> {
    Ok(hex_bytes(&Sha256::digest(
        fs::read(path).with_context(|| format!("reading {}", path.display()))?,
    )))
}

fn sanitize_for_artifact(value: &str) -> String {
    let mut sanitized = value.replace(&workspace_root().display().to_string(), "<coven-workspace>");
    for key in ["HOME", "USERPROFILE"] {
        if let Some(home) = std::env::var_os(key) {
            sanitized = sanitized.replace(&PathBuf::from(home).display().to_string(), "<home>");
        }
    }
    sanitized
}

fn replace_sanitized_path(value: String, path: &Path, placeholder: &str) -> String {
    let display = path.display().to_string();
    let with_private = if display.starts_with("/private/") {
        value
    } else {
        value.replace(&format!("/private{display}"), placeholder)
    };
    let with_direct = with_private.replace(&display, placeholder);
    if display.starts_with("/private/") {
        return with_direct;
    }
    with_direct
}

fn sanitize_json_strings(value: &Value, sanitize: &impl Fn(&str) -> String) -> Value {
    match value {
        Value::String(text) => Value::String(sanitize(text)),
        Value::Array(values) => Value::Array(
            values
                .iter()
                .map(|value| sanitize_json_strings(value, sanitize))
                .collect(),
        ),
        Value::Object(values) => Value::Object(
            values
                .iter()
                .map(|(key, value)| (key.clone(), sanitize_json_strings(value, sanitize)))
                .collect(),
        ),
        scalar => scalar.clone(),
    }
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}
