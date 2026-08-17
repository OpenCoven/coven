use std::{
    io::Read,
    os::unix::net::UnixStream,
    time::{Duration, Instant},
};

use socket2::{Domain, SockAddr, Socket, Type};

use crate::{
    error::{UNIX_CONFIGURE_WRITES_OPERATION, UNIX_CONNECT_OPERATION},
    transport::{
        write_with_deadline, HttpResponse, PeerIdentity, TransportResponse, MAX_RESPONSE_BODY_BYTES,
    },
    ClientError, DaemonEndpoint,
};

const MAX_RESPONSE_HEADERS_BYTES: usize = 64 * 1024;
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(5);
const MIN_READ_TIMEOUT: Duration = Duration::from_millis(1);

struct RetainedRequestOptions {
    timeout: Duration,
    max_response_body_bytes: usize,
    bind_process_identity: bool,
}

struct FramedResponse {
    response: HttpResponse,
    buffered_remainder: Vec<u8>,
}

pub(crate) fn request(
    endpoint: &DaemonEndpoint,
    method: &'static str,
    path: &str,
    body: Option<&[u8]>,
    expected_peer: Option<&PeerIdentity>,
) -> Result<TransportResponse, ClientError> {
    request_bound(
        endpoint,
        method,
        path,
        body,
        expected_peer,
        RESPONSE_TIMEOUT,
        MAX_RESPONSE_BODY_BYTES,
    )
}

pub(crate) fn verify_peer(
    endpoint: &DaemonEndpoint,
    expected_peer: &PeerIdentity,
) -> Result<(), ClientError> {
    let deadline = Instant::now()
        .checked_add(RESPONSE_TIMEOUT)
        .ok_or_else(|| {
            ClientError::InvalidHttpResponse("peer verification deadline overflowed".to_owned())
        })?;
    let (_stream, peer_identity) = connect_with_deadline(endpoint, deadline, false)?;
    if &peer_identity != expected_peer {
        return Err(ClientError::DaemonInstanceChanged);
    }
    Ok(())
}

fn request_bound(
    endpoint: &DaemonEndpoint,
    method: &'static str,
    path: &str,
    body: Option<&[u8]>,
    expected_peer: Option<&PeerIdentity>,
    timeout: Duration,
    max_response_body_bytes: usize,
) -> Result<TransportResponse, ClientError> {
    let pending = request_retaining_connection_bound(
        endpoint,
        method,
        path,
        body,
        expected_peer,
        RetainedRequestOptions {
            timeout,
            max_response_body_bytes,
            bind_process_identity: false,
        },
    )?;
    let (response, peer_identity) = pending.into_response_and_peer()?;
    Ok(TransportResponse {
        response,
        peer_identity,
    })
}

pub(crate) fn request_with_timeout(
    endpoint: &DaemonEndpoint,
    method: &'static str,
    path: &str,
    body: Option<&[u8]>,
    timeout: Duration,
    max_response_body_bytes: usize,
) -> Result<(HttpResponse, PeerIdentity), ClientError> {
    let pending = request_retaining_connection_bound(
        endpoint,
        method,
        path,
        body,
        None,
        RetainedRequestOptions {
            timeout,
            max_response_body_bytes,
            bind_process_identity: false,
        },
    )?;
    pending.into_response_and_peer()
}

pub(crate) fn request_with_timeout_bound(
    endpoint: &DaemonEndpoint,
    method: &'static str,
    path: &str,
    body: Option<&[u8]>,
    expected_peer: &PeerIdentity,
    timeout: Duration,
    max_response_body_bytes: usize,
) -> Result<HttpResponse, ClientError> {
    let pending = request_retaining_connection_bound(
        endpoint,
        method,
        path,
        body,
        Some(expected_peer),
        RetainedRequestOptions {
            timeout,
            max_response_body_bytes,
            bind_process_identity: true,
        },
    )?;
    Ok(pending.into_response_and_peer()?.0)
}

pub(crate) struct AuthenticatedUnixResponse {
    response: HttpResponse,
    peer_identity: PeerIdentity,
    stream: UnixStream,
    deadline: Instant,
    buffered_remainder: Vec<u8>,
}

impl AuthenticatedUnixResponse {
    pub(crate) fn peer_identity(&self) -> &PeerIdentity {
        &self.peer_identity
    }

    pub(crate) fn response(&self) -> Result<&HttpResponse, ClientError> {
        self.ensure_framing_valid("daemon sent bytes after its lifecycle response completed")?;
        Ok(&self.response)
    }

    fn into_response_and_peer(self) -> Result<(HttpResponse, PeerIdentity), ClientError> {
        self.ensure_framing_valid("daemon sent bytes after its response completed")?;
        Ok((self.response, self.peer_identity))
    }

