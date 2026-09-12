pub(super) const BEFORE: &str =
    r#"{"schema":"coven.output-format/v1","indent":2,"final_newline":true}"#;
pub(super) const AFTER: &str =
    r#"{"schema":"coven.output-format/v1","indent":4,"final_newline":false}"#;

pub(super) fn stage_supported_output_auto(home: &Path) -> Result<(PathBuf, String)> {
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
