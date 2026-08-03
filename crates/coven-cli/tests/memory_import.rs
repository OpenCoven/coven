use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use anyhow::Result;
use serde_json::Value;

#[test]
fn preview_native_reports_a_sorted_redacted_plan_without_creating_targets() -> Result<()> {
    let temp = trusted_tempdir()?;
    let workspace = temp.path().join("native-workspace");
    write_familiar(temp.path(), "sage", &workspace)?;
    write_file(&workspace.join("notes/z.md"), b"z")?;
    write_file(&workspace.join("MEMORY.md"), b"root secret")?;
    write_file(&workspace.join("memory/a.md"), b"a")?;
    write_file(&workspace.join("AGENTS.md"), b"excluded sentinel")?;
    write_file(
        &workspace.join("sessions/private.md"),
        b"excluded session sentinel",
    )?;

    let output = run_coven(
        temp.path(),
        &["memory", "import", "--familiar", "sage", "--json"],
    )?;
    assert_success(&output);
    let report: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(report["status"], "preview");
    assert_eq!(report["familiar_id"], "sage");
    assert_eq!(report["source_kind"], "native");
    assert_exact_object_keys(
        &report,
        &[
            "familiar_id",
            "source_kind",
            "bundle_id",
            "status",
            "file_count",
            "created_count",
            "unchanged_count",
            "restored_count",
            "conflict_count",
            "entries",
        ],
    );
    assert_eq!(report["file_count"], 3);
    assert_eq!(report["created_count"], 3);
    assert_eq!(report["unchanged_count"], 0);
    assert_eq!(report["restored_count"], 0);
    assert_eq!(report["conflict_count"], 0);
    assert!(report["bundle_id"]
        .as_str()
        .is_some_and(|bundle| bundle.starts_with("blake3-") && bundle.len() == 71));
    assert_eq!(
        report["entries"],
        serde_json::json!([
            {
                "source_label": "MEMORY.md",
                "target_name": "memory.md",
                "digest": format!("blake3:{}", blake3::hash(b"root secret").to_hex()),
                "status": "create"
            },
            {
                "source_label": "memory/a.md",
                "target_name": "memory-a.md",
                "digest": format!("blake3:{}", blake3::hash(b"a").to_hex()),
                "status": "create"
            },
            {
                "source_label": "notes/z.md",
                "target_name": "notes-z.md",
                "digest": format!("blake3:{}", blake3::hash(b"z").to_hex()),
                "status": "create"
            }
        ])
    );
    for entry in report["entries"]
        .as_array()
        .expect("entries must be an array")
    {
        assert_exact_object_keys(entry, &["source_label", "target_name", "digest", "status"]);
    }
    assert_no_absolute_path_values(&report);
    let rendered = String::from_utf8(output.stdout)?;
    assert!(!rendered.contains("root secret"));
    assert!(!rendered.contains("excluded sentinel"));
    assert!(!rendered.contains(&workspace.to_string_lossy().into_owned()));
    assert!(!temp.path().join("memory").exists());

    let human = run_coven(temp.path(), &["memory", "import", "--familiar", "sage"])?;
    assert_success(&human);
    let human = String::from_utf8(human.stdout)?;
    assert!(human.contains("Preview"), "{human}");
    assert!(human.contains("memory/a.md"), "{human}");
    assert!(human.contains("memory-a.md"), "{human}");
    assert!(human.contains("Bundle: blake3-"), "{human}");
    assert!(human.contains("eligible"), "{human}");
    assert!(!human.contains("root secret"), "{human}");
    assert!(!human.contains(&workspace.to_string_lossy().into_owned()));
    assert!(!temp.path().join("memory").exists());
    Ok(())
}

