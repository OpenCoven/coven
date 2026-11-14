//! The same signed control losses over the production per-connection path: a
//! real TCP socket, a TLS 1.3 client pinned to the host certificate and
//! `serve_tls_connection`, which the gateway's accept loop calls for each socket.
//! Loopback does not drop packets, so the client severs the connection or
//! withholds ciphertext it received. This is neither a shipped mobile client nor
//! a physical network.
use super::*;
use rustls::{
    pki_types::{CertificateDer, ServerName},
    ClientConfig, ClientConnection, RootCertStore,
};
use std::{
    io::{Read, Write},
    net::{Ipv4Addr, Shutdown, TcpListener, TcpStream},
    sync::Arc,
    time::Duration,
};

/// Hang guard only: a client read cannot outlive a parked server. Nothing here
/// asserts a duration.
const HANG_GUARD: Duration = Duration::from_secs(120);
/// The subject name `Fixture` gives the host certificate.
const HOST_NAME: &str = "192.0.2.1";

#[derive(Debug, Clone, Copy)]
enum Close {
    /// TCP FIN without a TLS `close_notify`.
    Fin,
    /// An orderly TLS close, so the server sees a clean end of stream.
    Notify,
    /// Drop the socket with received data unread, which may send a reset.
    Drop,
}

/// Each way a signed request can fail to arrive complete.
#[derive(Debug, Clone, Copy)]
enum RequestLoss {
    BeforeHandshake,
    DuringHandshake,
    /// Authentic TLS records carrying only the first `cut` request bytes.
    Partial {
        cut: usize,
        close: Close,
    },
    /// The record carrying the whole request, cut at half its length on the wire.
    HalfRecord,
    /// The same record without its last byte.
    RecordMissingLastByte,
    /// The whole record with a corrupted last byte.
    CorruptRecord,
}
impl RequestLoss {
    fn every(wire: &[u8]) -> Vec<Self> {
        let header = wire.windows(4).position(|p| p == b"\r\n\r\n").unwrap() + 4;
        let middle = header + (wire.len() - header) / 2;
        let mut losses = vec![Self::BeforeHandshake, Self::DuringHandshake];
        for cut in [header - 1, middle, wire.len() - 1] {
            for close in [Close::Fin, Close::Notify] {
                losses.push(Self::Partial { cut, close });
            }
        }
        losses.push(Self::Partial {
            cut: middle,
            close: Close::Drop,
        });
        losses.extend([
            Self::HalfRecord,
            Self::RecordMissingLastByte,
            Self::CorruptRecord,
        ]);
        losses
    }
    fn sever(self, mut tcp: TcpStream, mut tls: ClientConnection, wire: &[u8]) {
        match self {
            Self::BeforeHandshake => return,
            Self::DuringHandshake => {
                tls.write_tls(&mut tcp).unwrap();
                return;
            }
            Self::Partial { cut, close } => {
                handshake(&mut tcp, &mut tls);
                let notify = matches!(close, Close::Notify);
                tcp.write_all(&ciphertext(&mut tls, &wire[..cut], notify))
                    .unwrap();
                if matches!(close, Close::Drop) {
                    return;
                }
            }
            Self::HalfRecord | Self::RecordMissingLastByte | Self::CorruptRecord => {
                handshake(&mut tcp, &mut tls);
                let mut record = ciphertext(&mut tls, wire, false);
                match self {
                    Self::HalfRecord => record.truncate(record.len() / 2),
                    Self::RecordMissingLastByte => {
                        record.pop();
                    }
                    _ => *record.last_mut().unwrap() ^= 1,
                }
                tcp.write_all(&record).unwrap();
            }
        }
        // Half-close and drain, so unread session tickets cannot turn this close
        // into a reset that hides the intended end of stream from the server.
        tcp.shutdown(Shutdown::Write).unwrap();
        let _ = tcp.read_to_end(&mut Vec::new());
    }
}

