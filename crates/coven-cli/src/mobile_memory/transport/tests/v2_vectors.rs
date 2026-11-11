use super::super::*;

fn unhex(value: &str) -> Vec<u8> {
    value
        .as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}
fn data(value: &serde_json::Value, key: &str) -> Vec<u8> {
    unhex(value[key].as_str().unwrap())
}
fn scalar(n: u8) -> SigningKey {
    let mut bytes = [0; 32];
    bytes[31] = n;
    SigningKey::from_slice(&bytes).unwrap()
}

#[test]
fn v2_independent_signatures_reject_wrong_roles_and_every_routing_field_change() {
    let vectors: serde_json::Value =
        serde_json::from_str(include_str!("../vectors/channel-v2.json")).unwrap();
    let routing: serde_json::Value =
        serde_json::from_str(include_str!("../vectors/wire-v2.json")).unwrap();
    let pairing = Pairing {
        device_id: Uuid::from_u128(1),
        grant_id: Uuid::from_u128(2),
        revocation_epoch: 3,
        host_key: *scalar(1).verifying_key(),
        device_key: *scalar(2).verifying_key(),
        noise_key: std::array::from_fn(|i| i as u8),
    };
    for case in vectors["signatures"].as_array().unwrap() {
        let preimage = data(case, "preimage");
        let signature = data(case, "signature");
        let role = case["role"].as_str().unwrap();
        let key = if role == "host" {
            &pairing.host_key
        } else {
            &pairing.device_key
        };
        verify(key, &preimage, &signature).unwrap();
        assert!(verify(key, &preimage, &data(case, "invalidSignature")).is_err());
        assert!(verify(key, &data(case, "wrongRolePreimage"), &signature).is_err());
        let fields = &routing["possessionProofs"][case["case"].as_u64().unwrap() as usize];
        let proof = routing::HostProof {
            generation: fields["generation"].as_u64().unwrap(),
            host_floor: fields["hostFloor"].as_u64().unwrap(),
            counter: fields["counter"].as_u64().unwrap(),
        };
        assert_eq!(
            pairing
                .proof_v2(role, &[3; 32], &[4; 32], &[5; 32], proof)
                .unwrap(),
            preimage
        );
        for changed in [
            routing::HostProof {
                generation: proof.generation ^ 1,
                ..proof
            },
            routing::HostProof {
                host_floor: proof.host_floor ^ 1,
                ..proof
            },
            routing::HostProof {
                counter: proof.counter ^ 1,
                ..proof
            },
        ] {
            assert!(verify(
                key,
                &pairing
                    .proof_v2(role, &[3; 32], &[4; 32], &[5; 32], changed)
                    .unwrap(),
                &signature
            )
            .is_err());
        }
        for (hash, hn, dn) in [
            ([2; 32], [4; 32], [5; 32]),
            ([3; 32], [6; 32], [5; 32]),
            ([3; 32], [4; 32], [6; 32]),
        ] {
            assert!(verify(
                key,
                &pairing.proof_v2(role, &hash, &hn, &dn, proof).unwrap(),
                &signature
            )
            .is_err());
        }
    }
}

#[test]
fn independent_full_noise_transcripts_interoperate_in_both_directions() {
    let vectors: serde_json::Value =
        serde_json::from_str(include_str!("../vectors/channel-v2.json")).unwrap();
    for case in vectors["noise"].as_array().unwrap() {
        let prologue = data(case, "prologue");
        let private = data(case, "hostPrivate");
        let public = data(case, "hostPublic");
        let ie = data(case, "initiatorEphemeral");
        let re = data(case, "responderEphemeral");
        let mut initiator = Builder::new(PROFILE.parse().unwrap())
            .prologue(&prologue)
            .unwrap()
            .remote_public_key(&public)
            .unwrap()
            .fixed_ephemeral_key_for_testing_only(&ie)
            .build_initiator()
            .unwrap();
        let mut responder = Builder::new(PROFILE.parse().unwrap())
            .prologue(&prologue)
            .unwrap()
            .local_private_key(&private)
            .unwrap()
            .fixed_ephemeral_key_for_testing_only(&re)
            .build_responder()
            .unwrap();
        let mut output = [0; 1024];
        let mut clear = [0; 1024];
        let n = initiator.write_message(&[], &mut output).unwrap();
        assert_eq!(&output[..n], data(case, "first"));
        assert_eq!(responder.read_message(&output[..n], &mut clear).unwrap(), 0);
        let n = responder.write_message(&[], &mut output).unwrap();
        assert_eq!(&output[..n], data(case, "second"));
        assert_eq!(initiator.read_message(&output[..n], &mut clear).unwrap(), 0);
        assert_eq!(initiator.get_handshake_hash(), data(case, "handshakeHash"));
        assert_eq!(responder.get_handshake_hash(), data(case, "handshakeHash"));
        let mut initiator = initiator.into_transport_mode().unwrap();
        let mut responder = responder.into_transport_mode().unwrap();
        let plaintext = data(case, "plaintext");
        let n = initiator.write_message(&plaintext, &mut output).unwrap();
        assert_eq!(&output[..n], data(case, "initiatorRecord"));
        let n = responder.read_message(&output[..n], &mut clear).unwrap();
        assert_eq!(&clear[..n], plaintext);
        let n = responder.write_message(&plaintext, &mut output).unwrap();
        assert_eq!(&output[..n], data(case, "responderRecord"));
        let n = initiator.read_message(&output[..n], &mut clear).unwrap();
        assert_eq!(&clear[..n], plaintext);
    }
}
