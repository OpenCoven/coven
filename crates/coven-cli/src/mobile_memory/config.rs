use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, ToSocketAddrs};
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use url::{Host, Url};
use uuid::Uuid;

pub const MOBILE_STATE_DIR: &str = "mobile";
pub const GATEWAY_CONFIG_FILE: &str = "gateway.json";

/// Safe classification for background startup; retain the original cause for
/// direct callers without copying it into startup diagnostics.
#[derive(Debug)]
pub(crate) struct MobileConfigValidationFailure;

impl std::fmt::Display for MobileConfigValidationFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("invalid mobile gateway configuration")
    }
}

impl std::error::Error for MobileConfigValidationFailure {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct MobileGatewayConfig {
    pub enabled: bool,
    pub bind: SocketAddr,
    pub advertised_endpoint: String,
}

pub fn validate_mobile_config(config: &MobileGatewayConfig) -> Result<()> {
    validate_mobile_config_with(config, |(host, port)| {
        (host, port).to_socket_addrs().map(Iterator::collect)
    })
}

fn validate_mobile_config_with<F>(config: &MobileGatewayConfig, mut resolve: F) -> Result<()>
where
    F: FnMut((&str, u16)) -> io::Result<Vec<SocketAddr>>,
{
    if config.bind.port() == 0 {
        bail!("mobile gateway bind port must be non-zero");
    }
    if !is_private_mobile_address(config.bind.ip()) {
        bail!("mobile gateway bind address must be a concrete private-network address");
    }

    let endpoint = Url::parse(&config.advertised_endpoint)
        .context("mobile gateway endpoint must be a valid URL")?;
    if endpoint.scheme() != "https" {
        bail!("mobile gateway endpoint must use HTTPS");
    }
    if !endpoint.username().is_empty() || endpoint.password().is_some() {
        bail!("mobile gateway endpoint must not contain credentials");
    }
    if endpoint.query().is_some() || endpoint.fragment().is_some() {
        bail!("mobile gateway endpoint must not contain a query or fragment");
    }
    if endpoint.path() != "/" {
        bail!("mobile gateway endpoint must not contain a path");
    }

    let endpoint_port = endpoint
        .port_or_known_default()
        .context("mobile gateway endpoint must contain a port")?;
    if endpoint_port != config.bind.port() {
        bail!("mobile gateway endpoint port must match the bind port");
    }

    match endpoint
        .host()
        .context("mobile gateway endpoint must contain a host")?
    {
        Host::Ipv4(ip) => validate_endpoint_ip(IpAddr::V4(ip), config.bind.ip())?,
        Host::Ipv6(ip) => validate_endpoint_ip(IpAddr::V6(ip), config.bind.ip())?,
        Host::Domain(host) => {
            let addresses = resolve((host, endpoint_port))
                .with_context(|| format!("failed to resolve mobile gateway endpoint `{host}`"))?;
            if addresses.is_empty() {
                bail!("mobile gateway endpoint hostname did not resolve");
            }
            if addresses
                .iter()
                .any(|address| !is_private_mobile_address(address.ip()))
            {
                bail!("mobile gateway endpoint hostname resolved outside the private network");
            }
            if !addresses
                .iter()
                .any(|address| address.ip() == config.bind.ip())
            {
                bail!("mobile gateway endpoint hostname does not resolve to the bind address");
            }
        }
    }

    Ok(())
}

fn validate_endpoint_ip(endpoint: IpAddr, bind: IpAddr) -> Result<()> {
    if !is_private_mobile_address(endpoint) {
        bail!("mobile gateway endpoint must be private-network scoped");
    }
    if endpoint != bind {
        bail!("mobile gateway endpoint address must match the bind address");
    }
    Ok(())
}

fn is_private_mobile_address(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => {
            address.is_private() || address.is_link_local() || is_tailnet_address(address)
        }
        IpAddr::V6(address) => is_unique_local(address) || address.is_unicast_link_local(),
    }
}

