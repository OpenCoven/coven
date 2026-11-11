use super::*;
use crate::mobile_memory::{
    grant::{DeviceGrant, DeviceScope},
    registry::DeviceRecord,
};
use p256::pkcs8::DecodePrivateKey;

pub(super) struct Fixture {
    pub(super) home: tempfile::TempDir,
    pub(super) registry: DeviceRegistry,
    pub(super) host: SigningKey,
    pub(super) device: SigningKey,
    pub(super) id: Uuid,
}
impl Fixture {
    pub(super) fn new() -> Self {
        let home = tempfile::tempdir().unwrap();
        let host_identity =
            crate::mobile_memory::identity::load_or_create_host_identity(home.path(), "192.0.2.1")
                .unwrap();
        let host = SigningKey::from_pkcs8_der(&host_identity.private_key_der).unwrap();
        let device = SigningKey::from_slice(&[2; 32]).unwrap();
        let registry = DeviceRegistry::load(home.path()).unwrap();
        let id = Uuid::from_u128(5);
        let now = Utc::now();
        let key = URL_SAFE_NO_PAD.encode(device.verifying_key().to_sec1_point(false).as_bytes());
        let record = DeviceRecord {
            id,
            display_name: "Synthetic v2 phone".into(),
            public_key_x963: key.clone(),
            paired_at: now,
            revoked_at: None,
            suspended_at: None,
            scopes: vec![DeviceScope::MemoryRead],
        };
        let grant = DeviceGrant::for_device(id, &key, record.scopes.clone(), now).unwrap();
        registry.register_with_grant(record, grant).unwrap();
        Self {
            home,
            registry,
            host,
            device,
            id,
        }
    }
    pub(super) fn prepare(&self) -> Handover {
        prepare_direct(self.home.path(), &self.registry, &self.host, self.id).unwrap()
    }
    pub(super) fn commit(&self, handover: &Handover) {
        acknowledge_direct(
            self.home.path(),
            &self.registry,
            &self.host,
            self.id,
            &handover.acknowledgment(),
        )
        .unwrap()
    }
    pub(super) fn snapshot(&self) -> Option<Snapshot> {
        load(
            self.home.path(),
            &self.registry,
            self.host.verifying_key(),
            self.id,
            Transport::Relay,
        )
        .unwrap()
    }
}

#[test]
fn pending_handover_is_durable_identical_on_retry_and_inactive_until_acknowledged() {
    let f = Fixture::new();
    let first = f.prepare();
    assert_eq!(first.version, 2);
    assert_eq!(first.generation, 0);
    assert!(f.snapshot().is_none());
    let restarted = DeviceRegistry::load(f.home.path()).unwrap();
    let retry = prepare_direct(f.home.path(), &restarted, &f.host, f.id).unwrap();
    assert_eq!(
        serde_json::to_vec(&first).unwrap(),
        serde_json::to_vec(&retry).unwrap()
    );
    first
        .verify(f.host.verifying_key(), f.id, f.device.verifying_key())
        .unwrap();
    assert!(first
        .verify(f.device.verifying_key(), f.id, f.device.verifying_key())
        .is_err());
    let mut wrong = first.acknowledgment();
    wrong.seed_digest[0] ^= 1;
    assert!(acknowledge_direct(f.home.path(), &f.registry, &f.host, f.id, &wrong).is_err());
    assert!(f.snapshot().is_none());
    f.commit(&first);
    f.commit(&first);
    let loaded = f.snapshot().unwrap();
    assert_eq!(loaded.generation, 0);
    assert_eq!(loaded.floors, [0, 0]);
}

