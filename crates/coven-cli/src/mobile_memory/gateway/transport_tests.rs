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
            URL_SAFE_NO_PAD.encode(device.verifying_key().to_encoded_point(false).as_bytes());
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
