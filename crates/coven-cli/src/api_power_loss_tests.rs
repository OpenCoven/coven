#[test]
fn protected_approval_survives_loss_of_unsynced_wal_suffix() -> Result<()> {
    use coven_threads_core::{ApprovalPath, Channel, VetoWindow};
    for lane in 0..6 {
        for boundary in [
            Some(ProposalDecisionFailpoint::ApplyIntentBeforeWrite),
            Some(ProposalDecisionFailpoint::ApplyBeforeAudit),
            Some(ProposalDecisionFailpoint::AuditBeforeCleanup),
            None,
        ] {
            let temp = tempfile::tempdir()?;
            let home = temp.path();
            let database = home.join("coven.sqlite3");
            let loss = crate::power_loss_tests::PowerLoss::observe(&database);
            // A live keeper prevents graceful request-connection close from
            // checkpointing NORMAL commits and hiding the power-loss defect.
            let keeper = store::open_store(&database)?;
            let now = time::OffsetDateTime::now_utc();
            let (_, id, target, after) = match lane {
                0 => {
                    let (path, id) = stage_pending_protected_edit(home)?;
                    (path, id, "SOUL.md", b"approved identity".as_slice())
                }
                1..=3 => {
                    let approval = match lane {
                        1 => ApprovalPath::HumanApproval,
                        2 => ApprovalPath::HumanApprovalWithRationale,
                        _ => ApprovalPath::FamiliarCoherence {
                            veto: VetoWindow::new(
                                std::time::Duration::from_secs(300),
                                std::time::Duration::from_secs(60),
                            ),
                        },
                    };
                    let (path, id) = stage_scheduled_reviewed_edit(
                        home,
                        approval,
                        now - time::Duration::minutes(10),
                    )?;
                    (path, id, "reviewed/skill.md", b"after".as_slice())
                }
                4 => {
                    let (path, id) = output_auto_cases::stage_supported_output_auto(home)?;
                    (
                        path,
                        id,
                        "output-format.json",
                        output_auto_cases::AFTER.as_bytes(),
                    )
                }
                _ => {
                    let (path, id) = stage_scheduled_created_edit(
                        home,
                        "reviewed/new-skill.md",
                        1,
                        ApprovalPath::HumanApproval,
                        now,
                        Channel::Mutation,
                    )?;
                    (path, id, "reviewed/new-skill.md", b"after".as_slice())
                }
            };
            let target = home.join("familiars/sage").join(target);
            let body = if lane == 0 {
                r#"{"note":"principal reviewed"}"#.to_string()
            } else {
                scheduled_decision_body(home, &id, Some("principal reviewed"))?
            };
            keeper.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")?;
            if let Some(boundary) = boundary {
                set_proposal_decision_failpoint(Some((boundary, id.clone())));
            }
            let decide = || {
                if lane == 4 {
                    decide_threads_proposal_automatic(home, &id, "approve", None)
                } else {
                    decide_threads_proposal(home, &id, "approve", Some(&body))
                }
            };
            let result = decide();
            if boundary.is_some() {
                assert!(result.is_err(), "lane {lane}, {boundary:?}: {result:?}");
            } else {
                let result = result?;
                assert_eq!(result.status, 200, "lane {lane}: {}", result.body);
            }
            // This normal write must be lost, proving the harness actually
            // discards a suffix rather than merely reopening the live WAL.
            keeper.execute(
                "INSERT INTO store_meta(key, value) VALUES ('power-loss-tail', 'lost')",
                [],
            )?;
            let images = loss.synced_images();
            let bytes = std::fs::read(&target).ok();
            drop(keeper);
            loss.restore(images)?;
            let conn = store::open_store(&database)?;
            assert_eq!(
                conn.query_row(
                    "SELECT COUNT(*) FROM store_meta WHERE key = 'power-loss-tail'",
                    [],
                    |row| row.get::<_, i64>(0)
                )?,
                0
            );
            assert!(
                load_proposal_apply_intent(&conn, &id)?.is_some(),
                "intent lost: lane {lane}, {boundary:?}"
            );
            assert_eq!(std::fs::read(&target).ok(), bytes);
            let finalized = matches!(
                boundary,
                None | Some(ProposalDecisionFailpoint::AuditBeforeCleanup)
            );
            assert_eq!(
                proposal_terminal_event(&conn, &id)?.is_some(),
                finalized,
                "terminal durability: lane {lane}, {boundary:?}"
            );
            let census = serde_json::to_value(crate::ward_audit_census::load(home, 100_000)?)?;
            let windows = census["windows"].as_array().context("census windows")?;
            if lane == 3 {
                assert_eq!(windows.len(), 1);
                assert_eq!(
                    windows[0]["classification"],
                    if finalized {
                        "typed_terminal_recorded"
                    } else {
                        "unprovable_apply"
                    }
                );
            } else {
                // This census deliberately inventories opened veto windows.
                // The intent/terminal assertions above cover no-window lanes.
                assert!(windows.is_empty(), "{census}");
            }
            drop(conn);
            if !finalized || boundary.is_some() {
                assert_eq!(
                    process_due_threads_proposals(home)?,
                    1,
                    "recovery: lane {lane}, {boundary:?}"
                );
            }
            assert_eq!(std::fs::read(&target)?, after);
            assert!(find_any_pending_decision_claim(home, &id).is_none());
            let conn = store::open_store(&database)?;
            assert_eq!(conn.query_row("SELECT COUNT(*) FROM ward_audit WHERE proposal_id = ?1 AND event_type = 'proposal_approved'", [&id], |row| row.get::<_, i64>(0))?, 1);
            if lane == 3 {
                let census = serde_json::to_value(crate::ward_audit_census::load(home, 100_000)?)?;
                assert_eq!(
                    census["windows"][0]["classification"],
                    "typed_terminal_recorded"
                );
            }
        }
    }
    Ok(())
}