    pub(crate) fn wait_for_close(mut self) -> Result<bool, ClientError> {
        self.ensure_framing_valid("daemon sent bytes after its lifecycle response completed")?;
        let mut byte = [0_u8; 1];
        loop {
            if Instant::now() >= self.deadline {
                return Ok(false);
            }
            match self.stream.read(&mut byte) {
                Ok(0) => return Ok(true),
                Ok(_) => {
                    return Err(ClientError::InvalidHttpResponse(
                        "daemon sent bytes after its lifecycle response completed".to_owned(),
                    ))
                }
                Err(source)
                    if matches!(
                        source.kind(),
                        std::io::ErrorKind::TimedOut
                            | std::io::ErrorKind::WouldBlock
                            | std::io::ErrorKind::Interrupted
                    ) =>
                {
                    if source.kind() != std::io::ErrorKind::Interrupted {
                        let remaining = self
                            .deadline
                            .saturating_duration_since(Instant::now())
                            .min(MIN_READ_TIMEOUT);
                        if !remaining.is_zero() {
                            std::thread::sleep(remaining);
                        }
                    }
                }
                Err(source) => {
                    return Err(ClientError::Io {
                        operation: "failed to wait for Coven daemon shutdown",
                        source,
                    })
                }
            }
        }
    }

    fn ensure_framing_valid(&self, message: &'static str) -> Result<(), ClientError> {
        if self.buffered_remainder.is_empty() {
            Ok(())
        } else {
            Err(ClientError::InvalidHttpResponse(message.to_owned()))
        }
    }
}

pub(crate) fn request_retaining_connection(
    endpoint: &DaemonEndpoint,
    method: &'static str,
    path: &str,
    body: Option<&[u8]>,
    timeout: Duration,
    max_response_body_bytes: usize,
) -> Result<AuthenticatedUnixResponse, ClientError> {
    request_retaining_connection_bound(
        endpoint,
        method,
        path,
        body,
        None,
        RetainedRequestOptions {
            timeout,
            max_response_body_bytes,
            bind_process_identity: true,
        },
    )
}

fn request_retaining_connection_bound(
    endpoint: &DaemonEndpoint,
    method: &'static str,
    path: &str,
    body: Option<&[u8]>,
    expected_peer: Option<&PeerIdentity>,
    options: RetainedRequestOptions,
) -> Result<AuthenticatedUnixResponse, ClientError> {
    // A single deadline covers connect, write, and read: previously only the
    // read phase was bounded, so a daemon that accepted a connection but
    // never read from it (or a socket whose listen backlog was full) could
    // block the caller indefinitely.
    let deadline = Instant::now().checked_add(options.timeout).ok_or_else(|| {
        ClientError::InvalidHttpResponse("request deadline overflowed".to_owned())
    })?;
    let body = body.unwrap_or_default();
    let request = format!(
        "{method} {path} HTTP/1.1\r\nHost: coven\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
        body.len()
    );
    let (mut stream, peer_identity) =
        connect_with_deadline(endpoint, deadline, options.bind_process_identity)?;
    if expected_peer.is_some_and(|expected| expected != &peer_identity) {
        return Err(ClientError::DaemonInstanceChanged);
    }
    stream
        .set_nonblocking(true)
        .map_err(|source| ClientError::Io {
            operation: UNIX_CONFIGURE_WRITES_OPERATION,
            source,
        })?;
    const WRITE_OPERATION: &str = "failed to write Coven daemon request";
    write_with_deadline(&mut stream, request.as_bytes(), deadline, WRITE_OPERATION)?;
    write_with_deadline(&mut stream, body, deadline, WRITE_OPERATION)?;
    stream
        .shutdown(std::net::Shutdown::Write)
        .map_err(|source| ClientError::Io {
            operation: "failed to finish Coven daemon request",
            source,
        })?;

    let framed = read_framed_response(&mut stream, deadline, options.max_response_body_bytes)?;
    Ok(AuthenticatedUnixResponse {
        response: framed.response,
        peer_identity,
        stream,
        deadline,
        buffered_remainder: framed.buffered_remainder,
    })
}

