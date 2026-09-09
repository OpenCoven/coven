#![cfg(unix)]

//! Advisory real-daemon smoke coverage for the first three Threads journeys,
//! plus same-home daemon lifecycle scaffolding for future restart/replay work.
//! Full closure still requires the remaining journeys and their stronger
//! authentication, terminal-audit, deterministic-time, and restart assertions.

use std::ffi::OsString;
use std::fs;
use std::io::{Read, Write};
use std::net::Shutdown;
use std::os::unix::{
    ffi::OsStrExt,
    net::{UnixListener, UnixStream},
};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use rusqlite::Connection;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use uuid::Uuid;

const FAMILIAR_ID: &str = "sage";
const PRINCIPAL_FINGERPRINT: &str = "fpr-e2e-synthetic";
const REQUIRE_OVERRIDE_ENV: &str = "COVEN_THREADS_E2E_REQUIRE_LOCAL_OVERRIDE";

#[test]
fn smoke_bounded_ward_apply_over_real_daemon() -> Result<()> {
    run_journey("smoke-bounded-ward-apply", |fixture| {
        let request = json!({
            "edits": [{
                "target": "notes/today.md",
                "contents": "hello from the real daemon"
            }],
            "principalKeyFingerprint": PRINCIPAL_FINGERPRINT
        });
        let response = fixture.request("POST", "/api/v1/familiars/sage/edits", Some(&request))?;

        anyhow::ensure!(response.status == 200, "unexpected response: {response:?}");
        anyhow::ensure!(
            response.body["disposition"] == "applied",
            "write was not applied: {}",
            response.body
        );
        anyhow::ensure!(
            response.body["threadsGate"]["outcome"]["kind"] == "permitted",
            "Threads did not permit the write: {}",
            response.body
        );
        anyhow::ensure!(
            fs::read_to_string(fixture.workspace.join("notes/today.md"))?
                == "hello from the real daemon",
            "governed bytes do not match the applied request"
        );
        anyhow::ensure!(
            !fixture.coven_home.join("pending").exists(),
            "an applied write left pending proposal state"
        );

        let conn = fixture.store()?;
        let apply_rows: i64 = conn.query_row(
            "SELECT COUNT(*) FROM ward_audit
             WHERE familiar_id = ?1 AND event_type = 'apply_audit'",
            [FAMILIAR_ID],
            |row| row.get(0),
        )?;
        anyhow::ensure!(
            apply_rows == 1,
            "expected one apply audit, got {apply_rows}"
        );

        let audit = &response.body["changes"][0]["audit"];
        anyhow::ensure!(audit["nextSha256"].is_string(), "missing next hash");
        anyhow::ensure!(
            audit["bytesWritten"] == "hello from the real daemon".len(),
            "unexpected applied byte count"
        );
        Ok(())
    })
}

#[test]
fn smoke_unsigned_protected_rejection_over_real_daemon() -> Result<()> {
    run_journey("smoke-unsigned-protected-rejection", |fixture| {
        let request = json!({
            "edits": [{
                "target": "SOUL.md",
                "contents": "# Replaced identity\n"
            }]
        });
        let response = fixture.request("POST", "/api/v1/familiars/sage/edits", Some(&request))?;

        anyhow::ensure!(response.status == 403, "unexpected response: {response:?}");
        anyhow::ensure!(
            response.body["error"]["code"] == "protected_proposal_forbidden",
            "unexpected refusal: {}",
            response.body
        );
        anyhow::ensure!(
            fs::read_to_string(fixture.workspace.join("SOUL.md"))? == "# Sage\n",
            "unsigned request changed protected bytes"
        );
        anyhow::ensure!(
            !fixture.coven_home.join("pending").exists(),
            "unsigned protected request entered the proposal route"
        );

        let conn = fixture.store()?;
        let apply_rows: i64 = conn.query_row(
            "SELECT COUNT(*) FROM ward_audit
             WHERE familiar_id = ?1 AND event_type = 'apply_audit'",
            [FAMILIAR_ID],
            |row| row.get(0),
        )?;
        anyhow::ensure!(apply_rows == 0, "refused request appended an apply audit");
        Ok(())
    })
}

#[test]
fn protected_proposal_routes_never_gain_write_authority() -> Result<()> {
    run_journey("protected-proposal-route-prohibition", |fixture| {
        let contents = "# Synthetic forbidden replacement\n";
        for fingerprint in [json!(PRINCIPAL_FINGERPRINT), Value::Null] {
            let request = json!({
                "edits": [{"target": "SOUL.md", "contents": contents}],
                "principalKeyFingerprint": fingerprint,
                "approvalId": Uuid::new_v4().to_string(),
            });
            let response =
                fixture.request("POST", "/api/v1/familiars/sage/edits", Some(&request))?;
            anyhow::ensure!(
                response.status == 403
                    && response.body["error"]["code"] == "protected_proposal_forbidden",
                "protected proposal endpoint accepted a claimed authority: {response:?}"
            );
            anyhow::ensure!(
                !response.body.to_string().contains(contents),
                "protected rejection echoed proposed content"
            );
        }
        let proposals = fixture.request("GET", "/api/v1/threads/proposals", None)?;
        anyhow::ensure!(
            proposals.status == 200 && proposals.body["proposals"] == json!([]),
            "protected intake created proposal authority: {proposals:?}"
        );
        let invented_id = Uuid::new_v4();
        let approval = fixture.request(
            "POST",
            &format!("/api/v1/threads/proposals/{invented_id}/approve"),
            Some(&json!({"principalKeyFingerprint": PRINCIPAL_FINGERPRINT})),
        )?;
        anyhow::ensure!(
            approval.status == 404,
            "invented approval id was not refused: {approval:?}"
        );
        fixture.restart_daemon()?;
        anyhow::ensure!(
            fs::read_to_string(fixture.workspace.join("SOUL.md"))? == "# Sage\n",
            "a proposal or restart changed protected bytes"
        );
        let conn = fixture.store()?;
        let applied: i64 = conn.query_row(
            "SELECT COUNT(*) FROM ward_audit
             WHERE event_type IN ('apply_audit', 'proposal_approved')",
            [],
            |row| row.get(0),
        )?;
        anyhow::ensure!(applied == 0, "forbidden proposal produced apply evidence");
        Ok(())
    })
}

