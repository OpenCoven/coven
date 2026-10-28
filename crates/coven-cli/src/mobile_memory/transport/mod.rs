//! Endpoint encryption shared by relay and direct transports.
//!
//! Routing credentials never enter the authority model. A channel becomes usable
//! only after fresh possession proofs and a caller-owned current-policy check.

use std::time::{Duration, Instant};

use anyhow::{bail, ensure, Context, Result};
use ciborium::value::Value;
use p256::ecdsa::{
    signature::{Signer, Verifier},
    Signature, SigningKey, VerifyingKey,
};
use ring::rand::{SecureRandom, SystemRandom};
use snow::{Builder, HandshakeState, TransportState};
use uuid::Uuid;
use zeroize::Zeroizing;

pub mod authority;
pub mod cancellation;
pub(super) mod diagnostics;
mod message;
mod network;
pub mod storage;
#[cfg(test)]
pub(super) mod test_support;
pub mod websocket;

pub const PROFILE: &str = "Noise_NK_25519_ChaChaPoly_SHA256";
pub const MAX_RECORD_BYTES: usize = 65_535;
pub const MAX_PLAINTEXT_BYTES: usize = MAX_RECORD_BYTES - 16;
const HANDSHAKE_BUDGET: Duration = Duration::from_secs(30);

/// Implementations must bound reads/writes by the supplied absolute deadline.
/// Errors must not include addresses, room identifiers, credentials or payloads.
pub trait Records {
    fn send(&mut self, bytes: &[u8], deadline: Instant) -> Result<()>;
    fn receive(&mut self, deadline: Instant) -> Result<Vec<u8>>;
}

/// Trusted local pairing snapshot; never obtained from relay/discovery metadata.
/// The authority callback must compare it with current durable policy.
#[derive(Clone)]
pub struct Pairing {
    pub device_id: Uuid,
    pub grant_id: Uuid,
    pub revocation_epoch: u64,
    pub host_key: VerifyingKey,
    pub device_key: VerifyingKey,
    pub noise_key: [u8; 32],
}

impl Pairing {
    /// Authenticate a pairing-scoped Noise key with the already-pinned host key.
    /// The caller supplies the current grant/epoch; stale bindings cannot silently
    /// replace that trusted state. The resulting bytes are deterministic CBOR.
    pub fn verify_noise_binding(&self, signature: &[u8]) -> Result<()> {
        verify(&self.host_key, &self.binding_bytes()?, signature)
    }

    pub fn sign_noise_binding(&self, host: &SigningKey) -> Result<Vec<u8>> {
        ensure!(
            host.verifying_key() == &self.host_key,
            "host identity mismatch"
        );
        Ok(sign(host, &self.binding_bytes()?))
    }

    fn fields(&self) -> Vec<Value> {
        vec![
            bytes(self.device_id.as_bytes()),
            bytes(self.grant_id.as_bytes()),
            Value::Integer(self.revocation_epoch.into()),
            bytes(self.host_key.to_sec1_point(false).as_bytes()),
            bytes(self.device_key.to_sec1_point(false).as_bytes()),
            bytes(&self.noise_key),
        ]
    }

    fn binding_bytes(&self) -> Result<Vec<u8>> {
        let mut fields = vec![
            Value::Text("coven.mobile.noise-binding".into()),
            Value::Integer(1.into()),
            Value::Text(PROFILE.into()),
        ];
        fields.extend(self.fields());
        encode(fields)
    }

    fn proof(
        &self,
        role: &str,
        hash: &[u8],
        host_nonce: &[u8],
        device_nonce: &[u8],
    ) -> Result<Vec<u8>> {
        let mut fields = vec![
            Value::Text("coven.mobile.possession".into()),
            Value::Integer(1.into()),
            Value::Text(PROFILE.into()),
            Value::Text(role.into()),
            bytes(hash),
            bytes(host_nonce),
            bytes(device_nonce),
        ];
        fields.extend(self.fields());
        encode(fields)
    }
}

fn bytes(value: &[u8]) -> Value {
    Value::Bytes(value.to_vec())
}

fn encode(fields: Vec<Value>) -> Result<Vec<u8>> {
    // Fixed arrays contain only byte/text strings and unsigned integers: no map
    // ordering, float representations or untrusted recursive CBOR decoding.
    let mut encoded = Vec::new();
    ciborium::ser::into_writer(&Value::Array(fields), &mut encoded)?;
    Ok(encoded)
}

