pub(super) const BEFORE: &str =
    r#"{"schema":"coven.output-format/v1","indent":2,"final_newline":true}"#;
pub(super) const AFTER: &str =
    r#"{"schema":"coven.output-format/v1","indent":4,"final_newline":false}"#;

fn seed_supported_output_auto(home: &Path) -> Result<PathBuf> {
    let workspace = seed_warded_familiar(home)?;
    let config = std::fs::read_to_string(workspace.join("ward.toml"))?;
    std::fs::write(
        workspace.join("ward.toml"),
        format!(
            "{config}\n[[surface]]\npath='output-format.json'\ntier=2\n\
            [editable]\nharness_blocks=['output_format']\n\
            [approval_tiers.auto]\nblocks=['output_format']\ngate='regression_suite'\n\
            [[probe]]\nsurface='output-format.json'\nid='parse'\nformat='json'\n"
        ),
    )?;
    std::fs::write(workspace.join("output-format.json"), BEFORE)?;
    Ok(workspace)
}

pub(super) fn stage_supported_output_auto(home: &Path) -> Result<(PathBuf, String)> {
    seed_supported_output_auto(home)?;
    let response = post_edits(
        home,
        &json!({"edits":[{"target":"output-format.json","contents":AFTER}]}).to_string(),
    )?;
    anyhow::ensure!(
        response.status == 202,
        "supported auto staging: {}",
        response.body
    );
    let response: Value = serde_json::from_str(&response.body)?;
    Ok((
        PathBuf::from(response["pendingPath"].as_str().context("pending path")?),
        response["proposalId"].as_str().context("id")?.to_string(),
    ))
}

thread_local! {
    static ORDINARY_ADMISSION_HOOK: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        const { std::cell::RefCell::new(None) };
}

pub(crate) fn run_ordinary_admission_hook() {
    let hook = ORDINARY_ADMISSION_HOOK.with(|hook| hook.borrow_mut().take());
    if let Some(hook) = hook {
        hook();
    }
}

#[cfg(unix)]
#[test]
fn ordinary_admission_retarget_before_apply_refuses_whole_batch() -> Result<()> {
    use std::os::unix::fs::symlink;
    use std::sync::mpsc;
    use std::time::Duration;

    let temp = tempfile::tempdir()?;
    let home = temp.path();
    let workspace = seed_supported_output_auto(home)?;
    std::fs::write(workspace.join("notes.json"), BEFORE)?;
    std::fs::write(workspace.join("other.json"), "ordinary before")?;
    symlink("notes.json", workspace.join("alias.json"))?;
    let (admitted_tx, admitted_rx) = mpsc::channel();
    let (retargeted_tx, retargeted_rx) = mpsc::channel();
    // A generous hang guard bounds a broken fixture; channel ordering proves the race.
    let guard = Duration::from_secs(60);
    ORDINARY_ADMISSION_HOOK.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || {
            admitted_tx.send(()).expect("admission observer stopped");
            retargeted_rx
                .recv_timeout(guard)
                .expect("retarget worker did not release admission");
        }));
    });
    let response = std::thread::scope(|scope| {
        let changed_workspace = &workspace;
        let worker = scope.spawn(move || {
            admitted_rx
                .recv_timeout(guard)
                .context("ordinary admission hook was not reached")?;
            let result = (|| -> Result<()> {
                std::fs::remove_file(changed_workspace.join("alias.json"))?;
                symlink("output-format.json", changed_workspace.join("alias.json"))?;
                Ok(())
            })();
            retargeted_tx.send(()).context("admission request stopped")?;
            result
        });
        let response = post_edits(
            home,
            &json!({"edits":[
                {"target":"other.json","contents":"ordinary after"},
                {"target":"alias.json","contents":"not an output-format document"}
            ]})
            .to_string(),
        );
        ORDINARY_ADMISSION_HOOK.with(|hook| hook.borrow_mut().take());
        worker.join().expect("retarget worker panicked")?;
        response
    })?;
    assert_eq!(
        std::fs::read_to_string(workspace.join("output-format.json"))?,
        BEFORE,
        "ordinary admission must not directly write the retargeted configured surface: {}",
        response.body
    );
    assert_eq!(
        std::fs::read_to_string(workspace.join("other.json"))?,
        "ordinary before",
        "changed admission must refuse the entire batch"
    );
    assert_eq!(std::fs::read_to_string(workspace.join("notes.json"))?, BEFORE);
    assert!(response.status >= 400, "{}", response.body);
    let conn = store::open_store(&home.join("coven.sqlite3"))?;
    let writes: i64 = conn.query_row(
        "SELECT COUNT(*) FROM ward_audit WHERE event_type IN ('edit_applied', 'proposal_submitted')",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(writes, 0);
    Ok(())
}

