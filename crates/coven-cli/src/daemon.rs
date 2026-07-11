use std::collections::HashMap;
use std::io::Write;
#[cfg(unix)]
use std::net::{SocketAddr, TcpListener, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
#[cfg(unix)]
use std::ffi::CString;
use std::io::{BufRead, BufReader, Read};
#[cfg(unix)]
use std::os::unix::{
    ffi::OsStrExt,
    fs::{FileTypeExt, MetadataExt, PermissionsExt},
    net::{UnixListener, UnixStream},
    prelude::AsRawFd,
};

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

/// Claude Code shows a "Do you trust the files in this folder?" dialog the
/// first time it opens an *interactive* session in a directory it hasn't seen.
/// That dialog is NOT governed by `--permission-mode`, so an unattended cave
/// task session launched in a fresh directory stalls on it. (Only the
/// interactive TUI path hits this — `-p`/stream launches skip the trust dialog
/// per `claude --help`.) Pre-seed the trust decision in `~/.claude.json` — the
/// same state the dialog writes on "Yes" — so these sessions start cleanly.
fn ensure_claude_trusts_dir(dir: &str) {
    let Some(home) = dirs_next::home_dir() else {
        return;
    };
    ensure_dir_trusted_in_config(&home.join(".claude.json"), dir);
}

/// Core of [`ensure_claude_trusts_dir`], split out so it can be unit-tested
/// against an arbitrary config path. Best-effort: every failure is swallowed
/// so trust-seeding can never block a launch. Only writes when the directory
/// isn't already trusted, to stay off this shared file (Claude Code rewrites
/// it constantly) in the common case.
fn ensure_dir_trusted_in_config(config_path: &std::path::Path, dir: &str) {
    // Claude Code keys `projects` by the canonicalized absolute path
    // (e.g. macOS resolves `/tmp/x` to `/private/tmp/x`). Match that so the
    // seeded entry is the one the trust check actually looks up.
    let key = std::fs::canonicalize(dir)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| dir.to_string());

    let mut root: serde_json::Value = std::fs::read_to_string(config_path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| serde_json::json!({}));

    let already_trusted = root
        .get("projects")
        .and_then(|p| p.get(&key))
        .and_then(|e| e.get("hasTrustDialogAccepted"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    if already_trusted {
        return;
    }

    let Some(obj) = root.as_object_mut() else {
        return;
    };
    let Some(projects) = obj
        .entry("projects")
        .or_insert_with(|| serde_json::json!({}))
        .as_object_mut()
    else {
        return;
    };
    let Some(entry) = projects
        .entry(key)
        .or_insert_with(|| serde_json::json!({}))
        .as_object_mut()
    else {
        return;
    };
    entry.insert(
        "hasTrustDialogAccepted".to_string(),
        serde_json::Value::Bool(true),
    );

    let Ok(serialized) = serde_json::to_string(&root) else {
        return;
    };
    // Atomic write: a uniquely-named temp in the same dir + rename, mirroring
    // Claude Code's own update strategy so a concurrent writer never sees a
    // half-written config. The temp inherits 0600 so we never widen the
    // permissions of a file that can hold credentials.
    let seq = CLAUDE_JSON_TMP_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp = config_path.with_file_name(format!(
        ".claude.json.coven-{}-{}.tmp",
        std::process::id(),
        seq
    ));
    if std::fs::write(&tmp, serialized).is_err() {
        return;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
    }
    if std::fs::rename(&tmp, config_path).is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
}

static CLAUDE_JSON_TMP_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

impl SessionRuntime for LiveSessionRuntime {
    fn launch_session(&self, launch: &SessionLaunch) -> Result<()> {
        let familiar_ctx = match (&self.coven_home, launch.familiar_id.as_deref()) {
            (Some(home), familiar_id) => {
                crate::familiar_identity::resolve_optional(home, familiar_id)?
            }
            (None, Some(familiar_id)) => {
                anyhow::bail!("cannot resolve familiar `{familiar_id}` without COVEN_HOME")
            }
            (None, None) => None,
        };
        let command = pty_runner::build_harness_command_with_conversation(
            &launch.harness,
            &launch.prompt,
            Path::new(&launch.cwd),
            launch.launch_mode,
            launch.conversation.as_ref(),
            familiar_ctx.as_ref(),
            // The daemon launch path does not carry model/think/speed selection
            // yet; `coven run` drives those foreground flags.
            crate::harness::HarnessLaunchOptions::default(),
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
            let piped = pty_runner::spawn_piped_with_observer(&command, observer, true)?;
            let mut killer_box = piped_killer(piped.pid);
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

        // ConPTY can terminate one-shot `codex exec` children immediately on
        // Windows (observed as u32::MAX with no output). These sessions do not
        // need a terminal: the prompt is already in argv and stdout/stderr are
        // machine-drained. Ordinary pipes match direct `codex exec`, preserve
        // output observation, and let the child reach a real exit status.
        #[cfg(windows)]
        if launch.launch_mode == crate::harness::HarnessLaunchMode::NonInteractive {
            let piped = pty_runner::spawn_piped_with_observer(&command, observer, false)?;
            let killer = piped_killer(piped.pid);
            return self.register_kind(
                launch.id.clone(),
                LiveSessionKind::Pty,
                piped.input,
                killer,
            );
        }

        // Interactive claude launches hit the workspace trust dialog (not
        // covered by `--permission-mode`); pre-trust the cwd so unattended
        // task sessions don't stall on it. No-op for other harnesses and for
        // `-p`/stream modes, which skip the dialog.
        if launch.harness == "claude"
            && launch.launch_mode == crate::harness::HarnessLaunchMode::Interactive
        {
            ensure_claude_trusts_dir(&launch.cwd);
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

fn piped_killer(pid: u32) -> Box<dyn RuntimeKiller> {
    #[cfg(windows)]
    let job_handle = {
        use windows_sys::Win32::{
            Foundation::INVALID_HANDLE_VALUE,
            System::{
                JobObjects::{AssignProcessToJobObject, CreateJobObjectW},
                Threading::{OpenProcess, PROCESS_ALL_ACCESS},
            },
        };
        // SAFETY: Windows API calls; handles are owned by PipedKiller.
        unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job != INVALID_HANDLE_VALUE && job != 0 as _ {
                let ph = OpenProcess(PROCESS_ALL_ACCESS, 0, pid);
                if ph != INVALID_HANDLE_VALUE && ph != 0 as _ {
                    AssignProcessToJobObject(job, ph);
                    windows_sys::Win32::Foundation::CloseHandle(ph);
                }
                Some(job)
            } else {
                None
            }
        }
    };
    Box::new(PipedKiller {
        pid,
        #[cfg(windows)]
        job_handle,
    })
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
    /// On Windows, a Job Object handle that owns the child process tree.
    /// Calling TerminateJobObject on it kills the child and all descendants.
    #[cfg(windows)]
    job_handle: Option<windows_sys::Win32::Foundation::HANDLE>,
}

#[cfg(windows)]
unsafe impl Send for PipedKiller {}

#[cfg(windows)]
impl Drop for PipedKiller {
    fn drop(&mut self) {
        if let Some(h) = self.job_handle.take() {
            // SAFETY: h is a valid handle owned by this struct.
            unsafe { windows_sys::Win32::Foundation::CloseHandle(h) };
        }
    }
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

    #[cfg(windows)]
    fn kill(&mut self) -> Result<()> {
        use windows_sys::Win32::{
            Foundation::INVALID_HANDLE_VALUE,
            System::Threading::{OpenProcess, TerminateProcess, PROCESS_TERMINATE},
        };
        // Prefer Job Object kill (terminates the whole child tree).
        if let Some(h) = self.job_handle.take() {
            // SAFETY: h is a valid job handle; exit code 1 is conventional.
            let rc = unsafe { windows_sys::Win32::System::JobObjects::TerminateJobObject(h, 1) };
            unsafe { windows_sys::Win32::Foundation::CloseHandle(h) };
            if rc == 0 {
                // Fall back to direct TerminateProcess on the root pid.
                unsafe {
                    let ph = OpenProcess(PROCESS_TERMINATE, 0, self.pid);
                    if ph != INVALID_HANDLE_VALUE && ph != 0 as _ {
                        TerminateProcess(ph, 1);
                        windows_sys::Win32::Foundation::CloseHandle(ph);
                    }
                }
            }
            return Ok(());
        }
        // No job object — fall back to TerminateProcess on the root pid.
        unsafe {
            let ph = OpenProcess(PROCESS_TERMINATE, 0, self.pid);
            if ph == INVALID_HANDLE_VALUE || ph == 0 as _ {
                return Ok(()); // Already gone.
            }
            TerminateProcess(ph, 1);
            windows_sys::Win32::Foundation::CloseHandle(ph);
        }
        Ok(())
    }

    #[cfg(not(any(unix, windows)))]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
enum DaemonIpcPlatform {
    Unix,
    Windows,
}

fn daemon_windows_pipe_name(coven_home: &Path) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    coven_home.to_string_lossy().hash(&mut h);
    format!("coven-daemon-{:016x}.sock", h.finish())
}

fn daemon_startup_status_socket_for_platform(
    coven_home: &Path,
    platform: DaemonIpcPlatform,
) -> String {
    match platform {
        DaemonIpcPlatform::Unix => daemon_socket_path(coven_home)
            .to_string_lossy()
            .into_owned(),
        DaemonIpcPlatform::Windows => daemon_windows_pipe_name(coven_home),
    }
}

fn daemon_startup_status_socket(coven_home: &Path) -> String {
    #[cfg(windows)]
    {
        daemon_startup_status_socket_for_platform(coven_home, DaemonIpcPlatform::Windows)
    }
    #[cfg(not(windows))]
    {
        daemon_startup_status_socket_for_platform(coven_home, DaemonIpcPlatform::Unix)
    }
}

// Fail closed when daemon state already exists but is owned by a different
// user: a path we do not own could have been planted by another local user to
// capture the socket, status, or SQLite ledger. See docs/AUTH.md
// "Current hardening gap" — COVEN_HOME and the socket must be owned by the
// current user. Kept pure (uid passed in) so the refusal is unit-testable
// without a root-owned fixture.
#[cfg(unix)]
fn check_owned_by_current_user(path: &Path, owner_uid: u32, euid: u32) -> Result<()> {
    if owner_uid != euid {
        anyhow::bail!(
            "refusing to use {}: it is owned by uid {owner_uid}, not the current user (uid {euid})",
            path.display()
        );
    }
    Ok(())
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
        // SAFETY: geteuid() only reads the calling process's effective uid and
        // cannot fail.
        check_owned_by_current_user(coven_home, metadata.uid(), unsafe { libc::geteuid() })?;
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
    let child = background_server_command(&spec)
        .spawn()
        .with_context(|| format!("failed to start Coven daemon {}", spec.program.display()))?;
    let status = DaemonStatus {
        pid: child.id(),
        started_at,
        socket: daemon_startup_status_socket(coven_home),
    };
    write_status(coven_home, &status)?;
    Ok(status)
}

fn background_server_command(spec: &DaemonSpawnSpec) -> Command {
    let mut command = Command::new(&spec.program);
    command
        .args(&spec.args)
        .env("COVEN_HOME", &spec.coven_home)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    configure_background_server_command(&mut command);
    command
}

#[cfg(windows)]
fn configure_background_server_command(command: &mut Command) {
    use std::os::windows::process::CommandExt;

    command.creation_flags(windows_daemon_creation_flags());
}

#[cfg(not(windows))]
fn configure_background_server_command(_command: &mut Command) {}

#[cfg(windows)]
fn windows_daemon_creation_flags() -> u32 {
    use windows_sys::Win32::System::Threading::{CREATE_NEW_PROCESS_GROUP, DETACHED_PROCESS};

    DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP
}

pub fn ensure_background_server(
    coven_home: &Path,
    current_exe: &Path,
    started_at: String,
) -> Result<DaemonStatus> {
    let _lock = acquire_daemon_lifecycle_lock(coven_home)?;
    let status = ensure_background_server_with_controllers(
        coven_home,
        current_exe,
        started_at,
        &SystemDaemonStopController,
        &SystemDaemonStartController,
    )?;
    #[cfg(unix)]
    cleanup_unreachable_duplicate_daemons(coven_home, status.pid);
    Ok(status)
}

fn daemon_lifecycle_lock_path(coven_home: &Path) -> PathBuf {
    coven_home.join("daemon.lock")
}

#[cfg(unix)]
fn cleanup_unreachable_duplicate_daemons(coven_home: &Path, active_pid: u32) {
    use sysinfo::{Pid, ProcessesToUpdate, Signal, System};

    let mut sys = System::new_all();
    sys.refresh_all();
    let candidates: Vec<(Pid, u64, Vec<std::ffi::OsString>)> = sys
        .processes()
        .iter()
        .filter_map(|(pid, process)| {
            let pid_u32 = pid.as_u32();
            if pid_u32 == active_pid
                || !process_is_coven_daemon_serve(process.cmd())
                || !process_coven_home_matches(process.environ(), coven_home)
            {
                return None;
            }
            Some((*pid, process.start_time(), process.cmd().to_vec()))
        })
        .collect();

    for (pid, start_time, cmd) in candidates {
        sys.refresh_processes(ProcessesToUpdate::Some(&[pid]), true);
        let Some(process) = sys.process(pid) else {
            continue;
        };
        if process.start_time() != start_time
            || process.cmd() != cmd.as_slice()
            || !process_is_coven_daemon_serve(process.cmd())
            || !process_coven_home_matches(process.environ(), coven_home)
        {
            continue;
        }
        match process.kill_with(Signal::Term) {
            Some(true) => append_daemon_recovery_log(
                coven_home,
                &format!(
                    "terminated unreachable duplicate daemon pid={} active_pid={}",
                    pid.as_u32(),
                    active_pid
                ),
            ),
            Some(false) | None => append_daemon_recovery_log(
                coven_home,
                &format!(
                    "failed to terminate unreachable duplicate daemon pid={} active_pid={}",
                    pid.as_u32(),
                    active_pid
                ),
            ),
        }
    }
}

#[cfg(unix)]
fn process_is_coven_daemon_serve(cmd: &[std::ffi::OsString]) -> bool {
    cmd.windows(2)
        .any(|pair| pair[0].as_os_str() == "daemon" && pair[1].as_os_str() == "serve")
}

#[cfg(unix)]
fn process_coven_home_matches(environ: &[std::ffi::OsString], coven_home: &Path) -> bool {
    environ.iter().any(|entry| {
        let bytes = entry.as_os_str().as_bytes();
        bytes
            .strip_prefix(b"COVEN_HOME=")
            .is_some_and(|value| value == coven_home.as_os_str().as_bytes())
    })
}

#[cfg(unix)]
struct DaemonLifecycleLock {
    file: std::fs::File,
}

#[cfg(unix)]
impl Drop for DaemonLifecycleLock {
    fn drop(&mut self) {
        unsafe {
            libc::flock(self.file.as_raw_fd(), libc::LOCK_UN);
        }
    }
}

#[cfg(unix)]
fn acquire_daemon_lifecycle_lock(coven_home: &Path) -> Result<DaemonLifecycleLock> {
    ensure_private_coven_home(coven_home)?;
    let lock_path = daemon_lifecycle_lock_path(coven_home);
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&lock_path)
        .with_context(|| {
            format!(
                "failed to open daemon lifecycle lock {}",
                lock_path.display()
            )
        })?;
    std::fs::set_permissions(&lock_path, std::fs::Permissions::from_mode(0o600)).with_context(
        || {
            format!(
                "failed to set daemon lifecycle lock permissions {}",
                lock_path.display()
            )
        },
    )?;
    let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) };
    if rc != 0 {
        anyhow::bail!(
            "failed to lock daemon lifecycle {}: {}",
            lock_path.display(),
            std::io::Error::last_os_error()
        );
    }
    Ok(DaemonLifecycleLock { file })
}

#[cfg(not(unix))]
struct DaemonLifecycleLock;

#[cfg(not(unix))]
fn acquire_daemon_lifecycle_lock(coven_home: &Path) -> Result<DaemonLifecycleLock> {
    ensure_private_coven_home(coven_home)?;
    Ok(DaemonLifecycleLock)
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
    let _lock = acquire_daemon_lifecycle_lock(coven_home)?;
    stop_background_server_with_controller(coven_home, &SystemDaemonStopController)
}

pub fn restart_background_server(
    coven_home: &Path,
    current_exe: &Path,
    started_at: String,
) -> Result<(bool, DaemonStatus)> {
    let _lock = acquire_daemon_lifecycle_lock(coven_home)?;
    let was_running =
        stop_background_server_with_controller(coven_home, &SystemDaemonStopController)?;
    let status = start_background_server(coven_home, current_exe, started_at)?;
    if !SystemDaemonStartController.wait_for_running_daemon(&status, Duration::from_secs(2)) {
        anyhow::bail!(
            "started Coven daemon pid {} but its socket did not become ready",
            status.pid
        );
    }
    #[cfg(unix)]
    cleanup_unreachable_duplicate_daemons(coven_home, status.pid);
    Ok((was_running, status))
}

pub fn background_server_status(coven_home: &Path) -> Result<Option<DaemonStatusState>> {
    background_server_status_with_controller(coven_home, &SystemDaemonStopController)
}

trait DaemonStopController {
    fn signal_term(&self, pid: u32) -> Result<()>;
    fn pid_is_alive(&self, pid: u32) -> bool;
    fn wait_for_exit(&self, pid: u32, timeout: Duration) -> bool;
    fn status_matches_running_daemon(&self, status: &DaemonStatus) -> bool;
    fn status_from_default_socket(&self, coven_home: &Path) -> Result<Option<DaemonStatus>> {
        let _ = coven_home;
        Ok(None)
    }
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
        #[cfg(windows)]
        {
            let deadline = Instant::now() + timeout;
            while Instant::now() < deadline {
                if daemon_health_reports_pid_windows(&status.socket, status.pid).unwrap_or(false) {
                    return true;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            daemon_health_reports_pid_windows(&status.socket, status.pid).unwrap_or(false)
        }
        #[cfg(not(any(unix, windows)))]
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
        #[cfg(windows)]
        {
            use windows_sys::Win32::{
                Foundation::{CloseHandle, INVALID_HANDLE_VALUE},
                System::Threading::{OpenProcess, TerminateProcess, PROCESS_TERMINATE},
            };
            // SAFETY: Windows API calls; handle is closed immediately.
            unsafe {
                let handle = OpenProcess(PROCESS_TERMINATE, 0, pid);
                if handle == INVALID_HANDLE_VALUE || handle == 0 as _ {
                    // Process already gone — treat as success.
                    return Ok(());
                }
                let rc = TerminateProcess(handle, 1);
                CloseHandle(handle);
                if rc == 0 {
                    anyhow::bail!(
                        "failed to terminate Coven daemon pid {pid}: {}",
                        std::io::Error::last_os_error()
                    );
                }
            }
            Ok(())
        }
        #[cfg(not(any(unix, windows)))]
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
        #[cfg(windows)]
        {
            use windows_sys::Win32::{
                Foundation::{CloseHandle, INVALID_HANDLE_VALUE, STILL_ACTIVE},
                System::Threading::{GetExitCodeProcess, OpenProcess, PROCESS_QUERY_INFORMATION},
            };
            // SAFETY: handle is closed immediately after query.
            unsafe {
                let handle = OpenProcess(PROCESS_QUERY_INFORMATION, 0, pid);
                if handle == INVALID_HANDLE_VALUE || handle == 0 as _ {
                    return false;
                }
                let mut exit_code: u32 = 0;
                let ok = GetExitCodeProcess(handle, &mut exit_code);
                CloseHandle(handle);
                ok != 0 && exit_code == STILL_ACTIVE as u32
            }
        }
        #[cfg(not(any(unix, windows)))]
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
        #[cfg(windows)]
        {
            // On Windows, probe the named pipe health endpoint.
            daemon_health_reports_pid_windows(&status.socket, status.pid).unwrap_or(false)
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = status;
            true
        }
    }

    fn status_from_default_socket(&self, coven_home: &Path) -> Result<Option<DaemonStatus>> {
        daemon_status_from_default_socket(coven_home)
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
        return recover_missing_status_from_default_socket(coven_home, controller);
    };

    if controller.status_matches_running_daemon(&status) {
        return Ok(Some(DaemonStatusState::Running(status)));
    }

    if controller.pid_is_alive(status.pid) {
        return Ok(Some(DaemonStatusState::Stale(status)));
    }

    if let Some(recovered) = recover_missing_status_from_default_socket(coven_home, controller)? {
        return Ok(Some(recovered));
    }

    clear_status_and_socket(coven_home)?;
    Ok(None)
}

fn recover_missing_status_from_default_socket(
    coven_home: &Path,
    controller: &dyn DaemonStopController,
) -> Result<Option<DaemonStatusState>> {
    let Some(status) = controller.status_from_default_socket(coven_home)? else {
        return Ok(None);
    };

    if controller.status_matches_running_daemon(&status) {
        write_status(coven_home, &status)?;
        return Ok(Some(DaemonStatusState::Running(status)));
    }

    if controller.pid_is_alive(status.pid) {
        return Ok(Some(DaemonStatusState::Stale(status)));
    }

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

#[cfg(windows)]
fn daemon_status_from_default_socket(coven_home: &Path) -> Result<Option<DaemonStatus>> {
    daemon_status_from_windows_pipe(&daemon_windows_pipe_name(coven_home))
}

#[cfg(not(any(unix, windows)))]
fn daemon_status_from_default_socket(coven_home: &Path) -> Result<Option<DaemonStatus>> {
    let _ = coven_home;
    Ok(None)
}

#[cfg(windows)]
fn daemon_health_reports_pid_windows(pipe_name: &str, expected_pid: u32) -> Result<bool> {
    Ok(daemon_status_from_windows_pipe(pipe_name)?
        .map(|status| status.pid == expected_pid)
        .unwrap_or(false))
}

#[cfg(windows)]
fn daemon_status_from_windows_pipe(pipe_name: &str) -> Result<Option<DaemonStatus>> {
    use interprocess::{
        local_socket::{prelude::*, ConnectOptions, GenericNamespaced},
        ConnectWaitMode,
    };
    use std::io::Write;

    let name = pipe_name
        .to_ns_name::<GenericNamespaced>()
        .context("failed to parse pipe name for health check")?;
    let mut stream = match ConnectOptions::new()
        .name(name)
        .wait_mode(ConnectWaitMode::Timeout(Duration::from_secs(2)))
        .connect_sync()
    {
        Ok(s) => s,
        Err(_) => return Ok(None),
    };
    stream
        .write_all(b"GET /health HTTP/1.1\r\nHost: coven\r\nContent-Length: 0\r\n\r\n")
        .context("failed to write Windows health request")?;
    stream
        .flush()
        .context("failed to flush Windows health request")?;
    // Windows named pipes do not support read timeouts in interprocess.
    // Supervise the blocking framed read from another thread so doctor/status
    // still return deterministically when a daemon accepts but never replies.
    // Reading to EOF is incorrect for HTTP and used to hang indefinitely when
    // the daemon kept the pipe instance alive after writing its response.
    let (_status, body) =
        read_windows_pipe_http_response(stream, Duration::from_secs(2), MAX_SOCKET_BODY_BYTES)?;
    let body: DaemonHealthStatus =
        serde_json::from_slice(&body).context("failed to parse Windows health response")?;
    if body.ok {
        Ok(body.daemon)
    } else {
        Ok(None)
    }
}

#[cfg(windows)]
pub(crate) fn read_windows_pipe_http_response(
    mut stream: interprocess::local_socket::Stream,
    timeout: Duration,
    max_body_bytes: usize,
) -> Result<(u16, Vec<u8>)> {
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    std::thread::Builder::new()
        .name("coven-pipe-response".into())
        .spawn(move || {
            let result = read_http_response_with_deadline(&mut stream, timeout, max_body_bytes);
            let _ = tx.send(result);
        })
        .context("failed to spawn Windows pipe response reader")?;
    match rx.recv_timeout(timeout) {
        Ok(result) => result,
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
            anyhow::bail!("timed out reading Coven daemon HTTP response")
        }
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
            anyhow::bail!("Coven daemon HTTP response reader stopped unexpectedly")
        }
    }
}

/// Read one HTTP response without relying on the peer closing the transport.
///
/// Local sockets and especially Windows named pipes may remain open after a
/// response. HTTP/1.1 frames the body with Content-Length, so consume exactly
/// that many bytes. The reader is expected to be nonblocking; WouldBlock is
/// retried until `timeout` expires.
#[cfg(any(windows, test))]
pub(crate) fn read_http_response_with_deadline<R: Read>(
    reader: &mut R,
    timeout: Duration,
    max_body_bytes: usize,
) -> Result<(u16, Vec<u8>)> {
    const MAX_RESPONSE_HEADERS: usize = 64 * 1024;

    let deadline = Instant::now() + timeout;
    let mut received = Vec::with_capacity(1024);
    let mut chunk = [0_u8; 4096];
    let mut framing: Option<(u16, usize, usize)> = None;

    loop {
        if let Some((status, body_start, content_length)) = framing {
            if received.len() >= body_start + content_length {
                return Ok((
                    status,
                    received[body_start..body_start + content_length].to_vec(),
                ));
            }
        }
        if Instant::now() >= deadline {
            anyhow::bail!("timed out reading Coven daemon HTTP response");
        }

        match reader.read(&mut chunk) {
            Ok(0) => anyhow::bail!(
                "Coven daemon closed the connection before its HTTP response completed"
            ),
            Ok(n) => {
                received.extend_from_slice(&chunk[..n]);
                if framing.is_none() {
                    if received.len() > MAX_RESPONSE_HEADERS {
                        anyhow::bail!("Coven daemon HTTP response headers exceeded {MAX_RESPONSE_HEADERS} bytes");
                    }
                    if let Some(header_end) = find_http_header_end(&received) {
                        let status = response_status(&received[..header_end])?;
                        let content_length = response_content_length(&received[..header_end])?;
                        if content_length > max_body_bytes {
                            anyhow::bail!(
                                "Coven daemon HTTP response body exceeded {max_body_bytes} bytes"
                            );
                        }
                        framing = Some((status, header_end + 4, content_length));
                    }
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error).context("failed to read Windows health response"),
        }
    }
}

#[cfg(any(windows, test))]
fn response_status(headers: &[u8]) -> Result<u16> {
    let headers =
        std::str::from_utf8(headers).context("daemon HTTP response headers were not UTF-8")?;
    headers
        .split("\r\n")
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .context("daemon HTTP response omitted its status")?
        .parse::<u16>()
        .context("daemon HTTP response had an invalid status")
}

#[cfg(any(windows, test))]
fn find_http_header_end(bytes: &[u8]) -> Option<usize> {
    bytes.windows(4).position(|window| window == b"\r\n\r\n")
}

#[cfg(any(windows, test))]
fn response_content_length(headers: &[u8]) -> Result<usize> {
    let headers =
        std::str::from_utf8(headers).context("daemon HTTP response headers were not UTF-8")?;
    headers
        .split("\r\n")
        .skip(1)
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim())
        })
        .context("daemon HTTP response omitted Content-Length")?
        .parse::<usize>()
        .context("daemon HTTP response had an invalid Content-Length")
}