/// Each way the reply to a complete request can fail to reach adoption.
#[derive(Debug, Clone, Copy, PartialEq)]
enum ReplyLoss {
    /// The client closes straight after sending. The close can race the request
    /// itself, so dispatch may or may not happen; both outcomes must converge.
    Abort,
    /// All the ciphertext is withheld from the client's TLS stack.
    Whole,
    /// Half of the record carrying the reply is withheld.
    HalfRecord,
    /// Only the reply record's last byte is withheld.
    LastByte,
}
const REPLY_LOSSES: [ReplyLoss; 4] = [
    ReplyLoss::Abort,
    ReplyLoss::Whole,
    ReplyLoss::HalfRecord,
    ReplyLoss::LastByte,
];

fn handshake(tcp: &mut TcpStream, tls: &mut ClientConnection) {
    while tls.is_handshaking() {
        tls.complete_io(tcp).unwrap();
    }
}
fn ciphertext(tls: &mut ClientConnection, plaintext: &[u8], notify: bool) -> Vec<u8> {
    tls.writer().write_all(plaintext).unwrap();
    if notify {
        tls.send_close_notify();
    }
    let mut records = Vec::new();
    while tls.wants_write() {
        tls.write_tls(&mut records).unwrap();
    }
    records
}
/// Everything the TLS stack can release from `received`; a cut record yields
/// nothing until the rest of it arrives.
fn open(tls: &mut ClientConnection, mut received: &[u8]) -> Vec<u8> {
    while !received.is_empty() {
        tls.read_tls(&mut received).unwrap();
        tls.process_new_packets().unwrap();
    }
    let mut plaintext = Vec::new();
    // Ends with `WouldBlock`, keeping what was already read.
    let _ = tls.reader().read_to_end(&mut plaintext);
    plaintext
}
/// Where the last TLS record starts. The server closes without a `close_notify`,
/// so that record is the reply; session tickets come before it.
fn last_record(received: &[u8]) -> usize {
    let (mut next, mut last) = (0, 0);
    while next + 5 <= received.len() {
        last = next;
        next += 5 + usize::from(u16::from_be_bytes([received[next + 3], received[next + 4]]));
    }
    assert_eq!(
        next,
        received.len(),
        "server ciphertext is not record-framed"
    );
    last
}
fn status(reply: &[u8]) -> u16 {
    let line = reply.split(|byte| *byte == b'\r').next().unwrap();
    let line = std::str::from_utf8(line).unwrap();
    line.split_whitespace().nth(1).unwrap().parse().unwrap()
}

