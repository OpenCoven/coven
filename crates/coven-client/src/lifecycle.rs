use std::{
    path::Path,
    time::{Duration, Instant},
};

use serde::Deserialize;

#[cfg(target_os = "linux")]
use crate::transport::unix_process_identity;
use crate::{
    transport::{
        request_retaining_connection, request_with_timeout, request_with_timeout_bound,
        AuthenticatedUnixResponse, PeerIdentity,
    },
    ClientError, DaemonEndpoint, HealthCapabilities, LifecycleDaemonStatus, UnixDaemonShutdown,
    PROTOCOL_VERSION,
};

const HEALTH_PATH: &str = "/health";
const SHUTDOWN_PATH: &str = "/api/v1/internal/lifecycle/shutdown";
const MAX_LIFECYCLE_BODY_BYTES: usize = 64 * 1024;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LifecycleHealth {
    ok: bool,
    api_version: String,
    coven_version: String,
    capabilities: HealthCapabilities,
    daemon: Option<LifecycleDaemonStatus>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ShutdownAcknowledgement {
    ok: bool,
    api_version: String,
    capabilities: HealthCapabilities,
    daemon: LifecycleDaemonStatus,
}

pub fn probe_unix_daemon_health(
    coven_home: &Path,
    timeout: Duration,
) -> Result<Option<LifecycleDaemonStatus>, ClientError> {
    let Some(endpoint) = lifecycle_endpoint(coven_home)? else {
        return Ok(None);
    };
    let (response, peer) = match request_with_timeout(
        &endpoint,
        "GET",
        HEALTH_PATH,
        None,
        timeout,
        MAX_LIFECYCLE_BODY_BYTES,
    ) {
        Ok(response) => response,
        Err(error) if endpoint_unavailable(&error) => return Ok(None),
        Err(error) => return Err(error),
    };
    if response.status != 200 {
        return Err(ClientError::HttpStatus(response.status));
    }
    let health: LifecycleHealth =
        serde_json::from_slice(&response.body).map_err(ClientError::InvalidJson)?;
    validate_lifecycle_contract(health.ok, &health.api_version, &health.capabilities)?;
    match health.daemon {
        Some(mut status) => {
            canonicalize_profile_binding(&endpoint, &mut status, Some(&peer))?;
            Ok(Some(status))
        }
        None => Ok(None),
    }
}

pub fn shutdown_unix_daemon(
    coven_home: &Path,
    expected: &LifecycleDaemonStatus,
    timeout: Duration,
) -> Result<UnixDaemonShutdown, ClientError> {
    let deadline = Instant::now().checked_add(timeout).ok_or_else(|| {
        ClientError::InvalidHttpResponse("lifecycle shutdown deadline overflowed".to_owned())
    })?;
    let Some(endpoint) = lifecycle_endpoint(coven_home)? else {
        return Ok(UnixDaemonShutdown::Unavailable);
    };
    // Snapshot the canonical selected/recorded socket binding before any
    // request bytes go out: this is the only point at which `expected`'s
    // reported socket pathname is resolved against the filesystem. Once the
    // request below is written, the daemon may legitimately unlink that same
    // pathname while it finishes handling this very shutdown, so nothing
    // past this point may repeat that resolution.
    let mut canonical_expected = expected.clone();
    if canonicalize_profile_binding(&endpoint, &mut canonical_expected, None).is_err() {
        return Ok(UnixDaemonShutdown::IdentityMismatch);
    }
let body = serde_json::to_vec(&serde_json::json!({
    "apiVersion": PROTOCOL_VERSION,
    "daemon": &canonical_expected,
}))
.map_err(ClientError::InvalidJson)?
    let pending = match request_retaining_connection(
        &endpoint,
        "POST",
        SHUTDOWN_PATH,
        Some(&body),
        remaining(
            deadline,
            "failed to stop Coven daemon before the lifecycle deadline",
        )?,
        MAX_LIFECYCLE_BODY_BYTES,
    ) {
        Ok(response) => response,
        Err(error) if endpoint_unavailable(&error) => return Ok(UnixDaemonShutdown::Unavailable),
        Err(error) => return Err(error),
    };
    // `request_retaining_connection` already captured the connected peer
    // identity (pid plus the selected socket's device/inode, re-confirmed
    // immediately after connect) before writing the request bytes above, and
    // rejected the connection outright if the binding changed underneath it
    // in that window. Comparing the peer pid here validates the response
    // against that pre-send snapshot with a plain integer comparison instead
    // of re-canonicalizing a socket pathname the daemon may have already
    // unlinked while completing shutdown. A kernel-authenticated pid that
    // does not match `expected` is treated as a hard failure rather than a
    // soft identity mismatch, regardless of the response that follows: it
    // means the numeric pid we hold has already been substituted, which must
    // never be papered over as "the daemon changed" and quietly waved through.
    if pending.peer_identity().peer_pid != canonical_expected.pid {
        return Err(ClientError::Discovery(format!(
            "expected Coven daemon pid {} but the connected peer pid was {}",
            canonical_expected.pid,
            pending.peer_identity().peer_pid
        )));
    }
    let response_status = pending.response()?.status;
    if response_status == 409 {
        return Ok(UnixDaemonShutdown::IdentityMismatch);
    }
    if response_status == 404 {
        return shutdown_base_unix_daemon(&endpoint, &canonical_expected, pending, deadline);
    }
    if response_status != 202 {
        return Err(ClientError::HttpStatus(response_status));
    }
    let acknowledgement: ShutdownAcknowledgement =
        serde_json::from_slice(&pending.response()?.body).map_err(ClientError::InvalidJson)?;
    validate_lifecycle_contract(
        acknowledgement.ok,
        &acknowledgement.api_version,
        &acknowledgement.capabilities,
    )?;
    // The daemon only returns 202 when the request body's `daemon` (an exact
    // copy of `expected`) matched its own recorded identity, and it echoes
    // that same value back verbatim on success. Comparing the acknowledged
    // identity directly against `expected` therefore validates the response
    // body against the pre-send snapshot without touching the filesystem.
    if acknowledgement.daemon != *expected {
        return Ok(UnixDaemonShutdown::IdentityMismatch);
    }
    if pending.wait_for_close()? {
        Ok(UnixDaemonShutdown::Exited)
    } else {
        Ok(UnixDaemonShutdown::TimedOut)
    }
}

fn shutdown_base_unix_daemon(
    endpoint: &DaemonEndpoint,
    expected: &LifecycleDaemonStatus,
    missing_route: AuthenticatedUnixResponse,
    deadline: Instant,
) -> Result<UnixDaemonShutdown, ClientError> {
    let peer = missing_route.peer_identity().clone();
    if peer.peer_pid != expected.pid {
        return Ok(UnixDaemonShutdown::IdentityMismatch);
    }
    drop(missing_route);

    let response = match request_with_timeout_bound(
        endpoint,
        "GET",
        HEALTH_PATH,
        None,
        &peer,
        remaining(
            deadline,
            "failed to authenticate BASE Coven daemon health before the lifecycle deadline",
        )?,
        MAX_LIFECYCLE_BODY_BYTES,
    ) {
        Ok(response) => response,
        Err(error) if endpoint_unavailable(&error) => return Ok(UnixDaemonShutdown::Unavailable),
        Err(error) => return Err(error),
    };
    if response.status != 200 {
        return Err(ClientError::HttpStatus(response.status));
    }
    let health: LifecycleHealth =
        serde_json::from_slice(&response.body).map_err(ClientError::InvalidJson)?;
    validate_lifecycle_contract(health.ok, &health.api_version, &health.capabilities)?;
    validate_base_capabilities(&health.capabilities)?;
    if health.coven_version.is_empty() {
        return Err(ClientError::InvalidHttpResponse(
            "BASE Coven daemon health omitted covenVersion".to_owned(),
        ));
    }
    let Some(mut status) = health.daemon else {
        return Ok(UnixDaemonShutdown::IdentityMismatch);
    };
    canonicalize_profile_binding(endpoint, &mut status, Some(&peer))?;
    if status != *expected {
        return Ok(UnixDaemonShutdown::IdentityMismatch);
    }

    finish_legacy_shutdown(&peer, deadline)
}

#[cfg(target_os = "linux")]
fn finish_legacy_shutdown(
    peer: &PeerIdentity,
    deadline: Instant,
) -> Result<UnixDaemonShutdown, ClientError> {
    match signal_legacy_peer(peer, deadline)? {
        LegacySignal::Exited => Ok(UnixDaemonShutdown::Exited),
        LegacySignal::IdentityMismatch => Ok(UnixDaemonShutdown::IdentityMismatch),
        LegacySignal::TimedOut => Ok(UnixDaemonShutdown::TimedOut),
        LegacySignal::Signaled => {
            if wait_for_legacy_peer_exit(peer, deadline)? {
                Ok(UnixDaemonShutdown::Exited)
            } else {
                Ok(UnixDaemonShutdown::TimedOut)
            }
        }
    }
}

#[cfg(target_os = "macos")]
fn finish_legacy_shutdown(
    _peer: &PeerIdentity,
    deadline: Instant,
) -> Result<UnixDaemonShutdown, ClientError> {
    remaining(
        deadline,
        "failed to stop BASE Coven daemon before the lifecycle deadline",
    )?;
    Err(legacy_shutdown_upgrade_required("macOS"))
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn finish_legacy_shutdown(
    _peer: &PeerIdentity,
    deadline: Instant,
) -> Result<UnixDaemonShutdown, ClientError> {
    remaining(
        deadline,
        "failed to stop BASE Coven daemon before the lifecycle deadline",
    )?;
    Err(legacy_shutdown_upgrade_required(std::env::consts::OS))
}

fn validate_base_capabilities(capabilities: &HealthCapabilities) -> Result<(), ClientError> {
    if !capabilities.sessions {
        return Err(ClientError::CapabilityUnavailable {
            capability: "sessions",
        });
    }
    if !capabilities.events {
        return Err(ClientError::CapabilityUnavailable {
            capability: "events",
        });
    }
    if capabilities.event_cursor.as_deref() != Some("sequence") {
        return Err(ClientError::CapabilityUnavailable {
            capability: "eventCursor",
        });
    }
    Ok(())
}

#[cfg(target_os = "linux")]
#[derive(Debug, PartialEq, Eq)]
enum LegacySignal {
    Exited,
    IdentityMismatch,
    TimedOut,
    Signaled,
}

#[cfg(target_os = "linux")]
fn signal_legacy_peer(peer: &PeerIdentity, deadline: Instant) -> Result<LegacySignal, ClientError> {
    remaining(
        deadline,
        "failed to revalidate BASE Coven daemon before the lifecycle deadline",
    )?;
    let Some(expected_identity) = peer.process_identity else {
        return Err(ClientError::Discovery(
            "authenticated BASE Coven daemon process identity was not retained".to_owned(),
        ));
    };
    let Some(process) = peer.process_handle() else {
        return Err(legacy_shutdown_upgrade_required("Linux"));
    };
    let current_identity = unix_process_identity(peer.peer_pid)?;
    signal_if_identity_still_matches(expected_identity, current_identity, deadline, || {
        if process.send_sigterm()? {
            Ok(LegacySignal::Signaled)
        } else {
            Ok(LegacySignal::Exited)
        }
    })
}

#[cfg(target_os = "linux")]
fn signal_if_identity_still_matches<T: PartialEq>(
    expected_identity: T,
    current_identity: Option<T>,
    deadline: Instant,
    signal: impl FnOnce() -> Result<LegacySignal, ClientError>,
) -> Result<LegacySignal, ClientError> {
    let Some(current_identity) = current_identity else {
        return Ok(LegacySignal::Exited);
    };
    if current_identity != expected_identity {
        return Ok(LegacySignal::IdentityMismatch);
    }
    if Instant::now() >= deadline {
        return Ok(LegacySignal::TimedOut);
    }
    signal()
}

fn legacy_shutdown_upgrade_required(platform: &'static str) -> ClientError {
    ClientError::LegacyShutdownUpgradeRequired { platform }
}

#[cfg(target_os = "linux")]
fn wait_for_legacy_peer_exit(peer: &PeerIdentity, deadline: Instant) -> Result<bool, ClientError> {
    let Some(expected_identity) = peer.process_identity else {
        return Err(ClientError::Discovery(
            "authenticated BASE Coven daemon process identity was not retained".to_owned(),
        ));
    };
    loop {
        match unix_process_identity(peer.peer_pid)? {
            None => return Ok(true),
            Some(identity) if identity != expected_identity => return Ok(true),
            Some(_) => {}
        }
        let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
            return Ok(false);
        };
        if remaining.is_zero() {
            return Ok(false);
        }
        std::thread::sleep(remaining.min(Duration::from_millis(10)));
    }
}

fn remaining(deadline: Instant, message: &'static str) -> Result<Duration, ClientError> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|remaining| !remaining.is_zero())
        .ok_or_else(|| ClientError::Io {
            operation: message,
            source: std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "Coven daemon lifecycle deadline expired",
            ),
        })
}

