//! Daemon-owned process boundary for mobile authority. The native gateway and
//! every authority thread live in the worker; this parent only proxies existing
//! owner-control routes after the daemon's existing authorization/chat gates.
//!
//! IMPORTANT: callers must keep the daemon alive in its stopping state while
//! retry_retained() reports false. Returning an error and then exiting the daemon
//! would make the Unix stop controller falsely report a stopped worker.
use anyhow::{bail, ensure, Context, Result};
use std::collections::HashMap;
use std::io::{BufReader, BufWriter};
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, LazyLock, Mutex, Weak};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

#[path = "wire.rs"]
mod wire;
use wire::{Frame, Header, PROTOCOL};

const QUEUE: usize = 16;
const MAX_PENDING: usize = 32;
const POLL: Duration = Duration::from_millis(5);
const STARTUP_BUDGET: Duration = Duration::from_millis(1600);
const GRACE: Duration = Duration::from_millis(500);
const KILL_AND_JOIN_RESERVE: Duration = Duration::from_millis(300);
const STANDALONE_SHUTDOWN_BUDGET: Duration = Duration::from_millis(1400);
const REQUEST_WAIT: Duration = Duration::from_secs(15);
const SOURCE: &str = env!("COVEN_BUILD_COMMIT");

enum CallFailure {
    Unavailable,
    Native(String),
}
type Pending =
    Arc<Mutex<HashMap<u64, mpsc::SyncSender<Result<crate::api::ApiResponse, CallFailure>>>>>;
static ACTIVE: LazyLock<Mutex<Option<Weak<Client>>>> = LazyLock::new(|| Mutex::new(None));
// This is retained ownership, not a successful shutdown. The daemon MUST remain
// alive and poll retry_retained() after an explicit shutdown failure.
static RETAINED: LazyLock<Mutex<Vec<Owned>>> = LazyLock::new(|| Mutex::new(Vec::new()));

struct Client {
    admitting: Arc<AtomicBool>,
    next_id: AtomicU64,
    pending: Pending,
    requests: mpsc::SyncSender<Frame>,
    signals: Arc<Signals>,
}

impl Client {
    fn call(
        &self,
        method: &str,
        path: &str,
        body: Option<&str>,
    ) -> Result<crate::api::ApiResponse> {
        if !self.admitting.load(Ordering::Acquire)
            || self.signals.fatal.load(Ordering::Acquire)
            || self.signals.halt.load(Ordering::Acquire)
        {
            return unavailable();
        }
        ensure!(
            body.is_none_or(|body| body.len() <= wire::MAX_BODY),
            "mobile control body exceeds daemon socket limit"
        );
        let Some(id) = allocate_id(&self.next_id) else {
            return unavailable();
        };
        let frame = Frame {
            header: Header::Request {
                id,
                method: method.into(),
                path: path.into(),
                has_body: body.is_some(),
            },
            body: body.unwrap_or("").as_bytes().to_vec(),
        };
        wire::validate_frame(&frame)?;
        let (send, receive) = mpsc::sync_channel(1);
        {
            let mut pending = self
                .pending
                .lock()
                .map_err(|_| anyhow::anyhow!("mobile control pending lock poisoned"))?;
            if !self.admitting.load(Ordering::Acquire)
                || self.signals.fatal.load(Ordering::Acquire)
                || self.signals.halt.load(Ordering::Acquire)
                || pending.len() >= MAX_PENDING
            {
                return unavailable();
            }
            pending.insert(id, send);
        }
        if self.requests.try_send(frame).is_err() {
            self.pending
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .remove(&id);
            return unavailable();
        }
        let result = receive.recv_timeout(REQUEST_WAIT);
        self.pending
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&id);
        match result {
            Ok(Ok(response)) => Ok(response),
            Ok(Err(CallFailure::Native(message))) => Err(anyhow::Error::msg(message)),
            _ => unavailable(),
        }
    }
}

fn allocate_id(next: &AtomicU64) -> Option<u64> {
    let mut value = next.load(Ordering::Relaxed);
    loop {
        let following = value.checked_add(1)?;
        match next.compare_exchange_weak(value, following, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return Some(value),
            Err(actual) => value = actual,
        }
    }
}

fn unavailable() -> Result<crate::api::ApiResponse> {
    crate::api::api_error(
        503,
        "daemon_unavailable",
        "Mobile authority is unavailable; the request outcome may be unknown.",
        Some(serde_json::json!({ "outcome": "unknown", "retrySafe": false })),
    )
}

pub(super) fn proxy(
    method: &str,
    path: &str,
    body: Option<&str>,
) -> Option<Result<crate::api::ApiResponse>> {
    if !super::claims_local_control(path) {
        return None;
    }
    let active = ACTIVE
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .clone();
    // An expired worker is unavailable. Falling back to native dispatch here
    // could start new authority work in the parent after containment stopped.
    active.as_ref().map(|weak| match weak.upgrade() {
        Some(client) => client.call(method, path, body),
        None => unavailable(),
    })
}

