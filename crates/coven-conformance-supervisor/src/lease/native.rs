//! Native lease acquisition and independent acknowledgement/loss observation.
use crate::Failure;

use std::{
    fs::File,
    io::{self, Read},
    mem::{size_of, zeroed},
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
    ptr::null_mut,
    sync::{
        atomic::{AtomicBool, AtomicU8, Ordering},
        Arc,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::{
        DuplicateHandle, GetLastError, DUPLICATE_SAME_ACCESS, ERROR_IO_INCOMPLETE,
        ERROR_IO_PENDING, HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT,
    },
    Storage::FileSystem::ReadFile,
    System::{
        Pipes::{GetNamedPipeInfo, PIPE_SERVER_END},
        Threading::{
            CreateEventW, GetCurrentProcess, GetStartupInfoW, WaitForSingleObject, STARTUPINFOW,
        },
        IO::{CancelIoEx, CancelSynchronousIo, GetOverlappedResult, OVERLAPPED},
    },
};

fn byte_pipe(file: &File) -> Result<(), Failure> {
    let mut flags = 0;
    // SAFETY: The file owns a live handle; optional outputs are null, flags is writable.
    if unsafe {
        GetNamedPipeInfo(
            file.as_raw_handle(),
            &mut flags,
            null_mut(),
            null_mut(),
            null_mut(),
        )
    } == 0
        || flags & !PIPE_SERVER_END != 0
    {
        return Err(Failure::InvalidLaunch);
    }
    Ok(())
}

fn duplicate_pipe(raw: HANDLE) -> Result<File, Failure> {
    let mut duplicate = null_mut();
    // SAFETY: Win32 validates the source handle; null/pseudo values were rejected by
    // the startup decoder. No dereference is performed, and the duplicate is private.
    if unsafe {
        DuplicateHandle(
            GetCurrentProcess(),
            raw,
            GetCurrentProcess(),
            &mut duplicate,
            0,
            0,
            DUPLICATE_SAME_ACCESS,
        )
    } == 0
    {
        return Err(Failure::InvalidLaunch);
    }
    // SAFETY: Successful duplication returned an independently owned kernel handle.
    let file = File::from(unsafe { OwnedHandle::from_raw_handle(duplicate) });
    byte_pipe(&file)?;
    Ok(file)
}

/// Acquire only the private four-pipe libuv contract. Does not own/close CRT originals.
/// Actual supported Node/libuv versions must be exercised on native Windows.
pub fn inherited_pipe() -> Result<File, Failure> {
    // SAFETY: STARTUPINFOW is a plain Win32 output record with null pointers initially.
    let mut startup: STARTUPINFOW = unsafe { zeroed() };
    // SAFETY: Win32 writes the local output record and returns this process's startup data.
    unsafe { GetStartupInfoW(&mut startup) };
    let expected = 8 + 4 * size_of::<HANDLE>();
    if startup.lpReserved2.is_null() || startup.cbReserved2 as usize != expected {
        return Err(Failure::InvalidLaunch);
    }
    // SAFETY: GetStartupInfoW returns OS-owned process startup storage of cbReserved2
    // bytes. Its exact bounded length is checked above and it remains live here.
    let bytes = unsafe { std::slice::from_raw_parts(startup.lpReserved2, expected) };
    let raw = super::decode_node_lease(bytes, size_of::<HANDLE>())?;
    duplicate_pipe(raw as usize as HANDLE)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeaseStatus {
    AwaitingAck,
    Acknowledged,
    Failed,
}

struct CancellablePipe {
    file: File,
    stopping: Arc<AtomicBool>,
}
impl Read for CancellablePipe {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        // read_exact retries Interrupted. Checking before each retry makes cancellation
        // terminal rather than accidentally starting another blocking native read.
        if self.stopping.load(Ordering::Acquire) {
            return Err(io::ErrorKind::BrokenPipe.into());
        }
        if buffer.is_empty() {
            return Ok(0);
        }
        let count = u32::try_from(buffer.len()).map_err(|_| io::ErrorKind::InvalidInput)?;
        // SAFETY: Create an unnamed, noninheritable manual-reset event with no initial signal.
        let raw_event = unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) };
        if raw_event.is_null() {
            return Err(io::ErrorKind::Other.into());
        }
        // SAFETY: A successful event creation returned an independently owned handle.
        let event = unsafe { OwnedHandle::from_raw_handle(raw_event) };
        // SAFETY: Zero initializes the Win32 operation record, including byte offsets.
        let mut operation: OVERLAPPED = unsafe { zeroed() };
        operation.hEvent = event.as_raw_handle();
        // SAFETY: The file, buffer, event and unique operation record remain live
        // until completion, including after cancellation. For synchronous pipes this
        // call blocks; for overlapped pipes the event/result loop owns pending I/O.
        let completed = unsafe {
            ReadFile(
                self.file.as_raw_handle(),
                buffer.as_mut_ptr(),
                count,
                null_mut(),
                std::ptr::addr_of_mut!(operation),
            )
        };
        // SAFETY: Read the calling thread's last error immediately after ReadFile.
        if completed == 0 && unsafe { GetLastError() } != ERROR_IO_PENDING {
            return Err(io::ErrorKind::BrokenPipe.into());
        }
        loop {
            let mut transferred = 0;
            // SAFETY: This is the same live handle and outstanding operation. A
            // false/non-INCOMPLETE result is a terminal I/O completion error.
            if unsafe {
                GetOverlappedResult(
                    self.file.as_raw_handle(),
                    std::ptr::addr_of!(operation),
                    &mut transferred,
                    0,
                )
            } != 0
            {
                if self.stopping.load(Ordering::Acquire) || transferred > count {
                    return Err(io::ErrorKind::BrokenPipe.into());
                }
                return Ok(transferred as usize);
            }
            // SAFETY: Read the calling thread's last error immediately after the query.
            if unsafe { GetLastError() } != ERROR_IO_INCOMPLETE {
                return Err(io::ErrorKind::BrokenPipe.into());
            }
            if self.stopping.load(Ordering::Acquire) {
                // SAFETY: Cancel exactly this operation; cancellation is only a
                // request. Do not drop its storage until completion is observed.
                unsafe { CancelIoEx(self.file.as_raw_handle(), std::ptr::addr_of!(operation)) };
            }
            // SAFETY: The event is privately owned for this outstanding operation.
            let waited = unsafe { WaitForSingleObject(event.as_raw_handle(), 10) };
            if waited != WAIT_OBJECT_0 && waited != WAIT_TIMEOUT {
                // Retain all I/O storage even on an unexpected wait failure. The
                // coordinator's bounded stop may detach this worker and exit safely.
                thread::sleep(Duration::from_millis(10));
            }
        }
    }
}

