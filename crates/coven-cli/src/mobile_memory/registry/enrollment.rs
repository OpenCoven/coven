//! Recoverable registration across the device and authorization-key stores.
//!
//! The registry lock is always acquired before the key-store transaction. A
//! durable intent precedes either authority write, and its removal follows the
//! directory barriers of the selected outcome. An interrupted cleanup is
//! idempotent. Unknown images are refused, never inferred from device presence.
//! Barriers retain the generic mobile filesystem contract. Windows process-kill
//! recovery does not establish physical power-loss or directory durability.
use super::*;
use sha2::{Digest, Sha256};
use std::io::Read;

pub(super) const JOURNAL: &str = ".initial-enrollment.json";
const MAX_IMAGE: u64 = 4 * 1024 * 1024;
const MAX_JOURNAL: u64 = 12 * 1024 * 1024;

#[derive(Debug)]
pub(in crate::mobile_memory) struct InitialEnrollmentUncertain;
impl std::fmt::Display for InitialEnrollmentUncertain {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(
            "initial enrollment outcome uncertain; reconcile durable state before retrying",
        )
    }
}
impl std::error::Error for InitialEnrollmentUncertain {}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Intent {
    version: u16,
    transaction: Uuid,
    registry_before: Option<String>,
    registry_after: String,
    keys_before: Option<String>,
    keys_after: String,
    device: GrantedDeviceRecord,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Journal {
    intent: Intent,
    checksum: String,
}

fn digest(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(bytes))
}

fn path(registry: &Path) -> Result<PathBuf> {
    Ok(registry
        .parent()
        .context("registry has no parent")?
        .join(JOURNAL))
}

fn read_image(path: &Path, bound: u64) -> Result<Option<Vec<u8>>> {
    match fs::symlink_metadata(path) {
        Ok(_) => validate_private_file(path)?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).context("inspect initial-enrollment image"),
    }
    let mut bytes = Vec::new();
    fs::File::open(path)?
        .take(bound + 1)
        .read_to_end(&mut bytes)?;
    anyhow::ensure!(
        bytes.len() as u64 <= bound,
        "initial-enrollment image exceeds bound"
    );
    Ok(Some(bytes))
}

fn decode_image(encoded: &str) -> Result<Vec<u8>> {
    let bytes = URL_SAFE_NO_PAD
        .decode(encoded)
        .context("invalid initial-enrollment image encoding")?;
    anyhow::ensure!(
        bytes.len() as u64 <= MAX_IMAGE && URL_SAFE_NO_PAD.encode(&bytes) == encoded,
        "invalid initial-enrollment image"
    );
    Ok(bytes)
}

fn validate_digest(encoded: &str) -> Result<()> {
    let decoded = URL_SAFE_NO_PAD
        .decode(encoded)
        .context("invalid initial-enrollment digest")?;
    anyhow::ensure!(
        decoded.len() == 32 && URL_SAFE_NO_PAD.encode(decoded) == encoded,
        "invalid initial-enrollment digest"
    );
    Ok(())
}

impl Journal {
    fn validate(&self) -> Result<(Option<Vec<u8>>, Vec<u8>)> {
        anyhow::ensure!(
            self.intent.version == 1 && !self.intent.transaction.is_nil(),
            "invalid initial-enrollment journal version or transaction"
        );
        anyhow::ensure!(
            self.checksum == digest(&serde_json::to_vec(&self.intent)?),
            "initial-enrollment journal checksum mismatch"
        );
        if let Some(before) = &self.intent.registry_before {
            validate_digest(before)?;
        }
        validate_digest(&self.intent.registry_after)?;
        anyhow::ensure!(
            self.intent.registry_before.as_ref() != Some(&self.intent.registry_after),
            "initial-enrollment registry images are identical"
        );
        validate_granted_device(&self.intent.device)?;
        let before = self
            .intent
            .keys_before
            .as_deref()
            .map(decode_image)
            .transpose()?;
        let after = decode_image(&self.intent.keys_after)?;
        let prior = super::super::assurance::decode_enrollment_keys(before.as_deref())?;
        let next = super::super::assurance::decode_enrollment_keys(Some(&after))?;
        anyhow::ensure!(
            next.len() == prior.len() + 1 && next[..prior.len()] == prior,
            "initial-enrollment journal is not one key insertion"
        );
        let inserted = next
            .last()
            .context("initial-enrollment journal omitted inserted key")?;
        anyhow::ensure!(
            inserted.device_id == self.intent.device.device.id
                && inserted.public_key_x963 != self.intent.device.device.public_key_x963
                && inserted.key_epoch == 1
                && inserted.revoked_at.is_none()
                && prior.iter().all(|key| key.device_id != inserted.device_id),
            "initial-enrollment journal has conflicting key or device identity"
        );
        Ok((before, after))
    }
}