#[test]
fn scheduled_submission_survives_loss_of_unsynced_wal_suffix() -> Result<()> {
    for before_receipt in [true, false] {
        let temp = tempfile::tempdir()?;
        let home = temp.path();
        let database = home.join("coven.sqlite3");
        let loss = crate::power_loss_tests::PowerLoss::observe(&database);
        let keeper = store::open_store(&database)?;
        seed_scheduled_identity_predicate_familiar(home)?;
        keeper.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")?;
        if before_receipt {
            crate::threads_gate::fail_next_scheduled_submission_after_publish(home);
        }
        let response = post_edits(
            home,
            r#"{"edits":[{"target":"TOOLS.md","contents":"Synthetic tools after\n"}],"principalKeyFingerprint":"fpr-val"}"#,
        );
        if before_receipt {
            assert!(response.is_err());
        } else {
            let response = response?;
            assert_eq!(response.status, 202, "{}", response.body);
        }
        let pending = std::fs::read_dir(home.join("pending"))?
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .find(|path| path.extension().is_some_and(|ext| ext == "json"))
            .context("published proposal")?;
        let id = read_pending_proposal_document(&pending)?
            .pending()
            .id
            .0
            .to_string();
        let images = loss.synced_images();
        drop(keeper);
        loss.restore(images)?;
        assert_eq!(process_due_threads_proposals(home)?, 0);
        assert!(
            pending.exists(),
            "durable submission binding must permit receipt recovery"
        );
        let conn = store::open_store(&database)?;
        let receipts: i64 = conn.query_row("SELECT COUNT(*) FROM ward_audit WHERE proposal_id = ?1 AND event_type = 'proposal_submitted'", [&id], |row| row.get(0))?;
        assert_eq!(receipts, 1, "submission receipt missing after power loss");
        drop(conn);
        let body = scheduled_decision_body(home, &id, None)?;
        let response = decide_threads_proposal(home, &id, "approve", Some(&body))?;
        assert_eq!(response.status, 200, "{}", response.body);
    }
    Ok(())
}

#[test]
fn failed_authority_sync_never_acknowledges_an_unrecorded_apply() -> Result<()> {
    for after_apply in [false, true] {
        let temp = tempfile::tempdir()?;
        let home = temp.path();
        let database = home.join("coven.sqlite3");
        let loss = crate::power_loss_tests::PowerLoss::observe(&database);
        let keeper = store::open_store(&database)?;
        let (_, id) = stage_pending_protected_edit(home)?;
        let target = home.join("familiars/sage/SOUL.md");
        let before = std::fs::read(&target)?;
        let decide = || decide_threads_proposal(home, &id, "approve", Some("{}"));
        if after_apply {
            set_proposal_decision_failpoint(Some((
                ProposalDecisionFailpoint::ApplyBeforeAudit,
                id.clone(),
            )));
            assert!(decide().is_err());
            assert_eq!(std::fs::read(&target)?, b"approved identity");
        }
        loss.fail_next_sync();
        let response = decide();
        assert!(response
            .as_ref()
            .map_or(true, |response| response.status != 200));
        assert!(proposal_terminal_event(&keeper, &id)?.is_none());
        if after_apply {
            assert!(find_any_pending_decision_claim(home, &id).is_some());
            assert!(load_proposal_apply_intent(&keeper, &id)?.is_some());
        } else {
            assert_eq!(std::fs::read(&target)?, before);
        }
        let response = decide()?;
        assert_eq!(response.status, 200, "{}", response.body);
        assert_eq!(std::fs::read(&target)?, b"approved identity");
        assert!(loss
            .sync_flags()
            .iter()
            .any(|flags| flags & 0xf == rusqlite::ffi::SQLITE_SYNC_FULL));
    }
    Ok(())
}

