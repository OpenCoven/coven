use std::ffi::OsString;
use std::io;
use std::path::PathBuf;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

const POLL_INTERVAL: Duration = Duration::from_millis(10);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LaunchRequest {
    pub executable: PathBuf,
    pub args: Vec<OsString>,
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

        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
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
            (child, job)
        };

        Ok(Box::new(SystemRunningProcess {
            #[cfg(unix)]
            process_group: child.id() as libc::pid_t,
            child,
            #[cfg(windows)]
            job,
        }))
    }
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

    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, TerminateJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
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
}
