//! Shared stateless executor protocol (`coven.executor.v1`) and the
//! hub-owned dispatcher that drives it over SSH or a local process
//! transport.
//!
//! Protocol invariants (multi-host spec, issue #267):
//! - The hub is the only side that initiates contact. It polls executor
//!   availability with `coven executor probe` and dispatches work with
//!   `coven executor run-job`, both invoked over an outbound transport
//!   (SSH or a private-network process launch). Executors never push
//!   registration or heartbeats to the hub.
//! - A dispatched job carries everything the executor needs (argv, cwd,
//!   env, stdin payload, and opaque hub-provided context) so the node can
//!   run it without local durable authority.
//! - Executors reply on stdout with a normalized result envelope carrying
//!   stdout/stderr/exit metadata; transport failures are normalized into
//!   the same envelope shape by the hub-side dispatcher.
//! - Stationary and compute executors share this base protocol and only
//!   differ in the role and capabilities they advertise.

use std::{
    collections::BTreeMap,
    io::{Read, Write},
    path::Path,
    process::Stdio,
    time::{Duration, Instant},
};

use anyhow::{bail, Context, Result};
use chrono::{SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const EXECUTOR_PROTOCOL_VERSION: &str = "coven.executor.v1";
pub const ROLE_STATIONARY_EXECUTOR: &str = "stationary_executor";
pub const ROLE_COMPUTE_EXECUTOR: &str = "compute_executor";

pub const RESULT_STATUS_COMPLETED: &str = "completed";
pub const RESULT_STATUS_FAILED: &str = "failed";
pub const RESULT_STATUS_TIMEOUT: &str = "timeout";
pub const RESULT_STATUS_REJECTED: &str = "rejected";
pub const RESULT_STATUS_TRANSPORT_ERROR: &str = "transport_error";

pub const DEFAULT_JOB_TIMEOUT_SECONDS: u64 = 300;
const PROBE_TIMEOUT: Duration = Duration::from_secs(30);
/// Extra transport allowance beyond the job timeout so the executor-side
/// timeout (which produces a normalized `timeout` envelope) wins over a
/// blunt transport kill.
const DISPATCH_TIMEOUT_GRACE_SECONDS: u64 = 30;
/// Cap captured stdout/stderr so result envelopes stay bounded.
const MAX_CAPTURED_OUTPUT_BYTES: usize = 1_048_576;

const DEFAULT_EXECUTOR_CAPABILITIES: [&str; 1] = ["shell"];

pub fn is_executor_role(role: &str) -> bool {
    role == ROLE_STATIONARY_EXECUTOR || role == ROLE_COMPUTE_EXECUTOR
}

/// Availability envelope returned by `coven executor probe`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutorProbe {
    pub protocol_version: String,
    pub role: String,
    pub capabilities: Vec<String>,
    pub available: bool,
    pub queue_pressure: i64,
    pub coven_version: String,
    pub probed_at: String,
}

/// Job context dispatched by the hub. Carries everything the stateless
/// executor needs; the executor never reads hub-authoritative state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutorJob {
    pub protocol_version: String,
    pub job_id: String,
    #[serde(default)]
    pub hub_id: Option<String>,
    #[serde(default)]
    pub required_capabilities: Vec<String>,
    pub command: Vec<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub stdin: Option<String>,
    #[serde(default)]
    pub timeout_seconds: Option<u64>,
    /// Opaque hub-provided context (memory/workspace/policy excerpts).
    /// The executor treats it as data; it is the hub's job to scope it.
    #[serde(default)]
    pub context: Option<Value>,
}

/// Normalized result envelope returned by `coven executor run-job` (and
/// synthesized by the dispatcher for transport failures).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutorResultEnvelope {
    pub protocol_version: String,
    pub job_id: String,
    pub status: String,
    pub exit_code: Option<i64>,
    pub stdout: String,
    pub stderr: String,
    pub started_at: String,
    pub finished_at: String,
    pub duration_ms: i64,
    #[serde(default)]
    pub error: Option<String>,
}