struct Process {
    child: Child,
    tree: crate::pty_runner::StrictChildProcessTree,
    reaped: bool,
    #[cfg(test)]
    observation_release: Option<std::path::PathBuf>,
    #[cfg(test)]
    observation_reported: bool,
}

impl Process {
    fn terminate_if_unreaped(&mut self) {
        // Every call, including observer-fatal handling and retained retries,
        // checks this under the same mutex that owns exit observation/reaping.
        if !self.reaped {
            self.tree.terminate(&mut self.child);
        }
    }

    fn observe_exit(&mut self) -> std::io::Result<Option<ExitStatus>> {
        if self.reaped {
            return Ok(None);
        }
        #[cfg(test)]
        if let Some(release) = self.observation_release.as_ref() {
            if !self.observation_reported {
                publish_test_observation(
                    &release.with_extension("worker-pid"),
                    self.child.id().to_string().as_bytes(),
                )?;
                self.observation_reported = true;
            }
            if !release.exists() {
                // Withhold observation only. The unchanged real termination
                // path still owns and signals the child/tree; no success or
                // reaped flag is fabricated while the test gate is closed.
                return Ok(None);
            }
        }
        #[cfg(unix)]
        {
            // Keep the zombie leader's PID/PGID reserved until the tree's final
            // signal AND its Drop backstop have been disarmed. Reaping first
            // would allow a delayed retry/Drop to signal a recycled Unix PGID.
            if !crate::pty_runner::poll_child_exit_without_reaping(&self.child)? {
                return Ok(None);
            }
            self.tree.terminate_tree()?;
        }
        let status = self.child.try_wait()?;
        #[cfg(not(unix))]
        if status.is_some() {
            // Windows tree operations use the owned Job handle, never a
            // recyclable numeric group ID. Finish that containment handle too.
            self.tree.terminate_tree()?;
        }
        if status.is_some() {
            self.reaped = true;
        }
        Ok(status)
    }
}

struct Signals {
    halt: AtomicBool,
    stop: AtomicBool,
    fatal: AtomicBool,
    exited: Mutex<Option<ExitStatus>>,
}
impl Default for Signals {
    fn default() -> Self {
        Self {
            halt: AtomicBool::new(false),
            stop: AtomicBool::new(false),
            fatal: AtomicBool::new(false),
            exited: Mutex::new(None),
        }
    }
}

pub struct Handle {
    owned: Option<Owned>,
}

struct Owned {
    process: Arc<Mutex<Process>>,
    signals: Arc<Signals>,
    client: Arc<Client>,
    writer: Option<JoinHandle<()>>,
    reader: Option<JoinHandle<()>>,
    observer: Option<JoinHandle<()>>,
}

impl Owned {
    fn close_admission(&self) {
        self.client.admitting.store(false, Ordering::Release);
        fail_pending(&self.client.pending);
        self.signals.stop.store(true, Ordering::Release);
    }

    fn exited(&self) -> bool {
        self.signals
            .exited
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .is_some()
    }

    fn terminate(&self) {
        let mut process = self
            .process
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        process.terminate_if_unreaped();
    }

    fn observe_once(&self) {
        let result = self
            .process
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .observe_exit();
        if let Ok(Some(status)) = result {
            *self
                .signals
                .exited
                .lock()
                .unwrap_or_else(|error| error.into_inner()) = Some(status);
            // Only observed exit releases the parent's intentional-stop stdin.
            self.signals.halt.store(true, Ordering::Release);
        }
    }

    fn workers_finished(&self) -> bool {
        [&self.writer, &self.reader, &self.observer]
            .into_iter()
            .all(|worker| worker.as_ref().is_none_or(JoinHandle::is_finished))
    }

    fn join_finished(&mut self) -> Result<()> {
        ensure!(
            self.exited() && self.workers_finished(),
            "mobile worker exit/pipe-worker completion unobserved"
        );
        let mut panicked = false;
        for thread in [&mut self.writer, &mut self.reader, &mut self.observer] {
            if let Some(thread) = thread.take() {
                panicked |= thread.join().is_err();
            }
        }
        ensure!(!panicked, "mobile protocol worker panicked");
        Ok(())
    }