#[test]
fn ordinary_admission_changes_through_commit_refuse_whole_batch() -> Result<()> {
    #[cfg(unix)]
    use std::os::unix::fs::symlink;

    for phase in ["admission", "first-commit", "second-commit"] {
        for change in [
            #[cfg(unix)]
            "alias",
            "same-bytes-inode",
            "target-hardlink",
            "parent",
            #[cfg(unix)]
            "parent-symlink",
            #[cfg(unix)]
            "canonical-symlink",
            "canonical-hardlink",
        ] {
            let temp = tempfile::tempdir()?;
            let home = temp.path();
            let workspace = seed_supported_output_auto(home)?;
            std::fs::create_dir(workspace.join("ordinary"))?;
            std::fs::write(workspace.join("ordinary/notes.json"), BEFORE)?;
            std::fs::write(workspace.join("other.json"), "ordinary before")?;
            #[cfg(unix)]
            let target = {
                symlink("ordinary/notes.json", workspace.join("alias.json"))?;
                "alias.json"
            };
            #[cfg(not(unix))]
            let target = "ordinary/notes.json";
            let changed_workspace = workspace.clone();
            let mutate = move || {
                let workspace = changed_workspace;
                match change {
                    #[cfg(unix)]
                    "alias" => {
                        std::fs::remove_file(workspace.join("alias.json")).unwrap();
                        symlink("output-format.json", workspace.join("alias.json")).unwrap();
                    }
                    "same-bytes-inode" => {
                        std::fs::rename(
                            workspace.join("ordinary/notes.json"),
                            workspace.join("original.json"),
                        )
                        .unwrap();
                        std::fs::write(workspace.join("ordinary/notes.json"), BEFORE).unwrap();
                    }
                    "target-hardlink" => {
                        std::fs::remove_file(workspace.join("ordinary/notes.json")).unwrap();
                        std::fs::hard_link(
                            workspace.join("output-format.json"),
                            workspace.join("ordinary/notes.json"),
                        )
                        .unwrap();
                    }
                    "parent" => {
                        std::fs::rename(workspace.join("ordinary"), workspace.join("moved"))
                            .unwrap();
                        std::fs::create_dir(workspace.join("ordinary")).unwrap();
                        // Keep the leaf inode unchanged: the ancestor identity must matter.
                        std::fs::hard_link(
                            workspace.join("moved/notes.json"),
                            workspace.join("ordinary/notes.json"),
                        )
                        .unwrap();
                    }
                    #[cfg(unix)]
                    "parent-symlink" => {
                        std::fs::rename(workspace.join("ordinary"), workspace.join("moved"))
                            .unwrap();
                        symlink("moved", workspace.join("ordinary")).unwrap();
                    }
                    #[cfg(unix)]
                    "canonical-symlink" => {
                        std::fs::remove_file(workspace.join("output-format.json")).unwrap();
                        symlink("ordinary/notes.json", workspace.join("output-format.json"))
                            .unwrap();
                    }
                    "canonical-hardlink" => {
                        std::fs::remove_file(workspace.join("output-format.json")).unwrap();
                        std::fs::hard_link(
                            workspace.join("ordinary/notes.json"),
                            workspace.join("output-format.json"),
                        )
                        .unwrap();
                    }
                    _ => unreachable!(),
                }
            };
            match phase {
                "admission" => ORDINARY_ADMISSION_HOOK.with(|hook| {
                    *hook.borrow_mut() = Some(Box::new(mutate));
                }),
                "first-commit" => ward::set_direct_commit_hook("other.json", mutate),
                "second-commit" => ward::set_direct_commit_hook(target, mutate),
                _ => unreachable!(),
            }
            let response = post_edits(
                home,
                &json!({"edits":[
                    {"target":"other.json","contents":"ordinary after"},
                    {"target":target,"contents":AFTER}
                ]})
                .to_string(),
            )?;
            assert_eq!(
                std::fs::read_to_string(workspace.join("output-format.json"))?,
                BEFORE,
                "{phase}/{change}: {}",
                response.body
            );
            assert_eq!(
                std::fs::read_to_string(workspace.join("other.json"))?,
                "ordinary before",
                "{phase}/{change}: entire batch must be refused or rolled back"
            );
            assert_eq!(
                std::fs::read_to_string(workspace.join("ordinary/notes.json"))?,
                BEFORE,
                "{phase}/{change}: admitted destination must not be changed"
            );
            if workspace.join("moved").exists() {
                assert_eq!(
                    std::fs::read_to_string(workspace.join("moved/notes.json"))?,
                    BEFORE,
                    "{phase}/{change}: detached parent must not retain a write"
                );
            }
            assert!(response.status >= 400, "{phase}/{change}: {}", response.body);
        }
    }
    Ok(())
}

