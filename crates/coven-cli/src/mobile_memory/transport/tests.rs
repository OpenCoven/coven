use super::*;
use std::sync::{mpsc, Arc, Mutex};
use std::thread;

mod real_relay;

#[test]
fn signed_preimages_match_independent_cbor_vectors() {
    let vectors: serde_json::Value =
        serde_json::from_str(include_str!("vectors/wire.json")).unwrap();
    let hex = |value: &[u8]| {
        value
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    };
    let synthetic_identity = |scalar: u8| {
        let mut bytes = [0_u8; 32];
        bytes[31] = scalar;
        SigningKey::from_slice(&bytes).unwrap()
    };
    let mut pairing = Pairing {
        device_id: Uuid::from_u128(1),
        grant_id: Uuid::from_u128(2),
        revocation_epoch: 0,
        host_key: *synthetic_identity(1).verifying_key(),
        device_key: *synthetic_identity(2).verifying_key(),
        noise_key: std::array::from_fn(|index| index as u8),
    };
    assert_eq!(
        hex(pairing.host_key.to_sec1_point(false).as_bytes()),
        vectors["hostPublic"].as_str().unwrap()
    );
    assert_eq!(
        hex(pairing.device_key.to_sec1_point(false).as_bytes()),
        vectors["devicePublic"].as_str().unwrap()
    );
    for case in vectors["bindingAndProofs"].as_array().unwrap() {
        pairing.revocation_epoch = case["epoch"].as_u64().unwrap();
        assert_eq!(
            hex(&pairing.binding_bytes().unwrap()),
            case["binding"].as_str().unwrap()
        );
        for (role, name) in [("host", "hostProof"), ("device", "deviceProof")] {
            assert_eq!(
                hex(&pairing.proof(role, &[3; 32], &[4; 32], &[5; 32]).unwrap()),
                case[name].as_str().unwrap()
            );
        }
    }
    let context = std::array::from_fn(|index| (index + 32) as u8);
    for (direct, name) in [(true, "direct"), (false, "relay")] {
        assert_eq!(
            hex(&prologue(&context, direct).unwrap()),
            vectors["prologues"][name].as_str().unwrap()
        );
    }
}

type Capture = Arc<Mutex<Vec<Vec<u8>>>>;
struct Pipe {
    sender: mpsc::Sender<Vec<u8>>,
    receiver: mpsc::Receiver<Vec<u8>>,
    capture: Capture,
}
impl Records for Pipe {
    fn send(&mut self, bytes: &[u8], deadline: Instant) -> Result<()> {
        ensure!(Instant::now() < deadline, "deadline expired");
        self.capture.lock().unwrap().push(bytes.to_vec());
        self.sender.send(bytes.to_vec())?;
        Ok(())
    }
    fn receive(&mut self, deadline: Instant) -> Result<Vec<u8>> {
        Ok(self
            .receiver
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))?)
    }
}
fn pipes() -> (Pipe, Pipe, Capture) {
    let (a_tx, a_rx) = mpsc::channel();
    let (b_tx, b_rx) = mpsc::channel();
    let capture = Arc::new(Mutex::new(Vec::new()));
    (
        Pipe {
            sender: a_tx,
            receiver: b_rx,
            capture: capture.clone(),
        },
        Pipe {
            sender: b_tx,
            receiver: a_rx,
            capture: capture.clone(),
        },
        capture,
    )
}
fn identity(seed: u8) -> SigningKey {
    SigningKey::from_slice(&[seed; 32]).unwrap()
}
fn pairing(noise_key: [u8; 32]) -> Pairing {
    Pairing {
        device_id: Uuid::from_u128(1),
        grant_id: Uuid::from_u128(2),
        revocation_epoch: 3,
        host_key: *identity(1).verifying_key(),
        device_key: *identity(2).verifying_key(),
        noise_key,
    }
}

