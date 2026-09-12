use super::*;

#[derive(Clone, Copy)]
enum IdentityDrift {
    IdentityBytes,
    SoulBytes,
    RosterMetadata,
    ActivePolicy,
}

impl IdentityDrift {
    fn name(self) -> &'static str {
        match self {
            Self::IdentityBytes => "identity-bytes",
            Self::SoulBytes => "soul-bytes",
            Self::RosterMetadata => "roster-metadata",
            Self::ActivePolicy => "active-policy",
        }
    }

    fn apply(self, fixture: &ThreadsFixture, case: &Value) -> Result<()> {
        match self {
            Self::IdentityBytes | Self::SoulBytes => {
                let source = if matches!(self, Self::IdentityBytes) {
                    "IDENTITY.md"
                } else {
                    "SOUL.md"
                };
                let path = fixture.workspace.join(source);
                let original = fs::read_to_string(&path)?;
                fs::write(
                    path,
                    format!("{original}\n<!-- Synthetic source revision; identity facts unchanged. -->\n"),
                )?;
            }
            Self::RosterMetadata => {
                let path = fixture.coven_home.join("familiars.toml");
                let mut roster: toml::Value = toml::from_str(&fs::read_to_string(&path)?)?;
                let familiar = roster["familiar"]
                    .as_array_mut()
                    .context("fixture familiar roster")?
                    .iter_mut()
                    .find(|entry| entry["id"].as_str() == Some(FAMILIAR_ID))
                    .context("fixture familiar entry")?;
                familiar["description"] =
                    toml::Value::String("Synthetic revised roster metadata.".to_owned());
                fs::write(path, toml::to_string(&roster)?)?;
            }
            Self::ActivePolicy => {
                let path = fixture.workspace.join("ward.toml");
                let mut ward: toml::Value = toml::from_str(&fs::read_to_string(&path)?)?;
                let purpose = ward
                    .get_mut("identity_invariant")
                    .and_then(toml::Value::as_array_mut)
                    .context("active identity declarations")?
                    .iter_mut()
                    .find(|declaration| declaration["fact"].as_str() == Some("purpose"))
                    .context("active purpose declaration")?;
                anyhow::ensure!(
                    purpose["operator"].as_str() == Some("includes"),
                    "fixture purpose must use the includes predicate"
                );
                let full_purpose = case["candidate_facts"]
                    .as_array()
                    .context("corpus candidate facts")?
                    .iter()
                    .find(|fact| fact["fact"] == "purpose")
                    .and_then(|fact| fact["value"].as_str())
                    .context("corpus purpose")?;
                let original = purpose["expected"].as_str().context("expected purpose")?;
                anyhow::ensure!(
                    full_purpose.contains(original) && full_purpose != original,
                    "policy change must strengthen a still-satisfied purpose predicate"
                );
                purpose["expected"] = toml::Value::String(full_purpose.to_owned());
                fs::write(path, toml::to_string(&ward)?)?;
            }
        }
        Ok(())
    }
}

#[test]
fn scheduled_valid_identity_bytes_reject_at_deadline_and_after_restart() -> Result<()> {
    assert_scheduled_valid_identity_drift(IdentityDrift::IdentityBytes)
}

#[test]
fn scheduled_valid_soul_bytes_reject_at_deadline_and_after_restart() -> Result<()> {
    assert_scheduled_valid_identity_drift(IdentityDrift::SoulBytes)
}

#[test]
fn scheduled_valid_roster_metadata_rejects_at_deadline_and_after_restart() -> Result<()> {
    assert_scheduled_valid_identity_drift(IdentityDrift::RosterMetadata)
}

#[test]
fn scheduled_valid_active_policy_rejects_at_deadline_and_after_restart() -> Result<()> {
    assert_scheduled_valid_identity_drift(IdentityDrift::ActivePolicy)
}

