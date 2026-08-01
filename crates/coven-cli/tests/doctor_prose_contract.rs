use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;

fn coven_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_coven"))
}

fn isolated_doctor_command(
    root: &Path,
    project: &Path,
    coven_home: &Path,
    args: &[&str],
) -> Command {
    let fake_home = root.join("user-home");
    let mut command = Command::new(coven_bin());
    command
        .args(args)
        .current_dir(project)
        .env("COVEN_HOME", coven_home)
        .env("HOME", &fake_home)
        .env("USERPROFILE", &fake_home)
        .env("XDG_CONFIG_HOME", fake_home.join("config"))
        .env("PATH", OsString::new())
        .env("COVEN_ENGINE_BIN", root.join("missing-engine"))
        .env_remove("COVEN_HARNESS_ADAPTER_DIRS")
        .env_remove("COVEN_HARNESS_ADAPTER_MANIFEST")
        .env_remove("COVEN_SETTINGS_PATH");
    command
}

fn check_status<'a>(json: &'a Value, id: &str) -> Option<&'a str> {
    json["checks"]
        .as_array()?
        .iter()
        .find(|check| check["id"] == id)?["status"]
        .as_str()
}

fn assert_blocking_exit(output: &Output) {
    assert_eq!(output.status.code(), Some(1));
    assert!(
        output.stderr.is_empty(),
        "Doctor should keep diagnostics on stdout: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn doctor_prose_markers_match_json_severity_and_are_ansi_free() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let project = temp.path().join("project");
    let coven_home = temp.path().join("coven-home");
    fs::create_dir_all(project.join(".git"))?;
    fs::create_dir_all(&coven_home)?;
    fs::create_dir_all(temp.path().join("user-home"))?;
    fs::write(coven_home.join("familiars.toml"), "[[familiar]\n")?;

    let prose = isolated_doctor_command(
        temp.path(),
        &project,
        &coven_home,
        &["--color=always", "doctor"],
    )
    .output()?;
    assert_blocking_exit(&prose);
    let prose_text = String::from_utf8(prose.stdout)?;
    assert!(prose_text.contains("[--] Not running — run: coven daemon start"));
    assert!(prose_text.contains("[--] Codex"));
    assert!(prose_text.contains("[!!] No supported harness is available"));
    assert!(prose_text.contains("[!!] the Coven engine is missing"));
    assert!(prose_text.contains("[--] could not read"));
    assert!(prose_text.contains("fix access to or contents of"));
    assert!(
        !prose_text.contains('\u{1b}'),
        "Doctor prose must remain ANSI-free even when color is forced"
    );

    let json_output =
        isolated_doctor_command(temp.path(), &project, &coven_home, &["doctor", "--json"])
            .output()?;
    assert_blocking_exit(&json_output);
    let json: Value = serde_json::from_slice(&json_output.stdout)?;
    assert_eq!(check_status(&json, "daemon"), Some("warn"));
    assert_eq!(check_status(&json, "harness:codex"), Some("warn"));
    assert_eq!(check_status(&json, "harnesses"), Some("fail"));
    assert_eq!(check_status(&json, "engine"), Some("fail"));
    assert_eq!(check_status(&json, "familiars"), Some("warn"));

    let repeated = isolated_doctor_command(
        temp.path(),
        &project,
        &coven_home,
        &["--color=always", "doctor"],
    )
    .output()?;
    assert_blocking_exit(&repeated);
    assert_eq!(repeated.stdout, prose_text.as_bytes());
    Ok(())
}
