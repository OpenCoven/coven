use std::collections::HashMap;
use std::io::Write;
use std::net::{SocketAddr, TcpListener, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

#[cfg(unix)]
use std::io::{BufRead, BufReader, Read};
#[cfg(unix)]
use std::os::unix::{
    fs::{FileTypeExt, PermissionsExt},
    net::{UnixListener, UnixStream},
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::{
    api::{SessionLaunch, SessionRuntime},
    pty_runner,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DaemonStatus {
    pub pid: u32,
    pub started_at: String,
    pub socket: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DaemonStatusState {
    Running(DaemonStatus),
    Stale(DaemonStatus),
}

#[derive(Debug, Deserialize)]
struct DaemonHealthStatus {
    ok: bool,
    daemon: Option<DaemonStatus>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DaemonSpawnSpec {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub coven_home: PathBuf,
}

pub trait RuntimeKiller: Send {
    fn kill(&mut self) -> Result<()>;
}

/// Sentinel error returned by `LiveSessionRuntime::send_input` and
/// `kill_session` when the session id isn't in the live registry. The
/// API layer downcasts to this type instead of substring-matching the
/// error message — refactoring the prose now can't accidentally route
/// "not live" cases to the generic 500 path.
#[derive(Debug)]
pub struct NotLiveError {
    pub session_id: String,
}

impl std::fmt::Display for NotLiveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "session `{}` is not live in this daemon",
            self.session_id
        )
    }
}

impl std::error::Error for NotLiveError {}

#[derive(Default)]
pub struct LiveSessionRuntime {
    coven_home: Option<PathBuf>,
    sessions: Mutex<HashMap<String, LiveSessionHandle>>,
}

/// What kind of underlying process is bound to a registered live session.
/// PTY sessions take raw text on stdin (we forward `payload.data` as bytes).
/// Stream sessions take newline-delimited JSON; `payload.data` gets wrapped
/// in a `{"type":"user","message":{"role":"user","content":[{...}]}}` envelope
/// before being written to the child. See `docs/chat-persistence.md`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LiveSessionKind {
    Pty,
    Stream,
}

/// Registered live session. `input` and `killer` each sit behind their own
/// `Arc<Mutex<…>>` so `send_input` and `kill_session` can drop the global
/// `sessions` map lock before doing any potentially-blocking I/O (a
/// stream-mode harness whose child has stopped reading stdin will block
/// the write; we don't want that to wedge every other session op,
/// including a concurrent `/kill` to recover).
struct LiveSessionHandle {
    kind: LiveSessionKind,
    input: std::sync::Arc<Mutex<Box<dyn Write + Send>>>,
    killer: std::sync::Arc<Mutex<Box<dyn RuntimeKiller>>>,
}

impl LiveSessionRuntime {
    pub fn with_coven_home(coven_home: PathBuf) -> Self {
        Self {
            coven_home: Some(coven_home),
            sessions: Mutex::default(),
        }
    }

    #[allow(dead_code)]
    pub fn register(
        &self,
        session_id: String,
        input: Box<dyn Write + Send>,
        killer: Box<dyn RuntimeKiller>,
    ) -> Result<()> {
        self.register_kind(session_id, LiveSessionKind::Pty, input, killer)
    }

    fn register_kind(
        &self,
        session_id: String,
        kind: LiveSessionKind,
        input: Box<dyn Write + Send>,
        killer: Box<dyn RuntimeKiller>,
    ) -> Result<()> {
        use std::sync::Arc;
        self.sessions
            .lock()
            .map_err(|_| anyhow::anyhow!("live session registry lock poisoned"))?
            .insert(
                session_id,
                LiveSessionHandle {
                    kind,
                    input: Arc::new(Mutex::new(input)),
                    killer: Arc::new(Mutex::new(killer)),
                },
            );
        Ok(())
    }
}

impl SessionRuntime for LiveSessionRuntime {
    fn launch_session(&self, launch: &SessionLaunch) -> Result<()> {
        let command = pty_runner::build_harness_command_with_conversation(
            &launch.harness,
            &launch.prompt,
            Path::new(&launch.cwd),
            launch.launch_mode,
            launch.conversation.as_ref(),
        )?;
        let observer = self
            .coven_home
            .as_ref()
            .map(|coven_home| output_observer(coven_home.to_path_buf(), launch.id.clone()));

        if launch.launch_mode == crate::harness::HarnessLaunchMode::Stream {
            // Defense in depth: only allow Stream mode for harnesses that
            // actually have a stream-json entrypoint. Without this check
            // the chat's local gating could be bypassed by another client
            // requesting Stream for, say, codex — the daemon would then
            // JSON-wrap stdin into a one-shot `codex exec` process that
            // doesn't understand it.
            if !crate::harness::harness_supports_stream_mode(&launch.harness) {
                anyhow::bail!(
                    "harness `{}` does not support stream-mode launches; use launchMode `nonInteractive` instead",
                    launch.harness
                );
            }
            let piped = pty_runner::spawn_piped_with_observer(&command, observer)?;
            let mut killer_box: Box<dyn RuntimeKiller> = Box::new(PipedKiller { pid: piped.pid });
            let mut input = piped.input;
            // Send the launch's prompt as the first stream-json user
            // message so the chat doesn't need a separate send call right
            // after launch. A write failure here means the child already
            // exited (e.g. auth missing) — treat that as a hard launch
            // error: kill what's left of the child and surface it to the
            // caller so the session row is marked failed instead of
            // pretending we delivered the prompt.
            if !launch.prompt.is_empty() {
                if let Err(error) = write_stream_message(input.as_mut(), &launch.prompt) {
                    let _ = killer_box.kill();
                    return Err(error).with_context(|| {
                        format!(
                            "stream-mode launch of `{}` failed: child closed stdin before the initial message landed (auth/setup error?)",
                            launch.harness
                        )
                    });
                }
            }
            return self.register_kind(
                launch.id.clone(),
                LiveSessionKind::Stream,
                input,
                killer_box,
            );
        }

        let detached = pty_runner::spawn_detached_with_observer(&command, observer)?;
        self.register_kind(
            launch.id.clone(),
            LiveSessionKind::Pty,
            detached.input,
            Box::new(detached.killer),
        )
    }

    fn send_input(&self, session_id: &str, payload: &Value) -> Result<()> {
        let data = payload
            .get("data")
            .and_then(Value::as_str)
            .context("input payload requires string field `data`")?;
        // Look up the per-session input writer under the map lock, then
        // drop the map lock BEFORE blocking on the actual write. A
        // stream-mode child that's stopped reading stdin can stall the
        // write indefinitely; holding the global map lock during that
        // would wedge every other session op (including a concurrent
        // /kill that wants to recover from exactly this state).
        let (kind, input) = {
            let sessions = self
                .sessions
                .lock()
                .map_err(|_| anyhow::anyhow!("live session registry lock poisoned"))?;
            let session = sessions.get(session_id).ok_or_else(|| {
                anyhow::Error::new(NotLiveError {
                    session_id: session_id.to_string(),
                })
            })?;
            (session.kind, std::sync::Arc::clone(&session.input))
        };
        let mut input = input
            .lock()
            .map_err(|_| anyhow::anyhow!("live session input lock poisoned"))?;
        match kind {
            LiveSessionKind::Pty => {
                input
                    .write_all(data.as_bytes())
                    .context("failed to write input to live session")?;
                input
                    .flush()
                    .context("failed to flush live session input")?;
            }
            LiveSessionKind::Stream => {
                write_stream_message(input.as_mut(), data)?;
            }
        }
        Ok(())
    }

    fn kill_session(&self, session_id: &str) -> Result<()> {
        // Remove the handle under the map lock, then drop the map lock
        // before doing the actual kill. The killer is in its own
        // `Arc<Mutex>` so a concurrent `send_input` that's blocked on a
        // hung write can't prevent us from issuing the kill.
        let handle = {
            let mut sessions = self
                .sessions
                .lock()
                .map_err(|_| anyhow::anyhow!("live session registry lock poisoned"))?;
            sessions.remove(session_id).ok_or_else(|| {
                anyhow::Error::new(NotLiveError {
                    session_id: session_id.to_string(),
                })
            })?
        };
        let mut killer = handle
            .killer
            .lock()
            .map_err(|_| anyhow::anyhow!("live session killer lock poisoned"))?;
        killer.kill()
    }
}

/// Wrap raw user text in claude's stream-json user-message envelope and
/// write it to `input`, followed by a newline so the child reads it
/// immediately. Used by both the launch-time initial message and by the
/// per-turn `send_input` path.
fn write_stream_message(input: &mut dyn Write, text: &str) -> Result<()> {
    let envelope = json!({
        "type": "user",
        "message": {
            "role": "user",
            "content": [
                {"type": "text", "text": text}
            ]
        }
    });
    let mut line =
        serde_json::to_string(&envelope).context("failed to encode stream-json user envelope")?;
    line.push('\n');
    input
        .write_all(line.as_bytes())
        .context("failed to write stream-json message to live session")?;
    input
        .flush()
        .context("failed to flush stream-json message to live session")?;
    Ok(())
}

/// Killer for a non-PTY piped child (stream-mode harness sessions).
/// `pty_runner::spawn_piped_with_observer` returns just the child's PID
/// because the `Child` handle itself lives inside the wait/drain thread —
/// sharing it through a `Mutex` would deadlock when `wait()` blocks while
/// `kill()` waits for the same lock.
///
/// The spawn path puts the child in its own session/process group via
/// `setsid()` (pre_exec), so we can signal the entire group with one
/// syscall — that picks up subprocesses the harness may have spawned
/// (skills, MCP servers, shells, …) which would otherwise survive as
/// orphans. We send SIGKILL (not SIGTERM) because the daemon kill path
/// is reached from explicit user intent (`/kill`, `/clear`, chat exit)
/// where the right behavior is "stop immediately"; SIGTERM would let a
/// harness that ignores it linger past the user's request.
struct PipedKiller {
    pid: u32,
}

impl RuntimeKiller for PipedKiller {
    #[cfg(unix)]
    fn kill(&mut self) -> Result<()> {
        let pid = self.pid as libc::pid_t;
        // Negative argument signals the process group (pgid == pid
        // since the child called setsid). SIGKILL can't be ignored.
        let rc = unsafe { libc::kill(-pid, libc::SIGKILL) };
        if rc != 0 {
            let error = std::io::Error::last_os_error();
            // ESRCH means the group is already gone — fine, that's
            // the post-condition we want.
            if error.raw_os_error() != Some(libc::ESRCH) {
                return Err(error).with_context(|| {
                    format!("failed to SIGKILL stream harness process group {pid}")
                });
            }
        }
        Ok(())
    }

    #[cfg(not(unix))]
    fn kill(&mut self) -> Result<()> {
        anyhow::bail!(
            "stream-mode harness kill not implemented on this platform (pid {})",
            self.pid
        )
    }
}

impl RuntimeKiller for Box<dyn portable_pty::ChildKiller + Send + Sync> {
    fn kill(&mut self) -> Result<()> {
        self.as_mut().kill().context("failed to kill live session")
    }
}

fn output_observer(coven_home: PathBuf, session_id: String) -> pty_runner::DetachedPtyObserver {
    let output_home = coven_home.clone();
    let output_session_id = session_id.clone();
    let exit_home = coven_home;
    let exit_session_id = session_id;
    // UTF-8 boundary safety is enforced by `drain_detached_output` in
    // pty_runner per-source (separate buffers for stdout and stderr in
    // stream mode), so each chunk we receive here is already valid
    // UTF-8. We just decode and record. Lossy decode is a defensive
    // fallback that should never trigger.
    pty_runner::DetachedPtyObserver {
        on_output: Box::new(move |chunk| {
            if chunk.is_empty() {
                return;
            }
            let text = String::from_utf8(chunk)
                .unwrap_or_else(|err| String::from_utf8_lossy(err.as_bytes()).into_owned());
            let _ = record_session_event(
                &output_home,
                &output_session_id,
                "output",
                json!({ "data": text }),
            );
        }),
        on_exit: Box::new(move |result| {
            let _ = record_session_exit(&exit_home, &exit_session_id, result);
        }),
    }
}

fn record_session_exit(
    coven_home: &Path,
    session_id: &str,
    result: pty_runner::PtyRunResult,
) -> Result<()> {
    let conn = crate::store::open_store(&coven_home.join("coven.sqlite3"))?;
    if let Some(session) = crate::store::get_session(&conn, session_id)? {
        if session.status == "running" {
            // For conversation-grouped sessions (chat), a clean harness exit
            // is not the end of the conversation — the user can prompt again
            // and the daemon will resume into a sibling session under the
            // same `conversation_id`. Persist `idle` so API consumers (the
            // cockpit / dashboard) can distinguish "harness child terminated
            // cleanly, conversation still extendable" from "session failed".
            // Failed exits (non-zero / wait error) still surface as failure
            // so consumers don't mistake a crashed harness for a fresh slot.
            let persisted_status =
                if session.conversation_id.is_some() && result.status == "completed" {
                    "idle"
                } else {
                    result.status
                };
            crate::store::update_session_status(
                &conn,
                session_id,
                persisted_status,
                result.exit_code,
                &crate::api::current_timestamp(),
            )?;
        }
    }
    crate::store::insert_event_with_privacy(
        &conn,
        coven_home,
        &crate::store::EventRecord {
            seq: 0,
            id: uuid::Uuid::new_v4().to_string(),
            session_id: session_id.to_string(),
            kind: "exit".to_string(),
            payload_json: serde_json::to_string(&json!({
                "status": result.status,
                "exitCode": result.exit_code,
            }))
            .context("failed to serialize exit event payload")?,
            created_at: crate::api::current_timestamp(),
        },
    )
}

fn record_session_event(
    coven_home: &Path,
    session_id: &str,
    kind: &str,
    payload: Value,
) -> Result<()> {
    let conn = crate::store::open_store(&coven_home.join("coven.sqlite3"))?;
    crate::store::insert_event_with_privacy(
        &conn,
        coven_home,
        &crate::store::EventRecord {
            seq: 0,
            id: uuid::Uuid::new_v4().to_string(),
            session_id: session_id.to_string(),
            kind: kind.to_string(),
            payload_json: serde_json::to_string(&payload)
                .context("failed to serialize session event payload")?,
            created_at: crate::api::current_timestamp(),
        },
    )
}

pub fn daemon_status_path(coven_home: &Path) -> PathBuf {
    coven_home.join("daemon.json")
}

pub fn daemon_socket_path(coven_home: &Path) -> PathBuf {
    coven_home.join("coven.sock")
}

#[cfg(unix)]
fn ensure_private_coven_home(coven_home: &Path) -> Result<()> {
    // Fail closed if the home already exists as a symlink: following it would
    // let anyone able to plant the link redirect daemon state (socket, status,
    // SQLite ledger) outside the trusted directory. See docs/AUTH.md
    // "Current hardening gap".
    if let Ok(metadata) = std::fs::symlink_metadata(coven_home) {
        if metadata.file_type().is_symlink() {
            anyhow::bail!(
                "refusing to use Coven home {}: path is a symlink",
                coven_home.display()
            );
        }
    }
    std::fs::create_dir_all(coven_home)
        .with_context(|| format!("failed to create Coven home {}", coven_home.display()))?;
    std::fs::set_permissions(coven_home, std::fs::Permissions::from_mode(0o700)).with_context(
        || {
            format!(
                "failed to set Coven home permissions {}",
                coven_home.display()
            )
        },
    )?;
    Ok(())
}

#[cfg(not(unix))]
fn ensure_private_coven_home(coven_home: &Path) -> Result<()> {
    std::fs::create_dir_all(coven_home)
        .with_context(|| format!("failed to create Coven home {}", coven_home.display()))?;
    Ok(())
}

pub fn background_server_spec(current_exe: &Path, coven_home: &Path) -> DaemonSpawnSpec {
    DaemonSpawnSpec {
        program: current_exe.to_path_buf(),
        args: vec!["daemon".to_string(), "serve".to_string()],
        coven_home: coven_home.to_path_buf(),
    }
}

pub fn start_background_server(
    coven_home: &Path,
    current_exe: &Path,
    started_at: String,
) -> Result<DaemonStatus> {
    let spec = background_server_spec(current_exe, coven_home);
    ensure_private_coven_home(coven_home)?;
    let child = Command::new(&spec.program)
        .args(&spec.args)
        .env("COVEN_HOME", &spec.coven_home)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("failed to start Coven daemon {}", spec.program.display()))?;
    let status = DaemonStatus {
        pid: child.id(),
        started_at,
        socket: daemon_socket_path(coven_home)
            .to_string_lossy()
            .into_owned(),
    };
    write_status(coven_home, &status)?;
    Ok(status)
}

