//! The adversary's two positions.
//!
//! [`Hostile`] sits between the production device implementation and the relay,
//! where a broker or network attacker sits: it sees only ciphertext records and
//! can withhold, delay, repeat or reorder them. When the device gives up, the
//! adversary keeps the connection to the host open, so the daemon learns nothing
//! from the device's own failure.
//!
//! [`Insider`] is an independent implementation of the device side of the wire
//! contract, written against `snow` and the documented CBOR encodings instead of
//! the production handshake functions. It holds the genuine device key, which is
//! the strongest peer the protocol admits, and it can send well-formed records
//! in an order the production device never would.
use super::*;
use ciborium::value::Value;
use p256::ecdsa::signature::Verifier;
use std::collections::VecDeque;
use std::sync::Condvar;

/// The adversary's own socket budget. It is far above the 30-second handshake
/// budget so that it never decides an outcome; it only bounds a wedged fixture.
const ADVERSARY_IO: Duration = Duration::from_secs(90);

#[derive(Clone, Copy, Debug)]
pub(super) enum Outbound {
    Forward,
    /// Forward once this long has passed since the hosting was first observed.
    At(Duration),
    /// Keep the record and let the device's attempt fail.
    Withhold,
}

#[derive(Default)]
pub(super) struct Tap {
    /// Every record the device handed to the network, forwarded or not.
    pub(super) outbound: Vec<Vec<u8>>,
    /// Host records handed to the device.
    pub(super) inbound: usize,
    delaying: Option<usize>,
    released: bool,
    parked: Option<websocket::WebSocketRecords>,
}
pub(super) type Shared = Arc<(Mutex<Tap>, Condvar)>;

pub(super) struct Hostile {
    inner: Option<websocket::WebSocketRecords>,
    script: Vec<Outbound>,
    /// Hand the device the host's second handshake record before its first.
    swap_first_flight: bool,
    origin: Instant,
    reordered: VecDeque<Vec<u8>>,
    shared: Shared,
}

pub(super) fn hostile(
    inner: websocket::WebSocketRecords,
    script: &[Outbound],
    swap_first_flight: bool,
    origin: Instant,
) -> (Hostile, Shared) {
    let shared = Shared::default();
    (
        Hostile {
            inner: Some(inner),
            script: script.to_vec(),
            swap_first_flight,
            origin,
            reordered: VecDeque::new(),
            shared: Arc::clone(&shared),
        },
        shared,
    )
}

/// The connection the adversary kept after the device's attempt ended.
pub(super) fn kept_connection(shared: &Shared) -> websocket::WebSocketRecords {
    shared
        .0
        .lock()
        .unwrap()
        .parked
        .take()
        .expect("the adversary did not keep the host's connection")
}

pub(super) fn outbound(shared: &Shared) -> Vec<Vec<u8>> {
    shared.0.lock().unwrap().outbound.clone()
}

pub(super) fn inbound(shared: &Shared) -> usize {
    shared.0.lock().unwrap().inbound
}

/// Waits until the adversary is holding the device's record `index` back.
pub(super) fn await_delaying(shared: &Shared, index: usize) {
    let (tap, wake) = &**shared;
    // Hang guard only; the wait ends on the wrapper's own announcement.
    let (tap, timeout) = wake
        .wait_timeout_while(tap.lock().unwrap(), ADVERSARY_IO, |tap| {
            tap.delaying != Some(index)
        })
        .unwrap();
    assert!(
        !timeout.timed_out(),
        "the adversary never held record {index}; holding {:?}",
        tap.delaying
    );
}

/// Ends a delay early: the adversary stops waiting and forwards the record.
pub(super) fn release(shared: &Shared) {
    let (tap, wake) = &**shared;
    tap.lock().unwrap().released = true;
    wake.notify_all();
}

impl Hostile {
    fn forward(&mut self, bytes: &[u8]) -> Result<()> {
        self.inner
            .as_mut()
            .expect("connection is owned until drop")
            .send(bytes, Instant::now() + ADVERSARY_IO)
    }
}