struct Https<'a> {
    f: &'a Fixture,
    server: Arc<ServerConfig>,
    client: Arc<ClientConfig>,
}
impl<'a> Https<'a> {
    fn new(f: &'a Fixture) -> Self {
        let identity =
            crate::mobile_memory::identity::load_existing_host_identity(&f.state.coven_home)
                .unwrap();
        let mut roots = RootCertStore::empty();
        roots
            .add(CertificateDer::from(identity.certificate_der.clone()))
            .unwrap();
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let client = ClientConfig::builder_with_provider(provider)
            .with_protocol_versions(&[&rustls::version::TLS13])
            .unwrap()
            .with_root_certificates(roots)
            .with_no_client_auth();
        Self {
            f,
            server: tls_server_config(identity.certificate_der, identity.private_key_der.to_vec())
                .unwrap(),
            client: Arc::new(client),
        }
    }
    /// Runs `client` against one production connection and returns the server's
    /// result once it has finished. The socket connects before the server thread
    /// starts, so a client panic can only unwind past an open socket.
    fn connection<T>(
        &self,
        client: impl FnOnce(TcpStream, ClientConnection) -> T,
    ) -> (T, Result<()>) {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let tcp = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        tcp.set_read_timeout(Some(HANG_GUARD)).unwrap();
        tcp.set_write_timeout(Some(HANG_GUARD)).unwrap();
        let name = ServerName::try_from(HOST_NAME).unwrap();
        let tls = ClientConnection::new(Arc::clone(&self.client), name).unwrap();
        std::thread::scope(|scope| {
            let server = scope.spawn(|| {
                let (stream, _) = listener.accept().unwrap();
                serve_tls_connection(stream, Arc::clone(&self.server), &self.f.state)
            });
            let output = client(tcp, tls);
            (output, server.join().unwrap())
        })
    }
    /// A complete exchange on a fresh connection.
    fn fetch(&self, wire: &[u8]) -> Vec<u8> {
        let (reply, served) = self.connection(|mut tcp, mut tls| {
            handshake(&mut tcp, &mut tls);
            tcp.write_all(&ciphertext(&mut tls, wire, false)).unwrap();
            let mut received = Vec::new();
            let _ = tcp.read_to_end(&mut received);
            open(&mut tls, &received)
        });
        served.unwrap();
        reply
    }
    fn control(&self, body: &serde_json::Value) -> serde_json::Value {
        complete_reply(&self.fetch(&request(self.f, body))).unwrap()
    }
    fn sever_request(&self, phone: &Path, wire: &[u8]) {
        let before = routing_image(self.f, phone);
        for loss in RequestLoss::every(wire) {
            let ((), served) = self.connection(|tcp, tls| loss.sever(tcp, tls, wire));
            assert!(served.is_err(), "{loss:?} reached dispatch");
            assert!(
                routing_image(self.f, phone) == before,
                "{loss:?} changed routing state"
            );
        }
    }
    /// Delivers `wire` and loses its reply. Returns the withheld reply, proving
    /// the bytes kept back did hold a complete successful response.
    fn lose_reply(&self, wire: &[u8], loss: ReplyLoss) -> Option<serde_json::Value> {
        let (withheld, served) = self.connection(|mut tcp, mut tls| {
            handshake(&mut tcp, &mut tls);
            tcp.write_all(&ciphertext(&mut tls, wire, false)).unwrap();
            let received = |tcp: &mut TcpStream| {
                let mut received = Vec::new();
                let _ = tcp.read_to_end(&mut received);
                received
            };
            let (received, keep) = match loss {
                ReplyLoss::Abort => return None,
                ReplyLoss::Whole => (received(&mut tcp), 0),
                ReplyLoss::HalfRecord => {
                    let received = received(&mut tcp);
                    let record = last_record(&received);
                    let keep = record + (received.len() - record) / 2;
                    (received, keep)
                }
                ReplyLoss::LastByte => {
                    let received = received(&mut tcp);
                    let keep = received.len() - 1;
                    (received, keep)
                }
            };
            let mut delivered = open(&mut tls, &received[..keep]);
            assert!(
                complete_reply(&delivered).is_err(),
                "{loss:?} delivered a complete reply"
            );
            delivered.extend(open(&mut tls, &received[keep..]));
            Some(complete_reply(&delivered).unwrap())
        });
        if loss != ReplyLoss::Abort {
            served.unwrap();
        }
        withheld
    }
    /// One control stage: sever the request every way, lose the reply, replay the
    /// exact request, then retry with a fresh nonce on a fresh connection. Returns
    /// that retry, which must equal any reply the loss withheld.
    fn lose(&self, phone: &Path, body: &serde_json::Value, loss: ReplyLoss) -> serde_json::Value {
        let wire = request(self.f, body);
        // Severing never reaches dispatch, so the reply loss does not vary it.
        if loss == ReplyLoss::Whole {
            self.sever_request(phone, &wire);
        }
        let before = routing(self.f, phone);
        let withheld = self.lose_reply(&wire, loss);
        let after = routing(self.f, phone);
        let dispatched = after != before;
        assert!(
            dispatched || loss == ReplyLoss::Abort,
            "{loss:?}: a complete request changed no routing state"
        );
        let replay = status(&self.fetch(&wire));
        if dispatched {
            assert_eq!(replay, 409, "exact replay accepted");
            assert!(routing(self.f, phone) == after, "replay changed state");
        } else {
            // The close beat the request, so this is its first delivery.
            assert_eq!(replay, 200, "undelivered request stayed refused");
        }
        let retry = self.control(body);
        if let Some(withheld) = withheld {
            assert!(retry == withheld, "retry differs from the lost reply");
        }
        println!("native-https loss={loss:?} dispatched_before_replay={dispatched}");
        retry
    }
}

