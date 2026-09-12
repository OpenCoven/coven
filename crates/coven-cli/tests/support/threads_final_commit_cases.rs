use super::*;

#[test]
#[cfg(feature = "threads-test-clock")]
fn fixture_marker_is_invisible_until_fully_written() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("marker");
    write_private_fixture_file_with(&path, |file| {
        for part in [b"cap".as_slice(), b"ability".as_slice()] {
            assert_eq!(
                fs::symlink_metadata(&path).unwrap_err().kind(),
                std::io::ErrorKind::NotFound,
                "an unfinished marker was published"
            );
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                assert_eq!(file.metadata()?.permissions().mode() & 0o777, 0o600);
            }
            file.write_all(part)?;
        }
        assert!(!path.exists(), "marker became visible before publication");
        Ok(())
    })?;
    assert_eq!(fs::read(&path)?, b"capability");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(fs::metadata(&path)?.permissions().mode() & 0o777, 0o600);
    }
    assert_eq!(fs::read_dir(directory.path())?.count(), 1);
    Ok(())
}

#[test]
#[cfg(feature = "threads-test-clock")]
fn fixture_marker_never_clobbers_an_existing_destination() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("marker");
    fs::write(&path, b"existing")?;
    let permissions = fs::metadata(&path)?.permissions();
    let error = write_private_fixture_file(&path, "replacement").unwrap_err();
    assert_eq!(
        error.downcast_ref::<std::io::Error>().unwrap().kind(),
        std::io::ErrorKind::AlreadyExists
    );
    assert_eq!(fs::read(&path)?, b"existing");
    assert_eq!(fs::metadata(&path)?.permissions(), permissions);
    assert_eq!(fs::read_dir(directory.path())?.count(), 1);
    Ok(())
}

#[test]
#[cfg(all(unix, feature = "threads-test-clock"))]
fn fixture_marker_never_replaces_existing_or_dangling_symlinks() -> Result<()> {
    for dangling in [false, true] {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("marker");
        let target = directory.path().join("target");
        if !dangling {
            fs::write(&target, b"existing")?;
        }
        std::os::unix::fs::symlink(&target, &path)?;
        let error = write_private_fixture_file(&path, "replacement").unwrap_err();
        assert_eq!(
            error.downcast_ref::<std::io::Error>().unwrap().kind(),
            std::io::ErrorKind::AlreadyExists
        );
        assert_eq!(fs::read_link(&path)?, target);
        if dangling {
            assert!(!target.exists());
        } else {
            assert_eq!(fs::read(&target)?, b"existing");
        }
        assert_eq!(fs::read_dir(directory.path())?.count(), if dangling { 1 } else { 2 });
    }
    Ok(())
}

#[test]
#[cfg(feature = "threads-test-clock")]
fn fixture_marker_never_clobbers_a_concurrent_destination() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("marker");
    let error = write_private_fixture_file_with(&path, |file| {
        file.write_all(b"replacement")?;
        fs::write(&path, b"existing")?;
        Ok(())
    })
    .expect_err("publication must refuse the destination created during staging");
    assert_eq!(
        error.downcast_ref::<std::io::Error>().unwrap().kind(),
        std::io::ErrorKind::AlreadyExists
    );
    assert_eq!(fs::read(&path)?, b"existing");
    assert_eq!(fs::read_dir(directory.path())?.count(), 1);
    Ok(())
}

#[test]
#[cfg(feature = "threads-test-clock")]
fn fixture_marker_write_failure_leaves_no_published_or_staged_file() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("marker");
    let error = write_private_fixture_file_with(&path, |file| {
        file.write_all(b"partial")?;
        anyhow::bail!("injected marker write failure")
    })
    .unwrap_err();
    assert!(error.to_string().contains("injected marker write failure"));
    assert!(!path.exists(), "failed write published partial marker bytes");
    assert_eq!(fs::read_dir(directory.path())?.count(), 0);
    Ok(())
}

#[test]
#[cfg(feature = "threads-test-clock")]
fn final_commit_release_publication_does_not_expose_an_empty_capability() -> Result<()> {
    run_final_commit_release_publication("final-commit-release-publication", None)
}

