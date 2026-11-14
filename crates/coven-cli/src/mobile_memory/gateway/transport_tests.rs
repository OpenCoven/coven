use super::super::{
    grant::{DeviceGrant, DeviceScope},
    registry::DeviceRecord,
    transport::{storage, Pairing},
};
use super::*;
use p256::{
    ecdsa::{signature::Signer, Signature, SigningKey},
    pkcs8::DecodePrivateKey,
};
use sha2::{Digest, Sha256};
use std::time::Instant;

pub(super) struct Fixture {
    _home: tempfile::TempDir,
    pub(super) state: MobileGatewayState,
    pub(super) device: SigningKey,
    pub(super) pairing: Pairing,
}
impl Fixture {
    pub(super) fn new() -> Self {
        let home = tempfile::tempdir().unwrap();
        Self::with_home(home)
    }
    fn with_home(home: tempfile::TempDir) -> Self {
        Self::configured(home, false)
    }
    fn with_authorization() -> Self {
        Self::configured(tempfile::tempdir().unwrap(), true)
    }
    fn configured(home: tempfile::TempDir, with_authorization: bool) -> Self {
        Self::configured_after(home, with_authorization, 0)
    }
    fn configured_after(
        home: tempfile::TempDir,
        with_authorization: bool,
        preceding: usize,
    ) -> Self {
        let registry = Arc::new(DeviceRegistry::load(home.path()).unwrap());
        let identity = load_or_create_host_identity(home.path(), "192.0.2.1").unwrap();
        let host = SigningKey::from_pkcs8_der(&identity.private_key_der).unwrap();
        let device = SigningKey::from_slice(&[42; 32]).unwrap();
        let id = Uuid::new_v4();
        let now = Utc::now();
        let public_key =
            URL_SAFE_NO_PAD.encode(device.verifying_key().to_sec1_point(false).as_bytes());
        let record = DeviceRecord {
            id,
            display_name: "Synthetic peer".into(),
            public_key_x963: public_key.clone(),
            paired_at: now,
            revoked_at: None,
            suspended_at: None,
            scopes: vec![DeviceScope::MemoryRead],
        };
        let grant = DeviceGrant::for_device(id, &public_key, record.scopes.clone(), now).unwrap();
        let authorization =
            with_authorization.then(|| super::super::assurance::NewAuthorizationKey {
                public_key_x963: URL_SAFE_NO_PAD.encode(
                    SigningKey::from_slice(&[43; 32])
                        .unwrap()
                        .verifying_key()
                        .to_sec1_point(false)
                        .as_bytes(),
                ),
                assurance_class: super::super::assurance::AssuranceClass::BiometricOnly,
                enrolled_at: now,
            });
        for index in 0..preceding {
            let mut other = record.clone();
            other.id = Uuid::new_v4();
            let other_key =
                SigningKey::from_slice(&[u8::try_from(index + 100).unwrap(); 32]).unwrap();
            other.public_key_x963 =
                URL_SAFE_NO_PAD.encode(other_key.verifying_key().to_sec1_point(false).as_bytes());
            let other_grant = DeviceGrant::for_device(
                other.id,
                &other.public_key_x963,
                other.scopes.clone(),
                now,
            )
            .unwrap();
            registry
                .register_with_grant_and_authorization(other, other_grant, None)
                .unwrap();
        }
        registry
            .register_with_grant_and_authorization(record, grant, authorization)
            .unwrap();
        let pairing = storage::load_or_provision(home.path(), &registry, &host, id)
            .unwrap()
            .pairing;
        let state = MobileGatewayState {
            storage_runtime: super::super::store_lock::Runtime::current(),
            connections: Default::default(),
            network: Default::default(),
            coven_home: home.path().to_path_buf(),
            host_fingerprint: identity.public_key_fingerprint,
            pairing: PairingManager::new(Arc::clone(&registry)),
            authenticator: MobileAuthenticator::with_assurance(
                Arc::clone(&registry),
                AssuranceChallengeStore::load(home.path()).unwrap(),
            ),
            registry,
            advertised_endpoint: Some("https://192.0.2.1:7443".into()),
            introductions: super::super::enrollment::IntroductionLedger::new(
                home.path(),
                identity.public_key_fingerprint,
            ),
        };
        Self {
            _home: home,
            state,
            device,
            pairing,
        }
    }
    fn request(&self, method: &str, path: &str, id: Uuid) -> Vec<u8> {
        self.request_body(method, path, id, &[])
    }
    fn request_body(&self, method: &str, path: &str, id: Uuid, body: &[u8]) -> Vec<u8> {
        use ring::rand::SecureRandom;
        let timestamp = Utc::now().timestamp();
        let mut request_nonce = [0_u8; 32];
        ring::rand::SystemRandom::new()
            .fill(&mut request_nonce)
            .unwrap();
        let nonce = URL_SAFE_NO_PAD.encode(request_nonce);
        let digest = URL_SAFE_NO_PAD.encode(Sha256::digest(body));
        let canonical =
            crate::mobile_memory::auth::canonical_request(method, path, timestamp, &nonce, &digest)
                .unwrap();
        let signature: Signature = self.device.sign(&canonical);
        let mut request=format!("{method} {path} HTTP/1.1\r\nx-coven-protocol: 1\r\nx-coven-device: {id}\r\nx-coven-timestamp: {timestamp}\r\nx-coven-nonce: {nonce}\r\nx-coven-body-sha256: {digest}\r\nx-coven-signature: {}\r\ncontent-length: {}\r\n\r\n",URL_SAFE_NO_PAD.encode(signature.to_der().as_bytes()),body.len()).into_bytes();
        request.extend_from_slice(body);
        request
    }
}

#[cfg(unix)]
#[test]
fn relay_daemon_process_stops_after_authenticated_idle_connection() {
    exercise_relay_daemon_process(false);
}

#[cfg(unix)]
#[test]
fn relay_daemon_process_stops_during_unfinished_tls_handshake() {
    exercise_relay_daemon_process(true);
}

/// Waits for the first TLS record byte on a socket accepted from the polled
/// non-blocking fixture listener. BSD-derived kernels (macOS) let the accepted
/// socket inherit `O_NONBLOCK` from the listener; Linux does not. Without the
/// reset, a ClientHello that arrives after `accept` makes the read fail at once
/// with `WouldBlock` instead of waiting for the read timeout (#1636). The
/// socket stays blocking afterward, so the later close check also waits.
#[cfg(unix)]
fn await_tls_record_type(socket: &mut std::net::TcpStream) -> u8 {
    use std::io::Read;
    socket.set_nonblocking(false).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(30)))
        .unwrap();
    let mut record_type = [0_u8];
    socket.read_exact(&mut record_type).unwrap();
    record_type[0]
}

#[cfg(unix)]
#[test]
fn stalled_tls_fixture_waits_for_a_client_hello_sent_after_accept() {
    use std::io::Write;
    // Match the daemon fixture: a polled non-blocking listener.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let mut client = std::net::TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    // Hang guard only; loopback connect is normally visible immediately.
    let accept_deadline = Instant::now() + Duration::from_secs(30);
    let mut socket = loop {
        match listener.accept() {
            Ok((socket, _)) => break socket,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                assert!(Instant::now() < accept_deadline, "fixture accept wedged");
                std::thread::sleep(Duration::from_millis(1));
            }
            Err(error) => panic!("fixture accept failed: {error}"),
        }
    };
    let inherited = rustix::fs::fcntl_getfl(&socket)
        .unwrap()
        .contains(rustix::fs::OFlags::NONBLOCK);
    // This records the platform behavior behind #1636, not a fixture choice.
    #[cfg(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "freebsd",
        target_os = "netbsd",
        target_os = "openbsd",
        target_os = "dragonfly"
    ))]
    assert!(inherited, "BSD accept() should inherit O_NONBLOCK");
    #[cfg(any(target_os = "linux", target_os = "android"))]
    assert!(!inherited, "Linux accept() should not inherit O_NONBLOCK");
    let _ = inherited;

    // Nothing has been written yet, so the read below starts before the
    // ClientHello byte exists. The delay is only a lower bound on that order.
    const DELAY: Duration = Duration::from_millis(200);
    let started = Instant::now();
    let writer = std::thread::spawn(move || {
        std::thread::sleep(DELAY);
        client.write_all(&[22]).unwrap();
        client
    });
    let record_type = await_tls_record_type(&mut socket);
    let waited = started.elapsed();
    let _client = writer.join().unwrap();
    assert_eq!(record_type, 22, "expected the delayed handshake byte");
    assert!(
        waited >= DELAY,
        "read returned before the delayed byte was sent: {waited:?}"
    );
    assert!(
        !rustix::fs::fcntl_getfl(&socket)
            .unwrap()
            .contains(rustix::fs::OFlags::NONBLOCK),
        "stalled TLS socket must stay blocking for the close check"
    );
}

