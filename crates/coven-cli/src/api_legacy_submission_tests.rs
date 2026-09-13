const LEGACY_BEFORE: &str = "legacy before";
const LEGACY_AFTER: &str = "legacy after";

fn failed_coherence_publication(home: &Path) -> Result<(PathBuf, String)> {
    let workspace = seed_warded_familiar(home)?;
    std::fs::create_dir_all(workspace.join("reviewed"))?;
    std::fs::write(workspace.join("reviewed/skill.md"), LEGACY_BEFORE)?;
    let conn = store::open_store(&store_path(home))?;
    conn.execute_batch(
        "CREATE TRIGGER deny_legacy_submission BEFORE INSERT ON ward_audit
         WHEN NEW.event_type = 'proposal_submitted'
         BEGIN SELECT RAISE(ABORT, 'injected legacy submission failure'); END;",
    )?;
    let error = post_edits(
        home,
        &json!({"edits":[{"target":"reviewed/skill.md","contents":LEGACY_AFTER}]}).to_string(),
    )
    .expect_err("the supported legacy intake must surface the refused receipt");
    assert!(format!("{error:#}").contains("injected legacy submission failure"));
    conn.execute_batch("DROP TRIGGER deny_legacy_submission")?;
    let paths = std::fs::read_dir(home.join("pending"))?.collect::<std::io::Result<Vec<_>>>()?;
    let path = paths
        .iter()
        .map(|entry| entry.path())
        .find(|path| path.extension().is_some_and(|ext| ext == "json"))
        .context("published legacy pending file")?;
    let document = read_pending_proposal_document(&path)?;
    assert!(document.scheduled().is_none());
    assert!(document.decision_request.is_none());
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM ward_audit WHERE event_type='proposal_submitted'",
            [],
            |row| row.get::<_, i64>(0)
        )?,
        0
    );
    Ok((path, document.pending().id.to_string()))
}