#[cfg(unix)]
fn daemon_health_reports_pid(socket: &str, expected_pid: u32) -> Result<bool> {
    Ok(daemon_status_from_health_socket(socket)?
        .map(|status| status.pid == expected_pid)
        .unwrap_or(false))
}

#[cfg(unix)]
fn daemon_status_from_health_socket(socket: &str) -> Result<Option<DaemonStatus>> {
    let socket_path = std::path::Path::new(socket);
    // Fail-closed: refuse to connect to a socket that is not owned by the
    // current effective uid. A foreign-owned socket could be an attacker's
    // listener planted at the expected path.
    if let Ok(metadata) = std::fs::symlink_metadata(socket_path) {
        use std::os::unix::fs::MetadataExt;
        // SAFETY: geteuid() only reads the effective uid and cannot fail.
        let euid = unsafe { libc::geteuid() };
        check_owned_by_current_user(socket_path, metadata.uid(), euid)?;
    }
    let mut stream = match UnixStream::connect(socket) {
        Ok(stream) => stream,
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
            ) =>
        {
            return Ok(None);
        }
        Err(error) => {
            return Err(error)
                .with_context(|| format!("failed to connect to Coven daemon socket {socket}"));
        }
    };
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
#[cfg(unix)]
pub const TCP_IO_TIMEOUT: Duration = Duration::from_secs(30);
#[cfg(unix)]
pub const MAX_TCP_BODY_BYTES: usize = 1024 * 1024;

