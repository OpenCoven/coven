use super::super::transport_tests::Fixture;
use super::*;
use crate::mobile_memory::{
    config::atomic_replace_private,
    grant::{DeviceScope, GrantTransportConstraint},
    identity::load_existing_host_identity,
    registry::DeviceRecord,
    test_io::{Counts, Measurement},
};
use p256::pkcs8::DecodePrivateKey;
use std::sync::mpsc::{self, Sender};

fn signer(fixture: &Fixture) -> Arc<SigningKey> {
    Arc::new(
        SigningKey::from_pkcs8_der(
            &load_existing_host_identity(&fixture.state.coven_home)
                .unwrap()
                .private_key_der,
        )
        .unwrap(),
    )
}

fn configured() -> (Fixture, Arc<SigningKey>, Manager) {
    let fixture = Fixture::new();
    let signer = signer(&fixture);
    let config = storage::RelayConfig {
        version: 1,
        enabled: true,
        endpoints: vec![websocket::EndpointConfig {
            url: "wss://relay.example.test/ws".into(),
            ca_certificates: Vec::new(),
        }],
    };
    config.save(&fixture.state.coven_home).unwrap();
    let manager = Manager {
        endpoints: vec![Arc::new(config.endpoints[0].parse().unwrap())],
        configuration: Some(config),
        ..Manager::default()
    };
    (fixture, signer, manager)
}

// Sender ownership is declared after Manager in each test, so unwinding drops
// senders before Worker::drop joins. No fixture thread can strand the suite.
fn hold(manager: &mut Manager, pairing: transport::Pairing) -> Sender<()> {
    let (send, receive) = mpsc::channel();
    let thread = std::thread::spawn(move || {
        let _ = receive.recv();
    });
    manager.workers.insert(
        pairing.device_id,
        Worker {
            control: Arc::new(WorkerControl::default()),
            authenticated: Arc::new(AtomicBool::new(true)),
            pairing,
            thread: Some(thread),
        },
    );
    send
}

fn reap_fixture(manager: &mut Manager, id: Uuid, sender: Sender<()>) {
    drop(sender);
    manager
        .workers
        .get_mut(&id)
        .unwrap()
        .thread
        .take()
        .unwrap()
        .join()
        .unwrap();
}

#[test]
fn thirty_two_idle_workers_share_one_change_aware_snapshot_and_refresh_on_events() {
    let now = Instant::now();
    let (fixture, signer, mut manager) = configured();
    let mut held = Vec::new();
    held.push(hold(&mut manager, fixture.pairing.clone()));
    for index in 1..32_u8 {
        let key = SigningKey::from_slice(&[index; 32]).unwrap();
        let id = Uuid::new_v4();
        fixture
            .state
            .registry
            .register(DeviceRecord {
                id,
                display_name: "Synthetic peer".into(),
                public_key_x963: URL_SAFE_NO_PAD
                    .encode(key.verifying_key().to_sec1_point(false).as_bytes()),
                paired_at: Utc::now(),
                revoked_at: None,
                suspended_at: None,
                scopes: vec![DeviceScope::MemoryRead],
            })
            .unwrap();
        let material = storage::load_or_provision(
            &fixture.state.coven_home,
            &fixture.state.registry,
            &signer,
            id,
        )
        .unwrap();
        held.push(hold(&mut manager, material.pairing));
    }
    let home = fixture.state.coven_home.clone();
    let paths: Vec<_> = ["host-key.pem", "host-identity.json", "devices.json"]
        .iter()
        .map(|name| home.join("mobile").join(name))
        .collect();
    let measurement = Measurement::start(paths.clone());
    // Reproduce the old idle poll's real read/lock boundaries as a baseline.
    load_existing_host_identity(&home).unwrap();
    fixture.state.registry.reload().unwrap();
    for worker in manager.workers.values() {
        authority::check_current(&fixture.state.registry, &worker.pairing, false, Utc::now())
            .unwrap();
    }
    assert_eq!(
        measurement.count(&paths[2]),
        Counts {
            reads: 33,
            locks: 33
        }
    );
    eprintln!("legacy 32-worker idle poll: identity content reads=2, registry content reads=33, store locks=33");
    drop(measurement);
    let measurement = Measurement::start(paths.clone());
    let state = Arc::new(fixture.state);
    for _ in 0..10 {
        manager.tick(&state, &signer, &Stop::default(), now);
    }
    assert_eq!(measurement.count(&paths[0]), Counts { reads: 1, locks: 0 });
    assert_eq!(measurement.count(&paths[1]), Counts { reads: 1, locks: 0 });
    assert_eq!(measurement.count(&paths[2]), Counts { reads: 1, locks: 1 });
    assert!(manager
        .workers
        .values()
        .all(|worker| !worker.control.cancelled()));
    eprintln!("new 32-worker, 10 idle polls: identity content reads=2, registry content reads=1, store locks=1; unchanged polls add zero");

    reap_fixture(&mut manager, fixture.pairing.device_id, held.remove(0));
    manager.tick(&state, &signer, &Stop::default(), now);
    assert_eq!(measurement.count(&paths[0]).reads, 2);
    assert_eq!(measurement.count(&paths[2]), Counts { reads: 2, locks: 2 });
    assert_eq!(
        manager.retries[&fixture.pairing.device_id].count, 0,
        "successful uncancelled session should reset backoff"
    );
    drop(held);
}

