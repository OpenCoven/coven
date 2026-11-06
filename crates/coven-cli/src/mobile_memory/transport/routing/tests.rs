use super::*;

#[test]
fn routing_v2_uses_independent_length_prefixed_kdf() {
    let vectors: serde_json::Value =
        serde_json::from_str(include_str!("../vectors/wire-v2.json")).unwrap();
    let seed = std::array::from_fn(|i| i as u8);
    let hex = |bytes: &[u8]| bytes.iter().map(|b| format!("{b:02x}")).collect::<String>();
    for case in vectors["routing"].as_array().unwrap() {
        let transport = if case["transport"] == "direct" {
            Transport::Direct
        } else {
            Transport::Relay
        };
        let (room, credential) = derive(
            &seed,
            transport,
            case["generation"].as_u64().unwrap(),
            case["counter"].as_u64().unwrap(),
        )
        .unwrap();
        assert_eq!(hex(&room), case["room"].as_str().unwrap(), "{case}");
        assert_eq!(
            hex(credential.as_ref()),
            case["credential"].as_str().unwrap(),
            "{case}"
        );
    }
}

#[test]
fn three_device_only_advances_allow_fourth_success_and_lost_completions_do_not_exhaust() {
    let mut host = HostState {
        generation: 7,
        floor: 0,
    };
    let mut device = DeviceState {
        generation: 7,
        next: 0,
        floor: 0,
    };
    for counter in 0..4 {
        assert_eq!(device.room().unwrap(), counter);
        let proof = host.proof(counter).unwrap();
        device = device.consume(proof).unwrap();
        if counter == 3 {
            host = host.consume(proof).unwrap();
        }
    }
    assert_eq!(host.floor, 4);
    // The fourth completion was dropped: the next signed host floor heals f.
    // With no authenticated floor yet, the device first needs direct recovery
    // after four device-only advances; three drops plus success still needs the
    // next authenticated host proof to arrive within the pre-connect window.
    assert!(device.room().is_err());
    let mut host = HostState {
        generation: 7,
        floor: 0,
    };
    let mut device = DeviceState {
        generation: 7,
        next: 0,
        floor: 0,
    };
    for counter in 0..20 {
        let proof = host.proof(device.room().unwrap()).unwrap();
        device = device.consume(proof).unwrap();
        host = host.consume(proof).unwrap();
        // No completion updates device state; next proof carries the floor.
        assert_eq!(device.next, counter + 1);
    }
    assert_eq!(host.floor, 20);
    assert_eq!(device.floor, 19);
}

#[test]
fn genuine_host_proofs_with_four_dropped_device_proofs_exhaust_only_that_pairing() {
    let host = HostState {
        generation: 2,
        floor: 10,
    };
    let mut device = DeviceState {
        generation: 2,
        next: 10,
        floor: 10,
    };
    for counter in 10..14 {
        let proof = host.proof(device.room().unwrap()).unwrap();
        device = device.consume(proof).unwrap();
        assert_eq!(device.next, counter + 1);
    }
    assert!(device.room().is_err());
    assert!(host.proof(14).is_err());
    assert_eq!(host.floor, 10);
}

#[test]
fn stale_generation_consumed_room_and_outside_window_never_advance() {
    let host = HostState {
        generation: 3,
        floor: 10,
    };
    let device = DeviceState {
        generation: 3,
        next: 10,
        floor: 9,
    };
    for proof in [
        HostProof {
            generation: 2,
            host_floor: 10,
            counter: 10,
        },
        HostProof {
            generation: 4,
            host_floor: 10,
            counter: 10,
        },
        HostProof {
            generation: 3,
            host_floor: 11,
            counter: 10,
        },
        HostProof {
            generation: 3,
            host_floor: 8,
            counter: 10,
        },
        HostProof {
            generation: 3,
            host_floor: 10,
            counter: 11,
        },
    ] {
        assert!(device.consume(proof).is_err(), "{proof:?}");
    }
    let valid = host.proof(10).unwrap();
    let advanced = host.consume(valid).unwrap();
    assert_eq!(advanced.floor, 11);
    assert!(advanced.consume(valid).is_err());
    assert!(host.proof(9).is_err());
    assert!(host.proof(14).is_err());
    assert!(host
        .consume(HostProof {
            generation: 3,
            host_floor: 9,
            counter: 13
        })
        .is_err());
}

#[test]
fn counters_and_generation_are_never_wrapped_or_lowered() {
    for floor in [u64::MAX - 3, u64::MAX] {
        assert!(HostState {
            generation: 0,
            floor
        }
        .proof(floor)
        .is_err());
    }
    assert!(DeviceState {
        generation: 0,
        next: u64::MAX,
        floor: u64::MAX
    }
    .room()
    .is_err());
    assert!(DeviceState {
        generation: 0,
        next: 1,
        floor: 2
    }
    .room()
    .is_err());
    let host = HostState {
        generation: u64::MAX,
        floor: u64::MAX - 4,
    };
    assert_eq!(host.proof(u64::MAX - 1).unwrap().counter, u64::MAX - 1);
    assert_eq!(
        host.consume(host.proof(u64::MAX - 1).unwrap())
            .unwrap()
            .floor,
        u64::MAX
    );
}