#[test]
fn protected_rejection_has_durable_non_authorizing_audit() -> Result<()> {
    run_journey("protected-rejection-audit", |fixture| {
        let response = fixture.request(
            "POST",
            "/api/v1/familiars/sage/edits",
            Some(&json!({"edits": [{"target": "SOUL.md", "contents": "denied"}]})),
        )?;
        anyhow::ensure!(response.status == 403, "unexpected response: {response:?}");
        let conn = fixture.store()?;
        let rows: i64 = conn.query_row(
            "SELECT COUNT(*) FROM ward_audit
             WHERE familiar_id = ?1 AND event_type = 'validation_verdict'
               AND decision = 'protected-proposal-forbidden' AND proposal_id IS NULL",
            [FAMILIAR_ID],
            |row| row.get(0),
        )?;
        anyhow::ensure!(
            rows == 1,
            "protected admission refusal must have exactly one non-authorizing audit row, got {rows}"
        );
        anyhow::ensure!(
            fs::read_to_string(fixture.workspace.join("SOUL.md"))? == "# Sage\n",
            "audited rejection changed protected bytes"
        );
        Ok(())
    })
}

#[test]
fn smoke_out_of_band_drift_stages_without_execution() -> Result<()> {
    run_journey("smoke-out-of-band-drift", |fixture| {
        let baseline_request = json!({
            "edits": [{
                "target": "SOUL.md",
                "contents": "# Authorized identity\n"
            }],
            "principalKeyFingerprint": PRINCIPAL_FINGERPRINT
        });
        let baseline = fixture.request(
            "POST",
            "/api/v1/familiars/sage/edits",
            Some(&baseline_request),
        )?;
        anyhow::ensure!(
            baseline.status == 202 && baseline.body["disposition"] == "held",
            "protected baseline request was not held: {baseline:?}"
        );

        fs::write(fixture.workspace.join("SOUL.md"), "# Out-of-band drift\n")?;

        let request = json!({
            "edits": [{
                "target": "SOUL.md",
                "contents": "# Conflicting proposal\n"
            }],
            "principalKeyFingerprint": PRINCIPAL_FINGERPRINT
        });
        let response = fixture.request("POST", "/api/v1/familiars/sage/edits", Some(&request))?;

        anyhow::ensure!(response.status == 202, "unexpected response: {response:?}");
        anyhow::ensure!(
            response.body["disposition"] == "staged"
                && response.body["threadsGate"]["outcome"]["kind"] == "staged",
            "drift did not produce an explicit staged disposition: {}",
            response.body
        );
        anyhow::ensure!(
            fs::read_to_string(fixture.workspace.join("SOUL.md"))? == "# Out-of-band drift\n",
            "staged proposal overwrote drifted bytes"
        );

        let pending_path = response.body["threadsGate"]["outcome"]["pendingPath"]
            .as_str()
            .context("staged response is missing pendingPath")?;
        let pending_path = PathBuf::from(pending_path);
        let expected_pending = fixture.coven_home.join("pending").canonicalize()?;
        let actual_pending = pending_path.canonicalize()?;
        anyhow::ensure!(
            actual_pending.parent() == Some(expected_pending.as_path()),
            "staged proposal escaped the isolated pending directory: {}",
            actual_pending.display()
        );
        let pending: Value = serde_json::from_slice(&fs::read(&actual_pending)?)?;
        let proposal_id = response.body["threadsGate"]["outcome"]["proposalId"]
            .as_str()
            .context("staged response is missing proposalId")?;
        anyhow::ensure!(
            pending["id"] == proposal_id,
            "response and pending artifact identify different proposals"
        );
        anyhow::ensure!(
            pending["edits"][0]["surface"] == "SOUL.md",
            "pending proposal targets the wrong surface"
        );
        anyhow::ensure!(
            pending["edits"][0]["contents"]["encoding"] == "utf8"
                && pending["edits"][0]["contents"]["data"] == "# Conflicting proposal\n",
            "pending proposal does not retain the exact proposed bytes"
        );

        let conn = fixture.store()?;
        let audit: (String, String) = conn.query_row(
            "SELECT decision, files_touched FROM ward_audit
             WHERE familiar_id = ?1 ORDER BY id DESC LIMIT 1",
            [FAMILIAR_ID],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        anyhow::ensure!(
            audit.0 == "degrade_to_proposal",
            "unexpected terminal audit decision: {}",
            audit.0
        );
        anyhow::ensure!(
            serde_json::from_str::<Value>(&audit.1)? == json!(["SOUL.md"]),
            "drift audit does not identify the staged surface"
        );
        Ok(())
    })
}

#[test]
fn http_client_rejects_truncated_response_body() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let listener = UnixListener::bind(temp.path().join("coven.sock"))?;
    let server = thread::spawn(move || -> std::io::Result<()> {
        let (mut stream, _) = listener.accept()?;
        let mut request = Vec::new();
        stream.read_to_end(&mut request)?;
        stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 8\r\n\r\n{}")?;
        Ok(())
    });

    let error = unix_http_request(temp.path(), "GET", "/health", None)
        .expect_err("truncated response must fail closed");
    server
        .join()
        .map_err(|_| anyhow::anyhow!("HTTP fixture thread panicked"))??;
    anyhow::ensure!(
        error
            .to_string()
            .contains("before the declared 8-byte body"),
        "unexpected truncated-response error: {error:#}"
    );
    Ok(())
}

