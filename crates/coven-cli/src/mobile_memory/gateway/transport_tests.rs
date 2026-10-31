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

struct Fixture {
    _home: tempfile::TempDir,
    state: MobileGatewayState,
    device: SigningKey,
    pairing: Pairing,
}
impl Fixture {
    fn new() -> Self {
        let home = tempfile::tempdir().unwrap();
        Self::with_home(home)
    }
    fn with_home(home: tempfile::TempDir) -> Self {
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
        registry.register_with_grant(record, grant).unwrap();
        let pairing = storage::load_or_provision(home.path(), &registry, &host, id)
            .unwrap()
            .pairing;
        let state = MobileGatewayState {
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
        use ring::rand::SecureRandom;
        let timestamp = Utc::now().timestamp();
        let mut request_nonce = [0_u8; 32];
        ring::rand::SystemRandom::new()
            .fill(&mut request_nonce)
            .unwrap();
        let nonce = URL_SAFE_NO_PAD.encode(request_nonce);
        let digest = URL_SAFE_NO_PAD.encode(Sha256::digest([]));
        let canonical =
            crate::mobile_memory::auth::canonical_request(method, path, timestamp, &nonce, &digest)
                .unwrap();
        let signature: Signature = self.device.sign(&canonical);
        format!("{method} {path} HTTP/1.1\r\nx-coven-protocol: 1\r\nx-coven-device: {id}\r\nx-coven-timestamp: {timestamp}\r\nx-coven-nonce: {nonce}\r\nx-coven-body-sha256: {digest}\r\nx-coven-signature: {}\r\ncontent-length: 0\r\n\r\n", URL_SAFE_NO_PAD.encode(signature.to_der().as_bytes())).into_bytes()
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
    // Startup/store initialization is outside the strict stop budget. This is
    // only a hang guard; process exit reports early failure while retrying.
    let startup_deadline = Instant::now() + Duration::from_secs(120);
    let mut stalled_socket = None;
    let mut channel = if let Some(listener) = stalled_listener {
        use std::io::Read;
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
        socket
            .set_read_timeout(Some(Duration::from_secs(30)))
            .unwrap();
        let mut record_type = [0_u8];
        socket.read_exact(&mut record_type).unwrap();
        assert_eq!(record_type[0], 22, "expected TLS handshake record");
        // Receiving ClientHello proves the worker reached the real TLS path.
        // Send nothing back: shutdown must interrupt the pending handshake.
        stalled_socket = Some(socket);
        None
    } else {
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
        handle_protected_route(&fixture.state, MobileRoute::DeviceRead, path, request).status,
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
    let runtime = start_mobile_gateway_for_daemon(&fixture.state.coven_home)
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
        let runtime = start_mobile_gateway_for_daemon(&fixture.state.coven_home)
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
            // the unchanged fifteen-second idle request deadline. A timeout
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
    let runtime = start_mobile_gateway_for_daemon(&fixture.state.coven_home)
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
    let runtime = start_mobile_gateway_for_daemon(&fixture.state.coven_home)
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