/// Hub-side node link configuration persisted in the node registry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum TransportConfig {
    /// Outbound SSH dispatch: `ssh <target> coven executor ...`.
    #[serde(rename_all = "camelCase")]
    Ssh {
        host: String,
        #[serde(default)]
        user: Option<String>,
        #[serde(default)]
        port: Option<u16>,
        #[serde(default)]
        identity_file: Option<String>,
        /// Remote executor entrypoint; defaults to `coven`.
        #[serde(default)]
        remote_program: Option<String>,
    },
    /// Private-network/local process dispatch (also the deterministic
    /// test seam): run `<program> [args..] executor ...` directly.
    #[serde(rename_all = "camelCase")]
    Local {
        program: String,
        #[serde(default)]
        args: Vec<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessOutput {
    pub exit_code: Option<i64>,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
}

/// Outbound hub-to-executor transport. Implementations must never expose
/// the local daemon socket; they run the executor protocol subcommands on
/// the remote side and relay stdio.
pub trait ExecutorTransport {
    fn describe(&self) -> String;
    fn run(
        &self,
        protocol_args: &[&str],
        stdin: Option<&str>,
        timeout: Duration,
    ) -> Result<ProcessOutput>;
}

pub fn build_transport(config: &TransportConfig) -> Result<Box<dyn ExecutorTransport>> {
    match config {
        TransportConfig::Ssh {
            host,
            user,
            port,
            identity_file,
            remote_program,
        } => Ok(Box::new(SshTransport::new(
            host,
            user.as_deref(),
            *port,
            identity_file.as_deref(),
            remote_program.as_deref(),
        )?)),
        TransportConfig::Local { program, args } => {
            if program.trim().is_empty() {
                bail!("local transport program must not be empty");
            }
            Ok(Box::new(LocalProcessTransport {
                program: program.clone(),
                args: args.clone(),
            }))
        }
    }
}

/// Hub-owned SSH dispatcher transport. Connections are always outbound
/// from the hub, batch-mode only (no interactive prompts), and fail
/// closed on unknown host keys so node identity stays pinned to the
/// operator-managed `known_hosts`.
pub struct SshTransport {
    host: String,
    user: Option<String>,
    port: Option<u16>,
    identity_file: Option<String>,
    remote_program: String,
}

impl SshTransport {
    pub fn new(
        host: &str,
        user: Option<&str>,
        port: Option<u16>,
        identity_file: Option<&str>,
        remote_program: Option<&str>,
    ) -> Result<Self> {
        let host = host.trim();
        if host.is_empty() {
            bail!("ssh transport host must not be empty");
        }
        // Refuse values that OpenSSH would parse as options.
        if host.starts_with('-') {
            bail!("ssh transport host must not start with '-'");
        }
        let user = user.map(str::trim);
        if user.is_some_and(|user| user.is_empty() || user.starts_with('-')) {
            bail!("ssh transport user must not be empty or start with '-'");
        }
        let identity_file = identity_file.map(str::trim);
        if identity_file.is_some_and(|path| path.is_empty() || path.starts_with('-')) {
            bail!("ssh transport identity file must not be empty or start with '-'");
        }
        let remote_program = remote_program.map_or("coven", str::trim);
        if remote_program.is_empty() || remote_program.starts_with('-') {
            bail!("ssh transport remote program must not be empty or start with '-'");
        }
        Ok(Self {
            host: host.to_string(),
            user: user.map(str::to_string),
            port,
            identity_file: identity_file.map(str::to_string),
            remote_program: remote_program.to_string(),
        })
    }

    pub fn argv(&self, protocol_args: &[&str]) -> Vec<String> {
        let mut argv = vec![
            "ssh".to_string(),
            "-o".to_string(),
            "BatchMode=yes".to_string(),
            "-o".to_string(),
            "StrictHostKeyChecking=yes".to_string(),
            "-o".to_string(),
            "ConnectTimeout=10".to_string(),
        ];
        if let Some(port) = self.port {
            argv.push("-p".to_string());
            argv.push(port.to_string());
        }
        if let Some(identity_file) = &self.identity_file {
            argv.push("-i".to_string());
            argv.push(identity_file.clone());
        }
        match &self.user {
            Some(user) => argv.push(format!("{user}@{}", self.host)),
            None => argv.push(self.host.clone()),
        }
        argv.push(self.remote_program.clone());
        argv.extend(protocol_args.iter().map(|arg| arg.to_string()));
        argv
    }
}

impl ExecutorTransport for SshTransport {
    fn describe(&self) -> String {
        match &self.user {
            Some(user) => format!("ssh {user}@{}", self.host),
            None => format!("ssh {}", self.host),
        }
    }

    fn run(
        &self,
        protocol_args: &[&str],
        stdin: Option<&str>,
        timeout: Duration,
    ) -> Result<ProcessOutput> {
        let argv = self.argv(protocol_args);
        let mut command = std::process::Command::new(&argv[0]);
        command.args(&argv[1..]);
        run_process_with_timeout(&mut command, stdin, timeout)
            .with_context(|| format!("ssh dispatch to {} failed", self.describe()))
    }
}

/// Runs the executor protocol against a local program. Used for
/// same-host/private-network dispatch and as the deterministic test seam.
pub struct LocalProcessTransport {
    pub program: String,
    pub args: Vec<String>,
}

impl ExecutorTransport for LocalProcessTransport {
    fn describe(&self) -> String {
        format!("local {}", self.program)
    }

    fn run(
        &self,
        protocol_args: &[&str],
        stdin: Option<&str>,
        timeout: Duration,
    ) -> Result<ProcessOutput> {
        let mut command = std::process::Command::new(&self.program);
        command.args(&self.args);
        command.args(protocol_args);
        run_process_with_timeout(&mut command, stdin, timeout)
            .with_context(|| format!("local dispatch via {} failed", self.program))
    }
}

/// Hub-side availability poll: runs `executor probe` over the transport
/// and validates the returned envelope. Errors mean "unavailable"; the
/// caller records last-known state in the node registry.
pub fn poll_executor(transport: &dyn ExecutorTransport) -> Result<ExecutorProbe> {
    let output = transport.run(&["executor", "probe"], None, PROBE_TIMEOUT)?;
    if output.timed_out {
        bail!("executor probe timed out via {}", transport.describe());
    }
    if output.exit_code != Some(0) {
        bail!(
            "executor probe exited with {:?} via {}: {}",
            output.exit_code,
            transport.describe(),
            output.stderr.trim()
        );
    }
    let probe: ExecutorProbe = serde_json::from_str(output.stdout.trim()).with_context(|| {
        format!(
            "executor probe returned malformed envelope: {}",
            output.stdout
        )
    })?;
    if probe.protocol_version != EXECUTOR_PROTOCOL_VERSION {
        bail!(
            "executor probe protocol version mismatch: expected {EXECUTOR_PROTOCOL_VERSION}, got {}",
            probe.protocol_version
        );
    }
    if !is_executor_role(&probe.role) {
        bail!("executor probe advertised unknown role {}", probe.role);
    }
    Ok(probe)
}

/// Hub-side dispatch: sends the job spec on stdin to `executor run-job`
/// over the transport and always returns a normalized result envelope.
/// Transport failures become `transport_error` envelopes so callers see
/// one shape regardless of where the failure happened.
pub fn dispatch_job(
    transport: &dyn ExecutorTransport,
    job: &ExecutorJob,
) -> ExecutorResultEnvelope {
    let started = Instant::now();
    let started_at = current_timestamp();
    let payload = match serde_json::to_string(job) {
        Ok(payload) => payload,
        Err(error) => {
            return transport_error_envelope(
                &job.job_id,
                &started_at,
                started,
                format!("failed to serialize job spec: {error}"),
                String::new(),
                String::new(),
            );
        }
    };
    let job_timeout = job.timeout_seconds.unwrap_or(DEFAULT_JOB_TIMEOUT_SECONDS);
    let transport_timeout =
        Duration::from_secs(job_timeout.saturating_add(DISPATCH_TIMEOUT_GRACE_SECONDS));
    let output = match transport.run(&["executor", "run-job"], Some(&payload), transport_timeout) {
        Ok(output) => output,
        Err(error) => {
            return transport_error_envelope(
                &job.job_id,
                &started_at,
                started,
                format!("transport failure via {}: {error:#}", transport.describe()),
                String::new(),
                String::new(),
            );
        }
    };
    if output.timed_out {
        return transport_error_envelope(
            &job.job_id,
            &started_at,
            started,
            format!("transport timed out via {}", transport.describe()),
            output.stdout,
            output.stderr,
        );
    }
    let envelope: ExecutorResultEnvelope = match serde_json::from_str(output.stdout.trim()) {
        Ok(envelope) => envelope,
        Err(error) => {
            return transport_error_envelope(
                &job.job_id,
                &started_at,
                started,
                format!("executor returned malformed result envelope: {error}"),
                output.stdout,
                output.stderr,
            );
        }
    };
    if envelope.protocol_version != EXECUTOR_PROTOCOL_VERSION {
        return transport_error_envelope(
            &job.job_id,
            &started_at,
            started,
            format!(
                "result envelope protocol version mismatch: expected {EXECUTOR_PROTOCOL_VERSION}, got {}",
                envelope.protocol_version
            ),
            output.stdout,
            output.stderr,
        );
    }
    if envelope.status != RESULT_STATUS_REJECTED && envelope.job_id != job.job_id {
        return transport_error_envelope(
            &job.job_id,
            &started_at,
            started,
            format!(
                "result envelope job id mismatch: expected {}, got {}",
                job.job_id, envelope.job_id
            ),
            output.stdout,
            output.stderr,
        );
    }
    envelope
}

fn transport_error_envelope(
    job_id: &str,
    started_at: &str,
    started: Instant,
    error: String,
    stdout: String,
    stderr: String,
) -> ExecutorResultEnvelope {
    ExecutorResultEnvelope {
        protocol_version: EXECUTOR_PROTOCOL_VERSION.to_string(),
        job_id: job_id.to_string(),
        status: RESULT_STATUS_TRANSPORT_ERROR.to_string(),
        exit_code: None,
        stdout,
        stderr,
        started_at: started_at.to_string(),
        finished_at: current_timestamp(),
        duration_ms: i64::try_from(started.elapsed().as_millis()).unwrap_or(i64::MAX),
        error: Some(error),
    }
}

/// Executor-node configuration (`<covenHome>/executor.json`). Optional;
/// absent config means a stationary executor with base capabilities.
/// This is the only executor-side file the protocol reads: it describes
/// what the node advertises, not any hub-authoritative state.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutorNodeConfig {
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub capabilities: Option<Vec<String>>,
}