#[test]
fn v1_upgrade_disables_v1_recreation_and_generation_handover_never_downgrades() {
    let f = Fixture::new();
    let legacy = load_or_provision(f.home.path(), &f.registry, &f.host, f.id).unwrap();
    let initial = f.prepare();
    assert!(load_or_provision(f.home.path(), &f.registry, &f.host, f.id).is_err());
    f.commit(&initial);
    let before = f.snapshot().unwrap();
    assert_ne!(
        legacy.rendezvous(0, false).unwrap(),
        before.rendezvous(Transport::Relay, 0).unwrap()
    );
    let next = f.prepare();
    assert_eq!(next.generation, 1);
    assert_ne!(initial.routing_seed, next.routing_seed);
    assert!(f.snapshot().is_none());
    f.commit(&next);
    assert!(acknowledge_direct(
        f.home.path(),
        &f.registry,
        &f.host,
        f.id,
        &initial.acknowledgment()
    )
    .is_err());
    let current = f.snapshot().unwrap();
    assert_eq!(current.generation, 1);
    assert_eq!(current.floors, [0, 0]);
    assert!(consume(
        f.home.path(),
        &f.registry,
        &before.material.pairing,
        Transport::Relay,
        before.state(Transport::Relay).proof(0).unwrap()
    )
    .is_err());
}

#[test]
fn host_consumption_survives_reload_rejects_stale_commit_and_rechecks_revocation() {
    let f = Fixture::new();
    let handover = f.prepare();
    f.commit(&handover);
    let before = f.snapshot().unwrap();
    let proof = before.state(Transport::Relay).proof(2).unwrap();
    consume(
        f.home.path(),
        &f.registry,
        &before.material.pairing,
        Transport::Relay,
        proof,
    )
    .unwrap();
    let current = f.snapshot().unwrap();
    assert_eq!(current.floors, [0, 3]);
    assert!(consume(
        f.home.path(),
        &f.registry,
        &before.material.pairing,
        Transport::Relay,
        proof
    )
    .is_err());
    assert_eq!(f.snapshot().unwrap().floors, [0, 3]);
    f.registry.revoke(f.id, Utc::now()).unwrap();
    assert!(consume(
        f.home.path(),
        &f.registry,
        &current.material.pairing,
        Transport::Relay,
        current.state(Transport::Relay).proof(3).unwrap()
    )
    .is_err());
    assert!(load(
        f.home.path(),
        &f.registry,
        f.host.verifying_key(),
        f.id,
        Transport::Relay
    )
    .is_err());
    assert!(prepare_direct(f.home.path(), &f.registry, &f.host, f.id).is_err());
}

#[test]
fn direct_resync_raises_host_floor_without_lowering_or_reopening_consumed_rooms() {
    let f = Fixture::new();
    let handover = f.prepare();
    f.commit(&handover);
    let resynced = resync_direct(
        f.home.path(),
        &f.registry,
        &f.host,
        f.id,
        Transport::Relay,
        0,
        4,
    )
    .unwrap();
    assert_eq!(
        resynced,
        HostState {
            generation: 0,
            floor: 4
        }
    );
    assert_eq!(f.snapshot().unwrap().floors, [0, 4]);
    assert_eq!(
        resync_direct(
            f.home.path(),
            &f.registry,
            &f.host,
            f.id,
            Transport::Relay,
            0,
            2
        )
        .unwrap()
        .floor,
        4
    );
    assert!(resync_direct(
        f.home.path(),
        &f.registry,
        &f.host,
        f.id,
        Transport::Relay,
        1,
        8
    )
    .is_err());
    assert_eq!(f.snapshot().unwrap().floors, [0, 4]);
    assert!(f
        .snapshot()
        .unwrap()
        .state(Transport::Relay)
        .proof(3)
        .is_err());
}