#[test]
fn finalization_keeps_baseline_and_terminal_atomic_and_durable() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let home = temp.path();
    let database = home.join("coven.sqlite3");
    let loss = crate::power_loss_tests::PowerLoss::observe(&database);
    let conn = store::open_store(&database)?;
    let (_, id) = stage_pending_protected_edit(home)?;
    let workspace = home.join("familiars/sage");
    let target = "SOUL.md".to_string();
    let before = crate::threads_gate::load_baseline(&conn, "sage", &target)?;
    let after = b"approved identity".to_vec();
    std::fs::write(workspace.join(&target), &after)?;
    let approved = BTreeMap::from([(target.clone(), after.clone())]);
    let targets = vec![target.clone()];
    let report = ward::ApplyReport { changes: vec![] };
    let approver = coven_threads_core::WriterId::new(OWNER_LOCAL_DECISION_ACTOR);
    conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")?;
    let finalize = || {
        finalize_approved_proposal(
            &conn,
            ApprovedProposalFinalization {
                proposal_id: &id,
                familiar_id: "sage",
                workspace: &workspace,
                weave_hash: &[7; 32],
                approver: Some(&approver),
                apply_report: &report,
                gated_targets: &targets,
                approved_bytes: &approved,
                files_touched: &targets,
                rationale: None,
                approval_path_label: "human_review",
                window_close: None,
                channel: coven_threads_core::Channel::Mutation,
                probe_summary: None,
                decided_at: time::OffsetDateTime::now_utc(),
            },
        )
    };
    // Abort the terminal insert after the baseline update inside finalization.
    conn.execute_batch("CREATE TEMP TRIGGER fail_terminal BEFORE INSERT ON ward_audit
        WHEN NEW.event_type = 'proposal_approved' BEGIN SELECT RAISE(ABORT, 'injected terminal failure'); END;")?;
    assert!(format!("{:#}", finalize().unwrap_err()).contains("injected terminal failure"));
    assert_eq!(
        crate::threads_gate::load_baseline(&conn, "sage", &target)?,
        before
    );
    assert!(proposal_terminal_event(&conn, &id)?.is_none());
    conn.execute_batch("DROP TRIGGER fail_terminal")?;
    finalize()?;
    let images = loss.synced_images();
    drop(conn);
    loss.restore(images)?;
    let conn = store::open_store(&database)?;
    assert!(proposal_terminal_event(&conn, &id)?.is_some());
    let expected = coven_threads_core::manifest_entry_hash(
        &coven_threads_core::SurfaceId::new(target.clone()),
        &after,
    );
    assert_eq!(
        crate::threads_gate::load_baseline(&conn, "sage", &target)?.as_deref(),
        Some(expected.as_slice())
    );
    Ok(())
}

