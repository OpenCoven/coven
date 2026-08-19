use std::collections::HashMap;
use std::io::Write;
#[cfg(unix)]
use std::net::{SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Condvar, Mutex, Weak,
};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use fs2::FileExt;
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
};

use crate::{
    api::{SessionEventBoundaryError, SessionEventBoundaryResult, SessionLaunch, SessionRuntime},
    pty_runner,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DaemonStatus {
    pub pid: u32,
    pub started_at: String,
    pub socket: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_optional_windows_process_creation_time"
    )]
    pub(crate) process_creation_time: Option<WindowsProcessCreationTime>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct WindowsProcessCreationTime(u64);

impl WindowsProcessCreationTime {
    #[cfg(any(windows, test))]
    fn new(value: u64) -> Result<Self> {
        anyhow::ensure!(value != 0, "invalid Windows process creation time: zero");
        Ok(Self(value))
    }

    #[cfg(any(windows, test))]
    fn get(self) -> u64 {
        self.0
    }
}

impl Serialize for WindowsProcessCreationTime {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.0.to_string())
    }
}

impl<'de> Deserialize<'de> for WindowsProcessCreationTime {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;
        let serialized = value.as_str().ok_or_else(|| {
            serde::de::Error::custom(
                "invalid Windows process creation time: expected a decimal string",
            )
        })?;
        let value = serialized.parse::<u64>().map_err(|_| {
            serde::de::Error::custom(
                "invalid Windows process creation time: expected an unsigned 64-bit decimal string",
            )
        })?;
        if value == 0 {
            return Err(serde::de::Error::custom(
                "invalid Windows process creation time: zero",
            ));
        }
        Ok(Self(value))
    }
}

fn deserialize_optional_windows_process_creation_time<'de, D>(
    deserializer: D,
) -> std::result::Result<Option<WindowsProcessCreationTime>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    if value.is_null() {
        return Err(serde::de::Error::custom(
            "invalid Windows process creation time: null",
        ));
    }
    serde_json::from_value(value)
        .map(Some)
        .map_err(serde::de::Error::custom)
}

#[derive(Debug)]
struct DaemonStatusParseError {
    source: serde_json::Error,
    process_creation_time_present: bool,
}

impl std::fmt::Display for DaemonStatusParseError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "failed to parse daemon status: {}", self.source)
    }
}

impl std::error::Error for DaemonStatusParseError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DaemonStatusState {
    Running(DaemonStatus),
    Stale(DaemonStatus),
}

#[cfg(any(windows, test))]
#[derive(Debug, Deserialize)]
struct DaemonHealthStatus {
    ok: bool,
    daemon: Option<DaemonStatus>,
}

#[cfg(not(windows))]
const MAX_DAEMON_STATUS_BYTES: usize = coven_client::MAX_DAEMON_STATUS_BYTES;
const DAEMON_LIFECYCLE_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DaemonSpawnSpec {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub coven_home: PathBuf,
}

pub trait RuntimeKiller: Send {
    fn kill(&mut self) -> Result<()>;

    /// Begin daemon-shutdown cancellation without waiting for this one child.
    /// The default preserves existing PTY behavior; strict piped trees split
    /// signaling from their quiescence proof so N live sessions do not each
    /// consume the daemon's bounded stop budget serially.
    fn signal_shutdown(&mut self) -> Result<()> {
        self.kill()
    }

    fn wait_for_shutdown_quiescence(&mut self, _timeout: Duration) -> Result<()> {
        Ok(())
    }
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

/// Privacy-safe launch disposition: runtime ownership may still be registered
/// after cleanup failed, so callers must preserve an active, killable state.
#[derive(Debug)]
pub(crate) struct RuntimeOwnershipRetainedError;

impl std::fmt::Display for RuntimeOwnershipRetainedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("runtime ownership may remain after launch cleanup")
    }
}

impl std::error::Error for RuntimeOwnershipRetainedError {}

#[derive(Default)]
pub struct LiveSessionRuntime {
    coven_home: Option<PathBuf>,
    event_writer: Option<crate::event_writer::EventWriter>,
    sessions: Arc<Mutex<HashMap<String, LiveSessionHandle>>>,
    shutting_down: AtomicBool,
    launch_gate: Arc<LiveLaunchGate>,
}

#[derive(Default)]
struct LiveLaunchGate {
    state: Mutex<LiveLaunchGateState>,
    drained: Condvar,
}

#[derive(Default)]
struct LiveLaunchGateState {
    closed: bool,
    next_id: u64,
    in_flight: HashMap<u64, Option<SharedLaunchKiller>>,
}

struct LiveLaunchAdmission {
    gate: Arc<LiveLaunchGate>,
    id: u64,
    active: bool,
}

#[derive(Clone)]
struct SharedLaunchKiller {
    killer: Arc<Mutex<Box<dyn RuntimeKiller>>>,
}

impl RuntimeKiller for SharedLaunchKiller {
    fn kill(&mut self) -> Result<()> {
        match self.killer.lock() {
            Ok(mut killer) => killer.kill(),
            Err(poisoned) => poisoned.into_inner().kill(),
        }
    }

    fn signal_shutdown(&mut self) -> Result<()> {
        match self.killer.lock() {
            Ok(mut killer) => killer.signal_shutdown(),
            Err(poisoned) => poisoned.into_inner().signal_shutdown(),
        }
    }

    fn wait_for_shutdown_quiescence(&mut self, timeout: Duration) -> Result<()> {
        match self.killer.lock() {
            Ok(mut killer) => killer.wait_for_shutdown_quiescence(timeout),
            Err(poisoned) => poisoned.into_inner().wait_for_shutdown_quiescence(timeout),
        }
    }
}

impl LiveLaunchGate {
    fn begin(self: &Arc<Self>) -> Result<LiveLaunchAdmission> {
        let mut state = match self.state.lock() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        };
        anyhow::ensure!(
            !state.closed,
            "daemon is shutting down; refusing to launch a new live session"
        );
        let id = state.next_id;
        state.next_id = state.next_id.wrapping_add(1);
        state.in_flight.insert(id, None);
        Ok(LiveLaunchAdmission {
            gate: Arc::clone(self),
            id,
            active: true,
        })
    }

    fn close_and_wait(&self) -> Result<()> {
        const ADMISSION_DRAIN_BUDGET: Duration = Duration::from_millis(500);
        let mut state = match self.state.lock() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        };
        state.closed = true;
        // A published entry owns an exact tree and is signaled immediately.
        // An unpublished entry never holds this mutex across OS spawn: if its
        // closure resumes, `publish` observes the closed gate and terminates
        // that exact tree before returning it. Piped Unix launches also carry
        // an out-of-process owner-pipe guardian across this pre-publication
        // window; Windows strict launches carry a KILL_ON_JOB_CLOSE handle as
        // soon as their suspended CreateProcess call returns.
        let mut provisional = state
            .in_flight
            .values()
            .filter_map(Clone::clone)
            .collect::<Vec<_>>();
        drop(state);
        let mut failures = Vec::new();
        for killer in &mut provisional {
            if let Err(error) = killer.signal_shutdown() {
                failures.push(format!("{error:#}"));
            }
        }

        let deadline = Instant::now() + ADMISSION_DRAIN_BUDGET;
        let mut state = match self.state.lock() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        };
        while !state.in_flight.is_empty() {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            let waited = self.drained.wait_timeout(state, remaining);
            state = match waited {
                Ok((state, _)) => state,
                Err(poisoned) => poisoned.into_inner().0,
            };
        }
        anyhow::ensure!(
            failures.is_empty(),
            "failed to terminate {} provisional launch process tree(s): {}",
            failures.len(),
            failures.join("; ")
        );
        Ok(())
    }
}

impl LiveLaunchAdmission {
    fn spawn_owned<T>(
        &self,
        spawn: impl FnOnce(&mut dyn FnMut(Box<dyn RuntimeKiller>) -> Result<()>) -> Result<T>,
    ) -> Result<(T, SharedLaunchKiller)> {
        let mut published = None;
        let mut publish = |killer: Box<dyn RuntimeKiller>| -> Result<()> {
            anyhow::ensure!(
                published.is_none(),
                "live launch killer was published twice"
            );
            let mut shared = SharedLaunchKiller {
                killer: Arc::new(Mutex::new(killer)),
            };
            let mut state = match self.gate.state.lock() {
                Ok(state) => state,
                Err(poisoned) => poisoned.into_inner(),
            };
            let accepted = self.active && !state.closed;
            if accepted {
                let slot = state.in_flight.get_mut(&self.id).ok_or_else(|| {
                    anyhow::anyhow!("live launch admission disappeared before spawn ownership")
                })?;
                *slot = Some(shared.clone());
            }
            drop(state);
            if !accepted {
                let cleanup = shared.signal_shutdown().err();
                return match cleanup {
                    Some(error) => Err(anyhow::anyhow!(
                        "daemon is shutting down; rejected spawned session cleanup failed: {error:#}"
                    )),
                    None => Err(anyhow::anyhow!(
                        "daemon is shutting down; refusing to spawn a new live session"
                    )),
                };
            }
            published = Some(shared);
            Ok(())
        };
        let value = match spawn(&mut publish) {
            Ok(value) => value,
            Err(error) => {
                if let Some(mut killer) = published.take() {
                    let cleanup = killer.signal_shutdown().err();
                    return match cleanup {
                        Some(cleanup) => Err(anyhow::anyhow!(
                            "{error:#}; failed to terminate rejected spawned session: {cleanup:#}"
                        )),
                        None => Err(error),
                    };
                }
                return Err(error);
            }
        };
        let killer = published.context("spawn returned without publishing exact ownership")?;
        Ok((value, killer))
    }

    fn release(mut self) {
        self.finish();
    }

    fn finish(&mut self) {
        if !self.active {
            return;
        }
        let mut state = match self.gate.state.lock() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        };
        state.in_flight.remove(&self.id);
        self.active = false;
        if state.in_flight.is_empty() {
            self.gate.drained.notify_all();
        }
    }
}

impl Drop for LiveLaunchAdmission {
    fn drop(&mut self) {
        self.finish();
    }
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
    input: Arc<Mutex<Box<dyn Write + Send>>>,
    killer: Arc<Mutex<Box<dyn RuntimeKiller>>>,
    registration: Arc<LiveSessionRegistration>,
}

struct LiveSessionRegistration {
    exited: AtomicBool,
    writer: Mutex<Option<crate::maintenance_gate::WriterLease>>,
    event_order: Mutex<()>,
}

impl LiveSessionRegistration {
    fn new(writer: Option<crate::maintenance_gate::WriterLease>) -> Self {
        Self {
            exited: AtomicBool::new(false),
            writer: Mutex::new(writer),
            event_order: Mutex::new(()),
        }
    }

    fn release_writer(&self) {
        if let Ok(mut writer) = self.writer.lock() {
            drop(writer.take());
        }
    }
}

struct LiveSessionExitCleanup {
    session_id: String,
    sessions: Weak<Mutex<HashMap<String, LiveSessionHandle>>>,
    registration: Arc<LiveSessionRegistration>,
}

impl LiveSessionExitCleanup {
    fn mark_exited(&self) {
        self.mark_exited_with_poison_reporter(|| {
            eprintln!(
                "coven daemon: live session registry lock poisoned while reaping `{}`; recovering cleanup",
                self.session_id
            );
        });
    }

    fn mark_exited_with_poison_reporter(&self, report_poisoned: impl FnOnce()) {
        // Publish the exit before touching the registry. If the child wins the
        // race with `register_kind_with_registration`, the registration path
        // sees this flag and drops the newly inserted handle itself.
        self.registration.exited.store(true, Ordering::Release);

        let Some(sessions) = self.sessions.upgrade() else {
            return;
        };
        let (mut sessions, was_poisoned) = match sessions.lock() {
            Ok(sessions) => (sessions, false),
            Err(poisoned) => (poisoned.into_inner(), true),
        };
        let is_current_registration = sessions
            .get(&self.session_id)
            .is_some_and(|handle| Arc::ptr_eq(&handle.registration, &self.registration));
        let removed = if is_current_registration {
            sessions.remove(&self.session_id)
        } else {
            None
        };
        drop(sessions);
        drop(removed);
        self.registration.release_writer();
        if was_poisoned {
            report_poisoned();
        }
    }
}

impl LiveSessionRuntime {
    #[cfg(test)]
    pub fn with_coven_home(coven_home: PathBuf) -> Self {
        Self::try_with_coven_home(coven_home)
            .expect("daemon event writer must start for a live session runtime")
    }

    pub fn try_with_coven_home(coven_home: PathBuf) -> Result<Self> {
        let event_writer = crate::event_writer::EventWriter::start(coven_home.clone())?;
        Ok(Self {
            coven_home: Some(coven_home),
            event_writer: Some(event_writer),
            sessions: Arc::default(),
            shutting_down: AtomicBool::new(false),
            launch_gate: Arc::default(),
        })
    }

    fn begin_launch(&self) -> Result<LiveLaunchAdmission> {
        self.launch_gate.begin()
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
        self.register_kind_with_registration(
            session_id,
            kind,
            input,
            killer,
            Arc::new(LiveSessionRegistration::new(None)),
        )
    }

    fn register_kind_with_registration(
        &self,
        session_id: String,
        kind: LiveSessionKind,
        input: Box<dyn Write + Send>,
        mut killer: Box<dyn RuntimeKiller>,
        registration: Arc<LiveSessionRegistration>,
    ) -> Result<()> {
        if self.shutting_down.load(Ordering::Acquire) {
            return reject_registration_during_shutdown(killer.as_mut());
        }
        self.register_kind_after_initial_shutdown_check(
            session_id,
            kind,
            input,
            killer,
            registration,
        )
    }

    fn register_kind_after_initial_shutdown_check(
        &self,
        session_id: String,
        kind: LiveSessionKind,
        input: Box<dyn Write + Send>,
        mut killer: Box<dyn RuntimeKiller>,
        registration: Arc<LiveSessionRegistration>,
    ) -> Result<()> {
        let mut sessions = self
            .sessions
            .lock()
            .map_err(|_| anyhow::anyhow!("live session registry lock poisoned"))?;
        if self.shutting_down.load(Ordering::Acquire) {
            // The launch may have passed the first admission check and spawned
            // while shutdown was waiting for this registry lock. Never run a
            // potentially blocking process-tree kill while holding that lock.
            drop(sessions);
            return reject_registration_during_shutdown(killer.as_mut());
        }
        let replaced = sessions.insert(
            session_id.clone(),
            LiveSessionHandle {
                kind,
                input: Arc::new(Mutex::new(input)),
                killer: Arc::new(Mutex::new(killer)),
                registration: Arc::clone(&registration),
            },
        );
        let removed = if registration.exited.load(Ordering::Acquire)
            && sessions
                .get(&session_id)
                .is_some_and(|handle| Arc::ptr_eq(&handle.registration, &registration))
        {
            sessions.remove(&session_id)
        } else {
            None
        };
        drop(sessions);
        drop(replaced);
        drop(removed);
        Ok(())
    }

    /// Stop admitting sessions, remove every owned handle, and explicitly
    /// terminate each process tree. Dropping the handles remains a containment
    /// backstop (notably KILL_ON_JOB_CLOSE on Windows), while this method gives
    /// graceful daemon shutdown a checked, observable cancellation path.
    fn shutdown_all(&self) -> Result<()> {
        self.shutting_down.store(true, Ordering::Release);
        // No prompt bytes are delivered while an admission remains in this
        // gate. Closing it makes every pre-registration launcher either fail
        // registration and kill its exact tree or finish registering that tree
        // before shutdown drains the live map. This is also the barrier that
        // keeps a detached Unix request thread from losing a locally owned
        // setsid child when the daemon process exits.
        let admission_failure = self.launch_gate.close_and_wait().err();
        let handles = {
            let mut sessions = match self.sessions.lock() {
                Ok(sessions) => sessions,
                Err(poisoned) => poisoned.into_inner(),
            };
            sessions
                .drain()
                .map(|(_, handle)| handle)
                .collect::<Vec<_>>()
        };
        let mut failures = Vec::new();
        if let Some(error) = admission_failure {
            failures.push(format!("{error:#}"));
        }
        for handle in &handles {
            let result = match handle.killer.lock() {
                Ok(mut killer) => killer.signal_shutdown(),
                Err(poisoned) => poisoned.into_inner().signal_shutdown(),
            };
            if let Err(error) = result {
                failures.push(format!("{error:#}"));
            }
        }
        // All trees have been signaled before any wait begins. Share one
        // deadline across the set so shutdown remains bounded instead of
        // multiplying the wait budget by the number of live sessions.
        let quiescence_deadline = Instant::now() + Duration::from_secs(1);
        for handle in handles {
            let remaining = quiescence_deadline.saturating_duration_since(Instant::now());
            let result = match handle.killer.lock() {
                Ok(mut killer) => killer.wait_for_shutdown_quiescence(remaining),
                Err(poisoned) => poisoned
                    .into_inner()
                    .wait_for_shutdown_quiescence(remaining),
            };
            if let Err(error) = result {
                failures.push(format!("{error:#}"));
            }
        }
        anyhow::ensure!(
            failures.is_empty(),
            "failed to terminate {} live session process tree(s): {}",
            failures.len(),
            failures.join("; ")
        );
        Ok(())
    }

    #[cfg(test)]
    fn observer_for_session(
        &self,
        session_id: String,
    ) -> (
        pty_runner::DetachedPtyObserver,
        Arc<LiveSessionRegistration>,
    ) {
        self.observer_for_session_with_writer(session_id, None)
    }

    fn observer_for_session_with_writer(
        &self,
        session_id: String,
        writer: Option<crate::maintenance_gate::WriterLease>,
    ) -> (
        pty_runner::DetachedPtyObserver,
        Arc<LiveSessionRegistration>,
    ) {
        let registration = Arc::new(LiveSessionRegistration::new(writer));
        let cleanup = LiveSessionExitCleanup {
            session_id: session_id.clone(),
            sessions: Arc::downgrade(&self.sessions),
            registration: Arc::clone(&registration),
        };
        (
            output_observer_with_cleanup(self.event_writer.clone(), session_id, Some(cleanup)),
            registration,
        )
    }
}

fn reject_registration_during_shutdown(killer: &mut dyn RuntimeKiller) -> Result<()> {
    const REJECTION: &str = "daemon is shutting down; refusing to register a new live session";
    match killer.kill() {
        Ok(()) => anyhow::bail!(REJECTION),
        Err(error) => anyhow::bail!(
            "{REJECTION}; failed to terminate the rejected session process tree: {error:#}"
        ),
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
        self.launch_session_inner(launch, None, None)
    }

    fn launch_session_with_writer(
        &self,
        launch: &SessionLaunch,
        writer: crate::maintenance_gate::WriterLease,
    ) -> Result<()> {
        self.launch_session_inner(launch, Some(writer), None)
    }

    fn launch_adopted_session(
        &self,
        launch: &SessionLaunch,
        writer: Option<crate::maintenance_gate::WriterLease>,
        ownership_established: &mut dyn FnMut() -> Result<()>,
    ) -> Result<()> {
        self.launch_session_inner(launch, writer, Some(ownership_established))
    }

    fn send_input(&self, session_id: &str, payload: &Value) -> Result<()> {
        LiveSessionRuntime::send_input(self, session_id, payload)
    }

    fn kill_session(&self, session_id: &str) -> Result<()> {
        LiveSessionRuntime::kill_session(self, session_id)
    }

    fn with_session_event_boundary(
        &self,
        session_id: &str,
        kind: &str,
        payload: &Value,
        request_adoption_id: Option<&str>,
        action: &mut dyn FnMut() -> SessionEventBoundaryResult,
    ) -> Option<SessionEventBoundaryResult> {
        let writer = self.event_writer.as_ref()?;
        Some((|| -> SessionEventBoundaryResult {
            match kind {
                "input" => {
                    let reservation = writer
                        .reserve_record(session_id, kind, payload.clone(), request_adoption_id)
                        .map_err(SessionEventBoundaryError::Persistence)?;
                    match action() {
                        Ok(()) => reservation
                            .commit()
                            .map_err(SessionEventBoundaryError::Persistence),
                        Err(error) => {
                            reservation.cancel();
                            Err(error)
                        }
                    }
                }
                "kill" => {
                    let registration = {
                        let sessions = self.sessions.lock().map_err(|_| {
                            SessionEventBoundaryError::Coordination(anyhow::anyhow!(
                                "live session registry lock poisoned"
                            ))
                        })?;
                        sessions
                            .get(session_id)
                            .map(|handle| Arc::clone(&handle.registration))
                            .ok_or_else(|| {
                                SessionEventBoundaryError::Runtime(anyhow::Error::new(
                                    NotLiveError {
                                        session_id: session_id.to_string(),
                                    },
                                ))
                            })?
                    };
                    let _event_order = registration.event_order.lock().map_err(|_| {
                        SessionEventBoundaryError::Coordination(anyhow::anyhow!(
                            "live session event-order lock poisoned"
                        ))
                    })?;
                    action()?;
                    writer
                        .record(session_id, kind, payload.clone())
                        .map_err(SessionEventBoundaryError::Persistence)
                }
                _ => {
                    action()?;
                    writer
                        .record(session_id, kind, payload.clone())
                        .map_err(SessionEventBoundaryError::Persistence)
                }
            }
        })())
    }

    fn record_session_event(
        &self,
        session_id: &str,
        kind: &str,
        payload: &Value,
    ) -> Option<Result<()>> {
        self.event_writer
            .as_ref()
            .map(|writer| writer.record(session_id, kind, payload.clone()))
    }

    fn can_record_session_event(
        &self,
        _session_id: &str,
        _kind: &str,
        payload: &Value,
    ) -> Option<Result<bool>> {
        self.event_writer
            .as_ref()
            .map(|writer| writer.can_record_critical_payload(payload))
    }

    /// Must stay in this trait impl: `api::handle_request_with_runtime` reaches
    /// the runtime through `&dyn SessionRuntime`, so an inherent method of the
    /// same name is unreachable and `GET /health` silently falls back to the
    /// trait's `None` default.
    fn event_writer_health(&self) -> Option<crate::event_writer::EventWriterHealth> {
        self.event_writer
            .as_ref()
            .map(crate::event_writer::EventWriter::health)
    }
}

impl LiveSessionRuntime {
    fn launch_session_inner(
        &self,
        launch: &SessionLaunch,
        writer: Option<crate::maintenance_gate::WriterLease>,
        ownership_established: Option<&mut dyn FnMut() -> Result<()>>,
    ) -> Result<()> {
        anyhow::ensure!(
            !self.shutting_down.load(Ordering::Acquire),
            "daemon is shutting down; refusing to launch a new session"
        );
        let familiar_ctx = match (&self.coven_home, launch.familiar_id.as_deref()) {
            (Some(home), familiar_id) => {
                crate::familiar_identity::resolve_optional(home, familiar_id)?
            }
            (None, Some(familiar_id)) => {
                anyhow::bail!("cannot resolve familiar `{familiar_id}` without COVEN_HOME")
            }
            (None, None) => None,
        };
        let launch_options = crate::harness::HarnessLaunchOptions {
            model: launch.model.as_deref(),
            launch_policy: launch.launch_policy.as_ref(),
            ..Default::default()
        };
        let command = if launch.harness == "codex"
            && launch.launch_mode == crate::harness::HarnessLaunchMode::NonInteractive
        {
            pty_runner::build_piped_harness_command_with_conversation(
                &launch.harness,
                &launch.prompt,
                Path::new(&launch.cwd),
                launch.launch_mode,
                launch.conversation.as_ref(),
                familiar_ctx.as_ref(),
                launch_options,
            )?
        } else {
            pty_runner::build_harness_command_with_conversation(
                &launch.harness,
                &launch.prompt,
                Path::new(&launch.cwd),
                launch.launch_mode,
                launch.conversation.as_ref(),
                familiar_ctx.as_ref(),
                launch_options,
            )?
        };
        self.launch_prepared_session(launch, writer, command, ownership_established)
    }

    fn launch_prepared_session(
        &self,
        launch: &SessionLaunch,
        writer: Option<crate::maintenance_gate::WriterLease>,
        command: pty_runner::HarnessCommand,
        mut ownership_established: Option<&mut dyn FnMut() -> Result<()>>,
    ) -> Result<()> {
        let (observer, registration) =
            self.observer_for_session_with_writer(launch.id.clone(), writer);
        let observer = Some(observer);
        // Hold admission from before the OS spawn until the exact process-tree
        // handle is in the live registry. Shutdown closes and drains this gate,
        // so a detached request handler cannot lose a pre-registration Unix
        // process group when the daemon exits.
        let launch_admission = self.begin_launch()?;

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
            let (piped, _provisional_killer) = launch_admission.spawn_owned(|publish| {
                let piped = pty_runner::spawn_piped_with_observer(&command, observer, true)?;
                let killer: Box<dyn RuntimeKiller> = Box::new(piped.cancellation_handle());
                publish(killer)?;
                Ok(piped)
            })?;
            let activation = piped.activate(|input, process_tree| {
                self.register_kind_with_registration(
                    launch.id.clone(),
                    LiveSessionKind::Stream,
                    input,
                    Box::new(process_tree),
                    registration,
                )?;
                launch_admission.release();
                publish_established_runtime_ownership(&mut ownership_established)
            });
            self.classify_piped_activation_result(activation)?;
            // Cancellation registration and adopted `running` publication
            // both precede the first stream-json user message. A child that
            // stops reading can still block this per-session input lock, but
            // daemon shutdown or /kill owns an independent strict process-tree
            // handle and can interrupt the write without waiting for that lock.
            self.deliver_initial_stream_prompt(launch)?;
            return Ok(());
        }

        // Noninteractive Codex is a machine-owned one-shot on every platform:
        // ordinary pipes carry its complete prompt on stdin and keep argv
        // bounded. Windows additionally routes every noninteractive harness
        // here because ConPTY can terminate those children immediately.
        if launch.launch_mode == crate::harness::HarnessLaunchMode::NonInteractive
            && (cfg!(windows) || launch.harness == "codex")
        {
            let (piped, _provisional_killer) = launch_admission.spawn_owned(|publish| {
                let piped = pty_runner::spawn_piped_with_observer(&command, observer, false)?;
                let killer: Box<dyn RuntimeKiller> = Box::new(piped.cancellation_handle());
                publish(killer)?;
                Ok(piped)
            })?;
            let activation = piped.activate(|input, process_tree| {
                self.register_kind_with_registration(
                    launch.id.clone(),
                    LiveSessionKind::Pty,
                    input,
                    Box::new(process_tree),
                    registration,
                )?;
                launch_admission.release();
                publish_established_runtime_ownership(&mut ownership_established)
            });
            return self.classify_piped_activation_result(activation);
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

        let (input, provisional_killer) = launch_admission.spawn_owned(|publish| {
            let detached = pty_runner::spawn_detached_with_observer(&command, observer)?;
            let killer: Box<dyn RuntimeKiller> = Box::new(detached.killer);
            publish(killer)?;
            Ok(detached.input)
        })?;
        self.register_kind_with_registration(
            launch.id.clone(),
            LiveSessionKind::Pty,
            input,
            Box::new(provisional_killer),
            registration,
        )?;
        launch_admission.release();
        publish_established_runtime_ownership(&mut ownership_established)
    }

    fn deliver_initial_stream_prompt(&self, launch: &SessionLaunch) -> Result<()> {
        if launch.prompt.is_empty() {
            return Ok(());
        }
        if let Err(error) =
            SessionRuntime::send_input(self, &launch.id, &json!({ "data": launch.prompt.as_str() }))
        {
            let cleanup = SessionRuntime::kill_session(self, &launch.id);
            let primary = error.context(format!(
                "stream-mode launch of `{}` failed: child closed stdin before the initial message landed (auth/setup error?)",
                launch.harness
            ));
            return match cleanup {
                Err(_cleanup) if self.runtime_ownership_retained_or_ambiguous(&launch.id) => {
                    Err(anyhow::Error::new(RuntimeOwnershipRetainedError))
                }
                Err(cleanup) => Err(anyhow::anyhow!(
                    "{primary:#}; failed to terminate the rejected stream launch: {cleanup:#}"
                )),
                Ok(()) => Err(primary),
            };
        }
        Ok(())
    }

    fn runtime_ownership_retained_or_ambiguous(&self, session_id: &str) -> bool {
        match self.sessions.lock() {
            Ok(sessions) => sessions.contains_key(session_id),
            Err(_) => true,
        }
    }

    fn classify_piped_activation_result(&self, activation: Result<()>) -> Result<()> {
        match activation {
            Err(error)
                if error
                    .downcast_ref::<pty_runner::PipedLaunchCleanupRetainedError>()
                    .is_some() =>
            {
                Err(anyhow::Error::new(RuntimeOwnershipRetainedError))
            }
            result => result,
        }
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
        let kill_result = {
            let mut killer = handle
                .killer
                .lock()
                .map_err(|_| anyhow::anyhow!("live session killer lock poisoned"))?;
            killer.kill()
        };
        if let Err(error) = kill_result {
            // A quiescence timeout does not prove the strict owner is safe to
            // discard. Retain the exact input/killer handle for a retry unless
            // the exit callback already proved the process gone or daemon
            // shutdown has taken over containment.
            if !handle.registration.exited.load(Ordering::Acquire)
                && !self.shutting_down.load(Ordering::Acquire)
            {
                let mut sessions = self
                    .sessions
                    .lock()
                    .map_err(|_| anyhow::anyhow!("live session registry lock poisoned"))?;
                if !handle.registration.exited.load(Ordering::Acquire)
                    && !self.shutting_down.load(Ordering::Acquire)
                    && !sessions.contains_key(session_id)
                {
                    sessions.insert(session_id.to_string(), handle);
                }
            }
            return Err(error);
        }
        Ok(())
    }
}

fn publish_established_runtime_ownership(
    ownership_established: &mut Option<&mut dyn FnMut() -> Result<()>>,
) -> Result<()> {
    match ownership_established.take() {
        Some(publish) => publish(),
        None => Ok(()),
    }
}

impl Drop for LiveSessionRuntime {
    fn drop(&mut self) {
        let _ = self.shutdown_all();
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

impl RuntimeKiller for pty_runner::StrictChildProcessTree {
    fn kill(&mut self) -> Result<()> {
        self.terminate_tree()
            .context("failed to terminate contained piped harness process tree")
    }
}

impl RuntimeKiller for pty_runner::SharedStrictChildProcessTree {
    fn kill(&mut self) -> Result<()> {
        self.terminate_and_wait(Duration::from_secs(1))
            .context("failed to terminate and quiesce contained piped harness process tree")
    }

    fn signal_shutdown(&mut self) -> Result<()> {
        self.terminate_tree()
            .context("failed to terminate contained piped harness process tree")
    }

    fn wait_for_shutdown_quiescence(&mut self, timeout: Duration) -> Result<()> {
        pty_runner::SharedStrictChildProcessTree::wait_for_shutdown_quiescence(self, timeout)
            .context("contained piped harness process tree did not finish shutdown cleanup")
    }
}

impl RuntimeKiller for Box<dyn portable_pty::ChildKiller + Send + Sync> {
    fn kill(&mut self) -> Result<()> {
        self.as_mut().kill().context("failed to kill live session")
    }
}

#[cfg(test)]
fn output_observer(coven_home: PathBuf, session_id: String) -> pty_runner::DetachedPtyObserver {
    let writer =
        crate::event_writer::EventWriter::start(coven_home).expect("test event writer must start");
    output_observer_with_cleanup(Some(writer), session_id, None)
}

fn output_observer_with_cleanup(
    writer: Option<crate::event_writer::EventWriter>,
    session_id: String,
    cleanup: Option<LiveSessionExitCleanup>,
) -> pty_runner::DetachedPtyObserver {
    let output_writer = writer.clone();
    let output_session_id = session_id.clone();
    let exit_writer = writer;
    let exit_session_id = session_id;
    // The piped runner drains stdout and stderr independently.  Serialize the
    // final output submission with exit so a late stderr callback cannot add a
    // newly accepted output event after the exit barrier has committed.
    let event_closed = Arc::new(Mutex::new(false));
    let output_closed = Arc::clone(&event_closed);
    let exit_closed = event_closed;
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
            let closed = output_closed
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if *closed {
                eprintln!(
                    "coven daemon: discarded output observed after exit barrier for session `{output_session_id}`"
                );
                return;
            }
            if let Some(writer) = output_writer.as_ref() {
                match writer.record_output(&output_session_id, text) {
                    Ok(true) => {}
                    Ok(false) => eprintln!(
                        "coven daemon: event writer is pressured; raw output for session `{output_session_id}` was rejected"
                    ),
                    Err(error) => eprintln!(
                        "coven daemon: event writer failed while recording output for session `{output_session_id}`: {error:#}"
                    ),
                }
            }
        }),
        on_exit: Box::new(move |result| {
            let mut closed = exit_closed
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            *closed = true;
            let registration = cleanup
                .as_ref()
                .map(|cleanup| Arc::clone(&cleanup.registration));
            let _event_order = registration.as_ref().map(|registration| {
                registration
                    .event_order
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
            });
            if let Some(cleanup) = cleanup {
                cleanup.mark_exited();
            }
            if let Some(writer) = exit_writer.as_ref() {
                if let Err(error) = writer.record_exit(&exit_session_id, result) {
                    eprintln!(
                        "coven daemon: failed to persist exit for session `{exit_session_id}`: {error:#}"
                    );
                }
            }
            drop(closed);
        }),
    }
}

#[cfg(test)]
fn record_session_exit(
    coven_home: &Path,
    session_id: &str,
    result: pty_runner::PtyRunResult,
) -> Result<()> {
    crate::event_writer::EventWriter::start(coven_home.to_path_buf())?
        .record_exit(session_id, result)
}

#[cfg(test)]
fn record_session_event(
    coven_home: &Path,
    session_id: &str,
    kind: &str,
    payload: Value,
) -> Result<()> {
    crate::event_writer::EventWriter::start(coven_home.to_path_buf())?
        .record(session_id, kind, payload)
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

fn daemon_windows_pipe_name(coven_home: &Path) -> Result<String> {
    #[cfg(windows)]
    {
        coven_client::owner_only_windows_pipe_name(coven_home).map_err(anyhow::Error::new)
    }
    #[cfg(not(windows))]
    {
        let _ = coven_home;
        anyhow::bail!("Windows daemon pipe identity is unavailable on this platform")
    }
}

fn daemon_startup_status_socket_for_platform(
    coven_home: &Path,
    platform: DaemonIpcPlatform,
) -> Result<String> {
    Ok(match platform {
        DaemonIpcPlatform::Unix => daemon_socket_path(coven_home)
            .to_str()
            .context(
                "canonical Coven daemon socket is not valid UTF-8; daemon status JSON requires \
                 UTF-8 paths",
            )?
            .to_owned(),
        DaemonIpcPlatform::Windows => daemon_windows_pipe_name(coven_home)?,
    })
}

fn daemon_startup_status_socket(coven_home: &Path) -> Result<String> {
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
pub(crate) fn ensure_private_coven_home(coven_home: &Path) -> Result<()> {
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
pub(crate) fn ensure_private_coven_home(coven_home: &Path) -> Result<()> {
    #[cfg(windows)]
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
    #[cfg(windows)]
    set_windows_owner_only_directory_security(coven_home)?;
    Ok(())
}

#[cfg(any(windows, test))]
const WINDOWS_OWNER_ONLY_DIRECTORY_DACL_SDDL: &str = "D:P(A;OICI;GA;;;OW)";

#[cfg(windows)]
fn set_windows_owner_only_directory_security(path: &Path) -> Result<()> {
    use std::{ffi::OsStr, os::windows::ffi::OsStrExt, ptr};
    use windows_sys::Win32::Security::{
        Authorization::{
            ConvertStringSecurityDescriptorToSecurityDescriptorW, SetNamedSecurityInfoW,
            SE_FILE_OBJECT,
        },
        GetSecurityDescriptorDacl, DACL_SECURITY_INFORMATION, OWNER_SECURITY_INFORMATION,
        PROTECTED_DACL_SECURITY_INFORMATION,
    };

    let descriptor_sddl: Vec<u16> = OsStr::new(WINDOWS_OWNER_ONLY_DIRECTORY_DACL_SDDL)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let mut descriptor = ptr::null_mut();
    if unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            descriptor_sddl.as_ptr(),
            1,
            &mut descriptor,
            ptr::null_mut(),
        ) == 0
    } {
        return Err(std::io::Error::last_os_error())
            .context("failed to build owner-only Windows security descriptor");
    }
    let _descriptor = WindowsLocalAllocation(descriptor);
    let mut dacl_present = 0;
    let mut dacl = ptr::null_mut();
    let mut dacl_defaulted = 0;
    if unsafe {
        GetSecurityDescriptorDacl(
            descriptor,
            &mut dacl_present,
            &mut dacl,
            &mut dacl_defaulted,
        ) == 0
    } || dacl_present == 0
        || dacl.is_null()
    {
        anyhow::bail!("owner-only Windows security descriptor did not contain a DACL");
    }

    let owner = current_windows_user_sid()?;
    let path: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let status = unsafe {
        SetNamedSecurityInfoW(
            path.as_ptr(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION
                | DACL_SECURITY_INFORMATION
                | PROTECTED_DACL_SECURITY_INFORMATION,
            owner.as_ptr(),
            ptr::null_mut(),
            dacl,
            ptr::null_mut(),
        )
    };
    if status != 0 {
        anyhow::bail!("failed to set owner-only security on Coven path: Windows error {status}");
    }
    Ok(())
}

#[cfg(any(windows, test))]
struct WindowsTokenBuffer {
    words: Vec<usize>,
}

#[cfg(any(windows, test))]
impl WindowsTokenBuffer {
    fn new(byte_len: usize) -> Self {
        let word_len = byte_len.max(1).div_ceil(std::mem::size_of::<usize>());
        Self {
            words: vec![0; word_len],
        }
    }

    fn as_mut_ptr(&mut self) -> *mut std::ffi::c_void {
        self.words.as_mut_ptr().cast()
    }

    #[cfg(windows)]
    fn as_ptr(&self) -> *const std::ffi::c_void {
        self.words.as_ptr().cast()
    }

    fn byte_capacity(&self) -> usize {
        self.words.len() * std::mem::size_of::<usize>()
    }
}

#[cfg(windows)]
const _: () = assert!(
    std::mem::align_of::<usize>()
        >= std::mem::align_of::<windows_sys::Win32::Security::TOKEN_USER>()
);

#[cfg(windows)]
struct WindowsSid(WindowsTokenBuffer);

#[cfg(windows)]
impl WindowsSid {
    fn as_ptr(&self) -> windows_sys::Win32::Security::PSID {
        self.0.as_ptr().cast_mut()
    }

    fn to_sddl_string(&self) -> Result<String> {
        use std::ptr;
        use widestring::U16CStr;
        use windows_sys::Win32::Security::Authorization::ConvertSidToStringSidW;

        let mut string_sid = ptr::null_mut();
        if unsafe { ConvertSidToStringSidW(self.as_ptr(), &mut string_sid) } == 0 {
            return Err(std::io::Error::last_os_error())
                .context("failed to serialize current Windows user SID");
        }
        let _string_sid = WindowsLocalAllocation(string_sid.cast());
        unsafe { U16CStr::from_ptr_str(string_sid) }
            .to_string()
            .context("current Windows user SID was not valid UTF-16")
    }
}

#[cfg(windows)]
fn current_windows_user_sid() -> Result<WindowsSid> {
    use std::{mem::size_of, ptr};
    use windows_sys::Win32::{
        Foundation::{GetLastError, ERROR_INSUFFICIENT_BUFFER},
        Security::{
            CopySid, GetLengthSid, GetTokenInformation, TokenUser, TOKEN_QUERY, TOKEN_USER,
        },
        System::Threading::{GetCurrentProcess, OpenProcessToken},
    };

    let mut process_token = ptr::null_mut();
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut process_token) == 0 } {
        return Err(std::io::Error::last_os_error())
            .context("failed to inspect current Windows user");
    }
    let _token = WindowsHandle(process_token);
    let mut bytes = 0;
    let initial =
        unsafe { GetTokenInformation(process_token, TokenUser, ptr::null_mut(), 0, &mut bytes) };
    if initial != 0 || unsafe { GetLastError() } != ERROR_INSUFFICIENT_BUFFER || bytes == 0 {
        anyhow::bail!("failed to size current Windows user token");
    }
    let mut buffer = WindowsTokenBuffer::new(bytes as usize);
    if unsafe {
        GetTokenInformation(
            process_token,
            TokenUser,
            buffer.as_mut_ptr(),
            bytes,
            &mut bytes,
        ) == 0
    } {
        return Err(std::io::Error::last_os_error())
            .context("failed to read current Windows user token");
    }
    if (bytes as usize) < size_of::<TOKEN_USER>() || bytes as usize > buffer.byte_capacity() {
        anyhow::bail!("current Windows user token returned an invalid size");
    }
    let user = unsafe { &*buffer.as_ptr().cast::<TOKEN_USER>() };
    if user.User.Sid.is_null() {
        anyhow::bail!("current Windows user token had no SID");
    }
    let length = unsafe { GetLengthSid(user.User.Sid) } as usize;
    if length == 0 {
        anyhow::bail!("current Windows user token had an invalid SID");
    }
    let mut sid = WindowsTokenBuffer::new(length);
    if unsafe { CopySid(length as u32, sid.as_mut_ptr(), user.User.Sid) } == 0 {
        return Err(std::io::Error::last_os_error())
            .context("failed to copy current Windows user SID");
    }
    Ok(WindowsSid(sid))
}

