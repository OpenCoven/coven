use std::{
    io::{Read, Write},
    time::{Duration, Instant},
};

#[cfg(windows)]
use crate::{
    discovery::{is_coven_daemon_pipe_name, validate_owner_only_windows_pipe_handle},
    transport::{PeerIdentity, TransportResponse, MAX_RESPONSE_BODY_BYTES},
    DaemonEndpoint,
};
use crate::{transport::HttpResponse, ClientError};

const MAX_RESPONSE_HEADERS_BYTES: usize = 64 * 1024;
const ERROR_BROKEN_PIPE_CODE: i32 = 109;
const ERROR_NO_DATA_CODE: i32 = 232;
const TRAILING_RESPONSE_BYTES: &str =
    "daemon sent bytes beyond its declared response Content-Length";
#[cfg(windows)]
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(5);
#[cfg(any(windows, test))]
const fn windows_pipe_client_flags() -> u32 {
    // SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION. Passing zero asks
    // CreateFileW for SecurityImpersonation, which lets a pipe server
    // impersonate an elevated client.
    0x0010_0000 | 0x0001_0000
}

const fn filetime_parts_to_u64(low: u32, high: u32) -> u64 {
    (high as u64) << 32 | low as u64
}

fn remaining_for_phase(
    deadline: Instant,
    now: Instant,
    operation: &'static str,
) -> Result<Duration, ClientError> {
    deadline
        .checked_duration_since(now)
        .filter(|remaining| !remaining.is_zero())
        .ok_or_else(|| ClientError::Io {
            operation,
            source: std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!("timed out {operation}"),
            ),
        })
}

#[cfg(windows)]
pub(crate) fn request(
    endpoint: &DaemonEndpoint,
    method: &'static str,
    path: &str,
    body: Option<&[u8]>,
    expected_peer: Option<&PeerIdentity>,
) -> Result<TransportResponse, ClientError> {
    // One deadline covers the finite WaitNamedPipe/CreateFile loop and all
    // subsequent nonblocking I/O.
    let deadline = Instant::now() + RESPONSE_TIMEOUT;
    let body = body.unwrap_or_default();
    let request = format!(
        "{method} {path} HTTP/1.1\r\nHost: coven\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
        body.len()
    );
    let mut stream = connect_validated_with_deadline(endpoint.pipe_name(), deadline)?;
    let peer_identity = connected_windows_peer_identity(&stream)?;
    if expected_peer.is_some_and(|expected| expected != &peer_identity) {
        return Err(ClientError::DaemonInstanceChanged);
    }
    const OPERATION: &str = "failed to write Coven daemon request";
    write_windows_pipe_with_deadline(&mut stream, request.as_bytes(), deadline, OPERATION)?;
    write_windows_pipe_with_deadline(&mut stream, body, deadline, OPERATION)?;
    Ok(TransportResponse {
        response: read_response(&mut stream, deadline)?,
        peer_identity,
    })
}

#[cfg(windows)]
pub(crate) fn verify_peer(
    endpoint: &DaemonEndpoint,
    expected_peer: &PeerIdentity,
) -> Result<(), ClientError> {
    let deadline = Instant::now() + RESPONSE_TIMEOUT;
    let stream = connect_validated_with_deadline(endpoint.pipe_name(), deadline)?;
    if &connected_windows_peer_identity(&stream)? != expected_peer {
        return Err(ClientError::DaemonInstanceChanged);
    }
    Ok(())
}

#[cfg(windows)]
fn connect_validated_with_deadline(
    pipe_name: &str,
    deadline: Instant,
) -> Result<std::fs::File, ClientError> {
    let stream = connect_with_deadline(pipe_name, deadline)?;
    validate_connected_windows_pipe_file(&stream)?;
    set_windows_pipe_nonblocking(&stream)?;
    Ok(stream)
}

#[cfg(windows)]
fn connect_with_deadline(pipe_name: &str, deadline: Instant) -> Result<std::fs::File, ClientError> {
    use std::{
        ffi::OsStr,
        os::windows::{ffi::OsStrExt, io::FromRawHandle},
        ptr,
    };
    use windows_sys::Win32::{
        Foundation::{
            ERROR_PIPE_BUSY, ERROR_SEM_TIMEOUT, GENERIC_READ, GENERIC_WRITE, INVALID_HANDLE_VALUE,
        },
        Storage::FileSystem::{CreateFileW, OPEN_EXISTING, READ_CONTROL},
        System::Pipes::WaitNamedPipeW,
    };

    const OPERATION: &str = "failed to connect to Coven daemon pipe";
    let pipe_path = format!(r"\\.\pipe\{pipe_name}");
    let pipe_path: Vec<u16> = OsStr::new(&pipe_path)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    loop {
        if Instant::now() >= deadline {
            return Err(connect_timeout_error());
        }
        // SAFETY: `pipe_path` is NUL-terminated and all optional pointers are
        // null; a successful call returns a fresh owned handle.
        let handle = unsafe {
            CreateFileW(
                pipe_path.as_ptr(),
                GENERIC_READ | GENERIC_WRITE | READ_CONTROL,
                0,
                ptr::null(),
                OPEN_EXISTING,
                windows_pipe_client_flags(),
                ptr::null_mut(),
            )
        };
        if handle != INVALID_HANDLE_VALUE {
            // SAFETY: ownership of the fresh handle transfers to `File`
            // exactly once.
            return Ok(unsafe { std::fs::File::from_raw_handle(handle) });
        }

        let error = std::io::Error::last_os_error();
        if error.raw_os_error() != Some(ERROR_PIPE_BUSY as i32) {
            return Err(ClientError::Io {
                operation: OPERATION,
                source: error,
            });
        }
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .and_then(finite_windows_wait_millis)
            .ok_or_else(connect_timeout_error)?;
        // SAFETY: `pipe_path` remains a valid NUL-terminated buffer and the
        // timeout is finite.
        if unsafe { WaitNamedPipeW(pipe_path.as_ptr(), remaining) } == 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() == Some(ERROR_SEM_TIMEOUT as i32) || Instant::now() >= deadline
            {
                return Err(connect_timeout_error());
            }
            return Err(ClientError::Io {
                operation: OPERATION,
                source: error,
            });
        }
    }
}

fn finite_windows_wait_millis(remaining: Duration) -> Option<u32> {
    let millis = remaining.as_millis();
    (millis > 0).then(|| millis.min((u32::MAX - 1) as u128) as u32)
}

#[cfg(windows)]
fn connect_timeout_error() -> ClientError {
    ClientError::Io {
        operation: "failed to connect to Coven daemon pipe",
        source: std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "timed out connecting to Coven daemon pipe",
        ),
    }
}

#[cfg(windows)]
fn validate_connected_windows_pipe_file(stream: &std::fs::File) -> Result<(), ClientError> {
    use std::os::windows::io::AsRawHandle;

    validate_owner_only_windows_pipe_handle(stream.as_raw_handle())
}