pub fn ensure_background_server(
    coven_home: &Path,
    current_exe: &Path,
    started_at: String,
) -> Result<DaemonStatus> {
    ensure_background_server_with_controllers(
        coven_home,
        current_exe,
        started_at,
        &SystemDaemonStopController,
        &SystemDaemonStartController,
    )
}

pub fn recover_orphaned_sessions(coven_home: &Path, updated_at: &str) -> Result<usize> {
    let conn = crate::store::open_store(&coven_home.join("coven.sqlite3"))?;
    crate::store::mark_running_sessions_orphaned(&conn, updated_at)
}

pub fn write_status(coven_home: &Path, status: &DaemonStatus) -> Result<()> {
    ensure_private_coven_home(coven_home)?;
    let json = serde_json::to_string_pretty(status).context("failed to serialize daemon status")?;
    let status_path = daemon_status_path(coven_home);
    std::fs::write(&status_path, format!("{json}\n")).context("failed to write daemon status")?;
    #[cfg(unix)]
    std::fs::set_permissions(&status_path, std::fs::Permissions::from_mode(0o600)).with_context(
        || {
            format!(
                "failed to set daemon status permissions {}",
                status_path.display()
            )
        },
    )?;
    Ok(())
}

pub fn read_status(coven_home: &Path) -> Result<Option<DaemonStatus>> {
    let path = daemon_status_path(coven_home);
    if !path.exists() {
        return Ok(None);
    }

    let json = std::fs::read_to_string(&path)
        .with_context(|| format!("failed to read daemon status {}", path.display()))?;
    let status = serde_json::from_str(&json).context("failed to parse daemon status")?;
    Ok(Some(status))
}