fn load(registry: &Path) -> Result<Option<Journal>> {
    let Some(bytes) = read_image(&path(registry)?, MAX_JOURNAL)? else {
        return Ok(None);
    };
    let journal: Journal =
        serde_json::from_slice(&bytes).context("invalid initial-enrollment journal")?;
    journal.validate()?;
    Ok(Some(journal))
}

fn sync_image(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(_) => {
            validate_private_file(path)?;
            fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(path)?
                .sync_all()?;
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error).context("inspect initial-enrollment image for sync"),
    }
    super::super::config::sync_directory(
        path.parent()
            .context("initial-enrollment image has no parent")?,
    )
}

fn remove_journal(registry: &Path) -> Result<()> {
    let journal = path(registry)?;
    fs::remove_file(&journal).context("remove recovered initial-enrollment intent")?;
    super::super::config::sync_directory(
        journal
            .parent()
            .context("initial-enrollment journal has no parent")?,
    )?;
    #[cfg(test)]
    super::super::config::write_checkpoint(
        &journal,
        super::super::config::MobileWritePhase::RetainedParentSynced,
    )?;
    Ok(())
}

/// The caller holds the registry lock; the callback retains the key lock for
/// the entire selection, repair, durability barriers and journal retirement.
pub(super) fn recover(registry: &Path, keys: &AuthorizationKeyRegistry) -> Result<()> {
    let Some(journal) = load(registry)? else {
        return Ok(());
    };
    keys.with_enrollment_images(|images| recover_locked(registry, &journal, images))
}

/// Called by the common registry-lock admission point, including restore and
/// snapshot callers that do not own a DeviceRegistry handle. A stale handle
/// may not mutate the registry past an unresolved enrollment.
pub(super) fn recover_existing(registry: &Path) -> Result<()> {
    if load(registry)?.is_none() {
        return Ok(());
    }
    let home = registry
        .parent()
        .and_then(Path::parent)
        .context("registry omitted mobile/home directories")?;
    let keys = AuthorizationKeyRegistry::for_enrollment_recovery(home)?;
    recover(registry, &keys)
}

fn recover_locked(
    registry: &Path,
    journal: &Journal,
    keys: &super::super::assurance::EnrollmentImages<'_>,
) -> Result<()> {
    let (before, after) = journal.validate()?;
    let registry_image = read_image(registry, MAX_IMAGE)?;
    let registry_digest = registry_image.as_deref().map(digest);
    let committed = if registry_digest.as_ref() == Some(&journal.intent.registry_after) {
        true
    } else if registry_digest == journal.intent.registry_before {
        false
    } else {
        bail!("initial-enrollment registry image is unrecognized; preserve journal and recover explicitly");
    };
    let current = keys.read()?;
    // Validate *all* inputs and both recognized images before touching either
    // authority store. In particular, never erase an unrelated new key.
    anyhow::ensure!(
        current == before || current.as_deref() == Some(after.as_slice()),
        "initial-enrollment key image is unrecognized; preserve journal and recover explicitly"
    );
    let selected = if committed {
        Some(after.as_slice())
    } else {
        before.as_deref()
    };
    if current.as_deref() != selected {
        keys.replace(selected)?;
    }
    // A namespace replacement may have succeeded before returning a barrier
    // error. Repeat durability before removing its only recovery record.
    sync_image(registry)?;
    keys.sync()?;
    remove_journal(registry)
}

