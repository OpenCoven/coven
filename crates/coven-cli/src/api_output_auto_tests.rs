pub(super) const BEFORE: &str =
    r#"{"schema":"coven.output-format/v1","indent":2,"final_newline":true}"#;
pub(super) const AFTER: &str =
    r#"{"schema":"coven.output-format/v1","indent":4,"final_newline":false}"#;

fn seed_supported_output_auto(home: &Path) -> Result<PathBuf> {
    let workspace = seed_warded_familiar(home)?;
    enable_supported_output_auto(&workspace)?;
    Ok(workspace)
}

fn enable_supported_output_auto(workspace: &Path) -> Result<()> {
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
    Ok(())
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

fn install_ordinary_mutation(
    phase: &str,
    target: &str,
    mutate: impl FnOnce() -> std::io::Result<()> + 'static,
) -> std::rc::Rc<std::cell::Cell<Option<std::io::Result<()>>>> {
    let result = std::rc::Rc::new(std::cell::Cell::new(None));
    let observed = result.clone();
    // Fixture I/O is asserted after the API releases its global authority lock.
    let callback = move || observed.set(Some(mutate()));
    match phase {
        "admission" => ORDINARY_ADMISSION_HOOK.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(callback));
        }),
        "first-commit" => ward::set_direct_commit_hook("other.json", callback),
        "second-commit" => ward::set_direct_commit_hook(target, callback),
        _ => panic!("unknown ordinary mutation phase"),
    }
    result
}

#[cfg(unix)]
#[test]
fn ordinary_admission_broken_output_chain_keeps_structured_refusal() -> Result<()> {
    use std::os::unix::fs::symlink;

    for human in [false, true] {
        for contents in ["malformed formatting must not apply", AFTER] {
            let temp = tempfile::tempdir()?;
            let home = temp.path();
            let workspace = seed_supported_output_auto(home)?;
            if human {
                let path = workspace.join("ward.toml");
                let policy = std::fs::read_to_string(&path)?
                    .replace("[approval_tiers.auto]", "[approval_tiers.human_review]")
                    .replace("gate='regression_suite'", "gate='human_approval'");
                std::fs::write(path, policy)?;
            }
            std::fs::remove_file(workspace.join("output-format.json"))?;
            symlink("notes.json", workspace.join("output-format.json"))?;
            symlink("output-format.json", workspace.join("format-alias.json"))?;
            std::fs::write(workspace.join("unrelated.txt"), "original unrelated")?;

            let response = post_edits(
                home,
                &json!({"edits":[
                    {"target":"format-alias.json","contents":contents},
                    {"target":"unrelated.txt","contents":"must not change"}
                ]})
                .to_string(),
            );
            assert!(
                response.is_ok(),
                "unsupported output routing must return its structured refusal: {response:?}"
            );
            let response = response?;
            assert_eq!(response.status, 409, "{}", response.body);
            let body: Value = serde_json::from_str(&response.body)?;
            assert_eq!(body["error"]["code"], "scheduled_publication_invalid");
            assert_eq!(
                std::fs::read_to_string(workspace.join("unrelated.txt"))?,
                "original unrelated"
            );
            assert!(!workspace.join("notes.json").exists());
            assert!(!home.join("pending").exists());
            let conn = store::open_store(&home.join("coven.sqlite3"))?;
            let rows: i64 = conn.query_row(
                "SELECT COUNT(*) FROM ward_audit WHERE event_type IN
                 ('proposal_submitted','apply_audit','proposal_approved')",
                [],
                |row| row.get(0),
            )?;
            assert_eq!(rows, 0);
        }
    }
    Ok(())
}

#[cfg(unix)]
#[test]
fn ordinary_admission_failure_without_output_opt_in_is_not_publication_failure() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let workspace = seed_warded_familiar(temp.path())?;
    std::os::unix::fs::symlink("missing.json", workspace.join("alias.json"))?;
    let response = post_edits(
        temp.path(),
        &json!({"edits":[{"target":"alias.json","contents":"must not apply"}]}).to_string(),
    );
    assert!(
        response.is_err(),
        "ordinary admission failure was reclassified"
    );
    assert!(!workspace.join("missing.json").exists());
    assert!(!temp.path().join("pending").exists());
    Ok(())
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
            retargeted_tx
                .send(())
                .context("admission request stopped")?;
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
    assert_eq!(
        std::fs::read_to_string(workspace.join("notes.json"))?,
        BEFORE
    );
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
            #[cfg(unix)]
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
            let mutation = install_ordinary_mutation(phase, target, move || {
                let workspace = changed_workspace;
                match change {
                    #[cfg(unix)]
                    "alias" => {
                        std::fs::remove_file(workspace.join("alias.json"))?;
                        symlink("output-format.json", workspace.join("alias.json"))?;
                    }
                    "same-bytes-inode" => {
                        std::fs::rename(
                            workspace.join("ordinary/notes.json"),
                            workspace.join("original.json"),
                        )?;
                        std::fs::write(workspace.join("ordinary/notes.json"), BEFORE)?;
                    }
                    "target-hardlink" => {
                        std::fs::remove_file(workspace.join("ordinary/notes.json"))?;
                        std::fs::hard_link(
                            workspace.join("output-format.json"),
                            workspace.join("ordinary/notes.json"),
                        )?;
                    }
                    #[cfg(unix)]
                    "parent" => {
                        std::fs::rename(workspace.join("ordinary"), workspace.join("moved"))?;
                        std::fs::create_dir(workspace.join("ordinary"))?;
                        // Keep the leaf inode unchanged: the ancestor identity must matter.
                        std::fs::hard_link(
                            workspace.join("moved/notes.json"),
                            workspace.join("ordinary/notes.json"),
                        )?;
                    }
                    #[cfg(unix)]
                    "parent-symlink" => {
                        std::fs::rename(workspace.join("ordinary"), workspace.join("moved"))?;
                        symlink("moved", workspace.join("ordinary"))?;
                    }
                    #[cfg(unix)]
                    "canonical-symlink" => {
                        std::fs::remove_file(workspace.join("output-format.json"))?;
                        symlink("ordinary/notes.json", workspace.join("output-format.json"))?;
                    }
                    "canonical-hardlink" => {
                        std::fs::remove_file(workspace.join("output-format.json"))?;
                        std::fs::hard_link(
                            workspace.join("ordinary/notes.json"),
                            workspace.join("output-format.json"),
                        )?;
                    }
                    _ => unreachable!(),
                }
                Ok(())
            });
            let response = post_edits(
                home,
                &json!({"edits":[
                    {"target":"other.json","contents":"ordinary after"},
                    {"target":target,"contents":AFTER}
                ]})
                .to_string(),
            );
            mutation
                .take()
                .with_context(|| format!("{phase}/{change}: mutation was not attempted"))?
                .with_context(|| format!("{phase}/{change}: fixture mutation failed"))?;
            let response = response?;
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
            assert!(
                response.status >= 400,
                "{phase}/{change}: {}",
                response.body
            );
        }
    }
    Ok(())
}