#[test]
fn https_loss_before_any_anchor_converges_to_one_anchor_with_identical_material() {
    for legacy in [false, true] {
        for loss in REPLY_LOSSES {
            let f = Fixture::new();
            let phone = tempfile::tempdir().unwrap();
            let phone = phone.path();
            unanchored(&f, legacy);
            let https = Https::new(&f);

            let offered = https.lose(phone, &prepare_body(), loss);
            assert_eq!(offered["generation"].as_u64(), Some(0));
            assert_eq!(generations(&f), Some((None, None, Some(0))));
            assert!(routing(&f, phone)[2].is_none(), "adopted without a reply");
            inactive(&f);

            // Adoption, including a duplicated reply and a restart from prepare.
            let ack = handover(&f, offered.clone(), phone);
            let adopted = routing(&f, phone);
            let restarted = https.control(&prepare_body());
            assert!(restarted == offered, "restart replaced pending material");
            assert_eq!(handover(&f, restarted, phone).seed_digest, ack.seed_digest);
            assert!(routing(&f, phone) == adopted, "re-adoption changed state");
            inactive(&f);

            let committed = https.lose(phone, &ack_body(&ack), loss);
            assert_eq!(committed["committed"].as_bool(), Some(true));
            assert_eq!(generations(&f), Some((None, Some(0), None)));
            exchange(&f, phone, (1, 1, 0));

            // Retried and late deliveries keep the counters just consumed.
            let consumed = routing(&f, phone);
            assert_eq!(
                https.control(&ack_body(&ack))["committed"].as_bool(),
                Some(true)
            );
            assert_eq!(handover(&f, offered, phone).generation, 0);
            assert!(routing(&f, phone) == consumed, "retry reset counters");
            assert_eq!(generations(&f), Some((None, Some(0), None)));
            exchange(&f, phone, (2, 2, 1));
            println!("native-https-initial legacy={legacy} loss={loss:?} anchor=0 converged");
        }
    }
}

#[test]
fn https_loss_of_a_named_recovery_on_an_unanchored_host_converges_on_one_higher_anchor() {
    for start in [
        Unanchored::NeverProvisioned,
        Unanchored::V1,
        Unanchored::FloorOnly,
    ] {
        for loss in REPLY_LOSSES {
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
            }
            let https = Https::new(&f);

            let offered = https.lose(phone, &recover_named(4), loss);
            assert_eq!(offered["generation"].as_u64(), Some(5));
            assert_eq!(generations(&f), Some((Some(4), None, Some(5))));
            assert!(routing(&f, phone)[2].is_none(), "adopted without a reply");
            inactive(&f);

            let ack = handover(&f, offered, phone);
            let committed = https.lose(phone, &ack_body(&ack), loss);
            assert_eq!(committed["generation"].as_u64(), Some(5));
            assert_eq!(generations(&f), Some((Some(4), Some(5), None)));
            exchange(&f, phone, (1, 1, 0));
            println!("native-https-named start={start:?} loss={loss:?} anchor=5 converged");
        }
    }
}