/// Body cap for Unix socket and Windows named pipe transports.
/// These transports are local-only, so the risk is lower than TCP, but a
/// runaway or hostile local process should not be able to allocate unbounded
/// memory. 4 MiB is generous for any legitimate request payload.
pub const MAX_SOCKET_BODY_BYTES: usize = 4 * 1024 * 1024;

/// I/O timeout for Unix socket connections. The Windows named-pipe backend
/// does not support transport timeouts; those requests use isolated handler
/// threads and client-side response deadlines instead.
#[cfg_attr(windows, allow(dead_code))]
pub const SOCKET_IO_TIMEOUT: Duration = Duration::from_secs(60);

#[cfg(unix)]
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

#[cfg(unix)]
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

/// True when a per-connection error is just the client hanging up mid-exchange
/// — a disconnected browser SSE stream, an abandoned poll, a proxy that timed
/// out — rather than a genuine server-side fault. These are routine under
/// Coven's SSE + polling load and must not spam the daemon log or
/// `daemon-recovery.log`; see the accept-loop notes on "broken pipe" pile-ups.
///
/// Walks the whole `anyhow` chain because the response-write path wraps the
/// underlying `io::Error` in `.context(...)`, so the disconnect kind is not at
/// the head of the chain.
fn is_client_disconnect(error: &anyhow::Error) -> bool {
    use std::io::ErrorKind;
    error.chain().any(|cause| {
        cause.downcast_ref::<std::io::Error>().is_some_and(|io| {
            matches!(
                io.kind(),
                ErrorKind::BrokenPipe
                    | ErrorKind::ConnectionReset
                    | ErrorKind::ConnectionAborted
                    | ErrorKind::UnexpectedEof
                    | ErrorKind::WriteZero
            )
        })
    })
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
        true,
    )
}

