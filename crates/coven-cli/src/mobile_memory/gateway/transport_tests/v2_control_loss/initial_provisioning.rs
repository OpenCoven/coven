//! Signed direct control before any routing-generation anchor exists, plus the
//! rotation-adoption and device-recovery stages the parent matrix starts after
//! (#1685). Like the parent, requests here are dispatched in process; the
//! `native_https` child repeats the losses over a real TLS connection.
use super::*;
use serde_json::json;

fn prepare_body() -> serde_json::Value {
    json!({"version":2,"operation":"prepare"})
}
fn host(f: &Fixture) -> SigningKey {
    let identity =
        crate::mobile_memory::identity::load_existing_host_identity(&f.state.coven_home).unwrap();
    SigningKey::from_pkcs8_der(&identity.private_key_der).unwrap()
}
fn transport_file(f: &Fixture) -> std::path::PathBuf {
    f.state
        .coven_home
        .join(format!("mobile/transport-{}.json", f.pairing.device_id))
}
fn device_file(f: &Fixture, phone: &Path) -> std::path::PathBuf {
    phone.join(format!(
        "mobile/device-transport-{}.json",
        f.pairing.device_id
    ))
}
/// Host material, protocol floor and device files only. Successful signed
/// requests may legitimately touch other authority metadata.
fn routing(f: &Fixture, phone: &Path) -> Vec<Option<Vec<u8>>> {
    let mut image = routing_image(f, phone);
    image.truncate(3);
    image
}
fn host_state(f: &Fixture) -> Option<serde_json::Value> {
    optional(&transport_file(f)).map(|bytes| serde_json::from_slice(&bytes).unwrap())
}
/// (minimum, committed, pending) generations; `None` for absent or v1 material,
/// neither of which carries a routing-generation anchor.
fn generations(f: &Fixture) -> Option<(Option<u64>, Option<u64>, Option<u64>)> {
    let state = host_state(f)?;
    if state["version"] == 1 {
        return None;
    }
    assert_eq!(state["version"], 2);
    let generation = |key: &str| state[key]["generation"].as_u64();
    Some((
        state["minimumGeneration"].as_u64(),
        generation("committed"),
        generation("pending"),
    ))
}
fn unanchored(f: &Fixture, legacy: bool) {
    // Gateway fixtures start v1-provisioned. Remove that file to model a v2
    // client whose pairing has never fetched routing material.
    if !legacy {
        std::fs::remove_file(transport_file(f)).unwrap();
    }
    assert_eq!(optional(&transport_file(f)).is_some(), legacy);
    assert_eq!(generations(f), None, "fixture already has a v2 anchor");
    assert!(
        routing_image(f, Path::new("/nonexistent-phone"))[1].is_none(),
        "fixture already has a v2 protocol floor"
    );
}
fn inactive(f: &Fixture) {
    for transport in [Transport::Direct, Transport::Relay] {
        assert!(
            v2::load(
                &f.state.coven_home,
                &f.state.registry,
                host(f).verifying_key(),
                f.pairing.device_id,
                transport,
            )
            .unwrap()
            .is_none(),
            "unacknowledged generation admitted a {transport:?} channel"
        );
    }
}
fn refused(f: &Fixture, phone: &Path, body: &serde_json::Value) {
    let before = routing(f, phone);
    let response = dispatch(f, &request(f, body));
    assert_ne!(
        response.status, 200,
        "{body} was accepted: {}",
        response.body
    );
    assert!(!response.body.contains("routingSeed"));
    assert!(
        routing(f, phone) == before,
        "refused {body} changed routing state"
    );
}
/// Truncation at three points, a complete request lost before protected
/// dispatch and a tampered body each leave every routing file unchanged.
fn lose_request(f: &Fixture, phone: &Path, wire: &[u8]) {
    let before = routing_image(f, phone);
    let header = wire.windows(4).position(|p| p == b"\r\n\r\n").unwrap() + 4;
    for cut in [
        header - 1,
        header + (wire.len() - header) / 2,
        wire.len() - 1,
    ] {
        assert!(
            read_mobile_request_buffered(&mut BufReader::new(std::io::Cursor::new(&wire[..cut])))
                .is_err(),
            "truncated request was accepted"
        );
    }
    drop(read_mobile_request_buffered(&mut BufReader::new(std::io::Cursor::new(wire))).unwrap());
    let mut tampered = wire.to_vec();
    let version = tampered
        .windows(b"\"version\":2".len())
        .position(|p| p == b"\"version\":2")
        .unwrap();
    tampered[version + b"\"version\":2".len() - 1] = b'3';
    assert_eq!(dispatch(f, &tampered).status, 400);
    assert!(
        routing_image(f, phone) == before,
        "lost or tampered request changed routing state"
    );
}
/// Discard the entire reply or deliver only half of it.
fn lose_reply(response: MobileHttpResponse, partial: bool) -> serde_json::Value {
    assert_eq!(response.status, 200, "{}", response.body);
    let data = serde_json::from_str::<serde_json::Value>(&response.body).unwrap()["data"].clone();
    let encoded = response.to_http();
    let delivered = if partial {
        &encoded[..encoded.len() / 2]
    } else {
        &encoded[..0]
    };
    assert!(
        complete_reply(delivered).is_err(),
        "lost/truncated reply was accepted as complete"
    );
    data
}
fn exchange(f: &Fixture, phone: &Path, expected: (u64, u64, u64)) {
    for transport in [Transport::Direct, Transport::Relay] {
        let (server, client) = exchange_files(f, phone, transport, false, None);
        assert!(server.is_ok() && client.is_ok(), "{server:?} {client:?}");
        assert_eq!(counters(f, phone, transport), expected);
    }
}
fn ack_with(generation: u64, digest: [u8; 32]) -> serde_json::Value {
    json!({"version":2,"operation":"acknowledge","generation":generation,"seedDigest":digest})
}

