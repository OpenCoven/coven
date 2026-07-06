#![cfg(unix)]

use std::ffi::OsString;
use std::fs;
use std::io::{Read, Write};
use std::net::Shutdown;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

#[test]
fn daemon_status_clears_stale_metadata_when_daemon_is_gone() -> anyhow::Result<()> {
    let temp_dir = tempfile::tempdir()?;
    let coven_home = temp_dir.path().join("coven-home");
    fs::create_dir_all(&coven_home)?;
    fs::write(
        coven_home.join("daemon.json"),
        r#"{
  "pid": 999999,
  "startedAt": "2026-01-01T00:00:00Z",
  "socket": "/tmp/does-not-exist.sock"
}
"#,
    )?;

    let output = run_coven(
        &coven_bin(),
        &coven_home,
        &std::env::var_os("PATH").unwrap_or_default(),
        &["daemon", "status"],
    )?;

    assert_success("daemon status with stale metadata", &output);
    assert_stdout_contains(
        "daemon status with stale metadata",
        &output,
        "status=stopped",
    );
    assert!(
        !coven_home.join("daemon.json").exists(),
        "stale daemon metadata should be cleared"
    );
    Ok(())
}

#[test]
fn daemon_status_clears_corrupt_metadata_when_daemon_is_gone() -> anyhow::Result<()> {
    let temp_dir = tempfile::tempdir()?;
    let coven_home = temp_dir.path().join("coven-home");
    fs::create_dir_all(&coven_home)?;
    fs::write(coven_home.join("daemon.json"), "{not json\n")?;

    let output = run_coven(
        &coven_bin(),
        &coven_home,
        &std::env::var_os("PATH").unwrap_or_default(),
        &["daemon", "status"],
    )?;

    assert_success("daemon status with corrupt metadata", &output);
    assert_stdout_contains(
        "daemon status with corrupt metadata",
        &output,
        "status=stopped",
    );
    assert!(
        !coven_home.join("daemon.json").exists(),
        "corrupt daemon metadata should be cleared"
    );
    Ok(())
}

#[test]
fn daemon_status_recovers_corrupt_metadata_from_live_daemon_health() -> anyhow::Result<()> {
    let temp_dir = tempfile::tempdir()?;
    let coven_home = temp_dir.path().join("coven-home");
    let path = std::env::var_os("PATH").unwrap_or_default();
    let coven = coven_bin();
    let _daemon_guard = DaemonGuard {
        coven: coven.clone(),
        coven_home: coven_home.clone(),
        path: path.clone(),
    };

    let start = run_coven(&coven, &coven_home, &path, &["daemon", "start"])?;
    assert_success("daemon start", &start);
    wait_for_daemon_health(&coven_home)?;

    let status_path = coven_home.join("daemon.json");
    let original_status = fs::read_to_string(&status_path)?;
    let _restore_guard = DaemonStatusRestoreGuard {
        path: status_path.clone(),
        contents: original_status,
    };
    fs::write(&status_path, "{not json\n")?;

    let output = run_coven(&coven, &coven_home, &path, &["daemon", "status"])?;

    assert_success("daemon status with live corrupt metadata", &output);
    assert_stdout_contains(
        "daemon status with live corrupt metadata",
        &output,
        "status=running",
    );
    let recovered = fs::read_to_string(&status_path)?;
    let recovered: Value = serde_json::from_str(&recovered)?;
    assert!(
        recovered.get("pid").and_then(Value::as_u64).is_some(),
        "daemon status metadata should be restored from health"
    );
    Ok(())
}

