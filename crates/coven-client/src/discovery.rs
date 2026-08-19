use std::path::Path;

#[cfg(any(windows, test))]
use sha2::{Digest, Sha256};

use crate::ClientError;

pub(crate) const MAX_DAEMON_STATUS_BYTES: usize = 16 * 1024;
#[cfg(unix)]
pub(crate) const UNIX_SOCKET_DISAPPEARED_OPERATION: &str =
    "selected Coven daemon socket disappeared during discovery";
#[cfg(windows)]
const WINDOWS_DISCOVERY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

#[cfg(any(windows, test))]
fn windows_status_remaining_at(
    deadline: std::time::Instant,
    now: std::time::Instant,
    operation: &'static str,
) -> Result<std::time::Duration, ClientError> {
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

pub struct DaemonEndpoint {
    owner_local: bool,
    #[cfg(unix)]
    socket: std::path::PathBuf,
    #[cfg(unix)]
    owner_uid: u32,
    #[cfg(windows)]
    pipe_name: String,
}

impl std::fmt::Debug for DaemonEndpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("DaemonEndpoint::OwnerLocal")
    }
}

impl DaemonEndpoint {
    pub fn discover(coven_home: impl AsRef<Path>) -> Result<Self, ClientError> {
        let coven_home = coven_home.as_ref();
        #[cfg(unix)]
        validate_unix_daemon_path_encoding(coven_home)?;
        ensure_private_home(coven_home)?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::{FileTypeExt, MetadataExt};

            let coven_home = canonical_unix_daemon_home(coven_home)?;
            let socket_candidate = coven_home.join("coven.sock");
            let candidate_metadata =
                std::fs::symlink_metadata(&socket_candidate).map_err(|source| {
                    selected_unix_socket_discovery_error(&socket_candidate, source, "inspect")
                })?;
            if candidate_metadata.file_type().is_symlink() {
                return Err(ClientError::Discovery(format!(
                    "{} is not an owner-local Unix socket",
                    socket_candidate.display()
                )));
            }
            // Resolve symlinked ancestors and `..` before the definitive
            // metadata checks, then retain this exact validated path.
            let socket = std::fs::canonicalize(&socket_candidate).map_err(|source| {
                selected_unix_socket_discovery_error(&socket_candidate, source, "resolve")
            })?;
            let metadata = std::fs::symlink_metadata(&socket).map_err(|source| {
                selected_unix_socket_discovery_error(&socket_candidate, source, "inspect")
            })?;
            if metadata.file_type().is_symlink() || !metadata.file_type().is_socket() {
                return Err(ClientError::Discovery(format!(
                    "{} is not an owner-local Unix socket",
                    socket.display()
                )));
            }
            // SAFETY: geteuid reads process state and cannot fail.
            if metadata.uid() != unsafe { libc::geteuid() } {
                return Err(ClientError::Discovery(format!(
                    "{} is not owned by the current user",
                    socket.display()
                )));
            }
            if metadata.mode() & 0o077 != 0 {
                return Err(ClientError::Discovery(format!(
                    "{} is accessible by users other than its owner",
                    socket.display()
                )));
            }
            Ok(Self {
                owner_local: true,
                socket,
                owner_uid: metadata.uid(),
            })
        }

        #[cfg(windows)]
        {
            let deadline = std::time::Instant::now()
                .checked_add(WINDOWS_DISCOVERY_TIMEOUT)
                .ok_or_else(|| {
                    ClientError::Discovery("daemon discovery deadline overflowed".to_owned())
                })?;
            let stable_pipe_name = owner_only_windows_pipe_name(coven_home)?;
            let pipe_name =
                match validate_windows_daemon_pipe_name_until(&stable_pipe_name, deadline) {
                    Ok(()) => stable_pipe_name,
                    Err(stable_error) => {
                        match recorded_windows_pipe_candidate_until(coven_home, deadline)? {
                            Some(recorded_pipe_name) if recorded_pipe_name != stable_pipe_name => {
                                validate_windows_daemon_pipe_name_until(
                                    &recorded_pipe_name,
                                    deadline,
                                )?;
                                recorded_pipe_name
                            }
                            _ => return Err(stable_error),
                        }
                    }
                };
            Ok(Self {
                owner_local: true,
                pipe_name,
            })
        }

        #[cfg(not(any(unix, windows)))]
        {
            let _ = coven_home;
            Err(ClientError::UnsupportedPlatform)
        }
    }

    pub fn is_owner_local(&self) -> bool {
        self.owner_local
    }

    #[cfg(unix)]
    pub(crate) fn socket(&self) -> &Path {
        &self.socket
    }

    #[cfg(unix)]
    pub(crate) fn owner_uid(&self) -> u32 {
        self.owner_uid
    }

    #[cfg(windows)]
    pub(crate) fn pipe_name(&self) -> &str {
        &self.pipe_name
    }
}

#[cfg(unix)]
fn selected_unix_socket_discovery_error(
    socket: &Path,
    source: std::io::Error,
    action: &str,
) -> ClientError {
    if source.kind() == std::io::ErrorKind::NotFound {
        ClientError::Io {
            operation: UNIX_SOCKET_DISAPPEARED_OPERATION,
            source,
        }
    } else {
        ClientError::Discovery(format!(
            "cannot {action} selected Coven daemon socket {}: {source}",
            socket.display()
        ))
    }
}

#[cfg(unix)]
#[doc(hidden)]
pub fn validate_unix_daemon_path_encoding(coven_home: &Path) -> Result<(), ClientError> {
    let (home, label) = match std::fs::canonicalize(coven_home) {
        Ok(canonical) => (canonical, "canonical COVEN_HOME"),
        Err(source)
            if matches!(
                source.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::InvalidInput
            ) =>
        {
            (coven_home.to_path_buf(), "COVEN_HOME")
        }
        Err(source) => {
            return Err(ClientError::Io {
                operation: "failed to resolve COVEN_HOME before daemon lifecycle validation",
                source,
            })
        }
    };
    validate_utf8_daemon_path(&home, label)?;
    validate_utf8_daemon_path(&home.join("coven.sock"), "Coven daemon socket")
}

#[cfg(unix)]
#[doc(hidden)]
pub fn canonical_unix_daemon_home(coven_home: &Path) -> Result<std::path::PathBuf, ClientError> {
    validate_unix_daemon_path_encoding(coven_home)?;
    let canonical_home = std::fs::canonicalize(coven_home).map_err(|source| ClientError::Io {
        operation: "failed to resolve canonical COVEN_HOME",
        source,
    })?;
    validate_utf8_daemon_path(&canonical_home, "canonical COVEN_HOME")?;
    validate_utf8_daemon_path(
        &canonical_home.join("coven.sock"),
        "canonical Coven daemon socket",
    )?;
    Ok(canonical_home)
}

#[cfg(unix)]
fn validate_utf8_daemon_path(path: &Path, label: &str) -> Result<(), ClientError> {
    if path.to_str().is_none() {
        return Err(ClientError::Discovery(format!(
            "{label} {:?} is not valid UTF-8; Coven daemon status JSON requires UTF-8 home and \
             socket paths",
            path.as_os_str()
        )));
    }
    Ok(())
}

fn ensure_private_home(coven_home: &Path) -> Result<(), ClientError> {
    let metadata = std::fs::symlink_metadata(coven_home).map_err(|error| {
        ClientError::Discovery(format!("cannot inspect {}: {error}", coven_home.display()))
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(ClientError::Discovery(format!(
            "{} is not a Coven home directory",
            coven_home.display()
        )));
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;

        // SAFETY: geteuid reads process state and cannot fail.
        if metadata.uid() != unsafe { libc::geteuid() } {
            return Err(ClientError::Discovery(format!(
                "{} is not owned by the current user",
                coven_home.display()
            )));
        }
        if metadata.mode() & 0o077 != 0 {
            return Err(ClientError::Discovery(format!(
                "{} is accessible by users other than its owner",
                coven_home.display()
            )));
        }
    }

    Ok(())
}

/// Shared daemon/client naming for the owner-only Windows pipe.
///
/// The v2 identity comes from the canonical directory's volume serial and file
/// ID, so aliases converge without folding case-sensitive path components.
/// This returns the daemon's reserved name, not a user-supplied endpoint.
#[doc(hidden)]
#[cfg(any(windows, test))]
pub fn owner_only_windows_pipe_name(coven_home: &Path) -> Result<String, ClientError> {
    Ok(owner_only_windows_pipe_name_from_identity(
        windows_directory_identity(coven_home)?,
    ))
}

#[doc(hidden)]
#[cfg(any(windows, test))]
pub fn supported_windows_pipe_names(coven_home: &Path) -> Result<[String; 3], ClientError> {
    Ok([
        owner_only_windows_pipe_name(coven_home)?,
        lowercase_path_windows_pipe_name(coven_home),
        legacy_windows_pipe_name(coven_home),
    ])
}

#[cfg(any(windows, test))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct WindowsDirectoryIdentity {
    volume_serial_number: u64,
    file_id: [u8; 16],
}

#[cfg(any(windows, test))]
fn owner_only_windows_pipe_name_from_identity(identity: WindowsDirectoryIdentity) -> String {
    let mut digest = Sha256::new();
    digest.update(b"coven.daemon.pipe.v2\0");
    digest.update(identity.volume_serial_number.to_le_bytes());
    digest.update(identity.file_id);
    let digest = digest.finalize();
    let suffix = digest[..16]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    format!("coven-daemon-v2-{suffix}.sock")
}

#[cfg(windows)]
fn windows_directory_identity(coven_home: &Path) -> Result<WindowsDirectoryIdentity, ClientError> {
    let canonical_home = std::fs::canonicalize(coven_home).map_err(|source| ClientError::Io {
        operation: "failed to resolve Coven home for Windows daemon pipe identity",
        source,
    })?;
    windows_directory_identity_from_resolved_path(&canonical_home)
}