pub fn load_node_config(coven_home: &Path) -> Result<ExecutorNodeConfig> {
    let path = coven_home.join("executor.json");
    if !path.exists() {
        return Ok(ExecutorNodeConfig::default());
    }
    let raw = std::fs::read_to_string(&path)
        .with_context(|| format!("failed to read executor config {}", path.display()))?;
    serde_json::from_str(&raw)
        .with_context(|| format!("failed to parse executor config {}", path.display()))
}

/// Executor-side availability envelope for `coven executor probe`.
pub fn build_probe(coven_home: &Path) -> Result<ExecutorProbe> {
    let config = load_node_config(coven_home)?;
    let role = config
        .role
        .unwrap_or_else(|| ROLE_STATIONARY_EXECUTOR.to_string());
    if !is_executor_role(&role) {
        bail!(
            "executor role must be `{ROLE_STATIONARY_EXECUTOR}` or `{ROLE_COMPUTE_EXECUTOR}`, got `{role}`"
        );
    }
    let capabilities = config.capabilities.unwrap_or_else(|| {
        DEFAULT_EXECUTOR_CAPABILITIES
            .iter()
            .map(|capability| capability.to_string())
            .collect()
    });
    Ok(ExecutorProbe {
        protocol_version: EXECUTOR_PROTOCOL_VERSION.to_string(),
        role,
        capabilities,
        available: true,
        // Stateless executors run dispatched jobs synchronously per
        // connection and hold no durable queue; the hub owns queues.
        queue_pressure: 0,
        coven_version: env!("CARGO_PKG_VERSION").to_string(),
        probed_at: current_timestamp(),
    })
}

