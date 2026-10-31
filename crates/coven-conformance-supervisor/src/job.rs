//! Creation-time process-tree ownership. No post-creation assignment window.
use crate::{command::command_line, Failure};
use std::{
    alloc::{alloc_zeroed, dealloc, Layout},
    ffi::OsStr,
    mem::{size_of, zeroed},
    os::windows::{
        ffi::OsStrExt,
        io::{AsHandle, AsRawHandle, BorrowedHandle, FromRawHandle, OwnedHandle},
    },
    path::Path,
    ptr::{null, null_mut},
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::{
        DuplicateHandle, GetLastError, DUPLICATE_SAME_ACCESS, ERROR_INSUFFICIENT_BUFFER, HANDLE,
        WAIT_OBJECT_0, WAIT_TIMEOUT,
    },
    System::{
        JobObjects::{
            CreateJobObjectW, IsProcessInJob, JobObjectBasicAccountingInformation,
            JobObjectExtendedLimitInformation, QueryInformationJobObject, SetInformationJobObject,
            TerminateJobObject, JOBOBJECT_BASIC_ACCOUNTING_INFORMATION,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        },
        Threading::{
            CreateProcessW, DeleteProcThreadAttributeList, GetCurrentProcess, GetExitCodeProcess,
            InitializeProcThreadAttributeList, ResumeThread, TerminateProcess,
            UpdateProcThreadAttribute, WaitForSingleObject, CREATE_NO_WINDOW, CREATE_SUSPENDED,
            EXTENDED_STARTUPINFO_PRESENT, LPPROC_THREAD_ATTRIBUTE_LIST, PROCESS_INFORMATION,
            PROC_THREAD_ATTRIBUTE_HANDLE_LIST, PROC_THREAD_ATTRIBUTE_JOB_LIST,
            STARTF_USESTDHANDLES, STARTUPINFOEXW,
        },
    },
};

struct Attributes {
    pointer: LPPROC_THREAD_ATTRIBUTE_LIST,
    layout: Layout,
}
impl Attributes {
    fn new() -> Result<Self, Failure> {
        let mut bytes = 0;
        // SAFETY: Null is the documented sizing probe; bytes is writable.
        let sized = unsafe { InitializeProcThreadAttributeList(null_mut(), 2, 0, &mut bytes) };
        // SAFETY: Reads the calling thread's last Win32 error immediately.
        if sized != 0
            || unsafe { GetLastError() } != ERROR_INSUFFICIENT_BUFFER
            || bytes == 0
            || bytes > 65536
        {
            return Err(Failure::ContainmentUnavailable);
        }
        let layout =
            Layout::from_size_align(bytes, 16).map_err(|_| Failure::ContainmentUnavailable)?;
        // SAFETY: Valid bounded layout; checked for null before use.
        let pointer: LPPROC_THREAD_ATTRIBUTE_LIST = unsafe { alloc_zeroed(layout) }.cast();
        if pointer.is_null() {
            return Err(Failure::ContainmentUnavailable);
        }
        // SAFETY: The allocation is aligned and contains the reported byte count.
        if unsafe { InitializeProcThreadAttributeList(pointer, 2, 0, &mut bytes) } == 0 {
            // SAFETY: Not initialized, so only the matching allocation is released.
            unsafe { dealloc(pointer.cast(), layout) };
            return Err(Failure::ContainmentUnavailable);
        }
        Ok(Self { pointer, layout })
    }
}
impl Drop for Attributes {
    fn drop(&mut self) {
        // SAFETY: Initialization succeeded; list and allocation have one owner.
        unsafe {
            DeleteProcThreadAttributeList(self.pointer);
            dealloc(self.pointer.cast(), self.layout);
        }
    }
}

fn inheritable(handle: BorrowedHandle<'_>) -> Result<OwnedHandle, Failure> {
    let mut duplicate = null_mut();
    // SAFETY: Borrowed handle is live; duplicate receives a new owned handle.
    if unsafe {
        DuplicateHandle(
            GetCurrentProcess(),
            handle.as_raw_handle(),
            GetCurrentProcess(),
            &mut duplicate,
            0,
            1,
            DUPLICATE_SAME_ACCESS,
        )
    } == 0
    {
        return Err(Failure::ContainmentUnavailable);
    }
    // SAFETY: Successful DuplicateHandle returned an owned non-pseudo handle.
    Ok(unsafe { OwnedHandle::from_raw_handle(duplicate) })
}