/// Connects to the daemon's Unix socket bounded by `deadline`.
///
/// `UnixStream::connect` has no built-in timeout and, for a listener whose
/// accept backlog is full, can block for an unbounded time. `socket2` performs
/// a nonblocking connect and polls for completion without leaving a worker
/// thread behind after a timeout.
fn connect_with_deadline(
    endpoint: &DaemonEndpoint,
    deadline: Instant,
    bind_process_identity: bool,
) -> Result<(UnixStream, PeerIdentity), ClientError> {
    const OPERATION: &str = "failed to connect to Coven daemon socket";
    let socket_identity = validated_socket_identity(endpoint)?;
    let socket =
        Socket::new(Domain::UNIX, Type::STREAM, None).map_err(|source| ClientError::Io {
            operation: OPERATION,
            source,
        })?;
    let address = SockAddr::unix(endpoint.socket()).map_err(|source| ClientError::Io {
        operation: OPERATION,
        source,
    })?;
    connect_operation_with_deadline(deadline, |remaining| {
        socket.connect_timeout(&address, remaining)
    })?;
    let socket: std::os::fd::OwnedFd = socket.into();
    let stream: UnixStream = socket.into();
    let peer = validate_connected_peer(&stream, endpoint.owner_uid())?;
    let bound_process = bind_process_identity
        .then(|| bind_connected_process_identity(&stream, endpoint.owner_uid(), peer))
        .transpose()?;
    let process_identity = bound_process.as_ref().map(|bound| bound.identity);
    #[cfg(target_os = "linux")]
    let process_handle = bound_process.and_then(|bound| bound.handle);
    let confirmed_socket_identity =
        validated_socket_identity(endpoint).map_err(|_| ClientError::DaemonInstanceChanged)?;
    if confirmed_socket_identity != socket_identity {
        return Err(ClientError::DaemonInstanceChanged);
    }
    Ok((
        stream,
        PeerIdentity {
            socket_device: socket_identity.device,
            socket_inode: socket_identity.inode,
            peer_pid: peer.pid,
            peer_uid: peer.uid,
            peer_gid: peer.gid,
            process_identity,
            #[cfg(target_os = "linux")]
            process_handle,
        },
    ))
}

struct BoundUnixProcessIdentity {
    identity: UnixProcessIdentity,
    #[cfg(target_os = "linux")]
    handle: Option<std::sync::Arc<LinuxProcessHandle>>,
}

fn bind_connected_process_identity(
    stream: &UnixStream,
    expected_uid: u32,
    peer: UnixPeerCredentials,
) -> Result<BoundUnixProcessIdentity, ClientError> {
    let identity = unix_process_identity(peer.pid)?.ok_or_else(|| {
        ClientError::Discovery(
            "connected Coven daemon peer exited before its process identity was bound".to_owned(),
        )
    })?;
    #[cfg(target_os = "linux")]
    let handle = LinuxProcessHandle::open(peer.pid)?.map(std::sync::Arc::new);
    let confirmed_peer = validate_connected_peer(stream, expected_uid)
        .map_err(|_| ClientError::DaemonInstanceChanged)?;
    if confirmed_peer != peer || unix_process_identity(peer.pid)? != Some(identity) {
        return Err(ClientError::DaemonInstanceChanged);
    }
    Ok(BoundUnixProcessIdentity {
        identity,
        #[cfg(target_os = "linux")]
        handle,
    })
}