#[test]
fn terminal_retry_does_not_acknowledge_a_failed_directory_barrier() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let home = temp.path();
    let (_, id) = stage_pending_protected_edit(home)?;
    let decide = || decide_threads_proposal(home, &id, "approve", Some("{}"));
    assert_eq!(decide()?.status, 200);
    crate::durable_fs::fail_next_directory_sync();
    assert!(decide().is_err());
    assert_eq!(decide()?.status, 200);
    Ok(())
}
#[test]
fn direct_write_survives_loss_of_unsynced_wal_suffix() -> Result<()> {
    for before_audit in [false, true] {
        let temp = tempfile::tempdir()?;
        let home = temp.path();
        let database = home.join("coven.sqlite3");
        let loss = crate::power_loss_tests::PowerLoss::observe(&database);
        let keeper = store::open_store(&database)?;
        let workspace = seed_warded_familiar(home)?;
        std::fs::create_dir_all(workspace.join("notes"))?;
        let target = workspace.join("notes/today.md");
        std::fs::write(&target, "before")?;
        keeper.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")?;
        let apply = || {
            post_edits(
                home,
                r#"{"edits":[{"target":"notes/today.md","contents":"after"}]}"#,
            )
        };
        let images = if before_audit {
            let pause = set_direct_apply_pause(home, "sage");
            std::thread::scope(|scope| -> Result<_> {
                let request = scope.spawn(apply);
                pause.wait_until_reached("direct write before audit");
                assert_eq!(std::fs::read(&target)?, b"after");
                // Freeze the pre-ack crash boundary, then let the thread close
                // safely. Later syncs must not enter the restored image.
                let images = loss.synced_images();
                pause.release();
                assert_eq!(request.join().unwrap()?.status, 200);
                Ok(images)
            })?
        } else {
            let response = apply()?;
            assert_eq!(response.status, 200, "{}", response.body);
            keeper.execute(
                "INSERT INTO store_meta(key, value) VALUES ('power-loss-tail', 'lost')",
                [],
            )?;
            loss.synced_images()
        };
        drop(keeper);
        loss.restore(images)?;
        assert_eq!(std::fs::read(&target)?, b"after");
        let conn = store::open_store(&database)?;
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM store_meta WHERE key = 'power-loss-tail'",
                [],
                |row| row.get::<_, i64>(0)
            )?,
            0
        );
        let audits: i64 = conn.query_row(
            r#"SELECT COUNT(*) FROM ward_audit WHERE event_type = 'apply_audit'
             AND familiar_id = 'sage' AND proposal_id IS NULL
             AND files_touched = '["notes/today.md"]'
             AND decision = 'applied' AND tier = 'tier_2'
             AND lower(hex(diff_hash)) = 'f39592393ef0859cb196a52693d2cea00fb2df784b3c04ae54aa7cadb8e562f8'
             AND json_extract(detail, '$.bytes_written') = 5
             AND json_extract(detail, '$.prev_sha256') = '6db7d803e74f1ffa7d8f5adc0bf95b3e15bf4c8373fffadf546227cc6c6742cb'"#,
            [],
            |row| row.get(0),
        )?;
        assert_eq!(
            audits,
            i64::from(!before_audit),
            "acknowledged direct write lost its audit evidence (before_audit={before_audit})"
        );
        assert_eq!(
            conn.pragma_query_value(None, "synchronous", |row| row.get::<_, i64>(0))?,
            1
        );
    }
    Ok(())
}

#[test]
fn direct_write_does_not_acknowledge_failed_audit_sync() -> Result<()> {
    for directory_failure in [false, true] {
        let temp = tempfile::tempdir()?;
        let home = temp.path();
        let database = home.join("coven.sqlite3");
        let loss = crate::power_loss_tests::PowerLoss::observe(&database);
        let keeper = store::open_store(&database)?;
        let workspace = seed_warded_familiar(home)?;
        std::fs::create_dir_all(workspace.join("notes"))?;
        let target = workspace.join("notes/today.md");
        std::fs::write(&target, "before")?;
        keeper.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")?;
        let pause = set_direct_apply_pause(home, "sage");
        let response = std::thread::scope(|scope| -> Result<_> {
            let request = scope.spawn(|| {
                post_edits(
                    home,
                    r#"{"edits":[{"target":"notes/today.md","contents":"after"}]}"#,
                )
            });
            pause.wait_until_reached("direct audit sync failure");
            if directory_failure {
                // Arm the thread-local fault on the request thread after its
                // target file and directory have already been synchronized.
                pause.fail_audit_directory_sync();
            } else {
                loss.fail_next_sync();
            }
            pause.release();
            request.join().unwrap()
        })?;
        assert_eq!(response.status, 500, "{}", response.body);
        let body: Value = serde_json::from_str(&response.body)?;
        assert_eq!(body["error"]["code"], "audit_persist_failed");
        assert_eq!(body["error"]["details"]["writeApplied"], true);
        assert_eq!(body["error"]["details"]["retrySafe"], false);
        assert_eq!(std::fs::read(&target)?, b"after");
        assert_eq!(
            keeper.query_row(
                "SELECT COUNT(*) FROM coven_ward_audit_reservations WHERE purpose = 'direct-ward-apply'",
                [], |row| row.get::<_, i64>(0))?,
            1,
            "failed audit barrier must retain the applied write's reservation"
        );
    }
    Ok(())
}