#[test]
fn preview_openclaw_uses_explicit_root_and_registered_target_without_leaks() -> Result<()> {
    let temp = trusted_tempdir()?;
    let workspace = temp.path().join("native-workspace");
    let openclaw = temp.path().join("openclaw-workspace");
    write_familiar(temp.path(), "sage", &workspace)?;
    write_file(&workspace.join("MEMORY.md"), b"native must not win")?;
    write_file(&openclaw.join("memory/topic.md"), b"topic secret")?;
    write_file(&openclaw.join("DREAMS.md"), b"dream secret")?;
    write_file(&openclaw.join("MEMORY.md"), b"root secret")?;
    write_file(&openclaw.join("notes/excluded.md"), b"excluded notes")?;
    write_file(&openclaw.join("TOOLS.md"), b"excluded tools")?;

    let output = run_coven(
        temp.path(),
        &[
            "memory",
            "import",
            "--familiar",
            "sage",
            "--source",
            "openclaw",
            "--openclaw-root",
            openclaw.to_str().expect("fixture path is UTF-8"),
            "--json",
        ],
    )?;
    assert_success(&output);
    let report: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(report["status"], "preview");
    assert_eq!(report["familiar_id"], "sage");
    assert_eq!(report["source_kind"], "openclaw");
    assert_eq!(
        report["entries"]
            .as_array()
            .expect("entries must be an array")
            .iter()
            .map(|entry| (
                entry["source_label"].as_str().unwrap(),
                entry["target_name"].as_str().unwrap(),
                entry["status"].as_str().unwrap()
            ))
            .collect::<Vec<_>>(),
        vec![
            ("DREAMS.md", "dreams.md", "create"),
            ("MEMORY.md", "memory.md", "create"),
            ("memory/topic.md", "memory-topic.md", "create")
        ]
    );
    let rendered = String::from_utf8(output.stdout)?;
    for forbidden in [
        "topic secret",
        "dream secret",
        "root secret",
        "native must not win",
        "excluded notes",
        "excluded tools",
        openclaw.to_str().expect("fixture path is UTF-8"),
    ] {
        assert!(!rendered.contains(forbidden), "leaked {forbidden:?}");
    }

    let human = run_coven(
        temp.path(),
        &[
            "memory",
            "import",
            "--familiar",
            "sage",
            "--source",
            "openclaw",
            "--openclaw-root",
            openclaw.to_str().expect("fixture path is UTF-8"),
        ],
    )?;
    assert_success(&human);
    let human = String::from_utf8(human.stdout)?;
    assert!(human.contains("Preview for familiar `sage`"), "{human}");
    assert!(human.contains("DREAMS.md -> dreams.md [create]"), "{human}");
    assert!(human.contains("Bundle: blake3-"), "{human}");
    assert!(!human.contains("dream secret"), "{human}");
    assert!(!human.contains(openclaw.to_str().expect("fixture path is UTF-8")));
    assert!(!temp.path().join("memory").exists());
    Ok(())
}

#[test]
fn preview_conflict_is_whole_plan_ineligible_and_creates_nothing() -> Result<()> {
    let temp = trusted_tempdir()?;
    let workspace = temp.path().join("native-workspace");
    write_familiar(temp.path(), "sage", &workspace)?;
    write_file(&workspace.join("MEMORY.md"), b"new bytes")?;
    write_file(&workspace.join("notes/new.md"), b"create bytes")?;
    write_file(&temp.path().join("memory/sage/memory.md"), b"old bytes")?;

    let output = run_coven(
        temp.path(),
        &["memory", "import", "--familiar", "sage", "--json"],
    )?;
    assert_success(&output);
    let report: Value = serde_json::from_slice(&output.stdout)?;

    assert_eq!(report["status"], "conflict");
    assert_eq!(report["created_count"], 1);
    assert_eq!(report["restored_count"], 0);
    assert_eq!(report["conflict_count"], 1);
    assert_eq!(report["entries"][0]["status"], "conflict");
    assert_eq!(report["entries"][1]["status"], "create");
    assert!(!temp.path().join("memory/sage/notes-new.md").exists());
    assert!(!temp.path().join("memory-import").exists());
    assert!(!temp.path().join("journal").exists());
    Ok(())
}