fn connect_operation_with_deadline(
    deadline: Instant,
    connect: impl FnOnce(Duration) -> std::io::Result<()>,
) -> Result<(), ClientError> {
    let remaining = deadline
        .checked_duration_since(Instant::now())
        .filter(|remaining| !remaining.is_zero())
        .ok_or_else(|| ClientError::Io {
            operation: UNIX_CONNECT_OPERATION,
            source: std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "timed out connecting to Coven daemon socket",
            ),
        })?;
    connect(remaining).map_err(|source| ClientError::Io {
        operation: UNIX_CONNECT_OPERATION,
        source,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct UnixSocketIdentity {
    device: u64,
    inode: u64,
}

fn validated_socket_identity(endpoint: &DaemonEndpoint) -> Result<UnixSocketIdentity, ClientError> {
    use std::os::unix::fs::{FileTypeExt, MetadataExt};

    let metadata =
        std::fs::symlink_metadata(endpoint.socket()).map_err(|source| ClientError::Io {
            operation: UNIX_CONNECT_OPERATION,
            source,
        })?;
    let current_uid = unsafe { libc::geteuid() };
    if metadata.file_type().is_symlink()
        || !metadata.file_type().is_socket()
        || metadata.uid() != endpoint.owner_uid()
        || metadata.uid() != current_uid
        || metadata.mode() & 0o077 != 0
    {
        return Err(ClientError::Discovery(format!(
            "{} is no longer an owner-local Unix socket",
            endpoint.socket().display()
        )));
    }
    Ok(UnixSocketIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct UnixPeerCredentials {
    pid: u32,
    uid: u32,
    gid: u32,
}

#[cfg(target_os = "linux")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct UnixProcessIdentity {
    start_ticks: u64,
}

#[cfg(target_os = "macos")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct UnixProcessIdentity {
    start_seconds: u64,
    start_microseconds: u64,
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct UnixProcessIdentity;

#[cfg(target_os = "linux")]
#[derive(Debug)]
pub(crate) struct LinuxProcessHandle(std::os::fd::OwnedFd);

#[cfg(target_os = "linux")]
impl LinuxProcessHandle {
    fn open(pid: u32) -> Result<Option<Self>, ClientError> {
        use std::os::fd::FromRawFd;

        let pid = libc::pid_t::try_from(pid).map_err(|_| {
            ClientError::Discovery("connected Coven daemon peer PID exceeded pid_t".to_owned())
        })?;
        // SAFETY: pidfd_open takes a numeric PID and zero flags, returns a new
        // descriptor on success, and does not dereference userspace pointers.
        let descriptor = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0_u32) };
        if descriptor >= 0 {
            let descriptor = libc::c_int::try_from(descriptor).map_err(|_| {
                ClientError::Discovery("Linux pidfd exceeded the file-descriptor range".to_owned())
            })?;
            // SAFETY: a successful pidfd_open returned this fresh descriptor,
            // whose ownership transfers exactly once to OwnedFd.
            return Ok(Some(Self(unsafe {
                std::os::fd::OwnedFd::from_raw_fd(descriptor)
            })));
        }

        let source = std::io::Error::last_os_error();
        match source.raw_os_error() {
            Some(libc::ESRCH) => Err(ClientError::Discovery(
                "connected Coven daemon peer exited before its process handle was retained"
                    .to_owned(),
            )),
            // Do not block the authenticated shutdown route merely because
            // this kernel or app policy lacks pidfds. Exact-404 legacy fallback
            // checks the missing handle and fails closed with upgrade guidance.
            Some(libc::ENOSYS | libc::EPERM | libc::EACCES) => Ok(None),
            _ => Err(ClientError::Io {
                operation: "failed to retain connected Coven daemon process handle",
                source,
            }),
        }
    }

    pub(crate) fn send_sigterm(&self) -> Result<bool, ClientError> {
        use std::os::fd::AsRawFd;

        // SAFETY: the owned pidfd remains live for this call, SIGTERM is a
        // valid signal, siginfo is intentionally null, and flags must be zero.
        let result = unsafe {
            libc::syscall(
                libc::SYS_pidfd_send_signal,
                self.0.as_raw_fd(),
                libc::SIGTERM,
                std::ptr::null::<libc::siginfo_t>(),
                0_u32,
            )
        };
        if result == 0 {
            return Ok(true);
        }

        let source = std::io::Error::last_os_error();
        match source.raw_os_error() {
            Some(libc::ESRCH) => Ok(false),
            Some(libc::ENOSYS) => {
                Err(ClientError::LegacyShutdownUpgradeRequired { platform: "Linux" })
            }
            _ => Err(ClientError::Io {
                operation: "failed to signal authenticated BASE Coven daemon through pidfd",
                source,
            }),
        }
    }
}

#[cfg(target_os = "linux")]
pub(crate) fn unix_process_identity(pid: u32) -> Result<Option<UnixProcessIdentity>, ClientError> {
    let stat = match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(stat) => stat,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(ClientError::Io {
                operation: "failed to inspect Coven daemon process identity",
                source,
            })
        }
    };
    let Some(command_end) = stat.rfind(')') else {
        return Err(ClientError::Discovery(
            "Coven daemon process status omitted its command boundary".to_owned(),
        ));
    };
    let fields = stat[command_end + 1..]
        .split_whitespace()
        .collect::<Vec<_>>();
    if fields.len() <= 19 {
        return Err(ClientError::Discovery(
            "Coven daemon process status omitted its start identity".to_owned(),
        ));
    }
    if matches!(fields[0], "Z" | "X") {
        return Ok(None);
    }
    let start_ticks = fields[19].parse::<u64>().map_err(|_| {
        ClientError::Discovery("Coven daemon process start identity was invalid".to_owned())
    })?;
    if start_ticks == 0 {
        return Err(ClientError::Discovery(
            "Coven daemon process start identity was zero".to_owned(),
        ));
    }
    Ok(Some(UnixProcessIdentity { start_ticks }))
}