    fn shutdown_until(&mut self, deadline: Instant) -> Result<()> {
        self.close_admission();
        let start = Instant::now();
        // Reserve kill/reap/join time from the caller's SAME absolute budget;
        // never renew a two-second allowance after other daemon cleanup.
        let graceful_end =
            (start + GRACE).min(deadline.checked_sub(KILL_AND_JOIN_RESERVE).unwrap_or(start));
        while Instant::now() < graceful_end && !self.exited() {
            self.observe_once();
            std::thread::sleep(POLL);
        }
        if !self.exited() {
            self.terminate();
        }
        while Instant::now() < deadline {
            self.observe_once();
            if self.exited() && self.workers_finished() {
                return self.join_finished();
            }
            std::thread::sleep(POLL);
        }
        self.observe_once();
        if self.exited() && self.workers_finished() {
            return self.join_finished();
        }
        bail!(
            "mobile worker shutdown incomplete after {:?}; daemon must retain ownership and remain stopping",
            start.elapsed()
        )
    }
}

#[cfg(test)]
pub(super) struct Observation {
    signals: Arc<Signals>,
    process: Arc<Mutex<Process>>,
}

#[cfg(test)]
impl Observation {
    pub(super) fn exited(&self) -> bool {
        self.signals.exited.lock().unwrap().is_some()
    }

    pub(super) fn reaped(&self) -> bool {
        self.process.lock().unwrap().reaped
    }

    pub(super) fn exit_code(&self) -> Option<i32> {
        self.signals
            .exited
            .lock()
            .unwrap()
            .as_ref()
            .and_then(ExitStatus::code)
    }
}

impl Handle {
    /// Close mobile admission before the daemon waits on other subsystems.
    /// Cleanup still requires checked exit/reap/join under the shared deadline.
    pub fn request_shutdown(&self) {
        if let Some(owned) = &self.owned {
            owned.close_admission();
        }
    }

    #[cfg(test)]
    pub(super) fn observation(&self) -> Observation {
        let owned = self.owned.as_ref().expect("live worker handle");
        Observation {
            signals: Arc::clone(&owned.signals),
            process: Arc::clone(&owned.process),
        }
    }
    /// Explicit lifecycle result. An error requires the daemon to remain alive,
    /// retaining its PID/socket and all runtime ownership until retry_retained().
    pub fn shutdown(&mut self) -> Result<()> {
        self.shutdown_until(Instant::now() + STANDALONE_SHUTDOWN_BUDGET)
    }

    /// Daemon cleanup must pass its existing absolute stop deadline, measured
    /// when termination is first observed, including prior scheduler/TCP work.
    pub fn shutdown_until(&mut self, deadline: Instant) -> Result<()> {
        let Some(mut owned) = self.owned.take() else {
            return Ok(());
        };
        let result = owned.shutdown_until(deadline);
        if result.is_err() {
            let mut retained = RETAINED.lock().unwrap_or_else(|error| error.into_inner());
            retained.push(owned);
            #[cfg(test)]
            if let Some(owned) = retained.last() {
                let process = owned
                    .process
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                if let Some(release) = process.observation_release.as_ref() {
                    let _ = publish_test_observation(
                        &release.with_extension("retained"),
                        format!("pid={} exit-unobserved\n", process.child.id()).as_bytes(),
                    );
                }
            }
        }
        result
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        if let Err(error) = self.shutdown() {
            eprintln!("coven mobile shutdown incomplete: {error:#}");
        }
    }
}

/// Non-blocking retained-ownership retry for the daemon's stopping loop. A
/// false result forbids publishing stopped/clearing PID or socket/daemon exit.
pub(crate) fn retry_retained() -> bool {
    let mut retained = RETAINED.lock().unwrap_or_else(|error| error.into_inner());
    let mut index = 0;
    while index < retained.len() {
        let owned = &mut retained[index];
        owned.terminate();
        owned.observe_once();
        if owned.exited() && owned.workers_finished() {
            // Joining an already-finished protocol worker cannot detach live
            // authority. Panics still count as failure, and remain diagnostic.
            if let Err(error) = owned.join_finished() {
                eprintln!("coven mobile protocol cleanup failed: {error:#}");
            }
            retained.swap_remove(index);
        } else {
            index += 1;
        }
    }
    retained.is_empty()
}

/// Declare immediately after the daemon's status/socket ShutdownGuard and
/// before any mobile worker can be spawned. Reverse Drop ordering then keeps
/// the daemon/process alive through failed startup, ordinary return and unwind
/// until all retained children are observed exited and protocol workers joined.
pub(crate) struct RetainedOwnerGuard;
impl Drop for RetainedOwnerGuard {
    fn drop(&mut self) {
        while !retry_retained() {
            std::thread::sleep(POLL);
        }
    }
}

/// Startup interruption carries the ONE absolute cleanup deadline into the
/// daemon's error path. It never asserts that the child has actually stopped.
#[derive(Debug)]
struct StartupAbort {
    deadline: Instant,
    interrupted: bool,
}
impl std::fmt::Display for StartupAbort {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(if self.interrupted {
            "mobile startup cancelled during daemon shutdown"
        } else {
            "mobile gateway startup failed"
        })
    }
}
impl std::error::Error for StartupAbort {}

