//! Owner-adjacent Rust client for the OpenCoven Coven daemon.
//!
//! `opencoven-coven-client` speaks the named `coven.daemon.v1` contract
//! ([`PROTOCOL_VERSION`]) over same-user local IPC: a Unix domain socket under
//! the daemon home on Unix-like systems and an owner-only named pipe on
//! Windows. It discovers the running daemon ([`DaemonEndpoint`]), issues
//! bounded HTTP requests over that transport ([`DaemonClient`]), decodes the
//! health envelope ([`Health`]), and reports failures as typed errors
//! ([`ClientError`], [`DaemonError`]).
//!
//! The crate is pre-1.0 and owner-adjacent: it lives in the `OpenCoven/coven`
//! repository next to the daemon it talks to, `coven-cli` composes over it, and
//! it carries no CLI, TUI, or agent-runtime dependency. Items marked
//! `#[doc(hidden)]` are lifecycle hooks the daemon's own CLI uses; they are not
//! part of the supported public surface and may change in any release.
//!
//! See the crate README for the compatibility policy and a usage example.
mod discovery;
mod error;
mod http;
#[cfg(unix)]
mod lifecycle;
mod models;
#[cfg(windows)]
mod status;
#[cfg(any(windows, test))]
mod status_error;
mod transport;

pub use discovery::DaemonEndpoint;
#[cfg(unix)]
#[doc(hidden)]
pub use discovery::{canonical_unix_daemon_home, validate_unix_daemon_path_encoding};
#[cfg(windows)]
pub use discovery::{
    owner_only_windows_pipe_name, read_validated_windows_daemon_status,
    read_windows_daemon_status_for_lifecycle, read_windows_daemon_status_for_lifecycle_until,
    supported_windows_pipe_names, validate_windows_daemon_pipe_name,
};
pub use error::{
    is_response_deadline_timeout, ClientError, DaemonError, EMPTY_RESPONSE_TIMEOUT_MESSAGE,
    RESPONSE_READ_TIMEOUT_PREFIX, WINDOWS_CONNECT_OPERATION,
};
pub use http::{DaemonClient, DaemonHttpResponse};
#[cfg(unix)]
#[doc(hidden)]
pub use lifecycle::{probe_unix_daemon_health, shutdown_unix_daemon};
pub use models::{Health, HealthCapabilities, ReadEndpoint, WriteEndpoint, PROTOCOL_VERSION};
#[cfg(unix)]
#[doc(hidden)]
pub use models::{LifecycleDaemonStatus, UnixDaemonShutdown};
#[cfg(windows)]
#[doc(hidden)]
pub use status::{
    write_owner_only_windows_daemon_status, write_owner_only_windows_daemon_status_with_staging,
};
#[doc(hidden)]
pub const MAX_DAEMON_STATUS_BYTES: usize = discovery::MAX_DAEMON_STATUS_BYTES;
#[doc(hidden)]
pub const MAX_RESPONSE_BODY_BYTES: usize = transport::MAX_RESPONSE_BODY_BYTES;
#[cfg(windows)]
pub use transport::{
    open_windows_daemon_process_for_stop, open_windows_daemon_process_for_stop_until,
    open_windows_daemon_process_for_stop_with_creation_time, probe_windows_daemon_health,
    probe_windows_daemon_health_with_identity, probe_windows_daemon_health_with_identity_until,
    windows_process_creation_time, WindowsDaemonHealthProbe, WindowsDaemonProcess,
};
