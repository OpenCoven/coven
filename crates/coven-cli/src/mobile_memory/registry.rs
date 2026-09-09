use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex, MutexGuard, RwLock};

use anyhow::{bail, Context, Result};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use chrono::{DateTime, Utc};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::assurance::{
    AuthorizationKeyRegistry, DeviceAuthorizationKeyRecord, NewAuthorizationKey,
};
use super::audit::{
    AuditDeliveryReceipt, DeviceGrantReissueAuditTransition, DeviceRotationAuditTransition,
};
use super::config::{atomic_replace_private, ensure_private_mobile_dir, validate_private_file};
use super::contract::{MobileDeviceScope, MobilePairedDevice};
pub use super::grant::DeviceScope;
use super::grant::{AssuranceLevel, DeviceGrant};

pub const DEVICES_FILE: &str = "devices.json";
const DEVICE_REGISTRY_VERSION: u16 = 2;
const LEGACY_DEVICE_REGISTRY_VERSION: u16 = 1;
const MAX_DEVICE_RECORDS: usize = 128;
const MAX_ROTATION_TRANSITIONS: usize = 128;
const MAX_GRANT_REISSUE_TRANSITIONS: usize = 128;
const MAX_DEVICE_NAME_CHARS: usize = 80;
const DEVICE_REGISTRY_LOCK_FILE: &str = ".devices.lock";
static DEVICE_REGISTRY_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

struct DeviceRegistryStoreLock {
    _process: MutexGuard<'static, ()>,
    file: fs::File,
}

impl DeviceRegistryStoreLock {
    fn acquire(path: &Path) -> Result<Self> {
        let process = DEVICE_REGISTRY_LOCK
            .lock()
            .map_err(|_| anyhow::anyhow!("mobile device registry process lock poisoned"))?;
        let parent = path
            .parent()
            .context("mobile device registry path has no parent")?;
        let lock_path = parent.join(DEVICE_REGISTRY_LOCK_FILE);
        let file = crate::state_lock::open_lock_file(&lock_path)?;
        file.lock_exclusive()
            .with_context(|| format!("failed to lock {}", lock_path.display()))?;
        Ok(Self {
            _process: process,
            file,
        })
    }
}

impl Drop for DeviceRegistryStoreLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DeviceGrantRevision {
    pub device_id: Uuid,
    pub grant_id: Uuid,
    pub revocation_epoch: u64,
}