#[cfg(test)]
pub(crate) fn is_private_mobile_address_for_test(address: IpAddr) -> bool {
    is_private_mobile_address(address)
}

fn is_tailnet_address(address: Ipv4Addr) -> bool {
    let octets = address.octets();
    octets[0] == 100 && (64..=127).contains(&octets[1])
}

fn is_unique_local(address: Ipv6Addr) -> bool {
    address.octets()[0] & 0xfe == 0xfc
}

pub fn load_mobile_config(coven_home: &Path) -> Result<Option<MobileGatewayConfig>> {
    let config = load_mobile_config_unvalidated(coven_home)?;
    if let Some(config) = &config {
        validate_mobile_config(config)?;
    }
    Ok(config)
}

pub(crate) fn load_mobile_config_unvalidated(
    coven_home: &Path,
) -> Result<Option<MobileGatewayConfig>> {
    let mobile_dir = coven_home.join(MOBILE_STATE_DIR);
    match fs::symlink_metadata(&mobile_dir) {
        Ok(_) => validate_private_directory(&mobile_dir)?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("failed to inspect {}", mobile_dir.display()));
        }
    }

    let path = mobile_dir.join(GATEWAY_CONFIG_FILE);
    match fs::symlink_metadata(&path) {
        Ok(_) => validate_private_file(&path)?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| format!("failed to inspect {}", path.display()));
        }
    }

    let bytes = fs::read(&path).with_context(|| format!("failed to read {}", path.display()))?;
    let config: MobileGatewayConfig = serde_json::from_slice(&bytes)
        .with_context(|| format!("failed to parse {}", path.display()))?;
    Ok(Some(config))
}

pub fn save_mobile_config(coven_home: &Path, config: &MobileGatewayConfig) -> Result<()> {
    validate_mobile_config(config)?;
    let mobile_dir = ensure_private_mobile_dir(coven_home)?;
    let mut encoded =
        serde_json::to_vec_pretty(config).context("failed to encode mobile gateway config")?;
    encoded.push(b'\n');
    atomic_replace_private(&mobile_dir.join(GATEWAY_CONFIG_FILE), &encoded)
}

pub fn remove_mobile_config(coven_home: &Path) -> Result<bool> {
    let mobile_dir = coven_home.join(MOBILE_STATE_DIR);
    match fs::symlink_metadata(&mobile_dir) {
        Ok(_) => validate_private_directory(&mobile_dir)?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("failed to inspect {}", mobile_dir.display()));
        }
    }
    let path = mobile_dir.join(GATEWAY_CONFIG_FILE);
    match fs::symlink_metadata(&path) {
        Ok(_) => validate_private_file(&path)?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(error).with_context(|| format!("failed to inspect {}", path.display()));
        }
    }
    fs::remove_file(&path).with_context(|| format!("failed to remove {}", path.display()))?;
    sync_directory(&mobile_dir)?;
    Ok(true)
}

pub(crate) fn ensure_private_mobile_dir(coven_home: &Path) -> Result<PathBuf> {
    let path = coven_home.join(MOBILE_STATE_DIR);
    match fs::symlink_metadata(&path) {
        Ok(_) => validate_private_directory(&path)?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            fs::create_dir(&path)
                .with_context(|| format!("failed to create {}", path.display()))?;
            set_private_directory_permissions(&path)?;
            validate_private_directory(&path)?;
        }
        Err(error) => {
            return Err(error).with_context(|| format!("failed to inspect {}", path.display()));
        }
    }
    Ok(path)
}

pub(crate) fn validate_private_file(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("failed to inspect {}", path.display()))?;
    if metadata.file_type().is_symlink()
        || is_windows_reparse_point(&metadata)
        || !metadata.is_file()
    {
        bail!(
            "mobile state file is not a regular, non-symlink file: {}",
            path.display()
        );
    }
    validate_owner_and_mode(path, &metadata, 0o600)
}