pub fn clear_status(coven_home: &Path) -> Result<bool> {
    let path = daemon_status_path(coven_home);
    if !path.exists() {
        return Ok(false);
    }

    std::fs::remove_file(&path)
        .with_context(|| format!("failed to remove daemon status {}", path.display()))?;
    Ok(true)
}

pub fn stop_background_server(coven_home: &Path) -> Result<bool> {
    stop_background_server_with_controller(coven_home, &SystemDaemonStopController)
}

pub fn background_server_status(coven_home: &Path) -> Result<Option<DaemonStatusState>> {
    background_server_status_with_controller(coven_home, &SystemDaemonStopController)
}

trait DaemonStopController {
    fn signal_term(&self, pid: u32) -> Result<()>;
    fn pid_is_alive(&self, pid: u32) -> bool;
    fn wait_for_exit(&self, pid: u32, timeout: Duration) -> bool;
    fn status_matches_running_daemon(&self, status: &DaemonStatus) -> bool;
}

struct SystemDaemonStopController;

trait DaemonStartController {
    fn start_background_server(
        &self,
        coven_home: &Path,
        current_exe: &Path,
        started_at: String,
    ) -> Result<DaemonStatus>;
    fn wait_for_running_daemon(&self, status: &DaemonStatus, timeout: Duration) -> bool;
}

struct SystemDaemonStartController;

impl DaemonStartController for SystemDaemonStartController {
    fn start_background_server(
        &self,
        coven_home: &Path,
        current_exe: &Path,
        started_at: String,
    ) -> Result<DaemonStatus> {
        start_background_server(coven_home, current_exe, started_at)
    }

