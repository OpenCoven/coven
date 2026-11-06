//! Enumerated host/device file snapshots with production crypto and persistence.
use super::*;
use crate::mobile_memory::transport::{
    routing::{channel, DeviceState},
    storage::v2::{device, tests::Fixture, Snapshot},
    Records,
};
use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc, Arc,
    },
    time::{Duration, Instant},
};

#[derive(Debug, Clone, Copy)]
enum Loss {
    Device(usize),
    Host(usize),
}
struct Pipe {
    sender: Option<mpsc::Sender<Vec<u8>>>,
    receiver: mpsc::Receiver<Vec<u8>>,
    ordinal: usize,
    drop_at: Option<usize>,
    observed: Arc<AtomicUsize>,
}
impl Pipe {
    fn new(
        sender: mpsc::Sender<Vec<u8>>,
        receiver: mpsc::Receiver<Vec<u8>>,
        drop_at: Option<usize>,
        observed: Arc<AtomicUsize>,
    ) -> Self {
        Self {
            sender: Some(sender),
            receiver,
            ordinal: 0,
            drop_at,
            observed,
        }
    }
}
impl Records for Pipe {
    fn send(&mut self, bytes: &[u8], deadline: Instant) -> Result<()> {
        ensure!(Instant::now() < deadline, "record deadline expired");
        self.ordinal += 1;
        if Some(self.ordinal) == self.drop_at {
            // Accepted write, lost encrypted record, then directional EOF.
            // No shortened challenge deadline substitutes for message loss.
            self.observed.fetch_add(1, Ordering::SeqCst);
            self.sender = None;
            return Ok(());
        }
        self.sender
            .as_ref()
            .context("fixture direction closed after record loss")?
            .send(bytes.to_vec())?;
        Ok(())
    }
    fn receive(&mut self, deadline: Instant) -> Result<Vec<u8>> {
        Ok(self
            .receiver
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))?)
    }
    fn receive_idle(&mut self) -> Result<Vec<u8>> {
        // Hang guard only for a fixture pipe without peer-liveness probes.
        self.receive(Instant::now() + Duration::from_secs(30))
    }
}
fn exchange(
    f: &Fixture,
    device_home: &Path,
    snapshot: &Snapshot,
    transport: Transport,
) -> (Result<(), String>, Result<(), String>, usize) {
    exchange_loss(f, device_home, snapshot, transport, None)
}
fn exchange_loss(
    f: &Fixture,
    device_home: &Path,
    snapshot: &Snapshot,
    transport: Transport,
    loss: Option<Loss>,
) -> (Result<(), String>, Result<(), String>, usize) {
    let mut attempt = device::begin(
        device_home,
        f.host.verifying_key(),
        f.id,
        f.device.verifying_key(),
        transport,
    )
    .unwrap();
    let counter = attempt.state().next;
    let (room, _) = attempt.rendezvous().unwrap();
    let (a_tx, a_rx) = mpsc::channel();
    let (b_tx, b_rx) = mpsc::channel();
    let observed = Arc::new(AtomicUsize::new(0));
    let host_observed = observed.clone();
    std::thread::scope(|scope| {
        let host = scope.spawn(move || -> Result<()> {
            let admission = channel::HostAdmission {
                pairing: &snapshot.material.pairing,
                private: &snapshot.material.noise_private,
                host: &f.host,
                context: &room,
                direct: transport == Transport::Direct,
                proof: snapshot.state(transport).proof(counter)?,
            };
            let mut channel = channel::accept(
                Pipe::new(
                    b_tx,
                    a_rx,
                    match loss {
                        Some(Loss::Host(n)) => Some(n),
                        _ => None,
                    },
                    host_observed,
                ),
                admission,
                |pairing, _| {
                    super::super::super::check_channel(
                        f.home.path(),
                        &f.registry,
                        pairing,
                        transport,
                        Some(snapshot.generation),
                    )
                },
                |proof, deadline| {
                    super::super::super::refresh_proof(
                        f.home.path(),
                        &f.registry,
                        &snapshot.material.pairing,
                        transport,
                        proof,
                        deadline,
                    )
                },
                |verified| {
                    super::super::super::consume_verified(f.home.path(), &f.registry, verified)
                },
            )?;
            if loss.is_some() {
                return Ok(());
            }
            let deadline = Instant::now() + Duration::from_secs(30);
            ensure!(
                channel.receive(deadline)?.as_slice() == b"device-ready",
                "wrong encrypted request record"
            );
            super::super::super::with_channel_current(
                f.home.path(),
                &f.registry,
                &snapshot.material.pairing,
                transport,
                Some(snapshot.generation),
                || channel.send(b"snapshot-restored", deadline),
            )
        });
        let client = (|| -> Result<()> {
            let mut channel = attempt.connect(
                Pipe::new(
                    a_tx,
                    b_rx,
                    match loss {
                        Some(Loss::Device(n)) => Some(n),
                        _ => None,
                    },
                    observed.clone(),
                ),
                &f.device,
            )?;
            if loss.is_some() {
                return Ok(());
            }
            let deadline = Instant::now() + Duration::from_secs(30);
            channel.send(b"device-ready", deadline)?;
            ensure!(
                channel.receive(deadline)?.as_slice() == b"snapshot-restored",
                "wrong encrypted response record"
            );
            Ok(())
        })()
        .map_err(|e| format!("{e:#}"));
        drop(attempt);
        (
            client,
            host.join().unwrap().map_err(|e| format!("{e:#}")),
            observed.load(Ordering::SeqCst),
        )
    })
}
fn adopt(f: &Fixture, home: &Path, handover: Handover) {
    let ack = device::adopt_direct(
        home,
        handover,
        f.host.verifying_key(),
        f.id,
        f.device.verifying_key(),
    )
    .unwrap();
    acknowledge_direct(f.home.path(), &f.registry, &f.host, f.id, &ack).unwrap();
}

