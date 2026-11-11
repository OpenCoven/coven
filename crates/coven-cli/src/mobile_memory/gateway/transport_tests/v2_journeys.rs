//! Integrated v2 application journeys through the production runtime (#1684).
//!
//! Each test starts the daemon's relay runtime with `start_mobile_gateway_for_daemon`
//! against the real `coven-relay` router and serves direct ingress by handing every
//! loopback socket to `serve_tls_connection` with that runtime's own gateway state,
//! so signed HTTPS control, direct WSS and relay workers share one registry and one
//! authenticator. The device side holds only what a paired client holds: its
//! signing keys, the pinned host certificate and key, and a private device
//! directory. No test calls a routing storage owner on the host's behalf.
//!
//! Limits. The gateway's own accept loop is not used: it refuses loopback and port
//! 0, so the per-connection function it calls is driven directly. A relay worker
//! discards its error, so the exact refusal class is asserted on direct ingress
//! only; on the relay profile a refusal is a failed attempt with both sides'
//! durable counters unchanged. Sockets are loopback and keys are synthetic.
use super::*;
use crate::mobile_memory::{
    assurance::{
        authorization_key_id, AssuranceClass, AssuranceProofBinding, CanonicalAssuranceProof,
        NewAuthorizationKey,
    },
    config::atomic_replace_private,
    device::{DeviceAuthority, DeviceGrantPolicy, DeviceLifecycleStatus, DeviceRevocationReason},
    grant::{AssuranceLevel, DeviceGrantRestrictions},
    transport::{
        self,
        cancellation::Cancellation,
        routing::{DeviceState, Transport},
        storage::v2,
        test_support::{assert_bytes_exclude, stable_identifier_markers, Relay},
        websocket, Channel, Records,
    },
};
use rustls::{pki_types::ServerName, ClientConfig, ClientConnection, RootCertStore};
use std::sync::Condvar;

const BOOTSTRAP: &str = "/api/v1/mobile/transport/bootstrap";
const DEVICE: &str = "/api/v1/mobile/device";
const CHALLENGE: &str = "/api/v1/mobile/assurance/challenge";
/// The subject name the fixture gives the host certificate.
const HOST_NAME: &str = "192.0.2.1";
/// Hang guard only, far above anything load can produce. Nothing here asserts a
/// duration; production protocol deadlines are unchanged.
const HANG_GUARD: Duration = Duration::from_secs(120);

fn encode(bytes: impl AsRef<[u8]>) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}
fn other(transport: Transport) -> Transport {
    match transport {
        Transport::Direct => Transport::Relay,
        Transport::Relay => Transport::Direct,
    }
}
fn index(transport: Transport) -> usize {
    usize::from(transport == Transport::Relay)
}

/// A complete HTTP reply or nothing: a truncated reply is never adopted.
#[derive(Debug)]
struct Reply {
    status: u16,
    body: serde_json::Value,
}
impl Reply {
    fn parse(bytes: &[u8]) -> Self {
        let end = bytes
            .windows(4)
            .position(|part| part == b"\r\n\r\n")
            .expect("reply headers are incomplete");
        let head = std::str::from_utf8(&bytes[..end]).unwrap();
        let mut lines = head.split("\r\n");
        let status = lines
            .next()
            .and_then(|line| line.split(' ').nth(1))
            .and_then(|status| status.parse().ok())
            .expect("reply status line is malformed");
        let length: usize = lines
            .filter_map(|line| line.split_once(':'))
            .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
            .and_then(|(_, value)| value.trim().parse().ok())
            .expect("reply length is missing");
        let body = &bytes[end + 4..];
        assert_eq!(body.len(), length, "reply body is incomplete");
        Self {
            status,
            body: serde_json::from_slice(body).unwrap(),
        }
    }
    fn data(&self) -> &serde_json::Value {
        &self.body["data"]
    }
    /// Status with the envelope's error code, empty on success.
    fn outcome(&self) -> (u16, &str) {
        (
            self.status,
            self.body["error"]["code"].as_str().unwrap_or(""),
        )
    }
}