#[cfg(unix)]
fn exercise_relay_daemon_process(stall_tls: bool) {
    use super::super::transport::{self, test_support::Relay, websocket};
    use std::process::{Child, Command, Stdio};

    const CHILD_ENV: &str = "COVEN_TEST_RELAY_DAEMON_CHILD";
    const TEST: &str = "mobile_memory::gateway::transport_tests::relay_daemon_process_stops_after_authenticated_idle_connection";
    if std::env::var_os(CHILD_ENV).is_some() {
        let home = crate::coven_home_dir().unwrap();
        crate::daemon::serve_forever(&home, Utc::now().to_rfc3339(), None, &[]).unwrap();
        return;
    }
    struct OwnedDaemon(Child);
    impl Drop for OwnedDaemon {
        fn drop(&mut self) {
            if self.0.try_wait().ok().flatten().is_none() {
                let _ = self.0.kill();
            }
            let _ = self.0.wait();
        }
    }

    // Keep the canonical Unix socket path below the macOS pathname limit.
    let fixture = Fixture::with_home(
        tempfile::Builder::new()
            .prefix("coven-relay-process-")
            .tempdir_in("/tmp")
            .unwrap(),
    );
    let home = &fixture.state.coven_home;
    let relay = Relay::start();
    let stalled_listener = stall_tls.then(|| {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        listener
    });
    let endpoint = stalled_listener.as_ref().map_or_else(
        || relay.configuration(),
        |listener| websocket::EndpointConfig {
            url: format!("wss://{}/ws", listener.local_addr().unwrap()),
            ca_certificates: Vec::new(),
        },
    );
    storage::RelayConfig {
        listener_limit: crate::mobile_memory::transport::storage::MAX_LISTENERS,
        version: 1,
        enabled: true,
        endpoints: vec![endpoint],
    }
    .save(home)
    .unwrap();
    let identity = super::super::identity::load_existing_host_identity(home).unwrap();
    let signer = SigningKey::from_pkcs8_der(&identity.private_key_der).unwrap();
    let provisioning = storage::load_existing(
        home,
        &fixture.state.registry,
        signer.verifying_key(),
        fixture.pairing.device_id,
    )
    .unwrap()
    .unwrap()
    .provisioning(&signer)
    .unwrap();
    let user = home.join("isolated-user");
    std::fs::create_dir(&user).unwrap();
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .env_clear()
        .args(["--exact", TEST, "--nocapture"])
        .env(CHILD_ENV, "1")
        .env("COVEN_HOME", home)
        .env("HOME", &user)
        .env("TMPDIR", home)
        .current_dir(home)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit());
    if let Some(path) = std::env::var_os("PATH") {
        command.env("PATH", path);
    }
    let mut daemon = OwnedDaemon(command.spawn().unwrap());
    let authority_before = fixture
        .state
        .registry
        .authorization_record(fixture.pairing.device_id)
        .unwrap()
        .unwrap();
    let observe_phase = |daemon: &mut OwnedDaemon, expected| {
        let started = Instant::now();
        // Hang guard only: wait on the actual daemon observation, never a sleep
        // selected to race TLS/Noise setup. Child exit reports failure early.
        let deadline = started + Duration::from_secs(120);
        loop {
            assert!(
                daemon.0.try_wait().unwrap().is_none(),
                "daemon exited while waiting for {expected:?}"
            );
            if let Some(observed) = crate::mobile_memory::relay_connection_for_doctor(home) {
                if observed.state == expected {
                    assert_eq!(observed.transport, transport::connection::Transport::Relay);
                    break;
                }
            }
            assert!(
                Instant::now() < deadline,
                "daemon never reported {expected:?}; observed wait {:?}",
                started.elapsed()
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    };
    // Startup/store initialization is outside the strict stop budget. This is
    // only a hang guard; process exit reports early failure while retrying.
    let startup_deadline = Instant::now() + Duration::from_secs(120);
    let mut stalled_socket = None;
    let mut channel = if let Some(listener) = stalled_listener {
        let mut socket = loop {
            assert!(
                daemon.0.try_wait().unwrap().is_none(),
                "daemon exited before TLS attempt"
            );
            match listener.accept() {
                Ok((socket, _)) => break socket,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(
                        Instant::now() < startup_deadline,
                        "daemon never attempted relay TLS"
                    );
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(error) => panic!("fixture accept failed: {error}"),
            }
        };
        assert_eq!(
            await_tls_record_type(&mut socket),
            22,
            "expected TLS handshake record"
        );
        // Receiving ClientHello proves the worker reached the real TLS path.
        // Send nothing back: shutdown must interrupt the pending handshake.
        stalled_socket = Some(socket);
        observe_phase(
            &mut daemon,
            transport::connection::ConnectionState::Connecting,
        );
        None
    } else {
        observe_phase(
            &mut daemon,
            transport::connection::ConnectionState::Authenticating,
        );
        let mut channel = loop {
            assert!(
                daemon.0.try_wait().unwrap().is_none(),
                "daemon exited during startup"
            );
            let slot = u64::try_from(Utc::now().timestamp()).unwrap() / storage::SLOT_SECONDS;
            let (room, credential) = provisioning.rendezvous(slot, false).unwrap();
            let attempt = relay
                .connect(websocket::Role::Client, &room, &credential)
                .and_then(|records| {
                    transport::connect(
                        records,
                        &fixture.pairing,
                        &provisioning.binding_signature,
                        &fixture.device,
                        &room,
                        false,
                    )
                });
            match attempt {
                Ok(channel) => break channel,
                Err(error) => {
                    assert!(
                        Instant::now() < startup_deadline,
                        "daemon relay startup failed: {error}"
                    );
                    std::thread::sleep(Duration::from_millis(50));
                }
            }
        };
        let deadline = Instant::now() + Duration::from_secs(30);
        channel
            .send_message(
                &fixture.request("GET", "/api/v1/mobile/device", fixture.pairing.device_id),
                MAX_MOBILE_REQUEST_BYTES,
                deadline,
            )
            .unwrap();
        let response = channel
            .receive_message(MAX_MOBILE_RESPONSE_BYTES, deadline)
            .unwrap();
        assert!(response.starts_with(b"HTTP/1.1 200"));

        Some(channel)
    };

    if !stall_tls {
        observe_phase(&mut daemon, transport::connection::ConnectionState::Online);
    }
    let started = Instant::now();
    assert!(crate::daemon::stop_background_server(home).unwrap());
    let elapsed = started.elapsed();
    assert!(
        elapsed < Duration::from_secs(2),
        "daemon stop exceeded its two-second contract: {elapsed:?}"
    );
    // Reap the owned child separately from the API budget and require graceful
    // success, so forced termination cannot masquerade as worker cancellation.
    let reap_deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Some(status) = daemon.0.try_wait().unwrap() {
            assert!(status.success(), "daemon did not stop gracefully: {status}");
            break;
        }
        assert!(
            Instant::now() < reap_deadline,
            "daemon remained alive after stop"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        crate::mobile_memory::relay_connection_for_doctor(home).is_none(),
        "stopped daemon retained a live connection observation"
    );
    assert_eq!(
        authority_before,
        fixture
            .state
            .registry
            .authorization_record(fixture.pairing.device_id)
            .unwrap()
            .unwrap()
    );
    if let Some(channel) = channel.as_mut() {
        assert!(channel
            .receive_message(
                MAX_MOBILE_RESPONSE_BYTES,
                Instant::now() + Duration::from_secs(30)
            )
            .is_err());
    }
    if let Some(mut socket) = stalled_socket {
        use std::io::Read;
        let mut remaining = Vec::new();
        if let Err(error) = socket.read_to_end(&mut remaining) {
            assert_eq!(
                error.kind(),
                std::io::ErrorKind::ConnectionReset,
                "stalled TLS socket did not close: {error}"
            );
        }
    }
}

#[test]
fn encrypted_gateway_keeps_per_request_signatures_and_shared_replay_state() {
    let fixture = Fixture::new();
    let path = "/api/v1/mobile/device";
    let wire = fixture.request("GET", path, fixture.pairing.device_id);
    let request = read_mobile_request(&mut wire.as_slice()).unwrap();
    assert_eq!(
        handle_protected_route(&fixture.state, MobileRoute::DeviceRead, path, request, true).status,
        200
    );
    let (replay, _) =
        handle_transport_message(&fixture.state, &fixture.pairing, false, &wire).unwrap();
    assert_eq!(replay.status, 409);
    let replay_body: serde_json::Value = serde_json::from_str(&replay.body).unwrap();
    assert_eq!(replay_body["error"]["code"], "request_replayed");
    let wire = fixture.request("GET", path, fixture.pairing.device_id);
    let (response, _) =
        handle_transport_message(&fixture.state, &fixture.pairing, false, &wire).unwrap();
    assert_eq!(response.status, 200);
    let wrong_id = fixture.request("GET", path, Uuid::new_v4());
    assert!(handle_transport_message(&fixture.state, &fixture.pairing, false, &wrong_id).is_err());
    let mut trailing = fixture.request("GET", path, fixture.pairing.device_id);
    trailing.extend_from_slice(b"GET /api/v1/mobile/device HTTP/1.1\r\n\r\n");
    assert!(handle_transport_message(&fixture.state, &fixture.pairing, false, &trailing).is_err());
    fixture
        .state
        .registry
        .suspend(fixture.pairing.device_id, Utc::now())
        .unwrap();
    assert!(handle_transport_message(&fixture.state, &fixture.pairing, false, &wire).is_err());
}

/// Direct ingress answers a query-string target with 404 before routing. The
/// encrypted transport must reject the same validly signed target instead of
/// stripping it: no route is dispatched, the verifier never sees the request
/// (its nonce stays unspent), and later requests keep per-request semantics.
#[test]
fn encrypted_gateway_rejects_query_targets_before_routing_or_verification() {
    let fixture = Fixture::new();
    let device_id = fixture.pairing.device_id;
    let direct = read_mobile_request(
        &mut fixture
            .request("GET", "/api/v1/mobile/device?probe=1", device_id)
            .as_slice(),
    )
    .unwrap();
    assert_eq!(handle_mobile_request(&fixture.state, direct).status, 404);

    // A stripped signed DELETE would revoke the paired device.
    let delete = fixture.request("DELETE", "/api/v1/mobile/device?probe=1", device_id);
    let Err(error) = handle_transport_message(&fixture.state, &fixture.pairing, false, &delete)
    else {
        panic!("query DELETE was served");
    };
    assert_eq!(
        error.to_string(),
        "transport request target has a query string"
    );
    let record = fixture
        .state
        .registry
        .authorization_record(device_id)
        .unwrap()
        .unwrap();
    assert!(
        record.device.revoked_at.is_none(),
        "query DELETE dispatched"
    );

    let target = "/api/v1/mobile/device?probe=1";
    let wire = fixture.request("GET", target, device_id);
    for direct in [true, false] {
        assert!(handle_transport_message(&fixture.state, &fixture.pairing, direct, &wire).is_err());
    }
    let request = read_mobile_request(&mut wire.as_slice()).unwrap();
    let auth = parse_auth_headers(&request.headers).unwrap();
    fixture
        .state
        .authenticator
        .verify_with_assurance(
            "GET",
            target,
            &request.body,
            &auth,
            parse_assurance_headers(&request.headers),
            Utc::now(),
        )
        .expect("rejected query request reached the verifier");

    let wire = fixture.request("GET", "/api/v1/mobile/device", device_id);
    let (response, close) =
        handle_transport_message(&fixture.state, &fixture.pairing, false, &wire).unwrap();
    assert_eq!((response.status, close), (200, false));
    let (replay, _) =
        handle_transport_message(&fixture.state, &fixture.pairing, false, &wire).unwrap();
    assert_eq!(replay.status, 409);
}

#[test]
fn relay_shutdown_cancels_contended_registry_lock_without_mutating_authority() {
    use fs2::FileExt;
    let fixture = Fixture::new();
    let relay = super::super::transport::test_support::Relay::start();
    let home = fixture.state.coven_home.clone();
    storage::RelayConfig {
        listener_limit: crate::mobile_memory::transport::storage::MAX_LISTENERS,
        version: 1,
        enabled: true,
        endpoints: vec![relay.configuration()],
    }
    .save(&home)
    .unwrap();
    let before = std::fs::read(home.join("mobile/devices.json")).unwrap();
    let identity = super::super::identity::load_existing_host_identity(&home).unwrap();
    let signer = Arc::new(SigningKey::from_pkcs8_der(&identity.private_key_der).unwrap());
    let state = Arc::new(fixture.state);
    let registry = Arc::clone(&state.registry);
    let held = crate::state_lock::open_lock_file(&home.join("mobile/.devices.lock")).unwrap();
    held.lock_exclusive().unwrap();
    let (ready, waiting) = std::sync::mpsc::sync_channel(1);
    registry.observe_lock_waits(ready);
    let runtime = relay_runtime::RelayHandle::start(state, signer).unwrap();
    // An explicit failed try-lock, not a sleep, proves the manager is waiting.
    waiting.recv_timeout(Duration::from_secs(30)).unwrap();
    let started = Instant::now();
    drop(runtime);
    let elapsed = started.elapsed();
    assert!(
        elapsed < Duration::from_secs(2),
        "registry contention exceeded daemon stop contract: {elapsed:?}"
    );
    assert!(
        registry.reload().is_err(),
        "stopping registry admitted another operation"
    );
    FileExt::unlock(&held).unwrap();
    assert_eq!(
        std::fs::read(home.join("mobile/devices.json")).unwrap(),
        before
    );
    assert!(
        DeviceRegistry::load(&home).is_ok(),
        "shutdown poisoned a future registry instance"
    );
}

/// A DirectOnly grant may bootstrap over the direct channel, but the response
/// omits the configured relay endpoints and their CA material, a relay-channel
/// request is refused, and the production relay worker still refuses the
/// device's relay attempt (#1614).
#[test]
fn direct_only_bootstrap_omits_relay_endpoints_and_relay_still_refuses() {
    let _guard = super::tests::TEST_GATEWAY_LOCK.lock().unwrap();
    use super::super::grant::{AssuranceLevel, DeviceGrantRestrictions, GrantTransportConstraint};
    use super::super::transport::{self, test_support::Relay, websocket};
    let fixture = Fixture::new();
    let device_id = fixture.pairing.device_id;
    let relay = Relay::start();
    let certificate =
        rcgen::generate_simple_self_signed(vec!["relay.example.test".into()]).unwrap();
    let private_endpoint = websocket::EndpointConfig {
        url: "wss://relay.example.test/ws".into(),
        ca_certificates: vec![URL_SAFE_NO_PAD.encode(certificate.cert.der())],
    };
    storage::RelayConfig {
        listener_limit: crate::mobile_memory::transport::storage::MAX_LISTENERS,
        version: 1,
        enabled: true,
        endpoints: vec![relay.configuration(), private_endpoint.clone()],
    }
    .save(&fixture.state.coven_home)
    .unwrap();
    let record = fixture
        .state
        .registry
        .authorization_record(device_id)
        .unwrap()
        .unwrap();
    let direct_only = record
        .grant
        .reissue(
            &record.device.public_key_x963,
            record.grant.scopes.clone(),
            DeviceGrantRestrictions {
                transport: GrantTransportConstraint::DirectOnly,
                ..DeviceGrantRestrictions::default()
            },
            AssuranceLevel::Possession,
            Utc::now(),
            Utc::now() + chrono::Duration::hours(1),
        )
        .unwrap();
    fixture
        .state
        .registry
        .replace_grant(device_id, direct_only)
        .unwrap();
    let identity =
        super::super::identity::load_existing_host_identity(&fixture.state.coven_home).unwrap();
    let signer = SigningKey::from_pkcs8_der(&identity.private_key_der).unwrap();
    let material = storage::load_or_provision(
        &fixture.state.coven_home,
        &fixture.state.registry,
        &signer,
        device_id,
    )
    .unwrap();
    let pairing = material.pairing.clone();
    let bootstrap = "/api/v1/mobile/transport/bootstrap";

    let wire = fixture.request("POST", bootstrap, device_id);
    let (response, _) = handle_transport_message(&fixture.state, &pairing, true, &wire).unwrap();
    assert_eq!(response.status, 200);
    for (label, marker) in [
        ("relay URL", relay.configuration().url),
        ("private relay URL", private_endpoint.url.clone()),
        ("relay CA", private_endpoint.ca_certificates[0].clone()),
    ] {
        assert!(
            !response.body.contains(&marker),
            "DirectOnly bootstrap disclosed the {label}"
        );
    }
    let envelope: serde_json::Value = serde_json::from_str(&response.body).unwrap();
    let provisioning: storage::Provisioning =
        serde_json::from_value(envelope["data"].clone()).unwrap();
    assert!(provisioning.relay_endpoints.is_empty());
    let trusted = provisioning
        .verify(&pairing.host_key, device_id, fixture.device.verifying_key())
        .unwrap();
    assert_eq!(trusted.grant_id, pairing.grant_id);

    let wire = fixture.request("POST", bootstrap, device_id);
    let Err(error) = handle_transport_message(&fixture.state, &pairing, false, &wire) else {
        panic!("DirectOnly bootstrap was served over a relay channel");
    };
    assert_eq!(error.to_string(), "grant requires direct transport");

    // Real router: the relay runtime runs with the device's current material
    // and a reachable endpoint, yet the device's relay session is refused.
    // The same material derives the same routing secret the host uses.
    let provisioning = material.provisioning(&signer).unwrap();
    let runtime = start_mobile_gateway_native_for_daemon(&fixture.state.coven_home)
        .unwrap()
        .unwrap();
    let registry_path = fixture.state.coven_home.join("mobile/devices.json");
    let registry_before = std::fs::read(&registry_path).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut attempts = 0;
    while Instant::now() < deadline {
        let slot = u64::try_from(Utc::now().timestamp()).unwrap() / storage::SLOT_SECONDS;
        let (room, credential) = provisioning.rendezvous(slot, false).unwrap();
        let connected = relay
            .connect(websocket::Role::Client, &room, &credential)
            .and_then(|records| {
                transport::connect(
                    records,
                    &pairing,
                    &provisioning.binding_signature,
                    &fixture.device,
                    &room,
                    false,
                )
            });
        assert!(connected.is_err(), "DirectOnly device admitted over relay");
        attempts += 1;
        // Paces attempts across the manager's one-second reload; the positive
        // control below, not this sleep, shows the worker could be reached.
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(attempts > 1);
    assert_eq!(std::fs::read(&registry_path).unwrap(), registry_before);

    // Positive control on the same runtime and router: lifting the restriction
    // lets the re-provisioned device connect and be served over the relay, so
    // the refusals above came from transport policy, not an unreachable worker.
    let record = fixture
        .state
        .registry
        .authorization_record(device_id)
        .unwrap()
        .unwrap();
    let unrestricted = record
        .grant
        .reissue(
            &record.device.public_key_x963,
            record.grant.scopes.clone(),
            DeviceGrantRestrictions::default(),
            AssuranceLevel::Possession,
            Utc::now(),
            Utc::now() + chrono::Duration::hours(1),
        )
        .unwrap();
    fixture
        .state
        .registry
        .replace_grant(device_id, unrestricted)
        .unwrap();
    let material = storage::load_or_provision(
        &fixture.state.coven_home,
        &fixture.state.registry,
        &signer,
        device_id,
    )
    .unwrap();
    let provisioning = material.provisioning(&signer).unwrap();
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut channel = loop {
        let slot = u64::try_from(Utc::now().timestamp()).unwrap() / storage::SLOT_SECONDS;
        let (room, credential) = provisioning.rendezvous(slot, false).unwrap();
        match relay
            .connect(websocket::Role::Client, &room, &credential)
            .and_then(|records| {
                transport::connect(
                    records,
                    &material.pairing,
                    &provisioning.binding_signature,
                    &fixture.device,
                    &room,
                    false,
                )
            }) {
            Ok(channel) => break channel,
            Err(error) => {
                assert!(Instant::now() < deadline, "relay connect failed: {error}");
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    };
    let wire = fixture.request("GET", "/api/v1/mobile/device", device_id);
    let deadline = Instant::now() + Duration::from_secs(30);
    channel
        .send_message(&wire, MAX_MOBILE_REQUEST_BYTES, deadline)
        .unwrap();
    assert!(channel
        .receive_message(MAX_MOBILE_RESPONSE_BYTES, deadline)
        .unwrap()
        .starts_with(b"HTTP/1.1 200"));
    drop(runtime);
}

#[test]
fn authenticated_bootstrap_reuses_identity_and_persists_routing_without_qr() {
    let fixture = Fixture::new();
    let wire = fixture.request(
        "POST",
        "/api/v1/mobile/transport/bootstrap",
        fixture.pairing.device_id,
    );
    let (response, _) =
        handle_transport_message(&fixture.state, &fixture.pairing, false, &wire).unwrap();
    assert_eq!(response.status, 200);
    let envelope: serde_json::Value = serde_json::from_str(&response.body).unwrap();
    let provisioning: storage::Provisioning =
        serde_json::from_value(envelope["data"].clone()).unwrap();
    let trusted = provisioning
        .verify(
            &fixture.pairing.host_key,
            fixture.pairing.device_id,
            fixture.device.verifying_key(),
        )
        .unwrap();
    assert_eq!(trusted.noise_key, fixture.pairing.noise_key);
    assert_eq!(trusted.grant_id, fixture.pairing.grant_id);
    assert!(provisioning.relay_endpoints.is_empty());
    let certificate =
        rcgen::generate_simple_self_signed(vec!["relay.example.test".into()]).unwrap();
    let relay = storage::RelayConfig {
        listener_limit: storage::MAX_LISTENERS,
        version: 1,
        enabled: true,
        endpoints: vec![super::super::transport::websocket::EndpointConfig {
            url: "wss://relay.example.test/ws".into(),
            ca_certificates: vec![URL_SAFE_NO_PAD.encode(certificate.cert.der())],
        }],
    };
    relay.save(&fixture.state.coven_home).unwrap();
    let wire = fixture.request(
        "POST",
        "/api/v1/mobile/transport/bootstrap",
        fixture.pairing.device_id,
    );
    let (again, _) =
        handle_transport_message(&fixture.state, &fixture.pairing, false, &wire).unwrap();
    let envelope: serde_json::Value = serde_json::from_str(&again.body).unwrap();
    let again: storage::Provisioning = serde_json::from_value(envelope["data"].clone()).unwrap();
    assert!(again.relay_endpoints == relay.endpoints);
    let updated = again
        .verify(
            &fixture.pairing.host_key,
            fixture.pairing.device_id,
            fixture.device.verifying_key(),
        )
        .unwrap();
    assert_eq!(updated.noise_key, trusted.noise_key);
    assert_eq!(updated.grant_id, trusted.grant_id);
    assert_eq!(
        provisioning.rendezvous(123, false).unwrap(),
        again.rendezvous(123, false).unwrap()
    );
}

#[test]
fn relay_only_runtime_reconnects_without_identity_change_and_stops_idle_workers() {
    let _guard = super::tests::TEST_GATEWAY_LOCK.lock().unwrap();
    use super::super::transport::{self, test_support::Relay, websocket};
    let fixture = Fixture::new();
    let relay = Relay::start();
    let config = MobileGatewayConfig {
        enabled: false,
        bind: "192.168.50.1:7443".parse().unwrap(),
        advertised_endpoint: "https://192.168.50.1:7443".into(),
    };
    super::super::config::save_mobile_config(&fixture.state.coven_home, &config).unwrap();
    storage::RelayConfig {
        listener_limit: crate::mobile_memory::transport::storage::MAX_LISTENERS,
        version: 1,
        enabled: true,
        endpoints: vec![relay.configuration()],
    }
    .save(&fixture.state.coven_home)
    .unwrap();
    let identity =
        super::super::identity::load_existing_host_identity(&fixture.state.coven_home).unwrap();
    let signer = SigningKey::from_pkcs8_der(&identity.private_key_der).unwrap();
    let material = storage::load_existing(
        &fixture.state.coven_home,
        &fixture.state.registry,
        signer.verifying_key(),
        fixture.pairing.device_id,
    )
    .unwrap()
    .unwrap();
    let provisioning = material.provisioning(&signer).unwrap();
    let identity_path = fixture.state.coven_home.join("mobile/host-identity.json");
    let before = std::fs::read(&identity_path).unwrap();

    for round in 0..3_u8 {
        if round == 2 {
            super::super::config::remove_mobile_config(&fixture.state.coven_home).unwrap();
            let mut relay_config = storage::RelayConfig::load(&fixture.state.coven_home)
                .unwrap()
                .unwrap();
            relay_config.enabled = true;
            relay_config.save(&fixture.state.coven_home).unwrap();
        }
        let runtime = start_mobile_gateway_native_for_daemon(&fixture.state.coven_home)
            .unwrap()
            .unwrap();
        assert!(
            runtime._direct.is_none(),
            "relay-only startup opened a private listener"
        );
        let deadline = Instant::now() + Duration::from_secs(60);
        let mut channel = loop {
            let slot = u64::try_from(Utc::now().timestamp()).unwrap() / storage::SLOT_SECONDS;
            let (room, credential) = provisioning.rendezvous(slot, false).unwrap();
            let connected = relay
                .connect(websocket::Role::Client, &room, &credential)
                .and_then(|records| {
                    transport::connect(
                        records,
                        &fixture.pairing,
                        &provisioning.binding_signature,
                        &fixture.device,
                        &room,
                        false,
                    )
                });
            match connected {
                Ok(channel) => break channel,
                Err(error) => {
                    assert!(Instant::now() < deadline, "relay reconnect failed: {error}");
                    // Retry backoff exercises the real room-arrival race. A sleep
                    // is not used as proof that the opposite role is registered.
                    std::thread::sleep(Duration::from_millis(50));
                }
            }
        };
        let wire = fixture.request("GET", "/api/v1/mobile/device", fixture.pairing.device_id);
        let deadline = Instant::now() + Duration::from_secs(30);
        channel
            .send_message(&wire, MAX_MOBILE_REQUEST_BYTES, deadline)
            .unwrap();
        let response = channel
            .receive_message(MAX_MOBILE_RESPONSE_BYTES, deadline)
            .unwrap();
        assert!(response.starts_with(b"HTTP/1.1 200"));
        assert_eq!(
            std::fs::read(&identity_path).unwrap(),
            before,
            "relay startup rotated the LAN certificate"
        );
        if round == 1 {
            assert!(storage::RelayConfig::disable(&fixture.state.coven_home).unwrap());
            // Eight seconds separates the one-second configuration poll from
            // the thirty-second keepalive interval of the idle wait. A timeout
            // error cannot satisfy this assertion: the relay must send Close.
            let error = channel
                .receive_message(
                    MAX_MOBILE_RESPONSE_BYTES,
                    Instant::now() + Duration::from_secs(8),
                )
                .unwrap_err();
            assert_eq!(error.to_string(), "unexpected relay record");
        }
        if round == 2 {
            fixture
                .state
                .registry
                .revoke(fixture.pairing.device_id, Utc::now())
                .unwrap();
            let revoked =
                fixture.request("GET", "/api/v1/mobile/device", fixture.pairing.device_id);
            if channel
                .send_message(&revoked, MAX_MOBILE_REQUEST_BYTES, deadline)
                .is_ok()
            {
                assert!(channel
                    .receive_message(MAX_MOBILE_RESPONSE_BYTES, deadline)
                    .is_err());
            }
        }
        let started = Instant::now();
        drop(runtime);
        // The daemon's documented stop budget is a contract, not a jitter guard.
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "relay runtime stop exceeded two seconds: {:?}",
            started.elapsed()
        );
        if round == 0 {
            assert!(channel
                .receive_message(
                    MAX_MOBILE_RESPONSE_BYTES,
                    Instant::now() + Duration::from_secs(30)
                )
                .is_err());
        }
    }
}

/// An authenticated relay channel with nothing to say is held open: the worker
/// does not tear it down at the former 15-second request deadline, the device
/// completes a signed request on the same Noise session afterwards without
/// reconnecting, the host made no further relay connection meanwhile, and the
/// daemon still stops inside its two-second budget while the channel is idle
/// (#1609).
#[test]
fn relay_worker_holds_an_idle_authenticated_channel_and_serves_a_later_signed_request() {
    let _guard = super::tests::TEST_GATEWAY_LOCK.lock().unwrap();
    use super::super::transport::{self, test_support::Relay, websocket};
    let fixture = Fixture::new();
    let relay = Relay::start();
    super::super::config::save_mobile_config(
        &fixture.state.coven_home,
        &MobileGatewayConfig {
            enabled: false,
            bind: "192.168.50.1:7443".parse().unwrap(),
            advertised_endpoint: "https://192.168.50.1:7443".into(),
        },
    )
    .unwrap();
    storage::RelayConfig {
        listener_limit: crate::mobile_memory::transport::storage::MAX_LISTENERS,
        version: 1,
        enabled: true,
        endpoints: vec![relay.configuration()],
    }
    .save(&fixture.state.coven_home)
    .unwrap();
    let identity =
        super::super::identity::load_existing_host_identity(&fixture.state.coven_home).unwrap();
    let signer = SigningKey::from_pkcs8_der(&identity.private_key_der).unwrap();
    let device_id = fixture.pairing.device_id;
    let material = storage::load_existing(
        &fixture.state.coven_home,
        &fixture.state.registry,
        signer.verifying_key(),
        device_id,
    )
    .unwrap()
    .unwrap();
    let provisioning = material.provisioning(&signer).unwrap();
    let runtime = start_mobile_gateway_native_for_daemon(&fixture.state.coven_home)
        .unwrap()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut channel = loop {
        let slot = u64::try_from(Utc::now().timestamp()).unwrap() / storage::SLOT_SECONDS;
        let (room, credential) = provisioning.rendezvous(slot, false).unwrap();
        match relay
            .connect(websocket::Role::Client, &room, &credential)
            .and_then(|records| {
                transport::connect(
                    records,
                    &fixture.pairing,
                    &provisioning.binding_signature,
                    &fixture.device,
                    &room,
                    false,
                )
            }) {
            Ok(channel) => break channel,
            Err(error) => {
                assert!(Instant::now() < deadline, "relay connect failed: {error}");
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    };
    let exchange = |channel: &mut transport::Channel<websocket::WebSocketRecords>| {
        let wire = fixture.request("GET", "/api/v1/mobile/device", device_id);
        let deadline = Instant::now() + Duration::from_secs(30);
        channel
            .send_message(&wire, MAX_MOBILE_REQUEST_BYTES, deadline)
            .and_then(|()| channel.receive_message(MAX_MOBILE_RESPONSE_BYTES, deadline))
    };
    assert!(exchange(&mut channel).unwrap().starts_with(b"HTTP/1.1 200"));
    let observed = handle_local_control_native("POST", LOCAL_CONNECTION_PATH, Some("{}"))
        .unwrap()
        .unwrap();
    assert_eq!(observed.status, 200);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&observed.body).unwrap(),
        serde_json::json!({"state":"online", "transport":"relay"})
    );
    assert_eq!(
        handle_local_control_native("GET", LOCAL_CONNECTION_PATH, None)
            .unwrap()
            .unwrap()
            .status,
        405
    );

    // Every host connection is an upgrade with `role=host` in the relay's
    // TLS-terminated capture. The count must not move while the channel idles.
    let host_upgrades = |captured: &[u8]| {
        captured
            .windows(b"role=host".len())
            .filter(|part| *part == b"role=host")
            .count()
    };
    let before = relay.captured();
    let upgrades_before = host_upgrades(&before);
    assert!(upgrades_before >= 1);
    // The idle period is the behavior under test, not a sequencing wait: it
    // exceeds the former 15-second request deadline that tore the channel down.
    std::thread::sleep(Duration::from_secs(16));
    assert!(
        exchange(&mut channel)
            .expect("signed request after the idle period failed")
            .starts_with(b"HTTP/1.1 200"),
        "signed request after the idle period was not served"
    );
    let after = relay.captured();
    assert_eq!(
        host_upgrades(&after),
        upgrades_before,
        "host reconnected during the idle period"
    );
    assert!(
        !after[before.len()..]
            .windows(b"GET /ws".len())
            .any(|part| part == b"GET /ws"),
        "a relay upgrade crossed the broker during the idle period"
    );

    let started = Instant::now();
    drop(runtime);
    // The daemon's documented stop budget is a contract, not a jitter guard.
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "relay runtime stop exceeded two seconds with an idle channel: {:?}",
        started.elapsed()
    );
    let observed = handle_local_control_native("POST", LOCAL_CONNECTION_PATH, Some("{}"))
        .unwrap()
        .unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&observed.body).unwrap(),
        serde_json::json!({"state":"offline", "transport":"relay"})
    );
    assert!(channel
        .receive_message(
            MAX_MOBILE_RESPONSE_BYTES,
            Instant::now() + Duration::from_secs(30)
        )
        .is_err());
}