#[cfg(windows)]
fn connected_windows_peer_identity(stream: &std::fs::File) -> Result<PeerIdentity, ClientError> {
    use std::os::windows::io::{AsRawHandle, FromRawHandle};
    use windows_sys::Win32::{
        Foundation::{FILETIME, WAIT_FAILED, WAIT_OBJECT_0, WAIT_TIMEOUT},
        System::{
            Pipes::GetNamedPipeServerProcessId,
            Threading::{
                GetProcessTimes, OpenProcess, WaitForSingleObject,
                PROCESS_QUERY_LIMITED_INFORMATION,
            },
        },
    };

    let mut server_pid = 0;
    if unsafe { GetNamedPipeServerProcessId(stream.as_raw_handle(), &mut server_pid) } == 0
        || server_pid == 0
    {
        return Err(ClientError::Io {
            operation: "failed to identify connected Coven daemon pipe server",
            source: std::io::Error::last_os_error(),
        });
    }

    const SYNCHRONIZE_ACCESS: u32 = 0x0010_0000;
    let process = unsafe {
        OpenProcess(
            PROCESS_QUERY_LIMITED_INFORMATION | SYNCHRONIZE_ACCESS,
            0,
            server_pid,
        )
    };
    if process.is_null() {
        return Err(ClientError::Io {
            operation: "failed to retain connected Coven daemon process identity",
            source: std::io::Error::last_os_error(),
        });
    }
    let process = unsafe { std::os::windows::io::OwnedHandle::from_raw_handle(process) };
    let mut creation = FILETIME::default();
    let mut exit = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    if unsafe {
        GetProcessTimes(
            process.as_raw_handle(),
            &mut creation,
            &mut exit,
            &mut kernel,
            &mut user,
        )
    } == 0
    {
        return Err(ClientError::Io {
            operation: "failed to inspect connected Coven daemon process identity",
            source: std::io::Error::last_os_error(),
        });
    }
    match unsafe { WaitForSingleObject(process.as_raw_handle(), 0) } {
        WAIT_TIMEOUT => {}
        WAIT_OBJECT_0 => {
            return Err(ClientError::Discovery(
                "connected Coven daemon process exited during identity validation".to_owned(),
            ))
        }
        WAIT_FAILED => {
            return Err(ClientError::Io {
                operation: "failed to retain connected Coven daemon process identity",
                source: std::io::Error::last_os_error(),
            })
        }
        result => {
            return Err(ClientError::Io {
                operation: "failed to retain connected Coven daemon process identity",
                source: std::io::Error::other(format!("unexpected Windows wait result {result}")),
            })
        }
    }
    let mut confirmed_pid = 0;
    if unsafe { GetNamedPipeServerProcessId(stream.as_raw_handle(), &mut confirmed_pid) } == 0
        || confirmed_pid != server_pid
    {
        return Err(ClientError::Discovery(
            "connected Coven daemon pipe server identity changed during validation".to_owned(),
        ));
    }

    Ok(PeerIdentity {
        server_pid,
        process_creation_time: filetime_parts_to_u64(
            creation.dwLowDateTime,
            creation.dwHighDateTime,
        ),
    })
}

#[cfg(windows)]
fn set_windows_pipe_nonblocking(stream: &std::fs::File) -> Result<(), ClientError> {
    use std::{os::windows::io::AsRawHandle, ptr};
    use windows_sys::Win32::System::Pipes::{
        SetNamedPipeHandleState, PIPE_NOWAIT, PIPE_READMODE_BYTE,
    };

    let mode = PIPE_READMODE_BYTE | PIPE_NOWAIT;
    // SAFETY: the borrowed file owns a connected pipe handle and the mode
    // pointer remains valid for the call.
    if unsafe { SetNamedPipeHandleState(stream.as_raw_handle(), &mode, ptr::null(), ptr::null()) }
        == 0
    {
        return Err(ClientError::Io {
            operation: "failed to configure nonblocking Coven daemon pipe writes",
            source: std::io::Error::last_os_error(),
        });
    }
    Ok(())
}

fn write_windows_pipe_with_deadline<W: Write>(
    writer: &mut W,
    mut buffer: &[u8],
    deadline: Instant,
    operation: &'static str,
) -> Result<(), ClientError> {
    while !buffer.is_empty() {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .filter(|remaining| !remaining.is_zero())
            .ok_or_else(|| ClientError::Io {
                operation,
                source: std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "timed out writing Coven daemon request",
                ),
            })?;
        match writer.write(buffer) {
            Ok(0) => std::thread::sleep(remaining.min(Duration::from_millis(1))),
            Ok(written) => buffer = &buffer[written..],
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock
                        | std::io::ErrorKind::TimedOut
                        | std::io::ErrorKind::Interrupted
                ) =>
            {
                if error.kind() != std::io::ErrorKind::Interrupted {
                    std::thread::sleep(remaining.min(Duration::from_millis(1)));
                }
            }
            Err(source) => return Err(ClientError::Io { operation, source }),
        }
    }
    Ok(())
}

#[cfg(any(windows, test))]
#[derive(Debug, PartialEq, Eq)]
struct WindowsStopIdentity {
    pid: u32,
    creation_time: u64,
}

#[cfg(any(windows, test))]
fn recorded_windows_process_matches_pipe_server(
    expected_creation_time: Option<u64>,
    server_creation_time: u64,
) -> bool {
    expected_creation_time.is_none_or(|expected| expected == server_creation_time)
}

#[cfg(any(windows, test))]
fn windows_stop_identity_from_health(
    pipe_name: &str,
    expected_pid: u32,
    expected_creation_time: Option<u64>,
    server_pid: u32,
    server_creation_time: u64,
    body: &[u8],
) -> Result<WindowsStopIdentity, ClientError> {
    #[derive(serde::Deserialize)]
    struct StopHealth {
        ok: bool,
        daemon: Option<StopDaemonIdentity>,
    }

    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct StopDaemonIdentity {
        pid: u32,
        socket: String,
        #[serde(default)]
        process_creation_time: Option<String>,
    }

    if !crate::discovery::is_coven_daemon_pipe_name(pipe_name) {
        return Err(ClientError::Discovery(
            "unsupported Coven daemon pipe name".to_owned(),
        ));
    }
    let health: StopHealth = serde_json::from_slice(body).map_err(|_| {
        ClientError::Discovery("Windows daemon health identity was invalid".to_owned())
    })?;
    let daemon = health.daemon.filter(|_| health.ok).ok_or_else(|| {
        ClientError::Discovery("Windows daemon health did not report a ready identity".to_owned())
    })?;
    if daemon.socket != pipe_name {
        return Err(ClientError::Discovery(
            "Windows daemon health reported a pipe for a different Coven home".to_owned(),
        ));
    }
    if expected_pid == 0 || daemon.pid != expected_pid || server_pid != expected_pid {
        return Err(ClientError::Discovery(
            "Windows daemon health PID did not match the connected pipe server".to_owned(),
        ));
    }
    if expected_creation_time.is_some_and(|expected| expected != server_creation_time) {
        return Err(ClientError::Discovery(
            "recorded Windows daemon process creation time did not match the connected pipe server"
                .to_owned(),
        ));
    }
    if let Some(reported) = daemon.process_creation_time {
        let reported = reported.parse::<u64>().map_err(|_| {
            ClientError::Discovery(
                "Windows daemon health process creation time was invalid".to_owned(),
            )
        })?;
        if reported == 0 || reported != server_creation_time {
            return Err(ClientError::Discovery(
                "Windows daemon health process creation time did not match the connected pipe server"
                    .to_owned(),
            ));
        }
    }
    Ok(WindowsStopIdentity {
        pid: expected_pid,
        creation_time: server_creation_time,
    })
}