pub(crate) fn validate_private_directory(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("failed to inspect {}", path.display()))?;
    if metadata.file_type().is_symlink()
        || is_windows_reparse_point(&metadata)
        || !metadata.is_dir()
    {
        bail!(
            "mobile state directory is not a real directory: {}",
            path.display()
        );
    }
    validate_owner_and_mode(path, &metadata, 0o700)
}

#[cfg(unix)]
fn validate_owner_and_mode(path: &Path, metadata: &fs::Metadata, expected_mode: u32) -> Result<()> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    if metadata.uid() != unsafe { libc::geteuid() } {
        bail!(
            "mobile state is not owned by the current user: {}",
            path.display()
        );
    }
    let mode = metadata.permissions().mode() & 0o777;
    if mode != expected_mode {
        bail!(
            "mobile state has insecure permissions {:o}; expected {:o}: {}",
            mode,
            expected_mode,
            path.display()
        );
    }
    Ok(())
}

#[cfg(not(unix))]
fn validate_owner_and_mode(
    _path: &Path,
    _metadata: &fs::Metadata,
    _expected_mode: u32,
) -> Result<()> {
    Ok(())
}

#[cfg(unix)]
fn set_private_directory_permissions(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .with_context(|| format!("failed to secure {}", path.display()))
}

#[cfg(not(unix))]
fn set_private_directory_permissions(_path: &Path) -> Result<()> {
    Ok(())
}

/// Bound a source component before parsing it while its store lock is held.
/// Absence is explicit to the caller; this never creates an empty authority file.
pub(crate) fn validate_restore_export_component(path: &Path) -> Result<bool> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.into()),
    };
    validate_private_file(path)?;
    if metadata.len() > 4 * 1024 * 1024 {
        bail!("private authority component exceeds restore export limit");
    }
    Ok(true)
}

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MobileWritePhase {
    TempCreated,
    Written,
    FileSynced,
    Renamed,
    DirectorySynced,
    RetainedParentSynced,
    RetainedHomeSynced,
}
#[cfg(test)]
type WriteObserver = Box<dyn FnMut(&Path, MobileWritePhase) -> Result<()>>;
#[cfg(test)]
thread_local! {
    static WRITE_OBSERVER: std::cell::RefCell<Option<WriteObserver>> = const { std::cell::RefCell::new(None) };
}
#[cfg(test)]
pub(crate) fn set_write_observer(observer: WriteObserver) {
    WRITE_OBSERVER.with(|slot| *slot.borrow_mut() = Some(observer));
}
#[cfg(test)]
pub(crate) fn write_checkpoint(path: &Path, phase: MobileWritePhase) -> Result<()> {
    held_write::checkpoint(path, phase);
    WRITE_OBSERVER.with(|slot| {
        if let Some(observer) = slot.borrow_mut().as_mut() {
            observer(path, phase)
        } else {
            Ok(())
        }
    })
}

