#[cfg(target_os = "linux")]
#[test]
fn output_auto_chain_non_utf8_destination_never_becomes_unrelated() -> Result<()> {
    use std::os::unix::ffi::OsStringExt;
    run_clocked_journey(
        "output-auto-chain-non-utf8-destination",
        |home, workspace| {
            seed_chain(home, workspace, false, false)?;
            let destination = std::ffi::OsString::from_vec(b"notes-\xff.json".to_vec());
            fs::write(workspace.join(&destination), FORMAT_BEFORE)?;
            fs::hard_link(
                workspace.join(&destination),
                workspace.join("notes-alias.json"),
            )?;
            fs::remove_file(workspace.join(FORMAT_PATH))?;
            std::os::unix::fs::symlink(&destination, workspace.join(FORMAT_PATH))?;
            Ok(())
        },
        |fixture, _| {
            assert_alias_refused(fixture, "notes-alias.json", true)?;
            anyhow::ensure!(
                fs::read_to_string(fixture.workspace.join("notes-alias.json"))? == FORMAT_BEFORE
            );
            Ok(())
        },
    )
}

#[test]
fn output_auto_chain_resolves_parent_after_directory_symlink() -> Result<()> {
    run_clocked_journey(
        "output-auto-chain-directory-parent",
        |home, workspace| {
            seed_chain(home, workspace, false, false)?;
            fs::create_dir_all(workspace.join("nested/deep"))?;
            fs::write(workspace.join("nested/notes.json"), FORMAT_BEFORE)?;
            std::os::unix::fs::symlink("nested/deep", workspace.join("directory-alias"))?;
            fs::remove_file(workspace.join(FORMAT_PATH))?;
            std::os::unix::fs::symlink(
                "directory-alias/../notes.json",
                workspace.join(FORMAT_PATH),
            )?;
            Ok(())
        },
        |fixture, _| {
            assert_alias_refused(fixture, "format-alias.json", true)?;
            anyhow::ensure!(
                fs::read_to_string(fixture.workspace.join("nested/notes.json"))? == FORMAT_BEFORE
            );
            Ok(())
        },
    )
}

fn seed_chain(home: &Path, workspace: &Path, human: bool, broken: bool) -> Result<()> {
    seed_output_auto(home, workspace, false)?;
    if human {
        stronger_policy(workspace)?;
    }
    fs::write(workspace.join("unrelated.txt"), "original unrelated")?;
    if !broken {
        fs::write(workspace.join("notes.json"), FORMAT_BEFORE)?;
    }
    fs::remove_file(workspace.join(FORMAT_PATH))?;
    std::os::unix::fs::symlink("notes.json", workspace.join(FORMAT_PATH))?;
    std::os::unix::fs::symlink(FORMAT_PATH, workspace.join("format-alias.json"))?;
    Ok(())
}

fn chain_case(human: bool, mixed: bool, direct: bool, broken: bool) -> Result<()> {
    run_clocked_journey(
        &format!("output-auto-chain-{human}-{mixed}-{direct}-{broken}"),
        |home, workspace| seed_chain(home, workspace, human, broken),
        |fixture, _| {
            let before_audit: i64 =
                fixture
                    .store()?
                    .query_row("SELECT COUNT(*) FROM ward_audit", [], |row| row.get(0))?;
            assert_alias_refused(
                fixture,
                if direct {
                    "notes.json"
                } else {
                    "format-alias.json"
                },
                mixed,
            )?;
            let after_audit: i64 =
                fixture
                    .store()?
                    .query_row("SELECT COUNT(*) FROM ward_audit", [], |row| row.get(0))?;
            anyhow::ensure!(
                before_audit == after_audit,
                "refused chain partially mutated the audit"
            );
            anyhow::ensure!(
                fs::read_link(fixture.workspace.join(FORMAT_PATH))? == Path::new("notes.json")
            );
            anyhow::ensure!(
                fs::read_link(fixture.workspace.join("format-alias.json"))?
                    == Path::new(FORMAT_PATH)
            );
            if broken {
                anyhow::ensure!(
                    !fixture.workspace.join("notes.json").exists(),
                    "refused alias created its broken destination"
                );
            } else {
                anyhow::ensure!(
                    fs::read_to_string(fixture.workspace.join("notes.json"))? == FORMAT_BEFORE
                );
            }
            let pending = fixture.request("GET", "/api/v1/threads/proposals", None)?;
            anyhow::ensure!(
                pending.status == 200 && pending.body["proposals"] == json!([]),
                "{pending:?}"
            );
            Ok(())
        },
    )
}