#[test]
fn legacy_submission_survives_loss_of_unsynced_wal_suffix() -> Result<()> {
    for boundary in ["acknowledged", "receipt_refused", "sync_failed"] {
        let temp = tempfile::tempdir()?;
        let home = temp.path();
        let database = home.join("coven.sqlite3");
        let loss = crate::power_loss_tests::PowerLoss::observe(&database);
        let keeper = store::open_store(&database)?;
        let workspace = seed_warded_familiar(home)?;
        std::fs::create_dir_all(workspace.join("reviewed"))?;
        let target = workspace.join("reviewed/skill.md");
        std::fs::write(&target, "before")?;
        if boundary == "receipt_refused" {
            keeper.execute_batch(
                "CREATE TRIGGER refuse_legacy_receipt BEFORE INSERT ON ward_audit
                WHEN NEW.event_type = 'proposal_submitted'
                BEGIN SELECT RAISE(ABORT, 'injected receipt failure'); END;",
            )?;
        }
        keeper.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")?;
        if boundary == "sync_failed" {
            // NORMAL may sync a newly created WAL's header. Prime it before
            // arming the fault so it reaches the receipt's FULL commit,
            // after the pending file has been published.
            keeper.execute(
                "INSERT INTO store_meta(key, value) VALUES ('prime-wal', 'normal')",
                [],
            )?;
            loss.fail_next_sync();
        }
        let result = post_edits(
            home,
            r#"{"edits":[{"target":"reviewed/skill.md","contents":"after"}]}"#,
        );
        if boundary == "acknowledged" {
            let response = result?;
            assert_eq!(response.status, 202, "{}", response.body);
        } else {
            let error = result.expect_err("receipt failure must not acknowledge staging");
            let expected = if boundary == "receipt_refused" {
                "injected receipt failure"
            } else {
                "committing durable authority evidence"
            };
            assert!(
                format!("{error:#}").contains(expected),
                "{boundary}: {error:#}"
            );
        }
        let pending = std::fs::read_dir(home.join("pending"))?
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .find(|path| path.extension().is_some_and(|ext| ext == "json"))
            .context("published legacy proposal")?;
        let bytes = std::fs::read(&pending)?;
        let document = read_pending_proposal_document(&pending)?;
        assert!(document.scheduled().is_none());
        let id = document.pending().id.to_string();
        keeper.execute(
            "INSERT INTO store_meta(key, value) VALUES ('power-loss-tail', 'lost')",
            [],
        )?;
        let images = loss.synced_images();
        drop(keeper);
        loss.restore(images)?;
        assert_eq!(std::fs::read(&pending)?, bytes);
        assert_eq!(std::fs::read(&target)?, b"before");
        let conn = store::open_store(&database)?;
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM store_meta WHERE key = 'power-loss-tail'",
                [],
                |row| row.get::<_, i64>(0)
            )?,
            0
        );
        let receipts: i64 = conn.query_row(
            "SELECT COUNT(*) FROM ward_audit WHERE proposal_id = ?1 AND event_type = 'proposal_submitted'",
            [&id], |row| row.get(0))?;
        assert_eq!(
            receipts,
            i64::from(boundary == "acknowledged"),
            "legacy submission receipt durability at {boundary}"
        );
        assert_eq!(
            conn.pragma_query_value(None, "synchronous", |row| row.get::<_, i64>(0))?,
            1
        );
    }
    Ok(())
}