/// One signed mobile request. `step_up` sees the canonical bytes the possession
/// signature covers and returns any assurance headers bound to them.
fn signed(
    id: Uuid,
    key: &SigningKey,
    method: &str,
    path: &str,
    body: &[u8],
    step_up: impl FnOnce(&[u8]) -> Vec<(&'static str, String)>,
) -> Vec<u8> {
    let timestamp = Utc::now().timestamp();
    let nonce = encode(rand::random::<[u8; 32]>());
    let digest = encode(Sha256::digest(body));
    let canonical =
        crate::mobile_memory::auth::canonical_request(method, path, timestamp, &nonce, &digest)
            .unwrap();
    let signature: Signature = key.sign(&canonical);
    let mut head = format!(
        "{method} {path} HTTP/1.1\r\nx-coven-protocol: 1\r\nx-coven-device: {id}\r\nx-coven-timestamp: {timestamp}\r\nx-coven-nonce: {nonce}\r\nx-coven-body-sha256: {digest}\r\nx-coven-signature: {}\r\n",
        encode(signature.to_der().as_bytes())
    );
    for (name, value) in step_up(&canonical) {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str(&format!("content-length: {}\r\n\r\n", body.len()));
    let mut wire = head.into_bytes();
    wire.extend_from_slice(body);
    wire
}

/// Outcome of each direct connection, indexed by accept order.
#[derive(Default)]
struct Verdicts {
    slots: Mutex<Vec<Option<Result<(), String>>>>,
    filled: Condvar,
}

struct Running {
    // Declared first: the relay runtime stops before the shared state is released.
    _runtime: MobileRuntimeHandle,
    state: Arc<MobileGatewayState>,
    address: SocketAddr,
    stop: Arc<AtomicBool>,
    acceptor: Option<JoinHandle<()>>,
    verdicts: Arc<Verdicts>,
    dials: AtomicUsize,
}
impl Drop for Running {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        self.state.stop_network_connections();
        if let Some(acceptor) = self.acceptor.take() {
            let _ = acceptor.join();
        }
    }
}

/// The host installation: private state, the operator's device authority (the
/// owner behind `coven memory mobile device`), a real relay router and, while
/// started, the production runtime.
struct Host {
    home: tempfile::TempDir,
    relay: Relay,
    operator: DeviceAuthority,
    key: p256::ecdsa::VerifyingKey,
    certificate: Vec<u8>,
    client_tls: Arc<ClientConfig>,
    running: Option<Running>,
}
impl Host {
    fn new() -> Self {
        let home = tempfile::tempdir().unwrap();
        let identity = load_or_create_host_identity(home.path(), HOST_NAME).unwrap();
        let key = *SigningKey::from_pkcs8_der(&identity.private_key_der)
            .unwrap()
            .verifying_key();
        let relay = Relay::start_literal();
        crate::mobile_memory::config::save_mobile_config(
            home.path(),
            &MobileGatewayConfig {
                enabled: false,
                bind: "192.168.50.1:7443".parse().unwrap(),
                advertised_endpoint: "https://192.168.50.1:7443".into(),
            },
        )
        .unwrap();
        storage::RelayConfig {
            version: 1,
            enabled: true,
            listener_limit: storage::MAX_LISTENERS,
            endpoints: vec![relay.configuration()],
        }
        .save(home.path())
        .unwrap();
        let mut roots = RootCertStore::empty();
        roots
            .add(CertificateDer::from(identity.certificate_der.clone()))
            .unwrap();
        let client_tls = Arc::new(
            ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
                .with_protocol_versions(&[&rustls::version::TLS13])
                .unwrap()
                .with_root_certificates(roots)
                .with_no_client_auth(),
        );
        Self {
            operator: DeviceAuthority::load(home.path()).unwrap(),
            home,
            relay,
            key,
            certificate: identity.certificate_der,
            client_tls,
            running: None,
        }
    }

    /// Enroll a device as pairing would: a registry record, its initial grant and
    /// a step-up key. No transport material is created.
    fn enroll(&self, seed: u8) -> Phone {
        let key = SigningKey::from_slice(&[seed; 32]).unwrap();
        let step_up = SigningKey::from_slice(&[seed + 1; 32]).unwrap();
        let id = Uuid::new_v4();
        let now = Utc::now();
        let public = encode(key.verifying_key().to_sec1_point(false).as_bytes());
        let record = DeviceRecord {
            id,
            display_name: "Synthetic peer".into(),
            public_key_x963: public.clone(),
            paired_at: now,
            revoked_at: None,
            suspended_at: None,
            scopes: vec![DeviceScope::MemoryRead],
        };
        let grant = DeviceGrant::for_device(id, &public, record.scopes.clone(), now).unwrap();
        DeviceRegistry::load(self.home.path())
            .unwrap()
            .register_with_grant_and_authorization(
                record,
                grant,
                Some(NewAuthorizationKey {
                    public_key_x963: encode(
                        step_up.verifying_key().to_sec1_point(false).as_bytes(),
                    ),
                    assurance_class: AssuranceClass::BiometricOnly,
                    enrolled_at: now,
                }),
            )
            .unwrap();
        Phone {
            home: tempfile::tempdir().unwrap(),
            id,
            key,
            step_up,
        }
    }

    fn start(&mut self) {
        assert!(self.running.is_none(), "runtime is already started");
        let runtime = start_mobile_gateway_for_daemon(self.home.path())
            .unwrap()
            .expect("relay runtime did not start");
        assert!(runtime._direct.is_none() && runtime._relay.is_some());
        let state = active_gateway().unwrap();
        assert_eq!(state.coven_home, self.home.path());
        let identity =
            crate::mobile_memory::identity::load_existing_host_identity(self.home.path()).unwrap();
        let tls =
            tls_server_config(identity.certificate_der, identity.private_key_der.to_vec()).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let verdicts = Arc::new(Verdicts::default());
        let (accept_stop, accept_state, accept_verdicts) =
            (Arc::clone(&stop), Arc::clone(&state), Arc::clone(&verdicts));
        let acceptor = std::thread::spawn(move || {
            let mut workers = Vec::new();
            while !accept_stop.load(Ordering::Acquire) {
                let socket = match listener.accept() {
                    Ok((socket, _)) => socket,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5));
                        continue;
                    }
                    Err(error) => panic!("direct fixture accept failed: {error}"),
                };
                // An accepted socket can inherit the listener's nonblocking mode.
                socket.set_nonblocking(false).unwrap();
                let slot = {
                    let mut slots = accept_verdicts.slots.lock().unwrap();
                    slots.push(None);
                    slots.len() - 1
                };
                let (tls, state, verdicts) = (
                    Arc::clone(&tls),
                    Arc::clone(&accept_state),
                    Arc::clone(&accept_verdicts),
                );
                workers.push(std::thread::spawn(move || {
                    let verdict = serve_tls_connection(socket, tls, &state)
                        .map_err(|error| format!("{error:#}"));
                    verdicts.slots.lock().unwrap()[slot] = Some(verdict);
                    verdicts.filled.notify_all();
                }));
            }
            for worker in workers {
                worker.join().unwrap();
            }
        });
        self.running = Some(Running {
            _runtime: runtime,
            state,
            address,
            stop,
            acceptor: Some(acceptor),
            verdicts,
            dials: AtomicUsize::new(0),
        });
    }

    /// Stop the runtime and release its state, as a daemon exit does.
    fn stop(&mut self) {
        drop(self.running.take().expect("runtime is not started"));
        assert!(
            active_gateway().is_err(),
            "the stopped runtime's state is still reachable"
        );
    }

    fn running(&self) -> &Running {
        self.running.as_ref().expect("runtime is not started")
    }

    /// The next direct connection's position in accept order.
    fn dial(&self) -> usize {
        self.running().dials.fetch_add(1, Ordering::AcqRel)
    }

    /// How `serve_tls_connection` ended for one direct connection.
    fn verdict(&self, slot: usize) -> Result<(), String> {
        let verdicts = &self.running().verdicts;
        let (slots, timeout) = verdicts
            .filled
            .wait_timeout_while(verdicts.slots.lock().unwrap(), HANG_GUARD, |slots| {
                slots.get(slot).is_none_or(Option::is_none)
            })
            .unwrap();
        assert!(
            !timeout.timed_out(),
            "direct connection {slot} never finished"
        );
        slots[slot].clone().unwrap()
    }

    /// Direct ingress reports the production refusal class. A relay worker
    /// discards its error, so the relay profile has nothing to compare.
    fn assert_refusal(&self, slot: Option<usize>, expected: &str) {
        if let Some(slot) = slot {
            assert_eq!(self.verdict(slot), Err(expected.to_owned()));
        }
    }

    /// One signed request over pinned TLS 1.3 to the production connection path.
    fn https(&self, wire: &[u8]) -> Reply {
        self.dial();
        let socket = TcpStream::connect_timeout(&self.running().address, HANG_GUARD).unwrap();
        socket.set_read_timeout(Some(HANG_GUARD)).unwrap();
        socket.set_write_timeout(Some(HANG_GUARD)).unwrap();
        let connection = ClientConnection::new(
            Arc::clone(&self.client_tls),
            ServerName::try_from(HOST_NAME).unwrap(),
        )
        .unwrap();
        let mut tls = StreamOwned::new(connection, socket);
        tls.write_all(wire).unwrap();
        let mut reply = Vec::new();
        // The gateway closes without a TLS close_notify; keep what arrived.
        let _ = tls.read_to_end(&mut reply);
        Reply::parse(&reply)
    }

    /// Join a room as the client with its routing credential: the relay router
    /// for the relay profile, the production TLS path's `/ws` upgrade for direct.
    fn open(
        &self,
        transport: Transport,
        room: &[u8; 32],
        credential: &[u8; 32],
    ) -> (Option<usize>, Result<websocket::WebSocketRecords>) {
        match transport {
            Transport::Relay => (
                None,
                self.relay
                    .connect(websocket::Role::Client, room, credential),
            ),
            Transport::Direct => {
                let slot = self.dial();
                let address = self.running().address;
                let endpoint = websocket::EndpointConfig {
                    url: format!("wss://{HOST_NAME}:{}/ws", address.port()),
                    ca_certificates: vec![encode(&self.certificate)],
                }
                .parse()
                .unwrap();
                (
                    Some(slot),
                    websocket::connect_cancellable(
                        &endpoint,
                        address,
                        websocket::Role::Client,
                        room,
                        credential,
                        Instant::now() + HANG_GUARD,
                        &Cancellation::default(),
                    ),
                )
            }
        }
    }

    fn routing_path(&self, id: Uuid) -> PathBuf {
        self.home.path().join(format!("mobile/transport-{id}.json"))
    }
    /// The host's durable routing file, exactly as stored.
    fn routing_bytes(&self, id: Uuid) -> Option<Vec<u8>> {
        match std::fs::read(self.routing_path(id)) {
            Ok(bytes) => Some(bytes),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => panic!("routing file unavailable: {error}"),
        }
    }
    fn routing(&self, id: Uuid) -> serde_json::Value {
        serde_json::from_slice(&self.routing_bytes(id).expect("routing file is missing")).unwrap()
    }
    /// Committed generation with its direct and relay floors.
    fn committed(&self, id: Uuid) -> (u64, [u64; 2]) {
        let committed = &self.routing(id)["committed"];
        (
            committed["generation"].as_u64().unwrap(),
            [
                committed["floors"][0].as_u64().unwrap(),
                committed["floors"][1].as_u64().unwrap(),
            ],
        )
    }
    fn pending(&self, id: Uuid) -> Option<u64> {
        self.routing(id)["pending"]["generation"].as_u64()
    }

    /// Current grant binding, as the operator's device authority reports it.
    fn grant(&self, id: Uuid) -> (Uuid, u64) {
        let view = self.operator.inspect(&id.to_string(), Utc::now()).unwrap();
        (view.grant_id, view.revocation_epoch)
    }
    /// Operator grant reissue through the device authority, which shares no
    /// registry handle with the runtime.
    fn reissue(&self, id: Uuid, minimum_assurance: AssuranceLevel) {
        let now = Utc::now();
        self.operator
            .reissue_grant(
                &id.to_string(),
                DeviceGrantPolicy {
                    scopes: vec![DeviceScope::MemoryRead],
                    restrictions: DeviceGrantRestrictions::default(),
                    minimum_assurance,
                    expires_at: now + chrono::Duration::hours(1),
                },
                now,
            )
            .unwrap();
    }
    fn suspend(&self, id: Uuid) {
        self.operator.suspend(&id.to_string(), Utc::now()).unwrap();
    }
    fn resume(&self, id: Uuid) {
        self.operator.resume(&id.to_string()).unwrap();
    }
    fn revoke(&self, id: Uuid) {
        self.operator
            .revoke(
                &id.to_string(),
                DeviceRevocationReason::Ordinary,
                Utc::now(),
            )
            .unwrap();
    }
    fn is_revoked(&self, id: Uuid) -> bool {
        let view = self.operator.inspect(&id.to_string(), Utc::now()).unwrap();
        view.status == DeviceLifecycleStatus::Revoked
    }
}