#[test]
fn output_auto_chain_composed_auto_single() -> Result<()> {
    chain_case(false, false, false, false)
}
#[test]
fn output_auto_chain_composed_auto_mixed() -> Result<()> {
    chain_case(false, true, false, false)
}
#[test]
fn output_auto_chain_composed_human_single() -> Result<()> {
    chain_case(true, false, false, false)
}
#[test]
fn output_auto_chain_composed_human_mixed() -> Result<()> {
    chain_case(true, true, false, false)
}
#[test]
fn output_auto_chain_destination_auto_single() -> Result<()> {
    chain_case(false, false, true, false)
}
#[test]
fn output_auto_chain_destination_auto_mixed() -> Result<()> {
    chain_case(false, true, true, false)
}
#[test]
fn output_auto_chain_destination_human_single() -> Result<()> {
    chain_case(true, false, true, false)
}
#[test]
fn output_auto_chain_destination_human_mixed() -> Result<()> {
    chain_case(true, true, true, false)
}
#[test]
fn output_auto_chain_broken_destination_auto() -> Result<()> {
    chain_case(false, true, true, true)
}
#[test]
fn output_auto_chain_broken_destination_human() -> Result<()> {
    chain_case(true, true, true, true)
}

#[test]
fn output_auto_chain_composed_broken_destination() -> Result<()> {
    for human in [false, true] {
        chain_case(human, true, false, true)?;
    }
    Ok(())
}

#[test]
fn output_auto_chain_destination_hardlink_is_not_unrelated() -> Result<()> {
    for human in [false, true] {
        run_clocked_journey(
            &format!("output-auto-chain-destination-hardlink-{human}"),
            |home, workspace| {
                seed_chain(home, workspace, human, false)?;
                fs::hard_link(
                    workspace.join("notes.json"),
                    workspace.join("notes-alias.json"),
                )?;
                Ok(())
            },
            |fixture, _| {
                assert_alias_refused(fixture, "notes-alias.json", true)?;
                anyhow::ensure!(
                    fs::read_to_string(fixture.workspace.join("notes.json"))? == FORMAT_BEFORE
                );
                anyhow::ensure!(
                    fs::read_to_string(fixture.workspace.join("notes-alias.json"))?
                        == FORMAT_BEFORE
                );
                Ok(())
            },
        )?;
    }
    Ok(())
}

fn unrelated_chain_case(broken: bool) -> Result<()> {
    for human in [false, true] {
        run_clocked_journey(
            &format!("output-auto-chain-unrelated-{human}-{broken}"),
            |home, workspace| seed_chain(home, workspace, human, broken),
            |fixture, _| {
                let response = fixture.request("POST", "/api/v1/familiars/sage/edits",
                    Some(&json!({"edits":[{"target":"unrelated.txt","contents":"ordinary replacement"}]})))?;
                anyhow::ensure!(
                    response.status == 200 && response.body["disposition"] == "applied",
                    "{response:?}"
                );
                anyhow::ensure!(
                    fs::read_to_string(fixture.workspace.join("unrelated.txt"))?
                        == "ordinary replacement"
                );
                if broken {
                    anyhow::ensure!(!fixture.workspace.join("notes.json").exists());
                } else {
                    anyhow::ensure!(
                        fs::read_to_string(fixture.workspace.join("notes.json"))? == FORMAT_BEFORE
                    );
                }
                anyhow::ensure!(
                    fs::read_link(fixture.workspace.join(FORMAT_PATH))? == Path::new("notes.json")
                );
                Ok(())
            },
        )?;
    }
    Ok(())
}

#[test]
fn output_auto_chain_unrelated_ordinary_with_symlink() -> Result<()> {
    unrelated_chain_case(false)
}
#[test]
fn output_auto_chain_unrelated_ordinary_with_broken_symlink() -> Result<()> {
    unrelated_chain_case(true)
}