#[test]
fn corrupt_material_is_not_silently_reprovisioned() {
    let f = Fixture::new();
    let handover = f.prepare();
    f.commit(&handover);
    let path = f
        .home
        .path()
        .join(format!("mobile/transport-{}.json", f.id));
    atomic_replace_private(&path, b"{corrupt v2 state").unwrap();
    assert!(load(
        f.home.path(),
        &f.registry,
        f.host.verifying_key(),
        f.id,
        Transport::Relay
    )
    .is_err());
    assert!(prepare_direct(f.home.path(), &f.registry, &f.host, f.id).is_err());
    assert!(load_or_provision(f.home.path(), &f.registry, &f.host, f.id).is_err());
    assert_eq!(fs::read(&path).unwrap(), b"{corrupt v2 state");
}

#[test]
fn missing_v2_material_never_reopens_v1_or_restarts_generation_zero() {
    let f = Fixture::new();
    let initial = f.prepare();
    f.commit(&initial);
    fs::remove_file(
        f.home
            .path()
            .join(format!("mobile/transport-{}.json", f.id)),
    )
    .unwrap();
    assert!(
        load_or_provision(f.home.path(), &f.registry, &f.host, f.id).is_err(),
        "missing v2 material reopened v1"
    );
    assert!(
        prepare_direct(f.home.path(), &f.registry, &f.host, f.id).is_err(),
        "missing v2 material restarted generation zero"
    );
}

#[test]
fn failed_acknowledgment_directory_barrier_retains_committed_generation_and_retry_preserves_counters(
) {
    let f = Fixture::new();
    let handover = f.prepare();
    crate::durable_fs::fail_next_directory_sync();
    let failure = acknowledge_direct(
        f.home.path(),
        &f.registry,
        &f.host,
        f.id,
        &handover.acknowledgment(),
    )
    .unwrap_err();
    assert!(
        failure
            .to_string()
            .contains("injected directory sync failure"),
        "{failure:#}"
    );
    let committed = f.snapshot().unwrap();
    assert_eq!(committed.generation, 0);
    consume(
        f.home.path(),
        &f.registry,
        &committed.material.pairing,
        Transport::Relay,
        committed.state(Transport::Relay).proof(0).unwrap(),
    )
    .unwrap();
    f.commit(&handover);
    assert_eq!(f.snapshot().unwrap().floors, [0, 1]);
}

#[test]
fn failed_consumption_directory_barrier_does_not_imply_rollback_or_reopen_a_room() {
    let f = Fixture::new();
    let handover = f.prepare();
    f.commit(&handover);
    let before = f.snapshot().unwrap();
    let proof = before.state(Transport::Relay).proof(0).unwrap();
    crate::durable_fs::fail_next_directory_sync();
    let failure = consume(
        f.home.path(),
        &f.registry,
        &before.material.pairing,
        Transport::Relay,
        proof,
    )
    .unwrap_err();
    assert!(
        failure
            .to_string()
            .contains("injected directory sync failure"),
        "{failure:#}"
    );
    assert_eq!(f.snapshot().unwrap().floors, [0, 1]);
    assert!(consume(
        f.home.path(),
        &f.registry,
        &before.material.pairing,
        Transport::Relay,
        proof
    )
    .is_err());
    assert_eq!(f.snapshot().unwrap().floors, [0, 1]);
}

mod process;