#[test]
fn snapshot_restore_matrix_rejects_stale_channels_and_uses_fresh_device_handover() {
    for restored in ["host", "device", "joint"] {
        let f = Fixture::new();
        let device_home = tempfile::tempdir().unwrap();
        let old_handover = f.prepare();
        adopt(&f, device_home.path(), old_handover.clone());
        let host_path = f
            .home
            .path()
            .join(format!("mobile/transport-{}.json", f.id));
        let registry_path = f.home.path().join("mobile/devices.json");
        let device_path = device_home
            .path()
            .join(format!("mobile/device-transport-{}.json", f.id));
        let old_host = fs::read(&host_path).unwrap();
        let old_registry = fs::read(&registry_path).unwrap();
        let old_device = fs::read(&device_path).unwrap();
        for transport in [Transport::Direct, Transport::Relay] {
            for _ in 0..2 {
                let current = f.snapshot().unwrap();
                let results = exchange(&f, device_home.path(), &current, transport);
                assert!(
                    results.0.is_ok() && results.1.is_ok(),
                    "{restored}: {results:?}"
                );
            }
        }
        assert_eq!(f.snapshot().unwrap().floors, [2, 2]);
        let stale_worker = f.snapshot().unwrap();
        let backup = tempfile::tempdir().unwrap();
        let authority = ensure_private_mobile_dir(backup.path())
            .unwrap()
            .join("authority.json");
        export::export_authority(f.home.path(), &authority).unwrap();
        if restored != "device" {
            atomic_replace_private(&host_path, &old_host).unwrap();
            atomic_replace_private(&registry_path, &old_registry).unwrap();
        }
        if restored != "host" {
            atomic_replace_private(&device_path, &old_device).unwrap();
        }
        let pin = identity::load_existing_host_identity(f.home.path())
            .unwrap()
            .public_key_fingerprint;
        install(f.home.path(), &authority, pin, Uuid::new_v4()).unwrap();
        let installed_host = fs::read(&host_path).unwrap();
        let before_attempt = fs::read(&device_path).unwrap();
        for transport in [Transport::Direct, Transport::Relay] {
            let rejected = exchange(&f, device_home.path(), &stale_worker, transport);
            assert!(
                rejected.0.is_err() && rejected.1.is_err(),
                "{restored} stale {transport:?}: {rejected:?}"
            );
        }
        assert_eq!(
            fs::read(&host_path).unwrap(),
            installed_host,
            "{restored}: stale worker changed host state"
        );
        assert_eq!(
            fs::read(&device_path).unwrap(),
            before_attempt,
            "{restored}: rejected proof changed device state"
        );
        assert!(check_application(f.home.path(), &f.registry, f.id).is_err());
        let fresh = f.prepare();
        assert_eq!(fresh.generation, 1);
        assert_eq!(fresh.revocation_epoch, 1);
        assert_ne!(fresh.routing_seed, old_handover.routing_seed);
        adopt(&f, device_home.path(), fresh);
        assert_eq!(f.snapshot().unwrap().floors, [0, 0]);
        for transport in [Transport::Direct, Transport::Relay] {
            let current = f.snapshot().unwrap();
            let results = exchange(&f, device_home.path(), &current, transport);
            assert!(
                results.0.is_ok() && results.1.is_ok(),
                "{restored} fresh {transport:?}: {results:?}"
            );
            let device = device::begin(
                device_home.path(),
                f.host.verifying_key(),
                f.id,
                f.device.verifying_key(),
                transport,
            )
            .unwrap();
            assert_eq!(
                device.state(),
                DeviceState {
                    generation: 1,
                    next: 1,
                    floor: 0
                }
            );
        }
        assert_eq!(f.snapshot().unwrap().floors, [1, 1]);
        check_application(f.home.path(), &f.registry, f.id).unwrap();
    }
}

