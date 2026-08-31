#![cfg(unix)]

use std::ffi::OsString;
use std::fs;
use std::io::{Read, Write};
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Context;
use serde_json::Value;

const READY_TIMEOUT: Duration = Duration::from_secs(10);
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// First executable slice of the cross-repository Threads E2E contract.
///
/// This test deliberately crosses the shipped process and versioned HTTP
/// boundary. It must never be replaced with a direct call to
/// `threads_gate::gate_protected_edits`, because that would stop before daemon
/// lifecycle, route/version authority, persistence initialization, and the
/// actual client transport.
#[test]
fn real_daemon_exposes_threads_contract_in_isolated_authority_home() -> anyhow::Result<()> {
    let temp_dir = tempfile::tempdir()?;
    let coven_home = temp_dir.path().join("coven-home");
    let path = std::env::var_os("PATH").unwrap_or_default();
    let coven = coven_bin();
    let daemon = DaemonGuard {
        coven: coven.clone(),
        coven_home: coven_home.clone(),
        path: path.clone(),
    };

    let start = run_coven(&coven, &coven_home, &path, &["daemon", "start"])?;
    assert_success("daemon start", &start);

    let health = wait_for_json(&coven_home, "/api/v1/health")?;
    anyhow::ensure!(
        health.is_object(),
        "versioned daemon health must be a JSON object, got {health}"
    );

    for route in [
        "/api/v1/threads/weaves",
        "/api/v1/threads/proposals",
    ] {
        let response = wait_for_json(&coven_home, route)
            .with_context(|| format!("real daemon did not expose Threads route {route}"))?;
        anyhow::ensure!(
            response.is_array() || response.is_object(),
            "Threads route {route} must return a JSON collection/envelope, got {response}"
        );
    }

    wait_until("daemon-owned SQLite state", || {
        Ok(coven_home.join("coven.sqlite3").is_file())
    })?;

    let canonical_home = fs::canonicalize(&coven_home)?;
    let database = fs::canonicalize(coven_home.join("coven.sqlite3"))?;
    anyhow::ensure!(
        database.starts_with(&canonical_home),
        "daemon database escaped isolated COVEN_HOME: {}",
        database.display()
    );
    anyhow::ensure!(
        coven_home.join("coven.sock").is_file(),
        "real daemon Unix socket was not created inside isolated COVEN_HOME"
    );
    anyhow::ensure!(
        coven_home.join("daemon.json").is_file(),
        "real daemon identity metadata was not created inside isolated COVEN_HOME"
    );

    let stop = daemon.stop()?;
    assert_success("daemon stop", &stop);
    anyhow::ensure!(
        !coven_home.join("coven.sock").exists(),
        "daemon stop must remove the isolated Unix socket"
    );
    anyhow::ensure!(
        !coven_home.join("daemon.json").exists(),
        "daemon stop must remove isolated daemon identity metadata"
    );

    Ok(())
}

struct DaemonGuard {
    coven: PathBuf,
    coven_home: PathBuf,
    path: OsString,
}

impl DaemonGuard {
    fn stop(&self) -> anyhow::Result<Output> {
        run_coven(
            &self.coven,
            &self.coven_home,
            &self.path,
            &["daemon", "stop"],
        )
    }
}

impl Drop for DaemonGuard {
    fn drop(&mut self) {
        let _ = self.stop();
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

fn wait_for_json(coven_home: &Path, route: &str) -> anyhow::Result<Value> {
    let deadline = Instant::now() + READY_TIMEOUT;
    let mut last_observation = String::new();

    while Instant::now() < deadline {
        match unix_http_request(coven_home, "GET", route, None) {
            Ok((200, body)) => match serde_json::from_str::<Value>(&body) {
                Ok(value) => return Ok(value),
                Err(error) => {
                    last_observation = format!("200 with invalid JSON: {error}; body={body:?}");
                }
            },
            Ok((status, body)) => {
                last_observation = format!("HTTP {status}; body={body:?}");
            }
            Err(error) => {
                last_observation = error.to_string();
            }
        }
        thread::sleep(POLL_INTERVAL);
    }

    anyhow::bail!(
        "timed out waiting for {route} through real daemon transport; last observation: {last_observation}"
    )
}

fn wait_until(
    label: &str,
    mut predicate: impl FnMut() -> anyhow::Result<bool>,
) -> anyhow::Result<()> {
    let deadline = Instant::now() + READY_TIMEOUT;
    let mut last_error = None;

    while Instant::now() < deadline {
        match predicate() {
            Ok(true) => return Ok(()),
            Ok(false) => {}
            Err(error) => last_error = Some(error),
        }
        thread::sleep(POLL_INTERVAL);
    }

    if let Some(error) = last_error {
        anyhow::bail!("timed out waiting for {label}; last error: {error}");
    }
    anyhow::bail!("timed out waiting for {label}")
}

fn unix_http_request(
    coven_home: &Path,
    method: &str,
    route: &str,
    body: Option<&str>,
) -> anyhow::Result<(u16, String)> {
    let body = body.unwrap_or_default();
    let request = format!(
        "{method} {route} HTTP/1.1\r\nHost: coven\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
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