    fn wait_for_running_daemon(&self, status: &DaemonStatus, timeout: Duration) -> bool {
        #[cfg(unix)]
        {
            let deadline = Instant::now() + timeout;
            while Instant::now() < deadline {
                if daemon_health_reports_pid(&status.socket, status.pid).unwrap_or(false) {
                    return true;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            daemon_health_reports_pid(&status.socket, status.pid).unwrap_or(false)
        }
        #[cfg(not(unix))]
        {
            let _ = status;
            let _ = timeout;
            true
        }
    }
}

impl DaemonStopController for SystemDaemonStopController {
    fn signal_term(&self, pid: u32) -> Result<()> {
        #[cfg(unix)]
        {
            let output = Command::new("kill")
                .arg("-TERM")
                .arg(pid.to_string())
                .stdin(Stdio::null())
                .output()
                .with_context(|| format!("failed to signal Coven daemon pid {pid}"))?;
            if output.status.success() {
                Ok(())
            } else {
                anyhow::bail!(
                    "failed to signal Coven daemon pid {pid}: {}",
                    String::from_utf8_lossy(&output.stderr).trim()
                )
            }
        }
        #[cfg(not(unix))]
        {
            let _ = pid;
            Ok(())
        }
    }

    fn pid_is_alive(&self, pid: u32) -> bool {
        #[cfg(unix)]
        {
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
        #[cfg(not(unix))]
        {
            let _ = pid;
            false
        }
    }

    fn wait_for_exit(&self, pid: u32, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if !self.pid_is_alive(pid) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        !self.pid_is_alive(pid)
    }

    fn status_matches_running_daemon(&self, status: &DaemonStatus) -> bool {
        #[cfg(unix)]
        {
            daemon_health_reports_pid(&status.socket, status.pid).unwrap_or(false)
        }
        #[cfg(not(unix))]
        {
            let _ = status;
            true
        }
    }
}

fn stop_background_server_with_controller(
    coven_home: &Path,
    controller: &dyn DaemonStopController,
) -> Result<bool> {
    let status = read_status(coven_home)?;
    let Some(status) = status else {
        return Ok(false);
    };

    if !controller.status_matches_running_daemon(&status) {
        if controller.pid_is_alive(status.pid) {
            anyhow::bail!(
                "Coven daemon pid {} could not be verified through its socket; not signaling or clearing daemon status",
                status.pid
            );
        }
        clear_status_and_socket(coven_home)?;
        return Ok(true);
    }

    match controller.signal_term(status.pid) {
        Ok(()) => {
            if !controller.wait_for_exit(status.pid, Duration::from_secs(2)) {
                anyhow::bail!(
                    "Coven daemon pid {} did not exit after SIGTERM; not clearing daemon status",
                    status.pid
                );
            }
        }
        Err(error) if controller.pid_is_alive(status.pid) => {
            anyhow::bail!(
                "failed to stop Coven daemon pid {}; not clearing daemon status: {error}",
                status.pid
            );
        }
        Err(_) => {}
    }

    clear_status_and_socket(coven_home)?;
    Ok(true)
}

fn background_server_status_with_controller(
    coven_home: &Path,
    controller: &dyn DaemonStopController,
) -> Result<Option<DaemonStatusState>> {
    let status = match read_status(coven_home) {
        Ok(status) => status,
        Err(error) if is_daemon_status_parse_error(&error) => {
            return recover_corrupt_status_for_status_command(coven_home);
        }
        Err(error) => return Err(error),
    };
    let Some(status) = status else {
        return Ok(None);
    };

    if controller.status_matches_running_daemon(&status) {
        return Ok(Some(DaemonStatusState::Running(status)));
    }

    if controller.pid_is_alive(status.pid) {
        return Ok(Some(DaemonStatusState::Stale(status)));
    }

    clear_status_and_socket(coven_home)?;
    Ok(None)
}

fn ensure_background_server_with_controllers(
    coven_home: &Path,
    current_exe: &Path,
    started_at: String,
    status_controller: &dyn DaemonStopController,
    start_controller: &dyn DaemonStartController,
) -> Result<DaemonStatus> {
    match background_server_status_with_controller(coven_home, status_controller)? {
        Some(DaemonStatusState::Running(status)) => Ok(status),
        Some(DaemonStatusState::Stale(status)) => anyhow::bail!(
            "Coven daemon pid {} is recorded but unreachable; run `coven daemon restart`",
            status.pid
        ),
        None => {
            let status =
                start_controller.start_background_server(coven_home, current_exe, started_at)?;
            if start_controller.wait_for_running_daemon(&status, Duration::from_secs(2)) {
                Ok(status)
            } else {
                anyhow::bail!(
                    "started Coven daemon pid {} but its socket did not become ready",
                    status.pid
                )
            }
        }
    }
}

fn is_daemon_status_parse_error(error: &anyhow::Error) -> bool {
    error
        .chain()
        .any(|cause| cause.downcast_ref::<serde_json::Error>().is_some())
}

fn recover_corrupt_status_for_status_command(
    coven_home: &Path,
) -> Result<Option<DaemonStatusState>> {
    match daemon_status_from_default_socket(coven_home) {
        Ok(Some(status)) => {
            write_status(coven_home, &status)?;
            Ok(Some(DaemonStatusState::Running(status)))
        }
        Ok(None) | Err(_) => {
            clear_status(coven_home)?;
            Ok(None)
        }
    }
}

fn clear_status_and_socket(coven_home: &Path) -> Result<()> {
    clear_status(coven_home)?;
    let socket = daemon_socket_path(coven_home);
    if socket.exists() {
        std::fs::remove_file(&socket)
            .with_context(|| format!("failed to remove daemon socket {}", socket.display()))?;
    }
    Ok(())
}

#[cfg(unix)]
fn daemon_status_from_default_socket(coven_home: &Path) -> Result<Option<DaemonStatus>> {
    daemon_status_from_health_socket(&daemon_socket_path(coven_home).to_string_lossy())
}

#[cfg(not(unix))]
fn daemon_status_from_default_socket(coven_home: &Path) -> Result<Option<DaemonStatus>> {
    let _ = coven_home;
    Ok(None)
}

#[cfg(unix)]
fn daemon_health_reports_pid(socket: &str, expected_pid: u32) -> Result<bool> {
    Ok(daemon_status_from_health_socket(socket)?
        .map(|status| status.pid == expected_pid)
        .unwrap_or(false))
}

#[cfg(unix)]
fn daemon_status_from_health_socket(socket: &str) -> Result<Option<DaemonStatus>> {
    let mut stream = UnixStream::connect(socket)
        .with_context(|| format!("failed to connect to Coven daemon socket {socket}"))?;
    stream
        .write_all(b"GET /health HTTP/1.1\r\nHost: coven\r\n\r\n")
        .context("failed to write Coven health request")?;
    stream
        .shutdown(std::net::Shutdown::Write)
        .context("failed to finish Coven health request")?;
    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .context("failed to read Coven health response")?;
    let Some((_, body)) = response.split_once("\r\n\r\n") else {
        return Ok(None);
    };
    let body: DaemonHealthStatus =
        serde_json::from_str(body).context("failed to parse Coven health response")?;
    if body.ok {
        Ok(body.daemon)
    } else {
        Ok(None)
    }
}

// `bind_tcp_listener` and `serve_next_tcp_connection` expose the TCP transport
// so it can be unit-tested in isolation; `serve_forever` wires them into the
// daemon's accept loop alongside the Unix socket listener.
//
// TCP gets read/write timeouts and a Content-Length cap because — unlike the
// Unix socket — a misbehaving network client can otherwise hold the API
// thread indefinitely (slowloris) or force a huge allocation by claiming a
// large body.
pub const TCP_IO_TIMEOUT: Duration = Duration::from_secs(30);
pub const MAX_TCP_BODY_BYTES: usize = 1024 * 1024;

fn ensure_loopback_addrs(addrs: &[SocketAddr]) -> Result<()> {
    if addrs.is_empty() {
        anyhow::bail!("TCP listener address did not resolve to any sockets");
    }
    let non_loopback_addrs: Vec<SocketAddr> = addrs
        .iter()
        .copied()
        .filter(|addr| !addr.ip().is_loopback())
        .collect();
    if !non_loopback_addrs.is_empty() {
        let non_loopback_addrs = non_loopback_addrs
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        anyhow::bail!(
            "refusing to bind Coven TCP API to non-loopback address(es): {non_loopback_addrs}; use 127.0.0.1 or ::1"
        );
    }
    Ok(())
}

pub fn bind_tcp_listener<A: ToSocketAddrs>(addr: A) -> Result<TcpListener> {
    let addrs: Vec<SocketAddr> = addr
        .to_socket_addrs()
        .context("failed to resolve Coven TCP listener address")?
        .collect();
    ensure_loopback_addrs(&addrs)?;
    let listener =
        TcpListener::bind(&addrs[..]).with_context(|| "failed to bind Coven TCP listener")?;
    Ok(listener)
}

#[cfg(unix)]
pub fn serve_next_tcp_connection(
    listener: &TcpListener,
    coven_home: &Path,
    status: Option<DaemonStatus>,
    runtime: &dyn SessionRuntime,
) -> Result<()> {
    let (stream, _) = listener
        .accept()
        .context("failed to accept TCP API connection")?;
    stream
        .set_read_timeout(Some(TCP_IO_TIMEOUT))
        .context("failed to set TCP read timeout")?;
    stream
        .set_write_timeout(Some(TCP_IO_TIMEOUT))
        .context("failed to set TCP write timeout")?;
    let read = stream.try_clone().context("failed to clone TCP stream")?;
    handle_http_stream(
        read,
        stream,
        coven_home,
        status,
        runtime,
        Some(MAX_TCP_BODY_BYTES),
    )
}

#[cfg(unix)]
pub fn bind_api_socket(coven_home: &Path) -> Result<UnixListener> {
    ensure_private_coven_home(coven_home)?;
    let socket_path = daemon_socket_path(coven_home);
    // Only ever replace a genuine, non-symlink socket. Blindly removing
    // whatever sits at the path would follow an attacker-planted symlink or
    // delete an unrelated file. See docs/AUTH.md "Current hardening gap".
    match std::fs::symlink_metadata(&socket_path) {
        Ok(metadata) => {
            let file_type = metadata.file_type();
            if file_type.is_symlink() {
                anyhow::bail!(
                    "refusing to bind Coven API socket {}: path is a symlink",
                    socket_path.display()
                );
            }
            if !file_type.is_socket() {
                anyhow::bail!(
                    "refusing to bind Coven API socket {}: path exists and is not a socket",
                    socket_path.display()
                );
            }
            std::fs::remove_file(&socket_path).with_context(|| {
                format!("failed to remove stale socket {}", socket_path.display())
            })?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(error).with_context(|| {
                format!("failed to inspect socket path {}", socket_path.display())
            });
        }
    }
    let listener = UnixListener::bind(&socket_path)
        .with_context(|| format!("failed to bind Coven API socket {}", socket_path.display()))?;
    std::fs::set_permissions(&socket_path, std::fs::Permissions::from_mode(0o600)).with_context(
        || {
            format!(
                "failed to set Coven API socket permissions {}",
                socket_path.display()
            )
        },
    )?;
    Ok(listener)
}

#[cfg(unix)]
pub fn serve_forever(coven_home: &Path, started_at: String, tcp_addr: Option<&str>) -> Result<()> {
    use std::sync::Arc;
    let status = DaemonStatus {
        pid: std::process::id(),
        started_at: started_at.clone(),
        socket: daemon_socket_path(coven_home)
            .to_string_lossy()
            .into_owned(),
    };
    write_status(coven_home, &status)?;
    recover_orphaned_sessions(coven_home, &started_at)?;
    let unix_listener = bind_api_socket(coven_home)?;
    let runtime = Arc::new(LiveSessionRuntime::with_coven_home(
        coven_home.to_path_buf(),
    ));

    if let Some(addr) = tcp_addr {
        let tcp_listener = bind_tcp_listener(addr)?;
        let tcp_home = coven_home.to_path_buf();
        let tcp_status = status.clone();
        let tcp_runtime = Arc::clone(&runtime);
        // TCP accept errors are logged and the loop continues — misbehaving
        // network clients should not bring down the daemon. The Unix loop
        // below treats accept errors as fatal because they indicate local
        // system trouble.
        std::thread::Builder::new()
            .name("coven-tcp-api".into())
            .spawn(move || loop {
                if let Err(error) = serve_next_tcp_connection(
                    &tcp_listener,
                    &tcp_home,
                    Some(tcp_status.clone()),
                    tcp_runtime.as_ref(),
                ) {
                    eprintln!("coven daemon: TCP connection error: {error:#}");
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
            })
            .context("failed to spawn TCP API thread")?;
    }

    loop {
        serve_next_connection(
            &unix_listener,
            coven_home,
            Some(status.clone()),
            runtime.as_ref(),
        )?;
    }
}

#[cfg(unix)]
fn handle_http_stream<R, W>(
    read: R,
    mut write: W,
    coven_home: &Path,
    status: Option<DaemonStatus>,
    runtime: &dyn SessionRuntime,
    max_body_bytes: Option<usize>,
) -> Result<()>
where
    R: Read,
    W: Write,
{
    let mut reader = BufReader::new(read);
    let request_line = read_http_request_line(&mut reader)?;
    let content_length = read_http_headers(&mut reader)?;
    if let Some(max) = max_body_bytes {
        if content_length > max {
            return write_payload_too_large(&mut write, max);
        }
    }
    let body = read_http_body(&mut reader, content_length)?;
    let (method, path) = parse_request_line(&request_line)?;
    let response = crate::api::handle_request_with_runtime(
        method,
        path,
        coven_home,
        status,
        body.as_deref(),
        runtime,
    )?;
    let reason = http_reason_phrase(response.status);
    let http = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        response.status,
        reason,
        response.content_type,
        response.body.len(),
        response.body
    );
    write
        .write_all(http.as_bytes())
        .context("failed to write API response")?;
    Ok(())
}

#[cfg(unix)]
fn write_payload_too_large<W: Write>(write: &mut W, max: usize) -> Result<()> {
    let body = format!(
        "{{\"ok\":false,\"error\":{{\"code\":\"payload_too_large\",\"message\":\"Request body exceeds {max}-byte limit.\"}}}}",
    );
    let http = format!(
        "HTTP/1.1 413 Payload Too Large\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    write
        .write_all(http.as_bytes())
        .context("failed to write 413 response")?;
    Ok(())
}

#[cfg(unix)]
pub fn serve_next_connection(
    listener: &UnixListener,
    coven_home: &Path,
    status: Option<DaemonStatus>,
    runtime: &dyn SessionRuntime,
) -> Result<()> {
    let (stream, _) = listener
        .accept()
        .context("failed to accept API connection")?;
    let read = stream.try_clone().context("failed to clone Unix stream")?;
    // Unix socket has no body cap — only local processes can reach it and the
    // socket permission bits already gate access.
    handle_http_stream(read, stream, coven_home, status, runtime, None)
}

fn http_reason_phrase(status: u16) -> &'static str {
    match status {
        200 => "OK",
        201 => "Created",
        202 => "Accepted",
        400 => "Bad Request",
        404 => "Not Found",
        409 => "Conflict",
        500 => "Internal Server Error",
        _ => "OK",
    }
}

#[cfg(unix)]
fn read_http_request_line<R: BufRead>(reader: &mut R) -> Result<String> {
    let mut line = String::new();
    reader
        .read_line(&mut line)
        .context("failed to read API request line")?;
    if line.is_empty() {
        anyhow::bail!("empty API request");
    }
    Ok(line)
}

#[cfg(unix)]
fn read_http_headers<R: BufRead>(reader: &mut R) -> Result<usize> {
    let mut content_length = 0;
    let mut header = String::new();
    loop {
        header.clear();
        let bytes = reader
            .read_line(&mut header)
            .context("failed to read API request header")?;
        if bytes == 0 || header == "\r\n" || header == "\n" {
            break;
        }
        if let Some((name, value)) = header.split_once(':') {
            if name.eq_ignore_ascii_case("content-length") {
                content_length = value
                    .trim()
                    .parse()
                    .context("invalid Content-Length header")?;
            }
        }
    }
    Ok(content_length)
}

#[cfg(unix)]
fn read_http_body<R: Read>(reader: &mut R, content_length: usize) -> Result<Option<String>> {
    if content_length == 0 {
        return Ok(None);
    }
    let mut bytes = vec![0; content_length];
    reader
        .read_exact(&mut bytes)
        .context("failed to read API request body")?;
    String::from_utf8(bytes)
        .map(Some)
        .context("API request body was not valid UTF-8")
}

#[cfg(unix)]
fn parse_request_line(line: &str) -> Result<(&str, &str)> {
    let mut parts = line.split_whitespace();
    let method = parts.next().context("missing HTTP method")?;
    let path = parts.next().context("missing HTTP path")?;
    Ok((method, path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_observer_records_each_callback_as_an_event() -> Result<()> {
        // UTF-8 boundary safety lives in pty_runner::drain_detached_output
        // now (see its tests). The observer's only job is to take each
        // chunk it receives and persist it as an `output` event. This
        // test pins that minimal contract by feeding the observer two
        // pre-decoded chunks and checking they show up verbatim in the
        // events table.
        let temp_dir = tempfile::tempdir()?;
        let conn = crate::store::open_store(&temp_dir.path().join("coven.sqlite3"))?;
        let session = session_record("buffered");
        crate::store::insert_session(&conn, &session)?;

        let observer = output_observer(temp_dir.path().to_path_buf(), session.id.clone());
        let pty_runner::DetachedPtyObserver { mut on_output, .. } = observer;

        // The drain layer would only ever hand us valid-UTF-8 slices,
        // so simulate that: a complete emoji and then a plain ASCII
        // chunk, each fully decodable on its own.
        on_output("🎉".as_bytes().to_vec());
        on_output(b" done".to_vec());

        let events = crate::store::list_events(&conn, &session.id)?;
        let mut decoded = String::new();
        for event in events.iter().filter(|e| e.kind == "output") {
            let payload: serde_json::Value = serde_json::from_str(&event.payload_json)?;
            if let Some(text) = payload.get("data").and_then(|v| v.as_str()) {
                decoded.push_str(text);
            }
        }
        assert_eq!(decoded, "🎉 done");
        Ok(())
    }

    #[test]
    fn live_runtime_rejects_stream_launch_for_non_stream_capable_harness() {
        let runtime = LiveSessionRuntime::default();
        let launch = crate::api::SessionLaunch {
            id: "session-x".to_string(),
            project_root: "/tmp/x".to_string(),
            cwd: "/tmp/x".to_string(),
            harness: "codex".to_string(),
            launch_mode: crate::harness::HarnessLaunchMode::Stream,
            prompt: "hello".to_string(),
            title: "stream codex (should be rejected)".to_string(),
            conversation: None,
            conversation_id: None,
        };

        let error = SessionRuntime::launch_session(&runtime, &launch).unwrap_err();
        assert!(
            error.to_string().contains("does not support stream-mode"),
            "rejection message should name the constraint, got: {error}"
        );
    }

    #[test]
    fn live_runtime_writes_input_to_registered_session() -> Result<()> {
        let runtime = LiveSessionRuntime::default();
        let output = SharedBuffer::default();
        runtime.register(
            "session-1".to_string(),
            Box::new(output.clone()),
            Box::new(RecordingKiller::default()),
        )?;

        SessionRuntime::send_input(
            &runtime,
            "session-1",
            &serde_json::json!({ "data": "hello live pty" }),
        )?;

        assert_eq!(output.text(), "hello live pty");
        Ok(())
    }

    #[test]
    fn live_runtime_kills_and_removes_registered_session() -> Result<()> {
        let runtime = LiveSessionRuntime::default();
        let killed = std::sync::Arc::new(std::sync::Mutex::new(false));
        runtime.register(
            "session-1".to_string(),
            Box::new(SharedBuffer::default()),
            Box::new(RecordingKiller {
                killed: killed.clone(),
            }),
        )?;

        SessionRuntime::kill_session(&runtime, "session-1")?;

        assert!(*killed.lock().unwrap());
        assert!(SessionRuntime::kill_session(&runtime, "session-1")
            .unwrap_err()
            .to_string()
            .contains("not live"));
        Ok(())
    }

    #[derive(Clone, Default)]
    struct SharedBuffer {
        data: std::sync::Arc<std::sync::Mutex<Vec<u8>>>,
    }

    impl SharedBuffer {
        fn text(&self) -> String {
            String::from_utf8(self.data.lock().unwrap().clone()).unwrap()
        }
    }

    impl Write for SharedBuffer {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.data.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[derive(Clone, Default)]
    struct RecordingKiller {
        killed: std::sync::Arc<std::sync::Mutex<bool>>,
    }

    impl RuntimeKiller for RecordingKiller {
        fn kill(&mut self) -> Result<()> {
            *self.killed.lock().unwrap() = true;
            Ok(())
        }
    }

    /// `Write` impl whose `write` blocks until a kill signal is set.
    /// Used to simulate a stream-mode child that has stopped reading
    /// stdin — we want `kill_session` to succeed even while
    /// `send_input` is mid-write to that exact session.
    #[derive(Clone)]
    struct BlockingWriter {
        unblock: std::sync::Arc<(std::sync::Mutex<bool>, std::sync::Condvar)>,
    }

    impl Write for BlockingWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            let (lock, cvar) = &*self.unblock;
            let mut guard = lock.lock().unwrap();
            while !*guard {
                guard = cvar.wait(guard).unwrap();
            }
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn kill_session_succeeds_even_while_send_input_is_blocked_on_a_hung_child() {
        use std::sync::{Arc, Condvar, Mutex as StdMutex};
        use std::thread;

        let runtime = Arc::new(LiveSessionRuntime::default());
        let unblock = Arc::new((StdMutex::new(false), Condvar::new()));
        let writer = BlockingWriter {
            unblock: Arc::clone(&unblock),
        };
        let killed = Arc::new(StdMutex::new(false));
        runtime
            .register(
                "wedged-session".to_string(),
                Box::new(writer),
                Box::new(RecordingKiller {
                    killed: killed.clone(),
                }),
            )
            .unwrap();

        // Kick off a send that will block on the writer.
        let sender_runtime = Arc::clone(&runtime);
        let sender = thread::spawn(move || {
            SessionRuntime::send_input(
                &*sender_runtime,
                "wedged-session",
                &serde_json::json!({ "data": "wedge" }),
            )
        });

        // Give the sender a moment to take the input lock + block.
        thread::sleep(std::time::Duration::from_millis(50));

        // Kill must succeed regardless of the wedged send.
        SessionRuntime::kill_session(&*runtime, "wedged-session")
            .expect("kill_session must not be blocked by a hung send_input on the same session");
        assert!(*killed.lock().unwrap());

        // Let the writer unblock so the sender thread can finish (its
        // post-kill state isn't what we're testing).
        {
            let (lock, cvar) = &*unblock;
            *lock.lock().unwrap() = true;
            cvar.notify_all();
        }
        let _ = sender.join();
    }

    #[test]
    fn http_reason_phrase_names_bad_requests() {
        assert_eq!(http_reason_phrase(400), "Bad Request");
    }

    #[cfg(unix)]
    #[test]
    fn handle_http_stream_processes_health_request() {
        use crate::api::NoopSessionRuntime;
        use std::io::Cursor;
        let temp = tempfile::tempdir().expect("tempdir");
        ensure_private_coven_home(temp.path()).expect("ensure home");
        let request = b"GET /api/v1/health HTTP/1.1\r\nHost: x\r\nContent-Length: 0\r\n\r\n";
        let mut stream = Cursor::new(Vec::from(&request[..]));
        let mut output: Vec<u8> = Vec::new();
        let runtime = NoopSessionRuntime;
        handle_http_stream(&mut stream, &mut output, temp.path(), None, &runtime, None)
            .expect("handle ok");
        let response = String::from_utf8(output).expect("utf8");
        assert!(response.starts_with("HTTP/1.1 200 OK"), "got: {response}");
        assert!(response.contains("\"apiVersion\""), "got: {response}");
    }

    #[cfg(unix)]
    #[test]
    fn handle_http_stream_rejects_oversize_body() {
        use crate::api::NoopSessionRuntime;
        use std::io::Cursor;
        let temp = tempfile::tempdir().expect("tempdir");
        ensure_private_coven_home(temp.path()).expect("ensure home");
        // Claim a body larger than the cap; the handler must reject without
        // reading the body, so the bytes don't need to actually be present.
        let request = format!(
            "POST /api/v1/cast HTTP/1.1\r\nHost: x\r\nContent-Length: {}\r\n\r\n",
            MAX_TCP_BODY_BYTES + 1
        );
        let mut stream = Cursor::new(request.into_bytes());
        let mut output: Vec<u8> = Vec::new();
        let runtime = NoopSessionRuntime;
        handle_http_stream(
            &mut stream,
            &mut output,
            temp.path(),
            None,
            &runtime,
            Some(MAX_TCP_BODY_BYTES),
        )
        .expect("handle ok");
        let response = String::from_utf8(output).expect("utf8");
        assert!(
            response.starts_with("HTTP/1.1 413 Payload Too Large"),
            "got: {response}"
        );
        assert!(response.contains("payload_too_large"), "got: {response}");
    }

    #[cfg(unix)]
    #[test]
    fn bind_tcp_listener_serves_health_over_tcp() {
        use crate::api::NoopSessionRuntime;
        use std::io::{Read, Write};
        use std::net::TcpStream;
        use std::thread;
        let temp = tempfile::tempdir().expect("tempdir");
        ensure_private_coven_home(temp.path()).expect("ensure home");
        let listener = bind_tcp_listener("127.0.0.1:0").expect("bind tcp");
        let addr = listener.local_addr().expect("local addr");
        let coven_home = temp.path().to_path_buf();
        let server = thread::spawn(move || {
            let runtime = NoopSessionRuntime;
            serve_next_tcp_connection(&listener, &coven_home, None, &runtime).expect("serve tcp");
        });

        let mut client = TcpStream::connect(addr).expect("connect");
        client
            .set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .expect("read timeout");
        client
            .write_all(b"GET /api/v1/health HTTP/1.1\r\nHost: x\r\nContent-Length: 0\r\n\r\n")
            .expect("write request");
        let mut response = String::new();
        client.read_to_string(&mut response).expect("read response");
        server.join().expect("server thread");

        assert!(response.starts_with("HTTP/1.1 200 OK"), "got: {response}");
        assert!(response.contains("\"apiVersion\""), "got: {response}");
    }

    #[test]
    fn bind_tcp_listener_rejects_non_loopback() {
        let error = bind_tcp_listener("0.0.0.0:0").expect_err("should reject wildcard bind");
        let msg = format!("{error:#}");
        assert!(
            msg.contains("non-loopback"),
            "unexpected error message: {msg}"
        );
    }

    #[test]
    fn recovers_persisted_running_sessions_as_orphaned() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let conn = crate::store::open_store(&temp_dir.path().join("coven.sqlite3"))?;
        let mut running = session_record("running");
        running.status = "running".to_string();
        let mut killed = session_record("killed");
        killed.status = "killed".to_string();
        crate::store::insert_session(&conn, &running)?;
        crate::store::insert_session(&conn, &killed)?;
        drop(conn);

        let updated = recover_orphaned_sessions(temp_dir.path(), "2026-04-27T08:00:00Z")?;
        let conn = crate::store::open_store(&temp_dir.path().join("coven.sqlite3"))?;
        let sessions = crate::store::list_sessions(&conn)?;

        assert_eq!(updated, 1);
        assert_eq!(session_status(&sessions, "running"), "orphaned");
        assert_eq!(session_status(&sessions, "killed"), "killed");
        Ok(())
    }

    #[test]
    fn writes_reads_and_clears_daemon_status() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let status = DaemonStatus {
            pid: 12345,
            started_at: "2026-04-27T10:00:00Z".to_string(),
            socket: temp_dir
                .path()
                .join("coven.sock")
                .to_string_lossy()
                .into_owned(),
        };

        write_status(temp_dir.path(), &status)?;

        assert_eq!(read_status(temp_dir.path())?, Some(status));
        assert!(clear_status(temp_dir.path())?);
        assert_eq!(read_status(temp_dir.path())?, None);
        assert!(!clear_status(temp_dir.path())?);
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn write_status_and_socket_use_owner_only_permissions() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        std::fs::set_permissions(temp_dir.path(), std::fs::Permissions::from_mode(0o755))?;
        let status = DaemonStatus {
            pid: 12345,
            started_at: "2026-04-27T10:00:00Z".to_string(),
            socket: daemon_socket_path(temp_dir.path())
                .to_string_lossy()
                .into_owned(),
        };

        write_status(temp_dir.path(), &status)?;
        let status_mode = std::fs::metadata(daemon_status_path(temp_dir.path()))?
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(status_mode, 0o600);

        let listener = bind_api_socket(temp_dir.path())?;
        assert!(daemon_socket_path(temp_dir.path()).exists());
        let socket_mode = std::fs::metadata(daemon_socket_path(temp_dir.path()))?
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(socket_mode, 0o600);
        drop(listener);

        let home_mode = std::fs::metadata(temp_dir.path())?.permissions().mode() & 0o777;
        assert_eq!(home_mode, 0o700);
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn bind_api_socket_hardens_coven_home_permissions() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        std::fs::set_permissions(temp_dir.path(), std::fs::Permissions::from_mode(0o755))?;

        let listener = bind_api_socket(temp_dir.path())?;
        drop(listener);

        let home_mode = std::fs::metadata(temp_dir.path())?.permissions().mode() & 0o777;
        assert_eq!(home_mode, 0o700);
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn ensure_private_coven_home_rejects_symlinked_home() -> Result<()> {
        use std::os::unix::fs::symlink;
        let temp_dir = tempfile::tempdir()?;
        let target = temp_dir.path().join("real-home");
        std::fs::create_dir(&target)?;
        let link = temp_dir.path().join("link-home");
        symlink(&target, &link)?;

        let error = ensure_private_coven_home(&link)
            .expect_err("a symlinked Coven home must be refused (AUTH.md fail-closed)");
        assert!(
            error.to_string().contains("symlink"),
            "error should name the symlink cause, got: {error}"
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn bind_api_socket_refuses_symlinked_socket_path() -> Result<()> {
        use std::os::unix::fs::symlink;
        let temp_dir = tempfile::tempdir()?;
        // Plant a symlink (to a real file) where the socket should be created.
        let decoy = temp_dir.path().join("decoy");
        std::fs::write(&decoy, b"x")?;
        symlink(&decoy, daemon_socket_path(temp_dir.path()))?;

        let error = bind_api_socket(temp_dir.path())
            .expect_err("a symlinked socket path must be refused (AUTH.md fail-closed)");
        assert!(
            error.to_string().contains("symlink"),
            "error should name the symlink cause, got: {error}"
        );
        // The guard must refuse before touching the link, so its target survives.
        assert!(
            decoy.exists(),
            "the symlink target must not be removed by the bind guard"
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn bind_api_socket_refuses_non_socket_at_socket_path() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        std::fs::write(daemon_socket_path(temp_dir.path()), b"not a socket")?;

        let error = bind_api_socket(temp_dir.path()).expect_err(
            "a non-socket file at the socket path must be refused (AUTH.md fail-closed)",
        );
        assert!(
            error.to_string().contains("not a socket"),
            "error should name the non-socket cause, got: {error}"
        );
        Ok(())
    }

    #[test]
    fn read_status_still_errors_on_corrupt_daemon_status() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        std::fs::create_dir_all(temp_dir.path())?;
        std::fs::write(daemon_status_path(temp_dir.path()), "{not json\n")?;

        let error = read_status(temp_dir.path()).expect_err("read_status should remain strict");

        assert!(error.to_string().contains("failed to parse daemon status"));
        assert!(
            daemon_status_path(temp_dir.path()).exists(),
            "strict read should not clear corrupt metadata"
        );
        Ok(())
    }

    #[test]
    fn background_server_status_clears_corrupt_metadata_without_daemon() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        std::fs::create_dir_all(temp_dir.path())?;
        std::fs::write(daemon_status_path(temp_dir.path()), "{not json\n")?;

        let state = background_server_status_with_controller(
            temp_dir.path(),
            &FakeStopController {
                pid_alive: false,
                exited_after_signal: false,
                signal_error: None,
                verified_daemon: false,
                signaled: std::sync::Arc::default(),
            },
        )?;

        assert_eq!(state, None);
        assert!(
            !daemon_status_path(temp_dir.path()).exists(),
            "status command path should clear corrupt daemon metadata"
        );
        Ok(())
    }

    #[test]
    fn stop_background_server_keeps_status_when_existing_daemon_survives() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let status = DaemonStatus {
            pid: 12345,
            started_at: "2026-04-27T10:00:00Z".to_string(),
            socket: daemon_socket_path(temp_dir.path())
                .to_string_lossy()
                .into_owned(),
        };
        write_status(temp_dir.path(), &status)?;

        let error = stop_background_server_with_controller(
            temp_dir.path(),
            &FakeStopController {
                pid_alive: true,
                exited_after_signal: false,
                signal_error: None,
                verified_daemon: true,
                signaled: std::sync::Arc::default(),
            },
        )
        .expect_err("stop should refuse to clear status while pid is alive");

        assert!(error.to_string().contains("did not exit"));
        assert_eq!(read_status(temp_dir.path())?, Some(status));
        Ok(())
    }

    #[test]
    fn stop_background_server_clears_stale_status_when_pid_is_gone() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let status = DaemonStatus {
            pid: 12345,
            started_at: "2026-04-27T10:00:00Z".to_string(),
            socket: daemon_socket_path(temp_dir.path())
                .to_string_lossy()
                .into_owned(),
        };
        write_status(temp_dir.path(), &status)?;

        assert!(stop_background_server_with_controller(
            temp_dir.path(),
            &FakeStopController {
                pid_alive: false,
                exited_after_signal: false,
                signal_error: Some("No such process".to_string()),
                verified_daemon: false,
                signaled: std::sync::Arc::default(),
            },
        )?);

        assert_eq!(read_status(temp_dir.path())?, None);
        Ok(())
    }

    #[test]
    fn stop_background_server_refuses_unverified_live_pid_without_signaling() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let status = DaemonStatus {
            pid: 12345,
            started_at: "2026-04-27T10:00:00Z".to_string(),
            socket: daemon_socket_path(temp_dir.path())
                .to_string_lossy()
                .into_owned(),
        };
        write_status(temp_dir.path(), &status)?;
        let controller = FakeStopController {
            pid_alive: true,
            exited_after_signal: true,
            signal_error: None,
            verified_daemon: false,
            signaled: std::sync::Arc::default(),
        };

        let error = stop_background_server_with_controller(temp_dir.path(), &controller)
            .expect_err("stop should not signal an unverified live pid");

        assert!(error.to_string().contains("could not be verified"));
        assert_eq!(*controller.signaled.lock().unwrap(), 0);
        assert_eq!(read_status(temp_dir.path())?, Some(status));
        Ok(())
    }

    #[test]
    fn background_server_status_returns_running_for_verified_daemon() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let status = DaemonStatus {
            pid: 12345,
            started_at: "2026-04-27T10:00:00Z".to_string(),
            socket: daemon_socket_path(temp_dir.path())
                .to_string_lossy()
                .into_owned(),
        };
        write_status(temp_dir.path(), &status)?;

        let state = background_server_status_with_controller(
            temp_dir.path(),
            &FakeStopController {
                pid_alive: true,
                exited_after_signal: false,
                signal_error: None,
                verified_daemon: true,
                signaled: std::sync::Arc::default(),
            },
        )?;

        assert_eq!(state, Some(DaemonStatusState::Running(status)));
        Ok(())
    }

    #[test]
    fn background_server_status_returns_stale_without_clearing_live_unverified_pid() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let status = DaemonStatus {
            pid: 12345,
            started_at: "2026-04-27T10:00:00Z".to_string(),
            socket: daemon_socket_path(temp_dir.path())
                .to_string_lossy()
                .into_owned(),
        };
        write_status(temp_dir.path(), &status)?;

        let state = background_server_status_with_controller(
            temp_dir.path(),
            &FakeStopController {
                pid_alive: true,
                exited_after_signal: false,
                signal_error: None,
                verified_daemon: false,
                signaled: std::sync::Arc::default(),
            },
        )?;

        assert_eq!(state, Some(DaemonStatusState::Stale(status.clone())));
        assert_eq!(read_status(temp_dir.path())?, Some(status));
        Ok(())
    }

    #[test]
    fn ensure_background_server_starts_when_no_daemon_status_exists() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let started = std::sync::Arc::new(std::sync::Mutex::new(0));
        let start_controller = FakeStartController {
            started: started.clone(),
            running_after_start: true,
        };

        let status = ensure_background_server_with_controllers(
            temp_dir.path(),
            Path::new("/usr/bin/coven"),
            "2026-04-27T10:00:00Z".to_string(),
            &FakeStopController {
                pid_alive: false,
                exited_after_signal: false,
                signal_error: None,
                verified_daemon: false,
                signaled: std::sync::Arc::default(),
            },
            &start_controller,
        )?;

        assert_eq!(*started.lock().unwrap(), 1);
        assert_eq!(status.pid, 54321);
        assert_eq!(read_status(temp_dir.path())?, Some(status));
        Ok(())
    }

