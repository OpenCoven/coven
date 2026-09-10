use super::*;

#[test]
#[cfg(feature = "threads-test-clock")]
fn final_commit_identity_drift_closes_opened_window_without_applying() -> Result<()> {
    let corpus = retired_ward_corpus()?;
    let case = retired_review_case(&corpus)?;
    let duration = case["approval"]["veto"]["duration_seconds"]
        .as_i64()
        .context("corpus veto duration")?;

    run_clocked_journey(
        "final-commit-identity-drift",
        |home, workspace| seed_retired_review_case(home, workspace, case),
        |fixture, capability| {
            let staged = submit_retired_case(fixture, case)?;
            let id = staged["proposalId"].as_str().context("proposal id")?;
            tick_scheduler(fixture, capability)?;
            advance_clock_to_offset(fixture, capability, duration + 1)?;
            arm_final_commit_pause(fixture, capability)?;

            let tick_home = fixture.coven_home.clone();
            let tick_body = json!({ "capability": capability });
            let tick = thread::spawn(move || {
                daemon_http_request(
                    &tick_home,
                    "POST",
                    "/api/v1/internal/threads/test-clock/tick",
                    Some(&tick_body),
                )
            });

            let wait_result = wait_for_final_commit_pause(fixture);
            if wait_result.is_ok() {
                fs::write(
                    fixture.workspace.join("IDENTITY.md"),
                    "# IDENTITY.md - Synthetic-other\n- **Pronouns:** they/them\n",
                )?;
            }
            release_final_commit_pause(fixture, capability)?;
            wait_result?;

            let (status, body) = tick
                .join()
                .map_err(|_| anyhow::anyhow!("scheduler tick thread panicked"))??;
            let body: Value = serde_json::from_str(&body)
                .with_context(|| format!("scheduler tick returned non-JSON body: {body}"))?;
            anyhow::ensure!(
                status == 200 && body["processed"] == 1,
                "scheduler did not terminalize the drifted proposal: HTTP {status} {body}"
            );
            assert_window_terminal(
                fixture,
                id,
                "proposal_rejected",
                "revalidation_failed",
                json!(false),
            )?;
            assert_corpus_bytes(fixture, case, "before")?;
            let applied: i64 = fixture.store()?.query_row(
                "SELECT COUNT(*) FROM ward_audit
                 WHERE event_type IN ('proposal_approved', 'apply_audit')",
                [],
                |row| row.get(0),
            )?;
            anyhow::ensure!(
                applied == 0,
                "final authority drift produced applied-write evidence"
            );
            let remaining = fixture.request("GET", "/api/v1/threads/proposals", None)?;
            anyhow::ensure!(
                remaining.status == 200 && remaining.body["proposals"] == json!([]),
                "drifted opened window remained pending: {remaining:?}"
            );
            Ok(())
        },
    )
}

#[cfg(feature = "threads-test-clock")]
fn final_commit_pause_dir(fixture: &ThreadsFixture) -> PathBuf {
    fixture
        .coven_home
        .join("test-fixtures")
        .join("threads-deterministic-clock")
}

#[cfg(feature = "threads-test-clock")]
fn arm_final_commit_pause(fixture: &ThreadsFixture, capability: &str) -> Result<()> {
    write_private_fixture_file(
        &final_commit_pause_dir(fixture).join("pause-final-commit"),
        capability,
    )
}

#[cfg(feature = "threads-test-clock")]
fn release_final_commit_pause(fixture: &ThreadsFixture, capability: &str) -> Result<()> {
    write_private_fixture_file(
        &final_commit_pause_dir(fixture).join("pause-final-commit.release"),
        capability,
    )
}

#[cfg(feature = "threads-test-clock")]
fn wait_for_final_commit_pause(fixture: &ThreadsFixture) -> Result<()> {
    let reached = final_commit_pause_dir(fixture).join("pause-final-commit.reached");
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match fs::read_to_string(&reached) {
            Ok(marker) => {
                anyhow::ensure!(
                    marker.trim_end_matches(['\r', '\n'])
                        == "threads_test_final_commit_pause_reached_v1",
                    "unexpected final commit pause marker: {marker:?}"
                );
                return Ok(());
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                anyhow::ensure!(
                    Instant::now() < deadline,
                    "timed out waiting for final commit pause"
                );
                thread::sleep(Duration::from_millis(10));
            }
            Err(error) => {
                return Err(error).with_context(|| {
                    format!("reading final commit pause marker {}", reached.display())
                });
            }
        }
    }
}
