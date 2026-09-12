use super::final_commit_cases::{
    arm_final_commit_pause, release_final_commit_pause, wait_for_final_commit_pause,
};
use super::*;

const FORMAT_PATH: &str = "output-format.json";
const FORMAT_BEFORE: &str =
    r#"{"schema":"coven.output-format/v1","indent":2,"final_newline":true}"#;
const FORMAT_AFTER: &str =
    r#"{"schema":"coven.output-format/v1","indent":4,"final_newline":false}"#;

mod review_cases {
    use super::*;
    include!("threads_output_auto_review_cases.rs");
}

fn seed_output_auto(home: &Path, workspace: &Path, veto: bool) -> Result<()> {
    fs::write(
        home.join("familiars.toml"),
        "[[familiar]]\nid = \"sage\"\nname = \"Synthetic-format\"\ndisplay_name = \"Synthetic-format\"\nperson = \"Example principal\"\nrole = \"Synthetic fixture\"\ndescription = \"Repository-authored synthetic formatting fixture.\"\n",
    )?;
    fs::write(workspace.join("SOUL.md"), "# I am Synthetic-format\n")?;
    fs::write(
        workspace.join("IDENTITY.md"),
        "# IDENTITY.md - Synthetic-format\n- **Name:** Synthetic-format\n",
    )?;
    fs::write(workspace.join("MEMORY.md"), "Protected synthetic memory\n")?;
    fs::write(workspace.join(FORMAT_PATH), FORMAT_BEFORE)?;
    let window = if veto {
        "human_veto_window_hours = 1\nmin_visible_seconds = 60\n"
    } else {
        ""
    };
    fs::write(
        workspace.join("ward.toml"),
        format!(
            r#"principal_key_fingerprint = "{PRINCIPAL_FINGERPRINT}"
protected_surface = ["SOUL.md", "IDENTITY.md", "MEMORY.md", "ward.toml"]

[[identity_invariant]]
fact = "name"
operator = "equals"
expected = "Synthetic-format"
[[identity_invariant]]
fact = "person"
operator = "equals"
expected = "Example principal"

[[surface]]
path = "SOUL.md"
tier = 0
[[surface]]
path = "IDENTITY.md"
tier = 0
[[surface]]
path = "MEMORY.md"
tier = 0
[[surface]]
path = "ward.toml"
tier = 0
[[surface]]
path = "output-format.json"
tier = 2

[editable]
harness_blocks = ["output_format"]
[approval_tiers.auto]
blocks = ["output_format"]
gate = "regression_suite"
{window}
[[probe]]
surface = "output-format.json"
id = "parse"
format = "json"
"#
        ),
    )?;
    Ok(())
}