pub(crate) fn startup_cleanup_deadline(error: &anyhow::Error) -> Option<Instant> {
    error
        .downcast_ref::<StartupAbort>()
        .map(|abort| abort.deadline)
}

type StartupShutdown<'a> = Option<&'a dyn Fn() -> Option<Instant>>;

struct StartupWait<'a> {
    startup_deadline: Instant,
    shutdown: StartupShutdown<'a>,
    stop_deadline: Option<Instant>,
}
impl<'a> StartupWait<'a> {
    fn new(shutdown: StartupShutdown<'a>) -> Self {
        Self {
            startup_deadline: Instant::now() + STARTUP_BUDGET,
            shutdown,
            stop_deadline: None,
        }
    }

    fn observe_stop(&mut self) -> Option<Instant> {
        if self.stop_deadline.is_none() {
            self.stop_deadline = self.shutdown.and_then(|predicate| predicate());
        }
        self.stop_deadline
    }

    fn check(&mut self) -> Result<()> {
        if let Some(deadline) = self.observe_stop() {
            return Err(anyhow::Error::new(StartupAbort {
                deadline,
                interrupted: true,
            }));
        }
        ensure!(
            Instant::now() < self.startup_deadline,
            "mobile worker startup timed out"
        );
        Ok(())
    }

    fn receive(&mut self, receiver: &mpsc::Receiver<Frame>, phase: &'static str) -> Result<Frame> {
        loop {
            self.check()?;
            let remaining = self
                .startup_deadline
                .saturating_duration_since(Instant::now());
            match receiver.recv_timeout(remaining.min(POLL)) {
                Ok(frame) => {
                    // Stop wins even when a queued Hello/Ready arrives at the
                    // same instant; never authorize Start after cancellation.
                    self.check()?;
                    return Ok(frame);
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    bail!("mobile worker disconnected during {phase}")
                }
            }
        }
    }

    fn cleanup_deadline(&mut self) -> Instant {
        self.observe_stop()
            .unwrap_or(self.startup_deadline + KILL_AND_JOIN_RESERVE)
    }

    fn contextualize(&self, error: anyhow::Error, deadline: Instant) -> anyhow::Error {
        if error.is::<StartupAbort>() {
            error
        } else {
            error.context(StartupAbort {
                deadline,
                interrupted: self.stop_deadline.is_some(),
            })
        }
    }
}

fn daemon_startup_shutdown() -> Option<Instant> {
    crate::daemon::daemon_termination_requested()
        .then(|| Instant::now() + Duration::from_millis(1800))
}

#[cfg(test)]
pub fn start(home: &Path) -> Result<Option<Handle>> {
    if let Some(target) =
        std::env::var_os("COVEN_TEST_DAEMON_STARTUP_HOLD").map(std::path::PathBuf::from)
    {
        let ready = std::env::var_os("COVEN_TEST_DAEMON_STARTUP_READY")
            .map(std::path::PathBuf::from)
            .context("scoped daemon startup ready path")?;
        let pid = std::env::var_os("COVEN_TEST_DAEMON_STARTUP_WORKER_PID")
            .map(std::path::PathBuf::from)
            .context("scoped daemon startup worker PID path")?;
        return native_fixture::start_daemon_fixture(
            home,
            Some(&target),
            &ready,
            |handle| {
                let owned = handle.owned.as_ref().expect("startup fixture child");
                let process = owned.process.lock().unwrap();
                publish_test_observation(&pid, process.child.id().to_string().as_bytes())
                    .expect("publish startup worker PID");
            },
            &daemon_startup_shutdown,
        );
    }
    native_fixture::start_daemon_fixture(
        home,
        None,
        &home.join(format!("worker-ready-{}", uuid::Uuid::new_v4())),
        |_| {},
        &daemon_startup_shutdown,
    )
}

#[cfg(not(test))]
pub fn start(home: &Path) -> Result<Option<Handle>> {
    let mut command = Command::new(std::env::current_exe()?);
    command
        .arg("mobile-gateway-worker")
        .arg("--protocol")
        .arg(PROTOCOL)
        .arg("--home")
        .arg(home)
        .env("COVEN_HOME", home);
    start_command(command, SOURCE, false)
}

#[cfg(not(test))]
fn start_command(
    command: Command,
    expected_source: &str,
    fixture_preamble: bool,
) -> Result<Option<Handle>> {
    start_command_observing_with_shutdown(
        command,
        expected_source,
        fixture_preamble,
        |_| {},
        Some(&daemon_startup_shutdown),
    )
}