#[cfg(windows)]
fn windows_directory_identity_from_resolved_path(
    resolved_home: &Path,
) -> Result<WindowsDirectoryIdentity, ClientError> {
    use std::{
        mem::{size_of, MaybeUninit},
        os::windows::{ffi::OsStrExt, io::FromRawHandle},
        ptr,
    };
    use windows_sys::Win32::{
        Foundation::INVALID_HANDLE_VALUE,
        Storage::FileSystem::{
            CreateFileW, FileIdInfo, GetFileInformationByHandleEx, FILE_FLAG_BACKUP_SEMANTICS,
            FILE_ID_INFO, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
        },
    };

    let resolved_home: Vec<u16> = resolved_home
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let handle = unsafe {
        CreateFileW(
            resolved_home.as_ptr(),
            0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS,
            ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(ClientError::Io {
            operation: "failed to open canonical Coven home for Windows daemon pipe identity",
            source: std::io::Error::last_os_error(),
        });
    }
    let _directory = unsafe { std::fs::File::from_raw_handle(handle) };
    let mut information = MaybeUninit::<FILE_ID_INFO>::uninit();
    if unsafe {
        GetFileInformationByHandleEx(
            handle,
            FileIdInfo,
            information.as_mut_ptr().cast(),
            size_of::<FILE_ID_INFO>() as u32,
        )
    } == 0
    {
        return Err(ClientError::Io {
            operation: "failed to inspect canonical Coven home for Windows daemon pipe identity",
            source: std::io::Error::last_os_error(),
        });
    }
    // SAFETY: the successful FileIdInfo query initialized the full structure.
    let information = unsafe { information.assume_init() };
    Ok(WindowsDirectoryIdentity {
        volume_serial_number: information.VolumeSerialNumber,
        file_id: information.FileId.Identifier,
    })
}

#[cfg(all(test, not(windows)))]
fn windows_directory_identity(coven_home: &Path) -> Result<WindowsDirectoryIdentity, ClientError> {
    let canonical_home = std::fs::canonicalize(coven_home).map_err(|source| ClientError::Io {
        operation: "failed to resolve test Coven home filesystem identity",
        source,
    })?;
    windows_directory_identity_from_resolved_path(&canonical_home)
}

#[cfg(all(test, not(windows)))]
fn windows_directory_identity_from_resolved_path(
    resolved_home: &Path,
) -> Result<WindowsDirectoryIdentity, ClientError> {
    use std::os::unix::fs::MetadataExt;

    let metadata = std::fs::metadata(resolved_home).map_err(|source| ClientError::Io {
        operation: "failed to inspect test Coven home filesystem identity",
        source,
    })?;
    Ok(WindowsDirectoryIdentity {
        volume_serial_number: metadata.dev(),
        file_id: u128::from(metadata.ino()).to_le_bytes(),
    })
}

#[cfg(any(windows, test))]
pub(crate) fn is_coven_daemon_pipe_name(pipe_name: &str) -> bool {
    is_stable_coven_daemon_pipe_name(pipe_name)
        || is_lowercase_path_coven_daemon_pipe_name(pipe_name)
        || is_legacy_coven_daemon_pipe_name(pipe_name)
}

#[cfg(any(windows, test))]
fn is_stable_coven_daemon_pipe_name(pipe_name: &str) -> bool {
    let Some(name) = pipe_name.strip_suffix(".sock") else {
        return false;
    };
    let Some(hex) = name.strip_prefix("coven-daemon-v2-") else {
        return false;
    };
    is_lowercase_hex_suffix(hex, 32)
}

#[cfg(any(windows, test))]
fn is_lowercase_path_coven_daemon_pipe_name(pipe_name: &str) -> bool {
    let Some(name) = pipe_name.strip_suffix(".sock") else {
        return false;
    };
    let Some(hex) = name.strip_prefix("coven-daemon-v1-") else {
        return false;
    };
    is_lowercase_hex_suffix(hex, 32)
}

#[cfg(any(windows, test))]
fn is_legacy_coven_daemon_pipe_name(pipe_name: &str) -> bool {
    let Some(name) = pipe_name.strip_suffix(".sock") else {
        return false;
    };
    let Some(hex) = name.strip_prefix("coven-daemon-") else {
        return false;
    };
    is_lowercase_hex_suffix(hex, 16)
}

#[cfg(any(windows, test))]
fn legacy_windows_pipe_name(coven_home: &Path) -> String {
    use std::hash::{DefaultHasher, Hash, Hasher};

    let mut hasher = DefaultHasher::new();
    coven_home.to_string_lossy().hash(&mut hasher);
    format!("coven-daemon-{:016x}.sock", hasher.finish())
}

#[cfg(any(windows, test))]
fn lowercase_path_windows_pipe_name(coven_home: &Path) -> String {
    // Preserve the exact v1 lossy/lowercased derivation solely for upgrades.
    let normalized = canonical_windows_home_key(coven_home);
    let mut digest = Sha256::new();
    digest.update(b"coven.daemon.pipe.v1\0");
    digest.update(normalized.as_bytes());
    let digest = digest.finalize();
    let suffix = digest[..16]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    format!("coven-daemon-v1-{suffix}.sock")
}

#[cfg(any(windows, test))]
fn is_lowercase_hex_suffix(value: &str, expected_len: usize) -> bool {
    value.len() == expected_len
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

#[cfg(any(windows, test))]
fn recorded_pipe_name_from_status(serialized: &str) -> Result<String, ClientError> {
    #[derive(serde::Deserialize)]
    struct RecordedDaemonStatus {
        socket: String,
    }

    let status: RecordedDaemonStatus =
        serde_json::from_str(serialized).map_err(ClientError::InvalidJson)?;
    if !is_coven_daemon_pipe_name(&status.socket) {
        return Err(ClientError::Discovery(
            "daemon status reported an unsupported daemon pipe name".to_owned(),
        ));
    }
    Ok(status.socket)
}

#[cfg(any(windows, test))]
fn recorded_pipe_name_from_status_for_home(
    coven_home: &Path,
    serialized: &str,
) -> Result<String, ClientError> {
    let pipe_name = recorded_pipe_name_from_status(serialized)?;
    let matches_profile = if is_stable_coven_daemon_pipe_name(&pipe_name) {
        pipe_name == owner_only_windows_pipe_name(coven_home)?
    } else {
        legacy_windows_pipe_is_safe_for_home(coven_home, &pipe_name)?
    };
    if matches_profile {
        return Ok(pipe_name);
    }
    Err(ClientError::Discovery(
        "daemon status reported a pipe for a different Coven home".to_owned(),
    ))
}

#[cfg(any(windows, test))]
fn legacy_windows_pipe_is_safe_for_home(
    coven_home: &Path,
    pipe_name: &str,
) -> Result<bool, ClientError> {
    if pipe_name == legacy_windows_pipe_name(coven_home) {
        return Ok(true);
    }
    if pipe_name != lowercase_path_windows_pipe_name(coven_home) {
        return Ok(false);
    }
    legacy_windows_v1_home_is_safe(coven_home)
}

#[cfg(any(windows, test))]
fn legacy_windows_v1_home_is_safe(coven_home: &Path) -> Result<bool, ClientError> {
    let canonical_home = std::fs::canonicalize(coven_home).map_err(|source| ClientError::Io {
        operation: "failed to resolve selected Coven home for legacy Windows migration",
        source,
    })?;
    let selected_identity = windows_directory_identity_from_resolved_path(&canonical_home)?;
    verify_legacy_windows_v1_home(
        &canonical_home,
        selected_identity,
        windows_directory_is_case_sensitive,
        |case_folded_home| {
            let resolved_case_folded_home =
                std::fs::canonicalize(case_folded_home).map_err(|source| ClientError::Io {
                    operation:
                        "failed to resolve case-folded Coven home for legacy Windows migration",
                    source,
                })?;
            windows_directory_identity_from_resolved_path(&resolved_case_folded_home)
        },
    )
}

#[cfg(any(windows, test))]
fn verify_legacy_windows_home_ancestors_are_case_insensitive<F>(
    canonical_home: &Path,
    mut directory_is_case_sensitive: F,
) -> Result<bool, ClientError>
where
    F: FnMut(&Path) -> Result<bool, ClientError>,
{
    if !canonical_home.is_absolute() {
        return Err(ClientError::Discovery(
            "canonical Coven home for legacy Windows migration was not absolute".to_owned(),
        ));
    }

    let mut ancestors = canonical_home
        .ancestors()
        .filter(|ancestor| !ancestor.as_os_str().is_empty())
        .collect::<Vec<_>>();
    ancestors.reverse();
    for ancestor in ancestors {
        if directory_is_case_sensitive(ancestor)? {
            return Ok(false);
        }
    }
    Ok(true)
}

#[cfg(any(windows, test))]
fn verify_legacy_windows_v1_home<FCaseSensitivity, FResolveFolded>(
    canonical_home: &Path,
    selected_identity: WindowsDirectoryIdentity,
    inspect_case_sensitivity: FCaseSensitivity,
    resolve_case_folded_identity: FResolveFolded,
) -> Result<bool, ClientError>
where
    FCaseSensitivity: FnMut(&Path) -> Result<bool, ClientError>,
    FResolveFolded: FnOnce(&Path) -> Result<WindowsDirectoryIdentity, ClientError>,
{
    if !verify_legacy_windows_home_ancestors_are_case_insensitive(
        canonical_home,
        inspect_case_sensitivity,
    )? {
        return Ok(false);
    }
    verify_legacy_case_folded_windows_home_identity(
        canonical_home,
        selected_identity,
        resolve_case_folded_identity,
    )
}

#[cfg(any(windows, test))]
fn verify_legacy_case_folded_windows_home_identity<F>(
    canonical_home: &Path,
    selected_identity: WindowsDirectoryIdentity,
    resolve_case_folded_identity: F,
) -> Result<bool, ClientError>
where
    F: FnOnce(&Path) -> Result<WindowsDirectoryIdentity, ClientError>,
{
    let canonical_home = canonical_home.to_str().ok_or_else(|| {
        ClientError::Discovery(
            "canonical Coven home cannot be represented by the legacy Windows path identity"
                .to_owned(),
        )
    })?;
    let case_folded_home = std::path::PathBuf::from(canonical_home.to_ascii_lowercase());
    Ok(resolve_case_folded_identity(&case_folded_home)? == selected_identity)
}

#[cfg(windows)]
fn windows_directory_is_case_sensitive(directory: &Path) -> Result<bool, ClientError> {
    use std::{
        mem::{size_of, MaybeUninit},
        os::windows::{ffi::OsStrExt, io::FromRawHandle},
        ptr,
    };
    use windows_sys::Win32::{
        Foundation::INVALID_HANDLE_VALUE,
        Storage::FileSystem::{
            CreateFileW, FileCaseSensitiveInfo, GetFileInformationByHandle,
            GetFileInformationByHandleEx, BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_REPARSE_POINT,
            FILE_CASE_SENSITIVE_INFO, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
            FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
        },
    };
    const FILE_CS_FLAG_CASE_SENSITIVE_DIR: u32 = 0x0000_0001;

    let directory: Vec<u16> = directory
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    // SAFETY: `directory` is NUL-terminated and all pointer arguments remain
    // valid for the duration of the call.
    let handle = unsafe {
        CreateFileW(
            directory.as_ptr(),
            0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(ClientError::Io {
            operation:
                "failed to open canonical Coven home ancestor for case-sensitivity validation",
            source: std::io::Error::last_os_error(),
        });
    }
    // SAFETY: ownership of the checked handle transfers exactly once.
    let _directory = unsafe { std::fs::File::from_raw_handle(handle) };

    let mut attributes = MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::uninit();
    // SAFETY: the handle is live and `attributes` is the exact writable output
    // structure required by GetFileInformationByHandle.
    if unsafe { GetFileInformationByHandle(handle, attributes.as_mut_ptr()) } == 0 {
        return Err(ClientError::Io {
            operation:
                "failed to inspect canonical Coven home ancestor for case-sensitivity validation",
            source: std::io::Error::last_os_error(),
        });
    }
    // SAFETY: the successful query initialized the full structure.
    let attributes = unsafe { attributes.assume_init() };
    if attributes.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(ClientError::Discovery(
            "canonical Coven home ancestor was an unverifiable reparse point".to_owned(),
        ));
    }

    let mut information = MaybeUninit::<FILE_CASE_SENSITIVE_INFO>::uninit();
    // SAFETY: the live directory handle and output buffer match
    // FileCaseSensitiveInfo for the duration of the call.
    if unsafe {
        GetFileInformationByHandleEx(
            handle,
            FileCaseSensitiveInfo,
            information.as_mut_ptr().cast(),
            size_of::<FILE_CASE_SENSITIVE_INFO>() as u32,
        )
    } == 0
    {
        return Err(ClientError::Io {
            operation:
                "failed to inspect canonical Coven home ancestor case-sensitivity information",
            source: std::io::Error::last_os_error(),
        });
    }
    // SAFETY: the successful query initialized the full structure.
    let information = unsafe { information.assume_init() };
    Ok(information.Flags & FILE_CS_FLAG_CASE_SENSITIVE_DIR != 0)
}

#[cfg(all(test, not(windows)))]
fn windows_directory_is_case_sensitive(_directory: &Path) -> Result<bool, ClientError> {
    Ok(false)
}

#[cfg(any(windows, test))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WindowsStatusFileSecurity {
    Hardened,
    LegacyCurrentOwner,
}

#[cfg(any(windows, test))]
const fn windows_status_validation_phase(security: WindowsStatusFileSecurity) -> &'static str {
    match security {
        WindowsStatusFileSecurity::Hardened => "validating Coven daemon status",
        WindowsStatusFileSecurity::LegacyCurrentOwner => {
            "authenticating legacy Coven daemon status"
        }
    }
}

