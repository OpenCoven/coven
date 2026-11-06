//! Real v2 crypto/files and signed request dispatch under deterministic changes.
use super::*;
use crate::mobile_memory::{
    grant::AssuranceLevel,
    registry::{DeviceGrantReissueRequest, DeviceGrantRevision},
    transport::{
        routing::{channel, Transport},
        storage::v2,
        Records,
    },
};
use anyhow::ensure;
use std::{
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc,
    },
    time::Duration,
};
#[derive(Debug, Clone, Copy)]
enum Change {
    Revoke,
    Rotate,
    RotateNoise,
    Tighten,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    Proof,
    Commit,
    Completion,
    Response,
}
#[derive(Clone)]
struct Mutation {
    change: Change,
    registry: Arc<DeviceRegistry>,
    home: std::path::PathBuf,
    id: Uuid,
    replacement: Option<Uuid>,
    applied: Arc<AtomicBool>,
}
impl Mutation {
    fn new(f: &Fixture, change: Change) -> Self {
        let replacement = if matches!(change, Change::Rotate) {
            let key = SigningKey::from_slice(&[43; 32]).unwrap();
            let public =
                URL_SAFE_NO_PAD.encode(key.verifying_key().to_sec1_point(false).as_bytes());
            let mut record = f
                .state
                .registry
                .authorization_record(f.pairing.device_id)
                .unwrap()
                .unwrap()
                .device;
            record.id = Uuid::new_v4();
            record.public_key_x963 = public.clone();
            record.display_name = "Synthetic rotated peer".into();
            let grant =
                DeviceGrant::for_device(record.id, &public, record.scopes.clone(), Utc::now())
                    .unwrap();
            let id = record.id;
            f.state.registry.register_with_grant(record, grant).unwrap();
            Some(id)
        } else {
            None
        };
        Self {
            change,
            registry: Arc::clone(&f.state.registry),
            home: f.state.coven_home.clone(),
            id: f.pairing.device_id,
            replacement,
            applied: Arc::new(AtomicBool::new(false)),
        }
    }
    fn apply(&self) -> Result<()> {
        ensure!(
            !self.applied.load(Ordering::Acquire),
            "authority mutation repeated"
        );
        self.registry.reload()?;
        let current = self
            .registry
            .authorization_record(self.id)?
            .context("source missing")?;
        match self.change {
            Change::Revoke => self.registry.revoke(self.id, Utc::now())?,
            Change::Rotate => {
                let replacement = self
                    .registry
                    .authorization_record(self.replacement.unwrap())?
                    .context("replacement missing")?;
                self.registry.rotate_devices_atomically(
                    DeviceGrantRevision::from(&current),
                    DeviceGrantRevision::from(&replacement),
                    Utc::now(),
                )?;
            }
            Change::RotateNoise => {
                let identity =
                    crate::mobile_memory::identity::load_existing_host_identity(&self.home)?;
                let host = SigningKey::from_pkcs8_der(&identity.private_key_der)?;
                let handover = v2::prepare_direct(&self.home, &self.registry, &host, self.id)?;
                ensure!(handover.generation == 1, "unexpected new Noise generation");
            }
            Change::Tighten => {
                let now = Utc::now();
                self.registry
                    .reissue_grant_atomically(DeviceGrantReissueRequest {
                        expected: DeviceGrantRevision::from(&current),
                        scopes: current.grant.scopes.clone(),
                        restrictions: current.grant.restrictions.clone(),
                        minimum_assurance: AssuranceLevel::FreshUserVerification,
                        expires_at: now + chrono::Duration::hours(1),
                        issued_at: now,
                        requested_policy_digest: URL_SAFE_NO_PAD.encode([7; 32]),
                    })?;
            }
        }
        self.applied.store(true, Ordering::Release);
        Ok(())
    }
    fn assert_applied(&self) {
        assert!(
            self.applied.load(Ordering::Acquire),
            "authority mutation was never applied"
        );
        self.registry.reload().unwrap();
        let current = self
            .registry
            .authorization_record(self.id)
            .unwrap()
            .unwrap();
        if matches!(self.change, Change::RotateNoise) {
            assert_eq!(current.grant.revocation_epoch, 0);
        } else {
            assert!(current.grant.revocation_epoch > 0);
        }
        match self.change {
            Change::Revoke => assert!(current.device.revoked_at.is_some()),
            Change::Rotate => {
                assert!(current.device.revoked_at.is_some());
                assert!(self
                    .registry
                    .authorization_record(self.replacement.unwrap())
                    .unwrap()
                    .unwrap()
                    .device
                    .revoked_at
                    .is_none());
            }
            Change::RotateNoise => {
                let raw: serde_json::Value = serde_json::from_slice(
                    &std::fs::read(self.home.join(format!("mobile/transport-{}.json", self.id)))
                        .unwrap(),
                )
                .unwrap();
                assert_eq!(raw["pending"]["generation"].as_u64(), Some(1));
                assert!(
                    raw["pending"]["material"]["noisePublic"]
                        != raw["committed"]["material"]["noisePublic"],
                    "Noise public key did not rotate"
                );
                assert!(current.device.revoked_at.is_none());
            }
            Change::Tighten => assert_eq!(
                current.grant.minimum_assurance,
                AssuranceLevel::FreshUserVerification
            ),
        }
    }
}
type SendHook<'a> = Box<dyn FnMut(usize) -> Result<()> + 'a>;
struct Pipe<'a> {
    sender: mpsc::Sender<Vec<u8>>,
    receiver: mpsc::Receiver<Vec<u8>>,
    hook: Option<SendHook<'a>>,
    sent: Arc<AtomicUsize>,
    ordinal: usize,
}
impl Records for Pipe<'_> {
    fn send(&mut self, bytes: &[u8], deadline: Instant) -> Result<()> {
        self.ordinal += 1;
        if let Some(hook) = self.hook.as_mut() {
            hook(self.ordinal)?;
        }
        ensure!(
            Instant::now() < deadline,
            "original record deadline expired"
        );
        self.sender.send(bytes.to_vec())?;
        self.sent.fetch_add(1, Ordering::AcqRel);
        Ok(())
    }
    fn receive(&mut self, deadline: Instant) -> Result<Vec<u8>> {
        Ok(self
            .receiver
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))?)
    }
    fn receive_idle(&mut self) -> Result<Vec<u8>> {
        // Parent sends immediately or drops its sender. This is only a fixture
        // hang guard; production idle and protocol deadlines are not changed.
        Ok(self.receiver.recv_timeout(Duration::from_secs(120))?)
    }
}
fn exercise(transport: Transport, stage: Stage, change: Option<Change>) {
    let f = Fixture::new();
    let id = f.pairing.device_id;
    let identity = load_or_create_host_identity(&f.state.coven_home, "192.0.2.1").unwrap();
    let host = SigningKey::from_pkcs8_der(&identity.private_key_der).unwrap();
    let mutation = change.map(|change| Mutation::new(&f, change));
    let handover = v2::prepare_direct(&f.state.coven_home, &f.state.registry, &host, id).unwrap();
    let phone = tempfile::tempdir().unwrap();
    let ack = v2::device::adopt_direct(
        phone.path(),
        handover,
        host.verifying_key(),
        id,
        f.device.verifying_key(),
    )
    .unwrap();
    v2::acknowledge_direct(&f.state.coven_home, &f.state.registry, &host, id, &ack).unwrap();
    let snapshot = v2::load(
        &f.state.coven_home,
        &f.state.registry,
        host.verifying_key(),
        id,
        transport,
    )
    .unwrap()
    .unwrap();
    let room = snapshot.rendezvous(transport, 0).unwrap().0;
    let (a_tx, a_rx) = mpsc::channel();
    let (b_tx, b_rx) = mpsc::channel();
    let sent = Arc::new(AtomicUsize::new(0));
    let dispatched = Arc::new(AtomicBool::new(false));
    let (server_result, client_result) = std::thread::scope(|scope| {
        let server = scope.spawn(|| -> Result<()> {
            let proof_mutation = mutation.clone();
            let pipe = Pipe {
                sender: b_tx,
                receiver: a_rx,
                sent: Arc::clone(&sent),
                ordinal: 0,
                hook: Some(Box::new(move |ordinal| {
                    if stage == Stage::Proof && ordinal == 3 {
                        if let Some(mutation) = &proof_mutation {
                            mutation.apply()?;
                        }
                    }
                    Ok(())
                })),
            };
            let mut channel = channel::accept(
                pipe,
                channel::HostAdmission {
                    pairing: &snapshot.material.pairing,
                    private: &snapshot.material.noise_private,
                    host: &host,
                    context: &room,
                    direct: transport == Transport::Direct,
                    proof: snapshot.state(transport).proof(0)?,
                },
                |pairing, _| {
                    v2::check_channel(
                        &f.state.coven_home,
                        &f.state.registry,
                        pairing,
                        transport,
                        Some(0),
                    )
                },
                |proof, deadline| {
                    v2::refresh_proof(
                        &f.state.coven_home,
                        &f.state.registry,
                        &snapshot.material.pairing,
                        transport,
                        proof,
                        deadline,
                    )
                },
                |proof| {
                    if stage == Stage::Commit {
                        if let Some(mutation) = &mutation {
                            mutation.apply()?;
                        }
                    }
                    v2::consume_verified(&f.state.coven_home, &f.state.registry, proof)?;
                    if stage == Stage::Completion {
                        if let Some(mutation) = &mutation {
                            mutation.apply()?;
                        }
                    }
                    Ok(())
                },
            )?;
            if stage == Stage::Response {
                if let Some(mutation) = mutation.clone() {
                    AFTER_ROUTING_DISPATCH.with(|slot| {
                        *slot.borrow_mut() = Some(Box::new(move |response| {
                            assert_eq!(
                                response.status, 200,
                                "positive protected handler did not succeed before invalidation"
                            );
                            mutation.apply().unwrap();
                        }))
                    });
                }
            }
            dispatched.store(true, Ordering::Release);
            serve_transport_with_state(&f.state, &mut channel)
        });
        let client = (|| -> Result<()> {
            let mut attempt = v2::device::begin(
                phone.path(),
                host.verifying_key(),
                id,
                f.device.verifying_key(),
                transport,
            )?;
            let mut channel = attempt.connect(
                Pipe {
                    sender: a_tx,
                    receiver: b_rx,
                    hook: None,
                    sent: Arc::new(AtomicUsize::new(0)),
                    ordinal: 0,
                },
                &f.device,
            )?;
            let deadline = Instant::now() + MOBILE_IO_TIMEOUT;
            channel.send_message(
                &f.request("GET", "/api/v1/mobile/device", id),
                MAX_MOBILE_REQUEST_BYTES,
                deadline,
            )?;
            let response = channel.receive_message(MAX_MOBILE_RESPONSE_BYTES, deadline)?;
            ensure!(
                response.starts_with(b"HTTP/1.1 200 "),
                "protected response was not successful"
            );
            Ok(())
        })();
        (
            server.join().unwrap().map_err(|e| format!("{e:#}")),
            client.map_err(|e| format!("{e:#}")),
        )
    });
    let raw: serde_json::Value = serde_json::from_slice(
        &std::fs::read(
            f.state
                .coven_home
                .join(format!("mobile/transport-{id}.json")),
        )
        .unwrap(),
    )
    .unwrap();
    let slot = usize::from(transport == Transport::Relay);
    let expected = if matches!(stage, Stage::Proof | Stage::Commit) && change.is_some() {
        0
    } else {
        1
    };
    assert_eq!(raw["committed"]["floors"][slot].as_u64(), Some(expected));
    assert_eq!(raw["committed"]["floors"][1 - slot].as_u64(), Some(0));
    let device = v2::device::begin(
        phone.path(),
        host.verifying_key(),
        id,
        f.device.verifying_key(),
        transport,
    )
    .unwrap()
    .state();
    assert_eq!((device.generation, device.next, device.floor), (0, 1, 0));
    let other = if transport == Transport::Direct {
        Transport::Relay
    } else {
        Transport::Direct
    };
    let other_state = v2::device::begin(
        phone.path(),
        host.verifying_key(),
        id,
        f.device.verifying_key(),
        other,
    )
    .unwrap()
    .state();
    assert_eq!(
        (other_state.generation, other_state.next, other_state.floor),
        (0, 0, 0)
    );
    if let Some(mutation) = mutation {
        mutation.assert_applied();
        let error = server_result.as_ref().expect_err("stale server succeeded");
        assert_eq!(
            error,
            match mutation.change {
                Change::Tighten => "grant changed",
                Change::RotateNoise => "routing handover pending",
                Change::Revoke | Change::Rotate => "device unavailable",
            },
            "{transport:?} {stage:?} {change:?}: wrong refusal class"
        );
        assert!(
            client_result.is_err(),
            "stale application response reached client"
        );
        assert_eq!(
            sent.load(Ordering::Acquire),
            if matches!(stage, Stage::Proof | Stage::Commit) {
                3
            } else {
                4
            },
            "wrong handshake/response record boundary"
        );
        assert_eq!(dispatched.load(Ordering::Acquire), stage == Stage::Response);
        let loaded = v2::load(
            &f.state.coven_home,
            &f.state.registry,
            host.verifying_key(),
            id,
            transport,
        );
        if matches!(mutation.change, Change::RotateNoise) {
            assert!(matches!(loaded, Ok(None)));
        } else {
            assert!(loaded.is_err());
        }
    } else {
        assert!(
            server_result.is_ok(),
            "positive server failed: {server_result:?}"
        );
        assert!(
            client_result.is_ok(),
            "positive client failed: {client_result:?}"
        );
        assert_eq!(sent.load(Ordering::Acquire), 5);
    }
    println!("v2-authority transport={transport:?} stage={stage:?} change={change:?} host_floor={expected} response_records={} verified", sent.load(Ordering::Acquire).saturating_sub(4));
}
#[test]
fn v2_authority_changes_at_proof_commit_completion_and_response_release_no_stale_response() {
    for transport in [Transport::Direct, Transport::Relay] {
        exercise(transport, Stage::Response, None);
        for change in [
            Change::Revoke,
            Change::Rotate,
            Change::RotateNoise,
            Change::Tighten,
        ] {
            for stage in [
                Stage::Proof,
                Stage::Commit,
                Stage::Completion,
                Stage::Response,
            ] {
                exercise(transport, stage, Some(change));
            }
        }
    }
}