fn lifecycle_endpoint(coven_home: &Path) -> Result<Option<DaemonEndpoint>, ClientError> {
    lifecycle_endpoint_with_discovery(coven_home, |home| DaemonEndpoint::discover(home))
}

fn lifecycle_endpoint_with_discovery(
    coven_home: &Path,
    discover: impl FnOnce(&Path) -> Result<DaemonEndpoint, ClientError>,
) -> Result<Option<DaemonEndpoint>, ClientError> {
    crate::validate_unix_daemon_path_encoding(coven_home)?;
    match std::fs::symlink_metadata(coven_home) {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(ClientError::Discovery(format!(
                "cannot inspect COVEN_HOME {:?}: {error}",
                coven_home.as_os_str()
            )))
        }
    }
    let coven_home = crate::canonical_unix_daemon_home(coven_home)?;
    let home_identity = lifecycle_home_snapshot(&coven_home)?;
    let selected_socket = coven_home.join("coven.sock");
    match std::fs::symlink_metadata(&selected_socket) {
        Ok(_) => match discover(&coven_home) {
            Ok(endpoint) => Ok(Some(endpoint)),
            Err(ClientError::Io { operation, source })
                if operation == crate::discovery::UNIX_SOCKET_DISAPPEARED_OPERATION
                    && source.kind() == std::io::ErrorKind::NotFound =>
            {
                ensure_lifecycle_home_identity(&coven_home, home_identity)?;
                Ok(None)
            }
            Err(error) => Err(error),
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(ClientError::Discovery(format!(
            "cannot inspect {}: {error}",
            selected_socket.display()
        ))),
    }
}