#[test]
fn state_loss_recovery_uses_authenticated_generation_anchor_and_retries_the_same_pending_seed() {
    for corrupt in [false, true] {
        let f = Fixture::new();
        let home = tempfile::tempdir().unwrap();
        let initial = f.prepare();
        let ack = device::adopt_direct(
            home.path(),
            initial.clone(),
            f.host.verifying_key(),
            f.id,
            f.device.verifying_key(),
        )
        .unwrap();
        acknowledge_direct(f.home.path(), &f.registry, &f.host, f.id, &ack).unwrap();
        let path = f
            .home
            .path()
            .join(format!("mobile/transport-{}.json", f.id));
        if corrupt {
            atomic_replace_private(&path, b"{corrupt state").unwrap();
        } else {
            fs::remove_file(&path).unwrap();
        }
        assert!(
            recover_direct(f.home.path(), &f.registry, &f.host, f.id, None).is_err(),
            "unknown generation was guessed"
        );
        let recovered = recover_direct(f.home.path(), &f.registry, &f.host, f.id, Some(0)).unwrap();
        assert_eq!(recovered.generation, 1);
        assert_ne!(recovered.routing_seed, initial.routing_seed);
        assert!(f.snapshot().is_none());
        let retry = recover_direct(f.home.path(), &f.registry, &f.host, f.id, Some(0)).unwrap();
        assert_eq!(
            serde_json::to_vec(&recovered).unwrap(),
            serde_json::to_vec(&retry).unwrap()
        );
        let ack = device::adopt_direct(
            home.path(),
            recovered,
            f.host.verifying_key(),
            f.id,
            f.device.verifying_key(),
        )
        .unwrap();
        acknowledge_direct(f.home.path(), &f.registry, &f.host, f.id, &ack).unwrap();
        assert_eq!(f.snapshot().unwrap().generation, 1);
        assert_eq!(f.snapshot().unwrap().floors, [0, 0]);
        assert!(load_or_provision(f.home.path(), &f.registry, &f.host, f.id).is_err());
    }
}

#[test]
fn generation_overflow_requires_repair_and_preserves_existing_state() {
    let f = Fixture::new();
    let handover = f.prepare();
    f.commit(&handover);
    let path = f
        .home
        .path()
        .join(format!("mobile/transport-{}.json", f.id));
    let mut stored = read(&path, f.id).unwrap().unwrap();
    stored.committed.as_mut().unwrap().generation = u64::MAX;
    save(&path, &stored).unwrap();
    let before = fs::read(&path).unwrap();
    assert!(prepare_direct(f.home.path(), &f.registry, &f.host, f.id)
        .err()
        .expect("overflow accepted")
        .to_string()
        .contains("re-pair"));
    assert!(
        recover_direct(f.home.path(), &f.registry, &f.host, f.id, Some(u64::MAX))
            .err()
            .expect("overflow accepted")
            .to_string()
            .contains("re-pair")
    );
    assert_eq!(fs::read(&path).unwrap(), before);
}

#[test]
fn committed_generation_below_retained_minimum_is_rejected_as_corrupt_state() {
    let f = Fixture::new();
    let handover = f.prepare();
    f.commit(&handover);
    let path = f
        .home
        .path()
        .join(format!("mobile/transport-{}.json", f.id));
    let mut stored = read(&path, f.id).unwrap().unwrap();
    stored.minimum_generation = Some(1);
    save(&path, &stored).unwrap();
    assert!(
        load(
            f.home.path(),
            &f.registry,
            f.host.verifying_key(),
            f.id,
            Transport::Relay
        )
        .is_err(),
        "committed generation below retained minimum was admitted"
    );
}