#[test]
#[cfg(feature = "threads-test-clock")]
fn final_commit_release_refuses_fully_published_malformed_markers() -> Result<()> {
    for (scenario, contents, message) in [
        ("final-commit-release-empty", "", "capability is invalid"),
        ("final-commit-release-control", "cap\nsuffix", "capability is invalid"),
        ("final-commit-release-mismatch", "wrong-cap", "capability was rejected"),
    ] {
        run_final_commit_release_publication(scenario, Some((contents, message)))?;
    }
    Ok(())
}

#[cfg(feature = "threads-test-clock")]
fn run_final_commit_release_publication(
    scenario: &str,
    invalid: Option<(&str, &str)>,
) -> Result<()> {
    let corpus = retired_ward_corpus()?;
    let case = retired_review_case(&corpus)?;
    let duration = case["approval"]["veto"]["duration_seconds"]
        .as_i64()
        .context("corpus veto duration")?;
    run_clocked_journey(
        scenario,
        |home, workspace| seed_retired_review_case(home, workspace, case),
        |fixture, capability| {
            let staged = submit_retired_case(fixture, case)?;
            let id = staged["proposalId"].as_str().context("proposal id")?;
            tick_scheduler(fixture, capability)?;
            advance_clock_to_offset(fixture, capability, duration + 1)?;
            arm_final_commit_pause(fixture, capability)?;

            thread::scope(|scope| {
                let tick_home = fixture.coven_home.clone();
                let tick_body = json!({ "capability": capability });
                let mut tick = Some(scope.spawn(move || {
                    daemon_http_request(
                        &tick_home,
                        "POST",
                        "/api/v1/internal/threads/test-clock/tick",
                        Some(&tick_body),
                    )
                }));
                wait_for_final_commit_pause(fixture)?;
                let release = final_commit_pause_dir(fixture).join("pause-final-commit.release");
                let contents = invalid.map_or(capability, |(contents, _)| contents);
                write_private_fixture_file_with(&release, |file| {
                    if release.try_exists()? {
                        // Hold the writer before its first byte so the real daemon,
                        // not a simulated reader, observes the publication gap.
                        let visible = fs::read(&release)?;
                        let response = tick.take().context("tick thread")?.join()
                            .map_err(|_| anyhow::anyhow!("scheduler tick thread panicked"))??;
                        let log = fs::read_to_string(fixture.coven_home.join("daemon-recovery.log"))?;
                        anyhow::bail!(
                            "{}",
                            fixture.sanitize_fixture_text(&format!(
                                "unfinished release marker visible: bytes={visible:?}; tick={response:?}; recovery log:\n{log}"
                            ))
                        );
                    }
                    let (first, rest) = contents.as_bytes().split_at(contents.len() / 2);
                    file.write_all(first)?;
                    anyhow::ensure!(!release.try_exists()?, "partial capability was published");
                    file.write_all(rest)?;
                    Ok(())
                })?;
                let (status, body) = tick.take().context("tick thread")?.join()
                    .map_err(|_| anyhow::anyhow!("scheduler tick thread panicked"))??;
                let body: Value = serde_json::from_str(&body)?;
                let expected_processed = if invalid.is_some() { 0 } else { 1 };
                anyhow::ensure!(
                    status == 200 && body["processed"] == expected_processed,
                    "unexpected fully published release outcome: HTTP {status} {body}"
                );
                if let Some((contents, message)) = invalid {
                    let log = fixture.sanitize_fixture_text(&fs::read_to_string(
                        fixture.coven_home.join("daemon-recovery.log"),
                    )?);
                    anyhow::ensure!(log.contains(message), "missing capability refusal: {log}");
                    anyhow::ensure!(
                        fs::read(&release)? == contents.as_bytes(),
                        "invalid marker was consumed or replaced"
                    );
                }
                Ok::<_, anyhow::Error>(())
            })?;
            if invalid.is_some() {
                assert_corpus_bytes(fixture, case, "before")?;
                let applied: i64 = fixture.store()?.query_row(
                    "SELECT COUNT(*) FROM ward_audit
                     WHERE event_type IN ('proposal_approved', 'apply_audit')",
                    [],
                    |row| row.get(0),
                )?;
                anyhow::ensure!(applied == 0, "invalid marker allowed applied-write evidence");
            } else {
                assert_window_terminal(fixture, id, "proposal_approved", "applied", json!(true))?;
                assert_corpus_bytes(fixture, case, "after")?;
            }
            Ok(())
        },
    )
}

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