#[cfg(target_os = "macos")]
pub(crate) fn unix_process_identity(pid: u32) -> Result<Option<UnixProcessIdentity>, ClientError> {
    use std::mem::MaybeUninit;

    const PROC_PIDTBSDINFO: libc::c_int = 3;
    const SZOMB: u32 = 5;

    #[repr(C)]
    struct ProcBsdInfo {
        pbi_flags: u32,
        pbi_status: u32,
        pbi_xstatus: u32,
        pbi_pid: u32,
        pbi_ppid: u32,
        pbi_uid: u32,
        pbi_gid: u32,
        pbi_ruid: u32,
        pbi_rgid: u32,
        pbi_svuid: u32,
        pbi_svgid: u32,
        rfu_1: u32,
        pbi_comm: [libc::c_char; 16],
        pbi_name: [libc::c_char; 32],
        pbi_nfiles: u32,
        pbi_pgid: u32,
        pbi_pjobc: u32,
        e_tdev: u32,
        e_tpgid: u32,
        pbi_nice: i32,
        pbi_start_tvsec: u64,
        pbi_start_tvusec: u64,
    }

    #[link(name = "proc")]
    unsafe extern "C" {
        fn proc_pidinfo(
            pid: libc::c_int,
            flavor: libc::c_int,
            argument: u64,
            buffer: *mut libc::c_void,
            buffer_size: libc::c_int,
        ) -> libc::c_int;
    }

    let pid_as_int = libc::c_int::try_from(pid).map_err(|_| {
        ClientError::Discovery("connected Coven daemon peer PID exceeded pid_t".to_owned())
    })?;
    let expected_size = std::mem::size_of::<ProcBsdInfo>();
    let buffer_size = libc::c_int::try_from(expected_size).map_err(|_| {
        ClientError::Discovery("macOS process identity buffer size overflowed".to_owned())
    })?;
    let mut info = MaybeUninit::<ProcBsdInfo>::uninit();
    let result = unsafe {
        proc_pidinfo(
            pid_as_int,
            PROC_PIDTBSDINFO,
            0,
            info.as_mut_ptr().cast(),
            buffer_size,
        )
    };
    if result == 0 {
        let source = std::io::Error::last_os_error();
        return if source.raw_os_error() == Some(libc::ESRCH) {
            Ok(None)
        } else {
            Err(ClientError::Io {
                operation: "failed to inspect Coven daemon process identity",
                source,
            })
        };
    }
    if usize::try_from(result).ok() != Some(expected_size) {
        return Err(ClientError::Discovery(
            "macOS returned a truncated Coven daemon process identity".to_owned(),
        ));
    }
    let info = unsafe { info.assume_init() };
    if info.pbi_pid != pid {
        return Err(ClientError::DaemonInstanceChanged);
    }
    if info.pbi_status == SZOMB {
        return Ok(None);
    }
    if info.pbi_start_tvsec == 0 && info.pbi_start_tvusec == 0 {
        return Err(ClientError::Discovery(
            "Coven daemon process start identity was zero".to_owned(),
        ));
    }
    Ok(Some(UnixProcessIdentity {
        start_seconds: info.pbi_start_tvsec,
        start_microseconds: info.pbi_start_tvusec,
    }))
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub(crate) fn unix_process_identity(_pid: u32) -> Result<Option<UnixProcessIdentity>, ClientError> {
    Err(ClientError::UnsupportedPlatform)
}

fn peer_uid_matches(expected_uid: u32, current_uid: u32, peer_uid: u32) -> bool {
    peer_uid == expected_uid && peer_uid == current_uid
}

fn validate_connected_peer(
    stream: &UnixStream,
    expected_uid: u32,
) -> Result<UnixPeerCredentials, ClientError> {
    let peer = connected_peer_credentials(stream).map_err(|source| ClientError::Io {
        operation: "failed to inspect connected Coven daemon peer credentials",
        source,
    })?;
    // SAFETY: geteuid only reads process credentials and cannot fail.
    let current_uid = unsafe { libc::geteuid() };
    if !peer_uid_matches(expected_uid, current_uid, peer.uid) {
        return Err(ClientError::Discovery(format!(
            "connected Coven daemon peer uid {} did not match expected current owner uid {expected_uid}",
            peer.uid
        )));
    }
    Ok(peer)
}

#[cfg(target_os = "linux")]
fn connected_peer_credentials(stream: &UnixStream) -> std::io::Result<UnixPeerCredentials> {
    use std::{mem::MaybeUninit, os::fd::AsRawFd};

    let mut credentials = MaybeUninit::<libc::ucred>::uninit();
    let mut length = libc::socklen_t::try_from(std::mem::size_of::<libc::ucred>())
        .map_err(|_| std::io::Error::other("Linux peer credential size overflow"))?;
    let result = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            credentials.as_mut_ptr().cast(),
            &mut length,
        )
    };
    if result != 0 {
        return Err(std::io::Error::last_os_error());
    }
    if usize::try_from(length).ok() != Some(std::mem::size_of::<libc::ucred>()) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "Linux peer credential response had an unexpected size",
        ));
    }
    let credentials = unsafe { credentials.assume_init() };
    let pid = u32::try_from(credentials.pid)
        .map_err(|_| std::io::Error::other("Linux peer PID was not positive"))?;
    if pid == 0 {
        return Err(std::io::Error::other("Linux peer PID was zero"));
    }
    Ok(UnixPeerCredentials {
        pid,
        uid: credentials.uid,
        gid: credentials.gid,
    })
}

