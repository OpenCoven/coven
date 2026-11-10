//! Current v2 routing, production relay workers and direct TLS ingress under handoff.
use super::*;
use crate::mobile_memory::transport::{
    cancellation::Cancellation,
    connection::{ConnectionState, Transport},
    migration::Migration,
    routing,
    storage::v2,
    test_support::Relay,
    websocket,
};

#[test]
fn v2_migration_preserves_one_operation_and_durable_counters_across_transports() {
    let Fixture {
        _home,
        state,
        device,
        pairing: legacy,
    } = Fixture::with_authorization();
    let state = Arc::new(state);
    let identity =
        crate::mobile_memory::identity::load_existing_host_identity(&state.coven_home).unwrap();
    let host = Arc::new(SigningKey::from_pkcs8_der(&identity.private_key_der).unwrap());
    let id = legacy.device_id;
    let legacy_binding = legacy.sign_noise_binding(&host).unwrap();
    let handover = v2::prepare_direct(&state.coven_home, &state.registry, &host, id).unwrap();
    let pairing = handover
        .verify(host.verifying_key(), id, device.verifying_key())
        .unwrap();
    let binding = handover.binding_signature.clone();
    let device_home = tempfile::tempdir().unwrap();
    let ack = v2::device::adopt_direct(
        device_home.path(),
        handover,
        host.verifying_key(),
        id,
        device.verifying_key(),
    )
    .unwrap();
    v2::acknowledge_direct(&state.coven_home, &state.registry, &host, id, &ack).unwrap();
    let before = state.registry.authorization_record(id).unwrap().unwrap();
    let relay = Relay::start_literal();
    storage::RelayConfig {
        listener_limit: 4,
        version: 1,
        enabled: true,
        endpoints: vec![relay.configuration()],
    }
    .save(&state.coven_home)
    .unwrap();
    let runtime =
        super::super::relay_runtime::RelayHandle::start(Arc::clone(&state), Arc::clone(&host))
            .unwrap();
    let tls = tls_server_config(
        identity.certificate_der.clone(),
        identity.private_key_der.to_vec(),
    )
    .unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let endpoint = websocket::EndpointConfig {
        url: format!("wss://192.0.2.1:{}/ws", address.port()),
        ca_certificates: vec![URL_SAFE_NO_PAD.encode(&identity.certificate_der)],
    }
    .parse()
    .unwrap();
    std::thread::scope(|scope| {
        // Drop on unwind before scoped joins, so an unexpected failure closes
        // channels instead of leaving an idle server waiting for more requests.
        let migration = Migration::new(pairing, binding, device.clone()).unwrap();
        let connect_relay = || {
            let deadline = Instant::now() + Duration::from_secs(60);
            let mut tries = 0;
            loop {
                tries += 1;
                let routed = migration
                    .begin(Transport::Relay)
                    .unwrap()
                    .routed(device_home.path())
                    .unwrap();
                let (room, credential) = routed.rendezvous().unwrap();
                match relay
                    .connect(websocket::Role::Client, &room, &credential)
                    .and_then(|records| routed.authenticate(records))
                {
                    Ok(candidate) => break candidate,
                    Err(error) => {
                        eprintln!("v2 migration relay candidate {tries}: {error}");
                        assert!(
                            Instant::now() < deadline && tries < 100,
                            "v2 relay never became ready: {error}"
                        );
                    }
                }
            }
        };
        migration.commit(connect_relay()).unwrap();
        let pending = migration
            .prepare("POST", "/api/v1/mobile/assurance/challenge", &[])
            .unwrap();
        let operation_id = pending.id();
        assert!(migration
            .exchange(&pending)
            .unwrap()
            .bytes
            .starts_with(b"HTTP/1.1 200"));
        let old_pin = Migration::new(legacy, legacy_binding, device.clone()).unwrap();
        assert_eq!(
            old_pin
                .begin(Transport::Direct)
                .unwrap()
                .routed(device_home.path())
                .err()
                .unwrap()
                .to_string(),
            "routing pairing differs from pinned migration identity"
        );
        let routed = migration
            .begin(Transport::Direct)
            .unwrap()
            .routed(device_home.path())
            .unwrap();
        let (room, credential) = routed.rendezvous().unwrap();
        let state_ref = &state;
        let direct = scope.spawn(move || {
            let (socket, _) = listener.accept().unwrap();
            serve_tls_connection(socket, tls, state_ref)
        });
        let records = websocket::connect_cancellable(
            &endpoint,
            address,
            websocket::Role::Client,
            &room,
            &credential,
            Instant::now() + Duration::from_secs(30),
            &Cancellation::default(),
        )
        .unwrap();
        let authenticated = routed.authenticate(records).unwrap();
        let old_read = migration
            .prepare("GET", "/api/v1/mobile/device", &[])
            .unwrap();
        assert!(migration
            .exchange(&old_read)
            .unwrap()
            .bytes
            .starts_with(b"HTTP/1.1 200"));
        migration.commit(authenticated).unwrap();
        assert_eq!(
            migration.status().unwrap().unwrap().transport,
            Transport::Direct
        );
        assert!(migration
            .exchange(&pending)
            .unwrap()
            .bytes
            .starts_with(b"HTTP/1.1 409"));
        assert_eq!(
            state.connections.snapshot(Transport::Direct).unwrap().state,
            ConnectionState::Online
        );
        let authenticated = connect_relay();
        let direct_read = migration
            .prepare("GET", "/api/v1/mobile/device", &[])
            .unwrap();
        assert!(migration
            .exchange(&direct_read)
            .unwrap()
            .bytes
            .starts_with(b"HTTP/1.1 200"));
        migration.commit(authenticated).unwrap();
        assert_eq!(
            migration.status().unwrap().unwrap().transport,
            Transport::Relay
        );
        assert!(migration
            .exchange(&pending)
            .unwrap()
            .bytes
            .starts_with(b"HTTP/1.1 409"));
        let final_read = migration
            .prepare("GET", "/api/v1/mobile/device", &[])
            .unwrap();
        assert!(migration
            .exchange(&final_read)
            .unwrap()
            .bytes
            .starts_with(b"HTTP/1.1 200"));
        assert_eq!(pending.id(), operation_id);
        assert_eq!(
            state.connections.snapshot(Transport::Relay).unwrap().state,
            ConnectionState::Online
        );
        state.stop_network_connections();
        let _ = direct.join().unwrap();
    });
    drop(runtime);
    let registry = DeviceRegistry::load(&state.coven_home).unwrap();
    assert_eq!(before, registry.authorization_record(id).unwrap().unwrap());
    let snapshot = v2::load(
        &state.coven_home,
        &registry,
        host.verifying_key(),
        id,
        routing::Transport::Direct,
    )
    .unwrap()
    .unwrap();
    assert_eq!(snapshot.floors, [1, 2]);
    let challenges: serde_json::Value = serde_json::from_slice(
        &std::fs::read(
            state
                .coven_home
                .join("mobile")
                .join(crate::mobile_memory::assurance::ASSURANCE_CHALLENGES_FILE),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(challenges["challenges"].as_array().unwrap().len(), 1);
    assert_eq!(
        state.connections.snapshot(Transport::Direct).unwrap().state,
        ConnectionState::Offline
    );
    assert_eq!(
        state.connections.snapshot(Transport::Relay).unwrap().state,
        ConnectionState::Offline
    );
}