#[test]
fn same_home_daemon_lifecycle_helpers_survive_restart_and_crash() -> Result<()> {
    let evidence = EvidenceContext::new("same-home-daemon-lifecycle");
    let mut fixture = ThreadsFixture::start(&evidence)?;
    let journey_result = (|| {
        fixture.restart_daemon()?;
        let restarted = fixture.request("GET", "/health", None)?;
        anyhow::ensure!(
            restarted.status == 200 && restarted.body["ok"] == true,
            "daemon was not healthy after same-home restart: {restarted:?}"
        );

        fixture.stop_daemon()?;
        fixture.start_daemon()?;
        let restarted_after_stop = fixture.request("GET", "/health", None)?;
        anyhow::ensure!(
            restarted_after_stop.status == 200 && restarted_after_stop.body["ok"] == true,
            "daemon was not healthy after same-home stop/start: {restarted_after_stop:?}"
        );

        fixture.stop_daemon()?;
        fixture.restart_daemon()?;
        let restarted_from_stopped = fixture.request("GET", "/health", None)?;
        anyhow::ensure!(
            restarted_from_stopped.status == 200 && restarted_from_stopped.body["ok"] == true,
            "daemon was not healthy after restart from stopped: {restarted_from_stopped:?}"
        );

        fixture.crash_daemon()?;
        fixture.start_daemon()?;
        let restarted_after_crash = fixture.request("GET", "/health", None)?;
        anyhow::ensure!(
            restarted_after_crash.status == 200 && restarted_after_crash.body["ok"] == true,
            "daemon was not healthy after crash recovery start: {restarted_after_crash:?}"
        );
        Ok(())
    })();
    finalize_journey_with_artifact_check(&mut fixture, journey_result, |fixture| {
        let manifest: Value =
            serde_json::from_slice(&fs::read(fixture.artifact_dir.join("manifest.json"))?)?;
        anyhow::ensure!(
            manifest["result"] == "passed",
            "successful lifecycle run did not persist passed provenance: {manifest}"
        );
        let lifecycle = manifest["daemon_lifecycle"]
            .as_array()
            .context("success manifest is missing daemon_lifecycle")?;
        let operations = lifecycle
            .iter()
            .map(|event| {
                event["operation"]
                    .as_str()
                    .context("daemon lifecycle entry is missing operation")
            })
            .collect::<Result<Vec<_>>>()?;
        anyhow::ensure!(
            operations
                == [
                    "daemon start",
                    "daemon restart",
                    "daemon stop",
                    "daemon start",
                    "daemon stop",
                    "daemon restart",
                    "daemon crash",
                    "daemon start",
                    "daemon stop",
                ],
            "unexpected lifecycle sequence in success provenance: {operations:?}"
        );

        let daemon_log = fs::read_to_string(fixture.artifact_dir.join("logs/daemon.log"))?;
        anyhow::ensure!(
            !daemon_log.contains(&fixture.coven_home.display().to_string())
                && !daemon_log.contains(&fixture.workspace.display().to_string()),
            "success daemon log leaked unsanitized fixture paths:\n{daemon_log}"
        );
        anyhow::ensure!(
            daemon_log.contains("socket <coven-home>/coven.sock")
                && !daemon_log.contains("/private<coven-home>"),
            "success daemon log did not retain the sanitized socket placeholder:\n{daemon_log}"
        );

        let response: Value =
            serde_json::from_slice(&fs::read(fixture.artifact_dir.join("response.json"))?)?;
        anyhow::ensure!(
            response["body"]["daemon"]["socket"] == "<coven-home>/coven.sock",
            "success response provenance did not sanitize the socket path: {response}"
        );
        Ok(())
    })
}

fn run_journey(name: &str, journey: impl FnOnce(&mut ThreadsFixture) -> Result<()>) -> Result<()> {
    let evidence = EvidenceContext::new(name);
    let mut fixture = match ThreadsFixture::start(&evidence) {
        Ok(fixture) => fixture,
        Err(error) => {
            evidence.write_setup_failure(&error)?;
            return Err(error);
        }
    };
    let journey_result = journey(&mut fixture);
    finalize_journey(&mut fixture, journey_result)
}

fn finalize_journey(fixture: &mut ThreadsFixture, journey_result: Result<()>) -> Result<()> {
    finalize_journey_with_artifact_check(fixture, journey_result, |_| Ok(()))
}

fn finalize_journey_with_artifact_check(
    fixture: &mut ThreadsFixture,
    journey_result: Result<()>,
    verify_artifacts: impl FnOnce(&ThreadsFixture) -> Result<()>,
) -> Result<()> {
    let shutdown_result = fixture.shutdown();
    let mut result = match (journey_result, shutdown_result) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) => Err(error),
        (Ok(()), Err(error)) => Err(error.context("journey passed but daemon shutdown failed")),
        (Err(error), Err(shutdown)) => Err(error.context(format!(
            "daemon shutdown also failed after the journey error: {shutdown:#}"
        ))),
    };
    fixture.write_junit(result.as_ref().err())?;
    fixture.write_run_provenance(result.as_ref().err())?;
    if result.is_ok() {
        if let Err(error) = verify_artifacts(fixture) {
            result = Err(error);
            fixture.write_junit(result.as_ref().err())?;
            fixture.write_run_provenance(result.as_ref().err())?;
        }
    }
    if result.is_err() {
        fixture.write_failure_evidence()?;
    }
    result
}

#[derive(Debug)]
struct HttpResponse {
    status: u16,
    body: Value,
}

#[derive(Debug)]
struct DaemonLifecycleEvent {
    operation: &'static str,
    pid_before: Option<u32>,
    pid_after: Option<u32>,
    command_status: Option<i32>,
    stdout: Option<String>,
    stderr: Option<String>,
    note: Option<String>,
}

impl DaemonLifecycleEvent {
    fn from_output(
        operation: &'static str,
        pid_before: Option<u32>,
        pid_after: Option<u32>,
        output: &Output,
        note: Option<String>,
    ) -> Self {
        Self {
            operation,
            pid_before,
            pid_after,
            command_status: output.status.code(),
            stdout: Some(String::from_utf8_lossy(&output.stdout).into_owned()),
            stderr: Some(String::from_utf8_lossy(&output.stderr).into_owned()),
            note,
        }
    }

    fn note(operation: &'static str, pid_before: Option<u32>, note: impl Into<String>) -> Self {
        Self {
            operation,
            pid_before,
            pid_after: None,
            command_status: None,
            stdout: None,
            stderr: None,
            note: Some(note.into()),
        }
    }

    fn as_json(&self) -> Value {
        json!({
            "operation": self.operation,
            "pid_before": self.pid_before,
            "pid_after": self.pid_after,
            "command_status": self.command_status,
            "note": self.note,
        })
    }

    fn render_log(&self) -> String {
        let mut output = format!("event: {}\n", self.operation);
        if let Some(pid_before) = self.pid_before {
            output.push_str(&format!("pid_before: {pid_before}\n"));
        }
        if let Some(pid_after) = self.pid_after {
            output.push_str(&format!("pid_after: {pid_after}\n"));
        }
        if let Some(command_status) = self.command_status {
            output.push_str(&format!("command_status: {command_status}\n"));
        }
        if let Some(note) = &self.note {
            output.push_str(&format!("note: {note}\n"));
        }
        if let Some(stdout) = &self.stdout {
            output.push_str("stdout:\n");
            output.push_str(stdout);
            if !stdout.ends_with('\n') {
                output.push('\n');
            }
        }
        if let Some(stderr) = &self.stderr {
            output.push_str("stderr:\n");
            output.push_str(stderr);
            if !stderr.ends_with('\n') {
                output.push('\n');
            }
        }
        output
    }
}