pub struct ContainedProcess {
    // Drop closes the job before the remaining process/thread handles.
    job: OwnedHandle,
    process: OwnedHandle,
    thread: OwnedHandle,
    id: u32,
    resumed: bool,
}
impl ContainedProcess {
    pub fn process_id(&self) -> u32 {
        self.id
    }
    pub fn process_handle(&self) -> BorrowedHandle<'_> {
        self.process.as_handle()
    }
    pub fn resume(&mut self) -> Result<(), Failure> {
        if self.resumed {
            return Err(Failure::InvalidLaunch);
        }
        // SAFETY: This owned primary thread was created suspended exactly once.
        if unsafe { ResumeThread(self.thread.as_raw_handle()) } != 1 {
            return Err(Failure::ContainmentUnavailable);
        }
        self.resumed = true;
        Ok(())
    }
    pub fn wait_for_exit(&self, timeout_ms: u32) -> Result<Option<u32>, Failure> {
        // SAFETY: Process handle is owned for the duration of the wait.
        match unsafe { WaitForSingleObject(self.process.as_raw_handle(), timeout_ms) } {
            WAIT_TIMEOUT => Ok(None),
            WAIT_OBJECT_0 => {
                let mut code = 0;
                // SAFETY: Signaled process handle remains live; code is writable.
                if unsafe { GetExitCodeProcess(self.process.as_raw_handle(), &mut code) } == 0 {
                    return Err(Failure::ContainmentUnavailable);
                }
                Ok(Some(code))
            }
            _ => Err(Failure::ContainmentUnavailable),
        }
    }
    pub fn active_processes(&self) -> Result<u32, Failure> {
        // SAFETY: This Win32 output structure is valid when zero initialized.
        let mut info: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION = unsafe { zeroed() };
        // SAFETY: Owned job, correctly sized writable information structure.
        if unsafe {
            QueryInformationJobObject(
                self.job.as_raw_handle(),
                JobObjectBasicAccountingInformation,
                (&mut info as *mut JOBOBJECT_BASIC_ACCOUNTING_INFORMATION).cast(),
                size_of::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>() as u32,
                null_mut(),
            )
        } == 0
        {
            return Err(Failure::CleanupUnproven);
        }
        Ok(info.ActiveProcesses)
    }
    pub fn terminate_and_wait(&self) -> Result<(), Failure> {
        let started = Instant::now();
        // SAFETY: This private owned job cannot refer to an unrelated job.
        if unsafe { TerminateJobObject(self.job.as_raw_handle(), 1) } == 0 {
            return Err(Failure::CleanupUnproven);
        }
        loop {
            if self.active_processes()? == 0 {
                return Ok(());
            }
            // Cooperative cleanup hang guard, not a ratified recovery SLO.
            if started.elapsed() >= Duration::from_secs(10) {
                return Err(Failure::CleanupUnproven);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}
impl Drop for ContainedProcess {
    fn drop(&mut self) {
        // SAFETY: Handles remain owned until after Drop; kernel close is the
        // independent backstop even if this explicit termination fails.
        unsafe { TerminateJobObject(self.job.as_raw_handle(), 1) };
        // SAFETY: The exact created root handle is still owned. This also
        // cleans up a suspended root when job membership could not be proven;
        // no descendant can have run before that membership check succeeds.
        unsafe { TerminateProcess(self.process.as_raw_handle(), 1) };
    }
}

pub fn spawn_suspended(
    program: &OsStr,
    args: &[String],
    directory: &OsStr,
    stdio: [BorrowedHandle<'_>; 3],
) -> Result<ContainedProcess, Failure> {
    if !Path::new(program).is_absolute() || !Path::new(directory).is_absolute() {
        return Err(Failure::InvalidLaunch);
    }
    let application: Vec<u16> = program.encode_wide().collect();
    let mut command = command_line(&application, args)?;
    let mut application = application;
    application.push(0);
    let mut cwd: Vec<u16> = directory.encode_wide().collect();
    if cwd.len() > 8192 || cwd.contains(&0) {
        return Err(Failure::InvalidLaunch);
    }
    cwd.push(0);
    // SAFETY: Anonymous job with null security attributes is noninheritable.
    let raw_job = unsafe { CreateJobObjectW(null(), null()) };
    if raw_job.is_null() {
        return Err(Failure::ContainmentUnavailable);
    }
    // SAFETY: CreateJobObjectW returned one owned valid handle.
    let job = unsafe { OwnedHandle::from_raw_handle(raw_job) };
    // SAFETY: This C POD structure accepts zero initialization.
    let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { zeroed() };
    limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    // SAFETY: Owned job and correctly sized live configuration structure.
    if unsafe {
        SetInformationJobObject(
            raw_job,
            JobObjectExtendedLimitInformation,
            (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
            size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        )
    } == 0
    {
        return Err(Failure::ContainmentUnavailable);
    }
    let input = inheritable(stdio[0])?;
    let output = inheritable(stdio[1])?;
    let error = inheritable(stdio[2])?;
    let handles = [
        input.as_raw_handle(),
        output.as_raw_handle(),
        error.as_raw_handle(),
    ];
    let jobs = [raw_job];
    // Attribute values and owned handles outlive the list and CreateProcessW.
    let attributes = Attributes::new()?;
    // SAFETY: Live arrays have the exact attribute-specific byte sizes. Only
    // stdio is inherited; the job/controller lease handles are not in this list.
    if unsafe {
        UpdateProcThreadAttribute(
            attributes.pointer,
            0,
            PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
            handles.as_ptr().cast(),
            size_of::<[HANDLE; 3]>(),
            null_mut(),
            null(),
        )
    } == 0
        || unsafe {
            UpdateProcThreadAttribute(
                attributes.pointer,
                0,
                PROC_THREAD_ATTRIBUTE_JOB_LIST as usize,
                jobs.as_ptr().cast(),
                size_of::<[HANDLE; 1]>(),
                null_mut(),
                null(),
            )
        } == 0
    {
        return Err(Failure::ContainmentUnavailable);
    }
    // SAFETY: C POD structs; required fields are set before the API call.
    let mut startup: STARTUPINFOEXW = unsafe { zeroed() };
    startup.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
    startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
    startup.StartupInfo.hStdInput = handles[0];
    startup.StartupInfo.hStdOutput = handles[1];
    startup.StartupInfo.hStdError = handles[2];
    startup.lpAttributeList = attributes.pointer;
    // SAFETY: C POD output structure.
    let mut info: PROCESS_INFORMATION = unsafe { zeroed() };
    // SAFETY: Absolute NUL-terminated application/cwd, writable bounded command
    // line, initialized attributes and live inheritance arrays. The job is
    // attached by the kernel at creation, before any target instruction runs.
    if unsafe {
        CreateProcessW(
            application.as_ptr(),
            command.as_mut_ptr(),
            null(),
            null(),
            1,
            CREATE_SUSPENDED | CREATE_NO_WINDOW | EXTENDED_STARTUPINFO_PRESENT,
            null(),
            cwd.as_ptr(),
            &startup.StartupInfo,
            &mut info,
        )
    } == 0
    {
        return Err(Failure::ContainmentUnavailable);
    }
    // SAFETY: Successful creation returned owned process and primary-thread handles.
    let process = unsafe { OwnedHandle::from_raw_handle(info.hProcess) };
    // SAFETY: This is the distinct owned thread handle from the same successful call.
    let thread = unsafe { OwnedHandle::from_raw_handle(info.hThread) };
    // Release attributes while all referenced handle values are still live.
    drop(attributes);
    let contained = ContainedProcess {
        job,
        process,
        thread,
        id: info.dwProcessId,
        resumed: false,
    };
    let mut member = 0;
    // SAFETY: Live owned handles and writable BOOL result.
    if unsafe { IsProcessInJob(contained.process.as_raw_handle(), raw_job, &mut member) } == 0
        || member == 0
    {
        // Creation succeeded, but membership is unknown. Drop requests both
        // job and exact-root termination without proving either has completed;
        // ordinary containment refusal would hide that cleanup uncertainty.
        return Err(Failure::CleanupUnproven);
    }
    Ok(contained)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs::{read_to_string, File},
        process::Command,
    };
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
    };

    fn child(directory: &Path, fixture: &str) -> ContainedProcess {
        let executable = std::env::current_exe().expect("fixture executable unavailable");
        let null_device = File::options()
            .read(true)
            .write(true)
            .open("NUL")
            .expect("null device unavailable");
        spawn_suspended(
            executable.as_os_str(),
            &[
                "--exact".into(),
                format!("job::tests::{fixture}"),
                "--ignored".into(),
            ],
            directory.as_os_str(),
            [null_device.as_handle(); 3],
        )
        .expect("native containment unavailable")
    }

    #[test]
    fn dropping_a_pre_resume_owner_kills_the_created_process() {
        let directory = tempfile::tempdir().expect("private fixture directory unavailable");
        let process = child(directory.path(), "exits_successfully");
        let identity = process
            .process_handle()
            .try_clone_to_owned()
            .expect("process handle duplication unavailable");
        assert_eq!(process.wait_for_exit(0).unwrap(), None);
        drop(process);
        // Native cleanup hang guard, far above ordinary kernel settlement.
        // SAFETY: Owned process identity remains open across owner destruction.
        let status = unsafe { WaitForSingleObject(identity.as_raw_handle(), 60000) };
        assert_eq!(
            status, WAIT_OBJECT_0,
            "pre-resume root cleanup status: {status}"
        );
        assert!(
            !directory.path().join("started.marker").exists(),
            "target ran before resume"
        );
    }

    #[test]
    fn explicit_resume_runs_the_exact_fixture_and_settles_empty() {
        let directory = tempfile::tempdir().expect("private fixture directory unavailable");
        let mut process = child(directory.path(), "exits_successfully");
        process.resume().expect("primary thread resume unavailable");
        assert_eq!(process.wait_for_exit(60000).unwrap(), Some(0));
        assert_eq!(
            read_to_string(directory.path().join("started.marker")).unwrap(),
            "started"
        );
        process
            .terminate_and_wait()
            .expect("native cleanup unproven");
        assert_eq!(process.active_processes().unwrap(), 0);
    }

    #[test]
    fn root_exit_does_not_leave_a_descendant_alive_after_job_cleanup() {
        let directory = tempfile::tempdir().expect("private fixture directory unavailable");
        let mut process = child(directory.path(), "spawns_descendant_then_exits");
        process.resume().expect("primary thread resume unavailable");
        assert_eq!(process.wait_for_exit(60000).unwrap(), Some(0));
        let pid: u32 = read_to_string(directory.path().join("descendant.pid"))
            .unwrap()
            .parse()
            .unwrap();
        // SAFETY: Query/synchronization only; membership below binds this identity
        // to our private job before its eventual termination is accepted.
        let raw = unsafe {
            OpenProcess(
                PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
                0,
                pid,
            )
        };
        assert!(!raw.is_null(), "descendant process identity unavailable");
        // SAFETY: Successful OpenProcess returned a newly owned handle.
        let descendant = unsafe { OwnedHandle::from_raw_handle(raw) };
        let mut member = 0;
        // SAFETY: Live process/job handles and writable membership result.
        assert_ne!(
            unsafe { IsProcessInJob(raw, process.job.as_raw_handle(), &mut member) },
            0
        );
        assert_ne!(member, 0, "descendant did not inherit the private job");
        assert!(process.active_processes().unwrap() >= 1);
        process
            .terminate_and_wait()
            .expect("descendant cleanup unproven");
        // SAFETY: The exact native process identity remains held across cleanup.
        let status = unsafe { WaitForSingleObject(descendant.as_raw_handle(), 60000) };
        assert_eq!(status, WAIT_OBJECT_0, "descendant cleanup status: {status}");
        assert_eq!(process.active_processes().unwrap(), 0);
    }

    #[test]
    #[ignore = "Invoked only as a child by native job tests"]
    fn exits_successfully() {
        std::fs::write("started.marker", "started").expect("fixture marker unavailable");
        std::process::exit(0);
    }

    #[test]
    #[ignore = "Invoked only as a descendant by native job tests"]
    fn holds_open() {
        std::fs::write("descendant.pid.pending", std::process::id().to_string())
            .expect("fixture readiness unavailable");
        std::fs::rename("descendant.pid.pending", "descendant.pid")
            .expect("fixture readiness publication unavailable");
        loop {
            std::thread::park();
        }
    }

    #[test]
    #[ignore = "Invoked only as a root by native job tests"]
    fn spawns_descendant_then_exits() {
        let mut descendant = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "job::tests::holds_open", "--ignored"])
            .spawn()
            .expect("descendant creation unavailable");
        let started = Instant::now();
        while !Path::new("descendant.pid").exists() {
            assert!(
                descendant.try_wait().unwrap().is_none(),
                "descendant exited before readiness"
            );
            // Sequencing is the explicit readiness file; this is only a hang guard.
            assert!(
                started.elapsed() < Duration::from_secs(30),
                "descendant readiness hung after {:?}",
                started.elapsed()
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        std::process::exit(0);
    }
}