fn reservation_rows(conn: &rusqlite::Connection) -> Result<Vec<(String, String, i64)>> {
    let mut statement = conn.prepare(
        "SELECT token, purpose, reserved_bytes FROM coven_ward_audit_reservations ORDER BY token",
    )?;
    let rows = statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

fn corrupt_legacy_receipt(conn: &rusqlite::Connection, mutation: &str, id: &str) -> Result<()> {
    let trigger = if mutation.starts_with("DELETE") {
        Some("ward_audit_append_only_delete")
    } else if mutation.starts_with("UPDATE") {
        Some("ward_audit_append_only_update")
    } else {
        None
    };
    let restore = trigger
        .map(|name| {
            let sql: String = conn.query_row(
                "SELECT sql FROM sqlite_master WHERE type='trigger' AND name=?1",
                [name],
                |row| row.get(0),
            )?;
            conn.execute_batch(&format!("DROP TRIGGER {name}"))?;
            Ok::<_, rusqlite::Error>(sql)
        })
        .transpose()?;
    let mutation_result = conn.execute(mutation, [id]);
    let restore_result = restore.map(|sql| conn.execute_batch(&sql)).transpose();
    mutation_result?;
    restore_result?;
    Ok(())
}

#[test]
fn failed_legacy_publication_cannot_gain_authority_from_a_new_decision() -> Result<()> {
    for days in [0, 31] {
        let temp = tempfile::tempdir()?;
        let home = temp.path();
        let now = time::OffsetDateTime::now_utc();
        let (path, id) =
            crate::threads_clock::with_test_time(home, now, || failed_coherence_publication(home))?;
        let before = std::fs::read(&path)?;
        let conn = store::open_store(&store_path(home))?;
        let reservations = reservation_rows(&conn)?;
        let response =
            crate::threads_clock::with_test_time(home, now + time::Duration::days(days), || {
                decide_threads_proposal(home, &id, "approve", Some("{}"))
            })?;
        assert_eq!(
            response.status, 409,
            "unaudited legacy proposal executed: {}",
            response.body
        );
        let body: Value = serde_json::from_str(&response.body)?;
        assert_eq!(body["why"], "proposal-submission-receipt-invalid");
        assert_eq!(body["quarantined"], true);
        assert_eq!(
            std::fs::read_to_string(home.join("familiars/sage/reviewed/skill.md"))?,
            LEGACY_BEFORE
        );
        assert_eq!(reservation_rows(&conn)?, reservations);
        assert!(proposal_terminal_event(&conn, &id)?.is_none());
        assert!(load_proposal_apply_intent(&conn, &id)?.is_none());
        assert_eq!(
            std::fs::read(body["quarantinePath"].as_str().context("quarantine")?)?,
            before
        );
        assert_eq!(
            decide_threads_proposal(home, &id, "approve", Some("{}"))?.status,
            404
        );
    }
    Ok(())
}

#[test]
fn legacy_authority_intake_records_its_own_bound_submission() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let home = temp.path();
    let workspace = seed_warded_familiar(home)?;
    let config = ward::WardConfig::load(&workspace)?.context("Ward")?;
    let conn = store::open_store(&store_path(home))?;
    let edits = [ward::FileEdit::new("SOUL.md", b"candidate".to_vec())];
    let targets = ["SOUL.md".to_string()];
    let authorization = ward::Authorization::signed_by("fpr-val");
    let request = crate::threads_gate::GateRequest {
        coven_home: home,
        familiar_id: "sage",
        workspace: &workspace,
        config: &config,
        edits: &edits,
        gated_targets: &targets,
        authorization: &authorization,
    };
    assert!(matches!(
        crate::threads_gate::gate_protected_edits(&conn, &request)?.outcome,
        crate::threads_gate::GateOutcome::Permitted
    ));
    std::fs::write(workspace.join("SOUL.md"), "out of band")?;
    let report = crate::threads_gate::gate_protected_edits(&conn, &request)?;
    let crate::threads_gate::GateOutcome::Staged {
        pending_path,
        proposal_id,
    } = report.outcome
    else {
        anyhow::bail!("expected authority staging");
    };
    let document = read_pending_proposal_document(&pending_path)?;
    let (count, decision): (i64, Option<String>) = conn.query_row(
        "SELECT COUNT(*), decision FROM ward_audit WHERE proposal_id=?1 AND event_type='proposal_submitted'",
        [&proposal_id], |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    assert_eq!(
        count, 1,
        "authority intake never recorded the published proposal"
    );
    assert_eq!(decision.as_deref(), Some("staged:authority"));
    assert!(document.review_kind.is_none());
    let response = decide_threads_proposal(home, &proposal_id, "approve", Some("{}"))?;
    assert_eq!(
        response.status, 409,
        "a receipt does not override the protected floor: {}",
        response.body
    );
    let body: Value = serde_json::from_str(&response.body)?;
    assert_eq!(body["why"], "protected-target-not-proposable");
    assert_eq!(
        std::fs::read_to_string(workspace.join("SOUL.md"))?,
        "out of band"
    );
    Ok(())
}

#[test]
fn legacy_submission_receipt_must_match_before_claiming() -> Result<()> {
    for mutation in [
        "DELETE FROM ward_audit WHERE proposal_id=?1",
        "UPDATE ward_audit SET familiar_id='other' WHERE proposal_id=?1",
        "UPDATE ward_audit SET ward_hash=zeroblob(31) WHERE proposal_id=?1",
        "UPDATE ward_audit SET thread_id='other' WHERE proposal_id=?1",
        "UPDATE ward_audit SET files_touched='[\"other\"]' WHERE proposal_id=?1",
        "UPDATE ward_audit SET channel='observation' WHERE proposal_id=?1",
        "UPDATE ward_audit SET submitted_at='other' WHERE proposal_id=?1",
        "UPDATE ward_audit SET decided_at='other' WHERE proposal_id=?1",
        "UPDATE ward_audit SET approver='principal:other' WHERE proposal_id=?1",
        "UPDATE ward_audit SET decision='staged:scheduled' WHERE proposal_id=?1",
        "UPDATE ward_audit SET detail='{}' WHERE proposal_id=?1",
        "UPDATE ward_audit SET tier='0' WHERE proposal_id=?1",
        "INSERT INTO ward_audit (
             event_type, proposal_id, familiar_id, ward_version, ward_hash,
             tier, decision, approver, diff_hash, files_touched, channel,
             thread_id, submitted_at, decided_at, detail
         ) SELECT event_type, proposal_id, familiar_id, ward_version, ward_hash,
             tier, decision, approver, diff_hash, files_touched, channel,
             thread_id, submitted_at, decided_at, detail
         FROM ward_audit WHERE proposal_id=?1",
    ] {
        let temp = tempfile::tempdir()?;
        let home = temp.path();
        let (pending, id, workspace) =
            stage_coherence_edit(home, "reviewed/skill.md", Some(LEGACY_BEFORE), LEGACY_AFTER)?;
        let before = std::fs::read(&pending)?;
        let conn = store::open_store(&store_path(home))?;
        corrupt_legacy_receipt(&conn, mutation, &id)?;
        let reservations = reservation_rows(&conn)?;
        let response = decide_threads_proposal(home, &id, "approve", Some("{}"))?;
        assert_eq!(response.status, 409, "{mutation}: {}", response.body);
        let body: Value = serde_json::from_str(&response.body)?;
        assert_eq!(
            body["why"], "proposal-submission-receipt-invalid",
            "{mutation}"
        );
        assert_eq!(
            std::fs::read(body["quarantinePath"].as_str().context("quarantine")?)?,
            before,
            "{mutation}"
        );
        assert_eq!(
            std::fs::read_to_string(workspace.join("reviewed/skill.md"))?,
            LEGACY_BEFORE,
            "{mutation}"
        );
        assert_eq!(reservation_rows(&conn)?, reservations, "{mutation}");
        assert!(proposal_terminal_event(&conn, &id)?.is_none(), "{mutation}");
        assert!(
            load_proposal_apply_intent(&conn, &id)?.is_none(),
            "{mutation}"
        );
    }
    Ok(())
}