#[cfg(any(windows, test))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum UnavailableLegacyPipePolicy {
    Reject,
    ReturnRecordedStatus,
}

#[cfg(test)]
fn trusted_windows_status<P>(
    coven_home: &Path,
    serialized: String,
    security: WindowsStatusFileSecurity,
    probe: P,
) -> Result<String, ClientError>
where
    P: FnOnce(&str, std::time::Duration) -> Result<Option<(u16, Vec<u8>)>, ClientError>,
{
    trusted_windows_status_with_policy(
        coven_home,
        serialized,
        security,
        UnavailableLegacyPipePolicy::Reject,
        probe,
    )
}

#[cfg(test)]
fn trusted_windows_status_for_lifecycle<P>(
    coven_home: &Path,
    serialized: String,
    security: WindowsStatusFileSecurity,
    probe: P,
) -> Result<String, ClientError>
where
    P: FnOnce(&str, std::time::Duration) -> Result<Option<(u16, Vec<u8>)>, ClientError>,
{
    trusted_windows_status_with_policy(
        coven_home,
        serialized,
        security,
        UnavailableLegacyPipePolicy::ReturnRecordedStatus,
        probe,
    )
}

#[cfg(test)]
fn trusted_windows_status_with_policy<P>(
    coven_home: &Path,
    serialized: String,
    security: WindowsStatusFileSecurity,
    unavailable_legacy_pipe: UnavailableLegacyPipePolicy,
    probe: P,
) -> Result<String, ClientError>
where
    P: FnOnce(&str, std::time::Duration) -> Result<Option<(u16, Vec<u8>)>, ClientError>,
{
    trusted_windows_status_with_policy_and_timeout(
        coven_home,
        serialized,
        security,
        unavailable_legacy_pipe,
        std::time::Duration::from_secs(2),
        probe,
    )
}

#[cfg(any(windows, test))]
fn trusted_windows_status_with_policy_and_timeout<P>(
    coven_home: &Path,
    serialized: String,
    security: WindowsStatusFileSecurity,
    unavailable_legacy_pipe: UnavailableLegacyPipePolicy,
    probe_timeout: std::time::Duration,
    probe: P,
) -> Result<String, ClientError>
where
    P: FnOnce(&str, std::time::Duration) -> Result<Option<(u16, Vec<u8>)>, ClientError>,
{
    match security {
        WindowsStatusFileSecurity::Hardened => {
            recorded_pipe_name_from_status_for_home(coven_home, &serialized)?;
            Ok(serialized)
        }
        WindowsStatusFileSecurity::LegacyCurrentOwner => legacy_status_from_probe_with_policy(
            coven_home,
            &serialized,
            unavailable_legacy_pipe,
            probe_timeout,
            probe,
        ),
    }
}

#[cfg(test)]
fn legacy_status_from_probe<P>(
    coven_home: &Path,
    serialized: &str,
    probe: P,
) -> Result<String, ClientError>
where
    P: FnOnce(&str, std::time::Duration) -> Result<Option<(u16, Vec<u8>)>, ClientError>,
{
    legacy_status_from_probe_with_policy(
        coven_home,
        serialized,
        UnavailableLegacyPipePolicy::Reject,
        std::time::Duration::from_secs(2),
        probe,
    )
}

#[cfg(any(windows, test))]
fn legacy_status_from_probe_with_policy<P>(
    coven_home: &Path,
    serialized: &str,
    unavailable_pipe: UnavailableLegacyPipePolicy,
    probe_timeout: std::time::Duration,
    probe: P,
) -> Result<String, ClientError>
where
    P: FnOnce(&str, std::time::Duration) -> Result<Option<(u16, Vec<u8>)>, ClientError>,
{
    #[derive(serde::Deserialize)]
    struct LegacyHealth {
        ok: bool,
        daemon: Option<serde_json::Value>,
    }

    let recorded_pipe = match recorded_pipe_name_from_status(serialized) {
        Ok(recorded_pipe) => recorded_pipe,
        Err(_) if unavailable_pipe == UnavailableLegacyPipePolicy::ReturnRecordedStatus => {
            return Err(ClientError::Discovery(
                "inherited daemon status identity could not be validated".to_owned(),
            ));
        }
        Err(error) => return Err(error),
    };
    if !legacy_windows_pipe_is_safe_for_home(coven_home, &recorded_pipe)? {
        return Err(ClientError::Discovery(
            "inherited daemon status did not name a legacy pipe for this Coven home".to_owned(),
        ));
    }
    let Some((status, body)) = probe(&recorded_pipe, probe_timeout)? else {
        return match unavailable_pipe {
            UnavailableLegacyPipePolicy::Reject => Err(ClientError::Discovery(
                "legacy daemon pipe was not available through an owner-validated connection"
                    .to_owned(),
            )),
            UnavailableLegacyPipePolicy::ReturnRecordedStatus => {
                validate_legacy_lifecycle_identity(serialized)?;
                Ok(serialized.to_owned())
            }
        };
    };
    if status != 200 {
        return Err(ClientError::Discovery(format!(
            "legacy daemon health returned HTTP {status}"
        )));
    }
    let health: LegacyHealth = serde_json::from_slice(&body).map_err(|_| {
        ClientError::Discovery("legacy daemon health response was invalid".to_owned())
    })?;
    if !health.ok {
        return Err(ClientError::Discovery(
            "legacy daemon health did not report ready".to_owned(),
        ));
    }
    let daemon = health.daemon.ok_or_else(|| {
        ClientError::Discovery("legacy daemon health did not include daemon status".to_owned())
    })?;
    if daemon.get("socket").and_then(serde_json::Value::as_str) != Some(recorded_pipe.as_str()) {
        return Err(ClientError::Discovery(
            "legacy daemon health reported a pipe for a different Coven home".to_owned(),
        ));
    }
    serde_json::to_string(&daemon)
        .map_err(|_| ClientError::Discovery("legacy daemon health status was invalid".to_owned()))
}

#[cfg(any(windows, test))]
fn validate_legacy_lifecycle_identity(serialized: &str) -> Result<(), ClientError> {
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct LegacyLifecycleIdentity {
        pid: u32,
        started_at: String,
    }

    let identity: LegacyLifecycleIdentity = serde_json::from_str(serialized).map_err(|_| {
        ClientError::Discovery("inherited daemon status identity could not be validated".to_owned())
    })?;
    if identity.pid == 0 {
        return Err(ClientError::Discovery(
            "inherited daemon status PID was not a process identity".to_owned(),
        ));
    }
    let _ = identity.started_at;
    Ok(())
}

#[cfg(any(windows, test))]
fn canonical_windows_home_key(coven_home: &Path) -> String {
    #[cfg(windows)]
    let coven_home = std::fs::canonicalize(coven_home).unwrap_or_else(|_| coven_home.to_path_buf());

    #[cfg(not(windows))]
    let coven_home = coven_home.to_path_buf();

    let mut path = coven_home.to_string_lossy().replace('/', "\\");
    let mut is_unc = path.starts_with(r"\\");
    if let Some(unc_path) = path.strip_prefix(r"\\?\UNC\") {
        path = format!(r"\\{unc_path}");
        is_unc = true;
    } else if let Some(path_without_prefix) = path.strip_prefix(r"\\?\") {
        path = path_without_prefix.to_owned();
    }
    let mut components = Vec::new();
    if is_unc {
        components.push("unc".to_owned());
    }
    for component in path.split('\\') {
        match component {
            "" | "." => {}
            ".." => {
                components.pop();
            }
            component => components.push(component.to_ascii_lowercase()),
        }
    }
    components.join("\\")
}

#[cfg(test)]
const OWNER_ONLY_PIPE_ACCESS_MASK: u32 = 0x1000_0000;
#[cfg(any(windows, test))]
const WINDOWS_FILE_ALL_ACCESS_MASK: u32 = 0x001f_01ff;

#[cfg(any(windows, test))]
#[derive(Clone, Copy)]
struct PipeAccessRule {
    is_allow: bool,
    access_mask: u32,
    applies_to_owner_rights: bool,
}

#[cfg(any(windows, test))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WindowsAceParseError {
    Unsupported,
    Malformed,
}

#[cfg(any(windows, test))]
fn parse_windows_access_allowed_ace(bytes: &[u8]) -> Result<PipeAccessRule, WindowsAceParseError> {
    const ACCESS_ALLOWED_ACE_TYPE: u8 = 0;
    const SID_REVISION: u8 = 1;
    const SID_MAX_SUB_AUTHORITIES: usize = 15;
    const ACE_PREFIX_BYTES: usize = 8;
    const SID_PREFIX_BYTES: usize = 8;
    const OWNER_RIGHTS_SID: [u8; 12] = [1, 1, 0, 0, 0, 0, 0, 3, 4, 0, 0, 0];

    if bytes.len() < ACE_PREFIX_BYTES {
        return Err(WindowsAceParseError::Malformed);
    }
    if bytes[0] != ACCESS_ALLOWED_ACE_TYPE || bytes[1] != 0 {
        return Err(WindowsAceParseError::Unsupported);
    }
    let ace_size = usize::from(u16::from_le_bytes([bytes[2], bytes[3]]));
    if ace_size != bytes.len() || ace_size % 4 != 0 {
        return Err(WindowsAceParseError::Malformed);
    }

    let sid = bytes
        .get(ACE_PREFIX_BYTES..ace_size)
        .ok_or(WindowsAceParseError::Malformed)?;
    if sid.len() < SID_PREFIX_BYTES || sid[0] != SID_REVISION {
        return Err(WindowsAceParseError::Malformed);
    }
    let sub_authorities = usize::from(sid[1]);
    if sub_authorities > SID_MAX_SUB_AUTHORITIES {
        return Err(WindowsAceParseError::Malformed);
    }
    let sid_size = SID_PREFIX_BYTES
        .checked_add(
            sub_authorities
                .checked_mul(std::mem::size_of::<u32>())
                .ok_or(WindowsAceParseError::Malformed)?,
        )
        .ok_or(WindowsAceParseError::Malformed)?;
    if sid.len() != sid_size {
        return Err(WindowsAceParseError::Malformed);
    }

    Ok(PipeAccessRule {
        is_allow: true,
        access_mask: u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]),
        applies_to_owner_rights: sid == OWNER_RIGHTS_SID,
    })
}

#[cfg(any(windows, test))]
fn owner_only_pipe_security_is_valid(
    owner_is_current_user: bool,
    access_rules: &[PipeAccessRule],
) -> bool {
    let [rule] = access_rules else {
        return false;
    };
    owner_is_current_user
        && rule.is_allow
        && rule.applies_to_owner_rights
        && canonical_windows_file_access_mask(rule.access_mask) == WINDOWS_FILE_ALL_ACCESS_MASK
}

#[cfg(windows)]
fn canonical_windows_file_access_mask(mut access_mask: u32) -> u32 {
    use windows_sys::Win32::{
        Security::{MapGenericMask, GENERIC_MAPPING},
        Storage::FileSystem::{
            FILE_ALL_ACCESS, FILE_GENERIC_EXECUTE, FILE_GENERIC_READ, FILE_GENERIC_WRITE,
        },
    };

    let mapping = GENERIC_MAPPING {
        GenericRead: FILE_GENERIC_READ,
        GenericWrite: FILE_GENERIC_WRITE,
        GenericExecute: FILE_GENERIC_EXECUTE,
        GenericAll: FILE_ALL_ACCESS,
    };
    unsafe {
        MapGenericMask(&mut access_mask, &mapping);
    }
    access_mask
}