#[test]
fn cancelled_authenticated_worker_keeps_backoff_across_policy_and_configuration_changes() {
    let now = Instant::now();
    let (fixture, signer, mut manager) = configured();
    let held = hold(&mut manager, fixture.pairing.clone());
    let id = fixture.pairing.device_id;
    manager.retries.insert(
        id,
        Retry {
            count: 4,
            after: now,
        },
    );
    let state = Arc::new(fixture.state);
    manager.tick(&state, &signer, &Stop::default(), now);
    // A separate registry instance represents a CLI/process changing authority.
    DeviceRegistry::load(&state.coven_home)
        .unwrap()
        .suspend(id, Utc::now())
        .unwrap();
    manager.tick(&state, &signer, &Stop::default(), now);
    assert!(manager.workers[&id].control.cancelled());
    reap_fixture(&mut manager, id, held);
    manager.tick(&state, &signer, &Stop::default(), now);
    assert_eq!(manager.retries[&id].count, 5);
    let after = manager.retries[&id].after;
    storage::RelayConfig::disable(&state.coven_home).unwrap();
    manager.tick(&state, &signer, &Stop::default(), now);
    assert_eq!(manager.retries[&id].count, 5);
    assert_eq!(manager.retries[&id].after, after);
}

#[test]
fn failed_material_and_policy_get_bounded_retries_instead_of_poll_reloads() {
    let now = Instant::now();
    let (fixture, signer, mut manager) = configured();
    let id = fixture.pairing.device_id;
    let path = fixture
        .state
        .coven_home
        .join(format!("mobile/transport-{id}.json"));
    let original = std::fs::read(&path).unwrap();
    atomic_replace_private(&path, b"invalid").unwrap();
    let measured = Measurement::start(vec![path.clone()]);
    let state = Arc::new(fixture.state);
    manager.tick(&state, &signer, &Stop::default(), now);
    assert_eq!(manager.retries[&id].count, 1);
    for _ in 0..10 {
        manager.tick(&state, &signer, &Stop::default(), now);
    }
    assert_eq!(measured.count(&path).reads, 1);
    atomic_replace_private(&path, &original).unwrap();
    DeviceRegistry::load(&state.coven_home)
        .unwrap()
        .revoke(id, Utc::now())
        .unwrap();
    for count in 2..10 {
        manager.retries.get_mut(&id).unwrap().after = now;
        manager.tick(&state, &signer, &Stop::default(), now);
        assert_eq!(manager.retries[&id].count, count);
        assert!(manager.retries[&id].after.duration_since(now) <= Duration::from_millis(30_255));
        assert!(manager.workers.is_empty());
    }
    assert_eq!(measured.count(&path).reads, 9);
    std::fs::remove_file(&path).unwrap();
    manager.retries.get_mut(&id).unwrap().after = now;
    manager.tick(&state, &signer, &Stop::default(), now);
    assert_eq!(
        manager.retries[&id].count, 10,
        "missing material needs a retry entry too"
    );
    assert_eq!(manager.retries.len(), 1);
    eprintln!("invalid material: 11 polls read material once; policy failures and missing material retain bounded retry entries");
}