#[test]
fn unanchored_signed_provisioning_converges_to_one_anchor_across_loss_retry_and_duplicates() {
    for legacy in [false, true] {
        for partial_reply in [false, true] {
            let f = Fixture::new();
            let phone = tempfile::tempdir().unwrap();
            let phone = phone.path();
            unanchored(&f, legacy);
            let pristine = routing_image(&f, phone);
            // Controls that need an anchor or a pending value manufacture neither.
            for body in [
                ack_with(0, [0; 32]),
                json!({"version":2,"operation":"resync","transport":"direct","generation":0,"next":3}),
                json!({"version":2,"operation":"resync","transport":"relay","generation":0,"next":3}),
                json!({"version":2,"operation":"recover"}),
            ] {
                refused(&f, phone, &body);
                assert!(routing_image(&f, phone) == pristine);
            }
            let wire = request(&f, &prepare_body());
            lose_request(&f, phone, &wire);
            assert!(routing_image(&f, phone) == pristine);

            // The first dispatch persists one pending generation; its reply is lost.
            let offered = lose_reply(dispatch(&f, &wire), partial_reply);
            assert_eq!(offered["generation"].as_u64(), Some(0));
            assert_eq!(generations(&f), Some((None, None, Some(0))));
            let pending = routing(&f, phone);
            assert!(pending[1].is_some(), "protocol floor missing");
            assert!(pending[2].is_none(), "device adopted without a reply");
            inactive(&f);
            assert_eq!(dispatch(&f, &wire).status, 409, "exact replay accepted");
            let bootstrap = dispatch(&f, &f.request("POST", PATH, f.pairing.device_id));
            assert_ne!(bootstrap.status, 200, "legacy bootstrap opened a v1 anchor");
            assert!(routing(&f, phone) == pending);

            // Concurrent fresh-nonce prepares and anchorless recovery requests
            // all return the one retained pending value.
            let wires: Vec<_> = (0..8).map(|_| request(&f, &prepare_body())).collect();
            let replies: Vec<_> = std::thread::scope(|scope| {
                let workers: Vec<_> = wires
                    .iter()
                    .map(|wire| scope.spawn(|| dispatch(&f, wire)))
                    .collect();
                workers.into_iter().map(|w| w.join().unwrap()).collect()
            });
            for reply in replies {
                assert_eq!(reply.status, 200, "{}", reply.body);
                assert!(
                    complete_reply(&reply.to_http()).unwrap() == offered,
                    "concurrent duplicate replaced pending material"
                );
            }
            for body in [
                json!({"version":2,"operation":"recover"}),
                json!({"version":2,"operation":"recover","knownGeneration":0}),
            ] {
                assert!(
                    successful(&f, &body) == offered,
                    "anchorless recovery replaced pending material"
                );
            }
            for known in [1, 5] {
                refused(
                    &f,
                    phone,
                    &json!({"version":2,"operation":"recover","knownGeneration":known}),
                );
            }
            assert!(routing(&f, phone) == pending);

            // Nothing activates without the device's exact authenticated adoption.
            let exact = serde_json::from_value::<v2::Handover>(offered.clone())
                .unwrap()
                .acknowledgment();
            let mut wrong = exact.seed_digest;
            wrong[0] ^= 1;
            for body in [
                ack_with(0, wrong),
                ack_with(1, exact.seed_digest),
                json!({"version":2,"operation":"resync","transport":"direct","generation":0,"next":3}),
                json!({"version":2,"operation":"resync","transport":"relay","generation":0,"next":3}),
            ] {
                refused(&f, phone, &body);
            }
            assert!(routing(&f, phone) == pending);
            inactive(&f);

            // The device adopts; a duplicated reply is idempotent.
            let ack = handover(&f, offered.clone(), phone);
            assert_eq!(ack.seed_digest, exact.seed_digest);
            let adopted = routing(&f, phone);
            assert!(adopted[2].is_some());
            assert_eq!(
                handover(&f, offered.clone(), phone).seed_digest,
                ack.seed_digest
            );
            assert!(routing(&f, phone) == adopted);

            // Adoption-stage loss: the acknowledgment never arrives, so the
            // device restarts from prepare and re-adopts the identical value.
            let ack_wire = request(&f, &ack_body(&ack));
            lose_request(&f, phone, &ack_wire);
            assert_eq!(generations(&f), Some((None, None, Some(0))));
            let restarted = successful(&f, &prepare_body());
            assert!(restarted == offered, "restart replaced pending material");
            assert_eq!(handover(&f, restarted, phone).seed_digest, ack.seed_digest);
            assert!(routing(&f, phone) == adopted);
            inactive(&f);

            // The retransmitted acknowledgment commits; its reply is lost.
            let committed = lose_reply(dispatch(&f, &ack_wire), partial_reply);
            assert_eq!(committed["committed"].as_bool(), Some(true));
            assert_eq!(generations(&f), Some((None, Some(0), None)));
            assert_eq!(dispatch(&f, &ack_wire).status, 409, "exact replay accepted");
            exchange(&f, phone, (1, 1, 0));
            let consumed = routing(&f, phone);
            let retry = successful(&f, &ack_body(&ack));
            assert_eq!(retry["generation"].as_u64(), Some(0));
            assert_eq!(retry["committed"].as_bool(), Some(true));
            // A late duplicate of the original reply reaches the device.
            assert_eq!(handover(&f, offered.clone(), phone).generation, 0);
            assert!(
                routing(&f, phone) == consumed,
                "acknowledgment retry or late reply reset counters"
            );
            for transport in [Transport::Direct, Transport::Relay] {
                assert_eq!(counters(&f, phone, transport), (1, 1, 0));
            }

            // Exactly one anchor; no v1 or second v2 anchor reopened.
            assert_eq!(generations(&f), Some((None, Some(0), None)));
            let bootstrap = dispatch(&f, &f.request("POST", PATH, f.pairing.device_id));
            assert_ne!(bootstrap.status, 200, "legacy bootstrap reopened v1");
            assert!(routing(&f, phone) == consumed);
            exchange(&f, phone, (2, 2, 1));
            println!(
                "pre-anchor-signed legacy={legacy} partial_reply={partial_reply} anchor=0 converged"
            );
        }
    }
}