#[test]
fn changed_grant_burns_pending_generation_and_never_reuses_its_seed_or_authority() {
    for adopted_pending in [false, true] {
        let f = Fixture::new();
        let home = tempfile::tempdir().unwrap();
        let first = f.prepare();
        let ack = device::adopt_direct(
            home.path(),
            first,
            f.host.verifying_key(),
            f.id,
            f.device.verifying_key(),
        )
        .unwrap();
        acknowledge_direct(f.home.path(), &f.registry, &f.host, f.id, &ack).unwrap();
        let old = f.snapshot().unwrap();
        let pending = f.prepare();
        assert_eq!(pending.generation, 1);
        if adopted_pending {
            device::adopt_direct(
                home.path(),
                pending.clone(),
                f.host.verifying_key(),
                f.id,
                f.device.verifying_key(),
            )
            .unwrap();
        }
        let record = f.registry.authorization_record(f.id).unwrap().unwrap();
        let now = Utc::now();
        let replacement = record
            .grant
            .reissue(
                &record.device.public_key_x963,
                record.grant.scopes.clone(),
                record.grant.restrictions.clone(),
                record.grant.minimum_assurance,
                now,
                now + chrono::Duration::hours(1),
            )
            .unwrap();
        let replacement_id = replacement.id;
        f.registry.replace_grant(f.id, replacement).unwrap();
        let next = f.prepare();
        assert_eq!(next.generation, 2);
        assert_ne!(pending.routing_seed, next.routing_seed);
        assert_eq!(next.grant_id, replacement_id.to_string());
        assert!(acknowledge_direct(
            f.home.path(),
            &f.registry,
            &f.host,
            f.id,
            &pending.acknowledgment()
        )
        .is_err());
        let retry = f.prepare();
        assert_eq!(
            serde_json::to_vec(&next).unwrap(),
            serde_json::to_vec(&retry).unwrap()
        );
        let ack = device::adopt_direct(
            home.path(),
            next,
            f.host.verifying_key(),
            f.id,
            f.device.verifying_key(),
        )
        .unwrap();
        acknowledge_direct(f.home.path(), &f.registry, &f.host, f.id, &ack).unwrap();
        assert_eq!(f.snapshot().unwrap().generation, 2);
        assert_eq!(f.snapshot().unwrap().floors, [0, 0]);
        assert!(consume(
            f.home.path(),
            &f.registry,
            &old.material.pairing,
            Transport::Relay,
            old.state(Transport::Relay).proof(0).unwrap()
        )
        .is_err());
    }
}

#[test]
fn expired_possession_deadline_is_rechecked_under_the_consumption_lock_without_writing() {
    let f = Fixture::new();
    let handover = f.prepare();
    f.commit(&handover);
    let snapshot = f.snapshot().unwrap();
    let path = f
        .home
        .path()
        .join(format!("mobile/transport-{}.json", f.id));
    let before = fs::read(&path).unwrap();
    let expired = std::time::Instant::now();
    let result = consume_inner(
        f.home.path(),
        &f.registry,
        &snapshot.material.pairing,
        Transport::Relay,
        snapshot.state(Transport::Relay).proof(0).unwrap(),
        expired,
    );
    assert!(result.is_err(), "expired proof advanced the durable floor");
    assert_eq!(fs::read(&path).unwrap(), before);
    assert_eq!(f.snapshot().unwrap().floors, [0, 0]);
}

#[test]
fn v2_host_proof_refreshes_committed_floor_after_listener_creation() {
    let f = Fixture::new();
    let handover = f.prepare();
    f.commit(&handover);
    let before = f.snapshot().unwrap();
    let hint = before.state(Transport::Relay).proof(1).unwrap();
    consume(
        f.home.path(),
        &f.registry,
        &before.material.pairing,
        Transport::Relay,
        before.state(Transport::Relay).proof(0).unwrap(),
    )
    .unwrap();
    assert_eq!(
        refresh_proof(
            f.home.path(),
            &f.registry,
            &before.material.pairing,
            Transport::Relay,
            hint,
            std::time::Instant::now() + std::time::Duration::from_secs(30)
        )
        .unwrap(),
        HostProof {
            generation: 0,
            host_floor: 1,
            counter: 1
        }
    );
    assert!(refresh_proof(
        f.home.path(),
        &f.registry,
        &before.material.pairing,
        Transport::Relay,
        before.state(Transport::Relay).proof(0).unwrap(),
        std::time::Instant::now() + std::time::Duration::from_secs(30)
    )
    .is_err());
    let next = f.prepare();
    f.commit(&next);
    assert!(refresh_proof(
        f.home.path(),
        &f.registry,
        &before.material.pairing,
        Transport::Relay,
        hint,
        std::time::Instant::now() + std::time::Duration::from_secs(30)
    )
    .is_err());
}