pub(super) fn commit(
    registry: &Path,
    keys: &AuthorizationKeyRegistry,
    device: &GrantedDeviceRecord,
    key: NewAuthorizationKey,
    registry_after: &[u8],
    write_registry: impl FnOnce() -> Result<()>,
) -> Result<()> {
    // Every caller must recover a previous intent before starting a new one.
    recover(registry, keys)?;
    keys.with_enrollment_images(|images| {
        let registry_before = read_image(registry, MAX_IMAGE)?;
        anyhow::ensure!(
            registry_after.len() as u64 <= MAX_IMAGE,
            "initial-enrollment registry image exceeds bound"
        );
        let before = images.read()?;
        let after =
            images.prepare_initial(device.device.id, &device.device.public_key_x963, key)?;
        let intent = Intent {
            version: 1,
            transaction: Uuid::new_v4(),
            registry_before: registry_before.as_deref().map(digest),
            registry_after: digest(registry_after),
            keys_before: before.as_deref().map(|bytes| URL_SAFE_NO_PAD.encode(bytes)),
            keys_after: URL_SAFE_NO_PAD.encode(&after),
            device: device.clone(),
        };
        let journal = Journal {
            checksum: digest(&serde_json::to_vec(&intent)?),
            intent,
        };
        journal.validate()?;
        let encoded = serde_json::to_vec(&journal)?;
        anyhow::ensure!(
            encoded.len() as u64 <= MAX_JOURNAL,
            "initial-enrollment journal exceeds bound"
        );
        let journal_path = path(registry)?;
        anyhow::ensure!(
            fs::symlink_metadata(&journal_path)
                .is_err_and(|error| error.kind() == io::ErrorKind::NotFound),
            "initial-enrollment intent already exists or cannot be inspected"
        );
        atomic_replace_private(&journal_path, &encoded).context(InitialEnrollmentUncertain)?;
        sync_image(&journal_path).context(InitialEnrollmentUncertain)?;
        super::super::config::sync_directory(
            registry
                .parent()
                .and_then(Path::parent)
                .context("registry has no mobile/home parent")?,
        )
        .context(InitialEnrollmentUncertain)?;
        // The intent's file and directory barriers have succeeded before any
        // key authority becomes visible. All subsequent failures leave intent.
        images
            .replace(Some(&after))
            .context(InitialEnrollmentUncertain)?;
        let result = write_registry();
        // Reconcile returned errors as well as future crash recovery: an error
        // AFTER registry rename is committed uncertainty, never permission to
        // roll back the key belonging to an already committed device.
        let recovered = recover_locked(registry, &journal, images);
        match (result, recovered) {
            (_, Err(error)) => Err(error
                .context("initial-enrollment recovery pending")
                .context(InitialEnrollmentUncertain)),
            (Err(error), Ok(())) => Err(error.context(InitialEnrollmentUncertain)),
            (Ok(()), Ok(())) => Ok(()),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::super::super::assurance::AssuranceClass;
    use super::super::super::config::{set_write_observer, MobileWritePhase};
    use super::*;
    use p256::elliptic_curve::sec1::ToSec1Point;

    fn device(seed: u8) -> GrantedDeviceRecord {
        let key = p256::SecretKey::from_slice(&[seed; 32]).unwrap();
        let now = Utc::now();
        let record = DeviceRecord {
            id: Uuid::new_v4(),
            display_name: format!("Journal peer {seed}"),
            public_key_x963: URL_SAFE_NO_PAD
                .encode(key.public_key().to_sec1_point(false).as_bytes()),
            paired_at: now,
            revoked_at: None,
            suspended_at: None,
            scopes: vec![DeviceScope::MemoryRead],
        };
        let grant = DeviceGrant::for_device(
            record.id,
            &record.public_key_x963,
            record.scopes.clone(),
            now,
        )
        .unwrap();
        GrantedDeviceRecord {
            device: record,
            grant,
        }
    }

    fn key_for(device: &GrantedDeviceRecord) -> NewAuthorizationKey {
        let key = p256::SecretKey::from_slice(&[103; 32]).unwrap();
        NewAuthorizationKey {
            public_key_x963: URL_SAFE_NO_PAD
                .encode(key.public_key().to_sec1_point(false).as_bytes()),
            assurance_class: AssuranceClass::BiometricOnly,
            enrolled_at: device.device.paired_at,
        }
    }

    struct Cut {
        _home: tempfile::TempDir,
        registry: DeviceRegistry,
        target: GrantedDeviceRecord,
        before_registry: Vec<u8>,
        after_registry: Vec<u8>,
        before_keys: Option<Vec<u8>>,
        after_keys: Vec<u8>,
    }

    impl Cut {
        fn staged(prior_keys: bool) -> Self {
            let home = tempfile::tempdir().unwrap();
            let registry = DeviceRegistry::load(home.path()).unwrap();
            let prior = device(101);
            registry
                .register_with_grant(prior.device.clone(), prior.grant)
                .unwrap();
            if prior_keys {
                let prior_key = p256::SecretKey::from_slice(&[102; 32]).unwrap();
                registry
                    .authorization_keys
                    .enroll_initial(
                        prior.device.id,
                        &prior.device.public_key_x963,
                        NewAuthorizationKey {
                            public_key_x963: URL_SAFE_NO_PAD
                                .encode(prior_key.public_key().to_sec1_point(false).as_bytes()),
                            assurance_class: AssuranceClass::UserVerification,
                            enrolled_at: prior.device.paired_at,
                        },
                    )
                    .unwrap();
            }
            let target = device(104);
            let loaded = read_registry(&registry.path).unwrap();
            let mut devices = loaded.devices;
            devices.push(target.clone());
            let after_registry = encode_registry_with_introductions(
                &registry.path,
                &devices,
                &loaded.rotation_transitions,
                &loaded.grant_reissue_transitions,
                &loaded.introduction_transitions,
            )
            .unwrap();
            let before_registry = fs::read(&registry.path).unwrap();
            let (before_keys, after_keys) = registry
                .authorization_keys
                .with_enrollment_images(|images| {
                    let before = images.read()?;
                    let after = images.prepare_initial(
                        target.device.id,
                        &target.device.public_key_x963,
                        key_for(&target),
                    )?;
                    let intent = Intent {
                        version: 1,
                        transaction: Uuid::new_v4(),
                        registry_before: Some(digest(&before_registry)),
                        registry_after: digest(&after_registry),
                        keys_before: before.as_deref().map(|bytes| URL_SAFE_NO_PAD.encode(bytes)),
                        keys_after: URL_SAFE_NO_PAD.encode(&after),
                        device: target.clone(),
                    };
                    let journal = Journal {
                        checksum: digest(&serde_json::to_vec(&intent)?),
                        intent,
                    };
                    journal.validate()?;
                    atomic_replace_private(&path(&registry.path)?, &serde_json::to_vec(&journal)?)?;
                    Ok((before, after))
                })
                .unwrap();
            Self {
                _home: home,
                registry,
                target,
                before_registry,
                after_registry,
                before_keys,
                after_keys,
            }
        }

        fn keys_path(&self) -> PathBuf {
            self.registry
                .path
                .parent()
                .unwrap()
                .join(super::super::super::assurance::AUTHORIZATION_KEYS_FILE)
        }
        fn recover(&self) -> Result<DeviceRegistry> {
            DeviceRegistry::load(self._home.path())
        }
    }

    #[test]
    fn exact_recovery_matrix_preserves_absence_and_committed_keys() {
        for prior_keys in [false, true] {
            for keys_written in [false, true] {
                for device_written in [false, true] {
                    let cut = Cut::staged(prior_keys);
                    if keys_written {
                        atomic_replace_private(&cut.keys_path(), &cut.after_keys).unwrap();
                    }
                    if device_written {
                        atomic_replace_private(&cut.registry.path, &cut.after_registry).unwrap();
                    }
                    let recovered = cut.recover().unwrap();
                    assert!(
                        !path(&cut.registry.path).unwrap().exists(),
                        "recovery left intent"
                    );
                    assert_eq!(
                        fs::read(&cut.registry.path).unwrap(),
                        if device_written {
                            &cut.after_registry
                        } else {
                            &cut.before_registry
                        }
                        .clone()
                    );
                    assert_eq!(
                        fs::read(cut.keys_path()).ok(),
                        if device_written {
                            Some(cut.after_keys.clone())
                        } else {
                            cut.before_keys.clone()
                        }
                    );
                    assert_eq!(
                        recovered.device(cut.target.device.id).unwrap().is_some(),
                        device_written
                    );
                    // A crash after repair but before cleanup can simply repeat
                    // the same matrix row; neither side is incremented twice.
                    assert!(cut.recover().is_ok());
                }
            }
        }
    }

    #[test]
    fn corrupt_or_unknown_journal_images_refuse_without_mutation() {
        for corruption in ["checksum", "registry", "keys"] {
            let cut = Cut::staged(true);
            let journal_path = path(&cut.registry.path).unwrap();
            match corruption {
                "checksum" => {
                    let mut journal: Journal =
                        serde_json::from_slice(&fs::read(&journal_path).unwrap()).unwrap();
                    journal.checksum = URL_SAFE_NO_PAD.encode([0; 32]);
                    atomic_replace_private(&journal_path, &serde_json::to_vec(&journal).unwrap())
                        .unwrap();
                }
                "registry" => {
                    let mut loaded = read_registry(&cut.registry.path).unwrap();
                    loaded.devices[0].device.display_name.push_str(" changed");
                    write_registry_with_introductions(
                        &cut.registry.path,
                        &loaded.devices,
                        &loaded.rotation_transitions,
                        &loaded.grant_reissue_transitions,
                        &loaded.introduction_transitions,
                    )
                    .unwrap();
                }
                "keys" => {
                    let mut keys = super::super::super::assurance::decode_enrollment_keys(Some(
                        &cut.after_keys,
                    ))
                    .unwrap();
                    keys[0].revoked_at = Some(Utc::now());
                    let bytes =
                        serde_json::to_vec(&serde_json::json!({"version": 1, "keys": keys}))
                            .unwrap();
                    atomic_replace_private(&cut.keys_path(), &bytes).unwrap();
                }
                _ => unreachable!(),
            }
            let before = [
                fs::read(&cut.registry.path).ok(),
                fs::read(cut.keys_path()).ok(),
                fs::read(&journal_path).ok(),
            ];
            assert!(cut.recover().is_err(), "accepted {corruption}");
            assert_eq!(
                [
                    fs::read(&cut.registry.path).ok(),
                    fs::read(cut.keys_path()).ok(),
                    fs::read(&journal_path).ok()
                ],
                before,
                "mutated authority on {corruption}"
            );
        }
    }

    #[test]
    fn independent_key_mutation_refuses_pending_intent_without_overwrite() {
        let cut = Cut::staged(true);
        let before = fs::read(cut.keys_path()).unwrap();
        let prior_id = super::super::super::assurance::decode_enrollment_keys(Some(&before))
            .unwrap()[0]
            .device_id;
        assert!(cut
            .registry
            .authorization_keys
            .revoke(prior_id, Utc::now())
            .is_err());
        assert_eq!(fs::read(cut.keys_path()).unwrap(), before);
        assert!(path(&cut.registry.path).unwrap().exists());
    }

    #[test]
    fn post_rename_error_never_rolls_back_committed_device_key() {
        let home = tempfile::tempdir().unwrap();
        let registry = DeviceRegistry::load(home.path()).unwrap();
        let target = device(105);
        let target_path = registry.path.clone();
        set_write_observer(Box::new(move |path, phase| {
            if path == target_path && phase == MobileWritePhase::Renamed {
                bail!("injected post-rename failure");
            }
            Ok(())
        }));
        let result = registry.register_with_grant_and_authorization(
            target.device.clone(),
            target.grant.clone(),
            Some(key_for(&target)),
        );
        set_write_observer(Box::new(|_, _| Ok(())));
        let error = result.unwrap_err();
        assert!(
            error.is::<InitialEnrollmentUncertain>(),
            "lost uncertainty: {error:#}"
        );
        let recovered = DeviceRegistry::load(home.path()).unwrap();
        assert!(recovered.device(target.device.id).unwrap().is_some());
        assert_eq!(
            recovered
                .authorization_key_history(target.device.id)
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn key_timestamp_remains_independent_of_device_pairing_time() {
        let home = tempfile::tempdir().unwrap();
        let registry = DeviceRegistry::load(home.path()).unwrap();
        let target = device(106);
        let mut key = key_for(&target);
        key.enrolled_at += chrono::Duration::seconds(1);
        let expected = key.enrolled_at;
        registry
            .register_with_grant_and_authorization(target.device.clone(), target.grant, Some(key))
            .unwrap();
        assert_eq!(
            registry
                .authorization_key_history(target.device.id)
                .unwrap()[0]
                .enrolled_at,
            expected
        );
    }

    #[test]
    fn journal_publication_and_cleanup_errors_preserve_typed_uncertainty() {
        for failed_phase in [
            MobileWritePhase::Renamed,
            MobileWritePhase::RetainedParentSynced,
        ] {
            let home = tempfile::tempdir().unwrap();
            let registry = DeviceRegistry::load(home.path()).unwrap();
            let target = device(107);
            let journal_path = path(&registry.path).unwrap();
            set_write_observer(Box::new(move |path, phase| {
                if path == journal_path && phase == failed_phase {
                    bail!("injected journal boundary failure");
                }
                Ok(())
            }));
            let result = registry.register_with_grant_and_authorization(
                target.device.clone(),
                target.grant.clone(),
                Some(key_for(&target)),
            );
            set_write_observer(Box::new(|_, _| Ok(())));
            let error = result.unwrap_err();
            assert!(
                error.is::<InitialEnrollmentUncertain>(),
                "journal boundary lost uncertainty: {error:#}"
            );
            let recovered = DeviceRegistry::load(home.path()).unwrap();
            let committed = failed_phase == MobileWritePhase::RetainedParentSynced;
            assert_eq!(
                recovered.device(target.device.id).unwrap().is_some(),
                committed
            );
            assert_eq!(
                recovered
                    .authorization_key_history(target.device.id)
                    .unwrap()
                    .len(),
                usize::from(committed)
            );
            assert!(!path(&registry.path).unwrap().exists());
        }
    }
}