    #[test]
    fn ensure_background_server_reuses_verified_running_daemon() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let status = DaemonStatus {
            pid: 12345,
            started_at: "2026-04-27T10:00:00Z".to_string(),
            socket: daemon_socket_path(temp_dir.path())
                .to_string_lossy()
                .into_owned(),
        };
        write_status(temp_dir.path(), &status)?;
        let started = std::sync::Arc::new(std::sync::Mutex::new(0));

        let ensured = ensure_background_server_with_controllers(
            temp_dir.path(),
            Path::new("/usr/bin/coven"),
            "2026-04-27T10:00:00Z".to_string(),
            &FakeStopController {
                pid_alive: true,
                exited_after_signal: false,
                signal_error: None,
                verified_daemon: true,
                signaled: std::sync::Arc::default(),
            },
            &FakeStartController {
                started: started.clone(),
                running_after_start: true,
            },
        )?;

        assert_eq!(ensured, status);
        assert_eq!(*started.lock().unwrap(), 0);
        Ok(())
    }

    struct FakeStopController {
        pid_alive: bool,
        exited_after_signal: bool,
        signal_error: Option<String>,
        verified_daemon: bool,
        signaled: std::sync::Arc<std::sync::Mutex<usize>>,
    }

    impl DaemonStopController for FakeStopController {
        fn signal_term(&self, _pid: u32) -> Result<()> {
            *self.signaled.lock().unwrap() += 1;
            match &self.signal_error {
                Some(error) => anyhow::bail!(error.clone()),
                None => Ok(()),
            }
        }

        fn pid_is_alive(&self, _pid: u32) -> bool {
            self.pid_alive
        }

        fn wait_for_exit(&self, _pid: u32, _timeout: std::time::Duration) -> bool {
            self.exited_after_signal
        }

        fn status_matches_running_daemon(&self, _status: &DaemonStatus) -> bool {
            self.verified_daemon
        }
    }

    struct FakeStartController {
        started: std::sync::Arc<std::sync::Mutex<usize>>,
        running_after_start: bool,
    }

    impl DaemonStartController for FakeStartController {
        fn start_background_server(
            &self,
            coven_home: &Path,
            _current_exe: &Path,
            started_at: String,
        ) -> Result<DaemonStatus> {
            *self.started.lock().unwrap() += 1;
            let status = DaemonStatus {
                pid: 54321,
                started_at,
                socket: daemon_socket_path(coven_home)
                    .to_string_lossy()
                    .into_owned(),
            };
            write_status(coven_home, &status)?;
            Ok(status)
        }

        fn wait_for_running_daemon(&self, _status: &DaemonStatus, _timeout: Duration) -> bool {
            self.running_after_start
        }
    }

    #[test]
    fn builds_background_server_spawn_spec() {
        let spec = background_server_spec(
            Path::new("/usr/local/bin/coven"),
            Path::new("/tmp/coven-home"),
        );

        assert_eq!(spec.program, PathBuf::from("/usr/local/bin/coven"));
        assert_eq!(spec.args, vec!["daemon".to_string(), "serve".to_string()]);
        assert_eq!(spec.coven_home, PathBuf::from("/tmp/coven-home"));
    }

    #[cfg(unix)]
    #[test]
    fn serves_health_over_unix_socket() -> Result<()> {
        use std::io::{Read, Write};
        use std::net::Shutdown;
        use std::os::unix::net::UnixStream;
        use std::thread;

        let temp_dir = tempfile::tempdir()?;
        let status = DaemonStatus {
            pid: 12345,
            started_at: "2026-04-27T10:00:00Z".to_string(),
            socket: daemon_socket_path(temp_dir.path())
                .to_string_lossy()
                .into_owned(),
        };
        let listener = bind_api_socket(temp_dir.path())?;
        let home = temp_dir.path().to_path_buf();
        let runtime = LiveSessionRuntime::default();
        let server =
            thread::spawn(move || serve_next_connection(&listener, &home, Some(status), &runtime));

        let mut stream = UnixStream::connect(daemon_socket_path(temp_dir.path()))?;
        stream.write_all(b"GET /health HTTP/1.1\r\nHost: coven\r\n\r\n")?;
        stream.shutdown(Shutdown::Write)?;
        let mut response = String::new();
        stream.read_to_string(&mut response)?;

        server.join().expect("server thread panicked")?;
        assert!(response.starts_with("HTTP/1.1 200 OK"));
        assert!(response.contains(r#""ok":true"#));
        assert!(response.contains(r#""apiVersion":"coven.daemon.v1""#));
        assert!(response.contains(r#""pid":12345"#));
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn forwards_http_request_body_to_api() -> Result<()> {
        use std::io::{Read, Write};
        use std::net::Shutdown;
        use std::os::unix::net::UnixStream;
        use std::thread;

        let temp_dir = tempfile::tempdir()?;
        let conn = crate::store::open_store(&temp_dir.path().join("coven.sqlite3"))?;
        crate::store::insert_session(
            &conn,
            &crate::store::SessionRecord {
                id: "session-1".to_string(),
                project_root: "/repo".to_string(),
                harness: "codex".to_string(),
                title: "hello from coven".to_string(),
                status: "running".to_string(),
                exit_code: None,
                archived_at: None,
                created_at: "2026-04-27T10:00:00Z".to_string(),
                updated_at: "2026-04-27T10:00:00Z".to_string(),
                conversation_id: None,
                labels: Vec::new(),
                visibility: "private".to_string(),
            },
        )?;
        let listener = bind_api_socket(temp_dir.path())?;
        let home = temp_dir.path().to_path_buf();
        let runtime = LiveSessionRuntime::default();
        runtime.register(
            "session-1".to_string(),
            Box::new(SharedBuffer::default()),
            Box::new(RecordingKiller::default()),
        )?;
        let server = thread::spawn(move || serve_next_connection(&listener, &home, None, &runtime));

        let body = r#"{"data":"hello over socket"}"#;
        let request = format!(
            "POST /sessions/session-1/input HTTP/1.1\r\nHost: coven\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body
        );
        let mut stream = UnixStream::connect(daemon_socket_path(temp_dir.path()))?;
        stream.write_all(request.as_bytes())?;
        stream.shutdown(Shutdown::Write)?;
        let mut response = String::new();
        stream.read_to_string(&mut response)?;

        server.join().expect("server thread panicked")?;
        let events = crate::store::list_events(&conn, "session-1")?;
        assert!(response.starts_with("HTTP/1.1 202 Accepted"));
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, "input");
        assert!(events[0].payload_json.contains("hello over socket"));
        Ok(())
    }

    #[test]
    fn records_output_and_exit_events_for_live_session() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let conn = crate::store::open_store(&temp_dir.path().join("coven.sqlite3"))?;
        let mut session = session_record("session-1");
        session.status = "running".to_string();
        crate::store::insert_session(&conn, &session)?;
        drop(conn);

        record_session_event(
            temp_dir.path(),
            "session-1",
            "output",
            json!({ "data": "hello from pty" }),
        )?;
        record_session_exit(
            temp_dir.path(),
            "session-1",
            pty_runner::PtyRunResult {
                status: "completed",
                exit_code: Some(0),
            },
        )?;

        let conn = crate::store::open_store(&temp_dir.path().join("coven.sqlite3"))?;
        let sessions = crate::store::list_sessions(&conn)?;
        let events = crate::store::list_events(&conn, "session-1")?;
        assert_eq!(session_status(&sessions, "session-1"), "completed");
        assert_eq!(events.len(), 2);
        let output = events.iter().find(|event| event.kind == "output").unwrap();
        let exit = events.iter().find(|event| event.kind == "exit").unwrap();
        assert!(output.payload_json.contains("hello from pty"));
        assert!(exit.payload_json.contains("completed"));
        Ok(())
    }

    #[test]
    fn exit_event_does_not_overwrite_killed_session_status() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let conn = crate::store::open_store(&temp_dir.path().join("coven.sqlite3"))?;
        let mut session = session_record("session-1");
        session.status = "killed".to_string();
        crate::store::insert_session(&conn, &session)?;
        drop(conn);

        record_session_exit(
            temp_dir.path(),
            "session-1",
            pty_runner::PtyRunResult {
                status: "failed",
                exit_code: Some(1),
            },
        )?;

        let conn = crate::store::open_store(&temp_dir.path().join("coven.sqlite3"))?;
        let sessions = crate::store::list_sessions(&conn)?;
        assert_eq!(session_status(&sessions, "session-1"), "killed");
        Ok(())
    }

    #[test]
    fn clean_exit_on_conversational_session_persists_as_idle() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let conn = crate::store::open_store(&temp_dir.path().join("coven.sqlite3"))?;
        let mut session = session_record("session-1");
        session.status = "running".to_string();
        session.conversation_id = Some("conv-abc".to_string());
        crate::store::insert_session(&conn, &session)?;
        drop(conn);

        record_session_exit(
            temp_dir.path(),
            "session-1",
            pty_runner::PtyRunResult {
                status: "completed",
                exit_code: Some(0),
            },
        )?;

        let conn = crate::store::open_store(&temp_dir.path().join("coven.sqlite3"))?;
        let stored = crate::store::get_session(&conn, "session-1")?.unwrap();
        // Persisted status is `idle` (conversation still extendable), exit code is
        // preserved so consumers can see the prior child exited cleanly, and the
        // `exit` event still says `completed` so transcripts remain accurate.
        assert_eq!(stored.status, "idle");
        assert_eq!(stored.exit_code, Some(0));
        let events = crate::store::list_events(&conn, "session-1")?;
        let exit = events.iter().find(|event| event.kind == "exit").unwrap();
        assert!(exit.payload_json.contains("\"status\":\"completed\""));
        Ok(())
    }

    #[test]
    fn failed_exit_on_conversational_session_still_marks_failed() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let conn = crate::store::open_store(&temp_dir.path().join("coven.sqlite3"))?;
        let mut session = session_record("session-1");
        session.status = "running".to_string();
        session.conversation_id = Some("conv-abc".to_string());
        crate::store::insert_session(&conn, &session)?;
        drop(conn);

        record_session_exit(
            temp_dir.path(),
            "session-1",
            pty_runner::PtyRunResult {
                status: "failed",
                exit_code: Some(2),
            },
        )?;

        let conn = crate::store::open_store(&temp_dir.path().join("coven.sqlite3"))?;
        let sessions = crate::store::list_sessions(&conn)?;
        assert_eq!(session_status(&sessions, "session-1"), "failed");
        Ok(())
    }

    fn session_record(id: &str) -> crate::store::SessionRecord {
        crate::store::SessionRecord {
            id: id.to_string(),
            project_root: "/repo".to_string(),
            harness: "codex".to_string(),
            title: format!("Session {id}"),
            status: "running".to_string(),
            exit_code: None,
            archived_at: None,
            created_at: "2026-04-27T07:00:00Z".to_string(),
            updated_at: "2026-04-27T07:00:00Z".to_string(),
            conversation_id: None,
            labels: Vec::new(),
            visibility: "private".to_string(),
        }
    }

    fn session_status(sessions: &[crate::store::SessionRecord], id: &str) -> String {
        sessions
            .iter()
            .find(|session| session.id == id)
            .map(|session| session.status.clone())
            .unwrap_or_default()
    }
}