fn lifecycle_home_snapshot(coven_home: &Path) -> Result<(u64, u64, u32, u32), ClientError> {
    use std::os::unix::fs::MetadataExt;

    let metadata = std::fs::symlink_metadata(coven_home).map_err(|error| {
        ClientError::Discovery(format!(
            "cannot inspect COVEN_HOME {} before lifecycle discovery: {error}",
            coven_home.display()
        ))
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(ClientError::Discovery(format!(
            "COVEN_HOME {} is not a stable lifecycle directory",
            coven_home.display()
        )));
    }
    Ok((
        metadata.dev(),
        metadata.ino(),
        metadata.uid(),
        metadata.mode(),
    ))
}

fn ensure_lifecycle_home_identity(
    coven_home: &Path,
    expected: (u64, u64, u32, u32),
) -> Result<(), ClientError> {
    let actual = lifecycle_home_snapshot(coven_home).map_err(|error| {
        ClientError::Discovery(format!(
            "cannot confirm the selected COVEN_HOME while classifying a missing daemon socket: \
             {error}"
        ))
    })?;
    if actual.0 != expected.0 || actual.1 != expected.1 {
        return Err(ClientError::Discovery(format!(
            "COVEN_HOME {} changed while classifying a missing daemon socket",
            coven_home.display()
        )));
    }
    // SAFETY: geteuid reads process state and cannot fail.
    if actual.2 != unsafe { libc::geteuid() } {
        return Err(ClientError::Discovery(format!(
            "COVEN_HOME {} is not owned by the current user",
            coven_home.display()
        )));
    }
    if actual.3 & 0o077 != 0 {
        return Err(ClientError::Discovery(format!(
            "COVEN_HOME {} is accessible by users other than its owner",
            coven_home.display()
        )));
    }
    Ok(())
}

fn validate_lifecycle_contract(
    ok: bool,
    api_version: &str,
    capabilities: &HealthCapabilities,
) -> Result<(), ClientError> {
    if api_version != PROTOCOL_VERSION {
        return Err(ClientError::ProtocolVersion {
            expected: PROTOCOL_VERSION,
            actual: api_version.to_owned(),
        });
    }
    if !capabilities.structured_errors {
        return Err(ClientError::StructuredErrorsUnavailable);
    }
    if !ok {
        return Err(ClientError::HealthNotReady);
    }
    Ok(())
}

fn canonicalize_profile_binding(
    endpoint: &DaemonEndpoint,
    status: &mut LifecycleDaemonStatus,
    peer: Option<&PeerIdentity>,
) -> Result<(), ClientError> {
    if let Some(peer) = peer {
        if status.pid != peer.peer_pid {
            return Err(ClientError::Discovery(format!(
                "daemon health reported pid {} but the connected peer pid was {}",
                status.pid, peer.peer_pid
            )));
        }
    }
    let reported = Path::new(&status.socket);
    let socket_identity = peer.map(|peer| (peer.socket_device, peer.socket_inode));
    let matches_selected = if reported.is_absolute() {
        same_filesystem_object(endpoint.socket(), reported, socket_identity)
    } else {
        same_filesystem_object(endpoint.socket(), reported, socket_identity)
            || endpoint
                .socket()
                .parent()
                .into_iter()
                .flat_map(Path::ancestors)
                .any(|base| {
                    same_filesystem_object(endpoint.socket(), &base.join(reported), socket_identity)
                })
    };
    if !matches_selected {
        return Err(ClientError::Discovery(
            "daemon health reported a socket for a different Coven home".to_owned(),
        ));
    }
    status.socket = endpoint
        .socket()
        .to_str()
        .ok_or_else(|| {
            ClientError::Discovery(
                "canonical Coven daemon socket is not valid UTF-8; daemon status JSON requires \
                 UTF-8 paths"
                    .to_owned(),
            )
        })?
        .to_owned();
    Ok(())
}

fn same_filesystem_object(
    selected: &Path,
    candidate: &Path,
    expected_identity: Option<(u64, u64)>,
) -> bool {
    use std::os::unix::fs::MetadataExt;

    let (Ok(selected_path), Ok(candidate_path)) = (
        std::fs::canonicalize(selected),
        std::fs::canonicalize(candidate),
    ) else {
        return false;
    };
    if selected_path != candidate_path {
        return false;
    }
    let (Ok(selected), Ok(candidate)) = (
        std::fs::metadata(selected_path),
        std::fs::metadata(candidate_path),
    ) else {
        return false;
    };
    selected.dev() == candidate.dev()
        && selected.ino() == candidate.ino()
        && expected_identity
            .is_none_or(|(device, inode)| selected.dev() == device && selected.ino() == inode)
}

fn endpoint_unavailable(error: &ClientError) -> bool {
    matches!(
        error,
        ClientError::Io { source, .. }
            if matches!(
                source.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
            )
    )
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    struct LifecycleTestHome(std::path::PathBuf);

    #[cfg(unix)]
    impl LifecycleTestHome {
        fn new() -> Self {
            use std::os::unix::fs::PermissionsExt;

            static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
            let workspace = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .ancestors()
                .nth(2)
                .expect("workspace root");
            let path = workspace.join("c").join(format!(
                "r{:x}{:x}",
                std::process::id(),
                NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&path).expect("create lifecycle test home");
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))
                .expect("make lifecycle test home private");
            Self(path)
        }
    }

    #[cfg(unix)]
    impl Drop for LifecycleTestHome {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[cfg(unix)]
    #[test]
    fn rediscovery_classifies_an_exact_selected_socket_unlink_as_unavailable() {
        use std::os::unix::{fs::PermissionsExt, net::UnixListener};

        let home = LifecycleTestHome::new();
        let socket = home.0.join("coven.sock");
        let listener = UnixListener::bind(&socket).expect("bind selected lifecycle socket");
        std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600))
            .expect("make selected socket private");
        std::fs::symlink_metadata(&socket).expect("lifecycle pre-check sees selected socket");
        std::fs::remove_file(&socket).expect("unlink selected socket before rediscovery");

        let error = crate::DaemonEndpoint::discover(&home.0)
            .expect_err("rediscovery must preserve the selected socket disappearance");

        assert!(matches!(
            error,
            crate::ClientError::Io {
                operation: "selected Coven daemon socket disappeared during discovery",
                source,
            } if source.kind() == std::io::ErrorKind::NotFound
        ));
        drop(listener);
    }

    #[cfg(unix)]
    #[test]
    fn lifecycle_discovery_returns_none_when_the_prechecked_socket_is_unlinked() {
        use std::os::unix::{fs::PermissionsExt, net::UnixListener};

        let home = LifecycleTestHome::new();
        let socket = home.0.join("coven.sock");
        let listener = UnixListener::bind(&socket).expect("bind selected lifecycle socket");
        std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600))
            .expect("make selected socket private");

        let endpoint = super::lifecycle_endpoint_with_discovery(&home.0, |canonical_home| {
            let selected = canonical_home.join("coven.sock");
            std::fs::remove_file(&selected)
                .expect("unlink selected socket after lifecycle pre-check");
            crate::DaemonEndpoint::discover(canonical_home)
        })
        .expect("an exact selected-socket unlink is lifecycle unavailability");

        assert!(endpoint.is_none());
        drop(listener);
    }

    #[cfg(unix)]
    #[test]
    fn lifecycle_discovery_preserves_socket_permission_errors() {
        use std::os::unix::{fs::PermissionsExt, net::UnixListener};

        let home = LifecycleTestHome::new();
        let socket = home.0.join("coven.sock");
        let listener = UnixListener::bind(&socket).expect("bind selected lifecycle socket");
        std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o666))
            .expect("make selected socket intentionally non-private");

        let error = super::lifecycle_endpoint(&home.0)
            .expect_err("a non-owner-only selected socket must fail closed");
        assert!(matches!(error, crate::ClientError::Discovery(_)));
        drop(listener);
    }

    #[cfg(unix)]
    #[test]
    fn lifecycle_discovery_does_not_hide_a_replaced_profile_home() {
        use std::os::unix::{fs::PermissionsExt, net::UnixListener};

        let home = LifecycleTestHome::new();
        let socket = home.0.join("coven.sock");
        let moved_home = home.0.with_extension("selected");
        let listener = UnixListener::bind(&socket).expect("bind selected lifecycle socket");
        std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600))
            .expect("make selected socket private");

        let error = super::lifecycle_endpoint_with_discovery(&home.0, |canonical_home| {
            std::fs::remove_file(canonical_home.join("coven.sock"))
                .expect("unlink selected socket after lifecycle pre-check");
            std::fs::rename(canonical_home, &moved_home)
                .expect("move selected profile out of the way");
            std::fs::create_dir(canonical_home).expect("create substituted profile home");
            std::fs::set_permissions(canonical_home, std::fs::Permissions::from_mode(0o700))
                .expect("make substituted profile private");
            crate::DaemonEndpoint::discover(canonical_home)
        })
        .expect_err("profile replacement must not be classified as a stopped daemon");

        assert!(matches!(error, crate::ClientError::Discovery(_)));
        drop(listener);
        std::fs::remove_dir_all(moved_home).expect("clean moved lifecycle test home");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_identity_substitution_never_reaches_the_bound_signal() {
        let signal_called = std::cell::Cell::new(false);

        let result = super::signal_if_identity_still_matches(
            11_u64,
            Some(12_u64),
            std::time::Instant::now() + std::time::Duration::from_secs(1),
            || {
                signal_called.set(true);
                Ok(super::LegacySignal::Signaled)
            },
        )
        .expect("identity substitution is a non-error mismatch");

        assert_eq!(result, super::LegacySignal::IdentityMismatch);
        assert!(
            !signal_called.get(),
            "identity substitution reached a signaling operation"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_legacy_shutdown_fails_closed_with_upgrade_guidance() {
        let peer = crate::transport::PeerIdentity {
            socket_device: 0,
            socket_inode: 0,
            peer_pid: u32::MAX,
            peer_uid: 0,
            peer_gid: 0,
            process_identity: None,
        };

        let error = super::finish_legacy_shutdown(
            &peer,
            std::time::Instant::now() + std::time::Duration::from_secs(1),
        )
        .expect_err("macOS must not fall back to signaling a numeric PID");

        assert!(matches!(
            &error,
            crate::ClientError::LegacyShutdownUpgradeRequired { platform }
                if *platform == "macOS"
        ));
        let message = error.to_string();
        assert!(message.contains("upgrade Coven"));
        assert!(message.contains("restart the daemon manually"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_tmp_spellings_have_the_same_canonical_filesystem_identity() {
        assert!(super::same_filesystem_object(
            std::path::Path::new("/private/tmp"),
            std::path::Path::new("/tmp"),
            None,
        ));
    }
}