/// Create complete private authority without replacing an existing final name.
/// Staging and no-clobber publication prevent a killed writer from leaving a
/// truncated canonical authority file. Namespace durability retains the mobile
/// helper's existing platform contract; v2 adds its checked retained barriers.
pub(crate) fn atomic_create_private(path: &Path, bytes: &[u8]) -> Result<bool> {
    let parent = path
        .parent()
        .context("mobile state file must have a parent directory")?;
    validate_private_directory(parent)?;
    match fs::symlink_metadata(path) {
        // Preserve existing-final semantics: do not open, flush, repair or
        // require new write access. Owning callers validate/read the winner;
        // v2 retains its explicit checked durability barriers.
        Ok(_) => return Ok(false),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(error).with_context(|| format!("failed to inspect {}", path.display()))
        }
    }
    let staged = parent.join(format!(".mobile-state-{}.tmp", Uuid::new_v4()));
    let result = (|| {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(&staged)
            .with_context(|| format!("failed to stage {}", path.display()))?;
        #[cfg(test)]
        write_checkpoint(path, MobileWritePhase::TempCreated)?;
        write_and_sync(&mut file, &staged, bytes, path)?;
        drop(file);
        validate_private_file(&staged)?;
        // Reuse the repo-supported NOREPLACE / MoveFileExW no-replace shape.
        // Ordinary rename() could overwrite a concurrent valid authority file.
        let created = publish_private_noclobber(&staged, path)
            .with_context(|| format!("failed to publish new {}", path.display()))?;
        #[cfg(test)]
        if created {
            write_checkpoint(path, MobileWritePhase::Renamed)?;
        }
        if !created {
            // An atomic concurrent winner remains untouched. An orphan of this
            // unique staging file is non-authoritative if cleanup itself fails.
            let _ = fs::remove_file(&staged);
            return Ok(false);
        }
        sync_directory(parent)?;
        #[cfg(test)]
        write_checkpoint(path, MobileWritePhase::DirectorySynced)?;
        validate_private_file(path)?;
        Ok(created)
    })();
    if result.is_err() {
        // Never delete/repair a final floor, including malformed or uncertain
        // authority. Its owning store must fail closed and require recovery.
        let _ = fs::remove_file(&staged);
    }
    result
}

#[cfg(any(
    target_vendor = "apple",
    target_os = "linux",
    target_os = "android",
    target_os = "redox"
))]
fn publish_private_noclobber(staged: &Path, destination: &Path) -> io::Result<bool> {
    use rustix::fs::{renameat_with, RenameFlags, CWD};
    match renameat_with(CWD, staged, CWD, destination, RenameFlags::NOREPLACE) {
        Ok(()) => Ok(true),
        Err(error) if error == rustix::io::Errno::EXIST => Ok(false),
        Err(error) => Err(error.into()),
    }
}

#[cfg(windows)]
fn publish_private_noclobber(staged: &Path, destination: &Path) -> io::Result<bool> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::{ERROR_ALREADY_EXISTS, ERROR_FILE_EXISTS};
    use windows_sys::Win32::Storage::FileSystem::{MoveFileExW, MOVEFILE_WRITE_THROUGH};
    fn wide(path: &Path) -> io::Result<Vec<u16>> {
        let mut encoded = path.as_os_str().encode_wide().collect::<Vec<_>>();
        if encoded.contains(&0) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "mobile publication path contains NUL",
            ));
        }
        encoded.push(0);
        Ok(encoded)
    }
    let staged = wide(staged)?;
    let destination = wide(destination)?;
    // No MOVEFILE_REPLACE_EXISTING: an existing floor must never be clobbered.
    // SAFETY: both live vectors contain NUL-terminated native paths.
    if unsafe {
        MoveFileExW(
            staged.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_WRITE_THROUGH,
        )
    } != 0
    {
        return Ok(true);
    }
    let error = io::Error::last_os_error();
    match error.raw_os_error().map(|code| code as u32) {
        Some(ERROR_ALREADY_EXISTS | ERROR_FILE_EXISTS) => Ok(false),
        _ => Err(error),
    }
}

#[cfg(not(any(
    target_vendor = "apple",
    target_os = "linux",
    target_os = "android",
    target_os = "redox",
    windows
)))]
fn publish_private_noclobber(_staged: &Path, _destination: &Path) -> io::Result<bool> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "atomic mobile no-replace publication is unsupported",
    ))
}