impl Records for Hostile {
    fn send(&mut self, bytes: &[u8], _device_deadline: Instant) -> Result<()> {
        let index = {
            let mut tap = self.shared.0.lock().unwrap();
            tap.outbound.push(bytes.to_vec());
            tap.outbound.len() - 1
        };
        match self.script.get(index).copied().unwrap_or(Outbound::Forward) {
            Outbound::Forward => self.forward(bytes),
            Outbound::At(offset) => {
                let (tap, wake) = &*self.shared;
                let mut tap = tap.lock().unwrap();
                tap.delaying = Some(index);
                wake.notify_all();
                let until = self.origin + offset;
                while !tap.released {
                    let Some(remaining) = until.checked_duration_since(Instant::now()) else {
                        break;
                    };
                    tap = wake.wait_timeout(tap, remaining).unwrap().0;
                }
                tap.delaying = None;
                drop(tap);
                self.forward(bytes)
            }
            Outbound::Withhold => anyhow::bail!("record withheld by the adversary"),
        }
    }

    fn receive(&mut self, deadline: Instant) -> Result<Vec<u8>> {
        if let Some(record) = self.reordered.pop_front() {
            return Ok(record);
        }
        let inner = self.inner.as_mut().expect("connection is owned until drop");
        let first = inner.receive(deadline)?;
        let swap = self.swap_first_flight && self.shared.0.lock().unwrap().inbound == 0;
        if !swap {
            self.shared.0.lock().unwrap().inbound += 1;
            return Ok(first);
        }
        let second = inner.receive(deadline)?;
        self.shared.0.lock().unwrap().inbound += 2;
        self.reordered.push_back(first);
        Ok(second)
    }

    fn receive_idle(&mut self) -> Result<Vec<u8>> {
        anyhow::bail!("the handshake never reaches an idle channel here")
    }
}

impl Drop for Hostile {
    fn drop(&mut self) {
        self.shared.0.lock().unwrap().parked = self.inner.take();
    }
}

/// Blocks until the relay reports that the host left the room, and returns that
/// instant with any records the host sent first. The relay closes the client
/// with `1001 relay peer disconnected` as soon as the host's socket is gone, so
/// the returned instant is never earlier than the host's departure.
pub(super) fn await_host_departure(
    connection: &mut websocket::WebSocketRecords,
) -> (Instant, Vec<Vec<u8>>) {
    let guard = Instant::now() + ADVERSARY_IO;
    let mut records = Vec::new();
    loop {
        match connection.receive(guard) {
            Ok(record) => records.push(record),
            Err(_) => {
                let departed = Instant::now();
                assert!(
                    departed + Duration::from_secs(5) < guard,
                    "the host never left its room: the adversary's own hang guard expired"
                );
                return (departed, records);
            }
        }
    }
}

/// The authenticated host proof as it appears on the wire.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct HostProof {
    pub(super) generation: u64,
    pub(super) host_floor: u64,
    pub(super) counter: u64,
}

pub(super) struct Insider {
    pub(super) connection: websocket::WebSocketRecords,
    cipher: snow::TransportState,
    hash: Vec<u8>,
    pairing: Pairing,
}

fn cbor(fields: Vec<Value>) -> Vec<u8> {
    let mut encoded = Vec::new();
    ciborium::ser::into_writer(&Value::Array(fields), &mut encoded).unwrap();
    encoded
}

impl Insider {
    /// Completes the Noise NK exchange for one relay room.
    pub(super) fn open(
        mut connection: websocket::WebSocketRecords,
        pairing: &Pairing,
        room: &[u8; 32],
    ) -> Result<Self> {
        use crate::mobile_memory::transport::PROFILE;
        let prologue = cbor(vec![
            Value::Text("coven.mobile.connection".into()),
            Value::Integer(1.into()),
            Value::Text(PROFILE.into()),
            Value::Text("relay".into()),
            Value::Bytes(room.to_vec()),
        ]);
        let mut handshake = snow::Builder::new(PROFILE.parse()?)
            .prologue(&prologue)?
            .remote_public_key(&pairing.noise_key)?
            .build_initiator()?;
        let mut message = [0; 128];
        let len = handshake.write_message(&[], &mut message)?;
        connection.send(&message[..len], Instant::now() + ADVERSARY_IO)?;
        let reply = connection.receive(Instant::now() + ADVERSARY_IO)?;
        let mut payload = [0; 128];
        anyhow::ensure!(
            handshake.read_message(&reply, &mut payload)? == 0,
            "host sent an early payload"
        );
        let hash = handshake.get_handshake_hash().to_vec();
        Ok(Self {
            connection,
            cipher: handshake.into_transport_mode()?,
            hash,
            pairing: pairing.clone(),
        })
    }

