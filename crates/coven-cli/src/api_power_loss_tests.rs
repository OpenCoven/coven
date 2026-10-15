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