#[derive(Clone, Copy)]
enum Assurance {
    Possession,
    StepUp,
}

/// An authenticated channel and, on direct ingress, its server connection.
struct Live {
    channel: Channel<websocket::WebSocketRecords>,
    slot: Option<usize>,
}
impl Live {
    fn exchange(&mut self, wire: &[u8]) -> Result<Reply> {
        let deadline = Instant::now() + HANG_GUARD;
        self.channel
            .send_message(wire, MAX_MOBILE_REQUEST_BYTES, deadline)?;
        let reply = self
            .channel
            .receive_message(MAX_MOBILE_RESPONSE_BYTES, deadline)?;
        Ok(Reply::parse(&reply))
    }
}

/// The device side: its keys, its pins and a private device directory.
struct Phone {
    home: tempfile::TempDir,
    id: Uuid,
    key: SigningKey,
    step_up: SigningKey,
}
impl Phone {
    fn request(&self, method: &str, path: &str, body: &[u8]) -> Vec<u8> {
        signed(self.id, &self.key, method, path, body, |_| Vec::new())
    }

    fn device_path(&self) -> PathBuf {
        self.home
            .path()
            .join(format!("mobile/device-transport-{}.json", self.id))
    }
    /// The same device with an exact copy of its current routing file: an image
    /// retained from before a later change.
    fn image(&self) -> Phone {
        let image = Phone {
            home: tempfile::tempdir().unwrap(),
            id: self.id,
            key: self.key.clone(),
            step_up: self.step_up.clone(),
        };
        crate::mobile_memory::config::ensure_private_mobile_dir(image.home.path()).unwrap();
        atomic_replace_private(
            &image.device_path(),
            &std::fs::read(self.device_path()).unwrap(),
        )
        .unwrap();
        image
    }
    fn device_bytes(&self) -> Vec<u8> {
        std::fs::read(self.device_path()).unwrap()
    }
    fn state(&self, host: &Host, transport: Transport) -> DeviceState {
        v2::device::begin(
            self.home.path(),
            &host.key,
            self.id,
            self.key.verifying_key(),
            transport,
        )
        .unwrap()
        .state()
    }