#[test]
fn fresh_connections_authenticate_without_enrollment_and_hide_plaintext() {
    let (private, public) = generate_noise_key().unwrap();
    let mut transcripts = Vec::new();
    for _ in 0..2 {
        let (client, server, capture) = pipes();
        let host_pairing = pairing(public);
        let host_private = private.clone();
        let host = thread::spawn(move || {
            let mut checks = 0;
            let mut channel = accept(
                server,
                &host_pairing,
                &host_private,
                &identity(1),
                &[9; 32],
                false,
                |_, _| {
                    checks += 1;
                    Ok(())
                },
            )
            .unwrap();
            assert_eq!(checks, 3);
            let deadline = Instant::now() + Duration::from_secs(30);
            assert_eq!(
                channel.receive(deadline).unwrap().as_slice(),
                b"private-request-marker"
            );
            channel.send(b"private-response-marker", deadline).unwrap();
        });
        let client_pairing = pairing(public);
        let binding = client_pairing.sign_noise_binding(&identity(1)).unwrap();
        let mut channel = connect(
            client,
            &client_pairing,
            &binding,
            &identity(2),
            &[9; 32],
            false,
        )
        .unwrap();
        assert_eq!(channel.device_id(), client_pairing.device_id);
        let deadline = Instant::now() + Duration::from_secs(30);
        channel.send(b"private-request-marker", deadline).unwrap();
        assert_eq!(
            channel.receive(deadline).unwrap().as_slice(),
            b"private-response-marker"
        );
        host.join().unwrap();
        let frames = capture.lock().unwrap();
        for frame in frames.iter() {
            assert!(!frame
                .windows(b"private-request-marker".len())
                .any(|s| s == b"private-request-marker"));
            assert!(!frame
                .windows(b"private-response-marker".len())
                .any(|s| s == b"private-response-marker"));
        }
        transcripts.push(frames[0].clone());
    }
    assert_ne!(transcripts[0], transcripts[1]);
}

#[test]
fn copied_binding_cannot_authorize_a_different_grant_or_device() {
    let (_, public) = generate_noise_key().unwrap();
    let mut trusted = pairing(public);
    let binding = trusted.sign_noise_binding(&identity(1)).unwrap();
    trusted.revocation_epoch += 1;
    assert!(trusted.verify_noise_binding(&binding).is_err());
    trusted.revocation_epoch -= 1;
    trusted.device_key = *identity(3).verifying_key();
    assert!(trusted.verify_noise_binding(&binding).is_err());
}

#[test]
fn changed_transport_or_room_aborts_before_authentication() {
    for direct in [false, true] {
        let (private, public) = generate_noise_key().unwrap();
        let (client, server, _) = pipes();
        let host = thread::spawn(move || {
            accept(
                server,
                &pairing(public),
                &private,
                &identity(1),
                &[9; 32],
                false,
                |_, _| Ok(()),
            )
            .is_err()
        });
        let trusted = pairing(public);
        let binding = trusted.sign_noise_binding(&identity(1)).unwrap();
        let context = if direct { [9; 32] } else { [8; 32] };
        assert!(connect(client, &trusted, &binding, &identity(2), &context, direct).is_err());
        assert!(host.join().unwrap());
    }
}

#[test]
fn revocation_after_possession_proof_prevents_completion() {
    let (private, public) = generate_noise_key().unwrap();
    let (client, server, _) = pipes();
    let host = thread::spawn(move || {
        let mut checks = 0;
        accept(
            server,
            &pairing(public),
            &private,
            &identity(1),
            &[9; 32],
            false,
            |_, _| {
                checks += 1;
                ensure!(checks == 1, "revoked");
                Ok(())
            },
        )
        .is_err()
    });
    let trusted = pairing(public);
    let binding = trusted.sign_noise_binding(&identity(1)).unwrap();
    assert!(connect(client, &trusted, &binding, &identity(2), &[9; 32], false).is_err());
    assert!(host.join().unwrap());
}