#[test]
fn unacknowledged_initial_generation_burns_on_authority_change_and_stale_deliveries_cannot_lower_it(
) {
    for legacy in [false, true] {
        let f = Fixture::new();
        let phone = tempfile::tempdir().unwrap();
        let phone = phone.path();
        unanchored(&f, legacy);
        let first = successful(&f, &prepare_body());
        assert_eq!(first["generation"].as_u64(), Some(0));
        let first_ack = handover(&f, first.clone(), phone);
        // The device's acknowledgment is lost, then authority changes.
        let lost_ack = request(&f, &ack_body(&first_ack));
        let id = f.pairing.device_id;
        let current = f.state.registry.authorization_record(id).unwrap().unwrap();
        let now = Utc::now();
        let grant = current
            .grant
            .reissue(
                &current.device.public_key_x963,
                current.grant.scopes.clone(),
                current.grant.restrictions.clone(),
                current.grant.minimum_assurance,
                now,
                now + chrono::Duration::days(30),
            )
            .unwrap();
        let grant_id = grant.id.to_string();
        f.state.registry.replace_grant(id, grant).unwrap();
        let regranted = routing(&f, phone);
        assert_ne!(
            dispatch(&f, &lost_ack).status,
            200,
            "delayed acknowledgment committed under stale authority"
        );
        assert!(routing(&f, phone) == regranted);
        assert_eq!(generations(&f), Some((None, None, Some(0))));

        // The next prepare burns generation 0 and offers generation 1 once.
        let wire = request(&f, &prepare_body());
        let next = lose_reply(dispatch(&f, &wire), legacy);
        assert_eq!(next["generation"].as_u64(), Some(1));
        assert_ne!(next["routingSeed"], first["routingSeed"]);
        assert_eq!(next["grantId"].as_str(), Some(grant_id.as_str()));
        assert_eq!(generations(&f), Some((Some(0), None, Some(1))));
        let burned = routing(&f, phone);
        for body in [
            prepare_body(),
            json!({"version":2,"operation":"recover"}),
            json!({"version":2,"operation":"recover","knownGeneration":0}),
            json!({"version":2,"operation":"recover","knownGeneration":1}),
        ] {
            assert!(
                successful(&f, &body) == next,
                "{body} replaced the replacement generation"
            );
        }
        refused(
            &f,
            phone,
            &json!({"version":2,"operation":"recover","knownGeneration":2}),
        );
        refused(&f, phone, &ack_body(&first_ack));
        assert!(routing(&f, phone) == burned);
        inactive(&f);

        let ack = handover(&f, next.clone(), phone);
        assert_eq!(ack.generation, 1);
        successful(&f, &ack_body(&ack));
        assert_eq!(generations(&f), Some((Some(0), Some(1), None)));
        exchange(&f, phone, (1, 1, 0));

        // Stale deliveries cannot lower either side's generation.
        let anchored = routing(&f, phone);
        let stale: v2::Handover = serde_json::from_value(first).unwrap();
        assert!(
            v2::device::adopt_direct(
                phone,
                stale,
                host(&f).verifying_key(),
                id,
                f.device.verifying_key(),
            )
            .is_err(),
            "device adopted a lower generation"
        );
        refused(&f, phone, &ack_body(&first_ack));
        assert!(routing(&f, phone) == anchored);
        assert_eq!(generations(&f), Some((Some(0), Some(1), None)));
        exchange(&f, phone, (2, 2, 1));
        println!("pre-anchor-burn legacy={legacy} burned=0 anchor=1 converged");
    }
}

