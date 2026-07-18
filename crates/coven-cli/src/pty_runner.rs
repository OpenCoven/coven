use std::io::{self, BufRead, BufReader, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, SyncSender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

#[cfg(unix)]
use std::sync::atomic::AtomicI32;
#[cfg(unix)]
use std::sync::MutexGuard;

use anyhow::{Context, Result};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use portable_pty::{native_pty_system, ChildKiller, CommandBuilder, PtySize, PtySystem};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HarnessCommand {
    program: String,
    args: Vec<String>,
    cwd: PathBuf,
    stdin_prompt: Option<Vec<u8>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PtyRunResult {
    pub status: &'static str,
    pub exit_code: Option<i32>,
}

/// Outcome of Coven's one-shot `codex exec --json` bridge.
///
/// `harness_session_id` is the Codex thread id, not Coven's ledger session
/// id. Callers keep the two separate so they can expose a stable Coven id yet
/// resume the actual Codex conversation on a later turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexJsonRunResult {
    pub process: PtyRunResult,
    pub harness_session_id: Option<String>,
    pub error: Option<String>,
    pub emitted_assistant: bool,
}

pub struct DetachedPtySession {
    pub input: Box<dyn Write + Send>,
    pub killer: Box<dyn ChildKiller + Send + Sync>,
}

pub struct DetachedPtyObserver {
    pub on_output: Box<dyn FnMut(Vec<u8>) + Send + 'static>,
    pub on_exit: Box<dyn FnOnce(PtyRunResult) + Send + 'static>,
}

#[cfg(windows)]
const DETACHED_STARTUP_TIMEOUT: Duration = Duration::from_secs(30);
const PTY_WRITE_QUEUE_CAPACITY: usize = 16;
enum PtyWriteRequest {
    Write {
        bytes: Vec<u8>,
        flush: bool,
        completion: Option<SyncSender<io::Result<()>>>,
    },
    Flush {
        completion: SyncSender<io::Result<()>>,
    },
}

#[derive(Clone)]
struct SharedPtyWriter {
    sender: SyncSender<PtyWriteRequest>,
}

impl Write for SharedPtyWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.write_and_wait(buf, false)?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        let (completion, completed) = mpsc::sync_channel(1);
        self.sender
            .send(PtyWriteRequest::Flush { completion })
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "PTY writer stopped"))?;
        completed
            .recv()
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "PTY writer stopped"))?
    }

    fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        self.write_and_wait(buf, false)
    }
}

impl SharedPtyWriter {
    fn write_and_wait(&self, bytes: &[u8], flush: bool) -> io::Result<()> {
        let (completion, completed) = mpsc::sync_channel(1);
        self.sender
            .send(PtyWriteRequest::Write {
                bytes: bytes.to_vec(),
                flush,
                completion: Some(completion),
            })
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "PTY writer stopped"))?;
        completed
            .recv()
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "PTY writer stopped"))?
    }

    fn queue_terminal_reply(&self, reply: &'static [u8]) {
        // The output drain must never wait for the PTY input side. All replies
        // use this single FIFO path, preserving query order without blocking.
        let _ = self.sender.try_send(PtyWriteRequest::Write {
            bytes: reply.to_vec(),
            flush: true,
            completion: None,
        });
    }
}

fn spawn_shared_pty_writer(writer: Box<dyn Write + Send>) -> SharedPtyWriter {
    let (sender, receiver) = mpsc::sync_channel(PTY_WRITE_QUEUE_CAPACITY);
    thread::spawn(move || run_pty_writer(writer, receiver));
    SharedPtyWriter { sender }
}

fn run_pty_writer(mut writer: Box<dyn Write + Send>, receiver: mpsc::Receiver<PtyWriteRequest>) {
    while let Ok(request) = receiver.recv() {
        let (result, completion) = match request {
            PtyWriteRequest::Write {
                bytes,
                flush,
                completion,
            } => {
                let result = if completion.is_none() {
                    writer.write(&bytes).and_then(|written| {
                        if written == bytes.len() {
                            Ok(())
                        } else {
                            Err(io::Error::new(
                                io::ErrorKind::WriteZero,
                                "short terminal reply write",
                            ))
                        }
                    })
                } else {
                    writer.write_all(&bytes)
                }
                .and_then(|_| if flush { writer.flush() } else { Ok(()) });
                (result, completion)
            }
            PtyWriteRequest::Flush { completion } => (writer.flush(), Some(completion)),
        };
        let failed = result.is_err();
        let terminal_reply = completion.is_none();
        if let Some(completion) = completion {
            let _ = completion.send(result);
        }
        if terminal_reply && !failed {
            // ConPTY can acknowledge a tiny pipe write before its console
            // input loop is ready for the next reply. A short writer-thread
            // yield preserves FIFO pacing without ever delaying output drain.
            thread::sleep(Duration::from_millis(1));
        }
        if failed {
            break;
        }
    }
}

#[derive(Debug, Clone)]
struct SharedPtyKiller {
    inner: Arc<Mutex<PtyKillerInner>>,
}

#[derive(Debug)]
struct PtyKillerInner {
    fallback: Box<dyn ChildKiller + Send + Sync>,
    #[cfg(windows)]
    job_handle: Option<windows_sys::Win32::Foundation::HANDLE>,
}

#[cfg(windows)]
unsafe impl Send for PtyKillerInner {}

#[cfg(windows)]
impl Drop for PtyKillerInner {
    fn drop(&mut self) {
        if let Some(handle) = self.job_handle.take() {
            // SAFETY: this struct exclusively owns the job handle.
            unsafe { windows_sys::Win32::Foundation::CloseHandle(handle) };
        }
    }
}

impl ChildKiller for SharedPtyKiller {
    fn kill(&mut self) -> io::Result<()> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| io::Error::other("PTY killer lock poisoned"))?;
        #[cfg(windows)]
        if let Some(handle) = inner.job_handle.take() {
            // Terminating the job stops the harness and every process it
            // spawned. This is what prevents startup-timeout orphans.
            let result =
                unsafe { windows_sys::Win32::System::JobObjects::TerminateJobObject(handle, 1) };
            unsafe { windows_sys::Win32::Foundation::CloseHandle(handle) };
            if result != 0 {
                return Ok(());
            }
        }
        inner.fallback.kill()
    }

    fn clone_killer(&self) -> Box<dyn ChildKiller + Send + Sync> {
        Box::new(self.clone())
    }
}

impl HarnessCommand {
    pub fn program(&self) -> &str {
        &self.program
    }

    #[cfg(test)]
    pub fn args(&self) -> &[String] {
        &self.args
    }

    #[cfg(test)]
    pub fn cwd(&self) -> &Path {
        &self.cwd
    }

    fn to_command_builder(&self) -> CommandBuilder {
        let mut builder = CommandBuilder::new(&self.program);
        builder.args(&self.args);
        builder.cwd(self.cwd.as_os_str());
        builder
    }
}

pub fn build_harness_command(
    harness_id: &str,
    prompt: &str,
    cwd: &Path,
    mode: crate::harness::HarnessLaunchMode,
) -> Result<HarnessCommand> {
    build_harness_command_with_conversation(
        harness_id,
        prompt,
        cwd,
        mode,
        None,
        None,
        crate::harness::HarnessLaunchOptions::default(),
    )
}

#[allow(clippy::too_many_arguments)]
pub fn build_harness_command_with_conversation(
    harness_id: &str,
    prompt: &str,
    cwd: &Path,
    mode: crate::harness::HarnessLaunchMode,
    conversation: Option<&crate::harness::ConversationHint>,
    familiar: Option<&crate::harness::FamiliarContext>,
    options: crate::harness::HarnessLaunchOptions<'_>,
) -> Result<HarnessCommand> {
    build_harness_command_with_conversation_inner(
        harness_id,
        prompt,
        cwd,
        mode,
        conversation,
        familiar,
        options,
        false,
    )
}

/// Build the dedicated one-shot Codex JSON command used by the stream bridge.
/// Keeping JSON-mode construction here makes the actual Codex `exec` token
/// explicit before user-controlled launch values or the trailing prompt are
/// added to argv.
#[allow(clippy::too_many_arguments)]
pub fn build_codex_json_harness_command_with_conversation(
    harness_id: &str,
    prompt: &str,
    cwd: &Path,
    mode: crate::harness::HarnessLaunchMode,
    conversation: Option<&crate::harness::ConversationHint>,
    familiar: Option<&crate::harness::FamiliarContext>,
    options: crate::harness::HarnessLaunchOptions<'_>,
) -> Result<HarnessCommand> {
    build_harness_command_with_conversation_inner(
        harness_id,
        prompt,
        cwd,
        mode,
        conversation,
        familiar,
        options,
        true,
    )
}

#[allow(clippy::too_many_arguments)]
fn build_harness_command_with_conversation_inner(
    harness_id: &str,
    prompt: &str,
    cwd: &Path,
    mode: crate::harness::HarnessLaunchMode,
    conversation: Option<&crate::harness::ConversationHint>,
    familiar: Option<&crate::harness::FamiliarContext>,
    options: crate::harness::HarnessLaunchOptions<'_>,
    codex_json: bool,
) -> Result<HarnessCommand> {
    let (program, mut args) = if codex_json {
        crate::harness::command_parts_for_codex_json_with_conversation(
            harness_id,
            prompt,
            mode,
            conversation,
            familiar,
            options,
        )?
    } else {
        crate::harness::command_parts_for_harness_with_conversation(
            harness_id,
            prompt,
            mode,
            conversation,
            familiar,
            options,
        )?
    };
    let familiar_prompt;
    let stdin_prompt_text = if harness_id == "codex" {
        if let Some(familiar) = familiar {
            familiar_prompt = format!("{}\n\n{prompt}", familiar.identity_preamble());
            familiar_prompt.as_str()
        } else {
            prompt
        }
    } else {
        prompt
    };
    let stdin_prompt = move_windows_codex_prompt_to_stdin(
        harness_id,
        mode,
        stdin_prompt_text,
        &mut args,
        cfg!(windows),
    );

    Ok(HarnessCommand {
        program: program.to_string(),
        args,
        cwd: cwd.to_path_buf(),
        stdin_prompt,
    })
}

/// Windows may resolve an npm-installed Codex harness to `codex.CMD`. Rust
/// launches batch files through `cmd.exe` and deliberately rejects multiline
/// or otherwise unsafe batch arguments. Codex supports `-` as the prompt
/// positional, reading the real prompt from stdin, so keep user-controlled
/// prompt text out of the batch command line entirely.
fn move_windows_codex_prompt_to_stdin(
    harness_id: &str,
    mode: crate::harness::HarnessLaunchMode,
    prompt: &str,
    args: &mut [String],
    is_windows: bool,
) -> Option<Vec<u8>> {
    if !is_windows
        || harness_id != "codex"
        || mode != crate::harness::HarnessLaunchMode::NonInteractive
    {
        return None;
    }

    let prompt_arg = args.last_mut()?;
    *prompt_arg = "-".to_string();
    Some(prompt.as_bytes().to_vec())
}

#[cfg(windows)]
fn write_stdin_prompt(child: &mut std::process::Child, prompt: Option<&[u8]>) -> Result<()> {
    let Some(prompt) = prompt else {
        return Ok(());
    };
    let result = (|| -> Result<()> {
        let mut stdin = child
            .stdin
            .take()
            .context("piped harness did not expose stdin for its prompt")?;
        stdin
            .write_all(prompt)
            .context("failed writing harness prompt to stdin")?;
        stdin.flush().context("failed flushing harness prompt")?;
        Ok(())
    })();
    if result.is_err() {
        let _ = child.kill();
        let _ = child.wait();
    }
    result
}

const CODEX_JSON_ACTIVITY_TIMEOUT: Duration = Duration::from_secs(5 * 60);
const CODEX_POST_EXIT_DRAIN_TIMEOUT: Duration = Duration::from_secs(1);
const CODEX_CHILD_POLL_INTERVAL: Duration = Duration::from_millis(50);
const CODEX_STDERR_TAIL_BYTES: usize = 8 * 1024;

// `codex exec --json` runs in a separate Unix session so a timeout can clean
// up an npm/Node/Codex tree in one operation. That also means a TERM sent to
// coven itself would otherwise leave the child group behind. The scoped guard
// below records the cancellation in an async-signal-safe handler; the runner
// then performs ordinary cleanup and emits its terminal result.
#[cfg(unix)]
static CODEX_JSON_CANCELLATION_SIGNAL: AtomicI32 = AtomicI32::new(0);
#[cfg(unix)]
static CODEX_JSON_PROCESS_GROUP: AtomicI32 = AtomicI32::new(0);
#[cfg(unix)]
static CODEX_JSON_CANCELLATION_LOCK: Mutex<()> = Mutex::new(());

#[cfg(unix)]
extern "C" fn record_codex_json_cancellation(signal: libc::c_int) {
    // Atomic operations and kill(2) are async-signal-safe. The supervisor
    // turns the flag into a failed ledger/result update on its next <=50 ms
    // poll; killing the group here prevents a detached Codex descendant from
    // surviving if that poll is delayed.
    let process_group = CODEX_JSON_PROCESS_GROUP.load(Ordering::Relaxed);
    if process_group > 0 {
        unsafe {
            let _ = libc::kill(-process_group, libc::SIGKILL);
        }
    }
    CODEX_JSON_CANCELLATION_SIGNAL.store(signal, Ordering::Relaxed);
}

/// Temporarily converts TERM/INT/HUP into a supervised bridge cancellation.
///
/// Signal dispositions are process-global, so runs in one process are
/// serialized while the guard is installed. The old dispositions are restored
/// before releasing that lock, preserving normal signal behavior for other
/// Coven commands and unit tests.
#[cfg(unix)]
struct CodexCancellationGuard {
    _lock: MutexGuard<'static, ()>,
    previous_handlers: Vec<(libc::c_int, libc::sigaction)>,
}