/// Two persistent synthetic peers through the production relay router: a
/// device that misses a rendezvous slot (or runs a clock skewed across a slot
/// boundary) is never paired and recovers by rejoining the current slot
/// without re-enrollment; a grant rotation or revocation closes the live
/// session and blocks reconnection with the stale material; re-provisioned
/// material reconnects; a daemon restart keeps identity and material; and the
/// relay's TLS-terminated capture holds no plaintext or stable identifier.
#[test]
fn relay_runtime_recovers_a_missed_slot_and_closes_rotated_or_revoked_sessions() {
    let _guard = super::tests::TEST_GATEWAY_LOCK.lock().unwrap();
    use super::super::grant::{AssuranceLevel, DeviceGrantRestrictions};
    use super::super::transport::test_support::{assert_bytes_exclude, stable_identifier_markers};
    use super::super::transport::{self, test_support::Relay, websocket};
    let fixture = Fixture::new();
    let relay = Relay::start();
    super::super::config::save_mobile_config(
        &fixture.state.coven_home,
        &MobileGatewayConfig {
            enabled: false,
            bind: "192.168.50.1:7443".parse().unwrap(),
            advertised_endpoint: "https://192.168.50.1:7443".into(),
        },
    )
    .unwrap();
    storage::RelayConfig {
        listener_limit: crate::mobile_memory::transport::storage::MAX_LISTENERS,
        version: 1,
        enabled: true,
        endpoints: vec![relay.configuration()],
    }
    .save(&fixture.state.coven_home)
    .unwrap();
    let identity =
        super::super::identity::load_existing_host_identity(&fixture.state.coven_home).unwrap();
    let signer = SigningKey::from_pkcs8_der(&identity.private_key_der).unwrap();
    let device_id = fixture.pairing.device_id;
    let identity_path = fixture.state.coven_home.join("mobile/host-identity.json");
    let identity_before = std::fs::read(&identity_path).unwrap();
    let registry_path = fixture.state.coven_home.join("mobile/devices.json");
    let current_slot = || u64::try_from(Utc::now().timestamp()).unwrap() / storage::SLOT_SECONDS;
    let attempt = |provisioning: &storage::Provisioning, pairing: &Pairing, slot: u64| {
        let (room, credential) = provisioning.rendezvous(slot, false).unwrap();
        relay
            .connect(websocket::Role::Client, &room, &credential)
            .and_then(|records| {
                transport::connect(
                    records,
                    pairing,
                    &provisioning.binding_signature,
                    &fixture.device,
                    &room,
                    false,
                )
            })
    };
    let connect_current = |provisioning: &storage::Provisioning, pairing: &Pairing| {
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            match attempt(provisioning, pairing, current_slot()) {
                Ok(channel) => break channel,
                Err(error) => {
                    assert!(Instant::now() < deadline, "relay connect failed: {error}");
                    std::thread::sleep(Duration::from_millis(50));
                }
            }
        }
    };
    let exchange = |channel: &mut transport::Channel<websocket::WebSocketRecords>| {
        let wire = fixture.request("GET", "/api/v1/mobile/device", device_id);
        let deadline = Instant::now() + Duration::from_secs(30);
        channel
            .send_message(&wire, MAX_MOBILE_REQUEST_BYTES, deadline)
            .and_then(|()| channel.receive_message(MAX_MOBILE_RESPONSE_BYTES, deadline))
    };

    let material = storage::load_existing(
        &fixture.state.coven_home,
        &fixture.state.registry,
        signer.verifying_key(),
        device_id,
    )
    .unwrap()
    .unwrap();
    let provisioning = material.provisioning(&signer).unwrap();
    let pairing = material.pairing.clone();
    let runtime = start_mobile_gateway_native_for_daemon(&fixture.state.coven_home)
        .unwrap()
        .unwrap();

    // Missed slot / clock skew across a slot boundary: the previous and next
    // rooms hold no host, so the relay closes the attempt before any Noise
    // response. The failure is bounded and leaves durable authority untouched.
    let registry_before = std::fs::read(&registry_path).unwrap();
    for skewed in [current_slot() - 1, current_slot() + 1] {
        let started = Instant::now();
        assert!(attempt(&provisioning, &pairing, skewed).is_err());
        assert!(started.elapsed() < Duration::from_secs(30));
    }
    assert_eq!(std::fs::read(&registry_path).unwrap(), registry_before);
    let mut channel = connect_current(&provisioning, &pairing);
    assert!(exchange(&mut channel).unwrap().starts_with(b"HTTP/1.1 200"));

    // A signed query-string target closes the live session with no response,
    // matching direct ingress's rejection; the same material reconnects.
    let query = fixture.request("GET", "/api/v1/mobile/device?probe=1", device_id);
    let deadline = Instant::now() + Duration::from_secs(30);
    assert!(
        channel
            .send_message(&query, MAX_MOBILE_REQUEST_BYTES, deadline)
            .and_then(|()| channel.receive_message(MAX_MOBILE_RESPONSE_BYTES, deadline))
            .is_err(),
        "query-string target was served"
    );
    assert!(exchange(&mut channel).is_err());
    let mut channel = connect_current(&provisioning, &pairing);
    assert!(exchange(&mut channel).unwrap().starts_with(b"HTTP/1.1 200"));

    // Grant rotation mid-session: the live channel fails on its next request,
    // the stale material cannot reconnect, and only re-provisioned material
    // (obtained over the existing authenticated bootstrap path) reconnects.
    let record = fixture
        .state
        .registry
        .authorization_record(device_id)
        .unwrap()
        .unwrap();
    let rotated = record
        .grant
        .reissue(
            &record.device.public_key_x963,
            record.grant.scopes.clone(),
            DeviceGrantRestrictions::default(),
            AssuranceLevel::Possession,
            Utc::now(),
            Utc::now() + chrono::Duration::hours(1),
        )
        .unwrap();
    fixture
        .state
        .registry
        .replace_grant(device_id, rotated)
        .unwrap();
    assert!(
        exchange(&mut channel).is_err(),
        "rotated grant served a request"
    );
    let started = Instant::now();
    assert!(attempt(&provisioning, &pairing, current_slot()).is_err());
    assert!(started.elapsed() < Duration::from_secs(30));
    let rotated_material = storage::load_or_provision(
        &fixture.state.coven_home,
        &fixture.state.registry,
        &signer,
        device_id,
    )
    .unwrap();
    assert_ne!(rotated_material.pairing.noise_key, pairing.noise_key);
    let rotated_provisioning = rotated_material.provisioning(&signer).unwrap();
    let rotated_pairing = rotated_material.pairing.clone();
    assert_ne!(
        rotated_provisioning
            .rendezvous(current_slot(), false)
            .unwrap()
            .0,
        provisioning.rendezvous(current_slot(), false).unwrap().0
    );
    let mut channel = connect_current(&rotated_provisioning, &rotated_pairing);
    assert!(exchange(&mut channel).unwrap().starts_with(b"HTTP/1.1 200"));

    // Restart: the same material reconnects; no identity or material changes.
    let started = Instant::now();
    drop(runtime);
    assert!(started.elapsed() < Duration::from_secs(2));
    assert!(exchange(&mut channel).is_err());
    let runtime = start_mobile_gateway_native_for_daemon(&fixture.state.coven_home)
        .unwrap()
        .unwrap();
    let mut channel = connect_current(&rotated_provisioning, &rotated_pairing);
    assert!(exchange(&mut channel).unwrap().starts_with(b"HTTP/1.1 200"));
    assert_eq!(std::fs::read(&identity_path).unwrap(), identity_before);
    let reloaded = storage::load_existing(
        &fixture.state.coven_home,
        &fixture.state.registry,
        signer.verifying_key(),
        device_id,
    )
    .unwrap()
    .unwrap();
    assert_eq!(reloaded.pairing.noise_key, rotated_pairing.noise_key);

    // Revocation mid-session: the request fails and reconnection is refused.
    fixture
        .state
        .registry
        .revoke(device_id, Utc::now())
        .unwrap();
    assert!(
        exchange(&mut channel).is_err(),
        "revoked device served a request"
    );
    let started = Instant::now();
    assert!(attempt(&rotated_provisioning, &rotated_pairing, current_slot()).is_err());
    assert!(started.elapsed() < Duration::from_secs(30));
    let started = Instant::now();
    drop(runtime);
    assert!(started.elapsed() < Duration::from_secs(2));

    let captured = relay.captured();
    assert!(captured
        .windows(b"room=".len())
        .any(|part| part == b"room="));
    let mut markers = stable_identifier_markers(&pairing);
    markers.extend(stable_identifier_markers(&rotated_pairing));
    markers.push(("request path", b"/api/v1/mobile/device".to_vec()));
    markers.push(("request header", b"x-coven-device".to_vec()));
    markers.push(("response status", b"HTTP/1.1 200".to_vec()));
    markers.push(("completion plaintext", b"host-finished-v1".to_vec()));
    assert_bytes_exclude(&captured, &markers);
}