#[test]
fn rotation_adoption_and_device_recovery_loss_retry_identical_material_without_counter_reset() {
    for partial_reply in [false, true] {
        let f = Fixture::new();
        let phone = tempfile::tempdir().unwrap();
        let phone = phone.path();
        let ack = handover(&f, successful(&f, &prepare_body()), phone);
        successful(&f, &ack_body(&ack));
        exchange(&f, phone, (1, 1, 0));

        // Rotation prepare: request loss, then reply loss.
        let wire = request(&f, &prepare_body());
        lose_request(&f, phone, &wire);
        let offered = lose_reply(dispatch(&f, &wire), partial_reply);
        assert_eq!(offered["generation"].as_u64(), Some(1));
        assert_eq!(generations(&f), Some((None, Some(0), Some(1))));
        assert_eq!(
            host_state(&f).unwrap()["committed"]["floors"],
            json!([1, 1])
        );
        assert_eq!(dispatch(&f, &wire).status, 409, "exact replay accepted");
        let pending = routing(&f, phone);
        assert!(successful(&f, &prepare_body()) == offered);
        assert!(routing(&f, phone) == pending);

        // Adoption: the acknowledgment is lost; the device restarts from prepare.
        let next = handover(&f, offered.clone(), phone);
        let adopted = routing(&f, phone);
        let ack_wire = request(&f, &ack_body(&next));
        lose_request(&f, phone, &ack_wire);
        assert!(successful(&f, &prepare_body()) == offered);
        assert_eq!(
            handover(&f, offered.clone(), phone).seed_digest,
            next.seed_digest
        );
        assert!(
            routing(&f, phone) == adopted,
            "re-adoption replaced device material or counters"
        );
        assert_eq!(
            host_state(&f).unwrap()["committed"]["floors"],
            json!([1, 1])
        );
        lose_reply(dispatch(&f, &ack_wire), partial_reply);
        assert_eq!(generations(&f), Some((None, Some(1), None)));
        assert_eq!(dispatch(&f, &ack_wire).status, 409, "exact replay accepted");
        successful(&f, &ack_body(&next));
        exchange(&f, phone, (1, 1, 0));
        let consumed = routing(&f, phone);
        successful(&f, &ack_body(&next));
        assert!(routing(&f, phone) == consumed, "ack retry reset counters");

        // Device-state recovery: the phone lost its anchor and reports none.
        for (generation, corrupt) in [(2, false), (3, true)] {
            let path = device_file(&f, phone);
            if corrupt {
                crate::mobile_memory::config::atomic_replace_private(&path, b"{bad-json}").unwrap();
            } else {
                std::fs::remove_file(&path).unwrap();
            }
            let body = json!({"version":2,"operation":"recover"});
            let wire = request(&f, &body);
            lose_request(&f, phone, &wire);
            let offered = lose_reply(dispatch(&f, &wire), partial_reply);
            assert_eq!(offered["generation"].as_u64(), Some(generation));
            assert_eq!(
                generations(&f),
                Some((None, Some(generation - 1), Some(generation)))
            );
            assert_eq!(
                host_state(&f).unwrap()["committed"]["floors"],
                json!([1, 1])
            );
            assert_eq!(dispatch(&f, &wire).status, 409, "exact replay accepted");
            let pending = routing(&f, phone);
            let retry = successful(&f, &body);
            assert!(retry == offered, "recovery retry replaced pending material");
            assert!(routing(&f, phone) == pending);
            let host = host(&f);
            let mut recovered = None;
            for _ in 0..2 {
                recovered = Some(
                    v2::device::adopt_recovery_direct(
                        phone,
                        serde_json::from_value(retry.clone()).unwrap(),
                        host.verifying_key(),
                        f.pairing.device_id,
                        f.device.verifying_key(),
                    )
                    .unwrap(),
                );
            }
            let recovered = recovered.unwrap();
            assert_eq!(recovered.generation, generation);
            successful(&f, &ack_body(&recovered));
            assert_eq!(generations(&f), Some((None, Some(generation), None)));
            exchange(&f, phone, (1, 1, 0));
            println!(
                "device-recovery-signed partial_reply={partial_reply} corrupt={corrupt} generation={generation} converged"
            );
        }
        println!("rotation-signed partial_reply={partial_reply} converged");
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Unanchored {
    NeverProvisioned,
    V1,
    /// The crash window between the synced protocol floor and first material.
    FloorOnly,
}
fn recover_named(known: u64) -> serde_json::Value {
    json!({"version":2,"operation":"recover","knownGeneration":known})
}
fn bootstrap_refused(f: &Fixture, phone: &Path) {
    let before = routing(f, phone);
    let response = dispatch(f, &f.request("POST", PATH, f.pairing.device_id));
    assert_ne!(response.status, 200, "legacy bootstrap opened a v1 anchor");
    assert!(routing(f, phone) == before);
}

/// A signed `recover` naming a generation is the one path that can write the
/// first anchor without generation 0: the contract accepts an authenticated
/// device generation. It may only raise the generation, then converges.
#[test]
fn signed_recovery_naming_a_generation_on_an_unanchored_host_only_raises_it_and_converges() {
    for start in [
        Unanchored::NeverProvisioned,
        Unanchored::V1,
        Unanchored::FloorOnly,
    ] {
        for partial_reply in [false, true] {
            let f = Fixture::new();
            let id = f.pairing.device_id;
            let phone = tempfile::tempdir().unwrap();
            let phone = phone.path();
            unanchored(&f, start == Unanchored::V1);
            if start == Unanchored::FloorOnly {
                let marker = f
                    .state
                    .coven_home
                    .join(format!("mobile/.transport-v2-{id}.required"));
                crate::mobile_memory::config::atomic_create_private(
                    &marker,
                    b"coven.mobile.routing.v2\n",
                )
                .unwrap();
                // Without a named generation, nothing can be guessed here.
                for body in [
                    prepare_body(),
                    json!({"version":2,"operation":"recover"}),
                    ack_with(0, [0; 32]),
                    json!({"version":2,"operation":"resync","transport":"direct","generation":0,"next":3}),
                ] {
                    refused(&f, phone, &body);
                }
                bootstrap_refused(&f, phone);
            }
            let pristine = routing_image(&f, phone);
            refused(&f, phone, &recover_named(u64::MAX));
            assert!(routing_image(&f, phone) == pristine, "overflow wrote state");

            let body = recover_named(4);
            let wire = request(&f, &body);
            lose_request(&f, phone, &wire);
            assert!(routing_image(&f, phone) == pristine);
            let offered = lose_reply(dispatch(&f, &wire), partial_reply);
            assert_eq!(offered["generation"].as_u64(), Some(5));
            assert_eq!(generations(&f), Some((Some(4), None, Some(5))));
            let pending = routing(&f, phone);
            assert!(pending[1].is_some(), "protocol floor missing");
            assert!(pending[2].is_none(), "device adopted without a reply");
            inactive(&f);
            assert_eq!(dispatch(&f, &wire).status, 409, "exact replay accepted");
            bootstrap_refused(&f, phone);

            // Every retry shape converges on the one pending value.
            for retry in [
                body.clone(),
                recover_named(5),
                json!({"version":2,"operation":"recover"}),
                prepare_body(),
            ] {
                assert!(
                    successful(&f, &retry) == offered,
                    "{retry} replaced pending material"
                );
            }
            assert!(routing(&f, phone) == pending);
            for known in [0, 3, 6] {
                refused(&f, phone, &recover_named(known));
            }
            let exact = serde_json::from_value::<v2::Handover>(offered.clone())
                .unwrap()
                .acknowledgment();
            let mut wrong = exact.seed_digest;
            wrong[0] ^= 1;
            for body in [ack_with(5, wrong), ack_with(0, exact.seed_digest)] {
                refused(&f, phone, &body);
            }
            assert!(routing(&f, phone) == pending);
            inactive(&f);

            let ack = handover(&f, offered.clone(), phone);
            assert_eq!(ack.generation, 5);
            successful(&f, &ack_body(&ack));
            assert_eq!(generations(&f), Some((Some(4), Some(5), None)));
            exchange(&f, phone, (1, 1, 0));

            // Once anchored, a stale named recovery can neither lower nor rotate it.
            let consumed = routing(&f, phone);
            for known in [0, 4] {
                refused(&f, phone, &recover_named(known));
            }
            successful(&f, &ack_body(&ack));
            assert_eq!(handover(&f, offered, phone).generation, 5);
            assert!(routing(&f, phone) == consumed, "retry changed counters");
            assert_eq!(generations(&f), Some((Some(4), Some(5), None)));
            bootstrap_refused(&f, phone);
            println!(
                "unanchored-named-recovery start={start:?} partial_reply={partial_reply} anchor=5 converged"
            );
        }
    }
}

/// The legacy path blocks on the material lock while v2 polls it, so unstaggered
/// requests always let legacy win. These offsets are jitter, not a timing
/// assertion: the outcome must hold for whichever order the locks choose.
#[test]
fn legacy_bootstrap_racing_the_first_prepare_leaves_one_pending_generation_and_no_reused_seed() {
    const OFFSETS_MS: [u64; 4] = [0, 2, 4, 8];
    let (mut legacy_served, mut legacy_refused) = (0, 0);
    for round in 0..8 {
        let f = Fixture::new();
        let id = f.pairing.device_id;
        let phone = tempfile::tempdir().unwrap();
        let phone = phone.path();
        unanchored(&f, false);
        let prepares: Vec<_> = (0..4).map(|_| request(&f, &prepare_body())).collect();
        let legacies: Vec<_> = (0..4).map(|_| f.request("POST", PATH, id)).collect();
        let start = std::sync::Barrier::new(8);
        let timed = |wire: &[u8], offset: u64| {
            start.wait();
            std::thread::sleep(std::time::Duration::from_millis(offset));
            dispatch(&f, wire)
        };
        // Even rounds start prepares first; odd rounds start legacy first.
        let (prepare_lead, legacy_lead) = if round % 2 == 0 { (0, 1) } else { (1, 0) };
        let (prepared, legacy): (Vec<_>, Vec<_>) = std::thread::scope(|scope| {
            let timed = &timed;
            let prepared: Vec<_> = prepares
                .iter()
                .zip(OFFSETS_MS)
                .map(|(wire, offset)| scope.spawn(move || timed(wire, offset + prepare_lead)))
                .collect();
            let legacy: Vec<_> = legacies
                .iter()
                .zip(OFFSETS_MS)
                .map(|(wire, offset)| scope.spawn(move || timed(wire, offset + legacy_lead)))
                .collect();
            (
                prepared.into_iter().map(|w| w.join().unwrap()).collect(),
                legacy.into_iter().map(|w| w.join().unwrap()).collect(),
            )
        });
        let offers: Vec<_> = prepared
            .iter()
            .map(|reply| {
                assert_eq!(reply.status, 200, "{}", reply.body);
                complete_reply(&reply.to_http()).unwrap()
            })
            .collect();
        let offered = offers[0].clone();
        assert!(
            offers.iter().all(|offer| *offer == offered),
            "racing prepares offered different material"
        );
        assert_eq!(offered["generation"].as_u64(), Some(0));
        for reply in &legacy {
            if reply.status == 200 {
                legacy_served += 1;
                let material = complete_reply(&reply.to_http()).unwrap();
                assert_ne!(
                    material["routingSeed"], offered["routingSeed"],
                    "v2 reused a seed served to the legacy path"
                );
            } else {
                legacy_refused += 1;
            }
        }
        // Whichever order the locks chose, exactly one v2 pending value remains.
        assert_eq!(generations(&f), Some((None, None, Some(0))));
        bootstrap_refused(&f, phone);
        inactive(&f);
        let ack = handover(&f, offered, phone);
        successful(&f, &ack_body(&ack));
        assert_eq!(generations(&f), Some((None, Some(0), None)));
        exchange(&f, phone, (1, 1, 0));
    }
    println!(
        "legacy-prepare-race rounds=8 legacy_served={legacy_served} legacy_refused={legacy_refused} converged"
    );
}

/// Neither the legacy Noise channel nor the device's routing attempt can skip
/// the acknowledgment: a pending generation authorizes no channel.
#[test]
fn legacy_channel_and_device_attempts_cannot_skip_adoption_of_a_pending_first_generation() {
    for legacy in [false, true] {
        let f = Fixture::new();
        let id = f.pairing.device_id;
        let phone = tempfile::tempdir().unwrap();
        let phone = phone.path();
        unanchored(&f, legacy);
        let signer = host(&f);
        let no_legacy_effect = |stage: &str| {
            let before = routing(&f, phone);
            for direct in [false, true] {
                let wire = f.request("DELETE", "/api/v1/mobile/device", id);
                assert!(
                    handle_transport_message(&f.state, &f.pairing, direct, &wire).is_err(),
                    "legacy channel kept application authority {stage} (direct={direct})"
                );
            }
            let record = f.state.registry.authorization_record(id).unwrap().unwrap();
            assert!(record.device.revoked_at.is_none(), "{stage} revoked device");
            assert!(
                routing(&f, phone) == before,
                "{stage} changed routing state"
            );
        };
        let offered = successful(&f, &prepare_body());
        no_legacy_effect("before adoption");
        inactive(&f);
        // No device file yet: the device can neither join a room nor resync.
        for transport in [Transport::Direct, Transport::Relay] {
            assert!(v2::device::begin(
                phone,
                signer.verifying_key(),
                id,
                f.device.verifying_key(),
                transport,
            )
            .is_err());
            assert!(v2::device::resync_direct(
                phone,
                signer.verifying_key(),
                id,
                f.device.verifying_key(),
                transport,
                HostState {
                    generation: 0,
                    floor: 3,
                },
            )
            .is_err());
        }
        assert!(routing(&f, phone)[2].is_none());
        // Adopted but unacknowledged: the host still authorizes nothing.
        let ack = handover(&f, offered, phone);
        no_legacy_effect("after adoption");
        inactive(&f);
        successful(&f, &ack_body(&ack));
        no_legacy_effect("after commit");
        exchange(&f, phone, (1, 1, 0));
        println!("pending-first-generation legacy={legacy} no channel before acknowledgment");
    }
}

mod native_https;
