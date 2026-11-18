//! Freshly authenticated channel handoff with immutable signed operations.
//! A network error leaves the operation's identity and signature untouched.
use super::{
    connection::{Attempt, ConnectionState, ConnectionStatus, Connections, Observation, Transport},
    Channel, Pairing, Records,
};
use crate::mobile_memory::{
    auth::canonical_request, MAX_MOBILE_REQUEST_BYTES, MAX_MOBILE_RESPONSE_BYTES,
};
use anyhow::{ensure, Context, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use chrono::Utc;
use p256::ecdsa::{signature::Signer, Signature, SigningKey};
use sha2::{Digest, Sha256};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::time::{Duration, Instant};
use uuid::Uuid;
use zeroize::Zeroizing;

type AnyRecords = Box<dyn Records + Send>;

impl<R: Records + ?Sized> Records for Box<R> {
    fn send(&mut self, bytes: &[u8], deadline: Instant) -> Result<()> {
        (**self).send(bytes, deadline)
    }
    fn receive(&mut self, deadline: Instant) -> Result<Vec<u8>> {
        (**self).receive(deadline)
    }
    fn receive_idle(&mut self) -> Result<Vec<u8>> {
        (**self).receive_idle()
    }
}

struct Pin {
    scope: Uuid,
    pairing: Pairing,
    binding: Vec<u8>,
    device: SigningKey,
}
struct Live {
    ticket: Uuid,
    channel: Mutex<Channel<AnyRecords>>,
    online: AtomicBool,
    transport: Transport,
    _attempt: Attempt,
}
#[derive(Default)]
struct State {
    active: Option<Arc<Live>>,
    candidate: Option<(Uuid, Observation)>,
}

/// One pinned pairing. Neither network changes nor errors replace this pin.
/// Channel I/O does not hold the handoff lock: old work may finish during a
/// candidate handshake, and stale I/O failure cannot clear a new channel.
pub struct Migration {
    pin: Arc<Pin>,
    state: Mutex<State>,
    connections: Connections,
}

pub struct Candidate {
    pin: Arc<Pin>,
    ticket: Uuid,
    transport: Transport,
    attempt: Attempt,
}

/// Owns the existing durable v2 device lock only through authentication.
/// No copied counters or caller-supplied persistence callback are accepted.
pub struct RoutedCandidate {
    candidate: Candidate,
    routing: super::storage::v2::device::Attempt,
}

pub struct AuthenticatedCandidate {
    candidate: Candidate,
    channel: Channel<AnyRecords>,
}

/// An immutable signed request, retained by the caller across uncertain I/O.
/// Never regenerate its nonce/signature to retry an uncertain side effect.
pub struct PendingOperation {
    scope: Uuid,
    id: [u8; 32],
    request: Zeroizing<Vec<u8>>,
}
impl PendingOperation {
    pub fn id(&self) -> [u8; 32] {
        self.id
    }
}

pub struct OperationResponse {
    pub operation_id: [u8; 32],
    pub bytes: Zeroizing<Vec<u8>>,
}

impl Migration {
    pub fn new(pairing: Pairing, binding: Vec<u8>, device: SigningKey) -> Result<Self> {
        pairing.verify_noise_binding(&binding)?;
        ensure!(
            device.verifying_key() == &pairing.device_key,
            "device identity mismatch"
        );
        Ok(Self {
            pin: Arc::new(Pin {
                scope: Uuid::new_v4(),
                pairing,
                binding,
                device,
            }),
            state: Mutex::new(State::default()),
            connections: Connections::default(),
        })
    }

    pub fn begin(&self, transport: Transport) -> Result<Candidate> {
        let attempt = self.connections.begin(transport)?;
        let ticket = Uuid::new_v4();
        let observation = attempt.observation()?;
        self.state
            .lock()
            .map_err(|_| anyhow::anyhow!("connection handoff unavailable"))?
            .candidate = Some((ticket, observation));
        Ok(Candidate {
            pin: Arc::clone(&self.pin),
            ticket,
            transport,
            attempt,
        })
    }

    pub fn commit(&self, mut authenticated: AuthenticatedCandidate) -> Result<()> {
        ensure!(
            Arc::ptr_eq(&authenticated.candidate.pin, &self.pin),
            "candidate belongs to another pairing"
        );
        let mut state = self
            .state
            .lock()
            .map_err(|_| anyhow::anyhow!("connection handoff unavailable"))?;
        ensure!(
            state
                .candidate
                .as_ref()
                .is_some_and(|(ticket, _)| *ticket == authenticated.candidate.ticket),
            "stale connection candidate"
        );
        ensure!(authenticated.channel.is_open(), "candidate channel closed");
        authenticated.candidate.attempt.online()?;
        let previous = state.active.replace(Arc::new(Live {
            ticket: authenticated.candidate.ticket,
            channel: Mutex::new(authenticated.channel),
            online: AtomicBool::new(true),
            transport: authenticated.candidate.transport,
            _attempt: authenticated.candidate.attempt,
        }));
        // Records may own transport cleanup. Never run that cleanup while the
        // handoff lock blocks a new candidate or a status observation.
        drop(state);
        drop(previous);
        Ok(())
    }

    pub fn status(&self) -> Result<Option<ConnectionStatus>> {
        let state = self
            .state
            .lock()
            .map_err(|_| anyhow::anyhow!("connection handoff unavailable"))?;
        if let Some(active) = &state.active {
            if active.online.load(Ordering::Acquire) {
                return Ok(Some(ConnectionStatus {
                    state: ConnectionState::Online,
                    transport: active.transport,
                }));
            }
        }
        if let Some(active) = &state.active {
            if state
                .candidate
                .as_ref()
                .is_some_and(|(ticket, _)| *ticket == active.ticket)
            {
                return Ok(Some(ConnectionStatus {
                    state: ConnectionState::Offline,
                    transport: active.transport,
                }));
            }
        }
        state
            .candidate
            .as_ref()
            .map(|(_, observation)| observation.snapshot())
            .transpose()
    }

    pub fn prepare(&self, method: &str, path: &str, body: &[u8]) -> Result<PendingOperation> {
        ensure!(
            matches!(method, "GET" | "POST" | "DELETE"),
            "unsupported mobile operation method"
        );
        ensure!(
            path.starts_with("/api/v1/mobile/")
                && path.is_ascii()
                && !path.bytes().any(|byte| byte.is_ascii_whitespace()
                    || byte.is_ascii_control()
                    || matches!(byte, b'?' | b'#')),
            "invalid mobile operation target"
        );
        ensure!(
            body.len() <= MAX_MOBILE_REQUEST_BYTES,
            "mobile operation body exceeds limit"
        );
        let id = super::nonce()?;
        let nonce = URL_SAFE_NO_PAD.encode(id);
        let timestamp = Utc::now().timestamp();
        let digest = URL_SAFE_NO_PAD.encode(Sha256::digest(body));
        let canonical = canonical_request(method, path, timestamp, &nonce, &digest)
            .map_err(|_| anyhow::anyhow!("invalid mobile operation signing preimage"))?;
        let signature: Signature = self.pin.device.sign(&canonical);
        let signature = URL_SAFE_NO_PAD.encode(signature.to_der().as_bytes());
        let mut request = Zeroizing::new(format!("{method} {path} HTTP/1.1\r\nx-coven-protocol: 1\r\nx-coven-device: {}\r\nx-coven-timestamp: {timestamp}\r\nx-coven-nonce: {nonce}\r\nx-coven-body-sha256: {digest}\r\nx-coven-signature: {signature}\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n", self.pin.pairing.device_id, body.len()).into_bytes());
        request.extend_from_slice(body);
        Ok(PendingOperation {
            scope: self.pin.scope,
            id,
            request,
        })
    }

    /// An error after dispatch is an uncertain outcome. The caller retains the
    /// exact operation; this method never re-signs, re-enrolls or retries it.
    pub fn exchange(&self, pending: &PendingOperation) -> Result<OperationResponse> {
        ensure!(
            pending.scope == self.pin.scope,
            "operation belongs to another pairing"
        );
        let active = self
            .state
            .lock()
            .map_err(|_| anyhow::anyhow!("connection handoff unavailable"))?
            .active
            .clone()
            .context("no authenticated connection")?;
        let mut channel = active
            .channel
            .lock()
            .map_err(|_| anyhow::anyhow!("connection unavailable"))?;
        let deadline = Instant::now() + Duration::from_secs(15);
        let result = channel
            .send_message(
                &pending.request,
                MAX_MOBILE_REQUEST_BYTES + 16 * 1024,
                deadline,
            )
            .and_then(|()| {
                channel.receive_message(MAX_MOBILE_RESPONSE_BYTES + 16 * 1024, deadline)
            });
        match result {
            Ok(bytes) => Ok(OperationResponse {
                operation_id: pending.id,
                bytes,
            }),
            Err(error) => {
                active.online.store(false, Ordering::Release);
                Err(error).context(
                    "mobile operation outcome unknown; retain the original operation identity",
                )
            }
        }
    }
}

impl RoutedCandidate {
    pub fn rendezvous(&self) -> Result<([u8; 32], Zeroizing<[u8; 32]>)> {
        self.routing.rendezvous()
    }

    pub fn authenticate<R: Records + Send + 'static>(
        mut self,
        records: R,
    ) -> Result<AuthenticatedCandidate> {
        self.candidate.attempt.authenticating()?;
        let records: AnyRecords = Box::new(records);
        let channel = self.routing.connect(records, &self.candidate.pin.device)?;
        Ok(AuthenticatedCandidate {
            candidate: self.candidate,
            channel,
        })
    }
}