#[test]
fn daemon_start_is_idempotent_when_daemon_is_already_running() -> anyhow::Result<()> {
    let temp_dir = tempfile::tempdir()?;
    let coven_home = temp_dir.path().join("coven-home");
    let path = std::env::var_os("PATH").unwrap_or_default();
    let coven = coven_bin();
    let _daemon_guard = DaemonGuard {
        coven: coven.clone(),
        coven_home: coven_home.clone(),
        path: path.clone(),
    };

    let first = run_coven(&coven, &coven_home, &path, &["daemon", "start"])?;
    assert_success("first daemon start", &first);
    wait_for_daemon_health(&coven_home)?;
    let first_pid = daemon_status_pid(&coven_home)?;

    let second = run_coven(&coven, &coven_home, &path, &["daemon", "start"])?;
    assert_success("second daemon start", &second);
    wait_for_daemon_health(&coven_home)?;
    let second_pid = daemon_status_pid(&coven_home)?;

    assert_eq!(
        second_pid, first_pid,
        "daemon start should reuse the verified running daemon instead of spawning another serve process"
    );
    Ok(())
}

#[test]
fn concurrent_daemon_start_commands_share_one_daemon() -> anyhow::Result<()> {
    let temp_dir = tempfile::tempdir()?;
    let coven_home = temp_dir.path().join("coven-home");
    let path = std::env::var_os("PATH").unwrap_or_default();
    let coven = coven_bin();
    let _daemon_guard = DaemonGuard {
        coven: coven.clone(),
        coven_home: coven_home.clone(),
        path: path.clone(),
    };

    let first = Command::new(&coven)
        .args(["daemon", "start"])
        .env("COVEN_HOME", &coven_home)
        .env("PATH", &path)
        .spawn()?;
    let second = Command::new(&coven)
        .args(["daemon", "start"])
        .env("COVEN_HOME", &coven_home)
        .env("PATH", &path)
        .spawn()?;

    let first = first.wait_with_output()?;
    let second = second.wait_with_output()?;
    assert_success("first concurrent daemon start", &first);
    assert_success("second concurrent daemon start", &second);
    wait_for_daemon_health(&coven_home)?;

    let recovery_log = fs::read_to_string(coven_home.join("daemon-recovery.log"))?;
    let starts = recovery_log.matches("daemon starting pid=").count();
    assert_eq!(
        starts, 1,
        "concurrent daemon start commands should launch exactly one serve process\n{recovery_log}"
    );
    Ok(())
}