#[cfg(windows)]
struct WindowsHandle(windows_sys::Win32::Foundation::HANDLE);

#[cfg(windows)]
impl Drop for WindowsHandle {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.0);
        }
    }
}

#[cfg(windows)]
struct WindowsLocalAllocation(*mut std::ffi::c_void);

#[cfg(windows)]
impl Drop for WindowsLocalAllocation {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::Foundation::LocalFree(self.0);
        }
    }
}

pub fn background_server_spec(
    current_exe: &Path,
    coven_home: &Path,
    started_at: &str,
) -> DaemonSpawnSpec {
    DaemonSpawnSpec {
        program: current_exe.to_path_buf(),
        args: vec![
            "daemon".to_string(),
            "serve".to_string(),
            "--managed-started-at".to_string(),
            started_at.to_owned(),
        ],
        coven_home: coven_home.to_path_buf(),
    }
}

pub fn start_background_server(
    coven_home: &Path,
    current_exe: &Path,
    started_at: String,
) -> Result<DaemonStatus> {
    prevent_background_server_stdio_handle_leaks()?;
    start_background_server_with_spawn(coven_home, current_exe, started_at, |spec| {
        background_server_command(spec)
            .spawn()
            .map(|child| child.id())
            .with_context(|| format!("failed to start Coven daemon {}", spec.program.display()))
    })
}

fn start_background_server_with_spawn(
    coven_home: &Path,
    current_exe: &Path,
    started_at: String,
    spawn: impl FnOnce(&DaemonSpawnSpec) -> Result<u32>,
) -> Result<DaemonStatus> {
    let coven_home = canonical_lifecycle_home(coven_home)?;
    let spec = background_server_spec(current_exe, &coven_home, &started_at);
    let pid = spawn(&spec)?;
    Ok(DaemonStatus {
        pid,
        started_at,
        socket: daemon_startup_status_socket(&coven_home)?,
        process_creation_time: None,
    })
}

#[cfg(windows)]
fn prevent_background_server_stdio_handle_leaks() -> Result<()> {
    use windows_sys::Win32::{
        Foundation::{SetHandleInformation, HANDLE_FLAG_INHERIT, INVALID_HANDLE_VALUE},
        System::Console::{GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE},
    };

    // `CreateProcessW(..., bInheritHandles=TRUE, ...)` inherits every handle
    // still marked inheritable in the launcher, not only the null std handles
    // Rust creates for the detached `daemon serve` child. When `daemon start`
    // itself was launched with captured stdout/stderr, those inherited capture
    // handles otherwise stay open for the daemon's lifetime and the caller
    // never observes pipe EOF/Child `close` after the launcher exits.
    //
    // Clearing HANDLE_FLAG_INHERIT does not close these handles or prevent the
    // launcher from writing its normal diagnostics. Rust creates inheritable
    // duplicates for any later child whose stdio is intentionally inherited;
    // the daemon child below receives its own null std handles.
    for (label, stream) in [
        ("stdin", STD_INPUT_HANDLE),
        ("stdout", STD_OUTPUT_HANDLE),
        ("stderr", STD_ERROR_HANDLE),
    ] {
        let handle = unsafe { GetStdHandle(stream) };
        if handle.is_null() || handle == INVALID_HANDLE_VALUE {
            continue;
        }
        if unsafe { SetHandleInformation(handle, HANDLE_FLAG_INHERIT, 0) } == 0 {
            return Err(std::io::Error::last_os_error()).with_context(|| {
                format!("failed to prevent inherited {label} from leaking into Coven daemon")
            });
        }
    }
    Ok(())
}

#[cfg(not(windows))]
fn prevent_background_server_stdio_handle_leaks() -> Result<()> {
    Ok(())
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
    let deadline = LifecycleDeadline::after(DAEMON_LIFECYCLE_TIMEOUT)?;
    deadline.remaining("resolving Coven daemon profile")?;
    let coven_home = canonical_lifecycle_home(coven_home)?;
    let _lock = acquire_daemon_lifecycle_lock_until(&coven_home, deadline)?;
    let status = ensure_background_server_with_controllers_until(
        &coven_home,
        current_exe,
        started_at,
        &SystemDaemonStopController,
        &SystemDaemonStartController,
        deadline,
    )?;
    #[cfg(unix)]
    run_optional_lifecycle_diagnostic(deadline, move |deadline| {
        report_unreachable_duplicate_daemons(coven_home, status.pid, deadline);
    });
    Ok(status)
}

pub(crate) fn daemon_lifecycle_lock_path(coven_home: &Path) -> PathBuf {
    coven_home.join("daemon.lock")
}

#[cfg(unix)]
fn report_unreachable_duplicate_daemons(coven_home: PathBuf, active_pid: u32, deadline: Instant) {
    use sysinfo::System;

    let (finished_tx, finished_rx) = std::sync::mpsc::sync_channel(0);
    std::thread::spawn(move || {
        optional_diagnostic_test_delay();
        if Instant::now() >= deadline {
            let _ = finished_tx.send(());
            return;
        }
        let mut sys = System::new_all();
        sys.refresh_all();
        for (pid, process) in sys.processes() {
            if Instant::now() >= deadline {
                return;
            }
            if process_is_unreachable_duplicate_daemon_candidate(
                pid.as_u32(),
                active_pid,
                process.thread_kind(),
                process.cmd(),
                process.environ(),
                &coven_home,
            ) {
                append_daemon_recovery_log(
                    &coven_home,
                    &format!(
                        "preserved unreachable duplicate daemon pid={} active_pid={}: no authenticated lifecycle connection",
                        pid.as_u32(),
                        active_pid
                    ),
                );
            }
        }
        let _ = finished_tx.send(());
    });
    let remaining = deadline.saturating_duration_since(Instant::now());
    if !remaining.is_zero() {
        let _ = finished_rx.recv_timeout(remaining);
    }
}

#[cfg(unix)]
fn run_optional_lifecycle_diagnostic(
    deadline: LifecycleDeadline,
    diagnostic: impl FnOnce(Instant),
) {
    if deadline
        .remaining("running optional daemon diagnostics")
        .is_ok()
    {
        diagnostic(deadline.instant);
    }
}

#[cfg(all(unix, test))]
static OPTIONAL_DIAGNOSTIC_DELAY_MILLIS: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

#[cfg(unix)]
fn optional_diagnostic_test_delay() {
    #[cfg(test)]
    {
        let delay = OPTIONAL_DIAGNOSTIC_DELAY_MILLIS.load(std::sync::atomic::Ordering::SeqCst);
        if delay != 0 {
            std::thread::sleep(Duration::from_millis(delay));
        }
    }
}

#[cfg(unix)]
fn process_is_unreachable_duplicate_daemon_candidate(
    pid: u32,
    active_pid: u32,
    thread_kind: Option<sysinfo::ThreadKind>,
    cmd: &[std::ffi::OsString],
    environ: &[std::ffi::OsString],
    coven_home: &Path,
) -> bool {
    pid != active_pid
        && thread_kind.is_none()
        && process_is_coven_daemon_serve(cmd)
        && process_coven_home_matches(environ, coven_home)
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

struct DaemonLifecycleLock {
    file: std::fs::File,
}

impl Drop for DaemonLifecycleLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

fn acquire_daemon_lifecycle_lock(coven_home: &Path) -> Result<DaemonLifecycleLock> {
    ensure_private_coven_home(coven_home)?;
    let lock_path = daemon_lifecycle_lock_path(coven_home);
    let file = crate::state_lock::open_lock_file(&lock_path).with_context(|| {
        format!(
            "failed to open daemon lifecycle lock {}",
            lock_path.display()
        )
    })?;
    file.lock_exclusive()
        .with_context(|| format!("failed to lock daemon lifecycle {}", lock_path.display()))?;
    Ok(DaemonLifecycleLock { file })
}

fn acquire_daemon_lifecycle_lock_until(
    coven_home: &Path,
    deadline: LifecycleDeadline,
) -> Result<DaemonLifecycleLock> {
    ensure_private_coven_home(coven_home)?;
    let lock_path = daemon_lifecycle_lock_path(coven_home);
    let file = crate::state_lock::open_lock_file(&lock_path).with_context(|| {
        format!(
            "failed to open daemon lifecycle lock {}",
            lock_path.display()
        )
    })?;
    loop {
        match file.try_lock_exclusive() {
            Ok(()) => return Ok(DaemonLifecycleLock { file }),
            Err(error) if crate::state_lock::is_lock_contended(&error) => {
                let remaining = deadline.remaining("waiting for Coven daemon lifecycle lock")?;
                std::thread::sleep(remaining.min(Duration::from_millis(10)));
            }
            Err(error) => {
                return Err(error).with_context(|| {
                    format!("failed to lock daemon lifecycle {}", lock_path.display())
                })
            }
        }
    }
}

fn try_acquire_daemon_lifecycle_lock_in(
    coven_home: &Path,
    home_dir: &cap_std::fs::Dir,
) -> Result<Option<DaemonLifecycleLock>> {
    let lock_path = daemon_lifecycle_lock_path(coven_home);
    let file = crate::state_lock::open_lock_file_in(home_dir, "daemon.lock", &lock_path)
        .with_context(|| {
            format!(
                "failed to open daemon lifecycle lock {}",
                lock_path.display()
            )
        })?;
    match file.try_lock_exclusive() {
        Ok(()) => Ok(Some(DaemonLifecycleLock { file })),
        Err(error) if crate::state_lock::is_lock_contended(&error) => Ok(None),
        Err(error) => Err(error)
            .with_context(|| format!("failed to lock daemon lifecycle {}", lock_path.display())),
    }
}

pub fn recover_orphaned_sessions(coven_home: &Path, updated_at: &str) -> Result<usize> {
    let conn = crate::store::open_store(&coven_home.join("coven.sqlite3"))?;
    crate::store::mark_running_sessions_orphaned(&conn, updated_at)
}

/// Unmount AFS mounts whose owning daemon is gone (DESIGN.md §7).
///
/// Never fatal to startup. A mount we cannot reclaim is a stale mount point,
/// which is untidy; refusing to boot over it would take the whole daemon down
/// for a session nobody asked about. Deltas are left alone either way —
/// unreviewed work is not garbage.
fn recover_orphaned_afs_mounts(coven_home: &Path) {
    let reclaimed = crate::afs_mount::sweep_orphans(coven_home);
    if !reclaimed.is_empty() {
        append_daemon_recovery_log(
            coven_home,
            &format!("unmounted orphaned afs sessions: {}", reclaimed.join(", ")),
        );
    }
}

/// TTL before a `created` row with no live owner is declared dead (#342).
/// Generous on purpose: `coven run` writes the store directly, so a launch
/// can be legitimately mid-registration while the daemon boots or serves —
/// a blanket sweep would clobber it, an age check cannot.
pub const STALE_CREATED_TTL_SECS: i64 = 600;

/// Companion to [`recover_orphaned_sessions`] for the other unowned state:
/// rows a dead `coven run` left in `created` (registered, never launched —
/// fork exhaustion, missing adapter, crash). Nothing owns such a row, so no
/// exit path will ever fail it; only age proves it dead.
pub fn recover_stale_created_sessions(coven_home: &Path, updated_at: &str) -> Result<usize> {
    let conn = crate::store::open_store(&coven_home.join("coven.sqlite3"))?;
    let cutoff = (chrono::Utc::now() - chrono::Duration::seconds(STALE_CREATED_TTL_SECS))
        .to_rfc3339_opts(chrono::SecondsFormat::Nanos, true);
    crate::store::mark_stale_created_sessions_failed(&conn, &cutoff, updated_at)
}

pub fn write_status(coven_home: &Path, status: &DaemonStatus) -> Result<()> {
    ensure_private_coven_home(coven_home)?;
    let json = serde_json::to_string_pretty(status).context("failed to serialize daemon status")?;
    let status_path = daemon_status_path(coven_home);
    #[cfg(windows)]
    {
        return write_windows_status(&status_path, &json);
    }
    #[cfg(not(windows))]
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

#[cfg(windows)]
fn write_windows_status(status_path: &Path, json: &str) -> Result<()> {
    let coven_home = status_path
        .parent()
        .context("daemon status path has no Coven home")?;
    coven_client::write_owner_only_windows_daemon_status(coven_home, json.as_bytes())
        .map_err(anyhow::Error::new)
}

pub fn read_status(coven_home: &Path) -> Result<Option<DaemonStatus>> {
    #[cfg(windows)]
    {
        let Some(json) = coven_client::read_windows_daemon_status_for_lifecycle(coven_home)
            .map_err(windows_status_read_error)?
        else {
            return Ok(None);
        };
        let status = parse_daemon_status(&json)?;
        return Ok(Some(status));
    }

    #[cfg(not(windows))]
    {
        let path = daemon_status_path(coven_home);
        let file = match std::fs::File::open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("failed to open daemon status {}", path.display()))
            }
        };
        let json = read_bounded_daemon_status(file).map_err(|error| {
            anyhow::anyhow!("failed to read daemon status {}: {error:#}", path.display())
        })?;
        let status = parse_daemon_status(&json)?;
        Ok(Some(status))
    }
}

fn read_status_until(
    coven_home: &Path,
    deadline: LifecycleDeadline,
) -> Result<Option<DaemonStatus>> {
    deadline.remaining("reading Coven daemon lifecycle status")?;
    #[cfg(windows)]
    {
        let Some(json) = coven_client::read_windows_daemon_status_for_lifecycle_until(
            coven_home,
            deadline.instant,
        )
        .map_err(windows_status_read_error)?
        else {
            return Ok(None);
        };
        let status = parse_daemon_status(&json)?;
        deadline.remaining("parsing Coven daemon lifecycle status")?;
        Ok(Some(status))
    }
    #[cfg(not(windows))]
    {
        let status = read_status(coven_home)?;
        deadline.remaining("reading Coven daemon lifecycle status")?;
        Ok(status)
    }
}

#[cfg(windows)]
fn windows_status_read_error(error: coven_client::ClientError) -> anyhow::Error {
    match error {
        coven_client::ClientError::InvalidJson(source) => {
            anyhow::Error::new(DaemonStatusParseError {
                source,
                process_creation_time_present: false,
            })
        }
        error => anyhow::Error::new(error),
    }
}

fn parse_daemon_status(serialized: &str) -> Result<DaemonStatus> {
    let process_creation_time_present = serde_json::from_str::<serde_json::Value>(serialized)
        .ok()
        .and_then(|value| {
            value
                .as_object()
                .map(|object| object.contains_key("processCreationTime"))
        })
        .unwrap_or(false);
    serde_json::from_str(serialized).map_err(|source| {
        anyhow::Error::new(DaemonStatusParseError {
            source,
            process_creation_time_present,
        })
    })
}

pub(crate) fn read_status_synchronized(coven_home: &Path) -> Result<Option<DaemonStatus>> {
    let _lock = acquire_daemon_lifecycle_lock(coven_home)?;
    read_status(coven_home)
}