pub(crate) fn atomic_replace_private(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .context("mobile state file must have a parent directory")?;
    validate_private_directory(parent)?;
    match fs::symlink_metadata(path) {
        Ok(_) => validate_private_file(path)?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(error).with_context(|| format!("failed to inspect {}", path.display()));
        }
    }

    let staged = parent.join(format!(".mobile-state-{}.tmp", Uuid::new_v4()));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&staged)
        .with_context(|| format!("failed to create {}", staged.display()))?;
    #[cfg(test)]
    if let Err(error) = write_checkpoint(path, MobileWritePhase::TempCreated) {
        let _ = fs::remove_file(&staged);
        return Err(error);
    }
    if let Err(error) = write_and_sync(&mut file, &staged, bytes, path) {
        let _ = fs::remove_file(&staged);
        return Err(error);
    }
    drop(file);
    if let Err(error) = fs::rename(&staged, path) {
        let _ = fs::remove_file(&staged);
        return Err(error).with_context(|| format!("failed to replace {}", path.display()));
    }
    #[cfg(test)]
    write_checkpoint(path, MobileWritePhase::Renamed)?;
    sync_directory(parent)?;
    #[cfg(test)]
    write_checkpoint(path, MobileWritePhase::DirectorySynced)?;
    validate_private_file(path)
}

fn write_and_sync(file: &mut File, path: &Path, bytes: &[u8], logical_target: &Path) -> Result<()> {
    #[cfg(not(test))]
    let _ = logical_target;
    file.write_all(bytes)
        .with_context(|| format!("failed to write {}", path.display()))?;
    #[cfg(test)]
    write_checkpoint(logical_target, MobileWritePhase::Written)?;
    file.sync_all()
        .with_context(|| format!("failed to sync {}", path.display()))?;
    #[cfg(test)]
    write_checkpoint(logical_target, MobileWritePhase::FileSynced)?;
    Ok(())
}

#[cfg(unix)]
pub(super) fn sync_directory(path: &Path) -> Result<()> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .with_context(|| format!("failed to sync {}", path.display()))
}

