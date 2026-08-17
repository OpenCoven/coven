mod discovery;
mod error;
mod http;
#[cfg(unix)]
mod lifecycle;
mod models;
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
pub use error::{ClientError, DaemonError};
pub use http::DaemonClient;
#[cfg(unix)]
#[doc(hidden)]
pub use lifecycle::{probe_unix_daemon_health, shutdown_unix_daemon};
pub use models::{Health, HealthCapabilities, ReadEndpoint, WriteEndpoint, PROTOCOL_VERSION};
#[cfg(unix)]
#[doc(hidden)]
pub use models::{LifecycleDaemonStatus, UnixDaemonShutdown};
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