fn submit_output_auto(fixture: &mut ThreadsFixture) -> Result<Value> {
    let response = fixture.request(
        "POST",
        "/api/v1/familiars/sage/edits",
        Some(&json!({"edits":[{"target":FORMAT_PATH,"contents":FORMAT_AFTER}]})),
    )?;
    anyhow::ensure!(response.status == 202, "auto did not stage: {response:?}");
    anyhow::ensure!(
        fs::read_to_string(fixture.workspace.join(FORMAT_PATH))? == FORMAT_BEFORE,
        "intake applied before scheduling"
    );
    let pending = response.body["pendingPath"]
        .as_str()
        .context("pending path")?;
    let stored: Value = serde_json::from_slice(&fs::read(pending)?)?;
    anyhow::ensure!(stored["autoRegressionEvidence"].as_array().map(Vec::len) == Some(32));
    anyhow::ensure!(stored["identityEvidence"].as_array().map(Vec::len) == Some(32));
    let (tier, detail): (String, String) = fixture.store()?.query_row(
        "SELECT tier, detail FROM ward_audit WHERE proposal_id=?1 AND event_type='proposal_submitted'",
        [response.body["proposalId"].as_str().context("proposal id")?],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    let detail: Value = serde_json::from_str(&detail)?;
    anyhow::ensure!(
        tier == "2" && detail["autoRegressionEvidence"] == stored["autoRegressionEvidence"]
    );
    Ok(response.body)
}

#[test]
fn output_auto_veto_survives_restart_and_applies_once() -> Result<()> {
    run_clocked_journey(
        "output-auto-veto",
        |home, workspace| seed_output_auto(home, workspace, true),
        |fixture, capability| {
            let staged = submit_output_auto(fixture)?;
            let id = staged["proposalId"].as_str().context("proposal id")?;
            let earliest: time::OffsetDateTime =
                serde_json::from_value(staged["scheduledProposal"]["earliest_close"].clone())?;
            let deadline: time::OffsetDateTime =
                serde_json::from_value(staged["scheduledProposal"]["veto_deadline"].clone())?;
            anyhow::ensure!(earliest == fixture_time(60)? && deadline == fixture_time(3600)?);
            tick_scheduler(fixture, capability)?;
            advance_clock_to_offset(fixture, capability, 59)?;
            tick_scheduler(fixture, capability)?;
            anyhow::ensure!(
                fs::read_to_string(fixture.workspace.join(FORMAT_PATH))? == FORMAT_BEFORE
            );
            fixture.stop_daemon()?;
            fixture.start_daemon()?;
            advance_clock_to_offset(fixture, capability, 3599)?;
            tick_scheduler(fixture, capability)?;
            anyhow::ensure!(
                fs::read_to_string(fixture.workspace.join(FORMAT_PATH))? == FORMAT_BEFORE
            );
            advance_clock_to_offset(fixture, capability, 3600)?;
            tick_scheduler(fixture, capability)?;
            tick_scheduler(fixture, capability)?;
            anyhow::ensure!(
                fs::read_to_string(fixture.workspace.join(FORMAT_PATH))? == FORMAT_AFTER
            );
            assert_window_terminal(fixture, id, "proposal_approved", "applied", json!(true))
        },
    )
}

#[test]
fn output_auto_no_veto_survives_restart_without_window_events() -> Result<()> {
    run_clocked_journey(
        "output-auto-no-veto",
        |home, workspace| seed_output_auto(home, workspace, false),
        |fixture, capability| {
            let staged = submit_output_auto(fixture)?;
            anyhow::ensure!(
                staged["scheduledProposal"]["classification"]["approval_path"]
                    .as_object()
                    .context("typed approval")?
                    .get("veto")
                    == Some(&Value::Null)
            );
            let id = staged["proposalId"].as_str().context("proposal id")?;
            fixture.stop_daemon()?;
            fixture.start_daemon()?;
            tick_scheduler(fixture, capability)?;
            tick_scheduler(fixture, capability)?;
            anyhow::ensure!(
                fs::read_to_string(fixture.workspace.join(FORMAT_PATH))? == FORMAT_AFTER
            );
            let conn = fixture.store()?;
            let opened: i64 = conn.query_row(
                "SELECT COUNT(*) FROM ward_audit WHERE proposal_id=?1 AND event_type='proposal_window_opened'",
                [id], |row| row.get(0),
            )?;
            let approved: i64 = conn.query_row(
                "SELECT COUNT(*) FROM ward_audit WHERE proposal_id=?1 AND event_type='proposal_approved'",
                [id], |row| row.get(0),
            )?;
            anyhow::ensure!(opened == 0 && approved == 1);
            let detail: String = conn.query_row(
                "SELECT detail FROM ward_audit WHERE proposal_id=?1 AND event_type='proposal_approved'",
                [id], |row| row.get(0),
            )?;
            let detail: Value = serde_json::from_str(&detail)?;
            anyhow::ensure!(detail.get("window_close").is_none_or(Value::is_null));
            Ok(())
        },
    )
}

fn post_format(fixture: &mut ThreadsFixture, contents: &str) -> Result<HttpResponse> {
    fixture.request(
        "POST",
        "/api/v1/familiars/sage/edits",
        Some(&json!({"edits":[{"target":FORMAT_PATH,"contents":contents}]})),
    )
}

#[test]
fn output_auto_intake_refuses_malformed_images_and_mixed_batches() -> Result<()> {
    run_clocked_journey(
        "output-auto-invalid-images",
        |home, workspace| seed_output_auto(home, workspace, false),
        |fixture, _| {
            for invalid in [
                "{}",
                "null",
                "[]",
                r#"{"schema":"coven.output-format/v2","indent":2,"final_newline":true}"#,
                r#"{"schema":"coven.output-format/v1","indent":3,"final_newline":true}"#,
                r#"{"schema":"coven.output-format/v1","indent":2.0,"final_newline":true}"#,
                r#"{"schema":"coven.output-format/v1","indent":2,"indent":4,"final_newline":true}"#,
                r#"{"schema":"coven.output-format/v1","indent":2,"final_newline":true,"prompt":"no"}"#,
                &" ".repeat(257),
            ] {
                let response = post_format(fixture, invalid)?;
                anyhow::ensure!(
                    response.status == 409
                        && response.body["error"]["code"] == "scheduled_publication_invalid",
                    "invalid image escaped bounded refusal: {response:?}"
                );
                anyhow::ensure!(
                    fs::read_to_string(fixture.workspace.join(FORMAT_PATH))? == FORMAT_BEFORE
                );
            }
            fs::write(fixture.workspace.join(FORMAT_PATH), "{}")?;
            anyhow::ensure!(post_format(fixture, FORMAT_AFTER)?.status == 409);
            fs::remove_file(fixture.workspace.join(FORMAT_PATH))?;
            anyhow::ensure!(post_format(fixture, FORMAT_AFTER)?.status == 409);
            anyhow::ensure!(!fixture.workspace.join(FORMAT_PATH).exists());
            fs::write(fixture.workspace.join(FORMAT_PATH), FORMAT_BEFORE)?;
            fs::write(fixture.workspace.join("notes.txt"), "unchanged")?;
            let mixed = fixture.request(
                "POST",
                "/api/v1/familiars/sage/edits",
                Some(&json!({"edits":[
                    {"target":FORMAT_PATH,"contents":FORMAT_AFTER},
                    {"target":"notes.txt","contents":"must not apply"},
                ]})),
            )?;
            anyhow::ensure!(mixed.status == 409);
            anyhow::ensure!(
                fs::read_to_string(fixture.workspace.join("notes.txt"))? == "unchanged"
            );
            anyhow::ensure!(
                fs::read_to_string(fixture.workspace.join(FORMAT_PATH))? == FORMAT_BEFORE
            );
            let count: i64 = fixture.store()?.query_row(
                "SELECT COUNT(*) FROM ward_audit WHERE event_type IN ('proposal_submitted','proposal_approved','apply_audit')",
                [], |row| row.get(0),
            )?;
            anyhow::ensure!(
                count == 0,
                "refused intake fabricated proposal or apply evidence"
            );
            Ok(())
        },
    )
}

#[test]
fn output_auto_requires_applicable_json_and_all_probes_passed() -> Result<()> {
    run_clocked_journey(
        "output-auto-regression-gates",
        |home, workspace| seed_output_auto(home, workspace, false),
        |fixture, _| {
            let path = fixture.workspace.join("ward.toml");
            let original = fs::read_to_string(&path)?;
            let policy = original.split("\n[[probe]]").next().context("policy")?;
            for probes in [
                "",
                "\n[[probe]]\nsurface='other.json'\nid='parse'\nformat='json'\n",
                "\n[[probe]]\nsurface='output-format.json'\nid='size-delta'\n",
                "\n[[probe]]\nsurface='output-format.json'\nid='parse'\n",
                "\n[[probe]]\nsurface='output-format.json'\nid='parse'\nformat='toml'\n",
                "\n[[probe]]\nsurface='output-format.json'\nid='parse'\nformat='json'\n\
                 [[probe]]\nsurface='*.json'\nid='pattern-lint'\nforbidden=['indent']\n",
                "\n[[probe]]\nsurface='output-format.json'\nid='parse'\nformat='json'\n\
                 [[probe]]\nsurface='*.json'\nid='pattern-lint'\nrequired=['[']\n",
                "\n[[probe]]\nsurface='output-format.json'\nid='parse'\nformat='json'\n\
                 [[probe]]\nsurface='*.json'\nid='pattern-lint'\n",
            ] {
                fs::write(&path, format!("{policy}{probes}"))?;
                let response = post_format(fixture, FORMAT_AFTER)?;
                anyhow::ensure!(
                    response.status == 409,
                    "unscored/failed gate admitted: {response:?}"
                );
                anyhow::ensure!(
                    fs::read_to_string(fixture.workspace.join(FORMAT_PATH))? == FORMAT_BEFORE
                );
            }
            fs::write(path, original)?;
            submit_output_auto(fixture)?;
            Ok(())
        },
    )
}

#[test]
fn output_auto_preserves_literal_opt_in_stronger_floors_and_memory_protection() -> Result<()> {
    run_clocked_journey(
        "output-auto-floors",
        |home, workspace| seed_output_auto(home, workspace, false),
        |fixture, _| {
            let path = fixture.workspace.join("ward.toml");
            let original = fs::read_to_string(&path)?;
            for config in [
                original.replace(
                    "path = \"output-format.json\"\ntier = 2",
                    "path = \"*.json\"\ntier = 2",
                ),
                original.replace(
                    "path = \"output-format.json\"\ntier = 2",
                    "path = \"output-format.json\"\ntier = 1",
                ),
                format!("{original}\n[[surface]]\npath='*.json'\ntier=1\n"),
            ] {
                fs::write(&path, config)?;
                anyhow::ensure!(post_format(fixture, FORMAT_AFTER)?.status == 409);
                anyhow::ensure!(
                    fs::read_to_string(fixture.workspace.join(FORMAT_PATH))? == FORMAT_BEFORE
                );
            }
            let mut protected: toml::Value = toml::from_str(&original)?;
            protected["protected_surface"]
                .as_array_mut()
                .context("protected")?
                .push(toml::Value::String("OUTPUT-FORMAT.JSON".into()));
            protected["surface"]
                .as_array_mut()
                .context("surfaces")?
                .push(toml::from_str("path='OUTPUT-FORMAT.JSON'\ntier=0\n")?);
            fs::write(&path, toml::to_string(&protected)?)?;
            let blocked = post_format(fixture, FORMAT_AFTER)?;
            anyhow::ensure!(
                blocked.status == 403,
                "protected alias escaped: {blocked:?}"
            );
            fs::write(&path, original)?;
            for target in ["MEMORY.md", "./MEMORY.md", "memory.md"] {
                let response = fixture.request(
                    "POST",
                    "/api/v1/familiars/sage/edits",
                    Some(&json!({"edits":[{"target":target,"contents":FORMAT_AFTER}],
                        "principalKeyFingerprint":PRINCIPAL_FINGERPRINT})),
                )?;
                anyhow::ensure!(
                    response.status == 403,
                    "memory alias became auto-eligible: {response:?}"
                );
            }
            anyhow::ensure!(
                fs::read_to_string(fixture.workspace.join("MEMORY.md"))?
                    == "Protected synthetic memory\n"
            );
            Ok(())
        },
    )
}

#[test]
fn output_auto_keeps_stronger_ceremonies_and_other_probes_advisory() -> Result<()> {
    run_clocked_journey(
        "output-auto-advisory-isolation",
        |home, workspace| seed_output_auto(home, workspace, false),
        |fixture, capability| {
            let path = fixture.workspace.join("ward.toml");
            let original = fs::read_to_string(&path)?;
            let human = original
                .replace("[approval_tiers.auto]", "[approval_tiers.human_review]")
                .replace("gate = \"regression_suite\"", "gate = \"human_approval\"");
            fs::write(&path, format!("{human}\n[[probe]]\nsurface='*.json'\nid='pattern-lint'\nforbidden=['indent']\n"))?;
            let staged = post_format(fixture, FORMAT_AFTER)?;
            anyhow::ensure!(staged.status == 202);
            let id = staged.body["proposalId"].as_str().context("id")?;
            tick_scheduler(fixture, capability)?;
            anyhow::ensure!(
                fs::read_to_string(fixture.workspace.join(FORMAT_PATH))? == FORMAT_BEFORE
            );
            let payload = proposal_decision_payload(fixture, id, "Synthetic human approval")?;
            let approved = fixture.request(
                "POST",
                &format!("/api/v1/threads/proposals/{id}/approve"),
                Some(&payload),
            )?;
            anyhow::ensure!(
                approved.status == 200,
                "stronger ceremony failed: {approved:?}"
            );
            anyhow::ensure!(
                fs::read_to_string(fixture.workspace.join(FORMAT_PATH))? == FORMAT_AFTER
            );
            let mut ordinary: toml::Value = toml::from_str(&original)?;
            let table = ordinary.as_table_mut().context("policy table")?;
            table.remove("editable");
            table.remove("approval_tiers");
            fs::write(&path, toml::to_string(&ordinary)?)?;
            let applied = post_format(fixture, "ordinary unconfigured text")?;
            anyhow::ensure!(applied.status == 200 && applied.body["disposition"] == "applied");
            fs::write(&path, format!("{original}\n[[probe]]\nsurface='*'\nid='pattern-lint'\nforbidden=['anything']\n"))?;
            let unrelated = fixture.request(
                "POST",
                "/api/v1/familiars/sage/edits",
                Some(&json!({"edits":[{"target":"notes.txt","contents":"anything"}]})),
            )?;
            anyhow::ensure!(unrelated.status == 200 && unrelated.body["disposition"] == "applied");
            Ok(())
        },
    )
}

#[test]
fn output_auto_deadline_and_restart_reject_regression_and_valid_evidence_drift() -> Result<()> {
    for restart in [false, true] {
        for change in [
            "probe-valid",
            "probe-failed",
            "probe-unscored",
            "policy-valid",
            "identity-valid",
            "before-valid",
            "missing-evidence",
            "changed-evidence",
        ] {
            run_clocked_journey(
                &format!("output-auto-drift-{restart}-{change}"),
                |home, workspace| seed_output_auto(home, workspace, true),
                |fixture, capability| {
                    let staged = submit_output_auto(fixture)?;
                    let id = staged["proposalId"].as_str().context("id")?;
                    tick_scheduler(fixture, capability)?;
                    let policy_path = fixture.workspace.join("ward.toml");
                    let policy = fs::read_to_string(&policy_path)?;
                    let mut expected_before = FORMAT_BEFORE;
                    match change {
                        "probe-valid" => fs::write(&policy_path, format!("{policy}\n[[probe]]\nsurface='*.json'\nid='size-delta'\n"))?,
                        "probe-failed" => fs::write(&policy_path, format!("{policy}\n[[probe]]\nsurface='*.json'\nid='pattern-lint'\nforbidden=['indent']\n"))?,
                        "probe-unscored" => fs::write(&policy_path, format!("{policy}\n[[probe]]\nsurface='*.json'\nid='pattern-lint'\n"))?,
                        "policy-valid" => fs::write(&policy_path, format!("{policy}\n[[surface]]\npath='unrelated.txt'\ntier=2\n"))?,
                        "identity-valid" => fs::write(fixture.workspace.join("SOUL.md"), "# I am Synthetic-format\nA still-valid synthetic identity source.\n")?,
                        "before-valid" => {
                            expected_before = r#"{"schema":"coven.output-format/v1","indent":4,"final_newline":true}"#;
                            fs::write(fixture.workspace.join(FORMAT_PATH), expected_before)?;
                        }
                        "missing-evidence" | "changed-evidence" => {
                            let pending = staged["pendingPath"].as_str().context("pending")?;
                            let mut document: Value = serde_json::from_slice(&fs::read(pending)?)?;
                            if change == "missing-evidence" {
                                document.as_object_mut().context("document")?.remove("autoRegressionEvidence");
                            } else {
                                document["autoRegressionEvidence"] = serde_json::to_value([0_u8; 32])?;
                            }
                            fs::write(pending, serde_json::to_vec(&document)?)?;
                        }
                        _ => unreachable!(),
                    }
                    if restart {
                        fixture.stop_daemon()?;
                        fixture.start_daemon()?;
                    }
                    advance_clock_to_offset(fixture, capability, 3600)?;
                    tick_scheduler(fixture, capability)?;
                    tick_scheduler(fixture, capability)?;
                    let reason = if matches!(
                        change,
                        "probe-failed" | "probe-unscored" | "missing-evidence"
                    ) {
                        "revalidation_failed"
                    } else {
                        "evidence_diverged"
                    };
                    assert_window_terminal(fixture, id, "proposal_rejected", reason, json!(false))?;
                    anyhow::ensure!(
                        fs::read_to_string(fixture.workspace.join(FORMAT_PATH))? == expected_before
                    );
                    Ok(())
                },
            )?;
        }
    }
    Ok(())
}

#[test]
fn output_auto_veto_closes_once_and_no_veto_failure_invents_no_close() -> Result<()> {
    for veto in [false, true] {
        run_clocked_journey(
            &format!("output-auto-refusal-{veto}"),
            |home, workspace| seed_output_auto(home, workspace, veto),
            |fixture, capability| {
                let staged = submit_output_auto(fixture)?;
                let id = staged["proposalId"].as_str().context("id")?;
                if veto {
                    tick_scheduler(fixture, capability)?;
                    let payload = proposal_decision_payload(fixture, id, "Synthetic veto")?;
                    let response = fixture.request(
                        "POST",
                        &format!("/api/v1/threads/proposals/{id}/reject"),
                        Some(&payload),
                    )?;
                    anyhow::ensure!(response.status == 200, "{response:?}");
                    assert_window_terminal(fixture, id, "proposal_vetoed", "vetoed", Value::Null)?;
                } else {
                    let pending = staged["pendingPath"].as_str().context("pending")?;
                    let mut document: Value = serde_json::from_slice(&fs::read(pending)?)?;
                    document
                        .as_object_mut()
                        .context("document")?
                        .remove("autoRegressionEvidence");
                    fs::write(pending, serde_json::to_vec(&document)?)?;
                    fixture.stop_daemon()?;
                    fixture.start_daemon()?;
                }
                tick_scheduler(fixture, capability)?;
                tick_scheduler(fixture, capability)?;
                let (count, detail): (i64, Option<String>) = fixture.store()?.query_row(
                                            "SELECT COUNT(*), detail FROM ward_audit WHERE proposal_id=?1 AND event_type IN ('proposal_rejected','proposal_vetoed')",
                                            [id], |row| Ok((row.get(0)?, row.get(1)?)),
                                        )?;
                anyhow::ensure!(count == 1);
                if !veto {
                    anyhow::ensure!(
                        detail.is_none(),
                        "no-window refusal fabricated a typed window close"
                    );
                    let opened: i64 = fixture.store()?.query_row(
                                                "SELECT COUNT(*) FROM ward_audit WHERE proposal_id=?1 AND event_type='proposal_window_opened'",
                                                [id], |row| row.get(0),
                                            )?;
                    anyhow::ensure!(opened == 0);
                }
                anyhow::ensure!(
                    fs::read_to_string(fixture.workspace.join(FORMAT_PATH))? == FORMAT_BEFORE
                );
                Ok(())
            },
        )?;
    }
    Ok(())
}

#[test]
fn output_auto_final_commit_rechecks_policy_and_expected_image() -> Result<()> {
    for changed_source in ["policy", "surface"] {
        run_clocked_journey(
            &format!("output-auto-final-{changed_source}"),
            |home, workspace| seed_output_auto(home, workspace, true),
            |fixture, capability| {
                let staged = submit_output_auto(fixture)?;
                let id = staged["proposalId"].as_str().context("id")?;
                tick_scheduler(fixture, capability)?;
                advance_clock_to_offset(fixture, capability, 3600)?;
                arm_final_commit_pause(fixture, capability)?;
                let home = fixture.coven_home.clone();
                let body = json!({"capability":capability});
                let tick = thread::spawn(move || {
                    daemon_http_request(
                        &home,
                        "POST",
                        "/api/v1/internal/threads/test-clock/tick",
                        Some(&body),
                    )
                });
                let wait = wait_for_final_commit_pause(fixture);
                let mut expected = FORMAT_BEFORE;
                if wait.is_ok() {
                    if changed_source == "policy" {
                        let path = fixture.workspace.join("ward.toml");
                        let policy = fs::read_to_string(&path)?;
                        fs::write(
                            path,
                            format!("{policy}\n[[probe]]\nsurface='*.json'\nid='size-delta'\n"),
                        )?;
                    } else {
                        expected = r#"{"schema":"coven.output-format/v1","indent":4,"final_newline":true}"#;
                        fs::write(fixture.workspace.join(FORMAT_PATH), expected)?;
                    }
                }
                release_final_commit_pause(fixture, capability)?;
                wait?;
                let (status, body) = tick
                    .join()
                    .map_err(|_| anyhow::anyhow!("tick panicked"))??;
                anyhow::ensure!(status == 200, "final tick failed: {body}");
                tick_scheduler(fixture, capability)?;
                anyhow::ensure!(
                    fs::read_to_string(fixture.workspace.join(FORMAT_PATH))? == expected
                );
                assert_window_terminal(
                    fixture,
                    id,
                    "proposal_rejected",
                    if changed_source == "surface" {
                        "evidence_diverged"
                    } else {
                        "revalidation_failed"
                    },
                    json!(false),
                )
            },
        )?;
    }
    Ok(())
}

#[test]
fn output_auto_interrupted_apply_reexecutes_or_quarantines_without_false_terminal() -> Result<()> {
    for missing_ward in [false, true] {
        run_clocked_journey(
            &format!("output-auto-applying-restart-{missing_ward}"),
            |home, workspace| seed_output_auto(home, workspace, false),
            |fixture, capability| {
                let staged = submit_output_auto(fixture)?;
                let id = staged["proposalId"].as_str().context("id")?;
                arm_final_commit_pause(fixture, capability)?;
                let home = fixture.coven_home.clone();
                let body = json!({"capability":capability});
                let tick = thread::spawn(move || {
                    daemon_http_request(
                        &home,
                        "POST",
                        "/api/v1/internal/threads/test-clock/tick",
                        Some(&body),
                    )
                });
                let wait = wait_for_final_commit_pause(fixture);
                if let Err(error) = wait {
                    release_final_commit_pause(fixture, capability)?;
                    let _ = tick.join();
                    return Err(error);
                }
                fixture.crash_daemon()?;
                let interrupted = tick.join().map_err(|_| anyhow::anyhow!("tick panicked"))?;
                anyhow::ensure!(
                    interrupted.is_err(),
                    "crash unexpectedly returned completed apply evidence"
                );
                let pending = Path::new(staged["pendingPath"].as_str().context("pending")?);
                let claimed = pending.with_file_name(format!(
                    "{}.approve.deciding",
                    pending.file_name().context("filename")?.to_string_lossy()
                ));
                let document: Value = serde_json::from_slice(&fs::read(&claimed)?)?;
                anyhow::ensure!(
                    document["decisionState"].is_object(),
                    "crash lacked durable applying evidence"
                );
                anyhow::ensure!(
                    document["autoRegressionEvidence"].as_array().map(Vec::len) == Some(32)
                );
                anyhow::ensure!(
                    fs::read_to_string(fixture.workspace.join(FORMAT_PATH))? == FORMAT_BEFORE
                );
                let clock = fixture
                    .coven_home
                    .join("test-fixtures/threads-deterministic-clock");
                fs::remove_file(clock.join("pause-final-commit"))?;
                fs::remove_file(clock.join("pause-final-commit.reached"))?;
                if missing_ward {
                    fs::remove_file(fixture.workspace.join("ward.toml"))?;
                }
                fixture.start_daemon()?;
                tick_scheduler(fixture, capability)?;
                tick_scheduler(fixture, capability)?;
                let terminals: i64 = fixture.store()?.query_row(
                                            "SELECT COUNT(*) FROM ward_audit WHERE proposal_id=?1 AND event_type IN ('proposal_approved','proposal_rejected','proposal_vetoed')",
                                            [id], |row| row.get(0),
                                        )?;
                if missing_ward {
                    anyhow::ensure!(
                        terminals == 0,
                        "ambiguous applying recovery fabricated a terminal"
                    );
                    anyhow::ensure!(
                        fs::read_to_string(fixture.workspace.join(FORMAT_PATH))? == FORMAT_BEFORE
                    );
                    let quarantined = fs::read_dir(fixture.coven_home.join("pending/quarantine"))?
                        .collect::<std::io::Result<Vec<_>>>()?;
                    anyhow::ensure!(!quarantined.is_empty() && !claimed.exists());
                } else {
                    anyhow::ensure!(terminals == 1 && !claimed.exists());
                    anyhow::ensure!(
                        fs::read_to_string(fixture.workspace.join(FORMAT_PATH))? == FORMAT_AFTER
                    );
                }
                Ok(())
            },
        )?;
    }
    Ok(())
}