#[test]
#[cfg(not(windows))]
fn apply_publishes_a_private_redacted_bundle_and_leaves_sources_unchanged() -> Result<()> {
    let temp = trusted_tempdir()?;
    let workspace = temp.path().join("native-workspace");
    write_familiar(temp.path(), "sage", &workspace)?;
    write_file(&workspace.join("MEMORY.md"), b"secret")?;
    let source_before = fs::read(workspace.join("MEMORY.md"))?;

    let output = run_coven(
        temp.path(),
        &[
            "memory",
            "import",
            "--familiar",
            "sage",
            "--apply",
            "--json",
        ],
    )?;

    assert_success(&output);
    let report: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(report["status"], "verified");
    assert_eq!(report["created_count"], 1);
    assert_eq!(report["unchanged_count"], 0);
    assert_eq!(report["entries"][0]["status"], "create");
    assert_eq!(
        fs::read(temp.path().join("memory/sage/memory.md"))?,
        b"secret"
    );
    assert_eq!(fs::read(workspace.join("MEMORY.md"))?, source_before);
    let rendered = String::from_utf8(output.stdout)?;
    assert!(!rendered.contains("secret"));
    assert!(!rendered.contains(&workspace.to_string_lossy().into_owned()));
    assert!(temp.path().join("memory-migrations/sage").is_dir());
    Ok(())
}

#[test]
#[cfg(not(windows))]
fn apply_conflict_creates_no_bundle_or_additional_target() -> Result<()> {
    let temp = trusted_tempdir()?;
    let workspace = temp.path().join("native-workspace");
    write_familiar(temp.path(), "sage", &workspace)?;
    write_file(&workspace.join("MEMORY.md"), b"new bytes")?;
    write_file(&workspace.join("notes/new.md"), b"would create")?;
    write_file(&temp.path().join("memory/sage/memory.md"), b"old bytes")?;

    let output = run_coven(
        temp.path(),
        &[
            "memory",
            "import",
            "--familiar",
            "sage",
            "--apply",
            "--json",
        ],
    )?;

    assert!(!output.status.success());
    assert!(!temp.path().join("memory/sage/notes-new.md").exists());
    assert!(!temp.path().join("memory-migrations").exists());
    assert_eq!(
        fs::read(temp.path().join("memory/sage/memory.md"))?,
        b"old bytes"
    );
    Ok(())
}

#[test]
#[cfg(not(windows))]
fn apply_rerun_is_idempotent_and_isolated_to_one_familiar() -> Result<()> {
    let temp = trusted_tempdir()?;
    let sage_workspace = temp.path().join("sage-workspace");
    let cody_workspace = temp.path().join("cody-workspace");
    write_familiars(
        temp.path(),
        &[("sage", &sage_workspace), ("cody", &cody_workspace)],
    )?;
    write_file(&sage_workspace.join("MEMORY.md"), b"sage bytes")?;
    write_file(&cody_workspace.join("MEMORY.md"), b"cody bytes")?;
    write_file(
        &temp.path().join("memory/cody/existing.md"),
        b"cody sentinel",
    )?;

    let first = run_coven(
        temp.path(),
        &[
            "memory",
            "import",
            "--familiar",
            "sage",
            "--apply",
            "--json",
        ],
    )?;
    assert_success(&first);
    let first_report: Value = serde_json::from_slice(&first.stdout)?;
    let second = run_coven(
        temp.path(),
        &[
            "memory",
            "import",
            "--familiar",
            "sage",
            "--apply",
            "--json",
        ],
    )?;
    assert_success(&second);
    let second_report: Value = serde_json::from_slice(&second.stdout)?;

    assert_eq!(first_report["bundle_id"], second_report["bundle_id"]);
    assert_eq!(second_report["created_count"], 0);
    assert_eq!(second_report["unchanged_count"], 1);
    assert_eq!(
        fs::read(temp.path().join("memory/sage/memory.md"))?,
        b"sage bytes"
    );
    assert_eq!(
        fs::read(temp.path().join("memory/cody/existing.md"))?,
        b"cody sentinel"
    );
    assert!(!temp.path().join("memory/cody/memory.md").exists());
    assert!(!temp.path().join("memory-migrations/cody").exists());
    Ok(())
}