impl Candidate {
    /// V2 handoffs reuse the landed durable device counter implementation.
    /// Current stored pins must equal this controller's immutable pairing.
    pub fn routed(self, home: &std::path::Path) -> Result<RoutedCandidate> {
        let transport = match self.transport {
            Transport::Relay => super::routing::Transport::Relay,
            Transport::Direct => super::routing::Transport::Direct,
        };
        let expected = &self.pin.pairing;
        let routing = super::storage::v2::device::begin(
            home,
            &expected.host_key,
            expected.device_id,
            &expected.device_key,
            transport,
        )?;
        let actual = routing.pairing();
        ensure!(
            actual.device_id == expected.device_id
                && actual.grant_id == expected.grant_id
                && actual.revocation_epoch == expected.revocation_epoch
                && actual.host_key == expected.host_key
                && actual.device_key == expected.device_key
                && actual.noise_key == expected.noise_key,
            "routing pairing differs from pinned migration identity"
        );
        Ok(RoutedCandidate {
            candidate: self,
            routing,
        })
    }
    /// Every candidate runs a new Noise handshake and fresh possession proofs.
    /// Context comes from authenticated pairwise routing, never discovery.
    pub fn authenticate<R: Records + Send + 'static>(
        mut self,
        records: R,
        context: &[u8; 32],
    ) -> Result<AuthenticatedCandidate> {
        self.attempt.authenticating()?;
        let records: AnyRecords = Box::new(records);
        let channel = super::connect(
            records,
            &self.pin.pairing,
            &self.pin.binding,
            &self.pin.device,
            context,
            self.transport == Transport::Direct,
        )?;
        Ok(AuthenticatedCandidate {
            candidate: self,
            channel,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pending_operation_identity_and_signature_survive_failed_candidates() {
        let host = SigningKey::from_slice(&[11; 32]).unwrap();
        let device = SigningKey::from_slice(&[12; 32]).unwrap();
        let (_, noise_key) = super::super::generate_noise_key().unwrap();
        let pairing = Pairing {
            device_id: Uuid::new_v4(),
            grant_id: Uuid::new_v4(),
            revocation_epoch: 0,
            host_key: *host.verifying_key(),
            device_key: *device.verifying_key(),
            noise_key,
        };
        let binding = pairing.sign_noise_binding(&host).unwrap();
        let wrong_key = SigningKey::from_slice(&[13; 32]).unwrap();
        assert!(Migration::new(pairing.clone(), binding.clone(), wrong_key).is_err());
        let other_controller =
            Migration::new(pairing.clone(), binding.clone(), device.clone()).unwrap();
        let migration = Migration::new(pairing, binding, device).unwrap();
        let pending = migration
            .prepare("GET", "/api/v1/mobile/device", &[])
            .unwrap();
        let id = pending.id();
        let bytes = pending.request.to_vec();
        assert_eq!(
            other_controller
                .exchange(&pending)
                .err()
                .unwrap()
                .to_string(),
            "operation belongs to another pairing"
        );
        let candidate = migration.begin(Transport::Relay).unwrap();
        assert_eq!(
            migration.status().unwrap().unwrap().state,
            ConnectionState::Connecting
        );
        drop(candidate);
        assert_eq!(
            migration.status().unwrap().unwrap().state,
            ConnectionState::Offline
        );
        assert_eq!(id, pending.id());
        assert_eq!(bytes, pending.request.as_slice());
        assert!(migration
            .prepare("GET\r\nInjected:", "/api/v1/mobile/device", &[])
            .is_err());
        assert!(migration
            .prepare("GET", "/api/v1/mobile/device?query", &[])
            .is_err());
    }
    #[test]
    fn retired_channel_cleanup_releases_the_handoff_lock() {
        use super::super::{
            accept,
            tests::{pipes, Pipe},
        };
        use std::sync::mpsc;
        struct GatedDrop {
            inner: Pipe,
            entered: mpsc::Sender<()>,
            release: mpsc::Receiver<()>,
        }
        impl Records for GatedDrop {
            fn send(&mut self, b: &[u8], d: Instant) -> Result<()> {
                self.inner.send(b, d)
            }
            fn receive(&mut self, d: Instant) -> Result<Vec<u8>> {
                self.inner.receive(d)
            }
            fn receive_idle(&mut self) -> Result<Vec<u8>> {
                self.inner.receive_idle()
            }
        }
        impl Drop for GatedDrop {
            fn drop(&mut self) {
                let _ = self.entered.send(());
                // Hang guard only; the test releases cleanup explicitly.
                let _ = self.release.recv_timeout(Duration::from_secs(30));
            }
        }
        let host = SigningKey::from_slice(&[11; 32]).unwrap();
        let device = SigningKey::from_slice(&[12; 32]).unwrap();
        let (private, public) = super::super::generate_noise_key().unwrap();
        let pairing = Pairing {
            device_id: Uuid::new_v4(),
            grant_id: Uuid::new_v4(),
            revocation_epoch: 0,
            host_key: *host.verifying_key(),
            device_key: *device.verifying_key(),
            noise_key: public,
        };
        let migration = Migration::new(
            pairing.clone(),
            pairing.sign_noise_binding(&host).unwrap(),
            device,
        )
        .unwrap();
        let authenticate =
            |candidate: Candidate, gate: Option<(mpsc::Sender<()>, mpsc::Receiver<()>)>| {
                let (client, server, _) = pipes();
                let records: AnyRecords = match gate {
                    Some((entered, release)) => Box::new(GatedDrop {
                        inner: client,
                        entered,
                        release,
                    }),
                    None => Box::new(client),
                };
                std::thread::scope(|scope| {
                    let server = scope.spawn(|| {
                        accept(
                            server,
                            &pairing,
                            &private,
                            &host,
                            &[9; 32],
                            false,
                            |_, _| Ok(()),
                        )
                    });
                    let authenticated = candidate.authenticate(records, &[9; 32]).unwrap();
                    assert!(server.join().unwrap().unwrap().is_open());
                    authenticated
                })
            };
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        migration
            .commit(authenticate(
                migration.begin(Transport::Relay).unwrap(),
                Some((entered_tx, release_rx)),
            ))
            .unwrap();
        let replacement = authenticate(migration.begin(Transport::Relay).unwrap(), None);
        std::thread::scope(|scope| {
            let committed = scope.spawn(|| migration.commit(replacement));
            entered_rx
                .recv_timeout(Duration::from_secs(30))
                .expect("retired channel cleanup did not begin");
            let unlocked = migration.state.try_lock().is_ok();
            // Always unblock cleanup before an assertion can unwind the scope.
            release_tx.send(()).unwrap();
            assert!(
                unlocked,
                "retiring a channel held the migration handoff lock"
            );
            committed.join().unwrap().unwrap();
        });
        assert_eq!(
            migration.status().unwrap().unwrap().state,
            ConnectionState::Online
        );
    }

    #[test]
    fn old_exchange_failure_after_handoff_preserves_the_replacement() {
        use super::super::{
            accept,
            tests::{pipes, Pipe},
        };
        use std::sync::mpsc;

        #[derive(Debug)]
        enum OldEvent {
            Receiving,
            Finished,
        }
        struct AnnounceCompletion(mpsc::Sender<OldEvent>);
        impl Drop for AnnounceCompletion {
            fn drop(&mut self) {
                let _ = self.0.send(OldEvent::Finished);
            }
        }
        struct GatedFailure {
            inner: Pipe,
            armed: Arc<AtomicBool>,
            entered: mpsc::Sender<OldEvent>,
            release: mpsc::Receiver<()>,
        }
        impl Records for GatedFailure {
            fn send(&mut self, bytes: &[u8], deadline: Instant) -> Result<()> {
                self.inner.send(bytes, deadline)
            }
            fn receive(&mut self, deadline: Instant) -> Result<Vec<u8>> {
                if !self.armed.load(Ordering::Acquire) {
                    return self.inner.receive(deadline);
                }
                self.entered.send(OldEvent::Receiving)?;
                // Deliberately hold this old I/O completion until after commit,
                // even if its normal exchange deadline has passed. This models
                // a late transport failure without widening production budgets.
                // The 120s test-only hang guard clears both fixture handshakes.
                let budget = Duration::from_secs(120);
                let started = Instant::now();
                self.release.recv_timeout(budget).map_err(|error| {
                    anyhow::anyhow!(
                        "old receive release failed after {:?} (budget {budget:?}): {error}",
                        started.elapsed()
                    )
                })?;
                anyhow::bail!("deliberate old channel receive failure")
            }
            fn receive_idle(&mut self) -> Result<Vec<u8>> {
                self.inner.receive_idle()
            }
        }
        struct ReleaseOnDrop(Option<mpsc::Sender<()>>);
        impl Drop for ReleaseOnDrop {
            fn drop(&mut self) {
                if let Some(sender) = self.0.take() {
                    let _ = sender.send(());
                }
            }
        }
        let host = SigningKey::from_slice(&[11; 32]).unwrap();
        let device = SigningKey::from_slice(&[12; 32]).unwrap();
        let (private, public) = super::super::generate_noise_key().unwrap();
        let pairing = Pairing {
            device_id: Uuid::new_v4(),
            grant_id: Uuid::new_v4(),
            revocation_epoch: 0,
            host_key: *host.verifying_key(),
            device_key: *device.verifying_key(),
            noise_key: public,
        };
        let migration = Migration::new(
            pairing.clone(),
            pairing.sign_noise_binding(&host).unwrap(),
            device,
        )
        .unwrap();
        let authenticate = |candidate: Candidate, records: AnyRecords, server| {
            std::thread::scope(|scope| {
                let server = scope.spawn(|| {
                    accept(
                        server,
                        &pairing,
                        &private,
                        &host,
                        &[9; 32],
                        false,
                        |_, _| Ok(()),
                    )
                });
                let candidate = candidate.authenticate(records, &[9; 32]).unwrap();
                (candidate, server.join().unwrap().unwrap())
            })
        };
        let (client, server, _) = pipes();
        let armed = Arc::new(AtomicBool::new(false));
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let completion_tx = entered_tx.clone();
        let old_records = GatedFailure {
            inner: client,
            armed: Arc::clone(&armed),
            entered: entered_tx,
            release: release_rx,
        };
        let (old, old_host) = authenticate(
            migration.begin(Transport::Relay).unwrap(),
            Box::new(old_records),
            server,
        );
        migration.commit(old).unwrap();
        armed.store(true, Ordering::Release);
        let (client, server, _) = pipes();
        let (replacement, mut new_host) = authenticate(
            migration.begin(Transport::Relay).unwrap(),
            Box::new(client),
            server,
        );
        let pending = migration
            .prepare("GET", "/api/v1/mobile/device", &[])
            .unwrap();
        std::thread::scope(|scope| {
            // Drop this release before scoped joins on unwind as well as success.
            let mut release = ReleaseOnDrop(Some(release_tx));
            let old_request = scope.spawn(|| {
                // Announce normal early completion and panic before receive, so
                // readiness never waits for the hang guard after producer exit.
                let _completion = AnnounceCompletion(completion_tx);
                migration.exchange(&pending)
            });
            let budget = Duration::from_secs(120);
            let started = Instant::now();
            match entered_rx.recv_timeout(budget).unwrap_or_else(|error| {
                panic!(
                    "old receive never started after {:?} (budget {budget:?}): {error}",
                    started.elapsed()
                )
            }) {
                OldEvent::Receiving => (),
                OldEvent::Finished => {
                    let error = old_request.join().unwrap().err();
                    panic!(
                        "old exchange finished before gated receive after {:?}: {error:?}",
                        started.elapsed()
                    );
                }
            }
            migration.commit(replacement).unwrap();
            assert_eq!(
                migration.status().unwrap().unwrap().state,
                ConnectionState::Online
            );
            release.0.take().unwrap().send(()).unwrap();
            let error = old_request.join().unwrap().err().unwrap();
            assert!(format!("{error:#}").contains("deliberate old channel receive failure"));
            assert_eq!(
                migration.status().unwrap().unwrap().state,
                ConnectionState::Online
            );
            let response_host = scope.spawn(move || {
                let deadline = Instant::now() + Duration::from_secs(30);
                new_host
                    .receive_message(MAX_MOBILE_REQUEST_BYTES + 16 * 1024, deadline)
                    .unwrap();
                new_host
                    .send_message(
                        b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\n\r\n",
                        MAX_MOBILE_RESPONSE_BYTES + 16 * 1024,
                        deadline,
                    )
                    .unwrap();
            });
            let control = migration
                .prepare("GET", "/api/v1/mobile/device", &[])
                .unwrap();
            let response = migration.exchange(&control).unwrap();
            assert_eq!(response.operation_id, control.id());
            assert!(response.bytes.starts_with(b"HTTP/1.1 200"));
            response_host.join().unwrap();
        });
        drop(old_host);
    }
}