#[cfg(windows)]
#[test]
fn ordinary_admission_retained_windows_parent_blocks_mutation_until_released() -> Result<()> {
    for phase in ["admission", "first-commit", "second-commit"] {
        let temp = tempfile::tempdir()?;
        let home = temp.path();
        let workspace = seed_supported_output_auto(home)?;
        std::fs::create_dir(workspace.join("ordinary"))?;
        std::fs::write(workspace.join("ordinary/notes.json"), BEFORE)?;
        std::fs::write(workspace.join("other.json"), "ordinary before")?;
        let changed_workspace = workspace.clone();
        let mutation = install_ordinary_mutation(phase, "ordinary/notes.json", move || {
            std::fs::rename(
                changed_workspace.join("ordinary"),
                changed_workspace.join("moved"),
            )
        });
        let response = post_edits(
            home,
            &json!({"edits":[
                {"target":"other.json","contents":"ordinary after"},
                {"target":"ordinary/notes.json","contents":AFTER}
            ]})
            .to_string(),
        );
        let error = mutation
            .take()
            .context("parent mutation was not attempted")?
            .expect_err("retained Windows directory must deny rename");
        assert_eq!(error.raw_os_error(), Some(32), "{phase}: {error}");
        assert!(!workspace.join("moved").exists());
        let response = response?;
        // A blocked adversarial mutation did not change authority: this is a
        // legitimate ordinary commit, not detection of a successful retarget.
        assert_eq!(response.status, 200, "{phase}: {}", response.body);
        assert_eq!(
            std::fs::read_to_string(workspace.join("output-format.json"))?,
            BEFORE
        );
        assert_eq!(
            std::fs::read_to_string(workspace.join("ordinary/notes.json"))?,
            AFTER
        );
        assert_eq!(
            std::fs::read_to_string(workspace.join("other.json"))?,
            "ordinary after"
        );
        assert!(!home.join("pending").exists());
        std::fs::rename(workspace.join("ordinary"), workspace.join("moved"))
            .with_context(|| format!("{phase}: API retained a parent handle after return"))?;
        assert_eq!(
            std::fs::read_to_string(workspace.join("moved/notes.json"))?,
            AFTER
        );
    }
    Ok(())
}

#[test]
fn ordinary_admission_fixture_io_failure_does_not_poison_authority_lock() -> Result<()> {
    for phase in ["admission", "first-commit", "second-commit"] {
        let temp = tempfile::tempdir()?;
        let workspace = seed_supported_output_auto(temp.path())?;
        std::fs::write(workspace.join("other.json"), "ordinary before")?;
        std::fs::write(workspace.join("notes.json"), BEFORE)?;
        let changed_workspace = workspace.clone();
        let mutation = install_ordinary_mutation(phase, "notes.json", move || {
            std::fs::rename(
                changed_workspace.join("absent-parent"),
                changed_workspace.join("moved"),
            )
        });
        let body = json!({"edits":[
            {"target":"other.json","contents":"ordinary after"},
            {"target":"notes.json","contents":AFTER}
        ]})
        .to_string();
        let response = post_edits(temp.path(), &body);
        let error = mutation
            .take()
            .context("mutation was not attempted")?
            .expect_err("absent fixture parent cannot be renamed");
        assert_eq!(
            error.kind(),
            std::io::ErrorKind::NotFound,
            "{phase}: {error}"
        );
        assert_eq!(response?.status, 200);
        assert_eq!(post_edits(temp.path(), &body)?.status, 200);
        assert_eq!(
            std::fs::read_to_string(workspace.join("output-format.json"))?,
            BEFORE
        );
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
        }]})
        .to_string(),
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
    assert_eq!(
        body["changes"][0]["resolved"],
        "ordinary/output-format.json"
    );
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
        assert_eq!(
            std::fs::read_to_string(workspace.join(target))?,
            "ordinary after"
        );
        assert_eq!(
            std::fs::read_to_string(workspace.join("output-format.json"))?,
            BEFORE
        );
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
    assert_eq!(
        std::fs::read_to_string(workspace.join("other.json"))?,
        "ordinary before"
    );
    assert_eq!(
        std::fs::read_link(workspace.join("alias.json"))?,
        Path::new("output-format.json")
    );
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

fn stage_dual_evidence(
    home: &Path,
    interrupted: bool,
) -> Result<(PathBuf, ProposalEnvelopeDocument)> {
    stage_dual_evidence_with_window(home, interrupted, false)
}