#[cfg(unix)]
#[test]
fn ordinary_admission_restored_alias_cannot_redirect_the_consumed_outcome() -> Result<()> {
    use std::os::unix::fs::symlink;

    let temp = tempfile::tempdir()?;
    let home = temp.path();
    let workspace = seed_supported_output_auto(home)?;
    std::fs::remove_file(workspace.join("output-format.json"))?;
    std::fs::create_dir(workspace.join("ordinary"))?;
    symlink("ordinary", workspace.join("redirect"))?;

    let changed_workspace = workspace.clone();
    ORDINARY_ADMISSION_HOOK.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || {
            std::fs::remove_file(changed_workspace.join("redirect")).unwrap();
            symlink(".", changed_workspace.join("redirect")).unwrap();
        }));
    });
    let restored_workspace = workspace.clone();
    ward::set_direct_evaluation_hook(move || {
        std::fs::remove_file(restored_workspace.join("redirect")).unwrap();
        symlink("ordinary", restored_workspace.join("redirect")).unwrap();
    });
    let response = post_edits(
        home,
        &json!({"edits":[{
            "target":"redirect/output-format.json",
            "contents":"ordinary content, not an output-format document"
        }]}).to_string(),
    )?;

    assert!(
        !workspace.join("output-format.json").exists(),
        "a different evaluated outcome escaped the restored admission: {}",
        response.body
    );
    assert_eq!(response.status, 200, "{}", response.body);
    assert_eq!(
        std::fs::read_to_string(workspace.join("ordinary/output-format.json"))?,
        "ordinary content, not an output-format document"
    );
    let body: Value = serde_json::from_str(&response.body)?;
    assert_eq!(body["changes"][0]["resolved"], "ordinary/output-format.json");
    Ok(())
}

#[test]
fn ordinary_admission_stable_aliases_and_creates_still_apply() -> Result<()> {
    #[cfg(unix)]
    use std::os::unix::fs::symlink;

    for alias_kind in [
        "literal",
        #[cfg(unix)]
        "symlink",
        #[cfg(unix)]
        "parent-symlink",
        "hardlink",
        "create",
    ] {
        let temp = tempfile::tempdir()?;
        let home = temp.path();
        let workspace = seed_supported_output_auto(home)?;
        std::fs::create_dir(workspace.join("ordinary"))?;
        std::fs::write(workspace.join("ordinary/notes.json"), BEFORE)?;
        let target = match alias_kind {
            "literal" => "ordinary/./notes.json",
            #[cfg(unix)]
            "symlink" => {
                symlink("ordinary/notes.json", workspace.join("alias.json"))?;
                "alias.json"
            }
            #[cfg(unix)]
            "parent-symlink" => {
                symlink("ordinary", workspace.join("alias"))?;
                "alias/notes.json"
            }
            "hardlink" => {
                std::fs::hard_link(
                    workspace.join("ordinary/notes.json"),
                    workspace.join("alias.json"),
                )?;
                "alias.json"
            }
            "create" => "new/parent/notes.json",
            _ => unreachable!(),
        };
        let response = post_edits(
            home,
            &json!({"edits":[{"target":target,"contents":"ordinary after"}]}).to_string(),
        )?;
        assert_eq!(response.status, 200, "{alias_kind}: {}", response.body);
        assert_eq!(std::fs::read_to_string(workspace.join(target))?, "ordinary after");
        assert_eq!(std::fs::read_to_string(workspace.join("output-format.json"))?, BEFORE);
    }
    Ok(())
}