#[cfg(test)]
fn start_command_observing(
    command: Command,
    expected_source: &str,
    fixture_preamble: bool,
    observe: impl FnOnce(&Handle),
) -> Result<Option<Handle>> {
    start_command_observing_with_shutdown(command, expected_source, fixture_preamble, observe, None)
}

fn start_command_observing_with_shutdown(
    mut command: Command,
    expected_source: &str,
    fixture_preamble: bool,
    observe: impl FnOnce(&Handle),
    shutdown: StartupShutdown<'_>,
) -> Result<Option<Handle>> {
    let mut startup_wait = StartupWait::new(shutdown);
    startup_wait.check()?;
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
    let (mut child, tree) = crate::pty_runner::spawn_strict_child_process_tree(&mut command)
        .context("spawn contained mobile worker")?;
    // piped() guarantees these. No fallible operation before owning the child.
    let stdin = child.stdin.take().expect("piped mobile stdin");
    let stdout = child.stdout.take().expect("piped mobile stdout");
    let process = Arc::new(Mutex::new(Process {
        child,
        tree,
        reaped: false,
        #[cfg(test)]
        observation_release: std::env::var_os("COVEN_TEST_MOBILE_OBSERVATION_RELEASE")
            .map(std::path::PathBuf::from),
        #[cfg(test)]
        observation_reported: false,
    }));
    let signals = Arc::new(Signals::default());
    let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
    let (requests, incoming) = mpsc::sync_channel(QUEUE);
    let client = Arc::new(Client {
        admitting: Arc::new(AtomicBool::new(false)),
        next_id: AtomicU64::new(1),
        pending: Arc::clone(&pending),
        requests,
        signals: Arc::clone(&signals),
    });
    let mut handle = Handle {
        owned: Some(Owned {
            process: Arc::clone(&process),
            signals: Arc::clone(&signals),
            client: Arc::clone(&client),
            writer: None,
            reader: None,
            observer: None,
        }),
    };

    let outcome = (|| -> Result<bool> {
        // Guarded Handle already owns Child/tree before ANY thread creation can
        // fail. No closure is the child's sole owner on Builder::spawn failure.
        let observer_signals = Arc::clone(&signals);
        handle.owned.as_mut().unwrap().observer = Some(
            std::thread::Builder::new()
                .name("mobile-process-exit".into())
                .spawn(move || {
                    while !observer_signals.halt.load(Ordering::Acquire) {
                        let mut process = process.lock().unwrap_or_else(|error| error.into_inner());
                        if observer_signals.fatal.load(Ordering::Acquire) {
                            process.terminate_if_unreaped();
                        }
                        if let Ok(Some(status)) = process.observe_exit() {
                            *observer_signals
                                .exited
                                .lock()
                                .unwrap_or_else(|error| error.into_inner()) = Some(status);
                            observer_signals.halt.store(true, Ordering::Release);
                            break;
                        }
                        drop(process);
                        std::thread::sleep(POLL);
                    }
                })?,
        );

        let writer_signals = Arc::clone(&signals);
        handle.owned.as_mut().unwrap().writer = Some(
            std::thread::Builder::new()
                .name("mobile-process-stdin".into())
                .spawn(move || {
                    let mut stdin = BufWriter::new(stdin);
                    let mut stopped = false;
                    if wire::write_magic(&mut stdin).is_err() {
                        writer_signals.fatal.store(true, Ordering::Release);
                        stopped = true;
                    }
                    while !writer_signals.halt.load(Ordering::Acquire) {
                        if writer_signals.stop.load(Ordering::Acquire) && !stopped {
                            if wire::write_frame(&mut stdin, &Frame::control(Header::Stop)).is_err()
                            {
                                writer_signals.fatal.store(true, Ordering::Release);
                            }
                            stopped = true;
                        }
                        if stopped {
                            std::thread::sleep(POLL);
                            continue;
                        }
                        match incoming.recv_timeout(POLL) {
                            Ok(frame) => {
                                if writer_signals.stop.load(Ordering::Acquire) {
                                    continue;
                                }
                                if wire::write_frame(&mut stdin, &frame).is_err() {
                                    writer_signals.fatal.store(true, Ordering::Release);
                                    stopped = true;
                                }
                            }
                            Err(mpsc::RecvTimeoutError::Timeout) => {}
                            Err(mpsc::RecvTimeoutError::Disconnected) => {
                                writer_signals.fatal.store(true, Ordering::Release);
                                stopped = true;
                            }
                        }
                    }
                    // stdin stays open throughout an intentional Stop, until observed exit.
                })?,
        );

        let (events, startup) = mpsc::sync_channel(2);
        let reader_signals = Arc::clone(&signals);
        let admitting = Arc::clone(&client.admitting);
        handle.owned.as_mut().unwrap().reader = Some(
            std::thread::Builder::new()
                .name("mobile-process-stdout".into())
                .spawn(move || {
                    let result = read_responses(stdout, fixture_preamble, &pending, &events);
                    // Fatal first closes the race with startup reopening admission after
                    // Ready; every caller checks fatal again while holding pending's lock.
                    if result.as_ref().err().is_some_and(|error| {
                        reader_error_is_fatal(error, reader_signals.stop.load(Ordering::Acquire))
                    }) {
                        reader_signals.fatal.store(true, Ordering::Release);
                    }
                    admitting.store(false, Ordering::Release);
                    fail_pending(&pending);
                })?,
        );

        observe(&handle);
        let first = startup_wait.receive(&startup, "Hello")?;
        ensure!(
            matches!(first.header, Header::Hello { ref protocol, ref build_commit } if protocol == PROTOCOL && build_commit == expected_source),
            "mobile worker protocol/source mismatch before startup authorization"
        );
        startup_wait.check()?;
        client
            .requests
            .try_send(Frame::control(Header::Start))
            .context("mobile worker startup admission unavailable")?;
        let ready = startup_wait.receive(&startup, "Ready")?;
        let enabled = match ready.header {
            Header::Ready { enabled } => enabled,
            Header::StartupFailure { config_validation } => {
                let message = String::from_utf8(ready.body)
                    .context("non-UTF8 mobile startup failure diagnostic")?;
                let error = anyhow::Error::msg(message);
                return Err(if config_validation {
                    error.context(super::super::config::MobileConfigValidationFailure)
                } else {
                    error
                });
            }
            _ => bail!("unexpected mobile worker startup response"),
        };
        if !enabled {
            return Ok(false);
        }
        // A reader/child failure that happened after Ready must never reopen
        // admission. In a race, read_responses also checks the fatal flag indirectly
        // by closing pending; the writer/observer will reap the failed worker.
        ensure!(
            !signals.fatal.load(Ordering::Acquire) && !signals.halt.load(Ordering::Acquire),
            "mobile worker exited during startup"
        );
        startup_wait.check()?;
        client.admitting.store(true, Ordering::Release);
        *ACTIVE.lock().unwrap_or_else(|error| error.into_inner()) = Some(Arc::downgrade(&client));
        startup_wait.check()?;
        Ok(true)
    })();
    match outcome {
        Ok(true) => Ok(Some(handle)),
        Ok(false) => {
            let deadline = startup_wait.cleanup_deadline();
            match handle.shutdown_until(deadline) {
                Ok(()) => Ok(None),
                Err(error) => Err(startup_wait.contextualize(error, deadline)),
            }
        }
        Err(error) => {
            let deadline = startup_wait.cleanup_deadline();
            handle.request_shutdown();
            // No usable runtime was returned. An ordinary failure or daemon
            // cancellation aborts the owned startup immediately, rather than
            // adding another graceful allowance to an exhausted wait budget.
            if let Some(owned) = handle.owned.as_ref() {
                owned.terminate();
            }
            let cleanup = handle.shutdown_until(deadline);
            // shutdown_until consumes Owned or transfers it to RETAINED, so
            // Handle::drop CANNOT renew a standalone cleanup deadline here.
            let error = startup_wait.contextualize(error, deadline);
            match cleanup {
                Ok(()) => Err(error),
                Err(cleanup) => Err(error.context(format!(
                    "mobile startup ownership cleanup incomplete: {cleanup:#}"
                ))),
            }
        }
    }
}