fn stage_dual_evidence_with_window(
    home: &Path,
    interrupted: bool,
    veto: bool,
) -> Result<(PathBuf, ProposalEnvelopeDocument)> {
    let workspace = seed_identity_predicate_familiar(home)?;
    enable_supported_output_auto(&workspace)?;
    if veto {
        let path = workspace.join("ward.toml");
        let policy = std::fs::read_to_string(&path)?.replace(
            "gate='regression_suite'",
            "gate='regression_suite'\nhuman_veto_window_hours=1\nmin_visible_seconds=60",
        );
        std::fs::write(path, policy)?;
    }
    if interrupted {
        crate::threads_gate::fail_next_scheduled_submission_after_publish(home);
    }
    let response = post_edits(
        home,
        &json!({"edits":[{"target":"output-format.json","contents":AFTER}]}).to_string(),
    );
    let path = if interrupted {
        let error = response.expect_err("interrupt after publishing the exact body");
        assert!(
            error
                .to_string()
                .contains("after scheduled proposal publication"),
            "{error:#}"
        );
        let entries = std::fs::read_dir(home.join("pending"))?
            .collect::<std::io::Result<Vec<_>>>()?
            .into_iter()
            .filter(|entry| {
                entry
                    .path()
                    .extension()
                    .is_some_and(|extension| extension == "json")
            })
            .collect::<Vec<_>>();
        assert_eq!(entries.len(), 1);
        entries[0].path()
    } else {
        let response = response?;
        assert_eq!(response.status, 202, "{}", response.body);
        let value: Value = serde_json::from_str(&response.body)?;
        PathBuf::from(value["pendingPath"].as_str().context("pending path")?)
    };
    let document = parse_proposal_envelope(&std::fs::read(&path)?)?;
    assert!(document.identity_evidence.is_some());
    assert!(document.auto_regression_evidence.is_some());
    Ok((path, document))
}

fn assert_dual_receipt(home: &Path, document: &ProposalEnvelopeDocument) -> Result<()> {
    let conn = store::open_store(&store_path(home))?;
    let id = document.pending().id.to_string();
    let (count, detail): (i64, String) = conn.query_row(
        "SELECT COUNT(*), detail FROM ward_audit WHERE proposal_id=?1 AND event_type='proposal_submitted'",
        [&id], |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    assert_eq!(count, 1);
    let tier: i64 = conn.query_row(
        "SELECT CAST(tier AS INTEGER) FROM ward_audit WHERE proposal_id=?1 AND event_type='proposal_submitted'",
        [&id], |row| row.get(0),
    )?;
    assert_eq!(
        tier, 2,
        "canonical receipt must retain AUTO's admitted Tier 2"
    );
    let detail: Value = serde_json::from_str(&detail)?;
    assert_eq!(
        detail["identity_evidence"],
        json!(document.identity_evidence)
    );
    assert_eq!(
        detail["autoRegressionEvidence"],
        json!(document.auto_regression_evidence)
    );
    scheduled_submission_authority_for_document(home, &conn, document)?;
    Ok(())
}

#[test]
fn output_auto_dual_evidence_normal_submission_and_replay() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let home = temp.path();
    let (_, document) = stage_dual_evidence(home, false)?;
    assert_dual_receipt(home, &document)?;
    assert_eq!(process_due_threads_proposals(home)?, 1);
    assert_eq!(process_due_threads_proposals(home)?, 0);
    assert_eq!(
        std::fs::read_to_string(home.join("familiars/sage/output-format.json"))?,
        AFTER
    );
    assert_dual_receipt(home, &document)?;
    Ok(())
}

#[test]
fn output_auto_dual_evidence_missing_receipt_recovers_at_both_entries() -> Result<()> {
    for decision_entry in [false, true] {
        let temp = tempfile::tempdir()?;
        let home = temp.path();
        let (_, document) = stage_dual_evidence(home, true)?;
        if decision_entry {
            let response = decide_threads_proposal(
                home,
                &document.pending().id.to_string(),
                "approve",
                Some(&json!({"expectedRevision":document.revision()?}).to_string()),
            )?;
            assert_eq!(response.status, 200, "{}", response.body);
        } else {
            assert_eq!(process_due_threads_proposals(home)?, 1);
        }
        assert_dual_receipt(home, &document)?;
        assert_eq!(
            std::fs::read_to_string(home.join("familiars/sage/output-format.json"))?,
            AFTER
        );
        assert_eq!(process_due_threads_proposals(home)?, 0);
        let conn = store::open_store(&store_path(home))?;
        let (terminals, reservations): (i64, i64) = conn.query_row(
            "SELECT (SELECT COUNT(*) FROM ward_audit WHERE proposal_id=?1 AND event_type='proposal_approved'),
                    (SELECT COUNT(*) FROM coven_ward_audit_reservations)",
            [document.pending().id.to_string()], |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        assert_eq!((terminals, reservations), (1, 0));
    }
    Ok(())
}

#[test]
fn output_auto_dual_evidence_recovery_uses_original_not_fresh_authority() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let home = temp.path();
    let (path, document) = stage_dual_evidence(home, true)?;
    let original = std::fs::read(&path)?;
    let config_path = home.join("familiars/sage/ward.toml");
    let config = std::fs::read_to_string(&config_path)?;
    std::fs::write(
        &config_path,
        format!("{config}\n[[probe]]\nsurface='*.json'\nid='size-delta'\n"),
    )?;
    for _ in 0..2 {
        let deferred = reconcile_scheduled_submission_reservations(home)?;
        assert!(deferred.paths.is_empty() && deferred.proposal_ids.is_empty());
    }
    assert_eq!(std::fs::read(&path)?, original);
    assert_dual_receipt(home, &document)?;
    let conn = store::open_store(&store_path(home))?;
    let changed = ward::WardConfig::load(&home.join("familiars/sage"))?.context("config")?;
    assert!(revalidate_auto_regression(&conn, &changed, &document).is_err());
    drop(conn);
    assert_eq!(process_due_threads_proposals(home)?, 1);
    assert_eq!(process_due_threads_proposals(home)?, 0);
    assert_eq!(
        std::fs::read_to_string(home.join("familiars/sage/output-format.json"))?,
        BEFORE
    );
    Ok(())
}

