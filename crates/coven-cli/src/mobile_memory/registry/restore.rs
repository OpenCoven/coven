//! Complete current registry capture; never a redacted status projection.
use super::*;

/// Absence is legacy state; a present declaration must carry an integer.
/// In particular, null must never erase an existing restore reference.
pub(super) fn deserialize_epoch<'de, D>(
    deserializer: D,
) -> std::result::Result<Option<u64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Some(u64::deserialize(deserializer)?))
}

pub(crate) fn with_restore_snapshot<T>(
    home: &Path,
    operation: impl FnOnce(RegistryRestoreSnapshot) -> Result<T>,
) -> Result<T> {
    let path = home.join("mobile").join(DEVICES_FILE);
    let _lock = DeviceRegistryStoreLock::acquire(&path)?;
    anyhow::ensure!(
        super::super::config::validate_restore_export_component(&path)?,
        "current device authority is missing; export cannot infer an empty registry"
    );
    let loaded = read_registry(&path)?;
    anyhow::ensure!(
        !loaded.migrated,
        "migrate legacy device authority before exporting"
    );
    let devices = loaded
        .devices
        .iter()
        .map(|record| DeviceGrantRevision {
            device_id: record.device.id,
            grant_id: record.grant.id,
            revocation_epoch: record.grant.revocation_epoch,
        })
        .collect();
    let stored = StoredDeviceRegistry {
        version: DEVICE_REGISTRY_VERSION,
        restore_epoch: loaded.restore_epoch,
        devices: loaded.devices,
        rotation_transitions: loaded.rotation_transitions,
        grant_reissue_transitions: loaded.grant_reissue_transitions,
        introduction_transitions: loaded.introduction_transitions,
    };
    operation(RegistryRestoreSnapshot {
        encoded: zeroize::Zeroizing::new(serde_json::to_vec(&stored)?),
        devices,
        restore_epoch: stored.restore_epoch,
    })
}

/// Validated complete authority; never an inferred empty restore source.
pub(crate) struct RestoreRegistry {
    stored: StoredDeviceRegistry,
}
impl RestoreRegistry {
    fn from_loaded(loaded: LoadedRegistry) -> Result<Self> {
        anyhow::ensure!(!loaded.migrated, "restore requires current registry format");
        Ok(Self {
            stored: StoredDeviceRegistry {
                version: DEVICE_REGISTRY_VERSION,
                restore_epoch: loaded.restore_epoch,
                devices: loaded.devices,
                rotation_transitions: loaded.rotation_transitions,
                grant_reissue_transitions: loaded.grant_reissue_transitions,
                introduction_transitions: loaded.introduction_transitions,
            },
        })
    }
    pub(crate) fn decode(bytes: &[u8]) -> Result<Self> {
        anyhow::ensure!(
            bytes.len() <= 4 * 1024 * 1024,
            "restore registry exceeds limit"
        );
        Self::from_loaded(decode_registry(bytes)?)
    }
    pub(crate) fn retained(home: &Path) -> Result<Self> {
        let path = home.join("mobile").join(DEVICES_FILE);
        let _lock = DeviceRegistryStoreLock::acquire(&path)?;
        super::super::config::validate_restore_export_component(&path)?;
        Self::from_loaded(read_registry(&path)?)
    }
    pub(crate) fn epoch(&self) -> u64 {
        self.stored.restore_epoch.unwrap_or(0)
    }
    pub(crate) fn revisions(&self) -> Vec<DeviceGrantRevision> {
        self.stored
            .devices
            .iter()
            .map(|r| DeviceGrantRevision {
                device_id: r.device.id,
                grant_id: r.grant.id,
                revocation_epoch: r.grant.revocation_epoch,
            })
            .collect()
    }
    pub(crate) fn check_not_older_than(&self, retained: &Self) -> Result<()> {
        anyhow::ensure!(
            self.epoch() >= retained.epoch(),
            "restore authority predates retained installation epoch"
        );
        for current in &retained.stored.devices {
            if let Some(incoming) = self
                .stored
                .devices
                .iter()
                .find(|r| r.device.id == current.device.id)
            {
                anyhow::ensure!(
                    incoming.grant.revocation_epoch >= current.grant.revocation_epoch,
                    "restore authority predates retained grant epoch"
                );
                if incoming.grant.revocation_epoch == current.grant.revocation_epoch {
                    anyhow::ensure!(
                        incoming.grant == current.grant
                            && incoming.device.public_key_x963 == current.device.public_key_x963
                            && (current.device.revoked_at.is_none()
                                || incoming.device.revoked_at.is_some())
                            && (current.device.suspended_at.is_none()
                                || incoming.device.suspended_at.is_some()),
                        "restore authority conflicts with retained grant, revocation or suspension"
                    );
                }
            }
        }
        Ok(())
    }
    pub(crate) fn advanced(
        &self,
        epoch: u64,
        targets: &std::collections::BTreeMap<Uuid, u64>,
    ) -> Result<Self> {
        anyhow::ensure!(
            epoch > self.epoch(),
            "restore installation epoch must advance"
        );
        let mut stored = self.stored.clone();
        stored.restore_epoch = Some(epoch);
        for record in &mut stored.devices {
            let target = *targets
                .get(&record.device.id)
                .context("restore pairing target missing")?;
            anyhow::ensure!(
                target > record.grant.revocation_epoch,
                "restore grant epoch must advance"
            );
            record.grant.revocation_epoch = target;
        }
        // Reuse all ordinary registry validators, including transition receipts.
        Self::decode(&serde_json::to_vec(&stored)?)
    }
    pub(crate) fn matches(&self, other: &Self) -> Result<bool> {
        Ok(serde_json::to_vec(&self.stored)? == serde_json::to_vec(&other.stored)?)
    }
    pub(crate) fn install(&self, home: &Path) -> Result<()> {
        let path = home.join("mobile").join(DEVICES_FILE);
        let _lock = DeviceRegistryStoreLock::acquire(&path)?;
        atomic_replace_private(&path, &serde_json::to_vec(&self.stored)?)?;
        crate::durable_fs::sync_parent(&path)?;
        #[cfg(test)]
        crate::mobile_memory::config::write_checkpoint(
            &path,
            crate::mobile_memory::config::MobileWritePhase::RetainedParentSynced,
        )?;
        crate::durable_fs::sync_parent(path.parent().context("missing mobile directory")?)?;
        #[cfg(test)]
        crate::mobile_memory::config::write_checkpoint(
            &path,
            crate::mobile_memory::config::MobileWritePhase::RetainedHomeSynced,
        )?;
        Ok(())
    }
}