#[test]
fn signed_direct_v2_bootstrap_prepares_adopts_acknowledges_and_resyncs_without_v1_fallback() {
    use crate::mobile_memory::transport::{routing::Transport, storage::v2};
    let fixture = Fixture::new();
    let id = fixture.pairing.device_id;
    let path = "/api/v1/mobile/transport/bootstrap";
    let direct_request = |body: serde_json::Value| {
        let wire = fixture.request_body("POST", path, id, &serde_json::to_vec(&body).unwrap());
        let request =
            read_mobile_request_buffered(&mut BufReader::new(std::io::Cursor::new(wire))).unwrap();
        handle_mobile_request(&fixture.state, request)
    };
    let response = direct_request(serde_json::json!({"version":2,"operation":"prepare"}));
    assert_eq!(response.status, 200, "{}", response.body);
    let envelope: serde_json::Value = serde_json::from_str(&response.body).unwrap();
    assert_eq!(
        envelope["data"]["version"], 2,
        "v2 bootstrap returned v1 material"
    );
    let handover: v2::Handover = serde_json::from_value(envelope["data"].clone()).unwrap();
    let identity =
        super::super::identity::load_existing_host_identity(&fixture.state.coven_home).unwrap();
    let host = SigningKey::from_pkcs8_der(&identity.private_key_der).unwrap();
    assert!(v2::load(
        &fixture.state.coven_home,
        &fixture.state.registry,
        host.verifying_key(),
        id,
        Transport::Relay
    )
    .unwrap()
    .is_none());
    let device_home = tempfile::tempdir().unwrap();
    let ack = v2::device::adopt_direct(
        device_home.path(),
        handover,
        host.verifying_key(),
        id,
        fixture.device.verifying_key(),
    )
    .unwrap();
    let response = direct_request(
        serde_json::json!({"version":2,"operation":"acknowledge","generation":ack.generation,"seedDigest":ack.seed_digest}),
    );
    assert_eq!(response.status, 200, "{}", response.body);
    let response = direct_request(
        serde_json::json!({"version":2,"operation":"resync","transport":"relay","generation":0,"next":4}),
    );
    assert_eq!(response.status, 200, "{}", response.body);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&response.body).unwrap()["data"]["floor"],
        4
    );
    assert_eq!(
        v2::load(
            &fixture.state.coven_home,
            &fixture.state.registry,
            host.verifying_key(),
            id,
            Transport::Relay
        )
        .unwrap()
        .unwrap()
        .floors,
        [0, 4]
    );
    std::fs::remove_file(
        fixture
            .state
            .coven_home
            .join(format!("mobile/transport-{id}.json")),
    )
    .unwrap();
    let response =
        direct_request(serde_json::json!({"version":2,"operation":"recover","knownGeneration":0}));
    assert_eq!(response.status, 200, "{}", response.body);
    let handover: v2::Handover = serde_json::from_value(
        serde_json::from_str::<serde_json::Value>(&response.body).unwrap()["data"].clone(),
    )
    .unwrap();
    assert_eq!(handover.generation, 1);
    let ack = v2::device::adopt_direct(
        device_home.path(),
        handover,
        host.verifying_key(),
        id,
        fixture.device.verifying_key(),
    )
    .unwrap();
    assert_eq!(direct_request(serde_json::json!({"version":2,"operation":"acknowledge","generation":ack.generation,"seedDigest":ack.seed_digest})).status,200);
    assert_eq!(
        v2::load(
            &fixture.state.coven_home,
            &fixture.state.registry,
            host.verifying_key(),
            id,
            Transport::Relay
        )
        .unwrap()
        .unwrap()
        .generation,
        1
    );
    assert_ne!(
        direct_request(serde_json::json!({"version":2,"operation":"prepare","extra":true})).status,
        200
    );
    let wire = fixture.request("POST", path, id);
    let request =
        read_mobile_request_buffered(&mut BufReader::new(std::io::Cursor::new(wire))).unwrap();
    assert_ne!(
        handle_mobile_request(&fixture.state, request).status,
        200,
        "legacy bootstrap reopened v1"
    );
}

#[test]
fn relay_ingress_cannot_prepare_acknowledge_or_resync_v2_routing() {
    let fixture = Fixture::new();
    let id = fixture.pairing.device_id;
    let path = "/api/v1/mobile/transport/bootstrap";
    let material = fixture
        .state
        .coven_home
        .join(format!("mobile/transport-{id}.json"));
    let before = std::fs::read(&material).unwrap();
    for body in [
        serde_json::json!({"version":2,"operation":"recover","knownGeneration":0}),
        serde_json::json!({"version":2,"operation":"prepare"}),
        serde_json::json!({"version":2,"operation":"acknowledge","generation":0,"seedDigest":vec![0;32]}),
        serde_json::json!({"version":2,"operation":"resync","transport":"relay","generation":0,"next":4}),
    ] {
        let wire = fixture.request_body("POST", path, id, &serde_json::to_vec(&body).unwrap());
        let (response, _) =
            handle_transport_message(&fixture.state, &fixture.pairing, false, &wire).unwrap();
        assert_eq!(response.status, 403, "{}", response.body);
        assert_eq!(std::fs::read(&material).unwrap(), before);
        assert!(!response.body.contains("routingSeed"));
    }
}

#[test]
fn v2_upgrade_blocks_legacy_noise_effects_but_retains_authenticated_direct_recovery() {
    let fixture = Fixture::new();
    let id = fixture.pairing.device_id;
    let identity =
        super::super::identity::load_existing_host_identity(&fixture.state.coven_home).unwrap();
    let host = SigningKey::from_pkcs8_der(&identity.private_key_der).unwrap();
    let handover = storage::v2::prepare_direct(
        &fixture.state.coven_home,
        &fixture.state.registry,
        &host,
        id,
    )
    .unwrap();
    let home = tempfile::tempdir().unwrap();
    let ack = storage::v2::device::adopt_direct(
        home.path(),
        handover,
        host.verifying_key(),
        id,
        fixture.device.verifying_key(),
    )
    .unwrap();
    storage::v2::acknowledge_direct(
        &fixture.state.coven_home,
        &fixture.state.registry,
        &host,
        id,
        &ack,
    )
    .unwrap();
    for direct in [false, true] {
        let wire = fixture.request("DELETE", "/api/v1/mobile/device", id);
        assert!(
            handle_transport_message(&fixture.state, &fixture.pairing, direct, &wire).is_err(),
            "legacy Noise channel retained application authority after v2 upgrade"
        );
        assert!(fixture
            .state
            .registry
            .authorization_record(id)
            .unwrap()
            .unwrap()
            .device
            .revoked_at
            .is_none());
    }
    let wire = fixture.request_body(
        "POST",
        "/api/v1/mobile/transport/bootstrap",
        id,
        br#"{"version":2,"operation":"prepare"}"#,
    );
    let (response, _) =
        handle_transport_message(&fixture.state, &fixture.pairing, true, &wire).unwrap();
    assert_eq!(response.status, 200, "{}", response.body);
}

#[test]
fn generation_change_after_initial_admission_cannot_delete_device() {
    let fixture = Fixture::new();
    let id = fixture.pairing.device_id;
    let home = fixture.state.coven_home.clone();
    let registry = Arc::clone(&fixture.state.registry);
    AFTER_ROUTING_ADMISSION.with(|slot| {
        *slot.borrow_mut() = Some(Box::new(move || {
            let identity = super::super::identity::load_existing_host_identity(&home).unwrap();
            let host = SigningKey::from_pkcs8_der(&identity.private_key_der).unwrap();
            storage::v2::prepare_direct(&home, &registry, &host, id).unwrap();
        }));
    });
    let wire = fixture.request("DELETE", "/api/v1/mobile/device", id);
    assert!(handle_transport_message(&fixture.state, &fixture.pairing, true, &wire).is_err());
    assert!(fixture
        .state
        .registry
        .authorization_record(id)
        .unwrap()
        .unwrap()
        .device
        .revoked_at
        .is_none());
}