#[cfg(not(unix))]
pub(super) fn sync_directory(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(windows)]
fn is_windows_reparse_point(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;

    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn is_windows_reparse_point(_metadata: &fs::Metadata) -> bool {
    false
}

/// A path-scoped, test-only held storage phase. This models a call that has
/// not returned; it is not evidence of actual kernel or power-loss behavior.
#[cfg(test)]
pub(crate) mod held_write {
    use super::*;
    use std::collections::HashMap;
    use std::sync::{mpsc, Arc, Condvar, LazyLock, Mutex};
    struct State {
        phase: MobileWritePhase,
        skip: std::sync::atomic::AtomicUsize,
        ready: mpsc::SyncSender<()>,
        released: Mutex<bool>,
        wake: Condvar,
    }
    static HOLDS: LazyLock<Mutex<HashMap<PathBuf, Arc<State>>>> =
        LazyLock::new(|| Mutex::new(HashMap::new()));
    pub(crate) struct HeldWrite {
        path: PathBuf,
        state: Arc<State>,
    }
    impl Drop for HeldWrite {
        fn drop(&mut self) {
            HOLDS.lock().unwrap().remove(&self.path);
            *self.state.released.lock().unwrap() = true;
            self.state.wake.notify_all();
        }
    }
    /// Skip only matching real checkpoints. This can select compensation's
    /// second write without manufacturing an intermediate authority image.
    pub(crate) fn hold_after(
        path: &Path,
        phase: MobileWritePhase,
        skip: usize,
    ) -> (HeldWrite, mpsc::Receiver<()>) {
        let (ready, waiting) = mpsc::sync_channel(1);
        let state = Arc::new(State {
            phase,
            skip: std::sync::atomic::AtomicUsize::new(skip),
            ready,
            released: Mutex::new(false),
            wake: Condvar::new(),
        });
        assert!(HOLDS
            .lock()
            .unwrap()
            .insert(path.to_owned(), Arc::clone(&state))
            .is_none());
        (
            HeldWrite {
                path: path.to_owned(),
                state,
            },
            waiting,
        )
    }
    pub(super) fn checkpoint(path: &Path, phase: MobileWritePhase) {
        let state = HOLDS
            .lock()
            .unwrap()
            .get(path)
            .filter(|state| state.phase == phase)
            .cloned();
        if let Some(state) = state {
            if state
                .skip
                .try_update(
                    std::sync::atomic::Ordering::AcqRel,
                    std::sync::atomic::Ordering::Acquire,
                    |left| left.checked_sub(1),
                )
                .is_ok()
            {
                return;
            }
            let _ = state.ready.try_send(());
            let mut released = state.released.lock().unwrap();
            while !*released {
                released = state.wake.wait(released).unwrap();
            }
        }
    }

    #[test]
    fn skip_first_hold_selects_only_the_second_matching_checkpoint() {
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join("phase-cut");
        let (held, waiting) = hold_after(&path, MobileWritePhase::Written, 1);
        let (first_done, first_finished) = mpsc::sync_channel(1);
        let worker = std::thread::spawn(move || {
            checkpoint(&path, MobileWritePhase::FileSynced);
            checkpoint(&path, MobileWritePhase::Written);
            let _ = first_done.send(());
            checkpoint(&path, MobileWritePhase::Written);
        });
        let skipped = first_finished.recv_timeout(std::time::Duration::from_secs(30));
        let selected = waiting.recv_timeout(std::time::Duration::from_secs(30));
        drop(held);
        worker.join().unwrap();
        assert!(
            skipped.is_ok(),
            "first matching checkpoint did not finish: {skipped:?}"
        );
        assert!(
            selected.is_ok(),
            "second matching checkpoint was not held: {selected:?}"
        );
    }
}

#[cfg(test)]
#[path = "config_crash_create_tests.rs"]
mod crash_create_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};

    fn config(bind: &str, endpoint: &str) -> MobileGatewayConfig {
        MobileGatewayConfig {
            enabled: true,
            bind: bind.parse().unwrap(),
            advertised_endpoint: endpoint.to_owned(),
        }
    }

    #[test]
    fn mobile_config_rejects_wildcard_and_public_addresses() {
        for invalid in [
            config("0.0.0.0:7443", "https://0.0.0.0:7443"),
            config("[::]:7443", "https://[::]:7443"),
            config("127.0.0.1:7443", "https://127.0.0.1:7443"),
            config("203.0.113.10:7443", "https://203.0.113.10:7443"),
            config("192.168.1.10:0", "https://192.168.1.10"),
            config("192.168.1.10:7443", "http://192.168.1.10:7443"),
            config(
                "192.168.1.10:7443",
                "https://user@example.test:7443/path?query=yes#fragment",
            ),
        ] {
            assert!(validate_mobile_config_with(&invalid, |_| Ok(Vec::new())).is_err());
        }

        let mixed_name = config("192.168.1.10:7443", "https://coven-host.example.test:7443");
        assert!(validate_mobile_config_with(&mixed_name, |_| {
            Ok(vec![
                SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 10)), 7443),
                SocketAddr::new(IpAddr::V4(Ipv4Addr::new(203, 0, 113, 10)), 7443),
            ])
        })
        .is_err());
    }

    #[cfg(unix)]
    #[test]
    fn mobile_state_rejects_symlinked_directory_or_files() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        symlink(outside.path(), temp.path().join("mobile")).unwrap();
        assert!(ensure_private_mobile_dir(temp.path()).is_err());

        std::fs::remove_file(temp.path().join("mobile")).unwrap();
        let mobile = ensure_private_mobile_dir(temp.path()).unwrap();
        let target = outside.path().join("gateway.json");
        std::fs::write(&target, "{}").unwrap();
        symlink(&target, mobile.join("gateway.json")).unwrap();
        assert!(load_mobile_config(temp.path()).is_err());
    }

    #[test]
    fn gateway_is_disabled_when_config_is_absent() {
        let temp = tempfile::tempdir().unwrap();
        assert!(load_mobile_config(temp.path()).unwrap().is_none());
    }
}