#[test]
fn decision_reservation_survives_loss_of_unsynced_wal_suffix() -> Result<()> {
    for scheduled in [true, false] {
        let temp = tempfile::tempdir()?;
        let home = temp.path();
        let database = home.join("coven.sqlite3");
        let loss = crate::power_loss_tests::PowerLoss::observe(&database);
        // A live keeper prevents graceful request-connection close from
        // checkpointing NORMAL commits and hiding the power-loss defect.
        let keeper = store::open_store(&database)?;
        let (id, target, after) = if scheduled {
            let (_, id) = stage_scheduled_reviewed_edit(
                home,
                coven_threads_core::ApprovalPath::HumanApproval,
                time::OffsetDateTime::now_utc(),
            )?;
            (id, "reviewed/skill.md", b"after".as_slice())
        } else {
            let (_, id) = stage_pending_protected_edit(home)?;
            (id, "SOUL.md", b"approved identity".as_slice())
        };
        let target = home.join("familiars/sage").join(target);
        let before = std::fs::read(&target).ok();
        let body = if scheduled {
            scheduled_decision_body(home, &id, None)?
        } else {
            r#"{"note":"principal reviewed"}"#.to_string()
        };
        keeper.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")?;
        let reservations = || -> Result<i64> {
            Ok(store::open_store(&database)?.query_row(
                "SELECT COUNT(*) FROM coven_ward_audit_reservations",
                [],
                |row| row.get(0),
            )?)
        };
        let reserved_before = reservations()?;
        // Cut after the decision claim is published and before any apply
        // intent: the claim is durable, so its reservation must be too.
        set_proposal_decision_failpoint(Some((
            ProposalDecisionFailpoint::ClaimBeforeValidation,
            id.clone(),
        )));
        let interrupted = decide_threads_proposal(home, &id, "approve", Some(&body));
        assert!(interrupted.is_err(), "scheduled={scheduled}: {interrupted:?}");
        keeper.execute(
            "INSERT INTO store_meta(key, value) VALUES ('power-loss-tail', 'lost')",
            [],
        )?;
        let images = loss.synced_images();
        drop(keeper);
        loss.restore(images)?;
        let conn = store::open_store(&database)?;
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM store_meta WHERE key = 'power-loss-tail'",
                [],
                |row| row.get::<_, i64>(0)
            )?,
            0
        );
        assert!(load_proposal_apply_intent(&conn, &id)?.is_none());
        assert_eq!(
            conn.pragma_query_value(None, "synchronous", |row| row.get::<_, i64>(0))?,
            1
        );
        drop(conn);
        assert_eq!(std::fs::read(&target).ok(), before, "no effect before apply");
        assert_eq!(
            reservations()?,
            reserved_before + 1,
            "scheduled={scheduled}: published claim lost its decision reservation"
        );
        // The retry reuses the durable reservation instead of quarantining
        // the claim as having no pre-existing authority.
        let retry_body = if scheduled { None } else { Some(body.as_str()) };
        let retried = decide_threads_proposal(home, &id, "approve", retry_body)?;
        assert_eq!(
            retried.status, 200,
            "scheduled={scheduled}: {}",
            retried.body
        );
        assert!(
            !home.join("pending/quarantine").exists(),
            "scheduled={scheduled}: retry must not quarantine the claim"
        );
        assert_eq!(std::fs::read(&target)?, after);
    }
    Ok(())
}

#[test]
fn failed_decision_reservation_sync_publishes_no_claim() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let home = temp.path();
    let database = home.join("coven.sqlite3");
    let loss = crate::power_loss_tests::PowerLoss::observe(&database);
    let keeper = store::open_store(&database)?;
    let (pending, id) = stage_scheduled_reviewed_edit(
        home,
        coven_threads_core::ApprovalPath::HumanApproval,
        time::OffsetDateTime::now_utc(),
    )?;
    let target = home.join("familiars/sage/reviewed/skill.md");
    let before = std::fs::read(&target)?;
    let body = scheduled_decision_body(home, &id, None)?;
    keeper.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")?;
    // NORMAL may sync a newly created WAL's header. Prime it so the injected
    // failure reaches the reservation barrier's FULL commit.
    keeper.execute(
        "INSERT INTO store_meta(key, value) VALUES ('prime-wal', 'normal')",
        [],
    )?;
    loss.fail_next_sync();
    let error = decide_threads_proposal(home, &id, "approve", Some(&body))
        .expect_err("failed reservation sync must not proceed to the claim");
    assert!(
        format!("{error:#}").contains("committing durable authority evidence"),
        "{error:#}"
    );
    assert!(pending.exists(), "the pending proposal must stay unclaimed");
    assert!(find_any_pending_decision_claim(home, &id).is_none());
    assert!(!home.join("pending/quarantine").exists());
    assert_eq!(std::fs::read(&target)?, before);
    let conn = store::open_store(&database)?;
    assert!(load_proposal_apply_intent(&conn, &id)?.is_none());
    assert!(proposal_terminal_event(&conn, &id)?.is_none());
    drop(conn);
    let retried = decide_threads_proposal(home, &id, "approve", Some(&body))?;
    assert_eq!(retried.status, 200, "{}", retried.body);
    assert_eq!(std::fs::read(&target)?, b"after");
    Ok(())
}

