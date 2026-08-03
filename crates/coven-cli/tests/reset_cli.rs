#![cfg(windows)]

use std::fs;
use std::process::Command;

#[test]
fn windows_apply_is_rejected_before_state_changes() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let home = temp.path().join("coven-home");
    fs::create_dir_all(&home)?;
    fs::write(home.join("repos.toml"), "project")?;

    let output = Command::new(env!("CARGO_BIN_EXE_coven"))
        .env("COVEN_HOME", &home)
        .args(["reset", "--feature", "projects", "--apply"])
        .output()?;

    assert_eq!(output.status.code(), Some(2));
    assert_eq!(fs::read_to_string(home.join("repos.toml"))?, "project");
    assert!(!home.join("reset-backups").exists());
    assert!(!home.join("reset-transaction.json").exists());
    assert!(!home.join("state.lock").exists());
    assert!(String::from_utf8_lossy(&output.stderr).contains("unavailable on Windows"));
    Ok(())
}