struct ThreadsFixture {
    _temp: tempfile::TempDir,
    coven: PathBuf,
    coven_home: PathBuf,
    workspace: PathBuf,
    path: OsString,
    run_id: String,
    scenario: String,
    artifact_dir: PathBuf,
    coven_state: GitState,
    threads_state: GitState,
    coven_manifest_sha256: String,
    coven_lock_sha256: String,
    threads_manifest_sha256: String,
    local_threads_override_active: bool,
    last_request: Option<Value>,
    last_response: Option<Value>,
    daemon_events: Vec<DaemonLifecycleEvent>,
    daemon_pid: Option<u32>,
    stopped: bool,
}

impl ThreadsFixture {
    fn start(evidence: &EvidenceContext) -> Result<Self> {
        let workspace_root = workspace_root();
        let dependency = threads_dependency(&workspace_root)?;
        if std::env::var_os(REQUIRE_OVERRIDE_ENV).is_some() {
            anyhow::ensure!(
                dependency.local_override_active,
                "{REQUIRE_OVERRIDE_ENV}=1, but cargo metadata resolved coven-threads-core from {}",
                dependency.manifest_path.display()
            );
        }
        let coven_state = git_state(&workspace_root)?;
        let threads_state = git_state(
            dependency
                .manifest_path
                .parent()
                .context("Threads manifest has no parent directory")?,
        )?;
        let coven_manifest_sha256 =
            file_sha256(&workspace_root.join("crates/coven-cli/Cargo.toml"))?;
        let coven_lock_sha256 = file_sha256(&workspace_root.join("Cargo.lock"))?;
        let threads_manifest_sha256 = file_sha256(&dependency.manifest_path)?;
        let local_threads_override_active = dependency.local_override_active;

        let temp = tempfile::tempdir()?;
        let coven_home = temp.path().join("coven-home");
        let workspace = coven_home.join("familiars").join(FAMILIAR_ID);
        fs::create_dir_all(&workspace)?;
        seed_familiar(&coven_home, &workspace)?;

        let coven = PathBuf::from(env!("CARGO_BIN_EXE_coven"));
        let path = std::env::var_os("PATH").unwrap_or_default();
        let mut fixture = Self {
            _temp: temp,
            coven,
            coven_home,
            workspace,
            path,
            run_id: evidence.run_id.clone(),
            scenario: evidence.scenario.clone(),
            artifact_dir: evidence.artifact_dir.clone(),
            coven_state,
            threads_state,
            coven_manifest_sha256,
            coven_lock_sha256,
            threads_manifest_sha256,
            local_threads_override_active,
            last_request: None,
            last_response: None,
            daemon_events: Vec::new(),
            daemon_pid: None,
            stopped: false,
        };
        fixture.start_daemon()?;
        Ok(fixture)
    }

    fn request(&mut self, method: &str, path: &str, body: Option<&Value>) -> Result<HttpResponse> {
        let request = body.cloned().unwrap_or(Value::Null);
        self.last_request = Some(json!({
            "method": method,
            "path": path,
            "body": request,
        }));
        let serialized = body.map(Value::to_string);
        let (status, response) =
            unix_http_request(&self.coven_home, method, path, serialized.as_deref())?;
        let parsed: Value = serde_json::from_str(&response)
            .with_context(|| format!("daemon returned non-JSON response: {response}"))?;
        self.last_response = Some(json!({
            "status": status,
            "body": parsed,
        }));
        Ok(HttpResponse {
            status,
            body: parsed,
        })
    }

    fn store(&self) -> Result<Connection> {
        Connection::open(self.coven_home.join("coven.sqlite3")).map_err(Into::into)
    }

    fn current_daemon_pid(&self) -> Option<u32> {
        self.daemon_pid
            .filter(|pid| pid_is_alive(*pid))
            .or_else(|| {
                daemon_pid(&self.coven_home)
                    .ok()
                    .filter(|pid| pid_is_alive(*pid))
            })
    }

    fn daemon_command(&self, args: &[&str]) -> Result<Output> {
        run_coven(&self.coven, &self.coven_home, &self.path, args)
    }