#[cfg(any(windows, test))]
fn windows_stop_pipe_preflight(bytes_available: u32) -> Result<(), ClientError> {
    if bytes_available == 0 {
        Ok(())
    } else {
        Err(ClientError::Discovery(
            "connected Coven daemon pipe sent data before its health identity request".to_owned(),
        ))
    }
}

#[cfg(windows)]
pub struct WindowsDaemonProcess {
    process: std::os::windows::io::OwnedHandle,
    pid: u32,
    creation_time: u64,
}

#[cfg(windows)]
impl WindowsDaemonProcess {
    pub fn pid(&self) -> u32 {
        self.pid
    }

    #[doc(hidden)]
    pub fn creation_time(&self) -> u64 {
        self.creation_time
    }

    pub fn terminate_and_wait(self, timeout: Duration) -> Result<bool, ClientError> {
        let deadline = Instant::now()
            .checked_add(timeout)
            .ok_or_else(connect_timeout_error)?;
        self.terminate_and_wait_until(deadline)
    }

    #[doc(hidden)]
    pub fn terminate_and_wait_until(self, deadline: Instant) -> Result<bool, ClientError> {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::{
            Foundation::{WAIT_FAILED, WAIT_OBJECT_0, WAIT_TIMEOUT},
            System::Threading::{TerminateProcess, WaitForSingleObject},
        };

        let handle = self.process.as_raw_handle();
        remaining_for_phase(
            deadline,
            Instant::now(),
            "terminating verified Coven daemon process",
        )?;
        if unsafe { TerminateProcess(handle, 1) } == 0 {
            let source = std::io::Error::last_os_error();
            if unsafe { WaitForSingleObject(handle, 0) } == WAIT_OBJECT_0 {
                return Ok(true);
            }
            return Err(ClientError::Io {
                operation: "failed to terminate verified Coven daemon process",
                source,
            });
        }

        let remaining = remaining_for_phase(
            deadline,
            Instant::now(),
            "waiting for verified Coven daemon process to exit",
        )?;
        let milliseconds = remaining.as_millis().min((u32::MAX - 1) as u128) as u32;
        match unsafe { WaitForSingleObject(handle, milliseconds) } {
            WAIT_OBJECT_0 => Ok(true),
            WAIT_TIMEOUT => Ok(false),
            WAIT_FAILED => Err(ClientError::Io {
                operation: "failed to wait for verified Coven daemon process",
                source: std::io::Error::last_os_error(),
            }),
            result => Err(ClientError::Io {
                operation: "failed to wait for verified Coven daemon process",
                source: std::io::Error::other(format!("unexpected Windows wait result {result}")),
            }),
        }
    }
}

#[cfg(windows)]
#[doc(hidden)]
pub fn windows_process_creation_time(pid: u32) -> Result<Option<u64>, ClientError> {
    use std::os::windows::io::{AsRawHandle, FromRawHandle};
    use windows_sys::Win32::{
        Foundation::{ERROR_INVALID_PARAMETER, FILETIME, WAIT_FAILED, WAIT_OBJECT_0, WAIT_TIMEOUT},
        System::Threading::{
            GetProcessTimes, OpenProcess, WaitForSingleObject, PROCESS_QUERY_LIMITED_INFORMATION,
        },
    };

    const SYNCHRONIZE_ACCESS: u32 = 0x0010_0000;
    let process = unsafe {
        OpenProcess(
            PROCESS_QUERY_LIMITED_INFORMATION | SYNCHRONIZE_ACCESS,
            0,
            pid,
        )
    };
    if process.is_null() {
        let source = std::io::Error::last_os_error();
        return if source.raw_os_error() == Some(ERROR_INVALID_PARAMETER as i32) {
            Ok(None)
        } else {
            Err(ClientError::Io {
                operation: "failed to inspect recorded Coven daemon process identity",
                source,
            })
        };
    }
    let process = unsafe { std::os::windows::io::OwnedHandle::from_raw_handle(process) };
    let mut creation = FILETIME::default();
    let mut exit = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    if unsafe {
        GetProcessTimes(
            process.as_raw_handle(),
            &mut creation,
            &mut exit,
            &mut kernel,
            &mut user,
        )
    } == 0
    {
        return Err(ClientError::Io {
            operation: "failed to inspect recorded Coven daemon process identity",
            source: std::io::Error::last_os_error(),
        });
    }
    match unsafe { WaitForSingleObject(process.as_raw_handle(), 0) } {
        WAIT_TIMEOUT => Ok(Some(filetime_parts_to_u64(
            creation.dwLowDateTime,
            creation.dwHighDateTime,
        ))),
        WAIT_OBJECT_0 => Ok(None),
        WAIT_FAILED => Err(ClientError::Io {
            operation: "failed to inspect recorded Coven daemon process identity",
            source: std::io::Error::last_os_error(),
        }),
        result => Err(ClientError::Io {
            operation: "failed to inspect recorded Coven daemon process identity",
            source: std::io::Error::other(format!("unexpected Windows wait result {result}")),
        }),
    }
}

#[cfg(windows)]
#[doc(hidden)]
pub fn open_windows_daemon_process_for_stop(
    pipe_name: &str,
    expected_pid: u32,
    timeout: Duration,
) -> Result<Option<WindowsDaemonProcess>, ClientError> {
    open_windows_daemon_process_for_stop_with_creation_time(pipe_name, expected_pid, None, timeout)
}

#[cfg(windows)]
#[doc(hidden)]
pub fn open_windows_daemon_process_for_stop_with_creation_time(
    pipe_name: &str,
    expected_pid: u32,
    expected_creation_time: Option<u64>,
    timeout: Duration,
) -> Result<Option<WindowsDaemonProcess>, ClientError> {
    let deadline = Instant::now()
        .checked_add(timeout)
        .ok_or_else(connect_timeout_error)?;
    open_windows_daemon_process_for_stop_until(
        pipe_name,
        expected_pid,
        expected_creation_time,
        deadline,
    )
}