/// Executor-side entrypoint for `coven executor run-job`: parses the job
/// spec from the stdin payload and always produces an envelope, even for
/// malformed input.
pub fn run_job_from_stdin_payload(payload: &str) -> ExecutorResultEnvelope {
    match serde_json::from_str::<ExecutorJob>(payload) {
        Ok(job) => run_job(&job),
        Err(error) => rejected_envelope("", format!("malformed job spec on stdin: {error}")),
    }
}

/// Runs one dispatched job. Deliberately stateless: no store access, no
/// hub-authoritative writes — the job spec is the entire context.
pub fn run_job(job: &ExecutorJob) -> ExecutorResultEnvelope {
    if job.protocol_version != EXECUTOR_PROTOCOL_VERSION {
        return rejected_envelope(
            &job.job_id,
            format!(
                "job protocol version mismatch: expected {EXECUTOR_PROTOCOL_VERSION}, got {}",
                job.protocol_version
            ),
        );
    }
    if job.job_id.trim().is_empty() {
        return rejected_envelope("", "job id must not be empty".to_string());
    }
    let Some(program) = job.command.first() else {
        return rejected_envelope(&job.job_id, "job command must not be empty".to_string());
    };
    if let Some(cwd) = &job.cwd {
        if !Path::new(cwd).is_dir() {
            return rejected_envelope(
                &job.job_id,
                format!("job cwd does not exist on this executor: {cwd}"),
            );
        }
    }

    let started = Instant::now();
    let started_at = current_timestamp();
    let mut command = std::process::Command::new(program);
    command.args(&job.command[1..]);
    if let Some(cwd) = &job.cwd {
        command.current_dir(cwd);
    }
    for (key, value) in &job.env {
        command.env(key, value);
    }
    let timeout = Duration::from_secs(job.timeout_seconds.unwrap_or(DEFAULT_JOB_TIMEOUT_SECONDS));
    let output = match run_process_with_timeout(&mut command, job.stdin.as_deref(), timeout) {
        Ok(output) => output,
        Err(error) => {
            return rejected_envelope(
                &job.job_id,
                format!("failed to launch job command: {error:#}"),
            );
        }
    };
    let status = if output.timed_out {
        RESULT_STATUS_TIMEOUT
    } else if output.exit_code == Some(0) {
        RESULT_STATUS_COMPLETED
    } else {
        RESULT_STATUS_FAILED
    };
    ExecutorResultEnvelope {
        protocol_version: EXECUTOR_PROTOCOL_VERSION.to_string(),
        job_id: job.job_id.clone(),
        status: status.to_string(),
        exit_code: output.exit_code,
        stdout: output.stdout,
        stderr: output.stderr,
        started_at,
        finished_at: current_timestamp(),
        duration_ms: i64::try_from(started.elapsed().as_millis()).unwrap_or(i64::MAX),
        error: if output.timed_out {
            Some(format!("job exceeded timeout of {}s", timeout.as_secs()))
        } else {
            None
        },
    }
}