    fn start_daemon(&mut self) -> Result<()> {
        let pid_before = self.current_daemon_pid();
        self.stopped = false;
        let output = self.daemon_command(&["daemon", "start"])?;
        anyhow::ensure!(
            output.status.success(),
            "daemon start failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let health_result = wait_for_daemon_health(&self.coven_home);
        let pid_after = daemon_pid(&self.coven_home).ok();
        self.daemon_events.push(DaemonLifecycleEvent::from_output(
            "daemon start",
            pid_before,
            pid_after,
            &output,
            health_result
                .as_ref()
                .err()
                .map(|error| format!("daemon health check failed after start: {error:#}")),
        ));
        health_result?;

        let pid_after = pid_after.context("daemon status is missing pid after start")?;
        self.daemon_pid = Some(pid_after);
        Ok(())
    }

    fn stop_daemon(&mut self) -> Result<()> {
        if self.stopped {
            return Ok(());
        }
        let pid_before = self.current_daemon_pid();
        let output = self.daemon_command(&["daemon", "stop"])?;
        self.daemon_events.push(DaemonLifecycleEvent::from_output(
            "daemon stop",
            pid_before,
            None,
            &output,
            None,
        ));
        anyhow::ensure!(
            output.status.success(),
            "daemon stop failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        wait_for_daemon_shutdown(&self.coven_home, pid_before)?;
        self.daemon_pid = None;
        self.stopped = true;
        Ok(())
    }

    fn restart_daemon(&mut self) -> Result<()> {
        let pid_before = self.current_daemon_pid();
        self.stopped = false;
        let output = self.daemon_command(&["daemon", "restart"])?;
        anyhow::ensure!(
            output.status.success(),
            "daemon restart failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );

        let health_result = wait_for_daemon_health(&self.coven_home);
        let pid_after = daemon_pid(&self.coven_home).ok();
        self.daemon_events.push(DaemonLifecycleEvent::from_output(
            "daemon restart",
            pid_before,
            pid_after,
            &output,
            health_result
                .as_ref()
                .err()
                .map(|error| format!("daemon health check failed after restart: {error:#}")),
        ));
        health_result?;

        let pid_after = pid_after.context("daemon status is missing pid after restart")?;
        self.daemon_pid = Some(pid_after);
        anyhow::ensure!(
            pid_before != Some(pid_after),
            "daemon restart did not replace the running process {pid_after}"
        );
        Ok(())
    }

    fn crash_daemon(&mut self) -> Result<()> {
        let pid = self
            .current_daemon_pid()
            .context("daemon crash requires a running daemon")?;
        let status = Command::new("kill")
            .args(["-KILL", &pid.to_string()])
            .status()
            .context("sending SIGKILL to the daemon")?;
        anyhow::ensure!(
            status.success(),
            "SIGKILL did not terminate daemon pid {pid}"
        );
        wait_for_process_exit(pid, "crashed daemon", Duration::from_secs(3))?;
        self.daemon_events.push(DaemonLifecycleEvent::note(
            "daemon crash",
            Some(pid),
            format!("sent SIGKILL to daemon pid {pid}"),
        ));
        self.daemon_pid = None;
        self.stopped = false;
        Ok(())
    }

    fn shutdown(&mut self) -> Result<()> {
        self.stop_daemon()
    }

    fn write_junit(&self, error: Option<&anyhow::Error>) -> Result<()> {
        fs::create_dir_all(&self.artifact_dir)?;
        let failure = error
            .map(|error| {
                let sanitized = sanitize_for_artifact(&format!("{error:#}"));
                format!(
                    "<failure message=\"{}\">{}</failure>",
                    xml_escape(&sanitize_for_artifact(&error.to_string())),
                    xml_escape(&sanitized)
                )
            })
            .unwrap_or_default();
        let failures = usize::from(error.is_some());
        fs::write(
            self.artifact_dir.join("junit.xml"),
            format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
                 <testsuite name=\"threads-e2e\" tests=\"1\" failures=\"{failures}\">\n\
                 <testcase classname=\"threads_e2e\" name=\"{}\">{failure}</testcase>\n\
                 </testsuite>\n",
                xml_escape(&self.scenario)
            ),
        )?;
        Ok(())
    }

    fn write_run_provenance(&self, error: Option<&anyhow::Error>) -> Result<()> {
        fs::create_dir_all(&self.artifact_dir)?;
        let logs = self.artifact_dir.join("logs");
        fs::create_dir_all(&logs)?;
        fs::write(
            self.artifact_dir.join("manifest.json"),
            serde_json::to_vec_pretty(&json!({
                "run_id": self.run_id,
                "scenario": self.scenario,
                "command": "cargo test --locked -p coven-cli --test threads_e2e -- --nocapture",
                "platform": std::env::consts::OS,
                "setup_completed": true,
                "result": if error.is_some() { "failed" } else { "passed" },
                "coven_commit": self.coven_state.commit,
                "coven_dirty": self.coven_state.dirty,
                "coven_state_sha256": self.coven_state.state_sha256,
                "coven_manifest_sha256": self.coven_manifest_sha256,
                "coven_lock_sha256": self.coven_lock_sha256,
                "threads_commit": self.threads_state.commit,
                "threads_dirty": self.threads_state.dirty,
                "threads_state_sha256": self.threads_state.state_sha256,
                "threads_manifest_sha256": self.threads_manifest_sha256,
                "local_threads_override_active": self.local_threads_override_active,
                "authorization_limitation": "synthetic principal fingerprint uses the strongest current daemon-owned Ward path; signed principal proof is not yet available",
                "daemon_lifecycle": self.daemon_events.iter().map(DaemonLifecycleEvent::as_json).collect::<Vec<_>>(),
                "failure": error.map(|error| sanitize_for_artifact(&format!("{error:#}"))),
            }))?,
        )?;
        fs::write(
            self.artifact_dir.join("request.json"),
            serde_json::to_vec_pretty(&self.sanitized_json(&self.last_request))?,
        )?;
        fs::write(
            self.artifact_dir.join("response.json"),
            serde_json::to_vec_pretty(&self.sanitized_json(&self.last_response))?,
        )?;

        let recovery_log = fs::read(self.coven_home.join("daemon-recovery.log"))
            .unwrap_or_else(|_| b"<no daemon recovery log>\n".to_vec());
        let mut daemon_log = String::new();
        if self.daemon_events.is_empty() {
            daemon_log.push_str("<no daemon lifecycle events recorded>\n");
        } else {
            for event in &self.daemon_events {
                daemon_log.push_str(&event.render_log());
                daemon_log.push('\n');
            }
        }
        daemon_log.push_str("daemon recovery log:\n");
        daemon_log.push_str(&String::from_utf8_lossy(&recovery_log));
        fs::write(
            logs.join("daemon.log"),
            self.sanitize_fixture_text(&daemon_log),
        )?;
        Ok(())
    }

    fn write_failure_evidence(&self) -> Result<()> {
        let state = self.artifact_dir.join("state");
        fs::create_dir_all(&state)?;
        fs::write(
            state.join("ward-audit.jsonl"),
            ward_audit_jsonl(&self.coven_home.join("coven.sqlite3"))?,
        )?;
        fs::write(
            state.join("pending-tree.txt"),
            inventory(&self.coven_home.join("pending"))?,
        )?;
        fs::write(
            state.join("workspace-tree.txt"),
            inventory(&self.workspace)?,
        )?;
        fs::write(
            state.join("sqlite-schema.txt"),
            sqlite_schema(&self.coven_home.join("coven.sqlite3"))?,
        )?;
        Ok(())
    }

    fn sanitized_json(&self, value: &Option<Value>) -> Option<Value> {
        value
            .as_ref()
            .map(|value| sanitize_json_strings(value, &|text| self.sanitize_fixture_text(text)))
    }

    fn sanitize_fixture_text(&self, value: &str) -> String {
        replace_sanitized_path(
            replace_sanitized_path(
                sanitize_for_artifact(value),
                &self.workspace,
                "<familiar-workspace>",
            ),
            &self.coven_home,
            "<coven-home>",
        )
    }
}

impl Drop for ThreadsFixture {
    fn drop(&mut self) {
        if self.stopped {
            return;
        }
        let pid = self.current_daemon_pid();
        let stopped = self
            .daemon_command(&["daemon", "stop"])
            .is_ok_and(|output| {
                output.status.success() && wait_for_daemon_shutdown(&self.coven_home, pid).is_ok()
            });
        if let Some(pid) = pid.filter(|pid| pid_is_alive(*pid)) {
            eprintln!(
                "threads E2E fallback is terminating daemon pid {} after graceful stop success={stopped}",
                pid,
            );
            let _ = Command::new("kill")
                .args(["-KILL", &pid.to_string()])
                .status();
        }
    }
}

struct ThreadsDependency {
    manifest_path: PathBuf,
    local_override_active: bool,
}

struct EvidenceContext {
    run_id: String,
    scenario: String,
    artifact_dir: PathBuf,
}

impl EvidenceContext {
    fn new(scenario: &str) -> Self {
        let run_id = format!("{}-{}-{}", scenario, std::process::id(), Uuid::new_v4());
        let artifact_dir = workspace_root()
            .join("target")
            .join("e2e-artifacts")
            .join(&run_id);
        Self {
            run_id,
            scenario: scenario.to_owned(),
            artifact_dir,
        }
    }

    fn write_setup_failure(&self, error: &anyhow::Error) -> Result<()> {
        fs::create_dir_all(&self.artifact_dir)?;
        let logs = self.artifact_dir.join("logs");
        let state = self.artifact_dir.join("state");
        fs::create_dir_all(&logs)?;
        fs::create_dir_all(&state)?;
        let sanitized = sanitize_for_artifact(&format!("{error:#}"));
        let failure = format!(
            "<failure message=\"{}\">{}</failure>",
            xml_escape(&sanitize_for_artifact(&error.to_string())),
            xml_escape(&sanitized)
        );
        fs::write(
            self.artifact_dir.join("junit.xml"),
            format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
                 <testsuite name=\"threads-e2e\" tests=\"1\" failures=\"1\">\n\
                 <testcase classname=\"threads_e2e\" name=\"{}\">{failure}</testcase>\n\
                 </testsuite>\n",
                xml_escape(&self.scenario)
            ),
        )?;
        fs::write(
            self.artifact_dir.join("manifest.json"),
            serde_json::to_vec_pretty(&json!({
                "run_id": self.run_id,
                "scenario": self.scenario,
                "command": "cargo test --locked -p coven-cli --test threads_e2e -- --nocapture",
                "platform": std::env::consts::OS,
                "setup_completed": false,
                "coven_commit": Value::Null,
                "threads_commit": Value::Null,
                "local_threads_override_active": Value::Null,
                "failure": sanitized,
            }))?,
        )?;
        fs::write(self.artifact_dir.join("request.json"), b"null\n")?;
        fs::write(self.artifact_dir.join("response.json"), b"null\n")?;
        fs::write(
            logs.join("daemon.log"),
            "<daemon unavailable: fixture setup did not complete>\n",
        )?;
        for name in [
            "ward-audit.jsonl",
            "pending-tree.txt",
            "workspace-tree.txt",
            "sqlite-schema.txt",
        ] {
            fs::write(
                state.join(name),
                "<unavailable: fixture setup did not complete>\n",
            )?;
        }
        Ok(())
    }
}

struct GitState {
    commit: String,
    dirty: bool,
    state_sha256: String,
}

fn threads_dependency(workspace_root: &Path) -> Result<ThreadsDependency> {
    let workspace_root = fs::canonicalize(workspace_root)
        .context("canonicalizing the Coven workspace for dependency proof")?;
    let coven_cli_manifest =
        fs::canonicalize(Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml"))
            .context("canonicalizing the coven-cli manifest path")?;
    let output = Command::new("cargo")
        .args(["metadata", "--format-version", "1", "--locked"])
        .current_dir(&workspace_root)
        .output()
        .context("running cargo metadata for the Threads override proof")?;
    anyhow::ensure!(
        output.status.success(),
        "cargo metadata failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let metadata: Value = serde_json::from_slice(&output.stdout)?;
    let packages = metadata["packages"]
        .as_array()
        .context("cargo metadata packages is not an array")?;
    let coven_cli = packages
        .iter()
        .find(|package| {
            package["name"] == "coven-cli"
                && package["manifest_path"]
                    .as_str()
                    .and_then(|path| fs::canonicalize(path).ok())
                    .as_ref()
                    == Some(&coven_cli_manifest)
        })
        .context("cargo metadata did not contain this coven-cli package")?;
    let coven_cli_id = coven_cli["id"]
        .as_str()
        .context("coven-cli package is missing its package id")?;
    let nodes = metadata["resolve"]["nodes"]
        .as_array()
        .context("cargo metadata did not contain a resolve graph")?;
    let coven_cli_node = nodes
        .iter()
        .find(|node| node["id"].as_str() == Some(coven_cli_id))
        .context("cargo metadata resolve graph did not contain coven-cli")?;
    let threads_id = coven_cli_node["deps"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|dependency| dependency["name"] == "coven_threads_core")
        .and_then(|dependency| dependency["pkg"].as_str())
        .context("coven-cli does not resolve a coven-threads-core dependency")?;
    let package = packages
        .iter()
        .find(|package| package["id"].as_str() == Some(threads_id))
        .context("resolved coven-threads-core package is missing from metadata")?;
    let manifest_path = PathBuf::from(
        package["manifest_path"]
            .as_str()
            .context("Threads package is missing manifest_path")?,
    );
    let manifest_path = fs::canonicalize(&manifest_path).with_context(|| {
        format!(
            "canonicalizing resolved Threads manifest {}",
            manifest_path.display()
        )
    })?;
    let local_override_active =
        package["source"].is_null() && !manifest_path.starts_with(workspace_root.join("crates"));
    Ok(ThreadsDependency {
        manifest_path,
        local_override_active,
    })
}

fn seed_familiar(coven_home: &Path, workspace: &Path) -> Result<()> {
    fs::write(
        coven_home.join("familiars.toml"),
        r#"[[familiar]]
id = "sage"
display_name = "Sage"
role = "Research"
description = "Synthetic Threads E2E familiar."
"#,
    )?;
    fs::write(workspace.join("SOUL.md"), "# Sage\n")?;
    fs::write(
        workspace.join("ward.toml"),
        format!(
            r#"principal_key_fingerprint = "{PRINCIPAL_FINGERPRINT}"
protected_surface = ["SOUL.md"]

[[surface]]
path = "SOUL.md"
tier = 0

[[surface]]
path = "reviewed/"
tier = 1

[[probe]]
surface = "reviewed/**"
id = "size-delta"

[[probe]]
surface = "reviewed/**"
id = "pattern-lint"
forbidden = ["(?i)ignore previous"]
"#
        ),
    )?;
    Ok(())
}

fn run_coven(coven: &Path, coven_home: &Path, path: &OsString, args: &[&str]) -> Result<Output> {
    Command::new(coven)
        .args(args)
        .env("COVEN_HOME", coven_home)
        .env("PATH", path)
        .output()
        .map_err(Into::into)
}

fn wait_for_daemon_health(coven_home: &Path) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut last_error = None;
    while Instant::now() < deadline {
        if coven_home.join("coven.sock").exists() {
            match unix_http_request(coven_home, "GET", "/health", None) {
                Ok((200, body)) if body.contains(r#""ok":true"#) => return Ok(()),
                Ok(_) => {}
                Err(error) => last_error = Some(error),
            }
        }
        thread::sleep(Duration::from_millis(50));
    }
    match last_error {
        Some(error) => anyhow::bail!("daemon did not become ready: {error:#}"),
        None => anyhow::bail!("daemon did not become ready"),
    }
}

fn daemon_pid(coven_home: &Path) -> Result<u32> {
    let status: Value = serde_json::from_slice(&fs::read(coven_home.join("daemon.json"))?)?;
    let pid = status["pid"]
        .as_u64()
        .context("daemon status is missing pid")?;
    u32::try_from(pid).context("daemon pid does not fit u32")
}

fn wait_for_process_exit(pid: u32, label: &str, timeout: Duration) -> Result<()> {
    let started = Instant::now();
    let deadline = started + timeout;
    while Instant::now() < deadline {
        if !pid_is_alive(pid) {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(25));
    }
    anyhow::bail!(
        "{label} pid {pid} remained observable for {:?}",
        started.elapsed()
    )
}

fn wait_for_daemon_shutdown(coven_home: &Path, pid: Option<u32>) -> Result<()> {
    let started = Instant::now();
    let deadline = started + Duration::from_secs(3);
    while Instant::now() < deadline {
        if pid.map(|pid| !pid_is_alive(pid)).unwrap_or(true)
            && !coven_home.join("daemon.json").exists()
            && !coven_home.join("coven.sock").exists()
        {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(25));
    }
    if let Some(pid) = pid.filter(|pid| pid_is_alive(*pid)) {
        anyhow::bail!(
            "daemon pid {pid} remained observable after checked stop for {:?}",
            started.elapsed()
        );
    }
    anyhow::bail!(
        "daemon status artifacts remained observable after checked stop for {:?}",
        started.elapsed()
    )
}

fn pid_is_alive(pid: u32) -> bool {
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn unix_http_request(
    coven_home: &Path,
    method: &str,
    path: &str,
    body: Option<&str>,
) -> Result<(u16, String)> {
    let body = body.unwrap_or_default();
    let request = format!(
        "{method} {path} HTTP/1.1\r\nHost: coven\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    let mut stream = UnixStream::connect(coven_home.join("coven.sock"))?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    stream.write_all(request.as_bytes())?;
    stream.shutdown(Shutdown::Write)?;

    let mut response = Vec::new();
    let mut buffer = [0_u8; 8192];
    let mut expected_len: Option<(usize, usize)> = None;
    loop {
        let read = stream.read(&mut buffer)?;
        if read == 0 {
            let (body_start, content_length) =
                expected_len.context("daemon response ended before complete HTTP headers")?;
            anyhow::ensure!(
                response.len() >= body_start.saturating_add(content_length),
                "daemon response ended after {} bytes, before the declared {}-byte body completed",
                response.len().saturating_sub(body_start),
                content_length
            );
            break;
        }
        response.extend_from_slice(&buffer[..read]);
        anyhow::ensure!(
            response.len() <= 5 * 1024 * 1024,
            "daemon HTTP response exceeded the E2E response budget"
        );
        if expected_len.is_none() {
            if let Some(header_end) = find_bytes(&response, b"\r\n\r\n") {
                let headers = std::str::from_utf8(&response[..header_end])?;
                let content_length = headers
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>())
                    })
                    .transpose()?
                    .context("daemon response is missing Content-Length")?;
                expected_len = Some((header_end + 4, content_length));
            }
        }
        if expected_len.is_some_and(|(body_start, content_length)| {
            response.len() >= body_start.saturating_add(content_length)
        }) {
            break;
        }
    }
    let (body_start, content_length) =
        expected_len.context("daemon response is missing complete HTTP framing")?;
    let body_end = body_start
        .checked_add(content_length)
        .context("daemon Content-Length overflowed the response budget")?;
    anyhow::ensure!(
        response.len() >= body_end,
        "daemon response body is shorter than Content-Length"
    );
    let headers = std::str::from_utf8(&response[..body_start - 4])?;
    let status = headers
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|status| status.parse::<u16>().ok())
        .with_context(|| format!("invalid HTTP response headers: {headers}"))?;
    let body = String::from_utf8(response[body_start..body_end].to_vec())?;
    Ok((status, body))
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn ward_audit_jsonl(store: &Path) -> Result<String> {
    if !store.exists() {
        return Ok(String::new());
    }
    let conn = Connection::open(store)?;
    let mut statement = conn.prepare(
        "SELECT id, event_type, proposal_id, familiar_id, tier, decision,
                approver, hex(diff_hash), detail, files_touched, channel,
                thread_id, submitted_at, decided_at, recorded_at
         FROM ward_audit ORDER BY id",
    )?;
    let rows = statement.query_map([], |row| {
        Ok(json!({
            "id": row.get::<_, i64>(0)?,
            "event_type": row.get::<_, String>(1)?,
            "proposal_id": row.get::<_, Option<String>>(2)?,
            "familiar_id": row.get::<_, String>(3)?,
            "tier": row.get::<_, Option<String>>(4)?,
            "decision": row.get::<_, String>(5)?,
            "approver": row.get::<_, Option<String>>(6)?,
            "diff_hash": row.get::<_, String>(7)?,
            "detail": row.get::<_, Option<String>>(8)?,
            "files_touched": row.get::<_, String>(9)?,
            "channel": row.get::<_, Option<String>>(10)?,
            "thread_id": row.get::<_, Option<String>>(11)?,
            "submitted_at": row.get::<_, String>(12)?,
            "decided_at": row.get::<_, String>(13)?,
            "recorded_at": row.get::<_, String>(14)?,
        }))
    })?;
    let mut output = String::new();
    for row in rows {
        output.push_str(&serde_json::to_string(&row?)?);
        output.push('\n');
    }
    Ok(output)
}

fn sqlite_schema(store: &Path) -> Result<String> {
    if !store.exists() {
        return Ok(String::new());
    }
    let conn = Connection::open(store)?;
    let mut statement = conn.prepare(
        "SELECT type, name, sql FROM sqlite_master
         WHERE sql IS NOT NULL ORDER BY type, name",
    )?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        ))
    })?;
    let mut output = String::new();
    for row in rows {
        let (kind, name, sql) = row?;
        output.push_str(&format!("-- {kind} {name}\n{sql};\n\n"));
    }
    Ok(output)
}

