use std::ffi::OsString;
use std::io::{self, Read};
use std::path::PathBuf;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

const POLL_INTERVAL: Duration = Duration::from_millis(10);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LaunchRequest {
    pub executable: PathBuf,
    pub args: Vec<OsString>,
    pub current_dir: Option<PathBuf>,
    pub env_overrides: Vec<(OsString, Option<OsString>)>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProcessExit {
    Exited(i32),
    Signalled,
}

pub trait Clock {
    fn now(&self) -> Duration;
    fn sleep(&self, duration: Duration);
}

#[derive(Debug)]
pub struct SystemClock {
    origin: Instant,
}

impl SystemClock {
    pub fn new() -> Self {
        Self {
            origin: Instant::now(),
        }
    }
}

impl Default for SystemClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for SystemClock {
    fn now(&self) -> Duration {
        self.origin.elapsed()
    }

    fn sleep(&self, duration: Duration) {
        std::thread::sleep(duration);
    }
}

pub trait RunningProcess {
    fn try_wait(&mut self) -> io::Result<Option<ProcessExit>>;
    fn terminate_tree(&mut self) -> io::Result<()>;
    fn wait(&mut self) -> io::Result<ProcessExit>;
}

pub trait ProcessLauncher {
    fn launch(&mut self, request: &LaunchRequest) -> io::Result<Box<dyn RunningProcess>>;
}

pub struct SystemProcessLauncher;

impl ProcessLauncher for SystemProcessLauncher {
    fn launch(&mut self, request: &LaunchRequest) -> io::Result<Box<dyn RunningProcess>> {
        let mut command = Command::new(&request.executable);
        command
            .args(&request.args)
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit());
        if let Some(current_dir) = request.current_dir.as_deref() {
            command.current_dir(current_dir);
        }
        for (name, value) in &request.env_overrides {
            match value {
                Some(value) => {
                    command.env(name, value);
                }
                None => {
                    command.env_remove(name);
                }
            }
        }
        Ok(Box::new(spawn_managed(&mut command)?))
    }
}

pub fn probe_version(
    executable: &std::path::Path,
    args: &[OsString],
    timeout: Duration,
) -> io::Result<Option<String>> {
    let mut command = Command::new(executable);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut process = spawn_managed(&mut command)?;
    let stdout = process
        .child
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("version probe stdout was not piped"))?;
    let (sender, receiver) = mpsc::sync_channel(1);
    thread::spawn(move || {
        let mut bytes = Vec::new();
        let result = stdout.take(4097).read_to_end(&mut bytes).map(|_| bytes);
        let _ = sender.send(result);
    });

    let deadline = Instant::now() + timeout;
    let exit = loop {
        if let Some(exit) = process.try_wait()? {
            break exit;
        }
        if Instant::now() >= deadline {
            process.terminate_tree()?;
            let _ = process.wait()?;
            return Ok(None);
        }
        thread::sleep(POLL_INTERVAL);
    };
    let output = match receiver.recv_timeout(Duration::from_millis(100)) {
        Ok(output) => output?,
        Err(mpsc::RecvTimeoutError::Timeout) => {
            process.terminate_tree()?;
            receiver
                .recv_timeout(Duration::from_millis(150))
                .map_err(|_| {
                    io::Error::new(io::ErrorKind::TimedOut, "version probe output timed out")
                })??
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            return Err(io::Error::other("version probe output reader stopped"));
        }
    };
    if exit != ProcessExit::Exited(0) {
        return Ok(None);
    }
    Ok(extract_version(&String::from_utf8_lossy(&output)))
}

fn extract_version(output: &str) -> Option<String> {
    output
        .split_whitespace()
        .map(|token| {
            token.trim_matches(|character: char| {
                !character.is_ascii_alphanumeric() && !matches!(character, '.' | '_' | '+' | '-')
            })
        })
        .find(|token| {
            !token.is_empty()
                && token.len() <= 128
                && token.bytes().any(|byte| byte.is_ascii_digit())
                && token.bytes().all(|byte| {
                    byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'+' | b'-')
                })
        })
        .map(str::to_owned)
}

pub(crate) enum BoundedProcessResult {
    Exited(ProcessExit),
    TimedOut,
}

pub(crate) fn wait_bounded(
    process: &mut dyn RunningProcess,
    clock: &dyn Clock,
    timeout: Duration,
) -> io::Result<BoundedProcessResult> {
    let deadline = clock.now().saturating_add(timeout);
    loop {
        if let Some(exit) = process.try_wait()? {
            return Ok(BoundedProcessResult::Exited(exit));
        }
        let now = clock.now();
        if now >= deadline {
            process.terminate_tree()?;
            let _ = process.wait()?;
            return Ok(BoundedProcessResult::TimedOut);
        }
        clock.sleep(POLL_INTERVAL.min(deadline.saturating_sub(now)));
    }
}

struct SystemRunningProcess {
    child: Child,
    #[cfg(unix)]
    process_group: libc::pid_t,
    #[cfg(windows)]
    job: windows_job::Job,
}

fn spawn_managed(command: &mut Command) -> io::Result<SystemRunningProcess> {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        use windows_sys::Win32::System::Threading::CREATE_SUSPENDED;

        command.creation_flags(CREATE_SUSPENDED);
    }

    let child = command.spawn()?;

    #[cfg(windows)]
    let (child, job) = {
        let mut child = child;
        let job = match windows_job::assign_kill_on_close_job(&child) {
            Ok(job) => job,
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error);
            }
        };
        if !windows_job::resume_suspended_child(&child) {
            let _ = job.terminate();
            let _ = child.wait();
            return Err(io::Error::other("could not resume managed Windows process"));
        }
        (child, job)
    };

    Ok(SystemRunningProcess {
        #[cfg(unix)]
        process_group: child.id() as libc::pid_t,
        child,
        #[cfg(windows)]
        job,
    })
}