#[cfg(unix)]
#[test]
fn ordinary_admission_rollback_uses_admitted_destination_not_retargeted_alias() -> Result<()> {
    use std::os::unix::fs::symlink;

    let temp = tempfile::tempdir()?;
    let home = temp.path();
    let workspace = seed_supported_output_auto(home)?;
    std::fs::write(workspace.join("notes.json"), BEFORE)?;
    std::fs::write(workspace.join("other.json"), "ordinary before")?;
    symlink("notes.json", workspace.join("alias.json"))?;
    let changed_workspace = workspace.clone();
    ward::set_direct_commit_hook("other.json", move || {
        assert_eq!(
            std::fs::read_to_string(changed_workspace.join("notes.json")).unwrap(),
            AFTER,
            "the first edit must have committed before the routing refusal"
        );
        std::fs::remove_file(changed_workspace.join("alias.json")).unwrap();
        symlink("output-format.json", changed_workspace.join("alias.json")).unwrap();
    });
    let response = post_edits(
        home,
        &json!({"edits":[
            {"target":"alias.json","contents":AFTER},
            {"target":"other.json","contents":"ordinary after"}
        ]})
        .to_string(),
    )?;
    assert!(response.status >= 400, "{}", response.body);
    for target in ["notes.json", "output-format.json"] {
        assert_eq!(std::fs::read_to_string(workspace.join(target))?, BEFORE);
    }
    assert_eq!(std::fs::read_to_string(workspace.join("other.json"))?, "ordinary before");
    assert_eq!(std::fs::read_link(workspace.join("alias.json"))?, Path::new("output-format.json"));
    let body: Value = serde_json::from_str(&response.body)?;
    assert_eq!(body["error"]["details"]["writeApplied"], false);
    Ok(())
}

#[test]
fn output_auto_commitment_roundtrip_revision_and_submission_anchor() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let home = temp.path();
    let (path, _) = stage_supported_output_auto(home)?;
    let mut document = parse_proposal_envelope(&std::fs::read(&path)?)?;
    let conn = store::open_store(&home.join("coven.sqlite3"))?;
    let workspace = home.join("familiars/sage");
    let config = ward::WardConfig::load(&workspace)?.context("config")?;
    assert!(
        document.identity_evidence.is_none(),
        "absence must also be committed"
    );
    assert!(revalidate_auto_regression(&conn, &config, &document).is_ok());
    let original = document.revision()?;
    let restored = parse_proposal_envelope(&document.serialize_with_decision(None, None)?)?;
    assert_eq!(
        document.auto_regression_evidence,
        restored.auto_regression_evidence
    );
    assert_eq!(original, restored.revision()?);
    let mut changed = config.clone();
    changed.probe.push(ward::ProbeConfig {
        surface: "*.json".into(),
        id: ward::ProbeId::SizeDelta,
        format: None,
        forbidden: Vec::new(),
        required: Vec::new(),
    });
    let scheduled = document.scheduled().context("scheduled")?;
    assert_eq!(
        crate::ward_probes::run_materialized(&changed, scheduled.materialized_diff())?,
        crate::ward_probes::run_at_staging(
            &workspace,
            &changed,
            &staged_edits_to_ward_edits(document.pending())?,
            &ward::Authorization::unsigned(),
        )?,
        "image-based and filesystem probes must agree on the actual before/after bytes"
    );
    let (new_hash, reports) = crate::output_format_auto::regression_evidence(
        &changed,
        scheduled.materialized_diff(),
        &scheduled.classification().evidence_replay_hash,
        document.identity_evidence,
    )?;
    document.auto_regression_evidence = Some(new_hash);
    document.probes = Some(serde_json::value::RawValue::from_string(
        serde_json::to_string(&reports)?,
    )?);
    assert_ne!(original, document.revision()?);
    assert_eq!(
        revalidate_auto_regression(&conn, &changed, &document)
            .unwrap_err()
            .1,
        coven_threads_core::WindowCloseReason::EvidenceDiverged,
        "a recomputed valid sidecar cannot replace committed submission authority"
    );
    Ok(())
}

