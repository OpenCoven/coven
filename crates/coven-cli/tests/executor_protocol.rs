//! Integration coverage for the stateless executor protocol
//! (`coven.executor.v1`): the real `coven` binary must answer hub-issued
//! `executor probe` and `executor run-job` invocations with the shared
//! envelopes, for both stationary and compute roles.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use serde_json::{json, Value};

fn coven_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_coven"))
}

fn run_probe(coven_home: &Path) -> anyhow::Result<Output> {
    Command::new(coven_bin())
        .args(["executor", "probe"])
        .env("COVEN_HOME", coven_home)
        .output()
        .map_err(Into::into)
}

fn run_job(coven_home: &Path, job: &Value) -> anyhow::Result<Value> {
    let mut child = Command::new(coven_bin())
        .args(["executor", "run-job"])
        .env("COVEN_HOME", coven_home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    child
        .stdin
        .take()
        .expect("stdin is piped")
        .write_all(job.to_string().as_bytes())?;
    let output = child.wait_with_output()?;
    assert!(
        output.status.success(),
        "executor run-job failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(serde_json::from_slice(&output.stdout)?)
}

fn shell_command(script: &str) -> Value {
    #[cfg(unix)]
    {
        json!(["/bin/sh", "-c", script])
    }
    #[cfg(windows)]
    {
        json!(["cmd", "/C", script])
    }
}

#[test]
fn executor_probe_defaults_to_stationary_availability_envelope() -> anyhow::Result<()> {
    let temp_dir = tempfile::tempdir()?;

    let output = run_probe(temp_dir.path())?;

    assert!(output.status.success());
    let probe: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(probe["protocolVersion"], "coven.executor.v1");
    assert_eq!(probe["role"], "stationary_executor");
    assert_eq!(probe["capabilities"], json!(["shell"]));
    assert_eq!(probe["available"], true);
    assert_eq!(probe["queuePressure"], 0);
    assert!(probe["covenVersion"].as_str().is_some());
    assert!(probe["probedAt"].as_str().unwrap().contains('T'));
    Ok(())
}

#[test]
fn executor_probe_advertises_configured_compute_capabilities() -> anyhow::Result<()> {
    let temp_dir = tempfile::tempdir()?;
    std::fs::write(
        temp_dir.path().join("executor.json"),
        r#"{"role":"compute_executor","capabilities":["shell","gpu","long-running-loop"]}"#,
    )?;

    let output = run_probe(temp_dir.path())?;

    assert!(output.status.success());
    let probe: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(probe["role"], "compute_executor");
    assert_eq!(
        probe["capabilities"],
        json!(["shell", "gpu", "long-running-loop"])
    );
    Ok(())
}

#[test]
fn executor_run_job_returns_normalized_result_envelope() -> anyhow::Result<()> {
    let temp_dir = tempfile::tempdir()?;
    let job = json!({
        "protocolVersion": "coven.executor.v1",
        "jobId": "job_integration",
        "hubId": "hub_integration",
        "command": shell_command("echo dispatched-by-the-hub"),
        "timeoutSeconds": 60
    });

    let envelope = run_job(temp_dir.path(), &job)?;

    assert_eq!(envelope["protocolVersion"], "coven.executor.v1");
    assert_eq!(envelope["jobId"], "job_integration");
    assert_eq!(envelope["status"], "completed");
    assert_eq!(envelope["exitCode"], 0);
    assert!(envelope["stdout"]
        .as_str()
        .unwrap()
        .contains("dispatched-by-the-hub"));
    assert_eq!(envelope["stderr"], "");
    assert!(envelope["startedAt"].as_str().unwrap().contains('T'));
    assert!(envelope["finishedAt"].as_str().unwrap().contains('T'));
    assert!(envelope["durationMs"].as_i64().is_some());
    Ok(())
}

#[test]
fn executor_run_job_rejects_unknown_protocol_versions() -> anyhow::Result<()> {
    let temp_dir = tempfile::tempdir()?;
    let job = json!({
        "protocolVersion": "coven.executor.v99",
        "jobId": "job_future",
        "command": shell_command("echo never-runs")
    });

    let envelope = run_job(temp_dir.path(), &job)?;

    assert_eq!(envelope["status"], "rejected");
    assert!(envelope["error"]
        .as_str()
        .unwrap()
        .contains("protocol version mismatch"));
    Ok(())
}