fn sign(key: &SigningKey, message: &[u8]) -> Vec<u8> {
    let signature: Signature = key.sign(message);
    signature.to_bytes().to_vec()
}

fn verify(key: &VerifyingKey, message: &[u8], signature: &[u8]) -> Result<()> {
    let signature = Signature::from_slice(signature)
        .map_err(|_| anyhow::anyhow!("invalid possession proof"))?;
    key.verify(message, &signature)
        .map_err(|_| anyhow::anyhow!("invalid possession proof"))
}

fn nonce() -> Result<[u8; 32]> {
    let mut value = [0; 32];
    SystemRandom::new()
        .fill(&mut value)
        .map_err(|_| anyhow::anyhow!("secure randomness unavailable"))?;
    Ok(value)
}

pub fn generate_noise_key() -> Result<(Zeroizing<Vec<u8>>, [u8; 32])> {
    let key = Builder::new(PROFILE.parse()?).generate_keypair()?;
    Ok((
        Zeroizing::new(key.private),
        key.public
            .try_into()
            .map_err(|_| anyhow::anyhow!("invalid Noise key"))?,
    ))
}

fn prologue(context: &[u8; 32], direct: bool) -> Result<Vec<u8>> {
    encode(vec![
        Value::Text("coven.mobile.connection".into()),
        Value::Integer(1.into()),
        Value::Text(PROFILE.into()),
        Value::Text(if direct { "direct" } else { "relay" }.into()),
        bytes(context),
    ])
}

/// An authenticated encrypted channel. Each request still needs its original
/// signature and current grant/assurance checks at the application boundary.
pub struct Channel<R> {
    records: R,
    cipher: Option<TransportState>,
    pairing: Pairing,
    direct: bool,
}

impl<R: Records> Channel<R> {
    pub fn is_open(&self) -> bool {
        self.cipher.is_some()
    }

    pub fn close(&mut self) {
        self.cipher = None;
    }

    pub fn device_id(&self) -> Uuid {
        self.pairing.device_id
    }

    pub fn pairing(&self) -> &Pairing {
        &self.pairing
    }

    pub fn is_direct(&self) -> bool {
        self.direct
    }

    pub fn send(&mut self, plaintext: &[u8], deadline: Instant) -> Result<()> {
        let result = self.send_inner(plaintext, deadline);
        if result.is_err() {
            self.cipher = None;
        }
        result
    }

    fn send_inner(&mut self, plaintext: &[u8], deadline: Instant) -> Result<()> {
        ensure!(Instant::now() < deadline, "channel deadline expired");
        ensure!(
            plaintext.len() <= MAX_PLAINTEXT_BYTES,
            "plaintext exceeds record limit"
        );
        let cipher = self.cipher.as_mut().context("channel closed")?;
        let mut ciphertext = vec![0; plaintext.len() + 16];
        let len = cipher.write_message(plaintext, &mut ciphertext)?;
        self.records.send(&ciphertext[..len], deadline)
    }

    pub fn receive(&mut self, deadline: Instant) -> Result<Zeroizing<Vec<u8>>> {
        let result = self.receive_inner(deadline);
        if result.is_err() {
            self.cipher = None;
        }
        result
    }

    fn receive_inner(&mut self, deadline: Instant) -> Result<Zeroizing<Vec<u8>>> {
        ensure!(self.cipher.is_some(), "channel closed");
        ensure!(Instant::now() < deadline, "channel deadline expired");
        let ciphertext = self.records.receive(deadline)?;
        ensure!(
            (16..=MAX_RECORD_BYTES).contains(&ciphertext.len()),
            "invalid encrypted record size"
        );
        let mut plaintext = Zeroizing::new(vec![0; ciphertext.len() - 16]);
        let len = self
            .cipher
            .as_mut()
            .context("channel closed")?
            .read_message(&ciphertext, &mut plaintext)?;
        plaintext.truncate(len);
        Ok(plaintext)
    }
}

fn handshake_send(
    records: &mut impl Records,
    handshake: &mut HandshakeState,
    deadline: Instant,
) -> Result<()> {
    let mut message = [0; 128];
    let len = handshake.write_message(&[], &mut message)?;
    records.send(&message[..len], deadline)
}