#[cfg(target_os = "macos")]
fn connected_peer_credentials(stream: &UnixStream) -> std::io::Result<UnixPeerCredentials> {
    use std::{mem::MaybeUninit, os::fd::AsRawFd};

    let mut uid = 0;
    let mut gid = 0;
    if unsafe { libc::getpeereid(stream.as_raw_fd(), &mut uid, &mut gid) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    let mut pid = MaybeUninit::<libc::pid_t>::uninit();
    let mut length = libc::socklen_t::try_from(std::mem::size_of::<libc::pid_t>())
        .map_err(|_| std::io::Error::other("macOS peer PID size overflow"))?;
    if unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_LOCAL,
            libc::LOCAL_PEERPID,
            pid.as_mut_ptr().cast(),
            &mut length,
        )
    } != 0
    {
        return Err(std::io::Error::last_os_error());
    }
    if usize::try_from(length).ok() != Some(std::mem::size_of::<libc::pid_t>()) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "macOS peer PID response had an unexpected size",
        ));
    }
    let pid = u32::try_from(unsafe { pid.assume_init() })
        .map_err(|_| std::io::Error::other("macOS peer PID was not positive"))?;
    if pid == 0 {
        return Err(std::io::Error::other("macOS peer PID was zero"));
    }
    Ok(UnixPeerCredentials { pid, uid, gid })
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn connected_peer_credentials(_stream: &UnixStream) -> std::io::Result<UnixPeerCredentials> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "Unix peer credential validation is unavailable on this platform",
    ))
}

fn read_framed_response<R: Read>(
    stream: &mut R,
    deadline: Instant,
    max_response_body_bytes: usize,
) -> Result<FramedResponse, ClientError> {
    let mut received = Vec::with_capacity(1024);
    let mut chunk = [0_u8; 4096];
    let (status, body_start, content_length) = loop {
        if let Some((header_end, body_start)) = find_header_end(&received) {
            if header_end > MAX_RESPONSE_HEADERS_BYTES {
                return Err(ClientError::InvalidHttpResponse(format!(
                    "response headers exceeded {MAX_RESPONSE_HEADERS_BYTES} bytes"
                )));
            }
            let (status, content_length) = parse_headers(&received[..header_end])?;
            if content_length > max_response_body_bytes {
                return Err(ClientError::ResponseTooLarge {
                    max_bytes: max_response_body_bytes,
                });
            }
            break (status, body_start, content_length);
        }
        if received.len() > MAX_RESPONSE_HEADERS_BYTES {
            return Err(ClientError::InvalidHttpResponse(format!(
                "response headers exceeded {MAX_RESPONSE_HEADERS_BYTES} bytes"
            )));
        }
        read_with_deadline(stream, &mut chunk, deadline, &mut received)?;
    };

    while received.len().saturating_sub(body_start) < content_length {
        let remaining = content_length - (received.len() - body_start);
        let read_len = remaining.saturating_add(1).min(chunk.len());
        read_with_deadline(stream, &mut chunk[..read_len], deadline, &mut received)?;
    }

    let mut body_and_remainder = received.split_off(body_start);
    let mut buffered_remainder = body_and_remainder.split_off(content_length);
    if buffered_remainder.is_empty() {
        if let Some(byte) = nonblocking_boundary_probe(stream, deadline)? {
            buffered_remainder.push(byte);
        }
    }
    Ok(FramedResponse {
        response: HttpResponse {
            status,
            body: body_and_remainder,
        },
        buffered_remainder,
    })
}

fn nonblocking_boundary_probe<R: Read>(
    stream: &mut R,
    deadline: Instant,
) -> Result<Option<u8>, ClientError> {
    let mut byte = [0_u8; 1];
    loop {
        match stream.read(&mut byte) {
            Ok(0) => return Ok(None),
            Ok(_) => return Ok(Some(byte[0])),
            Err(source)
                if matches!(
                    source.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                return Ok(None);
            }
            Err(source) if source.kind() == std::io::ErrorKind::Interrupted => {
                read_timeout_until(deadline, Instant::now())?;
            }
            Err(source) => {
                return Err(ClientError::Io {
                    operation: "failed to validate Coven daemon response framing",
                    source,
                })
            }
        }
    }
}