fn inventory(root: &Path) -> Result<String> {
    if !root.exists() {
        return Ok("<absent>\n".to_owned());
    }
    let mut entries = Vec::new();
    collect_inventory(root, root, &mut entries)?;
    entries.sort();
    Ok(entries.join("\n") + "\n")
}

fn collect_inventory(root: &Path, path: &Path, entries: &mut Vec<String>) -> Result<()> {
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let entry_path = entry.path();
        let relative = entry_path.strip_prefix(root)?;
        let metadata = fs::symlink_metadata(&entry_path)?;
        if metadata.is_dir() {
            entries.push(format!("{}/", relative.display()));
            collect_inventory(root, &entry_path, entries)?;
        } else if metadata.is_file() {
            let bytes = fs::read(&entry_path)?;
            entries.push(format!(
                "{} bytes={} sha256={}",
                relative.display(),
                bytes.len(),
                hex_bytes(&Sha256::digest(&bytes))
            ));
        } else {
            entries.push(format!("{} <non-regular>", relative.display()));
        }
    }
    Ok(())
}

fn hex_bytes(bytes: &[u8]) -> String {
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        write!(&mut encoded, "{byte:02x}").expect("writing to a String cannot fail");
    }
    encoded
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("coven-cli manifest is nested under crates/")
        .to_path_buf()
}

