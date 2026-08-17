#[cfg(any(unix, test))]
use std::time::{Duration, Instant};

use crate::{ClientError, DaemonEndpoint};

pub(crate) const MAX_RESPONSE_BODY_BYTES: usize = 4 * 1024 * 1024;
/// Matches the Coven daemon's own Unix-socket/named-pipe request body cap
/// (`MAX_SOCKET_BODY_BYTES` in `coven-cli`). Rejecting oversized bodies here,
/// before any I/O, avoids ever sending a request the daemon is guaranteed to
/// answer with a structured 413 -- or, worse, reset the connection on before
/// that 413 can be read back, turning a structured error into a bare I/O
/// error.
pub(crate) const MAX_REQUEST_BODY_BYTES: usize = 4 * 1024 * 1024;

#[cfg(any(unix, test))]
const MIN_WRITE_RETRY_DELAY: Duration = Duration::from_millis(1);

pub(crate) struct HttpResponse {
    pub(crate) status: u16,
    pub(crate) body: Vec<u8>,
}

#[cfg(unix)]
#[derive(Clone, Debug)]
pub(crate) struct PeerIdentity {
    pub(crate) socket_device: u64,
    pub(crate) socket_inode: u64,
    pub(crate) peer_pid: u32,
    pub(crate) peer_uid: u32,
    pub(crate) peer_gid: u32,
    pub(crate) process_identity: Option<unix::UnixProcessIdentity>,
    #[cfg(target_os = "linux")]
    process_handle: Option<std::sync::Arc<unix::LinuxProcessHandle>>,
}

#[cfg(unix)]
impl PartialEq for PeerIdentity {
    fn eq(&self, other: &Self) -> bool {
        // Separate pidfds for the same process have different descriptor
        // numbers. The PID and birth marker establish equality; the original
        // handle is retained only to make a later legacy signal identity-bound.
        self.socket_device == other.socket_device
            && self.socket_inode == other.socket_inode
            && self.peer_pid == other.peer_pid
            && self.peer_uid == other.peer_uid
            && self.peer_gid == other.peer_gid
            && self.process_identity == other.process_identity
    }
}

#[cfg(unix)]
impl Eq for PeerIdentity {}

#[cfg(target_os = "linux")]
impl PeerIdentity {
    pub(crate) fn process_handle(&self) -> Option<&unix::LinuxProcessHandle> {
        self.process_handle.as_deref()
    }
}

#[cfg(windows)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PeerIdentity {
    pub(crate) server_pid: u32,
    pub(crate) process_creation_time: u64,
}

#[cfg(not(any(unix, windows)))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PeerIdentity;

pub(crate) struct TransportResponse {
    pub(crate) response: HttpResponse,
    pub(crate) peer_identity: PeerIdentity,
}

/// Writes `buf` to a stream already in nonblocking mode, bounded by
/// `deadline`. `WouldBlock`/`TimedOut` are treated as "try again" rather than
/// an error, and interrupted calls are resumed, so a peer that never drains
/// its receive buffer cannot block the caller past `deadline`.
#[cfg(any(unix, test))]
pub(crate) fn write_with_deadline<S: std::io::Write>(
    stream: &mut S,
    mut buf: &[u8],
    deadline: Instant,
    operation: &'static str,
) -> Result<(), ClientError> {
    while !buf.is_empty() {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .filter(|remaining| !remaining.is_zero());
        let Some(remaining) = remaining else {
            return Err(ClientError::Io {
                operation,
                source: std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "timed out writing Coven daemon request",
                ),
            });
        };
        match stream.write(buf) {
            Ok(0) => {
                return Err(ClientError::Io {
                    operation,
                    source: std::io::Error::new(
                        std::io::ErrorKind::WriteZero,
                        "failed to write whole buffer",
                    ),
                })
            }
            Ok(written) => buf = &buf[written..],
            Err(source)
                if matches!(
                    source.kind(),
                    std::io::ErrorKind::WouldBlock
                        | std::io::ErrorKind::TimedOut
                        | std::io::ErrorKind::Interrupted
                ) =>
            {
                if source.kind() != std::io::ErrorKind::Interrupted {
                    std::thread::sleep(remaining.min(MIN_WRITE_RETRY_DELAY));
                }
            }
            Err(source) => return Err(ClientError::Io { operation, source }),
        }
    }
    Ok(())
}

pub(crate) fn request(
    endpoint: &DaemonEndpoint,
    method: &'static str,
    path: &str,
    body: Option<&[u8]>,
) -> Result<TransportResponse, ClientError> {
    request_with_peer(endpoint, method, path, body, None)
}