fn read_with_deadline<R: Read>(
    stream: &mut R,
    chunk: &mut [u8],
    deadline: Instant,
    received: &mut Vec<u8>,
) -> Result<(), ClientError> {
    let timeout = read_timeout_until(deadline, Instant::now())?;
    match stream.read(chunk) {
        Ok(0) => Err(ClientError::InvalidHttpResponse(
            "connection closed before response completed".to_owned(),
        )),
        Ok(read) => {
            received.extend_from_slice(&chunk[..read]);
            Ok(())
        }
        Err(source)
            if matches!(
                source.kind(),
                std::io::ErrorKind::TimedOut
                    | std::io::ErrorKind::WouldBlock
                    | std::io::ErrorKind::Interrupted
            ) =>
        {
            if source.kind() != std::io::ErrorKind::Interrupted {
                std::thread::sleep(timeout.min(MIN_READ_TIMEOUT));
            }
            Ok(())
        }
        Err(source) => Err(ClientError::Io {
            operation: "failed to read Coven daemon response",
            source,
        }),
    }
}

fn read_timeout_until(deadline: Instant, now: Instant) -> Result<Duration, ClientError> {
    let remaining = deadline
        .checked_duration_since(now)
        .filter(|duration| !duration.is_zero());
    remaining.map_or_else(
        || {
            Err(ClientError::InvalidHttpResponse(
                "timed out reading Coven daemon response".to_owned(),
            ))
        },
        Ok,
    )
}

fn parse_headers(headers: &[u8]) -> Result<(u16, usize), ClientError> {
    let headers = std::str::from_utf8(headers)
        .map_err(|_| ClientError::InvalidHttpResponse("headers were not UTF-8".to_owned()))?;
    let status = headers
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|status| status.parse().ok())
        .ok_or_else(|| ClientError::InvalidHttpResponse("missing HTTP status".to_owned()))?;
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
    let content_length = content_length.ok_or_else(|| {
        ClientError::InvalidHttpResponse("missing Content-Length header".to_owned())
    })?;
    Ok((status, content_length))
}

fn find_header_end(response: &[u8]) -> Option<(usize, usize)> {
    response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|position| (position, position + 4))
        .or_else(|| {
            response
                .windows(2)
                .position(|window| window == b"\n\n")
                .map(|position| (position, position + 2))
        })
}

#[cfg(test)]
mod tests {
    use super::{
        connect_operation_with_deadline, connected_peer_credentials, peer_uid_matches,
        read_framed_response, read_timeout_until,
    };
    use std::{
        io::{Cursor, Read},
        time::{Duration, Instant},
    };

    #[test]
    fn deadline_timeout_preserves_sub_resolution_durations_and_rejects_expiry() {
        let now = Instant::now();

        assert_eq!(
            read_timeout_until(now + Duration::from_nanos(1), now).expect("future deadline"),
            Duration::from_nanos(1)
        );
        assert!(read_timeout_until(now, now).is_err());
    }

    #[test]
    fn stalled_connect_receives_only_the_absolute_deadline_budget() {
        let observed = std::cell::Cell::new(None);
        let deadline = Instant::now() + Duration::from_millis(25);

        let error = connect_operation_with_deadline(deadline, |remaining| {
            observed.set(Some(remaining));
            Err(std::io::Error::from(std::io::ErrorKind::TimedOut))
        })
        .expect_err("simulated stalled connect must time out");

        assert!(matches!(error, crate::ClientError::Io { .. }));
        let remaining = observed.get().expect("connect timeout was supplied");
        assert!(!remaining.is_zero());
        assert!(remaining <= Duration::from_millis(25));

        let invoked = std::cell::Cell::new(false);
        connect_operation_with_deadline(Instant::now(), |_| {
            invoked.set(true);
            Ok(())
        })
        .expect_err("expired connect deadline must fail before connect");
        assert!(!invoked.get());
    }

    #[test]
    fn connected_peer_uid_must_match_discovered_and_current_owner() {
        assert!(peer_uid_matches(501, 501, 501));
        assert!(!peer_uid_matches(501, 501, 502));
        assert!(!peer_uid_matches(501, 502, 501));
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn platform_peer_credentials_report_the_connected_process_uid() {
        let (client, _server) =
            std::os::unix::net::UnixStream::pair().expect("create connected Unix stream pair");

        let peer = connected_peer_credentials(&client).expect("inspect connected peer credentials");

        assert_eq!(peer.uid, unsafe { libc::geteuid() });
        assert_eq!(peer.pid, std::process::id());
    }

    struct InterruptedOnceReader {
        interrupted: bool,
        response: Cursor<&'static [u8]>,
    }

    impl Read for InterruptedOnceReader {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            if !self.interrupted {
                self.interrupted = true;
                return Err(std::io::Error::from(std::io::ErrorKind::Interrupted));
            }
            self.response.read(buffer)
        }
    }