fn git_state(path: &Path) -> Result<GitState> {
    let root_output = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(path)
        .output()
        .with_context(|| format!("locating git root from {}", path.display()))?;
    anyhow::ensure!(
        root_output.status.success(),
        "git rev-parse --show-toplevel failed in {}: {}",
        path.display(),
        String::from_utf8_lossy(&root_output.stderr)
    );
    let root = PathBuf::from(String::from_utf8(root_output.stdout)?.trim());
    let commit_output = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(&root)
        .output()?;
    anyhow::ensure!(
        commit_output.status.success(),
        "git rev-parse HEAD failed in {}: {}",
        root.display(),
        String::from_utf8_lossy(&commit_output.stderr)
    );
    let status_output = Command::new("git")
        .args(["status", "--porcelain=v1", "-z", "--untracked-files=all"])
        .current_dir(&root)
        .output()?;
    anyhow::ensure!(
        status_output.status.success(),
        "git status failed in {}: {}",
        root.display(),
        String::from_utf8_lossy(&status_output.stderr)
    );
    let diff_output = Command::new("git")
        .args(["diff", "--binary", "HEAD", "--"])
        .current_dir(&root)
        .output()?;
    anyhow::ensure!(
        diff_output.status.success(),
        "git diff failed in {}: {}",
        root.display(),
        String::from_utf8_lossy(&diff_output.stderr)
    );

    let mut fingerprint = Sha256::new();
    fingerprint.update(&status_output.stdout);
    fingerprint.update(&diff_output.stdout);
    for record in status_output.stdout.split(|byte| *byte == 0) {
        if record.starts_with(b"?? ") {
            let relative = std::str::from_utf8(&record[3..])
                .context("git reported a non-UTF-8 untracked path")?;
            let untracked = root.join(relative);
            let metadata = fs::symlink_metadata(&untracked)?;
            if metadata.file_type().is_symlink() {
                fingerprint.update(relative.as_bytes());
                fingerprint.update(fs::read_link(untracked)?.as_os_str().as_bytes());
            } else if metadata.is_file() {
                fingerprint.update(relative.as_bytes());
                fingerprint.update(fs::read(untracked)?);
            }
        }
    }

    Ok(GitState {
        commit: String::from_utf8(commit_output.stdout)?.trim().to_owned(),
        dirty: !status_output.stdout.is_empty(),
        state_sha256: hex_bytes(&fingerprint.finalize()),
    })
}