#[test]
#[cfg(not(windows))]
fn apply_preserves_an_existing_exact_target_and_human_output_is_redacted() -> Result<()> {
    let temp = trusted_tempdir()?;
    let workspace = temp.path().join("native-workspace");
    write_familiar(temp.path(), "sage", &workspace)?;
    write_file(&workspace.join("MEMORY.md"), b"same bytes")?;
    let target = temp.path().join("memory/sage/memory.md");
    write_file(&target, b"same bytes")?;
    let before = fs::metadata(&target)?;

    let output = run_coven(
        temp.path(),
        &["memory", "import", "--familiar", "sage", "--apply"],
    )?;

    assert_success(&output);
    let human = String::from_utf8(output.stdout)?;
    assert!(
        human.contains("Verified import for familiar `sage`"),
        "{human}"
    );
    assert!(human.contains("0 create, 1 unchanged"), "{human}");
    assert!(
        human.contains("source files were left unchanged"),
        "{human}"
    );
    assert!(!human.contains("same bytes"), "{human}");
    assert!(!human.contains(&workspace.to_string_lossy().into_owned()));
    let after = fs::metadata(&target)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        assert_eq!((before.dev(), before.ino()), (after.dev(), after.ino()));
    }
    #[cfg(not(unix))]
    let _ = (before, after);
    assert_eq!(fs::read(target)?, b"same bytes");
    Ok(())
}

#[cfg(windows)]
#[test]
fn apply_fails_closed_before_mutation_when_directory_durability_is_unavailable() -> Result<()> {
    let temp = trusted_tempdir()?;
    let workspace = temp.path().join("native-workspace");
    write_familiar(temp.path(), "sage", &workspace)?;
    write_file(&workspace.join("MEMORY.md"), b"same bytes")?;

    let output = run_coven(
        temp.path(),
        &[
            "memory",
            "import",
            "--familiar",
            "sage",
            "--apply",
            "--json",
        ],
    )?;

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr)?;
    assert!(stderr.contains("durable directory publication"), "{stderr}");
    assert!(!temp.path().join("memory").exists());
    assert!(!temp.path().join("memory-migrations").exists());
    Ok(())
}

#[test]
#[cfg(not(windows))]
fn restore_logically_hides_an_unchanged_imported_target() -> Result<()> {
    let temp = trusted_tempdir()?;
    let workspace = temp.path().join("native-workspace");
    write_familiar(temp.path(), "sage", &workspace)?;
    write_file(&workspace.join("MEMORY.md"), b"restore bytes")?;
    let applied = run_coven(
        temp.path(),
        &[
            "memory",
            "import",
            "--familiar",
            "sage",
            "--apply",
            "--json",
        ],
    )?;
    assert_success(&applied);
    let applied_report: Value = serde_json::from_slice(&applied.stdout)?;
    let bundle = applied_report["bundle_id"]
        .as_str()
        .expect("apply report has bundle ID");

    let restored = run_coven(
        temp.path(),
        &[
            "memory",
            "restore",
            "--familiar",
            "sage",
            "--bundle",
            bundle,
            "--json",
        ],
    )?;

    assert_success(&restored);
    let report: Value = serde_json::from_slice(&restored.stdout)?;
    assert_eq!(report["status"], "restored");
    assert_eq!(report["restored_count"], 1);
    assert_eq!(
        fs::read(temp.path().join("memory/sage/memory.md"))?,
        b"restore bytes"
    );

    let human = run_coven(
        temp.path(),
        &[
            "memory",
            "restore",
            "--familiar",
            "sage",
            "--bundle",
            bundle,
        ],
    )?;
    assert_success(&human);
    let human = String::from_utf8(human.stdout)?;
    assert!(
        human.contains("Logical restore for familiar `sage`"),
        "{human}"
    );
    assert!(human.contains("1 suppressed"), "{human}");
    assert!(human.contains("hidden from Coven readers"), "{human}");
    assert!(!human.contains("restore bytes"), "{human}");
    Ok(())
}