fn reader_error_is_fatal(error: &anyhow::Error, intentional_stop: bool) -> bool {
    // EOF after intentional Stop is the expected pipe closure on graceful
    // process exit. Observation/deadline handling still owns kill/reap/join;
    // EOF must not race that natural exit into an unnecessary SIGKILL.
    !(intentional_stop
        && error.chain().any(|cause| {
            cause
                .downcast_ref::<std::io::Error>()
                .is_some_and(|error| error.kind() == std::io::ErrorKind::UnexpectedEof)
        }))
}

fn read_responses(
    stdout: std::process::ChildStdout,
    fixture_preamble: bool,
    pending: &Pending,
    events: &mpsc::SyncSender<Frame>,
) -> Result<()> {
    let mut stdout = BufReader::new(stdout);
    wire::read_magic(&mut stdout, fixture_preamble)?;
    let hello = wire::read_frame(&mut stdout)?;
    ensure!(
        matches!(hello.header, Header::Hello { .. }),
        "mobile worker omitted hello"
    );
    events
        .try_send(hello)
        .context("mobile worker hello receiver unavailable")?;
    let ready = wire::read_frame(&mut stdout)?;
    ensure!(
        matches!(
            ready.header,
            Header::Ready { .. } | Header::StartupFailure { .. }
        ),
        "mobile worker omitted ready"
    );
    events
        .try_send(ready)
        .context("mobile worker ready receiver unavailable")?;
    loop {
        let frame = wire::read_frame(&mut stdout)?;
        let (id, response) = match frame.header {
            Header::Response { id, status } => {
                ensure!(
                    (100..600).contains(&status),
                    "invalid mobile response status"
                );
                let body = String::from_utf8(frame.body).context("non-UTF8 mobile response")?;
                (
                    id,
                    Ok(crate::api::ApiResponse {
                        status,
                        content_type: "application/json",
                        body,
                    }),
                )
            }
            Header::Unavailable { id } => (id, Err(CallFailure::Unavailable)),
            Header::Failure { id } => {
                let message =
                    String::from_utf8(frame.body).context("non-UTF8 mobile failure diagnostic")?;
                (id, Err(CallFailure::Native(message)))
            }
            _ => bail!("unexpected mobile worker response"),
        };
        if let Some(sender) = pending
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&id)
        {
            let _ = sender.try_send(response);
        }
    }
}