/// A synthetic direct-write report: one audited Tier-2 change for
/// `notes/today.md` ("before" -> "after") plus applied protected changes at
/// each of `protected`, advanced in order. The direct API refuses Tier-0 edits, so the protected
/// branch is exercised here at the persistence boundary only (#1640).
fn direct_evidence_report(protected: &[&str]) -> ward::ApplyReport {
    let mut changes = vec![
            ward::AppliedChange {
                decision: ward::Decision {
                    target: "notes/today.md".to_owned(),
                    resolved: "notes/today.md".to_owned(),
                    tier: ward::Tier::Logged,
                    verdict: ward::Verdict::AllowWithLog,
                },
                disposition: ward::Disposition::Applied,
                audit: Some(ward::AuditRecord {
                    target: "notes/today.md".to_owned(),
                    resolved: "notes/today.md".to_owned(),
                    tier: ward::Tier::Logged,
                    prev_sha256: Some(
                        "6db7d803e74f1ffa7d8f5adc0bf95b3e15bf4c8373fffadf546227cc6c6742cb"
                            .to_owned(),
                    ),
                    next_sha256: "f39592393ef0859cb196a52693d2cea00fb2df784b3c04ae54aa7cadb8e562f8"
                        .to_owned(),
                    bytes_written: 5,
                }),
            },
    ];
    changes.extend(protected.iter().map(|surface| ward::AppliedChange {
        decision: ward::Decision {
            target: (*surface).to_owned(),
            resolved: (*surface).to_owned(),
            tier: ward::Tier::Protected,
            verdict: ward::Verdict::AuthorizedProtectedChange,
        },
        disposition: ward::Disposition::Applied,
        audit: None,
    }));
    ward::ApplyReport { changes }
}

fn exact_direct_audit_rows(conn: &rusqlite::Connection) -> Result<i64> {
    Ok(conn.query_row(
        r#"SELECT COUNT(*) FROM ward_audit WHERE event_type = 'apply_audit'
         AND familiar_id = 'sage' AND proposal_id IS NULL
         AND files_touched = '["notes/today.md"]'
         AND decision = 'applied' AND tier = 'tier_2'
         AND lower(hex(diff_hash)) = 'f39592393ef0859cb196a52693d2cea00fb2df784b3c04ae54aa7cadb8e562f8'
         AND json_extract(detail, '$.bytes_written') = 5
         AND json_extract(detail, '$.prev_sha256') = '6db7d803e74f1ffa7d8f5adc0bf95b3e15bf4c8373fffadf546227cc6c6742cb'"#,
        [],
        |row| row.get(0),
    )?)
}

fn apply_audit_rows(conn: &rusqlite::Connection) -> Result<i64> {
    Ok(conn.query_row(
        "SELECT COUNT(*) FROM ward_audit WHERE event_type = 'apply_audit'",
        [],
        |row| row.get(0),
    )?)
}

fn soul_baseline(conn: &rusqlite::Connection) -> Result<Option<Vec<u8>>> {
    Ok(rusqlite::OptionalExtension::optional(conn.query_row(
        "SELECT entry_hash FROM ward_manifest WHERE familiar_id = 'sage' AND surface = 'SOUL.md'",
        [],
        |row| row.get(0),
    ))?)
}

fn expected_soul_baseline(bytes: &[u8]) -> Vec<u8> {
    coven_threads_core::manifest_entry_hash(
        &coven_threads_core::SurfaceId::new("SOUL.md".to_owned()),
        bytes,
    )
    .to_vec()
}

#[test]
fn direct_evidence_baseline_failure_rolls_back_its_audit_rows() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let home = temp.path();
    let workspace = seed_warded_familiar(home)?;
    let config = ward::WardConfig::load(&workspace)?.context("seeded ward.toml")?;
    let conn = store::open_store(&home.join("coven.sqlite3"))?;
    // `SOUL.md` advances its baseline first; the path-escaping surface then
    // fails, after both the audit rows and that manifest row were written in
    // the same transaction.
    let failure = persist_direct_apply_evidence(
        &conn,
        "sage",
        &workspace,
        &config,
        &direct_evidence_report(&["SOUL.md", "../SOUL.md"]),
        time::OffsetDateTime::now_utc(),
    )
    .expect_err("a failed baseline advance must fail the evidence write");
    let DirectEvidenceFailure::Baseline(error) = failure else {
        panic!("expected a typed baseline failure, got {failure:?}");
    };
    assert!(format!("{error:#}").contains("path-escaping"), "{error:#}");
    assert_eq!(
        apply_audit_rows(&conn)?,
        0,
        "a baseline failure after audit insertion must roll the audit rows back"
    );
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM ward_manifest WHERE familiar_id = 'sage'",
            [],
            |row| row.get::<_, i64>(0)
        )?,
        0,
        "the earlier SOUL.md baseline advance must roll back with the audit rows"
    );
    assert!(conn.is_autocommit());
    assert_eq!(
        conn.pragma_query_value(None, "synchronous", |row| row.get::<_, i64>(0))?,
        1
    );
    Ok(())
}

