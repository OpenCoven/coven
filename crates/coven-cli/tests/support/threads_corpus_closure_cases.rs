use super::*;

#[test]
fn retired_corpus_recovery_workers_contend_without_duplicate_apply() -> Result<()> {
    let corpus = retired_ward_corpus()?;
    let case = retired_review_case(&corpus)?;
    let duration = case["approval"]["veto"]["duration_seconds"]
        .as_i64()
        .context("corpus veto duration")?;
    run_clocked_journey(
        "retired-corpus-recovery-worker-contention",
        |home, workspace| seed_retired_review_case(home, workspace, case),
        |fixture, capability| {
            let staged = submit_retired_case(fixture, case)?;
            let id = staged["proposalId"].as_str().context("proposal id")?;
            tick_scheduler(fixture, capability)?;
            fixture.restart_daemon()?;
            advance_clock_to_offset(fixture, capability, duration + 1)?;
            final_commit_cases::arm_final_commit_pause(fixture, capability)?;
            let before = scheduler_entries(fixture)?;
            thread::scope(|scope| -> Result<()> {
                let first_home = fixture.coven_home.clone();
                let first_payload = json!({"capability": capability, "workers": 2});
                let first = scope.spawn(move || {
                    daemon_http_request(
                        &first_home,
                        "POST",
                        "/api/v1/internal/threads/test-clock/tick",
                        Some(&first_payload),
                    )
                });
                let paused = final_commit_cases::wait_for_final_commit_pause(fixture);
                if let Err(error) = paused {
                    final_commit_cases::release_final_commit_pause(fixture, capability)?;
                    first.join().map_err(|_| anyhow::anyhow!("first recovery worker panicked"))??;
                    return Err(error);
                }
                let contended = (|| -> Result<()> {
                    let deadline = Instant::now() + Duration::from_secs(5);
                    while scheduler_entries(fixture)? < before + 2 {
                        anyhow::ensure!(
                            Instant::now() < deadline,
                            "second daemon recovery worker did not reach the held pass lock"
                        );
                        thread::sleep(Duration::from_millis(10));
                    }
                    assert_corpus_bytes(fixture, case, "before")?;
                    let terminals: i64 = fixture.store()?.query_row(
                        "SELECT COUNT(*) FROM ward_audit WHERE proposal_id=?1
                         AND event_type IN ('proposal_approved','proposal_rejected','proposal_vetoed')",
                        [id],
                        |row| row.get(0),
                    )?;
                    anyhow::ensure!(terminals == 0, "paused recovery already terminalized");
                    Ok(())
                })();
                final_commit_cases::release_final_commit_pause(fixture, capability)?;
                let first = first.join().map_err(|_| anyhow::anyhow!("first recovery worker panicked"))??;
                contended?;
                let first_body: Value = serde_json::from_str(&first.1)?;
                let mut workers: Vec<usize> = serde_json::from_value(first_body["workerResults"].clone())?;
                workers.sort_unstable();
                anyhow::ensure!(
                    first.0 == 200 && first_body["processed"] == 1 && workers == [0, 1],
                    "competing recovery results: {first:?}"
                );
                fs::write(
                    fixture.artifact_dir.join("recovery-workers.json"),
                    serde_json::to_vec_pretty(&json!({
                        "tick": first_body,
                        "both_workers_entered_before_final_commit_release": true,
                    }))?,
                )?;
                Ok(())
            })?;
            assert_corpus_bytes(fixture, case, "after")?;
            assert_window_terminal(fixture, id, "proposal_approved", "applied", json!(true))?;
            let intents: i64 = fixture.store()?.query_row(
                "SELECT COUNT(*) FROM ward_audit WHERE proposal_id=?1 AND decision='proposal-apply-intent'",
                [id],
                |row| row.get(0),
            )?;
            anyhow::ensure!(intents == 1, "recovery created {intents} apply intents");
            let committed_audit = ward_audit_jsonl(&fixture.coven_home.join("coven.sqlite3"))?;
            fixture.restart_daemon()?;
            tick_scheduler(fixture, capability)?;
            assert_corpus_bytes(fixture, case, "after")?;
            assert_window_terminal(fixture, id, "proposal_approved", "applied", json!(true))?;
            anyhow::ensure!(
                committed_audit == ward_audit_jsonl(&fixture.coven_home.join("coven.sqlite3"))?,
                "restart replay appended evidence after the only terminal"
            );
            let remaining = fixture.request("GET", "/api/v1/threads/proposals", None)?;
            anyhow::ensure!(remaining.body["proposals"] == json!([]), "recovery left pending authority");
            Ok(())
        },
    )
}