#[cfg(all(test, not(windows)))]
fn canonical_windows_file_access_mask(mut access_mask: u32) -> u32 {
    const GENERIC_READ: u32 = 0x8000_0000;
    const GENERIC_WRITE: u32 = 0x4000_0000;
    const GENERIC_EXECUTE: u32 = 0x2000_0000;
    const GENERIC_ALL: u32 = 0x1000_0000;
    const FILE_GENERIC_READ: u32 = 0x0012_0089;
    const FILE_GENERIC_WRITE: u32 = 0x0012_0116;
    const FILE_GENERIC_EXECUTE: u32 = 0x0012_00a0;

    for (generic, mapped) in [
        (GENERIC_READ, FILE_GENERIC_READ),
        (GENERIC_WRITE, FILE_GENERIC_WRITE),
        (GENERIC_EXECUTE, FILE_GENERIC_EXECUTE),
        (GENERIC_ALL, WINDOWS_FILE_ALL_ACCESS_MASK),
    ] {
        if access_mask & generic != 0 {
            access_mask = (access_mask & !generic) | mapped;
        }
    }
    access_mask
}

#[cfg(any(windows, test))]
struct WindowsTokenBuffer {
    words: Vec<usize>,
}

#[cfg(any(windows, test))]
impl WindowsTokenBuffer {
    fn new(byte_len: usize) -> Self {
        let word_len = byte_len.max(1).div_ceil(std::mem::size_of::<usize>());
        Self {
            words: vec![0; word_len],
        }
    }

    fn as_mut_ptr(&mut self) -> *mut std::ffi::c_void {
        self.words.as_mut_ptr().cast()
    }

    #[cfg(windows)]
    fn as_ptr(&self) -> *const std::ffi::c_void {
        self.words.as_ptr().cast()
    }

    fn byte_capacity(&self) -> usize {
        self.words.len() * std::mem::size_of::<usize>()
    }
}

#[cfg(windows)]
const _: () = assert!(
    std::mem::align_of::<usize>()
        >= std::mem::align_of::<windows_sys::Win32::Security::TOKEN_USER>()
);