#[test]
fn direct_evidence_commits_audit_and_protected_baseline_atomically_and_durably() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let home = temp.path();
    let database = home.join("coven.sqlite3");
    let loss = crate::power_loss_tests::PowerLoss::observe(&database);
    let keeper = store::open_store(&database)?;
    let workspace = seed_warded_familiar(home)?;
    let config = ward::WardConfig::load(&workspace)?.context("seeded ward.toml")?;
    std::fs::write(workspace.join("SOUL.md"), "# Sage, revised\n")?;
    keeper.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")?;
    let conn = store::open_store(&database)?;
    persist_direct_apply_evidence(
        &conn,
        "sage",
        &workspace,
        &config,
        &direct_evidence_report(&["SOUL.md"]),
        time::OffsetDateTime::now_utc(),
    )
    .map_err(|failure| anyhow::anyhow!("{failure:?}"))?;
    assert_eq!(
        conn.pragma_query_value(None, "synchronous", |row| row.get::<_, i64>(0))?,
        1,
        "ordinary writes regain NORMAL after the durable evidence commit"
    );
    keeper.execute(
        "INSERT INTO store_meta(key, value) VALUES ('power-loss-tail', 'lost')",
        [],
    )?;
    let images = loss.synced_images();
    drop(conn);
    drop(keeper);
    loss.restore(images)?;
    let conn = store::open_store(&database)?;
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM store_meta WHERE key = 'power-loss-tail'",
            [],
            |row| row.get::<_, i64>(0)
        )?,
        0,
        "the deliberately unsynced NORMAL tail must disappear"
    );
    assert_eq!(exact_direct_audit_rows(&conn)?, 1);
    assert_eq!(
        soul_baseline(&conn)?,
        Some(expected_soul_baseline(b"# Sage, revised\n")),
        "the protected baseline advance must survive with its audit rows"
    );
    Ok(())
}

#[test]
fn direct_evidence_sync_failures_are_typed_and_never_assume_rollback() -> Result<()> {
    for directory_failure in [false, true] {
        let temp = tempfile::tempdir()?;
        let home = temp.path();
        let database = home.join("coven.sqlite3");
        let loss = crate::power_loss_tests::PowerLoss::observe(&database);
        let keeper = store::open_store(&database)?;
        let workspace = seed_warded_familiar(home)?;
        let config = ward::WardConfig::load(&workspace)?.context("seeded ward.toml")?;
        keeper.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")?;
        // NORMAL may sync a fresh WAL header; prime it before arming a fault.
        keeper.execute(
            "INSERT INTO store_meta(key, value) VALUES ('prime-wal', 'normal')",
            [],
        )?;
        let conn = store::open_store(&database)?;
        if directory_failure {
            crate::durable_fs::fail_next_directory_sync();
        } else {
            loss.fail_next_sync();
        }
        let failure = persist_direct_apply_evidence(
            &conn,
            "sage",
            &workspace,
            &config,
            &direct_evidence_report(&["SOUL.md"]),
            time::OffsetDateTime::now_utc(),
        )
        .expect_err("a failed sync or barrier must not report success");
        assert!(
            matches!(failure, DirectEvidenceFailure::Durability(_)),
            "directory_failure={directory_failure}: {failure:?}"
        );
        assert!(conn.is_autocommit());
        assert_eq!(
            conn.pragma_query_value(None, "synchronous", |row| row.get::<_, i64>(0))?,
            1
        );
        if directory_failure {
            // The barrier runs after COMMIT and xSync, so the evidence is
            // committed and synced; the error must not be read as rollback.
            assert_eq!(exact_direct_audit_rows(&conn)?, 1);
            assert!(soul_baseline(&conn)?.is_some());
        }
        let images = loss.synced_images();
        drop(conn);
        drop(keeper);
        loss.restore(images)?;
        let conn = store::open_store(&database)?;
        let audits = exact_direct_audit_rows(&conn)?;
        let baseline = soul_baseline(&conn)?.is_some();
        assert_eq!(
            (audits, baseline),
            if directory_failure { (1, true) } else { (0, false) },
            "directory_failure={directory_failure}: audit and baseline survive or vanish together"
        );
    }
    Ok(())
}