fn rejected_envelope(job_id: &str, error: String) -> ExecutorResultEnvelope {
    let now = current_timestamp();
    ExecutorResultEnvelope {
        protocol_version: EXECUTOR_PROTOCOL_VERSION.to_string(),
        job_id: job_id.to_string(),
        status: RESULT_STATUS_REJECTED.to_string(),
        exit_code: None,
        stdout: String::new(),
        stderr: String::new(),
        started_at: now.clone(),
        finished_at: now,
        duration_ms: 0,
        error: Some(error),
    }
}

fn current_timestamp() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn run_process_with_timeout(
    command: &mut std::process::Command,
    stdin: Option<&str>,
    timeout: Duration,
) -> Result<ProcessOutput> {
    command
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().context("failed to spawn process")?;

    let stdin_writer = stdin.map(|payload| {
        let mut handle = child.stdin.take().expect("stdin was requested as piped");
        let payload = payload.as_bytes().to_vec();
        std::thread::spawn(move || {
            let _ = handle.write_all(&payload);
        })
    });
    let stdout_reader = spawn_capped_reader(child.stdout.take().expect("stdout is piped"));
    let stderr_reader = spawn_capped_reader(child.stderr.take().expect("stderr is piped"));

    let deadline = Instant::now() + timeout;
    let mut timed_out = false;
    let exit_status = loop {
        match child.try_wait().context("failed to poll process")? {
            Some(status) => break Some(status),
            None => {
                if Instant::now() >= deadline {
                    timed_out = true;
                    let _ = child.kill();
                    break child.wait().ok();
                }
                std::thread::sleep(Duration::from_millis(25));
            }
        }
    };
    if let Some(writer) = stdin_writer {
        let _ = writer.join();
    }
    let stdout = stdout_reader
        .join()
        .unwrap_or_else(|_| String::from("<stdout reader panicked>"));
    let stderr = stderr_reader
        .join()
        .unwrap_or_else(|_| String::from("<stderr reader panicked>"));
    let exit_code = if timed_out {
        None
    } else {
        exit_status.and_then(|status| status.code()).map(i64::from)
    };
    Ok(ProcessOutput {
        exit_code,
        stdout,
        stderr,
        timed_out,
    })
}