impl From<&DeviceAuthorizationRecord> for DeviceGrantRevision {
    fn from(record: &DeviceAuthorizationRecord) -> Self {
        Self {
            device_id: record.device.id,
            grant_id: record.grant.id,
            revocation_epoch: record.grant.revocation_epoch,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DeviceRotationResult {
    pub replacement: DeviceAuthorizationRecord,
    pub transition: DeviceRotationAuditTransition,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DeviceGrantReissueResult {
    pub device: DeviceAuthorizationRecord,
    pub transition: DeviceGrantReissueAuditTransition,
}

pub(crate) struct DeviceGrantReissueRequest {
    pub expected: DeviceGrantRevision,
    pub scopes: Vec<DeviceScope>,
    pub restrictions: super::grant::DeviceGrantRestrictions,
    pub minimum_assurance: AssuranceLevel,
    pub expires_at: DateTime<Utc>,
    pub issued_at: DateTime<Utc>,
    pub requested_policy_digest: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct StoredDeviceRegistry {
    version: u16,
    devices: Vec<GrantedDeviceRecord>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    rotation_transitions: Vec<DeviceRotationAuditTransition>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    grant_reissue_transitions: Vec<DeviceGrantReissueAuditTransition>,
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
    rotation_transitions: Vec<DeviceRotationAuditTransition>,
    grant_reissue_transitions: Vec<DeviceGrantReissueAuditTransition>,
    migrated: bool,
}

pub struct DeviceRegistry {
    path: PathBuf,
    devices: RwLock<Vec<GrantedDeviceRecord>>,
    authorization_keys: AuthorizationKeyRegistry,
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
        let _store_lock = DeviceRegistryStoreLock::acquire(&path)?;
        let loaded = read_registry(&path)?;
        if loaded.migrated {
            write_registry(
                &path,
                &loaded.devices,
                &loaded.rotation_transitions,
                &loaded.grant_reissue_transitions,
            )?;
        }
        Ok(Self {
            path,
            devices: RwLock::new(loaded.devices),
            authorization_keys: AuthorizationKeyRegistry::load(coven_home)?,
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
        let _store_lock = DeviceRegistryStoreLock::acquire(&self.path)?;
        let loaded = read_registry(&self.path)?;
        if loaded.migrated {
            write_registry(
                &self.path,
                &loaded.devices,
                &loaded.rotation_transitions,
                &loaded.grant_reissue_transitions,
            )?;
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
        self.register_with_grant_and_authorization(record, grant, None)
    }

    pub(crate) fn register_with_grant_and_authorization(
        &self,
        record: DeviceRecord,
        grant: DeviceGrant,
        authorization_key: Option<NewAuthorizationKey>,
    ) -> Result<()> {
        let granted = GrantedDeviceRecord {
            device: record,
            grant,
        };
        validate_granted_device(&granted)?;
        let _store_lock = DeviceRegistryStoreLock::acquire(&self.path)?;
        let loaded = read_registry(&self.path)?;
        if loaded.devices.len() >= MAX_DEVICE_RECORDS {
            bail!("mobile device registry is full");
        }
        if loaded
            .devices
            .iter()
            .any(|existing| existing.device.id == granted.device.id)
        {
            bail!("mobile device id is already registered");
        }
        if loaded
            .devices
            .iter()
            .any(|existing| existing.device.public_key_x963 == granted.device.public_key_x963)
        {
            bail!("mobile device public key is already registered");
        }
        if loaded
            .devices
            .iter()
            .any(|existing| existing.grant.id == granted.grant.id)
        {
            bail!("mobile device grant id is already registered");
        }
        self.commit_registration(
            loaded.devices,
            loaded.rotation_transitions,
            loaded.grant_reissue_transitions,
            granted,
            authorization_key,
        )
    }

    fn commit_registration(
        &self,
        mut devices: Vec<GrantedDeviceRecord>,
        rotation_transitions: Vec<DeviceRotationAuditTransition>,
        grant_reissue_transitions: Vec<DeviceGrantReissueAuditTransition>,
        granted: GrantedDeviceRecord,
        authorization_key: Option<NewAuthorizationKey>,
    ) -> Result<()> {
        devices.push(granted);
        validate_devices(&devices)?;
        let enrolled_authorization = if let Some(key) = authorization_key {
            Some(
                self.authorization_keys.enroll_initial(
                    devices.last().expect("new device is present").device.id,
                    &devices
                        .last()
                        .expect("new device is present")
                        .device
                        .public_key_x963,
                    key,
                )?,
            )
        } else {
            None
        };
        if let Err(error) = write_registry(
            &self.path,
            &devices,
            &rotation_transitions,
            &grant_reissue_transitions,
        ) {
            if enrolled_authorization.is_some() {
                self.authorization_keys
                    .remove_device(devices.last().expect("new device is present").device.id)
                    .with_context(|| {
                        format!(
                            "failed to roll back authorization key after device commit failed: {error:#}"
                        )
                    })?;
            }
            return Err(error);
        }
        *self
            .devices
            .write()
            .map_err(|_| anyhow::anyhow!("mobile device registry lock poisoned"))? = devices;
        Ok(())
    }

    pub fn replace_grant(&self, device_id: Uuid, grant: DeviceGrant) -> Result<()> {
        let _store_lock = DeviceRegistryStoreLock::acquire(&self.path)?;
        let loaded = read_registry(&self.path)?;
        if loaded
            .grant_reissue_transitions
            .iter()
            .any(|transition| transition.device_id == device_id && transition.audited_at.is_none())
        {
            bail!("mobile device grant reissue audit delivery is pending");
        }
        let mut devices = self
            .devices
            .write()
            .map_err(|_| anyhow::anyhow!("mobile device registry lock poisoned"))?;
        let mut updated = loaded.devices;
        let index = updated
            .iter()
            .position(|record| record.device.id == device_id)
            .context("mobile device is not registered")?;
        grant
            .validate(&updated[index].device.public_key_x963)
            .context("replacement mobile device grant is invalid")?;
        if grant.id == updated[index].grant.id {
            bail!("replacement mobile device grant must use a fresh grant id");
        }
        if grant.revocation_epoch <= updated[index].grant.revocation_epoch {
            bail!("replacement mobile device grant must advance its revocation epoch");
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
        write_registry(
            &self.path,
            &updated,
            &loaded.rotation_transitions,
            &loaded.grant_reissue_transitions,
        )?;
        *devices = updated;
        Ok(())
    }

    pub(crate) fn reissue_grant_atomically(
        &self,
        request: DeviceGrantReissueRequest,
    ) -> Result<DeviceGrantReissueResult> {
        let _store_lock = DeviceRegistryStoreLock::acquire(&self.path)?;
        let loaded = read_registry(&self.path)?;
        if loaded.grant_reissue_transitions.iter().any(|transition| {
            transition.device_id == request.expected.device_id && transition.audited_at.is_none()
        }) {
            bail!("mobile device grant reissue audit delivery is pending");
        }
        let mut grant_reissue_transitions = loaded.grant_reissue_transitions.clone();
        if grant_reissue_transitions.len() >= MAX_GRANT_REISSUE_TRANSITIONS {
            if let Some(index) = grant_reissue_transitions
                .iter()
                .position(|transition| transition.audited_at.is_some())
            {
                grant_reissue_transitions.remove(index);
            } else {
                bail!("mobile device grant-reissue audit outbox is full");
            }
        }
        let mut devices = self
            .devices
            .write()
            .map_err(|_| anyhow::anyhow!("mobile device registry lock poisoned"))?;
        let mut updated = loaded.devices;
        let index = updated
            .iter()
            .position(|record| record.device.id == request.expected.device_id)
            .context("mobile device is not registered")?;
        let current = &updated[index];
        if current.grant.id != request.expected.grant_id
            || current.grant.revocation_epoch != request.expected.revocation_epoch
        {
            bail!("mobile device grant changed during reissue");
        }
        if current.device.revoked_at.is_some() {
            bail!("mobile device is revoked");
        }
        current
            .grant
            .authorize(None, current.grant.minimum_assurance, request.issued_at)
            .context("mobile device grant is not active")?;
        let replacement = current
            .grant
            .reissue(
                &current.device.public_key_x963,
                request.scopes,
                request.restrictions,
                request.minimum_assurance,
                request.issued_at,
                request.expires_at,
            )
            .context("replacement mobile device grant policy is invalid")?;
        let transition = DeviceGrantReissueAuditTransition {
            transition_id: Uuid::new_v4(),
            device_id: request.expected.device_id,
            requested_policy_digest: request.requested_policy_digest,
            previous_grant_id: current.grant.id,
            previous_revocation_epoch: current.grant.revocation_epoch,
            replacement_grant_id: replacement.id,
            replacement_revocation_epoch: replacement.revocation_epoch,
            occurred_at: request.issued_at,
            audited_at: None,
        };
        updated[index].device.scopes = replacement.scopes.clone();
        updated[index].grant = replacement;
        grant_reissue_transitions.push(transition.clone());
        write_registry(
            &self.path,
            &updated,
            &loaded.rotation_transitions,
            &grant_reissue_transitions,
        )?;
        let device = DeviceAuthorizationRecord {
            device: updated[index].device.clone(),
            grant: updated[index].grant.clone(),
        };
        *devices = updated;
        Ok(DeviceGrantReissueResult { device, transition })
    }

    pub(crate) fn rotate_devices_atomically(
        &self,
        source_revision: DeviceGrantRevision,
        replacement_revision: DeviceGrantRevision,
        rotated_at: DateTime<Utc>,
    ) -> Result<DeviceRotationResult> {
        if source_revision.device_id == replacement_revision.device_id {
            bail!("source and replacement mobile devices must differ");
        }
        let _store_lock = DeviceRegistryStoreLock::acquire(&self.path)?;
        let loaded = read_registry(&self.path)?;
        let mut devices = self
            .devices
            .write()
            .map_err(|_| anyhow::anyhow!("mobile device registry lock poisoned"))?;
        let mut updated = loaded.devices;
        let source_index = updated
            .iter()
            .position(|record| record.device.id == source_revision.device_id)
            .context("source mobile device is not registered")?;
        let replacement_index = updated
            .iter()
            .position(|record| record.device.id == replacement_revision.device_id)
            .context("replacement mobile device is not registered")?;
        let source = &updated[source_index];
        let replacement = &updated[replacement_index];
        if let Some(transition) = loaded.rotation_transitions.iter().rev().find(|transition| {
            transition.source_device_id == source_revision.device_id
                && transition.replacement_device_id == replacement_revision.device_id
                && source.device.revoked_at.is_some()
                && source.grant.id == transition.source_grant_id
                && source.grant.revocation_epoch == transition.source_revocation_epoch
                && replacement.grant.id == transition.replacement_grant_id
                && replacement.grant.revocation_epoch == transition.replacement_revocation_epoch
        }) {
            return Ok(DeviceRotationResult {
                replacement: DeviceAuthorizationRecord {
                    device: replacement.device.clone(),
                    grant: replacement.grant.clone(),
                },
                transition: transition.clone(),
            });
        }
        if loaded.grant_reissue_transitions.iter().any(|transition| {
            transition.audited_at.is_none()
                && (transition.device_id == source_revision.device_id
                    || transition.device_id == replacement_revision.device_id)
        }) {
            bail!("mobile device grant reissue audit delivery is pending");
        }
        let mut rotation_transitions = loaded.rotation_transitions.clone();
        if rotation_transitions.len() >= MAX_ROTATION_TRANSITIONS {
            if let Some(index) = rotation_transitions
                .iter()
                .position(|transition| transition.audited_at.is_some())
            {
                rotation_transitions.remove(index);
            } else {
                bail!("mobile device rotation audit outbox is full");
            }
        }
        if source.grant.id != source_revision.grant_id
            || source.grant.revocation_epoch != source_revision.revocation_epoch
        {
            bail!("source mobile device grant changed during rotation");
        }
        if replacement.grant.id != replacement_revision.grant_id
            || replacement.grant.revocation_epoch != replacement_revision.revocation_epoch
        {
            bail!("replacement mobile device grant changed during rotation");
        }
        if source.device.revoked_at.is_some() {
            bail!("source mobile device is revoked");
        }
        if replacement.device.revoked_at.is_some() {
            bail!("replacement mobile device is revoked");
        }
        source
            .grant
            .authorize(None, source.grant.minimum_assurance, rotated_at)
            .context("source mobile device grant is not active")?;
        replacement
            .grant
            .authorize(None, replacement.grant.minimum_assurance, rotated_at)
            .context("replacement mobile device grant is not active")?;
        let required_replacement_assurance = if source
            .grant
            .restrictions
            .require_fresh_user_verification_for
            .is_empty()
        {
            source.grant.minimum_assurance
        } else {
            source
                .grant
                .minimum_assurance
                .max(AssuranceLevel::FreshUserVerification)
        };
        if required_replacement_assurance > AssuranceLevel::Possession {
            let replacement_key = self
                .authorization_keys
                .active(replacement_revision.device_id)?
                .context(
                    "replacement mobile device lacks an active step-up authorization key required by the source grant",
                )?;
            if replacement_key.assurance_class.ceiling() < required_replacement_assurance {
                bail!(
                    "replacement mobile device authorization key cannot satisfy the source grant assurance policy"
                );
            }
        }

        let expires_at = source.grant.expires_at.unwrap_or(
            rotated_at + chrono::Duration::days(super::grant::MAX_DEVICE_GRANT_LIFETIME_DAYS),
        );
        let replacement_grant = replacement
            .grant
            .reissue(
                &replacement.device.public_key_x963,
                source.grant.scopes.clone(),
                source.grant.restrictions.clone(),
                source.grant.minimum_assurance,
                rotated_at,
                expires_at,
            )
            .context("source mobile device grant policy cannot be transferred")?;

        updated[source_index].device.revoked_at = Some(rotated_at);
        updated[source_index].grant.revocation_epoch = updated[source_index]
            .grant
            .revocation_epoch
            .checked_add(1)
            .context("source mobile device revocation epoch overflow")?;
        updated[replacement_index].device.scopes = replacement_grant.scopes.clone();
        updated[replacement_index].grant = replacement_grant;
        validate_devices(&updated)?;

        self.authorization_keys
            .revoke(source_revision.device_id, rotated_at)?;
        let transition = DeviceRotationAuditTransition {
            transition_id: Uuid::new_v4(),
            source_device_id: source_revision.device_id,
            replacement_device_id: replacement_revision.device_id,
            source_grant_id: updated[source_index].grant.id,
            source_revocation_epoch: updated[source_index].grant.revocation_epoch,
            replacement_grant_id: updated[replacement_index].grant.id,
            replacement_revocation_epoch: updated[replacement_index].grant.revocation_epoch,
            occurred_at: rotated_at,
            audited_at: None,
        };
        rotation_transitions.push(transition.clone());
        write_registry(
            &self.path,
            &updated,
            &rotation_transitions,
            &loaded.grant_reissue_transitions,
        )?;
        let replacement = DeviceAuthorizationRecord {
            device: updated[replacement_index].device.clone(),
            grant: updated[replacement_index].grant.clone(),
        };
        *devices = updated;
        Ok(DeviceRotationResult {
            replacement,
            transition,
        })
    }

    pub fn rename(&self, device_id: Uuid, display_name: String) -> Result<()> {
        let _store_lock = DeviceRegistryStoreLock::acquire(&self.path)?;
        let loaded = read_registry(&self.path)?;
        let mut devices = self
            .devices
            .write()
            .map_err(|_| anyhow::anyhow!("mobile device registry lock poisoned"))?;
        let mut updated = loaded.devices;
        let device = updated
            .iter_mut()
            .find(|record| record.device.id == device_id)
            .context("mobile device is not registered")?;
        device.device.display_name = display_name;
        validate_devices(&updated)?;
        write_registry(
            &self.path,
            &updated,
            &loaded.rotation_transitions,
            &loaded.grant_reissue_transitions,
        )?;
        *devices = updated;
        Ok(())
    }

    pub fn revoke(&self, device_id: Uuid, revoked_at: DateTime<Utc>) -> Result<()> {
        let _store_lock = DeviceRegistryStoreLock::acquire(&self.path)?;
        let loaded = read_registry(&self.path)?;
        if loaded
            .grant_reissue_transitions
            .iter()
            .any(|transition| transition.device_id == device_id && transition.audited_at.is_none())
        {
            bail!("mobile device grant reissue audit delivery is pending");
        }
        let mut devices = self
            .devices
            .write()
            .map_err(|_| anyhow::anyhow!("mobile device registry lock poisoned"))?;
        let mut updated = loaded.devices;
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
        write_registry(
            &self.path,
            &updated,
            &loaded.rotation_transitions,
            &loaded.grant_reissue_transitions,
        )?;
        *devices = updated;
        self.authorization_keys.revoke(device_id, revoked_at)
    }

    pub fn forget_all(&self) -> Result<()> {
        let _store_lock = DeviceRegistryStoreLock::acquire(&self.path)?;
        let loaded = read_registry(&self.path)?;
        if loaded
            .rotation_transitions
            .iter()
            .any(|transition| transition.audited_at.is_none())
            || loaded
                .grant_reissue_transitions
                .iter()
                .any(|transition| transition.audited_at.is_none())
        {
            bail!("cannot forget mobile devices while audit delivery is pending");
        }
        let mut devices = self
            .devices
            .write()
            .map_err(|_| anyhow::anyhow!("mobile device registry lock poisoned"))?;
        write_registry(&self.path, &[], &[], &[])?;
        devices.clear();
        self.authorization_keys.forget_all()
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

    pub fn authorization_key(
        &self,
        device_id: Uuid,
    ) -> Result<Option<DeviceAuthorizationKeyRecord>> {
        self.authorization_keys.active(device_id)
    }

    pub fn revoke_authorization_key(
        &self,
        device_id: Uuid,
        revoked_at: DateTime<Utc>,
    ) -> Result<()> {
        self.device(device_id)?
            .context("mobile device is not registered")?;
        self.authorization_keys.revoke(device_id, revoked_at)
    }

    pub fn authorization_key_history(
        &self,
        device_id: Uuid,
    ) -> Result<Vec<DeviceAuthorizationKeyRecord>> {
        self.authorization_keys.history(device_id)
    }

    pub(crate) fn pending_rotation_audits(&self) -> Result<Vec<DeviceRotationAuditTransition>> {
        let _store_lock = DeviceRegistryStoreLock::acquire(&self.path)?;
        let loaded = read_registry(&self.path)?;
        Ok(loaded
            .rotation_transitions
            .into_iter()
            .filter(|transition| transition.audited_at.is_none())
            .collect())
    }

    pub(crate) fn pending_grant_reissue_audit(
        &self,
        device_id: Uuid,
    ) -> Result<Option<DeviceGrantReissueAuditTransition>> {
        let _store_lock = DeviceRegistryStoreLock::acquire(&self.path)?;
        let loaded = read_registry(&self.path)?;
        let pending: Vec<_> = loaded
            .grant_reissue_transitions
            .iter()
            .filter(|transition| {
                transition.device_id == device_id && transition.audited_at.is_none()
            })
            .collect();
        let transition = match pending.as_slice() {
            [] => return Ok(None),
            [transition] => (*transition).clone(),
            _ => bail!("mobile device has multiple pending grant-reissue audits"),
        };
        let current = loaded
            .devices
            .iter()
            .find(|record| record.device.id == device_id)
            .context("mobile device grant-reissue transition references an unknown device")?;
        if current.grant.id != transition.replacement_grant_id
            || current.grant.revocation_epoch != transition.replacement_revocation_epoch
        {
            bail!("pending mobile device grant-reissue audit does not match current authority");
        }
        Ok(Some(transition))
    }

    pub(crate) fn mark_rotation_audited(
        &self,
        receipt: &AuditDeliveryReceipt,
        audited_at: DateTime<Utc>,
    ) -> Result<()> {
        let _store_lock = DeviceRegistryStoreLock::acquire(&self.path)?;
        let mut loaded = read_registry(&self.path)?;
        let transition = loaded
            .rotation_transitions
            .iter_mut()
            .find(|transition| transition.transition_id == receipt.transition_id())
            .context("mobile device rotation transition is not registered")?;
        if transition.audited_at.is_none() {
            transition.audited_at = Some(audited_at);
            write_registry(
                &self.path,
                &loaded.devices,
                &loaded.rotation_transitions,
                &loaded.grant_reissue_transitions,
            )?;
        }
        *self
            .devices
            .write()
            .map_err(|_| anyhow::anyhow!("mobile device registry lock poisoned"))? = loaded.devices;
        Ok(())
    }

    pub(crate) fn mark_grant_reissue_audited(
        &self,
        receipt: &AuditDeliveryReceipt,
        audited_at: DateTime<Utc>,
    ) -> Result<()> {
        let _store_lock = DeviceRegistryStoreLock::acquire(&self.path)?;
        let mut loaded = read_registry(&self.path)?;
        let transition = loaded
            .grant_reissue_transitions
            .iter_mut()
            .find(|transition| transition.transition_id == receipt.transition_id())
            .context("mobile device grant-reissue transition is not registered")?;
        let current = loaded
            .devices
            .iter()
            .find(|record| record.device.id == transition.device_id)
            .context("mobile device grant-reissue transition references an unknown device")?;
        if current.grant.id != transition.replacement_grant_id
            || current.grant.revocation_epoch != transition.replacement_revocation_epoch
        {
            bail!("mobile device grant-reissue audit cannot acknowledge absent authority");
        }
        if transition.audited_at.is_none() {
            transition.audited_at = Some(audited_at);
            write_registry(
                &self.path,
                &loaded.devices,
                &loaded.rotation_transitions,
                &loaded.grant_reissue_transitions,
            )?;
        }
        *self
            .devices
            .write()
            .map_err(|_| anyhow::anyhow!("mobile device registry lock poisoned"))? = loaded.devices;
        Ok(())
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
                rotation_transitions: Vec::new(),
                grant_reissue_transitions: Vec::new(),
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
    let version =
        u16::try_from(version).context("mobile device registry version is out of range")?;

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
                rotation_transitions: Vec::new(),
                grant_reissue_transitions: Vec::new(),
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
                rotation_transitions: stored.rotation_transitions,
                grant_reissue_transitions: stored.grant_reissue_transitions,
                migrated: false,
            }
        }
        _ => bail!("unsupported mobile device registry version"),
    };
    validate_devices(&loaded.devices)?;
    validate_rotation_transitions(&loaded.devices, &loaded.rotation_transitions)?;
    validate_grant_reissue_transitions(&loaded.devices, &loaded.grant_reissue_transitions)?;
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

fn write_registry(
    path: &Path,
    devices: &[GrantedDeviceRecord],
    rotation_transitions: &[DeviceRotationAuditTransition],
    grant_reissue_transitions: &[DeviceGrantReissueAuditTransition],
) -> Result<()> {
    validate_devices(devices)?;
    validate_rotation_transitions(devices, rotation_transitions)?;
    validate_grant_reissue_transitions(devices, grant_reissue_transitions)?;
    let stored = StoredDeviceRegistry {
        version: DEVICE_REGISTRY_VERSION,
        devices: devices.to_vec(),
        rotation_transitions: rotation_transitions.to_vec(),
        grant_reissue_transitions: grant_reissue_transitions.to_vec(),
    };
    let mut encoded =
        serde_json::to_vec_pretty(&stored).context("failed to encode mobile device registry")?;
    encoded.push(b'\n');
    atomic_replace_private(path, &encoded)
}

fn validate_grant_reissue_transitions(
    devices: &[GrantedDeviceRecord],
    transitions: &[DeviceGrantReissueAuditTransition],
) -> Result<()> {
    if transitions.len() > MAX_GRANT_REISSUE_TRANSITIONS {
        bail!("mobile device grant-reissue audit outbox exceeds the record limit");
    }
    for (index, transition) in transitions.iter().enumerate() {
        let policy_digest = URL_SAFE_NO_PAD
            .decode(&transition.requested_policy_digest)
            .context("mobile device grant-reissue policy digest is invalid")?;
        if policy_digest.len() != 32
            || URL_SAFE_NO_PAD.encode(policy_digest) != transition.requested_policy_digest
            || transitions[..index]
                .iter()
                .any(|existing| existing.transition_id == transition.transition_id)
            || !devices
                .iter()
                .any(|device| device.device.id == transition.device_id)
            || transition.previous_grant_id == transition.replacement_grant_id
            || transition.replacement_revocation_epoch <= transition.previous_revocation_epoch
            || transition
                .audited_at
                .is_some_and(|audited_at| audited_at < transition.occurred_at)
        {
            bail!("mobile device grant-reissue audit transition is invalid");
        }
    }
    Ok(())
}

fn validate_rotation_transitions(
    devices: &[GrantedDeviceRecord],
    transitions: &[DeviceRotationAuditTransition],
) -> Result<()> {
    if transitions.len() > MAX_ROTATION_TRANSITIONS {
        bail!("mobile device rotation audit outbox exceeds the record limit");
    }
    for (index, transition) in transitions.iter().enumerate() {
        if transition.source_device_id == transition.replacement_device_id
            || transitions[..index]
                .iter()
                .any(|existing| existing.transition_id == transition.transition_id)
            || !devices
                .iter()
                .any(|device| device.device.id == transition.source_device_id)
            || !devices
                .iter()
                .any(|device| device.device.id == transition.replacement_device_id)
            || transition
                .audited_at
                .is_some_and(|audited_at| audited_at < transition.occurred_at)
        {
            bail!("mobile device rotation audit transition is invalid");
        }
    }
    Ok(())
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
    use super::super::assurance::AssuranceClass;
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
        assert!(registry.replace_grant(record.id, grant.clone()).is_err());
        grant.id = Uuid::new_v4();
        grant.revocation_epoch = 2;
        registry.replace_grant(record.id, grant.clone()).unwrap();
        grant.id = Uuid::new_v4();
        grant.revocation_epoch = 1;
        assert!(registry.replace_grant(record.id, grant).is_err());
    }

    #[test]
    fn authorization_store_failure_cannot_block_possession_revocation_or_forget() {
        let temp = tempfile::tempdir().unwrap();
        let registry = DeviceRegistry::load(temp.path()).unwrap();
        let first = device(Uuid::from_u128(1), "First synthetic phone");
        registry.register(first.clone()).unwrap();
        let authorization_path = temp
            .path()
            .join("mobile")
            .join(super::super::assurance::AUTHORIZATION_KEYS_FILE);
        atomic_replace_private(&authorization_path, b"{not valid json}\n").unwrap();

        assert!(registry.revoke(first.id, Utc::now()).is_err());
        let revoked = registry.authorization_record(first.id).unwrap().unwrap();
        assert!(revoked.device.revoked_at.is_some());
        assert_eq!(revoked.grant.revocation_epoch, 1);

        std::fs::remove_file(&authorization_path).unwrap();
        std::fs::create_dir(&authorization_path).unwrap();
        assert!(registry.forget_all().is_err());
        assert!(registry.list_status().unwrap().is_empty());
    }

    #[test]
    fn stale_registry_handle_cannot_resurrect_a_revoked_device() {
        let temp = tempfile::tempdir().unwrap();
        let first = DeviceRegistry::load(temp.path()).unwrap();
        let first_device = device(Uuid::from_u128(1), "First synthetic phone");
        first.register(first_device.clone()).unwrap();
        let second = DeviceRegistry::load(temp.path()).unwrap();

        second.revoke(first_device.id, Utc::now()).unwrap();
        first
            .register(device(Uuid::from_u128(2), "Second synthetic phone"))
            .unwrap();
        first.reload().unwrap();

        let revoked = first
            .authorization_record(first_device.id)
            .unwrap()
            .unwrap();
        assert!(revoked.device.revoked_at.is_some());
        assert_eq!(revoked.grant.revocation_epoch, 1);
        assert_eq!(first.list_status().unwrap().len(), 2);
    }

    #[test]
    fn failed_device_commit_rolls_back_new_authorization_key() {
        let temp = tempfile::tempdir().unwrap();
        let registry = DeviceRegistry::load(temp.path()).unwrap();
        let record = device(Uuid::from_u128(1), "Synthetic phone");
        let grant = DeviceGrant::for_device(
            record.id,
            &record.public_key_x963,
            record.scopes.clone(),
            record.paired_at,
        )
        .unwrap();
        std::fs::create_dir(&registry.path).unwrap();

        assert!(registry
            .commit_registration(
                Vec::new(),
                Vec::new(),
                Vec::new(),
                GrantedDeviceRecord {
                    device: record.clone(),
                    grant,
                },
                Some(NewAuthorizationKey {
                    public_key_x963: device(Uuid::from_u128(2), "Step-up key").public_key_x963,
                    assurance_class: AssuranceClass::BiometricOnly,
                    enrolled_at: record.paired_at,
                }),
            )
            .is_err());
        assert!(registry
            .authorization_key_history(record.id)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn device_revoke_and_forget_cascade_to_authorization_keys() {
        let temp = tempfile::tempdir().unwrap();
        let registry = DeviceRegistry::load(temp.path()).unwrap();
        let first = device(Uuid::from_u128(1), "First synthetic phone");
        let second = device(Uuid::from_u128(2), "Second synthetic phone");
        registry.register(first.clone()).unwrap();
        registry.register(second.clone()).unwrap();
        registry
            .authorization_keys
            .enroll_initial(
                first.id,
                &first.public_key_x963,
                NewAuthorizationKey {
                    public_key_x963: device(Uuid::from_u128(3), "Step-up one").public_key_x963,
                    assurance_class: super::super::assurance::AssuranceClass::BiometricOnly,
                    enrolled_at: first.paired_at,
                },
            )
            .unwrap();
        registry
            .authorization_keys
            .enroll_initial(
                second.id,
                &second.public_key_x963,
                NewAuthorizationKey {
                    public_key_x963: device(Uuid::from_u128(4), "Step-up two").public_key_x963,
                    assurance_class: super::super::assurance::AssuranceClass::UserVerification,
                    enrolled_at: second.paired_at,
                },
            )
            .unwrap();

        registry.revoke(first.id, Utc::now()).unwrap();
        assert!(registry.authorization_key(first.id).unwrap().is_none());
        assert!(registry.authorization_key(second.id).unwrap().is_some());

        registry.forget_all().unwrap();
        assert!(registry
            .authorization_keys
            .history(first.id)
            .unwrap()
            .is_empty());
        assert!(registry
            .authorization_keys
            .history(second.id)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn stale_handles_have_no_raw_authorization_key_rotation_bypass() {
        let temp = tempfile::tempdir().unwrap();
        let stale = DeviceRegistry::load(temp.path()).unwrap();
        let record = device(Uuid::from_u128(1), "Synthetic phone");
        let grant = DeviceGrant::for_device(
            record.id,
            &record.public_key_x963,
            record.scopes.clone(),
            record.paired_at,
        )
        .unwrap();
        stale
            .register_with_grant_and_authorization(
                record.clone(),
                grant,
                Some(NewAuthorizationKey {
                    public_key_x963: device(Uuid::from_u128(2), "Step-up key").public_key_x963,
                    assurance_class: AssuranceClass::BiometricOnly,
                    enrolled_at: record.paired_at,
                }),
            )
            .unwrap();
        let current = DeviceRegistry::load(temp.path()).unwrap();
        current.revoke(record.id, Utc::now()).unwrap();

        assert!(stale.authorization_key(record.id).unwrap().is_none());
        let registry_bypass = ["pub fn ", "rotate_authorization_key("].concat();
        let key_store_bypass = ["    pub fn ", "rotate("].concat();
        assert!(!include_str!("registry.rs").contains(&registry_bypass));
        assert!(!include_str!("assurance.rs").contains(&key_store_bypass));
    }
}