#[test]
fn reconnect_admission_reads_durable_revocation_and_transport_policy() {
    use crate::mobile_memory::{
        grant::{DeviceGrant, DeviceScope, GrantTransportConstraint},
        registry::{DeviceRecord, DeviceRegistry},
    };
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
    use chrono::Utc;
    let home = tempfile::tempdir().unwrap();
    let registry = DeviceRegistry::load(home.path()).unwrap();
    let external_admin = DeviceRegistry::load(home.path()).unwrap();
    let (_, public) = generate_noise_key().unwrap();
    let mut trusted = pairing(public);
    let now = Utc::now();
    let key = URL_SAFE_NO_PAD.encode(trusted.device_key.to_sec1_point(false).as_bytes());
    let device = DeviceRecord {
        id: trusted.device_id,
        display_name: "fixture".into(),
        public_key_x963: key.clone(),
        paired_at: now,
        revoked_at: None,
        suspended_at: None,
        scopes: vec![DeviceScope::MemoryRead],
    };
    let mut grant = DeviceGrant::for_device(device.id, &key, device.scopes.clone(), now).unwrap();
    grant.restrictions.transport = GrantTransportConstraint::DirectOnly;
    grant.expires_at = Some(now + chrono::Duration::hours(1));
    trusted.grant_id = grant.id;
    trusted.revocation_epoch = grant.revocation_epoch;
    registry.register_with_grant(device, grant).unwrap();
    assert!(authority::check_current(&registry, &trusted, true, now).is_ok());
    assert!(authority::check_current(&registry, &trusted, false, now).is_err());
    assert!(
        authority::check_current(&registry, &trusted, true, now + chrono::Duration::hours(1))
            .is_err()
    );
    external_admin.suspend(trusted.device_id, now).unwrap();
    assert!(authority::check_current(&registry, &trusted, true, now).is_err());
    external_admin.resume(trusted.device_id).unwrap();
    assert!(authority::check_current(&registry, &trusted, true, now).is_ok());
    external_admin.revoke(trusted.device_id, now).unwrap();
    assert!(authority::check_current(&registry, &trusted, true, now).is_err());
}

#[derive(Clone, Copy, Debug)]
enum WireAttack {
    None,
    Tamper,
    Replay,
    Drop,
    Oversize,
    Reflect,
    Truncate,
    Inject,
    Reorder,
    Downgrade,
}

/// A malicious broker with actual HTTP upgrades and WebSocket framing. Its
/// deterministic direction script mirrors handshake/application exchanges so
/// no timing assertion or polling loop is needed to choose an attack point.
fn real_wire(attack: WireAttack) {
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicBool, Ordering};
    use tungstenite::Message;
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let (ready_tx, ready_rx) = mpsc::channel();
    let relay = thread::spawn(move || {
        let accept = || {
            let (stream, _) = listener.accept().unwrap();
            // Hang guard only; producer closure also announces failure through
            // socket close. No production deadline is widened by this fixture.
            stream
                .set_read_timeout(Some(Duration::from_secs(30)))
                .unwrap();
            stream
                .set_write_timeout(Some(Duration::from_secs(30)))
                .unwrap();
            tungstenite::accept(stream).unwrap()
        };
        let mut host = accept();
        ready_tx.send(()).unwrap();
        let mut client = accept();
        let mut captured: Vec<Vec<u8>> = Vec::new();
        for (index, from_client) in [
            true, false, false, true, false, true, false, true, true, false,
        ]
        .into_iter()
        .enumerate()
        {
            let (source, destination) = if from_client {
                (&mut client, &mut host)
            } else {
                (&mut host, &mut client)
            };
            let Ok(Message::Binary(frame)) = source.read() else {
                break;
            };
            let mut frame = frame.to_vec();
            captured.push(frame.clone());
            match attack {
                WireAttack::Drop if index == 0 => break,
                WireAttack::Tamper if index == 1 => {
                    let last = frame.len() - 1;
                    frame[last] ^= 1;
                }
                WireAttack::Replay if index == 5 => frame = captured[3].clone(),
                WireAttack::Oversize if index == 1 => frame = vec![0; MAX_RECORD_BYTES + 1],
                WireAttack::Reflect if index == 1 => frame = captured[0].clone(),
                WireAttack::Truncate if index == 1 => {
                    frame.pop();
                }
                WireAttack::Inject if index == 1 => frame.fill(0),
                WireAttack::Downgrade if index == 1 => {
                    frame = b"HTTP/1.1 200 OK\r\n\r\nplaintext-fallback".to_vec();
                }
                WireAttack::Reorder if index == 1 => {
                    // The host writes its Noise response and encrypted nonce
                    // consecutively. Swap those actual records, without sleeps
                    // or fabricating a second message to stand in for reorder.
                    let Message::Binary(next) = source.read().unwrap() else {
                        panic!("expected host nonce record");
                    };
                    captured.push(next.to_vec());
                    let _ = destination.send(Message::Binary(next));
                    let _ = destination.send(Message::Binary(frame.into()));
                    break;
                }
                _ => {}
            }
            if destination.send(Message::Binary(frame.into())).is_err() {
                break;
            }
        }
        captured
    });
    let host_admitted = Arc::new(AtomicBool::new(false));
    let host_admission = Arc::clone(&host_admitted);
    let (private, public) = generate_noise_key().unwrap();
    let host = thread::spawn(move || -> Result<()> {
        let records = websocket::fixture_connect(address)?;
        let mut channel = accept(
            records,
            &pairing(public),
            &private,
            &identity(1),
            &[7; 32],
            false,
            |_, _| Ok(()),
        )?;
        host_admission.store(true, Ordering::Release);
        let deadline = Instant::now() + Duration::from_secs(30);
        ensure!(
            channel.receive(deadline)?.as_slice() == b"wire-request-private-marker",
            "request changed"
        );
        channel.send(b"wire-response-private-marker", deadline)
    });
    ready_rx.recv_timeout(Duration::from_secs(30)).unwrap();
    let mut client_admitted = false;
    let client = (|| -> Result<()> {
        let trusted = pairing(public);
        let binding = trusted.sign_noise_binding(&identity(1))?;
        let records = websocket::fixture_connect(address)?;
        let mut channel = connect(records, &trusted, &binding, &identity(2), &[7; 32], false)?;
        client_admitted = true;
        let deadline = Instant::now() + Duration::from_secs(30);
        channel.send(b"wire-request-private-marker", deadline)?;
        ensure!(
            channel.receive(deadline)?.as_slice() == b"wire-response-private-marker",
            "response changed"
        );
        Ok(())
    })();
    let host = host.join().unwrap();
    let frames = relay.join().unwrap();
    if matches!(attack, WireAttack::None) {
        assert!(client.is_ok(), "client: {client:?}");
        assert!(host.is_ok(), "host: {host:?}");
        assert!(client_admitted && host_admitted.load(Ordering::Acquire));
        assert_eq!(frames.len(), 10);
    } else {
        assert!(
            client.is_err() && host.is_err(),
            "attack {attack:?} did not fail closed"
        );
        assert!(
            !client_admitted && !host_admitted.load(Ordering::Acquire),
            "attack {attack:?} admitted an endpoint before disconnect"
        );
    }
    for frame in frames {
        for marker in [
            b"wire-request-private-marker".as_slice(),
            b"wire-response-private-marker".as_slice(),
        ] {
            assert!(!frame.windows(marker.len()).any(|part| part == marker));
        }
    }
}

