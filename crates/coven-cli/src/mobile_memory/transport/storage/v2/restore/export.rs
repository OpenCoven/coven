//! Installation-signed current-authority export; authenticity does not establish freshness.
use super::*;
use p256::pkcs8::DecodePrivateKey;

use crate::mobile_memory::{assurance, config, identity, owner_policy::OwnerPolicyStore, registry};

const MAX_EXPORT_BYTES: usize = 20 * 1024 * 1024;

#[derive(Serialize, Deserialize, Zeroize, ZeroizeOnDrop)]
#[serde(tag = "profile", rename_all = "camelCase", deny_unknown_fields)]
pub(super) enum RoutingAnchor {
    Unprovisioned {
        #[zeroize(skip)]
        #[serde(rename = "deviceId")]
        device_id: Uuid,
    },
    Legacy {
        #[zeroize(skip)]
        #[serde(rename = "deviceId")]
        device_id: Uuid,
    },
    Counter {
        #[zeroize(skip)]
        #[serde(rename = "deviceId")]
        device_id: Uuid,
        #[serde(rename = "highestGeneration")]
        highest_generation: u64,
    },
}
impl RoutingAnchor {
    pub(super) fn id(&self) -> Uuid {
        match self {
            Self::Unprovisioned { device_id }
            | Self::Legacy { device_id }
            | Self::Counter { device_id, .. } => *device_id,
        }
    }
    pub(super) fn counter(&self, id: Uuid) -> Option<u64> {
        match self {
            Self::Counter {
                device_id,
                highest_generation,
            } if *device_id == id => Some(*highest_generation),
            _ => None,
        }
    }
}
#[derive(Serialize, Deserialize, Zeroize, ZeroizeOnDrop)]
#[serde(tag = "state", rename_all = "camelCase", deny_unknown_fields)]
pub(super) enum PolicyCapture {
    Absent,
    Present { encoded: String },
}
#[derive(Serialize, Deserialize, Zeroize, ZeroizeOnDrop)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(super) struct RestoreTarget {
    #[zeroize(skip)]
    pub(super) device_id: Uuid,
    pub(super) generation: u64,
    pub(super) revocation_epoch: u64,
}
/// Include offered material as well as the journal's reserved floor. A pairing
/// forgotten from the registry must not lose already-known generation/epoch data.
pub(super) fn retained_target(home: &Path, target: &PairingTarget) -> Result<RestoreTarget> {
    let highest = anchor(home, target.device_id)?
        .counter(target.device_id)
        .context("restore target routing floor missing")?;
    ensure!(
        highest >= target.generation - u64::from(!target.handover_complete),
        "restore target routing dependency incomplete"
    );
    let stored = super::super::read(
        &home
            .join("mobile")
            .join(format!("transport-{}.json", target.device_id)),
        target.device_id,
    )?
    .context("restore target routing material missing")?;
    let epoch = stored
        .committed
        .iter()
        .chain(stored.pending.iter())
        .map(|g| g.material.revocation_epoch)
        .chain(Some(target.revocation_epoch))
        .max()
        .context("restore epoch floor missing")?;
    Ok(RestoreTarget {
        device_id: target.device_id,
        generation: highest.max(target.generation),
        revocation_epoch: epoch,
    })
}
#[derive(Serialize, Deserialize, Zeroize, ZeroizeOnDrop)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(super) struct AuthorityExport {
    pub(super) format_version: u16,
    pub(super) installation_fingerprint: [u8; 32],
    pub(super) installation_epoch: u64,
    #[zeroize(skip)]
    pub(super) captured_at: chrono::DateTime<Utc>,
    pub(super) registry: String,
    pub(super) authorization_keys: String,
    pub(super) policy: PolicyCapture,
    pub(super) routing: Vec<RoutingAnchor>,
    pub(super) retained_targets: Vec<RestoreTarget>,
    pub(super) signature: Vec<u8>,
}
impl AuthorityExport {
    fn signature_message(&self) -> Result<Vec<u8>> {
        // Format 1 signs this ordered, typed JSON tuple. Integer fields retain
        // their exact u64 values, including values above the JavaScript range.
        let payload = Zeroizing::new(serde_json::to_vec(&(
            self.format_version,
            self.installation_fingerprint,
            self.installation_epoch,
            self.captured_at,
            &self.registry,
            &self.authorization_keys,
            &self.policy,
            &self.routing,
            &self.retained_targets,
        ))?);
        let mut message = b"coven.mobile.restore.authority.v1\0".to_vec();
        message.extend_from_slice(ring::digest::digest(&ring::digest::SHA256, &payload).as_ref());
        Ok(message)
    }
    fn authenticate(&self, pinned: &VerifyingKey) -> Result<()> {
        ensure!(
            self.format_version == 1,
            "unsupported authority export version"
        );
        let fingerprint = ring::digest::digest(
            &ring::digest::SHA256,
            pinned.to_sec1_point(false).as_bytes(),
        );
        ensure!(
            self.installation_fingerprint.as_slice() == fingerprint.as_ref(),
            "authority export installation mismatch"
        );
        crate::mobile_memory::transport::verify(
            pinned,
            &self.signature_message()?,
            &self.signature,
        )
        .context("authority export signature invalid")?;
        ensure!(
            self.retained_targets.len() <= MAX_PAIRINGS,
            "too many exported restore targets"
        );
        ensure!(
            self.installation_epoch > 0 || self.retained_targets.is_empty(),
            "legacy export contains restore targets"
        );
        let mut seen = std::collections::BTreeSet::new();
        for target in &self.retained_targets {
            ensure!(
                seen.insert(target.device_id)
                    && target.generation > 0
                    && target.revocation_epoch > 0,
                "invalid or duplicate exported restore target"
            );
        }
        Ok(())
    }
}
pub(super) fn read_authenticated(path: &Path, pinned: &VerifyingKey) -> Result<AuthorityExport> {
    validate_private_file(path)?;
    let mut bytes = Zeroizing::new(Vec::new());
    fs::File::open(path)?
        .take(MAX_EXPORT_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() <= MAX_EXPORT_BYTES,
        "authority export exceeds size limit"
    );
    let export: AuthorityExport = serde_json::from_slice(&bytes)?;
    export.authenticate(pinned)?;
    Ok(export)
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ExportReport {
    format_version: u16,
    pairing_count: usize,
    installation_epoch: u64,
    sha256: String,
}

pub(super) fn anchor(home: &Path, id: Uuid) -> Result<RoutingAnchor> {
    let directory = home.join("mobile");
    let path = directory.join(format!("transport-{id}.json"));
    let marker = directory.join(format!(".transport-v2-{id}.required"));
    let marked = exists(&marker)?;
    if marked {
        ensure!(
            read_private(&marker)?.as_slice() == b"coven.mobile.routing.v2\n",
            "invalid counter protocol floor"
        );
    }
    if let Some(stored) = super::super::read(&path, id)? {
        ensure!(
            marked,
            "counter protocol floor missing; explicit recovery required"
        );
        let highest_generation = stored
            .committed
            .as_ref()
            .map(|generation| generation.generation)
            .into_iter()
            .chain(
                stored
                    .pending
                    .as_ref()
                    .map(|generation| generation.generation),
            )
            .chain(stored.minimum_generation)
            .max()
            .context("counter generation unavailable")?;
        Ok(RoutingAnchor::Counter {
            device_id: id,
            highest_generation,
        })
    } else {
        ensure!(
            !marked,
            "counter generation unavailable; export cannot guess state"
        );
        Ok(if exists(&path)? {
            RoutingAnchor::Legacy { device_id: id }
        } else {
            RoutingAnchor::Unprovisioned { device_id: id }
        })
    }
}
fn capture(home: &Path) -> Result<(Zeroizing<Vec<u8>>, ExportReport, VerifyingKey)> {
    let directory = home.join("mobile");
    validate_private_directory(&directory)?;
    ensure!(
        config::validate_restore_export_component(&directory.join("host-identity.json"))?,
        "installation identity missing"
    );
    let host = identity::load_existing_host_identity(home)?;
    super::super::with_operator_lock(home, || {
        let policy = OwnerPolicyStore::open(home, host.public_key_fingerprint)?;
        policy.with_restore_snapshot(|policy| {
            registry::with_restore_snapshot(home, |registry| {
                assurance::with_restore_snapshot(home, |keys| {
                    super::check_installation(home)?;
                    let mut routing = registry
                        .devices
                        .iter()
                        .map(|device| anchor(home, device.device_id))
                        .collect::<Result<Vec<_>>>()?;
                    let mut retained_targets = Vec::new();
                    if let Some(state) = super::read(home)? {
                        for target in state.pairings {
                            retained_targets.push(retained_target(home, &target)?);
                            if let Some(current) = registry
                                .devices
                                .iter()
                                .find(|current| current.device_id == target.device_id)
                            {
                                ensure!(
                                    current.revocation_epoch >= target.revocation_epoch
                                        && routing
                                            .iter()
                                            .find_map(|anchor| anchor.counter(target.device_id))
                                            .is_some_and(|highest| highest
                                                >= target.generation
                                                    - u64::from(!target.handover_complete)),
                                    "restore authority dependencies incomplete"
                                );
                                // The journal durably reserves this generation
                                // even before a direct handover creates its seed.
                                if let Some(RoutingAnchor::Counter {
                                    highest_generation, ..
                                }) = routing.iter_mut().find(|a| a.id() == target.device_id)
                                {
                                    *highest_generation =
                                        (*highest_generation).max(target.generation);
                                }
                            }
                        }
                    }
                    let current = identity::load_existing_host_identity(home)?;
                    ensure!(
                        current.public_key_fingerprint == host.public_key_fingerprint,
                        "installation changed during export"
                    );
                    let mut export = AuthorityExport {
                        format_version: 1,
                        installation_fingerprint: host.public_key_fingerprint,
                        installation_epoch: registry.restore_epoch.unwrap_or(0),
                        captured_at: Utc::now(),
                        registry: URL_SAFE_NO_PAD.encode(registry.encoded.as_slice()),
                        authorization_keys: URL_SAFE_NO_PAD.encode(keys.as_slice()),
                        policy: policy.map_or(PolicyCapture::Absent, |encoded| {
                            PolicyCapture::Present {
                                encoded: URL_SAFE_NO_PAD.encode(encoded.as_slice()),
                            }
                        }),
                        routing,
                        retained_targets,
                        signature: Vec::new(),
                    };
                    let signing = SigningKey::from_pkcs8_der(&host.private_key_der)?;
                    export.signature = crate::mobile_memory::transport::sign(
                        &signing,
                        &export.signature_message()?,
                    );
                    let encoded = Zeroizing::new(serde_json::to_vec(&export)?);
                    ensure!(
                        encoded.len() <= MAX_EXPORT_BYTES,
                        "authority export exceeds size limit"
                    );
                    let digest = ring::digest::digest(&ring::digest::SHA256, &encoded);
                    let report = ExportReport {
                        format_version: 1,
                        pairing_count: registry.devices.len(),
                        installation_epoch: export.installation_epoch,
                        sha256: digest
                            .as_ref()
                            .iter()
                            .map(|byte| format!("{byte:02x}"))
                            .collect(),
                    };
                    Ok((encoded, report, *signing.verifying_key()))
                })
            })
        })
    })
}
pub(super) fn check_output_namespace(home: &Path, parent: &Path) -> Result<()> {
    use cap_fs_ext::MetadataExt;
    // Compare directory identity, not case-sensitive path text. This also
    // catches alternate spellings and ordinary aliases on Windows.
    let source =
        cap_std::fs::Dir::open_ambient_dir(home.join("mobile"), cap_std::ambient_authority())?;
    let source = source.dir_metadata()?;
    let parent = fs::canonicalize(parent)?;
    for ancestor in parent.ancestors() {
        let directory = cap_std::fs::Dir::open_ambient_dir(ancestor, cap_std::ambient_authority())?;
        let metadata = directory.dir_metadata()?;
        ensure!(
            source.dev() != metadata.dev() || source.ino() != metadata.ino(),
            "authority export output must be outside mobile state"
        );
    }
    Ok(())
}
pub(super) fn export_authority(home: &Path, output: &Path) -> Result<ExportReport> {
    let parent = output
        .parent()
        .context("authority export needs a private parent directory")?;
    validate_private_directory(parent)?;
    ensure!(!exists(output)?, "authority export output already exists");
    check_output_namespace(home, parent)?;
    let (encoded, report, pinned) = capture(home)?;
    ensure!(
        config::atomic_create_private(output, &encoded)?,
        "authority export output already exists"
    );
    crate::durable_fs::sync_file(&fs::File::open(output)?)?;
    #[cfg(test)]
    config::write_checkpoint(output, config::MobileWritePhase::FileSynced)?;
    crate::durable_fs::sync_parent(output)?;
    #[cfg(test)]
    config::write_checkpoint(output, config::MobileWritePhase::RetainedParentSynced)?;
    crate::durable_fs::sync_parent(parent)?;
    #[cfg(test)]
    config::write_checkpoint(output, config::MobileWritePhase::RetainedHomeSynced)?;
    read_authenticated(output, &pinned)?;
    let mut written = Zeroizing::new(Vec::new());
    fs::File::open(output)?
        .take(MAX_EXPORT_BYTES as u64 + 1)
        .read_to_end(&mut written)?;
    ensure!(
        written.as_slice() == encoded.as_slice(),
        "authority export changed after publication"
    );
    Ok(report)
}
pub(crate) fn run_export(output: &Path, json: bool) -> Result<()> {
    let report = export_authority(&crate::coven_home_dir()?, output)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!(
            "Mobile authority export saved ({} pairings; SHA-256 {}).",
            report.pairing_count, report.sha256
        );
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::mobile_memory::assurance::{
        AssuranceClass, AuthorizationKeyRegistry, NewAuthorizationKey,
    };
    use crate::mobile_memory::transport::storage::v2::tests::Fixture;

    #[test]
    fn authority_export_authentication_rejects_wrong_root_and_every_metadata_change() {
        let f = Fixture::new();
        let (encoded, _, pinned) = capture(f.home.path()).unwrap();
        let original: serde_json::Value = serde_json::from_slice(&encoded).unwrap();
        let valid: AuthorityExport = serde_json::from_slice(&encoded).unwrap();
        valid.authenticate(&pinned).unwrap();
        let wrong = SigningKey::from_slice(&[31; 32]).unwrap();
        assert!(valid.authenticate(wrong.verifying_key()).is_err());
        let changes = [
            ("formatVersion", serde_json::json!(2)),
            ("installationFingerprint", serde_json::json!(vec![0; 32])),
            ("installationEpoch", serde_json::json!(1)),
            ("capturedAt", serde_json::json!("2000-01-01T00:00:00Z")),
            ("registry", serde_json::json!("altered")),
            ("authorizationKeys", serde_json::json!("altered")),
            (
                "policy",
                serde_json::json!({"state":"present", "encoded":"altered"}),
            ),
            ("routing", serde_json::json!([])),
            (
                "retainedTargets",
                serde_json::json!([{"deviceId": f.id, "generation": 1, "revocationEpoch": 1}]),
            ),
            ("signature", serde_json::json!([])),
        ];
        for (field, changed) in changes {
            let mut value = original.clone();
            value[field] = changed;
            let changed: AuthorityExport = serde_json::from_value(value).unwrap();
            assert!(
                changed.authenticate(&pinned).is_err(),
                "accepted changed {field}"
            );
        }
        let mut unsigned = original;
        unsigned.as_object_mut().unwrap().remove("signature");
        assert!(serde_json::from_value::<AuthorityExport>(unsigned).is_err());
    }

    #[test]
    fn authority_export_signature_preserves_full_width_epochs_and_domain() {
        let f = Fixture::new();
        let (encoded, _, pinned) = capture(f.home.path()).unwrap();
        let mut export: AuthorityExport = serde_json::from_slice(&encoded).unwrap();
        let host = identity::load_existing_host_identity(f.home.path()).unwrap();
        let key = SigningKey::from_pkcs8_der(&host.private_key_der).unwrap();
        for epoch in [(1_u64 << 53) + 1, u64::MAX] {
            export.installation_epoch = epoch;
            export.signature =
                crate::mobile_memory::transport::sign(&key, &export.signature_message().unwrap());
            export.authenticate(&pinned).unwrap();
            export.installation_epoch = epoch - 1;
            assert!(
                export.authenticate(&pinned).is_err(),
                "signature did not distinguish {epoch}"
            );
        }
        export.retained_targets.push(RestoreTarget {
            device_id: f.id,
            generation: (1_u64 << 53) + 1,
            revocation_epoch: 1,
        });
        export.signature =
            crate::mobile_memory::transport::sign(&key, &export.signature_message().unwrap());
        export.authenticate(&pinned).unwrap();
        export.retained_targets[0].generation -= 1;
        assert!(
            export.authenticate(&pinned).is_err(),
            "signature rounded a retained generation floor"
        );
        let mut wrong_domain = export.signature_message().unwrap();
        wrong_domain[0] ^= 1;
        export.signature = crate::mobile_memory::transport::sign(&key, &wrong_domain);
        assert!(export.authenticate(&pinned).is_err());
    }

    pub(crate) fn install_policy(f: &Fixture) {
        use crate::mobile_memory::assurance::RequestedAssurance;
        use crate::mobile_memory::grant::DeviceScope;
        use crate::mobile_memory::introduction::IntroductionPolicy;
        use crate::mobile_memory::owner_policy::PolicyProposal;
        use p256::ecdsa::{signature::Signer, Signature};
        let fingerprint = identity::load_existing_host_identity(f.home.path())
            .unwrap()
            .public_key_fingerprint;
        let store = OwnerPolicyStore::open(f.home.path(), fingerprint).unwrap();
        let root = SigningKey::from_slice(&[11; 32]).unwrap();
        let public = URL_SAFE_NO_PAD.encode(root.verifying_key().to_sec1_point(false).as_bytes());
        let now = Utc::now();
        let current = store.current().unwrap();
        let revision = current.as_ref().map_or(1, |p| p.revision + 1);
        let proposal = PolicyProposal {
            revision,
            previous_digest: current.map(|p| p.digest),
            policy: IntroductionPolicy {
                eligible_introducers: vec![f.id],
                required_approvals: 1,
                allowed_scopes: vec![DeviceScope::MemoryRead],
                maximum_grant_lifetime_seconds: 86400 / u32::try_from(revision).unwrap(),
                required_assurance: RequestedAssurance::FreshUserVerification,
                maximum_step_up_age_seconds: 60,
                minimum_attestation: Default::default(),
            },
            nonce: [4 + u8::try_from(revision).unwrap(); 32],
            issued_at: now,
            expires_at: now + chrono::Duration::seconds(60),
        };
        let action = if revision == 1 {
            store.install_action(&public, &proposal).unwrap()
        } else {
            store.replace_action(&proposal).unwrap()
        };
        let signature: Signature = root.sign(&action.canonical_bytes().unwrap());
        let signature = URL_SAFE_NO_PAD.encode(signature.to_der().as_bytes());
        if revision == 1 {
            store.install(&public, &proposal, &signature, now).unwrap();
        } else {
            store
                .replace(
                    &proposal,
                    &crate::mobile_memory::owner_policy::PolicyApproval::OwnerRoot { signature },
                    now,
                )
                .unwrap();
        }
    }

    #[test]
    fn authority_export_preserves_revocations_and_highest_offered_generation() {
        let f = Fixture::new();
        let first = f.prepare();
        f.commit(&first);
        let pending = f.prepare();
        assert_eq!(pending.generation, 1);
        let key = SigningKey::from_slice(&[9; 32]).unwrap();
        let possession =
            URL_SAFE_NO_PAD.encode(f.device.verifying_key().to_sec1_point(false).as_bytes());
        AuthorizationKeyRegistry::load(f.home.path())
            .unwrap()
            .enroll_initial(
                f.id,
                &possession,
                NewAuthorizationKey {
                    public_key_x963: URL_SAFE_NO_PAD
                        .encode(key.verifying_key().to_sec1_point(false).as_bytes()),
                    assurance_class: AssuranceClass::BiometricOnly,
                    enrolled_at: Utc::now(),
                },
            )
            .unwrap();
        let old_registry = fs::read(f.home.path().join("mobile/devices.json")).unwrap();
        let old_keys = fs::read(f.home.path().join("mobile/authorization-keys.json")).unwrap();
        install_policy(&f);
        f.registry.revoke(f.id, Utc::now()).unwrap();
        let backup = tempfile::tempdir().unwrap();
        let output = ensure_private_mobile_dir(backup.path())
            .unwrap()
            .join("authority.json");
        export_authority(f.home.path(), &output).unwrap();
        let exported: serde_json::Value =
            serde_json::from_slice(&fs::read(&output).unwrap()).unwrap();
        assert_eq!(exported["formatVersion"], 1);
        assert_eq!(
            exported["signature"].as_array().map(Vec::len),
            Some(64),
            "authority export must authenticate its metadata"
        );
        assert_eq!(exported["installationEpoch"], 0);
        assert_eq!(exported["routing"][0]["profile"], "counter");
        assert_eq!(exported["routing"][0]["highestGeneration"], 1);
        assert_eq!(exported["routing"][0]["deviceId"], f.id.to_string());
        let registry: serde_json::Value = serde_json::from_slice(
            &URL_SAFE_NO_PAD
                .decode(exported["registry"].as_str().unwrap())
                .unwrap(),
        )
        .unwrap();
        let expected: serde_json::Value =
            serde_json::from_slice(&fs::read(f.home.path().join("mobile/devices.json")).unwrap())
                .unwrap();
        assert_eq!(registry, expected);
        assert!(registry["devices"][0]["device"]["revokedAt"].is_string());
        assert_eq!(registry["devices"][0]["grant"]["revocationEpoch"], 1);
        let keys: serde_json::Value = serde_json::from_slice(
            &URL_SAFE_NO_PAD
                .decode(exported["authorizationKeys"].as_str().unwrap())
                .unwrap(),
        )
        .unwrap();
        let expected: serde_json::Value = serde_json::from_slice(
            &fs::read(f.home.path().join("mobile/authorization-keys.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(keys, expected);
        assert!(keys["keys"][0]["revokedAt"].is_string());
        assert_eq!(exported["policy"]["state"], "present");
        let policy: serde_json::Value = serde_json::from_slice(
            &URL_SAFE_NO_PAD
                .decode(exported["policy"]["encoded"].as_str().unwrap())
                .unwrap(),
        )
        .unwrap();
        let expected: serde_json::Value = serde_json::from_slice(
            &fs::read(f.home.path().join("mobile/introduction-policy.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(policy, expected);
        validate_private_file(&output).unwrap();
        // Exercise the actual installer against an older snapshot's authority,
        // not just the export's metadata projection.
        atomic_replace_private(&f.home.path().join("mobile/devices.json"), &old_registry).unwrap();
        atomic_replace_private(
            &f.home.path().join("mobile/authorization-keys.json"),
            &old_keys,
        )
        .unwrap();
        fs::remove_file(f.home.path().join("mobile/introduction-policy.json")).unwrap();
        let fingerprint = identity::load_existing_host_identity(f.home.path())
            .unwrap()
            .public_key_fingerprint;
        super::super::install::install(f.home.path(), &output, fingerprint, Uuid::new_v4())
            .unwrap();
        let installed_keys: serde_json::Value = serde_json::from_slice(
            &fs::read(f.home.path().join("mobile/authorization-keys.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(installed_keys, keys);
        let installed_policy: serde_json::Value = serde_json::from_slice(
            &fs::read(f.home.path().join("mobile/introduction-policy.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(installed_policy, policy);
        f.registry.reload().unwrap();
        let installed = f.registry.authorization_record(f.id).unwrap().unwrap();
        assert!(installed.device.revoked_at.is_some());
        assert_eq!(installed.grant.revocation_epoch, 2);
    }

    #[test]
    fn authority_export_never_overwrites_an_existing_private_artifact() {
        let f = Fixture::new();
        let backup = tempfile::tempdir().unwrap();
        let output = ensure_private_mobile_dir(backup.path())
            .unwrap()
            .join("authority.json");
        atomic_replace_private(&output, b"retained private evidence").unwrap();
        assert!(export_authority(f.home.path(), &output).is_err());
        assert_eq!(fs::read(&output).unwrap(), b"retained private evidence");
    }

    #[test]
    fn authority_export_refuses_missing_counter_state_instead_of_guessing_generation() {
        let f = Fixture::new();
        let first = f.prepare();
        f.commit(&first);
        fs::remove_file(
            f.home
                .path()
                .join(format!("mobile/transport-{}.json", f.id)),
        )
        .unwrap();
        let backup = tempfile::tempdir().unwrap();
        let output = ensure_private_mobile_dir(backup.path())
            .unwrap()
            .join("authority.json");
        let error = export_authority(f.home.path(), &output)
            .err()
            .expect("lost counter state exported");
        assert!(error.to_string().contains("counter generation unavailable"));
        assert!(!output.exists());
    }

    #[test]
    fn authority_export_does_not_manufacture_a_missing_installation() {
        let home = tempfile::tempdir().unwrap();
        let backup = tempfile::tempdir().unwrap();
        let output = ensure_private_mobile_dir(backup.path())
            .unwrap()
            .join("authority.json");
        assert!(export_authority(home.path(), &output).is_err());
        assert!(!output.exists());
        assert!(!home.path().join("mobile").exists());
    }
    #[test]
    fn authority_export_records_explicit_absent_policy_and_unprovisioned_pairing() {
        let f = Fixture::new();
        let backup = tempfile::tempdir().unwrap();
        let output = ensure_private_mobile_dir(backup.path())
            .unwrap()
            .join("authority.json");
        export_authority(f.home.path(), &output).unwrap();
        let exported: serde_json::Value =
            serde_json::from_slice(&fs::read(output).unwrap()).unwrap();
        assert_eq!(exported["policy"]["state"], "absent");
        assert_eq!(exported["routing"][0]["profile"], "unprovisioned");
        assert_eq!(exported["routing"][0]["deviceId"], f.id.to_string());
    }

    #[test]
    fn authority_export_bounds_registry_input_before_parsing_or_publishing() {
        let f = Fixture::new();
        atomic_replace_private(
            &f.home.path().join("mobile/devices.json"),
            &vec![b' '; 4 * 1024 * 1024 + 1],
        )
        .unwrap();
        let backup = tempfile::tempdir().unwrap();
        let output = ensure_private_mobile_dir(backup.path())
            .unwrap()
            .join("authority.json");
        let error = export_authority(f.home.path(), &output)
            .err()
            .expect("oversized authority exported");
        assert!(error.to_string().contains("exceeds restore export limit"));
        assert!(!output.exists());
    }

    #[test]
    fn authority_export_does_not_default_unknown_key_authority_to_an_empty_store() {
        let f = Fixture::new();
        atomic_replace_private(
            &f.home.path().join("mobile/authorization-keys.json"),
            br#"{"version":99,"keys":[]}"#,
        )
        .unwrap();
        let backup = tempfile::tempdir().unwrap();
        let output = ensure_private_mobile_dir(backup.path())
            .unwrap()
            .join("authority.json");
        let error = export_authority(f.home.path(), &output)
            .err()
            .expect("unknown key authority exported");
        assert!(error
            .to_string()
            .contains("unsupported mobile authorization-key registry version"));
        assert!(!output.exists());
    }

    #[test]
    fn authority_export_rejects_missing_counter_protocol_floor() {
        let f = Fixture::new();
        let first = f.prepare();
        f.commit(&first);
        fs::remove_file(
            f.home
                .path()
                .join(format!("mobile/.transport-v2-{}.required", f.id)),
        )
        .unwrap();
        let backup = tempfile::tempdir().unwrap();
        let output = ensure_private_mobile_dir(backup.path())
            .unwrap()
            .join("authority.json");
        let error = export_authority(f.home.path(), &output)
            .err()
            .expect("counter floor loss was ignored");
        assert!(error.to_string().contains("counter protocol floor missing"));
        assert!(!output.exists());
    }
    #[test]
    fn authority_export_does_not_write_into_its_source_authority_namespace() {
        let f = Fixture::new();
        let output = f.home.path().join("mobile/authorization-keys.json");
        assert!(!output.exists());
        let error = export_authority(f.home.path(), &output)
            .err()
            .expect("export replaced absent source authority with a backup");
        assert!(error.to_string().contains("outside mobile state"));
        assert!(!output.exists());
    }
}