pub(crate) fn request_bound(
    endpoint: &DaemonEndpoint,
    method: &'static str,
    path: &str,
    body: Option<&[u8]>,
    expected_peer: &PeerIdentity,
) -> Result<TransportResponse, ClientError> {
    request_with_peer(endpoint, method, path, body, Some(expected_peer))
}

pub(crate) fn verify_peer(
    endpoint: &DaemonEndpoint,
    expected_peer: &PeerIdentity,
) -> Result<(), ClientError> {
    #[cfg(unix)]
    {
        unix::verify_peer(endpoint, expected_peer)
    }

    #[cfg(windows)]
    {
        windows::verify_peer(endpoint, expected_peer)
    }

    #[cfg(not(any(unix, windows)))]
    {
        let _ = (endpoint, expected_peer);
        Err(ClientError::UnsupportedPlatform)
    }
}

fn request_with_peer(
    endpoint: &DaemonEndpoint,
    method: &'static str,
    path: &str,
    body: Option<&[u8]>,
    expected_peer: Option<&PeerIdentity>,
) -> Result<TransportResponse, ClientError> {
    if !path.starts_with("/api/v1/") {
        return Err(ClientError::InvalidHttpResponse(
            "attempted request outside /api/v1".to_owned(),
        ));
    }
    if let Some(body) = body {
        if body.len() > MAX_REQUEST_BODY_BYTES {
            return Err(ClientError::RequestTooLarge {
                max_bytes: MAX_REQUEST_BODY_BYTES,
                actual_bytes: body.len(),
            });
        }
    }

    #[cfg(unix)]
    {
        unix::request(endpoint, method, path, body, expected_peer)
    }

    #[cfg(windows)]
    {
        windows::request(endpoint, method, path, body, expected_peer)
    }

    #[cfg(not(any(unix, windows)))]
    {
        let _ = (endpoint, method, path, body, expected_peer);
        Err(ClientError::UnsupportedPlatform)
    }
}

#[cfg(unix)]
mod unix;
#[cfg(target_os = "linux")]
pub(crate) use unix::unix_process_identity;
#[cfg(unix)]
pub(crate) use unix::{
    request_retaining_connection, request_with_timeout, request_with_timeout_bound,
    AuthenticatedUnixResponse,
};
#[cfg(any(windows, test))]
mod windows;
#[cfg(windows)]
pub use windows::{
    open_windows_daemon_process_for_stop, open_windows_daemon_process_for_stop_until,
    open_windows_daemon_process_for_stop_with_creation_time, probe_windows_daemon_health,
    probe_windows_daemon_health_with_identity, probe_windows_daemon_health_with_identity_until,
    windows_process_creation_time, WindowsDaemonHealthProbe, WindowsDaemonProcess,
};

#[cfg(test)]
mod tests {
    use crate::ClientError;
    use std::{io::Write, time::Instant};

    #[derive(Default)]
    struct WriteRecorder(Vec<u8>);

    impl Write for WriteRecorder {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn write_with_deadline_rejects_an_already_expired_deadline_without_writing() {
        let mut stream = WriteRecorder::default();
        let expired = Instant::now() - std::time::Duration::from_millis(1);

        let error = super::write_with_deadline(&mut stream, b"payload", expired, "write")
            .expect_err("an expired deadline must not attempt a write");

        assert!(matches!(error, ClientError::Io { .. }));
        assert!(stream.0.is_empty());
    }

    struct AlwaysWouldBlock;

    impl Write for AlwaysWouldBlock {
        fn write(&mut self, _bytes: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::from(std::io::ErrorKind::WouldBlock))
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn write_with_deadline_times_out_a_stalled_nonblocking_writer() {
        let mut stream = AlwaysWouldBlock;
        let deadline = Instant::now() + std::time::Duration::from_millis(20);
        let started = Instant::now();

        let error = super::write_with_deadline(&mut stream, b"payload", deadline, "write")
            .expect_err("a permanently blocked write must be bounded by the deadline");

        assert!(matches!(error, ClientError::Io { .. }));
        assert!(started.elapsed() < std::time::Duration::from_secs(2));
    }

    #[derive(Default)]
    struct InterruptedOnceWriter {
        interrupted: bool,
        bytes: Vec<u8>,
    }

    impl Write for InterruptedOnceWriter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if !self.interrupted {
                self.interrupted = true;
                return Err(std::io::Error::from(std::io::ErrorKind::Interrupted));
            }
            self.bytes.extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn write_with_deadline_retries_interrupted_writes() {
        let mut stream = InterruptedOnceWriter::default();
        let deadline = Instant::now() + std::time::Duration::from_secs(1);

        super::write_with_deadline(&mut stream, b"payload", deadline, "write")
            .expect("an interrupted write must be retried under the same deadline");

        assert_eq!(stream.bytes, b"payload");
    }
}