#[test]
fn output_auto_unknown_or_missing_evidence_never_becomes_legacy_success() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let (path, _) = stage_supported_output_auto(temp.path())?;
    let raw = std::fs::read(&path)?;
    let original = parse_proposal_envelope(&raw)?;
    let mut value: Value = serde_json::from_slice(&raw)?;
    value["autoRegressionEvidence"] = Value::Null;
    assert!(parse_proposal_envelope(&serde_json::to_vec(&value)?).is_err());
    value["autoRegressionEvidence"] = json!({"version":2,"digest":[]});
    assert!(parse_proposal_envelope(&serde_json::to_vec(&value)?).is_err());
    value
        .as_object_mut()
        .context("envelope")?
        .remove("autoRegressionEvidence");
    let missing = parse_proposal_envelope(&serde_json::to_vec(&value)?)?;
    assert_ne!(original.revision()?, missing.revision()?);
    let config = ward::WardConfig::load(&temp.path().join("familiars/sage"))?.context("config")?;
    let conn = store::open_store(&temp.path().join("coven.sqlite3"))?;
    assert_eq!(
        revalidate_auto_regression(&conn, &config, &missing)
            .unwrap_err()
            .1,
        coven_threads_core::WindowCloseReason::RevalidationFailed
    );
    Ok(())
}

#[test]
fn output_auto_uncommitted_historical_memory_envelope_is_refused() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let (path, _) = stage_scheduled_edit(
        temp.path(),
        "MEMORY.md",
        2,
        coven_threads_core::ApprovalPath::AutoRegression { veto: None },
        time::OffsetDateTime::now_utc(),
        coven_threads_core::Channel::Mutation,
    )?;
    let document = parse_proposal_envelope(&std::fs::read(path)?)?;
    let config = ward::WardConfig::load(&temp.path().join("familiars/sage"))?.context("config")?;
    let conn = store::open_store(&temp.path().join("coven.sqlite3"))?;
    assert_eq!(
        revalidate_auto_regression(&conn, &config, &document)
            .unwrap_err()
            .1,
        coven_threads_core::WindowCloseReason::RevalidationFailed
    );
    assert_eq!(
        std::fs::read_to_string(temp.path().join("familiars/sage/MEMORY.md"))?,
        "before"
    );
    Ok(())
}

#[test]
fn output_auto_legacy_migration_does_not_grant_logged_authority() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let workspace = seed_warded_familiar(temp.path())?;
    let legacy = "[meta]\nversion='0.1.0'\nowner='sage'\n\
        [protected]\nfiles=['SOUL.md']\n\
        [editable]\npaths=['output-format.json']\nharness_blocks=['output_format']\n\
        [approval_tiers.auto]\nblocks=['output_format']\ngate='regression_suite'\n";
    std::fs::write(workspace.join("ward.toml"), legacy)?;
    std::fs::write(workspace.join("output-format.json"), BEFORE)?;
    let report = crate::ward_migrate::run_migration(
        temp.path(),
        crate::ward_migrate::WardMigrateOptions {
            familiar: Some("sage".into()),
            fingerprint: "fpr-val".into(),
            apply: true,
        },
    )?;
    assert!(!report.has_errors(), "{report:?}");
    let config = ward::WardConfig::load(&workspace)?.context("migrated config")?;
    assert_eq!(
        config.classify_resolved_path("output-format.json")?,
        ward::Tier::Reviewed
    );
    assert!(matches!(
        config
            .compiled_approval_tiers()?
            .context("bindings")?
            .approval_path_for(&coven_threads_core::SurfaceRegionId::new("output_format")),
        Some(coven_threads_core::ApprovalPath::AutoRegression { veto: None })
    ));
    let response = post_edits(
        temp.path(),
        &json!({"edits":[{"target":"output-format.json","contents":AFTER}]}).to_string(),
    )?;
    assert_eq!(response.status, 409, "{}", response.body);
    assert_eq!(
        std::fs::read_to_string(workspace.join("output-format.json"))?,
        BEFORE
    );
    assert_eq!(
        std::fs::read_to_string(workspace.join("ward.toml.v01.bak"))?,
        legacy
    );
    Ok(())
}