#[test]
#[cfg(not(windows))]
fn restore_conflict_returns_nonzero_with_a_redacted_report() -> Result<()> {
    let temp = trusted_tempdir()?;
    let workspace = temp.path().join("native-workspace");
    write_familiar(temp.path(), "sage", &workspace)?;
    write_file(&workspace.join("MEMORY.md"), b"original secret")?;
    let applied = run_coven(
        temp.path(),
        &[
            "memory",
            "import",
            "--familiar",
            "sage",
            "--apply",
            "--json",
        ],
    )?;
    assert_success(&applied);
    let applied_report: Value = serde_json::from_slice(&applied.stdout)?;
    let bundle = applied_report["bundle_id"]
        .as_str()
        .expect("apply report has bundle ID");
    write_file(&temp.path().join("memory/sage/memory.md"), b"edited secret")?;

    let restored = run_coven(
        temp.path(),
        &[
            "memory",
            "restore",
            "--familiar",
            "sage",
            "--bundle",
            bundle,
            "--json",
        ],
    )?;

    assert!(!restored.status.success());
    let report: Value = serde_json::from_slice(&restored.stdout)?;
    assert_eq!(report["status"], "manual_recovery");
    assert_eq!(report["conflict_count"], 1);
    assert_eq!(report["entries"][0]["status"], "conflict");
    let rendered = format!(
        "{}\n{}",
        String::from_utf8_lossy(&restored.stdout),
        String::from_utf8_lossy(&restored.stderr)
    );
    assert!(!rendered.contains("original secret"));
    assert!(!rendered.contains("edited secret"));
    assert!(!rendered.contains(&workspace.to_string_lossy().into_owned()));
    Ok(())
}

fn assert_exact_object_keys(value: &Value, expected: &[&str]) {
    let object = value.as_object().expect("value must be a JSON object");
    let mut actual = object.keys().map(String::as_str).collect::<Vec<_>>();
    actual.sort_unstable();
    let mut expected = expected.to_vec();
    expected.sort_unstable();
    assert_eq!(actual, expected);
}

fn assert_no_absolute_path_values(value: &Value) {
    match value {
        Value::String(value) => {
            assert!(
                !Path::new(value).is_absolute(),
                "report contains an absolute path: {value}"
            );
        }
        Value::Array(values) => {
            for value in values {
                assert_no_absolute_path_values(value);
            }
        }
        Value::Object(values) => {
            for value in values.values() {
                assert_no_absolute_path_values(value);
            }
        }
        _ => {}
    }
}

#[test]
fn source_boundaries_unknown_familiar_fails_before_source_root_access() -> Result<()> {
    let temp = trusted_tempdir()?;
    let workspace = temp.path().join("native-workspace");
    write_familiar(temp.path(), "sage", &workspace)?;
    let missing_root = temp.path().join("must-not-be-touched");

    let output = run_coven(
        temp.path(),
        &[
            "memory",
            "import",
            "--familiar",
            "unknown",
            "--source",
            "openclaw",
            "--openclaw-root",
            missing_root.to_str().expect("fixture path is UTF-8"),
            "--json",
        ],
    )?;
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr)?;
    assert!(stderr.contains("unknown familiar `unknown`"), "{stderr}");
    assert!(!stderr.contains("source root"), "{stderr}");
    assert!(
        !stderr.contains(missing_root.to_str().expect("fixture path is UTF-8")),
        "{stderr}"
    );
    Ok(())
}

#[test]
fn source_boundaries_openclaw_cli_requires_explicit_root_and_target() -> Result<()> {
    let temp = trusted_tempdir()?;
    for args in [
        vec![
            "memory",
            "import",
            "--familiar",
            "sage",
            "--source",
            "openclaw",
        ],
        vec![
            "memory",
            "import",
            "--source",
            "openclaw",
            "--openclaw-root",
            temp.path().to_str().expect("fixture path is UTF-8"),
        ],
    ] {
        let output = run_coven(temp.path(), &args)?;
        assert!(!output.status.success());
    }
    Ok(())
}

#[cfg(unix)]
#[test]
fn source_boundaries_openclaw_symlink_root_fails_closed() -> Result<()> {
    use std::os::unix::fs::symlink;

    let temp = trusted_tempdir()?;
    let workspace = temp.path().join("native-workspace");
    let openclaw = temp.path().join("openclaw");
    let linked = temp.path().join("linked-openclaw");
    write_familiar(temp.path(), "sage", &workspace)?;
    write_file(&openclaw.join("MEMORY.md"), b"secret")?;
    symlink(&openclaw, &linked)?;

    let output = run_coven(
        temp.path(),
        &[
            "memory",
            "import",
            "--familiar",
            "sage",
            "--source",
            "openclaw",
            "--openclaw-root",
            linked.to_str().expect("fixture path is UTF-8"),
        ],
    )?;
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr)?;
    assert!(stderr.contains("real directory"), "{stderr}");
    assert!(!stderr.contains(linked.to_str().expect("fixture path is UTF-8")));
    assert!(!stderr.contains("secret"));
    Ok(())
}