fn file_sha256(path: &Path) -> Result<String> {
    Ok(hex_bytes(&Sha256::digest(
        fs::read(path).with_context(|| format!("reading {}", path.display()))?,
    )))
}

fn sanitize_for_artifact(value: &str) -> String {
    let mut sanitized = value.replace(&workspace_root().display().to_string(), "<coven-workspace>");
    if let Some(home) = std::env::var_os("HOME") {
        sanitized = sanitized.replace(&PathBuf::from(home).display().to_string(), "<home>");
    }
    sanitized
}

fn replace_sanitized_path(value: String, path: &Path, placeholder: &str) -> String {
    let display = path.display().to_string();
    let with_private = if display.starts_with("/private/") {
        value
    } else {
        value.replace(&format!("/private{display}"), placeholder)
    };
    let with_direct = with_private.replace(&display, placeholder);
    if display.starts_with("/private/") {
        return with_direct;
    }
    with_direct
}

fn sanitize_json_strings(value: &Value, sanitize: &impl Fn(&str) -> String) -> Value {
    match value {
        Value::String(text) => Value::String(sanitize(text)),
        Value::Array(values) => Value::Array(
            values
                .iter()
                .map(|value| sanitize_json_strings(value, sanitize))
                .collect(),
        ),
        Value::Object(values) => Value::Object(
            values
                .iter()
                .map(|(key, value)| (key.clone(), sanitize_json_strings(value, sanitize)))
                .collect(),
        ),
        scalar => scalar.clone(),
    }
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}