    /// Encrypts the next record without sending it.
    pub(super) fn seal(&mut self, plaintext: &[u8]) -> Result<Vec<u8>> {
        let mut record = vec![0; plaintext.len() + 16];
        let len = self.cipher.write_message(plaintext, &mut record)?;
        record.truncate(len);
        Ok(record)
    }

    pub(super) fn deliver(&mut self, record: &[u8]) -> Result<()> {
        self.connection.send(record, Instant::now() + ADVERSARY_IO)
    }

    pub(super) fn send(&mut self, plaintext: &[u8]) -> Result<()> {
        let record = self.seal(plaintext)?;
        self.deliver(&record)
    }

    pub(super) fn receive(&mut self) -> Result<Vec<u8>> {
        let record = self.connection.receive(Instant::now() + ADVERSARY_IO)?;
        let mut plaintext = vec![0; record.len()];
        let len = self.cipher.read_message(&record, &mut plaintext)?;
        plaintext.truncate(len);
        Ok(plaintext)
    }

    fn possession(
        &self,
        role: &str,
        host_nonce: &[u8],
        device_nonce: &[u8],
        proof: HostProof,
    ) -> Vec<u8> {
        cbor(vec![
            Value::Text("coven.mobile.possession".into()),
            Value::Integer(2.into()),
            Value::Text(crate::mobile_memory::transport::PROFILE.into()),
            Value::Text(role.into()),
            Value::Bytes(self.hash.clone()),
            Value::Bytes(host_nonce.to_vec()),
            Value::Bytes(device_nonce.to_vec()),
            Value::Bytes(self.pairing.device_id.as_bytes().to_vec()),
            Value::Bytes(self.pairing.grant_id.as_bytes().to_vec()),
            Value::Integer(self.pairing.revocation_epoch.into()),
            Value::Bytes(
                self.pairing
                    .host_key
                    .to_sec1_point(false)
                    .as_bytes()
                    .to_vec(),
            ),
            Value::Bytes(
                self.pairing
                    .device_key
                    .to_sec1_point(false)
                    .as_bytes()
                    .to_vec(),
            ),
            Value::Bytes(self.pairing.noise_key.to_vec()),
            Value::Integer(proof.generation.into()),
            Value::Integer(proof.host_floor.into()),
            Value::Integer(proof.counter.into()),
        ])
    }

    /// Exchanges nonces and returns the host proof after verifying the host's
    /// signature over it, together with this device's own signature over the
    /// same exchange. The signature is returned, not sent.
    pub(super) fn challenge(&mut self, device: &SigningKey) -> Result<(HostProof, Vec<u8>)> {
        let host_nonce = self.receive()?;
        anyhow::ensure!(host_nonce.len() == 32, "invalid host challenge");
        let device_nonce = [0x5a; 32];
        self.send(&device_nonce)?;
        let packet = self.receive()?;
        anyhow::ensure!(packet.len() == 24 + 64, "invalid v2 possession proof");
        let proof = HostProof {
            generation: u64::from_be_bytes(packet[..8].try_into()?),
            host_floor: u64::from_be_bytes(packet[8..16].try_into()?),
            counter: u64::from_be_bytes(packet[16..24].try_into()?),
        };
        self.pairing
            .host_key
            .verify(
                &self.possession("host", &host_nonce, &device_nonce, proof),
                &Signature::from_slice(&packet[24..])?,
            )
            .map_err(|_| anyhow::anyhow!("host proof is not authentic"))?;
        let signature: Signature =
            device.sign(&self.possession("device", &host_nonce, &device_nonce, proof));
        Ok((proof, signature.to_bytes().to_vec()))
    }
}
