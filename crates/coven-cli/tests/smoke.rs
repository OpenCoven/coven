#![cfg(unix)]

use std::ffi::OsString;
use std::fs;
use std::io::{Read, Write};
use std::net::Shutdown;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

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
    assert_stdout_not_contains("active sessions", &active_sessions, &kill_session[..12]);

    let archived_sessions = run_coven(
        &coven,
        &coven_home,
        &path,
        &["sessions", "--all", "--plain"],
    )?;
    assert_success("archived sessions", &archived_sessions);
    assert_stdout_contains("archived sessions", &archived_sessions, &kill_session[..12]);
    assert_stdout_contains("archived sessions", &archived_sessions, "archived");

    let summon = run_coven(&coven, &coven_home, &path, &["summon", &kill_session])?;
    assert_success("summon", &summon);

    let restored_sessions = run_coven(&coven, &coven_home, &path, &["sessions", "--plain"])?;
    assert_success("restored sessions", &restored_sessions);
    assert_stdout_contains("restored sessions", &restored_sessions, &kill_session[..12]);
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
    assert_stdout_not_contains(
        "all sessions after sacrifice",
        &all_sessions,
        &kill_session[..12],
    );

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