#[cfg(unix)]
pub fn bind_api_socket(coven_home: &Path) -> Result<UnixListener> {
    ensure_private_coven_home(coven_home)?;
    let socket_path = daemon_socket_path(coven_home);
    // Fail closed if the socket path would resolve outside the trusted state
    // directory: socket creation and cleanup must never cross the COVEN_HOME
    // boundary. daemon_socket_path() builds `<coven_home>/coven.sock`, so this is
    // an explicit guard so a future change can't let it escape. See docs/AUTH.md
    // "Current hardening gap".
    if socket_path.parent() != Some(coven_home) {
        anyhow::bail!(
            "refusing to bind Coven API socket {}: resolves outside Coven home {}",
            socket_path.display(),
            coven_home.display()
        );
    }
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
            // SAFETY: geteuid() only reads the effective uid and cannot fail.
            check_owned_by_current_user(&socket_path, metadata.uid(), unsafe { libc::geteuid() })?;
            // Break the socket-takeover orphan cycle: before unlinking and
            // rebinding, probe the existing socket's /health endpoint. If a live
            // daemon is already serving here, refuse to take over. Removing its
            // socket inode would not stop it — the incumbent keeps running on the
            // now-unlinked inode (no longer reachable by new clients, but never
            // exiting), leaking a zombie that competes for the path. Repeated
            // takeovers are how a single daemon turns into dozens. Only a dead or
            // stale socket — connection refused, or a non-ok health body — may be
            // reclaimed. See OpenCoven/coven#197 and docs/AUTH.md.
            if let Ok(Some(incumbent)) =
                daemon_status_from_health_socket(&socket_path.to_string_lossy())
            {
                anyhow::bail!(
                    "a healthy Coven daemon (pid {}) is already serving {}; refusing to take over",
                    incumbent.pid,
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
pub fn daemon_recovery_log_path(coven_home: &Path) -> PathBuf {
    coven_home.join("daemon-recovery.log")
}

#[cfg(unix)]
pub fn append_daemon_recovery_log(coven_home: &Path, msg: &str) {
    let path = daemon_recovery_log_path(coven_home);
    let line = format!("[{}] {}\n", crate::api::current_timestamp(), msg);
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = f.write_all(line.as_bytes());
    }
}

/// Cleans up the Unix-domain socket file and `daemon.json` when the daemon
/// exits via any path that runs destructors — normal return, `Err` propagation,
/// or panic unwinding. This is what prevents orphaned `~/.coven/coven.sock`
/// files from appearing when the daemon crashes (see OpenCoven/coven#197).
/// SIGKILL and `_exit` bypass Drop; the explicit signal handler covers SIGTERM
/// / SIGINT / SIGHUP.
#[cfg(unix)]
struct ShutdownGuard {
    socket_path: PathBuf,
    status_path: PathBuf,
    pid: u32,
}

#[cfg(unix)]
impl Drop for ShutdownGuard {
    fn drop(&mut self) {
        if daemon_status_file_pid(&self.status_path) == Some(self.pid) {
            let _ = std::fs::remove_file(&self.socket_path);
            let _ = std::fs::remove_file(&self.status_path);
        }
    }
}

#[cfg(unix)]
fn daemon_status_file_pid(status_path: &Path) -> Option<u32> {
    std::fs::read_to_string(status_path)
        .ok()
        .and_then(|json| serde_json::from_str::<DaemonStatus>(&json).ok())
        .map(|status| status.pid)
}

#[cfg(unix)]
extern "C" fn handle_termination_signal(sig: libc::c_int) {
    // Only async-signal-safe calls below. Anything that might allocate or take
    // a lock is forbidden. The ownership-aware ShutdownGuard handles normal
    // unwinding cleanup; signal exits leave stale metadata for the next
    // status/start command to validate and clear.
    let msg: &[u8] = b"coven daemon: received termination signal, exiting\n";
    unsafe {
        libc::write(
            libc::STDERR_FILENO,
            msg.as_ptr() as *const libc::c_void,
            msg.len(),
        );
        libc::_exit(128 + sig);
    }
}

#[cfg(unix)]
fn install_termination_signal_handlers(socket_path: &Path, status_path: &Path) -> Result<()> {
    let _socket_cstr = CString::new(socket_path.as_os_str().as_bytes())
        .context("daemon socket path contained an interior NUL")?;
    let _status_cstr = CString::new(status_path.as_os_str().as_bytes())
        .context("daemon status path contained an interior NUL")?;

    for sig in [libc::SIGTERM, libc::SIGINT, libc::SIGHUP] {
        // SAFETY: sigaction is the documented POSIX API for installing signal
        // handlers; we pass a zero-initialized struct, our handler pointer,
        // and an empty signal mask. Failure returns -1 and sets errno.
        unsafe {
            let mut sa: libc::sigaction = std::mem::zeroed();
            sa.sa_sigaction = handle_termination_signal as *const () as usize;
            libc::sigemptyset(&mut sa.sa_mask);
            // Intentionally no SA_RESTART: we want blocking syscalls (accept)
            // to return EINTR so the loop can exit promptly. The handler
            // itself calls _exit, so EINTR handling in the loop is academic,
            // but the principle is right.
            sa.sa_flags = 0;
            if libc::sigaction(sig, &sa, std::ptr::null_mut()) != 0 {
                return Err(std::io::Error::last_os_error())
                    .with_context(|| format!("failed to install signal handler for signal {sig}"));
            }
        }
    }
    Ok(())
}

#[cfg(unix)]
fn install_daemon_panic_hook(coven_home: &Path, socket_path: &Path, status_path: &Path) {
    let coven_home = coven_home.to_path_buf();
    let socket_path = socket_path.to_path_buf();
    let status_path = status_path.to_path_buf();
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        // Capture the panic location and payload before any potentially
        // failing IO so the original message always lands on stderr.
        prev(info);
        let backtrace = std::backtrace::Backtrace::force_capture();
        let payload = format!(
            "daemon panic: {info}\nbacktrace:\n{backtrace}\n----------------------------------------"
        );
        append_daemon_recovery_log(&coven_home, &payload);
        // Best-effort cleanup; Drop on ShutdownGuard would also run during
        // unwinding, but a panic from inside Drop or from a thread that does
        // not own the guard would otherwise leave the files behind.
        let _ = std::fs::remove_file(&socket_path);
        let _ = std::fs::remove_file(&status_path);
    }));
}

/// Path of the serve-lifetime single-writer lock. Distinct from the
/// `daemon.lock` *lifecycle* lock that `ensure_background_server` holds only
/// across a start/stop — this one is held by the `serve` process for its entire
/// life, so it must be a separate file or the two would deadlock at startup.
#[cfg(unix)]
fn daemon_serve_lock_path(coven_home: &Path) -> PathBuf {
    coven_home.join("daemon-serve.lock")
}

/// Acquire an exclusive, process-lifetime advisory lock so at most one `serve`
/// process ever runs against a given Coven home.
///
/// `ensure_background_server` already serializes `daemon start`/`stop` and prunes
/// unreachable duplicates — but a `coven daemon serve` run *directly* (e.g. from
/// a dev build) bypasses all of that. The socket-takeover guard in
/// `bind_api_socket` only refuses a *healthy* incumbent that answers `/health`;
/// a daemon that is alive but wedged would have its socket reclaimed, leaving
/// two processes writing one SQLite store — the loser then fails the
/// `events_fts` backfill with "database is locked". This OS lock is independent
/// of socket health and of the start path: a live incumbent still holds it, so a
/// duplicate fails fast with a clear message. `flock` releases automatically
/// when the fd closes — normal exit, panic, or SIGKILL — so it never wedges shut.
#[cfg(unix)]
fn acquire_serve_lock(coven_home: &Path) -> Result<std::fs::File> {
    std::fs::create_dir_all(coven_home)
        .with_context(|| format!("failed to create Coven home {}", coven_home.display()))?;
    let path = daemon_serve_lock_path(coven_home);
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&path)
        .with_context(|| format!("failed to open serve lock {}", path.display()))?;
    // SAFETY: flock only operates on the provided fd; it cannot corrupt memory.
    let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if rc != 0 {
        let err = std::io::Error::last_os_error();
        if err.raw_os_error() == Some(libc::EWOULDBLOCK) {
            anyhow::bail!(
                "another Coven daemon is already serving this home (holds {}); refusing to \
                 start a second daemon, which would contend for the SQLite store",
                path.display()
            );
        }
        return Err(err)
            .with_context(|| format!("failed to acquire serve lock {}", path.display()));
    }
    Ok(file)
}