#[cfg(unix)]
impl CodexCancellationGuard {
    fn install() -> Result<Self> {
        let lock = CODEX_JSON_CANCELLATION_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        CODEX_JSON_CANCELLATION_SIGNAL.store(0, Ordering::Relaxed);
        CODEX_JSON_PROCESS_GROUP.store(0, Ordering::Relaxed);

        let mut previous_handlers = Vec::with_capacity(3);
        for signal in [libc::SIGTERM, libc::SIGINT, libc::SIGHUP] {
            // SAFETY: sigaction is the POSIX interface for installing a signal
            // handler. The handler uses only atomics and kill(2), and each
            // successful installation retains the prior disposition for Drop.
            unsafe {
                let mut action: libc::sigaction = std::mem::zeroed();
                action.sa_sigaction = record_codex_json_cancellation as *const () as usize;
                libc::sigemptyset(&mut action.sa_mask);
                action.sa_flags = 0;
                let mut previous: libc::sigaction = std::mem::zeroed();
                if libc::sigaction(signal, &action, &mut previous) != 0 {
                    for (installed_signal, installed_previous) in previous_handlers.iter().rev() {
                        let _ = libc::sigaction(
                            *installed_signal,
                            installed_previous,
                            std::ptr::null_mut(),
                        );
                    }
                    CODEX_JSON_CANCELLATION_SIGNAL.store(0, Ordering::Relaxed);
                    return Err(std::io::Error::last_os_error()).with_context(|| {
                        format!("failed to install Codex cancellation handler for signal {signal}")
                    });
                }
                previous_handlers.push((signal, previous));
            }
        }

        Ok(Self {
            _lock: lock,
            previous_handlers,
        })
    }

    fn arm(&self, process_group: u32) {
        CODEX_JSON_PROCESS_GROUP.store(process_group as i32, Ordering::Relaxed);
    }

    fn disarm(&self) {
        CODEX_JSON_PROCESS_GROUP.store(0, Ordering::Relaxed);
    }

    fn cancelled_signal(&self) -> Option<libc::c_int> {
        let signal = CODEX_JSON_CANCELLATION_SIGNAL.load(Ordering::Relaxed);
        (signal != 0).then_some(signal)
    }
}

#[cfg(unix)]
impl Drop for CodexCancellationGuard {
    fn drop(&mut self) {
        CODEX_JSON_PROCESS_GROUP.store(0, Ordering::Relaxed);
        // SAFETY: every entry was captured from a successful sigaction call
        // in install. Restoring it here makes the scope transparent once the
        // bridge has reaped its child tree.
        unsafe {
            for (signal, previous) in self.previous_handlers.iter().rev() {
                let _ = libc::sigaction(*signal, previous, std::ptr::null_mut());
            }
        }
        CODEX_JSON_CANCELLATION_SIGNAL.store(0, Ordering::Relaxed);
    }
}

#[cfg(not(unix))]
struct CodexCancellationGuard;

#[cfg(not(unix))]
impl CodexCancellationGuard {
    fn install() -> Result<Self> {
        Ok(Self)
    }

    fn arm(&self, _process_group: u32) {}

    fn disarm(&self) {}
}

#[cfg(unix)]
fn codex_cancellation_error(guard: &CodexCancellationGuard) -> Option<String> {
    guard.cancelled_signal().map(|signal| {
        let name = match signal {
            libc::SIGTERM => "SIGTERM",
            libc::SIGINT => "SIGINT",
            libc::SIGHUP => "SIGHUP",
            _ => "a termination signal",
        };
        format!("Codex turn cancelled by {name}; the process tree was terminated")
    })
}

#[cfg(not(unix))]
fn codex_cancellation_error(_guard: &CodexCancellationGuard) -> Option<String> {
    None
}

fn codex_json_activity_timeout() -> Duration {
    // Integration tests execute the real `coven` binary, not the unit-test
    // crate, so `cfg(test)` cannot inject a short deadline into that child.
    // Keep this hook out of release builds while still making the terminal
    // timeout/result/ledger path testable without waiting five minutes.
    #[cfg(debug_assertions)]
    if let Some(timeout_ms) = std::env::var("COVEN_TEST_CODEX_JSON_IDLE_TIMEOUT_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|timeout_ms| *timeout_ms > 0)
    {
        return Duration::from_millis(timeout_ms);
    }
    CODEX_JSON_ACTIVITY_TIMEOUT
}

enum CodexStdoutMessage {
    Line(String),
    ReadError(String),
}

enum CodexRunnerMessage {
    Stdout(CodexStdoutMessage),
    StdoutClosed,
    StderrClosed(Vec<u8>),
    StdinComplete(std::result::Result<(), String>),
}

#[derive(Default)]
struct CodexJsonState {
    harness_session_id: Option<String>,
    protocol_error: Option<String>,
    emitted_assistant: bool,
}

/// Owns the direct Codex child and all of its descendants while a one-shot
/// JSON turn is running. A Node/npm wrapper can outlive or outspawn the direct
/// launcher, so a plain `Child::kill()` is not enough to guarantee pipe EOF.
struct CodexProcessTree {
    pid: u32,
    terminated: bool,
    #[cfg(windows)]
    job_handle: Option<windows_sys::Win32::Foundation::HANDLE>,
}

impl CodexProcessTree {
    fn attach(child: &std::process::Child) -> Self {
        let pid = child.id();
        #[cfg(windows)]
        let job_handle = codex_job_object_for_process(child);
        Self {
            pid,
            terminated: false,
            #[cfg(windows)]
            job_handle,
        }
    }

    fn terminate(&mut self, child: &mut std::process::Child) {
        if self.terminated {
            return;
        }
        self.terminated = true;
        #[cfg(unix)]
        {
            terminate_codex_unix_process_group(self.pid);
        }
        #[cfg(windows)]
        {
            let terminated_by_job = self
                .job_handle
                .take()
                .map(|job| {
                    let succeeded = unsafe {
                        windows_sys::Win32::System::JobObjects::TerminateJobObject(job, 1) != 0
                    };
                    unsafe { windows_sys::Win32::Foundation::CloseHandle(job) };
                    succeeded
                })
                .unwrap_or(false);
            if !terminated_by_job {
                // A Job Object can be unavailable when a parent policy forbids
                // assignment. Fall back to Windows' documented tree kill for
                // npm's cmd.exe -> node.exe -> codex.exe chain.
                if let Some(taskkill) = trusted_windows_taskkill_path() {
                    let pid = self.pid.to_string();
                    let _ = std::process::Command::new(taskkill)
                        .args(["/PID", &pid, "/T", "/F"])
                        .stdin(Stdio::null())
                        .stdout(Stdio::null())
                        .stderr(Stdio::null())
                        .status();
                }
            }
        }
        let _ = child.kill();
    }
}

#[cfg(unix)]
fn terminate_codex_unix_process_group(pid: u32) {
    // The launch config puts the child at the head of a new session, so the
    // negative pid reaches its wrapper and every descendant.
    let _ = unsafe { libc::kill(-(pid as libc::pid_t), libc::SIGKILL) };
}

#[cfg(unix)]
impl Drop for CodexProcessTree {
    fn drop(&mut self) {
        if !self.terminated {
            // A wrapper can exit after detaching a descendant that has already
            // closed stdout/stderr. There is then no pipe timeout to trigger
            // terminate(), but this one-shot runner still owns that group.
            terminate_codex_unix_process_group(self.pid);
        }
    }
}

#[cfg(windows)]
impl Drop for CodexProcessTree {
    fn drop(&mut self) {
        if let Some(job) = self.job_handle.take() {
            // The job is configured with KILL_ON_JOB_CLOSE, so an abrupt
            // coven.exe exit also cleans up npm/Node/Codex descendants.
            unsafe { windows_sys::Win32::Foundation::CloseHandle(job) };
        }
    }
}

#[cfg(windows)]
fn trusted_windows_taskkill_path() -> Option<PathBuf> {
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::System::SystemInformation::GetSystemDirectoryW;

    let mut buffer = vec![0u16; 260];
    let len = unsafe { GetSystemDirectoryW(buffer.as_mut_ptr(), buffer.len() as u32) };
    if len == 0 {
        return None;
    }
    let len = len as usize;
    if len >= buffer.len() {
        buffer.resize(len + 1, 0);
        let len = unsafe { GetSystemDirectoryW(buffer.as_mut_ptr(), buffer.len() as u32) };
        if len == 0 || len as usize >= buffer.len() {
            return None;
        }
        buffer.truncate(len as usize);
    } else {
        buffer.truncate(len);
    }

    let path = PathBuf::from(std::ffi::OsString::from_wide(&buffer)).join("taskkill.exe");
    // A missing binary (or an unexpected system directory) must surface as
    // "no trusted taskkill" rather than a silently ignored spawn failure.
    path.is_file().then_some(path)
}

#[cfg(windows)]
fn codex_job_object_for_process(
    child: &std::process::Child,
) -> Option<windows_sys::Win32::Foundation::HANDLE> {
    use std::mem::size_of;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::{
        Foundation::{CloseHandle, INVALID_HANDLE_VALUE},
        System::JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
            SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
            JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        },
    };

    unsafe {
        let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        if job == INVALID_HANDLE_VALUE || job == 0 as _ {
            return None;
        }
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let limits_set = SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            &limits as *const _ as *const std::ffi::c_void,
            size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        ) != 0;
        // Child already owns the CreateProcess handle with the permissions
        // required for assignment, avoiding a pid reuse race through
        // OpenProcess.
        let assigned = AssignProcessToJobObject(job, child.as_raw_handle() as _) != 0;
        if !limits_set || !assigned {
            CloseHandle(job);
            return None;
        }
        Some(job)
    }
}