fn handshake_receive(
    records: &mut impl Records,
    handshake: &mut HandshakeState,
    deadline: Instant,
) -> Result<()> {
    let message = records.receive(deadline)?;
    ensure!(message.len() <= 128, "oversized handshake");
    let mut payload = [0; 128];
    ensure!(
        handshake.read_message(&message, &mut payload)? == 0,
        "early payload forbidden"
    );
    Ok(())
}

/// The binding signature must come from authenticated provisioning, never TOFU.
pub fn connect<R: Records>(
    mut records: R,
    pairing: &Pairing,
    noise_binding: &[u8],
    device: &SigningKey,
    context: &[u8; 32],
    direct: bool,
) -> Result<Channel<R>> {
    pairing.verify_noise_binding(noise_binding)?;
    ensure!(
        device.verifying_key() == &pairing.device_key,
        "device identity mismatch"
    );
    let deadline = Instant::now() + HANDSHAKE_BUDGET;
    let prologue = prologue(context, direct)?;
    let mut handshake = Builder::new(PROFILE.parse()?)
        .prologue(&prologue)?
        .remote_public_key(&pairing.noise_key)?
        .build_initiator()?;
    handshake_send(&mut records, &mut handshake, deadline)?;
    handshake_receive(&mut records, &mut handshake, deadline)?;
    let hash = handshake.get_handshake_hash().to_vec();
    let mut channel = Channel {
        records,
        cipher: Some(handshake.into_transport_mode()?),
        pairing: pairing.clone(),
        direct,
    };
    let host_nonce = channel.receive(deadline)?;
    ensure!(host_nonce.len() == 32, "invalid host challenge");
    let device_nonce = nonce()?;
    channel.send(&device_nonce, deadline)?;
    let host_proof = channel.receive(deadline)?;
    verify(
        &pairing.host_key,
        &pairing.proof("host", &hash, &host_nonce, &device_nonce)?,
        &host_proof,
    )?;
    channel.send(
        &sign(
            device,
            &pairing.proof("device", &hash, &host_nonce, &device_nonce)?,
        ),
        deadline,
    )?;
    ensure!(
        channel.receive(deadline)?.as_slice() == b"host-finished-v1",
        "invalid completion"
    );
    channel.send(b"device-finished-v1", deadline)?;
    Ok(channel)
}

/// `current_policy` is invoked before handshake, after device proof, and after
/// completion. It must reload authoritative registry/grant state and reject any
/// suspension, revocation, expiry, rotation or restriction incompatible with the
/// pairing snapshot and transport. It must not merely consult cached state.
pub fn accept<R: Records>(
    mut records: R,
    pairing: &Pairing,
    noise_private: &[u8],
    host: &SigningKey,
    context: &[u8; 32],
    direct: bool,
    mut current_policy: impl FnMut(&Pairing, bool) -> Result<()>,
) -> Result<Channel<R>> {
    current_policy(pairing, direct)?;
    ensure!(
        host.verifying_key() == &pairing.host_key,
        "host identity mismatch"
    );
    let deadline = Instant::now() + HANDSHAKE_BUDGET;
    let prologue = prologue(context, direct)?;
    let mut handshake = Builder::new(PROFILE.parse()?)
        .prologue(&prologue)?
        .local_private_key(noise_private)?
        .build_responder()?;
    handshake_receive(&mut records, &mut handshake, deadline)?;
    handshake_send(&mut records, &mut handshake, deadline)?;
    let hash = handshake.get_handshake_hash().to_vec();
    let mut channel = Channel {
        records,
        cipher: Some(handshake.into_transport_mode()?),
        pairing: pairing.clone(),
        direct,
    };
    let host_nonce = nonce()?;
    channel.send(&host_nonce, deadline)?;
    let device_nonce = channel.receive(deadline)?;
    ensure!(device_nonce.len() == 32, "invalid device challenge");
    channel.send(
        &sign(
            host,
            &pairing.proof("host", &hash, &host_nonce, &device_nonce)?,
        ),
        deadline,
    )?;
    let device_proof = channel.receive(deadline)?;
    verify(
        &pairing.device_key,
        &pairing.proof("device", &hash, &host_nonce, &device_nonce)?,
        &device_proof,
    )?;
    current_policy(pairing, direct)?;
    channel.send(b"host-finished-v1", deadline)?;
    if channel.receive(deadline)?.as_slice() != b"device-finished-v1" {
        bail!("invalid completion");
    }
    current_policy(pairing, direct)?;
    Ok(channel)
}

#[cfg(test)]
mod tests;
