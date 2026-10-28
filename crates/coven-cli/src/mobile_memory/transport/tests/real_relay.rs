use super::*;
use crate::mobile_memory::transport::test_support::Relay;

/// The production router pairs whichever host and client present the same
/// room. A broker (or anyone holding routing material) can therefore place a
/// substitute peer in the room. Substitutes must fail before admission on both
/// sides, and the relay's own TLS-terminated view must still hold no plaintext
/// or stable identifier after a genuine control exchange.
#[test]
fn substituted_peers_fail_closed_through_the_production_relay() {
    let relay = Relay::start();
    let (private, public) = generate_noise_key().unwrap();
    let genuine = pairing(public);
    let binding = genuine.sign_noise_binding(&identity(1)).unwrap();

    // Control: the genuine pair exchanges application data in a fresh room.
    let room = [44; 32];
    let host_records = relay.join(websocket::Role::Host, &room);
    let host_pairing = genuine.clone();
    let host_private = private.clone();
    let host = thread::spawn(move || {
        let mut channel = accept(
            host_records,
            &host_pairing,
            &host_private,
            &identity(1),
            &room,
            false,
            |_, _| Ok(()),
        )
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(30);
        assert_eq!(
            channel.receive(deadline).unwrap().as_slice(),
            b"control-request-private-marker"
        );
        channel
            .send(b"control-response-private-marker", deadline)
            .unwrap();
        // The relay prioritises a peer disconnect over queued records, so the
        // host must not leave before the device has read the response.
        assert_eq!(
            channel.receive(deadline).unwrap().as_slice(),
            b"control-received"
        );
    });
    let device_records = relay.join(websocket::Role::Client, &room);
    let mut channel = connect(
        device_records,
        &genuine,
        &binding,
        &identity(2),
        &room,
        false,
    )
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    channel
        .send(b"control-request-private-marker", deadline)
        .unwrap();
    assert_eq!(
        channel.receive(deadline).unwrap().as_slice(),
        b"control-response-private-marker"
    );
    channel.send(b"control-received", deadline).unwrap();
    host.join().unwrap();

    // Substitute responder: a host with a different pairing-scoped Noise key.
    let room = [41; 32];
    let (impostor_private, impostor_public) = generate_noise_key().unwrap();
    let impostor_host_records = relay.join(websocket::Role::Host, &room);
    let impostor_pairing = pairing(impostor_public);
    let impostor_host = thread::spawn(move || {
        accept(
            impostor_host_records,
            &impostor_pairing,
            &impostor_private,
            &identity(1),
            &room,
            false,
            |_, _| Ok(()),
        )
        .map(|_| ())
    });
    let device_records = relay.join(websocket::Role::Client, &room);
    let substituted = connect(
        device_records,
        &genuine,
        &binding,
        &identity(2),
        &room,
        false,
    );
    assert!(
        substituted.is_err(),
        "device accepted a substitute responder"
    );
    assert!(impostor_host.join().unwrap().is_err());

    // Substitute initiator: a device holding the room, credential and copied
    // binding but a different signing key.
    let room = [43; 32];
    let host_records = relay.join(websocket::Role::Host, &room);
    let host_pairing = genuine.clone();
    let host_private = private.clone();
    let host = thread::spawn(move || {
        accept(
            host_records,
            &host_pairing,
            &host_private,
            &identity(1),
            &room,
            false,
            |_, _| Ok(()),
        )
        .map(|_| ())
    });
    let impostor_records = relay.join(websocket::Role::Client, &room);
    let impostor_device = connect(
        impostor_records,
        &genuine,
        &binding,
        &identity(3),
        &room,
        false,
    );
    assert!(impostor_device.is_err(), "impostor device was admitted");
    assert!(host.join().unwrap().is_err());

    let captured = relay.captured();
    assert!(
        captured
            .windows(b"room=".len())
            .any(|part| part == b"room="),
        "capture does not include the HTTP upgrade"
    );
    let mut markers = stable_identifier_markers(&genuine);
    markers.push((
        "request plaintext",
        b"control-request-private-marker".to_vec(),
    ));
    markers.push((
        "response plaintext",
        b"control-response-private-marker".to_vec(),
    ));
    markers.push(("completion plaintext", b"host-finished-v1".to_vec()));
    assert_bytes_exclude(&captured, &markers);
}

#[test]
fn production_relay_over_wss_carries_fresh_authenticated_reconnections() {
    let relay = Relay::start();
    let (private, public) = generate_noise_key().unwrap();
    let paired = pairing(public);
    let binding = paired.sign_noise_binding(&identity(1)).unwrap();
    for room in [[31; 32], [32; 32]] {
        let host_records = relay.join(websocket::Role::Host, &room);
        let client_records = relay.join(websocket::Role::Client, &room);
        let host_pairing = paired.clone();
        let host_private = private.clone();
        let host = thread::spawn(move || {
            let mut channel = accept(
                host_records,
                &host_pairing,
                &host_private,
                &identity(1),
                &room,
                false,
                |_, _| Ok(()),
            )
            .unwrap();
            let deadline = Instant::now() + Duration::from_secs(30);
            let request = channel.receive_message(1024 * 1024, deadline).unwrap();
            assert!(request.as_slice() == vec![73; 128 * 1024]);
            channel
                .send_message(b"authenticated-response", 1024, deadline)
                .unwrap();
            assert_eq!(
                channel.receive_message(1024, deadline).unwrap().as_slice(),
                b"response-received"
            );
        });
        let mut channel = connect(
            client_records,
            &paired,
            &binding,
            &identity(2),
            &room,
            false,
        )
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(30);
        channel
            .send_message(&vec![73; 128 * 1024], 1024 * 1024, deadline)
            .unwrap();
        assert_eq!(
            channel.receive_message(1024, deadline).unwrap().as_slice(),
            b"authenticated-response"
        );
        channel
            .send_message(b"response-received", 1024, deadline)
            .unwrap();
        host.join().unwrap();
    }
}
