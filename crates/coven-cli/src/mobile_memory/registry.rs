use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::RwLock;

use anyhow::{bail, Context, Result};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::config::{atomic_replace_private, ensure_private_mobile_dir, validate_private_file};
use super::contract::{MobileDeviceScope, MobilePairedDevice};
pub use super::grant::DeviceScope;
use super::grant::{AssuranceLevel, DeviceGrant};

pub const DEVICES_FILE: &str = "devices.json";
const DEVICE_REGISTRY_VERSION: u16 = 2;
const LEGACY_DEVICE_REGISTRY_VERSION: u16 = 1;
const MAX_DEVICE_RECORDS: usize = 128;
const MAX_DEVICE_NAME_CHARS: usize = 80;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct DeviceRecord {
    pub id: Uuid,
    pub display_name: String,
    pub public_key_x963: String,
    pub paired_at: DateTime<Utc>,
    pub revoked_at: Option<DateTime<Utc>>,
    pub scopes: Vec<DeviceScope>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct GrantedDeviceRecord {
    device: DeviceRecord,
    grant: DeviceGrant,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceAuthorizationRecord {
    pub device: DeviceRecord,
    pub grant: DeviceGrant,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct StoredDeviceRegistry {
    version: u16,
    devices: Vec<GrantedDeviceRecord>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct LegacyStoredDeviceRegistry {
    version: u16,
    devices: Vec<LegacyDeviceRecord>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct LegacyDeviceRecord {
    id: Uuid,
    display_name: String,
    public_key_x963: String,
    paired_at: DateTime<Utc>,
    revoked_at: Option<DateTime<Utc>>,
    scopes: Vec<LegacyDeviceScope>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum LegacyDeviceScope {
    MemoryRead,
}

struct LoadedRegistry {
    devices: Vec<GrantedDeviceRecord>,
    migrated: bool,
}

pub struct DeviceRegistry {
    path: PathBuf,
    devices: RwLock<Vec<GrantedDeviceRecord>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceStatusRecord {
    pub id: Uuid,
    pub display_name: String,
    pub paired_at: DateTime<Utc>,
    pub revoked_at: Option<DateTime<Utc>>,
    pub scopes: Vec<DeviceScope>,
    pub grant_id: Uuid,
    pub minimum_assurance: AssuranceLevel,
    pub expires_at: Option<DateTime<Utc>>,
    pub revocation_epoch: u64,
}

impl DeviceRegistry {
    pub fn load(coven_home: &Path) -> Result<Self> {
        let mobile_dir = ensure_private_mobile_dir(coven_home)?;
        let path = mobile_dir.join(DEVICES_FILE);
        let loaded = read_registry(&path)?;
        if loaded.migrated {
            write_registry(&path, &loaded.devices)?;
        }
        Ok(Self {
            path,
            devices: RwLock::new(loaded.devices),
        })
    }

    pub fn load_if_present(coven_home: &Path) -> Result<Option<Self>> {
        match fs::symlink_metadata(coven_home.join(super::config::MOBILE_STATE_DIR)) {
            Ok(_) => Self::load(coven_home).map(Some),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error).context("failed to inspect mobile state directory"),
        }
    }

    pub fn reload(&self) -> Result<()> {
        let loaded = read_registry(&self.path)?;
        if loaded.migrated {
            write_registry(&self.path, &loaded.devices)?;
        }
        *self
            .devices
            .write()
            .map_err(|_| anyhow::anyhow!("mobile device registry lock poisoned"))? = loaded.devices;
        Ok(())
    }

    pub fn register(&self, record: DeviceRecord) -> Result<()> {
        let grant = DeviceGrant::for_device(
            record.id,
            &record.public_key_x963,
            record.scopes.clone(),
            record.paired_at,
        )
        .context("failed to issue mobile device grant")?;
        self.register_with_grant(record, grant)
    }

    pub fn register_with_grant(&self, record: DeviceRecord, grant: DeviceGrant) -> Result<()> {
        let granted = GrantedDeviceRecord {
            device: record,
            grant,
        };
        validate_granted_device(&granted)?;
        let mut devices = self
            .devices
            .write()
            .map_err(|_| anyhow::anyhow!("mobile device registry lock poisoned"))?;
        if devices.len() >= MAX_DEVICE_RECORDS {
            bail!("mobile device registry is full");
        }
        if devices
            .iter()
            .any(|existing| existing.device.id == granted.device.id)
        {
            bail!("mobile device id is already registered");
        }
        if devices
            .iter()
            .any(|existing| existing.device.public_key_x963 == granted.device.public_key_x963)
        {
            bail!("mobile device public key is already registered");
        }
        if devices
            .iter()
            .any(|existing| existing.grant.id == granted.grant.id)
        {
            bail!("mobile device grant id is already registered");
        }
        let mut updated = devices.clone();
        updated.push(granted);
        validate_devices(&updated)?;
        write_registry(&self.path, &updated)?;
        *devices = updated;
        Ok(())
    }

    pub fn replace_grant(&self, device_id: Uuid, grant: DeviceGrant) -> Result<()> {
        let mut devices = self
            .devices
            .write()
            .map_err(|_| anyhow::anyhow!("mobile device registry lock poisoned"))?;
        let mut updated = devices.clone();
        let index = updated
            .iter()
            .position(|record| record.device.id == device_id)
            .context("mobile device is not registered")?;
        grant
            .validate(&updated[index].device.public_key_x963)
            .context("replacement mobile device grant is invalid")?;
        if grant.revocation_epoch < updated[index].grant.revocation_epoch {
            bail!("mobile device grant revocation epoch cannot decrease");
        }
        if updated
            .iter()
            .enumerate()
            .any(|(other, record)| other != index && record.grant.id == grant.id)
        {
            bail!("mobile device grant id is already registered");
        }
        updated[index].device.scopes = grant.scopes.clone();
        updated[index].grant = grant;
        validate_devices(&updated)?;
        write_registry(&self.path, &updated)?;
        *devices = updated;
        Ok(())
    }

    pub fn rename(&self, device_id: Uuid, display_name: String) -> Result<()> {
        let mut devices = self
            .devices
            .write()
            .map_err(|_| anyhow::anyhow!("mobile device registry lock poisoned"))?;
        let mut updated = devices.clone();
        let device = updated
            .iter_mut()
            .find(|record| record.device.id == device_id)
            .context("mobile device is not registered")?;
        device.device.display_name = display_name;
        validate_devices(&updated)?;
        write_registry(&self.path, &updated)?;
        *devices = updated;
        Ok(())
    }

    pub fn revoke(&self, device_id: Uuid, revoked_at: DateTime<Utc>) -> Result<()> {
        let mut devices = self
            .devices
            .write()
            .map_err(|_| anyhow::anyhow!("mobile device registry lock poisoned"))?;
        let mut updated = devices.clone();
        let record = updated
            .iter_mut()
            .find(|record| record.device.id == device_id)
            .context("mobile device is not registered")?;
        if record.device.revoked_at.is_none() {
            record.device.revoked_at = Some(revoked_at);
            record.grant.revocation_epoch = record
                .grant
                .revocation_epoch
                .checked_add(1)
                .context("mobile device revocation epoch overflow")?;
        }
        validate_devices(&updated)?;
        write_registry(&self.path, &updated)?;
        *devices = updated;
        Ok(())
    }

    pub fn forget_all(&self) -> Result<()> {
        let mut devices = self
            .devices
            .write()
            .map_err(|_| anyhow::anyhow!("mobile device registry lock poisoned"))?;
        write_registry(&self.path, &[])?;
        devices.clear();
        Ok(())
    }

    pub fn device(&self, device_id: Uuid) -> Result<Option<DeviceRecord>> {
        Ok(self
            .devices
            .read()
            .map_err(|_| anyhow::anyhow!("mobile device registry lock poisoned"))?
            .iter()
            .find(|record| record.device.id == device_id)
            .map(|record| record.device.clone()))
    }

    pub fn authorization_record(
        &self,
        device_id: Uuid,
    ) -> Result<Option<DeviceAuthorizationRecord>> {
        Ok(self
            .devices
            .read()
            .map_err(|_| anyhow::anyhow!("mobile device registry lock poisoned"))?
            .iter()
            .find(|record| record.device.id == device_id)
            .map(|record| DeviceAuthorizationRecord {
                device: record.device.clone(),
                grant: record.grant.clone(),
            }))
    }

    pub fn active_device(&self, device_id: Uuid) -> Result<Option<DeviceRecord>> {
        let now = Utc::now();
        Ok(self
            .authorization_record(device_id)?
            .filter(|record| record.device.revoked_at.is_none())
            .filter(|record| {
                record
                    .grant
                    .authorize(None, AssuranceLevel::Possession, now)
                    .is_ok()
            })
            .map(|record| record.device))
    }

    pub fn list_redacted(&self) -> Result<Vec<MobilePairedDevice>> {
        Ok(self
            .devices
            .read()
            .map_err(|_| anyhow::anyhow!("mobile device registry lock poisoned"))?
            .iter()
            .map(|record| MobilePairedDevice {
                id: record.device.id,
                display_name: record.device.display_name.clone(),
                paired_at: record.device.paired_at,
                scopes: record
                    .grant
                    .scopes
                    .iter()
                    .filter_map(|scope| match scope {
                        DeviceScope::MemoryRead => Some(MobileDeviceScope::MemoryRead),
                        _ => None,
                    })
                    .collect(),
            })
            .collect())
    }

    pub fn list_status(&self) -> Result<Vec<DeviceStatusRecord>> {
        Ok(self
            .devices
            .read()
            .map_err(|_| anyhow::anyhow!("mobile device registry lock poisoned"))?
            .iter()
            .map(|record| DeviceStatusRecord {
                id: record.device.id,
                display_name: record.device.display_name.clone(),
                paired_at: record.device.paired_at,
                revoked_at: record.device.revoked_at,
                scopes: record.grant.scopes.clone(),
                grant_id: record.grant.id,
                minimum_assurance: record.grant.minimum_assurance,
                expires_at: record.grant.expires_at,
                revocation_epoch: record.grant.revocation_epoch,
            })
            .collect())
    }
}

fn read_registry(path: &Path) -> Result<LoadedRegistry> {
    match fs::symlink_metadata(path) {
        Ok(_) => validate_private_file(path)?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(LoadedRegistry {
                devices: Vec::new(),
                migrated: false,
            });
        }
        Err(error) => {
            return Err(error).with_context(|| format!("failed to inspect {}", path.display()));
        }
    }
    let bytes = fs::read(path).with_context(|| format!("failed to read {}", path.display()))?;
    let value: serde_json::Value = serde_json::from_slice(&bytes)
        .with_context(|| format!("failed to parse {}", path.display()))?;
    let version = value
        .get("version")
        .and_then(serde_json::Value::as_u64)
        .context("mobile device registry omitted a numeric version")?;
    let version = u16::try_from(version).context("mobile device registry version is out of range")?;

    let loaded = match version {
        LEGACY_DEVICE_REGISTRY_VERSION => {
            let stored: LegacyStoredDeviceRegistry = serde_json::from_value(value)
                .with_context(|| format!("failed to parse {}", path.display()))?;
            if stored.version != LEGACY_DEVICE_REGISTRY_VERSION {
                bail!("unsupported mobile device registry version");
            }
            LoadedRegistry {
                devices: stored
                    .devices
                    .into_iter()
                    .map(migrate_legacy_device)
                    .collect::<Result<Vec<_>>>()?,
                migrated: true,
            }
        }
        DEVICE_REGISTRY_VERSION => {
            let stored: StoredDeviceRegistry = serde_json::from_value(value)
                .with_context(|| format!("failed to parse {}", path.display()))?;
            if stored.version != DEVICE_REGISTRY_VERSION {
                bail!("unsupported mobile device registry version");
            }
            LoadedRegistry {
                devices: stored.devices,
                migrated: false,
            }
        }
        _ => bail!("unsupported mobile device registry version"),
    };
    validate_devices(&loaded.devices)?;
    Ok(loaded)
}

fn migrate_legacy_device(record: LegacyDeviceRecord) -> Result<GrantedDeviceRecord> {
    if record.scopes != [LegacyDeviceScope::MemoryRead] {
        bail!("legacy mobile device must have exactly the memory_read scope");
    }
    let device = DeviceRecord {
        id: record.id,
        display_name: record.display_name,
        public_key_x963: record.public_key_x963,
        paired_at: record.paired_at,
        revoked_at: record.revoked_at,
        scopes: vec![DeviceScope::MemoryRead],
    };
    let mut grant = DeviceGrant::for_device(
        device.id,
        &device.public_key_x963,
        device.scopes.clone(),
        device.paired_at,
    )
    .context("failed to migrate legacy mobile device grant")?;
    if device.revoked_at.is_some() {
        grant.revocation_epoch = 1;
    }
    Ok(GrantedDeviceRecord { device, grant })
}

fn write_registry(path: &Path, devices: &[GrantedDeviceRecord]) -> Result<()> {
    validate_devices(devices)?;
    let stored = StoredDeviceRegistry {
        version: DEVICE_REGISTRY_VERSION,
        devices: devices.to_vec(),
    };
    let mut encoded =
        serde_json::to_vec_pretty(&stored).context("failed to encode mobile device registry")?;
    encoded.push(b'\n');
    atomic_replace_private(path, &encoded)
}

fn validate_devices(devices: &[GrantedDeviceRecord]) -> Result<()> {
    if devices.len() > MAX_DEVICE_RECORDS {
        bail!("mobile device registry exceeds the record limit");
    }
    for (index, record) in devices.iter().enumerate() {
        validate_granted_device(record)?;
        if devices[..index]
            .iter()
            .any(|existing| existing.device.id == record.device.id)
        {
            bail!("mobile device registry contains duplicate ids");
        }
        if devices[..index]
            .iter()
            .any(|existing| existing.device.public_key_x963 == record.device.public_key_x963)
        {
            bail!("mobile device registry contains duplicate public keys");
        }
        if devices[..index]
            .iter()
            .any(|existing| existing.grant.id == record.grant.id)
        {
            bail!("mobile device registry contains duplicate grant ids");
        }
    }
    Ok(())
}

fn validate_granted_device(record: &GrantedDeviceRecord) -> Result<()> {
    validate_device(&record.device)?;
    record
        .grant
        .validate(&record.device.public_key_x963)
        .context("mobile device grant is invalid")?;
    if record.device.scopes != record.grant.scopes {
        bail!("mobile device scopes do not match its grant");
    }
    if record.device.revoked_at.is_some() && record.grant.revocation_epoch == 0 {
        bail!("revoked mobile device must advance its grant revocation epoch");
    }
    Ok(())
}

fn validate_device(device: &DeviceRecord) -> Result<()> {
    let name = device.display_name.as_str();
    let name_chars = name.chars().count();
    if name.is_empty()
        || name.trim() != name
        || name_chars > MAX_DEVICE_NAME_CHARS
        || name.chars().any(char::is_control)
    {
        bail!("mobile device display name is invalid");
    }
    let public_key = URL_SAFE_NO_PAD
        .decode(&device.public_key_x963)
        .context("mobile device public key is not valid base64url")?;
    if public_key.len() != 65
        || public_key.first() != Some(&4)
        || URL_SAFE_NO_PAD.encode(&public_key) != device.public_key_x963
        || p256::PublicKey::from_sec1_bytes(&public_key).is_err()
    {
        bail!("mobile device public key is not a canonical P-256 X9.63 key");
    }
    super::grant::validate_scope_set(&device.scopes)
        .context("mobile device scope set is invalid")?;
    if device
        .revoked_at
        .is_some_and(|revoked_at| revoked_at < device.paired_at)
    {
        bail!("mobile device revocation cannot predate pairing");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use p256::elliptic_curve::sec1::ToEncodedPoint;

    fn device(id: Uuid, name: &str) -> DeviceRecord {
        let mut scalar = [1_u8; 32];
        for (target, source) in scalar[16..].iter_mut().zip(id.as_bytes()) {
            *target ^= source;
        }
        let signing_key = p256::SecretKey::from_slice(&scalar).unwrap();
        DeviceRecord {
            id,
            display_name: name.to_owned(),
            public_key_x963: URL_SAFE_NO_PAD
                .encode(signing_key.public_key().to_encoded_point(false).as_bytes()),
            paired_at: Utc::now(),
            revoked_at: None,
            scopes: vec![DeviceScope::MemoryRead],
        }
    }

    #[test]
    fn legacy_registry_migrates_atomically_to_grants() {
        let temp = tempfile::tempdir().unwrap();
        let mobile = ensure_private_mobile_dir(temp.path()).unwrap();
        let path = mobile.join(DEVICES_FILE);
        let record = device(Uuid::from_u128(7), "Synthetic phone");
        let legacy = serde_json::json!({
            "version": 1,
            "devices": [{
                "id": record.id,
                "displayName": record.display_name,
                "publicKeyX963": record.public_key_x963,
                "pairedAt": record.paired_at,
                "revokedAt": null,
                "scopes": ["memory_read"]
            }]
        });
        atomic_replace_private(
            &path,
            format!("{}\n", serde_json::to_string_pretty(&legacy).unwrap()).as_bytes(),
        )
        .unwrap();

        let registry = DeviceRegistry::load(temp.path()).unwrap();
        let authorized = registry.authorization_record(record.id).unwrap().unwrap();
        assert_eq!(authorized.grant.scopes, [DeviceScope::MemoryRead]);
        assert_eq!(authorized.grant.revocation_epoch, 0);
        let migrated: serde_json::Value =
            serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        assert_eq!(migrated["version"], DEVICE_REGISTRY_VERSION);
        assert!(migrated["devices"][0].get("grant").is_some());
    }

    #[test]
    fn revoked_device_advances_epoch_and_never_authenticates_after_reload() {
        let temp = tempfile::tempdir().unwrap();
        let first = DeviceRegistry::load(temp.path()).unwrap();
        let record = device(Uuid::new_v4(), "Synthetic phone");
        first.register(record.clone()).unwrap();
        let original_epoch = first
            .authorization_record(record.id)
            .unwrap()
            .unwrap()
            .grant
            .revocation_epoch;

        let second = DeviceRegistry::load(temp.path()).unwrap();
        second.revoke(record.id, Utc::now()).unwrap();
        first.reload().unwrap();

        assert!(first.active_device(record.id).unwrap().is_none());
        let revoked = first.authorization_record(record.id).unwrap().unwrap();
        assert!(revoked.device.revoked_at.is_some());
        assert_eq!(revoked.grant.revocation_epoch, original_epoch + 1);
    }

    #[test]
    fn registry_rejects_duplicate_public_keys_ids_and_grants() {
        let temp = tempfile::tempdir().unwrap();
        let registry = DeviceRegistry::load(temp.path()).unwrap();
        let first = device(Uuid::new_v4(), "Synthetic phone");
        registry.register(first.clone()).unwrap();

        let duplicate_id = device(first.id, "Other synthetic phone");
        assert!(registry.register(duplicate_id).is_err());
        let mut duplicate_key = device(Uuid::new_v4(), "Other synthetic phone");
        duplicate_key.public_key_x963 = first.public_key_x963.clone();
        assert!(registry.register(duplicate_key).is_err());

        let second = device(Uuid::new_v4(), "Second synthetic phone");
        let mut grant = DeviceGrant::for_device(
            second.id,
            &second.public_key_x963,
            second.scopes.clone(),
            second.paired_at,
        )
        .unwrap();
        grant.id = registry
            .authorization_record(first.id)
            .unwrap()
            .unwrap()
            .grant
            .id;
        assert!(registry.register_with_grant(second, grant).is_err());
    }

    #[test]
    fn registry_corruption_fails_closed_without_overwrite() {
        let temp = tempfile::tempdir().unwrap();
        let mobile = ensure_private_mobile_dir(temp.path()).unwrap();
        let path = mobile.join(DEVICES_FILE);
        atomic_replace_private(&path, b"{not valid json}\n").unwrap();
        let before = std::fs::read(&path).unwrap();

        assert!(DeviceRegistry::load(temp.path()).is_err());
        assert_eq!(std::fs::read(path).unwrap(), before);
    }

    #[test]
    fn device_status_output_omits_public_and_subject_keys() {
        let temp = tempfile::tempdir().unwrap();
        let registry = DeviceRegistry::load(temp.path()).unwrap();
        let record = device(Uuid::new_v4(), "Synthetic phone");
        registry.register(record.clone()).unwrap();

        let encoded = serde_json::to_value(registry.list_status().unwrap()).unwrap();
        assert_eq!(encoded[0]["id"], record.id.to_string());
        assert!(encoded[0].get("publicKeyX963").is_none());
        assert!(encoded[0].get("publicKey").is_none());
        assert!(encoded[0].get("subjectKeyId").is_none());
        assert!(encoded[0].get("grantId").is_some());
    }

    #[test]
    fn replacement_grant_cannot_decrease_revocation_epoch() {
        let temp = tempfile::tempdir().unwrap();
        let registry = DeviceRegistry::load(temp.path()).unwrap();
        let record = device(Uuid::new_v4(), "Synthetic phone");
        registry.register(record.clone()).unwrap();
        let mut grant = registry
            .authorization_record(record.id)
            .unwrap()
            .unwrap()
            .grant;
        grant.revocation_epoch = 2;
        registry.replace_grant(record.id, grant.clone()).unwrap();
        grant.revocation_epoch = 1;
        assert!(registry.replace_grant(record.id, grant).is_err());
    }
}