#[test]
fn websocket_relay_preserves_authenticated_encrypted_exchange() {
    real_wire(WireAttack::None);
}

#[test]
fn malicious_websocket_relay_cannot_tamper_replay_drop_or_oversize() {
    for attack in [
        WireAttack::Tamper,
        WireAttack::Replay,
        WireAttack::Drop,
        WireAttack::Oversize,
    ] {
        real_wire(attack);
    }
}

#[test]
fn malicious_websocket_relay_cannot_reflect_truncate_inject_reorder_or_downgrade() {
    for attack in [
        WireAttack::Reflect,
        WireAttack::Truncate,
        WireAttack::Inject,
        WireAttack::Reorder,
        WireAttack::Downgrade,
    ] {
        real_wire(attack);
    }
}

#[test]
fn bounded_messages_cross_record_boundaries_without_changing_content() {
    let (private, public) = generate_noise_key().unwrap();
    let (client, server, _) = pipes();
    let payload = vec![0x5a; 4 * 1024 * 1024];
    let expected = payload.clone();
    let host = thread::spawn(move || {
        let mut channel = accept(
            server,
            &pairing(public),
            &private,
            &identity(1),
            &[9; 32],
            false,
            |_, _| Ok(()),
        )
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(30);
        assert_eq!(
            *channel.receive_message(expected.len(), deadline).unwrap(),
            expected
        );
        channel.send_message(&[], 0, deadline).unwrap();
    });
    let trusted = pairing(public);
    let binding = trusted.sign_noise_binding(&identity(1)).unwrap();
    let mut channel = connect(client, &trusted, &binding, &identity(2), &[9; 32], false).unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    channel
        .send_message(&payload, payload.len(), deadline)
        .unwrap();
    assert!(channel.receive_message(0, deadline).unwrap().is_empty());
    host.join().unwrap();
}