fn configure_codex_json_command(_command: &mut std::process::Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        unsafe {
            _command.pre_exec(|| {
                // Isolate this turn in a fresh process group. A timeout can
                // then kill the npm/Node/native Codex tree in one signal.
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
}

/// Run one non-interactive Codex turn through its supported JSONL protocol.
///
/// This intentionally uses ordinary OS pipes on every platform. In
/// particular, Windows npm installs expose `codex.cmd`; putting that shim
/// behind ConPTY can stall before the real Node/Codex process starts. The
/// existing command builder keeps a Windows prompt on stdin (`codex exec -`),
/// so this runner neither needs a shell nor puts user text in a batch command
/// line.
pub fn stream_codex_json<F>(command: &HarnessCommand, on_assistant: F) -> Result<CodexJsonRunResult>
where
    F: FnMut(&str) -> Result<()>,
{
    stream_codex_json_with_timeouts(
        command,
        codex_json_activity_timeout(),
        CODEX_POST_EXIT_DRAIN_TIMEOUT,
        on_assistant,
    )
}

#[cfg(test)]
fn stream_codex_json_with_timeout<F>(
    command: &HarnessCommand,
    activity_timeout: Duration,
    on_assistant: F,
) -> Result<CodexJsonRunResult>
where
    F: FnMut(&str) -> Result<()>,
{
    stream_codex_json_with_timeouts(
        command,
        activity_timeout,
        CODEX_POST_EXIT_DRAIN_TIMEOUT,
        on_assistant,
    )
}

fn stream_codex_json_with_timeouts<F>(
    command: &HarnessCommand,
    activity_timeout: Duration,
    post_exit_drain_timeout: Duration,
    mut on_assistant: F,
) -> Result<CodexJsonRunResult>
where
    F: FnMut(&str) -> Result<()>,
{
    let prompt_separator = command
        .args
        .iter()
        .position(|arg| arg == "--")
        .context("Codex JSON bridge expected a prompt separator")?;
    if !command.args[..prompt_separator]
        .iter()
        .any(|arg| arg == "--json")
    {
        anyhow::bail!("Codex JSON bridge expected `--json` to be constructed before the prompt");
    }

    let mut child_command = std::process::Command::new(&command.program);
    child_command
        .args(&command.args)
        .current_dir(&command.cwd)
        .stdin(if command.stdin_prompt.is_some() {
            Stdio::piped()
        } else {
            // A one-shot prompt is already an argv positional on non-Windows
            // hosts. Do not inherit Coven's stdin: Codex may otherwise wait
            // for additional input after a completed request.
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    configure_codex_json_command(&mut child_command);
    let cancellation = CodexCancellationGuard::install()?;
    if let Some(error) = codex_cancellation_error(&cancellation) {
        anyhow::bail!(error);
    }
    let mut child = child_command.spawn().with_context(|| {
        format!(
            "failed to spawn harness `{}` in Codex JSON mode",
            command.program()
        )
    })?;
    let mut process_tree = CodexProcessTree::attach(&child);
    cancellation.arm(process_tree.pid);

    let stdout = match child.stdout.take() {
        Some(stdout) => stdout,
        None => {
            process_tree.terminate(&mut child);
            let _ = child.wait();
            anyhow::bail!("Codex JSON runner did not expose stdout");
        }
    };
    let stderr = match child.stderr.take() {
        Some(stderr) => stderr,
        None => {
            process_tree.terminate(&mut child);
            let _ = child.wait();
            anyhow::bail!("Codex JSON runner did not expose stderr");
        }
    };

    let (sender, receiver) = mpsc::channel();
    let stdin_pending = if let Some(prompt) = command.stdin_prompt.clone() {
        let stdin = match child.stdin.take() {
            Some(stdin) => stdin,
            None => {
                process_tree.terminate(&mut child);
                let _ = child.wait();
                anyhow::bail!("Codex JSON runner did not expose stdin for its prompt");
            }
        };
        let sender = sender.clone();
        thread::spawn(move || {
            let result = (|| -> std::io::Result<()> {
                let mut stdin = stdin;
                stdin.write_all(&prompt)?;
                stdin.flush()
            })()
            .map_err(|error| format!("failed writing Codex prompt to stdin: {error}"));
            let _ = sender.send(CodexRunnerMessage::StdinComplete(result));
        });
        true
    } else {
        false
    };

    let stdout_sender = sender.clone();
    thread::spawn(move || {
        let reader = BufReader::new(stdout);
        for line in reader.lines() {
            let message = match line {
                Ok(line) => CodexStdoutMessage::Line(line),
                Err(error) => CodexStdoutMessage::ReadError(error.to_string()),
            };
            if stdout_sender
                .send(CodexRunnerMessage::Stdout(message))
                .is_err()
            {
                return;
            }
        }
        let _ = stdout_sender.send(CodexRunnerMessage::StdoutClosed);
    });
    let stderr_sender = sender.clone();
    thread::spawn(move || {
        let mut reader = BufReader::new(stderr);
        let mut tail = Vec::new();
        let mut buffer = [0_u8; 1024];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(count) => {
                    append_bounded_tail(&mut tail, &buffer[..count], CODEX_STDERR_TAIL_BYTES)
                }
                Err(_) => break,
            }
        }
        let _ = stderr_sender.send(CodexRunnerMessage::StderrClosed(tail));
    });
    drop(sender);

    let mut state = CodexJsonState::default();
    let mut last_activity = Instant::now();
    let mut status = None;
    let mut post_exit_deadline = None;
    let mut stdout_closed = false;
    let mut stderr_tail = None;
    let mut stdin_complete = !stdin_pending;

    loop {
        if let Some(error) = codex_cancellation_error(&cancellation) {
            state.protocol_error.get_or_insert(error);
            process_tree.terminate(&mut child);
            status = Some(
                child
                    .wait()
                    .context("failed waiting for cancelled Codex process")?,
            );
            break;
        }
        if status.is_none() {
            status = child
                .try_wait()
                .context("failed polling Codex JSON process")?;
            if status.is_some() {
                post_exit_deadline = Some(Instant::now() + post_exit_drain_timeout);
            }
        }
        if status.is_some() && stdout_closed && stderr_tail.is_some() && stdin_complete {
            break;
        }

        let remaining = if let Some(deadline) = post_exit_deadline {
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .unwrap_or_default();
            if remaining.is_zero() {
                state.protocol_error.get_or_insert_with(|| {
                    "Codex exited but its output pipes remained open; terminated remaining process tree"
                        .to_string()
                });
                process_tree.terminate(&mut child);
                break;
            }
            remaining
        } else {
            let remaining = activity_timeout
                .checked_sub(last_activity.elapsed())
                .unwrap_or_default();
            if remaining.is_zero() {
                state.protocol_error.get_or_insert_with(|| {
                    format!(
                        "Codex produced no machine-readable activity for {} seconds; the process was terminated",
                        activity_timeout.as_secs()
                    )
                });
                process_tree.terminate(&mut child);
                status = Some(
                    child
                        .wait()
                        .context("failed waiting for timed-out Codex process")?,
                );
                break;
            }
            remaining
        };

        match receiver.recv_timeout(remaining.min(CODEX_CHILD_POLL_INTERVAL)) {
            Ok(CodexRunnerMessage::Stdout(CodexStdoutMessage::Line(line))) => {
                match handle_codex_json_line(&line, &mut state, &mut on_assistant) {
                    Ok(true) => last_activity = Instant::now(),
                    Ok(false) => {}
                    Err(error) => {
                        process_tree.terminate(&mut child);
                        let _ = child.wait();
                        return Err(error);
                    }
                }
                if state.protocol_error.is_some() {
                    process_tree.terminate(&mut child);
                    status = Some(
                        child
                            .wait()
                            .context("failed waiting for failed Codex turn")?,
                    );
                    break;
                }
            }
            Ok(CodexRunnerMessage::Stdout(CodexStdoutMessage::ReadError(error))) => {
                state
                    .protocol_error
                    .get_or_insert_with(|| format!("failed reading Codex JSON output: {error}"));
                process_tree.terminate(&mut child);
                status = Some(
                    child
                        .wait()
                        .context("failed waiting for Codex after stdout error")?,
                );
                break;
            }
            Ok(CodexRunnerMessage::StdoutClosed) => stdout_closed = true,
            Ok(CodexRunnerMessage::StderrClosed(tail)) => stderr_tail = Some(tail),
            Ok(CodexRunnerMessage::StdinComplete(Ok(()))) => stdin_complete = true,
            Ok(CodexRunnerMessage::StdinComplete(Err(error))) => {
                state.protocol_error.get_or_insert(error);
                process_tree.terminate(&mut child);
                status = Some(
                    child
                        .wait()
                        .context("failed waiting for Codex after stdin write error")?,
                );
                break;
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                // All sender threads are gone (pipes closed, stdin written),
                // but the child may still be running with its stdio closed.
                // recv_timeout returns immediately on a disconnected channel,
                // so sleep explicitly to keep the child/deadline polling at
                // its normal cadence instead of busy-spinning until the
                // activity timeout fires.
                thread::sleep(remaining.min(CODEX_CHILD_POLL_INTERVAL));
            }
        }
    }

    // A signal can arrive just after the final polling iteration. Honor it
    // before reporting a completed turn so cancellation always reaches the
    // ledger and terminal result when the runner still owns the child tree.
    if let Some(error) = codex_cancellation_error(&cancellation) {
        state.protocol_error.get_or_insert(error);
        process_tree.terminate(&mut child);
    }

    let status = match status {
        Some(status) => status,
        None => child
            .wait()
            .context("failed waiting for Codex JSON process")?,
    };
    let stderr_tail = stderr_tail.unwrap_or_default();

    if !status.success() && state.protocol_error.is_none() {
        let code = status
            .code()
            .map(|code| code.to_string())
            .unwrap_or_else(|| "an unknown status".to_string());
        let stderr = String::from_utf8_lossy(&stderr_tail).trim().to_string();
        let message = if stderr.is_empty() {
            format!("Codex exited with {code}")
        } else {
            format!("Codex exited with {code}: {stderr}")
        };
        state.protocol_error = Some(message);
    }
    if !state.emitted_assistant && state.protocol_error.is_none() {
        state.protocol_error = Some("Codex completed without an assistant message".to_string());
    }
    let failed = !status.success() || state.protocol_error.is_some();
    let exit_code = if failed {
        status.code().filter(|code| *code != 0).or(Some(1))
    } else {
        status.code()
    };
    // The direct child has reached a terminal status. Do not leave its former
    // pid armed in the async signal handler during the final return/drop
    // window, where a recycled pid could otherwise be targeted.
    cancellation.disarm();

    Ok(CodexJsonRunResult {
        process: PtyRunResult {
            status: if failed { "failed" } else { "completed" },
            exit_code,
        },
        harness_session_id: state.harness_session_id,
        error: state.protocol_error,
        emitted_assistant: state.emitted_assistant,
    })
}

/// Parse one Codex JSONL frame. Returns whether it was a well-formed Codex
/// event, which is the unit that resets the runner's activity deadline.
fn handle_codex_json_line<F>(
    line: &str,
    state: &mut CodexJsonState,
    on_assistant: &mut F,
) -> Result<bool>
where
    F: FnMut(&str) -> Result<()>,
{
    let Ok(event) = serde_json::from_str::<serde_json::Value>(line) else {
        // `--json` promises JSONL. Ignore an unexpected diagnostic here rather
        // than contaminating Coven's own stdout protocol; if Codex produces no
        // valid activity, the bounded timeout reports it.
        return Ok(false);
    };
    let Some(kind) = event.get("type").and_then(serde_json::Value::as_str) else {
        return Ok(false);
    };
    match kind {
        "thread.started" => {
            if let Some(thread_id) = event.get("thread_id").and_then(serde_json::Value::as_str) {
                state.harness_session_id = Some(thread_id.to_string());
            }
        }
        "item.completed" => {
            let Some(item) = event.get("item") else {
                return Ok(true);
            };
            if item.get("type").and_then(serde_json::Value::as_str) == Some("agent_message") {
                if let Some(text) = item.get("text").and_then(serde_json::Value::as_str) {
                    if !text.is_empty() {
                        on_assistant(text)?;
                        state.emitted_assistant = true;
                    }
                }
            }
        }
        "turn.failed" | "error" => {
            if let Some(message) = codex_event_error_message(&event) {
                state.protocol_error.get_or_insert(message);
            } else {
                state.protocol_error.get_or_insert_with(|| {
                    format!("Codex reported {kind} without an error message")
                });
            }
        }
        _ => {}
    }
    Ok(true)
}

fn append_bounded_tail(tail: &mut Vec<u8>, chunk: &[u8], max_bytes: usize) {
    if chunk.len() >= max_bytes {
        tail.clear();
        tail.extend_from_slice(&chunk[chunk.len() - max_bytes..]);
        return;
    }
    let excess = tail
        .len()
        .saturating_add(chunk.len())
        .saturating_sub(max_bytes);
    if excess > 0 {
        tail.drain(..excess);
    }
    tail.extend_from_slice(chunk);
}

fn codex_event_error_message(event: &serde_json::Value) -> Option<String> {
    if let Some(error) = event.get("error") {
        match error {
            serde_json::Value::String(message) if !message.trim().is_empty() => {
                return Some(message.clone());
            }
            serde_json::Value::Object(_) => {
                if let Some(message) = error
                    .get("message")
                    .and_then(serde_json::Value::as_str)
                    .filter(|message| !message.trim().is_empty())
                {
                    return Some(message.to_string());
                }
            }
            _ => {}
        }
    }
    // Codex currently emits some `type:"error"` frames as
    // `{ "message": "..." }` rather than nesting the message under `error`.
    // Keep the bridge tolerant of both documented JSONL shapes.
    event
        .get("message")
        .and_then(serde_json::Value::as_str)
        .filter(|message| !message.trim().is_empty())
        .map(ToOwned::to_owned)
}

#[allow(clippy::too_many_arguments)]
pub fn build_stream_harness_command_with_conversation(
    harness_id: &str,
    prompt: &str,
    cwd: &Path,
    forward_stdin: bool,
    conversation: Option<&crate::harness::ConversationHint>,
    familiar: Option<&crate::harness::FamiliarContext>,
    options: crate::harness::HarnessLaunchOptions<'_>,
) -> Result<HarnessCommand> {
    let (program, args) = crate::harness::command_parts_for_harness_with_conversation(
        harness_id,
        "",
        crate::harness::HarnessLaunchMode::Stream,
        conversation,
        familiar,
        options,
    )?;
    let mut args = stream_passthrough_args(args, forward_stdin);
    args.extend(["--".to_string(), prompt.to_string()]);
    let args = crate::harness::sanitize_argv_for_platform(args);
    Ok(HarnessCommand {
        program,
        args,
        cwd: cwd.to_path_buf(),
        stdin_prompt: None,
    })
}

fn stream_passthrough_args(args: Vec<String>, forward_stdin: bool) -> Vec<String> {
    if forward_stdin {
        return args;
    }
    let mut filtered = Vec::with_capacity(args.len());
    let mut iter = args.into_iter().peekable();
    while let Some(arg) = iter.next() {
        if arg == "--input-format" && iter.peek().is_some_and(|next| next == "stream-json") {
            let _ = iter.next();
            continue;
        }
        filtered.push(arg);
    }
    filtered
}

pub fn run_attached(command: &HarnessCommand) -> Result<PtyRunResult> {
    let pty_system = native_pty_system();
    run_attached_with_pty_system(command, pty_system.as_ref())
}

/// Run `command` on a PTY like `run_attached`, but capture the PTY output
/// instead of mirroring the raw bytes to stdout. Each captured chunk is
/// handed to `on_output` in order and is guaranteed valid UTF-8 (codepoints
/// split across reads are reassembled by `drain_detached_output`).
///
/// This is the `--stream-json` path for external harnesses without a native
/// machine-readable bridge: stdout must stay
/// JSONL-only, so the raw PTY output (ANSI escapes, prompts, partial lines)
/// is wrapped into `output` events by the caller rather than interleaving
/// with the frames (#307). Stdin is still forwarded to the PTY, matching
/// `run_attached`; raw terminal mode is never enabled because nothing is
/// echoed back to the caller's terminal.
#[cfg(not(windows))]
pub fn run_attached_captured(
    command: &HarnessCommand,
    mut on_output: Box<dyn FnMut(Vec<u8>) + Send + 'static>,
) -> Result<PtyRunResult> {
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(terminal_size())
        .context("failed to open PTY")?;
    let mut child = pair
        .slave
        .spawn_command(command.to_command_builder())
        .with_context(|| format!("failed to spawn harness `{}`", command.program()))?;

    drop(pair.slave);

    let mut reader = pair
        .master
        .try_clone_reader()
        .context("failed to clone PTY reader")?;
    let mut writer = pair
        .master
        .take_writer()
        .context("failed to open PTY writer")?;

    thread::spawn(move || {
        let mut stdin = io::stdin().lock();
        let _ = io::copy(&mut stdin, &mut writer);
    });

    // Drain on this thread until the child closes its end of the PTY; EOF
    // (or EIO on Linux) arrives when the child exits, so the wait below
    // returns promptly.
    drain_detached_output(&mut reader, Some(&mut on_output));

    Ok(wait_for_child(&mut child))
}

/// Run a one-shot harness directly on inherited stdio without allocating a
/// pseudo-terminal. Windows Codex `exec` is reliable in this mode while its
/// ConPTY child can stall before producing output. Inherited handles preserve
/// the caller's stdout/stderr stream exactly (including Coven's JSON framing).
#[cfg(windows)]
pub fn run_piped_attached(
    command: &HarnessCommand,
    merge_stderr_to_stdout: bool,
) -> Result<PtyRunResult> {
    let mut child = std::process::Command::new(&command.program)
        .args(&command.args)
        .current_dir(&command.cwd)
        .stdin(if command.stdin_prompt.is_some() {
            Stdio::piped()
        } else {
            Stdio::inherit()
        })
        // In stream mode Codex duplicates its final answer on stdout while
        // stderr carries the complete labeled transcript that Cave's filter
        // consumes. Keep only the transcript to avoid rendering it twice.
        .stdout(if merge_stderr_to_stdout {
            Stdio::null()
        } else {
            Stdio::inherit()
        })
        .stderr(if merge_stderr_to_stdout {
            Stdio::piped()
        } else {
            Stdio::inherit()
        })
        .spawn()
        .with_context(|| {
            format!(
                "failed to spawn harness `{}` in piped mode",
                command.program()
            )
        })?;
    write_stdin_prompt(&mut child, command.stdin_prompt.as_deref())?;

    // Codex on Windows writes its complete `exec` transcript (including the
    // final assistant response) to stderr. `coven run --stream-json` is a
    // stdout protocol consumed by Cave, so forward that transcript to stdout
    // for stream clients while continuing to drain it concurrently.
    let stderr_forwarder = child.stderr.take().map(|mut stderr| {
        thread::spawn(move || -> io::Result<()> {
            let stdout = io::stdout();
            let mut stdout = stdout.lock();
            io::copy(&mut stderr, &mut stdout)?;
            stdout.flush()
        })
    });

    let status = child.wait().context("failed waiting for piped harness")?;
    if let Some(forwarder) = stderr_forwarder {
        forwarder
            .join()
            .map_err(|_| anyhow::anyhow!("stderr forwarding thread panicked"))?
            .context("failed forwarding harness stderr to stdout")?;
    }
    Ok(PtyRunResult {
        status: if status.success() {
            "completed"
        } else {
            "failed"
        },
        exit_code: status.code(),
    })
}

/// Run a one-shot Windows harness through ordinary pipes while keeping stdout
/// available for Coven's stream-JSON protocol. Codex writes its labeled
/// transcript to stderr, so capture that stream and let the caller wrap it in
/// JSON `output` events; discard Codex's duplicate plain stdout answer.
#[cfg(windows)]
pub fn run_piped_attached_captured(
    command: &HarnessCommand,
    mut on_output: Box<dyn FnMut(Vec<u8>) + Send + 'static>,
) -> Result<PtyRunResult> {
    let mut child = std::process::Command::new(&command.program)
        .args(&command.args)
        .current_dir(&command.cwd)
        .stdin(if command.stdin_prompt.is_some() {
            Stdio::piped()
        } else {
            Stdio::inherit()
        })
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| {
            format!(
                "failed to spawn harness `{}` in captured piped mode",
                command.program()
            )
        })?;
    write_stdin_prompt(&mut child, command.stdin_prompt.as_deref())?;
    let mut stderr = child
        .stderr
        .take()
        .context("captured piped harness did not expose stderr")?;
    drain_detached_output(&mut stderr, Some(&mut on_output));
    let status = child.wait().context("failed waiting for piped harness")?;
    Ok(PtyRunResult {
        status: if status.success() {
            "completed"
        } else {
            "failed"
        },
        exit_code: status.code(),
    })
}

/// Run a harness in its native stream-JSON mode, framed by the caller (which
/// emits Coven's own `system.init` / `result` around the call). The command's
/// argv is built from the harness declaration (`stream_args`, continuity,
/// model, sandbox, and identity handling); this runner only spawns it and
/// forwards each non-empty JSONL line unchanged to `out`.
pub fn stream_harness<W: Write>(
    command: &HarnessCommand,
    forward_stdin: bool,
    harness_id: &str,
    out: &mut W,
) -> Result<i32> {
    stream_harness_with_program(
        &command.program,
        &command.cwd,
        command.args.clone(),
        forward_stdin,
        harness_id,
        out,
    )
}

fn stream_harness_with_program<W: Write>(
    program: &str,
    cwd: &Path,
    args: Vec<String>,
    forward_stdin: bool,
    harness_id: &str,
    out: &mut W,
) -> Result<i32> {
    let mut child = std::process::Command::new(program)
        .args(&args)
        .current_dir(cwd)
        .stdin(if forward_stdin {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .with_context(|| format!("failed to spawn {harness_id} in stream-json mode"))?;

    if forward_stdin {
        let Some(mut child_stdin) = child.stdin.take() else {
            anyhow::bail!("stdin requested but {harness_id} has no piped stdin");
        };
        thread::spawn(move || {
            let stdin = io::stdin();
            let mut handle = stdin.lock();
            let mut buf = String::new();
            loop {
                buf.clear();
                match handle.read_line(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {
                        if child_stdin.write_all(buf.as_bytes()).is_err() {
                            break;
                        }
                        let _ = child_stdin.flush();
                    }
                }
            }
        });
    }

    let Some(child_stdout) = child.stdout.take() else {
        anyhow::bail!("stdout requested but {harness_id} has no piped stdout");
    };
    let reader = BufReader::new(child_stdout);
    for line in reader.lines() {
        let line = line.with_context(|| format!("reading {harness_id} stdout"))?;
        if line.trim().is_empty() {
            continue;
        }
        writeln!(out, "{line}").with_context(|| format!("forwarding {harness_id} stdout"))?;
        out.flush()
            .with_context(|| format!("flushing {harness_id} stdout"))?;
    }

    let status = child
        .wait()
        .with_context(|| format!("waiting on {harness_id}"))?;
    Ok(status.code().unwrap_or(1))
}

#[cfg(test)]
#[allow(clippy::too_many_arguments)]
fn stream_harness_with_claude_args<W: Write>(
    program: &str,
    cwd: &Path,
    session_id: &str,
    is_resume: bool,
    prompt: &str,
    forward_stdin: bool,
    system_prompt: Option<&str>,
    options: crate::harness::HarnessLaunchOptions<'_>,
    out: &mut W,
) -> Result<i32> {
    stream_harness_with_claude_args_and_permission_bypass(
        program,
        cwd,
        session_id,
        is_resume,
        prompt,
        forward_stdin,
        system_prompt,
        options,
        false,
        out,
    )
}

#[cfg(test)]
#[allow(clippy::too_many_arguments)]
fn stream_harness_with_claude_args_and_permission_bypass<W: Write>(
    program: &str,
    cwd: &Path,
    session_id: &str,
    is_resume: bool,
    prompt: &str,
    forward_stdin: bool,
    system_prompt: Option<&str>,
    options: crate::harness::HarnessLaunchOptions<'_>,
    permission_bypass_enabled: bool,
    out: &mut W,
) -> Result<i32> {
    let normalized_model = options
        .model
        .map(str::trim)
        .filter(|m| !m.is_empty())
        .map(crate::harness::normalize_model_id);

    let mut args = vec!["-p".to_string()];
    if permission_bypass_enabled {
        args.extend([
            "--permission-mode".to_string(),
            "bypassPermissions".to_string(),
        ]);
    }
    if forward_stdin {
        args.extend(["--input-format".to_string(), "stream-json".to_string()]);
    }
    if let Some(sp) = system_prompt {
        args.extend(["--system-prompt".to_string(), sp.to_string()]);
    }
    if let Some(model) = normalized_model {
        args.extend(["--model".to_string(), model.to_string()]);
    }
    if let Some(effort) = options.claude_effort() {
        args.extend(["--effort".to_string(), effort.to_string()]);
    }
    args.extend([
        "--output-format".to_string(),
        "stream-json".to_string(),
        "--verbose".to_string(),
    ]);
    if is_resume {
        args.extend(["--resume".to_string(), session_id.to_string()]);
    } else {
        args.extend(["--session-id".to_string(), session_id.to_string()]);
    }
    args.extend(["--".to_string(), prompt.to_string()]);

    stream_harness_with_program(program, cwd, args, forward_stdin, "claude", out)
}

#[allow(dead_code)]
pub fn spawn_detached(command: &HarnessCommand) -> Result<DetachedPtySession> {
    spawn_detached_with_observer(command, None)
}

/// Handle returned by `spawn_piped_with_observer`. The child handle itself
/// is owned by the internal wait thread (so `wait()` can block without
/// blocking the killer); the caller gets a writable stdin and the PID so
/// it can signal termination via `libc::kill` instead of needing exclusive
/// access to the `Child`.
pub struct PipedSession {
    pub input: Box<dyn Write + Send>,
    pub pid: u32,
}

/// Spawn `command` as a plain piped child process (no PTY) and stream its
/// stdout to `observer`. Used by stream-mode harness launches where the
/// child reads newline-delimited JSON from stdin and writes
/// newline-delimited JSON to stdout — wrapping in a PTY would add ANSI
/// escapes the child wouldn't otherwise emit. Lifecycle mirrors
/// `spawn_detached_with_observer`: a background thread drains stdout and
/// fires `on_exit` when the child finishes. Stderr is line-buffered and
/// forwarded to `observer.on_output` wrapped in a stream-json
/// `{"type":"system","subtype":"stderr","text":"…"}` envelope so chat
/// surfaces auth/setup errors instead of swallowing them.
pub fn spawn_piped_with_observer(
    command: &HarnessCommand,
    observer: Option<DetachedPtyObserver>,
    wrap_stderr_as_stream_json: bool,
) -> Result<PipedSession> {
    use std::process::Command as StdCommand;
    use std::sync::{Arc, Mutex as StdMutex};

    let mut std_command = StdCommand::new(&command.program);
    std_command.args(&command.args);
    std_command.current_dir(&command.cwd);
    std_command.stdin(Stdio::piped());
    std_command.stdout(Stdio::piped());
    std_command.stderr(Stdio::piped());
    // Put the child in its own session/process group so the daemon can
    // signal it (and any subprocesses it spawns — skills, MCP servers,
    // shells) as a single unit via `kill(-pid, …)` from `PipedKiller`.
    // Without this, signals to the pid only reach the immediate child
    // and leave grandchildren as orphans.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        unsafe {
            std_command.pre_exec(|| {
                // setsid() makes the calling process the session leader
                // of a new session AND the leader of a new process
                // group with no controlling terminal. Returns -1 on
                // failure (we propagate as io::Error to abort the spawn).
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }

    let mut child = std_command.spawn().with_context(|| {
        format!(
            "failed to spawn harness `{}` in piped mode",
            command.program
        )
    })?;

    let pid = child.id();
    let mut stdin = child
        .stdin
        .take()
        .context("failed to take child stdin in piped mode")?;
    let stdin: Box<dyn Write + Send> = if let Some(prompt) = command.stdin_prompt.as_deref() {
        if let Err(error) = stdin.write_all(prompt).and_then(|_| stdin.flush()) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error).context("failed writing harness prompt to stdin");
        }
        drop(stdin);
        Box::new(io::sink())
    } else {
        Box::new(stdin)
    };
    let stdout = child
        .stdout
        .take()
        .context("failed to take child stdout in piped mode")?;
    let stderr = child
        .stderr
        .take()
        .context("failed to take child stderr in piped mode")?;

    // Share the on_output callback between the stdout and stderr drain
    // threads — both want to feed the same observer pipeline. `on_exit` is
    // moved into the stdout thread (it fires exactly once when the child
    // exits). If no observer was supplied, both callbacks are no-ops.
    let DetachedPtyObserver { on_output, on_exit } = observer.unwrap_or(DetachedPtyObserver {
        on_output: Box::new(|_| {}),
        on_exit: Box::new(|_| {}),
    });
    let on_output_shared = Arc::new(StdMutex::new(on_output));

    // Stderr drain: line-buffered, wrapped in a stream-json system
    // envelope so chat can render auth/setup messages as system lines
    // rather than dropping them silently. Reads raw bytes with
    // `read_until(b'\n')` + `from_utf8_lossy` so non-UTF-8 stderr
    // (rare but seen in some sandboxed environments) doesn't truncate
    // the stream at the first decode error — which `BufRead::lines()`
    // would do.
    let stderr_callback = Arc::clone(&on_output_shared);
    thread::spawn(move || {
        let mut reader = BufReader::new(stderr);
        let mut buf: Vec<u8> = Vec::with_capacity(256);
        loop {
            buf.clear();
            match reader.read_until(b'\n', &mut buf) {
                Ok(0) => break, // EOF
                Ok(_) => {
                    // Strip the trailing newline (if any) for cleaner
                    // display; the JSON envelope adds its own.
                    let trimmed = match buf.last() {
                        Some(b'\n') => &buf[..buf.len() - 1],
                        _ => &buf[..],
                    };
                    let line = String::from_utf8_lossy(trimmed);
                    let mut payload = if wrap_stderr_as_stream_json {
                        serde_json::json!({
                            "type": "system",
                            "subtype": "stderr",
                            "text": line,
                        })
                        .to_string()
                    } else {
                        line.into_owned()
                    };
                    payload.push('\n');
                    if let Ok(mut cb) = stderr_callback.lock() {
                        cb(payload.into_bytes());
                    }
                }
                Err(_) => break,
            }
        }
    });

    // Stdout drain + wait. The wait thread OWNS `child`; the killer never
    // touches the `Child` handle, only the PID. That removes the previous
    // deadlock risk where `wait()` and `kill()` raced on a shared mutex.
    let stdout_callback = Arc::clone(&on_output_shared);
    thread::spawn(move || {
        let mut reader = stdout;
        let mut bridge: Box<dyn FnMut(Vec<u8>) + Send + 'static> = Box::new(move |chunk| {
            if let Ok(mut cb) = stdout_callback.lock() {
                cb(chunk);
            }
        });
        drain_detached_output(&mut reader, Some(&mut bridge));
        let result = match child.wait() {
            Ok(status) => PtyRunResult {
                status: if status.success() {
                    "completed"
                } else {
                    "failed"
                },
                exit_code: status.code(),
            },
            Err(_) => PtyRunResult {
                status: "failed",
                exit_code: None,
            },
        };
        on_exit(result);
    });

    Ok(PipedSession { input: stdin, pid })
}

pub fn spawn_detached_with_observer(
    command: &HarnessCommand,
    observer: Option<DetachedPtyObserver>,
) -> Result<DetachedPtySession> {
    spawn_detached_with_observer_and_timeout(command, observer, detached_startup_timeout())
}

fn spawn_detached_with_observer_and_timeout(
    command: &HarnessCommand,
    observer: Option<DetachedPtyObserver>,
    startup_timeout: Option<Duration>,
) -> Result<DetachedPtySession> {
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(terminal_size())
        .context("failed to open PTY")?;
    let portable_pty::PtyPair { master, slave } = pair;
    let mut child = slave
        .spawn_command(command.to_command_builder())
        .with_context(|| format!("failed to spawn harness `{}`", command.program()))?;
    drop(slave);

    let mut reader = master
        .try_clone_reader()
        .context("failed to clone PTY reader")?;
    let writer = master.take_writer().context("failed to open PTY writer")?;
    let shared_writer = spawn_shared_pty_writer(writer);
    let input: Box<dyn Write + Send> = Box::new(shared_writer.clone());
    let killer = shared_pty_killer(child.as_ref());
    let timeout_killer = killer.clone_killer();

    // 0 = waiting for meaningful output, 1 = output or exit observed,
    // 2 = startup timeout won the race. VT queries do not count because the
    // filter consumes them before this state is advanced.
    let startup_state = Arc::new(AtomicU8::new(0));
    let DetachedPtyObserver { on_output, on_exit } = observer.unwrap_or(DetachedPtyObserver {
        on_output: Box::new(|_| {}),
        on_exit: Box::new(|_| {}),
    });
    let on_output = Arc::new(Mutex::new(on_output));

    if let Some(startup_timeout) = startup_timeout {
        let timeout_state = Arc::clone(&startup_state);
        let timeout_output = Arc::clone(&on_output);
        thread::spawn(move || {
            thread::sleep(startup_timeout);
            if timeout_state
                .compare_exchange(0, 2, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                if let Ok(mut callback) = timeout_output.lock() {
                    callback(
                        format!(
                            "Coven stopped the detached PTY: no meaningful output was produced before the startup timeout ({} ms).\n",
                            startup_timeout.as_millis()
                        )
                        .into_bytes(),
                    );
                }
                let mut timeout_killer = timeout_killer;
                let _ = timeout_killer.kill();
            }
        });
    }

    let (child_exit_tx, child_exit_rx) = mpsc::channel();
    let child_exit_state = Arc::clone(&startup_state);
    thread::spawn(move || {
        // The cloned read/write pipe handles do not own the Windows HPCON.
        // Keep the MasterPty alive until the child exits; dropping it when
        // this function returned was the source of intermittent 0x7fffffff
        // ConPTY exits with no output (#329).
        let _master = master;
        let result = wait_for_child(&mut child);
        child_exit_state
            .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
            .ok();
        drop(_master);
        let _ = child_exit_tx.send(result);
    });

    thread::spawn(move || {
        let output_state = Arc::clone(&startup_state);
        let output_callback = Arc::clone(&on_output);
        let mut meaningful_detector = MeaningfulOutputDetector::default();
        let mut bridge: Box<dyn FnMut(Vec<u8>) + Send + 'static> = Box::new(move |chunk| {
            if meaningful_detector.push(&chunk) {
                output_state
                    .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
                    .ok();
            }
            if let Ok(mut callback) = output_callback.lock() {
                callback(chunk);
            }
        });
        let mut terminal_reply = |reply| shared_writer.queue_terminal_reply(reply);
        drain_detached_pty_output(&mut reader, &mut terminal_reply, Some(&mut bridge));
        let mut result = child_exit_rx.recv().unwrap_or(PtyRunResult {
            status: "failed",
            exit_code: None,
        });
        let previous = startup_state.swap(1, Ordering::AcqRel);
        if previous == 2 {
            result = PtyRunResult {
                status: "failed",
                exit_code: None,
            };
        }
        on_exit(result);
    });

    Ok(DetachedPtySession {
        input,
        killer: Box::new(killer),
    })
}

