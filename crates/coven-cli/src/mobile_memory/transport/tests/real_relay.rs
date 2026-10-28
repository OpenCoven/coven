use super::*;
use crate::mobile_memory::transport::test_support::Relay;

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
