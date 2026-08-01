use std::collections::BTreeSet;
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
        .env("PATH", OsString::new())
        .env("COVEN_ENGINE_BIN", root.join("missing-engine"))
        .env_remove("COVEN_HARNESS_ADAPTER_DIRS")
        .env_remove("COVEN_SETTINGS_PATH");
    command
}

fn assert_single_json_document(output: &Output) -> anyhow::Result<Value> {
    let stdout = std::str::from_utf8(&output.stdout)?;
    anyhow::ensure!(
        output.stderr.is_empty(),
        "doctor --json must keep stderr empty; got: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_str(stdout).map_err(Into::into)
}

#[test]
fn doctor_json_is_stable_redacted_and_stdout_pure() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let project = temp.path().join("private-project-name");
    let coven_home = temp.path().join("private-coven-home");
    fs::create_dir_all(project.join(".git"))?;
    fs::create_dir_all(&coven_home)?;
    fs::create_dir_all(temp.path().join("user-home"))?;

    let first = isolated_doctor_command(
        temp.path(),
        &project,
        &coven_home,
        &["--color", "always", "doctor", "--json"],
    )
    .output()?;
    assert_eq!(first.status.code(), Some(1));
    let first_json = assert_single_json_document(&first)?;

    let keys: BTreeSet<_> = first_json
        .as_object()
        .expect("doctor output must be an object")
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        BTreeSet::from(["blocking", "checks", "nextSteps", "ok", "project", "store"])
    );
    assert_eq!(first_json["ok"], false);
    assert_eq!(first_json["blocking"], true);
    assert_eq!(first_json["store"], "<coven-home>");
    assert_eq!(first_json["project"], "<project>");

    let checks = first_json["checks"].as_array().expect("checks array");
    assert!(!checks.is_empty());
    for check in checks {
        let check = check.as_object().expect("check object");
        assert!(check.get("id").is_some_and(Value::is_string));
        assert!(check.get("message").is_some_and(Value::is_string));
        assert!(check
            .get("status")
            .and_then(Value::as_str)
            .is_some_and(|status| matches!(status, "pass" | "warn" | "fail")));
        assert!(check.get("hint").is_none_or(Value::is_string));
    }
    assert!(first_json["nextSteps"]
        .as_array()
        .is_some_and(|steps| !steps.is_empty() && steps.iter().all(Value::is_string)));

    let stdout = String::from_utf8(first.stdout)?;
    assert!(
        !stdout.contains('\u{1b}'),
        "JSON must never contain ANSI escapes"
    );
    assert!(!stdout.contains(&temp.path().display().to_string()));
    assert!(!stdout.contains("private-project-name"));
    assert!(!stdout.contains("private-coven-home"));

    let second = isolated_doctor_command(temp.path(), &project, &coven_home, &["doctor", "--json"])
        .output()?;
    assert_eq!(second.status.code(), Some(1));
    assert_eq!(assert_single_json_document(&second)?, first_json);
    Ok(())
}

#[test]
fn doctor_json_rejects_trailing_arguments_without_stdout() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let project = temp.path().join("project");
    let coven_home = temp.path().join("coven-home");
    fs::create_dir_all(project.join(".git"))?;
    fs::create_dir_all(&coven_home)?;
    fs::create_dir_all(temp.path().join("user-home"))?;

    let output = isolated_doctor_command(
        temp.path(),
        &project,
        &coven_home,
        &["doctor", "--json", "unexpected"],
    )
    .output()?;
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("unexpected argument"),
        "clap should explain the invalid argument: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}