#[cfg(all(test, windows))]
pub(crate) fn spawn_detached_with_observer_for_test(
    command: &HarnessCommand,
    observer: DetachedPtyObserver,
    startup_timeout: Duration,
) -> Result<DetachedPtySession> {
    spawn_detached_with_observer_and_timeout(command, Some(observer), Some(startup_timeout))
}

#[cfg(all(test, windows))]
pub(crate) fn windows_detached_stub_command(
    build_dir: &Path,
    mode: &str,
    auxiliary_file: Option<&Path>,
) -> Result<HarnessCommand> {
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/windows_detached_pty_stub.rs");
    let executable = build_dir.join("windows-detached-pty-stub.exe");
    let compile = std::process::Command::new("rustc.exe")
        .args(["--edition=2021", "-o"])
        .arg(&executable)
        .arg(&source)
        .output()
        .context("failed to compile native Windows detached-PTY stub")?;
    anyhow::ensure!(
        compile.status.success(),
        "native Windows detached-PTY stub failed to compile: {}",
        String::from_utf8_lossy(&compile.stderr)
    );
    let mut args = vec![mode.to_string()];
    if let Some(auxiliary_file) = auxiliary_file {
        args.push(auxiliary_file.to_string_lossy().into_owned());
    }
    Ok(HarnessCommand {
        program: executable.to_string_lossy().into_owned(),
        args,
        cwd: PathBuf::from(env!("CARGO_MANIFEST_DIR")),
        stdin_prompt: None,
    })
}