#[test]
fn counter_runtime_advertises_four_rooms_per_endpoint_and_serves_durable_device() {
    let _guard = super::tests::TEST_GATEWAY_LOCK.lock().unwrap();
    use super::super::transport::{routing::Transport, test_support::Relay, websocket};
    let fixture = Fixture::new();
    let relays = [Relay::start(), Relay::start()];
    super::super::config::save_mobile_config(
        &fixture.state.coven_home,
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
        listener_limit: 512,
        endpoints: relays.iter().map(Relay::configuration).collect(),
    }
    .save(&fixture.state.coven_home)
    .unwrap();
    let identity =
        super::super::identity::load_existing_host_identity(&fixture.state.coven_home).unwrap();
    let host = SigningKey::from_pkcs8_der(&identity.private_key_der).unwrap();
    let id = fixture.pairing.device_id;
    let handover = storage::v2::prepare_direct(
        &fixture.state.coven_home,
        &fixture.state.registry,
        &host,
        id,
    )
    .unwrap();
    let home = tempfile::tempdir().unwrap();
    let ack = storage::v2::device::adopt_direct(
        home.path(),
        handover,
        host.verifying_key(),
        id,
        fixture.device.verifying_key(),
    )
    .unwrap();
    storage::v2::acknowledge_direct(
        &fixture.state.coven_home,
        &fixture.state.registry,
        &host,
        id,
        &ack,
    )
    .unwrap();
    let runtime = start_mobile_gateway_native_for_daemon(&fixture.state.coven_home)
        .unwrap()
        .unwrap();
    assert!(runtime._direct.is_none());
    for round in 0..6_u64 {
        let snapshot = storage::v2::load(
            &fixture.state.coven_home,
            &fixture.state.registry,
            host.verifying_key(),
            id,
            Transport::Relay,
        )
        .unwrap()
        .unwrap();
        // A generous hang guard; readiness is each actual outgoing host room join,
        // not a delay or merely the existence of the daemon thread.
        let until = Instant::now() + Duration::from_secs(120);
        loop {
            let complete = relays.iter().all(|relay| {
                let captured = relay.captured();
                let wire = String::from_utf8_lossy(&captured);
                (round..round + 4).all(|counter| {
                    let (room, _) = snapshot.rendezvous(Transport::Relay, counter).unwrap();
                    wire.contains(&format!("room={}&role=host", URL_SAFE_NO_PAD.encode(room)))
                })
            });
            if complete {
                break;
            }
            assert!(
                Instant::now() < until,
                "daemon never advertised the four counter rooms on both endpoints"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        let reconnect_started = Instant::now();
        let mut retries = 0_u64;
        let (attempt, mut channel) = loop {
            let mut attempt = storage::v2::device::begin(
                home.path(),
                host.verifying_key(),
                id,
                fixture.device.verifying_key(),
                Transport::Relay,
            )
            .unwrap();
            let before = attempt.state();
            let (room, credential) = attempt.rendezvous().unwrap();
            let connected = relays[1]
                .connect(websocket::Role::Client, &room, &credential)
                .and_then(|records| attempt.connect(records, &fixture.device));
            match connected {
                Ok(channel) => break (attempt, channel),
                Err(error) => {
                    drop(attempt);
                    let retained = storage::v2::device::begin(
                        home.path(),
                        host.verifying_key(),
                        id,
                        fixture.device.verifying_key(),
                        Transport::Relay,
                    )
                    .unwrap();
                    // Room arrival/expiry may fail before a genuine host proof.
                    // Never retry away an advance or a lost completion here.
                    assert_eq!(
                        retained.state(),
                        before,
                        "round {round} failed after durable device consumption: {error:#}"
                    );
                    drop(retained);
                    retries += 1;
                    eprintln!("counter reconnect round {round} pre-consumption failure {retries}: {error:#}");
                    assert!(
                        reconnect_started.elapsed() < Duration::from_secs(120),
                        "round {round} reconnect wedged: {error:#}"
                    );
                    std::thread::sleep(Duration::from_millis(50));
                }
            }
        };
        eprintln!(
            "counter reconnect round {round}: {:?}, pre-consumption retries {retries}",
            reconnect_started.elapsed()
        );
        let deadline = Instant::now() + Duration::from_secs(30);
        channel
            .send_message(
                &fixture.request("GET", "/api/v1/mobile/device", id),
                MAX_MOBILE_REQUEST_BYTES,
                deadline,
            )
            .unwrap();
        let response = channel
            .receive_message(MAX_MOBILE_RESPONSE_BYTES, deadline)
            .unwrap();
        assert!(response.starts_with(b"HTTP/1.1 200"));
        assert_eq!(
            storage::v2::load(
                &fixture.state.coven_home,
                &fixture.state.registry,
                host.verifying_key(),
                id,
                Transport::Relay
            )
            .unwrap()
            .unwrap()
            .floors,
            [0, round + 1]
        );
        drop(channel);
        drop(attempt);
    }
    let started = Instant::now();
    drop(runtime);
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "counter daemon stop exceeded two seconds: {:?}",
        started.elapsed()
    );
}

#[test]
fn restore_fence_rejects_signed_direct_https_effects_until_fresh_handover() {
    for (phase, complete) in [
        ("installing", false),
        ("awaitingHandovers", false),
        ("awaitingHandovers", true),
    ] {
        let fixture = Fixture::new();
        let id = fixture.pairing.device_id;
        let directory = fixture.state.coven_home.join("mobile");
        if phase == "awaitingHandovers" {
            let path = directory.join("devices.json");
            let mut registry: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            registry["restoreEpoch"] = serde_json::json!(1);
            super::super::config::atomic_replace_private(
                &path,
                &serde_json::to_vec(&registry).unwrap(),
            )
            .unwrap();
        }
        let state = serde_json::json!({
            "version": 1, "operationId": Uuid::from_u128(99), "sourceDigest": vec![0_u8; 32],
            "installationFingerprint": fixture.state.host_fingerprint,
            "installationEpoch": 1, "phase": phase,
            "pairings": [{ "deviceId": id, "generation": 1, "revocationEpoch": 1, "handoverComplete": complete }]
        });
        super::super::config::atomic_replace_private(
            &directory.join(".transport-restore.required"),
            b"coven.mobile.restore.v1\n",
        )
        .unwrap();
        super::super::config::atomic_replace_private(
            &directory.join("transport-restore.json"),
            &serde_json::to_vec(&state).unwrap(),
        )
        .unwrap();
        let wire = fixture.request("DELETE", "/api/v1/mobile/device", id);
        let mut reader = BufReader::new(std::io::Cursor::new(wire));
        let request = read_mobile_request_buffered(&mut reader).unwrap();
        let response = handle_mobile_request(&fixture.state, request);
        assert_eq!(response.status, 503, "{phase}: {}", response.body);
        assert!(fixture
            .state
            .registry
            .authorization_record(id)
            .unwrap()
            .unwrap()
            .device
            .revoked_at
            .is_none());
    }
}

mod v2_authority;
mod v2_control_loss;
#[cfg(unix)]
mod v2_deadline;

#[test]
fn online_connection_never_overrides_denial_or_restores_device_authority() {
    use crate::mobile_memory::transport::connection::{ConnectionState, Transport};
    for revoked in [false, true] {
        let fixture = Fixture::new();
        let mut attempt = fixture.state.connections.begin(Transport::Relay).unwrap();
        attempt.authenticating().unwrap();
        attempt.online().unwrap();
        if revoked {
            fixture
                .state
                .registry
                .revoke(fixture.pairing.device_id, Utc::now())
                .unwrap();
        } else {
            fixture
                .state
                .registry
                .suspend(fixture.pairing.device_id, Utc::now())
                .unwrap();
        }
        let before = fixture
            .state
            .registry
            .authorization_record(fixture.pairing.device_id)
            .unwrap()
            .unwrap();
        assert_eq!(
            fixture
                .state
                .connections
                .snapshot(Transport::Relay)
                .unwrap()
                .state,
            ConnectionState::Online
        );
        let request = fixture.request("GET", "/api/v1/mobile/device", fixture.pairing.device_id);
        assert!(
            handle_transport_message(&fixture.state, &fixture.pairing, false, &request).is_err()
        );
        drop(attempt);
        assert_eq!(
            fixture
                .state
                .connections
                .snapshot(Transport::Relay)
                .unwrap()
                .state,
            ConnectionState::Offline
        );
        let after = fixture
            .state
            .registry
            .authorization_record(fixture.pairing.device_id)
            .unwrap()
            .unwrap();
        assert_eq!(before.device.suspended_at, after.device.suspended_at);
        assert_eq!(before.device.revoked_at, after.device.revoked_at);
        assert_eq!(before.grant.id, after.grant.id);
        assert_eq!(before.grant.revocation_epoch, after.grant.revocation_epoch);
        assert_eq!(after.device.revoked_at.is_some(), revoked);
    }
}

#[test]
fn migration_overlapping_real_relay_channels_preserves_pending_operation_and_applies_once() {
    use super::super::transport::{
        self,
        connection::{ConnectionState, Transport},
        migration::Migration,
        test_support::Relay,
        websocket,
    };
    use std::sync::mpsc;
    let fixture = Fixture::with_authorization();
    let before = fixture
        .state
        .registry
        .authorization_record(fixture.pairing.device_id)
        .unwrap()
        .unwrap();
    let identity =
        super::super::identity::load_existing_host_identity(&fixture.state.coven_home).unwrap();
    let host = SigningKey::from_pkcs8_der(&identity.private_key_der).unwrap();
    let material = storage::load_existing(
        &fixture.state.coven_home,
        &fixture.state.registry,
        host.verifying_key(),
        fixture.pairing.device_id,
    )
    .unwrap()
    .unwrap();
    let provisioning = material.provisioning(&host).unwrap();
    let slot = u64::try_from(Utc::now().timestamp()).unwrap() / storage::SLOT_SECONDS;
    let (room, credential) = provisioning.rendezvous(slot, false).unwrap();
    let relays = [Relay::start(), Relay::start()];
    let migration = Migration::new(
        fixture.pairing.clone(),
        provisioning.binding_signature.clone(),
        fixture.device.clone(),
    )
    .unwrap();
    let pending = migration
        .prepare("POST", "/api/v1/mobile/assurance/challenge", &[])
        .unwrap();
    let operation_id = pending.id();
    let request_hashes = Arc::new(Mutex::new(Vec::new()));
    std::thread::scope(|scope| {
        let (applied_tx, applied_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let (consumed_tx, consumed_rx) = mpsc::channel();
        let first_relay = &relays[0];
        let state = &fixture.state;
        let pairing = &fixture.pairing;
        let host_ref = &host;
        let material_ref = &material;
        let credential_ref = &credential;
        let hashes = Arc::clone(&request_hashes);
        let old_host = scope.spawn(move || {
            let records = first_relay
                .connect(websocket::Role::Host, &room, credential_ref)
                .unwrap();
            let mut channel = transport::accept(
                records,
                pairing,
                &material_ref.noise_private,
                host_ref,
                &room,
                false,
                |pairing, direct| {
                    transport::authority::check_current(
                        &state.registry,
                        pairing,
                        direct,
                        Utc::now(),
                    )
                },
            )
            .unwrap();
            let encoded = channel
                .receive_message(
                    MAX_MOBILE_REQUEST_BYTES + 16 * 1024,
                    Instant::now() + Duration::from_secs(30),
                )
                .unwrap();
            hashes
                .lock()
                .unwrap()
                .push(Sha256::digest(&*encoded).to_vec());
            let (response, _) = handle_transport_message(state, pairing, false, &encoded).unwrap();
            assert_eq!(response.status, 200);
            applied_tx.send(()).unwrap();
            // Explicit post-effect fault: the old request has executed, but its
            // response is lost while a freshly authenticated channel takes over.
            // Test-only hang guard: registration, connect and authentication
            // each permit 30s, and two exchanges permit 15s each. Leave ample
            // scheduler margin beyond those roughly 120s of valid phases.
            let budget = Duration::from_secs(240);
            let started = Instant::now();
            release_rx.recv_timeout(budget).unwrap_or_else(|error| {
                panic!(
                    "handoff never released old response fault after {:?} (budget {budget:?}): {error}",
                    started.elapsed()
                )
            });
            channel.close();
        });
        let candidate = migration.begin(Transport::Relay).unwrap();
        let records = relays[0]
            .connect(websocket::Role::Client, &room, &credential)
            .unwrap();
        migration
            .commit(candidate.authenticate(records, &room).unwrap())
            .unwrap();
        let old_request = scope.spawn(|| migration.exchange(&pending));
        applied_rx
            .recv_timeout(Duration::from_secs(30))
            .expect("old operation never reached its side effect");

        let next_relay = &relays[1];
        let hashes = Arc::clone(&request_hashes);
        let next_host = scope.spawn(move || {
            let records = next_relay
                .connect(websocket::Role::Host, &room, credential_ref)
                .unwrap();
            let mut channel = transport::accept(
                records,
                pairing,
                &material_ref.noise_private,
                host_ref,
                &room,
                false,
                |pairing, direct| {
                    transport::authority::check_current(
                        &state.registry,
                        pairing,
                        direct,
                        Utc::now(),
                    )
                },
            )
            .unwrap();
            for index in 0..3 {
                let encoded = channel
                    .receive_message(
                        MAX_MOBILE_REQUEST_BYTES + 16 * 1024,
                        Instant::now() + Duration::from_secs(30),
                    )
                    .unwrap();
                if index == 0 {
                    hashes
                        .lock()
                        .unwrap()
                        .push(Sha256::digest(&*encoded).to_vec());
                }
                let (response, _) =
                    handle_transport_message(state, pairing, false, &encoded).unwrap();
                channel
                    .send_message(
                        &response.to_http(),
                        MAX_MOBILE_RESPONSE_BYTES + 16 * 1024,
                        Instant::now() + Duration::from_secs(30),
                    )
                    .unwrap();
            }
            // A successful send only queues the relay record. Keep the new host
            // alive until the client consumes the final response: relay peer
            // disconnect may correctly take priority over queued application data.
            consumed_rx
                .recv_timeout(Duration::from_secs(30))
                .expect("client never consumed the final handoff response");
        });
        let candidate = migration.begin(Transport::Relay).unwrap();
        let records = relays[1]
            .connect(websocket::Role::Client, &room, &credential)
            .unwrap();
        migration
            .commit(candidate.authenticate(records, &room).unwrap())
            .unwrap();
        let replay = migration.exchange(&pending).unwrap();
        assert_eq!(replay.operation_id, operation_id);
        assert!(replay.bytes.starts_with(b"HTTP/1.1 409"));
        assert!(std::str::from_utf8(&replay.bytes)
            .unwrap()
            .contains("request_replayed"));
        let control = migration
            .prepare("GET", "/api/v1/mobile/device", &[])
            .unwrap();
        assert!(migration
            .exchange(&control)
            .unwrap()
            .bytes
            .starts_with(b"HTTP/1.1 200"));
        release_tx.send(()).unwrap();
        assert!(old_request.join().unwrap().is_err());
        assert_eq!(
            migration.status().unwrap().unwrap().state,
            ConnectionState::Online
        );
        let control = migration
            .prepare("GET", "/api/v1/mobile/device", &[])
            .unwrap();
        assert!(migration
            .exchange(&control)
            .unwrap()
            .bytes
            .starts_with(b"HTTP/1.1 200"));
        consumed_tx.send(()).unwrap();
        old_host.join().unwrap();
        next_host.join().unwrap();
    });
    let hashes = request_hashes.lock().unwrap();
    assert_eq!(hashes.len(), 2);
    assert_eq!(
        hashes[0], hashes[1],
        "handoff changed the pending signed request"
    );
    let challenges: serde_json::Value = serde_json::from_slice(
        &std::fs::read(
            fixture
                .state
                .coven_home
                .join("mobile")
                .join(super::super::assurance::ASSURANCE_CHALLENGES_FILE),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(
        challenges["challenges"].as_array().unwrap().len(),
        1,
        "overlapping replay repeated the side effect"
    );
    assert_eq!(
        before,
        fixture
            .state
            .registry
            .authorization_record(fixture.pairing.device_id)
            .unwrap()
            .unwrap()
    );
    assert_eq!(pending.id(), operation_id);
}

struct FirstFlight<R> {
    inner: R,
    first: Arc<Mutex<Option<Vec<u8>>>>,
}
impl<R: super::super::transport::Records> super::super::transport::Records for FirstFlight<R> {
    fn send(&mut self, bytes: &[u8], deadline: Instant) -> Result<()> {
        let mut first = self.first.lock().unwrap();
        if first.is_none() {
            *first = Some(bytes.to_vec());
        }
        drop(first);
        self.inner.send(bytes, deadline)
    }
    fn receive(&mut self, deadline: Instant) -> Result<Vec<u8>> {
        self.inner.receive(deadline)
    }
    fn receive_idle(&mut self) -> Result<Vec<u8>> {
        self.inner.receive_idle()
    }
}

#[test]
fn migration_relay_direct_relay_uses_fresh_keys_and_same_pinned_authority() {
    use super::super::transport::{
        self,
        cancellation::Cancellation,
        connection::{ConnectionState, Transport},
        migration::Migration,
        test_support::Relay,
        websocket,
    };
    // The registry admits 128 devices. A direct candidate must still route to
    // a provisioned pairing after 32 earlier active, unprovisioned records.
    let fixture = Fixture::configured_after(tempfile::tempdir().unwrap(), false, 32);
    assert_eq!(
        fixture.state.registry.list_status().unwrap()[32].id,
        fixture.pairing.device_id
    );
    let before = fixture
        .state
        .registry
        .authorization_record(fixture.pairing.device_id)
        .unwrap()
        .unwrap();
    let identity =
        super::super::identity::load_existing_host_identity(&fixture.state.coven_home).unwrap();
    let host = SigningKey::from_pkcs8_der(&identity.private_key_der).unwrap();
    let tls_config = tls_server_config(
        identity.certificate_der.clone(),
        identity.private_key_der.to_vec(),
    )
    .unwrap();
    let material = storage::load_existing(
        &fixture.state.coven_home,
        &fixture.state.registry,
        host.verifying_key(),
        fixture.pairing.device_id,
    )
    .unwrap()
    .unwrap();
    let provisioning = material.provisioning(&host).unwrap();
    let slot = u64::try_from(Utc::now().timestamp()).unwrap() / storage::SLOT_SECONDS;
    let (relay_room, relay_credential) = provisioning.rendezvous(slot, false).unwrap();
    let (direct_room, direct_credential) = provisioning.rendezvous(slot, true).unwrap();
    let relays = [Relay::start(), Relay::start()];
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let direct_endpoint = websocket::EndpointConfig {
        url: format!("wss://192.0.2.1:{}/ws", address.port()),
        ca_certificates: vec![URL_SAFE_NO_PAD.encode(&identity.certificate_der)],
    }
    .parse()
    .unwrap();
    let flights: Vec<_> = (0..3).map(|_| Arc::new(Mutex::new(None))).collect();
    std::thread::scope(|scope| {
        let migration = Migration::new(
            fixture.pairing.clone(),
            provisioning.binding_signature.clone(),
            fixture.device.clone(),
        )
        .unwrap();
        let state = &fixture.state;
        let pairing = &fixture.pairing;
        let material_ref = &material;
        let host_ref = &host;
        let relay_credential_ref = &relay_credential;
        let first_relay = &relays[0];
        let first_host = scope.spawn(move || {
            let records = first_relay
                .connect(websocket::Role::Host, &relay_room, relay_credential_ref)
                .unwrap();
            let mut channel = transport::accept(
                records,
                pairing,
                &material_ref.noise_private,
                host_ref,
                &relay_room,
                false,
                |pairing, direct| {
                    transport::authority::check_current(
                        &state.registry,
                        pairing,
                        direct,
                        Utc::now(),
                    )
                },
            )
            .unwrap();
            for _ in 0..2 {
                serve_transport_with_state(state, &mut channel).unwrap();
            }
        });
        let candidate = migration.begin(Transport::Relay).unwrap();
        let records = relays[0]
            .connect(websocket::Role::Client, &relay_room, &relay_credential)
            .unwrap();
        migration
            .commit(
                candidate
                    .authenticate(
                        FirstFlight {
                            inner: records,
                            first: Arc::clone(&flights[0]),
                        },
                        &relay_room,
                    )
                    .unwrap(),
            )
            .unwrap();
        let request = migration
            .prepare("GET", "/api/v1/mobile/device", &[])
            .unwrap();
        assert!(migration
            .exchange(&request)
            .unwrap()
            .bytes
            .starts_with(b"HTTP/1.1 200"));

        let direct_host = scope.spawn(move || {
            let (socket, _) = listener.accept().unwrap();
            socket.set_nonblocking(false).unwrap();
            serve_tls_connection(socket, tls_config, state)
        });
        let candidate = migration.begin(Transport::Direct).unwrap();
        let records = websocket::connect_cancellable(
            &direct_endpoint,
            address,
            websocket::Role::Client,
            &direct_room,
            &direct_credential,
            Instant::now() + Duration::from_secs(30),
            &Cancellation::default(),
        )
        .unwrap();
        let candidate = candidate
            .authenticate(
                FirstFlight {
                    inner: records,
                    first: Arc::clone(&flights[1]),
                },
                &direct_room,
            )
            .unwrap();
        assert_eq!(
            migration.status().unwrap().unwrap().transport,
            Transport::Relay
        );
        // The old relay remains useful after the direct endpoint has proved its
        // identity, until the caller explicitly commits the handoff.
        let request = migration
            .prepare("GET", "/api/v1/mobile/device", &[])
            .unwrap();
        assert!(migration
            .exchange(&request)
            .unwrap()
            .bytes
            .starts_with(b"HTTP/1.1 200"));
        migration.commit(candidate).unwrap();
        assert_eq!(
            migration.status().unwrap().unwrap().transport,
            Transport::Direct
        );
        let request = migration
            .prepare("GET", "/api/v1/mobile/device", &[])
            .unwrap();
        assert!(migration
            .exchange(&request)
            .unwrap()
            .bytes
            .starts_with(b"HTTP/1.1 200"));
        assert_eq!(
            state.connections.snapshot(Transport::Direct).unwrap().state,
            ConnectionState::Online
        );

        let last_relay = &relays[1];
        let last_host = scope.spawn(move || {
            let records = last_relay
                .connect(websocket::Role::Host, &relay_room, relay_credential_ref)
                .unwrap();
            let mut channel = transport::accept(
                records,
                pairing,
                &material_ref.noise_private,
                host_ref,
                &relay_room,
                false,
                |pairing, direct| {
                    transport::authority::check_current(
                        &state.registry,
                        pairing,
                        direct,
                        Utc::now(),
                    )
                },
            )
            .unwrap();
            serve_transport_with_state(state, &mut channel).unwrap();
        });
        let candidate = migration.begin(Transport::Relay).unwrap();
        let records = relays[1]
            .connect(websocket::Role::Client, &relay_room, &relay_credential)
            .unwrap();
        let candidate = candidate
            .authenticate(
                FirstFlight {
                    inner: records,
                    first: Arc::clone(&flights[2]),
                },
                &relay_room,
            )
            .unwrap();
        let request = migration
            .prepare("GET", "/api/v1/mobile/device", &[])
            .unwrap();
        assert!(migration
            .exchange(&request)
            .unwrap()
            .bytes
            .starts_with(b"HTTP/1.1 200"));
        migration.commit(candidate).unwrap();
        let request = migration
            .prepare("GET", "/api/v1/mobile/device", &[])
            .unwrap();
        assert!(migration
            .exchange(&request)
            .unwrap()
            .bytes
            .starts_with(b"HTTP/1.1 200"));
        assert_eq!(
            migration.status().unwrap().unwrap().transport,
            Transport::Relay
        );
        state.stop_network_connections();
        first_host.join().unwrap();
        let _ = direct_host.join().unwrap();
        last_host.join().unwrap();
        assert_eq!(
            state.connections.snapshot(Transport::Direct).unwrap().state,
            ConnectionState::Offline
        );
    });
    let first_keys: Vec<_> = flights
        .iter()
        .map(|flight| flight.lock().unwrap().as_ref().unwrap()[..32].to_vec())
        .collect();
    assert_ne!(
        first_keys[0], first_keys[1],
        "direct handoff reused a Noise ephemeral key"
    );
    assert_ne!(
        first_keys[1], first_keys[2],
        "relay handoff reused a Noise ephemeral key"
    );
    assert_ne!(
        first_keys[0], first_keys[2],
        "reconnect reused a Noise ephemeral key"
    );
    assert_eq!(
        before,
        fixture
            .state
            .registry
            .authorization_record(fixture.pairing.device_id)
            .unwrap()
            .unwrap()
    );
}

#[test]
fn direct_wss_rejects_invalid_routing_before_upgrade() {
    use super::super::transport::{routing, storage::v2};
    use rustls::{pki_types::ServerName, ClientConfig, ClientConnection, RootCertStore};

    let fixture = Fixture::with_authorization();
    let identity =
        super::super::identity::load_existing_host_identity(&fixture.state.coven_home).unwrap();
    let host = SigningKey::from_pkcs8_der(&identity.private_key_der).unwrap();
    let id = fixture.pairing.device_id;
    let handover = v2::prepare_direct(
        &fixture.state.coven_home,
        &fixture.state.registry,
        &host,
        id,
    )
    .unwrap();
    let device_home = tempfile::tempdir().unwrap();
    let ack = v2::device::adopt_direct(
        device_home.path(),
        handover,
        host.verifying_key(),
        id,
        fixture.device.verifying_key(),
    )
    .unwrap();
    v2::acknowledge_direct(
        &fixture.state.coven_home,
        &fixture.state.registry,
        &host,
        id,
        &ack,
    )
    .unwrap();
    let snapshot = v2::load(
        &fixture.state.coven_home,
        &fixture.state.registry,
        host.verifying_key(),
        id,
        routing::Transport::Direct,
    )
    .unwrap()
    .unwrap();
    let (room, credential) = snapshot.rendezvous(routing::Transport::Direct, 0).unwrap();
    let room = URL_SAFE_NO_PAD.encode(room);
    let bearer = format!("Bearer {}", URL_SAFE_NO_PAD.encode(credential.as_slice()));
    let valid = format!("/ws?v=1&role=client&room={room}");
    let before = fixture.state.registry.authorization_record(id).unwrap();
    let server_tls = tls_server_config(
        identity.certificate_der.clone(),
        identity.private_key_der.to_vec(),
    )
    .unwrap();
    let mut roots = RootCertStore::empty();
    roots
        .add(rustls::pki_types::CertificateDer::from(
            identity.certificate_der,
        ))
        .unwrap();
    let client_tls = Arc::new(
        ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_protocol_versions(&[&rustls::version::TLS13])
            .unwrap()
            .with_root_certificates(roots)
            .with_no_client_auth(),
    );
    let try_upgrade = |method: &str, target: &str, authorization: &str| {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        // Hang guards only. Queue both socket ends before spawning the TLS
        // worker so early fixture failure cannot leave an accept() waiting.
        let socket =
            TcpStream::connect_timeout(&listener.local_addr().unwrap(), Duration::from_secs(30))
                .unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(30)))
            .unwrap();
        socket
            .set_write_timeout(Some(Duration::from_secs(30)))
            .unwrap();
        let (server, _) = listener.accept().unwrap();
        std::thread::scope(|scope| {
            let worker = scope
                .spawn(|| serve_tls_connection(server, Arc::clone(&server_tls), &fixture.state));
            let connection = ClientConnection::new(
                Arc::clone(&client_tls),
                ServerName::try_from("192.0.2.1").unwrap(),
            )
            .unwrap();
            let mut tls = StreamOwned::new(connection, socket);
            let request = format!("{method} {target} HTTP/1.1\r\nHost: 192.0.2.1\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nAuthorization: {authorization}\r\n\r\n");
            tls.write_all(request.as_bytes()).unwrap();
            let mut status = [0_u8; 12];
            let upgraded = tls.read_exact(&mut status).is_ok() && &status == b"HTTP/1.1 101";
            // Close before joining: the positive control deliberately stops
            // after upgrade, before it sends any Noise possession proof.
            drop(tls);
            (upgraded, worker.join().unwrap())
        })
    };
    assert!(
        try_upgrade("GET", &valid, &bearer).0,
        "positive upgrade control failed"
    );
    let cases = [
        (
            "version",
            "GET",
            format!("/ws?v=2&role=client&room={room}"),
            bearer.clone(),
            "invalid direct routing",
        ),
        (
            "role",
            "GET",
            format!("/ws?v=1&role=host&room={room}"),
            bearer.clone(),
            "invalid direct routing",
        ),
        (
            "missing",
            "GET",
            format!("/ws?role=client&room={room}"),
            bearer.clone(),
            "invalid direct routing",
        ),
        (
            "duplicate",
            "GET",
            format!("{valid}&v=1"),
            bearer.clone(),
            "invalid direct routing",
        ),
        (
            "unknown",
            "GET",
            format!("{valid}&extra=1"),
            bearer.clone(),
            "invalid direct routing",
        ),
        (
            "padded room",
            "GET",
            format!("/ws?v=1&role=client&room={room}%3D"),
            bearer.clone(),
            "invalid direct routing",
        ),
        (
            "wrong room",
            "GET",
            format!(
                "/ws?v=1&role=client&room={}",
                URL_SAFE_NO_PAD.encode([255; 32])
            ),
            bearer.clone(),
            "direct pairing unavailable",
        ),
        (
            "wrong credential",
            "GET",
            valid.clone(),
            format!("Bearer {}", URL_SAFE_NO_PAD.encode([255; 32])),
            "direct pairing unavailable",
        ),
        (
            "padded credential",
            "GET",
            valid.clone(),
            format!("{bearer}="),
            "invalid direct routing",
        ),
        (
            "authorization scheme",
            "GET",
            valid.clone(),
            "Basic synthetic".into(),
            "invalid direct routing",
        ),
        (
            "fragment",
            "GET",
            format!("{valid}#fragment"),
            bearer.clone(),
            "invalid direct routing",
        ),
        (
            "method",
            "POST",
            valid.clone(),
            bearer.clone(),
            "invalid direct upgrade",
        ),
    ];
    for (case, method, target, authorization, expected) in cases {
        let (upgraded, result) = try_upgrade(method, &target, &authorization);
        assert!(!upgraded, "{case} reached the WebSocket upgrade");
        let error = result
            .expect_err("invalid routing was accepted")
            .to_string();
        assert!(error.contains(expected), "{case}: {error}");
    }
    assert!(
        try_upgrade("GET", &valid, &bearer).0,
        "rejections damaged positive upgrade control"
    );
    assert_eq!(
        before,
        fixture.state.registry.authorization_record(id).unwrap()
    );
    let after = v2::load(
        &fixture.state.coven_home,
        &fixture.state.registry,
        host.verifying_key(),
        id,
        routing::Transport::Direct,
    )
    .unwrap()
    .unwrap();
    assert_eq!(
        snapshot.floors, after.floors,
        "upgrade spent a routing counter before endpoint proof"
    );
}

#[test]
fn connection_observation_query_refuses_ambiguous_or_private_fields() {
    let _guard = super::tests::TEST_GATEWAY_LOCK.lock().unwrap();
    for body in [
        "{\"transport\":null}",
        "{\"transport\":\"unknown\"}",
        "{\"transport\":\"direct\",\"room\":\"private\"}",
        "[]",
    ] {
        let response = handle_local_control_native("POST", LOCAL_CONNECTION_PATH, Some(body))
            .unwrap()
            .unwrap();
        assert_eq!(response.status, 400);
        assert!(!response.body.contains("private"));
    }
    let response = handle_local_control_native(
        "POST",
        LOCAL_CONNECTION_PATH,
        Some("{\"transport\":\"direct\"}"),
    )
    .unwrap()
    .unwrap();
    let response: serde_json::Value = serde_json::from_str(&response.body).unwrap();
    assert_eq!(response["transport"], "direct");
    assert_eq!(response.as_object().unwrap().len(), 2);
}

mod v2_migration;

mod v2_journeys;

/// Initial store loading has no runtime handle to drop yet. Bound admission
/// while the competing store lock remains held, and retain exact authority.
#[test]
fn relay_startup_bounds_initial_store_contention_without_mutating_authority() {
    use fs2::FileExt;
    let _gateway = super::tests::TEST_GATEWAY_LOCK.lock().unwrap();
    let mut failures = Vec::new();
    for lock_name in [
        ".devices.lock",
        ".authorization-keys.lock",
        ".assurance-challenges.lock",
    ] {
        let fixture = Fixture::new();
        let relay = super::super::transport::test_support::Relay::start();
        let home = fixture.state.coven_home.clone();
        storage::RelayConfig {
            listener_limit: storage::MAX_LISTENERS,
            version: 1,
            enabled: true,
            endpoints: vec![relay.configuration()],
        }
        .save(&home)
        .unwrap();
        let files = [
            "devices.json",
            "authorization-keys.json",
            "assurance-challenges.json",
        ];
        let before: Vec<_> = files
            .iter()
            .map(|name| std::fs::read(home.join("mobile").join(name)).ok())
            .collect();
        let lock_path = home.join("mobile").join(lock_name);
        let held = crate::state_lock::open_lock_file(&lock_path).unwrap();
        held.lock_exclusive().unwrap();
        let (_observation, waiting) = super::super::store_lock::observe(&lock_path);
        let (finished, result) = std::sync::mpsc::sync_channel(1);
        let worker_home = home.clone();
        let worker = std::thread::spawn(move || {
            // Do not send a live runtime to another thread: cleanup remains
            // owned by this producer even if the old implementation succeeds.
            let outcome = start_mobile_gateway_native_for_daemon(&worker_home).map(drop);
            let _ = finished.send(outcome);
        });
        // A 30-second hang guard permits loaded scheduling. The actual failed
        // file-lock attempt, rather than a sleep, selects the contended path.
        let observed = waiting.recv_timeout(Duration::from_secs(30));
        let started = Instant::now();
        let bounded = if observed.is_ok() {
            result.recv_timeout(Duration::from_secs(2))
        } else {
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected)
        };
        let elapsed = started.elapsed();
        // Cleanup precedes assertions so a red regression never leaves a
        // blocked producer or competing store lock behind.
        FileExt::unlock(&held).unwrap();
        worker.join().unwrap();
        assert!(
            observed.is_ok(),
            "no failed acquisition observed for {lock_name}: {observed:?}"
        );
        match bounded {
            Ok(Err(error)) => assert!(
                error.to_string().contains("deadline"),
                "startup failed for an unrelated reason at {lock_name}: {error:#}"
            ),
            other => failures.push(format!(
                "{lock_name}: admission was not bounded while held ({elapsed:?}): {other:?}"
            )),
        }
        let after: Vec<_> = files
            .iter()
            .map(|name| std::fs::read(home.join("mobile").join(name)).ok())
            .collect();
        assert_eq!(after, before, "startup changed authority at {lock_name}");
        assert!(
            DeviceRegistry::load(&home).is_ok(),
            "startup failure poisoned future registry at {lock_name}"
        );
    }
    assert!(failures.is_empty(), "{}", failures.join("; "));
}

/// A real encrypted relay request can hold the runtime in an authority-store
/// wait after socket cancellation. Keep each competing file lock held until
/// either shutdown completes or the strict contract has failed.
#[test]
fn relay_shutdown_cancels_contended_authority_stores_without_mutating_bytes() {
    use super::super::transport::{self, test_support::Relay, websocket};
    use fs2::FileExt;
    let _gateway = super::tests::TEST_GATEWAY_LOCK.lock().unwrap();
    let mut failures = Vec::new();
    for lock_name in [
        ".authorization-keys.lock",
        ".assurance-challenges.lock",
        ".audit.lock",
    ] {
        let fixture = Fixture::with_authorization();
        let home = fixture.state.coven_home.clone();
        let relay = Relay::start();
        storage::RelayConfig {
            listener_limit: storage::MAX_LISTENERS,
            version: 1,
            enabled: true,
            endpoints: vec![relay.configuration()],
        }
        .save(&home)
        .unwrap();
        let identity = super::super::identity::load_existing_host_identity(&home).unwrap();
        let signer = SigningKey::from_pkcs8_der(&identity.private_key_der).unwrap();
        let provisioning = storage::load_existing(
            &home,
            &fixture.state.registry,
            signer.verifying_key(),
            fixture.pairing.device_id,
        )
        .unwrap()
        .unwrap()
        .provisioning(&signer)
        .unwrap();
        let runtime = start_mobile_gateway_native_for_daemon(&home)
            .unwrap()
            .unwrap();
        // This is only a room-arrival hang guard, independent of stop's strict budget.
        let connect_deadline = Instant::now() + Duration::from_secs(60);
        let mut channel = loop {
            let slot = u64::try_from(Utc::now().timestamp()).unwrap() / storage::SLOT_SECONDS;
            let (room, credential) = provisioning.rendezvous(slot, false).unwrap();
            let attempt = relay
                .connect(websocket::Role::Client, &room, &credential)
                .and_then(|records| {
                    transport::connect(
                        records,
                        &fixture.pairing,
                        &provisioning.binding_signature,
                        &fixture.device,
                        &room,
                        false,
                    )
                });
            match attempt {
                Ok(channel) => break channel,
                Err(error) => {
                    assert!(
                        Instant::now() < connect_deadline,
                        "relay arrival failed: {error:#}"
                    );
                    std::thread::sleep(Duration::from_millis(50));
                }
            }
        };
        let files = [
            "devices.json",
            "authorization-keys.json",
            "assurance-challenges.json",
            "audit.jsonl",
        ];
        let before: Vec<_> = files
            .iter()
            .map(|name| std::fs::read(home.join("mobile").join(name)).ok())
            .collect();
        let path = home.join("mobile").join(lock_name);
        let held = crate::state_lock::open_lock_file(&path).unwrap();
        held.lock_exclusive().unwrap();
        let (_observation, waiting) = super::super::store_lock::observe(&path);
        let mut wire = fixture.request(
            "POST",
            "/api/v1/mobile/assurance/challenge",
            fixture.pairing.device_id,
        );
        if lock_name == ".audit.lock" {
            // Preserve valid syntax and identity while invalidating the signature.
            let text = String::from_utf8(wire).unwrap();
            wire = text
                .replace(
                    "POST /api/v1/mobile/assurance/challenge",
                    "GET /api/v1/mobile/device",
                )
                .into_bytes();
        }
        channel
            .send_message(
                &wire,
                MAX_MOBILE_REQUEST_BYTES,
                Instant::now() + Duration::from_secs(30),
            )
            .unwrap();
        let observed = waiting.recv_timeout(Duration::from_secs(30));
        let (finished, stopped) = std::sync::mpsc::sync_channel(1);
        let started = Instant::now();
        let stopper = std::thread::spawn(move || {
            drop(runtime);
            let _ = finished.send(());
        });
        let bounded = stopped.recv_timeout(Duration::from_secs(2));
        let elapsed = started.elapsed();
        FileExt::unlock(&held).unwrap();
        stopper.join().unwrap();
        assert!(
            observed.is_ok(),
            "no actual file-lock wait for {lock_name}: {observed:?}"
        );
        if bounded.is_err() {
            failures.push(format!(
                "{lock_name}: stop exceeded contract ({elapsed:?}): {bounded:?}"
            ));
        }
        let after: Vec<_> = files
            .iter()
            .map(|name| std::fs::read(home.join("mobile").join(name)).ok())
            .collect();
        if after != before {
            failures.push(format!("{lock_name}: cancelled wait changed durable bytes"));
        }
        assert!(
            DeviceRegistry::load(&home).is_ok(),
            "shutdown poisoned fresh registry for {lock_name}"
        );
    }
    assert!(failures.is_empty(), "{}", failures.join("; "));
}

/// A later contended startup store must not turn a failed admission into a
/// durable registry migration. The original registry bytes are the oracle.
#[test]
fn relay_startup_contention_preserves_legacy_registry_until_all_stores_admit() {
    use fs2::FileExt;
    let _gateway = super::tests::TEST_GATEWAY_LOCK.lock().unwrap();
    let mut failures = Vec::new();
    for lock_name in [".authorization-keys.lock", ".assurance-challenges.lock"] {
        let fixture = Fixture::new();
        let home = fixture.state.coven_home.clone();
        let relay = super::super::transport::test_support::Relay::start();
        storage::RelayConfig {
            listener_limit: storage::MAX_LISTENERS,
            version: 1,
            enabled: true,
            endpoints: vec![relay.configuration()],
        }
        .save(&home)
        .unwrap();
        let record = fixture
            .state
            .registry
            .device(fixture.pairing.device_id)
            .unwrap()
            .unwrap();
        let legacy = serde_json::json!({
            "version": 1,
            "devices": [{
                "id": record.id,
                "displayName": record.display_name,
                "publicKeyX963": record.public_key_x963,
                "pairedAt": record.paired_at,
                "revokedAt": null,
                "scopes": ["memory_read"]
            }]
        });
        let registry_path = home.join("mobile/devices.json");
        super::super::config::atomic_replace_private(
            &registry_path,
            &serde_json::to_vec(&legacy).unwrap(),
        )
        .unwrap();
        let before = std::fs::read(&registry_path).unwrap();
        let path = home.join("mobile").join(lock_name);
        let held = crate::state_lock::open_lock_file(&path).unwrap();
        held.lock_exclusive().unwrap();
        let (_observation, waiting) = super::super::store_lock::observe(&path);
        let worker_home = home.clone();
        let (finished, result) = std::sync::mpsc::sync_channel(1);
        let worker = std::thread::spawn(move || {
            let outcome = start_mobile_gateway_native_for_daemon(&worker_home).map(drop);
            let _ = finished.send(outcome);
        });
        let observed = waiting.recv_timeout(Duration::from_secs(30));
        let started = Instant::now();
        let outcome = result.recv_timeout(Duration::from_secs(2));
        let elapsed = started.elapsed();
        FileExt::unlock(&held).unwrap();
        worker.join().unwrap();
        assert!(
            observed.is_ok(),
            "no failed lock attempt at {lock_name}: {observed:?}"
        );
        assert!(
            matches!(outcome, Ok(Err(_))),
            "startup was not refused while {lock_name} stayed held after {elapsed:?}: {outcome:?}"
        );
        if std::fs::read(&registry_path).unwrap() != before {
            failures.push(format!(
                "{lock_name}: failed startup migrated durable registry bytes"
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("; "));
}

#[test]
fn runtime_scope_cancels_policy_passkey_and_legacy_provisioning_waits() {
    use super::super::{
        owner_policy::OwnerPolicyStore, passkey::PasskeyStore, store_lock::Runtime,
    };
    use fs2::FileExt;
    for lock_name in [
        ".introduction-policy.lock",
        ".passkeys.lock",
        ".transport.lock",
    ] {
        let fixture = Fixture::with_authorization();
        let home = fixture.state.coven_home.clone();
        let identity = super::super::identity::load_existing_host_identity(&home).unwrap();
        let signer = SigningKey::from_pkcs8_der(&identity.private_key_der).unwrap();
        let audience = identity.public_key_fingerprint;
        let device_id = fixture.pairing.device_id;
        let files = [
            "devices.json",
            "authorization-keys.json",
            "assurance-challenges.json",
            "introduction-policy.json",
            "passkeys.json",
            "audit.jsonl",
        ];
        let before: Vec<_> = files
            .iter()
            .map(|name| std::fs::read(home.join("mobile").join(name)).ok())
            .collect();
        let path = home.join("mobile").join(lock_name);
        let held = crate::state_lock::open_lock_file(&path).unwrap();
        held.lock_exclusive().unwrap();
        let (_observation, waiting) = super::super::store_lock::observe(&path);
        let runtime = Runtime::default();
        let worker_runtime = runtime.clone();
        let worker_home = home.clone();
        let state = fixture.state;
        let (finished, stopped) = std::sync::mpsc::sync_channel(1);
        let worker = std::thread::spawn(move || {
            let _scope = worker_runtime.enter();
            let result = match lock_name {
                ".introduction-policy.lock" => OwnerPolicyStore::open(&worker_home, audience)
                    .and_then(|store| store.with_current_locked(|_| Ok(()))),
                ".passkeys.lock" => PasskeyStore::open(&worker_home, audience)
                    .and_then(|store| store.list().map(drop)),
                ".transport.lock" => {
                    storage::load_or_provision(&worker_home, &state.registry, &signer, device_id)
                        .map(drop)
                }
                _ => unreachable!(),
            };
            let _ = finished.send(result.map_err(|error| format!("{error:#}")));
        });
        let observed = waiting.recv_timeout(Duration::from_secs(30));
        let started = Instant::now();
        runtime.cancel();
        let result = stopped.recv_timeout(Duration::from_secs(2));
        let elapsed = started.elapsed();
        FileExt::unlock(&held).unwrap();
        worker.join().unwrap();
        assert!(
            observed.is_ok(),
            "no failed file-lock attempt at {lock_name}: {observed:?}"
        );
        let error = result
            .unwrap_or_else(|error| {
                panic!("{lock_name} wait exceeded shutdown budget after {elapsed:?}: {error:?}")
            })
            .unwrap_err();
        assert!(
            error.contains("cancelled"),
            "wrong refusal at {lock_name}: {error}"
        );
        let after: Vec<_> = files
            .iter()
            .map(|name| std::fs::read(home.join("mobile").join(name)).ok())
            .collect();
        assert_eq!(
            after, before,
            "cancelled store wait changed bytes for {lock_name}"
        );
        assert!(
            DeviceRegistry::load(&home).is_ok(),
            "scope poisoned future registry at {lock_name}"
        );
    }
}

/// A held storage phase is a synthetic in-flight-I/O model, not a real disk
/// stall. It distinguishes cancelled admission from already admitted work.
#[test]
fn relay_shutdown_contains_an_inflight_challenge_write_before_sync() {
    use super::super::transport::{self, test_support::Relay, websocket};
    let _gateway = super::tests::TEST_GATEWAY_LOCK.lock().unwrap();
    let fixture = Fixture::with_authorization();
    let _retained = process::RetainedOwnerGuard;
    let home = fixture.state.coven_home.clone();
    let relay = Relay::start();
    storage::RelayConfig {
        listener_limit: storage::MAX_LISTENERS,
        version: 1,
        enabled: true,
        endpoints: vec![relay.configuration()],
    }
    .save(&home)
    .unwrap();
    let identity = super::super::identity::load_existing_host_identity(&home).unwrap();
    let signer = SigningKey::from_pkcs8_der(&identity.private_key_der).unwrap();
    let provisioning = storage::load_existing(
        &home,
        &fixture.state.registry,
        signer.verifying_key(),
        fixture.pairing.device_id,
    )
    .unwrap()
    .unwrap()
    .provisioning(&signer)
    .unwrap();
    let target = home.join("mobile/assurance-challenges.json");
    let ready = home.join("held-write-ready");
    let mut runtime = process::native_fixture::start_fixture(&home, Some(&target), None, &ready)
        .unwrap()
        .unwrap();
    let observation = runtime.observation();
    // This is only a room-arrival hang guard, independent of stop's strict budget.
    let connect_deadline = Instant::now() + Duration::from_secs(60);
    let connect = || loop {
        let slot = u64::try_from(Utc::now().timestamp()).unwrap() / storage::SLOT_SECONDS;
        let (room, credential) = provisioning.rendezvous(slot, false).unwrap();
        let attempt = relay
            .connect(websocket::Role::Client, &room, &credential)
            .and_then(|records| {
                transport::connect(
                    records,
                    &fixture.pairing,
                    &provisioning.binding_signature,
                    &fixture.device,
                    &room,
                    false,
                )
            });
        match attempt {
            Ok(channel) => break channel,
            Err(error) => {
                assert!(
                    Instant::now() < connect_deadline,
                    "relay arrival failed: {error:#}"
                );
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    };
    let mut channel = connect();
    let before = std::fs::read(&target).ok();
    let wire = fixture.request(
        "POST",
        "/api/v1/mobile/assurance/challenge",
        fixture.pairing.device_id,
    );
    channel
        .send_message(
            &wire,
            MAX_MOBILE_REQUEST_BYTES,
            Instant::now() + Duration::from_secs(30),
        )
        .unwrap();
    // The bridge only publishes after the CHILD enters the real pre-sync
    // checkpoint. Keep that child hold installed through forced termination.
    let phase_deadline = Instant::now() + Duration::from_secs(30);
    while !ready.exists() {
        assert!(!observation.exited(), "worker exited before the held write");
        assert!(
            Instant::now() < phase_deadline,
            "request never reached held child write"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(std::fs::read(&ready).unwrap(), b"held-written\n");
    let started = Instant::now();
    let result = runtime.shutdown_until(started + Duration::from_millis(1800));
    let elapsed = started.elapsed();
    assert!(
        result.is_ok() && elapsed < Duration::from_secs(2),
        "in-flight write outlived the daemon stop contract ({elapsed:?}): {result:?}"
    );
    assert!(
        observation.exited() && observation.reaped(),
        "worker exit/reap unobserved"
    );
    drop(runtime);
    drop(channel);
    assert_eq!(
        std::fs::read(&target).ok(),
        before,
        "a worker continued an uncommitted write after observed shutdown"
    );

    // Restart on the SAME profile. The interrupted staging file must never
    // become authority; a newly acknowledged challenge must survive shutdown.
    let mut restarted =
        process::native_fixture::start_fixture(&home, None, None, &home.join("restart-ready"))
            .unwrap()
            .unwrap();
    let mut channel = connect();
    let request = fixture.request(
        "POST",
        "/api/v1/mobile/assurance/challenge",
        fixture.pairing.device_id,
    );
    let deadline = Instant::now() + Duration::from_secs(30);
    channel
        .send_message(&request, MAX_MOBILE_REQUEST_BYTES, deadline)
        .unwrap();
    let response = channel
        .receive_message(MAX_MOBILE_RESPONSE_BYTES, deadline)
        .unwrap();
    assert!(
        response.starts_with(b"HTTP/1.1 200"),
        "restart request failed: {}",
        String::from_utf8_lossy(&response)
    );
    let acknowledged = std::fs::read(&target).unwrap();
    assert_ne!(Some(acknowledged.clone()), before);
    restarted.shutdown().unwrap();
    assert_eq!(
        std::fs::read(&target).unwrap(),
        acknowledged,
        "acknowledged write changed after shutdown"
    );
    assert!(
        AssuranceChallengeStore::load(&home).is_ok(),
        "restart left unreadable authority"
    );
}

#[test]
fn oversized_owner_control_header_does_not_terminate_mobile_authority() {
    let _gateway = super::tests::TEST_GATEWAY_LOCK.lock().unwrap();
    let fixture = Fixture::with_authorization();
    let _retained = process::RetainedOwnerGuard;
    let home = &fixture.state.coven_home;
    let relay = super::super::transport::test_support::Relay::start();
    storage::RelayConfig {
        listener_limit: storage::MAX_LISTENERS,
        version: 1,
        enabled: true,
        endpoints: vec![relay.configuration()],
    }
    .save(home)
    .unwrap();
    let mut runtime =
        process::native_fixture::start_fixture(home, None, None, &home.join("header-ready"))
            .unwrap()
            .unwrap();
    let observation = runtime.observation();
    let oversized = super::handle_local_control(
        "GET",
        &format!("{LOCAL_CONTROL_ROOT}/{}", "x".repeat(17 * 1024)),
        None,
    )
    .unwrap();
    let subsequent = super::handle_local_control("POST", LOCAL_CONNECTION_PATH, Some("{}"))
        .unwrap()
        .unwrap();
    assert_eq!(
        subsequent.status, 200,
        "oversized header disabled authority: {}",
        subsequent.body
    );
    assert!(
        !observation.exited(),
        "worker exited after rejecting oversized header"
    );
    assert!(
        oversized.is_err() || oversized.is_ok_and(|response| (400..500).contains(&response.status)),
        "oversized caller input was not rejected locally"
    );
    runtime.shutdown().unwrap();
    assert_eq!(
        observation.exit_code(),
        Some(0),
        "normal worker stop escalated to a forced exit"
    );
}

#[test]
fn process_startup_store_waits_exit_without_changing_authority() {
    use fs2::FileExt;
    let _gateway = super::tests::TEST_GATEWAY_LOCK.lock().unwrap();
    for lock_name in [
        ".devices.lock",
        ".authorization-keys.lock",
        ".assurance-challenges.lock",
    ] {
        let fixture = Fixture::with_authorization();
        let _retained = process::RetainedOwnerGuard;
        let home = fixture.state.coven_home.clone();
        let relay = super::super::transport::test_support::Relay::start();
        storage::RelayConfig {
            listener_limit: storage::MAX_LISTENERS,
            version: 1,
            enabled: true,
            endpoints: vec![relay.configuration()],
        }
        .save(&home)
        .unwrap();
        let files = [
            "devices.json",
            "authorization-keys.json",
            "assurance-challenges.json",
        ];
        let snapshot = || {
            files
                .iter()
                .map(|name| std::fs::read(home.join("mobile").join(name)).ok())
                .collect::<Vec<_>>()
        };
        let before = snapshot();
        let lock_path = home.join("mobile").join(lock_name);
        let held = crate::state_lock::open_lock_file(&lock_path).unwrap();
        held.lock_exclusive().unwrap();
        let ready = home.join("startup-lock-ready");
        let (send_observation, receive_observation) = std::sync::mpsc::sync_channel(1);
        let (send_result, receive_result) = std::sync::mpsc::sync_channel(1);
        let child_home = home.clone();
        let child_ready = ready.clone();
        let start = Instant::now();
        let starter = std::thread::spawn(move || {
            let outcome = process::native_fixture::start_fixture_observing(
                &child_home,
                None,
                Some(&lock_path),
                &child_ready,
                |handle| {
                    send_observation.send(handle.observation()).unwrap();
                },
            )
            .map(drop);
            let _ = send_result.send(outcome);
        });
        let observation = receive_observation
            .recv_timeout(Duration::from_secs(30))
            .unwrap();
        let hang_guard = Instant::now() + Duration::from_secs(30);
        while !ready.exists() && !observation.exited() && Instant::now() < hang_guard {
            std::thread::sleep(Duration::from_millis(5));
        }
        // Always join; keep the competing lock held THROUGH exit observation.
        let outcome = receive_result.recv_timeout(Duration::from_secs(30));
        starter.join().unwrap();
        assert_eq!(
            std::fs::read(&ready).unwrap(),
            b"failed-file-lock\n",
            "{lock_name}"
        );
        assert!(
            outcome.unwrap().is_err(),
            "startup unexpectedly admitted {lock_name}"
        );
        assert!(
            start.elapsed() < Duration::from_secs(2),
            "startup contention exceeded bound for {lock_name}: {:?}",
            start.elapsed()
        );
        assert!(
            observation.exited() && observation.reaped(),
            "startup child not reaped for {lock_name}"
        );
        assert_eq!(
            snapshot(),
            before,
            "failed startup changed authority for {lock_name}"
        );
        FileExt::unlock(&held).unwrap();
        let mut restarted = process::native_fixture::start_fixture(
            &home,
            None,
            None,
            &home.join("fresh-start-ready"),
        )
        .unwrap()
        .unwrap();
        restarted.shutdown().unwrap();
        assert_eq!(
            snapshot(),
            before,
            "fresh restart changed authority for {lock_name}"
        );
    }
}

#[test]
fn native_shutdown_recovers_initial_enrollment_between_key_and_device_commit() {
    let _gateway = super::tests::TEST_GATEWAY_LOCK.lock().unwrap();
    let fixture = Fixture::with_authorization();
    let _retained = process::RetainedOwnerGuard;
    let home = &fixture.state.coven_home;
    let relay = super::super::transport::test_support::Relay::start();
    storage::RelayConfig {
        listener_limit: storage::MAX_LISTENERS,
        version: 1,
        enabled: true,
        endpoints: vec![relay.configuration()],
    }
    .save(home)
    .unwrap();
    let devices = home.join("mobile/devices.json");
    let keys = home.join("mobile/authorization-keys.json");
    let before_devices = std::fs::read(&devices).unwrap();
    let before_keys = std::fs::read(&keys).unwrap();
    let ready = home.join("registration-held-ready");
    let prepared = home.join("registration-pending.json");
    let mut runtime =
        process::native_fixture::start_enrollment_fixture(home, &devices, &ready, &prepared)
            .unwrap()
            .unwrap();
    let observation = runtime.observation();
    let setup_deadline = Instant::now() + Duration::from_secs(30);
    while !prepared.exists() {
        assert!(!observation.exited(), "worker exited before pairing setup");
        assert!(
            Instant::now() < setup_deadline,
            "native pairing setup never completed"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    let pending: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&prepared).unwrap()).unwrap();
    let path = format!(
        "{LOCAL_CONTROL_ROOT}/{}/confirm",
        pending["id"].as_str().unwrap()
    );
    let body = serde_json::json!({ "phrase": pending["phrase"] }).to_string();
    let confirmation = std::thread::spawn(move || {
        super::handle_local_control("POST", &path, Some(&body)).unwrap()
    });
    let cut_deadline = Instant::now() + Duration::from_secs(30);
    while !ready.exists() {
        assert!(
            !observation.exited(),
            "worker exited before the registry write cut"
        );
        assert!(
            Instant::now() < cut_deadline,
            "registry write cut was not entered"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(std::fs::read(&ready).unwrap(), b"held-written\n");
    assert_eq!(
        std::fs::read(&devices).unwrap(),
        before_devices,
        "cut was after registry publication"
    );
    assert_ne!(
        std::fs::read(&keys).unwrap(),
        before_keys,
        "cut did not follow durable key insertion"
    );
    let started = Instant::now();
    let stopped = runtime.shutdown_until(started + Duration::from_millis(1800));
    assert!(
        stopped.is_ok() && started.elapsed() < Duration::from_secs(2),
        "registration stop exceeded contract after {:?}: {stopped:?}",
        started.elapsed()
    );
    assert!(
        observation.exited() && observation.reaped(),
        "registration worker was not observed exited/reaped"
    );
    let response = confirmation.join().unwrap().unwrap();
    assert_eq!(
        response.status, 503,
        "lost confirmation must report uncertainty"
    );
    drop(runtime);
    // Restart recovery is part of this assertion. The old parent Fixture
    // cache does not stand in for the fresh worker's durable-state load.
    let mut restarted = process::native_fixture::start_fixture(
        home,
        None,
        None,
        &home.join("registration-restart-ready"),
    )
    .unwrap()
    .unwrap();
    assert_eq!(
        std::fs::read(&devices).unwrap(),
        before_devices,
        "interrupted registration gained a device"
    );
    assert_eq!(
        std::fs::read(&keys).unwrap(),
        before_keys,
        "restart left an orphaned authorization key"
    );
    restarted.shutdown().unwrap();
}

/// These are synthetic holds at real child write checkpoints, selected by
/// actual owner-confirm execution. They are process-kill/restart evidence;
/// they do not model physical media stalls or establish power-loss durability.
#[test]
fn native_initial_enrollment_journal_cut_matrix_recovers_exact_images() {
    use super::super::config::MobileWritePhase;
    use process::native_fixture::{self, EnrollmentCut};
    let _gateway = super::tests::TEST_GATEWAY_LOCK.lock().unwrap();
    for (
        label,
        target_name,
        phase,
        skip,
        failed_commit,
        committed,
        keys_visible,
        journal_visible,
    ) in [
        (
            "keys-written",
            "authorization-keys.json",
            MobileWritePhase::Written,
            0,
            false,
            false,
            false,
            true,
        ),
        (
            "keys-renamed",
            "authorization-keys.json",
            MobileWritePhase::Renamed,
            0,
            false,
            false,
            true,
            true,
        ),
        (
            "devices-renamed",
            "devices.json",
            MobileWritePhase::Renamed,
            0,
            false,
            true,
            true,
            true,
        ),
        (
            "journal-written",
            ".initial-enrollment.json",
            MobileWritePhase::Written,
            0,
            false,
            false,
            false,
            false,
        ),
        (
            "journal-retired",
            ".initial-enrollment.json",
            MobileWritePhase::RetainedParentSynced,
            0,
            false,
            true,
            true,
            false,
        ),
        // The first matching key Written belongs to enrollment. The second
        // belongs to actual compensation after the real registry write hook
        // fails. No authority images are fabricated by the parent test.
        (
            "rollback-keys-written",
            "authorization-keys.json",
            MobileWritePhase::Written,
            1,
            true,
            false,
            true,
            true,
        ),
    ] {
        let fixture = Fixture::with_authorization();
        let _retained = process::RetainedOwnerGuard;
        let home = &fixture.state.coven_home;
        let relay = super::super::transport::test_support::Relay::start();
        storage::RelayConfig {
            listener_limit: storage::MAX_LISTENERS,
            version: 1,
            enabled: true,
            endpoints: vec![relay.configuration()],
        }
        .save(home)
        .unwrap();
        let devices = home.join("mobile/devices.json");
        let keys = home.join("mobile/authorization-keys.json");
        let journal = home.join("mobile/.initial-enrollment.json");
        let before_devices = std::fs::read(&devices).unwrap();
        let before_keys = std::fs::read(&keys).unwrap();
        assert!(
            !journal.exists(),
            "fixture already has an enrollment intent: {label}"
        );
        let target = home.join("mobile").join(target_name);
        let cut = EnrollmentCut {
            target: &target,
            phase,
            skip_matching: skip,
            fail_registry_commit: failed_commit,
        };
        let ready = home.join(format!("{label}-held-ready"));
        let prepared = home.join(format!("{label}-pending.json"));
        let mut runtime =
            native_fixture::start_enrollment_cut_fixture(home, cut, &ready, &prepared)
                .unwrap()
                .unwrap();
        let observation = runtime.observation();
        let setup_deadline = Instant::now() + Duration::from_secs(30);
        while !prepared.exists() && !observation.exited() && Instant::now() < setup_deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        if !prepared.exists() {
            let stopped = runtime.shutdown();
            panic!(
                "{label}: native pairing setup did not publish; exited={} shutdown={stopped:?}",
                observation.exited()
            );
        }
        let pending: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&prepared).unwrap()).unwrap();
        assert_eq!(
            pending["registryWriteFailureInjected"], failed_commit,
            "fixture failed to select actual registry error: {label}"
        );
        let path = format!(
            "{LOCAL_CONTROL_ROOT}/{}/confirm",
            pending["id"].as_str().unwrap()
        );
        let body = serde_json::json!({ "phrase": pending["phrase"] }).to_string();
        let confirmation = std::thread::spawn(move || {
            super::handle_local_control("POST", &path, Some(&body)).unwrap()
        });
        let phase_deadline = Instant::now() + Duration::from_secs(30);
        while !ready.exists() && !observation.exited() && Instant::now() < phase_deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        let observed_marker = std::fs::read(&ready).ok();
        let held_devices = std::fs::read(&devices).ok();
        let held_keys = std::fs::read(&keys).ok();
        let held_journal = std::fs::read(&journal).ok();
        // Cleanup and join precede phase/state assertions, including a red
        // regression. The parent never releases the child's held checkpoint.
        let started = Instant::now();
        let stopped = runtime.shutdown_until(started + Duration::from_millis(1800));
        let elapsed = started.elapsed();
        let confirmation = confirmation.join().unwrap();
        drop(runtime);
        assert_eq!(
            observed_marker,
            Some(cut.marker()),
            "{label}: no matching held child checkpoint"
        );
        assert!(
            stopped.is_ok() && elapsed < Duration::from_secs(2),
            "{label}: native cut shutdown exceeded contract after {elapsed:?}: {stopped:?}"
        );
        assert!(
            observation.exited() && observation.reaped(),
            "{label}: child exit/reap not observed"
        );
        assert_ne!(
            observation.exit_code(),
            Some(0),
            "{label}: selected hold was released or bypassed instead of contained"
        );
        let response = confirmation.unwrap();
        assert_eq!(
            response.status, 503,
            "{label}: unacknowledged confirmation did not report uncertainty"
        );
        let uncertainty: serde_json::Value = serde_json::from_str(&response.body).unwrap();
        assert_eq!(
            uncertainty["error"]["code"], "daemon_unavailable",
            "{label}"
        );
        assert_eq!(
            uncertainty["error"]["details"]["outcome"], "unknown",
            "{label}"
        );
        assert_eq!(
            uncertainty["error"]["details"]["retrySafe"], false,
            "{label}"
        );
        assert_eq!(
            held_journal.is_some(),
            journal_visible,
            "{label}: incorrect journal cut"
        );
        let held_devices = held_devices.unwrap();
        let held_keys = held_keys.unwrap();
        if committed {
            assert_ne!(
                held_devices, before_devices,
                "{label}: device never committed"
            );
        } else {
            assert_eq!(
                held_devices, before_devices,
                "{label}: device committed before the selected cut"
            );
        }
        if keys_visible {
            assert_ne!(
                held_keys, before_keys,
                "{label}: enrolled keys not published at cut"
            );
        } else {
            assert_eq!(
                held_keys, before_keys,
                "{label}: keys published before the selected cut"
            );
        }
        // Capture exact expected committed bytes only after proving that this
        // real operation added one linked device/key to the prior profile.
        if committed {
            let before: serde_json::Value = serde_json::from_slice(&before_devices).unwrap();
            let after: serde_json::Value = serde_json::from_slice(&held_devices).unwrap();
            let prior_keys: serde_json::Value = serde_json::from_slice(&before_keys).unwrap();
            let next_keys: serde_json::Value = serde_json::from_slice(&held_keys).unwrap();
            let prior_devices = before["devices"].as_array().unwrap();
            let next_devices = after["devices"].as_array().unwrap();
            let prior_keys = prior_keys["keys"].as_array().unwrap();
            let next_keys = next_keys["keys"].as_array().unwrap();
            assert_eq!(next_devices.len(), prior_devices.len() + 1, "{label}");
            assert_eq!(
                &next_devices[..prior_devices.len()],
                prior_devices,
                "{label}: changed an existing device"
            );
            assert_eq!(next_keys.len(), prior_keys.len() + 1, "{label}");
            assert_eq!(
                &next_keys[..prior_keys.len()],
                prior_keys,
                "{label}: changed an existing authorization key"
            );
            assert_eq!(
                next_devices.last().unwrap()["device"]["id"],
                next_keys.last().unwrap()["deviceId"],
                "{label}: committed device/key identities differ"
            );
        }
        let expected_devices = if committed {
            held_devices
        } else {
            before_devices
        };
        let expected_keys = if committed { held_keys } else { before_keys };
        // SAME profile, fresh real worker. The parent's old Fixture cache is
        // never used as evidence for the recovered authority state.
        let mut restarted = native_fixture::start_fixture(
            home,
            None,
            None,
            &home.join(format!("{label}-restart-ready")),
        )
        .unwrap()
        .unwrap();
        let status = super::handle_local_control("POST", LOCAL_CONNECTION_PATH, Some("{}"))
            .unwrap()
            .unwrap();
        assert_eq!(
            status.status, 200,
            "{label}: restarted control plane unavailable"
        );
        assert_eq!(
            std::fs::read(&devices).unwrap(),
            expected_devices,
            "{label}: restart selected the wrong device image"
        );
        assert_eq!(
            std::fs::read(&keys).unwrap(),
            expected_keys,
            "{label}: restart selected the wrong key image"
        );
        assert!(
            !journal.exists(),
            "{label}: restart left enrollment recovery pending"
        );
        restarted.shutdown().unwrap();
        assert_eq!(
            std::fs::read(&devices).unwrap(),
            expected_devices,
            "{label}: device image changed after recovered shutdown"
        );
        assert_eq!(
            std::fs::read(&keys).unwrap(),
            expected_keys,
            "{label}: key image changed after recovered shutdown"
        );
        assert!(
            DeviceRegistry::load(home).is_ok(),
            "{label}: recovered registry is unreadable"
        );
    }
}

mod native_v2_create_process_tests;