#[cfg(windows)]
#[doc(hidden)]
pub fn open_windows_daemon_process_for_stop_until(
    pipe_name: &str,
    expected_pid: u32,
    expected_creation_time: Option<u64>,
    deadline: Instant,
) -> Result<Option<WindowsDaemonProcess>, ClientError> {
    use std::os::windows::io::{AsRawHandle, FromRawHandle};
    use windows_sys::Win32::{
        Foundation::{FILETIME, WAIT_FAILED, WAIT_OBJECT_0, WAIT_TIMEOUT},
        System::{
            Pipes::{GetNamedPipeServerProcessId, PeekNamedPipe},
            Threading::{
                GetProcessTimes, OpenProcess, WaitForSingleObject,
                PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE,
            },
        },
    };

    if !is_coven_daemon_pipe_name(pipe_name) {
        return Err(ClientError::Discovery(
            "unsupported Coven daemon pipe name".to_owned(),
        ));
    }
    let mut stream = match connect_validated_with_deadline(pipe_name, deadline) {
        Ok(stream) => stream,
        Err(error) if windows_pipe_connection_is_unavailable(&error) => return Ok(None),
        Err(error) => return Err(error),
    };
    remaining_for_phase(
        deadline,
        Instant::now(),
        "authenticating connected Coven daemon process",
    )?;

    let mut server_pid = 0;
    if unsafe { GetNamedPipeServerProcessId(stream.as_raw_handle(), &mut server_pid) } == 0 {
        return Err(ClientError::Io {
            operation: "failed to identify connected Coven daemon pipe server",
            source: std::io::Error::last_os_error(),
        });
    }
    const SYNCHRONIZE_ACCESS: u32 = 0x0010_0000;
    let process = unsafe {
        OpenProcess(
            PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_TERMINATE | SYNCHRONIZE_ACCESS,
            0,
            server_pid,
        )
    };
    if process.is_null() {
        return Err(ClientError::Io {
            operation: "failed to retain connected Coven daemon process identity",
            source: std::io::Error::last_os_error(),
        });
    }
    let process = unsafe { std::os::windows::io::OwnedHandle::from_raw_handle(process) };

    let mut creation = FILETIME::default();
    let mut exit = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    if unsafe {
        GetProcessTimes(
            process.as_raw_handle(),
            &mut creation,
            &mut exit,
            &mut kernel,
            &mut user,
        )
    } == 0
    {
        return Err(ClientError::Io {
            operation: "failed to inspect connected Coven daemon process identity",
            source: std::io::Error::last_os_error(),
        });
    }
    let creation_time = filetime_parts_to_u64(creation.dwLowDateTime, creation.dwHighDateTime);
    if !recorded_windows_process_matches_pipe_server(expected_creation_time, creation_time) {
        return Ok(None);
    }

    // A dead server could otherwise leave a health-shaped response buffered,
    // then have its PID reused before OpenProcess. Requiring an empty pipe
    // before our request makes the response below prove that the process
    // retained above was still the connected server after its handle opened.
    let mut bytes_available = 0;
    if unsafe {
        PeekNamedPipe(
            stream.as_raw_handle(),
            std::ptr::null_mut(),
            0,
            std::ptr::null_mut(),
            &mut bytes_available,
            std::ptr::null_mut(),
        )
    } == 0
    {
        return Err(ClientError::Io {
            operation: "failed to inspect connected Coven daemon pipe state",
            source: std::io::Error::last_os_error(),
        });
    }
    windows_stop_pipe_preflight(bytes_available)?;

    remaining_for_phase(
        deadline,
        Instant::now(),
        "requesting authenticated Coven daemon health identity",
    )?;
    const OPERATION: &str = "failed to write Coven daemon stop identity request";
    write_windows_pipe_with_deadline(
        &mut stream,
        b"GET /health HTTP/1.1\r\nHost: coven\r\nContent-Length: 0\r\n\r\n",
        deadline,
        OPERATION,
    )?;
    let response = read_windows_framed_response(&stream, deadline, MAX_RESPONSE_BODY_BYTES)?;
    remaining_for_phase(
        deadline,
        Instant::now(),
        "authenticating Coven daemon health identity",
    )?;
    if response.status != 200 {
        return Err(ClientError::HttpStatus(response.status));
    }
    let identity = windows_stop_identity_from_health(
        pipe_name,
        expected_pid,
        expected_creation_time,
        server_pid,
        creation_time,
        &response.body,
    )?;
    let mut confirmed_server_pid = 0;
    if unsafe { GetNamedPipeServerProcessId(stream.as_raw_handle(), &mut confirmed_server_pid) }
        == 0
        || confirmed_server_pid != identity.pid
    {
        return Err(ClientError::Discovery(
            "connected Coven daemon pipe server identity changed during validation".to_owned(),
        ));
    }
    match unsafe { WaitForSingleObject(process.as_raw_handle(), 0) } {
        WAIT_TIMEOUT => {}
        WAIT_OBJECT_0 => {
            return Err(ClientError::Discovery(
                "connected Coven daemon process exited during identity validation".to_owned(),
            ))
        }
        WAIT_FAILED => {
            return Err(ClientError::Io {
                operation: "failed to retain connected Coven daemon process identity",
                source: std::io::Error::last_os_error(),
            })
        }
        result => {
            return Err(ClientError::Io {
                operation: "failed to retain connected Coven daemon process identity",
                source: std::io::Error::other(format!("unexpected Windows wait result {result}")),
            })
        }
    }
    remaining_for_phase(
        deadline,
        Instant::now(),
        "retaining verified Coven daemon process identity",
    )?;

    Ok(Some(WindowsDaemonProcess {
        process,
        pid: identity.pid,
        creation_time: identity.creation_time,
    }))
}

#[cfg(windows)]
/// Internal compatibility probe for `coven-cli` daemon lifecycle commands.
///
/// This accepts only Coven-shaped, owner-only pipe names and applies one
/// deadline to the validated connection, request write, and framed response.
/// It is intentionally not a general-purpose transport entry point.
#[doc(hidden)]
pub fn probe_windows_daemon_health(
    pipe_name: &str,
    timeout: Duration,
) -> Result<Option<(u16, Vec<u8>)>, ClientError> {
    Ok(
        probe_windows_daemon_health_with_identity(pipe_name, timeout)?
            .map(|probe| (probe.status, probe.body)),
    )
}

#[cfg(windows)]
#[doc(hidden)]
pub struct WindowsDaemonHealthProbe {
    pub status: u16,
    pub body: Vec<u8>,
    pub server_pid: u32,
    pub process_creation_time: u64,
}

#[cfg(windows)]
#[doc(hidden)]
pub fn probe_windows_daemon_health_with_identity(
    pipe_name: &str,
    timeout: Duration,
) -> Result<Option<WindowsDaemonHealthProbe>, ClientError> {
    let deadline = Instant::now()
        .checked_add(timeout)
        .ok_or_else(connect_timeout_error)?;
    probe_windows_daemon_health_with_identity_until(pipe_name, deadline)
}