#[test]
fn unchanged_snapshot_still_enforces_expiry_and_current_admission_reloads() {
    let now = Instant::now();
    let (fixture, signer, mut manager) = configured();
    let held = hold(&mut manager, fixture.pairing.clone());
    let id = fixture.pairing.device_id;
    let state = Arc::new(fixture.state);
    manager.tick(&state, &signer, &Stop::default(), now);
    let mut record = manager.registry.value.as_ref().unwrap()[0].clone();
    record.grant.issued_at = Utc::now() - chrono::Duration::hours(1);
    record.grant.not_before = record.grant.issued_at;
    record.grant.expires_at = Some(Utc::now() - chrono::Duration::seconds(1));
    assert_eq!(
        authority::check_record(&record, &fixture.pairing, false, Utc::now())
            .unwrap_err()
            .to_string(),
        "grant inactive"
    );
    let mut direct_only = manager.registry.value.as_ref().unwrap()[0].clone();
    direct_only.grant.restrictions.transport = GrantTransportConstraint::DirectOnly;
    assert_eq!(
        authority::check_record(&direct_only, &fixture.pairing, false, Utc::now())
            .unwrap_err()
            .to_string(),
        "grant requires direct transport"
    );
    // Simulate the passage of the clock with a snapshot already past expiry,
    // without rewriting disk or waiting on a scheduler-selected short budget.
    manager.registry.value.as_mut().unwrap()[0] = record;
    manager.tick(&state, &signer, &Stop::default(), now);
    assert!(manager.workers[&id].control.cancelled());
    DeviceRegistry::load(&state.coven_home)
        .unwrap()
        .revoke(id, Utc::now())
        .unwrap();
    assert!(
        authority::check_current(&state.registry, &fixture.pairing, false, Utc::now()).is_err()
    );
    drop(held);
}

#[test]
fn cache_discards_failed_raced_and_unsafe_snapshots_and_recovers_on_change() {
    let fixture = Fixture::new();
    let home = &fixture.state.coven_home;
    let path = home.join("mobile/host-key.pem");
    let original = std::fs::read(&path).unwrap();
    let mut cached = Cache::default();
    let mut loads = 0;
    for _ in 0..3 {
        cached.refresh(home, &["host-key.pem"], false, || {
            loads += 1;
            Ok(true)
        });
    }
    assert_eq!(loads, 1);
    cached.refresh(home, &["host-key.pem"], true, || {
        atomic_replace_private(&path, b"bad").unwrap();
        Ok(true)
    });
    assert_eq!(cached.value, None, "raced stamp cannot bless a stale read");
    cached.refresh(home, &["host-key.pem"], false, || {
        Err::<bool, _>(anyhow::anyhow!("invalid"))
    });
    cached.refresh(home, &["host-key.pem"], false, || {
        panic!("unchanged failure was parsed again")
    });
    assert_eq!(cached.value, None);
    atomic_replace_private(&path, &original).unwrap();
    cached.refresh(home, &["host-key.pem"], false, || Ok(true));
    assert_eq!(cached.value, Some(true));
    std::fs::remove_file(&path).unwrap();
    cached.refresh(home, &["host-key.pem"], false, || {
        Err::<bool, _>(anyhow::anyhow!("missing"))
    });
    assert_eq!(cached.value, None);
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(home.join("mobile/host-identity.json"), &path).unwrap();
        cached.refresh(home, &["host-key.pem"], false, || {
            panic!("unsafe file read")
        });
        assert_eq!(cached.value, None);
    }
}

#[test]
fn corrupt_identity_or_registry_cancels_live_workers_without_reparsing_each_poll() {
    for name in ["devices.json", "host-key.pem", "host-identity.json"] {
        let (fixture, signer, mut manager) = configured();
        let held = hold(&mut manager, fixture.pairing.clone());
        let id = fixture.pairing.device_id;
        let path = fixture.state.coven_home.join("mobile").join(name);
        let original = std::fs::read(&path).unwrap();
        let measured = Measurement::start(vec![path.clone()]);
        let state = Arc::new(fixture.state);
        let now = Instant::now();
        manager.tick(&state, &signer, &Stop::default(), now);
        atomic_replace_private(&path, b"invalid").unwrap();
        for _ in 0..10 {
            manager.tick(&state, &signer, &Stop::default(), now);
        }
        assert!(
            manager.workers[&id].control.cancelled(),
            "{name} corruption left worker active"
        );
        assert_eq!(
            measured.count(&path).reads,
            2,
            "{name} was read again without change/event"
        );
        atomic_replace_private(&path, &original).unwrap();
        manager.tick(&state, &signer, &Stop::default(), now);
        assert!(manager.registry.value.is_some());
        assert_eq!(manager.identity.value, Some(true));
        assert_eq!(measured.count(&path).reads, 3);
        drop(held);
    }
}