#[test]
fn legacy_submission_scheduler_quarantines_without_inventing_expiry() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let home = temp.path();
    let now = time::OffsetDateTime::now_utc();
    let (pending, id) =
        crate::threads_clock::with_test_time(home, now, || failed_coherence_publication(home))?;
    let before = std::fs::read(&pending)?;
    let conn = store::open_store(&store_path(home))?;
    let reservations = reservation_rows(&conn)?;
    crate::threads_clock::with_test_time(home, now + time::Duration::days(31), || {
        process_due_threads_proposals(home)
    })?;
    assert!(!pending.exists());
    assert!(
        home.join("pending/quarantine").exists(),
        "the scheduler must quarantine an unaudited proposal, not terminalize it"
    );
    let quarantined =
        std::fs::read_dir(home.join("pending/quarantine"))?.collect::<std::io::Result<Vec<_>>>()?;
    assert_eq!(quarantined.len(), 1);
    assert_eq!(std::fs::read(quarantined[0].path())?, before);
    assert_eq!(reservation_rows(&conn)?, reservations);
    assert!(proposal_terminal_event(&conn, &id)?.is_none());
    assert_eq!(
        std::fs::read_to_string(home.join("familiars/sage/reviewed/skill.md"))?,
        LEGACY_BEFORE
    );
    Ok(())
}