fn spawn_capped_reader<R: Read + Send + 'static>(mut source: R) -> std::thread::JoinHandle<String> {
    std::thread::spawn(move || {
        let mut buffer = Vec::new();
        let mut chunk = [0u8; 8192];
        loop {
            match source.read(&mut chunk) {
                Ok(0) => break,
                Ok(read) => {
                    let remaining = MAX_CAPTURED_OUTPUT_BYTES.saturating_sub(buffer.len());
                    let take = read.min(remaining);
                    buffer.extend_from_slice(&chunk[..take]);
                    // Keep draining past the cap so the child never blocks
                    // on a full pipe.
                }
                Err(_) => break,
            }
        }
        let mut text = String::from_utf8_lossy(&buffer).into_owned();
        if buffer.len() >= MAX_CAPTURED_OUTPUT_BYTES {
            text.push_str("\n<output truncated>");
        }
        text
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shell_command(script: &str) -> Vec<String> {
        #[cfg(unix)]
        {
            vec!["/bin/sh".to_string(), "-c".to_string(), script.to_string()]
        }
        #[cfg(windows)]
        {
            vec!["cmd".to_string(), "/C".to_string(), script.to_string()]
        }
    }

    fn job_with_command(command: Vec<String>) -> ExecutorJob {
        ExecutorJob {
            protocol_version: EXECUTOR_PROTOCOL_VERSION.to_string(),
            job_id: "job_test".to_string(),
            hub_id: Some("hub_test".to_string()),
            required_capabilities: vec![],
            command,
            cwd: None,
            env: BTreeMap::new(),
            stdin: None,
            timeout_seconds: Some(30),
            context: None,
        }
    }

    #[test]
    fn probe_defaults_to_stationary_role_with_base_capabilities() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;

        let probe = build_probe(temp_dir.path())?;

        assert_eq!(probe.protocol_version, EXECUTOR_PROTOCOL_VERSION);
        assert_eq!(probe.role, ROLE_STATIONARY_EXECUTOR);
        assert_eq!(probe.capabilities, vec!["shell".to_string()]);
        assert!(probe.available);
        assert_eq!(probe.queue_pressure, 0);
        Ok(())
    }

    #[test]
    fn probe_reads_role_and_capabilities_from_executor_config() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        std::fs::write(
            temp_dir.path().join("executor.json"),
            r#"{"role":"compute_executor","capabilities":["shell","gpu","long-running-loop"]}"#,
        )?;

        let probe = build_probe(temp_dir.path())?;

        assert_eq!(probe.role, ROLE_COMPUTE_EXECUTOR);
        assert_eq!(
            probe.capabilities,
            vec![
                "shell".to_string(),
                "gpu".to_string(),
                "long-running-loop".to_string()
            ]
        );
        Ok(())
    }

    #[test]
    fn probe_rejects_unknown_role() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        std::fs::write(
            temp_dir.path().join("executor.json"),
            r#"{"role":"authority_hub"}"#,
        )?;

        let error = build_probe(temp_dir.path()).unwrap_err();

        assert!(error.to_string().contains("executor role must be"));
        Ok(())
    }

    #[test]
    fn stationary_and_compute_probes_share_the_same_envelope_shape() -> Result<()> {
        let stationary_dir = tempfile::tempdir()?;
        let compute_dir = tempfile::tempdir()?;
        std::fs::write(
            compute_dir.path().join("executor.json"),
            r#"{"role":"compute_executor","capabilities":["gpu"]}"#,
        )?;

        let stationary = serde_json::to_value(build_probe(stationary_dir.path())?)?;
        let compute = serde_json::to_value(build_probe(compute_dir.path())?)?;

        let field_names = |value: &Value| {
            value
                .as_object()
                .expect("probe serializes to an object")
                .keys()
                .cloned()
                .collect::<Vec<_>>()
        };
        assert_eq!(field_names(&stationary), field_names(&compute));
        assert_eq!(stationary["protocolVersion"], compute["protocolVersion"]);
        assert_ne!(stationary["role"], compute["role"]);
        assert_ne!(stationary["capabilities"], compute["capabilities"]);
        Ok(())
    }

    #[test]
    fn run_job_returns_completed_envelope_with_captured_output() {
        let job = job_with_command(shell_command("echo job-ran"));

        let envelope = run_job(&job);

        assert_eq!(envelope.protocol_version, EXECUTOR_PROTOCOL_VERSION);
        assert_eq!(envelope.job_id, "job_test");
        assert_eq!(envelope.status, RESULT_STATUS_COMPLETED);
        assert_eq!(envelope.exit_code, Some(0));
        assert!(envelope.stdout.contains("job-ran"));
        assert!(envelope.error.is_none());
        assert!(envelope.duration_ms >= 0);
        assert!(!envelope.started_at.is_empty());
        assert!(!envelope.finished_at.is_empty());
    }

    #[test]
    fn run_job_reports_failed_status_with_exit_code_and_stderr() {
        #[cfg(unix)]
        let script = "echo boom 1>&2; exit 3";
        #[cfg(windows)]
        let script = "echo boom 1>&2 & exit /b 3";
        let job = job_with_command(shell_command(script));

        let envelope = run_job(&job);

        assert_eq!(envelope.status, RESULT_STATUS_FAILED);
        assert_eq!(envelope.exit_code, Some(3));
        assert!(envelope.stderr.contains("boom"));
    }

    #[test]
    fn run_job_applies_env_and_cwd_and_stdin_from_the_job_spec() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        #[cfg(unix)]
        let script = "printf '%s|%s|' \"$COVEN_JOB_FLAVOR\" \"$PWD\"; cat";
        #[cfg(windows)]
        let script = "echo %COVEN_JOB_FLAVOR% & more";
        let mut job = job_with_command(shell_command(script));
        job.cwd = Some(temp_dir.path().to_string_lossy().into_owned());
        job.env
            .insert("COVEN_JOB_FLAVOR".to_string(), "midnight".to_string());
        job.stdin = Some("from-the-hub".to_string());

        let envelope = run_job(&job);

        assert_eq!(envelope.status, RESULT_STATUS_COMPLETED);
        assert!(envelope.stdout.contains("midnight"));
        assert!(envelope.stdout.contains("from-the-hub"));
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn run_job_times_out_and_reports_timeout_status() {
        let mut job = job_with_command(shell_command("sleep 20"));
        job.timeout_seconds = Some(1);

        let envelope = run_job(&job);

        assert_eq!(envelope.status, RESULT_STATUS_TIMEOUT);
        assert_eq!(envelope.exit_code, None);
        assert!(envelope.error.as_deref().unwrap().contains("timeout"));
    }

    #[test]
    fn run_job_rejects_protocol_version_mismatch() {
        let mut job = job_with_command(shell_command("echo never-runs"));
        job.protocol_version = "coven.executor.v99".to_string();

        let envelope = run_job(&job);

        assert_eq!(envelope.status, RESULT_STATUS_REJECTED);
        assert!(envelope
            .error
            .as_deref()
            .unwrap()
            .contains("protocol version mismatch"));
    }

    #[test]
    fn run_job_rejects_empty_command_and_missing_cwd() {
        let mut empty_command = job_with_command(vec![]);
        empty_command.job_id = "job_empty".to_string();
        let empty_envelope = run_job(&empty_command);
        assert_eq!(empty_envelope.status, RESULT_STATUS_REJECTED);

        let mut missing_cwd = job_with_command(shell_command("echo never-runs"));
        missing_cwd.cwd = Some("/definitely/not/a/real/coven/cwd".to_string());
        let cwd_envelope = run_job(&missing_cwd);
        assert_eq!(cwd_envelope.status, RESULT_STATUS_REJECTED);
        assert!(cwd_envelope.error.as_deref().unwrap().contains("cwd"));
    }

    #[test]
    fn run_job_from_stdin_payload_rejects_malformed_json() {
        let envelope = run_job_from_stdin_payload("this is not a job spec");

        assert_eq!(envelope.status, RESULT_STATUS_REJECTED);
        assert_eq!(envelope.job_id, "");
        assert!(envelope
            .error
            .as_deref()
            .unwrap()
            .contains("malformed job spec"));
    }

    #[test]
    fn ssh_transport_builds_batch_mode_pinned_argv() -> Result<()> {
        let transport = SshTransport::new(
            "executor.internal",
            Some("coven"),
            Some(2222),
            Some("/home/coven/.ssh/id_ed25519"),
            None,
        )?;

        let argv = transport.argv(&["executor", "probe"]);

        assert_eq!(
            argv,
            vec![
                "ssh",
                "-o",
                "BatchMode=yes",
                "-o",
                "StrictHostKeyChecking=yes",
                "-o",
                "ConnectTimeout=10",
                "-p",
                "2222",
                "-i",
                "/home/coven/.ssh/id_ed25519",
                "coven@executor.internal",
                "coven",
                "executor",
                "probe",
            ]
        );
        Ok(())
    }

    #[test]
    fn ssh_transport_rejects_option_injection_shaped_values() {
        assert!(SshTransport::new("-oProxyCommand=evil", None, None, None, None).is_err());
        assert!(SshTransport::new("host", Some("-badflag"), None, None, None).is_err());
        assert!(SshTransport::new("host", None, None, Some("-i-evil"), None).is_err());
        assert!(SshTransport::new("host", None, None, None, Some("-notaprogram")).is_err());
        assert!(SshTransport::new("", None, None, None, None).is_err());
        // Whitespace-only values must fail closed too.
        assert!(SshTransport::new("host", Some("  "), None, None, None).is_err());
        assert!(SshTransport::new("host", None, None, Some("  "), None).is_err());
        assert!(SshTransport::new("host", None, None, None, Some("  ")).is_err());
    }

    #[test]
    fn transport_config_round_trips_ssh_and_local_kinds() -> Result<()> {
        let ssh: TransportConfig = serde_json::from_str(
            r#"{"kind":"ssh","host":"executor.internal","user":"coven","port":22}"#,
        )?;
        assert!(matches!(ssh, TransportConfig::Ssh { .. }));

        let local: TransportConfig =
            serde_json::from_str(r#"{"kind":"local","program":"/usr/local/bin/coven"}"#)?;
        assert!(matches!(local, TransportConfig::Local { .. }));

        assert!(build_transport(&ssh).is_ok());
        assert!(build_transport(&local).is_ok());
        assert!(build_transport(&TransportConfig::Local {
            program: "  ".to_string(),
            args: vec![],
        })
        .is_err());
        Ok(())
    }

    struct ScriptedTransport {
        exit_code: Option<i64>,
        stdout: String,
        stderr: String,
        timed_out: bool,
        fail: bool,
    }

    impl ScriptedTransport {
        fn replying(stdout: &str) -> Self {
            Self {
                exit_code: Some(0),
                stdout: stdout.to_string(),
                stderr: String::new(),
                timed_out: false,
                fail: false,
            }
        }
    }

    impl ExecutorTransport for ScriptedTransport {
        fn describe(&self) -> String {
            "scripted".to_string()
        }

        fn run(
            &self,
            _protocol_args: &[&str],
            _stdin: Option<&str>,
            _timeout: Duration,
        ) -> Result<ProcessOutput> {
            if self.fail {
                bail!("network unreachable");
            }
            Ok(ProcessOutput {
                exit_code: self.exit_code,
                stdout: self.stdout.clone(),
                stderr: self.stderr.clone(),
                timed_out: self.timed_out,
            })
        }
    }

    fn scripted_envelope_json(job_id: &str, protocol_version: &str) -> String {
        serde_json::to_string(&ExecutorResultEnvelope {
            protocol_version: protocol_version.to_string(),
            job_id: job_id.to_string(),
            status: RESULT_STATUS_COMPLETED.to_string(),
            exit_code: Some(0),
            stdout: "remote output".to_string(),
            stderr: String::new(),
            started_at: "2026-07-06T00:00:00Z".to_string(),
            finished_at: "2026-07-06T00:00:01Z".to_string(),
            duration_ms: 1000,
            error: None,
        })
        .expect("envelope serializes")
    }

    #[test]
    fn dispatch_job_returns_the_executor_result_envelope() {
        let job = job_with_command(vec!["true".to_string()]);
        let transport = ScriptedTransport::replying(&scripted_envelope_json(
            "job_test",
            EXECUTOR_PROTOCOL_VERSION,
        ));

        let envelope = dispatch_job(&transport, &job);

        assert_eq!(envelope.status, RESULT_STATUS_COMPLETED);
        assert_eq!(envelope.job_id, "job_test");
        assert_eq!(envelope.stdout, "remote output");
    }

    #[test]
    fn dispatch_job_normalizes_transport_failures_into_envelopes() {
        let job = job_with_command(vec!["true".to_string()]);
        let transport = ScriptedTransport {
            exit_code: None,
            stdout: String::new(),
            stderr: String::new(),
            timed_out: false,
            fail: true,
        };

        let envelope = dispatch_job(&transport, &job);

        assert_eq!(envelope.status, RESULT_STATUS_TRANSPORT_ERROR);
        assert_eq!(envelope.job_id, "job_test");
        assert!(envelope
            .error
            .as_deref()
            .unwrap()
            .contains("network unreachable"));
    }

    #[test]
    fn dispatch_job_flags_job_id_and_protocol_mismatches() {
        let job = job_with_command(vec!["true".to_string()]);

        let wrong_id = ScriptedTransport::replying(&scripted_envelope_json(
            "job_other",
            EXECUTOR_PROTOCOL_VERSION,
        ));
        let envelope = dispatch_job(&wrong_id, &job);
        assert_eq!(envelope.status, RESULT_STATUS_TRANSPORT_ERROR);
        assert!(envelope
            .error
            .as_deref()
            .unwrap()
            .contains("job id mismatch"));

        let wrong_version =
            ScriptedTransport::replying(&scripted_envelope_json("job_test", "coven.executor.v99"));
        let envelope = dispatch_job(&wrong_version, &job);
        assert_eq!(envelope.status, RESULT_STATUS_TRANSPORT_ERROR);
        assert!(envelope
            .error
            .as_deref()
            .unwrap()
            .contains("protocol version mismatch"));
    }

    #[test]
    fn dispatch_job_flags_malformed_result_envelopes() {
        let job = job_with_command(vec!["true".to_string()]);
        let transport = ScriptedTransport::replying("mostly noise, not json");

        let envelope = dispatch_job(&transport, &job);

        assert_eq!(envelope.status, RESULT_STATUS_TRANSPORT_ERROR);
        assert!(envelope
            .error
            .as_deref()
            .unwrap()
            .contains("malformed result envelope"));
    }

    #[test]
    fn poll_executor_parses_and_validates_the_probe_envelope() -> Result<()> {
        let probe_json = serde_json::to_string(&ExecutorProbe {
            protocol_version: EXECUTOR_PROTOCOL_VERSION.to_string(),
            role: ROLE_COMPUTE_EXECUTOR.to_string(),
            capabilities: vec!["shell".to_string(), "gpu".to_string()],
            available: true,
            queue_pressure: 2,
            coven_version: "0.0.0".to_string(),
            probed_at: "2026-07-06T00:00:00Z".to_string(),
        })?;
        let transport = ScriptedTransport::replying(&probe_json);

        let probe = poll_executor(&transport)?;

        assert_eq!(probe.role, ROLE_COMPUTE_EXECUTOR);
        assert_eq!(probe.queue_pressure, 2);
        Ok(())
    }

    #[test]
    fn poll_executor_rejects_bad_versions_roles_and_exit_codes() {
        let bad_version = ScriptedTransport::replying(
            r#"{"protocolVersion":"coven.executor.v99","role":"compute_executor","capabilities":[],"available":true,"queuePressure":0,"covenVersion":"0.0.0","probedAt":"2026-07-06T00:00:00Z"}"#,
        );
        assert!(poll_executor(&bad_version)
            .unwrap_err()
            .to_string()
            .contains("protocol version mismatch"));

        let bad_role = ScriptedTransport::replying(
            r#"{"protocolVersion":"coven.executor.v1","role":"authority_hub","capabilities":[],"available":true,"queuePressure":0,"covenVersion":"0.0.0","probedAt":"2026-07-06T00:00:00Z"}"#,
        );
        assert!(poll_executor(&bad_role)
            .unwrap_err()
            .to_string()
            .contains("unknown role"));

        let nonzero_exit = ScriptedTransport {
            exit_code: Some(255),
            stdout: String::new(),
            stderr: "connection refused".to_string(),
            timed_out: false,
            fail: false,
        };
        assert!(poll_executor(&nonzero_exit)
            .unwrap_err()
            .to_string()
            .contains("exited"));
    }

    #[test]
    fn local_transport_runs_the_protocol_arguments() -> Result<()> {
        #[cfg(unix)]
        let transport = LocalProcessTransport {
            program: "/bin/sh".to_string(),
            args: vec!["-c".to_string(), "echo \"$0 $1\"".to_string()],
        };
        #[cfg(windows)]
        let transport = LocalProcessTransport {
            program: "cmd".to_string(),
            args: vec!["/C".to_string(), "echo".to_string()],
        };

        let output = transport.run(&["executor", "probe"], None, Duration::from_secs(10))?;

        assert_eq!(output.exit_code, Some(0));
        assert!(output.stdout.contains("executor probe"));
        Ok(())
    }
}