#[test]
fn daemon_serve_refuses_to_take_over_a_healthy_socket() -> anyhow::Result<()> {
    let temp_dir = tempfile::tempdir()?;
    let coven_home = temp_dir.path().join("coven-home");
    let path = std::env::var_os("PATH").unwrap_or_default();
    let coven = coven_bin();
    let _daemon_guard = DaemonGuard {
        coven: coven.clone(),
        coven_home: coven_home.clone(),
        path: path.clone(),
    };

    let first = run_coven(&coven, &coven_home, &path, &["daemon", "start"])?;
    assert_success("first daemon start", &first);
    wait_for_daemon_health(&coven_home)?;
    let first_pid = daemon_status_pid(&coven_home)?;

    // A second `daemon serve` against the live socket must refuse to take over.
    // Unlinking the incumbent's socket would not stop it — it would keep running
    // on the orphaned inode — so the duplicate has to exit on its own instead.
    let mut duplicate = Command::new(&coven)
        .args(["daemon", "serve"])
        .env("COVEN_HOME", &coven_home)
        .env("PATH", &path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    wait_until("duplicate daemon to exit on its own", || {
        Ok(duplicate.try_wait()?.is_some())
    })?;
    let duplicate_status = duplicate.wait()?;
    assert!(
        !duplicate_status.success(),
        "duplicate serve should fail rather than take over the live socket"
    );

    // The incumbent is untouched: still the recorded owner, still healthy, and the
    // refused duplicate must not have clobbered daemon.json on its way out.
    wait_for_daemon_health(&coven_home)?;
    assert_eq!(
        first_pid,
        daemon_status_pid(&coven_home)?,
        "incumbent must remain the recorded socket owner after a refused takeover"
    );
    assert!(
        pid_is_alive(first_pid as u32),
        "incumbent daemon must stay alive after a refused takeover"
    );
    Ok(())
}

#[test]
fn doctor_lists_configured_familiars() -> anyhow::Result<()> {
    let temp_dir = tempfile::tempdir()?;
    let coven_home = temp_dir.path().join("coven-home");
    fs::create_dir_all(&coven_home)?;
    fs::write(
        coven_home.join("familiars.toml"),
        r#"
[[familiar]]
id = "charm"
display_name = "Charm"
role = "Voice, Social, and Presence Familiar"
description = "Keeps the coven sociable."
"#,
    )?;
    let path = std::env::var_os("PATH").unwrap_or_default();
    let coven = coven_bin();

    let output = run_coven(&coven, &coven_home, &path, &["doctor"])?;

    assert_success("doctor with familiars", &output);
    assert_stdout_contains("doctor with familiars", &output, "Familiars (");
    assert_stdout_contains("doctor with familiars", &output, "charm");
    assert_stdout_contains(
        "doctor with familiars",
        &output,
        "Voice, Social, and Presence Familiar",
    );
    Ok(())
}

#[test]
fn doctor_reports_no_familiars_when_manifest_absent() -> anyhow::Result<()> {
    let temp_dir = tempfile::tempdir()?;
    let coven_home = temp_dir.path().join("coven-home");
    fs::create_dir_all(&coven_home)?;
    let path = std::env::var_os("PATH").unwrap_or_default();
    let coven = coven_bin();

    let output = run_coven(&coven, &coven_home, &path, &["doctor"])?;

    assert_success("doctor without familiars", &output);
    assert_stdout_contains("doctor without familiars", &output, "none configured");
    assert_stdout_contains("doctor without familiars", &output, "familiars.toml");
    Ok(())
}

#[test]
fn doctor_missing_harness_prints_cross_platform_setup_loop() -> anyhow::Result<()> {
    let temp_dir = tempfile::tempdir()?;
    let coven_home = temp_dir.path().join("coven-home");
    fs::create_dir_all(&coven_home)?;
    let coven = coven_bin();
    let empty_path = OsString::new();

    let output = run_coven(&coven, &coven_home, &empty_path, &["doctor"])?;

    assert_success("doctor without harnesses", &output);
    assert_stdout_contains("doctor without harnesses", &output, "Harnesses:");
    assert_stdout_contains("doctor without harnesses", &output, "`codex` is missing");
    assert_stdout_contains("doctor without harnesses", &output, "`claude` is missing");
    assert_stdout_contains(
        "doctor without harnesses",
        &output,
        "Install and authenticate at least one harness in this same shell.",
    );
    assert_stdout_contains(
        "doctor without harnesses",
        &output,
        "If PATH changed, open a new terminal and run `coven doctor` again.",
    );
    assert_stdout_contains("doctor without harnesses", &output, "coven daemon start");
    assert_stdout_contains(
        "doctor without harnesses",
        &output,
        "Install docs: https://github.com/OpenCoven/coven/blob/main/docs/install/index.md",
    );
    Ok(())
}

#[test]
fn doctor_reports_live_daemon_socket_status() -> anyhow::Result<()> {
    let temp_dir = tempfile::tempdir()?;
    let coven_home = temp_dir.path().join("coven-home");
    let path = std::env::var_os("PATH").unwrap_or_default();
    let coven = coven_bin();
    let _daemon_guard = DaemonGuard {
        coven: coven.clone(),
        coven_home: coven_home.clone(),
        path: path.clone(),
    };

    let start = run_coven(&coven, &coven_home, &path, &["daemon", "start"])?;
    assert_success("daemon start", &start);
    wait_for_daemon_health(&coven_home)?;

    let output = run_coven(&coven, &coven_home, &path, &["doctor"])?;

    assert_success("doctor with live daemon", &output);
    assert_stdout_contains("doctor with live daemon", &output, "Daemon:");
    assert_stdout_contains("doctor with live daemon", &output, "status=running");
    assert_stdout_contains("doctor with live daemon", &output, "socket=");
    Ok(())
}

#[test]
fn adapter_install_hermes_writes_trusted_manifest() -> anyhow::Result<()> {
    let temp_dir = tempfile::tempdir()?;
    let coven_home = temp_dir.path().join("coven-home");
    let path = std::env::var_os("PATH").unwrap_or_default();
    let coven = coven_bin();

    let install = run_coven(
        &coven,
        &coven_home,
        &path,
        &["adapter", "install", "hermes"],
    )?;

    assert_success("adapter install hermes", &install);
    assert_stdout_contains(
        "adapter install hermes",
        &install,
        "Installed adapter `hermes`",
    );
    assert!(coven_home.join("adapters").join("hermes.json").exists());

    let doctor = run_coven(&coven, &coven_home, &path, &["adapter", "doctor", "hermes"])?;

    assert_success("adapter doctor hermes", &doctor);
    assert_stdout_contains("adapter doctor hermes", &doctor, "Hermes Agent");
    assert_stdout_contains("adapter doctor hermes", &doctor, "manifest:");
    Ok(())
}

#[test]
fn adapter_install_hermes_replaces_existing_manifest() -> anyhow::Result<()> {
    let temp_dir = tempfile::tempdir()?;
    let coven_home = temp_dir.path().join("coven-home");
    let adapter_dir = coven_home.join("adapters");
    fs::create_dir_all(&adapter_dir)?;
    let manifest_path = adapter_dir.join("hermes.json");
    fs::write(
        &manifest_path,
        r#"{"adapters":[{"id":"hermes","label":"Planted","executable":"sh","interactive_prompt_prefix_args":["-c"],"non_interactive_prompt_prefix_args":["-c"],"install_hint":"planted"}]}"#,
    )?;
    let path = std::env::var_os("PATH").unwrap_or_default();
    let coven = coven_bin();

    let install = run_coven(
        &coven,
        &coven_home,
        &path,
        &["adapter", "install", "hermes"],
    )?;

    assert_success("adapter install hermes replaces manifest", &install);
    assert_stdout_contains(
        "adapter install hermes replaces manifest",
        &install,
        "Installed adapter `hermes`",
    );
    let manifest = serde_json::from_str::<Value>(&fs::read_to_string(manifest_path)?)?;
    let adapter = manifest
        .get("adapters")
        .and_then(Value::as_array)
        .and_then(|adapters| adapters.first())
        .expect("installed manifest should include one adapter");
    assert_eq!(adapter.get("id").and_then(Value::as_str), Some("hermes"));
    assert_eq!(
        adapter.get("executable").and_then(Value::as_str),
        Some("hermes")
    );
    Ok(())
}

#[test]
fn adapter_install_hermes_replaces_existing_manifest_directory() -> anyhow::Result<()> {
    let temp_dir = tempfile::tempdir()?;
    let coven_home = temp_dir.path().join("coven-home");
    let adapter_dir = coven_home.join("adapters");
    let manifest_path = adapter_dir.join("hermes.json");
    fs::create_dir_all(&manifest_path)?;
    fs::write(manifest_path.join("planted.txt"), "keep install broken")?;
    let path = std::env::var_os("PATH").unwrap_or_default();
    let coven = coven_bin();

    let install = run_coven(
        &coven,
        &coven_home,
        &path,
        &["adapter", "install", "hermes"],
    )?;

    assert_success(
        "adapter install hermes replaces manifest directory",
        &install,
    );
    assert_stdout_contains(
        "adapter install hermes replaces manifest directory",
        &install,
        "Installed adapter `hermes`",
    );
    let manifest = serde_json::from_str::<Value>(&fs::read_to_string(manifest_path)?)?;
    let adapter = manifest
        .get("adapters")
        .and_then(Value::as_array)
        .and_then(|adapters| adapters.first())
        .expect("installed manifest should include one adapter");
    assert_eq!(adapter.get("id").and_then(Value::as_str), Some("hermes"));
    assert_eq!(
        adapter.get("executable").and_then(Value::as_str),
        Some("hermes")
    );
    Ok(())
}

#[test]
fn smoke_daemon_session_replay_and_safe_session_rituals() -> anyhow::Result<()> {
    let temp_dir = tempfile::tempdir()?;
    let coven_home = temp_dir.path().join("coven-home");
    let project_root = temp_dir.path().join("project");
    fs::create_dir_all(&project_root)?;

    let fake_bin = temp_dir.path().join("bin");
    fs::create_dir_all(&fake_bin)?;
    write_fake_codex(&fake_bin)?;
    let path = prepend_path(&fake_bin);
    let coven = coven_bin();
    let _daemon_guard = DaemonGuard {
        coven: coven.clone(),
        coven_home: coven_home.clone(),
        path: path.clone(),
    };

    let start = run_coven(&coven, &coven_home, &path, &["daemon", "start"])?;
    assert_success("daemon start", &start);
    assert_stdout_contains("daemon start", &start, "status=running");

    wait_for_daemon_health(&coven_home)?;

    let status = run_coven(&coven, &coven_home, &path, &["daemon", "status"])?;
    assert_success("daemon status", &status);
    assert_stdout_contains("daemon status", &status, "status=running");

    let replay_session = launch_daemon_session(
        &coven_home,
        &project_root,
        "codex",
        "smoke replay",
        "Smoke replay",
    )?;
    wait_for_session_status(&coven_home, &replay_session, "completed")?;
    wait_for_event_text(
        &coven_home,
        &replay_session,
        "fake codex complete: smoke replay",
    )?;

    let restart = run_coven(&coven, &coven_home, &path, &["daemon", "restart"])?;
    assert_success("daemon restart", &restart);
    assert_stdout_contains("daemon restart", &restart, "status=restarted");
    wait_for_daemon_health(&coven_home)?;

    let restarted_status = run_coven(&coven, &coven_home, &path, &["daemon", "status"])?;
    assert_success("daemon restarted status", &restarted_status);
    assert_stdout_contains(
        "daemon restarted status",
        &restarted_status,
        "status=running",
    );

    let attach = run_coven(&coven, &coven_home, &path, &["attach", &replay_session])?;
    assert_success("attach replay", &attach);
    assert_stdout_contains(
        "attach replay",
        &attach,
        "fake codex complete: smoke replay",
    );
    assert_stdout_contains(
        "attach replay",
        &attach,
        "[coven session completed exitCode=0]",
    );

    let kill_session = launch_daemon_session(
        &coven_home,
        &project_root,
        "codex",
        "hold-for-kill",
        "Smoke kill",
    )?;
    wait_for_event_text(&coven_home, &kill_session, "fake codex ready for kill")?;

    let (kill_status, kill_body) = unix_http_request(
        &coven_home,
        "POST",
        &format!("/sessions/{kill_session}/kill"),
        None,
    )?;
    assert_eq!(kill_status, 202, "unexpected kill response: {kill_body}");
    wait_for_session_status(&coven_home, &kill_session, "killed")?;

    let archive = run_coven(&coven, &coven_home, &path, &["archive", &kill_session])?;
    assert_success("archive", &archive);
    assert_stdout_contains("archive", &archive, "archived session");

    let active_sessions = run_coven(&coven, &coven_home, &path, &["sessions", "--plain"])?;
    assert_success("active sessions", &active_sessions);
    assert_stdout_not_contains("active sessions", &active_sessions, &kill_session);

    let archived_sessions = run_coven(
        &coven,
        &coven_home,
        &path,
        &["sessions", "--all", "--plain"],
    )?;
    assert_success("archived sessions", &archived_sessions);
    assert_stdout_contains("archived sessions", &archived_sessions, &kill_session);
    assert_stdout_contains("archived sessions", &archived_sessions, "archived");

    let summon = run_coven(&coven, &coven_home, &path, &["summon", &kill_session])?;
    assert_success("summon", &summon);

    let restored_sessions = run_coven(&coven, &coven_home, &path, &["sessions", "--plain"])?;
    assert_success("restored sessions", &restored_sessions);
    assert_stdout_contains("restored sessions", &restored_sessions, &kill_session);
    assert_stdout_contains("restored sessions", &restored_sessions, "active");

    let sacrifice = run_coven(
        &coven,
        &coven_home,
        &path,
        &["sacrifice", &kill_session, "--yes"],
    )?;
    assert_success("sacrifice", &sacrifice);
    assert_stdout_contains("sacrifice", &sacrifice, "sacrificed session");

    let all_sessions = run_coven(
        &coven,
        &coven_home,
        &path,
        &["sessions", "--all", "--plain"],
    )?;
    assert_success("all sessions after sacrifice", &all_sessions);
    assert_stdout_not_contains("all sessions after sacrifice", &all_sessions, &kill_session);

    let stop = run_coven(&coven, &coven_home, &path, &["daemon", "stop"])?;
    assert_success("daemon stop", &stop);
    assert_stdout_contains("daemon stop", &stop, "status=stopped");

    let stopped = run_coven(&coven, &coven_home, &path, &["daemon", "status"])?;
    assert_success("daemon stopped status", &stopped);
    assert_stdout_contains("daemon stopped status", &stopped, "status=stopped");

    Ok(())
}

struct DaemonGuard {
    coven: PathBuf,
    coven_home: PathBuf,
    path: OsString,
}

impl Drop for DaemonGuard {
    fn drop(&mut self) {
        let _ = run_coven(
            &self.coven,
            &self.coven_home,
            &self.path,
            &["daemon", "stop"],
        );
    }
}

struct DaemonStatusRestoreGuard {
    path: PathBuf,
    contents: String,
}

impl Drop for DaemonStatusRestoreGuard {
    fn drop(&mut self) {
        let _ = fs::write(&self.path, &self.contents);
    }
}

fn coven_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_coven"))
}