#[test]
fn output_auto_dual_evidence_changed_body_cannot_reconstruct_authority() -> Result<()> {
    for change in [
        "identityEvidence",
        "autoRegressionEvidence",
        "probes",
        "bytes",
    ] {
        let temp = tempfile::tempdir()?;
        let home = temp.path();
        let (path, document) = stage_dual_evidence(home, true)?;
        let original = std::fs::read(&path)?;
        let changed = if change == "bytes" {
            [original.as_slice(), b"\n"].concat()
        } else {
            let mut value: Value = serde_json::from_slice(&original)?;
            value[change] = if change == "probes" {
                json!([])
            } else {
                json!(vec![0_u8; 32])
            };
            serde_json::to_vec(&value)?
        };
        std::fs::write(&path, changed)?;
        assert_eq!(process_due_threads_proposals(home)?, 0);
        assert_eq!(process_due_threads_proposals(home)?, 0);
        let conn = store::open_store(&store_path(home))?;
        let rows: i64 = conn.query_row(
            "SELECT COUNT(*) FROM ward_audit WHERE proposal_id=?1",
            [document.pending().id.to_string()],
            |row| row.get(0),
        )?;
        assert_eq!(rows, 0, "changed {change} acquired audit authority");
        assert!(!path.exists());
        assert_eq!(
            std::fs::read_to_string(home.join("familiars/sage/output-format.json"))?,
            BEFORE
        );
    }
    Ok(())
}

#[test]
fn output_auto_submission_quota_refusal_releases_bound_reservation() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let home = temp.path();
    seed_supported_output_auto(home)?;
    write_pending_quota_fillers(
        home,
        b"{}",
        crate::proposal_store::MAX_PENDING_PROPOSALS,
        "quota",
    )?;
    let response = post_edits(
        home,
        &json!({"edits":[{"target":"output-format.json","contents":AFTER}]}).to_string(),
    )?;
    assert_eq!(response.status, 413, "{}", response.body);
    let conn = store::open_store(&store_path(home))?;
    let reservations: i64 = conn.query_row(
        "SELECT COUNT(*) FROM coven_ward_audit_reservations",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(reservations, 0);
    Ok(())
}

#[test]
fn output_auto_changed_sidecar_preserves_original_non_authorizing_refusal() -> Result<()> {
    for veto in [false, true] {
        for missing in [false, true] {
            let temp = tempfile::tempdir()?;
            let home = temp.path();
            let now = time::OffsetDateTime::now_utc();
            let (path, document) = crate::threads_clock::with_test_time(home, now, || {
                stage_dual_evidence_with_window(home, false, veto)
            })?;
            if veto {
                assert_eq!(
                    crate::threads_clock::with_test_time(home, now, || {
                        process_due_threads_proposals(home)
                    })?,
                    0
                );
            }
            let mut changed: Value = serde_json::from_slice(&std::fs::read(&path)?)?;
            if missing {
                changed
                    .as_object_mut()
                    .context("envelope")?
                    .remove("autoRegressionEvidence");
            } else {
                changed["autoRegressionEvidence"] = json!(vec![0_u8; 32]);
            }
            std::fs::write(&path, serde_json::to_vec(&changed)?)?;
            assert_eq!(
                crate::threads_clock::with_test_time(home, now + time::Duration::hours(1), || {
                    process_due_threads_proposals(home)
                })?,
                1,
                "veto={veto}, missing={missing}"
            );
            assert_eq!(process_due_threads_proposals(home)?, 0);
            assert_eq!(
                std::fs::read_to_string(home.join("familiars/sage/output-format.json"))?,
                BEFORE
            );
            let conn = store::open_store(&store_path(home))?;
            let (count, event, decision, detail): (i64, String, String, Option<String>) = conn.query_row(
                "SELECT COUNT(*), event_type, decision, detail FROM ward_audit WHERE proposal_id=?1 AND event_type IN ('proposal_rejected','proposal_approved','proposal_vetoed')",
                [document.pending().id.to_string()], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )?;
            assert_eq!(count, 1);
            assert_eq!(event, "proposal_rejected");
            if veto {
                assert_eq!(
                    decision,
                    if missing {
                        "revalidation_failed"
                    } else {
                        "evidence_diverged"
                    }
                );
                let detail: Value = serde_json::from_str(&detail.context("window close")?)?;
                assert_eq!(detail["replay_hash_matched"], false);
            } else {
                assert!(detail.is_none());
            }
        }
    }
    Ok(())
}

#[test]
fn output_auto_receipt_absence_and_null_do_not_weaken_non_auto_identity() -> Result<()> {
    use super::threads_opened_window_repair::{append_audit_copy, audit_source};
    let temp = tempfile::tempdir()?;
    let home = temp.path();
    let (path, id, _) = stage_pending_scheduled_identity_predicate_edit(home)?;
    let document = parse_proposal_envelope(&std::fs::read(path)?)?;
    assert!(document.identity_evidence.is_some());
    assert!(document.auto_regression_evidence.is_none());
    let conn = store::open_store(&store_path(home))?;
    let original = audit_source(&conn, &id, "proposal_submitted")?;
    for change in ["absent", "null", "injected", "legacy-identity", "extra"] {
        let copy = tempfile::tempdir()?;
        let conn = store::open_store(&store_path(copy.path()))?;
        let mut receipt = original.clone();
        match change {
            "absent" => {}
            "null" => receipt["detail"]["autoRegressionEvidence"] = Value::Null,
            "injected" => receipt["detail"]["autoRegressionEvidence"] = json!(vec![0_u8; 32]),
            "legacy-identity" => {
                receipt["detail"]
                    .as_object_mut()
                    .context("detail")?
                    .remove("identity_evidence");
                receipt["detail"]["autoRegressionEvidence"] = Value::Null;
            }
            "extra" => receipt["detail"]["unbound"] = json!(true),
            _ => unreachable!(),
        }
        append_audit_copy(&conn, &id, "proposal_submitted", &receipt)?;
        let result = scheduled_submission_authority_for_document(home, &conn, &document);
        match change {
            "absent" | "null" => {
                result?;
            }
            "legacy-identity" => assert!(result
                .expect_err("identity must remain bound")
                .downcast_ref::<HistoricalDecisionReviewRequired>()
                .is_some()),
            _ => assert!(invalid_scheduled_submission_authority(
                &result.expect_err("unbound receipt")
            )),
        }
    }
    Ok(())
}

