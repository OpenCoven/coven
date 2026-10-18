#![cfg(any(unix, windows))]

use std::process::{Command, Stdio};

#[test]
fn leases_missing_daemon_never_creates_home() -> anyhow::Result<()> {
    let root = tempfile::tempdir()?;
    let home = root.path().join("must-not-be-created");
    let output = Command::new(env!("CARGO_BIN_EXE_coven"))
        .args(["automations", "leases", "--json"])
        .env("COVEN_HOME", &home)
        .env("HOME", root.path())
        .stdin(Stdio::null())
        .output()?;
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("owner-local Coven daemon unavailable")
    );
    assert!(
        !home.exists(),
        "inspection created a store or started a daemon"
    );
    Ok(())
}

#[test]
fn leases_stale_discovery_does_not_echo_metadata_or_open_a_store() -> anyhow::Result<()> {
    let root = tempfile::tempdir()?;
    let home = root.path().join("home");
    std::fs::create_dir(&home)?;
    let marker = "private-stale-daemon-marker";
    std::fs::write(
        home.join("daemon.json"),
        format!("{{\"private\":\"{marker}\"}}"),
    )?;
    let output = Command::new(env!("CARGO_BIN_EXE_coven"))
        .args(["automations", "leases", "--json"])
        .env("COVEN_HOME", &home)
        .env("HOME", root.path())
        .env_remove("COVEN_MAINTENANCE_PARTICIPANT")
        .stdin(Stdio::null())
        .output()?;
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(error.contains("owner-local Coven daemon unavailable"));
    assert!(!error.contains(marker));
    assert!(!home.join("coven.sqlite3").exists());
    assert_eq!(std::fs::read_dir(&home)?.count(), 1);
    Ok(())
}