fn assert_scheduled_valid_identity_drift(drift: IdentityDrift) -> Result<()> {
    let corpus = retired_ward_corpus()?;
    let case = retired_review_case(&corpus)?;
    let duration = case["approval"]["veto"]["duration_seconds"]
        .as_i64()
        .context("corpus veto duration")?;
    for restart in [false, true] {
        let route = if restart { "restart" } else { "live" };
        run_clocked_journey(
            &format!("scheduled-valid-{}-{route}", drift.name()),
            |home, workspace| seed_retired_review_case(home, workspace, case),
            |fixture, capability| {
                let staged = submit_retired_case(fixture, case)?;
                let id = staged["proposalId"].as_str().context("proposal id")?;
                let pending_path =
                    Path::new(staged["pendingPath"].as_str().context("pending path")?);
                let pending: Value = serde_json::from_slice(&fs::read(pending_path)?)?;
                tick_scheduler(fixture, capability)?;
                let opened = identity_proposal_audit(fixture, id)?;
                anyhow::ensure!(
                    opened.len() == 2
                        && opened[0]["event_type"] == "proposal_submitted"
                        && opened[1]["event_type"] == "proposal_window_opened"
                        && opened[0]["detail"]["classification"] == pending["classification"],
                    "supported intake/opening did not bind one auditable proposal: {opened:?}"
                );
                if restart {
                    fixture.stop_daemon()?;
                }
                drift.apply(fixture, case)?;
                let soul_after_drift = fs::read(fixture.workspace.join("SOUL.md"))?;
                let identity_after_drift = fs::read(fixture.workspace.join("IDENTITY.md"))?;
                if restart {
                    fixture.start_daemon()?;
                }
                advance_clock_to_offset(fixture, capability, duration)?;
                tick_scheduler(fixture, capability)?;
                assert_window_terminal(
                    fixture,
                    id,
                    "proposal_rejected",
                    "evidence_diverged",
                    json!(false),
                )?;
                anyhow::ensure!(
                    pending["identityEvidence"]
                        .as_array()
                        .is_some_and(|binding| binding.len() == 32),
                    "supported scheduled intake did not persist its identity commitment"
                );
                assert_corpus_bytes(fixture, case, "before")?;
                anyhow::ensure!(!pending_path.exists(), "rejected proposal remains on disk");
                let applied: i64 = fixture.store()?.query_row(
                    "SELECT COUNT(*) FROM ward_audit
                     WHERE event_type IN ('proposal_approved', 'apply_audit')",
                    [],
                    |row| row.get(0),
                )?;
                anyhow::ensure!(applied == 0, "stale identity evidence authorized a write");
                let terminal = identity_proposal_audit(fixture, id)?;
                anyhow::ensure!(
                    terminal.len() == 3 && terminal[..2] == opened,
                    "terminal audit lost or rewrote submission/opening correspondence"
                );

                fixture.restart_daemon()?;
                tick_scheduler(fixture, capability)?;
                anyhow::ensure!(
                    identity_proposal_audit(fixture, id)? == terminal,
                    "restart changed or duplicated the terminal audit chain"
                );
                assert_corpus_bytes(fixture, case, "before")?;

                // A fresh proposal under the changed authority must still pass.
                // This distinguishes evidence drift from an invalid predicate.
                let fresh = submit_retired_case(fixture, case)?;
                let fresh_id = fresh["proposalId"].as_str().context("fresh proposal id")?;
                let fresh_pending: Value = serde_json::from_slice(&fs::read(
                    fresh["pendingPath"]
                        .as_str()
                        .context("fresh pending path")?,
                )?)?;
                anyhow::ensure!(
                    fresh_id != id
                        && fresh_pending["identityEvidence"] != pending["identityEvidence"],
                    "fresh intake did not bind the changed identity evidence"
                );
                for field in [
                    "familiar_id",
                    "channel",
                    "affected_surfaces",
                    "affected_regions",
                    "path_tier_floor",
                    "approval_path",
                    "evidence_replay_hash",
                ] {
                    anyhow::ensure!(
                        fresh_pending["classification"][field] == pending["classification"][field],
                        "fresh intake changed {field}, not just identity evidence"
                    );
                }
                tick_scheduler(fixture, capability)?;
                let deadline: time::OffsetDateTime =
                    serde_json::from_value(fresh_pending["veto_deadline"].clone())?;
                advance_clock(
                    fixture,
                    capability,
                    &deadline.format(&time::format_description::well_known::Rfc3339)?,
                )?;
                tick_scheduler(fixture, capability)?;
                assert_window_terminal(
                    fixture,
                    fresh_id,
                    "proposal_approved",
                    "applied",
                    json!(true),
                )?;
                assert_corpus_bytes(fixture, case, "after")?;
                anyhow::ensure!(
                    fs::read(fixture.workspace.join("SOUL.md"))? == soul_after_drift
                        && fs::read(fixture.workspace.join("IDENTITY.md"))? == identity_after_drift,
                    "proposal processing rewrote authoritative identity sources"
                );
                anyhow::ensure!(
                    identity_proposal_audit(fixture, id)? == terminal,
                    "fresh approval altered the stale proposal's terminal record"
                );
                let remaining = fixture.request("GET", "/api/v1/threads/proposals", None)?;
                anyhow::ensure!(
                    remaining.status == 200 && remaining.body["proposals"] == json!([]),
                    "completed identity replay journey left pending authority: {remaining:?}"
                );
                fs::create_dir_all(&fixture.artifact_dir)?;
                fs::write(
                    fixture.artifact_dir.join("identity-replay-proof.json"),
                    serde_json::to_vec_pretty(&json!({
                        "scenario": fixture.scenario,
                        "original_intake": pending,
                        "original_audit": terminal,
                        "fresh_intake": fresh_pending,
                        "fresh_audit": identity_proposal_audit(fixture, fresh_id)?,
                        "stale_proposal_applied_rows": applied,
                        "changed_predicates_still_satisfied": true,
                    }))?,
                )?;
                Ok(())
            },
        )?;
    }
    Ok(())
}

fn identity_proposal_audit(fixture: &ThreadsFixture, id: &str) -> Result<Vec<Value>> {
    let conn = fixture.store()?;
    let mut statement = conn.prepare(
        "SELECT event_type, decision, detail FROM ward_audit WHERE proposal_id = ?1
         AND event_type IN ('proposal_submitted', 'proposal_window_opened',
                            'proposal_approved', 'proposal_rejected', 'proposal_vetoed')
         ORDER BY rowid",
    )?;
    let rows = statement
        .query_map([id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    rows.into_iter()
        .map(|(event, decision, detail)| {
            let detail = detail
                .map(|value| serde_json::from_str::<Value>(&value))
                .transpose()?;
            Ok(json!({"event_type": event, "decision": decision, "detail": detail}))
        })
        .collect()
}