#[cfg(unix)]
pub fn serve_forever(coven_home: &Path, started_at: String, tcp_addr: Option<&str>) -> Result<()> {
    use std::sync::Arc;
    // First thing, before touching the socket or the store: take the
    // single-writer serve lock and hold it for the whole process lifetime. It
    // is the authoritative guard against two daemons writing one SQLite store —
    // catching the wedged-but-alive incumbent the socket guard can't, and the
    // direct `daemon serve` path that bypasses ensure_background_server.
    let _serve_lock = acquire_serve_lock(coven_home)?;
    let status = DaemonStatus {
        pid: std::process::id(),
        started_at: started_at.clone(),
        socket: daemon_socket_path(coven_home)
            .to_string_lossy()
            .into_owned(),
    };
    let socket_path = daemon_socket_path(coven_home);
    let status_path = daemon_status_path(coven_home);
    // Install the shutdown hooks before anything else that can fail: a panic
    // during recovery or bind would otherwise leave a socket file behind.
    install_daemon_panic_hook(coven_home, &socket_path, &status_path);
    install_termination_signal_handlers(&socket_path, &status_path)?;
    // Acquire the socket BEFORE claiming any on-disk daemon state. bind_api_socket
    // refuses to take over a socket a healthy daemon already owns; if it bails we
    // must not yet have written daemon.json or armed the ShutdownGuard, or the
    // guard would delete the incumbent's status file and unlink its live socket on
    // our way out — re-orphaning the very daemon we declined to replace.
    let unix_listener = bind_api_socket(coven_home)?;
    write_status(coven_home, &status)?;
    let _shutdown_guard = ShutdownGuard {
        socket_path: socket_path.clone(),
        status_path: status_path.clone(),
        pid: status.pid,
    };
    append_daemon_recovery_log(
        coven_home,
        &format!(
            "daemon starting pid={} socket={}",
            std::process::id(),
            socket_path.display()
        ),
    );
    recover_orphaned_sessions(coven_home, &started_at)?;
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
        // below uses the same strategy: a single malformed local request must
        // not orphan the socket file (see #197).
        std::thread::Builder::new()
            .name("coven-tcp-api".into())
            .spawn(move || loop {
                if let Err(error) = serve_next_tcp_connection(
                    &tcp_listener,
                    &tcp_home,
                    Some(tcp_status.clone()),
                    tcp_runtime.as_ref(),
                ) {
                    // A client hanging up mid-response is expected under SSE +
                    // polling load; don't log it or throttle the accept loop.
                    if !is_client_disconnect(&error) {
                        eprintln!("coven daemon: TCP connection error: {error:#}");
                        std::thread::sleep(std::time::Duration::from_millis(100));
                    }
                }
            })
            .context("failed to spawn TCP API thread")?;
    }

    // Handle each accepted connection on its own thread. The Unix accept loop
    // used to be serial — accept, then run the handler to completion before
    // accepting again — so one slow handler (a blocking session spawn, a stuck
    // filesystem op) stalled *every* subsequent request: the socket kept
    // accepting but nothing got answered, and polling clients piled up "broken
    // pipe" as they timed out. Threading the handlers fixes that. It is safe by
    // construction: the TCP path and these handlers already share one
    // `Arc<LiveSessionRuntime>`, so request handling is already concurrency-safe
    // (TCP + Unix have always run at the same time).
    //
    // In-flight handlers are capped; past the cap we serve inline so a flood
    // applies backpressure instead of spawning unbounded threads. Per-connection
    // errors stay isolated (logged, loop continues) so one malformed request
    // can't bring the daemon down or orphan the socket.
    const MAX_INFLIGHT: usize = 64;
    let inflight = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    use std::sync::atomic::Ordering;
    loop {
        let (stream, _) = match unix_listener.accept() {
            Ok(pair) => pair,
            Err(error) => {
                eprintln!("coven daemon: unix accept error: {error:#}");
                append_daemon_recovery_log(coven_home, &format!("unix accept error: {error:#}"));
                std::thread::sleep(std::time::Duration::from_millis(100));
                continue;
            }
        };
        let conn_status = Some(status.clone());

        if inflight.load(Ordering::Relaxed) >= MAX_INFLIGHT {
            // Backpressure: at capacity, serve this one on the accept thread.
            if let Err(error) =
                serve_accepted_connection(stream, coven_home, conn_status, runtime.as_ref())
            {
                if !is_client_disconnect(&error) {
                    eprintln!("coven daemon: unix connection error: {error:#}");
                    append_daemon_recovery_log(
                        coven_home,
                        &format!("unix connection error: {error:#}"),
                    );
                }
            }
            continue;
        }

        inflight.fetch_add(1, Ordering::Relaxed);
        let conn_home = coven_home.to_path_buf();
        let conn_runtime = Arc::clone(&runtime);
        let conn_inflight = Arc::clone(&inflight);
        let spawn_result = std::thread::Builder::new()
            .name("coven-unix-api".into())
            .spawn(move || {
                if let Err(error) = serve_accepted_connection(
                    stream,
                    &conn_home,
                    conn_status,
                    conn_runtime.as_ref(),
                ) {
                    if !is_client_disconnect(&error) {
                        eprintln!("coven daemon: unix connection error: {error:#}");
                        append_daemon_recovery_log(
                            &conn_home,
                            &format!("unix connection error: {error:#}"),
                        );
                    }
                }
                conn_inflight.fetch_sub(1, Ordering::Relaxed);
            });
        if let Err(error) = spawn_result {
            // Thread budget exhausted: the closure (and the connection) is
            // dropped, so undo the counter we optimistically bumped. Rare; the
            // client simply retries.
            inflight.fetch_sub(1, Ordering::Relaxed);
            eprintln!("coven daemon: failed to spawn unix handler thread: {error:#}");
            append_daemon_recovery_log(
                coven_home,
                &format!("failed to spawn unix handler thread: {error:#}"),
            );
        }
    }
}