#[test]
fn legacy_submission_failure_preserves_existing_apply_evidence() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let home = temp.path();
    let (_, id, workspace) =
        stage_coherence_edit(home, "reviewed/skill.md", Some(LEGACY_BEFORE), LEGACY_AFTER)?;
    set_proposal_decision_failpoint(Some((
        ProposalDecisionFailpoint::ApplyBeforeAudit,
        id.clone(),
    )));
    assert!(decide_threads_proposal(home, &id, "approve", Some("{}")).is_err());
    let claim = find_pending_decision_claim(home, &id, "approve").context("applying claim")?;
    let before = std::fs::read(&claim)?;
    let conn = store::open_store(&store_path(home))?;
    conn.execute_batch(
        "CREATE TEMP TABLE original_legacy_submission AS
         SELECT * FROM ward_audit WHERE event_type='proposal_submitted';",
    )?;
    corrupt_legacy_receipt(
        &conn,
        "DELETE FROM ward_audit WHERE event_type='proposal_submitted' AND proposal_id=?1",
        &id,
    )?;
    let reservations = reservation_rows(&conn)?;
    assert!(!recover_proposal_claim(home, &claim)?);
    assert_eq!(std::fs::read(&claim)?, before);
    assert_eq!(reservation_rows(&conn)?, reservations);
    assert!(!home.join("pending/quarantine").exists());
    assert!(load_proposal_apply_intent(&conn, &id)?.is_some());
    assert!(proposal_terminal_event(&conn, &id)?.is_none());
    assert_eq!(
        std::fs::read_to_string(workspace.join("reviewed/skill.md"))?,
        LEGACY_AFTER
    );
    conn.execute_batch("INSERT INTO ward_audit SELECT * FROM original_legacy_submission")?;
    assert!(recover_proposal_claim(home, &claim)?);
    assert!(proposal_terminal_event(&conn, &id)?.is_some());
    Ok(())
}

#[test]
fn legacy_submission_failure_preserves_raw_intent_without_an_apply_sidecar() -> Result<()> {
    for entry in ["decision", "recovery", "scheduler"] {
        for malformed in [false, true] {
            let case = format!("{entry}/malformed={malformed}");
            let temp = tempfile::tempdir()?;
            let home = temp.path();
            let now = time::OffsetDateTime::now_utc();
            let (_, id, workspace) = crate::threads_clock::with_test_time(home, now, || {
                stage_coherence_edit(home, "reviewed/skill.md", Some(LEGACY_BEFORE), LEGACY_AFTER)
            })?;
            set_proposal_decision_failpoint(Some((
                ProposalDecisionFailpoint::ApplyBeforeAudit,
                id.clone(),
            )));
            assert!(
                crate::threads_clock::with_test_time(home, now, || {
                    decide_threads_proposal(home, &id, "approve", Some("{}"))
                })
                .is_err(),
                "{case}"
            );
            let claim = find_pending_decision_claim(home, &id, "approve")
                .context("interrupted legacy approval")?;
            let conn = store::open_store(&store_path(home))?;
            assert!(load_proposal_apply_intent(&conn, &id)?.is_some(), "{case}");
            let mut envelope: Value = serde_json::from_slice(&std::fs::read(&claim)?)?;
            assert!(
                envelope
                    .as_object_mut()
                    .context("legacy envelope")?
                    .remove("decisionState")
                    .is_some(),
                "{case}"
            );
            let before = serde_json::to_vec_pretty(&envelope)?;
            std::fs::write(&claim, &before)?;
            let document = read_pending_proposal_document(&claim)?;
            assert!(document.decision_state.is_none(), "{case}");
            assert!(document.decision_request.is_some(), "{case}");
            corrupt_legacy_receipt(
                &conn,
                "DELETE FROM ward_audit WHERE event_type='proposal_submitted' AND proposal_id=?1",
                &id,
            )?;
            if malformed {
                corrupt_legacy_receipt(
                    &conn,
                    "UPDATE ward_audit SET detail='{' WHERE decision='proposal-apply-intent' AND proposal_id=?1",
                    &id,
                )?;
                assert!(load_proposal_apply_intent(&conn, &id).is_err(), "{case}");
            }
            let intent_detail = || {
                conn.query_row(
                    "SELECT detail FROM ward_audit WHERE decision='proposal-apply-intent' AND proposal_id=?1",
                    [&id],
                    |row| row.get::<_, String>(0),
                )
            };
            let original_intent = intent_detail()?;
            let reservations = reservation_rows(&conn)?;
            crate::threads_clock::with_test_time(
                home,
                now + time::Duration::days(31),
                || -> Result<()> {
                    match entry {
                        "decision" => {
                            let response =
                                decide_threads_proposal(home, &id, "approve", Some("{}"))?;
                            assert_eq!(response.status, 409, "{case}: {}", response.body);
                            let body: Value = serde_json::from_str(&response.body)?;
                            assert_eq!(body["why"], "proposal-decision-review-required", "{case}");
                            assert_eq!(body["terminal"], false, "{case}");
                        }
                        "recovery" => assert!(!recover_proposal_claim(home, &claim)?, "{case}"),
                        "scheduler" => {
                            assert_eq!(process_due_threads_proposals(home)?, 0, "{case}")
                        }
                        _ => unreachable!(),
                    }
                    Ok(())
                },
            )?;
            assert_eq!(std::fs::read(&claim)?, before, "{case}");
            assert_eq!(reservation_rows(&conn)?, reservations, "{case}");
            assert_eq!(intent_detail()?, original_intent, "{case}");
            assert!(!home.join("pending/quarantine").exists(), "{case}");
            assert!(proposal_terminal_event(&conn, &id)?.is_none(), "{case}");
            assert_eq!(
                std::fs::read_to_string(workspace.join("reviewed/skill.md"))?,
                LEGACY_AFTER,
                "{case}"
            );
        }
    }
    Ok(())
}

