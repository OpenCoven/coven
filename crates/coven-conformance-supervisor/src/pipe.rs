//! Mode-aware byte pipe I/O. An independent owner must cancel synchronous waits
//! and keep detached worker storage live until the standalone helper exits.
use crate::Failure;
use std::{
    fs::File,
    io::{self, Read, Write},
    mem::zeroed,
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
    ptr::{null, null_mut},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
    time::Duration,
};
use windows_sys::Win32::{
    Foundation::{
        GetLastError, ERROR_BROKEN_PIPE, ERROR_HANDLE_EOF, ERROR_IO_INCOMPLETE, ERROR_IO_PENDING,
        WAIT_OBJECT_0, WAIT_TIMEOUT,
    },
    Storage::FileSystem::{ReadFile, WriteFile},
    System::{
        Pipes::{GetNamedPipeInfo, PIPE_SERVER_END},
        Threading::{CreateEventW, WaitForSingleObject},
        IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED},
    },
};

pub(crate) fn byte_pipe(file: &File) -> Result<(), Failure> {
    let mut flags = 0;
    // SAFETY: Owned live handle; optional outputs are null and flags is writable.
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

enum Buffer<'a> {
    Read(&'a mut [u8]),
    Write(&'a [u8]),
}

pub(crate) struct CancellablePipe {
    file: File,
    pub(crate) stopping: Arc<AtomicBool>,
}
impl CancellablePipe {
    pub(crate) fn new(file: File, stopping: Arc<AtomicBool>) -> Result<Self, Failure> {
        byte_pipe(&file)?;
        Ok(Self { file, stopping })
    }

    fn refused(&self, code: u32, reading: bool) -> io::Result<usize> {
        if reading
            && !self.stopping.load(Ordering::Acquire)
            && matches!(code, ERROR_BROKEN_PIPE | ERROR_HANDLE_EOF)
        {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        // EOF is explicit above; all other errors remain errors, never success.
        Err(io::ErrorKind::BrokenPipe.into())
    }

    fn transfer(&mut self, buffer: Buffer<'_>) -> io::Result<usize> {
        if self.stopping.load(Ordering::Acquire) {
            return Err(io::ErrorKind::BrokenPipe.into());
        }
        let (reading, count) = match &buffer {
            Buffer::Read(bytes) => (true, bytes.len()),
            Buffer::Write(bytes) => (false, bytes.len()),
        };
        if count == 0 {
            return Ok(0);
        }
        let count = u32::try_from(count).map_err(|_| io::ErrorKind::InvalidInput)?;
        // SAFETY: Create a private noninheritable manual-reset event.
        let raw_event = unsafe { CreateEventW(null(), 1, 0, null()) };
        if raw_event.is_null() {
            return Err(io::ErrorKind::Other.into());
        }
        // SAFETY: Successful creation returned one owned kernel handle.
        let event = unsafe { OwnedHandle::from_raw_handle(raw_event) };
        // SAFETY: Zero is a valid initial Win32 operation record.
        let mut operation: OVERLAPPED = unsafe { zeroed() };
        operation.hEvent = event.as_raw_handle();
        let completed = match buffer {
            // SAFETY: Owned handle, live writable buffer and unique operation/event.
            // All storage stays live through terminal completion below.
            Buffer::Read(bytes) => unsafe {
                ReadFile(
                    self.file.as_raw_handle(),
                    bytes.as_mut_ptr(),
                    count,
                    null_mut(),
                    std::ptr::addr_of_mut!(operation),
                )
            },
            // SAFETY: Same ownership as read; this API only reads the borrowed bytes.
            Buffer::Write(bytes) => unsafe {
                WriteFile(
                    self.file.as_raw_handle(),
                    bytes.as_ptr(),
                    count,
                    null_mut(),
                    std::ptr::addr_of_mut!(operation),
                )
            },
        };
        if completed == 0 {
            // SAFETY: Read the initiating thread's error immediately after the call.
            let code = unsafe { GetLastError() };
            if code != ERROR_IO_PENDING {
                return self.refused(code, reading);
            }
        }
        loop {
            let mut transferred = 0;
            // SAFETY: Exact live file/operation pair and writable output. The
            // caller's buffer, event and operation cannot leave before completion.
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
            // SAFETY: Read the completion query's error immediately.
            let code = unsafe { GetLastError() };
            if code != ERROR_IO_INCOMPLETE {
                return self.refused(code, reading);
            }
            if self.stopping.load(Ordering::Acquire) {
                // SAFETY: Cancel only this operation. A request is not completion;
                // retain its storage and continue observing its terminal state.
                unsafe { CancelIoEx(self.file.as_raw_handle(), std::ptr::addr_of!(operation)) };
            }
            // SAFETY: The event is privately owned for this pending operation.
            let waited = unsafe { WaitForSingleObject(event.as_raw_handle(), 10) };
            if waited != WAIT_OBJECT_0 && waited != WAIT_TIMEOUT {
                // Preserve pending storage even if the wait fails. An independent
                // coordinator can report unproven cleanup and exit the helper.
                thread::sleep(Duration::from_millis(10));
            }
        }
    }
}
impl Read for CancellablePipe {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        if bytes.is_empty() {
            return Ok(0);
        }
        loop {
            // A successful zero-byte pipe read can represent a peer's empty
            // write, rather than closure. Only explicit native EOF ends reading.
            // transfer distinguishes that closure below via UnexpectedEof.
            match self.transfer(Buffer::Read(bytes)) {
                Ok(0) => continue,
                Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(0),
                result => return result,
            }
        }
    }
}
impl Write for CancellablePipe {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.transfer(Buffer::Write(bytes))
    }
    fn flush(&mut self) -> io::Result<()> {
        // WriteFile completion already places bytes in the pipe. FlushFileBuffers
        // would add a consumer-drain wait; this adapter has no user-space buffer.
        if self.stopping.load(Ordering::Acquire) {
            Err(io::ErrorKind::BrokenPipe.into())
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{os::windows::io::FromRawHandle, time::Instant};
    use windows_sys::Win32::{
        Foundation::{ERROR_PIPE_CONNECTED, GENERIC_WRITE, INVALID_HANDLE_VALUE},
        Storage::FileSystem::{
            CreateFileW, FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAG_OVERLAPPED, FILE_READ_ATTRIBUTES,
            OPEN_EXISTING, PIPE_ACCESS_INBOUND,
        },
        System::{
            Pipes::{ConnectNamedPipe, CreateNamedPipeW, CreatePipe, PIPE_TYPE_BYTE, PIPE_WAIT},
            Threading::GetThreadIOPendingFlag,
            IO::CancelSynchronousIo,
        },
    };

    fn pair(overlapped_writer: bool) -> (File, File) {
        if !overlapped_writer {
            let (mut read, mut write) = (null_mut(), null_mut());
            // SAFETY: Writable outputs and default noninheritable byte-pipe handles.
            assert_ne!(
                unsafe { CreatePipe(&mut read, &mut write, null(), 4096) },
                0
            );
            // SAFETY: Successful CreatePipe returned two independently owned handles.
            return unsafe { (File::from_raw_handle(read), File::from_raw_handle(write)) };
        }
        use std::sync::atomic::AtomicU32;
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let name: Vec<u16> = format!(
            r"\\.\pipe\coven-audit-write-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        )
        .encode_utf16()
        .chain([0])
        .collect();
        // SAFETY: Private NUL-terminated name, one instance, synchronous read server.
        let server = unsafe {
            CreateNamedPipeW(
                name.as_ptr(),
                PIPE_ACCESS_INBOUND | FILE_FLAG_FIRST_PIPE_INSTANCE,
                PIPE_TYPE_BYTE | PIPE_WAIT,
                1,
                4096,
                4096,
                0,
                null(),
            )
        };
        assert_ne!(server, INVALID_HANDLE_VALUE);
        // SAFETY: Successful creation returned an owned server handle.
        let read = unsafe { File::from_raw_handle(server) };
        // SAFETY: Open a write-only overlapped client; only native WriteFile uses it.
        let client = unsafe {
            CreateFileW(
                name.as_ptr(),
                GENERIC_WRITE | FILE_READ_ATTRIBUTES,
                0,
                null(),
                OPEN_EXISTING,
                FILE_FLAG_OVERLAPPED,
                null_mut(),
            )
        };
        assert_ne!(client, INVALID_HANDLE_VALUE);
        // SAFETY: Successful open returned an owned client handle.
        let write = unsafe { File::from_raw_handle(client) };
        // SAFETY: Server is synchronous and its client is already connected.
        let connected = unsafe { ConnectNamedPipe(read.as_raw_handle(), null_mut()) };
        // SAFETY: Read the failed connection call's error immediately.
        assert!(connected != 0 || unsafe { GetLastError() } == ERROR_PIPE_CONNECTED);
        (read, write)
    }

    #[test]
    fn writes_binary_on_synchronous_and_overlapped_handles() {
        for overlapped in [false, true] {
            let (mut read, write) = pair(overlapped);
            let worker = thread::spawn(move || {
                let mut pipe =
                    CancellablePipe::new(write, Arc::new(AtomicBool::new(false))).unwrap();
                pipe.write_all(&[0, 255, 1, 128, 0]).unwrap();
                pipe.flush().unwrap();
            });
            let mut received = [0; 5];
            read.read_exact(&mut received).unwrap();
            worker.join().unwrap();
            assert_eq!(received, [0, 255, 1, 128, 0]);
        }
    }

    #[test]
    fn empty_peer_write_does_not_hide_payload_or_require_writer_closure() {
        let (read, write) = pair(false);
        let (release, held) = std::sync::mpsc::channel();
        let writer = thread::spawn(move || {
            let mut transferred = 0;
            // SAFETY: Live synchronous pipe, zero bytes and writable count output.
            assert_ne!(
                unsafe {
                    WriteFile(
                        write.as_raw_handle(),
                        [0u8].as_ptr(),
                        0,
                        &mut transferred,
                        null_mut(),
                    )
                },
                0
            );
            let mut write = write;
            write.write_all(&[0, 255, 42]).unwrap();
            held.recv().unwrap(); // Keep the peer open until payload was observed.
        });
        let mut pipe = CancellablePipe::new(read, Arc::new(AtomicBool::new(false))).unwrap();
        let mut bytes = [0; 3];
        pipe.read_exact(&mut bytes).unwrap();
        assert_eq!(bytes, [0, 255, 42]);
        release.send(()).unwrap();
        writer.join().unwrap();
        assert_eq!(pipe.read(&mut [0]).unwrap(), 0);
    }

    #[test]
    fn actual_eof_is_distinct_from_wrong_direction_and_nonpipe_errors() {
        let (read, write) = pair(false);
        let mut pipe = CancellablePipe::new(read, Arc::new(AtomicBool::new(false))).unwrap();
        drop(write);
        assert_eq!(pipe.read(&mut [0]).unwrap(), 0);
        let (read, write) = pair(false);
        let mut wrong_read = CancellablePipe::new(write, Arc::new(AtomicBool::new(false))).unwrap();
        assert!(wrong_read.read(&mut [0]).is_err());
        let mut wrong_write = CancellablePipe::new(read, Arc::new(AtomicBool::new(false))).unwrap();
        assert!(wrong_write.write(&[0]).is_err());
        assert!(matches!(
            CancellablePipe::new(
                tempfile::tempfile().unwrap(),
                Arc::new(AtomicBool::new(false))
            ),
            Err(Failure::InvalidLaunch)
        ));
    }

    #[test]
    fn cancels_kernel_pending_synchronous_and_overlapped_writes_without_reader_eof() {
        for overlapped in [false, true] {
            let (read, write) = pair(overlapped);
            let stopping = Arc::new(AtomicBool::new(false));
            let worker_stop = stopping.clone();
            let worker = thread::spawn(move || {
                let mut pipe = CancellablePipe::new(write, worker_stop).unwrap();
                pipe.write_all(&vec![0x41; 1024 * 1024])
            });
            let started = Instant::now();
            loop {
                let mut pending = 0;
                // SAFETY: This exact JoinHandle remains owned and output is writable.
                assert_ne!(
                    unsafe { GetThreadIOPendingFlag(worker.as_raw_handle(), &mut pending) },
                    0
                );
                assert!(
                    !worker.is_finished(),
                    "writer ended before pending-I/O proof"
                );
                if pending != 0 {
                    break;
                }
                assert!(
                    started.elapsed() < Duration::from_secs(30),
                    "pending-write fixture hang guard after {:?}",
                    started.elapsed()
                );
                thread::sleep(Duration::from_millis(10));
            }
            stopping.store(true, Ordering::Release);
            while !worker.is_finished() {
                // SAFETY: Owned exact thread; repeat cancellation to cover start races.
                unsafe { CancelSynchronousIo(worker.as_raw_handle()) };
                assert!(
                    started.elapsed() < Duration::from_secs(30),
                    "write cancellation hang guard after {:?}",
                    started.elapsed()
                );
                thread::sleep(Duration::from_millis(10));
            }
            assert!(worker.join().unwrap().is_err());
            // Keep the reader open throughout; EOF cannot make this fixture pass.
            drop(read);
        }
    }
}