fn paired() -> (Fixture, tempfile::TempDir) {
    let f = Fixture::new();
    let home = tempfile::tempdir().unwrap();
    let handover = f.prepare();
    adopt(&f, home.path(), handover);
    (f, home)
}
fn state(f: &Fixture, home: &Path, transport: Transport) -> DeviceState {
    device::begin(
        home,
        f.host.verifying_key(),
        f.id,
        f.device.verifying_key(),
        transport,
    )
    .unwrap()
    .state()
}
#[test]
fn actual_channel_each_dropped_record_has_exact_durable_effect_and_can_retry() {
    for transport in [Transport::Direct, Transport::Relay] {
        for loss in [
            Loss::Device(1),
            Loss::Device(2),
            Loss::Device(3),
            Loss::Device(4),
            Loss::Host(1),
            Loss::Host(2),
            Loss::Host(3),
            Loss::Host(4),
        ] {
            let (f, home) = paired();
            let outcome = exchange_loss(
                &f,
                home.path(),
                &f.snapshot().unwrap(),
                transport,
                Some(loss),
            );
            assert_eq!(
                outcome.2, 1,
                "{transport:?} {loss:?}: fault did not execute: {outcome:?}"
            );
            assert!(
                outcome.1.is_err(),
                "{transport:?} {loss:?}: host claimed completion: {outcome:?}"
            );
            assert_eq!(
                outcome.0.is_ok(),
                matches!(loss, Loss::Device(4)),
                "{transport:?} {loss:?}: {outcome:?}"
            );
            let (d, c) = match loss {
                Loss::Device(3) => (1, 0),
                Loss::Device(4) | Loss::Host(4) => (1, 1),
                _ => (0, 0),
            };
            assert_eq!(
                state(&f, home.path(), transport),
                DeviceState {
                    generation: 0,
                    next: d,
                    floor: 0
                },
                "{transport:?} {loss:?}"
            );
            assert_eq!(
                f.snapshot().unwrap().state(transport).floor,
                c,
                "{transport:?} {loss:?}"
            );
            let other = if transport == Transport::Direct {
                Transport::Relay
            } else {
                Transport::Direct
            };
            assert_eq!(state(&f, home.path(), other).next, 0);
            assert_eq!(f.snapshot().unwrap().state(other).floor, 0);
            let retry = exchange(&f, home.path(), &f.snapshot().unwrap(), transport);
            assert!(
                retry.0.is_ok() && retry.1.is_ok(),
                "{transport:?} {loss:?} retry: {retry:?}"
            );
            assert_eq!(state(&f, home.path(), transport).next, d + 1);
            assert_eq!(f.snapshot().unwrap().state(transport).floor, d + 1);
        }
    }
}
#[test]
fn actual_channel_repeated_completion_losses_do_not_exhaust_window() {
    for transport in [Transport::Direct, Transport::Relay] {
        for loss in [Loss::Host(4), Loss::Device(4)] {
            let (f, home) = paired();
            for counter in 0..8 {
                let outcome = exchange_loss(
                    &f,
                    home.path(),
                    &f.snapshot().unwrap(),
                    transport,
                    Some(loss),
                );
                assert_eq!(outcome.2, 1);
                assert!(outcome.1.is_err(), "{loss:?}: {outcome:?}");
                assert_eq!(outcome.0.is_ok(), matches!(loss, Loss::Device(4)));
                assert_eq!(
                    state(&f, home.path(), transport),
                    DeviceState {
                        generation: 0,
                        next: counter + 1,
                        floor: counter
                    }
                );
                assert_eq!(f.snapshot().unwrap().state(transport).floor, counter + 1);
            }
            let resumed = exchange(&f, home.path(), &f.snapshot().unwrap(), transport);
            assert!(
                resumed.0.is_ok() && resumed.1.is_ok(),
                "{loss:?}: {resumed:?}"
            );
            assert_eq!(state(&f, home.path(), transport).next, 9);
        }
    }
}
fn direct_resync(f: &Fixture, home: &Path, transport: Transport, next: u64) {
    let resynced = super::super::super::resync_direct(
        f.home.path(),
        &f.registry,
        &f.host,
        f.id,
        transport,
        0,
        next,
    )
    .unwrap();
    device::resync_direct(
        home,
        f.host.verifying_key(),
        f.id,
        f.device.verifying_key(),
        transport,
        resynced,
    )
    .unwrap();
}
#[test]
fn actual_channel_four_lost_device_proofs_require_direct_recovery() {
    for transport in [Transport::Direct, Transport::Relay] {
        let (f, home) = paired();
        for counter in 0..4 {
            let outcome = exchange_loss(
                &f,
                home.path(),
                &f.snapshot().unwrap(),
                transport,
                Some(Loss::Device(3)),
            );
            assert_eq!(outcome.2, 1);
            assert!(outcome.0.is_err() && outcome.1.is_err());
            assert_eq!(f.snapshot().unwrap().state(transport).floor, 0);
            if counter < 3 {
                assert_eq!(state(&f, home.path(), transport).next, counter + 1);
            }
        }
        assert!(device::begin(
            home.path(),
            f.host.verifying_key(),
            f.id,
            f.device.verifying_key(),
            transport
        )
        .is_err());
        direct_resync(&f, home.path(), transport, 4);
        let resumed = exchange(&f, home.path(), &f.snapshot().unwrap(), transport);
        assert!(
            resumed.0.is_ok() && resumed.1.is_ok(),
            "{transport:?}: {resumed:?}"
        );
        assert_eq!(
            state(&f, home.path(), transport),
            DeviceState {
                generation: 0,
                next: 5,
                floor: 4
            }
        );
    }
}
#[test]
fn actual_channel_combined_loss_reproduces_frozen_w4_direct_recovery_edge() {
    for transport in [Transport::Direct, Transport::Relay] {
        for completion_lost in [false, true] {
            let (f, home) = paired();
            for _ in 0..3 {
                let outcome = exchange_loss(
                    &f,
                    home.path(),
                    &f.snapshot().unwrap(),
                    transport,
                    Some(Loss::Device(3)),
                );
                assert_eq!(outcome.2, 1);
                assert!(outcome.0.is_err() && outcome.1.is_err());
            }
            let outcome = if completion_lost {
                exchange_loss(
                    &f,
                    home.path(),
                    &f.snapshot().unwrap(),
                    transport,
                    Some(Loss::Host(4)),
                )
            } else {
                exchange(&f, home.path(), &f.snapshot().unwrap(), transport)
            };
            assert_eq!(outcome.2, usize::from(completion_lost));
            assert_eq!(outcome.0.is_err(), completion_lost);
            assert_eq!(outcome.1.is_err(), completion_lost);
            assert_eq!(f.snapshot().unwrap().state(transport).floor, 4);
            let bytes = fs::read(
                home.path()
                    .join(format!("mobile/device-transport-{}.json", f.id)),
            )
            .unwrap();
            let stored: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            let index = if transport == Transport::Direct { 0 } else { 1 };
            assert_eq!(stored["states"][index]["next"], 4);
            assert_eq!(stored["states"][index]["floor"], 0);
            assert!(
                device::begin(
                    home.path(),
                    f.host.verifying_key(),
                    f.id,
                    f.device.verifying_key(),
                    transport
                )
                .is_err(),
                "frozen equations must not silently allow a fifth remote attempt"
            );
            direct_resync(&f, home.path(), transport, 4);
            assert_eq!(
                state(&f, home.path(), transport),
                DeviceState {
                    generation: 0,
                    next: 4,
                    floor: 4
                }
            );
        }
    }
}