#[cfg(windows)]
#[doc(hidden)]
pub fn probe_windows_daemon_health_with_identity_until(
    pipe_name: &str,
    deadline: Instant,
) -> Result<Option<WindowsDaemonHealthProbe>, ClientError> {
    if !is_coven_daemon_pipe_name(pipe_name) {
        return Err(ClientError::Discovery(
            "unsupported Coven daemon pipe name".to_owned(),
        ));
    }
    let mut stream = match connect_validated_with_deadline(pipe_name, deadline) {
        Ok(stream) => stream,
        Err(error) if windows_pipe_connection_is_unavailable(&error) => return Ok(None),
        Err(error) => return Err(error),
    };
    remaining_for_phase(
        deadline,
        Instant::now(),
        "authenticating connected Coven daemon pipe",
    )?;
    let peer_identity = connected_windows_peer_identity(&stream)?;
    remaining_for_phase(deadline, Instant::now(), "requesting Coven daemon health")?;
    const OPERATION: &str = "failed to write Coven daemon health request";
    write_windows_pipe_with_deadline(
        &mut stream,
        b"GET /health HTTP/1.1\r\nHost: coven\r\nContent-Length: 0\r\n\r\n",
        deadline,
        OPERATION,
    )?;
    let response = read_windows_framed_response(&stream, deadline, MAX_RESPONSE_BODY_BYTES)?;
    remaining_for_phase(
        deadline,
        Instant::now(),
        "authenticating Coven daemon health response",
    )?;
    let confirmed_peer = connected_windows_peer_identity(&stream)?;
    remaining_for_phase(
        deadline,
        Instant::now(),
        "authenticating Coven daemon health response",
    )?;
    if confirmed_peer != peer_identity {
        return Err(ClientError::DaemonInstanceChanged);
    }
    Ok(Some(WindowsDaemonHealthProbe {
        status: response.status,
        body: response.body,
        server_pid: peer_identity.server_pid,
        process_creation_time: peer_identity.process_creation_time,
    }))
}

#[cfg(any(windows, test))]
fn windows_pipe_connection_is_unavailable(error: &ClientError) -> bool {
    const ERROR_FILE_NOT_FOUND_CODE: i32 = 2;
    const ERROR_PATH_NOT_FOUND_CODE: i32 = 3;

    matches!(
        error,
        ClientError::Io { source, .. }
            if matches!(
                source.raw_os_error(),
                Some(ERROR_FILE_NOT_FOUND_CODE | ERROR_PATH_NOT_FOUND_CODE)
            )
    )
}

#[cfg(windows)]
fn read_response(
    stream: &mut std::fs::File,
    deadline: Instant,
) -> Result<HttpResponse, ClientError> {
    read_windows_framed_response(stream, deadline, MAX_RESPONSE_BODY_BYTES)
}

#[cfg(windows)]
struct WindowsPipeReader<'a>(&'a std::fs::File);

#[cfg(windows)]
impl Read for WindowsPipeReader<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> Result<usize, std::io::Error> {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::ReadFile;

        let mut bytes_read = 0;
        let buffer_len = u32::try_from(buffer.len()).unwrap_or(u32::MAX);
        // SAFETY: the borrowed file owns a synchronous connected pipe handle,
        // `buffer` is writable for `buffer_len` bytes, and both out-pointers
        // remain valid for this nonblocking call.
        if unsafe {
            ReadFile(
                self.0.as_raw_handle(),
                buffer.as_mut_ptr(),
                buffer_len,
                &mut bytes_read,
                std::ptr::null_mut(),
            )
        } == 0
        {
            // File::read turns ERROR_NO_DATA into Ok(0) because Windows maps it
            // to BrokenPipe. Preserve the raw status so the parser can tell a
            // live empty pipe from EOF and an actual disconnect.
            Err(std::io::Error::last_os_error())
        } else {
            Ok(bytes_read as usize)
        }
    }
}

#[cfg(windows)]
fn read_windows_framed_response(
    stream: &std::fs::File,
    deadline: Instant,
    max_body_bytes: usize,
) -> Result<HttpResponse, ClientError> {
    read_framed_response(&mut WindowsPipeReader(stream), deadline, max_body_bytes)
}

fn read_framed_response<R: Read>(
    reader: &mut R,
    deadline: Instant,
    max_body_bytes: usize,
) -> Result<HttpResponse, ClientError> {
    let mut response = Vec::with_capacity(1024);
    let mut chunk = [0_u8; 4096];

    loop {
        if let Some((head, body)) = split_response(&response) {
            if head.len() > MAX_RESPONSE_HEADERS_BYTES {
                return Err(ClientError::InvalidHttpResponse(format!(
                    "response headers exceeded {MAX_RESPONSE_HEADERS_BYTES} bytes"
                )));
            }
            let header = std::str::from_utf8(head).map_err(|_| {
                ClientError::InvalidHttpResponse("headers were not UTF-8".to_owned())
            })?;
            let status = header
                .lines()
                .next()
                .and_then(|line| line.split_whitespace().nth(1))
                .and_then(|status| status.parse().ok())
                .ok_or_else(|| {
                    ClientError::InvalidHttpResponse("missing HTTP status".to_owned())
                })?;
            let content_length = content_length(header)?;
            if content_length > max_body_bytes {
                return Err(ClientError::ResponseTooLarge {
                    max_bytes: max_body_bytes,
                });
            }
            match body.len().cmp(&content_length) {
                std::cmp::Ordering::Greater => {
                    return Err(ClientError::InvalidHttpResponse(
                        TRAILING_RESPONSE_BYTES.to_owned(),
                    ));
                }
                std::cmp::Ordering::Equal => {
                    ensure_no_immediately_available_response_bytes(reader, deadline)?;
                    return Ok(HttpResponse {
                        status,
                        body: body.to_vec(),
                    });
                }
                std::cmp::Ordering::Less => {}
            }
        } else if response.len() > MAX_RESPONSE_HEADERS_BYTES {
            return Err(ClientError::InvalidHttpResponse(format!(
                "response headers exceeded {MAX_RESPONSE_HEADERS_BYTES} bytes"
            )));
        }
        if Instant::now() >= deadline {
            return Err(ClientError::InvalidHttpResponse(
                "timed out reading Coven daemon response".to_owned(),
            ));
        }
        match reader.read(&mut chunk) {
            Ok(0) => {
                return Err(ClientError::InvalidHttpResponse(
                    "connection closed before response completed".to_owned(),
                ))
            }
            Ok(read) => response.extend_from_slice(&chunk[..read]),
            Err(error)
                if error.raw_os_error() == Some(ERROR_NO_DATA_CODE)
                    || matches!(
                        error.kind(),
                        std::io::ErrorKind::WouldBlock
                            | std::io::ErrorKind::TimedOut
                            | std::io::ErrorKind::Interrupted
                    ) =>
            {
                if error.kind() != std::io::ErrorKind::Interrupted {
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    if remaining.is_zero() {
                        return Err(ClientError::InvalidHttpResponse(
                            "timed out reading Coven daemon response".to_owned(),
                        ));
                    }
                    std::thread::sleep(remaining.min(Duration::from_millis(10)));
                }
            }
            Err(source) => {
                return Err(ClientError::Io {
                    operation: "failed to read Coven daemon response",
                    source,
                })
            }
        }
    }
}