#[test]
fn https_loss_of_a_resync_reconciles_a_device_ahead_without_counter_reset() {
    for (transport, name) in [(Transport::Direct, "direct"), (Transport::Relay, "relay")] {
        for loss in REPLY_LOSSES {
            let f = Fixture::new();
            let phone = tempfile::tempdir().unwrap();
            let phone = phone.path();
            let https = Https::new(&f);
            let ack = handover(&f, https.control(&prepare_body()), phone);
            https.control(&ack_body(&ack));
            // Three genuine host-proof/device-proof losses put the device ahead.
            for _ in 0..3 {
                let (server, client) = exchange_files(&f, phone, transport, true, None);
                assert!(server.is_err() && client.is_err());
            }
            assert_eq!(counters(&f, phone, transport), (0, 3, 0));

            let body =
                json!({"version":2,"operation":"resync","transport":name,"generation":0,"next":3});
            let reply = https.lose(phone, &body, loss);
            assert_eq!(reply["generation"].as_u64(), Some(0));
            assert_eq!(reply["floor"].as_u64(), Some(3));
            assert_eq!(counters(&f, phone, transport).1, 3, "device counter moved");
            v2::device::resync_direct(
                phone,
                host(&f).verifying_key(),
                f.pairing.device_id,
                f.device.verifying_key(),
                transport,
                HostState {
                    generation: 0,
                    floor: 3,
                },
            )
            .unwrap();
            assert_eq!(counters(&f, phone, transport), (3, 3, 3));
            for transport in [Transport::Direct, Transport::Relay] {
                let (server, client) = exchange_files(&f, phone, transport, false, None);
                assert!(server.is_ok() && client.is_ok(), "{server:?} {client:?}");
            }
            println!("native-https-resync transport={transport:?} loss={loss:?} reconciled");
        }
    }
}

#[test]
fn https_loss_during_rotation_and_recovery_retries_identical_material_without_counter_reset() {
    for loss in REPLY_LOSSES {
        let f = Fixture::new();
        let phone = tempfile::tempdir().unwrap();
        let phone = phone.path();
        let https = Https::new(&f);
        let ack = handover(&f, https.control(&prepare_body()), phone);
        https.control(&ack_body(&ack));
        exchange(&f, phone, (1, 1, 0));

        // Rotation keeps the committed floors until the new generation commits.
        let offered = https.lose(phone, &prepare_body(), loss);
        assert_eq!(offered["generation"].as_u64(), Some(1));
        assert_eq!(generations(&f), Some((None, Some(0), Some(1))));
        assert_eq!(
            host_state(&f).unwrap()["committed"]["floors"],
            json!([1, 1])
        );
        let next = handover(&f, offered, phone);
        https.lose(phone, &ack_body(&next), loss);
        assert_eq!(generations(&f), Some((None, Some(1), None)));
        exchange(&f, phone, (1, 1, 0));

        // The phone lost its state and names no generation.
        for (generation, corrupt) in [(2, false), (3, true)] {
            let path = device_file(&f, phone);
            if corrupt {
                crate::mobile_memory::config::atomic_replace_private(&path, b"{bad-json}").unwrap();
            } else {
                std::fs::remove_file(&path).unwrap();
            }
            let offered = https.lose(phone, &json!({"version":2,"operation":"recover"}), loss);
            assert_eq!(offered["generation"].as_u64(), Some(generation));
            assert_eq!(
                generations(&f),
                Some((None, Some(generation - 1), Some(generation)))
            );
            assert_eq!(
                host_state(&f).unwrap()["committed"]["floors"],
                json!([1, 1])
            );
            let recovered = v2::device::adopt_recovery_direct(
                phone,
                serde_json::from_value(offered).unwrap(),
                host(&f).verifying_key(),
                f.pairing.device_id,
                f.device.verifying_key(),
            )
            .unwrap();
            https.lose(phone, &ack_body(&recovered), loss);
            assert_eq!(generations(&f), Some((None, Some(generation), None)));
            exchange(&f, phone, (1, 1, 0));
        }

        // The host lost its state; the device names the generation it holds.
        for (known, corrupt) in [(3, false), (4, true)] {
            let path = transport_file(&f);
            if corrupt {
                crate::mobile_memory::config::atomic_replace_private(&path, b"{bad-json}").unwrap();
            } else {
                std::fs::remove_file(&path).unwrap();
            }
            let offered = https.lose(phone, &recover_named(known), loss);
            assert_eq!(offered["generation"].as_u64(), Some(known + 1));
            assert_eq!(generations(&f), Some((Some(known), None, Some(known + 1))));
            let next = handover(&f, offered, phone);
            https.lose(phone, &ack_body(&next), loss);
            assert_eq!(generations(&f), Some((Some(known), Some(known + 1), None)));
            exchange(&f, phone, (1, 1, 0));
        }
        println!("native-https-rotation-recovery loss={loss:?} converged");
    }
}