#[test]
fn output_auto_rejection_proof_refuses_corrupt_or_unbound_original_evidence() -> Result<()> {
    use super::threads_opened_window_repair::{append_audit_copy, audit_source};
    let temp = tempfile::tempdir()?;
    let home = temp.path();
    let now = time::OffsetDateTime::now_utc();
    let (_, mut document) = crate::threads_clock::with_test_time(home, now, || {
        stage_dual_evidence_with_window(home, false, true)
    })?;
    assert_eq!(
        crate::threads_clock::with_test_time(home, now, || process_due_threads_proposals(home))?,
        0
    );
    let id = document.pending().id.to_string();
    let conn = store::open_store(&store_path(home))?;
    let original_receipt = audit_source(&conn, &id, "proposal_submitted")?;
    let original_opening = audit_source(&conn, &id, "proposal_window_opened")?;
    document.auto_regression_evidence = Some([0; 32]);
    for change in [
        "valid",
        "receipt-missing",
        "receipt-duplicate",
        "auto-missing",
        "auto-null",
        "auto-hash",
        "identity",
        "targets",
        "thread",
        "opening-missing",
        "opening-duplicate",
        "opening-hash",
        "opening-deadline",
    ] {
        let copy = tempfile::tempdir()?;
        let conn = store::open_store(&store_path(copy.path()))?;
        let mut receipt = original_receipt.clone();
        let mut opening = original_opening.clone();
        match change {
            "auto-missing" => {
                receipt["detail"]
                    .as_object_mut()
                    .context("detail")?
                    .remove("autoRegressionEvidence");
            }
            "auto-null" => receipt["detail"]["autoRegressionEvidence"] = Value::Null,
            "auto-hash" => receipt["detail"]["autoRegressionEvidence"] = json!(vec![1_u8; 32]),
            "identity" => receipt["detail"]["identity_evidence"] = json!(vec![0_u8; 32]),
            "targets" => receipt["targets"] = json!(["TOOLS.md"]),
            "thread" => receipt["thread_id"] = json!(Uuid::new_v4()),
            "opening-hash" => opening["hash"] = json!(vec![0_u8; 32]),
            "opening-deadline" => {
                let format = time::format_description::well_known::Rfc3339;
                let deadline = time::OffsetDateTime::parse(
                    opening["detail"]["deadline"].as_str().context("deadline")?,
                    &format,
                )?;
                opening["detail"]["deadline"] =
                    json!((deadline + time::Duration::seconds(1)).format(&format)?);
            }
            _ => {}
        }
        if change != "receipt-missing" {
            append_audit_copy(&conn, &id, "proposal_submitted", &receipt)?;
        }
        if change == "receipt-duplicate" {
            append_audit_copy(&conn, &id, "proposal_submitted", &receipt)?;
        }
        if change != "opening-missing" {
            append_audit_copy(&conn, &id, "proposal_window_opened", &opening)?;
        }
        if change == "opening-duplicate" {
            append_audit_copy(&conn, &id, "proposal_window_opened", &opening)?;
        }
        let result = validate_proposal_submission_document(home, &conn, &document);
        if change == "valid" {
            assert!(matches!(
                result?,
                ScheduledSubmissionValidation::Reject(
                    ScheduledSubmissionRejection::AutoRegression(_)
                )
            ));
        } else {
            let error = match result {
                Err(error) => error,
                Ok(_) => anyhow::bail!("{change}: corrupt evidence acquired a refusal proof"),
            };
            assert!(
                invalid_scheduled_submission_authority(&error),
                "{change}: {error:#}"
            );
        }
    }
    Ok(())
}

#[test]
fn output_auto_rejection_proof_excludes_applying_and_orphaned_intent() -> Result<()> {
    for orphan in [false, true] {
        let temp = tempfile::tempdir()?;
        let home = temp.path();
        let (_, document) = stage_dual_evidence(home, false)?;
        let id = document.pending().id.to_string();
        set_proposal_decision_failpoint(Some((
            ProposalDecisionFailpoint::ApplyBeforeAudit,
            id.clone(),
        )));
        assert!(decide_threads_proposal(
            home,
            &id,
            "approve",
            Some(&json!({"expectedRevision":document.revision()?}).to_string()),
        )
        .is_err());
        let (claim, _) = find_any_pending_decision_claim(home, &id).context("applying claim")?;
        let mut value: Value = serde_json::from_slice(&std::fs::read(&claim)?)?;
        value["autoRegressionEvidence"] = json!(vec![0_u8; 32]);
        if orphan {
            assert!(value
                .as_object_mut()
                .context("envelope")?
                .remove("decisionState")
                .is_some());
        }
        let bytes = serde_json::to_vec(&value)?;
        std::fs::write(&claim, &bytes)?;
        assert_eq!(process_due_threads_proposals(home)?, 0);
        assert_eq!(process_due_threads_proposals(home)?, 0);
        assert!(!claim.exists());
        let quarantined = std::fs::read_dir(home.join("pending/quarantine"))?
            .collect::<std::io::Result<Vec<_>>>()?;
        assert_eq!(quarantined.len(), 1);
        assert_eq!(std::fs::read(quarantined[0].path())?, bytes);
        assert_eq!(
            std::fs::read_to_string(home.join("familiars/sage/output-format.json"))?,
            AFTER
        );
        let conn = store::open_store(&store_path(home))?;
        assert!(load_proposal_apply_intent(&conn, &id)?.is_some());
        assert!(proposal_terminal_event(&conn, &id)?.is_none());
    }
    Ok(())
}

