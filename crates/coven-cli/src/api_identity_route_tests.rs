#[test]
fn direct_identity_commit_refuses_source_and_policy_drift_as_a_batch() -> Result<()> {
    for target in ["first.txt", "second.txt"] {
        for source in ["IDENTITY.md", "SOUL.md", "familiars.toml", "ward.toml", "missing", "invalid"] {
            let temp = tempfile::tempdir()?;
            let home = temp.path();
            let workspace = seed_identity_predicate_familiar(home)?;
            for file in ["first.txt", "second.txt"] {
                std::fs::write(workspace.join(file), "before")?;
            }
            let path = match source {
                "familiars.toml" => home.join(source),
                "missing" | "invalid" => workspace.join("IDENTITY.md"),
                _ => workspace.join(source),
            };
            let original = std::fs::read_to_string(&path)?;
            let replacement = match source {
                "familiars.toml" => original.replace("Research", "Synthesis"),
                "ward.toml" => original.replace("expected = \"research\"", "expected = \"search\""),
                "invalid" => "# IDENTITY.md - Synthetic-other\n".to_owned(),
                _ => format!("{original}\n<!-- Synthetic source revision. -->\n"),
            };
            let changed = std::rc::Rc::new(std::cell::Cell::new(false));
            let observed = changed.clone();
            let retained_path = path.clone();
            ward::set_direct_commit_hook(target, move || {
                if source == "missing" {
                    std::fs::remove_file(path).expect("remove synthetic identity source");
                } else {
                    std::fs::write(path, replacement).expect("revise synthetic authority");
                }
                observed.set(true);
            });
            let response = post_edits(
                home,
                r#"{"edits":[{"target":"first.txt","contents":"after"},{"target":"second.txt","contents":"after"}]}"#,
            )?;
            assert!(changed.get(), "{target}/{source}: commit seam was not reached");
            assert_ne!(
                response.status, 200,
                "{target}/{source}: stale identity authorized ordinary writes: {}",
                response.body
            );
            let body: Value = serde_json::from_str(&response.body)?;
            assert_eq!(body["error"]["details"]["writeApplied"], false, "{body}");
            for file in ["first.txt", "second.txt"] {
                assert_eq!(std::fs::read_to_string(workspace.join(file))?, "before");
            }
            if source == "missing" {
                assert!(!retained_path.exists());
            } else {
                assert_ne!(std::fs::read_to_string(retained_path)?, original);
            }
            let conn = store::open_store(&home.join("coven.sqlite3"))?;
            let applied: i64 = conn.query_row(
                "SELECT COUNT(*) FROM ward_audit WHERE event_type IN ('apply_audit', 'proposal_approved', 'proposal_submitted')",
                [],
                |row| row.get(0),
            )?;
            assert_eq!(applied, 0);
        }
    }
    Ok(())
}

#[test]
fn direct_identity_commit_accepts_exact_sources_on_logged_and_free_routes() -> Result<()> {
    for tier in [2, 3] {
        let temp = tempfile::tempdir()?;
        let home = temp.path();
        let workspace = seed_identity_predicate_familiar(home)?;
        let path = workspace.join("ward.toml");
        let config = std::fs::read_to_string(&path)?;
        std::fs::write(path, format!("{config}\n[[surface]]\npath='note.txt'\ntier={tier}\n"))?;
        let identity = std::fs::read(workspace.join("IDENTITY.md"))?;
        let response = post_edits(home, r#"{"edits":[{"target":"note.txt","contents":"after"}]}"#)?;
        assert_eq!(response.status, 200, "{}", response.body);
        assert_eq!(std::fs::read_to_string(workspace.join("note.txt"))?, "after");
        assert_eq!(std::fs::read(workspace.join("IDENTITY.md"))?, identity);
    }
    Ok(())
}

#[test]
fn identity_adapter_rejects_unsupported_missing_and_ambiguous_sources() -> Result<()> {
    for case in ["unsupported", "missing", "conflicting", "empty", "punctuation"] {
        let temp = tempfile::tempdir()?;
        let home = temp.path();
        let workspace = seed_identity_predicate_familiar(home)?;
        let mut config = ward::WardConfig::load(&workspace)?.context("identity config")?;
        let identity = workspace.join("IDENTITY.md");
        match case {
            "unsupported" => {
                let mut value = serde_json::to_value(&config)?;
                value["identity_invariant"][0]["operator"] = json!("advisory_probe");
                assert!(serde_json::from_value::<ward::WardConfig>(value).is_err());
                // An invalid supported declaration must also refuse at the adapter boundary.
                let mut value = serde_json::to_value(&config)?;
                value["identity_invariant"][0]["expected"] = json!("");
                config = serde_json::from_value(value)?;
            }
            "missing" => std::fs::remove_file(&identity)?,
            "conflicting" => std::fs::write(&identity, "# IDENTITY.md - Synthetic-identity\n- **Name:** Other\n")?,
            "empty" => std::fs::write(&identity, "# IDENTITY.md - Synthetic-identity\n- **Name:**\n")?,
            "punctuation" => std::fs::write(&identity, "# IDENTITY.md - Synthetic-identity.\n")?,
            _ => unreachable!(),
        }
        let context = crate::ward_identity::candidate_identity_context(
            home, "sage", &workspace, &config, &[], &ward::Authorization::unsigned(), None,
        );
        let rejected = crate::ward_identity::candidate_rejection(&config, context.as_ref());
        assert!(rejected.is_err() || rejected?.is_some(), "{case} must fail closed");
    }
    Ok(())
}

#[test]
fn identity_adapter_exact_binding_distinguishes_stale_source_from_invalid_facts() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let home = temp.path();
    let workspace = seed_identity_predicate_familiar(home)?;
    let config = ward::WardConfig::load(&workspace)?.context("identity config")?;
    let context = || crate::ward_identity::candidate_identity_context(
        home, "sage", &workspace, &config, &[], &ward::Authorization::unsigned(), None,
    ).context("candidate identity context");
    let original = context()?;
    assert!(crate::ward_identity::candidate_rejection(&config, Some(&original))?.is_none());
    assert_eq!(
        crate::ward_identity::candidate_binding(&config, Some(&original))?,
        crate::ward_identity::candidate_binding(&config, Some(&context()?))?,
    );
    let path = workspace.join("IDENTITY.md");
    let bytes = std::fs::read_to_string(&path)?;
    std::fs::write(path, format!("{bytes}\n<!-- Synthetic source revision. -->\n"))?;
    let revised = context()?;
    assert!(crate::ward_identity::candidate_rejection(&config, Some(&revised))?.is_none());
    assert_ne!(
        crate::ward_identity::candidate_binding(&config, Some(&original))?,
        crate::ward_identity::candidate_binding(&config, Some(&revised))?,
    );
    assert!(crate::ward_identity::candidate_binding(&config, None).is_err());
    Ok(())
}