#[cfg(unix)]
#[test]
fn source_boundaries_openclaw_symlinked_intermediate_fails_closed() -> Result<()> {
    use std::os::unix::fs::symlink;

    let temp = trusted_tempdir()?;
    let workspace = temp.path().join("native-workspace");
    let actual_parent = temp.path().join("actual-parent");
    let linked_parent = temp.path().join("linked-parent");
    let openclaw = actual_parent.join("openclaw");
    write_familiar(temp.path(), "sage", &workspace)?;
    write_file(&openclaw.join("MEMORY.md"), b"ancestor secret")?;
    symlink(&actual_parent, &linked_parent)?;

    let linked_root = linked_parent.join("openclaw");
    let output = run_coven(
        temp.path(),
        &[
            "memory",
            "import",
            "--familiar",
            "sage",
            "--source",
            "openclaw",
            "--openclaw-root",
            linked_root.to_str().expect("fixture path is UTF-8"),
        ],
    )?;
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr)?;
    assert!(stderr.contains("real directory"), "{stderr}");
    assert!(!stderr.contains(linked_root.to_str().expect("fixture path is UTF-8")));
    assert!(!stderr.contains("ancestor secret"));
    Ok(())
}

#[cfg(unix)]
#[test]
fn source_boundaries_native_relative_workspace_symlinked_intermediate_fails_closed() -> Result<()> {
    use std::os::unix::fs::symlink;

    let temp = trusted_tempdir()?;
    let actual_parent = temp.path().join("actual-parent");
    let linked_parent = temp.path().join("linked-parent");
    let workspace = actual_parent.join("native-workspace");
    write_file(&workspace.join("MEMORY.md"), b"relative ancestor secret")?;
    symlink(&actual_parent, &linked_parent)?;
    write_familiar(
        temp.path(),
        "sage",
        Path::new("linked-parent/native-workspace"),
    )?;

    let output = run_coven_at(
        temp.path(),
        temp.path(),
        &["memory", "import", "--familiar", "sage"],
    )?;
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr)?;
    assert!(stderr.contains("real directory"), "{stderr}");
    assert!(!stderr.contains("linked-parent"));
    assert!(!stderr.contains("relative ancestor secret"));
    Ok(())
}

fn coven_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_coven"))
}

fn run_coven(coven_home: &Path, args: &[&str]) -> Result<Output> {
    Ok(Command::new(coven_bin())
        .args(args)
        .env("COVEN_HOME", coven_home)
        .output()?)
}

fn run_coven_at(coven_home: &Path, current_dir: &Path, args: &[&str]) -> Result<Output> {
    Ok(Command::new(coven_bin())
        .args(args)
        .env("COVEN_HOME", coven_home)
        .current_dir(current_dir)
        .output()?)
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "command failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn trusted_tempdir() -> Result<tempfile::TempDir> {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let worktree = manifest_dir
        .parent()
        .and_then(Path::parent)
        .expect("coven-cli manifest must be inside the repository");
    let repository = worktree
        .parent()
        .filter(|parent| parent.file_name() == Some(std::ffi::OsStr::new(".worktrees")))
        .and_then(Path::parent)
        .unwrap_or(worktree);
    let test_root = repository.join("target/m");
    fs::create_dir_all(&test_root)?;
    Ok(tempfile::Builder::new().prefix("m").tempdir_in(test_root)?)
}

fn write_familiar(coven_home: &Path, id: &str, workspace: &Path) -> Result<()> {
    write_familiars(coven_home, &[(id, workspace)])
}

fn write_familiars(coven_home: &Path, familiars: &[(&str, &Path)]) -> Result<()> {
    let mut config = String::new();
    for (id, workspace) in familiars {
        let workspace = serde_json::to_string(&workspace.to_string_lossy())?;
        config.push_str(&format!(
            r#"
[[familiar]]
id = "{id}"
display_name = "{id}"
role = "test"
description = "test"
workspace = {workspace}
"#
        ));
    }
    fs::write(coven_home.join("familiars.toml"), config)?;
    Ok(())
}

fn write_file(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, bytes)?;
    Ok(())
}