pub struct LeaseWatch {
    state: Arc<AtomicU8>,
    stopping: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}
impl LeaseWatch {
    /// Start before child creation. Poll failure independently of all target I/O.
    pub fn start(file: File, nonce: String) -> Result<Self, Failure> {
        if nonce.len() != 32
            || !nonce
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(Failure::InvalidLaunch);
        }
        byte_pipe(&file)?;
        let state = Arc::new(AtomicU8::new(0));
        let stopping = Arc::new(AtomicBool::new(false));
        let worker_state = state.clone();
        let mut reader = CancellablePipe {
            file,
            stopping: stopping.clone(),
        };
        let worker = thread::Builder::new()
            .name("coven-audit-lease".into())
            .spawn(move || {
                if crate::protocol::read_ack(&mut reader, &nonce).is_ok()
                    && !reader.stopping.load(Ordering::Acquire)
                {
                    worker_state.store(1, Ordering::Release);
                    // Any data, EOF or error after ACK is lease failure. This reader has
                    // no target-body/output responsibilities that could block observation.
                    let _ = reader.read(&mut [0]);
                }
                worker_state.store(2, Ordering::Release);
            })
            .map_err(|_| Failure::InvalidLaunch)?;
        Ok(Self {
            state,
            stopping,
            worker: Some(worker),
        })
    }

    pub fn status(&self) -> LeaseStatus {
        match self.state.load(Ordering::Acquire) {
            0 => LeaseStatus::AwaitingAck,
            1 => LeaseStatus::Acknowledged,
            _ => LeaseStatus::Failed,
        }
    }

    /// Explicit stop must be checked by the native coordinator after job cleanup.
    /// A failed stop detaches a still-owned worker; the helper process must then exit.
    pub fn stop(mut self) -> Result<(), Failure> {
        self.stop_inner()
    }

    fn stop_inner(&mut self) -> Result<(), Failure> {
        let Some(worker) = self.worker.take() else {
            return Ok(());
        };
        self.stopping.store(true, Ordering::Release);
        let started = Instant::now();
        loop {
            // SAFETY: The JoinHandle keeps this exact native thread handle alive.
            match unsafe { WaitForSingleObject(worker.as_raw_handle(), 0) } {
                WAIT_OBJECT_0 => return worker.join().map_err(|_| Failure::CleanupUnproven),
                WAIT_TIMEOUT => {}
                _ => return Err(Failure::CleanupUnproven),
            }
            // SAFETY: The owned worker handle remains live. No pending operation is
            // also valid: repeat to cover cancellation racing the start of ReadFile.
            unsafe { CancelSynchronousIo(worker.as_raw_handle()) };
            // Cooperative cleanup hang guard, deliberately not a latency/SLO assertion.
            if started.elapsed() >= Duration::from_secs(10) {
                return Err(Failure::CleanupUnproven);
            }
            thread::sleep(Duration::from_millis(10));
        }
    }
}
impl Drop for LeaseWatch {
    fn drop(&mut self) {
        // Best effort only; callers need explicit stop for reportable cleanup proof.
        let _ = self.stop_inner();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs::File,
        io::Write,
        os::windows::io::{AsRawHandle, FromRawHandle},
        ptr::null_mut,
        thread,
        time::{Duration, Instant},
    };
    use windows_sys::Win32::{
        Foundation::{GetHandleInformation, HANDLE_FLAG_INHERIT},
        System::Pipes::CreatePipe,
    };