impl RunningProcess for SystemRunningProcess {
    fn try_wait(&mut self) -> io::Result<Option<ProcessExit>> {
        self.child
            .try_wait()
            .map(|status| status.map(classify_exit))
    }

    fn terminate_tree(&mut self) -> io::Result<()> {
        terminate_system_tree(self)
    }

    fn wait(&mut self) -> io::Result<ProcessExit> {
        self.child.wait().map(classify_exit)
    }
}

#[cfg(unix)]
fn terminate_system_tree(process: &mut SystemRunningProcess) -> io::Result<()> {
    signal_process_group(process.process_group, libc::SIGTERM)?;
    let deadline = Instant::now() + Duration::from_millis(250);
    while process_group_is_alive(process.process_group) && Instant::now() < deadline {
        let _ = process.child.try_wait();
        std::thread::sleep(POLL_INTERVAL);
    }
    if process_group_is_alive(process.process_group) {
        match signal_process_group(process.process_group, libc::SIGKILL) {
            Ok(()) => {}
            Err(error) if error.raw_os_error() == Some(libc::EPERM) => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

#[cfg(windows)]
fn terminate_system_tree(process: &mut SystemRunningProcess) -> io::Result<()> {
    process.job.terminate()
}

#[cfg(not(any(unix, windows)))]
fn terminate_system_tree(process: &mut SystemRunningProcess) -> io::Result<()> {
    process.child.kill()
}

fn classify_exit(status: ExitStatus) -> ProcessExit {
    match status.code() {
        Some(code) => ProcessExit::Exited(code),
        None => ProcessExit::Signalled,
    }
}

#[cfg(unix)]
fn signal_process_group(process_group: libc::pid_t, signal: libc::c_int) -> io::Result<()> {
    let result = unsafe { libc::kill(-process_group, signal) };
    if result == 0 {
        return Ok(());
    }
    let error = io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::ESRCH) {
        Ok(())
    } else {
        Err(error)
    }
}

#[cfg(unix)]
fn process_group_is_alive(process_group: libc::pid_t) -> bool {
    let result = unsafe { libc::kill(-process_group, 0) };
    result == 0 || io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(windows)]
mod windows_job {
    use std::io;
    use std::mem::{size_of, zeroed};
    use std::os::windows::io::AsRawHandle;
    use std::process::Child;
    use std::ptr;

    use windows_sys::Win32::Foundation::{
        CloseHandle, GetLastError, ERROR_NO_MORE_FILES, HANDLE, INVALID_HANDLE_VALUE,
    };
    use windows_sys::Win32::System::{
        Diagnostics::ToolHelp::{
            CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD, THREADENTRY32,
        },
        JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
            SetInformationJobObject, TerminateJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
            JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        },
        Threading::{OpenThread, ResumeThread, THREAD_SUSPEND_RESUME},
    };

    pub(super) struct Job(HANDLE);

    impl Job {
        pub(super) fn terminate(&self) -> io::Result<()> {
            let result = unsafe { TerminateJobObject(self.0, 1) };
            if result == 0 {
                Err(io::Error::last_os_error())
            } else {
                Ok(())
            }
        }
    }

    impl Drop for Job {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }

    pub(super) fn assign_kill_on_close_job(child: &Child) -> io::Result<Job> {
        let handle = unsafe { CreateJobObjectW(ptr::null(), ptr::null()) };
        if handle.is_null() {
            return Err(io::Error::last_os_error());
        }
        let job = Job(handle);
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { zeroed() };
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let configured = unsafe {
            SetInformationJobObject(
                job.0,
                JobObjectExtendedLimitInformation,
                (&raw const limits).cast(),
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        };
        if configured == 0 {
            return Err(io::Error::last_os_error());
        }
        let assigned = unsafe { AssignProcessToJobObject(job.0, child.as_raw_handle() as HANDLE) };
        if assigned == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(job)
    }

    pub(super) fn resume_suspended_child(child: &Child) -> bool {
        let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
        if snapshot == INVALID_HANDLE_VALUE {
            return false;
        }

        let mut entry = THREADENTRY32 {
            dwSize: size_of::<THREADENTRY32>() as u32,
            ..Default::default()
        };
        let mut thread_ids = Vec::new();
        let mut has_entry = unsafe { Thread32First(snapshot, &mut entry) } != 0;
        let mut enumeration_complete = false;
        while has_entry {
            if entry.th32OwnerProcessID == child.id() {
                thread_ids.push(entry.th32ThreadID);
            }
            has_entry = unsafe { Thread32Next(snapshot, &mut entry) } != 0;
            if !has_entry {
                enumeration_complete = unsafe { GetLastError() } == ERROR_NO_MORE_FILES;
            }
        }
        unsafe { CloseHandle(snapshot) };

        if !enumeration_complete {
            return false;
        }
        let [thread_id] = thread_ids.as_slice() else {
            return false;
        };
        let thread = unsafe { OpenThread(THREAD_SUSPEND_RESUME, 0, *thread_id) };
        if thread.is_null() {
            return false;
        }
        let previous_suspend_count = unsafe { ResumeThread(thread) };
        unsafe { CloseHandle(thread) };
        previous_suspend_count == 1
    }
}