fn detached_startup_timeout() -> Option<Duration> {
    #[cfg(not(windows))]
    {
        None
    }
    #[cfg(windows)]
    {
        if let Some(milliseconds) = std::env::var("COVEN_PTY_STARTUP_TIMEOUT_MS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
        {
            return Some(Duration::from_millis(milliseconds.max(1)));
        }
        Some(DETACHED_STARTUP_TIMEOUT)
    }
}

fn shared_pty_killer(child: &dyn portable_pty::Child) -> SharedPtyKiller {
    #[cfg(windows)]
    let job_handle = child.process_id().and_then(assign_process_to_job);
    SharedPtyKiller {
        inner: Arc::new(Mutex::new(PtyKillerInner {
            fallback: child.clone_killer(),
            #[cfg(windows)]
            job_handle,
        })),
    }
}

#[cfg(windows)]
fn assign_process_to_job(pid: u32) -> Option<windows_sys::Win32::Foundation::HANDLE> {
    use windows_sys::Win32::{
        Foundation::{CloseHandle, INVALID_HANDLE_VALUE},
        System::{
            JobObjects::{AssignProcessToJobObject, CreateJobObjectW},
            Threading::{OpenProcess, PROCESS_ALL_ACCESS},
        },
    };
    // SAFETY: all returned handles are checked and either closed here or
    // transferred to PtyKillerInner for exclusive ownership.
    unsafe {
        let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        if job == INVALID_HANDLE_VALUE || job == 0 as _ {
            return None;
        }
        let process = OpenProcess(PROCESS_ALL_ACCESS, 0, pid);
        if process == INVALID_HANDLE_VALUE || process == 0 as _ {
            CloseHandle(job);
            return None;
        }
        let assigned = AssignProcessToJobObject(job, process);
        CloseHandle(process);
        if assigned == 0 {
            CloseHandle(job);
            None
        } else {
            Some(job)
        }
    }
}

fn drain_detached_pty_output(
    reader: &mut dyn Read,
    terminal_reply: &mut dyn FnMut(&'static [u8]),
    on_output: Option<&mut Box<dyn FnMut(Vec<u8>) + Send + 'static>>,
) {
    let mut filter = VtQueryFilter::default();
    drain_detached_output_inner(reader, on_output, Some((&mut filter, terminal_reply)));
}

fn drain_detached_output(
    reader: &mut dyn Read,
    on_output: Option<&mut Box<dyn FnMut(Vec<u8>) + Send + 'static>>,
) {
    drain_detached_output_inner(reader, on_output, None);
}

type VtDrain<'a> = (&'a mut VtQueryFilter, &'a mut dyn FnMut(&'static [u8]));

fn drain_detached_output_inner(
    reader: &mut dyn Read,
    mut on_output: Option<&mut Box<dyn FnMut(Vec<u8>) + Send + 'static>>,
    mut vt: Option<VtDrain<'_>>,
) {
    let mut buffer = [0_u8; 8192];
    // Per-drain UTF-8 reassembly buffer. Each call to this function
    // owns its own buffer, so concurrent stdout+stderr drains in
    // `spawn_piped_with_observer` (which share an `on_output` via
    // Arc<Mutex>) can't corrupt each other's codepoint state. Each
    // chunk we hand to the callback is guaranteed valid UTF-8.
    let mut utf8_buf: Vec<u8> = Vec::with_capacity(8192);
    loop {
        match reader.read(&mut buffer) {
            Ok(0) => {
                if let Some((filter, _)) = vt.as_mut() {
                    filter.finish(&mut utf8_buf);
                }
                // EOF: flush any trailing bytes (lossy if the stream
                // ended mid-codepoint — better to surface garbled
                // glyphs than drop the final message entirely).
                if !utf8_buf.is_empty() {
                    if let Some(callback) = on_output.as_deref_mut() {
                        let text = String::from_utf8_lossy(&utf8_buf).into_owned();
                        callback(text.into_bytes());
                    }
                }
                break;
            }
            Ok(bytes_read) => {
                if let Some((filter, terminal_reply)) = vt.as_mut() {
                    filter.push(&buffer[..bytes_read], *terminal_reply, &mut utf8_buf);
                } else {
                    utf8_buf.extend_from_slice(&buffer[..bytes_read]);
                }
                // Emit the longest valid-UTF-8 prefix; keep the trailing
                // partial codepoint in the buffer for the next read.
                let valid_up_to = match std::str::from_utf8(&utf8_buf) {
                    Ok(_) => utf8_buf.len(),
                    Err(error) => error.valid_up_to(),
                };
                if valid_up_to > 0 {
                    let prefix: Vec<u8> = utf8_buf.drain(..valid_up_to).collect();
                    if let Some(callback) = on_output.as_deref_mut() {
                        callback(prefix);
                    }
                }
                // Pathological tail: if the remaining bytes can't be a
                // partial codepoint (>4 bytes — max UTF-8 codepoint
                // length), the stream is genuinely malformed. Drop one
                // byte at a time via lossy decode so we make progress
                // instead of buffering forever.
                while utf8_buf.len() > 4
                    && std::str::from_utf8(&utf8_buf)
                        .err()
                        .map(|e| e.valid_up_to())
                        == Some(0)
                {
                    let dropped: Vec<u8> = utf8_buf.drain(..1).collect();
                    if let Some(callback) = on_output.as_deref_mut() {
                        let lossy = String::from_utf8_lossy(&dropped).into_owned();
                        callback(lossy.into_bytes());
                    }
                }
            }
            Err(_) => break,
        }
    }
}

#[derive(Default)]
struct VtQueryFilter {
    pending: Vec<u8>,
}

impl VtQueryFilter {
    fn push(
        &mut self,
        chunk: &[u8],
        terminal_reply: &mut dyn FnMut(&'static [u8]),
        output: &mut Vec<u8>,
    ) {
        self.pending.extend_from_slice(chunk);
        let mut offset = 0;
        while offset < self.pending.len() {
            let Some(relative_escape) =
                self.pending[offset..].iter().position(|byte| *byte == 0x1b)
            else {
                output.extend_from_slice(&self.pending[offset..]);
                offset = self.pending.len();
                break;
            };
            let escape = offset + relative_escape;
            output.extend_from_slice(&self.pending[offset..escape]);
            let remaining = &self.pending[escape..];
            if let Some((query_len, reply)) = vt_query_reply(remaining) {
                terminal_reply(reply);
                offset = escape + query_len;
            } else if VT_QUERIES
                .iter()
                .any(|(query, _)| query.starts_with(remaining))
            {
                offset = escape;
                break;
            } else {
                output.push(0x1b);
                offset = escape + 1;
            }
        }
        self.pending.drain(..offset);
    }

    fn finish(&mut self, output: &mut Vec<u8>) {
        output.append(&mut self.pending);
    }
}

const VT_QUERIES: [(&[u8], &[u8]); 4] = [
    (b"\x1b[6n", b"\x1b[1;1R"),
    (b"\x1b[c", b"\x1b[?62;c"),
    (b"\x1b[0c", b"\x1b[?62;c"),
    (b"\x1b[5n", b"\x1b[0n"),
];

fn vt_query_reply(bytes: &[u8]) -> Option<(usize, &'static [u8])> {
    VT_QUERIES
        .iter()
        .find(|(query, _)| bytes.starts_with(query))
        .map(|(query, reply)| (query.len(), *reply))
}

#[derive(Default)]
struct MeaningfulOutputDetector {
    state: EscapeState,
}

#[derive(Default, Clone, Copy)]
enum EscapeState {
    #[default]
    Ground,
    Escape,
    EscapeIntermediate,
    Csi,
    String,
    StringEscape,
}

impl MeaningfulOutputDetector {
    fn push(&mut self, bytes: &[u8]) -> bool {
        let mut meaningful = false;
        for byte in bytes {
            self.state = match self.state {
                EscapeState::Ground if *byte == 0x1b => EscapeState::Escape,
                EscapeState::Ground => {
                    // Whitespace and C0 controls do not prove that a harness
                    // reached a usable prompt. Printable ASCII or UTF-8 does.
                    meaningful |= *byte >= 0x80 || (*byte > 0x20 && *byte != 0x7f);
                    EscapeState::Ground
                }
                EscapeState::Escape if *byte == b'[' => EscapeState::Csi,
                EscapeState::Escape if matches!(*byte, b']' | b'P' | b'^' | b'_') => {
                    EscapeState::String
                }
                EscapeState::Escape if (0x20..=0x2f).contains(byte) => {
                    EscapeState::EscapeIntermediate
                }
                EscapeState::Escape if *byte == 0x1b => EscapeState::Escape,
                EscapeState::Escape => EscapeState::Ground,
                EscapeState::EscapeIntermediate if (0x20..=0x2f).contains(byte) => {
                    EscapeState::EscapeIntermediate
                }
                EscapeState::EscapeIntermediate if *byte == 0x1b => EscapeState::Escape,
                EscapeState::EscapeIntermediate => EscapeState::Ground,
                EscapeState::Csi if *byte == 0x1b => EscapeState::Escape,
                EscapeState::Csi if (0x40..=0x7e).contains(byte) => EscapeState::Ground,
                EscapeState::Csi => EscapeState::Csi,
                EscapeState::String if *byte == 0x07 => EscapeState::Ground,
                EscapeState::String if *byte == 0x1b => EscapeState::StringEscape,
                EscapeState::String => EscapeState::String,
                EscapeState::StringEscape if *byte == b'\\' => EscapeState::Ground,
                EscapeState::StringEscape => EscapeState::String,
            };
        }
        meaningful
    }
}

fn wait_for_child(child: &mut Box<dyn portable_pty::Child + Send + Sync>) -> PtyRunResult {
    match child.wait() {
        Ok(exit_status) => {
            let exit_code = i32::try_from(exit_status.exit_code()).unwrap_or(i32::MAX);
            let status = if exit_status.success() {
                "completed"
            } else {
                "failed"
            };
            PtyRunResult {
                status,
                exit_code: Some(exit_code),
            }
        }
        Err(_) => PtyRunResult {
            status: "failed",
            exit_code: None,
        },
    }
}

fn run_attached_with_pty_system(
    command: &HarnessCommand,
    pty_system: &(dyn PtySystem + Send),
) -> Result<PtyRunResult> {
    let pair = pty_system
        .openpty(terminal_size())
        .context("failed to open PTY")?;
    let mut child = pair
        .slave
        .spawn_command(command.to_command_builder())
        .with_context(|| format!("failed to spawn harness `{}`", command.program()))?;

    drop(pair.slave);

    let mut reader = pair
        .master
        .try_clone_reader()
        .context("failed to clone PTY reader")?;
    let mut writer = pair
        .master
        .take_writer()
        .context("failed to open PTY writer")?;
    let _raw_mode =
        RawModeGuard::enable_if_terminal().context("failed to enable raw terminal mode")?;

    let output_thread = thread::spawn(move || {
        let mut stdout = io::stdout().lock();
        io::copy(&mut reader, &mut stdout)?;
        stdout.flush()
    });

    // Only forward stdin to the PTY when it is an interactive terminal. A
    // one-shot `coven run` gets its prompt from argv, so a piped or
    // redirected stdin carries nothing the harness needs — and copying it
    // into the PTY makes the line discipline echo the EOF as a visible `^D`
    // in the captured output. Interactive sessions still need the forward so
    // the user can type.
    if io::stdin().is_terminal() {
        thread::spawn(move || {
            let mut stdin = io::stdin().lock();
            let _ = io::copy(&mut stdin, &mut writer);
        });
    }

    let exit_status = child.wait().context("failed to wait for harness process")?;
    let _ = output_thread.join();
    let exit_code = i32::try_from(exit_status.exit_code()).unwrap_or(i32::MAX);
    let status = if exit_status.success() {
        "completed"
    } else {
        "failed"
    };

    Ok(PtyRunResult {
        status,
        exit_code: Some(exit_code),
    })
}

struct RawModeGuard {
    enabled: bool,
}

impl RawModeGuard {
    fn enable_if_terminal() -> Result<Self> {
        let enabled = io::stdin().is_terminal() && io::stdout().is_terminal();
        if enabled {
            enable_raw_mode()?;
        }
        Ok(Self { enabled })
    }
}

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        if self.enabled {
            let _ = disable_raw_mode();
        }
    }
}