#[cfg(any(windows, test))]
fn read_bounded_windows_status<R: std::io::Read>(
    reader: R,
    known_size: u64,
) -> Result<String, ClientError> {
    use std::io::Read as _;

    if known_size > MAX_DAEMON_STATUS_BYTES as u64 {
        return Err(ClientError::Discovery(format!(
            "daemon status exceeded the {MAX_DAEMON_STATUS_BYTES}-byte limit"
        )));
    }
    let capacity = usize::try_from(known_size).map_err(|_| {
        ClientError::Discovery("daemon status size was not representable".to_owned())
    })?;
    let mut bytes = Vec::with_capacity(capacity);
    reader
        .take((MAX_DAEMON_STATUS_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| {
            ClientError::Discovery(format!("cannot read owner-only daemon status: {error}"))
        })?;
    if bytes.len() > MAX_DAEMON_STATUS_BYTES {
        return Err(ClientError::Discovery(format!(
            "daemon status exceeded the {MAX_DAEMON_STATUS_BYTES}-byte limit"
        )));
    }
    String::from_utf8(bytes)
        .map_err(|_| ClientError::Discovery("daemon status was not valid UTF-8".to_owned()))
}

#[cfg(any(windows, test))]
fn finite_windows_security_wait_millis(remaining: std::time::Duration) -> Option<u32> {
    (!remaining.is_zero()).then(|| remaining.as_millis().max(1).min((u32::MAX - 1) as u128) as u32)
}

#[cfg(any(windows, test))]
const fn windows_pipe_client_flags() -> u32 {
    // SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION prevents a named-pipe
    // server from impersonating the client while its ACL is inspected.
    0x0010_0000 | 0x0001_0000
}

#[cfg(test)]
mod tests {
    use super::{
        finite_windows_security_wait_millis, is_coven_daemon_pipe_name, legacy_status_from_probe,
        legacy_windows_pipe_name, lowercase_path_windows_pipe_name,
        owner_only_pipe_security_is_valid, owner_only_windows_pipe_name,
        owner_only_windows_pipe_name_from_identity, parse_windows_access_allowed_ace,
        read_bounded_windows_status, recorded_pipe_name_from_status,
        recorded_pipe_name_from_status_for_home, verify_legacy_case_folded_windows_home_identity,
        verify_legacy_windows_home_ancestors_are_case_insensitive, verify_legacy_windows_v1_home,
        windows_pipe_client_flags, windows_status_validation_phase, PipeAccessRule,
        WindowsDirectoryIdentity, WindowsStatusFileSecurity, WindowsTokenBuffer,
        MAX_DAEMON_STATUS_BYTES, OWNER_ONLY_PIPE_ACCESS_MASK,
    };
    #[cfg(windows)]
    use super::{
        legacy_windows_v1_home_is_safe, windows_directory_is_case_sensitive,
        windows_status_file_share_mode,
    };
    use crate::ClientError;
    use std::{
        hash::{DefaultHasher, Hash, Hasher},
        path::{Path, PathBuf},
    };

    fn historical_legacy_windows_pipe_name(coven_home: &Path) -> String {
        let mut hasher = DefaultHasher::new();
        coven_home.to_string_lossy().hash(&mut hasher);
        format!("coven-daemon-{:016x}.sock", hasher.finish())
    }

    #[cfg(windows)]
    fn enable_windows_directory_case_sensitivity(directory: &Path) -> std::io::Result<()> {
        use std::{
            mem::size_of,
            os::windows::{ffi::OsStrExt, io::FromRawHandle},
            ptr,
        };
        use windows_sys::Win32::{
            Foundation::INVALID_HANDLE_VALUE,
            Storage::FileSystem::{
                CreateFileW, FileCaseSensitiveInfo, SetFileInformationByHandle,
                FILE_CASE_SENSITIVE_INFO, FILE_FLAG_BACKUP_SEMANTICS, FILE_SHARE_DELETE,
                FILE_SHARE_READ, FILE_SHARE_WRITE, FILE_WRITE_ATTRIBUTES, OPEN_EXISTING,
            },
        };

        let directory: Vec<u16> = directory
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        // SAFETY: `directory` is NUL-terminated and the pointers remain valid
        // for the duration of the call.
        let handle = unsafe {
            CreateFileW(
                directory.as_ptr(),
                FILE_WRITE_ATTRIBUTES,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS,
                ptr::null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: ownership of the checked handle transfers exactly once.
        let _directory = unsafe { std::fs::File::from_raw_handle(handle) };
        let information = FILE_CASE_SENSITIVE_INFO { Flags: 1 };
        // SAFETY: the handle is live and the immutable information buffer has
        // the exact type and size required by FileCaseSensitiveInfo.
        if unsafe {
            SetFileInformationByHandle(
                handle,
                FileCaseSensitiveInfo,
                std::ptr::addr_of!(information).cast(),
                size_of::<FILE_CASE_SENSITIVE_INFO>() as u32,
            )
        } == 0
        {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    }

    #[cfg(windows)]
    fn windows_case_sensitivity_is_unavailable(error: &std::io::Error) -> bool {
        matches!(error.raw_os_error(), Some(1 | 5 | 50 | 87 | 1314))
    }

    #[test]
    fn windows_pipe_names_do_not_alias_distinct_case_sensitive_directories() {
        let upper = WindowsDirectoryIdentity {
            volume_serial_number: 7,
            file_id: 100_u128.to_le_bytes(),
        };
        let lower = WindowsDirectoryIdentity {
            volume_serial_number: 7,
            file_id: 101_u128.to_le_bytes(),
        };

        assert_ne!(
            owner_only_windows_pipe_name_from_identity(upper),
            owner_only_windows_pipe_name_from_identity(lower),
            "distinct filesystem identities must never share a daemon pipe"
        );
    }

    #[test]
    fn windows_pipe_identity_hashes_every_file_id_byte_without_truncation() {
        let lower_half = WindowsDirectoryIdentity {
            volume_serial_number: 0x8877_6655_4433_2211,
            file_id: [
                0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd,
                0xee, 0xff,
            ],
        };
        let mut upper_half = lower_half;
        upper_half.file_id[15] ^= 0x80;

        assert_eq!(
            owner_only_windows_pipe_name_from_identity(lower_half),
            "coven-daemon-v2-0ac550f4d9218a34aea335e8edd4d6f2.sock"
        );
        assert_ne!(
            owner_only_windows_pipe_name_from_identity(lower_half),
            owner_only_windows_pipe_name_from_identity(upper_half),
            "changing only the high 64 bits of FILE_ID_128 must change the pipe identity"
        );
    }

    #[test]
    fn windows_pipe_name_is_stable_for_equivalent_coven_home_spellings() {
        let home = Path::new(env!("CARGO_MANIFEST_DIR"));
        let expected = owner_only_windows_pipe_name(home).expect("derive canonical home pipe");

        assert_eq!(
            owner_only_windows_pipe_name(&home.join(".")).expect("derive dot-alias pipe"),
            expected
        );
        assert_eq!(
            owner_only_windows_pipe_name(
                &std::fs::canonicalize(home).expect("canonical home spelling")
            )
            .expect("derive canonical-spelling pipe"),
            expected
        );
    }

    #[test]
    fn supported_windows_pipe_names_cover_v2_v1_and_v0_identities() {
        let home = Path::new(env!("CARGO_MANIFEST_DIR"));
        let names = super::supported_windows_pipe_names(home).expect("supported Windows names");

        assert_eq!(names.len(), 3);
        assert!(super::is_stable_coven_daemon_pipe_name(&names[0]));
        assert!(super::is_lowercase_path_coven_daemon_pipe_name(&names[1]));
        assert!(super::is_legacy_coven_daemon_pipe_name(&names[2]));
        assert_ne!(names[0], names[1]);
        assert_ne!(names[1], names[2]);
    }

    #[test]
    fn status_deadline_phase_names_only_legacy_authentication_as_legacy() {
        assert_eq!(
            windows_status_validation_phase(WindowsStatusFileSecurity::Hardened),
            "validating Coven daemon status"
        );
        assert_eq!(
            windows_status_validation_phase(WindowsStatusFileSecurity::LegacyCurrentOwner),
            "authenticating legacy Coven daemon status"
        );
    }

    #[test]
    fn windows_security_inspection_waits_are_finite_and_preserve_submillisecond_budget() {
        assert_eq!(
            finite_windows_security_wait_millis(std::time::Duration::ZERO),
            None
        );
        assert_eq!(
            finite_windows_security_wait_millis(std::time::Duration::from_nanos(1)),
            Some(1)
        );
        assert_eq!(
            finite_windows_security_wait_millis(std::time::Duration::from_millis(
                u64::from(u32::MAX) + 1
            )),
            Some(u32::MAX - 1)
        );
    }

    #[test]
    fn named_pipe_security_inspection_requests_identification_only_sqos() {
        assert_eq!(windows_pipe_client_flags(), 0x0011_0000);
    }

    #[test]
    fn recorded_windows_pipe_candidates_accept_only_coven_stable_or_legacy_shapes() {
        assert!(is_coven_daemon_pipe_name(
            "coven-daemon-v2-ea05fac3452199fa7c8e19af2cc07659.sock"
        ));
        assert!(is_coven_daemon_pipe_name(
            "coven-daemon-v1-ea05fac3452199fa7c8e19af2cc07659.sock"
        ));
        assert!(is_coven_daemon_pipe_name(
            "coven-daemon-0123456789abcdef.sock"
        ));
        assert!(!is_coven_daemon_pipe_name(r"\\.\pipe\other-daemon.sock"));
        assert!(!is_coven_daemon_pipe_name("coven-daemon-not-hex.sock"));
        assert!(!is_coven_daemon_pipe_name("coven-daemon-v1-0123.sock"));
    }

    #[test]
    fn recorded_daemon_status_accepts_only_allowlisted_pipe_names() {
        assert_eq!(
            recorded_pipe_name_from_status(r#"{"socket":"coven-daemon-0123456789abcdef.sock"}"#)
                .expect("allow legacy pipe"),
            "coven-daemon-0123456789abcdef.sock"
        );
        assert!(recorded_pipe_name_from_status(r#"{"socket":"other-daemon.sock"}"#).is_err());
        assert!(recorded_pipe_name_from_status(r#"{"socket":42}"#).is_err());
    }

    #[test]
    fn recorded_daemon_status_rejects_a_stable_pipe_for_another_profile() {
        let coven_home = Path::new(env!("CARGO_MANIFEST_DIR"));
        let other_home = coven_home.parent().expect("workspace crates directory");
        let other_profile_pipe =
            owner_only_windows_pipe_name(other_home).expect("derive other profile pipe");
        let serialized = format!(r#"{{"socket":"{other_profile_pipe}"}}"#);

        assert!(recorded_pipe_name_from_status_for_home(coven_home, &serialized).is_err());
    }

    #[test]
    fn lowercase_hash_status_is_a_profile_bound_migration_candidate() {
        let coven_home = Path::new(std::path::MAIN_SEPARATOR_STR);
        let lowercase_pipe = lowercase_path_windows_pipe_name(coven_home);
        let serialized = format!(r#"{{"socket":"{lowercase_pipe}"}}"#);

        assert_eq!(
            recorded_pipe_name_from_status_for_home(coven_home, &serialized)
                .expect("same-profile lowercase-hash status remains migratable"),
            lowercase_pipe
        );
        let other_home = Path::new(r"C:\CovenTest\Other");
        assert!(
            recorded_pipe_name_from_status_for_home(other_home, &serialized).is_err(),
            "a lowercase-hash migration record must remain bound to its selected profile"
        );
    }

    #[test]
    fn lowercase_hash_migration_requires_the_case_folded_alias_to_have_the_same_full_identity() {
        let canonical_home = Path::new(r"\\?\C:\Profiles\Foo\Coven");
        let selected = WindowsDirectoryIdentity {
            volume_serial_number: 0x8877_6655_4433_2211,
            file_id: [
                0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd,
                0xee, 0xff,
            ],
        };

        let same_alias = verify_legacy_case_folded_windows_home_identity(
            canonical_home,
            selected,
            |case_folded_home| {
                assert_eq!(
                    case_folded_home,
                    Path::new(r"\\?\c:\profiles\foo\coven"),
                    "the exact v1 ASCII-case-folded spelling must be resolved"
                );
                Ok(selected)
            },
        )
        .expect("same-identity alias can be verified");
        assert!(same_alias);

        let mut distinct_foo = selected;
        distinct_foo.file_id[15] ^= 0x80;
        assert!(
            !verify_legacy_case_folded_windows_home_identity(canonical_home, selected, |_| Ok(
                distinct_foo
            ),)
            .expect("distinct Foo/foo identities can be compared"),
            "all 128 file-ID bits and the volume must match"
        );

        let mut distinct_volume = selected;
        distinct_volume.volume_serial_number ^= 1;
        assert!(
            !verify_legacy_case_folded_windows_home_identity(canonical_home, selected, |_| Ok(
                distinct_volume
            ),)
            .expect("distinct volumes can be compared"),
            "a matching file ID on another volume is not the selected profile"
        );

        assert!(
            verify_legacy_case_folded_windows_home_identity(canonical_home, selected, |_| {
                Err(ClientError::Discovery(
                    "case-folded spelling did not resolve".to_owned(),
                ))
            })
            .is_err(),
            "an unresolvable or unverifiable folded spelling must fail closed"
        );
    }

    #[test]
    fn legacy_v1_case_check_walks_every_component_from_root_to_coven_home() {
        #[cfg(windows)]
        let canonical_home = Path::new(r"C:\Profiles\foo\Coven");
        #[cfg(not(windows))]
        let canonical_home = Path::new("/Profiles/foo/Coven");

        let mut inspected = Vec::new();
        assert!(verify_legacy_windows_home_ancestors_are_case_insensitive(
            canonical_home,
            |directory| {
                inspected.push(directory.to_path_buf());
                Ok(false)
            },
        )
        .expect("all path components can be verified"));

        #[cfg(windows)]
        let expected = [
            PathBuf::from(r"C:\"),
            PathBuf::from(r"C:\Profiles"),
            PathBuf::from(r"C:\Profiles\foo"),
            PathBuf::from(r"C:\Profiles\foo\Coven"),
        ];
        #[cfg(not(windows))]
        let expected = [
            PathBuf::from("/"),
            PathBuf::from("/Profiles"),
            PathBuf::from("/Profiles/foo"),
            PathBuf::from("/Profiles/foo/Coven"),
        ];
        assert_eq!(inspected, expected);
    }

    #[test]
    fn legacy_v1_case_check_rejects_sensitive_or_unverifiable_ancestors() {
        let canonical_home = std::env::current_dir()
            .expect("current directory")
            .join("Profiles")
            .join("foo")
            .join("Coven");
        let mut after_sensitive_ancestor_was_inspected = false;
        let safe = verify_legacy_windows_home_ancestors_are_case_insensitive(
            &canonical_home,
            |directory| {
                if after_sensitive_ancestor_was_inspected {
                    panic!("case-sensitive ancestors must reject before inspecting descendants");
                }
                let sensitive = directory.file_name().is_some_and(|name| name == "Profiles");
                after_sensitive_ancestor_was_inspected = sensitive;
                Ok(sensitive)
            },
        )
        .expect("case-sensitive status was verified");
        assert!(!safe);

        let error = verify_legacy_windows_home_ancestors_are_case_insensitive(
            &canonical_home,
            |directory| {
                if directory.file_name().is_some_and(|name| name == "foo") {
                    return Err(ClientError::Discovery(
                        "case-sensitivity query unavailable".to_owned(),
                    ));
                }
                Ok(false)
            },
        )
        .expect_err("an unverifiable ancestor must fail closed");
        assert!(error.to_string().contains("query unavailable"));
    }

    #[test]
    fn legacy_v1_rejects_lowercase_selected_home_under_case_sensitive_parent() {
        let canonical_home = std::env::current_dir()
            .expect("current directory")
            .join("Profiles")
            .join("foo")
            .join("Coven");
        let selected = WindowsDirectoryIdentity {
            volume_serial_number: 7,
            file_id: 101_u128.to_le_bytes(),
        };
        let sibling = WindowsDirectoryIdentity {
            volume_serial_number: 7,
            file_id: 102_u128.to_le_bytes(),
        };
        assert_ne!(selected, sibling, "foo and Foo are distinct directories");

        let accepted = verify_legacy_windows_v1_home(
            &canonical_home,
            selected,
            |directory| Ok(directory.file_name().is_some_and(|name| name == "Profiles")),
            |case_folded_home| {
                assert_eq!(
                    case_folded_home,
                    PathBuf::from(canonical_home.to_string_lossy().to_ascii_lowercase())
                );
                Ok(selected)
            },
        )
        .expect("case sensitivity was verified");
        assert!(
            !accepted,
            "the lowercase foo identity matching itself cannot make v1 safe when Foo collides"
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_case_sensitive_ancestor_blocks_lowercase_v1_sibling_collision() {
        struct Cleanup(PathBuf);

        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }

        let root = std::env::current_dir()
            .expect("current directory")
            .join("target")
            .join(format!(
                "coven-client-case-sensitive-{}",
                std::process::id()
            ));
        let cleanup = Cleanup(root.clone());
        let profiles = root.join("profiles");
        std::fs::create_dir_all(&profiles).expect("create case-sensitivity test parent");
        if let Err(error) = enable_windows_directory_case_sensitivity(&profiles) {
            if windows_case_sensitivity_is_unavailable(&error) {
                eprintln!(
                    "skipping case-sensitive-directory runtime test: Windows denied support ({error})"
                );
                return;
            }
            panic!("enable case-sensitive-directory test fixture: {error}");
        }
        assert!(windows_directory_is_case_sensitive(&profiles)
            .expect("query enabled directory case sensitivity"));

        let lower_home = profiles.join("foo").join("coven");
        let upper_home = profiles.join("Foo").join("coven");
        std::fs::create_dir_all(&lower_home).expect("create lowercase profile");
        std::fs::create_dir_all(&upper_home).expect("create uppercase sibling profile");
        assert_ne!(
            super::windows_directory_identity(&lower_home).expect("lowercase profile identity"),
            super::windows_directory_identity(&upper_home).expect("uppercase profile identity")
        );
        assert_eq!(
            lowercase_path_windows_pipe_name(&lower_home),
            lowercase_path_windows_pipe_name(&upper_home),
            "the v1 lowercase hash aliases foo and Foo"
        );

        let canonical_lower = std::fs::canonicalize(&lower_home).expect("canonical lowercase home");
        let selected = super::windows_directory_identity_from_resolved_path(&canonical_lower)
            .expect("selected profile identity");
        let folded_identity_still_matches =
            verify_legacy_case_folded_windows_home_identity(&canonical_lower, selected, |folded| {
                let folded = std::fs::canonicalize(folded).map_err(|source| ClientError::Io {
                    operation: "failed to resolve folded Windows test profile",
                    source,
                })?;
                super::windows_directory_identity_from_resolved_path(&folded)
            })
            .expect("resolve lowercase selected profile");
        if !folded_identity_still_matches {
            eprintln!(
                "skipping sibling-collision assertion: an outer test ancestor is case-sensitive"
            );
            return;
        }
        assert!(
            !legacy_windows_v1_home_is_safe(&lower_home)
                .expect("verify every selected profile ancestor"),
            "a matching folded identity cannot make the colliding v1 pipe safe"
        );
        let serialized = format!(
            r#"{{"socket":"{}"}}"#,
            lowercase_path_windows_pipe_name(&lower_home)
        );
        assert!(
            recorded_pipe_name_from_status_for_home(&lower_home, &serialized).is_err(),
            "discovery must reject the ambiguous inherited v1 status"
        );
        drop(cleanup);
    }

    #[test]
    fn recorded_daemon_status_accepts_a_strict_legacy_pipe_for_its_profile() {
        let coven_home = Path::new(r"C:\CovenTest\Profile");
        let legacy_pipe = historical_legacy_windows_pipe_name(coven_home);
        assert_eq!(
            legacy_pipe, "coven-daemon-fa932683394959d8.sock",
            "the historical DefaultHasher profile identity must remain compatible"
        );
        let serialized = format!(r#"{{"socket":"{legacy_pipe}"}}"#);

        assert_eq!(
            recorded_pipe_name_from_status_for_home(coven_home, &serialized)
                .expect("legacy pipe is a supported upgrade record"),
            legacy_pipe
        );
    }

    #[test]
    fn recorded_daemon_status_rejects_a_legacy_pipe_for_another_profile() {
        let coven_home = Path::new(r"C:\CovenTest\Profile");
        let other_profile_pipe =
            historical_legacy_windows_pipe_name(Path::new(r"C:\CovenTest\Other"));
        let serialized = format!(r#"{{"socket":"{other_profile_pipe}"}}"#);

        assert!(recorded_pipe_name_from_status_for_home(coven_home, &serialized).is_err());
    }

    #[test]
    fn inherited_lowercase_hash_status_uses_owner_validated_profile_pipe_health() {
        let coven_home = Path::new(std::path::MAIN_SEPARATOR_STR);
        let expected_pipe = lowercase_path_windows_pipe_name(coven_home);
        let serialized =
            format!(r#"{{"pid":999,"startedAt":"untrusted","socket":"{expected_pipe}"}}"#);
        let observed = std::cell::RefCell::new(None);

        let trusted = super::trusted_windows_status(
            coven_home,
            serialized,
            WindowsStatusFileSecurity::LegacyCurrentOwner,
            |pipe_name, timeout| {
                observed.replace(Some((pipe_name.to_owned(), timeout)));
                Ok(Some((
                    200,
                    format!(
                        r#"{{"ok":true,"daemon":{{"pid":42,"startedAt":"trusted","socket":"{expected_pipe}"}}}}"#
                    )
                    .into_bytes(),
                )))
            },
        )
        .expect("same-profile inherited legacy status should use the validated health response");

        assert_eq!(
            observed.into_inner(),
            Some((expected_pipe.clone(), std::time::Duration::from_secs(2)))
        );
        let trusted: serde_json::Value =
            serde_json::from_str(&trusted).expect("trusted daemon status JSON");
        assert_eq!(trusted["pid"], 42);
        assert_eq!(trusted["startedAt"], "trusted");
        assert_eq!(trusted["socket"], expected_pipe);
    }

    #[test]
    fn legacy_status_probe_uses_only_the_remaining_outer_deadline() {
        let start = std::time::Instant::now();
        let deadline = start + std::time::Duration::from_millis(100);
        assert_eq!(
            super::windows_status_remaining_at(
                deadline,
                start + std::time::Duration::from_millis(65),
                "authenticating legacy Coven daemon status",
            )
            .expect("budget remains"),
            std::time::Duration::from_millis(35)
        );
        let error = super::windows_status_remaining_at(
            deadline,
            deadline,
            "authenticating legacy Coven daemon status",
        )
        .expect_err("expired deadline");
        assert!(matches!(
            error,
            ClientError::Io {
                operation: "authenticating legacy Coven daemon status",
                source,
            } if source.kind() == std::io::ErrorKind::TimedOut
        ));
    }

    #[test]
    fn inherited_legacy_status_rejects_cross_profile_and_arbitrary_redirection_before_connecting() {
        let coven_home = Path::new(r"C:\CovenTest\Profile");
        let other_pipe = legacy_windows_pipe_name(Path::new(r"C:\CovenTest\Other"));

        for redirected in [other_pipe.as_str(), "other-daemon.sock"] {
            let serialized = format!(r#"{{"socket":"{redirected}"}}"#);
            let result = legacy_status_from_probe(coven_home, &serialized, |_, _| {
                panic!("a redirected inherited status file must not select a pipe")
            });
            assert!(
                result.is_err(),
                "redirected status was accepted: {serialized}"
            );
        }
    }

    #[test]
    fn inherited_legacy_status_rejects_a_health_response_for_another_pipe() {
        let coven_home = Path::new(r"C:\CovenTest\Profile");
        let expected_pipe = legacy_windows_pipe_name(coven_home);
        let serialized = format!(r#"{{"socket":"{expected_pipe}"}}"#);

        let result = legacy_status_from_probe(coven_home, &serialized, |_, _| {
            Ok(Some((
                200,
                br#"{"ok":true,"daemon":{"pid":42,"startedAt":"trusted","socket":"coven-daemon-0000000000000000.sock"}}"#
                    .to_vec(),
            )))
        });

        assert!(result.is_err());
    }

    #[test]
    fn lifecycle_legacy_status_allows_only_an_unavailable_same_profile_pipe() {
        let coven_home = Path::new(r"C:\CovenTest\Profile");
        let expected_pipe = legacy_windows_pipe_name(coven_home);
        let serialized = format!(r#"{{"pid":42,"startedAt":"legacy","socket":"{expected_pipe}"}}"#);

        let accepted = super::trusted_windows_status_for_lifecycle(
            coven_home,
            serialized.clone(),
            WindowsStatusFileSecurity::LegacyCurrentOwner,
            |pipe_name, _| {
                assert_eq!(pipe_name, expected_pipe);
                Ok(None)
            },
        )
        .expect("CLI lifecycle may inspect a same-profile unavailable legacy record");
        assert_eq!(accepted, serialized);

        assert!(
            super::trusted_windows_status(
                coven_home,
                serialized,
                WindowsStatusFileSecurity::LegacyCurrentOwner,
                |_, _| Ok(None),
            )
            .is_err(),
            "endpoint discovery must continue to reject an unavailable legacy pipe"
        );
    }

    #[test]
    fn lifecycle_legacy_status_rejects_redirects_and_pipe_validation_errors() {
        let coven_home = Path::new(r"C:\CovenTest\Profile");
        let other_pipe = legacy_windows_pipe_name(Path::new(r"C:\CovenTest\Other"));
        for redirected in [other_pipe.as_str(), "other-daemon.sock"] {
            let serialized =
                format!(r#"{{"pid":42,"startedAt":"legacy","socket":"{redirected}"}}"#);
            let result = super::trusted_windows_status_for_lifecycle(
                coven_home,
                serialized,
                WindowsStatusFileSecurity::LegacyCurrentOwner,
                |_, _| panic!("redirected status must be rejected before probing"),
            );
            assert!(result.is_err(), "accepted redirected status {redirected}");
        }

        let expected_pipe = legacy_windows_pipe_name(coven_home);
        let serialized = format!(r#"{{"pid":42,"startedAt":"legacy","socket":"{expected_pipe}"}}"#);
        let result = super::trusted_windows_status_for_lifecycle(
            coven_home,
            serialized,
            WindowsStatusFileSecurity::LegacyCurrentOwner,
            |_, _| {
                Err(ClientError::Discovery(
                    "connected legacy pipe failed owner-handle validation".to_owned(),
                ))
            },
        );
        let error = result.expect_err("available but untrusted pipe must fail closed");
        assert!(error.to_string().contains("owner-handle validation"));

        let invalid_health = super::trusted_windows_status_for_lifecycle(
            coven_home,
            format!(r#"{{"pid":42,"startedAt":"legacy","socket":"{expected_pipe}"}}"#),
            WindowsStatusFileSecurity::LegacyCurrentOwner,
            |_, _| Ok(Some((200, b"{invalid health".to_vec()))),
        );
        assert!(
            matches!(invalid_health, Err(ClientError::Discovery(_))),
            "invalid legacy health identity did not fail closed: {invalid_health:?}"
        );

        for ambiguous in [
            format!(r#"{{"startedAt":"legacy","socket":"{expected_pipe}"}}"#),
            format!(r#"{{"pid":"42","startedAt":"legacy","socket":"{expected_pipe}"}}"#),
            format!(r#"{{"pid":0,"startedAt":"legacy","socket":"{expected_pipe}"}}"#),
        ] {
            let result = super::trusted_windows_status_for_lifecycle(
                coven_home,
                ambiguous,
                WindowsStatusFileSecurity::LegacyCurrentOwner,
                |_, _| Ok(None),
            );
            assert!(
                matches!(result, Err(ClientError::Discovery(_))),
                "ambiguous legacy lifecycle identity did not fail closed: {result:?}"
            );
        }
    }

    #[test]
    fn hardened_status_content_never_uses_the_legacy_probe_path() {
        let coven_home = Path::new(env!("CARGO_MANIFEST_DIR"));
        let stable_pipe =
            owner_only_windows_pipe_name(coven_home).expect("derive stable profile pipe");
        let serialized = format!(r#"{{"socket":"{stable_pipe}"}}"#);

        let trusted = super::trusted_windows_status(
            coven_home,
            serialized.clone(),
            WindowsStatusFileSecurity::Hardened,
            |_, _| panic!("a hardened status file must not use legacy downgrade probing"),
        )
        .expect("hardened same-profile status");

        assert_eq!(trusted, serialized);
    }

    #[test]
    fn owner_only_pipe_validation_accepts_generic_and_mapped_file_all_access() {
        let owner_rights_allow = PipeAccessRule {
            is_allow: true,
            access_mask: OWNER_ONLY_PIPE_ACCESS_MASK,
            applies_to_owner_rights: true,
        };
        let mapped_file_all_access = PipeAccessRule {
            access_mask: 0x001f_01ff,
            ..owner_rights_allow
        };

        assert!(owner_only_pipe_security_is_valid(
            true,
            &[owner_rights_allow]
        ));
        assert!(owner_only_pipe_security_is_valid(
            true,
            &[mapped_file_all_access]
        ));
        assert!(!owner_only_pipe_security_is_valid(
            false,
            &[owner_rights_allow]
        ));
        assert!(!owner_only_pipe_security_is_valid(
            true,
            &[PipeAccessRule {
                access_mask: OWNER_ONLY_PIPE_ACCESS_MASK,
                applies_to_owner_rights: false,
                ..owner_rights_allow
            }]
        ));
        assert!(!owner_only_pipe_security_is_valid(
            true,
            &[
                owner_rights_allow,
                PipeAccessRule {
                    is_allow: true,
                    access_mask: OWNER_ONLY_PIPE_ACCESS_MASK,
                    applies_to_owner_rights: true,
                },
            ]
        ));
        assert!(!owner_only_pipe_security_is_valid(
            true,
            &[PipeAccessRule {
                access_mask: 0x011f_01ff,
                ..owner_rights_allow
            }]
        ));
    }

    fn owner_rights_allow_ace() -> Vec<u8> {
        let mut ace = vec![0, 0];
        ace.extend_from_slice(&20_u16.to_le_bytes());
        ace.extend_from_slice(&OWNER_ONLY_PIPE_ACCESS_MASK.to_le_bytes());
        ace.extend_from_slice(&[1, 1, 0, 0, 0, 0, 0, 3, 4, 0, 0, 0]);
        ace
    }

    #[test]
    fn windows_ace_parser_accepts_only_a_bounded_plain_owner_rights_allow_ace() {
        let valid = owner_rights_allow_ace();
        let rule =
            parse_windows_access_allowed_ace(&valid).expect("parse plain owner-rights allow ACE");
        assert!(rule.is_allow);
        assert_eq!(rule.access_mask, OWNER_ONLY_PIPE_ACCESS_MASK);
        assert!(rule.applies_to_owner_rights);

        let mut unaligned = vec![0xff];
        unaligned.extend_from_slice(&valid);
        assert!(
            parse_windows_access_allowed_ace(&unaligned[1..]).is_ok(),
            "byte parser must not require aligned ACE storage"
        );

        for prefix_len in 0..valid.len() {
            assert!(
                parse_windows_access_allowed_ace(&valid[..prefix_len]).is_err(),
                "accepted truncated ACE prefix of {prefix_len} bytes"
            );
        }

        for unsupported_type in [1_u8, 5, 6, 9, 10, 11, 12, 13, 15] {
            let mut hostile = valid.clone();
            hostile[0] = unsupported_type;
            assert!(
                parse_windows_access_allowed_ace(&hostile).is_err(),
                "accepted unsupported ACE type {unsupported_type}"
            );
        }

        for claimed_size in [0_u16, 4, 8, 16, u16::MAX] {
            let mut hostile = valid.clone();
            hostile[2..4].copy_from_slice(&claimed_size.to_le_bytes());
            assert!(
                parse_windows_access_allowed_ace(&hostile).is_err(),
                "accepted hostile ACE size {claimed_size}"
            );
        }

        let mut bad_sid_revision = valid.clone();
        bad_sid_revision[8] = 0;
        assert!(parse_windows_access_allowed_ace(&bad_sid_revision).is_err());

        let mut oversized_sid = valid.clone();
        oversized_sid[9] = 16;
        assert!(parse_windows_access_allowed_ace(&oversized_sid).is_err());

        let mut flagged = valid.clone();
        flagged[1] = 1;
        assert!(parse_windows_access_allowed_ace(&flagged).is_err());

        let mut trailing = valid;
        trailing.push(0);
        assert!(parse_windows_access_allowed_ace(&trailing).is_err());
    }

    #[test]
    fn windows_token_storage_is_word_aligned_and_covers_requested_bytes() {
        for requested_bytes in [1, 3, 8, 31, 257] {
            let mut buffer = WindowsTokenBuffer::new(requested_bytes);
            assert_eq!(
                buffer.as_mut_ptr() as usize % std::mem::align_of::<usize>(),
                0
            );
            assert!(buffer.byte_capacity() >= requested_bytes);
        }
    }

    #[test]
    fn windows_status_limit_is_checked_before_read_and_rechecked_for_growth() {
        struct MustNotRead;

        impl std::io::Read for MustNotRead {
            fn read(&mut self, _buffer: &mut [u8]) -> std::io::Result<usize> {
                panic!("an oversized status must be rejected before any read or allocation")
            }
        }

        let error = read_bounded_windows_status(MustNotRead, (MAX_DAEMON_STATUS_BYTES + 1) as u64)
            .expect_err("known oversized status");
        assert!(error.to_string().contains("16384-byte limit"));

        let growing = std::io::Cursor::new(vec![b'x'; MAX_DAEMON_STATUS_BYTES + 1]);
        let error = read_bounded_windows_status(growing, 0)
            .expect_err("status growth after metadata inspection must remain bounded");
        assert!(error.to_string().contains("16384-byte limit"));
    }

    #[cfg(windows)]
    #[test]
    fn status_file_reader_allows_an_atomic_status_replacement() {
        use std::{ffi::OsStr, os::windows::ffi::OsStrExt, ptr};
        use windows_sys::Win32::{
            Foundation::{CloseHandle, GENERIC_READ, INVALID_HANDLE_VALUE},
            Storage::FileSystem::{
                CreateFileW, MoveFileExW, FILE_ATTRIBUTE_NORMAL, MOVEFILE_REPLACE_EXISTING,
                MOVEFILE_WRITE_THROUGH, OPEN_EXISTING,
            },
        };

        let test_prefix = format!(".coven-client-status-share-{}", std::process::id());
        let status_path = std::env::current_dir()
            .expect("test working directory")
            .join(format!("{test_prefix}.json"));
        let replacement_path = status_path.with_file_name(format!("{test_prefix}-next.json"));
        let _ = std::fs::remove_file(&status_path);
        let _ = std::fs::remove_file(&replacement_path);
        std::fs::write(&status_path, b"{\"pid\":1}").expect("write current status");
        std::fs::write(&replacement_path, b"{\"pid\":2}").expect("write replacement status");
        let status_path_wide: Vec<u16> = OsStr::new(&status_path)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let replacement_path_wide: Vec<u16> = OsStr::new(&replacement_path)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();

        let reader = unsafe {
            CreateFileW(
                status_path_wide.as_ptr(),
                GENERIC_READ,
                windows_status_file_share_mode(),
                ptr::null(),
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL,
                ptr::null_mut(),
            )
        };
        assert_ne!(reader, INVALID_HANDLE_VALUE, "open status reader");
        let replaced = unsafe {
            MoveFileExW(
                replacement_path_wide.as_ptr(),
                status_path_wide.as_ptr(),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
        };
        let replacement_error = std::io::Error::last_os_error();
        unsafe {
            CloseHandle(reader);
        }
        let _ = std::fs::remove_file(&replacement_path);
        let _ = std::fs::remove_file(&status_path);
        assert_ne!(
            replaced, 0,
            "atomic status replacement must not be blocked by a reader: {replacement_error}"
        );
    }
}

#[cfg(windows)]
#[doc(hidden)]
pub fn validate_windows_daemon_pipe_name(pipe_name: &str) -> Result<(), ClientError> {
    let deadline = std::time::Instant::now()
        .checked_add(WINDOWS_DISCOVERY_TIMEOUT)
        .ok_or_else(|| ClientError::Discovery("daemon discovery deadline overflowed".to_owned()))?;
    validate_windows_daemon_pipe_name_until(pipe_name, deadline)
}

#[cfg(windows)]
fn validate_windows_daemon_pipe_name_until(
    pipe_name: &str,
    deadline: std::time::Instant,
) -> Result<(), ClientError> {
    if !is_coven_daemon_pipe_name(pipe_name) {
        return Err(ClientError::Discovery(
            "unsupported Coven daemon pipe name".to_owned(),
        ));
    }
    validate_owner_only_windows_pipe_until(pipe_name, deadline)
}

#[cfg(windows)]
fn validate_owner_only_windows_pipe_until(
    pipe_name: &str,
    deadline: std::time::Instant,
) -> Result<(), ClientError> {
    use std::ffi::OsStr;

    let pipe_path = format!(r"\\.\pipe\{pipe_name}");
    validate_owner_only_windows_named_pipe_until(
        OsStr::new(&pipe_path),
        "Coven daemon pipe",
        deadline,
    )
}

#[cfg(windows)]
fn validate_owner_only_windows_named_pipe_until(
    object_path: &std::ffi::OsStr,
    object_label: &str,
    deadline: std::time::Instant,
) -> Result<(), ClientError> {
    use std::{
        os::windows::{
            ffi::OsStrExt,
            io::{AsRawHandle, FromRawHandle},
        },
        ptr,
    };
    use windows_sys::Win32::{
        Foundation::{
            ERROR_FILE_NOT_FOUND, ERROR_PATH_NOT_FOUND, ERROR_PIPE_BUSY, ERROR_SEM_TIMEOUT,
            INVALID_HANDLE_VALUE,
        },
        Storage::FileSystem::{CreateFileW, OPEN_EXISTING, READ_CONTROL},
    };

    let mut object_path: Vec<u16> = object_path
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    loop {
        deadline
            .checked_duration_since(std::time::Instant::now())
            .filter(|remaining| !remaining.is_zero())
            .ok_or_else(|| windows_pipe_security_timeout(object_label))?;
        let handle = unsafe {
            CreateFileW(
                object_path.as_mut_ptr(),
                READ_CONTROL,
                0,
                ptr::null(),
                OPEN_EXISTING,
                windows_pipe_client_flags(),
                ptr::null_mut(),
            )
        };
        if handle != INVALID_HANDLE_VALUE {
            let file = unsafe { std::fs::File::from_raw_handle(handle) };
            return validate_owner_only_windows_pipe_handle(file.as_raw_handle());
        }
        let source = std::io::Error::last_os_error();
        let code = source.raw_os_error();
        if code != Some(ERROR_PIPE_BUSY as i32) {
            return Err(ClientError::Discovery(format!(
                "cannot inspect owner-only {object_label}: {source}"
            )));
        }

        let remaining = deadline
            .checked_duration_since(std::time::Instant::now())
            .filter(|remaining| !remaining.is_zero())
            .ok_or_else(|| windows_pipe_security_timeout(object_label))?;
        let wait_millis = finite_windows_security_wait_millis(remaining)
            .ok_or_else(|| windows_pipe_security_timeout(object_label))?;
        if unsafe {
            windows_sys::Win32::System::Pipes::WaitNamedPipeW(object_path.as_ptr(), wait_millis)
        } == 0
        {
            let wait_error = std::io::Error::last_os_error();
            if wait_error.raw_os_error() == Some(ERROR_SEM_TIMEOUT as i32)
                || std::time::Instant::now() >= deadline
            {
                return Err(windows_pipe_security_timeout(object_label));
            }
            let wait_code = wait_error.raw_os_error();
            if wait_code != Some(ERROR_PIPE_BUSY as i32)
                && wait_code != Some(ERROR_FILE_NOT_FOUND as i32)
                && wait_code != Some(ERROR_PATH_NOT_FOUND as i32)
            {
                return Err(ClientError::Discovery(format!(
                    "cannot inspect owner-only {object_label}: {wait_error}"
                )));
            }
            std::thread::sleep(
                deadline
                    .saturating_duration_since(std::time::Instant::now())
                    .min(std::time::Duration::from_millis(1)),
            );
        }
    }
}

#[cfg(windows)]
fn windows_pipe_security_timeout(object_label: &str) -> ClientError {
    ClientError::Discovery(format!("timed out inspecting owner-only {object_label}"))
}

#[cfg(windows)]
fn validate_owner_only_windows_owner_and_dacl(
    owner: windows_sys::Win32::Security::PSID,
    dacl: *mut windows_sys::Win32::Security::ACL,
    object_label: &str,
) -> Result<(), ClientError> {
    use windows_sys::Win32::Security::EqualSid;

    if owner.is_null() || dacl.is_null() {
        return Err(ClientError::Discovery(format!(
            "{object_label} does not expose an owner-only security descriptor"
        )));
    }

    let current_user = current_process_user_sid()?;
    let owner_is_current_user = unsafe { EqualSid(owner, current_user.as_ptr()) != 0 };
    let dacl_is_owner_only = exact_owner_only_windows_dacl(dacl, object_label)?;
    if !owner_is_current_user || !dacl_is_owner_only {
        return Err(ClientError::Discovery(format!(
            "{object_label} is not owned by the current user with an owner-only DACL"
        )));
    }
    Ok(())
}

#[cfg(windows)]
fn exact_owner_only_windows_dacl(
    dacl: *mut windows_sys::Win32::Security::ACL,
    object_label: &str,
) -> Result<bool, ClientError> {
    use std::{mem::size_of, ptr};
    use windows_sys::Win32::Security::{
        AclSizeInformation, GetAce, GetAclInformation, ACL, ACL_SIZE_INFORMATION,
    };

    let mut acl_size = ACL_SIZE_INFORMATION::default();
    if unsafe {
        GetAclInformation(
            dacl,
            (&mut acl_size as *mut ACL_SIZE_INFORMATION).cast(),
            size_of::<ACL_SIZE_INFORMATION>() as u32,
            AclSizeInformation,
        ) == 0
    } {
        return Err(ClientError::Discovery(format!(
            "cannot inspect {object_label} access rules"
        )));
    }
    if acl_size.AceCount != 1 {
        return Ok(false);
    }

    let mut ace = ptr::null_mut();
    if unsafe { GetAce(dacl, 0, &mut ace) == 0 } || ace.is_null() {
        return Err(ClientError::Discovery(format!(
            "cannot read {object_label} access rule"
        )));
    }
    let acl_bytes = usize::try_from(acl_size.AclBytesInUse).map_err(|_| {
        ClientError::Discovery(format!("{object_label} ACL size was not representable"))
    })?;
    if acl_bytes < size_of::<ACL>() {
        return Err(ClientError::Discovery(format!(
            "{object_label} ACL was smaller than its header"
        )));
    }
    let acl_start = dacl.cast::<u8>() as usize;
    let acl_end = acl_start
        .checked_add(acl_bytes)
        .ok_or_else(|| ClientError::Discovery(format!("{object_label} ACL bounds overflowed")))?;
    let acl_header_end = acl_start.checked_add(size_of::<ACL>()).ok_or_else(|| {
        ClientError::Discovery(format!("{object_label} ACL header bounds overflowed"))
    })?;
    let ace_start = ace.cast::<u8>() as usize;
    let ace_header_end = ace_start.checked_add(4).ok_or_else(|| {
        ClientError::Discovery(format!("{object_label} ACE header bounds overflowed"))
    })?;
    if ace_start < acl_header_end || ace_header_end > acl_end {
        return Err(ClientError::Discovery(format!(
            "{object_label} ACE header was outside its ACL"
        )));
    }
    let header = unsafe { std::slice::from_raw_parts(ace.cast::<u8>(), 4) };
    let ace_size = usize::from(u16::from_le_bytes([header[2], header[3]]));
    let ace_end = ace_start
        .checked_add(ace_size)
        .ok_or_else(|| ClientError::Discovery(format!("{object_label} ACE bounds overflowed")))?;
    if ace_size < 4 || ace_end > acl_end {
        return Err(ClientError::Discovery(format!(
            "{object_label} ACE size exceeded its ACL"
        )));
    }
    let ace_bytes = unsafe { std::slice::from_raw_parts(ace.cast::<u8>(), ace_size) };
    let rule = match parse_windows_access_allowed_ace(ace_bytes) {
        Ok(rule) => rule,
        Err(WindowsAceParseError::Unsupported) => return Ok(false),
        Err(WindowsAceParseError::Malformed) => {
            return Err(ClientError::Discovery(format!(
                "{object_label} access rule was malformed"
            )))
        }
    };
    Ok(owner_only_pipe_security_is_valid(true, &[rule]))
}

#[cfg(windows)]
fn classify_windows_status_file_handle(
    file: &std::fs::File,
    object_label: &str,
) -> Result<WindowsStatusFileSecurity, ClientError> {
    use std::os::windows::io::AsRawHandle;
    use std::ptr;
    use windows_sys::Win32::Security::{
        Authorization::{GetSecurityInfo, SE_FILE_OBJECT},
        EqualSid, DACL_SECURITY_INFORMATION, OWNER_SECURITY_INFORMATION,
    };

    let mut owner = ptr::null_mut();
    let mut dacl = ptr::null_mut();
    let mut descriptor = ptr::null_mut();
    let status = unsafe {
        GetSecurityInfo(
            file.as_raw_handle(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            &mut owner,
            ptr::null_mut(),
            &mut dacl,
            ptr::null_mut(),
            &mut descriptor,
        )
    };
    if status != 0 {
        return Err(ClientError::Discovery(format!(
            "cannot inspect {object_label}: Windows error {status}"
        )));
    }
    let _descriptor = LocalAllocation(descriptor);
    if owner.is_null() || dacl.is_null() {
        return Err(ClientError::Discovery(format!(
            "{object_label} did not expose an owner and DACL"
        )));
    }
    let current_user = current_process_user_sid()?;
    if unsafe { EqualSid(owner, current_user.as_ptr()) } == 0 {
        return Err(ClientError::Discovery(format!(
            "{object_label} is not owned by the current user"
        )));
    }
    if exact_owner_only_windows_dacl(dacl, object_label)? {
        Ok(WindowsStatusFileSecurity::Hardened)
    } else {
        Ok(WindowsStatusFileSecurity::LegacyCurrentOwner)
    }
}

#[cfg(windows)]
pub(crate) fn validate_owner_only_windows_pipe_handle(
    handle: std::os::windows::io::RawHandle,
) -> Result<(), ClientError> {
    use windows_sys::Win32::Security::Authorization::SE_KERNEL_OBJECT;

    validate_owner_only_windows_handle(handle, SE_KERNEL_OBJECT, "connected Coven daemon pipe")
}

#[cfg(windows)]
fn validate_owner_only_windows_handle(
    handle: std::os::windows::io::RawHandle,
    object_type: windows_sys::Win32::Security::Authorization::SE_OBJECT_TYPE,
    object_label: &str,
) -> Result<(), ClientError> {
    use std::ptr;
    use windows_sys::Win32::Security::{
        Authorization::GetSecurityInfo, DACL_SECURITY_INFORMATION, OWNER_SECURITY_INFORMATION,
    };

    let mut owner = ptr::null_mut();
    let mut dacl = ptr::null_mut();
    let mut descriptor = ptr::null_mut();
    let status = unsafe {
        GetSecurityInfo(
            handle,
            object_type,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            &mut owner,
            ptr::null_mut(),
            &mut dacl,
            ptr::null_mut(),
            &mut descriptor,
        )
    };
    if status != 0 {
        return Err(ClientError::Discovery(format!(
            "cannot inspect owner-only {object_label}: Windows error {status}"
        )));
    }
    let _descriptor = LocalAllocation(descriptor);
    validate_owner_only_windows_owner_and_dacl(owner, dacl, object_label)
}

#[cfg(windows)]
fn recorded_windows_pipe_candidate_until(
    coven_home: &Path,
    deadline: std::time::Instant,
) -> Result<Option<String>, ClientError> {
    read_windows_daemon_status(coven_home, UnavailableLegacyPipePolicy::Reject, deadline)?
        .map(|serialized| recorded_pipe_name_from_status_for_home(coven_home, &serialized))
        .transpose()
}

#[cfg(windows)]
#[doc(hidden)]
pub fn read_validated_windows_daemon_status(
    coven_home: &Path,
) -> Result<Option<String>, ClientError> {
    let deadline = std::time::Instant::now()
        .checked_add(std::time::Duration::from_secs(2))
        .ok_or_else(|| ClientError::Discovery("daemon status deadline overflowed".to_owned()))?;
    read_windows_daemon_status(coven_home, UnavailableLegacyPipePolicy::Reject, deadline)
}

/// Read daemon status for CLI lifecycle recovery.
///
/// Unlike endpoint discovery, this permits an inherited-ACL legacy record only
/// when it names this profile's derived legacy pipe and that pipe is absent.
/// The lifecycle caller must still prove the recorded PID dead before clearing
/// the record. Available pipes remain subject to owner-handle validation.
#[cfg(windows)]
#[doc(hidden)]
pub fn read_windows_daemon_status_for_lifecycle(
    coven_home: &Path,
) -> Result<Option<String>, ClientError> {
    let deadline = std::time::Instant::now()
        .checked_add(std::time::Duration::from_secs(2))
        .ok_or_else(|| ClientError::Discovery("daemon status deadline overflowed".to_owned()))?;
    read_windows_daemon_status_for_lifecycle_until(coven_home, deadline)
}

#[cfg(windows)]
#[doc(hidden)]
pub fn read_windows_daemon_status_for_lifecycle_until(
    coven_home: &Path,
    deadline: std::time::Instant,
) -> Result<Option<String>, ClientError> {
    read_windows_daemon_status(
        coven_home,
        UnavailableLegacyPipePolicy::ReturnRecordedStatus,
        deadline,
    )
}

#[cfg(windows)]
fn read_windows_daemon_status(
    coven_home: &Path,
    unavailable_legacy_pipe: UnavailableLegacyPipePolicy,
    deadline: std::time::Instant,
) -> Result<Option<String>, ClientError> {
    windows_status_remaining_at(
        deadline,
        std::time::Instant::now(),
        "opening Coven daemon status",
    )?;
    let Some((status_file, security, size)) = open_validated_windows_status_file(coven_home)?
    else {
        return Ok(None);
    };
    windows_status_remaining_at(
        deadline,
        std::time::Instant::now(),
        "reading Coven daemon status",
    )?;
    let serialized = read_bounded_windows_status(status_file, size)?;
    let probe_timeout = windows_status_remaining_at(
        deadline,
        std::time::Instant::now(),
        windows_status_validation_phase(security),
    )?;
    let status = trusted_windows_status_with_policy_and_timeout(
        coven_home,
        serialized,
        security,
        unavailable_legacy_pipe,
        probe_timeout,
        |pipe_name, _| {
            crate::transport::probe_windows_daemon_health_with_identity_until(pipe_name, deadline)
                .map(|probe| probe.map(|probe| (probe.status, probe.body)))
        },
    )?;
    windows_status_remaining_at(
        deadline,
        std::time::Instant::now(),
        "validating Coven daemon status",
    )?;
    Ok(Some(status))
}

#[cfg(windows)]
fn open_validated_windows_status_file(
    coven_home: &Path,
) -> Result<Option<(std::fs::File, WindowsStatusFileSecurity, u64)>, ClientError> {
    use std::{
        os::windows::{ffi::OsStrExt, io::FromRawHandle},
        ptr,
    };
    use windows_sys::Win32::{
        Foundation::{ERROR_FILE_NOT_FOUND, GENERIC_READ, INVALID_HANDLE_VALUE},
        Storage::FileSystem::{
            CreateFileW, GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION,
            FILE_ATTRIBUTE_NORMAL, FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_OPEN_REPARSE_POINT,
            OPEN_EXISTING, READ_CONTROL,
        },
    };

    let status_path = coven_home.join("daemon.json");
    let status_path: Vec<u16> = status_path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let handle = unsafe {
        CreateFileW(
            status_path.as_ptr(),
            GENERIC_READ | READ_CONTROL,
            windows_status_file_share_mode(),
            ptr::null(),
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL | FILE_FLAG_OPEN_REPARSE_POINT,
            ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() == Some(ERROR_FILE_NOT_FOUND as i32) {
            return Ok(None);
        }
        return Err(ClientError::Discovery(format!(
            "cannot open owner-only daemon status: {error}"
        )));
    }
    let file = unsafe { std::fs::File::from_raw_handle(handle) };
    let mut information = BY_HANDLE_FILE_INFORMATION::default();
    if unsafe { GetFileInformationByHandle(handle, &mut information) == 0 } {
        return Err(ClientError::Discovery(format!(
            "cannot inspect owner-only daemon status: {}",
            std::io::Error::last_os_error()
        )));
    }
    if information.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(ClientError::Discovery(
            "daemon status must not be a reparse point".to_owned(),
        ));
    }
    let size = (u64::from(information.nFileSizeHigh) << 32) | u64::from(information.nFileSizeLow);
    let security = classify_windows_status_file_handle(&file, "Coven daemon status file")?;
    Ok(Some((file, security, size)))
}

#[cfg(windows)]
fn windows_status_file_share_mode() -> u32 {
    use windows_sys::Win32::Storage::FileSystem::{FILE_SHARE_DELETE, FILE_SHARE_READ};

    FILE_SHARE_READ | FILE_SHARE_DELETE
}

#[cfg(windows)]
struct WindowsSid(WindowsTokenBuffer);

#[cfg(windows)]
impl WindowsSid {
    fn as_ptr(&self) -> windows_sys::Win32::Security::PSID {
        self.0.as_ptr().cast_mut()
    }
}

#[cfg(windows)]
fn current_process_user_sid() -> Result<WindowsSid, ClientError> {
    use std::{mem::size_of, ptr};
    use windows_sys::Win32::{
        Foundation::{GetLastError, ERROR_INSUFFICIENT_BUFFER},
        Security::{
            CopySid, GetLengthSid, GetTokenInformation, TokenUser, TOKEN_QUERY, TOKEN_USER,
        },
        System::Threading::{GetCurrentProcess, OpenProcessToken},
    };

    let mut process_token = ptr::null_mut();
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut process_token) == 0 } {
        return Err(ClientError::Discovery(format!(
            "cannot inspect current Windows user: {}",
            std::io::Error::last_os_error()
        )));
    }
    let _token = WindowsHandle(process_token);
    let mut bytes = 0;
    let initial =
        unsafe { GetTokenInformation(process_token, TokenUser, ptr::null_mut(), 0, &mut bytes) };
    if initial != 0 || unsafe { GetLastError() } != ERROR_INSUFFICIENT_BUFFER || bytes == 0 {
        return Err(ClientError::Discovery(
            "cannot size current Windows user token".to_owned(),
        ));
    }
    let mut buffer = WindowsTokenBuffer::new(bytes as usize);
    if unsafe {
        GetTokenInformation(
            process_token,
            TokenUser,
            buffer.as_mut_ptr(),
            bytes,
            &mut bytes,
        ) == 0
    } {
        return Err(ClientError::Discovery(format!(
            "cannot read current Windows user token: {}",
            std::io::Error::last_os_error()
        )));
    }
    if (bytes as usize) < size_of::<TOKEN_USER>() || bytes as usize > buffer.byte_capacity() {
        return Err(ClientError::Discovery(
            "current Windows user token returned an invalid size".to_owned(),
        ));
    }
    let user = unsafe { &*buffer.as_ptr().cast::<TOKEN_USER>() };
    if user.User.Sid.is_null() {
        return Err(ClientError::Discovery(
            "current Windows user token had no SID".to_owned(),
        ));
    }
    let length = unsafe { GetLengthSid(user.User.Sid) } as usize;
    if length == 0 {
        return Err(ClientError::Discovery(
            "current Windows user token had an invalid SID".to_owned(),
        ));
    }
    let mut sid = WindowsTokenBuffer::new(length);
    if unsafe { CopySid(length as u32, sid.as_mut_ptr(), user.User.Sid) } == 0 {
        return Err(ClientError::Discovery(format!(
            "cannot copy current Windows user SID: {}",
            std::io::Error::last_os_error()
        )));
    }
    Ok(WindowsSid(sid))
}

#[cfg(windows)]
struct WindowsHandle(windows_sys::Win32::Foundation::HANDLE);

#[cfg(windows)]
impl Drop for WindowsHandle {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.0);
        }
    }
}

#[cfg(windows)]
struct LocalAllocation(*mut std::ffi::c_void);

#[cfg(windows)]
impl Drop for LocalAllocation {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::Foundation::LocalFree(self.0);
        }
    }
}