    /// Assurance headers for one canonical request. Timestamps are millisecond
    /// RFC 3339. The issue time is rounded up so it cannot precede the challenge,
    /// and this waits for the clock to reach it. The grant binding comes from the
    /// operator's device authority: how a shipped client learns a reissued grant
    /// is outside this fixture.
    fn proof(
        &self,
        host: &Host,
        challenge: &Reply,
        canonical: &[u8],
    ) -> Vec<(&'static str, String)> {
        assert_eq!(challenge.outcome(), (200, ""), "challenge was refused");
        let nonce: [u8; 32] = URL_SAFE_NO_PAD
            .decode(challenge.data()["challenge"].as_str().unwrap())
            .unwrap()
            .try_into()
            .unwrap();
        let expires: DateTime<Utc> = challenge.data()["expiresAt"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap();
        let millisecond = |time: DateTime<Utc>| {
            DateTime::<Utc>::from_timestamp_millis(time.timestamp_millis()).unwrap()
        };
        let issued_at = millisecond(Utc::now()) + chrono::Duration::milliseconds(1);
        while Utc::now() < issued_at {
            std::thread::sleep(Duration::from_millis(1));
        }
        let (grant_id, revocation_epoch) = host.grant(self.id);
        let mut proof = PresentedAssuranceProof {
            context_mode: AssuranceContextMode::Request,
            challenge: nonce,
            issued_at,
            expires_at: millisecond(expires),
            requested_assurance: RequestedAssurance::FreshBiometric,
            signature: String::new(),
        };
        let bytes = CanonicalAssuranceProof::for_request(
            AssuranceProofBinding {
                device_id: self.id,
                grant_id,
                revocation_epoch,
                authorization_key_id: authorization_key_id(&encode(
                    self.step_up.verifying_key().to_sec1_point(false).as_bytes(),
                ))
                .unwrap(),
            },
            &proof,
            canonical,
        )
        .unwrap()
        .canonical_bytes();
        let signature: Signature = self.step_up.sign(&bytes);
        proof.signature = encode(signature.to_der().as_bytes());
        let text = |time: DateTime<Utc>| time.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        vec![
            ("x-coven-assurance-context", "request".into()),
            ("x-coven-assurance-challenge", encode(proof.challenge)),
            ("x-coven-assurance-issued-at", text(proof.issued_at)),
            ("x-coven-assurance-expires-at", text(proof.expires_at)),
            ("x-coven-assurance-level", "fresh_biometric".into()),
            ("x-coven-assurance-signature", proof.signature.clone()),
        ]
    }
    /// A request carrying a fresh step-up proof. `send` carries the possession-
    /// signed challenge request on the same path the request will take.
    fn stepped(
        &self,
        host: &Host,
        method: &str,
        path: &str,
        body: &[u8],
        send: &mut dyn FnMut(&[u8]) -> Reply,
    ) -> Vec<u8> {
        let challenge = send(&self.request("POST", CHALLENGE, &[]));
        signed(self.id, &self.key, method, path, body, |canonical| {
            self.proof(host, &challenge, canonical)
        })
    }

    /// One signed routing-control request over pinned HTTPS.
    fn control(&self, host: &Host, body: serde_json::Value, assurance: Assurance) -> Reply {
        let body = serde_json::to_vec(&body).unwrap();
        let wire = match assurance {
            Assurance::Possession => self.request("POST", BOOTSTRAP, &body),
            Assurance::StepUp => {
                self.stepped(host, "POST", BOOTSTRAP, &body, &mut |wire| host.https(wire))
            }
        };
        host.https(&wire)
    }
    fn prepare(&self, host: &Host, assurance: Assurance) -> v2::Handover {
        let reply = self.control(
            host,
            serde_json::json!({"version":2,"operation":"prepare"}),
            assurance,
        );
        assert_eq!(reply.outcome(), (200, ""), "signed prepare was refused");
        serde_json::from_value(reply.data().clone()).unwrap()
    }
    /// Durable device adoption of a handover received over pinned HTTPS.
    fn adopt(&self, host: &Host, handover: v2::Handover) -> v2::Acknowledgment {
        v2::device::adopt_direct(
            self.home.path(),
            handover,
            &host.key,
            self.id,
            self.key.verifying_key(),
        )
        .unwrap()
    }
    fn acknowledge(&self, host: &Host, ack: &v2::Acknowledgment, assurance: Assurance) -> Reply {
        self.control(
            host,
            serde_json::json!({"version":2,"operation":"acknowledge","generation":ack.generation,"seedDigest":ack.seed_digest}),
            assurance,
        )
    }
    /// The whole authenticated handover: signed prepare, durable adoption, signed
    /// acknowledgment. Returns the pairing the device verified against its pins.
    fn handover(&self, host: &Host, assurance: Assurance) -> (u64, Pairing) {
        let handover = self.prepare(host, assurance);
        let generation = handover.generation;
        let pairing = handover
            .verify(&host.key, self.id, self.key.verifying_key())
            .unwrap();
        let ack = self.adopt(host, handover);
        let reply = self.acknowledge(host, &ack, assurance);
        assert_eq!(
            reply.outcome(),
            (200, ""),
            "signed acknowledgment was refused"
        );
        assert_eq!(reply.data()["committed"], true);
        (generation, pairing)
    }

    /// One connection attempt on the device's next counter room.
    fn attempt(
        &self,
        host: &Host,
        transport: Transport,
    ) -> (Option<usize>, Result<Channel<websocket::WebSocketRecords>>) {
        let mut attempt = v2::device::begin(
            self.home.path(),
            &host.key,
            self.id,
            self.key.verifying_key(),
            transport,
        )
        .unwrap();
        let (room, credential) = attempt.rendezvous().unwrap();
        let (slot, records) = host.open(transport, &room, &credential);
        (
            slot,
            records.and_then(|records| attempt.connect(records, &self.key)),
        )
    }
    /// Connect on the next counter room. A relay room is ready once a daemon
    /// worker has joined it; an attempt that arrives first is closed before any
    /// host proof, so it may be repeated only while the device's durable state
    /// is unchanged. Direct ingress has no such wait and must admit at once.
    fn connect(&self, host: &Host, transport: Transport) -> Live {
        let started = Instant::now();
        loop {
            let before = self.state(host, transport);
            let (slot, result) = self.attempt(host, transport);
            match result {
                Ok(channel) => {
                    let after = self.state(host, transport);
                    assert_eq!(
                        (after.generation, after.next),
                        (before.generation, before.next + 1),
                        "admission did not consume exactly one device room"
                    );
                    return Live { channel, slot };
                }
                Err(error) => {
                    assert!(
                        transport == Transport::Relay,
                        "direct ingress refused current material: {error:#}"
                    );
                    assert_eq!(
                        self.state(host, transport),
                        before,
                        "relay attempt failed after durable device consumption: {error:#}"
                    );
                    assert!(
                        started.elapsed() < HANG_GUARD,
                        "relay room never became ready: {error:#}"
                    );
                    std::thread::sleep(Duration::from_millis(50));
                }
            }
        }
    }
    /// One attempt that must be refused before either side consumes a room.
    fn refused(&self, host: &Host, transport: Transport, direct_error: &str) {
        let device = self.device_bytes();
        let routing = host.routing_bytes(self.id);
        let (slot, result) = self.attempt(host, transport);
        assert!(
            result.is_err(),
            "{transport:?}: refused material was admitted"
        );
        host.assert_refusal(slot, direct_error);
        assert_eq!(
            self.device_bytes(),
            device,
            "a refused attempt consumed a device room"
        );
        assert_eq!(
            host.routing_bytes(self.id),
            routing,
            "a refused attempt changed host routing state"
        );
    }

    /// Everything the device's routing file holds except its signing key: the
    /// next room, its credential and the pinned Noise key. The impostor completes
    /// the Noise handshake, receives the host's proof and answers with a
    /// signature from another key. Returns once the host has released its proof,
    /// so an early relay close before a worker joined is not mistaken for the
    /// possession refusal.
    fn impostor(&self, host: &Host, transport: Transport) -> Option<usize> {
        let attempt = v2::device::begin(
            self.home.path(),
            &host.key,
            self.id,
            self.key.verifying_key(),
            transport,
        )
        .unwrap();
        let (room, credential) = attempt.rendezvous().unwrap();
        let noise = attempt.pairing().noise_key;
        drop(attempt);
        let mut prologue = Vec::new();
        ciborium::ser::into_writer(
            &ciborium::value::Value::Array(vec![
                ciborium::value::Value::Text("coven.mobile.connection".into()),
                ciborium::value::Value::Integer(1.into()),
                ciborium::value::Value::Text(transport::PROFILE.into()),
                ciborium::value::Value::Text(
                    if transport == Transport::Direct {
                        "direct"
                    } else {
                        "relay"
                    }
                    .into(),
                ),
                ciborium::value::Value::Bytes(room.to_vec()),
            ]),
            &mut prologue,
        )
        .unwrap();
        let forger = SigningKey::from_slice(&[99; 32]).unwrap();
        let started = Instant::now();
        loop {
            let (slot, records) = host.open(transport, &room, &credential);
            let reached_proof = records.and_then(|mut records| -> Result<()> {
                let deadline = Instant::now() + HANG_GUARD;
                let mut handshake = snow::Builder::new(transport::PROFILE.parse()?)
                    .prologue(&prologue)?
                    .remote_public_key(&noise)?
                    .build_initiator()?;
                let mut buffer = [0_u8; 256];
                let length = handshake.write_message(&[], &mut buffer)?;
                records.send(&buffer[..length], deadline)?;
                handshake.read_message(&records.receive(deadline)?, &mut buffer)?;
                let mut cipher = handshake.into_transport_mode()?;
                let length = cipher.read_message(&records.receive(deadline)?, &mut buffer)?;
                anyhow::ensure!(length == 32, "host challenge is malformed");
                let length = cipher.write_message(&[7; 32], &mut buffer)?;
                records.send(&buffer[..length], deadline)?;
                let length = cipher.read_message(&records.receive(deadline)?, &mut buffer)?;
                anyhow::ensure!(length == 24 + 64, "host proof is malformed");
                // The host has now signed its proof. Only a device signature
                // over the same transcript can complete admission.
                let forged: Signature = forger.sign(b"routing credentials are not a device key");
                let length = cipher.write_message(&forged.to_bytes(), &mut buffer)?;
                records.send(&buffer[..length], deadline)?;
                let completion = records
                    .receive(deadline)
                    .ok()
                    .and_then(|record| cipher.read_message(&record, &mut buffer).ok());
                assert!(
                    completion.is_none(),
                    "{transport:?}: the host completed admission without a device signature"
                );
                Ok(())
            });
            match reached_proof {
                Ok(()) => return slot,
                Err(error) => {
                    assert!(
                        transport == Transport::Relay,
                        "direct ingress closed before its possession proof: {error:#}"
                    );
                    assert!(
                        started.elapsed() < HANG_GUARD,
                        "relay room never became ready for the impostor: {error:#}"
                    );
                    std::thread::sleep(Duration::from_millis(50));
                }
            }
        }
    }
}

/// A possession-signed read that must be served.
fn read_device(phone: &Phone, live: &mut Live) {
    assert_eq!(
        live.exchange(&phone.request("GET", DEVICE, &[]))
            .unwrap()
            .outcome(),
        (200, "")
    );
}
/// Close the current channel and connect on the next room. On the relay profile
/// the daemon rebuilds a pairing's window only in a manager pass that starts
/// after its established channel has gone, so a successful reconnection proves a
/// full pass over durable state since the previous one. A refusal repeated after
/// it is therefore not a listener that had yet to start.
fn reconnect(phone: &Phone, host: &Host, transport: Transport, live: Live) -> Live {
    drop(live);
    let mut live = phone.connect(host, transport);
    read_device(phone, &mut live);
    live
}
/// One continuous journey on one profile, from an enrolled pairing with no
/// transport material to revocation.
fn journey(transport: Transport) {
    let _guard = super::super::tests::TEST_GATEWAY_LOCK.lock().unwrap();
    let mut host = Host::new();
    let phone = host.enroll(42);
    let bystander = host.enroll(52);
    let identity_path = host.home.path().join("mobile/host-identity.json");
    let identity_before = std::fs::read(&identity_path).unwrap();
    host.start();

    // Provisioning and authenticated handover. The signed reply is the only
    // source of routing material, and nothing listens before the acknowledgment.
    assert!(host.routing_bytes(phone.id).is_none());
    let handover = phone.prepare(&host, Assurance::Possession);
    assert_eq!((handover.version, handover.generation), (2, 0));
    assert!(
        handover.relay_endpoints == vec![host.relay.configuration()],
        "the authenticated handover did not carry the operator's relay endpoint"
    );
    let first = handover
        .verify(&host.key, phone.id, phone.key.verifying_key())
        .unwrap();
    assert_eq!(host.pending(phone.id), Some(0));
    assert!(host.routing(phone.id)["committed"].is_null());
    let ack = phone.adopt(&host, handover);
    phone.refused(&host, transport, "direct pairing unavailable");
    assert_eq!(
        phone
            .acknowledge(&host, &ack, Assurance::Possession)
            .outcome(),
        (200, "")
    );
    assert_eq!(host.committed(phone.id), (0, [0, 0]));
    assert_eq!(host.pending(phone.id), None);

    // Counter-window connect and a signed application request.
    let mut live = phone.connect(&host, transport);
    let read = phone.request("GET", DEVICE, &[]);
    let reply = live.exchange(&read).unwrap();
    assert_eq!(reply.outcome(), (200, ""));
    assert_eq!(reply.data()["id"], phone.id.to_string());
    let mut floors = [0, 0];
    floors[index(transport)] = 1;
    assert_eq!(host.committed(phone.id), (0, floors));

    // Nonce: the exact signed bytes are refused once spent; the channel stays.
    assert_eq!(
        live.exchange(&read).unwrap().outcome(),
        (409, "request_replayed")
    );
    // Signature: a request naming the channel's device but signed by another
    // key is refused, on a channel that has already proved possession.
    let forger = SigningKey::from_slice(&[99; 32]).unwrap();
    let forged = |method| signed(phone.id, &forger, method, DEVICE, &[], |_| Vec::new());
    assert_eq!(
        live.exchange(&forged("GET")).unwrap().outcome(),
        (401, "signature_invalid")
    );
    // The rejected signature did not close the channel.
    read_device(&phone, &mut live);
    // Channel-device equality: another enrolled device's valid request on this
    // channel closes it undispatched. The same bytes are still unspent on that
    // device's own authenticated path, so the verifier never saw them here.
    let foreign = bystander.request("GET", DEVICE, &[]);
    assert!(
        live.exchange(&foreign).is_err(),
        "a request for another device was answered on this channel"
    );
    host.assert_refusal(live.slot, "request identity differs from channel identity");
    let reply = host.https(&foreign);
    assert_eq!(reply.outcome(), (200, ""));
    assert_eq!(reply.data()["id"], bystander.id.to_string());

    // Routing credentials alone: the room, its credential and the pinned Noise
    // key reach the host's proof and no further. Nothing is consumed.
    let routing = host.routing_bytes(phone.id);
    let device = phone.device_bytes();
    let slot = phone.impostor(&host, transport);
    host.assert_refusal(slot, "invalid possession proof");
    assert_eq!(host.routing_bytes(phone.id), routing);
    assert_eq!(phone.device_bytes(), device);

    // A forged deletion has no effect. Any deletion request ends its channel
    // after the reply, refused or not, so the device walks on to the next room.
    let mut live = phone.connect(&host, transport);
    assert_eq!(
        live.exchange(&forged("DELETE")).unwrap().outcome(),
        (401, "signature_invalid")
    );
    assert!(!host.is_revoked(phone.id), "a forged deletion took effect");
    assert!(live.exchange(&phone.request("GET", DEVICE, &[])).is_err());
    if let Some(slot) = live.slot {
        assert_eq!(host.verdict(slot), Ok(()));
    }
    let mut live = phone.connect(&host, transport);
    read_device(&phone, &mut live);
    floors[index(transport)] = 3;
    assert_eq!(host.committed(phone.id), (0, floors));

    // Grant rotation and tightening, by the operator. The live channel closes at
    // its next request, generation 0 is no longer admitted, and a possession-only
    // request cannot start the next handover.
    let generation_zero = phone.image();
    host.reissue(phone.id, AssuranceLevel::FreshBiometric);
    assert!(
        live.exchange(&phone.request("GET", DEVICE, &[])).is_err(),
        "a channel admitted under the previous grant answered a request"
    );
    host.assert_refusal(live.slot, "grant changed");
    phone.refused(&host, transport, "direct pairing unavailable");
    let routing = host.routing_bytes(phone.id);
    assert_eq!(
        phone
            .control(
                &host,
                serde_json::json!({"version":2,"operation":"prepare"}),
                Assurance::Possession
            )
            .outcome(),
        (403, "assurance_required")
    );
    assert_eq!(host.routing_bytes(phone.id), routing);
    let (generation, second) = phone.handover(&host, Assurance::StepUp);
    assert_eq!(generation, 1);
    assert_eq!(
        (second.grant_id, second.revocation_epoch),
        host.grant(phone.id)
    );
    assert_ne!(second.noise_key, first.noise_key);
    assert_eq!(host.committed(phone.id), (1, [0, 0]));

    // Assurance on the new generation: possession alone is refused per request,
    // an exact proof is accepted once, and it cannot authorize a second request.
    let before_connect = phone.image();
    let mut live = phone.connect(&host, transport);
    assert_eq!(
        live.exchange(&phone.request("GET", DEVICE, &[]))
            .unwrap()
            .outcome(),
        (403, "assurance_required")
    );
    let challenge = live
        .exchange(&phone.request("POST", CHALLENGE, &[]))
        .unwrap();
    let mut headers = Vec::new();
    let proven = signed(phone.id, &phone.key, "GET", DEVICE, &[], |canonical| {
        headers = phone.proof(&host, &challenge, canonical);
        headers.clone()
    });
    assert_eq!(live.exchange(&proven).unwrap().outcome(), (200, ""));
    let reused = signed(phone.id, &phone.key, "GET", DEVICE, &[], |_| headers);
    assert_eq!(
        live.exchange(&reused).unwrap().outcome(),
        (403, "assurance_required")
    );
    floors = [0, 0];
    floors[index(transport)] = 1;
    assert_eq!(host.committed(phone.id), (1, floors));
    generation_zero.refused(&host, transport, "direct pairing unavailable");

    // Daemon restart. The rebuilt runtime admits the next room from durable
    // state and still refuses the room consumed before the restart.
    host.stop();
    assert!(
        live.exchange(&phone.request("GET", DEVICE, &[])).is_err(),
        "a channel outlived its runtime"
    );
    assert_eq!(host.committed(phone.id), (1, floors));
    host.start();
    let mut live = phone.connect(&host, transport);
    let wire = phone.stepped(&host, "GET", DEVICE, &[], &mut |wire| {
        live.exchange(wire).unwrap()
    });
    assert_eq!(live.exchange(&wire).unwrap().outcome(), (200, ""));
    floors[index(transport)] = 2;
    assert_eq!(host.committed(phone.id), (1, floors));
    before_connect.refused(&host, transport, "direct pairing unavailable");
    assert_eq!(std::fs::read(&identity_path).unwrap(), identity_before);

    // Suspension, by the operator, leaves the grant and its epoch unchanged. The
    // live channel still closes at its next request and nothing is admitted or
    // moved while it lasts; after a resume the same generation is served again.
    let routing = host.routing_bytes(phone.id);
    let grant = host.grant(phone.id);
    host.suspend(phone.id);
    assert_eq!(host.grant(phone.id), grant);
    assert!(
        live.exchange(&phone.request("GET", DEVICE, &[])).is_err(),
        "a suspended device's channel answered a request"
    );
    host.assert_refusal(live.slot, "device unavailable");
    phone.refused(&host, transport, "direct pairing unavailable");
    assert_eq!(
        phone
            .control(
                &host,
                serde_json::json!({"version":2,"operation":"prepare"}),
                Assurance::Possession
            )
            .outcome(),
        (403, "device_suspended")
    );
    assert_eq!(host.routing_bytes(phone.id), routing);
    host.resume(phone.id);
    let mut live = phone.connect(&host, transport);
    let wire = phone.stepped(&host, "GET", DEVICE, &[], &mut |wire| {
        live.exchange(wire).unwrap()
    });
    assert_eq!(live.exchange(&wire).unwrap().outcome(), (200, ""));
    floors[index(transport)] = 3;
    assert_eq!(host.committed(phone.id), (1, floors));

    // Revocation, by the operator. The live channel closes at its next request,
    // reconnection and signed control are refused, and no routing state moves.
    let routing = host.routing_bytes(phone.id);
    host.revoke(phone.id);
    assert!(
        live.exchange(&phone.request("GET", DEVICE, &[])).is_err(),
        "a revoked device's channel answered a request"
    );
    host.assert_refusal(live.slot, "device unavailable");
    phone.refused(&host, transport, "direct pairing unavailable");
    for operation in ["prepare", "recover"] {
        assert_eq!(
            phone
                .control(
                    &host,
                    serde_json::json!({"version":2,"operation":operation}),
                    Assurance::Possession
                )
                .outcome(),
            (403, "device_revoked")
        );
    }
    assert_eq!(host.routing_bytes(phone.id), routing);
    assert_eq!(
        host.https(&bystander.request("GET", DEVICE, &[])).outcome(),
        (200, ""),
        "revoking one device disturbed another"
    );
    // The other profile's counters never moved.
    assert_eq!(host.committed(phone.id).1[index(other(transport))], 0);

    // The relay saw rooms and ciphertext only.
    let mut markers = stable_identifier_markers(&first);
    markers.extend(stable_identifier_markers(&second));
    markers.push(("request path", DEVICE.as_bytes().to_vec()));
    markers.push(("request header", b"x-coven-device".to_vec()));
    markers.push(("completion plaintext", b"host-finished-v2".to_vec()));
    let captured = host.relay.captured();
    assert!(captured.windows(5).any(|part| part == b"room="));
    assert_bytes_exclude(&captured, &markers);
    println!("v2-journey transport={transport:?} verified");
}

#[test]
fn integrated_v2_journey_over_direct_ingress() {
    journey(Transport::Direct);
}

#[test]
fn integrated_v2_journey_over_the_relay() {
    journey(Transport::Relay);
}

/// Signed control that must be refused without moving any routing state.
fn control_refused(phone: &Phone, host: &Host, body: serde_json::Value) {
    let routing = host.routing_bytes(phone.id);
    let reply = phone.control(host, body.clone(), Assurance::Possession);
    assert_ne!(reply.status, 200, "{body} was accepted");
    assert!(
        !reply.body.to_string().contains("routingSeed"),
        "{body}: a refusal released routing material"
    );
    assert_eq!(
        host.routing_bytes(phone.id),
        routing,
        "{body}: a refused control request changed routing state"
    );
}

/// A committed generation that an ordinary handover supersedes. The retained
/// generation-0 device image and the generation-0 acknowledgment are authentic;
/// only their generation is stale.
fn stale_generation(transport: Transport) {
    let _guard = super::super::tests::TEST_GATEWAY_LOCK.lock().unwrap();
    let mut host = Host::new();
    let phone = host.enroll(42);
    host.start();
    let first = phone.adopt(&host, phone.prepare(&host, Assurance::Possession));
    assert_eq!(
        phone
            .acknowledge(&host, &first, Assurance::Possession)
            .outcome(),
        (200, "")
    );
    let mut live = phone.connect(&host, transport);
    read_device(&phone, &mut live);
    let superseded = phone.image();

    // While generation 1 is pending, neither generation is admitted.
    let handover = phone.prepare(&host, Assurance::Possession);
    assert_eq!(handover.generation, 1);
    assert_eq!(host.pending(phone.id), Some(1));
    assert!(
        live.exchange(&phone.request("GET", DEVICE, &[])).is_err(),
        "a generation-0 channel answered while generation 1 was pending"
    );
    host.assert_refusal(live.slot, "routing handover pending");
    superseded.refused(&host, transport, "direct pairing unavailable");
    let second = phone.adopt(&host, handover);
    phone.refused(&host, transport, "direct pairing unavailable");
    assert_eq!(
        phone
            .acknowledge(&host, &second, Assurance::Possession)
            .outcome(),
        (200, "")
    );
    assert_eq!(host.committed(phone.id), (1, [0, 0]));

    // Generation 1 is live; generation 0 stays refused across a rebuilt window,
    // and signed control naming it cannot bring it back.
    let mut live = phone.connect(&host, transport);
    read_device(&phone, &mut live);
    superseded.refused(&host, transport, "direct pairing unavailable");
    let live = reconnect(&phone, &host, transport, live);
    superseded.refused(&host, transport, "direct pairing unavailable");
    for body in [
        serde_json::json!({"version":2,"operation":"acknowledge","generation":first.generation,"seedDigest":first.seed_digest}),
        serde_json::json!({"version":2,"operation":"resync","transport":if transport == Transport::Direct { "direct" } else { "relay" },"generation":0,"next":0}),
        serde_json::json!({"version":2,"operation":"recover","knownGeneration":0}),
    ] {
        control_refused(&phone, &host, body);
    }
    let mut floors = [0, 0];
    floors[index(transport)] = 2;
    assert_eq!(host.committed(phone.id), (1, floors));
    let mut live = live;
    read_device(&phone, &mut live);
    println!("v2-stale-generation transport={transport:?} verified");
}

#[test]
fn stale_generation_is_refused_on_both_profiles() {
    for transport in [Transport::Direct, Transport::Relay] {
        stale_generation(transport);
    }
}

/// A pending generation the device received and adopted, burned by a grant
/// reissue before its acknowledgment arrived.
fn burned_pending_generation(transport: Transport) {
    let _guard = super::super::tests::TEST_GATEWAY_LOCK.lock().unwrap();
    let mut host = Host::new();
    let phone = host.enroll(42);
    host.start();
    phone.handover(&host, Assurance::Possession);
    let mut live = phone.connect(&host, transport);
    read_device(&phone, &mut live);

    let offered = phone.prepare(&host, Assurance::Possession);
    assert_eq!(offered.generation, 1);
    let offered_seed = offered.routing_seed;
    let late = phone.adopt(&host, offered);
    let burned = phone.image();
    host.reissue(phone.id, AssuranceLevel::Possession);

    // The delayed acknowledgment commits nothing and the offered generation
    // admits nothing. The channel from before the reissue is closed.
    let acknowledge = |ack: &v2::Acknowledgment| serde_json::json!({"version":2,"operation":"acknowledge","generation":ack.generation,"seedDigest":ack.seed_digest});
    control_refused(&phone, &host, acknowledge(&late));
    assert_eq!(host.pending(phone.id), Some(1));
    phone.refused(&host, transport, "direct pairing unavailable");
    assert!(
        live.exchange(&phone.request("GET", DEVICE, &[])).is_err(),
        "a channel from before the reissue answered a request"
    );
    host.assert_refusal(live.slot, "grant changed");

    // The next prepare burns generation 1 and offers generation 2 with fresh
    // material. The device may skip the burned value on this higher handover.
    let replacement = phone.prepare(&host, Assurance::Possession);
    assert_eq!(replacement.generation, 2);
    assert_ne!(replacement.routing_seed, offered_seed);
    assert_eq!(
        (
            replacement.grant_id.parse::<Uuid>().unwrap(),
            replacement.revocation_epoch
        ),
        host.grant(phone.id)
    );
    let routing = host.routing(phone.id);
    assert_eq!(routing["minimumGeneration"].as_u64(), Some(1));
    assert!(routing["committed"].is_null());
    control_refused(&phone, &host, acknowledge(&late));
    let current = phone.adopt(&host, replacement);
    assert_eq!(
        phone
            .acknowledge(&host, &current, Assurance::Possession)
            .outcome(),
        (200, "")
    );
    assert_eq!(host.committed(phone.id), (2, [0, 0]));

    // Generation 2 is live; the burned image stays refused across a rebuilt
    // window, and signed control naming generation 1 cannot revive it.
    let mut live = phone.connect(&host, transport);
    read_device(&phone, &mut live);
    burned.refused(&host, transport, "direct pairing unavailable");
    let mut live = reconnect(&phone, &host, transport, live);
    burned.refused(&host, transport, "direct pairing unavailable");
    for body in [
        acknowledge(&late),
        serde_json::json!({"version":2,"operation":"resync","transport":if transport == Transport::Direct { "direct" } else { "relay" },"generation":1,"next":0}),
        serde_json::json!({"version":2,"operation":"recover","knownGeneration":1}),
    ] {
        control_refused(&phone, &host, body);
    }
    let mut floors = [0, 0];
    floors[index(transport)] = 2;
    assert_eq!(host.committed(phone.id), (2, floors));
    read_device(&phone, &mut live);
    println!("v2-burned-generation transport={transport:?} verified");
}

#[test]
fn burned_pending_generation_is_refused_on_both_profiles() {
    for transport in [Transport::Direct, Transport::Relay] {
        burned_pending_generation(transport);
    }
}

/// A paired v1 client with live channels on both ingress paths, upgraded by its
/// own signed handover. Its v1 room, credential and binding are authentic.
#[test]
fn v1_material_is_refused_after_the_v2_floor_on_both_ingress_paths() {
    let _guard = super::super::tests::TEST_GATEWAY_LOCK.lock().unwrap();
    let mut host = Host::new();
    let phone = host.enroll(42);
    host.start();
    let reply = host.https(&phone.request("POST", BOOTSTRAP, &[]));
    assert_eq!(reply.outcome(), (200, ""));
    let provisioning: storage::Provisioning = serde_json::from_value(reply.data().clone()).unwrap();
    let pairing = provisioning
        .verify(&host.key, phone.id, phone.key.verifying_key())
        .unwrap();
    let legacy = |transport: Transport| {
        let direct = transport == Transport::Direct;
        let slot = u64::try_from(Utc::now().timestamp()).unwrap() / storage::SLOT_SECONDS;
        let (room, credential) = provisioning.rendezvous(slot, direct).unwrap();
        let (server, records) = host.open(transport, &room, &credential);
        (
            server,
            records.and_then(|records| {
                transport::connect(
                    records,
                    &pairing,
                    &provisioning.binding_signature,
                    &phone.key,
                    &room,
                    direct,
                )
            }),
        )
    };
    // Positive control: before the upgrade v1 is served on both ingress paths.
    let mut channels = Vec::new();
    for transport in [Transport::Direct, Transport::Relay] {
        let started = Instant::now();
        let mut live = loop {
            let (slot, result) = legacy(transport);
            match result {
                Ok(channel) => break Live { channel, slot },
                Err(error) => {
                    // A v1 relay room is ready once the daemon's worker joins it.
                    assert!(
                        transport == Transport::Relay && started.elapsed() < HANG_GUARD,
                        "{transport:?}: v1 was not served before the upgrade: {error:#}"
                    );
                    std::thread::sleep(Duration::from_millis(50));
                }
            }
        };
        read_device(&phone, &mut live);
        channels.push(live);
    }

    // The signed prepare establishes the protocol floor. Both live v1 channels
    // close at their next request, before the handover is even acknowledged.
    let handover = phone.prepare(&host, Assurance::Possession);
    assert_eq!(handover.generation, 0);
    for live in &mut channels {
        assert!(
            live.exchange(&phone.request("GET", DEVICE, &[])).is_err(),
            "a v1 channel answered a request after the v2 floor"
        );
        host.assert_refusal(live.slot, "v2 routing required; legacy channel refused");
    }
    let ack = phone.adopt(&host, handover);
    assert_eq!(
        phone
            .acknowledge(&host, &ack, Assurance::Possession)
            .outcome(),
        (200, "")
    );

    // v2 is served on both profiles; the v1 room and credential are refused on
    // both ingress paths across a rebuilt relay window, and a legacy bootstrap
    // neither reopens v1 nor releases material.
    let mut live: Vec<_> = [Transport::Direct, Transport::Relay]
        .into_iter()
        .map(|transport| {
            let mut live = phone.connect(&host, transport);
            read_device(&phone, &mut live);
            live
        })
        .collect();
    let refuse_v1 = || {
        let routing = host.routing_bytes(phone.id);
        for transport in [Transport::Direct, Transport::Relay] {
            let (slot, result) = legacy(transport);
            assert!(
                result.is_err(),
                "{transport:?}: v1 material was admitted after the v2 floor"
            );
            host.assert_refusal(slot, "direct pairing unavailable");
        }
        let reply = host.https(&phone.request("POST", BOOTSTRAP, &[]));
        assert_ne!(reply.status, 200, "a legacy bootstrap was served");
        assert!(!reply.body.to_string().contains("routingSeed"));
        assert_eq!(host.routing_bytes(phone.id), routing);
    };
    refuse_v1();
    let relay = live.pop().unwrap();
    live.push(reconnect(&phone, &host, Transport::Relay, relay));
    refuse_v1();
    // Only the v2 positive controls moved a counter.
    assert_eq!(host.committed(phone.id), (0, [1, 2]));
    println!("v2-floor-refuses-v1 verified");
}