fn ensure_no_immediately_available_response_bytes<R: Read>(
    reader: &mut R,
    deadline: Instant,
) -> Result<(), ClientError> {
    let mut byte = [0_u8; 1];
    loop {
        match reader.read(&mut byte) {
            Ok(0) => return Ok(()),
            Ok(_) => {
                return Err(ClientError::InvalidHttpResponse(
                    TRAILING_RESPONSE_BYTES.to_owned(),
                ));
            }
            Err(source)
                if source.raw_os_error() == Some(ERROR_NO_DATA_CODE)
                    || source.raw_os_error() == Some(ERROR_BROKEN_PIPE_CODE)
                    || matches!(
                        source.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) =>
            {
                return Ok(());
            }
            Err(source) if source.kind() == std::io::ErrorKind::Interrupted => {
                if Instant::now() >= deadline {
                    return Err(ClientError::InvalidHttpResponse(
                        "timed out validating Coven daemon response framing".to_owned(),
                    ));
                }
            }
            Err(source) => {
                return Err(ClientError::Io {
                    operation: "failed to validate Coven daemon response framing",
                    source,
                });
            }
        }
    }
}

fn content_length(headers: &str) -> Result<usize, ClientError> {
    let mut content_length = None;
    for line in headers.lines().skip(1) {
        let Some((name, value)) = line.split_once(':') else {
            return Err(ClientError::InvalidHttpResponse(
                "malformed HTTP response header".to_owned(),
            ));
        };
        if name.eq_ignore_ascii_case("transfer-encoding") {
            return Err(ClientError::InvalidHttpResponse(format!(
                "unsupported Transfer-Encoding: {}",
                value.trim()
            )));
        }
        if name.eq_ignore_ascii_case("content-length") {
            if content_length.is_some() {
                return Err(ClientError::InvalidHttpResponse(
                    "duplicate Content-Length header".to_owned(),
                ));
            }
            content_length = Some(value.trim().parse().map_err(|_| {
                ClientError::InvalidHttpResponse("invalid Content-Length header".to_owned())
            })?);
        }
    }
    content_length
        .ok_or_else(|| ClientError::InvalidHttpResponse("missing Content-Length header".to_owned()))
}

fn split_response(response: &[u8]) -> Option<(&[u8], &[u8])> {
    response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|position| (&response[..position], &response[position + 4..]))
        .or_else(|| {
            response
                .windows(2)
                .position(|window| window == b"\n\n")
                .map(|position| (&response[..position], &response[position + 2..]))
        })
}

#[cfg(test)]
mod tests {
    use super::{
        filetime_parts_to_u64, finite_windows_wait_millis, read_framed_response,
        windows_pipe_client_flags, windows_pipe_connection_is_unavailable,
        windows_stop_identity_from_health, windows_stop_pipe_preflight,
        write_windows_pipe_with_deadline, MAX_RESPONSE_HEADERS_BYTES,
    };
    use crate::{discovery::supported_windows_pipe_names, ClientError};
    use std::{
        collections::VecDeque,
        io::{Cursor, Read, Write},
        path::Path,
        time::{Duration, Instant},
    };

    // Regression test for a Windows-side gap: once the header terminator is
    // found, an over-sized header block must be rejected immediately rather
    // than falling through to header parsing (or the much looser
    // `MAX_RESPONSE_HEADERS_BYTES + max_body_bytes` catch-all that only
    // guards against a terminator never arriving).
    #[test]
    fn oversized_headers_are_rejected_once_the_terminator_is_found() {
        let mut oversized = vec![b'x'; MAX_RESPONSE_HEADERS_BYTES + 1];
        oversized.extend_from_slice(b"\r\n\r\n");
        let mut reader = Cursor::new(oversized);

        match read_framed_response(&mut reader, Instant::now() + Duration::from_secs(1), 1024) {
            Ok(_) => panic!("oversized headers must be rejected"),
            Err(error) => assert!(
                error.to_string().contains("response headers exceeded"),
                "unexpected error: {error}"
            ),
        }
    }

    #[test]
    fn unterminated_oversized_headers_are_rejected_at_the_header_limit() {
        let mut reader = Cursor::new(vec![b'x'; MAX_RESPONSE_HEADERS_BYTES + 1]);

        match read_framed_response(
            &mut reader,
            Instant::now() + Duration::from_secs(1),
            4 * 1024 * 1024,
        ) {
            Ok(_) => panic!("unterminated oversized headers must be rejected"),
            Err(error) => assert!(
                error.to_string().contains("response headers exceeded"),
                "unexpected error: {error}"
            ),
        }
    }

    struct NoDataOnceReader {
        returned_no_data: bool,
        response: Cursor<&'static [u8]>,
    }