fn fail_pending(pending: &Pending) {
    let drained = std::mem::take(&mut *pending.lock().unwrap_or_else(|error| error.into_inner()));
    for sender in drained.into_values() {
        let _ = sender.try_send(Err(CallFailure::Unavailable));
    }
}

/// Called only by the hidden main branch, before shared StateLock or settings
/// I/O. The independent stdin reader is alive before Start can reach main.
pub(crate) fn run_worker(protocol: &str, home: &Path) -> Result<()> {
    ensure!(protocol == PROTOCOL, "unsupported mobile worker protocol");
    let mut stdout = BufWriter::new(std::io::stdout());
    wire::write_magic(&mut stdout)?;
    wire::write_frame(
        &mut stdout,
        &Frame::control(Header::Hello {
            protocol: PROTOCOL.into(),
            build_commit: SOURCE.into(),
        }),
    )?;
    let storage = super::super::store_lock::Runtime::default();
    let stopping = Arc::new(AtomicBool::new(false));
    let (commands, requests) = mpsc::sync_channel(QUEUE);
    let (responses, outgoing) = mpsc::sync_channel::<Frame>(QUEUE);
    std::thread::Builder::new()
        .name("mobile-worker-stdout".into())
        .spawn(move || {
            for frame in outgoing {
                if wire::write_frame(&mut stdout, &frame).is_err() {
                    std::process::exit(70);
                }
            }
        })?;
    let reader_storage = storage.clone();
    let reader_stopping = Arc::clone(&stopping);
    let overflow = responses.clone();
    std::thread::Builder::new()
        .name("mobile-worker-parent-death".into())
        .spawn(move || {
            let result = read_commands(&commands, &overflow, &reader_storage, &reader_stopping);
            // EOF means parent death even when native startup, a request or Drop is
            // stuck. This is the mobile worker only, never the user daemon.
            if let Err(_error) = result {
                #[cfg(test)]
                before_test_parent_eof_exit(&_error);
                std::process::exit(70);
            }
        })?;
    let start = requests
        .recv()
        .context("mobile worker startup authorization unavailable")?;
    ensure!(
        matches!(start.header, Header::Start),
        "mobile worker expected Start"
    );
    if stopping.load(Ordering::Acquire) {
        std::process::exit(0);
    }
    let _scope = storage.enter();
    // The child owns an independent shared lease. Parent death releases the
    // parent's lease before EOF can be processed; reset must remain excluded
    // while any old mobile authority thread is still alive in this child.
    let initialized = (|| -> Result<_> {
        let lease = crate::state_lock::acquire_shared(home)?;
        init_settings();
        let native = super::start_mobile_gateway_native_for_daemon(home)?;
        Ok((lease, native))
    })();
    let (_state_lock, native) = match initialized {
        Ok(initialized) => initialized,
        Err(error) => {
            let diagnostic = Frame {
                header: Header::StartupFailure {
                    config_validation: error
                        .is::<super::super::config::MobileConfigValidationFailure>(),
                },
                body: bounded_native_diagnostic(&error),
            };
            responses
                .try_send(diagnostic)
                .context("mobile startup failure response queue unavailable")?;
            // Stay alive with the independent reader/writer until the parent
            // RECEIVES this frame and requests Stop (or parent EOF terminates
            // us). Enqueue then immediate process::exit could lose diagnostics.
            while !stopping.load(Ordering::Acquire) {
                std::thread::sleep(POLL);
            }
            storage.cancel();
            std::process::exit(70);
        }
    };
    if responses
        .try_send(Frame::control(Header::Ready {
            enabled: native.is_some(),
        }))
        .is_err()
    {
        std::process::exit(70);
    }
    while !stopping.load(Ordering::Acquire) {
        let frame = match requests.recv_timeout(POLL) {
            Ok(frame) => frame,
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        };
        if stopping.load(Ordering::Acquire) {
            break;
        }
        let Header::Request {
            id,
            method,
            path,
            has_body,
        } = frame.header
        else {
            bail!("unexpected mobile worker main request");
        };
        let body = String::from_utf8(frame.body).context("non-UTF8 mobile control body")?;
        let response = match super::handle_local_control_native(
            &method,
            &path,
            has_body.then_some(body.as_str()),
        ) {
            Some(Ok(response)) if response.content_type == "application/json" => Frame {
                header: Header::Response {
                    id,
                    status: response.status,
                },
                body: response.body.into_bytes(),
            },
            Some(Err(error)) => Frame {
                header: Header::Failure { id },
                body: bounded_native_diagnostic(&error),
            },
            _ => Frame::control(Header::Unavailable { id }),
        };
        if responses.try_send(response).is_err() {
            std::process::exit(70);
        }
    }
    storage.cancel();
    drop(native); // Graceful native joins; the parent supplies the hard backstop.
                  // Worker protocol threads belong to this process. Exiting after all native
                  // authority joins closes them together; no detached authority thread lives.
    std::process::exit(0)
}