fn install_restore_fixture(f: &Fixture, phase: &str) {
    let identity =
        crate::mobile_memory::identity::load_existing_host_identity(f.home.path()).unwrap();
    let directory = f.home.path().join("mobile");
    atomic_replace_private(
        &directory.join(".transport-restore.required"),
        b"coven.mobile.restore.v1\n",
    )
    .unwrap();
    if phase == "awaitingHandovers" {
        let path = directory.join("devices.json");
        let mut registry: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        registry["restoreEpoch"] = serde_json::json!(1);
        atomic_replace_private(&path, &serde_json::to_vec(&registry).unwrap()).unwrap();
    }
    let state = serde_json::json!({
        "version": 1,
        "operationId": Uuid::from_u128(99),
        "sourceDigest": vec![0_u8; 32],
        "installationFingerprint": identity.public_key_fingerprint,
        "installationEpoch": 1,
        "phase": phase,
        "pairings": [{ "deviceId": f.id, "generation": 1, "revocationEpoch": 1, "handoverComplete": false }]
    });
    atomic_replace_private(
        &directory.join("transport-restore.json"),
        &serde_json::to_vec(&state).unwrap(),
    )
    .unwrap();
}

#[test]
fn installing_restore_fence_blocks_existing_channels_and_direct_bootstrap() {
    let f = Fixture::new();
    let first = f.prepare();
    f.commit(&first);
    let snapshot = f.snapshot().unwrap();
    let before = fs::read(
        f.home
            .path()
            .join(format!("mobile/transport-{}.json", f.id)),
    )
    .unwrap();
    install_restore_fixture(&f, "installing");
    assert!(
        load(
            f.home.path(),
            &f.registry,
            f.host.verifying_key(),
            f.id,
            Transport::Relay
        )
        .is_err(),
        "restoring installation still admitted v2 state"
    );
    assert!(
        prepare_direct(f.home.path(), &f.registry, &f.host, f.id).is_err(),
        "bootstrap ran during unfinished restore"
    );
    assert!(check_channel(
        f.home.path(),
        &f.registry,
        &snapshot.material.pairing,
        Transport::Relay,
        Some(0)
    )
    .is_err());
    assert!(acknowledge_direct(
        f.home.path(),
        &f.registry,
        &f.host,
        f.id,
        &first.acknowledgment()
    )
    .is_err());
    assert_eq!(
        fs::read(
            f.home
                .path()
                .join(format!("mobile/transport-{}.json", f.id))
        )
        .unwrap(),
        before
    );
}

#[test]
fn missing_or_corrupt_restore_journal_never_falls_back_to_ordinary_routing() {
    let f = Fixture::new();
    let first = f.prepare();
    f.commit(&first);
    install_restore_fixture(&f, "awaitingHandovers");
    let path = f.home.path().join("mobile/transport-restore.json");
    fs::remove_file(&path).unwrap();
    assert!(
        load(
            f.home.path(),
            &f.registry,
            f.host.verifying_key(),
            f.id,
            Transport::Relay
        )
        .is_err(),
        "missing restore journal bypassed durable fence"
    );
    atomic_replace_private(&path, b"{bad}").unwrap();
    assert!(prepare_direct(f.home.path(), &f.registry, &f.host, f.id).is_err());
}