/// Reusable production-channel exchange for control-loss and logical-time cases.
/// The optional date is passed only to the existing authority validity checker;
/// actual protocol I/O keeps the unchanged monotonic deadline.
pub(super) fn exchange_files(
    f: &Fixture,
    phone: &std::path::Path,
    transport: Transport,
    lose_device_proof: bool,
    policy_time: Option<chrono::DateTime<Utc>>,
) -> (Result<(), String>, Result<(), String>) {
    let id = f.pairing.device_id;
    let identity =
        crate::mobile_memory::identity::load_existing_host_identity(&f.state.coven_home).unwrap();
    let host = SigningKey::from_pkcs8_der(&identity.private_key_der).unwrap();
    let snapshot = v2::load(
        &f.state.coven_home,
        &f.state.registry,
        host.verifying_key(),
        id,
        transport,
    )
    .unwrap()
    .unwrap();
    let mut attempt = v2::device::begin(
        phone,
        host.verifying_key(),
        id,
        f.device.verifying_key(),
        transport,
    )
    .unwrap();
    let counter = attempt.state().next;
    let room = attempt.rendezvous().unwrap().0;
    let (a_tx, a_rx) = mpsc::channel();
    let (b_tx, b_rx) = mpsc::channel();
    let lost = Arc::new(AtomicBool::new(false));
    let result = std::thread::scope(|scope| {
        let server = scope.spawn(|| -> Result<()> {
            channel::accept(
                Pipe {
                    sender: b_tx,
                    receiver: a_rx,
                    hook: None,
                    sent: Arc::new(AtomicUsize::new(0)),
                    ordinal: 0,
                },
                channel::HostAdmission {
                    pairing: &snapshot.material.pairing,
                    private: &snapshot.material.noise_private,
                    host: &host,
                    context: &room,
                    direct: transport == Transport::Direct,
                    proof: snapshot.state(transport).proof(counter)?,
                },
                |pairing, direct| {
                    crate::mobile_memory::transport::authority::check_current(
                        &f.state.registry,
                        pairing,
                        direct,
                        policy_time.unwrap_or_else(Utc::now),
                    )?;
                    v2::check_channel(
                        &f.state.coven_home,
                        &f.state.registry,
                        pairing,
                        transport,
                        Some(snapshot.generation),
                    )
                },
                |proof, deadline| {
                    v2::refresh_proof(
                        &f.state.coven_home,
                        &f.state.registry,
                        &snapshot.material.pairing,
                        transport,
                        proof,
                        deadline,
                    )
                },
                |proof| v2::consume_verified(&f.state.coven_home, &f.state.registry, proof),
            )
            .map(|_| ())
        });
        let observed = Arc::clone(&lost);
        let client = attempt
            .connect(
                Pipe {
                    sender: a_tx,
                    receiver: b_rx,
                    sent: Arc::new(AtomicUsize::new(0)),
                    ordinal: 0,
                    hook: Some(Box::new(move |ordinal| {
                        if lose_device_proof && ordinal == 3 {
                            observed.store(true, Ordering::Release);
                            anyhow::bail!("injected complete device-proof loss");
                        }
                        Ok(())
                    })),
                },
                &f.device,
            )
            .map(|_| ())
            .map_err(|e| format!("{e:#}"));
        (server.join().unwrap().map_err(|e| format!("{e:#}")), client)
    });
    assert_eq!(
        lost.load(Ordering::Acquire),
        lose_device_proof,
        "configured proof loss was not observed"
    );
    result
}
