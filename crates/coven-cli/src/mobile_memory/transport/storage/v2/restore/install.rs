//! Explicit offline installation of current authority onto restored mobile state.
use super::*;

use crate::mobile_memory::{
    assurance, identity, owner_policy::OwnerPolicyStore, registry::RestoreRegistry,
};
use p256::pkcs8::DecodePrivateKey;
use std::collections::{BTreeMap, BTreeSet};

mod source;
use source::SnapshotSource;

fn journal_for_install(home: &Path, fingerprint: [u8; 32]) -> Result<Option<State>> {
    let directory = home.join("mobile");
    let marker = directory.join(MARKER);
    if exists(&marker)? {
        ensure!(
            read_private(&marker)?.as_slice() == MARKER_BYTES,
            "invalid mobile restore marker"
        );
    }
    let path = directory.join(JOURNAL);
    if !exists(&path)? {
        return Ok(None);
    }
    validate_private_file(&path)?;
    let mut bytes = Zeroizing::new(Vec::new());
    fs::File::open(&path)?
        .take(MAX_RESTORE_BYTES + 1)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() <= MAX_RESTORE_BYTES as usize,
        "mobile restore state exceeds limit"
    );
    let state: State = serde_json::from_slice(&bytes)?;
    state.validate()?;
    ensure!(
        state.installation_fingerprint == fingerprint,
        "mobile restore installation changed"
    );
    Ok(Some(state))
}
fn publish_marker(home: &Path) -> Result<()> {
    let path = home.join("mobile").join(MARKER);
    atomic_replace_private(&path, MARKER_BYTES)?;
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

/// Operates on an offline restored COVEN_HOME, never an unauthenticated remote
/// ingress. Snapshot copying is an offline operator prerequisite. All CLI/daemon
/// users hold the shared state lock; this exclusive lock rejects a live user.
#[cfg(test)]
pub(super) fn install(
    home: &Path,
    authority: &Path,
    fingerprint: [u8; 32],
    operation: Uuid,
) -> Result<State> {
    install_selected(home, None, authority, fingerprint, operation)
}

fn install_selected(
    home: &Path,
    snapshot: Option<&SnapshotSource>,
    authority: &Path,
    fingerprint: [u8; 32],
    operation: Uuid,
) -> Result<State> {
    ensure!(!operation.is_nil(), "restore operation id must not be nil");
    ensure_private_mobile_dir(home)?;
    crate::daemon::ensure_private_coven_home(home)?;
    let directory = cap_std::fs::Dir::open_ambient_dir(home, cap_std::ambient_authority())?;
    let _exclusive = crate::state_lock::try_acquire_exclusive_in(&directory, home)?
        .context("stop the daemon and other Coven commands before restoring mobile authority")?;
    crate::state_lock::refuse_interrupted_reset(&directory, home)?;
    let host = if let Some(snapshot) = snapshot {
        let host = snapshot.host()?;
        let previous = journal_for_install(home, fingerprint)?;
        let resuming = previous
            .as_ref()
            .is_some_and(|s| s.operation_id == operation);
        let key_exists = exists(&home.join("mobile/host-key.pem"))?;
        let certificate_exists = exists(&home.join("mobile/host-identity.json"))?;
        if key_exists && certificate_exists {
            match identity::load_existing_host_identity(home) {
                Ok(current) => ensure!(
                    current.public_key_fingerprint == fingerprint,
                    "destination installation differs from explicit pin"
                ),
                Err(error) if !resuming => return Err(error),
                Err(_) => {} // Interrupted identity installation stays fenced.
            }
        } else {
            ensure!(
                (!key_exists && !certificate_exists) || resuming,
                "partial destination identity requires the original restore operation"
            );
        }
        host
    } else {
        identity::load_existing_host_identity(home)?
    };
    ensure!(
        host.public_key_fingerprint == fingerprint,
        "restored installation does not match the explicit pin"
    );
    let signing = SigningKey::from_pkcs8_der(&host.private_key_der)?;
    export::check_output_namespace(
        home,
        authority
            .parent()
            .context("authority input needs a parent directory")?,
    )?;
    let input = export::read_authenticated(authority, signing.verifying_key())?;
    let canonical = Zeroizing::new(serde_json::to_vec(&input)?);
    let mut digest = ring::digest::Context::new(&ring::digest::SHA256);
    digest.update(b"coven.mobile.restore.input.v1\0");
    digest.update(&[u8::from(snapshot.is_some())]);
    if let Some(snapshot) = snapshot {
        digest.update(&snapshot.digest);
    }
    digest.update(&canonical);
    let source_digest: [u8; 32] = digest.finish().as_ref().try_into()?;
    let registry_bytes = Zeroizing::new(URL_SAFE_NO_PAD.decode(&input.registry)?);
    let incoming = RestoreRegistry::decode(&registry_bytes)?;
    ensure!(
        incoming.epoch() == input.installation_epoch,
        "export installation epoch disagrees with registry"
    );
    let keys = Zeroizing::new(URL_SAFE_NO_PAD.decode(&input.authorization_keys)?);
    let policy = match &input.policy {
        export::PolicyCapture::Absent => None,
        export::PolicyCapture::Present { encoded } => {
            Some(Zeroizing::new(URL_SAFE_NO_PAD.decode(encoded)?))
        }
    };
    let mut anchors = BTreeMap::new();
    for anchor in &input.routing {
        ensure!(
            anchors
                .insert(anchor.id(), anchor.counter(anchor.id()))
                .is_none(),
            "duplicate export routing anchor"
        );
    }
    let input_revisions = incoming.revisions();
    ensure!(
        anchors.keys().copied().collect::<BTreeSet<_>>()
            == input_revisions.iter().map(|r| r.device_id).collect(),
        "export routing anchors do not cover exact registry"
    );
    super::super::with_operator_lock(home, || {
        let previous = journal_for_install(home, fingerprint)?;
        let retained = RestoreRegistry::retained(home)?;
        let policy_store = OwnerPolicyStore::open(home, fingerprint)?;
        if let Some(snapshot) = snapshot {
            snapshot.validate_authority(
                &incoming,
                &keys,
                policy.as_deref().map(Vec::as_slice),
                &policy_store,
            )?;
        }
        let resuming = previous
            .as_ref()
            .is_some_and(|state| state.operation_id == operation);
        if resuming {
            let state = previous.as_ref().expect("resuming existing journal");
            ensure!(
                state.source_digest == source_digest,
                "restore retry requires the exact original authority source"
            );
            if state.phase == Phase::AwaitingHandovers {
                // Never reset counters or seeds after an uncertain final journal
                // result: a device may already have adopted its fresh handover.
                ensure!(
                    retained.epoch() == state.installation_epoch,
                    "restore registry dependency missing"
                );
                let revisions = retained.revisions();
                for target in &state.pairings {
                    if let Some(current) =
                        revisions.iter().find(|r| r.device_id == target.device_id)
                    {
                        ensure!(
                            current.revocation_epoch >= target.revocation_epoch,
                            "restore grant dependency below target"
                        );
                        let anchor = export::anchor(home, target.device_id)?;
                        ensure!(
                            anchor
                                .counter(target.device_id)
                                .is_some_and(|g| g >= target.generation - 1),
                            "restore routing dependency missing"
                        );
                    }
                }
                publish_marker(home)?;
                check_installation(home)?;
                return Ok(state.clone());
            }
        } else {
            ensure!(
                previous
                    .as_ref()
                    .is_none_or(|s| s.phase == Phase::AwaitingHandovers),
                "finish the existing restore operation before starting another"
            );
            incoming.check_not_older_than(&retained)?;
        }
        let mut state = if resuming {
            previous.context("restore journal disappeared")?
        } else {
            let retained_revisions = retained.revisions();
            let snapshot_revisions = snapshot.map(|s| s.registry.revisions()).unwrap_or_default();
            let previous_targets = previous
                .as_ref()
                .into_iter()
                .flat_map(|s| &s.pairings)
                .map(|p| export::retained_target(home, p))
                .collect::<Result<Vec<_>>>()?;
            let ids: BTreeSet<_> = input_revisions
                .iter()
                .chain(&retained_revisions)
                .chain(&snapshot_revisions)
                .map(|r| r.device_id)
                .chain(previous_targets.iter().map(|p| p.device_id))
                .chain(input.retained_targets.iter().map(|p| p.device_id))
                .chain(
                    snapshot
                        .into_iter()
                        .flat_map(|s| s.generations.keys().copied()),
                )
                .collect();
            let mut pairings = Vec::new();
            for id in ids {
                let retained_anchor = export::anchor(home, id)?;
                let generation = anchors
                    .get(&id)
                    .copied()
                    .flatten()
                    .into_iter()
                    .chain(retained_anchor.counter(id))
                    .chain(snapshot.and_then(|s| s.generations.get(&id).copied()))
                    .chain(
                        previous_targets
                            .iter()
                            .filter(|p| p.device_id == id)
                            .map(|p| p.generation),
                    )
                    .chain(
                        input
                            .retained_targets
                            .iter()
                            .filter(|p| p.device_id == id)
                            .map(|p| p.generation),
                    )
                    .chain(previous.as_ref().and_then(|s| {
                        s.pairings
                            .iter()
                            .find(|p| p.device_id == id)
                            .map(|p| p.generation)
                    }))
                    .max()
                    .unwrap_or(0)
                    .checked_add(1)
                    .context("routing re-pair required")?;
                let revocation_epoch = input_revisions
                    .iter()
                    .chain(&retained_revisions)
                    .chain(&snapshot_revisions)
                    .filter(|r| r.device_id == id)
                    .map(|r| r.revocation_epoch)
                    .chain(
                        previous_targets
                            .iter()
                            .filter(|p| p.device_id == id)
                            .map(|p| p.revocation_epoch),
                    )
                    .chain(
                        input
                            .retained_targets
                            .iter()
                            .filter(|p| p.device_id == id)
                            .map(|p| p.revocation_epoch),
                    )
                    .chain(snapshot.and_then(|s| s.revocation_epochs.get(&id).copied()))
                    .max()
                    .context("restore grant anchor missing")?
                    .checked_add(1)
                    .context("restore revocation epoch overflow; re-pair required")?;
                pairings.push(PairingTarget {
                    device_id: id,
                    generation,
                    revocation_epoch,
                    handover_complete: false,
                });
            }
            let installation_epoch = input
                .installation_epoch
                .max(retained.epoch())
                .max(snapshot.map_or(0, |s| s.registry.epoch()))
                .max(previous.as_ref().map_or(0, |s| s.installation_epoch))
                .checked_add(1)
                .context("restore installation epoch overflow")?;
            State {
                version: 1,
                operation_id: operation,
                source_digest,
                installation_fingerprint: fingerprint,
                installation_epoch,
                phase: Phase::Installing,
                pairings,
            }
        };
        state.validate()?;
        let epochs = state
            .pairings
            .iter()
            .map(|p| (p.device_id, p.revocation_epoch))
            .collect();
        let desired = incoming.advanced(state.installation_epoch, &epochs)?;
        if resuming && !desired.matches(&retained)? {
            incoming.check_not_older_than(&retained)?;
        }
        // Complete semantic preflight before publishing the first fence. The
        // exclusive state lock prevents CLI/daemon mutations between stages.
        assurance::restore_authorization_keys(home, &keys, false)?;
        policy_store.restore_snapshot(policy.as_deref().map(Vec::as_slice), false)?;
        // The journal goes first. Either orphan is rejected by admission; a
        // crash here cannot have changed any authority dependency yet.
        save(home, &state)?;
        publish_marker(home)?;
        if let Some(snapshot) = snapshot {
            snapshot.install(home)?;
        }
        assurance::restore_authorization_keys(home, &keys, true)?;
        policy_store.restore_snapshot(policy.as_deref().map(Vec::as_slice), true)?;
        desired.install(home)?;
        for target in &state.pairings {
            if resuming {
                // Only the original authenticated Installing operation may repair
                // a partially persisted O_CREAT protocol marker. Its durable
                // journal already reserves this pairing's generation/epoch and
                // the global fence blocks all handovers and application use.
                // Ordinary load/prepare and AwaitingHandovers never repair it.
                let marker = home
                    .join("mobile")
                    .join(format!(".transport-v2-{}.required", target.device_id));
                if exists(&marker)?
                    && read_private(&marker)?.as_slice() != super::super::PROTOCOL_FLOOR_BYTES
                {
                    atomic_replace_private(&marker, super::super::PROTOCOL_FLOOR_BYTES)?;
                }
            }
            super::super::require_v2(home, target.device_id)?;
            let stored = super::super::Stored {
                version: 2,
                minimum_generation: Some(target.generation - 1),
                committed: None,
                pending: None,
            };
            super::super::save(
                &home
                    .join("mobile")
                    .join(format!("transport-{}.json", target.device_id)),
                &stored,
            )?;
        }
        state.phase = Phase::AwaitingHandovers;
        save(home, &state)?;
        check_installation(home)?;
        Ok(state)
    })
}

fn install_from_snapshot(
    home: &Path,
    snapshot_home: &Path,
    authority: &Path,
    fingerprint: [u8; 32],
    operation: Uuid,
) -> Result<State> {
    let source = SnapshotSource::capture(snapshot_home)?;
    ensure!(
        source.host()?.public_key_fingerprint == fingerprint,
        "selected snapshot does not match explicit pin"
    );
    SnapshotSource::require_disjoint_homes(home, snapshot_home)?;
    crate::daemon::ensure_private_coven_home(home)?;
    ensure_private_mobile_dir(home)?;
    install_selected(home, Some(&source), authority, fingerprint, operation)
}

pub(crate) fn run_install(
    authority: &Path,
    snapshot_home: &Path,
    fingerprint: &str,
    operation: Uuid,
    json: bool,
) -> Result<()> {
    let pin = URL_SAFE_NO_PAD.decode(fingerprint)?;
    ensure!(
        URL_SAFE_NO_PAD.encode(&pin) == fingerprint,
        "installation fingerprint must be canonical base64url"
    );
    let pin: [u8; 32] = pin
        .try_into()
        .map_err(|_| anyhow::anyhow!("installation fingerprint must be 32 bytes"))?;
    let home = crate::coven_home_dir()?;
    let state = install_from_snapshot(&home, snapshot_home, authority, pin, operation)?;
    if json {
        println!(
            "{}",
            serde_json::json!({"operationId": state.operation_id, "installationEpoch": state.installation_epoch, "pairingCount": state.pairings.len(), "freshHandoversRequired": true})
        );
    } else {
        println!("Mobile authority restored (operation {}; epoch {}; {} pairings require fresh direct handover).", state.operation_id, state.installation_epoch, state.pairings.len());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mobile_memory::transport::storage::v2::tests::Fixture;

    #[test]
    fn restore_installer_reapplies_post_snapshot_revocation_and_advances_fixed_targets() {
        let f = Fixture::new();
        let old = f.prepare();
        f.commit(&old);
        let registry_path = f.home.path().join("mobile/devices.json");
        let snapshot_registry = fs::read(&registry_path).unwrap();
        f.registry.revoke(f.id, Utc::now()).unwrap();
        let backup = tempfile::tempdir().unwrap();
        let authority = ensure_private_mobile_dir(backup.path())
            .unwrap()
            .join("authority.json");
        export::export_authority(f.home.path(), &authority).unwrap();
        atomic_replace_private(&registry_path, &snapshot_registry).unwrap();
        let fingerprint =
            crate::mobile_memory::identity::load_existing_host_identity(f.home.path())
                .unwrap()
                .public_key_fingerprint;
        let operation = Uuid::new_v4();
        install(f.home.path(), &authority, fingerprint, operation).unwrap();
        let state = read(f.home.path()).unwrap().unwrap();
        assert_eq!(state.operation_id, operation);
        assert_eq!(state.installation_epoch, 1);
        assert!(state.phase == Phase::AwaitingHandovers);
        assert_eq!(state.pairings[0].generation, 1);
        assert_eq!(state.pairings[0].revocation_epoch, 2);
        assert!(!state.pairings[0].handover_complete);
        f.registry.reload().unwrap();
        let record = f.registry.authorization_record(f.id).unwrap().unwrap();
        assert!(record.device.revoked_at.is_some());
        assert_eq!(record.grant.revocation_epoch, 2);
        assert!(check_application(f.home.path(), &f.registry, f.id).is_err());
        assert!(prepare_direct(f.home.path(), &f.registry, &f.host, f.id).is_err());
        install(f.home.path(), &authority, fingerprint, operation).unwrap();
        let retried = read(f.home.path()).unwrap().unwrap();
        assert_eq!(retried.installation_epoch, 1);
        assert_eq!(retried.pairings[0].generation, 1);
    }

    #[test]
    fn restore_installer_keeps_active_pairing_fenced_until_fresh_direct_handover() {
        let f = Fixture::new();
        let old = f.prepare();
        f.commit(&old);
        let backup = tempfile::tempdir().unwrap();
        let authority = ensure_private_mobile_dir(backup.path())
            .unwrap()
            .join("authority.json");
        export::export_authority(f.home.path(), &authority).unwrap();
        let fingerprint =
            crate::mobile_memory::identity::load_existing_host_identity(f.home.path())
                .unwrap()
                .public_key_fingerprint;
        install(f.home.path(), &authority, fingerprint, Uuid::new_v4()).unwrap();
        assert!(check_application(f.home.path(), &f.registry, f.id).is_err());
        assert!(acknowledge_direct(
            f.home.path(),
            &f.registry,
            &f.host,
            f.id,
            &old.acknowledgment()
        )
        .is_err());
        let fresh = f.prepare();
        assert_eq!(fresh.generation, 1);
        assert_eq!(fresh.revocation_epoch, 1);
        assert_ne!(fresh.routing_seed, old.routing_seed);
        let retry = f.prepare();
        assert_eq!(retry.routing_seed, fresh.routing_seed);
        f.commit(&fresh);
        check_application(f.home.path(), &f.registry, f.id).unwrap();
        assert_eq!(f.snapshot().unwrap().floors, [0, 0]);
    }
    fn exported(f: &Fixture) -> (tempfile::TempDir, std::path::PathBuf, [u8; 32]) {
        let backup = tempfile::tempdir().unwrap();
        let path = ensure_private_mobile_dir(backup.path())
            .unwrap()
            .join("authority.json");
        export::export_authority(f.home.path(), &path).unwrap();
        let pin = identity::load_existing_host_identity(f.home.path())
            .unwrap()
            .public_key_fingerprint;
        (backup, path, pin)
    }

    #[test]
    fn restore_installer_retries_every_retained_directory_barrier_without_double_bumps() {
        for barrier in 0..14 {
            let f = Fixture::new();
            let old = f.prepare();
            f.commit(&old);
            let (_backup, path, pin) = exported(&f);
            let operation = Uuid::new_v4();
            crate::durable_fs::fail_directory_sync_after(barrier);
            let error = install(f.home.path(), &path, pin, operation).unwrap_err();
            assert!(
                format!("{error:#}").contains("injected directory sync failure"),
                "barrier {barrier}: {error:#}"
            );
            assert!(
                check_application(f.home.path(), &f.registry, f.id).is_err(),
                "barrier {barrier} admitted old authority"
            );
            let adopted = if check_installation(f.home.path()).is_ok() {
                Some(f.prepare())
            } else {
                None
            };
            install(f.home.path(), &path, pin, operation).unwrap();
            let fresh = f.prepare();
            assert_eq!(fresh.generation, 1, "barrier {barrier}");
            assert_eq!(fresh.revocation_epoch, 1, "barrier {barrier}");
            if let Some(adopted) = adopted {
                assert_eq!(
                    fresh.routing_seed, adopted.routing_seed,
                    "barrier {barrier} replaced an offered seed"
                );
            }
            f.commit(&fresh);
            check_application(f.home.path(), &f.registry, f.id).unwrap();
        }
    }

    #[test]
    fn restore_installer_refuses_live_state_holder_and_known_newer_revocation_before_fencing() {
        let f = Fixture::new();
        let (_backup, path, pin) = exported(&f);
        let shared = crate::state_lock::acquire_shared(f.home.path()).unwrap();
        let error = install(f.home.path(), &path, pin, Uuid::new_v4()).unwrap_err();
        assert!(format!("{error:#}").contains("stop the daemon"));
        assert!(!exists(&f.home.path().join("mobile").join(JOURNAL)).unwrap());
        drop(shared);
        f.registry.revoke(f.id, Utc::now()).unwrap();
        let error = install(f.home.path(), &path, pin, Uuid::new_v4()).unwrap_err();
        assert!(format!("{error:#}").contains("predates retained grant epoch"));
        assert!(!exists(&f.home.path().join("mobile").join(JOURNAL)).unwrap());
        assert!(f
            .registry
            .authorization_record(f.id)
            .unwrap()
            .unwrap()
            .device
            .revoked_at
            .is_some());
    }

    #[test]
    fn restore_installer_refuses_wrong_pin_and_tampered_source_without_changing_authority() {
        let f = Fixture::new();
        let (_backup, path, pin) = exported(&f);
        let before = fs::read(f.home.path().join("mobile/devices.json")).unwrap();
        let error = install(f.home.path(), &path, [0; 32], Uuid::new_v4()).unwrap_err();
        assert!(format!("{error:#}").contains("explicit pin"));
        let mut artifact: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        artifact["authorizationKeys"] = serde_json::json!("altered");
        atomic_replace_private(&path, &serde_json::to_vec(&artifact).unwrap()).unwrap();
        let error = install(f.home.path(), &path, pin, Uuid::new_v4()).unwrap_err();
        assert!(format!("{error:#}").contains("signature invalid"));
        assert_eq!(
            fs::read(f.home.path().join("mobile/devices.json")).unwrap(),
            before
        );
        assert!(!exists(&f.home.path().join("mobile").join(JOURNAL)).unwrap());
    }

    #[test]
    fn restore_export_includes_reserved_generation_before_any_fresh_handover() {
        let f = Fixture::new();
        let old = f.prepare();
        f.commit(&old);
        let (_backup, path, pin) = exported(&f);
        install(f.home.path(), &path, pin, Uuid::new_v4()).unwrap();
        let (_next_backup, next_path, _) = exported(&f);
        let next: serde_json::Value =
            serde_json::from_slice(&fs::read(next_path).unwrap()).unwrap();
        assert_eq!(
            next["routing"][0]["highestGeneration"], 1,
            "an unacknowledged restore target must survive another export/snapshot restore"
        );
    }
    #[test]
    fn restore_installer_does_not_bypass_interrupted_reset() {
        let f = Fixture::new();
        let (_backup, path, pin) = exported(&f);
        crate::daemon::ensure_private_coven_home(f.home.path()).unwrap();
        atomic_replace_private(
            &f.home
                .path()
                .join(crate::state_lock::RESET_TRANSACTION_FILE),
            b"{}\n",
        )
        .unwrap();
        let error = install(f.home.path(), &path, pin, Uuid::new_v4()).unwrap_err();
        assert!(format!("{error:#}").contains("interrupted reset"));
        assert!(!exists(&f.home.path().join("mobile").join(JOURNAL)).unwrap());
    }

    #[test]
    fn restore_installer_requires_original_source_and_operation_during_interruption() {
        let f = Fixture::new();
        let (_backup, path, pin) = exported(&f);
        let (_second_backup, second_path, _) = exported(&f);
        assert_ne!(fs::read(&path).unwrap(), fs::read(&second_path).unwrap());
        let operation = Uuid::new_v4();
        crate::durable_fs::fail_directory_sync_after(4);
        assert!(install(f.home.path(), &path, pin, operation).is_err());
        let error = install(f.home.path(), &second_path, pin, operation).unwrap_err();
        assert!(format!("{error:#}").contains("exact original authority source"));
        let error = install(f.home.path(), &path, pin, Uuid::new_v4()).unwrap_err();
        assert!(format!("{error:#}").contains("finish the existing restore"));
        install(f.home.path(), &path, pin, operation).unwrap();
    }
    #[test]
    fn restore_installer_does_not_clear_retained_suspension_from_an_older_export() {
        let f = Fixture::new();
        let (_backup, path, pin) = exported(&f);
        f.registry.suspend(f.id, Utc::now()).unwrap();
        let error = install(f.home.path(), &path, pin, Uuid::new_v4()).unwrap_err();
        assert!(format!("{error:#}").contains("retained grant"));
        assert!(!exists(&f.home.path().join("mobile").join(JOURNAL)).unwrap());
        assert!(f
            .registry
            .authorization_record(f.id)
            .unwrap()
            .unwrap()
            .device
            .suspended_at
            .is_some());
    }
}

#[cfg(test)]
mod snapshot_install_tests {
    mod process;
    use super::*;
    use crate::mobile_memory::transport::storage::v2::tests::Fixture;

    fn copy_snapshot(f: &Fixture) -> tempfile::TempDir {
        let snapshot = tempfile::tempdir().unwrap();
        let mobile = ensure_private_mobile_dir(snapshot.path()).unwrap();
        for name in ["host-key.pem", "host-identity.json", "devices.json"] {
            atomic_replace_private(
                &mobile.join(name),
                &fs::read(f.home.path().join("mobile").join(name)).unwrap(),
            )
            .unwrap();
        }
        let transport = format!("transport-{}.json", f.id);
        let marker = format!(".transport-v2-{}.required", f.id);
        for name in [transport, marker] {
            atomic_replace_private(
                &mobile.join(&name),
                &fs::read(f.home.path().join("mobile").join(&name)).unwrap(),
            )
            .unwrap();
        }
        snapshot
    }
    #[test]
    fn snapshot_command_installs_selected_identity_under_fence_and_reapplies_later_revocation() {
        let f = Fixture::new();
        let old = f.prepare();
        f.commit(&old);
        let snapshot = copy_snapshot(&f);
        let snapshot_before = fs::read(snapshot.path().join("mobile/devices.json")).unwrap();
        f.registry.revoke(f.id, Utc::now()).unwrap();
        let backup = tempfile::tempdir().unwrap();
        let authority = ensure_private_mobile_dir(backup.path())
            .unwrap()
            .join("authority.json");
        export::export_authority(f.home.path(), &authority).unwrap();
        let pin = identity::load_existing_host_identity(snapshot.path())
            .unwrap()
            .public_key_fingerprint;
        let destination = tempfile::tempdir().unwrap();
        ensure_private_mobile_dir(destination.path()).unwrap();
        assert!(!destination.path().join("mobile/host-key.pem").exists());
        let operation = Uuid::new_v4();
        let state = install_from_snapshot(
            destination.path(),
            snapshot.path(),
            &authority,
            pin,
            operation,
        )
        .unwrap();
        assert!(state.phase == Phase::AwaitingHandovers);
        assert_eq!(state.installation_epoch, 1);
        assert_eq!(state.pairings[0].generation, 1);
        assert_eq!(state.pairings[0].revocation_epoch, 2);
        assert_eq!(
            identity::load_existing_host_identity(destination.path())
                .unwrap()
                .public_key_fingerprint,
            pin
        );
        let registry = DeviceRegistry::load(destination.path()).unwrap();
        let record = registry.authorization_record(f.id).unwrap().unwrap();
        assert!(record.device.revoked_at.is_some());
        assert_eq!(record.grant.revocation_epoch, 2);
        assert!(check_application(destination.path(), &registry, f.id).is_err());
        assert_eq!(
            fs::read(snapshot.path().join("mobile/devices.json")).unwrap(),
            snapshot_before
        );
        install_from_snapshot(
            destination.path(),
            snapshot.path(),
            &authority,
            pin,
            operation,
        )
        .unwrap();
    }
    struct Case {
        f: Fixture,
        snapshot: tempfile::TempDir,
        _backup: tempfile::TempDir,
        authority: std::path::PathBuf,
        pin: [u8; 32],
    }
    fn case() -> Case {
        let f = Fixture::new();
        let old = f.prepare();
        f.commit(&old);
        let snapshot = copy_snapshot(&f);
        let backup = tempfile::tempdir().unwrap();
        let authority = ensure_private_mobile_dir(backup.path())
            .unwrap()
            .join("authority.json");
        export::export_authority(f.home.path(), &authority).unwrap();
        let pin = identity::load_existing_host_identity(snapshot.path())
            .unwrap()
            .public_key_fingerprint;
        Case {
            f,
            snapshot,
            _backup: backup,
            authority,
            pin,
        }
    }
    fn inventory(home: &Path) -> BTreeMap<String, Vec<u8>> {
        fs::read_dir(home.join("mobile"))
            .unwrap()
            .map(|entry| {
                let entry = entry.unwrap();
                let name = entry.file_name().into_string().unwrap();
                let bytes = fs::read(entry.path()).unwrap();
                (
                    name,
                    ring::digest::digest(&ring::digest::SHA256, &bytes)
                        .as_ref()
                        .to_vec(),
                )
            })
            .collect()
    }
    #[test]
    fn snapshot_command_retries_every_directory_barrier_and_never_copies_before_fence() {
        for barrier in 0..22 {
            let c = case();
            let before = inventory(c.snapshot.path());
            let destination = tempfile::tempdir().unwrap();
            ensure_private_mobile_dir(destination.path()).unwrap();
            let operation = Uuid::new_v4();
            crate::durable_fs::fail_directory_sync_after(barrier);
            let error = install_from_snapshot(
                destination.path(),
                c.snapshot.path(),
                &c.authority,
                c.pin,
                operation,
            )
            .unwrap_err();
            assert!(
                format!("{error:#}").contains("injected directory sync failure"),
                "barrier {barrier}: {error:#}"
            );
            if barrier < 4 {
                assert!(
                    !destination.path().join("mobile/host-key.pem").exists(),
                    "snapshot key copied before fence at barrier {barrier}"
                );
            }
            let registry = DeviceRegistry::load(destination.path()).unwrap();
            assert!(check_application(destination.path(), &registry, c.f.id).is_err());
            let offered = if check_installation(destination.path()).is_ok() {
                Some(prepare_direct(destination.path(), &registry, &c.f.host, c.f.id).unwrap())
            } else {
                None
            };
            let state = install_from_snapshot(
                destination.path(),
                c.snapshot.path(),
                &c.authority,
                c.pin,
                operation,
            )
            .unwrap();
            assert_eq!(state.installation_epoch, 1, "barrier {barrier}");
            assert_eq!(state.pairings[0].generation, 1, "barrier {barrier}");
            assert_eq!(state.pairings[0].revocation_epoch, 1, "barrier {barrier}");
            let fresh = prepare_direct(destination.path(), &registry, &c.f.host, c.f.id).unwrap();
            if let Some(offered) = offered {
                assert!(
                    fresh.routing_seed == offered.routing_seed,
                    "barrier {barrier} replaced offered seed"
                );
            }
            let device_home = tempfile::tempdir().unwrap();
            let ack = super::super::super::device::adopt_direct(
                device_home.path(),
                fresh,
                c.f.host.verifying_key(),
                c.f.id,
                c.f.device.verifying_key(),
            )
            .unwrap();
            acknowledge_direct(destination.path(), &registry, &c.f.host, c.f.id, &ack).unwrap();
            check_application(destination.path(), &registry, c.f.id).unwrap();
            assert_eq!(
                inventory(c.snapshot.path()),
                before,
                "barrier {barrier} mutated selected snapshot"
            );
        }
    }
    #[test]
    fn snapshot_command_rejects_changed_snapshot_on_partial_identity_retry() {
        let c = case();
        let destination = tempfile::tempdir().unwrap();
        ensure_private_mobile_dir(destination.path()).unwrap();
        let operation = Uuid::new_v4();
        crate::durable_fs::fail_directory_sync_after(4);
        assert!(install_from_snapshot(
            destination.path(),
            c.snapshot.path(),
            &c.authority,
            c.pin,
            operation
        )
        .is_err());
        assert!(destination.path().join("mobile/host-key.pem").exists());
        assert!(!destination
            .path()
            .join("mobile/host-identity.json")
            .exists());
        let source_path = c.snapshot.path().join("mobile/devices.json");
        let before = fs::read(&source_path).unwrap();
        let mut changed: serde_json::Value = serde_json::from_slice(&before).unwrap();
        changed["devices"][0]["device"]["displayName"] =
            serde_json::json!("Different selected snapshot");
        atomic_replace_private(&source_path, &serde_json::to_vec(&changed).unwrap()).unwrap();
        let error = install_from_snapshot(
            destination.path(),
            c.snapshot.path(),
            &c.authority,
            c.pin,
            operation,
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("exact original authority source"));
        assert!(!destination
            .path()
            .join("mobile/host-identity.json")
            .exists());
        atomic_replace_private(&source_path, &before).unwrap();
        install_from_snapshot(
            destination.path(),
            c.snapshot.path(),
            &c.authority,
            c.pin,
            operation,
        )
        .unwrap();
    }
    #[test]
    fn snapshot_command_rejects_overlapping_homes_without_creating_source_artifacts() {
        let c = case();
        let nested = c.snapshot.path().join("nested-destination");
        fs::create_dir(&nested).unwrap();
        let before = inventory(c.snapshot.path());
        for target in [
            c.snapshot.path().to_owned(),
            c.snapshot.path().join("nested-destination"),
        ] {
            let error = install_from_snapshot(
                &target,
                c.snapshot.path(),
                &c.authority,
                c.pin,
                Uuid::new_v4(),
            )
            .unwrap_err();
            assert!(
                format!("{error:#}").contains("non-overlapping homes"),
                "target {target:?}: {error:#}"
            );
        }
        assert!(
            !nested.join("mobile").exists(),
            "rejected restore created artifacts inside its selected snapshot"
        );
        assert_eq!(inventory(c.snapshot.path()), before);
    }
    #[test]
    fn successive_snapshot_restores_advance_journal_only_pairing_epochs() {
        let c = case();
        c.f.registry.forget_all().unwrap();
        let empty_authority = c._backup.path().join("mobile/empty-authority.json");
        export::export_authority(c.f.home.path(), &empty_authority).unwrap();
        let destination = tempfile::tempdir().unwrap();
        let first = install_from_snapshot(
            destination.path(),
            c.snapshot.path(),
            &empty_authority,
            c.pin,
            Uuid::new_v4(),
        )
        .unwrap();
        assert_eq!(first.pairings.len(), 1);
        assert_eq!(first.pairings[0].revocation_epoch, 1);
        assert_eq!(
            RestoreRegistry::retained(destination.path())
                .unwrap()
                .revisions()
                .len(),
            0
        );
        let next_authority = c._backup.path().join("mobile/next-authority.json");
        export::export_authority(destination.path(), &next_authority).unwrap();
        let second = install_from_snapshot(
            destination.path(),
            c.snapshot.path(),
            &next_authority,
            c.pin,
            Uuid::new_v4(),
        )
        .unwrap();
        assert_eq!(second.pairings.len(), 1);
        assert_eq!(second.pairings[0].generation, 2);
        assert_eq!(
            second.pairings[0].revocation_epoch, 2,
            "a journal-only pairing's retained revocation floor was reused"
        );
    }

    #[test]
    fn selected_snapshot_preserves_journal_only_pairing_floors_in_a_fresh_destination() {
        let c = case();
        c.f.registry.forget_all().unwrap();
        let empty_authority = c._backup.path().join("mobile/empty-authority.json");
        export::export_authority(c.f.home.path(), &empty_authority).unwrap();
        let first_destination = tempfile::tempdir().unwrap();
        install_from_snapshot(
            first_destination.path(),
            c.snapshot.path(),
            &empty_authority,
            c.pin,
            Uuid::new_v4(),
        )
        .unwrap();
        let next_authority = c._backup.path().join("mobile/next-authority.json");
        export::export_authority(first_destination.path(), &next_authority).unwrap();
        let snapshot = tempfile::tempdir().unwrap();
        let mobile = ensure_private_mobile_dir(snapshot.path()).unwrap();
        for entry in fs::read_dir(first_destination.path().join("mobile")).unwrap() {
            let entry = entry.unwrap();
            assert!(entry.file_type().unwrap().is_file());
            atomic_replace_private(
                &mobile.join(entry.file_name()),
                &fs::read(entry.path()).unwrap(),
            )
            .unwrap();
        }
        let destination = tempfile::tempdir().unwrap();
        let second = install_from_snapshot(
            destination.path(),
            snapshot.path(),
            &next_authority,
            c.pin,
            Uuid::new_v4(),
        )
        .unwrap();
        assert_eq!(second.installation_epoch, 2);
        assert_eq!(
            second.pairings.len(),
            1,
            "selected snapshot journal-only pairing was dropped"
        );
        assert_eq!(second.pairings[0].generation, 2);
        assert_eq!(second.pairings[0].revocation_epoch, 2);
    }

    #[test]
    fn latest_authority_export_preserves_journal_only_floors_with_an_older_snapshot() {
        let c = case();
        c.f.registry.forget_all().unwrap();
        let empty_authority = c._backup.path().join("mobile/empty-authority.json");
        export::export_authority(c.f.home.path(), &empty_authority).unwrap();
        let first_destination = tempfile::tempdir().unwrap();
        install_from_snapshot(
            first_destination.path(),
            c.snapshot.path(),
            &empty_authority,
            c.pin,
            Uuid::new_v4(),
        )
        .unwrap();
        let next_authority = c._backup.path().join("mobile/next-authority.json");
        export::export_authority(first_destination.path(), &next_authority).unwrap();
        let destination = tempfile::tempdir().unwrap();
        let second = install_from_snapshot(
            destination.path(),
            c.snapshot.path(),
            &next_authority,
            c.pin,
            Uuid::new_v4(),
        )
        .unwrap();
        assert_eq!(second.installation_epoch, 2);
        assert_eq!(second.pairings.len(), 1);
        assert_eq!(
            second.pairings[0].generation, 2,
            "latest signed authority forgot a retained generation floor"
        );
        assert_eq!(
            second.pairings[0].revocation_epoch, 2,
            "latest signed authority forgot a retained revocation floor"
        );
    }
    #[test]
    fn selected_snapshot_rejects_certificate_key_mismatch_before_fencing() {
        let c = case();
        let other = Fixture::new();
        let path = c.snapshot.path().join("mobile/host-identity.json");
        let mut identity: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        let unrelated: serde_json::Value = serde_json::from_slice(
            &fs::read(other.home.path().join("mobile/host-identity.json")).unwrap(),
        )
        .unwrap();
        identity["certificatePem"] = unrelated["certificatePem"].clone();
        atomic_replace_private(&path, &serde_json::to_vec(&identity).unwrap()).unwrap();
        let destination = tempfile::tempdir().unwrap();
        let error = install_from_snapshot(
            destination.path(),
            c.snapshot.path(),
            &c.authority,
            c.pin,
            Uuid::new_v4(),
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("TLS identity"), "{error:#}");
        assert!(
            !destination.path().join("mobile").exists(),
            "invalid snapshot created a destination before preflight completed"
        );
    }
}

#[cfg(test)]
mod snapshot_tests;

#[cfg(test)]
pub(super) fn install_selected_snapshot_fixture(
    home: &Path,
    snapshot_home: &Path,
    authority: &Path,
    fingerprint: [u8; 32],
    operation: Uuid,
) -> Result<State> {
    install_from_snapshot(home, snapshot_home, authority, fingerprint, operation)
}
