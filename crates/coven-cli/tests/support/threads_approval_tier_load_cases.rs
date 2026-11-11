use super::*;

const NOTE: &str = "synthetic approval-tier load control";

fn ward_source(tier: &str, gate: &str, unknown: bool) -> String {
    let timing = if tier == "familiar_review" {
        "human_veto_window_hours = 1\nmin_visible_seconds = 900\n"
    } else {
        ""
    };
    let extra = if unknown {
        "[approval_tiers.mystery_review]\nblocks = [\"tool_defaults\"]\ngate = \"familiar_coherence_check\"\n"
    } else {
        ""
    };
    format!(
        r#"principal_key_fingerprint = "{PRINCIPAL_FINGERPRINT}"
protected_surface = ["SOUL.md"]

[editable]
harness_blocks = ["tool_defaults"]

[approval_tiers.{tier}]
blocks = ["tool_defaults"]
gate = "{gate}"
{timing}
{extra}
[[surface]]
path = "SOUL.md"
tier = 0

[[surface]]
path = "TOOLS.md"
tier = 1
"#
    )
}

fn snapshot(fixture: &ThreadsFixture, label: &str) -> Result<(String, String)> {
    let workspace = inventory(&fixture.workspace)?;
    let audit = ward_audit_jsonl(&fixture.coven_home.join("coven.sqlite3"))?;
    let state = fixture.artifact_dir.join("state");
    fs::create_dir_all(&state)?;
    for (name, contents) in [("workspace", &workspace), ("ward-audit", &audit)] {
        fs::write(
            state.join(format!("load-{label}-{name}.txt")),
            fixture.sanitize_fixture_text(contents),
        )?;
    }
    Ok((workspace, audit))
}

fn load_case(name: &str, tier: &str, gate: &str, expected_gate: &str, unknown: bool) -> Result<()> {
    let source = ward_source(tier, gate, unknown);
    let expected_error = if unknown {
        Some("approval_tiers contains unknown tier `mystery_review`".to_owned())
    } else if gate != expected_gate {
        Some(format!(
            "approval_tiers.{tier}.gate must be `{expected_gate}`, found `{gate}`"
        ))
    } else {
        None
    };
    run_fixture_journey(
        name,
        |evidence| {
            ThreadsFixture::start_with_setup_and_start(
                evidence,
                |_, workspace| {
                    fs::write(workspace.join("ward.toml"), &source)?;
                    fs::write(workspace.join("TOOLS.md"), "# Synthetic tool defaults\n")?;
                    Ok(())
                },
                |fixture| {
                    fixture.start_owned_daemon_with_readiness(|child, home| {
                        child.wait_for_health(home)
                    })
                },
            )
        },
        |fixture| {
            let before = snapshot(fixture, "before")?;
            anyhow::ensure!(before.1.is_empty(), "fresh fixture already has Ward audit rows");
            let state = fixture.artifact_dir.join("state");
            fs::write(state.join("load-ward.toml"), &source)?;
            fs::write(
                state.join("load-sqlite-schema.txt"),
                sqlite_schema(&fixture.coven_home.join("coven.sqlite3"))?,
            )?;
            // This is the ordinary Tier-2 note control, not a positive principal-
            // authority fixture. The synthetic fingerprint grants no Tier-0 write.
            let response = fixture.request(
                "POST",
                "/api/v1/familiars/sage/edits",
                Some(&json!({
                    "edits": [{"target": "notes/load-control.md", "contents": NOTE}],
                    "principalKeyFingerprint": PRINCIPAL_FINGERPRINT,
                })),
            )?;
            let after = snapshot(fixture, "after")?;
            for (path, expected) in [
                ("ward.toml", source.as_bytes()),
                ("SOUL.md", b"# Sage\n".as_slice()),
                ("TOOLS.md", b"# Synthetic tool defaults\n".as_slice()),
            ] {
                anyhow::ensure!(
                    fs::read(fixture.workspace.join(path))? == expected,
                    "request changed {path}"
                );
            }
            anyhow::ensure!(
                inventory(&fixture.coven_home.join("pending"))? == "<absent>\n",
                "load case left pending proposal state"
            );
            if let Some(reason) = &expected_error {
                anyhow::ensure!(
                    response.status == 500
                        && response.body["error"]["code"] == "ward_config_invalid",
                    "malformed load did not return its typed refusal: {response:?}"
                );
                anyhow::ensure!(
                    response.body["error"]["message"]
                        .as_str()
                        .is_some_and(|message| message.contains(reason)),
                    "load failed for a different reason than {reason}: {response:?}"
                );
                anyhow::ensure!(after == before, "malformed Ward changed workspace or audit");
                anyhow::ensure!(
                    !fixture.workspace.join("notes/load-control.md").exists(),
                    "malformed Ward admitted the control write"
                );
            } else {
                anyhow::ensure!(
                    response.status == 200
                        && response.body["disposition"] == "applied"
                        && response.body["threadsGate"]["outcome"]["kind"] == "permitted",
                    "valid load control did not apply: {response:?}"
                );
                anyhow::ensure!(
                    fs::read(fixture.workspace.join("notes/load-control.md"))? == NOTE.as_bytes(),
                    "control bytes do not match the request"
                );
                let rows: i64 = fixture.store()?.query_row(
                    "SELECT COUNT(*) FROM ward_audit WHERE familiar_id = ?1 AND event_type = 'apply_audit'",
                    [FAMILIAR_ID],
                    |row| row.get(0),
                )?;
                anyhow::ensure!(rows == 1, "control needs exactly one apply audit, got {rows}");
                let audit = &response.body["changes"][0]["audit"];
                anyhow::ensure!(
                    audit["nextSha256"] == hex_bytes(&Sha256::digest(NOTE.as_bytes()))
                        && audit["bytesWritten"] == NOTE.len(),
                    "control audit does not bind the exact written bytes: {audit}"
                );
            }
            fixture.stop_daemon()?;
            anyhow::ensure!(
                snapshot(fixture, "stopped")? == after,
                "workspace or audit changed after shutdown"
            );
            anyhow::ensure!(
                inventory(&fixture.coven_home.join("pending"))? == "<absent>\n",
                "shutdown left pending proposal state"
            );
            Ok(())
        },
    )
}