fn run_coven(
    coven: &Path,
    coven_home: &Path,
    path: &OsString,
    args: &[&str],
) -> anyhow::Result<Output> {
    Command::new(coven)
        .args(args)
        .env("COVEN_HOME", coven_home)
        .env("PATH", path)
        .output()
        .map_err(Into::into)
}

fn assert_success(label: &str, output: &Output) {
    assert!(
        output.status.success(),
        "{label} failed\nstatus: {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn assert_stdout_contains(label: &str, output: &Output, needle: &str) {
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains(needle),
        "{label} stdout did not contain {needle:?}\nstdout:\n{stdout}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn assert_stdout_not_contains(label: &str, output: &Output, needle: &str) {
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.contains(needle),
        "{label} stdout unexpectedly contained {needle:?}\nstdout:\n{stdout}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn prepend_path(fake_bin: &Path) -> OsString {
    let mut paths = vec![fake_bin.to_path_buf()];
    if let Some(existing) = std::env::var_os("PATH") {
        paths.extend(std::env::split_paths(&existing));
    }
    std::env::join_paths(paths).expect("test PATH should be joinable")
}

fn write_fake_codex(fake_bin: &Path) -> anyhow::Result<()> {
    let codex = fake_bin.join("codex");
    fs::write(
        &codex,
        r#"#!/bin/sh
if [ "$*" = "hold-for-kill" ]; then
  printf 'fake codex ready for kill\n'
  exec sleep 300
fi
printf 'fake codex complete: %s\n' "$*"
"#,
    )?;
    let mut permissions = fs::metadata(&codex)?.permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&codex, permissions)?;
    Ok(())
}