fn handle_http_stream<R, W>(
    read: R,
    mut write: W,
    coven_home: &Path,
    status: Option<DaemonStatus>,
    runtime: &dyn SessionRuntime,
    max_body_bytes: Option<usize>,
    enforce_loopback_guard: bool,
) -> Result<()>
where
    R: Read,
    W: Write,
{
    let mut reader = BufReader::new(read);
    let request_line = read_http_request_line(&mut reader)?;
    let headers = read_http_headers(&mut reader)?;
    // On the TCP transport (loopback-only), defend against browser-driven CSRF
    // and DNS-rebinding: a real CLI/proxy client never sends a cross-origin
    // Origin, and a rebinding attack arrives with a non-loopback Host. The Unix
    // socket is filesystem-gated and skips this.
    if enforce_loopback_guard {
        if !host_is_loopback(headers.host.as_deref()) {
            return write_forbidden(&mut write, "Host header must be a loopback address.");
        }
        if let Some(origin) = headers.origin.as_deref() {
            if !origin_is_loopback(origin) {
                return write_forbidden(&mut write, "Cross-origin requests are not allowed.");
            }
        }
    }
    if let Some(max) = max_body_bytes {
        if headers.content_length > max {
            return write_payload_too_large(&mut write, max);
        }
    }
    let body = read_http_body(&mut reader, headers.content_length)?;
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

fn host_is_loopback(host: Option<&str>) -> bool {
    match host {
        Some(h) => is_loopback_host(strip_port(h.trim())),
        None => false,
    }
}

fn origin_is_loopback(origin: &str) -> bool {
    match origin.trim().split_once("://") {
        Some((_scheme, rest)) => is_loopback_host(strip_port(rest)),
        None => false,
    }
}

fn strip_port(authority: &str) -> &str {
    if let Some(rest) = authority.strip_prefix('[') {
        // IPv6 literal like [::1]:8080 -> ::1
        return rest.split(']').next().unwrap_or(rest);
    }
    authority.split(':').next().unwrap_or(authority)
}

fn is_loopback_host(host: &str) -> bool {
    // Parse as an IP and ask the address itself — never a string prefix. A prefix
    // test like `starts_with("127.")` would also accept attacker hostnames such as
    // `127.evil.com`, defeating the DNS-rebinding guard this function backs.
    if host == "localhost" {
        return true;
    }
    host.parse::<std::net::IpAddr>()
        .map(|ip| ip.is_loopback())
        .unwrap_or(false)
}

fn write_forbidden<W: Write>(write: &mut W, reason: &str) -> Result<()> {
    let body =
        format!("{{\"ok\":false,\"error\":{{\"code\":\"forbidden\",\"message\":\"{reason}\"}}}}");
    let http = format!(
        "HTTP/1.1 403 Forbidden\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    write
        .write_all(http.as_bytes())
        .context("failed to write 403 response")?;
    Ok(())
}

// Accept + serve in one call. Production no longer uses this (the accept loop
// threads each connection via serve_accepted_connection); it remains for tests
// that drive a listener end-to-end, hence cfg(test).
#[cfg(all(unix, test))]
pub fn serve_next_connection(
    listener: &UnixListener,
    coven_home: &Path,
    status: Option<DaemonStatus>,
    runtime: &dyn SessionRuntime,
) -> Result<()> {
    let (stream, _) = listener
        .accept()
        .context("failed to accept API connection")?;
    serve_accepted_connection(stream, coven_home, status, runtime)
}

/// Serve a single already-accepted Unix connection. Split out of
/// `serve_next_connection` so the accept loop can hand each connection to its
/// own thread (see `serve_forever`) — accept stays serial and cheap, while the
/// blocking request handling runs off the accept thread.
#[cfg(unix)]
fn serve_accepted_connection(
    stream: UnixStream,
    coven_home: &Path,
    status: Option<DaemonStatus>,
    runtime: &dyn SessionRuntime,
) -> Result<()> {
    // Best-effort I/O timeouts so a stalled client doesn't pin the handler
    // thread forever. These are an optimization, not a precondition: on macOS
    // setsockopt(SO_RCVTIMEO) returns EINVAL (os error 22) for some accepted
    // fds (e.g. a peer already half-closed by accept time), and a connection
    // that merely could not have a timeout applied is still serviceable. Making
    // this fatal aborted those connections and flooded the recovery log with
    // "failed to set read timeout" — to a polling client like CovenCave it looked
    // like the daemon constantly dropping. Mirror the named-pipe path, which
    // already sets these best-effort, and serve the request regardless.
    let _ = stream.set_read_timeout(Some(SOCKET_IO_TIMEOUT));
    let _ = stream.set_write_timeout(Some(SOCKET_IO_TIMEOUT));
    let read = stream.try_clone().context("failed to clone Unix stream")?;
    // Apply a body cap even on local Unix sockets: a buggy or hostile local
    // process should not be able to OOM the daemon with a huge payload.
    handle_http_stream(
        read,
        stream,
        coven_home,
        status,
        runtime,
        Some(MAX_SOCKET_BODY_BYTES),
        false,
    )
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

struct ParsedHeaders {
    content_length: usize,
    host: Option<String>,
    origin: Option<String>,
}

fn read_http_headers<R: BufRead>(reader: &mut R) -> Result<ParsedHeaders> {
    let mut headers = ParsedHeaders {
        content_length: 0,
        host: None,
        origin: None,
    };
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
            let name = name.trim();
            let value = value.trim();
            if name.eq_ignore_ascii_case("content-length") {
                headers.content_length = value.parse().context("invalid Content-Length header")?;
            } else if name.eq_ignore_ascii_case("host") {
                headers.host = Some(value.to_string());
            } else if name.eq_ignore_ascii_case("origin") {
                headers.origin = Some(value.to_string());
            }
        }
    }
    Ok(headers)
}

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

fn parse_request_line(line: &str) -> Result<(&str, &str)> {
    let mut parts = line.split_whitespace();
    let method = parts.next().context("missing HTTP method")?;
    let path = parts.next().context("missing HTTP path")?;
    Ok((method, path))
}

#[cfg(windows)]
fn owner_only_pipe_security_descriptor(
) -> Result<interprocess::os::windows::security_descriptor::SecurityDescriptor> {
    use interprocess::os::windows::security_descriptor::SecurityDescriptor;
    use widestring::U16CString;

    // D:(A;;GA;;;OW) — allow Generic All for the object owner only. This is the
    // Windows named-pipe equivalent of binding a Unix socket with mode 0600.
    let sddl = U16CString::from_str("D:(A;;GA;;;OW)")
        .context("failed to encode owner-only named pipe security descriptor")?;
    SecurityDescriptor::deserialize(&sddl)
        .context("failed to build owner-only named pipe security descriptor")
}

#[cfg(windows)]
pub(crate) fn windows_pipe_name(coven_home: &Path) -> String {
    daemon_windows_pipe_name(coven_home)
}

#[cfg(windows)]
pub fn serve_forever(coven_home: &Path, started_at: String, tcp_addr: Option<&str>) -> Result<()> {
    use interprocess::{
        local_socket::{prelude::*, GenericNamespaced, ListenerOptions},
        os::windows::local_socket::ListenerOptionsExt,
    };
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    let _ = tcp_addr; // TCP not wired on Windows in this prototype

    let status = DaemonStatus {
        pid: std::process::id(),
        started_at: started_at.clone(),
        socket: windows_pipe_name(coven_home),
    };
    let name = windows_pipe_name(coven_home)
        .to_ns_name::<GenericNamespaced>()
        .context("failed to create named pipe name")?;
    let security_descriptor = owner_only_pipe_security_descriptor()?;
    let listener = ListenerOptions::new()
        .name(name)
        .security_descriptor(security_descriptor)
        .create_sync()
        .context("failed to bind Windows named pipe")?;

    // Claim the pipe before mutating shared daemon/session state. A duplicate
    // daemon must fail at bind without replacing the incumbent's daemon.json
    // or marking sessions owned by that live daemon orphaned.
    write_status(coven_home, &status)?;
    recover_orphaned_sessions(coven_home, &started_at)?;

    let runtime = Arc::new(LiveSessionRuntime::with_coven_home(
        coven_home.to_path_buf(),
    ));

    const MAX_INFLIGHT: usize = 64;
    let inflight = Arc::new(AtomicUsize::new(0));
    for conn in listener.incoming() {
        let stream = match conn {
            Ok(s) => s,
            Err(error) => {
                eprintln!("coven daemon: pipe accept error: {error:#}");
                continue;
            }
        };
        let conn_home = coven_home.to_path_buf();
        let conn_status = status.clone();
        let conn_runtime = Arc::clone(&runtime);
        if inflight.load(Ordering::Relaxed) >= MAX_INFLIGHT {
            // Backpressure at capacity by serving on the accept thread rather
            // than allowing stalled clients to create unbounded OS threads.
            let _ = stream.set_recv_timeout(Some(SOCKET_IO_TIMEOUT));
            let _ = stream.set_send_timeout(Some(SOCKET_IO_TIMEOUT));
            if let Err(error) = handle_http_stream(
                &stream,
                &stream,
                &conn_home,
                Some(conn_status),
                conn_runtime.as_ref(),
                Some(MAX_SOCKET_BODY_BYTES),
                false,
            ) {
                if !is_client_disconnect(&error) {
                    eprintln!("coven daemon: pipe connection error: {error:#}");
                }
            }
            continue;
        }

        inflight.fetch_add(1, Ordering::Relaxed);
        let conn_inflight = Arc::clone(&inflight);
        let spawn_result = std::thread::Builder::new()
            .name("coven-windows-api".into())
            .spawn(move || {
                // Bound each transaction and isolate it so a stalled client
                // cannot block accept or starve Cave's polling requests.
                let _ = stream.set_recv_timeout(Some(SOCKET_IO_TIMEOUT));
                let _ = stream.set_send_timeout(Some(SOCKET_IO_TIMEOUT));
                if let Err(error) = handle_http_stream(
                    &stream,
                    &stream,
                    &conn_home,
                    Some(conn_status),
                    conn_runtime.as_ref(),
                    Some(MAX_SOCKET_BODY_BYTES),
                    false,
                ) {
                    if !is_client_disconnect(&error) {
                        eprintln!("coven daemon: pipe connection error: {error:#}");
                    }
                }
                conn_inflight.fetch_sub(1, Ordering::Relaxed);
            });
        if let Err(error) = spawn_result {
            inflight.fetch_sub(1, Ordering::Relaxed);
            eprintln!("coven daemon: failed to spawn pipe handler: {error:#}");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_client_disconnect_detects_wrapped_broken_pipe() {
        // The response-write path wraps the io error in `.context(...)`, so the
        // disconnect kind sits below the head of the chain — the classifier must
        // still find it.
        let io = std::io::Error::new(std::io::ErrorKind::BrokenPipe, "broken pipe");
        let wrapped = anyhow::Error::new(io).context("failed to write HTTP response");
        assert!(is_client_disconnect(&wrapped));

        for kind in [
            std::io::ErrorKind::ConnectionReset,
            std::io::ErrorKind::ConnectionAborted,
            std::io::ErrorKind::UnexpectedEof,
            std::io::ErrorKind::WriteZero,
        ] {
            let err = anyhow::Error::new(std::io::Error::new(kind, "peer gone"));
            assert!(is_client_disconnect(&err), "{kind:?} should be benign");
        }
    }

    struct NeverReady;

    impl Read for NeverReady {
        fn read(&mut self, _buf: &mut [u8]) -> std::io::Result<usize> {
            Err(std::io::Error::from(std::io::ErrorKind::WouldBlock))
        }
    }

    #[test]
    fn is_client_disconnect_ignores_real_faults() {
        // A genuine server-side error must still be logged.
        let logic = anyhow::anyhow!("live session registry lock poisoned");
        assert!(!is_client_disconnect(&logic));

        // A non-disconnect io error (e.g. timeout while the peer is alive) is not
        // classified as a benign hang-up — keep those visible.
        let timed_out = anyhow::Error::new(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "read timed out",
        ))
        .context("handling request");
        assert!(!is_client_disconnect(&timed_out));
    }

    #[test]
    fn http_response_reader_stops_at_content_length_without_eof() -> Result<()> {
        let response = b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 11\r\nConnection: keep-alive\r\n\r\n{\"ok\":true}";
        let mut reader = std::io::Cursor::new(response);

        let (status, body) =
            read_http_response_with_deadline(&mut reader, Duration::from_millis(100), 1024)?;

        assert_eq!(status, 200);
        assert_eq!(body, br#"{"ok":true}"#);
        Ok(())
    }

    #[test]
    fn http_response_reader_times_out_when_peer_never_responds() {
        let started = Instant::now();
        let error =
            read_http_response_with_deadline(&mut NeverReady, Duration::from_millis(30), 1024)
                .unwrap_err();

        assert!(error.to_string().contains("timed out"), "got: {error:#}");
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[cfg(unix)]
    #[test]
    fn serve_lock_is_exclusive_and_reusable() -> Result<()> {
        let home = tempfile::tempdir()?;
        // First acquisition holds the exclusive serve lock.
        let first = acquire_serve_lock(home.path())?;
        // A second acquisition while the first is held must fail — this is what
        // stops a duplicate daemon from contending for the SQLite store.
        assert!(
            acquire_serve_lock(home.path()).is_err(),
            "second serve lock acquisition must fail while the first is held"
        );
        // Releasing the lock lets the next daemon take it — it never wedges shut.
        drop(first);
        let _second =
            acquire_serve_lock(home.path()).expect("lock should be reacquirable once released");
        Ok(())
    }

    #[test]
    fn seeds_trust_for_new_dir_into_missing_config() -> Result<()> {
        let home = tempfile::tempdir()?;
        let work = tempfile::tempdir()?;
        let config = home.path().join(".claude.json");
        assert!(!config.exists());

        ensure_dir_trusted_in_config(&config, work.path().to_str().unwrap());

        let key = std::fs::canonicalize(work.path())?
            .to_string_lossy()
            .into_owned();
        let root: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&config)?)?;
        assert_eq!(
            root["projects"][&key]["hasTrustDialogAccepted"],
            serde_json::json!(true)
        );
        Ok(())
    }

    #[test]
    fn seeding_trust_preserves_existing_config() -> Result<()> {
        let home = tempfile::tempdir()?;
        let work = tempfile::tempdir()?;
        let config = home.path().join(".claude.json");
        std::fs::write(
            &config,
            serde_json::to_string(&serde_json::json!({
                "numStartups": 7,
                "projects": {
                    "/some/other/repo": { "hasTrustDialogAccepted": true, "ignorePatterns": ["x"] }
                }
            }))?,
        )?;

        ensure_dir_trusted_in_config(&config, work.path().to_str().unwrap());

        let key = std::fs::canonicalize(work.path())?
            .to_string_lossy()
            .into_owned();
        let root: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&config)?)?;
        // Untouched siblings survive...
        assert_eq!(root["numStartups"], serde_json::json!(7));
        assert_eq!(
            root["projects"]["/some/other/repo"]["ignorePatterns"],
            serde_json::json!(["x"])
        );
        // ...and the new dir is now trusted.
        assert_eq!(
            root["projects"][&key]["hasTrustDialogAccepted"],
            serde_json::json!(true)
        );
        Ok(())
    }

    #[test]
    fn seeding_trust_is_noop_when_already_trusted() -> Result<()> {
        let home = tempfile::tempdir()?;
        let work = tempfile::tempdir()?;
        let config = home.path().join(".claude.json");
        let key = std::fs::canonicalize(work.path())?
            .to_string_lossy()
            .into_owned();
        // Pre-trusted, with a sibling field that must not be disturbed.
        std::fs::write(
            &config,
            serde_json::to_string(&serde_json::json!({
                "projects": { &key: { "hasTrustDialogAccepted": true, "allowedTools": ["Bash"] } }
            }))?,
        )?;
        let before = std::fs::read_to_string(&config)?;

        ensure_dir_trusted_in_config(&config, work.path().to_str().unwrap());

        // Already trusted → file left byte-for-byte unchanged (no rewrite).
        assert_eq!(std::fs::read_to_string(&config)?, before);
        Ok(())
    }

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
            familiar_id: None,
            caller_familiar_id: None,
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
        handle_http_stream(
            &mut stream,
            &mut output,
            temp.path(),
            None,
            &runtime,
            None,
            false,
        )
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
            false,
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
    fn handle_http_stream_guard_blocks_cross_origin() {
        use crate::api::NoopSessionRuntime;
        use std::io::Cursor;
        let temp = tempfile::tempdir().expect("tempdir");
        ensure_private_coven_home(temp.path()).expect("ensure home");
        let request = b"GET /api/v1/health HTTP/1.1\r\nHost: 127.0.0.1:3000\r\nOrigin: http://evil.example\r\n\r\n";
        let mut stream = Cursor::new(Vec::from(&request[..]));
        let mut output: Vec<u8> = Vec::new();
        handle_http_stream(
            &mut stream,
            &mut output,
            temp.path(),
            None,
            &NoopSessionRuntime,
            Some(MAX_TCP_BODY_BYTES),
            true,
        )
        .expect("handle ok");
        let response = String::from_utf8(output).expect("utf8");
        assert!(
            response.starts_with("HTTP/1.1 403 Forbidden"),
            "got: {response}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn handle_http_stream_guard_blocks_foreign_host() {
        use crate::api::NoopSessionRuntime;
        use std::io::Cursor;
        let temp = tempfile::tempdir().expect("tempdir");
        ensure_private_coven_home(temp.path()).expect("ensure home");
        let request = b"GET /api/v1/health HTTP/1.1\r\nHost: evil.example\r\n\r\n";
        let mut stream = Cursor::new(Vec::from(&request[..]));
        let mut output: Vec<u8> = Vec::new();
        handle_http_stream(
            &mut stream,
            &mut output,
            temp.path(),
            None,
            &NoopSessionRuntime,
            Some(MAX_TCP_BODY_BYTES),
            true,
        )
        .expect("handle ok");
        let response = String::from_utf8(output).expect("utf8");
        assert!(
            response.starts_with("HTTP/1.1 403 Forbidden"),
            "got: {response}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn is_loopback_host_accepts_only_real_loopback_addresses() {
        // Real loopback: the whole 127.0.0.0/8, ::1, and the localhost name.
        assert!(is_loopback_host("127.0.0.1"));
        assert!(is_loopback_host("127.0.0.2"));
        assert!(is_loopback_host("::1"));
        assert!(is_loopback_host("localhost"));
        // Hostnames that merely *start with* "127." must NOT pass: a DNS-rebinding
        // attacker can register 127.evil.com -> 127.0.0.1 and would otherwise slip
        // through a string-prefix check and defeat the loopback guard.
        assert!(!is_loopback_host("127.evil.com"));
        assert!(!is_loopback_host("127001.example.com"));
        assert!(!is_loopback_host("evil.example"));
        assert!(!is_loopback_host(""));
    }

    #[cfg(unix)]
    #[test]
    fn handle_http_stream_guard_allows_loopback_origin() {
        use crate::api::NoopSessionRuntime;
        use std::io::Cursor;
        let temp = tempfile::tempdir().expect("tempdir");
        ensure_private_coven_home(temp.path()).expect("ensure home");
        let request = b"GET /api/v1/health HTTP/1.1\r\nHost: localhost:3000\r\nOrigin: http://localhost:3000\r\n\r\n";
        let mut stream = Cursor::new(Vec::from(&request[..]));
        let mut output: Vec<u8> = Vec::new();
        handle_http_stream(
            &mut stream,
            &mut output,
            temp.path(),
            None,
            &NoopSessionRuntime,
            Some(MAX_TCP_BODY_BYTES),
            true,
        )
        .expect("handle ok");
        let response = String::from_utf8(output).expect("utf8");
        assert!(response.starts_with("HTTP/1.1 200 OK"), "got: {response}");
    }

    #[cfg(unix)]
    #[test]
    fn handle_http_stream_unix_path_ignores_origin() {
        use crate::api::NoopSessionRuntime;
        use std::io::Cursor;
        let temp = tempfile::tempdir().expect("tempdir");
        ensure_private_coven_home(temp.path()).expect("ensure home");
        let request = b"GET /api/v1/health HTTP/1.1\r\nHost: evil.example\r\nOrigin: http://evil.example\r\n\r\n";
        let mut stream = Cursor::new(Vec::from(&request[..]));
        let mut output: Vec<u8> = Vec::new();
        handle_http_stream(
            &mut stream,
            &mut output,
            temp.path(),
            None,
            &NoopSessionRuntime,
            None,
            false,
        )
        .expect("handle ok");
        let response = String::from_utf8(output).expect("utf8");
        assert!(response.starts_with("HTTP/1.1 200 OK"), "got: {response}");
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
            .write_all(
                b"GET /api/v1/health HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: 0\r\n\r\n",
            )
            .expect("write request");
        let mut response = String::new();
        client.read_to_string(&mut response).expect("read response");
        server.join().expect("server thread");

        assert!(response.starts_with("HTTP/1.1 200 OK"), "got: {response}");
        assert!(response.contains("\"apiVersion\""), "got: {response}");
    }

    #[cfg(unix)]
    #[test]
    fn bind_tcp_listener_rejects_non_loopback() {
        let error = bind_tcp_listener("0.0.0.0:0").expect_err("should reject wildcard bind");
        let msg = format!("{error:#}");
        assert!(
            msg.contains("non-loopback"),
            "unexpected error message: {msg}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn bind_api_socket_refuses_to_take_over_a_healthy_incumbent() {
        use crate::api::NoopSessionRuntime;
        use std::thread;
        let temp = tempfile::tempdir().expect("tempdir");
        ensure_private_coven_home(temp.path()).expect("ensure home");
        let home = temp.path().to_path_buf();

        // Stand up an incumbent daemon: a bound socket plus a thread that answers
        // a single /health probe, reporting a recognizable pid.
        let incumbent = bind_api_socket(&home).expect("bind incumbent");
        let socket = daemon_socket_path(&home).to_string_lossy().into_owned();
        let server_home = home.clone();
        let status = DaemonStatus {
            pid: 4242,
            started_at: "2026-06-18T00:00:00Z".to_string(),
            socket,
        };
        let server = thread::spawn(move || {
            serve_next_connection(&incumbent, &server_home, Some(status), &NoopSessionRuntime)
                .expect("serve health probe");
        });

        // A second daemon must NOT clobber the live socket. It should bail and
        // name the incumbent pid rather than unlink the inode out from under it.
        let error = bind_api_socket(&home).expect_err("must refuse takeover");
        server.join().expect("incumbent server thread");
        let msg = format!("{error:#}");
        assert!(
            msg.contains("refusing to take over") && msg.contains("4242"),
            "unexpected error: {msg}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn bind_api_socket_reclaims_a_dead_socket_file() {
        let temp = tempfile::tempdir().expect("tempdir");
        ensure_private_coven_home(temp.path()).expect("ensure home");
        let home = temp.path().to_path_buf();

        // A crashed daemon (SIGKILL bypasses Drop) leaves the socket file behind
        // with nothing listening — connecting refuses. Reclaiming it must still
        // succeed, otherwise the guard would wedge every restart.
        let dead = bind_api_socket(&home).expect("first bind");
        drop(dead); // closes the listener; the socket file lingers, unserved
        let reclaimed = bind_api_socket(&home);
        assert!(
            reclaimed.is_ok(),
            "should reclaim a dead socket: {reclaimed:?}"
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
    fn check_owned_by_current_user_refuses_foreign_ownership() {
        let path = std::path::Path::new("/tmp/coven-example");
        // Owned by the current effective uid: accepted.
        assert!(check_owned_by_current_user(path, 1000, 1000).is_ok());
        // Owned by another uid (e.g. a root-planted dir while we run as a normal
        // user): refused before we ever touch it.
        let err = check_owned_by_current_user(path, 0, 1000)
            .expect_err("a foreign-owned path must be refused");
        assert!(
            err.to_string().contains("owned by uid 0"),
            "error should name the foreign owner, got: {err}"
        );
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
    fn daemon_socket_path_stays_inside_coven_home() {
        // AUTH.md L134: the socket must resolve directly inside COVEN_HOME, so
        // bind_api_socket's containment guard always holds for the derived path.
        let home = std::path::Path::new("/some/coven/home");
        assert_eq!(daemon_socket_path(home).parent(), Some(home));
    }

    #[test]
    fn daemon_startup_status_socket_uses_unix_socket_for_unix_platform() {
        let home = Path::new("/tmp/coven-home");
        assert_eq!(
            daemon_startup_status_socket_for_platform(home, DaemonIpcPlatform::Unix),
            daemon_socket_path(home).to_string_lossy()
        );
    }

    #[test]
    fn daemon_startup_status_socket_uses_named_pipe_for_windows_platform() {
        let home = Path::new("C:/Users/Sonic/.coven");
        let socket = daemon_startup_status_socket_for_platform(home, DaemonIpcPlatform::Windows);

        assert!(socket.starts_with("coven-daemon-"), "socket={socket}");
        assert!(socket.ends_with(".sock"), "socket={socket}");
        assert_ne!(socket, daemon_socket_path(home).to_string_lossy());
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

    #[cfg(unix)]
    #[test]
    fn daemon_status_from_default_socket_returns_none_when_socket_is_absent() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;

        assert_eq!(daemon_status_from_default_socket(temp_dir.path())?, None);

        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn ensure_background_server_reuses_daemon_recovered_from_default_socket() -> Result<()> {
        use crate::api::NoopSessionRuntime;
        use std::io::{Read, Write};
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
        let server_status = status.clone();
        let server = thread::spawn(move || -> Result<()> {
            let runtime = NoopSessionRuntime;
            for _ in 0..2 {
                serve_next_connection(&listener, &home, Some(server_status.clone()), &runtime)?;
            }
            Ok(())
        });
        let started = std::sync::Arc::new(std::sync::Mutex::new(0));

        let ensured = ensure_background_server_with_controllers(
            temp_dir.path(),
            Path::new("/usr/bin/coven"),
            "2026-04-27T10:00:00Z".to_string(),
            &SystemDaemonStopController,
            &FakeStartController {
                started: started.clone(),
                running_after_start: true,
            },
        )?;

        if ensured != status {
            for _ in 0..2 {
                if let Ok(mut stream) = UnixStream::connect(daemon_socket_path(temp_dir.path())) {
                    let _ = stream.write_all(b"GET /health HTTP/1.1\r\nHost: coven\r\n\r\n");
                    let mut response = String::new();
                    let _ = stream.read_to_string(&mut response);
                }
            }
        }
        server.join().expect("server thread")?;

        assert_eq!(ensured, status);
        assert_eq!(*started.lock().unwrap(), 0);
        assert_eq!(read_status(temp_dir.path())?, Some(status));
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn ensure_background_server_recovers_live_daemon_when_status_pid_is_stale() -> Result<()> {
        use crate::api::NoopSessionRuntime;
        use std::io::{Read, Write};
        use std::os::unix::net::UnixStream;
        use std::thread;

        let temp_dir = tempfile::tempdir()?;
        let recovered = DaemonStatus {
            pid: 12345,
            started_at: "2026-04-27T10:00:00Z".to_string(),
            socket: daemon_socket_path(temp_dir.path())
                .to_string_lossy()
                .into_owned(),
        };
        let stale = DaemonStatus {
            // Keep this within the range Linux `kill -0` treats as a plain
            // PID; u32::MAX can be interpreted as -1 by the shell utility.
            pid: 999_999,
            started_at: "2026-04-27T09:00:00Z".to_string(),
            socket: daemon_socket_path(temp_dir.path())
                .to_string_lossy()
                .into_owned(),
        };
        write_status(temp_dir.path(), &stale)?;
        let listener = bind_api_socket(temp_dir.path())?;
        let home = temp_dir.path().to_path_buf();
        let server_status = recovered.clone();
        let server = thread::spawn(move || -> Result<()> {
            let runtime = NoopSessionRuntime;
            for _ in 0..3 {
                serve_next_connection(&listener, &home, Some(server_status.clone()), &runtime)?;
            }
            Ok(())
        });
        let started = std::sync::Arc::new(std::sync::Mutex::new(0));

        let ensured = ensure_background_server_with_controllers(
            temp_dir.path(),
            Path::new("/usr/bin/coven"),
            "2026-04-27T10:00:00Z".to_string(),
            &SystemDaemonStopController,
            &FakeStartController {
                started: started.clone(),
                running_after_start: true,
            },
        )?;

        if ensured != recovered {
            for _ in 0..3 {
                if let Ok(mut stream) = UnixStream::connect(daemon_socket_path(temp_dir.path())) {
                    let _ = stream.write_all(b"GET /health HTTP/1.1\r\nHost: coven\r\n\r\n");
                    let mut response = String::new();
                    let _ = stream.read_to_string(&mut response);
                }
            }
        }
        server.join().expect("server thread")?;

        assert_eq!(ensured, recovered);
        assert_eq!(*started.lock().unwrap(), 0);
        assert_eq!(read_status(temp_dir.path())?, Some(recovered));
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

    #[cfg(windows)]
    #[test]
    fn windows_background_daemon_spawn_is_detached() {
        use windows_sys::Win32::System::Threading::{CREATE_NEW_PROCESS_GROUP, DETACHED_PROCESS};

        let flags = windows_daemon_creation_flags();

        assert_ne!(flags & DETACHED_PROCESS, 0);
        assert_ne!(flags & CREATE_NEW_PROCESS_GROUP, 0);
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
                familiar_id: None,
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
            familiar_id: None,
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

    #[cfg(windows)]
    #[test]
    fn serves_health_over_windows_named_pipe() -> Result<()> {
        use interprocess::local_socket::{prelude::*, GenericNamespaced, ListenerOptions, Stream};
        use std::io::Write;
        use std::thread;

        let temp_dir = tempfile::tempdir()?;
        let pipe_name = windows_pipe_name(temp_dir.path());

        let name = pipe_name
            .clone()
            .to_ns_name::<GenericNamespaced>()
            .expect("pipe name");
        let listener = ListenerOptions::new()
            .name(name)
            .create_sync()
            .expect("bind pipe");

        let status = DaemonStatus {
            pid: 12345,
            started_at: "2026-04-27T10:00:00Z".to_string(),
            socket: pipe_name.clone(),
        };
        let home = temp_dir.path().to_path_buf();
        let runtime = LiveSessionRuntime::default();
        let server = thread::spawn(move || {
            for _ in 0..2 {
                let conn = listener.incoming().next().expect("accept").expect("stream");
                handle_http_stream(
                    &conn,
                    &conn,
                    &home,
                    Some(status.clone()),
                    &runtime,
                    None,
                    false,
                )?;
            }
            Ok::<_, anyhow::Error>(())
        });

        for _ in 0..2 {
            let client_name = pipe_name
                .clone()
                .to_ns_name::<GenericNamespaced>()
                .expect("client pipe name");
            let mut client = Stream::connect(client_name).expect("connect");
            client
                .write_all(b"GET /api/v1/health HTTP/1.1\r\nHost: coven\r\n\r\n")
                .expect("write request");
            client.flush().expect("flush");
            let (status_code, response) = read_windows_pipe_http_response(
                client,
                Duration::from_secs(1),
                MAX_SOCKET_BODY_BYTES,
            )
            .expect("read response");
            assert_eq!(status_code, 200);
            let response = String::from_utf8(response)?;
            assert!(response.contains("\"apiVersion\""), "got: {response}");
        }
        server.join().expect("server thread")?;
        Ok(())
    }

    #[cfg(windows)]
    #[test]
    fn owner_only_pipe_security_descriptor_parses_sddl() -> Result<()> {
        owner_only_pipe_security_descriptor()?;
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn shutdown_guard_removes_socket_and_status_on_drop() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let socket_path = daemon_socket_path(temp_dir.path());
        let status_path = daemon_status_path(temp_dir.path());
        std::fs::write(&socket_path, b"")?;
        std::fs::write(
            &status_path,
            serde_json::to_string(&DaemonStatus {
                pid: 42,
                started_at: "2026-06-14T00:00:00Z".to_string(),
                socket: socket_path.to_string_lossy().into_owned(),
            })?,
        )?;
        assert!(socket_path.exists());
        assert!(status_path.exists());

        {
            let _guard = ShutdownGuard {
                socket_path: socket_path.clone(),
                status_path: status_path.clone(),
                pid: 42,
            };
            // Files are still present while the guard is alive.
            assert!(socket_path.exists());
            assert!(status_path.exists());
        }

        // Drop fires when the guard scope ends → both paths must be gone, even
        // if the daemon process is exiting via a propagated error or a panic.
        assert!(!socket_path.exists(), "socket file must be removed on Drop");
        assert!(!status_path.exists(), "status file must be removed on Drop");
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn shutdown_guard_drop_is_idempotent_when_files_already_missing() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let socket_path = daemon_socket_path(temp_dir.path());
        let status_path = daemon_status_path(temp_dir.path());
        // Files do not exist yet. Dropping the guard must not panic — the
        // daemon may have failed before bind_api_socket succeeded.
        let _guard = ShutdownGuard {
            socket_path,
            status_path,
            pid: 42,
        };
    }

    #[cfg(unix)]
    #[test]
    fn shutdown_guard_preserves_socket_and_status_for_newer_daemon() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let socket_path = daemon_socket_path(temp_dir.path());
        let status_path = daemon_status_path(temp_dir.path());
        std::fs::write(&socket_path, b"newer daemon socket")?;
        std::fs::write(
            &status_path,
            serde_json::to_string(&DaemonStatus {
                pid: 100,
                started_at: "2026-06-14T00:01:00Z".to_string(),
                socket: socket_path.to_string_lossy().into_owned(),
            })?,
        )?;

        {
            let _guard = ShutdownGuard {
                socket_path: socket_path.clone(),
                status_path: status_path.clone(),
                pid: 42,
            };
        }

        assert!(
            socket_path.exists(),
            "an older daemon must not remove a newer daemon socket on shutdown"
        );
        assert!(
            status_path.exists(),
            "an older daemon must not remove newer daemon status on shutdown"
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn append_daemon_recovery_log_creates_and_appends() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        append_daemon_recovery_log(temp_dir.path(), "first event");
        append_daemon_recovery_log(temp_dir.path(), "second event");
        let log = std::fs::read_to_string(daemon_recovery_log_path(temp_dir.path()))?;
        assert!(
            log.contains("first event"),
            "log should record the first event, got: {log}"
        );
        assert!(
            log.contains("second event"),
            "second append should not overwrite the first, got: {log}"
        );
        Ok(())
    }

    /// Regression test for OpenCoven/coven#197: a single malformed local
    /// request used to bring down the daemon because `serve_forever` used `?`
    /// on `serve_next_connection`, propagating per-connection errors all the
    /// way out and leaving the socket file orphaned. The fix turns the loop
    /// into log-and-continue. This test pins that contract by feeding the
    /// loop body a deliberately invalid request followed by a valid one and
    /// asserting both that the socket stays bound and the second request
    /// gets a real response.
    #[cfg(unix)]
    #[test]
    fn unix_serve_loop_isolates_per_connection_errors() -> Result<()> {
        use std::io::{Read, Write};
        use std::net::Shutdown;
        use std::os::unix::net::UnixStream;
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;
        use std::thread;

        let temp_dir = tempfile::tempdir()?;
        let listener = bind_api_socket(temp_dir.path())?;
        // Use a short accept timeout so the loop can poll the stop flag — we
        // don't want this test to hang the suite if the loop never exits.
        listener.set_nonblocking(false)?;
        let home = temp_dir.path().to_path_buf();
        let status = DaemonStatus {
            pid: std::process::id(),
            started_at: "2026-06-08T00:00:00Z".to_string(),
            socket: daemon_socket_path(temp_dir.path())
                .to_string_lossy()
                .into_owned(),
        };
        let stop = Arc::new(AtomicBool::new(false));
        let stop_thread = Arc::clone(&stop);

        let server = thread::spawn(move || {
            let runtime = LiveSessionRuntime::default();
            // Mirror the post-fix serve_forever loop body exactly: per-
            // connection errors must NOT exit the loop. A wakeup connection
            // from the test harness at the end unblocks the final accept().
            while !stop_thread.load(Ordering::SeqCst) {
                match serve_next_connection(&listener, &home, Some(status.clone()), &runtime) {
                    Ok(()) => {}
                    Err(error) => {
                        // This is the post-fix behavior. Pre-fix code would
                        // `?` here and exit the thread.
                        let _ = error;
                    }
                }
            }
        });

        // First, send a deliberately malformed request. The handler bails on
        // "empty API request" / parse errors; pre-fix this killed the daemon.
        let mut bad = UnixStream::connect(daemon_socket_path(temp_dir.path()))?;
        bad.write_all(b"not http\r\n\r\n")?;
        bad.shutdown(Shutdown::Write)?;
        let mut bad_response = String::new();
        let _ = bad.read_to_string(&mut bad_response);

        // Now send a well-formed health probe. If the loop swallowed the
        // earlier error correctly, this must succeed and the socket file must
        // still exist on disk.
        let mut good = UnixStream::connect(daemon_socket_path(temp_dir.path()))?;
        good.write_all(b"GET /health HTTP/1.1\r\nHost: coven\r\n\r\n")?;
        good.shutdown(Shutdown::Write)?;
        let mut good_response = String::new();
        good.read_to_string(&mut good_response)?;

        stop.store(true, Ordering::SeqCst);
        // Trigger one more accept so the loop wakes and observes the stop
        // flag, then joins cleanly. The unsolicited probe response is
        // ignored.
        let _ = UnixStream::connect(daemon_socket_path(temp_dir.path()));
        server.join().expect("server thread should not panic");

        assert!(
            good_response.starts_with("HTTP/1.1 200 OK"),
            "daemon must still respond to a valid request after a malformed one; got: {good_response}"
        );
        assert!(
            daemon_socket_path(temp_dir.path()).exists(),
            "socket file should still exist while the loop is running"
        );
        Ok(())
    }
}
