use super::*;
use crate::mobile_memory::introduction::{IntroductionCommitUncertain, MAX_INTRODUCTIONS};

impl DeviceRegistry {
    pub(in crate::mobile_memory) fn with_introducer<T>(
        &self,
        device_id: Uuid,
        operation: impl FnOnce(&DeviceAuthorizationRecord, &DeviceAuthorizationKeyRecord) -> Result<T>,
    ) -> Result<T> {
        let _lock = DeviceRegistryStoreLock::acquire(&self.path)?;
        let loaded = read_registry(&self.path)?;
        let source = introduction_source(&loaded, device_id)?;
        self.authorization_keys
            .with_active_key(device_id, |key| operation(&source, key))
    }

    pub(in crate::mobile_memory) fn commit_introduction(
        &self,
        source_id: Uuid,
        device: DeviceRecord,
        grant: DeviceGrant,
        nonce_digest: String,
        now: DateTime<Utc>,
        verify: impl FnOnce(&DeviceAuthorizationRecord, &DeviceAuthorizationKeyRecord) -> Result<()>,
    ) -> Result<(DeviceAuthorizationRecord, IntroductionTransition)> {
        let _lock = DeviceRegistryStoreLock::acquire(&self.path)?;
        let mut loaded = read_registry(&self.path)?;
        if loaded
            .introduction_transitions
            .iter()
            .any(|t| t.nonce_digest == nonce_digest)
        {
            bail!("introduction already consumed");
        }
        if loaded.introduction_transitions.len() >= MAX_INTRODUCTIONS {
            bail!("introduction consumption ledger is full");
        }
        let source = introduction_source(&loaded, source_id)?;
        let destination = DeviceAuthorizationRecord {
            device: device.clone(),
            grant: grant.clone(),
        };
        loaded.devices.push(GrantedDeviceRecord { device, grant });
        validate_devices(&loaded.devices)?;
        let transition = IntroductionTransition {
            transition_id: Uuid::new_v4(),
            nonce_digest,
            occurred_at: now,
            audited_at: None,
        };
        loaded.introduction_transitions.push(transition.clone());
        self.authorization_keys.with_active_key(source_id, |key| {
            verify(&source, key)?;
            // Challenge spend precedes this atomic registry commit. A failed
            // commit burns the challenge; it never becomes a success receipt.
            write_registry_with_introductions(
                &self.path,
                &loaded.devices,
                &loaded.rotation_transitions,
                &loaded.grant_reissue_transitions,
                &loaded.introduction_transitions,
            )
            .context(IntroductionCommitUncertain)?;
            #[cfg(test)]
            if FAIL_AFTER_INTRODUCTION_WRITE
                .lock()
                .unwrap()
                .remove(&self.path)
            {
                return Err(anyhow::anyhow!("injected post-replacement failure")
                    .context(IntroductionCommitUncertain));
            }
            *self
                .devices
                .write()
                .map_err(|_| anyhow::anyhow!("mobile device registry lock poisoned"))
                .context(IntroductionCommitUncertain)? = loaded.devices;
            Ok((destination, transition))
        })
    }

    pub(in crate::mobile_memory) fn pending_introduction_audits(
        &self,
    ) -> Result<Vec<IntroductionTransition>> {
        let _lock = DeviceRegistryStoreLock::acquire(&self.path)?;
        Ok(read_registry(&self.path)?
            .introduction_transitions
            .into_iter()
            .filter(|transition| transition.audited_at.is_none())
            .collect())
    }

    pub(in crate::mobile_memory) fn mark_introduction_audited(
        &self,
        receipt: &AuditDeliveryReceipt,
        now: DateTime<Utc>,
    ) -> Result<()> {
        let _lock = DeviceRegistryStoreLock::acquire(&self.path)?;
        let mut loaded = read_registry(&self.path)?;
        let transition = loaded
            .introduction_transitions
            .iter_mut()
            .find(|transition| transition.transition_id == receipt.transition_id())
            .context("introduction audit receipt has no committed transition")?;
        if transition.audited_at.is_some() {
            return Ok(());
        }
        transition.audited_at = Some(now);
        write_registry_with_introductions(
            &self.path,
            &loaded.devices,
            &loaded.rotation_transitions,
            &loaded.grant_reissue_transitions,
            &loaded.introduction_transitions,
        )
    }
}

fn introduction_source(
    loaded: &LoadedRegistry,
    device_id: Uuid,
) -> Result<DeviceAuthorizationRecord> {
    let source = loaded
        .devices
        .iter()
        .find(|record| record.device.id == device_id)
        .context("introduction source is not enrolled")?;
    if source.device.revoked_at.is_some() || source.device.suspended_at.is_some() {
        bail!("introduction source is inactive");
    }
    Ok(DeviceAuthorizationRecord {
        device: source.device.clone(),
        grant: source.grant.clone(),
    })
}

#[cfg(test)]
static FAIL_AFTER_INTRODUCTION_WRITE: LazyLock<Mutex<std::collections::HashSet<PathBuf>>> =
    LazyLock::new(|| Mutex::new(std::collections::HashSet::new()));

#[cfg(test)]
pub(in crate::mobile_memory) fn fail_after_introduction_write(path: &Path) {
    FAIL_AFTER_INTRODUCTION_WRITE
        .lock()
        .unwrap()
        .insert(path.to_owned());
}