#[test]
fn output_auto_retained_config_refusal_preserves_final_authority_cause_without_retry() -> Result<()>
{
    for callback_refuses in [true, false] {
        let temp = tempfile::tempdir()?;
        let workspace = seed_supported_output_auto(temp.path())?;
        let config = ward::WardConfig::load(&workspace)?.context("config")?;
        let ward = ward::Ward::new(&workspace, config)?;
        let path = workspace.join("ward.toml");
        let original = std::fs::read_to_string(&path)?;
        std::fs::write(
            path,
            format!("{original}\n[[probe]]\nsurface='*.json'\nid='size-delta'\n"),
        )?;
        let calls = std::cell::Cell::new(0);
        let mut check = || {
            calls.set(calls.get() + 1);
            if callback_refuses {
                Err(final_authority_drift("ward-config-changed", None))
            } else {
                Ok(())
            }
        };
        let error = ward
            .apply_after_scheduled_approval_with_commit_check(
                &[ward::FileEdit::new("output-format.json", AFTER.as_bytes())],
                &ward::Authorization::unsigned(),
                &BTreeMap::from([(
                    "output-format.json".to_string(),
                    Some(BEFORE.as_bytes().to_vec()),
                )]),
                &BTreeMap::from([(
                    "output-format.json".to_string(),
                    "output-format.json".to_string(),
                )]),
                ward::ApprovedApplyMode::Initial,
                Some(&mut check),
            )
            .expect_err("retained config failure must never become a successful retry");
        assert_eq!(calls.get(), 1);
        assert!(matches!(
            ward::approved_apply_failure(&error),
            Some(ward::ApprovedApplyFailure::NoWrite)
        ));
        assert_eq!(
            is_final_authority_drift(&error),
            callback_refuses,
            "{error:#}"
        );
        assert!(format!("{error:#}").contains("Ward config changed before approved apply"));
        assert_eq!(
            std::fs::read_to_string(workspace.join("output-format.json"))?,
            BEFORE
        );
    }
    Ok(())
}

