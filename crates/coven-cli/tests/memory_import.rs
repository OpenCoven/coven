use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use anyhow::Result;
use serde_json::Value;

#[test]
fn source_boundaries_native_preview_reports_only_sorted_logical_labels() -> Result<()> {
    let temp = tempfile::tempdir()?;
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
    assert_eq!(report["status"], "discovered");
    assert_eq!(report["familiar_id"], "sage");
    assert_eq!(report["source_kind"], "native");
    assert_eq!(
        report["entries"],
        serde_json::json!([
            {"source_label": "MEMORY.md"},
            {"source_label": "memory/a.md"},
            {"source_label": "notes/z.md"}
        ])
    );
    let rendered = String::from_utf8(output.stdout)?;
    assert!(!rendered.contains("root secret"));
    assert!(!rendered.contains("excluded sentinel"));
    assert!(!rendered.contains(&workspace.to_string_lossy().into_owned()));
    assert!(!temp.path().join("memory").exists());

    let human = run_coven(temp.path(), &["memory", "import", "--familiar", "sage"])?;
    assert_success(&human);
    let human = String::from_utf8(human.stdout)?;
    assert!(human.contains("Discovery only"), "{human}");
    assert!(human.contains("memory/a.md"), "{human}");
    assert!(!human.contains("root secret"), "{human}");
    assert!(!human.contains(&workspace.to_string_lossy().into_owned()));
    Ok(())
}

#[test]
fn source_boundaries_openclaw_preview_uses_explicit_root_and_target() -> Result<()> {
    let temp = tempfile::tempdir()?;
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
    assert_eq!(report["status"], "discovered");
    assert_eq!(report["familiar_id"], "sage");
    assert_eq!(report["source_kind"], "openclaw");
    assert_eq!(
        report["entries"],
        serde_json::json!([
            {"source_label": "DREAMS.md"},
            {"source_label": "MEMORY.md"},
            {"source_label": "memory/topic.md"}
        ])
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
    Ok(())
}

#[test]
fn source_boundaries_unknown_familiar_fails_before_source_root_access() -> Result<()> {
    let temp = tempfile::tempdir()?;
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
    let temp = tempfile::tempdir()?;
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

    let temp = tempfile::tempdir()?;
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

fn coven_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_coven"))
}

fn run_coven(coven_home: &Path, args: &[&str]) -> Result<Output> {
    Ok(Command::new(coven_bin())
        .args(args)
        .env("COVEN_HOME", coven_home)
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

fn write_familiar(coven_home: &Path, id: &str, workspace: &Path) -> Result<()> {
    let workspace = serde_json::to_string(&workspace.to_string_lossy())?;
    fs::write(
        coven_home.join("familiars.toml"),
        format!(
            r#"
[[familiar]]
id = "{id}"
display_name = "{id}"
role = "test"
description = "test"
workspace = {workspace}
"#
        ),
    )?;
    Ok(())
}

fn write_file(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, bytes)?;
    Ok(())
}