#[cfg(not(windows))]
fn read_bounded_daemon_status(file: std::fs::File) -> Result<String> {
    let size = file
        .metadata()
        .context("failed to inspect daemon status size")?
        .len();
    if size > MAX_DAEMON_STATUS_BYTES as u64 {
        anyhow::bail!("daemon status exceeded the {MAX_DAEMON_STATUS_BYTES}-byte limit");
    }
    let capacity = usize::try_from(size).context("daemon status size was not representable")?;
    let mut bytes = Vec::with_capacity(capacity);
    file.take((MAX_DAEMON_STATUS_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .context("failed to read bounded daemon status")?;
    if bytes.len() > MAX_DAEMON_STATUS_BYTES {
        anyhow::bail!("daemon status exceeded the {MAX_DAEMON_STATUS_BYTES}-byte limit");
    }
    String::from_utf8(bytes).context("daemon status was not valid UTF-8")
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
    let deadline = LifecycleDeadline::after(DAEMON_LIFECYCLE_TIMEOUT)?;
    deadline.remaining("resolving Coven daemon profile")?;
    let coven_home = canonical_lifecycle_home(coven_home)?;
    let _lock = acquire_daemon_lifecycle_lock_until(&coven_home, deadline)?;
    stop_background_server_with_controller_until(&coven_home, &SystemDaemonStopController, deadline)
}

pub fn restart_background_server(
    coven_home: &Path,
    current_exe: &Path,
    started_at: String,
) -> Result<(bool, DaemonStatus)> {
    let deadline = LifecycleDeadline::after(DAEMON_LIFECYCLE_TIMEOUT)?;
    deadline.remaining("resolving Coven daemon profile")?;
    let coven_home = canonical_lifecycle_home(coven_home)?;
    let _lock = acquire_daemon_lifecycle_lock_until(&coven_home, deadline)?;
    restart_background_server_with_controllers_until(
        &coven_home,
        current_exe,
        started_at,
        &SystemDaemonStopController,
        &SystemDaemonStartController,
        deadline,
    )
}

fn restart_background_server_with_controllers_until(
    coven_home: &Path,
    current_exe: &Path,
    started_at: String,
    stop_controller: &dyn DaemonStopController,
    start_controller: &dyn DaemonStartController,
    deadline: LifecycleDeadline,
) -> Result<(bool, DaemonStatus)> {
    let coven_home = canonical_lifecycle_home(coven_home)?;
    let was_running =
        stop_background_server_with_controller_until(&coven_home, stop_controller, deadline)?;
    deadline.remaining("starting Coven daemon")?;
    let launched =
        start_controller.start_background_server(&coven_home, current_exe, started_at)?;
    let Some(status) =
        start_controller.wait_for_running_daemon(&coven_home, &launched, deadline)?
    else {
        anyhow::bail!(
            "started Coven daemon pid {} but its socket did not become ready",
            launched.pid
        );
    };
    deadline.remaining("completing Coven daemon restart")?;
    #[cfg(unix)]
    run_optional_lifecycle_diagnostic(deadline, move |deadline| {
        report_unreachable_duplicate_daemons(coven_home, status.pid, deadline);
    });
    Ok((was_running, status))
}

pub fn background_server_status(coven_home: &Path) -> Result<Option<DaemonStatusState>> {
    background_server_status_locked_with_controller(coven_home, &SystemDaemonStopController)
}

fn background_server_status_locked_with_controller(
    coven_home: &Path,
    controller: &dyn DaemonStopController,
) -> Result<Option<DaemonStatusState>> {
    let deadline = LifecycleDeadline::after(DAEMON_LIFECYCLE_TIMEOUT)?;
    deadline.remaining("resolving Coven daemon profile")?;
    let coven_home = canonical_lifecycle_home(coven_home)?;
    let _lock = acquire_daemon_lifecycle_lock_until(&coven_home, deadline)?;
    background_server_status_with_controller_until(&coven_home, controller, deadline)
}

trait DaemonStopController {
    fn stop_verified_daemon(
        &self,
        coven_home: &Path,
        status: &DaemonStatus,
        deadline: LifecycleDeadline,
    ) -> Result<VerifiedStopOutcome>;
    fn recorded_process_state(&self, status: &DaemonStatus) -> Result<RecordedProcessState>;
    fn status_matches_running_daemon(
        &self,
        coven_home: &Path,
        status: &DaemonStatus,
        deadline: LifecycleDeadline,
    ) -> Result<bool>;
    fn authenticated_running_status(
        &self,
        coven_home: &Path,
        status: &DaemonStatus,
        deadline: LifecycleDeadline,
    ) -> Result<Option<DaemonStatus>> {
        self.status_matches_running_daemon(coven_home, status, deadline)
            .map(|matches| matches.then(|| status.clone()))
    }
    fn status_from_default_socket(
        &self,
        coven_home: &Path,
        deadline: LifecycleDeadline,
    ) -> Result<Option<DaemonStatus>> {
        let _ = coven_home;
        let _ = deadline;
        Ok(None)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum VerifiedStopOutcome {
    Unverified,
    Exited,
    TimedOut,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RecordedProcessState {
    Gone,
    Matching,
    #[cfg(any(windows, test))]
    Mismatched,
    Unverifiable,
}

#[derive(Clone, Copy, Debug)]
struct LifecycleDeadline {
    instant: Instant,
}

impl LifecycleDeadline {
    fn after(timeout: Duration) -> Result<Self> {
        let instant = Instant::now()
            .checked_add(timeout)
            .context("daemon lifecycle deadline overflowed")?;
        Ok(Self { instant })
    }

    #[cfg(test)]
    fn from_instant(instant: Instant) -> Self {
        Self { instant }
    }

    fn remaining(self, phase: &'static str) -> Result<Duration> {
        self.remaining_at(Instant::now(), phase)
    }

    fn remaining_at(self, now: Instant, phase: &'static str) -> Result<Duration> {
        self.instant
            .checked_duration_since(now)
            .filter(|remaining| !remaining.is_zero())
            .ok_or_else(|| anyhow::anyhow!("timed out {phase}"))
    }
}

#[cfg(any(windows, test))]
fn recorded_windows_process_state(
    status: &DaemonStatus,
    live_creation_time: Option<u64>,
) -> RecordedProcessState {
    let Some(live_creation_time) = live_creation_time else {
        return RecordedProcessState::Gone;
    };
    match status.process_creation_time {
        Some(recorded) if recorded.get() == live_creation_time => RecordedProcessState::Matching,
        Some(_) => RecordedProcessState::Mismatched,
        None => RecordedProcessState::Unverifiable,
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
    fn wait_for_running_daemon(
        &self,
        coven_home: &Path,
        status: &DaemonStatus,
        deadline: LifecycleDeadline,
    ) -> Result<Option<DaemonStatus>>;
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

    fn wait_for_running_daemon(
        &self,
        coven_home: &Path,
        status: &DaemonStatus,
        deadline: LifecycleDeadline,
    ) -> Result<Option<DaemonStatus>> {
        #[cfg(unix)]
        {
            if !unix_status_matches_selected_home(coven_home, status) {
                return Ok(None);
            }
            loop {
                let remaining = deadline.remaining("waiting for Coven daemon startup health")?;
                if let Some(live) = unix_authenticated_daemon_status(coven_home, status, remaining)?
                {
                    return Ok(Some(live));
                }
                let remaining = deadline.remaining("waiting for Coven daemon startup health")?;
                std::thread::sleep(remaining.min(Duration::from_millis(50)));
            }
        }
        #[cfg(windows)]
        {
            let _ = coven_home;
            wait_for_windows_running_daemon_with_identity_probe_until(
                status,
                deadline,
                |pipe_name, deadline| {
                    Ok(daemon_status_from_windows_pipe_until(pipe_name, deadline)?
                        .and_then(|live| resolved_windows_daemon_status(status, &live)))
                },
            )
            .map(Some)
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = coven_home;
            let _ = status;
            let _ = deadline;
            Ok(Some(status.clone()))
        }
    }
}

#[cfg(test)]
fn wait_for_windows_running_daemon_with_probe<P>(
    status: &DaemonStatus,
    timeout: Duration,
    probe: P,
) -> Result<bool>
where
    P: FnMut(&str, u32, Duration) -> Result<bool>,
{
    let deadline = LifecycleDeadline::after(timeout)?;
    wait_for_windows_running_daemon_with_probe_until(status, deadline, probe)
}

#[cfg(test)]
fn wait_for_windows_running_daemon_with_probe_until<P>(
    status: &DaemonStatus,
    deadline: LifecycleDeadline,
    mut probe: P,
) -> Result<bool>
where
    P: FnMut(&str, u32, Duration) -> Result<bool>,
{
    wait_for_windows_running_daemon_with_identity_probe_until(
        status,
        deadline,
        |pipe_name, deadline| {
            probe(
                pipe_name,
                status.pid,
                deadline.remaining("waiting for Coven daemon startup health")?,
            )
            .map(|matches| matches.then(|| status.clone()))
        },
    )
    .map(|_| true)
}

#[cfg(any(windows, test))]
fn wait_for_windows_running_daemon_with_identity_probe_until<P>(
    status: &DaemonStatus,
    deadline: LifecycleDeadline,
    mut probe: P,
) -> Result<DaemonStatus>
where
    P: FnMut(&str, LifecycleDeadline) -> Result<Option<DaemonStatus>>,
{
    loop {
        deadline.remaining("waiting for Coven daemon startup health")?;
        if let Some(live) = probe(&status.socket, deadline)? {
            return Ok(live);
        }
        let remaining = deadline.remaining("waiting for Coven daemon startup health")?;
        std::thread::sleep(remaining.min(Duration::from_millis(50)));
    }
}

impl DaemonStopController for SystemDaemonStopController {
    fn stop_verified_daemon(
        &self,
        coven_home: &Path,
        status: &DaemonStatus,
        deadline: LifecycleDeadline,
    ) -> Result<VerifiedStopOutcome> {
        #[cfg(unix)]
        {
            if !unix_status_matches_selected_home(coven_home, status) {
                return Ok(VerifiedStopOutcome::Unverified);
            }
            let expected = coven_client::LifecycleDaemonStatus {
                pid: status.pid,
                started_at: status.started_at.clone(),
                socket: status.socket.clone(),
            };
            Ok(
                match coven_client::shutdown_unix_daemon(
                    coven_home,
                    &expected,
                    deadline.remaining("authenticating and stopping Coven daemon")?,
                )
                .map_err(anyhow::Error::new)?
                {
                    coven_client::UnixDaemonShutdown::Unavailable
                    | coven_client::UnixDaemonShutdown::IdentityMismatch => {
                        VerifiedStopOutcome::Unverified
                    }
                    coven_client::UnixDaemonShutdown::Exited => VerifiedStopOutcome::Exited,
                    coven_client::UnixDaemonShutdown::TimedOut => VerifiedStopOutcome::TimedOut,
                },
            )
        }
        #[cfg(windows)]
        {
            let _ = coven_home;
            let Some(process) = coven_client::open_windows_daemon_process_for_stop_until(
                &status.socket,
                status.pid,
                status.process_creation_time.map(|value| value.get()),
                deadline.instant,
            )
            .map_err(anyhow::Error::new)?
            else {
                return Ok(VerifiedStopOutcome::Unverified);
            };
            anyhow::ensure!(
                process.pid() == status.pid,
                "verified Windows daemon process identity changed before termination"
            );
            Ok(
                if process
                    .terminate_and_wait_until(deadline.instant)
                    .map_err(anyhow::Error::new)?
                {
                    VerifiedStopOutcome::Exited
                } else {
                    VerifiedStopOutcome::TimedOut
                },
            )
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = coven_home;
            let _ = status;
            let _ = deadline;
            Ok(VerifiedStopOutcome::Unverified)
        }
    }

    fn recorded_process_state(&self, status: &DaemonStatus) -> Result<RecordedProcessState> {
        #[cfg(unix)]
        {
            let pid = status.pid;
            let pid = libc::pid_t::try_from(pid).context("daemon PID is outside pid_t range")?;
            if unsafe { libc::kill(pid, 0) } == 0 {
                return Ok(RecordedProcessState::Matching);
            }
            let error = std::io::Error::last_os_error();
            match error.raw_os_error() {
                Some(libc::ESRCH) => Ok(RecordedProcessState::Gone),
                // EPERM proves that the PID exists even though it cannot be
                // inspected or signaled by this process.
                Some(libc::EPERM) => Ok(RecordedProcessState::Unverifiable),
                _ => Err(error).with_context(|| format!("failed to inspect daemon pid {pid}")),
            }
        }
        #[cfg(windows)]
        {
            let live_creation_time = coven_client::windows_process_creation_time(status.pid)
                .map_err(anyhow::Error::new)?;
            Ok(recorded_windows_process_state(status, live_creation_time))
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = status;
            Ok(RecordedProcessState::Gone)
        }
    }

    fn status_matches_running_daemon(
        &self,
        coven_home: &Path,
        status: &DaemonStatus,
        deadline: LifecycleDeadline,
    ) -> Result<bool> {
        #[cfg(unix)]
        {
            if !unix_status_matches_selected_home(coven_home, status) {
                return Ok(false);
            }
            unix_daemon_health_matches_status(
                coven_home,
                status,
                deadline.remaining("probing Coven daemon health")?,
            )
        }
        #[cfg(windows)]
        {
            let _ = coven_home;
            // On Windows, probe the named pipe health endpoint.
            Ok(
                daemon_status_from_windows_pipe_until(&status.socket, deadline)?
                    .map(|live| windows_status_matches_authenticated_health(status, &live))
                    .unwrap_or(false),
            )
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = coven_home;
            let _ = status;
            let _ = deadline;
            Ok(true)
        }
    }

    fn status_from_default_socket(
        &self,
        coven_home: &Path,
        deadline: LifecycleDeadline,
    ) -> Result<Option<DaemonStatus>> {
        daemon_status_from_default_socket_until(coven_home, deadline)
    }

    fn authenticated_running_status(
        &self,
        coven_home: &Path,
        status: &DaemonStatus,
        deadline: LifecycleDeadline,
    ) -> Result<Option<DaemonStatus>> {
        #[cfg(unix)]
        {
            if !unix_status_matches_selected_home(coven_home, status) {
                return Ok(None);
            }
            unix_authenticated_daemon_status(
                coven_home,
                status,
                deadline.remaining("probing Coven daemon health")?,
            )
        }
        #[cfg(windows)]
        {
            let _ = coven_home;
            Ok(
                daemon_status_from_windows_pipe_until(&status.socket, deadline)?
                    .and_then(|live| resolved_windows_daemon_status(status, &live)),
            )
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = coven_home;
            let _ = deadline;
            Ok(Some(status.clone()))
        }
    }
}

#[cfg(test)]
fn stop_background_server_with_controller(
    coven_home: &Path,
    controller: &dyn DaemonStopController,
) -> Result<bool> {
    stop_background_server_with_controller_until(
        coven_home,
        controller,
        LifecycleDeadline::after(Duration::from_secs(2))?,
    )
}

fn stop_background_server_with_controller_until(
    coven_home: &Path,
    controller: &dyn DaemonStopController,
    deadline: LifecycleDeadline,
) -> Result<bool> {
    let coven_home = canonical_lifecycle_home(coven_home)?;
    deadline.remaining("reading Coven daemon lifecycle status")?;
    let status = status_for_stop_until(&coven_home, controller, deadline)?;
    let Some(mut status) = status else {
        return Ok(false);
    };

    let mut retried_recovered_status = false;
    loop {
        deadline.remaining("authenticating and stopping Coven daemon")?;
        match controller
            .stop_verified_daemon(&coven_home, &status, deadline)
            .with_context(|| {
                format!(
                    "failed to stop Coven daemon pid {}; not clearing daemon status",
                    status.pid
                )
            })? {
            VerifiedStopOutcome::Unverified => {
                deadline.remaining("checking Coven daemon process identity")?;
                let process_state = controller.recorded_process_state(&status).with_context(|| {
                    format!(
                        "could not determine whether Coven daemon pid {} is alive with the recorded process identity; not signaling or clearing daemon status",
                        status.pid
                    )
                })?;
                if matches!(
                    process_state,
                    RecordedProcessState::Matching | RecordedProcessState::Unverifiable
                ) {
                    anyhow::bail!(
                        "Coven daemon pid {} could not be verified through its socket; not signaling or clearing daemon status",
                        status.pid
                    );
                }

                let Some(recovered) =
                    recover_authenticated_status_for_stop(&coven_home, controller, deadline)?
                else {
                    break;
                };
                if retried_recovered_status {
                    anyhow::bail!(
                        "recovered Coven daemon identity changed while stopping; refusing to clear daemon status"
                    );
                }
                status = recovered;
                retried_recovered_status = true;
            }
            VerifiedStopOutcome::Exited => break,
            VerifiedStopOutcome::TimedOut => {
                anyhow::bail!(
                    "Coven daemon pid {} did not exit after SIGTERM; not clearing daemon status",
                    status.pid
                );
            }
        }
    }

    clear_status_and_socket_until(&coven_home, deadline)?;
    Ok(true)
}

fn status_for_stop_until(
    coven_home: &Path,
    controller: &dyn DaemonStopController,
    deadline: LifecycleDeadline,
) -> Result<Option<DaemonStatus>> {
    let read_result = read_status_until(coven_home, deadline);
    resolve_status_read_for_stop(coven_home, controller, deadline, read_result)
}

fn resolve_status_read_for_stop(
    coven_home: &Path,
    controller: &dyn DaemonStopController,
    deadline: LifecycleDeadline,
    read_result: Result<Option<DaemonStatus>>,
) -> Result<Option<DaemonStatus>> {
    match read_result {
        Ok(Some(status)) => Ok(Some(status)),
        Ok(None) => recover_authenticated_status_for_stop(coven_home, controller, deadline),
        Err(error) => {
            let parse_error = is_daemon_status_parse_error(&error);
            let preserve_if_unresolved =
                !parse_error || daemon_status_process_creation_time_is_malformed(&error);
            match recover_authenticated_status_for_stop(coven_home, controller, deadline)? {
                Some(status) => Ok(Some(status)),
                None if preserve_if_unresolved => Err(error),
                None => {
                    clear_status_and_socket_until(coven_home, deadline)?;
                    Ok(None)
                }
            }
        }
    }
}

fn recover_authenticated_status_for_stop(
    coven_home: &Path,
    controller: &dyn DaemonStopController,
    deadline: LifecycleDeadline,
) -> Result<Option<DaemonStatus>> {
    deadline.remaining("recovering Coven daemon lifecycle status")?;
    let Some(candidate) = controller
        .status_from_default_socket(coven_home, deadline)
        .context("failed to authenticate the default Coven daemon socket")?
    else {
        return Ok(None);
    };
    let Some(status) = controller.authenticated_running_status(coven_home, &candidate, deadline)?
    else {
        anyhow::bail!(
            "could not authenticate the recovered Coven daemon identity; refusing to stop or start"
        );
    };
    deadline.remaining("publishing recovered Coven daemon status")?;
    write_status(coven_home, &status)?;
    deadline.remaining("publishing recovered Coven daemon status")?;
    Ok(Some(status))
}

#[cfg(test)]
fn background_server_status_with_controller(
    coven_home: &Path,
    controller: &dyn DaemonStopController,
) -> Result<Option<DaemonStatusState>> {
    background_server_status_with_controller_until(
        coven_home,
        controller,
        LifecycleDeadline::after(DAEMON_LIFECYCLE_TIMEOUT)?,
    )
}

fn background_server_status_with_controller_until(
    coven_home: &Path,
    controller: &dyn DaemonStopController,
    deadline: LifecycleDeadline,
) -> Result<Option<DaemonStatusState>> {
    let coven_home = canonical_lifecycle_home(coven_home)?;
    deadline.remaining("reading Coven daemon lifecycle status")?;
    let status = match read_status_until(&coven_home, deadline) {
        Ok(status) => status,
        Err(error) if is_daemon_status_parse_error(&error) => {
            let preserve_if_unresolved = daemon_status_process_creation_time_is_malformed(&error);
            return recover_corrupt_status_for_status_command(
                &coven_home,
                controller,
                preserve_if_unresolved.then_some(error),
                deadline,
            );
        }
        Err(error) => return Err(error),
    };
    let Some(status) = status else {
        return recover_missing_status_from_default_socket(&coven_home, controller, deadline);
    };

    if let Some(running) =
        controller.authenticated_running_status(&coven_home, &status, deadline)?
    {
        deadline.remaining("verifying Coven daemon health")?;
        if running != status {
            deadline.remaining("publishing resolved Coven daemon status")?;
            write_status(&coven_home, &running)?;
        }
        deadline.remaining("publishing resolved Coven daemon status")?;
        return Ok(Some(DaemonStatusState::Running(running)));
    }

    deadline.remaining("checking Coven daemon process identity")?;
    let process_state = controller.recorded_process_state(&status).with_context(|| {
        format!(
            "could not determine whether Coven daemon pid {} is alive with the recorded process identity; not clearing daemon status",
            status.pid
        )
    })?;
    deadline.remaining("checking Coven daemon process identity")?;
    if matches!(
        process_state,
        RecordedProcessState::Matching | RecordedProcessState::Unverifiable
    ) {
        return Ok(Some(DaemonStatusState::Stale(status)));
    }

    if let Some(recovered) =
        recover_missing_status_from_default_socket(&coven_home, controller, deadline)?
    {
        return Ok(Some(recovered));
    }

    clear_status_and_socket_until(&coven_home, deadline)?;
    Ok(None)
}

fn recover_missing_status_from_default_socket(
    coven_home: &Path,
    controller: &dyn DaemonStopController,
    deadline: LifecycleDeadline,
) -> Result<Option<DaemonStatusState>> {
    let Some(status) = controller.status_from_default_socket(coven_home, deadline)? else {
        return Ok(None);
    };

    if let Some(running) = controller.authenticated_running_status(coven_home, &status, deadline)? {
        deadline.remaining("verifying recovered Coven daemon health")?;
        deadline.remaining("publishing recovered Coven daemon status")?;
        write_status(coven_home, &running)?;
        deadline.remaining("publishing recovered Coven daemon status")?;
        return Ok(Some(DaemonStatusState::Running(running)));
    }

    deadline.remaining("checking recovered Coven daemon process identity")?;
    let process_state = controller.recorded_process_state(&status).with_context(|| {
        format!(
            "could not determine whether recovered Coven daemon pid {} still has the recorded process identity",
            status.pid
        )
    })?;
    deadline.remaining("checking recovered Coven daemon process identity")?;
    if matches!(
        process_state,
        RecordedProcessState::Matching | RecordedProcessState::Unverifiable
    ) {
        return Ok(Some(DaemonStatusState::Stale(status)));
    }

    Ok(None)
}

#[cfg(test)]
fn ensure_background_server_with_controllers(
    coven_home: &Path,
    current_exe: &Path,
    started_at: String,
    status_controller: &dyn DaemonStopController,
    start_controller: &dyn DaemonStartController,
) -> Result<DaemonStatus> {
    ensure_background_server_with_controllers_until(
        coven_home,
        current_exe,
        started_at,
        status_controller,
        start_controller,
        LifecycleDeadline::after(DAEMON_LIFECYCLE_TIMEOUT)?,
    )
}

fn ensure_background_server_with_controllers_until(
    coven_home: &Path,
    current_exe: &Path,
    started_at: String,
    status_controller: &dyn DaemonStopController,
    start_controller: &dyn DaemonStartController,
    deadline: LifecycleDeadline,
) -> Result<DaemonStatus> {
    let coven_home = canonical_lifecycle_home(coven_home)?;
    match background_server_status_with_controller_until(&coven_home, status_controller, deadline)?
    {
        Some(DaemonStatusState::Running(status)) => Ok(status),
        Some(DaemonStatusState::Stale(status)) => anyhow::bail!(
            "Coven daemon pid {} is recorded but unreachable; run `coven daemon restart`",
            status.pid
        ),
        None => {
            deadline.remaining("starting Coven daemon")?;
            let launched =
                start_controller.start_background_server(&coven_home, current_exe, started_at)?;
            deadline.remaining("waiting for Coven daemon startup health")?;
            let Some(status) =
                start_controller.wait_for_running_daemon(&coven_home, &launched, deadline)?
            else {
                anyhow::bail!(
                    "started Coven daemon pid {} but its socket did not become ready",
                    launched.pid
                )
            };
            deadline.remaining("completing Coven daemon startup")?;
            Ok(status)
        }
    }
}

fn is_daemon_status_parse_error(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause.downcast_ref::<serde_json::Error>().is_some()
            || cause.downcast_ref::<DaemonStatusParseError>().is_some()
    })
}

fn daemon_status_process_creation_time_is_malformed(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause
            .downcast_ref::<DaemonStatusParseError>()
            .is_some_and(|error| error.process_creation_time_present)
    })
}

fn recover_corrupt_status_for_status_command(
    coven_home: &Path,
    controller: &dyn DaemonStopController,
    unresolved_error: Option<anyhow::Error>,
    deadline: LifecycleDeadline,
) -> Result<Option<DaemonStatusState>> {
    let recovered = match controller.status_from_default_socket(coven_home, deadline) {
        Ok(Some(status)) => {
            controller.authenticated_running_status(coven_home, &status, deadline)?
        }
        Ok(None) | Err(_) => None,
    };
    if let Some(status) = recovered {
        deadline.remaining("verifying recovered Coven daemon health")?;
        deadline.remaining("publishing recovered Coven daemon status")?;
        write_status(coven_home, &status)?;
        deadline.remaining("publishing recovered Coven daemon status")?;
        return Ok(Some(DaemonStatusState::Running(status)));
    }
    if let Some(error) = unresolved_error {
        Err(error)
    } else {
        clear_status_and_socket_until(coven_home, deadline)?;
        Ok(None)
    }
}

fn clear_status_and_socket(coven_home: &Path) -> Result<()> {
    let Some(_serve_lock) = try_acquire_serve_lock(coven_home)? else {
        return Ok(());
    };
    clear_status(coven_home)?;
    let socket = daemon_socket_path(coven_home);
    if socket.exists() {
        std::fs::remove_file(&socket)
            .with_context(|| format!("failed to remove daemon socket {}", socket.display()))?;
    }
    Ok(())
}

fn clear_status_and_socket_until(coven_home: &Path, deadline: LifecycleDeadline) -> Result<()> {
    deadline.remaining("cleaning up Coven daemon lifecycle state")?;
    clear_status_and_socket(coven_home)?;
    deadline.remaining("cleaning up Coven daemon lifecycle state")?;
    Ok(())
}

#[cfg(all(test, unix))]
fn daemon_status_from_default_socket(coven_home: &Path) -> Result<Option<DaemonStatus>> {
    daemon_status_from_default_socket_until(
        coven_home,
        LifecycleDeadline::after(DAEMON_LIFECYCLE_TIMEOUT)?,
    )
}

#[cfg(unix)]
fn daemon_status_from_default_socket_until(
    coven_home: &Path,
    deadline: LifecycleDeadline,
) -> Result<Option<DaemonStatus>> {
    daemon_status_from_health_home(
        coven_home,
        deadline.remaining("probing default Coven daemon socket")?,
    )
}

#[cfg(windows)]
fn daemon_status_from_default_socket_until(
    coven_home: &Path,
    deadline: LifecycleDeadline,
) -> Result<Option<DaemonStatus>> {
    daemon_status_from_windows_pipe_until(&daemon_windows_pipe_name(coven_home)?, deadline)
}

#[cfg(not(any(unix, windows)))]
fn daemon_status_from_default_socket_until(
    coven_home: &Path,
    deadline: LifecycleDeadline,
) -> Result<Option<DaemonStatus>> {
    let _ = coven_home;
    let _ = deadline;
    Ok(None)
}

#[cfg(any(windows, test))]
fn windows_status_matches_authenticated_health(
    recorded: &DaemonStatus,
    live: &DaemonStatus,
) -> bool {
    recorded.pid == live.pid
        && recorded.started_at == live.started_at
        && recorded.socket == live.socket
        && match recorded.process_creation_time {
            Some(recorded_creation_time) => {
                live.process_creation_time == Some(recorded_creation_time)
            }
            None => true,
        }
}

#[cfg(any(windows, test))]
fn resolved_windows_daemon_status(
    recorded: &DaemonStatus,
    live: &DaemonStatus,
) -> Option<DaemonStatus> {
    windows_status_matches_authenticated_health(recorded, live).then(|| live.clone())
}

#[cfg(test)]
fn daemon_status_from_windows_probe<P>(pipe_name: &str, probe: P) -> Result<Option<DaemonStatus>>
where
    P: FnOnce(
        &str,
        Duration,
    )
        -> std::result::Result<Option<(u16, Vec<u8>, u32, u64)>, coven_client::ClientError>,
{
    daemon_status_from_windows_probe_until(
        pipe_name,
        LifecycleDeadline::after(Duration::from_secs(2))?,
        probe,
    )
}

#[cfg(any(windows, test))]
fn daemon_status_from_windows_probe_until<P>(
    pipe_name: &str,
    deadline: LifecycleDeadline,
    probe: P,
) -> Result<Option<DaemonStatus>>
where
    P: FnOnce(
        &str,
        Duration,
    )
        -> std::result::Result<Option<(u16, Vec<u8>, u32, u64)>, coven_client::ClientError>,
{
    let timeout = deadline.remaining("probing Windows daemon health")?;
    let Some((response_status, body, server_pid, server_creation_time)) =
        probe(pipe_name, timeout).map_err(anyhow::Error::new)?
    else {
        return Ok(None);
    };
    anyhow::ensure!(
        response_status == 200,
        "Windows daemon health returned HTTP {response_status}"
    );
    let body: DaemonHealthStatus =
        serde_json::from_slice(&body).context("failed to parse Windows health response")?;
    let Some(mut daemon) = body.daemon.filter(|_| body.ok) else {
        return Ok(None);
    };
    anyhow::ensure!(
        daemon.socket == pipe_name,
        "Windows daemon health reported a pipe for a different Coven home"
    );
    anyhow::ensure!(
        daemon.pid == server_pid,
        "Windows daemon health PID did not match the authenticated pipe server"
    );
    match daemon.process_creation_time {
        Some(reported) => anyhow::ensure!(
            reported.get() == server_creation_time,
            "Windows daemon health process creation time did not match the authenticated pipe server"
        ),
        None => {
            daemon.process_creation_time =
                Some(WindowsProcessCreationTime::new(server_creation_time)?);
        }
    }
    Ok(Some(daemon))
}

#[cfg(all(windows, test))]
fn daemon_status_from_windows_pipe(pipe_name: &str) -> Result<Option<DaemonStatus>> {
    daemon_status_from_windows_pipe_with_timeout(pipe_name, Duration::from_secs(2))
}

#[cfg(all(windows, test))]
fn daemon_status_from_windows_pipe_with_timeout(
    pipe_name: &str,
    timeout: Duration,
) -> Result<Option<DaemonStatus>> {
    daemon_status_from_windows_probe_until(
        pipe_name,
        LifecycleDeadline::after(timeout)?,
        |pipe_name, timeout| {
            coven_client::probe_windows_daemon_health_with_identity(pipe_name, timeout).map(
                |probe| {
                    probe.map(|probe| {
                        (
                            probe.status,
                            probe.body,
                            probe.server_pid,
                            probe.process_creation_time,
                        )
                    })
                },
            )
        },
    )
}

#[cfg(windows)]
fn daemon_status_from_windows_pipe_until(
    pipe_name: &str,
    deadline: LifecycleDeadline,
) -> Result<Option<DaemonStatus>> {
    daemon_status_from_windows_probe_until(pipe_name, deadline, |pipe_name, _| {
        coven_client::probe_windows_daemon_health_with_identity_until(pipe_name, deadline.instant)
            .map(|probe| {
                probe.map(|probe| {
                    (
                        probe.status,
                        probe.body,
                        probe.server_pid,
                        probe.process_creation_time,
                    )
                })
            })
    })
}

/// Read one HTTP response without relying on the peer closing the transport.
///
/// Local sockets and especially Windows named pipes may remain open after a
/// response. HTTP/1.1 frames the body with Content-Length, so consume exactly
/// that many bytes. The reader is expected to be nonblocking; WouldBlock is
/// retried until `timeout` expires.
#[cfg(test)]
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

#[cfg(test)]
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

#[cfg(test)]
fn find_http_header_end(bytes: &[u8]) -> Option<usize> {
    bytes.windows(4).position(|window| window == b"\r\n\r\n")
}

#[cfg(test)]
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
fn unix_daemon_health_matches_status(
    coven_home: &Path,
    expected: &DaemonStatus,
    timeout: Duration,
) -> Result<bool> {
    unix_authenticated_daemon_status(coven_home, expected, timeout).map(|status| status.is_some())
}

#[cfg(unix)]
fn unix_authenticated_daemon_status(
    coven_home: &Path,
    expected: &DaemonStatus,
    timeout: Duration,
) -> Result<Option<DaemonStatus>> {
    if !unix_status_matches_selected_home(coven_home, expected) {
        return Ok(None);
    }
    Ok(coven_client::probe_unix_daemon_health(coven_home, timeout)
        .map_err(anyhow::Error::new)?
        .filter(|status| status.pid == expected.pid && status.started_at == expected.started_at)
        .map(|status| DaemonStatus {
            pid: status.pid,
            started_at: status.started_at,
            socket: status.socket,
            process_creation_time: None,
        }))
}

#[cfg(unix)]
fn daemon_status_from_health_home(
    coven_home: &Path,
    timeout: Duration,
) -> Result<Option<DaemonStatus>> {
    Ok(coven_client::probe_unix_daemon_health(coven_home, timeout)
        .map_err(anyhow::Error::new)?
        .map(|status| DaemonStatus {
            pid: status.pid,
            started_at: status.started_at,
            socket: status.socket,
            process_creation_time: None,
        }))
}

fn canonical_lifecycle_home(coven_home: &Path) -> Result<PathBuf> {
    #[cfg(unix)]
    {
        coven_client::validate_unix_daemon_path_encoding(coven_home).map_err(anyhow::Error::new)?;
        ensure_private_coven_home(coven_home)?;
        coven_client::canonical_unix_daemon_home(coven_home).map_err(anyhow::Error::new)
    }
    #[cfg(not(unix))]
    {
        ensure_private_coven_home(coven_home)?;
        Ok(coven_home.to_path_buf())
    }
}

#[cfg(unix)]
fn unix_status_matches_selected_home(coven_home: &Path, status: &DaemonStatus) -> bool {
    use std::os::unix::fs::MetadataExt;

    let selected = daemon_socket_path(coven_home);
    let recorded = Path::new(&status.socket);
    if recorded.is_absolute() && recorded == selected {
        return true;
    }
    let same_socket = |candidate: &Path| {
        let (Ok(selected_canonical), Ok(recorded_canonical)) = (
            std::fs::canonicalize(&selected),
            std::fs::canonicalize(candidate),
        ) else {
            return false;
        };
        if recorded_canonical != selected_canonical {
            return false;
        }
        let (Ok(selected), Ok(recorded)) = (
            std::fs::metadata(selected_canonical),
            std::fs::metadata(recorded_canonical),
        ) else {
            return false;
        };
        selected.dev() == recorded.dev() && selected.ino() == recorded.ino()
    };
    if recorded.is_absolute() {
        return same_socket(recorded);
    }
    same_socket(recorded)
        || selected
            .parent()
            .into_iter()
            .flat_map(Path::ancestors)
            .any(|base| same_socket(&base.join(recorded)))
}

// `bind_tcp_listener` plus the accepted-stream handler expose the TCP transport
// so it can be tested in isolation; `serve_forever` wires them into the daemon's
// accept loop alongside the Unix socket listener.
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
#[cfg(test)]
pub fn serve_next_tcp_connection(
    listener: &TcpListener,
    coven_home: &Path,
    status: Option<DaemonStatus>,
    runtime: &dyn SessionRuntime,
    allowed_hosts: &[String],
) -> Result<()> {
    let (stream, _) = listener
        .accept()
        .context("failed to accept TCP API connection")?;
    serve_accepted_tcp_connection(stream, coven_home, status, runtime, allowed_hosts)
}

#[cfg(unix)]
fn serve_accepted_tcp_connection(
    stream: TcpStream,
    coven_home: &Path,
    status: Option<DaemonStatus>,
    runtime: &dyn SessionRuntime,
    allowed_hosts: &[String],
) -> Result<()> {
    // Production may place the listener in nonblocking mode so its auxiliary
    // accept thread can observe daemon shutdown. Accepted sockets must retain
    // the ordinary bounded blocking request semantics.
    stream
        .set_nonblocking(false)
        .context("failed to configure accepted TCP API connection")?;
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
        HostGuard::Loopback { allowed_hosts },
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
                daemon_status_from_health_home(coven_home, Duration::from_secs(2))
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

pub fn daemon_recovery_log_path(coven_home: &Path) -> PathBuf {
    coven_home.join("daemon-recovery.log")
}

const DAEMON_RECOVERY_LOG_MAX_BYTES: u64 = 4 * 1024 * 1024;
const DAEMON_RECOVERY_LOG_BACKUPS: u8 = 3;
const DAEMON_RECOVERY_LOG_TRUNCATION_MARKER: &str = "... [truncated]\n";

fn recovery_log_lock() -> &'static Mutex<()> {
    static LOCK: std::sync::OnceLock<Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn format_daemon_recovery_log_entry(timestamp: &str, msg: &str) -> String {
    let prefix = format!("[{timestamp}] ");
    let max_bytes = DAEMON_RECOVERY_LOG_MAX_BYTES as usize;
    let full_len = prefix.len().saturating_add(msg.len()).saturating_add(1);
    let mut line = String::with_capacity(full_len.min(max_bytes));
    line.push_str(&prefix);
    if full_len <= max_bytes {
        line.push_str(msg);
        line.push('\n');
    } else {
        let message_bytes = max_bytes
            .saturating_sub(prefix.len())
            .saturating_sub(DAEMON_RECOVERY_LOG_TRUNCATION_MARKER.len());
        let mut truncate_at = message_bytes.min(msg.len());
        while !msg.is_char_boundary(truncate_at) {
            truncate_at -= 1;
        }
        line.push_str(&msg[..truncate_at]);
        line.push_str(DAEMON_RECOVERY_LOG_TRUNCATION_MARKER);
    }
    line
}

pub fn append_daemon_recovery_log(coven_home: &Path, msg: &str) {
    let path = daemon_recovery_log_path(coven_home);
    let line = format_daemon_recovery_log_entry(&crate::api::current_timestamp(), msg);
    let _guard = recovery_log_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    rotate_recovery_log(
        &path,
        line.len() as u64,
        DAEMON_RECOVERY_LOG_MAX_BYTES,
        DAEMON_RECOVERY_LOG_BACKUPS,
    );
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = f.write_all(line.as_bytes());
    }
}

fn rotate_recovery_log(path: &Path, incoming_bytes: u64, max_bytes: u64, backups: u8) {
    let current_bytes = std::fs::metadata(path)
        .map(|metadata| metadata.len())
        .unwrap_or(0);
    if current_bytes.saturating_add(incoming_bytes) <= max_bytes {
        return;
    }

    for index in (1..=backups).rev() {
        let destination = PathBuf::from(format!("{}.{}", path.display(), index));
        let source = if index == 1 {
            path.to_path_buf()
        } else {
            PathBuf::from(format!("{}.{}", path.display(), index - 1))
        };
        if source.exists() {
            // Windows does not replace an existing destination for
            // `std::fs::rename`, unlike Unix. Only remove a slot immediately
            // before replacing it, so a lone older archive survives a partial
            // prior rotation where its newer source is absent.
            let _ = std::fs::remove_file(&destination);
            let _ = std::fs::rename(source, destination);
        }
    }
}

fn start_threads_proposal_scheduler(coven_home: &Path) -> Result<()> {
    if let Err(error) = crate::api::process_due_threads_proposals(coven_home) {
        append_daemon_recovery_log(
            coven_home,
            &format!("threads scheduler startup pass failed: {error:#}"),
        );
    }
    let home = coven_home.to_path_buf();
    std::thread::Builder::new()
        .name("coven-threads-scheduler".into())
        .spawn(move || loop {
            std::thread::sleep(std::time::Duration::from_secs(30));
            if let Err(error) = crate::api::process_due_threads_proposals(&home) {
                append_daemon_recovery_log(
                    &home,
                    &format!("threads scheduler pass failed: {error:#}"),
                );
            }
        })
        .context("failed to spawn threads proposal scheduler")?;
    Ok(())
}

/// Starts asynchronous, bounded SQLite retention. The initial pass waits for
/// the interval so daemon startup never pays maintenance latency, and the
/// store helper intentionally performs no automatic VACUUM.
fn start_store_maintenance_scheduler(coven_home: &Path) -> Result<()> {
    const INTERVAL: Duration = Duration::from_secs(60);
    let home = coven_home.to_path_buf();
    std::thread::Builder::new()
        .name("coven-store-maintenance".into())
        .spawn(move || loop {
            std::thread::sleep(INTERVAL);
            let now = crate::api::current_timestamp();

            // The store helper owns the bounded convergence loop so one
            // scheduler tick cannot multiply the configured batch budget.
            match crate::store::run_scheduled_maintenance(&home, &now) {
                Ok(report) => refresh_storage_health_after_maintenance(&home, &report),
                Err(error) => {
                    let details = format!("store maintenance pass failed: {error:#}");
                    record_store_maintenance_failure(&home, &details);
                }
            }
        })
        .context("failed to spawn store maintenance scheduler")?;
    Ok(())
}

fn refresh_storage_health_after_maintenance(
    coven_home: &Path,
    report: &crate::store::ScheduledMaintenanceReport,
) {
    if report.blocked_by_free_disk {
        return;
    }

    let store_path = coven_home.join("coven.sqlite3");
    match crate::store::open_initialized_store(&store_path).and_then(|conn| {
        crate::store::refresh_storage_health_snapshot_from_connection(coven_home, &conn, None)
    }) {
        Ok(()) => {}
        Err(error) => {
            let details = format!("storage health snapshot refresh failed: {error:#}");
            record_store_maintenance_failure(coven_home, &details);
        }
    }
}

fn record_store_maintenance_failure(coven_home: &Path, details: &str) {
    record_store_maintenance_failure_with_free_disk_check(
        coven_home,
        details,
        fs2::available_space(coven_home),
    );
}

fn record_store_maintenance_failure_with_free_disk_check(
    coven_home: &Path,
    details: &str,
    free_disk_check: std::io::Result<u64>,
) {
    append_daemon_recovery_log(coven_home, details);
    let known_free_disk_bytes = free_disk_check.as_ref().ok().copied();
    if let Err(error) = crate::store::mark_storage_health_snapshot_maintenance_failure(
        coven_home,
        known_free_disk_bytes,
        None,
    ) {
        append_daemon_recovery_log(
            coven_home,
            &format!("failed to mark storage health snapshot degraded: {error:#}"),
        );
    }
    match free_disk_check {
        Ok(free_disk_bytes) if free_disk_bytes >= crate::store::MAINTENANCE_MIN_FREE_DISK_BYTES => {
            crate::store::record_maintenance_error(coven_home, details);
        }
        Ok(_) => {}
        Err(error) => append_daemon_recovery_log(
            coven_home,
            &format!(
                "store maintenance error not persisted: failed to inspect free disk: {error:#}"
            ),
        ),
    }
}

/// Cleans up the Unix-domain socket file and `daemon.json` when the daemon
/// exits via any path that runs destructors — normal return, `Err` propagation,
/// or panic unwinding. This is what prevents orphaned `~/.coven/coven.sock`
/// files from appearing when the daemon crashes (see OpenCoven/coven#197).
/// SIGKILL bypasses Drop. SIGTERM / SIGINT / SIGHUP only set a signal-safe
/// flag; the accept loop then drains live process ownership before this guard
/// removes endpoint metadata during normal unwinding.
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
    std::fs::File::open(status_path)
        .ok()
        .and_then(|file| read_bounded_daemon_status(file).ok())
        .and_then(|json| serde_json::from_str::<DaemonStatus>(&json).ok())
        .map(|status| status.pid)
}

#[cfg(unix)]
static DAEMON_TERMINATION_REQUESTED: AtomicBool = AtomicBool::new(false);

#[cfg(unix)]
extern "C" fn handle_termination_signal(sig: libc::c_int) {
    // Only async-signal-safe work belongs here. AtomicBool uses a lock-free
    // primitive on Coven's supported Unix targets and write(2) is signal-safe.
    // No allocation, lock, or destructor runs in the handler itself.
    DAEMON_TERMINATION_REQUESTED.store(true, Ordering::Release);
    let msg: &[u8] = b"coven daemon: received termination signal, shutting down\n";
    unsafe {
        libc::write(
            libc::STDERR_FILENO,
            msg.as_ptr() as *const libc::c_void,
            msg.len(),
        );
    }
    let _ = sig;
}

#[cfg(unix)]
fn daemon_termination_requested() -> bool {
    DAEMON_TERMINATION_REQUESTED.load(Ordering::Acquire)
}

#[cfg(unix)]
fn install_termination_signal_handlers(socket_path: &Path, status_path: &Path) -> Result<()> {
    let _socket_cstr = CString::new(socket_path.as_os_str().as_bytes())
        .context("daemon socket path contained an interior NUL")?;
    let _status_cstr = CString::new(status_path.as_os_str().as_bytes())
        .context("daemon status path contained an interior NUL")?;

    DAEMON_TERMINATION_REQUESTED.store(false, Ordering::Release);
    for sig in [libc::SIGTERM, libc::SIGINT, libc::SIGHUP] {
        // SAFETY: sigaction is the documented POSIX API for installing signal
        // handlers; we pass a zero-initialized struct, our handler pointer,
        // and an empty signal mask. Failure returns -1 and sets errno.
        unsafe {
            let mut sa: libc::sigaction = std::mem::zeroed();
            sa.sa_sigaction = handle_termination_signal as *const () as usize;
            libc::sigemptyset(&mut sa.sa_mask);
            // Intentionally no SA_RESTART: accept must return EINTR so the
            // ordinary daemon loop can observe the flag and run cleanup.
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
pub(crate) fn daemon_serve_lock_path(coven_home: &Path) -> PathBuf {
    coven_home.join("daemon-serve.lock")
}

/// Acquire an exclusive, process-lifetime advisory lock so at most one `serve`
/// process ever runs against a given Coven home.
///
/// `ensure_background_server` already serializes `daemon start`/`stop` and reports
/// unreachable duplicates without signaling stale PIDs — but a
/// `coven daemon serve` run *directly* (e.g. from a dev build) bypasses all of
/// that. The socket-takeover guard in
/// `bind_api_socket` only refuses a *healthy* incumbent that answers `/health`;
/// a daemon that is alive but wedged would have its socket reclaimed, leaving
/// two processes writing one SQLite store — the loser then fails the
/// `events_fts` backfill with "database is locked". This OS lock is independent
/// of socket health and of the start path: a live incumbent still holds it, so a
/// duplicate fails fast with a clear message. The OS releases the advisory lock
/// when the file closes — normal exit, panic, or termination — so it never
/// wedges shut.
fn try_acquire_serve_lock(coven_home: &Path) -> Result<Option<std::fs::File>> {
    ensure_private_coven_home(coven_home)?;
    let path = daemon_serve_lock_path(coven_home);
    let file = crate::state_lock::open_lock_file(&path)
        .with_context(|| format!("failed to open serve lock {}", path.display()))?;
    match file.try_lock_exclusive() {
        Ok(()) => Ok(Some(file)),
        Err(error) if crate::state_lock::is_lock_contended(&error) => Ok(None),
        Err(error) => {
            Err(error).with_context(|| format!("failed to acquire serve lock {}", path.display()))
        }
    }
}

fn try_acquire_serve_lock_in(
    coven_home: &Path,
    home_dir: &cap_std::fs::Dir,
) -> Result<Option<std::fs::File>> {
    let path = daemon_serve_lock_path(coven_home);
    let file = crate::state_lock::open_lock_file_in(home_dir, "daemon-serve.lock", &path)
        .with_context(|| format!("failed to open serve lock {}", path.display()))?;
    match file.try_lock_exclusive() {
        Ok(()) => Ok(Some(file)),
        Err(error) if crate::state_lock::is_lock_contended(&error) => Ok(None),
        Err(error) => {
            Err(error).with_context(|| format!("failed to acquire serve lock {}", path.display()))
        }
    }
}

pub(crate) struct ResetDaemonGuard {
    _lifecycle: DaemonLifecycleLock,
    _serve: std::fs::File,
    #[cfg(windows)]
    _pipes: Vec<interprocess::local_socket::Listener>,
}

/// Exclude daemon lifecycle changes for the duration of reset and detect a
/// daemon started by an older CLI that does not hold the repository state lock.
pub(crate) fn try_acquire_reset_guard(
    coven_home: &Path,
    home_dir: &cap_std::fs::Dir,
) -> Result<Option<ResetDaemonGuard>> {
    let Some(lifecycle) = try_acquire_daemon_lifecycle_lock_in(coven_home, home_dir)? else {
        return Ok(None);
    };
    let Some(serve) = try_acquire_serve_lock_in(coven_home, home_dir)? else {
        return Ok(None);
    };
    #[cfg(unix)]
    {
        // Reset must fail closed if an owner-credential probe cannot establish
        // that the socket is unoccupied. This also preserves the historical
        // daemon-active result for legacy listeners with weaker socket modes.
        match unix_daemon_transport_is_occupied(coven_home) {
            Ok(false) => {}
            Ok(true) | Err(_) => return Ok(None),
        }
    }
    #[cfg(windows)]
    let Some(pipes) = try_reserve_windows_daemon_pipes(coven_home)?
    else {
        return Ok(None);
    };
    Ok(Some(ResetDaemonGuard {
        _lifecycle: lifecycle,
        _serve: serve,
        #[cfg(windows)]
        _pipes: pipes,
    }))
}

#[cfg(unix)]
fn unix_daemon_transport_is_occupied(coven_home: &Path) -> Result<bool> {
    unix_daemon_transport_is_occupied_with_timeout(coven_home, Duration::from_secs(2))
}

#[cfg(unix)]
fn unix_daemon_transport_is_occupied_with_timeout(
    coven_home: &Path,
    timeout: Duration,
) -> Result<bool> {
    let socket_path = daemon_socket_path(coven_home);
    match coven_client::probe_unix_daemon_health(coven_home, timeout) {
        Ok(Some(_)) => Ok(true),
        Ok(None) => Ok(false),
        Err(error) => Err(anyhow::anyhow!("{error}")).with_context(|| {
            format!(
                "failed to determine whether daemon socket {} is occupied",
                socket_path.display()
            )
        }),
    }
}

#[cfg(any(windows, test))]
fn reserve_windows_pipe_identities<N, T, F>(
    pipe_names: &[N],
    mut reserve: F,
) -> Result<Option<Vec<T>>>
where
    N: AsRef<str>,
    F: FnMut(&str) -> Result<Option<T>>,
{
    let mut reservations = Vec::with_capacity(pipe_names.len());
    for pipe_name in pipe_names {
        let Some(reservation) = reserve(pipe_name.as_ref())? else {
            return Ok(None);
        };
        reservations.push(reservation);
    }
    Ok(Some(reservations))
}

#[cfg(windows)]
fn try_reserve_windows_daemon_pipes(
    coven_home: &Path,
) -> Result<Option<Vec<interprocess::local_socket::Listener>>> {
    use interprocess::{
        local_socket::{prelude::*, GenericNamespaced, ListenerOptions},
        os::windows::local_socket::ListenerOptionsExt,
    };

    let pipe_names = coven_client::supported_windows_pipe_names(coven_home)
        .context("failed to derive reset pipe reservation names")?;
    reserve_windows_pipe_identities(&pipe_names, |pipe_name| {
        let name = pipe_name
            .to_ns_name::<GenericNamespaced>()
            .context("failed to create reset pipe reservation name")?;
        let security_descriptor = owner_only_pipe_security_descriptor()?;
        match ListenerOptions::new()
            .name(name)
            .security_descriptor(security_descriptor)
            .create_sync()
        {
            Ok(listener) => Ok(Some(listener)),
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::AddrInUse | std::io::ErrorKind::PermissionDenied
                ) =>
            {
                Ok(None)
            }
            Err(error) => Err(error)
                .with_context(|| format!("failed to reserve Windows daemon pipe {pipe_name}")),
        }
    })
}

pub(crate) fn acquire_serve_lock(coven_home: &Path) -> Result<std::fs::File> {
    try_acquire_serve_lock(coven_home)?.ok_or_else(|| {
        anyhow::anyhow!(
            "another Coven daemon is already serving this home (holds {}); refusing to start a \
             second daemon, which would contend for the SQLite store",
            daemon_serve_lock_path(coven_home).display()
        )
    })
}

fn initialize_daemon_store(coven_home: &Path) -> Result<()> {
    let store_path = coven_home.join("coven.sqlite3");
    crate::store::initialize_store(&store_path)?;
    let conn = crate::store::open_initialized_store(&store_path)?;
    crate::hub::initialize_hub_identity(&conn)
        .context("failed to initialize hub identity during daemon startup")?;
    if let Err(error) = crate::hub::refresh_status_snapshot_from_connection(coven_home, &conn) {
        append_daemon_recovery_log(
            coven_home,
            &format!("hub status snapshot refresh failed during daemon startup: {error:#}"),
        );
    }
    if let Err(error) =
        crate::store::refresh_storage_health_snapshot_from_connection(coven_home, &conn, None)
    {
        append_daemon_recovery_log(
            coven_home,
            &format!("storage health snapshot refresh failed during daemon startup: {error:#}"),
        );
        if let Err(cache_error) = crate::store::cache_unavailable_storage_health(coven_home, None) {
            append_daemon_recovery_log(
                coven_home,
                &format!(
                    "failed to cache unavailable storage health during daemon startup: \
                     {cache_error:#}"
                ),
            );
        }
    }
    Ok(())
}

#[cfg(unix)]
pub fn serve_forever(
    coven_home: &Path,
    started_at: String,
    tcp_addr: Option<&str>,
    allowed_hosts: &[String],
) -> Result<()> {
    use std::sync::Arc;
    let canonical_home = canonical_lifecycle_home(coven_home)?;
    let coven_home = canonical_home.as_path();
    // `--allow-host` only widens the TCP guard; it is meaningless without `--tcp`.
    // Warn rather than fail so a stray flag doesn't take the daemon down.
    if !allowed_hosts.is_empty() && tcp_addr.is_none() {
        eprintln!("coven daemon: --allow-host has no effect without --tcp; ignoring");
    }
    // First thing, before touching the socket or the store: take the
    // single-writer serve lock and hold it for the whole process lifetime. It
    // is the authoritative guard against two daemons writing one SQLite store —
    // catching the wedged-but-alive incumbent the socket guard can't, and the
    // direct `daemon serve` path that bypasses ensure_background_server.
    let serve_lock = acquire_serve_lock(coven_home)?;
    initialize_daemon_store(coven_home)?;
    let status = DaemonStatus {
        pid: std::process::id(),
        started_at: started_at.clone(),
        socket: daemon_socket_path(coven_home)
            .to_str()
            .context(
                "canonical Coven daemon socket is not valid UTF-8; daemon status JSON requires \
                 UTF-8 paths",
            )?
            .to_owned(),
        process_creation_time: None,
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
    // A process-directed signal may be delivered to any daemon thread, not
    // necessarily the one blocked in accept(2). Nonblocking accept gives the
    // main loop a bounded opportunity to observe the signal-safe flag even in
    // that case; the explicit EINTR branch below remains the prompt path when
    // the accepting thread receives the signal itself.
    unix_listener
        .set_nonblocking(true)
        .context("failed to configure interruptible Unix API listener")?;
    write_status(coven_home, &status)?;
    let shutdown_guard = ShutdownGuard {
        socket_path: socket_path.clone(),
        status_path: status_path.clone(),
        pid: status.pid,
    };
    let mobile_gateway =
        crate::mobile_memory::gateway::start_mobile_gateway_for_daemon(coven_home)?;
    append_daemon_recovery_log(
        coven_home,
        &format!(
            "daemon starting pid={} socket={}",
            std::process::id(),
            socket_path.display()
        ),
    );
    recover_orphaned_sessions(coven_home, &started_at)?;
    recover_stale_created_sessions(coven_home, &started_at)?;
    recover_orphaned_afs_mounts(coven_home);
    let runtime = Arc::new(LiveSessionRuntime::try_with_coven_home(
        coven_home.to_path_buf(),
    )?);
    start_threads_proposal_scheduler(coven_home)?;
    start_store_maintenance_scheduler(coven_home)?;

    let (tcp_thread, active_tcp_connection) = if let Some(addr) = tcp_addr {
        let tcp_listener = bind_tcp_listener(addr)?;
        tcp_listener
            .set_nonblocking(true)
            .context("failed to configure interruptible TCP API listener")?;
        let tcp_home = coven_home.to_path_buf();
        let tcp_status = status.clone();
        let tcp_runtime = Arc::clone(&runtime);
        let tcp_allowed_hosts: Vec<String> = allowed_hosts.to_vec();
        let active_tcp_connection = Arc::new(Mutex::new(None::<TcpStream>));
        let active_tcp_for_thread = Arc::clone(&active_tcp_connection);
        // TCP accept errors are logged and the loop continues — misbehaving
        // network clients should not bring down the daemon. The Unix loop
        // below uses the same strategy: a single malformed local request must
        // not orphan the socket file (see #197).
        let thread = Some(
            std::thread::Builder::new()
                .name("coven-tcp-api".into())
                .spawn(move || {
                    while !daemon_termination_requested() {
                        let stream = match tcp_listener.accept() {
                            Ok((stream, _)) => stream,
                            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                                std::thread::sleep(std::time::Duration::from_millis(25));
                                continue;
                            }
                            Err(error) => {
                                eprintln!("coven daemon: TCP connection error: {error:#}");
                                std::thread::sleep(std::time::Duration::from_millis(100));
                                continue;
                            }
                        };
                        if daemon_termination_requested() {
                            let _ = stream.shutdown(std::net::Shutdown::Both);
                            break;
                        }
                        let cancellation = match stream.try_clone() {
                            Ok(cancellation) => cancellation,
                            Err(error) => {
                                eprintln!(
                                    "coven daemon: failed to retain TCP shutdown handle: {error:#}"
                                );
                                let _ = stream.shutdown(std::net::Shutdown::Both);
                                continue;
                            }
                        };
                        match active_tcp_for_thread.lock() {
                            Ok(mut active) => *active = Some(cancellation),
                            Err(poisoned) => *poisoned.into_inner() = Some(cancellation),
                        }
                        let result = serve_accepted_tcp_connection(
                            stream,
                            &tcp_home,
                            Some(tcp_status.clone()),
                            tcp_runtime.as_ref(),
                            &tcp_allowed_hosts,
                        );
                        match active_tcp_for_thread.lock() {
                            Ok(mut active) => {
                                active.take();
                            }
                            Err(poisoned) => {
                                poisoned.into_inner().take();
                            }
                        }
                        if let Err(error) = result {
                            // A client hanging up mid-response is expected under SSE +
                            // polling load; don't log or throttle that path.
                            if !is_client_disconnect(&error) {
                                eprintln!("coven daemon: TCP connection error: {error:#}");
                                std::thread::sleep(std::time::Duration::from_millis(100));
                            }
                        }
                    }
                })
                .context("failed to spawn TCP API thread")?,
        );
        (thread, Some(active_tcp_connection))
    } else {
        (None, None)
    };

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
    let shutdown_connections = Arc::new(Mutex::new(Vec::<UnixStream>::new()));
    use std::sync::atomic::Ordering;
    loop {
        if daemon_termination_requested() {
            break;
        }
        let (stream, _) = match unix_listener.accept() {
            Ok(pair) => pair,
            Err(error)
                if error.kind() == std::io::ErrorKind::Interrupted
                    && daemon_termination_requested() =>
            {
                break;
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(std::time::Duration::from_millis(25));
                continue;
            }
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
            match serve_accepted_connection(stream, coven_home, conn_status, runtime.as_ref()) {
                Ok(Some(stream)) => retain_shutdown_connection(&shutdown_connections, stream),
                Ok(None) => {}
                Err(error) => {
                    if !is_client_disconnect(&error) {
                        eprintln!("coven daemon: unix connection error: {error:#}");
                        append_daemon_recovery_log(
                            coven_home,
                            &format!("unix connection error: {error:#}"),
                        );
                    }
                }
            }
            continue;
        }

        inflight.fetch_add(1, Ordering::Relaxed);
        let conn_home = coven_home.to_path_buf();
        let conn_runtime = Arc::clone(&runtime);
        let conn_inflight = Arc::clone(&inflight);
        let conn_shutdown_connections = Arc::clone(&shutdown_connections);
        let spawn_result = std::thread::Builder::new()
            .name("coven-unix-api".into())
            .spawn(move || {
                match serve_accepted_connection(
                    stream,
                    &conn_home,
                    conn_status,
                    conn_runtime.as_ref(),
                ) {
                    Ok(Some(stream)) => {
                        retain_shutdown_connection(&conn_shutdown_connections, stream);
                    }
                    Ok(None) => {}
                    Err(error) => {
                        if !is_client_disconnect(&error) {
                            eprintln!("coven daemon: unix connection error: {error:#}");
                            append_daemon_recovery_log(
                                &conn_home,
                                &format!("unix connection error: {error:#}"),
                            );
                        }
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

    // Close admission and terminate process trees before waiting on any
    // transport worker. In particular, a TCP client may have sent only part of
    // a request and be sitting inside a 30-second socket read; the documented
    // daemon-stop budget is two seconds, so shutdown must not wait for that
    // client before it kills live sessions.
    let runtime_shutdown = runtime
        .shutdown_all()
        .context("failed to terminate live sessions during daemon shutdown");
    let tcp_shutdown = if let Some(tcp_thread) = tcp_thread {
        let deadline = Instant::now() + Duration::from_millis(250);
        while !tcp_thread.is_finished() && Instant::now() < deadline {
            if let Some(active) = active_tcp_connection.as_ref() {
                cancel_active_tcp_connection(active);
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        if tcp_thread.is_finished() {
            tcp_thread
                .join()
                .map_err(|_| anyhow::anyhow!("TCP API thread panicked during daemon shutdown"))
        } else {
            // Dropping JoinHandle detaches only this already-cancelled worker.
            // The daemon process exits immediately after serve_forever returns,
            // so it cannot outlive the process or retain live-session ownership.
            eprintln!(
                "coven daemon: TCP API worker did not stop within 250 ms; detaching for process exit"
            );
            Ok(())
        }
    } else {
        Ok(())
    };
    let shutdown_result = runtime_shutdown.and(tcp_shutdown);
    drop(unix_listener);
    drop(mobile_gateway);
    drop(shutdown_guard);
    drop(serve_lock);
    match shutdown_connections.lock() {
        Ok(mut connections) => connections.clear(),
        Err(poisoned) => poisoned.into_inner().clear(),
    }
    shutdown_result
}

#[cfg(unix)]
fn retain_shutdown_connection(connections: &Mutex<Vec<UnixStream>>, stream: UnixStream) {
    match connections.lock() {
        Ok(mut connections) => connections.push(stream),
        Err(poisoned) => poisoned.into_inner().push(stream),
    }
    DAEMON_TERMINATION_REQUESTED.store(true, Ordering::Release);
}

#[cfg(unix)]
fn cancel_active_tcp_connection(active: &Mutex<Option<TcpStream>>) {
    let stream = match active.lock() {
        Ok(mut active) => active.take(),
        Err(poisoned) => poisoned.into_inner().take(),
    };
    if let Some(stream) = stream {
        let _ = stream.shutdown(std::net::Shutdown::Both);
    }
}

/// Whether `handle_http_stream` runs its CSRF/DNS-rebinding guard, and if so
/// which non-loopback hosts it tolerates. The Unix socket is filesystem-gated
/// and skips the check (`Disabled`); the TCP transport enforces loopback plus
/// the operator's `--allow-host` allowlist (`Loopback`).
#[derive(Clone, Copy)]
enum HostGuard<'a> {
    Disabled,
    Loopback { allowed_hosts: &'a [String] },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LifecycleControl {
    Disabled,
    OwnerLocal,
}

#[derive(Clone, Copy)]
struct HttpStreamPolicy<'a> {
    host_guard: HostGuard<'a>,
    lifecycle: LifecycleControl,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HttpStreamOutcome {
    Complete,
    HoldForShutdown,
}

fn handle_http_stream<R, W>(
    read: R,
    write: W,
    coven_home: &Path,
    status: Option<DaemonStatus>,
    runtime: &dyn SessionRuntime,
    max_body_bytes: Option<usize>,
    guard: HostGuard<'_>,
) -> Result<()>
where
    R: Read,
    W: Write,
{
    handle_http_stream_with_lifecycle(
        read,
        write,
        coven_home,
        status,
        runtime,
        max_body_bytes,
        HttpStreamPolicy {
            host_guard: guard,
            lifecycle: LifecycleControl::Disabled,
        },
    )
    .map(|_| ())
}

fn handle_http_stream_with_lifecycle<R, W>(
    read: R,
    mut write: W,
    coven_home: &Path,
    status: Option<DaemonStatus>,
    runtime: &dyn SessionRuntime,
    max_body_bytes: Option<usize>,
    policy: HttpStreamPolicy<'_>,
) -> Result<HttpStreamOutcome>
where
    R: Read,
    W: Write,
{
    let HttpStreamPolicy {
        host_guard: guard,
        lifecycle,
    } = policy;
    let mut reader = BufReader::new(read);
    let request_line = read_http_request_line(&mut reader)?;
    let (method, path) = parse_request_line(&request_line)?;
    let headers = read_http_headers(&mut reader)?;
    // On the TCP transport (loopback-only), defend against browser-driven CSRF
    // and DNS-rebinding: a real CLI/proxy client never sends a cross-origin
    // Origin, and a rebinding attack arrives with a non-loopback Host. The Unix
    // socket is filesystem-gated and skips this.
    //
    // `allowed_hosts` (from `--allow-host`) widens the guard by an exact-match
    // allowlist so a trusted reverse proxy — e.g. `tailscale serve`, which
    // forwards a fixed tailnet FQDN it cannot rewrite — can reach the API. The
    // bind stays loopback and the API stays unauthenticated; the operator is
    // asserting an authenticated transport (Tailscale/SSH) fronts that host.
    if let HostGuard::Loopback { allowed_hosts } = guard {
        let host = headers.host.as_deref();
        if !host_is_loopback(host) && !host_in_allowlist(host, allowed_hosts) {
            write_forbidden(
                &mut write,
                "Host header must be a loopback or allowed address.",
            )?;
            return Ok(HttpStreamOutcome::Complete);
        }
        if let Some(origin) = headers.origin.as_deref() {
            if !origin_is_loopback(origin) && !origin_in_allowlist(origin, allowed_hosts) {
                write_forbidden(&mut write, "Cross-origin requests are not allowed.")?;
                return Ok(HttpStreamOutcome::Complete);
            }
        }
    }
    if let Some(max) = max_body_bytes {
        if headers.content_length > max {
            write_payload_too_large(&mut write, max)?;
            return Ok(HttpStreamOutcome::Complete);
        }
    }
    const MAX_LIFECYCLE_REQUEST_BODY_BYTES: usize = 16 * 1024;
    if lifecycle == LifecycleControl::OwnerLocal
        && method == "POST"
        && path == "/api/v1/internal/lifecycle/shutdown"
        && headers.content_length > MAX_LIFECYCLE_REQUEST_BODY_BYTES
    {
        write_payload_too_large(&mut write, MAX_LIFECYCLE_REQUEST_BODY_BYTES)?;
        return Ok(HttpStreamOutcome::Complete);
    }
    let body = read_http_body(&mut reader, headers.content_length)?;
    let lifecycle_response = if lifecycle == LifecycleControl::OwnerLocal
        && method == "POST"
        && path == "/api/v1/internal/lifecycle/shutdown"
    {
        Some(lifecycle_shutdown_response(
            status.as_ref(),
            body.as_deref(),
        )?)
    } else {
        None
    };
    let local_control = if matches!(guard, HostGuard::Disabled) {
        crate::mobile_memory::gateway::handle_local_control(method, path, body.as_deref())
    } else {
        None
    };
    let (response, hold_for_shutdown) = if let Some(response) = lifecycle_response {
        response
    } else {
        let response = match local_control.unwrap_or_else(|| {
            let authority = match guard {
                HostGuard::Disabled => crate::api::RequestAuthority::OwnerLocalIpc,
                HostGuard::Loopback { .. } => crate::api::RequestAuthority::Tcp,
            };
            crate::api::handle_request_with_runtime_and_authority(
                method,
                path,
                coven_home,
                status,
                body.as_deref(),
                runtime,
                authority,
            )
        }) {
            Ok(response) => response,
            Err(error) => {
                append_daemon_recovery_log(
                    coven_home,
                    &format!("API handler error for {method} {path}: {error:#}"),
                );
                write_internal_server_error(&mut write, &format!("{error:#}"))?;
                return Ok(HttpStreamOutcome::Complete);
            }
        };
        (response, false)
    };
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
    if hold_for_shutdown {
        Ok(HttpStreamOutcome::HoldForShutdown)
    } else {
        Ok(HttpStreamOutcome::Complete)
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LifecycleShutdownRequest {
    api_version: String,
    daemon: DaemonStatus,
}

fn lifecycle_shutdown_response(
    status: Option<&DaemonStatus>,
    body: Option<&str>,
) -> Result<(crate::api::ApiResponse, bool)> {
    let request = match body
        .filter(|body| !body.trim().is_empty())
        .map(serde_json::from_str::<LifecycleShutdownRequest>)
    {
        Some(Ok(request)) => request,
        Some(Err(error)) => {
            return Ok((
                crate::api::api_error(
                    400,
                    "invalid_request",
                    &format!("invalid daemon shutdown request: {error}"),
                    None,
                )?,
                false,
            ))
        }
        None => {
            return Ok((
                crate::api::api_error(
                    400,
                    "invalid_request",
                    "daemon shutdown request body is required",
                    None,
                )?,
                false,
            ))
        }
    };
    let Some(status) = status else {
        return Ok((
            crate::api::api_error(
                409,
                "daemon_identity_mismatch",
                "Daemon identity is unavailable; refusing lifecycle shutdown.",
                None,
            )?,
            false,
        ));
    };
    if request.api_version != crate::api::COVEN_API_NAMED_VERSION || request.daemon != *status {
        return Ok((
            crate::api::api_error(
                409,
                "daemon_identity_mismatch",
                "Daemon identity did not match; refusing lifecycle shutdown.",
                None,
            )?,
            false,
        ));
    }
    Ok((
        crate::api::json_response(
            202,
            &serde_json::json!({
                "ok": true,
                "apiVersion": crate::api::COVEN_API_NAMED_VERSION,
                "capabilities": { "structuredErrors": true },
                "daemon": status,
            }),
        )?,
        true,
    ))
}

fn write_internal_server_error<W: Write>(write: &mut W, message: &str) -> Result<()> {
    let body = serde_json::json!({
        "ok": false,
        "error": {
            "code": "internal_error",
            "message": message,
        },
    })
    .to_string();
    let http = format!(
        "HTTP/1.1 500 Internal Server Error\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    write
        .write_all(http.as_bytes())
        .context("failed to write 500 response")?;
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

/// Exact (case-insensitive, port-insensitive) match of a request `Host` against
/// the `--allow-host` allowlist. Hostnames are case-insensitive; ports are
/// stripped on both sides. No wildcards — a rebinding attacker must control the
/// exact host the operator vouched for, which they cannot forge over the
/// authenticated transport that fronts it.
fn host_in_allowlist(host: Option<&str>, allowed: &[String]) -> bool {
    match host {
        Some(h) => {
            let h = strip_port(h.trim());
            allowed
                .iter()
                .any(|a| strip_port(a.trim()).eq_ignore_ascii_case(h))
        }
        None => false,
    }
}

/// Same allowlist check applied to a request `Origin` (`scheme://host[:port]`).
fn origin_in_allowlist(origin: &str, allowed: &[String]) -> bool {
    match origin.trim().split_once("://") {
        Some((_scheme, rest)) => {
            let h = strip_port(rest);
            allowed
                .iter()
                .any(|a| strip_port(a.trim()).eq_ignore_ascii_case(h))
        }
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
    serve_accepted_connection(stream, coven_home, status, runtime).map(|_| ())
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
) -> Result<Option<UnixStream>> {
    stream
        .set_nonblocking(false)
        .context("failed to configure accepted Unix API connection")?;
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
    let outcome = handle_http_stream_with_lifecycle(
        read,
        &stream,
        coven_home,
        status,
        runtime,
        Some(MAX_SOCKET_BODY_BYTES),
        HttpStreamPolicy {
            host_guard: HostGuard::Disabled,
            lifecycle: LifecycleControl::OwnerLocal,
        },
    )?;
    Ok((outcome == HttpStreamOutcome::HoldForShutdown).then_some(stream))
}

fn http_reason_phrase(status: u16) -> &'static str {
    match status {
        200 => "OK",
        201 => "Created",
        202 => "Accepted",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        409 => "Conflict",
        413 => "Payload Too Large",
        422 => "Unprocessable Content",
        500 => "Internal Server Error",
        503 => "Service Unavailable",
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

#[cfg(any(windows, test))]
fn owner_only_pipe_sddl(owner_sid: &str) -> String {
    format!("O:{owner_sid}D:(A;;GA;;;OW)")
}

#[cfg(windows)]
fn owner_only_pipe_security_descriptor(
) -> Result<interprocess::os::windows::security_descriptor::SecurityDescriptor> {
    use interprocess::os::windows::security_descriptor::SecurityDescriptor;
    use widestring::U16CString;

    let owner = current_windows_user_sid()?;
    let owner = owner.to_sddl_string()?;
    // Explicitly bind the descriptor owner to TokenUser, rather than allowing
    // Windows to derive it from a configurable TokenOwner. The DACL continues
    // to grant Generic All solely through the OWNER RIGHTS SID.
    let sddl = U16CString::from_str(owner_only_pipe_sddl(&owner))
        .context("failed to encode owner-only named pipe security descriptor")?;
    SecurityDescriptor::deserialize(&sddl)
        .context("failed to build owner-only named pipe security descriptor")
}

#[cfg(windows)]
pub(crate) fn windows_pipe_name(coven_home: &Path) -> Result<String> {
    daemon_windows_pipe_name(coven_home)
}

/// Fully qualified Windows named-pipe endpoint for this Coven profile.
///
/// `windows_pipe_name` remains the name passed to `interprocess`, while
/// diagnostics need the concrete endpoint users can compare against a profile
/// root without treating the IPC surface as absent.
#[cfg(windows)]
pub(crate) fn windows_pipe_path(coven_home: &Path) -> Result<PathBuf> {
    Ok(PathBuf::from(r"\\.\pipe").join(daemon_windows_pipe_name(coven_home)?))
}

/// Put the Windows daemon itself in a process-lifetime kill-on-close Job.
///
/// Children inherit every non-breakaway parent Job at CreateProcess time.
/// This supplies birth-time containment for the narrow interval before a
/// suspended noninteractive child is assigned to its per-session Job. The
/// handle is intentionally retained by the process until kernel teardown; an
/// abrupt `TerminateProcess` from `daemon stop` then closes it and kills every
/// inherited descendant, while the per-session Job remains the checked,
/// explicit cancellation owner during normal operation.
///
/// This Job is a required startup invariant, not an optional backstop. The
/// per-session Job becomes the normal owner only after attachment, so running
/// without the daemon Job would reopen a birth-to-attachment interval where an
/// abrupt daemon exit could orphan the suspended child. Creation,
/// configuration, or assignment failure therefore aborts daemon startup
/// instead of continuing with degraded containment.
#[cfg(windows)]
fn install_daemon_lifetime_job() -> Result<()> {
    use windows_sys::Win32::System::JobObjects::AssignProcessToJobObject;

    install_daemon_lifetime_job_with(|job, process| {
        // SAFETY: both handles are supplied by the checked installer below.
        if unsafe { AssignProcessToJobObject(job, process) } == 0 {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(())
        }
    })
}

#[cfg(windows)]
fn install_daemon_lifetime_job_with(
    assign_process: impl FnOnce(
        windows_sys::Win32::Foundation::HANDLE,
        windows_sys::Win32::Foundation::HANDLE,
    ) -> std::io::Result<()>,
) -> Result<()> {
    use std::mem::size_of;
    use windows_sys::Win32::{
        Foundation::{CloseHandle, INVALID_HANDLE_VALUE},
        System::{
            JobObjects::{
                CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject,
                JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            },
            Threading::GetCurrentProcess,
        },
    };

    unsafe {
        let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        anyhow::ensure!(
            job != INVALID_HANDLE_VALUE && !job.is_null(),
            "failed to create daemon lifetime Job: {}",
            std::io::Error::last_os_error()
        );
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        if SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            &limits as *const _ as *const std::ffi::c_void,
            size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        ) == 0
        {
            let error = std::io::Error::last_os_error();
            CloseHandle(job);
            return Err(error).context("failed to configure daemon lifetime Job");
        }
        if let Err(error) = assign_process(job, GetCurrentProcess()) {
            CloseHandle(job);
            return Err(error).context(
                "failed to assign Coven daemon to its lifetime Job; the parent Job must permit modern nested jobs",
            );
        }
        // HANDLE is Copy and has no Rust destructor. Deliberately do not call
        // CloseHandle: kernel process teardown is the lifetime boundary this
        // Job is designed to enforce.
    }
    Ok(())
}

#[cfg(windows)]
pub fn serve_forever(
    coven_home: &Path,
    started_at: String,
    tcp_addr: Option<&str>,
    allowed_hosts: &[String],
) -> Result<()> {
    serve_forever_with_lifetime_job_installer(
        coven_home,
        started_at,
        tcp_addr,
        allowed_hosts,
        install_daemon_lifetime_job,
    )
}

#[cfg(windows)]
fn serve_forever_with_lifetime_job_installer(
    coven_home: &Path,
    started_at: String,
    tcp_addr: Option<&str>,
    allowed_hosts: &[String],
    install_lifetime_job: impl FnOnce() -> Result<()>,
) -> Result<()> {
    use interprocess::{
        local_socket::{prelude::*, GenericNamespaced, ListenerOptions},
        os::windows::local_socket::ListenerOptionsExt,
    };
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    let _ = tcp_addr; // TCP not wired on Windows in this prototype
    let _ = allowed_hosts; // only meaningful on the (Unix) TCP transport

    let _serve_lock = acquire_serve_lock(coven_home)?;
    install_lifetime_job()?;
    let pipe_name = windows_pipe_name(coven_home)?;
    let status = DaemonStatus {
        pid: std::process::id(),
        started_at: started_at.clone(),
        socket: pipe_name.clone(),
        process_creation_time: Some(WindowsProcessCreationTime::new(
            coven_client::windows_process_creation_time(std::process::id())
                .map_err(anyhow::Error::new)?
                .context("current Windows daemon process was not live during startup")?,
        )?),
    };
    let name = pipe_name
        .to_ns_name::<GenericNamespaced>()
        .context("failed to create named pipe name")?;
    let security_descriptor = owner_only_pipe_security_descriptor()?;
    let listener = ListenerOptions::new()
        .name(name)
        .security_descriptor(security_descriptor)
        .create_sync()
        .context("failed to bind Windows named pipe")?;
    let _mobile_gateway =
        crate::mobile_memory::gateway::start_mobile_gateway_for_daemon(coven_home)?;

    // Claim the pipe before mutating shared daemon/session state. A duplicate
    // daemon must fail at bind without replacing the incumbent's daemon.json
    // or marking sessions owned by that live daemon orphaned.
    initialize_daemon_store(coven_home)?;
    write_status(coven_home, &status)?;
    recover_orphaned_sessions(coven_home, &started_at)?;
    recover_orphaned_afs_mounts(coven_home);

    let runtime = Arc::new(LiveSessionRuntime::try_with_coven_home(
        coven_home.to_path_buf(),
    )?);
    start_threads_proposal_scheduler(coven_home)?;
    start_store_maintenance_scheduler(coven_home)?;

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
                HostGuard::Disabled,
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
                    HostGuard::Disabled,
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

    #[cfg(unix)]
    static DAEMON_TERMINATION_TEST_LOCK: Mutex<()> = Mutex::new(());

    fn test_daemon_status_socket(coven_home: &Path) -> String {
        daemon_startup_status_socket(coven_home).expect("derive test daemon endpoint")
    }

    fn write_test_daemon_status_text(coven_home: &Path, contents: &str) -> Result<()> {
        #[cfg(windows)]
        {
            ensure_private_coven_home(coven_home)?;
            write_windows_status(&daemon_status_path(coven_home), contents)
        }
        #[cfg(not(windows))]
        {
            std::fs::write(daemon_status_path(coven_home), contents)?;
            Ok(())
        }
    }

    #[cfg(windows)]
    fn write_inherited_windows_status(coven_home: &Path, contents: impl AsRef<[u8]>) -> Result<()> {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Security::{
            Authorization::{SetNamedSecurityInfoW, SE_FILE_OBJECT},
            OWNER_SECURITY_INFORMATION,
        };

        let status_path = daemon_status_path(coven_home);
        std::fs::write(&status_path, contents)?;
        let owner = current_windows_user_sid()?;
        let mut status_path: Vec<u16> = status_path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let status = unsafe {
            SetNamedSecurityInfoW(
                status_path.as_mut_ptr(),
                SE_FILE_OBJECT,
                OWNER_SECURITY_INFORMATION,
                owner.as_ptr(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        if status != 0 {
            anyhow::bail!("failed to set test daemon status owner: Windows error {status}");
        }
        Ok(())
    }

    #[test]
    fn windows_status_probe_passes_a_finite_timeout_to_the_shared_client() {
        let observed_timeout = std::cell::Cell::new(None);
        let status = daemon_status_from_windows_probe("coven-daemon-test.sock", |_, timeout| {
            observed_timeout.set(Some(timeout));
            Ok(Some((
                200,
                br#"{"ok":true,"daemon":{"pid":42,"startedAt":"2026-08-16T00:00:00Z","socket":"coven-daemon-test.sock"}}"#
                    .to_vec(),
                42,
                134_157_822_123_456_789,
            )))
        })
        .expect("parse successful shared probe response")
        .expect("running daemon");

        let observed_timeout = observed_timeout.get().expect("finite probe budget");
        assert!(observed_timeout <= Duration::from_secs(2));
        assert!(observed_timeout > Duration::from_secs(1));
        assert_eq!(status.pid, 42);
        assert_eq!(
            status.process_creation_time.map(|value| value.get()),
            Some(134_157_822_123_456_789)
        );
    }

    #[test]
    fn lifecycle_deadline_reuses_one_budget_and_reports_the_expired_phase() {
        let start = Instant::now();
        let deadline = LifecycleDeadline::from_instant(start + Duration::from_millis(100));

        assert_eq!(
            deadline
                .remaining_at(start + Duration::from_millis(40), "authenticating daemon")
                .expect("budget remains"),
            Duration::from_millis(60)
        );
        let error = deadline
            .remaining_at(
                start + Duration::from_millis(100),
                "cleaning up daemon status",
            )
            .expect_err("the original deadline is exhausted");
        assert!(
            error
                .to_string()
                .contains("timed out cleaning up daemon status"),
            "unexpected phase error: {error:#}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn expired_lifecycle_deadline_skips_optional_duplicate_scan_deterministically() {
        let scanned = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let scanned_in_diagnostic = std::sync::Arc::clone(&scanned);
        let expired = LifecycleDeadline::from_instant(Instant::now());

        run_optional_lifecycle_diagnostic(expired, move |_| {
            scanned_in_diagnostic.store(true, std::sync::atomic::Ordering::SeqCst);
        });

        assert!(
            !scanned.load(std::sync::atomic::Ordering::SeqCst),
            "an optional process scan started after the lifecycle deadline"
        );
    }

    #[cfg(unix)]
    #[test]
    fn restart_returns_within_deadline_when_optional_diagnostic_scan_is_slow() -> Result<()> {
        struct DiagnosticDelayGuard;

        impl Drop for DiagnosticDelayGuard {
            fn drop(&mut self) {
                OPTIONAL_DIAGNOSTIC_DELAY_MILLIS.store(0, std::sync::atomic::Ordering::SeqCst);
            }
        }

        let _guard = DAEMON_TERMINATION_TEST_LOCK.lock().unwrap();
        let temp_dir = tempfile::tempdir()?;
        let status = DaemonStatus {
            pid: 12345,
            started_at: "old".to_owned(),
            socket: daemon_socket_path(temp_dir.path())
                .to_string_lossy()
                .into_owned(),
            process_creation_time: None,
        };
        write_status(temp_dir.path(), &status)?;
        let started = std::sync::Arc::new(std::sync::Mutex::new(0));
        OPTIONAL_DIAGNOSTIC_DELAY_MILLIS.store(500, std::sync::atomic::Ordering::SeqCst);
        let _delay_guard = DiagnosticDelayGuard;
        let deadline = LifecycleDeadline::after(Duration::from_millis(100))?;
        let started_at = Instant::now();

        let restarted = restart_background_server_with_controllers_until(
            temp_dir.path(),
            Path::new("/usr/bin/coven"),
            "new".to_owned(),
            &FakeStopController {
                pid_alive: true,
                exited_after_signal: true,
                signal_error: None,
                verified_daemon: true,
                signaled: std::sync::Arc::default(),
            },
            &FakeStartController {
                started,
                running_after_start: true,
            },
            deadline,
        )?;

        assert!(restarted.0);
        assert!(
            started_at.elapsed() < Duration::from_millis(250),
            "optional diagnostic scan delayed restart completion"
        );
        Ok(())
    }

    #[test]
    fn expired_windows_health_deadline_never_runs_a_fixed_followup_probe() {
        let invoked = std::cell::Cell::new(false);
        let error = daemon_status_from_windows_probe_until(
            "coven-daemon-test.sock",
            LifecycleDeadline::from_instant(Instant::now()),
            |_, _| {
                invoked.set(true);
                Ok::<_, coven_client::ClientError>(None)
            },
        )
        .expect_err("an expired outer deadline must stop before probing");

        assert!(!invoked.get());
        assert!(
            error
                .to_string()
                .contains("timed out probing Windows daemon health"),
            "unexpected timeout phase: {error:#}"
        );
    }

    #[test]
    fn windows_start_wait_does_not_probe_again_after_its_total_budget() -> Result<()> {
        let status = parse_daemon_status(&windows_status_fixture(None))?;
        let calls = std::cell::Cell::new(0);
        let started = Instant::now();

        let error = wait_for_windows_running_daemon_with_probe(
            &status,
            Duration::from_millis(20),
            |_, _, _| {
                calls.set(calls.get() + 1);
                std::thread::sleep(Duration::from_millis(25));
                Ok(false)
            },
        )
        .expect_err("the single outer start budget must expire");

        assert_eq!(calls.get(), 1, "no fixed post-deadline probe is allowed");
        assert!(
            started.elapsed() < Duration::from_millis(100),
            "start readiness materially overshot its 20ms budget"
        );
        assert!(
            error
                .to_string()
                .contains("timed out waiting for Coven daemon startup health"),
            "unexpected timeout phase: {error:#}"
        );
        Ok(())
    }

    #[test]
    fn stop_cleanup_does_not_run_after_the_outer_lifecycle_deadline() -> Result<()> {
        struct SlowVerifiedStop;
        impl DaemonStopController for SlowVerifiedStop {
            fn stop_verified_daemon(
                &self,
                _coven_home: &Path,
                _status: &DaemonStatus,
                _deadline: LifecycleDeadline,
            ) -> Result<VerifiedStopOutcome> {
                std::thread::sleep(Duration::from_millis(25));
                Ok(VerifiedStopOutcome::Exited)
            }

            fn recorded_process_state(
                &self,
                _status: &DaemonStatus,
            ) -> Result<RecordedProcessState> {
                Ok(RecordedProcessState::Gone)
            }

            fn status_matches_running_daemon(
                &self,
                _coven_home: &Path,
                _status: &DaemonStatus,
                _deadline: LifecycleDeadline,
            ) -> Result<bool> {
                Ok(false)
            }
        }

        let temp_dir = tempfile::tempdir()?;
        let status = DaemonStatus {
            pid: 42,
            started_at: "2026-08-16T00:00:00Z".to_owned(),
            socket: test_daemon_status_socket(temp_dir.path()),
            process_creation_time: None,
        };
        write_status(temp_dir.path(), &status)?;
        let error = stop_background_server_with_controller_until(
            temp_dir.path(),
            &SlowVerifiedStop,
            LifecycleDeadline::after(Duration::from_millis(10))?,
        )
        .expect_err("cleanup must not start after the original stop budget");

        assert!(
            error
                .to_string()
                .contains("timed out cleaning up Coven daemon lifecycle state"),
            "unexpected timeout phase: {error:#}"
        );
        assert_eq!(read_status(temp_dir.path())?, Some(status));
        Ok(())
    }

    #[test]
    fn windows_status_probe_preserves_unavailable_as_not_running() {
        let status = daemon_status_from_windows_probe("coven-daemon-test.sock", |_, _| {
            Ok::<_, coven_client::ClientError>(None)
        })
        .expect("an unavailable owner-safe connection is not an error");

        assert_eq!(status, None);
    }

    #[test]
    fn windows_status_probe_rejects_health_identity_not_bound_to_pipe_server() {
        for (body, server_pid, server_creation_time) in [
            (
                br#"{"ok":true,"daemon":{"pid":41,"startedAt":"now","socket":"coven-daemon-test.sock","processCreationTime":"134157822123456789"}}"#.as_slice(),
                42,
                134_157_822_123_456_789,
            ),
            (
                br#"{"ok":true,"daemon":{"pid":42,"startedAt":"now","socket":"other-profile.sock","processCreationTime":"134157822123456789"}}"#.as_slice(),
                42,
                134_157_822_123_456_789,
            ),
            (
                br#"{"ok":true,"daemon":{"pid":42,"startedAt":"now","socket":"coven-daemon-test.sock","processCreationTime":"134157822123456788"}}"#.as_slice(),
                42,
                134_157_822_123_456_789,
            ),
        ] {
            let result = daemon_status_from_windows_probe("coven-daemon-test.sock", |_, _| {
                Ok(Some((
                    200,
                    body.to_vec(),
                    server_pid,
                    server_creation_time,
                )))
            });
            assert!(result.is_err(), "accepted unbound health body");
        }
    }

    #[test]
    fn windows_recorded_status_matches_only_its_authenticated_process_identity() -> Result<()> {
        let legacy = parse_daemon_status(&windows_status_fixture(None))?;
        let recorded = parse_daemon_status(&windows_status_fixture(Some("134157822123456789")))?;
        let matching = parse_daemon_status(&windows_status_fixture(Some("134157822123456789")))?;
        let reused = parse_daemon_status(&windows_status_fixture(Some("134157822123456790")))?;

        assert!(windows_status_matches_authenticated_health(
            &recorded, &matching
        ));
        assert!(!windows_status_matches_authenticated_health(
            &recorded, &reused
        ));
        assert!(
            windows_status_matches_authenticated_health(&legacy, &matching),
            "an authenticated health response may resolve a legacy record"
        );
        assert_eq!(
            resolved_windows_daemon_status(&legacy, &matching),
            Some(matching.clone()),
            "secure health resolution should migrate the legacy fingerprint"
        );
        assert_eq!(resolved_windows_daemon_status(&recorded, &reused), None);
        Ok(())
    }

    #[test]
    fn windows_daemon_token_storage_is_word_aligned_and_covers_requested_bytes() {
        for requested_bytes in [1, 3, 8, 31, 257] {
            let mut buffer = WindowsTokenBuffer::new(requested_bytes);
            assert_eq!(
                buffer.as_mut_ptr() as usize % std::mem::align_of::<usize>(),
                0
            );
            assert!(buffer.byte_capacity() >= requested_bytes);
        }
    }

    #[test]
    fn named_pipe_sddl_sets_token_user_as_owner_and_preserves_owner_only_dacl() {
        assert_eq!(
            owner_only_pipe_sddl("S-1-5-21-42"),
            "O:S-1-5-21-42D:(A;;GA;;;OW)"
        );
    }

    #[test]
    fn owner_only_windows_directory_dacl_inherits_to_children() {
        assert_eq!(
            WINDOWS_OWNER_ONLY_DIRECTORY_DACL_SDDL,
            "D:P(A;OICI;GA;;;OW)"
        );
    }

    #[test]
    fn daemon_store_initialization_prepares_request_connections() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        initialize_daemon_store(temp_dir.path())?;
        let store_path = temp_dir.path().join("coven.sqlite3");
        let conn = crate::store::open_initialized_store(&store_path)?;
        assert!(crate::store::list_sessions(&conn)?.is_empty());
        let hub_id: String = conn.query_row(
            "SELECT value FROM store_meta WHERE key = ?1",
            [crate::hub::HUB_ID_META_KEY],
            |row| row.get(0),
        )?;
        assert!(hub_id.starts_with("hub_"));

        initialize_daemon_store(temp_dir.path())?;
        let health = crate::api::handle_request("GET", "/health", temp_dir.path(), None)?;
        assert_eq!(health.status, 200);
        let health: serde_json::Value = serde_json::from_str(&health.body)?;
        assert_eq!(health["hub"]["hubId"], hub_id);
        assert_eq!(health["hub"]["nodesTotal"], 0);
        assert_eq!(health["hub"]["nodesAvailable"], 0);

        let summary = crate::hub::hub_health_summary(temp_dir.path())?;
        assert_eq!(summary["hubId"], hub_id);
        assert_eq!(summary["nodesTotal"], 0);
        assert_eq!(summary["nodesAvailable"], 0);

        let status = crate::hub::hub_status(temp_dir.path())?;
        assert_eq!(status.status, 200);
        let status: serde_json::Value = serde_json::from_str(&status.body)?;
        assert_eq!(status["hubId"], hub_id);
        assert_eq!(status["nodesTotal"], 0);
        assert_eq!(status["nodesAvailable"], 0);
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn termination_signal_requests_graceful_cleanup_instead_of_immediate_exit() {
        let _test_lock = DAEMON_TERMINATION_TEST_LOCK
            .lock()
            .expect("termination test lock");
        DAEMON_TERMINATION_REQUESTED.store(false, Ordering::Release);
        handle_termination_signal(libc::SIGTERM);
        assert!(daemon_termination_requested());
        DAEMON_TERMINATION_REQUESTED.store(false, Ordering::Release);
    }

    #[cfg(windows)]
    #[test]
    fn windows_daemon_startup_fails_closed_when_lifetime_job_assignment_fails() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let error = serve_forever_with_lifetime_job_installer(
            temp_dir.path(),
            "2026-08-11T00:00:00Z".to_string(),
            None,
            &[],
            || {
                install_daemon_lifetime_job_with(|_, _| {
                    Err(std::io::Error::new(
                        std::io::ErrorKind::PermissionDenied,
                        "injected assignment refusal",
                    ))
                })
            },
        )
        .expect_err("daemon startup must reject a missing lifetime Job");

        let diagnostic = format!("{error:#}");
        assert!(
            diagnostic.contains("failed to assign Coven daemon to its lifetime Job"),
            "{diagnostic}"
        );
        assert!(
            diagnostic.contains("injected assignment refusal"),
            "{diagnostic}"
        );
        assert_eq!(
            read_status(temp_dir.path())?,
            None,
            "failed lifetime Job assignment published daemon readiness"
        );
        Ok(())
    }

    #[test]
    fn daemon_store_initialization_degrades_health_for_malformed_privacy_config() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        std::fs::write(
            temp_dir.path().join("privacy.toml"),
            "log_retention_days = \"broken\"\n",
        )?;

        initialize_daemon_store(temp_dir.path())?;

        let health = crate::api::handle_request("GET", "/health", temp_dir.path(), None)?;
        assert_eq!(health.status, 200);
        let health: serde_json::Value = serde_json::from_str(&health.body)?;
        assert_eq!(health["storage"]["status"], "degraded");
        assert_eq!(
            health["storage"]["lastMaintenanceError"],
            "storage health unavailable"
        );
        assert!(!health
            .to_string()
            .contains(temp_dir.path().to_string_lossy().as_ref()));

        let hub_status = crate::hub::hub_status(temp_dir.path())?;
        assert_eq!(hub_status.status, 200);
        Ok(())
    }

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
    fn owner_local_lifecycle_shutdown_requires_the_exact_serving_identity() -> Result<()> {
        let _test_lock = DAEMON_TERMINATION_TEST_LOCK
            .lock()
            .expect("termination test lock");
        let temp = tempfile::tempdir()?;
        let status = DaemonStatus {
            pid: 42,
            started_at: "2026-08-16T12:00:00Z".to_owned(),
            socket: daemon_socket_path(temp.path())
                .to_string_lossy()
                .into_owned(),
            process_creation_time: None,
        };
        let wrong = DaemonStatus {
            pid: 43,
            ..status.clone()
        };
        let body = serde_json::json!({
            "apiVersion": crate::api::COVEN_API_NAMED_VERSION,
            "daemon": wrong,
        })
        .to_string();
        let request = format!(
            "POST /api/v1/internal/lifecycle/shutdown HTTP/1.1\r\nHost: coven\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        );
        let mut input = std::io::Cursor::new(request.into_bytes());
        let mut output = Vec::new();
        DAEMON_TERMINATION_REQUESTED.store(false, Ordering::Release);

        let outcome = handle_http_stream_with_lifecycle(
            &mut input,
            &mut output,
            temp.path(),
            Some(status),
            &crate::api::NoopSessionRuntime,
            Some(MAX_SOCKET_BODY_BYTES),
            HttpStreamPolicy {
                host_guard: HostGuard::Disabled,
                lifecycle: LifecycleControl::OwnerLocal,
            },
        )?;

        assert_eq!(outcome, HttpStreamOutcome::Complete);
        assert!(String::from_utf8(output)?.starts_with("HTTP/1.1 409 Conflict\r\n"));
        assert!(!daemon_termination_requested());
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn owner_local_lifecycle_shutdown_holds_its_authenticated_connection() -> Result<()> {
        let _test_lock = DAEMON_TERMINATION_TEST_LOCK
            .lock()
            .expect("termination test lock");
        let temp = tempfile::tempdir()?;
        let status = DaemonStatus {
            pid: 42,
            started_at: "2026-08-16T12:00:00Z".to_owned(),
            socket: daemon_socket_path(temp.path())
                .to_string_lossy()
                .into_owned(),
            process_creation_time: None,
        };
        let body = serde_json::json!({
            "apiVersion": crate::api::COVEN_API_NAMED_VERSION,
            "daemon": status,
        })
        .to_string();
        let request = format!(
            "POST /api/v1/internal/lifecycle/shutdown HTTP/1.1\r\nHost: coven\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        );
        let mut input = std::io::Cursor::new(request.into_bytes());
        let mut output = Vec::new();
        DAEMON_TERMINATION_REQUESTED.store(false, Ordering::Release);

        let outcome = handle_http_stream_with_lifecycle(
            &mut input,
            &mut output,
            temp.path(),
            Some(status),
            &crate::api::NoopSessionRuntime,
            Some(MAX_SOCKET_BODY_BYTES),
            HttpStreamPolicy {
                host_guard: HostGuard::Disabled,
                lifecycle: LifecycleControl::OwnerLocal,
            },
        )?;

        assert_eq!(outcome, HttpStreamOutcome::HoldForShutdown);
        assert!(String::from_utf8(output)?.starts_with("HTTP/1.1 202 Accepted\r\n"));
        assert!(
            !daemon_termination_requested(),
            "shutdown cannot begin before the authenticated connection is retained"
        );
        let (server, _client) = UnixStream::pair()?;
        let retained = Mutex::new(Vec::new());
        retain_shutdown_connection(&retained, server);
        assert!(daemon_termination_requested());
        assert_eq!(retained.into_inner().expect("retained lock").len(), 1);
        DAEMON_TERMINATION_REQUESTED.store(false, Ordering::Release);
        Ok(())
    }

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
    fn reset_pipe_reservation_retains_every_supported_identity_until_release() -> Result<()> {
        struct TrackedReservation(std::sync::Arc<std::sync::atomic::AtomicUsize>);
        impl Drop for TrackedReservation {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
        }

        let names = ["v2", "v1", "v0"];
        let attempted = std::cell::RefCell::new(Vec::new());
        let released = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let reservations = reserve_windows_pipe_identities(&names, |name| {
            attempted.borrow_mut().push(name.to_owned());
            Ok(Some(TrackedReservation(released.clone())))
        })?
        .expect("all identities were available");

        assert_eq!(&*attempted.borrow(), &names);
        assert_eq!(released.load(Ordering::SeqCst), 0);
        drop(reservations);
        assert_eq!(released.load(Ordering::SeqCst), names.len());
        Ok(())
    }

    #[test]
    fn reset_pipe_reservation_releases_partial_handles_on_legacy_collision() -> Result<()> {
        struct TrackedReservation(std::sync::Arc<std::sync::atomic::AtomicUsize>);
        impl Drop for TrackedReservation {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
        }

        let names = ["v2", "v1", "v0"];
        let attempted = std::cell::Cell::new(0);
        let released = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let reservations = reserve_windows_pipe_identities(&names, |_| {
            let index = attempted.get();
            attempted.set(index + 1);
            if index == 1 {
                Ok(None)
            } else {
                Ok(Some(TrackedReservation(released.clone())))
            }
        })?;

        assert!(reservations.is_none());
        assert_eq!(attempted.get(), 2);
        assert_eq!(released.load(Ordering::SeqCst), 1);
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn reset_transport_probe_bounds_an_unresponsive_socket() -> Result<()> {
        use std::os::unix::fs::PermissionsExt;

        let home = tempfile::tempdir()?;
        std::fs::set_permissions(home.path(), std::fs::Permissions::from_mode(0o700))?;
        let socket = daemon_socket_path(home.path());
        let _listener = UnixListener::bind(&socket)?;
        std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600))?;
        let started = Instant::now();

        let error =
            unix_daemon_transport_is_occupied_with_timeout(home.path(), Duration::from_millis(50))
                .unwrap_err();

        assert!(
            format!("{error:#}").contains("timed out"),
            "unexpected probe failure: {error:#}"
        );
        assert!(started.elapsed() < Duration::from_secs(1));
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn daemon_locks_refuse_symlinks_without_mutating_the_target() -> Result<()> {
        use std::os::unix::fs::{symlink, PermissionsExt};

        let home = tempfile::tempdir()?;
        let outside = tempfile::NamedTempFile::new()?;
        outside
            .as_file()
            .set_permissions(std::fs::Permissions::from_mode(0o640))?;
        symlink(outside.path(), daemon_lifecycle_lock_path(home.path()))?;

        assert!(acquire_daemon_lifecycle_lock(home.path()).is_err());
        assert_eq!(
            outside.as_file().metadata()?.permissions().mode() & 0o777,
            0o640
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn duplicate_daemon_candidate_rejects_threads_but_accepts_processes() {
        use std::ffi::OsString;
        use sysinfo::ThreadKind;

        let coven_home = Path::new("/home/coven");
        let cmd = [
            OsString::from("coven"),
            OsString::from("daemon"),
            OsString::from("serve"),
        ];
        let environ = [OsString::from("COVEN_HOME=/home/coven")];

        assert!(process_is_unreachable_duplicate_daemon_candidate(
            42, 41, None, &cmd, &environ, coven_home,
        ));
        for thread_kind in [ThreadKind::Userland, ThreadKind::Kernel] {
            assert!(
                !process_is_unreachable_duplicate_daemon_candidate(
                    42,
                    41,
                    Some(thread_kind),
                    &cmd,
                    &environ,
                    coven_home,
                ),
                "{thread_kind:?} thread must not be a duplicate-daemon candidate"
            );
        }
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
    fn output_observer_coalesces_callbacks_and_flushes_before_exit() -> Result<()> {
        // UTF-8 boundary safety lives in pty_runner::drain_detached_output
        // now (see its tests). The writer may combine adjacent chunks, but
        // every accepted byte must be persisted before the terminal event.
        let temp_dir = tempfile::tempdir()?;
        let conn = crate::store::open_store(&temp_dir.path().join("coven.sqlite3"))?;
        let session = session_record("buffered");
        crate::store::insert_session(&conn, &session)?;

        let observer = output_observer(temp_dir.path().to_path_buf(), session.id.clone());
        let pty_runner::DetachedPtyObserver {
            mut on_output,
            on_exit,
        } = observer;

        // The drain layer would only ever hand us valid-UTF-8 slices,
        // so simulate that: a complete emoji and then a plain ASCII
        // chunk, each fully decodable on its own.
        on_output("🎉".as_bytes().to_vec());
        on_output(b" done".to_vec());
        on_exit(pty_runner::PtyRunResult {
            status: "completed",
            exit_code: Some(0),
        });

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
    fn startup_timeout_observer_marks_running_session_failed_with_diagnostic() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let conn = crate::store::open_store(&temp_dir.path().join("coven.sqlite3"))?;
        let mut session = session_record("startup-timeout");
        session.status = "running".to_string();
        crate::store::insert_session(&conn, &session)?;

        let observer = output_observer(temp_dir.path().to_path_buf(), session.id.clone());
        let pty_runner::DetachedPtyObserver {
            mut on_output,
            on_exit,
        } = observer;
        on_output(
            b"Coven stopped the detached PTY: no meaningful output was produced before the startup timeout (50 ms).\n"
                .to_vec(),
        );
        on_exit(pty_runner::PtyRunResult {
            status: "failed",
            exit_code: None,
        });

        let persisted = crate::store::get_session(&conn, &session.id)?.unwrap();
        assert_eq!(persisted.status, "failed");
        assert_eq!(persisted.exit_code, None);
        let events = crate::store::list_events(&conn, &session.id)?;
        assert!(events.iter().any(|event| {
            event.kind == "output" && event.payload_json.contains("no meaningful output")
        }));
        assert!(events.iter().any(|event| {
            event.kind == "exit"
                && event.payload_json.contains("failed")
                && event.payload_json.contains("null")
        }));
        Ok(())
    }

    #[cfg(windows)]
    #[test]
    fn native_detached_pty_success_persists_marker_and_completed_exit() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let coven_home = temp_dir.path().join("home");
        std::fs::create_dir_all(&coven_home)?;
        let conn = crate::store::open_store(&coven_home.join("coven.sqlite3"))?;
        let session = session_record("native-pty-success");
        crate::store::insert_session(&conn, &session)?;
        let trace_file = temp_dir.path().join("query-trace.txt");
        let command = pty_runner::windows_detached_stub_command(
            temp_dir.path(),
            "queries",
            Some(&trace_file),
        )?;
        let observer = output_observer(coven_home.clone(), session.id.clone());

        let _detached = pty_runner::spawn_detached_with_observer_for_test(
            &command,
            observer,
            Duration::from_secs(5),
        )?;
        let persisted =
            wait_for_session_status(&conn, &session.id, "completed", Duration::from_secs(10))?;

        assert_eq!(persisted.exit_code, Some(0));
        wait_for_session_event(&conn, &session.id, "exit", Duration::from_secs(3))?;
        let events = crate::store::list_events(&conn, &session.id)?;
        let payloads = events
            .iter()
            .map(|event| event.payload_json.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(payloads.contains("WINDOWS_PTY_STUB_OK_🎉"), "{payloads}");
        for query in ["\\u001b[6n", "\\u001b[c", "\\u001b[0c", "\\u001b[5n"] {
            assert!(!payloads.contains(query), "query leaked: {query:?}");
        }
        assert!(events.iter().any(|event| {
            event.kind == "exit"
                && event.payload_json.contains("completed")
                && event.payload_json.contains("0")
        }));
        let trace = std::fs::read_to_string(trace_file)?;
        assert!(trace.starts_with("started mode="), "{trace:?}");
        for stage in ["cpr", "da", "status", "da0"] {
            assert!(trace.lines().any(|line| line == stage), "{trace:?}");
        }
        std::fs::remove_file(temp_dir.path().join("windows-detached-pty-stub.exe"))
            .context("native PTY stub executable remained in use after completed exit")?;
        Ok(())
    }

    #[cfg(windows)]
    #[test]
    fn native_detached_pty_timeout_persists_failed_exit_and_kills_tree() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let coven_home = temp_dir.path().join("home");
        std::fs::create_dir_all(&coven_home)?;
        let conn = crate::store::open_store(&coven_home.join("coven.sqlite3"))?;
        let session = session_record("native-pty-timeout");
        crate::store::insert_session(&conn, &session)?;
        let pid_file = temp_dir.path().join("descendant.pid");
        let command =
            pty_runner::windows_detached_stub_command(temp_dir.path(), "timeout", Some(&pid_file))?;
        let observer = output_observer(coven_home.clone(), session.id.clone());

        let _detached = pty_runner::spawn_detached_with_observer_for_test(
            &command,
            observer,
            Duration::from_secs(2),
        )?;
        let persisted =
            wait_for_session_status(&conn, &session.id, "failed", Duration::from_secs(10))?;
        let descendant_pid: u32 = std::fs::read_to_string(&pid_file)?.trim().parse()?;

        assert_eq!(persisted.exit_code, None);
        wait_for_session_event(&conn, &session.id, "exit", Duration::from_secs(3))?;
        let events = crate::store::list_events(&conn, &session.id)?;
        let payloads = events
            .iter()
            .map(|event| event.payload_json.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(payloads.contains("no meaningful output"), "{payloads}");
        assert!(!payloads.contains("\\u001b[6n"), "{payloads}");
        assert!(events.iter().any(|event| {
            event.kind == "exit"
                && event.payload_json.contains("failed")
                && event.payload_json.contains("null")
        }));
        assert!(
            wait_for_windows_process_exit(descendant_pid, Duration::from_secs(3)),
            "startup timeout left descendant process {descendant_pid} running"
        );
        std::fs::remove_file(temp_dir.path().join("windows-detached-pty-stub.exe"))
            .context("native PTY stub executable remained in use after timeout")?;
        Ok(())
    }

    #[cfg(windows)]
    fn wait_for_session_status(
        conn: &rusqlite::Connection,
        session_id: &str,
        expected: &str,
        timeout: Duration,
    ) -> Result<crate::store::SessionRecord> {
        let deadline = Instant::now() + timeout;
        loop {
            let session = crate::store::get_session(conn, session_id)?
                .with_context(|| format!("session {session_id} disappeared during PTY test"))?;
            if session.status == expected {
                return Ok(session);
            }
            if Instant::now() >= deadline {
                anyhow::bail!(
                    "session {session_id} stayed {:?}; expected {expected:?}",
                    session.status
                );
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[cfg(windows)]
    fn wait_for_session_event(
        conn: &rusqlite::Connection,
        session_id: &str,
        kind: &str,
        timeout: Duration,
    ) -> Result<()> {
        let deadline = Instant::now() + timeout;
        loop {
            if crate::store::list_events(conn, session_id)?
                .iter()
                .any(|event| event.kind == kind)
            {
                return Ok(());
            }
            if Instant::now() >= deadline {
                anyhow::bail!("session {session_id} never recorded {kind:?} event");
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[cfg(windows)]
    fn wait_for_windows_process_exit(pid: u32, timeout: Duration) -> bool {
        use windows_sys::Win32::{
            Foundation::{CloseHandle, WAIT_OBJECT_0},
            System::Threading::{OpenProcess, WaitForSingleObject},
        };
        // SAFETY: the checked process handle is closed exactly once.
        unsafe {
            const SYNCHRONIZE_ACCESS: u32 = 0x0010_0000;
            let process = OpenProcess(SYNCHRONIZE_ACCESS, 0, pid);
            if process == 0 as _ {
                return true;
            }
            let milliseconds = timeout.as_millis().min(u32::MAX as u128) as u32;
            let result = WaitForSingleObject(process, milliseconds);
            CloseHandle(process);
            result == WAIT_OBJECT_0
        }
    }

    #[test]
    fn live_runtime_rejects_stream_launch_for_non_stream_capable_harness() {
        let runtime = LiveSessionRuntime::default();
        let launch = crate::api::SessionLaunch {
            id: "session-x".to_string(),
            project_root: "/tmp/x".to_string(),
            cwd: "/tmp/x".to_string(),
            harness: "codex".to_string(),
            model: None,
            launch_mode: crate::harness::HarnessLaunchMode::Stream,
            launch_policy: None,
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
    fn live_runtime_reaps_registered_session_on_observed_exit() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let conn = crate::store::open_store(&temp_dir.path().join("coven.sqlite3"))?;
        crate::store::insert_session(&conn, &session_record("completed-session"))?;
        drop(conn);

        let runtime = LiveSessionRuntime::with_coven_home(temp_dir.path().to_path_buf());
        let (observer, registration) =
            runtime.observer_for_session("completed-session".to_string());
        runtime.register_kind_with_registration(
            "completed-session".to_string(),
            LiveSessionKind::Pty,
            Box::new(SharedBuffer::default()),
            Box::new(RecordingKiller::default()),
            registration,
        )?;
        let (input, killer) = {
            let sessions = runtime.sessions.lock().unwrap();
            let handle = sessions.get("completed-session").unwrap();
            (
                Arc::downgrade(&handle.input),
                Arc::downgrade(&handle.killer),
            )
        };

        let pty_runner::DetachedPtyObserver { on_exit, .. } = observer;
        on_exit(pty_runner::PtyRunResult {
            status: "completed",
            exit_code: Some(0),
        });

        assert!(!runtime
            .sessions
            .lock()
            .unwrap()
            .contains_key("completed-session"));
        assert!(input.upgrade().is_none(), "child stdin handle was retained");
        assert!(
            killer.upgrade().is_none(),
            "child killer handle was retained"
        );
        let conn = crate::store::open_store(&temp_dir.path().join("coven.sqlite3"))?;
        assert_eq!(
            crate::store::get_session(&conn, "completed-session")?
                .unwrap()
                .status,
            "completed"
        );
        Ok(())
    }

    #[test]
    fn live_runtime_does_not_register_session_after_early_observed_exit() -> Result<()> {
        let runtime = LiveSessionRuntime::default();
        let (observer, registration) = runtime.observer_for_session("fast-session".to_string());
        let pty_runner::DetachedPtyObserver { on_exit, .. } = observer;
        on_exit(pty_runner::PtyRunResult {
            status: "completed",
            exit_code: Some(0),
        });

        runtime.register_kind_with_registration(
            "fast-session".to_string(),
            LiveSessionKind::Pty,
            Box::new(SharedBuffer::default()),
            Box::new(RecordingKiller::default()),
            registration,
        )?;

        assert!(!runtime
            .sessions
            .lock()
            .unwrap()
            .contains_key("fast-session"));
        Ok(())
    }

    #[test]
    fn live_runtime_stale_exit_observer_does_not_reap_replacement_registration() -> Result<()> {
        let runtime = LiveSessionRuntime::default();
        let (stale_observer, stale_registration) =
            runtime.observer_for_session("reused-session".to_string());
        runtime.register_kind_with_registration(
            "reused-session".to_string(),
            LiveSessionKind::Pty,
            Box::new(SharedBuffer::default()),
            Box::new(RecordingKiller::default()),
            stale_registration,
        )?;

        let (_replacement_observer, replacement_registration) =
            runtime.observer_for_session("reused-session".to_string());
        runtime.register_kind_with_registration(
            "reused-session".to_string(),
            LiveSessionKind::Pty,
            Box::new(SharedBuffer::default()),
            Box::new(RecordingKiller::default()),
            Arc::clone(&replacement_registration),
        )?;

        let pty_runner::DetachedPtyObserver { on_exit, .. } = stale_observer;
        on_exit(pty_runner::PtyRunResult {
            status: "completed",
            exit_code: Some(0),
        });

        let sessions = runtime.sessions.lock().unwrap();
        let handle = sessions.get("reused-session").unwrap();
        assert!(Arc::ptr_eq(&handle.registration, &replacement_registration));
        Ok(())
    }

    #[test]
    fn live_runtime_replacement_drops_old_handle_after_registry_unlock() -> Result<()> {
        let runtime = LiveSessionRuntime::default();
        let dropped_after_unlock = Arc::new(AtomicBool::new(false));
        runtime.register(
            "reused-session".to_string(),
            Box::new(SharedBuffer::default()),
            Box::new(RegistryLockCheckingKiller {
                sessions: Arc::downgrade(&runtime.sessions),
                dropped_after_unlock: Arc::clone(&dropped_after_unlock),
            }),
        )?;

        runtime.register(
            "reused-session".to_string(),
            Box::new(SharedBuffer::default()),
            Box::new(RecordingKiller::default()),
        )?;

        assert!(dropped_after_unlock.load(Ordering::Acquire));
        Ok(())
    }

    #[test]
    fn live_runtime_reaper_recovers_poisoned_registry() -> Result<()> {
        use std::panic::{catch_unwind, AssertUnwindSafe};

        let runtime = LiveSessionRuntime::default();
        let (observer, registration) = runtime.observer_for_session("poisoned-session".to_string());
        runtime.register_kind_with_registration(
            "poisoned-session".to_string(),
            LiveSessionKind::Pty,
            Box::new(SharedBuffer::default()),
            Box::new(RecordingKiller::default()),
            registration,
        )?;
        let input = {
            let sessions = runtime.sessions.lock().unwrap();
            Arc::downgrade(&sessions.get("poisoned-session").unwrap().input)
        };

        let sessions = Arc::clone(&runtime.sessions);
        assert!(catch_unwind(AssertUnwindSafe(|| {
            let _guard = sessions.lock().unwrap();
            panic!("poison live session registry");
        }))
        .is_err());

        let pty_runner::DetachedPtyObserver { on_exit, .. } = observer;
        on_exit(pty_runner::PtyRunResult {
            status: "completed",
            exit_code: Some(0),
        });

        assert!(input.upgrade().is_none(), "child stdin handle was retained");
        let sessions = match runtime.sessions.lock() {
            Ok(_) => panic!("live session registry unexpectedly lost its poison state"),
            Err(poisoned) => poisoned.into_inner(),
        };
        assert!(!sessions.contains_key("poisoned-session"));
        Ok(())
    }

    #[test]
    fn live_runtime_reaper_reports_poison_after_registry_unlock() -> Result<()> {
        use std::panic::{catch_unwind, AssertUnwindSafe};

        let runtime = LiveSessionRuntime::default();
        let registration = Arc::new(LiveSessionRegistration::new(None));
        runtime.register_kind_with_registration(
            "poison-report-session".to_string(),
            LiveSessionKind::Pty,
            Box::new(SharedBuffer::default()),
            Box::new(RecordingKiller::default()),
            Arc::clone(&registration),
        )?;
        let cleanup = LiveSessionExitCleanup {
            session_id: "poison-report-session".to_string(),
            sessions: Arc::downgrade(&runtime.sessions),
            registration,
        };

        let sessions = Arc::clone(&runtime.sessions);
        assert!(catch_unwind(AssertUnwindSafe(|| {
            let _guard = sessions.lock().unwrap();
            panic!("poison live session registry");
        }))
        .is_err());

        let reported_after_unlock = Arc::new(AtomicBool::new(false));
        let reported = Arc::clone(&reported_after_unlock);
        cleanup.mark_exited_with_poison_reporter(|| {
            let registry_is_unlocked = match sessions.try_lock() {
                Ok(_) | Err(std::sync::TryLockError::Poisoned(_)) => true,
                Err(std::sync::TryLockError::WouldBlock) => false,
            };
            reported.store(registry_is_unlocked, Ordering::Release);
        });

        assert!(
            reported_after_unlock.load(Ordering::Acquire),
            "poison diagnostic ran while the live session registry was locked"
        );
        Ok(())
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

    #[test]
    fn runtime_shutdown_kills_all_handles_and_refuses_new_registrations() -> Result<()> {
        let runtime = LiveSessionRuntime::default();
        let first = Arc::new(Mutex::new(false));
        let second = Arc::new(Mutex::new(false));
        for (id, killed) in [
            ("first", Arc::clone(&first)),
            ("second", Arc::clone(&second)),
        ] {
            runtime.register(
                id.to_string(),
                Box::new(SharedBuffer::default()),
                Box::new(RecordingKiller { killed }),
            )?;
        }

        runtime.shutdown_all()?;

        assert!(*first.lock().unwrap());
        assert!(*second.lock().unwrap());
        let late_killed = Arc::new(Mutex::new(false));
        let late = runtime
            .register(
                "late".to_string(),
                Box::new(SharedBuffer::default()),
                Box::new(RecordingKiller {
                    killed: Arc::clone(&late_killed),
                }),
            )
            .expect_err("shutdown must close admission");
        assert!(late.to_string().contains("shutting down"), "{late:#}");
        assert!(
            *late_killed.lock().unwrap(),
            "pre-lock shutdown rejection must explicitly invoke the supplied PTY killer"
        );
        Ok(())
    }

    #[test]
    fn post_lock_shutdown_rejection_kills_without_holding_registry_lock() -> Result<()> {
        let runtime = LiveSessionRuntime::default();
        runtime.shutting_down.store(true, Ordering::Release);
        let killed = Arc::new(Mutex::new(false));
        let registry_was_unlocked = Arc::new(Mutex::new(false));
        let error = runtime
            .register_kind_after_initial_shutdown_check(
                "late-after-check".to_string(),
                LiveSessionKind::Pty,
                Box::new(SharedBuffer::default()),
                Box::new(RegistryObservingKiller {
                    sessions: Arc::downgrade(&runtime.sessions),
                    killed: Arc::clone(&killed),
                    registry_was_unlocked: Arc::clone(&registry_was_unlocked),
                    fail_message: None,
                }),
                Arc::new(LiveSessionRegistration::new(None)),
            )
            .expect_err("shutdown observed under the registry lock must reject admission");

        assert!(error.to_string().contains("shutting down"), "{error:#}");
        assert!(*killed.lock().unwrap());
        assert!(
            *registry_was_unlocked.lock().unwrap(),
            "late-registration cleanup ran while holding the registry lock"
        );
        assert!(runtime.sessions.lock().unwrap().is_empty());
        Ok(())
    }

    #[test]
    fn late_registration_reports_shutdown_and_cleanup_failure() -> Result<()> {
        let runtime = LiveSessionRuntime::default();
        runtime.shutting_down.store(true, Ordering::Release);
        let error = runtime
            .register(
                "late-cleanup-failure".to_string(),
                Box::new(SharedBuffer::default()),
                Box::new(FailingKiller("synthetic cleanup failure")),
            )
            .expect_err("late registration must fail closed when cleanup also fails");
        let diagnostic = format!("{error:#}");
        assert!(diagnostic.contains("shutting down"), "{diagnostic}");
        assert!(
            diagnostic.contains("failed to terminate the rejected session process tree"),
            "{diagnostic}"
        );
        assert!(
            diagnostic.contains("synthetic cleanup failure"),
            "{diagnostic}"
        );
        Ok(())
    }

    #[cfg(unix)]
    fn daemon_shutdown_descendant_fixture(
        build_dir: &Path,
        pid_file: &Path,
    ) -> pty_runner::HarnessCommand {
        pty_runner::HarnessCommand::fixture(
            "/bin/sh",
            vec![
                "-c".to_string(),
                "sleep 120 </dev/null >/dev/null 2>&1 & echo $! > \"$1\"; wait".to_string(),
                "daemon-shutdown-descendant".to_string(),
                pid_file.to_string_lossy().into_owned(),
            ],
            build_dir.to_path_buf(),
        )
    }

    #[cfg(windows)]
    fn daemon_shutdown_descendant_fixture(
        build_dir: &Path,
        pid_file: &Path,
    ) -> pty_runner::HarnessCommand {
        let probe = pty_runner::windows_console_probe_command(build_dir)
            .expect("compile native Windows process probe");
        pty_runner::HarnessCommand::fixture(
            probe.program().to_string(),
            vec![
                "--spawn-descendant".to_string(),
                pid_file.to_string_lossy().into_owned(),
            ],
            probe.cwd().to_path_buf(),
        )
    }

    #[cfg(any(unix, windows))]
    fn await_daemon_shutdown_descendant_pid(pid_file: &Path) -> Result<u32> {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Ok(raw) = std::fs::read_to_string(pid_file) {
                if let Ok(pid) = raw.trim().parse::<u32>() {
                    return Ok(pid);
                }
            }
            anyhow::ensure!(
                Instant::now() < deadline,
                "piped fixture did not publish its descendant pid"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[cfg(unix)]
    fn await_daemon_shutdown_descendant_exit(pid: u32, context: &str) -> Result<()> {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let result = unsafe { libc::kill(pid as libc::pid_t, 0) };
            if result == -1 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
                return Ok(());
            }
            anyhow::ensure!(
                Instant::now() < deadline,
                "{context} left descendant {pid} running"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[cfg(windows)]
    fn await_daemon_shutdown_descendant_exit(pid: u32, context: &str) -> Result<()> {
        anyhow::ensure!(
            wait_for_windows_process_exit(pid, Duration::from_secs(10)),
            "{context} left descendant {pid} running"
        );
        Ok(())
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn dropping_runtime_terminates_registered_piped_process_tree() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let pid_file = temp_dir.path().join("descendant.pid");
        let command = daemon_shutdown_descendant_fixture(temp_dir.path(), &pid_file);
        let (exit_tx, exit_rx) = std::sync::mpsc::channel();
        let observer = pty_runner::DetachedPtyObserver {
            on_output: Box::new(|_| {}),
            on_exit: Box::new(move |result| {
                let _ = exit_tx.send(result);
            }),
        };
        let piped = pty_runner::spawn_piped_with_observer(&command, Some(observer), false)?;
        let descendant_pid = await_daemon_shutdown_descendant_pid(&pid_file)?;
        let runtime = LiveSessionRuntime::default();
        piped.activate(|input, process_tree| {
            runtime.register("piped".to_string(), input, Box::new(process_tree))
        })?;

        drop(runtime);
        let result = exit_rx
            .try_recv()
            .context("runtime drop returned before the piped exit callback completed")?;
        assert_eq!(result.status, "failed", "{result:?}");
        await_daemon_shutdown_descendant_exit(descendant_pid, "runtime drop")?;
        Ok(())
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn shutdown_admission_failure_drops_already_spawned_piped_process_tree() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let pid_file = temp_dir.path().join("descendant.pid");
        let mut command = daemon_shutdown_descendant_fixture(temp_dir.path(), &pid_file);
        command.set_stdin_prompt_for_test(vec![b'x'; 1024 * 1024]);
        let (exit_tx, exit_rx) = std::sync::mpsc::channel();
        let observer = pty_runner::DetachedPtyObserver {
            on_output: Box::new(|_| {}),
            on_exit: Box::new(move |result| {
                let _ = exit_tx.send(result);
            }),
        };
        let piped = pty_runner::spawn_piped_with_observer(&command, Some(observer), false)?;
        let descendant_pid = await_daemon_shutdown_descendant_pid(&pid_file)?;
        let runtime = LiveSessionRuntime::default();
        runtime.shutdown_all()?;

        let error = piped
            .activate(|input, process_tree| {
                runtime.register("late-piped".to_string(), input, Box::new(process_tree))
            })
            .expect_err("shutdown admission must reject a late spawned tree");
        assert!(error.to_string().contains("shutting down"), "{error:#}");
        let result = exit_rx.recv_timeout(Duration::from_secs(10))?;
        assert_eq!(result.status, "failed", "{result:?}");
        await_daemon_shutdown_descendant_exit(descendant_pid, "rejected registration")?;
        Ok(())
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn runtime_kill_interrupts_in_flight_piped_prompt_within_stop_budget() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let pid_file = temp_dir.path().join("never-read.pid");
        let command = pty_runner::piped_prompt_probe_command(
            temp_dir.path(),
            "never-read",
            &pid_file.to_string_lossy(),
            None,
            vec![b'x'; 1024 * 1024],
        )?;
        let (exit_tx, exit_rx) = std::sync::mpsc::channel();
        let observer = pty_runner::DetachedPtyObserver {
            on_output: Box::new(|_| {}),
            on_exit: Box::new(move |result| {
                let _ = exit_tx.send(result);
            }),
        };
        let piped = pty_runner::spawn_piped_with_observer(&command, Some(observer), false)?;
        let child_pid = await_daemon_shutdown_descendant_pid(&pid_file)?;
        let runtime = Arc::new(LiveSessionRuntime::default());
        let activation_runtime = Arc::clone(&runtime);
        let (activation_tx, activation_rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let result = piped.activate(|input, process_tree| {
                activation_runtime.register(
                    "blocked-prompt".to_string(),
                    input,
                    Box::new(process_tree),
                )
            });
            let _ = activation_tx.send(result);
        });
        let registration_deadline = Instant::now() + Duration::from_secs(2);
        while !runtime
            .sessions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .contains_key("blocked-prompt")
        {
            anyhow::ensure!(
                Instant::now() < registration_deadline,
                "piped prompt was not registered before its writer blocked"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        std::thread::sleep(Duration::from_millis(50));

        let started = Instant::now();
        SessionRuntime::kill_session(runtime.as_ref(), "blocked-prompt")?;
        assert!(started.elapsed() < Duration::from_secs(2));
        let activation = activation_rx.recv_timeout(Duration::from_secs(2))?;
        let error = activation.expect_err("cancellation must interrupt prompt delivery");
        let diagnostic = format!("{error:#}");
        assert!(
            diagnostic.contains("terminated") || diagnostic.contains("failed writing"),
            "{diagnostic}"
        );
        let exit = exit_rx.recv_timeout(Duration::from_secs(2))?;
        assert_eq!(exit.status, "failed", "{exit:?}");
        await_daemon_shutdown_descendant_exit(child_pid, "prompt cancellation")?;
        Ok(())
    }

    #[cfg(any(unix, windows))]
    fn assert_adopted_publication_precedes_blocked_delivery(stream: bool) -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let session_id = if stream {
            "adopted-blocked-stream"
        } else {
            "adopted-blocked-piped"
        };
        let mut row = session_record(session_id);
        row.status = "created".to_string();
        let conn = crate::store::open_store(&temp_dir.path().join(crate::STORE_FILE_NAME))?;
        crate::store::insert_session(&conn, &row)?;
        drop(conn);

        let pid_file = temp_dir.path().join(if stream {
            "blocked-stream.pid"
        } else {
            "blocked-piped.pid"
        });
        let command = if stream {
            daemon_shutdown_descendant_fixture(temp_dir.path(), &pid_file)
        } else {
            pty_runner::piped_prompt_probe_command(
                temp_dir.path(),
                "never-read",
                &pid_file.to_string_lossy(),
                None,
                vec![b'x'; 4 * 1024 * 1024],
            )?
        };
        let launch = crate::api::SessionLaunch {
            id: session_id.to_string(),
            project_root: temp_dir.path().to_string_lossy().into_owned(),
            cwd: temp_dir.path().to_string_lossy().into_owned(),
            harness: if stream { "claude" } else { "codex" }.to_string(),
            model: None,
            launch_mode: if stream {
                crate::harness::HarnessLaunchMode::Stream
            } else {
                crate::harness::HarnessLaunchMode::NonInteractive
            },
            launch_policy: None,
            prompt: if stream {
                "x".repeat(4 * 1024 * 1024)
            } else {
                "piped prompt is carried by the prepared command".to_string()
            },
            title: "blocked adopted ownership publication".to_string(),
            conversation: None,
            conversation_id: None,
            familiar_id: None,
            caller_familiar_id: None,
        };
        let runtime = Arc::new(LiveSessionRuntime::with_coven_home(
            temp_dir.path().to_path_buf(),
        ));
        let callbacks = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let callback_count = Arc::clone(&callbacks);
        let callback_home = temp_dir.path().to_path_buf();
        let callback_id = session_id.to_string();
        let (published_tx, published_rx) = std::sync::mpsc::channel();
        let launch_runtime = Arc::clone(&runtime);
        let (launch_tx, launch_rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut publish_running = || -> Result<()> {
                callback_count.fetch_add(1, Ordering::SeqCst);
                let conn = crate::store::open_store(&callback_home.join(crate::STORE_FILE_NAME))?;
                let changed = crate::store::update_session_status_if_current(
                    &conn,
                    &callback_id,
                    "created",
                    "running",
                    None,
                    &chrono::Utc::now().to_rfc3339(),
                )?;
                anyhow::ensure!(changed, "created row did not publish running");
                let _ = published_tx.send(());
                Ok(())
            };
            let result = launch_runtime.launch_prepared_session(
                &launch,
                None,
                command,
                Some(&mut publish_running),
            );
            let _ = launch_tx.send(result);
        });

        let child_pid = await_daemon_shutdown_descendant_pid(&pid_file)?;
        published_rx.recv_timeout(Duration::from_secs(2))?;
        let conn = crate::store::open_store(&temp_dir.path().join(crate::STORE_FILE_NAME))?;
        assert_eq!(
            crate::store::get_session(&conn, session_id)?
                .context("blocked adopted session")?
                .status,
            "running"
        );
        assert_eq!(callbacks.load(Ordering::SeqCst), 1);
        std::thread::sleep(Duration::from_millis(100));
        assert!(matches!(
            launch_rx.try_recv(),
            Err(std::sync::mpsc::TryRecvError::Empty)
        ));

        let kill = crate::api::handle_request_with_runtime(
            "POST",
            &format!("/sessions/{session_id}/kill"),
            temp_dir.path(),
            None,
            None,
            runtime.as_ref(),
        )?;
        assert_eq!(kill.status, 202, "{}", kill.body);
        let launch_error = launch_rx
            .recv_timeout(Duration::from_secs(2))?
            .expect_err("kill must interrupt blocked initial delivery");
        assert!(
            !launch_error.to_string().is_empty(),
            "blocked launch returned an empty cancellation error"
        );
        assert_eq!(callbacks.load(Ordering::SeqCst), 1);
        assert_eq!(
            crate::store::get_session(&conn, session_id)?
                .context("killed adopted session")?
                .status,
            "killed"
        );
        await_daemon_shutdown_descendant_exit(
            child_pid,
            if stream {
                "blocked adopted stream cancellation"
            } else {
                "blocked adopted piped cancellation"
            },
        )?;
        Ok(())
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn adopted_stream_registration_publishes_running_before_blocked_prompt_and_is_killable(
    ) -> Result<()> {
        assert_adopted_publication_precedes_blocked_delivery(true)
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn adopted_piped_registration_publishes_running_before_blocked_prompt_and_is_killable(
    ) -> Result<()> {
        assert_adopted_publication_precedes_blocked_delivery(false)
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn concurrent_api_kill_during_prompt_delivery_preserves_killed_status() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let project_root = temp_dir.path().join("project");
        std::fs::create_dir_all(&project_root)?;
        let pid_file = temp_dir.path().join("api-cancel-never-read.pid");
        let command = pty_runner::piped_prompt_probe_command(
            temp_dir.path(),
            "never-read",
            &pid_file.to_string_lossy(),
            None,
            vec![b'x'; 1024 * 1024],
        )?;
        let (launched_tx, launched_rx) = std::sync::mpsc::channel();
        let runtime = Arc::new(PromptCancellationApiRuntime {
            inner: LiveSessionRuntime::with_coven_home(temp_dir.path().to_path_buf()),
            command: Mutex::new(Some(command)),
            launched: Mutex::new(Some(launched_tx)),
            await_root_exit_before_activate: None,
        });
        let body = serde_json::json!({
            "projectRoot": project_root,
            "harness": "codex",
            "launchMode": "nonInteractive",
            "prompt": "cancel this blocked delivery"
        })
        .to_string();
        let launch_runtime = Arc::clone(&runtime);
        let launch_home = temp_dir.path().to_path_buf();
        let (response_tx, response_rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let response = crate::api::handle_request_with_runtime(
                "POST",
                "/sessions",
                &launch_home,
                None,
                Some(&body),
                launch_runtime.as_ref(),
            );
            let _ = response_tx.send(response);
        });

        let session_id = launched_rx.recv_timeout(Duration::from_secs(2))?;
        let child_pid = await_daemon_shutdown_descendant_pid(&pid_file)?;
        let registration_deadline = Instant::now() + Duration::from_secs(2);
        while !runtime
            .inner
            .sessions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .contains_key(&session_id)
        {
            anyhow::ensure!(
                Instant::now() < registration_deadline,
                "API launch did not register before prompt delivery blocked"
            );
            std::thread::sleep(Duration::from_millis(5));
        }

        let kill = crate::api::handle_request_with_runtime(
            "POST",
            &format!("/sessions/{session_id}/kill"),
            temp_dir.path(),
            None,
            None,
            runtime.as_ref(),
        )?;
        assert_eq!(kill.status, 202, "{}", kill.body);
        let launch = response_rx.recv_timeout(Duration::from_secs(2))??;
        assert_eq!(launch.status, 500, "{}", launch.body);
        let conn = crate::store::open_store(&temp_dir.path().join(crate::STORE_FILE_NAME))?;
        let row = crate::store::get_session(&conn, &session_id)?
            .context("cancelled API launch row remains present")?;
        assert_eq!(row.status, "killed");
        await_daemon_shutdown_descendant_exit(child_pid, "concurrent API cancellation")?;
        Ok(())
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn api_prompt_delivery_failure_wins_over_successful_root_exit() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let project_root = temp_dir.path().join("project");
        std::fs::create_dir_all(&project_root)?;
        let pid_file = temp_dir.path().join("api-exit-zero-closed-stdin.pid");
        let command = pty_runner::piped_prompt_probe_command(
            temp_dir.path(),
            "exit-zero-close-stdin",
            &pid_file.to_string_lossy(),
            None,
            vec![b'x'; 1024 * 1024],
        )?;
        let (launched_tx, launched_rx) = std::sync::mpsc::channel();
        let runtime = PromptCancellationApiRuntime {
            inner: LiveSessionRuntime::with_coven_home(temp_dir.path().to_path_buf()),
            command: Mutex::new(Some(command)),
            launched: Mutex::new(Some(launched_tx)),
            await_root_exit_before_activate: Some(pid_file.clone()),
        };
        let body = serde_json::json!({
            "projectRoot": project_root,
            "harness": "codex",
            "launchMode": "nonInteractive",
            "prompt": "this prompt is replaced by the pipe-capacity fixture payload"
        })
        .to_string();

        let response = crate::api::handle_request_with_runtime(
            "POST",
            "/sessions",
            temp_dir.path(),
            None,
            Some(&body),
            &runtime,
        )?;
        assert_eq!(response.status, 500, "{}", response.body);
        assert!(response.body.contains("launch_failed"), "{}", response.body);
        let session_id = launched_rx.recv_timeout(Duration::from_secs(2))?;
        let conn = crate::store::open_store(&temp_dir.path().join(crate::STORE_FILE_NAME))?;
        let deadline = Instant::now() + Duration::from_secs(3);
        let (row, exit_payload) = loop {
            let row = crate::store::get_session(&conn, &session_id)?
                .context("failed API launch row remains present")?;
            let exit_payload = crate::store::list_events(&conn, &session_id)?
                .into_iter()
                .find(|event| event.kind == "exit")
                .map(|event| event.payload_json);
            if let Some(exit_payload) = exit_payload {
                break (row, exit_payload);
            }
            anyhow::ensure!(
                Instant::now() < deadline,
                "failed API launch never persisted its exit event"
            );
            std::thread::sleep(Duration::from_millis(20));
        };

        assert_eq!(row.status, "failed", "{row:?}");
        let exit_payload: Value = serde_json::from_str(&exit_payload)?;
        assert_eq!(exit_payload["status"], "failed", "{exit_payload}");
        assert_eq!(exit_payload["exitCode"], 0, "{exit_payload}");
        Ok(())
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn piped_kill_returns_only_after_descendant_output_is_quiescent() -> Result<()> {
        use std::sync::atomic::AtomicUsize;

        let temp_dir = tempfile::tempdir()?;
        let pid_file = temp_dir.path().join("output-descendant.pid");
        let command = pty_runner::piped_prompt_probe_command(
            temp_dir.path(),
            "descendant-output",
            &pid_file.to_string_lossy(),
            None,
            Vec::new(),
        )?;
        let observed = Arc::new(AtomicUsize::new(0));
        let observer_count = Arc::clone(&observed);
        let observer = pty_runner::DetachedPtyObserver {
            on_output: Box::new(move |chunk| {
                observer_count.fetch_add(chunk.len(), Ordering::AcqRel);
            }),
            on_exit: Box::new(|_| {}),
        };
        let piped = pty_runner::spawn_piped_with_observer(&command, Some(observer), false)?;
        let descendant_pid = await_daemon_shutdown_descendant_pid(&pid_file)?;
        let runtime = LiveSessionRuntime::default();
        piped.activate(|input, process_tree| {
            runtime.register(
                "output-descendant".to_string(),
                input,
                Box::new(process_tree),
            )
        })?;

        let output_deadline = Instant::now() + Duration::from_secs(2);
        while observed.load(Ordering::Acquire) == 0 {
            anyhow::ensure!(
                Instant::now() < output_deadline,
                "output descendant did not emit its readiness output"
            );
            std::thread::sleep(Duration::from_millis(5));
        }

        let started = Instant::now();
        SessionRuntime::kill_session(&runtime, "output-descendant")?;
        assert!(started.elapsed() < Duration::from_secs(2));
        let count_at_return = observed.load(Ordering::Acquire);
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(
            observed.load(Ordering::Acquire),
            count_at_return,
            "descendant output was observed after cancellation returned"
        );
        await_daemon_shutdown_descendant_exit(descendant_pid, "quiescent cancellation")?;

        let already_gone = SessionRuntime::kill_session(&runtime, "output-descendant")
            .expect_err("a completed cancellation must be idempotently not-live");
        assert!(already_gone.downcast_ref::<NotLiveError>().is_some());
        Ok(())
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn shutdown_bounds_pre_registration_launch_and_kills_its_exact_tree() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let pid_file = temp_dir.path().join("pre-registration.pid");
        let command = pty_runner::piped_prompt_probe_command(
            temp_dir.path(),
            "never-read",
            &pid_file.to_string_lossy(),
            None,
            vec![b'x'; 1024 * 1024],
        )?;
        let (exit_tx, exit_rx) = std::sync::mpsc::channel();
        let observer = pty_runner::DetachedPtyObserver {
            on_output: Box::new(|_| {}),
            on_exit: Box::new(move |result| {
                let _ = exit_tx.send(result);
            }),
        };
        let runtime = Arc::new(LiveSessionRuntime::default());
        let admission = runtime.begin_launch()?;
        let (piped, _provisional_killer) = admission.spawn_owned(|publish| {
            let piped = pty_runner::spawn_piped_with_observer(&command, Some(observer), false)?;
            let killer: Box<dyn RuntimeKiller> = Box::new(piped.cancellation_handle());
            publish(killer)?;
            Ok(piped)
        })?;
        let child_pid = await_daemon_shutdown_descendant_pid(&pid_file)?;
        let activation_runtime = Arc::clone(&runtime);
        let (at_registration_tx, at_registration_rx) = std::sync::mpsc::channel();
        let (continue_tx, continue_rx) = std::sync::mpsc::channel();
        let (activation_tx, activation_rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let result = piped.activate(|input, process_tree| {
                let _ = at_registration_tx.send(());
                let _ = continue_rx.recv();
                let registered = activation_runtime.register(
                    "pre-registration".to_string(),
                    input,
                    Box::new(process_tree),
                );
                admission.release();
                registered
            });
            let _ = activation_tx.send(result);
        });
        at_registration_rx.recv_timeout(Duration::from_secs(2))?;

        let shutdown_runtime = Arc::clone(&runtime);
        let (shutdown_tx, shutdown_rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = shutdown_tx.send(shutdown_runtime.shutdown_all());
        });
        shutdown_rx.recv_timeout(Duration::from_secs(2))??;
        await_daemon_shutdown_descendant_exit(child_pid, "bounded pre-registration shutdown")?;
        continue_tx.send(())?;
        let activation = activation_rx.recv_timeout(Duration::from_secs(2))?;
        let error = activation.expect_err("closed admission must reject the pending launch");
        assert!(error.to_string().contains("shutting down"), "{error:#}");
        let exit = exit_rx.recv_timeout(Duration::from_secs(2))?;
        assert_eq!(exit.status, "failed", "{exit:?}");
        Ok(())
    }

    #[test]
    fn shutdown_budget_does_not_wait_for_spawn_closure_after_ownership_publication() -> Result<()> {
        let runtime = Arc::new(LiveSessionRuntime::default());
        let admission = runtime.begin_launch()?;
        let killed = Arc::new(Mutex::new(false));
        let (published_tx, published_rx) = std::sync::mpsc::channel();
        let (continue_tx, continue_rx) = std::sync::mpsc::channel();
        let killed_in_spawn = Arc::clone(&killed);
        let spawn_thread = std::thread::spawn(move || {
            admission.spawn_owned(|publish| {
                publish(Box::new(RecordingKiller {
                    killed: killed_in_spawn,
                }))?;
                let _ = published_tx.send(());
                let _ = continue_rx.recv();
                Ok(())
            })
        });
        published_rx.recv_timeout(Duration::from_secs(1))?;

        let started = Instant::now();
        runtime.shutdown_all()?;
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "shutdown exceeded its external stop budget"
        );
        assert!(*killed.lock().unwrap());
        continue_tx.send(())?;
        let _ = spawn_thread.join().expect("spawn closure thread")?;
        Ok(())
    }

    #[test]
    fn shutdown_budget_closes_gate_while_spawn_closure_is_stalled_before_publication() -> Result<()>
    {
        let runtime = Arc::new(LiveSessionRuntime::default());
        let admission = runtime.begin_launch()?;
        let killed = Arc::new(Mutex::new(false));
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (continue_tx, continue_rx) = std::sync::mpsc::channel();
        let killed_in_spawn = Arc::clone(&killed);
        let spawn_thread = std::thread::spawn(move || {
            admission.spawn_owned(|publish| {
                let _ = entered_tx.send(());
                let _ = continue_rx.recv();
                publish(Box::new(RecordingKiller {
                    killed: killed_in_spawn,
                }))?;
                Ok(())
            })
        });
        entered_rx.recv_timeout(Duration::from_secs(1))?;

        let started = Instant::now();
        runtime.shutdown_all()?;
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "shutdown waited on a pre-publication spawn closure"
        );
        assert!(!*killed.lock().unwrap());

        continue_tx.send(())?;
        let error = match spawn_thread.join().expect("spawn closure thread") {
            Ok(_) => anyhow::bail!("a closed launch gate accepted late ownership publication"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("shutting down"), "{error:#}");
        assert!(
            *killed.lock().unwrap(),
            "late publication did not invoke the exact supplied killer"
        );
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

    struct RegistryObservingKiller {
        sessions: Weak<Mutex<HashMap<String, LiveSessionHandle>>>,
        killed: Arc<Mutex<bool>>,
        registry_was_unlocked: Arc<Mutex<bool>>,
        fail_message: Option<&'static str>,
    }

    impl RuntimeKiller for RegistryObservingKiller {
        fn kill(&mut self) -> Result<()> {
            *self.killed.lock().unwrap() = true;
            let unlocked = self
                .sessions
                .upgrade()
                .is_some_and(|sessions| sessions.try_lock().is_ok());
            *self.registry_was_unlocked.lock().unwrap() = unlocked;
            if let Some(message) = self.fail_message {
                anyhow::bail!(message);
            }
            Ok(())
        }
    }

    struct FailingKiller(&'static str);

    impl RuntimeKiller for FailingKiller {
        fn kill(&mut self) -> Result<()> {
            anyhow::bail!(self.0)
        }
    }

    struct FailingWriter(&'static str);

    impl Write for FailingWriter {
        fn write(&mut self, _buf: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::new(std::io::ErrorKind::BrokenPipe, self.0))
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn stream_launch_fixture(id: &str, prompt: &str) -> crate::api::SessionLaunch {
        crate::api::SessionLaunch {
            id: id.to_string(),
            project_root: "/private-project-fixture".to_string(),
            cwd: "/private-project-fixture".to_string(),
            harness: "private-harness-fixture".to_string(),
            model: None,
            launch_mode: crate::harness::HarnessLaunchMode::Stream,
            launch_policy: None,
            prompt: prompt.to_string(),
            title: "private-title-fixture".to_string(),
            conversation: None,
            conversation_id: None,
            familiar_id: None,
            caller_familiar_id: None,
        }
    }

    #[test]
    fn failed_initial_stream_input_with_failed_cleanup_retains_typed_ownership() -> Result<()> {
        let runtime = LiveSessionRuntime::default();
        let session_id = "private-retained-session-fixture";
        let prompt = "private-initial-input-fixture";
        let input_error = "private-input-error-fixture";
        let cleanup_error = "private-cleanup-error-fixture";
        runtime.register_kind(
            session_id.to_string(),
            LiveSessionKind::Stream,
            Box::new(FailingWriter(input_error)),
            Box::new(FailingKiller(cleanup_error)),
        )?;

        let error = runtime
            .deliver_initial_stream_prompt(&stream_launch_fixture(session_id, prompt))
            .expect_err("failed cleanup must report retained runtime ownership");

        assert!(
            error
                .downcast_ref::<RuntimeOwnershipRetainedError>()
                .is_some(),
            "returned anyhow error lost its retained-ownership disposition: {error:#}"
        );
        assert!(
            runtime.sessions.lock().unwrap().contains_key(session_id),
            "failed cleanup discarded the live runtime handle"
        );
        let diagnostic = format!("{error:#}");
        for private in [
            session_id,
            prompt,
            input_error,
            cleanup_error,
            "private-harness-fixture",
            "private-project-fixture",
        ] {
            assert!(
                !diagnostic.contains(private),
                "retained-ownership error leaked private fixture data"
            );
        }
        Ok(())
    }

    #[test]
    fn failed_initial_stream_input_with_successful_cleanup_is_definitive() -> Result<()> {
        let runtime = LiveSessionRuntime::default();
        let session_id = "definitively-cleaned-session";
        let killer = RecordingKiller::default();
        let killed = Arc::clone(&killer.killed);
        runtime.register_kind(
            session_id.to_string(),
            LiveSessionKind::Stream,
            Box::new(FailingWriter("initial input failed")),
            Box::new(killer),
        )?;

        let error = runtime
            .deliver_initial_stream_prompt(&stream_launch_fixture(session_id, "initial input"))
            .expect_err("initial input failure must be returned after cleanup");

        assert!(
            error
                .downcast_ref::<RuntimeOwnershipRetainedError>()
                .is_none(),
            "confirmed cleanup must remain an ordinary launch error"
        );
        assert!(*killed.lock().unwrap(), "cleanup kill was not invoked");
        assert!(
            !runtime.sessions.lock().unwrap().contains_key(session_id),
            "confirmed cleanup left a live registry handle"
        );
        Ok(())
    }

    #[test]
    fn ordinary_piped_error_stays_ordinary_while_registry_callback_is_delayed() -> Result<()> {
        let runtime = LiveSessionRuntime::default();
        let session_id = "delayed-registry-cleanup";
        runtime.register(
            session_id.to_string(),
            Box::new(std::io::sink()),
            Box::new(RecordingKiller::default()),
        )?;

        let error = runtime
            .classify_piped_activation_result(Err(anyhow::anyhow!(
                "ordinary definitive prompt failure"
            )))
            .expect_err("ordinary prompt failure must survive classification");

        assert!(
            error
                .downcast_ref::<RuntimeOwnershipRetainedError>()
                .is_none(),
            "registry timing overrode the authoritative cleanup disposition: {error:#}"
        );
        assert_eq!(error.to_string(), "ordinary definitive prompt failure");
        assert!(
            runtime.sessions.lock().unwrap().contains_key(session_id),
            "fixture must model an exit callback delayed before registry removal"
        );
        Ok(())
    }

    #[test]
    fn typed_piped_cleanup_disposition_translates_exactly_and_privately() {
        let runtime = LiveSessionRuntime::default();

        let error = runtime
            .classify_piped_activation_result(Err(anyhow::Error::new(
                pty_runner::PipedLaunchCleanupRetainedError,
            )))
            .expect_err("typed piped cleanup failure must translate");

        assert!(
            error
                .downcast_ref::<RuntimeOwnershipRetainedError>()
                .is_some(),
            "typed pty cleanup disposition did not translate exactly: {error:#}"
        );
        assert_eq!(
            error.to_string(),
            "runtime ownership may remain after launch cleanup"
        );
    }

    #[test]
    fn failed_piped_activation_after_definitive_cleanup_is_ordinary() -> Result<()> {
        let runtime = LiveSessionRuntime::default();
        let session_id = "definitively-cleaned-piped-session";
        let killer = RecordingKiller::default();
        let killed = Arc::clone(&killer.killed);
        runtime.register(
            session_id.to_string(),
            Box::new(std::io::sink()),
            Box::new(killer),
        )?;
        SessionRuntime::kill_session(&runtime, session_id)?;
        let error = runtime
            .classify_piped_activation_result(Err(anyhow::anyhow!("piped prompt delivery failed")))
            .expect_err("the prompt failure must survive definitive cleanup");

        assert!(
            error
                .downcast_ref::<RuntimeOwnershipRetainedError>()
                .is_none(),
            "confirmed piped cleanup must remain an ordinary launch error"
        );
        assert_eq!(error.to_string(), "piped prompt delivery failed");
        assert!(*killed.lock().unwrap(), "cleanup kill was not invoked");
        assert!(
            !runtime.sessions.lock().unwrap().contains_key(session_id),
            "confirmed piped cleanup left a live registry handle"
        );
        Ok(())
    }

    struct CountingFailingKiller {
        attempts: Arc<std::sync::atomic::AtomicUsize>,
    }

    impl RuntimeKiller for CountingFailingKiller {
        fn kill(&mut self) -> Result<()> {
            self.attempts.fetch_add(1, Ordering::AcqRel);
            anyhow::bail!("quiescence proof timed out")
        }
    }

    #[cfg(any(unix, windows))]
    struct PromptCancellationApiRuntime {
        inner: LiveSessionRuntime,
        command: Mutex<Option<pty_runner::HarnessCommand>>,
        launched: Mutex<Option<std::sync::mpsc::Sender<String>>>,
        await_root_exit_before_activate: Option<PathBuf>,
    }

    #[cfg(any(unix, windows))]
    impl SessionRuntime for PromptCancellationApiRuntime {
        fn launch_session(&self, launch: &crate::api::SessionLaunch) -> Result<()> {
            if let Some(sender) = self.launched.lock().unwrap().take() {
                let _ = sender.send(launch.id.clone());
            }
            let command = self
                .command
                .lock()
                .unwrap()
                .take()
                .context("prompt-cancellation fixture command was already consumed")?;
            let (observer, registration) = self.inner.observer_for_session(launch.id.clone());
            let piped = pty_runner::spawn_piped_with_observer(&command, Some(observer), false)?;
            if let Some(pid_file) = &self.await_root_exit_before_activate {
                let pid = await_daemon_shutdown_descendant_pid(pid_file)?;
                await_daemon_shutdown_descendant_exit(pid, "successful pre-delivery root exit")?;
            }
            piped.activate(|input, process_tree| {
                self.inner.register_kind_with_registration(
                    launch.id.clone(),
                    LiveSessionKind::Pty,
                    input,
                    Box::new(process_tree),
                    registration,
                )
            })
        }

        fn send_input(&self, session_id: &str, payload: &Value) -> Result<()> {
            SessionRuntime::send_input(&self.inner, session_id, payload)
        }

        fn kill_session(&self, session_id: &str) -> Result<()> {
            SessionRuntime::kill_session(&self.inner, session_id)
        }

        fn with_session_event_boundary(
            &self,
            session_id: &str,
            kind: &str,
            payload: &Value,
            request_adoption_id: Option<&str>,
            action: &mut dyn FnMut() -> crate::api::SessionEventBoundaryResult,
        ) -> Option<crate::api::SessionEventBoundaryResult> {
            SessionRuntime::with_session_event_boundary(
                &self.inner,
                session_id,
                kind,
                payload,
                request_adoption_id,
                action,
            )
        }

        fn record_session_event(
            &self,
            session_id: &str,
            kind: &str,
            payload: &Value,
        ) -> Option<Result<()>> {
            SessionRuntime::record_session_event(&self.inner, session_id, kind, payload)
        }

        fn can_record_session_event(
            &self,
            session_id: &str,
            kind: &str,
            payload: &Value,
        ) -> Option<Result<bool>> {
            SessionRuntime::can_record_session_event(&self.inner, session_id, kind, payload)
        }

        fn event_writer_health(&self) -> Option<crate::event_writer::EventWriterHealth> {
            SessionRuntime::event_writer_health(&self.inner)
        }
    }

    struct SignalingKiller {
        killed: Arc<(Mutex<bool>, std::sync::Condvar)>,
    }

    impl RuntimeKiller for SignalingKiller {
        fn kill(&mut self) -> Result<()> {
            let (lock, cvar) = &*self.killed;
            *lock.lock().unwrap() = true;
            cvar.notify_all();
            Ok(())
        }
    }

    struct ExitTriggeringKiller {
        on_exit: Option<Box<dyn FnOnce(pty_runner::PtyRunResult) + Send>>,
        started: Arc<(Mutex<bool>, std::sync::Condvar)>,
        exit_thread: Arc<Mutex<Option<std::thread::JoinHandle<()>>>>,
    }

    impl RuntimeKiller for ExitTriggeringKiller {
        fn kill(&mut self) -> Result<()> {
            let on_exit = self.on_exit.take().expect("exit callback is present");
            let started = Arc::clone(&self.started);
            let handle = std::thread::spawn(move || {
                let (lock, cvar) = &*started;
                *lock.lock().unwrap() = true;
                cvar.notify_all();
                on_exit(pty_runner::PtyRunResult {
                    status: "killed",
                    exit_code: None,
                });
            });
            let (lock, cvar) = &*self.started;
            let mut guard = lock.lock().unwrap();
            while !*guard {
                guard = cvar.wait(guard).unwrap();
            }
            drop(guard);
            std::thread::yield_now();
            *self.exit_thread.lock().unwrap() = Some(handle);
            Ok(())
        }
    }

    struct RegistryLockCheckingKiller {
        sessions: Weak<Mutex<HashMap<String, LiveSessionHandle>>>,
        dropped_after_unlock: Arc<AtomicBool>,
    }

    impl RuntimeKiller for RegistryLockCheckingKiller {
        fn kill(&mut self) -> Result<()> {
            Ok(())
        }
    }

    impl Drop for RegistryLockCheckingKiller {
        fn drop(&mut self) {
            let dropped_after_unlock = match self.sessions.upgrade() {
                Some(sessions) => sessions.try_lock().is_ok(),
                None => true,
            };
            self.dropped_after_unlock
                .store(dropped_after_unlock, Ordering::Release);
        }
    }

    /// `Write` impl whose `write` blocks until a kill signal is set.
    /// Used to simulate a stream-mode child that has stopped reading
    /// stdin — we want `kill_session` to succeed even while
    /// `send_input` is mid-write to that exact session.
    #[derive(Clone)]
    struct BlockingWriter {
        entered: std::sync::Arc<(std::sync::Mutex<bool>, std::sync::Condvar)>,
        unblock: std::sync::Arc<(std::sync::Mutex<bool>, std::sync::Condvar)>,
    }

    impl Write for BlockingWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            let (entered_lock, entered_cvar) = &*self.entered;
            *entered_lock.lock().unwrap() = true;
            entered_cvar.notify_all();
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

        let temp_dir = tempfile::tempdir().unwrap();
        let conn = crate::store::open_store(&temp_dir.path().join(crate::STORE_FILE_NAME)).unwrap();
        crate::store::insert_session(&conn, &session_record("wedged-session")).unwrap();
        drop(conn);

        let runtime = Arc::new(LiveSessionRuntime::with_coven_home(
            temp_dir.path().to_path_buf(),
        ));
        let entered = Arc::new((StdMutex::new(false), Condvar::new()));
        let unblock = Arc::new((StdMutex::new(false), Condvar::new()));
        let writer = BlockingWriter {
            entered: Arc::clone(&entered),
            unblock: Arc::clone(&unblock),
        };
        let killed = Arc::new((StdMutex::new(false), Condvar::new()));
        runtime
            .register(
                "wedged-session".to_string(),
                Box::new(writer),
                Box::new(SignalingKiller {
                    killed: Arc::clone(&killed),
                }),
            )
            .unwrap();

        let sender_runtime = Arc::clone(&runtime);
        let sender = thread::spawn(move || {
            let payload = serde_json::json!({ "data": "wedge" });
            let mut action = || {
                SessionRuntime::send_input(&*sender_runtime, "wedged-session", &payload)
                    .map_err(crate::api::SessionEventBoundaryError::Runtime)
            };
            SessionRuntime::with_session_event_boundary(
                &*sender_runtime,
                "wedged-session",
                "input",
                &payload,
                None,
                &mut action,
            )
            .expect("writer-backed runtime handles input boundaries")
        });

        {
            let (lock, cvar) = &*entered;
            let mut guard = lock.lock().unwrap();
            while !*guard {
                guard = cvar.wait(guard).unwrap();
            }
        }

        let killer_runtime = Arc::clone(&runtime);
        let killer = thread::spawn(move || {
            let payload = serde_json::json!({ "status": "killed" });
            let mut action = || {
                SessionRuntime::kill_session(&*killer_runtime, "wedged-session")
                    .map_err(crate::api::SessionEventBoundaryError::Runtime)
            };
            SessionRuntime::with_session_event_boundary(
                &*killer_runtime,
                "wedged-session",
                "kill",
                &payload,
                None,
                &mut action,
            )
            .expect("writer-backed runtime handles kill boundaries")
        });

        {
            let (lock, cvar) = &*killed;
            let mut guard = lock.lock().unwrap();
            while !*guard {
                guard = cvar.wait(guard).unwrap();
            }
        }
        {
            let (lock, cvar) = &*unblock;
            *lock.lock().unwrap() = true;
            cvar.notify_all();
        }
        sender.join().unwrap().unwrap();
        killer.join().unwrap().unwrap();
    }

    #[test]
    fn failed_kill_retains_exact_handle_for_retry() {
        let runtime = LiveSessionRuntime::default();
        let attempts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        runtime
            .register(
                "retry-kill".to_string(),
                Box::new(SharedBuffer::default()),
                Box::new(CountingFailingKiller {
                    attempts: Arc::clone(&attempts),
                }),
            )
            .unwrap();

        for expected_attempts in 1..=2 {
            let error = SessionRuntime::kill_session(&runtime, "retry-kill")
                .expect_err("failing quiescence proof remains retryable");
            assert!(error.to_string().contains("quiescence proof"));
            assert_eq!(attempts.load(Ordering::Acquire), expected_attempts);
            assert!(
                runtime.sessions.lock().unwrap().contains_key("retry-kill"),
                "failed kill discarded the exact ownership handle"
            );
        }
    }

    #[test]
    fn kill_event_commits_before_concurrent_exit() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let conn = crate::store::open_store(&temp_dir.path().join(crate::STORE_FILE_NAME))?;
        crate::store::insert_session(&conn, &session_record("kill-exit-session"))?;
        drop(conn);

        let runtime = LiveSessionRuntime::with_coven_home(temp_dir.path().to_path_buf());
        let (observer, registration) =
            runtime.observer_for_session("kill-exit-session".to_string());
        let pty_runner::DetachedPtyObserver { on_exit, .. } = observer;
        let started = Arc::new((Mutex::new(false), std::sync::Condvar::new()));
        let exit_thread = Arc::new(Mutex::new(None));
        runtime.register_kind_with_registration(
            "kill-exit-session".to_string(),
            LiveSessionKind::Pty,
            Box::new(SharedBuffer::default()),
            Box::new(ExitTriggeringKiller {
                on_exit: Some(on_exit),
                started,
                exit_thread: Arc::clone(&exit_thread),
            }),
            registration,
        )?;

        let response = crate::api::handle_request_with_runtime(
            "POST",
            "/sessions/kill-exit-session/kill",
            temp_dir.path(),
            None,
            None,
            &runtime,
        )?;
        assert_eq!(response.status, 202);
        exit_thread
            .lock()
            .unwrap()
            .take()
            .expect("exit thread was started")
            .join()
            .unwrap();

        let conn = crate::store::open_store(&temp_dir.path().join(crate::STORE_FILE_NAME))?;
        let kinds = crate::store::list_events(&conn, "kill-exit-session")?
            .into_iter()
            .map(|event| event.kind)
            .collect::<Vec<_>>();
        assert_eq!(kinds, ["kill", "exit"]);
        Ok(())
    }

    #[test]
    fn http_reason_phrase_names_bad_requests() {
        assert_eq!(http_reason_phrase(400), "Bad Request");
    }

    #[test]
    fn http_reason_phrase_names_memory_detail_failures() {
        let phrases = [
            (413, http_reason_phrase(413)),
            (422, http_reason_phrase(422)),
            (503, http_reason_phrase(503)),
        ];

        assert_eq!(
            phrases,
            [
                (413, "Payload Too Large"),
                (422, "Unprocessable Content"),
                (503, "Service Unavailable"),
            ]
        );
        assert!(phrases.iter().all(|(_, phrase)| *phrase != "OK"));
    }

    #[cfg(unix)]
    #[test]
    fn owner_local_ipc_health_advertises_session_launch_policy() {
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
            HostGuard::Disabled,
        )
        .expect("handle ok");
        let response = String::from_utf8(output).expect("utf8");
        assert!(response.starts_with("HTTP/1.1 200 OK"), "got: {response}");
        assert!(response.contains("\"apiVersion\""), "got: {response}");
        assert!(
            response.contains(r#""sessionLaunchPolicy":true"#),
            "got: {response}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn handle_http_stream_converts_api_handler_error_to_json_500() {
        use crate::api::NoopSessionRuntime;
        use std::io::Cursor;
        let temp = tempfile::tempdir().expect("tempdir");
        ensure_private_coven_home(temp.path()).expect("ensure home");
        std::fs::write(temp.path().join("familiars.toml"), "not = [valid")
            .expect("write malformed config");
        let request = b"GET /api/v1/familiars HTTP/1.1\r\nHost: x\r\nContent-Length: 0\r\n\r\n";
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
            HostGuard::Disabled,
        )
        .expect("handler errors should still write an HTTP response");

        let response = String::from_utf8(output).expect("utf8");
        assert!(
            response.starts_with("HTTP/1.1 500 Internal Server Error"),
            "got: {response}"
        );
        assert!(
            response.contains(r#""code":"internal_error""#),
            "got: {response}"
        );
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
            HostGuard::Disabled,
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
            HostGuard::Loopback { allowed_hosts: &[] },
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
            HostGuard::Loopback { allowed_hosts: &[] },
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
    fn handle_http_stream_allow_host_permits_listed_host_and_origin() {
        use crate::api::NoopSessionRuntime;
        use std::io::Cursor;
        let temp = tempfile::tempdir().expect("tempdir");
        ensure_private_coven_home(temp.path()).expect("ensure home");
        // A Tailscale-served request: the proxy forwards the tailnet FQDN as both
        // Host and Origin. Neither is loopback, but both are on the allowlist.
        let request = b"GET /api/v1/health HTTP/1.1\r\nHost: coven-host.taile46e90.ts.net\r\nOrigin: https://coven-host.taile46e90.ts.net\r\n\r\n";
        let mut stream = Cursor::new(Vec::from(&request[..]));
        let mut output: Vec<u8> = Vec::new();
        let allowed = vec!["coven-host.taile46e90.ts.net".to_string()];
        handle_http_stream(
            &mut stream,
            &mut output,
            temp.path(),
            None,
            &NoopSessionRuntime,
            Some(MAX_TCP_BODY_BYTES),
            HostGuard::Loopback {
                allowed_hosts: &allowed,
            },
        )
        .expect("handle ok");
        let response = String::from_utf8(output).expect("utf8");
        assert!(response.starts_with("HTTP/1.1 200 OK"), "got: {response}");
    }

    #[cfg(unix)]
    #[test]
    fn handle_http_stream_allow_host_still_blocks_unlisted_host() {
        use crate::api::NoopSessionRuntime;
        use std::io::Cursor;
        let temp = tempfile::tempdir().expect("tempdir");
        ensure_private_coven_home(temp.path()).expect("ensure home");
        // Allowlisting one host must not open the guard for a different one.
        let request = b"GET /api/v1/health HTTP/1.1\r\nHost: evil.example\r\n\r\n";
        let mut stream = Cursor::new(Vec::from(&request[..]));
        let mut output: Vec<u8> = Vec::new();
        let allowed = vec!["coven-host.taile46e90.ts.net".to_string()];
        handle_http_stream(
            &mut stream,
            &mut output,
            temp.path(),
            None,
            &NoopSessionRuntime,
            Some(MAX_TCP_BODY_BYTES),
            HostGuard::Loopback {
                allowed_hosts: &allowed,
            },
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
    fn host_and_origin_allowlist_match_is_case_and_port_insensitive() {
        let allowed = vec!["Coven-Host.Taile46E90.TS.net".to_string()];
        // Host: case-insensitive, and a forwarded port must not defeat the match.
        assert!(host_in_allowlist(
            Some("coven-host.taile46e90.ts.net:3000"),
            &allowed
        ));
        assert!(host_in_allowlist(
            Some("COVEN-HOST.TAILE46E90.TS.NET"),
            &allowed
        ));
        // Origin: scheme is stripped, host compared the same way.
        assert!(origin_in_allowlist(
            "https://coven-host.taile46e90.ts.net",
            &allowed
        ));
        // Non-members and an empty allowlist never match.
        assert!(!host_in_allowlist(Some("evil.example"), &allowed));
        assert!(!host_in_allowlist(
            Some("coven-host.taile46e90.ts.net"),
            &[]
        ));
        assert!(!host_in_allowlist(None, &allowed));
        assert!(!origin_in_allowlist("https://evil.example", &allowed));
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
            HostGuard::Loopback { allowed_hosts: &[] },
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
            HostGuard::Disabled,
        )
        .expect("handle ok");
        let response = String::from_utf8(output).expect("utf8");
        assert!(response.starts_with("HTTP/1.1 200 OK"), "got: {response}");
    }

    #[cfg(unix)]
    #[test]
    fn tcp_health_does_not_advertise_owner_only_session_launch_policy() {
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
            serve_next_tcp_connection(&listener, &coven_home, None, &runtime, &[])
                .expect("serve tcp");
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
        assert!(
            response.contains(r#""sessionLaunchPolicy":false"#),
            "got: {response}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn tcp_rejects_launch_policy_before_session_row_or_runtime_launch() -> Result<()> {
        use std::io::{Read, Write};
        use std::net::TcpStream;
        use std::sync::atomic::AtomicBool;
        use std::thread;

        struct LaunchDetectingRuntime(Arc<AtomicBool>);
        impl SessionRuntime for LaunchDetectingRuntime {
            fn launch_session(&self, _launch: &crate::api::SessionLaunch) -> Result<()> {
                self.0.store(true, Ordering::Release);
                Ok(())
            }

            fn send_input(&self, _session_id: &str, _payload: &Value) -> Result<()> {
                Ok(())
            }

            fn kill_session(&self, _session_id: &str) -> Result<()> {
                Ok(())
            }
        }

        let temp = tempfile::tempdir()?;
        ensure_private_coven_home(temp.path())?;
        let project_root = temp.path().join("project");
        std::fs::create_dir_all(&project_root)?;
        let body = serde_json::json!({
            "projectRoot": project_root,
            "harness": "codex",
            "launchMode": "nonInteractive",
            "launchPolicy": {
                "approval": "never",
                "sandbox": "workspace-write",
                "addDirs": []
            },
            "prompt": "write artifacts/primary.md"
        })
        .to_string();
        let request = format!(
            "POST /api/v1/sessions HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body
        );
        let listener = bind_tcp_listener("127.0.0.1:0")?;
        let addr = listener.local_addr()?;
        let coven_home = temp.path().to_path_buf();
        let launched = Arc::new(AtomicBool::new(false));
        let launched_for_server = Arc::clone(&launched);
        let server = thread::spawn(move || {
            let runtime = LaunchDetectingRuntime(launched_for_server);
            serve_next_tcp_connection(&listener, &coven_home, None, &runtime, &[])
        });

        let mut client = TcpStream::connect(addr)?;
        client.set_read_timeout(Some(Duration::from_secs(5)))?;
        client.write_all(request.as_bytes())?;
        let mut response = String::new();
        client.read_to_string(&mut response)?;
        server.join().expect("server thread")?;

        assert!(
            response.starts_with("HTTP/1.1 403 Forbidden"),
            "got: {response}"
        );
        assert!(response.contains(r#""code":"forbidden""#), "{response}");
        assert!(response.contains("owner-gated local IPC"), "{response}");
        assert!(!launched.load(Ordering::Acquire));
        let conn = crate::store::open_store(&temp.path().join(crate::STORE_FILE_NAME))?;
        assert!(crate::store::list_sessions(&conn)?.is_empty());
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn bind_tcp_listener_serves_memory_overview_over_tcp() -> Result<()> {
        use crate::api::NoopSessionRuntime;
        use std::io::{Read, Write};
        use std::net::TcpStream;
        use std::thread;

        let temp = tempfile::tempdir()?;
        ensure_private_coven_home(temp.path())?;
        let memory_dir = temp.path().join("memory").join("sage");
        std::fs::create_dir_all(&memory_dir)?;
        std::fs::write(memory_dir.join("notes.md"), "Durable fact.")?;
        let listener = bind_tcp_listener("127.0.0.1:0")?;
        let addr = listener.local_addr()?;
        let coven_home = temp.path().to_path_buf();
        let server = thread::spawn(move || {
            let runtime = NoopSessionRuntime;
            serve_next_tcp_connection(&listener, &coven_home, None, &runtime, &[])
        });

        let mut client = TcpStream::connect(addr)?;
        client.set_read_timeout(Some(std::time::Duration::from_secs(5)))?;
        client.write_all(
            b"GET /api/v1/memory/overview HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: 0\r\n\r\n",
        )?;
        let mut response = String::new();
        client.read_to_string(&mut response)?;
        server.join().expect("server thread")?;

        assert!(response.starts_with("HTTP/1.1 200 OK"), "got: {response}");
        assert!(response.contains(r#""entries":1"#), "got: {response}");
        assert!(response.contains(r#""detail":true"#), "got: {response}");
        Ok(())
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
        // a single /health probe, reporting its authenticated peer pid.
        let incumbent = bind_api_socket(&home).expect("bind incumbent");
        let canonical_home = std::fs::canonicalize(&home).expect("canonicalize home");
        let socket = daemon_socket_path(&canonical_home)
            .to_string_lossy()
            .into_owned();
        let server_home = home.clone();
        let status = DaemonStatus {
            pid: std::process::id(),
            started_at: "2026-06-18T00:00:00Z".to_string(),
            socket,
            process_creation_time: None,
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
            msg.contains("refusing to take over") && msg.contains(&std::process::id().to_string()),
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
    fn recovers_stale_created_sessions_as_failed() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let conn = crate::store::open_store(&temp_dir.path().join("coven.sqlite3"))?;
        // Helper's fixed 2026-04-27 created_at sits far past the TTL → stale.
        let mut stale = session_record("stale-created");
        stale.status = "created".to_string();
        // A row registered "just now" is inside the TTL and must survive —
        // its `coven run` may still be launching.
        let mut fresh = session_record("fresh-created");
        fresh.status = "created".to_string();
        fresh.created_at = crate::api::current_timestamp();
        let mut completed = session_record("completed");
        completed.status = "completed".to_string();
        crate::store::insert_session(&conn, &stale)?;
        crate::store::insert_session(&conn, &fresh)?;
        crate::store::insert_session(&conn, &completed)?;
        drop(conn);

        let updated = recover_stale_created_sessions(temp_dir.path(), "2026-04-27T08:00:00Z")?;
        let conn = crate::store::open_store(&temp_dir.path().join("coven.sqlite3"))?;
        let sessions = crate::store::list_sessions(&conn)?;

        assert_eq!(updated, 1);
        assert_eq!(session_status(&sessions, "stale-created"), "failed");
        assert_eq!(session_status(&sessions, "fresh-created"), "created");
        assert_eq!(session_status(&sessions, "completed"), "completed");
        Ok(())
    }

    #[test]
    fn writes_reads_and_clears_daemon_status() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let status = DaemonStatus {
            pid: 12345,
            started_at: "2026-04-27T10:00:00Z".to_string(),
            socket: test_daemon_status_socket(temp_dir.path()),
            process_creation_time: None,
        };

        write_status(temp_dir.path(), &status)?;

        assert_eq!(read_status(temp_dir.path())?, Some(status));
        assert!(clear_status(temp_dir.path())?);
        assert_eq!(read_status(temp_dir.path())?, None);
        assert!(!clear_status(temp_dir.path())?);
        Ok(())
    }

    fn windows_status_fixture(process_creation_time: Option<&str>) -> String {
        let mut status = serde_json::json!({
            "pid": 12345,
            "startedAt": "2026-04-27T10:00:00Z",
            "socket": "coven-daemon-v1-fixture.sock",
        });
        if let Some(process_creation_time) = process_creation_time {
            status["processCreationTime"] =
                serde_json::Value::String(process_creation_time.to_owned());
        }
        status.to_string()
    }

    #[test]
    fn windows_status_creation_time_matches_the_same_live_process() -> Result<()> {
        let creation_time = 134_157_822_123_456_789_u64;
        let status =
            parse_daemon_status(&windows_status_fixture(Some(&creation_time.to_string())))?;

        assert_eq!(
            recorded_windows_process_state(&status, Some(creation_time)),
            RecordedProcessState::Matching
        );
        Ok(())
    }

    #[test]
    fn windows_status_creation_time_detects_a_reused_pid() -> Result<()> {
        let status = parse_daemon_status(&windows_status_fixture(Some("134157822123456789")))?;

        assert_eq!(
            recorded_windows_process_state(&status, Some(134_157_822_999_999_999)),
            RecordedProcessState::Mismatched
        );
        Ok(())
    }

    #[test]
    fn legacy_windows_status_without_creation_time_remains_unverifiable() -> Result<()> {
        let status = parse_daemon_status(&windows_status_fixture(None))?;

        assert_eq!(
            recorded_windows_process_state(&status, Some(134_157_822_123_456_789)),
            RecordedProcessState::Unverifiable
        );
        assert_eq!(
            recorded_windows_process_state(&status, None),
            RecordedProcessState::Gone
        );
        Ok(())
    }

    #[test]
    fn malformed_windows_status_creation_time_fails_closed() {
        for malformed in [
            serde_json::Value::Null,
            serde_json::json!(134_157_822_123_456_789_u64),
            serde_json::json!(""),
            serde_json::json!("0"),
            serde_json::json!("-1"),
            serde_json::json!("not-a-filetime"),
            serde_json::json!("18446744073709551616"),
        ] {
            let mut status: serde_json::Value =
                serde_json::from_str(&windows_status_fixture(None)).unwrap();
            status["processCreationTime"] = malformed.clone();

            let error = parse_daemon_status(&status.to_string())
                .expect_err("malformed process creation time must be rejected");
            assert!(
                error
                    .to_string()
                    .contains("invalid Windows process creation time"),
                "unexpected error for {malformed}: {error:#}"
            );
        }
    }

    #[test]
    fn windows_process_creation_time_serializes_without_losing_filetime_width() -> Result<()> {
        let status = parse_daemon_status(&windows_status_fixture(Some(&u64::MAX.to_string())))?;

        let serialized = serde_json::to_value(&status)?;
        assert_eq!(
            serialized["processCreationTime"],
            serde_json::Value::String(u64::MAX.to_string())
        );
        Ok(())
    }

    #[cfg(windows)]
    #[test]
    fn reads_stale_windows_status_without_a_live_pipe() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let status = DaemonStatus {
            pid: 12345,
            started_at: "2026-04-27T10:00:00Z".to_string(),
            socket: windows_pipe_name(temp_dir.path())?,
            process_creation_time: None,
        };

        write_status(temp_dir.path(), &status)?;

        assert_eq!(read_status(temp_dir.path())?, Some(status));
        Ok(())
    }

    #[cfg(windows)]
    #[test]
    fn stop_clears_stale_windows_status_without_a_live_pipe() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let status = DaemonStatus {
            pid: 12345,
            started_at: "2026-04-27T10:00:00Z".to_string(),
            socket: windows_pipe_name(temp_dir.path())?,
            process_creation_time: None,
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

    #[cfg(windows)]
    fn inherited_legacy_windows_status(
        coven_home: &Path,
        pid: u32,
        socket: Option<String>,
    ) -> Result<DaemonStatus> {
        use std::hash::{DefaultHasher, Hash, Hasher};

        let socket = socket.unwrap_or_else(|| {
            let mut hasher = DefaultHasher::new();
            coven_home.to_string_lossy().hash(&mut hasher);
            format!("coven-daemon-{:016x}.sock", hasher.finish())
        });
        let status = DaemonStatus {
            pid,
            started_at: "2026-04-27T10:00:00Z".to_owned(),
            socket,
            process_creation_time: None,
        };
        write_inherited_windows_status(coven_home, serde_json::to_vec(&status)?)?;
        Ok(status)
    }

    #[cfg(windows)]
    #[test]
    fn lifecycle_clears_inherited_legacy_status_only_after_pid_is_dead() -> Result<()> {
        use coven_client::DaemonEndpoint;

        let temp_dir = tempfile::tempdir()?;
        let mut exited = Command::new("cmd.exe").args(["/C", "exit", "0"]).spawn()?;
        let exited_pid = exited.id();
        assert!(exited.wait()?.success());
        let status = inherited_legacy_windows_status(temp_dir.path(), exited_pid, None)?;
        assert!(
            DaemonEndpoint::discover(temp_dir.path()).is_err(),
            "endpoint discovery must reject an unavailable legacy pipe"
        );

        let state = background_server_status(temp_dir.path())?;

        assert_eq!(state, None);
        assert!(!daemon_status_path(temp_dir.path()).exists());
        assert_ne!(status.socket, windows_pipe_name(temp_dir.path())?);
        Ok(())
    }

    #[cfg(windows)]
    #[test]
    fn lifecycle_preserves_inherited_legacy_status_while_pid_is_alive() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let status = inherited_legacy_windows_status(temp_dir.path(), std::process::id(), None)?;

        let state = background_server_status(temp_dir.path())?;
        assert_eq!(state, Some(DaemonStatusState::Stale(status.clone())));
        ensure_background_server(
            temp_dir.path(),
            Path::new("coven.exe"),
            "2026-04-27T11:00:00Z".to_owned(),
        )
        .expect_err("start must not replace an unavailable legacy record with a live PID");
        stop_background_server(temp_dir.path())
            .expect_err("stop must not signal or clear an unverified live PID");
        restart_background_server(
            temp_dir.path(),
            Path::new("coven.exe"),
            "2026-04-27T11:00:00Z".to_owned(),
        )
        .expect_err("restart must not signal or clear an unverified live PID");

        assert_eq!(read_status(temp_dir.path())?, Some(status));
        Ok(())
    }

    #[cfg(windows)]
    #[test]
    fn lifecycle_rejects_redirected_inherited_legacy_status_without_clearing() -> Result<()> {
        use std::hash::{DefaultHasher, Hash, Hasher};

        let temp_dir = tempfile::tempdir()?;
        let other_home = temp_dir.path().join("other-profile");
        let mut hasher = DefaultHasher::new();
        other_home.to_string_lossy().hash(&mut hasher);
        let other_profile = format!("coven-daemon-{:016x}.sock", hasher.finish());

        for socket in [other_profile, "other-daemon.sock".to_owned()] {
            inherited_legacy_windows_status(temp_dir.path(), 12345, Some(socket))?;
            let result = background_server_status(temp_dir.path());
            assert!(result.is_err());
            assert!(
                daemon_status_path(temp_dir.path()).exists(),
                "rejected status must not be cleared"
            );
        }
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
            socket: test_daemon_status_socket(temp_dir.path()),
            process_creation_time: None,
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
    fn daemon_startup_status_socket_uses_unix_socket_for_unix_platform() -> Result<()> {
        let home = Path::new("/tmp/coven-home");
        assert_eq!(
            daemon_startup_status_socket_for_platform(home, DaemonIpcPlatform::Unix)?,
            daemon_socket_path(home).to_string_lossy()
        );
        Ok(())
    }

    #[cfg(windows)]
    #[test]
    fn daemon_startup_status_socket_uses_named_pipe_for_windows_platform() -> Result<()> {
        let home = Path::new(env!("CARGO_MANIFEST_DIR"));
        let socket = daemon_startup_status_socket_for_platform(home, DaemonIpcPlatform::Windows)?;

        assert_eq!(
            socket,
            coven_client::owner_only_windows_pipe_name(home).map_err(anyhow::Error::new)?
        );
        assert!(socket.starts_with("coven-daemon-"), "socket={socket}");
        assert!(socket.ends_with(".sock"), "socket={socket}");
        assert_ne!(socket, daemon_socket_path(home).to_string_lossy());
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

    #[cfg(windows)]
    #[test]
    fn ensure_private_coven_home_preserves_access_to_existing_children() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let existing_dir = temp_dir.path().join("repo");
        let existing_file = existing_dir.join("existing.txt");
        std::fs::create_dir(&existing_dir)?;
        std::fs::write(&existing_file, "before")?;

        ensure_private_coven_home(temp_dir.path())?;

        assert!(existing_dir.is_dir());
        std::fs::write(&existing_file, "after")?;
        assert_eq!(std::fs::read_to_string(&existing_file)?, "after");
        std::fs::write(temp_dir.path().join("new.txt"), "new")?;
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
        write_test_daemon_status_text(temp_dir.path(), "{not json\n")?;

        let error = read_status(temp_dir.path()).expect_err("read_status should remain strict");

        assert!(error.to_string().contains("failed to parse daemon status"));
        assert!(
            daemon_status_path(temp_dir.path()).exists(),
            "strict read should not clear corrupt metadata"
        );
        Ok(())
    }

    #[test]
    fn read_status_rejects_oversized_metadata_without_parsing_or_removing_it() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let status_path = daemon_status_path(temp_dir.path());
        write_test_daemon_status_text(
            temp_dir.path(),
            &" ".repeat(coven_client::MAX_DAEMON_STATUS_BYTES + 1),
        )?;

        let error = read_status(temp_dir.path()).expect_err("oversized daemon status must fail");

        assert!(
            error
                .to_string()
                .contains("daemon status exceeded the 16384-byte limit"),
            "unexpected error: {error:#}"
        );
        assert!(
            status_path.exists(),
            "an oversized status must be preserved"
        );
        Ok(())
    }

    #[test]
    fn background_server_status_clears_corrupt_metadata_without_daemon() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        std::fs::create_dir_all(temp_dir.path())?;
        write_test_daemon_status_text(temp_dir.path(), "{not json\n")?;

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
    fn malformed_process_creation_time_is_repaired_only_from_authenticated_health() -> Result<()> {
        struct AuthenticatedRecoveryController {
            recovered: Option<DaemonStatus>,
        }

        impl DaemonStopController for AuthenticatedRecoveryController {
            fn stop_verified_daemon(
                &self,
                _coven_home: &Path,
                _status: &DaemonStatus,
                _deadline: LifecycleDeadline,
            ) -> Result<VerifiedStopOutcome> {
                panic!("status recovery must not stop a daemon")
            }

            fn recorded_process_state(
                &self,
                _status: &DaemonStatus,
            ) -> Result<RecordedProcessState> {
                Ok(RecordedProcessState::Gone)
            }

            fn status_matches_running_daemon(
                &self,
                _coven_home: &Path,
                status: &DaemonStatus,
                _deadline: LifecycleDeadline,
            ) -> Result<bool> {
                Ok(self.recovered.as_ref() == Some(status))
            }

            fn status_from_default_socket(
                &self,
                _coven_home: &Path,
                _deadline: LifecycleDeadline,
            ) -> Result<Option<DaemonStatus>> {
                Ok(self.recovered.clone())
            }
        }

        let temp_dir = tempfile::tempdir()?;
        let socket = test_daemon_status_socket(temp_dir.path());
        let malformed = serde_json::json!({
            "pid": 12345,
            "startedAt": "2026-08-16T15:30:00Z",
            "socket": socket,
            "processCreationTime": "not-a-filetime",
        });
        write_test_daemon_status_text(temp_dir.path(), &malformed.to_string())?;

        let unavailable = background_server_status_locked_with_controller(
            temp_dir.path(),
            &AuthenticatedRecoveryController { recovered: None },
        );
        assert!(
            unavailable.is_err(),
            "malformed identity without authenticated health must fail closed"
        );
        assert!(
            daemon_status_path(temp_dir.path()).exists(),
            "malformed identity must be preserved while it cannot be securely resolved"
        );

        let recovered = DaemonStatus {
            pid: 54321,
            started_at: "2026-08-16T15:31:00Z".to_owned(),
            socket,
            process_creation_time: Some(WindowsProcessCreationTime::new(134_157_822_123_456_789)?),
        };
        let state = background_server_status_locked_with_controller(
            temp_dir.path(),
            &AuthenticatedRecoveryController {
                recovered: Some(recovered.clone()),
            },
        )?;
        assert_eq!(state, Some(DaemonStatusState::Running(recovered.clone())));
        assert_eq!(read_status(temp_dir.path())?, Some(recovered));
        Ok(())
    }

    #[test]
    fn stop_background_server_keeps_status_when_existing_daemon_survives() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let status = DaemonStatus {
            pid: 12345,
            started_at: "2026-04-27T10:00:00Z".to_string(),
            socket: test_daemon_status_socket(temp_dir.path()),
            process_creation_time: None,
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
            socket: test_daemon_status_socket(temp_dir.path()),
            process_creation_time: None,
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
            socket: test_daemon_status_socket(temp_dir.path()),
            process_creation_time: None,
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
    fn stop_uses_one_verified_process_identity_for_signal_and_wait() -> Result<()> {
        struct BoundIdentityController {
            stop_calls: std::sync::Arc<std::sync::Mutex<usize>>,
        }

        impl DaemonStopController for BoundIdentityController {
            fn stop_verified_daemon(
                &self,
                _coven_home: &Path,
                _status: &DaemonStatus,
                _deadline: LifecycleDeadline,
            ) -> Result<VerifiedStopOutcome> {
                *self.stop_calls.lock().unwrap() += 1;
                Ok(VerifiedStopOutcome::Exited)
            }

            fn recorded_process_state(
                &self,
                _status: &DaemonStatus,
            ) -> Result<RecordedProcessState> {
                panic!("verified stop must not reopen a process by PID")
            }

            fn status_matches_running_daemon(
                &self,
                _coven_home: &Path,
                _status: &DaemonStatus,
                _deadline: LifecycleDeadline,
            ) -> Result<bool> {
                panic!("verified stop must bind health and process identity in one operation")
            }
        }

        let temp_dir = tempfile::tempdir()?;
        write_status(
            temp_dir.path(),
            &DaemonStatus {
                pid: 12345,
                started_at: "2026-04-27T10:00:00Z".to_owned(),
                socket: test_daemon_status_socket(temp_dir.path()),
                process_creation_time: None,
            },
        )?;
        let stop_calls = std::sync::Arc::new(std::sync::Mutex::new(0));

        assert!(stop_background_server_with_controller(
            temp_dir.path(),
            &BoundIdentityController {
                stop_calls: stop_calls.clone(),
            },
        )?);
        assert_eq!(*stop_calls.lock().unwrap(), 1);
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn authenticated_relative_status_is_rewritten_to_the_canonical_socket() -> Result<()> {
        use std::{
            io::{Read, Write},
            os::unix::{fs::PermissionsExt, net::UnixListener},
            time::Instant,
        };

        fn relative_path(base: &Path, target: &Path) -> PathBuf {
            let base = base.components().collect::<Vec<_>>();
            let target = target.components().collect::<Vec<_>>();
            let shared = base
                .iter()
                .zip(&target)
                .take_while(|(left, right)| left == right)
                .count();
            let mut relative = PathBuf::new();
            for _ in shared..base.len() {
                relative.push("..");
            }
            for component in &target[shared..] {
                relative.push(component.as_os_str());
            }
            relative
        }

        let current_dir = std::fs::canonicalize(std::env::current_dir()?)?;
        let test_root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .expect("workspace root")
            .join("c");
        std::fs::create_dir_all(&test_root)?;
        let temp_dir = tempfile::tempdir_in(&test_root)?;
        let coven_home = temp_dir.path().to_path_buf();
        ensure_private_coven_home(&coven_home)?;
        let socket = daemon_socket_path(&coven_home);
        let listener = UnixListener::bind(&socket)?;
        std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600))?;
        let relative_socket = relative_path(&current_dir, &std::fs::canonicalize(&socket)?)
            .to_string_lossy()
            .into_owned();
        let legacy_status = DaemonStatus {
            pid: std::process::id(),
            started_at: "2026-08-16T12:00:00Z".to_owned(),
            socket: relative_socket,
            process_creation_time: None,
        };
        write_status(&coven_home, &legacy_status)?;
        listener.set_nonblocking(true)?;
        let status_for_server = legacy_status.clone();
        let server = std::thread::spawn(move || -> Result<()> {
            let deadline = Instant::now() + Duration::from_secs(1);
            loop {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let mut request = String::new();
                        stream.read_to_string(&mut request)?;
                        assert!(request.starts_with("GET /health HTTP/1.1\r\n"));
                        let body = serde_json::json!({
                            "ok": true,
                            "apiVersion": crate::api::COVEN_API_NAMED_VERSION,
                            "covenVersion": crate::api::COVEN_VERSION,
                            "capabilities": {
                                "sessions": true,
                                "events": true,
                                "eventCursor": "sequence",
                                "structuredErrors": true
                            },
                            "daemon": status_for_server,
                        })
                        .to_string();
                        write!(
                            stream,
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{body}",
                            body.len()
                        )?;
                        return Ok(());
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        if Instant::now() >= deadline {
                            return Ok(());
                        }
                        std::thread::sleep(Duration::from_millis(2));
                    }
                    Err(error) => return Err(error.into()),
                }
            }
        });

        let state = background_server_status(&coven_home)?;
        server.join().expect("relative-status server")?;

        let running = match state {
            Some(DaemonStatusState::Running(status)) => status,
            other => anyhow::bail!("relative same-profile status was not running: {other:?}"),
        };
        let canonical_socket = std::fs::canonicalize(&socket)?;
        assert_eq!(Path::new(&running.socket), canonical_socket);
        let rewritten = read_status(&coven_home)?.expect("rewritten daemon status");
        assert_eq!(Path::new(&rewritten.socket), canonical_socket);
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn copied_profile_status_cannot_select_another_profiles_socket() -> Result<()> {
        use std::{
            io::{Read, Write},
            os::unix::{fs::PermissionsExt, net::UnixListener},
            sync::{
                atomic::{AtomicBool, Ordering},
                Arc,
            },
            time::Instant,
        };

        fn serve_shutdown(
            listener: UnixListener,
            status: DaemonStatus,
            contacted: Arc<AtomicBool>,
        ) -> std::thread::JoinHandle<Result<()>> {
            listener
                .set_nonblocking(true)
                .expect("nonblocking listener");
            std::thread::spawn(move || {
                let deadline = Instant::now() + Duration::from_millis(500);
                loop {
                    match listener.accept() {
                        Ok((mut stream, _)) => {
                            contacted.store(true, Ordering::Release);
                            let mut request = String::new();
                            stream.read_to_string(&mut request)?;
                            let body = serde_json::json!({
                                "ok": true,
                                "apiVersion": crate::api::COVEN_API_NAMED_VERSION,
                                "capabilities": { "structuredErrors": true },
                                "daemon": status,
                            })
                            .to_string();
                            write!(
                                stream,
                                "HTTP/1.1 202 Accepted\r\nContent-Length: {}\r\n\r\n{body}",
                                body.len()
                            )?;
                            return Ok(());
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            if Instant::now() >= deadline {
                                return Ok(());
                            }
                            std::thread::sleep(Duration::from_millis(2));
                        }
                        Err(error) => return Err(error.into()),
                    }
                }
            })
        }

        let root = tempfile::tempdir()?;
        let profile_a = root.path().join("profile-a");
        let profile_b = root.path().join("profile-b");
        ensure_private_coven_home(&profile_a)?;
        ensure_private_coven_home(&profile_b)?;
        let socket_b = daemon_socket_path(&profile_b);
        let listener_b = UnixListener::bind(&socket_b)?;
        std::fs::set_permissions(&socket_b, std::fs::Permissions::from_mode(0o600))?;
        let status_b = DaemonStatus {
            pid: std::process::id(),
            started_at: "2026-08-16T12:00:00Z".to_owned(),
            socket: std::fs::canonicalize(&socket_b)?
                .to_string_lossy()
                .into_owned(),
            process_creation_time: None,
        };
        write_status(&profile_a, &status_b)?;
        let contacted_b = Arc::new(AtomicBool::new(false));
        let server_b = serve_shutdown(listener_b, status_b, Arc::clone(&contacted_b));

        let cross_profile = stop_background_server(&profile_a);
        server_b.join().expect("profile B server thread")?;

        assert!(
            cross_profile.is_err(),
            "profile A must not accept profile B's copied status"
        );
        assert!(
            !contacted_b.load(Ordering::Acquire),
            "profile A connected to profile B's daemon"
        );

        let socket_a = daemon_socket_path(&profile_a);
        let listener_a = UnixListener::bind(&socket_a)?;
        std::fs::set_permissions(&socket_a, std::fs::Permissions::from_mode(0o600))?;
        let status_a = DaemonStatus {
            pid: std::process::id(),
            started_at: "2026-08-16T12:01:00Z".to_owned(),
            socket: std::fs::canonicalize(&socket_a)?
                .to_string_lossy()
                .into_owned(),
            process_creation_time: None,
        };
        write_status(&profile_a, &status_a)?;
        let contacted_a = Arc::new(AtomicBool::new(false));
        let server_a = serve_shutdown(listener_a, status_a, Arc::clone(&contacted_a));

        assert!(stop_background_server(&profile_a)?);
        server_a.join().expect("profile A server thread")?;
        assert!(
            contacted_a.load(Ordering::Acquire),
            "same-profile stop did not reach its daemon"
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn unix_verified_stop_never_signals_a_substituted_numeric_pid() -> Result<()> {
        use std::os::unix::fs::PermissionsExt;

        let temp_dir = tempfile::tempdir()?;
        ensure_private_coven_home(temp_dir.path())?;
        let mut substituted = Command::new("sleep")
            .arg("5")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .context("spawn substituted process")?;
        let canonical_home = std::fs::canonicalize(temp_dir.path())?;
        let status = DaemonStatus {
            pid: substituted.id(),
            started_at: "2026-08-16T12:00:00Z".to_owned(),
            socket: daemon_socket_path(&canonical_home)
                .to_string_lossy()
                .into_owned(),
            process_creation_time: None,
        };
        let listener = UnixListener::bind(daemon_socket_path(temp_dir.path()))?;
        std::fs::set_permissions(
            daemon_socket_path(temp_dir.path()),
            std::fs::Permissions::from_mode(0o600),
        )?;
        let server_status = status.clone();
        let server = std::thread::spawn(move || -> Result<()> {
            let (mut stream, _) = listener.accept()?;
            let mut request = String::new();
            stream.read_to_string(&mut request)?;
            let (code, reason, body) =
                if request.starts_with("POST /api/v1/internal/lifecycle/shutdown HTTP/1.1\r\n") {
                    (
                        202,
                        "Accepted",
                        serde_json::json!({
                            "ok": true,
                            "apiVersion": crate::api::COVEN_API_NAMED_VERSION,
                            "capabilities": { "structuredErrors": true },
                            "daemon": server_status,
                        })
                        .to_string(),
                    )
                } else {
                    (
                        200,
                        "OK",
                        serde_json::json!({
                            "ok": true,
                            "apiVersion": crate::api::COVEN_API_NAMED_VERSION,
                            "covenVersion": crate::api::COVEN_VERSION,
                            "capabilities": { "structuredErrors": true },
                            "daemon": server_status,
                        })
                        .to_string(),
                    )
                };
            write!(
                stream,
                "HTTP/1.1 {code} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            )?;
            Ok(())
        });

        let error = SystemDaemonStopController
            .stop_verified_daemon(
                &canonical_home,
                &status,
                LifecycleDeadline::after(Duration::from_millis(250))?,
            )
            .expect_err("the connected peer must not authenticate a substituted numeric PID");
        server.join().expect("substitute server thread")?;
        let still_running = substituted.try_wait()?.is_none();
        if still_running {
            substituted.kill()?;
        }
        let _ = substituted.wait();

        assert!(
            error.to_string().contains("connected peer pid"),
            "unexpected identity rejection: {error:#}"
        );
        assert!(
            still_running,
            "a daemon connection closing must not authorize signaling its stale numeric PID"
        );
        Ok(())
    }

    #[test]
    fn background_server_status_returns_running_for_verified_daemon() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let status = DaemonStatus {
            pid: 12345,
            started_at: "2026-04-27T10:00:00Z".to_string(),
            socket: test_daemon_status_socket(temp_dir.path()),
            process_creation_time: None,
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
            socket: test_daemon_status_socket(temp_dir.path()),
            process_creation_time: None,
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
    fn reused_pid_fingerprint_mismatch_is_cleared_without_signaling() -> Result<()> {
        struct ReusedPidController {
            stop_calls: std::sync::Arc<std::sync::Mutex<usize>>,
        }

        impl DaemonStopController for ReusedPidController {
            fn stop_verified_daemon(
                &self,
                _coven_home: &Path,
                _status: &DaemonStatus,
                _deadline: LifecycleDeadline,
            ) -> Result<VerifiedStopOutcome> {
                *self.stop_calls.lock().unwrap() += 1;
                Ok(VerifiedStopOutcome::Unverified)
            }

            fn recorded_process_state(
                &self,
                _status: &DaemonStatus,
            ) -> Result<RecordedProcessState> {
                Ok(RecordedProcessState::Mismatched)
            }

            fn status_matches_running_daemon(
                &self,
                _coven_home: &Path,
                _status: &DaemonStatus,
                _deadline: LifecycleDeadline,
            ) -> Result<bool> {
                Ok(false)
            }
        }

        let temp_dir = tempfile::tempdir()?;
        let status = DaemonStatus {
            pid: std::process::id(),
            started_at: "2026-08-16T15:30:00Z".to_owned(),
            socket: test_daemon_status_socket(temp_dir.path()),
            process_creation_time: Some(WindowsProcessCreationTime::new(41)?),
        };
        write_status(temp_dir.path(), &status)?;
        let stop_calls = std::sync::Arc::new(std::sync::Mutex::new(0));
        let controller = ReusedPidController {
            stop_calls: stop_calls.clone(),
        };

        assert_eq!(
            background_server_status_locked_with_controller(temp_dir.path(), &controller)?,
            None
        );
        assert!(
            !daemon_status_path(temp_dir.path()).exists(),
            "PID reuse must not leave lifecycle commands wedged on stale metadata"
        );
        assert_eq!(
            *stop_calls.lock().unwrap(),
            0,
            "status cleanup must never signal a mismatched process identity"
        );
        Ok(())
    }

    #[test]
    fn status_cleanup_cannot_delete_state_published_by_concurrent_startup() -> Result<()> {
        struct BlockingDeadStatusController {
            inspected: std::sync::mpsc::SyncSender<()>,
            release: std::sync::Mutex<std::sync::mpsc::Receiver<()>>,
        }

        impl DaemonStopController for BlockingDeadStatusController {
            fn stop_verified_daemon(
                &self,
                _coven_home: &Path,
                _status: &DaemonStatus,
                _deadline: LifecycleDeadline,
            ) -> Result<VerifiedStopOutcome> {
                panic!("status inspection must not stop a daemon")
            }

            fn recorded_process_state(
                &self,
                _status: &DaemonStatus,
            ) -> Result<RecordedProcessState> {
                Ok(RecordedProcessState::Gone)
            }

            fn status_matches_running_daemon(
                &self,
                _coven_home: &Path,
                _status: &DaemonStatus,
                _deadline: LifecycleDeadline,
            ) -> Result<bool> {
                self.inspected.send(()).unwrap();
                self.release.lock().unwrap().recv().unwrap();
                Ok(false)
            }
        }

        let temp_dir = tempfile::tempdir()?;
        let home = temp_dir.path().to_path_buf();
        let old = DaemonStatus {
            pid: 12345,
            started_at: "old".to_owned(),
            socket: test_daemon_status_socket(&home),
            process_creation_time: None,
        };
        let replacement = DaemonStatus {
            pid: 54321,
            started_at: "new".to_owned(),
            socket: test_daemon_status_socket(&home),
            process_creation_time: None,
        };
        write_status(&home, &old)?;

        let (inspected_tx, inspected_rx) = std::sync::mpsc::sync_channel(0);
        let (release_tx, release_rx) = std::sync::mpsc::sync_channel(0);
        let status_home = home.clone();
        let status_thread = std::thread::spawn(move || {
            background_server_status_locked_with_controller(
                &status_home,
                &BlockingDeadStatusController {
                    inspected: inspected_tx,
                    release: std::sync::Mutex::new(release_rx),
                },
            )
        });
        inspected_rx.recv_timeout(Duration::from_secs(2))?;

        let (attempting_tx, attempting_rx) = std::sync::mpsc::sync_channel(0);
        let (acquired_tx, acquired_rx) = std::sync::mpsc::sync_channel(0);
        let startup_home = home.clone();
        let expected_replacement = replacement.clone();
        let startup_thread = std::thread::spawn(move || -> Result<()> {
            attempting_tx.send(())?;
            let _lock = acquire_daemon_lifecycle_lock(&startup_home)?;
            acquired_tx.send(())?;
            write_status(&startup_home, &expected_replacement)
        });
        attempting_rx.recv_timeout(Duration::from_secs(2))?;
        assert!(
            acquired_rx
                .recv_timeout(Duration::from_millis(250))
                .is_err(),
            "startup acquired daemon.lock while stale cleanup was still inspecting state"
        );

        release_tx.send(())?;
        assert_eq!(status_thread.join().unwrap()?, None);
        acquired_rx.recv_timeout(Duration::from_secs(2))?;
        startup_thread.join().unwrap()?;

        assert_eq!(read_status(&home)?, Some(replacement));
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn status_cleanup_cannot_delete_state_from_direct_serve_startup() -> Result<()> {
        struct BlockingDeadStatusController {
            inspected: std::sync::mpsc::SyncSender<()>,
            release: std::sync::Mutex<std::sync::mpsc::Receiver<()>>,
        }

        impl DaemonStopController for BlockingDeadStatusController {
            fn stop_verified_daemon(
                &self,
                _coven_home: &Path,
                _status: &DaemonStatus,
                _deadline: LifecycleDeadline,
            ) -> Result<VerifiedStopOutcome> {
                panic!("status inspection must not stop a daemon")
            }

            fn recorded_process_state(
                &self,
                _status: &DaemonStatus,
            ) -> Result<RecordedProcessState> {
                Ok(RecordedProcessState::Gone)
            }

            fn status_matches_running_daemon(
                &self,
                _coven_home: &Path,
                _status: &DaemonStatus,
                _deadline: LifecycleDeadline,
            ) -> Result<bool> {
                self.inspected.send(()).unwrap();
                self.release.lock().unwrap().recv().unwrap();
                Ok(false)
            }
        }

        let temp_dir = tempfile::tempdir()?;
        let home = temp_dir.path().to_path_buf();
        let old = DaemonStatus {
            pid: 12345,
            started_at: "old".to_owned(),
            socket: daemon_socket_path(&home).to_string_lossy().into_owned(),
            process_creation_time: None,
        };
        let replacement = DaemonStatus {
            pid: 54321,
            started_at: "direct-serve".to_owned(),
            socket: daemon_socket_path(&home).to_string_lossy().into_owned(),
            process_creation_time: None,
        };
        write_status(&home, &old)?;

        let (inspected_tx, inspected_rx) = std::sync::mpsc::sync_channel(0);
        let (release_tx, release_rx) = std::sync::mpsc::sync_channel(0);
        let status_home = home.clone();
        let status_thread = std::thread::spawn(move || {
            background_server_status_locked_with_controller(
                &status_home,
                &BlockingDeadStatusController {
                    inspected: inspected_tx,
                    release: std::sync::Mutex::new(release_rx),
                },
            )
        });
        inspected_rx.recv_timeout(Duration::from_secs(2))?;

        let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(0);
        let (finish_tx, finish_rx) = std::sync::mpsc::sync_channel(0);
        let startup_home = home.clone();
        let expected_replacement = replacement.clone();
        let startup_thread = std::thread::spawn(move || -> Result<()> {
            let _serve_lock = acquire_serve_lock(&startup_home)?;
            let _listener = bind_api_socket(&startup_home)?;
            write_status(&startup_home, &expected_replacement)?;
            ready_tx.send(())?;
            finish_rx.recv()?;
            Ok(())
        });
        ready_rx.recv_timeout(Duration::from_secs(2))?;

        release_tx.send(())?;
        let state = status_thread.join().expect("status thread")?;
        let status_after_cleanup = read_status(&home).ok().flatten();
        let socket_survived = daemon_socket_path(&home).exists();
        finish_tx.send(())?;
        startup_thread.join().expect("startup thread")?;

        assert_eq!(state, None);
        assert_eq!(status_after_cleanup, Some(replacement));
        assert!(
            socket_survived,
            "stale cleanup unlinked the direct serve process's bound socket"
        );
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

    struct RecoveringLifecycleStopController {
        recovered: Option<DaemonStatus>,
        authenticated: bool,
        stopped: std::sync::Arc<std::sync::Mutex<Vec<DaemonStatus>>>,
    }

    impl DaemonStopController for RecoveringLifecycleStopController {
        fn stop_verified_daemon(
            &self,
            _coven_home: &Path,
            status: &DaemonStatus,
            _deadline: LifecycleDeadline,
        ) -> Result<VerifiedStopOutcome> {
            self.stopped.lock().unwrap().push(status.clone());
            Ok(VerifiedStopOutcome::Exited)
        }

        fn recorded_process_state(&self, _status: &DaemonStatus) -> Result<RecordedProcessState> {
            Ok(RecordedProcessState::Gone)
        }

        fn status_matches_running_daemon(
            &self,
            _coven_home: &Path,
            status: &DaemonStatus,
            _deadline: LifecycleDeadline,
        ) -> Result<bool> {
            Ok(self.authenticated && self.recovered.as_ref() == Some(status))
        }

        fn status_from_default_socket(
            &self,
            _coven_home: &Path,
            _deadline: LifecycleDeadline,
        ) -> Result<Option<DaemonStatus>> {
            Ok(self.recovered.clone())
        }
    }

    #[test]
    fn restart_recovers_and_stops_live_daemon_when_status_is_missing_or_corrupt() -> Result<()> {
        for corrupt in [false, true] {
            let temp_dir = tempfile::tempdir()?;
            if corrupt {
                write_test_daemon_status_text(temp_dir.path(), "{not json\n")?;
            }
            let recovered = DaemonStatus {
                pid: 12345,
                started_at: "old".to_string(),
                socket: test_daemon_status_socket(temp_dir.path()),
                process_creation_time: None,
            };
            let stopped = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
            let started = std::sync::Arc::new(std::sync::Mutex::new(0));

            let (was_running, replacement) = restart_background_server_with_controllers_until(
                temp_dir.path(),
                Path::new("/usr/bin/coven"),
                "new".to_string(),
                &RecoveringLifecycleStopController {
                    recovered: Some(recovered.clone()),
                    authenticated: true,
                    stopped: stopped.clone(),
                },
                &FakeStartController {
                    started: started.clone(),
                    running_after_start: true,
                },
                LifecycleDeadline::after(DAEMON_LIFECYCLE_TIMEOUT)?,
            )?;

            assert!(was_running, "recovery mode corrupt={corrupt}");
            assert_eq!(&*stopped.lock().unwrap(), &[recovered]);
            assert_eq!(*started.lock().unwrap(), 1);
            assert_eq!(read_status(temp_dir.path())?, Some(replacement));
        }
        Ok(())
    }

    #[test]
    fn restart_recovers_live_default_socket_after_corrupt_recorded_identity() -> Result<()> {
        struct ReauthenticatingStopController {
            recovered: DaemonStatus,
            stopped: std::sync::Arc<std::sync::Mutex<Vec<DaemonStatus>>>,
        }

        impl DaemonStopController for ReauthenticatingStopController {
            fn stop_verified_daemon(
                &self,
                _coven_home: &Path,
                status: &DaemonStatus,
                _deadline: LifecycleDeadline,
            ) -> Result<VerifiedStopOutcome> {
                self.stopped.lock().unwrap().push(status.clone());
                Ok(if status == &self.recovered {
                    VerifiedStopOutcome::Exited
                } else {
                    VerifiedStopOutcome::Unverified
                })
            }

            fn recorded_process_state(
                &self,
                _status: &DaemonStatus,
            ) -> Result<RecordedProcessState> {
                Ok(RecordedProcessState::Gone)
            }

            fn status_matches_running_daemon(
                &self,
                _coven_home: &Path,
                status: &DaemonStatus,
                _deadline: LifecycleDeadline,
            ) -> Result<bool> {
                Ok(status == &self.recovered)
            }

            fn status_from_default_socket(
                &self,
                _coven_home: &Path,
                _deadline: LifecycleDeadline,
            ) -> Result<Option<DaemonStatus>> {
                Ok(Some(self.recovered.clone()))
            }
        }

        let temp_dir = tempfile::tempdir()?;
        let socket = test_daemon_status_socket(temp_dir.path());
        let corrupt = DaemonStatus {
            pid: 11111,
            started_at: "corrupt".to_string(),
            socket: socket.clone(),
            process_creation_time: None,
        };
        let recovered = DaemonStatus {
            pid: 22222,
            started_at: "authenticated".to_string(),
            socket,
            process_creation_time: None,
        };
        write_status(temp_dir.path(), &corrupt)?;
        let stopped = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let started = std::sync::Arc::new(std::sync::Mutex::new(0));

        restart_background_server_with_controllers_until(
            temp_dir.path(),
            Path::new("/usr/bin/coven"),
            "new".to_string(),
            &ReauthenticatingStopController {
                recovered: recovered.clone(),
                stopped: stopped.clone(),
            },
            &FakeStartController {
                started: started.clone(),
                running_after_start: true,
            },
            LifecycleDeadline::after(DAEMON_LIFECYCLE_TIMEOUT)?,
        )?;

        assert_eq!(&*stopped.lock().unwrap(), &[corrupt, recovered]);
        assert_eq!(*started.lock().unwrap(), 1);
        Ok(())
    }

    #[test]
    fn stop_recovers_authenticated_default_after_untrusted_status_read() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let recovered = DaemonStatus {
            pid: 22222,
            started_at: "authenticated".to_string(),
            socket: test_daemon_status_socket(temp_dir.path()),
            process_creation_time: None,
        };
        let controller = RecoveringLifecycleStopController {
            recovered: Some(recovered.clone()),
            authenticated: true,
            stopped: std::sync::Arc::default(),
        };

        let status = resolve_status_read_for_stop(
            temp_dir.path(),
            &controller,
            LifecycleDeadline::after(DAEMON_LIFECYCLE_TIMEOUT)?,
            Err(anyhow::anyhow!(
                "daemon status contained an untrusted profile identity"
            )),
        )?;

        assert_eq!(status, Some(recovered));
        Ok(())
    }

    #[test]
    fn restart_without_status_or_live_default_socket_starts_exactly_once() -> Result<()> {
        for corrupt in [false, true] {
            let temp_dir = tempfile::tempdir()?;
            if corrupt {
                write_test_daemon_status_text(temp_dir.path(), "{not json\n")?;
            }
            let stopped = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
            let started = std::sync::Arc::new(std::sync::Mutex::new(0));

            let (was_running, replacement) = restart_background_server_with_controllers_until(
                temp_dir.path(),
                Path::new("/usr/bin/coven"),
                "new".to_string(),
                &RecoveringLifecycleStopController {
                    recovered: None,
                    authenticated: false,
                    stopped: stopped.clone(),
                },
                &FakeStartController {
                    started: started.clone(),
                    running_after_start: true,
                },
                LifecycleDeadline::after(DAEMON_LIFECYCLE_TIMEOUT)?,
            )?;

            assert!(!was_running, "no-daemon mode corrupt={corrupt}");
            assert!(stopped.lock().unwrap().is_empty());
            assert_eq!(*started.lock().unwrap(), 1);
            assert_eq!(read_status(temp_dir.path())?, Some(replacement));
        }
        Ok(())
    }

    #[test]
    fn restart_never_spawns_from_an_unauthenticated_default_socket_status() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let recovered = DaemonStatus {
            pid: 12345,
            started_at: "untrusted".to_string(),
            socket: daemon_socket_path(temp_dir.path())
                .to_string_lossy()
                .into_owned(),
            process_creation_time: None,
        };
        let started = std::sync::Arc::new(std::sync::Mutex::new(0));

        let error = restart_background_server_with_controllers_until(
            temp_dir.path(),
            Path::new("/usr/bin/coven"),
            "new".to_string(),
            &RecoveringLifecycleStopController {
                recovered: Some(recovered),
                authenticated: false,
                stopped: std::sync::Arc::default(),
            },
            &FakeStartController {
                started: started.clone(),
                running_after_start: true,
            },
            LifecycleDeadline::after(DAEMON_LIFECYCLE_TIMEOUT)?,
        )
        .expect_err("an unauthenticated recovery candidate must fail closed");

        assert!(error.to_string().contains("authenticate"));
        assert_eq!(*started.lock().unwrap(), 0);
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn ensure_background_server_passes_the_canonical_home_through_startup() -> Result<()> {
        struct RecordingStartController {
            homes: std::sync::Arc<std::sync::Mutex<Vec<PathBuf>>>,
        }

        impl DaemonStartController for RecordingStartController {
            fn start_background_server(
                &self,
                coven_home: &Path,
                _current_exe: &Path,
                started_at: String,
            ) -> Result<DaemonStatus> {
                self.homes.lock().unwrap().push(coven_home.to_path_buf());
                let status = DaemonStatus {
                    pid: 54321,
                    started_at,
                    socket: daemon_socket_path(coven_home)
                        .to_string_lossy()
                        .into_owned(),
                    process_creation_time: None,
                };
                write_status(coven_home, &status)?;
                Ok(status)
            }

            fn wait_for_running_daemon(
                &self,
                coven_home: &Path,
                status: &DaemonStatus,
                _deadline: LifecycleDeadline,
            ) -> Result<Option<DaemonStatus>> {
                self.homes.lock().unwrap().push(coven_home.to_path_buf());
                Ok(Some(status.clone()))
            }
        }

        let temp_dir = tempfile::tempdir()?;
        std::fs::create_dir(temp_dir.path().join("nested"))?;
        let selected_home = temp_dir.path().join("nested").join("..");
        let canonical_home = std::fs::canonicalize(temp_dir.path())?;
        let homes = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));

        ensure_background_server_with_controllers(
            &selected_home,
            Path::new("/usr/bin/coven"),
            "2026-04-27T10:00:00Z".to_owned(),
            &FakeStopController {
                pid_alive: false,
                exited_after_signal: false,
                signal_error: None,
                verified_daemon: false,
                signaled: std::sync::Arc::default(),
            },
            &RecordingStartController {
                homes: homes.clone(),
            },
        )?;

        assert_eq!(
            *homes.lock().unwrap(),
            vec![canonical_home.clone(), canonical_home]
        );
        Ok(())
    }

    #[test]
    fn ensure_background_server_reuses_verified_running_daemon() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let status = DaemonStatus {
            pid: 12345,
            started_at: "2026-04-27T10:00:00Z".to_string(),
            socket: test_daemon_status_socket(temp_dir.path()),
            process_creation_time: None,
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
        let canonical_home = std::fs::canonicalize(temp_dir.path())?;
        let status = DaemonStatus {
            pid: std::process::id(),
            started_at: "2026-04-27T10:00:00Z".to_string(),
            socket: daemon_socket_path(&canonical_home)
                .to_string_lossy()
                .into_owned(),
            process_creation_time: None,
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
        let canonical_home = std::fs::canonicalize(temp_dir.path())?;
        let recovered = DaemonStatus {
            pid: std::process::id(),
            started_at: "2026-04-27T10:00:00Z".to_string(),
            socket: daemon_socket_path(&canonical_home)
                .to_string_lossy()
                .into_owned(),
            process_creation_time: None,
        };
        let stale = DaemonStatus {
            // Keep this within the range Linux `kill -0` treats as a plain
            // PID; u32::MAX can be interpreted as -1 by the shell utility.
            pid: 999_999,
            started_at: "2026-04-27T09:00:00Z".to_string(),
            socket: daemon_socket_path(&canonical_home)
                .to_string_lossy()
                .into_owned(),
            process_creation_time: None,
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

    #[test]
    fn lifecycle_preserves_status_when_pid_liveness_is_ambiguous() -> Result<()> {
        struct AmbiguousPidController;

        impl DaemonStopController for AmbiguousPidController {
            fn stop_verified_daemon(
                &self,
                _coven_home: &Path,
                _status: &DaemonStatus,
                _deadline: LifecycleDeadline,
            ) -> Result<VerifiedStopOutcome> {
                panic!("status inspection must not stop a daemon")
            }

            fn recorded_process_state(
                &self,
                _status: &DaemonStatus,
            ) -> Result<RecordedProcessState> {
                anyhow::bail!("process identity could not be inspected")
            }

            fn status_matches_running_daemon(
                &self,
                _coven_home: &Path,
                _status: &DaemonStatus,
                _deadline: LifecycleDeadline,
            ) -> Result<bool> {
                Ok(false)
            }
        }

        let temp_dir = tempfile::tempdir()?;
        let status = DaemonStatus {
            pid: 12345,
            started_at: "2026-04-27T10:00:00Z".to_owned(),
            socket: test_daemon_status_socket(temp_dir.path()),
            process_creation_time: None,
        };
        write_status(temp_dir.path(), &status)?;

        let error =
            background_server_status_with_controller(temp_dir.path(), &AmbiguousPidController)
                .expect_err("ambiguous PID liveness must fail closed");
        assert!(error
            .to_string()
            .contains("could not determine whether Coven daemon pid 12345 is alive"));
        assert_eq!(read_status(temp_dir.path())?, Some(status));
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
        fn stop_verified_daemon(
            &self,
            _coven_home: &Path,
            _status: &DaemonStatus,
            _deadline: LifecycleDeadline,
        ) -> Result<VerifiedStopOutcome> {
            if !self.verified_daemon {
                return Ok(VerifiedStopOutcome::Unverified);
            }
            *self.signaled.lock().unwrap() += 1;
            match &self.signal_error {
                Some(_) if !self.pid_alive => Ok(VerifiedStopOutcome::Exited),
                Some(error) => anyhow::bail!(error.clone()),
                None if self.exited_after_signal => Ok(VerifiedStopOutcome::Exited),
                None => Ok(VerifiedStopOutcome::TimedOut),
            }
        }

        fn recorded_process_state(&self, _status: &DaemonStatus) -> Result<RecordedProcessState> {
            Ok(if self.pid_alive {
                RecordedProcessState::Matching
            } else {
                RecordedProcessState::Gone
            })
        }

        fn status_matches_running_daemon(
            &self,
            _coven_home: &Path,
            _status: &DaemonStatus,
            _deadline: LifecycleDeadline,
        ) -> Result<bool> {
            Ok(self.verified_daemon)
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
                socket: test_daemon_status_socket(coven_home),
                process_creation_time: None,
            };
            write_status(coven_home, &status)?;
            Ok(status)
        }

        fn wait_for_running_daemon(
            &self,
            _coven_home: &Path,
            status: &DaemonStatus,
            _deadline: LifecycleDeadline,
        ) -> Result<Option<DaemonStatus>> {
            Ok(self.running_after_start.then(|| status.clone()))
        }
    }

    #[test]
    fn managed_start_returns_the_authenticated_child_identity() -> Result<()> {
        struct VerifiedIdentityStart {
            health: DaemonStatus,
        }

        impl DaemonStartController for VerifiedIdentityStart {
            fn start_background_server(
                &self,
                _coven_home: &Path,
                _current_exe: &Path,
                started_at: String,
            ) -> Result<DaemonStatus> {
                Ok(DaemonStatus {
                    pid: self.health.pid,
                    started_at,
                    socket: self.health.socket.clone(),
                    process_creation_time: None,
                })
            }

            fn wait_for_running_daemon(
                &self,
                _coven_home: &Path,
                _status: &DaemonStatus,
                _deadline: LifecycleDeadline,
            ) -> Result<Option<DaemonStatus>> {
                Ok(Some(self.health.clone()))
            }
        }

        let temp_dir = tempfile::tempdir()?;
        let health = DaemonStatus {
            pid: 54321,
            started_at: "2026-08-16T15:30:00Z".to_owned(),
            socket: daemon_startup_status_socket(temp_dir.path())?,
            process_creation_time: Some(WindowsProcessCreationTime::new(134_157_822_123_456_789)?),
        };
        let ensured = ensure_background_server_with_controllers(
            temp_dir.path(),
            Path::new("/usr/local/bin/coven"),
            health.started_at.clone(),
            &FakeStopController {
                pid_alive: false,
                exited_after_signal: true,
                signal_error: None,
                verified_daemon: false,
                signaled: std::sync::Arc::default(),
            },
            &VerifiedIdentityStart {
                health: health.clone(),
            },
        )?;

        assert_eq!(ensured, health);
        Ok(())
    }

    #[test]
    fn builds_background_server_spawn_spec() {
        let spec = background_server_spec(
            Path::new("/usr/local/bin/coven"),
            Path::new("/tmp/coven-home"),
            "2026-08-16T15:30:00.000000000Z",
        );

        assert_eq!(spec.program, PathBuf::from("/usr/local/bin/coven"));
        assert_eq!(
            spec.args,
            vec![
                "daemon".to_string(),
                "serve".to_string(),
                "--managed-started-at".to_string(),
                "2026-08-16T15:30:00.000000000Z".to_string(),
            ]
        );
        assert_eq!(spec.coven_home, PathBuf::from("/tmp/coven-home"));
    }

    #[cfg(unix)]
    #[test]
    fn managed_launcher_rejects_non_utf8_home_before_spawn_or_artifacts() -> Result<()> {
        use std::{cell::Cell, ffi::OsString, os::unix::ffi::OsStringExt};

        let test_root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .expect("workspace root")
            .join("c");
        std::fs::create_dir_all(&test_root)?;
        let root = tempfile::tempdir_in(test_root)?;
        let coven_home = root
            .path()
            .join(OsString::from_vec(b"non-utf8-\xff".to_vec()));
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::fs::PermissionsExt;

            std::fs::create_dir(&coven_home)?;
            std::fs::set_permissions(&coven_home, std::fs::Permissions::from_mode(0o700))?;
        }
        let spawned = Cell::new(false);

        let error = start_background_server_with_spawn(
            &coven_home,
            Path::new("/usr/local/bin/coven"),
            "2026-08-16T15:30:00Z".to_owned(),
            |_| {
                spawned.set(true);
                Ok(4242)
            },
        )
        .expect_err("non-UTF-8 lifecycle state must be rejected before child launch");

        assert!(
            !spawned.get(),
            "the daemon child launch must not be attempted"
        );
        assert!(
            error.to_string().contains("valid UTF-8"),
            "error must explain the JSON path requirement: {error:#}"
        );
        assert!(!daemon_socket_path(&coven_home).exists());
        assert!(!daemon_status_path(&coven_home).exists());
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn direct_serve_rejects_non_utf8_home_before_lifecycle_state() -> Result<()> {
        use std::{ffi::OsString, os::unix::ffi::OsStringExt};

        let test_root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .expect("workspace root")
            .join("c");
        std::fs::create_dir_all(&test_root)?;
        let root = tempfile::tempdir_in(test_root)?;
        let coven_home = root
            .path()
            .join(OsString::from_vec(b"serve-non-utf8-\xff".to_vec()));

        let error = serve_forever(&coven_home, "2026-08-16T15:30:00Z".to_owned(), None, &[])
            .expect_err("direct serve must reject non-UTF-8 lifecycle state before startup");

        assert!(
            error.to_string().contains("valid UTF-8"),
            "error must explain the JSON path requirement: {error:#}"
        );
        assert!(!daemon_socket_path(&coven_home).exists());
        assert!(!daemon_status_path(&coven_home).exists());
        assert!(!daemon_lifecycle_lock_path(&coven_home).exists());
        assert!(!daemon_serve_lock_path(&coven_home).exists());
        assert!(!coven_home.join("coven.sqlite3").exists());
        Ok(())
    }

    #[test]
    fn managed_launcher_never_overwrites_status_already_published_by_child() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let started_at = "2026-08-16T15:30:00.000000000Z";
        let socket = daemon_startup_status_socket(temp_dir.path())?;

        let launched = start_background_server_with_spawn(
            temp_dir.path(),
            Path::new("/usr/local/bin/coven"),
            started_at.to_owned(),
            |_| {
                std::fs::write(
                    daemon_status_path(temp_dir.path()),
                    serde_json::to_vec(&serde_json::json!({
                        "pid": 4242,
                        "startedAt": started_at,
                        "socket": socket,
                        "_publisher": "child",
                    }))?,
                )?;
                Ok(4242)
            },
        )?;

        assert_eq!(launched.pid, 4242);
        assert_eq!(launched.started_at, started_at);
        let serialized = std::fs::read_to_string(daemon_status_path(temp_dir.path()))?;
        assert!(
            serialized.contains(r#""_publisher":"child""#),
            "the launcher republished daemon.json after the child: {serialized}"
        );
        Ok(())
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
            process_creation_time: None,
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
    fn serves_memory_overview_over_unix_socket() -> Result<()> {
        use std::io::{Read, Write};
        use std::net::Shutdown;
        use std::os::unix::net::UnixStream;
        use std::thread;

        let temp_dir = tempfile::tempdir()?;
        let memory_dir = temp_dir.path().join("memory").join("sage");
        std::fs::create_dir_all(&memory_dir)?;
        std::fs::write(memory_dir.join("notes.md"), "Durable fact.")?;
        let listener = bind_api_socket(temp_dir.path())?;
        let home = temp_dir.path().to_path_buf();
        let runtime = LiveSessionRuntime::default();
        let server = thread::spawn(move || serve_next_connection(&listener, &home, None, &runtime));

        let mut stream = UnixStream::connect(daemon_socket_path(temp_dir.path()))?;
        stream.write_all(b"GET /api/v1/memory/overview HTTP/1.1\r\nHost: coven\r\n\r\n")?;
        stream.shutdown(Shutdown::Write)?;
        let mut response = String::new();
        stream.read_to_string(&mut response)?;
        server.join().expect("server thread panicked")?;

        assert!(response.starts_with("HTTP/1.1 200 OK"), "got: {response}");
        assert!(response.contains(r#""entries":1"#), "got: {response}");
        assert!(response.contains(r#""detail":true"#), "got: {response}");
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
                execution_binding: None,
                labels: Vec::new(),
                visibility: "private".to_string(),
                external: false,
                transcript_path: None,
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
            execution_binding: None,
            labels: Vec::new(),
            visibility: "private".to_string(),
            external: false,
            transcript_path: None,
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
        use interprocess::{
            local_socket::{prelude::*, GenericNamespaced, ListenerOptions},
            os::windows::local_socket::ListenerOptionsExt,
        };
        use std::thread;

        let temp_dir = tempfile::tempdir()?;
        let pipe_name = windows_pipe_name(temp_dir.path())?;

        let name = pipe_name
            .clone()
            .to_ns_name::<GenericNamespaced>()
            .expect("pipe name");
        let listener = ListenerOptions::new()
            .name(name)
            .security_descriptor(owner_only_pipe_security_descriptor()?)
            .create_sync()
            .expect("bind pipe");

        let server_pid = std::process::id();
        let server_creation_time = coven_client::windows_process_creation_time(server_pid)
            .map_err(anyhow::Error::new)?
            .context("test named-pipe server process was not live")?;
        let status = DaemonStatus {
            pid: server_pid,
            started_at: "2026-04-27T10:00:00Z".to_string(),
            socket: pipe_name.clone(),
            process_creation_time: Some(WindowsProcessCreationTime::new(server_creation_time)?),
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
                    HostGuard::Disabled,
                )?;
            }
            Ok::<_, anyhow::Error>(())
        });

        let probed_status =
            daemon_status_from_windows_pipe(&pipe_name)?.expect("probe running daemon status");
        assert_eq!(probed_status.pid, server_pid);
        assert_eq!(
            probed_status
                .process_creation_time
                .map(WindowsProcessCreationTime::get),
            Some(server_creation_time)
        );

        let (status_code, response) = coven_client::probe_windows_daemon_health(
            &pipe_name,
            // Windows CI runners can take longer than one second to schedule
            // the server thread while the Rust suite is busy.
            Duration::from_secs(5),
        )
        .map_err(anyhow::Error::new)?
        .expect("connect to owner-safe daemon pipe");
        assert_eq!(status_code, 200);
        let response = String::from_utf8(response)?;
        assert!(response.contains("\"apiVersion\""), "got: {response}");
        let response: serde_json::Value = serde_json::from_str(&response)?;
        assert_eq!(response["daemon"]["pid"], server_pid);
        assert_eq!(
            response["daemon"]["processCreationTime"],
            server_creation_time.to_string()
        );
        server.join().expect("server thread")?;
        Ok(())
    }

    #[cfg(windows)]
    #[test]
    fn shared_windows_probe_deadline_bounds_a_busy_pipe() -> Result<()> {
        use interprocess::{
            local_socket::{prelude::*, GenericNamespaced, ListenerOptions, Stream},
            os::windows::local_socket::ListenerOptionsExt,
        };

        let temp_dir = tempfile::tempdir()?;
        let pipe_name = windows_pipe_name(temp_dir.path())?;
        let name = pipe_name
            .clone()
            .to_ns_name::<GenericNamespaced>()
            .expect("pipe name");
        let listener = ListenerOptions::new()
            .name(name)
            .security_descriptor(owner_only_pipe_security_descriptor()?)
            .create_sync()
            .expect("bind pipe");
        let client_name = pipe_name
            .clone()
            .to_ns_name::<GenericNamespaced>()
            .expect("client pipe name");
        let _busy_client = Stream::connect(client_name).expect("occupy pipe instance");

        let started = Instant::now();
        let _ = coven_client::probe_windows_daemon_health(&pipe_name, Duration::from_millis(50));

        assert!(
            started.elapsed() < Duration::from_secs(2),
            "a busy named pipe must not invoke an infinite WaitNamedPipeW"
        );
        drop(listener);
        Ok(())
    }

    #[cfg(windows)]
    #[test]
    fn windows_discovery_retries_a_busy_pipe_until_an_owner_only_instance_is_available(
    ) -> Result<()> {
        use coven_client::DaemonEndpoint;
        use interprocess::{
            local_socket::{prelude::*, GenericNamespaced, ListenerOptions, Stream},
            os::windows::local_socket::ListenerOptionsExt,
            TryClone,
        };
        use std::sync::mpsc;

        let temp_dir = tempfile::tempdir()?;
        let pipe_name = windows_pipe_name(temp_dir.path())?;
        let name = pipe_name
            .clone()
            .to_ns_name::<GenericNamespaced>()
            .expect("busy pipe name");
        let listener = ListenerOptions::new()
            .name(name)
            .security_descriptor(owner_only_pipe_security_descriptor()?)
            .create_sync()
            .expect("bind busy pipe");
        let client_name = pipe_name
            .clone()
            .to_ns_name::<GenericNamespaced>()
            .expect("busy client pipe name");
        let busy_client = Stream::connect(client_name).expect("occupy pipe instance");
        let descriptor = owner_only_pipe_security_descriptor()?;
        let replacement_name = pipe_name
            .to_ns_name::<GenericNamespaced>()
            .expect("replacement pipe name");
        let (replacement_ready_tx, replacement_ready_rx) = mpsc::sync_channel(0);
        let (release_tx, release_rx) = mpsc::sync_channel(0);
        let replacement = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            drop(busy_client);
            drop(listener);
            let deadline = Instant::now() + Duration::from_secs(2);
            let replacement = loop {
                match ListenerOptions::new()
                    .name(replacement_name.clone())
                    .security_descriptor(
                        descriptor
                            .try_clone()
                            .expect("clone owner-only pipe descriptor"),
                    )
                    .create_sync()
                {
                    Ok(listener) => break listener,
                    Err(error) if Instant::now() < deadline => {
                        let _ = error;
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(error) => panic!("replace released busy pipe: {error}"),
                }
            };
            replacement_ready_tx
                .send(())
                .expect("report replacement pipe");
            release_rx.recv().expect("release replacement pipe");
            drop(replacement);
        });

        let started = Instant::now();
        let endpoint = DaemonEndpoint::discover(temp_dir.path()).map_err(anyhow::Error::new);
        let discovery_elapsed = started.elapsed();
        replacement_ready_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("replacement pipe became available");
        let owner_local = endpoint
            .as_ref()
            .is_ok_and(|endpoint| endpoint.is_owner_local());
        release_tx.send(()).expect("release replacement listener");
        replacement.join().expect("replacement pipe thread");
        let _endpoint = endpoint?;
        assert!(owner_local);
        assert!(
            discovery_elapsed >= Duration::from_millis(25),
            "discovery did not exercise the busy-instance retry"
        );
        assert!(discovery_elapsed < Duration::from_secs(2));
        Ok(())
    }

    #[cfg(windows)]
    #[test]
    fn windows_discovery_fails_a_persistently_busy_pipe_within_its_budget() -> Result<()> {
        use coven_client::DaemonEndpoint;
        use interprocess::{
            local_socket::{prelude::*, GenericNamespaced, ListenerOptions, Stream},
            os::windows::local_socket::ListenerOptionsExt,
        };

        let temp_dir = tempfile::tempdir()?;
        let pipe_name = windows_pipe_name(temp_dir.path())?;
        let name = pipe_name
            .clone()
            .to_ns_name::<GenericNamespaced>()
            .expect("busy pipe name");
        let listener = ListenerOptions::new()
            .name(name)
            .security_descriptor(owner_only_pipe_security_descriptor()?)
            .create_sync()
            .expect("bind busy pipe");
        let client_name = pipe_name
            .to_ns_name::<GenericNamespaced>()
            .expect("busy client pipe name");
        let busy_client = Stream::connect(client_name).expect("occupy pipe instance");

        let started = Instant::now();
        let result = DaemonEndpoint::discover(temp_dir.path());

        assert!(result.is_err(), "persistently busy pipe was discovered");
        assert!(
            started.elapsed() >= Duration::from_millis(250),
            "busy discovery failed fast instead of sharing its retry deadline"
        );
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "busy discovery exceeded its two-second budget"
        );
        drop(busy_client);
        drop(listener);
        Ok(())
    }

    #[cfg(windows)]
    #[test]
    fn owner_only_pipe_security_descriptor_has_explicit_token_user_owner() -> Result<()> {
        use interprocess::os::windows::security_descriptor::AsSecurityDescriptorExt;
        use windows_sys::Win32::Security::EqualSid;

        let descriptor = owner_only_pipe_security_descriptor()?;
        let (owner, defaulted) = descriptor.owner()?;
        let current_user = current_windows_user_sid()?;

        assert!(!owner.is_null());
        assert!(!defaulted);
        assert_ne!(
            unsafe { EqualSid(owner.cast_mut(), current_user.as_ptr()) },
            0
        );
        assert!(descriptor.dacl()?.is_some());
        Ok(())
    }

    #[cfg(windows)]
    #[test]
    fn client_accepts_object_manager_mapped_owner_only_pipe_and_status_acls() -> Result<()> {
        use coven_client::DaemonEndpoint;
        use interprocess::{
            local_socket::{prelude::*, GenericNamespaced, ListenerOptions},
            os::windows::local_socket::ListenerOptionsExt,
        };

        let temp_dir = tempfile::tempdir()?;
        ensure_private_coven_home(temp_dir.path())?;
        let pipe_name = windows_pipe_name(temp_dir.path())?;
        let name = pipe_name
            .clone()
            .to_ns_name::<GenericNamespaced>()
            .expect("daemon pipe name");
        let _listener = ListenerOptions::new()
            .name(name)
            .security_descriptor(owner_only_pipe_security_descriptor()?)
            .create_sync()
            .expect("bind owner-only pipe");
        let status = DaemonStatus {
            pid: std::process::id(),
            started_at: "2026-08-16T12:00:00Z".to_owned(),
            socket: pipe_name,
            process_creation_time: None,
        };
        write_status(temp_dir.path(), &status)?;

        let serialized = coven_client::read_windows_daemon_status_for_lifecycle(temp_dir.path())
            .map_err(anyhow::Error::new)?
            .context("read hardened status")?;
        assert_eq!(parse_daemon_status(&serialized)?, status);
        DaemonEndpoint::discover(temp_dir.path()).map_err(anyhow::Error::new)?;
        Ok(())
    }

    #[cfg(windows)]
    #[test]
    fn discovers_same_profile_legacy_pipe_from_an_inherited_acl_status_file() -> Result<()> {
        use coven_client::{DaemonClient, DaemonEndpoint};
        use interprocess::{
            local_socket::{prelude::*, GenericNamespaced, ListenerOptions},
            os::windows::local_socket::ListenerOptionsExt,
        };
        use std::{
            hash::{DefaultHasher, Hash, Hasher},
            thread,
        };

        let temp_dir = tempfile::tempdir()?;
        let mut hasher = DefaultHasher::new();
        temp_dir.path().to_string_lossy().hash(&mut hasher);
        let pipe_name = format!("coven-daemon-{:016x}.sock", hasher.finish());
        let name = pipe_name
            .clone()
            .to_ns_name::<GenericNamespaced>()
            .expect("legacy pipe name");
        let listener = ListenerOptions::new()
            .name(name)
            .security_descriptor(owner_only_pipe_security_descriptor()?)
            .create_sync()
            .expect("bind protected legacy pipe");
        let status = DaemonStatus {
            pid: 12345,
            started_at: "2026-04-27T10:00:00Z".to_string(),
            socket: pipe_name,
            process_creation_time: None,
        };
        write_inherited_windows_status(temp_dir.path(), serde_json::to_vec(&status)?)?;

        let home = temp_dir.path().to_path_buf();
        let server_status = status.clone();
        let server = thread::spawn(move || {
            // Legacy discovery authenticates status with health, opens a
            // metadata-only connection to validate the pipe ACL, then the
            // caller performs its own health negotiation.
            for serves_health in [true, false, true] {
                let conn = listener.incoming().next().expect("accept").expect("stream");
                if serves_health {
                    handle_http_stream(
                        &conn,
                        &conn,
                        &home,
                        Some(server_status.clone()),
                        &LiveSessionRuntime::default(),
                        None,
                        HostGuard::Disabled,
                    )?;
                }
            }
            Ok::<_, anyhow::Error>(())
        });

        let endpoint = DaemonEndpoint::discover(temp_dir.path()).map_err(anyhow::Error::new)?;
        let health = DaemonClient::new(endpoint)
            .health()
            .map_err(anyhow::Error::new)?;
        assert_eq!(health.api_version, "coven.daemon.v1");
        server.join().expect("server thread")?;
        Ok(())
    }

    #[cfg(windows)]
    #[test]
    fn inherited_acl_status_rejects_cross_profile_and_arbitrary_pipe_redirection() -> Result<()> {
        use coven_client::DaemonEndpoint;
        use std::hash::{DefaultHasher, Hash, Hasher};

        let temp_dir = tempfile::tempdir()?;
        let other_home = temp_dir.path().join("other-profile");
        let mut hasher = DefaultHasher::new();
        other_home.to_string_lossy().hash(&mut hasher);
        let other_profile_pipe = format!("coven-daemon-{:016x}.sock", hasher.finish());

        for redirected in [other_profile_pipe.as_str(), "other-daemon.sock"] {
            let status = DaemonStatus {
                pid: 12345,
                started_at: "2026-04-27T10:00:00Z".to_string(),
                socket: redirected.to_owned(),
                process_creation_time: None,
            };
            std::fs::write(
                daemon_status_path(temp_dir.path()),
                serde_json::to_vec(&status)?,
            )?;

            assert!(
                DaemonEndpoint::discover(temp_dir.path()).is_err(),
                "inherited status redirected discovery to {redirected}"
            );
        }
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
                process_creation_time: None,
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
                process_creation_time: None,
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

    #[test]
    fn append_daemon_recovery_log_creates_and_appends() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        append_daemon_recovery_log(temp_dir.path(), "first event");
        append_daemon_recovery_log(temp_dir.path(), "second event");
        let log = std::fs::read_to_string(daemon_recovery_log_path(temp_dir.path()))?;
        let mut entries = log.split_inclusive('\n');
        for expected in ["first event", "second event"] {
            let entry = entries.next().expect("expected recovery log entry");
            assert!(
                entry.starts_with('['),
                "entry should start with a timestamp"
            );
            let (_, message) = entry
                .split_once("] ")
                .expect("entry should separate timestamp and message");
            assert_eq!(message, format!("{expected}\n"));
        }
        assert!(entries.next().is_none(), "unexpected recovery log entry");
        Ok(())
    }

    #[test]
    fn append_daemon_recovery_log_truncates_oversized_ascii_entry() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let message = "x".repeat(DAEMON_RECOVERY_LOG_MAX_BYTES as usize + 1024);

        append_daemon_recovery_log(temp_dir.path(), &message);

        let log = std::fs::read(daemon_recovery_log_path(temp_dir.path()))?;
        assert!(
            log.len() <= DAEMON_RECOVERY_LOG_MAX_BYTES as usize,
            "active recovery log exceeded its byte bound: {}",
            log.len()
        );
        assert!(
            log.ends_with(b"... [truncated]\n"),
            "oversized entry should end with a truncation marker"
        );
        Ok(())
    }

    #[test]
    fn append_daemon_recovery_log_truncates_multibyte_entry_on_char_boundary() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let message = "🧙".repeat(DAEMON_RECOVERY_LOG_MAX_BYTES as usize / "🧙".len() + 1024);

        append_daemon_recovery_log(temp_dir.path(), &message);

        let log = std::fs::read_to_string(daemon_recovery_log_path(temp_dir.path()))?;
        assert!(log.len() <= DAEMON_RECOVERY_LOG_MAX_BYTES as usize);
        assert!(log.ends_with("... [truncated]\n"));
        Ok(())
    }

    #[test]
    fn maintenance_failures_below_watermark_log_without_creating_store() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;

        record_store_maintenance_failure_with_free_disk_check(
            temp_dir.path(),
            "store maintenance pass failed: failed to read /private/home",
            Ok(crate::store::MAINTENANCE_MIN_FREE_DISK_BYTES - 1),
        );

        let log = std::fs::read_to_string(daemon_recovery_log_path(temp_dir.path()))?;
        assert!(log.contains("store maintenance pass failed"));
        assert!(!temp_dir.path().join("coven.sqlite3").exists());
        let health = crate::store::cached_storage_health_with_free_disk(
            temp_dir.path(),
            crate::store::MAINTENANCE_MIN_FREE_DISK_BYTES - 1,
            None,
        )?;
        // Live low disk outranks the degraded snapshot, and the degraded
        // reason is still reported alongside the critical status.
        assert_eq!(health.status, "critical");
        assert!(health.maintenance_blocked);
        assert_eq!(
            health.free_disk_bytes,
            crate::store::MAINTENANCE_MIN_FREE_DISK_BYTES - 1
        );
        assert_eq!(
            health.last_maintenance_error.as_deref(),
            Some("storage health unavailable")
        );
        Ok(())
    }

    #[test]
    fn blocked_maintenance_report_skips_store_refresh() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let report = crate::store::ScheduledMaintenanceReport {
            raw_artifacts_pruned: 0,
            events_pruned: 0,
            checkpoint_ran: false,
            blocked_by_free_disk: true,
        };

        refresh_storage_health_after_maintenance(temp_dir.path(), &report);

        for file_name in ["coven.sqlite3", "coven.sqlite3-wal", "coven.sqlite3-shm"] {
            assert!(
                !temp_dir.path().join(file_name).exists(),
                "blocked maintenance must not create {file_name}"
            );
        }
        Ok(())
    }

    #[test]
    fn maintenance_failure_preserves_last_good_health_until_successful_refresh() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let home = temp_dir.path();
        let store_path = home.join("coven.sqlite3");
        let conn = crate::store::open_store(&store_path)?;
        for (key, value) in [
            ("maintenance_last_prune_at", "2026-08-05T12:00:00Z"),
            ("maintenance_last_checkpoint_at", "2026-08-05T12:01:00Z"),
        ] {
            conn.execute(
                "INSERT INTO store_meta(key, value) VALUES(?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                rusqlite::params![key, value],
            )?;
        }
        let writer = crate::event_writer::EventWriterHealth {
            state: "pressured".to_string(),
            queued_events: 7,
            queued_bytes: 8192,
            capacity_bytes: 2 * 1024 * 1024,
            dropped_output_events: 1,
            dropped_output_bytes: 512,
            connection_opens: 1,
            transactions: 3,
            committed_events: 12,
            last_error: None,
        };
        crate::store::refresh_storage_health_snapshot_from_connection_with_free_disk(
            home,
            &conn,
            crate::store::MAINTENANCE_MIN_FREE_DISK_BYTES,
            Some(&writer),
        )?;
        let before = crate::store::cached_storage_health_with_free_disk(
            home,
            crate::store::MAINTENANCE_MIN_FREE_DISK_BYTES,
            Some(&writer),
        )?;
        drop(conn);

        record_store_maintenance_failure_with_free_disk_check(
            home,
            "store maintenance pass failed: failed to read /private/home",
            Ok(crate::store::MAINTENANCE_MIN_FREE_DISK_BYTES),
        );

        let degraded = crate::store::cached_storage_health_with_free_disk(
            home,
            crate::store::MAINTENANCE_MIN_FREE_DISK_BYTES,
            None,
        )?;
        assert_eq!(degraded.status, "degraded");
        assert_eq!(degraded.database_bytes, before.database_bytes);
        assert_eq!(degraded.last_prune_at, before.last_prune_at);
        assert_eq!(degraded.last_checkpoint_at, before.last_checkpoint_at);
        assert_eq!(degraded.writer_backlog_events, 7);
        assert_eq!(degraded.writer_backlog_bytes, 8192);
        assert_eq!(
            degraded.last_maintenance_error.as_deref(),
            Some("maintenance pass failed")
        );

        crate::store::run_scheduled_maintenance(home, "2026-08-05T12:02:00Z")?;
        let conn = crate::store::open_initialized_store(&store_path)?;
        crate::store::refresh_storage_health_snapshot_from_connection_with_free_disk(
            home,
            &conn,
            crate::store::MAINTENANCE_MIN_FREE_DISK_BYTES,
            None,
        )?;
        let recovered = crate::store::cached_storage_health_with_free_disk(
            home,
            crate::store::MAINTENANCE_MIN_FREE_DISK_BYTES,
            None,
        )?;
        assert_ne!(recovered.status, "degraded");
        assert!(recovered.last_maintenance_error.is_none());
        Ok(())
    }

    #[test]
    fn recovery_log_rotation_keeps_a_bounded_history() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let path = daemon_recovery_log_path(temp_dir.path());
        std::fs::write(&path, "12345678")?;

        rotate_recovery_log(&path, 4, 10, 2);
        assert!(!path.exists());
        assert_eq!(
            std::fs::read_to_string(format!("{}.1", path.display()))?,
            "12345678"
        );

        std::fs::write(&path, "abcdefgh")?;
        rotate_recovery_log(&path, 4, 10, 2);
        assert_eq!(
            std::fs::read_to_string(format!("{}.1", path.display()))?,
            "abcdefgh"
        );
        assert_eq!(
            std::fs::read_to_string(format!("{}.2", path.display()))?,
            "12345678"
        );

        // A partial prior rotation can leave an older archive without its
        // newer neighbor. Keep the surviving history rather than deleting it
        // merely because there is no source to shift into its slot.
        std::fs::remove_file(format!("{}.1", path.display()))?;
        std::fs::write(&path, "ijklmnop")?;
        rotate_recovery_log(&path, 4, 10, 2);
        assert_eq!(
            std::fs::read_to_string(format!("{}.1", path.display()))?,
            "ijklmnop"
        );
        assert_eq!(
            std::fs::read_to_string(format!("{}.2", path.display()))?,
            "12345678"
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
            process_creation_time: None,
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