fn terminal_size() -> PtySize {
    PtySize {
        rows: env_u16("LINES").unwrap_or(24),
        cols: env_u16("COLUMNS").unwrap_or(80),
        pixel_width: 0,
        pixel_height: 0,
    }
}

fn env_u16(name: &str) -> Option<u16> {
    std::env::var(name)
        .ok()?
        .parse()
        .ok()
        .filter(|value| *value > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_codex_command_without_shell_interpolation() {
        let cwd = Path::new("/tmp/coven project");
        let command = build_harness_command(
            "codex",
            "hello; rm -rf /",
            cwd,
            crate::harness::HarnessLaunchMode::Interactive,
        )
        .unwrap();

        assert_eq!(command.program(), "codex");
        assert_eq!(command.args(), &["--", "hello; rm -rf /"]);
        assert_eq!(command.cwd(), cwd);
    }

    #[cfg(unix)]
    #[test]
    fn spawn_detached_starts_pty_and_returns_input_and_kill_handles() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let command = HarnessCommand {
            program: "cat".to_string(),
            args: vec![],
            cwd: temp_dir.path().to_path_buf(),
            stdin_prompt: None,
        };

        let mut session = spawn_detached(&command)?;
        session.input.write_all(b"hello detached pty\n")?;
        session.input.flush()?;
        session.killer.kill()?;
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn detached_startup_timeout_is_disabled() {
        assert_eq!(detached_startup_timeout(), None);
    }

    #[cfg(windows)]
    #[test]
    fn windows_detached_pty_stub_completes_after_terminal_replies() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let trace_file = temp_dir.path().join("query-trace.txt");
        let command = windows_detached_stub_command(temp_dir.path(), "queries", Some(&trace_file))?;
        let captured = Arc::new(Mutex::new(Vec::new()));
        let captured_for_output = Arc::clone(&captured);
        let (exit_tx, exit_rx) = mpsc::channel();
        let observer = DetachedPtyObserver {
            on_output: Box::new(move |chunk| {
                captured_for_output.lock().unwrap().extend(chunk);
            }),
            on_exit: Box::new(move |result| {
                let _ = exit_tx.send(result);
            }),
        };

        let mut session = spawn_detached_with_observer_and_timeout(
            &command,
            Some(observer),
            Some(Duration::from_secs(5)),
        )?;
        let result = match exit_rx.recv_timeout(Duration::from_secs(10)) {
            Ok(result) => result,
            Err(error) => {
                let _ = session.killer.kill();
                anyhow::bail!(
                    "{error}; trace: {:?}; observed: {:?}",
                    std::fs::read_to_string(&trace_file),
                    String::from_utf8_lossy(&captured.lock().unwrap())
                );
            }
        };

        let observed = String::from_utf8_lossy(&captured.lock().unwrap()).into_owned();
        let trace = std::fs::read_to_string(&trace_file).unwrap_or_default();
        assert_eq!(
            result.status, "completed",
            "result: {result:?}; observed output: {observed:?}; trace: {trace:?}"
        );
        assert_eq!(result.exit_code, Some(0));
        assert!(observed.contains("WINDOWS_PTY_STUB_OK_🎉"), "{observed:?}");
        for query in ["\x1b[6n", "\x1b[c", "\x1b[0c", "\x1b[5n"] {
            assert!(!observed.contains(query), "query leaked: {query:?}");
        }
        assert!(trace.starts_with("started mode="), "{trace:?}");
        for stage in ["cpr", "da", "status", "da0"] {
            assert!(trace.lines().any(|line| line == stage), "{trace:?}");
        }
        Ok(())
    }

    #[cfg(windows)]
    #[test]
    fn windows_detached_pty_timeout_fails_and_kills_descendant() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let pid_file = temp_dir.path().join("descendant.pid");
        let command = windows_detached_stub_command(temp_dir.path(), "timeout", Some(&pid_file))?;
        let captured = Arc::new(Mutex::new(Vec::new()));
        let captured_for_output = Arc::clone(&captured);
        let (exit_tx, exit_rx) = mpsc::channel();
        let observer = DetachedPtyObserver {
            on_output: Box::new(move |chunk| {
                captured_for_output.lock().unwrap().extend(chunk);
            }),
            on_exit: Box::new(move |result| {
                let _ = exit_tx.send(result);
            }),
        };

        let mut session = spawn_detached_with_observer_and_timeout(
            &command,
            Some(observer),
            Some(Duration::from_secs(2)),
        )?;
        let result = match exit_rx.recv_timeout(Duration::from_secs(10)) {
            Ok(result) => result,
            Err(error) => {
                let _ = session.killer.kill();
                return Err(error.into());
            }
        };
        let descendant_pid: u32 = std::fs::read_to_string(&pid_file)
            .with_context(|| {
                format!(
                    "timeout stub did not create pid file; observed output: {:?}",
                    String::from_utf8_lossy(&captured.lock().unwrap())
                )
            })?
            .trim()
            .parse()?;

        assert_eq!(result.status, "failed");
        assert_eq!(result.exit_code, None);
        let output = String::from_utf8(captured.lock().unwrap().clone())?;
        assert!(output.contains("no meaningful output"), "{output:?}");
        assert!(!output.contains("\x1b[6n"), "query leaked: {output:?}");
        assert!(
            wait_for_windows_process_exit(descendant_pid, Duration::from_secs(3)),
            "startup timeout left descendant process {descendant_pid} running"
        );
        Ok(())
    }

    #[cfg(windows)]
    fn wait_for_windows_process_exit(pid: u32, timeout: Duration) -> bool {
        use windows_sys::Win32::{
            Foundation::{CloseHandle, WAIT_OBJECT_0},
            System::Threading::{OpenProcess, WaitForSingleObject},
        };
        // SAFETY: the process handle is checked and closed exactly once.
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

    /// Serializes the fake-claude tests: each writes an executable script and
    /// immediately spawns it. Run in parallel, one test's `fork` can inherit
    /// another's still-open write fd and the exec fails with ETXTBSY
    /// ("Text file busy") — a real CI flake, not a theoretical one.
    #[cfg(unix)]
    static FAKE_CLAUDE_SPAWN_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[cfg(unix)]
    fn fake_claude_spawn_guard() -> std::sync::MutexGuard<'static, ()> {
        FAKE_CLAUDE_SPAWN_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    #[cfg(unix)]
    #[test]
    fn codex_json_runner_normalizes_agent_message_and_captures_thread() -> anyhow::Result<()> {
        use std::os::unix::fs::PermissionsExt;

        let _guard = fake_claude_spawn_guard();
        let temp_dir = tempfile::tempdir()?;
        let fake_codex = temp_dir.path().join("fake-codex");
        std::fs::write(
            &fake_codex,
            r#"#!/bin/sh
printf '%s\n' "$@" > args.txt
printf '%s\n' '{"type":"thread.started","thread_id":"thread-123"}'
printf '%s\n' '{"type":"turn.started"}'
printf '%s\n' '{"type":"item.completed","item":{"id":"item-1","type":"agent_message","text":"Coven reply"}}'
printf '%s\n' '{"type":"turn.completed"}'
"#,
        )?;
        let mut permissions = std::fs::metadata(&fake_codex)?.permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&fake_codex, permissions)?;
        let command = HarnessCommand {
            program: fake_codex.to_string_lossy().into_owned(),
            args: vec![
                "exec".to_string(),
                "--json".to_string(),
                "--model".to_string(),
                "gpt-5.5".to_string(),
                "--".to_string(),
                "reply exactly once".to_string(),
            ],
            cwd: temp_dir.path().to_path_buf(),
            stdin_prompt: None,
        };
        let mut assistant = Vec::new();

        let outcome = stream_codex_json_with_timeout(&command, Duration::from_secs(1), |text| {
            assistant.push(text.to_string());
            Ok(())
        })?;

        assert_eq!(
            std::fs::read_to_string(temp_dir.path().join("args.txt"))?,
            "exec\n--json\n--model\ngpt-5.5\n--\nreply exactly once\n"
        );
        assert_eq!(assistant, vec!["Coven reply"]);
        assert_eq!(outcome.harness_session_id.as_deref(), Some("thread-123"));
        assert!(outcome.emitted_assistant);
        assert!(outcome.error.is_none());
        assert_eq!(outcome.process.exit_code, Some(0));
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn codex_json_runner_times_out_and_reaps_a_silent_child() -> anyhow::Result<()> {
        use std::os::unix::fs::PermissionsExt;

        let _guard = fake_claude_spawn_guard();
        let temp_dir = tempfile::tempdir()?;
        let fake_codex = temp_dir.path().join("fake-codex");
        std::fs::write(
            &fake_codex,
            r#"#!/bin/sh
echo $$ > child.pid
exec sleep 10
"#,
        )?;
        let mut permissions = std::fs::metadata(&fake_codex)?.permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&fake_codex, permissions)?;
        let command = HarnessCommand {
            program: fake_codex.to_string_lossy().into_owned(),
            args: vec![
                "exec".to_string(),
                "--json".to_string(),
                "--".to_string(),
                "prompt".to_string(),
            ],
            cwd: temp_dir.path().to_path_buf(),
            stdin_prompt: None,
        };

        // The activity budget must outlive shell startup so the script can
        // record its pid before the runner kills the group; a 25ms budget
        // loses that race deterministically on macOS (~180ms cold start).
        let started = Instant::now();
        let outcome = stream_codex_json_with_timeout(&command, Duration::from_secs(1), |_| Ok(()))?;

        assert!(started.elapsed() < Duration::from_secs(5));
        assert!(outcome
            .error
            .as_deref()
            .is_some_and(|error| error.contains("terminated")));
        let pid = std::fs::read_to_string(temp_dir.path().join("child.pid"))?;
        let pid = pid.trim();
        let alive = std::process::Command::new("kill")
            .args(["-0", pid])
            .stderr(Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false);
        assert!(!alive, "timed-out child {pid} should be reaped");
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn codex_json_runner_times_out_while_a_large_prompt_is_still_writing() -> anyhow::Result<()> {
        use std::os::unix::fs::PermissionsExt;

        let _guard = fake_claude_spawn_guard();
        let temp_dir = tempfile::tempdir()?;
        let fake_codex = temp_dir.path().join("silent-codex");
        std::fs::write(&fake_codex, "#!/bin/sh\nexec sleep 10\n")?;
        let mut permissions = std::fs::metadata(&fake_codex)?.permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&fake_codex, permissions)?;
        let command = HarnessCommand {
            program: fake_codex.to_string_lossy().into_owned(),
            args: vec![
                "exec".to_string(),
                "--json".to_string(),
                "--".to_string(),
                "-".to_string(),
            ],
            cwd: temp_dir.path().to_path_buf(),
            // Far larger than an anonymous-pipe buffer. A synchronous write
            // would block indefinitely because the fake harness never reads.
            stdin_prompt: Some(vec![b'x'; 1024 * 1024]),
        };

        let started = Instant::now();
        let outcome =
            stream_codex_json_with_timeout(&command, Duration::from_millis(25), |_| Ok(()))?;

        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(outcome
            .error
            .as_deref()
            .is_some_and(|error| error.contains("terminated")));
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn codex_json_runner_reaps_a_pipe_holding_descendant_after_wrapper_exit() -> anyhow::Result<()>
    {
        use std::os::unix::fs::PermissionsExt;

        let _guard = fake_claude_spawn_guard();
        let temp_dir = tempfile::tempdir()?;
        let fake_codex = temp_dir.path().join("wrapper-codex");
        std::fs::write(
            &fake_codex,
            r#"#!/bin/sh
sleep 10 &
echo $! > descendant.pid
exit 0
"#,
        )?;
        let mut permissions = std::fs::metadata(&fake_codex)?.permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&fake_codex, permissions)?;
        let command = HarnessCommand {
            program: fake_codex.to_string_lossy().into_owned(),
            args: vec![
                "exec".to_string(),
                "--json".to_string(),
                "--".to_string(),
                "prompt".to_string(),
            ],
            cwd: temp_dir.path().to_path_buf(),
            stdin_prompt: None,
        };

        let started = Instant::now();
        let outcome = stream_codex_json_with_timeouts(
            &command,
            Duration::from_secs(1),
            Duration::from_millis(25),
            |_| Ok(()),
        )?;

        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(outcome
            .error
            .as_deref()
            .is_some_and(|error| error.contains("pipes remained open")));
        let pid = std::fs::read_to_string(temp_dir.path().join("descendant.pid"))?;
        let pid = pid.trim();
        let mut alive = true;
        for _ in 0..20 {
            alive = std::process::Command::new("kill")
                .args(["-0", pid])
                .stderr(Stdio::null())
                .status()
                .map(|status| status.success())
                .unwrap_or(false);
            if !alive {
                break;
            }
            thread::sleep(Duration::from_millis(25));
        }
        assert!(
            !alive,
            "descendant {pid} should be reaped with its process group"
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn codex_json_runner_reaps_a_closed_pipe_descendant_after_wrapper_exit() -> anyhow::Result<()> {
        use std::os::unix::fs::PermissionsExt;

        let _guard = fake_claude_spawn_guard();
        let temp_dir = tempfile::tempdir()?;
        let fake_codex = temp_dir.path().join("wrapper-codex");
        std::fs::write(
            &fake_codex,
            r#"#!/bin/sh
sleep 10 </dev/null >/dev/null 2>&1 &
echo $! > descendant.pid
printf '%s\n' '{"type":"thread.started","thread_id":"thread-closed-pipe"}'
printf '%s\n' '{"type":"item.completed","item":{"type":"agent_message","text":"reply before wrapper failure"}}'
exit 23
"#,
        )?;
        let mut permissions = std::fs::metadata(&fake_codex)?.permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&fake_codex, permissions)?;
        let command = HarnessCommand {
            program: fake_codex.to_string_lossy().into_owned(),
            args: vec![
                "exec".to_string(),
                "--json".to_string(),
                "--".to_string(),
                "prompt".to_string(),
            ],
            cwd: temp_dir.path().to_path_buf(),
            stdin_prompt: None,
        };
        let mut assistant = Vec::new();

        let outcome = stream_codex_json_with_timeout(&command, Duration::from_secs(1), |text| {
            assistant.push(text.to_string());
            Ok(())
        })?;

        assert_eq!(assistant, vec!["reply before wrapper failure"]);
        assert_eq!(outcome.process.status, "failed");
        assert_eq!(outcome.process.exit_code, Some(23));
        assert!(outcome
            .error
            .as_deref()
            .is_some_and(|error| error.contains("Codex exited with 23")));
        let pid = std::fs::read_to_string(temp_dir.path().join("descendant.pid"))?;
        let pid = pid.trim();
        let mut alive = true;
        for _ in 0..20 {
            alive = std::process::Command::new("kill")
                .args(["-0", pid])
                .stderr(Stdio::null())
                .status()
                .map(|status| status.success())
                .unwrap_or(false);
            if !alive {
                break;
            }
            thread::sleep(Duration::from_millis(25));
        }
        assert!(
            !alive,
            "closed-pipe descendant {pid} should be reaped with its process group"
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn codex_json_runner_synthesizes_nonzero_exit_for_protocol_error() -> anyhow::Result<()> {
        use std::os::unix::fs::PermissionsExt;

        let _guard = fake_claude_spawn_guard();
        let temp_dir = tempfile::tempdir()?;
        let fake_codex = temp_dir.path().join("failed-codex");
        std::fs::write(
            &fake_codex,
            r#"#!/bin/sh
printf '%s\n' '{"type":"turn.failed","error":{"message":"fake turn failure"}}'
exit 0
"#,
        )?;
        let mut permissions = std::fs::metadata(&fake_codex)?.permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&fake_codex, permissions)?;
        let command = HarnessCommand {
            program: fake_codex.to_string_lossy().into_owned(),
            args: vec![
                "exec".to_string(),
                "--json".to_string(),
                "--".to_string(),
                "prompt".to_string(),
            ],
            cwd: temp_dir.path().to_path_buf(),
            stdin_prompt: None,
        };

        let outcome = stream_codex_json_with_timeout(&command, Duration::from_secs(1), |_| Ok(()))?;

        assert_eq!(outcome.process.status, "failed");
        assert_eq!(outcome.process.exit_code, Some(1));
        assert_eq!(outcome.error.as_deref(), Some("fake turn failure"));
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn stream_harness_claude_forwards_jsonl_and_returns_exit_code() -> anyhow::Result<()> {
        use std::os::unix::fs::PermissionsExt;

        let _guard = fake_claude_spawn_guard();
        let temp_dir = tempfile::tempdir()?;
        let fake_claude = temp_dir.path().join("fake-claude");
        std::fs::write(
            &fake_claude,
            r#"#!/bin/sh
printf '%s\n' "$@" > args.txt
printf '\n'
printf '%s\n' '{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"hello"}]},"session_id":"session-123","stop_reason":"end_turn"}'
exit 7
"#,
        )?;
        let mut permissions = std::fs::metadata(&fake_claude)?.permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&fake_claude, permissions)?;

        let mut out = Vec::new();
        let code = stream_harness_with_claude_args(
            fake_claude.to_str().unwrap(),
            temp_dir.path(),
            "session-123",
            false,
            "hello prompt",
            false,
            None,
            crate::harness::HarnessLaunchOptions::default(),
            &mut out,
        )?;

        assert_eq!(code, 7);
        // One-shot mode (forward_stdin=false): `--input-format stream-json`
        // is omitted so the positional prompt is honored. Including it
        // makes claude wait for JSONL on stdin and ignore the positional —
        // which is the bug this commit fixes.
        assert_eq!(
            std::fs::read_to_string(temp_dir.path().join("args.txt"))?,
            "-p\n--output-format\nstream-json\n--verbose\n--session-id\nsession-123\n--\nhello prompt\n"
        );
        assert_eq!(
            String::from_utf8(out)?,
            "{\"type\":\"assistant\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"hello\"}]},\"session_id\":\"session-123\",\"stop_reason\":\"end_turn\"}\n"
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn stream_harness_forwards_declared_args_without_claude_rebuild() -> anyhow::Result<()> {
        use std::os::unix::fs::PermissionsExt;

        let _guard = fake_claude_spawn_guard();
        let temp_dir = tempfile::tempdir()?;
        let fake_harness = temp_dir.path().join("fake-streamy");
        std::fs::write(
            &fake_harness,
            r#"#!/bin/sh
printf '%s\n' "$@" > args.txt
printf '%s\n' '{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"streamy"}]},"session_id":"session-123","stop_reason":"end_turn"}'
exit 0
"#,
        )?;
        let mut permissions = std::fs::metadata(&fake_harness)?.permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&fake_harness, permissions)?;

        let mut out = Vec::new();
        let code = stream_harness_with_program(
            fake_harness.to_str().unwrap(),
            temp_dir.path(),
            vec![
                "--jsonl".to_string(),
                "--resume".to_string(),
                "session-123".to_string(),
                "--".to_string(),
                "hello prompt".to_string(),
            ],
            false,
            "streamy",
            &mut out,
        )?;

        assert_eq!(code, 0);
        assert_eq!(
            std::fs::read_to_string(temp_dir.path().join("args.txt"))?,
            "--jsonl\n--resume\nsession-123\n--\nhello prompt\n"
        );
        assert_eq!(
            String::from_utf8(out)?,
            "{\"type\":\"assistant\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"streamy\"}]},\"session_id\":\"session-123\",\"stop_reason\":\"end_turn\"}\n"
        );
        Ok(())
    }

    #[test]
    fn stream_passthrough_args_drop_stdin_format_for_one_shot_prompt() {
        let args = vec![
            "--print".to_string(),
            "--input-format".to_string(),
            "stream-json".to_string(),
            "--output-format".to_string(),
            "stream-json".to_string(),
        ];

        assert_eq!(
            stream_passthrough_args(args.clone(), false),
            vec![
                "--print".to_string(),
                "--output-format".to_string(),
                "stream-json".to_string(),
            ]
        );
        assert_eq!(stream_passthrough_args(args.clone(), true), args);
    }

    #[cfg(unix)]
    #[test]
    fn stream_harness_claude_includes_input_format_when_forwarding_stdin() -> anyhow::Result<()> {
        use std::os::unix::fs::PermissionsExt;

        let _guard = fake_claude_spawn_guard();
        let temp_dir = tempfile::tempdir()?;
        let fake_claude = temp_dir.path().join("fake-claude");
        std::fs::write(
            &fake_claude,
            r#"#!/bin/sh
printf '%s\n' "$@" > args.txt
exit 0
"#,
        )?;
        let mut permissions = std::fs::metadata(&fake_claude)?.permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&fake_claude, permissions)?;

        let mut out = Vec::new();
        let _code = stream_harness_with_claude_args(
            fake_claude.to_str().unwrap(),
            temp_dir.path(),
            "session-456",
            false,
            "hello prompt",
            // forward_stdin=true → long-lived chat mode where claude reads
            // user messages as JSONL on stdin, so --input-format stream-json
            // MUST be present.
            true,
            None,
            crate::harness::HarnessLaunchOptions::default(),
            &mut out,
        )?;

        assert_eq!(
            std::fs::read_to_string(temp_dir.path().join("args.txt"))?,
            "-p\n--input-format\nstream-json\n--output-format\nstream-json\n--verbose\n--session-id\nsession-456\n--\nhello prompt\n"
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn stream_harness_claude_honors_permission_bypass_opt_in() -> anyhow::Result<()> {
        use std::os::unix::fs::PermissionsExt;

        let _guard = fake_claude_spawn_guard();
        let temp_dir = tempfile::tempdir()?;
        let fake_claude = temp_dir.path().join("fake-claude");
        std::fs::write(
            &fake_claude,
            r#"#!/bin/sh
printf '%s\n' "$@" > args.txt
exit 0
"#,
        )?;
        let mut permissions = std::fs::metadata(&fake_claude)?.permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&fake_claude, permissions)?;

        let mut out = Vec::new();
        let _code = stream_harness_with_claude_args_and_permission_bypass(
            fake_claude.to_str().unwrap(),
            temp_dir.path(),
            "session-456",
            false,
            "hello prompt",
            false,
            None,
            crate::harness::HarnessLaunchOptions::default(),
            true,
            &mut out,
        )?;

        assert_eq!(
            std::fs::read_to_string(temp_dir.path().join("args.txt"))?,
            "-p\n--permission-mode\nbypassPermissions\n--output-format\nstream-json\n--verbose\n--session-id\nsession-456\n--\nhello prompt\n"
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn stream_harness_claude_resumes_with_resume_flag_not_session_id() -> anyhow::Result<()> {
        use std::os::unix::fs::PermissionsExt;

        let _guard = fake_claude_spawn_guard();
        let temp_dir = tempfile::tempdir()?;
        let fake_claude = temp_dir.path().join("fake-claude");
        std::fs::write(
            &fake_claude,
            r#"#!/bin/sh
printf '%s\n' "$@" > args.txt
exit 0
"#,
        )?;
        let mut permissions = std::fs::metadata(&fake_claude)?.permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&fake_claude, permissions)?;

        let mut out = Vec::new();
        let _code = stream_harness_with_claude_args(
            fake_claude.to_str().unwrap(),
            temp_dir.path(),
            "session-789",
            // is_resume=true → the session already exists. `--session-id`
            // only creates sessions and fails with "Session ID <id> is
            // already in use" on reuse, so resumed turns MUST go through
            // `--resume` or every `coven run --continue` loses the chat.
            true,
            "hello again",
            false,
            None,
            crate::harness::HarnessLaunchOptions::default(),
            &mut out,
        )?;

        assert_eq!(
            std::fs::read_to_string(temp_dir.path().join("args.txt"))?,
            "-p\n--output-format\nstream-json\n--verbose\n--resume\nsession-789\n--\nhello again\n"
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn stream_harness_claude_forwards_model_with_prefix_stripped() -> anyhow::Result<()> {
        use std::os::unix::fs::PermissionsExt;

        let _guard = fake_claude_spawn_guard();
        let temp_dir = tempfile::tempdir()?;
        let fake_claude = temp_dir.path().join("fake-claude");
        std::fs::write(
            &fake_claude,
            r#"#!/bin/sh
printf '%s\n' "$@" > args.txt
exit 0
"#,
        )?;
        let mut permissions = std::fs::metadata(&fake_claude)?.permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&fake_claude, permissions)?;

        let mut out = Vec::new();
        let _code = stream_harness_with_claude_args(
            fake_claude.to_str().unwrap(),
            temp_dir.path(),
            "session-123",
            false,
            "hello prompt",
            false,
            None,
            // Namespaced id is normalized to the bare model before forwarding.
            crate::harness::HarnessLaunchOptions {
                model: Some("anthropic/claude-sonnet-4"),
                ..Default::default()
            },
            &mut out,
        )?;

        assert_eq!(
            std::fs::read_to_string(temp_dir.path().join("args.txt"))?,
            "-p\n--model\nclaude-sonnet-4\n--output-format\nstream-json\n--verbose\n--session-id\nsession-123\n--\nhello prompt\n"
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn stream_harness_claude_forwards_think_as_effort_high() -> anyhow::Result<()> {
        use std::os::unix::fs::PermissionsExt;

        let _guard = fake_claude_spawn_guard();
        let temp_dir = tempfile::tempdir()?;
        let fake_claude = temp_dir.path().join("fake-claude");
        std::fs::write(
            &fake_claude,
            r#"#!/bin/sh
printf '%s\n' "$@" > args.txt
exit 0
"#,
        )?;
        let mut permissions = std::fs::metadata(&fake_claude)?.permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&fake_claude, permissions)?;

        let mut out = Vec::new();
        let _code = stream_harness_with_claude_args(
            fake_claude.to_str().unwrap(),
            temp_dir.path(),
            "session-123",
            false,
            "hello prompt",
            false,
            None,
            crate::harness::HarnessLaunchOptions {
                think: true,
                ..Default::default()
            },
            &mut out,
        )?;

        assert_eq!(
            std::fs::read_to_string(temp_dir.path().join("args.txt"))?,
            "-p\n--effort\nhigh\n--output-format\nstream-json\n--verbose\n--session-id\nsession-123\n--\nhello prompt\n"
        );
        Ok(())
    }

    #[test]
    fn detached_output_drain_invokes_callback_for_bytes() {
        let captured = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let captured_for_callback = captured.clone();
        let mut callback: Box<dyn FnMut(Vec<u8>) + Send + 'static> = Box::new(move |chunk| {
            captured_for_callback
                .lock()
                .unwrap()
                .extend_from_slice(&chunk);
        });
        let mut reader: &[u8] = b"hello coven";

        drain_detached_output(&mut reader, Some(&mut callback));

        assert_eq!(captured.lock().unwrap().as_slice(), b"hello coven");
    }

    /// `Read` adapter that yields a fixed sequence of byte slices, one per
    /// `read` call, then EOF. Lets us drive `drain_detached_output` with
    /// the same chunk boundaries the kernel would produce when a
    /// multi-byte UTF-8 codepoint straddles two reads.
    struct ChunkedReader<'a> {
        chunks: std::collections::VecDeque<&'a [u8]>,
    }

    impl<'a> Read for ChunkedReader<'a> {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            match self.chunks.pop_front() {
                Some(chunk) => {
                    let n = chunk.len().min(buf.len());
                    buf[..n].copy_from_slice(&chunk[..n]);
                    if n < chunk.len() {
                        self.chunks.push_front(&chunk[n..]);
                    }
                    Ok(n)
                }
                None => Ok(0),
            }
        }
    }

    #[test]
    fn drain_detached_output_reassembles_codepoint_split_across_reads() {
        // 🎉 = F0 9F 8E 89. Split across two reads so the first ends
        // mid-codepoint. The drainer must hold the trailing bytes back
        // until the continuation arrives instead of lossy-decoding to
        // U+FFFD.
        let emoji = "🎉".as_bytes();
        let (head, tail) = emoji.split_at(2);
        let mut reader = ChunkedReader {
            chunks: vec![head, tail].into(),
        };

        let captured = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
        let captured_for_cb = captured.clone();
        let mut callback: Box<dyn FnMut(Vec<u8>) + Send + 'static> = Box::new(move |chunk| {
            captured_for_cb
                .lock()
                .unwrap()
                .push_str(std::str::from_utf8(&chunk).expect(
                    "drain_detached_output must only emit chunks that are themselves valid UTF-8",
                ));
        });

        drain_detached_output(&mut reader, Some(&mut callback));

        assert_eq!(
            captured.lock().unwrap().as_str(),
            "🎉",
            "split codepoint must round-trip; the drain owns per-call buffer state"
        );
    }

    #[test]
    fn drain_detached_output_flushes_trailing_partial_codepoint_on_eof() {
        // A read that delivers only the first 2 bytes of a 4-byte
        // codepoint and then closes. The buffered tail can never
        // complete, but it shouldn't silently disappear either — flush
        // it through `from_utf8_lossy` so the user sees something.
        let half = &"🎉".as_bytes()[..2];
        let mut reader = ChunkedReader {
            chunks: vec![half].into(),
        };
        let captured = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
        let captured_for_cb = captured.clone();
        let mut callback: Box<dyn FnMut(Vec<u8>) + Send + 'static> = Box::new(move |chunk| {
            captured_for_cb
                .lock()
                .unwrap()
                .push_str(&String::from_utf8_lossy(&chunk));
        });

        drain_detached_output(&mut reader, Some(&mut callback));

        let final_text = captured.lock().unwrap().clone();
        assert!(
            !final_text.is_empty(),
            "EOF with a partial codepoint must flush, not drop the bytes"
        );
        assert!(
            final_text.contains('\u{FFFD}'),
            "the flushed bytes are unrecoverable; expected U+FFFD replacement, got: {final_text:?}"
        );
    }

    #[test]
    fn detached_pty_answers_split_vt_queries_without_leaking_them() {
        let emoji = "🎉".as_bytes();
        let chunks: Vec<&[u8]> = vec![
            b"ready ",
            b"\x1b[",
            b"6n",
            &emoji[..2],
            &emoji[2..],
            b"\x1b[c\x1b[0",
            b"c\x1b[5n done",
        ];
        let mut reader = ChunkedReader {
            chunks: chunks.into(),
        };
        let captured = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let captured_for_callback = captured.clone();
        let mut callback: Box<dyn FnMut(Vec<u8>) + Send + 'static> = Box::new(move |chunk| {
            captured_for_callback
                .lock()
                .unwrap()
                .extend_from_slice(&chunk);
        });
        let mut replies = Vec::new();
        let mut terminal_reply = |reply: &'static [u8]| replies.extend_from_slice(reply);

        drain_detached_pty_output(&mut reader, &mut terminal_reply, Some(&mut callback));

        assert_eq!(
            captured.lock().unwrap().as_slice(),
            "ready 🎉 done".as_bytes()
        );
        assert_eq!(
            replies, b"\x1b[1;1R\x1b[?62;c\x1b[?62;c\x1b[0n",
            "CPR, primary/explicit DA, and status queries must receive terminal replies"
        );
    }

    #[test]
    fn detached_pty_preserves_unknown_and_incomplete_escape_sequences() {
        let mut reader = ChunkedReader {
            chunks: vec![b"before\x1b[31mred\x1b[".as_slice()].into(),
        };
        let captured = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let captured_for_callback = captured.clone();
        let mut callback: Box<dyn FnMut(Vec<u8>) + Send + 'static> = Box::new(move |chunk| {
            captured_for_callback
                .lock()
                .unwrap()
                .extend_from_slice(&chunk);
        });
        let mut replies = Vec::new();
        let mut terminal_reply = |reply: &'static [u8]| replies.extend_from_slice(reply);

        drain_detached_pty_output(&mut reader, &mut terminal_reply, Some(&mut callback));

        assert_eq!(
            captured.lock().unwrap().as_slice(),
            b"before\x1b[31mred\x1b["
        );
        assert!(replies.is_empty());
    }

    #[test]
    fn detached_pty_keeps_draining_when_terminal_reply_queue_is_full() {
        let mut reader = ChunkedReader {
            chunks: vec![b"before\x1b[6nafter".as_slice()].into(),
        };
        let captured = Arc::new(Mutex::new(Vec::new()));
        let captured_for_callback = Arc::clone(&captured);
        let mut callback: Box<dyn FnMut(Vec<u8>) + Send + 'static> = Box::new(move |chunk| {
            captured_for_callback.lock().unwrap().extend(chunk);
        });
        let (sender, _receiver) = mpsc::sync_channel(1);
        let writer = SharedPtyWriter { sender };
        assert!(writer
            .sender
            .try_send(PtyWriteRequest::Write {
                bytes: b"queue already full".to_vec(),
                flush: true,
                completion: None,
            })
            .is_ok());
        let mut terminal_reply = |reply| writer.queue_terminal_reply(reply);

        drain_detached_pty_output(&mut reader, &mut terminal_reply, Some(&mut callback));

        assert_eq!(captured.lock().unwrap().as_slice(), b"beforeafter");
    }

    #[test]
    fn terminal_replies_share_one_fifo_writer_path() -> anyhow::Result<()> {
        struct RecordingWriter(Arc<Mutex<Vec<u8>>>);

        impl Write for RecordingWriter {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(bytes);
                Ok(bytes.len())
            }

            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }

        let recorded = Arc::new(Mutex::new(Vec::new()));
        let mut writer = spawn_shared_pty_writer(Box::new(RecordingWriter(Arc::clone(&recorded))));
        writer.queue_terminal_reply(b"reply-a");
        writer.queue_terminal_reply(b"reply-b");
        writer.flush()?;

        assert_eq!(recorded.lock().unwrap().as_slice(), b"reply-areply-b");
        Ok(())
    }

    #[test]
    fn startup_detector_ignores_terminal_control_traffic_across_chunks() {
        let mut detector = MeaningfulOutputDetector::default();
        assert!(!detector.push(b"\x1b[?1004"));
        assert!(!detector.push(b"\x1b[?25\x1b[?1004h\x1b]0;terminal title"));
        assert!(!detector.push(b"\x1b\\\x1b("));
        assert!(!detector.push(b"B\x1b#8   \r\n\t"));
        assert!(detector.push(b"\x1b[32mready"));
    }

    #[test]
    fn builds_claude_command_without_shell_interpolation() {
        let cwd = Path::new("/tmp/coven-project");
        let command = build_harness_command(
            "claude",
            "explain && exit",
            cwd,
            crate::harness::HarnessLaunchMode::Interactive,
        )
        .unwrap();

        assert_eq!(command.program(), "claude");
        #[cfg(windows)]
        assert_eq!(command.args(), &["--", "\"explain ^&^& exit\""]);
        #[cfg(not(windows))]
        assert_eq!(command.args(), &["--", "explain && exit"]);
        assert_eq!(command.cwd(), cwd);
    }

    #[test]
    fn windows_codex_noninteractive_prompt_uses_stdin() {
        let prompt = "first line\nsecond & line with %PATH%";
        let mut args = vec![
            "exec".to_string(),
            "--model".to_string(),
            "gpt-5.5".to_string(),
            "--".to_string(),
            prompt.to_string(),
        ];

        let stdin_prompt = move_windows_codex_prompt_to_stdin(
            "codex",
            crate::harness::HarnessLaunchMode::NonInteractive,
            prompt,
            &mut args,
            true,
        );

        assert_eq!(args.last().map(String::as_str), Some("-"));
        assert_eq!(stdin_prompt.as_deref(), Some(prompt.as_bytes()));
    }

    #[test]
    fn codex_top_level_error_message_is_preserved() -> anyhow::Result<()> {
        let mut state = CodexJsonState::default();
        let mut assistant = Vec::new();

        let valid = handle_codex_json_line(
            r#"{"type":"error","message":"request rejected by Codex"}"#,
            &mut state,
            &mut |text| {
                assistant.push(text.to_string());
                Ok(())
            },
        )?;

        assert!(valid);
        assert_eq!(
            state.protocol_error.as_deref(),
            Some("request rejected by Codex")
        );
        assert!(assistant.is_empty());
        Ok(())
    }

    #[cfg(windows)]
    #[test]
    fn windows_codex_stdin_prompt_keeps_familiar_identity() -> anyhow::Result<()> {
        let familiar = crate::harness::FamiliarContext {
            id: "codex-local".to_string(),
            display_name: "Codex Local".to_string(),
            role: None,
        };
        let command = build_harness_command_with_conversation(
            "codex",
            "diagnose the failure",
            Path::new("C:\\project"),
            crate::harness::HarnessLaunchMode::NonInteractive,
            None,
            Some(&familiar),
            crate::harness::HarnessLaunchOptions::default(),
        )?;

        let prompt = String::from_utf8(command.stdin_prompt.expect("prompt should use stdin"))?;
        assert!(prompt.starts_with(&familiar.identity_preamble()));
        assert!(prompt.ends_with("diagnose the failure"));
        assert_eq!(command.args.last().map(String::as_str), Some("-"));
        Ok(())
    }

    #[test]
    fn stdin_prompt_transport_is_not_used_for_other_launches() {
        let prompt = "hello";
        for (harness, mode) in [
            ("claude", crate::harness::HarnessLaunchMode::NonInteractive),
            ("codex", crate::harness::HarnessLaunchMode::Interactive),
        ] {
            let mut args = vec!["--".to_string(), prompt.to_string()];
            let stdin_prompt =
                move_windows_codex_prompt_to_stdin(harness, mode, prompt, &mut args, true);
            assert!(stdin_prompt.is_none());
            assert_eq!(args.last().map(String::as_str), Some(prompt));
        }
    }

    #[cfg(windows)]
    #[test]
    fn captured_piped_batch_receives_multiline_prompt_via_stdin() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let batch = temp_dir.path().join("fake-codex.cmd");
        std::fs::write(
            &batch,
            "@echo off\r\nset /p prompt=\r\n>&2 echo %prompt%\r\nexit /b 0\r\n",
        )?;
        let command = HarnessCommand {
            program: batch.to_string_lossy().into_owned(),
            args: vec!["exec".to_string(), "--".to_string(), "-".to_string()],
            cwd: temp_dir.path().to_path_buf(),
            stdin_prompt: Some(b"hello from stdin\nsecond & unsafe-looking line".to_vec()),
        };
        let captured = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let callback_output = captured.clone();

        let result = run_piped_attached_captured(
            &command,
            Box::new(move |chunk| {
                callback_output.lock().unwrap().extend_from_slice(&chunk);
            }),
        )?;

        assert_eq!(result.status, "completed");
        assert_eq!(result.exit_code, Some(0));
        assert!(String::from_utf8(captured.lock().unwrap().clone())?.contains("hello from stdin"));
        Ok(())
    }

    #[cfg(windows)]
    #[test]
    fn codex_json_batch_shim_uses_stdin_and_emits_assistant_text() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let batch = temp_dir.path().join("fake-codex.cmd");
        // Copy stdin with findstr (a native, always-present binary with no
        // cold start): PowerShell's multi-second startup on loaded runners
        // outlived even a 10s activity deadline and flaked this test twice
        // (issue #407).
        std::fs::write(
            &batch,
            concat!(
                "@echo off\r\n",
                "\"%SystemRoot%\\System32\\findstr.exe\" \"^\" > stdin.txt\r\n",
                "echo %* > args.txt\r\n",
                "echo {\"type\":\"thread.started\",\"thread_id\":\"thread-456\"}\r\n",
                "echo {\"type\":\"item.completed\",\"item\":{\"id\":\"item-1\",\"type\":\"agent_message\",\"text\":\"reply from Codex\"}}\r\n",
                "echo {\"type\":\"turn.completed\"}\r\n",
                "exit /b 0\r\n"
            ),
        )?;
        let command = HarnessCommand {
            program: batch.to_string_lossy().into_owned(),
            args: vec![
                "exec".to_string(),
                "--json".to_string(),
                "--".to_string(),
                "-".to_string(),
            ],
            cwd: temp_dir.path().to_path_buf(),
            stdin_prompt: Some(b"first line\nsecond line\n".to_vec()),
        };
        let mut assistant = Vec::new();

        // Generous headroom for shim process startup on a loaded Windows
        // runner (issue #407). This test exercises stdin and JSONL framing,
        // not the activity deadline.
        let outcome = stream_codex_json_with_timeout(&command, Duration::from_secs(10), |text| {
            assistant.push(text.to_string());
            Ok(())
        })?;

        let args = std::fs::read_to_string(temp_dir.path().join("args.txt"))?;
        assert!(
            args.contains("exec --json -- -"),
            "unexpected argv: {args:?}"
        );
        assert!(
            !args.contains("first line") && !args.contains("second line"),
            "the multiline user prompt must not reach cmd.exe argv: {args:?}"
        );
        let stdin = std::fs::read_to_string(temp_dir.path().join("stdin.txt"))?;
        assert!(
            stdin.contains("first line"),
            "missing first stdin line: {stdin:?}"
        );
        assert!(
            stdin.contains("second line"),
            "missing second stdin line: {stdin:?}"
        );
        assert_eq!(assistant, vec!["reply from Codex"]);
        assert_eq!(outcome.harness_session_id.as_deref(), Some("thread-456"));
        assert!(outcome.error.is_none());
        Ok(())
    }

    #[cfg(windows)]
    #[test]
    fn codex_json_batch_shim_times_out_while_large_prompt_is_still_writing() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let batch = temp_dir.path().join("silent-codex.cmd");
        std::fs::write(&batch, "@echo off\r\n:spin\r\ngoto spin\r\n")?;
        let command = HarnessCommand {
            program: batch.to_string_lossy().into_owned(),
            args: vec![
                "exec".to_string(),
                "--json".to_string(),
                "--".to_string(),
                "-".to_string(),
            ],
            cwd: temp_dir.path().to_path_buf(),
            // The shim deliberately never reads stdin. This payload exceeds
            // the anonymous-pipe buffer, proving the activity deadline also
            // covers a blocked prompt writer rather than only stdout reads.
            stdin_prompt: Some(vec![b'x'; 1024 * 1024]),
        };

        let started = Instant::now();
        let outcome =
            stream_codex_json_with_timeout(&command, Duration::from_millis(50), |_| Ok(()))?;

        assert!(started.elapsed() < Duration::from_secs(3));
        assert!(outcome
            .error
            .as_deref()
            .is_some_and(|error| error.contains("terminated")));
        Ok(())
    }
}