fn scheduler_entries(fixture: &ThreadsFixture) -> Result<usize> {
    Ok(fs::read_to_string(fixture.coven_home.join("daemon-recovery.log"))?
        .lines()
        .filter(|line| line.contains("threads_scheduler_checkpoint phase=pass-lock-wait"))
        .count())
}

#[test]
fn unsupported_retired_identity_declaration_never_migrates_or_stages() -> Result<()> {
    let corpus = retired_ward_corpus()?;
    let mut case = retired_review_case(&corpus)?.clone();
    let unsupported = corpus["unsupported_cases"]
        .as_array()
        .context("unsupported corpus cases")?
        .iter()
        .find(|case| case["id"] == "unknown-identity-fact")
        .context("canonical unsupported declaration")?;
    case["declarations"] = unsupported["declarations"].clone();
    run_clocked_journey(
        "retired-corpus-unsupported-identity-declaration",
        |home, workspace| {
            seed_retired_review_source(home, workspace, &case)?;
            Ok(())
        },
        |fixture, _| {
            let original = fs::read(fixture.workspace.join("ward.toml"))?;
            let migration = migrate_retired_review_source(&fixture.coven_home)?;
            let output = format!(
                "{}\n{}",
                String::from_utf8_lossy(&migration.stdout),
                String::from_utf8_lossy(&migration.stderr),
            );
            anyhow::ensure!(
                !migration.status.success()
                    && output.contains(unsupported["expected_error_contains"].as_str().context("expected refusal")?),
                "unsupported corpus declaration migrated: {output}"
            );
            fs::create_dir_all(&fixture.artifact_dir)?;
            fs::write(
                fixture.artifact_dir.join("migration-refusal.txt"),
                fixture.sanitize_fixture_text(&output),
            )?;
            fs::write(
                fixture.artifact_dir.join("corpus-unsupported.json"),
                serde_json::to_vec_pretty(unsupported)?,
            )?;
            anyhow::ensure!(fs::read(fixture.workspace.join("ward.toml"))? == original);
            anyhow::ensure!(!fixture.workspace.join("ward.toml.v01.bak").exists());
            let edits: Vec<Value> = case["surfaces"].as_array().context("surfaces")?
                .iter()
                .map(|surface| json!({"target": surface["path"], "contents": surface["after"]}))
                .collect();
            let refused = fixture.request(
                "POST",
                "/api/v1/familiars/sage/edits",
                Some(&json!({"edits": edits})),
            )?;
            anyhow::ensure!(
                refused.status == 500 && refused.body["error"]["code"] == "ward_config_invalid",
                "unsupported retired declaration reached staging: {refused:?}"
            );
            assert_corpus_bytes(fixture, &case, "before")?;
            let listed = fixture.request("GET", "/api/v1/threads/proposals", None)?;
            anyhow::ensure!(listed.body["proposals"] == json!([]));
            let authority: i64 = fixture.store()?.query_row(
                "SELECT COUNT(*) FROM ward_audit WHERE event_type IN
                 ('proposal_submitted','proposal_window_opened','proposal_approved','apply_audit')",
                [],
                |row| row.get(0),
            )?;
            anyhow::ensure!(authority == 0, "unsupported migration gained proposal authority");
            Ok(())
        },
    )
}
