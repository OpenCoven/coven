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

pub(crate) fn atomic_create_private(path: &Path, bytes: &[u8]) -> Result<bool> {
    let parent = path
        .parent()
        .context("mobile state file must have a parent directory")?;
    validate_private_directory(parent)?;
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = match options.open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => return Ok(false),
        Err(error) => {
            return Err(error).with_context(|| format!("failed to create {}", path.display()));
        }
    };
    if let Err(error) = write_and_sync(&mut file, path, bytes) {
        let _ = fs::remove_file(path);
        return Err(error);
    }
    sync_directory(parent)?;
    validate_private_file(path)?;
    Ok(true)
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
    if let Err(error) = write_and_sync(&mut file, &staged, bytes) {
        let _ = fs::remove_file(&staged);
        return Err(error);
    }
    drop(file);
    if let Err(error) = fs::rename(&staged, path) {
        let _ = fs::remove_file(&staged);
        return Err(error).with_context(|| format!("failed to replace {}", path.display()));
    }
    sync_directory(parent)?;
    validate_private_file(path)
}

fn write_and_sync(file: &mut File, path: &Path, bytes: &[u8]) -> Result<()> {
    file.write_all(bytes)
        .with_context(|| format!("failed to write {}", path.display()))?;
    file.sync_all()
        .with_context(|| format!("failed to sync {}", path.display()))
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> Result<()> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .with_context(|| format!("failed to sync {}", path.display()))
}

#[cfg(not(unix))]
fn sync_directory(_path: &Path) -> Result<()> {
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