#[test]
fn oversized_message_declaration_closes_the_channel_before_reassembly() {
    let (private, public) = generate_noise_key().unwrap();
    let (client, server, _) = pipes();
    let host = thread::spawn(move || {
        let mut channel = accept(
            server,
            &pairing(public),
            &private,
            &identity(1),
            &[9; 32],
            false,
            |_, _| Ok(()),
        )
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(30);
        assert!(channel.receive_message(1024, deadline).is_err());
        assert!(channel
            .send(b"cannot reuse failed channel", deadline)
            .is_err());
    });
    let trusted = pairing(public);
    let binding = trusted.sign_noise_binding(&identity(1)).unwrap();
    let mut channel = connect(client, &trusted, &binding, &identity(2), &[9; 32], false).unwrap();
    channel
        .send(
            &[1, 255, 255, 255, 255],
            Instant::now() + Duration::from_secs(30),
        )
        .unwrap();
    host.join().unwrap();
}

#[test]
fn transport_provisioning_survives_restart_and_rotates_opaque_routing() {
    use crate::mobile_memory::{
        grant::{DeviceGrant, DeviceScope},
        registry::{DeviceRecord, DeviceRegistry},
    };
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
    use chrono::Utc;
    let home = tempfile::tempdir().unwrap();
    let registry = DeviceRegistry::load(home.path()).unwrap();
    let host = identity(1);
    let device = identity(2);
    let id = Uuid::from_u128(5);
    let now = Utc::now();
    let public_key = URL_SAFE_NO_PAD.encode(device.verifying_key().to_sec1_point(false).as_bytes());
    let record = DeviceRecord {
        id,
        display_name: "Synthetic phone".into(),
        public_key_x963: public_key.clone(),
        paired_at: now,
        revoked_at: None,
        suspended_at: None,
        scopes: vec![DeviceScope::MemoryRead],
    };
    let grant = DeviceGrant::for_device(id, &public_key, record.scopes.clone(), now).unwrap();
    registry.register_with_grant(record, grant.clone()).unwrap();
    let first = storage::load_or_provision(home.path(), &registry, &host, id).unwrap();
    let received = first.provisioning(&host).unwrap();
    let trusted = received
        .verify(host.verifying_key(), id, device.verifying_key())
        .unwrap();
    assert_eq!(trusted.grant_id, grant.id);
    assert!(received
        .verify(identity(3).verifying_key(), id, device.verifying_key())
        .is_err());
    let restarted_registry = DeviceRegistry::load(home.path()).unwrap();
    let second = storage::load_or_provision(home.path(), &restarted_registry, &host, id).unwrap();
    assert_eq!(first.pairing.noise_key, second.pairing.noise_key);
    assert_eq!(
        first.rendezvous(17, false).unwrap(),
        second.rendezvous(17, false).unwrap()
    );
    assert_eq!(
        first.rendezvous(17, false).unwrap(),
        received.rendezvous(17, false).unwrap()
    );
    assert_ne!(
        first.rendezvous(17, false).unwrap(),
        first.rendezvous(18, false).unwrap()
    );
    assert_ne!(
        first.rendezvous(17, false).unwrap(),
        first.rendezvous(17, true).unwrap()
    );
    let mut narrowed = grant.clone();
    narrowed.id = Uuid::new_v4();
    narrowed.revocation_epoch += 1;
    registry.replace_grant(id, narrowed).unwrap();
    assert!(authority::check_current(&registry, &trusted, false, now).is_err());
    let updated = storage::load_or_provision(home.path(), &registry, &host, id).unwrap();
    assert_ne!(
        first.rendezvous(17, false).unwrap(),
        updated.rendezvous(17, false).unwrap()
    );
    assert_eq!(updated.pairing.device_key, first.pairing.device_key);
    registry.revoke(id, now).unwrap();
    assert!(storage::load_or_provision(home.path(), &registry, &host, id).is_err());
}

#[test]
fn relay_configuration_is_opt_in_and_private() {
    let home = tempfile::tempdir().unwrap();
    assert!(storage::RelayConfig::load(home.path()).unwrap().is_none());
    let config = storage::RelayConfig {
        version: 1,
        enabled: true,
        endpoints: vec![websocket::EndpointConfig {
            url: "wss://example.test/ws".into(),
            ca_certificates: Vec::new(),
        }],
    };
    config.save(home.path()).unwrap();
    let loaded = storage::RelayConfig::load(home.path()).unwrap().unwrap();
    assert!(loaded.enabled);
    assert!(loaded.endpoints == config.endpoints);
    assert!(storage::RelayConfig {
        version: 2,
        enabled: true,
        endpoints: config.endpoints
    }
    .save(home.path())
    .is_err());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let file = home.path().join("mobile/relay.json");
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(storage::RelayConfig::load(home.path()).is_err());
    }
}