    impl Read for NoDataOnceReader {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            if !self.returned_no_data {
                self.returned_no_data = true;
                return Err(std::io::Error::from_raw_os_error(232));
            }
            self.response.read(buffer)
        }
    }

    #[test]
    fn empty_nonblocking_pipe_reads_are_retried() {
        let mut reader = NoDataOnceReader {
            returned_no_data: false,
            response: Cursor::new(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}"),
        };

        let response =
            read_framed_response(&mut reader, Instant::now() + Duration::from_secs(1), 1024)
                .expect("ERROR_NO_DATA from an empty PIPE_NOWAIT pipe must be retried");

        assert_eq!(response.status, 200);
        assert_eq!(response.body, b"{}");
    }

    struct ChunkedReader {
        chunks: VecDeque<&'static [u8]>,
    }

    impl Read for ChunkedReader {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            let Some(chunk) = self.chunks.pop_front() else {
                return Ok(0);
            };
            assert!(chunk.len() <= buffer.len());
            buffer[..chunk.len()].copy_from_slice(chunk);
            Ok(chunk.len())
        }
    }

    fn assert_trailing_response_rejected(reader: &mut impl Read) {
        let error =
            match read_framed_response(reader, Instant::now() + Duration::from_secs(1), 1024) {
                Ok(_) => panic!("bytes beyond Content-Length must be rejected"),
                Err(error) => error,
            };

        assert!(matches!(
            error,
            ClientError::InvalidHttpResponse(message)
                if message == super::TRAILING_RESPONSE_BYTES
        ));
    }

    #[test]
    fn response_reader_rejects_coalesced_bytes_beyond_content_length() {
        let mut reader = Cursor::new(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}X".as_slice());

        assert_trailing_response_rejected(&mut reader);
    }

    #[test]
    fn response_reader_rejects_a_delayed_body_coalesced_with_trailing_bytes() {
        let mut reader = ChunkedReader {
            chunks: VecDeque::from([
                &b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\n\r\n"[..],
                &b"bodyX"[..],
            ]),
        };

        assert_trailing_response_rejected(&mut reader);
    }

    #[test]
    fn response_reader_rejects_an_immediately_available_byte_after_an_exact_body() {
        let mut reader = ChunkedReader {
            chunks: VecDeque::from([
                &b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\n\r\nbody"[..],
                &b"X"[..],
            ]),
        };

        assert_trailing_response_rejected(&mut reader);
    }

    #[test]
    fn response_reader_probes_beyond_a_zero_length_frame() {
        let mut reader = ChunkedReader {
            chunks: VecDeque::from([
                &b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n"[..],
                &b"X"[..],
            ]),
        };

        assert_trailing_response_rejected(&mut reader);
    }

    #[test]
    fn zero_byte_read_is_eof_not_live_pipe_backpressure() {
        let started = Instant::now();
        let error = match read_framed_response(
            &mut Cursor::new(Vec::<u8>::new()),
            Instant::now() + Duration::from_secs(1),
            1024,
        ) {
            Ok(_) => panic!("EOF before a response must fail"),
            Err(error) => error,
        };

        assert!(
            error
                .to_string()
                .contains("connection closed before response completed"),
            "unexpected error: {error}"
        );
        assert!(started.elapsed() < Duration::from_millis(100));
    }

    struct AlwaysNoDataReader {
        reads: usize,
    }

    impl Read for AlwaysNoDataReader {
        fn read(&mut self, _buffer: &mut [u8]) -> std::io::Result<usize> {
            self.reads += 1;
            Err(std::io::Error::from_raw_os_error(232))
        }
    }

    #[test]
    fn live_empty_pipe_polling_sleeps_and_obeys_the_absolute_deadline() {
        let mut reader = AlwaysNoDataReader { reads: 0 };
        let started = Instant::now();
        let error =
            match read_framed_response(&mut reader, started + Duration::from_millis(25), 1024) {
                Ok(_) => panic!("a permanently empty live pipe must time out"),
                Err(error) => error,
            };

        assert!(
            error
                .to_string()
                .contains("timed out reading Coven daemon response"),
            "unexpected error: {error}"
        );
        assert!(started.elapsed() < Duration::from_secs(1));
        assert!(
            reader.reads <= 5,
            "empty pipe polling spun {} times",
            reader.reads
        );
    }

    #[derive(Default)]
    struct ZeroOnceWriter {
        returned_zero: bool,
        bytes: Vec<u8>,
    }

    impl Write for ZeroOnceWriter {
        fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
            if !self.returned_zero {
                self.returned_zero = true;
                return Ok(0);
            }
            self.bytes.extend_from_slice(buffer);
            Ok(buffer.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn zero_byte_nonblocking_pipe_writes_are_retried() {
        let mut writer = ZeroOnceWriter::default();

        write_windows_pipe_with_deadline(
            &mut writer,
            b"payload",
            Instant::now() + Duration::from_secs(1),
            "write",
        )
        .expect("a zero-byte PIPE_NOWAIT write must be retried as backpressure");

        assert_eq!(writer.bytes, b"payload");
    }

    #[test]
    fn named_pipe_waits_are_finite_and_never_use_the_infinite_sentinel() {
        assert_eq!(finite_windows_wait_millis(Duration::from_micros(999)), None);
        assert_eq!(
            finite_windows_wait_millis(Duration::from_millis(1)),
            Some(1)
        );
        assert_eq!(
            finite_windows_wait_millis(Duration::from_millis(u64::from(u32::MAX) + 1)),
            Some(u32::MAX - 1)
        );
    }

    #[test]
    fn named_pipe_clients_request_identification_only_sqos() {
        assert_eq!(windows_pipe_client_flags(), 0x0011_0000);
    }

    #[test]
    fn one_absolute_deadline_threads_only_remaining_budget_to_each_phase() {
        let start = Instant::now();
        let deadline = start + Duration::from_millis(100);

        assert_eq!(
            super::remaining_for_phase(
                deadline,
                start + Duration::from_millis(70),
                "authenticating Coven daemon",
            )
            .expect("budget remains"),
            Duration::from_millis(30)
        );
        let error = super::remaining_for_phase(
            deadline,
            start + Duration::from_millis(100),
            "waiting for Coven daemon exit",
        )
        .expect_err("expired absolute deadline");
        assert!(matches!(
            error,
            ClientError::Io {
                operation: "waiting for Coven daemon exit",
                source,
            } if source.kind() == std::io::ErrorKind::TimedOut
        ));
    }

    #[test]
    fn only_missing_named_pipe_errors_are_classified_as_unavailable() {
        let io_error = |code| ClientError::Io {
            operation: "connect fixture",
            source: std::io::Error::from_raw_os_error(code),
        };

        assert!(windows_pipe_connection_is_unavailable(&io_error(2)));
        assert!(windows_pipe_connection_is_unavailable(&io_error(3)));
        assert!(!windows_pipe_connection_is_unavailable(&io_error(5)));
        assert!(!windows_pipe_connection_is_unavailable(
            &ClientError::Discovery("connected pipe handle was not owner-only".to_owned())
        ));
    }

    #[test]
    fn windows_stop_identity_binds_health_pid_and_socket_to_pipe_server() {
        let supported_pipes = supported_windows_pipe_names(Path::new(env!("CARGO_MANIFEST_DIR")))
            .expect("supported Windows pipe names");
        let pipe = supported_pipes[0].as_str();
        let creation_time = 134_157_822_123_456_789_u64;
        let health = |pid: u32, socket: &str, fingerprint: Option<&str>| {
            let fingerprint = fingerprint
                .map(|value| format!(r#","processCreationTime":"{value}""#))
                .unwrap_or_default();
            format!(
                r#"{{"ok":true,"daemon":{{"pid":{pid},"startedAt":"now","socket":"{socket}"{fingerprint}}}}}"#
            )
        };

        let identity = windows_stop_identity_from_health(
            pipe,
            41,
            Some(creation_time),
            41,
            creation_time,
            health(41, pipe, Some(&creation_time.to_string())).as_bytes(),
        )
        .expect("matching pipe server, status fingerprint, and health identity");
        assert_eq!(identity.pid, 41);
        assert_eq!(identity.creation_time, creation_time);

        windows_stop_identity_from_health(
            pipe,
            41,
            None,
            41,
            creation_time,
            health(41, pipe, None).as_bytes(),
        )
        .expect("authenticated legacy health may acquire the live process fingerprint");

        for (expected_pid, expected_creation, server_pid, server_creation, body) in [
            (
                40,
                Some(creation_time),
                41,
                creation_time,
                health(41, pipe, Some(&creation_time.to_string())),
            ),
            (
                41,
                Some(creation_time),
                42,
                creation_time,
                health(41, pipe, Some(&creation_time.to_string())),
            ),
            (
                41,
                Some(creation_time),
                41,
                creation_time,
                health(42, pipe, Some(&creation_time.to_string())),
            ),
            (
                41,
                Some(creation_time - 1),
                41,
                creation_time,
                health(41, pipe, Some(&creation_time.to_string())),
            ),
            (
                41,
                Some(creation_time),
                41,
                creation_time,
                health(41, pipe, Some(&(creation_time - 1).to_string())),
            ),
            (
                41,
                Some(creation_time),
                41,
                creation_time,
                health(
                    41,
                    "coven-daemon-v1-ffffffffffffffffffffffffffffffff.sock",
                    Some(&creation_time.to_string()),
                ),
            ),
        ] {
            assert!(
                windows_stop_identity_from_health(
                    pipe,
                    expected_pid,
                    expected_creation,
                    server_pid,
                    server_creation,
                    body.as_bytes(),
                )
                .is_err(),
                "accepted expected_pid={expected_pid}, expected_creation={expected_creation:?}, server_pid={server_pid}, server_creation={server_creation}, body={body}"
            );
        }
    }

    #[test]
    fn reused_pid_creation_mismatch_is_unverified_before_any_stop_request() {
        assert!(super::recorded_windows_process_matches_pipe_server(
            None, 200
        ));
        assert!(super::recorded_windows_process_matches_pipe_server(
            Some(200),
            200
        ));
        assert!(!super::recorded_windows_process_matches_pipe_server(
            Some(100),
            200
        ));
    }

    #[test]
    fn filetime_conversion_preserves_both_32_bit_halves() {
        assert_eq!(
            filetime_parts_to_u64(0x89ab_cdef, 0x0123_4567),
            0x0123_4567_89ab_cdef
        );
        assert_eq!(filetime_parts_to_u64(u32::MAX, u32::MAX), u64::MAX);
    }

    #[cfg(windows)]
    #[test]
    fn live_process_creation_time_queries_the_exact_running_process() {
        let creation_time = super::windows_process_creation_time(std::process::id())
            .expect("query current process")
            .expect("current process must be live");
        assert_ne!(creation_time, 0);
    }

    #[cfg(windows)]
    #[test]
    fn live_empty_named_pipe_waits_for_a_delayed_response() {
        use std::{
            ffi::OsStr,
            os::windows::{
                ffi::OsStrExt,
                io::{AsRawHandle, FromRawHandle},
            },
            ptr,
            sync::{
                atomic::{AtomicU64, Ordering},
                mpsc,
            },
            thread,
        };
        use windows_sys::Win32::{
            Foundation::{ERROR_PIPE_CONNECTED, INVALID_HANDLE_VALUE},
            Storage::FileSystem::{ReadFile, PIPE_ACCESS_DUPLEX},
            System::Pipes::{
                ConnectNamedPipe, CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_TYPE_BYTE, PIPE_WAIT,
            },
        };

        static NEXT_PIPE_ID: AtomicU64 = AtomicU64::new(0);
        let pipe_name = format!(
            "coven-client-read-regression-{}-{}",
            std::process::id(),
            NEXT_PIPE_ID.fetch_add(1, Ordering::Relaxed)
        );
        let pipe_path: Vec<u16> = OsStr::new(&format!(r"\\.\pipe\{pipe_name}"))
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        // SAFETY: `pipe_path` is NUL-terminated and the optional security
        // attributes pointer is null.
        let server_handle = unsafe {
            CreateNamedPipeW(
                pipe_path.as_ptr(),
                PIPE_ACCESS_DUPLEX,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
                1,
                4096,
                4096,
                0,
                ptr::null(),
            )
        };
        assert_ne!(
            server_handle,
            INVALID_HANDLE_VALUE,
            "create test named pipe: {}",
            std::io::Error::last_os_error()
        );
        // SAFETY: ownership of the fresh valid handle transfers exactly once.
        let mut server_pipe = unsafe { std::fs::File::from_raw_handle(server_handle) };
        let (connected_tx, connected_rx) = mpsc::channel();
        let (respond_tx, respond_rx) = mpsc::channel();
        let (read_done_tx, read_done_rx) = mpsc::channel();
        let server = thread::spawn(move || {
            // SAFETY: `server_pipe` owns a synchronous server pipe handle; a
            // null OVERLAPPED pointer selects synchronous connection setup.
            if unsafe { ConnectNamedPipe(server_pipe.as_raw_handle(), ptr::null_mut()) } == 0 {
                let error = std::io::Error::last_os_error();
                assert_eq!(
                    error.raw_os_error(),
                    Some(ERROR_PIPE_CONNECTED as i32),
                    "connect test named pipe: {error}"
                );
            }
            connected_tx.send(()).expect("signal connected test pipe");
            respond_rx.recv().expect("wait for client read");
            thread::sleep(Duration::from_millis(50));
            server_pipe
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}")
                .expect("write delayed response");
            read_done_rx.recv().expect("wait for response read");
        });

        let deadline = Instant::now() + Duration::from_secs(5);
        let client =
            super::connect_with_deadline(&pipe_name, deadline).expect("connect test client");
        super::set_windows_pipe_nonblocking(&client).expect("configure test client");
        connected_rx.recv().expect("wait for connected test pipe");

        let mut probe = [0_u8; 1];
        let mut bytes_read = 0;
        // SAFETY: `client` owns a connected synchronous pipe handle, and both
        // output buffers remain valid for this nonblocking call.
        let read_result = unsafe {
            ReadFile(
                client.as_raw_handle(),
                probe.as_mut_ptr(),
                probe.len() as u32,
                &mut bytes_read,
                ptr::null_mut(),
            )
        };
        let read_error = std::io::Error::last_os_error();
        assert_eq!(
            read_result, 0,
            "a live empty PIPE_NOWAIT read unexpectedly succeeded with {bytes_read} bytes"
        );
        assert_eq!(
            read_error.raw_os_error(),
            Some(super::ERROR_NO_DATA_CODE),
            "an empty connected pipe must report ERROR_NO_DATA"
        );
        respond_tx.send(()).expect("request delayed response");
        let response = super::read_windows_framed_response(&client, deadline, 1024);
        read_done_tx
            .send(())
            .expect("signal completed response read");
        server.join().expect("join test named-pipe server");
        let response = response.expect("wait for a delayed response from a live pipe");

        assert_eq!(response.status, 200);
        assert_eq!(response.body, b"{}");
    }

    #[test]
    fn windows_stop_rejects_a_response_buffered_before_its_health_request() {
        windows_stop_pipe_preflight(0).expect("empty connected pipe");
        assert!(windows_stop_pipe_preflight(1).is_err());
        assert!(windows_stop_pipe_preflight(u32::MAX).is_err());
    }

    struct DisconnectedReader;

    impl Read for DisconnectedReader {
        fn read(&mut self, _buffer: &mut [u8]) -> std::io::Result<usize> {
            Err(std::io::Error::from_raw_os_error(109))
        }
    }

    #[test]
    fn a_disconnected_pipe_read_fails_without_waiting_for_the_deadline() {
        let started = Instant::now();
        let error = match read_framed_response(
            &mut DisconnectedReader,
            Instant::now() + Duration::from_secs(1),
            1024,
        ) {
            Ok(_) => panic!("ERROR_BROKEN_PIPE is a disconnect, not backpressure"),
            Err(error) => error,
        };

        assert!(matches!(
            error,
            crate::ClientError::Io { source, .. } if source.raw_os_error() == Some(109)
        ));
        assert!(started.elapsed() < Duration::from_millis(100));
    }

    struct DisconnectedWriter;

    impl Write for DisconnectedWriter {
        fn write(&mut self, _buffer: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::from_raw_os_error(232))
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn a_disconnected_pipe_write_fails_without_waiting_for_the_deadline() {
        let started = Instant::now();
        let error = write_windows_pipe_with_deadline(
            &mut DisconnectedWriter,
            b"payload",
            Instant::now() + Duration::from_secs(1),
            "write",
        )
        .expect_err("ERROR_NO_DATA while writing is a disconnect, not backpressure");

        assert!(matches!(
            error,
            crate::ClientError::Io { source, .. } if source.raw_os_error() == Some(232)
        ));
        assert!(started.elapsed() < Duration::from_millis(100));
    }
}
