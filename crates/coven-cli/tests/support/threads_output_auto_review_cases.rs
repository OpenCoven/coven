#[cfg(unix)]
mod chain_cases {
    use super::*;
    include!("threads_output_auto_chain_cases.rs");
}

#[test]
fn output_auto_review_normalized_declared_paths_still_route() -> Result<()> {
    for human in [false, true] {
        run_clocked_journey(
            &format!("output-auto-review-normalized-{human}"),
            |home, workspace| {
                seed_output_auto(home, workspace, false)?;
                if human {
                    stronger_policy(workspace)?;
                }
                Ok(())
            },
            |fixture, _| {
                for target in [
                    "OUTPUT-FORMAT.JSON",
                    "./output-format.json",
                    "contained/../output-format.json",
                ] {
                    let response = fixture.request(
                        "POST",
                        "/api/v1/familiars/sage/edits",
                        Some(&json!({"edits":[{"target":target,"contents":"malformed"}]})),
                    )?;
                    anyhow::ensure!(
                        response.status == 409
                            && response.body["error"]["code"] == "scheduled_publication_invalid",
                        "normalized path lost its configured route: {response:?}"
                    );
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

fn finish_supported_replacement(
    fixture: &mut ThreadsFixture,
    capability: &str,
    staged: &Value,
    human: bool,
) -> Result<()> {
    anyhow::ensure!(fs::read_to_string(fixture.workspace.join(FORMAT_PATH))? == FORMAT_BEFORE);
    tick_scheduler(fixture, capability)?;
    if human {
        anyhow::ensure!(fs::read_to_string(fixture.workspace.join(FORMAT_PATH))? == FORMAT_BEFORE);
        let id = staged["proposalId"].as_str().context("id")?;
        let payload = proposal_decision_payload(fixture, id, "Synthetic alias-boundary approval")?;
        let response = fixture.request(
            "POST",
            &format!("/api/v1/threads/proposals/{id}/approve"),
            Some(&payload),
        )?;
        anyhow::ensure!(response.status == 200, "{response:?}");
    }
    anyhow::ensure!(fs::read_to_string(fixture.workspace.join(FORMAT_PATH))? == FORMAT_AFTER);
    Ok(())
}

#[cfg(unix)]
#[test]
fn output_auto_review_inward_symlink_preserves_canonical_ceremony() -> Result<()> {
    for human in [false, true] {
        run_clocked_journey(
            &format!("output-auto-review-inward-symlink-{human}"),
            |home, workspace| {
                seed_output_auto(home, workspace, false)?;
                if human {
                    stronger_policy(workspace)?;
                }
                std::os::unix::fs::symlink(FORMAT_PATH, workspace.join("format-alias.json"))?;
                Ok(())
            },
            |fixture, capability| {
                let response = fixture.request(
                    "POST",
                    "/api/v1/familiars/sage/edits",
                    Some(
                        &json!({"edits":[{"target":"format-alias.json","contents":FORMAT_AFTER}]}),
                    ),
                )?;
                anyhow::ensure!(
                    response.status == 202,
                    "inward alias bypassed staging: {response:?}"
                );
                finish_supported_replacement(fixture, capability, &response.body, human)
            },
        )?;
    }
    Ok(())
}

#[test]
fn output_auto_review_canonical_hardlink_keeps_staging_and_batch_isolation() -> Result<()> {
    for human in [false, true] {
        run_clocked_journey(
            &format!("output-auto-review-canonical-hardlink-{human}"),
            |home, workspace| {
                seed_output_auto(home, workspace, false)?;
                if human {
                    stronger_policy(workspace)?;
                }
                fs::hard_link(
                    workspace.join(FORMAT_PATH),
                    workspace.join("format-alias.json"),
                )?;
                Ok(())
            },
            |fixture, capability| {
                let mixed = fixture.request(
                    "POST",
                    "/api/v1/familiars/sage/edits",
                    Some(&json!({"edits":[
                        {"target":FORMAT_PATH,"contents":FORMAT_AFTER},
                        {"target":"format-alias.json","contents":FORMAT_AFTER}
                    ]})),
                )?;
                anyhow::ensure!(mixed.status == 409);
                anyhow::ensure!(
                    fs::read_to_string(fixture.workspace.join(FORMAT_PATH))? == FORMAT_BEFORE
                );
                anyhow::ensure!(
                    fs::read_to_string(fixture.workspace.join("format-alias.json"))?
                        == FORMAT_BEFORE
                );
                let staged = post_format(fixture, FORMAT_AFTER)?;
                anyhow::ensure!(staged.status == 202, "{staged:?}");
                finish_supported_replacement(fixture, capability, &staged.body, human)?;
                anyhow::ensure!(
                    fs::read_to_string(fixture.workspace.join("format-alias.json"))?
                        == FORMAT_BEFORE,
                    "canonical atomic replacement must not write through the other hardlink"
                );
                Ok(())
            },
        )?;
    }
    Ok(())
}

fn stronger_policy(workspace: &Path) -> Result<()> {
    let path = workspace.join("ward.toml");
    let policy = fs::read_to_string(&path)?
        .replace("[approval_tiers.auto]", "[approval_tiers.human_review]")
        .replace("gate = \"regression_suite\"", "gate = \"human_approval\"");
    fs::write(path, policy)?;
    Ok(())
}

fn assert_alias_refused(fixture: &mut ThreadsFixture, target: &str, mixed: bool) -> Result<()> {
    for contents in ["malformed formatting must not apply", FORMAT_AFTER] {
        let mut edits = vec![json!({"target":target,"contents":contents})];
        if mixed {
            edits.push(json!({"target":"unrelated.txt","contents":"must not change"}));
        }
        let response = fixture.request(
            "POST",
            "/api/v1/familiars/sage/edits",
            Some(&json!({"edits":edits})),
        )?;
        anyhow::ensure!(
            response.status == 409
                && response.body["error"]["code"] == "scheduled_publication_invalid",
            "unsupported alias escaped routing: {response:?}"
        );
    }
    anyhow::ensure!(
        fs::read_to_string(fixture.workspace.join("unrelated.txt"))? == "original unrelated"
    );
    let published: i64 = fixture.store()?.query_row(
        "SELECT COUNT(*) FROM ward_audit WHERE event_type IN ('proposal_submitted','apply_audit','proposal_approved')",
        [], |row| row.get(0),
    )?;
    anyhow::ensure!(
        published == 0,
        "alias refusal published or applied a proposal"
    );
    Ok(())
}

#[cfg(unix)]
fn outgoing_symlink_case(human: bool, mixed: bool) -> Result<()> {
    run_clocked_journey(
        &format!("output-auto-review-outgoing-symlink-{human}-{mixed}"),
        |home, workspace| {
            seed_output_auto(home, workspace, false)?;
            if human {
                stronger_policy(workspace)?;
            }
            fs::write(workspace.join("notes.json"), FORMAT_BEFORE)?;
            fs::write(workspace.join("unrelated.txt"), "original unrelated")?;
            fs::remove_file(workspace.join(FORMAT_PATH))?;
            std::os::unix::fs::symlink("notes.json", workspace.join(FORMAT_PATH))?;
            Ok(())
        },
        |fixture, _| {
            for target in [
                FORMAT_PATH,
                "./output-format.json",
                "contained/../output-format.json",
            ] {
                assert_alias_refused(fixture, target, mixed)?;
                anyhow::ensure!(
                    fs::read_to_string(fixture.workspace.join("notes.json"))? == FORMAT_BEFORE
                );
                anyhow::ensure!(fs::symlink_metadata(fixture.workspace.join(FORMAT_PATH))?
                    .file_type()
                    .is_symlink());
            }
            Ok(())
        },
    )
}

#[cfg(unix)]
#[test]
fn output_auto_review_symlink_auto_single() -> Result<()> {
    outgoing_symlink_case(false, false)
}
#[cfg(unix)]
#[test]
fn output_auto_review_symlink_auto_mixed() -> Result<()> {
    outgoing_symlink_case(false, true)
}
#[cfg(unix)]
#[test]
fn output_auto_review_symlink_human_single() -> Result<()> {
    outgoing_symlink_case(true, false)
}
#[cfg(unix)]
#[test]
fn output_auto_review_symlink_human_mixed() -> Result<()> {
    outgoing_symlink_case(true, true)
}

fn hardlink_case(human: bool, mixed: bool) -> Result<()> {
    for reverse in [false, true] {
        run_clocked_journey(
            &format!("output-auto-review-hardlink-{human}-{mixed}-{reverse}"),
            |home, workspace| {
                seed_output_auto(home, workspace, false)?;
                if human {
                    stronger_policy(workspace)?;
                }
                fs::write(workspace.join("unrelated.txt"), "original unrelated")?;
                if reverse {
                    fs::rename(
                        workspace.join(FORMAT_PATH),
                        workspace.join("format-alias.json"),
                    )?;
                    fs::hard_link(
                        workspace.join("format-alias.json"),
                        workspace.join(FORMAT_PATH),
                    )?;
                } else {
                    fs::hard_link(
                        workspace.join(FORMAT_PATH),
                        workspace.join("format-alias.json"),
                    )?;
                }
                Ok(())
            },
            |fixture, _| {
                assert_alias_refused(fixture, "format-alias.json", mixed)?;
                anyhow::ensure!(
                    fs::read_to_string(fixture.workspace.join("format-alias.json"))?
                        == FORMAT_BEFORE
                );
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
fn output_auto_review_hardlink_auto_single() -> Result<()> {
    hardlink_case(false, false)
}
#[test]
fn output_auto_review_hardlink_auto_mixed() -> Result<()> {
    hardlink_case(false, true)
}
#[test]
fn output_auto_review_hardlink_human_single() -> Result<()> {
    hardlink_case(true, false)
}
#[test]
fn output_auto_review_hardlink_human_mixed() -> Result<()> {
    hardlink_case(true, true)
}

fn representation_case(shape: &str, before: bool) -> Result<()> {
    let invalid = if shape == "array" {
        r#"["coven.output-format/v1",4,false]"#
    } else {
        r#"{"schema":{"coven.output-format/v1":null},"indent":4,"final_newline":false}"#
    };
    run_clocked_journey(
        &format!("output-auto-review-shape-{shape}-{before}"),
        |home, workspace| {
            seed_output_auto(home, workspace, false)?;
            if before {
                fs::write(workspace.join(FORMAT_PATH), invalid)?;
            }
            Ok(())
        },
        |fixture, capability| {
            let response = post_format(fixture, if before { FORMAT_AFTER } else { invalid })?;
            anyhow::ensure!(
                response.status == 409
                    && response.body["error"]["code"] == "scheduled_publication_invalid",
                "forbidden representation passed the closed schema: {response:?}"
            );
            tick_scheduler(fixture, capability)?;
            anyhow::ensure!(
                fs::read_to_string(fixture.workspace.join(FORMAT_PATH))?
                    == if before { invalid } else { FORMAT_BEFORE }
            );
            Ok(())
        },
    )
}

#[test]
fn output_auto_review_array_before() -> Result<()> {
    representation_case("array", true)
}
#[test]
fn output_auto_review_array_after() -> Result<()> {
    representation_case("array", false)
}
#[test]
fn output_auto_review_tagged_schema_before() -> Result<()> {
    representation_case("tagged", true)
}
#[test]
fn output_auto_review_tagged_schema_after() -> Result<()> {
    representation_case("tagged", false)
}

#[test]
fn output_auto_review_unrelated_hardlinks_remain_ordinary() -> Result<()> {
    run_clocked_journey(
        "output-auto-review-unrelated-hardlinks",
        |home, workspace| {
            seed_output_auto(home, workspace, false)?;
            fs::write(workspace.join("ordinary.txt"), FORMAT_BEFORE)?;
            fs::hard_link(
                workspace.join("ordinary.txt"),
                workspace.join("ordinary-alias.txt"),
            )?;
            Ok(())
        },
        |fixture, _| {
            let response = fixture.request("POST", "/api/v1/familiars/sage/edits",
                Some(&json!({"edits":[{"target":"ordinary-alias.txt","contents":"ordinary replacement"}]})))?;
            anyhow::ensure!(
                response.status == 200 && response.body["disposition"] == "applied",
                "{response:?}"
            );
            anyhow::ensure!(
                fs::read_to_string(fixture.workspace.join("ordinary.txt"))? == FORMAT_BEFORE
            );
            anyhow::ensure!(
                fs::read_to_string(fixture.workspace.join("ordinary-alias.txt"))?
                    == "ordinary replacement"
            );
            Ok(())
        },
    )
}