    struct ChunkedReader {
        chunks: std::collections::VecDeque<&'static [u8]>,
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

    struct FirstReadBoundedSocket {
        stream: std::os::unix::net::UnixStream,
        first_read_limit: Option<usize>,
    }

    impl Read for FirstReadBoundedSocket {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            let limit = self
                .first_read_limit
                .take()
                .unwrap_or(buffer.len())
                .min(buffer.len());
            self.stream.read(&mut buffer[..limit])
        }
    }

    #[test]
    fn response_reader_retries_interrupted_reads() {
        let mut reader = InterruptedOnceReader {
            interrupted: false,
            response: Cursor::new(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}"),
        };

        let framed = read_framed_response(
            &mut reader,
            Instant::now() + Duration::from_secs(1),
            super::MAX_RESPONSE_BODY_BYTES,
        )
        .expect("an interrupted read must be retried under the same deadline");

        assert_eq!(framed.response.status, 200);
        assert_eq!(framed.response.body, b"{}");
        assert!(framed.buffered_remainder.is_empty());
    }

    #[test]
    fn response_reader_accepts_a_header_coalesced_with_a_partial_body() {
        let mut reader = ChunkedReader {
            chunks: std::collections::VecDeque::from([
                &b"HTTP/1.1 200 OK\r\nContent-Length: 7\r\n\r\npar"[..],
                &b"tial"[..],
            ]),
        };

        let framed = read_framed_response(
            &mut reader,
            Instant::now() + Duration::from_secs(1),
            super::MAX_RESPONSE_BODY_BYTES,
        )
        .expect("read a body split across the header read and a later read");

        assert_eq!(framed.response.status, 200);
        assert_eq!(framed.response.body, b"partial");
        assert!(framed.buffered_remainder.is_empty());
    }

    #[test]
    fn response_reader_preserves_all_bytes_after_a_zero_length_body() {
        let mut reader =
            Cursor::new(b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\nXYZ".as_slice());

        let framed = read_framed_response(
            &mut reader,
            Instant::now() + Duration::from_secs(1),
            super::MAX_RESPONSE_BODY_BYTES,
        )
        .expect("read the complete frame and buffered remainder");

        assert_eq!(framed.response.status, 204);
        assert!(framed.response.body.is_empty());
        assert_eq!(framed.buffered_remainder, b"XYZ");
    }

    #[test]
    fn real_socket_reader_preserves_trailing_bytes_queued_with_a_later_body_read() {
        use std::io::Write;

        let (reader, mut writer) =
            std::os::unix::net::UnixStream::pair().expect("create response socket pair");
        reader
            .set_nonblocking(true)
            .expect("make response reader nonblocking");
        let server = std::thread::spawn(move || {
            writer
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\n\r\n")
                .expect("write response headers");
            std::thread::sleep(Duration::from_millis(20));
            writer
                .write_all(b"bodyX")
                .expect("write body and trailing byte together");
        });
        let mut reader = FirstReadBoundedSocket {
            stream: reader,
            first_read_limit: None,
        };

        let framed = read_framed_response(
            &mut reader,
            Instant::now() + Duration::from_secs(1),
            super::MAX_RESPONSE_BODY_BYTES,
        )
        .expect("read framed socket response");

        assert_eq!(framed.response.body, b"body");
        assert_eq!(framed.buffered_remainder, b"X");
        server.join().expect("response server");
    }

    #[test]
    fn real_socket_reader_probes_after_a_header_only_zero_length_frame() {
        use std::io::Write;

        let header = b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n";
        let (reader, mut writer) =
            std::os::unix::net::UnixStream::pair().expect("create response socket pair");
        reader
            .set_nonblocking(true)
            .expect("make response reader nonblocking");
        writer
            .write_all(&[header.as_slice(), b"X"].concat())
            .expect("queue header and trailing byte");
        let mut reader = FirstReadBoundedSocket {
            stream: reader,
            first_read_limit: Some(header.len()),
        };

        let framed = read_framed_response(
            &mut reader,
            Instant::now() + Duration::from_secs(1),
            super::MAX_RESPONSE_BODY_BYTES,
        )
        .expect("read zero-length socket response");

        assert!(framed.response.body.is_empty());
        assert_eq!(framed.buffered_remainder, b"X");
    }
}