#[test]
fn legacy_submission_cannot_relabel_a_scheduled_receipt() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let home = temp.path();
    let (path, id) = stage_scheduled_reviewed_edit(
        home,
        coven_threads_core::ApprovalPath::HumanApproval,
        time::OffsetDateTime::now_utc(),
    )?;
    let document = read_pending_proposal_document(&path)?;
    let review_kind = "coherence".to_string();
    let legacy = StoredLegacyProposalRef {
        pending: document.pending(),
        review_kind: Some(&review_kind),
        identity_evidence: document.identity_evidence,
        auto_regression_evidence: None,
        probes: document.probes.as_deref(),
        decision_request: None,
        decision_state: None,
    };
    let bytes = serde_json::to_vec_pretty(&legacy)?;
    std::fs::write(&path, &bytes)?;
    let response = decide_threads_proposal(home, &id, "approve", Some("{}"))?;
    assert_eq!(response.status, 409, "{}", response.body);
    let body: Value = serde_json::from_str(&response.body)?;
    assert_eq!(body["why"], "proposal-submission-receipt-invalid");
    assert_eq!(
        std::fs::read(body["quarantinePath"].as_str().context("quarantine")?)?,
        bytes
    );
    let conn = store::open_store(&store_path(home))?;
    assert!(proposal_terminal_event(&conn, &id)?.is_none());
    assert!(load_proposal_apply_intent(&conn, &id)?.is_none());
    Ok(())
}

#[test]
fn legacy_submission_database_errors_preserve_the_pending_proposal() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let home = temp.path();
    let (path, id, workspace) =
        stage_coherence_edit(home, "reviewed/skill.md", Some(LEGACY_BEFORE), LEGACY_AFTER)?;
    let before = std::fs::read(&path)?;
    let document = read_pending_proposal_document(&path)?;
    let unavailable = rusqlite::Connection::open_in_memory()?;
    let error = legacy_submission_preflight_response(home, &unavailable, &path, &document)
        .expect_err("an unavailable audit table must remain a database error");
    assert!(error.downcast_ref::<rusqlite::Error>().is_some());
    assert!(!invalid_scheduled_submission_authority(&error));
    assert!(!error.is::<HistoricalDecisionReviewRequired>());
    assert_eq!(std::fs::read(&path)?, before);
    assert!(!home.join("pending/quarantine").exists());
    assert_eq!(
        std::fs::read_to_string(workspace.join("reviewed/skill.md"))?,
        LEGACY_BEFORE
    );
    assert_eq!(
        decide_threads_proposal(home, &id, "approve", Some("{}"))?.status,
        200
    );
    Ok(())
}
