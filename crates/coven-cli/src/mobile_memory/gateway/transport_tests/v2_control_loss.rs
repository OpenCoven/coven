//! Signed request/response dispatch loss and explicit logical-time conformance.
//! This fixture does not claim to exercise a native HTTPS socket.
use super::v2_authority::exchange_files;
use super::*;
use crate::mobile_memory::transport::{
    routing::{HostState, Transport},
    storage::v2,
};
use anyhow::ensure;
use std::{collections::BTreeMap, path::Path};
const PATH: &str = "/api/v1/mobile/transport/bootstrap";
#[derive(Debug, Clone, Copy)]
enum Operation {
    Prepare,
    Ack,
    Resync(Transport),
    Recover(bool),
}
fn request(f: &Fixture, body: &serde_json::Value) -> Vec<u8> {
    f.request_body(
        "POST",
        PATH,
        f.pairing.device_id,
        &serde_json::to_vec(body).unwrap(),
    )
}
fn dispatch(f: &Fixture, wire: &[u8]) -> MobileHttpResponse {
    let parsed =
        read_mobile_request_buffered(&mut BufReader::new(std::io::Cursor::new(wire))).unwrap();
    handle_mobile_request(&f.state, parsed)
}
fn successful(f: &Fixture, body: &serde_json::Value) -> serde_json::Value {
    let response = dispatch(f, &request(f, body));
    assert_eq!(response.status, 200, "signed control failed");
    complete_reply(&response.to_http()).unwrap()
}
/// Bounded fixture collector: never adopt data from an incomplete HTTP reply.
/// This is not a shipped mobile-client HTTP parser.
fn complete_reply(bytes: &[u8]) -> Result<serde_json::Value> {
    ensure!(
        bytes.len() <= MAX_MOBILE_RESPONSE_BYTES + MAX_HEADER_LINE_BYTES * (MAX_HEADER_COUNT + 1),
        "response exceeds fixture collection bound"
    );
    let header_end = bytes
        .windows(4)
        .position(|p| p == b"\r\n\r\n")
        .context("incomplete response headers")?;
    ensure!(
        header_end <= MAX_HEADER_LINE_BYTES * (MAX_HEADER_COUNT + 1),
        "response headers exceed fixture bound"
    );
    let header = std::str::from_utf8(&bytes[..header_end])?;
    ensure!(
        header.starts_with("HTTP/1.1 200 "),
        "unsuccessful control response"
    );
    let mut length = None;
    for line in header.split("\r\n").skip(1) {
        if let Some((name, value)) = line.split_once(':') {
            if name.eq_ignore_ascii_case("content-length") {
                ensure!(length.is_none(), "duplicate response length");
                length = Some(value.trim().parse::<usize>()?);
            }
        }
    }
    let length = length.context("response length missing")?;
    ensure!(
        length <= MAX_MOBILE_RESPONSE_BYTES,
        "response exceeds fixture bound"
    );
    let body = &bytes[header_end + 4..];
    ensure!(body.len() == length, "incomplete response body");
    let envelope: serde_json::Value = serde_json::from_slice(body)?;
    Ok(envelope
        .get("data")
        .context("response data missing")?
        .clone())
}
fn handover(f: &Fixture, data: serde_json::Value, phone: &Path) -> v2::Acknowledgment {
    let value: v2::Handover = serde_json::from_value(data).unwrap();
    let identity =
        crate::mobile_memory::identity::load_existing_host_identity(&f.state.coven_home).unwrap();
    let host = SigningKey::from_pkcs8_der(&identity.private_key_der).unwrap();
    v2::device::adopt_direct(
        phone,
        value,
        host.verifying_key(),
        f.pairing.device_id,
        f.device.verifying_key(),
    )
    .unwrap()
}
fn ack_body(ack: &v2::Acknowledgment) -> serde_json::Value {
    serde_json::json!({"version":2,"operation":"acknowledge","generation":ack.generation,"seedDigest":ack.seed_digest})
}
fn optional(path: &Path) -> Option<Vec<u8>> {
    match std::fs::read(path) {
        Ok(bytes) => Some(bytes),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => panic!("fixture file unavailable: {e}"),
    }
}
fn routing_image(f: &Fixture, phone: &Path) -> Vec<Option<Vec<u8>>> {
    let id = f.pairing.device_id;
    [
        f.state
            .coven_home
            .join(format!("mobile/transport-{id}.json")),
        f.state
            .coven_home
            .join(format!("mobile/.transport-v2-{id}.required")),
        phone.join(format!("mobile/device-transport-{id}.json")),
        f.state.coven_home.join("mobile/devices.json"),
    ]
    .iter()
    .map(|p| optional(p))
    .collect()
}
fn counters(f: &Fixture, phone: &Path, transport: Transport) -> (u64, u64, u64) {
    let id = f.pairing.device_id;
    let host =
        crate::mobile_memory::identity::load_existing_host_identity(&f.state.coven_home).unwrap();
    let signer = SigningKey::from_pkcs8_der(&host.private_key_der).unwrap();
    let state = v2::load(
        &f.state.coven_home,
        &f.state.registry,
        signer.verifying_key(),
        id,
        transport,
    )
    .unwrap()
    .unwrap()
    .state(transport);
    let device = v2::device::begin(
        phone,
        signer.verifying_key(),
        id,
        f.device.verifying_key(),
        transport,
    )
    .unwrap()
    .state();
    (state.floor, device.next, device.floor)
}
#[test]
fn signed_control_dispatch_loss_retries_without_replacing_material_or_resetting_counters() {
    let mut count = BTreeMap::new();
    for operation in [
        Operation::Prepare,
        Operation::Ack,
        Operation::Resync(Transport::Direct),
        Operation::Resync(Transport::Relay),
        Operation::Recover(true),
        Operation::Recover(false),
    ] {
        for partial_reply in [false, true] {
            let f = Fixture::new();
            let phone = tempfile::tempdir().unwrap();
            let mut ack = None;
            if !matches!(operation, Operation::Prepare) {
                ack = Some(handover(
                    &f,
                    successful(&f, &serde_json::json!({"version":2,"operation":"prepare"})),
                    phone.path(),
                ));
                if !matches!(operation, Operation::Ack) {
                    successful(&f, &ack_body(ack.as_ref().unwrap()));
                }
            }
            if let Operation::Resync(transport) = operation {
                for _ in 0..3 {
                    let (server, client) = exchange_files(&f, phone.path(), transport, true, None);
                    assert!(server.is_err() && client.is_err());
                }
                assert_eq!(counters(&f, phone.path(), transport), (0, 3, 0));
            }
            if let Operation::Recover(missing) = operation {
                let path = f
                    .state
                    .coven_home
                    .join(format!("mobile/transport-{}.json", f.pairing.device_id));
                if missing {
                    std::fs::remove_file(path).unwrap();
                } else {
                    super::super::super::config::atomic_replace_private(&path, b"{bad-json}")
                        .unwrap();
                }
            }
            let body = match operation {
                Operation::Prepare => serde_json::json!({"version":2,"operation":"prepare"}),
                Operation::Ack => ack_body(ack.as_ref().unwrap()),
                Operation::Resync(transport) => {
                    serde_json::json!({"version":2,"operation":"resync","transport":if transport==Transport::Direct {"direct"} else {"relay"},"generation":0,"next":3})
                }
                Operation::Recover(_) => {
                    serde_json::json!({"version":2,"operation":"recover","knownGeneration":0})
                }
            };
            let wire = request(&f, &body);
            let before = routing_image(&f, phone.path());
            let header = wire.windows(4).position(|p| p == b"\r\n\r\n").unwrap() + 4;
            for cut in [
                header - 1,
                header + (wire.len() - header) / 2,
                wire.len() - 1,
            ] {
                assert!(
                    read_mobile_request_buffered(&mut BufReader::new(std::io::Cursor::new(
                        &wire[..cut]
                    )))
                    .is_err(),
                    "truncated request was accepted"
                );
                assert!(
                    routing_image(&f, phone.path()) == before,
                    "incomplete request changed routing authority"
                );
            }
            let parsed =
                read_mobile_request_buffered(&mut BufReader::new(std::io::Cursor::new(&wire)))
                    .unwrap();
            drop(parsed); // Complete request lost before protected dispatch.
            assert!(routing_image(&f, phone.path()) == before);
            let mut tampered = wire.clone();
            let version = tampered
                .windows(b"\"version\":2".len())
                .position(|p| p == b"\"version\":2")
                .unwrap();
            tampered[version + b"\"version\":2".len() - 1] = b'3';
            let parsed =
                read_mobile_request_buffered(&mut BufReader::new(std::io::Cursor::new(&tampered)))
                    .unwrap();
            let auth = parse_auth_headers(&parsed.headers).unwrap();
            assert!(matches!(
                f.state.authenticator.verify_with_assurance(
                    &parsed.method,
                    &parsed.target,
                    &parsed.body,
                    &auth,
                    parse_assurance_headers(&parsed.headers),
                    Utc::now()
                ),
                Err(MobileAuthError::BodyDigestMismatch)
            ));
            assert_eq!(
                dispatch(&f, &tampered).status,
                400,
                "body-digest rejection mapping changed"
            );
            assert!(routing_image(&f, phone.path()) == before);
            let first = dispatch(&f, &wire);
            assert_eq!(first.status, 200, "positive signed request failed");
            let first_data =
                serde_json::from_str::<serde_json::Value>(&first.body).unwrap()["data"].clone();
            let encoded = first.to_http();
            // Discard an entire or truncated serialized reply before delivery.
            let delivered = if partial_reply {
                &encoded[..encoded.len() / 2]
            } else {
                &encoded[..0]
            };
            assert!(
                complete_reply(delivered).is_err(),
                "lost/truncated reply was accepted as complete"
            );
            drop(first);
            if matches!(operation, Operation::Ack) {
                for transport in [Transport::Direct, Transport::Relay] {
                    let (server, client) = exchange_files(&f, phone.path(), transport, false, None);
                    assert!(server.is_ok() && client.is_ok());
                    assert_eq!(counters(&f, phone.path(), transport), (1, 1, 0));
                }
            }
            let committed = routing_image(&f, phone.path());
            assert_eq!(
                dispatch(&f, &wire).status,
                409,
                "exact signed request replay was not rejected"
            );
            assert!(routing_image(&f, phone.path()) == committed);
            let retry = successful(&f, &body); // Fresh nonce, same logical operation.
            match operation {
                Operation::Prepare | Operation::Recover(_) => {
                    assert!(
                        retry == first_data,
                        "retry replaced retained handover material"
                    );
                    let ack = handover(&f, retry, phone.path());
                    let expected = if matches!(operation, Operation::Prepare) {
                        0
                    } else {
                        1
                    };
                    assert_eq!(ack.generation, expected);
                    successful(&f, &ack_body(&ack));
                }
                Operation::Ack => {
                    assert_eq!(retry["version"].as_u64(), Some(2));
                    assert_eq!(retry["generation"].as_u64(), Some(0));
                    assert_eq!(retry["committed"].as_bool(), Some(true));
                    for transport in [Transport::Direct, Transport::Relay] {
                        assert_eq!(counters(&f, phone.path(), transport), (1, 1, 0));
                    }
                }
                Operation::Resync(transport) => {
                    assert_eq!(retry["generation"].as_u64(), Some(0));
                    assert_eq!(retry["floor"].as_u64(), Some(3));
                    let host = crate::mobile_memory::identity::load_existing_host_identity(
                        &f.state.coven_home,
                    )
                    .unwrap();
                    let signer = SigningKey::from_pkcs8_der(&host.private_key_der).unwrap();
                    v2::device::resync_direct(
                        phone.path(),
                        signer.verifying_key(),
                        f.pairing.device_id,
                        f.device.verifying_key(),
                        transport,
                        HostState {
                            generation: 0,
                            floor: 3,
                        },
                    )
                    .unwrap();
                    assert_eq!(counters(&f, phone.path(), transport), (3, 3, 3));
                }
            }
            for transport in [Transport::Direct, Transport::Relay] {
                let (server, client) = exchange_files(&f, phone.path(), transport, false, None);
                assert!(
                    server.is_ok() && client.is_ok(),
                    "reconciled control did not yield a usable channel"
                );
            }
            *count.entry(format!("{operation:?}")).or_insert(0) += 1;
            println!("signed-control-loss operation={operation:?} partial_reply={partial_reply} reconciled");
        }
    }
    println!("signed-control-loss counts={count:?}");
}
#[test]
fn logical_long_offline_reconnect_obeys_grant_validity_without_changing_routing_state_on_refusal() {
    for transport in [Transport::Direct, Transport::Relay] {
        for days in [-1, 120, 180] {
            let f = Fixture::new();
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
                    now + chrono::Duration::days(180),
                )
                .unwrap();
            f.state.registry.replace_grant(id, grant).unwrap();
            let phone = tempfile::tempdir().unwrap();
            let ack = handover(
                &f,
                successful(&f, &serde_json::json!({"version":2,"operation":"prepare"})),
                phone.path(),
            );
            successful(&f, &ack_body(&ack));
            let before = routing_image(&f, phone.path());
            let (server, client) = exchange_files(
                &f,
                phone.path(),
                transport,
                false,
                Some(now + chrono::Duration::days(days)),
            );
            if days == 120 {
                assert!(
                    server.is_ok() && client.is_ok(),
                    "logical offline reconnect failed"
                );
                assert_eq!(counters(&f, phone.path(), transport), (1, 1, 0));
            } else {
                assert_eq!(server.unwrap_err(), "grant inactive");
                assert!(client.is_err());
                assert!(
                    routing_image(&f, phone.path()) == before,
                    "inactive grant advanced routing state"
                );
            }
            f.state.registry.reload().unwrap();
            let record = f.state.registry.authorization_record(id).unwrap().unwrap();
            assert!(record.device.revoked_at.is_none() && record.device.suspended_at.is_none());
            println!(
                "logical-offline transport={transport:?} days={days} accepted={} verified",
                days == 120
            );
        }
    }
}