    fn pipe() -> (File, File) {
        let (mut read, mut write) = (null_mut(), null_mut());
        // SAFETY: Writable handle outputs, default noninheritable security, bounded buffer.
        assert_ne!(
            unsafe { CreatePipe(&mut read, &mut write, std::ptr::null(), 4096) },
            0
        );
        // SAFETY: CreatePipe returned distinct owned file handles.
        unsafe { (File::from_raw_handle(read), File::from_raw_handle(write)) }
    }
    fn overlapped_pipe() -> (File, File) {
        use std::sync::atomic::{AtomicU32, Ordering};
        use windows_sys::Win32::{
            Foundation::{ERROR_PIPE_CONNECTED, GENERIC_READ, INVALID_HANDLE_VALUE},
            Storage::FileSystem::{
                CreateFileW, FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAG_OVERLAPPED, OPEN_EXISTING,
                PIPE_ACCESS_OUTBOUND,
            },
            System::Pipes::{ConnectNamedPipe, CreateNamedPipeW, PIPE_TYPE_BYTE, PIPE_WAIT},
        };
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let name: Vec<u16> = format!(
            r"\\.\pipe\coven-audit-lease-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        )
        .encode_utf16()
        .chain([0])
        .collect();
        // SAFETY: Bounded NUL-terminated private name, byte-mode synchronous server,
        // one instance, no inherited handle or pointer outputs.
        let server = unsafe {
            CreateNamedPipeW(
                name.as_ptr(),
                PIPE_ACCESS_OUTBOUND | FILE_FLAG_FIRST_PIPE_INSTANCE,
                PIPE_TYPE_BYTE | PIPE_WAIT,
                1,
                4096,
                4096,
                0,
                std::ptr::null(),
            )
        };
        assert_ne!(server, INVALID_HANDLE_VALUE);
        // SAFETY: Successful creation returned an owned synchronous server handle.
        let write = unsafe { File::from_raw_handle(server) };
        // SAFETY: Connect a read-only overlapped client; only native ReadFile uses it.
        let client = unsafe {
            CreateFileW(
                name.as_ptr(),
                GENERIC_READ,
                0,
                std::ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_OVERLAPPED,
                null_mut(),
            )
        };
        assert_ne!(client, INVALID_HANDLE_VALUE);
        // SAFETY: Successful creation returned an owned client handle. No File::read.
        let read = unsafe { File::from_raw_handle(client) };
        // SAFETY: Server is synchronous; the client has already connected. No pending
        // OVERLAPPED operation can escape this setup routine.
        let connected = unsafe { ConnectNamedPipe(write.as_raw_handle(), null_mut()) };
        // SAFETY: Last error is read directly after the failed connection call.
        assert!(connected != 0 || unsafe { GetLastError() } == ERROR_PIPE_CONNECTED);
        (read, write)
    }
    fn pipe_mode(overlapped: bool) -> (File, File) {
        if overlapped {
            overlapped_pipe()
        } else {
            pipe()
        }
    }
    fn wait_status(watch: &LeaseWatch, expected: LeaseStatus) {
        let started = Instant::now();
        while watch.status() != expected {
            assert!(
                started.elapsed() < Duration::from_secs(30),
                "native lease fixture hang guard after {:?}: {:?}",
                started.elapsed(),
                watch.status()
            );
            thread::sleep(Duration::from_millis(10));
        }
    }
    fn wait_pending_read(watch: &LeaseWatch) {
        use windows_sys::Win32::System::Threading::GetThreadIOPendingFlag;
        let worker = watch.worker.as_ref().unwrap();
        let started = Instant::now();
        loop {
            let mut pending = 0;
            // SAFETY: The owned JoinHandle keeps this exact thread live, and the
            // output flag is writable. The thread does no other file/device I/O.
            assert_ne!(
                unsafe { GetThreadIOPendingFlag(worker.as_raw_handle(), &mut pending) },
                0
            );
            assert_ne!(
                watch.status(),
                LeaseStatus::Failed,
                "lease ended before pending-read proof"
            );
            if pending != 0 {
                return;
            }
            assert!(
                started.elapsed() < Duration::from_secs(30),
                "native pending-read hang guard after {:?}",
                started.elapsed()
            );
            thread::sleep(Duration::from_millis(10));
        }
    }
    #[test]
    fn acknowledges_then_detects_eof_or_unexpected_data_on_private_pipe() {
        for overlapped in [false, true] {
            for send_extra in [false, true] {
                let (read, mut write) = pipe_mode(overlapped);
                let file = duplicate_pipe(read.as_raw_handle()).unwrap();
                let mut flags = 0;
                // SAFETY: Owned duplicate is live and flags is writable.
                assert_ne!(
                    unsafe { GetHandleInformation(file.as_raw_handle(), &mut flags) },
                    0
                );
                assert_eq!(flags & HANDLE_FLAG_INHERIT, 0);
                let watch = LeaseWatch::start(file, "a".repeat(32)).unwrap();
                write
                    .write_all(format!("GO {}\n", "a".repeat(32)).as_bytes())
                    .unwrap();
                wait_status(&watch, LeaseStatus::Acknowledged);
                if send_extra {
                    write.write_all(b"x").unwrap();
                    wait_status(&watch, LeaseStatus::Failed);
                    // Keep the writer open so EOF cannot make this data case pass.
                    watch.stop().unwrap();
                    drop(write);
                } else {
                    drop(write);
                    wait_status(&watch, LeaseStatus::Failed);
                    watch.stop().unwrap();
                }
            }
        }
    }
    #[test]
    fn refuses_wrong_ack_and_cancels_waits_before_or_after_ack() {
        for overlapped in [false, true] {
            let (read, mut write) = pipe_mode(overlapped);
            let watch = LeaseWatch::start(read, "a".repeat(32)).unwrap();
            write
                .write_all(format!("GO {}\n", "b".repeat(32)).as_bytes())
                .unwrap();
            wait_status(&watch, LeaseStatus::Failed);
            watch.stop().unwrap();
            for acknowledged in [false, true] {
                let (read, mut write) = pipe_mode(overlapped);
                let watch = LeaseWatch::start(read, "a".repeat(32)).unwrap();
                if acknowledged {
                    write
                        .write_all(format!("GO {}\n", "a".repeat(32)).as_bytes())
                        .unwrap();
                    wait_status(&watch, LeaseStatus::Acknowledged);
                }
                // Kernel observation proves a read is pending, not merely a race
                // where stop wins before worker startup. No data/EOF can end it.
                wait_pending_read(&watch);
                // Keep the writer open: cancellation must stop without needing EOF.
                watch.stop().unwrap();
                drop(write);
            }
        }
    }
    #[test]
    fn refuses_nonpipe_and_invalid_nonce_without_closing_original() {
        let original = tempfile::tempfile().unwrap();
        assert!(matches!(
            duplicate_pipe(original.as_raw_handle()),
            Err(Failure::InvalidLaunch)
        ));
        assert!(original.metadata().is_ok());
        let (read, _write) = pipe();
        assert!(matches!(
            LeaseWatch::start(read, "bad".into()),
            Err(Failure::InvalidLaunch)
        ));
    }
}