fn bounded_native_diagnostic(error: &anyhow::Error) -> Vec<u8> {
    const SUFFIX: &str = " [diagnostic truncated]";
    let mut message = format!("{error:#}");
    if message.len() > wire::MAX_FAILURE_BYTES {
        let mut end = wire::MAX_FAILURE_BYTES - SUFFIX.len();
        while !message.is_char_boundary(end) {
            end -= 1;
        }
        message.truncate(end);
        message.push_str(SUFFIX);
    }
    message.into_bytes()
}

fn read_commands(
    commands: &mpsc::SyncSender<Frame>,
    responses: &mpsc::SyncSender<Frame>,
    storage: &super::super::store_lock::Runtime,
    stopping: &AtomicBool,
) -> Result<()> {
    let mut stdin = BufReader::new(std::io::stdin());
    wire::read_magic(&mut stdin, false)?;
    let mut started = false;
    loop {
        let frame = wire::read_frame(&mut stdin)?;
        match &frame.header {
            Header::Start if !started => {
                started = true;
                commands
                    .try_send(frame)
                    .context("mobile worker Start queue unavailable")?;
            }
            Header::Stop => {
                // Cancel the shared scope BEFORE native joins and continue
                // watching EOF throughout intentional graceful shutdown.
                stopping.store(true, Ordering::Release);
                storage.cancel();
                if !started {
                    std::process::exit(0);
                }
            }
            Header::Request { id, path, .. }
                if started
                    && !stopping.load(Ordering::Acquire)
                    && super::claims_local_control(path) =>
            {
                let id = *id;
                if commands.try_send(frame).is_err() {
                    responses
                        .try_send(Frame::control(Header::Unavailable { id }))
                        .context("mobile worker response queue saturated")?;
                }
            }
            Header::Request { id, .. } if started => {
                responses
                    .try_send(Frame::control(Header::Unavailable { id: *id }))
                    .context("mobile worker response queue saturated")?;
            }
            _ => bail!("unexpected mobile worker parent frame"),
        }
    }
}

fn init_settings() {
    let loaded = crate::settings::user_settings_path()
        .as_deref()
        .and_then(|path| match crate::settings::load_from(path) {
            Ok(settings) => settings,
            Err(error) => {
                eprintln!("coven: ignoring settings ({}): {error:#}", path.display());
                None
            }
        });
    crate::settings::init_cached(loaded);
}

#[cfg(test)]
fn before_test_parent_eof_exit(error: &anyhow::Error) {
    let eof = error.chain().any(|cause| {
        cause
            .downcast_ref::<std::io::Error>()
            .is_some_and(|error| error.kind() == std::io::ErrorKind::UnexpectedEof)
    });
    if !eof {
        return;
    }
    let Some(release) =
        std::env::var_os("COVEN_TEST_MOBILE_PARENT_EOF_RELEASE").map(std::path::PathBuf::from)
    else {
        return;
    };
    publish_test_observation(
        &release.with_extension("eof-seen"),
        b"real parent EOF observed\n",
    )
    .expect("publish parent-death observation");
    while !release.exists() {
        std::thread::sleep(POLL);
    }
}

#[cfg(test)]
fn publish_test_observation(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let staged = path.with_extension(format!("{}.pending", uuid::Uuid::new_v4()));
    std::fs::write(&staged, bytes)?;
    std::fs::rename(staged, path)
}

#[cfg(test)]
#[path = "native_fixture.rs"]
pub(super) mod native_fixture;

#[cfg(all(test, target_os = "macos"))]
#[path = "native_parent_death_tests.rs"]
mod parent_death_tests;