fn wait_for_daemon_health(coven_home: &Path) -> anyhow::Result<()> {
    wait_until("daemon health", || {
        let socket = coven_home.join("coven.sock");
        if !socket.exists() {
            return Ok(false);
        }
        let (status, body) = unix_http_request(coven_home, "GET", "/health", None)?;
        Ok(status == 200 && body.contains(r#""ok":true"#))
    })
}

fn daemon_status_pid(coven_home: &Path) -> anyhow::Result<u64> {
    let status = fs::read_to_string(coven_home.join("daemon.json"))?;
    let status = serde_json::from_str::<Value>(&status)?;
    status
        .get("pid")
        .and_then(Value::as_u64)
        .ok_or_else(|| anyhow::anyhow!("daemon status should include pid"))
}

fn pid_is_alive(pid: u32) -> bool {
    Command::new("kill")
        .arg("-0")
        .arg(pid.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn launch_daemon_session(
    coven_home: &Path,
    project_root: &Path,
    harness: &str,
    prompt: &str,
    title: &str,
) -> anyhow::Result<String> {
    let body = json!({
        "projectRoot": project_root,
        "harness": harness,
        "prompt": prompt,
        "title": title
    })
    .to_string();
    let (status, response_body) = unix_http_request(coven_home, "POST", "/sessions", Some(&body))?;
    assert_eq!(
        status, 201,
        "unexpected session launch response: {response_body}"
    );
    Ok(serde_json::from_str::<Value>(&response_body)?
        .get("id")
        .and_then(Value::as_str)
        .expect("daemon response should include session id")
        .to_string())
}

fn wait_for_session_status(
    coven_home: &Path,
    session_id: &str,
    expected_status: &str,
) -> anyhow::Result<()> {
    wait_until(
        &format!("session {session_id} status {expected_status}"),
        || {
            let (_status, body) =
                unix_http_request(coven_home, "GET", &format!("/sessions/{session_id}"), None)?;
            let body = serde_json::from_str::<Value>(&body)?;
            Ok(body
                .get("status")
                .and_then(Value::as_str)
                .is_some_and(|status| status == expected_status))
        },
    )
}

fn wait_for_event_text(coven_home: &Path, session_id: &str, needle: &str) -> anyhow::Result<()> {
    wait_until(&format!("session {session_id} event {needle:?}"), || {
        let (_status, body) = unix_http_request(
            coven_home,
            "GET",
            &format!("/events?sessionId={session_id}"),
            None,
        )?;
        Ok(body.contains(needle))
    })
}

fn wait_until(
    label: &str,
    mut predicate: impl FnMut() -> anyhow::Result<bool>,
) -> anyhow::Result<()> {
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut last_error = None;
    while Instant::now() < deadline {
        match predicate() {
            Ok(true) => return Ok(()),
            Ok(false) => {}
            Err(error) => last_error = Some(error),
        }
        thread::sleep(Duration::from_millis(100));
    }

    if let Some(error) = last_error {
        anyhow::bail!("timed out waiting for {label}; last error: {error}");
    }
    anyhow::bail!("timed out waiting for {label}")
}

fn unix_http_request(
    coven_home: &Path,
    method: &str,
    path: &str,
    body: Option<&str>,
) -> anyhow::Result<(u16, String)> {
    let body = body.unwrap_or_default();
    let request = format!(
        "{method} {path} HTTP/1.1\r\nHost: coven\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    let mut stream = UnixStream::connect(coven_home.join("coven.sock"))?;
    stream.write_all(request.as_bytes())?;
    stream.shutdown(Shutdown::Write)?;

    let mut response = String::new();
    stream.read_to_string(&mut response)?;
    let status = response
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|status| status.parse::<u16>().ok())
        .ok_or_else(|| anyhow::anyhow!("invalid HTTP response: {response}"))?;
    let body = response
        .split_once("\r\n\r\n")
        .map(|(_, body)| body.to_string())
        .unwrap_or_default();
    Ok((status, body))
}