macro_rules! tier_load_cases {
    ($($name:ident: ($tier:literal, $gate:literal, $expected:literal, $unknown:literal)),+ $(,)?) => {
        $(
            #[test]
            fn $name() -> Result<()> {
                load_case(stringify!($name), $tier, $gate, $expected, $unknown)
            }
        )+
    };
}

tier_load_cases! {
    approval_tier_load_valid_auto: ("auto", "regression_suite", "regression_suite", false),
    approval_tier_load_valid_familiar_review: ("familiar_review", "familiar_coherence_check", "familiar_coherence_check", false),
    approval_tier_load_valid_human_review: ("human_review", "human_approval", "human_approval", false),
    approval_tier_load_valid_human_required: ("human_required", "human_approval_with_rationale", "human_approval_with_rationale", false),
    approval_tier_load_unknown: ("familiar_review", "familiar_coherence_check", "familiar_coherence_check", true),
    approval_tier_load_auto_as_familiar: ("auto", "familiar_coherence_check", "regression_suite", false),
    approval_tier_load_auto_as_human: ("auto", "human_approval", "regression_suite", false),
    approval_tier_load_auto_as_rationale: ("auto", "human_approval_with_rationale", "regression_suite", false),
    approval_tier_load_familiar_as_auto: ("familiar_review", "regression_suite", "familiar_coherence_check", false),
    approval_tier_load_familiar_as_human: ("familiar_review", "human_approval", "familiar_coherence_check", false),
    approval_tier_load_familiar_as_rationale: ("familiar_review", "human_approval_with_rationale", "familiar_coherence_check", false),
    approval_tier_load_human_as_auto: ("human_review", "regression_suite", "human_approval", false),
    approval_tier_load_human_as_familiar: ("human_review", "familiar_coherence_check", "human_approval", false),
    approval_tier_load_human_as_rationale: ("human_review", "human_approval_with_rationale", "human_approval", false),
    approval_tier_load_rationale_as_auto: ("human_required", "regression_suite", "human_approval_with_rationale", false),
    approval_tier_load_rationale_as_familiar: ("human_required", "familiar_coherence_check", "human_approval_with_rationale", false),
    approval_tier_load_rationale_as_human: ("human_required", "human_approval", "human_approval_with_rationale", false),
}