#[test]
fn output_auto_rejection_keeps_evidence_until_durable_terminal_and_retries_once() -> Result<()> {
    for veto in [false, true] {
        let temp = tempfile::tempdir()?;
        let home = temp.path();
        let now = time::OffsetDateTime::now_utc();
        let (path, document) = crate::threads_clock::with_test_time(home, now, || {
            stage_dual_evidence_with_window(home, false, veto)
        })?;
        if veto {
            assert_eq!(
                crate::threads_clock::with_test_time(home, now, || process_due_threads_proposals(
                    home
                ))?,
                0
            );
        }
        let id = document.pending().id.to_string();
        let mut value: Value = serde_json::from_slice(&std::fs::read(&path)?)?;
        value["autoRegressionEvidence"] = json!(vec![0_u8; 32]);
        let original = serde_json::to_vec(&value)?;
        std::fs::write(&path, &original)?;
        let body = scheduled_decision_body(home, &id, None)?;
        let conn = store::open_store(&store_path(home))?;
        let limit: i64 = conn.query_row(
            "SELECT limit_bytes FROM coven_ward_audit_capacity WHERE singleton=1",
            [],
            |row| row.get(0),
        )?;
        conn.execute(
            "UPDATE coven_ward_audit_capacity SET limit_bytes=used_bytes WHERE singleton=1",
            [],
        )?;
        assert_eq!(
            decide_threads_proposal(home, &id, "approve", Some(&body))?.status,
            507
        );
        assert_eq!(std::fs::read(&path)?, original);
        assert!(proposal_terminal_event(&conn, &id)?.is_none());
        conn.execute(
            "UPDATE coven_ward_audit_capacity SET limit_bytes=?1 WHERE singleton=1",
            [limit],
        )?;
        set_proposal_decision_failpoint(Some((
            ProposalDecisionFailpoint::AuditBeforeCleanup,
            id.clone(),
        )));
        assert!(decide_threads_proposal(home, &id, "approve", Some(&body)).is_err());
        assert!(proposal_terminal_event(&conn, &id)?.is_some());
        assert_eq!(process_due_threads_proposals(home)?, 0);
        assert_eq!(process_due_threads_proposals(home)?, 0);
        assert!(!path.exists());
        assert!(find_any_pending_decision_claim(home, &id).is_none());
        let (terminals, reservations): (i64, i64) = conn.query_row(
            "SELECT (SELECT COUNT(*) FROM ward_audit WHERE proposal_id=?1 AND event_type='proposal_rejected'),
                    (SELECT COUNT(*) FROM coven_ward_audit_reservations)",
            [&id], |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        assert_eq!((terminals, reservations), (1, 0));
        assert_eq!(
            std::fs::read_to_string(home.join("familiars/sage/output-format.json"))?,
            BEFORE
        );
    }
    Ok(())
}

mod retention_review {
    use super::*;

    fn reservations(conn: &rusqlite::Connection) -> Result<Vec<(String, String, i64)>> {
        let mut query = conn.prepare(
            "SELECT token, purpose, reserved_bytes FROM coven_ward_audit_reservations ORDER BY token",
        )?;
        let rows = query
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    fn interrupted_request(
        home: &Path,
        now: time::OffsetDateTime,
        claimed: bool,
    ) -> Result<(PathBuf, String)> {
        let (pending, document) =
            crate::threads_clock::with_test_time(home, now, || stage_dual_evidence(home, false))?;
        let id = document.pending().id.to_string();
        set_proposal_decision_failpoint(Some((
            ProposalDecisionFailpoint::ClaimBeforeValidation,
            id.clone(),
        )));
        let error = crate::threads_clock::with_test_time(home, now, || {
            decide_threads_proposal(
                home,
                &id,
                "approve",
                Some(&json!({"expectedRevision": document.revision()?}).to_string()),
            )
        })
        .expect_err("interrupt after persisting the real owner decision");
        assert!(
            error.to_string().contains("ClaimBeforeValidation"),
            "{error:#}"
        );
        let conn = store::open_store(&store_path(home))?;
        assert!(load_proposal_apply_intent(&conn, &id)?.is_none());
        let path = if claimed {
            let mut claim = PendingDecisionClaim::acquire(
                home,
                document.pending().id.0,
                PendingDecisionClaimRequest {
                    audit_conn: &conn,
                    decision: "approve",
                    expired: false,
                    new_request: None,
                    expiry_recovery_prevalidated: false,
                    now,
                },
            )?
            .context("claim the durable original request without applying")?;
            claim.preserve();
            claim.path.clone()
        } else {
            pending
        };
        let stored = read_pending_proposal_document(&path)?;
        assert!(stored.decision_request.is_some());
        assert!(stored.decision_state.is_none());
        assert_eq!(reservations(&conn)?.len(), 1);
        Ok((path, id))
    }

    fn exercise(entry: &str, authority: &str, missing_sidecar: bool) -> Result<()> {
        let temp = tempfile::tempdir()?;
        let home = temp.path();
        let now = time::OffsetDateTime::now_utc();
        let later = now + time::Duration::days(31);
        let (path, id) = interrupted_request(home, now, entry != "direct")?;
        let mut document = read_pending_proposal_document(&path)?;
        document.auto_regression_evidence = (!missing_sidecar).then_some([0; 32]);
        let request = document
            .decision_request
            .as_mut()
            .context("durable request")?;
        request.decision_actor = Some("principal:forged".into());
        request.expected_revision = Some("0".repeat(64));
        let original =
            document.serialize_with_decision(document.decision_request.as_ref(), None)?;
        std::fs::write(&path, &original)?;
        let conn = store::open_store(&store_path(home))?;
        let original_reservations = reservations(&conn)?;
        let authority_path = if authority.contains("registry") {
            home.join("familiars.toml")
        } else {
            home.join("familiars/sage/ward.toml")
        };
        let saved = std::fs::read(&authority_path)?;
        let io = authority.ends_with("-io");
        if io {
            std::fs::remove_file(&authority_path)?;
            std::fs::create_dir(&authority_path)?;
        } else if authority.starts_with("missing-") {
            std::fs::remove_file(&authority_path)?;
        } else {
            let probe = if authority == "policy" {
                "\n[[probe]]\nsurface='*.json'\nid='size-delta'\n"
            } else {
                "\n[[probe]]\nsurface='*.json'\nid='pattern-lint'\nforbidden=['indent']\n"
            };
            std::fs::write(
                &authority_path,
                [saved.as_slice(), probe.as_bytes()].concat(),
            )?;
            assert!(ward::WardConfig::load(&home.join("familiars/sage"))?.is_some());
        }
        for pass in 0..2 {
            if entry == "recovery" {
                assert_eq!(
                    crate::threads_clock::with_test_time(home, later, || {
                        process_due_threads_proposals(home)
                    })?,
                    0
                );
            } else {
                let result = crate::threads_clock::with_test_time(home, later, || {
                    decide_threads_proposal(home, &id, "approve", None)
                });
                if io {
                    let error = result.expect_err("genuine read failure must remain retryable");
                    assert!(proposal_authority_read_failure(&error), "{error:#}");
                } else if pass == 0 {
                    let response = result?;
                    assert_eq!(response.status, 409, "{}", response.body);
                    let body: Value = serde_json::from_str(&response.body)?;
                    assert_eq!(body["why"], "proposal-submission-receipt-invalid");
                    assert_eq!(body["quarantined"], true);
                } else {
                    assert_eq!(result?.status, 404);
                }
            }
        }
        assert_eq!(
            std::fs::read_to_string(home.join("familiars/sage/output-format.json"))?,
            BEFORE
        );
        assert!(proposal_terminal_event(&conn, &id)?.is_none());
        assert!(load_proposal_apply_intent(&conn, &id)?.is_none());
        let actual_reservations = reservations(&conn)?;
        let retained = if io {
            assert!(!home.join("pending/quarantine").exists());
            std::fs::read(&path)?
        } else {
            assert!(!path.exists());
            assert!(find_any_pending_decision_claim(home, &id).is_none());
            let entries = std::fs::read_dir(home.join("pending/quarantine"))?
                .collect::<std::io::Result<Vec<_>>>()?;
            assert_eq!(entries.len(), 1);
            std::fs::read(entries[0].path())?
        };
        let retained_value: Value = serde_json::from_slice(&retained)?;
        assert!(
            retained == original && actual_reservations == original_reservations,
            "{entry}/{authority}/missing={missing_sidecar}: envelope_preserved={}, original_reservations={original_reservations:?}, retained_reservations={actual_reservations:?}, retained_request={}",
            retained == original, retained_value["decisionRequest"],
        );
        assert!(actual_reservations
            .iter()
            .all(|(token, _, _)| !token.ends_with(":expiry")));
        if io {
            std::fs::remove_dir(&authority_path)?;
            std::fs::write(&authority_path, saved)?;
            assert_eq!(
                crate::threads_clock::with_test_time(home, later, || {
                    process_due_threads_proposals(home)
                })?,
                0
            );
            assert!(!path.exists());
            assert_eq!(reservations(&conn)?, original_reservations);
            assert!(proposal_terminal_event(&conn, &id)?.is_none());
        }
        Ok(())
    }

    #[test]
    fn direct_decision_preserves_unreplayable_or_missing_authority_evidence() -> Result<()> {
        for entry in ["direct", "claimed-direct"] {
            for authority in ["policy", "failed-probe", "missing-ward", "missing-registry"] {
                for missing in [false, true] {
                    exercise(entry, authority, missing)?;
                }
            }
        }
        Ok(())
    }

    #[test]
    fn existing_claim_recovery_preserves_unreplayable_or_missing_authority_evidence() -> Result<()>
    {
        for authority in ["policy", "failed-probe", "missing-ward", "missing-registry"] {
            for missing in [false, true] {
                exercise("recovery", authority, missing)?;
            }
        }
        Ok(())
    }

    #[test]
    fn genuine_io_retains_retry_evidence_without_expiry() -> Result<()> {
        for entry in ["direct", "claimed-direct", "recovery"] {
            for authority in ["ward-io", "registry-io"] {
                for missing in [false, true] {
                    exercise(entry, authority, missing)?;
                }
            }
        }
        Ok(())
    }

    #[test]
    fn already_bound_expiry_resumes_once() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let home = temp.path();
        let now = time::OffsetDateTime::now_utc();
        let later = now + time::Duration::days(31);
        let (_, id) = interrupted_request(home, now, true)?;
        set_proposal_decision_failpoint(Some((
            ProposalDecisionFailpoint::ClaimBeforeValidation,
            id.clone(),
        )));
        assert!(
            crate::threads_clock::with_test_time(home, later, || expire_threads_proposal(
                home, &id
            ))
            .is_err()
        );
        let path = find_any_pending_decision_claim(home, &id)
            .map(|(path, _)| path)
            .or(find_pending_proposal(home, Uuid::parse_str(&id)?)?)
            .context("bound expiry")?;
        let document = read_pending_proposal_document(&path)?;
        let conn = store::open_store(&store_path(home))?;
        assert!(bound_proposal_expiry_transition(home, &conn, &document)?.is_some());
        assert_eq!(
            document
                .decision_request
                .as_ref()
                .context("expiry request")?
                .decision_actor,
            None
        );
        assert!(
            document
                .decision_request
                .as_ref()
                .context("expiry request")?
                .expired
        );
        assert_eq!(
            crate::threads_clock::with_test_time(home, later, || process_due_threads_proposals(
                home
            ))?,
            1
        );
        assert_eq!(
            crate::threads_clock::with_test_time(home, later, || process_due_threads_proposals(
                home
            ))?,
            0
        );
        let (count, decision): (i64, String) = conn.query_row(
            "SELECT COUNT(*), decision FROM ward_audit WHERE proposal_id=?1 AND event_type='proposal_rejected'",
            [&id], |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        assert_eq!((count, decision.as_str()), (1, "expired"));
        assert!(reservations(&conn)?.is_empty());
        assert_eq!(
            std::fs::read_to_string(home.join("familiars/sage/output-format.json"))?,
            BEFORE
        );
        Ok(())
    }
}

#[test]
fn output_auto_rejection_proof_cannot_discard_forged_origin_before_revision_cleanup() -> Result<()>
{
    for days in [0, 31] {
        let temp = tempfile::tempdir()?;
        let home = temp.path();
        let now = time::OffsetDateTime::now_utc();
        let (_, document) =
            crate::threads_clock::with_test_time(home, now, || stage_dual_evidence(home, false))?;
        let id = document.pending().id.to_string();
        set_proposal_decision_failpoint(Some((
            ProposalDecisionFailpoint::ClaimBeforeValidation,
            id.clone(),
        )));
        assert!(decide_threads_proposal(
            home,
            &id,
            "approve",
            Some(&json!({"expectedRevision":document.revision()?}).to_string()),
        )
        .is_err());
        let claim = find_any_pending_decision_claim(home, &id)
            .map(|(path, _)| path)
            .or(find_pending_proposal(home, document.pending().id.0)?)
            .context("durable request")?;
        let mut value: Value = serde_json::from_slice(&std::fs::read(&claim)?)?;
        assert!(value.get("decisionRequest").is_some());
        value["autoRegressionEvidence"] = json!(vec![0_u8; 32]);
        value["decisionRequest"]["decisionActor"] = json!("principal:forged");
        value["decisionRequest"]["expectedRevision"] = json!("0".repeat(64));
        let changed = parse_proposal_envelope(&serde_json::to_vec(&value)?)?;
        let bytes = changed.serialize_with_decision(
            changed.decision_request.as_ref(),
            changed.decision_state.as_ref(),
        )?;
        std::fs::write(&claim, &bytes)?;
        for pass in 0..2 {
            assert_eq!(
                crate::threads_clock::with_test_time(
                    home,
                    now + time::Duration::days(days),
                    || process_due_threads_proposals(home)
                )?,
                0,
                "days={days}, pass={pass}"
            );
        }
        assert!(!claim.exists());
        let quarantined = std::fs::read_dir(home.join("pending/quarantine"))?
            .collect::<std::io::Result<Vec<_>>>()?;
        assert_eq!(quarantined.len(), 1);
        assert_eq!(std::fs::read(quarantined[0].path())?, bytes);
        let conn = store::open_store(&store_path(home))?;
        assert!(proposal_terminal_event(&conn, &id)?.is_none());
        assert_eq!(
            std::fs::read_to_string(home.join("familiars/sage/output-format.json"))?,
            BEFORE
        );
    }
    Ok(())
}