#[test]
fn restored_pairing_requires_fresh_generation_and_epoch_ack_before_admission() {
    let f = Fixture::new();
    let old = f.prepare();
    f.commit(&old);
    install_restore_fixture(&f, "awaitingHandovers");
    assert!(
        acknowledge_direct(
            f.home.path(),
            &f.registry,
            &f.host,
            f.id,
            &old.acknowledgment()
        )
        .is_err(),
        "old acknowledgment cleared restore boundary"
    );
    let record = f.registry.authorization_record(f.id).unwrap().unwrap();
    let now = Utc::now();
    let replacement = record
        .grant
        .reissue(
            &record.device.public_key_x963,
            record.grant.scopes.clone(),
            record.grant.restrictions.clone(),
            record.grant.minimum_assurance,
            now,
            now + chrono::Duration::hours(1),
        )
        .unwrap();
    f.registry.replace_grant(f.id, replacement).unwrap();
    let fresh = f.prepare();
    assert_eq!(fresh.generation, 1);
    assert_eq!(fresh.revocation_epoch, 1);
    let device_home = tempfile::tempdir().unwrap();
    let ack = device::adopt_direct(
        device_home.path(),
        fresh,
        f.host.verifying_key(),
        f.id,
        f.device.verifying_key(),
    )
    .unwrap();
    assert!(load(
        f.home.path(),
        &f.registry,
        f.host.verifying_key(),
        f.id,
        Transport::Relay
    )
    .is_err());
    acknowledge_direct(f.home.path(), &f.registry, &f.host, f.id, &ack).unwrap();
    assert_eq!(f.snapshot().unwrap().generation, 1);
    let state: serde_json::Value = serde_json::from_slice(
        &fs::read(f.home.path().join("mobile/transport-restore.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(state["pairings"][0]["handoverComplete"], true);
    acknowledge_direct(f.home.path(), &f.registry, &f.host, f.id, &ack).unwrap();
    assert_eq!(f.snapshot().unwrap().floors, [0, 0]);
}

#[test]
fn completed_restore_journal_does_not_admit_older_routing_or_grant_images() {
    let f = Fixture::new();
    let first = f.prepare();
    f.commit(&first);
    install_restore_fixture(&f, "awaitingHandovers");
    let path = f.home.path().join("mobile/transport-restore.json");
    let mut journal: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    journal["pairings"][0]["handoverComplete"] = serde_json::json!(true);
    atomic_replace_private(&path, &serde_json::to_vec(&journal).unwrap()).unwrap();
    assert!(
        load(
            f.home.path(),
            &f.registry,
            f.host.verifying_key(),
            f.id,
            Transport::Relay
        )
        .is_err(),
        "a completed restore journal admitted older routing/grant images"
    );
}

#[test]
fn restore_metadata_rejects_unknown_versions_duplicate_targets_and_changed_installation() {
    for mutation in [
        "version",
        "phase",
        "duplicate",
        "fingerprint",
        "marker",
        "missingMarker",
    ] {
        let f = Fixture::new();
        let first = f.prepare();
        f.commit(&first);
        install_restore_fixture(&f, "awaitingHandovers");
        assert!(restore::check_installation(f.home.path()).is_ok());
        let directory = f.home.path().join("mobile");
        let path = directory.join("transport-restore.json");
        let mut state: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        match mutation {
            "version" => state["version"] = serde_json::json!(99),
            "phase" => state["phase"] = serde_json::json!("unrecognized"),
            "duplicate" => {
                let duplicate = state["pairings"][0].clone();
                state["pairings"].as_array_mut().unwrap().push(duplicate);
            }
            "fingerprint" => state["installationFingerprint"] = serde_json::json!(vec![0_u8; 32]),
            "marker" => {
                atomic_replace_private(&directory.join(".transport-restore.required"), b"unknown\n")
                    .unwrap()
            }
            "missingMarker" => {
                fs::remove_file(directory.join(".transport-restore.required")).unwrap()
            }
            _ => unreachable!(),
        }
        atomic_replace_private(&path, &serde_json::to_vec(&state).unwrap()).unwrap();
        assert!(
            restore::check_installation(f.home.path()).is_err(),
            "invalid {mutation} metadata passed installation admission"
        );
        assert!(
            prepare_direct(f.home.path(), &f.registry, &f.host, f.id).is_err(),
            "invalid {mutation} restore metadata admitted direct provisioning"
        );
        assert!(
            load(
                f.home.path(),
                &f.registry,
                f.host.verifying_key(),
                f.id,
                Transport::Relay
            )
            .is_err(),
            "invalid {mutation} restore metadata admitted relay state"
        );
    }
}

#[test]
fn restore_ack_journal_barrier_failure_aborts_and_both_retained_images_retry_safely() {
    for retain_old_journal in [false, true] {
        let f = Fixture::new();
        let old = f.prepare();
        f.commit(&old);
        install_restore_fixture(&f, "awaitingHandovers");
        let journal_path = f.home.path().join("mobile/transport-restore.json");
        let old_journal = fs::read(&journal_path).unwrap();
        let record = f.registry.authorization_record(f.id).unwrap().unwrap();
        let now = Utc::now();
        let replacement = record
            .grant
            .reissue(
                &record.device.public_key_x963,
                record.grant.scopes.clone(),
                record.grant.restrictions.clone(),
                record.grant.minimum_assurance,
                now,
                now + chrono::Duration::hours(1),
            )
            .unwrap();
        f.registry.replace_grant(f.id, replacement).unwrap();
        let fresh = f.prepare();
        let home = tempfile::tempdir().unwrap();
        let ack = device::adopt_direct(
            home.path(),
            fresh,
            f.host.verifying_key(),
            f.id,
            f.device.verifying_key(),
        )
        .unwrap();
        // Two host-state namespace barriers must finish before the first
        // restore-journal barrier fails after its rename. These are real fsyncs.
        crate::durable_fs::fail_directory_sync_after(2);
        let error =
            acknowledge_direct(f.home.path(), &f.registry, &f.host, f.id, &ack).unwrap_err();
        assert!(error
            .to_string()
            .contains("injected directory sync failure"));
        let host_path = f
            .home
            .path()
            .join(format!("mobile/transport-{}.json", f.id));
        let persisted = read(&host_path, f.id).unwrap().unwrap();
        assert_eq!(active(&persisted).unwrap().generation, 1);
        let new_journal = fs::read(&journal_path).unwrap();
        let after: serde_json::Value = serde_json::from_slice(&new_journal).unwrap();
        assert_eq!(after["pairings"][0]["handoverComplete"], true);
        if retain_old_journal {
            // Model the surviving old namespace image, not merely process death.
            atomic_replace_private(&journal_path, &old_journal).unwrap();
            assert!(load(
                f.home.path(),
                &f.registry,
                f.host.verifying_key(),
                f.id,
                Transport::Relay
            )
            .is_err());
        } else {
            assert_eq!(f.snapshot().unwrap().generation, 1);
        }
        acknowledge_direct(f.home.path(), &f.registry, &f.host, f.id, &ack).unwrap();
        let snapshot = f.snapshot().unwrap();
        consume(
            f.home.path(),
            &f.registry,
            &snapshot.material.pairing,
            Transport::Relay,
            snapshot.state(Transport::Relay).proof(0).unwrap(),
        )
        .unwrap();
        acknowledge_direct(f.home.path(), &f.registry, &f.host, f.id, &ack).unwrap();
        assert_eq!(f.snapshot().unwrap().floors, [0, 1]);
    }
}

#[test]
fn retained_registry_restore_epoch_blocks_loss_of_both_global_fence_files() {
    let f = Fixture::new();
    let first = f.prepare();
    f.commit(&first);
    assert!(f.snapshot().is_some());
    let path = f.home.path().join("mobile/devices.json");
    let mut registry: serde_json::Value =
        serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    registry["restoreEpoch"] = serde_json::json!(7);
    atomic_replace_private(&path, &serde_json::to_vec(&registry).unwrap()).unwrap();
    // The retained authority reference is valid even though both global files
    // are missing; their absence must not become the legacy/no-restore case.
    f.registry.reload().unwrap();
    assert!(
        restore::check_installation(f.home.path()).is_err(),
        "lost restore metadata became a never-restored installation"
    );
    assert!(load(
        f.home.path(),
        &f.registry,
        f.host.verifying_key(),
        f.id,
        Transport::Relay
    )
    .is_err());
}

pub(super) mod process_crash;

mod authenticated_race;
